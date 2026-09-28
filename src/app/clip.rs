//! The Clip tool: a box dragged over a page lifts out what is drawn there --
//! the page itself, and the markups, measurements and highlights over it,
//! all as vector drawing -- and puts it on the clipboard. Ctrl+V in any
//! Kinetic PDF window puts it down again as a markup of its own, which moves,
//! resizes, undoes and saves like any other.
//!
//! What's lifted is the shapes the GPU draws the page with (`gpu-lines`),
//! written back out as a small PDF of their own (`gpu_lines::clip_document`):
//! fonts, patterns and the file they came from are left behind, so a clip is
//! the same wherever it goes. Markups over the page are drawn from their
//! appearance streams into the same shapes, so a clip shows them without
//! carrying them: what it holds is a picture of them, not the markups.
//!
//! The work is done off the UI thread -- lifting on the thread that already
//! has the file parsed for the pages, reading a placed clip back on one of
//! its own -- and a clip on screen is drawn from shapes sent up once, so
//! moving or resizing it only changes the matrix it's drawn through.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;

use gpu_lines::lopdf::{self, dictionary};
use gpu_lines::{ClipOptions, ClipOverlay, Matrix};
use markup_model::markup::{ClipArt, Geometry, MarkupKind as MeasureKind};
use markup_model::Pt;

use super::copying::Copied;
use super::gpu::{ClipDrawings, ClipShown, Gpu};
use super::*;

/// The shortest side a clip can have, in points: a drag shorter than this
/// was a click.
const LEAST_SIDE: f32 = 2.0;

/// The tools that take an area of a page, dragged out as a box or clicked
/// round as a polygon: what they do with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum AreaTool {
    /// Copies it -- the page, and everything drawn over it -- to paste as a
    /// markup.
    Clip,
    /// Copies the page's own drawing there, then erases it: moving a piece of
    /// the drawing. Markups over it stay where they are.
    Cut,
    /// Erases the page's own drawing there. Markups over it stay.
    Erase,
}

impl AreaTool {
    pub(super) const ALL: [AreaTool; 3] = [AreaTool::Clip, AreaTool::Cut, AreaTool::Erase];

    pub(super) fn label(self) -> &'static str {
        match self {
            AreaTool::Clip => "Clip",
            AreaTool::Cut => "Cut",
            AreaTool::Erase => "Erase",
        }
    }

    pub(super) fn key(self) -> Key {
        match self {
            AreaTool::Clip => Key::C,
            AreaTool::Cut => Key::X,
            AreaTool::Erase => Key::D,
        }
    }

    pub(super) fn icon(self) -> Icon {
        match self {
            AreaTool::Clip => Icon::Clip,
            AreaTool::Cut => Icon::Cut,
            AreaTool::Erase => Icon::Erase,
        }
    }

    /// What its button says it does.
    pub(super) fn hint(self) -> &'static str {
        match self {
            AreaTool::Clip => "copy that part of the page, markups and all; Ctrl+V pastes it here or in another window",
            AreaTool::Cut => "take that part of the page's drawing out and copy it, to paste somewhere else with Ctrl+V; markups stay",
            AreaTool::Erase => "take that part of the page's drawing out; markups and measurements stay",
        }
    }
}

/// A piece of a page to lift out, and where to send it once it is.
pub(super) struct Capture {
    /// The page of the file.
    page: usize,
    /// From the page's user space to the clip's: points, the origin at its
    /// bottom left as it was seen on screen, y up.
    to_clip: [f32; 6],
    size: [f32; 2],
    /// The polygon to cut to, in the clip's space, if it isn't the box.
    outline: Option<Vec<[f32; 2]>>,
    /// Whether the annotations on the page come too: not for a cut, which
    /// leaves them where they are.
    annotations: bool,
    /// Parts of the page erased since the last save, in its user space.
    erased: Vec<Vec<[f32; 2]>>,
    /// What the app draws over the page, lifted with it in this order.
    overlays: Vec<ClipOverlay>,
    reply: Sender<Result<Lifted, String>>,
}

/// Where a clip was lifted from: the sheet's size, points across and down,
/// and the clip's corners on it, the drawing's bottom left first.
type Placed = ([f32; 2], [[f32; 2]; 4]);

/// A clip lifted: its drawing, and how many painting operators of the page
/// it kept and left out.
pub(super) struct Lifted {
    art: ClipArt,
    kept: usize,
    dropped: usize,
}

impl Capture {
    /// Lifts the clip out of `doc`, the file as parsed, and sends it back.
    pub(super) fn run(self, doc: Result<&lopdf::Document, String>) {
        let started = std::time::Instant::now();
        let lifted = doc.and_then(|doc| {
            let options = ClipOptions { outline: self.outline.as_deref(), annotations: self.annotations, erased: &self.erased, overlays: &self.overlays };
            let clipped = gpu_lines::clip_page(doc, self.page as u32 + 1, Matrix(self.to_clip), self.size, &options)?;
            Ok(Lifted { art: ClipArt::new(clipped.pdf, self.size.map(f64::from)), kept: clipped.kept, dropped: clipped.dropped })
        });
        crate::worker::trace(format_args!("clip: lifted in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0));
        let _ = self.reply.send(lifted);
    }
}

/// The affine map `f` is, found from where it takes three points: the one at
/// `(x0, y0)` and those `dx` along and `dy` up from it.
fn affine(f: impl Fn(f32, f32) -> [f32; 2], (x0, y0): (f32, f32), dx: f32, dy: f32) -> [f32; 6] {
    let [ox, oy] = f(x0, y0);
    let [px, py] = f(x0 + dx, y0);
    let [qx, qy] = f(x0, y0 + dy);
    let (a, b) = ((px - ox) / dx, (py - oy) / dx);
    let (c, d) = ((qx - ox) / dy, (qy - oy) / dy);
    [a, b, c, d, ox - a * x0 - c * y0, oy - b * x0 - d * y0]
}

/// How many points across and down a sheet with geometry `g` shows.
fn sheet_points(g: &PageGeometry) -> (f32, f32) {
    let (width, height) = (g.bounds.width(), g.bounds.height());
    if g.rotation % 2 == 1 { (height, width) } else { (width, height) }
}

/// The matrix taking a clip's drawing to screen points, placed on its page by
/// `placement` and the page drawn at `rect` with geometry `g`.
fn drawing_to_screen(placement: [f64; 6], rect: Rect, g: &PageGeometry) -> [f32; 6] {
    let [a, b, c, d, e, f] = placement;
    let on_screen = |x: f32, y: f32| {
        let (x, y) = (f64::from(x), f64::from(y));
        let (fx, fy) = g.to_view((a * x + c * y + e) as f32, (b * x + d * y + f) as f32);
        [rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height()]
    };
    affine(on_screen, (0.0, 0.0), 100.0, 100.0)
}

/// The clip being lifted, and the clip on hand to paste.
#[derive(Default)]
pub(super) struct Clipping {
    /// Which of the area tools is in hand, if any.
    pub(super) tool: Option<AreaTool>,
    /// A clip being lifted: which tool is lifting it, and where it was on the
    /// sheet it comes from, for pasting it back in place.
    lifting: Option<(AreaTool, Placed, Receiver<Result<Lifted, String>>)>,
    pub(super) drawings: ClipDrawings,
    /// A polygon being clicked round, on a sheet: its corners so far, in the
    /// user space of the page the sheet shows.
    pub(super) placing: Option<(usize, Vec<(f32, f32)>)>,
}

impl App {
    /// Takes up one of the area tools, putting down any other.
    pub(super) fn take_up_area_tool(&mut self, tool: AreaTool) {
        self.set_measure_tool(None);
        self.tool = None;
        self.highlighter = false;
        self.clipping.placing = None;
        self.clipping.tool = Some(tool);
    }

    /// Puts the area tool down, and any polygon half clicked round with it.
    pub(super) fn put_down_clip(&mut self) {
        self.clipping.tool = None;
        self.clipping.placing = None;
    }

    /// A click with the Clip tool: a corner of a polygon to lift what's
    /// inside. One back on the first corner, or on the last -- the second
    /// click of a double-click -- closes it.
    pub(super) fn clip_click(&mut self, sheet: usize, pos: Pos2) {
        let Some(point) = self.pdf_point(sheet, pos) else { return };
        let slack = measure::PICK_SLACK * self.points_per_screen(sheet);
        let near = |(x, y): (f32, f32)| (x - point.0).hypot(y - point.1) <= slack;
        match self.clipping.placing.as_mut() {
            Some((on, points)) if *on == sheet => {
                let closing = points.len() >= 3 && (near(points[0]) || points.last().is_some_and(|&last| near(last)));
                if closing {
                    self.finish_clip_outline();
                } else if !points.last().is_some_and(|&last| near(last)) {
                    points.push(point);
                }
            }
            _ => self.clipping.placing = Some((sheet, vec![point])),
        }
    }

    /// Lifts what's inside the polygon clicked round, if it has three
    /// corners or more.
    pub(super) fn finish_clip_outline(&mut self) {
        let Some((sheet, points)) = self.clipping.placing.take() else { return };
        if points.len() >= 3 {
            self.lift(sheet, &points, true);
        }
    }

    /// Takes back the polygon's last corner. Says whether there was one.
    pub(super) fn take_back_clip_corner(&mut self) -> bool {
        let Some((_, points)) = self.clipping.placing.as_mut() else { return false };
        points.pop();
        if points.is_empty() {
            self.clipping.placing = None;
        }
        true
    }

    /// Esc, Enter and Backspace while a polygon is being clicked round: Esc
    /// drops it, Enter closes it, Backspace takes back a corner. Taken
    /// before the tools' own keys, so Esc drops the polygon before the tool.
    pub(super) fn clip_outline_keys(&mut self, ctx: &egui::Context) {
        if self.clipping.placing.is_none() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let (escape, enter, back) = ctx.input_mut(|i| {
            (i.consume_key(Modifiers::NONE, Key::Escape), i.consume_key(Modifiers::NONE, Key::Enter), i.consume_key(Modifiers::NONE, Key::Backspace))
        });
        if escape {
            self.clipping.placing = None;
        }
        if enter {
            self.finish_clip_outline();
        }
        if back {
            self.take_back_clip_corner();
        }
    }

    /// Starts the Clip tool's box, unless a polygon is being clicked round.
    pub(super) fn start_clip(&mut self, sheet: usize, pos: Pos2) {
        if self.clipping.placing.is_some() {
            return;
        }
        if let Some(point) = self.pdf_point(sheet, pos) {
            self.drag = Some(Drag::Clip { sheet, start: point, end: point });
            self.popup = None;
        }
    }

    /// Letting go of the Clip tool's box: what it covers is lifted out, on
    /// the thread reading the pages, and put on the clipboard when it's done.
    pub(super) fn finish_clip(&mut self, sheet: usize, start: (f32, f32), end: (f32, f32)) {
        self.lift(sheet, &[start, end], false);
    }

    /// Lifts out what's within `points` -- in the user space of the page
    /// sheet `sheet` shows -- on the thread reading the pages, and puts it on
    /// the clipboard when it's done: the box round them, or with `outline`
    /// only what's inside the polygon they make, the rest of the box clear.
    fn lift(&mut self, sheet: usize, points: &[(f32, f32)], outline: bool) {
        let Some(doc) = self.doc.as_ref() else { return };
        let (Some(page), Some(g)) = (doc.sheet_page(sheet), doc.sheet_geometry(sheet)) else { return };
        // The box as it was seen: the sheet's own left, right, top and bottom,
        // however it and its page are turned.
        let (across, down) = sheet_points(&g);
        let seen: Vec<(f32, f32)> = points.iter().map(|&(x, y)| g.to_view(x, y)).collect();
        let (mut left, mut right, mut top, mut bottom) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(fx, fy) in &seen {
            (left, right, top, bottom) = (left.min(fx), right.max(fx), top.min(fy), bottom.max(fy));
        }
        let size = [(right - left) * across, (bottom - top) * down];
        if size[0] < LEAST_SIDE || size[1] < LEAST_SIDE {
            return;
        }
        let to_clip = affine(
            |x, y| {
                let (fx, fy) = g.to_view(x, y);
                [(fx - left) * across, (bottom - fy) * down]
            },
            (g.bounds.left, g.bounds.bottom),
            g.bounds.width().max(1.0),
            g.bounds.height().max(1.0),
        );
        let tool = self.clipping.tool.unwrap_or(AreaTool::Clip);
        // The area on the page, in its user space: the polygon as clicked, or
        // the box's four corners, which a quarter-turned sheet keeps square.
        let region: Vec<[f32; 2]> = match outline {
            true => points.iter().map(|&(x, y)| [x, y]).collect(),
            false => [(left, top), (right, top), (right, bottom), (left, bottom)].iter().map(|&(fx, fy)| g.from_view(fx, fy).into()).collect(),
        };
        let erased: Vec<Vec<[f32; 2]>> = doc.session.erasures().iter().filter(|e| e.page == page).map(|e| e.region.clone()).collect();
        // Erasing needs nothing lifted: it's shown at once, as paper, and
        // written into the page when it's saved.
        if tool != AreaTool::Clip {
            if let Some(doc) = self.doc.as_mut() {
                doc.session.apply(Command::Erase(crate::model::Erasure { page, region }));
            }
            if tool == AreaTool::Erase {
                return;
            }
        }
        let Some(doc) = self.doc.as_ref() else { return };
        let outline = outline.then(|| seen.iter().map(|&(fx, fy)| [(fx - left) * across, (bottom - fy) * down]).collect());
        let (reply, lifting) = mpsc::channel();
        // A clip is a picture of everything there; a cut is the drawing that
        // leaves the page, which markups and other programs' stamps don't.
        let clipping = tool == AreaTool::Clip;
        let overlays = if clipping { overlays(doc, page) } else { Vec::new() };
        let capture = Capture { page, to_clip, size, outline, annotations: clipping, erased, overlays, reply };
        // The thread reading the pages has the file parsed already; with no
        // GPU there's none, and a thread of its own reads the file.
        let capture = match doc.reader.as_ref() {
            Some(reader) => reader.capture(capture).err(),
            None => Some(capture),
        };
        if let Some(capture) = capture {
            let path = doc.path.clone();
            let ctx = self.ctx.clone();
            let run = move || {
                let parsed = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| lopdf::Document::load_mem(&bytes).map_err(|e| e.to_string()));
                capture.run(parsed.as_ref().map_err(Clone::clone));
                ctx.request_repaint();
            };
            if std::thread::Builder::new().name("clip".into()).spawn(run).is_err() {
                self.toast("Couldn't start clipping.".to_owned());
                return;
            }
        }
        let placed = ([across, down], [[left * across, bottom * down], [right * across, bottom * down], [right * across, top * down], [left * across, top * down]]);
        self.clipping.lifting = Some((tool, placed, lifting));
        self.toast(if clipping { "Clipping…" } else { "Cutting…" }.to_owned());
    }

    /// Once a frame: takes a clip lifted since the last, onto the clipboard,
    /// and sends clips' drawings up to the GPU.
    pub(super) fn clip_results(&mut self, ctx: &egui::Context) {
        if let Some((tool, placed, lifting)) = &self.clipping.lifting {
            let (tool, (sheet, corners)) = (*tool, *placed);
            let done = if tool == AreaTool::Cut { "Cut" } else { "Clipped" };
            match lifting.try_recv() {
                Ok(Ok(lifted)) => {
                    self.clipping.lifting = None;
                    crate::worker::trace(format_args!("clip: kept {} of {} painted, {} bytes", lifted.kept, lifted.kept + lifted.dropped, lifted.art.pdf.len()));
                    self.hold(Copied::clip(lifted.art, sheet, corners));
                    self.toast(done.to_owned());
                }
                Ok(Err(error)) => {
                    self.clipping.lifting = None;
                    self.toast(format!("Couldn't clip that: {error}"));
                }
                Err(TryRecvError::Disconnected) => {
                    self.clipping.lifting = None;
                    self.toast("Couldn't clip that.".to_owned());
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            }
        }
        if let Some(gpu) = &self.gpu {
            gpu.advance_clips(&mut self.clipping.drawings, Self::now(ctx), ctx);
        }
    }

    /// Resizes clip `id` by its corner `corner`, to the pointer at `pos`.
    pub(super) fn drag_clip_corner(&mut self, sheet: usize, id: MarkupId, corner: usize, pos: Pos2) {
        let Some(point) = self.pdf_point(sheet, pos) else { return };
        let least = f64::from(LEAST_SIDE * 4.0 * self.points_per_screen(sheet));
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        let Geometry::Polygon { pts, .. } = &markup.geometry else { return };
        let to = Pt::new(f64::from(point.0), f64::from(point.1));
        let Some(corners) = markup_model::clip_resized(pts, corner, to, least) else { return };
        let mut resized = markup.clone();
        resized.geometry = Geometry::Polygon { pts: corners, holes: Vec::new() };
        // One change while the drag lasts, so undo takes the whole resize back.
        doc.session.apply_merged(Command::ChangeMeasure(Box::new(resized)));
    }
}

/// What the app draws over page `page` rather than the file: its highlights,
/// the clips on it, markups not yet saved, and its measurements, in the
/// order the screen paints them. Each as a content stream in the page's user
/// space, as a file would hold it -- which is what saving would write.
fn overlays(doc: &Doc, page: usize) -> Vec<ClipOverlay> {
    let mut out = Vec::new();

    // Highlights darken what's under them, as a highlighter does.
    let mut ops = String::new();
    for e in doc.session.highlights().iter().filter(|e| e.hl.page == page) {
        let [r, g, b] = e.hl.color;
        ops += &format!("{r} {g} {b} rg\n");
        for q in &e.hl.quads {
            ops += &format!("{} {} {} {} re f\n", q.left, q.bottom, q.width(), q.height());
        }
    }
    if !ops.is_empty() {
        let resources = dictionary! { "ExtGState" => dictionary! { "M" => dictionary! { "Type" => "ExtGState", "BM" => "Multiply" } } };
        out.push(ClipOverlay::Content { content: format!("/M gs\n{ops}").into_bytes(), resources, patterns: Vec::new() });
    }

    let measures: Vec<_> = doc.session.measures().iter().filter(|(m, _)| m.page as usize == page).collect();
    for (m, _) in measures.iter().filter(|(m, _)| m.kind == MeasureKind::Clip) {
        let (Some(art), Geometry::Polygon { pts, .. }) = (&m.extras.clip, &m.geometry) else { continue };
        if let Some(placement) = art.placement(pts) {
            out.push(ClipOverlay::Pdf { pdf: Arc::clone(&art.pdf), placement: Matrix(placement.map(|v| v as f32)) });
        }
    }

    // The file draws saved markups itself; only new ones are the app's.
    for e in doc.session.markups().iter().filter(|e| e.markup.page == page && e.markup.key.is_none()) {
        let (content, resources, tile) = crate::markup::appearance(&e.markup);
        let patterns = tile.into_iter().map(|t| (t.name, t.dict, t.content)).collect();
        out.push(ClipOverlay::Content { content, resources, patterns });
    }

    let scale = scale::page_scale(doc, page);
    let units = scale.map_or(Default::default(), |s| s.display);
    let precision = scale.map_or(Default::default(), |s| s.precision);
    for (m, measured) in measures.iter().filter(|(m, _)| m.kind != MeasureKind::Clip) {
        let label = match &measured.result {
            Ok(q) => q.text(m.kind, &units, precision),
            Err(e) => Some(e.to_string()),
        };
        let look = pdf_io::appearance::appearance(m, label.as_deref());
        let patterns = look.patterns.iter().map(|t| (t.name.clone(), t.dict.clone(), t.content.clone())).collect();
        out.push(ClipOverlay::Content { content: look.content, resources: look.resources, patterns });
    }
    out
}

/// Draws the clips on page `page`, drawn at `rect` with geometry `g`, where
/// they're within `view`, with the one picked out outlined and its corners
/// ready to drag. Before the markups and measurements, which go over them.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_clips(
    painter: &egui::Painter,
    gpu: Option<&Gpu>,
    drawings: &mut ClipDrawings,
    doc: &Doc,
    page: usize,
    rect: Rect,
    g: &PageGeometry,
    view: Rect,
    active: Option<MarkupId>,
    picked: &[MarkupId],
    now: f64,
) {
    let ctx = painter.ctx().clone();
    let at = |p: Pt| {
        let (fx, fy) = g.to_view(p.x as f32, p.y as f32);
        pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height())
    };
    for (m, _) in doc.session.measures().iter().filter(|(m, _)| m.page as usize == page && m.kind == MeasureKind::Clip) {
        let Geometry::Polygon { pts, .. } = &m.geometry else { continue };
        let corners: Vec<Pos2> = pts.iter().map(|&p| at(p)).collect();
        let bounds = Rect::from_points(&corners);
        let shown = bounds.intersect(view).intersect(rect);
        if !shown.is_positive() {
            continue;
        }
        let drawn = match (gpu, &m.extras.clip) {
            (Some(gpu), Some(art)) => match gpu.clip_drawing(drawings, art, now, &ctx) {
                ClipShown::Ready(uploaded) => match art.placement(pts) {
                    Some(placement) => {
                        gpu.paint_clip(painter, uploaded, drawing_to_screen(placement, rect, g), shown);
                        true
                    }
                    None => false,
                },
                ClipShown::Coming => false,
                ClipShown::Failed => false,
            },
            _ => false,
        };
        // Its outline while its drawing is on its way, or where there's no GPU
        // to draw it.
        if !drawn {
            let ring: Vec<Pos2> = corners.iter().copied().chain(corners.first().copied()).collect();
            painter.extend(Shape::dashed_line(&ring, Stroke::new(1.0, SUBTLE), 4.0, 3.0));
        }
        if active == Some(m.id) || picked.contains(&m.id) {
            let ring: Vec<Pos2> = corners.iter().copied().chain(corners.first().copied()).collect();
            painter.add(Shape::line(ring, Stroke::new(1.5, ACCENT)));
            if active == Some(m.id) {
                for corner in &corners {
                    let handle = Rect::from_center_size(*corner, vec2(8.0, 8.0));
                    painter.rect_filled(handle, CornerRadius::same(1), Color32::WHITE);
                    painter.rect_stroke(handle, CornerRadius::same(1), Stroke::new(1.5, ACCENT), StrokeKind::Middle);
                }
            }
        }
    }
}

/// Paints what's been erased from page `page` since the last save, drawn at
/// `rect` with geometry `g`: paper, over the page's drawing and under what's
/// marked up on it, until the save writes the erasures into the page itself.
pub(super) fn paint_erasures(painter: &egui::Painter, doc: &Doc, page: usize, rect: Rect, g: &PageGeometry) {
    let at = |p: Pt| {
        let (fx, fy) = g.to_view(p.x as f32, p.y as f32);
        pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height())
    };
    // Those just saved, too, until the page is drawn again without them.
    let written = doc.erasures_written.iter().filter(|_| doc.redraw.contains(&page));
    for erasure in doc.session.erasures().iter().chain(written).filter(|e| e.page == page) {
        let ring: Vec<Pt> = erasure.region.iter().map(|&[x, y]| Pt::new(f64::from(x), f64::from(y))).collect();
        // Cut into triangles: a polygon clicked round needn't be convex.
        let mut mesh = egui::Mesh::default();
        for triangle in markup_model::geom::triangulate(&ring) {
            let first = mesh.vertices.len() as u32;
            for corner in triangle {
                mesh.colored_vertex(at(corner), Color32::WHITE);
            }
            mesh.add_triangle(first, first + 1, first + 2);
        }
        painter.add(Shape::mesh(mesh));
    }
}

/// Draws the polygon being clicked round on the sheet drawn at `rect` with
/// geometry `g`: its corners, and the pointer as the next, back round to the
/// first.
pub(super) fn paint_clip_outline(painter: &egui::Painter, rect: Rect, g: &PageGeometry, points: &[(f32, f32)], pointer: Option<Pos2>) {
    let at = |(x, y): (f32, f32)| {
        let (fx, fy) = g.to_view(x, y);
        pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height())
    };
    let mut ring: Vec<Pos2> = points.iter().map(|&p| at(p)).collect();
    ring.extend(pointer.filter(|p| rect.contains(*p)));
    // Outlined only: a polygon clicked round needn't be convex, and egui
    // fills only those.
    let mut closed = ring.clone();
    closed.extend(ring.first().copied());
    painter.extend(Shape::dashed_line(&closed, Stroke::new(1.5, ACCENT), 6.0, 4.0));
    for corner in points.iter().map(|&p| at(p)) {
        painter.circle_filled(corner, 3.0, ACCENT);
    }
}

/// Draws the Clip tool's box as it's dragged out on the sheet drawn at
/// `rect` with geometry `g`.
pub(super) fn paint_clip_box(painter: &egui::Painter, rect: Rect, g: &PageGeometry, start: (f32, f32), end: (f32, f32)) {
    let at = |(x, y): (f32, f32)| {
        let (fx, fy) = g.to_view(x, y);
        pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height())
    };
    let area = Rect::from_two_pos(at(start), at(end));
    painter.rect_filled(area, CornerRadius::ZERO, ACCENT.gamma_multiply(0.06));
    let ring = [area.left_top(), area.right_top(), area.right_bottom(), area.left_bottom(), area.left_top()];
    painter.extend(Shape::dashed_line(&ring, Stroke::new(1.5, ACCENT), 6.0, 4.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_affine_map_is_found_from_three_points() {
        let m = Matrix([0.0, -2.0, 2.0, 0.0, 10.0, 600.0]);
        let found = affine(|x, y| m.apply([x, y]), (100.0, 50.0), 500.0, 700.0);
        for (a, b) in found.iter().zip(m.0) {
            assert!((a - b).abs() < 1e-3, "{found:?}");
        }
    }

    #[test]
    fn a_box_on_a_turned_sheet_is_lifted_the_way_it_was_seen() {
        // A portrait page shown a quarter turn clockwise: the page's left
        // edge is along the top of the screen.
        let g = PageGeometry { rotation: 1, bounds: crate::model::PdfBox { left: 0.0, bottom: 0.0, right: 600.0, top: 800.0 } };
        let (across, down) = sheet_points(&g);
        assert_eq!((across, down), (800.0, 600.0));
        let to_clip = affine(
            |x, y| {
                let (fx, fy) = g.to_view(x, y);
                [fx * across, (1.0 - fy) * down]
            },
            (0.0, 0.0),
            600.0,
            800.0,
        );
        let m = Matrix(to_clip);
        // The page's bottom left is at the top left of the screen.
        let [x, y] = m.apply([0.0, 0.0]);
        assert!((x - 0.0).abs() < 1e-3 && (y - 600.0).abs() < 1e-3, "{x}, {y}");
        // Up the page is to the right on the screen.
        let [x, y] = m.apply([0.0, 100.0]);
        assert!((x - 100.0).abs() < 1e-3 && (y - 600.0).abs() < 1e-3, "{x}, {y}");
    }

}
