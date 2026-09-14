//! Reading what a PDF draws, for drawing it ourselves: content streams and
//! their operators (`lexer`), following references between objects
//! (`objects`), and which optional content -- layers -- is visible (`layers`).
//!
//! Shared by the app, which rewrites a copy of a document to draw faster
//! (`merge.rs`), and the GPU prototype (`gpu-lines`), which draws annotation
//! appearances from their content streams.

pub use lopdf;

pub mod layers;
pub mod lexer;
pub mod objects;

#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;
