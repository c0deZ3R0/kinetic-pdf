//! Render helpers read the same revision as the worker after external changes.
mod common;
use common::{build_pdf, next_reply, start_worker_with_helpers, take_pixels};
use kinetic_pdf::{model::{Reply, Request}, pool::Helpers};

#[test]
fn helper_render_survives_replacement_of_the_source_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.pdf");
    std::fs::write(&path, build_pdf(1, &[])).unwrap();
    let (tx, rx, wanted, ctx) = start_worker_with_helpers(Helpers::exe(env!("CARGO_BIN_EXE_kinetic-pdf"), 1));
    {
        let mut w = wanted.lock().unwrap();
        w.generation = 1;
        w.pages = vec![0];
        w.moving = true; // Avoid the worker scanning pages in this test.
        w.skip_drawing_ahead = true;
    }
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Opened { .. } => break,
            Reply::OpenFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    std::fs::write(&path, b"externally replaced").unwrap();
    tx.send(Request::Render { generation: 1, page: 0, scale: 0.5 }).unwrap();
    let texture = loop {
        match next_reply(&rx) {
            Reply::Rendered { texture, complete: true, .. } => break texture,
            Reply::RenderFailed { error, .. } => panic!("{error}"),
            Reply::Highlights { geometry, .. } if !geometry.is_empty() => panic!("worker fallback rendered instead of the helper"),
            _ => {}
        }
    };
    let pixels = take_pixels(&ctx);
    let (size, rgba) = &pixels[&texture.id()];
    assert_eq!(*size, [306, 396]);
    assert!(rgba.iter().any(|p| p.r() < 200), "the original page's text must still be rendered");
    assert_eq!(std::fs::read(&path).unwrap(), b"externally replaced");
}
