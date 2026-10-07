//! Erasing part of a page's drawing through the worker: the save takes it out
//! of the page, pdfium no longer finds the text that was there, and the
//! highlight over it -- a markup, not the drawing -- stays.
//!
//! One test to a file: the worker is the only thing in a test process that
//! may load pdfium, and it loads it once. See tests/common/mod.rs.

mod common;

use common::{build_pdf, next_reply, scratch_dir, start_worker, Spec};
use kinetic_pdf::model::{Changes, Erasure, Reply, Request};
use pdf_content::lopdf::{dictionary, Dictionary, Document, Stream};

fn read_text(tx: &std::sync::mpsc::Sender<Request>, rx: &std::sync::mpsc::Receiver<Reply>, generation: u64, page: usize) -> Vec<kinetic_pdf::domain::TextChar> {
    tx.send(Request::Text { generation, page }).unwrap();
    loop {
        if let Reply::Text { chars, .. } = next_reply(rx) { return chars; }
    }
}

#[test]
fn an_erased_line_is_gone_from_the_page_and_its_highlight_stays() {
    let dir = scratch_dir("erase");
    let path = dir.join("document.pdf");
    let mut fixture = Document::load_mem(&build_pdf(2, &[Spec::plain(0, "on the first line")])).unwrap();
    let second = fixture.get_pages()[&2];
    let resources = fixture.get_dictionary(second).unwrap().get(b"Resources").unwrap().clone();
    let form = fixture.add_object(Stream::new(dictionary! {
        "Type" => "XObject", "Subtype" => "Form",
        "BBox" => vec![0.into(), 0.into(), 612.into(), 792.into()], "Resources" => resources
    }, fixture.get_page_content(second)));
    let content = fixture.add_object(Stream::new(Dictionary::new(), b"/Fm Do q 1 0 0 1 0 -300 cm /Fm Do Q".to_vec()));
    let page = fixture.get_dictionary_mut(second).unwrap();
    page.set("Contents", content);
    page.set("Resources", dictionary! { "XObject" => dictionary! { "Fm" => form } });
    fixture.save(&path).unwrap();

    let (tx, rx, wanted) = start_worker();
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Opened { .. } => break,
            Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
            _ => {}
        }
    }

    // Prime the cache, just as displaying a page does before using Erase.
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages = vec![0, 1];
    }
    let before = read_text(&tx, &rx, 1, 0);
    assert!(before.iter().map(|c| c.ch).collect::<String>().contains("line 1 with"));

    // A band round the first line of text, and none of the second.
    let band = vec![[-10.0, 705.0], [700.0, 705.0], [700.0, 740.0], [-10.0, 740.0]];
    let changes = Changes { erasures: vec![Erasure { page: 0, region: band, layer: None }], ..Changes::default() };
    tx.send(Request::Save { generation: 1, changes, arrangement: None, new_pages: Vec::new() }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { redrawn, .. } => {
                assert!(redrawn.contains(&0), "the page draws differently now");
                break;
            }
            Reply::SaveFailed { error, .. } => panic!("the save failed: {error}"),
            _ => {}
        }
    }

    // The worker reads text only for pages the view wants.
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages = vec![0];
    }
    tx.send(Request::Text { generation: 1, page: 0 }).unwrap();
    let text: String = loop {
        match next_reply(&rx) {
            Reply::Text { chars, .. } => break chars.iter().map(|c| c.ch).collect(),
            _ => {}
        }
    };
    assert!(!text.contains("line 1 with"), "the first line is gone: {text}");
    assert!(text.contains("line 2 with"), "the second is still there: {text}");

    let doc = Document::load(&path).expect("the saved file reads");
    let page = doc.get_pages()[&1];
    let annots = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap();
    assert_eq!(annots.len(), 1, "the highlight over it stays");

    // A word in the middle of a single Tj, and the same word inside a
    // repeatedly placed Form XObject. Keep its neighbours at their positions.
    wanted.lock().unwrap().pages = vec![0, 1];
    let current = read_text(&tx, &rx, 1, 0);
    let nested = read_text(&tx, &rx, 1, 1);
    let word_region = |chars: &[kinetic_pdf::domain::TextChar], phrase: &str| {
        let text: String = chars.iter().map(|c| c.ch).collect();
        let at = text.find(phrase).expect("fixture phrase");
        let mut bounds = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for c in &chars[at..at + phrase.len()] {
            if let Some(b) = c.ink_bounds {
                bounds = [bounds[0].min(b.left), bounds[1].min(b.bottom), bounds[2].max(b.right), bounds[3].max(b.top)];
            }
        }
        let [l, b, r, t] = bounds;
        vec![[l-0.5,b-0.5], [r+0.5,b-0.5], [r+0.5,t+0.5], [l-0.5,t+0.5]]
    };
    let region = word_region(&current, "line 2");
    let nested_region = word_region(&nested, "line 1");
    let changes = Changes { erasures: vec![
        Erasure { page: 0, region: region.clone(), layer: None },
        Erasure { page: 1, region: nested_region, layer: None },
    ], ..Changes::default() };
    // Selection excludes it before saving, and undo can use the original cache.
    let masked = kinetic_pdf::selection::without_erasures(&current, 0, &changes.erasures);
    assert!(!kinetic_pdf::selection::copy_text(&masked, 0..masked.len()).contains("line 2"));
    tx.send(Request::Search { generation: 1, id: 1, query: "line 2".into(), erasures: changes.erasures.clone() }).unwrap();
    loop {
        if let Reply::Search { hits, done, .. } = next_reply(&rx) {
            assert!(hits.iter().all(|h| h.page != 0), "unsaved erasures also apply to search");
            if done { break; }
        }
    }
    tx.send(Request::Save { generation: 1, changes, arrangement: None, new_pages: vec![] }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { .. } => break,
            Reply::SaveFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    let check = |chars: &[kinetic_pdf::domain::TextChar]| {
        let text: String = chars.iter().map(|c| c.ch).collect();
        assert!(!text.contains("line 2"), "partial text run was erased: {text}");
        assert!(text.contains("with some words on it"), "neighbours remain: {text}");
        for old in current.iter().filter(|c| !c.ch.is_whitespace() && c.bounds.is_some_and(|b| b.left > region[1][0])) {
            assert!(chars.iter().any(|new| new.ch == old.ch && new.bounds.zip(old.bounds).is_some_and(|(a,b)|
                (a.left-b.left).abs() < 0.02 && (a.bottom-b.bottom).abs() < 0.02)), "remaining glyph moved: {old:?}");
        }
    };
    check(&read_text(&tx, &rx, 1, 0));
    let nested_after: String = read_text(&tx, &rx, 1, 1).iter().map(|c| c.ch).collect();
    assert_eq!(nested_after.matches("line 1").count(), 1, "only the erased placement changes: {nested_after}");

    // The new file itself has the corrected text, not just the UI cache.
    tx.send(Request::Open { generation: 2, path: path.clone() }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Opened { .. } => break,
            Reply::OpenFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    wanted.lock().unwrap().generation = 2;
    check(&read_text(&tx, &rx, 2, 0));
}
