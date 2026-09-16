//! Packing images into atlas pages: square textures the renderer uploads as
//! one texture array, so images draw among the other shapes without changing
//! texture.

use crate::image::Bitmap;

/// The side of an atlas page, in pixels. OpenGL ES 3.0 and WebGL 2 promise
/// textures this big.
pub const ATLAS_SIZE: u32 = 2048;

/// Where an image, or part of one, went in the atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub page: u32,
    /// Its left, top, right and bottom edges, as fractions of the page.
    pub uv: [f32; 4],
}

/// One part of an image: where it went, and which part of the image it is --
/// its left, top, right and bottom edges as fractions of the image, from its
/// top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    pub placed: Placed,
    pub of_image: [f32; 4],
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
    /// How far down any page is used: the pages' rows below it are empty, so
    /// needn't go to the GPU.
    pub fn height(&self) -> u32 {
        self.bottoms.iter().copied().max().unwrap_or(0)
    }

    /// An atlas read back from the cache: its pages' used rows, and how far
    /// down they're used. Nothing more can be packed into it -- what's drawn
    /// already holds where its images went.
    pub(crate) fn restored(pages: Vec<Vec<u8>>, height: u32) -> Atlas {
        let bottoms = vec![height; pages.len()];
        Atlas { pages, shelves: Vec::new(), bottoms }
    }

    /// Copies `bitmap` onto the pages at full size: whole, or cut into parts
    /// where it's too big for a page. Each part has a pixel around it from
    /// what's beside it in the image, or its own edge repeated at the image's
    /// edge, so filtering neither takes in other images nor shows a seam
    /// between parts.
    pub(crate) fn add(&mut self, bitmap: &Bitmap) -> Vec<Part> {
        let most = ATLAS_SIZE - 2;
        let (width, height) = (bitmap.width, bitmap.height);
        let mut parts = Vec::new();
        for top in (0..height).step_by(most as usize) {
            for left in (0..width).step_by(most as usize) {
                let part = [left, top, most.min(width - left), most.min(height - top)];
                let placed = self.place(bitmap, part);
                let of = |pixels: u32, whole: u32| pixels as f32 / whole as f32;
                let of_image = [of(left, width), of(top, height), of(left + part[2], width), of(top + part[3], height)];
                parts.push(Part { placed, of_image });
            }
        }
        parts
    }

    /// Copies `part` of `bitmap` -- left, top, width and height -- onto a page.
    fn place(&mut self, bitmap: &Bitmap, part: [u32; 4]) -> Placed {
        let [_, _, width, height] = part;
        let (page, left, top) = self.space_for(width + 2, height + 2);
        self.copy(page, left, top, bitmap, part);
        let fraction = |pixels: u32| pixels as f32 / ATLAS_SIZE as f32;
        Placed { page, uv: [fraction(left + 1), fraction(top + 1), fraction(left + 1 + width), fraction(top + 1 + height)] }
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

    /// Copies `part` of `bitmap` a pixel in from `left`, `top`, with the pixels
    /// beside the part in the image -- or at the image's edge, its own -- in
    /// that pixel around it.
    fn copy(&mut self, page: u32, left: u32, top: u32, bitmap: &Bitmap, [part_left, part_top, width, height]: [u32; 4]) {
        let pixels = &mut self.pages[page as usize];
        let beside = |start: u32, offset: usize, whole: u32| (i64::from(start) + offset as i64 - 1).clamp(0, i64::from(whole) - 1) as usize;
        for y in 0..height as usize + 2 {
            let from_row = beside(part_top, y, bitmap.height);
            for x in 0..width as usize + 2 {
                let from = (from_row * bitmap.width as usize + beside(part_left, x, bitmap.width)) * 4;
                let to = ((top as usize + y) * ATLAS_SIZE as usize + left as usize + x) * 4;
                pixels[to..to + 4].copy_from_slice(&bitmap.pixels[from..from + 4]);
            }
        }
    }
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

    /// The one part a small image goes in.
    fn whole(parts: Vec<Part>) -> Placed {
        let [part] = &parts[..] else { panic!("one part: {parts:?}") };
        assert_eq!(part.of_image, [0.0, 0.0, 1.0, 1.0]);
        part.placed
    }

    #[test]
    fn an_image_is_copied_with_its_edges_repeated_around_it() {
        let mut atlas = Atlas::default();
        let placed = whole(atlas.add(&numbered(2, 1)));
        assert_eq!(placed.page, 0);
        assert_eq!(placed.uv.map(|f| (f * ATLAS_SIZE as f32).round() as u32), [1, 1, 3, 2]);
        let row = |y| (0..4).map(|x| red_at(&atlas, 0, x, y)).collect::<Vec<_>>();
        assert_eq!([row(0), row(1), row(2)], [[1, 1, 2, 2], [1, 1, 2, 2], [1, 1, 2, 2]]);
    }

    #[test]
    fn images_share_shelves_of_about_their_height_and_pages_fill_up() {
        let mut atlas = Atlas::default();
        let first = whole(atlas.add(&numbered(10, 10)));
        assert_eq!(atlas.height(), 12, "the image and the pixel around it");
        let beside = whole(atlas.add(&numbered(10, 8)));
        let below = whole(atlas.add(&numbered(10, 2)));
        assert_eq!(beside.uv[1], first.uv[1], "on the same shelf");
        assert!(below.uv[1] > first.uv[3], "too short for it, so on a new shelf");
        for _ in 0..5 {
            atlas.add(&numbered(1000, 1000));
        }
        assert_eq!(atlas.pages.len(), 2, "four of them fit a page, two to a shelf; the fifth doesn't");
        let low = whole(atlas.add(&numbered(10, 10)));
        assert_eq!(low.page, 0, "a small one still fits on the first page's shelves");
        let flat = whole(atlas.add(&numbered(2000, 20)));
        assert_eq!(flat.page, 0, "and a new shelf still fits below the first page's shelves");
    }

    #[test]
    fn an_image_too_big_for_a_page_is_cut_into_parts_bordered_by_their_neighbours() {
        let mut atlas = Atlas::default();
        let most = ATLAS_SIZE - 2;
        let parts = atlas.add(&numbered(most + 3, 1));
        let fractions: Vec<[f32; 4]> = parts.iter().map(|p| p.of_image).collect();
        let split = most as f32 / (most + 3) as f32;
        assert_eq!(fractions, [[0.0, 0.0, split, 1.0], [split, 0.0, 1.0, 1.0]], "full size, in two parts");
        let second = parts[1].placed;
        let [left, top] = [second.uv[0], second.uv[1]].map(|f| (f * ATLAS_SIZE as f32).round() as u32);
        let red = |x: u32| red_at(&atlas, second.page as usize, x, top);
        let numbered_at = |x: u32| (x % 255) as u8 + 1;
        assert_eq!([red(left - 1), red(left), red(left + 3)], [numbered_at(most - 1), numbered_at(most), numbered_at(most + 2)], "the pixel before the part is its neighbour's; the one after, its own edge");
    }
}
