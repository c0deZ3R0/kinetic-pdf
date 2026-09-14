//! Reading and writing highlights in the PDF itself.
//!
//! The PDF is the only store -- there is no sidecar file. A highlight is a real
//! /Highlight annotation with /QuadPoints and /Contents, so the notes show up in
//! Acrobat, Edge, Preview, anything. That also means highlights made elsewhere
//! show up here.
//!
//! Everything in this file runs on the worker thread, the only thread that
//! touches pdfium.

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::Utc;
use pdfium_render::prelude::*;

use crate::model::{AnnotKey, Changes, Highlight, PageGeometry, PdfBox, Rgb, TextChar};
use crate::selection;

pub const DEFAULT_COLOR: Rgb = [1.0, 0.93, 0.25];

fn err(e: PdfiumError) -> String {
    e.to_string()
}

fn to_box(r: &PdfRect) -> PdfBox {
    PdfBox {
        left: r.left().value,
        bottom: r.bottom().value,
        right: r.right().value,
        top: r.top().value,
    }
}

fn bounds(quads: &[PdfBox]) -> PdfBox {
    quads.iter().skip(1).fold(quads[0], |acc, q| PdfBox {
        left: acc.left.min(q.left),
        bottom: acc.bottom.min(q.bottom),
        right: acc.right.max(q.right),
        top: acc.top.max(q.top),
    })
}

fn to_pdf_color([r, g, b]: Rgb) -> PdfColor {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    PdfColor::new(byte(r), byte(g), byte(b), 255)
}

/// Every page's size in points, read without loading the pages themselves, so
/// even a very long document lays out immediately.
pub fn page_sizes(doc: &PdfDocument) -> Vec<[f32; 2]> {
    let pages = doc.pages();
    (0..pages.len())
        .map(|i| match pages.page_size(i) {
            Ok(r) => [
                (r.right().value - r.left().value).abs(),
                (r.top().value - r.bottom().value).abs(),
            ],
            Err(_) => [612.0, 792.0],
        })
        .collect()
}

pub fn page_chars(doc: &PdfDocument, index: usize) -> Result<Vec<TextChar>, String> {
    let page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
    chars_of(&page)
}

/// `page_chars`, for a page that is already loaded.
pub fn chars_of(page: &PdfPage) -> Result<Vec<TextChar>, String> {
    let text = page.text().map_err(err)?;
    let chars = text.chars();
    let out = chars
        .iter()
        .map(|c| TextChar {
            ch: c.unicode_char().unwrap_or(' '),
            // Loose bounds span the font's full ascent and descent, which is
            // what a highlight band should cover, not just the ink.
            bounds: c.loose_bounds().ok().map(|r| to_box(&r)).filter(|b| !b.is_empty()),
        })
        .collect();
    Ok(out)
}

/// Every highlight in the document, in page order and, within a page, in
/// /Annots order. This loads every page, which is slow on a long document, so
/// the app reads a page at a time instead (`read_page_highlights`).
pub fn read_highlights(doc: &PdfDocument) -> Vec<Highlight> {
    (0..doc.pages().len() as usize).flat_map(|p| read_page_highlights(doc, p)).collect()
}

/// How a page's user space maps onto the page as pdfium draws it: its
/// rotation, and the part of it that's visible (the crop box trimmed to the
/// media box).
pub fn page_geometry(page: &PdfPage) -> PageGeometry {
    let rotation = match page.rotation() {
        Ok(PdfPageRenderRotation::Degrees90) => 1,
        Ok(PdfPageRenderRotation::Degrees180) => 2,
        Ok(PdfPageRenderRotation::Degrees270) => 3,
        _ => 0,
    };
    let bounds = page
        .boundaries()
        .bounding()
        .ok()
        .map(|b| to_box(&b.bounds))
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| {
            // pdfium reports width and height as displayed, so turn them back.
            let (w, h) = (page.width().value, page.height().value);
            let (w, h) = if rotation % 2 == 1 { (h, w) } else { (w, h) };
            PdfBox { left: 0.0, bottom: 0.0, right: w, top: h }
        });
    PageGeometry { rotation, bounds }
}

/// The highlights on one page, in /Annots order. See `read_page`.
pub fn read_page_highlights(doc: &PdfDocument, page_index: usize) -> Vec<Highlight> {
    read_page(doc, page_index).0
}

/// A page's highlights, in /Annots order, and its geometry -- both from one
/// load of the page, which is the costly part on a large drawing.
///
/// This removes each highlight's appearance stream from `doc` (see below), so
/// only call it on a copy that won't be saved -- in the app, the worker's
/// display copy, which deletes a page's highlights before drawing it anyway.
pub fn read_page(doc: &PdfDocument, page_index: usize) -> (Vec<Highlight>, Option<PageGeometry>) {
    match doc.pages().get(page_index as PdfPageIndex) {
        Ok(page) => {
            let (highlights, geometry) = read_loaded_page(&page, page_index);
            (highlights, Some(geometry))
        }
        Err(_) => (Vec::new(), None),
    }
}

/// `read_page`, for a page that is already loaded.
pub fn read_loaded_page(page: &PdfPage, page_index: usize) -> (Vec<Highlight>, PageGeometry) {
    let geometry = page_geometry(page);
    let mut on_page = Vec::new();

    let annots = page.annotations();
    for index in 0..annots.len() {
        let Ok(mut annot) = annots.get(index) else { continue };
        let Some(h) = annot.as_highlight_annotation_mut() else { continue };

        // pdfium reports an annotation's /C colour only when it has no
        // appearance stream -- and highlights made in Acrobat, Edge or
        // Bluebeam always have one. Removing it lets pdfium read /C directly.
        // (Unpatched pdfium-render crashed here instead; see
        // vendor/pdfium-render/PATCHES.md.)
        let _ = h.remove_appearance(PdfAppearanceMode::Normal);

        let quads: Vec<PdfBox> = h
            .attachment_points()
            .iter()
            .map(|q| PdfBox {
                left: q.left().value,
                bottom: q.bottom().value,
                right: q.right().value,
                top: q.top().value,
            })
            .filter(|b| !b.is_empty())
            .collect();
        if quads.is_empty() {
            continue;
        }

        let color = h
            .stroke_color()
            .map(|c| [c.red() as f32 / 255.0, c.green() as f32 / 255.0, c.blue() as f32 / 255.0])
            .unwrap_or(DEFAULT_COLOR);

        on_page.push(Highlight {
            key: Some(AnnotKey { page: page_index, index }),
            page: page_index,
            quads,
            color,
            comment: h.contents().unwrap_or_default(),
            author: h.creator().unwrap_or_default(),
            snippet: String::new(),
        });
    }

    // Only pages that carry highlights pay for text extraction.
    if !on_page.is_empty() {
        if let Ok(chars) = chars_of(&page) {
            for h in &mut on_page {
                h.snippet = selection::text_in_quads(&chars, &h.quads);
            }
        }
    }
    (on_page, geometry)
}

/// Apply pending changes to `bytes`, returning the new PDF.
pub fn save(pdfium: &Pdfium, bytes: &[u8], changes: &Changes) -> Result<Vec<u8>, String> {
    let doc = pdfium.load_pdf_from_byte_slice(bytes, None).map_err(err)?;
    apply(&doc, changes)?;
    let out = doc.save_to_bytes().map_err(err)?;
    Ok(out)
}

fn apply(doc: &PdfDocument, changes: &Changes) -> Result<(), String> {
    let now = Utc::now();
    let pages: BTreeSet<usize> = changes
        .adds
        .iter()
        .map(|a| a.page)
        .chain(changes.deletes.iter().map(|k| k.page))
        .chain(changes.edits.iter().map(|(k, _)| k.page))
        .collect();

    for p in pages {
        let mut page = doc.pages().get(p as PdfPageIndex).map_err(err)?;
        let annots = page.annotations_mut();

        // Edits first, while every key still points where it did when read.
        for (key, comment) in changes.edits.iter().filter(|(k, _)| k.page == p) {
            let mut annot = annots.get(key.index).map_err(err)?;
            if let Some(h) = annot.as_highlight_annotation_mut() {
                h.set_contents(comment).map_err(err)?;
                h.set_modification_date(now).map_err(err)?;
            }
        }

        // Highest index first, so removing one doesn't shift the rest.
        let mut deletes: Vec<usize> =
            changes.deletes.iter().filter(|k| k.page == p).map(|k| k.index).collect();
        deletes.sort_unstable_by(|a, b| b.cmp(a));
        deletes.dedup();
        for index in deletes {
            let annot = annots.get(index).map_err(err)?;
            if matches!(annot.annotation_type(), PdfPageAnnotationType::Highlight) {
                annots.delete_annotation(annot).map_err(err)?;
            }
        }

        for add in changes.adds.iter().filter(|a| a.page == p && !a.quads.is_empty()) {
            let mut h = annots.create_highlight_annotation().map_err(err)?;
            h.set_stroke_color(to_pdf_color(add.color)).map_err(err)?;
            for q in &add.quads {
                // Per-quad point order is upper-left, upper-right, lower-left,
                // lower-right -- the order Acrobat writes and expects.
                let quad = PdfQuadPoints::new_from_values(
                    q.left, q.top, q.right, q.top, q.left, q.bottom, q.right, q.bottom,
                );
                h.attachment_points_mut().create_attachment_point_at_end(quad).map_err(err)?;
            }
            let b = bounds(&add.quads);
            h.set_bounds(PdfRect::new_from_values(b.bottom, b.left, b.top, b.right)).map_err(err)?;
            h.set_contents(&add.comment).map_err(err)?;
            h.set_creator(&changes.author).map_err(err)?;
            h.set_creation_date(now).map_err(err)?;
            h.set_modification_date(now).map_err(err)?;
        }
    }
    Ok(())
}

/// Remove a page's highlights from the *display* copy of the document before
/// rendering it. The UI draws highlights itself, so they can change without a
/// re-render; if pdfium drew them too, a deleted highlight would linger in the
/// pixels. Every other kind of annotation still renders.
pub fn strip_highlights(doc: &PdfDocument, index: usize) {
    if let Ok(mut page) = doc.pages().get(index as PdfPageIndex) {
        strip_loaded_page(&mut page);
    }
}

/// `strip_highlights`, for a page that is already loaded.
pub fn strip_loaded_page(page: &mut PdfPage) {
    let annots = page.annotations_mut();
    for i in (0..annots.len()).rev() {
        if let Ok(annot) = annots.get(i) {
            if matches!(annot.annotation_type(), PdfPageAnnotationType::Highlight) {
                let _ = annots.delete_annotation(annot);
            }
        }
    }
}

/// `scale` is output pixels per PDF point.
pub fn render_page(doc: &PdfDocument, index: usize, scale: f32) -> Result<([usize; 2], Vec<u8>), String> {
    let page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
    render_loaded_page(&page, scale)
}

/// `render_page`, for a page that is already loaded.
pub fn render_loaded_page(page: &PdfPage, scale: f32) -> Result<([usize; 2], Vec<u8>), String> {
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(scale)
        .render_annotations(true)
        .render_form_data(true);
    let bitmap = page.render_with_config(&config).map_err(err)?;
    let size = [bitmap.width() as usize, bitmap.height() as usize];
    let rgba = bitmap.as_rgba_bytes();
    Ok((size, rgba))
}

/// How long pdfium draws before pausing to ask whether to carry on.
const RENDER_STEP: Duration = Duration::from_millis(40);

/// A rendered image: its size and RGBA pixels. `None` if it was abandoned.
pub type Rendered = Option<([usize; 2], Vec<u8>)>;

/// `render_loaded_page`, drawn in steps, with the page's annotations or
/// without them. About every 40 ms `on_pause` gets the bitmap as drawn so far,
/// and returns false to abandon it. A page with hundreds of thousands of
/// drawing objects takes over a second to draw at any size; this is what lets
/// one scrolled past be dropped part-way, and one in view show as it draws.
pub fn render_page_in_steps(page: &PdfPage, scale: f32, annotations: bool, on_pause: impl FnMut(&PdfBitmap) -> bool) -> Result<Rendered, String> {
    let size = |points: f32| (points * scale).round().max(1.0) as Pixels;
    let (w, h) = (size(page.width().value), size(page.height().value));
    let config = PdfRenderConfig::new().set_fixed_size(w, h);
    render_in_steps(page, [w, h], config, annotations, on_pause)
}

/// Only `region` -- x, y, width, height in pixels -- of the page as it would be
/// drawn `full` pixels in size, in steps like `render_page_in_steps`. Cheap
/// pages cost only the pixels rendered, so the part of a huge drawing in view
/// at deep zoom takes milliseconds where the whole sheet at that size would
/// take seconds and gigabytes.
pub fn render_region_in_steps(
    page: &PdfPage,
    full: [u32; 2],
    region: [u32; 4],
    annotations: bool,
    on_pause: impl FnMut(&PdfBitmap) -> bool,
) -> Result<Rendered, String> {
    let [x, y, w, h] = region.map(|v| v as Pixels);
    let config = PdfRenderConfig::new().set_fixed_size(full[0] as Pixels, full[1] as Pixels).set_origin(-x, -y);
    render_in_steps(page, [w, h], config, annotations, on_pause)
}

fn render_in_steps(
    page: &PdfPage,
    [w, h]: [Pixels; 2],
    config: PdfRenderConfig,
    annotations: bool,
    on_pause: impl FnMut(&PdfBitmap) -> bool,
) -> Result<Rendered, String> {
    if w <= 0 || h <= 0 {
        return Err("nothing to render".to_owned());
    }
    let mut bitmap = PdfBitmap::empty(w, h, PdfBitmapFormat::default()).map_err(err)?;
    // Form fields are annotations too, so they go with the rest.
    let config = config.render_annotations(annotations).render_form_data(annotations);
    let complete = page.render_into_bitmap_in_steps(&mut bitmap, &config, RENDER_STEP, on_pause).map_err(err)?;
    Ok(complete.then(|| ([w as usize, h as usize], bitmap.as_rgba_bytes())))
}
