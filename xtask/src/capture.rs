//! `cargo xtask capture <scenario>` — orchestrate a deterministic capture run,
//! compute metrics, diff baselines, and report.
//!
//! Independent of `wc-core`/`wc-sketches`: this launches the pre-built DEBUG
//! `waveconductor` binary (`target/debug/waveconductor`), teeing its output to
//! `<dir>/app.log`, then reads the PNGs + `run.json` the app wrote. It does NOT
//! build the app — build it first with `cargo build -p waveconductor` (a
//! separate, watchable step); capture fails fast if the binary is missing, so a
//! cold build can never be misattributed to the launch-timeout safety net.
//!
//! ## Signal flow
//!
//! 1. Resolve `<scenario>` from `tests/visual/scenarios.toml`.
//! 2. Assemble env: `WAVECONDUCTOR_START_SKETCH`, `WAVECONDUCTOR_HAND_PROVIDER`,
//!    `WAVECONDUCTOR_CONFIG_DIR` (fresh temp unless pinned), `WC_DEBUG_*`
//!    (scenario + `--debug` overrides), and `WC_CAPTURE` (the capture schedule).
//! 3. Launch the DEBUG binary; tee stdout+stderr to `<dir>/app.log`; enforce a
//!    wall-clock timeout safety net (the app self-exits via `AppExit`).
//! 4. Read the PNGs + `run.json`; compute metrics (`metrics`) -> `metrics.json`;
//!    diff each frame vs its committed baseline (`diff`).
//! 5. Report: human table (default) or `--json` (per-frame metrics + diff
//!    verdict + paths + which frames to open). Exit 0 on pass / nonzero on
//!    regression.

#![allow(clippy::print_stdout, reason = "xtask is a CLI; printing is its job")]

pub mod diff;
pub mod metrics;
pub mod scenarios;

use std::collections::BTreeMap;
// Both `Write` traits are imported anonymously: `io::Write` for `write_all` to
// files, `fmt::Write` for `write!` into a `String`. Trait method resolution
// selects the right one by receiver type, so the `_` aliases never collide.
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Args as ClapArgs;

use diff::diff_frames;
use metrics::{global_std, luma_from_mean, region_mean, FrameMetrics, Region};
use scenarios::{Scenario, Scenarios};

use crate::util::{
    git_short_commit, resolve_built_binary, spawn_log_tee, warn_if_stale, workspace_root,
};

/// Subcommand name used in this module's operator-facing error messages.
const TOOL: &str = "capture";

/// Per-pixel max-channel delta above which a pixel counts as changed.
const PIXEL_THRESHOLD: u8 = 12;

/// Mean-abs-diff tolerance (0..=255) below which a frame passes the baseline.
const DIFF_TOLERANCE: f64 = 6.0;

/// Mean-luma floor (0..=255 Rec. 601) below which a frame is treated as
/// near-zero-luminance ("all-black") by the `--update-baselines` guard. This
/// is the signature of an unrendered/backgrounded capture (see the black-frame
/// trap documented in `tests/visual/CLAUDE.md`), not a legitimately dark
/// sketch frame — real sketch output always has some non-zero structure even
/// at its darkest.
const BLACK_LUMA_THRESHOLD: f64 = 1.0;

/// Wall-clock safety timeout for the launched app (seconds). The app normally
/// self-exits via `AppExit` after the last scheduled frame; this is the net for
/// the case where a screenshot observer never fires.
const LAUNCH_TIMEOUT_SECS: u64 = 90;

/// Relative tolerance for the captured-vs-requested window aspect check
/// (1% = `0.01`). The comparison is deliberately on *aspect*, not on absolute
/// size: a window is created in LOGICAL pixels and the framebuffer we screenshot
/// is physical, so an honest capture on a `2x` (Retina / high-DPI) display comes
/// back at exactly twice the requested width and height — same aspect. A window
/// the OS had to *clamp* to fit the display shrinks one axis more than the
/// other, which always moves the aspect. The tolerance absorbs the ±1 px
/// rounding a fractional scale factor (e.g. Windows at `1.5x`) can introduce.
const ASPECT_TOLERANCE: f64 = 0.01;

/// The app's startup window size in logical pixels when a scenario pins no
/// `resolution`. Mirrors `window_resolution()` in
/// `crates/waveconductor/src/main.rs`; kept here so the window guard can check
/// unpinned scenarios too. If the app's default ever changes, change it here as
/// well — a stale value shows up as a loud aspect mismatch, not a silent pass.
const DEFAULT_WINDOW: (u32, u32) = (1280, 720);

/// Arguments for the capture subcommand.
#[derive(ClapArgs)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "clap CLI flags — each bool is an independent --flag toggle, not packed state"
)]
pub struct Args {
    /// Scenario name from `tests/visual/scenarios.toml`. Omit with `--list`.
    pub scenario: Option<String>,
    /// Copy the freshly-captured frames into the baseline dir (no tolerance
    /// diff gate — but see `--allow-black`, which *is* a gate).
    #[arg(long)]
    pub update_baselines: bool,
    /// Let `--update-baselines` bless near-zero-luminance (all-black) frames.
    /// Only pass this when black is genuinely the correct rendered output;
    /// otherwise an all-black frame almost always means the app window wasn't
    /// foregrounded during capture (see `tests/visual/CLAUDE.md`).
    #[arg(long)]
    pub allow_black: bool,
    /// Emit machine-readable JSON instead of the human table.
    #[arg(long)]
    pub json: bool,
    /// Launch the scenario for hands-on inspection (no capture); quit after N
    /// seconds (default 10). Runs the normal variable-dt clock.
    #[arg(long, value_name = "SECS", num_args = 0..=1, default_missing_value = "10")]
    pub watch: Option<u64>,
    /// List available scenarios and exit.
    #[arg(long)]
    pub list: bool,
    /// Ad-hoc `WC_DEBUG_*` overrides as `KEY=VAL` (KEY without the prefix).
    #[arg(long = "debug", value_name = "KEY=VAL")]
    pub debug: Vec<String>,
}

/// Execute the capture subcommand.
pub fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let scenarios = load_scenarios(&root)?;

    if args.list {
        print_list(&scenarios, args.json);
        return Ok(());
    }

    let name = args
        .scenario
        .as_deref()
        .ok_or("capture: a scenario name is required (or use --list)")?;
    let scenario = scenarios
        .get(name)
        .ok_or_else(|| format!("capture: unknown scenario {name:?}; try --list"))?;

    let out_dir = root.join("target").join("capture").join(name);
    std::fs::create_dir_all(&out_dir)?;

    if let Some(secs) = args.watch {
        return run_watch(&root, scenario, secs);
    }

    launch(&root, name, scenario, &out_dir, &args.debug)?;

    let report = analyze(&root, name, scenario, &out_dir)?;

    // Window-clamp guard: the captured PNGs are the ground truth for what the
    // window actually was. A mismatch invalidates the whole run, so it is
    // checked before baselines can be blessed and before the pass verdict.
    let window = check_window(scenario, &report);
    let clamped = window.as_ref().filter(|w| !w.aspect_ok);

    if args.update_baselines {
        // Blessing a clamped capture poisons the baseline exactly like an
        // all-black frame does (see `update_baselines`): every honest future
        // capture would then diff against a wrong-shaped PNG.
        if let Some(w) = clamped {
            return Err(window_clamp_error(name, w).into());
        }
        update_baselines(&root, name, scenario, &out_dir, &report, args.allow_black)?;
        if args.json {
            println!("{{\"scenario\":\"{name}\",\"updated_baselines\":true}}");
        } else {
            println!("Updated baselines for {name}.");
        }
        return Ok(());
    }

    let passed = report.frames.iter().all(|f| f.passed) && clamped.is_none();
    if args.json {
        print_json_report(name, &out_dir, &report, window.as_ref(), passed);
    } else {
        print_human_report(name, &report, window.as_ref());
    }
    if let Some(w) = clamped {
        return Err(window_clamp_error(name, w).into());
    }
    if passed {
        Ok(())
    } else {
        Err(format!("capture: {name} regressed beyond tolerance").into())
    }
}

/// Assemble the `WC_CAPTURE` env value for a scenario + output dir.
///
/// `name` and `commit` are threaded into the schedule string so the app can
/// record them in `run.json` for provenance (the app is otherwise unaware of
/// the scenario name or the repo state). `commit` is `None` outside a git repo.
pub fn build_wc_capture(
    name: &str,
    scenario: &Scenario,
    out_dir: &Path,
    commit: Option<&str>,
) -> String {
    let frames = scenario
        .frames
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut wc = format!("dir={};frames={}", out_dir.display(), frames);
    if let Some(dt) = scenario.dt {
        // `write!` to a `String` is infallible; the discard documents that.
        let _ = write!(wc, ";dt={dt}");
    }
    let _ = write!(wc, ";scenario={name}");
    if let Some(commit) = commit {
        let _ = write!(wc, ";commit={commit}");
    }
    wc
}

/// Merge CLI `--debug KEY=VAL` overrides over a scenario's `debug` table. CLI
/// values win; new keys are added.
pub fn merge_debug(scenario: &Scenario, overrides: &[String]) -> BTreeMap<String, String> {
    let mut merged = scenario.debug.clone();
    for ov in overrides {
        if let Some((k, v)) = ov.split_once('=') {
            merged.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    merged
}

/// Turn a merged debug table into `(WC_DEBUG_<KEY>, VAL)` env pairs.
pub fn debug_env_pairs(merged: &BTreeMap<String, String>) -> Vec<(String, String)> {
    merged
        .iter()
        .map(|(k, v)| (format!("WC_DEBUG_{k}"), v.clone()))
        .collect()
}

/// The `WC_CAPTURE_RESOLUTION` value for a scenario: `"WxH"` when the scenario
/// pins a window resolution, `None` when it relies on the app's 1280x720
/// default. The env var is honoured by debug builds only (the override is
/// `#[cfg(debug_assertions)]`-gated in the app, like the rest of `WC_CAPTURE`).
pub fn resolution_env(scenario: &Scenario) -> Option<String> {
    scenario.resolution.map(|[w, h]| format!("{w}x{h}"))
}

/// The window size a scenario asks for, in logical pixels: its `resolution`
/// field when present, otherwise the app's [`DEFAULT_WINDOW`]. This is the
/// reference the window guard compares the captured framebuffer against.
pub fn requested_window(scenario: &Scenario) -> (u32, u32) {
    scenario.resolution.map_or(DEFAULT_WINDOW, |[w, h]| (w, h))
}

/// Aspect ratio (width / height) of a window/framebuffer size. `None` for a
/// zero height, which has no defined aspect (a degenerate size the caller
/// treats as a mismatch rather than dividing by zero).
fn aspect_ratio(size: (u32, u32)) -> Option<f64> {
    if size.1 == 0 {
        return None;
    }
    Some(f64::from(size.0) / f64::from(size.1))
}

/// Whether a captured framebuffer preserves the requested window's aspect
/// ratio within `tolerance` (a relative fraction of the requested aspect, e.g.
/// `0.01` for 1%).
///
/// This is the whole window-clamp guard in one predicate. It must be
/// aspect-based rather than size-based because the requested size is LOGICAL
/// and the captured framebuffer is PHYSICAL: on a `2x` display an honest
/// capture of a 1280x720 request comes back 2560x1440 (aspect unchanged), while
/// a window the OS shrank to fit the display changes shape (the observed case:
/// a 1080x1920 request captured as 2160x1976 — the height was clamped, the
/// width was not, and a 0.5625 portrait aspect became a 1.09 near-square).
///
/// A zero dimension on either side is reported as a mismatch: there is no
/// aspect to compare, and a zero-sized request or capture is itself a fault
/// worth failing on.
pub fn aspect_matches(requested: (u32, u32), captured: (u32, u32), tolerance: f64) -> bool {
    // Screen out degenerate sizes on the integers, before any division: this
    // also guarantees `want` below is non-zero, so the relative error is well
    // defined.
    if requested.0 == 0 || requested.1 == 0 || captured.0 == 0 || captured.1 == 0 {
        return false;
    }
    let (Some(want), Some(got)) = (aspect_ratio(requested), aspect_ratio(captured)) else {
        return false;
    };
    // Relative error, so the tolerance means the same thing for a 0.5625
    // portrait aspect as for a 1.7778 landscape one.
    (got - want).abs() / want <= tolerance
}

// ---- private orchestration helpers --------------------------------------

/// Load `tests/visual/scenarios.toml`.
fn load_scenarios(root: &Path) -> Result<Scenarios, Box<dyn std::error::Error>> {
    let path = root.join("tests").join("visual").join("scenarios.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("capture: cannot read {}: {e}", path.display()))?;
    Ok(toml::from_str(&text)?)
}

/// Launch the debug binary with scenario env + capture schedule, teeing
/// stdout+stderr to `<dir>/app.log`, enforcing a wall-clock timeout.
fn launch(
    root: &Path,
    name: &str,
    scenario: &Scenario,
    out_dir: &Path,
    cli_debug: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let commit = git_short_commit(root);
    let binary = resolve_built_binary(root, TOOL)?;
    warn_if_stale(&binary, root);
    let mut cmd = Command::new(&binary);
    cmd.current_dir(root)
        .env("WAVECONDUCTOR_START_SKETCH", &scenario.sketch)
        .env("WAVECONDUCTOR_HAND_PROVIDER", &scenario.provider)
        .env(
            "WC_CAPTURE",
            build_wc_capture(name, scenario, out_dir, commit.as_deref()),
        );

    // Optional per-scenario window resolution (portrait scenarios etc.); the
    // app's debug-only window override reads it at startup.
    if let Some(res) = resolution_env(scenario) {
        cmd.env("WC_CAPTURE_RESOLUTION", res);
    }

    // Config isolation: a fresh temp dir for `config = "clean"`, else a pinned
    // path. The temp dir is created under the output dir so it is inspectable.
    if scenario.config == "clean" {
        let clean = out_dir.join("clean-config");
        std::fs::create_dir_all(&clean)?;
        cmd.env("WAVECONDUCTOR_CONFIG_DIR", &clean);
    } else {
        cmd.env("WAVECONDUCTOR_CONFIG_DIR", &scenario.config);
    }

    for (k, v) in debug_env_pairs(&merge_debug(scenario, cli_debug)) {
        cmd.env(k, v);
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;

    // Drain both pipes into app.log (shared with soak-test; see `util`).
    let log_path = out_dir.join("app.log");
    let handles = spawn_log_tee(&mut child, &log_path)?;

    // Wall-clock timeout safety net (the app self-exits via AppExit normally).
    let start = std::time::Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed().as_secs() > LAUNCH_TIMEOUT_SECS {
            let _ = child.kill();
            return Err(format!(
                "capture: app did not exit within {LAUNCH_TIMEOUT_SECS}s; see {}",
                log_path.display()
            )
            .into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

/// `--watch`: launch for hands-on inspection (no `WC_CAPTURE`), kill after N s.
fn run_watch(
    root: &Path,
    scenario: &Scenario,
    secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let binary = resolve_built_binary(root, TOOL)?;
    warn_if_stale(&binary, root);
    let mut cmd = Command::new(&binary);
    cmd.current_dir(root)
        .env("WAVECONDUCTOR_START_SKETCH", &scenario.sketch)
        .env("WAVECONDUCTOR_HAND_PROVIDER", &scenario.provider);
    // Match the capture run's window size so what you watch is what captures.
    if let Some(res) = resolution_env(scenario) {
        cmd.env("WC_CAPTURE_RESOLUTION", res);
    }
    let mut child = cmd.spawn()?;
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < secs {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    Ok(())
}

/// One frame's report row.
struct FrameReport {
    frame: u32,
    metrics: FrameMetrics,
    mean_abs_diff: Option<f64>,
    passed: bool,
    /// Decoded PNG dimensions `(width, height)` in physical pixels — the
    /// ground truth for how big the app's window actually was, read back from
    /// the file rather than trusted from the launch env (see [`check_window`]).
    size: (u32, u32),
    current_path: PathBuf,
    baseline_path: Option<PathBuf>,
}

/// Aggregate report.
struct Report {
    frames: Vec<FrameReport>,
}

/// Read PNGs + run.json, compute metrics + baseline diffs.
fn analyze(
    root: &Path,
    name: &str,
    scenario: &Scenario,
    out_dir: &Path,
) -> Result<Report, Box<dyn std::error::Error>> {
    let baseline_dir = root
        .join("tests")
        .join("visual")
        .join("baselines")
        .join(name);
    let mut frames = Vec::new();
    let mut prev: Option<image::RgbaImage> = None;

    // Write metrics.json alongside the report.
    let mut metrics_out: Vec<FrameMetrics> = Vec::new();

    for &frame in &scenario.frames {
        let current_path = out_dir.join(format!("frame_{frame:04}.png"));
        let current = image::open(&current_path)
            .map_err(|e| format!("capture: cannot read {}: {e}", current_path.display()))?
            .to_rgba8();

        let delta_prev = prev
            .as_ref()
            .map(|p| metrics::frame_mean_abs_delta(p, &current));
        let fm = FrameMetrics {
            frame,
            full_mean: region_mean(&current, Region::Full),
            center_mean: region_mean(&current, Region::Center),
            global_std: global_std(&current),
            delta_prev,
        };
        metrics_out.push(fm.clone());

        let baseline_path = baseline_dir.join(format!("frame_{frame:04}.png"));
        let (mean_abs_diff, passed, baseline_ref) = if baseline_path.exists() {
            let baseline = image::open(&baseline_path)?.to_rgba8();
            let d = diff_frames(&current, &baseline, PIXEL_THRESHOLD);
            (
                Some(d.mean_abs_diff),
                d.passes(DIFF_TOLERANCE),
                Some(baseline_path),
            )
        } else {
            // No baseline yet -> cannot regress; flag for the agent to review.
            (None, true, None)
        };

        frames.push(FrameReport {
            frame,
            metrics: fm,
            mean_abs_diff,
            passed,
            // Straight from the decoded PNG — no extra IO, no extra dependency.
            size: current.dimensions(),
            current_path,
            baseline_path: baseline_ref,
        });
        prev = Some(current);
    }

    let metrics_path = out_dir.join("metrics.json");
    let mut f = std::fs::File::create(&metrics_path)?;
    f.write_all(serde_json::to_string_pretty(&metrics_out)?.as_bytes())?;

    Ok(Report { frames })
}

/// Outcome of the requested-vs-realized window check for a capture run.
struct WindowCheck {
    /// Window size the scenario asked for, in logical pixels.
    requested: (u32, u32),
    /// Realized framebuffer size read back from a captured PNG, in physical
    /// pixels. On a high-DPI display this is legitimately a whole multiple of
    /// `requested`.
    captured: (u32, u32),
    /// Frame index `captured` was read from — the first offending frame when
    /// the aspect is wrong, otherwise the first captured frame.
    frame: u32,
    /// True when `captured` preserves `requested`'s aspect within
    /// [`ASPECT_TOLERANCE`].
    aspect_ok: bool,
}

/// Compare what the scenario asked the window to be against what the app
/// actually rendered, using the captured PNGs as ground truth.
///
/// The OS silently clamps a window that does not fit the attached display, and
/// nothing in the launch path notices: the app starts, renders, screenshots,
/// and exits cleanly at the wrong size. Every `*-portrait` scenario is exposed
/// to this — a 1080x1920 request needs a 2160x3840 physical framebuffer at
/// `2x`, which no ordinary display can host — so without this check a portrait
/// regression test can validate a near-square window and report PASS.
///
/// Returns `None` only when the report has no frames at all (a scenario's
/// `frames` list is non-empty by schema, so this is defensive).
fn check_window(scenario: &Scenario, report: &Report) -> Option<WindowCheck> {
    let requested = requested_window(scenario);
    // Prefer the first *offending* frame: a mid-run window change (a resize, a
    // display re-enumerating) would otherwise be masked by a good frame 0.
    let offender = report
        .frames
        .iter()
        .find(|f| !aspect_matches(requested, f.size, ASPECT_TOLERANCE));
    // The frame whose realized size the check reports: the offender when there
    // is one, else the first frame as the representative size.
    let sample = offender.or_else(|| report.frames.first())?;
    Some(WindowCheck {
        requested,
        captured: sample.size,
        frame: sample.frame,
        aspect_ok: offender.is_none(),
    })
}

/// Render a size as `WxH (aspect A)` for operator-facing output; the aspect
/// reads `n/a` for a degenerate zero-height size.
fn fmt_size(size: (u32, u32)) -> String {
    let aspect = aspect_ratio(size).map_or_else(|| "n/a".to_string(), |a| format!("{a:.4}"));
    format!("{}x{} (aspect {aspect})", size.0, size.1)
}

/// The operator-facing failure for a clamped (wrong-aspect) capture: what was
/// asked for, what was rendered, the likely cause, and the fix.
fn window_clamp_error(name: &str, check: &WindowCheck) -> String {
    let (rw, rh) = check.requested;
    // Relative aspect error as a percentage, for a number the operator can
    // weigh against the tolerance. `n/a` only for a degenerate zero dimension.
    let error_pct = match (aspect_ratio(check.requested), aspect_ratio(check.captured)) {
        (Some(want), Some(got)) if want > 0.0 => {
            format!("{:.1}%", 100.0 * (got - want).abs() / want)
        }
        _ => "n/a".to_string(),
    };
    let tolerance_pct = 100.0 * ASPECT_TOLERANCE;
    // An honest 2x capture, spelled out so the distinction is concrete.
    let (rw2, rh2) = (rw.saturating_mul(2), rh.saturating_mul(2));
    format!(
        "capture: {name} rendered at the wrong aspect ratio — requested {}, captured {} at frame {} \
         (off by {error_pct}, tolerance {tolerance_pct:.0}%). An honest capture on a scaled \
         (Retina / high-DPI) display is an exact multiple of the request — {rw}x{rh} at 2x is \
         {rw2}x{rh2}, same aspect. A CHANGED aspect means the OS clamped the window because it did \
         not fit the attached display, so this run validated a window shape the scenario never \
         asked for and nothing captured under it can be trusted (or blessed as a baseline). Fix: \
         give this scenario a resolution that fits the attached display at its scale factor (halve \
         both dimensions for a 2x display), or run the capture on a display that can host \
         {rw}x{rh} at that scale factor. If the display arrangement changed — a rotated/portrait \
         panel detached, an external unplugged — restore it and re-run.",
        fmt_size(check.requested),
        fmt_size(check.captured),
        check.frame,
    )
}

/// Frame indices from `report` whose mean luma falls below `threshold`
/// (0..=255 Rec. 601) — the near-zero-luminance guard for
/// [`update_baselines`]. Pulled out as a pure function over an already-built
/// `Report` (reusing `full_mean`, computed once in [`analyze`]) so the
/// detection logic is unit-testable without touching disk or the app.
fn near_black_frames(report: &Report, threshold: f64) -> Vec<u32> {
    report
        .frames
        .iter()
        .filter(|f| luma_from_mean(f.metrics.full_mean) < threshold)
        .map(|f| f.frame)
        .collect()
}

/// Copy captured frames into the baseline dir (plain committed PNGs, no LFS).
///
/// Refuses to bless a batch containing a near-zero-luminance ("all-black")
/// frame unless `allow_black` is set: seeding a baseline from an
/// unrendered/backgrounded capture (see the black-frame trap documented in
/// `tests/visual/CLAUDE.md`) would commit a PNG that can never honestly match
/// a correctly-rendered frame, silently reintroducing the exact
/// orphaned-baseline problem this guard exists to prevent.
fn update_baselines(
    root: &Path,
    name: &str,
    scenario: &Scenario,
    out_dir: &Path,
    report: &Report,
    allow_black: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !allow_black {
        let black = near_black_frames(report, BLACK_LUMA_THRESHOLD);
        if !black.is_empty() {
            return Err(format!(
                "capture: refusing to bless {name} baselines — frame(s) {black:?} are near-zero \
                 luminance (all-black, mean luma < {BLACK_LUMA_THRESHOLD}). This is almost always the \
                 app window not being foregrounded during capture, not a real render (see \
                 tests/visual/CLAUDE.md); re-run in the foreground, or pass --allow-black if black is \
                 genuinely the correct rendered output."
            )
            .into());
        }
    }

    let baseline_dir = root
        .join("tests")
        .join("visual")
        .join("baselines")
        .join(name);
    std::fs::create_dir_all(&baseline_dir)?;
    for &frame in &scenario.frames {
        let src = out_dir.join(format!("frame_{frame:04}.png"));
        let dst = baseline_dir.join(format!("frame_{frame:04}.png"));
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("capture: cannot copy baseline {}: {e}", dst.display()))?;
    }
    Ok(())
}

fn print_list(scenarios: &Scenarios, json: bool) {
    if json {
        let names: Vec<String> = scenarios
            .names()
            .into_iter()
            .map(|n| format!("\"{n}\""))
            .collect();
        println!("[{}]", names.join(","));
    } else {
        println!("SCENARIOS");
        for n in scenarios.names() {
            println!("  {n}");
        }
    }
}

fn print_human_report(name: &str, report: &Report, window: Option<&WindowCheck>) {
    println!("CAPTURE {name}");
    // Window line first: when the aspect is wrong every metric below it is
    // describing a window shape the scenario never asked for.
    if let Some(w) = window {
        println!(
            "WINDOW   requested {}  captured {}  {}",
            fmt_size(w.requested),
            fmt_size(w.captured),
            if w.aspect_ok {
                "ok"
            } else {
                "ASPECT MISMATCH (window clamped by the display)"
            },
        );
    }
    println!(
        "{:<8} {:<22} {:<10} {:<10} VERDICT",
        "FRAME", "FULL_MEAN(RGB)", "STD", "DIFF"
    );
    for f in &report.frames {
        let diff = f
            .mean_abs_diff
            .map_or_else(|| "n/a".to_string(), |d| format!("{d:.2}"));
        let verdict = if f.baseline_path.is_none() {
            "NEW (review)"
        } else if f.passed {
            "pass"
        } else {
            "REGRESS (open)"
        };
        println!(
            "{:<8} {:<22} {:<10.2} {:<10} {}",
            f.frame,
            format!(
                "{:.0},{:.0},{:.0}",
                f.metrics.full_mean[0], f.metrics.full_mean[1], f.metrics.full_mean[2]
            ),
            f.metrics.global_std,
            diff,
            verdict,
        );
    }
    let to_open: Vec<String> = report
        .frames
        .iter()
        .filter(|f| !f.passed || f.baseline_path.is_none())
        .map(|f| f.current_path.display().to_string())
        .collect();
    if to_open.is_empty() {
        println!("All frames within tolerance.");
    } else {
        println!("Open & judge these frames:");
        for p in to_open {
            println!("  {p}");
        }
    }
}

fn print_json_report(
    name: &str,
    out_dir: &Path,
    report: &Report,
    window: Option<&WindowCheck>,
    passed: bool,
) {
    // Hand-rolled JSON so the shape is explicit and stable for the agent.
    let mut frames_json = Vec::new();
    for f in &report.frames {
        let diff = f
            .mean_abs_diff
            .map_or_else(|| "null".to_string(), |d| format!("{d:.4}"));
        let baseline = f
            .baseline_path
            .as_ref()
            .map_or_else(|| "null".to_string(), |p| format!("\"{}\"", p.display()));
        frames_json.push(format!(
            "{{\"frame\":{},\"full_mean\":[{:.2},{:.2},{:.2}],\"center_mean\":[{:.2},{:.2},{:.2}],\"global_std\":{:.4},\"mean_abs_diff\":{},\"passed\":{},\"current\":\"{}\",\"baseline\":{}}}",
            f.frame,
            f.metrics.full_mean[0], f.metrics.full_mean[1], f.metrics.full_mean[2],
            f.metrics.center_mean[0], f.metrics.center_mean[1], f.metrics.center_mean[2],
            f.metrics.global_std,
            diff,
            f.passed,
            f.current_path.display(),
            baseline,
        ));
    }
    let open: Vec<String> = report
        .frames
        .iter()
        .filter(|f| !f.passed || f.baseline_path.is_none())
        .map(|f| format!("\"{}\"", f.current_path.display()))
        .collect();
    println!(
        "{{\"scenario\":\"{}\",\"dir\":\"{}\",\"passed\":{},\"window\":{},\"frames\":[{}],\"open_for_review\":[{}]}}",
        name,
        out_dir.display(),
        passed,
        window_json(window),
        frames_json.join(","),
        open.join(","),
    );
}

/// The `window` object of the `--json` report: the requested-vs-realized window
/// geometry and the aspect guard's verdict, so a machine consumer can see a
/// clamped capture as a field rather than having to parse the error text.
/// `null` when there was no frame to measure.
fn window_json(window: Option<&WindowCheck>) -> String {
    let Some(w) = window else {
        return "null".to_string();
    };
    // `null` aspects only for a degenerate zero-height size.
    let fmt = |size: (u32, u32)| {
        aspect_ratio(size).map_or_else(|| "null".to_string(), |a| format!("{a:.4}"))
    };
    format!(
        "{{\"requested\":[{},{}],\"requested_aspect\":{},\"captured\":[{},{}],\"captured_aspect\":{},\"frame\":{},\"aspect_tolerance\":{:.4},\"aspect_ok\":{}}}",
        w.requested.0,
        w.requested.1,
        fmt(w.requested),
        w.captured.0,
        w.captured.1,
        fmt(w.captured),
        w.frame,
        ASPECT_TOLERANCE,
        w.aspect_ok,
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, reason = "expect is appropriate in test code")]

    use super::*;
    use crate::capture::scenarios::Scenario;
    use std::collections::BTreeMap;

    fn scenario() -> Scenario {
        Scenario {
            sketch: "line".into(),
            provider: "synthetic".into(),
            config: "clean".into(),
            debug: BTreeMap::from([("FORCE_G".into(), "8000".into())]),
            frames: vec![30, 60],
            // Digit separators satisfy `clippy::unreadable_literal`; the parsed
            // `f64` value (and thus its formatted string) is unchanged.
            dt: Some(0.016_666_667),
            resolution: None,
        }
    }

    #[test]
    fn resolution_env_is_absent_by_default() {
        assert_eq!(resolution_env(&scenario()), None);
    }

    #[test]
    fn resolution_env_formats_wxh() {
        let mut s = scenario();
        s.resolution = Some([1080, 1920]);
        assert_eq!(resolution_env(&s).as_deref(), Some("1080x1920"));
    }

    #[test]
    fn builds_wc_capture_string() {
        let s = scenario();
        let wc = build_wc_capture(
            "line-synthetic",
            &s,
            std::path::Path::new("target/capture/x"),
            Some("abc1234"),
        );
        assert!(wc.starts_with("dir=target/capture/x;frames=30,60"));
        assert!(wc.contains("dt=0.016666667"));
        assert!(wc.contains("scenario=line-synthetic"));
        assert!(wc.contains("commit=abc1234"));
    }

    #[test]
    fn wc_capture_omits_commit_when_absent() {
        let s = scenario();
        let wc = build_wc_capture("line-synthetic", &s, std::path::Path::new("out"), None);
        assert!(wc.contains("scenario=line-synthetic"));
        assert!(!wc.contains("commit="));
    }

    #[test]
    fn cli_debug_overrides_merge_over_scenario() {
        let s = scenario();
        let overrides = vec!["FORCE_G=4000".to_string(), "DISABLE_SMEAR=1".to_string()];
        let merged = merge_debug(&s, &overrides);
        assert_eq!(merged.get("FORCE_G").map(String::as_str), Some("4000")); // overridden
        assert_eq!(merged.get("DISABLE_SMEAR").map(String::as_str), Some("1")); // added
    }

    #[test]
    fn env_pairs_prefix_wc_debug() {
        let merged = BTreeMap::from([("FORCE_G".to_string(), "8000".to_string())]);
        let pairs = debug_env_pairs(&merged);
        assert!(pairs.contains(&("WC_DEBUG_FORCE_G".to_string(), "8000".to_string())));
    }

    /// A [`FrameReport`] with only `frame` and `metrics.full_mean` set
    /// meaningfully — the two fields [`near_black_frames`] reads. Other
    /// fields are filled with harmless placeholders.
    fn frame_report(frame: u32, full_mean: [f64; 3]) -> FrameReport {
        sized_frame_report(frame, full_mean, (1280, 720))
    }

    /// A [`FrameReport`] with a specific captured `size` — for the window
    /// guard, which reads `size` (and `frame`) and nothing else.
    fn sized_frame_report(frame: u32, full_mean: [f64; 3], size: (u32, u32)) -> FrameReport {
        FrameReport {
            frame,
            metrics: FrameMetrics {
                frame,
                full_mean,
                center_mean: full_mean,
                global_std: 0.0,
                delta_prev: None,
            },
            mean_abs_diff: None,
            passed: true,
            size,
            current_path: PathBuf::from(format!("frame_{frame:04}.png")),
            baseline_path: None,
        }
    }

    #[test]
    fn near_black_frames_flags_only_dark_frames() {
        let report = Report {
            frames: vec![
                frame_report(30, [0.0, 0.0, 0.0]),     // all-black
                frame_report(60, [120.0, 80.0, 60.0]), // normal rendered frame
                frame_report(90, [0.3, 0.2, 0.1]),     // still effectively black
            ],
        };
        assert_eq!(
            near_black_frames(&report, BLACK_LUMA_THRESHOLD),
            vec![30, 90]
        );
    }

    #[test]
    fn near_black_frames_empty_when_all_lit() {
        let report = Report {
            frames: vec![frame_report(30, [10.0, 10.0, 10.0])],
        };
        assert!(near_black_frames(&report, BLACK_LUMA_THRESHOLD).is_empty());
    }

    // ---- window-clamp guard ---------------------------------------------

    #[test]
    fn aspect_matches_exact_size() {
        // 1x display: captured framebuffer == requested logical size.
        assert!(aspect_matches((1280, 720), (1280, 720), ASPECT_TOLERANCE));
        assert!(aspect_matches((1080, 1920), (1080, 1920), ASPECT_TOLERANCE));
    }

    #[test]
    fn aspect_matches_honest_hidpi_scale_up() {
        // 2x display: twice the size, identical aspect — must pass.
        assert!(aspect_matches((1280, 720), (2560, 1440), ASPECT_TOLERANCE));
        assert!(aspect_matches((1080, 1920), (2160, 3840), ASPECT_TOLERANCE));
        // 1.5x (Windows) with the ±1 px rounding a fractional factor gives.
        assert!(aspect_matches((1280, 720), (1920, 1081), ASPECT_TOLERANCE));
    }

    #[test]
    fn aspect_matches_rejects_the_observed_clamp() {
        // The real regression this guard exists for: `radiance-synthetic-portrait`
        // asked for 1080x1920 (aspect 0.5625) and the OS clamped the 2x window
        // to 2160x1976 (aspect ~1.09) — a portrait scenario validating a
        // near-square window while reporting PASS.
        let requested = (1080, 1920);
        let clamped = (2160, 1976);
        assert!(!aspect_matches(requested, clamped, ASPECT_TOLERANCE));
    }

    #[test]
    fn aspect_matches_rejects_degenerate_sizes() {
        assert!(!aspect_matches((1080, 0), (2160, 3840), ASPECT_TOLERANCE));
        assert!(!aspect_matches((1080, 1920), (2160, 0), ASPECT_TOLERANCE));
        assert!(!aspect_matches((0, 1920), (0, 3840), ASPECT_TOLERANCE));
    }

    #[test]
    fn aspect_tolerance_admits_small_error_and_rejects_large() {
        // 0.5% off — inside the 1% tolerance (fractional-scale-factor rounding
        // lives here). Deliberately not testing exactly 1%: the relative error
        // of a ratio of integers lands on either side of the constant by a few
        // ULPs, which says nothing useful about the guard.
        let square = (1000, 1000);
        assert!(aspect_matches(square, (1005, 1000), ASPECT_TOLERANCE));
        // 2% off — outside it.
        assert!(!aspect_matches(square, (1020, 1000), ASPECT_TOLERANCE));
    }

    #[test]
    fn requested_window_falls_back_to_the_app_default() {
        assert_eq!(requested_window(&scenario()), DEFAULT_WINDOW);
        let mut s = scenario();
        s.resolution = Some([1080, 1920]);
        assert_eq!(requested_window(&s), (1080, 1920));
    }

    #[test]
    fn check_window_passes_an_honest_2x_portrait_capture() {
        let mut s = scenario();
        s.resolution = Some([1080, 1920]);
        let report = Report {
            frames: vec![
                sized_frame_report(30, [10.0, 10.0, 10.0], (2160, 3840)),
                sized_frame_report(60, [10.0, 10.0, 10.0], (2160, 3840)),
            ],
        };
        let check = check_window(&s, &report).expect("report has frames");
        assert!(check.aspect_ok);
        assert_eq!(check.requested, (1080, 1920));
        assert_eq!(check.captured, (2160, 3840));
        assert_eq!(check.frame, 30); // no offender -> the first frame stands in
    }

    #[test]
    fn check_window_flags_the_first_clamped_frame() {
        let mut s = scenario();
        s.resolution = Some([1080, 1920]);
        let report = Report {
            frames: vec![
                sized_frame_report(30, [10.0, 10.0, 10.0], (2160, 3840)), // honest
                sized_frame_report(60, [10.0, 10.0, 10.0], (2160, 1976)), // clamped
            ],
        };
        let check = check_window(&s, &report).expect("report has frames");
        assert!(!check.aspect_ok);
        assert_eq!(check.frame, 60);
        assert_eq!(check.captured, (2160, 1976));
    }

    #[test]
    fn window_clamp_error_names_the_numbers_and_the_fix() {
        let check = WindowCheck {
            requested: (1080, 1920),
            captured: (2160, 1976),
            frame: 60,
            aspect_ok: false,
        };
        let msg = window_clamp_error("radiance-synthetic-portrait", &check);
        assert!(msg.contains("radiance-synthetic-portrait"), "{msg}");
        assert!(msg.contains("1080x1920"), "{msg}");
        assert!(msg.contains("2160x1976"), "{msg}");
        assert!(msg.contains("0.5625"), "{msg}"); // requested aspect
        assert!(msg.contains("clamped"), "{msg}");
        assert!(msg.contains("frame 60"), "{msg}");
    }

    #[test]
    fn window_json_surfaces_the_verdict_as_a_field() {
        let check = WindowCheck {
            requested: (1080, 1920),
            captured: (2160, 1976),
            frame: 60,
            aspect_ok: false,
        };
        let json = window_json(Some(&check));
        assert!(json.contains("\"requested\":[1080,1920]"), "{json}");
        assert!(json.contains("\"captured\":[2160,1976]"), "{json}");
        assert!(json.contains("\"requested_aspect\":0.5625"), "{json}");
        assert!(json.contains("\"aspect_ok\":false"), "{json}");
        assert!(json.contains("\"frame\":60"), "{json}");
        assert_eq!(window_json(None), "null");
    }
}
