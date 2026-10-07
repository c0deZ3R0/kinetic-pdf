//! Readers use the open revision, and conflicts preserve both versions.
mod common;
use common::{build_pdf, next_reply, start_worker, Spec};
use kinetic_pdf::model::{Changes, NewHighlight, PdfBox, Reply, Request};

#[test]
fn revisions_preserve_external_changes_and_save_as_recovers_without_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.pdf");
    let target = dir.path().join("copy.pdf");
    let original = build_pdf(1, &[Spec::plain(0, "original")]);
    std::fs::write(&source, &original).unwrap();
    let (tx, rx, _) = start_worker();
    // A save without a loaded document must terminate, rather than hang.
    tx.send(Request::Save { generation: 1, changes: Changes::default(), arrangement: None, new_pages: vec![] }).unwrap();
    assert!(matches!(next_reply(&rx), Reply::SaveFailed { generation: 1, .. }));
    tx.send(Request::Open { generation: 1, path: source.clone() }).unwrap();
    let revision = loop {
        match next_reply(&rx) {
            Reply::Opened { snapshot, .. } => {
                assert_eq!(snapshot.bytes(), original);
                break snapshot.file();
            }
            Reply::OpenFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    };
    // A lazy reader must not reopen the path and see a different document.
    let external = b"another program replaced this file";
    std::fs::write(&source, external).unwrap();
    tx.send(Request::ReadMeasurements { generation: 1 }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Measured { file, .. } => { assert_eq!(file, revision); break; }
            Reply::MeasureFailed { error, .. } => panic!("snapshot read failed: {error}"),
            _ => {}
        }
    }
    let changes = || Changes {
        adds: vec![NewHighlight {
            page: 0, quads: vec![PdfBox { left: 10.0, bottom: 10.0, right: 100.0, top: 30.0 }],
            color: [1.0, 1.0, 0.0], comment: "keep my edit".into(),
        }],
        ..Changes::default()
    };
    tx.send(Request::Save { generation: 1, changes: changes(), arrangement: None, new_pages: vec![] }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::SaveFailed { error, .. } => { assert!(error.contains("changed in another program")); break; }
            Reply::Saved { .. } | Reply::SaveTarget { .. } => panic!("a conflicting save committed"),
            _ => {}
        }
    }
    assert_eq!(std::fs::read(&source).unwrap(), external);
    tx.send(Request::SaveAs { overwrite: true, generation: 1, path: target.clone(), changes: changes(), arrangement: None, new_pages: vec![] }).unwrap();
    let committed = loop {
        match next_reply(&rx) {
            Reply::Saved { snapshot, highlights, .. } => {
                assert_ne!(snapshot.file(), revision);
                assert_eq!(snapshot.bytes(), std::fs::read(&target).unwrap());
                assert!(highlights.iter().any(|h| h.comment == "keep my edit"));
                break snapshot.file();
            }
            Reply::SaveFailed { error, .. } => panic!("Save As failed: {error}"),
            _ => {}
        }
    };
    assert_eq!(std::fs::read(&source).unwrap(), external);
    tx.send(Request::ReadMeasurements { generation: 1 }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Measured { file, .. } => { assert_eq!(file, committed); break; }
            Reply::MeasureFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
}
