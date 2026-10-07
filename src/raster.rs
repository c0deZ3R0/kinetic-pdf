//! Page image tiling and thumbnail utilities shared by rendering services.

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

/// How wide a page's thumbnail is kept, in pixels: enough to cover a sheet
/// shown at the smallest zoom on a high-resolution screen, where it is about
/// 480 pixels across, so the page can be shown from its thumbnail alone with
/// nothing lost. One costs three quarters of a megabyte in memory, against
/// the 18 MB a sheet's shapes come to, or about 100 KB kept on disk.
pub const THUMBNAIL_WIDTH: u32 = 512;

/// An opaque RGBA image averaged down to `width` pixels across, keeping its
/// shape: a page's thumbnail, made from an image pdfium drew of it. `None` if
/// the image is already no wider than that, or isn't the size it claims.
pub fn thumbnail(size: [usize; 2], rgba: &[u8], width: usize) -> Option<([usize; 2], Vec<u8>)> {
    let [from_width, from_height] = size;
    if rgba.len() != from_width * from_height * 4 || from_width <= width || width == 0 || from_height == 0 {
        return None;
    }
    let height = (width * from_height / from_width).max(1);
    // Which new pixel each old column falls in, worked out once, not per pixel.
    let column: Vec<usize> = (0..from_width).map(|x| x * width / from_width).collect();
    let mut sums = vec![[0_u32; 4]; width * height];
    let mut counts = vec![0_u32; width * height];
    for y in 0..from_height {
        let row = &rgba[y * from_width * 4..];
        let into = y * height / from_height * width;
        for (x, &column) in column.iter().enumerate() {
            let at = into + column;
            for (total, &byte) in sums[at].iter_mut().zip(&row[x * 4..x * 4 + 4]) {
                *total += u32::from(byte);
            }
            counts[at] += 1;
        }
    }
    let pixels = sums
        .iter()
        .zip(&counts)
        .flat_map(|(sum, &count)| {
            let count = count.max(1);
            [0, 1, 2, 3].map(|channel| ((sum[channel] + count / 2) / count) as u8)
        })
        .collect();
    Some(([width, height], pixels))
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
