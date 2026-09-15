//! Shared by the worker tests: a hand-written test PDF and helpers for driving
//! the worker thread.
//!
//! The worker must be the only thing in a test process that loads pdfium, so
//! test documents are written out by hand rather than generated with pdfium.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use kinetic_pdf::model::{Highlight, Reply, Request};
use kinetic_pdf::cache::Cache;
use kinetic_pdf::pool::Helpers;
use kinetic_pdf::worker::{self, Wanted};

pub const YELLOW: [f32; 3] = [1.0, 0.93, 0.25];

/// A highlight to put in a test PDF.
pub struct Spec {
    pub page: usize,
    pub comment: &'static str,
    pub color: [f32; 3],
    /// Give it an appearance stream, as Acrobat, Edge and Bluebeam do.
    pub appearance: bool,
}

impl Spec {
    pub fn plain(page: usize, comment: &'static str) -> Self {
        Spec { page, comment, color: YELLOW, appearance: false }
    }

    pub fn with_appearance(page: usize, comment: &'static str, color: [f32; 3]) -> Self {
        Spec { page, comment, color, appearance: true }
    }
}

/// Line `i` of every page sits at this baseline; highlight `k` on a page covers line `k`.
pub fn line_y(i: usize) -> f32 {
    720.0 - 20.0 * i as f32
}

/// A small, valid PDF: `pages` letter-size pages of Helvetica text, with a
/// /Highlight annotation for each spec.
pub fn build_pdf(pages: usize, highlights: &[Spec]) -> Vec<u8> {
    build_pdf_rotated(&vec![0; pages], highlights)
}

/// `build_pdf`, with page `i` given `/Rotate rotations[i]`.
pub fn build_pdf_rotated(rotations: &[i32], highlights: &[Spec]) -> Vec<u8> {
    let pages = rotations.len();
    let mut objects: Vec<(usize, String)> = Vec::new();
    let mut next = 4;
    let mut kids = Vec::new();

    for p in 0..pages {
        let page_id = next;
        let contents_id = next + 1;
        next += 2;
        kids.push(format!("{page_id} 0 R"));

        let mut annot_refs = Vec::new();
        for (k, spec) in highlights.iter().filter(|s| s.page == p).enumerate() {
            let y = line_y(k);
            let (bottom, top) = (y - 5.0, y + 15.0);
            let [r, g, b] = spec.color;

            let mut appearance = String::new();
            if spec.appearance {
                let ap_id = next;
                next += 1;
                let content = format!("/G0 gs {r} {g} {b} rg 70 {bottom} 230 20 re f\n");
                objects.push((
                    ap_id,
                    format!(
                        "<< /Type /XObject /Subtype /Form /BBox [70 {bottom} 300 {top}] \
                         /Resources << /ExtGState << /G0 << /Type /ExtGState /BM /Multiply >> >> >> \
                         /Length {} >>\nstream\n{content}endstream",
                        content.len()
                    ),
                ));
                appearance = format!(" /AP << /N {ap_id} 0 R >>");
            }

            objects.push((
                next,
                format!(
                    "<< /Type /Annot /Subtype /Highlight /Rect [70 {bottom} 300 {top}] \
                     /QuadPoints [70 {top} 300 {top} 70 {bottom} 300 {bottom}] /C [{r} {g} {b}] \
                     /Contents ({}) /T (tester) /F 4{appearance} >>",
                    spec.comment
                ),
            ));
            annot_refs.push(format!("{next} 0 R"));
            next += 1;
        }

        let content: String = (0..8)
            .map(|i| format!("BT /F1 12 Tf 72 {} Td (Page {} line {} with some words on it) Tj ET\n", line_y(i), p + 1, i + 1))
            .collect();
        objects.push((contents_id, format!("<< /Length {} >>\nstream\n{content}endstream", content.len())));

        let annots = if annot_refs.is_empty() { String::new() } else { format!(" /Annots [{}]", annot_refs.join(" ")) };
        let rotate = if rotations[p] == 0 { String::new() } else { format!(" /Rotate {}", rotations[p]) };
        objects.push((
            page_id,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792]{rotate} \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {contents_id} 0 R{annots} >>"
            ),
        ));
    }

    objects.push((1, "<< /Type /Catalog /Pages 2 0 R >>".to_owned()));
    objects.push((2, format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.join(" "))));
    objects.push((3, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned()));
    objects.sort_by_key(|(id, _)| *id);

    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (id, body) in &objects {
        offsets.push(out.len());
        out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes(),
    );
    out
}

/// A scratch folder for one test process.
/// One letter-size page holding `lines` separate stroked paths: slow to draw
/// in the way a dense drawing is, since pdfium's cost follows the objects.
pub fn dense_pdf(lines: usize) -> Vec<u8> {
    dense_pdf_pages(1, lines)
}

/// `dense_pdf`, `pages` pages long, each page's lines placed differently.
pub fn dense_pdf_pages(pages: usize, lines: usize) -> Vec<u8> {
    let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_owned(), String::new()];
    let mut kids = Vec::new();
    for p in 0..pages {
        let mut content = String::with_capacity(lines * 32);
        for i in 0..lines {
            let x = ((i + p * 31) * 7919 % 600) as f32;
            let y = ((i + p * 17) * 104_729 % 780) as f32;
            content.push_str(&format!("{x} {y} m {} {} l S\n", x + 5.0, y + 3.0));
        }
        let page_id = objects.len() + 1;
        kids.push(format!("{page_id} 0 R"));
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R >>", page_id + 1));
        objects.push(format!("<< /Length {} >>\nstream\n{content}endstream", content.len()));
    }
    objects[1] = format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.join(" "));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kinetic-pdf-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn start_worker() -> (Sender<Request>, Receiver<Reply>, Arc<Mutex<Wanted>>) {
    let (tx, rx, wanted, _) = start_worker_with_context();
    (tx, rx, wanted)
}

/// `start_worker`, also returning the egui context the worker makes page
/// textures on, so a test can read their pixels back with `take_pixels`.
pub fn start_worker_with_context() -> (Sender<Request>, Receiver<Reply>, Arc<Mutex<Wanted>>, egui::Context) {
    start_worker_with(Helpers::none(), None)
}

/// `start_worker_with_context`, drawing pages with render helpers.
pub fn start_worker_with_helpers(helpers: Helpers) -> (Sender<Request>, Receiver<Reply>, Arc<Mutex<Wanted>>, egui::Context) {
    start_worker_with(helpers, None)
}

/// `start_worker_with_context`, with render helpers and a page cache.
pub fn start_worker_with(
    helpers: Helpers,
    cache: Option<Arc<Cache>>,
) -> (Sender<Request>, Receiver<Reply>, Arc<Mutex<Wanted>>, egui::Context) {
    let ctx = egui::Context::default();
    let wanted = Arc::new(Mutex::new(Wanted::default()));
    let (tx, rx) = worker::spawn(ctx.clone(), wanted.clone(), helpers, cache);
    (tx, rx, wanted, ctx)
}

/// The pixels of every texture made on `ctx` since the last call, by texture.
/// With no window, textures are never uploaded, so their images wait here.
pub fn take_pixels(ctx: &egui::Context) -> HashMap<egui::TextureId, ([usize; 2], Vec<egui::Color32>)> {
    let mut delta = ctx.tex_manager().write().take_delta();
    let pixels = delta
        .set
        .iter()
        .filter_map(|(id, changes)| {
            let egui::ImageData::Color(image) = &changes.last()?.image;
            Some((*id, (image.size, image.pixels.clone())))
        })
        .collect();
    // egui checks, in debug builds, that texture changes were dealt with
    // rather than dropped; these are deliberately discarded.
    delta.clear();
    pixels
}

pub fn next_reply(rx: &Receiver<Reply>) -> Reply {
    match rx.recv_timeout(Duration::from_secs(30)).expect("the worker went quiet") {
        Reply::Fatal(message) => panic!("pdfium could not be loaded: {message}"),
        reply => reply,
    }
}

pub fn sorted(mut highlights: Vec<Highlight>) -> Vec<Highlight> {
    highlights.sort_by_key(|h| (h.page, h.key.map_or(usize::MAX, |k| k.index)));
    highlights
}

/// Opens `path` as `generation` and collects highlights until the worker says
/// every page has been read.
pub fn open_and_read_all(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, path: PathBuf) -> Vec<Highlight> {
    tx.send(Request::Open { generation, path }).unwrap();
    let mut found = Vec::new();
    loop {
        match next_reply(rx) {
            Reply::Highlights { generation: g, highlights, done, .. } if g == generation => {
                found.extend(highlights);
                if done {
                    return sorted(found);
                }
            }
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }
}
