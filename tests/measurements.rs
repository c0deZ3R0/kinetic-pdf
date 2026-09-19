//! A measurement through the worker: written into the file by a save, read
//! back as the same measurement, and removed again.
//!
//! One test to a file: the worker is the only thing in a test process that
//! may load pdfium, and it loads it once, so a second test here would fail to
//! bind. See tests/common/mod.rs.

#![allow(dead_code)]

mod common;

use std::sync::mpsc::{Receiver, Sender};

use common::{build_pdf, next_reply, scratch_dir, start_worker};
use kinetic_pdf::model::{Reply, Request, ScaleStore};
use kinetic_pdf::session::{Command, Session};
use markup_model::{Pt, Rect, Scale, ScaleId};

/// A4 in points.
fn page_box() -> Rect {
    Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(595.0, 842.0))
}

/// Scales with page 0 at 1:`ratio`.
fn at_ratio(ratio: f64) -> ScaleStore {
    let mut scales = ScaleStore::default();
    let scale = Scale::from_ratio(ScaleId::new(), ratio).unwrap();
    let id = scale.id;
    scales.set_scale(scale);
    scales.set_page_scale(0, page_box(), id);
    scales
}

fn measurements(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64) -> ScaleStore {
    tx.send(Request::ReadMeasurements { generation }).unwrap();
    loop {
        match next_reply(rx) {
            Reply::Measured { measurements, .. } => return measurements.scales,
            Reply::MeasureFailed { error, .. } => panic!("could not read the scales: {error}"),
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
    tx.send(Request::Save { generation, changes }).unwrap();
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

fn ratio(scales: &ScaleStore, page: u32) -> Option<f64> {
    scales.page_default(page).and_then(|v| scales.scale(v.scale)).and_then(Scale::ratio)
}

/// A page measures at 1:`expected`, to the precision a PDF real holds (32
/// bits, so about seven digits).
fn assert_ratio(scales: &ScaleStore, page: u32, expected: f64) {
    let got = ratio(scales, page).unwrap_or_else(|| panic!("page {page} has no scale"));
    assert!((got - expected).abs() <= 1e-5 * expected, "page {page} measures at 1:{got}, not 1:{expected}");
}


/// A length measurement on page 0, `points` long across the page.
fn length_at(points: f64) -> markup_model::Markup {
    markup_model::Markup::new(
        0,
        markup_model::MarkupKind::Length,
        markup_model::Geometry::Line { a: Pt::new(100.0, 100.0), b: Pt::new(100.0 + points, 100.0) },
    )
}

fn measurements_read(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64) -> (ScaleStore, Vec<markup_model::Markup>) {
    tx.send(Request::ReadMeasurements { generation }).unwrap();
    loop {
        match next_reply(rx) {
            Reply::Measured { measurements, .. } => return (measurements.scales, measurements.markups),
            Reply::MeasureFailed { error, .. } => panic!("could not read the measurements: {error}"),
            _ => {}
        }
    }
}

#[test]
fn a_measurement_is_written_read_back_and_removed() {
    let dir = scratch_dir("measures-test");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(1, &[])).unwrap();
    let (tx, rx, _wanted) = start_worker();
    open(&tx, &rx, 1, &path);

    let mut session = Session::default();
    session.load_scales(measurements(&tx, &rx, 1));
    session.apply(Command::SetScales(at_ratio(100.0)));
    // 283.46 pt at 1:100 is 10 m.
    let drawn = length_at(283.46);
    let id = drawn.id;
    session.apply(Command::AddMeasure(Box::new(drawn)));
    save(&tx, &rx, 1, &mut session);

    let (scales, markups) = measurements_read(&tx, &rx, 1);
    let [read] = &markups[..] else { panic!("one measurement: {markups:?}") };
    assert_eq!((read.id, read.kind), (id, markup_model::MarkupKind::Length));
    assert!(!read.extras.changed_externally, "our own save isn't an outside edit");
    let scale = scales.resolve(0, read.geometry.first_point(), read.scale_ref).map(|(s, _)| s);
    let length = markup_model::quantities(read, scale).unwrap().length_m.unwrap();
    assert!((length - 10.0).abs() < 1e-3, "it measures {length} m");

    // Moved, it is written again in place rather than twice.
    let mut moved = session.measures().get(id).unwrap().clone();
    moved.geometry = markup_model::Geometry::Line { a: Pt::new(100.0, 100.0), b: Pt::new(100.0 + 566.92, 100.0) };
    session.apply(Command::ChangeMeasure(Box::new(moved)));
    save(&tx, &rx, 1, &mut session);
    let (scales, markups) = measurements_read(&tx, &rx, 1);
    assert_eq!(markups.len(), 1, "written again, not twice");
    let scale = scales.resolve(0, markups[0].geometry.first_point(), markups[0].scale_ref).map(|(s, _)| s);
    let length = markup_model::quantities(&markups[0], scale).unwrap().length_m.unwrap();
    assert!((length - 20.0).abs() < 1e-3, "it now measures {length} m");

    // And taken out again.
    session.apply(Command::RemoveMeasure(id));
    save(&tx, &rx, 1, &mut session);
    let (_, markups) = measurements_read(&tx, &rx, 1);
    assert!(markups.is_empty(), "{markups:?}");
    assert!(!session.is_dirty());

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
