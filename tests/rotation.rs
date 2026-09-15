//! Rotated pages: pdfium draws them turned, but their text and annotations
//! stay in unrotated page space, so the app maps between the two with the
//! page's `PageGeometry`. This renders rotated pages through the real worker,
//! finds where the ink actually landed in the pixels, and checks the text's
//! mapped position agrees -- which pins the rotation direction to what pdfium
//! really does, not just to the maths.

mod common;

use std::collections::HashMap;

use common::{build_pdf_rotated, next_reply, scratch_dir, start_worker_with_context, take_pixels};
use eframe::egui::{Color32, TextureHandle};
use kinetic_pdf::model::{PageGeometry, Reply, Request, TextChar};

/// The bounding box of dark pixels, as fractions of the image: (left, top, right, bottom).
fn ink_bounds(size: [usize; 2], pixels: &[Color32]) -> (f32, f32, f32, f32) {
    let [w, h] = size;
    let (mut l, mut t, mut r, mut b) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if pixels[y * w + x].r() < 128 {
                l = l.min(x);
                t = t.min(y);
                r = r.max(x);
                b = b.max(y);
            }
        }
    }
    assert!(r > l && b > t, "no ink found on the rendered page");
    (l as f32 / w as f32, t as f32 / h as f32, (r + 1) as f32 / w as f32, (b + 1) as f32 / h as f32)
}

/// Where the page's text should appear, by mapping every character box.
fn text_bounds(geometry: &PageGeometry, chars: &[TextChar]) -> (f32, f32, f32, f32) {
    let mut out = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for b in chars.iter().filter_map(|c| c.bounds) {
        let (l, t, r, bottom) = geometry.box_to_view(&b);
        out = (out.0.min(l), out.1.min(t), out.2.max(r), out.3.max(bottom));
    }
    out
}

fn center(b: (f32, f32, f32, f32)) -> (f32, f32) {
    ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0)
}

#[test]
fn text_on_rotated_pages_maps_to_where_pdfium_draws_it() {
    let dir = scratch_dir("rotation-test");
    let path = dir.join("rotated.pdf");
    let rotations = [0, 90, 180, 270];
    std::fs::write(&path, build_pdf_rotated(&rotations, &[])).unwrap();

    let (tx, rx, wanted, ctx) = start_worker_with_context();
    tx.send(Request::Open { generation: 1, path }).unwrap();
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages.extend(0..rotations.len());
    }
    for page in 0..rotations.len() {
        tx.send(Request::Text { generation: 1, page }).unwrap();
        tx.send(Request::Render { generation: 1, page, scale: 0.5 }).unwrap();
    }

    let mut sizes = Vec::new();
    let mut geometry: HashMap<usize, PageGeometry> = HashMap::new();
    let mut text: HashMap<usize, Vec<TextChar>> = HashMap::new();
    let mut textures: HashMap<usize, TextureHandle> = HashMap::new();
    while geometry.len() < rotations.len() || text.len() < rotations.len() || textures.len() < rotations.len() {
        match next_reply(&rx) {
            Reply::Opened { page_sizes, .. } => sizes = page_sizes,
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            Reply::Highlights { geometry: g, .. } => geometry.extend(g),
            Reply::Text { page, chars, .. } => {
                text.insert(page, chars);
            }
            Reply::Rendered { page, texture, complete: true, .. } => {
                textures.insert(page, texture);
            }
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            _ => {}
        }
    }

    // The worker made the textures; with no window their images are still
    // waiting to be uploaded, which is where the pixels come from.
    let pixels = take_pixels(&ctx);

    for (page, &degrees) in rotations.iter().enumerate() {
        let g = geometry[&page];
        assert_eq!(g.rotation as i32 * 90, degrees, "page {page} rotation");

        // Quarter turns swap the displayed width and height.
        let [w, h] = sizes[page];
        let expected = if degrees % 180 == 0 { [612.0, 792.0] } else { [792.0, 612.0] };
        assert!((w - expected[0]).abs() < 1.0 && (h - expected[1]).abs() < 1.0, "page {page} size {w} x {h}");

        let (size, image) = &pixels[&textures[&page].id()];
        let ink = center(ink_bounds(*size, image));
        let mapped = center(text_bounds(&g, &text[&page]));
        assert!(
            (ink.0 - mapped.0).abs() < 0.05 && (ink.1 - mapped.1).abs() < 0.05,
            "page {page} (/Rotate {degrees}): ink centred at {ink:?}, text mapped to {mapped:?}"
        );
    }

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
