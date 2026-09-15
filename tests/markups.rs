//! Markups through the worker: a drawn one is written into the file, comes
//! back from the save with its shape, is found by a fresh read, and goes again.

mod common;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use common::{build_pdf, next_reply, scratch_dir, start_worker};
use kinetic_pdf::markup;
use kinetic_pdf::model::{Changes, Markup, MarkupKind, Reply, Request};

/// Opens `path` as `generation` and collects markups until the worker says
/// every page has been read.
fn open_and_read_markups(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, path: PathBuf) -> Vec<Markup> {
    tx.send(Request::Open { generation, path }).unwrap();
    let mut found = Vec::new();
    loop {
        match next_reply(rx) {
            Reply::Highlights { generation: g, markups, done, .. } if g == generation => {
                found.extend(markups);
                if done {
                    return found;
                }
            }
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }
}

/// Saves `changes`, and returns the markups now on the pages it changed and
/// the pages it redrew.
fn save(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, changes: Changes) -> (Vec<Markup>, Vec<usize>) {
    tx.send(Request::Save { generation, changes }).unwrap();
    loop {
        match next_reply(rx) {
            Reply::Saved { markups, redrawn, .. } => return (markups, redrawn),
            Reply::SaveFailed { error, .. } => panic!("save failed: {error}"),
            _ => {}
        }
    }
}

#[test]
fn a_markup_is_saved_read_back_and_removed() {
    let dir = scratch_dir("markups-test");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(2, &[])).unwrap();
    let (tx, rx, _wanted) = start_worker();
    assert!(open_and_read_markups(&tx, &rx, 1, path.clone()).is_empty());

    let points = vec![[100.0, 100.0], [200.0, 150.0]];
    let drawn = Markup {
        key: None,
        page: 1,
        kind: MarkupKind::Arrow,
        bounds: markup::bounds(MarkupKind::Arrow, &points, 2.0),
        points: points.clone(),
        color: [0.15, 0.39, 0.92],
        width: 2.0,
        comment: "look here".to_owned(),
        author: String::new(),
    };
    let author = "tester".to_owned();
    let (saved, redrawn) = save(&tx, &rx, 1, Changes { markups: vec![drawn.clone()], author: author.clone(), ..Changes::default() });
    assert_eq!(redrawn, [1]);
    let [just_saved] = &saved[..] else { panic!("one markup: {saved:?}") };
    assert!(just_saved.key.is_some());
    assert_eq!(
        (just_saved.kind, &just_saved.points, just_saved.comment.as_str(), just_saved.author.as_str()),
        (MarkupKind::Arrow, &points, "look here", "tester"),
        "just saved, it keeps its shape"
    );

    let fresh = open_and_read_markups(&tx, &rx, 2, path.clone());
    let [read] = &fresh[..] else { panic!("one markup in the file: {fresh:?}") };
    assert_eq!((read.page, read.kind, read.comment.as_str(), read.author.as_str()), (1, MarkupKind::Line, "look here", "tester"), "an arrow is a line");
    let (b, want) = (read.bounds, drawn.bounds);
    assert!([b.left - want.left, b.bottom - want.bottom, b.right - want.right, b.top - want.top].iter().all(|d| d.abs() < 0.01), "{b:?} against {want:?}");

    let (saved, redrawn) = save(&tx, &rx, 2, Changes { deletes: vec![read.key.unwrap()], author, ..Changes::default() });
    assert!(saved.is_empty(), "{saved:?}");
    assert_eq!(redrawn, [1]);
    assert!(open_and_read_markups(&tx, &rx, 3, path).is_empty());

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
