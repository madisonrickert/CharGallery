# Radiance Bioluminescence Rework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Retire the fullscreen beat-contour overlay and move all of its energy into the particle world — soft silhouette repel + contact glow, density-adaptive beat bursts, an in-medium flare-wave, and impulse-coupled motion glow — per `docs/superpowers/specs/2026-07-23-radiance-bioluminescence-design.md`.

**Architecture:** The beat-wave clock survives as CPU state baked into the sim uniform; a new **signed** chamfer distance field reaches the compute kernel through an `edge_upload`-shaped persistent storage buffer; the `Particle` struct gains one `glow: f32` lane that the kernel writes (contact/flare/motion) and `render.wgsl` consumes as a brightness multiplier.

**Tech Stack:** Rust (Bevy 0.19), WGSL compute + Material2d render shaders, bytemuck POD parity structs.

## Global Constraints

- **Kernel parity discipline:** `RadianceParticle` / `RadianceSimParamsGpu` (`compute/sim_params.rs`), the WGSL `Particle`/`SimParams` structs in `simulate.wgsl`, and the WGSL `Particle` mirror in `render.wgsl` change **together in the same commit**, with the `offset_of!`/size tests and `SIM_PARAMS_SIZE`-dependent tests updated to the new true values. WGSL uniform arrays of scalars have 16-byte stride: `[f32; 8]` on the Rust side is mirrored as **two `vec4<f32>`** fields.
- **No allocation on any hot path.** Field scratch/output buffers allocate once at resource construction; extract/upload scratch uses the `ExtractedEdges` capacity+`clear()` discipline; the baker and wave clock allocate nothing.
- **Stable-`BufferId` discipline:** the distance field reaches the GPU exclusively via a new persistent `STORAGE | COPY_DST` buffer on `RadiancePipeline` (created once in `init_radiance_pipeline`, refilled with `write_buffer`). The `RadianceBindGroupCache` key (particle `BufferId`) is **unchanged** — never key or invalidate on the field.
- **Idle gating:** the beat-wave clock advances in a system gated `in_state(AppState::Radiance)` (like today's `update_radiance_pulses`), NOT in the Active-only baker; field recompute keeps its existing generation gating; no new per-frame work runs when the sketch is inactive.
- **New tuning gains ship as Dev-category settings** on `RadianceSettings` (behind the per-launch ADVANCED toggle): `repel_strength`, `repel_radius`, `contact_glow`, `flare_gain`, `flare_band`, `burst_scale`, `burst_boost_cap`, `motion_glow`. Everything else stays a named const in `systems/sim_params.rs`.
- **No new dependencies, no new WC_DEBUG toggles** (a gain set to 0 is the A/B switch).
- Verification commands: `cargo nextest run -p wc-sketches`, `cargo clippy -p wc-sketches --all-targets --all-features -- -D warnings`; full workspace gates in Task 5 only.
- `cargo xtask capture` needs the app window **foregrounded** (agent/background runs produce all-black frames — check a known-good sketch before diagnosing regressions).

---

### Task 1: Retire the pulse overlay; beat-wave clock into the sim uniform

**Files:**
- Delete: `assets/shaders/radiance/pulse.wgsl`
- Modify: `crates/wc-sketches/src/radiance/pulse.rs` (heavy prune + rename of survivors)
- Modify: `crates/wc-sketches/src/radiance/mod.rs` (module doc + registration ~lines 217-239)
- Modify: `crates/wc-sketches/src/radiance/systems/spawn.rs` (quad spawn ~205-218, `init_asset` ~373, resource insert/remove ~249/351, imports ~40/127, tests ~452-484)
- Modify: `crates/wc-sketches/src/radiance/compute/sim_params.rs` (uniform tail fields + tests)
- Modify: `assets/shaders/radiance/simulate.wgsl` (SimParams mirror — fields declared, consumed in Task 3)
- Modify: `crates/wc-sketches/tests/radiance_lifecycle.rs:109` (pulse-material assertion)
- Modify: `crates/wc-sketches/src/radiance/distance_field.rs` doc header (its consumer changes; content change lands in Task 2)

**Interfaces:**
- Produces: `RadianceSimParamsGpu` tail (exact layout below) — `edge_motion_bias` at 400 keeps its offset; the old `_pad: [f32; 3]` is replaced by real fields; struct grows 416 → 496 bytes. Task 3 consumes `wave_radius_px`/`wave_strength`; Task 2 consumes the repel/glow scalars.
- Produces: `advance_beat_waves` system + `RadianceBeatWaves` resource (renamed `RadiancePulses`, same ring-buffer/rising-edge mechanics via the existing pure `step_pulses`).

- [ ] **Step 1: Prune `pulse.rs` to the wave clock**

Keep (unchanged): `MAX_PULSES`, `PULSE_SPEED_PX_S`, `PULSE_WIDTH_PX` (re-doc as the flare band default), `PULSE_LIFETIME_S`, `BEAT_EDGE`, `PULSE_DT_CAP`, `PulseSlot`, `step_pulses`, `raw_slot_fades`, `union_fade`, and their tests. Rename `RadiancePulses` → `RadianceBeatWaves` (field names unchanged; fix all uses).
Delete: `RadiancePulseUniform`, `RadiancePulseMaterial` (+ its `Material2d` impl), `pack_pulse_uniform`, `pulse_uniform_dead`, `gradient_sample`, `blend_present_colors`, `update_radiance_pulses`, and their tests (wave color and the master-fade lane die with the overlay: an in-medium flare has no color of its own and dies with its particles, so the union-fade "waves can't outlive the last figure" machinery is no longer needed — keep `raw_slot_fades`/`union_fade` only if another module still imports them; check with `rg` and delete if unreferenced).
Rewrite the module doc: this is now the beat-wave clock whose slots bake into the sim uniform (Task 3's flare-wave); the overlay history gets one sentence.

Replace the driver with:

```rust
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
```

(`AudioAnalysis::neutral` exists via `systems::sim_params::neutral_audio` — use whichever the old driver imported. Register in `mod.rs` exactly where `update_radiance_pulses` was, same `run_if(in_state(AppState::Radiance))`, keeping `update_distance_field.before(...)` pointed at the new name.)

- [ ] **Step 2: Uniform tail — Rust + WGSL together**

In `compute/sim_params.rs`, replace the `edge_motion_bias` + `_pad` tail of `RadianceSimParamsGpu` with:

```rust
    /// Motion-emission bias `0..=1` (unchanged; offset 400).
    pub edge_motion_bias: f32,
    /// Silhouette repel acceleration at the boundary, world px/s².
    pub repel_strength: f32,
    /// Repel influence radius outside the boundary, world px.
    pub repel_radius_px: f32,
    /// Glow added per second at the boundary (falloff-weighted).
    pub contact_glow: f32,
    /// Per-frame glow retention, baked CPU-side as `GLOW_PER_SECOND.powf(dt)`.
    pub glow_decay_baked: f32,
    /// Flare brightness gain as a beat wave passes a particle.
    pub flare_gain: f32,
    /// Gaussian half-band of the flare front, world px.
    pub flare_band_px: f32,
    /// Glow gain coupling to the limb-impulse weight (motion disturbance).
    pub motion_glow: f32,
    /// Per-wave current radius, world px (mirrors WGSL `wave_radius_a/_b`,
    /// two `vec4<f32>` — uniform scalar arrays have 16-byte stride).
    pub wave_radius_px: [f32; 8],
    /// Per-wave strength (0 = dead slot), age-decayed CPU-side.
    pub wave_strength: [f32; 8],
```

Update the layout tests: `edge_motion_bias` stays 400; new offsets 404, 408, 412, 416, 420, 424, 428, then `wave_radius_px` at 432 and `wave_strength` at 464; total size **496**. Update `sim_params_size_tracks_max_impulses` (`TAIL_BYTES` becomes 96) and `buffer_size_constants_match_contracts` in `compute/pipeline.rs` (416 → 496).

In `simulate.wgsl`'s `struct SimParams`, replace the trailing `edge_motion_bias: f32,` with:

```wgsl
    edge_motion_bias: f32,
    repel_strength: f32,
    repel_radius_px: f32,
    contact_glow: f32,
    glow_decay_baked: f32,
    flare_gain: f32,
    flare_band_px: f32,
    motion_glow: f32,
    wave_radius_a: vec4<f32>,
    wave_radius_b: vec4<f32>,
    wave_strength_a: vec4<f32>,
    wave_strength_b: vec4<f32>,
```

(The kernel does not read them yet; Tasks 2-3 do. naga accepts declared-unused fields.)

- [ ] **Step 3: Remove the quad, material, shader, and test references**

`spawn.rs`: delete the pulse-quad spawn block (~205-218), the `pulse_materials` system param (~127), the `RadiancePulses::default()` insert (~249) → insert `RadianceBeatWaves::default()` instead, the removal at ~351 → `RadianceBeatWaves`, imports at ~40, `init_asset::<RadiancePulseMaterial>()` at ~373; update the spawn-count test comment + assertion (~452-460: four draw entities → three) and the exit test (~484). `tests/radiance_lifecycle.rs:109`: drop/replace the pulse-material assertion. Delete `assets/shaders/radiance/pulse.wgsl`. `mod.rs`: update the module docs' pulse paragraphs (~55-58 stays as the wave-clock module; ~227-236 documents `advance_beat_waves`).

- [ ] **Step 4: Verify**

Run: `cargo nextest run -p wc-sketches`  → PASS (layout tests at the new offsets; spawn/lifecycle tests at three draw entities).
Run: `cargo clippy -p wc-sketches --all-targets --all-features -- -D warnings` → clean.
Run: `rg -n 'RadiancePulseMaterial|pulse.wgsl|pack_pulse_uniform' crates assets` → no hits.

- [ ] **Step 5: Commit**

```bash
git add -A crates/wc-sketches assets/shaders/radiance
git commit -m "feat(radiance): retire the pulse overlay; beat-wave clock bakes into sim uniforms"
```

---

### Task 2: Signed distance field to the kernel; repel + contact glow; the particle glow lane

**Files:**
- Modify: `crates/wc-sketches/src/radiance/distance_field.rs` (signed chamfer, resource restructure)
- Create: `crates/wc-sketches/src/radiance/compute/field_upload.rs` (extract + upload, `edge_upload` shape)
- Modify: `crates/wc-sketches/src/radiance/compute/mod.rs` (module decl), `compute/pipeline.rs` (binding 4 + buffer), `compute/sim_params.rs` (`RadianceParticle` + tests)
- Modify: `assets/shaders/radiance/simulate.wgsl` (binding 4, sampling, repel, glow), `assets/shaders/radiance/render.wgsl` (Particle mirror + glow consumption)
- Modify: `crates/wc-sketches/src/radiance/systems/spawn.rs` (field image → plain resource), `systems/sim_params.rs` (bake the new scalars)
- Modify: `crates/wc-sketches/src/radiance/settings.rs` (Dev knobs: `repel_strength`, `repel_radius`, `contact_glow`)

**Interfaces:**
- Produces: `RadianceDistanceField { pub signed: Vec<u8>, pub generation: u64, scratch: Vec<u32>, scratch_in: Vec<u32> }` (no more `Handle<Image>`); `pub fn signed_chamfer(mask_seed_out: &mut [u32], mask_seed_in: &mut [u32], out: &mut [u8])`; pipeline `field_buffer` (65 536 B, binding 4, `array<u32>` of 4-packed bytes); WGSL helpers `field_signed_px(texel: vec2<i32>) -> f32` and `field_world(pos) -> f32`; particle `glow: f32` at offset 32, struct size 40.
- Signed encoding: byte 128 = boundary; exterior `128 + d_out/DIST_MAX_TEXELS·127`, interior `128 − min(d_in, DIST_MAX_TEXELS)/DIST_MAX_TEXELS·127` (one scale both sides, ~2.6 texels ≈ 5-11 px world per step — inside the flare band and repel radius resolution needs).

- [ ] **Step 1: Failing tests for the signed chamfer**

In `distance_field.rs` tests (adapting the existing three):

```rust
    #[test]
    fn signed_field_is_biased_at_128() {
        // Half-plane body (rows 0..128): outside grows above 128 with the
        // 3-4 chamfer scale, inside falls below 128 with the same scale.
        let mut mask = vec![0_u8; MASK_SIZE * MASK_SIZE];
        for y in 0..128 {
            for x in 0..MASK_SIZE {
                mask[y * MASK_SIZE + x] = 255;
            }
        }
        let mut s_out = vec![0_u32; MASK_SIZE * MASK_SIZE];
        let mut s_in = vec![0_u32; MASK_SIZE * MASK_SIZE];
        let mut out = vec![0_u8; MASK_SIZE * MASK_SIZE];
        signed_chamfer_from_mask(&mask, &mut s_out, &mut s_in, &mut out);

        let outside10 = f32::from(out[(127 + 10) * MASK_SIZE + 64]);
        let expect_out = 128.0 + 10.0 / DIST_MAX_TEXELS * 127.0;
        assert!((outside10 - expect_out).abs() <= 2.0, "{outside10} vs {expect_out}");
        let inside10 = f32::from(out[(127 - 10) * MASK_SIZE + 64]);
        let expect_in = 128.0 - 10.0 / DIST_MAX_TEXELS * 127.0;
        assert!((inside10 - expect_in).abs() <= 2.0, "{inside10} vs {expect_in}");
    }

    #[test]
    fn signed_field_interior_gradient_points_at_the_boundary() {
        // Deep interior reads lower than shallow interior: the kernel's
        // ascent direction (+gradient) leads OUT of the body everywhere.
        let mut mask = vec![0_u8; MASK_SIZE * MASK_SIZE];
        for y in 0..128 {
            for x in 0..MASK_SIZE {
                mask[y * MASK_SIZE + x] = 255;
            }
        }
        let mut s_out = vec![0_u32; MASK_SIZE * MASK_SIZE];
        let mut s_in = vec![0_u32; MASK_SIZE * MASK_SIZE];
        let mut out = vec![0_u8; MASK_SIZE * MASK_SIZE];
        signed_chamfer_from_mask(&mask, &mut s_out, &mut s_in, &mut out);
        assert!(out[40 * MASK_SIZE + 64] < out[120 * MASK_SIZE + 64]);
    }
```

Run: `cargo test -p wc-sketches --lib radiance::distance_field` → FAIL (`signed_chamfer_from_mask` missing).

- [ ] **Step 2: Implement the signed transform + resource restructure**

`signed_chamfer_from_mask(mask, scratch_out, scratch_in, out)` (pure, test-facing): seed `scratch_out` body=0/else FAR and `scratch_in` inverted, run the existing relaxation (extract the pass pair from `chamfer_from_seeded` into a `chamfer_relax(scratch: &mut [u32])` helper that leaves raw chamfer units — keep `chamfer_from_seeded`'s public shape by reimplementing it over the helper + the old normalization so the existing three tests still pass), then combine per texel:

```rust
    let d_out = scratch_out[i] as f32 / ORTHO_COST as f32; // texels (allow-listed cast block, mirror the existing one)
    let d_in = scratch_in[i] as f32 / ORTHO_COST as f32;
    let signed = if d_out > 0.0 { d_out } else { -d_in };
    out[i] = (128.0 + signed / DIST_MAX_TEXELS * 127.0).clamp(0.0, 255.0) as u8;
```

Restructure `RadianceDistanceField`: fields `pub signed: Vec<u8>` (65 536, zero-init), `pub generation: u64` (start `u64::MAX`-distinct: use 0 and bump per recompute), `scratch: Vec<u32>`, `scratch_in: Vec<u32>`; `new()` takes no image handle. `update_distance_field` keeps its signature shape minus `Assets<Image>` for output: it still borrows the mask image read-only, seeds both scratches in the same union loop (RGBA any-channel, existing logic — seed `scratch_in` with the inverted condition), runs the relax passes, combines into `self.signed`, bumps `self.generation`. The borrow dance comment simplifies (no output-image borrow). `spawn.rs`: stop allocating the field `Image` (~193-203); construct `RadianceDistanceField::new()` directly.

- [ ] **Step 3: The upload seam (`field_upload.rs`)**

Mirror `edge_upload.rs` verbatim in shape — `ExtractedField { generation: u64 (u64::MAX sentinel), bytes: Vec<u8> (capacity 65 536), dirty: bool }`, `extract_distance_field` (ExtractSchedule, generation-gated copy from `Option<Res<RadianceDistanceField>>`), `upload_distance_field` (PrepareBindGroups before the bind-group prepare, `write_buffer` into `pipeline.field_buffer`). Register both in `RadianceComputePlugin::build` beside their edge twins; add `pub field_buffer: Buffer` to `RadiancePipeline`, created in `init_radiance_pipeline`:

```rust
    let field_buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some("radiance_signed_distance_field"),
        size: FIELD_BUFFER_SIZE, // (MASK_SIZE * MASK_SIZE) as u64, one byte per texel
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
```

plus layout entry binding 4 (read-only storage) and the matching `BindGroupEntry`. A default all-zero buffer decodes as "deep interior everywhere" — guard: the kernel skips repel/flare when `edge_count == 0` (no silhouette ⇒ no field consumers run), so the uninitialized state is unreachable; note this in the field-buffer doc.

- [ ] **Step 4: Particle glow lane + kernel forces (all three shader copies + mirrors in ONE commit)**

`RadianceParticle`: add `pub glow: f32` and `pub _pad: f32` after `slot` (offsets 32/36, size 40; update both offset tests and the doc — `slot` loses its "doubles as padding" clause). WGSL `struct Particle` in **both** `simulate.wgsl` and `render.wgsl`: add `glow: f32, _pad: f32,`.

`simulate.wgsl` additions:

```wgsl
@group(0) @binding(4) var<storage, read> field: array<u32>;

const MASK_DIM: i32 = 256;
const FIELD_DIST_MAX_TEXELS: f32 = 160.0;

// Signed distance at a mask texel, in world px (positive outside the body).
fn field_signed_px(t: vec2<i32>) -> f32 {
    let c = clamp(t, vec2<i32>(0), vec2<i32>(MASK_DIM - 1));
    let i = u32(c.y * MASK_DIM + c.x);
    let byte = (field[i >> 2u] >> ((i & 3u) * 8u)) & 0xffu;
    let texels = (f32(byte) - 128.0) / 127.0 * FIELD_DIST_MAX_TEXELS;
    // World px per mask texel: the fit-to-height mapping (uv_to_world.y
    // spans MASK_DIM texels vertically).
    return texels * params.uv_to_world.y / f32(MASK_DIM);
}

// World position -> mask texel (inverse of mask_uv_to_world).
fn world_to_texel(pos: vec2<f32>) -> vec2<i32> {
    var u = pos.x / params.uv_to_world.x + 0.5;
    if (params.mirror == 1u) {
        u = 1.0 - u;
    }
    let v = 0.5 - pos.y / params.uv_to_world.y;
    return vec2<i32>(vec2<f32>(u, v) * f32(MASK_DIM));
}
```

In the alive branch, after the impulse loop and before drag (comment each term per AGENTS.md):

```wgsl
    // Silhouette repel + contact glow: signed field, positive outside.
    // Central-difference gradient in world space; inside the body the
    // interior chamfer keeps the ascent direction pointing at the nearest
    // boundary, so overlap corrects smoothly instead of trapping.
    var glow = p.glow * params.glow_decay_baked;
    if (params.edge_count > 0u && params.repel_strength > 0.0) {
        let t = world_to_texel(p.position);
        let d = field_signed_px(t);
        if (d < params.repel_radius_px) {
            let gx = field_signed_px(t + vec2<i32>(1, 0)) - field_signed_px(t - vec2<i32>(1, 0));
            // Mask y is down, world y is up: flip the y difference.
            let gy = field_signed_px(t - vec2<i32>(0, 1)) - field_signed_px(t + vec2<i32>(0, 1));
            let g = vec2<f32>(gx, gy);
            let glen = length(g);
            if (glen > 1e-4) {
                // 1 at the boundary (and everywhere inside), 0 at the radius.
                let falloff = 1.0 - clamp(d, 0.0, params.repel_radius_px) / params.repel_radius_px;
                accel = accel + (g / glen) * (params.repel_strength * falloff);
                glow = glow + params.contact_glow * falloff * params.dt;
            }
        }
        // Flare-wave + motion glow land in Tasks 3-4.
    }
    p.glow = min(glow, 4.0);
```

(Also zero `p.glow` in the respawn branch: `p.glow = 0.0;` beside `p.age = 0.0;`.)

`render.wgsl`: multiply the color line — `out.rgb = flame_color(...) * (1.0 + sfac * SPEED_BRIGHTNESS) * (1.0 + p.glow);`.

- [ ] **Step 5: Bake + knobs**

`settings.rs`: three Dev-category `F32` settings — `repel_strength` (default 900.0, "Silhouette repel"), `repel_radius` (default 70.0, "Repel radius (px)"), `contact_glow` (default 1.2, "Contact glow") — mirroring an existing Dev field's attribute shape. `systems/sim_params.rs`: in `bake_radiance_sim`, bake them plus `out.glow_decay_baked = GLOW_PER_SECOND.powf(dt)` with `pub const GLOW_PER_SECOND: f32 = 0.02;` (fast decay: glow is a flash, not a state). Zero all four (and the Task 3-4 gains) in `freeze_radiance_emission`? No — Idle keeps decaying glow via the running kernel; the baker simply stops updating, which leaves the last baked decay constant in place: correct as-is, note it in the freeze fn's doc.

- [ ] **Step 6: Verify**

`cargo nextest run -p wc-sketches` → PASS. `cargo nextest run -p wc-core` (body-contract consumers unaffected) → PASS. Clippy clean. Then a visual smoke: `WC_DEBUG_FORCE_RADIANCE_SYNTHETIC_BODY=1 cargo rund` foregrounded, Radiance selected — particles visibly flow AROUND the phantom instead of across it, brighter at the boundary. (Operator-eyeball if the session is unattended; note in the report either way.)

- [ ] **Step 7: Commit**

```bash
git add -A crates/wc-sketches assets/shaders/radiance
git commit -m "feat(radiance): signed silhouette field drives repel + contact glow via a particle glow lane"
```

---

### Task 3: Flare-wave through the medium + density-adaptive beat burst

**Files:**
- Modify: `assets/shaders/radiance/simulate.wgsl` (flare term), `crates/wc-sketches/src/radiance/systems/sim_params.rs` (burst + integrator), `crates/wc-sketches/src/radiance/settings.rs` (`flare_gain`, `flare_band`, `burst_scale`, `burst_boost_cap`)

**Interfaces:**
- Consumes: Task 1's wave arrays, Task 2's `field_signed_px`/glow lane.
- Produces: `RadianceState` gains `prev_beat: f32` and `est_alive: f32`; pure fns `expected_alive_step(est, emission_prob, particle_count, dt, mean_lifespan) -> f32` and `burst_boost(est_alive, particle_count, cap) -> f32`.

- [ ] **Step 1: Failing tests for the integrator + boost**

In `systems/sim_params.rs` tests:

```rust
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
    fn burst_boost_is_one_when_dense_and_capped_when_empty() {
        assert!((burst_boost(9_000.0, 10_000.0, 4.0) - 1.0).abs() < 0.35);
        assert_eq!(burst_boost(0.0, 10_000.0, 4.0), 4.0);
        let mid = burst_boost(2_500.0, 10_000.0, 4.0);
        assert!(mid > 1.0 && mid < 4.0);
    }
```

Run: `cargo test -p wc-sketches --lib radiance::systems::sim_params` → FAIL.

- [ ] **Step 2: Implement**

```rust
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
```

In `bake_radiance_sim`, after `out.emission_prob` is computed: advance `state.est_alive = expected_alive_step(state.est_alive, out.emission_prob, particle count as f32, dt, (LIFESPAN_MIN + LIFESPAN_MAX) * 0.5)` (particle count: thread the existing count the caller has — extend the fn signature if it is not already a parameter, updating call sites in `update_radiance_sim` + the screensaver baker). Then beat rising edge (same `BEAT_EDGE` constant, `state.prev_beat` bookkeeping exactly like `step_pulses`): on an edge, multiply `out.emission_prob` by `1.0 + settings.burst_scale * audio.beat_confidence * burst_boost(state.est_alive, count, settings.burst_boost_cap)` (clamp 0..=1 after) and add the outward kick through the existing lane: `out.burst_speed += BURST_SPEED * settings.burst_scale * audio.beat_confidence;` for that bake only. Settings: `burst_scale` (Dev, default 1.0), `burst_boost_cap` (Dev, default 4.0).

- [ ] **Step 3: Flare term in the kernel**

Inside Task 2's `edge_count > 0` block (after the repel), using exterior distance only:

```wgsl
        // Beat flare-wave: brighten as a wave radius passes this particle's
        // exterior distance. Strength is age-decayed CPU-side (also kills
        // the saturation-shell sync flash); the shell itself is suppressed
        // outright: beyond it every particle reads the same clamped d.
        let d_ext = max(d, 0.0);
        if (params.flare_gain > 0.0 && d_ext < FIELD_DIST_MAX_TEXELS * params.uv_to_world.y / f32(MASK_DIM) - params.flare_band_px) {
            var flare = 0.0;
            for (var w = 0u; w < 4u; w = w + 1u) {
                flare = flare + wave_term(params.wave_radius_a[w], params.wave_strength_a[w], d_ext);
                flare = flare + wave_term(params.wave_radius_b[w], params.wave_strength_b[w], d_ext);
            }
            glow = glow + params.flare_gain * flare * params.dt;
        }
```

with the helper above `main`:

```wgsl
fn wave_term(radius: f32, strength: f32, d_ext: f32) -> f32 {
    if (strength <= 0.0) {
        return 0.0;
    }
    let x = (d_ext - radius) / max(params.flare_band_px, 1.0);
    return strength * exp(-x * x);
}
```

Settings: `flare_gain` (Dev, default 3.0), `flare_band` (Dev, default 60.0 — `PULSE_WIDTH_PX`'s heritage) baked into the two params.

- [ ] **Step 4: Verify + commit**

`cargo nextest run -p wc-sketches` → PASS (two new tests green). Clippy clean. Synthetic-body `cargo rund` smoke with music playing (or the audio file input if configured): beats visibly pulse the medium outward and light traveling fronts through dense plumes.

```bash
git add -A crates/wc-sketches assets/shaders/radiance
git commit -m "feat(radiance): in-medium beat flare-wave + density-adaptive burst"
```

---

### Task 4: Motion glow through the impulse loop

**Files:**
- Modify: `assets/shaders/radiance/simulate.wgsl` (one term in the impulse loop), `crates/wc-sketches/src/radiance/settings.rs` (`motion_glow`), `crates/wc-sketches/src/radiance/systems/sim_params.rs` (bake it)

- [ ] **Step 1: Implement**

The limb impulses already carry position/radius/gain for exactly the fast-moving-limb case — the motion disturbance is one glow term inside the existing loop (no new plumbing; the spec's emission-side fallback is moot). In the impulse loop, after `accel` accumulates:

```wgsl
        // Motion disturbance: the same locally-weighted coupling that pushes
        // particles near a fast limb also lights them — the algae beat:
        // disturbance is luminous.
        glow = glow + params.motion_glow * imp.gain * w * params.dt;
```

(This requires the glow accumulation from Task 2 to be in scope where the impulse loop runs — move the `var glow = p.glow * params.glow_decay_baked;` line above the impulse loop if Task 2 placed it after.) Settings: `motion_glow` (Dev, default 0.8, "Motion glow") baked in `bake_radiance_sim`.

- [ ] **Step 2: Verify + commit**

`cargo nextest run -p wc-sketches` → PASS; clippy clean. Synthetic smoke: the phantom's moving limbs trail brightened particles. (The real phantom has zero motion weights for *emission* bias, but its impulses derive from landmark velocity — if the phantom drives no impulses, this smoke needs the live camera instead; note which was used.)

```bash
git add -A crates/wc-sketches assets/shaders/radiance
git commit -m "feat(radiance): impulse-coupled motion glow"
```

---

### Task 5: Verification battery

**Files:** none (fixes belong to their owning tasks)

- [ ] **Step 1: Full gates** (each its own invocation)

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --workspace -- -D warnings
cargo nextest run --workspace --all-features
cargo test --doc --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --document-private-items
cargo deny check
cargo xtask check-secrets
```

- [ ] **Step 2: Deterministic captures** (window foregrounded — all-black frames mean the environment, not the code)

`cargo xtask capture --list` — if a radiance scenario exists, run it and compare against baseline expecting deliberate differences (no contour rings; clear interior); update baselines per `tests/visual/CLAUDE.md`'s procedure with a note in the run log. If none exists, capture ad-hoc synthetic-body stills before/after (git stash the branch for "before") for the report.

- [ ] **Step 3: Replay pass on real footage** (the mask-noise risks are only observable here)

Extract frames per `tests/eval-media/README.md` from the two fast-motion clips (`Tiffany light whip...` and `tiffany double fan with trailing fabric...`), run each with `WAVECONDUCTOR_BODY_REPLAY=<framesdir>@30 WAVECONDUCTOR_START_SKETCH=radiance WAVECONDUCTOR_CONFIG_DIR=$(mktemp -d) cargo rund`, watching for: boundary particle vibration (repel jitter — risk 1), interior pop-through on fast sweeps followed by expulsion pops (latency — risk 2), double-flash shimmer on flares (risk 3). Report observations with judgment: tuning-level or design-level.

- [ ] **Step 4: GPU before/after**

`xctrace` perf-state A/B (the Dots-diagnosis method, `docs/runbooks/dots-explode-gpu-saturation.md`): same synthetic scene on the pre-Task-1 commit vs HEAD, report GPU active residency + max-clock share. Regression budget: the new kernel terms must not exceed the removed fullscreen pass's refund by more than ~5% absolute residency; if they do, flag for the live session rather than auto-tuning.

- [ ] **Step 5: Report**

Summarize: gates, captures, replay observations per risk, GPU numbers, and the open operator items (live tuning session with the ADVANCED toggle flipped; art acceptance).
