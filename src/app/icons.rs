//! App icons - embedded Material Symbols and custom measurement/signature outlines.
//! SVG masks use pixel-aligned, native-resolution textures to avoid resampling blur.
//! See assets/icons/material/README.md for artwork attribution.
//!
//! PATH: src/app/icons.rs
//! CONSUMED BY: app rails, tool buttons and palette.

use super::*;
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Icon {
    Signature,
    Pin,
    Undo,
    Redo,
    Scale,
    Length,
    Polylength,
    Area,
    Cutout,
    Count,
    Angle,
    Radius,
    Diameter,
    Quantities,
    /// What the thing in hand is set to: sliders.
    Details,
    /// The tools kept by name: a tray of them.
    Tools,
    Edit,
    /// Finding words in the document: a magnifying glass.
    Find,
    Select,
    /// Copying a piece of the page: a dashed box with its copy behind it.
    Clip,
    /// Cutting a piece of the drawing out: scissors.
    Cut,
    /// Erasing part of the drawing: an eraser on its side, and what it has
    /// rubbed clear.
    Erase,
    /// Highlighting text: a marker over the band of colour it lays down.
    Highlighter,
    Pen,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    /// Typing on the page: a box with a T in it.
    TextBox,
    /// A box of text with an arrow out of it.
    Callout,
    /// The document's layers: a stack of three sheets.
    Layers,
    /// A layer shown, and one hidden: an eye, struck through.
    Eye,
    EyeOff,
    /// A layer locked, and one that can be changed.
    Lock,
    Unlock,
    /// A folded layer's arrow, and an open one's.
    Down,
    Right,
    /// Choosing a colour: a palette with its dabs of paint.
    Palette,
    Width(f32),
}

/// Choose an integer pixel size and origin together: rounding only the texture
/// size still resamples it when the destination has fractional pixel bounds.
fn raster_icon_rect(square: Rect, pixels_per_point: f32) -> (Rect, u32) {
    let pixels = (square.width() * pixels_per_point).round().max(1.0) as u32;
    let size = vec2(pixels as f32, pixels as f32);
    let origin = (square.center().to_vec2() * pixels_per_point - size * 0.5).round();
    let rect = Rect::from_min_size((origin / pixels_per_point).to_pos2(), size / pixels_per_point);
    (rect, pixels)
}

/// Draws a white SVG mask tinted with the button's current ink colour.
/// Rasterize and place at matching physical-pixel bounds at every display scale.
pub(super) fn paint(painter: &egui::Painter, box_: Rect, icon: Icon, ink: Color32) {
    let side = box_.width().min(box_.height());
    let square = Rect::from_center_size(box_.center(), vec2(side, side));
    macro_rules! material {
        ($name:literal) => { Some(($name, include_bytes!(concat!("../../assets/icons/material/", $name, ".svg")).as_slice())) };
    }
    let asset = match icon {
        Icon::Pin => material!("location_on"),
        Icon::Undo => material!("undo"),
        Icon::Redo => material!("redo"),
        Icon::Scale => material!("architecture"),
        Icon::Length => material!("straighten"),
        Icon::Polylength => material!("polyline"),
        Icon::Area => material!("area_chart"),
        Icon::Cutout => material!("select_all"),
        Icon::Count => material!("tag"),
        Icon::Angle => material!("square_foot"),
        Icon::Quantities => material!("table_chart"),
        Icon::Details => material!("tune"),
        Icon::Tools => material!("construction"),
        Icon::Edit => material!("edit"),
        Icon::Find => material!("search"),
        Icon::Select => material!("arrow_selector_tool"),
        Icon::Clip => material!("content_copy"),
        Icon::Cut => material!("content_cut"),
        Icon::Erase => material!("ink_eraser"),
        Icon::Highlighter => material!("ink_highlighter"),
        Icon::Pen => material!("draw"),
        Icon::Rectangle => material!("rectangle"),
        Icon::Ellipse => material!("circle"),
        Icon::Line => material!("horizontal_rule"),
        Icon::Arrow => material!("north_east"),
        Icon::TextBox => material!("text_fields"),
        Icon::Callout => material!("add_comment"),
        Icon::Layers => material!("layers"),
        Icon::Eye => material!("visibility"),
        Icon::EyeOff => material!("visibility_off"),
        Icon::Lock => material!("lock"),
        Icon::Unlock => material!("lock_open"),
        Icon::Down => material!("expand_more"),
        Icon::Right => material!("chevron_right"),
        Icon::Palette => material!("palette"),
        Icon::Signature | Icon::Radius | Icon::Diameter | Icon::Width(_) => None,
    };
    if let Some((name, bytes)) = asset {
        let (destination, pixels) = raster_icon_rect(square, painter.ctx().pixels_per_point());
        let id = Id::unique(("material-symbol", name, pixels));
        let cached = painter.ctx().data_mut(|data| data.get_temp::<egui::TextureHandle>(id));
        let texture = cached.unwrap_or_else(|| {
            let mut image = egui_extras::image::load_svg_bytes_with_size(
                bytes, egui::load::SizeHint::Height(pixels), &Default::default(),
            ).expect("embedded Material Symbol must be valid SVG");
            for pixel in &mut image.pixels {
                *pixel = Color32::from_white_alpha(pixel.a());
            }
            let texture = painter.ctx().load_texture(name, image, egui::TextureOptions::LINEAR);
            painter.ctx().data_mut(|data| data.insert_temp(id, texture.clone()));
            texture
        });
        // Keep SVG edge antialiasing, but avoid scaling or shifting texels between pixels.
        painter.image(texture.id(), destination, UV_FULL, ink);
        return;
    }

    let at = |x: f32, y: f32| pos2(square.min.x + x * side, square.min.y + y * side);
    let stroke = Stroke::new((side * 0.06).clamp(1.0, 1.5), ink);
    let line = |a: (f32, f32), b: (f32, f32)| painter.line_segment([at(a.0, a.1), at(b.0, b.1)], stroke);
    match icon {
        Icon::Signature => {
            // Folded document plus seal/ribbons, distinct from the drawing pen.
            for (a, b) in [
                ((0.18, 0.10), (0.62, 0.10)), ((0.62, 0.10), (0.82, 0.30)),
                ((0.82, 0.30), (0.82, 0.57)), ((0.62, 0.10), (0.62, 0.30)),
                ((0.62, 0.30), (0.82, 0.30)), ((0.18, 0.10), (0.18, 0.78)),
                ((0.18, 0.78), (0.45, 0.78)), ((0.31, 0.40), (0.65, 0.40)),
                ((0.31, 0.53), (0.49, 0.53)), ((0.56, 0.81), (0.51, 0.97)),
                ((0.51, 0.97), (0.64, 0.91)), ((0.73, 0.81), (0.78, 0.97)),
                ((0.78, 0.97), (0.65, 0.91)),
            ] { line(a, b); }
            painter.circle_stroke(at(0.65, 0.70), side * 0.15, stroke);
        }
        Icon::Radius => {
            painter.circle_stroke(at(0.5, 0.5), side * 0.33, stroke);
            painter.circle_filled(at(0.5, 0.5), side * 0.05, ink);
            line((0.5, 0.5), (0.78, 0.28));
            line((0.78, 0.28), (0.63, 0.31));
            line((0.78, 0.28), (0.75, 0.43));
        }
        Icon::Diameter => {
            painter.circle_stroke(at(0.5, 0.5), side * 0.33, stroke);
            line((0.19, 0.5), (0.81, 0.5));
            line((0.19, 0.5), (0.31, 0.42));
            line((0.19, 0.5), (0.31, 0.58));
            line((0.81, 0.5), (0.69, 0.42));
            line((0.81, 0.5), (0.69, 0.58));
        }
        Icon::Width(points) => {
            let bar = Stroke::new((points * 1.6).clamp(1.5, 7.0), ink);
            painter.line_segment([at(0.16, 0.5), at(0.84, 0.5)], bar);
        }
        _ => unreachable!("standard icons use Material Symbols"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_icons_match_screen_pixels_at_fractional_scales_and_positions() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.5] {
            for side in [14.0, 17.0, 18.0, 22.0, 23.5] {
                let square = Rect::from_center_size(pos2(31.3, 47.7), vec2(side, side));
                let (rect, pixels) = raster_icon_rect(square, scale);
                for edge in [rect.min.x, rect.min.y, rect.max.x, rect.max.y] {
                    let physical = edge * scale;
                    assert!((physical - physical.round()).abs() < 0.0001);
                }
                assert!((rect.width() * scale - pixels as f32).abs() < 0.0001);
                assert!((rect.height() * scale - pixels as f32).abs() < 0.0001);
                let shift = (rect.center() - square.center()) * scale;
                assert!(shift.x.abs() <= 0.5001 && shift.y.abs() <= 0.5001);
                assert!((pixels as f32 - side * scale).abs() <= 0.5);
            }
        }
    }
}
