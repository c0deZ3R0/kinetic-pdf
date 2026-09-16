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

/// Sheets either side of a sampled one that the app may already have drawn:
/// `pages::LOOK_AHEAD` of drawing ahead, plus the sheets in view at this zoom.
/// Samples are spaced wider than this, or they would time work already done.
const REACH: usize = super::pages::LOOK_AHEAD + 2;

/// The zoom the sheets are shown at unless the setting says otherwise. Roughly
/// a drawing sheet at readable size on a normal display.
const ZOOM: f32 = 1.0;

enum Stage {
    /// Waiting for the file to open, then settling until this time.
    Opening,
    Settling { until: f64 },
    /// Showing `sheets[at]`, asked for at `since`. `saw_unsharp` is the test
    /// that this sheet was really drawn for us: if the view was never once
    /// unsharp, the sheet was already there when we arrived -- drawn ahead, or
    /// the one the file opened on -- and timing it measures nothing.
    Showing { at: usize, since: f64, sharp_frames: usize, saw_unsharp: bool },
}

pub(super) struct PageBench {
    /// How many sheets to visit.
    wanted: usize,
    zoom: f32,
    /// The sheets chosen, once the page count is known.
    sheets: Vec<usize>,
    /// What each took, in milliseconds.
    took: Vec<f64>,
    /// Sheets that were already drawn when we got to them, so weren't counted.
    skipped: usize,
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
            skipped: 0,
            stage: Stage::Opening,
        })
    }
}

/// At most `count` sheets spread through `n`, never closer together than
/// `REACH`, so the app hasn't already drawn the next one by the time it is
/// asked for. Fewer sheets than asked for is better than sheets that measure
/// nothing.
fn spread(n: usize, count: usize) -> Vec<usize> {
    let count = count.min(n / REACH.max(1)).max(1);
    let step = (n / count).max(REACH);
    (0..count).map(|i| i * step).take_while(|&page| page < n).collect()
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
                bench.stage = Stage::Showing { at: 0, since: now, sharp_frames: 0, saw_unsharp: false };
                self.go_to_page(bench.sheets[0]);
            }
            Stage::Settling { .. } => {}
            Stage::Showing { at, since, sharp_frames, saw_unsharp } => {
                let sharp = if self.view_sharp { sharp_frames + 1 } else { 0 };
                let saw_unsharp = saw_unsharp || !self.view_sharp;
                let waited = (now - since) * 1000.0;
                let done = sharp >= SHARP_FRAMES || now - since >= GIVE_UP;
                if !done {
                    bench.stage = Stage::Showing { at, since, sharp_frames: sharp, saw_unsharp };
                    self.page_bench = Some(bench);
                    return;
                }
                if saw_unsharp {
                    eprintln!("page bench: sheet {} sharp after {waited:.0} ms", bench.sheets[at] + 1);
                    bench.took.push(waited);
                } else {
                    // Never once unsharp: it was drawn before we asked for it.
                    eprintln!("page bench: sheet {} was already drawn; not counted", bench.sheets[at] + 1);
                    bench.skipped += 1;
                }
                match at + 1 {
                    next if next < bench.sheets.len() => {
                        bench.stage = Stage::Showing { at: next, since: now, sharp_frames: 0, saw_unsharp: false };
                        self.go_to_page(bench.sheets[next]);
                    }
                    _ => {
                        let mut sorted = bench.took.clone();
                        sorted.sort_by(f64::total_cmp);
                        let worst = sorted.last().copied().unwrap_or(0.0);
                        eprintln!(
                            "page bench: {} sheets counted ({} already drawn), median {:.0} ms, worst {worst:.0} ms",
                            sorted.len(),
                            bench.skipped,
                            median(&sorted)
                        );
                        eprintln!("page-bench-counted: {}", sorted.len());
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
