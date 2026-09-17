//! End-to-end check of the worker thread's handling of highlights: that they
//! arrive a page at a time (a page's own before its pixels), and that after a
//! save -- which re-reads only the pages it changed -- what the app holds
//! matches exactly what a fresh read of the file finds.

mod common;

use common::{build_pdf, line_y, next_reply, open_and_read_all, scratch_dir, sorted, start_worker, Spec};
use kinetic_pdf::model::{AnnotKey, Changes, Highlight, NewHighlight, PdfBox, Reply, Request};

/// Everything about a highlight that must survive a save, in a comparable form.
fn summary(h: &Highlight) -> (usize, Option<AnnotKey>, String, String, Vec<[i32; 4]>) {
    let quads = h.quads.iter().map(|q| [q.left, q.bottom, q.right, q.top].map(|v| v.round() as i32)).collect();
    (h.page, h.key, h.comment.clone(), h.author.clone(), quads)
}

#[test]
fn highlights_arrive_a_page_at_a_time_and_survive_a_save() {
    let dir = scratch_dir("worker-test");
    let path = dir.join("doc.pdf");
    let specs = [Spec::plain(0, "first"), Spec::plain(2, "third"), Spec::plain(4, "fifth"), Spec::plain(4, "fifth again")];
    std::fs::write(&path, build_pdf(6, &specs)).unwrap();

    let (tx, rx, wanted) = start_worker();

    // Open, and ask straight away for page 3, as scrolling there would.
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages.push(2);
    }
    tx.send(Request::Render { generation: 1, page: 2, scale: 0.5 }).unwrap();

    let mut before_save = Vec::new();
    let mut page_three_pixels = false;
    let mut all_read = false;
    let mut pages = 0;
    while !(page_three_pixels && all_read) {
        match next_reply(&rx) {
            Reply::Opened { page_sizes, .. } => pages = page_sizes.len(),
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            Reply::Highlights { highlights, done, .. } => {
                before_save.extend(highlights);
                all_read |= done;
            }
            Reply::Rendered { page: 2, .. } => {
                assert!(
                    before_save.iter().any(|h| h.page == 2 && h.comment == "third"),
                    "page 3's highlight must arrive before its pixels"
                );
                page_three_pixels = true;
            }
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            _ => {}
        }
    }
    assert_eq!(pages, 6);
    let before_save = sorted(before_save);
    let comments: Vec<&str> = before_save.iter().map(|h| h.comment.as_str()).collect();
    assert_eq!(comments, ["first", "third", "fifth", "fifth again"], "each highlight exactly once, in file order");

    // Delete on page 1, add on page 2, edit on page 5. Page 3 is untouched.
    let key = |comment: &str| before_save.iter().find(|h| h.comment == comment).and_then(|h| h.key).unwrap();
    let y = line_y(0);
    let changes = Changes {
        adds: vec![NewHighlight {
            page: 1,
            quads: vec![PdfBox { left: 70.0, bottom: y - 5.0, right: 300.0, top: y + 15.0 }],
            color: [0.56, 0.93, 0.45],
            comment: "added".to_owned(),
        }],
        markups: Vec::new(),
        deletes: vec![key("first")],
        edits: vec![(key("fifth"), "fifth, edited".to_owned())],
        author: "tester".to_owned(),
        ..Changes::default()
    };
    tx.send(Request::Save { generation: 1, changes }).unwrap();

    let (changed_pages, reread) = loop {
        match next_reply(&rx) {
            Reply::Saved { pages, highlights, .. } => break (pages, highlights),
            Reply::SaveFailed { error, .. } => panic!("save failed: {error}"),
            _ => {}
        }
    };
    assert_eq!(changed_pages, [0, 1, 4]);

    // What the app holds after the save: untouched pages as they were, changed
    // pages as re-read.
    let mut app_view: Vec<Highlight> = before_save.into_iter().filter(|h| !changed_pages.contains(&h.page)).collect();
    app_view.extend(reread);
    let app_view = sorted(app_view);

    // What's really in the file now.
    let fresh = open_and_read_all(&tx, &rx, 2, path);

    let app_view: Vec<_> = app_view.iter().map(summary).collect();
    let fresh: Vec<_> = fresh.iter().map(summary).collect();
    assert_eq!(app_view, fresh, "after a save the app's highlights must match the file, keys included");
    let comments: Vec<&str> = fresh.iter().map(|s| s.2.as_str()).collect();
    assert_eq!(comments, ["added", "third", "fifth, edited", "fifth again"]);

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
