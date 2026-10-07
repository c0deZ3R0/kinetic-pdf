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
        Icon::Pin => {
            painter.circle_stroke(at(0.5, 0.33), side * 0.23, stroke);
            painter.circle_filled(at(0.5, 0.33), side * 0.07, ink);
            path(&[(0.30, 0.46), (0.5, 0.91), (0.70, 0.46)]);
        }
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
        // Three sliders, each with its knob at a different place along it:
        // settings to be set.
        Icon::Details => {
            for (row, knob) in [(0.28, 0.66), (0.5, 0.36), (0.72, 0.58)] {
                line((0.16, row), (0.84, row));
                painter.circle_filled(at(knob, row), side * 0.08, ink);
            }
        }
        // A tray with tools standing in it, the way they are kept by name.
        Icon::Tools => {
            painter.rect_stroke(
                Rect::from_min_max(at(0.14, 0.46), at(0.86, 0.82)),
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            line((0.3, 0.46), (0.3, 0.2));
            line((0.5, 0.46), (0.5, 0.3));
            line((0.7, 0.46), (0.7, 0.24));
        }
        Icon::Edit => {
            // A pencil over a short line, legible at the list's small size.
            path(&[(0.2, 0.73), (0.27, 0.54), (0.69, 0.15), (0.84, 0.30), (0.42, 0.70), (0.2, 0.73)]);
            line((0.17, 0.86), (0.80, 0.86));
        }
        // A glass with its handle, the same one the find box carries.
        Icon::Find => {
            painter.circle_stroke(at(0.44, 0.42), side * 0.24, stroke);
            line((0.62, 0.6), (0.82, 0.8));
        }
        // A pointer.
        Icon::Select => {
            path(&[(0.32, 0.18), (0.32, 0.76), (0.46, 0.62), (0.58, 0.84), (0.68, 0.78), (0.56, 0.58), (0.74, 0.54), (0.32, 0.18)]);
        }
        // A marker leaning over the band of colour it has laid down.
        Icon::Highlighter => {
            path(&[(0.3, 0.6), (0.62, 0.18), (0.8, 0.32), (0.48, 0.74), (0.3, 0.6)]);
            path(&[(0.3, 0.6), (0.24, 0.72), (0.36, 0.78), (0.48, 0.74)]);
            let band = Stroke::new((side * 0.12).clamp(2.0, 3.5), ink.gamma_multiply(0.55));
            painter.line_segment([at(0.14, 0.88), at(0.86, 0.88)], band);
        }
        // The area picked out, dashed as the box is while it's dragged, with
        // the copy taken of it standing behind.
        Icon::Clip => {
            path(&[(0.42, 0.34), (0.42, 0.14), (0.88, 0.14), (0.88, 0.58), (0.68, 0.58)]);
            let corners = [(0.12, 0.38), (0.64, 0.38), (0.64, 0.86), (0.12, 0.86), (0.12, 0.38)];
            let ring: Vec<Pos2> = corners.iter().map(|&(x, y)| at(x, y)).collect();
            painter.extend(egui::Shape::dashed_line(&ring, stroke, side * 0.09, side * 0.06));
        }
        // Scissors, open: two finger loops and the blades crossing above them.
        Icon::Cut => {
            painter.circle_stroke(at(0.28, 0.76), side * 0.12, stroke);
            painter.circle_stroke(at(0.72, 0.76), side * 0.12, stroke);
            line((0.35, 0.66), (0.72, 0.12));
            line((0.65, 0.66), (0.28, 0.12));
        }
        // An eraser block leaning over, its end band marked, on the line it
        // has rubbed along.
        Icon::Erase => {
            path(&[(0.14, 0.6), (0.5, 0.24), (0.84, 0.58), (0.5, 0.92), (0.14, 0.6)]);
            line((0.32, 0.42), (0.67, 0.75));
            line((0.5, 0.92), (0.9, 0.92));
        }
        // A capital T in a box.
        Icon::TextBox => {
            painter.rect_stroke(Rect::from_min_max(at(0.12, 0.18), at(0.88, 0.82)), CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
            line((0.32, 0.34), (0.68, 0.34));
            line((0.5, 0.34), (0.5, 0.68));
        }
        // A smaller box with a T in it, up and to the right, and an arrow
        // down to the left out of it.
        Icon::Callout => {
            painter.rect_stroke(Rect::from_min_max(at(0.38, 0.12), at(0.9, 0.56)), CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
            line((0.52, 0.25), (0.76, 0.25));
            line((0.64, 0.25), (0.64, 0.45));
            line((0.38, 0.5), (0.14, 0.86));
            path(&[(0.14, 0.66), (0.14, 0.86), (0.33, 0.83)]);
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
        // Three sheets, one over the next, seen edge-on.
        Icon::Layers => {
            for (i, y) in [0.36f32, 0.5, 0.64].into_iter().enumerate() {
                let sheet = [(0.5, y - 0.22), (0.86, y - 0.04), (0.5, y + 0.14), (0.14, y - 0.04), (0.5, y - 0.22)];
                if i == 2 {
                    path(&sheet);
                } else {
                    path(&sheet[1..4]);
                }
            }
        }
        // An eye: the lids meeting at the corners, and the pupil.
        Icon::Eye | Icon::EyeOff => {
            let lid: Vec<(f32, f32)> = (0..=12).map(|i| {
                let x = 0.12 + 0.76 * (i as f32) / 12.0;
                let t = (i as f32) / 12.0 * 2.0 - 1.0;
                (x, 0.5 - 0.26 * (1.0 - t * t))
            }).collect();
            let under: Vec<(f32, f32)> = lid.iter().map(|&(x, y)| (x, 1.0 - y)).collect();
            path(&lid);
            path(&under);
            painter.circle_stroke(at(0.5, 0.5), side * 0.11, stroke);
            if icon == Icon::EyeOff {
                line((0.2, 0.84), (0.8, 0.16));
            }
        }
        // A padlock: the body, and the shackle over it, open on one side when
        // it isn't locked.
        Icon::Lock | Icon::Unlock => {
            path(&[(0.22, 0.48), (0.78, 0.48), (0.78, 0.86), (0.22, 0.86), (0.22, 0.48)]);
            let arc: Vec<(f32, f32)> = (0..=10).map(|i| {
                let t = std::f32::consts::PI * (i as f32) / 10.0;
                (0.5 - 0.17 * t.cos(), 0.3 - 0.18 * t.sin())
            }).collect();
            if icon == Icon::Lock {
                path(&arc);
                line((0.33, 0.3), (0.33, 0.48));
                line((0.67, 0.3), (0.67, 0.48));
            } else {
                path(&arc);
                line((0.33, 0.3), (0.33, 0.4));
                line((0.67, 0.3), (0.67, 0.2));
            }
        }
        Icon::Down => path(&[(0.24, 0.38), (0.5, 0.64), (0.76, 0.38)]),
        Icon::Right => path(&[(0.38, 0.24), (0.64, 0.5), (0.38, 0.76)]),
        Icon::Palette => {
            painter.circle_stroke(at(0.5, 0.5), side * 0.36, stroke);
            for (x, y) in [(0.36, 0.42), (0.52, 0.32), (0.66, 0.44), (0.4, 0.62)] {
                painter.circle_filled(at(x, y), side * 0.06, ink);
            }
        }
        // How thick a new markup is drawn: the bar shows it.
        Icon::Width(points) => {
            let bar = Stroke::new((points * 1.6).clamp(1.5, 7.0), ink);
            painter.line_segment([at(0.16, 0.5), at(0.84, 0.5)], bar);
        }
    }
}
