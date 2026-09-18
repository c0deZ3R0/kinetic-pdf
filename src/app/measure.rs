//! The measurement tools: length, polylength and area.
//!
//! Each is placed by clicking, snapping to the drawing as it goes (see
//! `scale.rs` and the model's `snap`), and shows what it measures while it's
//! being drawn, so the number is there before the shape is finished. A
//! measurement is a markup in the session like any other change: it undoes,
//! counts as unsaved work, and is written as a standard measurement
//! annotation.

use markup_model::markup::{Geometry, MarkupKind};
use markup_model::{quantities, Hit, MarkupId, Pt, Quantities, Scale};

use super::*;
use crate::model::MeasureMarkup;

/// Screen points from a measurement that still pick it.
pub(super) const PICK_SLACK: f32 = 4.0;

/// The page a measurement is being placed on, its kind, and the points
/// placed so far with the pointer's as the last.
pub(super) type Preview = (usize, MarkupKind, Vec<(f32, f32)>);

/// A measurement being placed, click by click.
pub(super) struct Placing {
    pub(super) kind: MarkupKind,
    pub(super) page: usize,
    /// The points placed so far, in PDF user space.
    pub(super) points: Vec<(f32, f32)>,
    /// Points taken back, newest last, to put down again.
    pub(super) undone: Vec<(f32, f32)>,
}

impl Placing {
    fn new(kind: MarkupKind, page: usize, at: (f32, f32)) -> Placing {
        Placing { kind, page, points: vec![at], undone: Vec::new() }
    }

    fn place(&mut self, at: (f32, f32)) {
        self.points.push(at);
        // A new point is a new course; what was taken back can't come back.
        self.undone.clear();
    }

    /// Takes the last point back. `false` if there was none.
    fn take_back(&mut self) -> bool {
        match self.points.pop() {
            Some(point) => {
                self.undone.push(point);
                true
            }
            None => false,
        }
    }

    /// Puts back the last point taken back. `false` if there was none.
    fn put_back(&mut self) -> bool {
        match self.undone.pop() {
            Some(point) => {
                self.points.push(point);
                true
            }
            None => false,
        }
    }
}

impl MeasureTool {
    /// The kind of measurement a tool makes, if it makes one.
    pub(super) fn kind(self) -> Option<MarkupKind> {
        match self {
            MeasureTool::Length => Some(MarkupKind::Length),
            MeasureTool::Polylength => Some(MarkupKind::Polylength),
            MeasureTool::Area | MeasureTool::Cutout => Some(MarkupKind::Area),
            MeasureTool::Calibrate | MeasureTool::Verify => None,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            MeasureTool::Length => "Length",
            MeasureTool::Polylength => "Polylength",
            MeasureTool::Area => "Area",
            MeasureTool::Cutout => "Cutout",
            MeasureTool::Calibrate => "Calibrate",
            MeasureTool::Verify => "Check",
        }
    }

    /// What to tell the user while it's in use.
    pub(super) fn hint(self) -> &'static str {
        match self {
            MeasureTool::Length => "Click each end of what you're measuring. Hold Ctrl to place a point exactly where the pointer is.",
            MeasureTool::Polylength => "Click along the run. Double-click or press Enter to finish it; Ctrl+Z or Backspace takes back a point.",
            MeasureTool::Area => "Click around the area. Double-click, press Enter, or click the first point again to close it; Ctrl+Z takes back a point.",
            MeasureTool::Cutout => "Click around a hole inside an area already measured: its size comes off that area.",
            MeasureTool::Calibrate | MeasureTool::Verify => "Drag along a known dimension.",
        }
    }

}

/// Points a measurement of this kind needs before it measures anything.
fn least_points(kind: MarkupKind) -> usize {
    match kind {
        MarkupKind::Area => 3,
        _ => 2,
    }
}

/// The geometry a kind takes from points placed in user space.
fn geometry_of(kind: MarkupKind, points: &[(f32, f32)]) -> Geometry {
    let pts: Vec<Pt> = points.iter().map(|&(x, y)| Pt::new(f64::from(x), f64::from(y))).collect();
    match kind {
        MarkupKind::Length => Geometry::Line { a: pts.first().copied().unwrap_or_default(), b: pts.last().copied().unwrap_or_default() },
        MarkupKind::Area => Geometry::Polygon { pts, holes: Vec::new() },
        _ => Geometry::Polyline { pts },
    }
}

/// What a measurement of `kind` through `points` would measure at `scale`.
fn measured(kind: MarkupKind, points: &[(f32, f32)], page: usize, scale: Option<&Scale>) -> Option<Quantities> {
    let markup = MeasureMarkup::new(page as u32, kind, geometry_of(kind, points));
    quantities(&markup, scale).ok()
}

impl App {
    /// The measurement tools in the tool row.
    pub(super) fn measure_buttons(&mut self, ui: &mut Ui) {
        for tool in [MeasureTool::Length, MeasureTool::Polylength, MeasureTool::Area, MeasureTool::Cutout] {
            let on = self.measure_tool == Some(tool);
            if styled_button(ui, tool.label(), Tone::Secondary, on).on_hover_text(tool.hint()).clicked() {
                self.set_measure_tool(if on { None } else { Some(tool) });
            }
        }
    }

    /// Takes up a measurement tool, or puts one down, clearing what was being
    /// placed and the drawing tools.
    pub(super) fn set_measure_tool(&mut self, tool: Option<MeasureTool>) {
        self.placing = None;
        self.measure_tool = tool;
        if tool.is_some() {
            self.tool = None;
            self.want_measurements();
        }
    }

    /// A click on `page` while a measurement tool is in use.
    pub(super) fn measure_click(&mut self, page: usize, pos: Pos2, double: bool) {
        let Some(tool) = self.measure_tool else { return };
        let Some(kind) = tool.kind() else { return };
        let Some(point) = self.pdf_point(page, pos) else { return };
        let from = self.placing.as_ref().and_then(|p| p.points.last().copied());
        let (_, at) = self.snapped(page, point, from);

        // A click back on the first point closes an area.
        let closing = double
            || self.placing.as_ref().is_some_and(|p| {
                let slack = PICK_SLACK * self.points_per_screen(page);
                kind == MarkupKind::Area
                    && p.points.len() >= 3
                    && p.points.first().is_some_and(|&(x, y)| (x - at.0).hypot(y - at.1) <= slack)
            });
        let slack = PICK_SLACK * self.points_per_screen(page);
        // Pressing again where the last point went finishes the shape: that is
        // what a double click is, and the second press mustn't leave a point on
        // top of a point.
        let repeat = self.placing.as_ref().is_some_and(|p| {
            p.page == page && p.points.last().is_some_and(|&(x, y)| (x - at.0).hypot(y - at.1) <= slack)
        });
        let closing = closing || (repeat && kind != MarkupKind::Length);
        match self.placing.as_mut() {
            Some(placing) if placing.page == page => {
                if !closing && !repeat {
                    placing.place(at);
                }
            }
            _ => self.placing = Some(Placing::new(kind, page, at)),
        }
        let done = match (kind, closing) {
            (MarkupKind::Length, _) => self.placing.as_ref().is_some_and(|p| p.points.len() >= 2),
            (_, closing) => closing,
        };
        if done {
            self.finish_measurement();
        }
    }

    /// Keeps what's been placed, if it's enough to measure. A cutout goes
    /// into the area it was drawn in rather than becoming one of its own.
    pub(super) fn finish_measurement(&mut self) {
        let Some(placing) = self.placing.take() else { return };
        if placing.points.len() < least_points(placing.kind) {
            return;
        }
        if self.measure_tool == Some(MeasureTool::Cutout) {
            self.cut_out(&placing);
            return;
        }
        let author = self.author_name();
        let (color, width) = (self.markup_color, f64::from(self.markup_width));
        let now = chrono::Utc::now().timestamp_millis();
        let Some(doc) = self.doc.as_mut() else { return };
        let mut markup = MeasureMarkup::new(placing.page as u32, placing.kind, geometry_of(placing.kind, &placing.points));
        markup.style.stroke = color;
        markup.style.width = width;
        if placing.kind == MarkupKind::Area {
            markup.style.fill = Some(color);
            markup.style.opacity = 0.18;
        }
        markup.meta.author = author;
        markup.meta.created_ms = Some(now);
        markup.meta.modified_ms = Some(now);
        let id = markup.id;
        doc.session.apply(crate::session::Command::AddMeasure(Box::new(markup)));
        self.active_measure = Some(id);
    }

    /// Takes the ring just drawn out of the area it sits in. The area it
    /// belongs to is the smallest one holding its first point, so a cutout
    /// inside a cutout's area goes to the right one.
    fn cut_out(&mut self, placing: &Placing) {
        let ring: Vec<Pt> = placing.points.iter().map(|&(x, y)| Pt::new(f64::from(x), f64::from(y))).collect();
        let Some(first) = ring.first().copied() else { return };
        let page = placing.page as u32;
        let Some(doc) = self.doc.as_mut() else { return };
        let smallest = doc
            .session
            .measures()
            .iter()
            .filter(|(m, _)| m.page == page && m.kind == MarkupKind::Area)
            .filter(|(m, _)| matches!(&m.geometry, Geometry::Polygon { pts, .. } if markup_model::geom::point_in_ring(first, pts)))
            .min_by(|a, b| {
                let size = |m: &MeasureMarkup| m.geometry.bounds().map_or(f64::MAX, |bounds| bounds.area());
                size(a.0).total_cmp(&size(b.0))
            })
            .map(|(m, _)| m.clone());
        let Some(mut area) = smallest else {
            self.toast("A cutout goes inside an area: draw the area first.".to_owned());
            return;
        };
        if let Geometry::Polygon { holes, .. } = &mut area.geometry {
            holes.push(ring);
        }
        let id = area.id;
        doc.session.apply(crate::session::Command::ChangeMeasure(Box::new(area)));
        self.active_measure = Some(id);
    }

    /// Esc, Enter, Backspace and Delete for measurements: Esc drops what is
    /// half-drawn, then the tool, then the selection; Enter finishes a run or
    /// an area; Backspace takes back a point; Delete removes what is picked
    /// out, whichever tool is in hand.
    pub(super) fn measure_keys(&mut self, ctx: &egui::Context) {
        let anything = self.measure_tool.is_some() || self.placing.is_some() || self.active_measure.is_some();
        if !anything || self.doc.is_none() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let (escape, enter, back, delete) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Backspace),
                i.consume_key(Modifiers::NONE, Key::Delete),
            )
        });
        if escape {
            // The first Esc drops what's half-drawn, the next puts the tool
            // down, the next lets go of what was picked out.
            if self.placing.take().is_none() {
                if self.measure_tool.is_some() {
                    self.set_measure_tool(None);
                } else {
                    self.active_measure = None;
                }
            }
        }
        if enter {
            self.finish_measurement();
        }
        if back {
            self.take_back_point();
        }
        if delete {
            self.delete_selection();
        }
    }

    /// The measurement under a point on `page`, and what part of it.
    pub(super) fn measurement_at(&self, page: usize, pos: Pos2) -> Option<(MarkupId, Hit)> {
        let point = self.pdf_point(page, pos)?;
        measurement_at_in(self.doc.as_ref()?, page, point, PICK_SLACK * self.points_per_screen(page))
    }

    /// Takes back the last point placed, staying in the tool. Says whether
    /// there was one: Ctrl+Z takes back a point while a shape is being
    /// drawn, and only undoes the last change once it is finished.
    pub(super) fn take_back_point(&mut self) -> bool {
        self.placing.as_mut().is_some_and(Placing::take_back)
    }

    /// Puts back the last point taken back, for Ctrl+Y while drawing.
    pub(super) fn put_back_point(&mut self) -> bool {
        self.placing.as_mut().is_some_and(Placing::put_back)
    }

    /// Undo or redo: a point of the shape being drawn if there is one, or
    /// else the last change to the document.
    pub(super) fn undo_step(&mut self, redo: bool) {
        let took = if redo { self.put_back_point() } else { self.take_back_point() };
        if !took {
            self.undo(redo);
        }
    }

    /// Whether there is anything to undo or redo, a half-drawn shape's points
    /// included.
    pub(super) fn can_undo(&self, redo: bool) -> bool {
        let placing = self.placing.as_ref();
        let points = placing.is_some_and(|p| if redo { !p.undone.is_empty() } else { !p.points.is_empty() });
        points || self.doc.as_ref().is_some_and(|d| if redo { d.session.can_redo() } else { d.session.can_undo() })
    }

    /// Picks out the measurement under a press and takes hold of what was
    /// pressed: a corner to move it, the middle of an edge to add a corner
    /// there, or anywhere else on it to move the whole thing. Says whether it
    /// took the press.
    pub(super) fn pick_measurement(&mut self, page: usize, pos: Pos2) -> bool {
        let Some((id, hit)) = self.measurement_at(page, pos) else {
            self.active_measure = None;
            self.active_vertex = None;
            return false;
        };
        self.active_measure = Some(id);
        self.active_vertex = None;
        match hit {
            Hit::Vertex { ring, index } => {
                self.active_vertex = Some((ring, index));
                self.drag = Some(Drag::MeasureVertex { id, ring, index, page });
            }
            // Grabbing the middle of an edge puts a corner there and drags it,
            // which is how a shape gains a point.
            Hit::Midpoint { ring, index } => {
                if self.add_vertex(id, ring, index + 1, pos, page) {
                    self.active_vertex = Some((ring, index + 1));
                    self.drag = Some(Drag::MeasureVertex { id, ring, index: index + 1, page });
                }
            }
            Hit::Edge { .. } | Hit::Inside => {
                if let Some(from) = self.pdf_point(page, pos) {
                    self.drag = Some(Drag::MeasureBody { id, page, from });
                }
            }
        }
        true
    }

    /// Puts a corner into a measurement at `index` of ring `ring`, where the
    /// pointer is. Says whether it could.
    fn add_vertex(&mut self, id: MarkupId, ring: usize, index: usize, pos: Pos2, page: usize) -> bool {
        let Some(point) = self.pdf_point(page, pos) else { return false };
        let (_, at) = self.snapped(page, point, None);
        let Some(doc) = self.doc.as_mut() else { return false };
        let Some(markup) = doc.session.measures().get(id) else { return false };
        let mut changed = markup.clone();
        if !changed.geometry.insert_vertex(ring, index, Pt::new(f64::from(at.0), f64::from(at.1))) {
            return false;
        }
        doc.session.apply(crate::session::Command::ChangeMeasure(Box::new(changed)));
        true
    }

    /// Moves a whole measurement as the pointer moves, from where it was
    /// grabbed.
    pub(super) fn drag_measure_body(&mut self, page: usize, id: MarkupId, from: (f32, f32), pos: Pos2) {
        let Some(point) = self.pdf_point(page, pos) else { return };
        let delta = Pt::new(f64::from(point.0 - from.0), f64::from(point.1 - from.1));
        if delta.len() == 0.0 {
            return;
        }
        if let Some(Drag::MeasureBody { from, .. }) = self.drag.as_mut() {
            *from = point;
        }
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        let mut moved = markup.clone();
        moved.geometry = moved.geometry.moved_by(delta);
        doc.session.apply_merged(crate::session::Command::ChangeMeasure(Box::new(moved)));
    }

    /// Delete: the corner picked out if there is one and the shape can spare
    /// it, otherwise the whole measurement.
    pub(super) fn delete_selection(&mut self) {
        let Some(id) = self.active_measure else { return };
        let vertex = self.active_vertex;
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        if let Some((ring, index)) = vertex {
            let mut changed = markup.clone();
            if changed.geometry.remove_vertex(ring, index) {
                doc.session.apply(crate::session::Command::ChangeMeasure(Box::new(changed)));
                self.active_vertex = None;
                return;
            }
        }
        doc.session.apply(crate::session::Command::RemoveMeasure(id));
        self.active_measure = None;
        self.active_vertex = None;
    }

    /// Moves the vertex being dragged to `pos`.
    pub(super) fn drag_measure_vertex(&mut self, page: usize, id: MarkupId, ring: usize, index: usize, pos: Pos2) {
        let Some(point) = self.pdf_point(page, pos) else { return };
        let (_, at) = self.snapped(page, point, None);
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        let mut moved = markup.clone();
        if let Some(vertex) = moved.geometry.vertex_mut(ring, index) {
            *vertex = Pt::new(f64::from(at.0), f64::from(at.1));
            // Kept as one change while the drag lasts, so undo takes the whole
            // move back rather than a frame of it.
            doc.session.apply_merged(crate::session::Command::ChangeMeasure(Box::new(moved)));
        }
    }

    /// The measurement being placed, with the pointer as its next point: the
    /// page, its kind, and the points, worked out before the pages are drawn
    /// so the drawing itself needs nothing of the app.
    pub(super) fn placing_preview(&self) -> Option<Preview> {
        let placing = self.placing.as_ref()?;
        let mut points = placing.points.clone();
        if let Some(next) = self.pointer_on_page(placing.page) {
            points.push(next);
        }
        Some((placing.page, placing.kind, points))
    }

    /// Where the pointer is on `page`, snapped, while placing.
    fn pointer_on_page(&self, page: usize) -> Option<(f32, f32)> {
        let pos = self.ctx.pointer_latest_pos()?;
        let rect = self.page_rects.get(&page)?;
        if !rect.contains(pos) {
            return None;
        }
        let point = self.pdf_point(page, pos)?;
        let from = self.placing.as_ref().and_then(|p| p.points.last().copied());
        Some(self.snapped(page, point, from).1)
    }
}

/// How measurements are drawn: the one picked out, the one being placed, and
/// the colour and width new ones take.
pub(super) struct Painting<'a> {
    pub(super) active: Option<MarkupId>,
    /// Whether what's being placed is a cutout, which is drawn as an outline:
    /// it takes area away rather than adding it.
    pub(super) cutting_out: bool,
    /// Which corner of it is picked out, if any.
    pub(super) active_vertex: Option<(usize, usize)>,
    pub(super) placing: Option<&'a Preview>,
    pub(super) colour: crate::model::Rgb,
    pub(super) width: f32,
}

/// Draws page `page`'s measurements, with what they measure, and the one
/// being placed.
pub(super) fn paint_measurements(painter: &egui::Painter, doc: &Doc, page: usize, rect: Rect, g: &PageGeometry, how: &Painting) {
    let at = |p: Pt| {
        let (fx, fy) = g.to_view(p.x as f32, p.y as f32);
        pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height())
    };
    let per_point = rect.width() / doc.sizes[page].x;
    let scale = crate::app::scale::page_scale(doc, page);
    let units = scale.map_or(Default::default(), |s| s.display);
    let precision = scale.map_or(Default::default(), |s| s.precision);
    for (markup, measured) in doc.session.measures().iter().filter(|(m, _)| m.page as usize == page) {
        let colour = to_color32(markup.style.stroke);
        let stroke = Stroke::new((markup.style.width as f32 * per_point).max(1.0), colour);
        let points: Vec<Pos2> = outline_of(&markup.geometry).iter().map(|&p| at(p)).collect();
        // An area's triangles come from the session, worked out when it last
        // changed rather than every frame.
        let triangles: Vec<[Pos2; 3]> =
            measured.triangles.iter().map(|t| [at(t[0]), at(t[1]), at(t[2])]).collect();
        let closed = matches!(markup.geometry, Geometry::Polygon { .. });
        paint_shape(painter, &points, &triangles, closed, colour, stroke);
        // Cutouts: outlined, with nothing filled inside them.
        if let Geometry::Polygon { holes, .. } = &markup.geometry {
            for hole in holes {
                let ring: Vec<Pos2> = hole.iter().map(|&p| at(p)).collect();
                paint_joined(painter, &ring, true, stroke);
            }
        }
        if how.active == Some(markup.id) {
            paint_handles(painter, &markup.geometry, how.active_vertex, matches!(markup.geometry, Geometry::Polygon { .. }), &at);
        }
        let text = match &measured.result {
            Ok(q) => q.text(markup.kind, &units, precision),
            Err(e) => Some(e.to_string()),
        };
        if let (Some(text), Some(middle)) = (text, label_at(&points, matches!(markup.geometry, Geometry::Polygon { .. }))) {
            paint_label(painter, middle, &text, colour);
        }
    }

    let Some((on, kind, points)) = how.placing.filter(|(on, ..)| *on == page) else { return };
    let screen: Vec<Pos2> = points.iter().map(|&(x, y)| at(Pt::new(f64::from(x), f64::from(y)))).collect();
    let colour = to_color32(how.colour);
    let stroke = Stroke::new((how.width * per_point).max(1.0), colour);
    // The one being placed changes every frame anyway, so its triangles are
    // worked out here.
    let placing_triangles: Vec<[Pos2; 3]> = if *kind == MarkupKind::Area && !how.cutting_out {
        let ring: Vec<Pt> = screen.iter().map(|p| Pt::new(f64::from(p.x), f64::from(p.y))).collect();
        markup_model::geom::triangulate(&ring).into_iter().map(|t| [pos2(t[0].x as f32, t[0].y as f32), pos2(t[1].x as f32, t[1].y as f32), pos2(t[2].x as f32, t[2].y as f32)]).collect()
    } else {
        Vec::new()
    };
    paint_shape(painter, &screen, &placing_triangles, *kind == MarkupKind::Area, colour, stroke);
    for point in &screen {
        painter.circle_filled(*point, 3.0, colour);
    }
    if points.len() >= least_points(*kind) {
        if let Some(q) = measured(*kind, points, *on, scale) {
            if let (Some(text), Some(middle)) = (q.text(*kind, &units, precision), label_at(&screen, *kind == MarkupKind::Area)) {
                paint_label(painter, middle, &text, colour);
            }
        }
    }
}


/// Where a shape's label goes: half way along a line, and inside an area
/// rather than at the average of its corners, which for a shape with a notch
/// in it can fall outside the shape altogether.
fn label_at(points: &[Pos2], area: bool) -> Option<Pos2> {
    match points {
        [] => None,
        [one] => Some(*one),
        many if area => {
            // The middle of its largest triangle is always within it.
            let ring: Vec<Pt> = many.iter().map(|p| Pt::new(f64::from(p.x), f64::from(p.y))).collect();
            let biggest = markup_model::geom::triangulate(&ring).into_iter().max_by(|a, b| {
                markup_model::geom::signed_area(a).abs().total_cmp(&markup_model::geom::signed_area(b).abs())
            });
            match biggest {
                Some(t) => Some(pos2(
                    ((t[0].x + t[1].x + t[2].x) / 3.0) as f32,
                    ((t[0].y + t[1].y + t[2].y) / 3.0) as f32,
                )),
                None => Some(many[0]),
            }
        }
        many => {
            // Half way along, by length, so a long run labels where the eye
            // follows it rather than where its corners happen to average.
            let total: f32 = many.windows(2).map(|w| w[0].distance(w[1])).sum();
            let mut left = total / 2.0;
            for w in many.windows(2) {
                let step = w[0].distance(w[1]);
                if left <= step || step == 0.0 {
                    return Some(w[0] + (w[1] - w[0]) * if step > 0.0 { left / step } else { 0.0 });
                }
                left -= step;
            }
            many.last().copied()
        }
    }
}

/// A quantity, in a small white card so it reads over any drawing.
fn paint_label(painter: &egui::Painter, at: Pos2, text: &str, colour: Color32) {
    let font = FontId::proportional(12.0);
    let galley = painter.layout_no_wrap(text.to_owned(), font, TEXT);
    let box_rect = Rect::from_center_size(at, galley.size() + vec2(10.0, 6.0));
    painter.rect_filled(box_rect, CornerRadius::same(3), SURFACE.gamma_multiply(0.92));
    painter.rect_stroke(box_rect, CornerRadius::same(3), Stroke::new(1.0, colour.gamma_multiply(0.5)), StrokeKind::Middle);
    painter.galley(box_rect.center() - galley.size() / 2.0, galley, TEXT);
}

/// Draws a measurement's shape: an area filled and closed, anything else as a
/// line through its points.
///
/// An area is cut into triangles first. Filling it as one shape would only be
/// right while it stays convex: a shape with a notch in it would be filled
/// across the notch, and spikes would shoot out of it across the page.
fn paint_shape(painter: &egui::Painter, points: &[Pos2], triangles: &[[Pos2; 3]], area: bool, colour: Color32, stroke: Stroke) {
    if !area {
        // An open path mitres its corners within reason; a closed one does
        // not (see the tests), so only closed shapes have their joins drawn.
        painter.add(Shape::line(points.to_vec(), stroke));
        return;
    }
    // An outline that crosses itself has no triangles: it shows as an outline
    // alone, which is the honest picture, and its quantity says what's wrong.
    //
    // Drawn as filled shapes, each triangle would be smoothed at its edges by
    // spreading its corners outwards, by one over the sine of half the angle:
    // a sliver, which is what cutting up a shape that doubles back gives, then
    // throws a long faint spike out of the shape. A mesh is drawn as it is.
    if !triangles.is_empty() {
        let fill = colour.gamma_multiply(0.15);
        let mut mesh = egui::epaint::Mesh::default();
        for triangle in triangles {
            let base = mesh.vertices.len() as u32;
            for corner in triangle {
                mesh.vertices.push(egui::epaint::Vertex { pos: *corner, uv: egui::epaint::WHITE_UV, color: fill });
            }
            mesh.indices.extend([base, base + 1, base + 2]);
        }
        painter.add(Shape::mesh(mesh));
    }
    paint_joined(painter, points, true, stroke);
}

/// The points a measurement is drawn through, in the order they join up. A
/// line's two ends are held as a ring each, since that is how a vertex is
/// addressed for dragging, so they are put back together here.
fn outline_of(geometry: &Geometry) -> Vec<Pt> {
    match geometry {
        Geometry::Line { a, b } => vec![*a, *b],
        Geometry::Polyline { pts } | Geometry::Points { pts } | Geometry::Polygon { pts, .. } => pts.clone(),
        Geometry::Ellipse { rect } => rect.corners().to_vec(),
        Geometry::Ink { strokes } => strokes.first().cloned().unwrap_or_default(),
    }
}

/// Draws a line through `points`, each piece on its own with a round patch at
/// the joints.
///
/// Drawn as one closed path, a corner that doubles back on itself grows a
/// spike: the join reaches out by one over the cosine of half the angle, and
/// a closed path has no limit on it -- an outline 200 points across reached
/// 800. Estimators trace back along a line all the time, around a kerb or
/// back down a return, so the joins are drawn rather than mitred.
pub(super) fn paint_joined(painter: &egui::Painter, points: &[Pos2], closed: bool, stroke: Stroke) {
    if points.len() < 2 {
        // A single point still shows, as a dot the width of the line.
        if let [one] = points {
            painter.circle_filled(*one, stroke.width.max(1.0) / 2.0, stroke.color);
        }
        return;
    }
    let ends = points.len() - usize::from(!closed);
    for i in 0..ends {
        let (from, to) = (points[i], points[(i + 1) % points.len()]);
        painter.line_segment([from, to], stroke);
    }
    // The joints, and the ends of a closed shape, filled round.
    let joints = if closed { 0..points.len() } else { 1..points.len() - 1 };
    for i in joints {
        painter.circle_filled(points[i], stroke.width / 2.0, stroke.color);
    }
}

/// The measurement at a point in user space, and what part of it, from a
/// document being drawn.
pub(super) fn measurement_at_in(doc: &Doc, page: usize, (x, y): (f32, f32), slack: f32) -> Option<(MarkupId, Hit)> {
    doc.session.measures().pick(page as u32, Pt::new(f64::from(x), f64::from(y)), f64::from(slack))
}


/// The handles of the measurement picked out: a filled square at each corner,
/// the one picked out larger, and a hollow one in the middle of each edge,
/// which is where a corner is added.
fn paint_handles(painter: &egui::Painter, geometry: &Geometry, active: Option<(usize, usize)>, closed: bool, at: &impl Fn(Pt) -> Pos2) {
    let rings: Vec<Vec<Pt>> = match geometry {
        // A line's two ends are a ring each; its handles are one ring.
        Geometry::Line { a, b } => vec![vec![*a, *b]],
        _ => geometry.rings().iter().map(|ring| ring.to_vec()).collect(),
    };
    for (r, ring) in rings.iter().enumerate() {
        let edges = if closed && ring.len() > 2 { ring.len() } else { ring.len().saturating_sub(1) };
        for i in 0..edges {
            let middle = at(ring[i]).lerp(at(ring[(i + 1) % ring.len()]), 0.5);
            let box_ = Rect::from_center_size(middle, vec2(6.0, 6.0));
            painter.rect_filled(box_, CornerRadius::same(1), SURFACE);
            painter.rect_stroke(box_, CornerRadius::same(1), Stroke::new(1.0, ACCENT), StrokeKind::Middle);
        }
        for (i, point) in ring.iter().enumerate() {
            let size = if active == Some((r, i)) { 11.0 } else { 8.0 };
            painter.rect_filled(Rect::from_center_size(at(*point), vec2(size, size)), CornerRadius::same(1), ACCENT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markup_model::markup::Geometry;

    /// The bounds of the triangles a shape really comes to. A shape's own
    /// bounding box is worked out from its points, so it would hide the very
    /// spike these tests are about.
    fn tessellated_bounds(shapes: Vec<Shape>) -> Rect {
        use eframe::egui::epaint::{tessellator::Tessellator, Mesh, TessellationOptions};
        let mut mesh = Mesh::default();
        let mut tessellator = Tessellator::new(1.0, TessellationOptions::default(), [16, 16], vec![]);
        for shape in shapes {
            tessellator.tessellate_shape(shape, &mut mesh);
        }
        mesh.vertices.iter().fold(Rect::NOTHING, |bounds, v| bounds.union(Rect::from_min_max(v.pos, v.pos)))
    }

    /// The bounds of what `paint_joined` draws for these points.
    fn drawn_bounds(points: &[Pos2], closed: bool) -> Rect {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ctx| {
            paint_joined(&ctx.debug_painter(), points, closed, Stroke::new(3.0, Color32::RED));
        });
        // The fonts it prepared aren't wanted here, only the shapes.
        output.textures_delta.clear();
        tessellated_bounds(output.shapes.into_iter().map(|clipped| clipped.shape).collect())
    }

    #[test]
    fn a_line_doubling_back_on_itself_grows_no_spike() {
        // Out and almost straight back on itself: the sharpest corner there
        // is, and the one that used to shoot a spike off the page.
        let points = [pos2(100.0, 100.0), pos2(300.0, 100.0), pos2(101.0, 102.0)];
        let drawn = drawn_bounds(&points, false);
        let want = Rect::from_points(&points).expand(4.0);
        assert!(want.contains_rect(drawn), "drawn {drawn:?} reaches outside the line {want:?}");

        // The same corner in a closed shape.
        let ring = [pos2(100.0, 100.0), pos2(300.0, 100.0), pos2(101.0, 102.0), pos2(100.0, 300.0)];
        let drawn = drawn_bounds(&ring, true);
        let want = Rect::from_points(&ring).expand(4.0);
        assert!(want.contains_rect(drawn), "drawn {drawn:?} reaches outside the shape {want:?}");
    }

    #[test]
    fn drawn_as_one_closed_path_that_corner_spikes() {
        // Why a closed shape's joins are drawn: a closed path mitres its
        // corners without limit, so an outline 200 points across reaches 800.
        // If this ever stops being true, `paint_joined` can go.
        let points = vec![pos2(100.0, 100.0), pos2(300.0, 100.0), pos2(101.0, 102.0)];
        let drawn = tessellated_bounds(vec![Shape::closed_line(points.clone(), Stroke::new(3.0, Color32::RED))]);
        assert!(!Rect::from_points(&points).expand(4.0).contains_rect(drawn), "the mitre reaches out to {drawn:?}");
        // An open path keeps to itself, so those are drawn as one path.
        let open = tessellated_bounds(vec![Shape::line(points.clone(), Stroke::new(3.0, Color32::RED))]);
        assert!(Rect::from_points(&points).expand(4.0).contains_rect(open), "an open path reaches out to {open:?}");
    }

    #[test]
    fn an_area_draws_nothing_outside_itself() {
        // Fill and outline together, for a triangle and for a shape with a
        // notch: nothing drawn may reach past the corners, however thin a
        // piece of it is.
        for ring in [
            vec![pos2(100.0, 100.0), pos2(300.0, 120.0), pos2(280.0, 400.0)],
            vec![pos2(100.0, 100.0), pos2(300.0, 100.0), pos2(300.0, 300.0), pos2(200.0, 150.0), pos2(100.0, 300.0)],
            // A near-straight corner, where a sliver triangle comes out.
            vec![pos2(100.0, 100.0), pos2(200.0, 100.5), pos2(300.0, 100.0), pos2(300.0, 200.0)],
            // Doubling back on itself, which cuts into slivers: filled as
            // shapes, one of these threw a spike hundreds of points long.
            vec![pos2(100.0, 100.0), pos2(300.0, 100.0), pos2(299.0, 100.6), pos2(299.0, 300.0), pos2(100.0, 300.0)],
        ] {
            let points: Vec<Pt> = ring.iter().map(|p| Pt::new(f64::from(p.x), f64::from(p.y))).collect();
            let triangles: Vec<[Pos2; 3]> = markup_model::geom::triangulate(&points)
                .into_iter()
                .map(|t| [pos2(t[0].x as f32, t[0].y as f32), pos2(t[1].x as f32, t[1].y as f32), pos2(t[2].x as f32, t[2].y as f32)])
                .collect();
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(Default::default(), |ctx| {
                paint_shape(&ctx.debug_painter(), &ring, &triangles, true, Color32::RED, Stroke::new(3.0, Color32::RED));
            });
            output.textures_delta.clear();
            let drawn = tessellated_bounds(output.shapes.into_iter().map(|clipped| clipped.shape).collect());
            let want = Rect::from_points(&ring).expand(4.0);
            assert!(want.contains_rect(drawn), "{:?} drawn to {drawn:?}, outside {want:?}", ring.len());
        }
    }

    #[test]
    fn filled_as_a_shape_a_sliver_spikes() {
        // Why an area is filled as a mesh: a filled shape is smoothed by
        // spreading its corners outwards, without limit, so a sliver 200
        // points across reaches 660. Cutting up a shape that doubles back
        // gives slivers, which is where the faint spikes came from.
        let sliver = vec![pos2(100.0, 100.0), pos2(300.0, 100.0), pos2(299.0, 100.6)];
        let filled = tessellated_bounds(vec![Shape::convex_polygon(sliver.clone(), Color32::RED, Stroke::NONE)]);
        assert!(!Rect::from_points(&sliver).expand(2.0).contains_rect(filled), "the smoothing reaches out to {filled:?}");
    }

    #[test]
    fn points_are_taken_back_and_put_down_again_while_drawing() {
        let mut placing = Placing::new(MarkupKind::Area, 0, (10.0, 10.0));
        placing.place((20.0, 10.0));
        placing.place((20.0, 20.0));
        assert!(placing.take_back() && placing.take_back());
        assert_eq!(placing.points, [(10.0, 10.0)]);
        assert!(placing.put_back());
        assert_eq!(placing.points, [(10.0, 10.0), (20.0, 10.0)]);
        // Placing another point is a new course: what was taken back is gone.
        placing.place((30.0, 30.0));
        assert!(!placing.put_back());
        // Taking back every point leaves the tool in hand, with nothing drawn.
        assert!(placing.take_back() && placing.take_back() && placing.take_back());
        assert!(placing.points.is_empty() && !placing.take_back());
    }

    #[test]
    fn a_length_is_drawn_through_both_its_ends() {
        let line = Geometry::Line { a: Pt::new(10.0, 20.0), b: Pt::new(30.0, 40.0) };
        assert_eq!(outline_of(&line), vec![Pt::new(10.0, 20.0), Pt::new(30.0, 40.0)]);
        let drawn = drawn_bounds(&[pos2(10.0, 20.0), pos2(30.0, 40.0)], false);
        assert!(drawn.is_positive() && drawn.width() >= 20.0, "a length shows as a line: {drawn:?}");
    }
}
