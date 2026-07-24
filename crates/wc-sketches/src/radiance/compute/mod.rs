//! Render-world compute for the Radiance aura: GPU POD mirrors
//! ([`sim_params`]), the dispatch pipeline (`pipeline`), and the two
//! generation-gated storage-buffer uploads (`edge_upload` for the silhouette
//! edge list, `field_upload` for the signed distance field).

pub mod sim_params;

// `pipeline`, `edge_upload`, and `field_upload` consume
// `wc_core::input::body` (`EdgePoint`, `SilhouetteEdges`, `MASK_SIZE`,
// `MAX_EDGE_POINTS`), which wc-core gates behind this feature
// (camera-independent, CI-testable headless). The `cargo doc` gate builds
// default features only, so these modules must be absent there — see
// `Cargo.toml`'s `body-tracking-mediapipe` forwarding feature, and
// `radiance::systems::mod` for the identical precedent.
#[cfg(feature = "body-tracking-mediapipe")]
pub mod edge_upload;
#[cfg(feature = "body-tracking-mediapipe")]
pub mod field_upload;
#[cfg(feature = "body-tracking-mediapipe")]
pub mod pipeline;
