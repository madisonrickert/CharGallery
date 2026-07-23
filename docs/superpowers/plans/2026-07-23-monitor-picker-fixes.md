# Monitor Picker Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Monitor dropdown work in windowed mode and add an "Automatic (External monitor with fallback)" sentinel entry that keeps the kiosk bound to its TV across macOS display renames, per `docs/superpowers/specs/2026-07-23-monitor-picker-fixes-design.md`.

**Architecture:** A sentinel option heads the monitor list (camera-picker `AUTO_LABEL` convention); `resolve_monitor_selection` gains an automatic arm keyed on bevy_winit's `PrimaryMonitor` marker; a new one-shot system centers the window on the resolved monitor when the selection is edited while windowed.

**Tech Stack:** Rust (Bevy 0.19), existing settings/runtime-enum machinery — no new dependencies, no derive-macro changes.

## Global Constraints

- Sentinel label is exactly `Automatic (External monitor with fallback)` and is persisted verbatim as the `monitor` field's `String` value. No persistence migration (pre-release).
- Empty-string default keeps meaning `MonitorSelection::Current`; explicit-name resolution keeps exact match with fallback to `Current` and **never rewrites the saved name**.
- The windowed move fires only on a value edit (Local value-diff, never `Res::is_changed()` — the dock marks the resource changed every frame the tab renders), only while not effectively fullscreen, and only when resolution yields `MonitorSelection::Entity`.
- No per-frame allocation: list rebuilds stay topology-change-gated; the move system allocates one `String` only on an actual edit.
- All harness tests that build `DisplayPlugin` MUST isolate settings persistence by pointing `wc_core::settings::persistence::CONFIG_DIR_ENV` (`WAVECONDUCTOR_CONFIG_DIR`) at a fresh temp dir before constructing the `App` — `register_sketch_settings` loads the operator's real `sketch-settings.toml` at plugin-build time, and a persisted `monitor`/`start_fullscreen` value on a dev or kiosk machine silently breaks the edit-detection tests. Mirror the mechanism `crates/wc-core/tests/settings_plugin.rs` already uses (`tempfile` is a wc-core dev-dependency; nextest runs one process per test, so the env write cannot race).
- Verification: `cargo nextest run -p wc-core`, `cargo clippy --all-targets --all-features --workspace -- -D warnings`, `cargo fmt --all -- --check`.

---

### Task 1: Sentinel entry + automatic resolution arm

**Files:**
- Modify: `crates/wc-core/src/settings/panel_user/display.rs` (constant, `Default` impl, resolution fn + its tests, `compute_display_mode`)
- Modify: `crates/wc-core/src/lifecycle/display.rs` (`sync_available_monitors`, `apply_display_mode` query, list-order test)

**Interfaces:**
- Produces: `pub(crate) const AUTO_MONITOR_LABEL: &str`;
  `pub(crate) fn resolve_monitor_selection<'a>(saved_name: &str, live_monitors: impl IntoIterator<Item = (Entity, Option<&'a str>, bool)>) -> MonitorSelection` (third tuple element = is-primary);
  `compute_display_mode` takes the same widened item type. Task 2's move system consumes both.

- [ ] **Step 1: Write the failing resolution tests**

In the `tests` module of `settings/panel_user/display.rs`:

```rust
    // --- automatic (sentinel) arm ---

    #[test]
    fn automatic_targets_the_first_non_primary_monitor() {
        let external = entity(2);
        let live = [
            (entity(1), Some("Built-in Display"), true),
            (external, Some("LG TV"), false),
            (entity(3), Some("Second TV"), false),
        ];
        assert_eq!(
            resolve_monitor_selection(AUTO_MONITOR_LABEL, live),
            MonitorSelection::Entity(external)
        );
    }

    #[test]
    fn automatic_falls_back_to_current_when_every_monitor_is_primary() {
        // The single-display kiosk: the TV is the OS primary, and Current
        // targets it anyway — the fallback in the sentinel's label.
        let live = [(entity(1), Some("LG TV"), true)];
        assert_eq!(
            resolve_monitor_selection(AUTO_MONITOR_LABEL, live),
            MonitorSelection::Current
        );
    }

    #[test]
    fn automatic_falls_back_to_current_on_an_empty_monitor_list() {
        let live: [(Entity, Option<&str>, bool); 0] = [];
        assert_eq!(
            resolve_monitor_selection(AUTO_MONITOR_LABEL, live),
            MonitorSelection::Current
        );
    }

    #[test]
    fn available_monitors_default_is_exactly_the_sentinel() {
        assert_eq!(
            AvailableMonitors::default().0,
            vec![AUTO_MONITOR_LABEL.to_owned()],
            "the automatic entry must be selectable before enumeration"
        );
    }
```

Also widen every existing tuple literal in this module's tests with an
is-primary third element (mechanical; semantics unchanged):
- `empty_saved_name_resolves_to_current_regardless_of_live_monitors`: `(entity(1), Some("DELL U2720Q"), true)`
- `a_saved_name_matching_a_live_monitor_resolves_to_its_entity`: `(entity(1), Some("Built-in Display"), true)`, `(target, Some("LG TV"), false)`
- `a_saved_name_with_no_live_match_falls_back_to_current`: `(entity(1), Some("Built-in Display"), true)`
- `a_saved_name_resolves_against_an_empty_monitor_list_to_current`: `[(Entity, Option<&str>, bool); 0]`
- `an_unnamed_live_monitor_never_matches_a_non_empty_saved_name`: `(entity(1), None, false)`
- `windowed_when_start_fullscreen_is_false_regardless_of_monitor`: `(entity(1), Some("LG TV"), false)`
- `fullscreen_on_the_named_monitor_when_it_resolves`: `(entity(1), Some("Built-in Display"), true)`, `(target_entity, Some("LG TV"), false)`
- `an_override_of_false_windows_a_kiosk_configured_for_fullscreen`: `(entity(1), Some("LG TV"), true)`
- `an_override_of_true_fullscreens_on_the_saved_monitor`: `(target_entity, Some("LG TV"), false)`
- Delete `available_monitors_defaults_empty` (superseded by the sentinel-default test above).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p wc-core --lib settings::panel_user::display`
Expected: FAIL — `AUTO_MONITOR_LABEL` not found, tuple arity mismatches.

- [ ] **Step 3: Implement the settings-side changes**

In `settings/panel_user/display.rs`:

(a) Below the `AvailableMonitors` struct, add the constant and replace the
derived `Default` (remove `Default` from the `#[derive(...)]` list):

```rust
/// Sentinel entry heading the monitor dropdown (the camera picker's
/// `AUTO_LABEL` convention): selecting it targets the first non-primary
/// (external) monitor and falls back to the current one when none exists.
/// Persisted verbatim as the `monitor` field's value.
pub(crate) const AUTO_MONITOR_LABEL: &str = "Automatic (External monitor with fallback)";

impl Default for AvailableMonitors {
    /// The sentinel is always present — even before winit's first monitor
    /// enumeration — so automatic mode never renders as "(unavailable)".
    fn default() -> Self {
        Self(vec![AUTO_MONITOR_LABEL.to_owned()])
    }
}
```

(b) Replace `resolve_monitor_selection` (now `pub(crate)`, called by Task 2's
system in `lifecycle/display.rs`):

```rust
/// Resolve a persisted monitor selection to a [`MonitorSelection`] against
/// the monitors currently known to the ECS (`is_primary` = bevy_winit's
/// `PrimaryMonitor` marker).
///
/// - An empty `saved_name` (the field's default) means "no preference":
///   always [`MonitorSelection::Current`].
/// - [`AUTO_MONITOR_LABEL`] means automatic: the first non-primary
///   (external) monitor, else [`MonitorSelection::Current`]. "External" is
///   OS-primary-relative — see the `monitor` field doc for the caveat.
/// - A non-empty name matching a live monitor's `Some(name)` resolves to
///   that monitor's `Entity`.
/// - A non-empty name with no live match — the monitor is asleep, unplugged,
///   renamed by the OS (observed 2026-07-23: the same display re-enumerated
///   as `Monitor #4225` then `Monitor #2`), or winit has not enumerated yet —
///   falls back to [`MonitorSelection::Current`]. The caller never rewrites
///   `saved_name`; the `&str` parameter makes that structurally true.
pub(crate) fn resolve_monitor_selection<'a>(
    saved_name: &str,
    live_monitors: impl IntoIterator<Item = (Entity, Option<&'a str>, bool)>,
) -> MonitorSelection {
    if saved_name.is_empty() {
        return MonitorSelection::Current;
    }
    if saved_name == AUTO_MONITOR_LABEL {
        return live_monitors
            .into_iter()
            .find(|(_, _, is_primary)| !is_primary)
            .map_or(MonitorSelection::Current, |(entity, _, _)| {
                MonitorSelection::Entity(entity)
            });
    }
    live_monitors
        .into_iter()
        .find(|(_, name, _)| *name == Some(saved_name))
        .map_or(MonitorSelection::Current, |(entity, _, _)| {
            MonitorSelection::Entity(entity)
        })
}
```

(c) Widen `compute_display_mode`'s parameter to the same item type
(`impl IntoIterator<Item = (Entity, Option<&'a str>, bool)>`) — body
unchanged, it just forwards to `resolve_monitor_selection`.

(d) Doc updates: the `monitor` field rustdoc gains two sentences — the
dropdown is headed by the automatic sentinel (name it via a code span, and
state the non-primary definition + OS-primary caveat), and selecting any
entry also takes effect in windowed mode by centering the window (the
Task 2 system). The `AvailableMonitors` doc notes the sentinel heads the
list.

- [ ] **Step 4: Implement the lifecycle-side changes**

In `lifecycle/display.rs`:

(a) Import `PrimaryMonitor` alongside `Monitor` (`bevy::window::{CursorOptions, Monitor, PrimaryMonitor}`)
and `AUTO_MONITOR_LABEL` from `crate::settings`. (If the import fails,
`PrimaryMonitor` lives in the same bevy module that exports `Monitor` —
check the vendored `bevy_window` source; it is the marker
`bevy_winit::system::create_monitors` inserts.) Re-export
`AUTO_MONITOR_LABEL` through `settings/mod.rs` the same way
`AvailableMonitors` is re-exported.

(b) `apply_display_mode`: query becomes
`Query<'_, '_, (Entity, &Monitor, Has<PrimaryMonitor>)>`, mapped as
`(entity, monitor.name.as_deref(), is_primary)`.

(c) `sync_available_monitors` rebuild becomes sentinel-first:

```rust
    available.0.clear();
    available.0.push(AUTO_MONITOR_LABEL.to_owned());
    available
        .0
        .extend(monitors.iter().filter_map(|m| m.name.clone()));
```

and its doc comment notes the sentinel heads the list. Keep the topology
debug log line that follows (committed as d41f312f). Also rewrite the
now-stale paragraph of `AvailableMonitors`' rustdoc ("An empty list is a
normal state … 03a omits the key from its snapshot in that case"): with
the sentinel `Default`, the list is never empty, and the key is always
present in the snapshot because `DisplayPlugin` init's the resource —
"omits the key" only ever applied to an absent resource.

(d) Add the list-order test to this file's `tests` module, with a local
`Monitor` constructor (adjust the field list to compile against the
vendored `bevy_window-0.19.0` `Monitor` struct if it differs):

```rust
    fn test_monitor(name: &str) -> Monitor {
        Monitor {
            name: Some(name.to_owned()),
            physical_height: 1080,
            physical_width: 1920,
            physical_position: IVec2::ZERO,
            refresh_rate_millihertz: Some(60_000),
            scale_factor: 1.0,
            video_modes: Vec::new(),
        }
    }

    #[test]
    fn sync_keeps_the_sentinel_at_the_head_of_the_options() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(DisplayPlugin);
        app.world_mut().spawn(test_monitor("LG TV"));
        app.update();

        let list = &app
            .world()
            .resource::<crate::settings::AvailableMonitors>()
            .0;
        assert_eq!(
            list.as_slice(),
            [
                crate::settings::AUTO_MONITOR_LABEL.to_owned(),
                "LG TV".to_owned()
            ],
            "sentinel first, live names after"
        );
    }
```

- [ ] **Step 5: Run the tests**

Run: `cargo nextest run -p wc-core`
Expected: PASS, including the four new tests and the list-order test.

- [ ] **Step 6: Commit (includes the pre-existing debug-log line)**

```bash
git add crates/wc-core/src/settings/panel_user/display.rs crates/wc-core/src/lifecycle/display.rs crates/wc-core/src/settings/mod.rs
git commit -m "feat(display): Automatic (external) monitor dropdown entry"
```

---

### Task 2: One-shot windowed move on selection edit

**Files:**
- Modify: `crates/wc-core/src/lifecycle/display.rs` (new system + registration + tests)

**Interfaces:**
- Consumes: Task 1's `resolve_monitor_selection` (widened signature) and `AUTO_MONITOR_LABEL`.
- Produces: `move_window_on_monitor_edit` system registered in `DisplayPlugin`.

- [ ] **Step 1: Write the failing tests**

In `lifecycle/display.rs` `tests` (reusing `test_monitor` from Task 1):

```rust
    /// Harness: DisplayPlugin + one windowed Window + one non-primary
    /// monitor named "LG TV". Returns the app and the monitor entity.
    ///
    /// Settings persistence is isolated to a fresh temp dir FIRST —
    /// `register_sketch_settings` loads the operator's real
    /// `sketch-settings.toml` at plugin-build time, and a persisted
    /// `monitor` value (e.g. the sentinel, after the live acceptance run)
    /// would make the "edit" in these tests a no-op value-diff. Mirror
    /// `tests/settings_plugin.rs`'s mechanism.
    fn app_with_window_and_external_monitor() -> (App, Entity) {
        let config_dir = tempfile::tempdir()
            .expect("create isolated config dir")
            .keep();
        std::env::set_var(crate::settings::persistence::CONFIG_DIR_ENV, &config_dir);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(DisplayPlugin);
        app.world_mut()
            .spawn((Window::default(), CursorOptions::default()));
        let external = app.world_mut().spawn(test_monitor("LG TV")).id();
        (app, external)
    }

    fn window_position(app: &mut App) -> WindowPosition {
        let mut query = app.world_mut().query::<&Window>();
        query
            .single(app.world())
            .expect("harness spawns exactly one window")
            .position
    }

    #[test]
    fn boot_is_not_an_edit_so_the_window_does_not_move() {
        let (mut app, _) = app_with_window_and_external_monitor();
        app.update();
        app.update();
        assert_eq!(window_position(&mut app), WindowPosition::Automatic);
    }

    #[test]
    fn editing_the_selection_while_windowed_centers_on_the_resolved_monitor() {
        let (mut app, external) = app_with_window_and_external_monitor();
        app.update();
        app.world_mut()
            .resource_mut::<crate::settings::DisplaySettings>()
            .monitor = "LG TV".to_owned();
        app.update();
        assert_eq!(
            window_position(&mut app),
            WindowPosition::Centered(MonitorSelection::Entity(external))
        );
    }

    #[test]
    fn selecting_automatic_centers_on_the_external_monitor() {
        let (mut app, external) = app_with_window_and_external_monitor();
        app.update();
        app.world_mut()
            .resource_mut::<crate::settings::DisplaySettings>()
            .monitor = crate::settings::AUTO_MONITOR_LABEL.to_owned();
        app.update();
        assert_eq!(
            window_position(&mut app),
            WindowPosition::Centered(MonitorSelection::Entity(external))
        );
    }

    #[test]
    fn an_unresolvable_name_edit_does_not_move_the_window() {
        let (mut app, _) = app_with_window_and_external_monitor();
        app.update();
        app.world_mut()
            .resource_mut::<crate::settings::DisplaySettings>()
            .monitor = "Ghost Monitor".to_owned();
        app.update();
        assert_eq!(window_position(&mut app), WindowPosition::Automatic);
    }

    #[test]
    fn an_edit_while_effectively_fullscreen_does_not_touch_position() {
        let (mut app, _) = app_with_window_and_external_monitor();
        app.update();
        app.world_mut()
            .resource_mut::<crate::settings::DisplaySettings>()
            .start_fullscreen = true;
        app.update();
        app.world_mut()
            .resource_mut::<crate::settings::DisplaySettings>()
            .monitor = "LG TV".to_owned();
        app.update();
        assert_eq!(
            window_position(&mut app),
            WindowPosition::Automatic,
            "fullscreen targeting is apply_display_mode's job; position stays untouched"
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p wc-core --lib lifecycle::display`
Expected: FAIL — `move_window_on_monitor_edit` does not exist, so the
edit tests see an unmoved window.

- [ ] **Step 3: Implement the system**

In `lifecycle/display.rs` (import `WindowPosition` via the prelude — it is
`bevy::window::WindowPosition`, prelude-exported):

```rust
/// One-shot windowed move: when the operator edits the Monitor selection
/// while the window is not effectively fullscreen, center the window on the
/// monitor the new selection resolves to.
///
/// Value-diffed through a `Local` rather than `Res::is_changed()` — the user
/// panel writes the resource every frame the DISPLAY tab is open (see
/// `clear_fullscreen_override_on_settings_edit`, which pioneered this guard).
/// The first run only seeds the `Local`: a boot is not an edit, so a saved
/// selection never yanks a freshly opened dev window across displays.
///
/// Moves **only** when resolution yields a concrete monitor
/// (`MonitorSelection::Entity`): a fallback to `Current` would be a
/// pointless recenter on the monitor the window already occupies (or a jump
/// on an unresolvable name). Fullscreen edits are `apply_display_mode`'s
/// job — its every-frame re-derive already retargets the fullscreen window.
///
/// Allocates one `String` per actual edit (never per frame).
pub(crate) fn move_window_on_monitor_edit(
    settings: Res<'_, DisplaySettings>,
    fullscreen_override: Res<'_, FullscreenOverride>,
    monitors: Query<'_, '_, (Entity, &Monitor, Has<PrimaryMonitor>)>,
    mut windows: Query<'_, '_, &mut Window>,
    mut previous_selection: Local<'_, Option<String>>,
) {
    let current = settings.monitor.as_str();
    let edited = previous_selection
        .as_deref()
        .is_some_and(|previous| previous != current);
    if previous_selection.as_deref() != Some(current) {
        *previous_selection = Some(current.to_owned());
    }
    if !edited || fullscreen_override.effective_fullscreen(&settings) {
        return;
    }
    let resolved = resolve_monitor_selection(
        current,
        monitors
            .iter()
            .map(|(entity, monitor, is_primary)| (entity, monitor.name.as_deref(), is_primary)),
    );
    let MonitorSelection::Entity(_) = resolved else {
        return;
    };
    for mut window in &mut windows {
        window.position = WindowPosition::Centered(resolved);
    }
    tracing::info!(monitor = %current, "monitor selection edited; centering window on it");
}
```

Add `resolve_monitor_selection` and `MonitorSelection` to the existing
`crate::settings` / `bevy::window` imports as needed.

(b) Register it in `DisplayPlugin::build`'s `Update` tuple, ordered after
the override clear so it reads the same effective-fullscreen answer as
`apply_display_mode` that frame:

```rust
                move_window_on_monitor_edit
                    .after(clear_fullscreen_override_on_settings_edit),
```

(placed in the tuple between `clear_fullscreen_override_on_settings_edit`
and `apply_display_mode`). Update the plugin doc comment's system list.

Also add one line to the module-level doc: the monitor selection works in
both modes — fullscreen via the every-frame mode re-derive, windowed via
this one-shot centering system.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p wc-core`
Expected: PASS — all five new tests, and the Task 1 + pre-existing suites.

- [ ] **Step 5: Commit**

```bash
git add crates/wc-core/src/lifecycle/display.rs
git commit -m "feat(display): center the window on the picked monitor in windowed mode"
```

---

### Task 3: Gates + live acceptance

**Files:** none (verification; any fix belongs to the owning task's files)

- [ ] **Step 1: CI gates**

Run, expecting green:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --workspace -- -D warnings
cargo nextest run --workspace --all-features
cargo test --doc --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --document-private-items
cargo deny check
cargo xtask check-secrets
```

- [ ] **Step 2: Live acceptance (operator, external display attached)**

With `cargo rund` and the external monitor connected:
1. Open Display settings: the dropdown lists `Automatic (External monitor with fallback)` first, then the live names.
2. Pick the external monitor by name while windowed: the window centers on it immediately.
3. Pick `Automatic (External monitor with fallback)`: the window centers on the external display.
4. Toggle Start fullscreen with Automatic selected: fullscreen lands on the external display; untoggle restores windowed.
5. (Rename robustness, best-effort:) power-cycle the external display while fullscreen-on-Automatic — after re-enumeration (any new name), fullscreen re-lands on it without touching settings.
6. (Known-risk probe:) while fullscreen with both displays attached, pick the *other* monitor by name in the dropdown. The 2026-07-23 review found winit macOS handles a live Borderless→Borderless retarget in a fallthrough arm and caches the new state as applied — so the window may stay on the old display permanently (bevy_winit's diff never retries). Record what actually happens. If it fails: not a blocker — the kiosk's set-monitor-then-fullscreen flow is unaffected — but scope a follow-up (exit fullscreen → retarget → re-enter bounce) rather than patching inline.

Expected: behaviors 1-5 as described (6 is a probe, either outcome is
recorded); the topology debug log lines narrate the re-enumerations when
running with `RUST_LOG=wc_core::lifecycle::display=debug`.
