//! Measurement markups as data, and the maths that turns them into
//! quantities.
//!
//! Geometry plus scale is the source of truth: every quantity is worked out
//! from vertices in PDF points and the scale the markup resolves to, and any
//! number stored in a file is only a cache of that. All quantity maths is in
//! `f64`, and quantities are always metres, square metres and cubic metres;
//! other units exist only when formatting and parsing (`units`).
//!
//! This crate knows nothing of pdfium, lopdf or egui, so it can be tested on
//! its own. Reading and writing PDFs, drawing and tools build on it.
//!
//! - `geom`: points, boxes, shoelace areas, simple-polygon checks
//! - `units`: length, area and volume units; formatting and parsing
//! - `scale`: scales and calibration, sheet sizes, verification
//! - `viewport`: per-page viewports and resolving a markup's scale
//! - `markup`: the markup, its kinds, geometry, style and metadata
//! - `quantity`: quantities, the `Measure` trait, totals
//! - `layers`: named layers and what stacks above what
//! - `hit`: picking vertices, edges and bodies
//! - `store`: every markup in a document, measured and spatially indexed
//! - `transform`: user space to screen, with /Rotate and CropBox
//! - `affine`: turning and stretching markups
//! - `hash`: the /KPDF /GeomHash

pub mod affine;
pub mod geom;
pub mod hash;
pub mod hit;
pub mod id;
pub mod layers;
pub mod markup;
pub mod quantity;
pub mod scale;
pub mod snap;
pub mod spatial;
pub mod store;
pub mod text;
pub mod transform;
pub mod units;
pub mod viewport;

pub use affine::{box_mapped, Affine};
pub use geom::{Pt, Rect};
pub use hit::Hit;
pub use id::{LayerId, MarkupId, PageIndex, ScaleId, ViewportId};
pub use layers::{next_z, restacked, Layer, LayerStack, Place, Restack};
pub use markup::{clip_resized, ClipArt, Extras, FillPattern, Geometry, LabelFont, Markup, MarkupKind, MarkupMeta, Slope, Style};
pub use quantity::{quantities, Measure, Quantities, QuantityError, Totals};
pub use scale::Scale;
pub use snap::{Snap, SnapIndex, SnapKind};
pub use store::MarkupStore;
pub use text::{box_resized, callout_start, Frame, HAlign, Paragraph, Run, RunFormat, TextBox, VAlign};
pub use transform::PageTransform;
pub use units::{DisplayUnits, Precision};
pub use viewport::{ScaleRef, ScaleStore, Viewport};
