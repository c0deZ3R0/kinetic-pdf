//! Explicit release benchmark; also builds when copied to the pre-MCP revision.
//! Run with --release --test mcp_performance -- --ignored --nocapture.
mod common;
use kinetic_pdf::{
    arrange::Sheet,
    domain::{Changes, Highlight, NewHighlight, PdfBox},
    model::{Reply, Request},
    session::{Command, Session},
};
use std::{hint::black_box, time::Instant};

fn highlight() -> Highlight {
    Highlight {
        key: None,
        page: 0,
        quads: vec![PdfBox {
            left: 70.0,
            bottom: 710.0,
            right: 250.0,
            top: 730.0,
        }],
        color: common::YELLOW,
        comment: "Benchmark".into(),
        author: String::new(),
        snippet: String::new(),
    }
}

fn report(operation: &str, samples: Vec<f64>) {
    println!(
        "PERF {}",
        serde_json::json!({"operation": operation, "samples_ms": samples})
    );
}

#[test]
#[ignore = "explicit release performance comparison"]
fn edit_and_save_large_document() {
    for pages in [300, 3000] {
        let mut session = Session::default();
        // Before MCP labels lived on Doc, outside Session; the newer revision
        // must include its label tracking in the measured editing path.
        #[cfg(feature = "mcp")]
        session.load_page_labels((0..pages).map(|p| Some(format!("Drawing-{p}"))).collect());
        let mut samples = Vec::new();
        for sample in 0..12 {
            let start = Instant::now();
            for _ in 0..1000 {
                session.apply(Command::AddHighlights(vec![highlight()]));
                assert!(session.undo());
                black_box(session.is_dirty());
            }
            if sample > 0 {
                samples.push(start.elapsed().as_secs_f64() * 1000.0 / 1000.0);
            }
        }
        report(&format!("edit_undo_{pages}_labels"), samples);
    }

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large.pdf");
    let bytes = common::dense_pdf_pages(300, 50);
    if let Some(fixture) = std::env::var_os("KINETIC_PERF_FIXTURE") {
        std::fs::write(fixture, &bytes).unwrap();
    }
    let (tx, rx, _) = common::start_worker();
    let mut generation = 0;
    for structural in [false, true] {
        let mut samples = Vec::new();
        for sample in 0..12 {
            std::fs::write(&path, &bytes).unwrap();
            generation += 1;
            common::open_and_read_all(&tx, &rx, generation, path.clone());
            let h = highlight();
            let changes = Changes {
                adds: vec![NewHighlight {
                    page: h.page,
                    quads: h.quads,
                    color: h.color,
                    comment: h.comment,
                }],
                ..Default::default()
            };
            let arrangement = structural.then(|| (0..300).rev().map(Sheet::of_page).collect());
            let start = Instant::now();
            tx.send(Request::Save {
                generation,
                changes,
                arrangement,
                new_pages: vec![],
            })
            .unwrap();
            loop {
                match common::next_reply(&rx) {
                    Reply::Saved {
                        generation: saved, ..
                    } if saved == generation => break,
                    Reply::SaveFailed { error, .. } => panic!("{error}"),
                    _ => {}
                }
            }
            if sample > 0 {
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        report(
            if structural {
                "save_reordered_300_pages"
            } else {
                "save_annotation_300_pages"
            },
            samples,
        );
    }
}
