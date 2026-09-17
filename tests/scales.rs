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
