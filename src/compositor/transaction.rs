//! Transaction-based configure synchronization.
//!
//! A layout change can resize several clients at once. Each client answers a
//! configure on its own commit, so presenting immediately would show a mixed
//! generation: some windows at the new size, others still stretched from the
//! old buffer. A [`Transaction`] names the windows that still owe a commit; the
//! backend holds the next frame while one is blocking, so the first frame after
//! the change contains every client's answer.
//!
//! A client that ignores configure must not stall the rest of the session, so
//! every transaction has a deadline ([`TIME_LIMIT`], 300 ms). Expiry releases
//! the frame with whatever has been committed; the existing `Configure` and
//! `WindowFit` fallback then draws the laggard into its rectangle.
//!
//! This module is part of the binary, not the pure library: it is only about
//! synchronizing Smithay clients and has no place in `crab::{core, layout}`.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crab::core::WindowId;

/// How long a transaction waits for clients before presenting anyway.
///
/// Long enough for a normal client to answer a configure on a loaded system,
/// short enough that a client that ignores configure never makes the session
/// feel stuck. niri uses the same 300 ms bound.
pub(crate) const TIME_LIMIT: Duration = Duration::from_millis(300);

/// One layout change waiting for the clients it configured.
#[derive(Clone, Debug)]
struct Transaction {
    /// Windows that have not yet committed the state they were asked for.
    blockers: HashSet<WindowId>,
    /// When to stop waiting, whether or not the blockers answered.
    deadline: Instant,
}

/// The transactions currently holding the next frame.
///
/// Newer layout changes are queued rather than merged, so each gets its own
/// deadline: a client that keeps changing layout cannot extend an older hold
/// indefinitely. A commit releases its window from every transaction, because
/// "caught up" is a property of the window, not of one change.
#[derive(Clone, Debug, Default)]
pub(crate) struct TransactionQueue {
    pending: Vec<Transaction>,
}

impl TransactionQueue {
    /// Starts a transaction for `blockers`, or does nothing when none are left.
    pub(crate) fn begin(&mut self, blockers: impl IntoIterator<Item = WindowId>, now: Instant) {
        let blockers: HashSet<WindowId> = blockers.into_iter().collect();
        if blockers.is_empty() {
            return;
        }
        self.pending.push(Transaction {
            blockers,
            deadline: now + TIME_LIMIT,
        });
    }

    /// Adds `blockers` to the newest transaction, if one is still pending.
    ///
    /// A commit-driven re-layout can discover another window that needs a
    /// configure. It must not start a fresh hold of its own (that would make
    /// every commit wait again), but it should keep the current one open.
    pub(crate) fn add_blockers(&mut self, blockers: impl IntoIterator<Item = WindowId>) {
        let Some(transaction) = self.pending.last_mut() else {
            return;
        };
        transaction.blockers.extend(blockers);
    }

    /// Releases `window` from every transaction.
    ///
    /// Called once the window has committed the size it was asked for (or has
    /// given up on reaching it), so it is no longer holding a mixed frame.
    pub(crate) fn commit(&mut self, window: WindowId) {
        for transaction in &mut self.pending {
            transaction.blockers.remove(&window);
        }
        self.pending
            .retain(|transaction| !transaction.blockers.is_empty());
    }

    /// Drops transactions whose deadline has passed, returning how many.
    pub(crate) fn expire(&mut self, now: Instant) -> usize {
        let before = self.pending.len();
        self.pending
            .retain(|transaction| now < transaction.deadline);
        before - self.pending.len()
    }

    /// Whether any client is still holding the next frame.
    pub(crate) fn is_blocking(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The earliest deadline among pending transactions.
    #[cfg(test)]
    pub(crate) fn earliest_deadline(&self) -> Option<Instant> {
        self.pending
            .iter()
            .map(|transaction| transaction.deadline)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u64) -> WindowId {
        WindowId::new(index)
    }

    #[test]
    fn a_commit_releases_only_its_own_window() {
        let now = Instant::now();
        let mut queue = TransactionQueue::default();
        queue.begin([id(1), id(2)], now);

        assert!(queue.is_blocking());
        queue.commit(id(1));
        assert!(
            queue.is_blocking(),
            "one client has not committed its new size yet"
        );
        queue.commit(id(2));
        assert!(!queue.is_blocking(), "both clients released the frame");
    }

    #[test]
    fn a_commit_releases_a_window_from_every_transaction() {
        let now = Instant::now();
        let mut queue = TransactionQueue::default();
        queue.begin([id(1), id(2)], now);
        queue.begin([id(2), id(3)], now + Duration::from_millis(1));

        queue.commit(id(2));
        assert_eq!(queue.earliest_deadline(), Some(now + TIME_LIMIT));
        assert!(
            queue.is_blocking(),
            "the first transaction still waits on 1"
        );

        queue.commit(id(1));
        assert!(
            queue.is_blocking(),
            "the second transaction still waits on 3"
        );
        queue.commit(id(3));
        assert!(!queue.is_blocking());
    }

    #[test]
    fn the_deadline_releases_a_client_that_never_commits() {
        let now = Instant::now();
        let mut queue = TransactionQueue::default();
        queue.begin([id(1), id(2)], now);
        queue.commit(id(1));

        assert_eq!(queue.expire(now + TIME_LIMIT / 2), 0);
        assert!(queue.is_blocking());
        assert_eq!(queue.expire(now + TIME_LIMIT), 1);
        assert!(
            !queue.is_blocking(),
            "the laggard is drawn into its rectangle instead of stalling"
        );
    }

    #[test]
    fn a_newer_transaction_does_not_extend_an_older_deadline() {
        let now = Instant::now();
        let mut queue = TransactionQueue::default();
        queue.begin([id(1)], now);
        queue.begin([id(2)], now + Duration::from_millis(100));

        assert_eq!(queue.expire(now + TIME_LIMIT), 1);
        assert!(
            queue.is_blocking(),
            "the newer transaction keeps its own, later deadline"
        );
        assert_eq!(
            queue.earliest_deadline(),
            Some(now + Duration::from_millis(100) + TIME_LIMIT)
        );
    }

    #[test]
    fn a_commit_driven_blocker_joins_the_current_transaction() {
        let now = Instant::now();
        let mut queue = TransactionQueue::default();
        queue.begin([id(1)], now);
        queue.add_blockers([id(2)]);

        queue.commit(id(1));
        assert!(
            queue.is_blocking(),
            "the window discovered by the re-layout still holds the frame"
        );
        queue.commit(id(2));
        assert!(!queue.is_blocking());
    }

    #[test]
    fn an_empty_blocker_set_does_not_hold_the_frame() {
        let now = Instant::now();
        let mut queue = TransactionQueue::default();
        queue.begin([], now);
        assert!(!queue.is_blocking());
        assert_eq!(queue.earliest_deadline(), None);
    }
}
