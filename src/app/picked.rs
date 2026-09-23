//! What is picked out: the one thing `active` or `active_measure` holds, as
//! the page has always had it, and any others picked out with it in the
//! quantities table.
//!
//! The page picks out one thing at a time and knows nothing of the rest. They
//! are kept beside the one they were picked out with, and only for as long as
//! that is still the one picked out: when the page picks something else, or
//! nothing, they go with it, so nothing that sets `active` has to know about
//! them.
//!
//! The Select tool picks several out on the page itself, through the same
//! `pick`. A click picks out what is under it; Ctrl-click adds it, or takes it
//! back out. A drag across bare page draws a box: dragged rightwards it picks
//! out only what lies wholly inside, dragged leftwards everything it touches,
//! and with Ctrl held it adds to what was picked out before. Whatever is
//! picked out is deleted and moved together, as one step to undo.

use markup_model::markup::Geometry;
use markup_model::Hit;

use super::quantities::RowId;
use super::*;

#[derive(Default)]
pub(super) struct Picked {
    /// The one on the page the rest were picked out with.
    with: Option<RowId>,
    rest: Vec<RowId>,
    /// Where a Shift-click in the table picks out from: the row last clicked
    /// without Shift.
    anchor: Option<RowId>,
}

impl App {
    /// What the page has picked out, as rows of the table: a measurement, or
    /// a note or a drawing, or once in a while one of each.
    fn page_picked(&self) -> Vec<RowId> {
        // One field holds whichever of a note or a drawing is picked out, so
        // which row it is depends on which the uid belongs to.
        let written = self.active.and_then(|uid| {
            let session = &self.doc.as_ref()?.session;
            session.highlight(uid).map(|_| RowId::Note(uid)).or_else(|| session.markup(uid).map(|_| RowId::Drawing(uid)))
        });
        self.active_measure.map(RowId::Measure).into_iter().chain(written).collect()
    }

    /// Everything picked out, the one the page has first.
    pub(super) fn picked_rows(&self) -> Vec<RowId> {
        let mut rows = self.page_picked();
        if rows.first().is_some() && rows.first() == self.picked.with.as_ref() {
            for &id in &self.picked.rest {
                if !rows.contains(&id) && self.row_exists(id) {
                    rows.push(id);
                }
            }
        }
        rows
    }

    /// Lets the rest go once the page has picked out something else. Once a
    /// frame, before anything reads what is picked out, so the rest never
    /// come back with the one they were picked with after it was let go.
    pub(super) fn settle_picked(&mut self) {
        let first = self.page_picked().first().copied();
        if first != self.picked.with {
            self.picked.with = None;
            self.picked.rest.clear();
        }
    }

    fn row_exists(&self, id: RowId) -> bool {
        let Some(doc) = self.doc.as_ref() else { return false };
        match id {
            RowId::Measure(id) => doc.session.measures().get(id).is_some(),
            RowId::Note(uid) => doc.session.highlight(uid).is_some(),
            RowId::Drawing(uid) => doc.session.markup(uid).is_some(),
        }
    }

    /// Picks out these and nothing else, the first on the page, without
    /// moving the page to them.
    pub(super) fn pick(&mut self, ids: &[RowId]) {
        self.active = None;
        self.active_measure = None;
        self.active_vertex = None;
        match ids.first() {
            Some(RowId::Measure(id)) => self.active_measure = Some(*id),
            Some(RowId::Note(uid) | RowId::Drawing(uid)) => self.active = Some(*uid),
            None => {}
        }
        self.picked.with = ids.first().copied();
        self.picked.rest = ids.iter().skip(1).copied().collect();
    }

    /// A plain click on a row: that row alone, and where a Shift-click will
    /// pick out from.
    pub(super) fn pick_only(&mut self, id: RowId) {
        self.picked.with = None;
        self.picked.rest.clear();
        self.picked.anchor = Some(id);
    }

    /// Ctrl-click on a row: in with the rest if it wasn't, out if it was.
    pub(super) fn pick_toggle(&mut self, id: RowId) {
        let mut ids = self.picked_rows();
        match ids.iter().position(|&picked| picked == id) {
            Some(at) => {
                ids.remove(at);
            }
            None => ids.push(id),
        }
        self.pick(&ids);
        self.picked.anchor = Some(id);
    }

    /// Shift-click on a row: every row from the last one clicked to this one,
    /// in the order `rows` has them. With Ctrl as well, added to what is
    /// picked out already rather than in place of it.
    pub(super) fn pick_range(&mut self, rows: &[RowId], id: RowId, adding: bool) {
        let Some(to) = rows.iter().position(|&row| row == id) else { return };
        let from = self.picked.anchor.and_then(|anchor| rows.iter().position(|&row| row == anchor)).unwrap_or(to);
        let range = &rows[from.min(to)..=from.max(to)];
        // The row it runs from goes first, so the page keeps the one it had.
        let mut ids: Vec<RowId> = if adding { self.picked_rows() } else { Vec::new() };
        for &row in std::iter::once(&rows[from]).chain(range) {
            if !ids.contains(&row) {
                ids.push(row);
            }
        }
        let anchor = self.picked.anchor;
        self.pick(&ids);
        self.picked.anchor = anchor.or(Some(id));
    }
}

/* ------------------------------------------------------------------ *
 * Boxes
 * ------------------------------------------------------------------ */

/// What a box picks out, which is set by the way it was dragged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum BoxRule {
    /// Dragged rightwards: only what lies wholly inside the box.
    Inside,
    /// Dragged leftwards: everything the box touches.
    Touching,
}

impl BoxRule {
    /// The rule for a box dragged from `from` to `to` across the screen.
    pub fn of_drag(from: Pos2, to: Pos2) -> BoxRule {
        if to.x >= from.x {
            BoxRule::Inside
        } else {
            BoxRule::Touching
        }
    }
}

/// A piece of what is drawn, in PDF user space: a run of points, joined up
/// and closed round if `closed`. A single point is a mark on its own.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Outline {
    pub points: Vec<[f32; 2]>,
    pub closed: bool,
}

impl Outline {
    fn open(points: Vec<[f32; 2]>) -> Outline {
        Outline { points, closed: false }
    }

    fn closed(points: Vec<[f32; 2]>) -> Outline {
        Outline { points, closed: true }
    }

    fn rectangle(b: &PdfBox) -> Outline {
        Outline::closed(vec![[b.left, b.bottom], [b.right, b.bottom], [b.right, b.top], [b.left, b.top]])
    }

    fn edges(&self) -> impl Iterator<Item = ([f32; 2], [f32; 2])> + '_ {
        let n = self.points.len();
        let count = match n {
            0 | 1 => 0,
            _ if self.closed => n,
            _ => n - 1,
        };
        (0..count).map(move |i| (self.points[i], self.points[(i + 1) % n]))
    }

    fn inside(&self, b: &PdfBox) -> bool {
        self.points.iter().all(|p| b.contains(p[0], p[1]))
    }

    fn touches(&self, b: &PdfBox) -> bool {
        if self.points.iter().any(|p| b.contains(p[0], p[1])) {
            return true;
        }
        let corners = [[b.left, b.bottom], [b.right, b.bottom], [b.right, b.top], [b.left, b.top]];
        let sides = [(corners[0], corners[1]), (corners[1], corners[2]), (corners[2], corners[3]), (corners[3], corners[0])];
        if self.edges().any(|(p, q)| sides.iter().any(|&(a, c)| segments_cross(p, q, a, c))) {
            return true;
        }
        // A box drawn wholly inside a closed shape touches it too: it is on
        // the shape, however far from the outline.
        self.closed && self.points.len() > 2 && ring_holds(&self.points, corners[0])
    }
}

/// Whether what is drawn as `outlines` is caught by `b` under `rule`. Nothing
/// drawn at all is never caught.
pub(super) fn caught(outlines: &[Outline], b: &PdfBox, rule: BoxRule) -> bool {
    if outlines.iter().all(|o| o.points.is_empty()) {
        return false;
    }
    match rule {
        BoxRule::Inside => outlines.iter().all(|o| o.inside(b)),
        BoxRule::Touching => outlines.iter().any(|o| o.touches(b)),
    }
}

/// Whether segments `p`-`q` and `a`-`b` meet, ends included.
fn segments_cross(p: [f32; 2], q: [f32; 2], a: [f32; 2], b: [f32; 2]) -> bool {
    let side = |o: [f32; 2], s: [f32; 2], t: [f32; 2]| (s[0] - o[0]) * (t[1] - o[1]) - (s[1] - o[1]) * (t[0] - o[0]);
    let within = |o: [f32; 2], s: [f32; 2], t: [f32; 2]| {
        t[0] >= o[0].min(s[0]) && t[0] <= o[0].max(s[0]) && t[1] >= o[1].min(s[1]) && t[1] <= o[1].max(s[1])
    };
    let (d1, d2, d3, d4) = (side(a, b, p), side(a, b, q), side(p, q, a), side(p, q, b));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return true;
    }
    (d1 == 0.0 && within(a, b, p)) || (d2 == 0.0 && within(a, b, q)) || (d3 == 0.0 && within(p, q, a)) || (d4 == 0.0 && within(p, q, b))
}

/// Whether `point` lies inside the ring, by the even-odd rule the fill uses.
fn ring_holds(ring: &[[f32; 2]], point: [f32; 2]) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > point[1]) != (b[1] > point[1]) && point[0] < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn pt(p: markup_model::Pt) -> [f32; 2] {
    [p.x as f32, p.y as f32]
}

/// What a measurement draws, for a box to catch: its outline, or each of a
/// count's marks on its own, since a count is not a path through them.
pub(super) fn measurement_outlines(geometry: &Geometry) -> Vec<Outline> {
    match geometry {
        Geometry::Points { pts } => pts.iter().map(|&p| Outline::open(vec![pt(p)])).collect(),
        Geometry::Polygon { pts, .. } => vec![Outline::closed(pts.iter().map(|&p| pt(p)).collect())],
        Geometry::Ellipse { .. } => vec![Outline::closed(measure::outline_of(geometry).into_iter().map(pt).collect())],
        Geometry::Ink { strokes } => strokes.iter().map(|s| Outline::open(s.iter().map(|&p| pt(p)).collect())).collect(),
        Geometry::Line { .. } | Geometry::Polyline { .. } => vec![Outline::open(measure::outline_of(geometry).into_iter().map(pt).collect())],
    }
}

/// What a drawn markup draws. One read from the file has no points of its
/// own -- the page's drawing shows it -- so its box stands in for it.
pub(super) fn drawing_outlines(m: &Markup) -> Vec<Outline> {
    match (m.kind, &m.points[..]) {
        (_, []) => vec![Outline::rectangle(&m.bounds)],
        (MarkupKind::Rectangle, [a, b, ..]) => vec![Outline::rectangle(&PdfBox::spanning(*a, *b))],
        (MarkupKind::Ellipse, [a, b, ..]) => {
            let area = PdfBox::spanning(*a, *b);
            let (cx, cy) = area.center();
            let (rx, ry) = (area.width() / 2.0, area.height() / 2.0);
            let ring = (0..32)
                .map(|i| {
                    let angle = std::f32::consts::TAU * i as f32 / 32.0;
                    [cx + rx * angle.cos(), cy + ry * angle.sin()]
                })
                .collect();
            vec![Outline::closed(ring)]
        }
        (_, points) => vec![Outline::open(points.to_vec())],
    }
}

/// What a highlight covers: the band on each line.
pub(super) fn highlight_outlines(h: &Highlight) -> Vec<Outline> {
    h.quads.iter().map(Outline::rectangle).collect()
}

/// Everything on `page` that `b` catches under `rule`: measurements first,
/// then what is drawn, then highlights, each in the order the page holds
/// them.
pub(super) fn picks_in_box(doc: &Doc, page: usize, b: &PdfBox, rule: BoxRule) -> Vec<RowId> {
    let session = &doc.session;
    let measured = session
        .measures()
        .iter()
        .filter(|(m, _)| m.page as usize == page)
        .filter(|(m, _)| caught(&measurement_outlines(&m.geometry), b, rule))
        .map(|(m, _)| RowId::Measure(m.id));
    let drawn = session
        .markups()
        .iter()
        .filter(|e| e.markup.page == page && caught(&drawing_outlines(&e.markup), b, rule))
        .map(|e| RowId::Drawing(e.uid));
    let highlighted = session
        .highlights()
        .iter()
        .filter(|e| e.hl.page == page && caught(&highlight_outlines(&e.hl), b, rule))
        .map(|e| RowId::Note(e.uid));
    measured.chain(drawn).chain(highlighted).collect()
}

/* ------------------------------------------------------------------ *
 * The Select tool
 * ------------------------------------------------------------------ */

impl App {
    /// What a press at `pos` on sheet `sheet` lands on: a measurement first,
    /// since its corners and edges are what the Select tool takes hold of,
    /// then something drawn -- the smallest, so one inside another can be
    /// picked -- then a highlight, the one on top.
    pub(super) fn pick_at(&self, sheet: usize, pos: Pos2) -> Option<RowId> {
        if let Some((id, _)) = self.measurement_at(sheet, pos) {
            return Some(RowId::Measure(id));
        }
        let doc = self.doc.as_ref()?;
        let page = doc.sheet_page(sheet)?;
        let rect = *self.page_rects.get(&sheet)?;
        let (x, y) = self.pdf_point(sheet, pos)?;
        if let Some(uid) = markup_at(doc, page, rect, (x, y)) {
            return Some(RowId::Drawing(uid));
        }
        let highlights = doc.session.highlights().iter().rev();
        highlights.filter(|e| e.hl.page == page && e.hl.quads.iter().any(|q| q.contains(x, y))).map(|e| RowId::Note(e.uid)).next()
    }

    /// Where a point in the user space of the page sheet `sheet` shows is on
    /// screen, as the sheet is drawn now.
    pub(super) fn screen_point(&self, sheet: usize, (x, y): (f32, f32)) -> Option<Pos2> {
        let doc = self.doc.as_ref()?;
        let rect = self.page_rects.get(&sheet)?;
        let (fx, fy) = doc.sheet_geometry(sheet)?.to_view(x, y);
        Some(pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height()))
    }

    /// The Select tool's answer to the button going down. Without Ctrl it
    /// picks out what is under the press -- unless that is picked out
    /// already, in which case everything picked out stays so that a drag
    /// moves it all -- and a press on bare page lets go of everything. With
    /// Ctrl nothing changes yet: a click toggles, a drag adds a box.
    pub(super) fn press_to_pick(&mut self, sheet: usize, pos: Pos2, ctrl: bool) {
        self.active_vertex = None;
        if ctrl {
            return;
        }
        match self.pick_at(sheet, pos) {
            Some(id) if self.picked_rows().contains(&id) => {}
            Some(id) => self.pick(&[id]),
            None => self.pick(&[]),
        }
    }

    /// The Select tool's answer to a click. A plain click leaves just what
    /// is under it picked out, and opens the note of a highlight or of
    /// something drawn; Ctrl-click adds it or takes it back out.
    pub(super) fn click_to_pick(&mut self, sheet: usize, pos: Pos2, ctrl: bool) {
        let hit = self.pick_at(sheet, pos);
        if ctrl {
            if let Some(id) = hit {
                self.pick_toggle(id);
            }
            self.popup = None;
            return;
        }
        match hit {
            // Its note opens pinned under the line clicked, which the page
            // click works out.
            Some(id @ RowId::Note(_)) => {
                self.pick(&[id]);
                self.click_page(sheet, pos);
            }
            Some(id @ RowId::Drawing(uid)) => {
                self.pick(&[id]);
                self.open_markup_popup(uid);
            }
            Some(id) => {
                self.pick(&[id]);
                self.popup = None;
            }
            None => {
                self.pick(&[]);
                self.popup = None;
            }
        }
    }

    /// The Select tool's answer to a drag starting at `pos`. Without Ctrl, a
    /// drag from a measurement's corner moves the corner, from the middle of
    /// an edge adds one, and from anywhere else on a measurement or on
    /// something drawn moves everything picked out. Anywhere else -- bare
    /// page, a highlight, or anything at all with Ctrl held -- draws a box.
    pub(super) fn start_select_drag(&mut self, sheet: usize, pos: Pos2, ctrl: bool) {
        if !ctrl {
            if let Some((id, hit)) = self.measurement_at(sheet, pos) {
                let circle = self.doc.as_ref().and_then(|d| d.session.measures().get(id)).is_some_and(|m| matches!(m.geometry, Geometry::Ellipse { .. }));
                match hit {
                    Hit::Edge { .. } | Hit::Inside if !circle || matches!(hit, Hit::Inside) => {
                        if !self.picked_rows().contains(&RowId::Measure(id)) {
                            self.pick(&[RowId::Measure(id)]);
                        }
                        self.start_moving(sheet, pos);
                    }
                    _ => self.grab_measurement(sheet, pos, id, hit),
                }
                self.popup = None;
                return;
            }
            if let Some(id @ RowId::Drawing(_)) = self.pick_at(sheet, pos) {
                if !self.picked_rows().contains(&id) {
                    self.pick(&[id]);
                }
                self.start_moving(sheet, pos);
                self.popup = None;
                return;
            }
        }
        if let Some(point) = self.pdf_point(sheet, pos) {
            self.drag = Some(Drag::Pick { sheet, start: point, end: point, adding: ctrl });
            self.popup = None;
        }
    }

    /// Starts moving everything picked out, from where the pointer is.
    fn start_moving(&mut self, sheet: usize, pos: Pos2) {
        let Some(from) = self.pdf_point(sheet, pos) else { return };
        let session = self.doc.as_ref().map(|d| &d.session);
        let movable = self.picked_rows().into_iter().any(|id| match id {
            RowId::Measure(_) => true,
            RowId::Drawing(uid) => session.and_then(|s| s.markup(uid)).is_some_and(|e| e.markup.key.is_none()),
            RowId::Note(_) => false,
        });
        if !movable {
            self.toast("Already saved into the file, so it stays where it is.".to_owned());
            return;
        }
        self.drag = Some(Drag::MovePicked { sheet, from });
    }

    /// Moves everything picked out on the page sheet `sheet` shows as the
    /// pointer moves, from where it was last. What is on another page stays
    /// where it is: a distance on one sheet means nothing on the next. So do
    /// highlights, which belong to the text under them, and anything drawn
    /// that the file already holds.
    pub(super) fn move_picked(&mut self, sheet: usize, from: (f32, f32), pos: Pos2) {
        let Some(point) = self.pdf_point(sheet, pos) else { return };
        let by = [point.0 - from.0, point.1 - from.1];
        if by == [0.0, 0.0] {
            return;
        }
        if let Some(Drag::MovePicked { from, .. }) = self.drag.as_mut() {
            *from = point;
        }
        let picked = self.picked_rows();
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(page) = doc.sheet_page(sheet) else { return };
        let delta = markup_model::Pt::new(f64::from(by[0]), f64::from(by[1]));
        let mut commands = Vec::new();
        for id in picked {
            match id {
                RowId::Measure(id) => {
                    if let Some(markup) = doc.session.measures().get(id).filter(|m| m.page as usize == page) {
                        let mut moved = markup.clone();
                        moved.geometry = moved.geometry.moved_by(delta);
                        commands.push(Command::ChangeMeasure(Box::new(moved)));
                    }
                }
                RowId::Drawing(uid) => {
                    if doc.session.markup(uid).is_some_and(|e| e.markup.page == page) {
                        commands.push(Command::MoveMarkup { uid, by });
                    }
                }
                RowId::Note(_) => {}
            }
        }
        // Kept as one change while the drag lasts, so undo takes the whole
        // move back rather than a frame of it.
        doc.session.apply_merged(Command::Batch(commands));
    }

    /// Letting go of a box: what it caught is picked out, in place of what
    /// was, or as well with Ctrl.
    pub(super) fn finish_box(&mut self, sheet: usize, start: (f32, f32), end: (f32, f32), adding: bool) {
        let (Some(from), Some(to)) = (self.screen_point(sheet, start), self.screen_point(sheet, end)) else { return };
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(page) = doc.sheet_page(sheet) else { return };
        let caught = picks_in_box(doc, page, &box_between(start, end), BoxRule::of_drag(from, to));
        let mut ids = if adding { self.picked_rows() } else { Vec::new() };
        ids.extend(caught.into_iter().filter(|id| !ids.contains(id)).collect::<Vec<_>>());
        self.pick(&ids);
    }

    /// Picks out everything on the page in view: Ctrl+A, and the palette.
    pub(super) fn pick_everything_on_page(&mut self) {
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(page) = doc.sheet_page(self.current_page) else { return };
        // Far past any page's edge, so even something drawn hanging off the
        // sheet is caught.
        let everything = PdfBox { left: -1e7, bottom: -1e7, right: 1e7, top: 1e7 };
        let caught = picks_in_box(doc, page, &everything, BoxRule::Touching);
        self.pick(&caught);
    }

    /// Delete: the corner picked out, if one is and its shape can spare it,
    /// otherwise everything picked out, as one step to undo.
    pub(super) fn delete_picked(&mut self) {
        let picked = self.picked_rows();
        if let ([RowId::Measure(id)], Some((ring, index))) = (&picked[..], self.active_vertex) {
            let Some(doc) = self.doc.as_mut() else { return };
            if let Some(mut changed) = doc.session.measures().get(*id).cloned() {
                if changed.geometry.remove_vertex(ring, index) {
                    doc.session.apply(Command::ChangeMeasure(Box::new(changed)));
                    self.active_vertex = None;
                    return;
                }
            }
        }
        let commands: Vec<Command> = picked
            .iter()
            .map(|&id| match id {
                RowId::Measure(id) => Command::RemoveMeasure(id),
                RowId::Drawing(uid) | RowId::Note(uid) => Command::Remove(uid),
            })
            .collect();
        if commands.is_empty() {
            return;
        }
        if let Some(doc) = self.doc.as_mut() {
            doc.session.apply(Command::Batch(commands));
        }
        self.pick(&[]);
        self.popup = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(left: f32, bottom: f32, side: f32) -> PdfBox {
        PdfBox { left, bottom, right: left + side, top: bottom + side }
    }

    #[test]
    fn a_box_dragged_right_takes_what_is_inside_and_left_what_it_touches() {
        assert_eq!(BoxRule::of_drag(pos2(10.0, 50.0), pos2(90.0, 5.0)), BoxRule::Inside);
        assert_eq!(BoxRule::of_drag(pos2(90.0, 5.0), pos2(10.0, 50.0)), BoxRule::Touching);
    }

    #[test]
    fn a_line_is_inside_only_with_both_ends_in_and_touched_by_any_part() {
        let line = [Outline::open(vec![[0.0, 0.0], [100.0, 0.0]])];
        assert!(caught(&line, &square(-10.0, -10.0, 120.0), BoxRule::Inside));
        assert!(!caught(&line, &square(-10.0, -10.0, 50.0), BoxRule::Inside), "one end sticks out");
        assert!(caught(&line, &square(-10.0, -10.0, 50.0), BoxRule::Touching));
        // Across the middle, with neither end in the box.
        assert!(caught(&line, &square(40.0, -5.0, 10.0), BoxRule::Touching), "crossed between its ends");
        assert!(!caught(&line, &square(40.0, 5.0, 10.0), BoxRule::Touching), "near it isn't touching it");
    }

    /// A long diagonal's own box is huge, but a box beside the line that
    /// doesn't reach it doesn't catch it.
    #[test]
    fn touching_goes_by_the_line_not_by_its_box() {
        let diagonal = [Outline::open(vec![[0.0, 0.0], [100.0, 100.0]])];
        assert!(!caught(&diagonal, &square(70.0, 10.0, 10.0), BoxRule::Touching));
        assert!(caught(&diagonal, &square(45.0, 45.0, 10.0), BoxRule::Touching));
    }

    #[test]
    fn a_box_inside_an_area_touches_it() {
        let area = [Outline::closed(vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]])];
        assert!(caught(&area, &square(40.0, 40.0, 10.0), BoxRule::Touching), "in the middle of it, clear of its outline");
        assert!(!caught(&area, &square(40.0, 40.0, 10.0), BoxRule::Inside));
        // An open run shaped the same has nothing in the middle to touch.
        let run = [Outline::open(vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]])];
        assert!(!caught(&run, &square(40.0, 40.0, 10.0), BoxRule::Touching));
    }

    #[test]
    fn a_count_is_touched_by_any_of_its_marks_and_inside_with_all_of_them() {
        let geometry = Geometry::Points { pts: vec![markup_model::Pt::new(10.0, 10.0), markup_model::Pt::new(90.0, 90.0)] };
        let marks = measurement_outlines(&geometry);
        assert_eq!(marks.len(), 2, "each mark on its own, not a path through them");
        assert!(caught(&marks, &square(0.0, 0.0, 20.0), BoxRule::Touching));
        assert!(!caught(&marks, &square(40.0, 40.0, 20.0), BoxRule::Touching), "between the marks is not on them");
        assert!(!caught(&marks, &square(0.0, 0.0, 20.0), BoxRule::Inside));
        assert!(caught(&marks, &square(0.0, 0.0, 100.0), BoxRule::Inside));
    }

    #[test]
    fn nothing_drawn_is_never_caught() {
        assert!(!caught(&[], &square(0.0, 0.0, 1000.0), BoxRule::Inside));
        assert!(!caught(&[Outline::open(Vec::new())], &square(0.0, 0.0, 1000.0), BoxRule::Touching));
    }

    #[test]
    fn a_markup_read_from_the_file_is_caught_by_its_box() {
        let mut m = Markup {
            key: None,
            page: 0,
            kind: MarkupKind::Pen,
            points: Vec::new(),
            bounds: square(10.0, 10.0, 20.0),
            color: [1.0, 0.0, 0.0],
            width: 1.0,
            style: Default::default(),
            name: String::new(),
            comment: String::new(),
            author: String::new(),
        };
        assert!(caught(&drawing_outlines(&m), &square(0.0, 0.0, 50.0), BoxRule::Inside));
        // Drawn here, a pen stroke is caught by the stroke itself.
        m.points = vec![[10.0, 10.0], [30.0, 30.0]];
        assert!(!caught(&drawing_outlines(&m), &square(22.0, 10.0, 6.0), BoxRule::Touching));
        assert!(caught(&drawing_outlines(&m), &square(18.0, 18.0, 4.0), BoxRule::Touching));
    }

    /// The first page laid out one screen point to a PDF point, its top-left
    /// corner at the origin, so a point on screen is the same point on the
    /// page upside down. On it: a length across the top, a box drawn beside
    /// it, and a highlight further down.
    struct Page {
        app: App,
        length: RowId,
        drawn: RowId,
        highlight: RowId,
    }

    /// Where a point on the page is on screen.
    fn at(x: f32, y: f32) -> Pos2 {
        pos2(x, 800.0 - y)
    }

    fn page() -> Page {
        let (mut app, _) = super::super::tests::app_with_a_document();
        let doc = app.doc.as_mut().unwrap();
        doc.geometry[0] = Some(PageGeometry { rotation: 0, bounds: PdfBox { left: 0.0, bottom: 0.0, right: 600.0, top: 800.0 } });
        app.page_rects.insert(0, Rect::from_min_size(Pos2::ZERO, vec2(600.0, 800.0)));

        let line = Geometry::Line { a: markup_model::Pt::new(100.0, 700.0), b: markup_model::Pt::new(200.0, 700.0) };
        let length = crate::model::MeasureMarkup::new(0, markup_model::MarkupKind::Length, line);
        let id = length.id;
        doc.session.apply(Command::AddMeasure(Box::new(length)));
        let drawn = Markup {
            key: None,
            page: 0,
            kind: MarkupKind::Rectangle,
            points: vec![[300.0, 700.0], [350.0, 650.0]],
            bounds: PdfBox { left: 300.0, bottom: 650.0, right: 350.0, top: 700.0 },
            color: [1.0, 0.0, 0.0],
            width: 1.0,
            style: Default::default(),
            name: "Box".to_owned(),
            comment: String::new(),
            author: String::new(),
        };
        let drawn = doc.session.apply(Command::AddMarkup(drawn))[0];
        let highlight = Highlight {
            key: None,
            page: 0,
            quads: vec![PdfBox { left: 100.0, bottom: 500.0, right: 200.0, top: 512.0 }],
            color: [1.0, 0.9, 0.2],
            comment: String::new(),
            author: String::new(),
            snippet: String::new(),
        };
        let highlight = doc.session.apply(Command::AddHighlights(vec![highlight]))[0];
        Page { app, length: RowId::Measure(id), drawn: RowId::Drawing(drawn), highlight: RowId::Note(highlight) }
    }

    #[test]
    fn a_click_picks_one_out_and_ctrl_click_adds_it_or_takes_it_back() {
        let Page { mut app, length, drawn, .. } = page();
        app.press_to_pick(0, at(150.0, 700.0), false);
        app.click_to_pick(0, at(150.0, 700.0), false);
        assert_eq!(app.picked_rows(), [length]);

        app.click_to_pick(0, at(325.0, 675.0), true);
        assert_eq!(app.picked_rows(), [length, drawn], "Ctrl-click adds");
        app.click_to_pick(0, at(325.0, 675.0), true);
        assert_eq!(app.picked_rows(), [length], "and Ctrl-click again takes it back out");

        // A plain click on one of several leaves just that one.
        app.pick(&[length, drawn]);
        app.press_to_pick(0, at(150.0, 700.0), false);
        assert_eq!(app.picked_rows().len(), 2, "the press keeps the rest, in case it becomes a drag");
        app.click_to_pick(0, at(150.0, 700.0), false);
        assert_eq!(app.picked_rows(), [length]);

        app.press_to_pick(0, at(500.0, 100.0), false);
        app.click_to_pick(0, at(500.0, 100.0), false);
        assert!(app.picked_rows().is_empty(), "a click on bare page lets go of everything");
    }

    #[test]
    fn a_box_takes_what_is_inside_one_way_and_what_it_touches_the_other() {
        let Page { mut app, length, drawn, highlight } = page();
        // Round the length and the drawn box, not the highlight below them.
        app.finish_box(0, (50.0, 750.0), (400.0, 600.0), false);
        assert_eq!(app.picked_rows(), [length, drawn]);

        // A box over the right-hand end of the length and the corner of the
        // drawn box holds neither wholly...
        app.finish_box(0, (150.0, 750.0), (320.0, 690.0), false);
        assert!(app.picked_rows().is_empty(), "dragged rightwards it takes only what is wholly inside");
        // ...but touches both.
        app.finish_box(0, (320.0, 690.0), (150.0, 750.0), false);
        assert_eq!(app.picked_rows().len(), 2, "dragged leftwards it takes whatever it touches");

        // With Ctrl, a box adds to what is picked out.
        app.finish_box(0, (50.0, 520.0), (250.0, 490.0), true);
        assert_eq!(app.picked_rows(), [length, drawn, highlight]);
        // Without, it replaces it.
        app.finish_box(0, (50.0, 520.0), (250.0, 490.0), false);
        assert_eq!(app.picked_rows(), [highlight]);
    }

    #[test]
    fn a_drag_from_bare_page_draws_a_box_and_from_a_corner_moves_the_corner() {
        let Page { mut app, length, drawn, .. } = page();
        app.start_select_drag(0, at(500.0, 100.0), false);
        assert!(matches!(app.drag, Some(Drag::Pick { adding: false, .. })));
        app.drag = None;
        app.start_select_drag(0, at(150.0, 700.0), true);
        assert!(matches!(app.drag, Some(Drag::Pick { adding: true, .. })), "with Ctrl, even over something, it is a box that adds");
        app.drag = None;

        // A corner is still taken hold of by the Select tool.
        app.pick(&[length, drawn]);
        app.start_select_drag(0, at(100.0, 700.0), false);
        assert!(matches!(app.drag, Some(Drag::MeasureVertex { id, .. }) if RowId::Measure(id) == length));
        assert_eq!(app.picked_rows(), [length], "a corner is one measurement's alone");
        app.drag = None;

        // The body of one of several moves them all.
        app.pick(&[length, drawn]);
        app.start_select_drag(0, at(130.0, 700.0), false);
        assert!(matches!(app.drag, Some(Drag::MovePicked { .. })));
        assert_eq!(app.picked_rows().len(), 2);
    }

    #[test]
    fn everything_picked_out_moves_together_as_one_step() {
        let Page { mut app, length, drawn, highlight } = page();
        let (RowId::Measure(id), RowId::Drawing(uid), RowId::Note(note)) = (length, drawn, highlight) else { unreachable!() };
        app.pick(&[length, drawn, highlight]);
        app.drag = Some(Drag::MovePicked { sheet: 0, from: (150.0, 700.0) });
        app.move_picked(0, (150.0, 700.0), at(160.0, 710.0));
        app.move_picked(0, (160.0, 710.0), at(170.0, 720.0));
        app.doc.as_mut().unwrap().session.end_merge();

        let session = &app.doc.as_ref().unwrap().session;
        let bounds = session.measures().get(id).unwrap().geometry.bounds().unwrap();
        assert!((bounds.min.x - 120.0).abs() < 1e-3 && (bounds.min.y - 720.0).abs() < 1e-3, "{bounds:?}");
        let corner = session.markup(uid).unwrap().markup.points[0];
        assert!((corner[0] - 320.0).abs() < 1e-3 && (corner[1] - 720.0).abs() < 1e-3, "{corner:?}");
        assert_eq!(session.highlight(note).unwrap().hl.quads[0].left, 100.0, "a highlight stays with its text");

        app.undo(false);
        let session = &app.doc.as_ref().unwrap().session;
        assert_eq!(session.measures().get(id).unwrap().geometry.bounds().unwrap().min.x, 100.0, "one undo takes the whole move back");
        assert_eq!(session.markup(uid).unwrap().markup.points[0], [300.0, 700.0]);
    }

    #[test]
    fn everything_picked_out_is_deleted_together_and_comes_back_together() {
        let Page { mut app, length, drawn, highlight } = page();
        app.pick(&[length, drawn, highlight]);
        app.delete_picked();
        assert!(app.picked_rows().is_empty());
        let there = |app: &App| [length, drawn, highlight].map(|id| app.row_exists(id));
        assert_eq!(there(&app), [false; 3]);
        app.undo(false);
        assert_eq!(there(&app), [true; 3], "one undo brings all three back");
    }

    #[test]
    fn everything_on_the_page_can_be_picked_out_at_once() {
        let Page { mut app, .. } = page();
        app.pick_everything_on_page();
        assert_eq!(app.picked_rows().len(), 3);
    }
}
