//! Text boxes set in the fonts installed on this machine.
//!
//! - `fonts`: what fonts there are, and a face's glyphs and metrics
//! - `layout`: a box's text broken into lines, aligned and shrunk to fit
//!
//! The screen and the PDF both draw what `layout` gives, so a box looks the
//! same in the app as in any other viewer of the file.

pub mod fonts;
pub mod layout;

pub use fonts::{catalogue, Catalogue, Face, FontEntry};
pub use layout::{layout, Laid, Line, Word};
