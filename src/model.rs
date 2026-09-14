//! Plain data passed between the UI thread and the pdfium worker thread.
//! Nothing in here knows about pdfium; the one egui type is a finished page
//! texture, which the worker builds so the UI thread doesn't have to.

use std::path::PathBuf;

use eframe::egui::TextureHandle;

/// Zoomed in, a page is drawn in squares of this many pixels at its drawing
/// scale, and the squares are kept: zooming back in, or scrolling back over an
/// area, shows the squares already drawn instead of drawing them again.
pub const TILE: u32 = 512;

/// One square of a page drawn zoomed in: its column and row on the page's grid
/// of `TILE`-pixel squares.
pub struct Tile {
    pub column: u32,
    pub row: u32,
    pub texture: TextureHandle,
}

/// The grid squares, as (column, row), that `region` -- x, y, width, height in
/// pixels of the page drawn `full` pixels in size -- covers.
pub fn tile_cells(full: [u32; 2], region: [u32; 4]) -> Vec<(u32, u32)> {
    let [x, y, w, h] = region;
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let (right, bottom) = ((x + w).min(full[0]), (y + h).min(full[1]));
    let columns = x / TILE..right.div_ceil(TILE);
    (y / TILE..bottom.div_ceil(TILE)).flat_map(|row| columns.clone().map(move |column| (column, row))).collect()
}

/// A square's pixel rectangle -- x, y, width, height -- on a page drawn `full`
/// pixels in size. Squares at the right and bottom edges are cut short.
pub fn tile_rect(full: [u32; 2], column: u32, row: u32) -> [u32; 4] {
    let (x, y) = (column * TILE, row * TILE);
    [x, y, TILE.min(full[0].saturating_sub(x)), TILE.min(full[1].saturating_sub(y))]
}

/// Cuts the pixels of `region` into its grid squares: each square's column,
/// row, size and RGBA. `region` must start and end on square edges, or at the
/// page's edge, as the app always asks; anything else gives no squares.
pub fn cut_tiles(full: [u32; 2], region: [u32; 4], size: [usize; 2], rgba: &[u8]) -> Vec<(u32, u32, [usize; 2], Vec<u8>)> {
    let [x, y, w, h] = region;
    let on_edges = x % TILE == 0
        && y % TILE == 0
        && ((x + w) % TILE == 0 || x + w == full[0])
        && ((y + h) % TILE == 0 || y + h == full[1]);
    if !on_edges || size != [w as usize, h as usize] || rgba.len() != size[0] * size[1] * 4 {
        return Vec::new();
    }
    tile_cells(full, region)
        .into_iter()
        .map(|(column, row)| {
            let [tx, ty, tw, th] = tile_rect(full, column, row);
            let (left, top) = ((tx - x) as usize, (ty - y) as usize);
            let (tw, th) = (tw as usize, th as usize);
            let mut pixels = Vec::with_capacity(tw * th * 4);
            for line in top..top + th {
                let start = (line * size[0] + left) * 4;
                pixels.extend_from_slice(&rgba[start..start + tw * 4]);
            }
            (column, row, [tw, th], pixels)
        })
        .collect()
}

/// An axis-aligned box in PDF user space: points, origin bottom-left, so
/// `top > bottom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfBox {
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
}

impl PdfBox {
    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.top - self.bottom
    }

    pub fn center(&self) -> (f32, f32) {
        ((self.left + self.right) / 2.0, (self.bottom + self.top) / 2.0)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x <= self.right && y >= self.bottom && y <= self.top
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }
}

/// How a page's PDF user space maps onto the page as displayed.
///
/// Text, highlights and search matches all live in user space, which isn't
/// rotated. pdfium draws the page turned by its /Rotate and trimmed to its
/// visible box, so everything drawn over the page goes through this.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeometry {
    /// Quarter turns clockwise, 0 to 3, as /Rotate 0, 90, 180, 270.
    pub rotation: u8,
    /// The visible part of the page, in user space before rotation.
    pub bounds: PdfBox,
}

impl PageGeometry {
    /// A point in user space, as a fraction of the displayed page: (0, 0) its
    /// top-left corner, (1, 1) its bottom-right.
    pub fn to_view(&self, x: f32, y: f32) -> (f32, f32) {
        let b = &self.bounds;
        let u = (x - b.left) / b.width().max(f32::EPSILON);
        let v = (b.top - y) / b.height().max(f32::EPSILON);
        match self.rotation % 4 {
            0 => (u, v),
            1 => (1.0 - v, u),
            2 => (1.0 - u, 1.0 - v),
            _ => (v, 1.0 - u),
        }
    }

    /// The inverse of `to_view`.
    pub fn from_view(&self, fx: f32, fy: f32) -> (f32, f32) {
        let (u, v) = match self.rotation % 4 {
            0 => (fx, fy),
            1 => (fy, 1.0 - fx),
            2 => (1.0 - fx, 1.0 - fy),
            _ => (1.0 - fy, fx),
        };
        let b = &self.bounds;
        (b.left + u * b.width(), b.top - v * b.height())
    }

    /// A user-space box as fractions of the displayed page:
    /// (left, top, right, bottom). Quarter turns keep boxes axis-aligned.
    pub fn box_to_view(&self, q: &PdfBox) -> (f32, f32, f32, f32) {
        let (ax, ay) = self.to_view(q.left, q.top);
        let (bx, by) = self.to_view(q.right, q.bottom);
        (ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))
    }
}

/// One character of a page's text layer. `bounds` is `None` for characters
/// pdfium synthesises, such as the spaces and line breaks it infers between
/// text runs.
#[derive(Clone, Debug)]
pub struct TextChar {
    pub ch: char,
    pub bounds: Option<PdfBox>,
}

/// A colour as 0..1 RGB, the form a PDF /C array uses.
pub type Rgb = [f32; 3];

/// Where a saved highlight lives in the file: its page and its position in
/// that page's /Annots array. Stable for as long as the bytes are, and the
/// bytes only change on save, after which the worker re-reads everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnnotKey {
    pub page: usize,
    pub index: usize,
}

#[derive(Clone, Debug)]
pub struct Highlight {
    /// `None` until the highlight has been written to the file.
    pub key: Option<AnnotKey>,
    /// Zero-based.
    pub page: usize,
    /// One band per line, in PDF user space.
    pub quads: Vec<PdfBox>,
    pub color: Rgb,
    pub comment: String,
    pub author: String,
    /// Exactly the text the highlight covers, never a whole word or line that
    /// merely overlaps it.
    pub snippet: String,
}

#[derive(Clone, Debug)]
pub struct NewHighlight {
    pub page: usize,
    pub quads: Vec<PdfBox>,
    pub color: Rgb,
    pub comment: String,
}

/// Everything the user did since the last save.
#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub adds: Vec<NewHighlight>,
    pub deletes: Vec<AnnotKey>,
    pub edits: Vec<(AnnotKey, String)>,
    pub author: String,
}

/// One place a search query was found: a band per line it spans, and the
/// text around it for listing. `before` + `matched` + `after` read as one
/// continuous excerpt.
#[derive(Clone, Debug)]
pub struct SearchHit {
    pub page: usize,
    pub quads: Vec<PdfBox>,
    pub before: String,
    pub matched: String,
    pub after: String,
}

/// UI thread -> worker. `generation` ties a request to one opened document, so
/// work for a file that has since been replaced is dropped.
pub enum Request {
    Open { generation: u64, path: PathBuf },
    Text { generation: u64, page: usize },
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
    Save { generation: u64, changes: Changes },
    /// Replaces any search in progress. A blank query just stops it.
    Search { generation: u64, id: u64, query: String },
}

/// Worker -> UI thread.
pub enum Reply {
    /// pdfium could not be loaded, so nothing else will work.
    Fatal(String),
    /// `page_sizes` are as displayed: rotated, in points.
    Opened { generation: u64, path: PathBuf, page_sizes: Vec<[f32; 2]> },
    OpenFailed { generation: u64, error: String },
    /// Highlights arrive a page at a time: a page's own just before its first
    /// render, the rest in the background. Reading them all up front loads
    /// every page, which kept a long document blank for a noticeable moment.
    /// Each page read also reports its geometry, which comes from the same
    /// page load. `done` once every page has been read.
    Highlights { generation: u64, highlights: Vec<Highlight>, geometry: Vec<(usize, PageGeometry)>, done: bool },
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
    /// `pages` are the pages the save changed and `highlights` is everything
    /// now on them. A save doesn't move annotations on any other page, so
    /// those highlights stay as they were.
    Saved { generation: u64, pages: Vec<usize>, highlights: Vec<Highlight> },
    SaveFailed { generation: u64, error: String },
    /// Search results arrive in page order, a batch at a time. `searched` is
    /// how many pages have been looked at so far.
    Search { generation: u64, id: u64, searched: usize, hits: Vec<SearchHit>, done: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(rotation: u8) -> PageGeometry {
        PageGeometry { rotation, bounds: PdfBox { left: 0.0, bottom: 0.0, right: 612.0, top: 792.0 } }
    }

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4
    }

    #[test]
    fn unrotated_page_maps_top_left_to_origin() {
        let g = letter(0);
        assert!(close(g.to_view(0.0, 792.0), (0.0, 0.0)));
        assert!(close(g.to_view(612.0, 0.0), (1.0, 1.0)));
    }

    #[test]
    fn quarter_turn_clockwise_moves_the_top_left_corner_to_the_top_right() {
        let g = letter(1);
        assert!(close(g.to_view(0.0, 792.0), (1.0, 0.0)));
        assert!(close(g.to_view(612.0, 792.0), (1.0, 1.0)));
        assert!(close(g.to_view(0.0, 0.0), (0.0, 0.0)));
    }

    #[test]
    fn three_quarter_turn_moves_the_top_left_corner_to_the_bottom_left() {
        let g = letter(3);
        assert!(close(g.to_view(0.0, 792.0), (0.0, 1.0)));
        assert!(close(g.to_view(612.0, 792.0), (0.0, 0.0)));
    }

    #[test]
    fn from_view_undoes_to_view_for_every_rotation_and_an_offset_box() {
        for rotation in 0..4 {
            let g = PageGeometry { rotation, bounds: PdfBox { left: 36.0, bottom: -18.0, right: 1720.0, top: 2366.0 } };
            for &(x, y) in &[(36.0, 2366.0), (500.0, 100.0), (1720.0, -18.0), (900.5, 1200.25)] {
                let (fx, fy) = g.to_view(x, y);
                let back = g.from_view(fx, fy);
                assert!((back.0 - x).abs() < 0.05 && (back.1 - y).abs() < 0.05, "rotation {rotation}: {:?} -> {back:?}", (x, y));
            }
        }
    }

    #[test]
    fn boxes_stay_ordered_after_rotation() {
        let q = PdfBox { left: 72.0, bottom: 700.0, right: 300.0, top: 720.0 };
        for rotation in 0..4 {
            let (l, t, r, b) = letter(rotation).box_to_view(&q);
            assert!(l < r && t < b, "rotation {rotation}");
        }
    }
}
