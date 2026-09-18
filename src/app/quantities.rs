//! The quantities table across the bottom of the window: every measurement
//! in the document, a row each, with the numbers in columns and a subtotal
//! under each group.
//!
//! Nothing is measured here. Each row reads what the session worked out when
//! the measurement last changed, so the table costs a walk over the
//! measurements and nothing more, and a recalibration shows through it at
//! once.

use markup_model::markup::MarkupKind;
use markup_model::quantity::{QuantityError, Totals};
use markup_model::units::{format_area, format_length, format_volume, group_thousands, DisplayUnits, Precision};
use markup_model::{MarkupId, Quantities};

use egui_extras::{Column, TableBuilder};

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

/// Which column the table is sorted by, and which way round. Without one it
/// reads in the order the measurements were taken, page by page.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Sort {
    column: usize,
    descending: bool,
}

/// Numbers in a column, with empty cells always at the bottom whichever way
/// the column is sorted: a measurement with nothing to say in it isn't the
/// largest or the smallest.
fn compare(a: Option<f64>, b: Option<f64>, descending: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) if descending => b.total_cmp(&a),
        (Some(a), Some(b)) => a.total_cmp(&b),
    }
}

/// Which of a row's two typed cells is open.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    Name,
    Description,
    Depth,
}

/// A cell open for typing. The text lives here rather than in the measurement
/// while it's being typed: a depth would otherwise be written back out at
/// every keystroke and rewrite itself under the caret.
pub(super) struct Edit {
    id: MarkupId,
    field: Field,
    text: String,
    /// Whether it has been given the keyboard yet.
    focused: bool,
}

/// A line of the table: a heading, a measurement, or a line of sums. The
/// table is one flat run of them, so it draws only the lines in view.
enum Line<'a> {
    Group(&'a str),
    Measurement(&'a Row),
    Sum(String, Totals),
}

/// A line of the table, taken from the session as it draws.
struct Row {
    id: MarkupId,
    page: usize,
    kind: MarkupKind,
    /// An area's depth, if it has one, which makes it a volume.
    depth_m: Option<f64>,
    /// What the measurement is called, ahead of the description.
    name: String,
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

    /// The number under column `c`, for sorting by it: the columns after the
    /// text ones, in the order they're shown.
    fn number(&self, c: usize) -> Option<f64> {
        let q = self.numbers();
        match c {
            // What its own kind measures, so a column of mixed kinds still
            // sorts by size.
            4 => q.and_then(|q| q.length_m.or(q.area_m2).or(q.angle_deg).or(q.radius_m).or(q.diameter_m).or(q.count.map(|c| c as f64))),
            5 => q.and_then(|q| q.length_m),
            6 => q.and_then(|q| q.area_m2),
            7 => q.and_then(|q| q.perimeter_m),
            8 => self.depth_m,
            9 => q.and_then(|q| q.volume_m3),
            10 => q.and_then(|q| q.count.map(|c| c as f64)),
            _ => None,
        }
    }

    /// Whether a depth belongs on this row: an area priced by volume is an
    /// area with a depth against it.
    fn takes_depth(&self) -> bool {
        matches!(self.kind, MarkupKind::Area | MarkupKind::Volume)
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
fn columns(q: Option<Quantities>, units: &DisplayUnits, precision: Precision) -> [String; 4] {
    let Some(q) = q else { return Default::default() };
    let length = |m: Option<f64>| m.map_or(String::new(), |m| format_length(m, units.length, precision));
    [
        length(q.length_m),
        q.area_m2.map_or(String::new(), |a| format_area(a, units.area, precision)),
        length(q.perimeter_m),
        q.count.map_or(String::new(), |c| group_thousands(c as f64, 0)),
    ]
}

/// The same for a group's or the table's totals, where nothing of a kind
/// leaves its column empty rather than showing a zero.
fn total_columns(totals: &Totals, units: &DisplayUnits, precision: Precision) -> [String; 4] {
    let some = |v: f64| (v > 0.0).then_some(v);
    let q = Quantities {
        length_m: some(totals.length_m),
        area_m2: some(totals.area_m2),
        perimeter_m: some(totals.perimeter_m),
        count: (totals.count > 0).then_some(totals.count),
        ..Quantities::default()
    };
    columns(Some(q), units, precision)
}

/// A volume, or nothing when there's none: its own column, since a depth is
/// typed into the row beside it.
fn volume_cell(m3: Option<f64>, units: &DisplayUnits, precision: Precision) -> String {
    m3.map_or(String::new(), |v| format_volume(v, units.volume, precision))
}

const HEADINGS: [&str; 11] = ["Name", "Description", "Kind", "Page", "Measured", "Length", "Area", "Perimeter", "Depth", "Volume", "Count"];

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
fn csv_line(row: &Row, group: &str, shown: &[String; 4], depth: &str, volume: &str) -> String {
    let number = |v: Option<f64>| v.map_or(String::new(), |v| format!("{v:.6}"));
    let q = row.numbers().unwrap_or_default();
    let note = match &row.result {
        Ok(q) if q.uncalibrated => "no scale on this page".to_owned(),
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    };
    let mut fields = vec![cell(group), cell(&row.name), cell(&row.label), cell(row.kind.label()), (row.page + 1).to_string(), cell(&row.text)];
    fields.extend(shown[..3].iter().map(|s| cell(s)));
    fields.extend([cell(depth), cell(volume), cell(&shown[3])]);
    fields.extend([
        row.depth_m.map_or(String::new(), |d| format!("{d:.6}")),
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

const CSV_HEADINGS: &str = "Group,Name,Description,Kind,Page,Measured,Length,Area,Perimeter,Depth,Volume,Count,\
Depth (m),Length (m),Area (m2),Perimeter (m),Volume (m3),Count (n),Angle (deg),Radius (m),Diameter (m),Note";

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
                    depth_m: m.extras.depth_m,
                    name: m.meta.name.clone(),
                    label: m.meta.label.clone(),
                    created_ms: m.meta.created_ms.unwrap_or(0),
                    result: measured.result,
                    text,
                }
            })
            .collect();
        // The store is keyed by ID, so the order it gives is its own. Sorted
        // by a column when one was clicked, and within it by where the
        // measurements are, so rows keep a settled order.
        let taken = |r: &Row| (r.page, r.created_ms, r.id);
        rows.sort_by(|a, b| match self.quantity_sort {
            None => taken(a).cmp(&taken(b)),
            Some(Sort { column, descending }) => {
                let text = match column {
                    0 => Some(a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                    1 => Some(a.label.to_lowercase().cmp(&b.label.to_lowercase())),
                    2 => Some(a.kind.label().cmp(b.kind.label())),
                    3 => Some(a.page.cmp(&b.page)),
                    _ => None,
                };
                let ordered = match text {
                    Some(order) if descending => order.reverse(),
                    Some(order) => order,
                    None => compare(a.number(column), b.number(column), descending),
                };
                ordered.then_with(|| taken(a).cmp(&taken(b)))
            }
        });
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

    /// Renames a measurement, once the typing is finished: one step to undo,
    /// not one per letter.
    /// Sets what a measurement is called, the column ahead of its description.
    fn name_measurement(&mut self, id: MarkupId, name: String) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        if markup.meta.name == name {
            return;
        }
        let mut renamed = markup.clone();
        renamed.meta.name = name;
        doc.session.apply(crate::session::Command::ChangeMeasure(Box::new(renamed)));
    }

    fn describe_measurement(&mut self, id: MarkupId, label: String) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        if markup.meta.label == label {
            return;
        }
        let mut renamed = markup.clone();
        renamed.meta.label = label;
        doc.session.apply(crate::session::Command::ChangeMeasure(Box::new(renamed)));
    }

    /// Sets how deep an area goes, which is what makes it a volume: an area
    /// with a depth is priced by volume, and clearing the depth makes it an
    /// area again. Anything that doesn't parse is left alone, so a half-typed
    /// number doesn't wipe what's there.
    fn deepen_measurement(&mut self, id: MarkupId, text: &str) {
        let assumed = self.quantity_units().0.length;
        let depth = match text.trim() {
            "" => Some(None),
            typed => markup_model::units::parse_length(typed, Some(assumed)).ok().filter(|m| m.is_finite() && *m > 0.0).map(Some),
        };
        let Some(depth) = depth else {
            self.toast(format!("{text} isn't a depth I can read"));
            return;
        };
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        let kind = match depth {
            Some(_) => MarkupKind::Volume,
            None => MarkupKind::Area,
        };
        if markup.extras.depth_m != depth || markup.kind != kind {
            let mut deepened = markup.clone();
            deepened.extras.depth_m = depth;
            deepened.kind = kind;
            doc.session.apply(crate::session::Command::ChangeMeasure(Box::new(deepened)));
        }
    }

    /// The whole table as CSV, grouped and ordered as it's shown.
    fn quantities_csv(&self) -> String {
        let (units, precision) = self.quantity_units();
        let mut out = String::from(CSV_HEADINGS);
        for (name, rows) in self.quantity_groups(self.quantity_rows()) {
            for row in &rows {
                let depth = row.depth_m.map_or(String::new(), |d| format_length(d, units.length, precision));
                let volume = volume_cell(row.numbers().and_then(|q| q.volume_m3), &units, precision);
                out.push('\n');
                out.push_str(&csv_line(row, &name, &columns(row.numbers(), &units, precision), &depth, &volume));
            }
            // A group's subtotal is a line of its own, so a spreadsheet shows
            // the same shape as the table.
            if self.quantity_group != GroupBy::None {
                let totals: Totals = rows.iter().map(|r| &r.result).collect();
                let shown = total_columns(&totals, &units, precision);
                let mut fields =
                    vec![cell(&name), format!("Subtotal ({} measurements)", rows.len()), String::new(), String::new(), String::new(), String::new()];
                fields.extend(shown[..3].iter().map(|s| cell(s)));
                fields.extend([String::new(), cell(&volume_cell(Some(totals.volume_m3).filter(|v| *v > 0.0), &units, precision)), cell(&shown[3])]);
                out.push('\n');
                out.push_str(&fields.join(","));
            }
        }
        out
    }

    /// The table across the bottom of the window.
    pub(super) fn quantities_dock(&mut self, ui: &mut Ui) {
        let frame = Frame::NONE.fill(SURFACE).inner_margin(Margin::symmetric(12, 8));
        // Dragged to any height, up to the whole window under the tool row:
        // a long take-off is read as a table, not through a slot.
        let panel = egui::Panel::bottom("quantities").resizable(true).default_size(260.0).size_range(90.0..=f32::INFINITY);
        panel.frame(frame).show(ui, |ui| {
            self.want_measurements();
            self.quantities_header(ui);
            ui.add_space(6.0);
            // The table scrolls itself, and keeps its heading row in place
            // while it does.
            self.quantities_table(ui);
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
        if whole.excluded > 0 {
            let left_out = format!("{} left out of the totals: no scale, or a shape that can't be measured", whole.excluded);
            ui.label(RichText::new(left_out).size(11.5).color(SUBTLE));
        }

        // One flat run of lines -- a heading, its measurements and its
        // subtotal -- so the table can leave the lines out of view undrawn
        // however many there are.
        let mut lines: Vec<Line> = Vec::new();
        for (name, rows) in &groups {
            if grouped {
                lines.push(Line::Group(name));
            }
            lines.extend(rows.iter().map(Line::Measurement));
            if grouped {
                let totals: Totals = rows.iter().map(|r| &r.result).collect();
                lines.push(Line::Sum(format!("Subtotal · {} measurements", rows.len()), totals));
            }
        }

        let sort = self.quantity_sort;
        let picked = self.active_measure;
        // The cell being typed in, held out of the app while the table draws
        // so each cell can reach it.
        let mut edit = self.quantity_edit.take();

        let mut reveal = None;
        let mut delete = None;
        let mut open = None;
        let mut done = None;
        let mut sort_by = None;
        let number = || Column::initial(94.0).at_least(56.0).clip(true);
        TableBuilder::new(ui)
            .id_salt("quantities")
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .auto_shrink([false, false])
            .column(Column::initial(150.0).at_least(80.0).clip(true))
            .column(Column::initial(210.0).at_least(90.0).clip(true))
            .column(Column::initial(90.0).at_least(50.0).clip(true))
            .column(Column::initial(56.0).at_least(40.0).clip(true))
            .column(Column::initial(120.0).at_least(60.0).clip(true))
            .columns(number(), 3)
            .column(Column::initial(88.0).at_least(56.0).clip(true))
            .columns(number(), 2)
            .column(Column::exact(24.0).clip(true))
            .min_scrolled_height(0.0)
            .header(24.0, |mut header| {
                for (column, heading) in HEADINGS.into_iter().enumerate() {
                    let on = sort.is_some_and(|s| s.column == column);
                    let arrow = match sort {
                        Some(Sort { descending, .. }) if on && descending => " ↓",
                        Some(_) if on => " ↑",
                        _ => "",
                    };
                    let text = RichText::new(format!("{heading}{arrow}")).size(11.5).strong().color(if on { ACCENT } else { MUTED });
                    let (_, cell) = header.col(|ui| {
                        read_cell(ui, text, column_align(column)).on_hover_text("Sort by this column; again to turn it round");
                    });
                    // The whole heading sorts, not just the word in it.
                    if cell.clicked() {
                        // The same column again turns it round; a new one
                        // starts the way a column is read, smallest first.
                        sort_by = Some(match sort {
                            Some(Sort { column: was, descending: false }) if was == column => Sort { column, descending: true },
                            Some(Sort { column: was, .. }) if was == column => Sort { column, descending: false },
                            _ => Sort { column, descending: false },
                        });
                    }
                }
                header.col(|_| {});
            })
            .body(|body| {
                body.rows(24.0, lines.len(), |mut row| {
                    match &lines[row.index()] {
                        Line::Group(name) => {
                            // A heading over the measurements it gathers.
                            row.set_overline(true);
                            row.col(|ui| {
                                read_cell(ui, RichText::new(*name).size(12.5).strong().color(ACCENT), Align::Min);
                            });
                            for _ in 1..=HEADINGS.len() {
                                row.col(|_| {});
                            }
                        }
                        Line::Sum(name, totals) => {
                            row.set_overline(true);
                            let shown = total_columns(totals, &units, precision);
                            let sum = |ui: &mut Ui, text: &str, column: usize| {
                                read_cell(ui, RichText::new(text).size(12.0).strong().color(TEXT), column_align(column));
                            };
                            row.col(|ui| sum(ui, name, 0));
                            // Description, kind, page and what it measures.
                            for _ in 0..4 {
                                row.col(|_| {});
                            }
                            for (at, value) in shown[..3].iter().enumerate() {
                                row.col(|ui| sum(ui, value, 5 + at));
                            }
                            // Depths don't add up: two areas a foot deep
                            // aren't two feet deep.
                            row.col(|_| {});
                            let volume = volume_cell(Some(totals.volume_m3).filter(|v| *v > 0.0), &units, precision);
                            row.col(|ui| sum(ui, &volume, 9));
                            row.col(|ui| sum(ui, &shown[3], 10));
                            row.col(|_| {});
                        }
                        Line::Measurement(m) => {
                            row.set_selected(picked == Some(m.id));
                            let shown = columns(m.numbers(), &units, precision);
                            // A click anywhere on the row goes to it on the
                            // page. The cells take their own clicks, so this
                            // gathers them rather than waiting for the row.
                            let mut hit = false;
                            // Cells are text. Double-clicking one opens it for
                            // typing, and it is text again once it's left.
                            row.col(|ui| {
                                if typing(&edit, m.id, Field::Name) {
                                    if let Some(text) = write_cell(ui, edit.as_mut(), false) {
                                        done = Some((m.id, Field::Name, text));
                                    }
                                    return;
                                }
                                let (text, colour) = match m.name.is_empty() {
                                    true => ("Name it", SUBTLE),
                                    false => (m.name.as_str(), TEXT),
                                };
                                let cell = read_cell(ui, RichText::new(text).size(12.0).color(colour), column_align(0));
                                let cell = cell.on_hover_text("Double-click to name this measurement");
                                hit |= cell.clicked();
                                if cell.double_clicked() {
                                    open = Some((m.id, Field::Name, m.name.clone()));
                                }
                            });
                            row.col(|ui| {
                                if typing(&edit, m.id, Field::Description) {
                                    if let Some(text) = write_cell(ui, edit.as_mut(), false) {
                                        done = Some((m.id, Field::Description, text));
                                    }
                                    return;
                                }
                                let (text, colour) = match m.label.is_empty() {
                                    true => ("Describe it", SUBTLE),
                                    false => (m.label.as_str(), TEXT),
                                };
                                let cell = read_cell(ui, RichText::new(text).size(12.0).color(colour), Align::Min);
                                let cell = cell.on_hover_text("Double-click to name this quantity");
                                hit |= cell.clicked();
                                if cell.double_clicked() {
                                    open = Some((m.id, Field::Description, m.label.clone()));
                                }
                            });
                            row.col(|ui| {
                                hit |= read_cell(ui, RichText::new(m.kind.label()).size(12.0).color(TEXT), column_align(2)).clicked();
                            });
                            row.col(|ui| {
                                hit |= read_cell(ui, RichText::new((m.page + 1).to_string()).size(12.0).color(TEXT), column_align(3)).clicked();
                            });
                            row.col(|ui| {
                                let told = if m.numbers().is_some() { TEXT } else { SUBTLE };
                                hit |= read_cell(ui, RichText::new(m.text.as_str()).size(12.0).color(told), column_align(4)).clicked();
                            });
                            for (at, value) in shown[..3].iter().enumerate() {
                                row.col(|ui| {
                                    hit |= read_cell(ui, RichText::new(value.as_str()).size(12.0).color(TEXT), column_align(5 + at)).clicked();
                                });
                            }
                            // An area with a depth against it is a volume, so
                            // the depth is typed here rather than being a tool
                            // of its own.
                            row.col(|ui| {
                                if !m.takes_depth() {
                                    return;
                                }
                                if typing(&edit, m.id, Field::Depth) {
                                    if let Some(text) = write_cell(ui, edit.as_mut(), true) {
                                        done = Some((m.id, Field::Depth, text));
                                    }
                                    return;
                                }
                                let written = m.depth_m.map(|d| format_length(d, units.length, precision));
                                let colour = if written.is_some() { TEXT } else { SUBTLE };
                                let text = RichText::new(written.clone().unwrap_or_else(|| "Depth".to_owned())).size(12.0).color(colour);
                                let cell = read_cell(ui, text, column_align(8));
                                let cell = cell.on_hover_text("Double-click to say how deep it goes, and it's priced by volume");
                                hit |= cell.clicked();
                                if cell.double_clicked() {
                                    open = Some((m.id, Field::Depth, written.unwrap_or_default()));
                                }
                            });
                            let volume = volume_cell(m.numbers().and_then(|q| q.volume_m3), &units, precision);
                            row.col(|ui| {
                                hit |= read_cell(ui, RichText::new(volume).size(12.0).color(TEXT), column_align(9)).clicked();
                            });
                            row.col(|ui| {
                                hit |= read_cell(ui, RichText::new(shown[3].as_str()).size(12.0).color(TEXT), column_align(10)).clicked();
                            });
                            row.col(|ui| {
                                let cross = read_cell(ui, RichText::new("×").size(15.0).color(SUBTLE), Align::Max);
                                if cross.on_hover_text("Delete this measurement").clicked() {
                                    delete = Some(m.id);
                                }
                            });
                            if hit || row.response().clicked() {
                                reveal = Some(m.id);
                            }
                        }
                    }
                });
            });

        self.quantity_edit = edit;
        if let Some(sort) = sort_by {
            self.quantity_sort = Some(sort);
        }
        if let Some((id, field, text)) = open {
            self.quantity_edit = Some(Edit { id, field, text, focused: false });
        }
        if let Some((id, field, text)) = done {
            self.quantity_edit = None;
            match field {
                Field::Name => self.name_measurement(id, text),
                Field::Description => self.describe_measurement(id, text),
                Field::Depth => self.deepen_measurement(id, &text),
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

/// A cell of the table: text and nothing else, cut off at the column's edge
/// rather than widening it, and ranged right when it holds a number so the
/// digits line up down the page. It takes clicks, since that is how a cell is
/// opened for typing and how a row is picked out.
fn read_cell(ui: &mut Ui, text: RichText, align: Align) -> egui::Response {
    let label = egui::Label::new(text).truncate().sense(Sense::click());
    match align {
        // Along the row, not down it, so a cell stays centred in its height
        // the way the table's own layout puts it.
        Align::Center => ui.with_layout(Layout::left_to_right(Align::Center).with_main_align(Align::Center), |ui| ui.add(label)).inner,
        Align::Max => ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.add(label)).inner,
        Align::Min => ui.add(label),
    }
}

/// How a column's cells line up: the description and the kind read as text,
/// so they start at the left; the page and everything measured is centred
/// under its heading; the count and the delete cross keep to the right.
fn column_align(column: usize) -> Align {
    match column {
        0 | 1 | 2 => Align::Min,
        3..=9 => Align::Center,
        _ => Align::Max,
    }
}

/// Whether this cell is the one being typed in.
fn typing(edit: &Option<Edit>, id: MarkupId, field: Field) -> bool {
    edit.as_ref().is_some_and(|e| e.id == id && e.field == field)
}

/// A cell open for typing: the same text with a caret in it, no box drawn
/// around it. Gives back what was typed once it's left, and nothing while it
/// is still being typed or if Escape abandoned it.
fn write_cell(ui: &mut Ui, edit: Option<&mut Edit>, number: bool) -> Option<String> {
    let edit = edit?;
    let align = if number { Align::Max } else { Align::Min };
    let box_ = ui.add(
        TextEdit::singleline(&mut edit.text)
            .frame(Frame::NONE)
            .desired_width(f32::INFINITY)
            .horizontal_align(align)
            .margin(Margin::symmetric(0, 1)),
    );
    // The keyboard goes to it as it opens, and only then, so clicking away
    // can take it back.
    if !edit.focused {
        box_.request_focus();
        edit.focused = true;
    }
    if !box_.lost_focus() {
        return None;
    }
    let abandoned = ui.input(|i| i.key_pressed(Key::Escape));
    (!abandoned).then(|| edit.text.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &str, page: usize, kind: MarkupKind, result: Result<Quantities, QuantityError>) -> Row {
        Row { id: MarkupId(page as u128 + 1), page, kind, depth_m: None, label: label.to_owned(), created_ms: 0, result, text: "188.00 m²".to_owned() }
    }

    /// The cells as the table shows them, for a row of a CSV file.
    fn as_shown(row: &Row) -> ([String; 4], String, String) {
        let (units, precision) = (DisplayUnits::METRIC, Precision::Decimals(2));
        let depth = row.depth_m.map_or(String::new(), |d| format_length(d, units.length, precision));
        let volume = volume_cell(row.numbers().and_then(|q| q.volume_m3), &units, precision);
        (columns(row.numbers(), &units, precision), depth, volume)
    }

    fn area(m2: f64) -> Result<Quantities, QuantityError> {
        Ok(Quantities { area_m2: Some(m2), perimeter_m: Some(56.5), ..Quantities::default() })
    }

    #[test]
    fn a_row_becomes_a_line_of_the_file() {
        let row = row("Slab, ground floor", 4, MarkupKind::Area, area(188.0));
        let (shown, depth, volume) = as_shown(&row);
        let line = csv_line(&row, "Slabs", &shown, &depth, &volume);
        // The page as people count them, the description quoted for its
        // comma, what the table shows, then the numbers in metres.
        assert!(line.starts_with("Slabs,\"Slab, ground floor\",Area,5,188.00 m²,,188.00 m²,56.50 m,,,"), "{line}");
        assert!(line.ends_with(",,188.000000,56.500000,,,,,,"), "the raw numbers, and no note: {line}");
        // One comma of the description's own is inside quotes.
        assert_eq!(line.split(',').count() - 1, CSV_HEADINGS.split(',').count(), "a cell for each heading: {line}");
    }

    #[test]
    fn a_measurement_with_no_number_says_why_and_counts_as_left_out() {
        let no_scale = row("", 0, MarkupKind::Length, Ok(Quantities { uncalibrated: true, ..Quantities::default() }));
        assert_eq!(no_scale.numbers(), None);
        let (shown, depth, volume) = as_shown(&no_scale);
        assert!(shown.iter().all(|s| s.is_empty()) && depth.is_empty() && volume.is_empty(), "no numbers to show");
        assert!(csv_line(&no_scale, "", &shown, &depth, &volume).ends_with("no scale on this page"));

        let crossed = row("", 1, MarkupKind::Area, Err(QuantityError::SelfIntersecting));
        let totals: Totals = [no_scale.result, crossed.result, area(10.0)].iter().collect();
        assert_eq!((totals.included, totals.excluded), (1, 2));
        assert_eq!(totals.area_m2, 10.0);
    }

    #[test]
    fn a_depth_makes_an_area_a_volume_and_only_areas_take_one() {
        let mut slab = row("Slab", 0, MarkupKind::Volume, Ok(Quantities { area_m2: Some(100.0), volume_m3: Some(30.0), ..Quantities::default() }));
        slab.depth_m = Some(0.3);
        let (shown, depth, volume) = as_shown(&slab);
        assert_eq!((depth.as_str(), volume.as_str()), ("0.30 m", "30.00 m³"));
        assert_eq!(shown[1], "100.00 m²", "the plan area is still there to check the volume against");
        assert!(slab.takes_depth());
        // Sorting by the depth column reads the depth, by the volume column
        // the volume.
        assert_eq!((slab.number(7), slab.number(8)), (Some(0.3), Some(30.0)));
        // Nothing else has a depth: a length doesn't become a volume.
        assert!(!row("", 0, MarkupKind::Length, area(1.0)).takes_depth());
        assert!(!row("", 0, MarkupKind::Count, area(1.0)).takes_depth());
    }

    #[test]
    fn empty_cells_sort_to_the_bottom_whichever_way_the_column_runs() {
        use std::cmp::Ordering;
        assert_eq!(compare(Some(1.0), Some(2.0), false), Ordering::Less);
        assert_eq!(compare(Some(1.0), Some(2.0), true), Ordering::Greater);
        for descending in [false, true] {
            assert_eq!(compare(None, Some(2.0), descending), Ordering::Greater, "an empty cell is never the top one");
            assert_eq!(compare(Some(2.0), None, descending), Ordering::Less);
        }
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
