//! Extraction uses the save pipeline on a copy, then leaves the open PDF alone.
mod common;

use common::{build_pdf, next_reply, open_and_read_all, start_worker, Spec};
use kinetic_pdf::arrange::Sheet;
use kinetic_pdf::model::{Changes, NewHighlight, PdfBox, Reply, Request};
use pdf_content::lopdf::Document;

#[test]
fn extraction_preserves_selected_pages_edits_and_rotations_without_replacing_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.pdf");
    let bytes = build_pdf(4, &[
        Spec::plain(0, "first page"), Spec::plain(1, "excluded page"),
        Spec::with_appearance(2, "third page", [1.0, 0.9, 0.0]),
    ]);
    let mut fixture = Document::load_mem(&bytes).unwrap();
    let excluded = fixture.get_pages()[&2];
    let outline = fixture.new_object_id();
    use pdf_content::lopdf::{Object, dictionary};
    let item = fixture.add_object(dictionary! {
        "Title" => Object::string_literal("Excluded page"), "Parent" => outline,
        "Dest" => vec![Object::Reference(excluded), Object::Name(b"Fit".to_vec())]
    });
    fixture.objects.insert(outline, Object::Dictionary(dictionary! {"Type" => "Outlines", "First" => item, "Last" => item, "Count" => 1}));
    fixture.catalog_mut().unwrap().set("Outlines", outline);
    let mut bytes = Vec::new();
    fixture.save_to(&mut bytes).unwrap();
    std::fs::write(&source, &bytes).unwrap();
    let (tx, rx, wanted) = start_worker();
    open_and_read_all(&tx, &rx, 1, source.clone());
    let export = |path: &std::path::Path, sheets: Vec<Sheet>, changes: Changes, new_pages| {
        let count = sheets.len();
        tx.send(Request::ExtractPages { generation: 1, path: path.to_owned(), changes, sheets, new_pages }).unwrap();
        loop {
            match next_reply(&rx) {
                Reply::ExtractedPages { generation, path: to, count: actual, result } => {
                    assert_eq!(generation, 1);
                    assert_eq!(to, path);
                    assert_eq!(actual, count);
                    break result;
                }
                Reply::Saved { .. } | Reply::SaveTarget { .. } => panic!("extraction must not commit a save"),
                _ => {}
            }
        }
    };
    assert!(export(&source, vec![Sheet::of_page(0)], Changes::default(), vec![]).is_err());
    let empty = dir.path().join("empty.pdf");
    assert!(export(&empty, vec![], Changes::default(), vec![]).is_err());
    assert!(!empty.exists());
    let invalid = dir.path().join("existing.pdf");
    std::fs::write(&invalid, b"keep the existing destination").unwrap();
    assert!(export(&invalid, vec![Sheet::of_page(99)], Changes::default(), vec![]).is_err());
    assert_eq!(std::fs::read(&invalid).unwrap(), b"keep the existing destination");

    let highlight = |page, comment: &str| NewHighlight {
        page, comment: comment.into(), color: [1.0, 0.9, 0.0],
        quads: vec![PdfBox { left: 72.0, bottom: 675.0, right: 300.0, top: 695.0 }],
    };
    let changes = Changes { adds: vec![highlight(2, "unsaved note"), highlight(4, "on the blank")], ..Changes::default() };
    let target = dir.path().join("extracted.pdf");
    export(&target, vec![Sheet::of_page(2).turned(1), Sheet::of_page(0), Sheet::of_page(2), Sheet::of_page(4)], changes, vec![[210.0, 300.0]]).unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), bytes, "the source is byte-for-byte unchanged");
    let extracted = Document::load(&target).unwrap();
    assert_eq!(extracted.objects.values().filter(|o| o.as_dict().ok().is_some_and(|d| d.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"Page"))).count(), 4);
    for stream in extracted.objects.values().filter_map(|o| o.as_stream().ok()) {
        let content = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
        assert!(!String::from_utf8_lossy(&content).contains("Page 2 line"), "excluded bookmarked page content must be removed");
    }

    // The worker still has the original open, including the unselected fourth page.
    let fourth = dir.path().join("fourth.pdf");
    export(&fourth, vec![Sheet::of_page(3)], Changes::default(), vec![]).unwrap();
    let read = Document::load(&fourth).unwrap();
    assert_eq!(read.get_pages().len(), 1);
    assert!(read.extract_text(&[1]).unwrap().contains("Page 4"));

    // Reopen in the real PDF reader and check both the geometry and annotations.
    tx.send(Request::Open { generation: 2, path: target }).unwrap();
    let mut highlights = Vec::new();
    let mut sizes = Vec::new();
    loop {
        match next_reply(&rx) {
            Reply::Opened { generation: 2, page_sizes, .. } => sizes = page_sizes,
            Reply::OpenFailed { error, .. } => panic!("the extracted PDF did not open: {error}"),
            Reply::Highlights { generation: 2, highlights: found, done, .. } => {
                highlights.extend(found);
                if done { break; }
            }
            _ => {}
        }
    }
    assert_eq!(sizes, vec![[792.0, 612.0], [612.0, 792.0], [612.0, 792.0], [210.0, 300.0]]);
    let mut notes: Vec<_> = highlights.iter().map(|h| (h.page, h.comment.as_str())).collect();
    notes.sort();
    assert_eq!(notes, vec![(0, "third page"), (0, "unsaved note"), (1, "first page"), (2, "third page"), (2, "unsaved note"), (3, "on the blank")]);
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 2;
        w.pages = vec![0];
    }
    tx.send(Request::Text { generation: 2, page: 0 }).unwrap();
    loop {
        if let Reply::Text { generation: 2, chars, .. } = next_reply(&rx) {
            let text: String = chars.iter().map(|c| c.ch).collect();
            assert!(text.contains("Page 3"), "extraction keeps the text layer");
            break;
        }
    }
}
