//! A benchmark of scrolling, with `KINETIC_PDF_SCROLL_BENCH=1`: once the
//! document is open, the view scrolls from its top to its bottom at a steady
//! `SCREENS_A_SECOND`, then what it measured goes to stderr and the app closes.

use std::time::{Duration, Instant};

use eframe::egui;

use super::App;
use crate::worker::private_bytes;

/// How fast the view scrolls: slower than a fling, so pages load as they pass.
const SCREENS_A_SECOND: f32 = 2.0;

/// The bottom counts as reached once the view hasn't moved for this long.
const STILL_FOR: Duration = Duration::from_secs(2);

pub(super) struct ScrollBench {
    last_frame: Option<Instant>,
    /// Milliseconds between frames, and spent taking uploads each frame.
    frames: Vec<f32>,
    uploads: Vec<f32>,
    peak_bytes: usize,
    started: Option<Instant>,
    last_offset: f32,
    moved_at: Instant,
}

impl ScrollBench {
    pub(super) fn from_env() -> Option<ScrollBench> {
        std::env::var_os("KINETIC_PDF_SCROLL_BENCH").is_some_and(|v| v == "1").then(|| ScrollBench {
            last_frame: None,
            frames: Vec::new(),
            uploads: Vec::new(),
            peak_bytes: 0,
            started: None,
            last_offset: -1.0,
            moved_at: Instant::now(),
        })
    }
}

/// Median, 95th percentile and worst of `times`, in milliseconds.
fn summary(times: &[f32]) -> String {
    let mut sorted = times.to_vec();
    sorted.sort_by(f32::total_cmp);
    let pick = |q: f32| sorted.get(((sorted.len() as f32 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
    format!("median {:.1} ms, 95th percentile {:.1} ms, worst {:.0} ms", pick(0.5), pick(0.95), pick(1.0))
}

impl App {
    /// Scrolls a frame's worth, and records the frame; `upload` is how long
    /// taking shapes took this frame.
    pub(super) fn scroll_bench(&mut self, ctx: &egui::Context, upload: Duration) {
        let (Some(bench), Some(_)) = (self.scroll_bench.as_mut(), self.doc.as_ref()) else { return };
        let now = Instant::now();
        ctx.request_repaint();
        let started = *bench.started.get_or_insert(now);
        if let Some(last) = bench.last_frame {
            bench.frames.push((now - last).as_secs_f32() * 1000.0);
            bench.uploads.push(upload.as_secs_f32() * 1000.0);
        }
        bench.last_frame = Some(now);
        bench.peak_bytes = bench.peak_bytes.max(private_bytes());

        let offset = self.scroll_offset.y;
        if offset != bench.last_offset {
            bench.last_offset = offset;
            bench.moved_at = now;
        }
        let elapsed = now - started;
        if elapsed > STILL_FOR && now - bench.moved_at > STILL_FOR {
            let slow = bench.frames.iter().filter(|&&ms| ms > 50.0).count();
            eprintln!("pages drawn with: {}", self.gpu.as_ref().map_or("pdfium only", |gpu| gpu.name.as_str()));
            eprintln!("scrolled to the bottom in {:.1} s over {} frames", (bench.moved_at - started).as_secs_f64(), bench.frames.len());
            eprintln!("frame to frame: {}; {slow} frames over 50 ms", summary(&bench.frames));
            eprintln!("taking uploads: {}", summary(&bench.uploads));
            eprintln!("process memory: peak {} MB, now {} MB", bench.peak_bytes >> 20, private_bytes() >> 20);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.allow_close = true;
            self.scroll_bench = None;
            return;
        }
        let step = SCREENS_A_SECOND * self.viewer_rect.height() * bench.frames.last().copied().unwrap_or(16.0) / 1000.0;
        self.scroll_y = Some(offset + step);
    }
}
