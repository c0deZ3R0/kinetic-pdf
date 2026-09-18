//! The quantities table across the bottom of the window: every measurement
//! in the document, a row each, with the numbers in columns and totals under
//! them.
//!
//! Nothing is measured here. Each row reads what the session worked out when
//! the measurement last changed, so the table costs a walk over the
//! measurements and nothing more, and a recalibration shows through it at
//! once.

use markup_model::markup::MarkupKind;
use markup_model::quantity::{QuantityError, Totals};
use markup_model::units::{format_area, format_length, format_volume, group_thousands, DisplayUnits, Precision};
use markup_model::{MarkupId, Quantities};

use super::*;

/// How the rows are gathered together.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum GroupBy {
    #[default]
    None,
    /// Everything called the same thing, whichever page it's on: the way a
    /// take-off is priced.
    Description,
    Page,
    Kind,
}

impl GroupBy {
    const ALL: [GroupBy; 4] = [GroupBy::None, GroupBy::Description, GroupBy::Page, GroupBy::Kind];

    fn label(self) -> &'static str {
        match self {
            GroupBy::None => "Nothing",
            GroupBy::Description => "Description",
            GroupBy::Page => "Page",
            GroupBy::Kind => "Kind",
        }
    }
}

/// A line of the table, taken from the session as it draws.
struct Row {
    id: MarkupId,
    page: usize,
    kind: MarkupKind,
    /// What it's called: the name the quantity is priced under.
    label: String,
    /// When it was taken, so a page's measurements read in the order they
    /// were made.
    created_ms: i64,
    result: Result<Quantities, QuantityError>,
    /// What its own kind measures, as shown on the page.
    text: String,
}

impl Row {
    /// The quantities for the number columns: none for a measurement with no
    /// scale or a shape that can't be measured.
    fn numbers(&self) -> Option<Quantities> {
        self.result.ok().filter(|q| !q.uncalibrated)
    }

    /// What the row is filed under, given how the table is grouped.
    fn group(&self, by: GroupBy) -> String {
        match by {
            GroupBy::None => String::new(),
            GroupBy::Description if self.label.is_empty() => "No description".to_owned(),
            GroupBy::Description => self.label.clone(),
            GroupBy::Page => format!("Page {}", self.page + 1),
            GroupBy::Kind => self.kind.label().to_owned(),
        }
    }
}

/// The numbers a row shows, column by column, blank where its kind has
/// nothing to say.
fn columns(q: Option<Quantities>, units: &DisplayUnits, precision: Precision) -> [String; 5] {
    let Some(q) = q else { return Default::default() };
    let length = |m: Option<f64>| m.map_or(String::new(), |m| format_length(m, units.length, precision));
    [
        length(q.length_m),
        q.area_m2.map_or(String::new(), |a| format_area(a, units.area, precision)),
        length(q.perimeter_m),
        q.volume_m3.map_or(String::new(), |v| format_volume(v, units.volume, precision)),
        q.count.map_or(String::new(), |c| group_thousands(c as f64, 0)),
    ]
}

/// The same for a group's or the table's totals, where nothing of a kind
/// leaves its column empty rather than showing a zero.
fn total_columns(totals: &Totals, units: &DisplayUnits, precision: Precision) -> [String; 5] {
    let some = |v: f64| (v > 0.0).then_some(v);
    let q = Quantities {
        length_m: some(totals.length_m),
        area_m2: some(totals.area_m2),
        perimeter_m: some(totals.perimeter_m),
        volume_m3: some(totals.volume_m3),
        count: (totals.count > 0).then_some(totals.count),
        ..Quantities::default()
    };
    columns(Some(q), units, precision)
}

const HEADINGS: [&str; 9] = ["Description", "Kind", "Page", "Measured", "Length", "Area", "Perimeter", "Volume", "Count"];

/// A cell in a CSV file, quoted if it has to be.
fn cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// A measurement's line in a CSV file: what the table shows, then the raw
/// numbers in metres and square metres whatever the page is shown in, so a
/// spreadsheet has something to add up.
fn csv_line(row: &Row, group: &str, shown: &[String; 5]) -> String {
    let number = |v: Option<f64>| v.map_or(String::new(), |v| format!("{v:.6}"));
    let q = row.numbers().unwrap_or_default();
    let note = match &row.result {
        Ok(q) if q.uncalibrated => "no scale on this page".to_owned(),
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    };
    let mut fields = vec![cell(group), cell(&row.label), cell(row.kind.label()), (row.page + 1).to_string(), cell(&row.text)];
    fields.extend(shown.iter().map(|s| cell(s)));
    fields.extend([
        number(q.length_m),
        number(q.area_m2),
        number(q.perimeter_m),
        number(q.volume_m3),
        q.count.map_or(String::new(), |c| c.to_string()),
        number(q.angle_deg),
        number(q.radius_m),
        number(q.diameter_m),
        cell(&note),
    ]);
    fields.join(",")
}

const CSV_HEADINGS: &str = "Group,Description,Kind,Page,Measured,Length,Area,Perimeter,Volume,Count,\
Length (m),Area (m2),Perimeter (m),Volume (m3),Count (n),Angle (deg),Radius (m),Diameter (m),Note";

impl App {
    /// Every measurement as a row, in the order they read: page by page, and
    /// within a page in the order they were taken.
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
                let text = match &measured.result {
                    Ok(q) if q.uncalibrated => "no scale".to_owned(),
                    Ok(q) => q.text(m.kind, &units, precision).unwrap_or_default(),
                    Err(e) => e.to_string(),
                };
                Row {
                    id: m.id,
                    page: m.page as usize,
                    kind: m.kind,
                    label: m.meta.label.clone(),
                    created_ms: m.meta.created_ms.unwrap_or(0),
                    result: measured.result,
                    text,
                }
            })
            .collect();
        // The store is keyed by ID, so the order it gives is its own.
        rows.sort_by(|a, b| (a.page, a.created_ms, a.id).cmp(&(b.page, b.created_ms, b.id)));
        rows
    }

    /// The rows under the heading each belongs to, in the order the headings
    /// first appear. One nameless group when nothing is grouped.
    fn quantity_groups(&self, rows: Vec<Row>) -> Vec<(String, Vec<Row>)> {
        let by = self.quantity_group;
        let mut groups: Vec<(String, Vec<Row>)> = Vec::new();
        for row in rows {
            let name = row.group(by);
            match groups.iter_mut().find(|(n, _)| *n == name) {
                Some((_, rows)) => rows.push(row),
                None => groups.push((name, vec![row])),
            }
        }
        groups
    }

    /// The units the table is written in: the current page's, since a drawing
    /// set is nearly always in one system throughout.
    fn quantity_units(&self) -> (DisplayUnits, Precision) {
        let scale = self.doc.as_ref().and_then(|doc| crate::app::scale::page_scale(doc, self.current_page));
        (scale.map_or(Default::default(), |s| s.display), scale.map_or(Default::default(), |s| s.precision))
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

    /// Renames a measurement. Typing is one step to undo rather than one per
    /// letter: the changes merge until the box is left.
    fn describe_measurement(&mut self, id: MarkupId, label: String, done: bool) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        if markup.meta.label != label {
            let mut renamed = markup.clone();
            renamed.meta.label = label;
            doc.session.apply_merged(crate::session::Command::ChangeMeasure(Box::new(renamed)));
        }
        if done {
            doc.session.end_merge();
        }
    }

    /// The whole table as CSV, grouped and ordered as it's shown.
    fn quantities_csv(&self) -> String {
        let (units, precision) = self.quantity_units();
        let mut out = String::from(CSV_HEADINGS);
        for (name, rows) in self.quantity_groups(self.quantity_rows()) {
            for row in &rows {
                out.push('\n');
                out.push_str(&csv_line(row, &name, &columns(row.numbers(), &units, precision)));
            }
            // A group's subtotal is a line of its own, so a spreadsheet shows
            // the same shape as the table.
            if self.quantity_group != GroupBy::None {
                let totals: Totals = rows.iter().map(|r| &r.result).collect();
                let mut fields = vec![cell(&name), format!("Subtotal ({} measurements)", rows.len()), String::new(), String::new(), String::new()];
                fields.extend(total_columns(&totals, &units, precision).iter().map(|s| cell(s)));
                out.push('\n');
                out.push_str(&fields.join(","));
            }
        }
        out
    }

    /// The table across the bottom of the window.
    pub(super) fn quantities_dock(&mut self, ui: &mut Ui) {
        let frame = Frame::NONE.fill(SURFACE).inner_margin(Margin::symmetric(12, 8));
        egui::Panel::bottom("quantities").resizable(true).default_size(260.0).min_size(120.0).frame(frame).show(ui, |ui| {
            self.want_measurements();
            self.quantities_header(ui);
            ui.add_space(6.0);
            egui::ScrollArea::both().id_salt("quantities-table").auto_shrink(false).show(ui, |ui| {
                self.quantities_table(ui);
            });
        });
    }

    fn quantities_header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let count = self.doc.as_ref().map_or(0, |d| d.session.measures().len());
            let detail = match (count, self.doc.as_ref().map(|d| &d.measurements)) {
                (_, Some(MeasureRead::Reading)) => "reading…".to_owned(),
                (0, _) => String::new(),
                (1, _) => "1 measurement".to_owned(),
                (n, _) => format!("{n} measurements"),
            };
            ui.label(RichText::new("Quantities").size(14.0).strong().color(TEXT));
            ui.label(RichText::new(detail).size(12.0).color(MUTED));
            ui.separator();
            let page = self.quantities_this_page;
            if styled_button(ui, "This page", Tone::Secondary, page).on_hover_text("Show only what's on the page you're looking at").clicked() {
                self.quantities_this_page = !page;
            }
            ui.separator();
            ui.label(RichText::new("Group by").size(12.0).color(MUTED));
            for by in GroupBy::ALL {
                if styled_button(ui, by.label(), Tone::Secondary, self.quantity_group == by).clicked() {
                    self.quantity_group = by;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if styled_button(ui, "Close", Tone::Ghost, false).on_hover_text("Hide the table").clicked() {
                    self.quantities_open = false;
                }
                if styled_button(ui, "Export CSV…", Tone::Secondary, false).on_hover_text("Save the table as a spreadsheet").clicked() {
                    self.export_quantities();
                }
            });
        });
    }

    fn quantities_table(&mut self, ui: &mut Ui) {
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
        let whole: Totals = rows.iter().map(|r| &r.result).collect();
        let groups = self.quantity_groups(rows);
        let (units, precision) = self.quantity_units();
        let grouped = self.quantity_group != GroupBy::None;

        let mut reveal = None;
        let mut delete = None;
        let mut rename = None;
        egui::Grid::new("quantities-grid").num_columns(HEADINGS.len() + 1).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
            for heading in HEADINGS {
                ui.label(RichText::new(heading).size(11.5).strong().color(MUTED));
            }
            ui.label("");
            ui.end_row();

            for (name, rows) in &groups {
                if grouped {
                    ui.label(RichText::new(name.as_str()).size(12.5).strong().color(ACCENT));
                    for _ in 0..HEADINGS.len() {
                        ui.label("");
                    }
                    ui.end_row();
                }
                for row in rows {
                    let active = self.active_measure == Some(row.id);
                    let mut label = row.label.clone();
                    let box_ = ui.add(TextEdit::singleline(&mut label).id_salt(row.id.0 as u64).hint_text("Describe it").desired_width(170.0).margin(Margin::symmetric(6, 3)));
                    if box_.changed() || box_.lost_focus() {
                        rename = Some((row.id, label, box_.lost_focus()));
                    }
                    let mut picked = false;
                    picked |= ui.selectable_label(active, RichText::new(row.kind.label()).size(12.0)).clicked();
                    picked |= ui.selectable_label(active, RichText::new((row.page + 1).to_string()).size(12.0)).clicked();
                    let told = if row.numbers().is_some() { TEXT } else { SUBTLE };
                    picked |= ui.selectable_label(active, RichText::new(row.text.as_str()).size(12.0).color(told)).clicked();
                    for value in columns(row.numbers(), &units, precision) {
                        picked |= ui.selectable_label(active, RichText::new(value).size(12.0)).clicked();
                    }
                    if picked {
                        reveal = Some(row.id);
                    }
                    let bin = paint_button(ui, "×", FontId::proportional(15.0), Tone::Ghost, false, vec2(22.0, 20.0));
                    if bin.on_hover_text("Delete this measurement").clicked() {
                        delete = Some(row.id);
                    }
                    ui.end_row();
                }
                if grouped {
                    let totals: Totals = rows.iter().map(|r| &r.result).collect();
                    subtotal_row(ui, &format!("Subtotal · {} measurements", rows.len()), &totals, &units, precision);
                }
            }
            subtotal_row(ui, "Total", &whole, &units, precision);
        });
        if whole.excluded > 0 {
            ui.add_space(4.0);
            let left_out = format!("{} left out of the totals: no scale, or a shape that can't be measured", whole.excluded);
            ui.label(RichText::new(left_out).size(11.5).color(SUBTLE));
        }

        if let Some((id, label, done)) = rename {
            self.describe_measurement(id, label, done);
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

/// A line of sums: the name in the first column, the numbers under their own.
fn subtotal_row(ui: &mut Ui, name: &str, totals: &Totals, units: &DisplayUnits, precision: Precision) {
    ui.label(RichText::new(name).size(12.0).strong().color(TEXT));
    for _ in 0..3 {
        ui.label("");
    }
    for value in total_columns(totals, units, precision) {
        ui.label(RichText::new(value).size(12.0).strong().color(TEXT));
    }
    ui.label("");
    ui.end_row();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &str, page: usize, kind: MarkupKind, result: Result<Quantities, QuantityError>) -> Row {
        Row { id: MarkupId(page as u128 + 1), page, kind, label: label.to_owned(), created_ms: 0, result, text: "188.00 m²".to_owned() }
    }

    fn area(m2: f64) -> Result<Quantities, QuantityError> {
        Ok(Quantities { area_m2: Some(m2), perimeter_m: Some(56.5), ..Quantities::default() })
    }

    #[test]
    fn a_row_becomes_a_line_of_the_file() {
        let row = row("Slab, ground floor", 4, MarkupKind::Area, area(188.0));
        let shown = columns(row.numbers(), &DisplayUnits::METRIC, Precision::Decimals(2));
        let line = csv_line(&row, "Slabs", &shown);
        // The page as people count them, the description quoted for its
        // comma, what the table shows, then the numbers in metres.
        assert!(line.starts_with("Slabs,\"Slab, ground floor\",Area,5,188.00 m²,,188.00 m²,56.50 m,,"), "{line}");
        assert!(line.ends_with(",188.000000,56.500000,,,,,,"), "the raw numbers, and no note: {line}");
        // One comma of the description's own is inside quotes.
        assert_eq!(line.split(',').count() - 1, CSV_HEADINGS.split(',').count(), "a cell for each heading: {line}");
    }

    #[test]
    fn a_measurement_with_no_number_says_why_and_counts_as_left_out() {
        let no_scale = row("", 0, MarkupKind::Length, Ok(Quantities { uncalibrated: true, ..Quantities::default() }));
        assert_eq!(no_scale.numbers(), None);
        let shown = columns(no_scale.numbers(), &DisplayUnits::METRIC, Precision::Decimals(2));
        assert!(shown.iter().all(|s| s.is_empty()), "no numbers to show");
        assert!(csv_line(&no_scale, "", &shown).ends_with("no scale on this page"));

        let crossed = row("", 1, MarkupKind::Area, Err(QuantityError::SelfIntersecting));
        let totals: Totals = [no_scale.result, crossed.result, area(10.0)].iter().collect();
        assert_eq!((totals.included, totals.excluded), (1, 2));
        assert_eq!(totals.area_m2, 10.0);
    }

    #[test]
    fn rows_are_filed_under_the_heading_they_are_grouped_by() {
        let slab = row("Slab", 2, MarkupKind::Area, area(1.0));
        let unnamed = row("", 2, MarkupKind::Length, area(1.0));
        assert_eq!(slab.group(GroupBy::None), "");
        assert_eq!(slab.group(GroupBy::Description), "Slab");
        assert_eq!(unnamed.group(GroupBy::Description), "No description");
        assert_eq!(slab.group(GroupBy::Page), "Page 3");
        assert_eq!(unnamed.group(GroupBy::Kind), "Length");
    }
}
