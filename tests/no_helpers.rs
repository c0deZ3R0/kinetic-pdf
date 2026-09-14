//! If the render helpers can't be started, the worker draws pages itself.

mod common;

use common::{build_pdf, next_reply, scratch_dir, start_worker_with_helpers};
use pdf_annotate::model::{Reply, Request};
use pdf_annotate::pool::Helpers;

#[test]
fn pages_are_drawn_when_helpers_cannot_start() {
    let dir = scratch_dir("no-helpers");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(2, &[])).unwrap();

    let (tx, rx, wanted, _ctx) = start_worker_with_helpers(Helpers::exe(dir.join("no-such-helper.exe"), 2));
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages = vec![1];
    }
    tx.send(Request::Open { generation: 1, path }).unwrap();
    tx.send(Request::Render { generation: 1, page: 1, scale: 0.5 }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Rendered { page: 1, complete: true, .. } => break,
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            Reply::RenderSkipped { .. } => panic!("the page was skipped"),
            _ => {}
        }
    }

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
