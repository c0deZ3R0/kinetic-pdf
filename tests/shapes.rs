//! Shapes from the drawing tools through the worker: saved as the annotations
//! other programs write for them, read back as the same shapes on their
//! layers, and not listed a second time as markups drawn the older way.
//!
//! One test to a file: the worker is the only thing in a test process that
//! may load pdfium, and it loads it once. See tests/common/mod.rs.

#![allow(dead_code)]

mod common;

use std::sync::mpsc::{Receiver, Sender};

use common::{build_pdf, next_reply, scratch_dir, start_worker};
use kinetic_pdf::layering;
use kinetic_pdf::model::{Measurements, Reply, Request};
use kinetic_pdf::session::{Command, Session};
use markup_model::{Geometry, LayerId, Markup, MarkupKind, Pt};

fn read(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64) -> Measurements {
    tx.send(Request::ReadMeasurements { generation }).unwrap();
    loop {
        match next_reply(rx) {
            Reply::Measured { measurements, .. } => return *measurements,
            Reply::MeasureFailed { error, .. } => panic!("could not read the file: {error}"),
            _ => {}
        }
    }
}

fn open(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, path: &std::path::Path) {
    tx.send(Request::Open { generation, path: path.to_path_buf() }).unwrap();
    loop {
        match next_reply(rx) {
            Reply::Opened { generation: g, .. } if g == generation => return,
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }
}

/// Saves, and gives the annotations pdfium read back from the pages saved: the
/// ones drawn the older way, of which there should be none of ours.
fn save(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, session: &mut Session) -> usize {
    let changes = session.begin_save("tester".into()).expect("something to save");
    tx.send(Request::Save { generation, changes, arrangement: None, new_pages: Vec::new() }).unwrap();
    loop {
        match next_reply(rx) {
            Reply::Saved { pages, highlights, markups, .. } => {
                let older = markups.len();
                assert!(session.saved(&pages, highlights, markups));
                return older;
            }
            Reply::SaveFailed { error, .. } => panic!("save failed: {error}"),
            _ => {}
        }
    }
}

#[test]
fn shapes_are_saved_read_back_on_their_layers_and_not_listed_twice() {
    let dir = scratch_dir("shapes-test");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(1, &[])).unwrap();
    let (tx, rx, _wanted) = start_worker();
    open(&tx, &rx, 1, &path);

    let mut session = Session::default();
    let file = read(&tx, &rx, 1);
    session.load_layers(file.layers);
    session.load_measures(file.markups);

    let (command, marks) = layering::add(&session, "Markup");
    session.apply(command);
    let shapes = [
        Markup::new(0, MarkupKind::Box, Geometry::Polygon { pts: vec![Pt::new(100.0, 100.0), Pt::new(300.0, 100.0), Pt::new(300.0, 220.0), Pt::new(100.0, 220.0)], holes: vec![] }),
        Markup::new(0, MarkupKind::Ellipse, Geometry::Polygon { pts: vec![Pt::new(400.0, 300.0), Pt::new(480.0, 340.0), Pt::new(440.0, 420.0), Pt::new(360.0, 380.0)], holes: vec![] }),
        Markup::new(0, MarkupKind::Line, Geometry::Line { a: Pt::new(50.0, 500.0), b: Pt::new(250.0, 520.0) }),
        Markup::new(0, MarkupKind::Arrow, Geometry::Line { a: Pt::new(50.0, 600.0), b: Pt::new(250.0, 640.0) }),
        Markup::new(0, MarkupKind::Pen, Geometry::Ink { strokes: vec![vec![Pt::new(10.0, 10.0), Pt::new(20.0, 30.0), Pt::new(45.0, 25.0)]] }),
    ];
    let ids: Vec<_> = shapes.iter().map(|m| m.id).collect();
    for mut shape in shapes {
        shape.layer = marks;
        session.apply(Command::AddMeasure(Box::new(shape)));
    }
    let older = save(&tx, &rx, 1, &mut session);
    assert_eq!(older, 0, "none of ours read back as a shape of the older kind");
    assert!(!session.is_dirty());

    let back = read(&tx, &rx, 1);
    assert_eq!(back.markups.len(), 5);
    for id in ids {
        let shape = back.markups.iter().find(|m| m.id == id).expect("saved");
        assert_eq!(shape.layer, marks);
        assert!(!shape.extras.changed_externally);
    }
    assert!(back.layers.get(marks).is_some_and(|l| l.name == "Markup"));
    assert_ne!(marks, LayerId::DEFAULT);

    open(&tx, &rx, 2, &path);
    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
