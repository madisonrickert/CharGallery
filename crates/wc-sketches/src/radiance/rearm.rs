//! Attract re-arm: put the attract circle back when Radiance has nothing to
//! draw.
//!
//! ## The failure this closes
//!
//! Radiance draws its particles off tracked bodies. With no body there is
//! nothing to emit from, so an empty stage is a **black screen** — and the
//! only thing that normally ends it is the core idle timer, a full
//! `ScreensaverSettings::attract_mode_timeout_secs` (60 s by default) later.
//! Two ordinary events land there:
//!
//! - a dancer leaves mid-`Active` (the common one at a live show), and
//! - a wake that never had a body behind it. `poll_body_worker` now gates the
//!   wake on a *published* body, so a bare detector hit no longer wakes the
//!   screensaver at all — but a real visitor who steps in and immediately
//!   back out still wakes it and leaves the stage empty.
//!
//! This module is the backstop for both: after
//! [`RadianceSettings::attract_rearm_secs`] of empty stage it rewinds the
//! [`InteractionTimer`] past both idle thresholds, which is the same path
//! `Shift+S` uses, so `advance_activity` — still the single writer of
//! `NextState<SketchActivity>` — takes the app back into `Screensaver` and the
//! attract phantom starts drawing again.
//!
//! ## Why the guards are not optional
//!
//! Entering the screensaver closes the settings panel
//! (`screensaver::close_settings_panels`). An unguarded re-arm would therefore
//! shut the panel in the operator's face a few seconds after they opened it —
//! and they open it precisely when nobody is on the stage. So the re-arm holds
//! off while **either**:
//!
//! - a settings panel is open, or
//! - a *direct* input (mouse / keyboard / touch / hand — see
//!   [`InteractionTimer::mark_direct`]) landed within the same re-arm window.
//!   Camera presence deliberately does **not** count as direct: it is the very
//!   signal being judged.
//!
//! Both are inspected, not latched, so the countdown resumes on its own once
//! the operator steps away.
//!
//! ## Scheduling
//!
//! Registered `in_state(AppState::Radiance)` and **not** in
//! `SketchActivity::Screensaver` — attract mode is already showing there, and
//! the phantom is exactly the content this system exists to restore. It
//! therefore runs in `Active` and `Idle`.
//!
//! `Idle` is a deliberate, documented exception to AGENTS.md's "zero systems
//! when idle": with a re-arm delay at or above the core idle threshold the
//! natural `Idle → Screensaver` path wins the race, and this system has to
//! observe an empty stage across that boundary rather than reset its
//! countdown at it. The cost is a handful of resource reads and no allocation
//! — the same cheap-no-op contract as the sanctioned message listeners.

use std::time::Duration;

use bevy::prelude::*;
use wc_core::input::body::BodyTrackingState;
use wc_core::lifecycle::idle::InteractionTimer;
use wc_core::ui::buttons::SettingsPanelVisible;

use crate::radiance::settings::RadianceSettings;

/// Guard rails on the persisted `attract_rearm_secs`, mirroring the
/// screensaver framework's own clamps: a hand-edited TOML outside the
/// slider's 1–60 s range (or a `nan`) must not produce a degenerate window.
/// `max().min()` rather than `clamp()` because `clamp` passes NaN through.
const REARM_MIN_SECS: f32 = 1.0;
/// Upper rail; see [`REARM_MIN_SECS`].
const REARM_MAX_SECS: f32 = 60.0;

/// The re-arm window as a [`Duration`], sanitized against a degenerate
/// persisted value.
#[must_use]
#[allow(
    clippy::manual_clamp,
    reason = "max().min() is deliberate: clamp() passes NaN through, and a NaN duration panics \
              in Duration::from_secs_f32 — max/min sanitize a degenerate persisted TOML to the rail"
)]
pub fn rearm_window(secs: f32) -> Duration {
    Duration::from_secs_f32(secs.max(REARM_MIN_SECS).min(REARM_MAX_SECS))
}

/// Whether the stage is empty — no body in any slot, including bodies still
/// fading out.
///
/// A fading body is deliberately counted as content: its particles are still
/// on screen, so re-arming over the top of a graceful exit would cut the exit
/// short. The countdown starts once the last slot frees.
#[must_use]
pub fn stage_is_empty(bodies: Option<&BodyTrackingState>) -> bool {
    bodies.is_none_or(|b| b.iter_bodies().next().is_none())
}

/// Pure re-arm decision: given how long the stage has been empty, how long
/// since the last direct operator input, whether a panel is open, and the
/// window, should the screensaver be forced back?
///
/// Split out so the guard set is unit-tested without a clock or a worker.
#[must_use]
pub fn should_rearm(
    empty_for: Duration,
    direct_idle_for: Duration,
    panel_open: bool,
    window: Duration,
) -> bool {
    !panel_open && empty_for >= window && direct_idle_for >= window
}

/// `Update` (`in_state(AppState::Radiance)`, not in `Screensaver`): force
/// attract mode back once the stage has been empty for the configured window.
///
/// `empty_since` is the frame the stage went empty, or `None` while there is
/// something to draw. It is also cleared after a force, so a wake that lands
/// back on an empty stage starts a fresh countdown instead of re-forcing every
/// frame.
pub fn rearm_attract_when_nothing_to_render(
    time: Res<'_, Time>,
    settings: Res<'_, RadianceSettings>,
    bodies: Option<Res<'_, BodyTrackingState>>,
    panel: Option<Res<'_, SettingsPanelVisible>>,
    mut timer: ResMut<'_, InteractionTimer>,
    mut empty_since: Local<'_, Option<Duration>>,
) {
    let now = time.elapsed();
    if !stage_is_empty(bodies.as_deref()) {
        *empty_since = None;
        return;
    }
    let since = *empty_since.get_or_insert(now);
    let window = rearm_window(settings.attract_rearm_secs);
    let panel_open = panel.is_some_and(|p| p.0);
    if !should_rearm(
        now.saturating_sub(since),
        timer.direct_idle_for(now),
        panel_open,
        window,
    ) {
        return;
    }
    *empty_since = None;
    timer.rewind_past_screensaver(now);
    tracing::info!(
        empty_for_s = window.as_secs_f32(),
        "radiance: stage empty — re-arming attract mode"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window is clamped to the slider's rails, and a `nan` from a
    /// hand-edited TOML lands on the floor instead of panicking
    /// `Duration::from_secs_f32`.
    #[test]
    fn rearm_window_clamps_degenerate_values() {
        assert_eq!(rearm_window(3.0), Duration::from_secs_f32(3.0));
        assert_eq!(rearm_window(0.0), Duration::from_secs_f32(REARM_MIN_SECS));
        assert_eq!(rearm_window(-5.0), Duration::from_secs_f32(REARM_MIN_SECS));
        assert_eq!(
            rearm_window(10_000.0),
            Duration::from_secs_f32(REARM_MAX_SECS)
        );
        assert_eq!(
            rearm_window(f32::NAN),
            Duration::from_secs_f32(REARM_MIN_SECS)
        );
    }

    /// A body still fading out is content: the countdown must not start until
    /// the last slot frees.
    #[test]
    fn a_fading_body_still_counts_as_content() {
        use wc_core::input::body::TrackedBody;

        assert!(stage_is_empty(None));
        let mut state = BodyTrackingState::default();
        assert!(stage_is_empty(Some(&state)), "no slots occupied");

        state.bodies[0] = Some(TrackedBody {
            present: false, // departed, mid fade-out
            fade: 0.4,
            ..TrackedBody::default()
        });
        assert!(
            !stage_is_empty(Some(&state)),
            "a fading body is still on screen"
        );
    }

    /// Every guard independently blocks the force; all clear fires it.
    #[test]
    fn every_guard_blocks_the_rearm() {
        let window = Duration::from_secs(3);
        let past = Duration::from_secs(5);
        let recent = Duration::from_secs(1);

        assert!(
            should_rearm(past, past, false, window),
            "empty long enough, operator away, panel closed"
        );
        assert!(
            !should_rearm(recent, past, false, window),
            "stage only just went empty"
        );
        assert!(
            !should_rearm(past, recent, false, window),
            "operator touched the machine inside the window"
        );
        assert!(
            !should_rearm(past, past, true, window),
            "an open settings panel must never be closed under the operator"
        );
        // Exactly at the window: fire (>=, so the boundary is inclusive and a
        // slow frame cannot step over it).
        assert!(should_rearm(window, window, false, window));
    }
}
