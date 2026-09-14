//! `cargo run --release -p gpu-lines --example viewer -- file.pdf [page]`
//!
//! One page, its stamps' paths drawn on the GPU over pdfium's drawing of the
//! page without them, next to pdfium's own drawing of the whole page. Scroll
//! to zoom around the pointer, drag to pan. "Sweep zoom" zooms in and out
//! continuously and reports frame times; `GPU_LINES_VSYNC=0` lifts the frame
//! rate cap so they mean something.
//!
//! "pdfium sharp" has pdfium draw exactly the part of the page in view at the
//! zoom in use, once the view has held still, as the app does when zoomed in:
//! the fair picture to hold the GPU's drawing against.
//!
//! The page is read from the copy the app draws from (annotations on hidden
//! layers left out, stamps' lines merged), with its annotations flattened into
//! the page so pdfium places them (loader.rs). `GPU_LINES_BENCH=1` runs the
//! benchmark instead (bench.rs).

mod bench;
mod loader;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::{egui, egui_glow, glow};
use egui::{pos2, vec2, Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions};
use gpu_lines::{Primitive, Renderer, Shapes};

use loader::{FromLoader, SharpRequest};

/// How long the view holds still before pdfium is asked to draw it sharp.
const SHARP_AFTER: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    GpuOverPage,
    GpuOnly,
    Pdfium,
    PdfiumSharp,
    Compare,
}

impl Mode {
    fn wants_sharp(self) -> bool {
        matches!(self, Mode::PdfiumSharp | Mode::Compare)
    }
}

struct Viewer {
    from_loader: mpsc::Receiver<FromLoader>,
    requests: mpsc::Sender<SharpRequest>,
    loading: bool,
    error: Option<String>,
    page_size: [f32; 2],
    background: Option<TextureHandle>,
    reference: Option<TextureHandle>,
    report: Vec<String>,
    shape_count: usize,
    /// Shapes waiting to be uploaded, which can only happen with the GL
    /// context in hand, inside the paint callback.
    pending: Arc<Mutex<Option<Shapes>>>,
    renderer: Arc<Mutex<Option<Result<Renderer, String>>>>,
    /// Page top left, relative to the view's top left, and screen points per
    /// page point.
    offset: egui::Vec2,
    zoom: f32,
    /// The zoom at which the page fits the view's width.
    fit: f32,
    fitted: bool,
    view_size: egui::Vec2,
    mode: Mode,
    /// When the view last moved, and where it was then.
    view_moved: Instant,
    last_view: (f32, egui::Vec2),
    /// The sharp drawing last asked for, with the part of the page it covers
    /// in page points from the top left; and the latest one received.
    sharp_asked: Option<(SharpRequest, Rect)>,
    sharp: Option<(u64, TextureHandle, Rect)>,
    sharp_took: Option<Duration>,
    next_sharp: u64,
    sweep: Option<Instant>,
    last_frame: Option<Instant>,
    frame_times: Vec<f32>,
    /// Microseconds the last measured GPU draw took, waiting for it to finish.
    draw_micros: Arc<AtomicU32>,
    draw_times: Vec<f32>,
    /// With `GPU_LINES_BENCH=1`; see bench.rs.
    bench: bool,
    frames: u64,
    /// The screenshot being taken: its index in `bench::SHOTS`, the frame its
    /// view was set up on, and whether the screenshot has been asked for.
    shot: Option<(usize, u64, bool)>,
}

impl Viewer {
    fn new(cc: &eframe::CreationContext<'_>, path: PathBuf, page: usize) -> Self {
        let (tx, from_loader) = mpsc::channel();
        let (requests, requests_rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || loader::run(path, page, tx, requests_rx, ctx));
        Viewer {
            from_loader,
            requests,
            loading: true,
            error: None,
            page_size: [1.0, 1.0],
            background: None,
            reference: None,
            report: vec!["Loading...".to_owned()],
            shape_count: 0,
            pending: Arc::new(Mutex::new(None)),
            renderer: Arc::new(Mutex::new(None)),
            offset: egui::Vec2::ZERO,
            zoom: 1.0,
            fit: 1.0,
            fitted: false,
            view_size: vec2(1.0, 1.0),
            mode: Mode::GpuOverPage,
            view_moved: Instant::now(),
            last_view: (0.0, egui::Vec2::ZERO),
            sharp_asked: None,
            sharp: None,
            sharp_took: None,
            next_sharp: 1,
            sweep: None,
            last_frame: None,
            frame_times: Vec::new(),
            draw_micros: Arc::new(AtomicU32::new(0)),
            draw_times: Vec::new(),
            bench: std::env::var_os("GPU_LINES_BENCH").is_some_and(|v| v == "1"),
            frames: 0,
            shot: None,
        }
    }

    fn receive(&mut self, ctx: &egui::Context) {
        let texture = |name: &str, size: [usize; 2], rgba: &[u8]| ctx.load_texture(name, ColorImage::from_rgba_unmultiplied(size, rgba), TextureOptions::LINEAR);
        while let Ok(message) = self.from_loader.try_recv() {
            match message {
                FromLoader::Loaded(Ok(loaded)) => {
                    self.loading = false;
                    self.background = Some(texture("page without stamps", loaded.background.0, &loaded.background.1));
                    self.reference = Some(texture("page drawn by pdfium", loaded.reference.0, &loaded.reference.1));
                    self.page_size = loaded.page_size;
                    self.shape_count = loaded.shapes.primitives.len();
                    self.report = loaded.report;
                    *self.pending.lock().unwrap() = Some(loaded.shapes);
                }
                FromLoader::Loaded(Err(e)) => {
                    self.loading = false;
                    self.error = Some(e);
                }
                FromLoader::Sharp { id, image, took } => {
                    let Some((asked, area)) = self.sharp_asked.filter(|(r, _)| r.id == id) else { continue };
                    match image {
                        Ok((size, rgba)) => {
                            self.sharp = Some((asked.id, texture("view drawn by pdfium", size, &rgba), area));
                            self.sharp_took = Some(took);
                        }
                        Err(e) => self.error = Some(format!("pdfium couldn't draw the view: {e}")),
                    }
                }
            }
        }
    }

    /// The part of the page in view as pdfium would draw it: the page's size
    /// in pixels at this zoom, the region of it in view, and that region in
    /// page points from the top left. `None` if no part of the page is in view.
    fn view_to_draw(&self, ppp: f32) -> Option<([u32; 2], [u32; 4], Rect)> {
        let scale = self.zoom * ppp;
        let full = [(self.page_size[0] * scale).round() as u32, (self.page_size[1] * scale).round() as u32];
        let left = (-self.offset.x * ppp).max(0.0).floor() as u32;
        let top = (-self.offset.y * ppp).max(0.0).floor() as u32;
        let right = (((self.view_size.x - self.offset.x) * ppp).ceil().max(0.0) as u32).min(full[0]);
        let bottom = (((self.view_size.y - self.offset.y) * ppp).ceil().max(0.0) as u32).min(full[1]);
        if right <= left || bottom <= top {
            return None;
        }
        let area = Rect::from_min_max(pos2(left as f32 / scale, top as f32 / scale), pos2(right as f32 / scale, bottom as f32 / scale));
        Some((full, [left, top, right - left, bottom - top], area))
    }

    /// Whether the sharp drawing shown is of exactly the view as it is now.
    fn sharp_is_current(&self, ppp: f32) -> bool {
        let (Some((shown, ..)), Some((asked, _)), Some((full, region, _))) = (&self.sharp, &self.sharp_asked, self.view_to_draw(ppp)) else {
            return false;
        };
        *shown == asked.id && asked.full == full && asked.region == region
    }

    /// Asks pdfium for the part of the page in view -- once the view has held
    /// still, unless `now` -- unless that's what it last asked for.
    fn ask_for_sharp(&mut self, ppp: f32, now: bool) {
        if (!now && self.view_moved.elapsed() < SHARP_AFTER) || self.background.is_none() {
            return;
        }
        let Some((full, region, area)) = self.view_to_draw(ppp) else { return };
        if self.sharp_asked.is_some_and(|(r, _)| r.full == full && r.region == region) {
            return;
        }
        let request = SharpRequest { id: self.next_sharp, full, region };
        self.next_sharp += 1;
        self.sharp_asked = Some((request, area));
        let _ = self.requests.send(request);
    }

    fn view(&mut self, ui: &mut egui::Ui) {
        let view = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(view, Sense::click_and_drag());
        self.fit = view.width() / self.page_size[0];
        self.view_size = view.size();
        if !self.fitted && self.background.is_some() {
            self.zoom = self.fit;
            self.offset = vec2(0.0, (view.height() - self.page_size[1] * self.zoom).max(0.0) / 2.0);
            self.fitted = true;
        }

        // Zoom around the pointer, pan by dragging, or sweep.
        let (scroll, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.hover_pos()));
        if let Some(started) = self.sweep {
            let t = started.elapsed().as_secs_f32();
            let target = self.fit * 2f32.powf(2.0 + 1.5 * (t * 0.8).sin());
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
        if (self.zoom, self.offset) != self.last_view {
            self.last_view = (self.zoom, self.offset);
            self.view_moved = Instant::now();
        }
        if self.mode.wants_sharp() {
            self.ask_for_sharp(ui.ctx().pixels_per_point(), false);
        }

        let painter = ui.painter_at(view);
        painter.rect_filled(view, 0.0, Color32::from_gray(60));
        let page = Rect::from_min_size(view.min + self.offset, vec2(self.page_size[0], self.page_size[1]) * self.zoom);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let whole_page = |texture: &Option<TextureHandle>| {
            if let Some(t) = texture {
                painter.image(t.id(), page, uv, Color32::WHITE);
            }
        };
        let sharp_view = |faded: bool| {
            whole_page(&self.reference);
            if let Some((_, t, area)) = &self.sharp {
                let on_screen = Rect::from_min_max(page.min + area.min.to_vec2() * self.zoom, page.min + area.max.to_vec2() * self.zoom);
                painter.image(t.id(), on_screen, uv, Color32::WHITE);
            }
            if faded {
                painter.rect_filled(page, 0.0, Color32::from_white_alpha(150));
            }
        };
        match self.mode {
            Mode::GpuOverPage => whole_page(&self.background),
            Mode::GpuOnly => {
                painter.rect_filled(page, 0.0, Color32::WHITE);
            }
            Mode::Pdfium => whole_page(&self.reference),
            Mode::PdfiumSharp => sharp_view(false),
            Mode::Compare => sharp_view(true),
        }

        if !matches!(self.mode, Mode::Pdfium | Mode::PdfiumSharp) {
            painter.add(egui::PaintCallback { rect: view, callback: Arc::new(self.paint_shapes()) });
        }
    }

    /// The paint callback that draws the shapes on the GPU at the view as it
    /// is, uploading them first if they've just arrived.
    fn paint_shapes(&self) -> egui_glow::CallbackFn {
        let (renderer, pending, draw_micros) = (Arc::clone(&self.renderer), Arc::clone(&self.pending), Arc::clone(&self.draw_micros));
        let (zoom, offset, height, measure) = (self.zoom, self.offset, self.page_size[1], self.sweep.is_some());
        egui_glow::CallbackFn::new(move |info, painter| {
            let gl: &glow::Context = painter.gl();
            let mut slot = renderer.lock().unwrap();
            let renderer = slot.get_or_insert_with(|| Renderer::new(gl));
            let Ok(renderer) = renderer else { return };
            if let Some(shapes) = pending.lock().unwrap().take() {
                renderer.upload(gl, &shapes);
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
        })
    }

    /// The line under the report: shapes, zoom, and what's being timed.
    fn status(&self, ppp: f32) -> String {
        let average = |times: &[f32]| if times.is_empty() { 0.0 } else { times.iter().sum::<f32>() / times.len() as f32 };
        let worst = |times: &[f32]| times.iter().copied().fold(0.0_f32, f32::max);
        let mut status = format!(
            "{} shapes on the GPU ({:.0} MB); zoom {:.0}% of fit width",
            self.shape_count,
            self.shape_count as f64 * std::mem::size_of::<Primitive>() as f64 / 1e6,
            self.zoom / self.fit.max(1e-6) * 100.0
        );
        if self.mode.wants_sharp() {
            match (self.sharp_is_current(ppp), self.sharp_took) {
                (true, Some(took)) => status += &format!("; pdfium drew this view in {:.0} ms", took.as_secs_f64() * 1000.0),
                _ => status += "; pdfium is drawing this view...",
            }
        }
        if self.sweep.is_some() {
            status += &format!(
                "; frames {:.1} ms average, {:.1} ms worst; GPU draw {:.2} ms average, {:.2} ms worst",
                average(&self.frame_times),
                worst(&self.frame_times),
                average(&self.draw_times),
                worst(&self.draw_times)
            );
        }
        status
    }
}

impl eframe::App for Viewer {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
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
                ui.selectable_value(&mut self.mode, Mode::GpuOverPage, "GPU over page");
                ui.selectable_value(&mut self.mode, Mode::GpuOnly, "GPU only");
                ui.selectable_value(&mut self.mode, Mode::Pdfium, "pdfium whole page");
                ui.selectable_value(&mut self.mode, Mode::PdfiumSharp, "pdfium sharp");
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
            if let Some(Err(e)) = &*self.renderer.lock().unwrap() {
                ui.colored_label(Color32::RED, e);
            }
            ui.label(self.status(ppp));
            self.view(ui);
        });

        if self.sweep.is_some() || self.loading || self.bench || self.shot.is_some() {
            ctx.request_repaint();
        } else if self.mode.wants_sharp() && !self.sharp_is_current(ppp) {
            ctx.request_repaint_after(SHARP_AFTER);
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
    // Clips are drawn through the stencil buffer.
    options.stencil_buffer = 8;
    eframe::run_native("GPU lines", options, Box::new(move |cc| Ok(Box::new(Viewer::new(cc, path, page)))))
}
