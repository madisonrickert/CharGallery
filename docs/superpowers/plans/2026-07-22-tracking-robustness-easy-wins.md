# Tracking Robustness: Easy Wins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Radiance's body tracking degrade gracefully around held props, occlusion, exposure extremes, and fast limbs — ordered easiest/lowest-risk first, ending with the motion-biased edge emission that makes a swept fan shed particles.

**Architecture:** Ten small, independent increments in three zones: (1) consumer-layer hardening in the Radiance sketch (velocity clamp, Schmitt visibility latches, elbow fallback, sparkle hysteresis); (2) input-pipeline improvements in wc-core (capture-resolution un-pin, OBSBOT manual exposure, dead-reckoned slot re-association); (3) a per-edge motion signal threaded from the mask worker to the emission kernel. No new dependencies anywhere.

**Tech Stack:** Rust / Bevy 0.19, egui settings panels via the `SketchSettings` derive, WGSL compute, the vendored OBSBOT libdev SDK (C++ shim, Windows-only IO).

## Global Constraints

- Gates before claiming any task done (AGENTS.md): `cargo fmt --all -- --check`; `cargo clippy --all-targets --all-features --workspace -- -D warnings`; `cargo nextest run --workspace --all-features`; `cargo xtask check-secrets`. Run `cargo doc --no-deps --workspace --document-private-items` when a task adds rustdoc.
- **No allocation in hot paths** — per-frame Bevy systems, the body worker loop, egui paint hooks. Pre-allocate at init, refill with `clear()`. Applies hard to Tasks 9–10 (worker-thread code).
- No `unwrap()`/`expect()` in non-test code; no `as` numeric casts where `From`/`TryFrom` works.
- **No new dependencies** (long build times; reuse the graph).
- Doc gate builds default features only: never intra-doc-link (`[Foo]`) from non-feature-gated docs to feature-gated items (e.g. anything in `obsbot/`) — use plain code spans there.
- Serde house pattern: every new settings field gets `#[serde(default = "default_<name>")]` + a serde-default fn + entries in the struct's two defaults-match tests.
- Commit per task, message style `feat(scope):`/`fix(scope):` matching recent history. **Backticks in `git commit -m` get shell-substituted — always commit with `-F <file>`.**
- Manual smoke tests use `cargo rund` (never the bare target binary). No concurrent cargo builds if tasks are parallelized (they should not be — several tasks touch the same files; execute sequentially).
- All file paths below are repo-relative from the workspace root.

---

### Task 1: Clamp impulse velocity against landmark teleports

Motion blur can snap a landmark to a wrong position for one frame; the resulting spike velocity is written to the GPU **unclamped** (gain saturates, the vector does not), producing a visible particle blast.

**Files:**
- Modify: `crates/wc-sketches/src/radiance/systems/sim_params.rs` (constants near line 99, `bake_impulses` near line 524)

**Interfaces:**
- Produces: `pub const IMPULSE_MAX_SPEED: f32 = 1350.0;` (used again by Task 3's rewrite)

- [ ] **Step 1: Write the failing test** in the existing `#[cfg(test)] mod tests` of `sim_params.rs`, next to `bake_bakes_wrist_impulse_with_mirror_mapping` and using its `fixture_body`/`tracking_state`/`bake` helpers:

```rust
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
        assert!((out.impulses[0].gain - 1.0).abs() < 1e-6, "gain still saturates");
    }
```

- [ ] **Step 2: Run it to verify it fails.** `cargo nextest run -p wc-sketches bake_clamps_teleport_impulse_velocity` — expected: compile error (`IMPULSE_MAX_SPEED` undefined).

- [ ] **Step 3: Implement.** Next to `IMPULSE_FULL_SPEED` (`sim_params.rs:96-99`) add:

```rust
/// Hard cap on an impulse's velocity magnitude, world px/s. Gain already
/// saturates at [`IMPULSE_FULL_SPEED`], but the velocity *vector* is passed
/// to the kernel unscaled — a one-frame landmark teleport (motion blur, a
/// mis-detection) would otherwise blast particles across the field. 1.5×
/// full speed keeps every legitimate sweep untouched.
pub const IMPULSE_MAX_SPEED: f32 = 1.5 * IMPULSE_FULL_SPEED;
```

In `bake_impulses`, change the `vel` binding to clamp:

```rust
                let vel = mask_dir_to_world(
                    Vec2::new(body.velocities[lm].x, body.velocities[lm].y),
                    scale,
                    mirror,
                )
                .clamp_length_max(IMPULSE_MAX_SPEED);
```

- [ ] **Step 4: Verify.** `cargo nextest run -p wc-sketches sim_params` — all pass. Then the fmt/clippy gates.

- [ ] **Step 5: Commit** (`-F` file): `fix(radiance): clamp impulse velocity against landmark teleports`

---

### Task 2: Elbow landmark indices + shared Schmitt visibility latch

Foundation for Tasks 3–4. Elbow data already flows through the 33-landmark arrays; only the named constants are missing (`landmark_index` has NOSE/wrists/hips/ankles only).

**Files:**
- Modify: `crates/wc-core/src/input/body/mod.rs` (the `landmark_index` module, lines 110-125)
- Create: `crates/wc-sketches/src/radiance/visibility.rs`
- Modify: `crates/wc-sketches/src/radiance/mod.rs` (add `pub mod visibility;`)

**Interfaces:**
- Produces: `landmark_index::{LEFT_ELBOW, RIGHT_ELBOW}` (13/14); `radiance::visibility::{VIS_GATE_OPEN, VIS_GATE_CLOSE, VisibilityLatch}` with `fn step(&mut self, visibility: f32) -> bool`, `fn is_open(&self) -> bool`, `fn reset(&mut self)`. `VisibilityLatch` derives `Clone, Copy, Debug, Default` (Tasks 3–4 store it in `Copy` state structs).

- [ ] **Step 1:** In `wc-core`'s `landmark_index` module add, keeping the existing doc style:

```rust
    /// Left elbow.
    pub const LEFT_ELBOW: usize = 13;
    /// Right elbow.
    pub const RIGHT_ELBOW: usize = 14;
```

(Standard BlazePose topology indices; the arrays are already 33 wide.)

- [ ] **Step 2: Write the new module with tests.** Create `crates/wc-sketches/src/radiance/visibility.rs`:

```rust
//! Schmitt-trigger visibility gating shared by Radiance's landmark consumers
//! (limb impulses, extremity sparkles).
//!
//! A landmark hovering at the model's ~0.5 visibility boundary (a wrist
//! holding a fan, a hip behind a prop) chatters across a single-threshold
//! gate, strobing whatever layer it feeds. The latch opens only at
//! [`VIS_GATE_OPEN`] but stays open down to [`VIS_GATE_CLOSE`], so marginal
//! visibility holds the last decision instead of flickering.

/// Visibility above which a closed latch opens (the strict admission bar —
/// matches the pipeline's detector/presence thresholds).
pub const VIS_GATE_OPEN: f32 = 0.5;
/// Visibility below which an open latch closes (the lenient hold bar).
pub const VIS_GATE_CLOSE: f32 = 0.35;

/// One landmark's Schmitt visibility gate.
#[derive(Clone, Copy, Debug, Default)]
pub struct VisibilityLatch {
    open: bool,
}

impl VisibilityLatch {
    /// Advance the latch with this frame's visibility; returns whether the
    /// landmark passes the gate.
    pub fn step(&mut self, visibility: f32) -> bool {
        self.open = if self.open {
            visibility >= VIS_GATE_CLOSE
        } else {
            visibility >= VIS_GATE_OPEN
        };
        self.open
    }

    /// Whether the latch is currently open (last `step` decision).
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Close the latch (slot emptied / body absent).
    pub fn reset(&mut self) {
        self.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate opens at the strict bar, holds through the marginal band,
    /// and closes below the lenient bar — no chatter inside the band.
    #[test]
    fn latch_is_hysteretic() {
        let mut latch = VisibilityLatch::default();
        assert!(!latch.step(0.45), "below open bar stays closed");
        assert!(latch.step(0.55), "opens at the strict bar");
        assert!(latch.step(0.40), "marginal band holds open");
        assert!(!latch.step(0.30), "closes below the lenient bar");
        assert!(!latch.step(0.40), "marginal band does NOT reopen");
        assert!(latch.step(0.60), "reopens only at the strict bar");
    }

    #[test]
    fn reset_closes_the_latch() {
        let mut latch = VisibilityLatch::default();
        latch.step(0.9);
        latch.reset();
        assert!(!latch.is_open());
        assert!(!latch.step(0.40), "post-reset requires the strict bar again");
    }
}
```

Register in `crates/wc-sketches/src/radiance/mod.rs` alongside the existing `pub mod` lines: `pub mod visibility;`.

- [ ] **Step 3: Verify.** `cargo nextest run -p wc-sketches visibility` — 2 pass. `cargo nextest run -p wc-core landmark` — no regressions. fmt/clippy gates.

- [ ] **Step 4: Commit:** `feat(radiance): Schmitt visibility latch + elbow landmark indices`

---

### Task 3: Hysteretic impulse gate with elbow fallback

Rewrites `bake_impulses`'s per-landmark loop: the binary `visibility < 0.5` gate becomes a latched gate, and each arm falls back to its elbow when the wrist is the less-visible joint — so a prop-holding arm keeps shedding particles instead of going silent.

**Files:**
- Modify: `crates/wc-sketches/src/radiance/systems/sim_params.rs` (`IMPULSE_LANDMARKS` → `IMPULSE_SOURCES`, `RadianceState`, `bake_impulses`, its caller in `bake_radiance_sim`, tests)

**Interfaces:**
- Consumes: `VisibilityLatch` and elbow constants from Task 2; `IMPULSE_MAX_SPEED` from Task 1.
- Produces: `pub const IMPULSE_SOURCE_COUNT: usize = 7;`, `pub const IMPULSE_SOURCES: [(usize, Option<usize>); IMPULSE_SOURCE_COUNT]`, `RadianceState.impulse_latch: [[VisibilityLatch; IMPULSE_SOURCE_COUNT]; MAX_TRACKED_BODIES]`, and the new signature `fn bake_impulses(bodies, mirror, latches: &mut [[VisibilityLatch; IMPULSE_SOURCE_COUNT]; MAX_TRACKED_BODIES], out)`.

- [ ] **Step 1: Write the failing tests** (same test module, same helpers; `fixture_body` sets wrist velocity — extend it or add a variant that also sets per-landmark visibilities as needed):

```rust
    /// A wrist that dips into the marginal visibility band (held fan) keeps
    /// its impulse: the latch holds through 0.35..0.5 once opened.
    #[test]
    fn marginal_wrist_visibility_holds_the_impulse() {
        let mut body = fixture_body(Vec3::new(0.8, 0.0, 0.0));
        let mut latches = [[VisibilityLatch::default(); IMPULSE_SOURCE_COUNT]; MAX_TRACKED_BODIES];
        // Frame 1: clearly visible — opens the latch.
        let state = tracking_state(body.clone());
        let mut out = RadianceSimParamsGpu::default();
        bake_impulses(Some(&state), false, &mut latches, &mut out);
        assert_eq!(out.impulse_count, 1);
        // Frame 2: marginal (0.42) — a plain 0.5 gate would drop it.
        body.landmarks[RIGHT_WRIST].visibility = 0.42;
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
        let mut out = RadianceSimParamsGpu::default();
        bake_impulses(Some(&state), false, &mut latches, &mut out);
        assert_eq!(out.impulse_count, 1, "elbow carries the arm's impulse");
    }
```

Adjust the fixture-mutation lines to the actual `fixture_body` shape (it returns a `TrackedBody`; if it returns something wrapped, mutate before wrapping in `tracking_state`). If existing tests call `bake(...)` (which goes through `bake_radiance_sim`), these two may instead call `bake_impulses` directly as shown — that is the point of the new injectable-latches signature.

- [ ] **Step 2: Run to verify failure** (compile errors: `IMPULSE_SOURCE_COUNT`, new signature).

- [ ] **Step 3: Implement.** Replace `IMPULSE_LANDMARKS` (`sim_params.rs:29-42`) with:

```rust
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
```

Update the `landmark_index` import to include `LEFT_ELBOW, RIGHT_ELBOW`. Add to `RadianceState` (after `slot_fade_prev`):

```rust
    /// Per-slot, per-impulse-source Schmitt visibility latches (see
    /// `crate::radiance::visibility`): marginal landmark visibility holds
    /// its last gate decision instead of strobing the impulse layer.
    pub impulse_latch: [[VisibilityLatch; IMPULSE_SOURCE_COUNT]; MAX_TRACKED_BODIES],
```

Rewrite `bake_impulses`:

```rust
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
                        if body.landmarks[fb].visibility
                            > body.landmarks[primary].visibility =>
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
```

Keep the existing `#[allow(...)]` attribute and doc comment (update the doc's landmark list to mention the elbow fallback). Update the caller in `bake_radiance_sim` (around line 495) to pass `&mut state.impulse_latch` — `bake_radiance_sim` already has `&mut RadianceState`. Grep for other `IMPULSE_LANDMARKS` references (tests, docs) and update them.

- [ ] **Step 4: Verify.** `cargo nextest run -p wc-sketches sim_params` — all pass, including Task 1's clamp test and the pre-existing impulse tests (they open latches on their first visible frame, so single-`bake` tests still see their impulse). fmt/clippy gates.

- [ ] **Step 5: Commit:** `feat(radiance): hysteretic impulse gate with elbow fallback`

---

### Task 4: Hysteretic sparkle eligibility

Same chatter problem in the sparkle layer: `candidate_eligible` hard-gates at `visibility < VISIBILITY_GATE` per frame. Motes already cross-fade on reassignment (the driver's per-mote envelope), so the only change needed is latching the eligibility decision.

**Files:**
- Modify: `crates/wc-sketches/src/radiance/sparkle/tracker.rs`
- Modify: `crates/wc-sketches/src/radiance/sparkle/mod.rs` (driver call sites — grep `candidate_eligible(` and `step_scores(`)

**Interfaces:**
- Consumes: `VisibilityLatch`, `VIS_GATE_OPEN` from Task 2.
- Produces: `LimbOscillator.step_visibility(&mut self, body: &TrackedBody)` (call once per frame, before `select`); `candidate_eligible` becomes a method `LimbOscillator::candidate_eligible(&self, body, candidate, com) -> bool`. The free-function form is removed. `VISIBILITY_GATE` is removed; `body_com_uv` uses `VIS_GATE_OPEN`.

- [ ] **Step 1: Write the failing test** in `tracker.rs`'s test module (reuse its `fixture_body` helper; set candidate landmark visibilities directly):

```rust
    /// Eligibility is latched: a wrist that opened at 0.55 stays eligible
    /// through the 0.35..0.5 marginal band and drops only below 0.35.
    #[test]
    fn marginal_visibility_holds_eligibility() {
        let mut osc = LimbOscillator::default();
        let mut body = fixture_body();
        let wrist = CANDIDATE_LANDMARKS[0];
        body.landmarks[wrist].visibility = 0.55;
        osc.step_visibility(&body);
        assert!(osc.candidate_eligible(&body, 0, None));
        body.landmarks[wrist].visibility = 0.42;
        osc.step_visibility(&body);
        assert!(osc.candidate_eligible(&body, 0, None), "band holds");
        body.landmarks[wrist].visibility = 0.30;
        osc.step_visibility(&body);
        assert!(!osc.candidate_eligible(&body, 0, None), "closes below 0.35");
        body.landmarks[wrist].visibility = 0.42;
        osc.step_visibility(&body);
        assert!(!osc.candidate_eligible(&body, 0, None), "band does not reopen");
    }
```

If `fixture_body()` initializes candidate visibilities at some default, set them explicitly as above so the test controls the sequence.

- [ ] **Step 2: Run to verify failure** (no `step_visibility` method; `candidate_eligible` arity).

- [ ] **Step 3: Implement.**
  - Delete `pub const VISIBILITY_GATE: f32 = 0.5;`; import `crate::radiance::visibility::{VisibilityLatch, VIS_GATE_OPEN}`.
  - `body_com_uv`: replace both `VISIBILITY_GATE` uses with `VIS_GATE_OPEN` (hip COM keeps the plain gate — it feeds a distance heuristic, not a strobing visual).
  - Add to `LimbOscillator`: `/// Per-candidate Schmitt visibility latches.` `vis: [VisibilityLatch; 4],` (derives still hold: `VisibilityLatch` is `Copy + Default`).
  - Add the method:

```rust
    /// Advance the per-candidate visibility latches (call once per frame,
    /// before [`Self::select`]): marginal visibility holds its last gate
    /// decision instead of strobing mote assignment.
    pub fn step_visibility(&mut self, body: &TrackedBody) {
        for (i, &landmark) in CANDIDATE_LANDMARKS.iter().enumerate() {
            self.vis[i].step(body.landmarks[landmark].visibility);
        }
    }
```

  - Convert the free `candidate_eligible` into a method (same doc comment, visibility check swapped for the latch):

```rust
    /// Whether a candidate may carry motes this frame: latched-visible, and
    /// far enough from the centre of mass.
    #[must_use]
    pub fn candidate_eligible(
        &self,
        body: &TrackedBody,
        candidate: usize,
        com: Option<Vec2>,
    ) -> bool {
        if !self.vis[candidate].is_open() {
            return false;
        }
        let landmark = body.landmarks[CANDIDATE_LANDMARKS[candidate]];
        com.is_none_or(|c| landmark.pos.truncate().distance(c) >= MIN_COM_DIST_UV)
    }
```

  - Update `select` to call `self.candidate_eligible(...)` (two sites). `reset()` already zeroes `vis` via `*self = Self::default()`.
  - In the driver (`sparkle/mod.rs`, `update_radiance_sparkles`): call `osc.step_visibility(body)` immediately before the existing `step_scores`/`select` calls, and update its `candidate_eligible(...)` call sites (partner eligibility) to the method form.
  - Update existing tracker tests that call the free function or rely on one-frame visibility: tests that set visibility ≥ 0.5 before selecting still pass once they call `step_visibility` first — add that call where the compiler/tests demand it (e.g. `occlusion_releases_priority` drops visibility to ~0, which closes the latch, preserving its assertion).

- [ ] **Step 4: Verify.** `cargo nextest run -p wc-sketches sparkle` — all pass. fmt/clippy gates.

- [ ] **Step 5: Commit:** `feat(radiance): hysteretic sparkle eligibility`

---

### Task 5: Capture resolution un-pin (nokhwa bias + AVFoundation preset)

Finishes the outdoor spec's A3. Two changes: (a) the nokhwa (Windows/kiosk) selector still biases toward 640×480 and prefers decode cost over resolution — re-target to 720p, make resolution outrank decode cost, and guard frame rate so a 720p\@10fps mode can never win over a 480p\@30fps one; (b) the macOS AVFoundation backend hard-pins `AVCaptureSessionPreset640x480`.

**Files:**
- Modify: `crates/wc-core/src/input/capture/nokhwa.rs` (constants 19-34, `choose_camera_format` 36-80, tests in `camera_format_tests`)
- Modify: `crates/wc-core/src/input/capture/avfoundation.rs` (line ~325)

**Interfaces:**
- Produces: no API change; policy change only. Downstream is resolution-agnostic (both pipelines square-pad via `ContentRect::for_frame`), verified by the explorer pass.

- [ ] **Step 1: Write the failing tests** in `camera_format_tests` (reuse the `fmt(w, h, format, fps)` helper):

```rust
    /// A full-rate smaller mode always beats a starved larger one: 720p@10
    /// cannot feed the 30 Hz inference cadence.
    #[test]
    fn full_rate_beats_larger_area() {
        let chosen = choose_camera_format(&[
            fmt(1280, 720, FrameFormat::YUYV, 10),
            fmt(640, 480, FrameFormat::MJPEG, 30),
        ])
        .unwrap();
        assert_eq!((chosen.width(), chosen.height()), (640, 480));
    }

    /// Resolution now outranks decode cost: the un-pin means more pixels on
    /// a distant body beats saving a JPEG decode.
    #[test]
    fn resolution_outranks_decode_cost() {
        let chosen = choose_camera_format(&[
            fmt(640, 480, FrameFormat::YUYV, 30),
            fmt(1280, 720, FrameFormat::MJPEG, 30),
        ])
        .unwrap();
        assert_eq!((chosen.width(), chosen.height()), (1280, 720));
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo nextest run -p wc-core --features hand-tracking-mediapipe-camera camera_format` — both fail (current policy picks the other format in each).

- [ ] **Step 3: Implement.** Replace `TARGET_AREA` and add a frame-rate floor:

```rust
/// Resolution area we bias selection toward (720p — the un-pin from the
/// original 640×480; see docs/superpowers/specs/2026-07-06-outdoor-tracking
/// A3: more pixels on a distant/dim body is worth the decode cost).
const TARGET_AREA: i64 = 1280 * 720;
/// Formats below this frame rate sort behind everything at or above it: the
/// body pipeline's 30 Hz inference cadence starves on a 10 fps feed no
/// matter how large the frame is.
const MIN_FULL_RATE_FPS: u32 = 25;
```

Change the sort key in `choose_camera_format` (and its policy doc comment to match):

```rust
        .min_by_key(|f| {
            let rank = decode_rank(f.format()).unwrap_or(u8::MAX);
            let area = i64::from(f.width()) * i64::from(f.height());
            let area_dist = (area - TARGET_AREA).abs();
            // Full-rate formats first, then nearest the target resolution
            // (resolution outranks decode cost — the un-pin), then cheapest
            // decode, then the highest frame rate.
            (
                f.frame_rate() < MIN_FULL_RATE_FPS,
                area_dist,
                rank,
                std::cmp::Reverse(f.frame_rate()),
            )
        })
```

Update existing tests whose fixtures assumed the old ordering: `picks_resolution_closest_to_target` (target is now 720p — adjust its fixture/assertion), and `prefers_uncompressed_over_mjpeg_at_same_resolution` (same resolution ties on `area_dist`, so rank still decides — should pass unchanged; verify). In `avfoundation.rs:325` replace `AVCaptureSessionPreset640x480` with `AVCaptureSessionPreset1280x720`.

- [ ] **Step 4: Verify.** `cargo nextest run -p wc-core --all-features capture` — all pass. fmt/clippy gates. **Manual smoke (macOS, this machine):** `cargo rund`, enter Radiance with the camera preview checkbox on — confirm the preview looks correct and the inference-readouts overlay (Dev setting; remind Madison to flip ADVANCED) still shows ~30 Hz body frames. A meaningful drop means the convert path can't keep up and the preset change should be reverted to a discussion.

- [ ] **Step 5: Commit:** `feat(input): un-pin capture toward 720p (nokhwa bias + avfoundation preset)`

---

### Task 6: OBSBOT exposure settings + coalesced send plan (cross-platform half)

The manual-exposure slider + auto-exposure checkbox (default auto), and the pure planning logic that decides when to send. Device IO lands in Task 7; after this task the UI exists and the planner is fully tested, with the send a no-op variant the stub swallows.

**Files:**
- Modify: `crates/wc-core/src/input/obsbot/mod.rs` (`ObsbotSettings` fields ~181-267, `WorkerCommand` enum ~116-145, settings tests ~529+)
- Modify: `crates/wc-core/src/input/obsbot/framing.rs` (`FramingValues`, `FramingPlan`, `plan_framing_send`, `apply_framing_settings`, tests)

**Interfaces:**
- Consumes: existing `ObsbotSettings` derive/persistence machinery, `plan_framing_send` coalescing.
- Produces: `ObsbotSettings.auto_exposure: bool` (default `true`), `ObsbotSettings.manual_shutter: i32` (SDK `DevShutterTimeType` index, `9..=45`, default `28` = 1/100); `WorkerCommand::SetExposure { shutter: i32, auto: bool }`; `FramingValues` gains `auto_exposure: bool` + `manual_shutter: i32`; `FramingPlan` gains `exposure: bool`; `pub(crate) fn shutter_label(index: i32) -> &'static str`.

- [ ] **Step 1: Write the failing tests.**
  - In `mod.rs` settings tests, extend the existing defaults test (or add beside `take_control_defaults_on`):

```rust
    /// Exposure defaults: auto ON, manual shutter 1/100 (index 28) — a
    /// pre-exposure settings file must load these without error.
    #[test]
    fn exposure_defaults_auto_on() {
        let s: ObsbotSettings = toml::from_str("").unwrap_or_default();
        assert!(s.auto_exposure);
        assert_eq!(s.manual_shutter, 28);
        assert_eq!(shutter_label(28), "1/100");
        assert_eq!(shutter_label(9), "1/8000");
        assert_eq!(shutter_label(45), "1/2");
        assert_eq!(shutter_label(999), "?", "out-of-ladder index is visible, not a panic");
    }
```

(Match the deserialization idiom of the existing `pre_framing_settings_file_loads_neutral_framing` test rather than `unwrap_or_default` if it differs.)
  - In `framing.rs` tests:

```rust
    /// An exposure change plans only the exposure lane; entering control
    /// re-asserts it like every other lane.
    #[test]
    fn exposure_changes_plan_the_exposure_lane() {
        let last = neutral();
        let mut manual = neutral();
        manual.auto_exposure = false;
        let plan = plan_framing_send(Some(last), manual, false, Duration::from_secs(1), Duration::ZERO);
        assert!(plan.exposure && !plan.gimbal && !plan.zoom && !plan.fov);

        // Baseline must share auto=false so only the shutter differs.
        let mut base = neutral();
        base.auto_exposure = false;
        let mut moved = base;
        moved.manual_shutter = 21; // 1/500
        let plan = plan_framing_send(Some(base), moved, false, Duration::from_secs(1), Duration::ZERO);
        assert!(plan.exposure && !plan.gimbal);
    }
```

- [ ] **Step 2: Run to verify failure** (missing fields/variants).

- [ ] **Step 3: Implement.**
  - `ObsbotSettings` — add after `fov`, following the exact attribute style of `zoom`/`take_control`:

```rust
    /// Auto exposure. Default on: the take-control sequence re-asserts AE,
    /// and auto is the right default for a kiosk. Turn off for performance
    /// lighting (an LED prop makes AE hunt and underexpose the dancer);
    /// `manual_shutter` then applies.
    #[setting(
        default = true,
        ty = Boolean,
        category = User,
        section = "Camera",
        label = "Auto exposure"
    )]
    #[serde(default = "default_auto_exposure")]
    pub auto_exposure: bool,

    /// Manual shutter time as the SDK's `DevShutterTimeType` index
    /// (9 = 1/8000 … 45 = 1/2, contiguous ladder; see
    /// `vendor/libdev/include/dev/dev.hpp`). Applied only when
    /// `auto_exposure` is off. Left = faster/darker (crisper fast limbs),
    /// right = slower/brighter. The status section shows the fraction.
    #[setting(
        default = 28_i32,
        min = 9_i32,
        max = 45_i32,
        step = 1_i32,
        category = User,
        section = "Camera",
        label = "Manual shutter"
    )]
    #[serde(default = "default_manual_shutter")]
    pub manual_shutter: i32,
```

  serde fns `default_auto_exposure() -> bool { true }`, `default_manual_shutter() -> i32 { 28 }` next to the existing ones. Add the label table (module scope, `pub(crate)`):

```rust
/// Human label for a `DevShutterTimeType` index (the ladder in
/// `vendor/libdev/include/dev/dev.hpp`: contiguous 9..=45).
pub(crate) fn shutter_label(index: i32) -> &'static str {
    const LABELS: [&str; 37] = [
        "1/8000", "1/6400", "1/5000", "1/4000", "1/3200", "1/2500", "1/2000",
        "1/1600", "1/1250", "1/1000", "1/800", "1/640", "1/500", "1/400",
        "1/320", "1/240", "1/200", "1/160", "1/120", "1/100", "1/80", "1/60",
        "1/50", "1/40", "1/30", "1/25", "1/20", "1/15", "1/12.5", "1/10",
        "1/8", "1/6.25", "1/5", "1/4", "1/3", "1/2.5", "1/2",
    ];
    usize::try_from(index - 9)
        .ok()
        .and_then(|i| LABELS.get(i).copied())
        .unwrap_or("?")
}
```

  - `WorkerCommand` — add variant with the house doc style: `/// Set exposure: auto, or a manual shutter index (\`DevShutterTimeType\`).` `SetExposure { shutter: i32, auto: bool },`
  - `framing.rs` — extend `FramingValues` with `auto_exposure: bool` + `manual_shutter: i32` (snapshot both in `from_settings`); extend `FramingPlan` with `exposure: bool` (and `ALL`/`any()`); in `plan_framing_send`'s diff: `exposure: last.auto_exposure != current.auto_exposure || last.manual_shutter != current.manual_shutter,`; in `apply_framing_settings`: `if plan.exposure { ctl.send_command(WorkerCommand::SetExposure { shutter: current.manual_shutter, auto: current.auto_exposure }); }`. Update `framing_values_snapshot_maps_settings_fields` and any struct-literal `ObsbotSettings { .. }` in tests with the two new fields.

- [ ] **Step 4: Verify.** `cargo nextest run -p wc-core --all-features obsbot` — all pass (stub platform swallows the new command; no Windows needed). fmt/clippy/doc gates (this module is feature-gated — keep any cross-module references as plain code spans).

- [ ] **Step 5: Commit:** `feat(obsbot): exposure settings + coalesced send plan`

---

### Task 7: OBSBOT manual exposure — shim, FFI, worker handling, status row (Windows half)

**Files:**
- Modify: `vendor/libdev/shim/obsbot_shim.h` (extern-"C" surface)
- Modify: `vendor/libdev/shim/obsbot_shim.cpp`
- Modify: `crates/wc-core/src/input/obsbot/platform/windows.rs` (`mod ffi` ~51-87, `handle_command` ~339-384)
- Modify: `crates/wc-core/src/input/obsbot/section.rs` (status row)
- Modify: `docs/runbooks/obsbot.md` (document the new controls + that manual exposure survives take-control)

**Interfaces:**
- Consumes: `WorkerCommand::SetExposure` and `shutter_label` from Task 6.
- Produces: `int obsbot_set_exposure(obsbot_device *dev, int32_t shutter_time, bool auto_enabled)` in the shim; the worker applies it via the existing `manual()` in-control guard.

- [ ] **Step 1: Shim.** Declare in `obsbot_shim.h` next to `obsbot_set_zoom`, mirroring its doc style; define in `obsbot_shim.cpp`:

```cpp
/* Set exposure. auto_enabled=true restores auto exposure (shutter_time
 * ignored); false applies a manual DevShutterTimeType index. Uses
 * cameraSetExposureAbsolute — category includes the tiny series (unlike the
 * tail-air-only cameraSetExposureModeR used in take-control step 5). */
int obsbot_set_exposure(obsbot_device *dev, int32_t shutter_time,
			bool auto_enabled)
{
	if (dev == nullptr) {
		return -1;
	}
	Device &d = *reinterpret_cast<Device *>(dev);
	return d.cameraSetExposureAbsolute(shutter_time, auto_enabled) ==
			       RM_RET_OK
		       ? 0
		       : -1;
}
```

Match the file's actual null-check/cast idiom (copy from `obsbot_set_zoom`'s body verbatim, changing only the SDK call). Take-control step 5 (AE re-assert) stays **unchanged** — AE-on remains the device state on control acquisition, and the framing re-apply (Task 6) immediately sends the operator's stored exposure after it, exactly as it re-sends gimbal/zoom/FOV over the recentered defaults.

- [ ] **Step 2: FFI + worker.** In `windows.rs` `mod ffi`, add beside the `obsbot_set_zoom` binding: `pub fn obsbot_set_exposure(dev: *mut ObsbotDevice, shutter_time: i32, auto_enabled: bool) -> c_int;` (match the existing binding types exactly — if the shim uses `int`/`bool`, mirror what `obsbot_set_zoom`/`obsbot_take_control` do for return/bool types). In `handle_command`, add the arm following the `SetZoom` pattern:

```rust
            WorkerCommand::SetExposure { shutter, auto } => {
                self.manual("set exposure", |dev| unsafe {
                    ffi::obsbot_set_exposure(dev, shutter, auto)
                });
            }
```

(Adapt to `manual()`'s actual closure signature.) `platform/stub.rs` needs no change.

- [ ] **Step 3: Status row.** In `section.rs`'s `render_status_section`, after the existing status rows, add a line so the raw slider index is legible:

```rust
    if !settings.auto_exposure {
        ui.label(format!("Shutter: {}", super::shutter_label(settings.manual_shutter)));
    }
```

(Adapt to the section renderer's actual access to `ObsbotSettings` — it already renders setting-dependent rows; follow its existing pattern for obtaining the resource.)

- [ ] **Step 4: Runbook.** In `docs/runbooks/obsbot.md`, under the manual-framing section, document: the two new Camera-section controls, that they persist under the `obsbot` key, that manual exposure is re-applied after every take-control (step 5 still asserts AE first; the stored exposure follows immediately), and the LED-prop rationale (lock exposure for the dancer; let the prop blow out).

- [ ] **Step 5: Verify.** On this macOS machine: `cargo check -p wc-core --all-features` + `cargo nextest run -p wc-core --all-features obsbot` (compiles the stub; the C++ shim only builds on Windows). fmt/clippy/check-secrets gates. **Flag in the task report:** the shim + device behavior (including whether `cameraSetExposureAbsolute` visibly changes the Tiny 2 Lite image, and that AE-on → manual → AE-on round-trips) must be smoke-tested on the Windows kiosk before the next deployment — there is no way to compile or exercise the C++ path here.

- [ ] **Step 6: Commit:** `feat(obsbot): manual exposure via SDK shim (slider + auto checkbox)`

---

### Task 8: Dead-reckoned reservation anchor

A dancer crossing behind another emerges on the far side of a frozen anchor — outside `ASSOC_MAX_DIST` — and gets a fresh slot (identity/color swap). Advance a reserved slot's anchor along its last centroid velocity, with decay and a hard cap.

**Files:**
- Modify: `crates/wc-core/src/input/body/selection.rs` (pure function + tests)
- Modify: `crates/wc-core/src/input/body/pipeline.rs` (`SlotTrack` ~394-465, `lose` ~446-455, `associate_detections` ~845-885, anchor update ~1029, harness test)

**Interfaces:**
- Consumes: `SlotTrack.anchor`, `assign_slots` (unchanged).
- Produces: `pub fn dead_reckoned_anchor(anchor: Vec2, vel: Vec2, elapsed_secs: f32) -> Vec2` in `selection.rs`; `SlotTrack` fields `anchor_vel: Vec2`, `anchor_at: Duration`, `lost_at: Duration`.

- [ ] **Step 1: Write the failing pure-fn tests** in `selection.rs`:

```rust
    /// Dead reckoning advances along the velocity, decays (never overshoots
    /// v·τ), and is capped at ASSOC_MAX_DIST total advance.
    #[test]
    fn dead_reckoned_anchor_decays_and_caps() {
        let a = Vec2::new(0.5, 0.5);
        // Zero velocity: identity.
        assert_eq!(dead_reckoned_anchor(a, Vec2::ZERO, 2.0), a);
        // Short elapsed ≈ linear advance.
        let v = Vec2::new(0.2, 0.0);
        let short = dead_reckoned_anchor(a, v, 0.1);
        assert!((short.x - (0.5 + 0.02)).abs() < 0.005, "{short:?}");
        // Long elapsed converges to v·τ, not v·t.
        let long = dead_reckoned_anchor(a, v, 10.0);
        assert!((long.x - (0.5 + 0.2 * RECKON_DECAY_TAU)).abs() < 1e-3, "{long:?}");
        // A hot velocity is capped at ASSOC_MAX_DIST total advance.
        let capped = dead_reckoned_anchor(a, Vec2::new(5.0, 0.0), 10.0);
        assert!((capped - a).length() <= ASSOC_MAX_DIST + 1e-6);
    }
```

- [ ] **Step 2: Run to verify failure**, then implement in `selection.rs` near `ASSOC_MAX_DIST`:

```rust
/// Decay time constant for dead-reckoning a reserved slot's anchor, seconds:
/// the lost person's centroid velocity is integrated with exponential decay
/// (they were moving when occluded; they do not keep moving forever).
pub const RECKON_DECAY_TAU: f32 = 1.0;

/// Advance a reserved slot's anchor along its last centroid velocity so a
/// dancer crossing behind another re-binds to their own slot on the far
/// side instead of claiming a fresh one (identity/color swap). The advance
/// is the integral of `vel · exp(−t/τ)` and is capped at [`ASSOC_MAX_DIST`]
/// so a bad velocity estimate can never fling the anchor across the frame.
#[must_use]
pub fn dead_reckoned_anchor(anchor: Vec2, vel: Vec2, elapsed_secs: f32) -> Vec2 {
    let advance =
        vel * (RECKON_DECAY_TAU * (1.0 - (-elapsed_secs / RECKON_DECAY_TAU).exp()));
    anchor + advance.clamp_length_max(ASSOC_MAX_DIST)
}
```

- [ ] **Step 3: Wire into the worker** (`pipeline.rs`):
  - `SlotTrack` — add three fields with docs: `/// EMA'd centroid velocity (square-norm units/s) — dead-reckons the anchor while Reserved.` `anchor_vel: Vec2,` `/// Worker time of the last inference-driven anchor update.` `anchor_at: Duration,` `/// Worker time this occupancy was lost (entered Reserved).` `lost_at: Duration,`. Initialize them wherever `SlotTrack` is constructed/reset (`Default`/`release()` — match the existing reset idiom; velocity zero, times zero).
  - Anchor update in `run_slot_inference` (the existing `slot.anchor = Vec2::new(next_roi.cx, next_roi.cy);` at ~1029) becomes:

```rust
                let new_anchor = Vec2::new(next_roi.cx, next_roi.cy);
                let dt = now.saturating_sub(slot.anchor_at).as_secs_f32();
                if dt > 0.0 && dt < 1.0 {
                    // EMA matching the landmark velocity smoothing (alpha 0.5);
                    // a stale gap (round-robin skip storm) resets instead of
                    // fabricating a huge finite difference.
                    let raw = (new_anchor - slot.anchor) / dt;
                    slot.anchor_vel += (raw - slot.anchor_vel) * 0.5;
                } else {
                    slot.anchor_vel = Vec2::ZERO;
                }
                slot.anchor = new_anchor;
                slot.anchor_at = now;
```

  (If `now` is not already in scope in `run_slot_inference`, thread the worker clock in the same way the caller passes it to `lose` — it is available at every call site.)
  - `lose(&mut self, now: Duration)` — add `self.lost_at = now;` (covers both the normal and young-edge reservation paths if both route through `lose`; if the young-edge path sets `Reserved` elsewhere, set `lost_at` there too — grep `SlotPhase::Reserved`).
  - `associate_detections` — the anchor snapshot becomes phase-aware:

```rust
                SlotPhase::Active => anchors[i] = Some(slot.anchor),
                SlotPhase::Reserved => {
                    let elapsed = now.saturating_sub(slot.lost_at).as_secs_f32();
                    anchors[i] =
                        Some(dead_reckoned_anchor(slot.anchor, slot.anchor_vel, elapsed));
                }
```

  - On a fresh `Free → Active` claim (the existing `slot.active_since = now;` branch), also `slot.anchor_vel = Vec2::ZERO; slot.anchor_at = now;` — a new occupant carries no stale momentum. On `Reserved → Active` re-acquisition set `slot.anchor_at = now;` (keep `anchor_vel`; the EMA corrects it).

- [ ] **Step 4: Harness test.** In `pipeline.rs`'s mock-inference test module, add `reserved_anchor_dead_reckons_toward_a_moving_return`, modeled directly on `mature_track_reserves_and_reacquisition_keeps_its_age` / `invalid_frame_reserves_tracks_then_reacquires`: drive a track with a steadily right-moving detection for enough frames to establish `anchor_vel`, drop detections for ~1 s (Reserved), then present a detection displaced beyond `ASSOC_MAX_DIST` from the *frozen* anchor but within it of the *reckoned* anchor — assert it re-binds to the same slot (same slot index, `fresh[s]` set) rather than claiming a new one.

- [ ] **Step 5: Verify.** `cargo nextest run -p wc-core --all-features body` — all pass, including every pre-existing reservation/association test (zero-velocity tracks reckon to the identity, so existing fixtures are unaffected). fmt/clippy gates.

- [ ] **Step 6: Commit:** `feat(body): dead-reckoned reservation anchor`

---

### Task 9: Per-edge silhouette motion weights (worker → main world)

The CPU half of fan-sheds-fire: a per-texel frame-delta of the smoothed mask, sampled at each edge point into a `0..1` weight, carried next to the edge buffer. No GPU changes yet — Task 10 consumes it.

**Files:**
- Modify: `crates/wc-core/src/input/body/mask.rs` (`MaskProcessor` — new `ema_prev`/`motion` buffers, delta computation in `ingest` ~123-176, accessor)
- Modify: `crates/wc-core/src/input/body/edges.rs` (extraction samples the motion field into parallel weights)
- Modify: `crates/wc-core/src/input/body/pipeline.rs` (`write_payload` ~1072-1079 threads the weights)
- Modify: `crates/wc-core/src/input/body/transport.rs` (payload gains `edge_motion: Vec<f32>`, pre-allocated at `MAX_EDGE_POINTS`)
- Modify: `crates/wc-core/src/input/body/mod.rs` (`SilhouetteEdges` gains `pub motion: Vec<f32>`)
- Modify: `crates/wc-core/src/input/body/systems.rs` (`poll_body_worker` ~263-266 copies it)

**Interfaces:**
- Produces: `SilhouetteEdges.motion: Vec<f32>` — same length/order as `points`, each `0..1` (`1` = boundary sweeping at/above `EDGE_MOTION_FULL`); `pub const EDGE_MOTION_FULL: f32` in `edges.rs`.

**Hot-path rule:** every buffer added here is pre-allocated at construction (`vec![0.0; MASK_SIZE * MASK_SIZE]` in `MaskProcessor::new`, `Vec::with_capacity(MAX_EDGE_POINTS)` in the payload pool) and refilled with `clear()`/`copy_from_slice` — zero steady-state allocation on the worker thread.

- [ ] **Step 1: Write the failing test** in `edges.rs`'s test module (a pure test over the new sampling helper):

```rust
    /// Edge weights come from the per-texel motion field: an edge point on a
    /// moved boundary weighs in; a static boundary weighs ~0; weights clamp
    /// to 1.
    #[test]
    fn edge_motion_weight_maps_and_clamps() {
        let mut motion = vec![0.0_f32; MASK_SIZE * MASK_SIZE];
        // Texel (64, 128) saw a full-scale delta; (192, 128) saw none.
        motion[128 * MASK_SIZE + 64] = EDGE_MOTION_FULL * 2.0;
        let moving = motion_weight_at(&motion, Vec2::new(64.5 / 256.0, 128.5 / 256.0));
        let still = motion_weight_at(&motion, Vec2::new(192.5 / 256.0, 128.5 / 256.0));
        assert!((moving - 1.0).abs() < 1e-6, "clamps to 1: {moving}");
        assert!(still.abs() < 1e-6, "static boundary: {still}");
    }
```

- [ ] **Step 2: Run to verify failure**, then implement.
  - `mask.rs` — `MaskProcessor` gains two pre-allocated `Vec<f32>` fields, `ema_prev` and `motion` (both `MASK_SIZE * MASK_SIZE`, documented). In `ingest`, immediately **before** the in-place temporal blend: `self.ema_prev.copy_from_slice(&self.ema);`. Immediately **after** the blend, fill the motion field:

```rust
        // Per-texel frame delta of the smoothed mask: the edge-motion signal
        // (a swept prop/limb moves the boundary; a standing body does not).
        // Per-frame, not per-second: the worker cadence is capped and steady
        // (max_inference_hz), so the constant absorbs the rate.
        for ((m, now), prev) in self
            .motion
            .iter_mut()
            .zip(self.ema.iter())
            .zip(self.ema_prev.iter())
        {
            *m = (now - prev).abs();
        }
```

  Add an accessor `pub fn motion(&self) -> &[f32]` beside the existing `smoothed()`.
  - `edges.rs` — add the constant + helper + weight emission:

```rust
/// Per-frame mask delta that maps to full edge-motion weight (1.0). The
/// temporal blend damps boundary deltas to roughly 0.1..0.3 per frame on a
/// sweeping limb at the default combine ratio; 0.15 puts a brisk sweep at
/// full weight. Eye-tune at the venue alongside `edge_motion_bias`.
pub const EDGE_MOTION_FULL: f32 = 0.15;

/// Sample the per-texel motion field at a mask-UV position, mapped to a
/// `0..1` emission weight (nearest texel; edges were extracted from the same
/// grid, so sub-texel filtering buys nothing).
#[must_use]
pub fn motion_weight_at(motion: &[f32], pos: Vec2) -> f32 {
    let x = ((pos.x * MASK_SIZE as f32) as usize).min(MASK_SIZE - 1);
    let y = ((pos.y * MASK_SIZE as f32) as usize).min(MASK_SIZE - 1);
    (motion[y * MASK_SIZE + x] / EDGE_MOTION_FULL).clamp(0.0, 1.0)
}
```

  (If clippy rejects the `as` casts under the workspace lint policy, use the file's existing float→index idiom from the extraction loop — it already converts UV to texel indices; mirror it.) Extend `extract_edges_append`'s signature with `motion: &[f32], weights: &mut Vec<f32>` and push `weights.push(motion_weight_at(motion, pos));` at both points where an `EdgePoint` is pushed (horizontal and vertical passes), so `weights` stays index-parallel with the points.
  - `transport.rs` — the pooled payload gains `pub edge_motion: Vec<f32>` (capacity `MAX_EDGE_POINTS`, cleared where `edges` is cleared).
  - `pipeline.rs` `write_payload` — pass `slot.mask.motion()` and `&mut payload.edge_motion` through the per-slot `extract_edges_append` calls.
  - `mod.rs` — `SilhouetteEdges` gains `/// Per-point emission weight 0..1 (see edges.rs EDGE_MOTION_FULL): how fast the boundary was moving at this point.` `pub motion: Vec<f32>,` (update its `Default`/constructor).
  - `systems.rs` `poll_body_worker` — beside the existing points copy: `edges.motion.clear(); edges.motion.extend_from_slice(&payload.edge_motion);` under the same `generation` bump.

- [ ] **Step 3: Verify.** `cargo nextest run -p wc-core --all-features body` — new test passes, existing edge/mask tests pass (update `extract_edges_append` call sites in tests with a zeroed motion slice + scratch Vec). fmt/clippy/doc gates.

- [ ] **Step 4: Commit:** `feat(body): per-edge silhouette motion weights`

---

### Task 10: Motion-biased edge emission (GPU + setting)

The kernel half: dead particles re-roll their hashed edge pick up to 3 times, accepting moving-boundary points preferentially. `edge_motion_bias = 0` reproduces today's uniform pick bit-for-bit-equivalent behavior; `1` accepts almost exclusively where the silhouette moves. Default 0.5. A swept fan — part of the segmented blob — sheds fire along its arc.

**Files:**
- Modify: `crates/wc-sketches/src/radiance/settings.rs` (new field + serde fn + both defaults tests)
- Modify: `crates/wc-sketches/src/radiance/compute/sim_params.rs` (`RadianceSimParamsGpu` + offset/size tests)
- Modify: `crates/wc-sketches/src/radiance/systems/sim_params.rs` (baker copies the setting)
- Modify: `crates/wc-sketches/src/radiance/compute/edge_upload.rs` (extract + upload the weights)
- Modify: `crates/wc-sketches/src/radiance/compute/pipeline.rs` (buffer alloc, layout entry, bind group)
- Modify: `assets/shaders/radiance/simulate.wgsl` (binding + biased pick)

**Interfaces:**
- Consumes: `SilhouetteEdges.motion` from Task 9.
- Produces: `RadianceSettings.edge_motion_bias: f32` (0..=1, default 0.5, User, section "Simulation", label "Motion emission bias"); `RadianceSimParamsGpu.edge_motion_bias: f32`; storage buffer `edge_motion` at `@group(0) @binding(3)` (`MAX_EDGE_POINTS × 4` bytes, read-only).

- [ ] **Step 1: Setting + uniform (TDD on the CPU side).** Add the settings field following the exact `background_subdue` pattern:

```rust
    /// Bias particle births toward *moving* silhouette edges: 0 = uniform
    /// (every edge point equally likely, the pre-prop behaviour), 1 = births
    /// crowd wherever the boundary is sweeping. A held fan or prop is part
    /// of the segmented silhouette, so at higher bias a swept fan sheds fire
    /// along its arc. Live-tunable at the venue.
    #[setting(
        default = 0.5_f32,
        min = 0.0_f32,
        max = 1.0_f32,
        step = 0.05_f32,
        label = "Motion emission bias",
        section = "Simulation",
        category = User
    )]
    #[serde(default = "default_edge_motion_bias")]
    pub edge_motion_bias: f32,
```

  plus `fn default_edge_motion_bias() -> f32 { 0.5 }` and entries in **both** `missing_field_preserves_sibling_values`-style and `default_values_match_serde_defaults` tests. In `compute/sim_params.rs`, add `edge_motion_bias: f32` to `RadianceSimParamsGpu`: **reuse an existing trailing pad scalar if the struct has one** (keeping the 400-byte size and only re-documenting the offset); otherwise append the field plus 12 pad bytes (uniform structs round to 16), bump the size constant, and update the WGSL `SimParams` struct and the locked offset/size tests to match — the tests are the contract's home, so they change *with* the struct, in the same commit. Baker (`bake_radiance_sim`): `out.edge_motion_bias = settings.edge_motion_bias.clamp(0.0, 1.0);`. Add a baker test asserting the copy (follow `bake_packs_slot_ranges_and_cdf`'s shape).

- [ ] **Step 2: Upload path.** `edge_upload.rs`: `ExtractedEdges` gains a `motion: Vec<f32>` scratch filled in `extract_silhouette_edges` under the same generation gate; `upload_silhouette_edges` writes it to the new buffer with the same `write_buffer` idiom as the points. `compute/pipeline.rs`: allocate `edge_motion_buffer` once in `init_radiance_pipeline` beside `edges_buffer` (`MAX_EDGE_POINTS * std::mem::size_of::<f32>()`, `STORAGE | COPY_DST`); add a fourth bind-group-layout entry (binding 3, read-only storage) and the matching bind-group entry in `prepare_radiance_bind_group`. Pipeline-owned resources are process-lifetime like `sim_params_buffer`/`edges_buffer` — no new removal system needed (the existing `remove_radiance_sim_params_if_absent` + bind-group-cache clear already cover the per-frame objects).

- [ ] **Step 3: Kernel.** In `simulate.wgsl`, add below the edges binding:

```wgsl
@group(0) @binding(3) var<storage, read> edge_motion: array<f32>;
```

Add `edge_motion_bias: f32` to the WGSL `SimParams` struct at the position matching the Rust offset. In the respawn block, between the `e_idx` computation and `let e = edges[...]`:

```wgsl
    // Motion-biased pick: re-roll the hashed edge up to 3 times, accepting
    // with probability floor + (1-floor)·weight — births crowd the moving
    // boundary (a swept fan sheds fire along its arc) while bias 0 keeps
    // the uniform pick. Bounded retries: worst case accepts the last roll.
    let motion_floor = 1.0 - params.edge_motion_bias;
    for (var k = 0u; k < 3u; k = k + 1u) {
        let w = edge_motion[min(e_idx, params.edge_count - 1u)];
        if (rand01(hash2(idx ^ (0xa511e9b3u + k), frame)) < motion_floor + (1.0 - motion_floor) * w) {
            break;
        }
        e_idx = params.slot_start[slot]
            + (hash2(idx * 2654435769u ^ (0x27d4eb2fu + k), frame) % params.slot_count[slot]);
    }
```

- [ ] **Step 4: Verify.** `cargo nextest run -p wc-sketches --all-features` — offset tests, baker test, settings tests all pass. fmt/clippy/doc gates. `cargo rund`, enter Radiance: with `edge_motion_bias` at 0 confirm the look matches the pre-change baseline; at 1.0 wave an arm and confirm births visibly crowd the sweep; leave at 0.5. (Screen must be foregrounded — a backgrounded window invalidates any capture-based check.) Prompt Madison for the eye-tune: `edge_motion_bias` slider live in Simulation, plus `EDGE_MOTION_FULL` in `edges.rs` if the sweep saturates too early/late.

- [ ] **Step 5: Commit:** `feat(radiance): motion-biased edge emission`

---

## Deliberately out of scope

- **Recorded-footage replay `FrameSource`** — pinned per Madison (2026-07-22); revisit if time appears.
- **Luma telemetry / per-crop histogram stretch / "dark venue" threshold profile** — not approved in this round; the exposure controls (Tasks 6–7) are the chosen lever for exposure/dark scenarios.
- **Occlusion-aware differential presence-hold** — superseded for now by dead-reckoning (Task 8), the smaller of the two association fixes.

## Self-review notes

- Task ordering is strictly easy→hard per Madison's directive; Tasks 1–5 are an afternoon-class prefix that stands alone if execution stops early.
- Tasks 1 and 3 touch the same lines: Task 3's rewrite *includes* Task 1's clamp (shown in its code block) — execute in order, never in parallel.
- Task 7 cannot be device-verified on this machine (Windows-only SDK IO); its report must carry the kiosk smoke-test flag forward.
- Line numbers are anchors from the 2026-07-22 investigation, not gospel — match on the quoted code, not the number.
