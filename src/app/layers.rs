//! The layers side of the panel: which layers there are, which are shown and
//! locked, what is in front of what, and moving what is picked out between
//! them or forward and back within one.
//!
//! The list is a tree, front first as a stack of sheets reads. A layer is
//! dragged by its name: dropped on the top edge of another it goes in front
//! of it, on the bottom edge behind it, and on the middle inside it, where it
//! is one of a folder's layers. Everything here is asked for through
//! `layering`, so each is one step to undo.

use markup_model::{LayerId, Place, Restack};

use super::icons::Icon;
use super::*;
use crate::layering;

/// What a click in the panel asked for. Collected while the list is drawn,
/// which borrows the document, and done once it has been.
enum Ask {
    Add,
    AddIn(LayerId),
    Show(LayerId, bool),
    Lock(LayerId, bool),
    Colour(LayerId, Option<[f32; 3]>),
    Use(LayerId),
    Fold(LayerId),
    Drop(LayerId, Place),
    Remove(LayerId),
    Rename(LayerId, String),
    Select(LayerId),
}

/// Which part of a row a pointer is over, from the top of it: the top fifth
/// puts a dragged layer in front, the bottom fifth behind, the rest inside.
fn drop_on(row: Rect, y: f32, target: LayerId) -> Place {
    let across = (y - row.min.y) / row.height().max(1.0);
    if across < 0.2 {
        Place::Above(target)
    } else if across > 0.8 {
        Place::Below(target)
    } else {
        Place::Into(target)
    }
}

/// A palette button that opens a colour picker, and the colour chosen this
/// frame, if one was. `current` is where the picker starts. Nothing on it is
/// a coloured swatch: the colour shows where it is used.
pub(super) fn palette_picker(ui: &mut Ui, current: Option<[f32; 3]>, hover: &str) -> (egui::Response, Option<[f32; 3]>) {
    let button = tool_button_sized(ui, Icon::Palette, Tone::Ghost, false, vec2(24.0, 22.0)).on_hover_text(hover);
    let start = current.unwrap_or([0.85, 0.86, 0.88]);
    // Wide enough to be a picker: left to itself the popup is as wide as the
    // small button it opens from.
    let picked = egui::Popup::from_toggle_button_response(&button).width(240.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.spacing_mut().slider_width = 200.0;
        ui.set_min_width(230.0);
        let mut colour = Color32::from_rgb((start[0] * 255.0) as u8, (start[1] * 255.0) as u8, (start[2] * 255.0) as u8);
        egui::color_picker::color_picker_color32(ui, &mut colour, egui::color_picker::Alpha::Opaque).then(|| [f32::from(colour.r()) / 255.0, f32::from(colour.g()) / 255.0, f32::from(colour.b()) / 255.0])
    });
    (button, picked.and_then(|p| p.inner))
}

impl App {
    pub(super) fn layers_body(&mut self, ui: &mut Ui) {
        let Some(doc) = self.doc.as_ref() else {
            empty_note(ui, "Open a document to see its layers.");
            return;
        };
        let session = &doc.session;
        let stack = session.layers();
        let active = session.active_layer();
        let mut counts: HashMap<LayerId, usize> = HashMap::new();
        for (m, _) in session.measures().iter() {
            *counts.entry(m.layer).or_default() += 1;
        }
        for pin in session.pins() {
            *counts.entry(pin.layer).or_default() += 1;
        }
        let mut asked: Vec<Ask> = Vec::new();
        let ctx = ui.ctx().clone();

        ui.horizontal(|ui| {
            ui.label(RichText::new("Layers").size(11.5).strong().color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if styled_button(ui, "Add layer", Tone::Secondary, false).on_hover_text("A new layer in front, for what is drawn next").clicked() {
                    asked.push(Ask::Add);
                }
            });
        });
        ui.label(RichText::new("Front at the top. Drag a layer by its name: on the edge of another to move it there, on the middle to put it inside. Right-click for more.").size(11.5).color(SUBTLE));

        // Where each row was drawn, for working out what a drag is over.
        let mut rows: Vec<(LayerId, Rect)> = Vec::new();
        for (layer, depth) in stack.top_first() {
            let id = layer.id;
            // What is inside a folded layer isn't listed.
            let mut hidden_by_fold = false;
            let mut at = stack.parent_of(id);
            while let Some(parent) = at {
                if self.layers_folded.contains(&parent) {
                    hidden_by_fold = true;
                    break;
                }
                at = stack.parent_of(parent);
            }
            if hidden_by_fold {
                continue;
            }
            let is_active = id == active;
            let shown = stack.is_visible(id);
            let has_inside = !stack.children(id).is_empty();
            // A layer with a colour is that colour, softly, across its whole row;
            // the one being drawn on is a shade stronger, with its dot.
            let fill = match (layer.colour, is_active) {
                (Some([r, g, b]), active) => Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8).gamma_multiply(if active { 0.38 } else { 0.2 }),
                (None, true) => ACCENT_SOFT,
                (None, false) => Color32::TRANSPARENT,
            };
            let asked_before = asked.len();
            let mut swatch_used = false;
            let row = Frame::NONE.fill(fill).corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(4, 2)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.add_space(depth as f32 * 14.0);
                    if has_inside {
                        let folded = self.layers_folded.contains(&id);
                        let chevron = if folded { Icon::Right } else { Icon::Down };
                        if tool_button_sized(ui, chevron, Tone::Ghost, false, vec2(18.0, 22.0)).on_hover_text(if folded { "Show what is inside" } else { "Fold away what is inside" }).clicked() {
                            asked.push(Ask::Fold(id));
                        }
                    } else {
                        ui.add_space(20.0);
                    }
                    let eye = if layer.visible { Icon::Eye } else { Icon::EyeOff };
                    let tip = if layer.visible { "Shown. Click to hide it" } else { "Hidden. Click to show it" };
                    if tool_button_sized(ui, eye, Tone::Ghost, false, vec2(24.0, 22.0)).on_hover_text(tip).clicked() {
                        asked.push(Ask::Show(id, !layer.visible));
                    }
                    let lock = if layer.locked { Icon::Lock } else { Icon::Unlock };
                    let tip = if layer.locked { "Locked. Click to let it be changed" } else { "Click to lock it against changes" };
                    if tool_button_sized(ui, lock, Tone::Ghost, false, vec2(24.0, 22.0)).on_hover_text(tip).clicked() {
                        asked.push(Ask::Lock(id, !layer.locked));
                    }
                    // Drawn, not typed: a bullet glyph is missing from the
                    // panel's font and shows as an empty square.
                    let (mark, _) = ui.allocate_exact_size(vec2(12.0, 16.0), Sense::hover());
                    if is_active {
                        ui.painter().circle_filled(mark.center(), 3.5, ACCENT);
                    }
                    // A colour to tell layers apart by: the row takes it on.
                    let (palette, chosen) = palette_picker(ui, layer.colour, "Colour this layer. Right-click its name to clear the colour");
                    swatch_used = palette.clicked();
                    if let Some(rgb) = chosen {
                        asked.push(Ask::Colour(id, Some(rgb)));
                    }
                    match self.layer_rename.as_mut().filter(|(renaming, _)| *renaming == id) {
                        Some((_, name)) => {
                            // A width of its own, leaving room for the count
                            // after it: an unbounded box in a panel that sizes
                            // itself to what's in it grows every frame.
                            let room = (ui.available_width() - 40.0).max(60.0);
                            let edit = ui.add(egui::TextEdit::singleline(name).desired_width(room));
                            // Focus is asked for once, when the box first
                            // appears. Asked for every frame, it is taken
                            // straight back on the frame it is let go, and
                            // can't be left.
                            let first = ui.data_mut(|d| !d.get_temp::<bool>(edit.id).unwrap_or(false));
                            if first {
                                ui.data_mut(|d| d.insert_temp(edit.id, true));
                                edit.request_focus();
                            }
                            if edit.lost_focus() {
                                ui.data_mut(|d| d.remove_temp::<bool>(edit.id));
                                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                if enter && !escape {
                                    asked.push(Ask::Rename(id, name.clone()));
                                }
                                self.layer_rename = None;
                            }
                        }
                        None => {
                            let title = RichText::new(&layer.name).size(13.0).color(if shown { TEXT } else { SUBTLE });
                            let name = ui.add(egui::Label::new(title).truncate().sense(Sense::click_and_drag()));
                            name.dnd_set_drag_payload(id);
                            if name.clicked() {
                                asked.push(Ask::Use(id));
                            }
                            if name.double_clicked() {
                                self.layer_rename = Some((id, layer.name.clone()));
                            }
                            name.clone().on_hover_text("Click to draw on it. Double-click to rename it. Drag to move it");
                            name.context_menu(|ui| {
                                if ui.button("Rename").clicked() {
                                    self.layer_rename = Some((id, layer.name.clone()));
                                    ui.close();
                                }
                                if ui.button("Add a layer inside").clicked() {
                                    asked.push(Ask::AddIn(id));
                                    ui.close();
                                }
                                if ui.add_enabled(stack.get(id).is_some_and(|l| l.colour.is_some()), egui::Button::new("Clear its colour")).clicked() {
                                    asked.push(Ask::Colour(id, None));
                                    ui.close();
                                }
                                if ui.button("Pick out everything on it").clicked() {
                                    asked.push(Ask::Select(id));
                                    ui.close();
                                }
                                ui.separator();
                                if ui.add_enabled(!id.is_default(), egui::Button::new("Take the layer out")).on_disabled_hover_text("The default layer stays").clicked() {
                                    asked.push(Ask::Remove(id));
                                    ui.close();
                                }
                            });
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let n = counts.get(&id).copied().unwrap_or(0);
                        let count = ui.add(egui::Label::new(RichText::new(n.to_string()).size(12.0).color(MUTED)).sense(Sense::click()));
                        if count.on_hover_text("How many are on it. Click to pick them out").clicked() {
                            asked.push(Ask::Select(id));
                        }
                    });
                });
            });
            // A click anywhere else on the row draws on the layer, unless the
            // click was for one of the buttons on it or for its name box.
            let rect = row.response.rect;
            if asked.len() == asked_before && !swatch_used && self.layer_rename.is_none() && ui.input(|i| i.pointer.primary_clicked()) && ui.rect_contains_pointer(rect) {
                asked.push(Ask::Use(id));
            }
            rows.push((id, rect));
        }

        // What a layer being dragged is over, and what letting go would do.
        if let Some(dragged) = egui::DragAndDrop::payload::<LayerId>(&ctx).map(|p| *p) {
            let pointer = ctx.pointer_hover_pos();
            let over = pointer.and_then(|p| rows.iter().find(|(_, rect)| rect.contains(p)).map(|&(id, rect)| (id, rect, p)));
            let would = over.map(|(target, rect, p)| (drop_on(rect, p.y, target), rect)).filter(|(place, _)| {
                let mut trial = stack.clone();
                trial.place(dragged, *place)
            });
            if let Some((place, rect)) = would {
                let painter = ui.painter();
                match place {
                    Place::Above(_) => { painter.hline(rect.x_range(), rect.min.y, Stroke::new(2.0, ACCENT)); }
                    Place::Below(_) => { painter.hline(rect.x_range(), rect.max.y, Stroke::new(2.0, ACCENT)); }
                    Place::Into(_) => { painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(2.0, ACCENT), StrokeKind::Inside); }
                }
            }
            if let Some(pointer) = pointer {
                egui::Area::new(Id::new("layer-drag-name")).order(egui::Order::Tooltip).fixed_pos(pointer + vec2(12.0, 8.0)).interactable(false).show(&ctx, |ui| {
                    Frame::popup(ui.style()).show(ui, |ui| ui.label(stack.name(dragged)));
                });
            }
            if ctx.input(|i| i.pointer.any_released()) {
                if let Some((place, _)) = would {
                    asked.push(Ask::Drop(dragged, place));
                }
                egui::DragAndDrop::clear_payload(&ctx);
            }
        }

        for ask in asked {
            self.answer_layer_ask(ask);
        }
    }

    fn answer_layer_ask(&mut self, ask: Ask) {
        let Some(doc) = self.doc.as_mut() else { return };
        let session = &mut doc.session;
        // What a change to the layers leaves picked out: nothing on a layer
        // that can no longer be touched.
        let mut unpick: Option<LayerId> = None;
        let command = match ask {
            Ask::Add | Ask::AddIn(_) => {
                let parent = if let Ask::AddIn(parent) = ask { Some(parent) } else { None };
                let (command, id) = layering::add_in(session, parent, "Layer");
                session.apply(command);
                session.set_active_layer(id);
                let name = session.layers().name(id).to_owned();
                self.layer_rename = Some((id, name));
                if let Some(parent) = parent {
                    self.layers_folded.remove(&parent);
                }
                return;
            }
            Ask::Show(id, on) => {
                unpick = (!on).then_some(id);
                layering::edit(session, |l| {
                    l.set_visible(id, on);
                })
            }
            Ask::Lock(id, on) => {
                unpick = on.then_some(id);
                layering::edit(session, |l| {
                    l.set_locked(id, on);
                })
            }
            Ask::Colour(id, colour) => layering::edit(session, |l| {
                l.set_colour(id, colour);
            }),
            Ask::Use(id) => {
                session.set_active_layer(id);
                None
            }
            Ask::Fold(id) => {
                if !self.layers_folded.remove(&id) {
                    self.layers_folded.insert(id);
                }
                return;
            }
            Ask::Drop(id, place) => {
                if let Place::Into(folder) = place {
                    self.layers_folded.remove(&folder);
                }
                layering::edit(session, |l| {
                    l.place(id, place);
                })
            }
            Ask::Remove(id) => {
                unpick = Some(id);
                layering::remove(session, id)
            }
            Ask::Rename(id, name) => layering::edit(session, |l| {
                l.rename(id, &name);
            }),
            Ask::Select(id) => {
                let rows: Vec<RowId> = session.measures().on_layer(id).filter(|m| session.layers().is_editable(m)).map(|m| RowId::Measure(m.id)).collect();
                self.pick(&rows);
                return;
            }
        };
        if let Some(command) = command {
            session.apply(command);
        }
        if let Some(layer) = unpick {
            self.unpick_layer(layer);
        }
    }

    /// Lets go of whatever is picked out on `layer`, or inside it, which can
    /// no longer be touched.
    fn unpick_layer(&mut self, layer: LayerId) {
        let Some(doc) = self.doc.as_ref() else { return };
        let stack = doc.session.layers();
        let gone = |id: &MarkupId| doc.session.measures().get(*id).is_none_or(|m| m.layer == layer || stack.is_inside(m.layer, layer));
        let stays: Vec<RowId> = self.picked_rows().into_iter().filter(|row| !matches!(row, RowId::Measure(id) if gone(id))).collect();
        if stays.len() != self.picked_rows().len() {
            self.pick(&stays);
        }
    }

    /// Moves what a right-click was on, or everything picked out, to `layer`,
    /// in front of what is there. `None` makes a layer for them and opens the
    /// layers side to name it.
    pub(super) fn move_target_to_layer(&mut self, target: context::Target, layer: Option<LayerId>) {
        let ids: Vec<MarkupId> = match target {
            context::Target::Measurement(id) => vec![id],
            _ => self.picked_rows().into_iter().filter_map(|row| if let RowId::Measure(id) = row { Some(id) } else { None }).collect(),
        };
        let Some(doc) = self.doc.as_mut() else { return };
        let session = &mut doc.session;
        let (made, to) = match layer {
            Some(to) => (None, to),
            None => {
                let (command, id) = layering::add(session, "Layer");
                (Some(command), id)
            }
        };
        let moved = layering::move_to_layer(session, &ids, to);
        let steps: Vec<_> = made.clone().into_iter().chain(moved).collect();
        if !steps.is_empty() {
            session.apply(crate::session::Command::Batch(steps));
        }
        if made.is_some() {
            let name = session.layers().name(to).to_owned();
            self.layer_rename = Some((to, name));
            self.show_tool_panel(tool_panel::Tab::Layers);
        }
    }

    /// Every layer of the open document as a path, front first: what a tool
    /// can be set to draw on.
    pub(super) fn layer_paths(&self) -> Vec<String> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let stack = doc.session.layers();
        stack.top_first().into_iter().map(|(layer, _)| stack.path(layer.id)).collect()
    }

    /// Moves what a right-click was on, or everything picked out, forward or
    /// back within its layers.
    pub(super) fn restack_target(&mut self, target: context::Target, how: Restack) {
        let ids: Vec<MarkupId> = match target {
            context::Target::Measurement(id) => vec![id],
            _ => self.picked_rows().into_iter().filter_map(|row| if let RowId::Measure(id) = row { Some(id) } else { None }).collect(),
        };
        let Some(doc) = self.doc.as_mut() else { return };
        if let Some(command) = layering::restack(&doc.session, &ids, how) {
            doc.session.apply(command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::quantities::tests::Table;
    use super::*;

    #[test]
    fn a_drop_is_in_front_behind_or_inside_by_where_on_the_row_it_lands() {
        let row = Rect::from_min_size(pos2(0.0, 100.0), vec2(200.0, 20.0));
        let target = LayerId::new();
        assert_eq!(drop_on(row, 101.0, target), Place::Above(target));
        assert_eq!(drop_on(row, 110.0, target), Place::Into(target));
        assert_eq!(drop_on(row, 119.0, target), Place::Below(target));
    }

    #[test]
    fn the_panel_draws_and_what_it_is_asked_is_done_and_undone() {
        let mut table = Table::named(&["a", "b", "c"]);
        let ctx = table.ctx.clone();
        let app = &mut table.app;
        let frame = |app: &mut App| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.layers_body(ui));
            output.textures_delta.clear();
        };
        frame(app);

        let ids: Vec<MarkupId> = {
            let doc = app.doc.as_ref().unwrap();
            doc.session.measures().stacked(0, doc.session.layers()).into_iter().map(|m| m.id).collect()
        };
        let layers = |app: &App| app.doc.as_ref().unwrap().session.layers().clone();

        // Two picked out and moved to a layer made for them on the spot, which
        // is then ready to be named; all of it one step.
        app.pick(&[RowId::Measure(ids[0]), RowId::Measure(ids[1])]);
        app.move_target_to_layer(context::Target::Picked, None);
        let notes = layers(app).layers().last().unwrap().id;
        assert_eq!(app.layer_rename.as_ref().map(|(id, _)| *id), Some(notes), "a new layer is ready to be named");
        let on = |app: &App| app.doc.as_ref().unwrap().session.measures().on_layer(notes).count();
        assert_eq!(on(app), 2);
        app.answer_layer_ask(Ask::Rename(notes, "Notes".into()));
        assert_eq!(layers(app).name(notes), "Notes");
        app.answer_layer_ask(Ask::Colour(notes, Some([1.0, 0.0, 0.0])));
        assert_eq!(layers(app).get(notes).unwrap().colour, Some([1.0, 0.0, 0.0]));
        app.answer_layer_ask(Ask::Colour(notes, None));
        assert_eq!(layers(app).get(notes).unwrap().colour, None);
        frame(app);

        // A layer put inside another, which is then folded away.
        app.answer_layer_ask(Ask::AddIn(notes));
        let inside = layers(app).layers().last().unwrap().id;
        assert_eq!(layers(app).parent_of(inside), Some(notes));
        app.layer_rename = None;
        app.answer_layer_ask(Ask::Fold(notes));
        frame(app);
        app.answer_layer_ask(Ask::Drop(inside, Place::Above(LayerId::DEFAULT)));
        assert_eq!(layers(app).parent_of(inside), None, "dragged out to the top level");

        // Hiding a layer lets go of what was picked out on it.
        app.answer_layer_ask(Ask::Show(notes, false));
        assert!(app.picked_rows().is_empty());
        let shown = app.doc.as_ref().unwrap().session.measures().stacked(0, &layers(app)).len();
        assert_eq!(shown, 1, "the two on the hidden layer are not drawn");
        frame(app);

        app.answer_layer_ask(Ask::Show(notes, true));
        app.restack_target(context::Target::Measurement(ids[0]), Restack::ToBack);
        app.answer_layer_ask(Ask::Remove(notes));
        assert_eq!(on(app), 0);
        assert_eq!(app.doc.as_ref().unwrap().session.measures().on_layer(LayerId::DEFAULT).count(), 3);
        while app.doc.as_mut().unwrap().session.undo() {}
        assert_eq!(layers(app).layers().len(), 1, "undone back to the default layer alone");
    }
}

#[cfg(test)]
mod lock_tests {
    use super::super::quantities::tests::Table;
    use super::*;

    #[test]
    fn a_locked_layers_markups_cannot_be_picked_on_the_page() {
        let mut table = Table::named(&["a"]);
        let app = &mut table.app;
        let at = |app: &App| measure::measurement_at_in(app.doc.as_ref().unwrap(), 0, (100.0, 700.0), 4.0);
        assert!(at(app).is_some(), "on the line");
        app.answer_layer_ask(Ask::Lock(LayerId::DEFAULT, true));
        assert!(at(app).is_none(), "locked: nothing there to pick");
        let session = &app.doc.as_ref().unwrap().session;
        assert!(session.layers().is_locked(LayerId::DEFAULT));
        assert!(app.doc.as_ref().unwrap().session.measures().iter().all(|(m, _)| m.layer == LayerId::DEFAULT));
        // No way in picks it: not the table, not a paste.
        let id = app.doc.as_ref().unwrap().session.measures().iter().next().unwrap().0.id;
        app.pick(&[RowId::Measure(id)]);
        assert!(app.picked_rows().is_empty(), "locked: can't be picked out");
        app.answer_layer_ask(Ask::Lock(LayerId::DEFAULT, false));
        assert!(at(app).is_some(), "unlocked: picked again");
        app.pick(&[RowId::Measure(id)]);
        assert_eq!(app.picked_rows(), vec![RowId::Measure(id)]);
        // Hidden is the same as locked for picking: it can't be seen to be picked.
        app.answer_layer_ask(Ask::Show(LayerId::DEFAULT, false));
        assert!(at(app).is_none() && app.picked_rows().is_empty(), "hidden: nothing to pick, and what was picked is let go");
    }
}

#[cfg(test)]
mod shape_tests {
    use super::super::quantities::tests::Table;
    use super::*;
    use crate::domain::{DrawStyle, Markup, MarkupKind as Drawn, PdfBox};
    use markup_model::MarkupKind;

    /// A shape as the drawing tools hand it over when the pointer is let go.
    fn dragged(kind: Drawn, points: Vec<[f32; 2]>) -> Markup {
        Markup {
            key: None,
            page: 0,
            kind,
            points,
            bounds: PdfBox { left: 0.0, bottom: 0.0, right: 0.0, top: 0.0 },
            color: [1.0, 0.0, 0.0],
            width: 2.0,
            style: DrawStyle::default(),
            name: String::new(),
            comment: String::new(),
            author: "tester".into(),
        }
    }

    #[test]
    fn what_the_drawing_tools_draw_goes_on_layers_and_can_be_locked_hidden_and_ordered() {
        let mut table = Table::named(&[]);
        let app = &mut table.app;
        // A frame with nothing in it, the fill the tool starts with taken off.
        let key = super::tools::ToolKey::Draw(Drawn::Rectangle);
        let mut empty = app.tools.settings(key);
        empty.style.fill = None;
        app.tools.set(key, empty);
        let shapes = [
            dragged(Drawn::Rectangle, vec![[100.0, 100.0], [300.0, 220.0]]),
            dragged(Drawn::Ellipse, vec![[100.0, 300.0], [300.0, 400.0]]),
            dragged(Drawn::Line, vec![[50.0, 500.0], [250.0, 520.0]]),
            dragged(Drawn::Arrow, vec![[50.0, 600.0], [250.0, 640.0]]),
            dragged(Drawn::Pen, vec![[10.0, 700.0], [40.0, 720.0], [80.0, 705.0]]),
        ];
        for shape in shapes {
            app.add_markup(shape);
        }
        let all = |app: &App| -> Vec<(MarkupKind, LayerId)> {
            let doc = app.doc.as_ref().unwrap();
            doc.session.measures().stacked(0, doc.session.layers()).into_iter().map(|m| (m.kind, m.layer)).collect()
        };
        let kinds: Vec<MarkupKind> = all(app).into_iter().map(|(kind, _)| kind).collect();
        assert_eq!(kinds, [MarkupKind::Box, MarkupKind::Ellipse, MarkupKind::Line, MarkupKind::Arrow, MarkupKind::Pen], "in the order drawn");
        assert!(app.doc.as_ref().unwrap().session.markups().is_empty(), "none held the older way");
        assert!(!app.doc.as_ref().unwrap().session.measures().iter().any(|(m, _)| m.kind.is_measurement()), "and none measured");

        // On the frame's edge, and it is picked; lock its layer and it isn't.
        let at = |app: &App, x: f32, y: f32| measure::measurement_at_in(app.doc.as_ref().unwrap(), 0, (x, y), 4.0).map(|(id, _)| id);
        assert!(at(app, 200.0, 100.0).is_some(), "the frame's bottom edge");
        assert!(at(app, 200.0, 160.0).is_none(), "the empty inside lets clicks through");
        assert!(at(app, 25.0, 510.0).is_some() || at(app, 100.0, 505.0).is_some(), "the line");
        assert!(at(app, 40.0, 720.0).is_some(), "the pen stroke");
        app.answer_layer_ask(Ask::Lock(LayerId::DEFAULT, true));
        assert!(at(app, 200.0, 100.0).is_none() && at(app, 40.0, 720.0).is_none(), "locked: none of them can be picked");
        app.answer_layer_ask(Ask::Lock(LayerId::DEFAULT, false));

        // Moved to a layer of their own and hidden, they are not drawn.
        app.pick(&[]);
        let ids: Vec<MarkupId> = app.doc.as_ref().unwrap().session.measures().iter().map(|(m, _)| m.id).collect();
        let rows: Vec<RowId> = ids.iter().map(|&id| RowId::Measure(id)).collect();
        app.pick(&rows);
        app.move_target_to_layer(context::Target::Picked, None);
        let mine = app.doc.as_ref().unwrap().session.layers().layers().last().unwrap().id;
        assert!(all(app).iter().all(|(_, layer)| *layer == mine));
        app.answer_layer_ask(Ask::Show(mine, false));
        assert!(all(app).is_empty(), "hidden: nothing drawn");
        // One undo brings back the layer's visibility, and another the move.
        assert!(app.doc.as_mut().unwrap().session.undo());
        assert_eq!(all(app).len(), 5);
    }
}

#[cfg(test)]
mod zoomed_out {
    use super::super::quantities::tests::Table;
    use super::*;
    use eframe::egui::epaint::Shape as S;

    fn colour_seen(shapes: &[egui::epaint::ClippedShape], want: Color32) -> usize {
        fn walk(s: &S, want: Color32, n: &mut usize) {
            match s {
                S::LineSegment { stroke, .. } if stroke.color == want => *n += 1,
                S::Path(p) if matches!(p.stroke.color, egui::epaint::ColorMode::Solid(c) if c == want) => *n += 1,
                S::Vec(v) => v.iter().for_each(|s| walk(s, want, n)),
                _ => {}
            }
        }
        let mut n = 0;
        shapes.iter().for_each(|c| walk(&c.shape, want, &mut n));
        n
    }

    #[test]
    fn markups_stay_drawn_at_every_zoom_including_the_sheet_view_past_twenty_per_cent() {
        for (zoom, with_thumb) in [(1.0f32, false), (0.5, true), (0.25, true), (0.2, true), (0.2, false), (0.15, true), (0.1, true), (0.05, true)] {
            let mut table = Table::named(&["a"]);
            let ctx = table.ctx.clone();
            let app = &mut table.app;
            {
                let doc = app.doc.as_mut().unwrap();
                doc.sizes = vec![egui::vec2(3370.0, 2384.0); 2];
                doc.geometry[0] = Some(crate::domain::PageGeometry { rotation: 0, bounds: crate::domain::PdfBox { left: 0.0, bottom: 0.0, right: 3370.0, top: 2384.0 } });
                let id = doc.session.measures().iter().next().unwrap().0.id;
                let mut m = doc.session.measures().get(id).unwrap().clone();
                m.style.stroke = [0.1, 0.4, 0.9];
                m.style.width = 8.0;
                m.geometry = markup_model::Geometry::Line { a: markup_model::Pt::new(300.0, 1200.0), b: markup_model::Pt::new(2000.0, 1200.0) };
                doc.session.apply(crate::session::Command::ChangeMeasure(Box::new(m)));
                let tex = ctx.load_texture("t", egui::ColorImage::filled([8, 8], Color32::WHITE), Default::default());
                if with_thumb { doc.render.thumbnails.insert(0, Thumbnail { handle: tex, used: 0.0 }); }
            }
            app.zoom = zoom;
            app.zoom_mode = ZoomMode::Custom;
            let mut seen = 0;
            for _ in 0..3 {
                let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 900.0))), ..Default::default() };
                let mut output = ctx.run_ui(raw, |ui| app.viewer(ui));
                output.textures_delta.clear();
                seen = colour_seen(&output.shapes, Color32::from_rgb(26, 102, 230));
            }
            assert!(seen > 0, "zoom {zoom} (thumbnail {with_thumb}): the measurement is not drawn");
        }
    }
}
