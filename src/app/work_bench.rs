//! A benchmark of working on a sheet, with `KINETIC_PDF_WORK_BENCH=<sheet>`:
//! the view goes to that sheet, waits until it is drawn, and then does what
//! somebody reading a drawing does -- zoom in, move about, zoom in further,
//! move about, zoom back out -- timing every step to the frame where the view
//! is sharp again. Then it reports to stderr and the app closes.
//!
//! This is the measurement the other benchmarks miss. `page_bench` times a
//! sheet being drawn once, cold, at one zoom, which is the one thing a
//! resolution-independent renderer can't help with: somebody has to read the
//! page either way. What shapes on the GPU buy is everything afterwards --
//! a zoom is a different matrix on geometry that is already there, and panning
//! is free, where pdfium must rasterise the page again at the new scale and
//! rasterise every fresh region panned into. Nobody opens a drawing and stops;
//! they zoom and pan for minutes. So that is what this times.
//!
//! The warm-up is deliberately not counted: both renderers have to get the
//! sheet up once, and `page_bench` already reports that.

use eframe::egui;

use super::App;
use crate::worker::private_bytes;

/// Frames the view must be sharp for before a step counts as finished.
const SHARP_FRAMES: usize = 3;

/// A step that hasn't come sharp in this long is recorded as this long.
const GIVE_UP: f64 = 15.0;

/// How long to wait after opening before going to the sheet.
const SETTLE: f64 = 3.0;

/// What somebody does with a drawing, in order. Each step is a zoom to move to
/// and how far to pan down afterwards, as a fraction of the view's height.
const STEPS: [(f32, f32); 12] = [
    (2.0, 0.0),  // zoom in to read a detail
    (2.0, 0.6),  // follow it down the sheet
    (2.0, 0.6),
    (4.0, 0.0),  // closer
    (4.0, 0.6),
    (4.0, 0.6),
    (8.0, 0.0),  // right in, to check a dimension
    (8.0, 0.6),
    (8.0, 0.6),
    (4.0, 0.0),  // and back out again
    (2.0, 0.0),
    (1.0, 0.0),
];

enum Stage {
    Opening,
    Settling { until: f64 },
    /// Waiting for the sheet to be drawn the first time; not timed.
    WarmingUp { since: f64, sharp_frames: usize },
    /// Doing `STEPS[at]`, started at `since`.
    Step { at: usize, since: f64, sharp_frames: usize },
}

pub(super) struct WorkBench {
    /// The sheet to work on, counted from 0, or `None` for the middle one --
    /// which is what `0` asks for, so a caller needn't know the page count.
    sheet: Option<usize>,
    took: Vec<f64>,
    stage: Stage,
}

impl WorkBench {
    pub(super) fn from_env() -> Option<WorkBench> {
        let sheet = std::env::var("KINETIC_PDF_WORK_BENCH").ok()?;
        let sheet = sheet.trim().parse::<usize>().ok()?;
        Some(WorkBench { sheet: sheet.checked_sub(1), took: Vec::new(), stage: Stage::Opening })
    }
}

impl App {
    /// Runs the working benchmark, a frame at a time; see the module comment.
    pub(super) fn work_bench(&mut self, ctx: &egui::Context) {
        let Some(mut bench) = self.work_bench.take() else { return };
        let Some(doc) = self.doc.as_ref() else {
            self.work_bench = Some(bench);
            return;
        };
        let (now, pages) = (Self::now(ctx), doc.sizes.len());
        ctx.request_repaint();
        match bench.stage {
            Stage::Opening => {
                let sheet = bench.sheet.unwrap_or(pages / 2).min(pages.saturating_sub(1));
                bench.sheet = Some(sheet);
                bench.stage = Stage::Settling { until: now + SETTLE };
                self.zoom_mode = super::ZoomMode::Custom;
                self.change_zoom(1.0, None);
                eprintln!(
                    "work bench: sheet {} of {pages}, {}",
                    sheet + 1,
                    if self.gpu.is_some() { "our renderer" } else { "pdfium only" }
                );
            }
            Stage::Settling { until } if now >= until => {
                bench.stage = Stage::WarmingUp { since: now, sharp_frames: 0 };
                self.go_to_page(bench.sheet.unwrap_or(0));
            }
            Stage::Settling { .. } => {}
            Stage::WarmingUp { since, sharp_frames } => {
                let sharp = if self.view_sharp { sharp_frames + 1 } else { 0 };
                if sharp < SHARP_FRAMES && now - since < GIVE_UP {
                    bench.stage = Stage::WarmingUp { since, sharp_frames: sharp };
                    self.work_bench = Some(bench);
                    return;
                }
                eprintln!("work bench: sheet up after {:.0} ms; now working on it", (now - since) * 1000.0);
                bench.stage = Stage::Step { at: 0, since: now, sharp_frames: 0 };
                self.take_step(STEPS[0]);
            }
            Stage::Step { at, since, sharp_frames } => {
                let sharp = if self.view_sharp { sharp_frames + 1 } else { 0 };
                if sharp < SHARP_FRAMES && now - since < GIVE_UP {
                    bench.stage = Stage::Step { at, since, sharp_frames: sharp };
                    self.work_bench = Some(bench);
                    return;
                }
                // Sharp on the first frame is a real result here, not a fault:
                // it means nothing had to be drawn for the step at all.
                let waited = (now - since) * 1000.0;
                let (zoom, pan) = STEPS[at];
                eprintln!("work bench: {:.0}%{} sharp after {waited:.0} ms", zoom * 100.0, if pan > 0.0 { ", panned" } else { "" });
                bench.took.push(waited);
                match at + 1 {
                    next if next < STEPS.len() => {
                        bench.stage = Stage::Step { at: next, since: now, sharp_frames: 0 };
                        self.take_step(STEPS[next]);
                    }
                    _ => {
                        let mut sorted = bench.took.clone();
                        sorted.sort_by(f64::total_cmp);
                        let total: f64 = sorted.iter().sum();
                        let median = sorted[sorted.len() / 2];
                        let worst = sorted.last().copied().unwrap_or(0.0);
                        let free = sorted.iter().filter(|&&ms| ms < 34.0).count();
                        eprintln!(
                            "work bench: {} steps, median {median:.0} ms, worst {worst:.0} ms, {total:.0} ms in all, {free} needed no drawing",
                            sorted.len()
                        );
                        eprintln!("work-bench-median-ms: {median:.0}");
                        eprintln!("work-bench-worst-ms: {worst:.0}");
                        eprintln!("work-bench-total-ms: {total:.0}");
                        eprintln!("work-bench-free-steps: {free}");
                        eprintln!("work-bench-memory-mb: {}", private_bytes() >> 20);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        self.allow_close = true;
                        return;
                    }
                }
            }
        }
        self.work_bench = Some(bench);
    }

    /// Zooms to `zoom` and pans down `pan` screenfuls, as one step.
    fn take_step(&mut self, (zoom, pan): (f32, f32)) {
        // Anchored at the middle of the view, which is where drawing ahead aims.
        self.change_zoom(zoom, None);
        if pan > 0.0 {
            self.scroll_y = Some(self.scroll_offset.y + self.viewer_rect.height() * pan);
        }
    }
}
