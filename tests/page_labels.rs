mod common;
use common::*;
use kinetic_pdf::{domain::Changes, model::{Reply, Request}};

#[test]
fn unicode_page_labels_survive_save_reopen_and_page_reordering() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("labels.pdf");
    std::fs::write(&path, build_pdf(3, &[])).unwrap();
    let (tx, rx, _) = start_worker();
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    loop { if let Reply::Opened { .. } = next_reply(&rx) { break; } }
    tx.send(Request::Save { generation: 1, changes: Changes { page_labels: Some(vec![Some("A-101".into()), Some("測定 – café".into()), None]), ..Default::default() }, arrangement: Some(vec![kinetic_pdf::arrange::Sheet::of_page(1), kinetic_pdf::arrange::Sheet::of_page(0), kinetic_pdf::arrange::Sheet::of_page(2)]), new_pages: vec![] }).unwrap();
    loop { match next_reply(&rx) { Reply::Saved { .. } => break, Reply::SaveFailed { error, .. } => panic!("{error}"), _ => {} } }
    tx.send(Request::Open { generation: 2, path: path.clone() }).unwrap();
    loop {
        if let Reply::Opened { page_labels, .. } = next_reply(&rx) {
            assert_eq!(page_labels[0].as_deref(), Some("測定 – café"));
            assert_eq!(page_labels[1].as_deref(), Some("A-101"));
            break;
        }
    }
}
