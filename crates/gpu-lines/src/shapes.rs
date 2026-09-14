//! The shapes the GPU draws, in the order they're painted, and the clips they
//! stay within.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use crate::geometry::{convex_planes, Plane};

/// One shape to draw, in page space: points, with the origin at the bottom
/// left of the page as it's drawn. Either a straight piece of a stroked line,
/// from the first point to the second, or a filled triangle of all three. A
/// line with round ends reaches half its width past them in a half disc, so
/// such lines meeting make a round join, and one going nowhere is a dot.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Primitive {
    pub points: [[f32; 2]; 3],
    /// A line's width in points, or 0 for a hairline, which is one pixel wide
    /// at any zoom. Unused for a triangle.
    pub width: f32,
    /// 0 for a line with square ends, 2 for one with round ends, 1 for a
    /// triangle.
    pub kind: f32,
    /// Red, green, blue and alpha, 0 to 1, not premultiplied.
    pub colour: [f32; 4],
    /// One more than the convex clip set it's drawn within, whose half-planes
    /// the shader tests; 0 for none. Set by `Shapes::push`.
    pub clip: f32,
}

// SAFETY: thirteen f32s in a repr(C) struct: no padding, any bit pattern valid.
unsafe impl bytemuck::Zeroable for Primitive {}
unsafe impl bytemuck::Pod for Primitive {}

impl Primitive {
    const SQUARE_ENDS: f32 = 0.0;
    const TRIANGLE: f32 = 1.0;
    const ROUND_ENDS: f32 = 2.0;

    pub fn line(from: [f32; 2], to: [f32; 2], width: f32, colour: [f32; 4]) -> Self {
        Primitive { points: [from, to, to], width, kind: Self::SQUARE_ENDS, colour, clip: 0.0 }
    }

    pub fn round_line(from: [f32; 2], to: [f32; 2], width: f32, colour: [f32; 4]) -> Self {
        Primitive { kind: Self::ROUND_ENDS, ..Self::line(from, to, width, colour) }
    }

    pub fn triangle(points: [[f32; 2]; 3], colour: [f32; 4]) -> Self {
        Primitive { points, width: 0.0, kind: Self::TRIANGLE, colour, clip: 0.0 }
    }

    pub fn is_triangle(&self) -> bool {
        self.kind == Self::TRIANGLE
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
/// painted with one blend, and within one clip set that isn't convex, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub len: usize,
    pub blend: Blend,
    /// A clip set they stay within that the shader can't test, drawn through
    /// the stencil instead: an index into `Clips::sets`. `None` for no clip,
    /// or a convex one, which each primitive carries itself.
    pub clip: Option<usize>,
}

/// The areas shapes are clipped to. A clip set is a list of clip shapes, and
/// what's drawn within it shows only where all of them overlap.
#[derive(Debug, Default)]
pub struct Clips {
    /// The corners of every clip shape's triangles, three to a triangle.
    pub vertices: Vec<[f32; 2]>,
    /// Each clip shape's range of `vertices`.
    pub shapes: Vec<Range<usize>>,
    /// Each clip set, as indices into `shapes`.
    pub sets: Vec<Vec<usize>>,
    /// Each clip set's half-planes, where all its shapes are convex: inside
    /// the set is inside every one of them. `None` where a shape isn't.
    pub planes: Vec<Option<Vec<Plane>>>,
    shape_planes: Vec<Option<Vec<Plane>>>,
    shape_ids: HashMap<Vec<u32>, usize>,
    set_ids: HashMap<Vec<usize>, usize>,
}

impl Clips {
    /// The clip set of set `within`'s shapes (none for the whole page) and a
    /// shape covering `triangles`. Clip shapes and sets that are the same as
    /// earlier ones keep the earlier ones' indices, so shapes drawn within
    /// the same clip run on together.
    pub fn intersect(&mut self, within: Option<usize>, triangles: &[[[f32; 2]; 3]]) -> usize {
        let key: Vec<u32> = triangles.iter().flatten().flatten().map(|v| v.to_bits()).collect();
        let shape = match self.shape_ids.get(&key) {
            Some(&shape) => shape,
            None => {
                let start = self.vertices.len();
                self.vertices.extend(triangles.iter().flatten().copied());
                self.shapes.push(start..self.vertices.len());
                self.shape_planes.push(convex_planes(triangles));
                self.shape_ids.insert(key, self.shapes.len() - 1);
                self.shapes.len() - 1
            }
        };
        let mut set = within.map_or_else(Vec::new, |within| self.sets[within].clone());
        if !set.contains(&shape) {
            set.push(shape);
        }
        match self.set_ids.get(&set) {
            Some(&id) => id,
            None => {
                let planes = set.iter().map(|&shape| self.shape_planes[shape].clone()).collect::<Option<Vec<_>>>().map(|p| p.concat());
                self.planes.push(planes);
                self.sets.push(set.clone());
                self.set_ids.insert(set, self.sets.len() - 1);
                self.sets.len() - 1
            }
        }
    }

    /// Whether clip set `set` is tested by the shader, rather than drawn
    /// through the stencil.
    pub fn is_convex(&self, set: usize) -> bool {
        self.planes[set].is_some()
    }
}

/// What a page's annotations paint, the clips they stay within, and what
/// couldn't be drawn.
#[derive(Debug, Default)]
pub struct Shapes {
    /// Everything to draw, in the order it's painted.
    pub primitives: Vec<Primitive>,
    /// The primitives split where their blend or clip changes, in order,
    /// covering all of them.
    pub runs: Vec<Run>,
    pub lines: usize,
    pub triangles: usize,
    pub clips: Clips,
    /// What was left out or drawn only in part, by kind -- text, images and
    /// the like -- and how many times.
    pub not_drawn: BTreeMap<&'static str, usize>,
}

impl Shapes {
    /// Adds a shape drawn within clip set `clip`, if any. A convex clip goes
    /// on the shape, for the shader; any other starts a new run when it
    /// differs from the last one's, as a different blend does.
    pub fn push(&mut self, mut primitive: Primitive, blend: Blend, clip: Option<usize>) {
        let stencil = clip.filter(|&set| !self.clips.is_convex(set));
        primitive.clip = clip.filter(|&set| self.clips.is_convex(set)).map_or(0.0, |set| set as f32 + 1.0);
        match self.runs.last_mut() {
            Some(run) if run.blend == blend && run.clip == stencil => run.len += 1,
            _ => self.runs.push(Run { start: self.primitives.len(), len: 1, blend, clip: stencil }),
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

    const SQUARE: [[[f32; 2]; 3]; 2] = [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], [[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]]];

    /// An L: a 2 x 4 upright and a 2 x 2 foot beside it.
    const L: [[[f32; 2]; 3]; 4] = [[[0.0, 0.0], [2.0, 0.0], [2.0, 4.0]], [[0.0, 0.0], [2.0, 4.0], [0.0, 4.0]], [[2.0, 0.0], [4.0, 0.0], [4.0, 2.0]], [[2.0, 0.0], [4.0, 2.0], [2.0, 2.0]]];

    #[test]
    fn a_primitive_is_thirteen_floats() {
        assert_eq!(std::mem::size_of::<Primitive>(), 52);
        let mut line = Primitive::line([1.0, 2.0], [3.0, 4.0], 0.5, [1.0, 0.0, 0.0, 1.0]);
        line.clip = 3.0;
        let bytes: &[u8] = bytemuck::bytes_of(&line);
        assert_eq!(&bytes[24..28], &0.5_f32.to_ne_bytes(), "width after the three points");
        assert_eq!(&bytes[48..52], &3.0_f32.to_ne_bytes(), "the clip last");
        assert!(!line.is_triangle() && Primitive::triangle([[0.0; 2]; 3], [0.0; 4]).is_triangle());
        assert!(!Primitive::round_line([0.0; 2], [1.0; 2], 1.0, [0.0; 4]).is_triangle());
    }

    #[test]
    fn a_convex_clip_goes_on_the_shape_and_only_other_clips_and_blends_split_runs() {
        let mut shapes = Shapes::default();
        let square = shapes.clips.intersect(None, &SQUARE);
        let l = shapes.clips.intersect(None, &L);
        assert!(shapes.clips.is_convex(square) && !shapes.clips.is_convex(l));
        let line = Primitive::line([0.0; 2], [1.0; 2], 0.0, [0.0; 4]);
        for (blend, clip) in [(Blend::Normal, None), (Blend::Normal, Some(square)), (Blend::Multiply, Some(square)), (Blend::Multiply, Some(l))] {
            shapes.push(line, blend, clip);
        }
        let runs: Vec<(usize, usize, Blend, Option<usize>)> = shapes.runs.iter().map(|r| (r.start, r.len, r.blend, r.clip)).collect();
        assert_eq!(runs, [(0, 2, Blend::Normal, None), (2, 1, Blend::Multiply, None), (3, 1, Blend::Multiply, Some(l))]);
        let clips: Vec<f32> = shapes.primitives.iter().map(|p| p.clip).collect();
        assert_eq!(clips, [0.0, square as f32 + 1.0, square as f32 + 1.0, 0.0]);
        assert_eq!((shapes.lines, shapes.triangles), (4, 0));
    }

    #[test]
    fn the_same_clip_shapes_and_sets_are_kept_once() {
        let mut clips = Clips::default();
        let outer = clips.intersect(None, &SQUARE);
        assert_eq!(clips.intersect(None, &SQUARE), outer, "the same square again is the same set");
        let shifted = SQUARE.map(|t| t.map(|[x, y]| [x + 0.5, y]));
        let inner = clips.intersect(Some(outer), &shifted);
        assert_ne!(inner, outer);
        assert_eq!(clips.sets[inner], [0, 1], "within the first square and the second");
        assert_eq!(clips.intersect(Some(inner), &SQUARE), inner, "a shape already in the set adds nothing");
        assert_eq!((clips.shapes.len(), clips.vertices.len()), (2, 12));
        assert_eq!(clips.planes[inner].as_ref().map(Vec::len), Some(8), "both squares' edges");
    }
}
