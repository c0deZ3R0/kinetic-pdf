//! Crash triage: performs each pdfium call the app makes on a PDF, one at a
//! time, printing each step before running it. If pdfium brings the process
//! down, the last line printed names the call that did it.
//!
//!     cargo run --release --example probe -- file.pdf

use std::io::Write;

use pdfium_render::prelude::*;

use pdf_annotate::{annots, selection};

fn step(label: &str) {
    eprint!("{label} ... ");
    let _ = std::io::stderr().flush();
}

fn ok() {
    eprintln!("ok");
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe file.pdf");

    step("bind pdfium");
    let pdfium = pdf_annotate::worker::bind().expect("bind");
    ok();

    step("read file");
    let bytes = std::fs::read(&path).expect("read");
    ok();

    step("load document");
    let doc = pdfium.load_pdf_from_byte_vec(bytes, None).expect("load");
    ok();

    step("page sizes");
    let sizes = annots::page_sizes(&doc);
    eprintln!("ok, {} pages", sizes.len());

    for p in 0..sizes.len() {
        eprintln!("\npage {}:", p + 1);
        let page = doc.pages().get(p as PdfPageIndex).expect("page");
        let annotations = page.annotations();
        let count = annotations.len();
        eprintln!("  {count} annotations");
        eprintln!("  geometry {:?}", annots::page_geometry(&page));

        // Each piece of `read_page_highlights`, separately.
        for i in 0..count {
            let Ok(annot) = annotations.get(i) else {
                eprintln!("  annotation {i}: could not get");
                continue;
            };
            let Some(h) = annot.as_highlight_annotation() else {
                eprintln!("  annotation {i}: {:?}, skipped", annot.annotation_type());
                continue;
            };
            eprintln!("  highlight {i}:");
            step("    count attachment points");
            let points = h.attachment_points();
            eprintln!("ok, {}", points.len());
            step("    read attachment points");
            let quads: Vec<_> = points.iter().collect();
            eprintln!("ok, {}", quads.len());
            step("    stroke colour");
            let color = h.stroke_color().map(|c| (c.red(), c.green(), c.blue()));
            eprintln!("ok, {color:?}");
            step("    contents");
            let contents = h.contents().map(|c| c.chars().count());
            eprintln!("ok, {contents:?} chars");
            step("    creator");
            let creator = h.creator();
            eprintln!("ok, {creator:?}");
        }

        step("  load page text");
        let text = page.text().expect("text");
        ok();
        step("  count characters");
        let chars = text.chars();
        let n = chars.len();
        eprintln!("ok, {n}");
        for (i, ch) in chars.iter().enumerate() {
            if i % 250 == 0 {
                eprintln!("  characters from {i}");
            }
            let _ = ch.unicode_char();
            let _ = ch.loose_bounds();
        }
        eprintln!("  all characters ok");
        drop(chars);
        drop(text);
        drop(page);

        step("  page_chars (as the app does)");
        let extracted = annots::page_chars(&doc, p).expect("page_chars");
        eprintln!("ok, {}", extracted.len());

        step("  read_page_highlights (as the app does)");
        let found = annots::read_page_highlights(&doc, p);
        eprintln!("ok, {}", found.len());
        for h in &found {
            let _ = selection::text_in_quads(&extracted, &h.quads);
            let note: String = h.comment.chars().take(40).collect();
            let rgb = h.color.map(|v| (v * 255.0).round() as u8);
            eprintln!("    colour {rgb:?} by {:?}: {note:?}", h.author);
        }

        step("  strip_highlights");
        annots::strip_highlights(&doc, p);
        ok();

        step("  render_page as the app does");
        let result = annots::render_page(&doc, p, 1.0).map(|(size, _)| size);
        eprintln!("ok, {result:?}");
    }

    eprintln!("\nfinished without crashing");
}
