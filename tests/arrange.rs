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
    let path = arranged("arrange-order", &[Sheet::Page(3), Sheet::Page(0), Sheet::Page(1), Sheet::Page(1)]);
    let sizes = sizes_of(path);
    assert_eq!(sizes.len(), 4, "pdfium counted a different number of sheets");
    let landscape = |s: &[f32; 2]| s[0] > s[1];
    assert!(landscape(&sizes[0]), "the first sheet should be the page turned a quarter turn");
    assert!(!landscape(&sizes[1]), "the second sheet should be the upright first page");
    assert!(landscape(&sizes[2]) && landscape(&sizes[3]), "the same page twice should be the same shape twice");
}

#[test]
fn a_blank_sheet_comes_back_the_size_it_was_asked_for() {
    let path = arranged("arrange-blank", &[Sheet::Page(0), Sheet::Blank([595.0, 842.0])]);
    let sizes = sizes_of(path);
    assert_eq!(sizes.len(), 2);
    assert!((sizes[1][0] - 595.0).abs() < 0.5 && (sizes[1][1] - 842.0).abs() < 0.5, "the blank sheet came back {:?}", sizes[1]);
}

#[test]
fn highlights_travel_with_the_pages_they_are_on() {
    // The third page first, then the first: their highlights should come back
    // on sheets 0 and 1, saying what they always said.
    let path = arranged("arrange-highlights", &[Sheet::Page(2), Sheet::Page(0)]);
    let (tx, rx, _wanted) = start_worker();
    let highlights = open_and_read_all(&tx, &rx, 1, path);
    let found: Vec<(usize, String)> = highlights.iter().map(|h| (h.page, h.comment.clone())).collect();
    assert_eq!(found, vec![(0, "the third page".to_owned()), (1, "the first page".to_owned())]);
}
