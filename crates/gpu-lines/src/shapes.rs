//! The shapes the GPU draws, in the order they're painted.

use std::collections::BTreeMap;

/// One shape to draw, in page space: points, with the origin at the bottom
/// left of the page as it's drawn. Either a straight piece of a stroked line,
/// from the first point to the second, or a filled triangle of all three.
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

/// How a shape's colour combines with what's already drawn under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    /// Painted over, by its alpha.
    #[default]
    Normal,
    /// Darkens what's under it by its colour: white leaves it as it was.
    Multiply,
}

/// Consecutive shapes, `start` to `start + len` in `Shapes::primitives`,
/// painted with one blend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub len: usize,
    pub blend: Blend,
}

/// What a page's annotations paint, and what couldn't be drawn.
#[derive(Debug, Default)]
pub struct Shapes {
    /// Everything to draw, in the order it's painted.
    pub primitives: Vec<Primitive>,
    /// The primitives split where their blend changes, in order, covering all
    /// of them.
    pub runs: Vec<Run>,
    pub lines: usize,
    pub triangles: usize,
    /// What was left out or drawn only in part, by kind -- text, images,
    /// clips and the like -- and how many times.
    pub not_drawn: BTreeMap<&'static str, usize>,
}

impl Shapes {
    /// Adds a shape, starting a new run if its blend differs from the last.
    pub fn push(&mut self, primitive: Primitive, blend: Blend) {
        match self.runs.last_mut() {
            Some(run) if run.blend == blend => run.len += 1,
            _ => self.runs.push(Run { start: self.primitives.len(), len: 1, blend }),
        }
        if primitive.is_triangle() {
            self.triangles += 1;
        } else {
            self.lines += 1;
        }
        self.primitives.push(primitive);
    }

    /// Counts one more of something that wasn't drawn.
    pub fn not_drawn(&mut self, what: &'static str) {
        *self.not_drawn.entry(what).or_default() += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_primitive_is_twelve_floats() {
        assert_eq!(std::mem::size_of::<Primitive>(), 48);
        let line = Primitive::line([1.0, 2.0], [3.0, 4.0], 0.5, [1.0, 0.0, 0.0, 1.0]);
        let bytes: &[u8] = bytemuck::bytes_of(&line);
        assert_eq!(&bytes[24..28], &0.5_f32.to_ne_bytes(), "width after the three points");
        assert!(!line.is_triangle() && Primitive::triangle([[0.0; 2]; 3], [0.0; 4]).is_triangle());
    }

    #[test]
    fn runs_split_where_the_blend_changes() {
        let mut shapes = Shapes::default();
        let line = Primitive::line([0.0; 2], [1.0; 2], 0.0, [0.0; 4]);
        for blend in [Blend::Normal, Blend::Normal, Blend::Multiply, Blend::Normal] {
            shapes.push(line, blend);
        }
        assert_eq!(
            shapes.runs,
            [
                Run { start: 0, len: 2, blend: Blend::Normal },
                Run { start: 2, len: 1, blend: Blend::Multiply },
                Run { start: 3, len: 1, blend: Blend::Normal },
            ]
        );
        assert_eq!((shapes.lines, shapes.triangles), (4, 0));
    }
}
