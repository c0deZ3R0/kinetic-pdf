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

    /// The settings panel, down the left. Once something has been in it, it
    /// stays -- blank between one thing and the next -- rather than coming and
    /// going as measurements are picked out and let go.
    pub(super) fn tool_panel(&mut self, ui: &mut Ui) {
        let subject = self.subject();
        if subject.is_some() {
            self.tool_panel_open = true;
        }
        if !self.tool_panel_open {
            return;
        }
        let mut close = false;
        egui::Panel::left(Id::new("tool-settings"))
            .frame(Frame::NONE.fill(SURFACE))
            .default_size(260.0)
            .min_size(200.0)
            .show(ui, |ui| {
                let margin = Frame::NONE.inner_margin(Margin::symmetric(14, 12));
                egui::Panel::top(Id::new(("tool-settings", "head"))).frame(margin).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let title = match subject {
                            Some(subject) => self.tool_title(subject.key()),
                            None => "Details".to_owned(),
                        };
                        panel_heading(ui, &title, String::new());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            close = styled_button(ui, "Close", Tone::Ghost, false).on_hover_text("Hide this panel").clicked();
                        });
                    });
                });
                egui::CentralPanel::default().frame(Frame::NONE).show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("tool-settings-body").auto_shrink(false).show(ui, |ui| {
                        Frame::NONE.inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
                            match subject {
                                Some(subject) => self.tool_body(ui, subject),
                                None => empty_note(ui, "Pick a measurement, or take up a tool, to see what it is set to."),
                            }
                        });
                    });
                });
            });
        if close {
            // Closing it means being done with what was in it: a measurement
            // still picked out, or a tool still in hand, would open it again
            // on the next frame.
            self.tool_panel_open = false;
            self.active_measure = None;
            self.active_vertex = None;
            self.measure_tool = None;
            self.tool = None;
        }
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
