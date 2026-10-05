//! Manual comparison alignment: a fixed original and a movable green sheet.

use super::*;

const GREEN: Color32 = Color32::from_rgb(25, 153, 64);
const MIN_SCALE: f32 = 0.02;
const MAX_SCALE: f32 = 50.0;

/// Project a corner's movement onto its diagonal, preserving proportions
/// and resizing around the sheet's centre rather than its opposite corner.
fn scale_by(scale: [f32; 2], factor: f32) -> [f32; 2] {
    let factor = factor.clamp(MIN_SCALE / scale[0].min(scale[1]), MAX_SCALE / scale[0].max(scale[1]));
    scale.map(|v| v * factor)
}

fn resized_scales(scale: [f32; 2], half_size: Vec2, direction: Vec2, delta: Vec2, keep: bool) -> [f32; 2] {
    if keep {
        let diagonal = half_size * vec2(scale[0], scale[1]) * direction;
        scale_by(scale, 1.0 + delta.dot(diagonal) / diagonal.length_sq().max(0.0001))
    } else {
        [
            (scale[0] + delta.x * direction.x / half_size.x.max(0.0001)).clamp(MIN_SCALE, MAX_SCALE),
            (scale[1] + delta.y * direction.y / half_size.y.max(0.0001)).clamp(MIN_SCALE, MAX_SCALE),
        ]
    }
}

fn interact_sheet(ui: &mut Ui, editor: &mut AlignEditor, sizes: [[f32; 2]; 2], k: f32, centre: Pos2) -> bool {
    let ctx = ui.ctx().clone();
    let view = ui.max_rect();
    let compared_size = vec2(sizes[1][0], sizes[1][1]);
    let top_centre = centre + vec2(editor.draft.offset[0] * sizes[0][0], editor.draft.offset[1] * sizes[0][1]) * k;
    let top_rect = Rect::from_center_size(top_centre, compared_size * vec2(editor.draft.scale[0], editor.draft.scale[1]) * k);
    let mut cancel = false;
    // Register the body first so the handles win hit testing where they overlap it.
    let body = ui.interact(top_rect.intersect(view), Id::new("align-sheet"), Sense::drag());
    let mut resized = false;
    let mut handle_hovered = false;
    for (corner, direction) in [vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0)].into_iter().enumerate() {
        let point = top_centre + compared_size * vec2(editor.draft.scale[0], editor.draft.scale[1]) * (k * 0.5) * direction;
        let handle = Rect::from_center_size(point, vec2(16.0, 16.0));
        let response = ui.interact(handle, Id::new(("align-corner", corner)), Sense::drag());
        if response.hovered() || response.dragged() {
            handle_hovered = true;
            ctx.set_cursor_icon(if corner % 2 == 0 { CursorIcon::ResizeNwSe } else { CursorIcon::ResizeNeSw });
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            editor.draft.scale = resized_scales(editor.draft.scale, compared_size * (k * 0.5), direction, response.drag_delta(), editor.keep_proportions);
            resized = true;
        }
    }
    if body.dragged_by(egui::PointerButton::Primary) && !resized {
        let delta = body.drag_delta() / k;
        editor.draft.offset[0] += delta.x / sizes[0][0];
        editor.draft.offset[1] += delta.y / sizes[0][1];
        ctx.set_cursor_icon(CursorIcon::Grabbing);
    } else if body.hovered() && !resized && !handle_hovered {
        ctx.set_cursor_icon(CursorIcon::Grab);
    }
    if !ctx.egui_wants_keyboard_input() {
        cancel = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let step = if ui.input(|i| i.modifiers.shift) { 10.0 } else { 1.0 };
        for (key, direction) in [(Key::ArrowLeft, vec2(-1.0, 0.0)), (Key::ArrowRight, vec2(1.0, 0.0)), (Key::ArrowUp, vec2(0.0, -1.0)), (Key::ArrowDown, vec2(0.0, 1.0))] {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key) || i.consume_key(Modifiers::SHIFT, key)) {
                editor.draft.offset[0] += direction.x * step / (k * sizes[0][0]);
                editor.draft.offset[1] += direction.y * step / (k * sizes[0][1]);
            }
        }
    }

    cancel
}

impl Compare {
    pub(super) fn alignment_view(&mut self, ui: &mut Ui, gpu: &Gpu, now: f64) {
        let Some(mut editor) = self.aligning.take() else { return };
        let ctx = ui.ctx().clone();
        let sizes = [0, 1].map(|side| self.sides[side].sizes.as_ref().and_then(|s| s.get(editor.pair[side])).copied().unwrap_or([612.0, 792.0]));
        let mut apply = false;
        let mut cancel = false;
        let mut inherit = false;
        egui::Panel::top("compare-alignment-bar")
            .frame(Frame::NONE.fill(SURFACE).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(if editor.all { "Align all sheets" } else { "Align page" }).strong().color(GREEN));
                    ui.label(format!("Original {} · Compared {}", editor.pair[0] + 1, editor.pair[1] + 1));
                    ui.separator();
                    ui.label("Green: Compared · drag to move · corner handles to resize");
                    cancel = ui.button("Cancel").clicked();
                    apply = styled_button(ui, if editor.all { "Apply to all" } else { "Apply to this page" }, Tone::Primary, false).clicked();
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut editor.keep_proportions, "Keep proportions")
                        .on_hover_text("Locks the current width-to-height ratio. Untick to stretch width and height independently.");
                    for (axis, label) in [(0, "Width"), (1, "Height")] {
                        ui.label(label);
                        let mut percent = editor.draft.scale[axis] * 100.0;
                        if ui.add(egui::DragValue::new(&mut percent).speed(0.1).range(MIN_SCALE * 100.0..=MAX_SCALE * 100.0).suffix("%")).changed() {
                            if editor.keep_proportions {
                                editor.draft.scale = scale_by(editor.draft.scale, percent / (100.0 * editor.draft.scale[axis]));
                            } else {
                                editor.draft.scale[axis] = percent / 100.0;
                            }
                        }
                    }
                    if ui.button("Centre sheets").clicked() {
                        editor.draft.offset = [0.0; 2];
                    }
                    if ui.button("Reset size and position").clicked() {
                        editor.draft = Alignment::default();
                    }
                    ui.separator();
                    if ui.button("−").clicked() { editor.zoom = (editor.zoom / 1.25).max(0.25); }
                    if ui.button("+").clicked() { editor.zoom = (editor.zoom * 1.25).min(12.0); }
                    if ui.button("Fit view").clicked() { editor.zoom = 1.0; editor.pan = Vec2::ZERO; }
                    ui.label("Ctrl+wheel: zoom · middle drag: pan · arrows: nudge · Esc: cancel");
                    if editor.all {
                        if !self.alignments.pages.is_empty() {
                            ui.label(format!("{} custom page alignment(s) will be kept", self.alignments.pages.len()));
                        }
                    } else if self.alignments.pages.contains_key(&editor.pair) {
                        inherit = ui.button("Use document alignment").clicked();
                    }
                });
            });

        let view = ui.available_rect_before_wrap();
        let fit = ((view.width() - 100.0).max(1.0) / sizes[0][0].max(sizes[1][0]))
            .min((view.height() - 100.0).max(1.0) / sizes[0][1].max(sizes[1][1]));
        let hover = ui.input(|i| i.pointer.hover_pos()).filter(|p| view.contains(*p));
        if let Some(pointer) = hover {
            let zoom = ui.input(|i| i.zoom_delta());
            if zoom != 1.0 {
                let before = editor.zoom;
                editor.zoom = (editor.zoom * zoom).clamp(0.25, 12.0);
                let ratio = editor.zoom / before;
                editor.pan = (editor.pan + view.center().to_vec2() - pointer.to_vec2()) * ratio
                    + pointer.to_vec2() - view.center().to_vec2();
            }
            if ui.input(|i| i.pointer.middle_down()) {
                editor.pan += ui.input(|i| i.pointer.delta());
                ctx.set_cursor_icon(CursorIcon::Grabbing);
            }
        }
        let k = fit * editor.zoom;
        let original_size = vec2(sizes[0][0], sizes[0][1]);
        let compared_size = vec2(sizes[1][0], sizes[1][1]);
        let centre = view.center() + editor.pan;
        let mut canvas = ui.new_child(egui::UiBuilder::new().max_rect(view));
        canvas.set_clip_rect(view.intersect(ui.clip_rect()));
        cancel |= interact_sheet(&mut canvas, &mut editor, sizes, k, centre);

        let painter = canvas.painter().clone();
        painter.rect_filled(view, CornerRadius::ZERO, BG);
        let original_rect = Rect::from_center_size(centre, original_size * k);
        let top_centre = centre + vec2(editor.draft.offset[0] * sizes[0][0], editor.draft.offset[1] * sizes[0][1]) * k;
        let top_rect = Rect::from_center_size(top_centre, compared_size * vec2(editor.draft.scale[0], editor.draft.scale[1]) * k);
        let mut budget = DRAW_PER_FRAME;
        let mut drawing = false;
        for (side, rect) in [(ORIGINAL, original_rect), (COMPARED, top_rect)] {
            drawing |= self.drawings.cell(gpu, &painter, Content::AlignmentSheet { side, page: editor.pair[side] }, rect, [sizes[side], [0.0; 2]],
                // The cached sheet is already stretched to this rectangle.
                rect.width() / sizes[side][0], ctx.pixels_per_point(), now, &mut budget);
        }
        painter.rect_stroke(top_rect, CornerRadius::ZERO, Stroke::new(2.0, GREEN), StrokeKind::Outside);
        for point in [top_rect.left_top(), top_rect.right_top(), top_rect.right_bottom(), top_rect.left_bottom()] {
            let handle = Rect::from_center_size(point, vec2(10.0, 10.0));
            painter.rect_filled(handle, CornerRadius::same(2), SURFACE);
            painter.rect_stroke(handle, CornerRadius::same(2), Stroke::new(2.0, GREEN), StrokeKind::Inside);
        }
        self.want(&[(ORIGINAL, editor.pair[0]), (COMPARED, editor.pair[1])], now);
        if drawing { ctx.request_repaint(); }
        ui.allocate_rect(view, Sense::hover());
        if apply || inherit {
            self.alignment_changed();
            if inherit {
                self.alignments.pages.remove(&editor.pair);
            } else if editor.all {
                self.alignments.document = Some(editor.draft);
            } else {
                self.alignments.pages.insert(editor.pair, editor.draft);
            }
            self.pan = None;
            ctx.request_repaint();
        } else if !cancel {
            self.aligning = Some(editor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> AlignEditor {
        AlignEditor { pair: [0, 0], all: false, draft: Alignment::default(), zoom: 1.0, pan: Vec2::ZERO, keep_proportions: true }
    }

    fn gesture(editor: &mut AlignEditor, from: Pos2, to: Pos2, button: egui::PointerButton) {
        let ctx = egui::Context::default();
        let frame = |editor: &mut AlignEditor, events| {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() };
            let mut output = ctx.run_ui(raw, |ui| {
                interact_sheet(ui, editor, [[200.0; 2]; 2], 1.0, pos2(400.0, 300.0));
            });
            output.textures_delta.clear();
        };
        frame(editor, vec![]);
        frame(editor, vec![egui::Event::PointerMoved(from)]);
        frame(editor, vec![egui::Event::PointerButton { pos: from, button, pressed: true, modifiers: Modifiers::NONE }]);
        frame(editor, vec![egui::Event::PointerMoved(to)]);
        frame(editor, vec![egui::Event::PointerButton { pos: to, button, pressed: false, modifiers: Modifiers::NONE }]);
    }

    #[test]
    fn dragging_a_corner_resizes_while_dragging_the_body_moves() {
        let mut resized = editor();
        gesture(&mut resized, pos2(500.0, 400.0), pos2(550.0, 450.0), egui::PointerButton::Primary);
        assert_eq!(resized.draft.scale, [1.5; 2]);
        assert_eq!(resized.draft.offset, [0.0; 2], "the corner must not drag the body");
        let mut moved = editor();
        gesture(&mut moved, pos2(400.0, 300.0), pos2(450.0, 330.0), egui::PointerButton::Primary);
        assert_eq!(moved.draft.scale, [1.0; 2]);
        assert_eq!(moved.draft.offset, [0.25, 0.15]);
        let mut middle = editor();
        gesture(&mut middle, pos2(500.0, 400.0), pos2(550.0, 450.0), egui::PointerButton::Middle);
        assert_eq!(middle.draft, Alignment::default(), "panning must not change the alignment");
    }

    #[test]
    fn corner_resizing_preserves_proportions_and_centre() {
        let half = vec2(200.0, 100.0);
        assert_eq!(resized_scales([1.0; 2], half, vec2(1.0, 1.0), half, true), [2.0; 2]);
        assert_eq!(resized_scales([1.0; 2], half, vec2(-1.0, -1.0), -half, true), [2.0; 2]);
        assert_eq!(resized_scales([1.0; 2], half, vec2(1.0, 1.0), vec2(-100.0, 200.0), true), [1.0; 2]);
        assert_eq!(resized_scales([1.0; 2], half, vec2(1.0, 1.0), -half * 10.0, true), [MIN_SCALE; 2]);
    }

    #[test]
    fn unlocked_corner_stretches_each_axis_and_relocking_keeps_the_new_ratio() {
        let mut stretched = editor();
        stretched.keep_proportions = false;
        gesture(&mut stretched, pos2(500.0, 400.0), pos2(550.0, 400.0), egui::PointerButton::Primary);
        assert_eq!(stretched.draft.scale, [1.5, 1.0]);
        assert_eq!(stretched.draft.offset, [0.0; 2]);
        stretched.keep_proportions = true;
        gesture(&mut stretched, pos2(550.0, 400.0), pos2(700.0, 500.0), egui::PointerButton::Primary);
        assert_eq!(stretched.draft.scale, [3.0, 2.0]);
        assert_eq!(stretched.draft.offset, [0.0; 2]);
    }
}
