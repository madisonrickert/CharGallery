//! Operator webcam selection (storage key `"webcam"`), rendered on the
//! settings dock's Camera tab.
//!
//! The `camera` field is a `RuntimeEnum` over the `"camera_devices"` options
//! list, which the capture layer's `WebcamSelectPlugin` fills at startup
//! (`input::capture::devices` — compiled only when a camera backend feature
//! is). This struct itself is unconditional so the storage key, tab routing,
//! and serde behaviour stay testable without camera features.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use wc_core_macros::SketchSettings;

/// The dropdown's "no explicit choice" entry. Selecting it (or the empty
/// pre-selector stored value) restores the automatic behaviour: prefer an
/// OBSBOT by name, else the configured device index.
pub const AUTO_LABEL: &str = "Automatic";

/// Which physical webcam the tracking seams open.
#[derive(
    SketchSettings, Resource, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq, Eq,
)]
#[reflect(Resource, Default)]
#[settings(storage_key = "webcam")]
pub struct WebcamSettings {
    /// Capture device by name. "Automatic" prefers an OBSBOT (both webcam
    /// modalities target one physical camera on the deployment), falling
    /// back to the default device. Applies when a camera next opens: the
    /// reload badge covers the body seam; the hand seam picks it up on its
    /// next provider rebuild or app launch.
    #[setting(
        default = String::new(),
        ty = RuntimeEnum,
        options_key = "camera_devices",
        category = User,
        section = "Camera",
        label = "Webcam",
        requires_restart
    )]
    #[serde(default)]
    pub camera: String,
}

impl WebcamSettings {
    /// The operator's explicit camera choice: `None` when automatic (the
    /// [`AUTO_LABEL`] entry, or the empty pre-selector default).
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        let name = self.camera.trim();
        (!name.is_empty() && name != AUTO_LABEL).then_some(name)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "expect is appropriate in test code")]
mod tests {
    use super::*;

    /// Empty (the default), whitespace, and the Automatic entry all mean "no
    /// explicit choice"; anything else is the choice, trimmed.
    #[test]
    fn selected_treats_empty_and_auto_as_automatic() {
        let mut s = WebcamSettings::default();
        assert_eq!(s.selected(), None, "default is automatic");
        s.camera = "  ".to_string();
        assert_eq!(s.selected(), None, "whitespace is automatic");
        s.camera = AUTO_LABEL.to_string();
        assert_eq!(s.selected(), None, "the Automatic entry is automatic");
        s.camera = " OBSBOT Tiny 2 Lite ".to_string();
        assert_eq!(
            s.selected(),
            Some("OBSBOT Tiny 2 Lite"),
            "explicit, trimmed"
        );
    }

    /// A settings file from before this struct existed deserializes to the
    /// automatic default (per-field serde default, the house pattern).
    #[test]
    fn legacy_empty_toml_is_automatic() {
        let parsed: WebcamSettings = toml::from_str("").expect("empty TOML must parse");
        assert_eq!(parsed, WebcamSettings::default());
        assert_eq!(parsed.selected(), None);
    }
}
