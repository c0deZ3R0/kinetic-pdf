//! The app as a library, so the benchmark (src/bin/bench.rs) runs exactly the
//! engine code the app does rather than a copy of it. main.rs is only the
//! window setup.

pub mod annots;
pub mod app;
pub mod cache;
pub mod helper;
pub mod markup;
pub mod merge;
pub mod model;
pub mod pool;
pub mod selection;
pub mod worker;
