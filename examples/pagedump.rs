//! What a page says about itself: its boxes, its rotation, its graphics
//! states, and the first of its drawing instructions. For working out why
//! something lands where it does, or is drawn the way it is.
//!
//!     cargo run --release --example pagedump -- file.pdf 2

use gpu_lines::lopdf::{Document, Object};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("give a PDF");
    let number: u32 = args.next().and_then(|n| n.parse().ok()).unwrap_or(1);
    let doc = Document::load(&path).expect("couldn't load it");
    let page_id = *doc.get_pages().get(&number).expect("no such page");
    let mut at = page_id;
    for step in 0..8 {
        let Ok(dict) = doc.get_dictionary(at) else { break };
        println!("-- level {step} ({at:?})");
        for key in [&b"Type"[..], b"MediaBox", b"CropBox", b"Rotate", b"UserUnit"] {
            if let Some(v) = dict.get(key).ok().and_then(|v| doc.dereference(v).ok()) {
                println!("   /{} = {:?}", String::from_utf8_lossy(key), v.1);
            }
        }
        match dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok()) {
            Some(up) => at = up,
            None => break,
        }
    }
    let content = doc.get_page_content(page_id);
    println!("-- content is {} bytes; first 400:", content.len());
    println!("{}", String::from_utf8_lossy(&content[..content.len().min(400)]));
    dump_gs(&doc, page_id);
    if let Some(Object::Dictionary(res)) = doc.get_dictionary(page_id).ok().and_then(|p| p.get(b"Resources").ok()).and_then(|r| doc.dereference(r).ok()).map(|(_, o)| o.clone()) {
        println!("-- resources: {:?}", res.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect::<Vec<_>>());
    }
}

fn dump_gs(doc: &Document, page_id: gpu_lines::lopdf::ObjectId) {
    let Some(res) = doc.get_dictionary(page_id).ok().and_then(|p| p.get(b"Resources").ok()).and_then(|r| doc.dereference(r).ok()) else { return };
    let Ok(res) = res.1.as_dict() else { return };
    let Some(gs) = res.get(b"ExtGState").ok().and_then(|g| doc.dereference(g).ok()) else { return };
    let Ok(gs) = gs.1.as_dict() else { return };
    println!("-- ExtGState:");
    for (name, value) in gs.iter() {
        let resolved = doc.dereference(value).map(|(_, v)| v.clone()).unwrap_or(Object::Null);
        println!("   /{} = {:?}", String::from_utf8_lossy(name), resolved);
    }
}
