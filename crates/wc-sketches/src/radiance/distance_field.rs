//! Silhouette distance field: a per-body-frame 256² **signed** chamfer
//! distance transform of the person mask, feeding the particle compute
//! kernel.
//!
//! The kernel needs "signed distance from the silhouette" at every particle:
//! positive outside the body (the repel falloff and the beat flare-wave's
//! travel coordinate), negative inside (so the ascent direction — the
//! +gradient — leads out of the body everywhere and overlap corrects
//! smoothly instead of trapping). This module computes both half-transforms
//! on the CPU with the classic two-pass 3×3 chamfer relaxation over the same
//! 256² mask the silhouette fill samples — one seeded from the body, one
//! from its complement — and packs them into a single byte plane (`128` =
//! boundary; see [`signed_chamfer`]) published through
//! [`RadianceDistanceField::signed`]. The render world copies it
//! generation-gated into the compute pipeline's persistent field storage
//! buffer (`compute::field_upload`, the `edge_upload` shape). 256² is 65k
//! texels; the relaxation passes cost a fraction of a millisecond and run
//! only when [`SilhouetteEdges::generation`] advances (~30 Hz body frames,
//! not render frames).
//!
//! Historically this was an unsigned field published as an `R8Unorm` texture
//! for a fullscreen beat-contour overlay shader; that overlay was retired
//! and its energy moved into the particle world.
//!
//! Hot-path posture: the scratch and output buffers are allocated once at
//! sketch spawn; [`update_distance_field`] mutates them in place and
//! allocates nothing.

use bevy::image::Image;
use bevy::prelude::*;
use wc_core::input::body::{MaskTexture, SilhouetteEdges, MASK_CHANNELS, MASK_SIZE};

/// Distance (in mask texels) that maps to the full byte range on either side
/// of the signed encoding's `128` boundary bias: exterior distances encode
/// as `128 + d/DIST_MAX_TEXELS·127`, interior as `128 − d/DIST_MAX_TEXELS·127`
/// (one scale both sides). 160 texels is ~0.63 of the mask square — at a
/// 1080-px-tall window that is ~675 px of wave travel before the field
/// saturates, with ~2.6 texels (≈5–11 px world) of quantization per byte
/// step — inside the flare band's and repel radius's resolution needs.
pub const DIST_MAX_TEXELS: f32 = 160.0;

/// Mask coverage threshold for "inside the body" (the body-tracking
/// contract's fixed edge threshold).
pub const MASK_INSIDE_THRESHOLD: u8 = 128;

/// Chamfer weights ×12 as integers (3-4 chamfer: orthogonal 3, diagonal 4 —
/// the standard integer approximation of 1/√2 stepping, error < 6%).
const ORTHO_COST: u32 = 3;
/// See [`ORTHO_COST`].
const DIAG_COST: u32 = 4;
/// "Infinite" seed for texels with no body anywhere near.
const FAR: u32 = u32::MAX / 2;

/// Owns the packed signed-field bytes + chamfer scratch. Inserted at
/// Radiance spawn, removed at exit. A plain CPU resource: the GPU copy lives
/// in the compute pipeline's persistent field buffer, refilled
/// generation-gated by `compute::field_upload` (stable `BufferId` — the
/// bind-group cache never keys or invalidates on the field).
#[derive(Resource)]
pub struct RadianceDistanceField {
    /// The packed signed field (`MASK_SIZE²` bytes, one per texel; `128` =
    /// boundary, above = outside the body, below = inside — see
    /// [`signed_chamfer`]).
    pub signed: Vec<u8>,
    /// Recompute counter consumed by the render-world extract
    /// (`compute::field_upload::extract_distance_field`). Starts at 0 and
    /// bumps once per recompute; the extract's `u64::MAX` sentinel is
    /// distinct, so the first computed field always uploads.
    pub generation: u64,
    /// Preallocated exterior-chamfer scratch (one `u32` per texel; body = 0
    /// seed).
    scratch: Vec<u32>,
    /// Preallocated interior-chamfer scratch (the complement seed).
    scratch_in: Vec<u32>,
    /// Last [`SilhouetteEdges::generation`] the field was computed for
    /// (recompute gate; `u64::MAX` = never computed).
    last_generation: u64,
}

impl RadianceDistanceField {
    /// Zeroed field + scratch; the first body frame computes the real field.
    #[must_use]
    pub fn new() -> Self {
        Self {
            signed: vec![0; MASK_SIZE * MASK_SIZE],
            generation: 0,
            scratch: vec![0; MASK_SIZE * MASK_SIZE],
            scratch_in: vec![0; MASK_SIZE * MASK_SIZE],
            last_generation: u64::MAX,
        }
    }
}

impl Default for RadianceDistanceField {
    fn default() -> Self {
        Self::new()
    }
}

/// Combine two seeded chamfer scratches into the packed signed byte field:
/// relax both (exterior scratch: body = 0; interior scratch: complement =
/// 0), then encode per texel around the `128` boundary bias — exterior
/// distances rise above 128, interior distances fall below it, one
/// [`DIST_MAX_TEXELS`] scale on both sides. Pure over the buffers for
/// testability; the system seeds directly from the RGBA mask
/// ([`update_distance_field`]), the test-facing [`signed_chamfer_from_mask`]
/// seeds from a single-channel mask. All buffers must be `MASK_SIZE²` long.
pub fn signed_chamfer(scratch_out: &mut [u32], scratch_in: &mut [u32], out: &mut [u8]) {
    debug_assert_eq!(scratch_out.len(), MASK_SIZE * MASK_SIZE);
    debug_assert_eq!(scratch_in.len(), MASK_SIZE * MASK_SIZE);
    debug_assert_eq!(out.len(), MASK_SIZE * MASK_SIZE);
    chamfer_relax(scratch_out);
    chamfer_relax(scratch_in);
    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "d/3 <= ~2*MASK_SIZE texels, exact in f32; clamped \
                  into u8 range before the cast"
    )]
    for ((&s_out, &s_in), o) in scratch_out
        .iter()
        .zip(scratch_in.iter())
        .zip(out.iter_mut())
    {
        let d_out = s_out as f32 / ORTHO_COST as f32; // texels
        let d_in = s_in as f32 / ORTHO_COST as f32;
        // Body texels (exterior distance 0) carry the negated interior
        // distance; everything else carries the exterior distance.
        let signed = if d_out > 0.0 { d_out } else { -d_in };
        *o = (128.0 + signed / DIST_MAX_TEXELS * 127.0).clamp(0.0, 255.0) as u8;
    }
}

/// Seed both scratches from a **single-channel** mask (body =
/// `>= MASK_INSIDE_THRESHOLD`, interior scratch inverted) and run
/// [`signed_chamfer`]. Test-facing pure wrapper; all four buffers must be
/// `MASK_SIZE²` long.
pub fn signed_chamfer_from_mask(
    mask: &[u8],
    scratch_out: &mut [u32],
    scratch_in: &mut [u32],
    out: &mut [u8],
) {
    debug_assert_eq!(mask.len(), MASK_SIZE * MASK_SIZE);
    for ((s_out, s_in), &m) in scratch_out
        .iter_mut()
        .zip(scratch_in.iter_mut())
        .zip(mask.iter())
    {
        let inside = m >= MASK_INSIDE_THRESHOLD;
        *s_out = if inside { 0 } else { FAR };
        *s_in = if inside { FAR } else { 0 };
    }
    signed_chamfer(scratch_out, scratch_in, out);
}

/// Two-pass 3-4 chamfer distance transform: `out[i]` = distance from texel
/// `i` to the nearest body texel (`mask >= MASK_INSIDE_THRESHOLD`), in
/// units of [`DIST_MAX_TEXELS`] mapped to `0..=255`. Body-interior texels
/// are 0. The legacy **unsigned** encoding, kept for its tests and any
/// exterior-only consumer; the kernel consumes the signed path above. Pure
/// over the buffers for testability; `mask` here is **single-channel**
/// (`MASK_SIZE²` bytes). `scratch`/`out` must be `MASK_SIZE²` long.
pub fn chamfer_distance(mask: &[u8], scratch: &mut [u32], out: &mut [u8]) {
    debug_assert_eq!(mask.len(), MASK_SIZE * MASK_SIZE);
    debug_assert_eq!(scratch.len(), MASK_SIZE * MASK_SIZE);
    debug_assert_eq!(out.len(), MASK_SIZE * MASK_SIZE);

    // Seed: body = 0, everything else = far.
    for (s, &m) in scratch.iter_mut().zip(mask.iter()) {
        *s = if m >= MASK_INSIDE_THRESHOLD { 0 } else { FAR };
    }
    chamfer_from_seeded(scratch, out);
}

/// The two chamfer relaxation passes plus the legacy unsigned normalization
/// over an already-seeded scratch (see [`chamfer_distance`], which seeds and
/// then calls this). The signed path shares the relaxation via the private
/// `chamfer_relax` helper and applies its biased encoding instead.
pub fn chamfer_from_seeded(scratch: &mut [u32], out: &mut [u8]) {
    chamfer_relax(scratch);
    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "d/3 <= ~2*MASK_SIZE texels, exact in f32; clamped \
                  into u8 range before the cast"
    )]
    for (&d, o) in scratch.iter().zip(out.iter_mut()) {
        // Body texels (d == 0) map to 0 through the same formula.
        let texels = d as f32 / ORTHO_COST as f32;
        *o = (texels / DIST_MAX_TEXELS * 255.0).clamp(0.0, 255.0) as u8;
    }
}

/// `Update` (gated `in_state(AppState::Radiance)`, before the beat-wave
/// clock): recompute the signed field when a new body frame has arrived
/// (generation-gated, like the edge upload). Skips cleanly when any surface
/// is missing (headless tests, feature-reduced harnesses).
pub fn update_distance_field(
    edges: Option<Res<'_, SilhouetteEdges>>,
    mask: Option<Res<'_, MaskTexture>>,
    field: Option<ResMut<'_, RadianceDistanceField>>,
    images: Res<'_, Assets<Image>>,
) {
    let (Some(edges), Some(mask), Some(mut field)) = (edges, mask, field) else {
        return;
    };
    if edges.generation == field.last_generation {
        return;
    }
    field.last_generation = edges.generation;

    let Some(mask_data) = images.get(&mask.0).and_then(|m| m.data.as_ref()) else {
        return;
    };
    // With the output now a plain CPU byte plane (no second `Assets<Image>`
    // borrow), the old borrow dance is gone: split-borrow the resource's
    // fields directly and work fully in place.
    let field = &mut *field;
    // The mask is RGBA (channel i = body slot i): seed "inside" from the
    // UNION of all slot channels, so the field wraps every tracked dancer's
    // silhouette, not just the primary's. The interior scratch takes the
    // complement seed in the same pass.
    for ((s_out, s_in), texel) in field
        .scratch
        .iter_mut()
        .zip(field.scratch_in.iter_mut())
        .zip(mask_data.chunks_exact(MASK_CHANNELS))
    {
        let inside = texel.iter().any(|&c| c >= MASK_INSIDE_THRESHOLD);
        *s_out = if inside { 0 } else { FAR };
        *s_in = if inside { FAR } else { 0 };
    }
    signed_chamfer(&mut field.scratch, &mut field.scratch_in, &mut field.signed);
    field.generation = field.generation.wrapping_add(1);
}

/// The forward + backward 3-4 chamfer relaxation passes in place, leaving
/// raw chamfer units ([`ORTHO_COST`] per orthogonal texel step) in
/// `scratch`. Seeded texels (0) are the sources; everything else relaxes
/// toward its cheapest seeded neighbor.
fn chamfer_relax(scratch: &mut [u32]) {
    // Forward pass.
    for y in 0..MASK_SIZE {
        for x in 0..MASK_SIZE {
            let i = y * MASK_SIZE + x;
            let mut d = scratch[i];
            if d == 0 {
                continue;
            }
            if x > 0 {
                d = d.min(scratch[i - 1] + ORTHO_COST);
            }
            if y > 0 {
                let up = i - MASK_SIZE;
                d = d.min(scratch[up] + ORTHO_COST);
                if x > 0 {
                    d = d.min(scratch[up - 1] + DIAG_COST);
                }
                if x + 1 < MASK_SIZE {
                    d = d.min(scratch[up + 1] + DIAG_COST);
                }
            }
            scratch[i] = d;
        }
    }
    // Backward pass.
    for y in (0..MASK_SIZE).rev() {
        for x in (0..MASK_SIZE).rev() {
            let i = y * MASK_SIZE + x;
            let mut d = scratch[i];
            if d == 0 {
                continue;
            }
            if x + 1 < MASK_SIZE {
                d = d.min(scratch[i + 1] + ORTHO_COST);
            }
            if y + 1 < MASK_SIZE {
                let down = i + MASK_SIZE;
                d = d.min(scratch[down] + ORTHO_COST);
                if x > 0 {
                    d = d.min(scratch[down - 1] + DIAG_COST);
                }
                if x + 1 < MASK_SIZE {
                    d = d.min(scratch[down + 1] + DIAG_COST);
                }
            }
            scratch[i] = d;
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test assertions")]
mod tests {
    use super::*;

    /// A single body texel: distance grows outward and is exact along the
    /// axes (3-4 chamfer, ortho steps).
    #[test]
    fn chamfer_distances_grow_from_a_point() {
        let mut mask = vec![0_u8; MASK_SIZE * MASK_SIZE];
        let cx = 128;
        let cy = 128;
        mask[cy * MASK_SIZE + cx] = 255;
        let mut scratch = vec![0_u32; MASK_SIZE * MASK_SIZE];
        let mut out = vec![0_u8; MASK_SIZE * MASK_SIZE];
        chamfer_distance(&mask, &mut scratch, &mut out);

        assert_eq!(out[cy * MASK_SIZE + cx], 0, "body texel is distance 0");
        // 16 texels straight right: 16/160 of the range.
        let d16 = f32::from(out[cy * MASK_SIZE + cx + 16]);
        let expect = 16.0 / DIST_MAX_TEXELS * 255.0;
        assert!(
            (d16 - expect).abs() <= 2.0,
            "axis distance ~exact: {d16} vs {expect}"
        );
        // Distance is monotone along the axis.
        let d32 = f32::from(out[cy * MASK_SIZE + cx + 32]);
        assert!(d32 > d16, "farther texel reads farther");
        // Diagonal ~sqrt(2) ratio (4/3 chamfer approximation).
        let ddiag = f32::from(out[(cy + 16) * MASK_SIZE + cx + 16]);
        let ratio = ddiag / d16;
        assert!(
            (1.25..=1.45).contains(&ratio),
            "diagonal/ortho ratio ~1.33: {ratio}"
        );
    }

    /// An empty mask saturates the whole field at 255 (no body anywhere).
    #[test]
    fn empty_mask_saturates() {
        let mask = vec![0_u8; MASK_SIZE * MASK_SIZE];
        let mut scratch = vec![0_u32; MASK_SIZE * MASK_SIZE];
        let mut out = vec![0_u8; MASK_SIZE * MASK_SIZE];
        chamfer_distance(&mask, &mut scratch, &mut out);
        assert!(out.iter().all(|&d| d == 255), "no body -> field saturated");
    }

    /// A filled half-plane: distance equals the row gap to the boundary.
    #[test]
    fn half_plane_distance_is_row_gap() {
        let mut mask = vec![0_u8; MASK_SIZE * MASK_SIZE];
        for y in 0..128 {
            for x in 0..MASK_SIZE {
                mask[y * MASK_SIZE + x] = 255;
            }
        }
        let mut scratch = vec![0_u32; MASK_SIZE * MASK_SIZE];
        let mut out = vec![0_u8; MASK_SIZE * MASK_SIZE];
        chamfer_distance(&mask, &mut scratch, &mut out);
        let d10 = f32::from(out[(127 + 10) * MASK_SIZE + 64]);
        let expect = 10.0 / DIST_MAX_TEXELS * 255.0;
        assert!((d10 - expect).abs() <= 2.0, "{d10} vs {expect}");
    }

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
        assert!(
            (outside10 - expect_out).abs() <= 2.0,
            "{outside10} vs {expect_out}"
        );
        let inside10 = f32::from(out[(127 - 10) * MASK_SIZE + 64]);
        let expect_in = 128.0 - 10.0 / DIST_MAX_TEXELS * 127.0;
        assert!(
            (inside10 - expect_in).abs() <= 2.0,
            "{inside10} vs {expect_in}"
        );
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
}
