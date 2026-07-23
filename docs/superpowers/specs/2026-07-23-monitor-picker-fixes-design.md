# Monitor Picker Fixes — Design

**Date:** 2026-07-23
**Status:** Approved (Madison, 2026-07-23)
**Prior art:** `crates/wc-core/src/settings/panel_user/display.rs` (settings +
pure resolution), `crates/wc-core/src/lifecycle/display.rs` (systems), the
camera picker's `AUTO_LABEL` sentinel convention
(`crates/wc-core/src/input/capture/devices.rs`).

## Problems (observed live, 2026-07-23)

1. **Clicking a monitor in the dropdown does nothing in windowed mode.** The
   `monitor` setting is consumed only inside the
   `WindowMode::BorderlessFullscreen(..)` arm, so a windowed operator sees a
   dead control. Madison's requirement: "selecting the monitor should just
   work" regardless of window mode.
2. **macOS monitor names are unstable.** The same physical display
   re-enumerated as `Monitor #4225` and then `Monitor #2` two seconds apart
   across an HDMI handshake bounce (winit derives names from
   `CGDisplayModelNumber`). Exact-name persistence therefore silently loses
   the kiosk TV binding across a rename; names also collide across same-model
   displays and read as cryptic.

## Decision

One bespoke dropdown entry solves both problems without new settings fields
or panel machinery (a `disabled_when` derive attribute + checkbox design was
considered and dropped as heavier than needed):

- The Monitor dropdown gains a sentinel first entry, label exactly
  **`Automatic (External monitor with fallback)`** — the same
  sentinel-heads-the-list convention as the camera picker's `AUTO_LABEL`.
  Selecting it targets the first non-primary (external) monitor, falling
  back to the current monitor when none exists. This is the kiosk's one-tick
  fix for the rename problem: the TV is targeted whatever it calls itself.
- Selecting any entry (sentinel or explicit name) takes effect immediately
  in windowed mode too, by centering the window on the resolved monitor.

## Design

### Sentinel and options list

- `pub(crate) const AUTO_MONITOR_LABEL: &str = "Automatic (External monitor with fallback)";`
  lives in `settings/panel_user/display.rs` beside `AvailableMonitors`.
- `AvailableMonitors` gets a manual `Default` impl returning
  `vec![AUTO_MONITOR_LABEL.to_owned()]` (replacing `#[derive(Default)]`), so
  the sentinel is selectable even before winit's first monitor enumeration
  and the automatic mode never renders as "(unavailable)".
- `sync_available_monitors` rebuilds the list as sentinel-first:
  `push(AUTO_MONITOR_LABEL)` then extend with live names. (Allocation stays
  topology-change-gated, as today.)
- The generic runtime-enum widget needs no changes: the sentinel is an
  ordinary option, persisted verbatim as the field's `String` value. No
  persistence migration (pre-release; the empty-string default's meaning is
  unchanged).

### Resolution

The live-monitor iterator item grows a third element, `is_primary`
(bevy_winit's `PrimaryMonitor` marker, queried via `Has<PrimaryMonitor>`).
`resolve_monitor_selection` (now `pub(crate)`, since the windowed-move
system in `lifecycle/display.rs` also calls it) resolves:

1. `saved == AUTO_MONITOR_LABEL` → first live monitor with
   `!is_primary` → `MonitorSelection::Entity`; none → `Current`.
2. `saved` empty (fresh-install default) → `Current` — unchanged.
3. Explicit name → exact match → `Entity`; no match → `Current`, and the
   saved name is never rewritten — unchanged (an asleep TV must not lose
   its binding).

`compute_display_mode` and `apply_display_mode` carry the widened item type;
fullscreen behavior is otherwise untouched (its every-frame re-derive
already moves the fullscreen window when the resolved target changes).

### Windowed move (one-shot)

New system `move_window_on_monitor_edit` in `lifecycle/display.rs`,
registered in `DisplayPlugin`'s `Update` set:

- Value-diffs `settings.monitor` through a `Local<Option<String>>` — never
  `Res::is_changed()` (the dock writes the resource every frame the tab is
  open). The first run seeds the `Local`: a boot is not an edit, same guard
  as `clear_fullscreen_override_on_settings_edit`.
- On an edit while **not** effectively fullscreen
  (`FullscreenOverride::effective_fullscreen`), resolve the selection and —
  only when it yields `MonitorSelection::Entity` — set every window's
  `position = WindowPosition::Centered(<resolved>)`. A resolution that falls
  back to `Current` moves nothing: no pointless recenter on the same
  monitor, no jump on an unresolvable name.
- Allocates only on an actual edit (one `String` clone), honoring the
  no-alloc-in-hot-paths rule.

### Caveat (documented, not coded around)

"External" means non-primary in OS terms. Automatic mode assumes the
built-in display stays the OS primary — true by default on the MBP and
vacuous on the single-display kiosk, where the TV is primary and the
fallback (`Current`) targets it anyway.

## Testing

- Resolution arms: sentinel + external present → that entity; sentinel with
  several externals → the first; sentinel with primary-only list → `Current`;
  sentinel with empty list → `Current`; empty and explicit-name arms keep
  their existing tests (signatures widened with `is_primary`).
- Options list: `AvailableMonitors::default()` is `[sentinel]`;
  `sync_available_monitors` keeps the sentinel at index 0 with live names
  after it.
- Windowed move: in a `MinimalPlugins` + `DisplayPlugin` harness with a
  spawned `Window` and `Monitor` entities — boot frame does not move the
  window; an edit to a resolvable name centers it on that monitor; an edit
  while effectively fullscreen does not touch `position`; an edit to an
  unresolvable name does not touch `position`.

## Out of scope

- Upstream winit stable display names (UUID-based) — the sentinel makes the
  kiosk robust without it.
- Multi-external disambiguation policies (deployments have at most one
  external display).
- Richer name matching (resolution/position heuristics) — YAGNI given the
  sentinel.
