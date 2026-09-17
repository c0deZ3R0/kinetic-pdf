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
const PICK_SLACK: f32 = 4.0;

/// The page a measurement is being placed on, its kind, and the points
/// placed so far with the pointer's as the last.
pub(super) type Preview = (usize, MarkupKind, Vec<(f32, f32)>);

/// A measurement being placed, click by click.
pub(super) struct Placing {
    pub(super) kind: MarkupKind,
    pub(super) page: usize,
    /// The points placed so far, in PDF user space.
    pub(super) points: Vec<(f32, f32)>,
}

impl MeasureTool {
    /// The kind of measurement a tool makes, if it makes one.
    pub(super) fn kind(self) -> Option<MarkupKind> {
        match self {
            MeasureTool::Length => Some(MarkupKind::Length),
            MeasureTool::Polylength => Some(MarkupKind::Polylength),
            MeasureTool::Area => Some(MarkupKind::Area),
            MeasureTool::Calibrate | MeasureTool::Verify => None,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            MeasureTool::Length => "Length",
            MeasureTool::Polylength => "Polylength",
            MeasureTool::Area => "Area",
            MeasureTool::Calibrate => "Calibrate",
            MeasureTool::Verify => "Check",
        }
    }

    /// What to tell the user while it's in use.
    pub(super) fn hint(self) -> &'static str {
        match self {
            MeasureTool::Length => "Click each end of what you're measuring.",
            MeasureTool::Polylength => "Click along the run. Double-click, or press Enter, to finish it; Backspace takes back a point.",
            MeasureTool::Area => "Click around the area. Double-click, press Enter, or click the first point again to close it.",
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
        for tool in [MeasureTool::Length, MeasureTool::Polylength, MeasureTool::Area] {
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
        match self.placing.as_mut() {
            Some(placing) if placing.page == page => {
                if !closing {
                    placing.points.push(at);
                }
            }
            _ => self.placing = Some(Placing { kind, page, points: vec![at] }),
        }
        let done = match (kind, closing) {
            (MarkupKind::Length, _) => self.placing.as_ref().is_some_and(|p| p.points.len() >= 2),
            (_, closing) => closing,
        };
        if done {
            self.finish_measurement();
        }
    }

    /// Keeps what's been placed, if it's enough to measure.
    pub(super) fn finish_measurement(&mut self) {
        let Some(placing) = self.placing.take() else { return };
        if placing.points.len() < least_points(placing.kind) {
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

    /// Esc, Enter and Backspace while placing a measurement. Says whether it
    /// used the key.
    pub(super) fn measure_keys(&mut self, ctx: &egui::Context) {
        if self.measure_tool.is_none() || ctx.egui_wants_keyboard_input() {
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
            // The first Esc drops what's half-drawn, the next puts the tool down.
            if self.placing.take().is_none() {
                self.set_measure_tool(None);
            }
        }
        if enter {
            self.finish_measurement();
        }
        if back {
            if let Some(placing) = self.placing.as_mut() {
                placing.points.pop();
                if placing.points.is_empty() {
                    self.placing = None;
                }
            }
        }
        if delete {
            if let Some(id) = self.active_measure.take() {
                self.doc.as_mut().map(|doc| doc.session.apply(crate::session::Command::RemoveMeasure(id)));
            }
        }
    }

    /// Picks the measurement under a click, or starts dragging one's vertex.
    /// Says whether it took the click.
    pub(super) fn pick_measurement(&mut self, page: usize, pos: Pos2) -> bool {
        let Some(point) = self.pdf_point(page, pos) else { return false };
        let slack = f64::from(PICK_SLACK * self.points_per_screen(page));
        let at = Pt::new(f64::from(point.0), f64::from(point.1));
        let Some(doc) = self.doc.as_ref() else { return false };
        let Some((id, hit)) = doc.session.measures().pick(page as u32, at, slack) else {
            self.active_measure = None;
            return false;
        };
        self.active_measure = Some(id);
        if let Hit::Vertex { ring, index } = hit {
            self.drag = Some(Drag::MeasureVertex { id, ring, index, page });
        }
        true
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
        let points: Vec<Pos2> = markup.geometry.rings().first().map(|ring| ring.iter().map(|&p| at(p)).collect()).unwrap_or_default();
        if matches!(markup.geometry, Geometry::Polygon { .. }) && points.len() >= 3 {
            painter.add(Shape::convex_polygon(points.clone(), colour.gamma_multiply(0.15), stroke));
        } else {
            painter.add(Shape::line(points.clone(), stroke));
        }
        if how.active == Some(markup.id) {
            for point in &points {
                painter.rect_filled(Rect::from_center_size(*point, vec2(7.0, 7.0)), CornerRadius::same(1), ACCENT);
            }
        }
        let text = match &measured.result {
            Ok(q) => q.text(markup.kind, &units, precision),
            Err(e) => Some(e.to_string()),
        };
        if let (Some(text), Some(middle)) = (text, label_at(&points)) {
            paint_label(painter, middle, &text, colour);
        }
    }

    let Some((on, kind, points)) = how.placing.filter(|(on, ..)| *on == page) else { return };
    let screen: Vec<Pos2> = points.iter().map(|&(x, y)| at(Pt::new(f64::from(x), f64::from(y)))).collect();
    let colour = to_color32(how.colour);
    let stroke = Stroke::new((how.width * per_point).max(1.0), colour);
    if *kind == MarkupKind::Area && screen.len() >= 3 {
        painter.add(Shape::convex_polygon(screen.clone(), colour.gamma_multiply(0.12), stroke));
    } else {
        painter.add(Shape::line(screen.clone(), stroke));
    }
    for point in &screen {
        painter.circle_filled(*point, 3.0, colour);
    }
    if points.len() >= least_points(*kind) {
        if let Some(q) = measured(*kind, points, *on, scale) {
            if let (Some(text), Some(middle)) = (q.text(*kind, &units, precision), label_at(&screen)) {
                paint_label(painter, middle, &text, colour);
            }
        }
    }
}


/// Where a shape's label goes: the middle of its longest side for a line,
/// the middle of the shape for an area.
fn label_at(points: &[Pos2]) -> Option<Pos2> {
    match points {
        [] => None,
        [one] => Some(*one),
        many => {
            let (mut sum, mut count) = (Vec2::ZERO, 0.0);
            for p in many {
                sum += p.to_vec2();
                count += 1.0;
            }
            Some((sum / count).to_pos2())
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
