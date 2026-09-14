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

use crate::model::{
    AnnotKey, Changes, Highlight, NewHighlight, PageGeometry, PdfBox, Reply, Request, Rgb, SearchHit, TextChar,
};
use crate::selection;
use crate::cache::{self, Cache};
use crate::worker::{self, Wanted, MAX_SEARCH_HITS};

const COLORS: [(&str, Rgb); 4] = [
    ("Yellow", [1.0, 0.93, 0.25]),
    ("Green", [0.56, 0.93, 0.45]),
    ("Blue", [0.45, 0.76, 1.0]),
    ("Pink", [1.0, 0.62, 0.8]),
];

/// The steps Ctrl +/- and the zoom buttons move through. Wide enough for a
/// whole A0 drawing on a laptop screen at one end and fine detail at the other.
const ZOOMS: [f32; 15] = [0.1, 0.15, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0];

const TOP_PAD: f32 = 24.0;
const BOTTOM_PAD: f32 = 80.0;
const SIDE_PAD: f32 = 16.0;
const PAGE_GAP: f32 = 18.0;
/// Room the vertical scroll bar takes, allowed for when fitting pages.
const SCROLLBAR_ROOM: f32 = 14.0;

/// A page more than this much wider than the document's usual page counts as
/// oversized, and is shrunk to the usual width while shrinking is on.
const OVERSIZED: f32 = 1.05;

/// Memory allowed for page images kept beyond the ones in view (and the page
/// either side, which are always kept so they don't flicker).
const TEXTURE_BUDGET: usize = 512 * 1024 * 1024;
/// The most pixels a whole-page image is rendered at, about 32 MB of RGBA.
/// Zoomed in past this, the part of the page in view is rendered on its own at
/// full sharpness (`Detail`), so the whole-page image is only the backdrop for
/// the edges while scrolling -- and a smaller one renders much sooner.
const MAX_PAGE_PIXELS: f32 = 8_000_000.0;
/// Device pixels rendered beyond each edge of the view when zoomed in past a
/// whole-page image, so scrolling a little doesn't need a new render.
const DETAIL_MARGIN: u32 = 384;

/// Most memory for squares of pages drawn zoomed in (see `model::TILE`), kept
/// for zooming back in or scrolling back: about 380 squares. See `budgets`.
const TILE_BUDGET: usize = 384 * 1024 * 1024;

/// Most memory for earlier whole-page images kept for zooming back to their
/// size. See `budgets`.
const SPARE_BUDGET: usize = 256 * 1024 * 1024;

/// The memory allowed for squares and for spare page images, from the
/// physical memory free when the app started: the most above on a machine
/// with plenty, less on one without.
///
/// A texture costs the process about two and a half times its pixels, since
/// the graphics driver keeps copies of its own: opening a drawing set and
/// letting the app draw 176 MB of squares ahead of a zoom took the process
/// from 250 MB to 740 MB. So the full budgets can cost 1.6 GB, and here they
/// come to about a third of what was free.
fn budgets(free: u64) -> (usize, usize) {
    const MB: u64 = 1024 * 1024;
    let tiles = (free / 12).clamp(64 * MB, TILE_BUDGET as u64) as usize;
    let spares = (free / 18).clamp(32 * MB, SPARE_BUDGET as u64) as usize;
    (tiles, spares)
}

/// Seconds the pointer rests before the spot under it is drawn ahead of a
/// zoom in; see `draw_pages`.
const PREDICT_REST: f64 = 0.5;
/// Drawings asked for ahead of a zoom at once, about one per helper.
const PREDICT_JOBS: usize = 3;
/// Squares on a side in each region drawn ahead of a zoom: 2048 pixels.
const PREDICT_BLOCK: u32 = 4;
/// Seconds after which an unanswered request ahead of a zoom counts as lost,
/// as when no helpers are running to answer it.
const PREDICT_TIMEOUT: f64 = 20.0;

/// Screens a second. Faster than this is a fling or a scroll bar drag, not
/// reading, so pages flying past aren't loaded until the view slows. Steady
/// wheel scrolling stays under it (about five screens a second at most), so
/// pages still load as they pass.
const FAST_SCROLL: f32 = 8.0;
/// Pages loaded ahead of the view once everything in it is drawn.
const LOOK_AHEAD: usize = 3;

/// Seconds a zoom has to stay put before slow pages are drawn again at the new
/// size. Each redraw of one takes a second or more, so redrawing at every
/// step of a zoom would only queue them up; the old image stretches meanwhile.
const ZOOM_SETTLE: f64 = 0.3;

/// How long typing has to pause before the find box starts a search.
const SEARCH_DEBOUNCE: f64 = 0.25;

/// Every row in the results list is this tall, so the list can draw only the
/// rows in view however many matches there are.
const RESULT_ROW_HEIGHT: f32 = 64.0;

const POPUP_WIDTH: f32 = 300.0;

// A light palette throughout.
const BG: Color32 = Color32::from_rgb(0xe9, 0xeb, 0xee);
const SURFACE: Color32 = Color32::WHITE;
const BORDER: Color32 = Color32::from_rgb(0xe3, 0xe6, 0xea);
const INPUT_BORDER: Color32 = Color32::from_rgb(0xd0, 0xd5, 0xdb);
const HOVER_FILL: Color32 = Color32::from_rgb(0xf3, 0xf4, 0xf6);
const PRESSED_FILL: Color32 = Color32::from_rgb(0xe5, 0xe7, 0xeb);
const TEXT: Color32 = Color32::from_rgb(0x1f, 0x23, 0x28);
const MUTED: Color32 = Color32::from_rgb(0x6b, 0x72, 0x80);
const SUBTLE: Color32 = Color32::from_rgb(0xa9, 0xae, 0xb4);
const ACCENT: Color32 = Color32::from_rgb(0x3b, 0x82, 0xf6);
const ACCENT_HOVER: Color32 = Color32::from_rgb(0x2f, 0x6f, 0xd9);
const ACCENT_PRESSED: Color32 = Color32::from_rgb(0x26, 0x5f, 0xc0);
const ACCENT_SOFT: Color32 = Color32::from_rgb(0xe8, 0xf0, 0xfe);
const ACCENT_SOFT_BORDER: Color32 = Color32::from_rgb(0xbf, 0xd5, 0xfb);
const ACCENT_TEXT: Color32 = Color32::from_rgb(0x1d, 0x4e, 0xd8);
const DANGER: Color32 = Color32::from_rgb(0xdc, 0x26, 0x26);
const DANGER_SOFT: Color32 = Color32::from_rgb(0xfe, 0xf2, 0xf2);
const DANGER_PRESSED: Color32 = Color32::from_rgb(0xfe, 0xe2, 0xe2);
const DIRTY: Color32 = Color32::from_rgb(0xb4, 0x53, 0x09);
const SAVED: Color32 = Color32::from_rgb(0x15, 0x80, 0x3d);
const QUOTE_TEXT: Color32 = Color32::from_rgb(0x4b, 0x55, 0x63);
const QUOTE_BG: Color32 = Color32::from_rgb(0xf6, 0xf8, 0xfa);
const QUOTE_BORDER: Color32 = Color32::from_rgb(0xdf, 0xe3, 0xe7);
const NOTE_ACTIVE: Color32 = Color32::from_rgb(0xea, 0xf2, 0xfe);
const ROW_HOVER: Color32 = Color32::from_rgb(0xf6, 0xf8, 0xfa);
const ROW_RULE: Color32 = Color32::from_rgb(0xee, 0xf0, 0xf2);
const SELECTION: Color32 = Color32::from_rgba_premultiplied(0x1d, 0x41, 0x7b, 0x50);
// Search matches are orange, so they can't be mistaken for yellow highlights.
const HIT: Color32 = Color32::from_rgb(0xff, 0xc9, 0x8a);
const HIT_CURRENT: Color32 = Color32::from_rgb(0xff, 0x96, 0x3c);
const HIT_OUTLINE: Color32 = Color32::from_rgb(0xe8, 0x6a, 0x00);

const UV_FULL: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

fn next_uid() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn to_color32([r, g, b]: Rgb) -> Color32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(byte(r), byte(g), byte(b))
}

fn soft_shadow() -> Shadow {
    Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(36) }
}

/// The light theme, applied once. Only what egui draws itself (text boxes,
/// scroll bars, separators, tooltips) comes from here; the app's buttons are
/// painted by `paint_button`.
fn apply_light_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |style| {
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(10.0, 5.0);
        style.interaction.selectable_labels = false;

        let v = &mut style.visuals;
        v.panel_fill = SURFACE;
        v.window_fill = SURFACE;
        v.extreme_bg_color = SURFACE;
        v.text_edit_bg_color = Some(SURFACE);
        v.faint_bg_color = QUOTE_BG;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.window_shadow = soft_shadow();
        v.popup_shadow = soft_shadow();
        v.selection.bg_fill = Color32::from_rgb(0xcf, 0xe0, 0xfd);
        // Also the focus ring on text boxes.
        v.selection.stroke = Stroke::new(1.5, ACCENT);

        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        for (state, fill, border) in [
            (&mut w.inactive, SURFACE, INPUT_BORDER),
            (&mut w.hovered, HOVER_FILL, Color32::from_gray(0xb8)),
            (&mut w.active, PRESSED_FILL, ACCENT),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, border);
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.corner_radius = CornerRadius::same(6);
            state.expansion = 0.0;
        }
    });
}

/* ------------------------------------------------------------------ *
 * State
 * ------------------------------------------------------------------ */

struct PageTexture {
    handle: TextureHandle,
    /// Pixels per PDF point it was rendered at.
    scale: f32,
    /// False while a slow page is still drawing in.
    complete: bool,
}

/// One square of a page drawn zoomed in, placed on the page's grid (see
/// `model::TILE`): the page, its full size in pixels at that zoom, and the
/// square's column and row.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TileKey {
    page: usize,
    full: [u32; 2],
    column: u32,
    row: u32,
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

/// Where a square sits on screen, for the page drawn at `page`.
fn tile_screen_rect(page: Rect, full: [u32; 2], column: u32, row: u32) -> Rect {
    let [fw, fh] = full.map(|v| v as f32);
    let [x, y, w, h] = crate::model::tile_rect(full, column, row).map(|v| v as f32);
    Rect::from_min_size(
        page.min + vec2(x / fw * page.width(), y / fh * page.height()),
        vec2(w / fw * page.width(), h / fh * page.height()),
    )
}

/// A highlight as displayed. `uid` is stable for the life of the entry; the
/// file position in `hl.key` is not (it changes on save).
struct Entry {
    uid: u64,
    hl: Highlight,
}

impl Entry {
    fn is_new(&self) -> bool {
        self.hl.key.is_none()
    }
}

struct Doc {
    generation: u64,
    name: String,
    /// Page sizes in points, as displayed (rotated).
    sizes: Vec<Vec2>,
    /// The size most pages share; fit-to-width and shrinking work from it.
    usual_size: Vec2,
    /// Each page's rotation and visible box, once the worker has read it.
    /// Nothing is drawn over a page, and it can't be selected, until then.
    geometry: Vec<Option<PageGeometry>>,
    highlights: Vec<Entry>,
    /// Whether every page's highlights have arrived from the worker.
    highlights_done: bool,
    /// Keys of saved highlights the user removed.
    deletes: Vec<AnnotKey>,
    /// Key -> new comment, for saved highlights.
    edits: HashMap<AnnotKey, String>,
    dirty: bool,
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
    /// Where the pointer is resting, and since when; see `PREDICT_REST`.
    pointer_rest: Option<(Pos2, f64)>,
    /// Whether a middle-button drag is moving the document.
    panning: bool,
    allow_close: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        apply_light_style(&cc.egui_ctx);
        // Ctrl +/- zooms the page, not the whole interface.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);

        let wanted = Arc::new(Mutex::new(Wanted::default()));
        // Slow pages are kept on disk between sessions. PDF_ANNOTATE_CACHE=0
        // turns that off, for comparing; any other value is a folder to use.
        let cache = match std::env::var_os("PDF_ANNOTATE_CACHE") {
            Some(value) if value == "0" => None,
            Some(dir) => Cache::open(PathBuf::from(dir), cache::DEFAULT_LIMIT).ok(),
            None => Cache::open(cache::default_dir(), cache::DEFAULT_LIMIT).ok(),
        };
        let (tx, rx) = worker::spawn(cc.egui_ctx.clone(), wanted.clone(), crate::pool::Helpers::from_current_exe(), cache.map(Arc::new));
        let (tile_budget, spare_budget) = budgets(crate::pool::free_memory());
        worker::trace(format_args!("ui: {} MB for squares and {} MB for spares", tile_budget >> 20, spare_budget >> 20));

        let mut app = Self {
            tx,
            rx,
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
            last_view: None,
            heading_down: true,
            last_zoom: 1.0,
            zoom_changed_at: f64::NEG_INFINITY,
            render_scales: Arc::new(Vec::new()),
            view_sharp: false,
            pointer_rest: None,
            panning: false,
            allow_close: false,
        };
        if let Some(path) = initial {
            app.open(path);
        }
        app
    }

    fn now(ctx: &egui::Context) -> f64 {
        ctx.input(|i| i.time)
    }

    fn show_toast_message(&mut self, ctx: &egui::Context, message: String) {
        self.toast = Some((message, Self::now(ctx) + 5.0));
    }

    fn author_name(&self) -> String {
        match self.author.trim() {
            "" => "me".to_owned(),
            name => name.to_owned(),
        }
    }

    fn entry(&self, uid: u64) -> Option<&Entry> {
        self.doc.as_ref()?.highlights.iter().find(|e| e.uid == uid)
    }

    /* -------------------------------------------------------------- *
     * Opening and saving
     * -------------------------------------------------------------- */

    fn open(&mut self, path: PathBuf) {
        self.generation += 1;
        self.status = Status::Opening;
        let _ = self.tx.send(Request::Open { generation: self.generation, path });
    }

    fn pick_and_open(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
            self.open(path);
        }
    }

    fn confirm_discard(&self) -> bool {
        if !self.doc.as_ref().is_some_and(|d| d.dirty) {
            return true;
        }
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Unsaved highlights")
            .set_description("You have unsaved highlights. Discard them?")
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes
    }

    fn save(&mut self) {
        let Some(doc) = self.doc.as_ref() else { return };
        if !doc.dirty || matches!(self.status, Status::Saving) {
            return;
        }
        let changes = Changes {
            adds: doc
                .highlights
                .iter()
                .filter(|e| e.is_new())
                .map(|e| NewHighlight {
                    page: e.hl.page,
                    quads: e.hl.quads.clone(),
                    color: e.hl.color,
                    comment: e.hl.comment.clone(),
                })
                .collect(),
            deletes: doc.deletes.clone(),
            edits: doc.edits.iter().map(|(k, v)| (*k, v.clone())).collect(),
            author: self.author_name(),
        };
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

                Reply::Opened { generation, path, page_sizes } if generation == self.generation => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ctx.send_viewport_cmd(ViewportCommand::Title(format!("{name} - PDF Annotate")));
                    let sizes: Vec<Vec2> = page_sizes.iter().map(|[w, h]| vec2(*w, *h)).collect();
                    self.doc = Some(Doc {
                        generation,
                        name,
                        usual_size: usual_page_size(&sizes),
                        geometry: vec![None; sizes.len()],
                        sizes,
                        highlights: Vec::new(),
                        highlights_done: false,
                        deletes: Vec::new(),
                        edits: HashMap::new(),
                        dirty: false,
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
                    });
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
                    self.status = Status::Idle;
                    self.show_toast_message(ctx, format!("Could not open that PDF: {error}"));
                }

                Reply::Highlights { generation, highlights, geometry, done } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        for (page, g) in geometry {
                            if let Some(slot) = doc.geometry.get_mut(page) {
                                *slot = Some(g);
                            }
                        }
                        doc.highlights.extend(highlights.into_iter().map(|hl| Entry { uid: next_uid(), hl }));
                        // Pages arrive in the order they're viewed, not file
                        // order; keep the notes list in file order. The sort is
                        // stable, so unsaved highlights keep the order they
                        // were made in.
                        doc.highlights.sort_by_key(|e| (e.hl.page, e.hl.key.map_or(usize::MAX, |k| k.index)));
                        doc.highlights_done = done;
                    }
                }

                Reply::Text { generation, page, chars } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        doc.text_pending.remove(&page);
                        doc.text.insert(page, chars);
                    }
                }

                Reply::Rendered { generation, page, scale, texture, complete, slow } => {
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
                                doc.spares.insert((page, scale.to_bits()), PageTexture { handle: texture, scale, complete });
                            }
                        } else {
                            // A part-drawn page never replaces a finished image,
                            // such as the one being stretched during a zoom.
                            let finished = doc.textures.get(&page).is_some_and(|t| t.complete);
                            if complete || !finished {
                                if let Some(old) = doc.textures.insert(page, PageTexture { handle: texture, scale, complete }) {
                                    // Kept, for a zoom back to its size.
                                    if old.complete && (old.scale - scale).abs() > 1e-3 {
                                        doc.spares.insert((page, old.scale.to_bits()), old);
                                    }
                                }
                            }
                            if complete {
                                doc.render_pending.remove(&page);
                            }
                        }
                    }
                }

                Reply::TextSkipped { generation, page } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        doc.text_pending.remove(&page);
                    }
                }

                Reply::RenderedRegion { generation, page, full, region, tiles } => {
                    let now = Self::now(ctx);
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        if doc.predicting.remove(&PredictKey::Region(page, region)).is_none() {
                            doc.detail_pending.remove(&page);
                        }
                        for tile in tiles {
                            let key = TileKey { page, full, column: tile.column, row: tile.row };
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

                Reply::Saved { generation, pages, highlights } => {
                    if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                        // Only the pages the save touched come back, re-read;
                        // highlights everywhere else are exactly as they were.
                        doc.highlights.retain(|e| !pages.contains(&e.hl.page));
                        doc.highlights.extend(highlights.into_iter().map(|hl| Entry { uid: next_uid(), hl }));
                        doc.highlights.sort_by_key(|e| (e.hl.page, e.hl.key.map_or(usize::MAX, |k| k.index)));
                        doc.deletes.clear();
                        doc.edits.clear();
                        doc.dirty = false;
                        self.active = None;
                        self.status = Status::Saved { until: Self::now(ctx) + 2.5 };
                    }
                }

                Reply::SaveFailed { generation, error } => {
                    if generation == self.generation {
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
            if self.confirm_discard() {
                self.allow_close = true;
            } else {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let (save, open, zoom_in, zoom_out, fit, find, previous, next) = ctx.input_mut(|i| {
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
            )
        });
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
        if previous {
            self.step_hit(-1);
        }
        if next {
            self.step_hit(1);
        }

        // Ctrl + mouse wheel zooms in on whatever is under the pointer.
        let (pinch, pointer) = ctx.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
        if pinch != 1.0 && self.doc.is_some() {
            self.zoom_mode = ZoomMode::Custom;
            self.change_zoom(self.zoom * pinch, pointer);
        }

        // Dropping a PDF on the window opens it.
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            if self.confirm_discard() {
                self.open(path);
            }
        }
    }

    /* -------------------------------------------------------------- *
     * Zoom and layout
     * -------------------------------------------------------------- */

    /// The next zoom step up (1) or down (-1), holding the middle of the view still.
    fn zoom_by(&mut self, direction: i32) {
        let next = if direction > 0 {
            ZOOMS.iter().copied().find(|z| *z > self.zoom * 1.001)
        } else {
            ZOOMS.iter().rev().copied().find(|z| *z < self.zoom * 0.999)
        };
        if let Some(zoom) = next {
            self.zoom_mode = ZoomMode::Custom;
            self.change_zoom(zoom, None);
        }
    }

    /// Changes the zoom, keeping the page spot under `anchor` (or the middle of
    /// the view) where it is on screen.
    fn change_zoom(&mut self, zoom: f32, anchor: Option<Pos2>) {
        let zoom = zoom.clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1]);
        if (zoom - self.zoom).abs() < 1e-4 {
            return;
        }
        self.hold_still(anchor);
        self.zoom = zoom;
    }

    /// Remembers the page spot under `anchor` (or the middle of the view), so
    /// the next layout can scroll it back to the same place on screen.
    fn hold_still(&mut self, anchor: Option<Pos2>) {
        // Several changes in one frame keep the first anchor, which is the one
        // measured against what's actually on screen.
        if self.zoom_anchor.is_some() || !self.viewer_rect.is_positive() {
            return;
        }
        let view = self.viewer_rect;
        let screen = anchor.filter(|p| view.contains(*p)).unwrap_or_else(|| view.center());
        let distance = |r: &Rect| {
            if screen.y < r.min.y {
                r.min.y - screen.y
            } else if screen.y > r.max.y {
                screen.y - r.max.y
            } else {
                0.0
            }
        };
        let Some((&page, rect)) = self.page_rects.iter().min_by(|a, b| distance(a.1).total_cmp(&distance(b.1))) else {
            return;
        };
        self.zoom_anchor = Some(ZoomAnchor {
            page,
            fx: ((screen.x - rect.min.x) / rect.width()).clamp(0.0, 1.0),
            fy: ((screen.y - rect.min.y) / rect.height()).clamp(0.0, 1.0),
            screen,
            origin: self.content_origin + self.scroll_offset,
        });
    }

    fn set_shrink_wide(&mut self, shrink: bool) {
        if shrink != self.shrink_wide {
            self.hold_still(None);
            self.shrink_wide = shrink;
        }
    }

    /// The zoom a fit mode wants for a view of this size.
    fn fit_zoom(&self, doc: &Doc, view: Vec2) -> Option<f32> {
        let usual = doc.usual_size;
        let width = (view.x - 2.0 * SIDE_PAD - SCROLLBAR_ROOM).max(50.0) / usual.x.max(1.0);
        let zoom = match self.zoom_mode {
            ZoomMode::FitWidth => width,
            ZoomMode::FitPage => width.min((view.y - TOP_PAD - PAGE_GAP).max(50.0) / usual.y.max(1.0)),
            ZoomMode::Custom => return None,
        };
        Some(zoom.clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1]))
    }

    fn layout(&self, doc: &Doc) -> PageLayout {
        page_layout(&doc.sizes, doc.usual_size, self.zoom, self.shrink_wide)
    }

    /// Whether everything in view was drawn at full sharpness last frame: every
    /// page's image at the size wanted and, where zoomed in, every square
    /// covering the view -- wherever they came from, memory, disk or drawing.
    /// For tools driving the app, such as floors.
    pub fn view_is_sharp(&self) -> bool {
        self.view_sharp
    }

    /// Scrolls so `page` (counted from 0) starts at the top of the view. For
    /// tools driving the app, such as floors.
    pub fn go_to_page(&mut self, page: usize) {
        let Some(doc) = &self.doc else { return };
        let layout = self.layout(doc);
        if let Some(&top) = layout.tops.get(page) {
            self.scroll_y = Some(top);
        }
    }

    /// Scrolls a spot on a page into view: a third of the way down, and
    /// horizontally centred when the pages are wider than the window.
    fn scroll_to_box(&mut self, page: usize, q: &PdfBox) {
        let Some(doc) = &self.doc else { return };
        let layout = self.layout(doc);
        let Some(&top) = layout.tops.get(page) else { return };
        let size = doc.sizes[page] * layout.scales[page];
        let geometry = doc.geometry[page].unwrap_or_else(|| flat_geometry(doc.sizes[page]));
        let (left, t, right, _) = geometry.box_to_view(q);

        self.scroll_y = Some(top + t * size.y - self.viewer_rect.height() / 3.0);
        let view_w = self.viewer_rect.width();
        let content_w = content_width(view_w, layout.widest);
        if content_w > view_w + 1.0 {
            let x = page_x(content_w, size.x) + (left + right) / 2.0 * size.x;
            self.scroll_x = Some(x - view_w / 2.0);
        }
    }

    /* -------------------------------------------------------------- *
     * Find in document
     * -------------------------------------------------------------- */

    /// Starts a search once typing pauses.
    fn update_search(&mut self, ctx: &egui::Context) {
        if self.doc.is_none() || self.search.query == self.search.sent {
            return;
        }
        let wait = self.search.edited_at + SEARCH_DEBOUNCE - Self::now(ctx);
        if wait <= 0.0 {
            self.start_search();
        } else {
            ctx.request_repaint_after(Duration::from_secs_f64(wait));
        }
    }

    fn start_search(&mut self) {
        let Some(doc) = &self.doc else { return };
        let search = &mut self.search;
        search.id += 1;
        search.sent = search.query.clone();
        search.hits.clear();
        search.current = None;
        search.searched = 0;
        search.done = selection::normalize_query(&search.query).is_empty();
        search.start_page = self.current_page;
        search.reveal_row = None;
        search.list_scroll_to = Some(0.0);
        let _ = self.tx.send(Request::Search { generation: doc.generation, id: search.id, query: search.query.clone() });
    }

    /// Next (1) or previous (-1) match, wrapping around. With no match chosen
    /// yet, starts from the page in view.
    fn step_hit(&mut self, direction: isize) {
        let n = self.search.hits.len();
        if n == 0 {
            return;
        }
        let i = match self.search.current {
            Some(current) => (current as isize + direction).rem_euclid(n as isize) as usize,
            None if direction > 0 => self.search.hits.iter().position(|h| h.page >= self.current_page).unwrap_or(0),
            None => self.search.hits.iter().rposition(|h| h.page <= self.current_page).unwrap_or(n - 1),
        };
        self.go_to_hit(i);
    }

    /// Select a match, bring its row into view in the results list, and scroll
    /// the page to it unless it's already comfortably in view.
    fn go_to_hit(&mut self, i: usize) {
        let Some(doc) = &self.doc else { return };
        let Some(hit) = self.search.hits.get(i) else { return };
        let Some(q) = hit.quads.first().copied() else { return };
        let page = hit.page;
        self.search.current = Some(i);
        self.search.reveal_row = Some(i);

        if let (Some(rect), Some(geometry)) = (self.page_rects.get(&page), doc.geometry[page]) {
            if self.viewer_rect.shrink(40.0).contains_rect(to_screen(*rect, &geometry, &q)) {
                return;
            }
        }
        self.scroll_to_box(page, &q);
    }

    fn match_count(&self) -> String {
        let n = self.search.hits.len();
        if n >= MAX_SEARCH_HITS { format!("{MAX_SEARCH_HITS}+") } else { n.to_string() }
    }

    fn search_progress(&self) -> usize {
        let pages = self.doc.as_ref().map_or(1, |d| d.sizes.len().max(1));
        self.search.searched * 100 / pages
    }

    /// The short status beside the find box.
    fn search_label(&self) -> String {
        let s = &self.search;
        if selection::normalize_query(&s.sent).is_empty() {
            return String::new();
        }
        let count = self.match_count();
        if !s.done {
            return format!("{count} found · {}%", self.search_progress());
        }
        match (s.hits.len(), s.current) {
            (0, _) => "No matches".to_owned(),
            (_, Some(current)) => format!("{} of {count}", current + 1),
            _ => format!("{count} matches"),
        }
    }

    /// The find box and its controls, laid out right to left inside the
    /// toolbar's right-hand group.
    fn search_box(&mut self, ui: &mut Ui) {
        let any = !self.search.hits.is_empty();
        ui.add_enabled_ui(any, |ui| {
            // The arrows are in monospace: egui's bundled proportional font has
            // no arrow glyphs, but its monospace font does.
            if icon_button(ui, "↓").on_hover_text("Next match (Enter or F3)").clicked() {
                self.step_hit(1);
            }
            if icon_button(ui, "↑").on_hover_text("Previous match (Shift+Enter or Shift+F3)").clicked() {
                self.step_hit(-1);
            }
        });
        let label = self.search_label();
        if !label.is_empty() {
            ui.label(RichText::new(label).size(13.0).color(MUTED));
        }

        let response = ui.add_enabled(
            self.doc.is_some(),
            TextEdit::singleline(&mut self.search.query)
                .id(Id::new("find"))
                .hint_text("Find in document")
                .desired_width(200.0)
                .margin(Margin { left: 28, right: 8, top: 6, bottom: 6 }),
        );
        paint_magnifier(ui, response.rect);
        if response.changed() {
            self.search.edited_at = Self::now(ui.ctx());
        }
        if std::mem::take(&mut self.search.focus) {
            response.request_focus();
        }
        // A single-line box gives up focus on Enter and Esc.
        if response.lost_focus() {
            let (enter, escape, shift) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape), i.modifiers.shift));
            if enter {
                if self.search.query != self.search.sent {
                    self.start_search();
                } else {
                    self.step_hit(if shift { -1 } else { 1 });
                }
                // Stay in the box, so Enter can be pressed again.
                response.request_focus();
            } else if escape {
                self.search.query.clear();
                self.start_search();
            }
        }

        let results = self.sidebar == Sidebar::Results;
        if styled_button(ui, "Results", Tone::Secondary, results).on_hover_text("List every match in a side panel").clicked() {
            self.sidebar = if results { Sidebar::None } else { Sidebar::Results };
        }
    }

    /* -------------------------------------------------------------- *
     * Toolbar
     * -------------------------------------------------------------- */

    fn toolbar(&mut self, ui: &mut Ui) {
        let frame = Frame::NONE.fill(SURFACE).inner_margin(Margin::symmetric(12, 8));
        egui::Panel::top("toolbar").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;

                if styled_button(ui, "Open PDF…", Tone::Primary, false).on_hover_text("Open (Ctrl+O)").clicked() {
                    self.pick_and_open();
                }
                let can_save = self.doc.as_ref().is_some_and(|d| d.dirty) && !matches!(self.status, Status::Saving);
                let save = ui.add_enabled_ui(can_save, |ui| styled_button(ui, "Save", Tone::Secondary, false)).inner;
                if save.on_hover_text("Save (Ctrl+S)").clicked() {
                    self.save();
                }

                ui.separator();

                let has_doc = self.doc.is_some();
                ui.add_enabled_ui(has_doc, |ui| {
                    if icon_button(ui, "−").on_hover_text("Zoom out (Ctrl+-)").clicked() {
                        self.zoom_by(-1);
                    }
                    ui.add_sized(
                        vec2(46.0, 30.0),
                        egui::Label::new(RichText::new(format!("{:.0}%", self.zoom * 100.0)).size(13.0).color(MUTED)),
                    );
                    if icon_button(ui, "+").on_hover_text("Zoom in (Ctrl++, or Ctrl + mouse wheel)").clicked() {
                        self.zoom_by(1);
                    }
                    let fit_width = self.zoom_mode == ZoomMode::FitWidth;
                    if styled_button(ui, "Fit width", Tone::Secondary, fit_width).on_hover_text("Fit the page width (Ctrl+0)").clicked() {
                        self.zoom_mode = ZoomMode::FitWidth;
                    }
                    let fit_page = self.zoom_mode == ZoomMode::FitPage;
                    if styled_button(ui, "Fit page", Tone::Secondary, fit_page).on_hover_text("Fit a whole page").clicked() {
                        self.zoom_mode = ZoomMode::FitPage;
                    }
                });

                ui.separator();

                let (page_label, name) = match &self.doc {
                    Some(d) => (format!("{} / {}", self.current_page + 1, d.sizes.len()), d.name.clone()),
                    None => ("—".to_owned(), "No file open".to_owned()),
                };
                ui.label(RichText::new(page_label).size(13.0).color(MUTED));
                ui.add_space(4.0);
                ui.label(RichText::new(name).size(13.5).color(if has_doc { TEXT } else { MUTED }));

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if styled_button(ui, "Quit", Tone::Ghost, false).on_hover_text("Close the app").clicked() {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                    }
                    let notes = self.sidebar == Sidebar::Notes;
                    if styled_button(ui, "Notes", Tone::Secondary, notes).on_hover_text("Show or hide the notes panel").clicked() {
                        self.sidebar = if notes { Sidebar::None } else { Sidebar::Notes };
                    }
                    ui.separator();
                    self.search_box(ui);
                    let (text, color) = self.status_label(ui.ctx());
                    ui.label(RichText::new(text).size(13.0).color(color));
                });
            });
        });
    }

    fn status_label(&mut self, ctx: &egui::Context) -> (&'static str, Color32) {
        match self.status {
            Status::Opening => ("Opening...", MUTED),
            Status::Saving => ("Saving...", MUTED),
            _ if self.doc.as_ref().is_some_and(|d| d.dirty) => ("Unsaved changes", DIRTY),
            Status::Saved { until } => {
                let now = Self::now(ctx);
                if now < until {
                    ctx.request_repaint_after(Duration::from_secs_f64(until - now));
                    ("Saved", SAVED)
                } else {
                    self.status = Status::Idle;
                    ("", MUTED)
                }
            }
            Status::Idle => ("", MUTED),
        }
    }

    /* -------------------------------------------------------------- *
     * Page viewer
     * -------------------------------------------------------------- */

    fn viewer(&mut self, ui: &mut Ui) {
        let hovering_file = ui.input(|i| !i.raw.hovered_files.is_empty());
        let area = ui.max_rect();

        if let Some(message) = self.fatal.clone() {
            self.message_card(ui, "pdfium could not be loaded", &message, false);
        } else if self.doc.is_none() {
            self.message_card(
                ui,
                "PDF Annotate",
                "Open a PDF, drag across text to highlight it, and attach a note.\n\nHighlights are written \
                 into the PDF as real annotations, so they open anywhere. Drop a file on this window to get started.",
                true,
            );
        } else {
            self.pages(ui);
        }

        if hovering_file {
            ui.painter()
                .rect_stroke(area.shrink(12.0), CornerRadius::same(10), Stroke::new(2.5, ACCENT), StrokeKind::Inside);
        }
    }

    fn message_card(&mut self, ui: &mut Ui, title: &str, body: &str, with_open: bool) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.12);
            Frame::NONE
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(14))
                .inner_margin(Margin::same(32))
                .shadow(soft_shadow())
                .show(ui, |ui| {
                    ui.set_max_width(400.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(title).size(22.0).strong().color(TEXT));
                        ui.add_space(6.0);
                        ui.label(RichText::new(body).color(QUOTE_TEXT));
                        if with_open {
                            ui.add_space(14.0);
                            if styled_button(ui, "Open PDF…", Tone::Primary, false).clicked() {
                                self.pick_and_open();
                            }
                        }
                    });
                });
        });
    }

    fn pages(&mut self, ui: &mut Ui) {
        // Fit modes follow the window.
        let view = ui.available_size();
        let fit = self.doc.as_ref().and_then(|doc| self.fit_zoom(doc, view));
        if let Some(fit) = fit {
            if (fit - self.zoom).abs() > self.zoom * 0.002 {
                self.change_zoom(fit, None);
            }
        }

        let Some(doc) = self.doc.as_ref() else { return };
        let layout = self.layout(doc);

        // Put the spot held during a zoom change back where it was on screen.
        if let Some(anchor) = self.zoom_anchor.take() {
            if let (Some(&top), Some(&scale)) = (layout.tops.get(anchor.page), layout.scales.get(anchor.page)) {
                let view_w = if self.viewer_rect.is_positive() { self.viewer_rect.width() } else { view.x - SCROLLBAR_ROOM };
                let size = doc.sizes[anchor.page] * scale;
                let point = vec2(page_x(content_width(view_w, layout.widest), size.x) + anchor.fx * size.x, top + anchor.fy * size.y);
                let offset = anchor.origin.to_vec2() + point - anchor.screen.to_vec2();
                self.scroll_x = Some(offset.x);
                self.scroll_y = Some(offset.y);
            }
        }

        // Dragging with the middle button grabs the document and moves it with
        // the pointer, in both directions.
        let (pressed, held, moved, at) = ui.ctx().input(|i| {
            (i.pointer.button_pressed(egui::PointerButton::Middle), i.pointer.middle_down(), i.pointer.delta(), i.pointer.hover_pos())
        });
        if pressed && at.is_some_and(|at| self.viewer_rect.contains(at)) {
            self.panning = true;
        }
        if !held {
            self.panning = false;
        }
        if self.panning && moved != Vec2::ZERO {
            self.scroll_x = Some(self.scroll_x.unwrap_or(self.scroll_offset.x) - moved.x);
            self.scroll_y = Some(self.scroll_y.unwrap_or(self.scroll_offset.y) - moved.y);
        }

        let mut area = egui::ScrollArea::both().id_salt("pages").auto_shrink(false);
        if let Some(x) = self.scroll_x.take() {
            area = area.horizontal_scroll_offset(x.max(0.0));
        }
        if let Some(y) = self.scroll_y.take() {
            area = area.vertical_scroll_offset(y.max(0.0));
        }
        let content = vec2(layout.widest + 2.0 * SIDE_PAD, layout.height);
        let output = area.show_viewport(ui, |ui, viewport| {
            ui.set_min_width(content.x);
            ui.set_height(content.y);
            self.draw_pages(ui, viewport, &layout);
        });
        self.viewer_rect = output.inner_rect;
        self.scroll_offset = output.state.offset;
        // Set last, so the hand wins over the text cursor the pages ask for.
        if self.panning {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
    }

    /// Lays out every page but only draws, and only asks the worker about, the
    /// ones in view -- so a 5,000-page document costs no more per frame than a
    /// five-page one.
    fn draw_pages(&mut self, ui: &mut Ui, viewport: Rect, layout: &PageLayout) {
        let tops = &layout.tops;
        if tops.is_empty() {
            return;
        }
        let ctx = ui.ctx().clone();
        // Render at device resolution so text stays sharp on a high-DPI screen.
        let ppp = ctx.pixels_per_point().min(2.0);
        let max_side = ctx.input(|i| i.max_texture_side) as f32;
        let origin = ui.max_rect().min;
        self.content_origin = origin;
        let content_w = content_width(ui.max_rect().width(), layout.widest);
        let busy = matches!(self.status, Status::Saving);

        let n = tops.len();
        let first = tops.partition_point(|t| *t <= viewport.min.y).saturating_sub(1);
        let mut last = first;
        while last + 1 < n && tops[last + 1] < viewport.max.y {
            last += 1;
        }
        self.current_page = tops.partition_point(|t| *t <= viewport.min.y + viewport.height() * 0.35).saturating_sub(1);

        let segments = match (&self.drag, &self.doc) {
            (Some(drag), Some(doc)) => drag_segments(&doc.text, drag),
            _ => Vec::new(),
        };
        let active = self.active;
        let shrink_wide = self.shrink_wide;
        let mut drag_start = None;
        let mut clicked = None;
        let mut toggle_shrink = false;
        self.page_rects.clear();

        let Some(doc) = self.doc.as_mut() else { return };

        // Images of the page either side are always kept, so they don't flicker.
        let lo = first.saturating_sub(1);
        let hi = (last + 1).min(n - 1);

        // How fast the view is moving. A fast scroll passes pages that each
        // take a second to load if they're dense, so nothing new is asked for
        // until the view slows to where it can be read.
        let now = ctx.input(|i| i.time);
        let (moved, elapsed) = self.last_view.map_or((0.0, 1.0), |(t, y)| (viewport.min.y - y, (now - t) as f32));
        self.last_view = Some((now, viewport.min.y));
        if moved.abs() > 0.5 {
            self.heading_down = moved > 0.0;
        }
        let moving = elapsed > 0.0 && moved.abs() / elapsed > FAST_SCROLL * viewport.height();
        if moving {
            // Look again shortly, in case that was the scroll's last frame.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.zoom != self.last_zoom {
            self.last_zoom = self.zoom;
            self.zoom_changed_at = now;
        }
        let settling = now - self.zoom_changed_at < ZOOM_SETTLE;
        if settling {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(ZOOM_SETTLE));
        }

        // The pages in view, from the middle of the view out, so the page the
        // view is on goes first.
        let middle = viewport.center().y;
        let mut order: Vec<usize> = (first..=last).collect();
        let off_middle = |p: usize| (tops[p] + doc.sizes[p].y * layout.scales[p] / 2.0 - middle).abs();
        order.sort_by(|&a, &b| off_middle(a).total_cmp(&off_middle(b)));

        // Once they're all drawn, load ahead: the way the view is heading,
        // then back the other way.
        let scale_of = |p: usize| render_scale(doc.sizes[p], layout.scales[p], ppp, max_side);
        let settled = order.iter().all(|&p| !needs_render(doc, p, scale_of(p)));
        let ahead = if settled && !moving { pages_ahead(first, last, n, self.heading_down, LOOK_AHEAD) } else { Vec::new() };

        // Every page's drawing scale, for the helpers drawing ahead into the
        // page cache.
        let scales: Vec<f32> = (0..n).map(scale_of).collect();
        if *self.render_scales != scales {
            self.render_scales = Arc::new(scales);
        }
        // Drawing ahead and the highlight scan hold off while the view moves
        // or a zoom settles.
        let holding = moving || settling;
        if let Ok(mut wanted) = self.wanted.lock() {
            let pages: Vec<usize> = order.iter().chain(&ahead).copied().collect();
            if wanted.pages != pages || wanted.moving != holding {
                let unsettled: Vec<usize> = order.iter().copied().filter(|&p| needs_render(doc, p, scale_of(p))).collect();
                worker::trace(format_args!(
                    "ui: wanted {pages:?}, holding {holding}, heading down {}, in view but not drawn {unsettled:?}",
                    self.heading_down
                ));
            }
            wanted.generation = doc.generation;
            wanted.pages = pages;
            wanted.moving = holding;
            if !Arc::ptr_eq(&wanted.render_scales, &self.render_scales) {
                wanted.render_scales = Arc::clone(&self.render_scales);
            }
        }

        let mut tile_full_now: HashMap<usize, [u32; 2]> = HashMap::new();
        let mut sharp = true;
        for (i, &page) in order.iter().chain(&ahead).enumerate() {
            let in_view = i < order.len();
            let scale = render_scale(doc.sizes[page], layout.scales[page], ppp, max_side);
            let slow = doc.slow.contains(&page);
            // An earlier image of the page at this size comes straight back
            // from memory, as when zooming back out.
            if doc.textures.get(&page).is_none_or(|t| (t.scale - scale).abs() > 1e-3) {
                if let Some(spare) = doc.spares.remove(&(page, scale.to_bits())) {
                    if let Some(old) = doc.textures.insert(page, spare) {
                        if old.complete {
                            doc.spares.insert((page, old.scale.to_bits()), old);
                        }
                    }
                }
            }
            if moving {
                if let Some(&full) = doc.tile_full.get(&page) {
                    tile_full_now.insert(page, full);
                }
                sharp &= !in_view;
                continue;
            }
            if in_view && !doc.text.contains_key(&page) && doc.text_pending.insert(page) {
                let _ = self.tx.send(Request::Text { generation: doc.generation, page });
            }
            // A slow page already showing waits for the zoom to settle.
            let wait_for_zoom = settling && slow && doc.textures.contains_key(&page);
            if !wait_for_zoom && needs_render(doc, page, scale) && doc.render_pending.insert(page) {
                let _ = self.tx.send(Request::Render { generation: doc.generation, page, scale });
            }
            // The page's image counts towards a sharp view unless squares cover
            // the view, in which case it's only the backdrop to them.
            let page_unsharp = in_view && needs_render(doc, page, scale);
            if !in_view || doc.failed.contains(&page) {
                sharp &= !page_unsharp;
                continue;
            }

            // The part of the page in view is drawn on its own, at full
            // sharpness, when that's worth doing: zoomed in past what the
            // whole-page image holds, or zoomed in on a slow page, whose view
            // alone draws several times faster than the whole of it (0.2 s
            // against 1 s for a dense drawing at twice fit width). It's drawn
            // as squares on a grid, which are kept: zooming back in or scrolling
            // back over an area shows the squares already drawn. The squares in
            // view come first, then those in a margin around it.
            let want = layout.scales[page] * ppp;
            let size = doc.sizes[page] * layout.scales[page];
            let page_rect = Rect::from_min_size(pos2(page_x(content_w, size.x), tops[page]), size);
            let visible = page_rect.intersect(viewport);
            if !visible.is_positive() {
                sharp &= !page_unsharp;
                continue;
            }
            let past_whole_page = scale < want * 0.99;
            let zoomed_into_slow = slow && visible.area() < page_rect.area() * 0.5;
            if !(past_whole_page || zoomed_into_slow) {
                sharp &= !page_unsharp;
                continue;
            }
            // At a quarter power of two, so a small zoom change keeps its squares.
            let tile_scale = quantize_scale(want);
            let full = [
                (doc.sizes[page].x * tile_scale).round().max(1.0) as u32,
                (doc.sizes[page].y * tile_scale).round().max(1.0) as u32,
            ];
            let per_point = vec2(full[0] as f32 / size.x, full[1] as f32 / size.y);
            let span = |lo: f32, hi: f32, origin: f32, k: f32, limit: u32| {
                let a = (((lo - origin) * k).floor().max(0.0) as u32).min(limit);
                let b = (((hi - origin) * k).ceil().max(0.0) as u32).min(limit);
                (a, b)
            };
            let (x0, x1) = span(visible.min.x, visible.max.x, page_rect.min.x, per_point.x, full[0]);
            let (y0, y1) = span(visible.min.y, visible.max.y, page_rect.min.y, per_point.y, full[1]);
            let in_view_area = [x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0)];
            let (mx0, mx1) = (x0.saturating_sub(DETAIL_MARGIN), (x1 + DETAIL_MARGIN).min(full[0]));
            let (my0, my1) = (y0.saturating_sub(DETAIL_MARGIN), (y1 + DETAIL_MARGIN).min(full[1]));
            let around = [mx0, my0, mx1 - mx0, my1 - my0];
            for (column, row) in crate::model::tile_cells(full, around) {
                if let Some(tile) = doc.tiles.get_mut(&TileKey { page, full, column, row }) {
                    tile.used = now;
                }
            }
            let missing = |area: [u32; 4]| -> Vec<(u32, u32)> {
                crate::model::tile_cells(full, area)
                    .into_iter()
                    .filter(|&(column, row)| !doc.tiles.contains_key(&TileKey { page, full, column, row }))
                    .collect()
            };
            let in_view_missing = missing(in_view_area);
            if settling && slow && !in_view_missing.is_empty() {
                // Until the zoom settles, the squares from before it stay up --
                // unless every square in view at the new size is already here,
                // as when zooming back in.
                if let Some(&old) = doc.tile_full.get(&page) {
                    tile_full_now.insert(page, old);
                }
                sharp = false;
                continue;
            }
            tile_full_now.insert(page, full);
            if !in_view_missing.is_empty() {
                sharp = false;
            }
            if (settling && slow) || doc.detail_pending.contains(&page) {
                continue;
            }
            let mut cells = in_view_missing;
            if cells.is_empty() {
                cells = missing(around);
            }
            let (Some(c0), Some(c1), Some(r0), Some(r1)) = (
                cells.iter().map(|c| c.0).min(),
                cells.iter().map(|c| c.0).max(),
                cells.iter().map(|c| c.1).min(),
                cells.iter().map(|c| c.1).max(),
            ) else {
                continue;
            };
            // One region covering them, on square edges, no bigger than the
            // largest texture.
            let tile = crate::model::TILE;
            let most = (max_side as u32 / tile).max(1) * tile;
            let (left, top) = (c0 * tile, r0 * tile);
            let right = ((c1 + 1) * tile).min(full[0]).min(left + most);
            let bottom = ((r1 + 1) * tile).min(full[1]).min(top + most);
            if right <= left || bottom <= top {
                continue;
            }
            doc.detail_pending.insert(page);
            let region = [left, top, right - left, bottom - top];
            let _ = self.tx.send(Request::RenderRegion { generation: doc.generation, page, full, region });
        }
        doc.tile_full = tile_full_now;
        self.view_sharp = sharp;

        // Squares are kept while they fit their budget; those wanted on screen
        // longest ago go first.
        let tile_bytes = |t: &TileImage| {
            let [w, h] = t.handle.size();
            w * h * 4
        };
        let mut tile_total: usize = doc.tiles.values().map(tile_bytes).sum();
        if tile_total > self.tile_budget {
            let mut oldest: Vec<(f64, TileKey)> = doc.tiles.iter().map(|(key, t)| (t.used, *key)).collect();
            oldest.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (used, key) in oldest {
                if tile_total <= self.tile_budget || used >= now {
                    break;
                }
                if let Some(t) = doc.tiles.remove(&key) {
                    tile_total -= tile_bytes(&t);
                }
            }
        }
        // Spare page images only for pages near the view, and within a budget.
        doc.spares.retain(|(p, _), _| (lo..=hi).contains(p));
        let spare_total: usize = doc.spares.values().map(|t| t.handle.size()[0] * t.handle.size()[1] * 4).sum();
        if spare_total > self.spare_budget {
            doc.spares.clear();
        }
        // Traced whenever the textures held move by 32 MB, to see what the
        // process's memory is made of.
        {
            static LAST_STEP: AtomicU64 = AtomicU64::new(0);
            let page_total: usize = doc.textures.values().map(|t| t.handle.size()[0] * t.handle.size()[1] * 4).sum();
            let step = ((page_total + tile_total + spare_total) >> 25) as u64;
            if LAST_STEP.swap(step, Ordering::Relaxed) != step {
                worker::trace(format_args!(
                    "ui: textures held: {} MB of pages, {} MB in {} squares, {} MB of spares",
                    page_total >> 20,
                    tile_total >> 20,
                    doc.tiles.len(),
                    spare_total >> 20
                ));
            }
        }

        // Drawing ahead of a zoom. Ctrl+scroll zooms towards the pointer, and
        // Ctrl +/- towards the middle of the view, so once the view is sharp
        // and the pointer has rested, helpers with nothing else to do draw that
        // spot as the deepest zoom would show it: the whole page at the size it
        // would need, then the squares around the spot, nearest first. Zooming
        // in there is then sharp at once -- and at any zoom on the way, since
        // squares from a deeper zoom stand in until the zoom's own arrive.
        match (ctx.input(|i| i.pointer.hover_pos()), self.pointer_rest) {
            (Some(at), Some((resting, _))) if at.distance(resting) < 4.0 => {}
            (Some(at), _) => self.pointer_rest = Some((at, now)),
            (None, _) => self.pointer_rest = None,
        }
        doc.predicting.retain(|_, sent| now - *sent < PREDICT_TIMEOUT);
        let deepest = ZOOMS[ZOOMS.len() - 1];
        let rested = self.pointer_rest.is_none_or(|(_, since)| now - since >= PREDICT_REST);
        if sharp && !holding && rested && self.zoom < deepest * 0.99 && doc.predicting.len() < PREDICT_JOBS {
            let spot = self
                .pointer_rest
                .map(|(at, _)| at - origin.to_vec2())
                .filter(|at| viewport.contains(*at))
                .unwrap_or_else(|| viewport.center());
            let page_rect = |p: usize| {
                let size = doc.sizes[p] * layout.scales[p];
                Rect::from_min_size(pos2(page_x(content_w, size.x), tops[p]), size)
            };
            if let Some(page) = (first..=last).find(|&p| page_rect(p).contains(spot)) {
                let rect = page_rect(page);
                let deep = deepest * layout.scales[page] / self.zoom;

                // The squares around the spot first, since they're what the zoom
                // would show: the view it would land on, then half a view
                // further each way once that's done.
                let mut landing_missing = false;
                let tile_scale = quantize_scale(deep * ppp);
                let full = [
                    (doc.sizes[page].x * tile_scale).round().max(1.0) as u32,
                    (doc.sizes[page].y * tile_scale).round().max(1.0) as u32,
                ];
                let edge = vec2(full[0] as f32, full[1] as f32);
                let fraction = (spot - rect.min) / rect.size();
                let centre = vec2(fraction.x * edge.x, fraction.y * edge.y);
                let offset = (spot - viewport.min) * ppp;
                let view = viewport.size() * ppp;
                let tile = crate::model::TILE;
                let spot_cell = ((centre.x.max(0.0) as u32) / tile, (centre.y.max(0.0) as u32) / tile);
                for grow in [0.0_f32, 0.5] {
                    let margin = vec2(DETAIL_MARGIN as f32, DETAIL_MARGIN as f32) + view * grow;
                    let from = (centre - offset - margin).max(Vec2::ZERO);
                    let to = (centre - offset + view + margin).min(edge);
                    if to.x <= from.x || to.y <= from.y {
                        break;
                    }
                    let area = [from.x as u32, from.y as u32, (to.x - from.x) as u32, (to.y - from.y) as u32];
                    let mut missing: Vec<(u32, u32)> = crate::model::tile_cells(full, area)
                        .into_iter()
                        .filter(|&(column, row)| !doc.tiles.contains_key(&TileKey { page, full, column, row }))
                        .collect();
                    if grow == 0.0 {
                        landing_missing = !missing.is_empty();
                    }
                    if missing.is_empty() {
                        continue;
                    }
                    missing.sort_by_key(|&(column, row)| column.abs_diff(spot_cell.0).max(row.abs_diff(spot_cell.1)));
                    for (column, row) in missing {
                        if doc.predicting.len() >= PREDICT_JOBS {
                            break;
                        }
                        let (bx, by) = (column / PREDICT_BLOCK * PREDICT_BLOCK * tile, row / PREDICT_BLOCK * PREDICT_BLOCK * tile);
                        let block = [bx, by, (PREDICT_BLOCK * tile).min(full[0] - bx), (PREDICT_BLOCK * tile).min(full[1] - by)];
                        let key = PredictKey::Region(page, block);
                        if doc.predicting.contains_key(&key) {
                            continue;
                        }
                        doc.predicting.insert(key, now);
                        let _ = self.tx.send(Request::PredictRegion { generation: doc.generation, page, full, region: block });
                    }
                    // The nearer squares first; the further ones once they're done.
                    break;
                }

                // Then, once the view the zoom would land on is covered, the
                // whole page at the size it would be drawn at: the backdrop to
                // the squares, which on an overlay takes seconds on its own.
                let page_scale = render_scale(doc.sizes[page], deep, ppp, max_side);
                let key = PredictKey::Page(page, page_scale.to_bits());
                let have_page = doc.textures.get(&page).is_some_and(|t| t.complete && (t.scale - page_scale).abs() <= 1e-3)
                    || doc.spares.contains_key(&(page, page_scale.to_bits()));
                if !landing_missing && !have_page && doc.predicting.len() < PREDICT_JOBS && !doc.predicting.contains_key(&key) {
                    doc.predicting.insert(key, now);
                    let _ = self.tx.send(Request::PredictPage { generation: doc.generation, page, scale: page_scale });
                }
            }
        }

        // Let go of page images that have scrolled away: always keep the ones
        // asked for above, then the nearest others while they fit the budget.
        let distance = |p: usize| if p < first { first - p } else { p.saturating_sub(last) };
        let mut others: Vec<(usize, usize)> =
            doc.textures.keys().filter(|p| !(lo..=hi).contains(*p)).map(|&p| (distance(p), p)).filter(|(d, _)| *d <= 4).collect();
        others.sort_unstable();
        let mut kept: HashSet<usize> = (lo..=hi).collect();
        let mut bytes = 0;
        for (_, p) in others {
            let [w, h] = doc.textures[&p].handle.size();
            if bytes + w * h * 4 <= TEXTURE_BUDGET {
                bytes += w * h * 4;
                kept.insert(p);
            }
        }
        doc.textures.retain(|p, _| kept.contains(p));
        let keep = first.saturating_sub(12)..=last + 12;
        doc.text.retain(|p, _| keep.contains(p));

        let painter = ui.painter();
        let screen_view = viewport.translate(origin.to_vec2());
        let page_shadow = Shadow { offset: [0, 2], blur: 12, spread: 0, color: Color32::from_black_alpha(34) };
        for page in first..=last {
            let scale = layout.scales[page];
            let size = doc.sizes[page] * scale;
            let rect = Rect::from_min_size(origin + vec2(page_x(content_w, size.x), tops[page]), size);
            self.page_rects.insert(page, rect);
            let geometry = doc.geometry[page];

            painter.add(page_shadow.as_shape(rect, CornerRadius::same(2)));

            // While zooming, the old texture stretches to fit until the sharp
            // one arrives, which beats flashing a blank page.
            let texture: Option<TextureId> = doc.textures.get(&page).map(|t| t.handle.id());
            match texture {
                Some(id) => {
                    painter.image(id, rect, UV_FULL, Color32::WHITE);
                }
                None => {
                    painter.rect_filled(rect, CornerRadius::same(0), Color32::WHITE);
                    painter.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        format!("Page {}", page + 1),
                        FontId::proportional(12.0),
                        SUBTLE,
                    );
                }
            }
            // Squares drawn zoomed in, over the whole-page image.
            // Squares from other zooms stand in until this zoom's own arrive:
            // coarser ones first, sharper ones over them, this zoom's on top.
            let detail: Vec<(TextureId, Rect)> = match doc.tile_full.get(&page) {
                Some(&full) => {
                    let mut pieces: Vec<(u32, TextureId, Rect)> = doc
                        .tiles
                        .iter()
                        .filter(|(key, _)| key.page == page)
                        .map(|(key, tile)| (key.full[0], tile.handle.id(), tile_screen_rect(rect, key.full, key.column, key.row)))
                        .filter(|(_, _, area)| area.intersects(screen_view))
                        .collect();
                    pieces.sort_by_key(|&(width, ..)| if width == full[0] { u32::MAX } else { width });
                    pieces.into_iter().map(|(_, id, area)| (id, area)).collect()
                }
                None => Vec::new(),
            };
            for &(id, area) in &detail {
                painter.image(id, area, UV_FULL, Color32::WHITE);
            }
            painter.rect_stroke(rect, CornerRadius::same(0), Stroke::new(1.0, Color32::from_black_alpha(14)), StrokeKind::Outside);

            if let Some(g) = geometry {
                for e in doc.highlights.iter().filter(|e| e.hl.page == page) {
                    for q in &e.hl.quads {
                        let r = to_screen(rect, &g, q).intersect(rect);
                        if !r.is_positive() {
                            continue;
                        }
                        paint_highlight(painter, texture, &detail, rect, r, to_color32(e.hl.color));
                        if active == Some(e.uid) {
                            painter.rect_stroke(r.expand(1.0), CornerRadius::same(2), Stroke::new(2.0, ACCENT), StrokeKind::Outside);
                        }
                    }
                }

                // Search matches, found by binary search since they're in page order.
                let hits = &self.search.hits;
                let from = hits.partition_point(|h| h.page < page);
                let to = hits.partition_point(|h| h.page <= page);
                for (i, hit) in hits.iter().enumerate().take(to).skip(from) {
                    let current = self.search.current == Some(i);
                    for q in &hit.quads {
                        let r = to_screen(rect, &g, q).intersect(rect);
                        if !r.is_positive() {
                            continue;
                        }
                        paint_highlight(painter, texture, &detail, rect, r, if current { HIT_CURRENT } else { HIT });
                        if current {
                            painter.rect_stroke(r.expand(1.5), CornerRadius::same(2), Stroke::new(2.0, HIT_OUTLINE), StrokeKind::Outside);
                        }
                    }
                }

                if let Some(chars) = doc.text.get(&page) {
                    for (_, range) in segments.iter().filter(|(p, _)| *p == page) {
                        for band in selection::bands(chars, range.clone()) {
                            painter.rect_filled(to_screen(rect, &g, &band), CornerRadius::same(0), SELECTION);
                        }
                    }
                }

                // The box being drawn, with Ctrl held.
                if let Some(Drag::Box { page: box_page, start, end }) = self.drag {
                    if box_page == page {
                        let area = to_screen(rect, &g, &box_between(start, end));
                        painter.rect_filled(area, CornerRadius::same(0), ACCENT.gamma_multiply(0.06));
                        painter.rect_stroke(area, CornerRadius::same(0), Stroke::new(1.0, ACCENT), StrokeKind::Inside);
                    }
                }
            }

            if let Some(chars) = doc.text.get(&page) {
                if !chars.iter().any(|c| !c.ch.is_whitespace() && !c.ch.is_control()) {
                    let strip = Rect::from_min_max(pos2(rect.min.x, rect.max.y - 28.0), rect.max);
                    painter.rect_filled(strip, CornerRadius::same(0), Color32::from_rgba_unmultiplied(254, 243, 199, 245));
                    painter.text(
                        strip.left_center() + vec2(10.0, 0.0),
                        Align2::LEFT_CENTER,
                        "This page has no text layer (it is a scanned image), so text cannot be selected.",
                        FontId::proportional(12.0),
                        Color32::from_rgb(0x78, 0x35, 0x0f),
                    );
                }
            }

            if !busy {
                let response = ui.interact(rect, Id::new(("page", page)), Sense::click_and_drag());
                if let (Some(pos), Some(g)) = (response.hover_pos(), geometry) {
                    let (px, py) = to_pdf(rect, &g, pos);
                    let over_highlight =
                        doc.highlights.iter().any(|e| e.hl.page == page && e.hl.quads.iter().any(|q| q.contains(px, py)));
                    let over_text = doc
                        .text
                        .get(&page)
                        .is_some_and(|chars| chars.iter().any(|c| c.bounds.is_some_and(|b| b.contains(px, py))));
                    if ctx.input(|i| i.modifiers.command) {
                        // Ctrl held: dragging draws a box.
                        ctx.set_cursor_icon(CursorIcon::Crosshair);
                    } else if over_highlight {
                        ctx.set_cursor_icon(CursorIcon::PointingHand);
                    } else if over_text {
                        ctx.set_cursor_icon(CursorIcon::Text);
                    }
                }
                // Only the left button selects text or opens a highlight; the
                // middle button is for moving the document.
                if response.drag_started_by(egui::PointerButton::Primary) {
                    drag_start = ctx.input(|i| i.pointer.press_origin()).map(|pos| (page, pos));
                }
                if response.clicked_by(egui::PointerButton::Primary) {
                    clicked = response.interact_pointer_pos().map(|pos| (page, pos));
                }
            }

            // Oversized pages say how they're shown, and switch between shrunk
            // and actual size. Registered after the page, so it gets the click.
            if doc.sizes[page].x > doc.usual_size.x * OVERSIZED {
                let label = if shrink_wide {
                    format!("Shrunk to fit, {:.0}%  ·  Show actual size", scale * 100.0)
                } else {
                    "Actual size  ·  Shrink to fit".to_owned()
                };
                if size_badge(ui, rect, page, &label).clicked() {
                    toggle_shrink = true;
                }
            }
        }

        if toggle_shrink {
            self.set_shrink_wide(!shrink_wide);
        }
        if let Some((page, pos)) = drag_start {
            // With Ctrl held, the drag draws a box instead of following the text.
            if ctx.input(|i| i.modifiers.command) {
                if let Some(point) = self.pdf_point(page, pos) {
                    self.drag = Some(Drag::Box { page, start: point, end: point });
                    self.popup = None;
                }
            } else if let Some(caret) = self.caret_for(page, pos) {
                self.drag = Some(Drag::Text { anchor: (page, caret), focus: (page, caret) });
                self.popup = None;
            }
        }
        if self.drag.is_some() {
            self.update_drag(ui);
        }
        if let Some((page, pos)) = clicked {
            self.click_page(page, pos);
        }
    }

    fn caret_for(&self, page: usize, pos: Pos2) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        let rect = self.page_rects.get(&page)?;
        let geometry = doc.geometry.get(page).copied().flatten()?;
        let chars = doc.text.get(&page)?;
        let (x, y) = to_pdf(*rect, &geometry, pos);
        selection::caret_at(chars, x, y)
    }

    /// A point on screen as a point on `page`, in PDF user space.
    fn pdf_point(&self, page: usize, pos: Pos2) -> Option<(f32, f32)> {
        let doc = self.doc.as_ref()?;
        let rect = self.page_rects.get(&page)?;
        let geometry = doc.geometry.get(page).copied().flatten()?;
        Some(to_pdf(*rect, &geometry, pos))
    }

    fn update_drag(&mut self, ui: &Ui) {
        let (pos, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));

        if let Some(pos) = pos {
            if let Some(Drag::Box { page, .. }) = self.drag {
                // A box stays on the page it started on.
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                if let (Some(point), Some(Drag::Box { end, .. })) = (self.pdf_point(page, pos), self.drag.as_mut()) {
                    *end = point;
                }
            } else {
                // The page under the pointer, or the nearest one when it's in
                // the gap between two.
                let distance = |r: &Rect| {
                    if pos.y < r.min.y {
                        r.min.y - pos.y
                    } else if pos.y > r.max.y {
                        pos.y - r.max.y
                    } else {
                        0.0
                    }
                };
                let nearest = self.page_rects.iter().min_by(|a, b| distance(a.1).total_cmp(&distance(b.1))).map(|(p, _)| *p);
                if let Some(page) = nearest {
                    if let (Some(caret), Some(Drag::Text { focus, .. })) = (self.caret_for(page, pos), self.drag.as_mut()) {
                        *focus = (page, caret);
                    }
                }
            }

            // Dragging past an edge of the view scrolls.
            let view = self.viewer_rect;
            if down && view.is_positive() {
                let edge = |p: f32, min: f32, max: f32| {
                    if p > max - 12.0 {
                        -(p - (max - 12.0)).min(40.0)
                    } else if p < min + 12.0 {
                        ((min + 12.0) - p).min(40.0)
                    } else {
                        0.0
                    }
                };
                let delta = vec2(edge(pos.x, view.min.x, view.max.x), edge(pos.y, view.min.y, view.max.y));
                if delta != Vec2::ZERO {
                    ui.scroll_with_delta(delta);
                }
            }
        }

        if down {
            ui.ctx().request_repaint();
            return;
        }

        // Released: copy what was selected, and offer to highlight it.
        let Some(drag) = self.drag.take() else { return };
        let Some(doc) = &self.doc else { return };
        let segments = drag_segments(&doc.text, &drag);

        let copied = segments
            .iter()
            .filter_map(|(page, range)| Some(selection::copy_text(doc.text.get(page)?, range.clone())))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !copied.is_empty() {
            ui.ctx().copy_text(copied);
        }

        // One pending highlight per page, from all of that page's runs.
        let mut by_page: Vec<(usize, Vec<Range<usize>>)> = Vec::new();
        for (page, range) in segments {
            match by_page.last_mut() {
                Some((last, ranges)) if *last == page => ranges.push(range),
                _ => by_page.push((page, vec![range])),
            }
        }
        let pending: Vec<Pending> = by_page
            .into_iter()
            .filter_map(|(page, ranges)| {
                let chars = doc.text.get(&page)?;
                let quads = selection::bands_of(chars, &ranges);
                let text = ranges
                    .iter()
                    .map(|range| selection::text(chars, range.clone()))
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                (!quads.is_empty()).then_some(Pending { page, quads, text })
            })
            .collect();

        // Pin the popup under the last line highlighted.
        let Some(anchor) = pending.last().and_then(|p| {
            let q = p.quads.last()?;
            Some(Anchor { page: p.page, x: (q.left + q.right) / 2.0, y: q.bottom })
        }) else {
            return;
        };

        self.active = None;
        self.popup = Some(Popup {
            mode: PopupMode::Create(pending),
            anchor,
            color: COLORS[0].1,
            note: String::new(),
            just_opened: true,
            height: 250.0,
        });
    }

    /// A plain click on an existing highlight opens its note.
    fn click_page(&mut self, page: usize, pos: Pos2) {
        let Some(doc) = &self.doc else { return };
        let Some(rect) = self.page_rects.get(&page) else { return };
        let Some(geometry) = doc.geometry[page] else { return };
        let (x, y) = to_pdf(*rect, &geometry, pos);
        let hit = doc
            .highlights
            .iter()
            .rev()
            .find(|e| e.hl.page == page && e.hl.quads.iter().any(|q| q.contains(x, y)))
            .map(|e| e.uid);
        match hit {
            Some(uid) => {
                // Pin it under the bottom of the clicked highlight, not at the
                // pointer, so it never covers the text it's about.
                let bottom = doc
                    .highlights
                    .iter()
                    .find(|e| e.uid == uid)
                    .and_then(|e| e.hl.quads.iter().find(|q| q.contains(x, y)))
                    .map_or(y, |q| q.bottom);
                self.open_edit_popup(uid, Anchor { page, x, y: bottom });
            }
            None => {
                self.popup = None;
                self.active = None;
            }
        }
    }

    /* -------------------------------------------------------------- *
     * The popup: create a highlight, or edit an existing one
     * -------------------------------------------------------------- */

    fn open_edit_popup(&mut self, uid: u64, anchor: Anchor) {
        let Some(entry) = self.entry(uid) else { return };
        self.popup = Some(Popup {
            mode: PopupMode::Edit(uid),
            anchor,
            color: entry.hl.color,
            note: entry.hl.comment.clone(),
            just_opened: true,
            height: 250.0,
        });
        self.active = Some(uid);
    }

    /// Scroll a highlight into view and open its note.
    fn reveal(&mut self, uid: u64) {
        let Some(entry) = self.entry(uid) else { return };
        let Some(q) = entry.hl.quads.first().copied() else { return };
        let page = entry.hl.page;
        self.scroll_to_box(page, &q);
        let anchor = Anchor { page, x: (q.left + q.right) / 2.0, y: q.bottom };
        self.open_edit_popup(uid, anchor);
    }

    fn show_popup(&mut self, ctx: &egui::Context) {
        let Some(popup) = &self.popup else { return };
        let Some(doc) = &self.doc else { return };
        // Its page scrolled out of view: hide it until the page comes back.
        let Some(rect) = self.page_rects.get(&popup.anchor.page).copied() else { return };
        let Some(geometry) = doc.geometry[popup.anchor.page] else { return };

        // What the popup is about.
        let (title, quote, meta, create, color_locked) = match &popup.mode {
            PopupMode::Create(pending) => {
                let quote = pending.iter().map(|p| p.text.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ");
                let first = pending.first().map_or(0, |p| p.page) + 1;
                let last = pending.last().map_or(0, |p| p.page) + 1;
                let meta = if first == last { format!("Page {first}") } else { format!("Pages {first}-{last}") };
                ("New highlight", quote, meta, true, false)
            }
            PopupMode::Edit(uid) => {
                let Some(entry) = self.entry(*uid) else { return };
                let mut meta = format!("Page {}", entry.hl.page + 1);
                if !entry.hl.author.is_empty() {
                    meta += &format!(" · {}", entry.hl.author);
                }
                ("Note", entry.hl.snippet.clone(), meta, false, !entry.is_new())
            }
        };
        let (fx, fy) = geometry.to_view(popup.anchor.x, popup.anchor.y);
        let point = pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height());

        // Read before the text box sees them: Esc cancels, Ctrl+Enter saves.
        let (escape, commit_key) = ctx.input_mut(|i| {
            (i.consume_key(Modifiers::NONE, Key::Escape), i.consume_key(Modifiers::COMMAND, Key::Enter))
        });

        // Below the text if it fits, otherwise above it.
        let screen = ctx.content_rect();
        let gap = 14.0;
        let below = point.y + gap + popup.height <= screen.max.y - 8.0 || point.y - gap - popup.height < screen.min.y + 8.0;
        let x = (point.x - POPUP_WIDTH / 2.0)
            .clamp(screen.min.x + 8.0, (screen.max.x - POPUP_WIDTH - 8.0).max(screen.min.x + 8.0));
        let y = if below { point.y + gap } else { point.y - gap - popup.height };

        let Some(popup) = self.popup.as_mut() else { return };
        let mut action = PopupAction::None;
        let area_id = Id::new("highlight-popup");

        let area = egui::Area::new(area_id).order(Order::Foreground).fixed_pos(pos2(x, y)).show(ctx, |ui| {
            Frame::NONE
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(14))
                .shadow(soft_shadow())
                .show(ui, |ui| {
                    ui.set_width(POPUP_WIDTH - 30.0);
                    ui.spacing_mut().item_spacing = vec2(8.0, 10.0);

                    // Header: the colour, what this is, where it is, and a close button.
                    ui.horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 6.0, to_color32(popup.color));
                        ui.painter().circle_stroke(dot.center(), 6.0, Stroke::new(1.0, Color32::from_black_alpha(30)));
                        ui.label(RichText::new(title).size(14.0).strong().color(TEXT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let close = paint_button(ui, "×", FontId::proportional(17.0), Tone::Ghost, false, vec2(26.0, 26.0));
                            if close.on_hover_text("Close (Esc)").clicked() {
                                action = PopupAction::Cancel;
                            }
                            ui.label(RichText::new(&meta).size(12.0).color(MUTED));
                        });
                    });

                    if !quote.is_empty() {
                        quote_card(ui, &quote, to_color32(popup.color));
                    }

                    if color_locked {
                        ui.label(RichText::new("Saved highlights keep their colour.").size(12.0).color(MUTED));
                    } else {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            ui.label(RichText::new("Colour").size(12.0).color(MUTED));
                            ui.add_space(8.0);
                            for (name, rgb) in COLORS {
                                let selected = rgb.iter().zip(popup.color).all(|(a, b)| (a - b).abs() < 0.02);
                                if swatch(ui, rgb, selected).on_hover_text(name).clicked() {
                                    popup.color = rgb;
                                }
                            }
                        });
                    }

                    let note = ui.add(
                        TextEdit::multiline(&mut popup.note)
                            .hint_text("Add a note (optional)…")
                            .desired_rows(3)
                            .desired_width(f32::INFINITY)
                            .margin(Margin::symmetric(10, 8)),
                    );
                    if popup.just_opened {
                        note.request_focus();
                        popup.just_opened = false;
                    }

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if !create && styled_button(ui, "Delete", Tone::Danger, false).clicked() {
                            action = PopupAction::Delete;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if styled_button(ui, if create { "Highlight" } else { "Save" }, Tone::Primary, false).clicked() {
                                action = PopupAction::Commit;
                            }
                            if styled_button(ui, "Cancel", Tone::Secondary, false).clicked() {
                                action = PopupAction::Cancel;
                            }
                        });
                    });

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        keycap(ui, "Ctrl+Enter");
                        ui.label(RichText::new("to save").size(11.5).color(MUTED));
                        ui.add_space(6.0);
                        keycap(ui, "Esc");
                        ui.label(RichText::new("to cancel").size(11.5).color(MUTED));
                    });
                });
        });
        popup.height = area.response.rect.height();

        // A small pointer from the card to the text it's about, drawn on the
        // card's own layer so it sits on top of the card's border.
        let card = area.response.rect;
        let nub_x = point.x.clamp(card.min.x + 22.0, card.max.x - 22.0);
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, area_id));
        let (edge, tip, fill_base) = if below {
            (card.min.y, card.min.y - 7.0, card.min.y + 1.5)
        } else {
            (card.max.y, card.max.y + 7.0, card.max.y - 1.5)
        };
        // Clockwise on screen, for clean anti-aliasing.
        let points = if below {
            vec![pos2(nub_x - 8.0, fill_base), pos2(nub_x, tip), pos2(nub_x + 8.0, fill_base)]
        } else {
            vec![pos2(nub_x + 8.0, fill_base), pos2(nub_x, tip), pos2(nub_x - 8.0, fill_base)]
        };
        painter.add(Shape::convex_polygon(points, SURFACE, Stroke::NONE));
        let border = Stroke::new(1.0, BORDER);
        painter.line_segment([pos2(nub_x - 8.0, edge), pos2(nub_x, tip)], border);
        painter.line_segment([pos2(nub_x, tip), pos2(nub_x + 8.0, edge)], border);

        let action = if escape {
            PopupAction::Cancel
        } else if commit_key {
            PopupAction::Commit
        } else {
            action
        };
        match action {
            PopupAction::Commit => self.commit_popup(),
            PopupAction::Cancel => self.popup = None,
            PopupAction::Delete => {
                if let Some(PopupMode::Edit(uid)) = self.popup.as_ref().map(|p| &p.mode) {
                    let uid = *uid;
                    self.remove(uid);
                }
            }
            PopupAction::None => {}
        }
    }

    fn commit_popup(&mut self) {
        let Some(popup) = self.popup.take() else { return };
        let author = self.author_name();
        let Some(doc) = self.doc.as_mut() else { return };

        match popup.mode {
            PopupMode::Create(pending) => {
                for p in pending {
                    doc.highlights.push(Entry {
                        uid: next_uid(),
                        hl: Highlight {
                            key: None,
                            page: p.page,
                            quads: p.quads,
                            color: popup.color,
                            comment: popup.note.clone(),
                            author: author.clone(),
                            snippet: p.text,
                        },
                    });
                }
            }
            PopupMode::Edit(uid) => {
                let Some(entry) = doc.highlights.iter_mut().find(|e| e.uid == uid) else { return };
                entry.hl.comment = popup.note.clone();
                match entry.hl.key {
                    // A colour change only applies to highlights not yet in the
                    // file. A saved one may carry an appearance stream written
                    // by another viewer, which would keep showing the old colour.
                    None => entry.hl.color = popup.color,
                    Some(key) => {
                        doc.edits.insert(key, popup.note);
                    }
                }
            }
        }
        doc.dirty = true;
    }

    fn remove(&mut self, uid: u64) {
        self.popup = None;
        if self.active == Some(uid) {
            self.active = None;
        }
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(index) = doc.highlights.iter().position(|e| e.uid == uid) else { return };
        let entry = doc.highlights.remove(index);
        if let Some(key) = entry.hl.key {
            doc.deletes.push(key);
            doc.edits.remove(&key);
        }
        doc.dirty = true;
    }

    /* -------------------------------------------------------------- *
     * Side panels
     * -------------------------------------------------------------- */

    /// A white panel on the right with a fixed header, and optionally a fixed
    /// footer, around a body that fills the rest.
    fn side_panel(
        &mut self,
        ui: &mut Ui,
        id: &str,
        header: impl FnOnce(&mut Self, &mut Ui),
        footer: Option<fn(&mut Self, &mut Ui)>,
        body: impl FnOnce(&mut Self, &mut Ui),
    ) {
        egui::Panel::right(Id::new(id))
            .frame(Frame::NONE.fill(SURFACE))
            .default_size(320.0)
            .min_size(220.0)
            .show(ui, |ui| {
                let margin = Frame::NONE.inner_margin(Margin::symmetric(14, 12));
                egui::Panel::top(Id::new((id, "head"))).frame(margin).show(ui, |ui| header(self, ui));
                if let Some(footer) = footer {
                    egui::Panel::bottom(Id::new((id, "foot"))).frame(margin).show(ui, |ui| footer(self, ui));
                }
                egui::CentralPanel::default().frame(Frame::NONE).show(ui, |ui| body(self, ui));
            });
    }

    fn notes_panel(&mut self, ui: &mut Ui) {
        self.side_panel(
            ui,
            "notes",
            |app, ui| {
                let (count, done) = app.doc.as_ref().map_or((0, true), |d| (d.highlights.len(), d.highlights_done));
                let detail = match (count, done) {
                    (0, true) => String::new(),
                    (_, true) => format!("{count} in this file"),
                    (_, false) => format!("{count} so far, still reading…"),
                };
                panel_heading(ui, "Notes", detail);
            },
            Some(|app, ui| {
                ui.label(RichText::new("Your name on new notes").size(12.0).color(MUTED));
                let name = ui.add(
                    TextEdit::singleline(&mut app.author)
                        .hint_text("me")
                        .char_limit(60)
                        .desired_width(f32::INFINITY)
                        .margin(Margin::symmetric(8, 6)),
                );
                if name.changed() {
                    save_author(&app.author);
                }
            }),
            |app, ui| {
                egui::ScrollArea::vertical().id_salt("notes-list").auto_shrink(false).show(ui, |ui| {
                    app.note_rows(ui);
                });
            },
        );
    }

    fn note_rows(&mut self, ui: &mut Ui) {
        let Some(doc) = &self.doc else {
            empty_note(ui, "Open a PDF to see its highlights.");
            return;
        };
        if doc.highlights.is_empty() {
            let message = if doc.highlights_done {
                "No highlights yet. Drag across some text to make one."
            } else {
                "Reading highlights…"
            };
            empty_note(ui, message);
            return;
        }

        let mut open = None;
        let mut delete = None;
        let clicked = ui.input(|i| i.pointer.primary_clicked());

        for e in &doc.highlights {
            let fill = if self.active == Some(e.uid) { NOTE_ACTIVE } else { SURFACE };
            let row = Frame::NONE.fill(fill).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let del = ui
                    .horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 5.0, to_color32(e.hl.color));
                        let mut label = format!("Page {}", e.hl.page + 1);
                        if !e.hl.author.is_empty() {
                            label += &format!(" · {}", e.hl.author);
                        }
                        if e.is_new() {
                            label += " · unsaved";
                        }
                        ui.label(RichText::new(label).size(12.0).color(MUTED));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            paint_button(ui, "×", FontId::proportional(16.0), Tone::Ghost, false, vec2(24.0, 24.0))
                                .on_hover_text("Delete this highlight")
                        })
                        .inner
                    })
                    .inner;
                if !e.hl.snippet.is_empty() {
                    quote_block(ui, &e.hl.snippet, 110);
                }
                if e.hl.comment.is_empty() {
                    ui.label(RichText::new("No note").italics().color(SUBTLE));
                } else {
                    ui.label(RichText::new(e.hl.comment.as_str()).color(TEXT));
                }
                del
            });

            let rect = row.response.rect;
            ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, ROW_RULE));
            if row.inner.clicked() {
                delete = Some(e.uid);
            } else if clicked && ui.rect_contains_pointer(rect) && !row.inner.hovered() {
                open = Some(e.uid);
            }
        }

        if let Some(uid) = delete {
            self.remove(uid);
        } else if let Some(uid) = open {
            self.reveal(uid);
        }
    }

    fn results_panel(&mut self, ui: &mut Ui) {
        self.side_panel(
            ui,
            "results",
            |app, ui| panel_heading(ui, "Search results", app.results_summary()),
            None,
            |app, ui| app.result_rows(ui),
        );
    }

    /// e.g. "37 on 12 pages", with progress while the search is running.
    fn results_summary(&self) -> String {
        let s = &self.search;
        if selection::normalize_query(&s.sent).is_empty() {
            return String::new();
        }
        let mut summary = if s.hits.is_empty() {
            "No matches".to_owned()
        } else {
            let pages = 1 + s.hits.windows(2).filter(|w| w[0].page != w[1].page).count();
            format!("{} on {pages} page{}", self.match_count(), if pages == 1 { "" } else { "s" })
        };
        if !s.done {
            if s.hits.is_empty() {
                summary = "Searching".to_owned();
            }
            summary += &format!(" · {}%", self.search_progress());
        }
        summary
    }

    fn result_rows(&mut self, ui: &mut Ui) {
        let query = self.search.sent.trim().to_owned();
        if query.is_empty() {
            let hint = if self.doc.is_some() {
                "Type in the find box (Ctrl+F) and every match is listed here."
            } else {
                "Open a PDF to search it."
            };
            empty_note(ui, hint);
            return;
        }
        if self.search.done && self.search.hits.is_empty() {
            empty_note(ui, &format!("No matches for “{query}”."));
            return;
        }

        let mut area = egui::ScrollArea::vertical().id_salt("results-list").auto_shrink(false);
        if let Some(i) = self.search.reveal_row.take() {
            // Only scroll when the row isn't already fully in view.
            let top = i as f32 * RESULT_ROW_HEIGHT;
            let (offset, height) = (self.search.list_offset, self.search.list_height);
            if height <= 0.0 || top < offset || top + RESULT_ROW_HEIGHT > offset + height {
                area = area.vertical_scroll_offset((top - (height - RESULT_ROW_HEIGHT).max(0.0) / 2.0).max(0.0));
            }
            self.search.list_scroll_to = None;
        } else if let Some(y) = self.search.list_scroll_to.take() {
            area = area.vertical_scroll_offset(y);
        }

        ui.spacing_mut().item_spacing.y = 0.0;
        let search = &self.search;
        let mut clicked = None;
        let output = area.show_rows(ui, RESULT_ROW_HEIGHT, search.hits.len(), |ui, rows| {
            for i in rows {
                if result_row(ui, &search.hits[i], search.current == Some(i)).clicked() {
                    clicked = Some(i);
                }
            }
        });
        self.search.list_offset = output.state.offset.y;
        self.search.list_height = output.inner_rect.height();

        if let Some(i) = clicked {
            self.go_to_hit(i);
        }
    }

    /* -------------------------------------------------------------- *
     * Toast
     * -------------------------------------------------------------- */

    fn show_toast(&mut self, ctx: &egui::Context) {
        let now = Self::now(ctx);
        let Some((message, until)) = &self.toast else { return };
        if now > *until {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_secs_f64(until - now));
        egui::Area::new(Id::new("toast"))
            .order(Order::Tooltip)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -28.0))
            .interactable(false)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(TEXT)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(16, 10))
                    .shadow(soft_shadow())
                    .show(ui, |ui| {
                        ui.label(RichText::new(message.as_str()).color(Color32::WHITE));
                    });
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_replies(&ctx);
        self.handle_close(&ctx);
        self.handle_input(&ctx);
        self.update_search(&ctx);

        self.toolbar(ui);
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
    }
}

/* ------------------------------------------------------------------ *
 * Layout, geometry and drawing helpers
 * ------------------------------------------------------------------ */

/// The page size most pages share (to the nearest point), or the first page's
/// on a tie. Fit-to-width fits this, and pages much wider are "oversized".
fn usual_page_size(sizes: &[Vec2]) -> Vec2 {
    let mut counts: HashMap<(i32, i32), (usize, usize)> = HashMap::new();
    for (i, s) in sizes.iter().enumerate() {
        counts.entry((s.x.round() as i32, s.y.round() as i32)).or_insert((0, i)).0 += 1;
    }
    counts
        .values()
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map_or(vec2(612.0, 792.0), |&(_, i)| sizes[i])
}

/// Where every page goes at `zoom`, with oversized pages shrunk to the usual
/// page width if `shrink_wide`.
fn page_layout(sizes: &[Vec2], usual: Vec2, zoom: f32, shrink_wide: bool) -> PageLayout {
    let mut y = TOP_PAD;
    let mut tops = Vec::with_capacity(sizes.len());
    let mut scales = Vec::with_capacity(sizes.len());
    let mut widest = 0.0f32;
    for s in sizes {
        let shrink = if shrink_wide && s.x > usual.x * OVERSIZED { usual.x / s.x } else { 1.0 };
        let scale = zoom * shrink;
        tops.push(y);
        scales.push(scale);
        widest = widest.max(s.x * scale);
        y += s.y * scale + PAGE_GAP;
    }
    PageLayout { tops, scales, widest, height: y - PAGE_GAP + BOTTOM_PAD }
}

/// The scrolling column is the view's width, or the widest page plus margins
/// if that's wider.
fn content_width(view_width: f32, widest: f32) -> f32 {
    view_width.max(widest + 2.0 * SIDE_PAD)
}

/// A page is centred in the column.
fn page_x(content_width: f32, page_width: f32) -> f32 {
    ((content_width - page_width) / 2.0).max(SIDE_PAD)
}

/// A stand-in until the worker reports a page's real geometry: unrotated,
/// origin at the corner. Only used to choose where to scroll.
fn flat_geometry(size: Vec2) -> PageGeometry {
    PageGeometry { rotation: 0, bounds: PdfBox { left: 0.0, bottom: 0.0, right: size.x, top: size.y } }
}

/// A box in page space to its rectangle on screen.
fn to_screen(page: Rect, geometry: &PageGeometry, b: &PdfBox) -> Rect {
    let (left, top, right, bottom) = geometry.box_to_view(b);
    Rect::from_min_max(
        pos2(page.min.x + left * page.width(), page.min.y + top * page.height()),
        pos2(page.min.x + right * page.width(), page.min.y + bottom * page.height()),
    )
}

/// A point on screen to page space.
fn to_pdf(page: Rect, geometry: &PageGeometry, p: Pos2) -> (f32, f32) {
    geometry.from_view((p.x - page.min.x) / page.width(), (p.y - page.min.y) / page.height())
}

/// Output pixels per PDF point for a page drawn at `scale` screen points per
/// PDF point: device resolution, capped by the largest texture the GPU takes
/// and by `MAX_PAGE_PIXELS`.
///
/// The scale is rounded up to a quarter power of two first (`quantize_scale`),
/// so small changes of window size or zoom keep the page image they have --
/// and find the same one in the page cache -- at the cost of up to 19% more
/// pixels.
pub fn render_scale(size: Vec2, scale: f32, ppp: f32, max_side: f32) -> f32 {
    let by_side = max_side / size.x.max(size.y).max(1.0);
    let by_pixels = (MAX_PAGE_PIXELS / (size.x * size.y).max(1.0)).sqrt();
    quantize_scale(scale * ppp).min(by_side).min(by_pixels)
}

/// `scale` rounded up to the nearest quarter power of two.
pub fn quantize_scale(scale: f32) -> f32 {
    if scale <= 0.0 {
        return scale;
    }
    // The small nudge keeps an exact step, such as 1 or 2, where it is.
    2f32.powf((scale.log2() * 4.0 - 1e-3).ceil() / 4.0)
}

/// Whether a page still needs rendering at `scale`: it has no image, only a
/// part-drawn one, or one at another scale -- and pdfium hasn't failed on it.
fn needs_render(doc: &Doc, page: usize, scale: f32) -> bool {
    !doc.failed.contains(&page) && doc.textures.get(&page).is_none_or(|t| !t.complete || (t.scale - scale).abs() > 1e-3)
}

/// The pages to load once the view is drawn, nearest first: up to `count - 1`
/// the way the view is heading, then back the other way. Near either end of
/// the document there is less ahead, so more comes from behind.
fn pages_ahead(first: usize, last: usize, n: usize, down: bool, count: usize) -> Vec<usize> {
    let after: Vec<usize> = (last + 1..n).take(count).collect();
    let before: Vec<usize> = (0..first).rev().take(count).collect();
    let (forward, backward) = if down { (after, before) } else { (before, after) };
    let leading = forward.len().min(count.saturating_sub(1).max(1));
    let mut pages: Vec<usize> = forward[..leading].to_vec();
    pages.extend(backward.iter().take(count.saturating_sub(pages.len())));
    let room = count.saturating_sub(pages.len());
    pages.extend(forward[leading..].iter().take(room));
    pages
}

/// Redraw the part of the page under the highlight with the page texture
/// tinted by the highlight colour. Tinting multiplies, so the white page turns
/// the highlight colour while black glyphs stay black -- see-through, like a
/// real highlighter, with no blend modes needed. Where sharp squares of the
/// zoomed-in view (`detail`) cover the page, they are tinted too, on top.
fn paint_highlight(
    painter: &egui::Painter,
    texture: Option<TextureId>,
    detail: &[(TextureId, Rect)],
    page: Rect,
    r: Rect,
    color: Color32,
) {
    match texture {
        Some(id) => painter.image(id, r, uv_within(page, r), color),
        None => painter.rect_filled(r, CornerRadius::same(1), color.gamma_multiply(0.45)),
    };
    for &(id, area) in detail {
        let part = r.intersect(area);
        if part.is_positive() {
            painter.image(id, part, uv_within(area, part), color);
        }
    }
}

/// Where `inner` falls within `outer`, as texture coordinates.
fn uv_within(outer: Rect, inner: Rect) -> Rect {
    Rect::from_min_max(
        pos2((inner.min.x - outer.min.x) / outer.width(), (inner.min.y - outer.min.y) / outer.height()),
        pos2((inner.max.x - outer.min.x) / outer.width(), (inner.max.y - outer.min.y) / outer.height()),
    )
}

/// The selected character range on each page a drag covers, first page first.
fn drag_segments(text: &HashMap<usize, Vec<TextChar>>, drag: &Drag) -> Vec<(usize, Range<usize>)> {
    match *drag {
        Drag::Text { anchor, focus } => {
            let (a, b) = if anchor <= focus { (anchor, focus) } else { (focus, anchor) };
            (a.0..=b.0)
                .filter_map(|page| {
                    let chars = text.get(&page)?;
                    let start = if page == a.0 { a.1 } else { 0 };
                    let end = if page == b.0 { b.1 } else { chars.len() };
                    (start < end).then_some((page, start..end))
                })
                .collect()
        }
        Drag::Box { page, start, end } => match text.get(&page) {
            Some(chars) => selection::in_box(chars, &box_between(start, end)).into_iter().map(|range| (page, range)).collect(),
            None => Vec::new(),
        },
    }
}

/// The box with these two corners, in PDF user space.
fn box_between(a: (f32, f32), b: (f32, f32)) -> PdfBox {
    PdfBox { left: a.0.min(b.0), bottom: a.1.min(b.1), right: a.0.max(b.0), top: a.1.max(b.1) }
}

/// Every button in the app. Painted by hand rather than with egui's `Button`
/// so hover and press states can have their own colours per tone. Respects a
/// disabled parent `Ui`.
fn paint_button(ui: &mut Ui, text: &str, font: FontId, tone: Tone, selected: bool, min_size: Vec2) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let padding = if matches!(tone, Tone::Ghost | Tone::Danger) { 10.0 } else { 14.0 };
    let size = vec2(galley.size().x + 2.0 * padding, 30.0).max(min_size);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let enabled = ui.is_enabled();
    let hovered = enabled && response.hovered();
    let pressed = enabled && response.is_pointer_button_down_on();
    let pick = |idle: Color32, hover: Color32, press: Color32| {
        if pressed {
            press
        } else if hovered {
            hover
        } else {
            idle
        }
    };
    let (mut fill, border, mut ink) = match tone {
        Tone::Primary => (pick(ACCENT, ACCENT_HOVER, ACCENT_PRESSED), Color32::TRANSPARENT, Color32::WHITE),
        Tone::Secondary if selected => (pick(ACCENT_SOFT, ACCENT_SOFT, ACCENT_SOFT_BORDER), ACCENT_SOFT_BORDER, ACCENT_TEXT),
        Tone::Secondary => (pick(SURFACE, HOVER_FILL, PRESSED_FILL), INPUT_BORDER, TEXT),
        Tone::Ghost => (pick(Color32::TRANSPARENT, HOVER_FILL, PRESSED_FILL), Color32::TRANSPARENT, MUTED),
        Tone::Danger => (pick(Color32::TRANSPARENT, DANGER_SOFT, DANGER_PRESSED), Color32::TRANSPARENT, DANGER),
    };
    if !enabled {
        if matches!(tone, Tone::Primary) {
            fill = fill.gamma_multiply(0.45);
        } else {
            ink = SUBTLE;
        }
    }

    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(8), fill, Stroke::new(1.0, border), StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, ink);
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

fn styled_button(ui: &mut Ui, text: &str, tone: Tone, selected: bool) -> egui::Response {
    paint_button(ui, text, FontId::proportional(13.5), tone, selected, Vec2::ZERO)
}

/// A square button holding one symbol, in monospace (see `search_box`).
fn icon_button(ui: &mut Ui, glyph: &str) -> egui::Response {
    paint_button(ui, glyph, FontId::monospace(15.0), Tone::Secondary, false, vec2(30.0, 30.0))
}

/// The label on an oversized page, in its top-right corner, which switches
/// between shrunk and actual size when clicked.
fn size_badge(ui: &Ui, page: Rect, index: usize, label: &str) -> egui::Response {
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::proportional(12.0), Color32::PLACEHOLDER);
    let size = galley.size() + vec2(20.0, 10.0);
    let rect = Rect::from_min_size(pos2(page.max.x - size.x - 10.0, page.min.y + 10.0), size);
    let response = ui.interact(rect, Id::new(("size-badge", index)), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() { TEXT } else { TEXT.gamma_multiply(0.82) };
        painter.rect_filled(rect, CornerRadius::same(12), fill);
        painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

/// A magnifying glass inside the left margin of the find box.
fn paint_magnifier(ui: &Ui, field: Rect) {
    let center = pos2(field.min.x + 14.0, field.center().y - 1.5);
    let stroke = Stroke::new(1.5, SUBTLE);
    ui.painter().circle_stroke(center, 4.5, stroke);
    ui.painter().line_segment([center + vec2(3.3, 3.3), center + vec2(6.5, 6.5)], stroke);
}

fn swatch(ui: &mut Ui, rgb: Rgb, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let center = rect.center();
    let painter = ui.painter();
    if selected {
        painter.circle_stroke(center, 12.5, Stroke::new(2.0, ACCENT));
    } else if response.hovered() {
        painter.circle_stroke(center, 12.5, Stroke::new(1.5, INPUT_BORDER));
    }
    painter.circle_filled(center, 9.5, to_color32(rgb));
    painter.circle_stroke(center, 9.5, Stroke::new(1.0, Color32::from_black_alpha(28)));
    if selected {
        let tick = Stroke::new(1.8, TEXT);
        painter.line_segment([center + vec2(-4.0, 0.0), center + vec2(-1.2, 3.0)], tick);
        painter.line_segment([center + vec2(-1.2, 3.0), center + vec2(4.2, -3.2)], tick);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

/// The text being highlighted or annotated, on a tinted card with a bar in the
/// highlight's colour down its left edge.
fn quote_card(ui: &mut Ui, text: &str, color: Color32) {
    let shown = if text.chars().count() > 180 {
        format!("{}…", text.chars().take(180).collect::<String>().trim_end())
    } else {
        text.to_owned()
    };
    let card = Frame::NONE
        .fill(QUOTE_BG)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin { left: 16, right: 10, top: 8, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(shown).size(13.0).color(QUOTE_TEXT));
        });
    let r = card.response.rect;
    let bar = Rect::from_min_max(pos2(r.min.x + 5.0, r.min.y + 7.0), pos2(r.min.x + 8.0, r.max.y - 7.0));
    ui.painter().rect_filled(bar, CornerRadius::same(2), color);
}

fn quote_block(ui: &mut Ui, text: &str, max_chars: usize) {
    let shown = if text.chars().count() > max_chars {
        format!("{}…", text.chars().take(max_chars).collect::<String>().trim_end())
    } else {
        text.to_owned()
    };
    let inner = Frame::NONE
        .inner_margin(Margin { left: 10, right: 0, top: 0, bottom: 0 })
        .show(ui, |ui| ui.label(RichText::new(shown).size(12.0).color(QUOTE_TEXT)));
    let r = inner.response.rect;
    ui.painter().vline(r.left() + 1.5, r.y_range(), Stroke::new(3.0, QUOTE_BORDER));
}

/// A keyboard shortcut drawn as a key.
fn keycap(ui: &mut Ui, text: &str) {
    Frame::NONE
        .fill(HOVER_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).color(QUOTE_TEXT));
        });
}

fn empty_note(ui: &mut Ui, text: &str) {
    Frame::NONE.inner_margin(Margin::symmetric(14, 16)).show(ui, |ui| {
        ui.label(RichText::new(text).size(13.0).color(MUTED));
    });
}

fn panel_heading(ui: &mut Ui, title: &str, detail: String) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(14.0).strong().color(TEXT));
        if !detail.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(detail).size(12.5).color(MUTED));
            });
        }
    });
}

/// One search result: the page, then up to two lines of the surrounding text
/// with the match picked out. Painted directly at a fixed height so the list
/// can skip rows that are scrolled out of view.
fn result_row(ui: &mut Ui, hit: &SearchHit, current: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), RESULT_ROW_HEIGHT), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();

    let fill = if current {
        NOTE_ACTIVE
    } else if response.hovered() {
        ROW_HOVER
    } else {
        SURFACE
    };
    painter.rect_filled(rect, CornerRadius::same(0), fill);
    if current {
        painter.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), CornerRadius::same(0), HIT_OUTLINE);
    }
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, ROW_RULE));
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let inner = rect.shrink2(vec2(14.0, 8.0));
    painter.text(inner.left_top(), Align2::LEFT_TOP, format!("Page {}", hit.page + 1), FontId::proportional(12.0), MUTED);

    let font = FontId::proportional(13.0);
    let mut job = LayoutJob::default();
    job.wrap.max_width = inner.width();
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    let around = TextFormat { font_id: font.clone(), color: QUOTE_TEXT, ..Default::default() };
    job.append(&hit.before, 0.0, around.clone());
    job.append(&hit.matched, 0.0, TextFormat { font_id: font, color: TEXT, background: HIT, ..Default::default() });
    job.append(&hit.after, 0.0, around);
    painter.galley(inner.left_top() + vec2(0.0, 18.0), painter.layout_job(job), QUOTE_TEXT);

    response
}

/* ------------------------------------------------------------------ *
 * The author name, remembered between runs
 * ------------------------------------------------------------------ */

fn author_file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("pdf-annotate").join("author.txt"))
}

fn load_author() -> String {
    author_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

fn save_author(name: &str) {
    let Some(path) = author_file() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, name.trim());
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
