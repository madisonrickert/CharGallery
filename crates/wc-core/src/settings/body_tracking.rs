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

/// Quarter-turn applied to every incoming camera frame before anything else
/// looks at it.
///
/// ## Why this lives here
///
/// It is an operator knob ([`BodyTrackingSettings::camera_rotation`]) whose
/// whole meaning is geometric, so the turn arithmetic lives on the type
/// rather than being restated at each use. It sits in `settings` rather than
/// beside the pipeline that consumes it because `crate::input::body` is
/// gated behind `body-tracking-mediapipe` and this module is not — a gated
/// home would make the setting's own type vanish from a default-feature
/// build.
///
/// ## Why the pipeline rotates instead of the consumer
///
/// A portrait install wants a portrait *sensor*, and the cheapest way to get
/// one is to physically mount a 16:9 camera on its side. That trades the
/// sensor's short axis for its long one: the same lens now covers ~1.8× the
/// vertical field of view, which on a 9:16 panel is the difference between
/// framing a dancer's torso and framing all of them.
///
/// The frame arrives from the driver still in landscape, so it has to be
/// turned back upright somewhere. `body::pipeline::PoseInference::process`
/// does it first — before `CropRect`, before `ContentRect`, before the square
/// pad — which is what keeps every downstream geometry honest: the pre-crop
/// reasons about the *rotated* aspect (a portrait frame on a portrait display
/// needs no side-crop at all, so the pre-crop stands down by itself),
/// `ContentRect` normalizes the rotated frame, and the `frame_aspect`
/// published on `SilhouetteEdges` describes what the consumer actually maps.
/// A consumer-side rotation would leave all four of those describing a
/// landscape frame that no longer exists.
///
/// Rotation is a pure index remap inside the square pad, which already walks
/// every source pixel once — no extra buffer, no extra pass.
///
/// [`Self::None`] is the identity and the default; nothing changes for an
/// upright camera.
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CameraRotation {
    /// Camera is upright; frames pass through untouched.
    #[default]
    None,
    /// Camera is mounted rotated 90° **counter-clockwise** (its top edge
    /// points left), so frames are turned 90° **clockwise** to stand them
    /// back up.
    Cw90,
    /// Camera is mounted rotated 90° **clockwise** (its top edge points
    /// right), so frames are turned 270° clockwise — i.e. 90°
    /// counter-clockwise — to stand them back up.
    Cw270,
}

impl CameraRotation {
    /// Frame dimensions after the turn. The quarter turns swap the axes; the
    /// identity leaves them alone.
    #[must_use]
    pub fn rotated_dims(self, w: u32, h: u32) -> (u32, u32) {
        match self {
            Self::None => (w, h),
            Self::Cw90 | Self::Cw270 => (h, w),
        }
    }

    /// Map a pixel in the **rotated** frame back to its source pixel.
    ///
    /// `src_w`/`src_h` are the *unrotated* frame dimensions. Derivation, with
    /// a clockwise turn sending source `(sx, sy)` to destination
    /// `(src_h − 1 − sy, sx)`:
    ///
    /// - `Cw90`:  inverse is `sx = dy`, `sy = src_h − 1 − dx`
    /// - `Cw270`: inverse is `sx = src_w − 1 − dy`, `sy = dx`
    #[must_use]
    pub fn source_pixel(self, dx: u32, dy: u32, src_w: u32, src_h: u32) -> (u32, u32) {
        match self {
            Self::None => (dx, dy),
            Self::Cw90 => (dy, src_h.saturating_sub(1).saturating_sub(dx)),
            Self::Cw270 => (src_w.saturating_sub(1).saturating_sub(dy), dx),
        }
    }

    /// Wire encoding for the pipeline's lock-free `BodyLiveTuning` cell: the
    /// clockwise turn in degrees.
    #[must_use]
    pub fn to_degrees(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Cw90 => 90,
            Self::Cw270 => 270,
        }
    }

    /// Inverse of [`Self::to_degrees`]. Any other value (a torn read that
    /// cannot happen with a single `AtomicU32`, or a hand-edited settings
    /// file round-tripped through a future encoding) decodes to the identity
    /// rather than to a wrong turn.
    #[must_use]
    pub fn from_degrees(deg: u32) -> Self {
        match deg {
            90 => Self::Cw90,
            270 => Self::Cw270,
            _ => Self::None,
        }
    }
}

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

    /// Quarter-turn applied to every camera frame before the tracker looks at
    /// it — for a camera physically mounted on its side.
    ///
    /// A portrait install wants a portrait sensor. Turning a 16:9 camera 90°
    /// on its mount trades its short axis for its long one and buys ~1.8× the
    /// vertical field of view, which is the difference between framing a
    /// dancer's torso and framing all of them. This tells the tracker which
    /// way it was turned so it can stand the frame back up.
    ///
    /// Applied ahead of the aspect/crop geometry, so the pre-crop, the mask
    /// UV space, and the published frame aspect all describe the upright
    /// frame (see [`CameraRotation`]). Hot-applies: no worker restart, no
    /// camera re-open.
    #[setting(
        default = CameraRotation::None,
        ty = Enum,
        section = "Body tracking",
        category = User,
        label = "Camera rotation"
    )]
    #[serde(default = "default_camera_rotation")]
    pub camera_rotation: CameraRotation,
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

/// Serde fallback matching the setting default: an upright camera, i.e. the
/// behaviour every settings file written before this knob existed had.
fn default_camera_rotation() -> CameraRotation {
    CameraRotation::None
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
        let clamp = |max_figures| {
            BodyTrackingSettings {
                max_figures,
                ..BodyTrackingSettings::default()
            }
            .max_tracked_bodies()
        };
        assert_eq!(clamp(0), 1);
        assert_eq!(clamp(99), 4);
        assert_eq!(clamp(2), 2);
    }
}
