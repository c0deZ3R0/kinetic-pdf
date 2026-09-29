//! Layers through the worker: made and ordered in the session, written into
//! the file by a save as its optional content, and read back the same.
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
use markup_model::{Geometry, LayerId, Markup, MarkupKind, Pt, Restack};

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

fn save(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, session: &mut Session) {
    let changes = session.begin_save("tester".into()).expect("something to save");
    tx.send(Request::Save { generation, changes, arrangement: None, new_pages: Vec::new() }).unwrap();
    loop {
        match next_reply(rx) {
            Reply::Saved { pages, highlights, markups, .. } => {
                assert!(session.saved(&pages, highlights, markups));
                return;
            }
            Reply::SaveFailed { error, .. } => panic!("save failed: {error}"),
            _ => {}
        }
    }
}

fn length_on(session: &mut Session, layer: LayerId, x: f64) -> markup_model::MarkupId {
    let mut m = Markup::new(0, MarkupKind::Length, Geometry::Line { a: Pt::new(x, 100.0), b: Pt::new(x + 50.0, 100.0) });
    m.layer = layer;
    let id = m.id;
    session.apply(Command::AddMeasure(Box::new(m)));
    id
}

#[test]
fn layers_and_stacking_are_written_into_the_file_and_read_back() {
    let dir = scratch_dir("layers-test");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(1, &[])).unwrap();
    let (tx, rx, _wanted) = start_worker();
    open(&tx, &rx, 1, &path);

    let mut session = Session::default();
    let file = read(&tx, &rx, 1);
    session.load_layers(file.layers);
    session.load_measures(file.markups);
    assert!(!session.is_dirty(), "a file with no layers has just the default one");

    let (command, walls) = layering::add(&session, "Walls");
    session.apply(command);
    let (a, b, c) = (length_on(&mut session, walls, 0.0), length_on(&mut session, walls, 100.0), length_on(&mut session, LayerId::DEFAULT, 200.0));
    session.apply(layering::edit(&session, |l| {
        l.set_visible(walls, false);
    }).unwrap());
    save(&tx, &rx, 1, &mut session);
    assert!(!session.is_dirty());

    let back = read(&tx, &rx, 1);
    assert_eq!(&back.layers, session.layers(), "names, order and what is hidden");
    assert!(!back.layers.is_visible(walls));
    assert_eq!(back.markups.len(), 3);
    let layer_of = |id| back.markups.iter().find(|m| m.id == id).unwrap().layer;
    assert_eq!((layer_of(a), layer_of(b), layer_of(c)), (walls, walls, LayerId::DEFAULT));

    // Bringing the first to the front of its layer rewrites just that one,
    // and its place in the layer comes back with it.
    session.apply(layering::restack(&session, &[a], Restack::ToFront).unwrap());
    save(&tx, &rx, 1, &mut session);
    let back = read(&tx, &rx, 1);
    let z = |id| back.markups.iter().find(|m| m.id == id).unwrap().extras.z;
    assert!(z(a) > z(b), "a is in front of b now");
    let order: Vec<_> = back.markups.iter().filter(|m| m.layer == walls).map(|m| m.id).collect();
    assert_eq!(order.len(), 2);

    // Taking the layer out puts its markups on the default layer, and the
    // file says so.
    session.apply(layering::remove(&session, walls).unwrap());
    save(&tx, &rx, 1, &mut session);
    let back = read(&tx, &rx, 1);
    assert!(back.layers.get(walls).is_none());
    assert!(back.markups.iter().all(|m| m.layer == LayerId::DEFAULT));

    open(&tx, &rx, 2, &path);
    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
