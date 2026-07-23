//! Webcam enumeration and selection plumbing for the settings dock's
//! "Webcam" dropdown (`crate::settings::webcam::WebcamSettings`).
//!
//! Three pieces, mirroring the audio-device and camera-preview precedents:
//!
//! - [`AvailableCameraDevices`]: the `"camera_devices"` runtime-enum options
//!   source, filled once at `Startup` from the platform backend (nokhwa off
//!   macOS, `AVFoundation` on macOS). Camera hot-plug re-enumeration is
//!   deliberately not wired — the deployment kiosk has a fixed camera;
//!   relaunch to rescan.
//! - The process-wide selection mirror ([`selected_camera`]): written on
//!   settings change, read once per camera *open* by both capture seams (the
//!   `MediaPipe` hand worker and the body worker). A worker (re)built at any
//!   moment observes the current value at its next open — the same rationale
//!   as the camera-preview toggle's atomic mirror, with a `Mutex` instead of
//!   an atomic because this is open-time state, never per-frame.
//! - [`WebcamSelectPlugin`]: registers the settings struct, the options
//!   source, and the two systems.

use std::sync::Mutex;

use bevy::prelude::*;

use crate::settings::runtime_enum::{RegisterRuntimeEnumOptionsExt, RuntimeEnumOptionsSource};
use crate::settings::webcam::{WebcamSettings, AUTO_LABEL};
use crate::settings::RegisterSketchSettingsExt;

/// Names of the attached capture devices, headed by the [`AUTO_LABEL`]
/// entry, for the `"camera_devices"` dropdown.
#[derive(Resource, Default)]
pub struct AvailableCameraDevices(pub Vec<String>);

impl RuntimeEnumOptionsSource for AvailableCameraDevices {
    const OPTIONS_KEY: &'static str = "camera_devices";

    fn options(&self) -> &[String] {
        &self.0
    }
}

/// Wires the webcam dropdown: settings struct, options source, startup
/// enumeration, and the selection mirror.
pub struct WebcamSelectPlugin;

impl Plugin for WebcamSelectPlugin {
    fn build(&self, app: &mut App) {
        app.register_sketch_settings::<WebcamSettings>()
            .init_resource::<AvailableCameraDevices>()
            .register_runtime_enum_options::<AvailableCameraDevices>()
            .add_systems(Startup, enumerate_camera_devices)
            .add_systems(PreUpdate, mirror_webcam_selection);
    }
}

/// `Startup`: fill the dropdown from the platform backend's enumeration.
fn enumerate_camera_devices(mut list: ResMut<'_, AvailableCameraDevices>) {
    list.0.clear();
    list.0.push(AUTO_LABEL.to_string());
    list.0.extend(backend_device_names());
}

/// `PreUpdate`: mirror the operator's selection into the process-wide slot
/// on change only (change detection covers the initial insert, so a
/// persisted selection is visible before the first camera open).
fn mirror_webcam_selection(settings: Res<'_, WebcamSettings>) {
    if settings.is_changed() {
        set_selected_camera(settings.selected());
    }
}

/// The operator's explicit webcam choice, cloned out of the process-wide
/// mirror. `None` = automatic (the pre-selector behaviour). Called once per
/// camera open — never on a per-frame path.
pub(crate) fn selected_camera() -> Option<String> {
    SELECTED_CAMERA.lock().ok().and_then(|slot| slot.clone())
}

/// The device index to open on macOS: the operator's selection resolved
/// against the `AVFoundation` discovery order (a name's position is its
/// index), or `fallback` when automatic or unmatched.
#[cfg(target_os = "macos")]
pub(crate) fn selected_index_or(fallback: u32) -> u32 {
    match selected_camera() {
        Some(name) => {
            resolve_index_by_name(&super::avfoundation::device_names(), Some(&name), fallback)
        }
        None => fallback,
    }
}

/// Position of the first name containing `want` (case-insensitive substring,
/// mirroring nokhwa's `match_camera_by_name`), else `fallback`.
#[cfg_attr(
    not(target_os = "macos"),
    allow(
        dead_code,
        reason = "the name→index resolve is the macOS open path; kept uncfg'd so the pure logic is unit-tested on every platform"
    )
)]
fn resolve_index_by_name(names: &[String], want: Option<&str>, fallback: u32) -> u32 {
    let Some(want) = want else {
        return fallback;
    };
    let want = want.to_lowercase();
    names
        .iter()
        .position(|name| name.to_lowercase().contains(&want))
        .and_then(|i| u32::try_from(i).ok())
        .map_or(fallback, |i| i)
}

/// Process-wide mirror of the operator's webcam selection. See the module
/// doc for why this is a static rather than threaded provider config.
static SELECTED_CAMERA: Mutex<Option<String>> = Mutex::new(None);

/// Store the current selection (`None` = automatic). A poisoned lock is
/// ignored: the mirror degrades to the automatic behaviour, never panics.
fn set_selected_camera(name: Option<&str>) {
    if let Ok(mut slot) = SELECTED_CAMERA.lock() {
        *slot = name.map(str::to_owned);
    }
}

/// Platform dispatch for the enumeration the dropdown shows.
fn backend_device_names() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        super::avfoundation::device_names()
    }
    #[cfg(not(target_os = "macos"))]
    {
        super::nokhwa::device_names()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// Case-insensitive substring match resolves to the name's position;
    /// automatic (None) and unmatched names keep the fallback index.
    #[test]
    fn resolve_index_matches_substring_or_falls_back() {
        let devices = names(&["FaceTime HD Camera", "OBSBOT Tiny 2 Lite"]);
        assert_eq!(resolve_index_by_name(&devices, Some("obsbot"), 0), 1);
        assert_eq!(
            resolve_index_by_name(&devices, Some("FaceTime HD Camera"), 5),
            0,
            "full dropdown name matches its own device"
        );
        assert_eq!(resolve_index_by_name(&devices, None, 3), 3, "automatic");
        assert_eq!(
            resolve_index_by_name(&devices, Some("Logitech"), 2),
            2,
            "unplugged/unmatched selection falls back rather than misopening"
        );
        assert_eq!(resolve_index_by_name(&[], Some("obsbot"), 0), 0);
    }
}
