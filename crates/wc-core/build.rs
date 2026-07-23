//! Build-time linker setup for the vendored native SDKs on Windows: `LeapC`
//! (hand tracking) and, behind the `obsbot-camera-control` feature, the OBSBOT
//! `libdev` camera-control SDK.
//!
//! ## `LeapC`
//!
//! `wc-core` is the crate that owns the `leaprs` dependency (behind the
//! `hand-tracking-gestures` feature). `leaprs`'s own build script emits the
//! `-lLeapC` link directive plus a search path derived from `LEAPSDK_LIB_PATH`,
//! but it resolves that path *relative to its own crate directory in the cargo
//! registry* — and the workspace `.cargo/config.toml` sets `LEAPSDK_LIB_PATH`
//! (non-forced) to the macOS vendor dir as the primary-platform default. On a
//! fresh Windows checkout that default points at a directory with no
//! `LeapC.lib`, so every binary that links `leaprs` — the app, wc-core's own
//! test and example binaries, and dependents like wc-sketches — fails to link
//! with unresolved `Leap*` externals.
//!
//! Emitting an absolute, host-correct search path here fixes all of them at
//! once: a build script's `rustc-link-search` is inherited by every binary that
//! transitively links this crate, exactly as `leaprs`'s own directives are. The
//! path is built from `CARGO_MANIFEST_DIR`, so it is independent of where the
//! repo is cloned. We deliberately emit *only* the search path, not
//! `-lLeapC` — `leaprs` already emits the lib when its feature is enabled, so
//! this stays a no-op extra `-L` when hand tracking is compiled out.
//!
//! ## OBSBOT `libdev` (`obsbot-camera-control` feature)
//!
//! libdev's API is C++ (classes, `std::string`, `std::function`), so bindgen
//! cannot consume it. `vendor/libdev/shim/obsbot_shim.{h,cpp}` is a
//! hand-written extern "C" facade; this script compiles it with the `cc`
//! crate and links the vendored SDK binary for whichever of Windows or macOS
//! is building — both platforms run with an OBSBOT attached (the Windows
//! kiosk; the macOS dev/party rig). The Rust module
//! (`wc_core::input::obsbot`) compiles a no-op facade on Linux, so CI's
//! `--all-features` on that runner never touches a C++ toolchain or a libdev
//! binary here.
//!
//! On Windows, the runtime DLLs (`libdev.dll`, `w32-pthreads.dll`) are staged
//! into `target/<profile>/` and `target/<profile>/deps/` so both the app exe
//! and workspace test binaries resolve them via adjacent-file /
//! link-search-path discovery (the same convention as `LeapC.dll`, which
//! `crates/waveconductor/build.rs` stages for the app). macOS resolves the
//! linked `libdev.dylib` at runtime via the vendor-dir rpath entries in
//! `.cargo/config.toml` instead — nothing is staged for dev/test builds; the
//! app bundle is the one place a copy is made, by `cargo xtask bundle-mac`.
//!
//! LeapC resolution on non-Windows targets is unchanged: the
//! `.cargo/config.toml` rpath stanzas + `leaprs`'s default path, as before.
//! Linux compiles no OBSBOT shim at all; macOS does, per the OBSBOT section
//! above.

// A build script signals failure by panicking — the sanctioned way to fail a
// build when a setup-time invariant (a vendored file that must exist, a copy
// that must succeed) is violated. Scope the allow to this compilation unit;
// runtime code keeps the strict lints.
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "build scripts fail the build by panicking; setup-time invariants, not runtime paths"
)]

fn main() {
    #[cfg(target_os = "windows")]
    {
        let leapc_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/leapc/windows-x86_64");
        println!("cargo:rerun-if-changed=../../vendor/leapc/windows-x86_64/LeapC.lib");
        println!("cargo:rustc-link-search=native={}", leapc_dir.display());
    }

    // OBSBOT libdev shim — Windows/macOS + `obsbot-camera-control` only. The
    // feature check is an env probe (not a cfg) because build scripts see
    // features via CARGO_FEATURE_*; the cfg(target_os) here refers to the
    // *host*. The CARGO_CFG_TARGET_OS probe additionally guards cross builds
    // from this host — `cargo check --target wasm32-unknown-unknown
    // --all-features` (the web roadmap) must not compile or link the SDK,
    // and without the probe the macOS link half would panic on the wasm
    // arch.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if matches!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("windows") | Ok("macos")
    ) && std::env::var_os("CARGO_FEATURE_OBSBOT_CAMERA_CONTROL").is_some()
    {
        build_obsbot_shim();
    }
}

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

/// Link the vendored import library and stage the runtime DLLs beside every
/// binary this target directory produces (Windows resolves DLLs adjacent to
/// the exe; see the module doc).
#[cfg(target_os = "windows")]
fn link_libdev(libdev_root: &std::path::Path) {
    let lib_dir = libdev_root.join("windows/win64-release");

    println!("cargo:rerun-if-changed=../../vendor/libdev/windows/win64-release/libdev.lib");

    // The import library for libdev.dll. `cc` already emitted the link
    // directives for the static shim itself.
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=libdev");

    // Stage the runtime DLLs. Test binaries run from target/<profile>/deps and
    // the app exe from target/<profile>; Windows resolves DLLs adjacent to the
    // exe (and cargo/nextest additionally put the link-search dir above on
    // PATH, which covers stray layouts). OUT_DIR is
    // `target/<profile>/build/<crate>-<hash>/out`, so the profile dir is the
    // 4th ancestor.
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let profile_dir = std::path::Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has at least 4 ancestors")
        .to_path_buf();
    for dll in ["libdev.dll", "w32-pthreads.dll"] {
        println!("cargo:rerun-if-changed=../../vendor/libdev/windows/win64-release/{dll}");
        let src = lib_dir.join(dll);
        for dst_dir in [profile_dir.clone(), profile_dir.join("deps")] {
            std::fs::create_dir_all(&dst_dir).expect("create DLL staging dir");
            stage_dll(&src, &dst_dir.join(dll));
        }
    }
}

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

/// Copy a runtime DLL into a staging dir, tolerating the Windows quirk that a
/// loaded DLL cannot be overwritten: when a same-size copy is already in place
/// (the vendored DLLs never change in place), or the copy fails but a previous
/// staging left a usable file, the build proceeds — a running app/test process
/// holding the DLL must not wedge every parallel build. A missing destination
/// still fails the build, because binaries would crash at load.
#[cfg(target_os = "windows")]
fn stage_dll(src: &std::path::Path, dst: &std::path::Path) {
    let same_len = matches!(
        (std::fs::metadata(src), std::fs::metadata(dst)),
        (Ok(s), Ok(d)) if s.len() == d.len()
    );
    if same_len {
        return;
    }
    if let Err(err) = std::fs::copy(src, dst) {
        assert!(
            dst.exists(),
            "Failed to copy {} to {}: {}",
            src.display(),
            dst.display(),
            err
        );
        println!(
            "cargo:warning=could not refresh {} ({}); keeping the existing staged copy",
            dst.display(),
            err
        );
    }
}
