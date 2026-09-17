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
//! machine drawing OpenGL in software, or with `KINETIC_PDF_GPU=0`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Color32, Rect, Vec2};
use eframe::{egui_glow, glow};
use gpu_lines::{annotation_shapes, lopdf, page_shapes_unless, Mark, Renderer, Shapes, Upload, MOST_IMAGE_DENSITY, STOPPED};
pub(super) use gpu_lines::Uploaded;

use super::{App, Doc};
use crate::cache::Cache;
use crate::worker::{trace, Wanted};

/// Curves are flattened to within this many points.
const TOLERANCE: f32 = 0.05;

/// Seconds a page waits for its shapes before pdfium draws it after all,
/// counted from when the page was first wanted rather than from when the
/// reader got to it. Most pages are read in 20-40 ms; the heaviest drawing
/// sheet measured takes 266 ms, and waiting that long for it held the first
/// page of a drawing set off the screen, so pdfium draws meanwhile and the
/// shapes replace its drawing when they arrive.
pub(super) const SHAPES_WAIT: f64 = 0.15;

/// A page whose shapes would take more GPU memory than this -- one of large
/// photos, say -- isn't drawn whole on the GPU.
const WHOLE_PAGE_MOST: usize = 256 * 1024 * 1024;

/// GPU memory for shapes of pages away from the view, past which the farthest
/// are let go. On a machine without a graphics card of its own this is taken
/// from the computer's memory, and the driver keeps copies of its own.
const UPLOAD_BUDGET: usize = 256 * 1024 * 1024;

/// Pages either side of the view whose shapes always stay uploaded.
const KEEP_NEAR: usize = 1;

/// The most a page's shapes can come to and still be kept in the cache.
/// Bigger than this, reading them back off the disk and unsqueezing them takes
/// longer than reading the page again: a drawing sheet's shapes at fit width
/// are 18 MB and come back in 30 ms against 163 ms to read the page, but
/// zoomed right in they are 211 MB, which no longer pays.
const MOST_KEPT_SHAPES: usize = 64 * 1024 * 1024;

/// Bytes of shapes sent to the GPU a frame, so a heavy page goes up over a few
/// frames rather than holding one up.
const UPLOAD_PER_FRAME: usize = 32 * 1024 * 1024;

/// Who draws a page.
pub(super) enum PageDrawing {
    /// Not known yet: its shapes were wanted from this time, and `asked` once
    /// the reader -- which reads one page at a time -- has been given it.
    Reading { since: f64, asked: bool },
    /// pdfium draws all of it.
    Pdfium,
    /// The GPU draws the page's annotations, or with `whole` all of it.
    /// `uploaded` is `None` while its shapes are let go to save memory -- and
    /// meanwhile a page drawn whole is left to pdfium -- and `reading` once
    /// they've been asked for again. `density` is the pixels a point its
    /// images were kept at, so a deeper zoom can have the page read again.
    Gpu { whole: bool, uploaded: Option<Arc<Uploaded>>, reading: bool, density: f32 },
}

/// What a page's own shapes came to when it was read, which says what they
/// would come to at another zoom: the images grow with the square of the
/// density, the rest doesn't grow at all.
#[derive(Clone, Copy)]
pub(super) struct Sizes {
    density: f32,
    atlas: usize,
    other: usize,
}

impl Sizes {
    fn of(shapes: &Shapes, density: f32) -> Sizes {
        let atlas = shapes.atlas.pages.len() * gpu_lines::ATLAS_SIZE as usize * shapes.atlas.height() as usize * 4;
        Sizes { density, atlas, other: shapes.bytes().saturating_sub(atlas) }
    }

    /// What the page's shapes would come to at `density`.
    fn at(&self, density: f32) -> usize {
        let growth = (density / self.density.max(f32::EPSILON)).powi(2);
        self.other + (self.atlas as f32 * growth) as usize
    }
}

/// What reading a page came to.
enum Read {
    /// All of the page's shapes, with `whole`, or its annotations'. `sizes`
    /// is what the page's own shapes came to, whether they are drawn or were
    /// too big for the GPU.
    Shapes { shapes: Shapes, whole: bool, sizes: Option<Sizes> },
    Failed(String),
    /// The page was no longer wanted by the time its turn came, or its read
    /// stopped to let a page wanted more go first.
    Skipped,
}

use crate::model::THUMBNAIL_WIDTH;

/// A page's thumbnail, `THUMBNAIL_WIDTH` across and as tall as the page is.
pub(super) fn thumbnail_size(page: egui::Vec2) -> [u32; 2] {
    let height = (THUMBNAIL_WIDTH as f32 * page.y / page.x.max(1.0)).round();
    [THUMBNAIL_WIDTH, (height as u32).clamp(1, 4 * THUMBNAIL_WIDTH)]
}

/// Reads pages' thumbnails back from the page cache, off the thread that
/// draws: they're small, but a hundred of them is still a hundred reads.
pub(super) struct Thumbnails {
    requests: Sender<usize>,
    results: Receiver<(usize, [usize; 2], Vec<u8>)>,
}

impl Thumbnails {
    /// Reads the thumbnails kept for file `file`, as they're asked for.
    pub(super) fn spawn(cache: Option<Arc<Cache>>, file: u64, ctx: egui::Context) -> Option<Thumbnails> {
        let cache = cache?;
        let (requests, asked) = mpsc::channel::<usize>();
        let (found, results) = mpsc::channel();
        let run = move || {
            for page in asked {
                if let Some((size, rgba)) = cache.load(crate::cache::Key::thumbnail(file, page)) {
                    if found.send((page, size, rgba)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            }
        };
        std::thread::Builder::new().name("page thumbnails".into()).spawn(run).ok().map(|_| Thumbnails { requests, results })
    }

    /// Asks for page `page`'s thumbnail, if one was kept.
    pub(super) fn want(&self, page: usize) {
        let _ = self.requests.send(page);
    }
}

/// A page's shapes as the cache kept them: the flag saying whether the GPU
/// draws the whole page, then the shapes themselves. `None` if they were
/// written by another build, or don't read back.
fn restored(kept: &[u8], density: f32) -> Option<Read> {
    let (whole, shapes) = kept.split_first()?;
    let shapes = Shapes::from_bytes(shapes)?;
    let sizes = Some(Sizes::of(&shapes, density));
    Some(Read::Shapes { shapes, whole: *whole != 0, sizes })
}

/// Pixels a page point to keep a page's images at, for a page shown at
/// `scale` device pixels a point: the next power of two at or above it, up to
/// `MOST_IMAGE_DENSITY`. Powers of two, so zooming a little doesn't read the
/// page again. Zoomed out to where a sheet is an inch across, its photos are
/// kept at an eighth of a pixel a point -- sixty-fourth the memory of a pixel
/// a point -- since that is all the screen shows of them.
pub(super) fn image_density(scale: f32) -> f32 {
    IMAGE_DENSITIES.into_iter().find(|&step| step >= scale).unwrap_or(MOST_IMAGE_DENSITY)
}

/// The steps a page's images are kept at, coarsest first; past the last of them
/// comes `MOST_IMAGE_DENSITY`.
const IMAGE_DENSITIES: [f32; 6] = [0.125, 0.25, 0.5, 1.0, 2.0, 4.0];

/// Page `page`'s shapes: all of them, if the GPU can draw the page whole, or
/// else its annotations', with its images kept at `density` pixels a point.
/// With `stop`, reading gives up as soon as it is set.
fn read_page<'d>(doc: &'d lopdf::Document, page: usize, density: f32, stop: Option<&'d AtomicBool>) -> Read {
    let number = page as u32 + 1;
    let started = Instant::now();
    let milliseconds = || started.elapsed().as_secs_f64() * 1000.0;
    let mut sizes = None;
    let why_not = match page_shapes_unless(doc, number, TOLERANCE, density, stop) {
        Err(e) if e == STOPPED => {
            trace(format_args!("gpu: stopped reading page {page} after {:.0} ms, for a page wanted more", milliseconds()));
            return Read::Skipped;
        }
        Ok(shapes) if shapes.not_drawn.is_empty() && shapes.bytes() <= WHOLE_PAGE_MOST => {
            trace(format_args!("gpu: read page {page} whole at {density} px a point in {:.0} ms, {} MB", milliseconds(), shapes.bytes() >> 20));
            let sizes = Sizes::of(&shapes, density);
            return Read::Shapes { shapes, whole: true, sizes: Some(sizes) };
        }
        Ok(shapes) if !shapes.not_drawn.is_empty() => {
            let listed: Vec<String> = shapes.not_drawn.iter().map(|(what, n)| format!("{what} ({n})")).collect();
            listed.join(", ")
        }
        Ok(shapes) => {
            // Too big for the GPU at this zoom. What it came to says whether
            // it would fit at another, so the page isn't read again to find
            // out (`too_big_for_the_gpu`).
            sizes = Some(Sizes::of(&shapes, density));
            format!("{} MB", shapes.bytes() >> 20)
        }
        Err(e) => e,
    };
    let annotations = annotation_shapes(doc, number, TOLERANCE, density);
    trace(format_args!("gpu: page {page} isn't drawn whole ({why_not}); read its annotations, in {:.0} ms in all", milliseconds()));
    annotations.map_or_else(Read::Failed, |shapes| Read::Shapes { shapes, whole: false, sizes })
}

/// A page's shapes on their way to the GPU.
pub(super) struct Uploading {
    page: usize,
    whole: bool,
    density: f32,
    /// Read only to be thumbnailed, so its shapes go again once it is.
    ahead_only: bool,
    upload: Upload,
}

/// The thread reading a document's pages into shapes, a page at a time: each
/// asked for at a density (`image_density`), and answered with the one it was
/// read at.
pub(super) struct Reader {
    /// A page, the density to keep its images at, and whether it's wanted only
    /// for its thumbnail -- which is the one kind of read for a page that
    /// isn't in view.
    requests: Sender<(usize, f32, bool)>,
    /// The page, the density it was read at, whether it was read only to be
    /// thumbnailed, and what came of it.
    results: Receiver<(usize, f32, bool, Read)>,
    /// Set while a page wanted more than the one being read waits for the
    /// reader, so that read stops and lets it go first (`wait_for_shapes`).
    /// Just after opening a drawing set, the reader spent 5 s on a sheet of two
    /// million objects that had been ahead of the first page, and every sheet
    /// looked at meanwhile waited for it.
    give_way: Arc<AtomicBool>,
}

impl Reader {
    /// Reads the pages of `path`, opened as document `generation`, that are
    /// asked for while `wanted` still wants them. Pages read before are taken
    /// from `cache`, and the slow ones are kept there as they're read.
    pub(super) fn spawn(
        path: PathBuf,
        generation: u64,
        wanted: Arc<Mutex<Wanted>>,
        ctx: egui::Context,
        cache: Option<Arc<Cache>>,
    ) -> Reader {
        let (requests, asked) = mpsc::channel::<(usize, f32, bool)>();
        let (found, results) = mpsc::channel();
        let give_way = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&give_way);
        let run = move || {
            let started = Instant::now();
            // The file's bytes fingerprint it for the cache, the same way the
            // worker does. Parsing them is left until a page is wanted that
            // the cache hasn't got, which on a second open may be none.
            let mut bytes = Some(std::fs::read(&path).map_err(|e| e.to_string()));
            let file = bytes.as_ref().and_then(|bytes| bytes.as_ref().ok()).map(|bytes| crate::cache::fingerprint(bytes));
            trace(format_args!("gpu: read the file in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0));
            let mut doc: Option<Result<lopdf::Document, String>> = None;
            for (page, density, for_thumbnail) in asked {
                let still_wanted = for_thumbnail || wanted.lock().map(|w| w.rank(generation, page).is_some()).unwrap_or(true);
                if !still_wanted {
                    if found.send((page, density, for_thumbnail, Read::Skipped)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                    continue;
                }
                let started = Instant::now();
                let kept = cache.as_ref().zip(file).and_then(|(cache, file)| cache.load_shapes(file, page, density)).and_then(|kept| restored(&kept, density));
                let read = match kept {
                    Some(read) => {
                        trace(format_args!("gpu: page {page}'s shapes came from the cache in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0));
                        read
                    }
                    None => {
                        let doc = doc.get_or_insert_with(|| {
                            let started = Instant::now();
                            // The file's bytes go once it is parsed: lopdf
                            // keeps what it needs, and they are 81 MB of an
                            // 85 MB drawing set.
                            let read = bytes.take().unwrap_or_else(|| std::fs::read(&path).map_err(|e| e.to_string()));
                            let parsed = read.and_then(|bytes| lopdf::Document::load_mem(&bytes).map_err(|e| e.to_string()));
                            trace(format_args!("gpu: parsed the document in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0));
                            parsed
                        });
                        // Timed from here, so the one-off parse above doesn't
                        // count as the page's own reading.
                        let reading = Instant::now();
                        let read = match doc {
                            Ok(doc) => read_page(doc, page, density, Some(stop.as_ref())),
                            Err(e) => Read::Failed(e.clone()),
                        };
                        // Keeping a page that read quickly isn't worth the
                        // disk it takes; one with anything left undrawn isn't
                        // kept at all, since what comes back would be missing
                        // it without saying so.
                        // A page read only for its thumbnail keeps the
                        // thumbnail, which is 50 KB; its shapes, read small
                        // and wanted once, aren't worth the room.
                        let slow = !for_thumbnail && reading.elapsed().as_millis() >= u128::from(crate::cache::SLOW_MS);
                        if let (Some(cache), Some(file), Read::Shapes { shapes, whole, .. }) = (cache.as_ref(), file, &read) {
                            if slow && shapes.not_drawn.is_empty() && shapes.bytes() <= MOST_KEPT_SHAPES {
                                let mut bytes = vec![u8::from(*whole)];
                                bytes.extend_from_slice(&shapes.to_bytes());
                                trace(format_args!("gpu: keeping page {page}'s shapes, {} MB", bytes.len() >> 20));
                                cache.store_shapes(file, page, density, bytes);
                            }
                        }
                        read
                    }
                };
                if found.send((page, density, for_thumbnail, read)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        };
        if let Err(e) = std::thread::Builder::new().name("page shapes".into()).spawn(run) {
            trace(format_args!("gpu: could not start reading pages: {e}"));
        }
        Reader { requests, results, give_way }
    }

    /// Asks for page `page`'s shapes at `density`, or with `for_thumbnail`
    /// just enough of them for its thumbnail. Says whether the reader is
    /// still there to ask. Any call to give way is over: it was for a page
    /// that is asked for now, and a thumbnail is only asked for when nothing
    /// is waiting.
    fn ask(&self, page: usize, density: f32, for_thumbnail: bool) -> bool {
        self.give_way.store(false, Ordering::Relaxed);
        self.requests.send((page, density, for_thumbnail)).is_ok()
    }
}

/// The GPU's side: the context, and the shaders every page shares.
pub(super) struct Gpu {
    gl: Arc<glow::Context>,
    renderer: Arc<Renderer>,
    /// Which GPU draws, for the trace and the scroll benchmark: on a laptop
    /// with two, it may not be the one expected.
    pub(super) name: String,
}

/// The GPU and driver the window draws with, whether or not pages are drawn
/// on it, so a benchmark says what it ran on either way.
pub(super) fn describe(cc: &eframe::CreationContext<'_>) -> String {
    cc.gl.as_ref().map_or_else(|| "no OpenGL context".to_owned(), |gl| Renderer::describe(gl))
}

impl Gpu {
    /// The GPU, unless pages aren't to be drawn on it here.
    pub(super) fn new(cc: &eframe::CreationContext<'_>) -> Option<Gpu> {
        let gl = Arc::clone(cc.gl.as_ref()?);
        let name = Renderer::describe(&gl);
        trace(format_args!("gpu: drawing with {name}"));
        if std::env::var_os("KINETIC_PDF_GPU").is_some_and(|v| v == "0") {
            return None;
        }
        if Renderer::is_software(&gl) {
            trace(format_args!("gpu: OpenGL draws in software here, so pdfium draws everything"));
            return None;
        }
        match Renderer::new(&gl) {
            Ok(renderer) => Some(Gpu { gl, renderer: Arc::new(renderer), name }),
            Err(e) => {
                trace(format_args!("gpu: {e}, so pdfium draws everything"));
                None
            }
        }
    }

    /// Frees the shapes uploaded for a document's pages, and any on their way.
    pub(super) fn release(&self, drawing: HashMap<usize, PageDrawing>, uploading: Option<Uploading>) {
        for state in drawing.into_values() {
            if let PageDrawing::Gpu { uploaded: Some(uploaded), .. } = state {
                self.free(uploaded);
            }
        }
        if let Some(uploading) = uploading {
            uploading.upload.destroy(&self.gl);
        }
    }

    /// After a save that changed how `pages` are drawn, reads the saved file
    /// afresh. Pages on the GPU keep their drawing until the new one is up
    /// (see `wait_for_shapes`).
    pub(super) fn reread(&self, doc: &mut Doc, pages: &[usize], wanted: &Arc<Mutex<Wanted>>, ctx: &egui::Context, cache: Option<Arc<Cache>>) {
        doc.reader = Some(Reader::spawn(doc.path.clone(), doc.generation, Arc::clone(wanted), ctx.clone(), cache));
        // Whatever the old reader had yet to answer is asked for again, and
        // what the GPU had nothing to draw of is worth another look: the save
        // may have given it something.
        doc.drawing.retain(|_, state| !matches!(state, PageDrawing::Reading { .. }));
        for page in pages {
            doc.left_to_pdfium.remove(page);
        }
        for state in doc.drawing.values_mut() {
            if let PageDrawing::Gpu { reading, .. } = state {
                *reading = false;
            }
        }
        if let Some(uploading) = doc.uploading.take_if(|u| pages.contains(&u.page)) {
            uploading.upload.destroy(&self.gl);
        }
    }

    fn free(&self, uploaded: Arc<Uploaded>) {
        // A paint callback holds the shapes only for the frame it draws, so
        // between frames nothing else does.
        if let Ok(uploaded) = Arc::try_unwrap(uploaded) {
            uploaded.destroy(&self.gl);
        }
    }

    /// Sends a frame's worth of the page on its way to the GPU; once it's all
    /// there, takes the next page read, starting to upload shapes the GPU can
    /// draw and leaving the rest to pdfium. Says whether an upload is under way.
    fn take_shapes(&self, doc: &mut Doc, ctx: &egui::Context, cache: Option<&Cache>) -> bool {
        if let Some(mut uploading) = doc.uploading.take() {
            if !uploading.upload.step(&self.gl, UPLOAD_PER_FRAME) {
                doc.uploading = Some(uploading);
                return true;
            }
            let Uploading { page, whole, density, ahead_only, upload } = uploading;
            let what = if whole { "the whole of page" } else { "the annotations of page" };
            trace(format_args!("gpu: {what} {page} on the GPU, {} MB at {density} px a point", upload.bytes() >> 20));
            let uploaded = upload.finish();
            // A page the GPU draws whole has its thumbnail taken now, while
            // its shapes are there: it shows the page while they're being read
            // again after being let go, and on the next open before they are.
            if whole && !doc.thumbnails.contains_key(&page) {
                self.take_thumbnail(doc, page, &uploaded, ctx, cache);
            }
            // A page read only for its thumbnail lets its shapes go again at
            // once: they were read small, and the page is read afresh at the
            // density it needs whenever it is looked at.
            let kept = if ahead_only {
                uploaded.destroy(&self.gl);
                None
            } else {
                Some(Arc::new(uploaded))
            };
            let state = PageDrawing::Gpu { whole, uploaded: kept, reading: false, density };
            doc.redraw.remove(&page);
            if let Some(PageDrawing::Gpu { uploaded: Some(old), .. }) = doc.drawing.insert(page, state) {
                self.free(old);
            }
        }
        for uploaded in std::mem::take(&mut doc.releasing) {
            self.free(uploaded);
        }
        self.finish_handing_over(doc);
        let Some(reader) = &doc.reader else { return false };
        while let Ok((page, density, ahead_only, read)) = reader.results.try_recv() {
            // What the page's own shapes came to, so it needn't be read again
            // at a zoom they wouldn't fit at.
            if let Read::Shapes { sizes: Some(sizes), .. } = &read {
                doc.shape_sizes.insert(page, *sizes);
            }
            if doc.reading.is_some_and(|(on, _)| on == page) {
                doc.reading = None;
            }
            let state = match read {
                Read::Skipped => {
                    // A read for a thumbnail stopped to let a page in view go
                    // first, so it is tried again once nothing else is.
                    if ahead_only {
                        doc.thumbs_ahead.remove(&page);
                    }
                    match doc.drawing.get_mut(&page) {
                        Some(PageDrawing::Gpu { reading, .. }) => *reading = false,
                        Some(PageDrawing::Reading { .. }) => drop(doc.drawing.remove(&page)),
                        _ => {}
                    }
                    continue;
                }
                Read::Failed(error) => {
                    trace(format_args!("gpu: page {page} couldn't be read, so pdfium draws it: {error}"));
                    doc.left_to_pdfium.insert(page);
                    PageDrawing::Pdfium
                }
                Read::Shapes { shapes, whole: false, .. } if shapes.primitives.is_empty() => {
                    doc.left_to_pdfium.insert(page);
                    PageDrawing::Pdfium
                }
                Read::Shapes { shapes, whole: false, .. } if !shapes.not_drawn.is_empty() => {
                    let listed: Vec<String> = shapes.not_drawn.iter().map(|(what, n)| format!("{what} ({n})")).collect();
                    trace(format_args!("gpu: pdfium draws page {page}'s annotations, having {}", listed.join(", ")));
                    doc.left_to_pdfium.insert(page);
                    PageDrawing::Pdfium
                }
                // The page counts as still being read until it's all there.
                Read::Shapes { shapes, whole, .. } => match self.renderer.begin_upload(&self.gl, shapes) {
                    Ok(upload) => {
                        doc.uploading = Some(Uploading { page, whole, density, ahead_only, upload });
                        return true;
                    }
                    Err(e) => {
                        trace(format_args!("gpu: page {page}'s shapes couldn't be uploaded, so pdfium draws it: {e}"));
                        doc.left_to_pdfium.insert(page);
                        PageDrawing::Pdfium
                    }
                },
            };
            if let Some(PageDrawing::Gpu { uploaded: Some(old), .. }) = doc.drawing.insert(page, state) {
                self.free(old);
            }
        }
        false
    }

    /// Draws `page` small from the shapes in hand, keeps it to show the page
    /// with while nothing better is there, and puts it in the page cache.
    fn take_thumbnail(&self, doc: &mut Doc, page: usize, uploaded: &Uploaded, ctx: &egui::Context, cache: Option<&Cache>) {
        let Some(&points) = doc.sizes.get(page) else { return };
        let size = thumbnail_size(points);
        let started = Instant::now();
        let Some(rgba) = self.renderer.draw_to_image(&self.gl, uploaded, size, [points.x, points.y]) else {
            trace(format_args!("gpu: page {page}'s thumbnail couldn't be drawn"));
            return;
        };
        let size = [size[0] as usize, size[1] as usize];
        let texture = crate::worker::make_texture(ctx, format!("page-{page}-thumbnail"), size, &rgba);
        trace(format_args!("gpu: took page {page}'s thumbnail in {:.1} ms", started.elapsed().as_secs_f64() * 1000.0));
        doc.thumbnails.insert(page, super::Thumbnail { handle: texture, used: f64::MAX });
        if let (Some(cache), Some(file)) = (cache, Some(doc.file)) {
            cache.store(crate::cache::Key::thumbnail(file, page), size, rgba);
        }
    }

    /// Paints what the GPU draws of `page`, drawn at `rect`, where it's in
    /// `view` -- the whole page, or its annotations over an image of the page
    /// drawn without them -- with `marks`, highlights and the like on screen
    /// in their colours, over it all.
    pub(super) fn paint_page(&self, painter: &egui::Painter, doc: &Doc, page: usize, rect: Rect, view: Rect, marks: &[(Rect, Color32)]) {
        let Some(PageDrawing::Gpu { uploaded: Some(uploaded), .. }) = doc.drawing.get(&page) else { return };
        let visible = rect.intersect(view);
        if !draws_over(doc, page) || !visible.is_positive() {
            return;
        }
        let size = doc.sizes[page];
        let marks: Vec<Mark> = marks
            .iter()
            .map(|&(area, colour)| Mark { rect: page_points(rect, size, area), colour: [colour.r(), colour.g(), colour.b()].map(|c| f32::from(c) / 255.0) })
            .collect();
        let (renderer, uploaded) = (Arc::clone(&self.renderer), Arc::clone(uploaded));
        let callback = egui_glow::CallbackFn::new(move |info, painter| {
            let ppp = info.pixels_per_point;
            let viewport = info.viewport_in_pixels();
            // Pixels a page point, and page points to pixels in the viewport,
            // whose origin is its top left.
            let scale = rect.width() / size.x * ppp;
            let left = rect.min.x * ppp - viewport.left_px as f32;
            let top = rect.min.y * ppp - viewport.top_px as f32;
            let page_to_pixels = [scale, 0.0, 0.0, -scale, left, top + size.y * scale];
            renderer.paint(painter.gl(), &uploaded, &marks, page_to_pixels, [viewport.width_px as f32, viewport.height_px as f32], scale);
        });
        painter.add(egui::PaintCallback { rect: visible, callback: Arc::new(callback) });
    }

    /// Lets go of the shapes of pages pdfium has taken over (too big for the
    /// GPU at this zoom) once pdfium has actually drawn them, so the page is
    /// never left without a drawing in between.
    pub(super) fn finish_handing_over(&self, doc: &mut Doc) {
        let drawn: Vec<usize> = doc
            .handing_over
            .iter()
            .copied()
            .filter(|page| doc.textures.get(page).is_some_and(|texture| texture.complete && texture.annotations))
            .collect();
        for page in drawn {
            if let Some(PageDrawing::Gpu { uploaded, .. }) = doc.drawing.get_mut(&page) {
                if let Some(uploaded) = uploaded.take() {
                    trace(format_args!("gpu: pdfium has drawn page {page}; let its shapes go, {} MB", uploaded.bytes() >> 20));
                    self.free(uploaded);
                }
            }
            doc.drawing.insert(page, PageDrawing::Pdfium);
            doc.handing_over.remove(&page);
        }
    }

    /// Lets go of the shapes of pages away from the view, farthest first,
    /// while those beyond its neighbours take more than `UPLOAD_BUDGET` --
    /// and of any page shown from its thumbnail, wherever it is, since
    /// nothing draws from them.
    pub(super) fn keep_uploads_near(&self, doc: &mut Doc, first: usize, last: usize, from_thumbnails: &HashSet<usize>) {
        for &page in from_thumbnails {
            if let Some(PageDrawing::Gpu { uploaded, .. }) = doc.drawing.get_mut(&page) {
                if let Some(uploaded) = uploaded.take() {
                    trace(format_args!("gpu: page {page} is shown from its thumbnail; let its shapes go, {} MB", uploaded.bytes() >> 20));
                    self.free(uploaded);
                }
            }
        }
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
    /// Takes the pages read for the open document since the last frame, a
    /// frame's worth of uploading at a time.
    pub(super) fn receive_shapes(&mut self, ctx: &egui::Context) {
        let cache = self.cache.clone();
        if let (Some(gpu), Some(doc)) = (&self.gpu, self.doc.as_mut()) {
            if gpu.take_shapes(doc, ctx, cache.as_deref()) {
                ctx.request_repaint();
            }
            // Thumbnails read back from the cache for pages asked about.
            let now = ctx.input(|i| i.time);
            let read: Vec<(usize, [usize; 2], Vec<u8>)> = doc.thumbs.as_ref().map(|t| t.results.try_iter().collect()).unwrap_or_default();
            for (page, size, rgba) in read {
                if !doc.thumbnails.contains_key(&page) {
                    let texture = crate::worker::make_texture(ctx, format!("page-{page}-thumbnail"), size, &rgba);
                    doc.thumbnails.insert(page, super::Thumbnail { handle: texture, used: now });
                }
            }
        }
    }
}

/// Pixels a point images are kept at for a page read only to be thumbnailed:
/// the thumbnail is 320 pixels across, so this is as much as it shows.
const THUMBNAIL_DENSITY: f32 = 0.125;

/// Has one page that hasn't got a thumbnail read, so it has one before anyone
/// scrolls to it -- nearest the view first, and only while nothing else is
/// being read. Says whether it asked for one.
///
/// The page's shapes go up as any other page's, its thumbnail is taken from
/// them, and they are let go again with the rest of the pages away from the
/// view; read small, they cost a few megabytes. A page whose thumbnail can't
/// be had this way -- one pdfium has to draw -- is left, and gets one when
/// pdfium next draws it (`pool::keep_thumbnail`).
pub(super) fn read_a_thumbnail_ahead(doc: &mut Doc, near: usize, now: f64) -> bool {
    let Some(reader) = &doc.reader else { return false };
    if doc.uploading.is_some() || doc.drawing.values().any(|state| matches!(state, PageDrawing::Reading { asked: true, .. } | PageDrawing::Gpu { reading: true, .. })) {
        return false;
    }
    let pages = doc.sizes.len();
    let wanted = |page: &usize| {
        *page < pages && !doc.thumbnails.contains_key(page) && !doc.thumbs_ahead.contains(page) && !matches!(doc.drawing.get(page), Some(PageDrawing::Pdfium))
    };
    // Outwards from the view: after it, then before it, as loading ahead goes.
    let Some(page) = (0..pages).flat_map(|step| [near + step, near.wrapping_sub(step)]).find(wanted) else { return false };
    doc.thumbs_ahead.insert(page);
    if !reader.ask(page, THUMBNAIL_DENSITY, true) {
        return false;
    }
    doc.drawing.entry(page).or_insert(PageDrawing::Reading { since: now, asked: true });
    doc.reading = Some((page, true));
    trace(format_args!("gpu: reading page {page} ahead for its thumbnail"));
    true
}

/// Whether `page`, drawn at `scale` device pixels a point, is no wider than
/// its thumbnail -- which is then all the screen can show of it, so the page
/// needs neither its shapes nor an image of it.
pub(super) fn thumbnail_is_enough(doc: &Doc, page: usize, scale: f32) -> bool {
    let width = doc.sizes.get(page).map_or(0.0, |size| size.x * scale);
    width <= THUMBNAIL_WIDTH as f32 && doc.thumbnails.contains_key(&page)
}

/// Whether the GPU draws over `page` as it's shown now: the whole page, or its
/// annotations over an image of the page drawn without them.
pub(super) fn draws_over(doc: &Doc, page: usize) -> bool {
    match doc.drawing.get(&page) {
        Some(PageDrawing::Gpu { whole, uploaded: Some(_), .. }) => *whole || doc.textures.get(&page).is_some_and(|t| !t.annotations),
        _ => false,
    }
}

/// A rectangle on screen as left, bottom, right and top in the points of the
/// page drawn at `page`, `size` points in size.
fn page_points(page: Rect, size: Vec2, area: Rect) -> [f32; 4] {
    let scale = page.width() / size.x;
    let x = |screen: f32| (screen - page.min.x) / scale;
    let y = |screen: f32| size.y - (screen - page.min.y) / scale;
    [x(area.min.x), y(area.max.y), x(area.max.x), y(area.min.y)]
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

/// The shapes on the GPU now: how many pages, and the bytes they hold. The
/// graphics driver keeps copies of its own, so the process holds more.
pub(super) fn uploaded(doc: &Doc) -> (usize, usize) {
    let pages = doc.drawing.values().filter_map(|state| match state {
        PageDrawing::Gpu { uploaded: Some(uploaded), .. } => Some(uploaded.bytes()),
        _ => None,
    });
    pages.fold((0, 0), |(count, bytes), page| (count + 1, bytes + page))
}

/// The pages the GPU draws whole now, which pdfium needn't draw. A page being
/// handed to pdfium isn't one of them: it is still drawn from its shapes, but
/// pdfium is drawing it as well, and takes over when it's done.
pub(super) fn pages_drawn_whole(doc: &Doc) -> HashSet<usize> {
    doc.drawing.keys().copied().filter(|&page| drawn_whole(doc, page) && !doc.handing_over.contains(&page)).collect()
}


/// Asks for `page`'s shapes if they're needed, and says whether drawing the
/// page should wait for them. Only one page is read and uploaded at a time,
/// the pages in view first, so only one page's shapes are ever in hand; the
/// rest wait their turn. A page whose drawing a save made out of date is read
/// again while its old drawing stays up.
/// The densest images `page`'s shapes can hold and still fit the GPU, from what
/// they came to when it was last read -- `density` if they fit at it, a coarser
/// step if not, and `None` if even the coarsest is too much. Reading a drawing
/// sheet's photos at the deepest zoom comes to hundreds of megabytes and takes
/// 600 ms to find out, so once a page has been read at any zoom it isn't read
/// again at a size that wouldn't fit.
fn density_that_fits(doc: &Doc, page: usize, density: f32) -> Option<f32> {
    let Some(sizes) = doc.shape_sizes.get(&page) else { return Some(density) };
    let steps = IMAGE_DENSITIES.into_iter().chain([MOST_IMAGE_DENSITY]);
    steps.rev().find(|&step| step <= density && sizes.at(step) <= WHOLE_PAGE_MOST)
}

pub(super) fn wait_for_shapes(doc: &mut Doc, page: usize, now: f64, density: f32, moving: bool, wanted_more: &[usize]) -> bool {
    let Some(reader) = &doc.reader else { return false };
    let fits = density_that_fits(doc, page, density);
    // Zoomed in past what the page's images can be held at, pdfium takes it
    // over, since only pdfium can draw them this sharp. The shapes keep drawing
    // it meanwhile, with their images as sharp as they fit: every line and
    // letter is exactly right, and photos come good when pdfium arrives, which
    // on a sheet like this takes a second or more.
    let mut over_to_pdfium = false;
    let density = match fits {
        Some(fits) => {
            over_to_pdfium = fits < density;
            if over_to_pdfium {
                if doc.handing_over.insert(page) {
                    trace(format_args!("gpu: page {page} is too big for the GPU at {density} px a point; drawn at {fits} while pdfium takes it over"));
                }
            } else {
                // Zoomed back out before pdfium got there, it keeps its shapes.
                doc.handing_over.remove(&page);
            }
            fits
        }
        None => {
            let has_shapes = matches!(doc.drawing.get(&page), Some(PageDrawing::Gpu { uploaded: Some(_), .. }));
            if has_shapes {
                if doc.handing_over.insert(page) {
                    trace(format_args!("gpu: page {page} is too big for the GPU at {density} px a point; pdfium takes it over"));
                }
            } else {
                doc.drawing.insert(page, PageDrawing::Pdfium);
            }
            return false;
        }
    };
    // A page pdfium has drawn stays drawn by it: it is sharper than the shapes
    // could be at this zoom, which is why it was handed over. Zoomed back out to
    // where the shapes fit again, it is worth another look.
    if matches!(doc.drawing.get(&page), Some(PageDrawing::Pdfium)) {
        let worth_another_look =
            !over_to_pdfium && doc.shape_sizes.contains_key(&page) && !doc.left_to_pdfium.contains(&page);
        if !worth_another_look {
            doc.handing_over.remove(&page);
            return false;
        }
        doc.drawing.remove(&page);
    }
    let busy = doc.drawing.values().any(|state| matches!(state, PageDrawing::Reading { asked: true, .. } | PageDrawing::Gpu { reading: true, .. }));
    // Zoomed in past what the page's images were kept at, it is read again at
    // the density the zoom shows, its old shapes staying up meanwhile. Not
    // while the zoom is still moving, though: the page would be read again at
    // every step it passed through, each read outliving the zoom that asked for
    // it. Whatever is up keeps drawing until the view lands.
    let coarse = !moving && matches!(doc.drawing.get(&page), Some(PageDrawing::Gpu { density: at, .. }) if *at < density);
    let stale = doc.redraw.contains(&page) || coarse;
    // The page being read stops for this one if this one is wanted more --
    // `wanted_more` are the pages that come before it -- or the other is read
    // only for its thumbnail. A sheet of two million objects takes seconds to
    // read, and one that was ahead of the view when it was asked for, or
    // being thumbnailed, held up every page looked at meanwhile.
    let needs_reader = match doc.drawing.get(&page) {
        None => true,
        Some(PageDrawing::Reading { asked, .. }) => !asked,
        Some(PageDrawing::Gpu { uploaded, reading, .. }) => (uploaded.is_none() || stale) && !reading,
        Some(PageDrawing::Pdfium) => false,
    };
    let outranked = doc.reading.is_some_and(|(on, thumbnail)| on != page && (thumbnail || !wanted_more.contains(&on)));
    if busy && needs_reader && outranked {
        reader.give_way.store(true, Ordering::Relaxed);
    }
    match doc.drawing.get_mut(&page) {
        // A page whose turn hasn't come is still on the clock: its wait runs
        // from now, so the pages in view behind a heavy one aren't held up for
        // as long as that one takes to read.
        None if busy => {
            doc.drawing.insert(page, PageDrawing::Reading { since: now, asked: false });
            true
        }
        None => {
            let asked = reader.ask(page, density, false);
            if asked {
                doc.reading = Some((page, false));
                doc.drawing.insert(page, PageDrawing::Reading { since: now, asked });
            }
            asked
        }
        Some(PageDrawing::Reading { since, asked }) => {
            if !*asked && !busy {
                *asked = reader.ask(page, density, false);
                if *asked {
                    doc.reading = Some((page, false));
                }
            }
            now - *since < SHAPES_WAIT
        }
        Some(PageDrawing::Gpu { uploaded, reading, .. }) if uploaded.is_none() || stale => {
            if !*reading && !busy {
                *reading = reader.ask(page, density, false);
                if *reading {
                    doc.reading = Some((page, false));
                }
            }
            false
        }
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};

    #[test]
    fn screen_rectangles_become_page_points_from_the_bottom_left() {
        // A 100 x 50 point page drawn twice its size at 10, 20 on screen.
        let page = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 100.0));
        let area = Rect::from_min_max(pos2(30.0, 40.0), pos2(50.0, 60.0));
        assert_eq!(page_points(page, vec2(100.0, 50.0), area), [10.0, 30.0, 20.0, 40.0]);
    }
}
