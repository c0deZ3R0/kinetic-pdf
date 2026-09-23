//! The quantities table across the bottom of the window: every measurement
//! in the document, a row each, with the numbers in columns and each group
//! headed by what it comes to.
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

/// How the rows are gathered together. Not chosen: it follows the column the
/// table is sorted by, since sorting by a column is already saying that is
/// what the rows are to be read by. Only the columns that name things gather
/// rows; sorting by a number just orders them.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum GroupBy {
    #[default]
    None,
    Name,
    /// Everything called the same thing, whichever page it's on: the way a
    /// take-off is priced.
    Description,
    /// Everything one person put on the drawing, their notes with their
    /// measurements.
    Author,
    Kind,
    Page,
}

impl GroupBy {
    /// What sorting by column `column` gathers the rows by.
    fn of_column(sort: Option<Sort>) -> GroupBy {
        match sort.map(|s| s.column) {
            Some(0) => GroupBy::Name,
            Some(1) => GroupBy::Description,
            Some(2) => GroupBy::Author,
            Some(3) => GroupBy::Kind,
            Some(4) => GroupBy::Page,
            _ => GroupBy::None,
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

/// Which of a row's typed cells is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Field {
    Name,
    Description,
    Author,
    Depth,
}

/// What a line of the table stands for. Measurements are kept by their own
/// id; a note is a highlight and a drawing is a markup, both of which the
/// session knows by uid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RowId {
    Measure(MarkupId),
    Note(u64),
    /// Something drawn rather than measured: a pen stroke, a box, an ellipse,
    /// a line or an arrow, which the session knows by uid as a note is.
    Drawing(u64),
}

impl RowId {
    /// Where it sorts among rows that are otherwise level: measurements
    /// first, then what was drawn, then notes, and each by its own id.
    fn order(self) -> (u8, u128) {
        match self {
            RowId::Measure(MarkupId(id)) => (0, id),
            RowId::Drawing(uid) => (1, uid as u128),
            RowId::Note(uid) => (2, uid as u128),
        }
    }
}

/// What a row is: something measured, something drawn, or a note written on
/// the page. Only a measurement has quantities; the other two have empty
/// number cells and are left out of the sums.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Measure(MarkupKind),
    Note,
    /// Drawn with one of the drawing tools. Nothing about it is measured --
    /// that is what makes it a drawing rather than a measurement -- so its
    /// number cells stay empty and it is left out of the sums, as a note is.
    /// It is still named, described, filed under a page and grouped with
    /// everything else, since it is part of what was marked up.
    Drawing(crate::model::MarkupKind),
}

impl RowKind {
    fn label(self) -> &'static str {
        match self {
            RowKind::Measure(kind) => kind.label(),
            RowKind::Note => "Note",
            RowKind::Drawing(kind) => kind.label(),
        }
    }

    fn is_note(self) -> bool {
        matches!(self, RowKind::Note)
    }

    /// Whether it has quantities to add up.
    fn is_measured(self) -> bool {
        matches!(self, RowKind::Measure(_))
    }
}

/// A cell open for typing. The text lives here rather than in the measurement
/// while it's being typed: a depth would otherwise be written back out at
/// every keystroke and rewrite itself under the caret.
pub(super) struct Edit {
    id: RowId,
    field: Field,
    text: String,
    /// Whether it has been given the keyboard yet.
    focused: bool,
}

/// The last click on a cell that opens for typing, to pair with the next.
///
/// The table pairs its own clicks rather than asking egui for a double-click,
/// which misses too many. egui gives a double-click 0.3 s, where Windows gives
/// half a second, so a steady double-click reads as two single ones. And it
/// counts a click within 0.6 s of the one before last as a third: the click
/// that picks a row out, followed by a quick double-click on one of its cells,
/// is a triple click to egui and never a double. Here two clicks on the same
/// cell within the system's double-click time open it, whatever came before.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct CellClick {
    id: RowId,
    field: Field,
    at: f64,
}

/// Whether a click on a cell at `now` is the second of a double-click, going
/// by the last click kept in `last`. A second click is used up in opening the
/// cell, so a third starts a new pair rather than opening it again.
fn second_click(last: &mut Option<CellClick>, id: RowId, field: Field, now: f64, within: f64) -> bool {
    let paired = last.is_some_and(|c| c.id == id && c.field == field && now - c.at <= within);
    *last = (!paired).then_some(CellClick { id, field, at: now });
    paired
}

/// How long the second click of a double-click may take, as the system is set:
/// half a second unless it has been changed in the mouse settings.
fn double_click_time() -> f64 {
    #[cfg(windows)]
    {
        // SAFETY: takes nothing and only reads a setting.
        let ms = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
        if ms > 0 {
            return f64::from(ms) / 1000.0;
        }
    }
    0.5
}

/// A line of the table: a heading, a measurement, or a line of sums. The
/// table is one flat run of them, so it draws only the lines in view.
enum Line<'a> {
    /// A heading over the measurements it gathers, carrying their totals:
    /// what is in the group and what it comes to, on the one line. Rolled up,
    /// it stands for them, so the table reads as a summary.
    Group { name: &'a str, heading: String, totals: Totals, rolled: bool },
    Measurement(&'a Row),
}

/// A line of the table, taken from the session as it draws.
struct Row {
    id: RowId,
    page: usize,
    kind: RowKind,
    /// An area's depth, if it has one, which makes it a volume.
    depth_m: Option<f64>,
    /// What the measurement is called, ahead of the description. A note has
    /// no name of its own: what it says is its description.
    name: String,
    /// What it's called: the name the quantity is priced under. A note's own
    /// words go here.
    label: String,
    /// Whose it is.
    author: String,
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
            5 => q.and_then(|q| q.length_m.or(q.area_m2).or(q.angle_deg).or(q.radius_m).or(q.diameter_m).or(q.count.map(|c| c as f64))),
            6 => q.and_then(|q| q.length_m),
            7 => q.and_then(|q| q.area_m2),
            8 => q.and_then(|q| q.perimeter_m),
            9 => self.depth_m,
            10 => q.and_then(|q| q.volume_m3),
            11 => q.and_then(|q| q.count.map(|c| c as f64)),
            _ => None,
        }
    }

    /// Whether a depth belongs on this row: an area priced by volume is an
    /// area with a depth against it.
    fn takes_depth(&self) -> bool {
        matches!(self.kind, RowKind::Measure(MarkupKind::Area | MarkupKind::Volume))
    }

    fn is_note(&self) -> bool {
        self.kind.is_note()
    }

    /// Whether its numbers count towards a group's totals.
    fn is_measured(&self) -> bool {
        self.kind.is_measured()
    }

    /// What the row is filed under, given how the table is grouped.
    fn group(&self, by: GroupBy) -> String {
        match by {
            GroupBy::None => String::new(),
            GroupBy::Name if self.name.is_empty() => "Unnamed".to_owned(),
            GroupBy::Name => self.name.clone(),
            GroupBy::Description if self.label.is_empty() => "No description".to_owned(),
            GroupBy::Description => self.label.clone(),
            GroupBy::Author if self.author.is_empty() => "No name".to_owned(),
            GroupBy::Author => self.author.clone(),
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

const HEADINGS: [&str; 12] =
    ["Name", "Description", "Author", "Kind", "Page", "Measured", "Length", "Area", "Perimeter", "Depth", "Volume", "Count"];

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
    let mut fields = vec![
        cell(group),
        cell(&row.name),
        cell(&row.label),
        cell(&row.author),
        cell(row.kind.label()),
        (row.page + 1).to_string(),
        cell(&row.text),
    ];
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

const CSV_HEADINGS: &str = "Group,Name,Description,Author,Kind,Page,Measured,Length,Area,Perimeter,Depth,Volume,Count,\
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
                    id: RowId::Measure(m.id),
                    page: m.page as usize,
                    kind: RowKind::Measure(m.kind),
                    depth_m: m.extras.depth_m,
                    name: m.meta.name.clone(),
                    label: m.meta.label.clone(),
                    author: m.meta.author.clone(),
                    created_ms: m.meta.created_ms.unwrap_or(0),
                    result: measured.result,
                    text,
                }
            })
            .collect();
        // What was drawn rather than measured is a row too: a box round an
        // area of the drawing, or an arrow pointing at it, is part of the
        // take-off even though it carries no number.
        rows.extend(doc.session.markups().iter().map(|e| Row {
            id: RowId::Drawing(e.uid),
            page: e.markup.page,
            kind: RowKind::Drawing(e.markup.kind),
            depth_m: None,
            name: e.markup.name.clone(),
            label: e.markup.comment.clone(),
            author: e.markup.author.clone(),
            // Markups read in file order, which is the order they sort in
            // within a page already.
            created_ms: 0,
            result: Ok(Quantities::default()),
            text: String::new(),
        }));
        // The notes are rows of the same table: nothing measured, so their
        // number cells stay empty, but they are named, described, filed under
        // a page and grouped with everything else.
        rows.extend(doc.session.highlights().iter().map(|e| Row {
            id: RowId::Note(e.uid),
            page: e.hl.page,
            kind: RowKind::Note,
            depth_m: None,
            name: String::new(),
            label: e.hl.comment.clone(),
            author: e.hl.author.clone(),
            // Highlights read in file order, which is the order they sort in
            // within a page already.
            created_ms: 0,
            result: Ok(Quantities::default()),
            text: e.hl.snippet.clone(),
        }));
        // The store is keyed by ID, so the order it gives is its own. Sorted
        // by a column when one was clicked, and within it by where the
        // measurements are, so rows keep a settled order.
        // Measurements first within a page, then its notes, each in its own
        // settled order, so rows never swap places between frames.
        let taken = |r: &Row| (r.page, r.created_ms, r.id.order());
        rows.sort_by(|a, b| match self.quantity_sort {
            None => taken(a).cmp(&taken(b)),
            Some(Sort { column, descending }) => {
                let text = match column {
                    0 => Some(a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                    1 => Some(a.label.to_lowercase().cmp(&b.label.to_lowercase())),
                    2 => Some(a.author.to_lowercase().cmp(&b.author.to_lowercase())),
                    3 => Some(a.kind.label().cmp(b.kind.label())),
                    4 => Some(a.page.cmp(&b.page)),
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
        let by = GroupBy::of_column(self.quantity_sort);
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
        let scale = self.doc.as_ref().and_then(|doc| doc.sheet_page(self.current_page).and_then(|p| crate::app::scale::page_scale(doc, p)));
        (scale.map_or(Default::default(), |s| s.display), scale.map_or(Default::default(), |s| s.precision))
    }

    /// Scrolls a measurement into view and picks it out, and it alone.
    fn reveal_measurement(&mut self, id: MarkupId) {
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        let Some(bounds) = markup.geometry.bounds() else { return };
        let page = markup.page as usize;
        let box_ = crate::model::PdfBox::spanning([bounds.min.x as f32, bounds.min.y as f32], [bounds.max.x as f32, bounds.max.y as f32]);
        self.scroll_to_box(page, &box_);
        self.active_measure = Some(id);
        self.active = None;
        self.active_vertex = None;
    }

    /// Scrolls a drawn markup into view and picks it out, so the details
    /// panel is about it.
    fn reveal_drawing(&mut self, uid: u64) {
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(entry) = doc.session.markup(uid) else { return };
        let (page, bounds) = (entry.markup.page, entry.markup.bounds);
        self.scroll_to_box(page, &bounds);
        self.active = Some(uid);
        self.active_measure = None;
    }

    /// Sets what a drawn markup is called, or what it says, from the table.
    /// Both go through `Restyle`, which is the one way a markup's own fields
    /// are changed, so either is a single step to undo.
    fn rename_drawing(&mut self, uid: u64, field: Field, text: String) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(entry) = doc.session.markup(uid) else { return };
        let m = &entry.markup;
        let mut look = crate::session::Look {
            color: m.color,
            width: m.width,
            style: m.style.clone(),
            name: m.name.clone(),
            comment: m.comment.clone(),
        };
        match field {
            Field::Name => look.name = text,
            Field::Description => look.comment = text,
            _ => return,
        }
        doc.session.apply(crate::session::Command::Restyle { uid, look: Box::new(look) });
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

    /// Rewrites a note, which is a highlight's own words.
    fn write_note(&mut self, uid: u64, comment: String) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(entry) = doc.session.highlight(uid) else { return };
        let color = entry.hl.color;
        doc.session.apply(crate::session::Command::EditNote { uid, comment, color });
    }

    /// Sets whose a row is, whichever kind of row it is.
    fn attribute_row(&mut self, id: RowId, author: String) {
        let Some(doc) = self.doc.as_mut() else { return };
        match id {
            RowId::Note(uid) | RowId::Drawing(uid) => {
                doc.session.apply(crate::session::Command::SetAuthor { uid, author });
            }
            RowId::Measure(id) => {
                let Some(markup) = doc.session.measures().get(id) else { return };
                if markup.meta.author == author {
                    return;
                }
                let mut signed = markup.clone();
                signed.meta.author = author;
                doc.session.apply(crate::session::Command::ChangeMeasure(Box::new(signed)));
            }
        }
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
            if GroupBy::of_column(self.quantity_sort) != GroupBy::None {
                let totals: Totals = rows.iter().filter(|r| r.is_measured()).map(|r| &r.result).collect();
                let shown = total_columns(&totals, &units, precision);
                // The group, the subtotal in the name column, then blanks as
                // far as the numbers: description, author, kind, page and
                // what was measured.
                let measured = rows.iter().filter(|r| r.is_measured()).count();
                let mut fields = vec![cell(&name), format!("Subtotal ({measured} measurements)")];
                fields.extend(std::iter::repeat_n(String::new(), 5));
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
        // Barely any room above the table: the heading row is the top of the
        // box. There is no title -- the columns say what this is better than
        // the word "Quantities" did -- and no Close, since the button on the
        // status bar that opens the table is the one that shuts it.
        let frame = Frame::NONE.fill(SURFACE).inner_margin(Margin { left: 12, right: 12, top: 4, bottom: 8 });
        // Dragged to any height, up to the whole window under the tool row:
        // a long take-off is read as a table, not through a slot.
        let panel = egui::Panel::bottom("quantities").resizable(true).default_size(260.0).size_range(90.0..=f32::INFINITY);
        panel.frame(frame).show(ui, |ui| {
            self.want_measurements();
            // The table scrolls itself, and keeps its heading row in place
            // while it does.
            self.quantities_table(ui);
        });
    }


    fn quantities_table(&mut self, ui: &mut Ui) {
        // The table is drawn whatever there is in it -- no file open, nothing
        // measured yet, still reading -- so the heading row is always the top
        // of the box and the columns stand where they will stand. An empty
        // table under its own headings says "nothing here yet" by itself,
        // without a sentence saying so.
        let failed = match self.doc.as_ref().map(|d| &d.measurements) {
            Some(MeasureRead::Failed(error)) => Some(error.clone()),
            _ => None,
        };
        // Not an empty state but a fault, so it is still worth words.
        if let Some(error) = failed {
            ui.label(RichText::new(format!("The measurements couldn't be read: {error}")).size(11.5).color(DANGER));
        }
        let rows = if self.doc.is_some() { self.quantity_rows() } else { Vec::new() };
        let groups = self.quantity_groups(rows);
        let (units, precision) = self.quantity_units();
        let grouped = GroupBy::of_column(self.quantity_sort) != GroupBy::None;

        // Whichever rows the page has picked out, measured or written.
        let picked = self.picked_rows();
        let fresh = picked != self.quantity_seen;
        // Something newly picked out on the page is shown even in a group
        // rolled up to its heading: the group is opened to show it.
        let opened = match fresh {
            true => groups.iter().find(|(_, rows)| rows.iter().any(|r| picked.contains(&r.id))).is_some_and(|(name, _)| self.quantity_collapsed.remove(name)),
            false => false,
        };

        // Above the headings, the one thing done to the table as a whole.
        // Rolling up is only for groups, so without them it stands disabled
        // rather than coming and going as the table is sorted.
        let rolled_all = grouped && groups.iter().all(|(name, _)| self.quantity_collapsed.contains(name));
        let mut roll_all = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (label, hover) = match rolled_all {
                true => ("Expand all", "Open every group"),
                false => ("Collapse all", "Roll every group up to its heading and what it comes to"),
            };
            let button = ui.add_enabled_ui(grouped, |ui| slim_button(ui, label, Tone::Ghost, false)).inner;
            let button = button.on_hover_text(hover).on_disabled_hover_text("Sort by Name, Description, Author, Kind or Page to group the rows");
            roll_all = button.clicked();
        });

        // One flat run of lines -- a heading and the measurements under it --
        // so the table can leave the lines out of view undrawn however many
        // there are. A group rolled up is its heading alone.
        let mut lines: Vec<Line> = Vec::new();
        for (name, rows) in &groups {
            let rolled = grouped && self.quantity_collapsed.contains(name);
            if grouped {
                // How many are gathered under it, beside the name.
                let totals: Totals = rows.iter().filter(|r| r.is_measured()).map(|r| &r.result).collect();
                lines.push(Line::Group { name, heading: format!("{name}  ({})", rows.len()), totals, rolled });
            }
            if !rolled {
                lines.extend(rows.iter().map(Line::Measurement));
            }
        }

        let sort = self.quantity_sort;
        // Something newly picked out on the page is brought into view: the
        // first of it, in the order the table reads. Not what was picked out
        // here, which is under the pointer already, and not a row already in
        // view, which the table would only move away from -- unless a group
        // was just opened above it, and what was in view has moved.
        let wanted = match fresh {
            true => lines.iter().position(|line| matches!(line, Line::Measurement(r) if picked.contains(&r.id))),
            false => None,
        };
        let wanted = wanted.filter(|at| opened || !self.quantity_in_view.contains(at));
        let mut roll = None;
        // The lines wholly in view as the table draws, for the next time.
        let mut in_view: Option<std::ops::Range<usize>> = None;
        // The cell being typed in, held out of the app while the table draws
        // so each cell can reach it.
        let mut edit = self.quantity_edit.take();
        // A cell opens on the second of two clicks on it: see `CellClick`.
        let (now, within) = (ui.input(|i| i.time), double_click_time());
        let mut last_click = self.quantity_click.take();
        // A click with Ctrl or Shift picks rows out and opens nothing.
        let modifiers = ui.input(|i| i.modifiers);
        let plain = !modifiers.command && !modifiers.shift;
        let mut pair = |cell: &egui::Response, id: RowId, field: Field| cell.clicked() && plain && second_click(&mut last_click, id, field, now, within);

        let mut reveal = None;
        let mut delete = None;
        let mut open = None;
        let mut done = None;
        let mut sort_by = None;
        let number = || Column::initial(94.0).at_least(56.0).clip(true);
        let table = TableBuilder::new(ui)
            .id_salt("quantities")
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .auto_shrink([false, false])
            .column(Column::initial(150.0).at_least(80.0).clip(true))
            .column(Column::initial(210.0).at_least(90.0).clip(true))
            .column(Column::initial(110.0).at_least(60.0).clip(true))
            .column(Column::initial(90.0).at_least(50.0).clip(true))
            .column(Column::initial(56.0).at_least(40.0).clip(true))
            .column(Column::initial(120.0).at_least(60.0).clip(true))
            .columns(number(), 3)
            .column(Column::initial(88.0).at_least(56.0).clip(true))
            .columns(number(), 2)
            .column(Column::exact(24.0).clip(true))
            .min_scrolled_height(0.0);
        // Into the middle, so what is round it shows too.
        let table = match wanted {
            Some(at) => table.scroll_to_row(at, Some(Align::Center)),
            None => table,
        };
        table
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
                        read_cell(ui, text, column_align(column));
                    });
                    // The whole heading sorts, not just the word in it.
                    let cell = cell.on_hover_text("Sort by this column; again to turn it round");
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
            .body(|mut body| {
                let view = body.ui_mut().clip_rect();
                body.rows(24.0, lines.len(), |mut row| {
                    let at = row.index();
                    match &lines[at] {
                        // The heading and what the group comes to, on one line.
                        Line::Group { name, heading, totals, rolled } => {
                            row.set_overline(true);
                            let shown = total_columns(totals, &units, precision);
                            let sum = |ui: &mut Ui, text: &str, column: usize| {
                                read_cell(ui, RichText::new(text).size(12.0).strong().color(TEXT), column_align(column));
                            };
                            row.col(|ui| {
                                let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                tool_panel::caret(ui.painter(), rect, *rolled);
                                read_cell(ui, RichText::new(heading.as_str()).size(12.5).strong().color(ACCENT), Align::Min);
                            });
                            // Description, author, kind, page and what it
                            // measures.
                            for _ in 0..5 {
                                row.col(|_| {});
                            }
                            for (at, value) in shown[..3].iter().enumerate() {
                                row.col(|ui| sum(ui, value, 6 + at));
                            }
                            // Depths don't add up: two areas a foot deep
                            // aren't two feet deep.
                            row.col(|_| {});
                            let volume = volume_cell(Some(totals.volume_m3).filter(|v| *v > 0.0), &units, precision);
                            row.col(|ui| sum(ui, &volume, 10));
                            row.col(|ui| sum(ui, &shown[3], 11));
                            row.col(|_| {});
                            // Anywhere on the heading rolls the group up, or
                            // opens it again.
                            if row.response().clicked() {
                                roll = Some(*name);
                            }
                        }
                        Line::Measurement(m) => {
                            row.set_selected(picked.contains(&m.id));
                            let shown = columns(m.numbers(), &units, precision);
                            // Cells are text. Double-clicking one anywhere in
                            // it opens it for typing, and it is text again
                            // once it's left. Which cell is open is asked
                            // before the row is drawn, since a cell needs to
                            // know while the edit itself is lent to it.
                            let (naming, describing, authoring, deepening) = (
                                typing(&edit, m.id, Field::Name),
                                typing(&edit, m.id, Field::Description),
                                typing(&edit, m.id, Field::Author),
                                typing(&edit, m.id, Field::Depth),
                            );
                            // A note has no name apart from what it says, so
                            // its name cell is left empty and unopenable.
                            let nameable = !m.is_note();
                            let (_, cell) = row.col(|ui| {
                                if !nameable {
                                    return;
                                }
                                if naming {
                                    if let Some(text) = write_cell(ui, edit.as_mut(), false) {
                                        done = Some((m.id, Field::Name, text));
                                    }
                                    return;
                                }
                                let (text, colour) = match m.name.is_empty() {
                                    true => ("Name it", SUBTLE),
                                    false => (m.name.as_str(), TEXT),
                                };
                                read_cell(ui, RichText::new(text).size(12.0).color(colour), column_align(0));
                            });
                            // Not while it is being typed in: there a
                            // double-click picks a word out of what was
                            // typed, and opening the cell again would throw
                            // it away.
                            if nameable && !naming {
                                let names = if m.is_measured() { "Double-click to name this measurement" } else { "Double-click to name this markup" };
                                let cell = cell.on_hover_text(names);
                                if pair(&cell, m.id, Field::Name) {
                                    open = Some((m.id, Field::Name, m.name.clone()));
                                }
                            }
                            let (_, cell) = row.col(|ui| {
                                if describing {
                                    if let Some(text) = write_cell(ui, edit.as_mut(), false) {
                                        done = Some((m.id, Field::Description, text));
                                    }
                                    return;
                                }
                                let empty = if m.is_note() { "Write the note" } else { "Describe it" };
                                let (text, colour) = match m.label.is_empty() {
                                    true => (empty, SUBTLE),
                                    false => (m.label.as_str(), TEXT),
                                };
                                read_cell(ui, RichText::new(text).size(12.0).color(colour), Align::Min);
                            });
                            if !describing {
                                let hover = match m.kind {
                                    RowKind::Note => "Double-click to write this note",
                                    RowKind::Drawing(_) => "Double-click to say what this markup is",
                                    RowKind::Measure(_) => "Double-click to name this quantity",
                                };
                                let cell = cell.on_hover_text(hover);
                                if pair(&cell, m.id, Field::Description) {
                                    open = Some((m.id, Field::Description, m.label.clone()));
                                }
                            }
                            let (_, cell) = row.col(|ui| {
                                if authoring {
                                    if let Some(text) = write_cell(ui, edit.as_mut(), false) {
                                        done = Some((m.id, Field::Author, text));
                                    }
                                    return;
                                }
                                let (text, colour) = match m.author.is_empty() {
                                    true => ("Nobody", SUBTLE),
                                    false => (m.author.as_str(), TEXT),
                                };
                                read_cell(ui, RichText::new(text).size(12.0).color(colour), column_align(2));
                            });
                            if !authoring {
                                let cell = cell.on_hover_text("Double-click to say whose this is");
                                if pair(&cell, m.id, Field::Author) {
                                    open = Some((m.id, Field::Author, m.author.clone()));
                                }
                            }
                            row.col(|ui| {
                                read_cell(ui, RichText::new(m.kind.label()).size(12.0).color(TEXT), column_align(3));
                            });
                            row.col(|ui| {
                                read_cell(ui, RichText::new((m.page + 1).to_string()).size(12.0).color(TEXT), column_align(4));
                            });
                            row.col(|ui| {
                                let told = if m.is_note() || m.numbers().is_some() { TEXT } else { SUBTLE };
                                read_cell(ui, RichText::new(m.text.as_str()).size(12.0).color(told), column_align(5));
                            });
                            for (at, value) in shown[..3].iter().enumerate() {
                                row.col(|ui| {
                                    read_cell(ui, RichText::new(value.as_str()).size(12.0).color(TEXT), column_align(6 + at));
                                });
                            }
                            // An area with a depth against it is a volume, so
                            // the depth is typed here rather than being a tool
                            // of its own.
                            let written = m.depth_m.map(|d| format_length(d, units.length, precision));
                            let (_, cell) = row.col(|ui| {
                                if !m.takes_depth() {
                                    return;
                                }
                                if deepening {
                                    if let Some(text) = write_cell(ui, edit.as_mut(), true) {
                                        done = Some((m.id, Field::Depth, text));
                                    }
                                    return;
                                }
                                let colour = if written.is_some() { TEXT } else { SUBTLE };
                                let text = RichText::new(written.clone().unwrap_or_else(|| "Depth".to_owned())).size(12.0).color(colour);
                                read_cell(ui, text, column_align(9));
                            });
                            if m.takes_depth() && !deepening {
                                let cell = cell.on_hover_text("Double-click to say how deep it goes, and it's priced by volume");
                                if pair(&cell, m.id, Field::Depth) {
                                    open = Some((m.id, Field::Depth, written.unwrap_or_default()));
                                }
                            }
                            let volume = volume_cell(m.numbers().and_then(|q| q.volume_m3), &units, precision);
                            row.col(|ui| {
                                read_cell(ui, RichText::new(volume).size(12.0).color(TEXT), column_align(10));
                            });
                            row.col(|ui| {
                                read_cell(ui, RichText::new(shown[3].as_str()).size(12.0).color(TEXT), column_align(11));
                            });
                            let removes = match m.kind {
                                RowKind::Note => "Delete this note",
                                RowKind::Drawing(_) => "Delete this markup",
                                RowKind::Measure(_) => "Delete this measurement",
                            };
                            row.col(|ui| {
                                if cross_cell(ui).on_hover_text(removes).clicked() {
                                    delete = Some(m.id);
                                }
                            });
                            // A click anywhere on the row goes to it on the
                            // page. The row's response is its cells' together,
                            // and now they take the clicks rather than the
                            // words in them, it catches the gaps as well.
                            if row.response().clicked() {
                                reveal = Some(m.id);
                            }
                        }
                    }
                    if view.contains_rect(row.response().rect) {
                        in_view = Some(in_view.take().map_or(at..at + 1, |seen| seen.start.min(at)..at + 1));
                    }
                });
            });

        self.quantity_in_view = in_view.unwrap_or_default();
        self.quantity_edit = edit;
        self.quantity_click = last_click;
        if let Some(name) = roll {
            if !self.quantity_collapsed.remove(name) {
                self.quantity_collapsed.insert(name.to_owned());
            }
        }
        if roll_all {
            match rolled_all {
                true => self.quantity_collapsed.clear(),
                false => self.quantity_collapsed = groups.iter().map(|(name, _)| name.clone()).collect(),
            }
        }
        if let Some(sort) = sort_by {
            // Gathered another way, the groups are new ones, and start open.
            // Turning the same column round keeps what was rolled up.
            if GroupBy::of_column(Some(sort)) != GroupBy::of_column(self.quantity_sort) {
                self.quantity_collapsed.clear();
            }
            self.quantity_sort = Some(sort);
        }
        // What was being typed is put away before the next cell opens: a
        // double-click straight from one cell to another leaves the first one
        // and opens the second, rather than the first's leaving cancelling the
        // second on the way out.
        if let Some((id, field, text)) = done {
            self.quantity_edit = None;
            match (id, field) {
                (RowId::Measure(id), Field::Name) => self.name_measurement(id, text),
                (RowId::Measure(id), Field::Description) => self.describe_measurement(id, text),
                (RowId::Measure(id), Field::Depth) => self.deepen_measurement(id, &text),
                (RowId::Note(uid), Field::Description) => self.write_note(uid, text),
                (RowId::Drawing(uid), Field::Name | Field::Description) => self.rename_drawing(uid, field, text),
                (id, Field::Author) => self.attribute_row(id, text),
                // A note has no name of its own, and nothing but an area has
                // a depth.
                (RowId::Note(_), Field::Name | Field::Depth) | (RowId::Drawing(_), Field::Depth) => {}
            }
        }
        if let Some((id, field, text)) = open {
            self.quantity_edit = Some(Edit { id, field, text, focused: false });
        }
        if let Some(id) = delete {
            match id {
                RowId::Measure(id) => {
                    if let Some(doc) = self.doc.as_mut() {
                        doc.session.apply(crate::session::Command::RemoveMeasure(id));
                    }
                    if self.active_measure == Some(id) {
                        self.active_measure = None;
                    }
                }
                RowId::Note(uid) | RowId::Drawing(uid) => self.remove(uid),
            }
        } else if let Some(id) = reveal {
            // Ctrl adds a row to what is picked out or takes it out again,
            // Shift picks out the run of rows from the last one clicked; the
            // page stays where it is for both, since there is more than one
            // thing to go to. A plain click picks the row alone and goes to it.
            if modifiers.shift {
                let rows: Vec<RowId> = lines.iter().filter_map(|line| if let Line::Measurement(r) = line { Some(r.id) } else { None }).collect();
                self.pick_range(&rows, id, modifiers.command);
            } else if modifiers.command {
                self.pick_toggle(id);
            } else {
                self.pick_only(id);
                match id {
                    RowId::Measure(id) => self.reveal_measurement(id),
                    // The same as clicking it in the old notes panel: the page
                    // scrolls to it and its note opens.
                    RowId::Note(uid) => {
                        self.active_measure = None;
                        self.reveal(uid);
                    }
                    RowId::Drawing(uid) => self.reveal_drawing(uid),
                }
            }
        }
        // Whatever is picked out now has been seen, whether it was picked out
        // here or was already, so only a change made elsewhere moves the table.
        self.quantity_seen = self.picked_rows();
    }

    /// Writes the table to a file the user picks.
    pub(super) fn export_quantities(&mut self) {
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
/// digits line up down the page.
///
/// The text takes no clicks of its own, and so gives nothing back. Whatever a
/// cell is for -- opening it for typing, or picking its row out -- hangs on the
/// cell's own response from `col`, which covers the whole column width. A
/// widget within a cell is nearer the pointer than the cell itself, so an
/// interactive label here would leave the rest of the cell dead and only the
/// words worth aiming at.
fn read_cell(ui: &mut Ui, text: RichText, align: Align) {
    let label = egui::Label::new(text).truncate().selectable(false);
    match align {
        // Along the row, not down it, so a cell stays centred in its height
        // the way the table's own layout puts it.
        Align::Center => {
            ui.with_layout(Layout::left_to_right(Align::Center).with_main_align(Align::Center), |ui| ui.add(label));
        }
        Align::Max => {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.add(label));
        }
        Align::Min => {
            ui.add(label);
        }
    }
}

/// The delete cross: the one thing here with a target of its own rather than
/// the whole cell, since a measurement deleted by a stray click costs more
/// than having to aim at the cross.
fn cross_cell(ui: &mut Ui) -> egui::Response {
    let cross = egui::Label::new(RichText::new("×").size(15.0).color(SUBTLE)).selectable(false).sense(Sense::click());
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.add(cross)).inner
}

/// How a column's cells line up: the name, description, author and kind read
/// so they start at the left; the page and everything measured is centred
/// under its heading; the count and the delete cross keep to the right.
fn column_align(column: usize) -> Align {
    match column {
        0..=3 => Align::Min,
        4..=10 => Align::Center,
        _ => Align::Max,
    }
}

/// Whether this cell is the one being typed in.
fn typing(edit: &Option<Edit>, id: RowId, field: Field) -> bool {
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
pub(super) mod tests {
    use super::*;

    fn row(label: &str, page: usize, kind: MarkupKind, result: Result<Quantities, QuantityError>) -> Row {
        Row {
            id: RowId::Measure(MarkupId(page as u128 + 1)),
            page,
            kind: RowKind::Measure(kind),
            depth_m: None,
            name: String::new(),
            label: label.to_owned(),
            author: String::new(),
            created_ms: 0,
            result,
            text: "188.00 m²".to_owned(),
        }
    }

    /// The cells as the table shows them, for a row of a CSV file.
    fn as_shown(row: &Row) -> ([String; 4], String, String) {
        let (units, precision) = (DisplayUnits::METRIC, Precision::Decimals(2));
        let depth = row.depth_m.map_or(String::new(), |d| format_length(d, units.length, precision));
        let volume = volume_cell(row.numbers().and_then(|q| q.volume_m3), &units, precision);
        (columns(row.numbers(), &units, precision), depth, volume)
    }

    /// A drawn markup's row: named and described like any other, with nothing
    /// measured.
    fn drawn(name: &str, kind: crate::model::MarkupKind) -> Row {
        Row {
            id: RowId::Drawing(7),
            page: 3,
            kind: RowKind::Drawing(kind),
            depth_m: None,
            name: name.to_owned(),
            label: "site boundary".to_owned(),
            author: String::new(),
            created_ms: 0,
            result: Ok(Quantities::default()),
            text: String::new(),
        }
    }

    /// What was drawn is in the list, under its own name, with its number
    /// cells empty -- and it is left out of what the group comes to, so a box
    /// round an area doesn't add itself to the area measured.
    #[test]
    fn a_drawing_is_listed_but_adds_nothing_up() {
        let box_ = drawn("Hoarding", crate::model::MarkupKind::Rectangle);
        assert_eq!(box_.kind.label(), "Rectangle");
        assert!(!box_.is_measured(), "nothing about it is measured");
        assert!(!box_.is_note(), "and it is not a note either, so it can be named");
        let (shown, depth, volume) = as_shown(&box_);
        assert!(shown.iter().all(|cell| cell.is_empty()), "no numbers: {shown:?}");
        assert!(depth.is_empty() && volume.is_empty());

        let rows = vec![row("Slab", 3, MarkupKind::Area, area(188.0)), box_];
        let totals: Totals = rows.iter().filter(|r| r.is_measured()).map(|r| &r.result).collect();
        assert_eq!(totals.area_m2, 188.0, "the box adds nothing to the area");
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
        // The group, an empty Name, the description quoted for its comma,
        // then an empty Author.
        assert!(line.starts_with("Slabs,,\"Slab, ground floor\",,Area,5,188.00 m²,,188.00 m²,56.50 m,,,"), "{line}");
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
        assert_eq!((slab.number(9), slab.number(10)), (Some(0.3), Some(30.0)));
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

    /// Two clicks on the same cell in time are a double-click; on another
    /// cell, or too late, the second starts a pair of its own; and the one
    /// that opens the cell is used up, so a third click doesn't open it again.
    #[test]
    fn clicks_pair_on_one_cell_within_the_double_click_time() {
        let (a, b) = (RowId::Measure(MarkupId(1)), RowId::Note(2));
        let mut last = None;
        assert!(!second_click(&mut last, a, Field::Name, 1.0, 0.5), "a first click opens nothing");
        assert!(second_click(&mut last, a, Field::Name, 1.45, 0.5), "a second in time, on the same cell");
        assert!(!second_click(&mut last, a, Field::Name, 1.6, 0.5), "the pair was used up");
        assert!(!second_click(&mut last, a, Field::Description, 1.7, 0.5), "another cell of the row");
        assert!(!second_click(&mut last, b, Field::Description, 1.8, 0.5), "the same column of another row");
        assert!(!second_click(&mut last, b, Field::Description, 2.4, 0.5), "too slow");
        assert!(second_click(&mut last, b, Field::Description, 2.5, 0.5));
    }

    /// An app with a document open and `n` lengths measured on its first
    /// page, and nothing running behind it, so the table can be drawn and
    /// clicked the way a window would.
    pub(in crate::app) struct Table {
        pub app: App,
        pub ctx: egui::Context,
        pub time: f64,
        _requests: std::sync::mpsc::Receiver<Request>,
    }

    impl Table {
        pub fn new(n: usize) -> Table {
            // Long enough to be cut off at the column's edge, which is when
            // the words carry a tooltip of their own.
            let names: Vec<String> = (0..n).map(|i| format!("Wall {i}, the long run down the east side of the building")).collect();
            Table::named(&names.iter().map(String::as_str).collect::<Vec<_>>())
        }

        /// The same, with a length for each name given.
        pub fn named(names: &[&str]) -> Table {
            std::env::set_var("KINETIC_PDF_CACHE", "0");
            std::env::set_var("KINETIC_PDF_HELPERS", "0");
            std::env::set_var("KINETIC_PDF_UPDATE", "0");
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = App::new(&cc, None);
            let (replies, rx) = std::sync::mpsc::channel();
            let (tx, requests) = std::sync::mpsc::channel();
            app.rx = rx;
            app.tx = tx;
            app.generation = 1;
            replies
                .send(Reply::Opened {
                    generation: 1, path: PathBuf::from("quantities-test.pdf"), file: 1,
                    page_sizes: vec![[600.0, 800.0]; 2], page_labels: vec![None; 2],
                })
                .unwrap();
            app.drain_replies(&ctx);
            let doc = app.doc.as_mut().unwrap();
            doc.measurements = MeasureRead::Ready;
            let lengths = names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let y = 700.0 - i as f64 * 5.0;
                    let line = markup_model::Geometry::Line { a: markup_model::Pt::new(50.0, y), b: markup_model::Pt::new(150.0, y) };
                    let mut m = markup_model::Markup::new(0, MarkupKind::Length, line);
                    m.meta.name = (*name).to_owned();
                    m.meta.created_ms = Some(i as i64);
                    m
                })
                .collect();
            doc.session.load_measures(lengths);
            let mut table = Table { app, ctx, time: 1.0, _requests: requests };
            // Laid out, so there is something under the pointer: the first
            // frame only sizes the columns.
            table.frame(0.0, Vec::new());
            table.frame(0.016, Vec::new());
            table
        }

        /// One frame of the table alone, `dt` seconds after the last, with
        /// these events.
        pub fn frame(&mut self, dt: f64, events: Vec<egui::Event>) {
            self.time += dt;
            let raw = egui::RawInput {
                time: Some(self.time),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 600.0))),
                events,
                ..Default::default()
            };
            let app = &mut self.app;
            // As the window does before anything is drawn.
            app.settle_picked();
            let mut output = self.ctx.run_ui(raw, |ui| app.quantities_table(ui));
            output.textures_delta.clear();
        }

        /// The middle of line `line` of the table, counting from the first
        /// under the headings, at `x` across it.
        pub fn at(&self, line: usize, x: f32) -> Pos2 {
            let pitch = 24.0 + self.ctx.global_style().spacing.item_spacing.y;
            pos2(x, self.heading(0).y + 12.0 + pitch * line as f32 + pitch / 2.0)
        }

        /// The middle of the toolbar's one button.
        pub fn toolbar(&self) -> Pos2 {
            pos2(30.0, SLIM_HEIGHT / 2.0)
        }

        /// The middle of the heading of column `column`, at the columns'
        /// starting widths.
        pub fn heading(&self, column: usize) -> Pos2 {
            let widths = [150.0, 210.0, 110.0, 90.0, 56.0];
            let spacing = self.ctx.global_style().spacing.item_spacing;
            let left: f32 = widths[..column].iter().map(|w| w + spacing.x).sum();
            pos2(left + 20.0, SLIM_HEIGHT + spacing.y + 12.0)
        }

        /// A click `after` seconds from the last frame, pressed and let go a
        /// frame apart, and the frame egui asks for after it.
        pub fn click(&mut self, at: Pos2, after: f64, modifiers: egui::Modifiers) {
            let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers };
            self.frame(after, vec![egui::Event::ModifiersChanged(modifiers), egui::Event::PointerMoved(at), button(true)]);
            self.frame(0.03, vec![button(false)]);
            self.frame(0.016, vec![egui::Event::ModifiersChanged(Default::default())]);
        }

        /// Two clicks, the second `gap` seconds after the first.
        pub fn double_click(&mut self, at: Pos2, gap: f64) {
            self.click(at, 0.05, Default::default());
            self.click(at, gap, Default::default());
        }

        pub fn editing(&self) -> Option<(RowId, Field)> {
            self.app.quantity_edit.as_ref().map(|e| (e.id, e.field))
        }

        /// The row of the `i`th measurement, in the order the table has them.
        pub fn measure(&self, i: usize) -> RowId {
            self.app.quantity_rows()[i].id
        }
    }

    /// Double-clicking a cell opens it wherever in the cell the pointer is:
    /// on its words, cut off as they are, or on the blank beside them.
    #[test]
    fn a_double_click_on_the_words_or_beside_them_opens_the_cell() {
        for x in [20.0, 140.0] {
            let mut table = Table::new(3);
            // Hovered long enough for the tooltips to be up.
            for _ in 0..20 {
                table.frame(0.05, vec![egui::Event::PointerMoved(table.at(1, x))]);
            }
            table.double_click(table.at(1, x), 0.1);
            assert_eq!(table.editing(), Some((table.measure(1), Field::Name)), "at {x}");
        }
    }

    /// A double-click at the pace the system allows, slower than egui's own.
    #[test]
    fn an_unhurried_double_click_opens_the_cell() {
        let mut table = Table::new(3);
        table.double_click(table.at(0, 20.0), double_click_time() * 0.8);
        assert_eq!(table.editing(), Some((table.measure(0), Field::Name)));
    }

    /// Clicking a row picks it out; double-clicking one of its cells straight
    /// after is three clicks in quick succession, which still opens it.
    #[test]
    fn a_double_click_just_after_picking_the_row_out_opens_the_cell() {
        let mut table = Table::new(3);
        let at = table.at(2, 20.0);
        table.click(at, 0.5, Default::default());
        assert_eq!(table.editing(), None, "one click picks the row out");
        assert_eq!(table.app.active_measure.map(RowId::Measure), Some(table.measure(2)));
        // A triple click to egui, which gives a double-click 0.3 s and a
        // triple 0.6 s.
        table.click(at, 0.4, Default::default());
        table.click(at, 0.15, Default::default());
        assert_eq!(table.editing(), Some((table.measure(2), Field::Name)));
    }

    /// Two clicks too far apart in time, or on two different cells, are two
    /// clicks and open nothing.
    #[test]
    fn two_separate_clicks_open_nothing() {
        let mut table = Table::new(3);
        table.double_click(table.at(0, 20.0), double_click_time() + 0.2);
        assert_eq!(table.editing(), None, "too slow");
        table.click(table.at(0, 20.0), 1.0, Default::default());
        table.click(table.at(1, 20.0), 0.1, Default::default());
        assert_eq!(table.editing(), None, "two rows");
    }

    impl Table {
        /// Picks out the `i`th measurement the way clicking it on the page
        /// does, and gives the table time to settle.
        fn pick_on_page(&mut self, i: usize) {
            let RowId::Measure(id) = self.measure(i) else { unreachable!("only measurements here") };
            self.app.active_measure = Some(id);
            self.settle();
        }

        /// Frames enough for a scroll to finish.
        fn settle(&mut self) {
            for _ in 0..30 {
                self.frame(0.05, Vec::new());
            }
        }
    }

    /// Something picked out on the page far down the table is scrolled into
    /// view in it, and picked out there too.
    #[test]
    fn picking_on_the_page_scrolls_the_table_to_the_row() {
        let mut table = Table::new(60);
        let shown = table.app.quantity_in_view.clone();
        assert!(shown.start == 0 && shown.len() > 5 && shown.end < 45, "the top of the table: {shown:?}");
        table.pick_on_page(45);
        assert!(table.app.quantity_in_view.contains(&45), "{:?}", table.app.quantity_in_view);
        assert_eq!(table.app.picked_rows(), vec![table.measure(45)]);
        // And back up again.
        table.pick_on_page(2);
        assert!(table.app.quantity_in_view.contains(&2), "{:?}", table.app.quantity_in_view);
    }

    /// A row already in view is left where it is.
    #[test]
    fn a_row_in_view_does_not_move_the_table() {
        let mut table = Table::new(60);
        let shown = table.app.quantity_in_view.clone();
        table.pick_on_page(shown.end - 1);
        assert_eq!(table.app.quantity_in_view, shown);
    }

    /// Picking a row in the table goes to it on the page, and the table stays
    /// where it is under the pointer -- even for a row it has only half in
    /// view, which a pick on the page would bring right in.
    #[test]
    fn picking_in_the_table_does_not_scroll_it() {
        let mut table = Table::new(60);
        let shown = table.app.quantity_in_view.clone();
        // The top of the line cut off at the bottom of the table.
        table.click(table.at(shown.end, 20.0) - vec2(0.0, 10.0), 0.5, Default::default());
        table.settle();
        assert_eq!(table.app.picked_rows(), vec![table.measure(shown.end)]);
        assert_eq!(table.app.quantity_in_view, shown);
    }

    /// Three groups when sorted by name: Doors (2), Slab (1), Walls (3), nine
    /// lines with their headings.
    fn grouped() -> Table {
        let mut table = Table::named(&["Walls", "Doors", "Walls", "Slab", "Doors", "Walls"]);
        table.click(table.heading(0), 0.5, Default::default());
        table.settle();
        assert_eq!(table.app.quantity_in_view, 0..9, "sorted by name, and every line in view");
        table
    }

    fn rolled(table: &Table) -> Vec<&str> {
        let mut names: Vec<&str> = table.app.quantity_collapsed.iter().map(String::as_str).collect();
        names.sort();
        names
    }

    /// A click on a group's heading rolls it up to the heading alone, and
    /// another opens it again.
    #[test]
    fn a_group_rolls_up_to_its_heading_and_opens_again() {
        let mut table = grouped();
        // Slab's heading, under the two doors.
        table.click(table.at(3, 20.0), 0.5, Default::default());
        table.settle();
        assert_eq!(rolled(&table), ["Slab"]);
        assert_eq!(table.app.quantity_in_view, 0..8, "one line fewer");
        // It still reads as what it gathers and what that comes to.
        assert!(table.app.picked_rows().is_empty(), "rolling a group up picks nothing out");
        table.click(table.at(3, 20.0), 0.5, Default::default());
        table.settle();
        assert!(rolled(&table).is_empty());
        assert_eq!(table.app.quantity_in_view, 0..9);
    }

    /// The toolbar rolls every group up, and then opens them all again.
    #[test]
    fn collapse_all_then_expand_all() {
        let mut table = grouped();
        table.click(table.toolbar(), 0.5, Default::default());
        table.settle();
        assert_eq!(rolled(&table), ["Doors", "Slab", "Walls"]);
        assert_eq!(table.app.quantity_in_view, 0..3, "the headings alone");
        // The file still has every row: rolling up is only how it is shown.
        assert_eq!(table.app.quantities_csv().lines().skip(1).filter(|l| l.contains(",Length,")).count(), 6);
        table.click(table.toolbar(), 0.5, Default::default());
        table.settle();
        assert!(rolled(&table).is_empty());
        assert_eq!(table.app.quantity_in_view, 0..9);
        // With one group left open, the button rolls up the rest.
        table.click(table.at(0, 20.0), 0.5, Default::default());
        table.click(table.toolbar(), 0.5, Default::default());
        table.settle();
        assert_eq!(rolled(&table), ["Doors", "Slab", "Walls"]);
    }

    /// Without groups there is nothing to roll up, and the button does nothing.
    #[test]
    fn the_toolbar_does_nothing_without_groups() {
        let mut table = Table::named(&["Walls", "Doors", "Walls"]);
        table.click(table.toolbar(), 0.5, Default::default());
        table.settle();
        assert!(rolled(&table).is_empty());
        assert_eq!(table.app.quantity_in_view, 0..3);
    }

    /// Turning the column round keeps the groups rolled up as they were;
    /// sorting by another column gathers new groups, which start open.
    #[test]
    fn grouping_another_way_starts_with_every_group_open() {
        let mut table = grouped();
        table.click(table.toolbar(), 0.5, Default::default());
        table.click(table.heading(0), 0.5, Default::default());
        table.settle();
        assert_eq!(rolled(&table), ["Doors", "Slab", "Walls"], "the same groups, the other way round");
        // By kind: every one of them a length.
        table.click(table.heading(3), 0.5, Default::default());
        table.settle();
        assert!(rolled(&table).is_empty());
        assert_eq!(table.app.quantity_in_view, 0..7);
    }

    /// Something picked out on the page inside a rolled-up group opens that
    /// group, and only that one, to show its row.
    #[test]
    fn picking_on_the_page_opens_the_group_its_row_is_in() {
        let mut table = grouped();
        table.click(table.toolbar(), 0.5, Default::default());
        table.settle();
        // The first wall, after the doors and the slab.
        table.pick_on_page(3);
        assert_eq!(rolled(&table), ["Doors", "Slab"]);
        assert_eq!(table.app.quantity_in_view, 0..6);
    }

    const CTRL: egui::Modifiers = egui::Modifiers::COMMAND;
    const SHIFT: egui::Modifiers = egui::Modifiers::SHIFT;

    impl Table {
        fn rows(&self, at: &[usize]) -> Vec<RowId> {
            at.iter().map(|&i| self.measure(i)).collect()
        }
    }

    /// Ctrl-click adds a row to what is picked out, and takes it out again;
    /// the first one picked out stays the one the page has.
    #[test]
    fn ctrl_click_adds_a_row_and_takes_it_out_again() {
        let mut table = Table::new(8);
        table.click(table.at(1, 300.0), 0.5, Default::default());
        table.click(table.at(4, 300.0), 0.5, CTRL);
        table.click(table.at(6, 300.0), 0.5, CTRL);
        assert_eq!(table.app.picked_rows(), table.rows(&[1, 4, 6]));
        assert_eq!(table.app.active_measure.map(RowId::Measure), Some(table.measure(1)));
        table.click(table.at(4, 300.0), 0.5, CTRL);
        assert_eq!(table.app.picked_rows(), table.rows(&[1, 6]));
        // Taking out the one the page has hands it to the next.
        table.click(table.at(1, 300.0), 0.5, CTRL);
        assert_eq!(table.app.picked_rows(), table.rows(&[6]));
        assert_eq!(table.app.active_measure.map(RowId::Measure), Some(table.measure(6)));
        // A plain click is that row alone again.
        table.click(table.at(2, 300.0), 0.5, CTRL);
        table.click(table.at(3, 300.0), 0.5, Default::default());
        assert_eq!(table.app.picked_rows(), table.rows(&[3]));
    }

    /// Shift-click picks out the run of rows from the last one clicked, which
    /// goes first; with Ctrl as well it adds the run to what is there.
    #[test]
    fn shift_click_picks_out_a_run_of_rows() {
        let mut table = Table::new(10);
        table.click(table.at(5, 300.0), 0.5, Default::default());
        table.click(table.at(2, 300.0), 0.5, SHIFT);
        assert_eq!(table.app.picked_rows(), table.rows(&[5, 2, 3, 4]));
        // From the same row again, not from the end of the run.
        table.click(table.at(7, 300.0), 0.5, SHIFT);
        assert_eq!(table.app.picked_rows(), table.rows(&[5, 6, 7]));
        table.click(table.at(0, 300.0), 0.5, CTRL);
        table.click(table.at(9, 300.0), 0.5, CTRL | SHIFT);
        assert_eq!(table.app.picked_rows(), table.rows(&[5, 6, 7, 0, 1, 2, 3, 4, 8, 9]));
        assert_eq!(table.editing(), None, "none of that opened a cell");
    }

    /// A double-click with Ctrl or Shift held picks rows out and opens nothing.
    #[test]
    fn a_modified_double_click_opens_nothing() {
        let mut table = Table::new(4);
        table.click(table.at(1, 20.0), 0.5, CTRL);
        table.click(table.at(1, 20.0), 0.1, CTRL);
        table.click(table.at(2, 20.0), 0.5, SHIFT);
        table.click(table.at(2, 20.0), 0.1, SHIFT);
        assert_eq!(table.editing(), None);
    }

    /// The rest picked out in the table go once the page picks out something
    /// else, or nothing, and don't come back with the one they were with.
    #[test]
    fn picking_on_the_page_lets_the_rest_go() {
        let mut table = Table::new(6);
        table.click(table.at(0, 300.0), 0.5, Default::default());
        table.click(table.at(3, 300.0), 0.5, CTRL);
        assert_eq!(table.app.picked_rows().len(), 2);
        table.app.active_measure = None;
        table.settle();
        assert!(table.app.picked_rows().is_empty());
        table.pick_on_page(0);
        assert_eq!(table.app.picked_rows(), table.rows(&[0]));
        // And something else picked out on the page is that alone.
        table.click(table.at(3, 300.0), 0.5, CTRL);
        table.pick_on_page(4);
        assert_eq!(table.app.picked_rows(), table.rows(&[4]));
    }

    /// Several rows picked out in the table scroll nothing, and a later pick
    /// on the page of several goes to the first of them.
    #[test]
    fn picking_several_in_the_table_leaves_it_where_it_is() {
        let mut table = Table::new(60);
        let shown = table.app.quantity_in_view.clone();
        table.click(table.at(2, 300.0), 0.5, Default::default());
        table.click(table.at(shown.end, 300.0) - vec2(0.0, 10.0), 0.5, SHIFT);
        table.settle();
        assert_eq!(table.app.picked_rows().len(), shown.end - 1);
        assert_eq!(table.app.quantity_in_view, shown);
    }
}
