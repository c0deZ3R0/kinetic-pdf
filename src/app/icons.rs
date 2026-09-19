//! The pictures on the tool buttons.
//!
//! Drawn as lines and circles rather than set from an icon font: what the
//! measurement tools mean -- a length against a polylength, a radius against a
//! diameter -- has no glyph in any icon set, so they would have been drawn by
//! hand whatever the rest came from. Nothing is embedded and nothing is
//! licensed; the whole set is a few dozen line segments.
//!
//! Each is drawn inside a square box in unit coordinates, (0, 0) its top-left
//! and (1, 1) its bottom-right, so one icon is the same weight as the next
//! whatever size the button is.

use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Icon {
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
    Select,
    Pen,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Width(f32),
}

/// Draws `icon` centred in `box_`, in `ink`.
pub(super) fn paint(painter: &egui::Painter, box_: Rect, icon: Icon, ink: Color32) {
    // A square inside the button, so a wide button still holds a round icon.
    let side = box_.width().min(box_.height());
    let square = Rect::from_center_size(box_.center(), vec2(side, side));
    let at = |x: f32, y: f32| pos2(square.min.x + x * side, square.min.y + y * side);
    let stroke = Stroke::new((side * 0.085).clamp(1.3, 2.0), ink);
    let line = |a: (f32, f32), b: (f32, f32)| painter.line_segment([at(a.0, a.1), at(b.0, b.1)], stroke);
    let path = |pts: &[(f32, f32)]| {
        painter.add(egui::Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), stroke));
    };

    match icon {
        // An arrow going back the way it came: over the top and down to the
        // left, with a plain barbed head where it lands. Mirrored for redo.
        Icon::Undo | Icon::Redo => {
            let flip = |x: f32| if icon == Icon::Undo { x } else { 1.0 - x };
            // Half a turn over the top, from the left round to the right.
            let arc: Vec<(f32, f32)> = (0..=16)
                .map(|i| {
                    let t = std::f32::consts::PI * (i as f32) / 16.0;
                    (flip(0.5 - 0.32 * t.cos()), 0.6 - 0.3 * t.sin())
                })
                .collect();
            path(&arc);
            // Down from where it started, and the head on that end.
            let tail = (flip(0.18), 0.82);
            line((flip(0.18), 0.6), tail);
            line(tail, (flip(0.05), 0.68));
            line(tail, (flip(0.31), 0.68));
        }
        // A dimension line over a graduated rule -- the pair of conventions
        // every drawing office already uses, and what icon sets settle on for
        // a measured length. The graduations hang inside the rule rather than
        // standing out of it, which is what made the last one look like a
        // crown.
        Icon::Scale => {
            let (left, right) = (0.08, 0.92);
            // The dimension line, ticked square at both ends.
            line((left, 0.25), (right, 0.25));
            line((left, 0.14), (left, 0.36));
            line((right, 0.14), (right, 0.36));
            // The rule below it.
            painter.rect_stroke(
                Rect::from_min_max(at(left, 0.52), at(right, 0.86)),
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            for i in 1..4 {
                let x = left + (right - left) * (i as f32) / 4.0;
                line((x, 0.52), (x, 0.66));
            }
        }
        // A dimension line: a run with a tick square across each end.
        Icon::Length => {
            line((0.15, 0.5), (0.85, 0.5));
            line((0.15, 0.32), (0.15, 0.68));
            line((0.85, 0.32), (0.85, 0.68));
        }
        // The same, bent: several runs end to end.
        Icon::Polylength => {
            path(&[(0.13, 0.66), (0.38, 0.34), (0.62, 0.62), (0.87, 0.3)]);
            line((0.08, 0.72), (0.18, 0.6));
            line((0.82, 0.36), (0.92, 0.24));
        }
        // A filled patch of ground.
        Icon::Area => {
            let pts = [(0.15, 0.68), (0.3, 0.26), (0.78, 0.22), (0.86, 0.72)];
            painter.add(egui::Shape::convex_polygon(
                pts.iter().map(|&(x, y)| at(x, y)).collect(),
                ink.gamma_multiply(0.22),
                stroke,
            ));
        }
        // An area with a hole taken out of it.
        Icon::Cutout => {
            let outer = [(0.13, 0.7), (0.24, 0.24), (0.8, 0.2), (0.88, 0.74)];
            painter.add(egui::Shape::convex_polygon(
                outer.iter().map(|&(x, y)| at(x, y)).collect(),
                ink.gamma_multiply(0.18),
                stroke,
            ));
            painter.rect_stroke(
                Rect::from_min_max(at(0.4, 0.4), at(0.66, 0.6)),
                CornerRadius::same(1),
                Stroke::new(stroke.width, ink),
                egui::StrokeKind::Inside,
            );
        }
        // Tally marks: what a count leaves on the page.
        Icon::Count => {
            for (cx, cy) in [(0.28, 0.32), (0.68, 0.34), (0.35, 0.7), (0.74, 0.68)] {
                painter.circle_filled(at(cx, cy), side * 0.075, ink);
            }
        }
        // Two arms from a corner, with the angle marked between them.
        Icon::Angle => {
            line((0.16, 0.78), (0.88, 0.78));
            line((0.16, 0.78), (0.8, 0.26));
            let arc: Vec<(f32, f32)> = (0..=8)
                .map(|i| {
                    let t = (39.0_f32).to_radians() * (i as f32) / 8.0;
                    (0.16 + 0.36 * t.cos(), 0.78 - 0.36 * t.sin())
                })
                .collect();
            path(&arc);
        }
        // A circle measured from the middle out. The line leaves the middle,
        // which is marked, and runs out at a slant: side by side with the
        // diameter, a level line through both would tell them apart only by
        // its length.
        Icon::Radius => {
            painter.circle_stroke(at(0.5, 0.5), side * 0.33, stroke);
            painter.circle_filled(at(0.5, 0.5), side * 0.06, ink);
            line((0.5, 0.5), (0.78, 0.28));
            line((0.78, 0.28), (0.63, 0.31));
            line((0.78, 0.28), (0.75, 0.43));
        }
        // And one measured across: right through, arrowed at both ends, with
        // no mark in the middle.
        Icon::Diameter => {
            painter.circle_stroke(at(0.5, 0.5), side * 0.33, stroke);
            line((0.19, 0.5), (0.81, 0.5));
            line((0.19, 0.5), (0.31, 0.42));
            line((0.19, 0.5), (0.31, 0.58));
            line((0.81, 0.5), (0.69, 0.42));
            line((0.81, 0.5), (0.69, 0.58));
        }
        // A table of rows.
        Icon::Quantities => {
            painter.rect_stroke(
                Rect::from_min_max(at(0.14, 0.2), at(0.86, 0.8)),
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            line((0.14, 0.4), (0.86, 0.4));
            line((0.14, 0.6), (0.86, 0.6));
            line((0.46, 0.4), (0.46, 0.8));
        }
        // A pointer.
        Icon::Select => {
            path(&[(0.32, 0.18), (0.32, 0.76), (0.46, 0.62), (0.58, 0.84), (0.68, 0.78), (0.56, 0.58), (0.74, 0.54), (0.32, 0.18)]);
        }
        // A nib drawing a stroke.
        Icon::Pen => {
            path(&[(0.2, 0.8), (0.28, 0.58), (0.68, 0.18), (0.82, 0.32), (0.42, 0.72), (0.2, 0.8)]);
            line((0.28, 0.58), (0.42, 0.72));
        }
        Icon::Rectangle => {
            painter.rect_stroke(
                Rect::from_min_max(at(0.16, 0.26), at(0.84, 0.74)),
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Ellipse => {
            painter.circle_stroke(at(0.5, 0.5), side * 0.33, stroke);
        }
        Icon::Line => {
            line((0.18, 0.8), (0.82, 0.2));
        }
        Icon::Arrow => {
            line((0.18, 0.82), (0.8, 0.2));
            line((0.8, 0.2), (0.52, 0.24));
            line((0.8, 0.2), (0.76, 0.48));
        }
        // How thick a new markup is drawn: the bar shows it.
        Icon::Width(points) => {
            let bar = Stroke::new((points * 1.6).clamp(1.5, 7.0), ink);
            painter.line_segment([at(0.16, 0.5), at(0.84, 0.5)], bar);
        }
    }
}
