//! Deep zoom draws only the part of a page in view, cut into grid squares and
//! drawn over the whole-page image. Each square has to match the same pixels
//! of a whole-page render exactly, on rotated pages too, or the sharp view
//! would sit shifted against the page under it.

mod common;

use std::collections::HashMap;

use common::{build_pdf_rotated, next_reply, scratch_dir, start_worker_with_context, take_pixels};
use eframe::egui::TextureHandle;
use pdf_annotate::model::{tile_rect, Reply, Request, TILE};

#[test]
fn every_square_matches_the_same_pixels_of_the_whole_page() {
    let dir = scratch_dir("region-test");
    let path = dir.join("doc.pdf");
    let rotations = [0, 90];
    std::fs::write(&path, build_pdf_rotated(&rotations, &[])).unwrap();

    let (tx, rx, wanted, ctx) = start_worker_with_context();
    tx.send(Request::Open { generation: 1, path }).unwrap();
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages.extend(0..rotations.len());
    }
    for page in 0..rotations.len() {
        tx.send(Request::Render { generation: 1, page, scale: 1.5 }).unwrap();
    }
    let mut whole: HashMap<usize, TextureHandle> = HashMap::new();
    while whole.len() < rotations.len() {
        match next_reply(&rx) {
            Reply::Rendered { page, texture, complete: true, .. } => {
                whole.insert(page, texture);
            }
            Reply::RenderFailed { error, .. } | Reply::OpenFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    let mut pixels = take_pixels(&ctx);

    for page in 0..rotations.len() {
        let ([w, h], image) = pixels.remove(&whole[&page].id()).unwrap();
        let full = [w as u32, h as u32];

        // Two squares side by side from the grid, in the row holding the page's
        // ink, so they cross text.
        let dark: Vec<(usize, usize)> =
            (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| image[y * w + x].r() < 128).collect();
        assert!(!dark.is_empty(), "page {page} rendered blank");
        let cy = dark.iter().map(|p| p.1).sum::<usize>() / dark.len();
        let cx = dark.iter().map(|p| p.0).sum::<usize>() / dark.len();
        let row = cy as u32 / TILE;
        let column = (cx as u32 / TILE).min(full[0].div_ceil(TILE).saturating_sub(2));
        let top = row * TILE;
        let left = column * TILE;
        let region = [left, top, (2 * TILE).min(full[0] - left), TILE.min(full[1] - top)];

        tx.send(Request::RenderRegion { generation: 1, page, full, region }).unwrap();
        let tiles = loop {
            if let Reply::RenderedRegion { page: p, full: f, region: r, tiles, .. } = next_reply(&rx) {
                if p == page {
                    assert_eq!((f, r), (full, region));
                    break tiles;
                }
            }
        };
        assert_eq!(tiles.len(), if region[2] > TILE { 2 } else { 1 }, "page {page}: one texture per square");
        let mut drawn = take_pixels(&ctx);

        // Shifted even one pixel, glyph edges would disagree about as often as
        // there is ink; matching, they barely disagree at all.
        let (mut ink, mut differing) = (0, 0);
        for tile in &tiles {
            let [x0, y0, tw, th] = tile_rect(full, tile.column, tile.row).map(|v| v as usize);
            let (size, part) = drawn.remove(&tile.texture.id()).expect("the square's pixels");
            assert_eq!(size, [tw, th], "page {page}: square {},{}", tile.column, tile.row);
            for y in 0..th {
                for x in 0..tw {
                    let a = image[(y0 + y) * w + x0 + x];
                    let b = part[y * tw + x];
                    ink += usize::from(a.r() < 128);
                    differing += usize::from((a.r() as i32 - b.r() as i32).abs() > 32);
                }
            }
        }
        assert!(ink > 200, "page {page}: the squares cross only {ink} dark pixels");
        assert!(
            differing * 20 <= ink,
            "page {page} (/Rotate {}): {differing} pixels differ from the whole-page render, against {ink} of ink",
            rotations[page]
        );
    }

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
