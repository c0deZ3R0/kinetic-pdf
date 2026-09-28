//! Erasing part of a page's drawing through the worker: the save takes it out
//! of the page, pdfium no longer finds the text that was there, and the
//! highlight over it -- a markup, not the drawing -- stays.
//!
//! One test to a file: the worker is the only thing in a test process that
//! may load pdfium, and it loads it once. See tests/common/mod.rs.

mod common;

use common::{build_pdf, next_reply, scratch_dir, start_worker, Spec};
use kinetic_pdf::model::{Changes, Erasure, Reply, Request};
use pdf_content::lopdf::Document;

#[test]
fn an_erased_line_is_gone_from_the_page_and_its_highlight_stays() {
    let dir = scratch_dir("erase");
    let path = dir.join("document.pdf");
    std::fs::write(&path, build_pdf(1, &[Spec::plain(0, "on the first line")])).expect("the test PDF is written");

    let (tx, rx, wanted) = start_worker();
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Opened { .. } => break,
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }

    // A band round the first line of text, and none of the second.
    let band = vec![[-10.0, 705.0], [700.0, 705.0], [700.0, 740.0], [-10.0, 740.0]];
    let changes = Changes { erasures: vec![Erasure { page: 0, region: band }], ..Changes::default() };
    tx.send(Request::Save { generation: 1, changes, arrangement: None, new_pages: Vec::new() }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { redrawn, .. } => {
                assert!(redrawn.contains(&0), "the page draws differently now");
                break;
            }
            Reply::SaveFailed { error, .. } => panic!("the save failed: {error}"),
            _ => {}
        }
    }

    // The worker reads text only for pages the view wants.
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages = vec![0];
    }
    tx.send(Request::Text { generation: 1, page: 0 }).unwrap();
    let text: String = loop {
        match next_reply(&rx) {
            Reply::Text { chars, .. } => break chars.iter().map(|c| c.ch).collect(),
            _ => {}
        }
    };
    assert!(!text.contains("line 1 with"), "the first line is gone: {text}");
    assert!(text.contains("line 2 with"), "the second is still there: {text}");

    let doc = Document::load(&path).expect("the saved file reads");
    let page = doc.get_pages()[&1];
    let annots = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap();
    assert_eq!(annots.len(), 1, "the highlight over it stays");
}
