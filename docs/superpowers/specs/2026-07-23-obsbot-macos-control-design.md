# OBSBOT Camera Control on macOS — Design

**Date:** 2026-07-23
**Status:** Approved (Madison, 2026-07-23)
**Prior art:** `crates/wc-core/src/input/obsbot/` (Windows backend, 2026-07),
`docs/runbooks/obsbot.md`, the LeapC vendored-SDK link/rpath conventions in
`.cargo/config.toml` and `crates/wc-core/build.rs`.

## Problem

The OBSBOT control worker — the code that disables the camera's on-device AI
tracking and gesture control, recenters the gimbal, selects widest FOV, and
re-asserts auto exposure so the camera stops fighting WaveConductor's own
MediaPipe tracking — has a real backend on Windows only. On macOS the
`platform/stub.rs` facade reports `NoDevice` forever, so a dev session or
party gig on the MacBook Pro needs OBSBOT Center running (or pre-configured)
to keep the camera's AI out of the loop. The 2026-07-23 live session also
showed OBSBOT Center holding the camera's AVFoundation configuration lock,
which blocks the app's idle capture-throttle.

Everything needed for a macOS port is already in-tree:

- `vendor/libdev/macos/{arm64,x86_64}-release/libdev.dylib` — vendored SDK
  binaries, install name `@rpath/libdev.dylib`, minimum macOS 11.0.
- `vendor/libdev/shim/obsbot_shim.{h,cpp}` — the extern "C" facade, plain
  C++17 with no Windows-specific code (the MSVC flags in build.rs are
  `flag_if_supported` and no-op on clang).
- `platform/windows.rs` — the worker thread, poll loop, command handling,
  and hardware smoke tests are platform-neutral Rust; only the `ffi` extern
  block and the build-time link are SDK-facing, and the C ABI is identical
  on both platforms.

## Decision (Approach A of three considered)

Promote the Windows backend to a shared **libdev backend** compiled on both
Windows and macOS; keep the stub for everything else (Linux has no
deployment). Alternatives rejected: a separate `macos.rs` sibling
(duplicates ~600 lines of identical worker logic for zero behavioral
divergence) and runtime `dlopen` via `libloading` (new dependency, second
unsafe FFI surface, and the dylib is vendored in-tree so link-time
availability is guaranteed).

## Design

### 1. Module restructure

- `git mv crates/wc-core/src/input/obsbot/platform/windows.rs` →
  `platform/libdev.rs`. Contents unchanged except module docs (the backend
  is now "any OS that links the vendored libdev SDK").
- `platform/mod.rs` cfg re-point:
  `#[cfg(any(target_os = "windows", target_os = "macos"))] pub mod libdev;`
  re-exported as `spawn_worker` / `WorkerHandle`;
  `#[cfg(not(any(target_os = "windows", target_os = "macos")))]` keeps the
  stub. Facade contract (two exported names) unchanged.
- The stub-assertion test `facade_is_noop_off_windows` (in
  `obsbot/mod.rs` tests) is renamed `facade_is_noop_on_unsupported_platforms`
  and re-gated to `#[cfg(not(any(target_os = "windows", target_os = "macos")))]`.
- Doc sweep: `platform/mod.rs`'s "Windows-only by design" rationale and
  `obsbot/mod.rs`'s "Real device IO is **Windows-only**" module doc are
  rewritten; any user-visible Windows-only wording in `section.rs` and the
  runbooks is updated in the same pass.
- No changes to `WorkerCommand`, `ObsbotStatus`, `ControlSteps`, framing,
  or the settings dock section.

### 2. Build wiring

- `crates/wc-core/build.rs`: the obsbot section runs on
  `any(target_os = "windows", target_os = "macos")` (still gated on
  `CARGO_FEATURE_OBSBOT_CAMERA_CONTROL`). Shared shim compile via `cc`
  (`.cpp(true)`, `.std("c++17")`, `.debug(false)` — the MSVC CRT rationale
  keeps its comment; on clang `.debug(false)` merely skips `-g` on the shim,
  which is fine). Per-OS link half:
  - Windows (unchanged): link-search `vendor/libdev/windows/win64-release`,
    `rustc-link-lib=libdev`, DLL staging into `target/<profile>/{,deps}`.
  - macOS (new): link-search
    `vendor/libdev/macos/<arch>-release` where `<arch>` maps from
    `CARGO_CFG_TARGET_ARCH` (`aarch64` → `arm64`, `x86_64` → `x86_64`);
    `cargo:rustc-link-lib=dylib=dev`; `rerun-if-changed` on the dylib. No
    staging — macOS resolves at runtime via rpath, not adjacent files.
- `.cargo/config.toml`: two new rpath entries per Apple target, mirroring
  the LeapC pattern (binaries at `target/<profile>/` are two `..` from repo
  root; `examples/` and `deps/` are three):
  - `aarch64-apple-darwin`:
    `@executable_path/../../vendor/libdev/macos/arm64-release` and
    `@executable_path/../../../vendor/libdev/macos/arm64-release`.
  - `x86_64-apple-darwin`: same with `x86_64-release`.
- Feature defaults (decided 2026-07-23): `waveconductor/Cargo.toml` adds
  `"obsbot-camera-control"` to the `wc-core` features in **both** target
  tables — macOS (`thermal-sensor`, `thermal-sensor-macos`, +) and Windows
  (`thermal-sensor`, `thermal-sensor-windows`, +). `cargo rund` and the
  kiosk build get camera control with no flag; the persisted
  `ObsbotSettings::take_control` toggle is the runtime off-switch.

### 3. Deployment and docs

- `xtask/src/bundle/mac.rs`: stage `libdev.dylib` into the app bundle the
  same way LeapC is staged and signed (the `@executable_path/../lib` rpath
  entry already anticipates a bundled dylib dir). Follow whatever
  staging/signing treatment `bundle-mac` gives LeapC verbatim.
- `docs/runbooks/kiosk.md`: the OBSBOT bullet's "when built with
  `obsbot-camera-control`" phrasing becomes "by default" (there is no
  explicit build-command line to change; the feature arrives via the
  target table).
- `docs/runbooks/obsbot.md`: macOS is a supported control platform; add the
  macOS hardware-smoke invocations; note OBSBOT Center coexistence on macOS
  (it holds the AVFoundation configuration lock while running — quit it for
  kiosk-representative runs; the app's control worker replaces its job).
- CI: no new jobs, but two workflow-env fixes ride along (found in the
  2026-07-23 adversarial review). The workflow-level
  `RUSTFLAGS: "-D warnings"` **overrides** `.cargo/config.toml`'s
  per-target rpath rustflags (ci.yml documents this itself), so macOS test
  binaries resolve vendored dylibs via `DYLD_FALLBACK_LIBRARY_PATH` — the
  libdev vendor dir must join the LeapC entry there in **both** ci.yml and
  release.yml, or every wc-core test binary dies in dyld at nextest list
  time. Separately, the mac release jobs export a non-empty `RUSTFLAGS`
  (compiler-rt `-L`), which strips **all** baked rpaths from release
  binaries — a pre-existing latent break for the bundled LeapC dylib that
  the port fixes by appending the two bundle rpaths
  (`@executable_path/../lib`, `@loader_path`) to those exports.
- Corrected claim: the SDK has never actually *initialized* on any CI
  runner — the hardware tests are `#[ignore]`d and no test drives
  `Startup` through the plugin. Windows CI's green history proves
  compile + link + DLL load, and that remains the proven surface after
  the port. Any future test that spawns the worker on a runner is
  unprecedented on both platforms.

### 4. Risks and acceptance

Two unknowns, both resolved by hardware smoke on the M1 MBP with the Tiny 2
Lite attached:

1. **TCC permissions.** Expected: none — the control channel is device
   control, not capture. If macOS prompts anyway, document the grant in
   `docs/runbooks/obsbot.md`.
2. **Dylib code signature.** The vendored dylib is Remo-signed; a git-vendored
   file carries no quarantine xattr, so ad-hoc dev binaries should load it
   directly. Bundle signing (if any) follows the LeapC treatment.

Acceptance gate, in order:

1. `cargo test -p wc-core --features obsbot-camera-control obsbot_hardware_smoke -- --ignored --nocapture`
   passes on the MBP (gimbal physically recenters; `AI_OFF` + `GESTURE_OFF`
   both achieved).
2. `cargo test -p wc-core --features obsbot-camera-control obsbot_hardware_framing -- --ignored --nocapture`
   passes (gimbal tilts/pans, zooms, FOV narrows, recenters; clean restore
   on worker shutdown).
3. A `cargo rund` session (OBSBOT Center closed) shows `InControl` in the
   Camera section status, and the capture-throttle `lockForConfiguration`
   warning does not appear.
4. All CI gates green (fmt, clippy `--all-features`, nextest, doc, deny,
   check-secrets) on macOS locally; the Windows kiosk build is verified at
   the next Windows checkpoint (target-table change is compile-time only).

Failure semantics unchanged: a run that misses `AI_OFF`/`GESTURE_OFF`
publishes `Failed` with the existing loud warning pointing at
`docs/runbooks/obsbot.md`.

## Decision — SDK redistribution posture (resolved 2026-07-23)

`vendor/libdev` ships no license file and `dev.hpp` carries an internal
"not open to the public" marker; `docs/runbooks/obsbot.md` scopes use to
local/gig hardware and says redistribution terms must be clarified with
OBSBOT before any public release ships the SDK. Today's escape hatch is
that feature-off builds ship no SDK bits, and defaulting the feature on
closes that hatch: every bundle (including the published Promote Alpha
zips) embeds `libdev.dylib`/`libdev.dll`. **Madison's decision
(2026-07-23): accepted for now** — the project is pre-release and its
artifacts serve her own hardware. The runbook's license section is
updated to record the acceptance and its boundary: revisit and clear
terms with OBSBOT before any public or client-facing distribution.

## Out of scope

- Linux backend (dylibs are vendored but no Linux deployment exists).
- Gimbal-follow autopilot / auto-framing beyond the existing manual
  framing commands (tracked separately; see the outdoor-tracking analysis).
- x86_64 macOS hardware validation (wired by construction, untested — no
  Intel Mac in the fleet).
