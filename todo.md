# To-do

## Bug: Navigation broken on Flame sketch (Chromium only)

**Symptom:** Keyboard shortcuts (z/x/arrows/escape) and the Home button change the URL but the page doesn't re-render. Only affects the Flame sketch. Works perfectly on Firefox and Safari.

**Root cause:** React Router 7 wraps navigation state updates in `React.startTransition()`, making them low-priority. The `SketchRenderer` component calls `setTick((t) => t + 1)` every frame via `requestAnimationFrame` — a regular (high-priority) `setState` at 60fps. In Chromium, the high-priority animation updates continuously preempt the low-priority navigation transition, so the route change never commits. Only Flame triggers this because it's the only sketch with heavy per-frame computation (`animateSuperPoint` + OrbitControls).

**Key findings:**
- OrbitControls (`three-stdlib`) is NOT blocking keyboard/click events directly — it only `preventDefault()`s on wheel (zoom) and contextmenu events
- OrbitControls registers `pointerdown`/`wheel`/`contextmenu` on the canvas; no `stopPropagation()` calls
- OrbitControls does NOT listen for keyboard events unless `controls.listenToKeyEvents()` is called (it isn't)
- `react-hotkeys-hook`'s `useHotkeys` listens on `document` — events bubble fine from the canvas
- The `setTick` state update is needed — removing it makes Leap hand visualization choppy (2-3fps), even though hand rendering is pure WebGL (`renderOverlay()` in `LeapHandController`). The mechanism isn't fully understood but empirically confirmed.

**Approaches tried:**
1. **`startTransition(() => setTick(...))`** — Navigation works, but Leap hand visualization becomes choppy (2-3fps). Likely React's transition scheduler stealing main thread time from rAF callbacks in Chromium.
2. **Remove `setTick` entirely** — Navigation works, but Leap hands still choppy. Confirms setTick is somehow needed for smooth hand rendering.
3. **`flushSync(() => navigate(...))` on all navigation calls** — Attempted in appRoutes.tsx, useThrottledNavigate.ts, and homeButton. Did not fix the issue — `flushSync` may not override React Router's internal `startTransition`.

**Approaches tried (continued):**
4. **Optimize flame rendering hot path** — Reduced per-frame CPU cost of `animateSuperPoint()` / `superPoint.ts`:
   - Eliminated `Vector3.clone()` per point in `createInterpolatedVariation` (~100k allocs/frame) — reuse module-level temp vector
   - Replaced `posArr.set([x,y,z], offset)` with direct index writes — eliminated ~200k temporary array allocs/frame
   - Threaded `posArr`/`colArr` as parameters through `updateSubtree()` instead of re-fetching from geometry attributes ~100k times
   - Changed visitor spread (`...visitors`) to pass array directly — eliminated array copies at every recursive call
   - Replaced string-keyed box hash in `BoxCountVisitor` with numeric spatial hash using `Map<number, number>`
   - Guarded visitor modulo check with `visitors.length > 0`
   - **Result:** Navigation bug is fixed (React transitions get enough breathing room). However, Leap hand visualization is still choppy on Chromium — the main thread load is reduced but still high enough to cause compositor issues.

**Ideas not yet tried:**
- **Throttle `setTick`** — Run every Nth frame (~20fps) instead of every frame. Might give React enough breathing room for transitions while keeping hand rendering smooth. Unknown if 20fps is enough for hand smoothness.
- **Direct hash navigation** — Use `window.location.hash = '#/'` to bypass React Router's `startTransition` entirely. Hacky but would sidestep the priority conflict.
- **React Router `unstable_useTransitions: false`** — React Router 7 has this option to make navigation synchronous (skip `startTransition`). Uses unstable API.
- **Investigate WHY `setTick` affects hand rendering — RESOLVED**
  - **Root cause: Chromium throttles WebSocket message delivery when the main thread is idle between rAF frames.**
  - There are **two independent rAF loops**: leapjs runs its own (`controller.startAnimationLoop()` → `onAnimationFrame` → emits `'animationFrame'` → `processFinishedFrame` → emits `'frame'` → `_handleFrame` updates hand mesh positions), and the sketch runs a separate one (`useSketchAnimationLoop` → `sketch.animate()` → `renderOverlay()`).
  - Both rAF loops fire at 60fps regardless of `setTick`. But the Leap WebSocket only delivers **new frame data** 2-6 times/sec without `setTick`, vs ~60/sec with it. `_handleFrame` fires at 60fps but replays the same stale `lastConnectionFrame` (controller.js:64) because `processFrame()` (which updates it from WebSocket) isn't called.
  - The mechanism: When `setTick` triggers React re-renders, the main thread is busy with reconciliation work between rAF frames. This causes Chromium's event loop to process WebSocket message tasks at full rate. Without main thread activity, Chromium batches/defers WebSocket I/O delivery as an energy-saving optimization.
  - **Confirmed by:** A 2ms busy-wait loop (no React, no DOM) also restored 60 position changes/sec.
  - **Fix:** `setTimeout(noop, 0)` per frame. Posts a macrotask to keep the event loop spinning, causing Chromium to process WebSocket messages. Zero CPU waste, no React re-renders, and navigation transitions work because there's no competing `setState`.
  - **Approaches tested and ruled out (compositor theory):**
    1. `will-change: contents` on canvas — no effect
    2. Raw DOM mutation (`element.dataset.t`) on hidden span — no effect
    3. `gl.flush()` after rendering — no effect
    4. `transform: translateZ(0)` on canvas — no effect

**Relevant Three.js issues:**
- https://github.com/mrdoob/three.js/issues/4327 — OrbitControls key events not working when domElement is passed (fixed via `controls.listenToKeyEvents(window)`)
- https://github.com/mrdoob/three.js/issues/15834 — OrbitControls eats keyboard events globally when listening on window

---

### Add windows WS binary
### Add macOS x86 WS binary

---

## Testing
Test electron app while leap motion controller is connected.
Test on windows.