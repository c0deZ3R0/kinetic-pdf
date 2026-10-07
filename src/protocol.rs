//! Messages exchanged by the UI, worker and rendering services.
//! Domain values live in `domain`; only rendering replies carry UI textures.

use std::path::PathBuf;
use eframe::egui::TextureHandle;

use crate::domain::{Changes, Highlight, Markup, Measurements, PageGeometry, SearchHit, TextChar};
use crate::raster::Tile;

/// UI thread -> worker. `generation` ties a request to one opened document, so
/// work for a file that has since been replaced is dropped.
pub enum Request {
    Open { generation: u64, path: PathBuf },
    Text { generation: u64, page: usize },
    /// Explicit extraction does not get dropped when the page is off screen.
    ExtractText { generation: u64, page: usize },
    /// `scale` is output pixels per PDF point.
    Render { generation: u64, page: usize, scale: f32 },
    /// Part of a page, for zooming in past what a whole-page image holds: the
    /// page as drawn `full` pixels in size (as displayed), of which only
    /// `region` -- x, y, width, height in those pixels -- is rendered.
    RenderRegion { generation: u64, page: usize, full: [u32; 2], region: [u32; 4] },
    /// Drawing ahead of a zoom the user may be about to make: the whole page
    /// at `scale`, done only when nothing else is waiting. The reply is a
    /// `Rendered` like any other, or `RenderSkipped`. Without render helpers it
    /// isn't done at all.
    PredictPage { generation: u64, page: usize, scale: f32 },
    /// Likewise part of a page, as `RenderRegion`; the reply is a
    /// `RenderedRegion`.
    PredictRegion { generation: u64, page: usize, full: [u32; 2], region: [u32; 4] },
    /// `arrangement` is the order the sheets are to be written in, when the
    /// user has changed it: the pages reordered, taken out, duplicated,
    /// turned or blank sheets put in. `None` leaves the page tree alone,
    /// which is every save of a document nobody has rearranged.
    /// `new_pages` are the blank pages put in since the file was opened, by
    /// size: they go after the file's own pages, numbered on from them, before
    /// anything is written onto them (`arrange::append_pages`).
    Save { generation: u64, changes: Changes, arrangement: Option<Vec<crate::arrange::Sheet>>, new_pages: Vec<[f32; 2]> },
    SaveAs { generation: u64, path: PathBuf, changes: Changes, arrangement: Option<Vec<crate::arrange::Sheet>>, new_pages: Vec<[f32; 2]> },
    Print { generation: u64, snapshot: std::sync::Arc<crate::printing::Snapshot>, job: crate::printing::Job },
    PrintPreview { generation: u64, snapshot: std::sync::Arc<crate::printing::Snapshot>, configured: crate::printing::Configured, pages: Vec<usize>, serial: u64 },
    EndPrintPreview { id: u64 },
    /// Reads the document's scales and measurements. That needs a pass over
    /// the whole file with lopdf, since pdfium can't see /VP or /Measure, so
    /// it's only done when something asks: opening the scale tool, say.
    ReadMeasurements { generation: u64 },
    /// Replaces any search in progress. A blank query just stops it.
    Search { generation: u64, id: u64, query: String, erasures: Vec<crate::domain::Erasure> },
}

/// Worker -> UI thread.
pub enum Reply {
    /// pdfium could not be loaded, so nothing else will work.
    Fatal(String),
    /// `page_sizes` are as displayed: rotated, in points. `snapshot` owns
    /// the exact bytes and fingerprint shared by all readers and the page cache.
    /// `page_labels` are the sheet names the file gives its pages, if any.
    Opened { generation: u64, path: PathBuf, snapshot: crate::document::Snapshot, page_sizes: Vec<[f32; 2]>, page_labels: Vec<Option<String>> },
    OpenFailed { generation: u64, error: String },
    /// Highlights arrive a page at a time: a page's own just before its first
    /// render, the rest in the background. Reading them all up front loads
    /// every page, which kept a long document blank for a noticeable moment.
    /// Each page read also reports its geometry, which comes from the same
    /// page load. `done` once every page has been read.
    /// Markups come the same way.
    Highlights { generation: u64, highlights: Vec<Highlight>, markups: Vec<Markup>, geometry: Vec<(usize, PageGeometry)>, done: bool },
    Text { generation: u64, page: usize, chars: Vec<TextChar> },
    /// The page scrolled out of view before its text was read, which on a
    /// dense page would have meant loading it for nothing.
    TextSkipped { generation: u64, page: usize },
    /// A page drawn and already made into a texture, on the worker thread.
    /// A dense page arrives part-drawn a few times first (`complete` false),
    /// so it shows as it draws; the complete image always follows unless the
    /// render is abandoned, which sends `RenderSkipped`.
    /// `slow` if drawing it took long enough that it's worth avoiding again
    /// (see `cache::SLOW_MS`); a page read back from the cache is slow too.
    /// `annotations` if pdfium drew the page's annotations in it, as it does
    /// unless the app draws them itself (`Wanted::without_annotations`).
    Rendered { generation: u64, page: usize, scale: f32, texture: TextureHandle, complete: bool, slow: bool, annotations: bool },
    /// A `RenderRegion`, cut into squares (see `TILE`), each made into a
    /// texture. Empty if it went stale or failed; the whole-page image still
    /// stands in, so neither is an error. `annotations` as for `Rendered`.
    RenderedRegion { generation: u64, page: usize, full: [u32; 2], region: [u32; 4], annotations: bool, tiles: Vec<Tile> },
    /// The page scrolled out of view before the worker got to it.
    RenderSkipped { generation: u64, page: usize },
    /// pdfium could not draw the page; the UI stops asking for it.
    RenderFailed { generation: u64, page: usize, error: String },
    /// `pages` are the pages the save changed, and `highlights` and `markups`
    /// everything now on them. A save doesn't move annotations on any other
    /// page, so those stay as they were. `redrawn` are the pages whose drawing
    /// changed, as markups were added or removed: markups just written keep
    /// their points, so they can show until the page is drawn again.
    /// `snapshot` is the committed revision for subsequent reads and cache keys.
    Saved { generation: u64, snapshot: crate::document::Snapshot, pages: Vec<usize>, highlights: Vec<Highlight>, markups: Vec<Markup>, redrawn: Vec<usize> },
    SaveFailed { generation: u64, error: String },
    SaveTarget { generation: u64, path: PathBuf },
    Printed { generation: u64, result: Result<bool, String> },
    PrintProgress { generation: u64, completed: usize, total: usize },
    PrintPreview { generation: u64, id: u64, serial: u64, result: Result<(TextureHandle, Vec<crate::printing::Placement>), String> },
    /// The document's scales and measurements, once read.
    Measured { generation: u64, file: u64, measurements: Box<Measurements> },
    MeasureFailed { generation: u64, file: u64, error: String },
    /// Search results arrive in page order, a batch at a time. `searched` is
    /// how many pages have been looked at so far.
    Search { generation: u64, id: u64, searched: usize, hits: Vec<SearchHit>, done: bool },
}
