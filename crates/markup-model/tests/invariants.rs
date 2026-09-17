//! Properties every quantity must keep however a shape is placed or drawn:
//! moving, turning or re-ordering an outline never changes what it measures.

use markup_model::geom::{self, Pt};
use markup_model::{quantities, Geometry, Markup, MarkupKind, PageTransform, Rect, Scale, ScaleId};
use proptest::prelude::*;

/// A simple polygon: points at increasing angles around a centre, each at its
/// own distance, which can't cross itself. No two neighbours are half a turn
/// or more apart, so the centre is inside and sees the whole outline.
fn star() -> impl Strategy<Value = Vec<Pt>> {
    (3usize..40).prop_flat_map(|n| {
        prop::collection::vec((0.2f64..1.0, 1.0f64..500.0), n).prop_map(move |spokes| {
            let n = spokes.len() as f64;
            spokes
                .iter()
                .enumerate()
                .map(|(i, &(jitter, radius))| {
                    let angle = (i as f64 + 0.5 + (jitter - 0.6) * 0.5) / n * std::f64::consts::TAU;
                    Pt::new(1000.0 + radius * angle.cos(), 1000.0 + radius * angle.sin())
                })
                .collect()
        })
    })
}

fn area_of(pts: Vec<Pt>, scale: &Scale) -> f64 {
    let m = Markup::new(0, MarkupKind::Area, Geometry::Polygon { pts, holes: vec![] });
    quantities(&m, Some(scale)).unwrap().area_m2.unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

proptest! {
    #[test]
    fn area_is_unchanged_by_moving_the_shape(pts in star(), dx in -1e4f64..1e4, dy in -1e4f64..1e4) {
        let scale = Scale::from_ratio(ScaleId(1), 200.0).unwrap();
        let moved = pts.iter().map(|&p| p + Pt::new(dx, dy)).collect();
        prop_assert!(close(area_of(pts, &scale), area_of(moved, &scale)));
    }

    #[test]
    fn area_is_unchanged_by_turning_the_shape_at_a_uniform_scale(pts in star(), degrees in 0f64..360.0) {
        let scale = Scale::from_ratio(ScaleId(1), 500.0).unwrap();
        let (sin, cos) = degrees.to_radians().sin_cos();
        let turned = pts.iter().map(|p| Pt::new(p.x * cos - p.y * sin, p.x * sin + p.y * cos)).collect();
        prop_assert!(close(area_of(pts, &scale), area_of(turned, &scale)));
    }

    #[test]
    fn area_is_unchanged_by_where_the_outline_starts_or_which_way_it_runs(pts in star(), start in 0usize..40) {
        let scale = Scale::from_ratio(ScaleId(1), 100.0).unwrap();
        let mut shifted = pts.clone();
        shifted.rotate_left(start % pts.len());
        let reversed = pts.iter().rev().copied().collect();
        let area = area_of(pts, &scale);
        prop_assert!(close(area, area_of(shifted, &scale)));
        prop_assert!(close(area, area_of(reversed, &scale)));
        prop_assert!(area > 0.0);
    }

    #[test]
    fn a_cutout_takes_exactly_its_own_area(pts in star(), shrink in 0.05f64..0.9) {
        let scale = Scale::from_ratio(ScaleId(1), 100.0).unwrap();
        // The same star shrunk towards its centre stays inside it.
        let hole: Vec<Pt> = pts.iter().map(|p| Pt::new(1000.0 + (p.x - 1000.0) * shrink, 1000.0 + (p.y - 1000.0) * shrink)).collect();
        let whole = area_of(pts.clone(), &scale);
        let m = Markup::new(0, MarkupKind::Area, Geometry::Polygon { pts, holes: vec![hole] });
        let with_hole = quantities(&m, Some(&scale)).unwrap().area_m2.unwrap();
        prop_assert!(close(with_hole, whole * (1.0 - shrink * shrink)));
    }

    #[test]
    fn a_polylength_is_the_same_both_ways_and_split_anywhere(pts in prop::collection::vec((-1e4f64..1e4, -1e4f64..1e4), 2..30), cut in 1usize..29) {
        let pts: Vec<Pt> = pts.into_iter().map(|(x, y)| Pt::new(x, y)).collect();
        let scale = Scale::from_ratio(ScaleId(1), 250.0).unwrap();
        let length = |pts: Vec<Pt>| {
            let m = Markup::new(0, MarkupKind::Polylength, Geometry::Polyline { pts });
            quantities(&m, Some(&scale)).unwrap().length_m.unwrap()
        };
        let whole = length(pts.clone());
        prop_assert!(close(whole, length(pts.iter().rev().copied().collect())));
        let cut = cut.min(pts.len() - 1);
        let rest = if cut + 1 < pts.len() { length(pts[cut..].to_vec()) } else { 0.0 };
        prop_assert!(close(whole, length(pts[..=cut].to_vec()) + rest));
    }

    #[test]
    fn screen_and_back_is_the_same_point(x in -2e4f64..2e4, y in -2e4f64..2e4, rotation in 0u8..4, zoom in 0.01f64..64.0) {
        let t = PageTransform { rotation, crop: Rect::from_corners(Pt::new(-20.0, 30.0), Pt::new(3390.0, 2414.0)), zoom, origin: Pt::new(-512.0, 77.0) };
        let p = Pt::new(x, y);
        prop_assert!(t.from_screen(t.to_screen(p)).dist(p) < 1e-6);
    }

    #[test]
    fn stars_are_simple(pts in star()) {
        prop_assert!(geom::is_simple(&pts));
    }
}

/// Segments scattered over a sheet, as a drawing's linework is.
fn linework() -> impl Strategy<Value = Vec<[Pt; 2]>> {
    prop::collection::vec((0f64..2384.0, 0f64..1684.0, -200f64..200.0, -200f64..200.0), 1..300).prop_map(|lines| {
        lines.into_iter().map(|(x, y, dx, dy)| [Pt::new(x, y), Pt::new(x + dx, y + dy)]).collect()
    })
}

proptest! {
    /// Whatever it catches -- an end, a crossing, a middle, a point along a
    /// line -- a snap never moves the point further than its reach. A snap
    /// that jumped further would drag a measurement's corner across the page.
    #[test]
    fn a_snap_stays_within_its_reach(lines in linework(), x in 0f64..2384.0, y in 0f64..1684.0, reach in 0.5f64..40.0) {
        let index = markup_model::SnapIndex::build(lines);
        let at = Pt::new(x, y);
        if let Some(snapped) = markup_model::snap::snap(at, reach, [], Some(&index)) {
            prop_assert!(snapped.point.dist(at) <= reach + 1e-9, "{:?} is {} away, reaching {reach}", snapped.kind, snapped.point.dist(at));
            prop_assert!(snapped.point.is_finite());
        }
    }

    /// A crossing is on both lines, so it can't be somewhere neither goes.
    #[test]
    fn a_crossing_lies_on_both_lines(lines in linework(), x in 0f64..2384.0, y in 0f64..1684.0) {
        let index = markup_model::SnapIndex::build(lines.clone());
        let at = Pt::new(x, y);
        let Some(snapped) = markup_model::snap::snap(at, 20.0, [], Some(&index)) else { return Ok(()) };
        if snapped.kind == markup_model::SnapKind::Intersection {
            // The index holds 32-bit coordinates, so a crossing can sit a
            // thousandth of a point off the line it was worked out from.
            let on = lines.iter().filter(|[a, b]| geom::distance_to_segment(snapped.point, *a, *b) < 0.01).count();
            prop_assert!(on >= 2, "a crossing of {on} lines at {:?}", snapped.point);
        }
    }
}
