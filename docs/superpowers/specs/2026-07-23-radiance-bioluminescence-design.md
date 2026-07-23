# Radiance Bioluminescence Rework — Design

**Date:** 2026-07-23
**Status:** Approved in conversation (Madison, 2026-07-23); spec pending review
**Prior art:** `crates/wc-sketches/src/radiance/` (`pulse.rs`, `distance_field.rs`,
`compute/`, `systems/sim_params.rs`, `settings.rs`), `assets/shaders/radiance/`
(`pulse.wgsl`, `simulate.wgsl`), the Dots GPU-saturation runbook
(`docs/runbooks/dots-explode-gpu-saturation.md`).

## Direction

Madison's art notes (2026-07-23): the beat-pulse distance-field contours read
as cheesy; particles should overlap the tracked silhouette less; particles
should be a little more activated by motion while preserving baseline
emission and sound-responsiveness. Visual reference: **bioluminescent
algae** — light is always *caused*; an object moving through the water is
illuminated by the disturbance.

The design principle that falls out: **no non-diegetic light.** Every photon
belongs to something in the particle world. The fullscreen beat-contour
overlay is screen-space paint and goes; its energy moves into the medium.
**Strong beat legibility is a hard requirement** and is preserved
in-particle (see the density-adaptive burst).

Scope guards (decided): behavior only — **no palette work**, per-body
identity hues and every existing audio lane untouched; motion activation is
"a little more", not a rework.

## What is removed, what is kept

Removed:
- The fullscreen additive pulse quad, `RadiancePulseMaterial`, and
  `pulse.wgsl`'s render pass (entity-owned; despawned via the existing
  OnExit teardown — it is deliberately absent from the render-world
  `remove_*_if_absent` family, and removal must not leave a new member for
  it either).
- The `pulse_uniform_dead` hide-quad optimization (nothing left to hide).

Kept and repurposed:
- The chamfer **distance field** (`distance_field.rs`): promoted from
  fragment-shader input to a **particle-sim input**, bound into the compute
  pass. Its extraction/upload must gate on the sim being live, preserving
  the zero-systems/zero-uploads discipline while Idle-parked.
- The **beat-wave clock** in `pulse.rs` (rising-edge spawn on the debounced
  beat lane into a fixed ring of aging wave slots): survives as CPU-side
  state baked into the sim uniforms (per-wave radius + strength, fixed
  `MAX_PULSES`-sized arrays, no allocation) instead of a material uniform.

## New particle behaviors (all in `simulate.wgsl`)

1. **Soft silhouette repel + contact glow.** Per particle, sample the
   distance field `d` and its gradient (finite difference, smoothed): for
   `d` under a repel radius, apply a clamped outward force along the
   gradient — particles flow around the body like water; interior overlap
   corrects smoothly, never a hard reflection. The same falloff adds a
   **contact glow** so particles brighten as they kiss the boundary and
   slide off (the algae "illuminates what it touches" beat).
2. **Density-adaptive beat burst (the legibility floor).** On each beat
   rising edge, bake a burst of extra edge emission with an outward
   velocity kick, scaled by the bass-weighted beat strength **and** by an
   inverse-density boost: when the ambient alive-particle estimate is low,
   the burst grows (up to a cap), so every beat marks visibly even from
   near-empty water. Density comes from the **existing CPU-side
   deterministic alive estimate** (the same bookkeeping that parks the
   compute dispatch when every particle is dead) — no GPU readback, ever.
   The burst gets its **own Dev-gated scale constant**, deliberately not
   piggybacked on `ejecta_amount`: ejecta rides audio onsets, the burst
   rides the debounced beat lane, and sharing a knob muddies live tuning
   attribution (promotion/merging is a post-tuning decision).
3. **Flare-wave through the medium.** Each live beat wave carries an
   expanding radius; a particle briefly flares as the radius passes its own
   distance-from-silhouette (`|d − r| <` band, short temporal envelope).
   The wavefront is visible only where particles exist — dense plumes light
   in body-shaped sequence, empty water stays dark. **V1 ships
   envelope-only** and accepts occasional re-flare shimmer from mask/
   distance jitter; the documented escalation, only if shimmer is
   objectionable in review, is one `u32` of per-particle already-flared
   wave bits (a particle-buffer layout change — not paid up front).
4. **Motion disturbance (staged last, smallest).** Fast-moving edge regions
   inject a modest local flare + push into nearby particles, layered on the
   existing `edge_motion_bias` emission weighting. This is the least-pinned
   lane and the design explicitly permits the fallback of shipping it as
   **emission-side only** (deepening `edge_motion_bias`) if the
   per-particle form threatens the frame budget — plumbing options
   (bounded edge-point loop vs. a low-res motion texture) are a plan-time
   choice under that budget.

## Tuning surface

All new gains ship as **Dev-category settings** first (repel strength,
repel radius, contact glow, flare gain, flare band, burst scale, burst
boost cap, motion disturbance gain), promoted to operator knobs only after
the live tuning session shows which ones Madison actually reaches for.
(Dev knobs sit behind the per-launch ADVANCED toggle.)

## Risks and mitigations (from the 2026-07-23 review)

1. **Repel jitter on real masks** — gradient noise from low-res, wobbly
   segmentation edges. Mitigate: gradient smoothing + force clamping as
   first-class constants, and a **replay-harness look pass on the real
   eval clips** (`tests/eval-media/body/`) — the synthetic-dancer capture
   path has a structurally clean mask and cannot exhibit this failure.
2. **Mask latency vs. fast limbs** — the field lags, then over-corrects.
   Mitigate: capped corrective velocity; tune during the replay pass on the
   fast-motion clips (light whip, fans).
3. **Flare re-triggering** — handled by the staged escalation in behavior 3.
4. **Beat legibility** — hard requirement, carried by the density-adaptive
   burst (behavior 2).
5. **Knob attribution** — separate burst constant (behavior 2).
6. **GPU cost** — new per-particle samples and the wave loop land in the
   hot kernel on the machine with the documented Dots saturation history.
   Mitigate: the removed fullscreen additive pass refunds cost, but the
   plan includes a **before/after GPU check** (xctrace perf-state, the
   Dots-diagnosis method), and the kernel additions stay branch-light.
7. **Motion-lane scope creep** — bounded by the fallback in behavior 4.
8. **Lifecycle edges** — distance-field extraction re-pointed at the sim
   must stay silent while Idle-parked; pulse removal leaves no orphaned
   render-world state (checked against the `remove_*_if_absent` family).

## Verification

- **Deterministic captures** (`WC_DEBUG_FORCE_RADIANCE_SYNTHETIC_BODY` +
  `cargo xtask capture`): before/after stills proving interior clearing and
  edge glow; regression baselines for the new look.
- **Replay pass on real footage** (`WAVECONDUCTOR_BODY_REPLAY` with the
  eval clips): the jitter/latency risks are only observable here.
- **GPU before/after** via xctrace on the same scene.
- **Live tuning session**: Madison dances at it; art acceptance is hers.

## Out of scope

- Palette or color-curve changes (explicitly declined 2026-07-23).
- Rim/body-material beat involvement (declined in favor of the
  density-adaptive burst).
- Any change to audio analysis lanes, baseline emission rates, sparkle
  motes, or the screensaver's synthetic performer path beyond compiling
  against the revised sim params.
