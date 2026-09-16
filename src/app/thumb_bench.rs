//! A benchmark of blank pages while scrolling zoomed out, with
//! `KINETIC_PDF_THUMB_BENCH=<zoom percent>[:<screens a second>[:<passes>]]`:
//! the view goes to the top at that zoom, then scrolls to the bottom and back
//! up, pass after pass, counting every frame on which a page in view is blank
//! -- nothing of it at all, not even its thumbnail. Then it reports to stderr
//! and the app closes.
//!
//! Zoomed out, a page seen once should never be blank again: its thumbnail is
//! kept in memory and in the page cache. The first pass down a document never
//! opened before may well show blanks; later passes, or any pass with a cache
//! a previous run filled, should show none.

use std::collections::BTreeSet;

use eframe::egui;

use super::App;
use crate::worker::private_bytes;

/// How long to wait after opening before scrolling.
const SETTLE: f64 = 3.0;

/// A pass ends once the view has been still at the end for this many frames.
const STILL_FRAMES: usize = 20;

struct Pass {
    down: bool,
    frames: usize,
    blank_frames: usize,
    blank_seconds: f64,
    blank_pages: BTreeSet<usize>,
    started: f64,
}

enum Stage {
    Opening,
    Settling { until: f64 },
    Scrolling { still: usize, last_offset: f32, last_time: f64 },
}

pub(super) struct ThumbBench {
    zoom: f32,
    screens_a_second: f32,
    passes_wanted: usize,
    passes: Vec<Pass>,
    stage: Stage,
}

impl ThumbBench {
    pub(super) fn from_env() -> Option<ThumbBench> {
        let setting = std::env::var("KINETIC_PDF_THUMB_BENCH").ok()?;
        let mut fields = setting.split(':');
        let zoom = fields.next()?.trim().parse::<f32>().ok()? / 100.0;
        let screens_a_second = fields.next().and_then(|f| f.trim().parse().ok()).unwrap_or(4.0);
        let passes_wanted = fields.next().and_then(|f| f.trim().parse().ok()).unwrap_or(4usize).max(1);
        Some(ThumbBench { zoom, screens_a_second, passes_wanted, passes: Vec::new(), stage: Stage::Opening })
    }
}

impl App {
    /// Runs the thumbnail benchmark, a frame at a time; see the module comment.
    pub(super) fn thumb_bench(&mut self, ctx: &egui::Context) {
        let Some(mut bench) = self.thumb_bench.take() else { return };
        let Some(doc) = self.doc.as_ref() else {
            self.thumb_bench = Some(bench);
            return;
        };
        let (now, pages) = (Self::now(ctx), doc.sizes.len());
        ctx.request_repaint();
        match bench.stage {
            Stage::Opening => {
                self.zoom_mode = super::ZoomMode::Custom;
                self.change_zoom(bench.zoom, None);
                self.go_to_page(0);
                eprintln!(
                    "thumb bench: {pages} pages at {:.0}%, {} screens a second, {} passes, {}",
                    bench.zoom * 100.0,
                    bench.screens_a_second,
                    bench.passes_wanted,
                    if self.gpu.is_some() { "our renderer" } else { "pdfium only" }
                );
                bench.stage = Stage::Settling { until: now + SETTLE };
            }
            Stage::Settling { until } if now >= until => {
                bench.passes.push(Pass { down: true, frames: 0, blank_frames: 0, blank_seconds: 0.0, blank_pages: BTreeSet::new(), started: now });
                bench.stage = Stage::Scrolling { still: 0, last_offset: f32::NAN, last_time: now };
            }
            Stage::Settling { .. } => {}
            Stage::Scrolling { still, last_offset, last_time } => {
                let offset = self.scroll_offset.y;
                let dt = now - last_time;
                let finished = bench.passes.len();
                let pass = bench.passes.last_mut().expect("a pass is under way");
                pass.frames += 1;
                if !self.blank_pages.is_empty() {
                    pass.blank_frames += 1;
                    pass.blank_seconds += dt;
                    pass.blank_pages.extend(self.blank_pages.iter().copied());
                }
                let still = if offset == last_offset { still + 1 } else { 0 };
                if still >= STILL_FRAMES {
                    let down = pass.down;
                    eprintln!(
                        "thumb bench: pass {finished} {} in {:.1} s: {} of {} frames had a blank page, {:.0} ms in all, pages {}",
                        if down { "down" } else { "up" },
                        now - pass.started,
                        pass.blank_frames,
                        pass.frames,
                        pass.blank_seconds * 1000.0,
                        describe(&pass.blank_pages)
                    );
                    if finished >= bench.passes_wanted {
                        for (i, pass) in bench.passes.iter().enumerate() {
                            eprintln!(
                                "thumb-bench-pass-{}: {} {} {:.0} {}",
                                i + 1,
                                pass.blank_frames,
                                pass.frames,
                                pass.blank_seconds * 1000.0,
                                pass.blank_pages.len()
                            );
                        }
                        eprintln!("thumb-bench-thumbnails-held: {}", doc.thumbnails.len());
                        eprintln!("thumb-bench-memory-mb: {}", private_bytes() >> 20);
                        self.report_setup(ctx, "thumb-bench");
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        self.allow_close = true;
                        return;
                    }
                    bench.passes.push(Pass { down: !down, frames: 0, blank_frames: 0, blank_seconds: 0.0, blank_pages: BTreeSet::new(), started: now });
                    bench.stage = Stage::Scrolling { still: 0, last_offset: f32::NAN, last_time: now };
                    self.thumb_bench = Some(bench);
                    return;
                }
                let down = pass.down;
                let step = bench.screens_a_second * self.viewer_rect.height() * dt as f32;
                self.scroll_y = Some(if down { offset + step } else { (offset - step).max(0.0) });
                bench.stage = Stage::Scrolling { still, last_offset: offset, last_time: now };
            }
        }
        self.thumb_bench = Some(bench);
    }
}

/// Pages, counted from 1, as runs: "3-7, 12".
fn describe(pages: &BTreeSet<usize>) -> String {
    if pages.is_empty() {
        return "none".to_owned();
    }
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &page in pages {
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == page => *end = page,
            _ => runs.push((page, page)),
        }
    }
    runs.iter().map(|&(a, b)| if a == b { format!("{}", a + 1) } else { format!("{}-{}", a + 1, b + 1) }).collect::<Vec<_>>().join(", ")
}
