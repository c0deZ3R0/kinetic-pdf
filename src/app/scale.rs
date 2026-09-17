//! The scale tool: what a page measures at, how to set it, and how to check
//! it.
//!
//! A scale says how many real metres one PDF point stands for. Everything
//! measured on a page derives from its scale, so this is the part that has to
//! be right: calibrating shows the distance it measured, warns when the two
//! points were too close together on screen to be accurate, and offers a check
//! against a second known dimension.
//!
//! Scales live in the session with highlights and markups, so setting one is
//! an undoable change and counts as unsaved work. They are read from the file
//! only when this panel first opens (`Request::ReadMeasurements`), since that
//! needs a pass over the whole file.

use markup_model::scale::{self, CalibrationWarning, Sheet};
use markup_model::units::{DisplayUnits, LengthUnit, Precision};
use markup_model::{Pt, Rect as MRect, Scale, ScaleId, ScaleStore};

use super::*;

/// Ratios offered as presets, for a drawing printed at its true paper size.
const PRESETS: [f64; 12] = [1.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2500.0];

/// How far the pointer must move for a calibration to count, in screen points.
const LEAST_DRAG: f32 = 4.0;

/// Whether the document's scales and measurements have been read from the file.
#[derive(Default, Clone, Debug, PartialEq)]
pub(super) enum MeasureRead {
    #[default]
    NotRead,
    Reading,
    Ready,
    Failed(String),
}

/// What a calibration line is being drawn for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MeasureTool {
    /// Set the page's scale from a known dimension.
    Calibrate,
    /// Measure a known dimension to see how far the scale is out.
    Verify,
}

/// The dialog after a calibration line is drawn.
pub(super) struct ScaleDialog {
    pub(super) tool: MeasureTool,
    pub(super) page: usize,
    /// The line's ends, in PDF user space.
    pub(super) from: (f32, f32),
    pub(super) to: (f32, f32),
    /// How far apart the two ends were on screen, for the accuracy warning.
    pub(super) pixels: f32,
    /// What the user types the real length as.
    pub(super) text: String,
    pub(super) error: Option<String>,
    pub(super) just_opened: bool,
}

/// A page's box in PDF user space, as the model wants it.
fn page_box(doc: &Doc, page: usize) -> Option<MRect> {
    let b = doc.geometry.get(page).copied().flatten()?.bounds;
    Some(MRect::from_corners(Pt::new(f64::from(b.left), f64::from(b.bottom)), Pt::new(f64::from(b.right), f64::from(b.top))))
}

/// The standard sheet a page is, if it is one.
fn sheet_of(doc: &Doc, page: usize) -> Option<Sheet> {
    let size = doc.sizes.get(page)?;
    scale::standard_sheet(f64::from(size.x), f64::from(size.y))
}

/// `1:100`, or the ratio to a hundredth when it isn't a round one.
fn ratio_label(scale: &Scale) -> String {
    match scale.ratio() {
        Some(ratio) if ratio >= 1.0 => format!("1:{}", markup_model::units::group_thousands(ratio, if ratio.fract() < 0.005 { 0 } else { 2 })),
        _ => scale.label.clone(),
    }
}

impl App {
    /// The scale the current document's page `page` measures at.
    pub(super) fn page_scale(&self, page: usize) -> Option<&Scale> {
        page_scale(self.doc.as_ref()?, page)
    }

    /// Asks the worker for the file's scales and measurements, once.
    pub(super) fn want_measurements(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        if doc.measurements != MeasureRead::NotRead {
            return;
        }
        doc.measurements = MeasureRead::Reading;
        let _ = self.tx.send(Request::ReadMeasurements { generation: doc.generation });
    }

    /// Changes the document's scales as `change` says, as one undoable step.
    fn change_scales(&mut self, change: impl FnOnce(&mut ScaleStore)) {
        let Some(doc) = self.doc.as_mut() else { return };
        let mut scales = doc.session.scales().clone();
        change(&mut scales);
        doc.session.apply(crate::session::Command::SetScales(scales));
    }

    /// Gives `page` a scale, by recalibrating the one it already has -- which
    /// recalibrates every page sharing it -- or adding a new one.
    fn set_page_scale(&mut self, page: usize, mut scale: Scale) {
        let existing = self.page_scale(page).map(|s| s.id);
        let Some(page_box) = self.doc.as_ref().and_then(|doc| page_box(doc, page)) else { return };
        self.change_scales(|scales| {
            match existing {
                Some(id) => {
                    // Keep the same scale object, so pages sharing it follow.
                    scale.id = id;
                    scales.set_scale(scale);
                }
                None => {
                    let id = scale.id;
                    scales.set_scale(scale);
                    scales.set_page_scale(page as u32, page_box, id);
                }
            };
        });
    }

    /// How many pages, other than `page`, measure at its scale.
    fn pages_sharing(&self, page: usize) -> usize {
        let Some(doc) = self.doc.as_ref() else { return 0 };
        let Some(scale) = self.page_scale(page) else { return 0 };
        doc.session.scales().pages_using(scale.id).len().saturating_sub(1)
    }

    /// Starts a calibration line on `page` at `pos`.
    pub(super) fn start_calibration(&mut self, page: usize, pos: Pos2) {
        let Some(point) = self.pdf_point(page, pos) else { return };
        self.drag = Some(Drag::Calibrate { page, from: point, to: point });
        self.popup = None;
    }

    /// A calibration line was let go: ask for the real length, unless it was
    /// barely a click.
    pub(super) fn finish_calibration(&mut self, page: usize, from: (f32, f32), to: (f32, f32)) {
        let per_point = self.page_rects.get(&page).zip(self.doc.as_ref()).map_or(1.0, |(rect, doc)| rect.width() / doc.sizes[page].x);
        let pixels = (to.0 - from.0).hypot(to.1 - from.1) * per_point;
        if pixels < LEAST_DRAG {
            return;
        }
        let Some(tool) = self.measure_tool else { return };
        self.scale_dialog = Some(ScaleDialog { tool, page, from, to, pixels, text: String::new(), error: None, just_opened: true });
    }

}

/// The scale a page measures at, from a document being drawn.
pub(super) fn page_scale(doc: &Doc, page: usize) -> Option<&Scale> {
    let scales = doc.session.scales();
    scales.page_default(page as u32).and_then(|v| scales.scale(v.scale))
}

/// Draws the calibration line being dragged, with what it measures so far.
pub(super) fn paint_calibration(
    painter: &egui::Painter,
    (_, from, to): (usize, (f32, f32), (f32, f32)),
    scale: Option<&Scale>,
    rect: Rect,
    g: &PageGeometry,
) {
    let at = |(x, y): (f32, f32)| {
        let (fx, fy) = g.to_view(x, y);
        pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height())
    };
    let (a, b) = (at(from), at(to));
    let stroke = Stroke::new(1.5, ACCENT);
    painter.line_segment([a, b], stroke);
    // Ticks across each end, as a dimension line has.
    let along = (b - a).normalized();
    let across = vec2(-along.y, along.x) * 5.0;
    painter.line_segment([a - across, a + across], stroke);
    painter.line_segment([b - across, b + across], stroke);
    // What it measures as it's dragged, if the page already has a scale.
    if let Some(scale) = scale {
        let metres = scale.distance(Pt::new(f64::from(from.0), f64::from(from.1)), Pt::new(f64::from(to.0), f64::from(to.1)));
        let text = markup_model::units::format_length(metres, scale.display.length, scale.precision);
        let middle = a + (b - a) / 2.0 - across * 2.0;
        painter.text(middle, Align2::CENTER_BOTTOM, text, FontId::proportional(12.0), ACCENT);
    }
}

impl App {
    /// The scale panel: what this page measures at, and how to set it.
    pub(super) fn scale_panel(&mut self, ui: &mut Ui) {
        self.want_measurements();
        self.side_panel(
            ui,
            "scale",
            |app, ui| {
                let page = app.current_page;
                let detail = match app.page_scale(page) {
                    Some(scale) => ratio_label(scale),
                    None => "Not set".to_owned(),
                };
                panel_heading(ui, &format!("Scale · page {}", page + 1), detail);
            },
            None,
            |app, ui| {
                egui::ScrollArea::vertical().id_salt("scale-body").auto_shrink(false).show(ui, |ui| {
                    Frame::NONE.inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
                        app.scale_body(ui);
                    });
                });
            },
        );
    }

    fn scale_body(&mut self, ui: &mut Ui) {
        let Some(doc) = self.doc.as_ref() else {
            empty_note(ui, "Open a PDF to set its scale.");
            return;
        };
        match &doc.measurements {
            MeasureRead::NotRead | MeasureRead::Reading => {
                ui.label(RichText::new("Reading the file's scales…").color(MUTED));
                return;
            }
            MeasureRead::Failed(error) => {
                ui.label(RichText::new(format!("The file's scales could not be read: {error}")).color(DANGER));
                return;
            }
            MeasureRead::Ready => {}
        }
        let page = self.current_page;

        // What it measures at now.
        match self.page_scale(page) {
            Some(scale) => {
                let shared = self.pages_sharing(page);
                let label = scale.label.clone();
                ui.label(RichText::new(ratio_label(scale)).size(20.0).strong().color(TEXT));
                if !label.is_empty() {
                    ui.label(RichText::new(label).size(12.5).color(MUTED));
                }
                if shared > 0 {
                    let pages = if shared == 1 { "page".to_owned() } else { format!("{shared} pages") };
                    ui.label(RichText::new(format!("Shared with {}{pages}", if shared == 1 { "1 other " } else { "" })).size(12.0).color(SUBTLE));
                }
            }
            None => {
                ui.label(RichText::new("Not set").size(20.0).strong().color(TEXT));
                ui.label(RichText::new("Measurements on this page have no real length until it is.").size(12.5).color(MUTED));
            }
        }

        // A printed ratio only holds if the page is at its true paper size.
        match sheet_of(doc, page) {
            Some(sheet) => ui.label(RichText::new(format!("{} sheet", sheet.name())).size(12.0).color(SUBTLE)),
            None => ui.label(
                RichText::new("This page isn't a standard sheet size, so a printed ratio may not hold: measure a known dimension instead.")
                    .size(12.0)
                    .color(DIRTY),
            ),
        };

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let calibrating = self.measure_tool == Some(MeasureTool::Calibrate);
            if styled_button(ui, "Measure a known length", Tone::Primary, calibrating)
                .on_hover_text("Drag along something whose real length you know")
                .clicked()
            {
                self.measure_tool = if calibrating { None } else { Some(MeasureTool::Calibrate) };
                self.tool = None;
            }
        });
        if self.measure_tool == Some(MeasureTool::Calibrate) {
            ui.label(RichText::new("Drag along a known dimension on the page. Hold Shift to keep it straight.").size(12.0).color(ACCENT));
        }

        ui.add_space(2.0);
        ui.label(RichText::new("Or pick a printed ratio").size(12.0).color(MUTED));
        let mut chosen = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            let current = self.page_scale(page).and_then(Scale::ratio);
            for ratio in PRESETS {
                let selected = current.is_some_and(|r| (r - ratio).abs() < 0.001);
                if styled_button(ui, &format!("1:{ratio:.0}"), Tone::Secondary, selected).clicked() {
                    chosen = Some(ratio);
                }
            }
        });
        if let Some(ratio) = chosen {
            let display = self.page_scale(page).map_or(DisplayUnits::METRIC, |s| s.display);
            if let Ok(mut scale) = Scale::from_ratio(ScaleId::new(), ratio) {
                scale.display = display;
                self.set_page_scale(page, scale);
            }
        }

        if self.page_scale(page).is_none() {
            return;
        }

        ui.add_space(4.0);
        ui.separator();
        ui.label(RichText::new("Show lengths in").size(12.0).color(MUTED));
        let mut units = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            let current = self.page_scale(page).map(|s| s.display.length);
            for (name, unit) in [("m", LengthUnit::Metre), ("mm", LengthUnit::Millimetre), ("km", LengthUnit::Kilometre), ("ft-in", LengthUnit::FeetInches)] {
                if styled_button(ui, name, Tone::Secondary, current == Some(unit)).clicked() {
                    units = Some(unit);
                }
            }
        });
        if let Some(unit) = units {
            self.set_display_unit(page, unit);
        }

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            let verifying = self.measure_tool == Some(MeasureTool::Verify);
            if styled_button(ui, "Check it", Tone::Secondary, verifying).on_hover_text("Measure a second known dimension to see how far out it is").clicked() {
                self.measure_tool = if verifying { None } else { Some(MeasureTool::Verify) };
                self.tool = None;
            }
            let pages = self.doc.as_ref().map_or(0, |d| d.sizes.len());
            if pages > 1 && styled_button(ui, "Use on every page", Tone::Secondary, false).on_hover_text("Give every page this scale").clicked() {
                self.copy_scale_to_all(page);
            }
        });
        if self.measure_tool == Some(MeasureTool::Verify) {
            ui.label(RichText::new("Drag along a second dimension you know the length of.").size(12.0).color(ACCENT));
        }
    }

    fn set_display_unit(&mut self, page: usize, unit: LengthUnit) {
        let Some(mut scale) = self.page_scale(page).cloned() else { return };
        scale.display.length = unit;
        scale.display.area = if unit.is_metric() { markup_model::units::AreaUnit::SquareMetre } else { markup_model::units::AreaUnit::SquareFoot };
        scale.display.volume = if unit.is_metric() { markup_model::units::VolumeUnit::CubicMetre } else { markup_model::units::VolumeUnit::CubicYard };
        scale.precision = if unit == LengthUnit::FeetInches { Precision::Fraction(16) } else { Precision::Decimals(2) };
        self.change_scales(|scales| {
            scales.set_scale(scale);
        });
    }

    /// Gives every page the scale of `page`, sharing one scale so that
    /// recalibrating any of them recalibrates all.
    fn copy_scale_to_all(&mut self, page: usize) {
        let Some(doc) = self.doc.as_ref() else { return };
        let boxes: Vec<Option<MRect>> = (0..doc.sizes.len()).map(|p| page_box(doc, p)).collect();
        let pages: Vec<u32> = boxes.iter().enumerate().filter(|(_, b)| b.is_some()).map(|(p, _)| p as u32).collect();
        let count = pages.len().saturating_sub(1);
        self.change_scales(|scales| {
            scales.copy_page_scale(page as u32, pages, |p| boxes[p as usize].unwrap_or(MRect::from_corners(Pt::new(0.0, 0.0), Pt::new(0.0, 0.0))));
        });
        self.toast(format!("{count} other pages now measure at this scale"));
    }

    /// The dialog that asks what the drawn line really measures.
    pub(super) fn show_scale_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.scale_dialog else { return };
        let (page, tool) = (dialog.page, dialog.tool);
        let scale = self.page_scale(page).cloned();
        let points = (dialog.to.0 - dialog.from.0).hypot(dialog.to.1 - dialog.from.1);
        let unit = scale.as_ref().map_or(LengthUnit::Metre, |s| s.display.length);
        let measured = scale.as_ref().map(|s| {
            let (a, b) = (Pt::new(f64::from(dialog.from.0), f64::from(dialog.from.1)), Pt::new(f64::from(dialog.to.0), f64::from(dialog.to.1)));
            s.distance(a, b)
        });
        let short = scale::calibration_warnings(f64::from(dialog.pixels));

        let frame = Frame::NONE
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(20))
            .shadow(soft_shadow());
        let (mut apply, mut cancel) = (false, false);
        let modal = egui::Modal::new(Id::new("scale-dialog")).frame(frame).backdrop_color(Color32::from_black_alpha(60)).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
            let title = match tool {
                MeasureTool::Calibrate => "What is this really?",
                MeasureTool::Verify => "Checking the scale",
            };
            ui.label(RichText::new(title).size(16.0).strong().color(TEXT));
            ui.label(RichText::new(format!("You drew {points:.1} points across the page.")).size(12.5).color(MUTED));
            if let Some(metres) = measured {
                let shown = markup_model::units::format_length(metres, unit, scale.as_ref().map_or(Precision::default(), |s| s.precision));
                ui.label(RichText::new(format!("It measures {shown} at the scale set now.")).size(12.5).color(QUOTE_TEXT));
            }
            for warning in &short {
                if let CalibrationWarning::ShortOnScreen { pixels } = warning {
                    ui.label(
                        RichText::new(format!("Those points are only {pixels:.0} pixels apart on screen. Zoom in, or measure a longer dimension, for an accurate scale."))
                            .size(12.0)
                            .color(DIRTY),
                    );
                }
            }

            let Some(dialog) = self.scale_dialog.as_mut() else { return };
            ui.label(RichText::new("Its real length").size(12.0).color(MUTED));
            let entry = ui.add(
                TextEdit::singleline(&mut dialog.text)
                    .hint_text(match unit {
                        LengthUnit::FeetInches => "82' 6\"",
                        _ => "25 m",
                    })
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(10, 8)),
            );
            if dialog.just_opened {
                entry.request_focus();
                dialog.just_opened = false;
            }
            if let Some(error) = &dialog.error {
                ui.label(RichText::new(error.as_str()).size(12.0).color(DANGER));
            }
            let enter = entry.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.add_space(4.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let confirm = match tool {
                    MeasureTool::Calibrate => "Set the scale",
                    MeasureTool::Verify => "Check",
                };
                apply = styled_button(ui, confirm, Tone::Primary, false).clicked() || enter;
                cancel = styled_button(ui, "Cancel", Tone::Secondary, false).clicked();
            });
        });

        if cancel || modal.should_close() {
            self.scale_dialog = None;
            return;
        }
        if apply {
            self.apply_scale_dialog();
        }
    }

    /// Reads what was typed and sets or checks the scale.
    fn apply_scale_dialog(&mut self) {
        let Some(dialog) = self.scale_dialog.as_ref() else { return };
        let (page, tool) = (dialog.page, dialog.tool);
        let scale = self.page_scale(page).cloned();
        let assumed = scale.as_ref().map(|s| s.display.length).unwrap_or(LengthUnit::Metre);
        let metres = match markup_model::units::parse_length(&dialog.text, Some(assumed)) {
            Ok(metres) if metres > 0.0 => metres,
            Ok(_) => {
                self.scale_dialog_error("The length must be more than zero.");
                return;
            }
            Err(e) => {
                self.scale_dialog_error(&e.to_string());
                return;
            }
        };
        let (a, b) = (
            Pt::new(f64::from(dialog.from.0), f64::from(dialog.from.1)),
            Pt::new(f64::from(dialog.to.0), f64::from(dialog.to.1)),
        );
        match tool {
            MeasureTool::Calibrate => {
                let display = scale.as_ref().map_or(DisplayUnits::METRIC, |s| s.display);
                match Scale::from_two_points(ScaleId::new(), a, b, metres, display) {
                    Ok(mut new) => {
                        if let Some(old) = &scale {
                            new.precision = old.precision;
                        }
                        let shared = self.pages_sharing(page);
                        let label = ratio_label(&new);
                        self.set_page_scale(page, new);
                        self.scale_dialog = None;
                        self.measure_tool = None;
                        let extra = match shared {
                            0 => String::new(),
                            1 => " (and 1 page sharing it)".to_owned(),
                            n => format!(" (and {n} pages sharing it)"),
                        };
                        self.toast(format!("Page {} now measures at {label}{extra}", page + 1));
                    }
                    Err(e) => self.scale_dialog_error(&e.to_string()),
                }
            }
            MeasureTool::Verify => {
                let Some(scale) = scale else { return };
                match scale::verify(&scale, a, b, metres) {
                    Ok(check) => {
                        let shown = markup_model::units::format_length(check.measured_metres, scale.display.length, scale.precision);
                        let out = check.percent_error.abs();
                        let message = if out < 0.5 {
                            format!("It measures {shown}: the scale is right to within {out:.1}%.")
                        } else {
                            format!("It measures {shown}, which is {out:.1}% out. Measure it again to set the scale from this instead.")
                        };
                        self.scale_dialog = None;
                        self.measure_tool = None;
                        self.toast(message);
                    }
                    Err(e) => self.scale_dialog_error(&e.to_string()),
                }
            }
        }
    }

    fn scale_dialog_error(&mut self, message: &str) {
        if let Some(dialog) = self.scale_dialog.as_mut() {
            dialog.error = Some(message.to_owned());
        }
    }
}
