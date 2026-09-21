//! A rearranged file, opened by pdfium rather than by the writer that made it.
//!
//! `arrange::rearrange` is checked against lopdf in its own unit tests, which
//! proves the page tree says what it should. This proves the harder thing: a
//! real PDF reader opens the result, counts the sheets the arrangement asked
//! for, reports them in that order, and finds each page's highlights still on
//! it -- so the annotations travelled with their pages rather than staying at
//! the place in the file they used to be at.

mod common;

use common::{build_pdf_rotated, open_and_read_all, scratch_dir, start_worker, Spec};
use kinetic_pdf::arrange::{rearrange, Sheet};
use kinetic_pdf::model::{Reply, Request};
use pdf_content::lopdf::Document;

/// Writes `sheets` of a four-page document -- two portrait, two landscape,
/// with a highlight on each of the first three -- and gives back the file.
fn arranged(name: &str, sheets: &[Sheet]) -> std::path::PathBuf {
    let bytes = build_pdf_rotated(
        &[0, 90, 0, 90],
        &[Spec::plain(0, "the first page"), Spec::plain(1, "the second page"), Spec::plain(2, "the third page")],
    );
    let dir = scratch_dir(name);
    let from = dir.join("before.pdf");
    std::fs::write(&from, &bytes).expect("the test PDF is written");

    let mut doc = Document::load(&from).expect("lopdf reads it");
    rearrange(&mut doc, sheets).expect("it rearranges");
    let to = dir.join("after.pdf");
    doc.save(&to).expect("it saves");
    to
}

/// The sizes of the pages as pdfium reports them, which for a page turned a
/// quarter turn are its own the other way round.
fn sizes_of(path: std::path::PathBuf) -> Vec<[f32; 2]> {
    let (tx, rx, _wanted) = start_worker();
    tx.send(Request::Open { generation: 1, path }).unwrap();
    loop {
        match common::next_reply(&rx) {
            Reply::Opened { page_sizes, .. } => return page_sizes,
            Reply::OpenFailed { error, .. } => panic!("pdfium would not open the rearranged file: {error}"),
            _ => {}
        }
    }
}

#[test]
fn pdfium_opens_a_rearranged_file_with_its_sheets_in_the_order_asked_for() {
    // Last, first, and the second one twice: an order, a page left out and a
    // page used more than once, all in one file.
    let path = arranged("arrange-order", &[Sheet::of_page(3), Sheet::of_page(0), Sheet::of_page(1), Sheet::of_page(1)]);
    let sizes = sizes_of(path);
    assert_eq!(sizes.len(), 4, "pdfium counted a different number of sheets");
    let landscape = |s: &[f32; 2]| s[0] > s[1];
    assert!(landscape(&sizes[0]), "the first sheet should be the page turned a quarter turn");
    assert!(!landscape(&sizes[1]), "the second sheet should be the upright first page");
    assert!(landscape(&sizes[2]) && landscape(&sizes[3]), "the same page twice should be the same shape twice");
}

#[test]
fn a_blank_sheet_comes_back_the_size_it_was_asked_for() {
    let path = arranged("arrange-blank", &[Sheet::of_page(0), Sheet::blank([595.0, 842.0])]);
    let sizes = sizes_of(path);
    assert_eq!(sizes.len(), 2);
    assert!((sizes[1][0] - 595.0).abs() < 0.5 && (sizes[1][1] - 842.0).abs() < 0.5, "the blank sheet came back {:?}", sizes[1]);
}

#[test]
fn pdfium_reports_a_turned_sheet_the_other_way_round() {
    // Page 0 is upright in the file and page 1 is already turned a quarter
    // turn, so one quarter-turn each puts the first on its side and the second
    // back upright -- which is the test that the turn is counted from the way
    // the file already had the page, not from square.
    let path = arranged("arrange-turned", &[Sheet::of_page(0).turned(1), Sheet::of_page(1).turned(1)]);
    let sizes = sizes_of(path);
    assert_eq!(sizes.len(), 2);
    let landscape = |s: &[f32; 2]| s[0] > s[1];
    assert!(landscape(&sizes[0]), "the upright page turned a quarter should come back on its side, got {:?}", sizes[0]);
    assert!(!landscape(&sizes[1]), "the page the file already turned should come back upright, got {:?}", sizes[1]);
}

#[test]
fn highlights_travel_with_the_pages_they_are_on() {
    // The third page first, then the first: their highlights should come back
    // on sheets 0 and 1, saying what they always said.
    let path = arranged("arrange-highlights", &[Sheet::of_page(2), Sheet::of_page(0)]);
    let (tx, rx, _wanted) = start_worker();
    let highlights = open_and_read_all(&tx, &rx, 1, path);
    let found: Vec<(usize, String)> = highlights.iter().map(|h| (h.page, h.comment.clone())).collect();
    assert_eq!(found, vec![(0, "the third page".to_owned()), (1, "the first page".to_owned())]);
}

/// Saving with an arrangement writes the new order into the file the document
/// is already open on -- the original, in place, once -- and the highlights go
/// with their pages rather than staying at the place in the file they were at.
/// This is what one Ctrl+S does after the sheets have been sorted: there is no
/// separate "apply", and no second file.
#[test]
fn a_save_that_carries_an_arrangement_rewrites_the_open_file_in_place() {
    use kinetic_pdf::model::{Changes, Request};

    let bytes = build_pdf_rotated(
        &[0, 0, 0, 0],
        &[Spec::plain(0, "the first page"), Spec::plain(1, "the second page"), Spec::plain(2, "the third page")],
    );
    let dir = scratch_dir("arrange-save-in-place");
    let path = dir.join("document.pdf");
    std::fs::write(&path, &bytes).expect("the test PDF is written");
    let before = std::fs::metadata(&path).expect("it is there").len();

    let (tx, rx, _wanted) = start_worker();
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    loop {
        match common::next_reply(&rx) {
            Reply::Opened { .. } => break,
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }

    // The third page first, then the first, and the second turned on its side.
    let sheets = vec![Sheet::of_page(2), Sheet::of_page(0), Sheet::of_page(1).turned(1)];
    tx.send(Request::Save { generation: 1, changes: Changes::default(), arrangement: Some(sheets) }).unwrap();
    loop {
        match common::next_reply(&rx) {
            Reply::Saved { .. } => break,
            Reply::SaveFailed { error, .. } => panic!("the save failed: {error}"),
            _ => {}
        }
    }

    assert!(std::fs::metadata(&path).expect("it is still there").len() > 0, "the original was left empty");
    assert_ne!(before, 0, "the test PDF was empty to begin with");
    assert!(!dir.join("document (arranged).pdf").exists(), "a save should not leave a second file behind");

    // Read back with pdfium, on the same worker: one process can only bind
    // the library once.
    let highlights = open_and_read_all(&tx, &rx, 2, path.clone());
    let found: Vec<(usize, String)> = highlights.iter().map(|h| (h.page, h.comment.clone())).collect();
    assert_eq!(
        found,
        vec![(0, "the third page".to_owned()), (1, "the first page".to_owned()), (2, "the second page".to_owned())],
        "the highlights should have travelled with their pages into the new order"
    );

}
