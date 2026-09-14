//! Drawing ahead of a zoom, through the real worker and helpers. What's asked
//! for comes back as ordinary replies -- the whole page, and a region cut into
//! squares -- alongside what the view wants, and a request for a page that has
//! left the view is dropped with an empty reply.

mod common;

use std::sync::mpsc::Receiver;

use common::{build_pdf, next_reply, scratch_dir, start_worker_with_helpers};
use pdf_annotate::model::{Reply, Request};
use pdf_annotate::pool::Helpers;

fn wait_until_open(rx: &Receiver<Reply>, generation: u64) {
    loop {
        match next_reply(rx) {
            Reply::Opened { generation: g, .. } if g == generation => return,
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }
}

#[test]
fn drawing_ahead_of_a_zoom_comes_back_with_what_the_view_wants() {
    let dir = scratch_dir("predict");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(2, &[])).unwrap();

    let (tx, rx, wanted, _ctx) = start_worker_with_helpers(Helpers::exe(env!("CARGO_BIN_EXE_pdf-annotate"), 2));
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages = vec![0];
    }
    tx.send(Request::Open { generation: 1, path }).unwrap();
    wait_until_open(&rx, 1);

    // At a scale of 1.5 a letter page is 918 x 1188 pixels.
    let full = [918, 1188];
    tx.send(Request::Render { generation: 1, page: 0, scale: 0.5 }).unwrap();
    tx.send(Request::PredictPage { generation: 1, page: 0, scale: 1.5 }).unwrap();
    tx.send(Request::PredictRegion { generation: 1, page: 0, full, region: [512, 512, 406, 512] }).unwrap();
    // Page 2 isn't in view, so this is dropped.
    tx.send(Request::PredictRegion { generation: 1, page: 1, full, region: [0, 0, 512, 512] }).unwrap();

    let (mut view, mut ahead_page, mut ahead_squares, mut dropped) = (false, false, false, false);
    while !(view && ahead_page && ahead_squares && dropped) {
        match next_reply(&rx) {
            Reply::Rendered { page: 0, scale, complete: true, .. } if scale == 0.5 => view = true,
            Reply::Rendered { page: 0, scale, complete: true, .. } if scale == 1.5 => ahead_page = true,
            Reply::RenderedRegion { page: 0, region, tiles, .. } => {
                assert_eq!(region, [512, 512, 406, 512]);
                assert_eq!(tiles.len(), 1, "one square, cut short at the page's right edge");
                ahead_squares = true;
            }
            Reply::RenderedRegion { page: 1, tiles, .. } => {
                assert!(tiles.is_empty(), "a page out of view isn't drawn ahead");
                dropped = true;
            }
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            _ => {}
        }
    }

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
