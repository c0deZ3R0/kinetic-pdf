//! The app as a library, so the benchmark (src/bin/bench.rs) runs exactly the
//! engine code the app does rather than a copy of it. main.rs is only the
//! window setup.

pub mod annots;
pub mod app;
pub mod arrange;
pub mod cache;
pub mod document;
pub mod domain;
pub mod helper;
pub mod layering;
pub mod markup;
pub mod merge;
pub mod model;
pub mod overlay;
pub mod pool;
pub mod protocol;
pub mod raster;
pub mod printing;
pub mod selection;
pub mod session;
pub mod update;
pub mod worker;
