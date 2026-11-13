//! The app as a library, so the benchmark (src/bin/bench/main.rs) runs exactly the
//! engine code the app does rather than a copy of it. main.rs is only the
//! window setup.
//!
//! PATH: src/lib.rs
//! CONSUMED BY: main, benchmark and integration tests; exports engine/output-safety APIs.
//! Deferred signature inventory remains private to the worker.

pub mod annots;
pub mod app;
pub mod arrange;
pub mod cache;
pub mod document;
pub mod domain;
mod erasure;
pub mod helper;
pub mod layering;
pub mod markup;
pub mod merge;
pub mod model;
pub mod overlay;
pub mod output_safety;
pub mod pool;
pub mod pins;
pub mod protocol;
pub mod raster;
pub mod printing;
pub mod selection;
pub mod session;
pub mod signing;
mod signature_discovery;
pub mod update;
pub mod worker;
