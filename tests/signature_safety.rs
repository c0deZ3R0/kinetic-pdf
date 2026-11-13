//! Signature safety regressions: synthetic discovery fixtures and mutation barriers.
//!
//! PATH: tests/signature_safety.rs
//! CONSUMED BY: cargo test --test signature_safety; uses tests/common worker helpers.
//! Only the worker loads PDFium. Populated signatures are NOT cryptographically valid.
//! Deferred discovery is revision-bound; interleaved rendering must not unlock edits.
//! Discovery is not historical/orphan-signature detection, trust, or security validation.

mod common;

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};

use common::{build_pdf, next_reply, start_worker};
use kinetic_pdf::arrange::Sheet;
use kinetic_pdf::domain::SignatureDiscovery;
use kinetic_pdf::model::{Changes, Highlight, NewHighlight, PdfBox, Reply, Request};
use kinetic_pdf::session::{Command, Session};
use pdf_content::lopdf::{dictionary, Document, Object, StringFormat};

#[derive(Clone, Copy)]
enum FieldValue { Missing, Null, Populated, Direct, MalformedString, MalformedInteger, EmptyDictionary }

#[derive(Clone, Copy)]
enum FieldLayout { Flat, Inherited, ChildTyped }

fn synthetic_signature() -> pdf_content::lopdf::Dictionary {
    dictionary! {
        "Type" => "Sig", "Filter" => "Adobe.PPKLite", "SubFilter" => "adbe.pkcs7.detached",
        "ByteRange" => vec![Object::Integer(0), Object::Integer(1), Object::Integer(2), Object::Integer(1)],
        "Contents" => Object::String(vec![0x30, 0x00], StringFormat::Hexadecimal)
    }
}

/// Add real AcroForm signature widgets to the shared two-page text fixture.
/// Invalid CMS/ByteRange values intentionally exercise discovery, not validation.
fn signature_fixture(values: &[FieldValue]) -> Vec<u8> {
    field_fixture(values, false)
}

/// Two levels of Kids: the widget inherits FT from its root field, not its
/// immediate parent. V remains on the terminal field and may be direct/indirect.
fn field_fixture(values: &[FieldValue], nested: bool) -> Vec<u8> {
    field_fixture_layout(values, if nested { FieldLayout::Inherited } else { FieldLayout::Flat })
}

/// ChildTyped leaves both ancestor containers without FT; only the terminal
/// widget is Sig. PDFium's top-level-only signature counter can report zero.
fn field_fixture_layout(values: &[FieldValue], layout: FieldLayout) -> Vec<u8> {
    let nested = !matches!(layout, FieldLayout::Flat);
    let mut doc = Document::load_mem(&build_pdf(2, &[])).unwrap();
    let page = doc.get_pages()[&1];
    let mut fields = Vec::new();
    let mut widgets = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let mut field = dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Sig",
            "T" => Object::string_literal(format!("Synthetic signature {}", index + 1)),
            "P" => page, "F" => 4,
            "Rect" => vec![Object::Integer(20), Object::Integer(20), Object::Integer(100), Object::Integer(60)]
        };
        match value {
            FieldValue::Missing => {},
            FieldValue::Null => field.set("V", Object::Null),
            FieldValue::Populated => {
                let signature = doc.add_object(synthetic_signature());
                field.set("V", signature);
            },
            FieldValue::Direct => field.set("V", synthetic_signature()),
            FieldValue::MalformedString => field.set("V", Object::string_literal("not a signature dictionary")),
            FieldValue::MalformedInteger => field.set("V", Object::Integer(7)),
            FieldValue::EmptyDictionary => field.set("V", dictionary! {}),
        }
        if matches!(layout, FieldLayout::Inherited) { field.remove(b"FT"); }
        let widget = doc.add_object(field);
        widgets.push(Object::Reference(widget));
        if nested {
            let branch = doc.add_object(dictionary! {
                "T" => Object::string_literal("Branch"), "Kids" => vec![Object::Reference(widget)]
            });
            let mut root = dictionary! {
                "T" => Object::string_literal(format!("Root {index}")), "Kids" => vec![Object::Reference(branch)]
            };
            if matches!(layout, FieldLayout::Inherited) { root.set("FT", "Sig"); }
            let root = doc.add_object(root);
            doc.get_dictionary_mut(widget).unwrap().set("Parent", branch);
            doc.get_dictionary_mut(branch).unwrap().set("Parent", root);
            fields.push(Object::Reference(root));
        } else {
            fields.push(Object::Reference(widget));
        }
    }
    doc.get_dictionary_mut(page).unwrap().set("Annots", widgets);
    let form = doc.add_object(dictionary! { "Fields" => fields, "SigFlags" => 3 });
    doc.catalog_mut().unwrap().set("AcroForm", form);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

/// Ordinary populated non-signature form fields must not remain locked merely
/// because a document-level AcroForm hint scheduled deferred inventory.
fn ordinary_form_fixture() -> Vec<u8> {
    let mut doc = Document::load_mem(&build_pdf(2, &[])).unwrap();
    let page = doc.get_pages()[&1];
    let mut fields = Vec::new();
    for (kind, value) in [("Tx", Object::string_literal("ordinary text")),
        ("Btn", Object::Name(b"Off".to_vec())), ("Ch", Object::string_literal("Alpha"))] {
        let mut field = dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => kind,
            "T" => Object::string_literal(format!("Ordinary {kind}")), "V" => value,
            "P" => page, "F" => 4,
            "Rect" => vec![Object::Integer(20), Object::Integer(20), Object::Integer(100), Object::Integer(60)]
        };
        if kind == "Ch" { field.set("Opt", vec![Object::string_literal("Alpha"), Object::string_literal("Beta")]); }
        fields.push(Object::Reference(doc.add_object(field)));
    }
    doc.get_dictionary_mut(page).unwrap().set("Annots", fields.clone());
    let form = doc.add_object(dictionary! { "Fields" => fields });
    doc.catalog_mut().unwrap().set("AcroForm", form);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

/// Wait for Opened, then its exact revision's eventual discovery. Background
/// Highlights/render replies and obsolete discovery completions may interleave.
fn open_discovery(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, path: &Path, bytes: &[u8]) -> SignatureDiscovery {
    tx.send(Request::Open { generation, path: path.to_owned() }).unwrap();
    let file = loop {
        match next_reply(rx) {
            Reply::Opened { generation: g, path: opened, snapshot, page_sizes, .. } if g == generation => {
                assert_eq!(opened, path);
                assert_eq!(snapshot.bytes(), bytes);
                assert_eq!(page_sizes.len(), 2);
                break snapshot.file();
            },
            Reply::OpenFailed { generation: g, error } if g == generation => panic!("fixture did not open: {error}"),
            Reply::Saved { .. } | Reply::SaveTarget { .. } => panic!("unexpected save while opening"),
            _ => {},
        }
    };
    loop {
        match next_reply(rx) {
            Reply::SignatureDiscovery { generation: g, file: discovered_file, discovery }
                if g == generation && discovered_file == file => return discovery,
            Reply::OpenFailed { generation: g, error } if g == generation => panic!("fixture failed during discovery: {error}"),
            Reply::Saved { .. } | Reply::SaveTarget { .. } => panic!("unexpected save while awaiting discovery"),
            _ => {},
        }
    }
}

#[test]
fn discovery_waiter_ignores_stale_unlocks_and_interleaved_page_work() {
    // Pure protocol fixture, not a delayed native-worker race or app replacement.
    let (tx, _requests) = std::sync::mpsc::channel();
    let (replies, rx) = std::sync::mpsc::channel();
    let bytes = b"synthetic immutable protocol revision";
    let snapshot = kinetic_pdf::document::Snapshot::new(bytes.to_vec());
    let file = snapshot.file();
    let path = std::path::PathBuf::from("protocol-only.pdf");
    let generation = 7;
    replies.send(Reply::Opened {
        generation, path: path.clone(), snapshot, page_sizes: vec![[612.0, 792.0]; 2], page_labels: vec![None; 2],
    }).unwrap();
    for (g, f) in [(generation - 1, file), (generation, file.wrapping_add(1))] {
        replies.send(Reply::SignatureDiscovery { generation: g, file: f, discovery: SignatureDiscovery::NoSignaturesDetected }).unwrap();
    }
    replies.send(Reply::Highlights { generation, highlights: vec![], markups: vec![], geometry: vec![], done: true }).unwrap();
    replies.send(Reply::RenderSkipped { generation, page: 0 }).unwrap();
    replies.send(Reply::SignatureDiscovery { generation, file, discovery: SignatureDiscovery::Present { count: 1 } }).unwrap();
    assert_eq!(open_discovery(&tx, &rx, generation, &path, bytes), SignatureDiscovery::Present { count: 1 });
}

const EDIT_COMMENT: &str = "must not be written to a protected source";

/// Positive recovery checks must prove the requested edit reached a page, not
/// merely that a parser can open rewritten bytes or find an empty Annots array.
fn assert_written_edit(path: &Path, initial: &[u8]) {
    let bytes = std::fs::read(path).unwrap();
    assert_ne!(bytes, initial, "save left the initial unsigned bytes unchanged");
    let saved = Document::load_mem(&bytes).unwrap();
    assert_eq!(saved.get_pages().len(), 2);
    let page = saved.get_pages()[&1];
    let annots = saved.get_dictionary(page).unwrap().get_deref(b"Annots", &saved).unwrap().as_array().unwrap();
    assert_eq!(annots.len(), 1, "expected exactly the requested new highlight");
    let (_, annot) = saved.dereference(&annots[0]).unwrap();
    let annot = annot.as_dict().unwrap();
    assert_eq!(annot.get(b"Subtype").unwrap().as_name().unwrap(), b"Highlight");
    assert_eq!(pdf_content::lopdf::decode_text_string(annot.get(b"Contents").unwrap()).unwrap(), EDIT_COMMENT);
}

fn changes(edited: bool) -> Changes {
    if !edited { return Changes::default(); }
    Changes {
        adds: vec![NewHighlight {
            page: 0, quads: vec![PdfBox { left: 72.0, bottom: 715.0, right: 200.0, top: 735.0 }],
            color: [1.0, 0.9, 0.0], comment: EDIT_COMMENT.into(),
        }],
        ..Changes::default()
    }
}

fn sheets(edited: bool) -> Vec<Sheet> {
    if edited {
        // Reorder, rotate, duplicate, and include a newly appended blank page.
        vec![Sheet::of_page(1).turned(1), Sheet::of_page(0), Sheet::of_page(0), Sheet::of_page(2)]
    } else {
        vec![Sheet::of_page(0)]
    }
}

fn blanks(edited: bool) -> Vec<[f32; 2]> {
    if edited { vec![[210.0, 300.0]] } else { vec![] }
}

fn save_refused(rx: &Receiver<Reply>, generation: u64, discovery: SignatureDiscovery) {
    let error = save_error(rx, generation);
    assert_eq!(Some(error.as_str()), discovery.read_only_reason(), "must fail at signature barrier, not later in PDF rewriting");
}

fn save_error(rx: &Receiver<Reply>, generation: u64) -> String {
    loop {
        match next_reply(rx) {
            Reply::SaveFailed { generation: g, error } => {
                assert_eq!(g, generation);
                return error;
            },
            Reply::Saved { .. } | Reply::SaveTarget { .. } => panic!("protected save committed or switched destination"),
            Reply::ExtractedPages { .. } => panic!("unexpected extraction reply"),
            _ => {},
        }
    }
}

fn extract_refused(rx: &Receiver<Reply>, generation: u64, target: &Path, count: usize, discovery: SignatureDiscovery) {
    let error = extract_error(rx, generation, target, count);
    assert_eq!(Some(error.as_str()), discovery.read_only_reason());
}

fn extract_error(rx: &Receiver<Reply>, generation: u64, target: &Path, count: usize) -> String {
    loop {
        match next_reply(rx) {
            Reply::ExtractedPages { generation: g, path, count: actual, result } => {
                assert_eq!(g, generation);
                assert_eq!(path, target);
                assert_eq!(actual, count);
                return result.expect_err("protected extraction must fail");
            },
            Reply::Saved { .. } | Reply::SaveTarget { .. } => panic!("extraction committed a save"),
            Reply::SaveFailed { .. } => panic!("extraction returned the wrong failure protocol"),
            _ => {},
        }
    }
}

#[test]
fn worker_discovery_distinguishes_empty_fields_from_populated_signatures() {
    // Only worker threads load PDFium; never bind it directly in a fixture.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.pdf");
    let (tx, rx, _) = start_worker();
    let unsigned = build_pdf(2, &[]);
    let cases = [
        ("ordinary", unsigned.clone(), SignatureDiscovery::NoSignaturesDetected),
        ("ordinary non-signature AcroForm", ordinary_form_fixture(), SignatureDiscovery::NoSignaturesDetected),
        ("missing V", signature_fixture(&[FieldValue::Missing]), SignatureDiscovery::NoSignaturesDetected),
        ("null V", signature_fixture(&[FieldValue::Null]), SignatureDiscovery::NoSignaturesDetected),
        ("indirect V", signature_fixture(&[FieldValue::Populated]), SignatureDiscovery::Present { count: 1 }),
        ("direct V", signature_fixture(&[FieldValue::Direct]), SignatureDiscovery::Present { count: 1 }),
        ("mixed", signature_fixture(&[FieldValue::Populated, FieldValue::Missing, FieldValue::Populated]), SignatureDiscovery::Present { count: 2 }),
        ("nested missing V", field_fixture(&[FieldValue::Missing], true), SignatureDiscovery::NoSignaturesDetected),
        ("nested null V", field_fixture(&[FieldValue::Null], true), SignatureDiscovery::NoSignaturesDetected),
        ("nested indirect V", field_fixture(&[FieldValue::Populated], true), SignatureDiscovery::Present { count: 1 }),
        ("nested direct V", field_fixture(&[FieldValue::Direct], true), SignatureDiscovery::Present { count: 1 }),
        ("untyped containers / child indirect Sig", field_fixture_layout(&[FieldValue::Populated], FieldLayout::ChildTyped), SignatureDiscovery::Present { count: 1 }),
        ("untyped containers / child direct Sig", field_fixture_layout(&[FieldValue::Direct], FieldLayout::ChildTyped), SignatureDiscovery::Present { count: 1 }),
        ("untyped containers / empty child Sig", field_fixture_layout(&[FieldValue::Missing], FieldLayout::ChildTyped), SignatureDiscovery::NoSignaturesDetected),
    ];
    for (index, (label, bytes, expected)) in cases.iter().enumerate() {
        std::fs::write(&source, bytes).unwrap();
        let generation = index as u64 + 1;
        assert_eq!(open_discovery(&tx, &rx, generation, &source, bytes), *expected, "discovery case: {label}");
        if !expected.allows_document_mutation() {
            // Includes child-typed signatures that the native top-level counter
            // misses. Test the worker barrier, not only the reported UI status.
            tx.send(Request::Save { generation, changes: changes(true), arrangement: None, new_pages: vec![] }).unwrap();
            save_refused(&rx, generation, *expected);
        }
        assert_eq!(std::fs::read(&source).unwrap(), *bytes, "discovery/refusal must not rewrite fixtures");
    }
}

#[test]
fn malformed_non_null_signature_values_fail_closed_at_discovery_and_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("malformed-value.pdf");
    let target = dir.path().join("unsigned-target.pdf");
    let missing = dir.path().join("new-extract.pdf");
    let unsigned = build_pdf(2, &[]);
    std::fs::write(&target, &unsigned).unwrap();
    let (tx, rx, _) = start_worker();
    let mut generation = 0;
    for nested in [false, true] {
        for value in [FieldValue::MalformedString, FieldValue::MalformedInteger, FieldValue::EmptyDictionary] {
            generation += 1;
            let bytes = field_fixture(&[value], nested);
            std::fs::write(&source, &bytes).unwrap();
            let discovery = open_discovery(&tx, &rx, generation, &source, &bytes);
            // Unknown or Present is conservative; NoSignaturesDetected is not.
            // No malformed value is interpreted as a usable signing identity.
            assert!(!discovery.allows_document_mutation(), "malformed non-null V was treated as unsigned (nested={nested})");
            tx.send(Request::Save { generation, changes: changes(true), arrangement: Some(sheets(true)), new_pages: blanks(true) }).unwrap();
            save_refused(&rx, generation, discovery);
            tx.send(Request::SaveAs {
                generation, path: target.clone(), changes: Changes::default(), arrangement: None, new_pages: vec![],
            }).unwrap();
            save_refused(&rx, generation, discovery);
            tx.send(Request::ExtractPages {
                generation, path: missing.clone(), changes: changes(true), sheets: sheets(true), new_pages: blanks(true),
            }).unwrap();
            extract_refused(&rx, generation, &missing, sheets(true).len(), discovery);
            assert_eq!(std::fs::read(&source).unwrap(), bytes);
            assert_eq!(std::fs::read(&target).unwrap(), unsigned);
            assert!(!missing.exists());
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        }
    }
    // Malformed discovery must not poison the next unsigned document.
    std::fs::write(&source, &unsigned).unwrap();
    assert_eq!(open_discovery(&tx, &rx, generation + 1, &source, &unsigned), SignatureDiscovery::NoSignaturesDetected);
}

#[test]
fn worker_refuses_direct_mutation_of_signed_source_without_touching_files() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.pdf");
    let target = dir.path().join("existing-target.pdf");
    let missing = dir.path().join("new-target.pdf");
    let (tx, rx, wanted) = start_worker();
    let unsigned = build_pdf(2, &[]);
    let signed = signature_fixture(&[FieldValue::Populated]);
    std::fs::write(&source, &signed).unwrap();
    let generation = 10;
    let discovery = open_discovery(&tx, &rx, generation, &source, &signed);
    assert_eq!(discovery, SignatureDiscovery::Present { count: 1 });
    let sentinel = b"existing destination: do not overwrite";
    std::fs::write(&target, sentinel).unwrap();
    let unchanged = || {
        assert_eq!(std::fs::read(&source).unwrap(), signed, "protected source bytes changed");
        assert_eq!(std::fs::read(&target).unwrap(), sentinel, "existing destination bytes changed");
        assert!(!missing.exists(), "refusal created a destination");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2, "refusal left an extra temporary file");
    };

    for edited in [false, true] {
        tx.send(Request::Save {
            generation, changes: changes(edited), arrangement: edited.then(|| sheets(true)), new_pages: blanks(edited),
        }).unwrap();
        save_refused(&rx, generation, discovery);
        unchanged();
        for destination in [&target, &missing, &source] {
            tx.send(Request::SaveAs {
                generation, path: destination.clone(), changes: changes(edited),
                arrangement: edited.then(|| sheets(true)), new_pages: blanks(edited),
            }).unwrap();
            save_refused(&rx, generation, discovery);
            unchanged();

            let selection = sheets(edited);
            let count = selection.len();
            tx.send(Request::ExtractPages {
                generation, path: destination.clone(), changes: changes(edited), sheets: selection, new_pages: blanks(edited),
            }).unwrap();
            extract_refused(&rx, generation, destination, count, discovery);
            unchanged();
        }
    }

    // The protected document is still readable and its original page order survived.
    {
        let mut w = wanted.lock().unwrap();
        w.generation = generation;
        w.pages = vec![1];
    }
    tx.send(Request::Text { generation, page: 1 }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Text { generation: g, page, chars } => {
                assert_eq!((g, page), (generation, 1));
                let text: String = chars.iter().map(|c| c.ch).collect();
                assert!(text.contains("Page 2 line 1"), "refusals damaged the loaded document");
                break;
            },
            Reply::TextSkipped { .. } | Reply::SaveTarget { .. } | Reply::Saved { .. } => panic!("worker unusable or late save after refusal"),
            _ => {},
        }
    }
    unchanged();

    // Reopening an unsigned file must clear the protection and permit real saves.
    let ordinary = dir.path().join("unsigned.pdf");
    std::fs::write(&ordinary, &unsigned).unwrap();
    assert_eq!(open_discovery(&tx, &rx, 20, &ordinary, &unsigned), SignatureDiscovery::NoSignaturesDetected);

    // The conservative destination guard also refuses malformed/non-PDF files.
    // Replace the sentinel with a valid unsigned PDF for the positive save case.
    std::fs::write(&target, &unsigned).unwrap();
    tx.send(Request::SaveAs {
        generation: 20, path: target.clone(), changes: changes(true), arrangement: None, new_pages: vec![],
    }).unwrap();
    let mut switched = false;
    loop {
        match next_reply(&rx) {
            Reply::SaveTarget { generation: g, path } => {
                assert_eq!(g, 20);
                assert_eq!(path, target);
                switched = true;
            },
            Reply::Saved { generation: g, .. } => {
                assert_eq!(g, 20);
                assert!(switched, "successful SaveAs must announce the new destination");
                break;
            },
            Reply::SaveFailed { error, .. } => panic!("worker remained protected after unsigned reopen: {error}"),
            _ => {},
        }
    }
    assert_written_edit(&target, &unsigned);
    assert_eq!(std::fs::read(&ordinary).unwrap(), unsigned);
    assert_eq!(std::fs::read(&source).unwrap(), signed);
    assert!(!missing.exists());
}

#[test]
fn unsigned_source_cannot_overwrite_a_detected_signed_destination() {
    // Requires the shared output-safety hook; separate so source-barrier coverage
    // can run independently while that hook is being integrated.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("unsigned.pdf");
    let target = dir.path().join("signed-target.pdf");
    let unsigned = build_pdf(2, &[]);
    let signed = signature_fixture(&[FieldValue::Populated]);
    std::fs::write(&source, &unsigned).unwrap();
    std::fs::write(&target, &signed).unwrap();
    let (tx, rx, _) = start_worker();
    let generation = 1;
    assert_eq!(open_discovery(&tx, &rx, generation, &source, &unsigned), SignatureDiscovery::NoSignaturesDetected);
    for edited in [false, true] {
        tx.send(Request::SaveAs {
            generation, path: target.clone(), changes: changes(edited),
            arrangement: edited.then(|| sheets(true)), new_pages: blanks(edited),
        }).unwrap();
        let error = save_error(&rx, generation);
        assert!(error.to_ascii_lowercase().contains("sign"), "expected signed-destination refusal, got: {error}");
        assert_eq!(std::fs::read(&target).unwrap(), signed);
        assert_eq!(std::fs::read(&source).unwrap(), unsigned);

        let selection = sheets(edited);
        let count = selection.len();
        tx.send(Request::ExtractPages {
            generation, path: target.clone(), changes: changes(edited), sheets: selection, new_pages: blanks(edited),
        }).unwrap();
        let error = extract_error(&rx, generation, &target, count);
        assert!(error.to_ascii_lowercase().contains("sign"), "expected signed-destination refusal, got: {error}");
        assert_eq!(std::fs::read(&target).unwrap(), signed);
        assert_eq!(std::fs::read(&source).unwrap(), unsigned);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2, "refusal left an extra temporary file");
    }
    // Refused SaveAs must not switch the loaded path or poison an ordinary Save.
    tx.send(Request::Save { generation, changes: changes(true), arrangement: None, new_pages: vec![] }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { generation: g, .. } => { assert_eq!(g, generation); break; },
            Reply::SaveFailed { error, .. } => panic!("worker unusable after destination refusal: {error}"),
            Reply::SaveTarget { .. } => panic!("refused SaveAs changed the destination"),
            _ => {},
        }
    }
    assert_written_edit(&source, &unsigned);
    assert_eq!(std::fs::read(&target).unwrap(), signed);
}

fn highlight(comment: &str) -> Highlight {
    Highlight {
        key: None, page: 0, quads: vec![PdfBox { left: 20.0, bottom: 20.0, right: 100.0, top: 40.0 }],
        color: [1.0, 0.9, 0.0], comment: comment.into(), author: String::new(), snippet: String::new(),
    }
}

#[test]
fn unknown_and_present_policy_blocks_session_edits_saves_and_history_without_losing_work() {
    assert!(SignatureDiscovery::NoSignaturesDetected.allows_document_mutation());
    assert!(SignatureDiscovery::NoSignaturesDetected.read_only_reason().is_none());
    for discovery in [SignatureDiscovery::Unknown, SignatureDiscovery::Present { count: 0 },
        SignatureDiscovery::Present { count: 1 }, SignatureDiscovery::Present { count: 65_536 }] {
        assert!(!discovery.allows_document_mutation());
        assert!(discovery.read_only_reason().is_some());
        let mut session = Session::default();
        session.set_signature_discovery(SignatureDiscovery::NoSignaturesDetected);
        session.apply(Command::AddHighlights(vec![highlight("keep")]));
        session.apply(Command::AddHighlights(vec![highlight("redo later")]));
        assert!(session.undo());
        assert!(session.can_undo() && session.can_redo() && session.is_dirty());
        // The app explicitly sets Unknown pending the snapshot-bound reply;
        // Session::default is also used for new, unsigned documents.
        session.set_signature_discovery(discovery);
        assert!(session.is_signature_read_only());
        assert!(!session.can_edit() && !session.can_undo() && !session.can_redo());
        assert!(session.apply(Command::AddHighlights(vec![highlight("blocked")])).is_empty());
        assert!(session.apply_merged(Command::AddHighlights(vec![highlight("blocked merge")])).is_empty());
        assert!(!session.undo());
        assert!(!session.redo());
        assert!(session.begin_save("synthetic tester".into()).is_none());
        assert!(!session.is_saving());
        assert!(session.is_dirty());
        assert_eq!(session.highlights().len(), 1);
        assert_eq!(session.highlights()[0].hl.comment, "keep");

        session.set_signature_discovery(SignatureDiscovery::NoSignaturesDetected);
        assert!(!session.is_signature_read_only());
        assert!(session.can_edit() && session.can_undo() && session.can_redo());
        assert!(session.redo(), "read-only guard must not consume redo history");
        assert_eq!(session.highlights().len(), 2);
        assert!(session.begin_save("synthetic tester".into()).is_some());
        session.save_failed();
    }
}

#[test]
fn signature_discovery_does_not_clear_an_independent_editing_block() {
    let mut session = Session::default();
    session.block_editing(true);
    session.set_signature_discovery(SignatureDiscovery::Unknown);
    session.set_signature_discovery(SignatureDiscovery::NoSignaturesDetected);
    assert!(!session.is_signature_read_only());
    assert!(!session.can_edit());
    assert!(session.apply(Command::AddHighlights(vec![highlight("blocked")])).is_empty());
    session.block_editing(false);
    assert!(session.can_edit());
}
