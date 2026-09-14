//! Packing images into atlas pages: square textures the renderer uploads as
//! one texture array, so images draw among the other shapes without changing
//! texture.

use crate::image::Bitmap;

/// The side of an atlas page, in pixels. OpenGL ES 3.0 and WebGL 2 promise
/// textures this big.
pub const ATLAS_SIZE: u32 = 2048;

/// Where an image went in the atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub page: u32,
    /// Its left, top, right and bottom edges, as fractions of the page.
    pub uv: [f32; 4],
}

/// Images packed onto pages in shelves: rows of images of about one height.
#[derive(Debug, Default)]
pub struct Atlas {
    /// Each page's pixels, `ATLAS_SIZE` square: red, green, blue and alpha,
    /// premultiplied, rows from the top.
    pub pages: Vec<Vec<u8>>,
    shelves: Vec<Shelf>,
    /// How far down each page its shelves reach.
    bottoms: Vec<u32>,
}

#[derive(Debug)]
struct Shelf {
    page: u32,
    top: u32,
    height: u32,
    /// How far along it's filled.
    right: u32,
}

impl Atlas {
    /// Copies `bitmap` onto a page -- shrunk, if it's too big for one -- with
    /// its edge pixels repeated around it, so filtering at its edges doesn't
    /// take in its neighbours.
    pub(crate) fn add(&mut self, bitmap: &Bitmap) -> Placed {
        let shrunk;
        let bitmap = if bitmap.width.max(bitmap.height) > ATLAS_SIZE - 2 {
            shrunk = shrink(bitmap, ATLAS_SIZE - 2);
            &shrunk
        } else {
            bitmap
        };
        let (page, left, top) = self.space_for(bitmap.width + 2, bitmap.height + 2);
        self.copy(page, left, top, bitmap);
        let fraction = |pixels: u32| pixels as f32 / ATLAS_SIZE as f32;
        Placed { page, uv: [fraction(left + 1), fraction(top + 1), fraction(left + 1 + bitmap.width), fraction(top + 1 + bitmap.height)] }
    }

    /// A page and top left corner with room for `width` by `height` pixels:
    /// along a shelf that's tall enough but not twice as tall, else on a new
    /// shelf on the first page with room below its shelves, else on a new
    /// page.
    fn space_for(&mut self, width: u32, height: u32) -> (u32, u32, u32) {
        if let Some(shelf) = self.shelves.iter_mut().find(|s| s.height >= height && s.height <= height * 2 && s.right + width <= ATLAS_SIZE) {
            shelf.right += width;
            return (shelf.page, shelf.right - width, shelf.top);
        }
        let page = match self.bottoms.iter().position(|&bottom| bottom + height <= ATLAS_SIZE) {
            Some(page) => page,
            None => {
                self.pages.push(vec![0; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize]);
                self.bottoms.push(0);
                self.pages.len() - 1
            }
        };
        let top = self.bottoms[page];
        self.bottoms[page] += height;
        self.shelves.push(Shelf { page: page as u32, top, height, right: width });
        (page as u32, 0, top)
    }

    /// Copies `bitmap` a pixel in from `left`, `top`, its edges repeated into
    /// that pixel around it.
    fn copy(&mut self, page: u32, left: u32, top: u32, bitmap: &Bitmap) {
        let pixels = &mut self.pages[page as usize];
        let (width, height) = (bitmap.width as usize, bitmap.height as usize);
        for y in 0..height + 2 {
            let from_row = y.clamp(1, height) - 1;
            for x in 0..width + 2 {
                let from = (from_row * width + x.clamp(1, width) - 1) * 4;
                let to = ((top as usize + y) * ATLAS_SIZE as usize + left as usize + x) * 4;
                pixels[to..to + 4].copy_from_slice(&bitmap.pixels[from..from + 4]);
            }
        }
    }
}

/// `bitmap` scaled down so neither side is over `most` pixels, taking the
/// nearest pixel.
fn shrink(bitmap: &Bitmap, most: u32) -> Bitmap {
    let scale = most as f32 / bitmap.width.max(bitmap.height) as f32;
    let side = |pixels: u32| ((pixels as f32 * scale) as u32).clamp(1, most);
    let (width, height) = (side(bitmap.width), side(bitmap.height));
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        let from_y = (u64::from(y) * u64::from(bitmap.height) / u64::from(height)) as usize;
        for x in 0..width {
            let from_x = (u64::from(x) * u64::from(bitmap.width) / u64::from(width)) as usize;
            let from = (from_y * bitmap.width as usize + from_x) * 4;
            pixels.extend_from_slice(&bitmap.pixels[from..from + 4]);
        }
    }
    Bitmap { width, height, pixels }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bitmap whose pixels are numbered from 1 in their red byte, counting
    /// round again after 255.
    fn numbered(width: u32, height: u32) -> Bitmap {
        let pixels = (0..width * height).flat_map(|i| [(i % 255) as u8 + 1, 0, 0, 255]).collect();
        Bitmap { width, height, pixels }
    }

    fn red_at(atlas: &Atlas, page: usize, x: u32, y: u32) -> u8 {
        atlas.pages[page][((y * ATLAS_SIZE + x) * 4) as usize]
    }

    #[test]
    fn an_image_is_copied_with_its_edges_repeated_around_it() {
        let mut atlas = Atlas::default();
        let placed = atlas.add(&numbered(2, 1));
        assert_eq!(placed.page, 0);
        assert_eq!(placed.uv.map(|f| (f * ATLAS_SIZE as f32).round() as u32), [1, 1, 3, 2]);
        let row = |y| (0..4).map(|x| red_at(&atlas, 0, x, y)).collect::<Vec<_>>();
        assert_eq!([row(0), row(1), row(2)], [[1, 1, 2, 2], [1, 1, 2, 2], [1, 1, 2, 2]]);
    }

    #[test]
    fn images_share_shelves_of_about_their_height_and_pages_fill_up() {
        let mut atlas = Atlas::default();
        let first = atlas.add(&numbered(10, 10));
        let beside = atlas.add(&numbered(10, 8));
        let below = atlas.add(&numbered(10, 2));
        assert_eq!(beside.uv[1], first.uv[1], "on the same shelf");
        assert!(below.uv[1] > first.uv[3], "too short for it, so on a new shelf");
        for _ in 0..5 {
            atlas.add(&numbered(1000, 1000));
        }
        assert_eq!(atlas.pages.len(), 2, "four of them fit a page, two to a shelf; the fifth doesn't");
        let low = atlas.add(&numbered(10, 10));
        assert_eq!(low.page, 0, "a small one still fits on the first page's shelves");
        let flat = atlas.add(&numbered(2000, 20));
        assert_eq!(flat.page, 0, "and a new shelf still fits below the first page's shelves");
    }

    #[test]
    fn an_image_too_big_for_a_page_is_shrunk() {
        let mut atlas = Atlas::default();
        let placed = atlas.add(&numbered(ATLAS_SIZE * 2, 2));
        assert_eq!(((placed.uv[2] - placed.uv[0]) * ATLAS_SIZE as f32).round() as u32, ATLAS_SIZE - 2);
    }
}
