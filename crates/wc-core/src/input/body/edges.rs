//! Silhouette edge extraction: scan the temporally-blended mask for
//! [`EDGE_THRESHOLD`] crossings between neighbouring texels and emit up to
//! [`MAX_EDGE_POINTS`] `(position, outward normal)` pairs.
//!
//! Runs on the worker (a single 256² pass, negligible next to inference).
//! The output is Plan C's particle-emission surface (uploaded as a storage
//! buffer) and doubles as the silhouette rim source. The caller supplies a
//! buffer with capacity [`MAX_EDGE_POINTS`]; extraction clear-refills it and
//! clamps at capacity, so it never allocates (worker hot-path rule).

use bevy::math::Vec2;

use super::{EdgePoint, MASK_SIZE, MAX_EDGE_POINTS};

/// Iso-level at which the mask boundary is traced.
pub const EDGE_THRESHOLD: f32 = 0.5;

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
    let x = texel_index(pos.x);
    let y = texel_index(pos.y);
    (motion[y * MASK_SIZE + x] / EDGE_MOTION_FULL).clamp(0.0, 1.0)
}

/// Extract silhouette edge points from a `MASK_SIZE`² smoothed mask
/// (row-major, values in `[0, 1]`) into `out`, sampling the parallel
/// `MASK_SIZE`² per-texel `motion` field into `weights` (both cleared first;
/// capacity must be ≥ [`MAX_EDGE_POINTS`], which the pooled payload and
/// `SilhouetteEdges` guarantee by construction). `weights` stays index-parallel
/// with `out`. Single-mask convenience over [`extract_edges_append`].
pub fn extract_edges(
    mask: &[f32],
    motion: &[f32],
    out: &mut Vec<EdgePoint>,
    weights: &mut Vec<f32>,
) {
    out.clear();
    weights.clear();
    extract_edges_append(mask, motion, out, weights);
}

/// Append one mask's edge points to `out` **without clearing it**, stopping
/// at the shared [`MAX_EDGE_POINTS`] capacity; returns how many points this
/// call appended. The multi-body pipeline calls this once per slot in
/// ascending slot order (clearing `out` before slot 0), producing the
/// slot-partitioned list `SilhouetteEdges::slot_counts` describes; earlier
/// slots fill first when the shared cap is hit.
///
/// Two passes in deterministic scan order: horizontal crossings (between
/// x and x+1) then vertical (between y and y+1). Each crossing interpolates
/// the sub-texel position and takes the outward normal from the mask
/// gradient (central differences, clamped at borders): inside > threshold >
/// outside, so the outward direction is −gradient. Degenerate zero-gradient
/// crossings are skipped rather than given a fake normal. Never allocates
/// past the caller's [`MAX_EDGE_POINTS`] capacity (worker hot-path rule).
///
/// The parallel `motion` field (`MASK_SIZE`² per-texel frame deltas) is sampled
/// at each emitted point via [`motion_weight_at`] into `weights`, which the
/// caller must supply with the same capacity as `out`; `weights` stays
/// index-parallel with the points this call appends.
pub fn extract_edges_append(
    mask: &[f32],
    motion: &[f32],
    out: &mut Vec<EdgePoint>,
    weights: &mut Vec<f32>,
) -> usize {
    let start = out.len();
    debug_assert_eq!(mask.len(), MASK_SIZE * MASK_SIZE);
    let n = MASK_SIZE;
    let nf = cellf(n);
    // Horizontal crossings: between (x, y) and (x+1, y).
    for y in 0..n {
        for x in 0..n - 1 {
            if out.len() == MAX_EDGE_POINTS {
                return out.len() - start;
            }
            let a = mask[y * n + x];
            let b = mask[y * n + x + 1];
            if !crosses(a, b) {
                continue;
            }
            let t = (EDGE_THRESHOLD - a) / (b - a);
            let pos = Vec2::new((cellf(x) + 0.5 + t) / nf, (cellf(y) + 0.5) / nf);
            let sample_x = if t < 0.5 { x } else { x + 1 };
            if let Some(normal) = outward_normal(mask, sample_x, y) {
                out.push(EdgePoint { pos, normal });
                weights.push(motion_weight_at(motion, pos));
            }
        }
    }
    // Vertical crossings: between (x, y) and (x, y+1).
    for y in 0..n - 1 {
        for x in 0..n {
            if out.len() == MAX_EDGE_POINTS {
                return out.len() - start;
            }
            let a = mask[y * n + x];
            let b = mask[(y + 1) * n + x];
            if !crosses(a, b) {
                continue;
            }
            let t = (EDGE_THRESHOLD - a) / (b - a);
            let pos = Vec2::new((cellf(x) + 0.5) / nf, (cellf(y) + 0.5 + t) / nf);
            let sample_y = if t < 0.5 { y } else { y + 1 };
            if let Some(normal) = outward_normal(mask, x, sample_y) {
                out.push(EdgePoint { pos, normal });
                weights.push(motion_weight_at(motion, pos));
            }
        }
    }
    out.len() - start
}

/// Whether the mask value crosses [`EDGE_THRESHOLD`] between two texels.
/// Strict inequality: a texel exactly at the threshold is not a crossing on
/// its own (its neighbour pair on the other side will be).
fn crosses(a: f32, b: f32) -> bool {
    (a - EDGE_THRESHOLD) * (b - EDGE_THRESHOLD) < 0.0
}

/// Outward unit normal at texel `(x, y)`: −normalize(∇mask), central
/// differences with border clamping. `None` when the local gradient is
/// degenerate (flat plateau — cannot orient a normal).
fn outward_normal(mask: &[f32], x: usize, y: usize) -> Option<Vec2> {
    let n = MASK_SIZE;
    let xl = x.saturating_sub(1);
    let xr = (x + 1).min(n - 1);
    let yu = y.saturating_sub(1);
    let yd = (y + 1).min(n - 1);
    let g = Vec2::new(
        mask[y * n + xr] - mask[y * n + xl],
        mask[yd * n + x] - mask[yu * n + x],
    );
    let len = g.length();
    if len > f32::EPSILON {
        Some(-g / len)
    } else {
        None
    }
}

/// `usize` → `f32` for mask-grid indices (all ≤ 256, exact in `f32`).
fn cellf(v: usize) -> f32 {
    f32::from(u16::try_from(v).unwrap_or(u16::MAX))
}

/// Map a `0..1` mask-UV coordinate to a nearest mask texel index, clamped
/// in-range. Mirrors the extraction loop's grid convention (`float→index` has
/// no `From`/`TryFrom`; the `as` cast saturates so a sign/overflow is clamped
/// away by the following `min`).
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value is a saturating float->int cast then clamped to MASK_SIZE - 1"
)]
fn texel_index(uv: f32) -> usize {
    ((uv * cellf(MASK_SIZE)) as usize).min(MASK_SIZE - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec2;

    /// Binary disc mask: 1.0 inside radius `r` texels of `centre`, else 0.0.
    fn disc(centre: Vec2, r: f32) -> Vec<f32> {
        let mut m = vec![0.0_f32; MASK_SIZE * MASK_SIZE];
        for y in 0..MASK_SIZE {
            for x in 0..MASK_SIZE {
                let p = Vec2::new(cellf(x) + 0.5, cellf(y) + 0.5);
                if p.distance(centre) < r {
                    m[y * MASK_SIZE + x] = 1.0;
                }
            }
        }
        m
    }

    fn cellf(v: usize) -> f32 {
        u16::try_from(v).map_or(0.0, f32::from)
    }

    /// A zeroed motion field for call sites that don't exercise the
    /// edge-motion path (the extraction geometry is motion-independent).
    fn no_motion() -> Vec<f32> {
        vec![0.0_f32; MASK_SIZE * MASK_SIZE]
    }

    #[test]
    fn circle_yields_perimeter_points_with_outward_unit_normals() {
        let centre = Vec2::new(128.0, 128.0);
        let mask = disc(centre, 60.0);
        let mut out = Vec::with_capacity(MAX_EDGE_POINTS);
        let mut weights = Vec::with_capacity(MAX_EDGE_POINTS);
        extract_edges(&mask, &no_motion(), &mut out, &mut weights);
        // A radius-60 disc crosses ~2 texels per row over ~120 rows plus the
        // same per column: ≈ 480 crossings. Wide band for discretization.
        assert!(
            (380..=600).contains(&out.len()),
            "unexpected edge count {}",
            out.len()
        );
        let centre_uv = centre / cellf(MASK_SIZE);
        for p in &out {
            // Unit-length normal…
            assert!(
                (p.normal.length() - 1.0).abs() < 1e-3,
                "normal={:?}",
                p.normal
            );
            // …pointing away from the disc centre (outward).
            let radial = p.pos - centre_uv;
            assert!(
                radial.dot(p.normal) > 0.0,
                "normal {:?} not outward at {:?}",
                p.normal,
                p.pos
            );
            // Positions stay in the unit square, on the circle (± one texel).
            let r_uv = 60.0 / cellf(MASK_SIZE);
            assert!((radial.length() - r_uv).abs() < 2.0 / cellf(MASK_SIZE));
        }
    }

    #[test]
    fn torso_blob_edges_have_axis_aligned_normals_on_the_flanks() {
        // A filled axis-aligned rectangle (torso stand-in): x ∈ [96, 160),
        // y ∈ [64, 192).
        let mut mask = vec![0.0_f32; MASK_SIZE * MASK_SIZE];
        for y in 64..192 {
            for x in 96..160 {
                mask[y * MASK_SIZE + x] = 1.0;
            }
        }
        let mut out = Vec::with_capacity(MAX_EDGE_POINTS);
        let mut weights = Vec::with_capacity(MAX_EDGE_POINTS);
        extract_edges(&mask, &no_motion(), &mut out, &mut weights);
        // 2 horizontal crossings × 128 rows + 2 vertical × 64 columns = 384.
        assert!(
            (350..=420).contains(&out.len()),
            "unexpected edge count {}",
            out.len()
        );
        // Points on the left flank (x ≈ 96/256, away from corners) must point
        // straight −x.
        let mut checked = 0;
        for p in &out {
            if (p.pos.x - 96.0 / 256.0).abs() < 1.5 / 256.0
                && p.pos.y > 100.0 / 256.0
                && p.pos.y < 150.0 / 256.0
            {
                assert!(p.normal.x < -0.9, "left-flank normal {:?}", p.normal);
                assert!(p.normal.y.abs() < 0.3);
                checked += 1;
            }
        }
        assert!(checked > 10, "too few left-flank samples: {checked}");
    }

    #[test]
    fn capacity_clamps_without_reallocating() {
        // Vertical bands, width 2 (period 4): every band edge crosses 0.5 —
        // ~128 crossings per row × 256 rows, far beyond MAX_EDGE_POINTS.
        // Deviation from the brief's single-texel-wide (period-2) stripes:
        // with period 2 every interior crossing's chosen sample column has
        // identical left/right neighbours (same parity → same value), so the
        // central-difference gradient is exactly zero and outward_normal
        // correctly skips it as unorientable — only ~256 boundary-clamp
        // artifacts survive, never reaching capacity. Width-2 bands keep
        // every interior crossing's gradient nonzero while still producing
        // far more than MAX_EDGE_POINTS crossings, so the capacity-clamp,
        // no-realloc, and pointer-stability invariants are still exercised.
        let mut mask = vec![0.0_f32; MASK_SIZE * MASK_SIZE];
        for y in 0..MASK_SIZE {
            for x in 0..MASK_SIZE {
                if x % 4 < 2 {
                    mask[y * MASK_SIZE + x] = 1.0;
                }
            }
        }
        let mut out = Vec::with_capacity(MAX_EDGE_POINTS);
        let mut weights = Vec::with_capacity(MAX_EDGE_POINTS);
        let ptr = out.as_ptr();
        extract_edges(&mask, &no_motion(), &mut out, &mut weights);
        assert_eq!(out.len(), MAX_EDGE_POINTS, "must clamp at capacity");
        assert_eq!(out.capacity(), MAX_EDGE_POINTS, "must never grow");
        assert_eq!(out.as_ptr(), ptr, "must never reallocate");
    }

    #[test]
    fn append_partitions_two_slots_and_respects_the_shared_cap() {
        // Two disc masks appended back-to-back (slot 0 then slot 1): the
        // counts partition the list exactly, and the total stays under cap.
        let a = disc(Vec2::new(90.0, 128.0), 40.0);
        let b = disc(Vec2::new(180.0, 128.0), 30.0);
        let mut out = Vec::with_capacity(MAX_EDGE_POINTS);
        let mut weights = Vec::with_capacity(MAX_EDGE_POINTS);
        let motion = no_motion();
        let ptr = out.as_ptr();
        let n_a = extract_edges_append(&a, &motion, &mut out, &mut weights);
        let n_b = extract_edges_append(&b, &motion, &mut out, &mut weights);
        assert!(n_a > 0 && n_b > 0);
        assert_eq!(out.len(), n_a + n_b, "counts partition the list");
        assert_eq!(weights.len(), out.len(), "weights stay index-parallel");
        assert_eq!(out.as_ptr(), ptr, "append never reallocates");
        // Slot 0's range centres on disc A, slot 1's on disc B.
        let mean =
            |pts: &[EdgePoint]| pts.iter().map(|p| p.pos).sum::<Vec2>() / cellf(pts.len().max(1));
        let ca = mean(&out[..n_a]);
        let cb = mean(&out[n_a..]);
        assert!((ca.x - 90.0 / 256.0).abs() < 0.02, "slot 0 range = disc A");
        assert!((cb.x - 180.0 / 256.0).abs() < 0.02, "slot 1 range = disc B");
    }

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

    #[test]
    fn refill_clears_previous_points() {
        let mask_a = disc(Vec2::new(128.0, 128.0), 40.0);
        let empty = vec![0.0_f32; MASK_SIZE * MASK_SIZE];
        let mut out = Vec::with_capacity(MAX_EDGE_POINTS);
        let mut weights = Vec::with_capacity(MAX_EDGE_POINTS);
        let motion = no_motion();
        extract_edges(&mask_a, &motion, &mut out, &mut weights);
        assert!(!out.is_empty());
        assert_eq!(weights.len(), out.len());
        extract_edges(&empty, &motion, &mut out, &mut weights);
        assert!(out.is_empty(), "clear-refill semantics");
        assert!(weights.is_empty(), "weights cleared with points");
    }
}
