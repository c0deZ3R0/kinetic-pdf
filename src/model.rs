//! Compatibility exports for existing library consumers.
//! New code imports document data from `domain`, messages from `protocol`,
//! and page image utilities from `raster`.

pub use crate::domain::*;
pub use crate::protocol::{Reply, Request};
pub use crate::raster::*;
