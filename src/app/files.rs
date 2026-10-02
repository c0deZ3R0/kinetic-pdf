//! Creating documents, choosing save destinations, and print scaling.
use super::*;

pub(super) struct NewPdfDialog {
    preset: usize,
    landscape: bool,
    mm: [f32; 2],
    pages: u16,
}
impl Default for NewPdfDialog {
    fn default() -> Self {
        Self {
            preset: 4,
            landscape: false,
            mm: [210.0, 297.0],
            pages: 1,
        }
    }
}
impl NewPdfDialog {
    fn size(&self) -> [f32; 2] {
        let mm = crate::printing::PAPER_SIZES
            .get(self.preset)
            .map_or(self.mm, |p| p.1);
        let [w, h] = mm.map(|mm| mm * 72.0 / 25.4);
        if self.landscape {
            [h, w]
        } else {
            [w, h]
        }
    }
}

impl App {
    pub(super) fn new_pdf_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.new_document.take() else {
            return;
        };
        let mut create = false;
        let mut cancel = false;
        let modal = egui::Modal::new(Id::new("new-pdf")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("New PDF");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Page size");
                egui::ComboBox::from_id_salt("new-paper")
                    .selected_text(
                        crate::printing::PAPER_SIZES
                            .get(dialog.preset)
                            .map_or("Custom", |p| p.0),
                    )
                    .show_ui(ui, |ui| {
                        for (i, (name, _)) in crate::printing::PAPER_SIZES.iter().enumerate() {
                            ui.selectable_value(&mut dialog.preset, i, *name);
                        }
                        ui.selectable_value(
                            &mut dialog.preset,
                            crate::printing::PAPER_SIZES.len(),
                            "Custom",
                        );
                    });
            });
            if dialog.preset == crate::printing::PAPER_SIZES.len() {
                let factor = self.units.paper_factor() * 72.0 / 25.4;
                let mut dimensions = dialog.mm.map(|mm| mm * factor);
                let previous = dimensions;
                ui.horizontal(|ui| {
                    ui.label("Width");
                    ui.add(
                        egui::DragValue::new(&mut dimensions[0])
                            .range(10.0 * factor..=5000.0 * factor)
                            .max_decimals(2)
                            .suffix(format!(" {}", self.units.paper_suffix())),
                    );
                    ui.label("Height");
                    ui.add(
                        egui::DragValue::new(&mut dimensions[1])
                            .range(10.0 * factor..=5000.0 * factor)
                            .max_decimals(2)
                            .suffix(format!(" {}", self.units.paper_suffix())),
                    );
                });
                if dimensions != previous {
                    dialog.mm = dimensions.map(|value| value / factor);
                }
            }
            ui.horizontal(|ui| {
                ui.label("Orientation");
                ui.radio_value(&mut dialog.landscape, false, "Portrait");
                ui.radio_value(&mut dialog.landscape, true, "Landscape");
            });
            ui.horizontal(|ui| {
                ui.label("Pages");
                ui.add(egui::DragValue::new(&mut dialog.pages).range(1..=999));
            });
            let size = dialog.size();
            ui.add_space(12.0);
            ui.label(format!(
                "{} · {} page{}",
                self.units.paper_size(size),
                dialog.pages,
                if dialog.pages == 1 { "" } else { "s" }
            ));
            let preview = vec2(size[0], size[1]) * (160.0 / size[0].max(size[1]));
            ui.vertical_centered(|ui| {
                let (rect, _) = ui.allocate_exact_size(preview, Sense::hover());
                ui.painter().rect(
                    rect,
                    CornerRadius::ZERO,
                    Color32::WHITE,
                    Stroke::new(1.0, BORDER),
                    StrokeKind::Inside,
                );
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                create = styled_button(ui, "Create PDF", Tone::Primary, false).clicked();
                cancel = styled_button(ui, "Cancel", Tone::Secondary, false).clicked();
            });
        });
        if create {
            self.unless_unsaved(Discarding::New {
                size: dialog.size(),
                pages: dialog.pages,
            });
        } else if !cancel && !modal.should_close() {
            self.new_document = Some(dialog);
        }
    }
    pub(super) fn is_untitled(&self, path: &std::path::Path) -> bool {
        self.temporary_documents
            .iter()
            .any(|dir| path.starts_with(dir.path()))
    }

    pub(super) fn new_pdf(&mut self) {
        if self.action_enabled(palette::Action::NewPdf) {
            self.new_document = Some(NewPdfDialog::default());
        }
    }

    pub(super) fn create_pdf(&mut self, size: [f32; 2], pages: u16) {
        if pages == 0
            || size
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0 || *v > 14400.0)
        {
            self.toast("Choose a valid page size and at least one page".into());
            return;
        }
        let create = || -> Result<tempfile::TempDir, String> {
            use pdf_content::lopdf::{dictionary, Document, Object};
            let dir = tempfile::Builder::new()
                .prefix("kinetic-pdf-new-")
                .tempdir()
                .map_err(|e| e.to_string())?;
            let mut pdf = Document::with_version("1.7");
            let page_root = pdf.add_object(
                dictionary! { "Type" => "Pages", "Kids" => Vec::<Object>::new(), "Count" => 0 },
            );
            let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => page_root });
            pdf.trailer.set("Root", catalog);
            crate::arrange::append_pages(&mut pdf, &vec![size; usize::from(pages)])?;
            pdf.save(dir.path().join("Untitled.pdf"))
                .map_err(|e| e.to_string())?;
            Ok(dir)
        };
        match create() {
            Ok(dir) => {
                let path = dir.path().join("Untitled.pdf");
                self.temporary_documents.push(dir);
                self.open(path);
            }
            Err(error) => self.toast(format!("Could not create PDF: {error}")),
        }
    }

    pub(super) fn save_as(&mut self) {
        if !self.action_enabled(palette::Action::SaveAs) {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save PDF As")
            .add_filter("PDF", &["pdf"])
            .set_file_name(doc.path.file_name().unwrap_or_default().to_string_lossy());
        if !self.is_untitled(&doc.path) {
            if let Some(parent) = doc.path.parent() {
                dialog = dialog.set_directory(parent);
            }
        }
        if let Some(mut path) = dialog.save_file() {
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            self.save_to(Some(path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_pdf_asks_before_creating_and_uses_the_chosen_orientation_and_count() {
        let (mut app, _) = super::super::tests::app_with_a_document();
        app.new_pdf();
        assert!(app.temporary_documents.is_empty());
        let mut dialog = app.new_document.take().unwrap();
        dialog.preset = 3;
        dialog.landscape = true;
        dialog.pages = 3;
        let size = dialog.size();
        app.create_pdf(size, dialog.pages);
        let path = app
            .temporary_documents
            .last()
            .unwrap()
            .path()
            .join("Untitled.pdf");
        let pdf = pdf_content::lopdf::Document::load(path).unwrap();
        assert_eq!(pdf.get_pages().len(), 3);
        for page in pdf.get_pages().values() {
            let bounds = pdf
                .get_dictionary(*page)
                .unwrap()
                .get(b"MediaBox")
                .unwrap()
                .as_array()
                .unwrap();
            assert!((bounds[2].as_float().unwrap() - 420.0 * 72.0 / 25.4).abs() < 0.01);
            assert!((bounds[3].as_float().unwrap() - 297.0 * 72.0 / 25.4).abs() < 0.01);
        }
    }
    #[test]
    fn a_new_pdf_has_one_a4_page_and_counts_as_unsaved() {
        let (mut app, _) = super::super::tests::app_with_a_document();
        app.create_pdf([595.276, 841.89], 1);
        let path = app
            .temporary_documents
            .last()
            .unwrap()
            .path()
            .join("Untitled.pdf");
        let pdf = pdf_content::lopdf::Document::load(&path).unwrap();
        let pages = pdf.get_pages();
        assert_eq!(pages.len(), 1);
        let page = pdf.get_dictionary(*pages.values().next().unwrap()).unwrap();
        let bounds = page.get(b"MediaBox").unwrap().as_array().unwrap();
        assert!((bounds[2].as_float().unwrap() - 595.276).abs() < 0.01);
        assert!((bounds[3].as_float().unwrap() - 841.89).abs() < 0.01);
        app.doc.as_mut().unwrap().path = path;
        // Save becomes available once the worker finishes opening the page.
        app.lifecycle.finish();
        assert!(app.has_unsaved_work());
        assert!(app.action_enabled(palette::Action::Save));
    }
}
