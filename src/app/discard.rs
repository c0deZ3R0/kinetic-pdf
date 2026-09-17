//! Asking, in a dialog of our own, before unsaved highlights and markups are
//! thrown away.

use super::*;

/// What was asked for while there were unsaved changes, done once the user
/// agrees to lose them.
pub(super) enum Discarding {
    /// Choose a file to open.
    Pick,
    Open(PathBuf),
    Close,
    /// Start the version an update put in place, and close this one.
    Restart,
}

impl App {
    /// Does `then` now if nothing is unsaved, and otherwise asks first.
    pub(super) fn unless_unsaved(&mut self, then: Discarding) {
        if self.doc.as_ref().is_some_and(|d| d.dirty) {
            self.discarding = Some(then);
            self.popup = None;
            self.drag = None;
        } else {
            self.carry_out(then);
        }
    }

    fn carry_out(&mut self, then: Discarding) {
        match then {
            Discarding::Pick => {
                #[cfg(not(target_os = "android"))]
                if let Some(path) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                    self.open(path);
                }
                // TODO(android): open through the system's document picker.
            }
            Discarding::Open(path) => self.open(path),
            Discarding::Close => self.allow_close = true,
            Discarding::Restart => {
                let file = self.doc.as_ref().map(|d| d.path.clone());
                match crate::update::relaunch(file.as_deref()) {
                    Ok(()) => self.allow_close = true,
                    Err(e) => self.updater.fail(e),
                }
            }
        }
    }

    /// The dialog, while something waits on the answer. Esc or a click outside
    /// keeps the changes.
    pub(super) fn discard_dialog(&mut self, ctx: &egui::Context) {
        let Some(then) = &self.discarding else { return };
        let (question, confirm) = match then {
            Discarding::Close => ("You have unsaved highlights or markups. Close without saving them?", "Close without saving"),
            Discarding::Restart => ("You have unsaved highlights or markups. Restart without saving them?", "Restart without saving"),
            _ => ("You have unsaved highlights or markups. Discard them?", "Discard"),
        };
        let frame = Frame::NONE
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(20))
            .shadow(soft_shadow());
        let (mut discard, mut keep) = (false, false);
        let modal = egui::Modal::new(Id::new("discard")).frame(frame).backdrop_color(Color32::from_black_alpha(60)).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 5.0, DIRTY);
                ui.label(RichText::new("Unsaved changes").size(16.0).strong().color(TEXT));
            });
            ui.label(RichText::new(question).size(13.5).color(QUOTE_TEXT));
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                discard = styled_button(ui, confirm, Tone::Danger, false).clicked();
                keep = styled_button(ui, "Keep editing", Tone::Secondary, false).clicked();
            });
        });
        if keep || modal.should_close() {
            self.discarding = None;
        } else if discard {
            if let Some(then) = self.discarding.take() {
                self.carry_out(then);
                if self.allow_close {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }
}
