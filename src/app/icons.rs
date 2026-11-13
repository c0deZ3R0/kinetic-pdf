//! Material Symbols Outlined, embedded locally. See assets/icons/material/README.md.
//! Radius, diameter and stroke width retain their precise measurement meanings.

use super::*;
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Icon {
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

/// Draws a white SVG mask tinted with the button's current ink colour.
/// Cache at physical-pixel size so small icons stay sharp at each display scale.
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
        Icon::Radius | Icon::Diameter | Icon::Width(_) => None,
    };
    if let Some((name, bytes)) = asset {
        let pixels = (side * painter.ctx().pixels_per_point()).ceil().max(1.0) as u32;
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
        painter.image(texture.id(), square, UV_FULL, ink);
        return;
    }

    let at = |x: f32, y: f32| pos2(square.min.x + x * side, square.min.y + y * side);
    let stroke = Stroke::new((side * 0.06).clamp(1.0, 1.5), ink);
    let line = |a: (f32, f32), b: (f32, f32)| painter.line_segment([at(a.0, a.1), at(b.0, b.1)], stroke);
    match icon {
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
