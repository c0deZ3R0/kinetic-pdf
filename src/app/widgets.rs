//! Buttons, swatches, quote cards and the other small pieces the panels are made of.

use super::*;

/// Every button in the app. Painted by hand rather than with egui's `Button`
/// so hover and press states can have their own colours per tone. Respects a
/// disabled parent `Ui`.
pub(super) fn paint_button(ui: &mut Ui, text: &str, font: FontId, tone: Tone, selected: bool, min_size: Vec2) -> egui::Response {
    paint_button_tall(ui, text, font, tone, selected, min_size, 30.0)
}

/// `paint_button` with the row height said out loud, for the slim bar above
/// the quantities, whose buttons are shorter than the toolbar's.
pub(super) fn paint_button_tall(
    ui: &mut Ui,
    text: &str,
    font: FontId,
    tone: Tone,
    selected: bool,
    min_size: Vec2,
    height: f32,
) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let slim = height < 28.0;
    let padding = match tone {
        _ if slim => 8.0,
        Tone::Ghost | Tone::Danger => 10.0,
        _ => 14.0,
    };
    let size = vec2(galley.size().x + 2.0 * padding, height).max(min_size);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let enabled = ui.is_enabled();
    let hovered = enabled && response.hovered();
    let pressed = enabled && response.is_pointer_button_down_on();
    let pick = |idle: Color32, hover: Color32, press: Color32| {
        if pressed {
            press
        } else if hovered {
            hover
        } else {
            idle
        }
    };
    let (mut fill, border, mut ink) = match tone {
        Tone::Primary => (pick(ACCENT, ACCENT_HOVER, ACCENT_PRESSED), Color32::TRANSPARENT, Color32::WHITE),
        Tone::Secondary if selected => (pick(ACCENT_SOFT, ACCENT_SOFT, ACCENT_SOFT_BORDER), ACCENT_SOFT_BORDER, ACCENT_TEXT),
        Tone::Secondary => (pick(SURFACE, HOVER_FILL, PRESSED_FILL), INPUT_BORDER, TEXT),
        Tone::Ghost => (pick(Color32::TRANSPARENT, HOVER_FILL, PRESSED_FILL), Color32::TRANSPARENT, MUTED),
        Tone::Danger => (pick(Color32::TRANSPARENT, DANGER_SOFT, DANGER_PRESSED), Color32::TRANSPARENT, DANGER),
    };
    if !enabled {
        if matches!(tone, Tone::Primary) {
            fill = fill.gamma_multiply(0.45);
        } else {
            ink = SUBTLE;
        }
    }

    let painter = ui.painter();
    let radius = CornerRadius::same(if slim { 6 } else { 8 });
    painter.rect(rect, radius, fill, Stroke::new(1.0, border), StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, ink);
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

pub(super) fn styled_button(ui: &mut Ui, text: &str, tone: Tone, selected: bool) -> egui::Response {
    paint_button(ui, text, FontId::proportional(13.5), tone, selected, Vec2::ZERO)
}

/// The status bar's button: the same tones as the toolbar's, drawn shorter
/// and in smaller type so the bar stays a thin strip.
pub(super) fn slim_button(ui: &mut Ui, text: &str, tone: Tone, selected: bool) -> egui::Response {
    paint_button_tall(ui, text, FontId::proportional(12.0), tone, selected, Vec2::ZERO, SLIM_HEIGHT)
}

/// A slim square, for the one-glyph zoom buttons.
pub(super) fn slim_icon_button(ui: &mut Ui, glyph: &str) -> egui::Response {
    let size = vec2(SLIM_HEIGHT, SLIM_HEIGHT);
    paint_button_tall(ui, glyph, FontId::monospace(13.0), Tone::Secondary, false, size, SLIM_HEIGHT)
}

/// How tall everything in the status bar is.
pub(super) const SLIM_HEIGHT: f32 = 22.0;

/// A square button holding one symbol, in monospace (see `search_box`).
pub(super) fn icon_button(ui: &mut Ui, glyph: &str) -> egui::Response {
    paint_button(ui, glyph, FontId::monospace(15.0), Tone::Secondary, false, vec2(30.0, 30.0))
}

/// A tool button showing what it does rather than saying it: the same square
/// as every other, with one of `icons::Icon` drawn inside. What it is called
/// belongs on the hover text, which every caller gives it.
pub(super) fn tool_button(ui: &mut Ui, icon: icons::Icon, tone: Tone, selected: bool) -> egui::Response {
    let size = vec2(32.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let enabled = ui.is_enabled();
    let hovered = enabled && response.hovered();
    let pressed = enabled && response.is_pointer_button_down_on();
    let pick = |idle: Color32, hover: Color32, press: Color32| {
        if pressed {
            press
        } else if hovered {
            hover
        } else {
            idle
        }
    };
    let (fill, border, mut ink) = match tone {
        Tone::Secondary if selected => (pick(ACCENT_SOFT, ACCENT_SOFT, ACCENT_SOFT_BORDER), ACCENT_SOFT_BORDER, ACCENT_TEXT),
        Tone::Ghost => (pick(Color32::TRANSPARENT, HOVER_FILL, PRESSED_FILL), Color32::TRANSPARENT, MUTED),
        _ => (pick(SURFACE, HOVER_FILL, PRESSED_FILL), INPUT_BORDER, TEXT),
    };
    if !enabled {
        ink = SUBTLE;
    }

    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(8), fill, Stroke::new(1.0, border), StrokeKind::Inside);
    icons::paint(painter, rect.shrink(7.0), icon, ink);
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

/// The label on an oversized page, in its top-right corner, which switches
/// between shrunk and actual size when clicked.
pub(super) fn size_badge(ui: &Ui, page: Rect, index: usize, label: &str) -> egui::Response {
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::proportional(12.0), Color32::PLACEHOLDER);
    let size = galley.size() + vec2(20.0, 10.0);
    let rect = Rect::from_min_size(pos2(page.max.x - size.x - 10.0, page.min.y + 10.0), size);
    let response = ui.interact(rect, Id::new(("size-badge", index)), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() { TEXT } else { TEXT.gamma_multiply(0.82) };
        painter.rect_filled(rect, CornerRadius::same(12), fill);
        painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

/// A magnifying glass inside the left margin of the find box.
pub(super) fn paint_magnifier(ui: &Ui, field: Rect) {
    let center = pos2(field.min.x + 14.0, field.center().y - 1.5);
    let stroke = Stroke::new(1.5, SUBTLE);
    ui.painter().circle_stroke(center, 4.5, stroke);
    ui.painter().line_segment([center + vec2(3.3, 3.3), center + vec2(6.5, 6.5)], stroke);
}

pub(super) fn swatch(ui: &mut Ui, rgb: Rgb, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let center = rect.center();
    let painter = ui.painter();
    if selected {
        painter.circle_stroke(center, 12.5, Stroke::new(2.0, ACCENT));
    } else if response.hovered() {
        painter.circle_stroke(center, 12.5, Stroke::new(1.5, INPUT_BORDER));
    }
    painter.circle_filled(center, 9.5, to_color32(rgb));
    painter.circle_stroke(center, 9.5, Stroke::new(1.0, Color32::from_black_alpha(28)));
    if selected {
        let tick = Stroke::new(1.8, TEXT);
        painter.line_segment([center + vec2(-4.0, 0.0), center + vec2(-1.2, 3.0)], tick);
        painter.line_segment([center + vec2(-1.2, 3.0), center + vec2(4.2, -3.2)], tick);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

/// The text being highlighted or annotated, on a tinted card with a bar in the
/// highlight's colour down its left edge.
pub(super) fn quote_card(ui: &mut Ui, text: &str, color: Color32) {
    let shown = if text.chars().count() > 180 {
        format!("{}…", text.chars().take(180).collect::<String>().trim_end())
    } else {
        text.to_owned()
    };
    let card = Frame::NONE
        .fill(QUOTE_BG)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin { left: 16, right: 10, top: 8, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(shown).size(13.0).color(QUOTE_TEXT));
        });
    let r = card.response.rect;
    let bar = Rect::from_min_max(pos2(r.min.x + 5.0, r.min.y + 7.0), pos2(r.min.x + 8.0, r.max.y - 7.0));
    ui.painter().rect_filled(bar, CornerRadius::same(2), color);
}

pub(super) fn quote_block(ui: &mut Ui, text: &str, max_chars: usize) {
    let shown = if text.chars().count() > max_chars {
        format!("{}…", text.chars().take(max_chars).collect::<String>().trim_end())
    } else {
        text.to_owned()
    };
    let inner = Frame::NONE
        .inner_margin(Margin { left: 10, right: 0, top: 0, bottom: 0 })
        .show(ui, |ui| ui.label(RichText::new(shown).size(12.0).color(QUOTE_TEXT)));
    let r = inner.response.rect;
    ui.painter().vline(r.left() + 1.5, r.y_range(), Stroke::new(3.0, QUOTE_BORDER));
}

/// A keyboard shortcut drawn as a key.
pub(super) fn keycap(ui: &mut Ui, text: &str) {
    Frame::NONE
        .fill(HOVER_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).color(QUOTE_TEXT));
        });
}

pub(super) fn empty_note(ui: &mut Ui, text: &str) {
    Frame::NONE.inner_margin(Margin::symmetric(14, 16)).show(ui, |ui| {
        ui.label(RichText::new(text).size(13.0).color(MUTED));
    });
}

pub(super) fn panel_heading(ui: &mut Ui, title: &str, detail: String) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(14.0).strong().color(TEXT));
        if !detail.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(detail).size(12.5).color(MUTED));
            });
        }
    });
}
