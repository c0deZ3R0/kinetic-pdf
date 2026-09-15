//! Pages drawn by render helpers -- the app's own exe, started with its helper
//! flag -- through the real worker. Several pages are drawn at once and come
//! back without their highlights, which the app draws itself; a slow render
//! stops early once its page is no longer wanted; and the file can be saved
//! while the helpers have it open, with drawing carrying on afterwards.

mod common;

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use common::{
    build_pdf, dense_pdf, line_y, next_reply, open_and_read_all, scratch_dir, start_worker_with_helpers, take_pixels, Spec, YELLOW,
};
use eframe::egui::TextureHandle;
use kinetic_pdf::model::{Changes, NewHighlight, PdfBox, Reply, Request};
use kinetic_pdf::pool::Helpers;

fn wait_until_open(rx: &Receiver<Reply>, generation: u64) {
    loop {
        match next_reply(rx) {
            Reply::Opened { generation: g, .. } if g == generation => return,
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }
}

/// Asks for a page and waits until it is completely drawn.
fn draw(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, page: usize, scale: f32) {
    tx.send(Request::Render { generation, page, scale }).unwrap();
    loop {
        match next_reply(rx) {
            Reply::Rendered { generation: g, page: p, complete: true, .. } if g == generation && p == page => return,
            Reply::RenderSkipped { generation: g, page: p } if g == generation && p == page => panic!("page {page} was skipped"),
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            _ => {}
        }
    }
}

#[test]
fn helpers_draw_pages_stop_when_told_and_carry_on_after_a_save() {
    let dir = scratch_dir("helpers");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(3, &[Spec::with_appearance(0, "yellow note", YELLOW), Spec::plain(1, "another")])).unwrap();

    let helpers = Helpers::exe(env!("CARGO_BIN_EXE_kinetic-pdf"), 2);
    let (tx, rx, wanted, ctx) = start_worker_with_helpers(helpers);

    // Three pages at once. The view is marked as moving first, which holds
    // back the worker's background read of highlights. A worker drawing a page
    // itself reads its highlights just before, so no highlights arriving shows
    // the helpers did the drawing.
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages = vec![0, 1, 2];
        w.moving = true;
    }
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    wait_until_open(&rx, 1);
    for page in 0..3 {
        tx.send(Request::Render { generation: 1, page, scale: 0.5 }).unwrap();
    }
    let mut drawn: HashMap<usize, TextureHandle> = HashMap::new();
    while drawn.len() < 3 {
        match next_reply(&rx) {
            Reply::Rendered { page, texture, complete: true, .. } => {
                drawn.insert(page, texture);
            }
            Reply::Highlights { geometry, .. } if !geometry.is_empty() => {
                panic!("the worker read highlights to draw a page itself, so the helpers didn't draw it")
            }
            Reply::RenderSkipped { page, .. } => panic!("page {page} was skipped"),
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            _ => {}
        }
    }
    let mut pixels = take_pixels(&ctx);
    for page in 0..3 {
        let (size, image) = pixels.remove(&drawn[&page].id()).expect("the page's pixels");
        assert_eq!(size, [306, 396], "page {page}");
        assert!(image.iter().any(|c| c.r() < 100), "page {page} should have text drawn on it");
        let yellow = image.iter().filter(|c| c.r() > 200 && c.g() > 170 && c.b() < 140).count();
        assert_eq!(yellow, 0, "page {page} should come back without its highlights, which the app draws");
    }
    wanted.lock().unwrap().moving = false;

    // A slow page stops part-way once it's no longer wanted. The helper loads
    // the page afresh for each render, and loading can't be stopped, so the
    // saving is in the drawing that follows.
    let dense = dir.join("dense.pdf");
    std::fs::write(&dense, dense_pdf(300_000)).unwrap();
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 2;
        w.pages = vec![0];
    }
    tx.send(Request::Open { generation: 2, path: dense }).unwrap();
    wait_until_open(&rx, 2);
    let started = Instant::now();
    draw(&tx, &rx, 2, 0, 2.0);
    let full = started.elapsed();
    assert!(full >= Duration::from_millis(300), "the dense page drew in {full:?}, too quickly to test stopping it");

    tx.send(Request::Render { generation: 2, page: 0, scale: 2.0 }).unwrap();
    let started = Instant::now();
    std::thread::sleep(full / 3);
    wanted.lock().unwrap().pages.clear();
    loop {
        match next_reply(&rx) {
            Reply::RenderSkipped { generation: 2, .. } => break,
            Reply::Rendered { generation: 2, complete: true, .. } => panic!("the render finished instead of stopping"),
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            _ => {}
        }
    }
    let stopped = started.elapsed();
    assert!(stopped < full * 3 / 4, "stopped after {stopped:?}; a full render takes {full:?}");

    // Saving replaces the file while the helpers have it open.
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 3;
        w.pages = vec![2];
    }
    tx.send(Request::Open { generation: 3, path: path.clone() }).unwrap();
    wait_until_open(&rx, 3);
    draw(&tx, &rx, 3, 2, 0.5);
    let y = line_y(0);
    let changes = Changes {
        adds: vec![NewHighlight {
            page: 2,
            quads: vec![PdfBox { left: 70.0, bottom: y - 5.0, right: 300.0, top: y + 15.0 }],
            color: [0.56, 0.93, 0.45],
            comment: "added while helpers read the file".to_owned(),
        }],
        markups: Vec::new(),
        deletes: Vec::new(),
        edits: Vec::new(),
        author: "tester".to_owned(),
    };
    tx.send(Request::Save { generation: 3, changes }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { .. } => break,
            Reply::SaveFailed { error, .. } => panic!("could not save while the helpers had the file open: {error}"),
            _ => {}
        }
    }
    // The helpers open the saved file and go on drawing.
    draw(&tx, &rx, 3, 2, 0.75);
    let highlights = open_and_read_all(&tx, &rx, 4, path);
    assert!(highlights.iter().any(|h| h.comment == "added while helpers read the file"), "the save reached the file");

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
