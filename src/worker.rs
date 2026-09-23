//! The pdfium thread.
//!
//! pdfium is not thread-safe -- pdfium-render serialises every call behind one
//! global lock -- so a pool of render threads would only queue up behind each
//! other. Instead one thread owns the library and the open document, and the
//! UI talks to it over channels. The UI thread never waits on it.
//!
//! Pages are drawn in other processes when they can be (pool.rs): a thread in
//! front of this one sends page renders to the helpers and everything else
//! here. This thread draws pages itself only if no helper could start.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use pdfium_render::prelude::*;

use crate::annots;
use crate::cache::{self, Cache, Key};
use crate::helper::Target;
use crate::pool::{self, Helpers};
use crate::model::{self, Markup, Measurements, PageGeometry, PageNotes, Reply, Request, SearchHit, TextChar, Tile};
use crate::selection;

/// A search stops collecting after this many matches; a one-letter query in
/// a long document would otherwise produce more than anyone could step through.
pub const MAX_SEARCH_HITS: usize = 5000;

/// Memory for page text kept after it is first extracted. Enough for the
/// whole text of most documents; beyond it, pages are extracted each time.
const TEXT_CACHE_BYTES: usize = 256 * 1024 * 1024;

/// Pages kept loaded after use; see `Loaded::page`.
const OPEN_PAGES: usize = 6;

/// Memory allowed for pages kept loaded. pdfium keeps the images it decoded to
/// draw a page until the page is closed: a few MB for a text page, but 150-400
/// MB for a large drawing, so on a drawing set this limit is reached long
/// before `OPEN_PAGES` is.
const OPEN_PAGE_BYTES: usize = 512 * 1024 * 1024;

/// A page kept loaded, and the memory it holds.
struct OpenPage<'a> {
    index: usize,
    page: PdfPage<'a>,
    /// Measured: the process's private memory taken by loading the page and by
    /// drawing it. Other threads allocate too, so this can overstate it, which
    /// only closes pages sooner.
    bytes: usize,
}

/// How often a page that is slow to draw sends what it has drawn so far.
const PARTIAL_EVERY: Duration = Duration::from_millis(250);

/// What the UI wants drawn. Shared rather than sent, so the worker always sees
/// the latest: work for a page scrolled past is dropped instead of done for
/// nobody, and the page the view stopped on goes first.
#[derive(Default)]
pub struct Wanted {
    pub generation: u64,
    /// Most wanted first: the pages in view from the middle out, then the
    /// pages being loaded ahead.
    pub pages: Vec<usize>,
    /// The view is moving too fast to read, or a zoom is settling, so
    /// background work waits rather than holding up the page the view stops
    /// on.
    pub moving: bool,
    /// Every page's drawing scale at the current zoom, for drawing pages
    /// ahead into the cache. Replaced only when the zoom or layout changes.
    pub render_scales: Arc<Vec<f32>>,
    /// Pages whose annotations the app draws itself, so pdfium draws them
    /// without. Replaced only when it changes.
    pub without_annotations: Arc<HashSet<usize>>,
    /// Pages the app draws whole itself, so pdfium needn't draw them ahead.
    /// Replaced only when it changes.
    pub drawn_whole: Arc<HashSet<usize>>,
    /// Whether the lines of pages being read are indexed for snapping, which
    /// a measurement tool wants and nothing else does.
    pub snapping: bool,
    /// Whether the helpers leave the rest of the document undrawn, as they do
    /// while the app draws pages itself: pages it draws need nothing from
    /// them, and which those are is only known as each is read.
    pub skip_drawing_ahead: bool,
}

impl Wanted {
    /// Whether pdfium draws `page`'s annotations.
    pub fn draws_annotations(&self, page: usize) -> bool {
        !self.without_annotations.contains(&page)
    }

    /// Where `page` comes in the order, if it is wanted at all.
    pub fn rank(&self, generation: u64, page: usize) -> Option<usize> {
        if self.generation != generation {
            return None;
        }
        self.pages.iter().position(|&p| p == page)
    }
}

struct Loaded<'a> {
    generation: u64,
    path: PathBuf,
    /// The fingerprint of `bytes`, which keys the page cache.
    file: u64,
    /// The file as last read or written; saves start from these bytes.
    bytes: Vec<u8>,
    /// Pages kept loaded, least recently used first. Declared before `doc` so
    /// they are closed before it: a page must never outlive its document.
    open_pages: Vec<OpenPage<'a>>,
    /// The copy used for display, with highlights stripped page by page as
    /// they are first rendered.
    doc: PdfDocument<'a>,
    stripped: HashSet<usize>,
    /// Whether each page's highlights have been read and sent to the UI. A
    /// page is always read before it is stripped for display, or its
    /// highlights would be gone.
    scanned: Vec<bool>,
    unscanned: usize,
    /// How far the background read of highlights has got.
    scan_next: usize,
    /// Page text already extracted; see `text`.
    text_cache: HashMap<usize, Arc<Vec<TextChar>>>,
    text_cache_bytes: usize,
    /// Whether pages loaded to read their text stay open for drawing. Not
    /// while the app draws pages itself: loading a dense drawing holds
    /// hundreds of MB that nothing would use.
    keep_open: bool,
}

impl<'a> Loaded<'a> {
    /// A page, from those kept loaded or loaded now. Loading parses the page's
    /// content -- 9 ms for a typical drawing, most of a second for a dense one
    /// -- and pdfium keeps nothing between loads, so the most recently used pages
    /// stay open: reading a page's highlights, extracting its text and
    /// rendering it come together, and zooming re-renders the same pages. They
    /// are limited by count and by estimated memory, oldest closed first; the
    /// page just asked for always stays.
    fn page(&mut self, index: usize) -> Option<&mut PdfPage<'a>> {
        match self.open_pages.iter().position(|p| p.index == index) {
            Some(position) => {
                let entry = self.open_pages.remove(position);
                self.open_pages.push(entry);
            }
            None => {
                let before = private_bytes();
                let page = self.doc.pages().get(index as PdfPageIndex).ok()?;
                let bytes = private_bytes().saturating_sub(before);
                self.open_pages.push(OpenPage { index, page, bytes });
                trace(format_args!("worker: loaded page {index}, {} MB", bytes >> 20));
                self.close_over_budget();
            }
        }
        self.open_pages.last_mut().map(|p| &mut p.page)
    }

    /// Adds memory an open page took on after loading -- the images pdfium
    /// decoded to draw it -- then closes pages over the limits.
    fn grew(&mut self, index: usize, bytes: usize) {
        if let Some(open) = self.open_pages.iter_mut().find(|p| p.index == index) {
            open.bytes += bytes;
            trace(format_args!("worker: page {index} took {} MB more to draw, {} MB in all", bytes >> 20, open.bytes >> 20));
        }
        self.close_over_budget();
    }

    /// Closes page `index`, if it's open.
    fn close(&mut self, index: usize) {
        if let Some(position) = self.open_pages.iter().position(|p| p.index == index) {
            let closed = self.open_pages.remove(position);
            trace(format_args!("worker: closed page {index} after reading its text, which held {} MB", closed.bytes >> 20));
        }
    }

    /// Closes the least recently used pages while there are more than
    /// `OPEN_PAGES` or they hold more than `OPEN_PAGE_BYTES`. The most recently
    /// used page always stays.
    fn close_over_budget(&mut self) {
        while self.open_pages.len() > 1
            && (self.open_pages.len() > OPEN_PAGES || self.open_pages.iter().map(|p| p.bytes).sum::<usize>() > OPEN_PAGE_BYTES)
        {
            let closed = self.open_pages.remove(0);
            trace(format_args!("worker: closed page {}, which held {} MB", closed.index, closed.bytes >> 20));
        }
    }

    /// A page's highlights and geometry, unless they have already been read.
    /// `keep` says whether the page is about to be used again, as when it is
    /// read just before being drawn; otherwise -- the background scan -- a page
    /// not already open is loaded just for this and closed again, rather than
    /// holding memory for a page nobody may look at.
    fn scan_page(&mut self, page: usize, keep: bool) -> Option<(PageNotes, Option<PageGeometry>)> {
        if self.scanned.get(page).copied().unwrap_or(true) {
            return None;
        }
        self.scanned[page] = true;
        self.unscanned -= 1;
        let read = if keep || self.open_pages.iter().any(|p| p.index == page) {
            self.page(page).map(|loaded| annots::read_loaded_page(loaded, page))
        } else {
            let read = self.doc.pages().get(page as PdfPageIndex).ok().map(|loaded| annots::read_loaded_page(&loaded, page));
            trace(format_args!("worker: scanned page {page} for highlights, not kept"));
            read
        };
        Some(match read {
            Some((notes, geometry)) => (notes, Some(geometry)),
            None => (PageNotes::default(), None),
        })
    }

    /// Get a page ready to draw: its highlights read first, returned for
    /// sending, then deleted from this display copy the first time.
    fn prepare(&mut self, page: usize) -> Option<(PageNotes, Option<PageGeometry>)> {
        let scanned = self.scan_page(page, true);
        if self.stripped.insert(page) {
            if let Some(loaded) = self.page(page) {
                annots::strip_loaded_page(loaded);
            }
        }
        scanned
    }

    /// A page's text. Extracting it means loading the page -- tens of
    /// milliseconds for a large drawing -- so it is kept for later searches
    /// and views, while the cache has room. Saving doesn't change page text,
    /// so the cache lasts until another file is opened.
    fn text(&mut self, page: usize) -> Arc<Vec<TextChar>> {
        if let Some(chars) = self.text_cache.get(&page) {
            return Arc::clone(chars);
        }
        let chars = Arc::new(self.page(page).and_then(|loaded| annots::chars_of(loaded).ok()).unwrap_or_default());
        if !self.keep_open {
            self.close(page);
        }
        let bytes = chars.len() * std::mem::size_of::<TextChar>();
        if self.text_cache_bytes + bytes <= TEXT_CACHE_BYTES {
            self.text_cache_bytes += bytes;
            self.text_cache.insert(page, Arc::clone(&chars));
        }
        chars
    }
}

fn highlights_reply(
    generation: u64,
    page: usize,
    notes: PageNotes,
    geometry: Option<PageGeometry>,
    done: bool,
) -> Reply {
    let geometry = geometry.map(|g| vec![(page, g)]).unwrap_or_default();
    Reply::Highlights { generation, highlights: notes.highlights, markups: notes.markups, geometry, done }
}

/// Rendered pixels as a texture, made here rather than on the UI thread: for a
/// large drawing that conversion took about 20 ms, a visible hitch per new page
/// while scrolling. Pages render opaque, so the pixels are already
/// premultiplied.
pub(crate) fn make_texture(ctx: &egui::Context, name: String, size: [usize; 2], rgba: &[u8]) -> egui::TextureHandle {
    let image = egui::ColorImage::from_rgba_premultiplied(size, rgba);
    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
}

/// A drawn region of a page cut into its grid squares, each made into a
/// texture. With `keep`, the squares also go into the page cache.
pub(crate) fn make_tiles(
    ctx: &egui::Context,
    page: usize,
    full: [u32; 2],
    region: [u32; 4],
    annotations: bool,
    size: [usize; 2],
    rgba: &[u8],
    keep: Option<(&Cache, u64)>,
) -> Vec<Tile> {
    model::cut_tiles(full, region, size, rgba)
        .into_iter()
        .map(|(column, row, tile_size, pixels)| {
            let texture = make_texture(ctx, format!("page-{page}-detail-{column}-{row}"), tile_size, &pixels);
            if let Some((cache, file)) = keep {
                cache.store(Key::tile(file, page, full, column, row).annotations(annotations), tile_size, pixels);
            }
            Tile { column, row, texture }
        })
        .collect()
}

/// Every square of `region` from the page cache, as textures, if all of them
/// are there.
pub(crate) fn load_tiles(
    ctx: &egui::Context,
    cache: &Cache,
    file: u64,
    page: usize,
    full: [u32; 2],
    region: [u32; 4],
    annotations: bool,
) -> Option<Vec<Tile>> {
    model::tile_cells(full, region)
        .into_iter()
        .map(|(column, row)| {
            let (size, pixels) = cache.load(Key::tile(file, page, full, column, row).annotations(annotations))?;
            let texture = make_texture(ctx, format!("page-{page}-detail-{column}-{row}"), size, &pixels);
            Some(Tile { column, row, texture })
        })
        .collect()
}

/// A trace of loading decisions -- what the view wants, which job runs, what
/// is skipped or abandoned -- to stderr, when KINETIC_PDF_TRACE is set.
/// Otherwise it costs one check.
pub fn trace(message: std::fmt::Arguments) {
    static START: std::sync::OnceLock<Option<Instant>> = std::sync::OnceLock::new();
    if let Some(start) = START.get_or_init(|| std::env::var_os("KINETIC_PDF_TRACE").map(|_| Instant::now())) {
        eprintln!("{:>9.1} ms {:>6} MB  {message}", start.elapsed().as_secs_f64() * 1000.0, private_bytes() >> 20);
    }
}

/// This process's private memory in bytes: what it has committed for itself.
pub(crate) fn private_bytes() -> usize {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let ok = unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS, counters.cb)
    };
    if ok == 0 { 0 } else { counters.PrivateUsage }
}

struct SearchJob {
    generation: u64,
    id: u64,
    query: String,
    next_page: usize,
    found: usize,
}

/// Work on one page of the open document. Unlike opening or saving, it is only
/// worth doing while the page is still wanted, so it waits in a queue and the
/// most wanted goes first (`next_job`).
enum Job {
    Render { page: usize, scale: f32, annotations: bool },
    Region { page: usize, full: [u32; 2], region: [u32; 4], annotations: bool },
    Text { page: usize },
}

impl Job {
    fn page(&self) -> usize {
        match *self {
            Job::Render { page, .. } | Job::Region { page, .. } | Job::Text { page } => page,
        }
    }

    /// Within one page: the sharp view of the part in view first -- when it's
    /// asked for, that's what the user is looking at -- then the whole page,
    /// then its text.
    fn order(&self) -> u8 {
        match self {
            Job::Region { .. } => 0,
            Job::Render { .. } => 1,
            Job::Text { .. } => 2,
        }
    }

    /// The reply for a job dropped because its page is no longer wanted, so
    /// the UI knows to ask again if the page comes back into view.
    fn skipped(&self, generation: u64) -> Reply {
        match *self {
            Job::Render { page, .. } => Reply::RenderSkipped { generation, page },
            Job::Region { page, full, region, annotations } => Reply::RenderedRegion { generation, page, full, region, annotations, tiles: Vec::new() },
            Job::Text { page } => Reply::TextSkipped { generation, page },
        }
    }
}

/// Adds a job, replacing any older one of the same kind for the same page.
fn queue(jobs: &mut Vec<Job>, job: Job) {
    jobs.retain(|j| !(j.page() == job.page() && j.order() == job.order()));
    jobs.push(job);
}

/// Takes the most wanted job off the queue, first dropping the jobs for pages
/// no longer wanted.
fn next_job(jobs: &mut Vec<Job>, generation: u64, wanted: &Mutex<Wanted>, send: &impl Fn(Reply)) -> Option<Job> {
    let ranks: Vec<Option<usize>> = {
        let w = wanted.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter().map(|job| w.rank(generation, job.page())).collect()
    };
    let mut best: Option<(usize, (usize, u8))> = None;
    let mut kept = Vec::with_capacity(jobs.len());
    for (job, rank) in jobs.drain(..).zip(ranks) {
        match rank {
            None => {
                trace(format_args!("worker: skip job {} for page {}", job.order(), job.page()));
                send(job.skipped(generation))
            }
            Some(rank) => {
                let key = (rank, job.order());
                if best.is_none_or(|(_, b)| key < b) {
                    best = Some((kept.len(), key));
                }
                kept.push(job);
            }
        }
    }
    *jobs = kept;
    best.map(|(i, (rank, _))| {
        let job = jobs.remove(i);
        trace(format_args!("worker: start job {} for page {} (rank {rank}, {} more queued)", job.order(), job.page(), jobs.len()));
        job
    })
}

/// Does one page job on the open document.
fn do_job(
    job: Job,
    l: &mut Loaded<'_>,
    ctx: &egui::Context,
    send: &impl Fn(Reply),
    is_wanted: &impl Fn(u64, usize) -> bool,
    cache: Option<&Cache>,
) {
    let generation = l.generation;
    match job {
        Job::Text { page } => {
            // The page's highlights and geometry come with its text, from the
            // same load: when a helper draws the page, this is where they're read.
            if let Some((notes, geometry)) = l.scan_page(page, true) {
                send(highlights_reply(generation, page, notes, geometry, l.unscanned == 0));
            }
            let chars = l.text(page).as_ref().clone();
            send(Reply::Text { generation, page, chars });
        }

        Job::Render { page, scale, annotations } => {
            // The page's own highlights and geometry go first, so they are
            // there by the time its pixels are.
            if let Some((notes, geometry)) = l.prepare(page) {
                send(highlights_reply(generation, page, notes, geometry, l.unscanned == 0));
            }
            let key = Key::new(l.file, page, scale).annotations(annotations);
            if let Some((size, rgba)) = cache.and_then(|cache| cache.load(key)) {
                let texture = make_texture(ctx, format!("page-{page}"), size, &rgba);
                send(Reply::Rendered { generation, page, scale, texture, complete: true, slow: true, annotations });
                trace(format_args!("worker: page {page} came from the cache"));
                return;
            }
            let started = Instant::now();
            // Loaded first, so what drawing adds is measured on its own.
            let _ = l.page(page);
            let before = private_bytes();
            let mut shown = Instant::now();
            let rendered = match l.page(page) {
                Some(loaded) => annots::render_page_in_steps(loaded, scale, annotations, |bitmap| {
                    // A dense page shows as it draws, rather than all at once
                    // a second or more later.
                    if shown.elapsed() >= PARTIAL_EVERY {
                        let size = [bitmap.width() as usize, bitmap.height() as usize];
                        let texture = make_texture(ctx, format!("page-{page}-drawing"), size, &bitmap.as_rgba_bytes());
                        send(Reply::Rendered { generation, page, scale, texture, complete: false, slow: false, annotations });
                        trace(format_args!("worker: part-drawn page {page} sent"));
                        shown = Instant::now();
                    }
                    let wanted = is_wanted(generation, page);
                    if !wanted {
                        trace(format_args!("worker: page {page} no longer wanted, abandoning its render"));
                    }
                    wanted
                }),
                None => Err("could not load the page".to_owned()),
            };
            // Not counting the finished image, which goes to the UI.
            let image = rendered.as_ref().ok().and_then(|r| r.as_ref()).map_or(0, |(_, rgba)| rgba.len());
            l.grew(page, private_bytes().saturating_sub(before).saturating_sub(image));
            match rendered {
                Ok(Some((size, rgba))) => {
                    let took = started.elapsed().as_millis();
                    let slow = took >= u128::from(cache::SLOW_MS);
                    let texture = make_texture(ctx, format!("page-{page}"), size, &rgba);
                    // A thumbnail of the page, if none is kept; see
                    // `pool::keep_thumbnail`. Kept before the page is sent, so
                    // the UI, hearing of the page, finds it in the cache.
                    if let Some(cache) = cache {
                        if annotations && !cache.has_image(Key::thumbnail(l.file, page)) {
                            if let Some((size, pixels)) = model::thumbnail(size, &rgba, model::THUMBNAIL_WIDTH as usize) {
                                cache.store(Key::thumbnail(l.file, page), size, pixels);
                            }
                        }
                    }
                    send(Reply::Rendered { generation, page, scale, texture, complete: true, slow, annotations });
                    trace(format_args!("worker: page {page} rendered in {took} ms"));
                    if let Some(cache) = cache {
                        if slow {
                            cache.store(key, size, rgba);
                        } else {
                            cache.mark_fast(key);
                        }
                    }
                }
                Ok(None) => send(Reply::RenderSkipped { generation, page }),
                Err(error) => {
                    trace(format_args!("worker: page {page} failed: {error}"));
                    send(Reply::RenderFailed { generation, page, error })
                }
            }
        }

        Job::Region { page, full, region, annotations } => {
            if let Some((notes, geometry)) = l.prepare(page) {
                send(highlights_reply(generation, page, notes, geometry, l.unscanned == 0));
            }
            if let Some(tiles) = cache.and_then(|cache| load_tiles(ctx, cache, l.file, page, full, region, annotations)) {
                send(Reply::RenderedRegion { generation, page, full, region, annotations, tiles });
                return;
            }
            let _ = l.page(page);
            let before = private_bytes();
            let rendered = l
                .page(page)
                .map(|loaded| annots::render_region_in_steps(loaded, full, region, annotations, |_| is_wanted(generation, page)));
            let image = match &rendered {
                Some(Ok(Some((_, rgba)))) => rgba.len(),
                _ => 0,
            };
            l.grew(page, private_bytes().saturating_sub(before).saturating_sub(image));
            let tiles = match rendered {
                Some(Ok(Some((size, rgba)))) => {
                    let keep = cache.map(|cache| (cache, l.file));
                    make_tiles(ctx, page, full, region, annotations, size, &rgba, keep)
                }
                _ => Vec::new(),
            };
            send(Reply::RenderedRegion { generation, page, full, region, annotations, tiles });
        }
    }
}

/// Starts the worker thread and, if it can, `helpers` to draw pages. With a
/// `cache`, slow pages are kept on disk and the rest of the document is drawn
/// into it ahead of time.
pub fn spawn(
    ctx: egui::Context,
    wanted: Arc<Mutex<Wanted>>,
    helpers: Helpers,
    cache: Option<Arc<Cache>>,
) -> (Sender<Request>, Receiver<Reply>) {
    let (request_tx, request_rx) = mpsc::channel();
    let (reply_tx, reply_rx) = mpsc::channel();
    let (work_tx, work_rx) = mpsc::channel();
    let started = pool::start(helpers, ctx.clone(), wanted.clone(), reply_tx.clone(), work_tx.clone(), cache.clone());
    let (requests, saved) = match started {
        Some(helpers) => {
            let saved = helpers.inputs.clone();
            let wanted = wanted.clone();
            std::thread::Builder::new()
                .name("requests".into())
                .spawn(move || route(request_rx, helpers, work_tx, wanted))
                .expect("could not start the request thread");
            (work_rx, Some(saved))
        }
        None => (request_rx, None),
    };
    std::thread::Builder::new()
        .name("pdfium".into())
        .spawn(move || run(requests, reply_tx, ctx, wanted, saved, cache))
        .expect("could not start the pdfium thread");
    (request_tx, reply_rx)
}

/// Sends page renders to the helpers and everything else to the worker, in the
/// order they came. Opening goes to both. A render for the helpers never waits
/// behind what the worker is busy with. Whether pdfium draws a page's
/// annotations is as `wanted` says when the render is asked for.
fn route(requests: Receiver<Request>, helpers: pool::Pool, work: Sender<Request>, wanted: Arc<Mutex<Wanted>>) {
    let draws_annotations = |page: usize| wanted.lock().map(|w| w.draws_annotations(page)).unwrap_or(true);
    for request in requests {
        let alive = helpers.alive.load(Ordering::Relaxed);
        match request {
            Request::Render { generation, page, scale } if alive => {
                let target = Target::Page { scale, annotations: draws_annotations(page) };
                if helpers.inputs.send(pool::Input::Render { generation, page, target }).is_err() {
                    let _ = work.send(Request::Render { generation, page, scale });
                }
            }
            Request::RenderRegion { generation, page, full, region } if alive => {
                let target = Target::Region { full, region, annotations: draws_annotations(page) };
                if helpers.inputs.send(pool::Input::Render { generation, page, target }).is_err() {
                    let _ = work.send(Request::RenderRegion { generation, page, full, region });
                }
            }
            Request::PredictPage { generation, page, scale } if alive => {
                let target = Target::Page { scale, annotations: draws_annotations(page) };
                let _ = helpers.inputs.send(pool::Input::Predict { generation, page, target });
            }
            Request::PredictRegion { generation, page, full, region } if alive => {
                let target = Target::Region { full, region, annotations: draws_annotations(page) };
                let _ = helpers.inputs.send(pool::Input::Predict { generation, page, target });
            }
            Request::Open { generation, path } => {
                let _ = helpers.inputs.send(pool::Input::Open { generation, path: path.clone() });
                let _ = work.send(Request::Open { generation, path });
            }
            other => {
                let _ = work.send(other);
            }
        }
    }
    let _ = helpers.inputs.send(pool::Input::Shutdown);
}

fn run(
    requests: Receiver<Request>,
    replies: Sender<Reply>,
    ctx: egui::Context,
    wanted: Arc<Mutex<Wanted>>,
    helpers: Option<Sender<pool::Input>>,
    cache: Option<Arc<Cache>>,
) {
    let send = |reply: Reply| {
        let _ = replies.send(reply);
        ctx.request_repaint();
    };
    // Whether the UI still wants a page's pixels, or scrolled past while the
    // request sat in the queue.
    let is_wanted = |generation: u64, page: usize| wanted.lock().map(|w| w.rank(generation, page).is_some()).unwrap_or(true);
    let draws_annotations = |page: usize| wanted.lock().map(|w| w.draws_annotations(page)).unwrap_or(true);

    let pdfium = match bind() {
        Ok(pdfium) => pdfium,
        Err(message) => {
            send(Reply::Fatal(message));
            return;
        }
    };

    let mut loaded: Option<Loaded> = None;
    let mut search: Option<SearchJob> = None;
    let mut jobs: Vec<Job> = Vec::new();

    loop {
        let (moving, keep_open) = wanted.lock().map(|w| (w.moving, !w.skip_drawing_ahead)).unwrap_or((false, true));
        if let Some(l) = loaded.as_mut() {
            l.keep_open = keep_open;
        }
        let scanning = loaded.as_ref().is_some_and(|l| l.unscanned > 0);
        let mut batch = Vec::new();
        if jobs.is_empty() && search.is_none() && !(scanning && !moving) {
            // Nothing to get on with: wait for a request -- or, if the
            // highlight scan is waiting for the view to settle, not for long.
            let request = if scanning {
                match requests.recv_timeout(Duration::from_millis(50)) {
                    Ok(request) => Some(request),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match requests.recv() {
                    Ok(request) => Some(request),
                    Err(_) => return,
                }
            };
            batch.extend(request);
        }
        batch.extend(requests.try_iter());

        for request in batch {
            match request {
                Request::Open { generation, path } => {
                    loaded = None;
                    search = None;
                    jobs.clear();
                    let started = Instant::now();
                    let bytes = match std::fs::read(&path) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            send(Reply::OpenFailed { generation, error: e.to_string() });
                            continue;
                        }
                    };
                    let read = started.elapsed();
                    let doc = match pdfium.load_pdf_from_byte_vec(bytes.clone(), None) {
                        Ok(doc) => doc,
                        Err(e) => {
                            send(Reply::OpenFailed { generation, error: e.to_string() });
                            continue;
                        }
                    };
                    let parsed = started.elapsed();
                    let page_sizes = annots::page_sizes(&doc);
                    let page_labels = annots::page_labels(&doc);
                    let pages = page_sizes.len();
                    // The page cache's key for this file; see cache.rs.
                    let file = cache::fingerprint(&bytes);
                    send(Reply::Opened { generation, path: path.clone(), file, page_sizes, page_labels });
                    // Highlights come afterwards: see `Reply::Highlights`.
                    if pages == 0 {
                        send(Reply::Highlights { generation, highlights: Vec::new(), markups: Vec::new(), geometry: Vec::new(), done: true });
                    }
                    let sized = started.elapsed();
                    trace(format_args!(
                        "worker: opened {pages} pages, {} MB file: read {:.1} ms, parsed {:.1} ms, sized {:.1} ms, fingerprinted {:.1} ms",
                        bytes.len() >> 20,
                        read.as_secs_f64() * 1000.0,
                        (parsed - read).as_secs_f64() * 1000.0,
                        (sized - parsed).as_secs_f64() * 1000.0,
                        (started.elapsed() - sized).as_secs_f64() * 1000.0
                    ));
                    if let Some(helpers) = &helpers {
                        let layers = pdf_content::layers::may_hide_annotations(&bytes);
                        let _ = helpers.send(pool::Input::File { generation, fingerprint: file, layers });
                    }
                    loaded = Some(Loaded {
                        generation,
                        path,
                        file,
                        bytes,
                        open_pages: Vec::new(),
                        doc,
                        stripped: HashSet::new(),
                        scanned: vec![false; pages],
                        unscanned: pages,
                        scan_next: 0,
                        text_cache: HashMap::new(),
                        text_cache_bytes: 0,
                        keep_open: true,
                    });
                }

                // Page work queues; see `next_job`.
                Request::Text { generation, page } => {
                    if loaded.as_ref().is_some_and(|l| l.generation == generation) {
                        queue(&mut jobs, Job::Text { page });
                    }
                }

                Request::Render { generation, page, scale } => {
                    if loaded.as_ref().is_some_and(|l| l.generation == generation) {
                        queue(&mut jobs, Job::Render { page, scale, annotations: draws_annotations(page) });
                    }
                }

                Request::RenderRegion { generation, page, full, region } => {
                    if loaded.as_ref().is_some_and(|l| l.generation == generation) {
                        queue(&mut jobs, Job::Region { page, full, region, annotations: draws_annotations(page) });
                    }
                }

                // Scales and measurements need a pass over the whole file with
                // lopdf, which pdfium can't do: about half a second and a few
                // hundred MB on a large drawing set. So it happens on a thread
                // of its own, off the file on disk, leaving this one to draw.
                Request::ReadMeasurements { generation } => {
                    let Some(l) = loaded.as_ref().filter(|l| l.generation == generation) else { continue };
                    let (path, replies, ctx) = (l.path.clone(), replies.clone(), ctx.clone());
                    let started = std::thread::Builder::new().name("measurements".into()).spawn(move || {
                        let reply = match read_measurements(&path) {
                            Ok(measurements) => Reply::Measured { generation, measurements: Box::new(measurements) },
                            Err(error) => Reply::MeasureFailed { generation, error },
                        };
                        let _ = replies.send(reply);
                        ctx.request_repaint();
                    });
                    if let Err(e) = started {
                        send(Reply::MeasureFailed { generation, error: e.to_string() });
                    }
                }

                Request::Save { generation, changes, arrangement } => {
                    let Some(l) = loaded.as_mut().filter(|l| l.generation == generation) else { continue };
                    // The annotations go in first, against the pages as the
                    // file still holds them, and the pages are moved after --
                    // which carries each page's annotations along with it, so
                    // a markup stays on the sheet it was drawn on however far
                    // that sheet has been moved.
                    let written = annots::save(&pdfium, &l.bytes, &changes)
                        .and_then(|mut saved| {
                            if let Some(sheets) = &arrangement {
                                saved.bytes = rearranged_bytes(&saved.bytes, sheets)?;
                            }
                            Ok(saved)
                        })
                        .and_then(|saved| write_atomically(&l.path, &saved.bytes).map(|()| saved));
                    let saved = match written {
                        Ok(saved) => saved,
                        Err(error) => {
                            send(Reply::SaveFailed { generation, error });
                            continue;
                        }
                    };
                    // The file on disk is new. Highlights are never drawn into a
                    // page, so its cached pages move to the new fingerprint --
                    // but for pages whose markups changed, which are drawn
                    // afresh, as is the copy to draw from. The helpers, still
                    // reading the old file, reopen it.
                    let file = cache::fingerprint(&saved.bytes);
                    if let Some(cache) = &cache {
                        // Rearrangement changes the meaning of page indices and
                        // rotations. Images, tiles, shapes and drawing copies
                        // under the old fingerprint cannot be reused as-is.
                        if arrangement.is_none() {
                            cache.rekey(l.file, file);
                            if !saved.redrawn.is_empty() {
                                cache.forget_drawn(file, &saved.redrawn);
                            }
                        }
                    }
                    l.file = file;
                    if let Some(helpers) = &helpers {
                        let _ = helpers.send(pool::Input::Saved { generation, fingerprint: file, redrawn: arrangement.is_some() || !saved.redrawn.is_empty() });
                    }
                    match pdfium.load_pdf_from_byte_vec(saved.bytes.clone(), None) {
                        Ok(doc) => {
                            // Close the old document's pages before the old
                            // document itself goes.
                            l.open_pages.clear();
                            l.doc = doc;
                            l.bytes = saved.bytes;
                            l.stripped.clear();
                            if arrangement.is_some() {
                                jobs.clear();
                                l.text_cache.clear();
                                l.text_cache_bytes = 0;
                                l.scanned = vec![false; l.doc.pages().len() as usize];
                                l.unscanned = l.scanned.len();
                                l.scan_next = 0;
                                // The UI reopens with a fresh generation. Do not
                                // report old annotation keys against new pages.
                                send(Reply::Saved { generation, pages: Vec::new(), highlights: Vec::new(), markups: Vec::new(), redrawn: Vec::new() });
                                continue;
                            }
                            // A save only moves annotations on the pages it
                            // changed, so only those are read again; every other
                            // highlight keeps the position it already has.
                            let changed = changes.pages();
                            let mut notes = PageNotes::default();
                            for &page in &changed {
                                if page < l.scanned.len() && !l.scanned[page] {
                                    l.scanned[page] = true;
                                    l.unscanned -= 1;
                                }
                                let (read, _) = annots::read_page(&l.doc, page);
                                notes.highlights.extend(read.highlights);
                                notes.markups.extend(read.markups);
                            }
                            // Markups just written keep their shapes, to show
                            // until their pages are drawn with them.
                            for (key, written) in saved.markups.iter().zip(&changes.markups) {
                                if let Some(m) = notes.markups.iter_mut().find(|m| m.key == Some(*key)) {
                                    *m = Markup { key: m.key, author: std::mem::take(&mut m.author), ..written.clone() };
                                }
                            }
                            send(Reply::Saved {
                                generation,
                                pages: changed.into_iter().collect(),
                                highlights: notes.highlights,
                                markups: notes.markups,
                                redrawn: saved.redrawn.into_iter().collect(),
                            });
                            if l.unscanned == 0 {
                                send(Reply::Highlights { generation, highlights: Vec::new(), markups: Vec::new(), geometry: Vec::new(), done: true });
                            }
                        }
                        Err(e) => {
                            loaded = None;
                            send(Reply::SaveFailed {
                                generation,
                                error: format!("the file was written but could not be read back: {e}"),
                            });
                        }
                    }
                }

                // Without helpers to spare, drawing ahead of a zoom isn't worth
                // holding up anything else.
                Request::PredictPage { .. } | Request::PredictRegion { .. } => {}

                Request::Search { generation, id, query } => {
                    search = (!selection::normalize_query(&query).is_empty())
                        .then_some(SearchJob { generation, id, query, next_page: 0, found: 0 });
                }
            }
        }

        // Then one page job, most wanted first, before looking at requests
        // again, so a new scroll position counts after at most one job; a
        // render in progress checks for itself. The highlight scan only runs
        // when no page is waiting.
        match loaded.as_mut() {
            Some(l) => match next_job(&mut jobs, l.generation, &wanted, &send) {
                Some(job) => do_job(job, l, &ctx, &send, &is_wanted, cache.as_deref()),
                // Read fresh: the view may have started moving since.
                None if l.unscanned > 0 && !wanted.lock().map(|w| w.moving).unwrap_or(false) => scan_step(l, &send),
                None => {}
            },
            None => jobs.clear(),
        }
        let finished = match (search.as_mut(), loaded.as_mut()) {
            (Some(job), Some(l)) if l.generation == job.generation => search_step(job, l, &send),
            (Some(_), _) => true,
            _ => false,
        };
        if finished {
            search = None;
        }
    }
}

/// Reads highlights for pages not yet shown, for about 20 ms, then reports
/// what it found, so the notes panel fills in without holding up renders.
fn scan_step(l: &mut Loaded<'_>, send: &impl Fn(Reply)) {
    let started = Instant::now();
    let mut found = PageNotes::default();
    let mut geometry = Vec::new();
    while l.unscanned > 0 && started.elapsed() < Duration::from_millis(20) {
        while l.scan_next < l.scanned.len() && l.scanned[l.scan_next] {
            l.scan_next += 1;
        }
        let page = l.scan_next;
        let Some((notes, page_geometry)) = l.scan_page(page, false) else {
            // Every page has been read, whatever the count said.
            l.unscanned = 0;
            break;
        };
        found.highlights.extend(notes.highlights);
        found.markups.extend(notes.markups);
        geometry.extend(page_geometry.map(|g| (page, g)));
    }
    let done = l.unscanned == 0;
    if !found.highlights.is_empty() || !found.markups.is_empty() || !geometry.is_empty() || done {
        send(Reply::Highlights { generation: l.generation, highlights: found.highlights, markups: found.markups, geometry, done });
    }
}

/// Searches pages for a few milliseconds, then reports what it found, so
/// renders and other requests keep flowing during a long search instead of
/// waiting for it. Returns true once the search is complete.
fn search_step(job: &mut SearchJob, l: &mut Loaded<'_>, send: &impl Fn(Reply)) -> bool {
    let total = l.scanned.len();
    let started = Instant::now();
    let mut hits = Vec::new();

    while job.next_page < total && job.found < MAX_SEARCH_HITS && started.elapsed() < Duration::from_millis(30) {
        {
            // From the cache after the first search, so a repeat search never
            // loads a page.
            let chars = l.text(job.next_page);
            for range in selection::find(&chars, &job.query) {
                let quads = selection::bands(&chars, range.clone());
                if !quads.is_empty() && job.found < MAX_SEARCH_HITS {
                    let (before, matched, after) = selection::context(&chars, range, 40, 120);
                    hits.push(SearchHit { page: job.next_page, quads, before, matched, after });
                    job.found += 1;
                }
            }
        }
        job.next_page += 1;
    }

    let done = job.next_page >= total || job.found >= MAX_SEARCH_HITS;
    send(Reply::Search { generation: job.generation, id: job.id, searched: job.next_page, hits, done });
    done
}

/// pdfium.dll is compiled into the exe (build.rs checks it is there), so the
/// app ships as a single file. Windows can only load a DLL from disk, so it is
/// written out once and loaded from there.
#[cfg(not(feature = "store"))]
static PDFIUM_DLL: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/pdfium.dll"));

/// pdfium-render keeps the library in a process-wide slot that can be filled
/// only once: after the first `Pdfium::new`, every later bind fails with
/// `PdfiumLibraryBindingsAlreadyInitialized`. Two workers binding at once
/// usually both get in before either fills it, which is how the tests with
/// several workers to a process passed until one started late on CI. So the
/// process binds once, and every worker shares it; the calls were already
/// serialised behind that one library whichever worker made them.
pub fn bind() -> Result<&'static Pdfium, String> {
    static PDFIUM: std::sync::OnceLock<Result<Pdfium, String>> = std::sync::OnceLock::new();
    PDFIUM.get_or_init(load).as_ref().map_err(Clone::clone)
}

#[cfg(not(feature = "store"))]
fn load() -> Result<Pdfium, String> {
    let library = unpack_pdfium().map_err(|e| format!("Could not unpack pdfium.dll: {e}"))?;
    bind_to(&library)
}

/// The Store package ships pdfium.dll beside the exe, where the package's
/// signature covers it; a copy written out at runtime wouldn't be.
#[cfg(feature = "store")]
fn load() -> Result<Pdfium, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Could not find the app's folder: {e}"))?;
    bind_to(&exe.with_file_name("pdfium.dll"))
}

fn bind_to(library: &Path) -> Result<Pdfium, String> {
    Pdfium::bind_to_library(library)
        .map(Pdfium::new)
        .map_err(|e| format!("Could not load {}: {e}", library.display()))
}

/// Writes the embedded DLL to %LOCALAPPDATA%\kinetic-pdf, named by a hash of
/// its contents, so a newer build never collides with an older one that is
/// still running and holding its copy open. Later launches reuse the file.
#[cfg(not(feature = "store"))]
pub fn unpack_pdfium() -> std::io::Result<PathBuf> {
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("kinetic-pdf");
    unpack_pdfium_to(&dir)
}

/// `unpack_pdfium`, into a given folder. The benchmark uses this to time a
/// first launch without disturbing the app's real copy.
#[cfg(not(feature = "store"))]
pub fn unpack_pdfium_to(dir: &Path) -> std::io::Result<PathBuf> {
    let path = dir.join(format!("pdfium-{:016x}.dll", fnv1a(PDFIUM_DLL)));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == PDFIUM_DLL.len() as u64) {
        return Ok(path);
    }

    // Write under a name unique to this process and rename into place, so two
    // copies of the app starting together never load a half-written file.
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("pdfium-{}.tmp", std::process::id()));
    std::fs::write(&tmp, PDFIUM_DLL)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        // Fine if another instance got there first.
        if !path.exists() {
            return Err(e);
        }
    }
    Ok(path)
}

#[cfg_attr(feature = "store", allow(dead_code))]
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, b| (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// Write to a temporary file beside the original, then swap it in, so a failed
/// write never leaves a half-written PDF behind.
/// The file with its pages put in the order `sheets` says: reordered, left
/// out, repeated, turned, with blanks between. Done on the bytes the
/// annotations were just written into, never on the file on disk, so the
/// original is replaced once, in one atomic write, or not at all.
fn rearranged_bytes(bytes: &[u8], sheets: &[crate::arrange::Sheet]) -> Result<Vec<u8>, String> {
    let mut doc = pdf_content::lopdf::Document::load_mem(bytes).map_err(|e| e.to_string())?;
    crate::arrange::rearrange(&mut doc, sheets)?;
    let mut out = Vec::with_capacity(bytes.len());
    doc.save_to(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("kinetic-pdf.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("could not write the file: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not replace the file (is it open in another program?): {e}")
    })
}


/// Reads a file's scales and measurements with lopdf. Costs a pass over the
/// whole file, so it runs on a thread of its own (`Request::ReadMeasurements`).
fn read_measurements(path: &Path) -> Result<Measurements, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let doc = pdf_content::lopdf::Document::load_mem(&bytes).map_err(|e| e.to_string())?;
    drop(bytes);
    let read = pdf_io::read(&doc);
    Ok(Measurements {
        scales: read.scales,
        markups: read.markups,
        skipped: read.skipped.into_iter().map(|(page, why)| format!("page {}: {why}", page + 1)).collect(),
    })
}
