//! Read a saved PDF independently of the viewer cache; never writes the PDF.
use kinetic_pdf::{cache, model::{Reply, Request}, pool::Helpers, worker};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() {
    let path = std::path::PathBuf::from(std::env::args_os().nth(1).expect("PDF path"));
    let bytes = std::fs::read(&path).unwrap();
    let doc = pdf_content::lopdf::Document::load_mem(&bytes).unwrap();
    println!("Fingerprint: {:016x}; pages: {}", cache::fingerprint(&bytes), doc.get_pages().len());
    for (number, id) in doc.get_pages() {
        let page = doc.get_dictionary(id).unwrap();
        let text = doc.extract_text(&[number]).unwrap_or_default().replace(['\r', '\n'], " ");
        println!("{number}: object {id:?}, rotation {:?}, content bytes {}, text: {}", page.get(b"Rotate"), doc.get_page_content(id).len(), text.chars().take(100).collect::<String>());
    }
    let wanted = Arc::new(Mutex::new(worker::Wanted::default()));
    let (tx, rx) = worker::spawn(eframe::egui::Context::default(), wanted.clone(), Helpers::none(), None);
    tx.send(Request::Open { generation: 1, path }).unwrap();
    let count = loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            Reply::Opened { page_sizes, .. } => { println!("PDFium opened {} pages", page_sizes.len()); break page_sizes.len(); }
            Reply::OpenFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    };
    { let mut w = wanted.lock().unwrap(); w.generation = 1; w.pages = (0..count).collect(); }
    for page in 0..count {
        tx.send(Request::Render { generation: 1, page, scale: 0.05 }).unwrap();
        loop {
            match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
                Reply::Rendered { page: p, complete: true, .. } if p == page => break,
                Reply::RenderFailed { error, .. } => panic!("page {}: {error}", page + 1),
                _ => {}
            }
        }
    }
    println!("All {count} pages rendered without the cache");
}
