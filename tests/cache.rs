//! The page cache through the real worker and helpers. A slow page, once drawn,
//! comes back from the cache when the file is opened again -- identical, and
//! much sooner. Pages nobody has looked at are drawn into the cache in the
//! background. After saving a note the cached pages still count. A quick page
//! only gets a marker.

mod common;

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{build_pdf, dense_pdf_pages, line_y, next_reply, scratch_dir, start_worker_with, take_pixels};
use eframe::egui::TextureHandle;
use kinetic_pdf::cache::{self, Cache, Key};
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

/// Asks for a page and waits until it's complete: how long that took, whether
/// it counted as slow, and its texture.
fn draw(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, page: usize, scale: f32) -> (Duration, bool, TextureHandle) {
    let started = Instant::now();
    tx.send(Request::Render { generation, page, scale }).unwrap();
    loop {
        match next_reply(rx) {
            Reply::Rendered { generation: g, page: p, complete: true, slow, texture, .. } if g == generation && p == page => {
                return (started.elapsed(), slow, texture)
            }
            Reply::RenderSkipped { generation: g, page: p } if g == generation && p == page => panic!("page {page} was skipped"),
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            _ => {}
        }
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn slow_pages_are_kept_drawn_ahead_and_survive_a_save() {
    let dir = scratch_dir("cache");
    let path = dir.join("dense.pdf");
    let pdf = dense_pdf_pages(3, 300_000);
    std::fs::write(&path, &pdf).unwrap();
    let file = cache::fingerprint(&pdf);
    let cache = Arc::new(Cache::open(dir.join("pages"), cache::DEFAULT_LIMIT).unwrap());
    let scale = 0.5;

    let helpers = Helpers::exe(env!("CARGO_BIN_EXE_kinetic-pdf"), 2);
    let (tx, rx, wanted, ctx) = start_worker_with(helpers, Some(Arc::clone(&cache)));
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages = vec![0];
        w.render_scales = Arc::new(vec![scale; 3]);
    }
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    wait_until_open(&rx, 1);

    // The page in view is drawn, found slow and kept. The other two are drawn
    // into the cache in the background without anyone asking for them.
    let (first, slow, texture) = draw(&tx, &rx, 1, 0, scale);
    assert!(slow, "a page of 300,000 paths should count as slow; it drew in {first:?}");
    let drawn = take_pixels(&ctx).remove(&texture.id()).expect("the page's pixels").1;
    wait_for("all three pages in the cache", || (0..3).all(|page| cache.has_image(Key::new(file, page, scale))));

    // Opened again: pages come back from the cache, identical and much sooner,
    // including one only ever drawn in the background.
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 2;
        w.pages = vec![0, 2];
    }
    tx.send(Request::Open { generation: 2, path: path.clone() }).unwrap();
    wait_until_open(&rx, 2);
    let (again, slow, texture) = draw(&tx, &rx, 2, 0, scale);
    assert!(slow, "a page from the cache still counts as slow");
    assert!(again * 4 < first, "from the cache in {again:?}; drawing it took {first:?}");
    assert_eq!(take_pixels(&ctx).remove(&texture.id()).expect("the page's pixels").1, drawn, "the cached page is the page as drawn");
    let (ahead, _, _) = draw(&tx, &rx, 2, 2, scale);
    assert!(ahead * 4 < first, "the page drawn ahead came in {ahead:?}; drawing takes {first:?}");

    // Saving a note changes the file but nothing drawn, so the cache still
    // counts for the saved file.
    wanted.lock().unwrap().pages = vec![1];
    let y = line_y(0);
    let changes = Changes {
        adds: vec![NewHighlight {
            page: 1,
            quads: vec![PdfBox { left: 70.0, bottom: y - 5.0, right: 300.0, top: y + 15.0 }],
            color: [0.56, 0.93, 0.45],
            comment: "a note".to_owned(),
        }],
        markups: Vec::new(),
        deletes: Vec::new(),
        edits: Vec::new(),
        author: "tester".to_owned(),
        ..Changes::default()
    };
    tx.send(Request::Save { generation: 2, changes, arrangement: None }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { .. } => break,
            Reply::SaveFailed { error, .. } => panic!("save failed: {error}"),
            _ => {}
        }
    }
    let saved = cache::fingerprint(&std::fs::read(&path).unwrap());
    assert_ne!(saved, file, "saving changed the file");
    assert!(cache.has_image(Key::new(saved, 1, scale)), "the cached pages moved to the saved file");
    let (after_save, _, _) = draw(&tx, &rx, 2, 1, scale);
    assert!(after_save * 4 < first, "after saving, the page came in {after_save:?}; drawing takes {first:?}");

    // A quick page is only noted as quick.
    let quick = dir.join("quick.pdf");
    let quick_pdf = build_pdf(1, &[]);
    std::fs::write(&quick, &quick_pdf).unwrap();
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 3;
        w.pages = vec![0];
        w.render_scales = Arc::new(vec![scale]);
    }
    tx.send(Request::Open { generation: 3, path: quick }).unwrap();
    wait_until_open(&rx, 3);
    let (_, slow, _) = draw(&tx, &rx, 3, 0, scale);
    assert!(!slow);
    let key = Key::new(cache::fingerprint(&quick_pdf), 0, scale);
    wait_for("the quick page's marker", || cache.is_fast(key));
    assert!(!cache.has_image(key));

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
