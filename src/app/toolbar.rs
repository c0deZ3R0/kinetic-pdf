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
                ui.add_space(6.0);
                self.tools_menu(ui);
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
            if ui.add_enabled(self.compare.is_none(), egui::Button::new("Open PDF…")).clicked() {
                self.pick_and_open();
                ui.close();
            }
            let can_save = self.has_unsaved_work() && !matches!(self.status, Status::Saving) && self.compare.is_none();
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
            if ui.button("What's new").clicked() {
                self.run_action(Action::WhatsNew);
                ui.close();
            }
            if ui.button("About").clicked() {
                self.show_about = true;
                ui.close();
            }
        });
    }

    /// What's done with the document as a whole: comparing it with another.
    fn tools_menu(&mut self, ui: &mut Ui) {
        menu(ui, "Tools", 200.0, |ui| {
            let comparing = self.compare.is_some();
            let label = if comparing { "Exit Kinetic Compare" } else { "Kinetic Compare…" };
            let button = egui::Button::new(label).selected(comparing);
            if ui.add_enabled(self.action_enabled(Action::Compare), button).on_hover_text("Lay another revision of this drawing set over it, sheet by sheet").clicked() {
                self.run_action(Action::Compare);
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
            let (scroll, zoom) = ui.scope(|ui| {
                ui.spacing_mut().slider_rail_height = 6.0;
                ui.visuals_mut().widgets.inactive.bg_fill = Color32::from_rgb(0xb8, 0xc0, 0xcc);
                ui.visuals_mut().selection.bg_fill = ACCENT;
                let scroll = ui.add(egui::Slider::new(&mut self.scroll_speed, prefs::SPEEDS)
                    .logarithmic(true).trailing_fill(true).suffix("×").max_decimals(2))
                    .on_hover_text("Mouse-wheel scrolling speed. 1× is normal; Ctrl-wheel zoom is unchanged.");
                ui.label("Zoom speed");
                let zoom = ui.add(egui::Slider::new(&mut self.zoom_speed, prefs::SPEEDS)
                    .logarithmic(true).trailing_fill(true).suffix("×").max_decimals(2))
                    .on_hover_text("Ctrl-wheel and pinch zoom speed. 1× is normal.");
                (scroll, zoom)
            }).inner;
            // Written once a slider is let go, or changed from the keyboard,
            // rather than at every frame of a drag along it.
            let settled = |r: &egui::Response| r.drag_stopped() || (r.changed() && !r.dragged());
            if settled(&scroll) || settled(&zoom) {
                // Over what the file holds, so the rest of it is kept.
                prefs::Prefs { scroll_speed: self.scroll_speed, zoom_speed: self.zoom_speed, ..prefs::Prefs::load() }.save();
            }
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
                let this = crate::update::VERSION;
                let hover = if cfg!(feature = "store") {
                    format!("Install {tag} from the Microsoft Store (this is v{this}). The app closes while it updates.")
                } else {
                    format!("Download {tag} (this is v{this}). It runs the next time the app starts.")
                };
                if styled_button(ui, &format!("Update to {tag}"), Tone::Primary, false).on_hover_text(hover).clicked() {
                    if cfg!(feature = "store") {
                        // The Store closes the app to install, so unsaved
                        // work is asked about first, as for closing.
                        self.unless_unsaved(Discarding::StoreUpdate);
                    } else {
                        self.updater.install(None);
                    }
                }
            }
            State::Downloading(tag) => {
                let doing = if cfg!(feature = "store") { "Updating to" } else { "Downloading" };
                ui.label(RichText::new(format!("{doing} {tag}…")).size(13.0).color(MUTED));
            }
            // The Store put it in place without closing the app, as it may
            // when the app wasn't among what it had to replace just then.
            State::Ready(tag) if cfg!(feature = "store") => {
                ui.label(RichText::new(format!("{tag} installed")).size(13.0).color(MUTED))
                    .on_hover_text("It runs the next time the app starts.");
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
                        // As wide as this message needs, up to a limit: the
                        // area otherwise keeps the last one's width for a
                        // frame, and "Clipped" after "Clipping…" broke in
                        // two mid-word.
                        let text = RichText::new(message.as_str()).color(Color32::WHITE);
                        let one_line = egui::WidgetText::from(text.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
                        let width = one_line.size().x.ceil().min(TOAST_WIDTH);
                        ui.set_min_width(width);
                        ui.set_max_width(width);
                        ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Wrap));
                    });
            });
    }
}

/// Toasts wider than this, in points, wrap onto more lines.
const TOAST_WIDTH: f32 = 520.0;
