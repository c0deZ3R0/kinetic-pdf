//! Measurement markups and scales to and from PDF.
//!
//! Each markup is written as a standard ISO 32000 measurement annotation --
//! /Line, /PolyLine or /Polygon with a dimension /IT, /Vertices or /L, and a
//! /Measure -- so other viewers show and measure it, plus one versioned /KPDF
//! dictionary for what the standard has no place for. A page's scales are a
//! /VP array. Each scale's /Measure is written once, as an indirect object
//! every viewport and markup using it refers to.
//!
//! Written with lopdf as incremental updates, never touching the original
//! bytes. lopdf holds real numbers as 32 bits; see `docs/design-log.md`.
//!
//! - `write`: appending viewports and markups
//! - `read`: reading them back
//! - `measure`: a scale as a /Measure dictionary and back
//! - `appearance`: the /AP stream other viewers draw
//! - `values`: numbers, text, dates and kept values

pub mod appearance;
pub mod measure;
pub mod read;
pub mod values;
pub mod write;

use std::fmt;

pub use read::{read, Read};
pub use write::append;

use markup_model::PageIndex;
use pdf_content::lopdf;

#[derive(Debug)]
pub enum Error {
    Pdf(lopdf::Error),
    NoPage(PageIndex),
    /// Something valid that this doesn't handle yet.
    Unsupported(String),
    /// Something malformed.
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Pdf(e) => write!(f, "{e}"),
            Error::NoPage(p) => write!(f, "there's no page {}", p + 1),
            Error::Unsupported(what) => write!(f, "{what} isn't supported yet"),
            Error::Invalid(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<lopdf::Error> for Error {
    fn from(e: lopdf::Error) -> Error {
        Error::Pdf(e)
    }
}
