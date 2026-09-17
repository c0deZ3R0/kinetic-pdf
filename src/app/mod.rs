//! The window: toolbar, find box, page viewer, the notes and search-results
//! side panels, and the highlight popup.
//!
//! This thread never touches pdfium. It lays out every page from sizes the
//! worker reports up front, asks for pixels and text only for the pages in
//! view, and draws highlights and search matches itself on top of the
//! rendered pages.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Id, Key, LayerId, Layout,
    Margin, Modifiers, Order, Pos2, Rect, RichText, Sense, Shadow, Shape, Stroke, StrokeKind, TextEdit,
    TextureHandle, TextureId, Ui, Vec2, ViewportCommand,
};

use crate::model::{Highlight, Markup, MarkupKind, PageGeometry, PdfBox, Reply, Request, Rgb, SearchHit, TextChar};
use crate::selection;
use crate::session::{Command, HighlightEntry, Session};
use crate::cache::{self, Cache};
use crate::worker::{self, Wanted, MAX_SEARCH_HITS};

mod about;
mod discard;
mod drag;
mod gpu;
mod layout;
mod markups;
mod notes;
mod pages;
mod scroll_bench;
mod search;
mod style;
mod toolbar;
mod widgets;
mod page_bench;
mod work_bench;
mod thumb_bench;
mod zoom_bench;

pub use layout::{quantize_scale, render_scale};
use discard::*;
use drag::*;
use layout::*;
use markups::*;
use notes::*;
use pages::*;
use style::*;
use widgets::*;

/* ------------------------------------------------------------------ *
 * State
 * ------------------------------------------------------------------ */

struct PageTexture {
    handle: TextureHandle,
    /// Pixels per PDF point it was rendered at.
    scale: f32,
    /// False while a slow page is still drawing in.
    complete: bool,
    /// Whether pdfium drew the page's annotations in it.
    annotations: bool,
}

/// One square of a page drawn zoomed in, placed on the page's grid (see
/// `model::TILE`): the page, its full size in pixels at that zoom, the
/// square's column and row, and whether pdfium drew the page's annotations.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TileKey {
    page: usize,
    full: [u32; 2],
    column: u32,
    row: u32,
    annotations: bool,
}

/// Something asked for ahead of a zoom: a whole page at a scale, or a region
/// of a page.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PredictKey {
    Page(usize, u32),
    Region(usize, [u32; 4]),
}

struct TileImage {
    handle: TextureHandle,
    /// When it was last wanted on screen, so the least recently used go first.
    used: f64,
}

/// A small image of a page, kept for the whole document so a page scrolled
/// back to shows something at once; see `gpu::THUMBNAIL_WIDTH`.
struct Thumbnail {
    handle: TextureHandle,
    /// When it was last shown, so the ones furthest from the view go first.
    used: f64,
}

struct Doc {
    generation: u64,
    name: String,
    /// The file, to read again after a save changes how its pages are drawn.
    path: PathBuf,
    /// The fingerprint of its contents, which keys what the page cache keeps
    /// for it.
    file: u64,
    /// Page sizes in points, as displayed (rotated).
    sizes: Vec<Vec2>,
    /// The size most pages share; fit-to-width and shrinking work from it.
    usual_size: Vec2,
    /// Each page's rotation and visible box, once the worker has read it.
    /// Nothing is drawn over a page, and it can't be selected, until then.
    geometry: Vec<Option<PageGeometry>>,
    /// The highlights and markups, every change to them, and what's unsaved.
    session: Session,
    /// Whether every page's highlights have arrived from the worker.
    highlights_done: bool,
    /// Pages whose drawing is out of date since a save changed their markups,
    /// until they're drawn again; meanwhile the markups just saved show as
    /// drawn here.
    redraw: HashSet<usize>,
    text: HashMap<usize, Vec<TextChar>>,
    text_pending: HashSet<usize>,
    textures: HashMap<usize, PageTexture>,
    render_pending: HashSet<usize>,
    /// Squares of pages drawn zoomed in, kept while they fit `TILE_BUDGET`, so
    /// zooming back in or scrolling back over an area shows them at once.
    tiles: HashMap<TileKey, TileImage>,
    /// For each page drawn zoomed in this frame, its full size in pixels,
    /// which says which of its squares to draw.
    tile_full: HashMap<usize, [u32; 2]>,
    /// Earlier whole-page images, by page and scale, for zooming back to them
    /// -- and whole pages drawn ahead of a zoom, for zooming to them.
    spares: HashMap<(usize, u32), PageTexture>,
    /// What's been asked for ahead of a zoom, and when.
    predicting: HashMap<PredictKey, f64>,
    detail_pending: HashSet<usize>,
    failed: HashSet<usize>,
    /// Pages that were slow to draw: zooming redraws them only once the zoom
    /// settles, and zoomed in, the part in view is drawn on its own first.
    slow: HashSet<usize>,
    /// Who draws each page, pdfium or the GPU, once asked.
    drawing: HashMap<usize, gpu::PageDrawing>,
    /// Reads pages into shapes for the GPU, if there's one.
    reader: Option<gpu::Reader>,
    /// The page whose shapes are on their way to the GPU.
    uploading: Option<gpu::Uploading>,
    /// A small image of each page seen, shown while what draws it properly is
    /// on its way, and kept in the page cache between sessions.
    thumbnails: HashMap<usize, Thumbnail>,
    /// When each page's thumbnail was last asked of the cache. Asked again
    /// after a while, since one may have been kept since: the page may have
    /// been drawn, by pdfium or here, after the first time it was asked for.
    thumbs_asked: HashMap<usize, f64>,
    /// Reads thumbnails back from the cache.
    thumbs: Option<gpu::Thumbnails>,
    /// Pages read ahead just to be thumbnailed, so each is tried once.
    thumbs_ahead: HashSet<usize>,
    /// The page the shapes reader is on, if any, and whether just for its
    /// thumbnail. It gives way to a page wanted more that is waiting.
    reading: Option<(usize, bool)>,
    /// Pages that took a while to read into shapes, whose shapes are kept
    /// even while the page is small enough to show from its thumbnail.
    slow_to_read: HashSet<usize>,
    /// What each page's shapes came to when it was read, which says what they
    /// would come to at another zoom; see `gpu::Sizes`.
    shape_sizes: HashMap<usize, gpu::Sizes>,
    /// Shapes let go where there was no GPU in hand to free them; freed on the
    /// next frame.
    releasing: Vec<Arc<gpu::Uploaded>>,
    /// Pages too big for the GPU at the zoom in view, which pdfium is drawing
    /// but whose shapes still draw them until it has (`finish_handing_over`).
    handing_over: HashSet<usize>,
    /// Pages the GPU has nothing to draw of, whatever the zoom: reading them
    /// came back with nothing it could use, so they are never read again --
    /// without this they are read, turned down and read again, every frame.
    left_to_pdfium: HashSet<usize>,
}

/// Where every page sits in the scrolling column at the current zoom.
struct PageLayout {
    /// Top of each page, in content coordinates.
    tops: Vec<f32>,
    /// Screen points per PDF point for each page: the zoom, times its shrink
    /// if it's an oversized page being shrunk to fit.
    scales: Vec<f32>,
    /// The widest page as drawn.
    widest: f32,
    /// Height of the whole column.
    height: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ZoomMode {
    /// Follow the window: the usual page fills its width.
    FitWidth,
    /// Follow the window: the usual page fits entirely.
    FitPage,
    /// Whatever the user zoomed to.
    Custom,
}

/// A spot on a page to hold still under a point on screen while the zoom
/// changes, so zooming into a detail keeps it in view.
#[derive(Clone, Copy)]
struct ZoomAnchor {
    page: usize,
    /// Where on the page, as fractions of its displayed width and height.
    fx: f32,
    fy: f32,
    /// Where on screen it should stay.
    screen: Pos2,
    /// The screen position of the content's top-left corner at zero scroll.
    origin: Pos2,
}

/// A selection being dragged out.
enum Drag {
    /// Following the text, as (page, caret) at each end.
    Text { anchor: (usize, usize), focus: (usize, usize) },
    /// With Ctrl held: a box on one page, its corners in PDF user space.
    /// Everything whose centre is inside it is selected.
    Box { page: usize, start: (f32, f32), end: (f32, f32) },
    /// With a drawing tool: the markup being drawn, on the page it started on.
    Markup(Markup),
}

/// A point on a page in PDF user space. The popup is pinned to one of these
/// rather than to the screen, so it stays with its text through scrolling.
#[derive(Clone, Copy)]
struct Anchor {
    page: usize,
    x: f32,
    y: f32,
}

struct Pending {
    page: usize,
    quads: Vec<PdfBox>,
    text: String,
}

enum PopupMode {
    /// One entry per page the selection covers.
    Create(Vec<Pending>),
    Edit(u64),
    /// A markup's note.
    Markup(u64),
}

/// One popup serves both "highlight this selection" and "edit this note".
struct Popup {
    mode: PopupMode,
    anchor: Anchor,
    color: Rgb,
    note: String,
    just_opened: bool,
    /// Its height last frame, for deciding whether it fits below the text.
    height: f32,
}

enum PopupAction {
    None,
    Commit,
    Cancel,
    Delete,
}

enum Status {
    Idle,
    Opening,
    Saving,
    Saved { until: f64 },
}

/// The right-hand side panel. Notes and search results take turns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sidebar {
    None,
    Notes,
    Results,
}

#[derive(Clone, Copy)]
enum Tone {
    /// Filled blue: the main action.
    Primary,
    /// White with a border; tinted blue when `selected`.
    Secondary,
    /// No border until hovered.
    Ghost,
    /// Red text, for Delete.
    Danger,
}

/// Find in document. `query` is what's in the box; `sent` is what the worker
/// is searching for, which lags behind while the user is still typing.
#[derive(Default)]
struct Search {
    query: String,
    sent: String,
    edited_at: f64,
    /// Identifies the latest search, so batches from an older one are ignored.
    id: u64,
    /// In page order, as the worker finds them.
    hits: Vec<SearchHit>,
    /// Pages searched so far.
    searched: usize,
    done: bool,
    current: Option<usize>,
    /// The page in view when the search started. The first match shown is the
    /// first one from there on, not the first in the document.
    start_page: usize,
    /// Move keyboard focus to the find box next frame (Ctrl+F).
    focus: bool,
    /// Bring this row of the results list into view when the panel next draws.
    reveal_row: Option<usize>,
    /// Scroll the results list here (back to the top for a new search).
    list_scroll_to: Option<f32>,
    /// Where the results list was scrolled to, and how tall it was, last frame.
    list_offset: f32,
    list_height: f32,
}

pub struct App {
    tx: Sender<Request>,
    rx: Receiver<Reply>,
    /// To start reading a file's pages into shapes as soon as it is opened,
    /// and to ask for a repaint from anywhere.
    ctx: egui::Context,
    /// The reader started when a file was opened, until the worker says the
    /// file is open and it goes to the document.
    pending_reader: Option<(u64, gpu::Reader)>,
    /// The page cache, which also keeps the shapes pages are read into.
    cache: Option<Arc<Cache>>,
    wanted: Arc<Mutex<Wanted>>,
    fatal: Option<String>,
    doc: Option<Doc>,
    /// Bumped on every open; replies for older generations are ignored.
    generation: u64,
    zoom: f32,
    zoom_mode: ZoomMode,
    /// Show oversized pages at the usual page width rather than actual size.
    shrink_wide: bool,
    zoom_anchor: Option<ZoomAnchor>,
    status: Status,
    toast: Option<(String, f64)>,
    sidebar: Sidebar,
    author: String,
    active: Option<u64>,
    drag: Option<Drag>,
    /// The drawing tool in use, or `None` to select text and open notes.
    tool: Option<MarkupKind>,
    /// The colour and stroke width, in points, new markups take.
    markup_color: Rgb,
    markup_width: f32,
    /// Memory allowed for squares and for spare page images; see `budgets`.
    tile_budget: usize,
    spare_budget: usize,
    popup: Option<Popup>,
    search: Search,
    /// Scroll the page view here next frame.
    scroll_x: Option<f32>,
    scroll_y: Option<f32>,
    /// The page view's scroll offset last frame.
    scroll_offset: Vec2,
    /// Where the page view's content started on screen last frame.
    content_origin: Pos2,
    /// Screen rectangles of the pages drawn this frame.
    page_rects: HashMap<usize, Rect>,
    viewer_rect: Rect,
    current_page: usize,
    /// What the toolbar's page box holds: the page in view, unless it is being
    /// typed in.
    page_box: String,
    /// Move keyboard focus to the page box next frame (Ctrl+G).
    page_box_focus: bool,
    /// When the pages were last drawn, and the top of the view then: how fast
    /// the view is moving.
    last_view: Option<(f64, f32)>,
    /// Whether the view last moved down the document.
    heading_down: bool,
    /// The zoom last frame, and when it last changed; see `ZOOM_SETTLE`.
    last_zoom: f32,
    zoom_changed_at: f64,
    /// Every page's drawing scale, as last shared with the helpers.
    render_scales: Arc<Vec<f32>>,
    /// Whether everything in view was drawn at full sharpness last frame.
    view_sharp: bool,
    /// Whether a page in view had nothing of its own on screen last frame:
    /// paper with its thumbnail stretched over it, standing in for a drawing.
    view_stood_in: bool,
    /// Pages in view drawn this frame with nothing of them at all: no image,
    /// no shapes, no squares and no thumbnail. What the thumbnail benchmark counts.
    blank_pages: Vec<usize>,
    /// Where the pointer is resting, and since when; see `PREDICT_REST`.
    pointer_rest: Option<(Pos2, f64)>,
    /// Whether a middle-button drag is moving the document.
    panning: bool,
    allow_close: bool,
    /// What waits on the user agreeing to lose unsaved changes.
    discarding: Option<Discarding>,
    /// The GPU that draws pages, unless pdfium draws them all.
    gpu: Option<gpu::Gpu>,
    /// With `KINETIC_PDF_SCROLL_BENCH=1`; see scroll_bench.rs.
    scroll_bench: Option<scroll_bench::ScrollBench>,
    /// With `KINETIC_PDF_ZOOM_BENCH=<page>`; see zoom_bench.rs.
    zoom_bench: Option<zoom_bench::ZoomBench>,
    /// With `KINETIC_PDF_PAGE_BENCH=<sheets>`; see page_bench.rs.
    page_bench: Option<page_bench::PageBench>,
    /// With `KINETIC_PDF_WORK_BENCH=<sheet>`; see work_bench.rs.
    work_bench: Option<work_bench::WorkBench>,
    /// With `KINETIC_PDF_THUMB_BENCH=<zoom>`; see thumb_bench.rs.
    thumb_bench: Option<thumb_bench::ThumbBench>,
    /// The GPU and driver the window draws with, for the benchmarks' reports.
    gl_name: String,
    /// Newer releases on GitHub, and installing them.
    updater: crate::update::Updater,
    /// Whether the About dialog, with the licences, is open.
    show_about: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        apply_light_style(&cc.egui_ctx);
        // Ctrl +/- zooms the page, not the whole interface.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);

        let wanted = Arc::new(Mutex::new(Wanted::default()));
        // Slow pages are kept on disk between sessions. KINETIC_PDF_CACHE=0
        // turns that off, for comparing; any other value is a folder to use.
        let cache = match std::env::var_os("KINETIC_PDF_CACHE") {
            Some(value) if value == "0" => None,
            Some(dir) => Cache::open(PathBuf::from(dir), cache::DEFAULT_LIMIT).ok(),
            None => Cache::open(cache::default_dir(), cache::DEFAULT_LIMIT).ok(),
        };
        let cache = cache.map(Arc::new);
        let (tx, rx) = worker::spawn(cc.egui_ctx.clone(), wanted.clone(), crate::pool::Helpers::from_current_exe(), cache.clone());
        let (tile_budget, spare_budget) = budgets(crate::pool::free_memory());
        worker::trace(format_args!("ui: {} MB for squares and {} MB for spares", tile_budget >> 20, spare_budget >> 20));

        let mut app = Self {
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
            pending_reader: None,
            cache,
            wanted,
            fatal: None,
            doc: None,
            generation: 0,
            zoom: 1.0,
            zoom_mode: ZoomMode::FitWidth,
            shrink_wide: true,
            zoom_anchor: None,
            status: Status::Idle,
            toast: None,
            sidebar: Sidebar::None,
            author: load_author(),
            active: None,
            drag: None,
            tool: None,
            markup_color: MARKUP_COLORS[0].1,
            markup_width: WIDTHS[1].1,
            tile_budget,
            spare_budget,
            popup: None,
            search: Search::default(),
            scroll_x: None,
            scroll_y: None,
            scroll_offset: Vec2::ZERO,
            content_origin: Pos2::ZERO,
            page_rects: HashMap::new(),
            viewer_rect: Rect::NOTHING,
            current_page: 0,
            page_box: "1".to_owned(),
            page_box_focus: false,
            last_view: None,
            heading_down: true,
            last_zoom: 1.0,
            zoom_changed_at: f64::NEG_INFINITY,
            render_scales: Arc::new(Vec::new()),
            view_sharp: false,
            view_stood_in: false,
            blank_pages: Vec::new(),
            pointer_rest: None,
            panning: false,
            allow_close: false,
            discarding: None,
            gpu: gpu::Gpu::new(cc),
            scroll_bench: scroll_bench::ScrollBench::from_env(),
            zoom_bench: zoom_bench::ZoomBench::from_env(),
            page_bench: page_bench::PageBench::from_env(),
            work_bench: work_bench::WorkBench::from_env(),
            thumb_bench: thumb_bench::ThumbBench::from_env(),
            gl_name: gpu::describe(cc),
            updater: crate::update::Updater::start(cc.egui_ctx.clone()),
            show_about: false,
        };
        if let Some(path) = initial {
            app.open(path);
        }
        app
    }

    fn now(ctx: &egui::Context) -> f64 {
        ctx.input(|i| i.time)
    }

    fn entry(&self, uid: u64) -> Option<&HighlightEntry> {
        self.doc.as_ref()?.session.highlight(uid)
    }

    /* -------------------------------------------------------------- *
     * Opening and saving
     * -------------------------------------------------------------- */

    fn open(&mut self, path: PathBuf) {
        self.generation += 1;
        self.status = Status::Opening;
        // Pages are read into shapes from the file itself, so that starts now
        // rather than when the worker has the file open: reading and parsing
        // an 85 MB drawing set takes the reader about 150 ms before it can
        // look at a page, and it needn't wait for the worker to do the same.
        let generation = self.generation;
        self.pending_reader = self
            .gpu
            .as_ref()
            .map(|_| (generation, gpu::Reader::spawn(path.clone(), generation, Arc::clone(&self.wanted), self.ctx.clone(), self.cache.clone())));
        let _ = self.tx.send(Request::Open { generation, path });
    }

    fn pick_and_open(&mut self) {
        self.unless_unsaved(Discarding::Pick);
    }

    fn save(&mut self) {
        let author = self.author_name();
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(changes) = doc.session.begin_save(author) else { return };
        let generation = doc.generation;
        self.status = Status::Saving;
        self.popup = None;
        let _ = self.tx.send(Request::Save { generation, changes });
    }

    fn drain_replies(&mut self, ctx: &egui::Context) {
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Fatal(message) => {
                    self.fatal = Some(message);
                    self.status = Status::Idle;
                }

                Reply::Opened { generation, path, file, page_sizes } if generation == self.generation => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ctx.send_viewport_cmd(ViewportCommand::Title(format!("{name} - Kinetic PDF")));
                    let sizes: Vec<Vec2> = page_sizes.iter().map(|[w, h]| vec2(*w, *h)).collect();
                    if let (Some(gpu), Some(old)) = (&self.gpu, self.doc.take()) {
                        gpu.release(old.drawing, old.uploading);
                    }
                    // Started when the file was opened, unless that was for
                    // another file or the app had no GPU then.
                    let started = self.pending_reader.take().filter(|(started, _)| *started == generation).map(|(_, reader)| reader);
                    let reader = started
                        .or_else(|| self.gpu.as_ref().map(|_| gpu::Reader::spawn(path.clone(), generation, Arc::clone(&self.wanted), ctx.clone(), self.cache.clone())));
                    let thumbs = gpu::Thumbnails::spawn(self.cache.clone(), file, ctx.clone());
                    self.doc = Some(Doc {
                        generation,
                        name,
                        path,
                        file,
                        usual_size: usual_page_size(&sizes),
                        geometry: vec![None; sizes.len()],
                        sizes,
                        session: Session::default(),
                        highlights_done: false,
                        redraw: HashSet::new(),
                        text: HashMap::new(),
                        text_pending: HashSet::new(),
                        textures: HashMap::new(),
                        render_pending: HashSet::new(),
                        tiles: HashMap::new(),
                        tile_full: HashMap::new(),
                        spares: HashMap::new(),
                        predicting: HashMap::new(),
                        detail_pending: HashSet::new(),
                        failed: HashSet::new(),
                        slow: HashSet::new(),
                        drawing: HashMap::new(),
                        reader,
                        uploading: None,
                        thumbnails: HashMap::new(),
                        thumbs_asked: HashMap::new(),
                        thumbs,
                        thumbs_ahead: HashSet::new(),
                        reading: None,
                        slow_to_read: HashSet::new(),
                        shape_sizes: HashMap::new(),
                        releasing: Vec::new(),
                        handing_over: HashSet::new(),
                        left_to_pdfium: HashSet::new(),
                    });
                    // Every thumbnail kept from before, read now rather than as
                    // each page scrolls into view: read as they come, they land
                    // a frame or several after the page, which shows blank until
                    // they do. From the top, where the file opens, and no more
                    // than the thumbnail budget holds.
                    if let Some(doc) = self.doc.as_mut() {
                        if let Some(thumbs) = &doc.thumbs {
                            let [w, h] = gpu::thumbnail_size(doc.usual_size);
                            let most = THUMBNAIL_BUDGET / (w as usize * h as usize * 4).max(1);
                            let now = Self::now(ctx);
                            for page in 0..doc.sizes.len().min(most) {
                                thumbs.want(page);
                                doc.thumbs_asked.insert(page, now);
                            }
                        }
                    }
                    self.active = None;
                    self.drag = None;
                    self.popup = None;
                    self.current_page = 0;
                    // Each document opens fitted to the window, at the top.
                    self.zoom_mode = ZoomMode::FitWidth;
                    self.zoom_anchor = None;
                    self.scroll_x = Some(0.0);
                    self.scroll_y = Some(0.0);
                    self.status = Status::Idle;
                    // Whatever is in the find box gets searched again in the
                    // new document.
                    self.search.sent.clear();
                    self.search.hits.clear();
                    self.search.current = None;
                    self.search.done = true;
                }

                Reply::OpenFailed { generation, error } if generation == self.generation => {
                    self.pending_reader = None;
                    self.status = Status::Idle;
                    self.show_toast_message(ctx, format!("Could not open that PDF: {error}"));
                }

                Reply::Highlights { generation, highlights, markups, geometry, done } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        for (page, g) in geometry {
                            if let Some(slot) = doc.geometry.get_mut(page) {
                                *slot = Some(g);
                            }
                        }
                        // Pages arrive in the order they're viewed, not file
                        // order; the session keeps the notes list in file order.
                        doc.session.load(highlights, markups);
                        doc.highlights_done = done;
                    }
                }

                Reply::Text { generation, page, chars } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        doc.text_pending.remove(&page);
                        doc.text.insert(page, chars);
                    }
                }

                Reply::Rendered { generation, page, scale, texture, complete, slow, annotations } => {
                    // The worker already made the texture; this just keeps it.
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        if slow {
                            doc.slow.insert(page);
                        }
                        let predicted = PredictKey::Page(page, scale.to_bits());
                        if doc.predicting.contains_key(&predicted) {
                            // Drawn ahead of a zoom: kept aside for when it comes.
                            if complete {
                                doc.predicting.remove(&predicted);
                                doc.spares.insert((page, scale.to_bits()), PageTexture { handle: texture, scale, complete, annotations });
                            }
                        } else {
                            // A part-drawn page never replaces a finished image,
                            // such as the one being stretched during a zoom.
                            let finished = doc.textures.get(&page).is_some_and(|t| t.complete);
                            if complete || !finished {
                                if let Some(old) = doc.textures.insert(page, PageTexture { handle: texture, scale, complete, annotations }) {
                                    // Kept, for a zoom back to its size.
                                    if old.complete && (old.scale - scale).abs() > 1e-3 {
                                        doc.spares.insert((page, old.scale.to_bits()), old);
                                    }
                                }
                            }
                            if complete {
                                doc.render_pending.remove(&page);
                                // Pdfium has just kept a thumbnail of it, if it
                                // had none: asked for now, not in a few seconds,
                                // so the page isn't blank when next scrolled to.
                                if annotations && !doc.thumbnails.contains_key(&page) {
                                    if let Some(thumbs) = &doc.thumbs {
                                        doc.thumbs_asked.insert(page, Self::now(ctx));
                                        thumbs.want(page);
                                    }
                                }
                                // Drawn with its annotations, it shows its
                                // markups as saved.
                                if annotations {
                                    doc.redraw.remove(&page);
                                }
                            }
                        }
                    }
                }

                Reply::TextSkipped { generation, page } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        doc.text_pending.remove(&page);
                    }
                }

                Reply::RenderedRegion { generation, page, full, region, annotations, tiles } => {
                    let now = Self::now(ctx);
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        if doc.predicting.remove(&PredictKey::Region(page, region)).is_none() {
                            doc.detail_pending.remove(&page);
                        }
                        for tile in tiles {
                            let key = TileKey { page, full, column: tile.column, row: tile.row, annotations };
                            doc.tiles.insert(key, TileImage { handle: tile.texture, used: now });
                        }
                    }
                }

                Reply::RenderSkipped { generation, page } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        doc.render_pending.remove(&page);
                        doc.predicting.retain(|key, _| !matches!(key, PredictKey::Page(p, _) if *p == page));
                    }
                }

                Reply::RenderFailed { generation, page, error } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        doc.render_pending.remove(&page);
                        doc.failed.insert(page);
                        self.show_toast_message(ctx, format!("Could not draw page {}: {error}", page + 1));
                    }
                }

                Reply::Saved { generation, pages, highlights, markups, redrawn } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        // Only the pages the save touched come back, re-read;
                        // highlights and markups everywhere else are exactly as
                        // they were. The session matches them to what it
                        // already shows, so selection and undo carry on, as
                        // does anything changed while the save ran.
                        if !doc.session.saved(&pages, highlights, markups) {
                            self.active = None;
                            self.popup = None;
                        }
                        if !redrawn.is_empty() {
                            redraw_pages(doc, &redrawn);
                            if let Some(gpu) = &self.gpu {
                                gpu.reread(doc, &redrawn, &self.wanted, ctx, self.cache.clone());
                            }
                        }
                        self.status = Status::Saved { until: Self::now(ctx) + 2.5 };
                    }
                }

                Reply::SaveFailed { generation, error } => {
                    if generation == self.generation {
                        if let Some(doc) = self.doc.as_mut() {
                            doc.session.save_failed();
                        }
                        self.status = Status::Idle;
                        self.show_toast_message(ctx, format!("Save failed: {error}"));
                    }
                }

                Reply::Search { generation, id, searched, hits, done } => {
                    let current_doc = self.doc.as_ref().is_some_and(|d| d.generation == generation);
                    if current_doc && id == self.search.id {
                        self.search.hits.extend(hits);
                        self.search.searched = searched;
                        self.search.done = done;
                        if self.search.current.is_none() {
                            let start = self.search.start_page;
                            let first_ahead = self.search.hits.iter().position(|h| h.page >= start);
                            // Nothing from the starting page on: wrap to the top.
                            let pick = first_ahead.or((done && !self.search.hits.is_empty()).then_some(0));
                            if let Some(i) = pick {
                                self.go_to_hit(i);
                            }
                        }
                    }
                }

                _ => {}
            }
        }
    }

    /* -------------------------------------------------------------- *
     * Window-level input
     * -------------------------------------------------------------- */

    fn handle_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            if self.doc.as_ref().is_some_and(|d| d.session.is_dirty()) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
            self.unless_unsaved(Discarding::Close);
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let (save, open, zoom_in, zoom_out, fit, find, previous, next, go_to) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::O),
                i.consume_key(Modifiers::COMMAND, Key::Plus) | i.consume_key(Modifiers::COMMAND, Key::Equals),
                i.consume_key(Modifiers::COMMAND, Key::Minus),
                i.consume_key(Modifiers::COMMAND, Key::Num0),
                i.consume_key(Modifiers::COMMAND, Key::F),
                // Shift+F3 before F3, which would otherwise match it too.
                i.consume_key(Modifiers::SHIFT, Key::F3),
                i.consume_key(Modifiers::NONE, Key::F3),
                i.consume_key(Modifiers::COMMAND, Key::G),
            )
        });
        // Home and End go to the start and end of the document, with or without
        // Ctrl -- unless something is being typed in, where they move the cursor.
        let typing = ctx.memory(|m| m.focused().is_some());
        let (start, end) = if typing || self.doc.is_none() {
            (false, false)
        } else {
            ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Home) | i.consume_key(Modifiers::COMMAND, Key::Home),
                    i.consume_key(Modifiers::NONE, Key::End) | i.consume_key(Modifiers::COMMAND, Key::End),
                )
            })
        };
        // Ctrl+Z undoes; Ctrl+Y or Ctrl+Shift+Z redoes. While typing, the text
        // box undoes its own typing instead.
        if !typing && self.doc.is_some() {
            let (redo, undo) = ctx.input_mut(|i| {
                // Ctrl+Shift+Z before Ctrl+Z, which would otherwise match it too.
                let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) | i.consume_key(Modifiers::COMMAND, Key::Y);
                (redo, i.consume_key(Modifiers::COMMAND, Key::Z))
            });
            if undo {
                self.undo(false);
            }
            if redo {
                self.undo(true);
            }
        }
        if start {
            self.go_to_page(0);
        }
        if end {
            self.go_to_end();
        }
        // Page Up and Page Down go to the page before or after, at the same
        // place on it, at whatever zoom.
        if !typing && self.doc.is_some() {
            let (up, down) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::PageUp), i.consume_key(Modifiers::NONE, Key::PageDown)));
            if up {
                self.step_page(-1);
            }
            if down {
                self.step_page(1);
            }
        }
        if save {
            self.save();
        }
        if open {
            self.pick_and_open();
        }
        if zoom_in {
            self.zoom_by(1);
        }
        if zoom_out {
            self.zoom_by(-1);
        }
        if fit {
            self.zoom_mode = ZoomMode::FitWidth;
        }
        if find && self.doc.is_some() {
            self.search.focus = true;
        }
        if go_to && self.doc.is_some() {
            self.page_box_focus = true;
        }
        if previous {
            self.step_hit(-1);
        }
        if next {
            self.step_hit(1);
        }

        self.tool_keys(ctx);

        // Ctrl + mouse wheel zooms in on whatever is under the pointer.
        let (pinch, pointer) = ctx.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
        if pinch != 1.0 && self.doc.is_some() {
            self.zoom_mode = ZoomMode::Custom;
            self.change_zoom(self.zoom * pinch, pointer);
        }

        // Dropping a PDF on the window opens it.
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.unless_unsaved(Discarding::Open(path));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_replies(&ctx);
        let taking = std::time::Instant::now();
        self.receive_shapes(&ctx);
        self.scroll_bench(&ctx, taking.elapsed());
        self.zoom_bench(&ctx);
        self.page_bench(&ctx);
        self.work_bench(&ctx);
        self.thumb_bench(&ctx);
        self.handle_close(&ctx);
        self.handle_input(&ctx);
        self.update_search(&ctx);

        self.toolbar(ui);
        self.tool_strip(ui);
        if self.fatal.is_none() {
            match self.sidebar {
                Sidebar::Notes => self.notes_panel(ui),
                Sidebar::Results => self.results_panel(ui),
                Sidebar::None => {}
            }
        }
        egui::CentralPanel::default().frame(Frame::NONE.fill(BG)).show(ui, |ui| self.viewer(ui));

        self.show_popup(&ctx);
        self.show_toast(&ctx);
        self.discard_dialog(&ctx);
        self.about_dialog(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_follow_the_memory_free() {
        const MB: u64 = 1024 * 1024;
        assert_eq!(budgets(u64::MAX), (TILE_BUDGET, SPARE_BUDGET), "no answer from the system means the most");
        assert_eq!(budgets(24 * 1024 * MB), (TILE_BUDGET, SPARE_BUDGET), "plenty free");
        let (tiles, spares) = budgets(3 * 1024 * MB);
        assert_eq!((tiles >> 20, spares >> 20), (256, 170), "3 GB free");
        assert_eq!(budgets(256 * MB), (64 * MB as usize, 32 * MB as usize), "never below the least");
    }

    /// A drawing set: A1 sheets turned landscape, with two much longer sheets.
    fn drawing_set() -> Vec<Vec2> {
        let a1 = vec2(2384.0, 1684.0);
        let long = vec2(8504.0, 2835.0);
        vec![a1, a1, a1, long, a1, a1, long, a1]
    }

    #[test]
    fn drawing_scales_step_in_quarter_powers_of_two() {
        let step = 2f32.powf(0.25);
        assert_eq!(quantize_scale(1.0), 1.0);
        assert_eq!(quantize_scale(2.0), 2.0);
        assert!((quantize_scale(1.01) - step).abs() < 1e-4);
        // A step stays put when quantized again.
        assert_eq!(quantize_scale(quantize_scale(0.8)), quantize_scale(0.8));
        // Nearby scales share one image, so a slightly wider window reuses it.
        assert_eq!(quantize_scale(1.02), quantize_scale(1.15));
        let q = quantize_scale(0.8);
        assert!(q >= 0.8 && q < 0.8 * step + 1e-4, "{q}");
    }

    #[test]
    fn loading_ahead_goes_the_way_the_view_is_heading_first() {
        // Pages 5 and 6 in view out of 20.
        assert_eq!(pages_ahead(5, 6, 20, true, 3), vec![7, 8, 4]);
        assert_eq!(pages_ahead(5, 6, 20, false, 3), vec![4, 3, 7]);
        // One page left below: it goes first, then back up.
        assert_eq!(pages_ahead(17, 18, 20, true, 3), vec![19, 16, 15]);
    }

    #[test]
    fn the_page_under_a_height_is_the_nearest_across_a_gap() {
        let sizes = vec![vec2(100.0, 200.0); 3];
        let layout = page_layout(&sizes, vec2(100.0, 200.0), 1.0, false);
        let [first, second, _] = [layout.tops[0], layout.tops[1], layout.tops[2]];
        assert_eq!(page_under(&layout, &sizes, 0.0), Some(0), "above the first page");
        assert_eq!(page_under(&layout, &sizes, first + 100.0), Some(0));
        assert_eq!(page_under(&layout, &sizes, first + 202.0), Some(0), "just below it");
        assert_eq!(page_under(&layout, &sizes, second - 2.0), Some(1), "just above the next");
        assert_eq!(page_under(&layout, &sizes, 1e6), Some(2), "past the end");
        assert_eq!(page_under(&page_layout(&[], vec2(1.0, 1.0), 1.0, false), &[], 5.0), None);
    }

    #[test]
    fn at_either_end_loading_ahead_works_back_from_the_view() {
        assert_eq!(pages_ahead(18, 19, 20, true, 3), vec![17, 16, 15]);
        assert_eq!(pages_ahead(0, 1, 20, false, 3), vec![2, 3, 4]);
        // A document smaller than the look-ahead.
        assert_eq!(pages_ahead(0, 0, 2, true, 3), vec![1]);
        assert_eq!(pages_ahead(0, 0, 1, true, 3), Vec::<usize>::new());
    }

    #[test]
    fn the_usual_size_is_what_most_pages_share() {
        assert_eq!(usual_page_size(&drawing_set()), vec2(2384.0, 1684.0));
        assert_eq!(usual_page_size(&[]), vec2(612.0, 792.0));
    }

    #[test]
    fn oversized_pages_shrink_to_the_usual_width() {
        let sizes = drawing_set();
        let layout = page_layout(&sizes, usual_page_size(&sizes), 0.5, true);
        for (size, scale) in sizes.iter().zip(&layout.scales) {
            assert!((size.x * scale - 2384.0 * 0.5).abs() < 0.01, "{size:?} drawn {} wide", size.x * scale);
        }
        assert!((layout.widest - 1192.0).abs() < 0.01);
    }

    #[test]
    fn without_shrinking_oversized_pages_keep_their_size() {
        let sizes = drawing_set();
        let layout = page_layout(&sizes, usual_page_size(&sizes), 0.5, false);
        assert!(layout.scales.iter().all(|s| (*s - 0.5).abs() < 1e-6));
        assert!((layout.widest - 4252.0).abs() < 0.01);
    }

    #[test]
    fn pages_stack_with_a_gap() {
        let sizes = vec![vec2(100.0, 200.0), vec2(100.0, 300.0)];
        let layout = page_layout(&sizes, vec2(100.0, 200.0), 1.0, true);
        assert_eq!(layout.tops, vec![TOP_PAD, TOP_PAD + 200.0 + PAGE_GAP]);
        assert_eq!(layout.height, TOP_PAD + 200.0 + PAGE_GAP + 300.0 + BOTTOM_PAD);
    }

    #[test]
    fn narrow_pages_centre_and_wide_ones_start_at_the_margin() {
        assert_eq!(page_x(content_width(1000.0, 400.0), 400.0), 300.0);
        let wide = content_width(1000.0, 3000.0);
        assert_eq!(wide, 3000.0 + 2.0 * SIDE_PAD);
        assert_eq!(page_x(wide, 3000.0), SIDE_PAD);
    }

    #[test]
    fn render_size_is_capped_for_huge_pages() {
        let long = vec2(8504.0, 2835.0);
        let scale = render_scale(long, 4.0, 2.0, 16384.0);
        assert!(long.x * scale * long.y * scale <= MAX_PAGE_PIXELS * 1.001);
    }
}
