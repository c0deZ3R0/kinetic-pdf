//! Pages drawn on the GPU (crates/gpu-lines): whole, or just their
//! annotations over pdfium's drawing of the page without them.
//!
//! pdfium draws on the CPU, and a dense drawing or a Bluebeam overlay of stamps
//! takes it a second or more at every zoom. Instead a thread reads each page
//! into shapes as the page is wanted. A page the GPU can draw entirely, within
//! `WHOLE_PAGE_MOST`, is drawn whole: pdfium draws nothing of it, and it's sharp
//! at any zoom with nothing to draw again. Otherwise, if the GPU can draw all
//! the page's annotations, pdfium draws the page without them and they're
//! painted over it. Anything else is left to pdfium, as is every page on a
//! machine drawing OpenGL in software, or with `PDF_ANNOTATE_GPU=0`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Rect};
use eframe::{egui_glow, glow};
use gpu_lines::{annotation_shapes, lopdf, page_shapes, Renderer, Shapes, Uploaded};

use super::{App, Doc};
use crate::worker::{trace, Wanted};

/// Curves are flattened to within this many points.
const TOLERANCE: f32 = 0.05;

/// Seconds a page waits for its shapes before pdfium draws it after all.
pub(super) const SHAPES_WAIT: f64 = 0.5;

/// A page whose shapes would take more GPU memory than this -- one of large
/// photos, say -- isn't drawn whole on the GPU.
const WHOLE_PAGE_MOST: usize = 256 * 1024 * 1024;

/// GPU memory for shapes of pages away from the view, past which the farthest
/// are let go.
const UPLOAD_BUDGET: usize = 512 * 1024 * 1024;

/// Pages either side of the view whose shapes always stay uploaded.
const KEEP_NEAR: usize = 2;

/// Who draws a page.
pub(super) enum PageDrawing {
    /// Not known yet: its shapes were asked for at this time.
    Reading(f64),
    /// pdfium draws all of it.
    Pdfium,
    /// The GPU draws the page's annotations, or with `whole` all of it.
    /// `uploaded` is `None` while its shapes are let go to save memory -- and
    /// meanwhile a page drawn whole is left to pdfium -- and `reading` once
    /// they've been asked for again.
    Gpu { whole: bool, uploaded: Option<Arc<Uploaded>>, reading: bool },
}

/// What reading a page came to.
enum Read {
    /// All of the page's shapes, with `whole`, or its annotations'.
    Shapes { shapes: Shapes, whole: bool },
    Failed(String),
    /// The page was no longer wanted by the time its turn came.
    Skipped,
}

/// Page `page`'s shapes: all of them, if the GPU can draw the page whole, or
/// else its annotations'.
fn read_page(doc: &lopdf::Document, page: usize) -> Read {
    let number = page as u32 + 1;
    let started = Instant::now();
    let milliseconds = || started.elapsed().as_secs_f64() * 1000.0;
    let why_not = match page_shapes(doc, number, TOLERANCE) {
        Ok(shapes) if shapes.not_drawn.is_empty() && shapes.bytes() <= WHOLE_PAGE_MOST => {
            trace(format_args!("gpu: read page {page} whole in {:.0} ms", milliseconds()));
            return Read::Shapes { shapes, whole: true };
        }
        Ok(shapes) if !shapes.not_drawn.is_empty() => {
            let listed: Vec<String> = shapes.not_drawn.iter().map(|(what, n)| format!("{what} ({n})")).collect();
            listed.join(", ")
        }
        Ok(shapes) => format!("{} MB", shapes.bytes() >> 20),
        Err(e) => e,
    };
    let annotations = annotation_shapes(doc, number, TOLERANCE);
    trace(format_args!("gpu: page {page} isn't drawn whole ({why_not}); read its annotations, in {:.0} ms in all", milliseconds()));
    annotations.map_or_else(Read::Failed, |shapes| Read::Shapes { shapes, whole: false })
}

/// The thread reading a document's pages into shapes, a page at a time.
pub(super) struct Reader {
    requests: Sender<usize>,
    results: Receiver<(usize, Read)>,
}

impl Reader {
    /// Reads the pages of `path`, opened as document `generation`, that are
    /// asked for while `wanted` still wants them.
    pub(super) fn spawn(path: PathBuf, generation: u64, wanted: Arc<Mutex<Wanted>>, ctx: egui::Context) -> Reader {
        let (requests, asked) = mpsc::channel::<usize>();
        let (found, results) = mpsc::channel();
        let run = move || {
            let started = Instant::now();
            let doc = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| lopdf::Document::load_mem(&bytes).map_err(|e| e.to_string()));
            trace(format_args!("gpu: read the document in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0));
            for page in asked {
                let still_wanted = wanted.lock().map(|w| w.rank(generation, page).is_some()).unwrap_or(true);
                let read = match &doc {
                    _ if !still_wanted => Read::Skipped,
                    Ok(doc) => read_page(doc, page),
                    Err(e) => Read::Failed(e.clone()),
                };
                if found.send((page, read)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        };
        if let Err(e) = std::thread::Builder::new().name("page shapes".into()).spawn(run) {
            trace(format_args!("gpu: could not start reading pages: {e}"));
        }
        Reader { requests, results }
    }
}

/// The GPU's side: the context, and the shaders every page shares.
pub(super) struct Gpu {
    gl: Arc<glow::Context>,
    renderer: Arc<Renderer>,
}

impl Gpu {
    /// The GPU, unless pages aren't to be drawn on it here.
    pub(super) fn new(cc: &eframe::CreationContext<'_>) -> Option<Gpu> {
        if std::env::var_os("PDF_ANNOTATE_GPU").is_some_and(|v| v == "0") {
            return None;
        }
        let gl = Arc::clone(cc.gl.as_ref()?);
        if Renderer::is_software(&gl) {
            trace(format_args!("gpu: OpenGL draws in software here, so pdfium draws everything"));
            return None;
        }
        match Renderer::new(&gl) {
            Ok(renderer) => Some(Gpu { gl, renderer: Arc::new(renderer) }),
            Err(e) => {
                trace(format_args!("gpu: {e}, so pdfium draws everything"));
                None
            }
        }
    }

    /// Frees the shapes uploaded for a document's pages.
    pub(super) fn release(&self, drawing: HashMap<usize, PageDrawing>) {
        for state in drawing.into_values() {
            if let PageDrawing::Gpu { uploaded: Some(uploaded), .. } = state {
                self.free(uploaded);
            }
        }
    }

    fn free(&self, uploaded: Arc<Uploaded>) {
        // A paint callback holds the shapes only for the frame it draws, so
        // between frames nothing else does.
        if let Ok(uploaded) = Arc::try_unwrap(uploaded) {
            uploaded.destroy(&self.gl);
        }
    }

    /// Takes the pages read since last time: uploads the shapes the GPU can
    /// draw, and leaves the rest to pdfium.
    fn take_shapes(&self, doc: &mut Doc) {
        let Some(reader) = &doc.reader else { return };
        while let Ok((page, read)) = reader.results.try_recv() {
            let state = match read {
                Read::Skipped => {
                    match doc.drawing.get_mut(&page) {
                        Some(PageDrawing::Gpu { reading, .. }) => *reading = false,
                        Some(PageDrawing::Reading(_)) => drop(doc.drawing.remove(&page)),
                        _ => {}
                    }
                    continue;
                }
                Read::Failed(error) => {
                    trace(format_args!("gpu: page {page} couldn't be read, so pdfium draws it: {error}"));
                    PageDrawing::Pdfium
                }
                Read::Shapes { shapes, whole: false } if shapes.primitives.is_empty() => PageDrawing::Pdfium,
                Read::Shapes { shapes, whole: false } if !shapes.not_drawn.is_empty() => {
                    let listed: Vec<String> = shapes.not_drawn.iter().map(|(what, n)| format!("{what} ({n})")).collect();
                    trace(format_args!("gpu: pdfium draws page {page}'s annotations, having {}", listed.join(", ")));
                    PageDrawing::Pdfium
                }
                Read::Shapes { shapes, whole } => match self.renderer.upload(&self.gl, &shapes) {
                    Ok(uploaded) => {
                        let what = if whole { "the whole of page" } else { "the annotations of page" };
                        trace(format_args!("gpu: {what} {page} on the GPU, {} MB", uploaded.bytes() >> 20));
                        PageDrawing::Gpu { whole, uploaded: Some(Arc::new(uploaded)), reading: false }
                    }
                    Err(e) => {
                        trace(format_args!("gpu: page {page}'s shapes couldn't be uploaded, so pdfium draws it: {e}"));
                        PageDrawing::Pdfium
                    }
                },
            };
            if let Some(PageDrawing::Gpu { uploaded: Some(old), .. }) = doc.drawing.insert(page, state) {
                self.free(old);
            }
        }
    }

    /// Paints what the GPU draws of `page`, drawn at `rect`, where it's in
    /// `view`: the whole page, or its annotations over an image of the page
    /// drawn without them.
    pub(super) fn paint_page(&self, painter: &egui::Painter, doc: &Doc, page: usize, rect: Rect, view: Rect) {
        let Some(PageDrawing::Gpu { whole, uploaded: Some(uploaded), .. }) = doc.drawing.get(&page) else { return };
        let visible = rect.intersect(view);
        let over_bare_image = doc.textures.get(&page).is_some_and(|t| !t.annotations);
        if !(*whole || over_bare_image) || !visible.is_positive() {
            return;
        }
        let (renderer, uploaded, size) = (Arc::clone(&self.renderer), Arc::clone(uploaded), doc.sizes[page]);
        let callback = egui_glow::CallbackFn::new(move |info, painter| {
            let ppp = info.pixels_per_point;
            let viewport = info.viewport_in_pixels();
            // Pixels a page point, and page points to pixels in the viewport,
            // whose origin is its top left.
            let scale = rect.width() / size.x * ppp;
            let left = rect.min.x * ppp - viewport.left_px as f32;
            let top = rect.min.y * ppp - viewport.top_px as f32;
            let page_to_pixels = [scale, 0.0, 0.0, -scale, left, top + size.y * scale];
            renderer.paint(painter.gl(), &uploaded, page_to_pixels, [viewport.width_px as f32, viewport.height_px as f32], scale);
        });
        painter.add(egui::PaintCallback { rect: visible, callback: Arc::new(callback) });
    }

    /// Lets go of the shapes of pages away from the view, farthest first,
    /// while those beyond its neighbours take more than `UPLOAD_BUDGET`.
    pub(super) fn keep_uploads_near(&self, doc: &mut Doc, first: usize, last: usize) {
        let near = first.saturating_sub(KEEP_NEAR)..=last + KEEP_NEAR;
        let distance = |page: usize| if page < first { first - page } else { page.saturating_sub(last) };
        let mut far: Vec<(usize, usize, usize)> = doc
            .drawing
            .iter()
            .filter_map(|(&page, state)| match state {
                PageDrawing::Gpu { uploaded: Some(uploaded), .. } if !near.contains(&page) => Some((distance(page), page, uploaded.bytes())),
                _ => None,
            })
            .collect();
        let mut total: usize = far.iter().map(|&(.., bytes)| bytes).sum();
        far.sort_unstable();
        while total > UPLOAD_BUDGET {
            let Some((_, page, bytes)) = far.pop() else { break };
            if let Some(PageDrawing::Gpu { uploaded, .. }) = doc.drawing.get_mut(&page) {
                if let Some(uploaded) = uploaded.take() {
                    self.free(uploaded);
                    trace(format_args!("gpu: let go of page {page}'s shapes, {} MB", bytes >> 20));
                }
            }
            total -= bytes;
        }
    }
}

impl App {
    /// Takes the pages read for the open document since the last frame.
    pub(super) fn receive_shapes(&mut self) {
        if let (Some(gpu), Some(doc)) = (&self.gpu, self.doc.as_mut()) {
            gpu.take_shapes(doc);
        }
    }
}

/// Whether pdfium draws `page`'s annotations into its images: unless the GPU
/// draws them over an image of the page without.
pub(super) fn annotations_drawn(doc: &Doc, page: usize) -> bool {
    !matches!(doc.drawing.get(&page), Some(PageDrawing::Gpu { whole: false, .. }))
}

/// Whether the GPU draws the whole of `page` now, so pdfium needn't.
pub(super) fn drawn_whole(doc: &Doc, page: usize) -> bool {
    matches!(doc.drawing.get(&page), Some(PageDrawing::Gpu { whole: true, uploaded: Some(_), .. }))
}

/// The pages whose annotations the GPU draws over pdfium's drawing of them.
pub(super) fn pages_without_annotations(doc: &Doc) -> HashSet<usize> {
    doc.drawing.keys().copied().filter(|&page| !annotations_drawn(doc, page)).collect()
}

/// The pages the GPU draws whole now.
pub(super) fn pages_drawn_whole(doc: &Doc) -> HashSet<usize> {
    doc.drawing.keys().copied().filter(|&page| drawn_whole(doc, page)).collect()
}

/// Asks for `page`'s shapes if they're needed, and says whether drawing the
/// page should wait for them.
pub(super) fn wait_for_shapes(doc: &mut Doc, page: usize, now: f64) -> bool {
    let Some(reader) = &doc.reader else { return false };
    match doc.drawing.get_mut(&page) {
        None => {
            let asked = reader.requests.send(page).is_ok();
            if asked {
                doc.drawing.insert(page, PageDrawing::Reading(now));
            }
            asked
        }
        Some(PageDrawing::Reading(since)) => now - *since < SHAPES_WAIT,
        Some(PageDrawing::Gpu { uploaded: None, reading, .. }) => {
            if !*reading {
                *reading = reader.requests.send(page).is_ok();
            }
            false
        }
        Some(_) => false,
    }
}
