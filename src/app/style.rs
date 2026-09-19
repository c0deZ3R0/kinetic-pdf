//! The palette and the light theme.

use super::*;

// A light palette throughout.
pub(super) const BG: Color32 = Color32::from_rgb(0xe9, 0xeb, 0xee);
pub(super) const SURFACE: Color32 = Color32::WHITE;
pub(super) const BORDER: Color32 = Color32::from_rgb(0xe3, 0xe6, 0xea);
pub(super) const INPUT_BORDER: Color32 = Color32::from_rgb(0xd0, 0xd5, 0xdb);
pub(super) const HOVER_FILL: Color32 = Color32::from_rgb(0xf3, 0xf4, 0xf6);
pub(super) const PRESSED_FILL: Color32 = Color32::from_rgb(0xe5, 0xe7, 0xeb);
pub(super) const TEXT: Color32 = Color32::from_rgb(0x1f, 0x23, 0x28);
pub(super) const MUTED: Color32 = Color32::from_rgb(0x6b, 0x72, 0x80);
pub(super) const SUBTLE: Color32 = Color32::from_rgb(0xa9, 0xae, 0xb4);
pub(super) const ACCENT: Color32 = Color32::from_rgb(0x3b, 0x82, 0xf6);
pub(super) const ACCENT_HOVER: Color32 = Color32::from_rgb(0x2f, 0x6f, 0xd9);
pub(super) const ACCENT_PRESSED: Color32 = Color32::from_rgb(0x26, 0x5f, 0xc0);
pub(super) const ACCENT_SOFT: Color32 = Color32::from_rgb(0xe8, 0xf0, 0xfe);
pub(super) const ACCENT_SOFT_BORDER: Color32 = Color32::from_rgb(0xbf, 0xd5, 0xfb);
pub(super) const ACCENT_TEXT: Color32 = Color32::from_rgb(0x1d, 0x4e, 0xd8);
pub(super) const DANGER: Color32 = Color32::from_rgb(0xdc, 0x26, 0x26);
pub(super) const DANGER_SOFT: Color32 = Color32::from_rgb(0xfe, 0xf2, 0xf2);
pub(super) const DANGER_PRESSED: Color32 = Color32::from_rgb(0xfe, 0xe2, 0xe2);
pub(super) const DIRTY: Color32 = Color32::from_rgb(0xb4, 0x53, 0x09);
pub(super) const SAVED: Color32 = Color32::from_rgb(0x15, 0x80, 0x3d);
pub(super) const QUOTE_TEXT: Color32 = Color32::from_rgb(0x4b, 0x55, 0x63);
pub(super) const QUOTE_BG: Color32 = Color32::from_rgb(0xf6, 0xf8, 0xfa);
pub(super) const NOTE_ACTIVE: Color32 = Color32::from_rgb(0xea, 0xf2, 0xfe);
pub(super) const ROW_HOVER: Color32 = Color32::from_rgb(0xf6, 0xf8, 0xfa);
pub(super) const ROW_RULE: Color32 = Color32::from_rgb(0xee, 0xf0, 0xf2);
pub(super) const SELECTION: Color32 = Color32::from_rgba_premultiplied(0x1d, 0x41, 0x7b, 0x50);
// Search matches are orange, so they can't be mistaken for yellow highlights.
pub(super) const HIT: Color32 = Color32::from_rgb(0xff, 0xc9, 0x8a);
pub(super) const HIT_CURRENT: Color32 = Color32::from_rgb(0xff, 0x96, 0x3c);
pub(super) const HIT_OUTLINE: Color32 = Color32::from_rgb(0xe8, 0x6a, 0x00);

pub(super) const UV_FULL: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

pub(super) fn to_color32([r, g, b]: Rgb) -> Color32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(byte(r), byte(g), byte(b))
}

pub(super) fn soft_shadow() -> Shadow {
    Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(36) }
}

/// The light theme, applied once. Only what egui draws itself (text boxes,
/// scroll bars, separators, tooltips) comes from here; the app's buttons are
/// painted by `paint_button`.
pub(super) fn apply_light_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |style| {
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(10.0, 5.0);
        style.interaction.selectable_labels = false;

        let v = &mut style.visuals;
        v.panel_fill = SURFACE;
        v.window_fill = SURFACE;
        v.extreme_bg_color = SURFACE;
        v.text_edit_bg_color = Some(SURFACE);
        v.faint_bg_color = QUOTE_BG;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.window_shadow = soft_shadow();
        v.popup_shadow = soft_shadow();
        v.selection.bg_fill = Color32::from_rgb(0xcf, 0xe0, 0xfd);
        // Also the focus ring on text boxes.
        v.selection.stroke = Stroke::new(1.5, ACCENT);

        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        for (state, fill, border) in [
            (&mut w.inactive, SURFACE, INPUT_BORDER),
            (&mut w.hovered, HOVER_FILL, Color32::from_gray(0xb8)),
            (&mut w.active, PRESSED_FILL, ACCENT),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, border);
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.corner_radius = CornerRadius::same(6);
            state.expansion = 0.0;
        }
    });
}

/// A colour from the picker, back to the model's red, green and blue.
pub(super) fn from_color32(c: Color32) -> Rgb {
    [f32::from(c.r()) / 255.0, f32::from(c.g()) / 255.0, f32::from(c.b()) / 255.0]
}
