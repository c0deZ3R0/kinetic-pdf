//! The page viewer: what's in view, asking for pages and zoomed-in squares,
//! keeping textures within budget, and drawing them.

use super::gpu::{self, annotations_drawn};
use super::*;

/// Memory allowed for page images kept beyond the ones in view (and the page
/// either side, which are always kept so they don't flicker).
pub(super) const TEXTURE_BUDGET: usize = 512 * 1024 * 1024;
/// The most pixels a whole-page image is rendered at, about 32 MB of RGBA.
/// Zoomed in past this, the part of the page in view is rendered on its own at
/// full sharpness (`Detail`), so the whole-page image is only the backdrop for
/// the edges while scrolling -- and a smaller one renders much sooner.
pub(super) const MAX_PAGE_PIXELS: f32 = 8_000_000.0;
/// Device pixels rendered beyond each edge of the view when zoomed in past a
/// whole-page image, so scrolling a little doesn't need a new render.
pub(super) const DETAIL_MARGIN: u32 = 384;

/// Most memory for squares of pages drawn zoomed in (see `model::TILE`), kept
/// for zooming back in or scrolling back: about 380 squares. See `budgets`.
pub(super) const TILE_BUDGET: usize = 384 * 1024 * 1024;

/// Most memory for earlier whole-page images kept for zooming back to their
/// size. See `budgets`.
pub(super) const SPARE_BUDGET: usize = 256 * 1024 * 1024;

/// Most memory for pages' thumbnails: about 130 sheets of a drawing set, at
/// three quarters of a megabyte each. Zoomed out they are what the pages are
/// drawn from, in place of shapes costing twenty times as much.
pub(super) const THUMBNAIL_BUDGET: usize = 96 * 1024 * 1024;

/// How much bigger than its thumbnail a page can be drawn before the thumbnail
/// starts fading into the paper behind it, and how much bigger before it has
/// faded as far as it goes. Four times is soft but still the page; sixty times,
/// as zooming a sheet right in would be, is a smear that on a dense drawing
/// reads as the screen going dark.
pub(super) const MOST_THUMBNAIL_STRETCH: f32 = 6.0;
const THUMBNAIL_FADED_AT: f32 = 20.0;

/// How much of the thumbnail is left once it has faded as far as it goes: still
/// plainly the page, just softer, rather than the screen going either dark or
/// white.
const FAINTEST_THUMBNAIL: f32 = 0.45;

/// Seconds before a page with no thumbnail kept is asked about again. What is
/// drawn is kept as it's drawn, and written in the background, so a page asked
/// about a moment too early would otherwise stay blank however often it came
/// back into view.
pub(super) const THUMBNAIL_RETRY: f64 = 2.0;

/// The memory allowed for squares and for spare page images, from the
/// physical memory free when the app started: the most above on a machine
/// with plenty, less on one without.
///
/// A texture costs the process about two and a half times its pixels, since
/// the graphics driver keeps copies of its own: opening a drawing set and
/// letting the app draw 176 MB of squares ahead of a zoom took the process
/// from 250 MB to 740 MB. So the full budgets can cost 1.6 GB, and here they
/// come to about a third of what was free.
pub(super) fn budgets(free: u64) -> (usize, usize) {
    const MB: u64 = 1024 * 1024;
    let tiles = (free / 12).clamp(64 * MB, TILE_BUDGET as u64) as usize;
    let spares = (free / 18).clamp(32 * MB, SPARE_BUDGET as u64) as usize;
    (tiles, spares)
}

/// Seconds the pointer rests before the spot under it is drawn ahead of a
/// zoom in; see `draw_pages`.
pub(super) const PREDICT_REST: f64 = 0.5;
/// Drawings asked for ahead of a zoom at once, about one per helper.
pub(super) const PREDICT_JOBS: usize = 3;
/// Squares on a side in each region drawn ahead of a zoom: 2048 pixels.
pub(super) const PREDICT_BLOCK: u32 = 4;
/// Seconds after which an unanswered request ahead of a zoom counts as lost,
/// as when no helpers are running to answer it.
pub(super) const PREDICT_TIMEOUT: f64 = 20.0;

/// Screens a second. Faster than this is a fling or a scroll bar drag, not
/// reading, so pages flying past aren't loaded until the view slows. Steady
/// wheel scrolling stays under it (about five screens a second at most), so
/// pages still load as they pass.
pub(super) const FAST_SCROLL: f32 = 8.0;
/// Pages loaded ahead of the view once everything in it is drawn.
pub(super) const LOOK_AHEAD: usize = 3;

/// Seconds a zoom has to stay put before slow pages are drawn again at the new
/// size. Each redraw of one takes a second or more, so redrawing at every
/// step of a zoom would only queue them up; the old image stretches meanwhile.
pub(super) const ZOOM_SETTLE: f64 = 0.3;

/// Where a square sits on screen, for the page drawn at `page`.
pub(super) fn tile_screen_rect(page: Rect, full: [u32; 2], column: u32, row: u32) -> Rect {
    let [fw, fh] = full.map(|v| v as f32);
    let [x, y, w, h] = crate::model::tile_rect(full, column, row).map(|v| v as f32);
    Rect::from_min_size(
        page.min + vec2(x / fw * page.width(), y / fh * page.height()),
        vec2(w / fw * page.width(), h / fh * page.height()),
    )
}

/// The size the sharpest squares kept for `page` were drawn for, if it has any.
/// A page zoomed away from, or one drawn ahead of a zoom, keeps its squares
/// without asking for any, and they are what shows it until it is drawn again.
fn squares_drawn_for(doc: &Doc, page: usize) -> Option<[u32; 2]> {
    let annotations = annotations_drawn(doc, page);
    let mine = doc.tiles.keys().filter(|key| key.page == page && key.annotations == annotations);
    mine.max_by_key(|key| key.full[0]).map(|key| key.full)
}

/// Whether a page still needs rendering at `scale`: it has no image, only a
/// part-drawn one, or one at another scale or with its annotations drawn in or
/// not when they should be the other way -- and pdfium hasn't failed on it,
/// and the GPU doesn't draw the whole of it.
pub(super) fn needs_render(doc: &Doc, page: usize, scale: f32) -> bool {
    let annotations = annotations_drawn(doc, page);
    !doc.failed.contains(&page)
        && !gpu::drawn_whole(doc, page)
        && doc.textures.get(&page).is_none_or(|t| !t.complete || (t.scale - scale).abs() > 1e-3 || t.annotations != annotations)
}

/// The pages to load once the view is drawn, nearest first: up to `count - 1`
/// the way the view is heading, then back the other way. Near either end of
/// the document there is less ahead, so more comes from behind.
pub(super) fn pages_ahead(first: usize, last: usize, n: usize, down: bool, count: usize) -> Vec<usize> {
    let after: Vec<usize> = (last + 1..n).take(count).collect();
    let before: Vec<usize> = (0..first).rev().take(count).collect();
    let (forward, backward) = if down { (after, before) } else { (before, after) };
    let leading = forward.len().min(count.saturating_sub(1).max(1));
    let mut pages: Vec<usize> = forward[..leading].to_vec();
    pages.extend(backward.iter().take(count.saturating_sub(pages.len())));
    let room = count.saturating_sub(pages.len());
    pages.extend(forward[leading..].iter().take(room));
    pages
}

/// Redraw the part of the page under the highlight with the page texture
/// tinted by the highlight colour. Tinting multiplies, so the white page turns
/// the highlight colour while black glyphs stay black -- see-through, like a
/// real highlighter, with no blend modes needed. Where sharp squares of the
/// zoomed-in view (`detail`) cover the page, they are tinted too, on top.
pub(super) fn paint_highlight(
    painter: &egui::Painter,
    texture: Option<TextureId>,
    detail: &[(TextureId, Rect)],
    page: Rect,
    r: Rect,
    color: Color32,
) {
    match texture {
        Some(id) => painter.image(id, r, uv_within(page, r), color),
        None => painter.rect_filled(r, CornerRadius::same(1), color.gamma_multiply(0.45)),
    };
    for &(id, area) in detail {
        let part = r.intersect(area);
        if part.is_positive() {
            painter.image(id, part, uv_within(area, part), color);
        }
    }
}

/// Where `inner` falls within `outer`, as texture coordinates.
pub(super) fn uv_within(outer: Rect, inner: Rect) -> Rect {
    Rect::from_min_max(
        pos2((inner.min.x - outer.min.x) / outer.width(), (inner.min.y - outer.min.y) / outer.height()),
        pos2((inner.max.x - outer.min.x) / outer.width(), (inner.max.y - outer.min.y) / outer.height()),
    )
}

impl App {
    /* -------------------------------------------------------------- *
     * Page viewer
     * -------------------------------------------------------------- */

    pub(super) fn viewer(&mut self, ui: &mut Ui) {
        let hovering_file = ui.input(|i| !i.raw.hovered_files.is_empty());
        let area = ui.max_rect();

        if let Some(message) = self.fatal.clone() {
            self.message_card(ui, "pdfium could not be loaded", &message, false);
        } else if self.doc.is_none() {
            self.message_card(
                ui,
                "Kinetic PDF",
                "Open a PDF, drag across text to highlight it, and attach a note.\n\nHighlights are written \
                 into the PDF as real annotations, so they open anywhere. Drop a file on this window to get started.",
                true,
            );
        } else {
            self.pages(ui);
        }

        if hovering_file {
            ui.painter()
                .rect_stroke(area.shrink(12.0), CornerRadius::same(10), Stroke::new(2.5, ACCENT), StrokeKind::Inside);
        }
    }

    pub(super) fn message_card(&mut self, ui: &mut Ui, title: &str, body: &str, with_open: bool) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.12);
            Frame::NONE
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(14))
                .inner_margin(Margin::same(32))
                .shadow(soft_shadow())
                .show(ui, |ui| {
                    ui.set_max_width(400.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(title).size(22.0).strong().color(TEXT));
                        ui.add_space(6.0);
                        ui.label(RichText::new(body).color(QUOTE_TEXT));
                        if with_open {
                            ui.add_space(14.0);
                            if styled_button(ui, "Open PDF…", Tone::Primary, false).clicked() {
                                self.pick_and_open();
                            }
                        }
                    });
                });
        });
    }

    pub(super) fn pages(&mut self, ui: &mut Ui) {
        // Fit modes follow the window.
        let view = ui.available_size();
        let fit = self.doc.as_ref().and_then(|doc| self.fit_zoom(doc, view));
        if let Some(fit) = fit {
            if (fit - self.zoom).abs() > self.zoom * 0.002 {
                self.change_zoom(fit, None);
            }
        }

        let Some(doc) = self.doc.as_ref() else { return };
        let layout = self.layout(doc);

        // Put the spot held during a zoom change back where it was on screen.
        if let Some(anchor) = self.zoom_anchor.take() {
            if let (Some(&top), Some(&scale)) = (layout.tops.get(anchor.page), layout.scales.get(anchor.page)) {
                let view_w = if self.viewer_rect.is_positive() { self.viewer_rect.width() } else { view.x - SCROLLBAR_ROOM };
                let size = doc.sizes[anchor.page] * scale;
                let point = vec2(page_x(content_width(view_w, layout.widest), size.x) + anchor.fx * size.x, top + anchor.fy * size.y);
                let offset = anchor.origin.to_vec2() + point - anchor.screen.to_vec2();
                self.scroll_x = Some(offset.x);
                self.scroll_y = Some(offset.y);
            }
        }

        // Dragging with the middle button grabs the document and moves it with
        // the pointer, in both directions.
        let (pressed, held, moved, at) = ui.ctx().input(|i| {
            (i.pointer.button_pressed(egui::PointerButton::Middle), i.pointer.middle_down(), i.pointer.delta(), i.pointer.hover_pos())
        });
        if pressed && at.is_some_and(|at| self.viewer_rect.contains(at)) {
            self.panning = true;
        }
        if !held {
            self.panning = false;
        }
        if self.panning && moved != Vec2::ZERO {
            self.scroll_x = Some(self.scroll_x.unwrap_or(self.scroll_offset.x) - moved.x);
            self.scroll_y = Some(self.scroll_y.unwrap_or(self.scroll_offset.y) - moved.y);
        }

        let mut area = egui::ScrollArea::both().id_salt("pages").auto_shrink(false);
        if let Some(x) = self.scroll_x.take() {
            area = area.horizontal_scroll_offset(x.max(0.0));
        }
        if let Some(y) = self.scroll_y.take() {
            area = area.vertical_scroll_offset(y.max(0.0));
        }
        let content = vec2(layout.widest + 2.0 * SIDE_PAD, layout.height);
        let output = area.show_viewport(ui, |ui, viewport| {
            ui.set_min_width(content.x);
            ui.set_height(content.y);
            self.draw_pages(ui, viewport, &layout);
        });
        self.viewer_rect = output.inner_rect;
        self.scroll_offset = output.state.offset;
        // Set last, so the hand wins over the text cursor the pages ask for.
        if self.panning {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
    }

    /// Lays out every page but only draws, and only asks the worker about, the
    /// ones in view -- so a 5,000-page document costs no more per frame than a
    /// five-page one.
    pub(super) fn draw_pages(&mut self, ui: &mut Ui, viewport: Rect, layout: &PageLayout) {
        let tops = &layout.tops;
        if tops.is_empty() {
            return;
        }
        let ctx = ui.ctx().clone();
        // Render at device resolution so text stays sharp on a high-DPI screen.
        let ppp = ctx.pixels_per_point().min(2.0);
        let max_side = ctx.input(|i| i.max_texture_side) as f32;
        let origin = ui.max_rect().min;
        self.content_origin = origin;
        let content_w = content_width(ui.max_rect().width(), layout.widest);
        let busy = matches!(self.status, Status::Saving);

        let n = tops.len();
        let first = tops.partition_point(|t| *t <= viewport.min.y).saturating_sub(1);
        let mut last = first;
        while last + 1 < n && tops[last + 1] < viewport.max.y {
            last += 1;
        }
        self.current_page = tops.partition_point(|t| *t <= viewport.min.y + viewport.height() * 0.35).saturating_sub(1);

        let segments = match (&self.drag, &self.doc) {
            (Some(drag), Some(doc)) => drag_segments(&doc.text, drag),
            _ => Vec::new(),
        };
        // The calibration line being drawn, if any, copied out before the
        // document is borrowed to draw the pages.
        let calibrating = match self.drag {
            Some(Drag::Calibrate { page, from, to }) => Some((page, from, to)),
            _ => None,
        };
        let active = self.active;
        let shrink_wide = self.shrink_wide;
        let mut drag_start = None;
        let mut clicked = None;
        let mut toggle_shrink = false;
        self.page_rects.clear();

        let Some(doc) = self.doc.as_mut() else { return };

        // Images of the page either side are always kept, so they don't flicker.
        let lo = first.saturating_sub(1);
        let hi = (last + 1).min(n - 1);

        // How fast the view is moving. A fast scroll passes pages that each
        // take a second to load if they're dense, so nothing new is asked for
        // until the view slows to where it can be read.
        let now = ctx.input(|i| i.time);
        let (moved, elapsed) = self.last_view.map_or((0.0, 1.0), |(t, y)| (viewport.min.y - y, (now - t) as f32));
        self.last_view = Some((now, viewport.min.y));
        if moved.abs() > 0.5 {
            self.heading_down = moved > 0.0;
        }
        let moving = elapsed > 0.0 && moved.abs() / elapsed > FAST_SCROLL * viewport.height();
        if moving {
            // Look again shortly, in case that was the scroll's last frame.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.zoom != self.last_zoom {
            self.last_zoom = self.zoom;
            self.zoom_changed_at = now;
        }
        let settling = now - self.zoom_changed_at < ZOOM_SETTLE;
        if settling {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(ZOOM_SETTLE));
        }

        // The pages in view, from the middle of the view out, so the page the
        // view is on goes first.
        let middle = viewport.center().y;
        let mut order: Vec<usize> = (first..=last).collect();
        let off_middle = |p: usize| (tops[p] + doc.sizes[p].y * layout.scales[p] / 2.0 - middle).abs();
        order.sort_by(|&a, &b| off_middle(a).total_cmp(&off_middle(b)));

        // Once they're all drawn, load ahead: the way the view is heading,
        // then back the other way.
        let scale_of = |p: usize| render_scale(doc.sizes[p], layout.scales[p], ppp, max_side);
        let settled = order.iter().all(|&p| !needs_render(doc, p, scale_of(p)));
        let ahead = if settled && !moving { pages_ahead(first, last, n, self.heading_down, LOOK_AHEAD) } else { Vec::new() };

        // Every page's drawing scale, for the helpers drawing ahead into the
        // page cache.
        let scales: Vec<f32> = (0..n).map(scale_of).collect();
        if *self.render_scales != scales {
            self.render_scales = Arc::new(scales);
        }
        // A measurement tool snaps to the drawing's own lines, which are
        // indexed as each page is read.
        let snapping = self.measure_tool.is_some();
        // Drawing ahead and the highlight scan hold off while the view moves
        // or a zoom settles.
        let holding = moving || settling;
        if let Ok(mut wanted) = self.wanted.lock() {
            let pages: Vec<usize> = order.iter().chain(&ahead).copied().collect();
            if wanted.pages != pages || wanted.moving != holding {
                let unsettled: Vec<usize> = order.iter().copied().filter(|&p| needs_render(doc, p, scale_of(p))).collect();
                worker::trace(format_args!(
                    "ui: wanted {pages:?}, holding {holding}, heading down {}, in view but not drawn {unsettled:?}",
                    self.heading_down
                ));
            }
            wanted.generation = doc.generation;
            wanted.pages = pages;
            wanted.moving = holding;
            if !Arc::ptr_eq(&wanted.render_scales, &self.render_scales) {
                wanted.render_scales = Arc::clone(&self.render_scales);
            }
            let without_annotations = gpu::pages_without_annotations(doc);
            if *wanted.without_annotations != without_annotations {
                wanted.without_annotations = Arc::new(without_annotations);
            }
            wanted.skip_drawing_ahead = doc.reader.is_some();
            wanted.snapping = snapping;
        }
        // The page in view is read again for its lines the first time a
        // measurement tool wants them.
        if snapping {
            let page = self.current_page.min(n.saturating_sub(1));
            gpu::want_snapping(doc, page, gpu::image_density(scale_of(page) * ppp));
            gpu::trim_snapping(doc, page);
        }

        let mut tile_full_now: HashMap<usize, [u32; 2]> = HashMap::new();
        // Pages small enough to be shown from their thumbnail; see
        // `gpu::thumbnail_is_enough`.
        let mut from_thumbnails: HashSet<usize> = HashSet::new();
        let mut sharp = true;
        let wanted_pages: Vec<usize> = order.iter().chain(&ahead).copied().collect();
        for (i, &page) in wanted_pages.iter().enumerate() {
            let in_view = i < order.len();
            let scale = render_scale(doc.sizes[page], layout.scales[page], ppp, max_side);
            let slow = doc.slow.contains(&page);
            // An earlier image of the page at this size comes straight back
            // from memory, as when zooming back out.
            let annotations = annotations_drawn(doc, page);
            if doc.textures.get(&page).is_none_or(|t| (t.scale - scale).abs() > 1e-3 || t.annotations != annotations) {
                if let Some(spare) = doc.spares.remove(&(page, scale.to_bits())).filter(|s| s.annotations == annotations) {
                    if let Some(old) = doc.textures.insert(page, spare) {
                        if old.complete {
                            doc.spares.insert((page, old.scale.to_bits()), old);
                        }
                    }
                }
            }
            // Zoomed out to where the page is no wider than its thumbnail,
            // that is every pixel the screen can show of it: nothing is read,
            // drawn or kept for it, and its text isn't worth extracting when
            // none of it can be picked out.
            let density = gpu::image_density(layout.scales[page] * ppp);
            if gpu::thumbnail_is_enough(doc, page, layout.scales[page] * ppp) {
                from_thumbnails.insert(page);
                // Squares drawn ahead of a zoom show through the thumbnail once
                // the zoom is under way -- not while the view rests out here,
                // where they would be one sharp patch on an otherwise soft page.
                if moving {
                    if let Some(full) = doc.tile_full.get(&page).copied().or_else(|| squares_drawn_for(doc, page)) {
                        tile_full_now.insert(page, full);
                    }
                }
                continue;
            }
            if moving {
                // Whatever squares the page has stay up while the view moves,
                // including ones drawn ahead for the zoom it is heading for:
                // zooming in from far out they are the only sharp thing there
                // is, and they grow into place as the zoom reaches them.
                let full = doc.tile_full.get(&page).copied().or_else(|| squares_drawn_for(doc, page));
                if let Some(full) = full {
                    tile_full_now.insert(page, full);
                }
                // Drawing by pdfium holds off until the view lands, but shapes
                // don't: they cost nothing to draw at any zoom once they're up,
                // and reading them takes long enough that starting when the
                // zoom ends is starting too late. Zooming from far out onto a
                // page that has only a thumbnail, waiting would mean watching
                // it stretch for the whole of the zoom. Only the page nearest
                // the middle of the view, which is where a zoom lands and what
                // drawing ahead aims at: the rest would be read for zooms they
                // are out of view by the end of.
                if i == 0 {
                    gpu::wait_for_shapes(doc, page, now, density, true, &[]);
                }
                sharp &= !in_view;
                continue;
            }
            if in_view && !doc.text.contains_key(&page) && doc.text_pending.insert(page) {
                let _ = self.tx.send(Request::Text { generation: doc.generation, page });
            }
            // A page whose annotations may go on the GPU waits a moment for
            // them, rather than have pdfium draw them in only to draw the page
            // again without them.
            // A page being handed to pdfium doesn't wait: pdfium is asked for it
            // now, and its shapes, read at whatever size fits, land when they do.
            let wanted_more = &wanted_pages[..i];
            let waiting = gpu::wait_for_shapes(doc, page, now, density, false, wanted_more);
            if waiting && !doc.handing_over.contains(&page) {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(gpu::SHAPES_WAIT));
                sharp &= !in_view;
                continue;
            }
            // A page the GPU draws whole needs nothing drawn by pdfium -- unless
            // it is being handed over, when its shapes only keep it on screen
            // until pdfium, which is what it is waiting for, has drawn it.
            if gpu::drawn_whole(doc, page) && !doc.handing_over.contains(&page) {
                continue;
            }
            // A slow page already showing waits for the zoom to settle.
            let wait_for_zoom = settling && slow && doc.textures.contains_key(&page);
            if !wait_for_zoom && needs_render(doc, page, scale) && doc.render_pending.insert(page) {
                let _ = self.tx.send(Request::Render { generation: doc.generation, page, scale });
            }
            // The page's image counts towards a sharp view unless squares cover
            // the view, in which case it's only the backdrop to them.
            let page_unsharp = in_view && needs_render(doc, page, scale);
            if !in_view || doc.failed.contains(&page) {
                sharp &= !page_unsharp;
                continue;
            }

            // The part of the page in view is drawn on its own, at full
            // sharpness, when that's worth doing: zoomed in past what the
            // whole-page image holds, or zoomed in on a slow page, whose view
            // alone draws several times faster than the whole of it (0.2 s
            // against 1 s for a dense drawing at twice fit width). It's drawn
            // as squares on a grid, which are kept: zooming back in or scrolling
            // back over an area shows the squares already drawn. The squares in
            // view come first, then those in a margin around it.
            let want = layout.scales[page] * ppp;
            let size = doc.sizes[page] * layout.scales[page];
            let page_rect = Rect::from_min_size(pos2(page_x(content_w, size.x), tops[page]), size);
            let visible = page_rect.intersect(viewport);
            if !visible.is_positive() {
                sharp &= !page_unsharp;
                continue;
            }
            let past_whole_page = scale < want * 0.99;
            let zoomed_into_slow = slow && visible.area() < page_rect.area() * 0.5;
            if !(past_whole_page || zoomed_into_slow) {
                sharp &= !page_unsharp;
                continue;
            }
            // At a quarter power of two, so a small zoom change keeps its squares.
            let tile_scale = quantize_scale(want);
            let full = [
                (doc.sizes[page].x * tile_scale).round().max(1.0) as u32,
                (doc.sizes[page].y * tile_scale).round().max(1.0) as u32,
            ];
            let per_point = vec2(full[0] as f32 / size.x, full[1] as f32 / size.y);
            let span = |lo: f32, hi: f32, origin: f32, k: f32, limit: u32| {
                let a = (((lo - origin) * k).floor().max(0.0) as u32).min(limit);
                let b = (((hi - origin) * k).ceil().max(0.0) as u32).min(limit);
                (a, b)
            };
            let (x0, x1) = span(visible.min.x, visible.max.x, page_rect.min.x, per_point.x, full[0]);
            let (y0, y1) = span(visible.min.y, visible.max.y, page_rect.min.y, per_point.y, full[1]);
            let in_view_area = [x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0)];
            let (mx0, mx1) = (x0.saturating_sub(DETAIL_MARGIN), (x1 + DETAIL_MARGIN).min(full[0]));
            let (my0, my1) = (y0.saturating_sub(DETAIL_MARGIN), (y1 + DETAIL_MARGIN).min(full[1]));
            let around = [mx0, my0, mx1 - mx0, my1 - my0];
            for (column, row) in crate::model::tile_cells(full, around) {
                if let Some(tile) = doc.tiles.get_mut(&TileKey { page, full, column, row, annotations }) {
                    tile.used = now;
                }
            }
            let missing = |area: [u32; 4]| -> Vec<(u32, u32)> {
                crate::model::tile_cells(full, area)
                    .into_iter()
                    .filter(|&(column, row)| !doc.tiles.contains_key(&TileKey { page, full, column, row, annotations }))
                    .collect()
            };
            let in_view_missing = missing(in_view_area);
            if settling && slow && !in_view_missing.is_empty() {
                // Until the zoom settles, the squares from before it stay up --
                // unless every square in view at the new size is already here,
                // as when zooming back in.
                if let Some(&old) = doc.tile_full.get(&page) {
                    tile_full_now.insert(page, old);
                }
                sharp = false;
                continue;
            }
            tile_full_now.insert(page, full);
            if !in_view_missing.is_empty() {
                sharp = false;
            }
            if (settling && slow) || doc.detail_pending.contains(&page) {
                continue;
            }
            let mut cells = in_view_missing;
            if cells.is_empty() {
                cells = missing(around);
            }
            let (Some(c0), Some(c1), Some(r0), Some(r1)) = (
                cells.iter().map(|c| c.0).min(),
                cells.iter().map(|c| c.0).max(),
                cells.iter().map(|c| c.1).min(),
                cells.iter().map(|c| c.1).max(),
            ) else {
                continue;
            };
            // One region covering them, on square edges, no bigger than the
            // largest texture.
            let tile = crate::model::TILE;
            let most = (max_side as u32 / tile).max(1) * tile;
            let (left, top) = (c0 * tile, r0 * tile);
            let right = ((c1 + 1) * tile).min(full[0]).min(left + most);
            let bottom = ((r1 + 1) * tile).min(full[1]).min(top + most);
            if right <= left || bottom <= top {
                continue;
            }
            doc.detail_pending.insert(page);
            let region = [left, top, right - left, bottom - top];
            let _ = self.tx.send(Request::RenderRegion { generation: doc.generation, page, full, region });
        }
        doc.tile_full = tile_full_now;
        self.view_sharp = sharp;
        // Squares aren't needed for pages the GPU draws whole. A page shown
        // from its thumbnail keeps any it has: they are the deeper zoom drawn
        // ahead for it, which is what makes zooming in sharp at once. Unused,
        // they are the first to go when the squares pass their budget.
        let drawn_whole = gpu::pages_drawn_whole(doc);
        doc.tiles.retain(|key, _| !drawn_whole.contains(&key.page));
        // Told to the helpers here, after the loop above decided which pages the
        // GPU keeps: a frame earlier, a page just handed to pdfium would still
        // have counted as the GPU's, and the drawing it is waiting for would be
        // stopped as soon as it was asked for.
        if let Ok(mut wanted) = self.wanted.lock() {
            if *wanted.drawn_whole != drawn_whole {
                wanted.drawn_whole = Arc::new(drawn_whole.clone());
            }
        }

        // Squares are kept while they fit their budget; those wanted on screen
        // longest ago go first.
        let tile_bytes = |t: &TileImage| {
            let [w, h] = t.handle.size();
            w * h * 4
        };
        let mut tile_total: usize = doc.tiles.values().map(tile_bytes).sum();
        if tile_total > self.tile_budget {
            let mut oldest: Vec<(f64, TileKey)> = doc.tiles.iter().map(|(key, t)| (t.used, *key)).collect();
            oldest.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (used, key) in oldest {
                if tile_total <= self.tile_budget || used >= now {
                    break;
                }
                if let Some(t) = doc.tiles.remove(&key) {
                    tile_total -= tile_bytes(&t);
                }
            }
        }
        // Thumbnails of the pages in view, from the cache, so a page scrolled
        // back to shows at once rather than waiting to be read again. A page
        // with none kept is asked about again every few seconds: one may have
        // been kept since, by the page being drawn here or by pdfium.
        for &page in order.iter().chain(&ahead) {
            let asked = doc.thumbs_asked.get(&page).copied();
            if !doc.thumbnails.contains_key(&page) && asked.is_none_or(|asked| now - asked >= THUMBNAIL_RETRY) {
                doc.thumbs_asked.insert(page, now);
                if let Some(thumbs) = &doc.thumbs {
                    thumbs.want(page);
                }
            }
        }
        // With the view drawn and nothing else being read, a page that has no
        // thumbnail is read just to be given one, nearest the view first, so
        // scrolling anywhere in a long document shows the pages rather than
        // blanks. One at a time, and it stops as soon as the view wants
        // anything.
        if sharp && !holding {
            gpu::read_a_thumbnail_ahead(doc, first, now);
        }

        // They are small, but a long document holds many: the ones shown
        // longest ago go once they pass the budget.
        let thumbnail_bytes = |t: &Thumbnail| t.handle.size()[0] * t.handle.size()[1] * 4;
        let mut thumbnails_total: usize = doc.thumbnails.values().map(thumbnail_bytes).sum();
        if thumbnails_total > THUMBNAIL_BUDGET {
            let mut oldest: Vec<(f64, usize)> = doc.thumbnails.iter().map(|(&page, t)| (t.used, page)).collect();
            oldest.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (used, page) in oldest {
                if thumbnails_total <= THUMBNAIL_BUDGET || used >= now {
                    break;
                }
                if let Some(thumbnail) = doc.thumbnails.remove(&page) {
                    thumbnails_total -= thumbnail_bytes(&thumbnail);
                    doc.thumbs_asked.remove(&page);
                }
            }
        }

        // Spare page images only for pages near the view, and within a budget.
        doc.spares.retain(|(p, _), _| (lo..=hi).contains(p));
        let spare_total: usize = doc.spares.values().map(|t| t.handle.size()[0] * t.handle.size()[1] * 4).sum();
        if spare_total > self.spare_budget {
            doc.spares.clear();
        }
        // Traced whenever the textures held move by 32 MB, to see what the
        // process's memory is made of.
        {
            static LAST_STEP: AtomicU64 = AtomicU64::new(0);
            let page_total: usize = doc.textures.values().map(|t| t.handle.size()[0] * t.handle.size()[1] * 4).sum();
            let thumbnail_total: usize = doc.thumbnails.values().map(|t| t.handle.size()[0] * t.handle.size()[1] * 4).sum();
            let (shape_pages, shape_bytes) = gpu::uploaded(doc);
            let step = ((page_total + tile_total + spare_total + thumbnail_total + shape_bytes) >> 25) as u64;
            if LAST_STEP.swap(step, Ordering::Relaxed) != step {
                worker::trace(format_args!(
                    "ui: held: {} MB of pages, {} MB in {} squares, {} MB of spares, {} MB of thumbnails ({}), {} MB of shapes on the GPU ({shape_pages} pages)",
                    page_total >> 20,
                    tile_total >> 20,
                    doc.tiles.len(),
                    spare_total >> 20,
                    thumbnail_total >> 20,
                    doc.thumbnails.len(),
                    shape_bytes >> 20,
                ));
            }
        }

        // Drawing ahead of a zoom. Ctrl+scroll zooms towards the pointer, and
        // Ctrl +/- towards the middle of the view, so once the view is sharp
        // and the pointer has rested, helpers with nothing else to do draw that
        // spot as the deepest zoom would show it: the whole page at the size it
        // would need, then the squares around the spot, nearest first. Zooming
        // in there is then sharp at once -- and at any zoom on the way, since
        // squares from a deeper zoom stand in until the zoom's own arrive.
        match (ctx.input(|i| i.pointer.hover_pos()), self.pointer_rest) {
            (Some(at), Some((resting, _))) if at.distance(resting) < 4.0 => {}
            (Some(at), _) => self.pointer_rest = Some((at, now)),
            (None, _) => self.pointer_rest = None,
        }
        doc.predicting.retain(|_, sent| now - *sent < PREDICT_TIMEOUT);
        let deepest = ZOOMS[ZOOMS.len() - 1];
        let rested = self.pointer_rest.is_none_or(|(_, since)| now - since >= PREDICT_REST);
        if sharp && !holding && rested && self.zoom < deepest * 0.99 && doc.predicting.len() < PREDICT_JOBS {
            let spot = self
                .pointer_rest
                .map(|(at, _)| at - origin.to_vec2())
                .filter(|at| viewport.contains(*at))
                .unwrap_or_else(|| viewport.center());
            let page_rect = |p: usize| {
                let size = doc.sizes[p] * layout.scales[p];
                Rect::from_min_size(pos2(page_x(content_w, size.x), tops[p]), size)
            };
            // A page shown from its thumbnail is drawn ahead of a zoom too --
            // that is what makes zooming straight in from far out sharp at
            // once -- but only the screen the zoom would land on. Drawing the
            // whole sheet ahead at the deepest zoom was filling 320 MB with
            // squares of a zoom nobody had asked for.
            let landing_only = from_thumbnails.contains(&self.current_page);
            if let Some(page) = (first..=last).find(|&p| page_rect(p).contains(spot)).filter(|&p| !gpu::drawn_whole(doc, p)) {
                let rect = page_rect(page);
                let deep = deepest * layout.scales[page] / self.zoom;

                // The squares around the spot first, since they're what the zoom
                // would show: the view it would land on, then half a view
                // further each way once that's done.
                let mut landing_missing = false;
                let tile_scale = quantize_scale(deep * ppp);
                let full = [
                    (doc.sizes[page].x * tile_scale).round().max(1.0) as u32,
                    (doc.sizes[page].y * tile_scale).round().max(1.0) as u32,
                ];
                let edge = vec2(full[0] as f32, full[1] as f32);
                let fraction = (spot - rect.min) / rect.size();
                let centre = vec2(fraction.x * edge.x, fraction.y * edge.y);
                let offset = (spot - viewport.min) * ppp;
                let view = viewport.size() * ppp;
                let tile = crate::model::TILE;
                let spot_cell = ((centre.x.max(0.0) as u32) / tile, (centre.y.max(0.0) as u32) / tile);
                let around: &[f32] = if landing_only { &[0.0] } else { &[0.0, 0.5] };
                for &grow in around {
                    let margin = vec2(DETAIL_MARGIN as f32, DETAIL_MARGIN as f32) + view * grow;
                    let from = (centre - offset - margin).max(Vec2::ZERO);
                    let to = (centre - offset + view + margin).min(edge);
                    if to.x <= from.x || to.y <= from.y {
                        break;
                    }
                    let area = [from.x as u32, from.y as u32, (to.x - from.x) as u32, (to.y - from.y) as u32];
                    let annotations = annotations_drawn(doc, page);
                    let mut missing: Vec<(u32, u32)> = crate::model::tile_cells(full, area)
                        .into_iter()
                        .filter(|&(column, row)| !doc.tiles.contains_key(&TileKey { page, full, column, row, annotations }))
                        .collect();
                    if grow == 0.0 {
                        landing_missing = !missing.is_empty();
                    }
                    if missing.is_empty() {
                        continue;
                    }
                    missing.sort_by_key(|&(column, row)| column.abs_diff(spot_cell.0).max(row.abs_diff(spot_cell.1)));
                    for (column, row) in missing {
                        if doc.predicting.len() >= PREDICT_JOBS {
                            break;
                        }
                        let (bx, by) = (column / PREDICT_BLOCK * PREDICT_BLOCK * tile, row / PREDICT_BLOCK * PREDICT_BLOCK * tile);
                        let block = [bx, by, (PREDICT_BLOCK * tile).min(full[0] - bx), (PREDICT_BLOCK * tile).min(full[1] - by)];
                        let key = PredictKey::Region(page, block);
                        if doc.predicting.contains_key(&key) {
                            continue;
                        }
                        doc.predicting.insert(key, now);
                        let _ = self.tx.send(Request::PredictRegion { generation: doc.generation, page, full, region: block });
                    }
                    // The nearer squares first; the further ones once they're done.
                    break;
                }

                // Then, once the view the zoom would land on is covered, the
                // whole page at the size it would be drawn at: the backdrop to
                // the squares, which on an overlay takes seconds on its own.
                let page_scale = render_scale(doc.sizes[page], deep, ppp, max_side);
                let key = PredictKey::Page(page, page_scale.to_bits());
                let have_page = doc.textures.get(&page).is_some_and(|t| t.complete && (t.scale - page_scale).abs() <= 1e-3)
                    || doc.spares.contains_key(&(page, page_scale.to_bits()));
                // The backdrop to those squares is a whole sheet at the
                // deepest zoom, which is worth having only once the page is
                // being looked at, not while it is an inch across.
                if !landing_only && !landing_missing && !have_page && doc.predicting.len() < PREDICT_JOBS && !doc.predicting.contains_key(&key) {
                    doc.predicting.insert(key, now);
                    let _ = self.tx.send(Request::PredictPage { generation: doc.generation, page, scale: page_scale });
                }
            }
        }

        // Let go of page images that have scrolled away: always keep the ones
        // asked for above, then the nearest others while they fit the budget.
        let distance = |p: usize| if p < first { first - p } else { p.saturating_sub(last) };
        let mut others: Vec<(usize, usize)> =
            doc.textures.keys().filter(|p| !(lo..=hi).contains(*p)).map(|&p| (distance(p), p)).filter(|(d, _)| *d <= 4).collect();
        others.sort_unstable();
        let mut kept: HashSet<usize> = (lo..=hi).collect();
        let mut bytes = 0;
        for (_, p) in others {
            let [w, h] = doc.textures[&p].handle.size();
            if bytes + w * h * 4 <= TEXTURE_BUDGET {
                bytes += w * h * 4;
                kept.insert(p);
            }
        }
        doc.textures.retain(|p, _| kept.contains(p) && !drawn_whole.contains(p));
        if let Some(gpu) = &self.gpu {
            gpu.keep_uploads_near(doc, first, last, &from_thumbnails);
        }
        let keep = first.saturating_sub(12)..=last + 12;
        doc.text.retain(|p, _| keep.contains(p));

        let painter = ui.painter();
        let screen_view = viewport.translate(origin.to_vec2());
        let page_shadow = Shadow { offset: [0, 2], blur: 12, spread: 0, color: Color32::from_black_alpha(34) };
        // Pages in view with nothing of their own on screen: paper with their
        // thumbnail stretched over it, standing in until something draws them.
        let mut stood_in_for = 0usize;
        self.blank_pages.clear();
        for page in first..=last {
            let scale = layout.scales[page];
            let size = doc.sizes[page] * scale;
            let rect = Rect::from_min_size(origin + vec2(page_x(content_w, size.x), tops[page]), size);
            self.page_rects.insert(page, rect);
            let geometry = doc.geometry[page];

            painter.add(page_shadow.as_shape(rect, CornerRadius::same(2)));

            // While zooming, the old texture stretches to fit until the sharp
            // one arrives, which beats flashing a blank page.
            // A page the GPU draws whole is paper, with the GPU's drawing on it.
            // One shown from its thumbnail has neither.
            let whole = gpu::drawn_whole(doc, page) && !from_thumbnails.contains(&page);
            let texture: Option<TextureId> = doc.textures.get(&page).filter(|_| !whole && !from_thumbnails.contains(&page)).map(|t| t.handle.id());
            match texture {
                Some(id) => {
                    painter.image(id, rect, UV_FULL, Color32::WHITE);
                }
                None if whole => {
                    painter.rect_filled(rect, CornerRadius::same(0), Color32::WHITE);
                }
                None => {
                    if !from_thumbnails.contains(&page) && !doc.tile_full.contains_key(&page) {
                        stood_in_for += 1;
                    }
                    painter.rect_filled(rect, CornerRadius::same(0), Color32::WHITE);
                    // Its thumbnail, until whatever draws it properly arrives:
                    // soft, but the page rather than a blank. Drawn much bigger
                    // than the thumbnail it stops being a picture of the page
                    // and becomes a smear, which on a dense drawing reads as the
                    // screen going dark, so it fades into the paper as it is
                    // stretched -- never dark, never blank either.
                    let stretch = rect.width() * ppp / crate::model::THUMBNAIL_WIDTH as f32;
                    match doc.thumbnails.get_mut(&page) {
                        Some(thumbnail) => {
                            thumbnail.used = now;
                            let over = (stretch - MOST_THUMBNAIL_STRETCH) / (THUMBNAIL_FADED_AT - MOST_THUMBNAIL_STRETCH);
                            let left = 1.0 - over.clamp(0.0, 1.0) * (1.0 - FAINTEST_THUMBNAIL);
                            let tint = Color32::from_white_alpha((left * 255.0).round() as u8);
                            painter.image(thumbnail.handle.id(), rect, UV_FULL, tint);
                        }
                        None => {
                            // Blank: nothing of the page at all.
                            self.blank_pages.push(page);
                            painter.text(rect.center(), Align2::CENTER_CENTER, format!("Page {}", page + 1), FontId::proportional(12.0), SUBTLE);
                        }
                    }
                }
            }
            // Squares drawn zoomed in, over the whole-page image.
            // Squares from other zooms stand in until this zoom's own arrive:
            // coarser ones first, sharper ones over them, this zoom's on top.
            let detail: Vec<(TextureId, Rect)> = match doc.tile_full.get(&page).filter(|_| !whole) {
                Some(&full) => {
                    let mut pieces: Vec<(u32, TextureId, Rect)> = doc
                        .tiles
                        .iter()
                        .filter(|(key, _)| key.page == page && key.annotations == annotations_drawn(doc, page))
                        .map(|(key, tile)| (key.full[0], tile.handle.id(), tile_screen_rect(rect, key.full, key.column, key.row)))
                        .filter(|(_, _, area)| area.intersects(screen_view))
                        .collect();
                    pieces.sort_by_key(|&(width, ..)| if width == full[0] { u32::MAX } else { width });
                    pieces.into_iter().map(|(_, id, area)| (id, area)).collect()
                }
                None => Vec::new(),
            };
            for &(id, area) in &detail {
                painter.image(id, area, UV_FULL, Color32::WHITE);
            }
            painter.rect_stroke(rect, CornerRadius::same(0), Stroke::new(1.0, Color32::from_black_alpha(14)), StrokeKind::Outside);

            // Highlights and search matches tint what's under them, as a
            // highlighter does. Where the GPU draws over the page they go with
            // its drawing, over every shape; elsewhere they tint the page's
            // image. Outlines go over either.
            // A page shown from its thumbnail may keep its shapes for zooming in,
            // but the thumbnail is all there is room to show of it.
            let gpu_layer = self.gpu.as_ref().filter(|_| gpu::draws_over(doc, page) && !from_thumbnails.contains(&page));
            let mut marks: Vec<(Rect, Color32)> = Vec::new();
            let mut outlines: Vec<(Rect, Stroke)> = Vec::new();
            let mut mark = |area: Rect, colour: Color32| match gpu_layer {
                Some(_) => marks.push((area, colour)),
                None => paint_highlight(painter, texture, &detail, rect, area, colour),
            };
            if let Some(g) = geometry {
                for e in doc.session.highlights().iter().filter(|e| e.hl.page == page) {
                    for q in &e.hl.quads {
                        let r = to_screen(rect, &g, q).intersect(rect);
                        if !r.is_positive() {
                            continue;
                        }
                        mark(r, to_color32(e.hl.color));
                        if active == Some(e.uid) {
                            outlines.push((r.expand(1.0), Stroke::new(2.0, ACCENT)));
                        }
                    }
                }

                // Search matches, found by binary search since they're in page order.
                let hits = &self.search.hits;
                let from = hits.partition_point(|h| h.page < page);
                let to = hits.partition_point(|h| h.page <= page);
                for (i, hit) in hits.iter().enumerate().take(to).skip(from) {
                    let current = self.search.current == Some(i);
                    for q in &hit.quads {
                        let r = to_screen(rect, &g, q).intersect(rect);
                        if !r.is_positive() {
                            continue;
                        }
                        mark(r, if current { HIT_CURRENT } else { HIT });
                        if current {
                            outlines.push((r.expand(1.5), Stroke::new(2.0, HIT_OUTLINE)));
                        }
                    }
                }
            }
            if let Some(gpu) = gpu_layer {
                gpu.paint_page(painter, doc, page, rect, screen_view, &marks);
            }
            for (area, stroke) in outlines {
                painter.rect_stroke(area, CornerRadius::same(2), stroke, StrokeKind::Outside);
            }
            if let Some(g) = geometry {
                paint_markups(painter, doc, page, rect, &g, active, self.drag.as_ref());
            }
            if let (Some(g), Some(line)) = (geometry, calibrating.filter(|(on, ..)| *on == page)) {
                let scale = page_scale(doc, page);
                paint_calibration(painter, line, scale, rect, &g);
            }

            if let Some(g) = geometry {
                if let Some(chars) = doc.text.get(&page) {
                    for (_, range) in segments.iter().filter(|(p, _)| *p == page) {
                        for band in selection::bands(chars, range.clone()) {
                            painter.rect_filled(to_screen(rect, &g, &band), CornerRadius::same(0), SELECTION);
                        }
                    }
                }

                // The box being drawn, with Ctrl held.
                if let Some(Drag::Box { page: box_page, start, end }) = self.drag {
                    if box_page == page {
                        let area = to_screen(rect, &g, &box_between(start, end));
                        painter.rect_filled(area, CornerRadius::same(0), ACCENT.gamma_multiply(0.06));
                        painter.rect_stroke(area, CornerRadius::same(0), Stroke::new(1.0, ACCENT), StrokeKind::Inside);
                    }
                }
            }

            if !busy {
                let response = ui.interact(rect, Id::new(("page", page)), Sense::click_and_drag());
                if let (Some(pos), Some(g)) = (response.hover_pos(), geometry) {
                    let (px, py) = to_pdf(rect, &g, pos);
                    let over_highlight =
                        doc.session.highlights().iter().any(|e| e.hl.page == page && e.hl.quads.iter().any(|q| q.contains(px, py)));
                    let over_text = doc
                        .text
                        .get(&page)
                        .is_some_and(|chars| chars.iter().any(|c| c.bounds.is_some_and(|b| b.contains(px, py))));
                    if self.tool.is_some() || self.measure_tool.is_some() || ctx.input(|i| i.modifiers.command) {
                        // A drawing tool, or Ctrl held for a box.
                        ctx.set_cursor_icon(CursorIcon::Crosshair);
                    } else if over_highlight || markup_at(doc, page, rect, (px, py)).is_some() {
                        ctx.set_cursor_icon(CursorIcon::PointingHand);
                    } else if over_text {
                        ctx.set_cursor_icon(CursorIcon::Text);
                    }
                }
                // Only the left button selects text or opens a highlight; the
                // middle button is for moving the document.
                if response.drag_started_by(egui::PointerButton::Primary) {
                    drag_start = ctx.input(|i| i.pointer.press_origin()).map(|pos| (page, pos));
                }
                if response.clicked_by(egui::PointerButton::Primary) {
                    clicked = response.interact_pointer_pos().map(|pos| (page, pos));
                }
            }

            // Oversized pages say how they're shown, and switch between shrunk
            // and actual size. Registered after the page, so it gets the click.
            if doc.sizes[page].x > doc.usual_size.x * OVERSIZED {
                let label = if shrink_wide {
                    format!("Shrunk to fit, {:.0}%  ·  Show actual size", scale * 100.0)
                } else {
                    "Actual size  ·  Shrink to fit".to_owned()
                };
                if size_badge(ui, rect, page, &label).clicked() {
                    toggle_shrink = true;
                }
            }
        }

        self.view_stood_in = stood_in_for > 0;
        if toggle_shrink {
            self.set_shrink_wide(!shrink_wide);
        }
        if let Some((page, pos)) = drag_start {
            // A drawing tool draws; with Ctrl held, the drag draws a box
            // instead of following the text.
            if self.measure_tool.is_some() {
                self.start_calibration(page, pos);
            } else if self.tool.is_some() {
                self.start_markup(page, pos);
            } else if ctx.input(|i| i.modifiers.command) {
                if let Some(point) = self.pdf_point(page, pos) {
                    self.drag = Some(Drag::Box { page, start: point, end: point });
                    self.popup = None;
                }
            } else if let Some(caret) = self.caret_for(page, pos) {
                self.drag = Some(Drag::Text { anchor: (page, caret), focus: (page, caret) });
                self.popup = None;
            }
        }
        if self.drag.is_some() {
            self.update_drag(ui);
        }
        self.paint_snap(ui);
        if let Some((page, pos)) = clicked {
            self.click_page(page, pos);
        }
    }
}
