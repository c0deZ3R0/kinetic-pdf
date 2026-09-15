//! How the worker orders page work: the most wanted page first, nothing done
//! for pages the view has left, and a slow render abandoned part-way once its
//! page is no longer wanted.
//!
//! pdfium can only be set up once in a process, so one worker runs every case
//! in turn, each on its own document under its own generation.

mod common;

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{build_pdf, dense_pdf, next_reply, scratch_dir, start_worker};
use kinetic_pdf::model::{Reply, Request};
use kinetic_pdf::worker::Wanted;

struct Worker {
    tx: Sender<Request>,
    rx: Receiver<Reply>,
    wanted: Arc<Mutex<Wanted>>,
}

impl Worker {
    fn open(&self, generation: u64, path: &Path, pdf: Vec<u8>) {
        std::fs::write(path, pdf).unwrap();
        self.tx.send(Request::Open { generation, path: path.to_path_buf() }).unwrap();
        loop {
            match next_reply(&self.rx) {
                Reply::Opened { generation: g, .. } if g == generation => break,
                Reply::OpenFailed { error, .. } => panic!("could not open the test PDF: {error}"),
                _ => {}
            }
        }
        let mut w = self.wanted.lock().unwrap();
        w.generation = generation;
        w.pages.clear();
    }
}

#[test]
fn page_work_follows_what_the_view_wants() {
    let dir = scratch_dir("scheduling");
    let (tx, rx, wanted) = start_worker();
    let worker = Worker { tx, rx, wanted };

    text_for_a_page_no_longer_wanted_is_skipped(&worker, &dir.join("text.pdf"), 1);
    queued_renders_go_most_wanted_first(&worker, &dir.join("order.pdf"), 2);
    a_slow_render_stops_once_its_page_is_not_wanted(&worker, &dir.join("dense.pdf"), 3);

    drop(worker);
    let _ = std::fs::remove_dir_all(dir);
}

fn text_for_a_page_no_longer_wanted_is_skipped(worker: &Worker, path: &Path, generation: u64) {
    worker.open(generation, path, build_pdf(3, &[]));
    worker.wanted.lock().unwrap().pages = vec![0];
    worker.tx.send(Request::Text { generation, page: 2 }).unwrap();
    worker.tx.send(Request::Text { generation, page: 0 }).unwrap();

    let (mut skipped, mut read) = (Vec::new(), Vec::new());
    while skipped.len() + read.len() < 2 {
        match next_reply(&worker.rx) {
            Reply::TextSkipped { generation: g, page } if g == generation => skipped.push(page),
            Reply::Text { generation: g, page, .. } if g == generation => read.push(page),
            _ => {}
        }
    }
    assert_eq!((skipped, read), (vec![2], vec![0]));
}

fn queued_renders_go_most_wanted_first(worker: &Worker, path: &Path, generation: u64) {
    worker.open(generation, path, build_pdf(4, &[]));

    // Hold the shared state while the requests go in, so they queue up
    // behind the first, then say which pages matter most.
    {
        let mut w = worker.wanted.lock().unwrap();
        for page in 0..4 {
            worker.tx.send(Request::Render { generation, page, scale: 0.5 }).unwrap();
        }
        std::thread::sleep(Duration::from_millis(100));
        w.pages = vec![3, 1, 2, 0];
    }

    let mut order = Vec::new();
    while order.len() < 4 {
        match next_reply(&worker.rx) {
            Reply::Rendered { generation: g, page, complete: true, .. } if g == generation => order.push(page),
            Reply::RenderSkipped { generation: g, page } if g == generation => panic!("page {page} was skipped"),
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            _ => {}
        }
    }
    // The worker may already have taken the first request before the order
    // was given; everything after it follows the order.
    assert!(order == [3, 1, 2, 0] || order == [0, 3, 1, 2], "rendered in the order {order:?}");
}

fn a_slow_render_stops_once_its_page_is_not_wanted(worker: &Worker, path: &Path, generation: u64) {
    worker.open(generation, path, dense_pdf(300_000));
    worker.wanted.lock().unwrap().pages = vec![0];

    // A full render first, to know how long one takes. This also loads the
    // page, so the second render starts drawing straight away.
    worker.tx.send(Request::Render { generation, page: 0, scale: 1.0 }).unwrap();
    let started = Instant::now();
    loop {
        match next_reply(&worker.rx) {
            Reply::Rendered { generation: g, complete: true, .. } if g == generation => break,
            Reply::RenderFailed { error, .. } => panic!("render failed: {error}"),
            _ => {}
        }
    }
    let full = started.elapsed();
    assert!(full >= Duration::from_millis(300), "the dense page drew in {full:?}, too quickly to test abandoning it");

    worker.tx.send(Request::Render { generation, page: 0, scale: 1.0 }).unwrap();
    let started = Instant::now();
    std::thread::sleep(Duration::from_millis(60));
    worker.wanted.lock().unwrap().pages.clear();
    loop {
        match next_reply(&worker.rx) {
            Reply::RenderSkipped { generation: g, .. } if g == generation => break,
            Reply::Rendered { generation: g, complete: true, .. } if g == generation => {
                panic!("the render finished instead of stopping")
            }
            _ => {}
        }
    }
    let stopped = started.elapsed();
    assert!(stopped < full / 2, "stopped after {stopped:?}; a full render takes {full:?}");
}
