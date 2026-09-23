//! The highlighter: selecting text by dragging, and whole boxes of it with
//! Ctrl held.

use super::*;

/// The selected character range on each sheet a drag covers, first sheet
/// first. A drag runs down the column, so it is measured in sheets; the text
/// it picks out belongs to the page each sheet shows.
pub(super) fn drag_segments(doc: &Doc, drag: &Drag) -> Vec<(usize, Range<usize>)> {
    match *drag {
        Drag::Text { anchor, focus } => {
            let (a, b) = if anchor <= focus { (anchor, focus) } else { (focus, anchor) };
            (a.0..=b.0)
                .filter_map(|sheet| {
                    let chars = doc.text.get(&doc.sheet_page(sheet)?)?;
                    let start = if sheet == a.0 { a.1 } else { 0 };
                    let end = if sheet == b.0 { b.1 } else { chars.len() };
                    (start < end).then_some((sheet, start..end))
                })
                .collect()
        }
        Drag::Box { sheet, start, end } => match doc.sheet_page(sheet).and_then(|page| doc.text.get(&page)) {
            Some(chars) => selection::in_box(chars, &box_between(start, end)).into_iter().map(|range| (sheet, range)).collect(),
            None => Vec::new(),
        },
        Drag::Markup { .. } | Drag::Calibrate { .. } | Drag::MeasureVertex { .. } | Drag::Pick { .. } | Drag::MovePicked { .. } => Vec::new(),
    }
}

/// The box with these two corners, in PDF user space.
pub(super) fn box_between(a: (f32, f32), b: (f32, f32)) -> PdfBox {
    PdfBox::spanning([a.0, a.1], [b.0, b.1])
}

impl App {
    /// Where in the text of the page sheet `sheet` shows the point `pos`
    /// falls. Taken by sheet, not by page: a point on screen is a point on a
    /// place in the column, and the same page shown twice is two places.
    pub(super) fn caret_for(&self, sheet: usize, pos: Pos2) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        let rect = self.page_rects.get(&sheet)?;
        let geometry = doc.sheet_geometry(sheet)?;
        let chars = doc.text.get(&doc.sheet_page(sheet)?)?;
        let (x, y) = to_pdf(*rect, &geometry, pos);
        selection::caret_at(chars, x, y)
    }

    /// A point on screen as a point in the user space of the page sheet
    /// `sheet` shows -- through the sheet's own geometry, so a turned sheet
    /// gives the point the user is actually pointing at.
    pub(super) fn pdf_point(&self, sheet: usize, pos: Pos2) -> Option<(f32, f32)> {
        let doc = self.doc.as_ref()?;
        let rect = self.page_rects.get(&sheet)?;
        let geometry = doc.sheet_geometry(sheet)?;
        Some(to_pdf(*rect, &geometry, pos))
    }

    pub(super) fn update_drag(&mut self, ui: &Ui) {
        let (pos, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));

        if let Some(pos) = pos {
            if let Some(Drag::Markup { sheet, .. }) = &self.drag {
                // A markup stays on the sheet it started on.
                let sheet = *sheet;
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                let spacing = PEN_SPACING * self.points_per_screen(sheet);
                if let (Some((x, y)), Some(Drag::Markup { markup, .. })) = (self.pdf_point(sheet, pos), self.drag.as_mut()) {
                    follow(markup, [x, y], spacing);
                }
            } else if let Some(Drag::MeasureVertex { id, ring, index, sheet }) = self.drag {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                self.drag_measure_vertex(sheet, id, ring, index, pos);
            } else if let Some(Drag::MovePicked { sheet, from }) = self.drag {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                self.move_picked(sheet, from, pos);
            } else if let Some(Drag::Pick { sheet, .. }) = self.drag {
                // A box stays on the page it started on.
                if let (Some(point), Some(Drag::Pick { end, .. })) = (self.pdf_point(sheet, pos), self.drag.as_mut()) {
                    *end = point;
                }
            } else if let Some(Drag::Calibrate { sheet, from, .. }) = self.drag {
                // A calibration line stays on its page, snapping to what is
                // drawn there -- or, with Shift, held straight instead.
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                if let Some(point) = self.pdf_point(sheet, pos) {
                    let (_, at) = self.snapped(sheet, point, Some(from));
                    if let Some(Drag::Calibrate { to, .. }) = self.drag.as_mut() {
                        *to = at;
                    }
                }
            } else if let Some(Drag::Box { sheet, .. }) = self.drag {
                // A box stays on the page it started on.
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                if let (Some(point), Some(Drag::Box { end, .. })) = (self.pdf_point(sheet, pos), self.drag.as_mut()) {
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
                let nearest = self.page_rects.iter().min_by(|a, b| distance(a.1).total_cmp(&distance(b.1))).map(|(s, _)| *s);
                if let Some(sheet) = nearest {
                    if let (Some(caret), Some(Drag::Text { focus, .. })) = (self.caret_for(sheet, pos), self.drag.as_mut()) {
                        *focus = (sheet, caret);
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

        let drag = match self.drag.take() {
            // Let go after dragging: the line is drawn. Let go without having
            // moved anywhere: that press placed the first end, and the line
            // follows the pointer to the next click.
            Some(Drag::Calibrate { sheet, from, to, placed }) => {
                // Waiting for the second click: the line stays live and
                // follows the pointer instead of ending here.
                if placed || !self.finish_calibration(sheet, from, to) {
                    self.drag = Some(Drag::Calibrate { sheet, from, to, placed: true });
                }
                return;
            }
            // The move was applied as it went; letting go ends the one step.
            Some(Drag::MeasureVertex { .. } | Drag::MovePicked { .. }) => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.session.end_merge();
                }
                return;
            }
            Some(Drag::Pick { sheet, start, end, adding }) => return self.finish_box(sheet, start, end, adding),
            Some(Drag::Markup { markup, .. }) => return self.add_markup(markup),
            Some(drag) => drag,
            None => return,
        };
        // Released: copy what was selected, and offer to highlight it.
        let Some(doc) = &self.doc else { return };
        let segments = drag_segments(doc, &drag);

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

        let highlight = self.tools.settings(tools::ToolKey::Highlight);
        self.active = None;
        self.popup = Some(Popup {
            mode: PopupMode::Create(pending),
            anchor,
            color: highlight.style.stroke,
            note: highlight.defaults.description,
            just_opened: true,
            height: 250.0,
        });
    }

    /// A plain click on an existing highlight or markup opens its note.
    pub(super) fn click_page(&mut self, sheet: usize, pos: Pos2) {
        // A measurement was picked out when the button went down.
        if self.measurement_at(sheet, pos).is_some() {
            self.popup = None;
            return;
        }
        let Some(doc) = &self.doc else { return };
        let Some(page) = doc.sheet_page(sheet) else { return };
        let Some(rect) = self.page_rects.get(&sheet) else { return };
        let Some(geometry) = doc.sheet_geometry(sheet) else { return };
        let (x, y) = to_pdf(*rect, &geometry, pos);
        let hit = doc
            .session
            .highlights()
            .iter()
            .rev()
            .find(|e| e.hl.page == page && e.hl.quads.iter().any(|q| q.contains(x, y)))
            .map(|e| e.uid);
        match hit {
            Some(uid) => {
                // Pin it under the bottom of the clicked highlight, not at the
                // pointer, so it never covers the text it's about.
                let bottom = doc
                    .session
                    .highlight(uid)
                    .and_then(|e| e.hl.quads.iter().find(|q| q.contains(x, y)))
                    .map_or(y, |q| q.bottom);
                self.open_edit_popup(uid, Anchor { page, x, y: bottom });
            }
            None => match markup_at(doc, page, *rect, (x, y)) {
                Some(uid) => self.open_markup_popup(uid),
                None => {
                    self.popup = None;
                    self.active = None;
                }
            },
        }
    }
}

