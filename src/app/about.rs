//! The About dialog: the version, the app's licence, and the licences of
//! everything compiled into it. make-notices.ps1 writes those.

use std::sync::OnceLock;

use super::*;

const NOTICES: &str = include_str!("../../assets/THIRD-PARTY-NOTICES.txt");

impl App {
    pub(super) fn about_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        // Split once; the list is drawn a row at a time, only rows in view.
        static LINES: OnceLock<Vec<&'static str>> = OnceLock::new();
        let lines = LINES.get_or_init(|| NOTICES.lines().collect());

        let frame = Frame::NONE
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(20))
            .shadow(soft_shadow());
        let mut close = false;
        let modal = egui::Modal::new(Id::new("about")).frame(frame).backdrop_color(Color32::from_black_alpha(60)).show(ctx, |ui| {
            let width = (ctx.content_rect().width() - 80.0).clamp(320.0, 680.0);
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            ui.label(RichText::new("Kinetic PDF").size(16.0).strong().color(TEXT));
            ui.label(RichText::new(format!("Version {}  ·  MIT OR Apache-2.0", crate::update::VERSION)).size(13.0).color(MUTED));
            ui.add_space(6.0);
            ui.label(RichText::new("Open-source software included in this app, and its licences:").size(13.5).color(QUOTE_TEXT));

            let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
            let list_height = (ctx.content_rect().height() - 260.0).clamp(160.0, 440.0);
            Frame::NONE.fill(BG).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(8)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::both().max_height(list_height).auto_shrink([false, false]).show_rows(ui, row_height, lines.len(), |ui, rows| {
                    for line in &lines[rows] {
                        ui.add(egui::Label::new(RichText::new(*line).monospace().color(TEXT)).wrap_mode(egui::TextWrapMode::Extend));
                    }
                });
            });

            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = styled_button(ui, "Close", Tone::Secondary, false).clicked();
            });
        });
        if close || modal.should_close() {
            self.show_about = false;
        }
    }
}
