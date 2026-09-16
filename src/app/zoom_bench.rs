//! A repeatable benchmark of zooming in, with
//! `KINETIC_PDF_ZOOM_BENCH=<page>[:<rest seconds>[:<steps>]]`: the view goes to
//! that page (counted from 1) at 10%, waits there while pages are
//! drawn ahead, then zooms to the deepest zoom -- in one step, or in `steps`
//! frames, which is what spinning the wheel in fast looks like. It times how
//! long until everything in view is sharp, and how long the view spends with
//! nothing on it but a page's thumbnail stretched to stand in for a drawing.
//! Then it reports to stderr and the app closes.
//!
//! Nothing here depends on the pointer: the zoom is anchored at the middle of
//! the view, which is also where drawing ahead aims when no pointer is over
//! the page, so the same thing is measured every time.

use eframe::egui;

use super::{App, ZOOMS};

/// The zoom it starts from. 10% rather than the smallest zoom there is, so its
/// results stay comparable with those measured before 5% was added.
const FROM: f32 = 0.1;
use crate::worker::private_bytes;

/// Frames the view must be sharp for before it counts: one sharp frame can be
/// a page drawn at the old zoom, stretched.
const SHARP_FRAMES: usize = 3;

/// Given up on after this long, so a page that never comes sharp still reports.
const GIVE_UP: f64 = 30.0;

/// How long the view rests at `FROM` before zooming in, unless
/// `KINETIC_PDF_ZOOM_BENCH` says otherwise. Long enough for the page under the
/// middle of the view to be drawn ahead at the zoom it would land on.
const REST: f64 = 6.0;

enum Stage {
    /// Waiting for the file to open.
    Opening,
    /// At `FROM`, resting until this time.
    Resting { until: f64 },
    /// Zooming in, a step a frame, from this time. `left` steps to go.
    Zooming { at: f64, left: usize, stood_in: f64 },
    /// At the deepest zoom since this time, waiting for the view to be sharp.
    Zoomed { at: f64, sharp_frames: usize, stood_in: f64 },
}

pub(super) struct ZoomBench {
    /// The page to zoom in on, counted from 0.
    page: usize,
    rest: f64,
    /// Frames the zoom is spread over: one is a jump, more is a fast wheel.
    steps: usize,
    stage: Stage,
}

impl ZoomBench {
    pub(super) fn from_env() -> Option<ZoomBench> {
        let setting = std::env::var("KINETIC_PDF_ZOOM_BENCH").ok()?;
        let mut fields = setting.split(':');
        let page = fields.next()?.trim().parse::<usize>().ok()?;
        let rest = fields.next().and_then(|f| f.trim().parse().ok()).unwrap_or(REST);
        let steps = fields.next().and_then(|f| f.trim().parse().ok()).unwrap_or(1usize).max(1);
        Some(ZoomBench { page: page.saturating_sub(1), rest, steps, stage: Stage::Opening })
    }
}

impl App {
    /// Runs the zoom benchmark, a frame at a time; see the module comment.
    pub(super) fn zoom_bench(&mut self, ctx: &egui::Context) {
        let Some(mut bench) = self.zoom_bench.take() else { return };
        let Some(doc) = self.doc.as_ref() else {
            self.zoom_bench = Some(bench);
            return;
        };
        let (now, pages) = (Self::now(ctx), doc.sizes.len());
        let squares: usize = doc.tiles.values().map(|t| t.handle.size()[0] * t.handle.size()[1] * 4).sum();
        let kept_squares = doc.tiles.len();
        let (smallest, deepest) = (FROM, ZOOMS[ZOOMS.len() - 1]);
        ctx.request_repaint();
        match bench.stage {
            Stage::Opening => {
                let page = bench.page.min(pages.saturating_sub(1));
                bench.stage = Stage::Resting { until: now + bench.rest };
                self.zoom_mode = super::ZoomMode::Custom;
                self.change_zoom(smallest, None);
                self.go_to_page(page);
                eprintln!("zoom bench: page {} of {pages}, resting {:.1} s at {:.0}%", page + 1, bench.rest, smallest * 100.0);
            }
            Stage::Resting { until } if now >= until => {
                eprintln!("zoom bench: drawn ahead before zooming: {kept_squares} squares, {} MB", squares >> 20);
                bench.stage = match bench.steps {
                    1 => {
                        // Anchored at the middle of the view, as drawing ahead aims.
                        self.change_zoom(deepest, None);
                        Stage::Zoomed { at: now, sharp_frames: 0, stood_in: 0.0 }
                    }
                    steps => Stage::Zooming { at: now, left: steps, stood_in: 0.0 },
                };
            }
            Stage::Resting { .. } => {}
            // A step a frame, as spinning the wheel in fast does, so what the
            // view shows on the way in is what a zoom like that really shows.
            Stage::Zooming { at, left, stood_in } => {
                let stood_in = stood_in + f64::from(self.view_stood_in) * ctx.input(|i| i.stable_dt).min(0.1) as f64;
                let step = (bench.steps - left + 1) as f32 / bench.steps as f32;
                self.change_zoom(smallest * (deepest / smallest).powf(step), None);
                bench.stage = match left - 1 {
                    0 => Stage::Zoomed { at, sharp_frames: 0, stood_in },
                    left => Stage::Zooming { at, left, stood_in },
                };
            }
            Stage::Zoomed { at, sharp_frames, stood_in } => {
                let stood_in = stood_in + f64::from(self.view_stood_in) * ctx.input(|i| i.stable_dt).min(0.1) as f64;
                let sharp = if self.view_sharp { sharp_frames + 1 } else { 0 };
                bench.stage = Stage::Zoomed { at, sharp_frames: sharp, stood_in };
                let waited = (now - at) * 1000.0;
                if sharp >= SHARP_FRAMES {
                    eprintln!("zoom bench: {:.0}% -> {:.0}% over {} frames, sharp after {waited:.0} ms", smallest * 100.0, deepest * 100.0, bench.steps);
                    eprintln!("zoom bench: standing in for a drawing for {:.0} ms of that", stood_in * 1000.0);
                    eprintln!("zoom-bench-ms: {waited:.0}");
                    eprintln!("zoom-bench-stood-in-ms: {:.0}", stood_in * 1000.0);
                } else if now - at < GIVE_UP {
                    self.zoom_bench = Some(bench);
                    return;
                } else {
                    eprintln!("zoom bench: not sharp within {GIVE_UP:.0} s");
                    eprintln!("zoom-bench-ms: none");
                }
                eprintln!("zoom bench: process memory {} MB", private_bytes() >> 20);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                self.allow_close = true;
                return;
            }
        }
        self.zoom_bench = Some(bench);
    }
}
