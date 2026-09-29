//! Turning and stretching what's picked out, as a drawing program does: a
//! frame round it with handles to stretch it by, which a click on it again
//! swaps for handles to turn it by, and back.
//!
//! The frame is square to the sheet as it's seen, round everything picked
//! out on the page -- or, for one text box, clip, rectangle or ellipse
//! alone, that thing's own box, turned as it is, so it stretches along its
//! own sides. A drag works out one map of the page (`Affine`) from where it
//! started to where the pointer is, and puts it on each thing as it was
//! when the drag began, so nothing creeps however long it goes on, and the
//! whole drag is one step to undo. Each kind takes the map its own way:
//! see `markup_model::affine`, and `drawing_reshaped` for drawn markups.
//!
//! Shift stretches about the middle; Ctrl keeps the proportions, and turns
//! in steps of 15 degrees. Drawings already saved into the file, and
//! highlights, stay as they are.

use std::f32::consts::FRAC_PI_2;

use markup_model::markup::{Geometry, MarkupKind as MeasureKind};
use markup_model::{box_mapped, Affine, Frame, Pt};

use super::*;
use crate::model::MeasureMarkup;

/// How far out from the frame a handle sits, in screen points.
const OUT: f32 = 12.0;
/// How near a handle the pointer has to be to take hold of it.
const REACH: f32 = 9.0;
/// Turning with Ctrl goes in steps this big.
const TURN_STEP: f64 = std::f64::consts::PI / 12.0;

/// Which handles show, and what a click does to them.
#[derive(Default)]
pub(super) struct Reshaping {
    /// What was picked out when its handles were swapped for turning ones:
    /// picking anything else brings the stretching ones back.
    turning: Option<Vec<RowId>>,
    /// The press going on landed on something already picked out, so if it
    /// comes to a click, the click swaps the handles.
    pub(super) pressed_picked: bool,
}

/// A handle on the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Handle {
    /// A corner: 0 bottom left, round anticlockwise, as a frame's corners go.
    Corner(u8),
    /// The middle of a side: 0 the bottom, round anticlockwise.
    Side(u8),
}

impl Handle {
    const ALL: [Handle; 8] = [Handle::Corner(0), Handle::Side(0), Handle::Corner(1), Handle::Side(1), Handle::Corner(2), Handle::Side(2), Handle::Corner(3), Handle::Side(3)];

    /// Where it is on the frame, 0 to 1 across and up.
    fn at(self) -> (f64, f64) {
        match self {
            Handle::Corner(0) => (0.0, 0.0),
            Handle::Corner(1) => (1.0, 0.0),
            Handle::Corner(2) => (1.0, 1.0),
            Handle::Corner(_) => (0.0, 1.0),
            Handle::Side(0) => (0.5, 0.0),
            Handle::Side(1) => (1.0, 0.5),
            Handle::Side(2) => (0.5, 1.0),
            Handle::Side(_) => (0.0, 0.5),
        }
    }
}

/// What's picked out on one page that can be turned and stretched, and the
/// frame round it, in user space.
#[derive(Clone)]
pub(super) struct Selection {
    pub(super) page: usize,
    pub(super) frame: Frame,
    rows: Vec<RowId>,
    pub(super) turning: bool,
}

impl Selection {
    /// Whether it's more than a line along one of the frame's sides: a frame
    /// with no width, or no height, has nothing to stretch that way.
    fn stretches(&self) -> (bool, bool) {
        (self.frame.width > 0.5, self.frame.height > 0.5)
    }

    fn shows(&self, handle: Handle) -> bool {
        let (across, up) = self.stretches();
        match handle {
            // Turning goes by the corners alone.
            Handle::Side(_) if self.turning => false,
            Handle::Side(0 | 2) => up,
            Handle::Side(_) => across,
            Handle::Corner(_) => across || up,
        }
    }
}

/// A handle being dragged: the frame, and everything as it was, when the
/// drag began.
pub(super) struct Reshape {
    pub(super) sheet: usize,
    handle: Handle,
    pub(super) turning: bool,
    /// The pointer while it goes on: the handle's own.
    pub(super) cursor: CursorIcon,
    start: Pt,
    frame: Frame,
    was: Vec<Was>,
}

enum Was {
    Measure(Box<MeasureMarkup>),
    Drawing { uid: u64, kind: MarkupKind, points: Vec<[f32; 2]> },
}

/// Where each handle of `selection` is on the sheet drawn at `rect` with
/// geometry `g`, and which way is out from the frame there, on screen.
pub(super) fn handles(selection: &Selection, rect: Rect, g: &PageGeometry) -> Vec<(Handle, Pos2, Vec2)> {
    let f = &selection.frame;
    let at = |x: f64, y: f64| {
        let p = f.to_user(x * f.width, y * f.height);
        let (fx, fy) = g.to_view(p.x as f32, p.y as f32);
        pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height())
    };
    // Which way across and up the frame run on screen, to put a handle out
    // from its corner or side whatever size the frame is.
    let (across, up) = ((at(1.0, 0.5) - at(0.0, 0.5)).normalized(), (at(0.5, 1.0) - at(0.5, 0.0)).normalized());
    let across = if across == Vec2::ZERO { vec2(1.0, 0.0) } else { across };
    let up = if up == Vec2::ZERO { vec2(-across.y, across.x) } else { up };
    Handle::ALL
        .into_iter()
        .filter(|h| selection.shows(*h))
        .map(|h| {
            let (x, y) = h.at();
            let out = (across * (x as f32 * 2.0 - 1.0) + up * (y as f32 * 2.0 - 1.0)).normalized();
            (h, at(x, y) + out * OUT, out)
        })
        .collect()
}

/// A drawn markup's points taken by `m`: a rectangle or an ellipse kept a
/// box by its four corners, as a text box is; anything else point by point.
fn drawing_reshaped(kind: MarkupKind, points: &[[f32; 2]], m: &Affine) -> Vec<[f32; 2]> {
    let pt = |[x, y]: [f32; 2]| Pt::new(f64::from(x), f64::from(y));
    let back = |p: Pt| [p.x as f32, p.y as f32];
    if let Some(corners) = crate::markup::box_corners(kind, points) {
        let corners: Vec<Pt> = corners.into_iter().map(pt).collect();
        if let Some(mapped) = box_mapped(&corners, m, false) {
            return mapped.into_iter().map(back).collect();
        }
    }
    points.iter().map(|&p| back(m.apply(pt(p)))).collect()
}

impl App {
    /// Whether the handles showing are the turning ones.
    fn is_turning(&self) -> bool {
        self.reshaping.turning.as_ref().is_some_and(|rows| *rows == self.picked_rows())
    }

    /// What's picked out that can be turned and stretched, on the page of
    /// what was picked first, and the frame round it. `None` with nothing
    /// that can, or a box being typed into.
    pub(super) fn selection(&self) -> Option<Selection> {
        if !self.selecting() || self.text_editing.is_some() {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let picked = self.picked_rows();
        let page = match picked.first()? {
            RowId::Measure(id) => doc.session.measures().get(*id)?.page as usize,
            RowId::Drawing(uid) => doc.session.markup(*uid)?.markup.page,
            RowId::Note(_) => return None,
        };
        let mut rows = Vec::new();
        let mut points: Vec<Pt> = Vec::new();
        // A lone box's own frame, turned as it is.
        let mut own: Option<Frame> = None;
        for row in picked {
            match row {
                RowId::Measure(id) => {
                    let Some(m) = doc.session.measures().get(id).filter(|m| m.page as usize == page) else { continue };
                    let mut all = m.clone();
                    all.for_each_place_mut(|p| points.push(*p));
                    if let (MeasureKind::Clip | MeasureKind::Text | MeasureKind::Box | MeasureKind::Ellipse, Geometry::Polygon { pts, .. }) = (m.kind, &m.geometry) {
                        own = Frame::of(pts);
                    }
                }
                RowId::Drawing(uid) => {
                    let Some(e) = doc.session.markup(uid).filter(|e| e.markup.page == page && e.markup.key.is_none() && !e.markup.points.is_empty()) else { continue };
                    let pt = |[x, y]: [f32; 2]| Pt::new(f64::from(x), f64::from(y));
                    match crate::markup::box_corners(e.markup.kind, &e.markup.points) {
                        Some(corners) => {
                            let corners: Vec<Pt> = corners.into_iter().map(pt).collect();
                            own = Frame::of(&corners);
                            points.extend(corners);
                        }
                        None => points.extend(e.markup.points.iter().map(|&p| pt(p))),
                    }
                }
                RowId::Note(_) => continue,
            }
            rows.push(row);
        }
        let frame = match own.filter(|_| rows.len() == 1) {
            Some(frame) => frame,
            None => {
                // Square to the sheet, round everything.
                let sheet = doc.first_sheet_showing(page)?;
                let (right, up) = self.sheet_axes(sheet)?;
                let (xs, ys): (Vec<f64>, Vec<f64>) = points.iter().map(|p| (p.dot(right), p.dot(up))).unzip();
                let (x0, x1) = (xs.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max));
                let (y0, y1) = (ys.iter().copied().fold(f64::INFINITY, f64::min), ys.iter().copied().fold(f64::NEG_INFINITY, f64::max));
                if !(x0.is_finite() && y0.is_finite()) {
                    return None;
                }
                Frame { origin: right * x0 + up * y0, across: right, up, width: x1 - x0, height: y1 - y0 }
            }
        };
        let selection = Selection { page, frame, rows, turning: self.is_turning() };
        let (across, up) = selection.stretches();
        (across || up).then_some(selection)
    }

    /// The handle under `pos` on sheet `sheet`, if the frame shows there.
    pub(super) fn handle_at(&self, sheet: usize, pos: Pos2) -> Option<Handle> {
        let selection = self.selection()?;
        let doc = self.doc.as_ref()?;
        if doc.sheet_page(sheet)? != selection.page {
            return None;
        }
        let (rect, g) = (*self.page_rects.get(&sheet)?, doc.sheet_geometry(sheet)?);
        handles(&selection, rect, &g).into_iter().filter(|(_, at, _)| at.distance(pos) <= REACH).min_by(|a, b| a.1.distance(pos).total_cmp(&b.1.distance(pos))).map(|(h, _, _)| h)
    }

    /// A click on what's already picked out swaps its stretching handles for
    /// turning ones, and back.
    pub(super) fn swap_handles(&mut self) {
        self.reshaping.turning = if self.is_turning() { None } else { Some(self.picked_rows()) };
    }

    /// Takes hold of handle `handle` on sheet `sheet` at `pos`.
    pub(super) fn start_reshape(&mut self, sheet: usize, pos: Pos2, handle: Handle) {
        let Some(selection) = self.selection() else { return };
        let Some((x, y)) = self.pdf_point(sheet, pos) else { return };
        let Some(doc) = self.doc.as_ref() else { return };
        let was = selection
            .rows
            .iter()
            .filter_map(|row| match *row {
                RowId::Measure(id) => doc.session.measures().get(id).map(|m| Was::Measure(Box::new(m.clone()))),
                RowId::Drawing(uid) => doc.session.markup(uid).map(|e| Was::Drawing { uid, kind: e.markup.kind, points: e.markup.points.clone() }),
                RowId::Note(_) => None,
            })
            .collect();
        let start = Pt::new(f64::from(x), f64::from(y));
        let out = match (self.page_rects.get(&sheet), doc.sheet_geometry(sheet)) {
            (Some(&rect), Some(g)) => handles(&selection, rect, &g).into_iter().find(|(h, _, _)| *h == handle).map_or(vec2(1.0, 0.0), |(_, _, out)| out),
            _ => vec2(1.0, 0.0),
        };
        let cursor = if selection.turning { CursorIcon::Grabbing } else { stretch_cursor(out) };
        self.drag = Some(Drag::Reshape(Box::new(Reshape { sheet, handle, turning: selection.turning, cursor, start, frame: selection.frame, was })));
        self.popup = None;
    }

    /// Follows the pointer at `pos` with the handle being dragged: the map
    /// from where the drag began, put on everything as it was then.
    pub(super) fn drag_reshape(&mut self, pos: Pos2, ctrl: bool, shift: bool) {
        let Some(Drag::Reshape(r)) = self.drag.as_ref() else { return };
        let Some((x, y)) = self.pdf_point(r.sheet, pos) else { return };
        let point = Pt::new(f64::from(x), f64::from(y));
        let f = &r.frame;
        let map = if r.turning {
            let middle = f.to_user(f.width / 2.0, f.height / 2.0);
            let angle_of = |p: Pt| (p.y - middle.y).atan2(p.x - middle.x);
            let mut angle = angle_of(point) - angle_of(r.start);
            if ctrl {
                angle = (angle / TURN_STEP).round() * TURN_STEP;
            }
            Affine::rotate_about(middle, angle)
        } else {
            let (hx, hy) = r.handle.at();
            // The side or corner across from the handle stays put, or with
            // Shift the middle.
            let (ax, ay) = if shift { (0.5, 0.5) } else { (1.0 - hx, 1.0 - hy) };
            let ((px, py), (qx, qy)) = (f.to_box(point), f.to_box(r.start));
            // How much further from the anchor the pointer is than where it
            // took hold, so taking hold a little off the frame is no jump.
            let stretch = |handle: f64, anchor: f64, pointer: f64, held: f64, size: f64| {
                let from = held - anchor * size;
                if handle == 0.5 || size <= 0.5 || from.abs() < 1e-6 {
                    return None;
                }
                // Never through the anchor and out the other side: nothing
                // is turned inside out.
                Some(((pointer - anchor * size) / from).max(1.0 / size.max(1.0)))
            };
            let (sx, sy) = (stretch(hx, ax, px, qx, f.width), stretch(hy, ay, py, qy, f.height));
            let (sx, sy) = match (sx, sy, ctrl) {
                // Ctrl keeps the proportions: as far as the handle has gone
                // furthest, or a side's handle, both ways alike.
                (Some(sx), Some(sy), true) => (sx.max(sy), sx.max(sy)),
                (Some(s), None, true) | (None, Some(s), true) => (s, s),
                (sx, sy, _) => (sx.unwrap_or(1.0), sy.unwrap_or(1.0)),
            };
            Affine::scale_along(f, f.to_user(ax * f.width, ay * f.height), sx, sy)
        };
        let now = chrono::Utc::now().timestamp_millis();
        let commands = r
            .was
            .iter()
            .map(|was| match was {
                Was::Measure(m) => {
                    let mut changed = m.transformed(&map);
                    changed.meta.modified_ms = Some(now);
                    Command::ChangeMeasure(Box::new(changed))
                }
                Was::Drawing { uid, kind, points } => Command::Reshape { uid: *uid, points: drawing_reshaped(*kind, points, &map) },
            })
            .collect();
        if let Some(doc) = self.doc.as_mut() {
            doc.session.apply_merged(Command::Batch(commands));
        }
    }

    /// Sets the pointer for what's under it on the frame: arrows along which
    /// a handle stretches, a hand to turn by. `true` if it's over a handle.
    pub(super) fn handle_cursor(&self, ctx: &egui::Context, sheet: usize, pos: Pos2) -> bool {
        let Some(selection) = self.selection() else { return false };
        let Some(doc) = self.doc.as_ref() else { return false };
        if doc.sheet_page(sheet) != Some(selection.page) {
            return false;
        }
        let (Some(&rect), Some(g)) = (self.page_rects.get(&sheet), doc.sheet_geometry(sheet)) else { return false };
        let near = handles(&selection, rect, &g).into_iter().filter(|(_, at, _)| at.distance(pos) <= REACH).min_by(|a, b| a.1.distance(pos).total_cmp(&b.1.distance(pos)));
        let Some((_, _, out)) = near else { return false };
        ctx.set_cursor_icon(if selection.turning { CursorIcon::Grab } else { stretch_cursor(out) });
        true
    }
}

/// The resize pointer along `out`, a handle's way out from the frame.
fn stretch_cursor(out: Vec2) -> CursorIcon {
    // Its angle on screen, folded to a half turn: arrows point both ways.
    let angle = out.y.atan2(out.x).rem_euclid(std::f32::consts::PI);
    let eighth = std::f32::consts::FRAC_PI_8;
    if angle < eighth || angle >= 7.0 * eighth {
        CursorIcon::ResizeHorizontal
    } else if angle < 3.0 * eighth {
        CursorIcon::ResizeNwSe
    } else if angle < 5.0 * eighth {
        CursorIcon::ResizeVertical
    } else {
        CursorIcon::ResizeNeSw
    }
}

/// Draws `selection`'s frame and handles on the sheet drawn at `rect` with
/// geometry `g`: a pair of heads pointing apart to stretch by, or a curved
/// stroke round each corner and a mark at the middle to turn by -- one
/// colour, nothing round them. The one under `pointer` is drawn larger and
/// darker.
pub(super) fn paint_selection(painter: &egui::Painter, selection: &Selection, rect: Rect, g: &PageGeometry, pointer: Option<Pos2>) {
    let f = &selection.frame;
    let at = |x: f64, y: f64| {
        let p = f.to_user(x * f.width, y * f.height);
        let (fx, fy) = g.to_view(p.x as f32, p.y as f32);
        pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height())
    };
    let ring = vec![at(0.0, 0.0), at(1.0, 0.0), at(1.0, 1.0), at(0.0, 1.0), at(0.0, 0.0)];
    painter.extend(Shape::dashed_line(&ring, Stroke::new(1.0, ACCENT.gamma_multiply(0.7)), 4.0, 3.0));
    for (_, centre, out) in handles(selection, rect, g) {
        let hovered = pointer.is_some_and(|p| p.distance(centre) <= REACH);
        let (size, ink) = if hovered { (1.25, ACCENT_PRESSED) } else { (1.0, ACCENT) };
        if selection.turning {
            paint_turn_arrow(painter, centre, out, 6.0 * size, ink);
        } else {
            paint_stretch_arrow(painter, centre, out, 6.0 * size, ink);
        }
    }
    if selection.turning {
        // Where it turns about.
        let middle = at(0.5, 0.5);
        let stroke = Stroke::new(1.5, ACCENT);
        painter.line_segment([middle - vec2(5.0, 0.0), middle + vec2(5.0, 0.0)], stroke);
        painter.line_segment([middle - vec2(0.0, 5.0), middle + vec2(0.0, 5.0)], stroke);
    }
}

/// A filled arrowhead at `tip` pointing along `along`, `size` long.
fn head(tip: Pos2, along: Vec2, size: f32, ink: Color32) -> Shape {
    let side = vec2(-along.y, along.x) * size * 0.6;
    Shape::convex_polygon(vec![tip, tip - along * size + side, tip - along * size - side], ink, Stroke::NONE)
}

/// Two heads pointing apart along `out`, to stretch by.
fn paint_stretch_arrow(painter: &egui::Painter, centre: Pos2, out: Vec2, size: f32, ink: Color32) {
    let gap = size * 0.2;
    painter.add(head(centre + out * (size + gap), out, size * 0.8, ink));
    painter.add(head(centre - out * (size + gap), -out, size * 0.8, ink));
}

/// A curved stroke bowed out round a corner, a small head at either end, to
/// turn by.
fn paint_turn_arrow(painter: &egui::Painter, centre: Pos2, out: Vec2, size: f32, ink: Color32) {
    let base = out.y.atan2(out.x);
    let (middle, radius, half) = (centre - out * size * 0.6, size * 1.2, FRAC_PI_2 * 0.6);
    let point = |t: f32| middle + vec2((base + t).cos(), (base + t).sin()) * radius;
    let arc: Vec<Pos2> = (0..=12).map(|i| point(-half + 2.0 * half * i as f32 / 12.0)).collect();
    painter.add(Shape::line(arc, Stroke::new(1.5, ink)));
    for (t, sign) in [(half, 1.0f32), (-half, -1.0)] {
        let tangent = vec2(-(base + t).sin(), (base + t).cos()) * sign;
        painter.add(head(point(t) + tangent * size * 0.35, tangent, size * 0.6, ink));
    }
}

/// A curved arrow by the pointer while turning, since there's no pointer
/// shape for it.
pub(super) fn paint_turn_pointer(ctx: &egui::Context, pos: Pos2) {
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("turn-pointer")));
    paint_turn_arrow(&painter, pos + vec2(16.0, 16.0), vec2(0.7071, 0.7071), 5.0, ACCENT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turned_box_stays_a_box_and_other_drawings_turn_point_by_point() {
        let quarter = Affine::rotate_about(Pt::new(0.0, 0.0), std::f64::consts::FRAC_PI_2);
        let turned = drawing_reshaped(MarkupKind::Rectangle, &[[0.0, 0.0], [20.0, 10.0]], &quarter);
        assert_eq!(turned.len(), 4, "a turned rectangle keeps its four corners");
        let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        assert!(close(turned[0], [0.0, 0.0]) && close(turned[1], [0.0, 20.0]) && close(turned[2], [-10.0, 20.0]), "{turned:?}");
        let line = drawing_reshaped(MarkupKind::Line, &[[0.0, 0.0], [20.0, 0.0]], &quarter);
        assert!(close(line[1], [0.0, 20.0]));
        assert_eq!(stretch_cursor(vec2(1.0, 0.0)), CursorIcon::ResizeHorizontal);
        assert_eq!(stretch_cursor(vec2(0.7, 0.7)), CursorIcon::ResizeNwSe, "down and right on screen");
        assert_eq!(stretch_cursor(vec2(0.7, -0.7)), CursorIcon::ResizeNeSw);
    }
}
