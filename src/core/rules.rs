//! Window rules: matching a window at map time and applying actions.
//!
//! M2 ships the pure matching and action model (`COMPOSITOR.md` §13.4) so
//! transient/dialog windows can be forced to float. Loading them from config
//! and re-evaluating on title/app-id change arrives with M5.

use crate::core::{SpaceSet, Window};

/// One rule: a matcher plus the actions it forces.
///
/// A matcher field of `None` is a wildcard. Every present matcher must agree
/// for the rule to apply; actions are applied verbatim.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowRule {
    /// Match windows whose app id contains this text (case-insensitive).
    pub app_id: Option<String>,
    /// Match windows whose title contains this text (case-insensitive).
    pub title: Option<String>,
    /// Force the window to float (or stay tiled).
    pub floating: Option<bool>,
    /// Force the window onto exactly these spaces.
    pub spaces: Option<SpaceSet>,
    /// Pin the window to every space.
    pub pinned: bool,
    /// Draw the window above other floating windows.
    pub always_on_top: bool,
    /// Exclude the window from keyboard focus.
    pub no_focus: bool,
}

impl WindowRule {
    /// Whether the matcher part of this rule agrees with `window`.
    #[must_use]
    pub fn matches(&self, window: &Window) -> bool {
        let text_matches = |needle: Option<&str>, haystack: Option<&String>| {
            needle.is_none_or(|needle| {
                haystack.is_some_and(|haystack| {
                    haystack.to_lowercase().contains(&needle.to_lowercase())
                })
            })
        };
        text_matches(self.app_id.as_deref(), window.app_id.as_ref())
            && text_matches(self.title.as_deref(), window.title.as_ref())
    }
}

/// Applies the first matching rule for each property of `window`.
///
/// Rules are scanned in order; a later rule never overwrites an earlier one,
/// so precedence is "first match wins" and can be reasoned about statically.
pub fn apply(window: &mut Window, rules: &[WindowRule]) {
    let mut applied = crate::core::AppliedRules::default();

    for rule in rules {
        if !rule.matches(window) {
            continue;
        }
        if applied.floating.is_none() {
            applied.floating = rule.floating;
        }
        if applied.spaces.is_none() {
            applied.spaces = rule.spaces;
        }
        applied.pinned |= rule.pinned;
        applied.always_on_top |= rule.always_on_top;
        applied.no_focus |= rule.no_focus;
    }

    if let Some(spaces) = applied.spaces {
        window.set_spaces(spaces);
    }
    if let Some(floating) = applied.floating {
        // Remember the current geometry so un-floating later is lossless; the
        // caller supplies the real rect, so a zero rect is a placeholder.
        window.set_floating(floating, crate::core::Rect::default());
    }
    window.pinned |= applied.pinned;
    window.always_on_top |= applied.always_on_top;
    window.rules = applied;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{OutputId, Rect, WindowId};

    fn window() -> Window {
        Window::new(WindowId::new(1), OutputId::new(0), SpaceSet::SPACE_1)
    }

    #[test]
    fn wildcards_match_everything() {
        assert!(WindowRule::default().matches(&window()));
    }

    #[test]
    fn app_id_matches_by_substring_case_insensitively() {
        let mut w = window();
        w.app_id = Some("org.gnome.Ptyxis".to_owned());
        let rule = WindowRule {
            app_id: Some("gnome".to_owned()),
            ..Default::default()
        };
        assert!(rule.matches(&w));
        assert!(
            !WindowRule {
                app_id: Some("kitty".to_owned()),
                ..Default::default()
            }
            .matches(&w)
        );
    }

    #[test]
    fn applies_first_match_wins() {
        let rules = [
            WindowRule {
                app_id: Some("foot".to_owned()),
                floating: Some(true),
                spaces: Some(SpaceSet::SPACE_2),
                ..Default::default()
            },
            WindowRule {
                app_id: Some("foot".to_owned()),
                floating: Some(false),
                pinned: true,
                ..Default::default()
            },
        ];
        let mut w = window();
        w.app_id = Some("org.codeberg.dnkl.foot".to_owned());
        apply(&mut w, &rules);
        assert_eq!(w.rules.floating, Some(true), "first floating wins");
        assert!(w.pinned, "or-accumulating actions merge");
        assert_eq!(w.spaces, SpaceSet::SPACE_2);
        assert_eq!(w.float_geometry, Some(Rect::default()));
    }

    #[test]
    fn no_match_leaves_the_window_alone() {
        let rules = [WindowRule {
            app_id: Some("kitty".to_owned()),
            floating: Some(true),
            ..Default::default()
        }];
        let mut w = window();
        apply(&mut w, &rules);
        assert!(!w.floating);
        assert!(w.rules.is_empty());
    }
}
