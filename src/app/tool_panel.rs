//! The panel down the left: what the tool in hand is set to.
//!
//! It edits `tools::ToolSettings` for whichever tool is held, and nothing
//! else. Everything it shows is part of that structure, so when tools can be
//! saved and named this panel is already the editor for a saved one -- only
//! where the settings come from changes, not what is shown.

use markup_model::markup::{FillPattern, LabelFont, WidthUnit};
use markup_model::units::{format_length, LengthUnit, Precision};

use super::tools::{Mixed, ToolKey, ToolSettings, Tools, SAVABLE_MEASURE_TOOLS};
use super::*;

/// How wide the rail of symbols down the left edge is: one button and the
/// margin either side of it.
const RAIL_WIDTH: f32 = 40.0;

/// Which side of the panel is showing.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Tab {
    /// What the tool in hand, or the measurement picked out, is set to.
    #[default]
    Details,
    /// The tools kept by name, in their groups.
    Tools,
    /// Finding words in the document, and every match found.
    Find,
    /// What the page in view measures at.
    Scale,
}

/// What the panel is editing. Each is a `ToolSettings`: the one the next
/// markup will be drawn with, or the one a markup already down has.
#[derive(Clone, Copy)]
enum Subject {
    Tool(ToolKey),
    Measurement { id: MarkupId, key: ToolKey },
    /// A drawn markup picked out on the page: a pen stroke, a box, an
    /// ellipse, a line or an arrow. It carries fewer settings than a
    /// measurement -- nothing about it is measured -- but the same form sets
    /// them.
    Drawing { uid: u64, key: ToolKey },
    /// Several things picked out together, all drawn with the one tool: the
    /// form is that tool's, and a change goes to every one of them. Which
    /// they are is `App::picked_rows`.
    Many(ToolKey),
}

impl Subject {
    fn key(self) -> ToolKey {
        match self {
            Subject::Tool(key) | Subject::Measurement { key, .. } | Subject::Drawing { key, .. } | Subject::Many(key) => key,
        }
    }

    /// Whether what it draws is measured. Only a measurement writes a
    /// quantity on the page, or is priced by a depth or a slope.
    fn measures(self) -> bool {
        matches!(self.key(), ToolKey::Measure(_))
    }
}

impl App {
    /// The tool in hand, if it is one with settings.
    pub(super) fn held_tool(&self) -> Option<ToolKey> {
        match (self.measure_tool, self.tool) {
            // Calibrating and checking set the page's scale rather than
            // drawing anything that is kept, so they have nothing to set.
            (Some(MeasureTool::Calibrate | MeasureTool::CalibrateVertical | MeasureTool::Verify), _) => None,
            (Some(tool), _) => Some(ToolKey::Measure(tool)),
            (None, Some(kind)) => Some(ToolKey::Draw(kind)),
            (None, None) if self.highlighting() => Some(ToolKey::Highlight),
            (None, None) => None,
        }
    }

    /// What the panel is about: whatever is picked out on the page -- a
    /// measurement, or something drawn -- and failing that the tool in hand.
    /// What is picked out wins, since picking it out is asking about that one
    /// rather than about the next one drawn.
    fn subject(&self) -> Option<Subject> {
        // Several picked out are one subject only if one tool draws them all:
        // a form for areas can't set a length. Otherwise there is no form,
        // and the panel says what is picked out instead.
        let several = self.picked_rows();
        if several.len() > 1 {
            let key = self.row_key(several[0])?;
            return several.iter().all(|&id| self.row_key(id) == Some(key)).then_some(Subject::Many(key));
        }
        let picked = self.active_measure.and_then(|id| {
            let markup = self.doc.as_ref()?.session.measures().get(id)?;
            Some(Subject::Measurement { id, key: ToolKey::of_measurement(markup.kind)? })
        });
        let drawn = || {
            let uid = self.active?;
            let entry = self.doc.as_ref()?.session.markup(uid)?;
            Some(Subject::Drawing { uid, key: ToolKey::Draw(entry.markup.kind) })
        };
        picked.or_else(drawn).or_else(|| self.held_tool().map(Subject::Tool))
    }

    /// The tool a row of the table was drawn with. A note has none.
    fn row_key(&self, id: RowId) -> Option<ToolKey> {
        let session = &self.doc.as_ref()?.session;
        match id {
            RowId::Measure(id) => ToolKey::of_measurement(session.measures().get(id)?.kind),
            RowId::Drawing(uid) => Some(ToolKey::Draw(session.markup(uid)?.markup.kind)),
            RowId::Note(_) => None,
        }
    }

    /// What a row of the table is set to, for the form. A note has no form.
    fn row_settings(&self, id: RowId) -> Option<ToolSettings> {
        let session = &self.doc.as_ref()?.session;
        match id {
            RowId::Measure(id) => Some(ToolSettings::of_markup(session.measures().get(id)?)),
            RowId::Drawing(uid) => Some(ToolSettings::of_drawing(&session.markup(uid)?.markup)),
            RowId::Note(_) => None,
        }
    }

    /// What a row is, in a word, for saying what is picked out.
    fn row_label(&self, id: RowId) -> &'static str {
        let Some(session) = self.doc.as_ref().map(|d| &d.session) else { return "" };
        match id {
            RowId::Measure(id) => session.measures().get(id).map_or("", |m| m.kind.label()),
            RowId::Drawing(uid) => session.markup(uid).map_or("", |e| e.markup.kind.label()),
            RowId::Note(_) => "Note",
        }
    }

    /// Several things of different kinds picked out: how many of each, since
    /// there is no one form to set them all with.
    fn assorted_body(&self, ui: &mut Ui, picked: &[RowId]) {
        let mut kinds: Vec<(&str, usize)> = Vec::new();
        for &id in picked {
            let label = self.row_label(id);
            match kinds.iter_mut().find(|(kind, _)| *kind == label) {
                Some((_, count)) => *count += 1,
                None => kinds.push((label, 1)),
            }
        }
        section(ui, &format!("{} picked out", picked.len()));
        for (kind, count) in kinds {
            ui.label(RichText::new(format!("{count} {}", plural(kind, count))).size(12.5).color(TEXT));
        }
        empty_note(ui, "Pick out things of one kind to change them together.");
    }

    /// The rail down the left edge, always there: one symbol per side of the
    /// panel. The symbol for whichever side is open is lit; clicking it again
    /// collapses the panel back to the rail.
    pub(super) fn tool_rail(&mut self, ui: &mut Ui) {
        let frame = Frame::NONE.fill(SURFACE).inner_margin(Margin::symmetric(4, 6));
        egui::Panel::left(Id::new("tool-rail")).exact_size(RAIL_WIDTH).resizable(false).frame(frame).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let kept = self.tools.saved_count();
                let tabs = [
                    (Tab::Details, Icon::Details, "Details — what the tool in hand is set to".to_owned()),
                    (Tab::Tools, Icon::Tools, format!("Tools — the {kept} kept by name")),
                    (Tab::Find, Icon::Find, "Find — search the document (Ctrl+F)".to_owned()),
                    (Tab::Scale, Icon::Scale, "Scale — what this page measures at".to_owned()),
                ];
                for (tab, icon, hover) in tabs {
                    // Lit only while that side is actually showing: a tab
                    // remembered behind a collapsed panel isn't open.
                    let showing = self.tool_panel_open && self.tool_tab == tab;
                    if tool_button(ui, icon, Tone::Secondary, showing).on_hover_text(hover).clicked() {
                        self.toggle_tool_panel(tab);
                    }
                }
            });
        });
    }

    /// Opens the panel on `tab`, or collapses it if that side is already
    /// showing.
    pub(super) fn toggle_tool_panel(&mut self, tab: Tab) {
        if self.tool_panel_open && self.tool_tab == tab {
            self.tool_panel_open = false;
            // Putting the scale side away puts down the tools that belong to
            // it, and only those: a measurement tool in hand has nothing to
            // do with this side, and dropping it would be a surprise.
            if tab == Tab::Scale && matches!(self.measure_tool, Some(MeasureTool::Calibrate | MeasureTool::CalibrateVertical | MeasureTool::Verify)) {
                self.measure_tool = None;
            }
        } else {
            self.show_tool_panel(tab);
        }
    }

    /// Opens the panel on `tab`, and never collapses it. For the ways in that
    /// say where they are going rather than offering a switch: Ctrl+F, and
    /// the command palette, where running "Show scale" twice should leave the
    /// scale showing both times.
    pub(super) fn show_tool_panel(&mut self, tab: Tab) {
        self.tool_panel_open = true;
        self.tool_tab = tab;
    }

    /// The settings panel, beside the rail. It opens and closes only when
    /// asked -- from the rail, from the command palette, or from Ctrl+F for
    /// the find side. Nothing done on the page opens it: taking up a tool is
    /// a decision to draw, and drawing a measurement or picking one out is
    /// not a request to see its settings either. A panel that let itself in
    /// shifted the page sideways under the pointer between one click and the
    /// next, which is the worst possible moment while something is being
    /// measured.
    pub(super) fn tool_panel(&mut self, ui: &mut Ui) {
        let subject = self.subject();
        if !self.tool_panel_open {
            return;
        }
        egui::Panel::left(Id::new("tool-settings"))
            .frame(Frame::NONE.fill(SURFACE))
            .default_size(260.0)
            .min_size(200.0)
            .show(ui, |ui| {
                // No heading and nothing to close: the rail beside it says
                // which side is showing, and puts it away again.
                egui::CentralPanel::default().frame(Frame::NONE).show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("tool-settings-body").auto_shrink(false).show(ui, |ui| {
                        Frame::NONE.inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
                            match (self.tool_tab, subject) {
                                (Tab::Find, _) => self.find_body(ui),
                                (Tab::Scale, _) => self.scale_side(ui),
                                (Tab::Tools, _) => self.saved_tools_body(ui),
                                (Tab::Details, Some(subject)) => self.tool_body(ui, subject),
                                (Tab::Details, None) if self.picked_rows().len() > 1 => self.assorted_body(ui, &self.picked_rows()),
                                (Tab::Details, None) => {
                                    empty_note(ui, "Pick something out on the page, or take up a tool, to see what it is set to.")
                                }
                            }
                        });
                    });
                });
            });
    }

    pub(super) fn tool_title(&self, key: ToolKey) -> String {
        match key {
            ToolKey::Measure(tool) => tool.label().to_owned(),
            ToolKey::Draw(kind) => kind.label().to_owned(),
            ToolKey::Highlight => "Highlight".to_owned(),
        }
    }

    fn tool_body(&mut self, ui: &mut Ui, subject: Subject) {
        let key = subject.key();
        if key == ToolKey::Highlight {
            let before = self.tools.settings(key);
            let mut settings = before.clone();
            section(ui, "Highlight");
            colour_row(ui, "Colour", &mut settings.style.stroke, false);
            ui.add_space(6.0);
            section(ui, "Given to each one");
            field(ui, "Default note", &mut settings.defaults.description, "Optional note", false);
            if self.tools.is_changed(key) && styled_button(ui, "Back to defaults", Tone::Ghost, false).clicked() {
                self.tools.reset(key);
            } else if settings != before {
                self.tools.set(key, settings);
            }
            return;
        }
        // Edited as a copy and handed back when it differs, so every change
        // goes through one place whichever setting moved, and whether it is a
        // tool being set up or a measurement being changed.
        let mut mixed = Mixed::default();
        // Whether some of several are filled and some not, which is more than
        // their fills differing in colour.
        let mut fills_mixed = false;
        let before = match subject {
            Subject::Tool(key) => self.tools.settings(key),
            Subject::Measurement { id, .. } => match self.doc.as_ref().and_then(|d| d.session.measures().get(id)) {
                Some(markup) => ToolSettings::of_markup(markup),
                None => return,
            },
            Subject::Drawing { uid, .. } => match self.doc.as_ref().and_then(|d| d.session.markup(uid)) {
                Some(entry) => ToolSettings::of_drawing(&entry.markup),
                None => return,
            },
            // Several at once: the form shows the first of them, and marks
            // the settings they don't share as mixed.
            Subject::Many(_) => {
                // Each as the form will show it, so looking at them is no
                // change to any: see `as_shown`.
                let each: Vec<ToolSettings> = self.picked_rows().into_iter().filter_map(|id| self.row_settings(id)).map(as_shown).collect();
                let Some(first) = each.first() else { return };
                mixed = Mixed::of(&each);
                fills_mixed = each.iter().any(|s| s.style.fill.is_some() != first.style.fill.is_some());
                // A box can't be typed in with two things in it: one that
                // differs starts empty, and is left alone unless something is
                // typed there.
                let mut shown = first.clone();
                let d = &mut shown.defaults;
                for (text, differs) in [(&mut d.name, mixed.name), (&mut d.description, mixed.description), (&mut d.item_code, mixed.item_code), (&mut d.layer, mixed.layer)] {
                    if differs {
                        text.clear();
                    }
                }
                if mixed.depth_m {
                    shown.depth_m = None;
                }
                let count = each.len();
                section(ui, &format!("{count} {} picked out", plural(&self.tool_title(key), count)));
                ui.label(RichText::new("A change here goes to all of them. What they don't share shows as mixed.").size(11.5).color(SUBTLE));
                ui.add_space(4.0);
                shown
            }
        };
        let mut s = before.clone();

        // The rail of a slider takes its colour from the inactive widget fill,
        // which is the panel's own colour: left alone the sliders are all but
        // invisible. Given a rail that reads, and the part up to the handle
        // filled in, so the setting can be seen at a glance.
        ui.style_mut().visuals.widgets.inactive.bg_fill = INPUT_BORDER;
        ui.style_mut().visuals.slider_trailing_fill = true;

        // Three layers, in the order they are painted: the inside, whatever is
        // ruled over it, and the line round it.
        section(ui, "Line");
        colour_row(ui, "Colour", &mut s.style.stroke, mixed.stroke);
        // In pixels on screen, so a line stays the thickness it was set to
        // however far the drawing is zoomed. See `WidthUnit`.
        s.style.width_unit = WidthUnit::ScreenPixels;
        slider_row(ui, "Thickness", &mut s.style.width, 0.5..=12.0, "px", mixed.width);
        opacity_row(ui, "Opacity", &mut s.style.opacity, mixed.opacity);

        if key.fills() {
            ui.add_space(6.0);
            let mut filled = s.style.fill.is_some();
            section_toggle(ui, "Fill", &mut filled, fills_mixed);
            if filled {
                let mut rgb = s.style.fill.unwrap_or(s.style.stroke);
                colour_row(ui, "Colour", &mut rgb, mixed.fill && !fills_mixed);
                opacity_row(ui, "Opacity", &mut s.style.fill_opacity, mixed.fill_opacity);
                s.style.fill = Some(rgb);
            } else {
                s.style.fill = None;
            }

            if s.style.fill.is_some() {
                ui.add_space(6.0);
                section(ui, "Pattern");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    for pattern in FillPattern::ALL {
                        // None lit when they differ, as no one is theirs.
                        if styled_button(ui, pattern.label(), Tone::Secondary, !mixed.pattern && s.style.pattern == pattern).clicked() {
                            s.style.pattern = pattern;
                        }
                    }
                    if mixed.pattern {
                        mixed_mark(ui);
                    }
                });
                // Its own colour and transparency, over the fill: a grey hatch
                // on a pale fill is the usual way a take-off is marked up.
                if s.style.pattern.is_ruled() {
                    let mut rgb = s.style.pattern_colour.unwrap_or(s.style.stroke);
                    colour_row(ui, "Colour", &mut rgb, mixed.pattern_colour);
                    s.style.pattern_colour = Some(rgb);
                    opacity_row(ui, "Opacity", &mut s.style.pattern_opacity, mixed.pattern_opacity);
                    // In points on the page, the cell the file's own pattern
                    // repeats, so the hatch on screen is the hatch in the file.
                    slider_row(ui, "Size", &mut s.style.pattern_size, 2.0..=30.0, "pt", mixed.pattern_size);
                }
            }
        }

        // The quantity written on the drawing. It is set straight on the page
        // rather than in a card of its own, so what is on screen is what the
        // file gets. Only a measurement writes one: a pen stroke or a box has
        // no number to put beside it.
        if subject.measures() {
            ui.add_space(6.0);
            section(ui, "Quantity");
            row(ui, "Face", |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for face in LabelFont::ALL {
                        if styled_button(ui, face.label(), Tone::Secondary, !mixed.label_font && s.style.label_font == face).clicked() {
                            s.style.label_font = face;
                        }
                    }
                    if mixed.label_font {
                        mixed_mark(ui);
                    }
                });
            });
            {
                let mut rgb = s.style.label_colour.unwrap_or(s.style.stroke);
                colour_row(ui, "Colour", &mut rgb, mixed.label_colour);
                s.style.label_colour = Some(rgb);
            }
            // In points on the page, as the file writes it, so it scales with the
            // drawing the way printed text does.
            slider_row(ui, "Size", &mut s.style.label_size, 4.0..=48.0, "pt", mixed.label_size);
        }

        // What every measurement drawn with this tool is called and filed
        // under before anyone types anything. A drawing carries the first two
        // of these, which are what the quantities list shows it under.
        ui.add_space(4.0);
        section(ui, if matches!(subject, Subject::Tool(_)) { "Given to each one" } else { "In the list" });
        field(ui, "Name", &mut s.defaults.name, "What it's called", mixed.name);
        field(ui, "Description", &mut s.defaults.description, "What it's priced as", mixed.description);
        if key.takes_depth() {
            ui.add_space(4.0);
            section(ui, "How it measures");
        }
        if key.takes_depth() {
            // An area with a depth is priced by volume, so setting it here
            // saves typing it into every row of the table.
            let mut text = s.depth_m.map_or(String::new(), |m| format_length(m, LengthUnit::Metre, Precision::Decimals(3)));
            let hint = if mixed.depth_m { MIXED } else { "e.g. 200 mm" };
            if labelled(ui, "Depth", &mut text, hint).changed() {
                s.depth_m = markup_model::units::parse_length(text.trim(), Some(LengthUnit::Metre)).ok().filter(|m| *m > 0.0);
            }
            ui.label(RichText::new("An area with a depth is measured as a volume.").size(11.5).color(SUBTLE));
        }
        match subject {
            Subject::Tool(key) => {
                if self.tools.is_changed(key) {
                    ui.add_space(6.0);
                    ui.separator();
                    if styled_button(ui, "Back to defaults", Tone::Ghost, false).clicked() {
                        self.tools.reset(key);
                        return;
                    }
                }
                if s != before {
                    self.tools.set(key, s);
                }
            }
            Subject::Measurement { id, .. } => {
                ui.add_space(6.0);
                ui.separator();
                // Changing the tool from here is how a measurement already
                // drawn becomes the pattern for the next one.
                if styled_button(ui, "Make this the tool's setting", Tone::Ghost, false)
                    .on_hover_text("Draw the next one like this one")
                    .clicked()
                {
                    self.tools.set(key, s.clone());
                }
                if s != before {
                    self.change_measurement(id, &s);
                }
            }
            Subject::Drawing { uid, .. } => {
                ui.add_space(6.0);
                ui.separator();
                if styled_button(ui, "Make this the tool's setting", Tone::Ghost, false)
                    .on_hover_text("Draw the next one like this one")
                    .clicked()
                {
                    self.tools.set(key, s.clone());
                }
                // One already written carries its own appearance in the file,
                // which nothing here rewrites: see `Command::Restyle`.
                if self.doc.as_ref().and_then(|d| d.session.markup(uid)).is_some_and(|e| e.markup.key.is_some()) {
                    ui.label(
                        RichText::new("Already saved into the file, so how it looks can't be changed here.")
                            .size(11.5)
                            .color(SUBTLE),
                    );
                } else if s != before {
                    self.change_drawing(uid, &s);
                }
            }
            Subject::Many(_) => {
                let picked = self.picked_rows();
                let saved = picked
                    .iter()
                    .filter(|id| matches!(id, RowId::Drawing(uid) if self.doc.as_ref().and_then(|d| d.session.markup(*uid)).is_some_and(|e| e.markup.key.is_some())))
                    .count();
                if saved > 0 {
                    ui.add_space(6.0);
                    ui.separator();
                    let those = if saved == 1 { "One of these is".to_owned() } else { format!("{saved} of these are") };
                    ui.label(RichText::new(format!("{those} already saved into the file, so can't be changed here.")).size(11.5).color(SUBTLE));
                }
                if s != before {
                    self.change_many(&picked, &before, &s);
                }
            }
        }
    }

    /// Puts changed settings on a markup already drawn, as one undoable step.
    pub(super) fn change_drawing(&mut self, uid: u64, settings: &ToolSettings) {
        if let Some(restyle) = self.restyled(uid, settings) {
            self.apply_change(restyle);
        }
    }

    /// Puts changed settings on a measurement already drawn, as one undoable
    /// step like any other change to it.
    pub(super) fn change_measurement(&mut self, id: MarkupId, settings: &ToolSettings) {
        if let Some(change) = self.remeasured(id, settings) {
            self.apply_change(change);
        }
    }

    /// Carries what changed between `before` and `after` onto each of several
    /// things picked out -- only that, so what they don't share stays as it
    /// was on each -- as one step to undo, however many there are.
    pub(super) fn change_many(&mut self, ids: &[RowId], before: &ToolSettings, after: &ToolSettings) {
        self.change_each(ids, |own| {
            own.carry(before, after);
            // A thickness is set in the form's own unit, whatever the one it
            // replaces was in.
            if after.style.width != before.style.width {
                own.style.width_unit = after.style.width_unit;
            }
        });
    }

    /// Changes the settings of each of several things, as one step to undo.
    pub(super) fn change_each(&mut self, ids: &[RowId], change: impl Fn(&mut ToolSettings)) {
        let mut commands = Vec::new();
        for &id in ids {
            let Some(mut own) = self.row_settings(id) else { continue };
            change(&mut own);
            let command = match id {
                RowId::Measure(id) => self.remeasured(id, &own),
                RowId::Drawing(uid) => self.restyled(uid, &own),
                RowId::Note(_) => None,
            };
            commands.extend(command);
        }
        self.apply_change(crate::session::Command::Batch(commands));
    }

    /// The change that puts these settings on a drawn markup.
    fn restyled(&self, uid: u64, settings: &ToolSettings) -> Option<crate::session::Command> {
        let entry = self.doc.as_ref()?.session.markup(uid)?;
        let mut markup = entry.markup.clone();
        settings.apply_to_drawing(&mut markup);
        let look = crate::session::Look {
            color: markup.color,
            width: markup.width,
            style: markup.style,
            name: markup.name,
            comment: markup.comment,
        };
        Some(crate::session::Command::Restyle { uid, look: Box::new(look) })
    }

    /// The change that puts these settings on a measurement.
    fn remeasured(&self, id: MarkupId, settings: &ToolSettings) -> Option<crate::session::Command> {
        let mut changed = self.doc.as_ref()?.session.measures().get(id)?.clone();
        settings.apply(&mut changed);
        Some(crate::session::Command::ChangeMeasure(Box::new(changed)))
    }

    fn apply_change(&mut self, command: crate::session::Command) {
        if let Some(doc) = self.doc.as_mut() {
            doc.session.apply(command);
        }
    }
}

/// What a setting shows when the things picked out differ in it.
const MIXED: &str = "mixed";

/// The word beside a control whose things picked out differ in it: the control
/// shows the first of them, and changing it sets them all.
fn mixed_mark(ui: &mut Ui) {
    ui.label(RichText::new(MIXED).size(11.5).italics().color(SUBTLE));
}

/// Settings as the form shows them, where it fills in what is left to follow
/// something else: a quantity or a ruling in the line's colour, a thickness in
/// pixels on screen. Several picked out are compared and changed from this,
/// so that showing them isn't itself a change -- which would give every label
/// that follows its own line the colour of the first one's line.
fn as_shown(mut s: ToolSettings) -> ToolSettings {
    s.style.width_unit = WidthUnit::ScreenPixels;
    s.style.label_colour = Some(s.style.label_colour.unwrap_or(s.style.stroke));
    s.style.pattern_colour = Some(s.style.pattern_colour.unwrap_or(s.style.stroke));
    s
}

/// "Area" or "3 Areas": a kind of thing, as many times as there are.
fn plural(word: &str, count: usize) -> String {
    match count {
        1 => word.to_owned(),
        _ if word.ends_with(['s', 'x']) || word.ends_with("sh") || word.ends_with("ch") => format!("{word}es"),
        _ => format!("{word}s"),
    }
}

fn section(ui: &mut Ui, name: &str) {
    ui.label(RichText::new(name).size(11.5).strong().color(MUTED));
}

/// A labelled text box that fills the panel's width. One whose things picked
/// out differ starts empty, and says so.
fn field(ui: &mut Ui, name: &str, text: &mut String, hint: &str, mixed: bool) {
    ui.label(RichText::new(name).size(12.0).color(MUTED));
    let hint = if mixed { MIXED } else { hint };
    ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(f32::INFINITY));
}

/// The same, side by side with its label, for the short number boxes.
fn labelled(ui: &mut Ui, name: &str, text: &mut String, hint: &str) -> egui::Response {
    ui.label(RichText::new(name).size(12.0).color(MUTED));
    ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(80.0))
}

/// A row of the panel: what it sets on the left, the control on the right, so
/// the controls line up down the panel however wide their labels are.
fn row<R>(ui: &mut Ui, name: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_sized([72.0, 20.0], egui::Label::new(RichText::new(name).size(12.0).color(MUTED)).halign(Align::Min));
        out = Some(control(ui));
    });
    out.expect("the row draws its control")
}

fn colour_row(ui: &mut Ui, name: &str, rgb: &mut Rgb, mixed: bool) {
    row(ui, name, |ui| {
        let mut colour = to_color32(*rgb);
        if ui.color_edit_button_srgba(&mut colour).changed() {
            *rgb = from_color32(colour);
        }
        if mixed {
            mixed_mark(ui);
        }
    });
}

fn slider_row(ui: &mut Ui, name: &str, value: &mut f64, range: std::ops::RangeInclusive<f64>, suffix: &str, mixed: bool) {
    row(ui, name, |ui| {
        ui.add(egui::Slider::new(value, range).suffix(suffix).fixed_decimals(1));
        if mixed {
            mixed_mark(ui);
        }
    });
}

/// Shown as a percentage, which reads better than a hundredth of a unit.
fn opacity_row(ui: &mut Ui, name: &str, value: &mut f32, mixed: bool) {
    row(ui, name, |ui| {
        let mut percent = f64::from(*value) * 100.0;
        if ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%").fixed_decimals(0)).changed() {
            *value = (percent / 100.0) as f32;
        }
        if mixed {
            mixed_mark(ui);
        }
    });
}

/// A section heading that can be turned off, for the parts of a shape that
/// needn't be drawn at all. Neither ticked nor clear when some of the things
/// picked out have it and some don't.
fn section_toggle(ui: &mut Ui, name: &str, on: &mut bool, mixed: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add(egui::Checkbox::new(on, RichText::new(name).size(11.5).strong().color(MUTED)).indeterminate(mixed));
    });
}


/// A row of a list: the full width of the panel, lit as the pointer passes,
/// with no button drawn round it.
///
/// The closure gives back the area inside the row that takes its own clicks,
/// and the row stops short of it. Without that the row's own interaction,
/// registered last and over the whole width, swallows the clicks meant for the
/// button sitting in it.
fn list_row(ui: &mut Ui, id: Id, selected: bool, add: impl FnOnce(&mut Ui) -> Option<Rect>) -> bool {
    let background = ui.painter().add(Shape::Noop);
    let mut its_own = None;
    let inner = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(4.0);
        its_own = add(ui);
    });
    let rect = Rect::from_min_max(
        pos2(ui.min_rect().left(), inner.response.rect.top() - 3.0),
        pos2(ui.min_rect().right(), inner.response.rect.bottom() + 3.0),
    );
    let mine = match its_own {
        Some(theirs) => Rect::from_min_max(rect.min, pos2((theirs.left() - 4.0).max(rect.left()), rect.max.y)),
        None => rect,
    };
    let response = ui.interact(mine, id, Sense::click());
    let fill = if selected {
        ACCENT_SOFT
    } else if response.hovered() {
        HOVER_FILL
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().set(background, Shape::rect_filled(rect, CornerRadius::same(6), fill));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.clicked()
}

/// What a tool draws, in miniature: its fill, whatever is ruled over it, and
/// the line round the outside. Ruled by the same code the drawing uses, so the
/// hatch in the list is the hatch on the page.
fn tool_preview(ui: &mut Ui, settings: &ToolSettings, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let style = &settings.style;
    if let Some(fill) = style.fill {
        painter.rect_filled(rect, CornerRadius::same(3), to_color32(fill).gamma_multiply(style.fill_opacity));
        if style.pattern.is_ruled() {
            let ring = vec![rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
            let colour = to_color32(style.pattern_colour.unwrap_or(style.stroke)).gamma_multiply(style.pattern_opacity);
            let spacing = 4.0;
            for [from, to] in measure::hatch(std::slice::from_ref(&ring), rect, style.pattern, spacing) {
                if style.pattern == FillPattern::Dots {
                    let steps = ((to - from).length() / spacing).floor() as i32;
                    for step in 0..=steps {
                        painter.circle_filled(from + (to - from).normalized() * (step as f32 * spacing), 0.9, colour);
                    }
                } else {
                    painter.line_segment([from, to], Stroke::new(1.0, colour));
                }
            }
        }
    }
    painter.rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.5, to_color32(style.stroke)), StrokeKind::Inside);
}

fn saved_tool_action(ui: &mut Ui, icon: Icon, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(5), HOVER_FILL);
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if response.has_focus() {
        ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, ACCENT), StrokeKind::Inside);
    }
    icons::paint(ui.painter(), rect.shrink(4.0), icon, MUTED);
    response.on_hover_text(hint)
}

/// The triangle beside a group: along when it is rolled up, down when open.
pub(super) fn caret(painter: &egui::Painter, rect: Rect, rolled: bool) {
    let c = rect.center();
    let r = rect.width() * 0.34;
    let points = if rolled {
        vec![pos2(c.x - r * 0.6, c.y - r), pos2(c.x + r * 0.8, c.y), pos2(c.x - r * 0.6, c.y + r)]
    } else {
        vec![pos2(c.x - r, c.y - r * 0.6), pos2(c.x + r, c.y - r * 0.6), pos2(c.x, c.y + r * 0.8)]
    };
    painter.add(Shape::convex_polygon(points, MUTED, Stroke::NONE));
}

/// A draft stays separate from the saved tool until Create or Save is pressed.
pub(super) struct ToolCreator {
    editing: Option<usize>,
    was_active: bool,
    name: String,
    group: String,
    new_group: bool,
    group_open: bool,
    kind_open: bool,
    group_search: String,
    kind_search: String,
    group_selected: usize,
    kind_selected: usize,
    focus_group_search: bool,
    focus_kind_search: bool,
    focus_new_group: bool,
    key: ToolKey,
    settings: ToolSettings,
    depth_text: String,
    focus_name: bool,
}

impl ToolCreator {
    fn new() -> Self {
        let key = ToolKey::Measure(MeasureTool::Area);
        Self {
            editing: None, was_active: false,
            name: String::new(), group: String::new(), new_group: false,
            group_open: false, kind_open: false, group_search: String::new(), kind_search: String::new(),
            group_selected: 0, kind_selected: 0,
            focus_group_search: false, focus_kind_search: false, focus_new_group: false,
            key, settings: ToolSettings::new(key), depth_text: String::new(), focus_name: true,
        }
    }

    fn edit(at: usize, tool: &tools::SavedTool, was_active: bool) -> Option<Self> {
        let key = tool.key()?;
        let depth_text = tool.settings.depth_m.map_or(String::new(), |m| format_length(m, LengthUnit::Metre, Precision::Decimals(3)));
        Some(Self {
            editing: Some(at), was_active,
            name: tool.name.clone(), group: tool.group.clone(), new_group: false,
            group_open: false, kind_open: false, group_search: String::new(), kind_search: String::new(),
            group_selected: 0, kind_selected: 0,
            focus_group_search: false, focus_kind_search: false, focus_new_group: false,
            key, settings: tool.settings.clone(), depth_text, focus_name: true,
        })
    }

    fn depth_m(&self) -> Option<Option<f64>> {
        if self.depth_text.trim().is_empty() {
            Some(None)
        } else {
            markup_model::units::parse_length(self.depth_text.trim(), Some(LengthUnit::Metre)).ok().filter(|m| *m > 0.0).map(Some)
        }
    }

    fn can_submit(&self, tools: &Tools) -> bool {
        !self.name.trim().is_empty()
            && (!self.new_group || !self.group.trim().is_empty())
            && !tools.has_saved_name_except(&self.name, &self.group, self.editing)
            && self.depth_m().is_some()
    }
}

#[derive(Clone)]
enum CreatorGroupChoice {
    New,
    None,
    Existing(String),
}

fn creator_picker_button(ui: &mut Ui, label: &str, icon: Option<Icon>) -> egui::Response {
    let caption = if icon.is_some() { format!("    {label}") } else { label.to_owned() };
    let response = ui.add_sized([ui.available_width(), 28.0], egui::Button::new(caption));
    if let Some(icon) = icon {
        icons::paint(ui.painter(), Rect::from_min_size(response.rect.min + vec2(10.0, 5.0), vec2(18.0, 18.0)), icon, MUTED);
    }
    let center = pos2(response.rect.right() - 16.0, response.rect.center().y);
    ui.painter().add(Shape::convex_polygon(
        vec![center + vec2(-4.0, -2.0), center + vec2(4.0, -2.0), center + vec2(0.0, 3.0)],
        MUTED,
        Stroke::NONE,
    ));
    response
}

/// A small drawing made with the draft's current style.
fn creator_preview(ui: &mut Ui, key: ToolKey, settings: &ToolSettings, name: &str, icon: Icon) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Preview").size(11.5).strong().color(MUTED));
        if !name.trim().is_empty() {
            ui.label(RichText::new(name.trim()).size(11.5).color(SUBTLE));
        }
    });
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 88.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(8), BG);
    painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, BORDER), StrokeKind::Inside);
    let sample = Rect::from_min_max(rect.min + vec2(28.0, 13.0), pos2(rect.min.x + rect.width() * 0.60, rect.max.y - 13.0));
    let at = |x: f32, y: f32| pos2(sample.left() + x * sample.width(), sample.top() + y * sample.height());
    let style = &settings.style;
    let ink = to_color32(style.stroke).gamma_multiply(style.opacity);
    let stroke = Stroke::new(style.width as f32, ink);
    let fill = style.fill.map(|rgb| measure::Fill {
        colour: to_color32(rgb).gamma_multiply(style.fill_opacity),
        pattern: style.pattern,
        ruling: to_color32(style.pattern_colour.unwrap_or(style.stroke)).gamma_multiply(style.pattern_opacity),
        cell: style.pattern_size,
    });
    let polygon = match key {
        ToolKey::Measure(MeasureTool::Area | MeasureTool::Cutout) => Some(vec![at(0.10, 0.80), at(0.25, 0.20), at(0.77, 0.13), at(0.91, 0.68), at(0.55, 0.88)]),
        ToolKey::Draw(MarkupKind::Rectangle) => Some(vec![at(0.13, 0.20), at(0.86, 0.20), at(0.86, 0.80), at(0.13, 0.80)]),
        ToolKey::Draw(MarkupKind::Ellipse) => Some((0..48).map(|i| {
            let angle = i as f32 * std::f32::consts::TAU / 48.0;
            at(0.5 + 0.35 * angle.cos(), 0.5 + 0.37 * angle.sin())
        }).collect()),
        _ => None,
    };
    if let Some(points) = polygon {
        painter.add(Shape::convex_polygon(points.clone(), fill.map_or(Color32::TRANSPARENT, |f| f.colour), stroke));
        if let Some(fill) = fill.filter(|f| f.pattern.is_ruled()) {
            measure::paint_pattern(&painter, &[points], fill, 1.0);
        }
    } else {
        match key {
            ToolKey::Highlight => {
                let band = Rect::from_min_max(at(0.13, 0.24), at(0.87, 0.76));
                painter.rect_filled(band, CornerRadius::same(2), ink);
                painter.text(band.center(), Align2::CENTER_CENTER, "Sample text", FontId::proportional(17.0), Color32::BLACK);
            }
            ToolKey::Measure(MeasureTool::Count) => {
                for (x, y) in [(0.22, 0.35), (0.48, 0.70), (0.77, 0.31)] {
                    let p = at(x, y);
                    painter.line_segment([p - vec2(7.0, 7.0), p + vec2(7.0, 7.0)], stroke);
                    painter.line_segment([p + vec2(-7.0, 7.0), p + vec2(7.0, -7.0)], stroke);
                }
            }
            ToolKey::Measure(MeasureTool::Radius | MeasureTool::Diameter) => {
                let center = at(0.50, 0.50);
                painter.circle_stroke(center, sample.height() * 0.37, stroke);
                let from = if key == ToolKey::Measure(MeasureTool::Diameter) { at(0.24, 0.72) } else { center };
                painter.line_segment([from, at(0.76, 0.28)], stroke);
            }
            ToolKey::Measure(MeasureTool::Angle) => {
                painter.line_segment([at(0.18, 0.76), at(0.47, 0.76)], stroke);
                painter.line_segment([at(0.47, 0.76), at(0.80, 0.16)], stroke);
            }
            ToolKey::Measure(MeasureTool::Polylength) | ToolKey::Draw(MarkupKind::Pen) => {
                painter.add(Shape::line(vec![at(0.12, 0.75), at(0.32, 0.26), at(0.58, 0.65), at(0.86, 0.20)], stroke));
            }
            ToolKey::Draw(MarkupKind::Arrow) => {
                let tip = at(0.86, 0.22);
                painter.line_segment([at(0.13, 0.78), tip], stroke);
                painter.line_segment([at(0.68, 0.20), tip], stroke);
                painter.line_segment([at(0.78, 0.43), tip], stroke);
            }
            _ => { painter.line_segment([at(0.13, 0.76), at(0.86, 0.24)], stroke); }
        }
    }
    let icon_rect = Rect::from_center_size(pos2(rect.min.x + rect.width() * 0.77, rect.center().y - 15.0), vec2(24.0, 24.0));
    icons::paint(&painter, icon_rect, icon, MUTED);
    if let ToolKey::Measure(tool) = key {
        let text = match tool {
            MeasureTool::Area | MeasureTool::Cutout => "12.4 m²",
            MeasureTool::Count => "3",
            MeasureTool::Angle => "42°",
            MeasureTool::Radius | MeasureTool::Diameter => "2.4 m",
            _ => "5.2 m",
        };
        let font = match style.label_font {
            LabelFont::Sans => FontId::proportional(style.label_size as f32),
            LabelFont::Mono => FontId::monospace(style.label_size as f32),
        };
        painter.text(pos2(icon_rect.center().x, rect.center().y + 20.0), Align2::CENTER_CENTER, text, font,
            to_color32(style.label_colour.unwrap_or(style.stroke)));
    }
}

fn creator_text(ui: &mut Ui, text: &mut String, hint: &str) -> egui::Response {
    ui.add_sized([ui.available_width(), 28.0], egui::TextEdit::singleline(text).hint_text(hint).vertical_align(Align::Center))
}

fn creator_field(ui: &mut Ui, label: &str, text: &mut String, hint: &str) {
    ui.label(RichText::new(label).size(12.0).color(MUTED));
    creator_text(ui, text, hint);
}

fn move_creator_selection(selected: &mut usize, len: usize, up: bool, down: bool) {
    if len == 0 { return; }
    if down { *selected = (*selected + 1) % len; }
    if up { *selected = (*selected + len - 1) % len; }
}

fn creator_settings(ui: &mut Ui, settings: &mut ToolSettings, key: ToolKey, depth_text: &mut String) {
    if key == ToolKey::Highlight {
        section(ui, "Highlight");
        colour_row(ui, "Colour", &mut settings.style.stroke, false);
        ui.add_space(10.0);
        section(ui, "Given to each one");
        creator_field(ui, "Default note", &mut settings.defaults.description, "Optional note");
        return;
    }
    settings.style.width_unit = WidthUnit::ScreenPixels;
    section(ui, "Line");
    colour_row(ui, "Colour", &mut settings.style.stroke, false);
    slider_row(ui, "Thickness", &mut settings.style.width, 0.5..=12.0, "px", false);
    opacity_row(ui, "Opacity", &mut settings.style.opacity, false);

    if key.fills() {
        ui.add_space(10.0);
        let mut filled = settings.style.fill.is_some();
        section_toggle(ui, "Fill", &mut filled, false);
        if filled {
            let mut rgb = settings.style.fill.unwrap_or(settings.style.stroke);
            colour_row(ui, "Colour", &mut rgb, false);
            opacity_row(ui, "Opacity", &mut settings.style.fill_opacity, false);
            settings.style.fill = Some(rgb);
            ui.add_space(8.0);
            section(ui, "Pattern");
            ui.horizontal_wrapped(|ui| {
                for pattern in FillPattern::ALL {
                    if styled_button(ui, pattern.label(), Tone::Secondary, settings.style.pattern == pattern).clicked() {
                        settings.style.pattern = pattern;
                    }
                }
            });
            if settings.style.pattern.is_ruled() {
                let mut rgb = settings.style.pattern_colour.unwrap_or(settings.style.stroke);
                colour_row(ui, "Colour", &mut rgb, false);
                settings.style.pattern_colour = Some(rgb);
                opacity_row(ui, "Opacity", &mut settings.style.pattern_opacity, false);
                slider_row(ui, "Size", &mut settings.style.pattern_size, 2.0..=30.0, "pt", false);
            }
        } else {
            settings.style.fill = None;
        }
    }

    if matches!(key, ToolKey::Measure(_)) {
        ui.add_space(10.0);
        section(ui, "Quantity label");
        row(ui, "Face", |ui| {
            for face in LabelFont::ALL {
                if styled_button(ui, face.label(), Tone::Secondary, settings.style.label_font == face).clicked() {
                    settings.style.label_font = face;
                }
            }
        });
        let mut rgb = settings.style.label_colour.unwrap_or(settings.style.stroke);
        colour_row(ui, "Colour", &mut rgb, false);
        settings.style.label_colour = Some(rgb);
        slider_row(ui, "Size", &mut settings.style.label_size, 4.0..=48.0, "pt", false);
    }

    ui.add_space(10.0);
    section(ui, "Given to each one");
    creator_field(ui, "Description", &mut settings.defaults.description, "What it's priced as");
    if key.takes_depth() {
        ui.add_space(10.0);
        section(ui, "How it measures");
        ui.label(RichText::new("Depth").size(12.0).color(MUTED));
        creator_text(ui, depth_text, "e.g. 200 mm");
        ui.label(RichText::new("An area with a depth is measured as a volume.").size(11.5).color(SUBTLE));
    }
}

impl App {
    pub(super) fn open_tool_creator(&mut self) {
        self.tool_creator = Some(ToolCreator::new());
    }

    pub(super) fn open_tool_creator_from(&mut self, key: ToolKey, settings: ToolSettings, name: String) {
        let mut draft = ToolCreator::new();
        draft.key = key;
        draft.depth_text = settings.depth_m.map_or(String::new(), |m| format_length(m, LengthUnit::Metre, Precision::Decimals(3)));
        draft.settings = settings;
        draft.name = name;
        self.tool_creator = Some(draft);
    }

    fn open_tool_editor(&mut self, at: usize) {
        self.tool_creator = self.tools.saved_tool(at).and_then(|tool| {
            let key = tool.key()?;
            let was_active = self.held_tool() == Some(key) && self.tools.settings(key) == tool.settings;
            ToolCreator::edit(at, tool, was_active)
        });
    }

    fn submit_tool(&mut self, mut draft: ToolCreator) {
        if !draft.can_submit(&self.tools) { return; }
        let name = draft.name.trim().to_owned();
        let group = draft.group.trim().to_owned();
        draft.settings.depth_m = draft.depth_m().flatten();
        if let Some(at) = draft.editing {
            if self.tools.update_tool(at, &name, &group, draft.key, draft.settings) {
                if draft.was_active { self.take_up_saved(at); }
                self.toast(format!("Updated tool {name}"));
            }
            return;
        }
        self.tools.save_tool(&name, &group, draft.key, draft.settings);
        if self.doc.is_some() { self.take_up_saved(self.tools.saved_count() - 1); }
        self.show_tool_panel(Tab::Tools);
        self.toast(format!("Created tool {name}"));
    }

    pub(super) fn show_tool_creator(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.tool_creator.take() else { return };
        let groups: Vec<String> = self.tools.groups().into_iter().map(|(name, _)| name).filter(|name| !name.is_empty()).collect();
        let mut create = false;
        let mut cancel = false;
        let response = egui::Modal::new(Id::new("tool-creator"))
            .frame(Frame::NONE.fill(SURFACE).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(20)))
            .show(ctx, |ui| {
                ui.set_width(540.0);
                ui.style_mut().visuals.widgets.inactive.bg_fill = INPUT_BORDER;
                ui.style_mut().visuals.slider_trailing_fill = true;
                creator_preview(ui, draft.key, &draft.settings, &draft.name, self.key_icon(draft.key));
                ui.add_space(12.0);
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 190.0).max(250.0)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    section(ui, "Tool");
                    ui.label(RichText::new("Name").size(12.0).color(MUTED));
                    let name = creator_text(ui, &mut draft.name, "Concrete slab 200");
                    if std::mem::take(&mut draft.focus_name) { name.request_focus(); }
                    ui.label(RichText::new("Group").size(12.0).color(MUTED));
                    let group_label = if draft.new_group { "New group" } else if draft.group.is_empty() { "No group" } else { &draft.group };
                    let group_button = creator_picker_button(ui, group_label, None);
                    let mut group_opened = false;
                    if group_button.clicked() {
                        draft.group_open = !draft.group_open;
                        draft.kind_open = false;
                        group_opened = draft.group_open;
                        draft.group_search.clear();
                        draft.group_selected = 0;
                        draft.focus_group_search = draft.group_open;
                    }
                    let mut group_open = draft.group_open;
                    let mut group_choice = None;
                    egui::Popup::menu(&group_button)
                        .open_bool(&mut group_open)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .width(group_button.rect.width())
                        .show(|ui| {
                            ui.set_min_width(group_button.rect.width());
                            let search_id = ui.make_persistent_id("creator-group-search");
                            let focus_before = ui.memory(|m| m.focused());
                            let (up, down, enter) = if focus_before == Some(search_id) {
                                ui.input_mut(|i| (
                                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                                    i.consume_key(Modifiers::NONE, Key::Enter),
                                ))
                            } else { (false, false, false) };
                            let search = ui.add_sized([ui.available_width(), 28.0], egui::TextEdit::singleline(&mut draft.group_search)
                                .id(search_id).hint_text("Search groups…").vertical_align(Align::Center));
                            if std::mem::take(&mut draft.focus_group_search) { search.request_focus(); }
                            ui.memory_mut(|m| m.set_focus_lock_filter(search.id, egui::EventFilter { vertical_arrows: true, ..Default::default() }));
                            if search.changed() { draft.group_selected = 0; }
                            let query = draft.group_search.trim().to_lowercase();
                            let mut options = vec![CreatorGroupChoice::New];
                            if query.is_empty() { options.push(CreatorGroupChoice::None); }
                            options.extend(groups.iter().filter(|group| group.to_lowercase().contains(&query))
                                .cloned().map(CreatorGroupChoice::Existing));
                            draft.group_selected = draft.group_selected.min(options.len() - 1);
                            move_creator_selection(&mut draft.group_selected, options.len(), up, down);
                            if up || down { search.request_focus(); }
                            if !group_opened && enter {
                                group_choice = options.get(draft.group_selected).cloned();
                            }
                            ui.separator();
                            egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                                for (i, option) in options.iter().enumerate() {
                                    let label = match option {
                                        CreatorGroupChoice::New => "Create new group…",
                                        CreatorGroupChoice::None => "No group",
                                        CreatorGroupChoice::Existing(group) => group,
                                    };
                                    let response = ui.selectable_label(i == draft.group_selected, label);
                                    if (up || down) && i == draft.group_selected { response.scroll_to_me(None); }
                                    if response.clicked() { group_choice = Some(option.clone()); }
                                }
                            });
                        });
                    draft.group_open = group_open;
                    if let Some(choice) = group_choice {
                        match choice {
                            CreatorGroupChoice::New => {
                                draft.group = draft.group_search.trim().to_owned();
                                draft.new_group = true;
                                draft.focus_new_group = true;
                            }
                            CreatorGroupChoice::None => { draft.group.clear(); draft.new_group = false; }
                            CreatorGroupChoice::Existing(group) => { draft.group = group; draft.new_group = false; }
                        }
                        draft.group_open = false;
                        if !draft.new_group { group_button.request_focus(); }
                    }
                    if draft.new_group {
                        let group_name = creator_text(ui, &mut draft.group, "Group name");
                        if std::mem::take(&mut draft.focus_new_group) { group_name.request_focus(); }
                    }
                    ui.label(RichText::new("Type").size(12.0).color(MUTED));
                    let kind_button = creator_picker_button(ui, &self.tool_title(draft.key), Some(self.key_icon(draft.key)));
                    let mut kind_opened = false;
                    if kind_button.clicked() {
                        draft.kind_open = !draft.kind_open;
                        draft.group_open = false;
                        kind_opened = draft.kind_open;
                        draft.kind_search.clear();
                        draft.kind_selected = 0;
                        draft.focus_kind_search = draft.kind_open;
                    }
                    let mut kind_open = draft.kind_open;
                    let mut kind_choice = None;
                    egui::Popup::menu(&kind_button)
                        .open_bool(&mut kind_open)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .width(kind_button.rect.width())
                        .show(|ui| {
                            ui.set_min_width(kind_button.rect.width());
                            let search_id = ui.make_persistent_id("creator-kind-search");
                            let focus_before = ui.memory(|m| m.focused());
                            let (up, down, enter) = if focus_before == Some(search_id) {
                                ui.input_mut(|i| (
                                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                                    i.consume_key(Modifiers::NONE, Key::Enter),
                                ))
                            } else { (false, false, false) };
                            let search = ui.add_sized([ui.available_width(), 28.0], egui::TextEdit::singleline(&mut draft.kind_search)
                                .id(search_id).hint_text("Search types…").vertical_align(Align::Center));
                            if std::mem::take(&mut draft.focus_kind_search) { search.request_focus(); }
                            ui.memory_mut(|m| m.set_focus_lock_filter(search.id, egui::EventFilter { vertical_arrows: true, ..Default::default() }));
                            if search.changed() { draft.kind_selected = 0; }
                            let query = draft.kind_search.trim().to_lowercase();
                            let matching: Vec<ToolKey> = SAVABLE_MEASURE_TOOLS.into_iter().map(ToolKey::Measure)
                                .chain(MarkupKind::TOOLS.into_iter().map(ToolKey::Draw))
                                .chain([ToolKey::Highlight])
                                .filter(|key| self.tool_title(*key).to_lowercase().contains(&query)).collect();
                            if !matching.is_empty() {
                                draft.kind_selected = draft.kind_selected.min(matching.len() - 1);
                                move_creator_selection(&mut draft.kind_selected, matching.len(), up, down);
                            } else { draft.kind_selected = 0; }
                            if up || down { search.request_focus(); }
                            if !kind_opened && enter {
                                kind_choice = matching.get(draft.kind_selected).copied();
                            }
                            ui.separator();
                            egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                                if matching.is_empty() {
                                    ui.label(RichText::new("No matching type").size(12.0).color(MUTED));
                                }
                                for (i, key) in matching.into_iter().enumerate() {
                                    let response = ui.horizontal(|ui| {
                                        let (icon_rect, icon_response) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                                        icons::paint(ui.painter(), icon_rect, self.key_icon(key), MUTED);
                                        let label = ui.selectable_label(i == draft.kind_selected, self.tool_title(key));
                                        icon_response.clicked() || label.clicked()
                                    });
                                    if (up || down) && i == draft.kind_selected { response.response.scroll_to_me(None); }
                                    if response.inner { kind_choice = Some(key); }
                                }
                            });
                        });
                    draft.kind_open = kind_open;
                    if let Some(key) = kind_choice {
                        if draft.key != key {
                            draft.key = key;
                            draft.settings = ToolSettings::new(key);
                            draft.depth_text.clear();
                        }
                        draft.kind_open = false;
                        kind_button.request_focus();
                    }
                    ui.add_space(12.0);
                    creator_settings(ui, &mut draft.settings, draft.key, &mut draft.depth_text);
                });
                ui.add_space(10.0);
                let name = draft.name.trim();
                let group = draft.group.trim();
                let duplicate = self.tools.has_saved_name_except(name, group, draft.editing);
                if duplicate {
                    ui.label(RichText::new("A tool with this name is already in that group.").size(12.0).color(MUTED));
                } else if draft.depth_m().is_none() {
                    ui.label(RichText::new("Enter a depth greater than zero, such as 200 mm.").size(12.0).color(MUTED));
                }
                ui.horizontal(|ui| {
                    if styled_button(ui, "Cancel", Tone::Ghost, false).clicked() { cancel = true; }
                    let valid = draft.can_submit(&self.tools);
                    let action = if draft.editing.is_some() { "Save changes" } else { "Create tool" };
                    if ui.add_enabled_ui(valid, |ui| styled_button(ui, action, Tone::Primary, false)).inner.clicked() { create = true; }
                });
            });
        if response.should_close() || cancel { return; }
        if create {
            self.submit_tool(draft);
        } else {
            self.tool_creator = Some(draft);
        }
    }

    /// The picture on a tool's button, whichever kind of tool it is.
    pub(super) fn key_icon(&self, key: ToolKey) -> Icon {
        match key {
            ToolKey::Measure(tool) => tool.icon(),
            ToolKey::Draw(kind) => markups::tool_icon(kind),
            ToolKey::Highlight => Icon::Highlighter,
        }
    }

    /// The tools kept by name, in their groups.
    fn saved_tools_body(&mut self, ui: &mut Ui) {
        // Worked out as the list is walked and acted on after it, since taking
        // one up or forgetting one changes the list.
        let mut take_up: Option<usize> = None;
        let mut edit: Option<usize> = None;
        let mut forget: Option<usize> = None;
        let mut roll: Option<String> = None;

        if styled_button(ui, "Create new tool…", Tone::Primary, false).clicked() {
            self.open_tool_creator();
        }
        ui.add_space(8.0);

        if self.tools.saved_count() == 0 {
            empty_note(ui, "No tools kept yet. Create one here or press Ctrl+K.");
        }
        for (group, tools) in self.tools.groups() {
            let rolled = self.tools.is_collapsed(&group);
            // The ungrouped ones read as a plain list, with nothing to roll.
            if !group.is_empty() {
                let (name, count) = (group.clone(), tools.len());
                let rolled_up = rolled;
                if list_row(ui, Id::new(("tool-group", &group)), false, |ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    caret(ui.painter(), rect, rolled_up);
                    ui.label(RichText::new(&name).size(12.5).strong().color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(count.to_string()).size(11.5).color(SUBTLE));
                    });
                    None
                }) {
                    roll = Some(group.clone());
                }
            }
            if rolled && !group.is_empty() {
                continue;
            }
            let indent = if group.is_empty() { 0.0 } else { 10.0 };
            for (at, tool) in tools {
                let key = tool.key();
                let held = key.is_some_and(|key| self.held_tool() == Some(key) && self.tools.settings(key) == tool.settings);
                let icon = key.map(|key| self.key_icon(key));
                let mut hit_cross = false;
                let mut hit_edit = false;
                let opened = list_row(ui, Id::new(("tool-kept", at)), held, |ui| {
                    ui.add_space(indent);
                    // What kind of tool it is, then what it draws with.
                    let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    if let Some(icon) = icon {
                        icons::paint(ui.painter(), rect, icon, if held { ACCENT_TEXT } else { MUTED });
                    }
                    tool_preview(ui, &tool.settings, 16.0);
                    ui.label(RichText::new(&tool.name).size(12.5).color(if held { ACCENT_TEXT } else { TEXT }));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let cross = styled_button(ui, "\u{00d7}", Tone::Ghost, false).on_hover_text("Forget this tool");
                        hit_cross = cross.clicked();
                        let edit_button = saved_tool_action(ui, Icon::Edit, "Edit this tool");
                        hit_edit = edit_button.clicked();
                        Some(edit_button.rect)
                    })
                    .inner
                });
                if hit_cross {
                    forget = Some(at);
                } else if hit_edit {
                    edit = Some(at);
                } else if opened && key.is_some() {
                    take_up = Some(at);
                }
            }
            ui.add_space(4.0);
        }

        // A set of tools is worth handing round an office, so it goes out and
        // comes back as a file of its own.
        ui.add_space(6.0);
        ui.separator();
        let mut copied = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if ui
                .add_enabled_ui(self.tools.saved_count() > 0, |ui| {
                    paint_button(ui, "Export…", FontId::proportional(12.0), Tone::Ghost, false, Vec2::ZERO)
                })
                .inner
                .on_hover_text("Save these tools as a file to share")
                .clicked()
            {
                self.export_tools();
            }
            if paint_button(ui, "Import…", FontId::proportional(12.0), Tone::Ghost, false, Vec2::ZERO)
                .on_hover_text("Take in tools from a file")
                .clicked()
            {
                self.import_tools();
            }
            // What a tool file holds, to hand to whoever -- or whatever -- is
            // writing one.
            if paint_button(ui, "Schema", FontId::proportional(12.0), Tone::Ghost, false, Vec2::ZERO)
                .on_hover_text("Copy what a tool file holds, field by field, to the clipboard")
                .clicked()
            {
                ui.ctx().copy_text(tools::Tools::schema_json());
                copied = true;
            }
        });
        if copied {
            self.toast("The tool file's schema is on the clipboard".to_owned());
        }

        if let Some(group) = roll {
            self.tools.toggle_collapsed(&group);
        }
        if let Some(at) = forget {
            self.tools.forget_tool(at);
        }
        if let Some(at) = edit {
            self.open_tool_editor(at);
        }
        if let Some(at) = take_up {
            self.take_up_saved(at);
        }
    }

    /// Writes the tools kept by name out as a file to hand to someone else.
    fn export_tools(&mut self) {
        let Ok(text) = self.tools.export_json() else {
            self.toast("Those tools could not be written out".to_owned());
            return;
        };
        let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).set_file_name("kinetic-pdf tools.json").save_file() else {
            return;
        };
        let count = self.tools.saved_count();
        match std::fs::write(&path, text) {
            Ok(()) => self.toast(format!("{count} tools written to {}", path.display())),
            Err(e) => self.toast(format!("Could not write that file: {e}")),
        }
    }

    /// Takes in tools from a file someone else wrote.
    fn import_tools(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() else { return };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => return self.toast(format!("Could not read that file: {e}")),
        };
        match self.tools.import_json(&text) {
            Ok(count) => {
                self.tool_tab = Tab::Tools;
                self.toast(format!("{count} tools taken in"));
            }
            Err(why) => self.toast(format!("Nothing taken in: {why}")),
        }
    }

    /// Takes up a tool kept by name: its settings become that tool's, and that
    /// tool goes in hand, so the next measurement is drawn as it says. The
    /// list stays up, since picking tools off it is usually a run of them.
    pub(super) fn take_up_saved(&mut self, at: usize) {
        let found = self
            .tools
            .groups()
            .into_iter()
            .flat_map(|(_, tools)| tools)
            .find(|(i, _)| *i == at)
            .and_then(|(_, tool)| Some((tool.key()?, tool.settings.clone())));
        let Some((key, settings)) = found else { return };
        self.tools.set(key, settings);
        match key {
            ToolKey::Measure(tool) => {
                self.measure_tool = Some(tool);
                self.tool = None;
                self.highlighter = false;
            }
            ToolKey::Draw(kind) => self.take_up_drawing(kind),
            ToolKey::Highlight => self.take_up_highlighter(),
        }
        self.active_measure = None;
        self.active = None;
    }
}

#[cfg(test)]
mod tests {
    use super::super::quantities::tests::Table;
    use super::*;

    #[test]
    fn taking_up_a_saved_highlight_uses_its_settings() {
        let mut table = Table::named(&[]);
        table.app.tools = Tools::default();
        let mut settings = ToolSettings::new(ToolKey::Highlight);
        settings.style.stroke = [0.45, 0.76, 1.0];
        settings.defaults.description = "Review this".to_owned();
        table.app.tools.save_tool("Review", "", ToolKey::Highlight, settings.clone());
        table.app.take_up_saved(0);
        assert!(table.app.highlighting());
        assert_eq!(table.app.held_tool(), Some(ToolKey::Highlight));
        settings.defaults.name = "Review".to_owned();
        assert_eq!(table.app.tools.settings(ToolKey::Highlight), settings);
    }

    #[test]
    fn editing_a_saved_tool_keeps_its_place_and_updates_the_active_tool() {
        let mut table = Table::named(&[]);
        table.app.tools = Tools::default();
        let key = ToolKey::Measure(MeasureTool::Area);
        let mut settings = ToolSettings::new(key);
        settings.depth_m = Some(0.2);
        table.app.tools.save_tool("Slab", "Concrete", key, settings);
        table.app.tools.save_tool("Wall", "Concrete", key, ToolSettings::new(key));
        table.app.take_up_saved(0);

        table.app.open_tool_editor(0);
        let mut draft = table.app.tool_creator.take().unwrap();
        assert_eq!(draft.editing, Some(0));
        assert_eq!(draft.name, "Slab");
        assert_eq!(draft.group, "Concrete");
        assert_eq!(draft.depth_m(), Some(Some(0.2)));
        assert!(draft.can_submit(&table.app.tools));
        draft.name = "Wall".to_owned();
        assert!(!draft.can_submit(&table.app.tools));
        draft.name = "Slab revised".to_owned();
        draft.group = "Earth".to_owned();
        draft.settings.style.stroke = [0.0, 0.5, 1.0];
        table.app.submit_tool(draft);

        assert_eq!(table.app.tools.saved_count(), 2);
        let saved = table.app.tools.saved_tool(0).unwrap();
        assert_eq!((&saved.name[..], &saved.group[..]), ("Slab revised", "Earth"));
        assert_eq!(saved.settings.defaults.name, "Slab revised");
        assert_eq!(table.app.tools.saved_tool(1).unwrap().name, "Wall");
        assert_eq!(table.app.tools.settings(key).style.stroke, [0.0, 0.5, 1.0]);
    }

    #[test]
    fn creator_text_fields_match_picker_height() {
        let ctx = egui::Context::default();
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), ..Default::default() };
        let mut heights = (0.0, 0.0);
        let mut output = ctx.run_ui(raw, |ui| {
            ui.set_width(300.0);
            let mut text = String::new();
            heights.0 = creator_text(ui, &mut text, "Name").rect.height();
            heights.1 = creator_picker_button(ui, "No group", None).rect.height();
        });
        output.textures_delta.clear();
        assert_eq!(heights, (28.0, 28.0));
    }

    #[test]
    fn creator_saves_a_grouped_area_with_its_settings_and_takes_it_up() {
        let mut table = Table::named(&[]);
        table.app.tools = Tools::default();
        let mut draft = ToolCreator::new();
        draft.name = "Slab 200".to_owned();
        draft.group = "Concrete".to_owned();
        draft.new_group = true;
        draft.settings.defaults.description = "Ground floor slab".to_owned();
        draft.settings.style.stroke = [0.0, 0.5, 1.0];
        draft.depth_text = "200 mm".to_owned();
        assert!(draft.can_submit(&table.app.tools));
        table.app.submit_tool(draft);
        let groups = table.app.tools.groups();
        assert_eq!(groups[0].0, "Concrete");
        let saved = groups[0].1[0].1;
        assert_eq!(saved.name, "Slab 200");
        assert_eq!(saved.settings.defaults.description, "Ground floor slab");
        assert_eq!(saved.settings.depth_m, Some(0.2));
        assert_eq!(table.app.measure_tool, Some(MeasureTool::Area));
        assert_eq!(table.app.tools.settings(ToolKey::Measure(MeasureTool::Area)).style.stroke, [0.0, 0.5, 1.0]);

        let mut duplicate = ToolCreator::new();
        duplicate.name = "Slab 200".to_owned();
        duplicate.group = "Concrete".to_owned();
        assert!(!duplicate.can_submit(&table.app.tools));
        duplicate.group = "Other".to_owned();
        duplicate.depth_text = "bad depth".to_owned();
        assert!(!duplicate.can_submit(&table.app.tools));
    }

    #[test]
    fn creator_modal_opens_and_escape_discards_its_draft() {
        let mut table = Table::named(&[]);
        table.app.tools = Tools::default();
        table.app.open_tool_creator();
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))), ..Default::default() };
        let mut output = table.ctx.run_ui(raw, |ui| table.app.show_tool_creator(ui.ctx()));
        output.textures_delta.clear();
        assert!(table.app.tool_creator.is_some());
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))),
            events: vec![egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }],
            ..Default::default()
        };
        let mut output = table.ctx.run_ui(raw, |ui| table.app.show_tool_creator(ui.ctx()));
        output.textures_delta.clear();
        assert!(table.app.tool_creator.is_none());
        assert_eq!(table.app.tools.saved_count(), 0);
    }

    #[test]
    fn creator_group_list_can_be_opened_and_chosen_from_keyboard() {
        let mut table = Table::named(&[]);
        table.app.tools = Tools::default();
        table.app.open_tool_creator();
        let frame = |table: &mut Table, events: Vec<egui::Event>| {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))), events, ..Default::default() };
            let mut output = table.ctx.run_ui(raw, |ui| table.app.show_tool_creator(ui.ctx()));
            output.textures_delta.clear();
        };
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        frame(&mut table, Vec::new());
        frame(&mut table, vec![key(Key::Tab)]);
        frame(&mut table, vec![key(Key::Enter)]);
        assert!(table.app.tool_creator.as_ref().unwrap().group_open);
        frame(&mut table, vec![key(Key::Tab)]);
        frame(&mut table, vec![key(Key::Enter)]);
        let draft = table.app.tool_creator.as_ref().unwrap();
        assert!(draft.new_group);
        assert!(!draft.group_open);
    }

    #[test]
    fn creator_kind_list_can_be_chosen_from_keyboard() {
        let mut table = Table::named(&[]);
        table.app.open_tool_creator();
        let frame = |table: &mut Table, events: Vec<egui::Event>| {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))), events, ..Default::default() };
            let mut output = table.ctx.run_ui(raw, |ui| table.app.show_tool_creator(ui.ctx()));
            output.textures_delta.clear();
        };
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        frame(&mut table, Vec::new());
        frame(&mut table, vec![key(Key::Tab)]);
        frame(&mut table, vec![key(Key::Tab)]);
        frame(&mut table, vec![key(Key::Enter)]);
        assert!(table.app.tool_creator.as_ref().unwrap().kind_open);
        frame(&mut table, Vec::new());
        frame(&mut table, vec![key(Key::ArrowDown)]);
        assert_eq!(table.app.tool_creator.as_ref().unwrap().kind_selected, 1);
        frame(&mut table, vec![key(Key::Enter)]);
        assert_eq!(table.app.tool_creator.as_ref().unwrap().key, ToolKey::Measure(MeasureTool::Polylength));
        frame(&mut table, vec![key(Key::Enter)]);
        frame(&mut table, Vec::new());
        frame(&mut table, vec![egui::Event::Text("radius".to_owned())]);
        frame(&mut table, vec![key(Key::Enter)]);
        let draft = table.app.tool_creator.as_ref().unwrap();
        assert_eq!(draft.key, ToolKey::Measure(MeasureTool::Radius));
        assert!(!draft.kind_open);
    }

    #[test]
    fn creator_group_search_selects_an_existing_group() {
        let mut table = Table::named(&[]);
        table.app.tools = Tools::default();
        let key = ToolKey::Measure(MeasureTool::Area);
        table.app.tools.save_tool("Slab", "Concrete", key, ToolSettings::new(key));
        table.app.open_tool_creator();
        let frame = |table: &mut Table, events: Vec<egui::Event>| {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))), events, ..Default::default() };
            let mut output = table.ctx.run_ui(raw, |ui| table.app.show_tool_creator(ui.ctx()));
            output.textures_delta.clear();
        };
        let key_event = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        frame(&mut table, Vec::new());
        frame(&mut table, vec![key_event(Key::Tab)]);
        frame(&mut table, vec![key_event(Key::Enter)]);
        frame(&mut table, Vec::new());
        frame(&mut table, vec![egui::Event::Text("conc".to_owned())]);
        frame(&mut table, vec![key_event(Key::ArrowDown)]);
        assert_eq!(table.app.tool_creator.as_ref().unwrap().group_selected, 1);
        frame(&mut table, vec![key_event(Key::Enter)]);
        let draft = table.app.tool_creator.as_ref().unwrap();
        assert_eq!(draft.group, "Concrete");
        assert!(!draft.new_group && !draft.group_open);
    }

    impl Table {
        /// One frame of the details panel alone, open on its Details side.
        fn details(&mut self) {
            self.app.tool_panel_open = true;
            self.app.tool_tab = Tab::Details;
            let raw = egui::RawInput { time: Some(self.time), screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 900.0))), ..Default::default() };
            let app = &mut self.app;
            let mut output = self.ctx.run_ui(raw, |ui| {
                app.settle_picked();
                app.tool_panel(ui);
            });
            output.textures_delta.clear();
        }

        fn name_of(&self, i: usize) -> String {
            let RowId::Measure(id) = self.measure(i) else { unreachable!("only measurements here") };
            self.app.doc.as_ref().unwrap().session.measures().get(id).unwrap().meta.name.clone()
        }

        fn can_undo(&self) -> bool {
            self.app.doc.as_ref().unwrap().session.can_undo()
        }
    }

    /// Several of one kind picked out share one form: showing it changes none
    /// of them, even where they differ, and a change made in it goes to each
    /// one as a single step to undo -- that change and nothing else.
    #[test]
    fn several_of_one_kind_are_changed_together_as_one_step() {
        let mut table = Table::named(&["Kerb", "Wall", "Fence"]);
        let (first, third) = (table.measure(0), table.measure(2));
        table.app.pick(&[first, third]);
        assert!(matches!(table.app.subject(), Some(Subject::Many(ToolKey::Measure(MeasureTool::Length)))));
        for _ in 0..3 {
            table.details();
        }
        assert!(!table.can_undo(), "looking changes nothing");

        // What the form starts from: the first one, with the name they don't
        // share left empty. A new colour is all that is changed.
        let mut before = as_shown(table.app.row_settings(first).unwrap());
        before.defaults.name.clear();
        let mut after = before.clone();
        after.style.stroke = [0.0, 0.5, 1.0];
        table.app.change_many(&[first, third], &before, &after);

        let stroke = |table: &Table, id| table.app.row_settings(id).unwrap().style.stroke;
        assert_eq!((stroke(&table, first), stroke(&table, third)), ([0.0, 0.5, 1.0], [0.0, 0.5, 1.0]));
        assert_ne!(stroke(&table, table.measure(1)), [0.0, 0.5, 1.0], "the one not picked out is left alone");
        assert_eq!((table.name_of(0), table.name_of(2)), ("Kerb".to_owned(), "Fence".to_owned()), "the mixed names stay their own");
        let label = |table: &Table, id| table.app.row_settings(id).unwrap().style.label_colour;
        assert_eq!((label(&table, first), label(&table, third)), (None, None), "their quantities still follow their lines");

        let session = &mut table.app.doc.as_mut().unwrap().session;
        assert!(session.undo());
        assert!(!session.can_undo(), "one step for both");
        assert_ne!(stroke(&table, first), [0.0, 0.5, 1.0]);
        assert_ne!(stroke(&table, third), [0.0, 0.5, 1.0]);
    }

    /// Things of different kinds have no one form: the panel counts them
    /// instead, and a note, which has no form at all, counts too.
    #[test]
    fn different_kinds_are_counted_rather_than_edited() {
        let mut table = Table::named(&["Kerb", "Wall"]);
        let (kerb, wall) = (table.measure(0), table.measure(1));
        let drawing = crate::model::Markup {
            key: None,
            page: 0,
            kind: MarkupKind::Rectangle,
            points: vec![[10.0, 10.0], [60.0, 40.0]],
            bounds: PdfBox { left: 10.0, bottom: 10.0, right: 60.0, top: 40.0 },
            color: [1.0, 0.0, 0.0],
            width: 1.0,
            style: crate::model::DrawStyle::default(),
            name: String::new(),
            comment: String::new(),
            author: String::new(),
        };
        let uid = table.app.doc.as_mut().unwrap().session.apply(crate::session::Command::AddMarkup(drawing))[0];
        table.app.pick(&[kerb, RowId::Drawing(uid), wall]);
        assert!(table.app.subject().is_none());
        assert_eq!(table.app.picked_rows().len(), 3);
        table.details();
        // Without the box, the two lengths share a form.
        table.app.pick(&[kerb, wall]);
        assert!(matches!(table.app.subject(), Some(Subject::Many(_))));
    }

    #[test]
    fn kinds_are_counted_in_the_plural() {
        assert_eq!(plural("Area", 1), "Area");
        assert_eq!(plural("Area", 3), "Areas");
        assert_eq!(plural("Box", 2), "Boxes");
        assert_eq!(plural("Note", 0), "Notes");
    }
}
