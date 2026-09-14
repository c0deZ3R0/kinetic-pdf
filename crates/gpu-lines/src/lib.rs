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
//!   nested forms, and layers inside the stream.
//! - What it paints becomes `Shapes`, in the page's painting order: straight
//!   pieces of stroked lines, curves flattened, dashed, with triangles for
//!   their caps and joins (`stroke`), and triangles covering filled areas by
//!   their fill rule (`geometry`). What it can't draw yet is counted.
//! - `Renderer` draws the shapes with OpenGL (glow), a run of the same blend
//!   at a time, lines anti-aliased, hairlines one pixel wide at any zoom.
//!
//! `examples/viewer` puts them on screen next to pdfium's own drawing.

pub use pdf_content::lopdf;

mod geometry;
mod interpret;
mod page;
mod pdf;
mod render;
mod shapes;
mod stroke;

pub use geometry::Matrix;
pub use interpret::Interpreter;
pub use page::annotation_shapes;
pub use render::Renderer;
pub use shapes::{Blend, Primitive, Run, Shapes};
