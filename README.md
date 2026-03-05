# CharGallery

Interactive art gallery built with React, Three.js/WebGL, and Web Audio. Features generative visualizations that respond to mouse, touch, and Leap Motion hand tracking. Runs as a website or a self-contained Electron desktop app.

## Features

- **5 interactive sketches** — generative visualizations built with Three.js/WebGL, each with unique physics and rendering
- **Generative audio** — every sketch produces real-time audio driven by its simulation state
- **Mouse, touch, and Leap Motion input** — immersive tactile interaction from a variety of input styles
- **Screensaver mode** — auto-activates after 30 seconds of idle
- **Advanced settings panel** — per-sketch tuning (particle density, quality, gamma) via `Shift`+`D` or gear icon
- **Electron desktop app** — fullscreen kiosk mode with display sleep prevention, auto-launching Leap Motion Websocket compatibility server
- **Cross-platform builds** — DMG for macOS, portable exe for Windows, and a browser target for web portfolio use

## Sketches

- **flame** — Iterated function system fractal driven by your name, with generative audio
- **line** — Particle line that responds to mouse/touch/Leap Motion attractors
- **dots** — Particle grid with gravitational attractors
- **cymatics** — Chladni plate vibration patterns with Leap Motion control
- **waves** — Audio-reactive wave visualization

## Development

Install [Node.js](https://nodejs.org/), then:

```sh
npm install
npm run start
```

Opens at http://localhost:5173. Supports hot module replacement.

## Web Build

```sh
npm run build     # Production build to dist/
npm run preview   # Serve the production build locally
```

## Electron App

```sh
npm run electron:dev       # Electron + Vite HMR dev mode
npm run electron:build     # Build renderer + main process
npm run electron:package   # Package into DMG (macOS) or portable exe (Windows)
```

To cross-compile for Windows from macOS, install Wine (`brew install --cask wine-stable`) then:

```sh
npm run electron:build && npx electron-builder --win
```

The Electron app auto-launches the Ultraleap WebSocket binary (if present in `bin/`) and enables audio autoplay without user gesture.

## Releasing

1. Bump `version` in `package.json` and commit
2. Run `npm run release:tag` to create and push the git tag
3. GitHub Actions builds macOS DMG + Windows exe and creates a draft release
4. Review the draft on the [Releases](../../releases) page, edit notes if needed, then publish

The web build deploys to GitHub Pages automatically on every push to `main`.

## Keyboard Shortcuts

| Key          | Action                          |
|--------------|---------------------------------|
| `z` / `←`    | Previous sketch                 |
| `x` / `→`    | Next sketch                     |
| `Escape`     | Return to home / gallery        |
| `v`          | Toggle volume on/off            |
| `Shift+D`    | Toggle advanced settings panel  |

## Leap Motion

Optional. Sketches support [Leap Motion](https://www.ultraleap.com/) hand tracking.

Compatible with Leap Motion Software 4.x out of the box. For 5.x+ (Gemini), the [UltraleapTrackingWebSocket](https://github.com/ultraleap/UltraleapTrackingWebSocket) compatibility layer is needed. A pre-built macOS Apple Silicon binary is included:

```sh
npm run leap-websocket:macos
```

In Electron mode, this binary is launched automatically on startup.
