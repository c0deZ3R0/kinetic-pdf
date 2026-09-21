//! The toolbar, the save status, and toasts.

use super::palette::Action;
use super::*;

/// How tall the menu bar's row is: a line of words, not a row of buttons.
const MENU_HEIGHT: f32 = 18.0;

impl App {
    /// Saving does not move the viewport or open a dialog.
    pub(super) fn saved_notice(&mut self, ctx: &egui::Context) {
        self.status = Status::Idle;
        self.toast = Some(("Saved".to_owned(), Self::now(ctx) + 2.0));
        ctx.request_repaint();
    }

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

    /// The menu bar along the very top: a File menu, a View menu, and what the
    /// file is doing at the other end. The window's own title bar already says
    /// which file is open, so it isn't said again here.
    pub(super) fn toolbar(&mut self, ui: &mut Ui) {
        let frame = Frame::NONE.fill(SURFACE).inner_margin(Margin::symmetric(8, 1));
        egui::Panel::top("toolbar").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.set_height(MENU_HEIGHT);
                self.file_menu(ui);
                ui.add_space(6.0);
                self.view_menu(ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.update_button(ui);
                    let (text, color) = self.status_label();
                    ui.label(RichText::new(text).size(12.0).color(color));
                });
            });
        });
    }

    fn file_menu(&mut self, ui: &mut Ui) {
        menu(ui, "File", 180.0, |ui| {
            if ui.button("Open PDF…").clicked() {
                self.pick_and_open();
                ui.close();
            }
            let can_save = self.has_unsaved_work() && !matches!(self.status, Status::Saving);
            if ui.add_enabled(can_save, egui::Button::new("Save")).clicked() {
                self.save();
                ui.close();
            }
            ui.separator();
            // The table itself carries no buttons any more, so the way to a
            // spreadsheet is here, beside the other things done to the file.
            let has_rows = self.doc.as_ref().is_some_and(|d| !d.session.measures().is_empty() || !d.session.highlights().is_empty());
            if ui.add_enabled(has_rows, egui::Button::new("Export quantities as CSV…")).clicked() {
                self.export_quantities();
                ui.close();
            }
            ui.separator();
            if ui.button("About").clicked() {
                self.show_about = true;
                ui.close();
            }
        });
    }

    /// Page fitting, display switches and document scrolling speed.
    fn view_menu(&mut self, ui: &mut Ui) {
        menu(ui, "View", 260.0, |ui| {
            self.menu_action(ui, Action::FitWidth, self.zoom_mode == ZoomMode::FitWidth);
            self.menu_action(ui, Action::FitPage, self.zoom_mode == ZoomMode::FitPage);
            ui.separator();
            self.menu_action(ui, Action::Quantities, self.quantities_open);
            self.menu_action(ui, Action::ShrinkWide, self.shrink_wide);
            self.menu_action(ui, Action::SideBySide, self.side_by_side);
            ui.separator();
            ui.label("Scroll speed");
            ui.scope(|ui| {
                ui.spacing_mut().slider_rail_height = 6.0;
                ui.visuals_mut().widgets.inactive.bg_fill = Color32::from_rgb(0xb8, 0xc0, 0xcc);
                ui.visuals_mut().selection.bg_fill = ACCENT;
                ui.add(egui::Slider::new(&mut self.scroll_speed, 0.25..=10.0)
                    .logarithmic(true).trailing_fill(true).suffix("×").max_decimals(2))
                    .on_hover_text("Mouse-wheel scrolling speed. 1× is normal; Ctrl-wheel zoom is unchanged.");
                ui.label("Zoom speed");
                ui.add(egui::Slider::new(&mut self.zoom_speed, 0.25..=10.0)
                    .logarithmic(true).trailing_fill(true).suffix("×").max_decimals(2))
                    .on_hover_text("Ctrl-wheel and pinch zoom speed. 1× is normal.");
            });
        });
    }

    /// One row of a menu, built from the action itself: the palette's name
    /// for it and the shortcut it already answers to, greyed when it can't
    /// run just now, and lit when what it turns on is on already.
    fn menu_action(&mut self, ui: &mut Ui, action: Action, lit: bool) {
        let mut button = egui::Button::new(action.label()).selected(lit);
        if let Some(keys) = action.shortcut() {
            button = button.shortcut_text(keys);
        }
        if ui.add_enabled(self.action_enabled(action), button).clicked() {
            self.run_action(action);
            ui.close();
        }
    }
}

/// A menu on the bar: a word rather than a button, with no frame of its own,
/// so the bar reads as one thin line.
fn menu(ui: &mut Ui, name: &str, width: f32, contents: impl FnOnce(&mut Ui)) {
    let title = RichText::new(name).size(12.5).color(TEXT);
    let button = egui::Button::new(title).frame(false);
    egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        ui.set_min_width(width);
        contents(ui);
    });
}

impl App {

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

    pub(super) fn status_label(&self) -> (&'static str, Color32) {
        match self.status {
            Status::Opening => ("Opening...", MUTED),
            Status::Saving => ("Saving...", MUTED),
            _ if self.has_unsaved_work() => ("Unsaved changes", DIRTY),
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
