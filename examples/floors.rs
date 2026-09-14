//! Works out how fast each part of the app could theoretically be.
//!
//! The benchmark (src/bin/bench.rs) times what the app does today. This takes
//! each of those jobs apart -- what pdfium has to do, what the hardware limits
//! are, and what is overhead of ours -- and measures the pieces, so each
//! part's floor can be stated from measurements rather than guessed.
//!
//!     cargo run --release --example floors -- file.pdf [more.pdf ...]
//!
//! Internal modes, run as child processes (pdfium allows one binding per
//! process, and parallel rendering needs separate processes anyway):
//!     floors render-all FILE SCALE     render every page once, print "ms pages"
//!     floors frames FILE               run the real app headless, print frame times

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, TextureOptions};
use eframe::App as _;
use pdfium_render::prelude::*;

use pdf_annotate::model::TextChar;
use pdf_annotate::{annots, selection, worker};

/// The app window measured on the laptop this was written on: 2122 x 1406
/// device pixels, of which the page view is roughly this much.
const VIEW_W: i32 = 2000;
const VIEW_H: i32 = 1240;
/// Horizontal margins and scroll bar around a fitted page, in device pixels at 2x.
const FIT_MARGINS: f32 = 92.0;

const MB: f64 = 1024.0 * 1024.0;

macro_rules! out {
    ($report:expr) => {{
        println!();
        $report.push('\n');
    }};
    ($report:expr, $($arg:tt)*) => {{
        let line = format!($($arg)*);
        println!("{line}");
        $report.push_str(&line);
        $report.push('\n');
    }};
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed())
}

fn ms(d: Duration) -> String {
    let v = d.as_secs_f64() * 1000.0;
    if v >= 100.0 {
        format!("{v:.0} ms")
    } else if v >= 10.0 {
        format!("{v:.1} ms")
    } else if v >= 0.1 {
        format!("{v:.2} ms")
    } else {
        format!("{:.0} µs", v * 1000.0)
    }
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v.get(v.len() / 2).copied().unwrap_or_default()
}

fn percentile(mut v: Vec<Duration>, p: f64) -> Duration {
    v.sort();
    if v.is_empty() {
        return Duration::ZERO;
    }
    v[((v.len() as f64 * p).ceil() as usize).saturating_sub(1).min(v.len() - 1)]
}

fn err(e: PdfiumError) -> String {
    e.to_string()
}

fn spread(n: usize, k: usize) -> Vec<usize> {
    if n <= k {
        (0..n).collect()
    } else {
        (0..k).map(|i| i * n / k).collect()
    }
}

fn usual_size(sizes: &[[f32; 2]]) -> [f32; 2] {
    let mut counts: HashMap<(i32, i32), (usize, usize)> = HashMap::new();
    for (i, [w, h]) in sizes.iter().enumerate() {
        counts.entry((w.round() as i32, h.round() as i32)).or_insert((0, i)).0 += 1;
    }
    counts.values().max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1))).map_or([612.0, 792.0], |&(_, i)| sizes[i])
}

fn build_texture(ctx: &egui::Context, size: [usize; 2], rgba: &[u8]) {
    let image = ColorImage::from_rgba_premultiplied(size, rgba);
    drop(ctx.load_texture("floors", image, TextureOptions::LINEAR));
    ctx.tex_manager().write().take_delta().clear();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        // The headless app starts this exe as its render helpers.
        Some(flag) if flag == pdf_annotate::helper::FLAG => {
            pdf_annotate::helper::run();
            Ok(())
        }
        Some("render-all") if args.len() == 3 => render_all(Path::new(&args[1]), args[2].parse().unwrap_or(1.0)),
        Some("frames") if args.len() == 2 => frames(Path::new(&args[1])),
        Some("deep-zoom") if args.len() == 2 || args.len() == 3 => {
            deep_zoom(Path::new(&args[1]), args.get(2).and_then(|page| page.parse().ok()))
        }
        Some("zoom-in") if args.len() == 3 => zoom_in(Path::new(&args[1]), args[2].parse().unwrap_or(2.0)),
        Some("zoom-cycle") if args.len() == 2 => zoom_cycle(Path::new(&args[1])),
        Some("rest-zoom") if args.len() == 3 => rest_zoom(Path::new(&args[1]), args[2].parse().unwrap_or(0)),
        Some("tile-compression") if args.len() == 3 => tile_compression(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("cache-formats") if args.len() == 3 => cache_formats(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("page-costs") if args.len() == 2 => page_costs(Path::new(&args[1])),
        Some("fling") if args.len() == 2 => fling(Path::new(&args[1])),
        Some("memory") if args.len() == 2 => memory(Path::new(&args[1])),
        Some("split") if args.len() == 2 => split(Path::new(&args[1])),
        Some("annotations") if args.len() == 2 => annotation_costs(Path::new(&args[1])),
        Some("stamps") if args.len() == 2 => stamp_costs(Path::new(&args[1])),
        Some("stamp-contents") if args.len() == 2 => stamp_contents(Path::new(&args[1])),
        Some("stamp-paths") if args.len() == 3 => stamp_paths(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("stamp-runs") if args.len() == 3 => stamp_runs(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("stamp-breakdown") if args.len() == 3 => stamp_breakdown(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("stamp-ops") if args.len() == 3 => stamp_ops(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("merge-render") if args.len() == 3 => merge_render(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        // The merged copy merge.rs makes, written out to look at with the other modes.
        Some("merge-save") if args.len() == 3 => std::fs::read(&args[1])
            .map_err(|e| e.to_string())
            .and_then(|bytes| pdf_annotate::merge::merge_document(&bytes, 256))
            .and_then(|merged| match merged {
                Some((out, stats)) => {
                    println!("{stats:?}");
                    std::fs::write(&args[2], out).map_err(|e| e.to_string())
                }
                None => Err("nothing to merge".to_owned()),
            }),
        Some("gstates") if args.len() == 3 => gstates(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("stamp-view") if args.len() == 4 => stamp_view(Path::new(&args[1]), args[2].parse().unwrap_or(1), Path::new(&args[3])),
        Some("annot-dicts") if args.len() == 3 => annot_dicts(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("object") if args.len() == 3 => print_object(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("copy-check") if args.len() == 3 || args.len() == 4 => {
            copy_check(Path::new(&args[1]), Path::new(&args[2]), args.get(3).map(Path::new))
        }
        Some("merge-test") if args.len() == 3 || args.len() == 4 => {
            merge_test(Path::new(&args[1]), args[2].parse().unwrap_or(1), args.get(3).map(Path::new))
        }
        Some("render-options") if args.len() == 3 => render_options(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("regions") if args.len() == 3 => regions(Path::new(&args[1]), args[2].parse().unwrap_or(1)),
        Some("scales") if args.len() == 3 || args.len() == 4 => {
            scales(Path::new(&args[1]), args[2].parse().unwrap_or(1), args.get(3).map(Path::new))
        }
        Some("search-worker") if args.len() == 3 => search_worker(Path::new(&args[1]), &args[2]),
        Some("pages-worker") if args.len() == 3 => pages_worker(Path::new(&args[1]), args[2].parse().unwrap_or(1.0)),
        Some(_) => analyse(&args.iter().map(PathBuf::from).collect::<Vec<_>>()),
        None => Err("usage: floors file.pdf [more.pdf ...]".to_owned()),
    };
    if let Err(e) = result {
        eprintln!("floors failed: {e}");
        std::process::exit(1);
    }
}

/* ------------------------------------------------------------------ *
 * Child: render every page once
 * ------------------------------------------------------------------ */

fn render_all(path: &Path, scale: f32) -> Result<(), String> {
    let pdfium = worker::bind()?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let doc = pdfium.load_pdf_from_byte_vec(bytes, None).map_err(err)?;
    let n = doc.pages().len() as usize;
    let config = PdfRenderConfig::new().scale_page_by_factor(scale).render_annotations(true).render_form_data(true);
    let start = Instant::now();
    for p in 0..n {
        let page = doc.pages().get(p as PdfPageIndex).map_err(err)?;
        drop(page.render_with_config(&config).map_err(err)?);
    }
    println!("{} {n}", start.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}

/* ------------------------------------------------------------------ *
 * Child: the real app, headless
 * ------------------------------------------------------------------ */

/// Each page as scrolling brings it into view, through the app's worker: its
/// text and its pixels requested together, one page after another, timed from
/// the requests to both replies. Waits for the background read of highlights
/// to finish first, so it doesn't share the timing. Prints
/// "median_ms total_ms pages".
fn pages_worker(path: &Path, scale: f32) -> Result<(), String> {
    use pdf_annotate::model::{Reply, Request};

    let ctx = egui::Context::default();
    let wanted = std::sync::Arc::new(std::sync::Mutex::new(worker::Wanted::default()));
    let (tx, rx) = worker::spawn(ctx.clone(), wanted.clone(), pdf_annotate::pool::Helpers::from_current_exe(), None);
    let wait = Duration::from_secs(120);
    tx.send(Request::Open { generation: 1, path: path.to_path_buf() }).map_err(|e| e.to_string())?;
    let mut pages = 0;
    loop {
        match rx.recv_timeout(wait).map_err(|e| e.to_string())? {
            Reply::Opened { page_sizes, .. } => pages = page_sizes.len(),
            Reply::Highlights { done: true, .. } => break,
            Reply::OpenFailed { error, .. } | Reply::Fatal(error) => return Err(error),
            _ => {}
        }
    }

    let mut times = Vec::new();
    for page in 0..pages {
        if let Ok(mut w) = wanted.lock() {
            w.generation = 1;
            w.pages.clear();
            w.pages.push(page);
        }
        let start = Instant::now();
        tx.send(Request::Text { generation: 1, page }).map_err(|e| e.to_string())?;
        tx.send(Request::Render { generation: 1, page, scale }).map_err(|e| e.to_string())?;
        let (mut text, mut pixels) = (false, false);
        while !(text && pixels) {
            match rx.recv_timeout(wait).map_err(|e| e.to_string())? {
                Reply::Text { page: p, .. } if p == page => text = true,
                Reply::Rendered { page: p, complete: true, .. } if p == page => pixels = true,
                Reply::RenderFailed { error, .. } => return Err(error),
                _ => {}
            }
        }
        times.push(start.elapsed());
        // With no window the textures are never uploaded; let them go.
        ctx.tex_manager().write().take_delta().clear();
    }
    let total: Duration = times.iter().sum();
    println!("{} {} {pages}", median(times).as_secs_f64() * 1000.0, total.as_secs_f64() * 1000.0);
    Ok(())
}

/// The app's own search, through its worker thread: a first search of the
/// document, then the same search again. Prints "first_ms repeat_ms matches".
fn search_worker(path: &Path, query: &str) -> Result<(), String> {
    use pdf_annotate::model::{Reply, Request};

    let wanted = std::sync::Arc::new(std::sync::Mutex::new(worker::Wanted::default()));
    let (tx, rx) = worker::spawn(egui::Context::default(), wanted, pdf_annotate::pool::Helpers::none(), None);
    let wait = Duration::from_secs(120);
    tx.send(Request::Open { generation: 1, path: path.to_path_buf() }).map_err(|e| e.to_string())?;
    // Let the background read of highlights finish, so it doesn't share the timing.
    loop {
        match rx.recv_timeout(wait).map_err(|e| e.to_string())? {
            Reply::Highlights { done: true, .. } => break,
            Reply::OpenFailed { error, .. } | Reply::Fatal(error) => return Err(error),
            _ => {}
        }
    }

    let run = |id: u64| -> Result<(Duration, usize), String> {
        let start = Instant::now();
        tx.send(Request::Search { generation: 1, id, query: query.to_owned() }).map_err(|e| e.to_string())?;
        let mut matches = 0;
        loop {
            if let Reply::Search { id: got, hits, done, .. } = rx.recv_timeout(wait).map_err(|e| e.to_string())? {
                if got == id {
                    matches += hits.len();
                    if done {
                        return Ok((start.elapsed(), matches));
                    }
                }
            }
        }
    };
    let (first, matches) = run(1)?;
    let (repeat, _) = run(2)?;
    println!("{} {} {matches}", first.as_secs_f64() * 1000.0, repeat.as_secs_f64() * 1000.0);
    Ok(())
}

fn frames(path: &Path) -> Result<(), String> {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = pdf_annotate::app::App::new(&cc, Some(path.to_path_buf()));
    let mut frame = eframe::Frame::_new_kittest();

    let ppp = 2.0;
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VIEW_W as f32 / ppp, (VIEW_H as f32 + 130.0) / ppp));
    let started = Instant::now();

    struct FrameTimes {
        run: Duration,
        tessellate: Duration,
        uploaded_bytes: usize,
    }

    let mut step = |events: Vec<egui::Event>| -> FrameTimes {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(started.elapsed().as_secs_f64()),
            max_texture_side: Some(16384),
            events,
            ..Default::default()
        };
        if let Some(viewport) = input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(ppp);
        }
        let (mut output, run) = timed(|| ctx.run_ui(input, |ui| app.ui(ui, &mut frame)));
        let shapes = std::mem::take(&mut output.shapes);
        let (_, tessellate) = timed(|| ctx.tessellate(shapes, output.pixels_per_point));
        let uploaded_bytes = output
            .textures_delta
            .set
            .values()
            .flat_map(|deltas| deltas.iter())
            .map(|d| {
                let [w, h] = d.image.size();
                w * h * 4
            })
            .sum();
        output.textures_delta.clear();
        FrameTimes { run, tessellate, uploaded_bytes }
    };

    // Let the document open and the first pages arrive.
    while started.elapsed() < Duration::from_secs(4) {
        step(Vec::new());
        std::thread::sleep(Duration::from_millis(8));
    }

    let pointer = egui::Event::PointerMoved(screen.center());
    let mut idle = Vec::new();
    for _ in 0..120 {
        idle.push(step(vec![pointer.clone()]));
    }

    let mut scrolling = Vec::new();
    for _ in 0..240 {
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -45.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        };
        scrolling.push(step(vec![pointer.clone(), wheel]));
        // 60 frames a second, as a real display would pace it.
        std::thread::sleep(Duration::from_millis(16));
    }

    let summarise = |label: &str, frames: &[FrameTimes]| {
        let run: Vec<Duration> = frames.iter().map(|f| f.run).collect();
        let tess: Vec<Duration> = frames.iter().map(|f| f.tessellate).collect();
        let total: Vec<Duration> = frames.iter().map(|f| f.run + f.tessellate).collect();
        let max_upload = frames.iter().map(|f| f.uploaded_bytes).max().unwrap_or(0);
        let uploads = frames.iter().filter(|f| f.uploaded_bytes > 0).count();
        println!(
            "| {label} | {} | {} | {} | {} | {} | {uploads} frames, largest {:.0} MB |",
            ms(median(run.clone())),
            ms(percentile(run, 0.95)),
            ms(median(tess)),
            ms(median(total.clone())),
            ms(total.iter().copied().max().unwrap_or_default()),
            max_upload as f64 / MB,
        );
    };
    summarise("Idle (pointer over the page)", &idle);
    summarise("Scrolling at 60 fps through the document", &scrolling);
    let _ = std::io::stdout().flush();
    Ok(())
}

/// What each page's annotations cost: how many of each kind, how long the page
/// takes to draw with and without them, how long the app's reading of its
/// highlights and removing them for display take, and the page drawn as the
/// app shows it. Printed only.
fn annotation_costs(path: &Path) -> Result<(), String> {
    let pdfium = worker::bind()?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    let sizes = annots::page_sizes(&doc);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    println!("{} pages; fit width renders the usual page at {fit:.2} px/pt", sizes.len());
    println!("page | objects | annots | load | draw all | no annots | read highlights | remove highlights | draw as shown | kinds");
    for (i, &[w, h]) in sizes.iter().enumerate() {
        let (page, load) = timed(|| doc.pages().get(i as PdfPageIndex));
        let mut page = page.map_err(err)?;
        let objects = page.objects().len();

        let mut kinds: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        let annotations = page.annotations();
        let count = annotations.len();
        for index in 0..count {
            if let Ok(annotation) = annotations.get(index) {
                *kinds.entry(format!("{:?}", annotation.annotation_type())).or_default() += 1;
            }
        }
        let mut kinds: Vec<(String, usize)> = kinds.into_iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(&a.1));
        let kinds: Vec<String> = kinds.iter().map(|(kind, n)| format!("{n} {kind}")).collect();

        let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit, 1.0, 16384.0);
        let (_, draw_all) = timed(|| annots::render_loaded_page(&page, scale));
        let bare = PdfRenderConfig::new().scale_page_by_factor(scale).render_annotations(false).render_form_data(false);
        let (_, no_annots) = timed(|| page.render_with_config(&bare).map(drop));
        let (highlights, read) = timed(|| annots::read_loaded_page(&page, i).0.len());
        let (_, remove) = timed(|| annots::strip_loaded_page(&mut page));
        let (_, shown) = timed(|| annots::render_loaded_page(&page, scale));

        println!(
            "{:>4} | {objects:>7} | {count:>6} | {} | {} | {} | {} ({highlights} highlights) | {} | {} | {}",
            i + 1,
            ms(load),
            ms(draw_all),
            ms(no_annots),
            ms(read),
            ms(remove),
            ms(shown),
            kinds.join(", ")
        );
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// Where a page's annotations spend their drawing time. The whole page drawn
/// three times in a row (does anything carry over between draws?), at an eighth
/// of fit width (does the cost follow the pixels?) and without annotations;
/// then each annotation on its own: its kind, the objects in its appearance, how
/// much of the page it covers, and a fresh copy of the page drawn with only it.
/// Printed only.
fn stamp_costs(path: &Path) -> Result<(), String> {
    let pdfium = worker::bind()?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    let sizes = annots::page_sizes(&doc);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    for (i, &[w, h]) in sizes.iter().enumerate() {
        let page = doc.pages().get(i as PdfPageIndex).map_err(err)?;
        let count = page.annotations().len() as usize;
        if count == 0 {
            continue;
        }
        let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit, 1.0, 16384.0);
        let draws: Vec<String> = (0..3).map(|_| ms(timed(|| annots::render_loaded_page(&page, scale)).1)).collect();
        let (_, eighth) = timed(|| annots::render_loaded_page(&page, scale / 8.0));
        let bare = PdfRenderConfig::new().scale_page_by_factor(scale).render_annotations(false).render_form_data(false);
        let (_, without) = timed(|| page.render_with_config(&bare).map(drop));
        println!("page {}: {count} annotations, drawn at {scale:.2} px/pt", i + 1);
        println!("  whole page, three times in a row: {}", draws.join(", "));
        println!("  at an eighth of the size: {}; without annotations: {}", ms(eighth), ms(without));

        let mut alone_total = Duration::ZERO;
        for k in 0..count {
            let annotations = page.annotations();
            let Ok(annotation) = annotations.get(k) else { continue };
            let kind = format!("{:?}", annotation.annotation_type());
            let objects = annotation.objects().len();
            let covers = annotation.bounds().map_or(0.0, |b| b.width().value * b.height().value / (w * h) * 100.0);

            // A fresh copy of the page with every other annotation deleted.
            let copy = pdfium.load_pdf_from_file(path, None).map_err(err)?;
            let mut only = copy.pages().get(i as PdfPageIndex).map_err(err)?;
            {
                let others = only.annotations_mut();
                for j in (0..count).rev().filter(|&j| j != k) {
                    if let Ok(other) = others.get(j) {
                        let _ = others.delete_annotation(other);
                    }
                }
            }
            let (_, alone) = timed(|| annots::render_loaded_page(&only, scale));
            alone_total += alone;
            println!("  {:>3}. {kind}: {objects} objects in its appearance, covers {covers:.0}% of the page, page drawn with only it: {}", k + 1, ms(alone));
            let _ = std::io::stdout().flush();
        }
        println!("  each alone, added up: {}", ms(alone_total));
    }
    Ok(())
}

/// When zoomed in on a slow page, does drawing only the area in view save time?
/// At 1, 2, 4 and 8 times fit width: the whole page drawn as the app would
/// (capped by its pixel limit), then just a view-sized area from the middle of
/// the page at full sharpness. Printed only.
fn regions(path: &Path, page_number: usize) -> Result<(), String> {
    let pdfium = worker::bind()?;
    let index = page_number.saturating_sub(1);
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    let sizes = annots::page_sizes(&doc);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    let [w, h] = sizes[index];
    let page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
    println!("page {page_number}, view {VIEW_W} x {VIEW_H} px");
    for factor in [1.0_f32, 2.0, 4.0, 8.0] {
        let zoom = fit * factor;
        let capped = pdf_annotate::app::render_scale(egui::vec2(w, h), zoom, 1.0, 16384.0);
        let (_, whole) = timed(|| annots::render_loaded_page(&page, capped));
        let full = [(w * zoom).round() as u32, (h * zoom).round() as u32];
        let (vw, vh) = ((VIEW_W as u32).min(full[0]), (VIEW_H as u32).min(full[1]));
        let region = [(full[0] - vw) / 2, (full[1] - vh) / 2, vw, vh];
        let (_, area) = timed(|| annots::render_region_in_steps(&page, full, region, |_| true));
        println!(
            "  {factor}x fit: whole page {:.1} MP in {}; view-sized area {:.1} MP in {}",
            (w * capped) * (h * capped) / 1e6,
            ms(whole),
            (vw * vh) as f64 / 1e6,
            ms(area)
        );
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// Is a slow page's cost in its content or in its pixels? The page (numbered
/// from 1) drawn at half, one, two and three times fit width, each size capped
/// by the app's pixel limit as the app would, with its megapixels. With `dump`,
/// the fit-width image's raw RGBA is written there, to try compressing it for a
/// cache. Printed only.
fn scales(path: &Path, page_number: usize, dump: Option<&Path>) -> Result<(), String> {
    let pdfium = worker::bind()?;
    let index = page_number.saturating_sub(1);
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    let sizes = annots::page_sizes(&doc);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    let [w, h] = sizes[index];
    let page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
    println!("page {page_number}");
    for factor in [0.5_f32, 1.0, 2.0, 3.0] {
        let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit * factor, 1.0, 16384.0);
        let (rendered, took) = timed(|| annots::render_loaded_page(&page, scale));
        let (size, rgba) = rendered?;
        println!("  {factor}x fit width: {scale:.2} px/pt, {:.1} MP, {}", (size[0] * size[1]) as f64 / 1e6, ms(took));
        if factor == 1.0 {
            if let Some(dump) = dump {
                std::fs::write(dump, &rgba).map_err(|e| e.to_string())?;
                println!("  wrote the {} x {} image, {:.1} MB raw", size[0], size[1], rgba.len() as f64 / MB);
            }
        }
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// `n` draws of `page` with `config`, as a list of times.
fn draw_times(page: &PdfPage, config: &PdfRenderConfig, n: usize) -> String {
    (0..n).map(|_| ms(timed(|| page.render_with_config(config).map(drop)).1)).collect::<Vec<_>>().join(", ")
}

/// What pdfium itself can do for a slow page (numbered from 1), at fit width:
/// drawing without anti-aliasing, and merging its annotations into the page
/// content once so they're parsed when the page loads rather than on every
/// draw. Printed only.
fn render_options(path: &Path, page_number: usize) -> Result<(), String> {
    let pdfium = worker::bind()?;
    let index = page_number.saturating_sub(1);
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    let sizes = annots::page_sizes(&doc);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    let [w, h] = sizes[index];
    let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit, 1.0, 16384.0);
    let normal = || PdfRenderConfig::new().scale_page_by_factor(scale).render_annotations(true).render_form_data(true);

    let page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
    println!("page {page_number} at {scale:.2} px/pt, {} objects, {} annotations", page.objects().len(), page.annotations().len());
    println!("  as the app draws it:        {}", draw_times(&page, &normal(), 2));
    println!("  paths not anti-aliased:     {}", draw_times(&page, &normal().set_path_smoothing(false), 2));
    println!(
        "  nothing anti-aliased:       {}",
        draw_times(&page, &normal().set_path_smoothing(false).set_image_smoothing(false).set_text_smoothing(false), 2)
    );
    drop(page);

    // Flattened: the annotations become part of the page content, in a copy.
    let copy = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    {
        let mut page = copy.pages().get(index as PdfPageIndex).map_err(err)?;
        let (flattened, took) = timed(|| page.flatten());
        println!("  flattening took {}{}", ms(took), flattened.err().map(|e| format!(" and failed: {e}")).unwrap_or_default());
    }
    let (page, load) = timed(|| copy.pages().get(index as PdfPageIndex));
    let page = page.map_err(err)?;
    println!(
        "  flattened page: loads in {}, {} objects, {} annotations",
        ms(load),
        page.objects().len(),
        page.annotations().len()
    );
    println!("  flattened, drawn 3 times:   {}", draw_times(&page, &normal(), 3));
    println!("  flattened, paths not anti-aliased: {}", draw_times(&page, &normal().set_path_smoothing(false), 2));
    Ok(())
}

/// Everything inside one annotation's appearance, nested forms included.
#[derive(Default)]
struct Contents {
    by_type: std::collections::BTreeMap<String, usize>,
    transparent: usize,
    deepest: usize,
    images: Vec<String>,
}

fn tally(object: &PdfPageObject, depth: usize, contents: &mut Contents) {
    *contents.by_type.entry(format!("{:?}", object.object_type())).or_default() += 1;
    contents.deepest = contents.deepest.max(depth);
    if object.has_transparency() {
        contents.transparent += 1;
    }
    if let Some(image) = object.as_image_object() {
        let filters: Vec<String> = image.filters().iter().map(|f| f.name().to_owned()).collect();
        contents.images.push(format!(
            "{} x {} px {}",
            image.width().unwrap_or(0),
            image.height().unwrap_or(0),
            if filters.is_empty() { "uncompressed".to_owned() } else { filters.join("+") }
        ));
    }
    if let Some(form) = object.as_x_object_form_object() {
        for index in 0..form.len() {
            if let Ok(child) = form.get(index) {
                tally(&child, depth + 1, contents);
            }
        }
    }
}

/// The paths inside a page's annotations (numbered from 1), through every nested
/// form: how many are stroked, filled or both, dashed, curved, transparent, and
/// how large they and their strokes come out at fit width. Sizes are in each
/// form's own space, so only roughly on the page. Printed only.
fn stamp_paths(path: &Path, page_number: usize) -> Result<(), String> {
    #[derive(Default)]
    struct Paths {
        total: usize,
        stroked: usize,
        filled: usize,
        both: usize,
        dashed: usize,
        curved: usize,
        transparent: usize,
        segments: usize,
        most_segments: usize,
        /// By size on screen: under a pixel, 1-4 px, 4-32 px, larger.
        by_size: [usize; 4],
        /// Stroked paths by line width on screen: hairline (0), under a pixel,
        /// 1-3 px, wider.
        by_width: [usize; 4],
    }

    fn walk(object: &PdfPageObject, scale: f32, paths: &mut Paths) {
        if let Some(form) = object.as_x_object_form_object() {
            for index in 0..form.len() {
                if let Ok(child) = form.get(index) {
                    walk(&child, scale, paths);
                }
            }
            return;
        }
        let Some(p) = object.as_path_object() else { return };
        paths.total += 1;
        let stroked = p.is_stroked().unwrap_or(false);
        let filled = !matches!(p.fill_mode(), Ok(PdfPathFillMode::None) | Err(_));
        match (stroked, filled) {
            (true, true) => paths.both += 1,
            (true, false) => paths.stroked += 1,
            (false, true) => paths.filled += 1,
            _ => {}
        }
        if p.dash_array().is_ok_and(|d| !d.is_empty()) {
            paths.dashed += 1;
        }
        if object.has_transparency() {
            paths.transparent += 1;
        }
        let segments = p.segments();
        let n = segments.len() as usize;
        paths.segments += n;
        paths.most_segments = paths.most_segments.max(n);
        if segments.iter().any(|s| s.segment_type() == PdfPathSegmentType::BezierTo) {
            paths.curved += 1;
        }
        if let Ok(b) = object.bounds() {
            let px = b.width().value.max(b.height().value) * scale;
            paths.by_size[if px < 1.0 { 0 } else if px < 4.0 { 1 } else if px < 32.0 { 2 } else { 3 }] += 1;
        }
        if stroked {
            let px = p.stroke_width().map_or(0.0, |w| w.value) * scale;
            paths.by_width[if px == 0.0 { 0 } else if px < 1.0 { 1 } else if px < 3.0 { 2 } else { 3 }] += 1;
        }
    }

    let pdfium = worker::bind()?;
    let index = page_number.saturating_sub(1);
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    let sizes = annots::page_sizes(&doc);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    let [w, h] = sizes[index];
    let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit, 1.0, 16384.0);
    let page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
    let annotations = page.annotations();
    println!("page {page_number} at {scale:.2} px/pt (fit width)");
    let mut all = Paths::default();
    for k in 0..annotations.len() {
        let Ok(annotation) = annotations.get(k) else { continue };
        let mut paths = Paths::default();
        let objects = annotation.objects();
        for i in 0..objects.len() {
            if let Ok(object) = objects.get(i) {
                walk(&object, scale, &mut paths);
            }
        }
        let pct = |n: usize| n as f64 * 100.0 / paths.total.max(1) as f64;
        println!(
            "  annotation {}: {} paths, {:.1} segments each (most {}); stroked only {:.0}%, filled only {:.0}%, both {:.0}%; dashed {:.0}%, curved {:.0}%, transparent {:.0}%",
            k + 1,
            paths.total,
            paths.segments as f64 / paths.total.max(1) as f64,
            paths.most_segments,
            pct(paths.stroked),
            pct(paths.filled),
            pct(paths.both),
            pct(paths.dashed),
            pct(paths.curved),
            pct(paths.transparent)
        );
        println!(
            "    size on screen: under 1 px {:.0}%, 1-4 px {:.0}%, 4-32 px {:.0}%, larger {:.0}%; stroke width: hairline {:.0}%, under 1 px {:.0}%, 1-3 px {:.0}%, wider {:.0}%",
            pct(paths.by_size[0]),
            pct(paths.by_size[1]),
            pct(paths.by_size[2]),
            pct(paths.by_size[3]),
            pct(paths.by_width[0]),
            pct(paths.by_width[1]),
            pct(paths.by_width[2]),
            pct(paths.by_width[3])
        );
        let _ = std::io::stdout().flush();
        all.total += paths.total;
        all.segments += paths.segments;
        for s in 0..4 {
            all.by_size[s] += paths.by_size[s];
        }
    }
    println!(
        "  all: {} paths, {} segments, {:.0}% under a pixel on screen",
        all.total,
        all.segments,
        all.by_size[0] as f64 * 100.0 / all.total.max(1) as f64
    );
    Ok(())
}

/// How far a page's annotations (numbered from 1) could be merged: paths next to
/// each other in the same form that are stroked only, undashed, opaque and drawn
/// with the same colour, width, cap and join could be one path with many parts,
/// drawn by pdfium as one object. Counts those runs through every nested form.
/// Clipping isn't visible through pdfium-render, so this is an upper bound.
/// Printed only.
fn stamp_runs(path: &Path, page_number: usize) -> Result<(), String> {
    #[derive(Default)]
    struct Runs {
        paths: usize,
        other: usize,
        /// Objects left if each run became one path.
        after: usize,
        longest: usize,
        /// Objects left if runs also had to share a clip path.
        after_with_clips: usize,
        clips: std::collections::HashSet<Vec<i64>>,
        rectangular_clips: usize,
    }

    type Style = (Option<(u8, u8, u8, u8)>, u32, String, String);

    fn style_of(object: &PdfPageObject) -> Option<Style> {
        let p = object.as_path_object()?;
        let stroked_only = p.is_stroked().unwrap_or(false) && matches!(p.fill_mode(), Ok(PdfPathFillMode::None));
        if !stroked_only || object.has_transparency() || p.dash_array().is_ok_and(|d| !d.is_empty()) {
            return None;
        }
        Some((
            p.stroke_color().ok().map(|c| (c.red(), c.green(), c.blue(), c.alpha())),
            p.stroke_width().map_or(0, |w| w.value.to_bits()),
            format!("{:?}", p.line_cap()),
            format!("{:?}", p.line_join()),
        ))
    }

    /// An object's clip path as its points to a hundredth of a point, path by
    /// path, and whether it's one axis-aligned rectangle.
    fn clip_of(object: &PdfPageObject) -> (Vec<i64>, bool) {
        let Some(clip) = object.get_clip_path() else { return (Vec::new(), false) };
        let mut key = Vec::new();
        let mut points = Vec::new();
        for path in clip.iter() {
            key.push(-1);
            for segment in path.iter() {
                let (x, y) = segment.point();
                key.push((x.value * 100.0).round() as i64);
                key.push((y.value * 100.0).round() as i64);
                points.push((x.value, y.value));
            }
        }
        let rectangle = clip.len() == 1
            && (4..=5).contains(&points.len())
            && points.windows(2).all(|w| (w[0].0 - w[1].0).abs() < 0.01 || (w[0].1 - w[1].1).abs() < 0.01);
        (key, rectangle)
    }

    fn walk_form(children: Vec<PdfPageObject>, runs: &mut Runs) {
        let mut current: Option<(Style, usize)> = None;
        let mut current_with_clip: Option<(Style, Vec<i64>)> = None;
        for child in children {
            if let Some(form) = child.as_x_object_form_object() {
                current = None;
                current_with_clip = None;
                runs.after += 1;
                runs.after_with_clips += 1;
                runs.other += 1;
                walk_form((0..form.len()).filter_map(|i| form.get(i).ok()).collect(), runs);
                continue;
            }
            let is_path = child.as_path_object().is_some();
            match style_of(&child) {
                Some(style) => {
                    runs.paths += 1;
                    let (clip, rectangle) = clip_of(&child);
                    if !clip.is_empty() && runs.clips.insert(clip.clone()) && rectangle {
                        runs.rectangular_clips += 1;
                    }
                    match &current_with_clip {
                        Some((s, c)) if *s == style && *c == clip => {}
                        _ => {
                            current_with_clip = Some((style.clone(), clip));
                            runs.after_with_clips += 1;
                        }
                    }
                    match &mut current {
                        Some((s, n)) if *s == style => {
                            *n += 1;
                            runs.longest = runs.longest.max(*n);
                        }
                        _ => {
                            current = Some((style, 1));
                            runs.after += 1;
                        }
                    }
                }
                None => {
                    current = None;
                    current_with_clip = None;
                    runs.after += 1;
                    runs.after_with_clips += 1;
                    if is_path {
                        runs.paths += 1;
                    } else {
                        runs.other += 1;
                    }
                }
            }
        }
    }

    let pdfium = worker::bind()?;
    let index = page_number.saturating_sub(1);
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    let page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
    let annotations = page.annotations();
    println!("page {page_number}");
    let (mut before, mut after, mut after_with_clips) = (0, 0, 0);
    for k in 0..annotations.len() {
        let Ok(annotation) = annotations.get(k) else { continue };
        let objects = annotation.objects();
        let mut runs = Runs::default();
        walk_form((0..objects.len()).filter_map(|i| objects.get(i).ok()).collect(), &mut runs);
        println!(
            "  annotation {}: {} paths and {} other objects would become {} objects ({:.0}x fewer); longest run {}",
            k + 1,
            runs.paths,
            runs.other,
            runs.after,
            (runs.paths + runs.other) as f64 / runs.after.max(1) as f64,
            runs.longest
        );
        println!(
            "    keeping clip paths: {} objects ({:.0}x fewer); {} distinct clips, {} of them rectangles",
            runs.after_with_clips,
            (runs.paths + runs.other) as f64 / runs.after_with_clips.max(1) as f64,
            runs.clips.len(),
            runs.rectangular_clips
        );
        let _ = std::io::stdout().flush();
        before += runs.paths + runs.other;
        after += runs.after;
        after_with_clips += runs.after_with_clips;
    }
    println!("  all: {before} objects would become {after} ({:.0}x fewer)", before as f64 / after.max(1) as f64);
    println!(
        "  keeping clip paths: {after_with_clips} ({:.0}x fewer)",
        before as f64 / after_with_clips.max(1) as f64
    );
    Ok(())
}

/// Would merging paths help pdfium? A page's annotations (numbered from 1) are
/// flattened into a copy, and every path in them rebuilt on a blank page twice:
/// once as it is, one object per path, and once with each run of paths that
/// share a stroke style merged into one object (see `stamp-runs`). Both are
/// drawn at fit width, timed and compared pixel by pixel. Images, text and
/// transparency are left out of both, so the two differ only in merging.
/// With `dump`, the flattened page and both rebuilds are written there as PNGs.
/// Printed only.
fn merge_test(path: &Path, page_number: usize, dump: Option<&Path>) -> Result<(), String> {
    fn save_png(path: &Path, size: [usize; 2], rgba: &[u8]) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size[0] as u32, size[1] as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().and_then(|mut w| w.write_image_data(rgba)).map_err(|e| e.to_string())
    }

    struct Leaf {
        matrix: PdfMatrix,
        stroke: Option<(PdfColor, f32)>,
        fill: Option<(PdfColor, PdfPathFillMode)>,
        cap: PdfPageObjectLineCap,
        join: PdfPageObjectLineJoin,
        mergeable: bool,
        /// Kind, x, y, and whether it closes the subpath.
        segments: Vec<(PdfPathSegmentType, f32, f32, bool)>,
    }

    fn collect(object: &PdfPageObject, parent: PdfMatrix, leaves: &mut Vec<Leaf>, skipped: &mut usize) {
        let matrix = object.matrix().unwrap_or(PdfMatrix::IDENTITY).multiply(parent);
        if let Some(form) = object.as_x_object_form_object() {
            for i in 0..form.len() {
                if let Ok(child) = form.get(i) {
                    collect(&child, matrix, leaves, skipped);
                }
            }
            return;
        }
        let Some(p) = object.as_path_object() else {
            *skipped += 1;
            return;
        };
        let stroked = p.is_stroked().unwrap_or(false);
        let fill_mode = p.fill_mode().unwrap_or(PdfPathFillMode::None);
        let filled = !matches!(fill_mode, PdfPathFillMode::None);
        let dashed = p.dash_array().is_ok_and(|d| !d.is_empty());
        leaves.push(Leaf {
            matrix,
            stroke: stroked.then(|| (p.stroke_color().unwrap_or(PdfColor::BLACK), p.stroke_width().map_or(0.0, |w| w.value))),
            fill: filled.then(|| (p.fill_color().unwrap_or(PdfColor::BLACK), fill_mode)),
            cap: p.line_cap().unwrap_or(PdfPageObjectLineCap::Butt),
            join: p.line_join().unwrap_or(PdfPageObjectLineJoin::Miter),
            mergeable: stroked && !filled && !dashed && !object.has_transparency(),
            segments: p.segments().iter().map(|s| (s.segment_type(), s.x().value, s.y().value, s.is_close())).collect(),
        });
    }

    fn same_style(a: &Leaf, b: &Leaf) -> bool {
        let colour = |l: &Leaf| l.stroke.map(|(c, w)| (c.red(), c.green(), c.blue(), c.alpha(), w.to_bits()));
        let m = |l: &Leaf| [l.matrix.a(), l.matrix.b(), l.matrix.c(), l.matrix.d(), l.matrix.e(), l.matrix.f()].map(f32::to_bits);
        colour(a) == colour(b)
            && m(a) == m(b)
            && format!("{:?}{:?}", a.cap, a.join) == format!("{:?}{:?}", b.cap, b.join)
    }

    /// Adds the leaves to `page`, merged or not; returns how many objects.
    fn build<'a>(doc: &PdfDocument<'a>, page: &mut PdfPage<'a>, leaves: &[Leaf], merge: bool) -> Result<usize, String> {
        let pt = PdfPoints::new;
        let mut added = 0;
        let mut i = 0;
        while i < leaves.len() {
            let mut j = i + 1;
            if merge && leaves[i].mergeable {
                while j < leaves.len() && leaves[j].mergeable && same_style(&leaves[i], &leaves[j]) {
                    j += 1;
                }
            }
            let first = &leaves[i];
            let Some(&(_, x0, y0, _)) = first.segments.first() else {
                i = j;
                continue;
            };
            let mut object = PdfPagePathObject::new(
                doc,
                pt(x0),
                pt(y0),
                first.stroke.map(|s| s.0),
                first.stroke.map(|s| pt(s.1)),
                first.fill.map(|f| f.0),
            )
            .map_err(err)?;
            for (k, leaf) in leaves[i..j].iter().enumerate() {
                let mut bezier: Vec<(f32, f32)> = Vec::new();
                for (n, &(kind, x, y, close)) in leaf.segments.iter().enumerate() {
                    if k > 0 || n > 0 {
                        match kind {
                            PdfPathSegmentType::MoveTo => object.move_to(pt(x), pt(y)).map_err(err)?,
                            PdfPathSegmentType::LineTo => object.line_to(pt(x), pt(y)).map_err(err)?,
                            PdfPathSegmentType::BezierTo => {
                                bezier.push((x, y));
                                if let [(c1x, c1y), (c2x, c2y), (ex, ey)] = bezier[..] {
                                    object.bezier_to(pt(ex), pt(ey), pt(c1x), pt(c1y), pt(c2x), pt(c2y)).map_err(err)?;
                                    bezier.clear();
                                }
                            }
                            #[allow(unreachable_patterns)]
                            _ => {}
                        }
                    }
                    if close {
                        object.close_path().map_err(err)?;
                    }
                }
            }
            object
                .set_fill_and_stroke_mode(first.fill.map_or(PdfPathFillMode::None, |f| f.1), first.stroke.is_some())
                .map_err(err)?;
            let _ = object.set_line_cap(first.cap);
            let _ = object.set_line_join(first.join);
            object.apply_matrix(first.matrix).map_err(err)?;
            page.objects_mut().add_path_object(object).map_err(err)?;
            added += 1;
            i = j;
        }
        Ok(added)
    }

    /// Share of pixels differing by more than 32 in any channel, and the mean
    /// largest channel difference.
    fn differ(a: &[u8], b: &[u8]) -> (f64, f64) {
        let pixels = (a.len() / 4).max(1);
        let (mut off, mut sum) = (0usize, 0u64);
        for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            let d = (0..3).map(|c| (p[c] as i32 - q[c] as i32).unsigned_abs()).max().unwrap_or(0);
            sum += d as u64;
            if d > 32 {
                off += 1;
            }
        }
        (off as f64 * 100.0 / pixels as f64, sum as f64 / pixels as f64)
    }

    let pdfium = worker::bind()?;
    let index = page_number.saturating_sub(1);
    let copy = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    {
        let mut page = copy.pages().get(index as PdfPageIndex).map_err(err)?;
        page.flatten().map_err(err)?;
    }
    let source = copy.pages().get(index as PdfPageIndex).map_err(err)?;
    let sizes = annots::page_sizes(&copy);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    let [w, h] = sizes[index];
    let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit, 1.0, 16384.0);
    let (original, took) = timed(|| annots::render_loaded_page(&source, scale));
    let (original_size, original) = original?;
    println!("page {page_number} flattened, at {scale:.2} px/pt: drawn in {}", ms(took));
    if let Some(dir) = dump {
        save_png(&dir.join("flattened.png"), original_size, &original)?;
    }

    let mut leaves = Vec::new();
    let mut skipped = 0;
    let (_, collecting) = timed(|| {
        let objects = source.objects();
        for i in 0..objects.len() {
            if let Ok(object) = objects.get(i) {
                collect(&object, PdfMatrix::IDENTITY, &mut leaves, &mut skipped);
            }
        }
    });
    println!(
        "  read {} paths ({:.0}% mergeable) and left out {skipped} other objects, in {}",
        leaves.len(),
        leaves.iter().filter(|l| l.mergeable).count() as f64 * 100.0 / leaves.len().max(1) as f64,
        ms(collecting)
    );

    let mut images: Vec<Vec<u8>> = Vec::new();
    for merge in [false, true] {
        let mut doc = pdfium.create_new_pdf().map_err(err)?;
        let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(w), PdfPoints::new(h))).map_err(err)?;
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        let (added, building) = timed(|| build(&doc, &mut page, &leaves, merge));
        let added = added?;
        let draws: Vec<String> = (0..3).map(|_| ms(timed(|| annots::render_loaded_page(&page, scale)).1)).collect();
        let (_, eighth) = timed(|| annots::render_loaded_page(&page, scale / 8.0));
        let (size, rgba) = annots::render_loaded_page(&page, scale)?;
        if let Some(dir) = dump {
            save_png(&dir.join(if merge { "merged.png" } else { "one-each.png" }), size, &rgba)?;
        }
        println!(
            "  {}: {added} objects, built in {}; drawn in {}; at an eighth of the size {}",
            if merge { "merged    " } else { "one each  " },
            ms(building),
            draws.join(", "),
            ms(eighth)
        );
        let _ = std::io::stdout().flush();
        images.push(rgba);
        drop(page);
        drop(doc);
    }
    let (off, mean) = differ(&images[0], &images[1]);
    println!("  merged against one each: {off:.3}% of pixels differ by more than 32, mean difference {mean:.2}");
    let (off, mean) = differ(&original, &images[0]);
    println!("  one each against the flattened page (which also has images, text and transparency): {off:.2}% differ, mean {mean:.2}");
    Ok(())
}

/// Where a page's annotation drawing time goes, beyond the paths. The page
/// (numbered from 1) is flattened into a fresh copy for each case, one kind of
/// object is removed from inside every annotation form, and the page is drawn
/// at fit width three times; the fastest is printed. Also counts the paths
/// with a clip path, which merging would have to respect. Printed only.
fn stamp_breakdown(path: &Path, page_number: usize) -> Result<(), String> {
    /// Removes every object inside `object`'s forms, at any depth, for which
    /// `remove` is true. Returns how many.
    fn strip(object: &mut PdfPageObject, remove: &dyn Fn(&PdfPageObject) -> bool) -> usize {
        let Some(form) = object.as_x_object_form_object_mut() else { return 0 };
        let mut removed = 0;
        for i in (0..form.len()).rev() {
            let Ok(mut child) = form.get(i) else { continue };
            if child.as_x_object_form_object().is_some() {
                removed += strip(&mut child, remove);
            } else if remove(&child) && form.remove_object(child).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    fn count_clipped(object: &PdfPageObject, paths: &mut usize, clipped: &mut usize) {
        if let Some(form) = object.as_x_object_form_object() {
            for i in 0..form.len() {
                if let Ok(child) = form.get(i) {
                    count_clipped(&child, paths, clipped);
                }
            }
        } else if object.as_path_object().is_some() {
            *paths += 1;
            if object.get_clip_path().is_some_and(|c| c.len() > 0) {
                *clipped += 1;
            }
        }
    }

    let pdfium = worker::bind()?;
    let index = page_number.saturating_sub(1);
    let sizes = annots::page_sizes(&pdfium.load_pdf_from_file(path, None).map_err(err)?);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    let [w, h] = sizes[index];
    let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit, 1.0, 16384.0);
    println!("page {page_number} flattened, at {scale:.2} px/pt");

    let is_image = |o: &PdfPageObject| o.as_image_object().is_some();
    let is_text = |o: &PdfPageObject| o.as_text_object().is_some();
    let is_transparent = |o: &PdfPageObject| o.has_transparency();
    let is_path = |o: &PdfPageObject| o.as_path_object().is_some();
    let not_path = |o: &PdfPageObject| o.as_path_object().is_none();
    let nothing = |_: &PdfPageObject| false;
    let cases: [(&str, &dyn Fn(&PdfPageObject) -> bool); 6] = [
        ("as it is", &nothing),
        ("without images", &is_image),
        ("without text", &is_text),
        ("without transparent objects", &is_transparent),
        ("without paths", &is_path),
        ("paths only", &not_path),
    ];
    for (label, remove) in cases {
        let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
        doc.pages().get(index as PdfPageIndex).map_err(err)?.flatten().map_err(err)?;
        let page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
        let mut removed = 0;
        let (mut paths, mut clipped) = (0, 0);
        let objects = page.objects();
        for i in 0..objects.len() {
            if let Ok(mut object) = objects.get(i) {
                removed += strip(&mut object, remove);
                if label == "as it is" {
                    count_clipped(&object, &mut paths, &mut clipped);
                }
            }
        }
        let fastest = (0..3).map(|_| timed(|| annots::render_loaded_page(&page, scale)).1).min().unwrap_or_default();
        println!("  {label:<28} {removed:>7} objects removed, drawn in {}", ms(fastest));
        if label == "as it is" {
            println!("  ({clipped} of {paths} paths have a clip path)");
        }
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// The drawing instructions inside a page's annotation appearances (numbered
/// from 1), read with lopdf: every form they use, how often each is used, the
/// operators by count, the commonest sequences of operators leading up to each
/// painted path, and the first instructions of the largest form. Printed only.
fn stamp_ops(path: &Path, page_number: usize) -> Result<(), String> {
    use lopdf::content::Content;
    use lopdf::{Document, Object, ObjectId};
    use std::collections::HashMap;

    struct Form {
        operations: usize,
        sample: Vec<String>,
    }

    fn operand(o: &Object) -> String {
        match o {
            Object::Integer(i) => i.to_string(),
            Object::Real(r) => format!("{r:.2}"),
            Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
            Object::Array(a) => format!("[{}]", a.iter().map(operand).collect::<Vec<_>>().join(" ")),
            Object::String(..) => "(..)".to_owned(),
            other => format!("{other:?}").chars().take(20).collect(),
        }
    }

    fn visit(
        doc: &Document,
        id: ObjectId,
        uses: &mut HashMap<ObjectId, usize>,
        forms: &mut HashMap<ObjectId, Form>,
        operators: &mut HashMap<String, usize>,
        sequences: &mut HashMap<String, usize>,
    ) -> Result<(), String> {
        *uses.entry(id).or_default() += 1;
        if forms.contains_key(&id) {
            return Ok(());
        }
        let stream = doc.get_object(id).and_then(Object::as_stream).map_err(|e| e.to_string())?;
        let data = stream.get_plain_content().map_err(|e| e.to_string())?;
        let content = Content::decode(&data).map_err(|e| e.to_string())?;
        let mut since_paint: Vec<&str> = Vec::new();
        for op in &content.operations {
            *operators.entry(op.operator.clone()).or_default() += 1;
            since_paint.push(&op.operator);
            if matches!(op.operator.as_str(), "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" | "Do") {
                // Collapse repeated path-building operators so "m l l l S" and
                // "m l S" read as one pattern.
                let mut pattern: Vec<&str> = Vec::new();
                for name in since_paint.drain(..) {
                    if pattern.last() != Some(&name) || !matches!(name, "l" | "c" | "m" | "re") {
                        pattern.push(name);
                    }
                }
                *sequences.entry(pattern.join(" ")).or_default() += 1;
            }
        }
        let sample = content
            .operations
            .iter()
            .take(80)
            .map(|op| format!("{} {}", op.operands.iter().map(operand).collect::<Vec<_>>().join(" "), op.operator))
            .collect();
        forms.insert(id, Form { operations: content.operations.len(), sample });

        // Forms this one draws.
        let resources = stream.dict.get(b"Resources").ok().and_then(|r| doc.dereference(r).ok()).and_then(|(_, r)| r.as_dict().ok());
        let xobjects = resources.and_then(|r| r.get(b"XObject").ok()).and_then(|x| doc.dereference(x).ok()).and_then(|(_, x)| x.as_dict().ok());
        if let Some(xobjects) = xobjects {
            for (_, value) in xobjects.iter() {
                let Ok(child) = value.as_reference() else { continue };
                let is_form = doc
                    .get_object(child)
                    .and_then(Object::as_stream)
                    .is_ok_and(|s| s.dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Form"));
                if is_form {
                    visit(doc, child, uses, forms, operators, sequences)?;
                }
            }
        }
        Ok(())
    }

    let (doc, took) = timed(|| Document::load(path));
    let doc = doc.map_err(|e| e.to_string())?;
    println!("loaded with lopdf in {}", ms(took));
    let page_id = *doc.get_pages().get(&(page_number as u32)).ok_or("no such page")?;
    let page = doc.get_dictionary(page_id).map_err(|e| e.to_string())?;
    let annots = page.get(b"Annots").ok().and_then(|a| doc.dereference(a).ok()).and_then(|(_, a)| a.as_array().ok()).cloned().unwrap_or_default();

    let mut uses = HashMap::new();
    let mut forms = HashMap::new();
    let mut operators = HashMap::new();
    let mut sequences = HashMap::new();
    let (visited, took) = timed(|| -> Result<(), String> {
        for annot in &annots {
            let Ok((_, annot)) = doc.dereference(annot) else { continue };
            let Ok(annot) = annot.as_dict() else { continue };
            let Some(normal) = annot.get(b"AP").ok().and_then(|ap| doc.dereference(ap).ok()).and_then(|(_, ap)| ap.as_dict().ok()).and_then(|ap| ap.get(b"N").ok()) else {
                continue;
            };
            if let Ok(id) = normal.as_reference() {
                visit(&doc, id, &mut uses, &mut forms, &mut operators, &mut sequences)?;
            }
        }
        Ok(())
    });
    visited?;
    println!("{} annotations use {} forms; decoded in {}", annots.len(), forms.len(), ms(took));
    let shared = uses.values().filter(|&&n| n > 1).count();
    println!("  {shared} forms are used more than once (most {} times)", uses.values().max().unwrap_or(&0));

    let mut by_count: Vec<(&String, &usize)> = operators.iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(a.1));
    println!(
        "  operators: {}",
        by_count.iter().take(20).map(|(op, n)| format!("{op} {n}")).collect::<Vec<_>>().join(", ")
    );
    let mut patterns: Vec<(&String, &usize)> = sequences.iter().collect();
    patterns.sort_by(|a, b| b.1.cmp(a.1));
    println!("  commonest instructions before each paint:");
    for (pattern, n) in patterns.iter().take(12) {
        let shown: String = pattern.chars().take(100).collect();
        println!("    {n:>7}  {shown}");
    }
    if let Some((id, form)) = forms.iter().max_by_key(|(_, f)| f.operations) {
        println!("  largest form {id:?}: {} instructions; the first {}:", form.operations, form.sample.len());
        for line in &form.sample {
            println!("    {line}");
        }
    }
    Ok(())
}

/// What merging back-to-back strokes (merge.rs) does to a page (numbered from
/// 1): the whole document merged with a few limits on parts to a path, each
/// copy's page drawn at fit width and a view-sized area of it at 800% of fit
/// width, against the original, with how far the fit-width pixels differ.
/// Printed only.
fn merge_render(path: &Path, page_number: usize) -> Result<(), String> {
    use pdf_annotate::merge;

    fn differ(a: &[u8], b: &[u8]) -> (f64, f64) {
        let pixels = (a.len() / 4).max(1);
        let (mut off, mut sum) = (0usize, 0u64);
        for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            let d = (0..3).map(|c| (p[c] as i32 - q[c] as i32).unsigned_abs()).max().unwrap_or(0);
            sum += d as u64;
            if d > 32 {
                off += 1;
            }
        }
        (off as f64 * 100.0 / pixels as f64, sum as f64 / pixels as f64)
    }

    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let pdfium = worker::bind()?;
    let index = page_number.saturating_sub(1);
    let original = pdfium.load_pdf_from_byte_slice(&bytes, None).map_err(err)?;
    let sizes = annots::page_sizes(&original);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    let [w, h] = sizes[index];
    let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit, 1.0, 16384.0);
    let full = [(w * fit * 8.0).round() as u32, (h * fit * 8.0).round() as u32];
    let (vw, vh) = ((VIEW_W as u32).min(full[0]), (VIEW_H as u32).min(full[1]));
    let region = [(full[0] - vw) / 2, (full[1] - vh) / 2, vw, vh];

    let measure = |page: &PdfPage| -> Result<(Duration, Duration, Vec<u8>), String> {
        let fit_time = (0..3).map(|_| timed(|| annots::render_loaded_page(page, scale)).1).min().unwrap_or_default();
        let zoomed = (0..2).map(|_| timed(|| annots::render_region_in_steps(page, full, region, |_| true).map(drop)).1).min().unwrap_or_default();
        let (_, rgba) = annots::render_loaded_page(page, scale)?;
        Ok((fit_time, zoomed, rgba))
    };

    let page = original.pages().get(index as PdfPageIndex).map_err(err)?;
    let (fit_time, zoomed, reference) = measure(&page)?;
    println!("page {page_number}: original drawn at fit width in {}, a view at 800% in {}", ms(fit_time), ms(zoomed));
    drop(page);

    for most in [usize::MAX, 1024, 256, 64] {
        let (merged, took) = timed(|| merge::merge_document(&bytes, most));
        let Some((out, stats)) = merged? else {
            println!("  nothing to merge");
            return Ok(());
        };
        let size = out.len();
        let doc = pdfium.load_pdf_from_byte_vec(out, None).map_err(err)?;
        let page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
        let (fit_time, zoomed, rgba) = measure(&page)?;
        let (off, mean) = differ(&reference, &rgba);
        println!(
            "  at most {}: merged in {} ({} of {} streams rewritten, {} left see-through; {} of {} strokes merged; {:.1} MB)",
            if most == usize::MAX { "unlimited".to_owned() } else { most.to_string() },
            ms(took),
            stats.rewritten,
            stats.streams,
            stats.see_through,
            stats.merged,
            stats.strokes,
            size as f64 / MB
        );
        println!("    fit width {}, 800% view {}; {off:.3}% of pixels differ by more than 32, mean {mean:.2}", ms(fit_time), ms(zoomed));
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// The transparency a page's annotations (numbered from 1) use, read with
/// lopdf: each annotation's own opacity, and for every form under it, the
/// graphics states that aren't plainly opaque, with what they set. Printed only.
fn gstates(path: &Path, page_number: usize) -> Result<(), String> {
    use lopdf::{Document, Object, ObjectId};
    use std::collections::HashSet;

    fn show(o: &Object) -> String {
        match o {
            Object::Integer(i) => i.to_string(),
            Object::Real(r) => format!("{r}"),
            Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
            Object::Reference(id) => format!("{id:?}"),
            other => format!("{other:?}").chars().take(40).collect(),
        }
    }

    fn visit(doc: &Document, id: ObjectId, depth: usize, seen: &mut HashSet<ObjectId>, lines: &mut Vec<String>) {
        if !seen.insert(id) {
            return;
        }
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else { return };
        let group = stream.dict.get(b"Group").ok().and_then(|g| doc.dereference(g).ok()).and_then(|(_, g)| g.as_dict().ok());
        let resources = stream.dict.get(b"Resources").ok().and_then(|r| doc.dereference(r).ok()).and_then(|(_, r)| r.as_dict().ok());
        let mut notes = Vec::new();
        if let Some(group) = group {
            notes.push(format!("group {}", group.iter().map(|(k, v)| format!("/{} {}", String::from_utf8_lossy(k), show(v))).collect::<Vec<_>>().join(" ")));
        }
        if let Some(states) = resources.and_then(|r| r.get(b"ExtGState").ok()).and_then(|s| doc.dereference(s).ok()).and_then(|(_, s)| s.as_dict().ok()) {
            for (name, state) in states.iter() {
                let Ok((_, state)) = doc.dereference(state) else { continue };
                let Ok(state) = state.as_dict() else { continue };
                let keys: Vec<String> = state
                    .iter()
                    .filter(|(k, _)| matches!(k.as_slice(), b"CA" | b"ca" | b"BM" | b"SMask" | b"AIS" | b"TK"))
                    .map(|(k, v)| format!("/{} {}", String::from_utf8_lossy(k), show(v)))
                    .collect();
                notes.push(format!("gs /{}: {}", String::from_utf8_lossy(name), if keys.is_empty() { "nothing transparent".to_owned() } else { keys.join(" ") }));
            }
        }
        lines.push(format!("{}form {id:?}{}", "  ".repeat(depth + 2), if notes.is_empty() { String::new() } else { format!(": {}", notes.join("; ")) }));
        if let Some(xobjects) = resources.and_then(|r| r.get(b"XObject").ok()).and_then(|x| doc.dereference(x).ok()).and_then(|(_, x)| x.as_dict().ok()) {
            for (_, value) in xobjects.iter() {
                if let Ok(child) = value.as_reference() {
                    let is_form = doc
                        .get_object(child)
                        .and_then(Object::as_stream)
                        .is_ok_and(|s| s.dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Form"));
                    if is_form {
                        visit(doc, child, depth + 1, seen, lines);
                    }
                }
            }
        }
    }

    let doc = Document::load(path).map_err(|e| e.to_string())?;
    let page_id = *doc.get_pages().get(&(page_number as u32)).ok_or("no such page")?;
    let mut seen = HashSet::new();
    for (k, annot) in doc.get_page_annotations(page_id).map_err(|e| e.to_string())?.iter().enumerate() {
        let subtype = annot.get(b"Subtype").map(show).unwrap_or_default();
        let opacity = annot.get(b"CA").map(show).unwrap_or_else(|_| "none".to_owned());
        println!("annotation {} {subtype}: /CA {opacity}", k + 1);
        let normal = annot.get(b"AP").ok().and_then(|ap| doc.dereference(ap).ok()).and_then(|(_, ap)| ap.as_dict().ok()).and_then(|ap| ap.get(b"N").ok());
        if let Some(Ok(id)) = normal.map(Object::as_reference) {
            let mut lines = Vec::new();
            visit(&doc, id, 0, &mut seen, &mut lines);
            for line in lines.iter().take(40) {
                println!("{line}");
            }
            if lines.len() > 40 {
                println!("    ... {} more forms", lines.len() - 40);
            }
        }
    }
    Ok(())
}

/// One object of a PDF, by number, printed with lopdf: a dictionary in full,
/// references inside it followed one level, and a stream's dictionary only.
/// Printed only.
fn print_object(path: &Path, number: u32) -> Result<(), String> {
    use lopdf::{Document, Object};
    fn show(doc: &Document, o: &Object, depth: usize) -> String {
        match o {
            Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
            Object::String(s, _) => format!("({})", String::from_utf8_lossy(s)),
            Object::Reference(id) if depth < 2 => match doc.get_object(*id) {
                Ok(Object::Stream(s)) => format!("{} {} R (stream {})", id.0, id.1, show(doc, &Object::Dictionary(s.dict.clone()), depth + 1)),
                Ok(inner) => format!("{} {} R = {}", id.0, id.1, show(doc, inner, depth + 1)),
                Err(_) => format!("{} {} R (missing)", id.0, id.1),
            },
            Object::Reference(id) => format!("{} {} R", id.0, id.1),
            Object::Array(a) => format!("[{}]", a.iter().map(|x| show(doc, x, depth)).collect::<Vec<_>>().join(" ")),
            Object::Dictionary(d) => format!(
                "<< {} >>",
                d.iter().map(|(k, v)| format!("/{} {}", String::from_utf8_lossy(k), show(doc, v, depth))).collect::<Vec<_>>().join(" ")
            ),
            Object::Stream(s) => format!("stream {}", show(doc, &Object::Dictionary(s.dict.clone()), depth)),
            other => format!("{other:?}"),
        }
    }
    let doc = Document::load(path).map_err(|e| e.to_string())?;
    let object = doc.get_object((number, 0)).map_err(|e| e.to_string())?;
    println!("{number} 0 R = {}", show(&doc, object, 0));
    Ok(())
}

/// Each annotation's dictionary on a page (numbered from 1), read with lopdf,
/// without its appearance streams: flags, optional content, links to other
/// annotations, names, and any application-specific keys. Then the document's
/// optional content (layers): their names, which are on or off by default,
/// and the page's use of them. Printed only.
fn annot_dicts(path: &Path, page_number: usize) -> Result<(), String> {
    use lopdf::{Document, Object};

    fn show(doc: &Document, o: &Object, depth: usize) -> String {
        match o {
            Object::Integer(i) => i.to_string(),
            Object::Real(r) => format!("{r}"),
            Object::Boolean(b) => b.to_string(),
            Object::Null => "null".to_owned(),
            Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
            Object::String(s, _) => format!("({})", String::from_utf8_lossy(s).chars().take(60).collect::<String>()),
            Object::Reference(id) => {
                // Follow references to small things, so names and arrays read.
                match doc.get_object(*id) {
                    Ok(Object::Stream(_)) => format!("{} {} R (stream)", id.0, id.1),
                    Ok(inner) if depth < 2 => format!("{} {} R = {}", id.0, id.1, show(doc, inner, depth + 1)),
                    _ => format!("{} {} R", id.0, id.1),
                }
            }
            Object::Array(a) => {
                let items: Vec<String> = a.iter().take(12).map(|x| show(doc, x, depth + 1)).collect();
                format!("[{}{}]", items.join(" "), if a.len() > 12 { format!(" ... {} more", a.len() - 12) } else { String::new() })
            }
            Object::Dictionary(d) if depth < 3 => format!(
                "<< {} >>",
                d.iter().map(|(k, v)| format!("/{} {}", String::from_utf8_lossy(k), show(doc, v, depth + 1))).collect::<Vec<_>>().join(" ")
            ),
            Object::Dictionary(_) => "<< ... >>".to_owned(),
            Object::Stream(_) => "(stream)".to_owned(),
        }
    }

    let doc = Document::load(path).map_err(|e| e.to_string())?;
    let page_id = *doc.get_pages().get(&(page_number as u32)).ok_or("no such page")?;
    let page = doc.get_dictionary(page_id).map_err(|e| e.to_string())?;
    let annots = page.get(b"Annots").ok().and_then(|a| doc.dereference(a).ok()).and_then(|(_, a)| a.as_array().ok()).cloned().unwrap_or_default();
    for (k, reference) in annots.iter().enumerate() {
        let id = reference.as_reference().map(|id| format!("{} {} R", id.0, id.1)).unwrap_or_default();
        let Ok((_, annot)) = doc.dereference(reference) else { continue };
        let Ok(annot) = annot.as_dict() else { continue };
        println!("annotation {} ({id}):", k + 1);
        for (key, value) in annot.iter() {
            let key = String::from_utf8_lossy(key);
            let shown = if key == "AP" {
                match doc.dereference(value).ok().and_then(|(_, v)| v.as_dict().ok()) {
                    Some(ap) => ap.iter().map(|(k, v)| format!("/{} {}", String::from_utf8_lossy(k), show(&doc, v, 2))).collect::<Vec<_>>().join(" "),
                    None => show(&doc, value, 0),
                }
            } else {
                show(&doc, value, 0)
            };
            let shown: String = shown.chars().take(300).collect();
            println!("  /{key} {shown}");
        }
        // The appearance's own dictionary: its box, matrix and any optional content.
        if let Some(Ok(normal)) = annot
            .get(b"AP")
            .ok()
            .and_then(|ap| doc.dereference(ap).ok())
            .and_then(|(_, ap)| ap.as_dict().ok())
            .and_then(|ap| ap.get(b"N").ok())
            .map(Object::as_reference)
        {
            if let Ok(stream) = doc.get_object(normal).and_then(Object::as_stream) {
                let keys: Vec<String> = stream
                    .dict
                    .iter()
                    .filter(|(k, _)| !matches!(k.as_slice(), b"Resources" | b"Length" | b"Filter"))
                    .map(|(k, v)| format!("/{} {}", String::from_utf8_lossy(k), show(&doc, v, 1)))
                    .collect();
                println!("  appearance {} {}: {}", normal.0, normal.1, keys.join(" ").chars().take(300).collect::<String>());
            }
        }
    }

    let catalog = doc.catalog().map_err(|e| e.to_string())?;
    match catalog.get(b"OCProperties") {
        Ok(properties) => println!("optional content: {}", show(&doc, properties, 0).chars().take(2000).collect::<String>()),
        Err(_) => println!("no optional content (layers) in the document"),
    }
    Ok(())
}

/// A page (numbered from 1) drawn at fit width the ways that matter for its
/// annotations, written to `dir` as PNGs to look at: as the helpers draw it
/// (highlights stripped, drawn in steps), drawn in one go, with annotations
/// but no form drawing, and with no annotations; plus the area around each
/// annotation at twice fit width as the helpers draw it. Printed only.
fn stamp_view(path: &Path, page_number: usize, dir: &Path) -> Result<(), String> {
    fn save_png(path: &Path, size: [usize; 2], rgba: &[u8]) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size[0] as u32, size[1] as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().and_then(|mut w| w.write_image_data(rgba)).map_err(|e| e.to_string())
    }

    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let pdfium = worker::bind()?;
    let index = page_number.saturating_sub(1);
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    let sizes = annots::page_sizes(&doc);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    let [w, h] = sizes[index];
    let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit, 1.0, 16384.0);
    let mut page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
    println!("page {page_number} at {scale:.2} px/pt; form fill environment: {}", doc.form().is_some());
    annots::strip_loaded_page(&mut page);

    let (size, rgba) = annots::render_page_in_steps(&page, scale, |_| true)?.ok_or("render stopped")?;
    save_png(&dir.join("1-as-helpers-draw.png"), size, &rgba)?;
    let (size, rgba) = annots::render_loaded_page(&page, scale)?;
    save_png(&dir.join("2-in-one-go.png"), size, &rgba)?;
    let config = |annotations: bool| {
        PdfRenderConfig::new().scale_page_by_factor(scale).render_annotations(annotations).render_form_data(false)
    };
    let bitmap = page.render_with_config(&config(true)).map_err(err)?;
    save_png(&dir.join("3-annotations-no-form-drawing.png"), [bitmap.width() as usize, bitmap.height() as usize], &bitmap.as_rgba_bytes())?;
    let bitmap = page.render_with_config(&config(false)).map_err(err)?;
    save_png(&dir.join("4-no-annotations.png"), [bitmap.width() as usize, bitmap.height() as usize], &bitmap.as_rgba_bytes())?;

    let full = [(w * fit * 2.0).round() as u32, (h * fit * 2.0).round() as u32];
    let (vw, vh) = ((VIEW_W as u32).min(full[0]), (VIEW_H as u32).min(full[1]));
    for (n, annotation) in page.annotations().iter().enumerate() {
        let Ok(r) = annotation.bounds() else { continue };
        println!(
            "  annotation {} {:?}: bounds {:.1}, {:.1} to {:.1}, {:.1}",
            n + 1,
            annotation.annotation_type(),
            r.left().value,
            r.bottom().value,
            r.right().value,
            r.top().value
        );
        let cx = ((r.left().value + r.right().value) / 2.0 / w * full[0] as f32) as i64;
        let cy = ((1.0 - (r.bottom().value + r.top().value) / 2.0 / h) * full[1] as f32) as i64;
        let x = (cx - vw as i64 / 2).clamp(0, (full[0] - vw) as i64) as u32;
        let y = (cy - vh as i64 / 2).clamp(0, (full[1] - vh) as i64) as u32;
        let (_, rgba) = annots::render_region_in_steps(&page, full, [x, y, vw, vh], |_| true)?.ok_or("render stopped")?;
        save_png(&dir.join(format!("5-annotation{}-2x.png", n + 1)), [vw as usize, vh as usize], &rgba)?;
    }
    Ok(())
}

/// Does a merged copy (merge.rs) draw like the file it came from? For every
/// page: the annotations each has, with their kinds and bounds, then the page
/// drawn at fit width and at four times fit width (capped as the app caps it)
/// from both, with how far the pixels differ. Pages that differ by more than a
/// trace are written to `dump` as PNGs, original and copy side by side. Also
/// prints the copy's trailer. Printed only.
fn copy_check(original: &Path, copy: &Path, dump: Option<&Path>) -> Result<(), String> {
    fn differ(a: &[u8], b: &[u8]) -> f64 {
        let pixels = (a.len() / 4).max(1);
        let off = a
            .chunks_exact(4)
            .zip(b.chunks_exact(4))
            .filter(|(p, q)| (0..3).any(|c| (p[c] as i32 - q[c] as i32).abs() > 32))
            .count();
        off as f64 * 100.0 / pixels as f64
    }
    fn save_png(path: &Path, size: [usize; 2], rgba: &[u8]) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size[0] as u32, size[1] as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().and_then(|mut w| w.write_image_data(rgba)).map_err(|e| e.to_string())
    }

    let copied = lopdf::Document::load(copy).map_err(|e| e.to_string())?;
    println!(
        "copy's trailer: {}",
        copied.trailer.iter().map(|(k, v)| format!("/{} {:?}", String::from_utf8_lossy(k), v)).collect::<Vec<_>>().join(", ")
    );
    drop(copied);

    let pdfium = worker::bind()?;
    let a = pdfium.load_pdf_from_file(original, None).map_err(err)?;
    let b = pdfium.load_pdf_from_file(copy, None).map_err(err)?;
    let sizes = annots::page_sizes(&a);
    println!("pages: {} in the original, {} in the copy", sizes.len(), b.pages().len());
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    for (i, &[w, h]) in sizes.iter().enumerate() {
        let (pa, pb) = (a.pages().get(i as PdfPageIndex).map_err(err)?, b.pages().get(i as PdfPageIndex).map_err(err)?);
        let describe = |page: &PdfPage| -> Vec<String> {
            page.annotations()
                .iter()
                .map(|an| {
                    let r = an.bounds().map(|r| format!("{:.1},{:.1},{:.1},{:.1}", r.left().value, r.bottom().value, r.right().value, r.top().value)).unwrap_or_default();
                    format!("{:?} [{r}]", an.annotation_type())
                })
                .collect()
        };
        let (da, db) = (describe(&pa), describe(&pb));
        println!("page {}: {} annotations in the original, {} in the copy{}", i + 1, da.len(), db.len(), if da == db { ", same kinds and bounds" } else { "" });
        if da != db {
            for (n, (x, y)) in da.iter().zip(db.iter().chain(std::iter::repeat(&String::new()))).enumerate() {
                if x != y {
                    println!("    {}: original {x}, copy {y}", n + 1);
                }
            }
            for extra in db.iter().skip(da.len()) {
                println!("    only in the copy: {extra}");
            }
        }
        for factor in [1.0_f32, 4.0] {
            let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit * factor, 1.0, 16384.0);
            let (size, ra) = annots::render_loaded_page(&pa, scale)?;
            let (_, rb) = annots::render_loaded_page(&pb, scale)?;
            let off = differ(&ra, &rb);
            println!("    at {factor}x fit width ({scale:.2} px/pt): {off:.3}% of pixels differ");
            if let (Some(dir), true) = (dump, off > 0.1) {
                save_png(&dir.join(format!("page{}-{factor}x-original.png", i + 1)), size, &ra)?;
                save_png(&dir.join(format!("page{}-{factor}x-copy.png", i + 1)), size, &rb)?;
            }
        }
        // Areas drawn zoomed in, as the app draws squares: a view-sized area
        // over the middle of each annotation, at two and eight times fit width.
        for factor in [2.0_f32, 8.0] {
            let full = [(w * fit * factor).round() as u32, (h * fit * factor).round() as u32];
            let (vw, vh) = ((VIEW_W as u32).min(full[0]), (VIEW_H as u32).min(full[1]));
            let mut worst = 0.0_f64;
            for (n, annotation) in pa.annotations().iter().enumerate() {
                let Ok(r) = annotation.bounds() else { continue };
                // Page space (origin bottom left) to pixels (origin top left).
                let cx = ((r.left().value + r.right().value) / 2.0 / w * full[0] as f32) as i64;
                let cy = ((1.0 - (r.bottom().value + r.top().value) / 2.0 / h) * full[1] as f32) as i64;
                let x = (cx - vw as i64 / 2).clamp(0, (full[0] - vw) as i64) as u32;
                let y = (cy - vh as i64 / 2).clamp(0, (full[1] - vh) as i64) as u32;
                let region = [x, y, vw, vh];
                let (_, ra) = annots::render_region_in_steps(&pa, full, region, |_| true)?.ok_or("render stopped")?;
                let (_, rb) = annots::render_region_in_steps(&pb, full, region, |_| true)?.ok_or("render stopped")?;
                let (ga, gb) = (&ra[..], &rb[..]);
                let off = differ(ga, gb);
                worst = worst.max(off);
                if let (Some(dir), true) = (dump, off > 0.1) {
                    save_png(&dir.join(format!("page{}-{factor}x-annot{}-original.png", i + 1, n + 1)), [vw as usize, vh as usize], ga)?;
                    save_png(&dir.join(format!("page{}-{factor}x-annot{}-copy.png", i + 1, n + 1)), [vw as usize, vh as usize], gb)?;
                    println!("    area over annotation {} at {factor}x: {off:.3}% differ", n + 1);
                }
            }
            println!("    areas over each annotation at {factor}x fit width: at most {worst:.3}% of pixels differ");
        }
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// What each annotation's appearance is made of: objects by type through every
/// nested form, how many use transparency or a blend mode, how deep the forms
/// go, and each image's size and compression. Printed only.
fn stamp_contents(path: &Path) -> Result<(), String> {
    let pdfium = worker::bind()?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    for i in 0..doc.pages().len() {
        let page = doc.pages().get(i).map_err(err)?;
        let annotations = page.annotations();
        for k in 0..annotations.len() {
            let Ok(annotation) = annotations.get(k) else { continue };
            let mut contents = Contents::default();
            let objects = annotation.objects();
            for index in 0..objects.len() {
                if let Ok(object) = objects.get(index) {
                    tally(&object, 0, &mut contents);
                }
            }
            let types: Vec<String> = contents.by_type.iter().map(|(kind, n)| format!("{n} {kind}")).collect();
            println!(
                "page {} annotation {} ({:?}): {}; {} with transparency or blending; forms nested {} deep",
                i + 1,
                k + 1,
                annotation.annotation_type(),
                types.join(", "),
                contents.transparent,
                contents.deepest
            );
            if !contents.images.is_empty() {
                let shown: Vec<&String> = contents.images.iter().take(6).collect();
                println!("    {} images: {:?}{}", contents.images.len(), shown, if contents.images.len() > 6 { " ..." } else { "" });
            }
            let _ = std::io::stdout().flush();
        }
    }
    Ok(())
}

/// A render helper started for `split`.
struct SplitHelper {
    child: std::process::Child,
    to: std::io::BufWriter<std::process::ChildStdin>,
    from: std::io::BufReader<std::process::ChildStdout>,
}

/// Would splitting one dense page between helpers draw it faster? For each of
/// the densest pages, at fit width: drawn whole by one helper, then cut into 2,
/// 3 and 4 horizontal strips drawn at the same time by that many helpers, timed
/// until the last strip arrives. Each helper loads the page for its strip, as
/// it would in the app. Printed only.
fn split(path: &Path) -> Result<(), String> {
    use pdf_annotate::helper::{self, Command as ToHelper, Event};
    use std::io::{BufReader, BufWriter, Write};
    use std::process::{Command, Stdio};

    // The densest pages and their size at fit width, from pdfium here.
    let pages: Vec<(usize, usize, [u32; 2])> = {
        let pdfium = worker::bind()?;
        let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
        let sizes = annots::page_sizes(&doc);
        let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
        let mut densest: Vec<(usize, usize)> = (0..sizes.len())
            .filter_map(|i| Some((doc.pages().get(i as PdfPageIndex).ok()?.objects().len(), i)))
            .collect();
        densest.sort_unstable_by(|a, b| b.cmp(a));
        densest
            .into_iter()
            .take(6)
            .map(|(objects, i)| (i, objects, [(sizes[i][0] * fit).round() as u32, (sizes[i][1] * fit).round() as u32]))
            .collect()
    };

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut helpers = Vec::new();
    for _ in 0..4 {
        let mut child = Command::new(&exe)
            .arg(helper::FLAG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let to = BufWriter::new(child.stdin.take().ok_or("no helper stdin")?);
        let from = BufReader::new(child.stdout.take().ok_or("no helper stdout")?);
        helpers.push(SplitHelper { child, to, from });
    }
    for h in &mut helpers {
        ToHelper::Open { version: 1, path: path.to_path_buf() }.write(&mut h.to).and_then(|()| h.to.flush()).map_err(|e| e.to_string())?;
        match Event::read(&mut h.from).map_err(|e| e.to_string())? {
            Event::Opened { error: None, .. } => {}
            Event::Opened { error: Some(e), .. } => return Err(format!("a helper could not open the file: {e}")),
            _ => return Err("a helper answered out of turn".to_owned()),
        }
    }

    println!("Densest pages at fit width, best of 2. Each strip's helper loads the page itself.");
    println!("page | objects |   1 piece |  2 pieces |  3 pieces |  4 pieces | best speed-up");
    let mut next_id = 1;
    for (page, objects, full) in pages {
        let mut times = Vec::new();
        for pieces in 1..=4 {
            let mut best = Duration::MAX;
            for _ in 0..2 {
                best = best.min(draw_in_strips(&mut helpers, &mut next_id, page, full, pieces)?);
            }
            times.push(best);
        }
        let speedup = times[0].as_secs_f64() / times.iter().min().copied().unwrap_or(times[0]).as_secs_f64();
        let cells: Vec<String> = times.iter().map(|t| format!("{:>9}", ms(*t))).collect();
        println!("{:>4} | {objects:>7} | {} | {speedup:.2}x", page + 1, cells.join(" | "));
        let _ = std::io::stdout().flush();
    }
    for h in &mut helpers {
        let _ = h.child.kill();
    }
    Ok(())
}

/// Draws `page` cut into `pieces` horizontal strips, one helper each, all at
/// once. Returns the time until the last strip arrived.
fn draw_in_strips(helpers: &mut [SplitHelper], next_id: &mut u64, page: usize, full: [u32; 2], pieces: usize) -> Result<Duration, String> {
    use pdf_annotate::helper::{Command as ToHelper, Event, Target};
    use std::io::Write;

    let first_id = *next_id;
    *next_id += pieces as u64;
    let started = Instant::now();
    std::thread::scope(|scope| {
        let strips: Vec<_> = helpers
            .iter_mut()
            .take(pieces)
            .enumerate()
            .map(|(i, h)| {
                let id = first_id + i as u64;
                let top = full[1] * i as u32 / pieces as u32;
                let bottom = full[1] * (i as u32 + 1) / pieces as u32;
                let target = Target::Region { full, region: [0, top, full[0], bottom - top] };
                scope.spawn(move || -> Result<(), String> {
                    ToHelper::Render { id, page: page as u32, target }
                        .write(&mut h.to)
                        .and_then(|()| h.to.flush())
                        .map_err(|e| e.to_string())?;
                    loop {
                        match Event::read(&mut h.from).map_err(|e| e.to_string())? {
                            Event::Image { id: got, complete: true, .. } if got == id => return Ok(()),
                            Event::Stopped { id: got, error, .. } if got == id => {
                                return Err(error.unwrap_or_else(|| "the strip was stopped".to_owned()))
                            }
                            _ => {}
                        }
                    }
                })
            })
            .collect();
        strips.into_iter().try_for_each(|strip| strip.join().unwrap_or_else(|_| Err("a strip thread panicked".to_owned())))
    })?;
    Ok(started.elapsed())
}

/// This process's memory in MB: private (committed memory only it uses, what
/// another process really adds) and working set (in RAM now, including shared
/// pages such as the DLL's code).
fn process_memory() -> (f64, f64) {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let ok = unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS, counters.cb)
    };
    if ok == 0 {
        return (0.0, 0.0);
    }
    (counters.PrivateUsage as f64 / MB, counters.WorkingSetSize as f64 / MB)
}

/// What one more pdfium process costs in memory, stage by stage: pdfium
/// itself, the open file (copied into memory, as the worker does now, and read
/// from disk as needed), then the densest pages loaded and drawn one at a
/// time. Printed only.
fn memory(path: &Path) -> Result<(), String> {
    let line = |label: &str| {
        let (private, working) = process_memory();
        println!("{private:>6.0} MB private  {working:>6.0} MB working set  {label}");
    };
    line("process started");
    let pdfium = worker::bind()?;
    line("pdfium loaded");
    {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let doc = pdfium.load_pdf_from_byte_vec(bytes, None).map_err(err)?;
        line("file opened from a copy in memory (how the worker opens it now)");
        drop(doc);
    }
    line("closed again");
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    line("file opened from disk, read as needed");

    let n = doc.pages().len();
    let mut densest: Vec<(usize, usize)> = (0..n)
        .filter_map(|i| {
            let page = doc.pages().get(i).ok()?;
            Some((page.objects().len(), i as usize))
        })
        .collect();
    line("every page loaded once and closed again");
    densest.sort_unstable_by(|a, b| b.cmp(a));

    let sizes = annots::page_sizes(&doc);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    let mut open = Vec::new();
    for &(objects, i) in densest.iter().take(6) {
        let page = doc.pages().get(i as PdfPageIndex).map_err(err)?;
        line(&format!("+ page {} loaded, {objects} objects", i + 1));
        let pixels = annots::render_loaded_page(&page, fit).map(|(size, _)| size[0] * size[1] * 4).unwrap_or(0);
        line(&format!("  and drawn at fit width ({:.0} MB of pixels, since freed)", pixels as f64 / MB));
        open.push(page);
    }
    line("the six densest pages open");
    drop(open);
    line("pages closed");
    Ok(())
}

/// Flings through the whole document in the real app, headless, the way a hard
/// spin of the scroll wheel does, and prints when each page image arrived --
/// showing whether the pages in view when the scroll stops come first, or the
/// worker catches up through every page passed on the way.
fn fling(path: &Path) -> Result<(), String> {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = pdf_annotate::app::App::new(&cc, Some(path.to_path_buf()));
    let mut frame = eframe::Frame::_new_kittest();

    let ppp = 2.0;
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VIEW_W as f32 / ppp, (VIEW_H as f32 + 130.0) / ppp));
    let started = Instant::now();

    // One frame; the names of the textures that arrived in it.
    let mut step = |events: Vec<egui::Event>| -> Vec<String> {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(started.elapsed().as_secs_f64()),
            max_texture_side: Some(16384),
            events,
            ..Default::default()
        };
        if let Some(viewport) = input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(ppp);
        }
        let mut output = ctx.run_ui(input, |ui| app.ui(ui, &mut frame));
        let arrived = {
            let manager = ctx.tex_manager();
            let manager = manager.read();
            output.textures_delta.set.keys().filter_map(|id| Some(manager.meta(*id)?.name.clone())).collect()
        };
        output.textures_delta.clear();
        arrived
    };

    // Let the first pages arrive, then fling for a second.
    while started.elapsed() < Duration::from_secs(6) {
        step(Vec::new());
        std::thread::sleep(Duration::from_millis(8));
    }
    let pointer = egui::Event::PointerMoved(screen.center());
    let flung = Instant::now();
    let mut arrivals: Vec<(Duration, String)> = Vec::new();
    for _ in 0..60 {
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -2000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        };
        for name in step(vec![pointer.clone(), wheel]) {
            arrivals.push((flung.elapsed(), name));
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    let stopped = flung.elapsed();

    // Then wait until nothing has arrived for a few seconds.
    let mut last = Instant::now();
    while last.elapsed() < Duration::from_secs(5) && flung.elapsed() < Duration::from_secs(60) {
        for name in step(vec![pointer.clone()]) {
            arrivals.push((flung.elapsed(), name));
            last = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(8));
    }

    let (private, working) = process_memory();
    println!("memory after scrolling and loading ahead: {private:.0} MB private, {working:.0} MB working set");
    println!("scrolling stopped at {}", ms(stopped));
    for (at, name) in &arrivals {
        println!("{:>9}  {name}", ms(*at));
    }
    let _ = std::io::stdout().flush();
    Ok(())
}

/// Every page's cost, one line each: its size and object count, then how long
/// loading it, reading its text and rendering it at fit width (and at a quarter
/// of that) take. Printed only, never saved to a report, so the details of a
/// private document stay off disk.
fn page_costs(path: &Path) -> Result<(), String> {
    let pdfium = worker::bind()?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let doc = pdfium.load_pdf_from_byte_vec(bytes, None).map_err(err)?;
    let sizes = annots::page_sizes(&doc);
    let usual = usual_size(&sizes);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual[0];
    println!("{} pages; fit width renders the usual page at {fit:.2} px/pt", sizes.len());
    println!("page | size pt     | objects | load     | text     | render fit | render fit/4 | pixels | in steps");
    let (mut load_total, mut text_total, mut render_total) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    for (i, &[w, h]) in sizes.iter().enumerate() {
        let (page, load) = timed(|| doc.pages().get(i as PdfPageIndex));
        let page = page.map_err(err)?;
        let objects = page.objects().len();
        let (_, text) = timed(|| annots::chars_of(&page));
        let scale = pdf_annotate::app::render_scale(egui::vec2(w, h), fit, 1.0, 16384.0);
        let (full, render) = timed(|| annots::render_loaded_page(&page, scale));
        let pixels = full.map(|([pw, ph], _)| pw * ph).unwrap_or(0);
        let (_, quarter) = timed(|| annots::render_loaded_page(&page, scale / 4.0));
        // The same render as the app does it, a step at a time: its result,
        // how many pauses, and whether the steps cost anything.
        let mut pauses = 0;
        let (stepped, stepped_time) = timed(|| {
            annots::render_page_in_steps(&page, scale, |_| {
                pauses += 1;
                true
            })
        });
        let stepped = match stepped {
            Ok(Some(_)) => format!("{} ({pauses} pauses)", ms(stepped_time)),
            Ok(None) => format!("abandoned after {pauses} pauses"),
            Err(e) => format!("FAILED after {pauses} pauses: {e}"),
        };
        println!(
            "{:>4} | {:>5.0} x {:<5.0} | {objects:>7} | {:>8} | {:>8} | {:>10} | {:>12} | {:.1} MP | {stepped}",
            i + 1,
            w,
            h,
            ms(load),
            ms(text),
            ms(render),
            ms(quarter),
            pixels as f64 / 1e6
        );
        let _ = std::io::stdout().flush();
        load_total += load;
        text_total += text;
        render_total += render;
    }
    println!("total: load {}, text {}, render at fit {}", ms(load_total), ms(text_total), ms(render_total));
    Ok(())
}

/// Which format the page cache should store images in: a whole page at fit
/// width, and an 8 x 8 block of squares at 800% from the middle of the page
/// (numbered from 1), each stored in every format the cache has, with the
/// space taken, the time to write and the time to read back. Printed only.
fn cache_formats(path: &Path, page_number: usize) -> Result<(), String> {
    use pdf_annotate::cache::{decode, encode, Format};
    use pdf_annotate::model::{cut_tiles, TILE};

    let pdfium = worker::bind()?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    let index = page_number.saturating_sub(1);
    let sizes = annots::page_sizes(&doc);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual_size(&sizes)[0];
    let [w, h] = sizes[index];
    let mut page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
    annots::strip_loaded_page(&mut page);

    let whole = annots::render_loaded_page(&page, pdf_annotate::app::render_scale(egui::vec2(w, h), fit, 1.0, 16384.0))?;
    let full = [(w * 16.0).round() as u32, (h * 16.0).round() as u32];
    let x = (full[0] / 2 / TILE).saturating_sub(4) * TILE;
    let y = (full[1] / 2 / TILE).saturating_sub(4) * TILE;
    let region = [x, y, (8 * TILE).min(full[0] - x), (8 * TILE).min(full[1] - y)];
    let (size, rgba) = annots::render_region_in_steps(&page, full, region, |_| true)?.ok_or("the render was stopped")?;
    let squares: Vec<([usize; 2], Vec<u8>)> = cut_tiles(full, region, size, &rgba).into_iter().map(|(_, _, s, p)| (s, p)).collect();

    println!("page {page_number}");
    for (label, images) in [("whole page at fit width", vec![whole]), ("64 squares at 800%", squares)] {
        let raw: usize = images.iter().map(|(_, p)| p.len()).sum();
        println!("  {label}: {:.1} MB of pixels", raw as f64 / MB);
        for format in [Format::Zlib, Format::PngFast, Format::PngBalanced] {
            let (mut stored, mut write, mut read) = (0, Duration::ZERO, Duration::ZERO);
            for (size, pixels) in &images {
                let (bytes, wrote) = timed(|| encode(*size, pixels, format));
                let bytes = bytes.map_err(|e| e.to_string())?;
                let (back, took) = timed(|| decode(&bytes));
                if back.map_or(true, |(_, p)| p != *pixels) {
                    return Err(format!("{format:?} didn't read back the same pixels"));
                }
                stored += bytes.len();
                write += wrote;
                read += took;
            }
            let n = images.len() as u32;
            println!(
                "    {:<12} {:>7.2} MB ({:>4.1}x smaller), writing {} and reading {} each",
                format!("{format:?}"),
                stored as f64 / MB,
                raw as f64 / stored.max(1) as f64,
                ms(write / n),
                ms(read / n)
            );
        }
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// PNG-style prediction: each row stored as its difference from what the
/// pixels before it predict (left, above, or the Paeth mix of both), whichever
/// leaves the smallest numbers, with a byte saying which. Line art becomes
/// mostly zeros, which compresses far better.
fn predict_rows(data: &[u8], width: usize, channels: usize) -> Vec<u8> {
    let stride = width * channels;
    let rows = data.len() / stride.max(1);
    let mut out = Vec::with_capacity(data.len() + rows);
    let mut candidate = vec![0u8; stride];
    for row in 0..rows {
        let line = &data[row * stride..(row + 1) * stride];
        let above = (row > 0).then(|| &data[(row - 1) * stride..row * stride]);
        let mut best: Option<(u64, u8, Vec<u8>)> = None;
        for filter in 1u8..=4 {
            for i in 0..stride {
                let a = if i >= channels { line[i - channels] } else { 0 };
                let b = above.map_or(0, |up| up[i]);
                let c = if i >= channels { above.map_or(0, |up| up[i - channels]) } else { 0 };
                let predicted = match filter {
                    1 => a,
                    2 => b,
                    3 => ((a as u16 + b as u16) / 2) as u8,
                    _ => paeth(a, b, c),
                };
                candidate[i] = line[i].wrapping_sub(predicted);
            }
            let cost: u64 = candidate.iter().map(|&v| (v as i8).unsigned_abs() as u64).sum();
            if best.as_ref().is_none_or(|(c, ..)| cost < *c) {
                best = Some((cost, filter, candidate.clone()));
            }
        }
        let (_, filter, bytes) = best.unwrap_or((0, 0, line.to_vec()));
        out.push(filter);
        out.extend_from_slice(&bytes);
    }
    out
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Undoes `predict_rows`, to time reading a square back.
fn unpredict_rows(data: &[u8], width: usize, channels: usize) -> Vec<u8> {
    let stride = width * channels;
    let rows = data.len() / (stride + 1).max(1);
    let mut out = vec![0u8; rows * stride];
    for row in 0..rows {
        let filter = data[row * (stride + 1)];
        let src = &data[row * (stride + 1) + 1..(row + 1) * (stride + 1)];
        for i in 0..stride {
            let a = if i >= channels { out[row * stride + i - channels] } else { 0 };
            let b = if row > 0 { out[(row - 1) * stride + i] } else { 0 };
            let c = if row > 0 && i >= channels { out[(row - 1) * stride + i - channels] } else { 0 };
            let predicted = match filter {
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => paeth(a, b, c),
                _ => 0,
            };
            out[row * stride + i] = src[i].wrapping_add(predicted);
        }
    }
    out
}

/// How small squares drawn zoomed in could be kept in the page cache. Draws
/// an 8 x 8 block of 800%-zoom squares from the middle of a page (numbered
/// from 1) and one from its top-left corner, and tries on each square: the
/// cache's format now (RGBA, fast zlib), without the unused alpha channel,
/// with PNG-style prediction, as a palette where a square has 256 colours or
/// fewer, and a few bytes for a square of one colour. Reports sizes and how
/// long reading a square back takes. Printed only.
fn tile_compression(path: &Path, page_number: usize) -> Result<(), String> {
    use flate2::read::ZlibDecoder;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use pdf_annotate::model::{cut_tiles, TILE};
    use std::io::Read;

    let zlib = |data: &[u8], level: u32| {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(level));
        encoder.write_all(data).and_then(|()| encoder.finish()).unwrap_or_default()
    };
    let inflate = |data: &[u8]| {
        let mut out = Vec::new();
        let _ = ZlibDecoder::new(data).read_to_end(&mut out);
        out
    };

    let pdfium = worker::bind()?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(err)?;
    let index = page_number.saturating_sub(1);
    let [w, h] = annots::page_sizes(&doc)[index];
    // 800% on a 200% display is 16 pixels per point, already a quarter power of two.
    let scale = 16.0_f32;
    let full = [(w * scale).round() as u32, (h * scale).round() as u32];
    let mut page = doc.pages().get(index as PdfPageIndex).map_err(err)?;
    annots::strip_loaded_page(&mut page);
    const BLOCK: u32 = 8;
    let middle = [(full[0] / 2 / TILE).saturating_sub(BLOCK / 2) * TILE, (full[1] / 2 / TILE).saturating_sub(BLOCK / 2) * TILE];
    println!("page {page_number} at 800%: {} x {} px, {} squares in all", full[0], full[1], full[0].div_ceil(TILE) * full[1].div_ceil(TILE));

    for (name, [x, y]) in [("middle", middle), ("top-left corner", [0, 0])] {
        let region = [x, y, (BLOCK * TILE).min(full[0] - x), (BLOCK * TILE).min(full[1] - y)];
        let (rendered, took) = timed(|| annots::render_region_in_steps(&page, full, region, |_| true));
        let (size, rgba) = match rendered {
            Ok(Some(image)) => image,
            Ok(None) => return Err("the render was stopped".to_owned()),
            Err(e) => return Err(e),
        };

        let (mut squares, mut solid, mut raw, mut now, mut rgb, mut predicted, mut palette, mut best) = (0, 0, 0, 0, 0, 0, 0, 0);
        let (mut read_now, mut read_best) = (Duration::ZERO, Duration::ZERO);
        for (_, _, [tw, _], pixels) in cut_tiles(full, region, size, &rgba) {
            squares += 1;
            raw += pixels.len();
            let current = zlib(&pixels, 1);
            now += current.len();
            read_now += timed(|| inflate(&current)).1;

            if pixels.chunks_exact(4).all(|p| p == &pixels[..4]) {
                solid += 1;
                best += 8;
                continue;
            }
            let colours: Vec<u8> = pixels.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            rgb += zlib(&colours, 1).len();
            let with_prediction = zlib(&predict_rows(&colours, tw, 3), 6);
            predicted += with_prediction.len();

            // A palette, where a square has 256 colours or fewer.
            let mut lookup: HashMap<[u8; 3], u8> = HashMap::new();
            let mut indices = Vec::with_capacity(colours.len() / 3);
            let mut fits = true;
            for p in colours.chunks_exact(3) {
                let key = [p[0], p[1], p[2]];
                let next = lookup.len();
                if next == 256 && !lookup.contains_key(&key) {
                    fits = false;
                    break;
                }
                indices.push(*lookup.entry(key).or_insert(next as u8));
            }
            let as_palette = fits.then(|| (lookup.len() * 3, zlib(&predict_rows(&indices, tw, 1), 6)));
            palette += as_palette.as_ref().map_or(with_prediction.len(), |(table, packed)| table + packed.len());

            let palette_size = as_palette.as_ref().map_or(usize::MAX, |(table, packed)| table + packed.len());
            if palette_size < with_prediction.len() {
                best += palette_size;
                let packed = &as_palette.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
                read_best += timed(|| unpredict_rows(&inflate(packed), tw, 1)).1;
            } else {
                best += with_prediction.len().min(current.len());
                read_best += timed(|| unpredict_rows(&inflate(&with_prediction), tw, 3)).1;
            }
        }
        let mb = |bytes: usize| bytes as f64 / MB;
        let per = |d: Duration| ms(d / squares.max(1) as u32);
        println!("  {name}: {squares} squares drawn in {}, {solid} of them one colour", ms(took));
        println!(
            "    raw {:.1} MB | cache now {:.2} MB | no alpha {:.2} MB | + prediction {:.2} MB | + palettes {:.2} MB | best of all, blank squares as a few bytes {:.2} MB ({:.1}x smaller than now)",
            mb(raw),
            mb(now),
            mb(rgb + solid * 8),
            mb(predicted + solid * 8),
            mb(palette + solid * 8),
            mb(best),
            now as f64 / best.max(1) as f64
        );
        println!("    reading one square back: now {}, best format {}", per(read_now), per(read_best));
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// Opens a file in the real app, headless, keeps the pointer moving until the
/// first page is sharp, rests it in the middle of the view for `rest_ms`, then
/// zooms straight to 800% there and times until the view is sharp. With a rest,
/// the spot has been drawn ahead of the zoom. Printed only.
fn rest_zoom(path: &Path, rest_ms: u64) -> Result<(), String> {
    use pdf_annotate::app::App;

    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(&cc, Some(path.to_path_buf()));
    let mut frame = eframe::Frame::_new_kittest();

    let ppp = 2.0;
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VIEW_W as f32 / ppp, (VIEW_H as f32 + 130.0) / ppp));
    let started = Instant::now();
    let mut step = |app: &mut App, events: Vec<egui::Event>| {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(started.elapsed().as_secs_f64()),
            max_texture_side: Some(16384),
            events,
            ..Default::default()
        };
        if let Some(viewport) = input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(ppp);
        }
        let mut output = ctx.run_ui(input, |ui| app.ui(ui, &mut frame));
        output.textures_delta.clear();
    };
    let centre = screen.center();

    // Keep the pointer moving, so nothing is drawn ahead yet.
    let mut wiggle = 0;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        wiggle += 1;
        let at = centre + egui::vec2(if wiggle % 2 == 0 { 12.0 } else { -12.0 }, 0.0);
        step(&mut app, vec![egui::Event::PointerMoved(at)]);
        if app.view_is_sharp() || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    let opened = started.elapsed();

    let rest = Instant::now();
    while rest.elapsed() < Duration::from_millis(rest_ms) {
        step(&mut app, vec![egui::Event::PointerMoved(centre)]);
        std::thread::sleep(Duration::from_millis(16));
    }

    step(&mut app, vec![egui::Event::PointerMoved(centre), egui::Event::Zoom(64.0)]);
    let zoomed = Instant::now();
    let mut sharp = None;
    while zoomed.elapsed() < Duration::from_secs(20) {
        step(&mut app, vec![egui::Event::PointerMoved(centre)]);
        if app.view_is_sharp() {
            sharp = Some(zoomed.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    println!(
        "opened sharp after {}; rested the pointer {rest_ms} ms; sharp at 800% after {}",
        ms(opened),
        sharp.map_or_else(|| "not within 20 s".to_owned(), ms)
    );
    let _ = std::io::stdout().flush();
    Ok(())
}

/// Zooms the real app, headless, in and out on the first page three times: to
/// 800%, then back to fit width with Ctrl+0, timing each until the app says
/// the view is sharp -- whether from memory, the disk cache or drawing.
/// Printed only.
fn zoom_cycle(path: &Path) -> Result<(), String> {
    use pdf_annotate::app::App;

    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(&cc, Some(path.to_path_buf()));
    let mut frame = eframe::Frame::_new_kittest();

    let ppp = 2.0;
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VIEW_W as f32 / ppp, (VIEW_H as f32 + 130.0) / ppp));
    let started = Instant::now();

    let mut step = |app: &mut App, events: Vec<egui::Event>| {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(started.elapsed().as_secs_f64()),
            max_texture_side: Some(16384),
            events,
            ..Default::default()
        };
        if let Some(viewport) = input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(ppp);
        }
        let mut output = ctx.run_ui(input, |ui| app.ui(ui, &mut frame));
        output.textures_delta.clear();
    };
    let pointer = egui::Event::PointerMoved(screen.center());

    // Frames after the events until the view is sharp, up to 20 s.
    let mut until_sharp = |app: &mut App, events: Vec<egui::Event>| -> Option<Duration> {
        step(app, events);
        let from = Instant::now();
        while from.elapsed() < Duration::from_secs(20) {
            step(app, vec![pointer.clone()]);
            if app.view_is_sharp() {
                return Some(from.elapsed());
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        None
    };
    let show = |t: Option<Duration>| t.map_or_else(|| "not within 20 s".to_owned(), ms);

    println!("opened: sharp after {}", show(until_sharp(&mut app, vec![pointer.clone()])));
    let fit_width = egui::Event::Key {
        key: egui::Key::Num0,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };
    for cycle in 1..=3 {
        std::thread::sleep(Duration::from_millis(500));
        let zoomed = until_sharp(&mut app, vec![pointer.clone(), egui::Event::Zoom(64.0)]);
        std::thread::sleep(Duration::from_millis(500));
        let back = until_sharp(&mut app, vec![pointer.clone(), fit_width.clone()]);
        println!("cycle {cycle}: sharp at 800% after {}; sharp back at fit width after {}", show(zoomed), show(back));
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// Zooms the real app, headless, in on the first page by `factor` in one step,
/// once that page has been drawn, and reports when the sharp view of the part
/// in view arrived and when the whole page at the new size did. Printed only.
fn zoom_in(path: &Path, factor: f32) -> Result<(), String> {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = pdf_annotate::app::App::new(&cc, Some(path.to_path_buf()));
    let mut frame = eframe::Frame::_new_kittest();

    let ppp = 2.0;
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VIEW_W as f32 / ppp, (VIEW_H as f32 + 130.0) / ppp));
    let started = Instant::now();

    // One frame; the names of the textures that arrived in it.
    let mut step = |events: Vec<egui::Event>| -> Vec<String> {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(started.elapsed().as_secs_f64()),
            max_texture_side: Some(16384),
            events,
            ..Default::default()
        };
        if let Some(viewport) = input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(ppp);
        }
        let mut output = ctx.run_ui(input, |ui| app.ui(ui, &mut frame));
        let arrived = {
            let manager = ctx.tex_manager();
            let manager = manager.read();
            output.textures_delta.set.keys().filter_map(|id| Some(manager.meta(*id)?.name.clone())).collect()
        };
        output.textures_delta.clear();
        arrived
    };

    // Let the first page draw, so the app knows whether it's slow.
    let pointer = egui::Event::PointerMoved(screen.center());
    let mut drawn = None;
    while started.elapsed() < Duration::from_secs(15) && drawn.is_none() {
        if step(vec![pointer.clone()]).iter().any(|name| name == "page-0") {
            drawn = Some(started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    let Some(drawn) = drawn else { return Err("the first page never arrived".to_owned()) };
    for _ in 0..60 {
        step(vec![pointer.clone()]);
        std::thread::sleep(Duration::from_millis(16));
    }

    step(vec![pointer.clone(), egui::Event::Zoom(factor)]);
    let zoomed = Instant::now();
    let (mut view, mut whole) = (None, None);
    while zoomed.elapsed() < Duration::from_secs(20) && (view.is_none() || whole.is_none()) {
        for name in step(vec![pointer.clone()]) {
            if name.starts_with("page-0-detail") && view.is_none() {
                view = Some(zoomed.elapsed());
            } else if name == "page-0" && whole.is_none() {
                whole = Some(zoomed.elapsed());
            }
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    let show = |t: Option<Duration>| t.map_or_else(|| "never".to_owned(), ms);
    println!(
        "first page drawn at fit width after {}; zoomed {factor}x: sharp view of the part in view after {}, whole page at the new size after {}",
        ms(drawn),
        show(view),
        show(whole)
    );
    let _ = std::io::stdout().flush();
    Ok(())
}

/// Zooms the real app, headless, all the way in (800%) at the middle of the
/// view, then reports how soon the sharp render of the view arrived and how
/// scrolling at that zoom went. With `page` (counted from 1), the view goes to
/// that page and waits for it to be drawn first.
fn deep_zoom(path: &Path, page: Option<usize>) -> Result<(), String> {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = pdf_annotate::app::App::new(&cc, Some(path.to_path_buf()));
    let mut frame = eframe::Frame::_new_kittest();

    let ppp = 2.0;
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VIEW_W as f32 / ppp, (VIEW_H as f32 + 130.0) / ppp));
    let started = Instant::now();

    // One frame: its time, and the name and size of each texture that arrived.
    let mut step = |app: &mut pdf_annotate::app::App, events: Vec<egui::Event>| -> (Duration, Vec<(String, [usize; 2])>) {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(started.elapsed().as_secs_f64()),
            max_texture_side: Some(16384),
            events,
            ..Default::default()
        };
        if let Some(viewport) = input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(ppp);
        }
        let (mut output, run) = timed(|| ctx.run_ui(input, |ui| app.ui(ui, &mut frame)));
        let shapes = std::mem::take(&mut output.shapes);
        let (_, tessellate) = timed(|| ctx.tessellate(shapes, output.pixels_per_point));
        let arrived = {
            let manager = ctx.tex_manager();
            let manager = manager.read();
            output
                .textures_delta
                .set
                .iter()
                .filter_map(|(id, deltas)| Some((manager.meta(*id)?.name.clone(), deltas.last()?.image.size())))
                .collect()
        };
        output.textures_delta.clear();
        (run + tessellate, arrived)
    };
    let is_detail = |arrived: &[(String, [usize; 2])]| arrived.iter().find(|(name, _)| name.contains("-detail")).map(|(_, size)| *size);

    while started.elapsed() < Duration::from_secs(4) {
        step(&mut app, Vec::new());
        std::thread::sleep(Duration::from_millis(8));
    }

    // Go to the page asked for and wait for it to be drawn, so the app knows
    // whether it's slow before zooming.
    let shown = page.unwrap_or(1).saturating_sub(1);
    if shown > 0 {
        app.go_to_page(shown);
        let name = format!("page-{shown}");
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut drawn = false;
        while !drawn && Instant::now() < deadline {
            drawn = step(&mut app, Vec::new()).1.iter().any(|(arrived, _)| *arrived == name);
            std::thread::sleep(Duration::from_millis(8));
        }
        if !drawn {
            return Err(format!("page {} was never drawn", shown + 1));
        }
        for _ in 0..60 {
            step(&mut app, Vec::new());
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    let pointer = egui::Event::PointerMoved(screen.center());
    // Straight to the deepest zoom in one frame, so the time below is from the
    // zoom change itself.
    step(&mut app, vec![pointer.clone(), egui::Event::Zoom(64.0)]);
    let zoomed = Instant::now();
    let mut sharp = None;
    while sharp.is_none() && zoomed.elapsed() < Duration::from_secs(20) {
        let (_, arrived) = step(&mut app, vec![pointer.clone()]);
        if let Some(size) = is_detail(&arrived) {
            sharp = Some((zoomed.elapsed(), size));
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    let Some((settle, [w, h])) = sharp else {
        println!("page {}: no sharp render arrived within 20 s of zooming in", shown + 1);
        return Ok(());
    };

    let mut frames = Vec::new();
    let mut details = 0;
    for _ in 0..240 {
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-20.0, -30.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        };
        let (time, arrived) = step(&mut app, vec![pointer.clone(), wheel]);
        frames.push(time);
        details += usize::from(is_detail(&arrived).is_some());
        std::thread::sleep(Duration::from_millis(16));
    }
    print!("page {}: ", shown + 1);
    println!(
        "sharp {} after zooming to 800% ({w} x {h} px); scrolling 240 frames: median {}, worst {}, {details} sharp renders",
        ms(settle),
        ms(median(frames.clone())),
        ms(frames.iter().copied().max().unwrap_or_default()),
    );
    let _ = std::io::stdout().flush();
    Ok(())
}

/* ------------------------------------------------------------------ *
 * The analysis
 * ------------------------------------------------------------------ */

struct Floors {
    name: String,
    pages: usize,
    first_page_now: Duration,
    first_page_floor: Duration,
    new_page_now: Duration,
    new_page_floor: Duration,
    deep_zoom_tile: Option<Duration>,
    search_now: Duration,
    search_parallel_floor: Duration,
    search_repeat_floor: Duration,
    /// The app's own search through its worker: first time, and again.
    worker_search: Option<(Duration, Duration)>,
    save_floor: Duration,
    parallel_speedup: f64,
    parallel_processes: usize,
}

fn analyse(paths: &[PathBuf]) -> Result<(), String> {
    let mut report = String::new();
    let now = chrono::Local::now();
    out!(report, "# PDF Annotate: theoretical fastest times");
    out!(report);
    out!(report, "- Run: {}", now.format("%Y-%m-%d %H:%M"));
    out!(report, "- Logical CPUs: {}", std::thread::available_parallelism().map_or(0, |n| n.get()));
    out!(report, "- View assumed: {VIEW_W} x {VIEW_H} device pixels (a laptop window at 200% scaling)");

    /* ---------- Hardware ---------- */

    out!(report);
    out!(report, "## Hardware limits");
    out!(report);
    let buffer = vec![0x7fu8; 8192 * 2048 * 4];
    drop(buffer.clone());
    let copies: Vec<Duration> = (0..5).map(|_| timed(|| drop(buffer.clone())).1).collect();
    let copy = median(copies);
    let copy_rate = buffer.len() as f64 / MB / copy.as_secs_f64();
    let conversions: Vec<Duration> = (0..3).map(|_| timed(|| drop(ColorImage::from_rgba_premultiplied([8192, 2048], &buffer))).1).collect();
    let convert = median(conversions);
    let convert_rate = buffer.len() as f64 / MB / convert.as_secs_f64();
    out!(report, "| Operation on 64 MB of pixels | Time | Throughput |");
    out!(report, "| --- | ---: | ---: |");
    out!(report, "| Plain memory copy | {} | {copy_rate:.0} MB/s |", ms(copy));
    out!(report, "| Bytes to egui colours (making a texture's image) | {} | {convert_rate:.0} MB/s |", ms(convert));
    out!(report);
    out!(report, "Every pass over a page's pixels -- copying it out of pdfium, making the texture, uploading it to the GPU -- is bounded by roughly the memory copy rate. Uploading goes through the graphics driver, typically at a similar rate.");
    drop(buffer);

    /* ---------- Startup ---------- */

    out!(report);
    out!(report, "## Startup");
    out!(report);
    let scratch = std::env::temp_dir().join(format!("pdf-annotate-floors-{}", std::process::id()));
    let (library, unpack) = timed(|| worker::unpack_pdfium_to(&scratch));
    let library = library.map_err(|e| e.to_string())?;
    let (_, check) = timed(|| worker::unpack_pdfium_to(&scratch));
    let (bindings, load) = timed(|| Pdfium::bind_to_library(&library));
    let bindings = bindings.map_err(err)?;
    let (pdfium, init) = timed(|| Pdfium::new(bindings));
    out!(report, "| Step | Now | Floor | Why |");
    out!(report, "| --- | ---: | ---: | --- |");
    out!(report, "| Check the unpacked pdfium.dll | {} | ~0 | Hash it at build time, not every launch |", ms(check));
    out!(
        report,
        "| Load pdfium.dll | {} | {} | Windows loading a 7 MB DLL. This copy was just written, so antivirus scans it on first load; the app's long-lived copy loads faster (about 10 ms in the benchmark) |",
        ms(load),
        ms(load)
    );
    out!(report, "| Initialise pdfium | {} | {} | pdfium's own setup |", ms(init), ms(init));
    out!(report, "| First launch only: write the DLL out | {} | {} | Disk write of 7 MB |", ms(unpack), ms(unpack));
    let _ = std::fs::remove_dir_all(&scratch);

    let egui_ctx = egui::Context::default();
    let mut floors = Vec::new();
    for path in paths {
        floors.push(analyse_document(&pdfium, path, &egui_ctx, copy_rate, &mut report)?);
    }

    /* ---------- Summary ---------- */

    out!(report);
    out!(report, "## Summary: now vs theoretical floor");
    for f in &floors {
        out!(report);
        out!(report, "### {} ({} pages)", f.name, f.pages);
        out!(report);
        out!(report, "| What you feel | Now | Floor | What would get there |");
        out!(report, "| --- | ---: | ---: | --- |");
        out!(report, "| Open to first page drawn | {} | {} | Read each page once; render only the visible part |", ms(f.first_page_now), ms(f.first_page_floor));
        out!(report, "| A new page scrolling into view | {} | {} | Load it once, not three times; render only the visible part |", ms(f.new_page_now), ms(f.new_page_floor));
        match f.deep_zoom_tile {
            Some(t) => out!(report, "| Deep zoom (4x fit), sharp | soft (capped) | {} | Render visible tiles at full resolution |", ms(t)),
            None => out!(report, "| Deep zoom (4x fit), sharp | soft (capped) | not measured | Region rendering was unavailable |"),
        }
        let (first_now, repeat_now) =
            f.worker_search.map_or_else(|| (ms(f.search_now), ms(f.search_now)), |(first, repeat)| (ms(first), ms(repeat)));
        out!(
            report,
            "| First search of the whole document | {first_now} | {} | Keep pages loaded; extract text in {} processes |",
            ms(f.search_parallel_floor),
            f.parallel_processes
        );
        out!(
            report,
            "| Any later search | {repeat_now} | {} | Page text is cached after the first search; the rest is sharing time with other work |",
            ms(f.search_repeat_floor)
        );
        out!(report, "| Save (whole file rewritten) | see bench | {} | pdfium writing the unchanged document |", ms(f.save_floor));
        out!(report, "| Save (incremental) | see bench | ~1 ms | Append only the changed objects |");
        out!(report, "| Pages rendered per second, flat out | 1x | {:.1}x | {} render processes |", f.parallel_speedup, f.parallel_processes);
    }

    let results = Path::new(env!("CARGO_MANIFEST_DIR")).join("bench-results");
    std::fs::create_dir_all(&results).map_err(|e| e.to_string())?;
    let file = results.join(format!("{}-floors.md", now.format("%Y-%m-%d-%H%M")));
    std::fs::write(&file, &report).map_err(|e| e.to_string())?;
    println!("\nSaved to {}", file.display());
    Ok(())
}

fn analyse_document(
    pdfium: &Pdfium,
    path: &Path,
    egui_ctx: &egui::Context,
    copy_rate: f64,
    report: &mut String,
) -> Result<Floors, String> {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    out!(report);
    out!(report, "## {name}");

    /* ---------- Opening ---------- */

    let (bytes, read) = timed(|| std::fs::read(path));
    let bytes = bytes.map_err(|e| e.to_string())?;
    let file_mb = bytes.len() as f64 / MB;
    let (copied, copy_bytes) = timed(|| bytes.clone());
    let (doc, parse) = timed(|| pdfium.load_pdf_from_byte_vec(copied, None));
    let doc = doc.map_err(err)?;
    let (sizes, page_sizes) = timed(|| annots::page_sizes(&doc));
    let n = sizes.len();
    let usual = usual_size(&sizes);
    let fit = (VIEW_W as f32 - FIT_MARGINS) / usual[0];

    out!(report);
    out!(report, "{n} pages, {file_mb:.1} MB. Fit width on this view renders the usual page at {fit:.2} pixels per point.");
    out!(report);
    out!(report, "### Opening the file");
    out!(report);
    out!(report, "| Step | Now | Floor | Why |");
    out!(report, "| --- | ---: | ---: | --- |");
    out!(report, "| Read the file | {} | {} | Already in memory from the file cache; a cold disk read is slower |", ms(read), ms(read));
    out!(report, "| Copy it for pdfium | {} | ~0 | Let pdfium read the file directly |", ms(copy_bytes));
    out!(report, "| Parse the document | {} | {} | pdfium reading the cross-reference table |", ms(parse), ms(parse));
    out!(report, "| Every page's size | {} | {} | One pdfium call per page |", ms(page_sizes), ms(page_sizes));

    /* ---------- Per page ---------- */

    let sample = spread(n, 12);
    let mut load_cold = Vec::new();
    let mut load_warm = Vec::new();
    let mut render_first = Vec::new();
    let mut render_again = Vec::new();
    let mut pixels_out = Vec::new();
    let mut texture = Vec::new();
    let mut view_render = Vec::new();
    let mut deep_tile = Vec::new();
    let mut region_error = None;
    let mut text_load = Vec::new();
    let mut text_chars = Vec::new();
    let mut annotations = Vec::new();
    let mut megapixels = Vec::new();

    for &p in &sample {
        let (page, t) = timed(|| doc.pages().get(p as PdfPageIndex));
        drop(page.map_err(err)?);
        load_cold.push(t);
        let (page, t) = timed(|| doc.pages().get(p as PdfPageIndex));
        let page = page.map_err(err)?;
        load_warm.push(t);

        let config = PdfRenderConfig::new().scale_page_by_factor(fit).render_annotations(true).render_form_data(true);
        let (bitmap, t) = timed(|| page.render_with_config(&config));
        let bitmap = bitmap.map_err(err)?;
        render_first.push(t);
        let (w, h) = (bitmap.width() as usize, bitmap.height() as usize);
        megapixels.push((w * h) as f64 / 1e6);
        let (rgba, t) = timed(|| bitmap.as_rgba_bytes());
        pixels_out.push(t);
        drop(bitmap);
        let (again, t) = timed(|| page.render_with_config(&config));
        drop(again.map_err(err)?);
        render_again.push(t);
        texture.push(timed(|| build_texture(egui_ctx, [w, h], &rgba)).1);
        drop(rgba);

        // Only the part of the page the view shows, at the same scale.
        match PdfBitmap::empty(VIEW_W, VIEW_H, PdfBitmapFormat::default()) {
            Ok(mut view) => {
                let origin_y = -((h as i32 - VIEW_H).max(0) / 2);
                let region = PdfRenderConfig::new().scale_page_by_factor(fit).render_annotations(true).set_origin(0, origin_y);
                match timed(|| page.render_into_bitmap_with_config(&mut view, &region)) {
                    (Ok(()), t) => view_render.push(t),
                    (Err(e), _) => region_error = Some(e.to_string()),
                }
                // A view-sized tile from the middle of the page at four times fit.
                let deep = fit * 4.0;
                let (dw, dh) = ((w as f32 * 4.0) as i32, (h as f32 * 4.0) as i32);
                let tile = PdfRenderConfig::new()
                    .scale_page_by_factor(deep)
                    .render_annotations(true)
                    .set_origin(-(dw - VIEW_W) / 2, -(dh - VIEW_H) / 2);
                match timed(|| page.render_into_bitmap_with_config(&mut view, &tile)) {
                    (Ok(()), t) => deep_tile.push(t),
                    (Err(e), _) => region_error = Some(e.to_string()),
                }
            }
            Err(e) => region_error = Some(e.to_string()),
        }

        {
            let (text, t) = timed(|| page.text());
            let text = text.map_err(err)?;
            text_load.push(t);
            let (_, t) = timed(|| {
                let chars = text.chars();
                chars
                    .iter()
                    .map(|c| {
                        let _ = c.unicode_char();
                        let _ = c.loose_bounds();
                    })
                    .count()
            });
            text_chars.push(t);
        }

        let (_, t) = timed(|| {
            let annots = page.annotations();
            (0..annots.len()).filter(|&i| annots.get(i).is_ok_and(|a| a.as_highlight_annotation().is_some())).count()
        });
        annotations.push(t);
    }

    let mp = megapixels.iter().sum::<f64>() / megapixels.len().max(1) as f64;
    let view_mp = (VIEW_W * VIEW_H) as f64 / 1e6;
    let render_ns_per_mp = median(render_again.clone()).as_secs_f64() * 1e9 / mp.max(0.01);

    out!(report);
    out!(report, "### One page (median of {} pages)", sample.len());
    out!(report);
    out!(report, "Rendered at fit width the page is {mp:.1} MP; the view is {view_mp:.1} MP.");
    out!(report);
    out!(report, "| Piece | Time | Notes |");
    out!(report, "| --- | ---: | --- |");
    out!(report, "| Load the page, first time | {} | pdfium parses the page's content stream |", ms(median(load_cold.clone())));
    out!(report, "| Load it again | {} | pdfium parses it all again: nothing is cached between loads |", ms(median(load_warm.clone())));
    out!(report, "| Render the whole page, first time | {} | Includes building font and image caches |", ms(median(render_first.clone())));
    out!(report, "| Render it again (same loaded page) | {} | Pure rasterising: {:.0} ms per megapixel |", ms(median(render_again.clone())), render_ns_per_mp / 1e6);
    out!(report, "| Copy the pixels out of pdfium | {} | Bounded by memory copy speed |", ms(median(pixels_out.clone())));
    out!(report, "| Make the texture | {} | Bounded by memory copy speed |", ms(median(texture.clone())));
    let upload_estimate = Duration::from_secs_f64(mp * 1e6 * 4.0 / MB / copy_rate);
    out!(report, "| Upload the texture to the GPU (estimate) | {} | At the memory copy rate; not measurable without a window |", ms(upload_estimate));
    if view_render.is_empty() {
        out!(report, "| Render only the visible part | not measured | {} |", region_error.clone().unwrap_or_default());
    } else {
        out!(report, "| Render only the visible part | {} | {view_mp:.1} MP instead of {mp:.1} MP |", ms(median(view_render.clone())));
    }
    if !deep_tile.is_empty() {
        out!(report, "| Render a view-sized tile at 4x fit | {} | What sharp deep zoom would cost per screenful |", ms(median(deep_tile.clone())));
    }
    out!(report, "| Load the page's text | {} | pdfium's text analysis |", ms(median(text_load.clone())));
    out!(report, "| Read every character and its box | {} | Two pdfium calls per character; a bulk read would be less |", ms(median(text_chars.clone())));
    out!(report, "| Read the page's highlights (page already loaded) | {} | |", ms(median(annotations.clone())));

    /* ---------- Search ---------- */

    let (all_chars, extract_all) = timed(|| (0..n).map(|p| annots::page_chars(&doc, p).unwrap_or_default()).collect::<Vec<Vec<TextChar>>>());
    let mut words: HashMap<String, usize> = HashMap::new();
    for chars in all_chars.iter().take(30) {
        let s: String = chars.iter().map(|c| c.ch).collect();
        for w in s.split(|c: char| !c.is_alphabetic()) {
            if w.chars().count() >= 4 {
                *words.entry(w.to_lowercase()).or_default() += 1;
            }
        }
    }
    let common = words.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map_or_else(|| "the".to_owned(), |(w, _)| w.clone());
    let (hits, find_cached) = timed(|| {
        all_chars
            .iter()
            .map(|chars| {
                selection::find(chars, &common)
                    .into_iter()
                    .map(|r| {
                        let _ = selection::bands(chars, r.clone());
                        let _ = selection::context(chars, r, 40, 120);
                    })
                    .count()
            })
            .sum::<usize>()
    });

    out!(report);
    out!(report, "### Search for \"{common}\" ({hits} matches)");
    out!(report);
    out!(report, "| Piece | Time |");
    out!(report, "| --- | ---: |");
    out!(report, "| Extract every page's text, as the app does now (loads each page) | {} |", ms(extract_all));
    out!(report, "| Find the matches in text already in memory | {} |", ms(find_cached));
    let worker_search = std::env::current_exe().ok().and_then(|exe| {
        let output = Command::new(exe)
            .args(["search-worker", &path.display().to_string(), &common])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let mut parts = text.split_whitespace().map(|v| v.parse::<f64>().ok());
        let first = parts.next()??;
        let repeat = parts.next()??;
        Some((Duration::from_secs_f64(first / 1000.0), Duration::from_secs_f64(repeat / 1000.0)))
    });
    if let Some((first, repeat)) = worker_search {
        out!(report, "| The app's own search, first time | {} |", ms(first));
        out!(report, "| The app's own search, again (page text cached) | {} |", ms(repeat));
    }

    /* ---------- Save ---------- */

    let (saved, serialise) = timed(|| doc.save_to_bytes());
    let saved_mb = saved.map(|b| b.len() as f64 / MB).unwrap_or(0.0);
    out!(report);
    out!(report, "### Saving");
    out!(report);
    out!(report, "pdfium writing the whole unchanged document ({saved_mb:.1} MB): {}. A full save can't be faster than this plus the disk write; an incremental save only writes what changed.", ms(serialise));

    /* ---------- Parallel rendering ---------- */

    out!(report);
    out!(report, "### Rendering in parallel processes");
    out!(report);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let cores = std::thread::available_parallelism().map_or(1, |c| c.get());
    let mut counts: Vec<usize> = vec![1, 2, 4, 8, cores];
    counts.retain(|c| *c <= cores);
    counts.dedup();
    out!(report, "Each process renders every page once at fit width, all running at the same time.");
    out!(report);
    out!(report, "| Processes | Wall time | Pages per second | Speed-up |");
    out!(report, "| ---: | ---: | ---: | ---: |");
    let mut single = 0.0;
    let mut best = (1.0, 1);
    for &count in &counts {
        let start = Instant::now();
        let children: Vec<_> = (0..count)
            .map(|_| {
                Command::new(&exe)
                    .args(["render-all", &path.display().to_string(), &fit.to_string()])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
            })
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        for child in children {
            child.wait_with_output().map_err(|e| e.to_string())?;
        }
        let wall = start.elapsed().as_secs_f64();
        let rate = (count * n) as f64 / wall;
        if count == 1 {
            single = rate;
        }
        let speedup = rate / single.max(1e-9);
        if speedup > best.0 {
            best = (speedup, count);
        }
        out!(report, "| {count} | {:.2} s | {rate:.1} | {speedup:.1}x |", wall);
    }
    out!(report);
    out!(report, "Process start-up (binding pdfium, parsing the file) is included, which is why one process is slower than the per-page numbers above suggest.");

    /* ---------- UI frames ---------- */

    out!(report);
    out!(report, "### The UI thread, running the real app headless");
    out!(report);
    out!(report, "| Frames | Layout median | Layout p95 | Tessellate median | Frame median | Frame worst | Textures arriving |");
    out!(report, "| --- | ---: | ---: | ---: | ---: | ---: | --- |");
    let output = Command::new(&exe)
        .args(["frames", &path.display().to_string()])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    for line in String::from_utf8_lossy(&output.stdout).lines().filter(|l| l.starts_with('|')) {
        out!(report, "{line}");
    }
    out!(report);
    out!(report, "A 60 Hz display allows 16.7 ms per frame for everything, including the GPU drawing it. Upload of arriving textures happens on top of these times, in the frame that shows them.");

    /* ---------- Floors ---------- */

    let load = median(load_cold.clone());
    let texture_per_mp = median(texture.clone()).as_secs_f64() / mp.max(0.01);
    let view_part_mp = view_mp.min(mp);
    let visible_render = view_render.first().map(|_| median(view_render.clone())).unwrap_or_else(|| Duration::from_secs_f64(render_ns_per_mp * view_part_mp / 1e9));
    let visible_texture = Duration::from_secs_f64(texture_per_mp * view_part_mp);
    let visible_upload = Duration::from_secs_f64(view_part_mp * 1e6 * 4.0 / MB / copy_rate);

    let first_page_now = read + copy_bytes + parse + page_sizes + load + median(annotations.clone()) + median(render_first.clone()) + median(pixels_out.clone()) + median(texture.clone()) + upload_estimate;
    let first_page_floor = read + parse + page_sizes + load + visible_render + visible_texture + visible_upload;
    // Today a page that scrolls into view is loaded three times: to read its
    // highlights, its text and its pixels.
    let new_page_now = load * 3 + median(annotations) + median(text_load.clone()) + median(text_chars.clone()) + median(render_first) + median(pixels_out) + median(texture) + upload_estimate;
    let new_page_floor = load + visible_render + visible_texture + visible_upload;

    Ok(Floors {
        name,
        pages: n,
        first_page_now,
        first_page_floor,
        new_page_now,
        new_page_floor,
        deep_zoom_tile: deep_tile.first().map(|_| median(deep_tile.clone())),
        search_now: extract_all + find_cached,
        // From the measured whole-document extraction, not a median page times
        // the page count: pages vary a lot in drawing sets.
        search_parallel_floor: Duration::from_secs_f64(extract_all.as_secs_f64() / best.0) + find_cached,
        search_repeat_floor: find_cached,
        worker_search,
        save_floor: serialise,
        parallel_speedup: best.0,
        parallel_processes: best.1,
    })
}
