//! App preferences in a panel with category navigation.
use super::*;

impl App {
    pub(super) fn settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let available = ctx.input(|i| i.content_rect().size());
        let width = (available.x - 64.0).clamp(480.0, 740.0);
        let height = (available.y - 180.0).clamp(220.0, 360.0);
        let before = self.units;
        let mut close = false;
        let modal = egui::Modal::new(Id::new("app-settings")).show(ctx, |ui| {
            ui.set_width(width);
            ui.heading("Settings");
            ui.add_space(12.0);
            ui.allocate_ui_with_layout(vec2(width, height), Layout::left_to_right(Align::TOP), |ui| {
                ui.vertical(|ui| {
                    ui.set_width(140.0);
                    ui.selectable_label(true, "Units").on_hover_text("Measurement and page-size units");
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(width - 170.0);
                    egui::ScrollArea::vertical().id_salt("settings-content").max_height(height).show(ui, |ui| {
                        ui.heading("Units");
                        ui.label(RichText::new("Choose your default measurement system.").color(MUTED));
                        ui.add_space(16.0);
                        Frame::NONE.fill(SURFACE).inner_margin(Margin::same(14)).corner_radius(CornerRadius::same(6)).show(ui, |ui| {
                            ui.set_width(width - 202.0);
                            ui.radio_value(&mut self.units, prefs::UnitSystem::Metric, "Metric");
                            ui.label(RichText::new("Lengths in metres · areas in m² · volumes in m³").small().color(MUTED));
                            ui.add_space(12.0);
                            ui.radio_value(&mut self.units, prefs::UnitSystem::Imperial, "Imperial");
                            ui.label(RichText::new("Lengths in feet and inches · areas in ft² · volumes in yd³").small().color(MUTED));
                        });
                        ui.add_space(16.0);
                        ui.label("Page dimensions use millimetres in Metric and inches in Imperial.");
                        ui.add_space(8.0);
                        ui.label(RichText::new("New measurement scales use this default. Existing drawing units are preserved; change them in the page's Scale panel.").small().color(MUTED));
                    });
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Changes are saved automatically.").small().color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    close = styled_button(ui, "Done", Tone::Primary, false).clicked();
                });
            });
        });
        if self.units != before {
            prefs::Prefs {
                units: self.units,
                ..prefs::Prefs::load()
            }
            .save();
            ctx.request_repaint();
        }
        if close || modal.should_close() {
            self.show_settings = false;
        }
    }
}
