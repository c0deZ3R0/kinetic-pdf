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

/// Pdfium's drawings of the whole page are this wide, for the background and
/// for comparing.
const PAGE_PIXELS: f32 = 4096.0;

/// How long the view holds still before pdfium is asked to draw it sharp.
const SHARP_AFTER: Duration = Duration::from_millis(120);

/// How long the benchmark runs its zoom sweep.
const BENCH_FOR: Duration = Duration::from_secs(10);

/// Screenshots the benchmark takes, centred on `GPU_LINES_BENCH_AT` ("x,y" in
/// page points, the page's middle if unset), at a zoom relative to fit width,
/// into `GPU_LINES_BENCH_DIR` (the current folder if unset).
const SHOTS: [(Mode, f32, &str); 4] = [
    (Mode::GpuOverPage, 1.0, "gpu-fit"),
    (Mode::PdfiumSharp, 1.0, "pdfium-fit"),
    (Mode::GpuOverPage, 6.0, "gpu-600"),
    (Mode::PdfiumSharp, 6.0, "pdfium-600"),
];

struct Loaded {
    page_size: [f32; 2],
    background: ([usize; 2], Vec<u8>),
    reference: ([usize; 2], Vec<u8>),
    primitives: Vec<Primitive>,
    report: Vec<String>,
}

/// A part of the page for pdfium to draw: the page drawn `full` pixels in
/// size, and the `region` of it (x, y, width, height).
#[derive(Clone, Copy, PartialEq)]
struct SharpRequest {
    id: u64,
    full: [u32; 2],
    region: [u32; 4],
}

enum FromLoader {
    Loaded(Result<Loaded, String>),
    Sharp { id: u64, image: Result<([usize; 2], Vec<u8>), String>, took: Duration },
}

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
    pending: Arc<Mutex<Option<Vec<Primitive>>>>,
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
    /// With `GPU_LINES_BENCH=1`: sweep as soon as the shapes are uploaded,
    /// print the times to stderr after `BENCH_FOR`, take `SHOTS`, and close.
    bench: bool,
    frames: u64,
    /// The screenshot being taken: its index in `SHOTS`, the frame its view
    /// was set up on, and whether the screenshot has been asked for.
    shot: Option<(usize, u64, bool)>,
}

/// Makes the drawing copy, reads the page's shapes and has pdfium draw the
/// page with and without its annotations. Returns the copy's bytes too, for
/// drawing sharp views from later.
fn prepare(pdfium: &Pdfium, path: &std::path::Path, page_number: usize) -> Result<(Loaded, Vec<u8>), String> {
    let mut report = Vec::new();
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;

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
    drop(doc);

    // Flatten the annotations into the page, so their appearances sit where
    // pdfium puts them; what was there before stays first.
    let started = Instant::now();
    let found = {
        let flat = pdfium.load_pdf_from_byte_slice(&drawing, None).map_err(|e| e.to_string())?;
        let before = {
            let mut page = flat.pages().get(index).map_err(|e| e.to_string())?;
            let before = page.objects().len();
            page.flatten().map_err(|e| e.to_string())?;
            before
        };
        let page = flat.pages().get(index).map_err(|e| e.to_string())?;
        let objects = page.objects();
        extract((before..objects.len()).filter_map(|i| objects.get(i).ok()), 0.05)
    };
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
    Ok((Loaded { page_size, background, reference, primitives: found.primitives, report }, drawing))
}

/// The loader thread: prepares the page, then keeps it open and draws the
/// views asked for, only ever the latest.
fn loader(path: PathBuf, page_number: usize, tx: mpsc::Sender<FromLoader>, requests: mpsc::Receiver<SharpRequest>, ctx: egui::Context) {
    let send = |message: FromLoader| {
        let _ = tx.send(message);
        ctx.request_repaint();
    };
    let pdfium = match worker::bind() {
        Ok(pdfium) => pdfium,
        Err(e) => return send(FromLoader::Loaded(Err(e))),
    };
    let drawing = match prepare(&pdfium, &path, page_number) {
        Ok((loaded, drawing)) => {
            send(FromLoader::Loaded(Ok(loaded)));
            drawing
        }
        Err(e) => return send(FromLoader::Loaded(Err(e))),
    };
    let Ok(doc) = pdfium.load_pdf_from_byte_slice(&drawing, None) else { return };
    let Ok(mut page) = doc.pages().get(page_number.saturating_sub(1) as PdfPageIndex) else { return };
    // As the app draws it: its own highlights drawn separately.
    annots::strip_loaded_page(&mut page);

    while let Ok(mut request) = requests.recv() {
        while let Ok(newer) = requests.try_recv() {
            request = newer;
        }
        let started = Instant::now();
        let image = annots::render_region_in_steps(&page, request.full, request.region, |_| true)
            .and_then(|drawn| drawn.ok_or_else(|| "the drawing was stopped".to_owned()));
        send(FromLoader::Sharp { id: request.id, image, took: started.elapsed() });
    }
}

impl Viewer {
    fn new(cc: &eframe::CreationContext<'_>, path: PathBuf, page: usize) -> Self {
        let (tx, from_loader) = mpsc::channel();
        let (requests, requests_rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || loader(path, page, tx, requests_rx, ctx));
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
        while let Ok(message) = self.from_loader.try_recv() {
            match message {
                FromLoader::Loaded(Ok(loaded)) => {
                    self.loading = false;
                    let texture = |name: &str, (size, rgba): &([usize; 2], Vec<u8>)| {
                        ctx.load_texture(name, ColorImage::from_rgba_unmultiplied(*size, rgba), TextureOptions::LINEAR)
                    };
                    self.background = Some(texture("page without stamps", &loaded.background));
                    self.reference = Some(texture("page drawn by pdfium", &loaded.reference));
                    self.page_size = loaded.page_size;
                    self.shape_count = loaded.primitives.len();
                    self.report = loaded.report;
                    *self.pending.lock().unwrap() = Some(loaded.primitives);
                }
                FromLoader::Loaded(Err(e)) => {
                    self.loading = false;
                    self.error = Some(e);
                }
                FromLoader::Sharp { id, image, took } => {
                    let Some((asked, area)) = self.sharp_asked.filter(|(r, _)| r.id == id) else { continue };
                    match image {
                        Ok((size, rgba)) => {
                            let texture = ctx.load_texture("view drawn by pdfium", ColorImage::from_rgba_unmultiplied(size, &rgba), TextureOptions::LINEAR);
                            self.sharp = Some((asked.id, texture, area));
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

    /// In benchmark mode: starts sweeping once the shapes are on the GPU, and
    /// after a while prints what it measured and starts the screenshots.
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
            None if uploaded && self.shape_count > 0 => {
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
                eprintln!("{} shapes on the GPU ({:.1} MB)", self.shape_count, self.shape_count as f64 * 48.0 / 1e6);
                eprintln!(
                    "sweeping zoom between 141% and 1131% of fit width, vsync {}",
                    if std::env::var_os("GPU_LINES_VSYNC").is_some_and(|v| v == "0") { "off" } else { "on" }
                );
                eprintln!("frame to frame: {}", summary(&self.frame_times));
                eprintln!("GPU draw, waited for: {}", summary(&self.draw_times));
                self.sweep = None;
                self.shot = Some((0, self.frames + 1, false));
            }
            _ => {}
        }
    }

    /// Takes the benchmark's screenshots one at a time: sets up the view, waits
    /// until it's drawn (for pdfium, the sharp drawing of it), asks for a
    /// screenshot and saves it when it arrives. Closes after the last.
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
        let ppp = ctx.pixels_per_point();
        if mode.wants_sharp() && !asked {
            self.ask_for_sharp(ppp, true);
        }
        let drawn = self.frames >= set_at + 3 && (!mode.wants_sharp() || self.sharp_is_current(ppp));
        if !asked && drawn {
            eprintln!(
                "{name}: zoom {:.0}% of fit width{}",
                self.zoom / self.fit.max(1e-6) * 100.0,
                match (mode.wants_sharp(), self.sharp_took, self.sharp_asked) {
                    (true, Some(took), Some((request, _))) => format!(
                        "; pdfium drew the {} x {} px view of the page at {} x {} px in {:.0} ms",
                        request.region[2],
                        request.region[3],
                        request.full[0],
                        request.full[1],
                        took.as_secs_f64() * 1000.0
                    ),
                    _ => String::new(),
                }
            );
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
        let ppp = ui.ctx().pixels_per_point();
        if self.mode.wants_sharp() {
            self.ask_for_sharp(ppp, false);
        }

        let painter = ui.painter_at(view);
        painter.rect_filled(view, 0.0, Color32::from_gray(60));
        let page = Rect::from_min_size(view.min + self.offset, vec2(self.page_size[0], self.page_size[1]) * self.zoom);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let on_screen = |area: Rect| Rect::from_min_max(page.min + area.min.to_vec2() * self.zoom, page.min + area.max.to_vec2() * self.zoom);
        let paint_pdfium = |faded: bool| {
            if let Some(t) = &self.reference {
                painter.image(t.id(), page, uv, Color32::WHITE);
            }
            if let Some((_, t, area)) = &self.sharp {
                painter.image(t.id(), on_screen(*area), uv, Color32::WHITE);
            }
            if faded {
                painter.rect_filled(page, 0.0, Color32::from_white_alpha(150));
            }
        };
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
            Mode::PdfiumSharp => paint_pdfium(false),
            Mode::Compare => paint_pdfium(true),
        }

        if !matches!(self.mode, Mode::Pdfium | Mode::PdfiumSharp) {
            let (renderer, pending, draw_micros) = (Arc::clone(&self.renderer), Arc::clone(&self.pending), Arc::clone(&self.draw_micros));
            let (zoom, offset, height, measure) = (self.zoom, self.offset, self.page_size[1], self.sweep.is_some());
            let callback = egui_glow::CallbackFn::new(move |info, painter| {
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
            let renderer_error = match &*self.renderer.lock().unwrap() {
                Some(Err(e)) => Some(e.clone()),
                _ => None,
            };
            if let Some(e) = renderer_error {
                ui.colored_label(Color32::RED, e);
            }
            let average = |times: &[f32]| if times.is_empty() { 0.0 } else { times.iter().sum::<f32>() / times.len() as f32 };
            let worst = |times: &[f32]| times.iter().copied().fold(0.0_f32, f32::max);
            let mut status = format!(
                "{} shapes on the GPU ({:.0} MB); zoom {:.0}% of fit width",
                self.shape_count,
                self.shape_count as f64 * 48.0 / 1e6,
                self.zoom / self.fit.max(1e-6) * 100.0
            );
            if self.mode.wants_sharp() {
                match (self.sharp_is_current(ui.ctx().pixels_per_point()), self.sharp_took) {
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
            ui.label(status);
            self.view(ui);
        });

        if self.sweep.is_some() || self.loading || self.bench || self.shot.is_some() {
            ctx.request_repaint();
        } else if self.mode.wants_sharp() && !self.sharp_is_current(ctx.pixels_per_point()) {
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
    eframe::run_native("GPU lines", options, Box::new(move |cc| Ok(Box::new(Viewer::new(cc, path, page)))))
}
