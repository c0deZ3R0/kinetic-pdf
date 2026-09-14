//! Geometry in page space: transformation matrices, and outlines turned into
//! pieces of line and fill triangles.

use lyon_tessellation::math::point;
use lyon_tessellation::path::Path;
use lyon_tessellation::{BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers};

/// An affine transformation as PDF writes one, `[a b c d e f]`, taking (x, y)
/// to (a x + c y + e, b x + d y + f).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix(pub [f32; 6]);

impl Matrix {
    pub const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn translate(x: f32, y: f32) -> Matrix {
        Matrix([1.0, 0.0, 0.0, 1.0, x, y])
    }

    pub fn scale(x: f32, y: f32) -> Matrix {
        Matrix([x, 0.0, 0.0, y, 0.0, 0.0])
    }

    /// This transformation, followed by `then`. PDF's `cm` puts its matrix
    /// before the current one: `cm.then(ctm)`.
    pub fn then(self, then: Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = then.0;
        Matrix([a * a2 + b * c2, a * b2 + b * d2, c * a2 + d * c2, c * b2 + d * d2, e * a2 + f * c2 + e2, e * b2 + f * d2 + f2])
    }

    pub fn apply(self, [x, y]: [f32; 2]) -> [f32; 2] {
        let [a, b, c, d, e, f] = self.0;
        [a * x + c * y + e, b * x + d * y + f]
    }

    /// How much it scales lengths, averaged over directions: what a stroke's
    /// width is multiplied by.
    pub fn length_scale(self) -> f32 {
        let [a, b, c, d, ..] = self.0;
        (a * d - b * c).abs().sqrt()
    }

    /// The transformation undoing this one; `None` if it flattens everything.
    pub fn inverse(self) -> Option<Matrix> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        (det.abs() > f32::EPSILON).then(|| Matrix([d / det, -b / det, -c / det, a / det, (c * f - d * e) / det, (b * e - a * f) / det]))
    }
}

/// A half-plane `[a, b, c]`: a point (x, y) is inside where `a x + b y + c` is
/// not below 0, and that is its distance inside, in page units.
pub type Plane = [f32; 3];

/// The half-planes a shape covered by `triangles` is the inside of all of, if
/// it's convex; `None` if it isn't (or has holes). A shape with no area gives
/// a half-plane with nothing inside it.
pub(crate) fn convex_planes(triangles: &[[[f32; 2]; 3]]) -> Option<Vec<Plane>> {
    const NOTHING: Plane = [0.0, 0.0, -1.0];
    let mut points: Vec<[f32; 2]> = triangles.iter().flatten().copied().collect();
    points.sort_by(|p, q| p[0].total_cmp(&q[0]).then(p[1].total_cmp(&q[1])));
    points.dedup();
    let hull = convex_hull(&points);
    let hull_area = polygon_area(&hull);
    if hull.len() < 3 || hull_area <= f32::EPSILON {
        return Some(vec![NOTHING]);
    }
    // Convex exactly when the triangles cover the whole of their hull.
    let area: f32 = triangles.iter().map(|[a, b, c]| polygon_area(&[*a, *b, *c])).sum();
    if (hull_area - area).abs() > hull_area * 1e-3 {
        return None;
    }
    Some(
        hull.iter()
            .zip(hull.iter().cycle().skip(1))
            .map(|(p, q)| {
                // The hull runs anticlockwise, so inside is on each edge's left.
                let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
                let length = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
                let (a, b) = (-dy / length, dx / length);
                [a, b, -(a * p[0] + b * p[1])]
            })
            .collect(),
    )
}

/// The convex hull of `points`, sorted by x then y, anticlockwise (Andrew's
/// monotone chain).
fn convex_hull(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let turn = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut hull: Vec<[f32; 2]> = Vec::with_capacity(points.len() + 1);
    for pass in [points.to_vec(), points.iter().rev().copied().collect()] {
        let floor = hull.len();
        for p in pass {
            while hull.len() >= floor + 2 && turn(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// The area inside a polygon, whichever way round it runs.
fn polygon_area(corners: &[[f32; 2]]) -> f32 {
    corners.iter().zip(corners.iter().cycle().skip(1)).map(|(p, q)| p[0] * q[1] - q[0] * p[1]).sum::<f32>().abs() / 2.0
}

/// One step along an outline, in page space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Piece {
    Move([f32; 2]),
    Line([f32; 2]),
    /// Two control points and the end.
    Curve([f32; 2], [f32; 2], [f32; 2]),
    /// Back to where the subpath started.
    Close,
}

/// Calls `visit` with the points along each subpath of the outline that
/// strokes paint, and whether it's closed. Curves are flattened to within
/// `tolerance`, a point the same as the one before is left out, and a closed
/// subpath doesn't repeat its first point at the end. A subpath that's only a
/// move isn't painted, so isn't visited; one that goes nowhere is, as a
/// single point.
pub(crate) fn polylines(outline: &[Piece], tolerance: f32, mut visit: impl FnMut(&[[f32; 2]], bool)) {
    fn add(points: &mut Vec<[f32; 2]>, point: [f32; 2]) {
        if points.last() != Some(&point) {
            points.push(point);
        }
    }
    let mut points: Vec<[f32; 2]> = Vec::new();
    let mut start = [0.0_f32; 2];
    let mut painted = false;
    for &piece in outline {
        // A line or curve after a close carries on from the subpath's start.
        if points.is_empty() && matches!(piece, Piece::Line(_) | Piece::Curve(..)) {
            points.push(start);
        }
        match piece {
            Piece::Move(at) => {
                if painted {
                    visit(&points, false);
                }
                points.clear();
                points.push(at);
                (start, painted) = (at, false);
            }
            Piece::Line(at) => {
                add(&mut points, at);
                painted = true;
            }
            Piece::Curve(c1, c2, end) => {
                let from = points[points.len() - 1];
                let steps = curve_steps(from, c1, c2, end, tolerance);
                for i in 1..=steps {
                    add(&mut points, cubic(from, c1, c2, end, i as f32 / steps as f32));
                }
                painted = true;
            }
            Piece::Close if points.is_empty() => {}
            Piece::Close => {
                if points.len() > 1 && points.first() == points.last() {
                    points.pop();
                }
                visit(&points, true);
                points.clear();
                painted = false;
            }
        }
    }
    if painted {
        visit(&points, false);
    }
}

/// The triangles covering the outline's filled area, by `rule`; `None` if it
/// couldn't be tessellated. Every subpath counts as closed, as PDF fills it.
pub(crate) fn fill(outline: &[Piece], rule: FillRule, tolerance: f32, tessellator: &mut FillTessellator) -> Option<Vec<[[f32; 2]; 3]>> {
    let p = |v: [f32; 2]| point(v[0], v[1]);
    let mut builder = Path::builder();
    let mut open = false;
    let mut start = [0.0_f32; 2];
    for &piece in outline {
        // A line or curve after a close carries on from the subpath's start.
        if matches!(piece, Piece::Line(_) | Piece::Curve(..)) && !open {
            builder.begin(p(start));
            open = true;
        }
        match piece {
            Piece::Move(at) => {
                if open {
                    builder.end(true);
                }
                builder.begin(p(at));
                open = true;
                start = at;
            }
            Piece::Line(at) => {
                builder.line_to(p(at));
            }
            Piece::Curve(c1, c2, end) => {
                builder.cubic_bezier_to(p(c1), p(c2), p(end));
            }
            Piece::Close => {
                if open {
                    builder.end(true);
                    open = false;
                }
            }
        }
    }
    if open {
        builder.end(true);
    }
    let path = builder.build();

    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let options = FillOptions::tolerance(tolerance).with_fill_rule(rule);
    tessellator
        .tessellate_path(&path, &options, &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| v.position().to_array()))
        .ok()?;
    Some(
        buffers
            .indices
            .chunks_exact(3)
            .map(|t| [buffers.vertices[t[0] as usize], buffers.vertices[t[1] as usize], buffers.vertices[t[2] as usize]])
            .collect(),
    )
}

/// How many straight steps a curve is drawn in to stay within `tolerance`.
fn curve_steps(p0: [f32; 2], c1: [f32; 2], c2: [f32; 2], p3: [f32; 2], tolerance: f32) -> usize {
    let reach = distance(p0, c1) + distance(c1, c2) + distance(c2, p3);
    ((reach / tolerance.max(0.001)).sqrt().ceil() as usize).clamp(1, 64)
}

pub(crate) fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn cubic(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [0, 1].map(|i| a * p0[i] + b * p1[i] + c * p2[i] + d * p3[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(triangles: &[[[f32; 2]; 3]]) -> f32 {
        triangles
            .iter()
            .map(|[a, b, c]| ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0)
            .sum()
    }

    #[test]
    fn matrices_apply_in_the_order_pdf_writes_them() {
        let (scale, shift) = (Matrix::scale(2.0, 3.0), Matrix::translate(5.0, 7.0));
        assert_eq!(scale.then(shift).apply([1.0, 1.0]), [7.0, 10.0], "scaled, then moved");
        assert_eq!(shift.then(scale).apply([1.0, 1.0]), [12.0, 24.0], "moved, then scaled");
        assert_eq!(Matrix::scale(2.0, 8.0).length_scale(), 4.0);
    }

    #[test]
    fn a_matrix_and_its_inverse_undo_each_other() {
        let m = Matrix([0.0, 2.0, -3.0, 0.0, 5.0, 7.0]);
        let back = m.inverse().expect("it doesn't flatten anything").apply(m.apply([1.5, -2.0]));
        assert!((back[0] - 1.5).abs() < 1e-5 && (back[1] + 2.0).abs() < 1e-5, "{back:?}");
        assert_eq!(Matrix::scale(0.0, 1.0).inverse(), None);
    }

    fn inside(planes: &[Plane], [x, y]: [f32; 2]) -> bool {
        planes.iter().all(|[a, b, c]| a * x + b * y + c >= 0.0)
    }

    #[test]
    fn a_convex_shape_becomes_its_edges() {
        let square = [[[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]], [[0.0, 0.0], [4.0, 4.0], [0.0, 4.0]]];
        let planes = convex_planes(&square).expect("a square is convex");
        assert_eq!(planes.len(), 4);
        assert!(inside(&planes, [2.0, 2.0]) && !inside(&planes, [5.0, 2.0]) && !inside(&planes, [2.0, -1.0]));
        let [a, b, c] = planes[0];
        assert!((a * a + b * b - 1.0).abs() < 1e-5 && c.is_finite(), "distances in page units");
    }

    #[test]
    fn a_shape_that_isnt_convex_has_no_edges_and_an_empty_one_hides_everything() {
        // An L: a 2 x 4 upright and a 2 x 2 foot beside it.
        let l = [[[0.0, 0.0], [2.0, 0.0], [2.0, 4.0]], [[0.0, 0.0], [2.0, 4.0], [0.0, 4.0]], [[2.0, 0.0], [4.0, 0.0], [4.0, 2.0]], [[2.0, 0.0], [4.0, 2.0], [2.0, 2.0]]];
        assert_eq!(convex_planes(&l), None);
        let nothing = convex_planes(&[]).expect("no area is a clip too");
        assert!(!inside(&nothing, [0.0, 0.0]));
    }

    #[test]
    fn a_curve_ends_where_it_should() {
        let (p0, p3) = ([0.0, 0.0], [10.0, 0.0]);
        assert_eq!(cubic(p0, [0.0, 5.0], [10.0, 5.0], p3, 0.0), p0);
        assert_eq!(cubic(p0, [0.0, 5.0], [10.0, 5.0], p3, 1.0), p3);
        let middle = cubic(p0, [0.0, 5.0], [10.0, 5.0], p3, 0.5);
        assert!((middle[0] - 5.0).abs() < 1e-5 && (middle[1] - 3.75).abs() < 1e-5);
    }

    /// A 10 x 10 square with a 4 x 4 square inside it, both drawn the same way
    /// round.
    fn square_with_hole() -> Vec<Piece> {
        use Piece::{Close, Line, Move};
        vec![
            Move([0.0, 0.0]),
            Line([10.0, 0.0]),
            Line([10.0, 10.0]),
            Line([0.0, 10.0]),
            Close,
            Move([3.0, 3.0]),
            Line([7.0, 3.0]),
            Line([7.0, 7.0]),
            Line([3.0, 7.0]),
            Close,
        ]
    }

    #[test]
    fn fills_follow_their_rule() {
        let mut tessellator = FillTessellator::new();
        let even_odd = fill(&square_with_hole(), FillRule::EvenOdd, 0.05, &mut tessellator).unwrap();
        assert!((area(&even_odd) - 84.0).abs() < 0.01, "even-odd leaves the hole: {}", area(&even_odd));
        let non_zero = fill(&square_with_hole(), FillRule::NonZero, 0.05, &mut tessellator).unwrap();
        assert!((area(&non_zero) - 100.0).abs() < 0.01, "non-zero, both the same way round, fills it: {}", area(&non_zero));
    }

    #[test]
    fn an_unclosed_subpath_is_filled_as_if_closed_and_stroked_as_it_is() {
        let triangle = [Piece::Move([0.0, 0.0]), Piece::Line([4.0, 0.0]), Piece::Line([0.0, 4.0])];
        let filled = fill(&triangle, FillRule::NonZero, 0.05, &mut FillTessellator::new()).unwrap();
        assert!((area(&filled) - 8.0).abs() < 0.01);
        let mut subpaths = Vec::new();
        polylines(&triangle, 0.05, |points, closed| subpaths.push((points.to_vec(), closed)));
        assert_eq!(subpaths, [(vec![[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]], false)], "no closing piece unless the path closes");
    }

    #[test]
    fn subpaths_are_visited_as_strokes_paint_them() {
        use Piece::{Close, Line, Move};
        let outline = [Move([9.0, 9.0]), Move([0.0, 0.0]), Line([1.0, 0.0]), Line([1.0, 1.0]), Line([0.0, 0.0]), Close, Line([0.0, 2.0]), Move([5.0, 5.0]), Close];
        let mut subpaths = Vec::new();
        polylines(&outline, 0.05, |points, closed| subpaths.push((points.to_vec(), closed)));
        assert_eq!(
            subpaths,
            [
                (vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], true),
                (vec![[0.0, 0.0], [0.0, 2.0]], false),
                (vec![[5.0, 5.0]], true),
            ],
            "a lone move isn't painted; a closed subpath doesn't repeat its start; a line after a close starts there; a move then close is a point"
        );
    }
}
