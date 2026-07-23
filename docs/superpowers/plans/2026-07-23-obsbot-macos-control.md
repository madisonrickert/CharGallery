# OBSBOT Camera Control on macOS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give macOS the real OBSBOT camera-control backend (AI-off / gesture-off / gimbal / FOV / exposure) that today exists only on Windows, per `docs/superpowers/specs/2026-07-23-obsbot-macos-control-design.md`.

**Architecture:** Promote `platform/windows.rs` to a shared `platform/libdev.rs` compiled on Windows + macOS; extend `wc-core/build.rs` to compile the C++ shim with clang and link the vendored `libdev.dylib`; resolve at runtime via the repo's existing vendor-rpath convention; default the `obsbot-camera-control` feature on in both waveconductor platform target tables; stage the dylib in `bundle-mac`.

**Tech Stack:** Rust (Bevy), `cc` build-script C++ compile (already a transitive workspace dep), vendored OBSBOT libdev SDK (`vendor/libdev`), ld64 rpath resolution.

## Global Constraints

- **No new dependencies.** `cc` is already in the workspace graph (ort, blake3) and already a wc-core build-dependency on Windows.
- **Linux stays on the stub.** The real backend cfg is exactly `any(target_os = "windows", target_os = "macos")`; the stub cfg is its negation. Ubuntu CI's `--all-features` must keep building the stub with no C++ toolchain use.
- **Link names are exact:** macOS links `cargo:rustc-link-lib=dylib=dev` against `vendor/libdev/macos/<arch>-release/` where `<arch>` is `arm64` for target arch `aarch64` and `x86_64` for `x86_64`. Windows keeps `cargo:rustc-link-lib=libdev` and its DLL staging, unchanged.
- **The dylib is never staged in dev builds on macOS** — runtime resolution is rpath-only (install name `@rpath/libdev.dylib`). The app bundle is the one place the dylib is copied (`Contents/MacOS/libdev.dylib`).
- **Feature default:** `"obsbot-camera-control"` is added to the `wc-core` feature lists in **both** waveconductor target tables (macOS and Windows). It stays out of `[features] default` and out of wc-core's own defaults.
- **Move preserves history:** the backend move is a `git mv` of `platform/windows.rs` to `platform/libdev.rs`.
- **Repo standards apply:** no `unwrap`/`expect` outside tests/build-scripts, no `as` numeric casts, comments preserved and updated rather than deleted, rustdoc gate is `-D warnings` on **default features** (keep links to feature-gated items as plain code spans), commit messages contain no backticks.
- **Verification commands** (used throughout): `cargo nextest run -p wc-core --features obsbot-camera-control`, `cargo clippy --all-targets --all-features --workspace -- -D warnings`, `cargo fmt --all -- --check`.

---

### Task 1: Build wiring — compile the shim and link libdev.dylib on macOS

**Files:**
- Modify: `crates/wc-core/build.rs`
- Modify: `crates/wc-core/Cargo.toml:95-100` (feature comment) and `:227-233` (build-dep target table)
- Modify: `.cargo/config.toml` (Apple target rustflags)

**Interfaces:**
- Consumes: `vendor/libdev/shim/obsbot_shim.cpp`, `vendor/libdev/macos/{arm64,x86_64}-release/libdev.dylib` (already vendored).
- Produces: a macOS build of wc-core with `--features obsbot-camera-control` that compiles the shim and links `libdev.dylib`; test/app binaries resolve it via rpath. Task 2's `libdev.rs` module relies on these symbols resolving.

- [ ] **Step 1: Extend the cc build-dependency to macOS**

In `crates/wc-core/Cargo.toml`, replace the Windows-only build-dep table:

```toml
[target.'cfg(target_os = "windows")'.build-dependencies]
# Compiles the extern "C" shim over the vendored OBSBOT libdev C++ SDK
# (vendor/libdev/shim) when the `obsbot-camera-control` feature is on.
# Windows-target-only table + a CARGO_FEATURE_* check in build.rs, so
# macOS/Linux `--all-features` builds never need a C++ toolchain or a libdev
# binary for this. Already in the workspace graph transitively (ort, blake3).
cc = "1"
```

with:

```toml
[target.'cfg(any(target_os = "windows", target_os = "macos"))'.build-dependencies]
# Compiles the extern "C" shim over the vendored OBSBOT libdev C++ SDK
# (vendor/libdev/shim) when the `obsbot-camera-control` feature is on.
# Windows + macOS target table + a CARGO_FEATURE_* check in build.rs, so
# Linux `--all-features` builds never need a C++ toolchain or a libdev
# binary for this. Already in the workspace graph transitively (ort, blake3).
cc = "1"
```

And update the feature's own comment block (directly above `obsbot-camera-control = []`):

```toml
# Real device IO is compiled on Windows and macOS (build.rs compiles the
# extern "C" shim and links the vendored libdev binary for the building
# platform); on Linux the module is a documented no-op facade, so CI's
# `--all-features` stays green there without a C++ toolchain. No Rust deps:
# the worker uses std::thread + std::sync::mpsc.
obsbot-camera-control = []
```

- [ ] **Step 2: Split build.rs into a shared shim compile + per-OS link half**

In `crates/wc-core/build.rs`:

(a) Change the obsbot gate in `main()` from:

```rust
    // OBSBOT libdev shim — Windows + `obsbot-camera-control` only. The feature
    // check is an env probe (not a cfg) because build scripts see features via
    // CARGO_FEATURE_*; the cfg(windows) above/below refers to the *host*, which
    // equals the target for every supported build of this project.
    #[cfg(target_os = "windows")]
    if std::env::var_os("CARGO_FEATURE_OBSBOT_CAMERA_CONTROL").is_some() {
        build_obsbot_shim();
    }
```

to:

```rust
    // OBSBOT libdev shim — Windows/macOS + `obsbot-camera-control` only. The
    // feature check is an env probe (not a cfg) because build scripts see
    // features via CARGO_FEATURE_*; the cfg(target_os) here refers to the
    // *host*, which equals the target for every supported build of this
    // project.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if std::env::var_os("CARGO_FEATURE_OBSBOT_CAMERA_CONTROL").is_some() {
        build_obsbot_shim();
    }
```

(b) Re-gate `build_obsbot_shim` to `#[cfg(any(target_os = "windows", target_os = "macos"))]`, keep the shared `cc::Build` invocation exactly as is (the `/EHsc` and `/utf-8` flags are `flag_if_supported` and no-op under Apple clang; `.debug(false)` keeps its MSVC CRT comment and merely skips `-g` on clang), and move the Windows-only link + DLL staging into a new `link_libdev` with two cfg variants. End state:

```rust
/// Compile `vendor/libdev/shim/obsbot_shim.cpp` (shared between Windows and
/// macOS — the shim is plain C++17 with no platform-specific code), then hand
/// off to the per-OS `link_libdev` for the SDK binary link.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn build_obsbot_shim() {
    let libdev_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/libdev");

    println!("cargo:rerun-if-changed=../../vendor/libdev/shim/obsbot_shim.cpp");
    println!("cargo:rerun-if-changed=../../vendor/libdev/shim/obsbot_shim.h");

    // CRT contract (Windows): libdev.dll ships as a release-CRT (/MD) binary
    // and its API passes MSVC STL types (std::string/std::function/
    // std::shared_ptr) across the DLL boundary. Rust's MSVC target also always
    // links the release CRT. `.debug(false)` keeps the shim off the debug CRT /
    // _ITERATOR_DEBUG_LEVEL path in debug profiles so the STL object layouts
    // match on both sides. (On clang the same call merely skips `-g` for the
    // shim, which is fine.)
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .debug(false)
        .file(libdev_root.join("shim/obsbot_shim.cpp"))
        .include(libdev_root.join("include"))
        .include(libdev_root.join("shim"))
        // C++ exceptions stay inside the shim (every entry point is
        // try/catch-wrapped); MSVC still needs unwind semantics enabled.
        // Both flags are MSVC-style and no-op under Apple clang
        // (`flag_if_supported` probes and drops them).
        .flag_if_supported("/EHsc")
        // dev.hpp contains UTF-8 comments; keep MSVC from guessing a codepage.
        .flag_if_supported("/utf-8")
        .compile("obsbot_shim");

    link_libdev(&libdev_root);
}
```

The Windows variant is the existing link + staging code moved verbatim (import-lib search path, `cargo:rustc-link-lib=libdev`, the `rerun-if-changed` on `libdev.lib`, and the DLL staging loop with `stage_dll`), under:

```rust
/// Link the vendored import library and stage the runtime DLLs beside every
/// binary this target directory produces (Windows resolves DLLs adjacent to
/// the exe; see the module doc).
#[cfg(target_os = "windows")]
fn link_libdev(libdev_root: &std::path::Path) {
```

The macOS variant is new:

```rust
/// Link the vendored `libdev.dylib` for the compile target's architecture.
/// No staging, unlike Windows: the dylib's install name is
/// `@rpath/libdev.dylib`, and the vendor-dir rpath entries in
/// `.cargo/config.toml` (same convention as LeapC) resolve it at runtime for
/// binaries at `target/<profile>/`, `deps/`, and `examples/` depths. The app
/// bundle instead ships a copy at `Contents/MacOS/` resolved via
/// `@loader_path` — see xtask bundle-mac.
#[cfg(target_os = "macos")]
fn link_libdev(libdev_root: &std::path::Path) {
    // Build scripts read the *target* arch from CARGO_CFG_TARGET_ARCH (a
    // cfg! here would report the build host).
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH set by cargo");
    let subdir = match arch.as_str() {
        "aarch64" => "arm64-release",
        "x86_64" => "x86_64-release",
        other => panic!("no vendored libdev.dylib for macOS target arch '{other}'"),
    };
    let lib_dir = libdev_root.join("macos").join(subdir);
    println!("cargo:rerun-if-changed=../../vendor/libdev/macos/{subdir}/libdev.dylib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    // The file is libdev.dylib, so the link name is "dev".
    println!("cargo:rustc-link-lib=dylib=dev");
}
```

Also update the build.rs module doc header: the "## OBSBOT libdev (`obsbot-camera-control` feature)" section's "Windows-only by design" paragraph now reads that the shim + link cover Windows **and macOS**, Linux keeps the no-op facade, and macOS resolves the dylib via the `.cargo/config.toml` vendor rpaths instead of staging. Keep the existing DLL-staging paragraph as the Windows half.

- [ ] **Step 3: Add the libdev rpath entries to the Apple target stanzas**

In `.cargo/config.toml`, extend both Apple `rustflags` arrays (the stanza comment already explains the two-depths convention — extend its mention of "vendored" paths to name libdev alongside LeapC):

`[target.aarch64-apple-darwin]` gains, after the existing LeapC entries:

```toml
    "-C", "link-arg=-Wl,-rpath,@executable_path/../../vendor/libdev/macos/arm64-release",
    "-C", "link-arg=-Wl,-rpath,@executable_path/../../../vendor/libdev/macos/arm64-release",
```

`[target.x86_64-apple-darwin]` gains:

```toml
    "-C", "link-arg=-Wl,-rpath,@executable_path/../../vendor/libdev/macos/x86_64-release",
    "-C", "link-arg=-Wl,-rpath,@executable_path/../../../vendor/libdev/macos/x86_64-release",
```

- [ ] **Step 4: Verify the link end-to-end (module is still the stub — that is expected)**

Run: `cargo nextest run -p wc-core --features obsbot-camera-control`
Expected: builds (shim compiles under clang, `-ldev` links) and all tests pass. The backend is still `stub.rs` on macOS at this point; the dylib links unreferenced, which proves search path + rpath resolution without touching device code.

Run: `otool -L "$(fd -t x 'wc_core-' target/debug/deps --max-depth 1 | head -1)" | grep libdev`
Expected: one line containing `@rpath/libdev.dylib`. (`-t x` limits fd to executables so a `wc_core-<hash>.d` dep-info file is never picked.)

- [ ] **Step 5: Commit**

```bash
git add crates/wc-core/build.rs crates/wc-core/Cargo.toml .cargo/config.toml
git commit -m "build(obsbot): compile the libdev shim and link libdev.dylib on macOS"
```

---

### Task 2: Shared libdev backend module

**Files:**
- Rename (git mv): `crates/wc-core/src/input/obsbot/platform/windows.rs` → `crates/wc-core/src/input/obsbot/platform/libdev.rs`
- Modify: `crates/wc-core/src/input/obsbot/platform/mod.rs` (full rewrite below)
- Modify: `crates/wc-core/src/input/obsbot/platform/stub.rs:1-5` (doc header)
- Modify: `crates/wc-core/src/input/obsbot/mod.rs:29-31` (module doc) and the `facade_is_noop_off_windows` test (~line 663)

**Interfaces:**
- Consumes: Task 1's link wiring (the `ffi` extern block now resolves on macOS).
- Produces: `platform::spawn_worker(take_control: bool) -> Option<WorkerHandle>` and `platform::WorkerHandle { send, try_recv_status }` — same facade, now real on macOS. Tasks 3-5 rely on the worker actually running there.

- [ ] **Step 1: Move the backend**

```bash
git mv crates/wc-core/src/input/obsbot/platform/windows.rs crates/wc-core/src/input/obsbot/platform/libdev.rs
```

- [ ] **Step 2: Re-point the facade**

Replace the body of `platform/mod.rs` with:

```rust
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
```

- [ ] **Step 3: Update the moved file's docs (contents otherwise untouched)**

In `platform/libdev.rs`:
- Module doc first line: `//! Windows OBSBOT backend: raw bindings to the vendored libdev SDK's` → `//! libdev OBSBOT backend (Windows + macOS): raw bindings to the vendored SDK's`.
- In the `obsbot_hardware_smoke` test doc, the parenthetical `(and libdev.dll + w32-pthreads.dll beside the test binary, which build.rs stages)` → `(plus the SDK runtime: on Windows libdev.dll + w32-pthreads.dll beside the test binary, staged by build.rs; on macOS libdev.dylib, resolved via the vendor rpath)`.
- In the `ffi` module doc, `wc-core's build.rs compiles the shim and links libdev.lib` → `wc-core's build.rs compiles the shim and links the vendored SDK binary (libdev.lib on Windows, libdev.dylib on macOS)`.

- [ ] **Step 4: Update the stub doc header**

In `platform/stub.rs`, replace lines 1-5 with:

```rust
//! No-op OBSBOT backend for platforms without the libdev link (today:
//! Linux). Exists so the `obsbot-camera-control` feature — which CI's
//! `--all-features` switches on for every runner — builds and tests green
//! without a C++ toolchain, the vendored `libdev` binary, or an OBSBOT
//! plugged in. See `platform/mod.rs` for the facade contract.
```

- [ ] **Step 5: Update the parent module doc and the stub test**

In `obsbot/mod.rs`, replace (lines 29-32):

```rust
//! Real device IO is **Windows-only** (the deployment target; see
//! `platform/`): elsewhere [`platform::spawn_worker`](crate::input::obsbot::platform::spawn_worker) returns `None` and the
//! resource reports [`ObsbotStatus::NoDevice`](crate::input::obsbot::ObsbotStatus::NoDevice) forever, which keeps CI's
//! `--all-features` builds green on every runner without a C++ toolchain.
```

with:

```rust
//! Real device IO runs on **Windows and macOS** (the kiosk and the dev/party
//! rig; see `platform/`): elsewhere [`platform::spawn_worker`](crate::input::obsbot::platform::spawn_worker) returns `None` and the
//! resource reports [`ObsbotStatus::NoDevice`](crate::input::obsbot::ObsbotStatus::NoDevice) forever, which keeps CI's
//! `--all-features` builds green on Linux runners without a C++ toolchain.
```

And replace the stub test:

```rust
    /// The facade contract on platforms without a real backend: no worker,
    /// ever — the status stays `NoDevice` and nothing links or loads.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn facade_is_noop_off_windows() {
        assert!(platform::spawn_worker(true).is_none());
    }
```

with:

```rust
    /// The facade contract on platforms without a real backend: no worker,
    /// ever — the status stays `NoDevice` and nothing links or loads.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    fn facade_is_noop_on_unsupported_platforms() {
        assert!(platform::spawn_worker(true).is_none());
    }
```

- [ ] **Step 6: Verify**

Run: `cargo nextest run -p wc-core --features obsbot-camera-control`
Expected: PASS. `buf_to_string_stops_at_nul` (inside `libdev.rs`) now runs on macOS; the two hardware tests stay ignored; `facade_is_noop_on_unsupported_platforms` is compiled out here.

Run: `cargo clippy -p wc-core --all-targets --features obsbot-camera-control -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add -A crates/wc-core/src/input/obsbot
git commit -m "feat(obsbot): share the libdev worker backend with macOS"
```

---

### Task 3: Default the feature on in both platform target tables

**Files:**
- Modify: `crates/waveconductor/Cargo.toml:69-73`

**Interfaces:**
- Consumes: Tasks 1-2 (a macOS waveconductor build now compiles and links the real backend).
- Produces: `cargo rund` / `cargo build -p waveconductor` carry OBSBOT control on macOS and Windows with no feature flag. Task 4's bundle staging becomes load-bearing (the release binary links the dylib).

- [ ] **Step 1: Add the feature to both target tables**

Replace:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
wc-core = { workspace = true, features = ["thermal-sensor", "thermal-sensor-macos"] }

[target.'cfg(target_os = "windows")'.dependencies]
wc-core = { workspace = true, features = ["thermal-sensor", "thermal-sensor-windows"] }
```

with:

```toml
# Both real-backend platforms also get OBSBOT camera control by default
# (decided 2026-07-23): the app takes control of an attached OBSBOT — its
# on-device AI otherwise fights our MediaPipe tracking — and the persisted
# "Take control" toggle in the Camera settings is the runtime off-switch.
# Linux gets the no-op facade via wc-core's platform cfg, so the feature is
# deliberately absent from its (nonexistent) target table rather than from
# [features] default.
[target.'cfg(target_os = "macos")'.dependencies]
wc-core = { workspace = true, features = [
    "thermal-sensor",
    "thermal-sensor-macos",
    "obsbot-camera-control",
] }

[target.'cfg(target_os = "windows")'.dependencies]
wc-core = { workspace = true, features = [
    "thermal-sensor",
    "thermal-sensor-windows",
    "obsbot-camera-control",
] }
```

- [ ] **Step 2: Verify the app builds and starts the worker**

Run: `cargo build -p waveconductor`
Expected: builds; the binary links `@rpath/libdev.dylib` (`otool -L target/debug/waveconductor | grep libdev` shows it).

Run (stock macOS has no `timeout`; background + kill instead):

```bash
cargo rund > /tmp/rund_obsbot_check.log 2>&1 &
sleep 25 && grep -i obsbot /tmp/rund_obsbot_check.log; kill %1
```

Expected: an `OBSBOT SDK initialized (libdev); watching for devices` line (and, with the camera attached, `OBSBOT device detected: ...`).

- [ ] **Step 3: Commit**

```bash
git add crates/waveconductor/Cargo.toml
git commit -m "feat(obsbot): default camera control on for macOS and Windows app builds"
```

---

### Task 4: bundle-mac staging + runbook updates

**Files:**
- Modify: `xtask/src/bundle/common.rs:87-107` (rename `copy_leap_lib` → `copy_vendored_lib`, neutral error)
- Modify: `xtask/src/bundle/mac.rs` (layout doc, staging call, subdir helper, tests)
- Modify: `xtask/src/bundle/windows.rs:146`, `xtask/src/bundle/linux.rs:124` (renamed helper call sites)
- Modify: `docs/runbooks/obsbot.md`, `docs/runbooks/kiosk.md:52-54`

**Interfaces:**
- Consumes: Task 3 (the macOS release binary always links libdev, so the bundle must ship it).
- Produces: `libdev_vendor_subdir(arch: &str) -> Option<&'static str>` in `bundle/mac.rs`; a `WaveConductor.app` whose `Contents/MacOS/` contains `libdev.dylib`.

- [ ] **Step 1: Write the failing tests (mirror the LeapC subdir tests)**

In `xtask/src/bundle/mac.rs` `tests` module:

```rust
    // ---- libdev_vendor_subdir -------------------------------------------------

    #[test]
    fn libdev_vendor_subdir_aarch64() {
        assert_eq!(
            libdev_vendor_subdir("aarch64"),
            Some("arm64-release"),
            "aarch64 must map to the vendored arm64-release dir"
        );
    }

    #[test]
    fn libdev_vendor_subdir_x86_64() {
        assert_eq!(
            libdev_vendor_subdir("x86_64"),
            Some("x86_64-release"),
            "x86_64 must map to the vendored x86_64-release dir"
        );
    }

    #[test]
    fn libdev_vendor_subdir_unsupported_returns_none() {
        assert_eq!(libdev_vendor_subdir("riscv64"), None);
        assert_eq!(libdev_vendor_subdir(""), None);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p xtask libdev_vendor_subdir`
Expected: FAIL — `cannot find function libdev_vendor_subdir`.

- [ ] **Step 3: Implement helper + staging; generalize the copy helper**

In `xtask/src/bundle/common.rs`, rename `copy_leap_lib` to `copy_vendored_lib` and neutralize its error text (the helper already dereferences symlinks for any dylib):

```rust
pub fn copy_vendored_lib(src: &Path, dst: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !src.exists() {
        return Err(format!(
            "bundle: vendored library not found at {}; restore the vendor tree",
            src.display()
        )
        .into());
    }
```

(body otherwise unchanged; update the doc comment's LeapC-specific phrasing to "a vendored native library"). Update the three call sites (`bundle/windows.rs:146`, `bundle/linux.rs:124`, `bundle/mac.rs:372`) to the new name.

In `xtask/src/bundle/mac.rs`:

(a) Add to the bundle-layout doc comment, after the `libLeapC.dylib` line:

```text
//! │   ├── MacOS/libdev.dylib          (vendored OBSBOT libdev SDK runtime)
```

(b) Below `leap_vendor_subdir`, add:

```rust
/// Map a Rust target architecture name to the vendor subdirectory that holds
/// the prebuilt OBSBOT libdev dylib for that architecture.
///
/// Returns `None` for architectures that have no vendored libdev copy.
/// Covered by unit tests in the `tests` module.
pub fn libdev_vendor_subdir(arch: &str) -> Option<&'static str> {
    match arch {
        "aarch64" => Some("arm64-release"),
        "x86_64" => Some("x86_64-release"),
        _ => None,
    }
}

/// Copy the vendored OBSBOT libdev dylib into `dst_dir` (normally
/// `Contents/MacOS/`) so the binary's `@loader_path` rpath entry resolves its
/// `@rpath/libdev.dylib` install name at launch. The macOS release binary
/// always links libdev (`obsbot-camera-control` is a waveconductor
/// target-table default), so a missing dylib is a hard bundling error — the
/// app would die in dyld at launch.
fn copy_libdev_dylib(root: &Path, dst_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let arch = std::env::consts::ARCH;
    let subdir = libdev_vendor_subdir(arch).ok_or_else(|| {
        format!(
            "bundle-mac: no vendored OBSBOT libdev for architecture '{arch}'; \
             add a vendor/libdev/macos/ directory for it"
        )
    })?;
    let src = root
        .join("vendor")
        .join("libdev")
        .join("macos")
        .join(subdir)
        .join("libdev.dylib");
    common::copy_vendored_lib(&src, &dst_dir.join("libdev.dylib"))
}
```

(c) Call it in `run`, directly after the `copy_leap_dylib` call (step 2b):

```rust
    // 2b-bis. Copy the vendored OBSBOT libdev dylib next to the binary; the
    //     macOS binary always links it (obsbot-camera-control target-table
    //     default) and resolves it via the @loader_path rpath entry.
    copy_libdev_dylib(&root, &macos_dir)?;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p xtask`
Expected: PASS (three new tests green, LeapC and plist tests still green).

- [ ] **Step 5: Update the runbooks**

`docs/runbooks/obsbot.md`:
- Header paragraph: `real device IO is Windows-only (the deployment target) — every other platform compiles a documented no-op facade` → `real device IO runs on Windows and macOS — Linux compiles a documented no-op facade`.
- Module map line: `worker thread ('platform/windows.rs')` → `worker thread ('platform/libdev.rs', Windows + macOS)`.
- Deploy notes: keep the DLL bullet as the Windows half; add a macOS bullet:
  ```markdown
  - **macOS: dylib via rpath.** Dev/test builds resolve the vendored
    `libdev.dylib` through the vendor rpath entries in `.cargo/config.toml`
    (nothing staged); `cargo xtask bundle-mac` ships a copy at
    `Contents/MacOS/libdev.dylib`. Quit OBSBOT Center while the app runs:
    it holds the camera's AVFoundation configuration lock (observed
    2026-07-23: the app's idle capture-throttle is blocked with an
    `avf: lockForConfiguration failed` warning while Center is open) and its
    device-control channel is redundant once the app takes control.
  ```
- Replace the `The feature is **not** in default; enable it on the app build that runs with the OBSBOT connected.` bullet with:
  ```markdown
  - The feature is a **target-table default** of the waveconductor crate on
    Windows and macOS (decided 2026-07-23) — plain `cargo build -p
    waveconductor` / `cargo rund` carry it. The persisted **Take control**
    toggle is the runtime off-switch; Linux builds compile the no-op facade.
  ```
- Hardware smoke section: note it runs on both platforms, and add the framing test command:
  ```
  cargo test -p wc-core --features obsbot-camera-control obsbot_hardware_framing -- --ignored --nocapture
  ```

`docs/runbooks/kiosk.md` (lines 52-54): replace

```markdown
- OBSBOT: the app takes control of the camera itself when built with
  `obsbot-camera-control` (see docs/runbooks/obsbot.md); otherwise disable
  its on-device AI in OBSBOT Center once (persists on-device).
```

with

```markdown
- OBSBOT: the app takes control of the camera itself by default
  (`obsbot-camera-control` is a Windows/macOS target-table default; see
  docs/runbooks/obsbot.md). The Camera-tab "Take control" toggle is the
  off-switch if OBSBOT Center should own the camera instead.
```

- [ ] **Step 6: Commit**

```bash
git add xtask/src/bundle docs/runbooks/obsbot.md docs/runbooks/kiosk.md
git commit -m "feat(bundle): stage libdev.dylib in the mac bundle; runbook updates for macOS control"
```

Note: a full `cargo xtask bundle-mac` run requires the ~5-8 min release build — per repo convention, defer it to the next pre-tag verification rather than running it here; the unit tests plus the staging code path cover the logic.

---

### Task 5: Full gates + hardware acceptance on the MBP

**Files:** none (verification only; fixes discovered here belong to the task that owns the file)

**Interfaces:**
- Consumes: everything above, plus the OBSBOT Tiny 2 Lite attached to this machine.

- [ ] **Step 1: CI gates**

Run, expecting every one green:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --workspace -- -D warnings
cargo nextest run --workspace --all-features
cargo test --doc --workspace
cargo doc --no-deps --workspace --document-private-items
cargo deny check
cargo xtask check-secrets
```

(For `cargo doc`, match CI by running with `RUSTDOCFLAGS="-D warnings"`.)

- [ ] **Step 2: Hardware smoke (the gimbal physically recenters)**

Quit OBSBOT Center first (it re-asserts its own settings while running).

Run: `cargo test -p wc-core --features obsbot-camera-control obsbot_hardware_smoke -- --ignored --nocapture`
Expected: PASS — device found within 10 s, `AI_OFF` and `GESTURE_OFF` both achieved, gimbal visibly recenters, control released at the end.

- [ ] **Step 3: Hardware framing (gimbal moves, zooms, restores)**

Run: `cargo test -p wc-core --features obsbot-camera-control obsbot_hardware_framing -- --ignored --nocapture`
Expected: PASS — worker reaches `InControl`, camera tilts/pans to (+20 deg, +30 deg), zooms to 1.5, narrows FOV, recenters, and the shutdown drop restores AI/gestures.

- [ ] **Step 4: End-to-end app session**

Run: `cargo rund` (OBSBOT Center closed), let it run ~1 min with the settings panel's Camera section open, then quit.
Expected: log shows `OBSBOT SDK initialized`, `OBSBOT device detected: ...`, five `OBSBOT take control: <step>: ok` lines, and **no** `avf: lockForConfiguration failed` warning; the Camera section status shows In control with the serial/firmware. (Operator eyeball: Madison confirms the status line and that the camera stays still — no on-device AI fighting the app.)

- [ ] **Step 5: Ledger + wrap-up commit (docs only if gates forced edits)**

```bash
git status --short
```

Expected: clean (or commit any gate-forced fixes with the owning task's scope in the message).
