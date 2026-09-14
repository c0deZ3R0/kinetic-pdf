//! Drawing a PDF page's linework on the GPU.
//!
//! pdfium rasterises every path on the CPU, at a few microseconds each, so a
//! page of Bluebeam stamps or CAD linework -- about a million line segments --
//! takes the best part of a second to draw at any zoom. A GPU draws the same
//! lines as instances of one quad: the page is uploaded once, and every pan
//! or zoom after that only changes a transform.
//!
//! This crate is a prototype, kept apart from the app until it proves itself:
//!
//! - `extract` reads a page's paths out of pdfium, through nested forms, into
//!   `Primitive`s in page space, in the order the page paints them: straight
//!   pieces of stroked lines, curves flattened, and triangles covering filled
//!   areas by their fill rule. Text, images and shadings are counted but left
//!   to pdfium, and clip paths, dash patterns and transparency are counted
//!   but ignored.
//! - `Renderer` draws them with OpenGL (glow) in one instanced draw call,
//!   lines anti-aliased, hairlines one pixel wide at any zoom.
//!
//! `examples/viewer.rs` puts both on screen next to pdfium's own drawing.

mod extract;
mod render;

pub use extract::{extract, Blend, Extracted, Primitive, Run};
pub use render::Renderer;
