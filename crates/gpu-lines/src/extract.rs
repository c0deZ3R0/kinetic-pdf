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

    // The path's segments in page space, once for the fill and the stroke.
    let place = |x: f32, y: f32| {
        let (x, y) = matrix.apply_to_points(PdfPoints::new(x), PdfPoints::new(y));
        [x.value, y.value]
    };
    let segments: Vec<(PdfPathSegmentType, [f32; 2], bool)> = path
        .segments()
        .iter()
        .map(|s| {
            let (x, y) = s.point();
            (s.segment_type(), place(x.value, y.value), s.is_close())
        })
        .collect();
    let colour_of = |c: Result<PdfColor, PdfiumError>| {
        c.map(|c| [c.red(), c.green(), c.blue(), c.alpha()].map(|v| v as f32 / 255.0)).unwrap_or([0.0, 0.0, 0.0, 1.0])
    };

    // A path that's filled and stroked is filled first, as PDF paints it.
    if filled {
        out.filled += 1;
        let rule = if matches!(fill_mode, PdfPathFillMode::EvenOdd) { FillRule::EvenOdd } else { FillRule::NonZero };
        let colour = colour_of(path.fill_color());
        match fill(&segments, rule, tolerance, tessellator) {
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
        stroke(&segments, tolerance, |from, to| out.primitives.push(Primitive::line(from, to, width, colour)));
        out.lines += out.primitives.len() - before;
    }
}

/// Calls `line` for each straight piece of the path's outline, curves
/// flattened.
fn stroke(segments: &[(PdfPathSegmentType, [f32; 2], bool)], tolerance: f32, mut line: impl FnMut([f32; 2], [f32; 2])) {
    let (mut current, mut start) = ([0.0_f32; 2], [0.0_f32; 2]);
    let mut controls: Vec<[f32; 2]> = Vec::with_capacity(3);
    for &(kind, point, close) in segments {
        match kind {
            PdfPathSegmentType::MoveTo => {
                current = point;
                start = point;
            }
            PdfPathSegmentType::LineTo => {
                line(current, point);
                current = point;
            }
            PdfPathSegmentType::BezierTo => {
                // pdfium lists a curve as three points: two controls, then the end.
                controls.push(point);
                if let [c1, c2, end] = controls[..] {
                    let pieces = curve_pieces(current, c1, c2, end, tolerance);
                    let mut previous = current;
                    for i in 1..=pieces {
                        let next = cubic(current, c1, c2, end, i as f32 / pieces as f32);
                        line(previous, next);
                        previous = next;
                    }
                    current = end;
                    controls.clear();
                }
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
        if close {
            if current != start {
                line(current, start);
            }
            current = start;
        }
    }
}

/// The triangles covering the path's filled area, by `rule`; `None` if it
/// couldn't be tessellated. Every subpath counts as closed, as PDF fills it.
fn fill(segments: &[(PdfPathSegmentType, [f32; 2], bool)], rule: FillRule, tolerance: f32, tessellator: &mut FillTessellator) -> Option<Vec<[[f32; 2]; 3]>> {
    let mut builder = Path::builder();
    let mut open = false;
    let mut start = [0.0_f32; 2];
    let mut controls: Vec<[f32; 2]> = Vec::with_capacity(3);
    let p = |v: [f32; 2]| point(v[0], v[1]);
    for &(kind, at, close) in segments {
        match kind {
            PdfPathSegmentType::MoveTo => {
                if open {
                    builder.end(true);
                }
                builder.begin(p(at));
                open = true;
                start = at;
            }
            PdfPathSegmentType::LineTo => {
                if !open {
                    builder.begin(p(start));
                    open = true;
                }
                builder.line_to(p(at));
            }
            PdfPathSegmentType::BezierTo => {
                controls.push(at);
                if let [c1, c2, end] = controls[..] {
                    if !open {
                        builder.begin(p(start));
                        open = true;
                    }
                    builder.cubic_bezier_to(p(c1), p(c2), p(end));
                    controls.clear();
                }
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
        if close && open {
            builder.end(true);
            open = false;
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

fn curve_pieces(p0: [f32; 2], c1: [f32; 2], c2: [f32; 2], p3: [f32; 2], tolerance: f32) -> usize {
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

    /// A 10 x 10 square with a 4 x 4 square inside it, both drawn the same way
    /// round, as `x y m ... h` subpaths.
    fn square_with_hole() -> Vec<(PdfPathSegmentType, [f32; 2], bool)> {
        use PdfPathSegmentType::{LineTo, MoveTo};
        vec![
            (MoveTo, [0.0, 0.0], false),
            (LineTo, [10.0, 0.0], false),
            (LineTo, [10.0, 10.0], false),
            (LineTo, [0.0, 10.0], true),
            (MoveTo, [3.0, 3.0], false),
            (LineTo, [7.0, 3.0], false),
            (LineTo, [7.0, 7.0], false),
            (LineTo, [3.0, 7.0], true),
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
        use PdfPathSegmentType::{LineTo, MoveTo};
        let triangle = [(MoveTo, [0.0, 0.0], false), (LineTo, [4.0, 0.0], false), (LineTo, [0.0, 4.0], false)];
        let filled = fill(&triangle, FillRule::NonZero, 0.05, &mut FillTessellator::new()).unwrap();
        assert!((area(&filled) - 8.0).abs() < 0.01);
        let mut pieces = 0;
        stroke(&triangle, 0.05, |_, _| pieces += 1);
        assert_eq!(pieces, 2, "no closing piece unless the path closes");
    }
}
