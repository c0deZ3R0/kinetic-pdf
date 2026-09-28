//! Turning and stretching markups: an affine map of user space, and what it
//! does to each kind of markup.
//!
//! Most markups are points, and every point is mapped. A text box and a clip
//! are boxes that have to stay boxes -- words and drawings aren't skewed --
//! so each keeps its shape: its middle goes where the map takes it, turned
//! as the map turns its bottom edge, and stretched along each side as much as
//! the map stretches that side (a clip the same both ways, keeping its
//! proportions). An ellipse is kept square to the page, which is all its
//! geometry can say: its middle is moved and its axes stretched.

use crate::geom::{Pt, Rect};
use crate::markup::{Geometry, Markup, MarkupKind};
use crate::text::Frame;

/// `x' = a x + c y + e`, `y' = b x + d y + f`, as PDF writes a matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [f64; 6]);

impl Affine {
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn apply(&self, p: Pt) -> Pt {
        let [a, b, c, d, e, f] = self.0;
        Pt::new(a * p.x + c * p.y + e, b * p.x + d * p.y + f)
    }

    /// A direction mapped: where it points, and how long it is, after.
    pub fn apply_vector(&self, v: Pt) -> Pt {
        let [a, b, c, d, _, _] = self.0;
        Pt::new(a * v.x + c * v.y, b * v.x + d * v.y)
    }

    /// This map, and then `next`.
    pub fn then(&self, next: &Affine) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let [p, q, r, s, t, u] = next.0;
        Affine([p * a + r * b, q * a + s * b, p * c + r * d, q * c + s * d, p * e + r * f + t, q * e + s * f + u])
    }

    pub fn translate(by: Pt) -> Affine {
        Affine([1.0, 0.0, 0.0, 1.0, by.x, by.y])
    }

    /// Turned `angle` radians anticlockwise, in user space, about `centre`.
    pub fn rotate_about(centre: Pt, angle: f64) -> Affine {
        let (sin, cos) = angle.sin_cos();
        let turn = Affine([cos, sin, -sin, cos, 0.0, 0.0]);
        Affine::translate(centre * -1.0).then(&turn).then(&Affine::translate(centre))
    }

    /// Stretched `sx` times along `frame`'s bottom edge and `sy` times up its
    /// side, `anchor` staying where it is.
    pub fn scale_along(frame: &Frame, anchor: Pt, sx: f64, sy: f64) -> Affine {
        let (u, v) = (frame.across, frame.up);
        // Into the frame's axes, stretched there, and back out: u and v are
        // at right angles, so back out is the transpose.
        let into = Affine([u.x, v.x, u.y, v.y, 0.0, 0.0]);
        let out = Affine([u.x, u.y, v.x, v.y, 0.0, 0.0]);
        let stretch = Affine([sx, 0.0, 0.0, sy, 0.0, 0.0]);
        Affine::translate(anchor * -1.0).then(&into).then(&stretch).then(&out).then(&Affine::translate(anchor))
    }
}

/// A box's `corners` -- its bottom left first, on round anticlockwise --
/// taken by `m` and kept a box, the right way round: see the module's notes.
/// `keep_shape` for one that keeps its proportions.
pub fn box_mapped(corners: &[Pt], m: &Affine, keep_shape: bool) -> Option<Vec<Pt>> {
    let frame = Frame::of(corners)?;
    let middle = frame.to_user(frame.width / 2.0, frame.height / 2.0);
    let (across, up) = (m.apply_vector(frame.across), m.apply_vector(frame.up));
    let (sx, sy) = (across.len(), up.len());
    if !(sx > 0.0 && sy > 0.0) {
        return None;
    }
    let (sx, sy) = if keep_shape { ((sx * sy).sqrt(), (sx * sy).sqrt()) } else { (sx, sy) };
    let along = across * (1.0 / across.len());
    // Up at right angles to the bottom, anticlockwise from it: never a
    // mirror, which would draw its words backwards.
    let up = Pt::new(-along.y, along.x);
    let (w, h) = (frame.width * sx, frame.height * sy);
    let origin = m.apply(middle) - along * (w / 2.0) - up * (h / 2.0);
    Some(vec![origin, origin + along * w, origin + along * w + up * h, origin + up * h])
}

impl Markup {
    /// This markup taken by `m`: turned, stretched or moved, a text box's
    /// arrow with it.
    pub fn transformed(&self, m: &Affine) -> Markup {
        let mut out = self.clone();
        match (self.kind, &mut out.geometry) {
            (MarkupKind::Clip | MarkupKind::Text, Geometry::Polygon { pts, .. }) => {
                if let Some(corners) = box_mapped(pts, m, self.kind == MarkupKind::Clip) {
                    *pts = corners;
                }
            }
            (_, Geometry::Ellipse { rect }) => {
                let middle = m.apply(rect.center());
                let rx = rect.width() / 2.0 * m.apply_vector(Pt::new(1.0, 0.0)).len();
                let ry = rect.height() / 2.0 * m.apply_vector(Pt::new(0.0, 1.0)).len();
                *rect = Rect::from_corners(middle - Pt::new(rx, ry), middle + Pt::new(rx, ry));
            }
            (_, geometry) => geometry.for_each_point_mut(|p| *p = m.apply(*p)),
        }
        if let Some(tip) = out.extras.text.as_mut().and_then(|t| t.callout.as_mut()) {
            *tip = m.apply(*tip);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::signed_area;
    use crate::text::{HAlign, RunFormat, TextBox};

    fn close(a: Pt, b: Pt) -> bool {
        a.dist(b) < 1e-9
    }

    #[test]
    fn a_turn_and_a_stretch_map_points_as_they_should() {
        let quarter = Affine::rotate_about(Pt::new(10.0, 10.0), std::f64::consts::FRAC_PI_2);
        assert!(close(quarter.apply(Pt::new(20.0, 10.0)), Pt::new(10.0, 20.0)), "anticlockwise about the centre");
        let frame = Frame::of(&[Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(10.0, 5.0), Pt::new(0.0, 5.0)]).unwrap();
        let wider = Affine::scale_along(&frame, Pt::new(0.0, 0.0), 2.0, 1.0);
        assert!(close(wider.apply(Pt::new(10.0, 5.0)), Pt::new(20.0, 5.0)));
        // Along a turned frame, stretched along its own axes.
        let turned = Frame { across: Pt::new(0.0, 1.0), up: Pt::new(-1.0, 0.0), ..frame };
        assert!(close(Affine::scale_along(&turned, Pt::new(0.0, 0.0), 3.0, 1.0).apply(Pt::new(0.0, 2.0)), Pt::new(0.0, 6.0)));
        assert!(close(quarter.then(&quarter).apply(Pt::new(20.0, 10.0)), Pt::new(0.0, 10.0)), "two quarters make a half");
    }

    #[test]
    fn a_measurement_turns_with_its_area_kept_and_boxes_stay_boxes() {
        let square = vec![Pt::new(0.0, 0.0), Pt::new(100.0, 0.0), Pt::new(100.0, 50.0), Pt::new(0.0, 50.0)];
        let turn = Affine::rotate_about(Pt::new(50.0, 25.0), 0.5);
        let area = Markup::new(0, MarkupKind::Area, Geometry::Polygon { pts: square.clone(), holes: vec![] });
        let Geometry::Polygon { pts, .. } = &area.transformed(&turn).geometry else { panic!() };
        assert!((signed_area(pts) - signed_area(&square)).abs() < 1e-6, "turned, the same area");

        // A text box turned stays a box, the same size, its arrow turned too.
        let mut text = Markup::new(0, MarkupKind::Text, Geometry::Polygon { pts: square.clone(), holes: vec![] });
        let mut words = TextBox::plain("Kerb", &RunFormat::default(), HAlign::Left);
        words.callout = Some(Pt::new(-40.0, 25.0));
        text.extras.text = Some(words);
        let turned = text.transformed(&turn);
        let Geometry::Polygon { pts, .. } = &turned.geometry else { panic!() };
        let frame = Frame::of(pts).unwrap();
        assert!((frame.width - 100.0).abs() < 1e-9 && (frame.height - 50.0).abs() < 1e-9);
        assert!((frame.across.y.atan2(frame.across.x) - 0.5).abs() < 1e-9, "turned as far as the map turns");
        assert!(close(turned.extras.text.as_ref().unwrap().callout.unwrap(), turn.apply(Pt::new(-40.0, 25.0))));

        // Stretched one way only across a turned box, it's still a box -- not
        // skewed -- and a clip keeps its proportions.
        let wide = Affine([2.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let Geometry::Polygon { pts, .. } = &turned.transformed(&wide).geometry.clone() else { panic!() };
        let f = Frame::of(pts).unwrap();
        assert!(f.across.dot(f.up).abs() < 1e-9, "square corners");
        let clip = Markup::new(0, MarkupKind::Clip, Geometry::Polygon { pts: square, holes: vec![] });
        let Geometry::Polygon { pts, .. } = &clip.transformed(&wide).geometry else { panic!() };
        let f = Frame::of(pts).unwrap();
        assert!((f.width / f.height - 2.0).abs() < 1e-9, "a clip keeps its proportions");
    }
}
