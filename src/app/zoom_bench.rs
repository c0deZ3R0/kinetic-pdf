//! A repeatable benchmark of zooming in, with
//! `KINETIC_PDF_ZOOM_BENCH=<page>[:<rest seconds>]`: the view goes to that
//! page (counted from 1) at the smallest zoom, waits there while pages are
//! drawn ahead, then zooms straight to the deepest zoom in one step and times
//! how long until everything in view is sharp. Then it reports to stderr and
//! the app closes.
//!
//! Nothing here depends on the pointer: the zoom is anchored at the middle of
//! the view, which is also where drawing ahead aims when no pointer is over
//! the page, so the same thing is measured every time.

use eframe::egui;

use super::{App, ZOOMS};
use crate::worker::private_bytes;

/// Frames the view must be sharp for before it counts: one sharp frame can be
/// a page drawn at the old zoom, stretched.
const SHARP_FRAMES: usize = 3;

/// Given up on after this long, so a page that never comes sharp still reports.
const GIVE_UP: f64 = 30.0;

/// How long the view rests at the smallest zoom before zooming in, unless
/// `KINETIC_PDF_ZOOM_BENCH` says otherwise. Long enough for the page under the
/// middle of the view to be drawn ahead at the zoom it would land on.
const REST: f64 = 6.0;

enum Stage {
    /// Waiting for the file to open.
    Opening,
    /// At the smallest zoom, resting until this time.
    Resting { until: f64 },
    /// Zoomed in at this time, waiting for the view to be sharp.
    Zoomed { at: f64, sharp_frames: usize },
}

pub(super) struct ZoomBench {
    /// The page to zoom in on, counted from 0.
    page: usize,
    rest: f64,
    stage: Stage,
}

impl ZoomBench {
    pub(super) fn from_env() -> Option<ZoomBench> {
        let setting = std::env::var("KINETIC_PDF_ZOOM_BENCH").ok()?;
        let (page, rest) = setting.split_once(':').unwrap_or((setting.as_str(), ""));
        Some(ZoomBench {
            page: page.trim().parse::<usize>().ok()?.saturating_sub(1),
            rest: rest.trim().parse().unwrap_or(REST),
            stage: Stage::Opening,
        })
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
        let (smallest, deepest) = (ZOOMS[0], ZOOMS[ZOOMS.len() - 1]);
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
                bench.stage = Stage::Zoomed { at: now, sharp_frames: 0 };
                // Anchored at the middle of the view, as drawing ahead aims.
                self.change_zoom(deepest, None);
            }
            Stage::Resting { .. } => {}
            Stage::Zoomed { at, sharp_frames } => {
                let sharp = if self.view_sharp { sharp_frames + 1 } else { 0 };
                bench.stage = Stage::Zoomed { at, sharp_frames: sharp };
                let waited = (now - at) * 1000.0;
                if sharp >= SHARP_FRAMES {
                    eprintln!("zoom bench: {:.0}% -> {:.0}%, sharp after {waited:.0} ms", smallest * 100.0, deepest * 100.0);
                    eprintln!("zoom-bench-ms: {waited:.0}");
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
