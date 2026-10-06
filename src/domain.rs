//! Document geometry, annotations and changes, independent of UI and worker messages.

use std::collections::BTreeSet;
use serde::{Deserialize, Serialize};

pub use markup_model::markup::FillPattern;
pub use markup_model::Markup as MeasureMarkup;
pub use markup_model::ScaleStore;

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

    /// The same page turned `turns` further quarter-turns clockwise: a sheet
    /// the user has turned but the file has not yet been written with.
    ///
    /// Everything drawn over a page is placed as a fraction of the page *as
    /// displayed*, so turning the sheet is turning this and nothing else --
    /// text, highlights, markups and measurements all come round with it.
    pub fn turned(self, turns: u8) -> Self {
        PageGeometry { rotation: (self.rotation + turns) % 4, bounds: self.bounds }
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
    /// Ink bounds for deciding whether an erasure removed the whole glyph.
    pub ink_bounds: Option<PdfBox>,
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

/// What a markup is: one of the drawing tools' shapes, or, read from a file,
/// another kind of drawn annotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkupKind {
    Pen,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    /// A polygon, cloud or polyline made in another program.
    Other,
}

impl MarkupKind {
    /// Whether it encloses an area, and so can be filled. A pen stroke, a
    /// line and an arrow have an inside only by accident of where they run.
    pub fn fills(self) -> bool {
        matches!(self, MarkupKind::Rectangle | MarkupKind::Ellipse)
    }
}

/// How a drawn markup looks beyond its line's colour and width: what fills
/// it, what is ruled over the fill, and how see-through each layer is.
///
/// The same ground `markup_model::markup::Style` covers for measurements, for
/// the shapes that aren't measured. Kept apart from `color` and `width`,
/// which a markup has had since before any of this and which the file itself
/// carries in /C and /BS.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawStyle {
    /// The line's, from see-through to solid.
    pub opacity: f32,
    /// What's inside it, if anything. Only shapes that enclose an area take
    /// one: see `MarkupKind::fills`.
    pub fill: Option<Rgb>,
    pub fill_opacity: f32,
    /// Ruled over the fill, in its own colour, so a pale fill can carry a
    /// darker hatch the way a take-off is usually marked up.
    pub pattern: FillPattern,
    pub pattern_colour: Option<Rgb>,
    pub pattern_opacity: f32,
    /// The cell the pattern repeats in, in points on the page.
    pub pattern_size: f32,
    /// Dash and gap lengths in PDF points. Empty for a solid line.
    pub dash: Vec<f64>,
}

impl Default for DrawStyle {
    fn default() -> Self {
        DrawStyle {
            opacity: 1.0,
            fill: None,
            fill_opacity: 1.0,
            pattern: FillPattern::default(),
            pattern_colour: None,
            pattern_opacity: 1.0,
            pattern_size: 6.0,
            dash: Vec::new(),
        }
    }
}

/// A drawn annotation: a pen stroke, rectangle, ellipse, line or arrow.
#[derive(Clone, Debug, PartialEq)]
pub struct Markup {
    /// `None` until the markup has been written to the file.
    pub key: Option<AnnotKey>,
    /// Zero-based.
    pub page: usize,
    pub kind: MarkupKind,
    /// In PDF user space: a pen stroke's points, or the two ends of a drag
    /// for the other tools. Empty for a markup read from the file, whose page
    /// drawing shows it.
    pub points: Vec<[f32; 2]>,
    /// The box it covers, stroke included, in PDF user space.
    pub bounds: PdfBox,
    pub color: Rgb,
    /// The stroke's width in points.
    pub width: f32,
    /// The fill and what's ruled over it.
    pub style: DrawStyle,
    /// What it's called in the quantities list, ahead of its description.
    /// Blank unless the tool it was drawn with names what it draws.
    pub name: String,
    pub comment: String,
    pub author: String,
}

/// A document's scales and the measurements made with them, read from the
/// file. Kept apart from highlights and markups, which the worker reads with
/// pdfium a page at a time; these come from one pass with lopdf, since
/// pdfium can't see /VP or /Measure.
#[derive(Debug, Default)]
pub struct Measurements {
    pub pins: Vec<crate::pins::Pin>,
    pub scales: ScaleStore,
    /// The layers, bottom first.
    pub layers: markup_model::LayerStack,
    pub markups: Vec<MeasureMarkup>,
    /// Measurement annotations that couldn't be read, and why.
    pub skipped: Vec<String>,
}

/// The measurements to write into the file, and those to take out of it.
#[derive(Clone, Debug, Default)]
pub struct MeasureChanges {
    /// New measurements, and changed ones, which are written afresh.
    pub written: Vec<MeasureMarkup>,
    /// The page and /NM of each annotation to take out: those gone, and
    /// those about to be written again.
    pub removed: Vec<(usize, String)>,
}

/// The scales to write into the file, and the pages whose /VP they change.
#[derive(Clone, Debug, Default)]
pub struct ScaleChanges {
    pub scales: ScaleStore,
    pub pages: Vec<usize>,
}

/// An annotation already in the file whose note or author was changed. Both
/// are written whichever of them changed, since the pair is what the file
/// holds for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotEdit {
    pub key: AnnotKey,
    pub comment: String,
    pub author: String,
}

/// Part of a page's own drawing to take out: what the page draws within
/// `region`, a polygon in its user space. Not the annotations over it.
#[derive(Clone, Debug, PartialEq)]
pub struct Erasure {
    pub page: usize,
    pub region: Vec<[f32; 2]>,
    /// The one layer erased -- an overlay's set, by its optional content
    /// group's object number and generation -- or `None` for everything.
    pub layer: Option<(u32, u16)>,
}

/// Everything the user did since the last save.
#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub pins: Option<Vec<crate::pins::Pin>>,
    pub adds: Vec<NewHighlight>,
    /// New markups, those without a key.
    pub markups: Vec<Markup>,
    /// Saved highlights and markups the user removed.
    pub deletes: Vec<AnnotKey>,
    pub edits: Vec<AnnotEdit>,
    /// The name put on new highlights; each edit carries its own.
    pub author: String,
    /// Scales and viewports to write, if any changed.
    pub scales: Option<ScaleChanges>,
    /// The document's layers, if any changed.
    pub layers: Option<markup_model::LayerStack>,
    /// Measurements to write, and to take out.
    pub measures: MeasureChanges,
    /// Parts of pages' own drawing to erase, in the order they were.
    pub erasures: Vec<Erasure>,
}

impl Changes {
    /// The pages whose highlights or notes change, or that lose annotations.
    pub fn edited_pages(&self) -> BTreeSet<usize> {
        let adds = self.adds.iter().map(|a| a.page);
        adds.chain(self.deletes.iter().map(|k| k.page)).chain(self.edits.iter().map(|e| e.key.page)).collect()
    }

    /// Every page the changes touch, new markups' included.
    pub fn pages(&self) -> BTreeSet<usize> {
        let mut pages = self.edited_pages();
        pages.extend(self.markups.iter().map(|m| m.page));
        pages
    }
}

/// A page's highlights and markups, in /Annots order.
#[derive(Clone, Debug, Default)]
pub struct PageNotes {
    pub highlights: Vec<Highlight>,
    pub markups: Vec<Markup>,
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
