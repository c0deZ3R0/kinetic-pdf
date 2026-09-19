//! The panel down the left: what the tool in hand is set to.
//!
//! It edits `tools::ToolSettings` for whichever tool is held, and nothing
//! else. Everything it shows is part of that structure, so when tools can be
//! saved and named this panel is already the editor for a saved one -- only
//! where the settings come from changes, not what is shown.

use markup_model::markup::{FillPattern, LabelFont, Slope, WidthUnit};
use markup_model::units::{format_length, LengthUnit, Precision};

use super::tools::{ToolKey, ToolSettings};
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

/// What the panel is editing. Both are a `ToolSettings`: one that the next
/// measurement will be drawn with, or one a measurement already has.
#[derive(Clone, Copy)]
enum Subject {
    Tool(ToolKey),
    Measurement { id: MarkupId, key: ToolKey },
}

impl Subject {
    fn key(self) -> ToolKey {
        match self {
            Subject::Tool(key) | Subject::Measurement { key, .. } => key,
        }
    }
}

impl App {
    /// The tool in hand, if it is one with settings.
    pub(super) fn held_tool(&self) -> Option<ToolKey> {
        match (self.measure_tool, self.tool) {
            // Calibrating and checking set the page's scale rather than
            // drawing anything that is kept, so they have nothing to set.
            (Some(MeasureTool::Calibrate | MeasureTool::Verify), _) => None,
            (Some(tool), _) => Some(ToolKey::Measure(tool)),
            (None, Some(kind)) => Some(ToolKey::Draw(kind)),
            (None, None) => None,
        }
    }

    /// What the panel is about: a measurement picked out on the page, or
    /// failing that the tool in hand. A measurement wins, since picking one
    /// out is asking about that one rather than about the next one drawn.
    fn subject(&self) -> Option<Subject> {
        let picked = self.active_measure.and_then(|id| {
            let markup = self.doc.as_ref()?.session.measures().get(id)?;
            Some(Subject::Measurement { id, key: ToolKey::of_measurement(markup.kind)? })
        });
        picked.or_else(|| self.held_tool().map(Subject::Tool))
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
            // Collapsed for what is in hand: it stays collapsed until
            // something else is, rather than springing open again next frame.
            self.tool_shut_for = self.subject().map(|s| s.key());
            // Putting the scale side away puts down the tools that belong to
            // it, and only those: a measurement tool in hand has nothing to
            // do with this side, and dropping it would be a surprise.
            if tab == Tab::Scale && matches!(self.measure_tool, Some(MeasureTool::Calibrate | MeasureTool::Verify)) {
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
        self.tool_shut_for = None;
    }

    /// The settings panel, beside the rail. Once something has been in it, it
    /// stays -- blank between one thing and the next -- rather than coming and
    /// going as measurements are picked out and let go.
    pub(super) fn tool_panel(&mut self, ui: &mut Ui) {
        let subject = self.subject();
        // Taking up a tool, or picking a measurement out, opens the panel on
        // it -- unless the panel was collapsed on that very thing.
        let key = subject.map(|s| s.key());
        if key.is_some() && key != self.tool_shut_for {
            self.tool_panel_open = true;
            self.tool_shut_for = None;
        }
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
                                (Tab::Tools, _) => self.saved_tools_body(ui, subject),
                                (Tab::Details, Some(subject)) => self.tool_body(ui, subject),
                                (Tab::Details, None) => {
                                    empty_note(ui, "Pick a measurement, or take up a tool, to see what it is set to.")
                                }
                            }
                        });
                    });
                });
            });
    }

    fn tool_title(&self, key: ToolKey) -> String {
        match key {
            ToolKey::Measure(tool) => tool.label().to_owned(),
            ToolKey::Draw(kind) => kind.label().to_owned(),
        }
    }

    fn tool_body(&mut self, ui: &mut Ui, subject: Subject) {
        let key = subject.key();
        // Edited as a copy and handed back when it differs, so every change
        // goes through one place whichever setting moved, and whether it is a
        // tool being set up or a measurement being changed.
        let before = match subject {
            Subject::Tool(key) => self.tools.settings(key),
            Subject::Measurement { id, .. } => match self.doc.as_ref().and_then(|d| d.session.measures().get(id)) {
                Some(markup) => ToolSettings::of_markup(markup),
                None => return,
            },
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
        colour_row(ui, "Colour", &mut s.style.stroke);
        // In pixels on screen, so a line stays the thickness it was set to
        // however far the drawing is zoomed. See `WidthUnit`.
        s.style.width_unit = WidthUnit::ScreenPixels;
        slider_row(ui, "Thickness", &mut s.style.width, 0.5..=12.0, "px");
        opacity_row(ui, "Opacity", &mut s.style.opacity);

        if key.fills() {
            ui.add_space(6.0);
            let mut filled = s.style.fill.is_some();
            section_toggle(ui, "Fill", &mut filled);
            if filled {
                let mut rgb = s.style.fill.unwrap_or(s.style.stroke);
                colour_row(ui, "Colour", &mut rgb);
                opacity_row(ui, "Opacity", &mut s.style.fill_opacity);
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
                        if styled_button(ui, pattern.label(), Tone::Secondary, s.style.pattern == pattern).clicked() {
                            s.style.pattern = pattern;
                        }
                    }
                });
                // Its own colour and transparency, over the fill: a grey hatch
                // on a pale fill is the usual way a take-off is marked up.
                if s.style.pattern.is_ruled() {
                    let mut rgb = s.style.pattern_colour.unwrap_or(s.style.stroke);
                    colour_row(ui, "Colour", &mut rgb);
                    s.style.pattern_colour = Some(rgb);
                    opacity_row(ui, "Opacity", &mut s.style.pattern_opacity);
                    // In points on the page, the cell the file's own pattern
                    // repeats, so the hatch on screen is the hatch in the file.
                    slider_row(ui, "Size", &mut s.style.pattern_size, 2.0..=30.0, "pt");
                }
            }
        }

        // The quantity written on the drawing. It is set straight on the page
        // rather than in a card of its own, so what is on screen is what the
        // file gets.
        ui.add_space(6.0);
        section(ui, "Quantity");
        row(ui, "Face", |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for face in LabelFont::ALL {
                    if styled_button(ui, face.label(), Tone::Secondary, s.style.label_font == face).clicked() {
                        s.style.label_font = face;
                    }
                }
            });
        });
        {
            let mut rgb = s.style.label_colour.unwrap_or(s.style.stroke);
            colour_row(ui, "Colour", &mut rgb);
            s.style.label_colour = Some(rgb);
        }
        // In points on the page, as the file writes it, so it scales with the
        // drawing the way printed text does.
        slider_row(ui, "Size", &mut s.style.label_size, 4.0..=48.0, "pt");

        // What every measurement drawn with this tool is called and filed
        // under before anyone types anything.
        ui.add_space(4.0);
        section(ui, "Given to each one");
        field(ui, "Name", &mut s.defaults.name, "What it's called");
        field(ui, "Description", &mut s.defaults.description, "What it's priced as");
        field(ui, "Item code", &mut s.defaults.item_code, "A-120");
        field(ui, "Layer", &mut s.defaults.layer, "");

        if key.takes_depth() || key.takes_slope() {
            ui.add_space(4.0);
            section(ui, "How it measures");
        }
        if key.takes_depth() {
            // An area with a depth is priced by volume, so setting it here
            // saves typing it into every row of the table.
            let mut text = s.depth_m.map_or(String::new(), |m| format_length(m, LengthUnit::Metre, Precision::Decimals(3)));
            if labelled(ui, "Depth", &mut text, "e.g. 200 mm").changed() {
                s.depth_m = markup_model::units::parse_length(text.trim(), Some(LengthUnit::Metre)).ok().filter(|m| *m > 0.0);
            }
            ui.label(RichText::new("An area with a depth is measured as a volume.").size(11.5).color(SUBTLE));
        }
        if key.takes_slope() {
            let sloped = s.slope.is_some();
            if styled_button(ui, "On a slope", Tone::Secondary, sloped)
                .on_hover_text("Lengths and areas on a pitch are divided by its cosine")
                .clicked()
            {
                s.slope = if sloped { None } else { Some(Slope { rise: 1.0, run: 10.0 }) };
            }
            if let Some(slope) = s.slope {
                let mut rise = format!("{}", slope.rise);
                let mut run = format!("{}", slope.run);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let a = labelled(ui, "Rise", &mut rise, "1");
                    let b = labelled(ui, "Run", &mut run, "10");
                    if a.changed() || b.changed() {
                        if let (Ok(rise), Ok(run)) = (rise.trim().parse::<f64>(), run.trim().parse::<f64>()) {
                            if run > 0.0 {
                                s.slope = Some(Slope { rise, run });
                            }
                        }
                    }
                });
            }
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
        }
    }

    /// Puts changed settings on a measurement already drawn, as one undoable
    /// step like any other change to it.
    fn change_measurement(&mut self, id: MarkupId, settings: &ToolSettings) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        let mut changed = markup.clone();
        settings.apply(&mut changed);
        doc.session.apply(crate::session::Command::ChangeMeasure(Box::new(changed)));
    }
}

fn section(ui: &mut Ui, name: &str) {
    ui.label(RichText::new(name).size(11.5).strong().color(MUTED));
}

/// A labelled text box that fills the panel's width.
fn field(ui: &mut Ui, name: &str, text: &mut String, hint: &str) {
    ui.label(RichText::new(name).size(12.0).color(MUTED));
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

fn colour_row(ui: &mut Ui, name: &str, rgb: &mut Rgb) {
    row(ui, name, |ui| {
        let mut colour = to_color32(*rgb);
        if ui.color_edit_button_srgba(&mut colour).changed() {
            *rgb = from_color32(colour);
        }
    });
}

fn slider_row(ui: &mut Ui, name: &str, value: &mut f64, range: std::ops::RangeInclusive<f64>, suffix: &str) {
    row(ui, name, |ui| {
        ui.add(egui::Slider::new(value, range).suffix(suffix).fixed_decimals(1));
    });
}

/// Shown as a percentage, which reads better than a hundredth of a unit.
fn opacity_row(ui: &mut Ui, name: &str, value: &mut f32) {
    row(ui, name, |ui| {
        let mut percent = f64::from(*value) * 100.0;
        if ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%").fixed_decimals(0)).changed() {
            *value = (percent / 100.0) as f32;
        }
    });
}

/// A section heading that can be turned off, for the parts of a shape that
/// needn't be drawn at all.
fn section_toggle(ui: &mut Ui, name: &str, on: &mut bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.checkbox(on, RichText::new(name).size(11.5).strong().color(MUTED));
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

/// The triangle beside a group: along when it is rolled up, down when open.
fn caret(painter: &egui::Painter, rect: Rect, rolled: bool) {
    let c = rect.center();
    let r = rect.width() * 0.34;
    let points = if rolled {
        vec![pos2(c.x - r * 0.6, c.y - r), pos2(c.x + r * 0.8, c.y), pos2(c.x - r * 0.6, c.y + r)]
    } else {
        vec![pos2(c.x - r, c.y - r * 0.6), pos2(c.x + r, c.y - r * 0.6), pos2(c.x, c.y + r * 0.8)]
    };
    painter.add(Shape::convex_polygon(points, MUTED, Stroke::NONE));
}

impl App {
    /// The picture on a tool's button, whichever kind of tool it is.
    fn key_icon(&self, key: ToolKey) -> Icon {
        match key {
            ToolKey::Measure(tool) => tool.icon(),
            ToolKey::Draw(kind) => markups::tool_icon(kind),
        }
    }

    /// The tools kept by name, in their groups, with what is in hand offered
    /// to be kept beside them.
    fn saved_tools_body(&mut self, ui: &mut Ui, subject: Option<Subject>) {
        // Worked out as the list is walked and acted on after it, since taking
        // one up or forgetting one changes the list.
        let mut take_up: Option<usize> = None;
        let mut forget: Option<usize> = None;
        let mut roll: Option<String> = None;

        if self.tools.saved_count() == 0 {
            empty_note(ui, "No tools kept yet. Set one up under Details, then keep it here by name.");
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
                        Some(cross.rect)
                    })
                    .inner
                });
                if hit_cross {
                    forget = Some(at);
                } else if opened && key.is_some() {
                    take_up = Some(at);
                }
            }
            ui.add_space(4.0);
        }

        // Keeping what is in hand, under the list it joins.
        ui.add_space(6.0);
        ui.separator();
        match subject.map(|s| s.key()) {
            Some(key) => {
                section(ui, &format!("Keep this {} as", self.tool_title(key).to_lowercase()));
                field(ui, "Name", &mut self.tool_save.0, "Concrete slab 200");
                field(ui, "Group", &mut self.tool_save.1, "Concrete");
                let named = !self.tool_save.0.trim().is_empty();
                if ui.add_enabled_ui(named, |ui| styled_button(ui, "Keep it", Tone::Primary, false)).inner.clicked() {
                    let settings = match subject {
                        Some(Subject::Measurement { id, .. }) => {
                            self.doc.as_ref().and_then(|d| d.session.measures().get(id)).map(ToolSettings::of_markup)
                        }
                        _ => Some(self.tools.settings(key)),
                    };
                    if let Some(settings) = settings {
                        let (name, group) = (self.tool_save.0.clone(), self.tool_save.1.clone());
                        self.tools.save_tool(&name, &group, key, settings);
                        self.tool_save.0.clear();
                    }
                }
            }
            None => empty_note(ui, "Take up a tool, or pick a measurement out, to keep it here."),
        }

        // A set of tools is worth handing round an office, so it goes out and
        // comes back as a file of its own.
        ui.add_space(6.0);
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
    fn take_up_saved(&mut self, at: usize) {
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
            }
            ToolKey::Draw(kind) => {
                self.tool = Some(kind);
                self.measure_tool = None;
            }
        }
        self.active_measure = None;
    }
}
