//! The find box, stepping through matches, and the results panel.

use super::*;

/// How long typing has to pause before the find box starts a search.
pub(super) const SEARCH_DEBOUNCE: f64 = 0.25;

/// Every row in the results list is this tall, so the list can draw only the
/// rows in view however many matches there are.
pub(super) const RESULT_ROW_HEIGHT: f32 = 64.0;

/// One search result: the page, then up to two lines of the surrounding text
/// with the match picked out. Painted directly at a fixed height so the list
/// can skip rows that are scrolled out of view.
pub(super) fn result_row(ui: &mut Ui, hit: &SearchHit, current: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), RESULT_ROW_HEIGHT), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();

    let fill = if current {
        NOTE_ACTIVE
    } else if response.hovered() {
        ROW_HOVER
    } else {
        SURFACE
    };
    painter.rect_filled(rect, CornerRadius::same(0), fill);
    if current {
        painter.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), CornerRadius::same(0), HIT_OUTLINE);
    }
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, ROW_RULE));
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let inner = rect.shrink2(vec2(14.0, 8.0));
    painter.text(inner.left_top(), Align2::LEFT_TOP, format!("Page {}", hit.page + 1), FontId::proportional(12.0), MUTED);

    let font = FontId::proportional(13.0);
    let mut job = LayoutJob::default();
    job.wrap.max_width = inner.width();
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    let around = TextFormat { font_id: font.clone(), color: QUOTE_TEXT, ..Default::default() };
    job.append(&hit.before, 0.0, around.clone());
    job.append(&hit.matched, 0.0, TextFormat { font_id: font, color: TEXT, background: HIT, ..Default::default() });
    job.append(&hit.after, 0.0, around);
    painter.galley(inner.left_top() + vec2(0.0, 18.0), painter.layout_job(job), QUOTE_TEXT);

    response
}

impl App {
    /* -------------------------------------------------------------- *
     * Find in document
     * -------------------------------------------------------------- */

    /// Starts a search once typing pauses.
    pub(super) fn update_search(&mut self, ctx: &egui::Context) {
        if self.doc.is_none() || self.search.query == self.search.sent {
            return;
        }
        let wait = self.search.edited_at + SEARCH_DEBOUNCE - Self::now(ctx);
        if wait <= 0.0 {
            self.start_search();
        } else {
            ctx.request_repaint_after(Duration::from_secs_f64(wait));
        }
    }

    pub(super) fn start_search(&mut self) {
        let Some(doc) = &self.doc else { return };
        let search = &mut self.search;
        search.id += 1;
        search.sent = search.query.clone();
        search.hits.clear();
        search.current = None;
        search.searched = 0;
        search.done = selection::normalize_query(&search.query).is_empty();
        search.start_page = self.current_page;
        search.reveal_row = None;
        search.list_scroll_to = Some(0.0);
        let _ = self.tx.send(Request::Search { generation: doc.generation, id: search.id, query: search.query.clone() });
    }

    /// Next (1) or previous (-1) match, wrapping around. With no match chosen
    /// yet, starts from the page in view.
    pub(super) fn step_hit(&mut self, direction: isize) {
        let n = self.search.hits.len();
        if n == 0 {
            return;
        }
        let i = match self.search.current {
            Some(current) => (current as isize + direction).rem_euclid(n as isize) as usize,
            None if direction > 0 => self.search.hits.iter().position(|h| h.page >= self.current_page).unwrap_or(0),
            None => self.search.hits.iter().rposition(|h| h.page <= self.current_page).unwrap_or(n - 1),
        };
        self.go_to_hit(i);
    }

    /// Select a match, bring its row into view in the results list, and scroll
    /// the page to it unless it's already comfortably in view.
    pub(super) fn go_to_hit(&mut self, i: usize) {
        let Some(doc) = &self.doc else { return };
        let Some(hit) = self.search.hits.get(i) else { return };
        let Some(q) = hit.quads.first().copied() else { return };
        let page = hit.page;
        self.search.current = Some(i);
        self.search.reveal_row = Some(i);

        if let (Some(rect), Some(geometry)) = (self.page_rects.get(&page), doc.geometry[page]) {
            if self.viewer_rect.shrink(40.0).contains_rect(to_screen(*rect, &geometry, &q)) {
                return;
            }
        }
        self.scroll_to_box(page, &q);
    }

    pub(super) fn match_count(&self) -> String {
        let n = self.search.hits.len();
        if n >= MAX_SEARCH_HITS { format!("{MAX_SEARCH_HITS}+") } else { n.to_string() }
    }

    pub(super) fn search_progress(&self) -> usize {
        let pages = self.doc.as_ref().map_or(1, |d| d.sizes.len().max(1));
        self.search.searched * 100 / pages
    }

    /// The short status beside the find box.
    pub(super) fn search_label(&self) -> String {
        let s = &self.search;
        if selection::normalize_query(&s.sent).is_empty() {
            return String::new();
        }
        let count = self.match_count();
        if !s.done {
            return format!("{count} found · {}%", self.search_progress());
        }
        match (s.hits.len(), s.current) {
            (0, _) => "No matches".to_owned(),
            (_, Some(current)) => format!("{} of {count}", current + 1),
            _ => format!("{count} matches"),
        }
    }

    /// The find box and its controls, laid out right to left inside the
    /// toolbar's right-hand group.
    pub(super) fn search_box(&mut self, ui: &mut Ui) {
        let any = !self.search.hits.is_empty();
        ui.add_enabled_ui(any, |ui| {
            // The arrows are in monospace: egui's bundled proportional font has
            // no arrow glyphs, but its monospace font does.
            if icon_button(ui, "↓").on_hover_text("Next match (Enter or F3)").clicked() {
                self.step_hit(1);
            }
            if icon_button(ui, "↑").on_hover_text("Previous match (Shift+Enter or Shift+F3)").clicked() {
                self.step_hit(-1);
            }
        });
        let label = self.search_label();
        if !label.is_empty() {
            ui.label(RichText::new(label).size(13.0).color(MUTED));
        }

        let response = ui.add_enabled(
            self.doc.is_some(),
            TextEdit::singleline(&mut self.search.query)
                .id(Id::new("find"))
                .hint_text("Find in document")
                .desired_width(200.0)
                .margin(Margin { left: 28, right: 8, top: 6, bottom: 6 }),
        );
        paint_magnifier(ui, response.rect);
        if response.changed() {
            self.search.edited_at = Self::now(ui.ctx());
        }
        if std::mem::take(&mut self.search.focus) {
            response.request_focus();
        }
        // A single-line box gives up focus on Enter and Esc.
        if response.lost_focus() {
            let (enter, escape, shift) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape), i.modifiers.shift));
            if enter {
                if self.search.query != self.search.sent {
                    self.start_search();
                } else {
                    self.step_hit(if shift { -1 } else { 1 });
                }
                // Stay in the box, so Enter can be pressed again.
                response.request_focus();
            } else if escape {
                self.search.query.clear();
                self.start_search();
            }
        }

        let results = self.sidebar == Sidebar::Results;
        if styled_button(ui, "Results", Tone::Secondary, results).on_hover_text("List every match in a side panel").clicked() {
            self.sidebar = if results { Sidebar::None } else { Sidebar::Results };
        }
    }

    pub(super) fn results_panel(&mut self, ui: &mut Ui) {
        self.side_panel(
            ui,
            "results",
            |app, ui| panel_heading(ui, "Search results", app.results_summary()),
            None,
            |app, ui| app.result_rows(ui),
        );
    }

    /// e.g. "37 on 12 pages", with progress while the search is running.
    pub(super) fn results_summary(&self) -> String {
        let s = &self.search;
        if selection::normalize_query(&s.sent).is_empty() {
            return String::new();
        }
        let mut summary = if s.hits.is_empty() {
            "No matches".to_owned()
        } else {
            let pages = 1 + s.hits.windows(2).filter(|w| w[0].page != w[1].page).count();
            format!("{} on {pages} page{}", self.match_count(), if pages == 1 { "" } else { "s" })
        };
        if !s.done {
            if s.hits.is_empty() {
                summary = "Searching".to_owned();
            }
            summary += &format!(" · {}%", self.search_progress());
        }
        summary
    }

    pub(super) fn result_rows(&mut self, ui: &mut Ui) {
        let query = self.search.sent.trim().to_owned();
        if query.is_empty() {
            let hint = if self.doc.is_some() {
                "Type in the find box (Ctrl+F) and every match is listed here."
            } else {
                "Open a PDF to search it."
            };
            empty_note(ui, hint);
            return;
        }
        if self.search.done && self.search.hits.is_empty() {
            empty_note(ui, &format!("No matches for “{query}”."));
            return;
        }

        let mut area = egui::ScrollArea::vertical().id_salt("results-list").auto_shrink(false);
        if let Some(i) = self.search.reveal_row.take() {
            // Only scroll when the row isn't already fully in view.
            let top = i as f32 * RESULT_ROW_HEIGHT;
            let (offset, height) = (self.search.list_offset, self.search.list_height);
            if height <= 0.0 || top < offset || top + RESULT_ROW_HEIGHT > offset + height {
                area = area.vertical_scroll_offset((top - (height - RESULT_ROW_HEIGHT).max(0.0) / 2.0).max(0.0));
            }
            self.search.list_scroll_to = None;
        } else if let Some(y) = self.search.list_scroll_to.take() {
            area = area.vertical_scroll_offset(y);
        }

        ui.spacing_mut().item_spacing.y = 0.0;
        let search = &self.search;
        let mut clicked = None;
        let output = area.show_rows(ui, RESULT_ROW_HEIGHT, search.hits.len(), |ui, rows| {
            for i in rows {
                if result_row(ui, &search.hits[i], search.current == Some(i)).clicked() {
                    clicked = Some(i);
                }
            }
        });
        self.search.list_offset = output.state.offset.y;
        self.search.list_height = output.inner_rect.height();

        if let Some(i) = clicked {
            self.go_to_hit(i);
        }
    }
}
