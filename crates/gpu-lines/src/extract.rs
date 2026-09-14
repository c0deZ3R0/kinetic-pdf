//! Reading a page's paths out of pdfium, as shapes the GPU draws: pieces of
//! stroked lines, and triangles covering filled areas.

use lyon_tessellation::math::point;
use lyon_tessellation::path::Path;
use lyon_tessellation::{BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers};
use pdfium_render::prelude::*;

/// One shape to draw, in page space: points, with the origin at the page's
/// bottom left. Either a straight piece of a stroked line, from the first
/// point to the second, or a filled triangle of all three.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Primitive {
    pub points: [[f32; 2]; 3],
    /// A line's width in points, or 0 for a hairline, which is one pixel wide
    /// at any zoom. Unused for a triangle.
    pub width: f32,
    /// 0 for a line, 1 for a triangle.
    pub kind: f32,
    /// Red, green, blue and alpha, 0 to 1, not premultiplied.
    pub colour: [f32; 4],
}

// SAFETY: twelve f32s in a repr(C) struct: no padding, any bit pattern valid.
unsafe impl bytemuck::Zeroable for Primitive {}
unsafe impl bytemuck::Pod for Primitive {}

impl Primitive {
    pub fn line(from: [f32; 2], to: [f32; 2], width: f32, colour: [f32; 4]) -> Self {
        Primitive { points: [from, to, to], width, kind: 0.0, colour }
    }

    pub fn triangle(points: [[f32; 2]; 3], colour: [f32; 4]) -> Self {
        Primitive { points, width: 0.0, kind: 1.0, colour }
    }

    pub fn is_triangle(&self) -> bool {
        self.kind > 0.5
    }
}

/// What `extract` found.
#[derive(Debug, Default)]
pub struct Extracted {
    /// Everything to draw, in the order the page paints it.
    pub primitives: Vec<Primitive>,
    pub lines: usize,
    pub triangles: usize,
    /// Paths stroked, and paths filled (a path can be both).
    pub stroked: usize,
    pub filled: usize,
    /// Filled paths the tessellator couldn't turn into triangles.
    pub unfilled: usize,
    /// Of the paths painted: with a clip path, a dash pattern, or
    /// transparency or a blend mode, all ignored -- they're drawn whole,
    /// solid and opaque.
    pub clipped: usize,
    pub dashed: usize,
    pub see_through: usize,
    /// Objects that aren't paths (text, images, shadings), and paths that
    /// paint nothing: not drawn.
    pub other: usize,
}

/// Every path in `objects` and the forms inside them, as shapes in page
/// space. Curves are flattened to within `tolerance` points.
pub fn extract<'a>(objects: impl IntoIterator<Item = PdfPageObject<'a>>, tolerance: f32) -> Extracted {
    let mut out = Extracted::default();
    let mut tessellator = FillTessellator::new();
    for object in objects {
        walk(&object, PdfMatrix::IDENTITY, tolerance.max(0.001), &mut tessellator, &mut out);
    }
    out
}

fn walk(object: &PdfPageObject, parent: PdfMatrix, tolerance: f32, tessellator: &mut FillTessellator, out: &mut Extracted) {
    let matrix = object.matrix().unwrap_or(PdfMatrix::IDENTITY).multiply(parent);
    if let Some(form) = object.as_x_object_form_object() {
        for i in 0..form.len() {
            if let Ok(child) = form.get(i) {
                walk(&child, matrix, tolerance, tessellator, out);
            }
        }
        return;
    }
    let Some(path) = object.as_path_object() else {
        out.other += 1;
        return;
    };
    let fill_mode = path.fill_mode().unwrap_or(PdfPathFillMode::None);
    let filled = !matches!(fill_mode, PdfPathFillMode::None);
    let stroked = path.is_stroked().unwrap_or(false);
    if !filled && !stroked {
        out.other += 1;
        return;
    }
    if object.get_clip_path().is_some_and(|c| c.len() > 0) {
        out.clipped += 1;
    }
    if object.has_transparency() {
        out.see_through += 1;
    }

    // The path's outline in page space, read once for the fill and the stroke.
    let place = |x: f32, y: f32| {
        let (x, y) = matrix.apply_to_points(PdfPoints::new(x), PdfPoints::new(y));
        [x.value, y.value]
    };
    let outline = pieces(path.segments().iter().map(|s| {
        let (x, y) = s.point();
        (s.segment_type(), place(x.value, y.value), s.is_close())
    }));
    let colour_of = |c: Result<PdfColor, PdfiumError>| {
        c.map(|c| [c.red(), c.green(), c.blue(), c.alpha()].map(|v| v as f32 / 255.0)).unwrap_or([0.0, 0.0, 0.0, 1.0])
    };

    // A path that's filled and stroked is filled first, as PDF paints it.
    if filled {
        out.filled += 1;
        let rule = if matches!(fill_mode, PdfPathFillMode::EvenOdd) { FillRule::EvenOdd } else { FillRule::NonZero };
        let colour = colour_of(path.fill_color());
        match fill(&outline, rule, tolerance, tessellator) {
            Some(triangles) => {
                out.triangles += triangles.len();
                out.primitives.extend(triangles.into_iter().map(|t| Primitive::triangle(t, colour)));
            }
            None => out.unfilled += 1,
        }
    }
    if stroked {
        out.stroked += 1;
        if path.dash_array().is_ok_and(|d| !d.is_empty()) {
            out.dashed += 1;
        }
        // Widths scale with the matrix; a skewed or stretched one is averaged.
        let scale = (matrix.a() * matrix.d() - matrix.b() * matrix.c()).abs().sqrt();
        let width = path.stroke_width().map_or(0.0, |w| w.value) * scale;
        let colour = colour_of(path.stroke_color());
        let before = out.primitives.len();
        stroke(&outline, tolerance, |from, to| out.primitives.push(Primitive::line(from, to, width, colour)));
        out.lines += out.primitives.len() - before;
    }
}

/// One step along a path's outline.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Piece {
    Move([f32; 2]),
    Line([f32; 2]),
    /// Two control points and the end.
    Curve([f32; 2], [f32; 2], [f32; 2]),
    /// Back to where the subpath started.
    Close,
}

/// A path's segments as pdfium lists them -- kind, point, and whether it
/// closes the subpath -- as the steps of its outline. pdfium lists a curve as
/// three points, two controls then the end; they come out as one `Curve`.
fn pieces(segments: impl IntoIterator<Item = (PdfPathSegmentType, [f32; 2], bool)>) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut controls: Vec<[f32; 2]> = Vec::with_capacity(3);
    for (kind, point, close) in segments {
        match kind {
            PdfPathSegmentType::MoveTo => out.push(Piece::Move(point)),
            PdfPathSegmentType::LineTo => out.push(Piece::Line(point)),
            PdfPathSegmentType::BezierTo => {
                controls.push(point);
                if let [c1, c2, end] = controls[..] {
                    out.push(Piece::Curve(c1, c2, end));
                    controls.clear();
                }
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
        if close {
            out.push(Piece::Close);
        }
    }
    out
}

/// Calls `line` for each straight piece of the outline, curves flattened to
/// within `tolerance`.
fn stroke(outline: &[Piece], tolerance: f32, mut line: impl FnMut([f32; 2], [f32; 2])) {
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
fn fill(outline: &[Piece], rule: FillRule, tolerance: f32, tessellator: &mut FillTessellator) -> Option<Vec<[[f32; 2]; 3]>> {
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
    ((reach / tolerance).sqrt().ceil() as usize).clamp(1, 64)
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
    use PdfPathSegmentType::{BezierTo, LineTo, MoveTo};

    fn area(triangles: &[[[f32; 2]; 3]]) -> f32 {
        triangles
            .iter()
            .map(|[a, b, c]| ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0)
            .sum()
    }

    #[test]
    fn a_primitive_is_twelve_floats() {
        assert_eq!(std::mem::size_of::<Primitive>(), 48);
        let line = Primitive::line([1.0, 2.0], [3.0, 4.0], 0.5, [1.0, 0.0, 0.0, 1.0]);
        let bytes: &[u8] = bytemuck::bytes_of(&line);
        assert_eq!(&bytes[24..28], &0.5_f32.to_ne_bytes(), "width after the three points");
        assert!(!line.is_triangle() && Primitive::triangle([[0.0; 2]; 3], [0.0; 4]).is_triangle());
    }

    #[test]
    fn a_curve_ends_where_it_should() {
        let (p0, p3) = ([0.0, 0.0], [10.0, 0.0]);
        assert_eq!(cubic(p0, [0.0, 5.0], [10.0, 5.0], p3, 0.0), p0);
        assert_eq!(cubic(p0, [0.0, 5.0], [10.0, 5.0], p3, 1.0), p3);
        let middle = cubic(p0, [0.0, 5.0], [10.0, 5.0], p3, 0.5);
        assert!((middle[0] - 5.0).abs() < 1e-5 && (middle[1] - 3.75).abs() < 1e-5);
    }

    #[test]
    fn pdfiums_three_curve_points_become_one_curve() {
        let outline = pieces([(MoveTo, [0.0, 0.0], false), (BezierTo, [0.0, 5.0], false), (BezierTo, [10.0, 5.0], false), (BezierTo, [10.0, 0.0], true)]);
        assert_eq!(outline, [Piece::Move([0.0, 0.0]), Piece::Curve([0.0, 5.0], [10.0, 5.0], [10.0, 0.0]), Piece::Close]);
    }

    /// A 10 x 10 square with a 4 x 4 square inside it, both drawn the same way
    /// round, as `x y m ... h` subpaths.
    fn square_with_hole() -> Vec<Piece> {
        pieces([
            (MoveTo, [0.0, 0.0], false),
            (LineTo, [10.0, 0.0], false),
            (LineTo, [10.0, 10.0], false),
            (LineTo, [0.0, 10.0], true),
            (MoveTo, [3.0, 3.0], false),
            (LineTo, [7.0, 3.0], false),
            (LineTo, [7.0, 7.0], false),
            (LineTo, [3.0, 7.0], true),
        ])
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
        let triangle = pieces([(MoveTo, [0.0, 0.0], false), (LineTo, [4.0, 0.0], false), (LineTo, [0.0, 4.0], false)]);
        let filled = fill(&triangle, FillRule::NonZero, 0.05, &mut FillTessellator::new()).unwrap();
        assert!((area(&filled) - 8.0).abs() < 0.01);
        let mut steps = 0;
        stroke(&triangle, 0.05, |_, _| steps += 1);
        assert_eq!(steps, 2, "no closing piece unless the path closes");
    }
}
