//! The shapes the GPU draws, in the order they're painted, and the clips they
//! stay within.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use crate::atlas::{Atlas, Placed};
use crate::geometry::{convex_planes, Plane};

// SAFETY, for the runs of values `to_bytes` copies: `Plane` and the vertices
// are arrays of f32, which bytemuck already knows.

/// Why something can't be drawn, as counted in `Shapes::not_drawn`.
pub(crate) type Unsupported = &'static str;

/// One shape to draw, in page space: points, with the origin at the bottom
/// left of the page as it's drawn. Either a straight piece of a stroked line,
/// from the first point to the second, or a filled triangle of all three. A
/// line with round ends reaches half its width past them in a half disc, so
/// such lines meeting make a round join, and one going nowhere is a dot. An
/// image fills the parallelogram from its first point, the bottom left, to
/// the second along its bottom and the third up its left side.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Primitive {
    pub points: [[f32; 2]; 3],
    /// A line's width in points, or 0 for a hairline, which is one pixel wide
    /// at any zoom. An image's alpha. Unused for a triangle.
    pub width: f32,
    /// 0 for a line with square ends, 2 for one with round ends, 1 for a
    /// triangle, and from 3 up an image, on atlas page `kind - 3`.
    pub kind: f32,
    /// Red, green, blue and alpha, 0 to 1, not premultiplied. For an image,
    /// its place on its atlas page: left, top, right and bottom.
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
    const IMAGE: f32 = 3.0;

    pub fn line(from: [f32; 2], to: [f32; 2], width: f32, colour: [f32; 4]) -> Self {
        Primitive { points: [from, to, to], width, kind: Self::SQUARE_ENDS, colour, clip: 0.0 }
    }

    pub fn round_line(from: [f32; 2], to: [f32; 2], width: f32, colour: [f32; 4]) -> Self {
        Primitive { kind: Self::ROUND_ENDS, ..Self::line(from, to, width, colour) }
    }

    pub fn triangle(points: [[f32; 2]; 3], colour: [f32; 4]) -> Self {
        Primitive { points, width: 0.0, kind: Self::TRIANGLE, colour, clip: 0.0 }
    }

    /// An image `placed` in the atlas, filling the parallelogram with corners
    /// bottom left, bottom right and top left, faded to `alpha`.
    pub fn image(corners: [[f32; 2]; 3], placed: Placed, alpha: f32) -> Self {
        Primitive { points: corners, width: alpha, kind: Self::IMAGE + placed.page as f32, colour: placed.uv, clip: 0.0 }
    }

    pub fn is_triangle(&self) -> bool {
        self.kind == Self::TRIANGLE
    }

    pub fn is_image(&self) -> bool {
        self.kind >= Self::IMAGE
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
    pub images: usize,
    pub clips: Clips,
    /// The images drawn, each once however often it's drawn.
    pub atlas: Atlas,
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
        if primitive.is_image() {
            self.images += 1;
        } else if primitive.is_triangle() {
            self.triangles += 1;
        } else {
            self.lines += 1;
        }
        self.primitives.push(primitive);
    }

    /// The bytes the shapes take on the GPU: the shapes themselves, the clip
    /// shapes, and the used rows of the atlas pages.
    pub fn bytes(&self) -> usize {
        self.primitives.len() * std::mem::size_of::<Primitive>()
            + std::mem::size_of_val(self.clips.vertices.as_slice())
            + self.atlas.pages.len() * crate::atlas::ATLAS_SIZE as usize * self.atlas.height() as usize * 4
    }

    /// Counts one more of something that wasn't drawn.
    pub fn not_drawn(&mut self, what: &'static str) {
        *self.not_drawn.entry(what).or_default() += 1;
    }

    /// These shapes as bytes, to keep and read back with `from_bytes`: reading
    /// a dense page takes a fifth of a second, and what it comes to is the
    /// same every time. What couldn't be drawn isn't written, since only pages
    /// the GPU draws entirely are worth keeping.
    ///
    /// The format is this machine's own: little-endian, and the primitives and
    /// clip vertices copied as they sit in memory. Its magic carries a version,
    /// so a later layout simply misses rather than misreads.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.primitives.len() * std::mem::size_of::<Primitive>() + 1024);
        out.extend_from_slice(MAGIC);
        for count in [self.lines, self.triangles, self.images] {
            out.extend_from_slice(&(count as u64).to_le_bytes());
        }
        block(&mut out, bytemuck::cast_slice(&self.primitives));
        out.extend_from_slice(&(self.runs.len() as u64).to_le_bytes());
        for run in &self.runs {
            out.extend_from_slice(&(run.start as u64).to_le_bytes());
            out.extend_from_slice(&(run.len as u64).to_le_bytes());
            out.push(u8::from(run.blend == Blend::Multiply));
            out.extend_from_slice(&(run.clip.map_or(u64::MAX, |set| set as u64)).to_le_bytes());
        }
        block(&mut out, bytemuck::cast_slice(&self.clips.vertices));
        out.extend_from_slice(&(self.clips.shapes.len() as u64).to_le_bytes());
        for shape in &self.clips.shapes {
            out.extend_from_slice(&(shape.start as u64).to_le_bytes());
            out.extend_from_slice(&(shape.end as u64).to_le_bytes());
        }
        out.extend_from_slice(&(self.clips.sets.len() as u64).to_le_bytes());
        for set in &self.clips.sets {
            out.extend_from_slice(&(set.len() as u64).to_le_bytes());
            out.extend(set.iter().flat_map(|shape| (*shape as u64).to_le_bytes()));
        }
        out.extend_from_slice(&(self.clips.planes.len() as u64).to_le_bytes());
        for planes in &self.clips.planes {
            match planes {
                None => out.push(0),
                Some(planes) => {
                    out.push(1);
                    block(&mut out, bytemuck::cast_slice(planes));
                }
            }
        }
        out.extend_from_slice(&self.atlas.height().to_le_bytes());
        out.extend_from_slice(&(self.atlas.pages.len() as u64).to_le_bytes());
        let used = self.atlas.height() as usize * crate::atlas::ATLAS_SIZE as usize * 4;
        for page in &self.atlas.pages {
            block(&mut out, &page[..used.min(page.len())]);
        }
        out
    }

    /// Shapes written by `to_bytes`, or `None` if the bytes aren't those of
    /// this build: a damaged or older file simply reads as nothing kept.
    pub fn from_bytes(bytes: &[u8]) -> Option<Shapes> {
        let mut read = Reader { bytes, at: MAGIC.len() };
        if !bytes.starts_with(MAGIC) {
            return None;
        }
        let [lines, triangles, images] = [read.count()?, read.count()?, read.count()?];
        let primitives = read.values::<Primitive>()?;
        let mut runs = Vec::with_capacity(read.count()?.min(primitives.len() + 1));
        for _ in 0..runs.capacity() {
            let (start, len) = (read.count()?, read.count()?);
            let blend = if read.byte()? == 0 { Blend::Normal } else { Blend::Multiply };
            let clip = read.u64()?;
            runs.push(Run { start, len, blend, clip: (clip != u64::MAX).then_some(clip as usize) });
        }
        let vertices = read.values::<[f32; 2]>()?;
        let mut shapes = Vec::with_capacity(read.count()?.min(vertices.len() + 1));
        for _ in 0..shapes.capacity() {
            let (start, end) = (read.count()?, read.count()?);
            if start > end || end > vertices.len() {
                return None;
            }
            shapes.push(start..end);
        }
        let mut sets: Vec<Vec<usize>> = Vec::with_capacity(read.count()?.min(shapes.len() + 1));
        for _ in 0..sets.capacity() {
            let mut set = Vec::with_capacity(read.count()?.min(shapes.len()));
            for _ in 0..set.capacity() {
                set.push(read.count().filter(|shape| *shape < shapes.len())?);
            }
            sets.push(set);
        }
        let mut planes = Vec::with_capacity(read.count()?.min(sets.len() + 1));
        for _ in 0..planes.capacity() {
            planes.push(match read.byte()? {
                0 => None,
                _ => Some(read.values::<Plane>()?),
            });
        }
        let height = read.u32()?;
        let mut pages = Vec::with_capacity(read.count()?.min(MOST_ATLAS_PAGES));
        for _ in 0..pages.capacity() {
            pages.push(read.values::<u8>()?);
        }
        let clips = Clips { vertices, shapes, sets, planes, ..Clips::default() };
        Some(Shapes { primitives, runs, lines, triangles, images, clips, atlas: Atlas::restored(pages, height), not_drawn: BTreeMap::new() })
    }
}

/// What `Shapes::to_bytes` writes in front of everything else. The last two
/// figures go up whenever the layout changes, so what an older build wrote is
/// read as nothing kept rather than as rubbish.
const MAGIC: &[u8; 8] = b"GPUSHP01";

/// Atlas pages a file may claim, against a damaged one asking for memory by
/// the gigabyte. A page of images is four pages of atlas; a sheet of them
/// fifteen.
const MOST_ATLAS_PAGES: usize = 256;

/// Writes a run of values: how many bytes, then the bytes.
fn block(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Reads what `Shapes::to_bytes` wrote, refusing anything that doesn't fit
/// what's left.
struct Reader<'b> {
    bytes: &'b [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, length: usize) -> Option<&[u8]> {
        let taken = self.bytes.get(self.at..self.at.checked_add(length)?)?;
        self.at += length;
        Some(taken)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    /// A count, which can't be more than the bytes left could hold.
    fn count(&mut self) -> Option<usize> {
        let count = usize::try_from(self.u64()?).ok()?;
        (count <= self.bytes.len() - self.at.min(self.bytes.len()) + 1).then_some(count)
    }

    /// A run of values written by `block`, copied into place: the vector's own
    /// memory is aligned for them, which the file's bytes needn't be.
    fn values<T: bytemuck::Pod>(&mut self) -> Option<Vec<T>> {
        let length = usize::try_from(self.u64()?).ok()?;
        let size = std::mem::size_of::<T>();
        if length % size != 0 {
            return None;
        }
        let bytes = self.take(length)?;
        let mut values: Vec<T> = vec![T::zeroed(); length / size];
        bytemuck::cast_slice_mut::<T, u8>(&mut values).copy_from_slice(bytes);
        Some(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: [[[f32; 2]; 3]; 2] = [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], [[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]]];

    /// An L: a 2 x 4 upright and a 2 x 2 foot beside it.
    const L: [[[f32; 2]; 3]; 4] = [[[0.0, 0.0], [2.0, 0.0], [2.0, 4.0]], [[0.0, 0.0], [2.0, 4.0], [0.0, 4.0]], [[2.0, 0.0], [4.0, 0.0], [4.0, 2.0]], [[2.0, 0.0], [4.0, 2.0], [2.0, 2.0]]];

    #[test]
    fn shapes_read_back_as_they_were_written() {
        let mut shapes = Shapes::default();
        let square = shapes.clips.intersect(None, &SQUARE);
        let l = shapes.clips.intersect(None, &L);
        shapes.push(Primitive::line([0.0, 1.0], [2.0, 3.0], 0.5, [1.0, 0.0, 0.0, 0.25]), Blend::Normal, None);
        shapes.push(Primitive::triangle([[0.0; 2], [1.0, 0.0], [1.0, 1.0]], [0.0, 1.0, 0.0, 1.0]), Blend::Multiply, Some(square));
        shapes.push(Primitive::round_line([4.0, 5.0], [6.0, 7.0], 1.0, [0.0; 4]), Blend::Normal, Some(l));
        shapes.atlas.add(&crate::image::Bitmap { width: 2, height: 2, pixels: (0..16).collect() });

        let read = Shapes::from_bytes(&shapes.to_bytes()).expect("they read back");
        assert_eq!(read.primitives, shapes.primitives);
        assert_eq!(read.runs, shapes.runs);
        assert_eq!((read.lines, read.triangles, read.images), (shapes.lines, shapes.triangles, shapes.images));
        assert_eq!(read.clips.vertices, shapes.clips.vertices);
        assert_eq!(read.clips.shapes, shapes.clips.shapes);
        assert_eq!(read.clips.sets, shapes.clips.sets);
        assert_eq!(read.clips.planes, shapes.clips.planes);
        assert!(read.clips.is_convex(square) && !read.clips.is_convex(l), "convex clips still go to the shader");
        assert_eq!(read.atlas.height(), shapes.atlas.height());
        let used = read.atlas.height() as usize * crate::atlas::ATLAS_SIZE as usize * 4;
        assert_eq!(read.atlas.pages[0][..used], shapes.atlas.pages[0][..used], "the rows of the atlas in use");
        assert_eq!(read.bytes(), shapes.bytes());
    }

    #[test]
    fn bytes_that_arent_shapes_read_as_nothing() {
        let written = Shapes::default().to_bytes();
        assert!(Shapes::from_bytes(&written).is_some());
        assert!(Shapes::from_bytes(b"not shapes at all").is_none(), "another kind of file");
        assert!(Shapes::from_bytes(&written[..written.len() - 4]).is_none(), "cut short");
        let mut older = written.clone();
        older[7] = b'0';
        assert!(Shapes::from_bytes(&older).is_none(), "written by another build");
        for cut in (8..written.len()).step_by(3) {
            // Whatever is damaged, it never panics or asks for wild memory.
            let mut damaged = written.clone();
            damaged[cut] = 0xFF;
            let _ = Shapes::from_bytes(&damaged);
        }
    }

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
        let image = Primitive::image([[0.0; 2]; 3], Placed { page: 2, uv: [0.1, 0.2, 0.3, 0.4] }, 0.5);
        assert!(image.is_image() && !image.is_triangle());
        assert_eq!((image.kind, image.width, image.colour), (5.0, 0.5, [0.1, 0.2, 0.3, 0.4]), "page 2, faded by half, where it is on the page");
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
