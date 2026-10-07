//! Saving to another file preserves the source and changes subsequent saves.
mod common;
use common::{build_pdf, next_reply, start_worker, Spec};
use kinetic_pdf::model::{Changes, NewHighlight, PdfBox, Reply, Request};

#[test]
fn save_as_switches_destination_only_after_success() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.pdf");
    let target = dir.path().join("copy.pdf");
    let original = build_pdf(1, &[Spec::plain(0, "original")]);
    std::fs::write(&source, &original).unwrap();
    let (tx, rx, _) = start_worker();
    tx.send(Request::Open { generation: 1, path: source.clone() }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Opened { .. } => break,
            Reply::OpenFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    // An invalid target must neither switch destinations nor alter the source.
    tx.send(Request::SaveAs { overwrite: true,
        generation: 1,
        path: dir.path().join("missing/copy.pdf"),
        changes: Changes::default(),
        arrangement: None,
        new_pages: vec![],
    })
    .unwrap();
    loop {
        match next_reply(&rx) {
            Reply::SaveFailed { .. } => break,
            Reply::SaveTarget { .. } => panic!("failed save changed destination"),
            _ => {}
        }
    }
    tx.send(Request::SaveAs { overwrite: true,
        generation: 1,
        path: target.clone(),
        changes: Changes {
            adds: vec![NewHighlight {
                page: 0,
                quads: vec![PdfBox { left: 10.0, bottom: 10.0, right: 100.0, top: 30.0 }],
                color: [1.0, 1.0, 0.0],
                comment: "saved in the copy".into(),
            }],
            ..Changes::default()
        },
        arrangement: None,
        new_pages: vec![],
    })
    .unwrap();
    let mut switched = false;
    loop {
        match next_reply(&rx) {
            Reply::SaveTarget { path, .. } => {
                assert_eq!(path, target);
                switched = true;
            }
            Reply::Saved { .. } => {
                assert!(switched);
                break;
            }
            Reply::SaveFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    assert_eq!(std::fs::read(&source).unwrap(), original);
    assert!(target.exists());
    let copy = pdf_content::lopdf::Document::load(&target).unwrap();
    let page = *copy.get_pages().values().next().unwrap();
    assert_eq!(copy.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap().len(), 2);
    // Remove the copy, then Save must recreate that copy rather than the source.
    std::fs::remove_file(&target).unwrap();
    tx.send(Request::Save { generation: 1, changes: Changes::default(), arrangement: None, new_pages: vec![] })
        .unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { .. } => break,
            Reply::SaveFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    assert!(target.exists());
    assert_eq!(std::fs::read(&source).unwrap(), original);
}
