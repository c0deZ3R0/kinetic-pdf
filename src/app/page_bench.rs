//! A benchmark of showing a sheet, with `KINETIC_PDF_PAGE_BENCH=<sheets>[:<zoom
//! percent>]`: the view goes to that many sheets spread through the document,
//! one after another, and times how long each takes to come sharp on screen.
//! Then it reports to stderr and the app closes.
//!
//! This is the one measurement that compares the two renderers fairly. Reading
//! it out of the trace doesn't: pdfium's own timing is a whole rasterisation,
//! in a helper process competing with two others, while the GPU renderer's is
//! the read alone, without the upload that follows it or the thumbnail drawn
//! from it -- and half its reads are thumbnails at an eighth of a pixel a
//! point, which are not sheets at all. Here the clock starts when the view is
//! sent to a sheet and stops when that sheet is sharp, whoever drew it and
//! whatever they had to do to get there, so `KINETIC_PDF_GPU=0` measures the
//! same thing this does.
//!
//! Sheets are spread out rather than taken in order so that drawing ahead
//! hasn't already done the work: the app draws a page or so either side of the
//! view, and the gaps here are far wider than that.

use eframe::egui;

use super::App;
use crate::worker::private_bytes;

/// Frames a sheet must be sharp for before it counts: one sharp frame can be
/// the sheet before it, still up.
const SHARP_FRAMES: usize = 3;

/// A sheet that hasn't come sharp in this long is recorded as this long, so one
/// bad sheet can't hang the benchmark.
const GIVE_UP: f64 = 20.0;

/// How long to wait after opening before the first sheet is asked for, so the
/// file is open and the first page settled.
const SETTLE: f64 = 3.0;

/// The zoom the sheets are shown at unless the setting says otherwise. Roughly
/// a drawing sheet at readable size on a normal display.
const ZOOM: f32 = 1.0;

enum Stage {
    /// Waiting for the file to open, then settling until this time.
    Opening,
    Settling { until: f64 },
    /// Showing `sheets[at]`, asked for at `since`.
    Showing { at: usize, since: f64, sharp_frames: usize },
}

pub(super) struct PageBench {
    /// How many sheets to visit.
    wanted: usize,
    zoom: f32,
    /// The sheets chosen, once the page count is known.
    sheets: Vec<usize>,
    /// What each took, in milliseconds.
    took: Vec<f64>,
    stage: Stage,
}

impl PageBench {
    pub(super) fn from_env() -> Option<PageBench> {
        let setting = std::env::var("KINETIC_PDF_PAGE_BENCH").ok()?;
        let (sheets, zoom) = setting.split_once(':').unwrap_or((setting.as_str(), ""));
        Some(PageBench {
            wanted: sheets.trim().parse::<usize>().ok()?.max(1),
            zoom: zoom.trim().parse::<f32>().map(|percent| percent / 100.0).unwrap_or(ZOOM),
            sheets: Vec::new(),
            took: Vec::new(),
            stage: Stage::Opening,
        })
    }
}

/// `count` sheets spread evenly through `n`, so drawing ahead hasn't reached
/// the next one by the time it is asked for.
fn spread(n: usize, count: usize) -> Vec<usize> {
    if n <= count {
        (0..n).collect()
    } else {
        (0..count).map(|i| i * n / count).collect()
    }
}

fn median(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[sorted.len() / 2]
}

impl App {
    /// Runs the sheet benchmark, a frame at a time; see the module comment.
    pub(super) fn page_bench(&mut self, ctx: &egui::Context) {
        let Some(mut bench) = self.page_bench.take() else { return };
        let Some(doc) = self.doc.as_ref() else {
            self.page_bench = Some(bench);
            return;
        };
        let (now, pages) = (Self::now(ctx), doc.sizes.len());
        ctx.request_repaint();
        match bench.stage {
            Stage::Opening => {
                bench.sheets = spread(pages, bench.wanted);
                bench.stage = Stage::Settling { until: now + SETTLE };
                self.zoom_mode = super::ZoomMode::Custom;
                self.change_zoom(bench.zoom, None);
                eprintln!(
                    "page bench: {} of {pages} sheets at {:.0}%, {}",
                    bench.sheets.len(),
                    bench.zoom * 100.0,
                    if self.gpu.is_some() { "our renderer" } else { "pdfium only" }
                );
            }
            Stage::Settling { until } if now >= until => {
                bench.stage = Stage::Showing { at: 0, since: now, sharp_frames: 0 };
                self.go_to_page(bench.sheets[0]);
            }
            Stage::Settling { .. } => {}
            Stage::Showing { at, since, sharp_frames } => {
                let sharp = if self.view_sharp { sharp_frames + 1 } else { 0 };
                let waited = (now - since) * 1000.0;
                let done = sharp >= SHARP_FRAMES || now - since >= GIVE_UP;
                if !done {
                    bench.stage = Stage::Showing { at, since, sharp_frames: sharp };
                    self.page_bench = Some(bench);
                    return;
                }
                eprintln!("page bench: sheet {} sharp after {waited:.0} ms", bench.sheets[at] + 1);
                bench.took.push(waited);
                match at + 1 {
                    next if next < bench.sheets.len() => {
                        bench.stage = Stage::Showing { at: next, since: now, sharp_frames: 0 };
                        self.go_to_page(bench.sheets[next]);
                    }
                    _ => {
                        let mut sorted = bench.took.clone();
                        sorted.sort_by(f64::total_cmp);
                        let worst = sorted.last().copied().unwrap_or(0.0);
                        eprintln!(
                            "page bench: {} sheets, median {:.0} ms, worst {worst:.0} ms",
                            sorted.len(),
                            median(&sorted)
                        );
                        eprintln!("page-bench-median-ms: {:.0}", median(&sorted));
                        eprintln!("page-bench-worst-ms: {worst:.0}");
                        eprintln!("page-bench-memory-mb: {}", private_bytes() >> 20);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        self.allow_close = true;
                        return;
                    }
                }
            }
        }
        self.page_bench = Some(bench);
    }
}
