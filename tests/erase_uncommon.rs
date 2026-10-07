//! Exercise the fallback through the real worker and reopen the saved PDF.
mod common;
#[path = "../crates/gpu-lines/src/test_font.rs"]
mod fonts;

use common::{build_pdf, next_reply, start_worker};
use kinetic_pdf::{domain::{Changes, Erasure, TextChar}, protocol::{Reply, Request}};
use pdf_content::lopdf::{dictionary, Dictionary, Document, Object, Stream};
use std::sync::mpsc::{Sender, Receiver};

fn unicode(entries: &str) -> Stream {
    let spaces = if entries.split_whitespace().next().unwrap().len() == 6 { "1 begincodespacerange <0000> <ffff>" } else { "2 begincodespacerange <00> <7f> <8000> <ffff>" };
    Stream::new(dictionary! {}, format!("/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /TestUnicode def /CMapType 2 def {spaces} endcodespacerange 3 beginbfchar {entries} endbfchar endcmap CMapName currentdict /CMap defineresource pop end end").into_bytes())
}

fn indirect(object: Object, doc: &mut Document) -> Object {
    match object {
        Object::Dictionary(d) => Object::Dictionary(d.into_iter().map(|(k,v)| (k,indirect(v,doc))).collect()),
        Object::Array(a) => Object::Array(a.into_iter().map(|v| indirect(v,doc)).collect()),
        Object::Stream(mut s) => {
            s.dict = s.dict.into_iter().map(|(k,v)| (k,indirect(v,doc))).collect();
            Object::Reference(doc.add_object(s))
        }
        other => other,
    }
}

fn type3(coloured: bool) -> Dictionary {
    let glyph = if coloured { "600 0 d0 0 0 500 700 re f" } else { "600 0 0 0 500 700 d1 0 0 500 700 re f" };
    let proc = || Object::Stream(Stream::new(dictionary! {}, glyph.as_bytes().to_vec()));
    dictionary! {
        "Type" => "Font", "Subtype" => "Type3", "Name" => "F",
        "FontBBox" => vec![0.into(),0.into(),500.into(),700.into()],
        "FontMatrix" => vec![0.001.into(),0.into(),0.into(),0.001.into(),0.into(),0.into()],
        "FirstChar" => 65, "LastChar" => 67, "Widths" => vec![600.into(),600.into(),600.into()],
        "Encoding" => dictionary! { "Type" => "Encoding", "Differences" => vec![65.into(),Object::Name(b"A".to_vec()),Object::Name(b"B".to_vec()),Object::Name(b"C".to_vec())] },
        "CharProcs" => dictionary! { "A" => proc(), "B" => proc(), "C" => proc() },
        "Resources" => dictionary! {}, "ToUnicode" => unicode("<41> <0041> <42> <0042> <43> <0043>")
    }
}

fn cid(encoding: Object, mapping: &str) -> Dictionary {
    let mut font = fonts::type0_font();
    font.set("Encoding", encoding);
    font.set("ToUnicode", unicode(mapping));
    let Object::Array(descendants) = font.get_mut(b"DescendantFonts").unwrap() else { panic!() };
    let descendant = descendants[0].as_dict_mut().unwrap();
    descendant.set("CIDSystemInfo", dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Japan1"), "Supplement" => 0 });
    // Every code shows the same square, with distinct Unicode for extraction.
    descendant.set("CIDToGIDMap", Stream::new(dictionary! {}, (0..256).flat_map(|_| [0,1]).collect()));
    font
}

fn text(tx: &Sender<Request>, rx: &Receiver<Reply>, generation: u64, page: usize) -> Vec<TextChar> {
    tx.send(Request::Text { generation, page }).unwrap();
    loop { if let Reply::Text { chars, .. } = next_reply(rx) { return chars; } }
}

#[test]
fn uncommon_fonts_erase_actual_characters_without_moving_or_rasterizing_the_rest() {
    let cmap = Stream::new(dictionary! {}, b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def /CMapName /Mixed def /CMapType 1 def /WMode 0 def 2 begincodespacerange <00> <7f> <8100> <81ff> endcodespacerange 3 begincidchar <41> 1 <8101> 2 <42> 3 endcidchar endcmap CMapName currentdict /CMap defineresource pop end end".to_vec());
    let cases = vec![
        (type3(false), "<414243>"),
        (type3(true), "<414243>"),
        (cid(cmap.into(), "<41> <0041> <8101> <0042> <42> <0043>"), "<41810142>"),
        (cid(Object::Name(b"Identity-V".to_vec()), "<0001> <0041> <0002> <0042> <0003> <0043>"), "<000100020003>"),
        (cid(Object::Name(b"UniJIS-UTF16-H".to_vec()), "<0041> <0041> <0042> <0042> <0043> <0043>"), "<004100420043>"),
    ];
    let mut fixture = Document::load_mem(&build_pdf(cases.len(), &[])).unwrap();
    for (index, (font, chars)) in cases.iter().enumerate() {
        let page = fixture.get_pages()[&(index as u32 + 1)];
        let contents = fixture.add_object(Stream::new(dictionary! {}, format!("BT /F 20 Tf 50 500 Td {chars} Tj ET").into_bytes()));
        let font = indirect(Object::Dictionary(font.clone()), &mut fixture);
        let page = fixture.get_dictionary_mut(page).unwrap();
        page.set("Resources", dictionary! { "Font" => dictionary! { "F" => font } });
        page.set("Contents", contents);
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uncommon.pdf");
    fixture.save(&path).unwrap();
    let (tx, rx, wanted) = start_worker();
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    loop {
        match next_reply(&rx) { Reply::Opened { .. } => break, Reply::OpenFailed { error, .. } => panic!("{error}"), _ => {} }
    }
    { let mut w = wanted.lock().unwrap(); w.generation = 1; w.pages = (0..cases.len()).collect(); }
    let mut erasures = Vec::new();
    let mut before = Vec::new();
    for page in 0..cases.len() {
        let chars = text(&tx, &rx, 1, page);
        let b = chars.iter().find(|c| c.ch == 'B').unwrap_or_else(|| panic!("no B on page {page}: {chars:?}")).ink_bounds.unwrap();
        erasures.push(Erasure { page, layer: None, region: vec![
            [b.left-0.2,b.bottom-0.2], [b.right+0.2,b.bottom-0.2], [b.right+0.2,b.top+0.2], [b.left-0.2,b.top+0.2],
        ] });
        before.push(chars);
    }
    tx.send(Request::Save { generation: 1, changes: Changes { erasures, ..Default::default() }, arrangement: None, new_pages: vec![] }).unwrap();
    loop {
        match next_reply(&rx) { Reply::Saved { .. } => break, Reply::SaveFailed { error, .. } => panic!("{error}"), _ => {} }
    }
    for generation in [1,2] {
        if generation == 2 {
            tx.send(Request::Open { generation, path: path.clone() }).unwrap();
            loop {
                match next_reply(&rx) { Reply::Opened { .. } => break, Reply::OpenFailed { error, .. } => panic!("{error}"), _ => {} }
            }
            wanted.lock().unwrap().generation = generation;
        }
        for page in 0..cases.len() {
            let after = text(&tx, &rx, generation, page);
            assert!(!after.iter().any(|c| c.ch == 'B'), "erased glyph remains on page {page}: {after:?}");
            for ch in ['A','C'] {
                let old = before[page].iter().find(|c| c.ch == ch).unwrap().ink_bounds.unwrap();
                let kept = after.iter().find(|c| c.ch == ch).unwrap_or_else(|| panic!("{ch} lost on page {page}: {after:?}")).ink_bounds.unwrap();
                assert!((old.left-kept.left).abs() < 0.02 && (old.bottom-kept.bottom).abs() < 0.02, "{ch} moved on page {page}: {old:?} -> {kept:?}");
            }
        }
    }
    let saved = Document::load(&path).unwrap();
    assert!(!saved.objects.values().filter_map(|o| o.as_stream().ok()).any(|s| s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image")), "text stays vector text");

    // A broken, unresolved CMap must not silently produce a visually erased
    // PDF that still contains the text. Failed preparation leaves the file alone.
    let mut broken = Document::load_mem(&build_pdf(1, &[])).unwrap();
    let font = cid(Object::Name(b"MissingVendorCMap".to_vec()), "<0041> <0041> <0042> <0042> <0043> <0043>");
    let font = indirect(Object::Dictionary(font), &mut broken);
    let page = broken.get_pages()[&1];
    broken.get_dictionary_mut(page).unwrap().set("Resources", dictionary! { "Font" => dictionary! { "F1" => font } });
    let invalid_path = dir.path().join("unresolved.pdf");
    broken.save(&invalid_path).unwrap();
    let original = std::fs::read(&invalid_path).unwrap();
    tx.send(Request::Open { generation: 3, path: invalid_path.clone() }).unwrap();
    loop {
        match next_reply(&rx) { Reply::Opened { .. } => break, Reply::OpenFailed { error, .. } => panic!("{error}"), _ => {} }
    }
    tx.send(Request::Save { generation: 3, changes: Changes { erasures: vec![Erasure {
        page: 0, layer: None, region: vec![[0.0,0.0],[612.0,0.0],[612.0,792.0],[0.0,792.0]],
    }], ..Default::default() }, arrangement: None, new_pages: vec![] }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { .. } => panic!("an unresolved font must not silently fall back to clipping"),
            Reply::SaveFailed { error, .. } => { assert!(error.contains("decode"), "{error}"); break; }
            _ => {}
        }
    }
    assert_eq!(std::fs::read(invalid_path).unwrap(), original);
}
