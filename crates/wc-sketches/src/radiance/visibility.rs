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
        assert!(
            !latch.step(0.40),
            "post-reset requires the strict bar again"
        );
    }
}
