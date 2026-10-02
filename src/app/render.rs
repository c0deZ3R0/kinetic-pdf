//! Per-document rendering resources and cache invalidation.
//! Editing and undo state remain in the session and arrangement.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use eframe::egui::TextureHandle;

use super::gpu;

pub(super) struct PageTexture {
    pub(super) handle: TextureHandle,
    /// Pixels per PDF point it was rendered at.
    pub(super) scale: f32,
    /// False while a slow page is still drawing in.
    pub(super) complete: bool,
    /// Whether pdfium drew the page's annotations in it.
    pub(super) annotations: bool,
}

/// One square of a page drawn zoomed in, placed on the page's grid (see
/// `raster::TILE`): the page, its full size in pixels at that zoom, the
/// square's column and row, and whether pdfium drew the page's annotations.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TileKey {
    pub(super) page: usize,
    pub(super) full: [u32; 2],
    pub(super) column: u32,
    pub(super) row: u32,
    pub(super) annotations: bool,
}

/// Something asked for ahead of a zoom: a whole page at a scale, or a region
/// of a page.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PredictKey {
    Page(usize, u32),
    Region(usize, [u32; 4]),
}

pub(super) struct TileImage {
    pub(super) handle: TextureHandle,
    /// When it was last wanted on screen, so the least recently used go first.
    pub(super) used: f64,
}

/// A small image of a page, kept for the whole document so a page scrolled
/// back to shows something at once; see `gpu::THUMBNAIL_WIDTH`.
pub(super) struct Thumbnail {
    pub(super) handle: TextureHandle,
    /// When it was last shown, so the ones furthest from the view go first.
    pub(super) used: f64,
}

#[derive(Default)]
pub(super) struct RenderState {
    /// Pages whose drawing is out of date since a save changed their markups,
    /// until they're drawn again; meanwhile the markups just saved show as
    /// drawn here.
    pub(super) redraw: HashSet<usize>,
    /// Erasures the last save wrote, still shown as paper on the pages in
    /// `redraw` until they are drawn again without what was erased.
    pub(super) erasures_written: Vec<crate::domain::Erasure>,
    /// Until when its layers are being faded, so drawn straight rather than
    /// into squares.
    pub(super) fading_until: f64,
    pub(super) textures: HashMap<usize, PageTexture>,
    pub(super) render_pending: HashSet<usize>,
    /// Squares of pages drawn zoomed in, kept while they fit `TILE_BUDGET`, so
    /// zooming back in or scrolling back over an area shows them at once.
    pub(super) tiles: HashMap<TileKey, TileImage>,
    /// For each page drawn zoomed in this frame, its full size in pixels,
    /// which says which of its squares to draw.
    pub(super) tile_full: HashMap<usize, [u32; 2]>,
    /// Earlier whole-page images, by page and scale, for zooming back to them
    /// -- and whole pages drawn ahead of a zoom, for zooming to them.
    pub(super) spares: HashMap<(usize, u32), PageTexture>,
    /// What's been asked for ahead of a zoom, and when.
    pub(super) predicting: HashMap<PredictKey, f64>,
    pub(super) detail_pending: HashSet<usize>,
    pub(super) failed: HashSet<usize>,
    /// Pages that were slow to draw: zooming redraws them only once the zoom
    /// settles, and zoomed in, the part in view is drawn on its own first.
    pub(super) slow: HashSet<usize>,
    /// Who draws each page, pdfium or the GPU, once asked.
    pub(super) drawing: HashMap<usize, gpu::PageDrawing>,
    /// Reads pages into shapes for the GPU, if there's one.
    pub(super) reader: Option<gpu::Reader>,
    /// The page whose shapes are on their way to the GPU.
    pub(super) uploading: Option<gpu::Uploading>,
    /// Thumbnails the GPU is drawing, collected once it has.
    pub(super) thumbnails_drawing: Vec<gpu::DrawingThumbnail>,
    /// Squares heavy pages on the GPU are drawn into (`gpu::Tiles`).
    pub(super) gpu_tiles: gpu::Tiles,
    /// A small image of each page seen, shown while what draws it properly is
    /// on its way, and kept in the page cache between sessions.
    pub(super) thumbnails: HashMap<usize, Thumbnail>,
    /// Visible sheets held across a save refresh until fresh drawings arrive.
    /// These are display-only, with explicit turns; never put in the cache.
    pub(super) save_previews: HashMap<usize, (TextureHandle, u8)>,
    /// When each page's thumbnail was last asked of the cache. Asked again
    /// after a while, since one may have been kept since: the page may have
    /// been drawn, by pdfium or here, after the first time it was asked for.
    pub(super) thumbs_asked: HashMap<usize, f64>,
    /// Reads thumbnails back from the cache.
    pub(super) thumbs: Option<gpu::Thumbnails>,
    /// Pages read ahead just to be thumbnailed, so each is tried once.
    pub(super) thumbs_ahead: HashSet<usize>,
    /// The page the shapes reader is on, if any, and whether just for its
    /// thumbnail. It gives way to a page wanted more that is waiting.
    pub(super) reading: Option<(usize, bool)>,
    /// The lines of each page read for snapping, kept under `SNAP_BUDGET`
    /// for the pages near the view.
    pub(super) snap: HashMap<usize, Arc<markup_model::SnapIndex>>,
    /// Pages whose lines have been asked for, so they are asked once.
    pub(super) snap_asked: HashSet<usize>,
    /// Pages that took a while to read into shapes, whose shapes are kept
    /// even while the page is small enough to show from its thumbnail.
    pub(super) slow_to_read: HashSet<usize>,
    /// What each page's shapes came to when it was read, which says what they
    /// would come to at another zoom; see `gpu::Sizes`.
    pub(super) shape_sizes: HashMap<usize, gpu::Sizes>,
    /// Shapes let go where there was no GPU in hand to free them; freed on the
    /// next frame.
    pub(super) releasing: Vec<Arc<gpu::Uploaded>>,
    /// Pages too big for the GPU at the zoom in view, which pdfium is drawing
    /// but whose shapes still draw them until it has (`finish_handing_over`).
    pub(super) handing_over: HashSet<usize>,
    /// Pages the GPU has nothing to draw of, whatever the zoom: reading them
    /// came back with nothing it could use, so they are never read again --
    /// without this they are read, turned down and read again, every frame.
    pub(super) left_to_pdfium: HashSet<usize>,
}

impl RenderState {
    /// Advance caches after a committed save. Changed pages retain their old
    /// drawing as a preview until a fresh one arrives; pending thumbnails for
    /// unchanged pages can safely use the committed revision's cache key.
    pub(super) fn revision_committed(
        &mut self,
        previous: u64,
        committed: u64,
        changed: &[usize],
        thumbs: Option<gpu::Thumbnails>,
    ) {
        for thumbnail in &mut self.thumbnails_drawing {
            thumbnail.revision_committed(previous, committed, changed);
        }
        self.thumbs = thumbs;
        self.thumbs_asked.clear();
        for &page in changed {
            self.redraw.insert(page);
            if let Some(texture) = self.textures.get_mut(&page) {
                texture.complete = false;
            }
        }
        self.tiles.retain(|key, _| !changed.contains(&key.page));
        self.spares.retain(|(page, _), _| !changed.contains(page));
    }
}
