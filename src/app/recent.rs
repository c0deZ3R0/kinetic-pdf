//! Successfully opened PDFs, remembered across app launches.
use super::*;

impl App {
    pub(super) fn remember_recent(&mut self, path: &std::path::Path) {
        if self.is_untitled(path) || !path.is_file() {
            return;
        }
        let Ok(path) = path.canonicalize() else {
            return;
        };
        let mut prefs = prefs::Prefs::load();
        prefs.remember_recent(path);
        self.recent_files = prefs.recent_files.clone();
        prefs.save();
    }

    pub(super) fn recent_menu(&mut self, ui: &mut Ui) {
        ui.add_enabled_ui(self.action_enabled(palette::Action::Open), |ui| {
            ui.menu_button("Open Recent", |ui| {
                ui.set_width(360.0);
                if self.recent_files.is_empty() {
                    ui.label(RichText::new("No recent PDFs yet").color(MUTED));
                    return;
                }
                let mut chosen = None;
                let height =
                    (ui.ctx().input(|i| i.content_rect().height()) - 160.0).clamp(120.0, 420.0);
                egui::ScrollArea::vertical()
                    .id_salt("recent-files")
                    .max_height(height)
                    .show(ui, |ui| {
                        for path in &self.recent_files {
                            let name = path.file_name().unwrap_or_default().to_string_lossy();
                            if ui
                                .add(egui::Button::new(name.as_ref()).truncate())
                                .on_hover_text(readable_path(path))
                                .clicked()
                            {
                                chosen = Some(path.clone());
                                ui.close();
                            }
                            if let Some(parent) = path.parent() {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(readable_path(parent)).small().color(MUTED),
                                    )
                                    .truncate(),
                                );
                            }
                            ui.add_space(4.0);
                        }
                    });
                ui.separator();
                if ui.button("Clear Recent Files").clicked() {
                    self.recent_files.clear();
                    let mut prefs = prefs::Prefs::load();
                    prefs.recent_files.clear();
                    prefs.save();
                    ui.close();
                }
                if let Some(path) = chosen {
                    // Check before asking to discard the current document.
                    if path.is_file() {
                        self.unless_unsaved(Discarding::Open(path));
                    } else {
                        self.toast(format!(
                            "This PDF was moved or is unavailable: {}",
                            readable_path(&path)
                        ));
                    }
                }
            });
        });
    }
}

fn readable_path(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    }
}
