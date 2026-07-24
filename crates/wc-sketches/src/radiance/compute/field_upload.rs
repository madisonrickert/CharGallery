//! Signed distance field → GPU storage buffer, keyed on generation.
//!
//! `RadianceDistanceField` (main world) is recomputed in place per body
//! frame and bumps `generation`. Extracting the 64 KiB byte plane every
//! render frame would copy it per frame for a ~30 Hz signal, and pushing it
//! through a `ShaderBuffer` asset would recreate the GPU buffer — churning
//! the bind-group cache's `BufferId` key. Instead (the `edge_upload` shape):
//!
//! 1. [`extract_distance_field`] (`ExtractSchedule`) copies the packed bytes
//!    into a render-world scratch ([`ExtractedField`], capacity `MASK_SIZE²`,
//!    refilled with `clear()` — zero steady-state allocation) ONLY when
//!    `generation` changed.
//! 2. [`upload_distance_field`] (`RenderSystems::PrepareBindGroups`, before
//!    the bind-group prepare) `write_buffer`s the scratch into the
//!    persistent `field_buffer` on [`super::pipeline::RadiancePipeline`] — a
//!    staged copy, no allocation, stable `BufferId`.
//!
//! The kernel reads the buffer as `array<u32>` (four texel bytes per word)
//! and unpacks in `field_signed_px`; the byte plane uploads verbatim.

use bevy::prelude::*;
use bevy::render::renderer::RenderQueue;
use bevy::render::Extract;
use wc_core::input::body::MASK_SIZE;

use super::pipeline::RadiancePipeline;
use crate::radiance::distance_field::RadianceDistanceField;

/// Render-world scratch copy of the newest signed distance field.
#[derive(Resource)]
pub struct ExtractedField {
    /// Generation of the copy currently held (and, once uploaded, of the GPU
    /// buffer). `u64::MAX` = "never copied" sentinel — distinct from the
    /// field's 0-based counter, so the first computed field always uploads.
    pub generation: u64,
    /// Byte scratch; capacity `MASK_SIZE²`, refilled with `clear()`.
    pub bytes: Vec<u8>,
    /// A fresh copy is waiting for [`upload_distance_field`].
    pub dirty: bool,
}

impl Default for ExtractedField {
    fn default() -> Self {
        Self {
            generation: u64::MAX,
            bytes: Vec::with_capacity(MASK_SIZE * MASK_SIZE),
            dirty: false,
        }
    }
}

/// `ExtractSchedule`: copy the main-world field when (and only when) its
/// generation changed. No-ops in one compare in the steady state between
/// body frames.
pub fn extract_distance_field(
    main: Extract<'_, '_, Option<Res<'_, RadianceDistanceField>>>,
    mut extracted: ResMut<'_, ExtractedField>,
) {
    let Some(src) = main.as_ref() else {
        return;
    };
    if src.generation == extracted.generation {
        return;
    }
    extracted.bytes.clear();
    // The field is `MASK_SIZE²` by construction; truncate defensively so the
    // scratch (and the fixed GPU buffer) can never overflow.
    let take = src.signed.len().min(MASK_SIZE * MASK_SIZE);
    extracted.bytes.extend_from_slice(&src.signed[..take]);
    extracted.generation = src.generation;
    extracted.dirty = true;
}

/// `Render` (`PrepareBindGroups`, ordered before the bind-group prepare):
/// stage the fresh copy into the persistent field buffer.
pub fn upload_distance_field(
    pipeline: Option<Res<'_, RadiancePipeline>>,
    render_queue: Res<'_, RenderQueue>,
    mut extracted: ResMut<'_, ExtractedField>,
) {
    let Some(pipeline) = pipeline else {
        return;
    };
    if !extracted.dirty {
        return;
    }
    if !extracted.bytes.is_empty() {
        render_queue
            .0
            .write_buffer(&pipeline.field_buffer, 0, &extracted.bytes);
    }
    extracted.dirty = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scratch starts at the never-copied sentinel with full capacity and
    /// nothing pending — so the first computed field always copies, and the
    /// steady state never allocates.
    #[test]
    fn extracted_field_default_is_clean_sentinel() {
        let f = ExtractedField::default();
        assert_eq!(f.generation, u64::MAX);
        assert!(f.bytes.is_empty());
        assert!(f.bytes.capacity() >= MASK_SIZE * MASK_SIZE);
        assert!(!f.dirty);
    }
}
