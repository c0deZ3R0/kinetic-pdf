//! `cargo run --release -p gpu-lines --example viewer -- file.pdf [page]`
//!
//! One page, its stamps' lines drawn on the GPU over pdfium's drawing of the
//! page without them, next to pdfium's own drawing of the whole page. Scroll
//! to zoom around the pointer, drag to pan. "Sweep zoom" zooms in and out
//! continuously and reports frame times; `GPU_LINES_VSYNC=0` lifts the frame
//! rate cap so they mean something.
//!
//! The page is read from the copy the app draws from (annotations on hidden
//! layers left out, stamps' lines merged), with its annotations flattened into
//! the page so pdfium places them.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::{egui, egui_glow, glow};
use egui::{pos2, vec2, Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions};
use gpu_lines::{extract, Primitive, Renderer};
use pdf_annotate::{annots, merge, worker};
use pdfium_render::prelude::*;

/// Pdfium's drawings of the page are this wide, for the background and for
/// comparing.
const PAGE_PIXELS: f32 = 4096.0;

struct Loaded {
    page_size: [f32; 2],
    background: ([usize; 2], Vec<u8>),
    reference: ([usize; 2], Vec<u8>),
    primitives: Vec<Primitive>,
    report: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    GpuOverPage,
    GpuOnly,
    Pdfium,
    Compare,
}

struct Viewer {
    loading: Option<mpsc::Receiver<Result<Loaded, String>>>,
    error: Option<String>,
    page_size: [f32; 2],
    background: Option<TextureHandle>,
    reference: Option<TextureHandle>,
    report: Vec<String>,
    line_count: usize,
    /// Lines waiting to be uploaded, which can only happen with the GL
    /// context in hand, inside the paint callback.
    pending: Arc<Mutex<Option<Vec<Primitive>>>>,
    renderer: Arc<Mutex<Option<Result<Renderer, String>>>>,
    /// Page top left, relative to the view's top left, and screen points per
    /// page point.
    offset: egui::Vec2,
    zoom: f32,
    /// The zoom at which the page fits the view's width.
    fit: f32,
    fitted: bool,
    mode: Mode,
    sweep: Option<Instant>,
    last_frame: Option<Instant>,
    frame_times: Vec<f32>,
    /// Microseconds the last measured GPU draw took, waiting for it to finish.
    draw_micros: Arc<AtomicU32>,
    draw_times: Vec<f32>,
    /// With `GPU_LINES_BENCH=1`: sweep as soon as the lines are uploaded, print
    /// the times to stderr after `BENCH_FOR`, take `SHOTS`, and close.
    bench: bool,
    frames: u64,
    view_size: egui::Vec2,
    /// The screenshot being taken: its index in `SHOTS`, the frame its view
    /// was set up on, and whether the screenshot has been asked for.
    shot: Option<(usize, u64, bool)>,
}

const BENCH_FOR: Duration = Duration::from_secs(10);

/// Screenshots the benchmark takes, centred on `GPU_LINES_BENCH_AT` ("x,y" in
/// page points, the page's middle if unset), at a zoom relative to fit width,
/// into `GPU_LINES_BENCH_DIR` (the current folder if unset).
const SHOTS: [(Mode, f32, &str); 4] = [
    (Mode::GpuOverPage, 1.0, "gpu-fit"),
    (Mode::Pdfium, 1.0, "pdfium-fit"),
    (Mode::GpuOverPage, 6.0, "gpu-600"),
    (Mode::Pdfium, 6.0, "pdfium-600"),
];

fn load(path: PathBuf, page_number: usize) -> Result<Loaded, String> {
    let mut report = Vec::new();
    let pdfium = worker::bind()?;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;

    let started = Instant::now();
    let drawing = match merge::merge_document(&bytes, merge::MOST_PARTS)? {
        Some((copy, stats)) => {
            report.push(format!(
                "Drawing copy made in {:.0} ms: {} hidden annotations left out, {} of {} strokes merged",
                started.elapsed().as_secs_f64() * 1000.0,
                stats.hidden,
                stats.merged,
                stats.strokes
            ));
            copy
        }
        None => bytes,
    };

    let index = page_number.saturating_sub(1) as PdfPageIndex;
    let doc = pdfium.load_pdf_from_byte_slice(&drawing, None).map_err(|e| e.to_string())?;
    let page = doc.pages().get(index).map_err(|e| e.to_string())?;
    let page_size = [page.width().value, page.height().value];
    let scale = PAGE_PIXELS / page_size[0];

    let started = Instant::now();
    let reference = annots::render_loaded_page(&page, scale)?;
    report.push(format!("pdfium drew the whole page {} x {} px in {:.0} ms", reference.0[0], reference.0[1], started.elapsed().as_secs_f64() * 1000.0));
    let config = PdfRenderConfig::new().scale_page_by_factor(scale).render_annotations(false).render_form_data(false);
    let bitmap = page.render_with_config(&config).map_err(|e| e.to_string())?;
    let background = ([bitmap.width() as usize, bitmap.height() as usize], bitmap.as_rgba_bytes());
    drop(bitmap);
    drop(page);

    // Flatten the annotations into the page, so their appearances sit where
    // pdfium puts them; what was there before stays first.
    let flat = pdfium.load_pdf_from_byte_slice(&drawing, None).map_err(|e| e.to_string())?;
    let before = {
        let mut page = flat.pages().get(index).map_err(|e| e.to_string())?;
        let before = page.objects().len();
        page.flatten().map_err(|e| e.to_string())?;
        before
    };
    let page = flat.pages().get(index).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let objects = page.objects();
    let found = extract((before..objects.len()).filter_map(|i| objects.get(i).ok()), 0.05);
    report.push(format!(
        "Read {} line pieces from {} stroked paths and {} triangles from {} filled paths in {:.0} ms",
        found.lines,
        found.stroked,
        found.triangles,
        found.filled,
        started.elapsed().as_secs_f64() * 1000.0
    ));
    report.push(format!(
        "Not drawn on the GPU: {} other objects, {} fills that wouldn't tessellate; drawn without their effect: {} clipped, {} dashed, {} see-through",
        found.other, found.unfilled, found.clipped, found.dashed, found.see_through
    ));
    Ok(Loaded { page_size, background, reference, primitives: found.primitives, report })
}

impl Viewer {
    fn new(cc: &eframe::CreationContext<'_>, path: PathBuf, page: usize) -> Self {
        let (tx, rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(load(path, page));
            ctx.request_repaint();
        });
        Viewer {
            loading: Some(rx),
            error: None,
            page_size: [1.0, 1.0],
            background: None,
            reference: None,
            report: vec!["Loading...".to_owned()],
            line_count: 0,
            pending: Arc::new(Mutex::new(None)),
            renderer: Arc::new(Mutex::new(None)),
            offset: egui::Vec2::ZERO,
            zoom: 1.0,
            fit: 1.0,
            fitted: false,
            mode: Mode::GpuOverPage,
            sweep: None,
            last_frame: None,
            frame_times: Vec::new(),
            draw_micros: Arc::new(AtomicU32::new(0)),
            draw_times: Vec::new(),
            bench: std::env::var_os("GPU_LINES_BENCH").is_some_and(|v| v == "1"),
            frames: 0,
            view_size: vec2(1.0, 1.0),
            shot: None,
        }
    }

    /// Takes the benchmark's screenshots one at a time: sets up the view, gives
    /// it a few frames to draw, asks for a screenshot and saves it when it
    /// arrives. Closes after the last.
    fn take_shots(&mut self, ctx: &egui::Context) {
        let Some((index, set_at, asked)) = self.shot else { return };
        let Some(&(mode, factor, name)) = SHOTS.get(index) else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.shot = None;
            self.bench = false;
            return;
        };
        if self.frames == set_at {
            let at = std::env::var("GPU_LINES_BENCH_AT")
                .ok()
                .and_then(|s| s.split_once(',').and_then(|(x, y)| Some([x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?])))
                .unwrap_or([self.page_size[0] / 2.0, self.page_size[1] / 2.0]);
            self.mode = mode;
            self.zoom = self.fit * factor;
            let centre = self.view_size / 2.0;
            self.offset = centre - vec2(at[0], self.page_size[1] - at[1]) * self.zoom;
            return;
        }
        if !asked && self.frames >= set_at + 3 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.shot = Some((index, set_at, true));
            return;
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(Arc::clone(image)),
                _ => None,
            })
        });
        if let Some(image) = image {
            let dir = std::env::var_os("GPU_LINES_BENCH_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            let path = dir.join(format!("{name}.png"));
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            let saved = std::fs::File::create(&path).map_err(|e| e.to_string()).and_then(|file| {
                let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), image.size[0] as u32, image.size[1] as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.write_header().and_then(|mut w| w.write_image_data(&rgba)).map_err(|e| e.to_string())
            });
            match saved {
                Ok(()) => eprintln!("saved {}", path.display()),
                Err(e) => eprintln!("couldn't save {}: {e}", path.display()),
            }
            self.shot = Some((index + 1, self.frames + 1, false));
        }
    }

    /// In benchmark mode: starts sweeping once the lines are on the GPU, and
    /// after a while prints what it measured and closes.
    fn run_bench(&mut self, ctx: &egui::Context) {
        if !self.bench {
            return;
        }
        let uploaded = self.pending.lock().unwrap().is_none() && matches!(&*self.renderer.lock().unwrap(), Some(Ok(_)));
        let failed = self.error.clone().or_else(|| match &*self.renderer.lock().unwrap() {
            Some(Err(e)) => Some(e.clone()),
            _ => None,
        });
        if let Some(error) = failed {
            eprintln!("bench failed: {error}");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.bench = false;
            return;
        }
        match self.sweep {
            None if uploaded && self.line_count > 0 => {
                self.mode = Mode::GpuOverPage;
                self.sweep = Some(Instant::now());
                self.frame_times.clear();
                self.draw_times.clear();
            }
            Some(started) if started.elapsed() >= BENCH_FOR => {
                let summary = |times: &[f32]| {
                    let mut sorted = times.to_vec();
                    sorted.sort_by(f32::total_cmp);
                    let pick = |q: f32| sorted.get(((sorted.len() as f32 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
                    format!("median {:.2} ms, 95th percentile {:.2} ms, worst {:.2} ms over {} frames", pick(0.5), pick(0.95), pick(1.0), sorted.len())
                };
                for line in &self.report {
                    eprintln!("{line}");
                }
                eprintln!("{} shapes on the GPU ({:.1} MB)", self.line_count, self.line_count as f64 * 48.0 / 1e6);
                eprintln!("sweeping zoom between 141% and 1131% of fit width, vsync {}", if std::env::var_os("GPU_LINES_VSYNC").is_some_and(|v| v == "0") { "off" } else { "on" });
                eprintln!("frame to frame: {}", summary(&self.frame_times));
                eprintln!("GPU draw, waited for: {}", summary(&self.draw_times));
                self.sweep = None;
                self.shot = Some((0, self.frames + 1, false));
            }
            _ => {}
        }
    }

    fn receive(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.loading else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.loading = None;
        match result {
            Ok(loaded) => {
                let texture = |name: &str, (size, rgba): &([usize; 2], Vec<u8>)| {
                    ctx.load_texture(name, ColorImage::from_rgba_unmultiplied(*size, rgba), TextureOptions::LINEAR)
                };
                self.background = Some(texture("page without stamps", &loaded.background));
                self.reference = Some(texture("page drawn by pdfium", &loaded.reference));
                self.page_size = loaded.page_size;
                self.line_count = loaded.primitives.len();
                self.report = loaded.report;
                *self.pending.lock().unwrap() = Some(loaded.primitives);
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn page_rect(&self, view: Rect) -> Rect {
        Rect::from_min_size(view.min + self.offset, vec2(self.page_size[0], self.page_size[1]) * self.zoom)
    }

    fn view(&mut self, ui: &mut egui::Ui) {
        let view = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(view, Sense::click_and_drag());
        self.fit = view.width() / self.page_size[0];
        self.view_size = view.size();
        if !self.fitted && self.background.is_some() {
            self.zoom = view.width() / self.page_size[0];
            self.offset = vec2(0.0, (view.height() - self.page_size[1] * self.zoom).max(0.0) / 2.0);
            self.fitted = true;
        }

        // Zoom around the pointer, pan by dragging, or sweep.
        let (scroll, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.hover_pos()));
        if let Some(started) = self.sweep {
            let fit = view.width() / self.page_size[0];
            let t = started.elapsed().as_secs_f32();
            let target = fit * 2f32.powf(2.0 + 1.5 * (t * 0.8).sin());
            let centre = view.center() - view.min;
            let page_point = (centre - self.offset) / self.zoom;
            self.zoom = target;
            self.offset = centre - page_point * self.zoom;
        } else if response.hovered() && scroll != 0.0 {
            let factor = (scroll * 0.0025).exp();
            let anchor = pointer.map_or(view.center(), |p| p) - view.min;
            self.offset = anchor - (anchor - self.offset) * factor;
            self.zoom *= factor;
        }
        self.offset += response.drag_delta();

        let painter = ui.painter_at(view);
        painter.rect_filled(view, 0.0, Color32::from_gray(60));
        let page = self.page_rect(view);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        match self.mode {
            Mode::GpuOverPage => {
                if let Some(t) = &self.background {
                    painter.image(t.id(), page, uv, Color32::WHITE);
                }
            }
            Mode::GpuOnly => {
                painter.rect_filled(page, 0.0, Color32::WHITE);
            }
            Mode::Pdfium => {
                if let Some(t) = &self.reference {
                    painter.image(t.id(), page, uv, Color32::WHITE);
                }
            }
            Mode::Compare => {
                if let Some(t) = &self.reference {
                    painter.image(t.id(), page, uv, Color32::WHITE);
                }
                painter.rect_filled(page, 0.0, Color32::from_white_alpha(150));
            }
        }

        if self.mode != Mode::Pdfium {
            let (renderer, pending, draw_micros) = (Arc::clone(&self.renderer), Arc::clone(&self.pending), Arc::clone(&self.draw_micros));
            let (zoom, offset, height, measure) = (self.zoom, self.offset, self.page_size[1], self.sweep.is_some());
            let callback = egui_glow::CallbackFn::new(move |info, painter| {
                let gl: &glow::Context = painter.gl();
                let mut slot = renderer.lock().unwrap();
                let renderer = slot.get_or_insert_with(|| Renderer::new(gl));
                let Ok(renderer) = renderer else { return };
                if let Some(lines) = pending.lock().unwrap().take() {
                    renderer.upload(gl, &lines);
                }
                let ppp = info.pixels_per_point;
                let viewport = info.viewport_in_pixels();
                let scale = zoom * ppp;
                let page_to_pixels = [scale, 0.0, 0.0, -scale, offset.x * ppp, offset.y * ppp + height * scale];
                let started = Instant::now();
                renderer.paint(gl, page_to_pixels, [viewport.width_px as f32, viewport.height_px as f32], scale);
                if measure {
                    use glow::HasContext as _;
                    unsafe { gl.finish() };
                    draw_micros.store(started.elapsed().as_micros() as u32, Ordering::Relaxed);
                }
            });
            painter.add(egui::PaintCallback { rect: view, callback: Arc::new(callback) });
        }
    }
}

impl eframe::App for Viewer {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frames += 1;
        self.receive(&ctx);
        if self.shot.is_some() {
            self.take_shots(&ctx);
        } else {
            self.run_bench(&ctx);
        }

        let now = Instant::now();
        if let (Some(last), Some(_)) = (self.last_frame, self.sweep) {
            self.frame_times.push((now - last).as_secs_f32() * 1000.0);
            self.draw_times.push(self.draw_micros.load(Ordering::Relaxed) as f32 / 1000.0);
            for times in [&mut self.frame_times, &mut self.draw_times] {
                if times.len() > 240 {
                    times.remove(0);
                }
            }
        }
        self.last_frame = Some(now);

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.mode, Mode::GpuOverPage, "GPU lines over page");
                ui.selectable_value(&mut self.mode, Mode::GpuOnly, "GPU lines only");
                ui.selectable_value(&mut self.mode, Mode::Pdfium, "pdfium");
                ui.selectable_value(&mut self.mode, Mode::Compare, "Compare (GPU over faded pdfium)");
                let mut sweeping = self.sweep.is_some();
                if ui.checkbox(&mut sweeping, "Sweep zoom").changed() {
                    self.sweep = sweeping.then(Instant::now);
                    self.frame_times.clear();
                    self.draw_times.clear();
                }
                if ui.button("Fit").clicked() {
                    self.fitted = false;
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(Color32::RED, error);
            }
            for line in &self.report {
                ui.label(line);
            }
            let renderer_error = match &*self.renderer.lock().unwrap() {
                Some(Err(e)) => Some(e.clone()),
                _ => None,
            };
            if let Some(e) = renderer_error {
                ui.colored_label(Color32::RED, e);
            }
            let average = |times: &[f32]| if times.is_empty() { 0.0 } else { times.iter().sum::<f32>() / times.len() as f32 };
            let worst = |times: &[f32]| times.iter().copied().fold(0.0_f32, f32::max);
            ui.label(format!(
                "{} shapes on the GPU ({:.0} MB); zoom {:.0}% of fit width{}",
                self.line_count,
                self.line_count as f64 * 48.0 / 1e6,
                self.zoom / self.fit.max(1e-6) * 100.0,
                if self.sweep.is_some() {
                    format!(
                        "; frames {:.1} ms average, {:.1} ms worst; GPU draw {:.2} ms average, {:.2} ms worst",
                        average(&self.frame_times),
                        worst(&self.frame_times),
                        average(&self.draw_times),
                        worst(&self.draw_times)
                    )
                } else {
                    String::new()
                }
            ));
            self.view(ui);
        });

        if self.sweep.is_some() || self.loading.is_some() || self.bench || self.shot.is_some() {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next().map(PathBuf::from) else {
        eprintln!("usage: viewer file.pdf [page]");
        std::process::exit(2);
    };
    let page = args.next().and_then(|p| p.parse().ok()).unwrap_or(1);
    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default().with_title("GPU lines").with_inner_size([1400.0, 900.0]),
        ..Default::default()
    };
    options.glow_options.vsync = std::env::var_os("GPU_LINES_VSYNC").is_none_or(|v| v != "0");
    // Smooths the edges of filled triangles; lines smooth their own.
    options.multisampling = 4;
    eframe::run_native("GPU lines", options, Box::new(move |cc| Ok(Box::new(Viewer::new(cc, path, page)))))
}
