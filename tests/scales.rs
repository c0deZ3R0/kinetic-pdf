//! A page's scale through the worker: set in the session, written into the
//! file by a save, and read back from the file as the same scale.

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

#[test]
fn a_scale_is_written_into_the_file_and_read_back() {
    let dir = scratch_dir("scales-test");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(3, &[])).unwrap();
    let (tx, rx, _wanted) = start_worker();
    open(&tx, &rx, 1, &path);

    let mut session = Session::default();
    session.load_scales(measurements(&tx, &rx, 1));
    assert_eq!(ratio(session.scales(), 0), None, "a fresh file has no scales");

    session.apply(Command::SetScales(at_ratio(100.0)));
    assert!(session.is_dirty(), "a scale is unsaved work");
    save(&tx, &rx, 1, &mut session);
    assert!(!session.is_dirty());

    let from_file = measurements(&tx, &rx, 1);
    assert_ratio(&from_file, 0, 100.0);
    assert_eq!(from_file.viewports(0).len(), 1);
    assert!(from_file.viewports(0)[0].whole_page);
    assert_eq!(from_file.viewports(0)[0].id, session.scales().viewports(0)[0].id, "the same viewport, not a second one");

    // Recalibrating it, and giving another page the same scale, writes both
    // pages and keeps one scale between them.
    let mut scales = session.scales().clone();
    let id = scales.page_default(0).unwrap().scale;
    let mut scale = scales.scale(id).unwrap().clone();
    scale.metres_per_point_x *= 2.0;
    scale.metres_per_point_y *= 2.0;
    scales.set_scale(scale);
    scales.set_page_scale(2, page_box(), id);
    session.apply(Command::SetScales(scales));
    save(&tx, &rx, 1, &mut session);

    let from_file = measurements(&tx, &rx, 1);
    assert_ratio(&from_file, 0, 200.0);
    assert_ratio(&from_file, 2, 200.0);
    assert_eq!(from_file.scales().count(), 1, "one scale shared by both pages");
    assert!(ratio(&from_file, 1).is_none(), "the page that was left alone has none");

    // Undoing the whole lot and saving takes the scales back out of the file.
    while session.undo() {}
    assert!(session.is_dirty());
    save(&tx, &rx, 1, &mut session);
    let from_file = measurements(&tx, &rx, 1);
    assert_eq!(from_file.scales().count(), 0);
    assert!(from_file.viewports(0).is_empty() && from_file.viewports(2).is_empty());

    // And the highlights still in the file are untouched by all of that.
    open(&tx, &rx, 2, &path);
    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
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
