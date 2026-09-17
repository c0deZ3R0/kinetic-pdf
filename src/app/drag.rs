//! Selecting text by dragging, and whole boxes of it with Ctrl held.

use super::*;

/// The selected character range on each page a drag covers, first page first.
pub(super) fn drag_segments(text: &HashMap<usize, Vec<TextChar>>, drag: &Drag) -> Vec<(usize, Range<usize>)> {
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
        Drag::Markup(_) | Drag::Calibrate { .. } => Vec::new(),
    }
}

/// The box with these two corners, in PDF user space.
pub(super) fn box_between(a: (f32, f32), b: (f32, f32)) -> PdfBox {
    PdfBox::spanning([a.0, a.1], [b.0, b.1])
}

impl App {
    pub(super) fn caret_for(&self, page: usize, pos: Pos2) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        let rect = self.page_rects.get(&page)?;
        let geometry = doc.geometry.get(page).copied().flatten()?;
        let chars = doc.text.get(&page)?;
        let (x, y) = to_pdf(*rect, &geometry, pos);
        selection::caret_at(chars, x, y)
    }

    /// A point on screen as a point on `page`, in PDF user space.
    pub(super) fn pdf_point(&self, page: usize, pos: Pos2) -> Option<(f32, f32)> {
        let doc = self.doc.as_ref()?;
        let rect = self.page_rects.get(&page)?;
        let geometry = doc.geometry.get(page).copied().flatten()?;
        Some(to_pdf(*rect, &geometry, pos))
    }

    pub(super) fn update_drag(&mut self, ui: &Ui) {
        let (pos, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));

        if let Some(pos) = pos {
            if let Some(Drag::Markup(markup)) = &self.drag {
                // A markup stays on the page it started on.
                let page = markup.page;
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                let spacing = PEN_SPACING * self.points_per_screen(page);
                if let (Some((x, y)), Some(Drag::Markup(markup))) = (self.pdf_point(page, pos), self.drag.as_mut()) {
                    follow(markup, [x, y], spacing);
                }
            } else if let Some(Drag::Calibrate { page, from, .. }) = self.drag {
                // A calibration line stays on its page, snapping to what is
                // drawn there -- or, with Shift, held straight instead.
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                if let Some(point) = self.pdf_point(page, pos) {
                    let (_, at) = self.snapped(page, point, Some(from));
                    if let Some(Drag::Calibrate { to, .. }) = self.drag.as_mut() {
                        *to = at;
                    }
                }
            } else if let Some(Drag::Box { page, .. }) = self.drag {
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

        let drag = match self.drag.take() {
            Some(Drag::Calibrate { page, from, to }) => return self.finish_calibration(page, from, to),
            Some(Drag::Markup(markup)) => return self.add_markup(markup),
            Some(drag) => drag,
            None => return,
        };
        // Released: copy what was selected, and offer to highlight it.
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

    /// A plain click on an existing highlight or markup opens its note.
    pub(super) fn click_page(&mut self, page: usize, pos: Pos2) {
        let Some(doc) = &self.doc else { return };
        let Some(rect) = self.page_rects.get(&page) else { return };
        let Some(geometry) = doc.geometry[page] else { return };
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

