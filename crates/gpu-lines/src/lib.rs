//! Drawing a PDF page's annotations on the GPU.
//!
//! pdfium rasterises every path on the CPU, at a few microseconds each, so a
//! page of Bluebeam stamps or CAD linework -- about a million line segments --
//! takes the best part of a second to draw at any zoom. A GPU draws the same
//! shapes as instances of one quad: the page is uploaded once, and every pan
//! or zoom after that only changes a transform.
//!
//! This crate is a prototype, kept apart from the app until it proves itself:
//!
//! - `annotation_shapes` finds the annotations shown on a page -- by their
//!   flags and layers -- and places each appearance as PDF says, then an
//!   `Interpreter` reads its content stream (via `pdf-content`), following
//!   the graphics state: transforms, colours, transparency and blending,
//!   nested forms, layers inside the stream, text (`text`), its glyphs
//!   outlined from the fonts embedded (`font`), and images, decoded (`image`)
//!   and packed into texture pages (`atlas`).
//! - What it paints becomes `Shapes`, in the page's painting order: straight
//!   pieces of stroked lines, curves flattened, dashed, with triangles for
//!   their caps and joins (`stroke`), and triangles covering filled areas by
//!   their fill rule (`geometry`). What it can't draw yet is counted.
//! - `Renderer` draws the shapes with OpenGL (glow), a run of the same blend
//!   at a time, lines anti-aliased, hairlines one pixel wide at any zoom. Each
//!   page's shapes are uploaded apart, as an `Uploaded`, so any number of
//!   pages share its shaders.
//!
//! `examples/viewer` puts them on screen next to pdfium's own drawing.

pub use pdf_content::lopdf;

mod atlas;
mod colour;
mod font;
mod function;
mod geometry;
mod image;
mod interpret;
mod page;
mod pdf;
mod render;
mod shapes;
mod stroke;
#[cfg(test)]
mod test_font;
mod text;

pub use atlas::ATLAS_SIZE;
pub use geometry::Matrix;
pub use interpret::{Interpreter, MOST_IMAGE_DENSITY};
pub use page::{annotation_shapes, page_shapes, page_shapes_unless, STOPPED};
pub use render::{Mark, Renderer, Upload, Uploaded};
pub use shapes::{Blend, Primitive, Run, Shape, Shapes, Style};
