//! Points, boxes and the plane geometry quantities come from, all in `f64`.
//!
//! Everything here works in whatever units it's given. Stored geometry is in
//! PDF user space (points, origin at the bottom-left of the MediaBox, y up);
//! the scaled functions take metres per point along x and y separately, so a
//! drawing with an exaggerated vertical scale measures correctly.

use std::ops::{Add, Mul, Sub};

use serde::{Deserialize, Serialize};

/// A point, or a vector between two.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Pt {
        Pt { x, y }
    }

    pub fn dot(self, o: Pt) -> f64 {
        self.x * o.x + self.y * o.y
    }

    /// The z of the cross product: positive when `o` turns anticlockwise from
    /// `self`.
    pub fn cross(self, o: Pt) -> f64 {
        self.x * o.y - self.y * o.x
    }

    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }

    pub fn dist(self, o: Pt) -> f64 {
        (o - self).len()
    }

    pub fn midpoint(self, o: Pt) -> Pt {
        Pt::new((self.x + o.x) / 2.0, (self.y + o.y) / 2.0)
    }

    /// Scaled per axis: from points to metres, given metres per point.
    pub fn scaled(self, sx: f64, sy: f64) -> Pt {
        Pt::new(self.x * sx, self.y * sy)
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Add for Pt {
    type Output = Pt;
    fn add(self, o: Pt) -> Pt {
        Pt::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Pt {
    type Output = Pt;
    fn sub(self, o: Pt) -> Pt {
        Pt::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Pt {
    type Output = Pt;
    fn mul(self, k: f64) -> Pt {
        Pt::new(self.x * k, self.y * k)
    }
}

/// An axis-aligned box, always with `min` at or below and left of `max`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub min: Pt,
    pub max: Pt,
}

impl Rect {
    /// The box with corners `a` and `b`, in either order.
    pub fn from_corners(a: Pt, b: Pt) -> Rect {
        Rect { min: Pt::new(a.x.min(b.x), a.y.min(b.y)), max: Pt::new(a.x.max(b.x), a.y.max(b.y)) }
    }

    /// The smallest box holding every point, or `None` for no points.
    pub fn around<'a>(points: impl IntoIterator<Item = &'a Pt>) -> Option<Rect> {
        let mut points = points.into_iter();
        let first = *points.next()?;
        Some(points.fold(Rect::from_corners(first, first), |r, &p| r.union(Rect::from_corners(p, p))))
    }

    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }

    pub fn area(&self) -> f64 {
        self.width() * self.height()
    }

    pub fn center(&self) -> Pt {
        self.min.midpoint(self.max)
    }

    /// Whether `p` is inside or on the edge.
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// Whether `o` lies wholly inside or on the edge.
    pub fn contains_rect(&self, o: &Rect) -> bool {
        self.contains(o.min) && self.contains(o.max)
    }

    /// Whether the two boxes share any point, edges included.
    pub fn intersects(&self, o: &Rect) -> bool {
        self.min.x <= o.max.x && o.min.x <= self.max.x && self.min.y <= o.max.y && o.min.y <= self.max.y
    }

    pub fn union(&self, o: Rect) -> Rect {
        Rect {
            min: Pt::new(self.min.x.min(o.min.x), self.min.y.min(o.min.y)),
            max: Pt::new(self.max.x.max(o.max.x), self.max.y.max(o.max.y)),
        }
    }

    /// Grown by `d` on every side.
    pub fn expand(&self, d: f64) -> Rect {
        Rect { min: Pt::new(self.min.x - d, self.min.y - d), max: Pt::new(self.max.x + d, self.max.y + d) }
    }

    /// The four corners, anticlockwise from `min`.
    pub fn corners(&self) -> [Pt; 4] {
        [self.min, Pt::new(self.max.x, self.min.y), self.max, Pt::new(self.min.x, self.max.y)]
    }
}

/// The signed area of a closed ring by the shoelace formula: positive when
/// the points run anticlockwise (y up). The ring closes itself; don't repeat
/// the first point at the end, though doing so changes nothing.
pub fn signed_area(ring: &[Pt]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    // Relative to the first point, so a ring far from the origin doesn't lose
    // digits to cancellation.
    let origin = ring[0];
    let mut twice = 0.0;
    for w in ring[1..].windows(2) {
        twice += (w[0] - origin).cross(w[1] - origin);
    }
    twice / 2.0
}

/// The length of an open path through `points`, each axis scaled.
pub fn path_length(points: &[Pt], sx: f64, sy: f64) -> f64 {
    points.windows(2).map(|w| (w[1] - w[0]).scaled(sx, sy).len()).sum()
}

/// The length around a closed ring, each axis scaled.
pub fn ring_perimeter(ring: &[Pt], sx: f64, sy: f64) -> f64 {
    match ring {
        [] | [_] => 0.0,
        [first, .., last] => path_length(ring, sx, sy) + (*first - *last).scaled(sx, sy).len(),
    }
}

/// `ring` without consecutive repeated points, the closing point included:
/// a double click often lands twice on the same spot.
pub fn dedup_ring(ring: &[Pt]) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::with_capacity(ring.len());
    for &p in ring {
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// Which side of the line through `a` and `b` the point `c` is: +1 left, -1
/// right, 0 on it (within a tolerance relative to the sizes involved).
fn orientation(a: Pt, b: Pt, c: Pt) -> i8 {
    let (u, v) = (b - a, c - a);
    let value = u.cross(v);
    let tolerance = 1e-12 * (u.len() * v.len()).max(f64::MIN_POSITIVE);
    if value > tolerance {
        1
    } else if value < -tolerance {
        -1
    } else {
        0
    }
}

/// Whether `p`, known to be on the line through `a` and `b`, is within the
/// segment's box.
fn within(a: Pt, b: Pt, p: Pt) -> bool {
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// Whether segments `a`–`b` and `c`–`d` share any point, ends included.
pub fn segments_intersect(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    let (o1, o2, o3, o4) = (orientation(a, b, c), orientation(a, b, d), orientation(c, d, a), orientation(c, d, b));
    if o1 != o2 && o3 != o4 {
        return true;
    }
    (o1 == 0 && within(a, b, c)) || (o2 == 0 && within(a, b, d)) || (o3 == 0 && within(c, d, a)) || (o4 == 0 && within(c, d, b))
}

/// The ring's edges as pairs of points, the closing edge last.
fn edges(ring: &[Pt]) -> impl Iterator<Item = (Pt, Pt)> + '_ {
    (0..ring.len()).map(move |i| (ring[i], ring[(i + 1) % ring.len()]))
}

/// Whether a closed ring is a simple polygon: at least three distinct points,
/// some area, and no edge touching another except its neighbours at their
/// shared corner. A ring that doubles back along itself isn't simple either.
///
/// Compares every pair of edges, skipping those whose boxes don't meet, which
/// is quick for the few hundred points a hand-drawn area has.
pub fn is_simple(ring: &[Pt]) -> bool {
    let ring = dedup_ring(ring);
    let n = ring.len();
    if n < 3 || signed_area(&ring) == 0.0 {
        return false;
    }
    let boxes: Vec<Rect> = edges(&ring).map(|(a, b)| Rect::from_corners(a, b)).collect();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        for j in i + 1..n {
            let (c, d) = (ring[j], ring[(j + 1) % n]);
            let adjacent = j == i + 1 || (i == 0 && j == n - 1);
            if adjacent {
                // Neighbours share a corner; they mustn't also run back over
                // each other.
                let (shared, other_a, other_b) = if j == i + 1 { (b, a, d) } else { (a, b, c) };
                if orientation(other_a, shared, other_b) == 0 && (other_a - shared).dot(other_b - shared) > 0.0 {
                    return false;
                }
                continue;
            }
            if boxes[i].intersects(&boxes[j]) && segments_intersect(a, b, c, d) {
                return false;
            }
        }
    }
    true
}

/// Whether any edge of ring `r` meets any edge of ring `s`.
pub fn rings_cross(r: &[Pt], s: &[Pt]) -> bool {
    let Some(sb) = Rect::around(s) else { return false };
    edges(r).any(|(a, b)| {
        Rect::from_corners(a, b).intersects(&sb) && edges(s).any(|(c, d)| segments_intersect(a, b, c, d))
    })
}

/// Whether `p` is inside the closed ring, by the even-odd rule. Points exactly
/// on an edge may go either way.
pub fn point_in_ring(p: Pt, ring: &[Pt]) -> bool {
    let mut inside = false;
    for (a, b) in edges(ring) {
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// The nearest point to `p` on segment `a`–`b`, and how far along it is (0 at
/// `a`, 1 at `b`).
pub fn nearest_on_segment(p: Pt, a: Pt, b: Pt) -> (Pt, f64) {
    let ab = b - a;
    let length2 = ab.dot(ab);
    if length2 == 0.0 {
        return (a, 0.0);
    }
    let t = ((p - a).dot(ab) / length2).clamp(0.0, 1.0);
    (a + ab * t, t)
}

pub fn distance_to_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    nearest_on_segment(p, a, b).0.dist(p)
}

/// The centre and radius of the circle through three points, or `None` when
/// they lie on a line.
pub fn circle_through(a: Pt, b: Pt, c: Pt) -> Option<(Pt, f64)> {
    let (ab, ac) = (b - a, c - a);
    let d = 2.0 * ab.cross(ac);
    if orientation(a, b, c) == 0 || d == 0.0 {
        return None;
    }
    let (ab2, ac2) = (ab.dot(ab), ac.dot(ac));
    let offset = Pt::new((ac.y * ab2 - ab.y * ac2) / d, (ab.x * ac2 - ac.x * ab2) / d);
    Some((a + offset, offset.len()))
}

/// The angle at `vertex` between the arms to `a` and `b`, in degrees from 0
/// to 180. `None` if either arm has no length.
pub fn angle_between(a: Pt, vertex: Pt, b: Pt) -> Option<f64> {
    let (u, v) = (a - vertex, b - vertex);
    if u.len() == 0.0 || v.len() == 0.0 {
        return None;
    }
    Some(u.cross(v).abs().atan2(u.dot(v)).to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(coords: &[(f64, f64)]) -> Vec<Pt> {
        coords.iter().map(|&(x, y)| Pt::new(x, y)).collect()
    }

    #[test]
    fn a_square_has_its_area_signed_by_direction() {
        let square = pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        assert_eq!(signed_area(&square), 100.0);
        let clockwise: Vec<Pt> = square.iter().rev().copied().collect();
        assert_eq!(signed_area(&clockwise), -100.0);
        assert_eq!(ring_perimeter(&square, 1.0, 1.0), 40.0);
        assert_eq!(ring_perimeter(&square, 2.0, 0.5), 50.0, "20 + 5 + 20 + 5");
    }

    #[test]
    fn a_bow_tie_is_not_simple() {
        assert!(!is_simple(&pts(&[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)])));
        assert!(is_simple(&pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)])));
    }

    #[test]
    fn a_ring_touching_itself_or_doubling_back_is_not_simple() {
        // The fifth corner sits on the first edge.
        assert!(!is_simple(&pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (5.0, 5.0), (5.0, 0.0), (0.0, 10.0)])));
        // A spike back along the edge it came in on.
        assert!(!is_simple(&pts(&[(0.0, 0.0), (10.0, 0.0), (5.0, 0.0), (5.0, 10.0)])));
        assert!(!is_simple(&pts(&[(0.0, 0.0), (5.0, 0.0), (10.0, 0.0)])), "no area");
    }

    #[test]
    fn a_repeated_point_from_a_double_click_is_ignored() {
        let ring = pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)]);
        assert!(is_simple(&ring));
        assert_eq!(dedup_ring(&ring).len(), 4);
    }

    #[test]
    fn crossing_and_touching_segments_meet() {
        let p = |x, y| Pt::new(x, y);
        assert!(segments_intersect(p(0.0, 0.0), p(10.0, 10.0), p(0.0, 10.0), p(10.0, 0.0)));
        assert!(segments_intersect(p(0.0, 0.0), p(10.0, 0.0), p(10.0, 0.0), p(10.0, 5.0)), "at an end");
        assert!(segments_intersect(p(0.0, 0.0), p(10.0, 0.0), p(5.0, 0.0), p(15.0, 0.0)), "overlapping");
        assert!(!segments_intersect(p(0.0, 0.0), p(10.0, 0.0), p(11.0, 0.0), p(15.0, 0.0)), "in line, apart");
        assert!(!segments_intersect(p(0.0, 0.0), p(10.0, 0.0), p(0.0, 1.0), p(10.0, 1.0)), "parallel");
    }

    #[test]
    fn the_circle_through_three_points() {
        let (centre, radius) = circle_through(Pt::new(5.0, 0.0), Pt::new(0.0, 5.0), Pt::new(-5.0, 0.0)).unwrap();
        assert!(centre.dist(Pt::new(0.0, 0.0)) < 1e-12 && (radius - 5.0).abs() < 1e-12);
        assert!(circle_through(Pt::new(0.0, 0.0), Pt::new(1.0, 1.0), Pt::new(2.0, 2.0)).is_none());
    }

    #[test]
    fn angles_run_from_nought_to_a_straight_line() {
        let o = Pt::new(0.0, 0.0);
        assert!((angle_between(Pt::new(1.0, 0.0), o, Pt::new(0.0, 1.0)).unwrap() - 90.0).abs() < 1e-12);
        assert!((angle_between(Pt::new(1.0, 0.0), o, Pt::new(0.0, -1.0)).unwrap() - 90.0).abs() < 1e-12);
        assert!((angle_between(Pt::new(1.0, 0.0), o, Pt::new(-1.0, 0.0)).unwrap() - 180.0).abs() < 1e-12);
        assert!(angle_between(o, o, Pt::new(1.0, 0.0)).is_none());
    }

    #[test]
    fn inside_a_ring_and_near_a_segment() {
        let square = pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        assert!(point_in_ring(Pt::new(5.0, 5.0), &square));
        assert!(!point_in_ring(Pt::new(15.0, 5.0), &square));
        assert_eq!(distance_to_segment(Pt::new(5.0, 3.0), Pt::new(0.0, 0.0), Pt::new(10.0, 0.0)), 3.0);
        assert_eq!(distance_to_segment(Pt::new(13.0, 4.0), Pt::new(0.0, 0.0), Pt::new(10.0, 0.0)), 5.0);
    }
}

/// A simple ring cut into triangles, by ear clipping.
///
/// Areas are drawn by filling triangles: a fan from one corner, which is what
/// a drawing library gives you for free, is only right for a convex shape --
/// on a concave one it fills across the notch and throws spikes outside the
/// shape. Ear clipping is O(n²) on the corners, which is nothing for the
/// hundreds a hand-drawn outline has. An outline that crosses itself has no
/// area anyone can price, so it gives nothing.
pub fn triangulate(ring: &[Pt]) -> Vec<[Pt; 3]> {
    let ring = dedup_ring(ring);
    if ring.len() < 3 || !is_simple(&ring) {
        return Vec::new();
    }
    // Anticlockwise, so an ear is a corner that turns the same way.
    let mut left: Vec<Pt> = if signed_area(&ring) < 0.0 { ring.into_iter().rev().collect() } else { ring };
    let mut out = Vec::with_capacity(left.len().saturating_sub(2));
    let mut guard = left.len() * left.len();
    while left.len() > 3 && guard > 0 {
        guard -= 1;
        let n = left.len();
        let ear = (0..n).find(|&i| {
            let (a, b, c) = (left[(i + n - 1) % n], left[i], left[(i + 1) % n]);
            // A corner that turns anticlockwise, with no other corner inside it.
            if (b - a).cross(c - b) <= 0.0 {
                return false;
            }
            !left.iter().enumerate().any(|(j, &p)| {
                let outside = j == i || j == (i + n - 1) % n || j == (i + 1) % n;
                !outside && in_triangle(p, a, b, c)
            })
        });
        match ear {
            Some(i) => {
                let n = left.len();
                out.push([left[(i + n - 1) % n], left[i], left[(i + 1) % n]]);
                left.remove(i);
            }
            // No ear found: the ring isn't as simple as it looked.
            None => return out,
        }
    }
    if let [a, b, c] = left[..] {
        out.push([a, b, c]);
    }
    out
}

/// Whether `p` is inside or on the edge of the triangle `a`, `b`, `c`.
fn in_triangle(p: Pt, a: Pt, b: Pt, c: Pt) -> bool {
    let side = |from: Pt, to: Pt| (to - from).cross(p - from);
    let (x, y, z) = (side(a, b), side(b, c), side(c, a));
    (x >= 0.0 && y >= 0.0 && z >= 0.0) || (x <= 0.0 && y <= 0.0 && z <= 0.0)
}

#[cfg(test)]
mod triangles {
    use super::*;

    fn pts(coords: &[(f64, f64)]) -> Vec<Pt> {
        coords.iter().map(|&(x, y)| Pt::new(x, y)).collect()
    }

    /// The triangles cover the ring: their areas add up to its own, and none
    /// of them turns the other way or reaches outside it.
    fn covers(ring: &[Pt]) {
        let triangles = triangulate(ring);
        assert_eq!(triangles.len(), dedup_ring(ring).len() - 2, "one triangle per corner but two");
        let total: f64 = triangles.iter().map(|t| signed_area(t).abs()).sum();
        assert!((total - signed_area(ring).abs()).abs() < 1e-9, "{total} against {}", signed_area(ring).abs());
        for t in &triangles {
            let middle = Pt::new((t[0].x + t[1].x + t[2].x) / 3.0, (t[0].y + t[1].y + t[2].y) / 3.0);
            assert!(point_in_ring(middle, ring), "a triangle reaches outside the shape: {t:?}");
        }
    }

    #[test]
    fn a_square_is_two_triangles() {
        covers(&pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]));
    }

    #[test]
    fn a_notch_is_not_filled_across() {
        // The shape from the app: a deep notch in the top edge.
        covers(&pts(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (60.0, 90.0), (50.0, 40.0), (40.0, 90.0), (0.0, 100.0)]));
    }

    #[test]
    fn either_winding_works_and_a_crossed_ring_gives_nothing() {
        let ring = pts(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (60.0, 90.0), (50.0, 40.0), (40.0, 90.0), (0.0, 100.0)]);
        let backwards: Vec<Pt> = ring.iter().rev().copied().collect();
        covers(&backwards);
        assert!(triangulate(&pts(&[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)])).is_empty(), "a bow tie");
        assert!(triangulate(&pts(&[(0.0, 0.0), (10.0, 0.0)])).is_empty());
    }

    #[test]
    fn a_long_thin_spiral_still_comes_out_whole() {
        let mut ring = Vec::new();
        for i in 0..40 {
            let t = f64::from(i) * 0.4;
            ring.push(Pt::new(t.cos() * (10.0 + t), t.sin() * (10.0 + t)));
        }
        for i in (0..40).rev() {
            let t = f64::from(i) * 0.4;
            ring.push(Pt::new(t.cos() * (12.0 + t), t.sin() * (12.0 + t)));
        }
        if is_simple(&ring) {
            covers(&ring);
        }
    }
}
