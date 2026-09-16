//! Laying the pages out, zooming, and moving the view to a page or a match.

use super::*;

/// The steps Ctrl +/- and the zoom buttons move through. Wide enough for a
/// whole A0 drawing on a laptop screen at one end and fine detail at the other;
/// at 5%, a whole drawing set's sheets fit on screen at once to pick from.
pub(super) const ZOOMS: [f32; 16] = [0.05, 0.1, 0.15, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0];

pub(super) const TOP_PAD: f32 = 24.0;
pub(super) const BOTTOM_PAD: f32 = 80.0;
pub(super) const SIDE_PAD: f32 = 16.0;
pub(super) const PAGE_GAP: f32 = 18.0;
/// Room the vertical scroll bar takes, allowed for when fitting pages.
pub(super) const SCROLLBAR_ROOM: f32 = 14.0;

/// A page more than this much wider than the document's usual page counts as
/// oversized, and is shrunk to the usual width while shrinking is on.
pub(super) const OVERSIZED: f32 = 1.05;

/* ------------------------------------------------------------------ *
 * Layout, geometry and drawing helpers
 * ------------------------------------------------------------------ */

/// The page size most pages share (to the nearest point), or the first page's
/// on a tie. Fit-to-width fits this, and pages much wider are "oversized".
pub(super) fn usual_page_size(sizes: &[Vec2]) -> Vec2 {
    let mut counts: HashMap<(i32, i32), (usize, usize)> = HashMap::new();
    for (i, s) in sizes.iter().enumerate() {
        counts.entry((s.x.round() as i32, s.y.round() as i32)).or_insert((0, i)).0 += 1;
    }
    counts
        .values()
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map_or(vec2(612.0, 792.0), |&(_, i)| sizes[i])
}

/// Where every page goes at `zoom`, with oversized pages shrunk to the usual
/// page width if `shrink_wide`.
pub(super) fn page_layout(sizes: &[Vec2], usual: Vec2, zoom: f32, shrink_wide: bool) -> PageLayout {
    let mut y = TOP_PAD;
    let mut tops = Vec::with_capacity(sizes.len());
    let mut scales = Vec::with_capacity(sizes.len());
    let mut widest = 0.0f32;
    for s in sizes {
        let shrink = if shrink_wide && s.x > usual.x * OVERSIZED { usual.x / s.x } else { 1.0 };
        let scale = zoom * shrink;
        tops.push(y);
        scales.push(scale);
        widest = widest.max(s.x * scale);
        y += s.y * scale + PAGE_GAP;
    }
    PageLayout { tops, scales, widest, height: y - PAGE_GAP + BOTTOM_PAD }
}

/// The scrolling column is the view's width, or the widest page plus margins
/// if that's wider.
pub(super) fn content_width(view_width: f32, widest: f32) -> f32 {
    view_width.max(widest + 2.0 * SIDE_PAD)
}

/// A page is centred in the column.
pub(super) fn page_x(content_width: f32, page_width: f32) -> f32 {
    ((content_width - page_width) / 2.0).max(SIDE_PAD)
}

/// A stand-in until the worker reports a page's real geometry: unrotated,
/// origin at the corner. Only used to choose where to scroll.
pub(super) fn flat_geometry(size: Vec2) -> PageGeometry {
    PageGeometry { rotation: 0, bounds: PdfBox { left: 0.0, bottom: 0.0, right: size.x, top: size.y } }
}

/// A box in page space to its rectangle on screen.
pub(super) fn to_screen(page: Rect, geometry: &PageGeometry, b: &PdfBox) -> Rect {
    let (left, top, right, bottom) = geometry.box_to_view(b);
    Rect::from_min_max(
        pos2(page.min.x + left * page.width(), page.min.y + top * page.height()),
        pos2(page.min.x + right * page.width(), page.min.y + bottom * page.height()),
    )
}

/// A point on screen to page space.
pub(super) fn to_pdf(page: Rect, geometry: &PageGeometry, p: Pos2) -> (f32, f32) {
    geometry.from_view((p.x - page.min.x) / page.width(), (p.y - page.min.y) / page.height())
}

/// Output pixels per PDF point for a page drawn at `scale` screen points per
/// PDF point: device resolution, capped by the largest texture the GPU takes
/// and by `MAX_PAGE_PIXELS`.
///
/// The scale is rounded up to a quarter power of two first (`quantize_scale`),
/// so small changes of window size or zoom keep the page image they have --
/// and find the same one in the page cache -- at the cost of up to 19% more
/// pixels.
pub fn render_scale(size: Vec2, scale: f32, ppp: f32, max_side: f32) -> f32 {
    let by_side = max_side / size.x.max(size.y).max(1.0);
    let by_pixels = (MAX_PAGE_PIXELS / (size.x * size.y).max(1.0)).sqrt();
    quantize_scale(scale * ppp).min(by_side).min(by_pixels)
}

/// `scale` rounded up to the nearest quarter power of two.
pub fn quantize_scale(scale: f32) -> f32 {
    if scale <= 0.0 {
        return scale;
    }
    // The small nudge keeps an exact step, such as 1 or 2, where it is.
    2f32.powf((scale.log2() * 4.0 - 1e-3).ceil() / 4.0)
}

impl App {
    /* -------------------------------------------------------------- *
     * Zoom and layout
     * -------------------------------------------------------------- */

    /// The next zoom step up (1) or down (-1), holding the middle of the view still.
    pub(super) fn zoom_by(&mut self, direction: i32) {
        let next = if direction > 0 {
            ZOOMS.iter().copied().find(|z| *z > self.zoom * 1.001)
        } else {
            ZOOMS.iter().rev().copied().find(|z| *z < self.zoom * 0.999)
        };
        if let Some(zoom) = next {
            self.zoom_mode = ZoomMode::Custom;
            self.change_zoom(zoom, None);
        }
    }

    /// Changes the zoom, keeping the page spot under `anchor` (or the middle of
    /// the view) where it is on screen.
    pub(super) fn change_zoom(&mut self, zoom: f32, anchor: Option<Pos2>) {
        let zoom = zoom.clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1]);
        if (zoom - self.zoom).abs() < 1e-4 {
            return;
        }
        self.hold_still(anchor);
        self.zoom = zoom;
    }

    /// Remembers the page spot under `anchor` (or the middle of the view), so
    /// the next layout can scroll it back to the same place on screen.
    pub(super) fn hold_still(&mut self, anchor: Option<Pos2>) {
        // Several changes in one frame keep the first anchor, which is the one
        // measured against what's actually on screen.
        if self.zoom_anchor.is_some() || !self.viewer_rect.is_positive() {
            return;
        }
        let view = self.viewer_rect;
        let screen = anchor.filter(|p| view.contains(*p)).unwrap_or_else(|| view.center());
        let distance = |r: &Rect| {
            if screen.y < r.min.y {
                r.min.y - screen.y
            } else if screen.y > r.max.y {
                screen.y - r.max.y
            } else {
                0.0
            }
        };
        let Some((&page, rect)) = self.page_rects.iter().min_by(|a, b| distance(a.1).total_cmp(&distance(b.1))) else {
            return;
        };
        self.zoom_anchor = Some(ZoomAnchor {
            page,
            fx: ((screen.x - rect.min.x) / rect.width()).clamp(0.0, 1.0),
            fy: ((screen.y - rect.min.y) / rect.height()).clamp(0.0, 1.0),
            screen,
            origin: self.content_origin + self.scroll_offset,
        });
    }

    pub(super) fn set_shrink_wide(&mut self, shrink: bool) {
        if shrink != self.shrink_wide {
            self.hold_still(None);
            self.shrink_wide = shrink;
        }
    }

    /// The zoom a fit mode wants for a view of this size.
    pub(super) fn fit_zoom(&self, doc: &Doc, view: Vec2) -> Option<f32> {
        let usual = doc.usual_size;
        let width = (view.x - 2.0 * SIDE_PAD - SCROLLBAR_ROOM).max(50.0) / usual.x.max(1.0);
        let zoom = match self.zoom_mode {
            ZoomMode::FitWidth => width,
            ZoomMode::FitPage => width.min((view.y - TOP_PAD - PAGE_GAP).max(50.0) / usual.y.max(1.0)),
            ZoomMode::Custom => return None,
        };
        Some(zoom.clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1]))
    }

    pub(super) fn layout(&self, doc: &Doc) -> PageLayout {
        page_layout(&doc.sizes, doc.usual_size, self.zoom, self.shrink_wide)
    }

    /// Whether everything in view was drawn at full sharpness last frame: every
    /// page's image at the size wanted and, where zoomed in, every square
    /// covering the view -- wherever they came from, memory, disk or drawing.
    /// For tools driving the app, such as floors.
    pub fn view_is_sharp(&self) -> bool {
        self.view_sharp
    }

    /// Scrolls so `page` (counted from 0) starts at the top of the view: the
    /// toolbar's page box, and tools driving the app, such as floors.
    pub fn go_to_page(&mut self, page: usize) {
        let Some(doc) = &self.doc else { return };
        let layout = self.layout(doc);
        if let Some(&top) = layout.tops.get(page) {
            self.scroll_y = Some(top);
            self.current_page = page;
        }
    }

    /// Scrolls to the very end of the document: the bottom of the last page.
    /// The scroll area keeps the offset within what it can scroll.
    pub(super) fn go_to_end(&mut self) {
        let Some(doc) = &self.doc else { return };
        let layout = self.layout(doc);
        let Some(last) = doc.sizes.len().checked_sub(1) else { return };
        self.scroll_y = Some(layout.height);
        self.current_page = last;
    }

    /// Scrolls a spot on a page into view: a third of the way down, and
    /// horizontally centred when the pages are wider than the window.

    pub(super) fn scroll_to_box(&mut self, page: usize, q: &PdfBox) {
        let Some(doc) = &self.doc else { return };
        let layout = self.layout(doc);
        let Some(&top) = layout.tops.get(page) else { return };
        let size = doc.sizes[page] * layout.scales[page];
        let geometry = doc.geometry[page].unwrap_or_else(|| flat_geometry(doc.sizes[page]));
        let (left, t, right, _) = geometry.box_to_view(q);

        self.scroll_y = Some(top + t * size.y - self.viewer_rect.height() / 3.0);
        let view_w = self.viewer_rect.width();
        let content_w = content_width(view_w, layout.widest);
        if content_w > view_w + 1.0 {
            let x = page_x(content_w, size.x) + (left + right) / 2.0 * size.x;
            self.scroll_x = Some(x - view_w / 2.0);
        }
    }
}
