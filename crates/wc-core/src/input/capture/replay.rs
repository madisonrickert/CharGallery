//! Deterministic recorded-footage replay for the body-tracking pipeline
//! (debug builds only): serves a directory of numbered image frames through
//! the [`FrameSource`] trait at a fixed wall-clock rate, so a real captured
//! clip (desk framing, multi-person, props) runs through the full detector /
//! landmark / association stack reproducibly — no human in front of a
//! camera, no camera at all.
//!
//! Activation: set `WAVECONDUCTOR_BODY_REPLAY=<dir>[@fps]` (default 30 fps)
//! and enter a body sketch; the body worker opens this source instead of a
//! physical webcam (see `body::systems::open_camera_source`). Prepare frames
//! from a video with:
//!
//! ```text
//! ffmpeg -i clip.mov -vf fps=30 frames/%05d.png
//! ```
//!
//! Frames are served in lexicographic filename order (zero-padded ffmpeg
//! output sorts correctly) and loop at the end so the worker sees a
//! continuous stream. Per-frame decode allocates; that is accepted here —
//! this is a debug-only investigation tool, never a production path.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{CaptureError, Frame, FrameSource};

/// Default replay rate when the env spec carries no `@fps` suffix.
const DEFAULT_FPS: u32 = 30;

/// Image extensions accepted as replay frames (the formats the `image`
/// dependency is built with).
const FRAME_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// A directory of image frames served as a paced, looping camera.
pub struct ReplayFrameSource {
    /// Frame paths in lexicographic order.
    frames: Vec<PathBuf>,
    /// Wall-clock replay rate.
    fps: u32,
    /// Pacing clock, armed on the first `next_frame`/`discard_frame` call.
    started: Option<Instant>,
    /// Frames served so far (monotonic; index = `served % frames.len()`).
    served: u64,
    /// Diagnostics label ("replay 640x480 @30, 900 frames").
    label: String,
}

impl ReplayFrameSource {
    /// Enumerate `dir`, sort the frames, and decode the first one to
    /// validate the set and record its dimensions.
    ///
    /// # Errors
    /// [`CaptureError::NoCamera`] when the directory is unreadable or holds
    /// no decodable frames.
    pub fn open(dir: &Path, fps: u32) -> Result<Self, CaptureError> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| CaptureError::NoCamera(format!("replay dir {}: {e}", dir.display())))?;
        let mut frames: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        FRAME_EXTENSIONS
                            .iter()
                            .any(|known| ext.eq_ignore_ascii_case(known))
                    })
            })
            .collect();
        frames.sort();
        let Some(first) = frames.first() else {
            return Err(CaptureError::NoCamera(format!(
                "replay dir {} holds no {FRAME_EXTENSIONS:?} frames",
                dir.display()
            )));
        };
        let probe = image::open(first)
            .map_err(|e| CaptureError::NoCamera(format!("replay frame {}: {e}", first.display())))?
            .to_rgb8();
        let label = format!(
            "replay {}x{} @{fps}, {} frames",
            probe.width(),
            probe.height(),
            frames.len()
        );
        tracing::info!(dir = %dir.display(), frames = frames.len(), fps, "body replay source opened");
        Ok(Self {
            frames,
            fps: fps.clamp(1, 120),
            started: None,
            served: 0,
            label,
        })
    }

    /// Whether the pacing clock says the next frame is due; arms the clock
    /// on first call so replay starts at the caller's first poll.
    fn due(&mut self) -> bool {
        let elapsed = self.started.get_or_insert_with(Instant::now).elapsed();
        self.served <= frames_due(elapsed, self.fps)
    }

    /// The path index for the next serve (looping). `open` guarantees a
    /// non-empty frame list; the `.max(1)` only guards the modulus.
    fn cursor(&self) -> usize {
        let len = u64::try_from(self.frames.len()).unwrap_or(u64::MAX).max(1);
        usize::try_from(self.served % len).unwrap_or(0)
    }
}

impl FrameSource for ReplayFrameSource {
    fn next_frame(&mut self, out: &mut Frame) -> Result<bool, CaptureError> {
        if !self.due() {
            return Ok(false);
        }
        let path = &self.frames[self.cursor()];
        let img = image::open(path)
            .map_err(|e| CaptureError::Read(format!("replay frame {}: {e}", path.display())))?
            .to_rgb8();
        out.fit_to(img.width(), img.height());
        out.rgb.copy_from_slice(img.as_raw());
        self.served += 1;
        Ok(true)
    }

    fn discard_frame(&mut self) -> Result<bool, CaptureError> {
        // Sequencing parity with next_frame (same cursor advance), minus the
        // decode — the FrameSource contract's over-budget drain.
        if !self.due() {
            return Ok(false);
        }
        self.served += 1;
        Ok(true)
    }

    fn format_label(&self) -> Option<&str> {
        Some(&self.label)
    }
}

/// Parse the `WAVECONDUCTOR_BODY_REPLAY` value: `<dir>` or `<dir>@<fps>`.
/// An `@suffix` that is not a number is treated as part of the path.
#[must_use]
pub fn parse_spec(spec: &str) -> (PathBuf, u32) {
    if let Some((dir, fps)) = spec.rsplit_once('@') {
        if let Ok(fps) = fps.parse::<u32>() {
            return (PathBuf::from(dir), fps.clamp(1, 120));
        }
    }
    (PathBuf::from(spec), DEFAULT_FPS)
}

/// How many frames a clip at `fps` owes after `elapsed` wall time (frame 0
/// is due immediately). Integer math, no float truncation casts.
fn frames_due(elapsed: Duration, fps: u32) -> u64 {
    u64::try_from(elapsed.as_micros().saturating_mul(u128::from(fps)) / 1_000_000)
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "expect is appropriate in test code")]
mod tests {
    use super::*;

    /// Spec parsing: bare dir defaults to 30 fps; `@fps` overrides; a
    /// non-numeric `@` stays in the path; fps clamps into 1..=120.
    #[test]
    fn parse_spec_handles_fps_suffix() {
        assert_eq!(
            parse_spec("/tmp/frames"),
            (PathBuf::from("/tmp/frames"), 30)
        );
        assert_eq!(
            parse_spec("/tmp/frames@24"),
            (PathBuf::from("/tmp/frames"), 24)
        );
        assert_eq!(
            parse_spec("/tmp/odd@name"),
            (PathBuf::from("/tmp/odd@name"), 30),
            "non-numeric suffix is part of the path"
        );
        assert_eq!(parse_spec("/tmp/f@0").1, 1, "fps floors at 1");
        assert_eq!(parse_spec("/tmp/f@999").1, 120, "fps caps at 120");
    }

    /// Frame 0 is due immediately; thereafter the count tracks elapsed*fps.
    #[test]
    fn frames_due_paces_at_fps() {
        assert_eq!(frames_due(Duration::ZERO, 30), 0);
        assert_eq!(frames_due(Duration::from_millis(34), 30), 1);
        assert_eq!(frames_due(Duration::from_secs(2), 30), 60);
        assert_eq!(frames_due(Duration::from_millis(999), 1), 0);
        assert_eq!(frames_due(Duration::from_secs(1), 1), 1);
    }

    /// End-to-end over a real temp directory: frames serve in sorted order,
    /// loop at the end, and discard advances the same cursor.
    #[test]
    fn replay_serves_sorted_and_loops() {
        let dir = std::env::temp_dir().join(format!("wc-replay-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp frame dir");
        // Two 1x1 frames with distinct red values, written out of order.
        for (name, red) in [("00002.png", 200_u8), ("00001.png", 100_u8)] {
            let mut img = image::RgbImage::new(1, 1);
            img.put_pixel(0, 0, image::Rgb([red, 0, 0]));
            img.save(dir.join(name)).expect("write test frame");
        }
        let mut src = ReplayFrameSource::open(&dir, 120).expect("open replay source");
        assert!(src
            .format_label()
            .expect("label")
            .starts_with("replay 1x1 @120"));

        let mut out = Frame::default();
        let mut reds = Vec::new();
        // Serve three frames (the third wraps to the first): poll until each
        // is due rather than sleeping a fixed pace.
        while reds.len() < 3 {
            if src.next_frame(&mut out).expect("replay frame") {
                reds.push(out.rgb[0]);
            }
        }
        assert_eq!(reds, vec![100, 200, 100], "sorted order, then loop");

        // Discard shares the cursor: after one discard the next served frame
        // is the second in sort order again.
        while !src.discard_frame().expect("discard") {}
        loop {
            if src.next_frame(&mut out).expect("post-discard frame") {
                break;
            }
        }
        assert_eq!(out.rgb[0], 100, "discard advanced past frame 2's wrap");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// An empty or missing directory refuses to open with a named error.
    #[test]
    fn replay_open_rejects_empty_dirs() {
        let missing = std::env::temp_dir().join("wc-replay-test-missing-dir");
        assert!(ReplayFrameSource::open(&missing, 30).is_err());
        let empty =
            std::env::temp_dir().join(format!("wc-replay-test-empty-{}", std::process::id()));
        std::fs::create_dir_all(&empty).expect("create empty dir");
        assert!(ReplayFrameSource::open(&empty, 30).is_err());
        std::fs::remove_dir_all(&empty).ok();
    }
}
