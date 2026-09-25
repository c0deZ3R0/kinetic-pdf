//! The window: the menu bar, the tool strip, the page viewer, the panel down
//! the left and the quantities along the bottom, and the highlight popup.
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
mod arrange;
mod context;
mod discard;
mod drag;
mod gpu;
mod icons;

mod layout;
mod line_style;
mod markups;
mod notes;
mod pages;
mod picked;
mod palette;
mod prefs;
mod quantities;
mod measure;
mod scale;
mod status_bar;
mod scroll_bench;
mod search;
mod style;
mod toolbar;
mod tool_panel;
mod tools;
mod widgets;
mod page_bench;
mod work_bench;
mod thumb_bench;
mod zoom_bench;

pub use layout::{quantize_scale, render_scale};
use arrange::{SheetAction, SheetDrag};
use discard::*;
use drag::*;
use layout::*;
use markups::*;
use notes::*;
use pages::*;
use palette::Palette;
use picked::Picked;
use measure::*;
use quantities::{CellClick, Edit, RowId, Sort};
use scale::*;
use markup_model::{MarkupId, Snap};
use style::*;
use icons::Icon;
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
    /// The file, to read again after a save changes how its pages are drawn.
    path: PathBuf,
    /// The fingerprint of its contents, which keys what the page cache keeps
    /// for it.
    file: u64,
    /// Page sizes in points, as displayed (rotated).
    sizes: Vec<Vec2>,
    /// What the file calls each page -- the sheet name, in a drawing set.
    /// Empty where the file names nothing, which many do.
    labels: Vec<Option<String>>,
    /// The size most pages share; fit-to-width and shrinking work from it.
    usual_size: Vec2,
    /// Each page's rotation and visible box, once the worker has read it.
    /// Nothing is drawn over a page, and it can't be selected, until then.
    geometry: Vec<Option<PageGeometry>>,
    /// The order the sheets are in and what the user has picked out: just the
    /// pages of the file, in the file's own order, until something is done to
    /// it. See `crate::arrange` and `app/arrange.rs`.
    arrange: crate::arrange::Arrangement,
    /// The highlights and markups, every change to them, and what's unsaved.
    session: Session,
    /// Whether the file's scales and measurements have been read; they are
    /// only read when something needs them (see scale.rs).
    measurements: MeasureRead,
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
    /// Thumbnails the GPU is drawing, collected once it has.
    thumbnails_drawing: Vec<gpu::DrawingThumbnail>,
    /// Squares heavy pages on the GPU are drawn into (`gpu::Tiles`).
    gpu_tiles: gpu::Tiles,
    /// A small image of each page seen, shown while what draws it properly is
    /// on its way, and kept in the page cache between sessions.
    thumbnails: HashMap<usize, Thumbnail>,
    /// Visible sheets held across a save refresh until fresh drawings arrive.
    /// These are display-only, with explicit turns; never put in the cache.
    save_previews: HashMap<usize, (TextureHandle, u8)>,
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
    /// The lines of each page read for snapping, kept under `SNAP_BUDGET`
    /// for the pages near the view.
    snap: HashMap<usize, Arc<markup_model::SnapIndex>>,
    /// Pages whose lines have been asked for, so they are asked once.
    snap_asked: HashSet<usize>,
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

impl Doc {
    /* ------------------------------------------------------------------ *
     * Sheets and the pages they show
     *
     * The column the user scrolls is the arrangement's sheets; everything
     * kept about a page -- its texture, its squares, its thumbnail, its text,
     * its shapes -- is kept against the page of the *file*, so two sheets
     * showing one page share all of it and taking a sheet out throws none of
     * it away. These three are the crossing between the two, and they are the
     * only place that crossing is made.
     * ------------------------------------------------------------------ */

    /// The page of the file sheet `at` shows, if it shows one: a blank sheet
    /// shows no page of the file at all.
    fn sheet_page(&self, at: usize) -> Option<usize> {
        self.arrange.page_of(at)
    }

    /// Quarter-turns clockwise the user has turned sheet `at` through, on top
    /// of however the file already has its page.
    fn sheet_turns(&self, at: usize) -> u8 {
        self.arrange.sheets().get(at).map_or(0, |sheet| sheet.turns())
    }

    /// Where page `page` of the file is shown in the column, if it still is.
    /// A page can be shown by more than one sheet, once it has been
    /// duplicated; the first is the one anything going to a page aims at.
    fn first_sheet_showing(&self, page: usize) -> Option<usize> {
        self.arrange.sheets().iter().position(|sheet| sheet.page() == Some(page))
    }

    /// How sheet `at`'s user space maps onto it as displayed -- the geometry
    /// of the page it shows, turned by whatever the user has turned the sheet
    /// through. Everything drawn over a sheet goes through this, which is why
    /// turning a sheet brings its markups and measurements round with it.
    fn sheet_geometry(&self, at: usize) -> Option<PageGeometry> {
        let page = self.sheet_page(at)?;
        let geometry = self.geometry.get(page).copied().flatten()?;
        Some(geometry.turned(self.sheet_turns(at)))
    }
}

/// Where every page sits in the scrolling column at the current zoom.
struct PageLayout {
    lefts: Vec<f32>,
    columns: usize,
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
}

/// A selection being dragged out.
/// A drag in progress.
///
/// Every one of these is a thing the user is doing to a place on screen, so
/// each names the *sheet* it is happening on -- where it is in the column --
/// not the page of the file that sheet shows. The page is looked up from the
/// sheet wherever what is being dragged has to be stored against one.
enum Drag {
    /// Following the text, as (sheet, caret) at each end.
    Text { anchor: (usize, usize), focus: (usize, usize) },
    /// With Ctrl held: a box on one sheet, its corners in PDF user space.
    /// Everything whose centre is inside it is selected.
    Box { sheet: usize, start: (f32, f32), end: (f32, f32) },
    /// The Select tool's box, its corners in PDF user space: what it catches
    /// is picked out when it is let go, as well as what was with `adding`.
    /// See `picked.rs`.
    Pick { sheet: usize, start: (f32, f32), end: (f32, f32), adding: bool },
    /// Moving everything picked out, from where the pointer was last.
    MovePicked { sheet: usize, from: (f32, f32) },
    /// Moving a vertex of a measurement.
    MeasureVertex { id: MarkupId, ring: usize, index: usize, sheet: usize },
    /// Setting or checking a page's scale: a line along a known dimension.
    /// `placed` once the first end was put down by a click rather than held
    /// down, so the line follows the pointer until the second click.
    Calibrate { sheet: usize, from: (f32, f32), to: (f32, f32), placed: bool },
    /// Dragging the first point of an area or cutout draws a rectangle.
    AreaRectangle { sheet: usize, start: (f32, f32), end: (f32, f32) },
    /// With a drawing tool: the markup being drawn, and the sheet it started
    /// on. The markup itself names the page of the file it belongs to; the
    /// sheet is where on screen the pointer is being followed.
    Markup { markup: Markup, sheet: usize },
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
    fit_requested: bool,
    /// Show oversized pages at the usual page width rather than actual size.
    shrink_wide: bool,
    side_by_side: bool,
    /// Multiplier for wheel scrolling in the document view. Remembered
    /// between runs, with `zoom_speed`: see `prefs.rs`.
    scroll_speed: f32,
    /// Multiplier for Ctrl-wheel and pinch zoom steps in logarithmic space.
    zoom_speed: f32,
    insert_sheet: Option<arrange::InsertSheetDialog>,
    zoom_anchor: Option<ZoomAnchor>,
    status: Status,
    toast: Option<(String, f64)>,
    author: String,
    active: Option<u64>,
    drag: Option<Drag>,
    /// The scale tool in use, if any.
    measure_tool: Option<MeasureTool>,
    /// The measurement being placed, click by click.
    placing: Option<Placing>,
    /// The measurement picked out, if any, and which of its corners.
    active_vertex: Option<(usize, usize)>,
    active_measure: Option<MarkupId>,
    /// The quantities table across the bottom: whether it's open, and whether
    /// it is showing this page alone.
    quantities_open: bool,
    /// Which column it is sorted by, if any, and the cell open for typing.
    quantity_sort: Option<Sort>,
    quantity_edit: Option<Edit>,
    /// The last click on a cell that opens for typing, to pair with the next.
    quantity_click: Option<CellClick>,
    /// What was picked out the last time the table looked, so it can tell
    /// when the page picks out something else, and the lines it had wholly
    /// in view.
    quantity_seen: Vec<RowId>,
    quantity_in_view: std::ops::Range<usize>,
    /// The groups rolled up to their headings, by name. Only for as long as
    /// the table is gathered the same way.
    quantity_collapsed: HashSet<String>,
    /// What is picked out beside `active` or `active_measure`: see picked.rs.
    picked: Picked,
    /// What the pointer would snap to, worked out as the pages are drawn.
    snap: Option<Snap>,
    /// The dialog asking what a calibration line really measures.
    scale_dialog: Option<ScaleDialog>,
    /// The drawing tool in use, or `None` for the Select tool or the
    /// highlighter.
    tool: Option<MarkupKind>,
    /// Whether the highlighter is in hand: a drag follows the text under it,
    /// and with Ctrl held draws a box round it, to highlight what it covers.
    /// Only while no drawing or measurement tool is; see `highlighting`.
    highlighter: bool,
    /// The colour and stroke width, in points, new markups take.
    markup_color: Rgb,
    markup_width: f32,
    /// Memory allowed for squares and for spare page images; see `budgets`.
    tile_budget: usize,
    spare_budget: usize,
    popup: Option<Popup>,
    /// The command palette (Ctrl+Shift+P) and what has been typed into it.
    palette: Palette,
    tool_creator: Option<tool_panel::ToolCreator>,
    search: Search,
    /// Scroll the page view here next frame.
    scroll_x: Option<f32>,
    scroll_y: Option<f32>,
    /// Put a newly opened document at its ordinary centred starting position
    /// once the viewport and its overscroll canvas are known.
    rest_view: bool,
    /// The page view's scroll offset last frame.
    scroll_offset: Vec2,
    /// Where the page view's content started on screen last frame.
    content_origin: Pos2,
    /// Screen rectangles of the pages drawn this frame.
    page_rects: HashMap<usize, Rect>,
    viewer_rect: Rect,
    /// The sheet in view, by where it sits in the column -- not the page of
    /// the file it shows. The page box, the scale panel and everything else
    /// that says "the page you are on" means this one; where the file's own
    /// page is wanted, it comes from `Doc::sheet_page`.
    current_page: usize,
    /// Whether the save under way is also writing a new page order. When it
    /// lands the pages have moved, so the document is opened again: everything
    /// held against where a page used to be has to be read afresh.
    rearranged_on_save: bool,
    /// A saved document is being refreshed, preserving the live viewport.
    refreshing_save: Option<u64>,
    /// A page pressed on, which is the page being worked on until it is
    /// scrolled out of sight. Pressing a sheet says which one you mean far
    /// more plainly than where the column happens to be scrolled to.
    picked_page: Option<usize>,
    /// Sheets being dragged into a new place, in the sheet view.
    sheet_drag: Option<SheetDrag>,
    /// What each tool is set to. Read from disk once at startup.
    tools: tools::Tools,
    /// What the last right-click landed on, kept while its menu is open: the
    /// pointer leaves whatever was clicked as it moves onto the menu.
    context_target: Option<(usize, context::Target)>,
    /// Which tab of the details panel is showing.
    tool_tab: tool_panel::Tab,
    /// The name and group being typed when keeping a tool.
    /// Whether the details panel is open. It stays open once something has
    /// been in it, blank between one thing and the next: a panel that came
    /// and went as measurements were picked and let go moved everything else
    /// on screen each time.
    tool_panel_open: bool,
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
        let prefs = prefs::Prefs::load();
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
            fit_requested: false,
            shrink_wide: true,
            side_by_side: false,
            scroll_speed: prefs.scroll_speed,
            zoom_speed: prefs.zoom_speed,
            insert_sheet: None,
            zoom_anchor: None,
            status: Status::Idle,
            toast: None,
            author: load_author(),
            active: None,
            drag: None,
            tool: None,
            highlighter: false,
            measure_tool: None,
            scale_dialog: None,
            snap: None,
            placing: None,
            active_measure: None,
            quantities_open: false,
            quantity_sort: None,
            quantity_edit: None,
            quantity_click: None,
            quantity_seen: Vec::new(),
            quantity_in_view: 0..0,
            quantity_collapsed: HashSet::new(),
            picked: Picked::default(),
            active_vertex: None,
            markup_color: MARKUP_COLORS[0].1,
            markup_width: WIDTHS[1].1,
            tile_budget,
            spare_budget,
            popup: None,
            palette: Palette::default(),
            tool_creator: None,
            search: Search::default(),
            scroll_x: None,
            scroll_y: None,
            rest_view: false,
            scroll_offset: Vec2::ZERO,
            content_origin: Pos2::ZERO,
            page_rects: HashMap::new(),
            viewer_rect: Rect::NOTHING,
            current_page: 0,
            rearranged_on_save: false,
            refreshing_save: None,
            picked_page: None,
            sheet_drag: None,
            tools: tools::Tools::load(),
            tool_panel_open: false,
            context_target: None,
            tool_tab: tool_panel::Tab::default(),
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
        self.insert_sheet = None;
        self.refreshing_save = None;
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

    /// Whether there is work the file does not yet have: markups, highlights
    /// and scales in the session, or a sheet order the user has changed.
    ///
    /// The order counts as unsaved work like anything else, so the save button
    /// lights up for it, closing asks about it, and Ctrl+S puts it down. There
    /// is no separate "apply": a rearranged document is a changed document.
    fn has_unsaved_work(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.session.is_dirty() || d.arrange.edited())
    }

    /// The page of the file the sheet in view shows, for the callers that
    /// need the file's own page rather than the place in the column -- the
    /// scale panel, the sheet name, the search. A blank sheet shows none.
    fn current_file_page(&self) -> Option<usize> {
        self.doc.as_ref()?.sheet_page(self.current_page)
    }

    fn save(&mut self) {
        if matches!(self.status, Status::Saving | Status::Opening) {
            return;
        }
        let author = self.author_name();
        let Some(doc) = self.doc.as_mut() else { return };
        // The sheets go with the save when the user has changed their order,
        // so one Ctrl+S puts down the markups and the arrangement together.
        // The annotations are written first and the pages moved afterwards,
        // which carries each page's annotations along with it.
        let arrangement = doc.arrange.edited().then(|| doc.arrange.sheets().to_vec());
        let Some(changes) = doc.session.begin_save(author).or_else(|| {
            // Nothing in the session changed, but the order did: still a save.
            arrangement.is_some().then(crate::model::Changes::default)
        }) else {
            return;
        };
        let generation = doc.generation;
        self.rearranged_on_save = arrangement.is_some();
        self.status = Status::Saving;
        self.popup = None;
        let _ = self.tx.send(Request::Save { generation, changes, arrangement });
    }

    fn drain_replies(&mut self, ctx: &egui::Context) {
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Fatal(message) => {
                    self.fatal = Some(message);
                    self.status = Status::Idle;
                }

                Reply::Opened { generation, path, file, page_sizes, page_labels } if generation == self.generation => {
                    let refreshed_save = self.refreshing_save.take() == Some(generation);
                    // Fit modes and oversized-page scaling use this reference.
                    // Recomputing it after rotations would change the layout
                    // even if zoom and scroll were left alone.
                    let previous_usual = if refreshed_save { self.doc.as_ref().map(|d| d.usual_size) } else { None };
                    let mut save_previews = HashMap::new();
                    if refreshed_save {
                        if let Some(doc) = &self.doc {
                            for &sheet in self.page_rects.keys() {
                                let Some(page) = doc.sheet_page(sheet) else { continue };
                                let handle = doc.textures.get(&page).map(|t| &t.handle)
                                    .or_else(|| doc.thumbnails.get(&page).map(|t| &t.handle));
                                if let Some(handle) = handle {
                                    save_previews.insert(sheet, (handle.clone(), doc.sheet_turns(sheet)));
                                }
                            }
                        }
                    }
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ctx.send_viewport_cmd(ViewportCommand::Title(format!("{name} - Kinetic PDF")));
                    let sizes: Vec<Vec2> = page_sizes.iter().map(|[w, h]| vec2(*w, *h)).collect();
                    if let (Some(gpu), Some(old)) = (&self.gpu, self.doc.take()) {
                        gpu.release(old.drawing, old.uploading, old.thumbnails_drawing, old.gpu_tiles);
                    }
                    // Started when the file was opened, unless that was for
                    // another file or the app had no GPU then.
                    let started = self.pending_reader.take().filter(|(started, _)| *started == generation).map(|(_, reader)| reader);
                    let reader = started
                        .or_else(|| self.gpu.as_ref().map(|_| gpu::Reader::spawn(path.clone(), generation, Arc::clone(&self.wanted), ctx.clone(), self.cache.clone())));
                    let thumbs = gpu::Thumbnails::spawn(self.cache.clone(), file, ctx.clone());
                    self.doc = Some(Doc {
                        generation,
                        path,
                        file,
                        usual_size: previous_usual.unwrap_or_else(|| usual_page_size(&sizes)),
                        geometry: vec![None; sizes.len()],
                        labels: page_labels,
                        sizes,
                        arrange: crate::arrange::Arrangement::new(page_sizes.len()),
                        session: Session::default(),
                        measurements: MeasureRead::default(),
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
                        thumbnails_drawing: Vec::new(),
                        gpu_tiles: gpu::Tiles::default(),
                        thumbnails: HashMap::new(),
                        save_previews,
                        thumbs_asked: HashMap::new(),
                        thumbs,
                        thumbs_ahead: HashSet::new(),
                        reading: None,
                        slow_to_read: HashSet::new(),
                        snap: HashMap::new(),
                        snap_asked: HashSet::new(),
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
                    if refreshed_save {
                        // The saved sheets have the same displayed positions.
                        // Keep the latest viewport, not a snapshot from Ctrl+S:
                        // the user may have kept scrolling during the save.
                        self.saved_notice(ctx);
                    } else {
                        self.current_page = 0;
                        self.picked_page = None;
                        self.request_fit(ZoomMode::FitWidth);
                        self.zoom_anchor = None;
                        self.scroll_x = Some(0.0);
                        self.scroll_y = Some(0.0);
                        self.rest_view = true;
                        self.status = Status::Idle;
                    }
                    // Whatever is in the find box gets searched again in the
                    // new document.
                    self.search.sent.clear();
                    self.search.hits.clear();
                    self.search.current = None;
                    self.search.done = true;
                }

                Reply::OpenFailed { generation, error } if generation == self.generation => {
                    self.refreshing_save = None;
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
                    if !self.doc.as_ref().is_some_and(|d| d.generation == generation) {
                        continue;
                    }
                    if std::mem::take(&mut self.rearranged_on_save) {
                        if let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) {
                            self.open(path);
                            self.refreshing_save = Some(self.generation);
                            self.status = Status::Saving;
                        }
                        continue;
                    }
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
                        self.saved_notice(ctx);
                    }
                }

                Reply::Measured { generation, measurements } if self.doc.as_ref().is_some_and(|d| d.generation == generation) => {
                    let skipped = measurements.skipped.len();
                    if let Some(doc) = self.doc.as_mut() {
                        doc.session.load_scales(measurements.scales);
                        doc.session.load_measures(measurements.markups);
                        doc.measurements = MeasureRead::Ready;
                    }
                    // Only ever our own markups and viewports: another
                    // program's are passed over, not skipped, so this counts
                    // things that really were written here and are damaged.
                    if skipped > 0 {
                        let what = if skipped == 1 { "one of this file's markups or scales" } else { "markups or scales in this file" };
                        let count = if skipped == 1 { String::new() } else { format!("{skipped} ") };
                        self.show_toast_message(ctx, format!("{count}{what} could not be read"));
                    }
                }

                Reply::MeasureFailed { generation, error } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        doc.measurements = MeasureRead::Failed(error);
                    }
                }

                Reply::SaveFailed { generation, error } => {
                    if generation == self.generation {
                        self.rearranged_on_save = false;
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
            if self.has_unsaved_work() {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
            self.unless_unsaved(Discarding::Close);
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        if self.insert_sheet.is_some() || self.tool_creator.is_some() {
            return;
        }
        // The palette answers first, and keeps the keyboard while it is up:
        // what is typed into it is the name of a command, not a tool letter.
        if self.palette_keys(ctx) {
            return;
        }
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
            // While a shape is being drawn, these take its last point back and
            // put it down again, rather than undoing the change before it.
            if undo {
                self.undo_step(false);
            }
            if redo {
                self.undo_step(true);
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
            self.request_fit(ZoomMode::FitWidth);
        }
        if find && self.doc.is_some() {
            // The find box lives on the panel's find side, so Ctrl+F opens
            // that side before asking for the keyboard.
            self.show_tool_panel(tool_panel::Tab::Find);
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

        // Pulled back to sort sheets, the keys are about sheets: Delete takes
        // them out rather than taking out a measurement, and the drawing tools
        // have nothing to draw on at that size.
        if self.sheet_mode() {
            self.sheet_keys(ctx);
        } else {
            self.measure_keys(ctx);
            self.tool_keys(ctx);
        }

        // Ctrl + mouse wheel zooms in on whatever is under the pointer.
        let (pinch, pointer) = ctx.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
        if pinch != 1.0 && self.doc.is_some() {
            self.zoom_mode = ZoomMode::Custom;
            self.change_zoom(self.zoom * pinch.powf(self.zoom_speed), pointer);
        }

        // Dropping a PDF on the window opens it.
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.unless_unsaved(Discarding::Open(path));
        }
    }
}

/// Whether frames wait for the display, as they do unless `KINETIC_PDF_VSYNC=0`:
/// the benchmarks turn it off to see what the frame rate was hiding.
pub fn vsync() -> bool {
    static VSYNC: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *VSYNC.get_or_init(|| std::env::var_os("KINETIC_PDF_VSYNC").is_none_or(|v| v != "0"))
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_replies(&ctx);
        self.settle_picked();
        self.handle_close(&ctx);
        // Answer window shortcuts before deciding whether to prepare another
        // background thumbnail in the page view.
        self.handle_input(&ctx);
        let taking = std::time::Instant::now();
        self.receive_shapes(&ctx);
        self.scroll_bench(&ctx, taking.elapsed());
        self.zoom_bench(&ctx);
        self.page_bench(&ctx);
        self.work_bench(&ctx);
        self.thumb_bench(&ctx);
        self.update_search(&ctx);

        self.toolbar(ui);
        self.tool_strip(ui);
        self.tools.flush();
        if self.fatal.is_none() {
            // The quantities are a table across the whole bottom of the
            // window, under the pages and under the side panels alike, so it
            // takes its strip before they claim their columns.
            if self.quantities_open {
                self.quantities_dock(ui);
            } else {
                // Picking something out while the table is shut is not a
                // reason to move it once it opens.
                self.quantity_seen = self.picked_rows();
            }
            // The thin bar of zoom and page controls rides on top of the
            // quantities, and along the bottom of the window without them.
            self.status_bar(ui);
            // Down the left, beside whatever is open on the right.
            self.tool_rail(ui);
            self.tool_panel(ui);
        }
        egui::CentralPanel::default().frame(Frame::NONE.fill(BG)).show(ui, |ui| self.viewer(ui));

        self.show_popup(&ctx);
        self.show_scale_dialog(&ctx);
        self.show_insert_sheet_dialog(&ctx);
        self.show_toast(&ctx);
        self.discard_dialog(&ctx);
        self.about_dialog(&ctx);
        self.show_palette(&ctx);
        self.show_tool_creator(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_arrangement_refresh_keeps_the_live_view_and_shows_a_toast() {
        // Keep the App's background services local and replace the worker
        // channels with deterministic save replies.
        std::env::set_var("KINETIC_PDF_CACHE", "0");
        std::env::set_var("KINETIC_PDF_HELPERS", "0");
        std::env::set_var("KINETIC_PDF_UPDATE", "0");
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, None);
        let (replies, rx) = std::sync::mpsc::channel();
        let (tx, requests) = std::sync::mpsc::channel();
        app.rx = rx;
        app.tx = tx;
        app.generation = 1;
        let opened = |generation, sizes| Reply::Opened {
            generation, path: PathBuf::from("save-view-test.pdf"), file: generation,
            page_sizes: sizes, page_labels: vec![None; 3],
        };
        replies.send(opened(1, vec![[600.0, 800.0]; 3])).unwrap();
        app.drain_replies(&ctx);
        let doc = app.doc.as_mut().unwrap();
        doc.arrange.select_all();
        doc.arrange.rotate(1);
        let handle = ctx.load_texture("save-preview", egui::ColorImage::new([2, 3], vec![Color32::WHITE; 6]), Default::default());
        doc.thumbnails.insert(1, Thumbnail { handle, used: 0.0 });
        app.page_rects.insert(1, Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0)));
        app.zoom = 0.2;
        app.zoom_mode = ZoomMode::Custom;
        app.current_page = 1;
        app.scroll_offset = vec2(12.0, 300.0);
        app.scroll_x = None;
        app.scroll_y = None;
        app.save();
        assert!(matches!(requests.recv().unwrap(), Request::Save { arrangement: Some(_), .. }));
        replies.send(Reply::Saved { generation: 1, pages: vec![], highlights: vec![], markups: vec![], redrawn: vec![] }).unwrap();
        app.drain_replies(&ctx);
        assert!(matches!(requests.recv().unwrap(), Request::Open { generation: 2, .. }));
        // The user keeps navigating while the refreshed metadata is in flight.
        app.zoom = 0.73;
        app.current_page = 2;
        app.scroll_offset = vec2(24.0, 650.0);
        app.scroll_y = Some(675.0);
        let before = app.layout(app.doc.as_ref().unwrap());
        replies.send(opened(2, vec![[800.0, 600.0]; 3])).unwrap();
        app.drain_replies(&ctx);
        assert_eq!(app.zoom, 0.73);
        assert!(app.zoom_mode == ZoomMode::Custom);
        assert_eq!(app.current_page, 2);
        assert_eq!(app.scroll_offset, vec2(24.0, 650.0));
        assert_eq!(app.scroll_y, Some(675.0));
        assert_eq!(app.scroll_x, None);
        let doc = app.doc.as_ref().unwrap();
        let after = app.layout(doc);
        assert_eq!(before.tops, after.tops);
        assert_eq!(before.scales, after.scales);
        assert_eq!(before.widest, after.widest);
        assert_eq!(doc.save_previews[&1].1, 1);
        assert!(!doc.arrange.edited());
        assert!(matches!(app.status, Status::Idle));
        assert_eq!(app.toast.as_ref().unwrap().0, "Saved");
        assert_eq!(app.toast.as_ref().unwrap().1, App::now(&ctx) + 2.0);
        // Fit the actual rotated spread, including its gutter, even though
        // the saved document retains the old usual page size.
        app.side_by_side = true;
        app.shrink_wide = false;
        app.request_fit(ZoomMode::FitWidth);
        let view = vec2(1200.0, 900.0);
        app.zoom = app.fit_zoom(app.doc.as_ref().unwrap(), view).unwrap();
        let spread = app.layout_for_view(app.doc.as_ref().unwrap(), view);
        let canvas = content_width(view.x, spread.widest);
        let offset = resting_scroll_x(view.x, spread.widest);
        assert!((spread.x(canvas, 0) - offset - SIDE_PAD).abs() < 0.01);
        let right = spread.x(canvas, 1) + 800.0 * spread.scales[1] - offset;
        assert!((right - (view.x - SIDE_PAD)).abs() < 0.01);
        // A genuinely different document still opens fitted at its beginning.
        app.open(PathBuf::from("another.pdf"));
        replies.send(opened(3, vec![[600.0, 800.0]; 3])).unwrap();
        app.drain_replies(&ctx);
        assert!(app.zoom_mode == ZoomMode::FitWidth);
        assert_eq!(app.current_page, 0);
        assert_eq!(app.scroll_y, Some(0.0));
    }

    /// An app with no window and no worker behind it, with a three-page
    /// document open: what the tests of tools and picking out need.
    pub(super) fn app_with_a_document() -> (App, egui::Context) {
        std::env::set_var("KINETIC_PDF_CACHE", "0");
        std::env::set_var("KINETIC_PDF_HELPERS", "0");
        std::env::set_var("KINETIC_PDF_UPDATE", "0");
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, None);
        let (replies, rx) = std::sync::mpsc::channel();
        let (tx, _) = std::sync::mpsc::channel();
        app.rx = rx;
        app.tx = tx;
        app.generation = 1;
        replies
            .send(Reply::Opened {
                generation: 1,
                path: PathBuf::from("tools-test.pdf"),
                file: 1,
                page_sizes: vec![[600.0, 800.0]; 3],
                page_labels: vec![None; 3],
            })
            .unwrap();
        app.drain_replies(&ctx);
        (app, ctx)
    }

    /// Presses `key` for one frame and lets the tool keys answer it.
    fn press(app: &mut App, ctx: &egui::Context, key: Key) {
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        let mut output = ctx.run_ui(input, |ui| app.tool_keys(ui.ctx()));
        // The first frame carries the font atlas, which a debug build won't
        // let go unapplied.
        output.textures_delta.clear();
    }

    /// The highlighter is a tool of its own, taken up and put down like the
    /// others, and the Select tool is what is left in hand when none is.
    #[test]
    fn the_highlighter_is_a_tool_and_select_is_what_is_left_without_one() {
        let (mut app, ctx) = app_with_a_document();
        assert!(app.selecting() && !app.highlighting(), "the Select tool is in hand to begin with");

        press(&mut app, &ctx, Key::H);
        assert!(app.highlighting() && !app.selecting());
        press(&mut app, &ctx, Key::R);
        assert!(!app.highlighting(), "taking up a drawing tool puts the highlighter down");
        assert_eq!(app.tool, Some(MarkupKind::Rectangle));

        app.run_action(palette::Action::Highlighter);
        assert!(app.highlighting() && app.tool.is_none());
        app.run_action(palette::Action::Measure(MeasureTool::Length));
        assert!(!app.highlighting(), "and so does a measurement tool");

        app.run_action(palette::Action::Highlighter);
        press(&mut app, &ctx, Key::Escape);
        assert!(app.selecting(), "Esc goes back to the Select tool");
    }

    /// Only the highlighter picks out text. With nothing in hand a drag is
    /// the Select tool's, and leaves the text alone.
    #[test]
    fn only_the_highlighter_drags_across_text() {
        let (mut app, _) = app_with_a_document();
        assert_eq!(app.drag_starts(false), DragStart::Select);
        assert_eq!(app.drag_starts(true), DragStart::Select, "Ctrl no longer draws a box round text");

        app.take_up_highlighter();
        assert_eq!(app.drag_starts(false), DragStart::FollowText);
        assert_eq!(app.drag_starts(true), DragStart::TextBox);

        app.take_up_drawing(MarkupKind::Pen);
        assert_eq!(app.drag_starts(false), DragStart::Draw);
        app.set_measure_tool(Some(MeasureTool::Area));
        assert_eq!(app.drag_starts(false), DragStart::Nothing);
    }

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
    fn pages_rest_in_the_middle_and_can_be_pulled_to_either_side() {
        let view = 1000.0;
        let canvas = content_width(view, 400.0);
        let page = page_x(canvas, 400.0);
        let rest = resting_scroll_x(view, 400.0);
        assert_eq!(page - rest, 300.0);
        assert_eq!(page, view - SIDE_PAD);

        let canvas = content_width(view, 3000.0);
        let page = page_x(canvas, 3000.0);
        let far_right = canvas - view;
        assert_eq!(page, view - SIDE_PAD);
        assert_eq!(page + 3000.0 - far_right, SIDE_PAD);
    }

    #[test]
    fn zoom_preserves_the_cursor_fraction_inside_and_outside_paper() {
        assert_eq!(zoom_axis(200.0, 100.0, 300.0), (0.5, 200.0));
        assert_eq!(zoom_axis(50.0, 100.0, 300.0), (-0.25, 50.0));
        assert_eq!(zoom_axis(350.0, 100.0, 300.0), (1.25, 350.0));
    }

    #[test]
    fn render_size_is_capped_for_huge_pages() {
        let long = vec2(8504.0, 2835.0);
        let scale = render_scale(long, 4.0, 2.0, 16384.0);
        assert!(long.x * scale * long.y * scale <= MAX_PAGE_PIXELS * 1.001);
    }
}
