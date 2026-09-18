//! The quantities panel: every measurement in the document, what each one
//! measures, and the totals.
//!
//! The numbers themselves are never held here. Each row reads what the
//! session already worked out when the measurement last changed, so opening
//! the panel costs a walk over the measurements and nothing else, and a
//! recalibration shows through it at once.

use markup_model::markup::MarkupKind;
use markup_model::quantity::Totals;
use markup_model::{MarkupId, Quantities};

use super::*;

/// A line of the table, taken from the session as the panel draws.
struct Row {
    id: MarkupId,
    page: usize,
    kind: MarkupKind,
    label: String,
    /// When it was taken, so a page's measurements read in the order they
    /// were made.
    created_ms: i64,
    /// What it measures, or what's wrong with it.
    text: String,
    /// False when it has no number: no scale, or a shape that can't be
    /// measured. Those are left out of the totals.
    counted: bool,
}

/// A measurement's line in a CSV file: the numbers in metres and square
/// metres, whatever the page is shown in, and the text as the panel shows it.
fn csv_line(row: &Row, q: Option<&Quantities>) -> String {
    let number = |v: Option<f64>| v.map_or(String::new(), |v| format!("{v:.6}"));
    let cell = |s: &str| {
        if s.contains([',', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    };
    let q = q.copied().unwrap_or_default();
    let fields = [
        (row.page + 1).to_string(),
        cell(row.kind.label()),
        cell(&row.label),
        cell(&row.text),
        number(q.length_m),
        number(q.area_m2),
        number(q.perimeter_m),
        number(q.volume_m3),
        q.count.map_or(String::new(), |c| c.to_string()),
        number(q.angle_deg),
        number(q.radius_m),
        number(q.diameter_m),
    ];
    fields.join(",")
}

const CSV_HEADINGS: &str = "Page,Kind,Label,Measured,Length (m),Area (m2),Perimeter (m),Volume (m3),Count,Angle (deg),Radius (m),Diameter (m)";

impl App {
    /// Every measurement as a row, in the order they read on the page: page
    /// by page, and within a page in the order they were taken.
    fn quantity_rows(&self) -> Vec<Row> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let mut rows: Vec<Row> = doc
            .session
            .measures()
            .iter()
            .filter(|(m, _)| !self.quantities_this_page || m.page as usize == self.current_page)
            .map(|(m, measured)| {
                let scale = measured.scale.and_then(|id| doc.session.scales().scale(id));
                let units = scale.map_or(Default::default(), |s| s.display);
                let precision = scale.map_or(Default::default(), |s| s.precision);
                let (text, counted) = match &measured.result {
                    Ok(q) if q.uncalibrated => ("no scale on this page".to_owned(), false),
                    Ok(q) => (q.text(m.kind, &units, precision).unwrap_or_default(), true),
                    Err(e) => (e.to_string(), false),
                };
                let created_ms = m.meta.created_ms.unwrap_or(0);
                Row { id: m.id, page: m.page as usize, kind: m.kind, label: m.meta.label.clone(), created_ms, text, counted }
            })
            .collect();
        // The store is keyed by ID, so the order it gives is its own; these
        // are sorted for reading, oldest first within a page.
        rows.sort_by_key(|row| (row.page, row.created_ms, row.id));
        rows
    }

    /// The totals of what the table is showing.
    fn quantity_totals(&self) -> Totals {
        let Some(doc) = self.doc.as_ref() else { return Totals::default() };
        let (this_page, page) = (self.quantities_this_page, self.current_page as u32);
        doc.session.measures().totals(|m| !this_page || m.page == page)
    }

    /// Scrolls a measurement into view and picks it out.
    fn reveal_measurement(&mut self, id: MarkupId) {
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        let Some(bounds) = markup.geometry.bounds() else { return };
        let page = markup.page as usize;
        let box_ = crate::model::PdfBox::spanning([bounds.min.x as f32, bounds.min.y as f32], [bounds.max.x as f32, bounds.max.y as f32]);
        self.scroll_to_box(page, &box_);
        self.active_measure = Some(id);
        self.active_vertex = None;
    }

    /// The whole table as CSV, in the order and selection the panel shows.
    fn quantities_csv(&self) -> String {
        let Some(doc) = self.doc.as_ref() else { return String::new() };
        let mut out = String::from(CSV_HEADINGS);
        for row in self.quantity_rows() {
            let measured = doc.session.measures().measured(row.id);
            let q = measured.and_then(|m| m.result.as_ref().ok());
            out.push('\n');
            out.push_str(&csv_line(&row, q));
        }
        out
    }

    pub(super) fn quantities_panel(&mut self, ui: &mut Ui) {
        self.want_measurements();
        self.side_panel(
            ui,
            "quantities",
            |app, ui| {
                let count = app.doc.as_ref().map_or(0, |d| d.session.measures().len());
                let detail = match (count, app.doc.as_ref().map(|d| &d.measurements)) {
                    (_, Some(MeasureRead::Reading)) => "reading…".to_owned(),
                    (0, _) => String::new(),
                    (1, _) => "1 measurement".to_owned(),
                    (n, _) => format!("{n} measurements"),
                };
                panel_heading(ui, "Quantities", detail);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let page = app.quantities_this_page;
                    if styled_button(ui, "This page", Tone::Secondary, page).on_hover_text("Show only what's on the page you're looking at").clicked() {
                        app.quantities_this_page = !page;
                    }
                    if styled_button(ui, "Export CSV…", Tone::Secondary, false).on_hover_text("Save the table as a spreadsheet").clicked() {
                        app.export_quantities();
                    }
                });
            },
            Some(|app, ui| app.quantity_totals_row(ui)),
            |app, ui| {
                egui::ScrollArea::vertical().id_salt("quantities-list").auto_shrink(false).show(ui, |ui| {
                    app.quantity_table(ui);
                });
            },
        );
    }

    /// The totals at the foot of the panel, in the units the current page is
    /// shown in.
    fn quantity_totals_row(&mut self, ui: &mut Ui) {
        let totals = self.quantity_totals();
        let scale = self.doc.as_ref().and_then(|doc| crate::app::scale::page_scale(doc, self.current_page));
        let units = scale.map_or(Default::default(), |s| s.display);
        let precision = scale.map_or(Default::default(), |s| s.precision);
        use markup_model::units::{format_area, format_length, format_volume, group_thousands};
        let lines = [
            ("Length", (totals.length_m > 0.0).then(|| format_length(totals.length_m, units.length, precision))),
            ("Area", (totals.area_m2 > 0.0).then(|| format_area(totals.area_m2, units.area, precision))),
            ("Perimeter", (totals.perimeter_m > 0.0).then(|| format_length(totals.perimeter_m, units.length, precision))),
            ("Volume", (totals.volume_m3 > 0.0).then(|| format_volume(totals.volume_m3, units.volume, precision))),
            ("Count", (totals.count > 0).then(|| group_thousands(totals.count as f64, 0))),
        ];
        if lines.iter().all(|(_, v)| v.is_none()) {
            ui.label(RichText::new("Nothing to total yet.").size(12.0).color(MUTED));
            return;
        }
        ui.label(RichText::new("Totals").size(12.0).strong().color(TEXT));
        for (name, value) in lines.into_iter().filter_map(|(n, v)| v.map(|v| (n, v))) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).size(12.0).color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(value).size(12.5).color(TEXT));
                });
            });
        }
        if totals.excluded > 0 {
            let left_out = format!("{} left out: no scale, or a shape that can't be measured", totals.excluded);
            ui.label(RichText::new(left_out).size(11.5).color(SUBTLE));
        }
    }

    fn quantity_table(&mut self, ui: &mut Ui) {
        let Some(doc) = self.doc.as_ref() else {
            empty_note(ui, "Open a PDF to see what's been measured.");
            return;
        };
        if let MeasureRead::Failed(error) = &doc.measurements {
            empty_note(ui, &format!("The measurements couldn't be read: {error}"));
            return;
        }
        let rows = self.quantity_rows();
        if rows.is_empty() {
            let message = match (&doc.measurements, self.quantities_this_page) {
                (MeasureRead::Reading | MeasureRead::NotRead, _) => "Reading the measurements…",
                (_, true) => "Nothing measured on this page yet.",
                (_, false) => "Nothing measured yet. Set the page's scale, then take one with the tools above.",
            };
            empty_note(ui, message);
            return;
        }

        let clicked = ui.input(|i| i.pointer.primary_clicked());
        let mut reveal = None;
        let mut delete = None;
        for row in &rows {
            let fill = if self.active_measure == Some(row.id) { NOTE_ACTIVE } else { SURFACE };
            let drawn = Frame::NONE.fill(fill).inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let remove = ui
                    .horizontal(|ui| {
                        let name = if row.label.is_empty() { row.kind.label().to_owned() } else { format!("{} · {}", row.kind.label(), row.label) };
                        ui.label(RichText::new(name).size(12.5).color(TEXT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            paint_button(ui, "×", FontId::proportional(16.0), Tone::Ghost, false, vec2(22.0, 22.0)).on_hover_text("Delete this measurement")
                        })
                        .inner
                    })
                    .inner;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Page {}", row.page + 1)).size(11.5).color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let colour = if row.counted { TEXT } else { SUBTLE };
                        ui.label(RichText::new(row.text.as_str()).size(12.5).color(colour));
                    });
                });
                remove
            });
            let rect = drawn.response.rect;
            ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, ROW_RULE));
            if drawn.inner.clicked() {
                delete = Some(row.id);
            } else if clicked && ui.rect_contains_pointer(rect) && !drawn.inner.hovered() {
                reveal = Some(row.id);
            }
        }
        if let Some(id) = delete {
            if let Some(doc) = self.doc.as_mut() {
                doc.session.apply(crate::session::Command::RemoveMeasure(id));
            }
            if self.active_measure == Some(id) {
                self.active_measure = None;
            }
        } else if let Some(id) = reveal {
            self.reveal_measurement(id);
        }
    }

    /// Writes the table to a file the user picks.
    fn export_quantities(&mut self) {
        let csv = self.quantities_csv();
        let name = self
            .doc
            .as_ref()
            .and_then(|d| d.path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "quantities".to_owned());
        let Some(path) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name(format!("{name} quantities.csv")).save_file() else {
            return;
        };
        match std::fs::write(&path, csv) {
            Ok(()) => self.toast(format!("Saved to {}", path.display())),
            Err(e) => self.toast(format!("Couldn't save the table: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_becomes_a_line_of_the_file() {
        let row = Row {
            id: MarkupId(1),
            page: 4,
            kind: MarkupKind::Area,
            label: "Slab, ground floor".to_owned(),
            created_ms: 0,
            text: "188.00 m²".to_owned(),
            counted: true,
        };
        let q = Quantities { area_m2: Some(188.0), perimeter_m: Some(56.5), ..Quantities::default() };
        let line = csv_line(&row, Some(&q));
        // The page as people count them, the label quoted for its comma, the
        // numbers in metres whatever the page is shown in.
        assert_eq!(line, "5,Area,\"Slab, ground floor\",188.00 m²,,188.000000,56.500000,,,,,");
    }

    #[test]
    fn a_measurement_with_no_number_still_has_a_line() {
        let row =
            Row { id: MarkupId(2), page: 0, kind: MarkupKind::Length, label: String::new(), created_ms: 0, text: "no scale on this page".to_owned(), counted: false };
        let line = csv_line(&row, None);
        assert_eq!(line, "1,Length,,no scale on this page,,,,,,,,");
        assert_eq!(line.matches(',').count(), CSV_HEADINGS.matches(',').count(), "a cell for each heading");
    }
}
