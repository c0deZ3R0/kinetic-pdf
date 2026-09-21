//! Every kind of measurement through the worker: a count, an angle, a radius
//! and a diameter all come back measuring what they did.
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
    tx.send(Request::Save { generation, changes, arrangement: None }).unwrap();
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
fn a_count_an_angle_a_radius_and_a_diameter_all_come_back_measuring_the_same() {
    use markup_model::{Geometry, Markup, MarkupKind};

    let dir = scratch_dir("kinds-test");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(1, &[])).unwrap();
    let (tx, rx, _wanted) = start_worker();
    open(&tx, &rx, 1, &path);

    let mut session = Session::default();
    session.load_scales(measurements(&tx, &rx, 1));
    session.apply(Command::SetScales(at_ratio(100.0)));

    // 283.46 pt at 1:100 is 10 m.
    let ten_m = 283.46;
    let count = Markup::new(0, MarkupKind::Count, Geometry::Points { pts: vec![Pt::new(50.0, 50.0), Pt::new(90.0, 60.0), Pt::new(120.0, 40.0)] });
    let angle = Markup::new(0, MarkupKind::Angle, Geometry::Polyline { pts: vec![Pt::new(300.0, 100.0), Pt::new(200.0, 100.0), Pt::new(200.0, 200.0)] });
    let radius = Markup::new(0, MarkupKind::Radius, Geometry::Line { a: Pt::new(400.0, 400.0), b: Pt::new(400.0 + ten_m, 400.0) });
    let across = Markup::new(
        0,
        MarkupKind::Diameter,
        Geometry::Ellipse { rect: markup_model::Rect::from_corners(Pt::new(100.0, 500.0), Pt::new(100.0 + ten_m, 500.0 + ten_m)) },
    );
    let ids = [count.id, angle.id, radius.id, across.id];
    for markup in [count, angle, radius, across] {
        session.apply(Command::AddMeasure(Box::new(markup)));
    }
    save(&tx, &rx, 1, &mut session);

    let (scales, markups) = measurements_read(&tx, &rx, 1);
    assert_eq!(markups.len(), 4, "{markups:?}");
    let quantity = |id: markup_model::MarkupId| {
        let read = markups.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("{id:?} is missing from {markups:?}"));
        let scale = scales.resolve(0, read.geometry.first_point(), read.scale_ref).map(|(s, _)| s);
        (read.kind, markup_model::quantities(read, scale).unwrap())
    };
    let [count, angle, radius, across] = ids.map(quantity);
    assert_eq!((count.0, count.1.count), (MarkupKind::Count, Some(3)));
    assert_eq!(angle.0, MarkupKind::Angle);
    assert!((angle.1.angle_deg.unwrap() - 90.0).abs() < 1e-6, "{:?}", angle.1);
    assert!((radius.1.radius_m.unwrap() - 10.0).abs() < 1e-3, "{:?}", radius.1);
    assert!((across.1.diameter_m.unwrap() - 10.0).abs() < 1e-3, "{:?}", across.1);

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
