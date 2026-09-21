//! Picking sheets out and putting them in another order.
//!
//! Pulled back far enough that a sheet can't be read -- 20% and further out,
//! where a drawing set is a wall of thumbnails -- the pages stop being
//! something to read and become something to sort. So at that zoom a click
//! picks a sheet out rather than selecting text: one on its own, Ctrl for
//! another, Shift for a run; Delete takes them out; Ctrl+X, C, V and D cut,
//! copy, paste and duplicate; the right-click menu turns them a quarter-turn
//! either way; and dragging them drops them somewhere else, with a caret
//! showing the gap they will land in. Zoomed back in, every one of those keys
//! means what it always did.
//!
//! None of it touches the file until it is applied. An arrangement is a list
//! of which page goes where (see `crate::arrange`), so taking out twenty
//! sheets of an 85 MB drawing set costs twenty `usize`s and nothing is
//! rewritten, redrawn or re-read. Applying it writes a new file and opens it,
//! which is also why the old one is left alone: the highlights and markups on
//! a page are held against its place in the file, and moving pages under them
//! would move every annotation after them.

use crate::arrange::Sheet;

use super::*;

/// The zoom at and below which sheets are picked rather than read. A sheet at
/// 20% is about the size of a playing card: too small to read, big enough to
/// tell apart, which is exactly when sorting them is what you want.
pub(super) const SHEET_ZOOM: f32 = 0.2;

/// How far a press has to move before it counts as dragging sheets rather
/// than clicking one, in screen points.
const DRAG_SLACK: f32 = 4.0;

/// One of the things the sheet view does to what is picked out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SheetAction {
    Cut,
    Copy,
    /// Into the gap the menu was opened on, or after what is picked out when
    /// it comes from the keyboard.
    Paste(Option<usize>),
    Duplicate,
    /// A quarter-turn of what is picked out: 1 clockwise, -1 the other way.
    Rotate(i8),
    /// A blank sheet in the gap after the sheet the menu was opened on.
    InsertBlank(usize),
    Delete,
    SelectAll,
}

/// Sheets being dragged into a new place.
pub(super) struct SheetDrag {
    /// Where the press started, so a click can be told from a drag.
    from: Pos2,
    /// Whether it has moved far enough to be a drag at all.
    moving: bool,
    /// The gap the sheets would land in, counted before any of them move --
    /// 0 before the first sheet, `len` after the last.
    caret: usize,
}

impl App {
    /// Whether the view is far enough out that sheets are picked rather than
    /// read.
    pub(super) fn sheet_mode(&self) -> bool {
        self.doc.is_some() && self.zoom <= SHEET_ZOOM + 1e-4
    }

    /// Lays out and draws the sheets in view, and answers clicks, drags and
    /// right-clicks on them. It stands in for `draw_pages` at this zoom: each
    /// sheet is its page's thumbnail, which is all the screen can show of it
    /// anyway, so nothing is rendered, no squares are kept and a whole set
    /// costs one image apiece.
    pub(super) fn draw_sheets(&mut self, ui: &mut Ui, viewport: Rect, layout: &PageLayout) {
        let tops = &layout.tops;
        if tops.is_empty() {
            return;
        }
        let ctx = ui.ctx().clone();
        let now = Self::now(&ctx);
        let origin = ui.max_rect().min;
        self.content_origin = origin;
        let content_w = content_width(ui.max_rect().width(), layout.widest);

        let n = tops.len();
        let first = tops.partition_point(|t| *t <= viewport.min.y).saturating_sub(1);
        let mut last = first;
        while last + 1 < n && tops[last + 1] < viewport.max.y {
            last += 1;
        }

        // Filled in as the sheets are drawn, below. `page_rects` is keyed by
        // sheet -- where each one is on screen -- which is exactly what this
        // view has, so it is reported rather than thrown away: without it, a
        // Ctrl+wheel zoom out of the sheet view has no spot to hold still and
        // the column jumps instead of zooming under the pointer.
        self.page_rects.clear();
        self.blank_pages.clear();
        self.view_stood_in = false;

        let sizes = self.doc.as_ref().map_or_else(Vec::new, sheet_sizes);
        let (modifiers, pointer) = ctx.input(|i| (i.modifiers, i.pointer.hover_pos()));
        let mut clicked: Option<usize> = None;
        let mut pressed: Option<(usize, Pos2)> = None;
        // Read from the pointer rather than from the sheet the drag started
        // on: dragged far enough, that sheet is no longer in view to report it.
        let released = ctx.input(|i| i.pointer.primary_released());
        let mut chose: Option<SheetAction> = None;
        let mut cleared = false;

        let Some(doc) = self.doc.as_mut() else { return };
        // The sheet at the top of the view is the one the toolbar counts as
        // current, so the page box still says where you are. It is the sheet's
        // place in the column, which is what the box counts.
        self.current_page = first;

        // The pages the sheets in view show, from the middle out, so the
        // reader gives the middle of the view its thumbnails first. A sheet
        // shown twice asks for its page once.
        let middle = viewport.center().y;
        let mut order: Vec<usize> = (first..=last).collect();
        order.sort_by(|&a, &b| {
            let off = |s: usize| (tops[s] + sizes[s].y * layout.scales[s] / 2.0 - middle).abs();
            off(a).total_cmp(&off(b))
        });
        let mut pages: Vec<usize> = Vec::with_capacity(order.len());
        for sheet in &order {
            if let Some(page) = doc.arrange.page_of(*sheet) {
                if !pages.contains(&page) {
                    pages.push(page);
                }
            }
        }
        if let Ok(mut wanted) = self.wanted.lock() {
            wanted.generation = doc.generation;
            if wanted.pages != pages {
                wanted.pages = pages.clone();
            }
            wanted.moving = false;
            wanted.snapping = false;
            // Nothing here asks for a page to be drawn: a sheet is its
            // thumbnail. So the helpers are told not to draw the rest of the
            // document ahead either, which at this size would be a hundred
            // sheets rendered for pictures the width of a playing card.
            wanted.skip_drawing_ahead = true;
        }
        // Thumbnails of what is in view, from the cache, and one read ahead
        // for a page that has none kept -- the same as the page view does,
        // since at this zoom the thumbnail is the sheet.
        for &page in &pages {
            let asked = doc.thumbs_asked.get(&page).copied();
            if !doc.thumbnails.contains_key(&page) && asked.is_none_or(|asked| now - asked >= THUMBNAIL_RETRY) {
                doc.thumbs_asked.insert(page, now);
                if let Some(thumbs) = &doc.thumbs {
                    thumbs.want(page);
                }
            }
        }
        let all_there = pages.iter().all(|page| doc.thumbnails.contains_key(page));
        if all_there {
            // Outwards from the page in the middle of the view, which is the
            // page wanted most -- not from where its sheet happens to sit.
            gpu::read_a_thumbnail_ahead(doc, pages.first().copied().unwrap_or(0), now);
        }
        self.view_sharp = all_there;

        let dragging = self.sheet_drag.as_ref().is_some_and(|drag| drag.moving);
        // Clicking the paper around the sheets lets everything go. Registered
        // before them, so a sheet drawn over it gets the click instead.
        let elsewhere = ui.interact(ui.max_rect(), Id::new("sheet-paper"), Sense::click());
        let painter = ui.painter();
        let shadow = Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(30) };

        for sheet in first..=last {
            let size = sizes[sheet] * layout.scales[sheet];
            let rect = Rect::from_min_size(origin + vec2(page_x(content_w, size.x), tops[sheet]), size);
            self.page_rects.insert(sheet, rect);
            let picked = doc.arrange.is_selected(sheet);
            let turns = doc.arrange.sheets().get(sheet).map_or(0, |s| s.turns());
            // Sheets being dragged are left faint where they were, so the
            // caret reads as where they are going rather than where they are.
            let fade = if dragging && picked { 0.35 } else { 1.0 };

            painter.add(shadow.as_shape(rect, CornerRadius::same(2)));
            painter.rect_filled(rect, CornerRadius::same(0), Color32::WHITE.gamma_multiply(fade));
            match doc.arrange.page_of(sheet) {
                Some(page) => match doc.thumbnails.get_mut(&page) {
                    Some(thumbnail) => {
                        doc.save_previews.remove(&page);
                        thumbnail.used = now;
                        let tint = Color32::from_white_alpha((fade * 255.0) as u8);
                        image_turned(painter, rect, thumbnail.handle.id(), turns, tint);
                    }
                    None => {
                        if let Some((handle, saved_turns)) = doc.save_previews.get(&page) {
                            image_turned(painter, rect, handle.id(), (saved_turns + turns) % 4, Color32::from_white_alpha((fade * 255.0) as u8));
                        } else {
                            self.blank_pages.push(page);
                        }
                    }
                },
                // A blank sheet is paper and nothing else, which is what it is.
                None => {
                    painter.rect_stroke(rect, CornerRadius::same(0), Stroke::new(1.0, BORDER), StrokeKind::Inside);
                }
            }
            if picked {
                painter.rect_filled(rect, CornerRadius::same(0), ACCENT.gamma_multiply(0.10));
                painter.rect_stroke(rect.expand(1.0), CornerRadius::same(2), Stroke::new(2.0, ACCENT), StrokeKind::Outside);
            }

            // What the sheet is called, under it in the gap: its number here,
            // and the name the file gives the page it shows, which in a
            // drawing set is the sheet number everyone actually uses.
            let label = sheet_label(doc, sheet);
            painter.text(
                pos2(rect.center().x, rect.max.y + 4.0),
                Align2::CENTER_TOP,
                label,
                FontId::proportional(11.0),
                if picked { ACCENT_TEXT } else { MUTED },
            );

            let response = ui.interact(rect, Id::new(("sheet", sheet)), Sense::click_and_drag());
            if response.hovered() {
                ctx.set_cursor_icon(if dragging { CursorIcon::Grabbing } else { CursorIcon::PointingHand });
            }
            if response.clicked_by(egui::PointerButton::Primary) {
                clicked = Some(sheet);
            }
            if ctx.input(|i| i.pointer.primary_pressed()) {
                if let Some(at) = response.interact_pointer_pos() {
                    pressed = Some((sheet, at));
                }
            }
            // Right-clicking a sheet that isn't picked picks it, so the menu
            // is always about something the user can see is chosen.
            if response.secondary_clicked() && !doc.arrange.is_selected(sheet) {
                doc.arrange.click(sheet, false, false);
            }
            let can_paste = !doc.arrange.clipboard_is_empty();
            let items = context::Target::Sheet { at: sheet, can_paste }.items();
            response.context_menu(|ui| {
                ui.set_min_width(190.0);
                for item in items {
                    if item.apart {
                        ui.separator();
                    }
                    if ui.button(item.label).clicked() {
                        if let context::Action::Sheet(action) = item.action {
                            chose = Some(action);
                        }
                        ui.close();
                    }
                }
            });
        }

        // The gap the dragged sheets would land in.
        if let Some(drag) = &self.sheet_drag {
            if drag.moving {
                let caret = drag.caret.min(n);
                let y = match tops.get(caret) {
                    Some(&top) => origin.y + top - PAGE_GAP / 2.0,
                    None => origin.y + tops[n - 1] + sizes[n - 1].y * layout.scales[n - 1] + PAGE_GAP / 2.0,
                };
                let half = (layout.widest / 2.0 + 12.0).min(content_w / 2.0);
                let x = origin.x + content_w / 2.0;
                painter.line_segment([pos2(x - half, y), pos2(x + half, y)], Stroke::new(3.0, ACCENT));
            }
        }

        if elsewhere.clicked() && clicked.is_none() {
            cleared = true;
        }

        /* -------------------------------------------------------------- *
         * What the pointer did, once nothing is borrowing the document
         * -------------------------------------------------------------- */

        if cleared {
            self.sheets_mut(|a| a.clear_selection());
        }
        if let Some(sheet) = clicked {
            let (ctrl, shift) = (modifiers.command, modifiers.shift);
            self.sheets_mut(|a| a.click(sheet, ctrl, shift));
        }
        if let Some((sheet, at)) = pressed {
            // A press on a sheet that isn't picked picks it first, so
            // dragging one out of a set drags that one and not the set.
            let picked = self.doc.as_ref().is_some_and(|d| d.arrange.is_selected(sheet));
            if !picked && !modifiers.command && !modifiers.shift {
                self.sheets_mut(|a| a.click(sheet, false, false));
            }
            self.sheet_drag = Some(SheetDrag { from: at, moving: false, caret: sheet });
        }
        if let Some(drag) = self.sheet_drag.as_mut() {
            if let Some(at) = pointer {
                if (at - drag.from).length() > DRAG_SLACK {
                    drag.moving = true;
                }
                if drag.moving {
                    drag.caret = gap_at(tops, &sizes, layout, at.y - origin.y);
                }
            }
        }
        if released {
            if let Some(drag) = self.sheet_drag.take() {
                if drag.moving {
                    let caret = drag.caret;
                    self.sheets_mut(|a| {
                        a.move_selected(caret);
                    });
                }
            }
        }
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.sheet_drag = None;
        }
        if let Some(action) = chose {
            self.act_on_sheets(action);
        }
    }

    /// Changes the sheet order, and asks for a repaint since everything drawn
    /// depends on it.
    pub(super) fn sheets_mut(&mut self, change: impl FnOnce(&mut crate::arrange::Arrangement)) {
        if matches!(self.status, Status::Saving) {
            return;
        }
        if let Some(doc) = self.doc.as_mut() {
            change(&mut doc.arrange);
            self.ctx.request_repaint();
        }
    }

    /// Does one of the things the menu and the keys offer.
    pub(super) fn act_on_sheets(&mut self, action: SheetAction) {
        // A blank sheet is the size of the sheet it goes after, so a blank in
        // a drawing set is a drawing sheet rather than a letter page.
        let blank_size = match (action, self.doc.as_ref()) {
            (SheetAction::InsertBlank(at), Some(doc)) => {
                let sheet = at.min(doc.arrange.len().saturating_sub(1));
                sheet_size(doc, sheet).map(|s| [s.x, s.y])
            }
            _ => None,
        };
        let picked_everything = self.doc.as_ref().is_some_and(|doc| doc.arrange.selected().len() == doc.arrange.len());
        let mut changed = false;
        self.sheets_mut(|a| {
            changed = match action {
                SheetAction::Cut => a.cut(),
                SheetAction::Copy => a.copy(),
                SheetAction::Paste(Some(at)) => a.paste(at),
                SheetAction::Paste(None) => a.paste_after_selection(),
                SheetAction::Duplicate => a.duplicate(),
                SheetAction::Rotate(quarters) => a.rotate(quarters),
                SheetAction::InsertBlank(at) => a.insert_blank(at + 1, blank_size.unwrap_or([612.0, 792.0])),
                SheetAction::Delete => a.delete(),
                SheetAction::SelectAll => {
                    a.select_all();
                    false
                }
            };
        });
        // Asking to take out every sheet is the one refusal worth saying out
        // loud; the others do nothing because there was nothing to do.
        if !changed && matches!(action, SheetAction::Cut | SheetAction::Delete) && picked_everything {
            self.toast("A PDF has to keep at least one sheet".to_owned());
        }
    }

    /// Undo and redo, while sheets are being sorted, are about the sheet
    /// order; `undo_step` gives them first refusal and falls back to the
    /// markups when there is nothing of the sort to take back.
    pub(super) fn undo_sheets(&mut self, redo: bool) -> bool {
        let mut took = false;
        self.sheets_mut(|a| took = if redo { a.redo() } else { a.undo() });
        took
    }

    /// The keys the sheet view answers. Only while the view is far enough out
    /// for sheets to be picked, and never while something is being typed in,
    /// so Ctrl+C in the find box still copies text.
    pub(super) fn sheet_keys(&mut self, ctx: &egui::Context) {
        if !self.sheet_mode() || ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        // Ctrl+Z and Ctrl+Y are not here: they are taken where every other
        // undo is, in `handle_input`, which asks `undo_sheets` first.
        let (cut, copy, paste, duplicate, all, delete) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::X),
                i.consume_key(Modifiers::COMMAND, Key::C),
                i.consume_key(Modifiers::COMMAND, Key::V),
                i.consume_key(Modifiers::COMMAND, Key::D),
                i.consume_key(Modifiers::COMMAND, Key::A),
                i.consume_key(Modifiers::NONE, Key::Delete) | i.consume_key(Modifiers::NONE, Key::Backspace),
            )
        });
        if cut {
            self.act_on_sheets(SheetAction::Cut);
        }
        if copy {
            self.act_on_sheets(SheetAction::Copy);
        }
        if paste {
            self.act_on_sheets(SheetAction::Paste(None));
        }
        if duplicate {
            self.act_on_sheets(SheetAction::Duplicate);
        }
        if all {
            self.act_on_sheets(SheetAction::SelectAll);
        }
        if delete {
            self.act_on_sheets(SheetAction::Delete);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.sheets_mut(|a| a.clear_selection());
        }
    }

}

/// Draws a thumbnail into `rect`, turned through `turns` quarter-turns
/// clockwise.
///
/// `Painter::image` can only put a texture down square, so the quad is built
/// by hand and the texture's corners are handed round it. Nothing is redrawn
/// to turn a sheet: the thumbnail is the one pdfium already made, shown from
/// a different corner, so turning a whole drawing set costs four vertices a
/// sheet.
pub(super) fn image_turned(painter: &egui::Painter, rect: Rect, texture: egui::TextureId, turns: u8, tint: Color32) {
    if turns.is_multiple_of(4) {
        painter.image(texture, rect, UV_FULL, tint);
        return;
    }
    // The texture's corners, and which of them lands on each corner of the
    // rect -- left-top, right-top, left-bottom, right-bottom -- once the
    // picture has been turned.
    let uv = [UV_FULL.left_top(), UV_FULL.right_top(), UV_FULL.left_bottom(), UV_FULL.right_bottom()];
    let from: [usize; 4] = match turns % 4 {
        1 => [2, 0, 3, 1],
        2 => [3, 2, 1, 0],
        _ => [1, 3, 0, 2],
    };
    let mut mesh = egui::Mesh::with_texture(texture);
    mesh.indices.extend_from_slice(&[0, 1, 2, 2, 1, 3]);
    let corners = [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()];
    for (at, pos) in corners.into_iter().enumerate() {
        mesh.vertices.push(egui::epaint::Vertex { pos, uv: uv[from[at]], color: tint });
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// How big each sheet is: the size of the page it shows, or its own if it is
/// blank.
pub(super) fn sheet_sizes(doc: &Doc) -> Vec<Vec2> {
    (0..doc.arrange.len()).map(|sheet| sheet_size(doc, sheet).unwrap_or(doc.usual_size)).collect()
}

pub(super) fn sheet_size(doc: &Doc, sheet: usize) -> Option<Vec2> {
    let sheet = *doc.arrange.sheets().get(sheet)?;
    let upright = match sheet {
        Sheet::Page { page, .. } => doc.sizes.get(page).copied(),
        Sheet::Blank { size: [w, h], .. } => Some(vec2(w, h)),
    }?;
    // A sheet turned onto its side is as wide as its page is tall.
    Some(if sheet.on_its_side() { vec2(upright.y, upright.x) } else { upright })
}

/// What is written under a sheet: where it sits now, and what the file calls
/// the page it shows.
fn sheet_label(doc: &Doc, sheet: usize) -> String {
    let number = sheet + 1;
    match doc.arrange.sheets().get(sheet) {
        Some(Sheet::Blank { .. }) => format!("{number}  ·  blank"),
        Some(Sheet::Page { page, .. }) => match doc.labels.get(*page).and_then(|l| l.as_deref()) {
            Some(name) if !name.is_empty() => format!("{number}  ·  {name}"),
            _ => number.to_string(),
        },
        None => number.to_string(),
    }
}

/// Which gap between sheets the point `y` down the column is nearest: 0 above
/// the first sheet, `len` below the last. The half-way line of a sheet is
/// where the gap above it gives way to the gap below.
fn gap_at(tops: &[f32], sizes: &[Vec2], layout: &PageLayout, y: f32) -> usize {
    for sheet in 0..tops.len() {
        let middle = tops[sheet] + sizes[sheet].y * layout.scales[sheet] / 2.0;
        if y < middle {
            return sheet;
        }
    }
    tops.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(heights: &[f32]) -> (Vec<f32>, Vec<Vec2>, PageLayout) {
        let sizes: Vec<Vec2> = heights.iter().map(|&h| vec2(100.0, h)).collect();
        let layout = page_layout(&sizes, vec2(100.0, 100.0), 1.0, false);
        (layout.tops.clone(), sizes, layout)
    }

    #[test]
    fn the_drop_caret_follows_the_half_way_line_of_each_sheet() {
        let (tops, sizes, layout) = column(&[100.0, 100.0, 100.0]);
        // Above the first sheet's middle: the gap before it.
        assert_eq!(gap_at(&tops, &sizes, &layout, tops[0] + 10.0), 0);
        // Past it: the gap after it, which is the gap before the second.
        assert_eq!(gap_at(&tops, &sizes, &layout, tops[0] + 60.0), 1);
        assert_eq!(gap_at(&tops, &sizes, &layout, tops[2] + 60.0), 3);
        // Below everything: after the last sheet.
        assert_eq!(gap_at(&tops, &sizes, &layout, tops[2] + 10_000.0), 3);
    }

    #[test]
    fn sheets_are_picked_rather_than_read_only_when_pulled_right_back() {
        // The zoom steps either side of the threshold, so a change to `ZOOMS`
        // that moved it past 20% would be caught here.
        assert!(ZOOMS.iter().any(|&z| z <= SHEET_ZOOM), "no zoom step is far enough out to sort sheets at");
        assert!(ZOOMS.iter().any(|&z| z > SHEET_ZOOM), "no zoom step is close enough in to read at");
    }
}
