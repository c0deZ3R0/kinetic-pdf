mod common;

use common::{build_pdf, next_reply, start_worker};
use kinetic_pdf::{arrange::{self, Sheet}, domain::Changes, pins::{self, Pin}, protocol::{Reply, Request}, session::{Command, Session}};
use pdf_content::lopdf::Document;

fn pin(page: usize) -> Pin {
    let mut pin = Pin::new(page, [140.0, 520.0], "Level 2 – entrée".into(), [0.1, 0.4, 0.8]);
    pin.jump_to_position = false;
    pin
}

#[test]
fn pins_follow_reordered_rotated_copied_and_deleted_pages() {
    let original = pin(1);
    let bytes = pins::append(build_pdf(3, &[]), &[original.clone()]).unwrap();
    let mut doc = Document::load_mem(&bytes).unwrap();
    assert_eq!(pins::read(&doc).unwrap(), vec![original.clone()]);
    arrange::rearrange(&mut doc, &[Sheet::of_page(1).turned(1), Sheet::of_page(0), Sheet::of_page(1)]).unwrap();
    let found = pins::read(&doc).unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!((found[0].page, found[1].page), (0, 2));
    assert_eq!(found[0].position, original.position);
    assert_ne!(found[0].id, found[1].id);
    assert!(!found[0].jump_to_position);
    arrange::rearrange(&mut doc, &[Sheet::of_page(1)]).unwrap();
    assert!(pins::read(&doc).unwrap().is_empty());
}

#[test]
fn pin_edits_undo_across_saves_and_preserve_edits_made_during_a_save() {
    let mut session = Session::default();
    let mut pin = pin(0);
    session.apply(Command::SetPins(vec![pin.clone()]));
    assert!(session.is_dirty());
    assert!(session.undo());
    assert!(!session.is_dirty());
    assert!(session.redo());
    let written = session.begin_save(String::new()).unwrap();
    assert_eq!(written.pins, Some(vec![pin.clone()]));
    pin.name = "Renamed while saving".into();
    pin.jump_to_position = true;
    session.apply(Command::SetPins(vec![pin.clone()]));
    assert!(session.saved(&[], vec![], vec![]));
    assert!(session.is_dirty());
    assert_eq!(session.pins(), &[pin]);
    assert!(session.undo());
    assert!(!session.is_dirty());
    session.apply(Command::SetPins(vec![]));
    assert_eq!(session.begin_save(String::new()).unwrap().pins, Some(vec![]));
    session.save_failed();
    assert!(session.is_dirty());
}

#[test]
fn removing_a_layer_refiles_its_pins_and_undo_restores_them() {
    let mut session = Session::default();
    let (add, layer) = kinetic_pdf::layering::add(&session, "Pins");
    session.apply(add);
    let mut pin = pin(0);
    pin.layer = layer;
    session.apply(Command::SetPins(vec![pin.clone()]));
    session.apply(kinetic_pdf::layering::remove(&session, layer).unwrap());
    assert_eq!(session.pins()[0].layer, markup_model::LayerId::DEFAULT);
    assert!(session.undo());
    assert_eq!(session.pins(), &[pin]);
    assert!(session.layers().get(layer).is_some());
}

#[test]
fn moved_pins_save_their_new_position_and_undo_across_a_save() {
    let mut session = Session::default();
    let original = pin(0);
    session.load_pins(vec![original.clone()]);
    for position in [[150.0, 510.0], [170.0, 490.0], [230.0, 350.0]] {
        session.apply_merged(Command::MovePin { id: original.id.clone(), position });
    }
    session.end_merge();
    let changes = session.begin_save(String::new()).unwrap();
    let stored = changes.pins.unwrap();
    let bytes = pins::append(build_pdf(1, &[]), &stored).unwrap();
    let read = pins::read(&Document::load_mem(&bytes).unwrap()).unwrap();
    assert_eq!(read[0].position, [230.0, 350.0]);
    assert_eq!((&read[0].name, read[0].jump_to_position), (&original.name, original.jump_to_position));
    assert!(session.saved(&[], vec![], vec![]));
    assert!(!session.is_dirty());
    assert!(session.undo());
    assert_eq!(session.pins(), &[original]);
    assert!(session.is_dirty());
    assert!(!session.can_undo(), "one drag is one step");
}

#[test]
fn pins_survive_worker_save_as_reopen_and_an_unrelated_save() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("original.pdf");
    let target = dir.path().join("pinned.pdf");
    let original = build_pdf(2, &[]);
    std::fs::write(&source, &original).unwrap();
    let (tx, rx, _) = start_worker();
    tx.send(Request::Open { generation: 1, path: source.clone() }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Opened { .. } => break,
            Reply::OpenFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    let pins = vec![pin(1)];
    tx.send(Request::SaveAs { overwrite: true, generation: 1, path: target.clone(), changes: Changes { pins: Some(pins.clone()), ..Default::default() }, arrangement: None, new_pages: vec![] }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { .. } => break,
            Reply::SaveFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    assert_eq!(std::fs::read(source).unwrap(), original);
    assert_eq!(pins::read(&Document::load(&target).unwrap()).unwrap(), pins);
    tx.send(Request::Save { generation: 1, changes: Changes::default(), arrangement: None, new_pages: vec![] }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { .. } => break,
            Reply::SaveFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    tx.send(Request::Open { generation: 2, path: target }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Opened { .. } => break,
            Reply::OpenFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    tx.send(Request::ReadMeasurements { generation: 2 }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Measured { measurements, .. } => { assert_eq!(measurements.pins, pins); break; }
            Reply::MeasureFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
}
