//! Annotations drawn on the GPU (crates/gpu-lines), over pages pdfium draws
//! without them.
//!
//! pdfium draws a page's annotations into its image, which for a Bluebeam
//! overlay of stamps takes a second or more at every zoom. Instead a thread
//! reads each page's annotations into shapes as the page is wanted, and a page
//! whose annotations the GPU can draw entirely is drawn by pdfium without them,
//! its shapes painted over the image: sharp at any zoom, with nothing to draw
//! again. A page with anything the GPU can't draw yet keeps pdfium drawing its
//! annotations -- as does every page on a machine drawing OpenGL in software,
//! or with `PDF_ANNOTATE_GPU=0`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Rect};
use eframe::{egui_glow, glow};
use gpu_lines::{annotation_shapes, lopdf, Renderer, Shapes, Uploaded};

use super::{App, Doc};
use crate::worker::{trace, Wanted};

/// Curves in annotations are flattened to within this many points.
const TOLERANCE: f32 = 0.05;

/// Seconds a page waits for its shapes before pdfium draws it with its
/// annotations after all.
pub(super) const SHAPES_WAIT: f64 = 0.5;

/// GPU memory for shapes of pages away from the view, past which the farthest
/// are let go.
const UPLOAD_BUDGET: usize = 512 * 1024 * 1024;

/// Pages either side of the view whose shapes always stay uploaded.
const KEEP_NEAR: usize = 2;

/// Who draws a page's annotations.
pub(super) enum PageAnnotations {
    /// Not known yet: its shapes were asked for at this time.
    Reading(f64),
    /// pdfium: the page has none, or some the GPU can't draw yet.
    Pdfium,
    /// The GPU. `uploaded` is `None` while its shapes are let go to save
    /// memory, and `reading` once they've been asked for again.
    Gpu { uploaded: Option<Arc<Uploaded>>, reading: bool },
}

/// What reading a page's annotations came to.
enum Read {
    Shapes(Shapes),
    Failed(String),
    /// The page was no longer wanted by the time its turn came.
    Skipped,
}

/// The thread reading a document's annotations into shapes, a page at a time.
pub(super) struct Reader {
    requests: Sender<usize>,
    results: Receiver<(usize, Read)>,
}

impl Reader {
    /// Reads the annotations of `path`, opened as document `generation`, for
    /// the pages asked for while `wanted` still wants them.
    pub(super) fn spawn(path: PathBuf, generation: u64, wanted: Arc<Mutex<Wanted>>, ctx: egui::Context) -> Reader {
        let (requests, asked) = mpsc::channel::<usize>();
        let (found, results) = mpsc::channel();
        let run = move || {
            let started = Instant::now();
            let doc = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| lopdf::Document::load_mem(&bytes).map_err(|e| e.to_string()));
            trace(format_args!("gpu: read the document for its annotations in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0));
            for page in asked {
                let still_wanted = wanted.lock().map(|w| w.rank(generation, page).is_some()).unwrap_or(true);
                let read = match &doc {
                    _ if !still_wanted => Read::Skipped,
                    Ok(doc) => {
                        let started = Instant::now();
                        let shapes = annotation_shapes(doc, page as u32 + 1, TOLERANCE);
                        trace(format_args!("gpu: read page {page}'s annotations in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0));
                        shapes.map_or_else(Read::Failed, Read::Shapes)
                    }
                    Err(e) => Read::Failed(e.clone()),
                };
                if found.send((page, read)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        };
        if let Err(e) = std::thread::Builder::new().name("annotation shapes".into()).spawn(run) {
            trace(format_args!("gpu: could not start reading annotations: {e}"));
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
    /// The GPU, unless annotations aren't to be drawn on it here.
    pub(super) fn new(cc: &eframe::CreationContext<'_>) -> Option<Gpu> {
        if std::env::var_os("PDF_ANNOTATE_GPU").is_some_and(|v| v == "0") {
            return None;
        }
        let gl = Arc::clone(cc.gl.as_ref()?);
        if Renderer::is_software(&gl) {
            trace(format_args!("gpu: OpenGL draws in software here, so pdfium draws annotations"));
            return None;
        }
        match Renderer::new(&gl) {
            Ok(renderer) => Some(Gpu { gl, renderer: Arc::new(renderer) }),
            Err(e) => {
                trace(format_args!("gpu: {e}, so pdfium draws annotations"));
                None
            }
        }
    }

    /// Frees the shapes uploaded for a document's pages.
    pub(super) fn release(&self, annotations: HashMap<usize, PageAnnotations>) {
        for state in annotations.into_values() {
            if let PageAnnotations::Gpu { uploaded: Some(uploaded), .. } = state {
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

    /// Takes the shapes read since last time: uploads those the GPU can draw
    /// entirely, and leaves the rest to pdfium.
    fn take_shapes(&self, doc: &mut Doc) {
        let Some(reader) = &doc.reader else { return };
        while let Ok((page, read)) = reader.results.try_recv() {
            let state = match read {
                Read::Skipped => {
                    match doc.annotations.get_mut(&page) {
                        Some(PageAnnotations::Gpu { reading, .. }) => *reading = false,
                        Some(PageAnnotations::Reading(_)) => drop(doc.annotations.remove(&page)),
                        _ => {}
                    }
                    continue;
                }
                Read::Failed(error) => {
                    trace(format_args!("gpu: page {page}'s annotations couldn't be read, so pdfium draws them: {error}"));
                    PageAnnotations::Pdfium
                }
                Read::Shapes(shapes) if shapes.primitives.is_empty() => PageAnnotations::Pdfium,
                Read::Shapes(shapes) if !shapes.not_drawn.is_empty() => {
                    let listed: Vec<String> = shapes.not_drawn.iter().map(|(what, n)| format!("{what} ({n})")).collect();
                    trace(format_args!("gpu: pdfium draws page {page}'s annotations, having {}", listed.join(", ")));
                    PageAnnotations::Pdfium
                }
                Read::Shapes(shapes) => match self.renderer.upload(&self.gl, &shapes) {
                    Ok(uploaded) => {
                        trace(format_args!("gpu: page {page}'s annotations on the GPU, {} MB", uploaded.bytes() >> 20));
                        PageAnnotations::Gpu { uploaded: Some(Arc::new(uploaded)), reading: false }
                    }
                    Err(e) => {
                        trace(format_args!("gpu: page {page}'s annotations couldn't be uploaded, so pdfium draws them: {e}"));
                        PageAnnotations::Pdfium
                    }
                },
            };
            if let Some(PageAnnotations::Gpu { uploaded: Some(old), .. }) = doc.annotations.insert(page, state) {
                self.free(old);
            }
        }
    }

    /// Paints `page`'s annotations over its image, drawn at `rect`, where it's
    /// in `view` -- if the GPU draws them and the image was drawn without them.
    pub(super) fn paint_page(&self, painter: &egui::Painter, doc: &Doc, page: usize, rect: Rect, view: Rect) {
        let Some(PageAnnotations::Gpu { uploaded: Some(uploaded), .. }) = doc.annotations.get(&page) else { return };
        let visible = rect.intersect(view);
        if !doc.textures.get(&page).is_some_and(|t| !t.annotations) || !visible.is_positive() {
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
            .annotations
            .iter()
            .filter_map(|(&page, state)| match state {
                PageAnnotations::Gpu { uploaded: Some(uploaded), .. } if !near.contains(&page) => Some((distance(page), page, uploaded.bytes())),
                _ => None,
            })
            .collect();
        let mut total: usize = far.iter().map(|&(.., bytes)| bytes).sum();
        far.sort_unstable();
        while total > UPLOAD_BUDGET {
            let Some((_, page, bytes)) = far.pop() else { break };
            if let Some(PageAnnotations::Gpu { uploaded, .. }) = doc.annotations.get_mut(&page) {
                if let Some(uploaded) = uploaded.take() {
                    self.free(uploaded);
                    trace(format_args!("gpu: let go of page {page}'s annotations, {} MB", bytes >> 20));
                }
            }
            total -= bytes;
        }
    }
}

impl App {
    /// Takes the shapes read for the open document since the last frame.
    pub(super) fn receive_shapes(&mut self) {
        if let (Some(gpu), Some(doc)) = (&self.gpu, self.doc.as_mut()) {
            gpu.take_shapes(doc);
        }
    }
}

/// Whether pdfium draws `page`'s annotations into its images: unless the GPU
/// draws them.
pub(super) fn annotations_drawn(doc: &Doc, page: usize) -> bool {
    !matches!(doc.annotations.get(&page), Some(PageAnnotations::Gpu { .. }))
}

/// The pages whose annotations the GPU draws.
pub(super) fn pages_without_annotations(doc: &Doc) -> HashSet<usize> {
    doc.annotations.iter().filter(|(_, state)| matches!(state, PageAnnotations::Gpu { .. })).map(|(&page, _)| page).collect()
}

/// Asks for `page`'s shapes if they're needed, and says whether drawing the
/// page should wait for them.
pub(super) fn wait_for_shapes(doc: &mut Doc, page: usize, now: f64) -> bool {
    let Some(reader) = &doc.reader else { return false };
    match doc.annotations.get_mut(&page) {
        None => {
            let asked = reader.requests.send(page).is_ok();
            if asked {
                doc.annotations.insert(page, PageAnnotations::Reading(now));
            }
            asked
        }
        Some(PageAnnotations::Reading(since)) => now - *since < SHAPES_WAIT,
        Some(PageAnnotations::Gpu { uploaded: None, reading }) => {
            if !*reading {
                *reading = reader.requests.send(page).is_ok();
            }
            false
        }
        Some(_) => false,
    }
}
