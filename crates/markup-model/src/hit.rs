//! Picking part of a markup: a vertex to drag, an edge's midpoint to insert a
//! vertex at, an edge, or the inside of a filled shape.
//!
//! Works in user space with a tolerance in points; the caller turns its pixel
//! radius into points with the page transform, so picking feels the same at
//! any zoom.

use serde::{Deserialize, Serialize};

use crate::geom::{self, Pt, Rect};
use crate::markup::Geometry;

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

    #[test]
    fn an_ellipse_by_its_outline_and_inside() {
        let g = Geometry::Ellipse { rect: Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(200.0, 100.0)) };
        assert_eq!(hit_test(&g, Pt::new(199.0, 50.0), 3.0), Some(Hit::Edge { ring: 0, index: 0 }));
        assert_eq!(hit_test(&g, Pt::new(100.0, 50.0), 3.0), Some(Hit::Inside));
        assert_eq!(hit_test(&g, Pt::new(5.0, 5.0), 3.0), None, "the box's corner is outside the ellipse");
    }
}
