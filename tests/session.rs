//! Changes made through the session, saved by the worker into a real file:
//! after each save the session's highlights and markups carry the keys a
//! fresh read of the file gives them, with the same uids as before, so undo
//! and changes made while a save ran carry on.

mod common;

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};

use common::{build_pdf, line_y, next_reply, scratch_dir, start_worker, Spec};
use kinetic_pdf::markup;
use kinetic_pdf::model::{Changes, Highlight, Markup, MarkupKind, PdfBox, Reply, Request};
use kinetic_pdf::session::{Command, Session};

/// Opens `path` as `generation` and reads every highlight and markup in it.
fn open(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, path: &Path) -> (Vec<Highlight>, Vec<Markup>) {
    tx.send(Request::Open { generation, path: path.to_path_buf() }).unwrap();
    let (mut highlights, mut markups) = (Vec::new(), Vec::new());
    loop {
        match next_reply(rx) {
            Reply::Highlights { generation: g, highlights: h, markups: m, done, .. } if g == generation => {
                highlights.extend(h);
                markups.extend(m);
                if done {
                    return (highlights, markups);
                }
            }
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }
}

/// Sends a save and waits for what it read back.
fn send_save(tx: &Sender<Request>, generation: u64, changes: Changes) {
    tx.send(Request::Save { generation, changes }).unwrap();
}

fn wait_saved(rx: &Receiver<Reply>) -> (Vec<usize>, Vec<Highlight>, Vec<Markup>) {
    loop {
        match next_reply(rx) {
            Reply::Saved { pages, highlights, markups, .. } => return (pages, highlights, markups),
            Reply::SaveFailed { error, .. } => panic!("save failed: {error}"),
            _ => {}
        }
    }
}

/// A fresh read of the file holds exactly what the session says is in it:
/// the shown highlights and markups with keys, at those keys, plus those
/// removed but not yet deleted by a save.
fn assert_matches_file(session: &Session, tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, path: &Path) {
    let (highlights, markups) = open(tx, rx, generation, path);
    let mut in_file: Vec<(usize, usize)> = highlights.iter().map(|h| (h.page, h.key.unwrap().index)).collect();
    in_file.extend(markups.iter().map(|m| (m.page, m.key.unwrap().index)));
    in_file.sort();
    let mut expected: Vec<(usize, usize)> = session.highlights().iter().filter_map(|e| e.hl.key.map(|k| (k.page, k.index))).collect();
    expected.extend(session.markups().iter().filter_map(|e| e.markup.key.map(|k| (k.page, k.index))));
    expected.extend(session.pending_deletes().iter().map(|k| (k.page, k.index)));
    expected.sort();
    assert_eq!(expected, in_file, "annotations in the file");
    // Shown highlights have the file's comments where they haven't been
    // edited since.
    for e in session.highlights() {
        if let Some(key) = e.hl.key {
            let read = highlights.iter().find(|h| h.key == Some(key)).expect("a shown key is in the file");
            assert!(read.comment == e.hl.comment || session.is_dirty(), "{:?}: {} vs {}", key, read.comment, e.hl.comment);
        }
    }
}

fn new_highlight(line: usize, comment: &str) -> Highlight {
    let y = line_y(line);
    Highlight {
        key: None,
        page: 0,
        quads: vec![PdfBox { left: 72.0, bottom: y - 3.0, right: 250.0, top: y + 12.0 }],
        color: [1.0, 0.93, 0.25],
        comment: comment.to_owned(),
        author: String::new(),
        snippet: String::new(),
    }
}

fn new_markup(y: f32) -> Markup {
    let points = vec![[100.0, y], [200.0, y + 30.0]];
    Markup {
        key: None,
        page: 0,
        kind: MarkupKind::Rectangle,
        bounds: markup::bounds(MarkupKind::Rectangle, &points, 2.0),
        points,
        color: [0.15, 0.39, 0.92],
        width: 2.0,
        comment: String::new(),
        author: String::new(),
    }
}

#[test]
fn saving_through_the_worker_keeps_uids_and_keys_in_step_with_the_file() {
    let dir = scratch_dir("session-test");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(1, &[Spec::plain(0, "first"), Spec::plain(0, "second"), Spec::plain(0, "third")])).unwrap();
    let (tx, rx, _wanted) = start_worker();
    let mut generation = 1;
    let (highlights, markups) = open(&tx, &rx, generation, &path);
    let mut session = Session::default();
    session.load(highlights, markups);
    let uid_of = |s: &Session, comment: &str| s.highlights().iter().find(|e| e.hl.comment == comment).unwrap().uid;
    let (first, second, third) = (uid_of(&session, "first"), uid_of(&session, "second"), uid_of(&session, "third"));

    // A markup drawn, a highlight added, one removed and one's note changed.
    let drawn = session.apply(Command::AddMarkup(new_markup(300.0)))[0];
    let added = session.apply(Command::AddHighlights(vec![new_highlight(5, "added")]))[0];
    session.apply(Command::Remove(second));
    session.apply(Command::EditNote { uid: first, comment: "first, edited".into(), color: [0.0; 3] });

    let changes = session.begin_save("tester".into()).unwrap();
    send_save(&tx, generation, changes);
    // While it saves: another highlight, and the one being saved removed.
    let during = session.apply(Command::AddHighlights(vec![new_highlight(6, "during")]))[0];
    session.apply(Command::Remove(added));
    let (pages, highlights, markups) = wait_saved(&rx);
    assert!(session.saved(&pages, highlights, markups), "what came back matches what was written");

    assert!(session.highlight(first).unwrap().hl.key.is_some());
    assert!(session.highlight(third).unwrap().hl.key.is_some());
    assert!(session.markup(drawn).unwrap().markup.key.is_some());
    assert!(session.highlight(during).unwrap().is_new());
    assert!(session.is_dirty(), "the changes made during the save are still to save");
    generation += 1;
    assert_matches_file(&session, &tx, &rx, generation, &path);
    // The reopen above is only to check; the session carries on as it was.
    let changes = session.begin_save("tester".into()).unwrap();
    assert_eq!(changes.adds.len(), 1, "during");
    assert_eq!(changes.deletes.len(), 1, "added, saved and then removed");
    send_save(&tx, generation, changes);
    let (pages, highlights, markups) = wait_saved(&rx);
    assert!(session.saved(&pages, highlights, markups));
    assert!(!session.is_dirty());
    generation += 1;
    assert_matches_file(&session, &tx, &rx, generation, &path);

    // Undo everything after saving twice: what was added goes, and "second",
    // which the first save deleted, goes back in as new.
    let mut undone = 0;
    while session.undo() {
        undone += 1;
    }
    assert_eq!(undone, 6);
    assert!(session.highlight(second).is_some());
    assert!(session.highlight(added).is_none() && session.markup(drawn).is_none(), "everything added is undone");
    let changes = session.begin_save("tester".into()).unwrap();
    send_save(&tx, generation, changes);
    let (pages, highlights, markups) = wait_saved(&rx);
    assert!(session.saved(&pages, highlights, markups));
    generation += 1;
    assert_matches_file(&session, &tx, &rx, generation, &path);
    let mut comments: Vec<&str> = session.highlights().iter().map(|e| e.hl.comment.as_str()).collect();
    comments.sort_unstable();
    assert_eq!(comments, ["first", "second", "third"], "back to the file as it started");

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
