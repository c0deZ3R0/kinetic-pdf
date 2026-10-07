//! Placing pins, their visible markers, and the navigation panel.

use super::*;
use super::tools::ToolKey;
use super::tool_panel::section;
use crate::pins::Pin;

#[derive(Default)]
pub(super) struct State {
    pub placing: bool,
    pub selected: Option<String>,
    pub renaming: Option<Rename>,
}

pub(super) struct Rename {
    id: String,
    name: String,
    focus: bool,
}

pub(super) fn marker_rect(point: Pos2) -> Rect {
    Rect::from_min_size(point - vec2(12.0, 24.0), vec2(24.0, 26.0))
}

pub(super) fn paint(painter: &egui::Painter, doc: &Doc, page: usize, rect: Rect, geometry: &PageGeometry, selected: Option<&str>) {
    for pin in doc.session.pins().iter().filter(|pin| pin.page == page && doc.session.layers().is_visible(pin.layer)) {
        let (x, y) = geometry.to_view(pin.position[0], pin.position[1]);
        let at = rect.min + vec2(x * rect.width(), y * rect.height());
        let marker = marker_rect(at);
        if selected == Some(pin.id.as_str()) {
            painter.rect_filled(marker.expand(3.0), CornerRadius::same(6), ACCENT_SOFT);
        }
        // A paper halo keeps even a dark pin readable over a busy drawing.
        painter.circle_filled(marker.center_top() + vec2(0.0, 9.0), 8.0, Color32::WHITE);
        icons::paint(painter, marker, Icon::Pin, to_color32(pin.colour));
    }
}

impl App {
    pub(super) fn take_up_pin(&mut self) {
        self.take_up_select();
        self.pick(&[]);
        self.pins.placing = true;
        self.want_measurements();
        self.show_tool_panel(tool_panel::Tab::Pins);
    }

    pub(super) fn place_pin(&mut self, sheet: usize, pos: Pos2) {
        let Some((x, y)) = self.pdf_point(sheet, pos) else { return };
        let settings = self.tools.settings(ToolKey::Pin);
        let Some(doc) = self.doc.as_mut() else { return };
        if doc.measurements != MeasureRead::Ready { return; }
        let Some(page) = doc.sheet_page(sheet) else { return };
        let layer = doc.session.layer_for_new(&settings.defaults.layer, settings.defaults.layer_colour);
        if doc.session.layers().is_locked(layer) || !doc.session.layers().is_visible(layer) { return; }
        let name = if settings.defaults.name.trim().is_empty() {
            let mut number = 1;
            while doc.session.pins().iter().any(|pin| pin.name == format!("Pin {number}")) { number += 1; }
            format!("Pin {number}")
        } else { settings.defaults.name.clone() };
        let mut pin = Pin::new(page, [x, y], name, settings.style.stroke);
        pin.layer = layer;
        let mut pins = doc.session.pins().to_vec();
        pins.push(pin.clone());
        doc.session.apply(Command::SetPins(pins));
        self.pins.selected = Some(pin.id.clone());
    }

    pub(super) fn pin_at(&self, sheet: usize, pos: Pos2) -> Option<Pin> {
        let doc = self.doc.as_ref()?;
        let page = doc.sheet_page(sheet)?;
        let geometry = doc.sheet_geometry(sheet)?;
        let rect = *self.page_rects.get(&sheet)?;
        doc.session.pins().iter().rev().find(|pin| {
            if pin.page != page || !doc.session.layers().is_visible(pin.layer) { return false; }
            let (x, y) = geometry.to_view(pin.position[0], pin.position[1]);
            marker_rect(rect.min + vec2(x * rect.width(), y * rect.height())).contains(pos)
        }).cloned()
    }

    pub(super) fn pick_pin(&mut self, pin: Pin) {
        if !self.doc.as_ref().is_some_and(|doc| doc.session.layers().is_visible(pin.layer) && !doc.session.layers().is_locked(pin.layer)) { return; }
        self.pick(&[]);
        self.pins.selected = Some(pin.id.clone());
        self.popup = None;
    }

    pub(super) fn start_pin_drag(&mut self, sheet: usize, pos: Pos2) {
        let Some(pin) = self.pin_at(sheet, pos) else { return };
        if self.doc.as_ref().is_none_or(|doc| doc.session.layers().is_locked(pin.layer)) { return; }
        let Some((x, y)) = self.pdf_point(sheet, pos) else { return };
        let offset = [pin.position[0] - x, pin.position[1] - y];
        self.pick_pin(pin.clone());
        if let Some(doc) = self.doc.as_mut() { doc.session.end_merge(); }
        self.drag = Some(Drag::Pin { sheet, id: pin.id, offset });
    }

    pub(super) fn drag_pin(&mut self, pos: Pos2) {
        let Some(Drag::Pin { sheet, id, offset }) = &self.drag else { return };
        let Some((x, y)) = self.pdf_point(*sheet, pos) else { return };
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(pin) = doc.session.pins().iter().find(|pin| &pin.id == id) else { return };
        if doc.session.layers().is_locked(pin.layer) || !doc.session.layers().is_visible(pin.layer) { return; }
        let Some(geometry) = doc.sheet_geometry(*sheet) else { return };
        let bounds = geometry.bounds;
        let position = [(x + offset[0]).clamp(bounds.left, bounds.right), (y + offset[1]).clamp(bounds.bottom, bounds.top)];
        let command = Command::MovePin { id: id.clone(), position };
        if let Some(doc) = self.doc.as_mut() { doc.session.apply_merged(command); }
    }

    pub(super) fn delete_selected_pin(&mut self) -> bool {
        let pin = self.doc.as_ref().and_then(|doc| doc.session.pins().iter().find(|pin| Some(pin.id.as_str()) == self.pins.selected.as_deref())).cloned();
        let Some(pin) = pin else { return false };
        self.change_pin(&pin, true);
        true
    }

    fn rename_pin(&mut self, pin: &Pin) {
        self.pins.renaming = Some(Rename { id: pin.id.clone(), name: pin.name.clone(), focus: true });
    }

    fn visit_pin(&mut self, pin: &Pin) {
        self.take_up_select();
        self.pick(&[]);
        let Some(sheet) = self.doc.as_ref().and_then(|doc| doc.first_sheet_showing(pin.page)) else { return };
        self.go_to_page(sheet);
        if pin.jump_to_position {
            let [x, y] = pin.position;
            self.scroll_to_box(pin.page, &PdfBox { left: x, right: x, bottom: y, top: y });
        }
        self.pins.selected = Some(pin.id.clone());
    }

    fn change_pin(&mut self, pin: &Pin, remove: bool) {
        let Some(doc) = self.doc.as_mut() else { return };
        let mut pins = doc.session.pins().to_vec();
        let Some(at) = pins.iter().position(|p| p.id == pin.id) else { return };
        if doc.session.layers().is_locked(pins[at].layer) { return; }
        if !remove && (doc.session.layers().get(pin.layer).is_none() || doc.session.layers().is_locked(pin.layer)) { return; }
        if remove { pins.remove(at); } else { pins[at] = pin.clone(); }
        doc.session.apply(Command::SetPins(pins));
        if remove && !doc.session.pins().iter().any(|p| p.id == pin.id) {
            if self.pins.selected.as_deref() == Some(&pin.id) { self.pins.selected = None; }
            if self.pins.renaming.as_ref().is_some_and(|r| r.id == pin.id) { self.pins.renaming = None; }
        }
    }

    pub(super) fn pins_body(&mut self, ui: &mut Ui) {
        self.want_measurements();
        section(ui, "Pins");
        let Some(doc) = self.doc.as_ref() else {
            empty_note(ui, "Open a document to place pins.");
            return;
        };
        match &doc.measurements {
            MeasureRead::Ready => {}
            MeasureRead::Failed(error) => { empty_note(ui, &format!("Could not load pins: {error}")); return; }
            _ => { empty_note(ui, "Loading pins…"); return; }
        }
        let mut pins = doc.session.pins().to_vec();
        // Follow the displayed page numbers, including unsaved page reordering.
        // A stable sort keeps pins on the same page in their existing order.
        pins.sort_by_key(|pin| doc.first_sheet_showing(pin.page).unwrap_or(usize::MAX));
        let layers = doc.session.layers().clone();
        let sheets: Vec<Option<usize>> = pins.iter().map(|pin| doc.first_sheet_showing(pin.page)).collect();
        if styled_button(ui, "Place pin", Tone::Secondary, self.pins.placing).clicked() { self.take_up_pin(); }
        if self.pins.placing { empty_note(ui, "Click the page to place a pin. Esc finishes."); }
        if pins.is_empty() { empty_note(ui, "Place a pin to keep a shortcut to this page."); }
        if self.pins.renaming.as_ref().is_some_and(|r| !pins.iter().any(|pin| pin.id == r.id)) {
            self.pins.renaming = None;
        }
        for (pin, sheet) in pins.iter().zip(sheets) {
            let Some(sheet) = sheet else { continue };
            let selected = self.pins.selected.as_deref() == Some(&pin.id);
            let colour = to_color32(pin.colour);
            let locked = layers.is_locked(pin.layer);
            let mut changed = pin.clone();
            let mut remove = false;
            ui.push_id(&pin.id, |ui| {
                Frame::NONE.fill(colour.gamma_multiply(if selected { 0.32 } else { 0.16 }))
                    .corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(4, 3)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.add_enabled_ui(!locked, |ui| {
                            let (_, chosen) = layers::palette_picker(ui, Some(pin.colour), "Colour this pin");
                            if let Some(colour) = chosen { changed.colour = colour; }
                        });
                        let page_label = format!("{}", sheet + 1);
                        let page_width = ui.painter().layout_no_wrap(page_label.clone(), FontId::proportional(11.5), MUTED).size().x;
                        let width = (ui.available_width() - page_width - 58.0).max(28.0);
                        let renaming = self.pins.renaming.as_ref().is_some_and(|r| r.id == pin.id);
                        if renaming && !locked {
                            let draft = self.pins.renaming.as_mut().unwrap();
                            let edit = ui.add_sized(vec2(width, 22.0), TextEdit::singleline(&mut draft.name));
                            if std::mem::take(&mut draft.focus) { edit.request_focus(); }
                            let escape = (edit.has_focus() || edit.lost_focus()) && ui.input(|i| i.key_pressed(Key::Escape));
                            let enter = edit.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            if escape || enter || edit.lost_focus() {
                                if !escape && !draft.name.trim().is_empty() { changed.name = draft.name.trim().to_owned(); }
                                self.pins.renaming = None;
                            }
                        } else {
                            let label = ui.add_sized(vec2(width, 22.0), egui::Label::new(RichText::new(&pin.name).color(TEXT)).truncate().sense(Sense::click()));
                            if label.double_clicked() && !locked { self.rename_pin(pin); }
                            else if label.clicked() { self.visit_pin(pin); }
                            label.clone().on_hover_text(format!("{}\nClick to go to page {}. Double-click to rename.{}", pin.name, sheet + 1, if layers.is_visible(pin.layer) { "" } else { " Layer hidden." }));
                            label.context_menu(|ui| {
                                ui.add_enabled_ui(!locked, |ui| {
                                    if ui.button("Rename").clicked() { self.rename_pin(pin); ui.close(); }
                                    ui.menu_button("Move to layer", |ui| {
                                        for (layer, depth) in layers.top_first() {
                                            if ui.add_enabled(!layers.is_locked(layer.id), egui::Button::new(format!("{}{}", "  ".repeat(depth), layer.name)).selected(pin.layer == layer.id)).clicked() {
                                                changed.layer = layer.id;
                                                ui.close();
                                            }
                                        }
                                    });
                                });
                            });
                        }
                        ui.label(RichText::new(page_label).size(11.5).color(MUTED));
                        ui.add_enabled_ui(!locked, |ui| {
                            ui.checkbox(&mut changed.jump_to_position, "").on_hover_text("Jump to position: on returns to the pin’s spot; off opens the page");
                            remove = ui.add_sized(vec2(22.0, 22.0), egui::Button::new("×").frame(false)).on_hover_text("Delete pin").clicked();
                        });
                    });
                });
            });
            if remove || changed != *pin { self.change_pin(&changed, remove); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::quantities::tests::Table;

    fn ready() -> Table {
        let mut table = Table::named(&[]);
        table.app.tools = super::super::tools::Tools::default();
        let doc = table.app.doc.as_mut().unwrap();
        doc.geometry = doc.sizes.iter().copied().map(|size| Some(flat_geometry(size))).collect();
        table.app.page_rects.insert(0, Rect::from_min_size(pos2(100.0, 100.0), vec2(300.0, 400.0)));
        table
    }

    #[test]
    fn a_pin_drag_preserves_the_grab_offset_and_undoes_as_one_step() {
        let mut table = ready();
        table.app.take_up_pin();
        table.app.place_pin(0, pos2(175.0, 200.0));
        table.app.take_up_select();
        table.app.tool_panel_open = false;
        table.app.start_pin_drag(0, pos2(175.0, 188.0));
        assert!(matches!(table.app.drag, Some(Drag::Pin { .. })));
        assert!(!table.app.tool_panel_open, "selecting must not move the page under the pointer");
        table.app.drag_pin(pos2(175.0, 188.0));
        assert_eq!(table.app.doc.as_ref().unwrap().session.pins()[0].position, [150.0, 600.0]);
        table.app.drag_pin(pos2(185.0, 198.0));
        table.app.drag_pin(pos2(210.0, 208.0));
        let session = &mut table.app.doc.as_mut().unwrap().session;
        session.end_merge();
        assert_eq!(session.pins()[0].position, [220.0, 560.0]);
        assert!(session.undo());
        assert_eq!(session.pins()[0].position, [150.0, 600.0]);
        assert!(session.redo());
        assert_eq!(session.pins()[0].position, [220.0, 560.0]);
        table.app.drag = None;
        table.app.delete_picked();
        assert!(table.app.doc.as_ref().unwrap().session.pins().is_empty());
        assert!(table.app.pins.selected.is_none());
    }

    #[test]
    fn pins_move_in_the_displayed_direction_on_a_rotated_page_and_stay_on_it() {
        let mut table = ready();
        table.app.take_up_pin();
        table.app.place_pin(0, pos2(175.0, 200.0));
        table.app.take_up_select();
        let doc = table.app.doc.as_mut().unwrap();
        doc.arrange.select_all();
        doc.arrange.rotate(1);
        table.app.page_rects.insert(0, Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0)));
        table.app.start_pin_drag(0, pos2(300.0, 63.0));
        table.app.drag_pin(pos2(340.0, 93.0));
        let position = table.app.doc.as_ref().unwrap().session.pins()[0].position;
        assert!((position[0] - 210.0).abs() < 0.001);
        assert!((position[1] - 680.0).abs() < 0.001);
        table.app.drag_pin(pos2(2000.0, -2000.0));
        assert_eq!(table.app.doc.as_ref().unwrap().session.pins()[0].position, [0.0, 800.0]);
    }

    #[test]
    fn locked_pins_cannot_be_dragged_and_selecting_elsewhere_clears_pin_selection() {
        let mut table = ready();
        table.app.take_up_pin();
        table.app.place_pin(0, pos2(175.0, 200.0));
        table.app.take_up_select();
        let doc = table.app.doc.as_mut().unwrap();
        let mut layers = doc.session.layers().clone();
        layers.set_locked(doc.session.pins()[0].layer, true);
        doc.session.apply(Command::SetLayers(layers));
        table.app.start_pin_drag(0, pos2(175.0, 188.0));
        assert!(table.app.drag.is_none());
        table.app.settle_picked();
        assert!(table.app.pins.selected.is_none());
        table.app.pins.selected = Some("test pin".into());
        table.app.pick(&[]);
        assert!(table.app.pins.selected.is_none());
    }

    #[test]
    fn inline_rename_commits_on_enter_and_cancels_on_escape() {
        let mut table = ready();
        let pin = Pin::new(0, [120.0, 300.0], "Entry".into(), [0.2, 0.5, 0.8]);
        table.app.doc.as_mut().unwrap().session.apply(Command::SetPins(vec![pin.clone()]));
        let frame = |table: &mut Table, key: Option<Key>| {
            let events = key.map(|key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }).into_iter().collect();
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(260.0, 800.0))), events, ..Default::default() };
            let mut output = table.ctx.run_ui(raw, |ui| table.app.pins_body(ui));
            output.textures_delta.clear();
        };
        table.app.rename_pin(&pin);
        frame(&mut table, None);
        table.app.pins.renaming.as_mut().unwrap().name = "  West entry  ".into();
        frame(&mut table, Some(Key::Enter));
        assert_eq!(table.app.doc.as_ref().unwrap().session.pins()[0].name, "West entry");
        let pin = table.app.doc.as_ref().unwrap().session.pins()[0].clone();
        table.app.rename_pin(&pin);
        frame(&mut table, None);
        table.app.pins.renaming.as_mut().unwrap().name = "Discard this".into();
        frame(&mut table, Some(Key::Escape));
        assert_eq!(table.app.doc.as_ref().unwrap().session.pins()[0].name, "West entry");
        assert!(table.app.pins.renaming.is_none());
    }

    #[test]
    fn escape_puts_down_the_pin_tool_and_delete_removes_the_selected_pin() {
        let mut table = ready();
        table.app.take_up_pin();
        table.app.place_pin(0, pos2(175.0, 200.0));
        let key = |table: &mut Table, key| {
            let raw = egui::RawInput {
                events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }],
                ..Default::default()
            };
            let mut output = table.ctx.run_ui(raw, |ui| table.app.measure_keys(ui.ctx()));
            output.textures_delta.clear();
        };
        key(&mut table, Key::Escape);
        assert!(table.app.selecting());
        assert!(table.app.pins.selected.is_some());
        key(&mut table, Key::Delete);
        assert!(table.app.doc.as_ref().unwrap().session.pins().is_empty());
        assert!(table.app.doc.as_mut().unwrap().session.undo());
        assert_eq!(table.app.doc.as_ref().unwrap().session.pins().len(), 1);
    }

    #[test]
    fn placing_a_pin_uses_pdf_coordinates_and_tool_settings_and_undoes() {
        let mut table = ready();
        let mut settings = table.app.tools.settings(ToolKey::Pin);
        settings.defaults.name = "West entrance".into();
        settings.style.stroke = [0.1, 0.2, 0.9];
        table.app.tools.set(ToolKey::Pin, settings);
        table.app.take_up_pin();
        assert!(!table.app.selecting());
        assert!(matches!(table.app.drag_starts(false), DragStart::Nothing));
        table.app.place_pin(0, pos2(175.0, 200.0));
        let pin = table.app.doc.as_ref().unwrap().session.pins()[0].clone();
        assert_eq!(pin.name, "West entrance");
        assert_eq!(pin.colour, [0.1, 0.2, 0.9]);
        assert_eq!(pin.position, [150.0, 600.0]);
        assert!(table.app.doc.as_mut().unwrap().session.undo());
        assert!(table.app.doc.as_ref().unwrap().session.pins().is_empty());
        table.app.take_up_drawing(crate::domain::MarkupKind::Rectangle);
        assert!(!table.app.pins.placing);
    }

    #[test]
    fn hidden_layers_hide_pin_markers_and_locked_layers_prevent_edits() {
        let mut table = ready();
        table.app.take_up_pin();
        table.app.place_pin(0, pos2(175.0, 200.0));
        let mut pin = table.app.doc.as_ref().unwrap().session.pins()[0].clone();
        let hit = pos2(175.0, 188.0);
        assert!(table.app.pin_at(0, hit).is_some());
        let doc = table.app.doc.as_mut().unwrap();
        let mut layers = doc.session.layers().clone();
        layers.set_visible(pin.layer, false);
        layers.set_locked(pin.layer, true);
        doc.session.apply(Command::SetLayers(layers));
        assert!(table.app.pin_at(0, hit).is_none());
        pin.name = "Must not change".into();
        table.app.change_pin(&pin, false);
        table.app.change_pin(&pin, true);
        assert_eq!(table.app.doc.as_ref().unwrap().session.pins()[0].name, "Pin 1");
    }

    #[test]
    fn each_pin_controls_whether_navigation_targets_the_page_or_the_position() {
        let mut table = ready();
        table.app.viewer_rect = Rect::from_min_size(Pos2::ZERO, vec2(500.0, 400.0));
        table.app.zoom_mode = ZoomMode::Custom;
        table.app.zoom = 2.0;
        let mut pin = Pin::new(1, [450.0, 180.0], "Detail".into(), [0.2, 0.5, 0.8]);
        pin.jump_to_position = false;
        table.app.visit_pin(&pin);
        let page_top = table.app.scroll_y.unwrap();
        assert_eq!(table.app.current_page, 1);
        pin.jump_to_position = true;
        table.app.visit_pin(&pin);
        assert!(table.app.scroll_y.unwrap() > page_top + 100.0);
        assert!(table.app.scroll_x.is_some());
    }

    #[test]
    fn pins_panel_renders_inline_rename_at_minimum_width() {
        let mut table = ready();
        let pin = Pin::new(0, [120.0, 300.0], "A very long pin name that needs to truncate in the left panel".into(), [0.2, 0.5, 0.8]);
        table.app.doc.as_mut().unwrap().session.apply(Command::SetPins(vec![pin.clone()]));
        table.app.rename_pin(&pin);
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 800.0))), ..Default::default() };
        let mut output = table.ctx.run_ui(raw, |ui| {
            ui.set_width(172.0);
            table.app.pins_body(ui);
        });
        output.textures_delta.clear();
        assert!(!output.shapes.is_empty());
        assert!(table.app.pins.renaming.is_some());
    }
}
