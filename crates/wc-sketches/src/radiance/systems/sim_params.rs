//! Per-frame Radiance simulation writer plus the idle freeze and the
//! all-dead idle pause.
//!
//! Owns [`RadianceState`] (the smoothed audio-drive envelopes), the pure
//! mask-UV↔world mapping (CPU twin of the kernel's), the pure
//! [`audio_drive`] mapping, and the single [`bake_radiance_sim`] baker that
//! both the live writer ([`update_radiance_sim`]) and the screensaver
//! performer call — one baker, two writers, so the audio/impulse derivation
//! cannot drift between the live and attract paths (flame's Condition A1).
//!
//! Nothing here allocates: every value is stack math over `Copy` inputs, so
//! the per-frame path is heap-free per the multi-hour soak target.

use bevy::prelude::*;
use wc_core::audio::input::AudioAnalysis;
use wc_core::input::body::landmark_index::{
    LEFT_ANKLE, LEFT_ELBOW, LEFT_HIP, LEFT_WRIST, NOSE, RIGHT_ANKLE, RIGHT_ELBOW, RIGHT_HIP,
    RIGHT_WRIST,
};
use wc_core::input::body::selection::motion_weight;
use wc_core::input::body::{
    BodyTrackingState, SilhouetteEdges, MAX_EDGE_POINTS, MAX_TRACKED_BODIES,
};

use crate::radiance::compute::sim_params::{
    RadianceImpulse, RadianceSimParams, RadianceSimParamsGpu, MAX_IMPULSES,
};
use crate::radiance::pulse::BEAT_EDGE;
use crate::radiance::settings::{RadianceFrameFit, RadianceSettings};
use crate::radiance::visibility::VisibilityLatch;

/// Number of impulse source concepts per body (seven of the eight
/// [`MAX_IMPULSES`] slots; the eighth is headroom).
pub const IMPULSE_SOURCE_COUNT: usize = 7;

/// Impulse sources: `(primary landmark, optional fallback)`. Arms fall back
/// to the elbow when the wrist is the less-visible joint — a held fan or
/// prop covers the hand long before the elbow, and the prop arm (the most
/// expressive one) must keep shedding particles. Indices come from
/// `wc_core::input::body::landmark_index` so the contract cannot drift.
pub const IMPULSE_SOURCES: [(usize, Option<usize>); IMPULSE_SOURCE_COUNT] = [
    (NOSE, None),
    (LEFT_WRIST, Some(LEFT_ELBOW)),
    (RIGHT_WRIST, Some(RIGHT_ELBOW)),
    (LEFT_HIP, None),
    (RIGHT_HIP, None),
    (LEFT_ANKLE, None),
    (RIGHT_ANKLE, None),
];

/// Frame-time cap in seconds (matches the shared particle engine's 50 ms cap).
pub const DT_CAP: f32 = 0.05;
/// Per-dead-particle respawn attempts per second at `emission_rate == 1.0`
/// and neutral audio. The baker multiplies by the bass drive and `dt`.
pub const EMISSION_BASE_HZ: f32 = 0.2;
/// Onset envelope exponential release time constant, seconds.
pub const ONSET_DECAY_SECS: f32 = 0.18;
/// Onset envelope clamp (spectral flux is unbounded above).
pub const ONSET_MAX: f32 = 2.0;
/// Outward burst speed at full onset envelope, world px/s. A gentle global
/// push — the drama of a hit lives in the ejecta layer (see
/// [`EJECTA_SPEED`]), so this stays small enough that the flame body swells
/// rather than detaching wholesale.
pub const BURST_SPEED: f32 = 90.0;
/// Spawn offset along the outward normal, world px.
pub const SPAWN_OFFSET: f32 = 4.0;
/// Baseline spawn speed along the outward normal, world px/s.
pub const SPAWN_SPEED: f32 = 70.0;
/// Particle lifespan range, seconds.
pub const LIFESPAN_MIN: f32 = 0.8;
/// See [`LIFESPAN_MIN`].
pub const LIFESPAN_MAX: f32 = 2.2;
/// Ejecta launch speed at neutral intensity, world px/s (the "shooting
/// particles" of an onset hit; the render shader streaks anything this fast).
pub const EJECTA_SPEED: f32 = 480.0;
/// Baseline fraction of spawns that are ejecta with **zero** onset — a few
/// stray sparks keep the flame alive-looking between hits.
pub const EJECTA_BASE_FRACTION: f32 = 0.01;
/// Extra ejecta fraction at full onset envelope (scaled by the
/// `ejecta_amount` setting).
pub const EJECTA_ONSET_FRACTION: f32 = 0.35;
/// Flame-tongue spatial frequency, radians per world px (~300 px wavelength:
/// two to three licking tongues across a standing figure).
pub const TONGUE_FREQ: f32 = 0.017;
/// Emission-share boost for an igniting body (fade rising through the low
/// range): the appearing dancer's flame catches with a visible flare while
/// the *total* budget stays constant (weights are normalized).
pub const IGNITE_BOOST: f32 = 2.5;
/// [`motion_weight`] floor for the background-subdue emission grace:
/// a completely still body's weight factor at FULL `background_subdue`. Kept
/// high (0.6, above the primary-selection floor of 0.55) because this scales
/// *rendered flame* rather than a selection score — a still body must stay
/// visibly alight, just subdued, so bystanders never see their aura vanish
/// outright. At the default `background_subdue = 0.5` a still body burns at
/// `1 − 0.5·(1 − 0.6)` = 80% relative weight (before normalization).
pub const SUBDUE_MOTION_FLOOR: f32 = 0.6;
/// Fade ceiling below which a rising body still counts as igniting.
pub const IGNITE_FADE_CEIL: f32 = 0.7;
/// Velocity fraction remaining after one second of drag.
pub const DRAG_PER_SECOND: f32 = 0.25;
/// Glow fraction remaining after one second (baked per frame as
/// `GLOW_PER_SECOND.powf(dt)`, the drag idiom). Fast decay deliberately:
/// glow is a flash, not a state — a contact/flare highlight dies within a
/// few hundred milliseconds of its cause.
pub const GLOW_PER_SECOND: f32 = 0.02;
// (Curl spatial frequency and the limb-impulse coupling gain were hardwired
// consts here until 2026-07-24; both are operator settings now — see
// `RadianceSettings::curl_scale` / `curl_evolve` / `impulse_coupling`.)
/// Limb impulse influence radius, world px.
pub const IMPULSE_RADIUS: f32 = 140.0;
/// Limb speed (world px/s) that maps to impulse gain 1.0.
pub const IMPULSE_FULL_SPEED: f32 = 900.0;
/// Hard cap on an impulse's velocity magnitude, world px/s. Gain already
/// saturates at [`IMPULSE_FULL_SPEED`], but the velocity *vector* is passed
/// to the kernel unscaled — a one-frame landmark teleport (motion blur, a
/// mis-detection) would otherwise blast particles across the field. 1.5×
/// full speed keeps every legitimate sweep untouched.
pub const IMPULSE_MAX_SPEED: f32 = 1.5 * IMPULSE_FULL_SPEED;
/// Smoothing time constant for the intensity/sparkle envelopes, seconds.
pub const ENVELOPE_SMOOTH_SECS: f32 = 0.25;
/// Time constant of the slow per-aggregate running means the band drives are
/// normalized by (see [`band_drive`]). Long enough to track a song section,
/// short enough to re-adapt across a DJ transition.
pub const BAND_NORM_TAU_S: f32 = 8.0;
/// Floor on the bass running mean: silence must not normalize the noise
/// floor up into a full drive.
pub const BASS_AVG_FLOOR: f32 = 0.02;
/// Floor on the highs running mean. Far lower than the bass floor: a party
/// room mic delivers almost no absolute energy above 1.6 kHz (measured
/// p90 ≈ 0.004 on real material), so the highs lane is useful only as a
/// *relative* signal — but it still needs a floor against amplified hiss.
pub const HIGHS_AVG_FLOOR: f32 = 1.0e-3;

/// Smoothed audio-drive envelopes and the palette-shift accumulator; also
/// read by the material driver (Task 8). Rebuilt fresh on every sketch entry.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct RadianceState {
    /// Onset burst envelope: instant attack, exponential release.
    pub onset_env: f32,
    /// Smoothed master intensity from RMS (`~0.55..1.5`); drives the
    /// particle-material brightness.
    pub intensity: f32,
    /// Smoothed high-band energy (`0..1`); drives sparkle flicker + fill
    /// shimmer.
    pub sparkle: f32,
    /// Gradient-shift accumulator in `0..1` (wraps); bass advances it.
    pub palette_shift: f32,
    /// Slow running mean of the bass aggregate ([`band_drive`] reference).
    pub bass_avg: f32,
    /// Slow running mean of the highs aggregate ([`band_drive`] reference).
    pub highs_avg: f32,
    /// Palette hue-rotation phase in `0..1` (wraps; 1 = one full spectrum
    /// rotation). Advanced by `hue_cycle_speed`, accelerated by bass.
    pub hue_phase: f32,
    /// Smoothed bass drive (`0..1`, the [`band_drive`]-normalized bass lane):
    /// the beat-weighted "flame swell" signal shared by the billboard-size
    /// breathing and the beat-pulse strength.
    pub bass_drive: f32,
    /// Previous frame's per-slot fade envelopes — the ignite detector
    /// compares against these to spot a body fading *in* (see
    /// [`emission_slot_weights`]).
    pub slot_fade_prev: [f32; MAX_TRACKED_BODIES],
    /// Per-slot, per-impulse-source Schmitt visibility latches (see
    /// `crate::radiance::visibility`): marginal landmark visibility holds
    /// its last gate decision instead of strobing the impulse layer.
    pub impulse_latch: [[VisibilityLatch; IMPULSE_SOURCE_COUNT]; MAX_TRACKED_BODIES],
    /// Previous frame's `beat_confidence` — the density-adaptive burst's
    /// rising-edge detector (same `BEAT_EDGE` bookkeeping as the wave
    /// clock's `step_pulses`).
    pub prev_beat: f32,
    /// Deterministic expected-alive estimate (see [`expected_alive_step`]):
    /// the density signal the beat burst adapts to. An expectation, not a
    /// readback — the GPU owns the real births/deaths.
    ///
    /// Known staleness bound, accepted as-is: the Active-only baker stops
    /// advancing this during Idle while the real field dies out, so the
    /// first post-Idle beats see a stale-high estimate and UNDER-boost. The
    /// recurrence re-converges within a few seconds of Active baking, and
    /// the failure mode is a merely-normal burst, never an oversized one —
    /// deliberately documented instead of adding cross-state plumbing.
    pub est_alive: f32,
}

/// The neutral [`AudioAnalysis`] used when the resource is absent (headless
/// tests, feature-less harnesses) — the same values Plan A publishes when the
/// stream is inactive. Delegates to `AudioAnalysis::neutral()`; kept as a
/// named free function so this module's own public surface (and the tests
/// below) can spell the neutral case without reaching into Plan A's type.
#[must_use]
pub fn neutral_audio() -> AudioAnalysis {
    AudioAnalysis::neutral()
}

/// Mask-UV (0..1, y down) → world px (origin center, y up), with the mirror
/// flip. CPU twin of the kernel's `mask_uv_to_world` — the two must stay
/// term-for-term identical (world = ((u − 0.5)·sx, (0.5 − v)·sy)).
#[must_use]
pub fn mask_uv_to_world(uv: Vec2, scale: Vec2, mirror: bool) -> Vec2 {
    let u = if mirror { 1.0 - uv.x } else { uv.x };
    Vec2::new((u - 0.5) * scale.x, (0.5 - uv.y) * scale.y)
}

/// Mask-UV direction → world direction (mirror sign on x, y flip). NOT
/// normalized — impulse velocities keep their magnitude (UV/s × scale =
/// px/s); the kernel normalizes separately where it needs a unit normal.
#[must_use]
pub fn mask_dir_to_world(dir: Vec2, scale: Vec2, mirror: bool) -> Vec2 {
    let sx = if mirror { -scale.x } else { scale.x };
    Vec2::new(dir.x * sx, -dir.y * scale.y)
}

/// The audio→simulation coupling, as pure multipliers/values over one
/// [`AudioAnalysis`] frame (spec: bass→emission+buoyancy, highs→turbulence+
/// sparkle, onset→radial burst, slow RMS→master intensity).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioDrive {
    /// Multiplier on the emission pressure (bass).
    pub emission_mul: f32,
    /// Multiplier on buoyancy (bass pulse).
    pub buoyancy_mul: f32,
    /// Multiplier on curl flow strength (highs).
    pub turbulence_mul: f32,
    /// Sparkle target `0..1` (highs).
    pub sparkle: f32,
    /// Master intensity target (RMS-lifted brightness).
    pub intensity: f32,
    /// Raw onset strength this frame, sensitivity-scaled and clamped.
    pub onset: f32,
    /// Normalized bass drive `0..1` (the [`band_drive`] lane the multipliers
    /// above are built from), exposed for the tongue/swell/pulse consumers.
    pub bass: f32,
}

/// Contrast-expanded, room-adaptive band drive: `value` relative to its own
/// slow running mean `reference`, mapped so sitting *at* the mean yields a
/// moderate drive and ~1.5x the mean saturates. Calibrated against real
/// party-room mic material (48 s report, 2026-07-18): the post-AGC bass
/// aggregate spans only ~0.07..0.21 absolute (a 1.1x..1.3x multiplier under
/// the old absolute mapping — visually near-static), but its *ratio* to its
/// own mean spans ~0.45..1.55, which this map stretches across the full
/// `0..1` drive. The highs aggregate is ~50x smaller in absolute terms
/// (p90 ≈ 0.004) yet has 2x ratio dynamics, so relative normalization is the
/// only mapping that makes the sparkle/turbulence lane live on a room mic.
#[must_use]
pub fn band_drive(value: f32, reference: f32) -> f32 {
    // ratio 0.7 → 0.0, ratio 1.5 → 1.0 (clamped outside).
    ((value / reference - 0.7) / 0.8).clamp(0.0, 1.0)
}

/// Map one analysis frame into drive values. Pure and allocation-free.
/// `sensitivity == 0.0` returns the exact neutral drive (all multipliers 1.0)
/// so audio coupling is provably inert at the knob's floor.
///
/// `bass_avg` / `highs_avg` are the slow running means tracked in
/// [`RadianceState`] (floored here so a fresh/silent state cannot divide by
/// ~0); see [`band_drive`] for the normalization rationale.
#[must_use]
pub fn audio_drive(
    audio: &AudioAnalysis,
    sensitivity: f32,
    bass_avg: f32,
    highs_avg: f32,
) -> AudioDrive {
    let s = sensitivity.max(0.0);
    let (bass, highs) = band_aggregates(audio);
    let bass_n = band_drive(bass, bass_avg.max(BASS_AVG_FLOOR));
    let highs_n = band_drive(highs, highs_avg.max(HIGHS_AVG_FLOOR));
    AudioDrive {
        emission_mul: 1.0 + 1.5 * bass_n * s,
        buoyancy_mul: 1.0 + 0.8 * bass_n * s,
        turbulence_mul: 1.0 + 1.6 * highs_n * s,
        sparkle: (highs_n * s).clamp(0.0, 1.0),
        // RMS lifts the floor brightness; each detected beat rides a throb on
        // top (beat_confidence snaps to 1 and decays in ~0.3 s). The 1.7x RMS
        // slope is calibrated to real material (rms p10..p90 ≈ 0.08..0.23 →
        // intensity ~0.63..0.89 before the beat term).
        intensity: 0.5 + (1.7 * audio.rms + 0.3 * audio.beat_confidence) * s,
        onset: (audio.onset * s).clamp(0.0, ONSET_MAX),
        bass: (bass_n * s).clamp(0.0, 1.0),
    }
}

/// The two band aggregates every drive consumer shares: low three bands =
/// bass body (50–400 Hz), top three = air/sparkle (1.6–12.8 kHz).
#[must_use]
pub fn band_aggregates(audio: &AudioAnalysis) -> (f32, f32) {
    let bass = (audio.bands[0] + audio.bands[1] + audio.bands[2]) / 3.0;
    let highs = (audio.bands[5] + audio.bands[6] + audio.bands[7]) / 3.0;
    (bass, highs)
}

/// One step of the deterministic expected-alive recurrence: dead slots win
/// respawns at `emission_prob`, alive particles die at `1/mean_lifespan`.
/// An expectation, not a count — the GPU owns the real births/deaths — but
/// bias-stable, which is all the burst boost needs (see the spec's
/// density-adaptive burst).
#[must_use]
pub fn expected_alive_step(
    est: f32,
    emission_prob: f32,
    particle_count: f32,
    dt: f32,
    mean_lifespan: f32,
) -> f32 {
    let births = emission_prob * (particle_count - est).max(0.0);
    let deaths = est * (dt / mean_lifespan.max(1e-3));
    (est + births - deaths).clamp(0.0, particle_count)
}

/// Inverse-density boost for the beat burst: 1 when the medium is full,
/// rising toward `cap` as it empties — the legibility floor: a beat from
/// near-empty water births a visibly larger shell.
#[must_use]
pub fn burst_boost(est_alive: f32, particle_count: f32, cap: f32) -> f32 {
    let density = (est_alive / particle_count.max(1.0)).clamp(0.0, 1.0);
    (1.0 + (cap - 1.0) * (1.0 - density)).clamp(1.0, cap)
}

/// Apportion the **shared** particle budget across body slots: normalized
/// fade-weighted spawn shares (density stays constant as dancers come and
/// go — four dancers each get a quarter of the flame, not four flames).
///
/// - A slot with no edge points this frame gets zero share (nothing to
///   spawn on).
/// - An *igniting* slot (fade rising through the low range — a dancer
///   appearing) gets an [`IGNITE_BOOST`]× share so its flame catches with a
///   visible flare; the boost shifts share, never raises the total.
/// - **Background subdue** (crowded-venue grace): each slot's weight is
///   scaled by that body's [`motion_weight`] (floor
///   [`SUBDUE_MOTION_FLOOR`]), blended by `background_subdue` — `0` is the
///   exact legacy behaviour, `1` is the full motion scaling. A static
///   background loiterer burns subdued while dancers burn full; because the
///   weights renormalize, equal motion across all bodies (including "all
///   still") cancels out and nothing changes. `motions[i]` is
///   `TrackedBody::motion`. Applied CPU-side, before the CDF — the GPU
///   uniform layout is untouched.
/// - When **no** slot carries fade (the attract phantom and the synthetic
///   writers publish mask/edges without `TrackedBody` entries), shares fall
///   back to each slot's edge-count proportion so those single-body paths
///   keep their flame.
/// - All-zero output (no edges anywhere) means "spawn nothing".
#[must_use]
pub fn emission_slot_weights(
    fades: [f32; MAX_TRACKED_BODIES],
    igniting: [bool; MAX_TRACKED_BODIES],
    counts: [usize; MAX_TRACKED_BODIES],
    motions: [f32; MAX_TRACKED_BODIES],
    background_subdue: f32,
) -> [f32; MAX_TRACKED_BODIES] {
    let subdue = background_subdue.clamp(0.0, 1.0);
    let mut weights = [0.0_f32; MAX_TRACKED_BODIES];
    let mut sum = 0.0_f32;
    for i in 0..MAX_TRACKED_BODIES {
        if counts[i] == 0 {
            continue;
        }
        let boost = if igniting[i] { IGNITE_BOOST } else { 1.0 };
        // 1 at subdue 0 (knob off — provably identical to the pre-knob
        // behaviour); eases toward the motion weight as the knob rises.
        let grace = 1.0 - subdue * (1.0 - motion_weight(motions[i], SUBDUE_MOTION_FLOOR));
        weights[i] = fades[i].clamp(0.0, 1.0) * boost * grace;
        sum += weights[i];
    }
    if sum <= f32::EPSILON {
        // Phantom/synthetic fallback: no tracked fades but edges exist.
        let total: usize = counts.iter().sum();
        if total == 0 {
            return [0.0; MAX_TRACKED_BODIES];
        }
        #[allow(
            clippy::as_conversions,
            clippy::cast_precision_loss,
            reason = "edge counts are bounded by MAX_EDGE_POINTS (2048), exact in f32"
        )]
        for (w, &c) in weights.iter_mut().zip(counts.iter()) {
            *w = c as f32 / total as f32;
        }
        return weights;
    }
    for w in &mut weights {
        *w /= sum;
    }
    weights
}

/// Fold normalized weights into the monotone CDF the kernel samples
/// (`pick_slot`: first `i` with `rand < cdf[i]`). All-zero weights stay an
/// all-zero CDF, which the kernel reads as "no live slot".
#[must_use]
pub fn weights_to_cdf(weights: [f32; MAX_TRACKED_BODIES]) -> [f32; MAX_TRACKED_BODIES] {
    let mut cdf = [0.0_f32; MAX_TRACKED_BODIES];
    let mut acc = 0.0_f32;
    for (c, w) in cdf.iter_mut().zip(weights.iter()) {
        acc += w;
        *c = acc;
    }
    cdf
}

/// One baker, two writers (live + screensaver) — flame's Condition A1.
///
/// Advances the [`RadianceState`] envelopes (onset attack/release, smoothed
/// intensity/sparkle/bass, palette shift), then writes every field of the
/// kernel uniform: audio-scaled emission/buoyancy/turbulence, the
/// beat-weighted swell, the onset ejecta lane, the per-slot edge ranges +
/// fade-weighted emission CDF (multi-body budget apportioning), the
/// mask-UV→world transform for the current window + mirror setting, and up
/// to [`MAX_IMPULSES`] limb impulse slots fanned across every present body.
#[allow(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "edge/particle counts are bounded (MAX_EDGE_POINTS / the 300k \
              particle slider); usize -> u32 and u32 -> f32 are exact in range"
)]
#[allow(
    clippy::too_many_arguments,
    reason = "a pure baker's parameters are its data dependencies; the \
              elapsed-alongside-dt pair is what lets the screensaver \
              performer (Task 12) drive the same function on its own virtual \
              clock, and particle_count is what the density-adaptive beat \
              burst measures fullness against, instead of duplicating the \
              kernel-uniform write"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one baker, two writers (Condition A1): the linear write of \
              every kernel-uniform lane is the single source of truth for \
              the audio/body -> sim derivation; splitting it would scatter \
              the lane order the WGSL struct mirrors"
)]
pub fn bake_radiance_sim(
    settings: &RadianceSettings,
    audio: &AudioAnalysis,
    bodies: Option<&BodyTrackingState>,
    slot_counts: [usize; MAX_TRACKED_BODIES],
    mask_frame_aspect: f32,
    particle_count: u32,
    window_size: Vec2,
    dt: f32,
    elapsed: f32,
    state: &mut RadianceState,
    out: &mut RadianceSimParamsGpu,
) {
    let dt = dt.min(DT_CAP);
    let sensitivity = settings.audio_sensitivity.max(0.0);
    // "Beat pulses": the operator master over the beat-synchronized visuals.
    // Scales the flare-wave brightness and the beat-burst factor below; 0
    // silences both without touching the fine-grained Dev gains.
    let pulse_master = settings.pulse_intensity.max(0.0);
    // Advance the slow per-aggregate running means the band drives are
    // normalized by (room-adaptive contrast expansion — see `band_drive`).
    let (bass_raw, highs_raw) = band_aggregates(audio);
    let kn = 1.0 - (-dt / BAND_NORM_TAU_S).exp();
    state.bass_avg += (bass_raw - state.bass_avg) * kn;
    state.highs_avg += (highs_raw - state.highs_avg) * kn;
    let drive = audio_drive(audio, sensitivity, state.bass_avg, state.highs_avg);

    // Onset envelope: instant attack to the incoming strength, exponential
    // release — so one drum hit reads as one burst, not a sustained gale.
    let released = state.onset_env * (-dt / ONSET_DECAY_SECS).exp();
    state.onset_env = released.max(drive.onset);
    // Smoothed intensity/sparkle/bass (one-pole toward the drive targets).
    let k = 1.0 - (-dt / ENVELOPE_SMOOTH_SECS).exp();
    state.intensity += (drive.intensity - state.intensity) * k;
    state.sparkle += (drive.sparkle - state.sparkle) * k;
    state.bass_drive += (drive.bass - state.bass_drive) * k;
    // Palette drifts slowly, faster under bass (audio-shifted gradient).
    state.palette_shift =
        (state.palette_shift + dt * (0.02 + 0.10 * (drive.emission_mul - 1.0))).fract();
    // Hue rotation phase: the psychedelic full-spectrum drift. Base rate from
    // the setting, accelerated up to ~2.8x by the bass drive so heavy
    // sections push the whole palette around the wheel. The mid/high lane
    // adds a subtle shimmer-rate term (spec: highs drive color shimmer,
    // never the big pulses).
    state.hue_phase = (state.hue_phase
        + dt * settings.hue_cycle_speed
            * (1.0 + 0.6 * (drive.emission_mul - 1.0) + 0.4 * state.sparkle))
        .fract();

    // Beat swell: the debounced beat lane pumps emission + buoyancy so the
    // whole flame visibly SWELLS on the beat (bass-weighted per the spec —
    // this multiplies the bass-derived drive, it does not replace it).
    let beat_swell = 1.0 + 0.5 * audio.beat_confidence * sensitivity.min(1.5);

    out.dt = dt;
    out.time = elapsed;
    // Monotonic per-bake counter salting the kernel's respawn hash. Wraps
    // freely (the hash tolerates it) and, unlike the old `u32(time * 60.0)`
    // salt, never aliases when `elapsed` is pinned or two bakes fall in the
    // same 1/60 s bucket.
    out.frame = out.frame.wrapping_add(1);
    out.emission_prob =
        (settings.emission_rate * drive.emission_mul * beat_swell * EMISSION_BASE_HZ * dt)
            .clamp(0.0, 1.0);
    // Density estimate: advance the deterministic expected-alive recurrence
    // on this frame's pre-burst emission (the GPU owns the real
    // births/deaths; this expectation only feeds the burst boost below).
    // Only bakes advance it, so it holds stale through Idle while the real
    // field dies — see the `est_alive` field doc for the accepted
    // under-boost bound.
    let count_f = particle_count as f32;
    state.est_alive = expected_alive_step(
        state.est_alive,
        out.emission_prob,
        count_f,
        dt,
        (LIFESPAN_MIN + LIFESPAN_MAX) * 0.5,
    );
    out.spawn_offset = SPAWN_OFFSET;
    out.spawn_speed = SPAWN_SPEED * (0.6 + 0.4 * state.intensity);
    out.burst_speed = state.onset_env * BURST_SPEED;
    // Density-adaptive beat burst: on a beat rising edge (same BEAT_EDGE
    // bookkeeping as the wave clock's `step_pulses`), pump this bake's
    // emission by the inverse-density boost — a beat from near-empty water
    // births a visibly larger shell (the legibility floor) — and add the
    // outward kick through the existing burst lane. The kick lasts exactly
    // this bake: the lane is rewritten from the onset envelope next bake.
    let rising = audio.beat_confidence > BEAT_EDGE && state.prev_beat <= BEAT_EDGE;
    state.prev_beat = audio.beat_confidence;
    if rising {
        let boost = burst_boost(state.est_alive, count_f, settings.burst_boost_cap);
        // Spec contract: the burst is "scaled by the bass-weighted beat
        // strength". beat_confidence snaps to 1.0 at every rising edge, so
        // on its own it carries no weight — the old overlay's drive shape
        // supplies it (the 0.35 floor keeps soft beats visible, the bass
        // body carries the rest). pulse_master is the operator's "Beat
        // pulses" knob.
        let bass_weight = 0.35 + 0.65 * state.bass_drive;
        let burst = settings.burst_scale * pulse_master * audio.beat_confidence * bass_weight;
        out.emission_prob = (out.emission_prob * (1.0 + burst * boost)).clamp(0.0, 1.0);
        out.burst_speed += BURST_SPEED * burst;
    }
    out.buoyancy = settings.buoyancy * drive.buoyancy_mul * beat_swell;
    out.flow_strength = settings.flow_strength * drive.turbulence_mul;
    out.curl_scale = settings.curl_scale.max(0.0);
    out.curl_evolve = settings.curl_evolve.max(0.0);
    out.impulse_coupling = settings.impulse_coupling.max(0.0);
    out.curl_octaves = settings.curl_octaves.clamp(1, 3);
    out.drag_baked = DRAG_PER_SECOND.powf(dt);
    out.lifespan_min = LIFESPAN_MIN;
    out.lifespan_max = LIFESPAN_MAX;
    out.mirror = u32::from(settings.mirror);

    // Ejecta lane: onsets convert a fraction of spawns into fast shooting
    // sparks (the kernel rolls per spawn; render streaks them by velocity).
    out.ejecta_prob = (settings.ejecta_amount
        * (EJECTA_BASE_FRACTION + EJECTA_ONSET_FRACTION * (state.onset_env / ONSET_MAX)))
        .clamp(0.0, 1.0);
    out.ejecta_speed = EJECTA_SPEED * (0.8 + 0.4 * state.intensity);
    // Flame tongues: buoyancy noise amplitude breathes with the bass drive
    // (the tongue multiplier can dip briefly ~zero at full strength + full
    // bass — a transient local downdraft reads as organic flicker).
    out.tongue_amp = settings.tongue_strength * (0.55 + 0.5 * state.bass_drive);
    out.tongue_freq = TONGUE_FREQ;
    // Motion-emission bias: the kernel's rejection sampler reads it as a
    // probability floor (1 - bias), so clamp to the unit range here.
    out.edge_motion_bias = settings.edge_motion_bias.clamp(0.0, 1.0);

    // Silhouette-field couplings (the bioluminescence rework): the kernel
    // reads the signed distance field for the repel force + contact glow.
    // The radius floors at 1 px (it divides the falloff, mirroring the
    // kernel's max(imp.radius, 1.0) guard); glow decays framerate-
    // independently exactly like drag.
    out.repel_strength = settings.repel_strength.max(0.0);
    out.repel_radius_px = settings.repel_radius.max(1.0);
    out.contact_glow = settings.contact_glow.max(0.0);
    out.glow_decay_baked = GLOW_PER_SECOND.powf(dt);
    // Flare-wave gains: the kernel brightens particles as the beat waves
    // (baked by `pulse::advance_beat_waves` into the wave lanes) pass their
    // exterior distance. The band floors at 1 px (it divides the Gaussian).
    // The operator's "Beat pulses" master rides on the Dev gain.
    out.flare_gain = settings.flare_gain.max(0.0) * pulse_master;
    out.flare_band_px = settings.flare_band.max(1.0);
    // Motion glow: the impulse loop's disturbance-is-luminous coupling.
    out.motion_glow = settings.motion_glow.max(0.0);

    // Per-slot edge ranges: `SilhouetteEdges` concatenates slots ascending,
    // so starts are the prefix sums; counts clamp so `start + count` stays
    // inside the uploaded MAX_EDGE_POINTS prefix.
    let mut start = 0_usize;
    let mut fades = [0.0_f32; MAX_TRACKED_BODIES];
    let mut igniting = [false; MAX_TRACKED_BODIES];
    let mut motions = [0.0_f32; MAX_TRACKED_BODIES];
    let mut clamped_counts = [0_usize; MAX_TRACKED_BODIES];
    for i in 0..MAX_TRACKED_BODIES {
        let clamped = slot_counts[i].min(MAX_EDGE_POINTS.saturating_sub(start));
        out.slot_start[i] = start as u32;
        out.slot_count[i] = clamped as u32;
        clamped_counts[i] = clamped;
        start += clamped;
    }
    out.edge_count = start as u32;

    // Per-slot fades + the ignite detector (fade rising through the low
    // range = a dancer appearing; their flame catches with a flare).
    if let Some(bodies) = bodies {
        for body in bodies.iter_bodies() {
            if body.slot < MAX_TRACKED_BODIES {
                let fade = body.fade.clamp(0.0, 1.0);
                fades[body.slot] = fade;
                igniting[body.slot] =
                    fade > state.slot_fade_prev[body.slot] + 1e-4 && fade < IGNITE_FADE_CEIL;
                // The publisher's smoothed motion envelope (held through the
                // fade-out tail) — feeds the background-subdue grace.
                motions[body.slot] = body.motion;
            }
        }
    }
    out.slot_cdf = weights_to_cdf(emission_slot_weights(
        fades,
        igniting,
        clamped_counts,
        motions,
        settings.background_subdue,
    ));
    state.slot_fade_prev = fades;

    // Mask → world scale: the aspect-fit rect (see [`mask_fit_rect`]). Every
    // consumer (fill, rim, edges, limb impulses, sparkles) reads
    // `uv_to_world` or its silhouette-shader mirror, so this one line keeps
    // them consistent.
    out.uv_to_world = mask_fit_rect(window_size, mask_frame_aspect, settings.frame_fit).to_array();

    // Limb impulses from the smoothed landmark velocities.
    bake_impulses(bodies, settings.mirror, &mut state.impulse_latch, out);
    // particle_count is owned by spawn (buffer size); the baker leaves it.
}

/// The mask's world-space rect: the camera frame placed in the window at its
/// true proportions, centred, per the [`RadianceFrameFit`] mode.
///
/// Mask-UV `[0,1]²` is NOT a square view of the world —
/// `ContentRect::to_content_norm` divides each camera axis out
/// independently, so the unit square is the camera frame *squished* square.
/// Un-squishing by `frame_aspect` is what restores the dancer's true
/// proportions, and it is unconditional: **neither mode ever stretches the
/// body.** `frame_aspect` is stamped by whoever wrote the mask (camera
/// worker: the real frame aspect; attract circle / debug dancer:
/// square-authored, 1.0).
///
/// The modes differ only on the axis where window and camera aspects
/// disagree — the rect is always `w × w/aspect`, only `w` differs:
/// - [`RadianceFrameFit::FillHeight`] — the frame spans the window height.
///   A window *narrower* than the camera aspect (portrait) crops the sides;
///   a wider one (ultrawide) pillarboxes, since height is already the
///   binding axis there.
/// - [`RadianceFrameFit::Fit`] — the frame fits entirely inside the window
///   ("contain"), so nothing is ever cropped on any aspect; the mismatched
///   axis gets margins.
///
/// When the window aspect equals the camera aspect both yield the full
/// window exactly (this installation: a 16:9 camera on a 16:9 panel).
#[must_use]
pub fn mask_fit_rect(window: Vec2, frame_aspect: f32, fit: RadianceFrameFit) -> Vec2 {
    let aspect = frame_aspect.max(0.1);
    let win = Vec2::new(window.x.max(1.0), window.y.max(1.0));
    // Width the window's height allows at this aspect. `FillHeight` takes it
    // verbatim (overflowing width = the side crop); `Fit` also clamps to the
    // window's own width, so the frame never exceeds either axis.
    let height_bound = win.y * aspect;
    let w = match fit {
        RadianceFrameFit::FillHeight => height_bound,
        RadianceFrameFit::Fit => win.x.min(height_bound),
    };
    Vec2::new(w, w / aspect)
}

/// Per-axis `window / mask_rect` ratios for the silhouette shader's UV
/// remap: `1.0` on an axis that spans the window, `> 1.0` on a boxed axis
/// (samples land outside `[0,1]` in the margin and are discarded), `< 1.0`
/// on a cropped axis (the frame's outer edges fall beyond the window).
/// Derived from [`mask_fit_rect`] so fill and rim agree with the particles.
#[must_use]
pub fn mask_fit_uv_scale(window: Vec2, frame_aspect: f32, fit: RadianceFrameFit) -> Vec2 {
    let rect = mask_fit_rect(window, frame_aspect, fit);
    Vec2::new(window.x.max(1.0), window.y.max(1.0)) / rect
}

/// Fan the limb impulses across EVERY present body in slot order until the
/// eight [`MAX_IMPULSES`] slots fill (one dancer uses at most seven, so a
/// duo always gets at least one slot). Each source is one of
/// [`IMPULSE_SOURCES`] — nose, wrists (falling back to their elbow when the
/// wrist is the less-visible joint), hips, ankles — gated through a
/// per-slot Schmitt [`VisibilityLatch`] so a marginal landmark holds its
/// last decision instead of strobing the impulse layer. Stale slots past the
/// live count are zeroed so a limb dropping out of frame cannot leave a ghost
/// impulse.
#[allow(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "impulse count <= MAX_IMPULSES (8); usize -> u32 is exact"
)]
fn bake_impulses(
    bodies: Option<&BodyTrackingState>,
    mirror: bool,
    latches: &mut [[VisibilityLatch; IMPULSE_SOURCE_COUNT]; MAX_TRACKED_BODIES],
    out: &mut RadianceSimParamsGpu,
) {
    let scale = Vec2::new(out.uv_to_world[0], out.uv_to_world[1]);
    let mut n = 0usize;
    if let Some(bodies) = bodies {
        // Absent/empty slots close their latches so a slot's next occupant
        // starts from the strict admission bar, not a stale open gate.
        for (slot, entry) in bodies.bodies.iter().enumerate() {
            if entry.as_ref().is_none_or(|b| !b.present) {
                for latch in &mut latches[slot] {
                    latch.reset();
                }
            }
        }
        'bodies: for body in bodies.iter_bodies() {
            if !body.present {
                continue;
            }
            for (i, &(primary, fallback)) in IMPULSE_SOURCES.iter().enumerate() {
                if n >= MAX_IMPULSES {
                    break 'bodies;
                }
                // Prefer the primary joint; an occluded wrist hands the
                // arm's impulse to its elbow rather than silencing the arm.
                let lm = match fallback {
                    Some(fb)
                        if body.landmarks[fb].visibility > body.landmarks[primary].visibility =>
                    {
                        fb
                    }
                    _ => primary,
                };
                let landmark = body.landmarks[lm];
                if !latches[body.slot][i].step(landmark.visibility) {
                    continue;
                }
                let vel = mask_dir_to_world(
                    Vec2::new(body.velocities[lm].x, body.velocities[lm].y),
                    scale,
                    mirror,
                )
                .clamp_length_max(IMPULSE_MAX_SPEED);
                let gain = (vel.length() / IMPULSE_FULL_SPEED).clamp(0.0, 1.0);
                if gain < 0.05 {
                    continue; // resting limbs shed nothing
                }
                let pos =
                    mask_uv_to_world(Vec2::new(landmark.pos.x, landmark.pos.y), scale, mirror);
                out.impulses[n] = RadianceImpulse {
                    position: pos.into(),
                    velocity: vel.into(),
                    radius: IMPULSE_RADIUS,
                    gain,
                    _pad: [0.0; 2],
                };
                n += 1;
            }
        }
    } else {
        for slot in latches.iter_mut() {
            for latch in slot.iter_mut() {
                latch.reset();
            }
        }
    }
    for slot in out.impulses.iter_mut().skip(n) {
        *slot = RadianceImpulse::default();
    }
    out.impulse_count = n as u32;
}

/// `Update` (gated `sketch_active(AppState::Radiance)`): the live writer.
/// Gathers the current analysis/body/edges resources (all optional — the
/// sketch degrades to motion-only or emission-only gracefully) and bakes.
pub fn update_radiance_sim(
    time: Res<'_, Time>,
    window: Single<'_, '_, &Window>,
    settings: Res<'_, RadianceSettings>,
    audio: Option<Res<'_, AudioAnalysis>>,
    body: Option<Res<'_, BodyTrackingState>>,
    edges: Option<Res<'_, SilhouetteEdges>>,
    mut state: ResMut<'_, RadianceState>,
    mut sim: ResMut<'_, RadianceSimParams>,
) {
    let audio_frame = audio.map_or_else(neutral_audio, |a| *a);
    let (slot_counts, mask_frame_aspect) = edges.map_or(([0; MAX_TRACKED_BODIES], 1.0), |e| {
        (e.slot_counts, e.frame_aspect)
    });
    let window_size = Vec2::new(window.width(), window.height());
    // Copied out first: the baker borrows `sim.params` mutably.
    let particle_count = sim.particle_count;
    bake_radiance_sim(
        &settings,
        &audio_frame,
        body.as_deref(),
        slot_counts,
        mask_frame_aspect,
        particle_count,
        window_size,
        time.delta_secs(),
        time.elapsed_secs(),
        &mut state,
        &mut sim.params,
    );
}

/// `OnEnter(SketchActivity::Idle)` (gated `in_state(AppState::Radiance)`):
/// zero emission and the burst so the aura fades out over one lifespan while
/// the throttled last frames hold — flame's freeze idiom, adapted to a
/// particle field that must die out rather than stop mid-air. Once the field
/// is deterministically all-dead, [`update_radiance_pause`] stops the
/// dispatch and the billboard draw entirely.
///
/// The glow/repel/flare gains are deliberately NOT zeroed here: Idle keeps
/// decaying glow via the still-running kernel — the baker simply stops
/// updating, which leaves the last baked `glow_decay_baked` (and the gains)
/// constant in place. Correct as-is: with emission zero the surviving
/// particles just fade their residual glow out.
pub fn freeze_radiance_emission(mut sim: ResMut<'_, RadianceSimParams>) {
    sim.params.emission_prob = 0.0;
    sim.params.burst_speed = 0.0;
    sim.params.ejecta_prob = 0.0;
}

/// Safety margin on top of [`LIFESPAN_MAX`] before the frozen aura is
/// declared all-dead and paused.
pub const PAUSE_MARGIN_S: f32 = 0.5;

/// Simulated seconds of continuously-zero emission after which every
/// particle is deterministically dead: the kernel assigns lifespans in
/// `[LIFESPAN_MIN, LIFESPAN_MAX]`, so the youngest possible particle — born
/// at the last instant emission was nonzero — is dead once [`LIFESPAN_MAX`]
/// simulated seconds have elapsed; [`PAUSE_MARGIN_S`] is slack on top.
/// Derived from the same [`LIFESPAN_MAX`] the baker writes into the kernel
/// uniform (the single source of truth), never re-hardcoded.
pub const PAUSE_BOUND_S: f32 = LIFESPAN_MAX + PAUSE_MARGIN_S;

/// One pause-bookkeeping step, pure for testability: given the current
/// frozen-clock/paused pair and this frame's `emission_prob` + kernel `dt`,
/// return the next pair.
///
/// - Any nonzero emission (live bake, screensaver ember bake) resets the
///   clock and resumes immediately.
/// - With emission zero, the clock advances by the kernel `dt` — the exact
///   amount one dispatch ages every particle, so the bound is met in
///   *simulated* time regardless of Idle frame throttling — and pauses only
///   at [`PAUSE_BOUND_S`]; it can never fire while a particle is alive.
/// - The clock clamps at the bound so a settled pause stops changing state
///   (no per-frame change-detection churn on the extract resource).
#[must_use]
pub fn step_radiance_pause(
    frozen_secs: f32,
    paused: bool,
    emission_prob: f32,
    dt: f32,
) -> (f32, bool) {
    if emission_prob > 0.0 {
        return (0.0, false);
    }
    let frozen = (frozen_secs + dt.max(0.0)).min(PAUSE_BOUND_S);
    (frozen, paused || frozen >= PAUSE_BOUND_S)
}

/// `Update` (gated `in_state(AppState::Radiance)`, after the sim writers):
/// advance the pause bookkeeping and flip the billboard entity's visibility
/// on pause transitions.
///
/// While `paused`, the render world maps the flag to a dispatch size of 0
/// (no compute workgroups for an all-dead field) and the hidden billboard
/// skips the count × 6 vertex draw — the two costs an idle Radiance
/// otherwise pays forever. The screensaver's ember bake writes nonzero
/// emission, so entering the attract mode (or returning to Active) resumes
/// both within one frame, before any new particle could be born *and*
/// rendered.
pub fn update_radiance_pause(
    mut sim: ResMut<'_, RadianceSimParams>,
    mut billboards: Query<
        '_,
        '_,
        &mut Visibility,
        (
            With<crate::radiance::systems::spawn::RadianceRoot>,
            With<bevy::sprite_render::MeshMaterial2d<crate::radiance::render::RadianceMaterial>>,
        ),
    >,
) {
    let (frozen, paused) = step_radiance_pause(
        sim.frozen_secs,
        sim.paused,
        sim.params.emission_prob,
        sim.params.dt,
    );
    let was_paused = sim.paused;
    // Write only on change so a settled state stops dirtying the extract
    // resource (and the render-world copy stops being re-extracted).
    if (frozen - sim.frozen_secs).abs() > 0.0 || paused != was_paused {
        sim.frozen_secs = frozen;
        sim.paused = paused;
    }
    if paused != was_paused {
        let desired = if paused {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
        for mut visibility in &mut billboards {
            *visibility = desired;
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test assertions")]
mod tests {
    use super::*;
    use wc_core::input::body::{BodyLandmark, TrackedBody, BODY_LANDMARK_COUNT};

    fn fixture_audio(bands: [f32; 8], rms: f32, onset: f32) -> AudioAnalysis {
        AudioAnalysis {
            rms,
            gain: 1.0,
            bands,
            onset,
            beat_confidence: 0.0,
            peak: 0.0,
            active: true,
        }
    }

    fn fixture_body(wrist_vel: Vec3) -> TrackedBody {
        let mut landmarks = [BodyLandmark::default(); BODY_LANDMARK_COUNT];
        for lm in &mut landmarks {
            lm.visibility = 1.0;
            lm.pos = Vec3::new(0.5, 0.5, 0.0);
        }
        // Right wrist (16) moving.
        landmarks[16].pos = Vec3::new(0.7, 0.4, 0.0);
        let mut velocities = [Vec3::ZERO; BODY_LANDMARK_COUNT];
        velocities[16] = wrist_vel;
        TrackedBody {
            slot: 0,
            present: true,
            fade: 1.0,
            confidence: 0.9,
            landmarks,
            velocities,
            timestamp: std::time::Duration::from_millis(33),
            crop_fraction: 1.0,
            size: 0.2,
            ..TrackedBody::default()
        }
    }

    /// Wrap a single body (in its own slot) into the tracking-state shape
    /// the baker consumes.
    fn tracking_state(body: TrackedBody) -> BodyTrackingState {
        let mut state = BodyTrackingState::default();
        let slot = body.slot.min(MAX_TRACKED_BODIES - 1);
        state.primary = body.present.then_some(slot);
        state.bodies[slot] = Some(body);
        state
    }

    fn bake(
        settings: &RadianceSettings,
        audio: &AudioAnalysis,
        bodies: Option<&BodyTrackingState>,
        edge_count: usize,
    ) -> (RadianceState, RadianceSimParamsGpu) {
        let mut state = RadianceState::default();
        let mut out = RadianceSimParamsGpu::default();
        bake_radiance_sim(
            settings,
            audio,
            bodies,
            [edge_count, 0, 0, 0],
            1.0,
            120_000,
            Vec2::new(1920.0, 1080.0),
            1.0 / 60.0,
            10.0,
            &mut state,
            &mut out,
        );
        (state, out)
    }

    /// Mirror on: UV x flips around center; y flips down→up. Golden points.
    #[test]
    fn mask_uv_to_world_maps_and_mirrors() {
        let scale = Vec2::new(1920.0, 1080.0);
        // Center maps to origin either way.
        assert_eq!(
            mask_uv_to_world(Vec2::new(0.5, 0.5), scale, false),
            Vec2::ZERO
        );
        assert_eq!(
            mask_uv_to_world(Vec2::new(0.5, 0.5), scale, true),
            Vec2::ZERO
        );
        // UV (0,0) is the top-left of the mask -> left edge, top of screen.
        let tl = mask_uv_to_world(Vec2::new(0.0, 0.0), scale, false);
        assert_eq!(tl, Vec2::new(-960.0, 540.0));
        // Mirrored, the same UV lands on the RIGHT edge.
        let tl_m = mask_uv_to_world(Vec2::new(0.0, 0.0), scale, true);
        assert_eq!(tl_m, Vec2::new(960.0, 540.0));
        // Directions: mask +y (down) maps to world -y; mirror negates x.
        let d = mask_dir_to_world(Vec2::new(1.0, 1.0), scale, false);
        assert!(d.x > 0.0 && d.y < 0.0);
        let d_m = mask_dir_to_world(Vec2::new(1.0, 1.0), scale, true);
        assert!(d_m.x < 0.0 && d_m.y < 0.0);
    }

    /// The frame-fit geometry, on every display aspect. The invariant that
    /// matters most: **neither mode ever distorts** — the mapped rect always
    /// carries the camera's aspect, whatever the window is.
    #[test]
    fn frame_fit_never_distorts_on_any_display_aspect() {
        let cam = 16.0 / 9.0;
        let windows = [
            ("matched 16:9", Vec2::new(1920.0, 1080.0)),
            ("laptop 16:10", Vec2::new(2560.0, 1600.0)),
            ("ultrawide 21:9", Vec2::new(3440.0, 1440.0)),
            ("portrait 9:16", Vec2::new(1080.0, 1920.0)),
            ("square", Vec2::new(1200.0, 1200.0)),
        ];
        for (name, win) in windows {
            for fit in [RadianceFrameFit::FillHeight, RadianceFrameFit::Fit] {
                let rect = mask_fit_rect(win, cam, fit);
                assert!(
                    (rect.x / rect.y - cam).abs() < 1e-3,
                    "{name} / {fit:?} distorts: {rect:?}"
                );
                // The UV scale is the exact inverse mapping, so fill/rim and
                // particles cannot disagree.
                let uv = mask_fit_uv_scale(win, cam, fit);
                assert!((uv.x - win.x / rect.x).abs() < 1e-4, "{name} / {fit:?}");
                assert!((uv.y - win.y / rect.y).abs() < 1e-4, "{name} / {fit:?}");
            }
        }
    }

    /// Mode semantics: `FillHeight` spans the window height (cropping the
    /// sides on a portrait screen — the zoom/crop look the operator asked to
    /// keep); `Fit` never crops on any aspect. They agree exactly when the
    /// window matches the camera.
    #[test]
    fn fill_height_crops_portrait_sides_while_fit_never_crops() {
        let cam = 16.0 / 9.0;

        // Matched 16:9: both modes are the full window, no margin, no crop.
        let win = Vec2::new(1920.0, 1080.0);
        for fit in [RadianceFrameFit::FillHeight, RadianceFrameFit::Fit] {
            let r = mask_fit_rect(win, cam, fit);
            assert!(
                (r.x - 1920.0).abs() < 0.1 && (r.y - 1080.0).abs() < 0.1,
                "{fit:?} {r:?}"
            );
        }

        // Portrait 9:16 — the mode that matters. FillHeight spans the height
        // (1920) and overflows the 1080-wide window: the sides crop.
        let portrait = Vec2::new(1080.0, 1920.0);
        let filled = mask_fit_rect(portrait, cam, RadianceFrameFit::FillHeight);
        assert!(
            (filled.y - 1920.0).abs() < 0.1,
            "spans the height: {filled:?}"
        );
        assert!(filled.x > portrait.x, "sides crop: {filled:?}");
        // Its UV x scale is < 1: the frame's outer edges fall outside the
        // window rather than the window falling outside the frame.
        let filled_uv = mask_fit_uv_scale(portrait, cam, RadianceFrameFit::FillHeight);
        assert!(
            filled_uv.x < 1.0 && (filled_uv.y - 1.0).abs() < 1e-4,
            "{filled_uv:?}"
        );

        // Fit letterboxes instead: the whole frame is on screen.
        let fitted = mask_fit_rect(portrait, cam, RadianceFrameFit::Fit);
        assert!(
            fitted.x <= portrait.x + 0.1 && fitted.y <= portrait.y + 0.1,
            "{fitted:?}"
        );
        let fitted_uv = mask_fit_uv_scale(portrait, cam, RadianceFrameFit::Fit);
        assert!(
            fitted_uv.y > 1.0,
            "vertical letterbox margin: {fitted_uv:?}"
        );

        // Ultrawide: height is already the binding axis, so both pillarbox
        // identically — the modes only diverge on windows narrower than the
        // camera aspect.
        let ultrawide = Vec2::new(3440.0, 1440.0);
        let a = mask_fit_rect(ultrawide, cam, RadianceFrameFit::FillHeight);
        let b = mask_fit_rect(ultrawide, cam, RadianceFrameFit::Fit);
        assert!((a - b).length() < 0.1, "{a:?} vs {b:?}");
        assert!(a.x < ultrawide.x, "pillarboxed: {a:?}");
    }

    /// The baker publishes the fit rect verbatim as `uv_to_world`, so the
    /// particle kernel shares the fill/rim geometry.
    #[test]
    fn bake_publishes_the_frame_fit_rect_as_uv_to_world() {
        let cam = 16.0 / 9.0;
        let win = Vec2::new(1080.0, 1920.0); // portrait
        for fit in [RadianceFrameFit::FillHeight, RadianceFrameFit::Fit] {
            let settings = RadianceSettings {
                frame_fit: fit,
                ..RadianceSettings::default()
            };
            let mut state = RadianceState::default();
            let mut out = RadianceSimParamsGpu::default();
            bake_radiance_sim(
                &settings,
                &neutral_audio(),
                None,
                [100, 0, 0, 0],
                cam,
                120_000,
                win,
                1.0 / 60.0,
                0.0,
                &mut state,
                &mut out,
            );
            let expect = mask_fit_rect(win, cam, fit);
            assert!(
                (out.uv_to_world[0] - expect.x).abs() < 0.1
                    && (out.uv_to_world[1] - expect.y).abs() < 0.1,
                "{fit:?}: {:?} vs {expect:?}",
                out.uv_to_world
            );
        }
    }

    /// Sensitivity 0 (or silent input) is the exact neutral drive: every
    /// multiplier 1.0, no burst — audio coupling provably inert.
    #[test]
    fn audio_drive_neutral_at_zero_sensitivity() {
        let loud = fixture_audio([1.0; 8], 1.0, 1.0);
        let d = audio_drive(&loud, 0.0, 0.5, 0.5);
        assert!((d.emission_mul - 1.0).abs() < f32::EPSILON);
        assert!((d.buoyancy_mul - 1.0).abs() < f32::EPSILON);
        assert!((d.turbulence_mul - 1.0).abs() < f32::EPSILON);
        assert!(d.sparkle.abs() < f32::EPSILON);
        assert!(d.onset.abs() < f32::EPSILON);
    }

    /// Bass raises emission + buoyancy; highs raise turbulence + sparkle.
    /// References at half the aggregate: ratio 2 saturates both drives.
    #[test]
    fn audio_drive_routes_bands_per_spec() {
        let bassy = fixture_audio([0.9, 0.9, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0], 0.3, 0.0);
        let airy = fixture_audio([0.0, 0.0, 0.0, 0.0, 0.0, 0.9, 0.9, 0.9], 0.3, 0.0);
        let db = audio_drive(&bassy, 1.0, 0.45, 0.45);
        let da = audio_drive(&airy, 1.0, 0.45, 0.45);
        assert!(db.emission_mul > 1.5 && db.buoyancy_mul > 1.2);
        assert!(
            (db.turbulence_mul - 1.0).abs() < 1e-6,
            "bass must not stir turbulence"
        );
        assert!(da.turbulence_mul > 1.5 && da.sparkle > 0.5);
        assert!(
            (da.emission_mul - 1.0).abs() < 1e-6,
            "highs must not pump emission"
        );
    }

    /// The relative normalization is the point: a compressed room-mic bass
    /// wiggle (0.15 mean, ±0.06 swing — the measured party-room shape) maps
    /// to a wide drive range instead of the near-static absolute mapping.
    #[test]
    fn band_drive_expands_compressed_room_mic_dynamics() {
        let avg = 0.15;
        let quiet = band_drive(0.09, avg); // p10-ish trough
        let mid = band_drive(0.15, avg); // sitting at the mean
        let peak = band_drive(0.22, avg); // p95-ish hit
        assert!(quiet.abs() < f32::EPSILON, "trough must drop to 0: {quiet}");
        assert!(
            (0.2..=0.6).contains(&mid),
            "at-mean must be moderate: {mid}"
        );
        assert!(peak > 0.9, "hits must approach full drive: {peak}");
    }

    /// Beats throb the intensity target on top of the RMS floor.
    #[test]
    fn audio_drive_intensity_throbs_on_beats() {
        let base = fixture_audio([0.1; 8], 0.16, 0.0);
        let mut on_beat = base;
        on_beat.beat_confidence = 1.0;
        let di = audio_drive(&base, 1.0, 0.1, 0.1).intensity;
        let db = audio_drive(&on_beat, 1.0, 0.1, 0.1).intensity;
        assert!((db - di - 0.3).abs() < 1e-6, "beat adds 0.3: {di} -> {db}");
    }

    /// The baker's running means adapt toward the aggregates, so a sustained
    /// level stops reading as a hit: the emission drive relaxes over time.
    #[test]
    fn bake_normalization_adapts_to_sustained_level() {
        let settings = RadianceSettings::default();
        let mut state = RadianceState::default();
        let mut out = RadianceSimParamsGpu::default();
        let sustained = fixture_audio([0.3; 8], 0.2, 0.0);
        let win = Vec2::new(1920.0, 1080.0);
        let mut first = 0.0;
        // 40 simulated seconds: five BAND_NORM_TAU_S constants, so the mean
        // has fully converged onto the sustained aggregate.
        for i in 0..2400 {
            bake_radiance_sim(
                &settings,
                &sustained,
                None,
                [100, 0, 0, 0],
                1.0,
                120_000,
                win,
                1.0 / 60.0,
                0.0,
                &mut state,
                &mut out,
            );
            if i == 0 {
                first = out.emission_prob;
            }
        }
        assert!(
            out.emission_prob < first,
            "sustained level must relax: first {first}, settled {}",
            out.emission_prob
        );
        assert!(
            (state.bass_avg - 0.3).abs() < 0.02,
            "bass mean converges to the aggregate: {}",
            state.bass_avg
        );
    }

    /// The baker scales emission with the bass drive vs the neutral bake.
    #[test]
    fn bake_bass_raises_emission_prob() {
        // Non-zero buoyancy so the bass-raise lane is observable (the
        // 2026-07-24 calibration default is 0 — the water look).
        let settings = RadianceSettings {
            buoyancy: 135.0,
            ..RadianceSettings::default()
        };
        let quiet = neutral_audio();
        let bassy = fixture_audio([0.9, 0.9, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0], 0.4, 0.0);
        let (_, base) = bake(&settings, &quiet, None, 500);
        let (_, driven) = bake(&settings, &bassy, None, 500);
        assert!(driven.emission_prob > base.emission_prob);
        assert!(driven.buoyancy > base.buoyancy);
        // Expected neutral value: rate * 1.0 * EMISSION_BASE_HZ * dt.
        let expect = settings.emission_rate * EMISSION_BASE_HZ / 60.0;
        assert!((base.emission_prob - expect).abs() < 1e-6);
    }

    /// Onset attacks instantly and releases exponentially across frames.
    #[test]
    fn onset_envelope_attacks_then_decays() {
        let settings = RadianceSettings::default();
        let mut state = RadianceState::default();
        let mut out = RadianceSimParamsGpu::default();
        let hit = fixture_audio([0.0; 8], 0.2, 1.5);
        let silence = neutral_audio();
        let win = Vec2::new(1920.0, 1080.0);
        bake_radiance_sim(
            &settings,
            &hit,
            None,
            [100, 0, 0, 0],
            1.0,
            120_000,
            win,
            1.0 / 60.0,
            0.0,
            &mut state,
            &mut out,
        );
        let peak = out.burst_speed;
        assert!(peak > 0.0, "onset must produce a burst");
        for _ in 0..30 {
            bake_radiance_sim(
                &settings,
                &silence,
                None,
                [100, 0, 0, 0],
                1.0,
                120_000,
                win,
                1.0 / 60.0,
                0.0,
                &mut state,
                &mut out,
            );
        }
        assert!(
            out.burst_speed < peak * 0.1,
            "burst must decay: {} vs peak {peak}",
            out.burst_speed
        );
    }

    /// A fast right wrist produces exactly one impulse slot with a mirrored
    /// world position and a bounded gain; slots past it are zeroed.
    #[test]
    fn bake_bakes_wrist_impulse_with_mirror_mapping() {
        let settings = RadianceSettings::default(); // mirror = true
        let body = tracking_state(fixture_body(Vec3::new(0.8, 0.0, 0.0))); // fast +u sweep
        let (_, out) = bake(&settings, &neutral_audio(), Some(&body), 500);
        assert_eq!(out.impulse_count, 1, "one moving limb -> one slot");
        let imp = out.impulses[0];
        // Wrist at UV (0.7, 0.4), mirrored. The bake's square-authored source
        // aspect (1.0) makes the frame-fit rect the window height square
        // (1080), so world x =
        // (1-0.7-0.5)*1080 = -216; world y = (0.5-0.4)*1080 = 108.
        assert!(
            (imp.position[0] - -216.0).abs() < 1e-3,
            "{:?}",
            imp.position
        );
        assert!((imp.position[1] - 108.0).abs() < 1e-3, "{:?}", imp.position);
        // Mirrored +u velocity points -x in world.
        assert!(imp.velocity[0] < 0.0);
        assert!(imp.gain > 0.0 && imp.gain <= 1.0);
        assert!(
            (out.impulses[1].gain).abs() < f32::EPSILON,
            "stale slots zeroed"
        );
    }

    /// A one-frame landmark teleport (blur artifact) must not blast the
    /// field: the impulse's velocity magnitude is clamped even though its
    /// gain already saturates.
    #[test]
    fn bake_clamps_teleport_impulse_velocity() {
        let settings = RadianceSettings::default();
        // 50 UV/s is far beyond any real limb sweep (~0.1..1.0 UV/s).
        let body = tracking_state(fixture_body(Vec3::new(50.0, 0.0, 0.0)));
        let (_, out) = bake(&settings, &neutral_audio(), Some(&body), 500);
        assert_eq!(out.impulse_count, 1);
        let v = Vec2::new(out.impulses[0].velocity[0], out.impulses[0].velocity[1]);
        assert!(
            v.length() <= IMPULSE_MAX_SPEED + 1e-3,
            "teleport velocity must clamp: {}",
            v.length()
        );
        assert!(
            (out.impulses[0].gain - 1.0).abs() < 1e-6,
            "gain still saturates"
        );
    }

    /// Absent body / present-but-still body bakes zero impulses.
    #[test]
    fn bake_no_body_means_no_impulses() {
        let settings = RadianceSettings::default();
        let (_, out) = bake(&settings, &neutral_audio(), None, 500);
        assert_eq!(out.impulse_count, 0);
        let still = tracking_state(fixture_body(Vec3::ZERO));
        let (_, out) = bake(&settings, &neutral_audio(), Some(&still), 500);
        assert_eq!(out.impulse_count, 0, "resting limbs shed nothing");
    }

    /// A wrist that dips into the marginal visibility band (held fan) keeps
    /// its impulse: the latch holds through 0.35..0.5 once opened.
    #[test]
    fn marginal_wrist_visibility_holds_the_impulse() {
        let mut body = fixture_body(Vec3::new(0.8, 0.0, 0.0));
        let mut latches = [[VisibilityLatch::default(); IMPULSE_SOURCE_COUNT]; MAX_TRACKED_BODIES];
        // Frame 1: clearly visible — opens the latch.
        let state = tracking_state(body.clone());
        // The direct `bake_impulses` path skips `bake_radiance_sim`'s
        // transform write, so seed the mask→world scale it reads.
        let mut out = RadianceSimParamsGpu {
            uv_to_world: [1080.0, 1080.0],
            ..Default::default()
        };
        bake_impulses(Some(&state), false, &mut latches, &mut out);
        assert_eq!(out.impulse_count, 1);
        // Frame 2: marginal (0.42) — a plain 0.5 gate would drop it. The fan
        // dims the whole forearm, so the elbow is no more visible than the
        // wrist (else the fallback would carry the arm on the still elbow);
        // this keeps the moving wrist the chosen joint so the latch hold is
        // what's under test.
        body.landmarks[RIGHT_WRIST].visibility = 0.42;
        body.landmarks[RIGHT_ELBOW].visibility = 0.42;
        let state = tracking_state(body);
        bake_impulses(Some(&state), false, &mut latches, &mut out);
        assert_eq!(out.impulse_count, 1, "latched gate must hold");
    }

    /// A fully occluded wrist hands the arm's impulse to the elbow instead
    /// of silencing the arm.
    #[test]
    fn occluded_wrist_falls_back_to_elbow() {
        let mut body = fixture_body(Vec3::ZERO);
        body.landmarks[RIGHT_WRIST].visibility = 0.1; // fan covers the hand
        body.landmarks[RIGHT_ELBOW].visibility = 0.9;
        body.landmarks[RIGHT_ELBOW].pos = Vec3::new(0.6, 0.45, 0.0);
        body.velocities[RIGHT_ELBOW] = Vec3::new(0.6, 0.0, 0.0); // sweeping arm
        let mut latches = [[VisibilityLatch::default(); IMPULSE_SOURCE_COUNT]; MAX_TRACKED_BODIES];
        let state = tracking_state(body);
        // Seed the mask→world scale the direct path would otherwise inherit
        // from `bake_radiance_sim`.
        let mut out = RadianceSimParamsGpu {
            uv_to_world: [1080.0, 1080.0],
            ..Default::default()
        };
        bake_impulses(Some(&state), false, &mut latches, &mut out);
        assert_eq!(out.impulse_count, 1, "elbow carries the arm's impulse");
    }

    /// Edge count clamps to the contract capacity.
    #[test]
    fn bake_clamps_edge_count() {
        let settings = RadianceSettings::default();
        let (_, out) = bake(&settings, &neutral_audio(), None, MAX_EDGE_POINTS * 4);
        assert_eq!(
            out.edge_count,
            u32::try_from(MAX_EDGE_POINTS).expect("fits")
        );
    }

    /// Fade-weighted apportioning: shares are normalized (constant total
    /// density), zero-edge slots get nothing, and an igniting slot's share
    /// is boosted at its sibling's expense — never the total's. (Subdue off
    /// here; its own tests are below.)
    #[test]
    fn emission_weights_apportion_by_fade() {
        // Two full-fade bodies with edges split the budget evenly.
        let w = emission_slot_weights(
            [1.0, 1.0, 0.0, 0.0],
            [false; 4],
            [300, 300, 0, 0],
            [0.0; 4],
            0.0,
        );
        assert!((w[0] - 0.5).abs() < 1e-6 && (w[1] - 0.5).abs() < 1e-6);
        // A half-faded second body takes a third of the budget.
        let w = emission_slot_weights(
            [1.0, 0.5, 0.0, 0.0],
            [false; 4],
            [300, 300, 0, 0],
            [0.0; 4],
            0.0,
        );
        assert!((w[0] - 2.0 / 3.0).abs() < 1e-6 && (w[1] - 1.0 / 3.0).abs() < 1e-6);
        // A slot with fade but no edges spawns nothing.
        let w = emission_slot_weights(
            [1.0, 1.0, 0.0, 0.0],
            [false; 4],
            [300, 0, 0, 0],
            [0.0; 4],
            0.0,
        );
        assert!((w[0] - 1.0).abs() < 1e-6 && w[1].abs() < f32::EPSILON);
        // Ignite boost shifts share toward the appearing body; sum stays 1.
        let w = emission_slot_weights(
            [1.0, 0.3, 0.0, 0.0],
            [false, true, false, false],
            [300, 300, 0, 0],
            [0.0; 4],
            0.0,
        );
        let boosted = 0.3 * IGNITE_BOOST;
        assert!((w[1] - boosted / (1.0 + boosted)).abs() < 1e-6, "{w:?}");
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    /// Phantom fallback: no fades at all but edges present → edge-count
    /// shares; no edges anywhere → all-zero (spawn nothing).
    #[test]
    #[allow(clippy::float_cmp, reason = "exact zero sentinel comparison")]
    fn emission_weights_phantom_fallback() {
        let w = emission_slot_weights([0.0; 4], [false; 4], [400, 100, 0, 0], [0.0; 4], 0.5);
        assert!((w[0] - 0.8).abs() < 1e-6 && (w[1] - 0.2).abs() < 1e-6);
        let w = emission_slot_weights([0.0; 4], [false; 4], [0; 4], [0.0; 4], 0.5);
        assert_eq!(w, [0.0; 4]);
    }

    /// `background_subdue = 0` is the exact legacy behaviour, whatever the
    /// motion inputs — the venue kill-switch must be provably inert.
    #[test]
    #[allow(clippy::float_cmp, reason = "knob-off must be bit-identical")]
    fn emission_weights_subdue_off_is_identity() {
        let fades = [1.0, 0.4, 0.9, 0.0];
        let igniting = [false, true, false, false];
        let counts = [300, 200, 100, 0];
        let legacy = emission_slot_weights(fades, igniting, counts, [0.0; 4], 0.0);
        let wild_motion = emission_slot_weights(fades, igniting, counts, [0.0, 5.0, 0.3, 9.0], 0.0);
        assert_eq!(legacy, wild_motion, "knob at 0 must ignore motion");
    }

    /// A still background loiterer's share shrinks in favour of a moving
    /// dancer; the total stays normalized; the still body never drops below
    /// the floored fraction of an even split.
    #[test]
    fn emission_weights_subdue_favours_movers() {
        use wc_core::input::body::selection::MOTION_SPEED_HI;
        let fades = [1.0, 1.0, 0.0, 0.0];
        let counts = [300, 300, 0, 0];
        // Slot 0 dances, slot 1 stands still. Full subdue for the clearest
        // split: still weight = SUBDUE_MOTION_FLOOR vs 1.0.
        let w = emission_slot_weights(
            fades,
            [false; 4],
            counts,
            [MOTION_SPEED_HI, 0.0, 0.0, 0.0],
            1.0,
        );
        let expect_still = SUBDUE_MOTION_FLOOR / (1.0 + SUBDUE_MOTION_FLOOR);
        assert!((w[1] - expect_still).abs() < 1e-6, "{w:?}");
        assert!(w[0] > w[1], "the dancer takes the larger share");
        assert!(
            (w.iter().sum::<f32>() - 1.0).abs() < 1e-6,
            "still normalized"
        );
        // Default (modest) strength subdues less than full strength.
        let w_default = emission_slot_weights(
            fades,
            [false; 4],
            counts,
            [MOTION_SPEED_HI, 0.0, 0.0, 0.0],
            0.5,
        );
        assert!(
            w_default[1] > w[1] && w_default[1] < 0.5,
            "default strength is between off and full: {w_default:?}"
        );
    }

    /// Equal motion across all live bodies cancels under normalization: a
    /// lone still person (or an all-still, all-moving crowd) keeps exactly
    /// the legacy shares — the subdue only ever *redistributes*.
    #[test]
    fn emission_weights_subdue_cancels_when_motion_is_uniform() {
        let fades = [1.0, 0.5, 0.0, 0.0];
        let counts = [300, 300, 0, 0];
        let legacy = emission_slot_weights(fades, [false; 4], counts, [0.0; 4], 0.0);
        for uniform in [0.0_f32, 2.0] {
            let w = emission_slot_weights(fades, [false; 4], counts, [uniform; 4], 1.0);
            for (a, b) in w.iter().zip(legacy.iter()) {
                assert!(
                    (a - b).abs() < 1e-6,
                    "uniform motion {uniform} must cancel: {w:?} vs {legacy:?}"
                );
            }
        }
    }

    /// The CDF is the running sum; all-zero weights stay all-zero (the
    /// kernel's "no live slot" sentinel).
    #[test]
    #[allow(clippy::float_cmp, reason = "exact zero sentinel comparison")]
    fn weights_fold_to_monotone_cdf() {
        let cdf = weights_to_cdf([0.25, 0.25, 0.0, 0.5]);
        assert!((cdf[0] - 0.25).abs() < 1e-6);
        assert!((cdf[1] - 0.5).abs() < 1e-6);
        assert!((cdf[2] - 0.5).abs() < 1e-6);
        assert!((cdf[3] - 1.0).abs() < 1e-6);
        assert_eq!(weights_to_cdf([0.0; 4]), [0.0; 4]);
    }

    /// The baker writes the per-slot ranges and a fade-weighted CDF, clamped
    /// into the uploaded edge prefix.
    #[test]
    #[allow(clippy::float_cmp, reason = "fades pass through the baker unmodified")]
    fn bake_packs_slot_ranges_and_cdf() {
        let settings = RadianceSettings::default();
        let mut state = RadianceState {
            // Pre-seed the previous fades so neither body reads as *rising*
            // (igniting) — this test checks the steady-state shares; the
            // ignite boost has its own test in `emission_weights_apportion_by_fade`.
            slot_fade_prev: [1.0, 0.5, 0.0, 0.0],
            ..RadianceState::default()
        };
        let mut out = RadianceSimParamsGpu::default();
        let mut bodies = BodyTrackingState::default();
        bodies.bodies[0] = Some(TrackedBody {
            slot: 0,
            present: true,
            fade: 1.0,
            ..TrackedBody::default()
        });
        bodies.bodies[1] = Some(TrackedBody {
            slot: 1,
            present: true,
            fade: 0.5,
            ..TrackedBody::default()
        });
        bake_radiance_sim(
            &settings,
            &neutral_audio(),
            Some(&bodies),
            [200, 300, 0, 0],
            1.0,
            120_000,
            Vec2::new(1920.0, 1080.0),
            1.0 / 60.0,
            0.0,
            &mut state,
            &mut out,
        );
        assert_eq!(out.slot_start, [0, 200, 500, 500]);
        assert_eq!(out.slot_count, [200, 300, 0, 0]);
        assert_eq!(out.edge_count, 500);
        // Fades 1.0 / 0.5 → shares 2/3, 1/3 → CDF [2/3, 1, 1, 1].
        assert!(
            (out.slot_cdf[0] - 2.0 / 3.0).abs() < 1e-5,
            "{:?}",
            out.slot_cdf
        );
        assert!((out.slot_cdf[3] - 1.0).abs() < 1e-5);
        assert_eq!(state.slot_fade_prev, [1.0, 0.5, 0.0, 0.0]);
    }

    /// The baker copies the motion-emission bias setting into the uniform,
    /// clamped to `0..=1` (the kernel treats it as a probability floor).
    #[test]
    fn bake_copies_edge_motion_bias_clamped() {
        let settings = RadianceSettings {
            edge_motion_bias: 0.8,
            ..RadianceSettings::default()
        };
        let (_, out) = bake(&settings, &neutral_audio(), None, 500);
        assert!((out.edge_motion_bias - 0.8).abs() < 1e-6);

        let wild = RadianceSettings {
            edge_motion_bias: 7.0,
            ..RadianceSettings::default()
        };
        let (_, out) = bake(&wild, &neutral_audio(), None, 500);
        assert!((out.edge_motion_bias - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn expected_alive_rises_under_emission_and_decays_without() {
        let mut est = 0.0;
        for _ in 0..600 {
            est = expected_alive_step(est, 0.05, 10_000.0, 1.0 / 60.0, 1.5);
        }
        assert!(est > 1_000.0, "sustained emission fills the field: {est}");
        let peak = est;
        for _ in 0..600 {
            est = expected_alive_step(est, 0.0, 10_000.0, 1.0 / 60.0, 1.5);
        }
        assert!(est < peak * 0.05, "no emission decays toward empty: {est}");
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "the empty-field boost clamps to the cap exactly"
    )]
    fn burst_boost_is_one_when_dense_and_capped_when_empty() {
        assert!((burst_boost(9_000.0, 10_000.0, 4.0) - 1.0).abs() < 0.35);
        assert_eq!(burst_boost(0.0, 10_000.0, 4.0), 4.0);
        let mid = burst_boost(2_500.0, 10_000.0, 4.0);
        assert!(mid > 1.0 && mid < 4.0);
    }

    /// A beat pumps emission + buoyancy over the identical no-beat frame
    /// (the "flame swells on the beat" lane).
    #[test]
    fn bake_beat_swells_emission_and_buoyancy() {
        // Non-zero buoyancy so the swell lane is observable (calibration
        // default is 0 — see `bake_bass_raises_emission_prob`).
        let settings = RadianceSettings {
            buoyancy: 135.0,
            ..RadianceSettings::default()
        };
        let base = fixture_audio([0.2; 8], 0.2, 0.0);
        let mut on_beat = base;
        on_beat.beat_confidence = 1.0;
        let (_, quiet) = bake(&settings, &base, None, 500);
        let (_, thump) = bake(&settings, &on_beat, None, 500);
        assert!(thump.emission_prob > quiet.emission_prob * 1.3);
        assert!(thump.buoyancy > quiet.buoyancy * 1.3);
    }

    /// Onsets raise the ejecta fraction; silence keeps the stray-spark floor;
    /// `ejecta_amount = 0` disables the lane entirely.
    #[test]
    fn bake_onset_drives_ejecta() {
        let settings = RadianceSettings::default();
        let (_, calm) = bake(&settings, &neutral_audio(), None, 500);
        let expect_floor = settings.ejecta_amount * EJECTA_BASE_FRACTION;
        assert!((calm.ejecta_prob - expect_floor).abs() < 1e-6);
        let hit = fixture_audio([0.3; 8], 0.3, 2.0);
        let (_, driven) = bake(&settings, &hit, None, 500);
        assert!(
            driven.ejecta_prob > calm.ejecta_prob * 4.0,
            "{}",
            driven.ejecta_prob
        );
        assert!(driven.ejecta_speed > 0.0);
        let mut off = settings.clone();
        off.ejecta_amount = 0.0;
        let (_, none) = bake(&off, &hit, None, 500);
        assert!(none.ejecta_prob.abs() < f32::EPSILON);
    }

    /// Tongue amplitude follows the setting and breathes with bass; zero
    /// setting pins it off (uniform buoyancy).
    #[test]
    fn bake_bakes_tongue_noise() {
        // Explicit tongue strength: the calibration default is 0 (tongues
        // off in the water look), so the "on" branch sets its own.
        let settings = RadianceSettings {
            tongue_strength: 0.65,
            ..RadianceSettings::default()
        };
        let (_, out) = bake(&settings, &neutral_audio(), None, 500);
        assert!(out.tongue_amp > 0.0 && (out.tongue_freq - TONGUE_FREQ).abs() < f32::EPSILON);
        let mut flat = settings.clone();
        flat.tongue_strength = 0.0;
        let (_, out) = bake(&flat, &neutral_audio(), None, 500);
        assert!(out.tongue_amp.abs() < f32::EPSILON);
    }

    /// The per-bake frame counter advances every call (it salts the kernel's
    /// respawn hash), even when `elapsed` is pinned — the exact case the old
    /// `u32(time * 60.0)` salt aliased on.
    #[test]
    fn frame_counter_increments_each_bake_even_with_pinned_time() {
        let settings = RadianceSettings::default();
        let mut state = RadianceState::default();
        let mut out = RadianceSimParamsGpu::default();
        assert_eq!(out.frame, 0, "zeroed default");
        for expected in 1..=5 {
            bake_radiance_sim(
                &settings,
                &neutral_audio(),
                None,
                [100, 0, 0, 0],
                1.0,
                120_000,
                Vec2::new(1920.0, 1080.0),
                1.0 / 60.0,
                7.0, // pinned elapsed: the old time-based salt would not advance
                &mut state,
                &mut out,
            );
            assert_eq!(out.frame, expected, "frame must advance per bake");
        }
    }

    /// The pause decision can never fire while a particle could still be
    /// alive: the bound derives from the kernel's real [`LIFESPAN_MAX`]
    /// (single source of truth), the clock only advances while emission is
    /// zero, and any nonzero emission resets + resumes instantly.
    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "the settled clock is clamped to the bound, so the fixed \
                  point is bitwise-exact"
    )]
    fn pause_decision_waits_out_the_full_lifespan() {
        assert!(
            (PAUSE_BOUND_S - (LIFESPAN_MAX + PAUSE_MARGIN_S)).abs() < f32::EPSILON,
            "the bound must be derived from LIFESPAN_MAX, not re-hardcoded"
        );
        let dt = 1.0 / 60.0;
        let mut frozen = 0.0_f32;
        let mut paused = false;
        // Simulate LIFESPAN_MAX of frozen time: a particle born at the
        // freeze instant may just now be dying, so the pause must not fire.
        let mut simulated = 0.0_f32;
        while simulated < LIFESPAN_MAX {
            (frozen, paused) = step_radiance_pause(frozen, paused, 0.0, dt);
            simulated += dt;
            assert!(
                !paused,
                "must never pause inside LIFESPAN_MAX ({simulated} s)"
            );
        }
        // Push through the margin: now it pauses, and the clock clamps at
        // the bound (a settled pause stops changing state).
        while simulated < PAUSE_BOUND_S + 0.2 {
            (frozen, paused) = step_radiance_pause(frozen, paused, 0.0, dt);
            simulated += dt;
        }
        assert!(paused, "all-dead field must pause past the bound");
        assert!(
            frozen <= PAUSE_BOUND_S,
            "clock clamps at the bound: {frozen}"
        );
        let settled = step_radiance_pause(frozen, paused, 0.0, dt);
        assert_eq!(settled, (frozen, true), "settled pause is a fixed point");
        // Emission returning (Active bake or the screensaver ember) resets
        // and resumes in one step.
        let (frozen, paused) = step_radiance_pause(frozen, paused, 0.01, dt);
        assert!(!paused && frozen.abs() < f32::EPSILON, "emission resumes");
    }

    /// The pause system hides the billboard once the frozen clock clears the
    /// bound and re-shows it as soon as emission returns.
    #[test]
    fn pause_system_parks_and_resumes_billboard() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let params = RadianceSimParamsGpu {
            dt: 0.05,
            emission_prob: 0.0, // frozen (the Idle hook's write)
            ..RadianceSimParamsGpu::default()
        };
        world.insert_resource(RadianceSimParams {
            params,
            particles: Handle::default(),
            particle_count: 1000,
            paused: false,
            frozen_secs: 0.0,
        });
        let billboard = world
            .spawn((
                crate::radiance::systems::spawn::RadianceRoot,
                bevy::sprite_render::MeshMaterial2d(Handle::<
                    crate::radiance::render::RadianceMaterial,
                >::default()),
                Visibility::default(),
            ))
            .id();

        // 40 dispatches × 0.05 s = 2.0 simulated seconds: under
        // LIFESPAN_MAX, so particles may be alive and nothing hides.
        for _ in 0..40 {
            world
                .run_system_once(update_radiance_pause)
                .expect("pause step");
        }
        assert!(!world.resource::<RadianceSimParams>().paused);
        assert_eq!(
            *world.entity(billboard).get::<Visibility>().expect("vis"),
            Visibility::default(),
            "billboard untouched while particles may live"
        );

        // 20 more (3.0 s total > PAUSE_BOUND_S): paused + hidden.
        for _ in 0..20 {
            world
                .run_system_once(update_radiance_pause)
                .expect("pause step");
        }
        assert!(world.resource::<RadianceSimParams>().paused);
        assert_eq!(
            *world.entity(billboard).get::<Visibility>().expect("vis"),
            Visibility::Hidden,
            "all-dead field parks the billboard draw"
        );

        // A writer bakes emission again (screensaver ember / Active): the
        // very next step resumes and re-shows.
        world
            .resource_mut::<RadianceSimParams>()
            .params
            .emission_prob = 0.01;
        world
            .run_system_once(update_radiance_pause)
            .expect("pause step");
        let sim = world.resource::<RadianceSimParams>();
        assert!(!sim.paused && sim.frozen_secs.abs() < f32::EPSILON);
        assert_eq!(
            *world.entity(billboard).get::<Visibility>().expect("vis"),
            Visibility::Visible,
            "emission returning re-shows the billboard"
        );
    }

    /// The freeze hook zeroes emission and burst, nothing else.
    #[test]
    fn freeze_zeroes_emission() {
        let mut world = World::new();
        let settings = RadianceSettings::default();
        let (_, params) = bake(&settings, &neutral_audio(), None, 500);
        world.insert_resource(RadianceSimParams {
            params,
            particles: Handle::default(),
            particle_count: 1000,
            paused: false,
            frozen_secs: 0.0,
        });
        bevy::ecs::system::RunSystemOnce::run_system_once(&mut world, freeze_radiance_emission)
            .expect("freeze runs");
        let sim = world.resource::<RadianceSimParams>();
        assert!(sim.params.emission_prob.abs() < f32::EPSILON);
        assert!(sim.params.burst_speed.abs() < f32::EPSILON);
        assert!(sim.params.ejecta_prob.abs() < f32::EPSILON);
        assert!(
            sim.params.flow_strength > 0.0,
            "flow untouched (fade-out drifts)"
        );
    }
}
