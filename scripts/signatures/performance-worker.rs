//! Headless paired performance workload for the real worker request/reply path.
//!
//! PATH: scripts/signatures/performance-worker.rs
//! CONSUMED BY: performance.py; shared workload copied into OS-temp snapshots.
//! A separate generated, hashed protocol adapter differs by available APIs.
//! Final analysis readiness is measured and awaited before Save; no fixed sleeps.
//! No GUI frames/GPU upload proof. Only generated scratch PDFs may be passed.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use kinetic_pdf::domain::{Changes, NewHighlight, PdfBox};
use kinetic_pdf::pool::Helpers;
use kinetic_pdf::protocol::{Reply, Request};
use kinetic_pdf::worker::{self, Wanted};

#[path = "signature-performance-adapter.rs"]
mod adapter;

fn elapsed_ms(start: Instant) -> f64 { start.elapsed().as_secs_f64() * 1000.0 }

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 { return Err("usage: signature-performance-worker GENERATED.pdf SCRATCH-DIR ITERATIONS".into()); }
    let input = PathBuf::from(&args[1]);
    let scratch = PathBuf::from(&args[2]);
    let iterations: usize = args[3].parse().map_err(|_| "invalid iteration count")?;
    if !(1..=100).contains(&iterations) { return Err("iterations must be 1..100".into()); }
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let initialize = Instant::now();
    worker::bind()?;
    let native_initialization_ms = elapsed_ms(initialize);
    let ctx = egui::Context::default();
    let wanted = Arc::new(Mutex::new(Wanted::default()));
    let (tx, rx) = worker::spawn(ctx.clone(), wanted.clone(), Helpers::none(), None);
    for iteration in 0..iterations {
        let generation = iteration as u64 + 1;
        let path = scratch.join(format!("iteration-{iteration}.pdf"));
        // Copying warms filesystem cache; these are NOT OS-cache-cold samples.
        std::fs::copy(&input, &path).map_err(|e| e.to_string())?;
        *wanted.lock().unwrap() = Wanted { generation, pages: vec![0], ..Default::default() };
        let start = Instant::now();
        tx.send(Request::Open { generation, path }).map_err(|e| e.to_string())?;
        let mut opened_ms = None;
        let mut first_render_ms = None;
        let mut annotation_scan_ms = None;
        let mut analysis_ready_ms = None;
        let mut full_open_ms = None;
        let mut file = None;
        while first_render_ms.is_none() || annotation_scan_ms.is_none() || analysis_ready_ms.is_none() {
            let reply = rx.recv_timeout(Duration::from_secs(60)).map_err(|e| e.to_string())?;
            if let Reply::Opened { generation: g, snapshot, .. } = &reply {
                if *g == generation { file = Some(snapshot.file()); }
            }
            if adapter::ready(&reply, generation, file)? {
                analysis_ready_ms.get_or_insert_with(|| elapsed_ms(start));
            }
            match reply {
                Reply::Opened { generation: g, page_sizes, .. } if g == generation => {
                    if page_sizes.is_empty() { return Err("fixture has no pages".into()); }
                    opened_ms = Some(elapsed_ms(start));
                    tx.send(Request::Render { generation, page: 0, scale: 1.0 }).map_err(|e| e.to_string())?;
                }
                Reply::Rendered { generation: g, page: 0, complete: true, .. } if g == generation => {
                    if first_render_ms.is_none() {
                        first_render_ms = Some(elapsed_ms(start));
                        if analysis_ready_ms.is_none() {
                            adapter::release(&tx, generation, file.ok_or("render before Opened")?)?;
                        }
                    }
                }
                Reply::Highlights { generation: g, done: true, .. } if g == generation => {
                    annotation_scan_ms.get_or_insert_with(|| elapsed_ms(start));
                }
                Reply::Fatal(error) | Reply::OpenFailed { error, .. } => return Err(error),
                Reply::RenderFailed { error, .. } => return Err(error),
                _ => {}
            }
            if first_render_ms.is_some() && annotation_scan_ms.is_some() {
                full_open_ms.get_or_insert_with(|| elapsed_ms(start));
            }
        }
        // Headless final-discovery readiness is not UI edit latency. The baseline
        // adapter uses Opened because HEAD has no discovery gate; candidate waits
        // for the actual matching result, never a fixed scheduling sleep.
        let full_open_ms = full_open_ms.unwrap();
        let save_start = Instant::now();
        tx.send(Request::Save {
            generation,
            changes: Changes {
                adds: vec![NewHighlight {
                    page: 0,
                    quads: vec![PdfBox { left: 50.0, bottom: 690.0, right: 250.0, top: 705.0 }],
                    color: [1.0, 1.0, 0.0], comment: "synthetic performance annotation".into(),
                }],
                ..Default::default()
            },
            arrangement: None, new_pages: vec![],
        }).map_err(|e| e.to_string())?;
        loop {
            match rx.recv_timeout(Duration::from_secs(60)).map_err(|e| e.to_string())? {
                Reply::Saved { generation: g, .. } if g == generation => break,
                Reply::SaveFailed { error, .. } | Reply::Fatal(error) => return Err(error),
                _ => {}
            }
        }
        let save_ms = elapsed_ms(save_start);
        println!("{}", serde_json::json!({
            "iteration": iteration,
            "native_initialization_ms": native_initialization_ms,
            "opened_ms": opened_ms.unwrap(),
            "first_render_ms": first_render_ms.unwrap(),
            "annotation_scan_ms": annotation_scan_ms.unwrap(),
            "full_open_ms": full_open_ms,
            "analysis_ready_ms": analysis_ready_ms.unwrap(),
            "fully_interactive_ms": first_render_ms.unwrap().max(analysis_ready_ms.unwrap()),
            "post_render_analysis_wait_ms": (analysis_ready_ms.unwrap() - first_render_ms.unwrap()).max(0.0),
            "save_ms": save_ms,
        }));
        ctx.tex_manager().write().take_delta().clear();
    }
    Ok(())
}
