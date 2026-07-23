//! Platform facade for OBSBOT device IO.
//!
//! The vendored libdev SDK binary is linked on **Windows and macOS** — the
//! platforms that actually run with an OBSBOT attached (the Windows kiosk;
//! the macOS dev/party rig). `vendor/libdev` ships Linux binaries too, but
//! with no Linux deployment the link would be dead weight there. Following
//! the `lifecycle/thermal/platform/` convention:
//!
//! - `libdev` — the real backend: extern "C" bindings to
//!   `vendor/libdev/shim/obsbot_shim.h` plus the dedicated worker thread that
//!   owns all device IO (SDK setters can block for a device round-trip, so
//!   they must never run on the Bevy schedule). wc-core's build.rs compiles
//!   the shim and links the vendored SDK binary for whichever platform is
//!   building.
//! - `stub` — everywhere else: [`spawn_worker`] returns `None`, nothing
//!   links, and the module stays compile-checked by Linux CI's
//!   `--all-features` build.
//!
//! Both export the same two names, so `super` code is platform-agnostic:
//! `spawn_worker(take_control) -> Option<WorkerHandle>` and the
//! [`WorkerHandle`] type with `send` / `try_recv_status`.

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub mod libdev;

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use libdev::{spawn_worker, WorkerHandle};

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub mod stub;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub use stub::{spawn_worker, WorkerHandle};
