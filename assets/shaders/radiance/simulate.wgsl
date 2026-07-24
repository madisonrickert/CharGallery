// Radiance aura simulation — one workgroup per 64 particles.
//
// Reads SimParams from a uniform buffer at @group(0) @binding(0).
// Reads + writes Particles in a storage buffer at @group(0) @binding(1).
// Reads the silhouette edge list (CPU-extracted where the smoothed person
// mask crosses 0.5) at @group(0) @binding(2), and its index-parallel per-point
// motion weights (0..1, how fast the boundary moved there) at @group(0)
// @binding(3). The edge buffers are allocated at full MAX_EDGE_POINTS
// capacity; the CPU packs per-slot (start, count) ranges so indexing
// `start + hash % count` never leaves a slot's live prefix. Reads the packed
// signed silhouette distance field (byte plane, four texels per u32) at
// @group(0) @binding(4) — the repel force, contact glow, and flare-wave
// below, each gated on its own gain.
//
// Life cycle: a particle is DEAD when age >= lifespan (a zeroed buffer is all
// dead). Each frame a dead particle rolls a hash against emission_prob; on a
// win it picks a BODY SLOT from the fade-weighted emission CDF (slot_cdf), a
// hashed edge point inside that slot's edge range, and respawns there offset
// along the outward normal. A second roll against ejecta_prob decides whether
// this spawn is a fast "shooting" streak (onset-driven, high normal velocity,
// short life) or an ordinary flame particle. Alive particles advance under
// tongue-modulated buoyancy + limb impulses + drag, then are advected along a
// divergence-free curl-noise flow. There is no OOB teleport — a particle that
// drifts off-screen simply dies at end of life and respawns on a silhouette.
//
// CPU parity: RadianceSimParamsGpu / RadianceParticle / RadianceImpulse in
// crates/wc-sketches/src/radiance/compute/sim_params.rs mirror these structs
// field for field (offset_of! tests lock them); the mask-UV -> world mapping
// below must stay term-for-term identical to
// systems::sim_params::mask_uv_to_world.

struct Particle {
    position: vec2<f32>,
    velocity: vec2<f32>,
    age: f32,
    lifespan: f32,
    seed: f32,
    // Body slot index (0..4) stored as f32; the render shader rounds it back
    // to index the per-slot color array.
    slot: f32,
    // Accumulated bioluminescent glow (contact/flare/motion terms), decayed
    // per frame; render.wgsl reads it as a brightness multiplier (1 + glow).
    glow: f32,
    _pad: f32,
};

// Plan B contract shape: mask-UV position (0..1, y down) + outward unit
// normal in the same space.
struct EdgePoint {
    pos: vec2<f32>,
    normal: vec2<f32>,
};

struct Impulse {
    position: vec2<f32>,
    velocity: vec2<f32>,
    radius: f32,
    gain: f32,
    _pad: vec2<f32>,
};

const MAX_IMPULSES: u32 = 8u;

struct SimParams {
    dt: f32,
    time: f32,
    emission_prob: f32,
    edge_count: u32,
    particle_count: u32,
    spawn_offset: f32,
    spawn_speed: f32,
    burst_speed: f32,
    buoyancy: f32,
    flow_strength: f32,
    curl_scale: f32,
    curl_octaves: u32,
    drag_baked: f32,
    lifespan_min: f32,
    lifespan_max: f32,
    mirror: u32,
    uv_to_world: vec2<f32>,
    impulse_count: u32,
    frame: u32,
    // Per-slot edge ranges + fade-weighted emission CDF (multi-body spawn
    // apportioning; see the CPU baker).
    slot_start: vec4<u32>,
    slot_count: vec4<u32>,
    slot_cdf: vec4<f32>,
    // Onset-driven shooting-spark layer + flame-tongue buoyancy noise.
    ejecta_prob: f32,
    ejecta_speed: f32,
    tongue_amp: f32,
    tongue_freq: f32,
    impulses: array<Impulse, MAX_IMPULSES>,
    // Motion-emission bias 0..1: 0 = uniform edge pick, 1 = the respawn
    // rejection sampler below accepts almost exclusively moving-boundary
    // points. First tail scalar at byte offset 400.
    edge_motion_bias: f32,
    // Radiance-rework tail: repel / contact-glow / flare / motion gains plus
    // the beat flare-wave lanes, layout-parity with the Rust
    // `RadianceSimParamsGpu`. WGSL uniform scalar arrays have 16-byte
    // stride, so the Rust `[f32; 8]` wave lanes mirror as two `vec4<f32>`
    // each. After the wave lanes: the 2026-07-24 calibration lanes
    // (curl_evolve at 496, impulse_coupling at 500) and a two-lane tail
    // pad. Struct total 512 bytes.
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
    // Time-evolution multiplier on the curl field's phase drift (see
    // curl_flow: the drift clock is params.time * curl_evolve).
    curl_evolve: f32,
    // Limb-impulse force coupling per second (was the IMPULSE_COUPLING
    // const, 6.0): how fast a particle inside an impulse radius converges
    // on the limb's velocity without hard-snapping to it.
    impulse_coupling: f32,
    _pad_tail: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: SimParams;
@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(2) var<storage, read> edges: array<EdgePoint>;
@group(0) @binding(3) var<storage, read> edge_motion: array<f32>;
// Packed signed distance field: one byte per 256^2 mask texel, four texels
// per u32 word (see field_signed_px). 128 = boundary, above = outside the
// body, below = inside. Uploaded generation-gated by field_upload.rs.
@group(0) @binding(4) var<storage, read> field: array<u32>;

// Mask square dimension (CPU MASK_SIZE) and the signed encoding's full-range
// distance (CPU DIST_MAX_TEXELS): byte 255 = +160 texels, byte 0 ~= -160.
const MASK_DIM: i32 = 256;
const FIELD_DIST_MAX_TEXELS: f32 = 160.0;

// (The impulse coupling gain lives in params.impulse_coupling — the
// operator's "Motion coupling" setting — since 2026-07-24.)
// Ejecta lifespan multiplier: shooting sparks die young, so the streaks stay
// crisp instead of loitering as slow embers far from the body.
const EJECTA_LIFE_MUL: f32 = 0.3;

// PCG-style integer hash (Jarzynski & Olano) — cheap, well-distributed.
fn pcg(v: u32) -> u32 {
    var state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn hash2(a: u32, b: u32) -> u32 {
    return pcg(a ^ pcg(b));
}

fn rand01(h: u32) -> f32 {
    // Top 24 bits -> [0, 1). Keeps full float precision.
    return f32(h >> 8u) * (1.0 / 16777216.0);
}

// Divergence-free curl-noise flow at a world-space point: the 2D curl of a
// scalar stream function psi built from sine octaves. Curl of a scalar field
// has zero divergence by construction, so the flow swirls without sources or
// sinks (the shared particle engine's turbulence, generalized to a
// param-driven 1..3 octave sum). Per octave the frequency doubles and the
// weight halves; each octave drifts along its own incommensurate direction so
// the field never visibly loops.
// Per-octave time-drift directions for curl_flow (incommensurate, matching
// the shared engine's 0.13/0.11 family). Module-scope const rather than a
// function-scope `var`: the old local array was re-initialized by every
// alive-particle invocation, every frame (3 vec2 writes × up to 300k
// particles), purely to satisfy naga's historical rule that dynamically
// indexed arrays be locals — current naga accepts indexing a const array.
const CURL_DRIFTS = array<vec2<f32>, 3>(
    vec2<f32>(0.13, -0.11),
    vec2<f32>(-0.17, 0.15),
    vec2<f32>(0.07, 0.19),
);

fn curl_flow(pos: vec2<f32>, scale: f32, t: f32, octaves: u32) -> vec2<f32> {
    var flow = vec2<f32>(0.0);
    var freq = 1.0;
    var amp = 1.0;
    var total = 0.0;
    let n = clamp(octaves, 1u, 3u);
    for (var i = 0u; i < n; i = i + 1u) {
        let a = pos.x * scale * freq + CURL_DRIFTS[i].x * t;
        let b = pos.y * scale * freq + CURL_DRIFTS[i].y * t;
        // psi = sin(a)cos(b); curl = (d psi/dy, -d psi/dx). The chain-rule
        // scale*freq factor is folded into flow_strength by the caller.
        let dpsi_dx = cos(a) * cos(b);
        let dpsi_dy = -sin(a) * sin(b);
        flow = flow + vec2<f32>(dpsi_dy, -dpsi_dx) * amp;
        total = total + amp;
        freq = freq * 2.0;
        amp = amp * 0.5;
    }
    return flow / max(total, 1e-4);
}

// Mask-UV (0..1, y down) -> world px (origin center, y up), with the mirror
// flip. MUST stay identical to systems::sim_params::mask_uv_to_world.
fn mask_uv_to_world(uv: vec2<f32>) -> vec2<f32> {
    var u = uv.x;
    if (params.mirror == 1u) {
        u = 1.0 - u;
    }
    return vec2<f32>(
        (u - 0.5) * params.uv_to_world.x,
        (0.5 - uv.y) * params.uv_to_world.y,
    );
}

// Mask-UV direction -> world direction (mirror sign on x, y flip), normalized.
fn mask_dir_to_world(dir: vec2<f32>) -> vec2<f32> {
    var sx = params.uv_to_world.x;
    if (params.mirror == 1u) {
        sx = -sx;
    }
    let d = vec2<f32>(dir.x * sx, -dir.y * params.uv_to_world.y);
    let len = length(d);
    if (len < 1e-6) {
        return vec2<f32>(0.0, 1.0);
    }
    return d / len;
}

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

// Pick the spawn slot from the fade-weighted emission CDF: the first entry
// the roll falls under. Returns 4 (invalid) when every weight is zero, which
// the caller treats as "no live body — stay dead".
fn pick_slot(r: f32) -> u32 {
    for (var i = 0u; i < 4u; i = i + 1u) {
        if (r < params.slot_cdf[i]) {
            return i;
        }
    }
    return 4u;
}

// One beat wave's Gaussian flare contribution at a particle's exterior
// distance: peak strength where the wave radius crosses the particle,
// falling off over the flare band. Dead slots (strength 0) cost one compare.
fn wave_term(radius: f32, strength: f32, d_ext: f32) -> f32 {
    if (strength <= 0.0) {
        return 0.0;
    }
    let x = (d_ext - radius) / max(params.flare_band_px, 1.0);
    return strength * exp(-x * x);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    let count = min(arrayLength(&particles), params.particle_count);
    if (idx >= count) {
        return;
    }
    var p = particles[idx];

    // --- Dead: roll for an edge respawn --------------------------------
    if (p.age >= p.lifespan) {
        if (params.edge_count == 0u) {
            return; // no silhouette this frame; stay dead, write nothing
        }
        // Salt the roll with the CPU frame counter so a losing particle
        // re-rolls fresh every frame (emission_prob is already rate*dt baked).
        let frame = params.frame;
        if (rand01(hash2(idx, frame)) >= params.emission_prob) {
            return;
        }
        // Fade-weighted slot pick: the shared budget is apportioned across
        // the live bodies (density stays constant as dancers come and go).
        let slot = pick_slot(rand01(hash2(idx ^ 0x9e3779b9u, frame)));
        if (slot >= 4u || params.slot_count[slot] == 0u) {
            return;
        }
        var e_idx = params.slot_start[slot]
            + (hash2(idx * 2654435769u, frame) % params.slot_count[slot]);
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
        let e = edges[min(e_idx, params.edge_count - 1u)];
        let n = mask_dir_to_world(e.normal);
        var life = mix(
            params.lifespan_min,
            params.lifespan_max,
            rand01(hash2(idx, frame ^ 2654435769u)),
        );
        // Ejecta roll: on onsets a fraction of spawns become shooting sparks
        // — fast along the normal, short-lived, streaked by the velocity-
        // stretch in render.wgsl.
        var speed = params.spawn_speed + params.burst_speed;
        if (rand01(hash2(idx ^ 0x51ed270bu, frame)) < params.ejecta_prob) {
            speed = params.ejecta_speed
                * (0.7 + 0.6 * rand01(hash2(idx, frame ^ 0x51ed270bu)));
            life = life * EJECTA_LIFE_MUL;
        }
        p.position = mask_uv_to_world(e.pos) + n * params.spawn_offset;
        p.velocity = n * speed;
        p.age = 0.0;
        p.glow = 0.0;
        p.lifespan = life;
        p.seed = rand01(hash2(idx, 2246822519u));
        p.slot = f32(slot);
        particles[idx] = p;
        return;
    }

    // --- Alive: forces -> drag -> integrate -> curl advection ----------
    p.age = p.age + params.dt;

    // Buoyancy: upward acceleration modulated by two incommensurate sines
    // along world x (plus a weak y term) — locally stronger columns of lift
    // form the licking tongues of a flame instead of a uniform rising sheet.
    // The sines each span ±1, so the sum spans ±1 after the 0.5 weights and
    // the multiplier stays positive for tongue_amp <= 1.
    let tongue = 1.0 + params.tongue_amp
        * (0.5 * sin(p.position.x * params.tongue_freq + params.time * 1.7)
            + 0.5 * sin(p.position.x * params.tongue_freq * 2.33
                - params.time * 1.1
                + p.position.y * params.tongue_freq * 0.5));
    var accel = vec2<f32>(0.0, params.buoyancy * tongue);

    // Glow accumulator: decay last frame's residue first (framerate-
    // independent, baked CPU-side like drag), then let this frame's motion /
    // contact / flare terms add onto it.
    var glow = p.glow * params.glow_decay_baked;

    // Limb impulses: locally-weighted coupling toward each limb's velocity,
    // fading to zero by the slot radius — a fast limb sheds a burst.
    let live_impulses = min(params.impulse_count, MAX_IMPULSES);
    for (var i = 0u; i < live_impulses; i = i + 1u) {
        let imp = params.impulses[i];
        if (imp.gain <= 0.0) {
            continue;
        }
        let dist = length(p.position - imp.position);
        let w = 1.0 - smoothstep(0.0, max(imp.radius, 1.0), dist);
        accel = accel + imp.velocity * (imp.gain * w * params.impulse_coupling);
        // Motion disturbance: the same locally-weighted coupling that pushes
        // particles near a fast limb also lights them — the algae beat:
        // disturbance is luminous.
        glow = glow + params.motion_glow * imp.gain * w * params.dt;
    }

    // Silhouette field couplings (repel force, contact glow, beat flare):
    // signed field, positive outside. Each term gates on its OWN gain so any
    // one lane can be A/B'd alone; the texel lookup + signed distance are
    // shared, computed once when any lane is live.
    let field_live = params.repel_strength > 0.0
        || params.contact_glow > 0.0
        || params.flare_gain > 0.0;
    if (params.edge_count > 0u && field_live) {
        let t = world_to_texel(p.position);
        let d = field_signed_px(t);
        // Repel + contact glow share the boundary falloff and the central-
        // difference gradient (worth its four extra field taps only when one
        // of the two is live); inside the body the interior chamfer keeps
        // the ascent direction pointing at the nearest boundary, so overlap
        // corrects smoothly instead of trapping.
        if ((params.repel_strength > 0.0 || params.contact_glow > 0.0)
            && d < params.repel_radius_px) {
            let gx = field_signed_px(t + vec2<i32>(1, 0)) - field_signed_px(t - vec2<i32>(1, 0));
            // Mask y is down, world y is up: flip the y difference.
            let gy = field_signed_px(t - vec2<i32>(0, 1)) - field_signed_px(t + vec2<i32>(0, 1));
            var g = vec2<f32>(gx, gy);
            // The differences live in texel space but g is applied as a
            // world-space direction: world_to_texel flips u under mirror, so
            // a texel-space +x step is a world-space -x step — negate g.x,
            // exactly as mask_dir_to_world negates its x scale.
            if (params.mirror == 1u) {
                g.x = -g.x;
            }
            let glen = length(g);
            if (glen > 1e-4) {
                // 1 at the boundary (and everywhere inside), 0 at the radius.
                let falloff = 1.0 - clamp(d, 0.0, params.repel_radius_px) / params.repel_radius_px;
                accel = accel + (g / glen) * (params.repel_strength * falloff);
                glow = glow + params.contact_glow * falloff * params.dt;
            }
        }
        // Beat flare-wave: brighten as a wave radius passes this particle's
        // exterior distance. Strength is age-decayed CPU-side (also kills
        // the saturation-shell sync flash); the shell itself is suppressed
        // outright: beyond it every particle reads the same clamped d.
        if (params.flare_gain > 0.0) {
            // Pillarbox extension: with fit-to-height on a wide window the
            // mask square (|world x| <= uv_to_world.x / 2) ends before the
            // window does, and the clamped texel lookup freezes d at the
            // square's vertical edge — the flare front would halt on an
            // invisible line there. From beyond that edge the nearest
            // silhouette point sits overshoot px away horizontally and
            // edge_d px away within the edge column, at right angles, so
            // the exterior distance extends in quadrature (the retired
            // overlay shader's over_texels handling, ported into the
            // kernel). overshoot is 0 everywhere inside the square, leaving
            // the plain clamped distance.
            let overshoot = max(0.0, abs(p.position.x) - 0.5 * params.uv_to_world.x);
            let edge_d = max(d, 0.0);
            let d_ext = sqrt(edge_d * edge_d + overshoot * overshoot);
            if (d_ext < FIELD_DIST_MAX_TEXELS * params.uv_to_world.y / f32(MASK_DIM) - params.flare_band_px) {
                var flare = 0.0;
                for (var w = 0u; w < 4u; w = w + 1u) {
                    flare = flare + wave_term(params.wave_radius_a[w], params.wave_strength_a[w], d_ext);
                    flare = flare + wave_term(params.wave_radius_b[w], params.wave_strength_b[w], d_ext);
                }
                glow = glow + params.flare_gain * flare * params.dt;
            }
        }
    }
    p.glow = min(glow, 4.0);

    p.velocity = p.velocity + accel * params.dt;
    // Framerate-independent drag, baked CPU-side as pow(retention, dt).
    p.velocity = p.velocity * params.drag_baked;
    p.position = p.position + p.velocity * params.dt;

    // Curl advection: position (not force) so the drift speed is exactly
    // flow_strength px/s regardless of the drag regime, and the
    // divergence-free field can never collapse the aura inward.
    if (params.flow_strength > 0.0) {
        // The drift clock is scaled by curl_evolve so the vortex pattern
        // itself churns at a tunable rate (1.0 = the original slow drift).
        let turb = curl_flow(
            p.position,
            params.curl_scale,
            params.time * params.curl_evolve,
            params.curl_octaves,
        );
        p.position = p.position + turb * params.flow_strength * params.dt;
    }

    particles[idx] = p;
}
