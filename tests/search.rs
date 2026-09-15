//! Search through the real worker: a first search extracts each page's text,
//! a repeat search reads it back from the worker's cache. Both must find
//! exactly the same matches.

mod common;

use std::sync::mpsc::{Receiver, Sender};

use common::{build_pdf, next_reply, scratch_dir, start_worker};
use kinetic_pdf::model::{Reply, Request, SearchHit};

fn search(tx: &Sender<Request>, rx: &Receiver<Reply>, id: u64, query: &str) -> Vec<SearchHit> {
    tx.send(Request::Search { generation: 1, id, query: query.to_owned() }).unwrap();
    let mut hits = Vec::new();
    loop {
        match next_reply(rx) {
            Reply::Search { id: got, hits: batch, done, .. } if got == id => {
                hits.extend(batch);
                if done {
                    return hits;
                }
            }
            _ => {}
        }
    }
}

fn summary(hits: &[SearchHit]) -> Vec<(usize, String, Vec<[i32; 4]>)> {
    hits.iter()
        .map(|h| {
            let quads = h.quads.iter().map(|q| [q.left, q.bottom, q.right, q.top].map(|v| v.round() as i32)).collect();
            (h.page, format!("{}[{}]{}", h.before, h.matched, h.after), quads)
        })
        .collect()
}

#[test]
fn a_repeat_search_finds_exactly_what_the_first_did() {
    let dir = scratch_dir("search-test");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, build_pdf(3, &[])).unwrap();

    let (tx, rx, _wanted) = start_worker();
    tx.send(Request::Open { generation: 1, path }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Opened { .. } => break,
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }

    // Every page has 8 lines containing "some words".
    let first = search(&tx, &rx, 1, "some words");
    assert_eq!(first.len(), 24);
    let again = search(&tx, &rx, 2, "some words");
    assert_eq!(summary(&first), summary(&again));

    // A different query against the cached text.
    let line_two = search(&tx, &rx, 3, "line 2 with");
    assert_eq!(line_two.len(), 3);
    assert!(line_two.iter().all(|h| h.matched.eq_ignore_ascii_case("line 2 with")));

    drop(tx);
    let _ = std::fs::remove_dir_all(dir);
}
