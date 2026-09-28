//! Kinetic Compare: the open drawing set and another laid side by side,
//! sheet by sheet, with the two laid over each other in a third column.
//!
//! Row n pairs the n-th sheet of each set. A revision adds sheets, drops
//! them and moves them about, so each side can be put in order to pair up:
//! a sheet dragged to where it belongs, a blank sheet put in opposite one
//! the other set hasn't got, or one left out. That's only how the two are
//! paired -- neither file is changed -- which is why whatever was being
//! done to the open one is saved first: it's compared as it's saved.
//!
//! Both files are read here, each on a thread of its own, straight into
//! shapes for the GPU (`gpu_lines`), so the open document, and the one
//! pdfium thread that draws it, are left alone meanwhile. A page is sent up
//! a piece a frame, and each cell drawn into an image of its own a few
//! milliseconds a frame (`Renderer::paint_some`), then only shown: a sheet
//! is a few hundred thousand shapes, and three columns of them drawn again
//! every frame would never keep up with a scroll. The overlay is the two
//! pages drawn into one image, each in a colour of its own and multiplied
//! (`paint_some_tinted`): what both draw comes out dark, what only the
//! original draws red, what only the other draws blue.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui_glow;
use gpu_lines::{lopdf, Canvas, Prepared, Progress, Tint, Upload, Uploaded};

use super::*;
use super::gpu::Gpu;
use crate::arrange::Arrangement;
use crate::worker::trace;

/// Screen points between columns, and down the left for the rows' numbers.
const GUTTER: f32 = 28.0;
const MARGIN: f32 = 40.0;
/// Screen points between one row and the next.
const ROW_GAP: f32 = 28.0;
/// Screen points the columns' names take over the rows.
const HEADINGS: f32 = 40.0;
/// The narrowest a column is shown.
const NARROWEST: f32 = 120.0;
/// Pixels a point a page's images are read at: sharp for a sheet across a
/// column, and a fraction of the memory of the viewer's.
const DENSITY: f32 = 1.0;
/// How near the curve the shapes are flattened, as the viewer reads them.
const TOLERANCE: f32 = 0.05;
/// Pages on the GPU, and images of cells, kept past what's shown.
const UPLOADS_MOST: usize = 1536 << 20;
const CANVASES_MOST: usize = 512 << 20;
/// A frame's time for sending pages up, and its work for drawing cells (in
/// the renderer's microseconds: `gpu_lines::cost`).
const UPLOAD_PER_FRAME: Duration = Duration::from_micros(3000);
const UPLOAD_PIECE: usize = 1024 * 1024;
const DRAW_PER_FRAME: f32 = 8000.0;
/// The largest a cell's image is drawn, along its longer side.
const LARGEST_CANVAS: f32 = 4096.0;

/// The original, then what it's compared with.
const ORIGINAL: usize = 0;
const COMPARED: usize = 1;
const NAMES: [&str; 3] = ["Original", "Compared", "Overlay preview"];

/// How starting a comparison is going, before there is one.
pub(super) enum Starting {
    /// Asking to save what's unsaved first.
    Asking,
    /// Saving, to carry on once it's saved.
    Saving,
}

/// The comparison open.
pub(super) struct Compare {
    sides: [Side; 2],
    /// Screen points a column is across, once zoomed; `None` to fit the
    /// window.
    width: Option<f32>,
    /// The column's width last frame, to zoom from.
    shown_width: f32,
    /// Where the columns' top left is, from the view's, once they've been
    /// moved or zoomed; `None` for across the middle, at the top.
    pan: Option<Vec2>,
    /// The view held with the middle button, moving with the pointer.
    panning: bool,
    /// A sheet being dragged: its side, and the gap it would go into.
    moving: Option<(usize, usize)>,
    /// The side last clicked, which the keys act on.
    focus: usize,
    /// Which side each change to the pairing was made on, to undo it there.
    undo: Vec<usize>,
    redo: Vec<usize>,
    drawings: Drawings,
    /// The overlay being written out, and where to.
    generating: Option<Receiver<Result<PathBuf, String>>>,
}

/// One of the two sets.
struct Side {
    name: String,
    path: PathBuf,
    reader: Reader,
    /// Each page of the file's size in points, as it's shown, once read.
    sizes: Option<Vec<[f32; 2]>>,
    failed: Option<String>,
    /// The sheets in the order they're paired, blanks and all.
    arrange: Option<Arrangement>,
}

impl Side {
    fn open(path: &Path, ctx: &egui::Context) -> Side {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        Side { name, path: path.to_path_buf(), reader: Reader::spawn(path.to_path_buf(), ctx.clone()), sizes: None, failed: None, arrange: None }
    }

    fn len(&self) -> usize {
        self.arrange.as_ref().map_or(0, Arrangement::len)
    }

    /// What's at row `row`: the page, its size, and whether it's the file's
    /// rather than a blank.
    fn sheet(&self, row: usize) -> Option<(usize, [f32; 2], bool)> {
        let (arrange, sizes) = (self.arrange.as_ref()?, self.sizes.as_ref()?);
        let page = arrange.page_of(row)?;
        match sizes.get(page) {
            Some(&size) => Some((page, size, true)),
            None => arrange.new_pages().get(page - sizes.len()).map(|&size| (page, size, false)),
        }
    }
}

/// A file's pages, read into shapes on a thread of its own: its size first,
/// then whichever pages are wanted most.
struct Reader {
    wants: Arc<Mutex<Wants>>,
    results: Receiver<Read>,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct Wants {
    /// Most wanted first.
    pages: Vec<usize>,
    /// Read, or being read: not to be read again unless let go of.
    taken: HashSet<usize>,
}

enum Read {
    Opened(Vec<[f32; 2]>),
    Failed(String),
    Page(usize, Result<Prepared, String>),
}

impl Reader {
    fn spawn(path: PathBuf, ctx: egui::Context) -> Reader {
        let wants = Arc::new(Mutex::new(Wants::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (found, results) = mpsc::channel();
        let (asked, stopping) = (Arc::clone(&wants), Arc::clone(&stop));
        let run = move || {
            let started = Instant::now();
            let parsed = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| lopdf::Document::load_mem(&bytes).map_err(|e| e.to_string()));
            let doc = match parsed {
                Ok(doc) => doc,
                Err(e) => {
                    let _ = found.send(Read::Failed(e));
                    ctx.request_repaint();
                    return;
                }
            };
            let count = doc.get_pages().len();
            let sizes = (1..=count as u32).map(|n| gpu_lines::page_size(&doc, n).unwrap_or([612.0, 792.0])).collect();
            trace(format_args!("compare: opened {} in {:.0} ms", path.display(), started.elapsed().as_secs_f64() * 1000.0));
            if found.send(Read::Opened(sizes)).is_err() {
                return;
            }
            ctx.request_repaint();
            while !stopping.load(Ordering::Relaxed) {
                let next = asked.lock().ok().and_then(|mut wants| {
                    let page = wants.pages.iter().copied().find(|p| *p < count && !wants.taken.contains(p))?;
                    wants.taken.insert(page);
                    Some(page)
                });
                let Some(page) = next else {
                    std::thread::sleep(Duration::from_millis(15));
                    continue;
                };
                let started = Instant::now();
                let read = gpu_lines::page_shapes_unless(&doc, page as u32 + 1, TOLERANCE, DENSITY, Some(&stopping)).map(Prepared::new);
                trace(format_args!("compare: read page {page} in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0));
                if found.send(Read::Page(page, read)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        };
        if let Err(e) = std::thread::Builder::new().name("compare".into()).spawn(run) {
            trace(format_args!("compare: could not start reading: {e}"));
        }
        Reader { wants, results, stop }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// What a cell shows: one set's sheet, or both laid over each other --
/// either page missing where that side has a blank or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Content {
    Sheet { side: usize, page: usize },
    Overlay { a: Option<usize>, b: Option<usize> },
}

/// A cell drawn, or being drawn, into an image of its own.
struct Cell {
    content: Content,
    size: [u32; 2],
    canvas: Canvas,
    /// Which of its pages is being drawn, and how far.
    stage: usize,
    progress: Progress,
    done: bool,
    used: f64,
}

/// What's on the GPU for the comparison.
#[derive(Default)]
struct Drawings {
    uploads: HashMap<(usize, usize), (Arc<Uploaded>, f64)>,
    waiting: Vec<((usize, usize), Prepared)>,
    uploading: Option<((usize, usize), Upload)>,
    failed: HashSet<(usize, usize)>,
    cells: Vec<Cell>,
}

/// Where a row is down the view, how big, and the scale its sheets are
/// drawn at: screen points to a page point, the widest of its two across a
/// column.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Row {
    top: f32,
    height: f32,
    scale: f32,
}

/// The rows, for sheets whose sizes are `a` and `b` on each side (`None`
/// where a side has none), `width` screen points across a column.
fn rows_for(a: &[Option<[f32; 2]>], b: &[Option<[f32; 2]>], width: f32) -> Vec<Row> {
    let count = a.len().max(b.len());
    let mut top = 0.0;
    (0..count)
        .map(|at| {
            let (x, y) = (a.get(at).copied().flatten(), b.get(at).copied().flatten());
            let widest = x.map_or(0.0, |s| s[0]).max(y.map_or(0.0, |s| s[0])).max(1.0);
            let scale = width / widest;
            let tallest = x.map_or(0.0, |s| s[1]).max(y.map_or(0.0, |s| s[1]));
            let height = if tallest > 0.0 { tallest * scale } else { width * 0.7 };
            let row = Row { top, height, scale };
            top += height + ROW_GAP;
            row
        })
        .collect()
}

/// The gap between rows nearest `y` down the view: 0 before the first row,
/// `rows.len()` after the last.
fn gap_at(rows: &[Row], y: f32) -> usize {
    rows.iter().position(|row| y < row.top + row.height / 2.0).unwrap_or(rows.len())
}

/// The rows laid out at one zoom: columns `width` across.
struct Spread<'a> {
    rows: &'a [Row],
    width: f32,
}

impl Spread<'_> {
    /// Which column and row a point is on -- from the top left of it all --
    /// and how far across and down it, as fractions of the sheet.
    fn place_of(&self, p: Vec2) -> (f32, f32, usize, f32) {
        let stride = self.width + GUTTER;
        let x = p.x - MARGIN;
        let column = (x / stride).floor().clamp(0.0, 2.0);
        let across = (x - column * stride) / self.width;
        let y = p.y - ROW_GAP;
        let row = self.rows.iter().rposition(|r| r.top <= y).unwrap_or(0);
        let down = self.rows.get(row).map_or(0.0, |r| (y - r.top) / r.height.max(1.0));
        (column, across, row, down)
    }

    /// The point `place_of` gave, here.
    fn point_at(&self, (column, across, row, down): (f32, f32, usize, f32)) -> Vec2 {
        let x = MARGIN + column * (self.width + GUTTER) + across * self.width;
        let y = ROW_GAP + self.rows.get(row).map_or(0.0, |r| r.top + down * r.height);
        vec2(x, y)
    }

    /// How big it all is.
    fn size(&self) -> Vec2 {
        vec2(MARGIN * 2.0 + self.width * 3.0 + GUTTER * 2.0, self.rows.last().map_or(0.0, |r| r.top + r.height) + ROW_GAP * 2.0)
    }
}

/// Where it all goes -- its top left, from the view's -- to keep the spot of
/// a sheet at `at` in the view under it as the zoom goes from `before` to
/// `after`, from `pan`: the same sheet, the same place on it, whichever
/// column it's in.
fn zoomed_pan(before: &Spread, after: &Spread, pan: Vec2, at: Vec2) -> Vec2 {
    at - after.point_at(before.place_of(at - pan))
}

/// `pan`, as far as keeps a little of what's `size` in a view `view` in
/// size: it can go almost anywhere, but not out of sight altogether.
fn kept_in_sight(pan: Vec2, size: Vec2, view: Vec2) -> Vec2 {
    let least = vec2(120.0, 120.0).min(size).min(view);
    vec2(pan.x.clamp(least.x - size.x, view.x - least.x), pan.y.clamp(least.y - size.y, view.y - least.y))
}

/// The pixel size to draw a cell `points` across at, `ppp` pixels a point:
/// in steps of about a fifth, so a zoom going on doesn't draw it afresh
/// every frame, and never past `LARGEST_CANVAS`.
fn canvas_size(points: Vec2, ppp: f32) -> [u32; 2] {
    let wide = (points.x * ppp).max(1.0);
    let stepped = 2f32.powf((wide.log2() * 4.0).ceil() / 4.0);
    let longest = stepped.max(stepped * points.y / points.x.max(1.0));
    let fit = (LARGEST_CANVAS / longest).min(1.0);
    [(stepped * fit).round().max(1.0) as u32, (stepped * points.y / points.x.max(1.0) * fit).round().max(1.0) as u32]
}

impl Compare {
    fn new(original: &Path, compared: &Path, ctx: &egui::Context) -> Compare {
        Compare {
            sides: [Side::open(original, ctx), Side::open(compared, ctx)],
            width: None,
            shown_width: 0.0,
            pan: None,
            panning: false,
            moving: None,
            focus: ORIGINAL,
            undo: Vec::new(),
            redo: Vec::new(),
            drawings: Drawings::default(),
            generating: None,
        }
    }

    fn rows(&self, width: f32) -> Vec<Row> {
        let sizes = |side: &Side| (0..side.len()).map(|row| side.sheet(row).map(|(_, size, _)| size)).collect::<Vec<_>>();
        rows_for(&sizes(&self.sides[ORIGINAL]), &sizes(&self.sides[COMPARED]), width)
    }

    /// Changes side `side`'s pairing by `change`, as a step to undo.
    fn arrange(&mut self, side: usize, change: impl FnOnce(&mut Arrangement) -> bool) {
        let Some(arrange) = self.sides[side].arrange.as_mut() else { return };
        if change(arrange) {
            self.undo.push(side);
            self.redo.clear();
        }
    }

    fn undo_step(&mut self, redo: bool) {
        let (from, to) = if redo { (&mut self.redo, &mut self.undo) } else { (&mut self.undo, &mut self.redo) };
        let Some(side) = from.pop() else { return };
        if let Some(arrange) = self.sides[side].arrange.as_mut() {
            if if redo { arrange.redo() } else { arrange.undo() } {
                to.push(side);
            }
        }
    }

    /// The size a blank put in at row `row` of side `side` takes: the sheet
    /// opposite's, or else its neighbour's.
    fn blank_size(&self, side: usize, row: usize) -> [f32; 2] {
        let other = &self.sides[1 - side];
        let own = &self.sides[side];
        other
            .sheet(row)
            .or_else(|| own.sheet(row))
            .or_else(|| row.checked_sub(1).and_then(|r| own.sheet(r)))
            .map_or([1190.55, 841.89], |(_, size, _)| size)
    }

    /// Takes in what the readers have read, and sends a frame's worth up.
    fn receive(&mut self, gpu: &Gpu, now: f64, ctx: &egui::Context) {
        for (at, side) in self.sides.iter_mut().enumerate() {
            for read in side.reader.results.try_iter() {
                match read {
                    Read::Opened(sizes) => {
                        side.arrange = Some(Arrangement::new(sizes.len()));
                        side.sizes = Some(sizes);
                    }
                    Read::Failed(e) => side.failed = Some(e),
                    Read::Page(page, Ok(prepared)) => self.drawings.waiting.push(((at, page), prepared)),
                    Read::Page(page, Err(e)) => {
                        trace(format_args!("compare: page {page} couldn't be read: {e}"));
                        self.drawings.failed.insert((at, page));
                    }
                }
            }
        }
        let d = &mut self.drawings;
        let started = Instant::now();
        while started.elapsed() < UPLOAD_PER_FRAME {
            let Some((key, mut upload)) = d.uploading.take().or_else(|| {
                let (key, prepared) = d.waiting.pop()?;
                match gpu.renderer.begin_upload(&gpu.gl, prepared) {
                    Ok(upload) => Some((key, upload)),
                    Err(e) => {
                        trace(format_args!("compare: a page couldn't be sent up: {e}"));
                        d.failed.insert(key);
                        None
                    }
                }
            }) else {
                break;
            };
            if upload.step(&gpu.gl, UPLOAD_PIECE) {
                d.uploads.insert(key, (Arc::new(upload.finish()), now));
            } else {
                d.uploading = Some((key, upload));
            }
        }
        if d.uploading.is_some() || !d.waiting.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Asks each side's reader for `wanted` pages, most wanted first, that
    /// aren't on the GPU or on their way.
    fn want(&mut self, wanted: &[(usize, usize)], now: f64) {
        let d = &mut self.drawings;
        for key in wanted {
            if let Some((_, used)) = d.uploads.get_mut(key) {
                *used = now;
            }
        }
        for (at, side) in self.sides.iter().enumerate() {
            let pages: Vec<usize> = wanted
                .iter()
                .filter(|(s, page)| *s == at && !d.uploads.contains_key(&(at, *page)) && !d.failed.contains(&(at, *page)))
                .map(|&(_, page)| page)
                .collect();
            if let Ok(mut wants) = side.reader.wants.lock() {
                wants.pages = pages;
            }
        }
    }

    /// Lets go of pages and images not shown lately past what's kept, and
    /// of images begun at a size no longer wanted. A page let go of can be
    /// read again.
    fn trim(&mut self, gpu: &Gpu, now: f64) {
        let d = &mut self.drawings;
        let (kept, dropped): (Vec<Cell>, Vec<Cell>) = std::mem::take(&mut d.cells).into_iter().partition(|c| c.done || c.used >= now);
        dropped.into_iter().for_each(|c| c.canvas.destroy(&gpu.gl));
        d.cells = kept;
        let mut total: usize = d.cells.iter().map(|c| c.canvas.bytes()).sum();
        if total > CANVASES_MOST {
            d.cells.sort_by(|a, b| b.used.total_cmp(&a.used));
            while total > CANVASES_MOST && d.cells.last().is_some_and(|c| c.used < now - 1.0) {
                if let Some(cell) = d.cells.pop() {
                    total -= cell.canvas.bytes();
                    cell.canvas.destroy(&gpu.gl);
                }
            }
        }
        let mut total: usize = d.uploads.values().map(|(u, _)| u.bytes()).sum();
        if total > UPLOADS_MOST {
            let mut oldest: Vec<((usize, usize), f64)> = d.uploads.iter().filter(|(_, (_, used))| *used < now - 1.0).map(|(k, (_, used))| (*k, *used)).collect();
            oldest.sort_by(|a, b| a.1.total_cmp(&b.1));
            for (key, _) in oldest {
                if total <= UPLOADS_MOST {
                    break;
                }
                if let Some((uploaded, _)) = d.uploads.remove(&key) {
                    total -= uploaded.bytes();
                    if let Ok(uploaded) = Arc::try_unwrap(uploaded) {
                        uploaded.destroy(&gpu.gl);
                    }
                    if let Ok(mut wants) = self.sides[key.0].reader.wants.lock() {
                        wants.taken.remove(&key.1);
                    }
                }
            }
        }
    }

    /// Each row's pages of the two sets, from 0: none where a side has a
    /// blank there, or nothing.
    fn pairs(&self) -> Vec<[Option<usize>; 2]> {
        let count = self.sides[ORIGINAL].len().max(self.sides[COMPARED].len());
        (0..count).map(|row| [ORIGINAL, COMPARED].map(|side| self.sides[side].sheet(row).filter(|(_, _, in_file)| *in_file).map(|(page, _, _)| page))).collect()
    }

    /// Writes the overlay, row by row as they're paired now, to `out`, on a
    /// thread of its own.
    fn generate(&mut self, out: PathBuf, ctx: &egui::Context) {
        let (original, compared, rows) = (self.sides[ORIGINAL].path.clone(), self.sides[COMPARED].path.clone(), self.pairs());
        let (done, result) = mpsc::channel();
        let ctx = ctx.clone();
        let run = move || {
            let started = Instant::now();
            let written = crate::overlay::write(&original, &compared, &rows, &out).map(|()| out);
            trace(format_args!("compare: wrote the overlay in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0));
            let _ = done.send(written);
            ctx.request_repaint();
        };
        match std::thread::Builder::new().name("overlay".into()).spawn(run) {
            Ok(_) => self.generating = Some(result),
            Err(e) => trace(format_args!("compare: could not start writing the overlay: {e}")),
        }
    }

    /// Frees everything it has on the GPU.
    fn release(self, gpu: &Gpu) {
        let d = self.drawings;
        for cell in d.cells {
            cell.canvas.destroy(&gpu.gl);
        }
        for (uploaded, _) in d.uploads.into_values() {
            if let Ok(uploaded) = Arc::try_unwrap(uploaded) {
                uploaded.destroy(&gpu.gl);
            }
        }
        if let Some((_, upload)) = d.uploading {
            upload.destroy(&gpu.gl);
        }
    }
}

impl App {
    /// Tools → Kinetic Compare: saving first if there's anything unsaved,
    /// then the file to compare with -- or, while comparing, back to the
    /// document.
    pub(super) fn kinetic_compare(&mut self) {
        if self.compare.is_some() {
            return self.leave_compare();
        }
        if self.gpu.is_none() {
            return self.toast("Kinetic Compare draws on the graphics card, and there isn't one to draw on here".to_owned());
        }
        if self.has_unsaved_work() {
            self.compare_starting = Some(Starting::Asking);
        } else {
            self.pick_compared();
        }
    }

    fn pick_compared(&mut self) {
        let Some(original) = self.doc.as_ref().map(|d| d.path.clone()) else { return };
        let Some(path) = rfd::FileDialog::new().set_title("Compare with…").add_filter("PDF", &["pdf"]).pick_file() else { return };
        self.drag = None;
        self.popup = None;
        self.compare = Some(Compare::new(&original, &path, &self.ctx));
    }

    /// Back to the document, letting go of the comparison.
    pub(super) fn leave_compare(&mut self) {
        // Let go of at the start of the next frame, not now: this one's
        // drawing may already have been set out from its images, which would
        // come out black.
        self.compare_leaving = self.compare.take();
    }

    /// Frees what the comparison just left had on the GPU. Before anything
    /// is drawn.
    pub(super) fn release_left_compare(&mut self) {
        if let (Some(compare), Some(gpu)) = (self.compare_leaving.take(), self.gpu.as_ref()) {
            compare.release(gpu);
        }
    }

    /// The question asked before comparing with unsaved changes, and then,
    /// once the save it starts is done -- and the file opened again, if its
    /// pages were moved -- the file to compare with.
    pub(super) fn compare_start_dialog(&mut self, ctx: &egui::Context) {
        if matches!(self.compare_starting, Some(Starting::Saving)) && matches!(self.status, Status::Idle) {
            self.compare_starting = None;
            if self.has_unsaved_work() {
                self.toast("Nothing to compare yet: the changes weren't saved".to_owned());
            } else {
                self.pick_compared();
            }
            return;
        }
        if !matches!(self.compare_starting, Some(Starting::Asking)) {
            return;
        }
        let frame = Frame::NONE.fill(SURFACE).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(20)).shadow(soft_shadow());
        let (mut save, mut cancel) = (false, false);
        let modal = egui::Modal::new(Id::new("compare-save")).frame(frame).backdrop_color(Color32::from_black_alpha(60)).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 5.0, DIRTY);
                ui.label(RichText::new("Save before comparing").size(16.0).strong().color(TEXT));
            });
            ui.label(RichText::new("Kinetic Compare compares this file as it's saved. Save your changes to carry on.").size(13.5).color(QUOTE_TEXT));
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                save = styled_button(ui, "Save and continue", Tone::Primary, false).clicked();
                cancel = styled_button(ui, "Cancel", Tone::Secondary, false).clicked();
            });
        });
        if cancel || modal.should_close() {
            self.compare_starting = None;
        } else if save {
            self.compare_starting = Some(Starting::Saving);
            self.save();
        }
    }

    /// The comparison, in place of the pages: a bar of its own along the
    /// top, the columns' names, and the rows of sheets.
    pub(super) fn compare_view(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        let (Some(compare), Some(gpu)) = (self.compare.as_mut(), self.gpu.as_ref()) else { return };
        compare.receive(gpu, now, &ctx);
        // The overlay written, opened in a window of its own.
        let mut told: Option<String> = None;
        if let Some(result) = compare.generating.as_ref().and_then(|r| r.try_recv().ok()) {
            compare.generating = None;
            told = Some(match result {
                Ok(path) => match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg(&path).spawn()) {
                    Ok(_) => format!("Overlay written to {} and opened in a new window", path.display()),
                    Err(e) => format!("Overlay written to {}, but it couldn't be opened: {e}", path.display()),
                },
                Err(e) => format!("The overlay couldn't be written: {e}"),
            });
        }
        compare.trim(gpu, now);

        let mut leave = false;
        let mut generate = false;
        // A zoom asked for by the bar's buttons, about the middle of the view.
        let mut stepped: Option<f32> = None;
        // The bar: what this is, the zoom, and the way out.
        egui::Panel::top("compare-bar").frame(Frame::NONE.fill(SURFACE).inner_margin(Margin::symmetric(12, 6))).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Kinetic Compare").size(14.0).strong().color(TEXT));
                ui.add_space(12.0);
                ui.label(RichText::new("Drag sheets to pair them up. Right-click a sheet to put a blank beside it or leave it out.").size(12.0).color(SUBTLE));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    leave = styled_button(ui, "Exit compare", Tone::Secondary, false).clicked();
                    ui.add_space(4.0);
                    let ready = compare.generating.is_none() && compare.sides.iter().all(|s| s.len() > 0);
                    let label = if compare.generating.is_some() { "Generating…" } else { "Generate" };
                    generate = ui.add_enabled_ui(ready, |ui| styled_button(ui, label, Tone::Primary, false)).inner.on_hover_text("Write the overlay out as a PDF, paired as it is here, and open it in a new window").clicked();
                    ui.add_space(8.0);
                    if styled_button(ui, "Fit", Tone::Ghost, compare.width.is_none()).clicked() {
                        compare.width = None;
                        compare.pan = None;
                    }
                    if styled_button(ui, "+", Tone::Ghost, false).on_hover_text("Zoom in (Ctrl+wheel)").clicked() {
                        stepped = Some(1.25);
                    }
                    if styled_button(ui, "−", Tone::Ghost, false).on_hover_text("Zoom out (Ctrl+wheel)").clicked() {
                        stepped = Some(1.0 / 1.25);
                    }
                });
            });
        });

        // The keys: undo and redo on the side last changed, Delete leaves
        // out what's picked, Esc lets go of it.
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing {
            let (undo, redo, delete, escape) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::COMMAND, Key::Z),
                    i.consume_key(Modifiers::COMMAND, Key::Y) | i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z),
                    i.consume_key(Modifiers::NONE, Key::Delete),
                    i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if undo || redo {
                compare.undo_step(redo);
            }
            if delete {
                let side = compare.focus;
                compare.arrange(side, Arrangement::delete);
            }
            if escape {
                compare.sides.iter_mut().filter_map(|s| s.arrange.as_mut()).for_each(Arrangement::clear_selection);
            }
        }

        let view = Rect::from_min_size(ui.cursor().min, ui.available_size());
        // The sheets move under the columns' names, freely: dragged with the
        // middle button, scrolled with the wheel, zoomed about the pointer.
        let rows_view = vec2(view.width(), (view.height() - HEADINGS).max(1.0));
        let rows_min = view.min + vec2(0.0, HEADINGS);
        let fit = ((rows_view.x - MARGIN * 2.0 - GUTTER * 2.0) / 3.0).max(NARROWEST);
        let mut width = compare.width.unwrap_or(fit).max(NARROWEST);
        let spread_size = |width: f32| Spread { rows: &compare.rows(width), width }.size();
        // Until it's moved: across the middle, at the top.
        let mut pan = compare.pan.unwrap_or_else(|| vec2(((rows_view.x - spread_size(width).x) / 2.0).max(0.0), 0.0));
        // Zoomed with Ctrl and the wheel about the pointer, or by the bar's
        // buttons about the middle: the spot of whichever sheet is under that
        // point stays under it.
        let wheel = ui.input(|i| i.zoom_delta());
        let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| view.contains(*p));
        let zoom = match (stepped, pointer) {
            (Some(step), _) => Some((step, view.center())),
            (None, Some(at)) if wheel != 1.0 => Some((wheel, at)),
            _ => None,
        };
        if let Some((factor, at)) = zoom {
            let before = width;
            width = (width * factor).clamp(NARROWEST, 8000.0);
            let (rows_before, rows_after) = (compare.rows(before), compare.rows(width));
            let (was, will) = (Spread { rows: &rows_before, width: before }, Spread { rows: &rows_after, width });
            pan = zoomed_pan(&was, &will, pan, at - rows_min);
            compare.width = Some(width);
            compare.pan = Some(pan);
        }
        compare.shown_width = width;
        // The middle button grabs the view and moves it with the pointer, as
        // it does the pages; the wheel scrolls it, Shift and the wheel across.
        let (pressed, held, moved, scrolled) = ui.input(|i| (i.pointer.button_pressed(egui::PointerButton::Middle), i.pointer.middle_down(), i.pointer.delta(), i.smooth_scroll_delta));
        if pressed && pointer.is_some() {
            compare.panning = true;
        }
        if !held {
            compare.panning = false;
        }
        let mut shift = Vec2::ZERO;
        if compare.panning {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            shift += moved;
        }
        if pointer.is_some() && zoom.is_none() {
            shift += scrolled * self.scroll_speed;
        }
        if shift != Vec2::ZERO {
            pan += shift;
            compare.pan = Some(pan);
        }
        let rows = compare.rows(width);
        pan = kept_in_sight(pan, Spread { rows: &rows, width }.size(), rows_view);
        if compare.pan.is_some() {
            compare.pan = Some(pan);
        }

        // The columns' names, over the sheets, moving across with them.
        let heading = |ui: &mut Ui, x: f32, text: &str, detail: &str, colour: Color32| {
            let at = pos2(ui.max_rect().min.x + pan.x + MARGIN + x, ui.max_rect().min.y + 10.0);
            ui.painter().text(at, Align2::LEFT_TOP, text, FontId::proportional(13.0), colour);
            ui.painter().text(at + vec2(0.0, 17.0), Align2::LEFT_TOP, detail, FontId::proportional(11.5), SUBTLE);
        };
        let (header, _) = ui.allocate_exact_size(vec2(view.width(), HEADINGS), Sense::hover());
        {
            let mut header_ui = ui.new_child(egui::UiBuilder::new().max_rect(header));
            header_ui.set_clip_rect(header);
            let [a, b] = &compare.sides;
            let describe = |side: &Side| match (&side.failed, side.len()) {
                (Some(e), _) => format!("{} — couldn't be read: {e}", side.name),
                (None, 0) => format!("{} — opening…", side.name),
                (None, n) => format!("{} — {n} sheets", side.name),
            };
            heading(&mut header_ui, 0.0, NAMES[0], &describe(a), Color32::from_rgb(0xc0, 0x39, 0x2b));
            heading(&mut header_ui, width + GUTTER, NAMES[1], &describe(b), Color32::from_rgb(0x1f, 0x5f, 0xbf));
            heading(&mut header_ui, (width + GUTTER) * 2.0, NAMES[2], "Red: only the original · Blue: only the compared · Dark: both", TEXT);
        }

        let ppp = ctx.pixels_per_point();
        let mut wanted: Vec<(usize, usize)> = Vec::new();
        let mut budget = DRAW_PER_FRAME;
        let mut drawing = false;
        let mut menu_choice: Option<(usize, usize, SheetChoice)> = None;
        let (sheets_rect, _) = ui.allocate_exact_size(rows_view, Sense::hover());
        {
            let mut canvas = ui.new_child(egui::UiBuilder::new().max_rect(sheets_rect));
            canvas.set_clip_rect(sheets_rect.intersect(ui.clip_rect()));
            let ui = &mut canvas;
            let origin = sheets_rect.min + pan + vec2(MARGIN, ROW_GAP);
            let painter = ui.painter().clone();
            let shown = |row: &Row| origin.y + row.top + row.height >= sheets_rect.min.y - 200.0 && origin.y + row.top <= sheets_rect.max.y + 200.0;
            let visible: Vec<usize> = (0..rows.len()).filter(|&r| shown(&rows[r])).collect();
            // The pages wanted: those shown, then a row either side.
            let near = visible.first().map_or(0, |f| f.saturating_sub(1))..visible.last().map_or(0, |l| (l + 2).min(rows.len()));
            for r in near.clone() {
                for side in [ORIGINAL, COMPARED] {
                    if let Some((page, _, true)) = compare.sides[side].sheet(r) {
                        wanted.push((side, page));
                    }
                }
            }
            let pointer = ui.input(|i| i.pointer.hover_pos());
            for &r in &visible {
                let row = rows[r];
                let top = origin.y + row.top;
                painter.text(pos2(origin.x - 12.0, top), Align2::RIGHT_TOP, (r + 1).to_string(), FontId::proportional(12.0), SUBTLE);
                let mut pages: [Option<(usize, [f32; 2])>; 2] = [None, None];
                for side in [ORIGINAL, COMPARED] {
                    let left = origin.x + side as f32 * (width + GUTTER);
                    let sheet = compare.sides[side].sheet(r);
                    let rect = match sheet {
                        Some((_, size, _)) => Rect::from_min_size(pos2(left, top), vec2(size[0], size[1]) * row.scale),
                        None => Rect::from_min_size(pos2(left, top), vec2(width, row.height)),
                    };
                    let selected = compare.sides[side].arrange.as_ref().is_some_and(|a| a.is_selected(r));
                    match sheet {
                        Some((page, size, true)) => {
                            pages[side] = Some((page, size));
                            drawing |= compare.drawings.cell(gpu, &painter, Content::Sheet { side, page }, rect, [size, [0.0; 2]], row.scale, ppp, now, &mut budget);
                        }
                        Some((_, _, false)) => paint_blank(&painter, rect, "Blank sheet"),
                        None => paint_missing(&painter, rect),
                    }
                    if selected {
                        painter.rect_stroke(rect.expand(3.0), CornerRadius::same(3), Stroke::new(2.0, ACCENT), StrokeKind::Outside);
                    }
                    // Taking hold of it: a click picks it, a drag moves it,
                    // a right-click offers the rest.
                    let response = ui.interact(rect, Id::new(("compare-sheet", side, r)), Sense::click_and_drag());
                    if sheet.is_some() && response.clicked_by(egui::PointerButton::Primary) {
                        let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
                        compare.focus = side;
                        if let Some(arrange) = compare.sides[side].arrange.as_mut() {
                            arrange.click(r, ctrl, shift);
                        }
                    }
                    // Only the left button takes a sheet: the middle one moves the view.
                    if sheet.is_some() && response.drag_started_by(egui::PointerButton::Primary) && !compare.panning {
                        compare.focus = side;
                        if let Some(arrange) = compare.sides[side].arrange.as_mut() {
                            if !arrange.is_selected(r) {
                                arrange.click(r, false, false);
                            }
                        }
                        compare.moving = Some((side, r));
                    }
                    if response.hovered() && sheet.is_some() && compare.moving.is_none() {
                        ctx.set_cursor_icon(CursorIcon::Grab);
                    }
                    response.context_menu(|ui| {
                        compare.focus = side;
                        if sheet.is_some() {
                            if ui.button("Put a blank sheet before").clicked() {
                                menu_choice = Some((side, r, SheetChoice::BlankBefore));
                                ui.close();
                            }
                            if ui.button("Put a blank sheet after").clicked() {
                                menu_choice = Some((side, r, SheetChoice::BlankAfter));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Leave out of the comparison").clicked() {
                                menu_choice = Some((side, r, SheetChoice::LeaveOut));
                                ui.close();
                            }
                        } else if ui.button("Put a blank sheet here").clicked() {
                            menu_choice = Some((side, r, SheetChoice::BlankAfter));
                            ui.close();
                        }
                    });
                }
                // The two laid over each other, top left to top left.
                let left = origin.x + 2.0 * (width + GUTTER);
                let widest = pages.iter().flatten().map(|(_, s)| s[0]).fold(0.0, f32::max);
                let rect = Rect::from_min_size(pos2(left, top), vec2(if widest > 0.0 { widest * row.scale } else { width }, row.height));
                let overlay = Content::Overlay { a: pages[ORIGINAL].map(|(p, _)| p), b: pages[COMPARED].map(|(p, _)| p) };
                if pages.iter().any(Option::is_some) {
                    let sizes = [pages[ORIGINAL].map_or([0.0; 2], |(_, s)| s), pages[COMPARED].map_or([0.0; 2], |(_, s)| s)];
                    drawing |= compare.drawings.cell(gpu, &painter, overlay, rect, sizes, row.scale, ppp, now, &mut budget);
                } else {
                    paint_blank(&painter, rect, "Nothing to lay over");
                }
            }
            // A sheet being dragged: where it would go, as a line across its
            // column between rows.
            if let Some((side, _)) = compare.moving {
                ctx.set_cursor_icon(CursorIcon::Grabbing);
                if let Some(p) = pointer {
                    let gap = gap_at(&rows, p.y - origin.y);
                    let y = match gap {
                        0 => origin.y - ROW_GAP / 2.0,
                        g => origin.y + rows[g - 1].top + rows[g - 1].height + ROW_GAP / 2.0,
                    };
                    let left = origin.x + side as f32 * (width + GUTTER);
                    painter.line_segment([pos2(left - 6.0, y), pos2(left + width + 6.0, y)], Stroke::new(3.0, ACCENT));
                    if ui.input(|i| i.pointer.any_released()) {
                        let to = gap.min(compare.sides[side].len());
                        compare.arrange(side, |a| a.move_selected(to));
                        compare.moving = None;
                    }
                }
            }
            if !ui.input(|i| i.pointer.any_down()) {
                compare.moving = None;
            }
        }
        compare.want(&wanted, now);
        if drawing {
            ctx.request_repaint();
        }
        if let Some((side, row, choice)) = menu_choice {
            let size = compare.blank_size(side, row);
            compare.arrange(side, |a| match choice {
                SheetChoice::BlankBefore => a.insert_blank(row, size),
                SheetChoice::BlankAfter => a.insert_blank(row + 1, size),
                SheetChoice::LeaveOut => {
                    if !a.is_selected(row) {
                        a.click(row, false, false);
                    }
                    a.delete()
                }
            });
        }
        if generate {
            let names = compare.sides.each_ref().map(|s| s.path.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned()));
            let mut dialog = rfd::FileDialog::new().set_title("Save the overlay as…").add_filter("PDF", &["pdf"]).set_file_name(format!("{} vs {}.pdf", names[0], names[1]));
            if let Some(folder) = compare.sides[ORIGINAL].path.parent() {
                dialog = dialog.set_directory(folder);
            }
            if let Some(out) = dialog.save_file() {
                compare.generate(out, &ctx);
            }
        }
        if let Some(message) = told {
            self.toast(message);
        }
        if leave {
            self.leave_compare();
        }
    }
}

#[derive(Clone, Copy)]
enum SheetChoice {
    BlankBefore,
    BlankAfter,
    LeaveOut,
}

/// A blank sheet: paper, its edge dashed, and what it is.
fn paint_blank(painter: &egui::Painter, rect: Rect, what: &str) {
    painter.rect_filled(rect, CornerRadius::ZERO, Color32::WHITE);
    let ring = vec![rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
    painter.extend(Shape::dashed_line(&ring, Stroke::new(1.0, SUBTLE), 6.0, 4.0));
    painter.text(rect.center(), Align2::CENTER_CENTER, what, FontId::proportional(12.0), SUBTLE);
}

/// Where a side has no sheet opposite the other's.
fn paint_missing(painter: &egui::Painter, rect: Rect) {
    let ring = vec![rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
    painter.extend(Shape::dashed_line(&ring, Stroke::new(1.0, BORDER), 6.0, 4.0));
    painter.text(rect.center(), Align2::CENTER_CENTER, "No sheet", FontId::proportional(12.0), SUBTLE);
}

impl Drawings {
    /// Shows `content` in `rect`, drawing its image a budget's worth further
    /// if it isn't done: pages `sizes` in points at `scale` screen points a
    /// point. What's shown meanwhile is the same content at another size,
    /// stretched, or paper. Says whether its image is still being drawn: a
    /// page still being read has nothing to draw yet, and its reader wakes
    /// the window when it comes.
    #[allow(clippy::too_many_arguments)]
    fn cell(&mut self, gpu: &Gpu, painter: &egui::Painter, content: Content, rect: Rect, sizes: [[f32; 2]; 2], scale: f32, ppp: f32, now: f64, budget: &mut f32) -> bool {
        let size = canvas_size(rect.size(), ppp);
        // Its pages, in the order they're drawn: (side, page), size, tint.
        let pages: Vec<((usize, usize), [f32; 2], Tint)> = match content {
            Content::Sheet { side, page } => vec![((side, page), sizes[0], Tint::nth(side))],
            Content::Overlay { a, b } => [(ORIGINAL, a, sizes[0]), (COMPARED, b, sizes[1])]
                .into_iter()
                .filter_map(|(side, page, size)| page.map(|page| ((side, page), size, Tint::nth(side))))
                .collect(),
        };
        let failed = pages.iter().any(|(key, _, _)| self.failed.contains(key));
        let at = self.cells.iter().position(|c| c.content == content && c.size == size);
        let at = match at {
            Some(at) => Some(at),
            None if !failed && pages.iter().all(|(key, _, _)| self.uploads.contains_key(key)) => {
                Canvas::new(&gpu.gl, size, gpu.samples).map(|canvas| {
                    self.cells.push(Cell { content, size, canvas, stage: 0, progress: Progress::START, done: false, used: now });
                    self.cells.len() - 1
                })
            }
            None => None,
        };
        // Drawn further, a budget's worth.
        if let Some(at) = at {
            let cell = &mut self.cells[at];
            cell.used = now;
            // Pixels a page point covers in the image.
            let k = size[0] as f32 / (rect.width() / scale).max(1.0);
            while !cell.done && *budget > 0.0 {
                let Some(&(key, page_size, tint)) = pages.get(cell.stage) else {
                    cell.done = true;
                    cell.canvas.keep_only_texture(&gpu.gl);
                    break;
                };
                let Some((uploaded, _)) = self.uploads.get(&key) else { break };
                let to_pixels = [k, 0.0, 0.0, -k, 0.0, page_size[1] * k];
                let tinted = matches!(content, Content::Overlay { .. }).then_some(tint);
                let (reached, spent) = gpu.renderer.paint_some_tinted(&gpu.gl, uploaded, &cell.canvas, to_pixels, k, cell.progress, *budget, tinted, cell.stage == 0);
                *budget -= spent.max(1.0);
                cell.progress = reached;
                if reached.is_done(uploaded) {
                    cell.stage += 1;
                    cell.progress = Progress::START;
                }
            }
        }
        // What to show: this one if it's done, else the same content done at
        // another size, stretched to fit meanwhile.
        let drawing = at.is_some_and(|at| self.cells.get(at).is_some_and(|c| !c.done));
        let shown = self.cells.iter_mut().filter(|c| c.content == content && c.done).min_by_key(|c| (c.size[0] as i64 - size[0] as i64).abs());
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::WHITE);
        match shown {
            Some(cell) => {
                cell.used = now;
                let (texture, renderer) = (cell.canvas.texture(), Arc::clone(&gpu.renderer));
                let visible = rect.intersect(painter.clip_rect());
                if visible.is_positive() {
                    let callback = egui_glow::CallbackFn::new(move |info, painter| {
                        let ppp = info.pixels_per_point;
                        let viewport = info.viewport_in_pixels();
                        let screen = [viewport.width_px as f32, viewport.height_px as f32];
                        let left = (rect.min.x * ppp - viewport.left_px as f32).round();
                        let top = (rect.min.y * ppp - viewport.top_px as f32).round();
                        renderer.blit(painter.gl(), texture, [left, top, rect.width() * ppp, rect.height() * ppp], screen, false);
                    });
                    painter.add(egui::PaintCallback { rect: visible, callback: Arc::new(callback) });
                }
            }
            None if failed => {
                painter.text(rect.center(), Align2::CENTER_CENTER, "Couldn't be read", FontId::proportional(12.0), SUBTLE);
            }
            None => {
                painter.text(rect.center(), Align2::CENTER_CENTER, "Reading…", FontId::proportional(12.0), SUBTLE);
            }
        }
        painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, BORDER), StrokeKind::Outside);
        drawing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_pair_sheets_at_the_scale_of_the_wider_and_leave_room_for_either() {
        let a = [Some([1190.0, 842.0]), Some([842.0, 595.0]), None];
        let b = [Some([1190.0, 842.0]), Some([1190.0, 842.0])];
        let rows = rows_for(&a, &b, 400.0);
        assert_eq!(rows.len(), 3, "as many rows as the longer side has");
        assert!((rows[1].scale - 400.0 / 1190.0).abs() < 1e-6, "the wider of the two fills the column");
        assert!((rows[1].height - 842.0 * 400.0 / 1190.0).abs() < 1e-3);
        assert!((rows[1].top - (rows[0].height + ROW_GAP)).abs() < 1e-3);
        assert_eq!(gap_at(&rows, -5.0), 0);
        assert_eq!(gap_at(&rows, rows[1].top + 1.0), 1, "the top half of a row is the gap before it");
        assert_eq!(gap_at(&rows, rows[1].top + rows[1].height - 1.0), 2);
        assert_eq!(gap_at(&rows, 1e6), 3);
    }

    #[test]
    fn zooming_keeps_the_spot_of_a_sheet_under_the_pointer_under_it_whichever_column() {
        let sheets = [Some([1190.0, 842.0]); 12];
        let (a, b) = (rows_for(&sheets, &sheets, 400.0), rows_for(&sheets, &sheets, 150.0));
        let (was, will) = (Spread { rows: &a, width: 400.0 }, Spread { rows: &b, width: 150.0 });
        // Zoomed out with the pointer over the overlay, well down the set.
        let (pan, at) = (vec2(-500.0, -2400.0), vec2(1100.0, 250.0));
        let spot = was.place_of(at - pan);
        assert_eq!(spot.0, 2.0, "the overlay's column");
        let after = zoomed_pan(&was, &will, pan, at);
        let (column, across, row, down) = will.place_of(at - after);
        assert_eq!((column, row), (spot.0, spot.2), "the same sheet under the pointer");
        assert!((across - spot.1).abs() < 1e-4 && (down - spot.3).abs() < 1e-4, "the same place on it");
        assert!(after.x > 0.0, "free to move right of where the columns would sit");
        // And back in, where it came from.
        assert!((zoomed_pan(&will, &was, after, at) - pan).length() < 1e-2);
        // Moved as far as it goes, a little of it is still in sight.
        let size = was.size();
        let kept = kept_in_sight(vec2(1e6, -1e6), size, vec2(1000.0, 800.0));
        assert_eq!(kept, vec2(1000.0 - 120.0, 120.0 - size.y));
    }

    #[test]
    fn the_slider_shows_the_original_at_the_left_the_compared_at_the_right_and_both_between() {
        assert_eq!(fades(0.0), [1.0, 0.0], "the original alone");
        assert_eq!(fades(1.0), [0.0, 1.0], "the compared set alone");
        assert_eq!(fades(0.5), [1.0, 1.0], "both whole");
        assert_eq!(fades(0.25), [1.0, 0.5], "the compared set coming in");
    }

    #[test]
    fn a_cell_is_drawn_at_stepped_sizes_never_past_the_largest() {
        let [w, h] = canvas_size(vec2(400.0, 283.0), 1.0);
        assert!(w >= 400 && w < 480 && (h as f32 - w as f32 * 283.0 / 400.0).abs() <= 1.0, "{w} x {h}");
        assert_eq!(canvas_size(vec2(401.0, 283.0) * 1.0, 1.0), canvas_size(vec2(401.0, 283.0) * 1.02, 1.0), "a little bigger is the same image");
        let [w, h] = canvas_size(vec2(9000.0, 6000.0), 2.0);
        assert!(w as f32 <= LARGEST_CANVAS && h as f32 <= LARGEST_CANVAS);
    }
}

/// Looks, on a thread of its own, at whether the file at `path` is an
/// overlay Kinetic Compare wrote, and which layers are its sets: the file's
/// bytes are searched for the marker first, so any other PDF isn't parsed
/// again for it.
pub(super) fn probe_overlay(path: PathBuf, ctx: egui::Context) -> Receiver<Option<[(u32, u16); 2]>> {
    let (found, result) = mpsc::channel();
    let run = move || {
        let layers = std::fs::read(&path).ok().filter(|bytes| bytes.windows(crate::overlay::MARKER.len()).any(|w| w == crate::overlay::MARKER)).and_then(|bytes| {
            let doc = lopdf::Document::load_mem(&bytes).ok()?;
            crate::overlay::compare_layers(&doc)
        });
        if found.send(layers).is_ok() && layers.is_some() {
            ctx.request_repaint();
        }
    };
    if let Err(e) = std::thread::Builder::new().name("overlay".into()).spawn(run) {
        trace(format_args!("compare: could not look for an overlay: {e}"));
    }
    result
}

/// How far each set of an overlay shows with its slider at `at`: at the left
/// the original alone, at the right the compared set alone, and in the middle
/// both whole, each fading out towards the other's end.
fn fades(at: f32) -> [f32; 2] {
    [((1.0 - at) * 2.0).clamp(0.0, 1.0), (at * 2.0).clamp(0.0, 1.0)]
}

impl App {
    /// Tells the renderer how far each of an overlay's layers shows, as its
    /// slider says -- none faded for any other file -- and lets go of what
    /// was drawn at other fades. Before the pages are drawn: squares let go
    /// of after a frame's drawing has been set out would still be drawn from,
    /// and come out black.
    pub(super) fn apply_overlay_fades(&mut self, ctx: &egui::Context) {
        if let Some((generation, probe)) = &self.overlay_probe {
            if let Ok(layers) = probe.try_recv() {
                let generation = *generation;
                self.overlay_probe = None;
                if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                    doc.overlay = layers;
                }
            }
        }
        let now = ctx.input(|i| i.time);
        let layers = self.doc.as_ref().and_then(|d| d.overlay).filter(|_| self.compare.is_none());
        let shown = layers.map(|_| fades(self.overlay_fade));
        if let Some(gpu) = &self.gpu {
            if shown != self.fades_given {
                let given: Vec<((u32, u16), f32)> = layers.zip(shown).map(|(l, f)| vec![(l[0], f[0]), (l[1], f[1])]).unwrap_or_default();
                gpu.renderer.set_layer_fades(&given);
                // What was drawn at the old fades is drawn again; while the
                // slider moves, pages are drawn straight rather than into
                // squares that would each be drawn again for every step.
                if let Some(doc) = self.doc.as_mut() {
                    gpu.forget_tiles(&mut doc.gpu_tiles);
                    doc.fading_until = now + 0.35;
                }
                self.fades_given = shown;
                ctx.request_repaint();
            }
            // Erased from one layer alone and not saved yet: that layer is
            // masked out there, on each page drawn, the other showing through.
            let masks = self.doc.as_ref().map(layer_masks).unwrap_or_default();
            if masks != self.masks_given {
                gpu.renderer.set_layer_masks(masks.clone());
                if let Some(doc) = self.doc.as_mut() {
                    gpu.forget_tiles(&mut doc.gpu_tiles);
                }
                self.masks_given = masks;
                ctx.request_repaint();
            }
        }
    }

    /// The one layer of an overlay that Clip, Cut and Erase work on: the
    /// original's with its slider all the way left, the compared set's all
    /// the way right, and `None` -- both -- anywhere between, or for any
    /// other file.
    pub(super) fn working_layer(&self) -> Option<(u32, u16)> {
        let [original, compared] = self.doc.as_ref()?.overlay?;
        match self.overlay_fade {
            at if at <= 0.02 => Some(original),
            at if at >= 0.98 => Some(compared),
            _ => None,
        }
    }

    /// An overlay's slider, over the foot of its pages: from the original to
    /// the compared set. Where it's moved to is shown from the next frame
    /// (`apply_overlay_fades`).
    pub(super) fn overlay_slider(&mut self, ctx: &egui::Context) {
        if self.doc.as_ref().and_then(|d| d.overlay).filter(|_| self.compare.is_none()).is_none() {
            return;
        }
        let was = self.overlay_fade;
        let view = self.viewer_rect;
        let frame = Frame::NONE.fill(SURFACE).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(14, 8)).shadow(soft_shadow());
        egui::Area::new(Id::new("overlay-slider"))
            .order(Order::Foreground)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(view.center().x, view.max.y - 18.0))
            .show(ctx, |ui| {
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let [a, b] = fades(self.overlay_fade);
                        ui.label(RichText::new("Original").size(12.5).color(Color32::from_rgb(0xc0, 0x39, 0x2b).gamma_multiply(0.4 + 0.6 * a)));
                        ui.spacing_mut().slider_width = 260.0;
                        // A rail that reads on the panel, as the details panel's do.
                        ui.spacing_mut().slider_rail_height = 6.0;
                        ui.visuals_mut().widgets.inactive.bg_fill = INPUT_BORDER;
                        ui.visuals_mut().selection.bg_fill = ACCENT;
                        let slider = ui.add(egui::Slider::new(&mut self.overlay_fade, 0.0..=1.0).show_value(false));
                        slider.on_hover_text("Slide to the left for the original alone, to the right for the compared set alone; in the middle, both");
                        ui.label(RichText::new("Compared").size(12.5).color(Color32::from_rgb(0x1f, 0x5f, 0xbf).gamma_multiply(0.4 + 0.6 * b)));
                        if styled_button(ui, "Both", Tone::Ghost, (self.overlay_fade - 0.5).abs() < 0.01).on_hover_text("Both sets whole").clicked() {
                            self.overlay_fade = 0.5;
                        }
                    });
                });
            });
        if self.overlay_fade != was {
            ctx.request_repaint();
        }
    }
}

/// The layers masked out on each page of `doc` drawn on the GPU, by its
/// upload: each erased from one layer alone, and not saved yet -- or saved
/// just now, until the page is drawn again from the file -- cut into
/// triangles in the page's points, as its shapes are laid out.
fn layer_masks(doc: &Doc) -> HashMap<u64, Vec<((u32, u16), Vec<[f32; 2]>)>> {
    let written = doc.erasures_written.iter().filter(|e| doc.redraw.contains(&e.page));
    let mut masks: HashMap<u64, Vec<((u32, u16), Vec<[f32; 2]>)>> = HashMap::new();
    for erasure in doc.session.erasures().iter().chain(written) {
        let Some(layer) = erasure.layer else { continue };
        let Some(super::gpu::PageDrawing::Gpu { uploaded: Some(uploaded), .. }) = doc.drawing.get(&erasure.page) else { continue };
        let (Some(Some(g)), Some(size)) = (doc.geometry.get(erasure.page), doc.sizes.get(erasure.page)) else { continue };
        let ring: Vec<markup_model::Pt> = erasure.region.iter().map(|&[x, y]| markup_model::Pt::new(f64::from(x), f64::from(y))).collect();
        let to_page = |p: markup_model::Pt| {
            let (fx, fy) = g.to_view(p.x as f32, p.y as f32);
            [fx * size.x, (1.0 - fy) * size.y]
        };
        let triangles: Vec<[f32; 2]> = markup_model::geom::triangulate(&ring).into_iter().flatten().map(to_page).collect();
        let layers = masks.entry(uploaded.id()).or_default();
        match layers.iter_mut().find(|(l, _)| *l == layer) {
            Some((_, all)) => all.extend(triangles),
            None => layers.push((layer, triangles)),
        }
    }
    masks
}
