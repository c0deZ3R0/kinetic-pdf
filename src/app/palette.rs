//! The command palette: Ctrl+Shift+P, type a few letters, press Enter.
//!
//! Every action the toolbar, the tool row and the keyboard offer is named
//! once here, as an `Action`. The palette is the one place that knows the
//! whole list, so an action gains a name, a shortcut hint and a search entry
//! by being added to `Action::catalog` -- nothing else has to be touched.
//!
//! An `Action` is a plain value: what it is called, which group it belongs
//! to, whether it can be run just now, and what running it does. Keeping it
//! that way means the palette never holds a borrow of the app while it draws,
//! and the matcher below can be tested without a window.
//!
//! Ctrl+K opens the same box over a different list: the tools kept by name,
//! so a run of takeoff can go from one kept tool to the next by typing a few
//! letters of it rather than scrolling the panel for it.

use super::tools::{SavedTool, ToolKey, MEASURE_TOOLS};
use super::*;

/* ------------------------------------------------------------------ *
 * The catalogue
 * ------------------------------------------------------------------ */

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
    fn label(self) -> &'static str {
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
    Open,
    Save,
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
    /// Put every tool down, which leaves the Select tool in hand.
    Select,
    /// Take up the highlighter, which picks out text to highlight.
    Highlighter,
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
            Open, Save, ExportCsv, ZoomIn, ZoomOut, FitWidth, FitPage, ShrinkWide, GoToPage, FirstPage, LastPage, NextPage, PreviousPage, Find,
            FindNext, FindPrevious, Undo, Redo, PickAll, DeletePicked, Details, KeptTools, Scale, Quantities, About, Select, Highlighter, Quit,
            SideBySide,
        ];
        let measure = [MeasureTool::Calibrate, MeasureTool::CalibrateVertical, MeasureTool::Verify].into_iter().chain(MEASURE_TOOLS).map(Measure);
        fixed.into_iter().chain(MarkupKind::TOOLS.into_iter().map(Draw)).chain(measure).collect()
    }

    pub(super) fn label(self) -> String {
        match self {
            Action::Open => "Open PDF...".to_owned(),
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
            Action::Select => "Select tool".to_owned(),
            Action::Highlighter => "Highlighter".to_owned(),
            Action::Draw(kind) => format!("Draw: {}", kind.label()),
            Action::Measure(tool) => format!("Measure: {}", tool.label()),
        }
    }

    pub(super) fn group(self) -> Group {
        match self {
            Action::Open | Action::Save | Action::ExportCsv | Action::Quit => Group::File,
            Action::ZoomIn | Action::ZoomOut | Action::FitWidth | Action::FitPage | Action::ShrinkWide | Action::SideBySide => Group::View,
            Action::GoToPage | Action::FirstPage | Action::LastPage | Action::NextPage | Action::PreviousPage => Group::Go,
            Action::Find | Action::FindNext | Action::FindPrevious | Action::Undo | Action::Redo | Action::PickAll | Action::DeletePicked => {
                Group::Edit
            }
            Action::Details | Action::KeptTools | Action::Scale | Action::Quantities | Action::About => Group::Panels,
            Action::Select | Action::Highlighter | Action::Draw(_) | Action::Measure(_) => Group::Tools,
        }
    }

    /// The shortcut shown down the right, where the action has one. The
    /// palette does not bind these: they are hints to what is already bound
    /// in `handle_input`, `tool_keys` and `measure_keys`.
    pub(super) fn shortcut(self) -> Option<&'static str> {
        Some(match self {
            Action::Open => "Ctrl+O",
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
            Action::About => "version licences licenses",
            Action::Quit => "exit close",
            _ => "",
        }
    }

    /// Everything a query is matched against: the name, the group and the
    /// aliases, so one matcher covers all three.
    fn haystack(self) -> String {
        format!("{} {} {}", self.label(), self.group().label(), self.aliases())
    }
}

/* ------------------------------------------------------------------ *
 * Matching
 * ------------------------------------------------------------------ */

/// How well `query` matches `text`, or `None` if it doesn't. Every character
/// of the query has to appear in the text, in order; a match scores higher
/// the more of it lands on the starts of words and the fewer gaps it leaves.
///
/// Both are compared without case. The query's spaces are dropped, so "fit w"
/// and "fitw" behave the same.
pub(super) fn score(query: &str, text: &str) -> Option<i32> {
    let needle: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    if needle.len() > hay.len() {
        return None;
    }

    let mut total = 0;
    let mut at = 0;
    let mut previous: Option<usize> = None;
    for &want in &needle {
        // The next place this character appears, preferring the start of a
        // word further along, so "fp" finds "Fit page" rather than the "p"
        // inside "Fit".
        let found = (at..hay.len()).find(|&i| hay[i] == want)?;
        let boundary = (at..hay.len()).find(|&i| hay[i] == want && is_word_start(&hay, i));
        let i = boundary.unwrap_or(found);
        total += if is_word_start(&hay, i) { 12 } else { 3 };
        // Running straight on from the last character is worth more than the
        // same letters scattered about.
        if i > 0 && previous == Some(i - 1) {
            total += 8;
        }
        previous = Some(i);
        at = i + 1;
    }
    // A short name that is nearly all match beats a long one that merely
    // contains the letters.
    total += 40i32.saturating_sub(hay.len() as i32 / 2);
    if hay.starts_with(&needle[..]) {
        total += 25;
    }
    Some(total)
}

fn is_word_start(hay: &[char], i: usize) -> bool {
    i == 0 || !hay[i - 1].is_alphanumeric()
}

/// The actions matching `query`, best first. An empty query keeps the
/// catalogue's own order, which is the order the toolbar reads in.
pub(super) fn matches(query: &str, catalog: &[Action]) -> Vec<Action> {
    let entries: Vec<(String, String)> = catalog.iter().map(|a| (a.label(), a.haystack())).collect();
    rank(query, &entries).into_iter().map(|i| catalog[i]).collect()
}

/// The places in `entries` matching `query`, best first. Each entry is its
/// name and everything it answers to, the name included.
fn rank(query: &str, entries: &[(String, String)]) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..entries.len()).collect();
    }
    let mut scored: Vec<(i32, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, (name, haystack))| {
            // A match on the name alone is worth more than one spread over
            // the group and the aliases, so the obvious answer comes first.
            let name = score(query, name).map(|s| s + 20);
            let wide = score(query, haystack);
            name.into_iter().chain(wide).max().map(|s| (s, i))
        })
        .collect();
    // Ties keep the catalogue's order, so the list never shuffles about.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

/// A tool kept by name, as the Ctrl+K list shows it. Taken out of the kept
/// tools each frame, so the list holds no borrow of them while it draws.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Kept {
    /// Where it sits among the kept tools, which is how it is taken up.
    pub at: usize,
    pub name: String,
    pub group: String,
    /// The tool it draws with, or `None` for one from a version that knows a
    /// tool this one doesn't.
    pub key: Option<ToolKey>,
    /// That tool's name: "Area", "Rectangle".
    pub kind: String,
}

impl Kept {
    pub(super) fn new(at: usize, tool: &SavedTool, kind: String) -> Kept {
        Kept { at, name: tool.name.clone(), group: tool.group.clone(), key: tool.key(), kind }
    }

    /// Down the right of the row: the group it is filed under, then the tool
    /// it draws with.
    fn detail(&self) -> String {
        match (self.group.is_empty(), self.kind.is_empty()) {
            (true, _) => self.kind.clone(),
            (false, true) => self.group.clone(),
            (false, false) => format!("{} \u{00b7} {}", self.group, self.kind),
        }
    }
}

/// The kept tools matching `query`, best first. An empty query keeps the
/// order the panel lists them in. "slab" finds "Slab 200"; "concrete" finds
/// everything filed under Concrete; "area" finds every area tool.
pub(super) fn kept_matches(query: &str, kept: &[Kept]) -> Vec<Kept> {
    let entries: Vec<(String, String)> =
        kept.iter().map(|k| (k.name.clone(), format!("{} {} {}", k.name, k.group, k.kind))).collect();
    rank(query, &entries).into_iter().map(|i| kept[i].clone()).collect()
}

/* ------------------------------------------------------------------ *
 * State
 * ------------------------------------------------------------------ */

/// How many rows are drawn before the list scrolls.
const VISIBLE_ROWS: usize = 10;

const ROW_HEIGHT: f32 = 34.0;

const WIDTH: f32 = 540.0;

/// How many frames the box ignores typed characters for after it opens; see
/// `Palette::swallow`. Two is enough for the character that came in with the
/// chord, without swallowing anything a user could have typed since.
const SWALLOW_FRAMES: u8 = 2;

/// Which list the box is over.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum Mode {
    /// Every command: Ctrl+Shift+P.
    #[default]
    Commands,
    /// The tools kept by name: Ctrl+K.
    KeptTools,
}

/// What a row runs when it is chosen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Choice {
    Action(Action),
    CreateTool,
    /// A kept tool, by where it sits among them.
    Kept(usize),
}

/// One row as drawn: the same shape whichever list it came from.
struct Row {
    choice: Choice,
    label: String,
    /// Down the right: the group, and for a kept tool what it draws with.
    detail: String,
    shortcut: Option<&'static str>,
    /// Drawn before the name. Only the kept tools carry one: a long list of
    /// look-alike names reads faster with the kind of tool beside each.
    icon: Option<Icon>,
    enabled: bool,
}

#[derive(Default)]
pub(super) struct Palette {
    pub(super) open: bool,
    mode: Mode,
    query: String,
    /// Which row is picked out, as an index into the matches.
    selected: usize,
    /// Set when the palette opens, to put the cursor in the box.
    focus: bool,
    /// Set when the keyboard moves the pick, so the list scrolls to it once.
    /// Scrolling to it every frame instead would fight the wheel: the list
    /// sprang straight back to the picked row the moment it was let go.
    follow: bool,
    /// Frames left over which typed characters are thrown away. The chord
    /// that opens the palette carries a letter of its own, and the box takes
    /// keyboard focus a frame after it is asked to, so without this the P of
    /// Ctrl+Shift+P arrives in the box and every query begins "p".
    swallow: u8,
}

impl Palette {
    fn show(&mut self, mode: Mode) {
        self.open = true;
        self.mode = mode;
        self.query.clear();
        self.selected = 0;
        self.focus = true;
        self.follow = true;
        self.swallow = SWALLOW_FRAMES;
    }

    fn hide(&mut self) {
        self.open = false;
        self.focus = false;
    }
}

/* ------------------------------------------------------------------ *
 * The app's side of it
 * ------------------------------------------------------------------ */

impl App {
    /// Ctrl+Shift+P opens the palette over the commands, Ctrl+K over the kept
    /// tools; the same chord again closes it, and the other one swaps lists.
    /// Answered before anything else reads the keyboard, and `true` while the
    /// palette has it, so the letters typed into the box never reach the tools.
    pub(super) fn palette_keys(&mut self, ctx: &egui::Context) -> bool {
        let (commands, kept) = ctx.input_mut(|i| {
            (i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::P), i.consume_key(Modifiers::COMMAND, Key::K))
        });
        let asked = if commands {
            Some(Mode::Commands)
        } else if kept {
            Some(Mode::KeptTools)
        } else {
            None
        };
        if let Some(mode) = asked {
            if self.palette.open && self.palette.mode == mode {
                self.palette.hide();
            } else {
                self.palette.show(mode);
            }
        }
        self.palette.open
    }

    /// The kept tools, in the order the panel lists them.
    fn kept_catalog(&self) -> Vec<Kept> {
        self.tools
            .groups()
            .into_iter()
            .flat_map(|(_, tools)| tools)
            .map(|(at, tool)| Kept::new(at, tool, tool.key().map(|key| self.tool_title(key)).unwrap_or_default()))
            .collect()
    }

    /// The rows matching what has been typed, from whichever list is up.
    fn palette_rows(&self) -> Vec<Row> {
        match self.palette.mode {
            Mode::Commands => matches(&self.palette.query, &Action::catalog())
                .into_iter()
                .map(|action| Row {
                    choice: Choice::Action(action),
                    label: action.label(),
                    detail: action.group().label().to_owned(),
                    shortcut: action.shortcut(),
                    icon: None,
                    enabled: self.action_enabled(action),
                })
                .collect(),
            Mode::KeptTools => {
                let mut rows = Vec::new();
                if score(&self.palette.query, "Create new tool add tool").is_some() {
                    rows.push(Row {
                        choice: Choice::CreateTool,
                        label: "Create new tool…".to_owned(),
                        detail: "New".to_owned(),
                        shortcut: None,
                        icon: None,
                        enabled: true,
                    });
                }
                rows.extend(kept_matches(&self.palette.query, &self.kept_catalog())
                .into_iter()
                .map(|kept| Row {
                    choice: Choice::Kept(kept.at),
                    detail: kept.detail(),
                    shortcut: None,
                    icon: kept.key.map(|key| self.key_icon(key)),
                    // Like the tools among the commands: nothing to draw on
                    // without a document.
                    enabled: self.doc.is_some() && kept.key.is_some(),
                    label: kept.name,
                }));
                rows
            }
        }
    }

    /// Whether an action can be run just now. A palette that hid everything
    /// unavailable would be a different list every time it opened, so what
    /// can't run is shown greyed instead: the name is still worth finding,
    /// and the grey says why it does nothing.
    pub(super) fn action_enabled(&self, action: Action) -> bool {
        let doc = self.doc.is_some();
        let dirty = self.has_unsaved_work();
        match action {
            Action::Open | Action::About | Action::Quit => true,
            Action::Save => dirty && !matches!(self.status, Status::Saving),
            // Notes are rows of that table too, so a file with nothing
            // measured but something noted still has a spreadsheet in it.
            Action::ExportCsv => {
                self.doc.as_ref().is_some_and(|d| !d.session.measures().is_empty() || !d.session.highlights().is_empty())
            }
            Action::FindNext | Action::FindPrevious => doc && !self.search.hits.is_empty(),
            Action::DeletePicked => doc && !self.picked_rows().is_empty(),
            Action::ShrinkWide | Action::SideBySide => true,
            _ => doc,
        }
    }

    pub(super) fn run_action(&mut self, action: Action) {
        match action {
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
            Action::Select => self.take_up_select(),
            Action::Highlighter => self.take_up_highlighter(),
            Action::Draw(kind) => self.take_up_drawing(kind),
            Action::Measure(tool) => self.set_measure_tool(Some(tool)),
        }
    }

    /* -------------------------------------------------------------- *
     * Drawing
     * -------------------------------------------------------------- */

    /// The palette itself: a box to type in over a list of what matches.
    /// Drawn last, over everything else.
    pub(super) fn show_palette(&mut self, ctx: &egui::Context) {
        if !self.palette.open {
            return;
        }
        // The letter the opening chord carried must not land in the box.
        if self.palette.swallow > 0 {
            self.palette.swallow -= 1;
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
        }

        let found = self.palette_rows();
        self.palette.selected = self.palette.selected.min(found.len().saturating_sub(1));

        // Arrows and Enter are taken before the text box sees them, so typing
        // and choosing share the one keyboard.
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.palette.hide();
            return;
        }
        if !found.is_empty() {
            let last = found.len() - 1;
            if down {
                self.palette.selected = if self.palette.selected == last { 0 } else { self.palette.selected + 1 };
            }
            if up {
                self.palette.selected = if self.palette.selected == 0 { last } else { self.palette.selected - 1 };
            }
            if up || down {
                self.palette.follow = true;
            }
        }

        let mut chosen = if enter { found.get(self.palette.selected).map(|row| row.choice) } else { None };

        // A press anywhere outside shuts it, the way a menu does.
        let pressed = ctx.input(|i| i.pointer.any_pressed());

        // `anchor` takes the alignment as the pivot as well, so CENTER_TOP
        // puts the palette's own middle on the window's middle. Not
        // constrained: `Area::constrain` clamps against the size the area had
        // last frame, which on the frame it opens is nothing, and it stayed
        // clamped -- the list came out two rows tall.
        let area = egui::Area::new(Id::new("command-palette"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, 84.0))
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(12))
                    .shadow(soft_shadow())
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(WIDTH);
                        if let Some(choice) = self.palette_box(ui, &found) {
                            chosen = Some(choice);
                        }
                    });
            });

        if pressed && !area.response.contains_pointer() {
            self.palette.hide();
            return;
        }
        let Some(choice) = chosen else { return };
        self.palette.hide();
        let Some(row) = found.iter().find(|row| row.choice == choice) else { return };
        if !row.enabled {
            self.toast(format!("{} isn't available just now.", row.label));
            return;
        }
        match choice {
            Choice::Action(action) => self.run_action(action),
            Choice::CreateTool => self.open_tool_creator(),
            Choice::Kept(at) => self.take_up_saved(at),
        }
    }

    /// The text box and the rows under it. Returns what a click chose.
    fn palette_box(&mut self, ui: &mut Ui, found: &[Row]) -> Option<Choice> {
        let (box_id, hint) = match self.palette.mode {
            Mode::Commands => (Id::new("command-palette-query"), "Type a command..."),
            Mode::KeptTools => (Id::new("kept-tools-query"), "Type a kept tool's name or group..."),
        };
        let field = TextEdit::singleline(&mut self.palette.query)
            .id(box_id)
            .hint_text(hint)
            .desired_width(f32::INFINITY)
            .vertical_align(Align::Center)
            .font(FontId::proportional(15.0));
        let response = ui.add_sized(vec2(WIDTH, 32.0), field);
        if std::mem::take(&mut self.palette.focus) {
            response.request_focus();
        }
        // Typing moves the pick back to the top: the best match for what has
        // just been typed, not whatever row the last query left behind.
        if response.changed() {
            self.palette.selected = 0;
            self.palette.follow = true;
        }

        ui.add_space(8.0);
        if found.is_empty() {
            let note = match self.palette.mode {
                Mode::Commands => "No matching command",
                // Nothing kept at all wants saying differently from nothing
                // matching: the first is a question of where tools come from.
                Mode::KeptTools if self.tools.saved_count() == 0 => {
                    "No tools kept yet. Keep one from the Kept tools panel, and it is listed here."
                }
                Mode::KeptTools => "No matching kept tool",
            };
            ui.add_space(6.0);
            ui.label(RichText::new(note).size(13.0).color(MUTED));
            ui.add_space(6.0);
            return None;
        }

        let mut chosen = None;
        // Ten rows' worth, and no taller than the list actually is: a short
        // list leaves no empty space below it.
        // Set outright rather than left to shrink: the area around it is
        // sized from what it held last frame, so a list that shrank to two
        // matches stayed two rows tall after the query was cleared.
        let height = ROW_HEIGHT * found.len().min(VISIBLE_ROWS) as f32;
        // Taken once, so only the row drawn this frame scrolls itself into
        // view. Left set, every frame would re-scroll and the wheel would
        // never get anywhere.
        let follow = std::mem::take(&mut self.palette.follow);
        let selected = self.palette.selected;
        egui::ScrollArea::vertical().min_scrolled_height(height).max_height(height).auto_shrink([false, true]).show(ui, |ui| {
            // The rows carry their own padding, so the usual gap between
            // widgets would make `VISIBLE_ROWS` a lie -- ten rows would
            // stand taller than the height set aside for them.
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, row) in found.iter().enumerate() {
                if palette_row(ui, row, i == selected, follow) {
                    chosen = Some(row.choice);
                }
            }
        });
        chosen
    }
}

/// One row: the name on the left, the group and the shortcut on the right,
/// tinted when it is the one Enter would run.
fn palette_row(ui: &mut Ui, row: &Row, selected: bool, follow: bool) -> bool {
    // `Sense::CLICK` rather than `Sense::click()`: the latter also makes the
    // row focusable, and egui counts Space or Enter on a focused widget as a
    // click. A row that can take focus therefore fires when the space bar is
    // pressed -- so typing "fit page" ran whichever row held focus, at the
    // space, instead of typing it. Only the box above takes the keyboard.
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::CLICK);
    // Only when the keyboard moved the pick, never while the wheel is doing
    // the moving.
    if selected && follow {
        response.scroll_to_me(None);
    }
    if !ui.is_rect_visible(rect) {
        return response.clicked();
    }

    let enabled = row.enabled;
    let hovered = response.hovered();
    let fill = if selected {
        ACCENT_SOFT
    } else if hovered {
        ROW_HOVER
    } else {
        Color32::TRANSPARENT
    };
    let ink = if !enabled {
        SUBTLE
    } else if selected {
        ACCENT_TEXT
    } else {
        TEXT
    };
    let faint = if enabled { MUTED } else { SUBTLE };

    let painter = ui.painter();
    painter.rect_filled(rect.shrink2(vec2(2.0, 1.0)), CornerRadius::same(7), fill);
    let mut inner = rect.shrink2(vec2(10.0, 0.0));
    if let Some(icon) = row.icon {
        let square = Rect::from_min_size(pos2(inner.left(), inner.center().y - 8.0), vec2(16.0, 16.0));
        icons::paint(painter, square, icon, if selected && enabled { ACCENT_TEXT } else { faint });
        inner.min.x += 26.0;
    }
    painter.text(pos2(inner.left(), inner.center().y), Align2::LEFT_CENTER, &row.label, FontId::proportional(13.5), ink);

    let mut right = inner.right();
    if let Some(keys) = row.shortcut {
        let galley = painter.layout_no_wrap(keys.to_owned(), FontId::monospace(11.5), faint);
        let width = galley.size().x;
        painter.galley(pos2(right - width, inner.center().y - galley.size().y / 2.0), galley, faint);
        right -= width + 12.0;
    }
    painter.text(pos2(right, inner.center().y), Align2::RIGHT_CENTER, &row.detail, FontId::proportional(11.5), SUBTLE);

    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.clicked()
}

/* ------------------------------------------------------------------ *
 * Tests
 * ------------------------------------------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;

    fn best(query: &str) -> Action {
        let catalog = Action::catalog();
        matches(query, &catalog).into_iter().next().expect("something should match")
    }

    #[test]
    fn an_empty_query_keeps_every_command_in_order() {
        let catalog = Action::catalog();
        assert_eq!(matches("", &catalog), catalog);
        assert_eq!(matches("   ", &catalog), catalog);
    }

    #[test]
    fn initials_find_the_command() {
        assert_eq!(best("fp"), Action::FitPage);
        assert_eq!(best("fw"), Action::FitWidth);
        assert_eq!(best("gtp"), Action::GoToPage);
    }

    #[test]
    fn a_whole_word_finds_its_command() {
        assert_eq!(best("save"), Action::Save);
        assert_eq!(best("undo"), Action::Undo);
        assert_eq!(best("about"), Action::About);
    }

    #[test]
    fn tools_are_found_by_name() {
        assert_eq!(best("area"), Action::Measure(MeasureTool::Area));
        assert_eq!(best("rectangle"), Action::Draw(MarkupKind::Rectangle));
        assert_eq!(best("calibrate"), Action::Measure(MeasureTool::Calibrate));
        assert_eq!(best("highlighter"), Action::Highlighter);
        assert_eq!(best("select"), Action::Select);
    }

    /// Every tool on the tool row is in the palette too, so none of them is
    /// reachable only by eye.
    #[test]
    fn every_tool_on_the_row_is_in_the_palette() {
        let catalog = Action::catalog();
        let tools = [Action::Select, Action::Highlighter]
            .into_iter()
            .chain(MarkupKind::TOOLS.into_iter().map(Action::Draw))
            .chain(MEASURE_TOOLS.into_iter().map(Action::Measure));
        for tool in tools {
            assert!(catalog.contains(&tool), "{tool:?} is missing from the palette");
            assert_eq!(tool.group(), Group::Tools);
        }
        assert!(matches("highlight", &catalog).contains(&Action::Highlighter));
    }

    #[test]
    fn aliases_reach_commands_that_are_not_called_that() {
        let catalog = Action::catalog();
        assert!(matches("csv", &catalog).contains(&Action::ExportCsv));
        assert!(matches("bill", &catalog).contains(&Action::Quantities));
        assert!(matches("exit", &catalog).contains(&Action::Quit));
    }

    #[test]
    fn nothing_matches_letters_the_command_has_not_got() {
        let catalog = Action::catalog();
        assert!(matches("zzzz", &catalog).is_empty());
    }

    #[test]
    fn spaces_in_the_query_are_ignored() {
        let catalog = Action::catalog();
        assert_eq!(matches("fit w", &catalog).first(), matches("fitw", &catalog).first());
    }

    #[test]
    fn a_word_start_beats_the_same_letters_buried_in_a_word() {
        let start = score("fp", "Fit page").expect("matches");
        let buried = score("fp", "Afternoon plan").expect("matches");
        assert!(start > buried, "{start} should beat {buried}");
    }

    #[test]
    fn a_shorter_name_wins_when_both_match_the_same_way() {
        let short = score("save", "Save").expect("matches");
        let long = score("save", "Save a copy of everything somewhere").expect("matches");
        assert!(short > long);
    }

    #[test]
    fn every_command_is_named_and_grouped() {
        for action in Action::catalog() {
            assert!(!action.label().is_empty(), "{action:?} has no name");
            assert!(!action.group().label().is_empty(), "{action:?} has no group");
        }
    }

    fn kept(at: usize, name: &str, group: &str, key: ToolKey) -> Kept {
        let kind = match key {
            ToolKey::Measure(tool) => tool.label().to_owned(),
            ToolKey::Draw(kind) => kind.label().to_owned(),
            ToolKey::Highlight => "Highlight".to_owned(),
        };
        Kept { at, name: name.to_owned(), group: group.to_owned(), key: Some(key), kind }
    }

    fn some_kept() -> Vec<Kept> {
        vec![
            kept(0, "Slab 200", "Concrete", ToolKey::Measure(MeasureTool::Area)),
            kept(1, "Footing", "Concrete", ToolKey::Measure(MeasureTool::Length)),
            kept(2, "Fence", "Site", ToolKey::Measure(MeasureTool::Length)),
            kept(3, "Cloud", "", ToolKey::Draw(MarkupKind::Rectangle)),
        ]
    }

    fn names(found: &[Kept]) -> Vec<&str> {
        found.iter().map(|k| k.name.as_str()).collect()
    }

    #[test]
    fn an_empty_query_lists_every_kept_tool_in_order() {
        let all = some_kept();
        assert_eq!(kept_matches("", &all), all);
    }

    #[test]
    fn a_kept_tool_is_found_by_its_name_first() {
        let found = kept_matches("fence", &some_kept());
        assert_eq!(names(&found)[0], "Fence");
        assert_eq!(kept_matches("slab", &some_kept())[0].at, 0);
    }

    /// Typing a group finds everything filed under it, and typing a kind of
    /// tool finds every kept tool that draws with it.
    #[test]
    fn kept_tools_are_found_by_group_and_by_kind() {
        assert_eq!(names(&kept_matches("concrete", &some_kept())), ["Slab 200", "Footing"]);
        let found = kept_matches("length", &some_kept());
        let lengths = names(&found);
        assert!(lengths.contains(&"Footing") && lengths.contains(&"Fence"), "{lengths:?}");
        assert!(kept_matches("zzzz", &some_kept()).is_empty());
    }

    #[test]
    fn a_kept_tool_shows_its_group_and_kind() {
        let all = some_kept();
        assert_eq!(all[0].detail(), "Concrete \u{00b7} Area");
        assert_eq!(all[3].detail(), "Rectangle", "an ungrouped tool shows only its kind");
    }

    /// Runs one frame of the keyboard and the palette, with `events` in it.
    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 900.0))), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            app.palette_keys(ui.ctx());
            app.show_palette(ui.ctx());
        });
        output.textures_delta.clear();
    }

    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    /// Ctrl+K lists the kept tools, and Enter on one takes it up with its
    /// settings, just as picking it off the panel does.
    #[test]
    fn ctrl_k_takes_up_a_kept_tool_by_name() {
        let (mut app, ctx) = super::super::tests::app_with_a_document();
        // Not whatever tools this machine has kept.
        app.tools = super::super::tools::Tools::default();
        let key_area = ToolKey::Measure(MeasureTool::Area);
        let mut settings = app.tools.settings(key_area);
        settings.defaults.description = "poured on site".to_owned();
        app.tools.save_tool("Slab 200", "Concrete", key_area, settings);
        app.tools.save_tool("Fence", "Site", ToolKey::Measure(MeasureTool::Length), app.tools.settings(ToolKey::Measure(MeasureTool::Length)));

        frame(&mut app, &ctx, vec![key(Key::K, Modifiers::COMMAND)]);
        assert!(app.palette.open && app.palette.mode == Mode::KeptTools);
        let listed: Vec<String> = app.palette_rows().into_iter().map(|row| row.label).collect();
        assert_eq!(listed, ["Create new tool…", "Slab 200", "Fence"]);

        // Past the frames that swallow the chord's own letter.
        frame(&mut app, &ctx, Vec::new());
        frame(&mut app, &ctx, Vec::new());
        app.palette.query = "slab".to_owned();
        frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE)]);
        assert!(!app.palette.open, "choosing closes the box");
        assert_eq!(app.measure_tool, Some(MeasureTool::Area));
        assert_eq!(app.tools.settings(key_area).defaults.description, "poured on site");
    }

    fn palette_height(ctx: &egui::Context) -> f32 {
        ctx.memory(|m| m.area_rect(Id::new("command-palette"))).expect("the palette was drawn").height()
    }

    /// The list is as tall as what matches, up to `VISIBLE_ROWS`, every time
    /// it is drawn. It used to keep the height it last had: a query that
    /// narrowed it to two rows left it two rows tall after the query was
    /// cleared, and for the other list too.
    #[test]
    fn the_list_grows_back_after_a_query_narrows_it() {
        let (mut app, ctx) = super::super::tests::app_with_a_document();
        app.tools = super::super::tools::Tools::default();
        let area = ToolKey::Measure(MeasureTool::Area);
        for name in ["Slab", "Kerb", "Footing", "Wall", "Pier"] {
            app.tools.save_tool(name, "Concrete", area, app.tools.settings(area));
        }

        frame(&mut app, &ctx, vec![key(Key::K, Modifiers::COMMAND)]);
        let full = palette_height(&ctx);
        app.palette.query = "slab".to_owned();
        frame(&mut app, &ctx, Vec::new());
        let narrowed = palette_height(&ctx);
        assert!(narrowed < full, "one match is shorter than five: {narrowed} against {full}");
        app.palette.query.clear();
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(palette_height(&ctx), full, "all five rows again once the query is cleared");

        frame(&mut app, &ctx, vec![key(Key::P, Modifiers::COMMAND | Modifiers::SHIFT)]);
        let commands = palette_height(&ctx);
        assert_eq!(commands - full, ROW_HEIGHT * (VISIBLE_ROWS - 6) as f32, "the commands fill the rows the five tools and Create left");
    }

    /// The other chord swaps lists; the same chord again closes the box.
    #[test]
    fn each_chord_opens_its_own_list() {
        let (mut app, ctx) = super::super::tests::app_with_a_document();
        frame(&mut app, &ctx, vec![key(Key::P, Modifiers::COMMAND | Modifiers::SHIFT)]);
        assert_eq!(app.palette.mode, Mode::Commands);
        frame(&mut app, &ctx, vec![key(Key::K, Modifiers::COMMAND)]);
        assert!(app.palette.open && app.palette.mode == Mode::KeptTools);
        frame(&mut app, &ctx, vec![key(Key::K, Modifiers::COMMAND)]);
        assert!(!app.palette.open);
    }

    #[test]
    fn ctrl_k_starts_with_create_tool() {
        let (mut app, ctx) = super::super::tests::app_with_a_document();
        app.tools = super::super::tools::Tools::default();
        frame(&mut app, &ctx, vec![key(Key::K, Modifiers::COMMAND)]);
        assert_eq!(app.palette_rows()[0].choice, Choice::CreateTool);
        frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE)]);
        assert!(!app.palette.open);
        assert!(app.tool_creator.is_some());
        assert_eq!(app.tools.saved_count(), 0, "opening the creator does not save a draft");
    }

    #[test]
    fn no_two_commands_share_a_name() {
        let mut names: Vec<String> = Action::catalog().into_iter().map(Action::label).collect();
        names.sort();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "two commands share a name");
    }
}
