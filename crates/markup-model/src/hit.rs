//! Picking part of a markup: a vertex to drag, an edge's midpoint to insert a
//! vertex at, an edge, or the inside of a filled shape.
//!
//! Works in user space with a tolerance in points; the caller turns its pixel
//! radius into points with the page transform, so picking feels the same at
//! any zoom.

use serde::{Deserialize, Serialize};

use crate::geom::{self, Pt, Rect};
use crate::markup::{Geometry, Markup, MarkupKind};

/// What a point is on. `ring` is 0 for a markup's outline, path or marks,
/// the cutout's number from 1 for a polygon's cutouts, and the stroke's
/// number for ink. `index` is the vertex, or the edge starting at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hit {
    Vertex { ring: usize, index: usize },
    Midpoint { ring: usize, index: usize },
    Edge { ring: usize, index: usize },
    /// Inside a polygon (not in a cutout) or an ellipse: the body, to move.
    Inside,
}

/// The best hit on `geometry` within `tolerance` of `p`, preferring vertices,
/// then midpoints, then edges, then the inside; the nearest of each.
pub fn hit_test(geometry: &Geometry, p: Pt, tolerance: f64) -> Option<Hit> {
    if !geometry.bounds().is_some_and(|b| b.expand(tolerance).contains(p)) {
        return None;
    }
    if let Geometry::Ellipse { rect } = geometry {
        return hit_ellipse(rect, p, tolerance);
    }
    let rings = geometry.rings();
    let closed = matches!(geometry, Geometry::Polygon { .. });
    let has_edges = !matches!(geometry, Geometry::Points { .. });
    // A line is stored as two one-point rings; treat it as one path.
    let line;
    let rings: Vec<&[Pt]> = match geometry {
        Geometry::Line { a, b } => {
            line = [*a, *b];
            vec![&line]
        }
        _ => rings,
    };

    let nearest = |candidates: &mut dyn Iterator<Item = (f64, usize, usize)>| {
        candidates.filter(|(d, ..)| *d <= tolerance).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, ring, index)| (ring, index))
    };
    let vertices = &mut rings.iter().enumerate().flat_map(|(r, pts)| pts.iter().enumerate().map(move |(i, v)| (v.dist(p), r, i)));
    if let Some((ring, index)) = nearest(vertices) {
        return Some(Hit::Vertex { ring, index });
    }
    if !has_edges {
        return None;
    }
    fn edges(r: usize, pts: &[Pt], closed: bool) -> impl Iterator<Item = (usize, usize, Pt, Pt)> + '_ {
        let count = if closed && pts.len() > 2 { pts.len() } else { pts.len().saturating_sub(1) };
        (0..count).map(move |i| (r, i, pts[i], pts[(i + 1) % pts.len()]))
    }
    let midpoints = &mut rings.iter().enumerate().flat_map(|(r, pts)| edges(r, pts, closed)).map(|(r, i, a, b)| (a.midpoint(b).dist(p), r, i));
    if let Some((ring, index)) = nearest(midpoints) {
        return Some(Hit::Midpoint { ring, index });
    }
    let on_edges = &mut rings.iter().enumerate().flat_map(|(r, pts)| edges(r, pts, closed)).map(|(r, i, a, b)| (geom::distance_to_segment(p, a, b), r, i));
    if let Some((ring, index)) = nearest(on_edges) {
        return Some(Hit::Edge { ring, index });
    }
    if let Geometry::Polygon { pts, holes } = geometry {
        if geom::point_in_ring(p, pts) && !holes.iter().any(|h| geom::point_in_ring(p, h)) {
            return Some(Hit::Inside);
        }
    }
    None
}

/// `hit_test` for a markup, which knows more than its geometry: a box or an
/// ellipse drawn on the page is picked by its outline, and by its inside only
/// where it is filled -- an empty frame round part of a drawing mustn't take
/// every click on what's in it -- its corners resize it and no corner is added
/// in the middle of an edge, and a pen stroke is picked only along the stroke.
pub fn hit_test_markup(m: &Markup, p: Pt, tolerance: f64) -> Option<Hit> {
    let filled = m.style.fill.is_some();
    match (m.kind, &m.geometry) {
        (MarkupKind::Ellipse, Geometry::Polygon { pts, .. }) if pts.len() == 4 => {
            if !m.geometry.bounds().is_some_and(|b| b.expand(tolerance).contains(p)) {
                return None;
            }
            if let Some(index) = pts.iter().position(|corner| corner.dist(p) <= tolerance) {
                return Some(Hit::Vertex { ring: 0, index });
            }
            let (away, inside) = geom::oval_distance(pts, p)?;
            if away <= tolerance {
                Some(Hit::Edge { ring: 0, index: 0 })
            } else {
                (inside && filled).then_some(Hit::Inside)
            }
        }
        (MarkupKind::Box, Geometry::Polygon { .. }) => match hit_test(&m.geometry, p, tolerance)? {
            Hit::Midpoint { ring, index } => Some(Hit::Edge { ring, index }),
            Hit::Inside if !filled => None,
            other => Some(other),
        },
        (MarkupKind::Pen, Geometry::Ink { strokes }) => {
            if !m.geometry.bounds().is_some_and(|b| b.expand(tolerance).contains(p)) {
                return None;
            }
            strokes.iter().enumerate().find_map(|(ring, stroke)| {
                let near = |(index, pair): (usize, &[Pt])| (geom::distance_to_segment(p, pair[0], pair[1]) <= tolerance).then_some(Hit::Edge { ring, index });
                match stroke.as_slice() {
                    [only] => (only.dist(p) <= tolerance).then_some(Hit::Edge { ring, index: 0 }),
                    _ => stroke.windows(2).enumerate().find_map(near),
                }
            })
        }
        _ => hit_test(&m.geometry, p, tolerance),
    }
}

fn hit_ellipse(rect: &Rect, p: Pt, tolerance: f64) -> Option<Hit> {
    let (c, rx, ry) = (rect.center(), rect.width() / 2.0, rect.height() / 2.0);
    if rx <= 0.0 || ry <= 0.0 {
        return None;
    }
    let d = p - c;
    // How far out the point is, 1 on the outline; times the smaller radius
    // it's close to a distance in points for any ellipse not too flat.
    let r = (d.x / rx).hypot(d.y / ry);
    if (r - 1.0).abs() * rx.min(ry) <= tolerance {
        Some(Hit::Edge { ring: 0, index: 0 })
    } else if r < 1.0 {
        Some(Hit::Inside)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Geometry {
        Geometry::Polygon {
            pts: vec![Pt::new(0.0, 0.0), Pt::new(100.0, 0.0), Pt::new(100.0, 100.0), Pt::new(0.0, 100.0)],
            holes: vec![vec![Pt::new(40.0, 40.0), Pt::new(60.0, 40.0), Pt::new(60.0, 60.0), Pt::new(40.0, 60.0)]],
        }
    }

    #[test]
    fn vertices_then_midpoints_then_edges_then_inside() {
        let g = square();
        assert_eq!(hit_test(&g, Pt::new(101.0, 99.0), 3.0), Some(Hit::Vertex { ring: 0, index: 2 }));
        assert_eq!(hit_test(&g, Pt::new(50.0, 2.0), 3.0), Some(Hit::Midpoint { ring: 0, index: 0 }));
        assert_eq!(hit_test(&g, Pt::new(1.0, 30.0), 3.0), Some(Hit::Edge { ring: 0, index: 3 }), "the closing edge");
        assert_eq!(hit_test(&g, Pt::new(20.0, 70.0), 3.0), Some(Hit::Inside));
        assert_eq!(hit_test(&g, Pt::new(50.0, 55.0), 3.0), None, "in the cutout");
        assert_eq!(hit_test(&g, Pt::new(59.0, 41.0), 3.0), Some(Hit::Vertex { ring: 1, index: 1 }), "the cutout's corner");
        assert_eq!(hit_test(&g, Pt::new(150.0, 50.0), 3.0), None);
    }

    #[test]
    fn an_open_path_has_no_closing_edge_or_inside() {
        let g = Geometry::Polyline { pts: vec![Pt::new(0.0, 0.0), Pt::new(100.0, 0.0), Pt::new(100.0, 100.0)] };
        assert_eq!(hit_test(&g, Pt::new(30.0, 29.0), 3.0), None);
        assert_eq!(hit_test(&g, Pt::new(40.0, 40.0), 3.0), None);
        let line = Geometry::Line { a: Pt::new(0.0, 0.0), b: Pt::new(100.0, 0.0) };
        assert_eq!(hit_test(&line, Pt::new(20.0, 1.0), 3.0), Some(Hit::Edge { ring: 0, index: 0 }));
        assert_eq!(hit_test(&line, Pt::new(100.0, 1.0), 3.0), Some(Hit::Vertex { ring: 0, index: 1 }));
    }

    #[test]
    fn a_count_is_picked_only_at_its_marks() {
        let g = Geometry::Points { pts: vec![Pt::new(0.0, 0.0), Pt::new(100.0, 0.0)] };
        assert_eq!(hit_test(&g, Pt::new(99.0, 1.0), 3.0), Some(Hit::Vertex { ring: 0, index: 1 }));
        assert_eq!(hit_test(&g, Pt::new(50.0, 0.0), 3.0), None);
    }


    fn drawn(kind: MarkupKind, geometry: Geometry) -> Markup {
        Markup::new(0, kind, geometry)
    }

    fn corners(w: f64, h: f64) -> Geometry {
        Geometry::Polygon { pts: vec![Pt::new(0.0, 0.0), Pt::new(w, 0.0), Pt::new(w, h), Pt::new(0.0, h)], holes: vec![] }
    }

    #[test]
    fn an_empty_frame_is_picked_by_its_edge_not_by_what_is_inside_it() {
        let mut frame = drawn(MarkupKind::Box, corners(100.0, 100.0));
        assert_eq!(hit_test_markup(&frame, Pt::new(50.0, 50.0), 3.0), None, "an empty frame lets clicks through");
        assert_eq!(hit_test_markup(&frame, Pt::new(50.0, 1.0), 3.0), Some(Hit::Edge { ring: 0, index: 0 }), "no corner is added mid-edge");
        assert_eq!(hit_test_markup(&frame, Pt::new(99.0, 99.0), 3.0), Some(Hit::Vertex { ring: 0, index: 2 }));
        frame.style.fill = Some([0.0, 0.0, 1.0]);
        assert_eq!(hit_test_markup(&frame, Pt::new(50.0, 50.0), 3.0), Some(Hit::Inside), "filled, the inside is the shape");
    }

    #[test]
    fn a_turned_ellipse_is_picked_along_its_own_outline() {
        // A 200 by 100 box turned a quarter, so it stands 100 wide and 200 tall.
        let turned = Geometry::Polygon { pts: vec![Pt::new(100.0, 0.0), Pt::new(100.0, 200.0), Pt::new(0.0, 200.0), Pt::new(0.0, 0.0)], holes: vec![] };
        let mut oval = drawn(MarkupKind::Ellipse, turned);
        assert_eq!(hit_test_markup(&oval, Pt::new(50.0, 199.0), 3.0), Some(Hit::Edge { ring: 0, index: 0 }), "the far end of the long axis");
        assert_eq!(hit_test_markup(&oval, Pt::new(99.0, 100.0), 3.0), Some(Hit::Edge { ring: 0, index: 0 }));
        assert_eq!(hit_test_markup(&oval, Pt::new(90.0, 190.0), 3.0), None, "the box's corner region is outside the oval");
        assert_eq!(hit_test_markup(&oval, Pt::new(50.0, 100.0), 3.0), None, "and unfilled its inside is not it");
        oval.style.fill = Some([1.0, 0.0, 0.0]);
        assert_eq!(hit_test_markup(&oval, Pt::new(50.0, 100.0), 3.0), Some(Hit::Inside));
        assert_eq!(hit_test_markup(&oval, Pt::new(100.0, 0.0), 3.0), Some(Hit::Vertex { ring: 0, index: 0 }), "a corner still resizes it");
    }

    #[test]
    fn a_pen_stroke_is_picked_only_along_the_stroke() {
        let pen = drawn(MarkupKind::Pen, Geometry::Ink { strokes: vec![vec![Pt::new(0.0, 0.0), Pt::new(50.0, 0.0), Pt::new(50.0, 50.0)]] });
        assert_eq!(hit_test_markup(&pen, Pt::new(25.0, 1.0), 3.0), Some(Hit::Edge { ring: 0, index: 0 }));
        assert_eq!(hit_test_markup(&pen, Pt::new(50.0, 50.0), 3.0), Some(Hit::Edge { ring: 0, index: 1 }), "no vertex to drag or add");
        assert_eq!(hit_test_markup(&pen, Pt::new(10.0, 40.0), 3.0), None, "off it");
    }

    #[test]
    fn an_oval_ring_lies_on_the_oval_and_an_arrow_head_points_back_along_the_line() {
        let ring = geom::oval_ring(&[Pt::new(0.0, 0.0), Pt::new(200.0, 0.0), Pt::new(200.0, 100.0), Pt::new(0.0, 100.0)], 64);
        assert_eq!(ring.len(), 64);
        assert!(ring.iter().all(|&p| geom::oval_distance(&[Pt::new(0.0, 0.0), Pt::new(200.0, 0.0), Pt::new(200.0, 100.0), Pt::new(0.0, 100.0)], p).unwrap().0 < 1e-9));
        let [left, right] = geom::arrow_head(Pt::new(0.0, 0.0), Pt::new(100.0, 0.0), 1.0);
        assert!(left.x < 100.0 && right.x < 100.0 && (left.y + right.y).abs() < 1e-9 && left.y != right.y);
    }
    #[test]
    fn an_ellipse_by_its_outline_and_inside() {
        let g = Geometry::Ellipse { rect: Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(200.0, 100.0)) };
        assert_eq!(hit_test(&g, Pt::new(199.0, 50.0), 3.0), Some(Hit::Edge { ring: 0, index: 0 }));
        assert_eq!(hit_test(&g, Pt::new(100.0, 50.0), 3.0), Some(Hit::Inside));
        assert_eq!(hit_test(&g, Pt::new(5.0, 5.0), 3.0), None, "the box's corner is outside the ellipse");
    }
}
