//! Operator toggles for which sketches the next/previous cycle visits.
//!
//! The pedestal's two physical buttons are wired to the `NavigateNext` /
//! `NavigatePrev` keyboard shortcuts, and an installation environment is not
//! always set up for every sketch (no Leap controller for the hand sketches,
//! a room where a particular look doesn't land). These per-sketch toggles let
//! the operator drop sketches from the button cycle without touching the
//! build. Scope is deliberately the *cycle only*: the Home picker tiles and
//! the number-key direct selects still reach a disabled sketch — an explicit
//! choice by the operator at the keyboard is honored; the walk-up visitor
//! mashing the pedestal buttons only sees the curated set.
//!
//! Data flow: [`SketchCycleSettings`] is a normal persisted settings resource
//! (`[sketch_cycle]` in `sketch-settings.toml`, Display tab of the settings
//! dock). `nav::handle_navigation_actions` (the buttons) and the debug-only
//! soak cycler read it live at press/cycle time via
//! [`SketchCycleSettings::next_enabled`] /
//! [`SketchCycleSettings::prev_enabled`] — no restart, no sync system. With
//! every sketch
//! disabled the walk returns `None` and next/prev become no-ops (the
//! operator's explicit configuration is honored rather than second-guessed;
//! Home and direct selects still work).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use wc_core_macros::SketchSettings;

use super::state::AppState;

/// Per-sketch inclusion toggles for the next/previous sketch cycle.
///
/// One `bool` per [`AppState::SKETCH_ORDER`] entry, all defaulting to
/// enabled. A fixed field per sketch (rather than a keyed map) matches the
/// house settings machinery — the sketch set is a closed enum, and the
/// `cycle_toggles_cover_sketch_order` test pins the two in sync.
// One independent, persisted toggle per sketch — a checkbox row, not a state
// machine; the same shape (and allow) as `RadianceSettings`' debug toggles.
#[allow(clippy::struct_excessive_bools)]
#[derive(SketchSettings, Resource, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Resource, Default)]
#[settings(storage_key = "sketch_cycle")]
pub struct SketchCycleSettings {
    /// Include Line in the next/previous cycle.
    #[setting(
        default = true,
        ty = Boolean,
        section = "Sketch Cycle",
        category = User,
        label = "Cycle: Line"
    )]
    #[serde(default = "default_enabled")]
    pub enable_line: bool,

    /// Include Flame in the next/previous cycle.
    #[setting(
        default = true,
        ty = Boolean,
        section = "Sketch Cycle",
        category = User,
        label = "Cycle: Flame"
    )]
    #[serde(default = "default_enabled")]
    pub enable_flame: bool,

    /// Include Dots in the next/previous cycle.
    #[setting(
        default = true,
        ty = Boolean,
        section = "Sketch Cycle",
        category = User,
        label = "Cycle: Dots"
    )]
    #[serde(default = "default_enabled")]
    pub enable_dots: bool,

    /// Include Cymatics in the next/previous cycle.
    #[setting(
        default = true,
        ty = Boolean,
        section = "Sketch Cycle",
        category = User,
        label = "Cycle: Cymatics"
    )]
    #[serde(default = "default_enabled")]
    pub enable_cymatics: bool,

    /// Include Radiance in the next/previous cycle.
    #[setting(
        default = true,
        ty = Boolean,
        section = "Sketch Cycle",
        category = User,
        label = "Cycle: Radiance"
    )]
    #[serde(default = "default_enabled")]
    pub enable_radiance: bool,
}

impl SketchCycleSettings {
    /// Whether `sketch` is included in the cycle. Non-cycle states (`Home`,
    /// the reserved `Waves` seam) are never "enabled" — they are not cycle
    /// stops.
    #[must_use]
    pub fn enabled(&self, sketch: AppState) -> bool {
        match sketch {
            AppState::Line => self.enable_line,
            AppState::Flame => self.enable_flame,
            AppState::Dots => self.enable_dots,
            AppState::Cymatics => self.enable_cymatics,
            AppState::Radiance => self.enable_radiance,
            AppState::Home | AppState::Waves => false,
        }
    }

    /// The next enabled sketch after `from` in [`AppState::SKETCH_ORDER`]
    /// (wrapping), or `None` when every sketch is disabled. From `Home` (or
    /// any non-cycle state) the walk starts at the first entry, matching
    /// `AppState::next_sketch`'s `Home → Line` behavior when everything is
    /// enabled.
    #[must_use]
    pub fn next_enabled(&self, from: AppState) -> Option<AppState> {
        self.walk(from, Direction::Forward)
    }

    /// The previous enabled sketch before `from` (wrapping), or `None` when
    /// every sketch is disabled. From `Home` the walk starts at the last
    /// entry, matching `AppState::prev_sketch`'s `Home → Radiance`.
    #[must_use]
    pub fn prev_enabled(&self, from: AppState) -> Option<AppState> {
        self.walk(from, Direction::Backward)
    }

    /// Walk [`AppState::SKETCH_ORDER`] one full lap from `from`, returning
    /// the first enabled entry strictly after (forward) or before (backward)
    /// `from`'s position. A disabled *current* sketch is simply not a
    /// possible return value, so pressing next while parked on a
    /// just-disabled sketch moves to the nearest enabled neighbour.
    fn walk(&self, from: AppState, dir: Direction) -> Option<AppState> {
        let order = &AppState::SKETCH_ORDER;
        let len = order.len();
        // Home (or Waves) is "before the first / after the last": one step
        // lands on index 0 forward, the last index backward.
        let start = order.iter().position(|s| *s == from);
        for step in 1..=len {
            let idx = match (dir, start) {
                (Direction::Forward, Some(i)) => (i + step) % len,
                (Direction::Forward, None) => step - 1,
                (Direction::Backward, Some(i)) => (i + len - (step % len)) % len,
                (Direction::Backward, None) => len - step,
            };
            if self.enabled(order[idx]) {
                return Some(order[idx]);
            }
        }
        None
    }
}

/// Walk direction for [`SketchCycleSettings::walk`].
#[derive(Clone, Copy)]
enum Direction {
    /// `next_enabled`: ascending [`AppState::SKETCH_ORDER`] order.
    Forward,
    /// `prev_enabled`: descending order.
    Backward,
}

/// Serde fallback: every sketch defaults into the cycle (matching the
/// pre-setting behavior where the cycle always visited all five).
fn default_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::trait_def::SketchSettings as _;

    #[test]
    fn storage_key_is_stable() {
        assert_eq!(SketchCycleSettings::STORAGE_KEY, "sketch_cycle");
    }

    /// Drift guard: every [`AppState::SKETCH_ORDER`] entry must have a
    /// toggle (adding a sketch to the order without a toggle would silently
    /// pin it always-enabled through the `Home | Waves => false` arm).
    #[test]
    fn cycle_toggles_cover_sketch_order() {
        let all_on = SketchCycleSettings::default();
        for sketch in AppState::SKETCH_ORDER {
            assert!(
                all_on.enabled(sketch),
                "{sketch:?} is in SKETCH_ORDER but its default toggle reads disabled — \
                 missing field in SketchCycleSettings::enabled?"
            );
        }
    }

    /// With everything enabled (the default), the filtered walk agrees with
    /// the unfiltered `next_sketch`/`prev_sketch` on every cycle state —
    /// the setting is invisible until an operator turns something off.
    #[test]
    fn all_enabled_matches_the_unfiltered_cycle() {
        let s = SketchCycleSettings::default();
        for from in AppState::SKETCH_ORDER {
            assert_eq!(s.next_enabled(from), Some(from.next_sketch()), "{from:?}");
            assert_eq!(s.prev_enabled(from), Some(from.prev_sketch()), "{from:?}");
        }
        assert_eq!(s.next_enabled(AppState::Home), Some(AppState::Line));
        assert_eq!(s.prev_enabled(AppState::Home), Some(AppState::Radiance));
    }

    /// A disabled sketch is skipped in both directions, including across the
    /// wrap-around seam.
    #[test]
    fn disabled_sketch_is_skipped_and_wraps() {
        let s = SketchCycleSettings {
            enable_flame: false,
            ..SketchCycleSettings::default()
        };
        // Line → (Flame skipped) → Dots.
        assert_eq!(s.next_enabled(AppState::Line), Some(AppState::Dots));
        // Dots ← (Flame skipped) ← Line.
        assert_eq!(s.prev_enabled(AppState::Dots), Some(AppState::Line));
        // Wrap seam: Radiance → Line forward, Line → Radiance backward.
        assert_eq!(s.next_enabled(AppState::Radiance), Some(AppState::Line));
        assert_eq!(s.prev_enabled(AppState::Line), Some(AppState::Radiance));
    }

    /// Pressing next while parked on a sketch that was just disabled moves
    /// to the nearest enabled neighbour (the current sketch is never a
    /// candidate).
    #[test]
    fn current_sketch_disabled_moves_to_nearest_enabled() {
        let s = SketchCycleSettings {
            enable_dots: false,
            ..SketchCycleSettings::default()
        };
        assert_eq!(s.next_enabled(AppState::Dots), Some(AppState::Cymatics));
        assert_eq!(s.prev_enabled(AppState::Dots), Some(AppState::Flame));
    }

    /// All toggles off: the walk yields `None` (the handlers no-op rather
    /// than second-guessing the operator's explicit configuration).
    #[test]
    fn all_disabled_yields_none() {
        let s = SketchCycleSettings {
            enable_line: false,
            enable_flame: false,
            enable_dots: false,
            enable_cymatics: false,
            enable_radiance: false,
        };
        for from in AppState::SKETCH_ORDER {
            assert_eq!(s.next_enabled(from), None, "{from:?}");
            assert_eq!(s.prev_enabled(from), None, "{from:?}");
        }
        assert_eq!(s.next_enabled(AppState::Home), None);
    }

    /// One survivor: the walk finds it from anywhere, in both directions —
    /// and from the survivor itself it wraps a full lap back to the
    /// survivor (the only enabled stop).
    #[test]
    fn single_enabled_sketch_is_found_from_anywhere() {
        let s = SketchCycleSettings {
            enable_line: false,
            enable_flame: false,
            enable_dots: false,
            enable_cymatics: true,
            enable_radiance: false,
        };
        for from in AppState::SKETCH_ORDER {
            assert_eq!(s.next_enabled(from), Some(AppState::Cymatics), "{from:?}");
            assert_eq!(s.prev_enabled(from), Some(AppState::Cymatics), "{from:?}");
        }
    }

    /// Forward-compat: TOML persisted before these toggles existed (an empty
    /// `[sketch_cycle]` table, or none at all) parses with every sketch
    /// enabled.
    #[test]
    #[allow(
        clippy::expect_used,
        reason = "test-only: panic on bad TOML is the intended failure mode"
    )]
    fn legacy_toml_without_keys_enables_everything() {
        let parsed: SketchCycleSettings = toml::from_str("").expect("empty TOML must parse");
        assert_eq!(parsed, SketchCycleSettings::default());
        assert!(parsed.enable_line && parsed.enable_radiance);
    }
}
