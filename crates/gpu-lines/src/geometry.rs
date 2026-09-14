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

/// Calls `line` for each straight piece of the outline, curves flattened to
/// within `tolerance`.
pub(crate) fn stroke(outline: &[Piece], tolerance: f32, mut line: impl FnMut([f32; 2], [f32; 2])) {
    let (mut current, mut start) = ([0.0_f32; 2], [0.0_f32; 2]);
    for &piece in outline {
        match piece {
            Piece::Move(point) => {
                current = point;
                start = point;
            }
            Piece::Line(point) => {
                line(current, point);
                current = point;
            }
            Piece::Curve(c1, c2, end) => {
                let steps = curve_steps(current, c1, c2, end, tolerance);
                let mut previous = current;
                for i in 1..=steps {
                    let next = cubic(current, c1, c2, end, i as f32 / steps as f32);
                    line(previous, next);
                    previous = next;
                }
                current = end;
            }
            Piece::Close => {
                if current != start {
                    line(current, start);
                }
                current = start;
            }
        }
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

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
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
        let mut steps = 0;
        stroke(&triangle, 0.05, |_, _| steps += 1);
        assert_eq!(steps, 2, "no closing piece unless the path closes");
    }
}
