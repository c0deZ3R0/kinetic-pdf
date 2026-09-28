//! A blank page put in and marked up before it's ever saved: one save puts
//! the page into the file, and what was drawn on it lands on it there, in
//! the place the page was put.
//!
//! One test to a file: the worker is the only thing in a test process that
//! may load pdfium, and it loads it once. See tests/common/mod.rs.

mod common;

use common::{build_pdf, next_reply, scratch_dir, start_worker, Spec};
use kinetic_pdf::arrange::Arrangement;
use kinetic_pdf::model::{MeasureMarkup, Reply, Request};
use kinetic_pdf::session::{Command, Session};
use markup_model::{Geometry, MarkupKind, Pt};
use pdf_content::lopdf::Document;

#[test]
fn what_is_drawn_on_a_new_page_is_saved_onto_it_where_it_was_put() {
    let dir = scratch_dir("new-pages");
    let path = dir.join("document.pdf");
    std::fs::write(&path, build_pdf(2, &[Spec::plain(0, "the first page")])).expect("the test PDF is written");

    let (tx, rx, _wanted) = start_worker();
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Opened { .. } => break,
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }

    // A blank page between the two, landscape, which is page 2: the file's
    // own are 0 and 1.
    let mut order = Arrangement::new(2);
    assert!(order.insert_blank(1, [400.0, 300.0]));
    let new_page = order.page_of(1).expect("a sheet there");
    assert_eq!(new_page, 2);

    // A length drawn across it, as on any page.
    let mut session = Session::default();
    session.load_measures(Vec::new());
    let line = Geometry::Line { a: Pt::new(50.0, 50.0), b: Pt::new(350.0, 250.0) };
    let drawn = MeasureMarkup::new(new_page as u32, MarkupKind::Length, line.clone());
    let id = drawn.id;
    session.apply(Command::AddMeasure(Box::new(drawn)));

    let changes = session.begin_save("tester".into()).expect("something to save");
    tx.send(Request::Save { generation: 1, changes, arrangement: Some(order.sheets().to_vec()), new_pages: order.new_pages().to_vec() }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { .. } => break,
            Reply::SaveFailed { error, .. } => panic!("the save failed: {error}"),
            _ => {}
        }
    }

    // The file has three pages, the new one second, the size it was put in
    // at, with the length on it.
    let doc = Document::load(&path).expect("the saved file reads");
    let pages: Vec<_> = doc.get_pages().values().copied().collect();
    assert_eq!(pages.len(), 3);
    let media: Vec<f32> = doc.get_dictionary(pages[1]).unwrap().get(b"MediaBox").unwrap().as_array().unwrap().iter().map(|n| n.as_float().unwrap()).collect();
    assert_eq!(media, [0.0, 0.0, 400.0, 300.0]);
    let read = pdf_io::read(&doc);
    let length = read.markups.iter().find(|m| m.id == id).expect("the length was saved");
    assert_eq!(length.page, 1, "on the page put in, now the second");
    assert_eq!(length.geometry, line);
}
