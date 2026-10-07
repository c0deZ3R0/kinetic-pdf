//! Application actions shared by menus, shortcuts and automation.

use super::*;
use super::tools::MEASURE_TOOLS;

/// Which part of the app an action belongs to. Shown down the right of each
/// row, and searched along with the name, so "tool area" finds the area tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Group {
    File,
    View,
    Go,
    Edit,
    Panels,
    Tools,
}

impl Group {
    pub(super) fn label(self) -> &'static str {
        match self {
            Group::File => "File",
            Group::View => "View",
            Group::Go => "Go",
            Group::Edit => "Edit",
            Group::Panels => "Panels",
            Group::Tools => "Tools",
        }
    }
}

/// One thing the app can be asked to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Action {
    NewPdf,
    Open,
    Save,
    SaveAs,
    Print,
    ExportCsv,
    Quit,
    ZoomIn,
    ZoomOut,
    FitWidth,
    FitPage,
    /// Show oversized sheets at the usual page width, or at actual size.
    ShrinkWide,
    SideBySide,
    GoToPage,
    FirstPage,
    LastPage,
    NextPage,
    PreviousPage,
    Find,
    FindNext,
    FindPrevious,
    Undo,
    Redo,
    /// Pick out everything on the page in view, with the Select tool.
    PickAll,
    /// Delete everything picked out, as one step to undo.
    DeletePicked,
    /// The four sides of the panel on the rail, each shown by name.
    Details,
    KeptTools,
    Scale,
    Quantities,
    About,
    Settings,
    /// The release notes of every version up to this one.
    WhatsNew,
    /// Put every tool down, which leaves the Select tool in hand.
    Select,
    Pin,
    Pins,
    /// Take up Clip, Cut or Erase, which take an area of the page.
    Area(clip::AreaTool),
    /// Put down what's been copied or clipped, under the pointer.
    Paste,
    /// Put it down where it was on the sheet it came from.
    PasteInPlace,
    /// Take up the highlighter, which picks out text to highlight.
    Highlighter,
    /// Open Kinetic Compare, or close it while it's open.
    Compare,
    /// Take up a text box tool: `true` for one with an arrow.
    Text(bool),
    Draw(MarkupKind),
    Measure(MeasureTool),
}

impl Action {
    /// Every action, in the order the palette shows them when nothing has
    /// been typed. The tools come last: they are the longest run, and the
    /// ones a user is most likely to reach for by name rather than by eye.
    pub(super) fn catalog() -> Vec<Action> {
        use Action::*;
        let fixed = [
            NewPdf, Open, Save, SaveAs, Print, ExportCsv, ZoomIn, ZoomOut, FitWidth, FitPage, ShrinkWide, GoToPage, FirstPage, LastPage, NextPage, PreviousPage, Find,
            FindNext, FindPrevious, Undo, Redo, PickAll, DeletePicked, Details, KeptTools, Scale, Quantities, About, Settings, WhatsNew, Select, Highlighter, Text(false), Text(true), Compare, Area(clip::AreaTool::Clip), Area(clip::AreaTool::Cut), Area(clip::AreaTool::Erase), Paste, PasteInPlace, Quit,
            SideBySide, Pin, Pins,
        ];
        let measure = [MeasureTool::Calibrate, MeasureTool::CalibrateVertical, MeasureTool::Verify].into_iter().chain(MEASURE_TOOLS).map(Measure);
        fixed.into_iter().chain(MarkupKind::TOOLS.into_iter().map(Draw)).chain(measure).collect()
    }

    pub(super) fn label(self) -> String {
        match self {
            Action::Open => "Open PDF...".to_owned(),
            Action::NewPdf => "New PDF".to_owned(),
            Action::SaveAs => "Save As…".to_owned(),
            Action::Print => "Print…".to_owned(),
            Action::Save => "Save".to_owned(),
            Action::ExportCsv => "Export quantities as CSV...".to_owned(),
            Action::Quit => "Quit".to_owned(),
            Action::ZoomIn => "Zoom in".to_owned(),
            Action::ZoomOut => "Zoom out".to_owned(),
            Action::FitWidth => "Fit width".to_owned(),
            Action::FitPage => "Fit page".to_owned(),
            Action::ShrinkWide => "Shrink oversized sheets to fit".to_owned(),
            Action::SideBySide => "Show sheets side by side".to_owned(),
            Action::GoToPage => "Go to page...".to_owned(),
            Action::FirstPage => "First page".to_owned(),
            Action::LastPage => "Last page".to_owned(),
            Action::NextPage => "Next page".to_owned(),
            Action::PreviousPage => "Previous page".to_owned(),
            Action::Find => "Find in document...".to_owned(),
            Action::FindNext => "Find next".to_owned(),
            Action::FindPrevious => "Find previous".to_owned(),
            Action::Undo => "Undo".to_owned(),
            Action::Redo => "Redo".to_owned(),
            Action::PickAll => "Select everything on this page".to_owned(),
            Action::DeletePicked => "Delete what is selected".to_owned(),
            Action::Details => "Show tool details".to_owned(),
            Action::KeptTools => "Show kept tools".to_owned(),
            Action::Scale => "Show page scale".to_owned(),
            Action::Quantities => "Toggle quantities and notes".to_owned(),
            Action::About => "About Kinetic PDF".to_owned(),
            Action::Settings => "Settings…".to_owned(),
            Action::WhatsNew => "What's new".to_owned(),
            Action::Select => "Select tool".to_owned(),
            Action::Pin => "Place pin".to_owned(),
            Action::Pins => "Show pins".to_owned(),
            Action::Highlighter => "Highlighter".to_owned(),
            Action::Compare => "Kinetic Compare".to_owned(),
            Action::Text(false) => "Text box".to_owned(),
            Action::Text(true) => "Text box with arrow".to_owned(),
            Action::Area(tool) => format!("{} tool", tool.label()),
            Action::Paste => "Paste".to_owned(),
            Action::PasteInPlace => "Paste in place".to_owned(),
            Action::Draw(kind) => format!("Draw: {}", kind.label()),
            Action::Measure(tool) => format!("Measure: {}", tool.label()),
        }
    }

    pub(super) fn group(self) -> Group {
        match self {
            Action::NewPdf | Action::Open | Action::Save | Action::SaveAs | Action::Print | Action::ExportCsv | Action::Settings | Action::Quit => Group::File,
            Action::ZoomIn | Action::ZoomOut | Action::FitWidth | Action::FitPage | Action::ShrinkWide | Action::SideBySide => Group::View,
            Action::GoToPage | Action::FirstPage | Action::LastPage | Action::NextPage | Action::PreviousPage => Group::Go,
            Action::Find | Action::FindNext | Action::FindPrevious | Action::Undo | Action::Redo | Action::PickAll | Action::DeletePicked | Action::Paste | Action::PasteInPlace => {
                Group::Edit
            }
            Action::Pins | Action::Details | Action::KeptTools | Action::Scale | Action::Quantities | Action::About | Action::WhatsNew => Group::Panels,
            Action::Pin | Action::Select | Action::Highlighter | Action::Text(_) | Action::Compare | Action::Area(_) | Action::Draw(_) | Action::Measure(_) => Group::Tools,
        }
    }

    /// The shortcut shown down the right, where the action has one. The
    /// palette does not bind these: they are hints to what is already bound
    /// in `handle_input`, `tool_keys` and `measure_keys`.
    pub(super) fn shortcut(self) -> Option<&'static str> {
        Some(match self {
            Action::Open => "Ctrl+O",
            Action::NewPdf => "Ctrl+N",
            Action::SaveAs => "Ctrl+Shift+S",
            Action::Print => "Ctrl+P",
            Action::Save => "Ctrl+S",
            Action::ZoomIn => "Ctrl++",
            Action::ZoomOut => "Ctrl+-",
            Action::FitWidth => "Ctrl+0",
            Action::GoToPage => "Ctrl+G",
            Action::FirstPage => "Home",
            Action::LastPage => "End",
            Action::NextPage => "Page Down",
            Action::PreviousPage => "Page Up",
            Action::Find => "Ctrl+F",
            Action::FindNext => "F3",
            Action::FindPrevious => "Shift+F3",
            Action::Undo => "Ctrl+Z",
            Action::Redo => "Ctrl+Y",
            Action::PickAll => "Ctrl+A",
            Action::DeletePicked => "Delete",
            Action::Select => "V",
            Action::Highlighter => "H",
            Action::Text(false) => "T",
            Action::Text(true) => "Shift+T",
            Action::Area(clip::AreaTool::Clip) => "C",
            Action::Area(clip::AreaTool::Cut) => "X",
            Action::Area(clip::AreaTool::Erase) => "D",
            Action::Paste => "Ctrl+V",
            Action::PasteInPlace => "Ctrl+Shift+V",
            Action::Draw(MarkupKind::Pen) => "P",
            Action::Draw(MarkupKind::Rectangle) => "R",
            Action::Draw(MarkupKind::Ellipse) => "E",
            Action::Draw(MarkupKind::Line) => "L",
            Action::Draw(MarkupKind::Arrow) => "A",
            _ => return None,
        })
    }

    /// Extra words the action answers to but isn't called. "zoom" should
    /// reach fit width; "csv" should reach the export.
    fn aliases(self) -> &'static str {
        match self {
            Action::Open => "file load document",
            Action::Save => "write file",
            Action::ExportCsv => "spreadsheet takeoff",
            Action::FitWidth | Action::FitPage => "zoom",
            Action::ShrinkWide => "oversized wide actual size shrunk",
            Action::GoToPage => "jump number",
            Action::Find | Action::FindNext | Action::FindPrevious => "search text",
            // The notes panel became rows of the quantities table, so someone
            // still looking for "notes" should land there.
            Action::Quantities => "takeoff bill notes",
            Action::Scale => "calibration ratio",
            Action::KeptTools => "saved named panel",
            Action::Details => "settings panel appearance",
            Action::Measure(_) => "takeoff",
            Action::Draw(_) => "annotate markup",
            Action::Select => "pick pointer arrow",
            Action::PickAll => "pick all markups measurements",
            Action::DeletePicked => "remove erase picked selection markups measurements",
            Action::Highlighter => "highlight text copy note",
            Action::Compare => "overlay revision difference changes versus",
            Action::Text(false) => "type write words label note annotate",
            Action::Text(true) => "callout leader type write words label note annotate",
            Action::Area(clip::AreaTool::Clip) => "capture copy crop region area picture",
            Action::Area(clip::AreaTool::Cut) => "move remove region area drawing",
            Action::Area(clip::AreaTool::Erase) => "delete remove rub out whiteout region area drawing",
            Action::Paste => "clip markups measurements put down",
            Action::PasteInPlace => "clip markups measurements same position where it was",
            Action::About => "version licences licenses",
            Action::Settings => "preferences options units metric imperial",
            Action::WhatsNew => "release notes changes changelog version update",
            Action::Quit => "exit close",
            _ => "",
        }
    }

    /// Everything a query is matched against: the name, the group and the
    /// aliases, so one matcher covers all three.
    pub(super) fn haystack(self) -> String {
        format!("{} {} {}", self.label(), self.group().label(), self.aliases())
    }
}

impl App {
    /// Whether an action can be run just now. A palette that hid everything
    /// unavailable would be a different list every time it opened, so what
    /// can't run is shown greyed instead: the name is still worth finding,
    /// and the grey says why it does nothing.
    pub(super) fn action_enabled(&self, action: Action) -> bool {
        let doc = self.doc.is_some();
        let dirty = self.has_unsaved_work();
        // Comparing, the document and its tools wait: only the way out, and
        // what isn't about the document, are there.
        if self.compare.is_some() {
            return matches!(action, Action::Compare | Action::About | Action::Settings | Action::WhatsNew | Action::Quit);
        }
        match action {
            Action::Open | Action::NewPdf => !matches!(self.lifecycle.status(), Status::Opening | Status::Saving),
            Action::About | Action::Settings | Action::WhatsNew | Action::Quit => true,
            Action::SaveAs => doc && !matches!(self.lifecycle.status(), Status::Opening | Status::Saving | Status::Unavailable),
            Action::Print => doc && !self.printing && !matches!(self.lifecycle.status(), Status::Opening | Status::Saving | Status::Unavailable),
            Action::Compare => doc && self.compare_starting.is_none(),
            Action::Save => dirty && !matches!(self.lifecycle.status(), Status::Saving | Status::Opening | Status::Unavailable),
            // Notes are rows of that table too, so a file with nothing
            // measured but something noted still has a spreadsheet in it.
            Action::ExportCsv => {
                self.doc.as_ref().is_some_and(|d| !d.session.measures().is_empty() || !d.session.highlights().is_empty())
            }
            Action::FindNext | Action::FindPrevious => doc && !self.search.hits.is_empty(),
            Action::DeletePicked => doc && (!self.picked_rows().is_empty() || self.pins.selected.is_some()),
            Action::ShrinkWide | Action::SideBySide => true,
            _ => doc,
        }
    }

    pub(super) fn run_action(&mut self, action: Action) {
        match action {
            Action::NewPdf => self.new_pdf(),
            Action::SaveAs => self.save_as(),
            Action::Print => self.show_print_options(),
            Action::Open => self.pick_and_open(),
            Action::Save => self.save(),
            Action::ExportCsv => self.export_quantities(),
            Action::Quit => self.ctx.send_viewport_cmd(ViewportCommand::Close),
            Action::ZoomIn => self.zoom_by(1),
            Action::ZoomOut => self.zoom_by(-1),
            Action::FitWidth => self.request_fit(ZoomMode::FitWidth),
            Action::FitPage => self.request_fit(ZoomMode::FitPage),
            Action::ShrinkWide => self.set_shrink_wide(!self.shrink_wide),
            Action::SideBySide => {
                self.hold_still(None);
                self.side_by_side = !self.side_by_side;
            }
            Action::GoToPage => self.page_box_focus = true,
            Action::FirstPage => self.go_to_page(0),
            Action::LastPage => self.go_to_end(),
            Action::NextPage => self.step_page(1),
            Action::PreviousPage => self.step_page(-1),
            Action::Find => {
                // The find box lives on the panel's find side, as Ctrl+F knows.
                self.show_tool_panel(tool_panel::Tab::Find);
                self.search.focus = true;
            }
            Action::FindNext => self.step_hit(1),
            Action::FindPrevious => self.step_hit(-1),
            Action::Undo => self.undo_step(false),
            Action::Redo => self.undo_step(true),
            Action::PickAll => {
                self.take_up_select();
                self.pick_everything_on_page();
            }
            Action::DeletePicked => self.delete_picked(),
            Action::Details => self.show_tool_panel(tool_panel::Tab::Details),
            Action::KeptTools => self.show_tool_panel(tool_panel::Tab::Tools),
            Action::Scale => self.show_tool_panel(tool_panel::Tab::Scale),
            Action::Quantities => self.quantities_open = !self.quantities_open,
            Action::About => self.show_about = true,
            Action::Settings => self.show_settings = true,
            Action::WhatsNew => self.whats_new = whats_new::all(),
            Action::Select => self.take_up_select(),
            Action::Pin => self.take_up_pin(),
            Action::Pins => self.show_tool_panel(tool_panel::Tab::Pins),
            Action::Highlighter => self.take_up_highlighter(),
            Action::Compare => self.kinetic_compare(),
            Action::Text(arrow) => self.take_up_text(arrow),
            Action::Area(tool) => self.take_up_area_tool(tool),
            Action::Paste => self.paste(false),
            Action::PasteInPlace => self.paste(true),
            Action::Draw(kind) => self.take_up_drawing(kind),
            Action::Measure(tool) => self.set_measure_tool(Some(tool)),
        }
    }

}
