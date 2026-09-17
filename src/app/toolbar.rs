//! The toolbar, the save status, and toasts.

use super::*;

impl App {
    pub(super) fn show_toast_message(&mut self, ctx: &egui::Context, message: String) {
        self.toast = Some((message, Self::now(ctx) + 5.0));
    }

    /* -------------------------------------------------------------- *
     * Toolbar
     * -------------------------------------------------------------- */

    pub(super) fn toolbar(&mut self, ui: &mut Ui) {
        let frame = Frame::NONE.fill(SURFACE).inner_margin(Margin::symmetric(12, 8));
        egui::Panel::top("toolbar").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;

                if styled_button(ui, "Open PDF…", Tone::Primary, false).on_hover_text("Open (Ctrl+O)").clicked() {
                    self.pick_and_open();
                }
                let can_save = self.doc.as_ref().is_some_and(|d| d.session.is_dirty()) && !matches!(self.status, Status::Saving);
                let save = ui.add_enabled_ui(can_save, |ui| styled_button(ui, "Save", Tone::Secondary, false)).inner;
                if save.on_hover_text("Save (Ctrl+S)").clicked() {
                    self.save();
                }

                ui.separator();

                let has_doc = self.doc.is_some();
                ui.add_enabled_ui(has_doc, |ui| {
                    if icon_button(ui, "−").on_hover_text("Zoom out (Ctrl+-)").clicked() {
                        self.zoom_by(-1);
                    }
                    ui.add_sized(
                        vec2(46.0, 30.0),
                        egui::Label::new(RichText::new(format!("{:.0}%", self.zoom * 100.0)).size(13.0).color(MUTED)),
                    );
                    if icon_button(ui, "+").on_hover_text("Zoom in (Ctrl++, or Ctrl + mouse wheel)").clicked() {
                        self.zoom_by(1);
                    }
                    let fit_width = self.zoom_mode == ZoomMode::FitWidth;
                    if styled_button(ui, "Fit width", Tone::Secondary, fit_width).on_hover_text("Fit the page width (Ctrl+0)").clicked() {
                        self.zoom_mode = ZoomMode::FitWidth;
                    }
                    let fit_page = self.zoom_mode == ZoomMode::FitPage;
                    if styled_button(ui, "Fit page", Tone::Secondary, fit_page).on_hover_text("Fit a whole page").clicked() {
                        self.zoom_mode = ZoomMode::FitPage;
                    }
                });

                ui.separator();

                // The page number is a box to type in: a number and Enter goes
                // to that page. It shows where the view is the rest of the
                // time, so it follows scrolling unless it's being typed in.
                match self.doc.as_ref().map(|d| d.sizes.len()) {
                    Some(pages) => {
                        let box_id = Id::new("page-number");
                        let typing = ui.memory(|m| m.has_focus(box_id));
                        if !typing {
                            self.page_box = (self.current_page + 1).to_string();
                        }
                        let field = TextEdit::singleline(&mut self.page_box)
                            .id(box_id)
                            .desired_width(34.0)
                            .horizontal_align(Align::Center)
                            // The box is taller than a line of text, which
                            // otherwise sits at its top.
                            .vertical_align(Align::Center)
                            .font(FontId::proportional(13.0));
                        let response = ui.add_sized(vec2(38.0, 26.0), field).on_hover_text("Go to a page (Ctrl+G): type a number and press Enter");
                        if std::mem::take(&mut self.page_box_focus) {
                            self.page_box.clear();
                            response.request_focus();
                        }
                        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            match self.page_box.trim().parse::<usize>() {
                                Ok(page) if (1..=pages).contains(&page) => self.go_to_page(page - 1),
                                // Anything else goes back to where the view is.
                                _ => self.page_box = (self.current_page + 1).to_string(),
                            }
                        }
                        ui.label(RichText::new(format!("/ {pages}")).size(13.0).color(MUTED));
                    }
                    None => {
                        ui.label(RichText::new("—").size(13.0).color(MUTED));
                    }
                }
                let name = self.doc.as_ref().map_or_else(|| "No file open".to_owned(), |d| d.name.clone());
                ui.add_space(4.0);
                ui.label(RichText::new(name).size(13.5).color(if has_doc { TEXT } else { MUTED }));

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if styled_button(ui, "Quit", Tone::Ghost, false).on_hover_text("Close the app").clicked() {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                    }
                    if styled_button(ui, "About", Tone::Ghost, false).on_hover_text("Version and licences").clicked() {
                        self.show_about = true;
                    }
                    let notes = self.sidebar == Sidebar::Notes;
                    if styled_button(ui, "Notes", Tone::Secondary, notes).on_hover_text("Show or hide the notes panel").clicked() {
                        self.sidebar = if notes { Sidebar::None } else { Sidebar::Notes };
                    }
                    self.update_button(ui);
                    ui.separator();
                    self.search_box(ui);
                    let (text, color) = self.status_label(ui.ctx());
                    ui.label(RichText::new(text).size(13.0).color(color));
                });
            });
        });
    }

    /// Offers a newer release once one is found (update.rs), then a restart
    /// into it once it's swapped in.
    fn update_button(&mut self, ui: &mut Ui) {
        use crate::update::State;
        match self.updater.state() {
            State::Current => {}
            State::Available(tag) => {
                let hover = format!("Download {tag} (this is v{}). It runs the next time the app starts.", crate::update::VERSION);
                if styled_button(ui, &format!("Update to {tag}"), Tone::Primary, false).on_hover_text(hover).clicked() {
                    self.updater.install();
                }
            }
            State::Downloading(tag) => {
                ui.label(RichText::new(format!("Downloading {tag}…")).size(13.0).color(MUTED));
            }
            State::Ready(tag) => {
                let restart = styled_button(ui, "Restart to update", Tone::Primary, false);
                if restart.on_hover_text(format!("{tag} is installed; restart now, or it runs next time")).clicked() {
                    self.unless_unsaved(Discarding::Restart);
                    if self.allow_close {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                    }
                }
            }
            State::Failed(message) => {
                ui.label(RichText::new("Update failed").size(13.0).color(DIRTY)).on_hover_text(message);
            }
        }
    }

    pub(super) fn status_label(&mut self, ctx: &egui::Context) -> (&'static str, Color32) {
        match self.status {
            Status::Opening => ("Opening...", MUTED),
            Status::Saving => ("Saving...", MUTED),
            _ if self.doc.as_ref().is_some_and(|d| d.session.is_dirty()) => ("Unsaved changes", DIRTY),
            Status::Saved { until } => {
                let now = Self::now(ctx);
                if now < until {
                    ctx.request_repaint_after(Duration::from_secs_f64(until - now));
                    ("Saved", SAVED)
                } else {
                    self.status = Status::Idle;
                    ("", MUTED)
                }
            }
            Status::Idle => ("", MUTED),
        }
    }

    /* -------------------------------------------------------------- *
     * Toast
     * -------------------------------------------------------------- */

    pub(super) fn show_toast(&mut self, ctx: &egui::Context) {
        let now = Self::now(ctx);
        let Some((message, until)) = &self.toast else { return };
        if now > *until {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_secs_f64(until - now));
        egui::Area::new(Id::new("toast"))
            .order(Order::Tooltip)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -28.0))
            .interactable(false)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(TEXT)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(16, 10))
                    .shadow(soft_shadow())
                    .show(ui, |ui| {
                        ui.label(RichText::new(message.as_str()).color(Color32::WHITE));
                    });
            });
    }
}
