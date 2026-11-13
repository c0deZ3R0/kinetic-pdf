//! Revision-bound, multi-page UNSEALED signature drafts and exclusive pointer input.
//!
//! PATH: src/app/signatures/placement.rs
//! CONSUMED BY: signatures panel and pages' current-frame geometry/input path.
//! Never changes PDF/session/dirty state. Coordinates are unrotated PDF user
//! space; current PageGeometry supplies crop/rotation. Only certification seals.

use super::*;
use crate::signing::SignatureAppearance;

#[derive(Clone)]
struct Preview { binding: Binding, appearance: SignatureAppearance }
struct DragBox { page: usize, sheet: usize, geometry: PageGeometry, start: (f32, f32), end: (f32, f32) }

#[derive(Default)]
pub(in crate::app) struct Placement {
    binding: Option<Binding>,
    armed: bool,
    armed_final: bool,
    final_pending: bool,
    remove_final_on_cancel: bool,
    drag: Option<DragBox>,
    previews: Vec<Preview>,
    completed: bool,
    pub notice: Option<String>,
    texture: Option<TextureHandle>,
}
impl Placement {
    pub fn active(&self) -> bool { self.armed }
    pub fn final_pending(&self) -> bool { self.final_pending }
    pub fn count(&self) -> usize { self.previews.len() }
    pub fn clear(&mut self) {
        self.cancel_current(); self.binding = None; self.previews.clear();
    }
    pub fn cancel_current(&mut self) {
        self.armed = false; self.drag = None; self.completed = false; self.notice = None;
        if self.final_pending && self.remove_final_on_cancel { self.previews.pop(); }
        self.final_pending = false; self.remove_final_on_cancel = false;
    }
    pub fn certify_existing(&mut self, binding: Binding) -> bool {
        self.cancel_current();
        if self.binding.is_some_and(|selected| selected != binding) { return false; }
        self.binding = Some(binding);
        self.final_pending = true; // Promote in place, or certify invisibly; cancellation deletes neither.
        true
    }
    pub fn certification_failed(&mut self) {
        self.armed = false; self.drag = None; self.completed = false;
        self.final_pending = false; self.remove_final_on_cancel = false;
    }
    pub fn undo_last(&mut self) {
        let removed_final = self.final_pending && self.remove_final_on_cancel; self.cancel_current();
        if !removed_final { self.previews.pop(); }
    }
    pub fn arm(&mut self, binding: Binding, final_box: bool) {
        if self.binding != Some(binding) { self.clear(); } else { self.cancel_current(); }
        self.binding = Some(binding); self.armed_final = final_box;
        // Reserve the last of the backend's 256 slots for the certification widget.
        self.armed = self.previews.len() < if final_box { 256 } else { 255 };
    }
    pub(super) fn finish_box(&mut self, binding: Binding, appearance: SignatureAppearance) {
        if !self.armed || self.binding != Some(binding) || self.previews.len() >= 256 { return; }
        self.previews.push(Preview { binding, appearance }); self.final_pending = self.armed_final;
        self.remove_final_on_cancel = self.armed_final;
        self.armed = false; self.completed = true; self.notice = None;
    }
    pub fn appearances(&self, binding: Option<Binding>) -> Vec<SignatureAppearance> {
        self.previews.iter().filter(|p| Some(p.binding) == binding).map(|p| p.appearance.clone()).collect()
    }
    pub fn appearance(&self, binding: Option<Binding>) -> Option<SignatureAppearance> {
        self.previews.last().filter(|p| Some(p.binding) == binding).map(|p| p.appearance.clone())
    }
    pub fn take_completed(&mut self) -> bool { std::mem::take(&mut self.completed) }
    pub fn finish_frame(&mut self, ui: &Ui) {
        // A release must be handled on its current page, never on stale screen rects.
        if !ui.input(|i| i.pointer.primary_down()) { self.drag = None; }
    }
    pub fn page(&mut self, ui: &Ui, binding: Binding, page: usize, sheet: usize, rect: Rect, geometry: PageGeometry, enabled: bool, caption: &str) {
        if self.binding != Some(binding) || geometry.bounds.width() <= 0.0 || geometry.bounds.height() <= 0.0 { return; }
        if self.armed && enabled && rect.is_positive() {
            let response = ui.interact(rect, Id::unique(("signature-box", binding.generation, binding.file, sheet)), Sense::click_and_drag())
                .on_hover_cursor(CursorIcon::Crosshair);
            if ui.input(|i| i.pointer.primary_pressed()) && response.is_pointer_button_down_on() {
                if let Some(pos) = response.interact_pointer_pos().filter(|pos| rect.contains(*pos)) {
                    let start = pdf_point(rect, geometry, pos);
                    self.drag = Some(DragBox { page, sheet, geometry, start, end: start });
                }
            }
            let mut finished = None;
            if let Some(drag) = &mut self.drag {
                if drag.page == page && drag.sheet == sheet {
                    if drag.geometry != geometry { self.drag = None; return; }
                    if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) { drag.end = pdf_point(rect, geometry, pos); }
                    if ui.input(|i| i.pointer.primary_released()) {
                        finished = appearance_between(page, geometry, drag.start, drag.end);
                        if finished.is_none() { self.notice = Some("Redraw larger: minimum 96 × 40 points as displayed.".into()); }
                        self.drag = None;
                    }
                }
            }
            if let Some(appearance) = finished { self.finish_box(binding, appearance); }
        }
        let mut boxes: Vec<_> = self.previews.iter().filter(|p| p.appearance.page == page).map(|p| p.appearance.rect).collect();
        if let Some(drag) = self.drag.as_ref().filter(|d| d.page == page && d.sheet == sheet) { boxes.push(bounds(drag.start, drag.end)); }
        if boxes.is_empty() { return; }
        if self.texture.is_none() {
            if let Ok(image) = egui_extras::image::load_svg_bytes_with_size(
                include_bytes!("../../../assets/signature-watermark.svg"), egui::load::SizeHint::Height(128), &Default::default()) {
                self.texture = Some(ui.ctx().load_texture("signature-watermark-preview", image, egui::TextureOptions::LINEAR));
            }
        }
        let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
        for [left, bottom, right, top] in boxes {
            let (l, t, r, b) = geometry.box_to_view(&PdfBox { left, bottom, right, top });
            let area = Rect::from_min_max(rect.min + rect.size() * vec2(l, t), rect.min + rect.size() * vec2(r, b));
            if !area.is_positive() { continue; }
            painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgb(251, 252, 254));
            if let Some(texture) = &self.texture {
                let size = texture.size_vec2();
                let scale = (area.width() * 0.80 / size.x).min(area.height() * 0.85 / size.y);
                painter.image(texture.id(), Rect::from_center_size(area.center(), size * scale), UV_FULL, Color32::from_white_alpha(41));
            }
            let (w, h) = (right - left, top - bottom);
            let (display_w, display_h) = if geometry.rotation % 2 == 1 { (h, w) } else { (w, h) };
            let zoom = area.width() / display_w;
            let size = crate::signing::appearance::preferred_text_size(display_w, display_h) * zoom;
            let inner = area.shrink(4.0 * zoom);
            let text_painter = painter.with_clip_rect(painter.clip_rect().intersect(inner));
            let text = text_painter.layout(caption.to_owned(), FontId::proportional(size), Color32::from_rgb(26, 33, 41), inner.width().max(0.1));
            text_painter.galley(inner.min, text, Color32::from_rgb(26, 33, 41));
            painter.rect_stroke(area, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), StrokeKind::Inside);
            painter.text(pos2(area.left(), (area.top() - 2.0).max(rect.top() + 12.0)), Align2::LEFT_BOTTOM,
                "DRAFT / UNSEALED", FontId::proportional(10.0), ACCENT);
        }
    }
}
fn pdf_point(rect: Rect, geometry: PageGeometry, pos: Pos2) -> (f32, f32) {
    let fraction = (pos - rect.min) / rect.size();
    geometry.from_view(fraction.x.clamp(0.0, 1.0), fraction.y.clamp(0.0, 1.0))
}
fn bounds(a: (f32, f32), b: (f32, f32)) -> [f32; 4] { [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)] }
fn appearance_between(page: usize, geometry: PageGeometry, a: (f32, f32), b: (f32, f32)) -> Option<SignatureAppearance> {
    let rect = bounds(a, b);
    if rect.iter().any(|v| !v.is_finite()) { return None; }
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let (display_w, display_h) = if geometry.rotation % 2 == 1 { (h, w) } else { (w, h) };
    if display_w < 96.0 || display_h < 40.0 { return None; }
    Some(SignatureAppearance { page, rect, rotation: u16::from(geometry.rotation % 4) * 90 })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn block(page: usize) -> SignatureAppearance { SignatureAppearance { page, rect: [10.0, 20.0, 210.0, 100.0], rotation: 0 } }
    #[test]
    fn drafts_append_across_pages_and_cancel_final_preserves_prior_drafts() {
        let binding = Binding { generation: 2, file: 9 }; let mut p = Placement::default();
        for page in [0, 3, 1] { p.arm(binding, false); p.finish_box(binding, block(page)); assert!(!p.final_pending()); }
        assert_eq!(p.appearances(Some(binding)).iter().map(|b| b.page).collect::<Vec<_>>(), vec![0, 3, 1]);
        p.arm(binding, true); p.finish_box(binding, block(4)); assert!(p.final_pending());
        p.cancel_current(); assert_eq!(p.count(), 3); assert!(!p.final_pending());
        p.arm(binding, false); p.cancel_current(); assert_eq!(p.count(), 3);
        p.undo_last(); assert_eq!(p.count(), 2);
        assert!(p.appearances(Some(Binding { generation: 2, file: 10 })).is_empty());
        p.arm(Binding { generation: 3, file: 10 }, false); assert_eq!(p.count(), 0);
    }
    #[test]
    fn certifying_existing_promotes_in_place_and_cancel_or_failure_keeps_all_drafts() {
        let binding = Binding { generation: 2, file: 9 }; let mut p = Placement::default();
        assert!(p.certify_existing(binding)); p.cancel_current(); assert_eq!(p.count(), 0);
        for page in [0, 2] { p.arm(binding, false); p.finish_box(binding, block(page)); }
        assert!(p.certify_existing(binding) && p.final_pending()); assert_eq!(p.count(), 2);
        p.cancel_current(); assert_eq!(p.count(), 2); assert!(!p.final_pending());
        p.arm(binding, true); p.finish_box(binding, block(3)); p.certification_failed();
        assert_eq!(p.count(), 3); assert!(!p.final_pending());
        assert!(p.certify_existing(binding)); assert_eq!(p.appearances(Some(binding)).last().unwrap().page, 3);
    }
    #[test]
    fn reserves_the_256th_box_for_final_certification() {
        let binding = Binding { generation: 2, file: 9 }; let mut p = Placement::default();
        for page in 0..255 { p.arm(binding, false); p.finish_box(binding, block(page)); }
        p.arm(binding, false); assert!(!p.active());
        p.arm(binding, true); p.finish_box(binding, block(255)); assert_eq!(p.count(), 256);
    }
    #[test]
    fn minimum_box_uses_displayed_dimensions_on_rotated_pages() {
        for rotation in 0..4 {
            let geometry = PageGeometry { rotation, bounds: PdfBox { left: 0.0, bottom: 0.0, right: 600.0, top: 600.0 } };
            let a = geometry.from_view(0.0, 0.0);
            assert!(appearance_between(0, geometry, a, geometry.from_view(95.0 / 600.0, 40.0 / 600.0)).is_none());
            assert!(appearance_between(0, geometry, a, geometry.from_view(96.0 / 600.0, 39.0 / 600.0)).is_none());
            let accepted = appearance_between(0, geometry, a, geometry.from_view(100.0 / 600.0, 45.0 / 600.0)).unwrap();
            assert_eq!(accepted.rotation, u16::from(rotation) * 90);
        }
    }
    #[test]
    fn cropped_rotated_boxes_remain_unrotated_pdf_coordinates() {
        let display = Rect::from_min_size(pos2(40.0, 80.0), vec2(600.0, 400.0));
        for rotation in 0..4 {
            let geometry = PageGeometry { rotation, bounds: PdfBox { left: 20.0, bottom: 30.0, right: 220.0, top: 330.0 } };
            let a = pdf_point(display, geometry, display.min + display.size() * vec2(0.2, 0.25));
            let b = pdf_point(display, geometry, display.min + display.size() * vec2(0.7, 0.8));
            let appearance = appearance_between(3, geometry, a, b).unwrap();
            assert_eq!(appearance.page, 3); assert_eq!(appearance.rotation, u16::from(rotation) * 90);
            let [left, bottom, right, top] = appearance.rect;
            let (l, t, r, bot) = geometry.box_to_view(&PdfBox { left, bottom, right, top });
            for (actual, expected) in [(l, 0.2), (t, 0.25), (r, 0.7), (bot, 0.8)] { assert!((actual - expected).abs() < 0.0001); }
        }
    }
}
