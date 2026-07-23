//! Operator body-tracking behaviour (storage key `"body_tracking"`),
//! rendered on the settings dock's Camera tab.
//!
//! Distinct from the hardware-facing webcam/OBSBOT storages on the same tab:
//! these knobs shape what the tracker does with the frames, not which device
//! produces them. Changes hot-apply: `input::body::systems::
//! restart_worker_on_max_figures_change` bounces the worker on a value diff.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use wc_core_macros::SketchSettings;

/// How the body-tracking pipeline divides its attention between people.
#[derive(
    SketchSettings, Resource, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq, Eq,
)]
#[reflect(Resource, Default)]
#[settings(storage_key = "body_tracking")]
pub struct BodyTrackingSettings {
    /// Maximum people tracked as figures at once (the pipeline's
    /// `max_tracked_bodies` cap). 1 featuring a single dancer keeps every
    /// inference frame on that dancer; 3–4 trade per-figure update rate for
    /// crowd coverage (the worker round-robins landmark inference above 2
    /// active tracks).
    #[setting(
        default = 4_i32,
        min = 1_i32,
        max = 4_i32,
        step = 1_i32,
        category = User,
        section = "Body tracking",
        label = "Max figures"
    )]
    #[serde(default = "default_max_figures")]
    pub max_figures: i32,
}

impl BodyTrackingSettings {
    /// The cap as the pipeline's `usize`, clamped to the valid slot range.
    #[must_use]
    pub fn max_tracked_bodies(&self) -> usize {
        usize::try_from(self.max_figures.clamp(1, 4)).unwrap_or(1)
    }
}

/// Serde fallback: all slots, matching the setting default (pre-setting
/// behaviour) so older settings files load unchanged.
fn default_max_figures() -> i32 {
    4
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "expect is appropriate in test code")]
mod tests {
    use super::*;

    /// Serde defaults match the setting defaults (the house invariant), and a
    /// pre-existing settings file without the storage loads the default.
    #[test]
    fn legacy_empty_toml_matches_defaults() {
        let parsed: BodyTrackingSettings = toml::from_str("").expect("empty TOML must parse");
        assert_eq!(parsed, BodyTrackingSettings::default());
        assert_eq!(parsed.max_figures, 4);
        assert_eq!(parsed.max_tracked_bodies(), 4);
    }

    /// The pipeline-facing accessor clamps out-of-range persisted values
    /// (hand-edited settings files) instead of feeding them to the worker.
    #[test]
    fn max_tracked_bodies_clamps_to_slot_range() {
        let mut s = BodyTrackingSettings::default();
        s.max_figures = 0;
        assert_eq!(s.max_tracked_bodies(), 1);
        s.max_figures = 99;
        assert_eq!(s.max_tracked_bodies(), 4);
        s.max_figures = 2;
        assert_eq!(s.max_tracked_bodies(), 2);
    }
}
