# Eval media (local-only)

Recorded clips used as evaluation input for tracking work. Everything in
this directory except this README is gitignored and stays on the local
machine: the footage shows real people, and the files are far too large to
commit. Treat the contents as per-machine assets, like `.env`.

## Layout

- `body/` holds body-tracking eval clips (1280x720 @ ~30 fps `.mov`).
  Filenames describe the scenario the clip exercises (featured performer,
  multiple dancers, occlusion, lights on/off, props).

## Using a clip with the body replay harness

The replay harness consumes a directory of JPEG frames, not the `.mov`
itself. Extract frames into a scratch directory:

```sh
ffmpeg -i tests/eval-media/body/<clip>.mov -vf fps=30 -q:v 3 <framesdir>/%05d.jpg
```

Then drive the app from the frames:

```sh
WAVECONDUCTOR_BODY_REPLAY=<framesdir>@30 \
WAVECONDUCTOR_START_SKETCH=radiance \
WAVECONDUCTOR_CONFIG_DIR=$(mktemp -d) \
RUST_LOG='info,wc_core::input::body=debug' \
cargo rund
```

Notes:

- Replay pays a ~10-20 ms/frame JPEG decode the live AVFoundation camera
  path does not, so replay understates the live inference rate.
- Frame directories are disposable and large; put them in a scratch
  location outside the repo and delete them when done. They regenerate
  from the clips with the `ffmpeg` line above.
