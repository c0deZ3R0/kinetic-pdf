//! Benchmarks the work behind what you feel in the app -- starting up, opening
//! a file, drawing pages, extracting text, searching and saving -- by running
//! the app's own engine code (the same functions its worker thread calls).
//!
//!     cargo run --release --bin bench -- [--label NAME] [--pages N] [--runs N] [file.pdf ...]
//!
//! With no PDF given it generates a long, text-heavy document with highlights
//! on it, and reuses it on later runs so numbers stay comparable. PDFs you pass
//! are measured as they are, plus a copy repeated up to `--pages` long.
//!
//! Results print as tables and are saved to bench-results/ as Markdown, so a
//! run before a change can be compared with one after.
//!
//! Not measured: the UI thread's frame time, which needs a real window. Making
//! the texture for a page is measured, and now happens on the worker thread;
//! the upload of that texture to the GPU still happens on the UI thread, and
//! isn't measured.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, TextureOptions};
use pdfium_render::prelude::*;

use kinetic_pdf::annots;
use kinetic_pdf::model::{Changes, NewHighlight};
use kinetic_pdf::selection;
use kinetic_pdf::worker::{self, MAX_SEARCH_HITS};

mod test_pdfs;

const USAGE: &str = "usage: bench [--label NAME] [--pages N] [--runs N] [--repeats N] [--renderers-only] [--only NAME] [--compare-shots] [--test-pdfs] [file.pdf ...]";

/// Render scales the app really uses, in output pixels per PDF point: page
/// zoom times display scaling (which the app caps at 2).
const SCALES: [(&str, f32); 3] = [
    ("125% zoom, 100% display", 1.25),
    ("125% zoom, 200% display", 2.5),
    ("300% zoom, 200% display", 6.0),
];

/// The app keeps textures for the pages in view plus four either side.
const PAGES_KEPT: usize = 9;

/// Planted every 50 pages in the generated document, for the rare-word search.
const RARE_WORD: &str = "zephyrine";

const WORDS: &[&str] = &[
    "the", "of", "and", "a", "to", "in", "is", "that", "for", "it", "as", "was", "with", "be", "by", "on", "not",
    "this", "are", "or", "from", "at", "which", "but", "have", "an", "they", "were", "there", "one", "all", "their",
    "document", "annotation", "highlight", "page", "reader", "margin", "paragraph", "section", "figure", "table",
    "result", "method", "analysis", "evidence", "history", "system", "between", "however", "because", "through",
    "during", "without", "against", "information", "development", "important", "different", "following",
    "available", "particular", "research", "measure", "process", "structure", "question", "example",
];

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

struct Args {
    label: String,
    pages: usize,
    runs: usize,
    /// Build the test drawing sets and measure those; see test_pdfs.rs.
    test_pdfs: bool,
    /// Runs a renderer for each benchmark that launches the app.
    repeats: usize,
    /// Only the benchmarks that set our renderer against pdfium.
    renderers_only: bool,
    /// Only documents whose name contains this.
    only: Option<String>,
    pdfs: Vec<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args { label: "run".to_owned(), pages: 300, runs: 3, test_pdfs: false, repeats: 5, renderers_only: false, only: None, pdfs: Vec::new() };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--label" => args.label = it.next().unwrap_or_else(|| exit_usage()),
            "--pages" => args.pages = it.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| exit_usage()),
            "--runs" => args.runs = it.next().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or_else(|| exit_usage()),
            "--test-pdfs" | "--make-test-pdfs" => args.test_pdfs = true,
            "--repeats" => args.repeats = it.next().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or_else(|| exit_usage()),
            "--renderers-only" => args.renderers_only = true,
            "--only" => args.only = Some(it.next().unwrap_or_else(|| exit_usage())),
            "--compare-shots" => {
                let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("bench-results").join("shots");
                let docs: Vec<_> = std::fs::read_dir(&root).into_iter().flatten().flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), PathBuf::new(), None)).collect();
                compare_shots(&docs, &root, &mut String::new());
                std::process::exit(0);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => args.pdfs.push(PathBuf::from(arg)),
        }
    }
    args
}

fn exit_usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

/* ------------------------------------------------------------------ *
 * Measuring
 * ------------------------------------------------------------------ */

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed())
}

#[derive(Default)]
struct Samples(Vec<Duration>);

impl Samples {
    fn push(&mut self, d: Duration) {
        self.0.push(d);
    }

    fn sorted(&self) -> Vec<Duration> {
        let mut v = self.0.clone();
        v.sort();
        v
    }

    fn first(&self) -> Duration {
        self.0.first().copied().unwrap_or_default()
    }

    fn min(&self) -> Duration {
        self.sorted().first().copied().unwrap_or_default()
    }

    fn median(&self) -> Duration {
        let v = self.sorted();
        v.get(v.len() / 2).copied().unwrap_or_default()
    }

    fn p95(&self) -> Duration {
        let v = self.sorted();
        if v.is_empty() {
            return Duration::ZERO;
        }
        let i = ((v.len() as f64 * 0.95).ceil() as usize).saturating_sub(1).min(v.len() - 1);
        v[i]
    }
}

fn ms(d: Duration) -> String {
    let v = d.as_secs_f64() * 1000.0;
    if v >= 100.0 {
        format!("{v:.0} ms")
    } else if v >= 10.0 {
        format!("{v:.1} ms")
    } else {
        format!("{v:.2} ms")
    }
}

const MB: f64 = 1024.0 * 1024.0;

/// This process's working set now and at its peak, in MB.
fn memory() -> (f64, f64) {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    if ok == 0 {
        return (0.0, 0.0);
    }
    (counters.WorkingSetSize as f64 / MB, counters.PeakWorkingSetSize as f64 / MB)
}

fn memory_line(report: &mut String) {
    let (now, peak) = memory();
    out!(report, "");
    out!(report, "Memory: {now:.0} MB now, {peak:.0} MB peak so far.");
}

fn err(e: PdfiumError) -> String {
    e.to_string()
}

/// What the worker does with a rendered page before handing it over: make its
/// texture. On a context with no window, the image is then discarded rather
/// than uploaded.
fn build_texture(ctx: &egui::Context, size: [usize; 2], rgba: &[u8]) {
    let image = ColorImage::from_rgba_premultiplied(size, rgba);
    drop(ctx.load_texture("bench", image, TextureOptions::LINEAR));
    // egui checks, in debug builds, that texture changes are dealt with rather
    // than dropped; this one is deliberately discarded.
    ctx.tex_manager().write().take_delta().clear();
}

/// The render scale the app would really use for this page: the requested one,
/// capped for very large pages exactly as the app caps it (assuming a GPU that
/// takes 16384-pixel textures).
fn capped_scale(sizes: &[[f32; 2]], page: usize, scale: f32) -> f32 {
    let [w, h] = sizes[page];
    kinetic_pdf::app::render_scale(eframe::egui::vec2(w, h), scale, 1.0, 16384.0)
}

/// Up to `k` page indices spread evenly through a document.
fn spread(n: usize, k: usize) -> Vec<usize> {
    if n <= k {
        (0..n).collect()
    } else {
        (0..k).map(|i| i * n / k).collect()
    }
}

/* ------------------------------------------------------------------ *
 * Test documents
 * ------------------------------------------------------------------ */

/// A deterministic pseudo-random sequence (xorshift), so every run generates
/// the same document.
fn next_random(state: &mut u64) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 11) as usize
}

/// Pages of dense body text, like a report or a book.
fn generate_document(pdfium: &Pdfium, pages: usize, path: &Path) -> Result<(), String> {
    let mut doc = pdfium.create_new_pdf().map_err(err)?;
    let font = doc.fonts_mut().helvetica();
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;

    for p in 0..pages {
        let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::a4()).map_err(err)?;
        page.objects_mut()
            .create_text_object(PdfPoints::new(56.0), PdfPoints::new(790.0), format!("Section {}", p + 1), font, PdfPoints::new(16.0))
            .map_err(err)?;
        for line in 0..58 {
            let mut text = String::new();
            while text.len() < 95 {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(WORDS[next_random(&mut seed) % WORDS.len()]);
            }
            if p % 50 == 7 && line == 20 {
                text.push(' ');
                text.push_str(RARE_WORD);
            }
            let y = 760.0 - line as f32 * 12.5;
            page.objects_mut()
                .create_text_object(PdfPoints::new(56.0), PdfPoints::new(y), text, font, PdfPoints::new(10.0))
                .map_err(err)?;
        }
    }
    doc.save_to_file(path).map_err(err)
}

/// A highlight with a note on every `every`th page, written the way the app
/// saves them.
fn add_highlights(pdfium: &Pdfium, path: &Path, every: usize) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let changes = {
        let doc = pdfium.load_pdf_from_byte_slice(&bytes, None).map_err(err)?;
        let n = doc.pages().len() as usize;
        let mut adds = Vec::new();
        for p in (0..n).step_by(every.max(1)) {
            let chars = annots::page_chars(&doc, p)?;
            let range = 40.min(chars.len())..160.min(chars.len());
            let quads = selection::bands(&chars, range);
            if !quads.is_empty() {
                adds.push(NewHighlight { page: p, quads, color: [1.0, 0.93, 0.25], comment: format!("A note on page {}", p + 1) });
            }
        }
        Changes { adds, author: "bench".to_owned(), ..Changes::default() }
    };
    let out = annots::save(pdfium, &bytes, &changes)?.bytes;
    std::fs::write(path, out).map_err(|e| e.to_string())
}

/// A long document made by repeating another's pages.
fn repeat_pages(pdfium: &Pdfium, source: &Path, target: usize, path: &Path) -> Result<usize, String> {
    let bytes = std::fs::read(source).map_err(|e| e.to_string())?;
    let source = pdfium.load_pdf_from_byte_vec(bytes, None).map_err(err)?;
    if source.pages().len() == 0 {
        return Err("the PDF has no pages".to_owned());
    }
    let mut long = pdfium.create_new_pdf().map_err(err)?;
    while (long.pages().len() as usize) < target {
        long.pages_mut().append(&source).map_err(err)?;
    }
    long.save_to_file(path).map_err(err)?;
    Ok(long.pages().len() as usize)
}

fn page_count(pdfium: &Pdfium, path: &Path) -> Result<usize, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let doc = pdfium.load_pdf_from_byte_vec(bytes, None).map_err(err)?;
    Ok(doc.pages().len() as usize)
}

/* ------------------------------------------------------------------ *
 * The benchmark
 * ------------------------------------------------------------------ */

fn main() {
    let args = parse_args();
    if let Err(e) = run(&args) {
        eprintln!("\nbenchmark failed: {e}");
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<(), String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or("could not locate the benchmark executable")?;
    let data_dir = exe_dir.join("bench-data");
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;

    let mut report = String::new();
    let mut summary: Vec<String> = Vec::new();
    let now = chrono::Local::now();

    out!(report, "# Kinetic PDF benchmark: {}", args.label);
    out!(report);
    out!(report, "- Run: {}", now.format("%Y-%m-%d %H:%M"));
    let build = if cfg!(debug_assertions) { "debug (numbers not representative; use --release)" } else { "release" };
    out!(report, "- Build: {build}");
    let cpus = std::thread::available_parallelism().map_or(0, |n| n.get());
    out!(report, "- Logical CPUs: {cpus}");
    match std::fs::metadata(exe_dir.join("kinetic-pdf.exe")) {
        Ok(m) => out!(report, "- App exe: {:.1} MB", m.len() as f64 / MB),
        Err(_) => out!(report, "- App exe: not built yet (cargo build --release)"),
    }
    out!(report, "- Open runs per document: {}", args.runs);

    /* ---------------- Startup ---------------- */

    out!(report);
    out!(report, "## Startup");
    out!(report);

    // A folder of our own, so the first-launch unpack really is a first
    // launch. Clear out copies from earlier runs; ones still in use by a
    // running benchmark just stay.
    if let Ok(entries) = std::fs::read_dir(&data_dir) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("pdfium-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let unpack_dir = data_dir.join(format!("pdfium-{}", std::process::id()));
    let (library, cold) = timed(|| worker::unpack_pdfium_to(&unpack_dir));
    let library = library.map_err(|e| e.to_string())?;
    let mut warm = Samples::default();
    for _ in 0..5 {
        let (result, t) = timed(|| worker::unpack_pdfium_to(&unpack_dir));
        result.map_err(|e| e.to_string())?;
        warm.push(t);
    }
    let (bindings, load) = timed(|| Pdfium::bind_to_library(&library));
    let bindings = bindings.map_err(err)?;
    let (pdfium, init) = timed(|| Pdfium::new(bindings));

    out!(report, "| Step | Time |");
    out!(report, "| --- | ---: |");
    out!(report, "| First launch: unpack pdfium.dll (hash and write) | {} |", ms(cold));
    out!(report, "| Later launches: check the unpacked copy (hash and size) | {} |", ms(warm.median()));
    out!(report, "| Load pdfium.dll | {} |", ms(load));
    out!(report, "| Initialise pdfium | {} |", ms(init));
    out!(report, "| **Later launch, pdfium ready** | **{}** |", ms(warm.median() + load + init));
    out!(report);
    out!(report, "Window creation and the first egui frame come on top and aren't measured here.");
    summary.push(format!("Startup to pdfium ready (later launch): {}", ms(warm.median() + load + init)));
    memory_line(&mut report);

    /* ---------------- Documents ---------------- */

    let mut docs: Vec<(String, PathBuf, Option<String>)> = Vec::new();
    if args.test_pdfs {
        // Built once and kept. Each set is uniform, so what a run measures is
        // the renderer rather than which sheets it happened to visit.
        let dir = data_dir.join("test-pdfs");
        println!("\nBuilding the test drawing sets in {} (once; later runs reuse them)...", dir.display());
        for (name, path) in test_pdfs::write_all(&dir)? {
            let mb = std::fs::metadata(&path).map(|m| m.len() as f64 / MB).unwrap_or(0.0);
            println!("  {name}: {mb:.1} MB");
            docs.push((name, path, None));
        }
    }
    if args.pdfs.is_empty() && !args.test_pdfs {
        let path = data_dir.join(format!("synthetic-{}p-v1.pdf", args.pages));
        if !path.exists() {
            println!("\nGenerating a {}-page test document (once; later runs reuse it)...", args.pages);
            generate_document(&pdfium, args.pages, &path)?;
            add_highlights(&pdfium, &path, 5)?;
        }
        docs.push((
            format!("Generated text document, {} pages, a highlight on every 5th page", args.pages),
            path,
            Some(RARE_WORD.to_owned()),
        ));
    }
    for pdf in &args.pdfs {
        let name = pdf.file_name().map_or_else(|| pdf.display().to_string(), |n| n.to_string_lossy().into_owned());
        let pages = page_count(&pdfium, pdf)?;
        docs.push((format!("{name}, {pages} pages"), pdf.clone(), None));
        if pages < args.pages && !args.renderers_only {
            let stem = pdf.file_stem().map_or_else(|| "document".to_owned(), |s| s.to_string_lossy().into_owned());
            let path = data_dir.join(format!("repeated-{stem}-{}p.pdf", args.pages));
            let long_pages = repeat_pages(&pdfium, pdf, args.pages, &path)?;
            docs.push((format!("{name} repeated to {long_pages} pages"), path, None));
        }
    }

    if let Some(only) = &args.only {
        docs.retain(|(name, _, _)| name.contains(only.as_str()));
    }
    if !args.renderers_only {
        for (name, path, rare) in &docs {
            bench_document(&pdfium, args, name, path, rare.as_deref(), &data_dir, &mut report, &mut summary)?;
        }
        bench_zoom(args, &docs, &exe_dir, &data_dir, &mut report, &mut summary);
    }

    /* ---------------- Our renderer against pdfium ---------------- */

    bench_renderers(args, &docs, &exe_dir, &data_dir, &mut report, &mut summary);
    bench_working(args, &docs, &exe_dir, &data_dir, &mut report, &mut summary);

    /* ---------------- Summary ---------------- */

    out!(report);
    out!(report, "## Summary");
    out!(report);
    for line in &summary {
        out!(report, "- {line}");
    }
    let (_, peak) = memory();
    out!(report, "- Peak memory over the whole run: {peak:.0} MB");

    let results = Path::new(env!("CARGO_MANIFEST_DIR")).join("bench-results");
    std::fs::create_dir_all(&results).map_err(|e| e.to_string())?;
    let file = results.join(format!("{}-{}.md", now.format("%Y-%m-%d-%H%M"), args.label));
    std::fs::write(&file, &report).map_err(|e| e.to_string())?;
    println!("\nSaved to {}", file.display());
    Ok(())
}

/// Zooming from the smallest zoom straight to the deepest, and how long until
/// everything in view is sharp.
///
/// This one runs the app itself, in a window of its own
/// (`KINETIC_PDF_ZOOM_BENCH`, see app/zoom_bench.rs): what it measures --
/// pages read into shapes, drawn ahead of the zoom, and drawn by the GPU --
/// only happens with a real window and a real graphics context, so there is
/// nothing here for the engine code to answer on its own.
fn bench_zoom(args: &Args, docs: &[(String, PathBuf, Option<String>)], exe_dir: &Path, data_dir: &Path, report: &mut String, summary: &mut Vec<String>) {
    let app = exe_dir.join("kinetic-pdf.exe");
    if !app.exists() {
        return;
    }
    out!(report);
    out!(report, "## Zooming in");
    out!(report);
    out!(report, "From {}% to {}% in one step, on the last page, after resting {:.0} s so pages can be drawn ahead. The app runs it in a window of its own; a cold cache has nothing kept from before, a warm one has what the cold run left.", (ZOOM_FROM * 100.0) as u32, (ZOOM_TO * 100.0) as u32, ZOOM_REST);
    out!(report);
    out!(report, "| Document | Cold cache | Warm cache | Drawn ahead |");
    out!(report, "| --- | ---: | ---: | ---: |");
    for (name, path, _) in docs {
        let cache = data_dir.join("zoom-cache");
        let _ = std::fs::remove_dir_all(&cache);
        let mut runs = Vec::new();
        let mut ahead = String::from("—");
        for _ in 0..2 {
            match run_zoom_bench(&app, path, &cache) {
                Ok((sharp, drawn_ahead)) => {
                    ahead = drawn_ahead;
                    runs.push(sharp);
                }
                Err(e) => {
                    out!(report, "| {name} | {e} | | |");
                    runs.clear();
                    break;
                }
            }
        }
        if let [cold, warm] = runs.as_slice() {
            out!(report, "| {name} | {cold} | {warm} | {ahead} |");
            summary.push(format!("{name}: zoom to {}% sharp after {warm} (warm cache)", (ZOOM_TO * 100.0) as u32));
        }
    }
    let _ = args;
}

/// The zooms `bench_zoom` reports, which are the zoom benchmark's own start and
/// deepest, and how long it rests before zooming.
const ZOOM_FROM: f32 = 0.1;
const ZOOM_TO: f32 = 8.0;
const ZOOM_REST: f64 = 6.0;

/// Sheets sampled by `bench_renderers`, and the zoom they're shown at.
const SHEETS_SAMPLED: usize = 12;
const SHEET_ZOOM: u32 = 100;

/// What one launch of the app reported: every `key: value` line it printed
/// that starts with the benchmark's prefix.
type Said = HashMap<String, String>;

/// Launches the app on `pdf` with `env` set, a cache folder of its own, and
/// pdfium drawing everything unless `ours`; returns what it reported.
fn run_app(app: &Path, pdf: &Path, cache: &Path, ours: bool, env: &[(&str, String)], prefix: &str) -> Result<Said, String> {
    let _ = std::fs::remove_dir_all(cache);
    let mut command = std::process::Command::new(app);
    command.arg(pdf).env("KINETIC_PDF_CACHE", cache).env("KINETIC_PDF_UPDATE", "0").stdout(std::process::Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    if !ours {
        command.env("KINETIC_PDF_GPU", "0");
    }
    let out = command.output().map_err(|e| format!("could not run the app: {e}"))?;
    let _ = std::fs::remove_dir_all(cache);
    let said: Said = String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once(": ")?;
            let key = key.strip_prefix(prefix)?.strip_prefix('-')?;
            Some((key.to_owned(), value.trim().to_owned()))
        })
        .collect();
    if said.is_empty() {
        return Err("the app didn't report anything".to_owned());
    }
    Ok(said)
}

fn numbers(list: Option<&String>) -> Vec<f64> {
    list.map(|l| l.split(',').filter_map(|n| n.trim().parse().ok()).collect()).unwrap_or_default()
}

fn number(said: &Said, key: &str) -> Option<f64> {
    said.get(key).and_then(|v| v.parse().ok())
}

/// Nearest-rank percentile of unsorted values.
fn percentile(values: &[f64], share: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((sorted.len() as f64 * share).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn ms_or_s(v: f64) -> String {
    if v >= 1000.0 { format!("{:.1} s", v / 1000.0) } else { format!("{v:.0} ms") }
}

/// A document's name made safe for a folder.
fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// Runs one of the app's benchmarks `repeats` times a renderer, plus a pair
/// thrown away first. The two renderers take turns going first, so neither
/// always inherits the other's warm file cache; the pair thrown away warms the
/// file and the driver for both. `shots`, if given, is where the last pair
/// pictures the view.
#[allow(clippy::too_many_arguments)]
fn run_pairs(
    app: &Path,
    pdf: &Path,
    data_dir: &Path,
    env: &[(&str, String)],
    prefix: &str,
    repeats: usize,
    shots: Option<&Path>,
) -> Result<[Vec<Said>; 2], String> {
    let mut kept: [Vec<Said>; 2] = [Vec::new(), Vec::new()];
    for pair in 0..=repeats {
        let order = if pair % 2 == 0 { [false, true] } else { [true, false] };
        for ours in order {
            let who = if ours { "ours" } else { "pdfium" };
            println!("  {prefix} {who}, run {pair} of {repeats}{}", if pair == 0 { " (warm-up, not counted)" } else { "" });
            let mut env = env.to_vec();
            if let (Some(dir), true) = (shots, pair == repeats) {
                env.push(("KINETIC_PDF_SHOTS", dir.join(who).display().to_string()));
            }
            let cache = data_dir.join(format!("{prefix}-cache-{who}"));
            let said = run_app(app, pdf, &cache, ours, &env, prefix)?;
            if pair > 0 {
                kept[usize::from(ours)].push(said);
            }
        }
    }
    Ok(kept)
}

/// Reads a PNG the app saved as 8-bit RGBA: its width, height and pixels.
fn read_png(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    let file = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let mut reader = png::Decoder::new(file).read_info().map_err(|e| e.to_string())?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or("picture too big")?];
    let info = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("not 8-bit RGBA".to_owned());
    }
    pixels.truncate(info.buffer_size());
    Ok((info.width as usize, info.height as usize, pixels))
}

/// How far our picture of the view is from pdfium's: the mean difference a
/// channel, out of 255, and the share of pixels off by more than 64 in some
/// channel -- which antialiasing alone rarely reaches, so it counts pixels
/// drawn differently rather than drawn a shade differently.
fn picture_difference(theirs: &Path, ours: &Path) -> Result<(f64, f64), String> {
    let (tw, th, t) = read_png(theirs)?;
    let (ow, oh, o) = read_png(ours)?;
    let (w, h) = (tw.min(ow), th.min(oh));
    let (mut sum, mut far) = (0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (&t[(y * tw + x) * 4..][..3], &o[(y * ow + x) * 4..][..3]);
            let most = (0..3).map(|c| a[c].abs_diff(b[c])).inspect(|&d| sum += u64::from(d)).max().unwrap_or(0);
            far += u64::from(most > 64);
        }
    }
    let n = (w * h).max(1) as f64;
    Ok((sum as f64 / (3.0 * n), 100.0 * far as f64 / n))
}

/// Sets each renderer's picture of the view beside the other's, as numbers.
fn compare_shots(docs: &[(String, PathBuf, Option<String>)], root: &Path, report: &mut String) {
    out!(report);
    out!(report, "### Our pictures against pdfium's");
    out!(report);
    out!(report, "The view pictured by each renderer once sharp, at the same place and zoom. Mean difference is per channel, out of 255. Pixels drawn differently are those off by more than 64 in some channel, which antialiasing alone rarely reaches.");
    out!(report);
    out!(report, "| Document | Zoom | Mean difference | Pixels drawn differently |");
    out!(report, "| --- | ---: | ---: | ---: |");
    for (name, _, _) in docs {
        for zoom in ["200", "800"] {
            let dir = root.join(slug(name));
            let file = format!("{zoom}.png");
            match picture_difference(&dir.join("pdfium").join(&file), &dir.join("ours").join(&file)) {
                Ok((mean, far)) => out!(report, "| {name} | {zoom}% | {mean:.2} | {far:.2}% |"),
                Err(e) => out!(report, "| {name} | {zoom}% | {e} | |"),
            }
        }
    }
}

/// What the machine is, for the report.
fn describe_machine(report: &mut String, said: &Said) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let cpu = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", "(Get-CimInstance Win32_Processor).Name"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default());
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    let ram = if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 { status.ullTotalPhys as f64 / MB / 1024.0 } else { 0.0 };
    let get = |key: &str| said.get(key).cloned().unwrap_or_else(|| "not reported".to_owned());
    out!(report, "| | |");
    out!(report, "| --- | --- |");
    out!(report, "| Processor | {cpu}, {} logical |", std::thread::available_parallelism().map_or(0, |n| n.get()));
    out!(report, "| Memory | {ram:.0} GB |");
    out!(report, "| Graphics | {} |", get("gl"));
    out!(report, "| Page area of the window | {} pixels |", get("view-px"));
    out!(report, "| Display scaling | {} |", get("scaling"));
    out!(report, "| Vsync | {} |", get("vsync"));
    out!(report, "| Build | {} |", if cfg!(debug_assertions) { "debug" } else { "release" });
}

/// Our renderer against pdfium, sheet by sheet: how long from sending the view
/// to a sheet until the app reports that sheet sharp.
///
/// The only fair way to compare the two. Reading it out of the trace isn't:
/// pdfium's own timing is a whole rasterisation in a helper process competing
/// with two others, while the GPU renderer's is the read alone, without the
/// upload that follows it -- and half its reads are thumbnails at an eighth of
/// a pixel a point, which aren't sheets at all. Here one clock times both,
/// started and stopped by the same code, so the only difference is who drew.
fn bench_renderers(args: &Args, docs: &[(String, PathBuf, Option<String>)], exe_dir: &Path, data_dir: &Path, report: &mut String, summary: &mut Vec<String>) {
    let app = exe_dir.join("kinetic-pdf.exe");
    if !app.exists() {
        return;
    }
    out!(report);
    out!(report, "## Showing a sheet for the first time");
    out!(report);
    out!(
        report,
        "{SHEETS_SAMPLED} sheets spread through the document, shown one after another at {SHEET_ZOOM}% zoom, each timed from asking for it to the app reporting it sharp. Sheets are spread out so drawing ahead hasn't already done the work. `KINETIC_PDF_GPU=0` is pdfium doing all of it, which is what most PDF software does. {} runs a renderer, taking turns to go first, after a pair thrown away; each run in a cache folder of its own. Sheet times are pooled across runs.",
        args.repeats
    );
    out!(report);
    out!(report, "| Document | pdfium median sheet | Ours | pdfium p90 sheet | Ours | Sheets timed a renderer | Sheets ours gave to pdfium | pdfium memory | Ours |");
    out!(report, "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    let env = [("KINETIC_PDF_PAGE_BENCH", format!("{SHEETS_SAMPLED}:{SHEET_ZOOM}"))];
    for (name, path, _) in docs {
        let [theirs, ours] = match run_pairs(&app, path, data_dir, &env, "page-bench", args.repeats, None) {
            Ok(kept) => kept,
            Err(e) => {
                out!(report, "| {name} | {e} | | | | | | | |");
                continue;
            }
        };
        let pooled = |runs: &[Said]| runs.iter().flat_map(|s| numbers(s.get("sheets-ms"))).collect::<Vec<f64>>();
        let memory = |runs: &[Said]| percentile(&runs.iter().filter_map(|s| number(s, "memory-mb")).collect::<Vec<_>>(), 0.5);
        let (t, o) = (pooled(&theirs), pooled(&ours));
        let fell_back = ours.iter().filter_map(|s| number(s, "fell-back")).fold(0.0, f64::max);
        out!(
            report,
            "| {name} | {} | **{}** | {} | **{}** | {} / {} | {fell_back:.0} | {:.0} MB | {:.0} MB |",
            ms_or_s(percentile(&t, 0.5)),
            ms_or_s(percentile(&o, 0.5)),
            ms_or_s(percentile(&t, 0.9)),
            ms_or_s(percentile(&o, 0.9)),
            t.len(),
            o.len(),
            memory(&theirs),
            memory(&ours)
        );
        summary.push(format!("{name}: a sheet first shown sharp in {} with our renderer, {} with pdfium (median)", ms_or_s(percentile(&o, 0.5)), ms_or_s(percentile(&t, 0.5))));
    }
}

/// Working on a sheet: zooming in, panning about, zooming back out, timed step
/// by step. This is where a renderer that draws from geometry rather than
/// pixels earns its keep -- `bench_renderers` above times a sheet being drawn
/// once, cold, which is page reading that both renderers have to do.
fn bench_working(args: &Args, docs: &[(String, PathBuf, Option<String>)], exe_dir: &Path, data_dir: &Path, report: &mut String, summary: &mut Vec<String>) {
    let app = exe_dir.join("kinetic-pdf.exe");
    if !app.exists() {
        return;
    }
    let shots_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("bench-results").join("shots");
    out!(report);
    out!(report, "## Working on a sheet");
    out!(report);
    out!(
        report,
        "Twelve steps on the middle sheet, after it is already up: zoom to 200%, pan down twice, zoom to 400%, pan down twice, zoom to 800%, pan down twice, then back out to 400%, 200% and 100%. Each step is timed to the app reporting the view sharp: everything in view drawn at the zoom in view. That is not proof the frame reached the screen. A step ends on the third sharp frame, so no step can measure shorter than about three frame intervals; the frame interval is reported so results at that floor can be seen for what they are. Getting the sheet up isn't counted. {} runs a renderer, taking turns to go first, after a pair thrown away. Step times are pooled across runs.",
        args.repeats
    );
    out!(report);
    out!(report, "| Document | pdfium, 12 steps (median run) | Ours | pdfium slowest run | Ours | Ratio of medians | pdfium median step | Ours | pdfium p90 step | Ours | Frame interval, pdfium / ours | Sheets ours gave to pdfium |");
    out!(report, "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    let env = [("KINETIC_PDF_WORK_BENCH", "0".to_owned())];
    let mut setup: Option<Said> = None;
    let mut notes = Vec::new();
    for (name, path, _) in docs {
        let shots = shots_root.join(slug(name));
        let [theirs, ours] = match run_pairs(&app, path, data_dir, &env, "work-bench", args.repeats, Some(&shots)) {
            Ok(kept) => kept,
            Err(e) => {
                out!(report, "| {name} | {e} | | | | | | | | | | |");
                continue;
            }
        };
        if setup.is_none() {
            setup = ours.first().cloned();
        }
        let totals = |runs: &[Said]| runs.iter().filter_map(|s| number(s, "total-ms")).collect::<Vec<f64>>();
        let steps = |runs: &[Said]| runs.iter().flat_map(|s| numbers(s.get("steps-ms"))).collect::<Vec<f64>>();
        let frame = |runs: &[Said]| percentile(&runs.iter().filter_map(|s| number(s, "frame-ms")).collect::<Vec<_>>(), 0.5);
        let (tt, ot) = (totals(&theirs), totals(&ours));
        let (ts, os) = (steps(&theirs), steps(&ours));
        let ratio = percentile(&tt, 0.5) / percentile(&ot, 0.5).max(1e-9);
        let fell_back = ours.iter().filter_map(|s| number(s, "fell-back")).fold(0.0, f64::max);
        out!(
            report,
            "| {name} | {} | **{}** | {} | **{}** | {ratio:.1}× | {} | **{}** | {} | **{}** | {:.1} / {:.1} ms | {fell_back:.0} |",
            ms_or_s(percentile(&tt, 0.5)),
            ms_or_s(percentile(&ot, 0.5)),
            ms_or_s(percentile(&tt, 1.0)),
            ms_or_s(percentile(&ot, 1.0)),
            ms_or_s(percentile(&ts, 0.5)),
            ms_or_s(percentile(&os, 0.5)),
            ms_or_s(percentile(&ts, 0.9)),
            ms_or_s(percentile(&os, 0.9)),
            frame(&theirs),
            frame(&ours)
        );
        let floor = frame(&ours) * 3.0;
        let at_floor = os.iter().filter(|&&s| s <= floor * 1.2).count();
        notes.push(format!("{name}: {at_floor} of our {} steps were within 20% of the three-frame floor ({floor:.0} ms). Pictures of the view at 200% and 800%: `shots/{}/`.", os.len(), slug(name)));
        summary.push(format!("{name}: zooming and panning about a sheet, {} against pdfium's {} (median run)", ms_or_s(percentile(&ot, 0.5)), ms_or_s(percentile(&tt, 0.5))));
    }
    out!(report);
    for note in notes {
        out!(report, "- {note}");
    }
    compare_shots(docs, &shots_root, report);
    if let Some(said) = setup {
        out!(report);
        out!(report, "### Measured on");
        out!(report);
        describe_machine(report, &said);
    }
}

/// Runs the app's zoom benchmark once, returning how long the view took to be
/// sharp and what had been drawn ahead.
fn run_zoom_bench(app: &Path, pdf: &Path, cache: &Path) -> Result<(String, String), String> {
    let out = std::process::Command::new(app)
        .arg(pdf)
        .env("KINETIC_PDF_ZOOM_BENCH", format!("{}:{ZOOM_REST}", usize::MAX))
        .env("KINETIC_PDF_CACHE", cache)
        .env("KINETIC_PDF_UPDATE", "0")
        .stdout(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("could not run the app: {e}"))?;
    let said = String::from_utf8_lossy(&out.stderr);
    let after = |mark: &str| said.lines().find_map(|line| line.trim().strip_prefix(mark).map(|rest| rest.trim().to_owned()));
    let sharp = after("zoom-bench-ms:").ok_or("the app didn't report a zoom time")?;
    let ahead = after("zoom bench: drawn ahead before zooming:").unwrap_or_default();
    let sharp = if sharp == "none" { "not sharp".to_owned() } else { format!("{sharp} ms") };
    Ok((sharp, ahead))
}

#[allow(clippy::too_many_arguments)]
fn bench_document(
    pdfium: &Pdfium,
    args: &Args,
    name: &str,
    path: &Path,
    rare_hint: Option<&str>,
    data_dir: &Path,
    report: &mut String,
    summary: &mut Vec<String>,
) -> Result<(), String> {
    let file_mb = std::fs::metadata(path).map(|m| m.len() as f64 / MB).unwrap_or(0.0);
    out!(report);
    out!(report, "## {name}");
    out!(report);
    // No path, just the name in the heading: reports get shared, and a full
    // path gives away where the file lived and whose computer it was on.
    out!(report, "{file_mb:.1} MB");

    /* ---------------- Opening ---------------- */

    out!(report);
    out!(report, "### Opening, step by step as the app does it");
    out!(report);
    out!(report, "Everything up to the first page appearing. The other pages' highlights are read afterwards, in the background, and don't hold up the first page.");
    out!(report);

    let mut read = Samples::default();
    let mut copy = Samples::default();
    let mut parse = Samples::default();
    let mut sizes = Samples::default();
    let mut highlights = Samples::default();
    let mut first_render = Samples::default();
    let mut first_convert = Samples::default();
    let mut total = Samples::default();
    let mut background = Samples::default();
    let mut pages = 0;
    let mut highlight_count = 0;

    let egui_ctx = egui::Context::default();
    for _ in 0..args.runs {
        let run_start = Instant::now();
        let (bytes, t) = timed(|| std::fs::read(path));
        let bytes = bytes.map_err(|e| e.to_string())?;
        read.push(t);
        let (copied, t) = timed(|| bytes.clone());
        copy.push(t);
        let (doc, t) = timed(|| pdfium.load_pdf_from_byte_vec(copied, None));
        let doc = doc.map_err(err)?;
        parse.push(t);
        let (page_sizes, t) = timed(|| annots::page_sizes(&doc));
        pages = page_sizes.len();
        sizes.push(t);
        let (found, t) = timed(|| annots::read_page_highlights(&doc, 0));
        highlights.push(t);
        let (rendered, t) = timed(|| {
            annots::strip_highlights(&doc, 0);
            annots::render_page(&doc, 0, capped_scale(&page_sizes, 0, 2.5))
        });
        let (size, rgba) = rendered?;
        first_render.push(t);
        let ((), t) = timed(|| build_texture(&egui_ctx, size, &rgba));
        first_convert.push(t);
        total.push(run_start.elapsed());
        let (rest, t) = timed(|| (1..pages).map(|p| annots::read_page_highlights(&doc, p).len()).sum::<usize>());
        background.push(t);
        highlight_count = found.len() + rest;
    }

    out!(report, "| Step | First run | Best | Median |");
    out!(report, "| --- | ---: | ---: | ---: |");
    let rows: [(&str, &Samples); 7] = [
        ("Read the file into memory", &read),
        ("Copy it for pdfium", &copy),
        ("Parse the document", &parse),
        ("Every page's size", &sizes),
        ("Read page 1's highlights", &highlights),
        ("Render page 1 (125% zoom, 200% display)", &first_render),
        ("Make page 1's texture (worker thread)", &first_convert),
    ];
    for (label, s) in rows {
        out!(report, "| {label} | {} | {} | {} |", ms(s.first()), ms(s.min()), ms(s.median()));
    }
    out!(report, "| **Time to first page** | **{}** | **{}** | **{}** |", ms(total.first()), ms(total.min()), ms(total.median()));
    out!(
        report,
        "| Then, in the background: every other page's highlights | {} | {} | {} |",
        ms(background.first()),
        ms(background.min()),
        ms(background.median())
    );
    out!(report);
    out!(report, "{pages} pages, {highlight_count} highlights.");
    summary.push(format!("{name}: time to first page {} (median)", ms(total.median())));
    memory_line(report);

    // One copy of the document for the rest, as the worker keeps.
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let doc = pdfium.load_pdf_from_byte_vec(bytes, None).map_err(err)?;
    let n = doc.pages().len() as usize;

    /* ---------------- Rendering ---------------- */

    out!(report);
    out!(report, "### Rendering");
    out!(report);
    let sample = spread(n, 12);
    let sizes = annots::page_sizes(&doc);
    let mut strip = Samples::default();
    for &p in &sample {
        strip.push(timed(|| annots::strip_highlights(&doc, p)).1);
    }
    out!(report, "Measured over {} pages spread through the document. Removing a page's highlights before its first render: {} median.", sample.len(), ms(strip.median()));
    out!(report);
    out!(report, "| View | Pixels | Texture | Render best | Render median | Render p95 | Make texture (worker thread) | Memory for {PAGES_KEPT} kept pages |");
    out!(report, "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for (label, scale) in SCALES {
        let mut render = Samples::default();
        let mut convert = Samples::default();
        let mut largest = [0usize, 0usize];
        for &p in &sample {
            let (rendered, t) = timed(|| annots::render_page(&doc, p, capped_scale(&sizes, p, scale)));
            let (size, rgba) = rendered?;
            render.push(t);
            if size[0] * size[1] > largest[0] * largest[1] {
                largest = size;
            }
            convert.push(timed(|| build_texture(&egui_ctx, size, &rgba)).1);
        }
        let texture_mb = (largest[0] * largest[1] * 4) as f64 / MB;
        out!(
            report,
            "| {label} | {} × {} | {texture_mb:.1} MB | {} | {} | {} | {} | {:.0} MB |",
            largest[0],
            largest[1],
            ms(render.min()),
            ms(render.median()),
            ms(render.p95()),
            ms(convert.median()),
            texture_mb * PAGES_KEPT as f64
        );
        if scale == 2.5 {
            summary.push(format!(
                "{name}: page render {} + making its texture {}, both on the worker thread (median, 125% zoom on a 200% display)",
                ms(render.median()),
                ms(convert.median())
            ));
        }
    }
    memory_line(report);

    /* ---------------- Text ---------------- */

    out!(report);
    out!(report, "### Text extraction");
    out!(report);
    let text_sample = spread(n, 30);
    let mut text = Samples::default();
    let mut chars_total = 0;
    let mut words: HashMap<String, usize> = HashMap::new();
    for &p in &text_sample {
        let (chars, t) = timed(|| annots::page_chars(&doc, p));
        let chars = chars?;
        text.push(t);
        chars_total += chars.len();
        let page_text: String = chars.iter().map(|c| c.ch).collect();
        for word in page_text.split(|c: char| !c.is_alphabetic()) {
            if word.chars().count() >= 3 {
                *words.entry(word.to_lowercase()).or_default() += 1;
            }
        }
    }
    let per_page = chars_total / text_sample.len().max(1);
    out!(report, "| Pages sampled | Characters per page | Best | Median | p95 |");
    out!(report, "| ---: | ---: | ---: | ---: | ---: |");
    out!(report, "| {} | {per_page} | {} | {} | {} |", text_sample.len(), ms(text.min()), ms(text.median()), ms(text.p95()));
    out!(report);
    out!(report, "The app extracts text for each page it shows, in a separate request from the render.");

    /* ---------------- Search ---------------- */

    out!(report);
    out!(report, "### Search, the whole document as the app does it");
    out!(report);
    let mut ranked: Vec<(&String, &usize)> = words.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let common = ranked.first().map(|(w, _)| (*w).clone());
    let rare = rare_hint.map(str::to_owned).or_else(|| {
        ranked.iter().rev().find(|(w, c)| **c == 1 && w.chars().count() >= 6).map(|(w, _)| (*w).clone())
    });
    let queries = [("Common word", common), ("Rare word", rare), ("No matches", Some("qzxjvkw".to_owned()))];

    out!(report, "| Query | Matches | First match after | Whole document | Pages per second |");
    out!(report, "| --- | ---: | ---: | ---: | ---: |");
    for (kind, query) in queries {
        let Some(query) = query else { continue };
        let start = Instant::now();
        let mut hits = 0usize;
        let mut first_hit = None;
        for p in 0..n {
            if hits >= MAX_SEARCH_HITS {
                break;
            }
            let Ok(chars) = annots::page_chars(&doc, p) else { continue };
            for range in selection::find(&chars, &query) {
                let quads = selection::bands(&chars, range.clone());
                if !quads.is_empty() && hits < MAX_SEARCH_HITS {
                    let _context = selection::context(&chars, range, 40, 120);
                    hits += 1;
                    first_hit.get_or_insert_with(|| start.elapsed());
                }
            }
        }
        let elapsed = start.elapsed();
        let first = first_hit.map_or_else(|| "none".to_owned(), ms);
        let capped = if hits >= MAX_SEARCH_HITS { "+ (capped)" } else { "" };
        let rate = n as f64 / elapsed.as_secs_f64().max(1e-9);
        out!(report, "| {kind}: \"{query}\" | {hits}{capped} | {first} | {} | {rate:.0} |", ms(elapsed));
        if kind == "Common word" {
            summary.push(format!("{name}: search for a common word {}", ms(elapsed)));
        }
    }
    out!(report);
    out!(report, "The app searches in 30 ms slices between other work, so on screen it takes a little longer than this.");
    memory_line(report);
    drop(doc);

    /* ---------------- Saving ---------------- */

    out!(report);
    out!(report, "### Saving");
    out!(report);
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let changes = {
        let doc = pdfium.load_pdf_from_byte_slice(&bytes, None).map_err(err)?;
        let mut adds = Vec::new();
        for p in spread(n, 10) {
            let chars = annots::page_chars(&doc, p)?;
            let quads = selection::bands(&chars, 0..80.min(chars.len()));
            if !quads.is_empty() {
                adds.push(NewHighlight { page: p, quads, color: [0.56, 0.93, 0.45], comment: "Benchmark note".to_owned() });
            }
        }
        Changes { adds, author: "bench".to_owned(), ..Changes::default() }
    };
    let added = changes.adds.len();

    let (saved, apply) = timed(|| annots::save(pdfium, &bytes, &changes));
    let saved = saved?.bytes;
    let scratch = data_dir.join("save-test.pdf");
    let (written, write) = timed(|| std::fs::write(&scratch, &saved));
    written.map_err(|e| e.to_string())?;
    let (reloaded, reload) = timed(|| pdfium.load_pdf_from_byte_vec(saved.clone(), None));
    let reloaded = reloaded.map_err(err)?;
    // The app re-reads only the pages the save changed.
    let mut changed: Vec<usize> = changes.adds.iter().map(|a| a.page).collect();
    changed.sort_unstable();
    changed.dedup();
    let (found, reread) =
        timed(|| changed.iter().map(|&p| annots::read_page_highlights(&reloaded, p).len()).sum::<usize>());
    let round_trip = apply + write + reload + reread;

    out!(report, "Adding {added} highlights, then everything the app does after Save.");
    out!(report);
    out!(report, "| Step | Time |");
    out!(report, "| --- | ---: |");
    out!(report, "| Parse, apply changes, serialise | {} |", ms(apply));
    out!(report, "| Write the file | {} |", ms(write));
    out!(report, "| Re-open the saved bytes | {} |", ms(reload));
    out!(report, "| Re-read highlights on the {} changed pages ({found} found) | {} |", changed.len(), ms(reread));
    out!(report, "| **Save round trip** | **{}** |", ms(round_trip));
    summary.push(format!("{name}: save round trip {}", ms(round_trip)));
    let _ = std::fs::remove_file(&scratch);
    memory_line(report);
    Ok(())
}
