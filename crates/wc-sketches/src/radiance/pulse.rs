//! Beat-wave clock: a fixed ring of expanding wavefronts, one born on every
//! detected beat, whose current radius + age-decayed strength bake into the
//! sim uniform for the in-medium flare-wave (`simulate.wgsl`, the Task 3
//! kernel consumer).
//!
//! ## Data flow
//!
//! The analysis engine's debounced beat lane (`AudioAnalysis::beat_confidence`
//! snaps to 1.0 on a beat and decays exponentially) is the strongest signal a
//! party-room mic delivers, and this module is its dedicated consumer. Each
//! rising edge spawns one wave into a fixed ring buffer of [`MAX_PULSES`]
//! slots ([`RadianceBeatWaves`]); [`advance_beat_waves`] ages every slot and
//! writes its radius (`age ×`[`PULSE_SPEED_PX_S`]) and age-decayed strength
//! into the sim uniform's `wave_radius_px` / `wave_strength` lanes. The
//! particle kernel brightens each particle as a wave front passes its exterior
//! distance, so the flare travels *through the medium* instead of over it.
//!
//! Historically this drove a fullscreen additive silhouette-contour overlay
//! quad; that overlay was retired and its energy moved into the particle world.
//!
//! ## Hot-path invariants
//!
//! Fixed-size arrays throughout: per-frame work is a slot walk plus the wave
//! lane bake, and nothing allocates after spawn. During Idle/Screensaver the
//! mic is paused → `beat_confidence` holds 0 → no spawns; residual waves keep
//! expanding and fade within [`PULSE_LIFETIME_S`]. Once every slot is dead and
//! the zeros have been written once, the driver stops touching the sim
//! resource (the `frozen_secs` stop-dirtying contract).

use bevy::prelude::*;
use wc_core::audio::input::AudioAnalysis;

use super::compute::sim_params::RadianceSimParams;

/// Fixed wave slot count (the CPU ring buffer size).
pub const MAX_PULSES: usize = 6;
/// Wave expansion speed, world px/s. One wave is still travelling when the
/// next beat lands at dance tempi, so multiple flare fronts sweep the medium
/// at once.
pub const PULSE_SPEED_PX_S: f32 = 650.0;
/// Default flare-band half-width, world px — the Gaussian half-band of the
/// in-medium flare front. Task 3's `flare_band` Dev default inherits it.
pub const PULSE_WIDTH_PX: f32 = 60.0;
/// Seconds until a wave slot is dead. The CPU age-decay (`exp(-age · 1.8)` in
/// [`advance_beat_waves`]) dims the strength smoothly to near-zero well before
/// this cutoff, so the hard cutoff is invisible.
pub const PULSE_LIFETIME_S: f32 = 1.6;
/// `beat_confidence` rising-edge threshold that fires a wave. Confidence
/// snaps to 1.0 on a beat and decays with a 0.3 s time constant, so at the
/// 240 BPM debounce ceiling it still falls to ~0.43 between beats — every
/// debounced beat produces exactly one rising edge here.
pub const BEAT_EDGE: f32 = 0.6;
/// Frame-delta cap, matching the sim baker's hitch guard.
const PULSE_DT_CAP: f32 = 0.05;

/// One wave: born on a beat, expanding with age.
#[derive(Clone, Copy, Debug)]
pub struct PulseSlot {
    /// Seconds since the beat that spawned it (`>= PULSE_LIFETIME_S` = dead).
    pub age: f32,
    /// Brightness scale in `0..1` (beat-derived at spawn).
    pub strength: f32,
    /// Linear-HDR color carried per slot. The in-medium flare has no color of
    /// its own (it brightens the particle's existing hue), so this is
    /// currently unused; the slot keeps the field so [`step_pulses`] stays
    /// shape-compatible.
    pub color: Vec4,
}

impl Default for PulseSlot {
    /// A dead slot: expired age, zero strength.
    fn default() -> Self {
        Self {
            age: PULSE_LIFETIME_S,
            strength: 0.0,
            color: Vec4::ZERO,
        }
    }
}

/// CPU beat-wave state: a fixed ring buffer of slots plus the beat edge
/// tracker. Inserted on Radiance entry, removed on exit.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct RadianceBeatWaves {
    /// The slots; spawning overwrites round-robin (oldest-first by index).
    pub slots: [PulseSlot; MAX_PULSES],
    /// Next slot index to overwrite.
    next_slot: usize,
    /// Previous frame's `beat_confidence` (rising-edge detection).
    prev_beat: f32,
}

/// Advance every slot by `dt` and spawn one wave on a rising beat edge.
/// Returns `true` when a wave was spawned. Pure over its inputs so the
/// beat-edge/round-robin behavior is unit-testable without an app.
pub fn step_pulses(
    pulses: &mut RadianceBeatWaves,
    dt: f32,
    beat_confidence: f32,
    spawn_enabled: bool,
    strength: f32,
    color: Vec4,
) -> bool {
    for slot in &mut pulses.slots {
        slot.age += dt;
    }
    let rising = beat_confidence > BEAT_EDGE && pulses.prev_beat <= BEAT_EDGE;
    pulses.prev_beat = beat_confidence;
    if !(rising && spawn_enabled) {
        return false;
    }
    pulses.slots[pulses.next_slot] = PulseSlot {
        age: 0.0,
        strength: strength.clamp(0.0, 1.0),
        color,
    };
    pulses.next_slot = (pulses.next_slot + 1) % MAX_PULSES;
    true
}

/// `Update` (gated `in_state(AppState::Radiance)` — Idle and the screensaver
/// included, so a wave that is mid-flight when the dancer leaves keeps
/// expanding and fades out instead of freezing as a bright ring on the
/// surviving particles): advance the ring, spawn on a rising beat edge, and
/// bake every slot's current radius + strength into the sim uniform.
///
/// Writes only the wave fields of `RadianceSimParamsGpu` — the Active-only
/// baker owns everything else, so the two writers never conflict. During
/// Idle the mic is paused (`beat_confidence` holds 0) so no new waves spawn;
/// ages still advance here.
pub fn advance_beat_waves(
    time: Res<'_, Time>,
    audio: Option<Res<'_, AudioAnalysis>>,
    mut waves: ResMut<'_, RadianceBeatWaves>,
    mut sim: ResMut<'_, RadianceSimParams>,
    mut settled_dead: Local<'_, bool>,
) {
    let dt = time.delta_secs().min(PULSE_DT_CAP);
    let audio_frame = audio.map_or_else(AudioAnalysis::neutral, |a| *a);
    // Strength: the bass-weighted beat lane, exactly the old overlay's drive.
    let strength = (audio_frame.beat_confidence * 0.6 + 0.4).min(1.0);
    step_pulses(
        &mut waves,
        dt,
        audio_frame.beat_confidence,
        true,
        strength,
        Vec4::ONE, // color is unused by the in-medium flare; slot keeps the field
    );
    // Settle guard: once every slot is dead AND the zeros have been written
    // once, stop touching the sim resource — an Idle frame with no residual
    // waves must not re-dirty `RadianceSimParams` every tick (the
    // `frozen_secs` clamp's stop-dirtying contract; see its field doc).
    let any_live = waves
        .slots
        .iter()
        .any(|s| s.age < PULSE_LIFETIME_S && s.strength > 0.0);
    if !any_live && *settled_dead {
        return;
    }
    *settled_dead = !any_live;
    for (i, slot) in waves.slots.iter().enumerate() {
        let live = slot.age < PULSE_LIFETIME_S && slot.strength > 0.0;
        sim.params.wave_radius_px[i] = slot.age * PULSE_SPEED_PX_S;
        sim.params.wave_strength[i] = if live {
            // Age decay: the old contour pass's exp(-age * 1.8) dimming,
            // now also the guard against the saturation-shell sync flash.
            slot.strength * (-slot.age * 1.8).exp()
        } else {
            0.0
        };
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A rising beat edge spawns exactly one wave; the decaying confidence
    /// tail and a held-high value do not retrigger; the next beat does.
    #[test]
    fn beat_edge_spawns_once_per_beat() {
        let mut pulses = RadianceBeatWaves::default();
        let dt = 1.0 / 60.0;
        let c = Vec4::ONE;
        assert!(step_pulses(&mut pulses, dt, 1.0, true, 0.8, c));
        // Decay tail: still above the edge, but not rising.
        assert!(!step_pulses(&mut pulses, dt, 0.9, true, 0.8, c));
        assert!(!step_pulses(&mut pulses, dt, 0.7, true, 0.8, c));
        // Below the edge, then the next beat snaps it back up.
        assert!(!step_pulses(&mut pulses, dt, 0.3, true, 0.8, c));
        assert!(step_pulses(&mut pulses, dt, 1.0, true, 0.8, c));
        // Exactly two waves total across five frames with two beats: the
        // first (now four frames old) and the fresh one in the next slot.
        let live = pulses
            .slots
            .iter()
            .filter(|s| s.age < PULSE_LIFETIME_S)
            .count();
        assert_eq!(live, 2, "two beats -> two live waves");
        assert!(
            pulses.slots[1].age.abs() < f32::EPSILON,
            "round-robin advanced to slot 1"
        );
    }

    /// Spawns overwrite round-robin without disturbing other slots' ages.
    #[test]
    fn spawns_rotate_through_slots() {
        let mut pulses = RadianceBeatWaves::default();
        for i in 0..MAX_PULSES + 2 {
            // Drop confidence to re-arm the edge, then beat. Tag each spawn
            // by strength so wrap-around is observable.
            #[allow(
                clippy::as_conversions,
                clippy::cast_precision_loss,
                reason = "small loop index, exact in f32"
            )]
            let tag = 0.1 + (i as f32) * 0.1;
            step_pulses(&mut pulses, 0.01, 0.0, true, tag, Vec4::ONE);
            let spawned = step_pulses(&mut pulses, 0.01, 1.0, true, tag, Vec4::ONE);
            assert!(spawned, "beat {i} must spawn");
        }
        // The two wrap-around spawns overwrote slots 0 and 1.
        #[allow(
            clippy::as_conversions,
            clippy::cast_precision_loss,
            reason = "small loop index, exact in f32"
        )]
        {
            let expect0 = 0.1 + (MAX_PULSES as f32) * 0.1;
            let expect1 = 0.1 + ((MAX_PULSES + 1) as f32) * 0.1;
            assert!((pulses.slots[0].strength - expect0).abs() < 1e-6);
            assert!((pulses.slots[1].strength - expect1).abs() < 1e-6);
        }
    }

    /// Disabled spawning (beat lane silent / inactive audio) never fires.
    #[test]
    fn disabled_spawning_never_fires() {
        let mut pulses = RadianceBeatWaves::default();
        assert!(!step_pulses(&mut pulses, 0.01, 1.0, false, 1.0, Vec4::ONE));
        assert!(pulses.slots.iter().all(|s| s.strength.abs() < f32::EPSILON));
    }
}
