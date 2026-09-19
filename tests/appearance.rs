//! Highlights made in most other PDF programs carry
//! appearance streams. Reading the colour of one used to crash the whole app
//! inside pdfium (see vendor/pdfium-render/PATCHES.md). This opens a PDF with
//! one, draws its page, checks the colour came through, then edits the note and
//! saves to check nothing is lost.

mod common;

use common::{build_pdf, next_reply, open_and_read_all, scratch_dir, sorted, start_worker, Spec};
use kinetic_pdf::model::{AnnotEdit, Changes, Reply, Request};

const GREEN: [f32; 3] = [0.5, 1.0, 0.0];

fn assert_color(actual: [f32; 3], expected: [f32; 3]) {
    let close = actual.iter().zip(expected).all(|(a, e)| (a - e).abs() < 0.01);
    assert!(close, "colour {actual:?}, expected {expected:?}");
}

#[test]
fn highlights_with_appearance_streams_open_and_keep_their_colour() {
    let dir = scratch_dir("appearance-test");
    let path = dir.join("doc.pdf");
    let specs = [Spec::with_appearance(0, "from another app", GREEN), Spec::plain(1, "made here")];
    std::fs::write(&path, build_pdf(2, &specs)).unwrap();

    let (tx, rx, wanted) = start_worker();
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages.push(0);
    }
    tx.send(Request::Render { generation: 1, page: 0, scale: 0.5 }).unwrap();

    let mut found = Vec::new();
    let mut drawn = false;
    let mut all_read = false;
    while !(drawn && all_read) {
        match next_reply(&rx) {
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            Reply::Highlights { highlights, done, .. } => {
                found.extend(highlights);
                all_read |= done;
            }
            Reply::Rendered { page: 0, .. } => drawn = true,
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            _ => {}
        }
    }

    let found = sorted(found);
    assert_eq!(found.len(), 2);
    let other_app = &found[0];
    assert_eq!(other_app.comment, "from another app");
    assert_eq!(other_app.author, "tester");
    assert_color(other_app.color, GREEN);
    assert_color(found[1].color, common::YELLOW);

    // Edit the note on the highlight with the appearance stream and save.
    let changes = Changes {
        adds: Vec::new(),
        markups: Vec::new(),
        deletes: Vec::new(),
        edits: vec![AnnotEdit { key: other_app.key.unwrap(), comment: "edited here".to_owned(), author: "tester".to_owned() }],
        author: "tester".to_owned(),
        ..Changes::default()
    };
    tx.send(Request::Save { generation: 1, changes }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { .. } => break,
            Reply::SaveFailed { error, .. } => panic!("save failed: {error}"),
            _ => {}
        }
    }

    let fresh = open_and_read_all(&tx, &rx, 2, path);
    assert_eq!(fresh.len(), 2);
    assert_eq!(fresh[0].comment, "edited here");
    assert_color(fresh[0].color, GREEN);
    assert_eq!(fresh[1].comment, "made here");

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
