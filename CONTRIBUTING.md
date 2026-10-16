# Contributing to crawm

## Ground rules

1. **Respect the crate boundaries.** `crawm-core`, `crawm-layout`, `crawm-anim`, and
   `crawm-config` must not depend on Smithay or any Wayland crate; `crawm-ipc` must
   not depend on any other `crawm-*` crate. Run `scripts/check-boundaries.sh` (also
   enforced in CI) before opening a PR.
2. **Layout, animation, and config logic is pure and tested.** No globals, no
   `Instant::now()` inside animation stepping, no I/O in layout code.
3. **Never block the event loop.** No synchronous file I/O or D-Bus waits inside
   calloop handlers; hand work to a thread and return via a channel.

## Attribution (important)

crawm is GPL-3.0-or-later (ADR-0001) and deliberately adapts code from other
GPL-3.0 projects. When you copy or substantially adapt code:

- add a header comment to the file naming the upstream project, the file/commit, and
  the license;
- add the project to [`NOTICE`](./NOTICE) if it is not already listed;
- never paste code whose license is incompatible with GPL-3.0.

## Commits & PRs

- Keep commits focused; describe *why* in the message body.
- `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`
  must pass.
- Update the relevant ADR / `ROADMAP.md` / `TODO.md` when you change a decision or
  finish a task.
