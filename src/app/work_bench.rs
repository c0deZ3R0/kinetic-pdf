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
//!
//! What "sharp" means is the app's own word for it (`view_sharp`): everything
//! in view is drawn by what it should be, at the zoom in view. It is not proof
//! that frame reached the screen, so the time is *time to the app reporting the
//! view sharp*, not input-to-photon latency. Its resolution is the frame: the
//! step ends on the third sharp frame, so the shortest step this can report is
//! about three frame intervals, and those are reported alongside so a result
//! near that floor can be recognised as one.
//!
//! With `KINETIC_PDF_SHOTS=<folder>`, the view is also saved as a picture once
//! it is sharp at 200% and at 800%, untimed, so the two renderers' output can
//! be compared by eye as well as by the clock.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui;

use super::App;
use crate::worker::private_bytes;

/// Frames the view must be sharp for before a step counts as finished.
const SHARP_FRAMES: usize = 3;

/// A step that hasn't come sharp in this long is recorded as this long.
const GIVE_UP: f64 = 15.0;

/// How long to wait after opening before going to the sheet.
const SETTLE: f64 = 3.0;

/// How long to wait for a picture of the view before going on without it.
const SHOT_GIVE_UP: f64 = 5.0;

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

/// Steps after which the view is pictured, and what the picture is called.
/// Neither is panned, so both renderers are pictured at the same place.
const SHOTS: [(usize, &str); 2] = [(0, "200"), (6, "800")];

enum Stage {
    Opening,
    Settling { until: f64 },
    /// Waiting for the sheet to be drawn the first time; not timed.
    WarmingUp { since: f64, sharp_frames: usize },
    /// Doing `STEPS[at]`, started at `since`.
    Step { at: usize, since: f64, sharp_frames: usize },
    /// Picturing the view after `STEPS[after]`, asked for at `since`; not timed.
    Shooting { after: usize, name: &'static str, since: f64 },
}

pub(super) struct WorkBench {
    /// The sheet to work on, counted from 0, or `None` for the middle one --
    /// which is what `0` asks for, so a caller needn't know the page count.
    sheet: Option<usize>,
    took: Vec<f64>,
    /// Time between frames while steps were timed, in milliseconds.
    frames: Vec<f64>,
    last_frame: Option<f64>,
    /// Pages our renderer gave to pdfium at some point.
    fell_back: HashSet<usize>,
    shots: Option<PathBuf>,
    stage: Stage,
}

impl WorkBench {
    pub(super) fn from_env() -> Option<WorkBench> {
        let sheet = std::env::var("KINETIC_PDF_WORK_BENCH").ok()?;
        let sheet = sheet.trim().parse::<usize>().ok()?;
        Some(WorkBench {
            sheet: sheet.checked_sub(1),
            took: Vec::new(),
            frames: Vec::new(),
            last_frame: None,
            fell_back: HashSet::new(),
            shots: std::env::var_os("KINETIC_PDF_SHOTS").map(PathBuf::from),
            stage: Stage::Opening,
        })
    }
}

/// The value `share` of the way through `sorted`, nearest rank.
pub(super) fn percentile(sorted: &[f64], share: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((sorted.len() as f64 * share).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
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
        self.note_fallbacks(&mut bench.fell_back);
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
                bench.fell_back.clear();
                bench.last_frame = Some(now);
                bench.stage = Stage::Step { at: 0, since: now, sharp_frames: 0 };
                self.take_step(STEPS[0]);
            }
            Stage::Step { at, since, sharp_frames } => {
                if let Some(last) = bench.last_frame.replace(now) {
                    bench.frames.push((now - last) * 1000.0);
                }
                let sharp = if self.view_sharp { sharp_frames + 1 } else { 0 };
                if sharp < SHARP_FRAMES && now - since < GIVE_UP {
                    bench.stage = Stage::Step { at, since, sharp_frames: sharp };
                    self.work_bench = Some(bench);
                    return;
                }
                let waited = (now - since) * 1000.0;
                let (zoom, pan) = STEPS[at];
                let gave_up = if sharp < SHARP_FRAMES { " (gave up)" } else { "" };
                eprintln!("work bench: {:.0}%{} sharp after {waited:.0} ms{gave_up}", zoom * 100.0, if pan > 0.0 { ", panned" } else { "" });
                bench.took.push(waited);
                match SHOTS.iter().find(|(after, _)| *after == at) {
                    Some(&(after, name)) if bench.shots.is_some() => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                        bench.stage = Stage::Shooting { after, name, since: now };
                    }
                    _ => {
                        if self.next_step(ctx, &mut bench, at + 1, now) {
                            return;
                        }
                    }
                }
            }
            Stage::Shooting { after, name, since } => {
                let image = ctx.input(|i| {
                    i.raw.events.iter().find_map(|event| match event {
                        egui::Event::Screenshot { image, .. } => Some(image.clone()),
                        _ => None,
                    })
                });
                if image.is_none() && now - since < SHOT_GIVE_UP {
                    self.work_bench = Some(bench);
                    return;
                }
                match (image, &bench.shots) {
                    (Some(image), Some(dir)) => {
                        let path = dir.join(format!("{name}.png"));
                        match save_view(&image, self.viewer_rect, ctx.pixels_per_point(), &path) {
                            Ok(()) => eprintln!("work bench: pictured the view at {name}%"),
                            Err(e) => eprintln!("work bench: couldn't picture the view at {name}%: {e}"),
                        }
                    }
                    _ => eprintln!("work bench: no picture of the view at {name}% came"),
                }
                // Picturing isn't timed, and neither is the frame gap it left.
                bench.last_frame = Some(now);
                if self.next_step(ctx, &mut bench, after + 1, now) {
                    return;
                }
            }
        }
        self.work_bench = Some(bench);
    }

    /// Starts `STEPS[next]`, or reports and closes after the last. True once
    /// the benchmark is over.
    fn next_step(&mut self, ctx: &egui::Context, bench: &mut WorkBench, next: usize, now: f64) -> bool {
        if next < STEPS.len() {
            bench.stage = Stage::Step { at: next, since: now, sharp_frames: 0 };
            self.take_step(STEPS[next]);
            return false;
        }
        let mut sorted = bench.took.clone();
        sorted.sort_by(f64::total_cmp);
        let total: f64 = sorted.iter().sum();
        let median = percentile(&sorted, 0.5);
        let worst = sorted.last().copied().unwrap_or(0.0);
        let mut frames = bench.frames.clone();
        frames.sort_by(f64::total_cmp);
        let frame = percentile(&frames, 0.5);
        eprintln!(
            "work bench: {} steps, median {median:.0} ms, worst {worst:.0} ms, {total:.0} ms in all; frames every {frame:.1} ms",
            sorted.len()
        );
        let steps: Vec<String> = bench.took.iter().map(|ms| format!("{ms:.1}")).collect();
        eprintln!("work-bench-steps-ms: {}", steps.join(","));
        eprintln!("work-bench-median-ms: {median:.0}");
        eprintln!("work-bench-worst-ms: {worst:.0}");
        eprintln!("work-bench-total-ms: {total:.0}");
        eprintln!("work-bench-frame-ms: {frame:.2}");
        eprintln!("work-bench-frame-p90-ms: {:.2}", percentile(&frames, 0.9));
        eprintln!("work-bench-fell-back: {}", bench.fell_back.len());
        eprintln!("work-bench-memory-mb: {}", private_bytes() >> 20);
        self.report_setup(ctx, "work-bench");
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        self.allow_close = true;
        true
    }

    /// Zooms to `zoom` and pans down `pan` screenfuls, as one step.
    fn take_step(&mut self, (zoom, pan): (f32, f32)) {
        // Anchored at the middle of the view, which is where drawing ahead aims.
        self.change_zoom(zoom, None);
        if pan > 0.0 {
            self.scroll_y = Some(self.scroll_offset.y + self.viewer_rect.height() * pan);
        }
    }

    /// Adds to `pages` every page our renderer has given to pdfium: being
    /// handed over, handed over, or never drawable on the GPU. With pdfium
    /// drawing everything there is nothing to give, so nothing is added.
    pub(super) fn note_fallbacks(&self, pages: &mut HashSet<usize>) {
        let (Some(doc), Some(_)) = (self.doc.as_ref(), self.gpu.as_ref()) else { return };
        pages.extend(doc.handing_over.iter().copied());
        pages.extend(doc.left_to_pdfium.iter().copied());
        pages.extend(doc.drawing.iter().filter(|(_, state)| matches!(state, super::gpu::PageDrawing::Pdfium)).map(|(page, _)| *page));
    }

    /// What the run was measured on, for the report: the GPU and driver, the
    /// view's size, the display's scaling, and whether frames wait for vsync.
    pub(super) fn report_setup(&self, ctx: &egui::Context, prefix: &str) {
        let ppp = ctx.pixels_per_point();
        let size = self.viewer_rect.size() * ppp;
        let vsync = std::env::var_os("KINETIC_PDF_VSYNC").is_none_or(|v| v != "0");
        eprintln!("{prefix}-gl: {}", self.gl_name);
        eprintln!("{prefix}-view-px: {:.0}x{:.0}", size.x, size.y);
        eprintln!("{prefix}-scaling: {:.0}%", ppp * 100.0);
        eprintln!("{prefix}-vsync: {}", if vsync { "on" } else { "off" });
    }
}

/// Saves the part of a picture of the window that shows pages.
fn save_view(image: &egui::ColorImage, view: egui::Rect, ppp: f32, path: &std::path::Path) -> Result<(), String> {
    let [width, height] = image.size;
    let left = ((view.min.x * ppp).round().max(0.0) as usize).min(width);
    let top = ((view.min.y * ppp).round().max(0.0) as usize).min(height);
    let right = ((view.max.x * ppp).round().max(0.0) as usize).clamp(left, width);
    let bottom = ((view.max.y * ppp).round().max(0.0) as usize).clamp(top, height);
    let (w, h) = (right - left, bottom - top);
    if w == 0 || h == 0 {
        return Err("the view has no size".to_owned());
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in top..bottom {
        for pixel in &image.pixels[y * width + left..y * width + right] {
            rgba.extend_from_slice(&pixel.to_srgba_unmultiplied());
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
    let mut encoder = png::Encoder::new(file, w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgba).map_err(|e| e.to_string())
}
