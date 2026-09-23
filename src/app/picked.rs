//! What is picked out: the one thing `active` or `active_measure` holds, as
//! the page has always had it, and any others picked out with it in the
//! quantities table.
//!
//! The page picks out one thing at a time and knows nothing of the rest. They
//! are kept beside the one they were picked out with, and only for as long as
//! that is still the one picked out: when the page picks something else, or
//! nothing, they go with it, so nothing that sets `active` has to know about
//! them. Picking several out on the page itself is for the select tool to add.

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
