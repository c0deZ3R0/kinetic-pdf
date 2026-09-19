//! The toolbar, the save status, and toasts.

use super::*;

/// How tall the menu bar's row is: a line of words, not a row of buttons.
const MENU_HEIGHT: f32 = 18.0;

impl App {
    /// Shows a message for a few seconds.
    pub(super) fn toast(&mut self, message: String) {
        let ctx = self.ctx.clone();
        self.show_toast_message(&ctx, message);
    }

    pub(super) fn show_toast_message(&mut self, ctx: &egui::Context, message: String) {
        self.toast = Some((message, Self::now(ctx) + 5.0));
    }

    /* -------------------------------------------------------------- *
     * Toolbar
     * -------------------------------------------------------------- */

    /// The menu bar along the very top: a File menu, and what the file is
    /// doing at the other end. The window's own title bar already says which
    /// file is open, so it isn't said again here.
    pub(super) fn toolbar(&mut self, ui: &mut Ui) {
        let frame = Frame::NONE.fill(SURFACE).inner_margin(Margin::symmetric(8, 1));
        egui::Panel::top("toolbar").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.set_height(MENU_HEIGHT);
                self.file_menu(ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.update_button(ui);
                    let (text, color) = self.status_label(ui.ctx());
                    ui.label(RichText::new(text).size(12.0).color(color));
                });
            });
        });
    }

    fn file_menu(&mut self, ui: &mut Ui) {
        // A word on the bar rather than a button on it: no frame of its own,
        // so the bar reads as one thin line.
        let title = RichText::new("File").size(12.5).color(TEXT);
        let button = egui::Button::new(title).frame(false);
        let menu = egui::containers::menu::MenuButton::from_button(button);
        menu.ui(ui, |ui| {
            ui.set_min_width(180.0);
            if ui.button("Open PDF…").clicked() {
                self.pick_and_open();
                ui.close();
            }
            let can_save = self.doc.as_ref().is_some_and(|d| d.session.is_dirty()) && !matches!(self.status, Status::Saving);
            if ui.add_enabled(can_save, egui::Button::new("Save")).clicked() {
                self.save();
                ui.close();
            }
            ui.separator();
            if ui.button("About").clicked() {
                self.show_about = true;
                ui.close();
            }
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
