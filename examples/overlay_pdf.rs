//! Writing an overlay out as a PDF, in vectors, that both renderers can draw.
//!
//! `crates/gpu-lines/examples/overlay.rs` overlays pages on the GPU, which is
//! what you want while you're working: nothing is written, and it costs
//! nothing at any zoom. But an overlay you mean to send someone has to be a
//! document, and it should still be a drawing when it gets there -- vectors
//! that print and zoom, not a picture of a drawing.
//!
//! The obvious way to tint a page in PDF is a luminosity soft mask over a
//! solid fill. It is elegant, it needs no rewriting at all, and it is the
//! wrong answer here: `interpret.rs` counts soft masks and transparency
//! groups among the things it can't draw, so every overlay written that way
//! would fall off our own renderer onto pdfium, for the one feature whose
//! whole point is that it's fast.
//!
//! So this recolours the vectors instead and blends with `/BM /Multiply`,
//! which the interpreter already understands. Recolouring sounds like the
//! bigger job, but a drawing has a handful of pen colours -- 26 to 33 on the
//! sheets this was written against -- and the colour operators say what they
//! are. Each one is left where it is and a replacement written straight after
//! it, so no operand is ever touched and nothing else in the stream moves.
//!
//! Each source becomes a form XObject in its own optional-content group, so
//! the layers toggle in any viewer.
//!
//! ```text
//! cargo run --release --example overlay_pdf -- a.pdf 9 --and b.pdf 9 --out over.pdf
//! cargo run --release --example overlay_pdf -- a.pdf 1 2 --out over.pdf --check
//! ```
//!
//! `--check` draws the written file with both renderers -- ours and pdfium --
//! and reports how far apart they are. Checking it with the renderer that
//! wrote it would only prove it agrees with itself.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpu_lines::lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use gpu_lines::{page_shapes, page_size, Renderer, Tint, MOST_IMAGE_DENSITY};
use pdf_content::lexer::each_operation;

const TOLERANCE: f32 = 0.05;

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    match write_overlay(&args) {
        Ok(report) => {
            print!("{report}");
            if args.check {
                check(&args);
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

// ------------------------------------------------------------- recolouring

/// The colour a tinted shape is drawn in. Multiplied onto white paper this
/// gives back the tint where the shape was black and nothing where it was
/// white, which is what the GPU overlay's shader works out per pixel; here
/// the colour is constant across a path, so it can be worked out once.
fn tinted(rgb: [f32; 3], tint: [f32; 3], strength: f32) -> [f32; 3] {
    let luminance = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
    let ink = (1.0 - luminance).clamp(0.0, 1.0) * strength;
    [0, 1, 2].map(|c| 1.0 - ink * (1.0 - tint[c]))
}

/// What a colour operator's operands mean, as RGB. `None` for the ones that
/// aren't a plain colour -- a pattern name, or a space we can't read -- which
/// are left alone.
fn colour_of(op: &[u8], operands: &[f32], named: bool) -> Option<[f32; 3]> {
    if named {
        return None;
    }
    match (op, operands.len()) {
        (b"g" | b"G", 1) => Some([operands[0]; 3]),
        (b"rg" | b"RG", 3) => Some([operands[0], operands[1], operands[2]]),
        (b"k" | b"K", 4) | (b"sc" | b"SC" | b"scn" | b"SCN", 4) => {
            let (c, m, y, k) = (operands[0], operands[1], operands[2], operands[3]);
            Some([(1.0 - (c + k)).max(0.0), (1.0 - (m + k)).max(0.0), (1.0 - (y + k)).max(0.0)])
        }
        (b"sc" | b"SC" | b"scn" | b"SCN", 1) => Some([operands[0]; 3]),
        (b"sc" | b"SC" | b"scn" | b"SCN", 3) => Some([operands[0], operands[1], operands[2]]),
        _ => None,
    }
}

fn strokes(op: &[u8]) -> bool {
    op.iter().all(|b| b.is_ascii_uppercase())
}

/// `content` with every colour it sets followed by the tint of that colour.
/// The original operator stays exactly where it was -- the one written after
/// it simply wins -- so operands are never rewritten and every other byte of
/// the stream keeps its place.
fn recolour(content: &[u8], tint: [f32; 3], strength: f32) -> (Vec<u8>, usize) {
    // Strength 0 means write the page through untouched, which is how the
    // two renderers get compared on the same content with none of this in
    // the way -- the floor that any tinted comparison has to be read against.
    if strength <= 0.0 {
        return (content.to_vec(), 0);
    }
    let mut insert: Vec<(usize, Vec<u8>)> = Vec::new();
    each_operation(content, |op, operands, range| {
        // Choosing a colour space sets the colour back to that space's
        // default, which for every space a drawing uses is black -- so a
        // tint written before one is wiped by it. Renderers differ on
        // exactly when that bites, which is the sort of thing you only find
        // by having two of them draw the same file: tint the space's default
        // as well, and any real colour set afterwards writes over it.
        if matches!(op, b"cs" | b"CS") {
            let [r, g, b] = tinted([0.0; 3], tint, strength);
            let set = if strokes(op) { "RG" } else { "rg" };
            insert.push((range.end, format!(" {r:.4} {g:.4} {b:.4} {set}").into_bytes()));
            return;
        }
        if !matches!(op, b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K" | b"sc" | b"SC" | b"scn" | b"SCN") {
            return;
        }
        let named = operands.last().is_some_and(|o| o.name().is_some());
        let numbers: Vec<f32> = operands.iter().filter_map(|o| o.number()).collect();
        let Some(rgb) = colour_of(op, &numbers, named) else { return };
        let [r, g, b] = tinted(rgb, tint, strength);
        let set = if strokes(op) { "RG" } else { "rg" };
        insert.push((range.end, format!(" {r:.4} {g:.4} {b:.4} {set}").into_bytes()));
    });

    let mut out = Vec::with_capacity(content.len() + insert.len() * 24);
    let mut from = 0usize;
    for (at, bytes) in &insert {
        out.extend_from_slice(&content[from..*at]);
        out.extend_from_slice(bytes);
        from = *at;
    }
    out.extend_from_slice(&content[from..]);
    (out, insert.len())
}

// ----------------------------------------------------------------- copying

/// Copies `object` and everything it refers to into `to`, recolouring the
/// content of any form it meets on the way, since a drawing keeps much of
/// itself in forms. Object ids are reserved before their contents are copied,
/// so a page that refers back to itself doesn't go round for ever.
fn import(from: &Document, to: &mut Document, object: &Object, done: &mut HashMap<ObjectId, ObjectId>, tint: [f32; 3], strength: f32, recoloured: &mut usize) -> Object {
    match object {
        Object::Reference(id) => {
            if let Some(&already) = done.get(id) {
                return Object::Reference(already);
            }
            let new_id = to.new_object_id();
            done.insert(*id, new_id);
            let copied = match from.get_object(*id) {
                Ok(target) => import(from, to, target, done, tint, strength, recoloured),
                Err(_) => Object::Null,
            };
            to.objects.insert(new_id, copied);
            Object::Reference(new_id)
        }
        Object::Array(items) => Object::Array(items.iter().map(|i| import(from, to, i, done, tint, strength, recoloured)).collect()),
        Object::Dictionary(d) => Object::Dictionary(import_dict(from, to, d, done, tint, strength, recoloured)),
        Object::Stream(stream) => {
            let dict = import_dict(from, to, &stream.dict, done, tint, strength, recoloured);
            let is_form = stream.dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Form");
            match (is_form, stream.decompressed_content()) {
                (true, Ok(content)) => {
                    let (tinted, n) = recolour(&content, tint, strength);
                    *recoloured += n;
                    let mut plain = dict;
                    plain.remove(b"Filter");
                    plain.remove(b"DecodeParms");
                    Object::Stream(Stream::new(plain, tinted))
                }
                _ => {
                    let mut copied = stream.clone();
                    copied.dict = dict;
                    Object::Stream(copied)
                }
            }
        }
        other => other.clone(),
    }
}

fn import_dict(from: &Document, to: &mut Document, d: &Dictionary, done: &mut HashMap<ObjectId, ObjectId>, tint: [f32; 3], strength: f32, recoloured: &mut usize) -> Dictionary {
    let mut out = Dictionary::new();
    for (key, value) in d.iter() {
        out.set(key.to_vec(), import(from, to, value, done, tint, strength, recoloured));
    }
    // A drawing carries graphics states of its own, and they all say
    // `/BM /Normal` -- every `gs` in the content would switch our multiply
    // back off partway through the sheet, and the second layer would paint
    // over the first instead of darkening it. Setting it on the way in means
    // nothing inside the page can turn it off.
    if strength > 0.0 && out.get(b"Type").and_then(Object::as_name).is_ok_and(|t| t == b"ExtGState") {
        out.set(b"BM".to_vec(), Object::Name(b"Multiply".to_vec()));
    }
    out
}

// ---------------------------------------------------------------- assembly

/// The box a page shows, and how many quarter turns it is displayed through,
/// as `page_shapes` takes them.
fn placed(doc: &Document, page_id: ObjectId) -> ([f32; 4], i32) {
    let boxed = |key: &[u8]| {
        let mut at = page_id;
        for _ in 0..32 {
            let Ok(page) = doc.get_dictionary(at) else { break };
            if let Some(found) = page.get(key).ok().and_then(|b| doc.dereference(b).ok()).and_then(|(_, b)| b.as_array().ok()) {
                let numbers: Vec<f32> = found.iter().filter_map(|n| n.as_float().ok().or_else(|| n.as_i64().ok().map(|i| i as f32))).collect();
                if numbers.len() == 4 {
                    return Some([numbers[0].min(numbers[2]), numbers[1].min(numbers[3]), numbers[0].max(numbers[2]), numbers[1].max(numbers[3])]);
                }
            }
            match page.get(b"Parent").ok().and_then(|p| p.as_reference().ok()) {
                Some(up) => at = up,
                None => break,
            }
        }
        None
    };
    let area = match (boxed(b"MediaBox"), boxed(b"CropBox")) {
        (Some(m), Some(c)) => [m[0].max(c[0]), m[1].max(c[1]), m[2].min(c[2]), m[3].min(c[3])],
        (m, c) => c.or(m).unwrap_or([0.0, 0.0, 612.0, 792.0]),
    };
    // Inherited, like the boxes: a set of drawings commonly carries one
    // /Rotate on the page tree rather than on every sheet, and reading it
    // only from the page itself turns the whole overlay upside down.
    let mut turns = 0i32;
    let mut at = page_id;
    for _ in 0..32 {
        let Ok(page) = doc.get_dictionary(at) else { break };
        if let Some(found) = page.get(b"Rotate").ok().and_then(|r| doc.dereference(r).ok()).and_then(|(_, r)| r.as_i64().ok()) {
            turns = ((found / 90) % 4) as i32;
            break;
        }
        match page.get(b"Parent").ok().and_then(|p| p.as_reference().ok()) {
            Some(up) => at = up,
            None => break,
        }
    }
    (area, turns.rem_euclid(4))
}

/// The form's matrix: the page's own space moved so its visible corner is the
/// origin, then turned by `/Rotate`, which is where `page_shapes` puts it too.
fn matrix_for(area: [f32; 4], turns: i32) -> Vec<Object> {
    let (across, up) = (area[2] - area[0], area[3] - area[1]);
    let (left, bottom) = (area[0], area[1]);
    // Translate first, then rotate: the composition of [1 0 0 1 -l -b] with
    // the turn, written out rather than multiplied at runtime.
    // /Rotate is how far clockwise the *page* is turned to be shown, so the
    // content turns with it: 90 takes (x, y) to (y, across - x). Getting this
    // the wrong way round is invisible on a square page and upside down on
    // every real sheet.
    let m: [f32; 6] = match turns {
        1 => [0.0, -1.0, 1.0, 0.0, -bottom, across + left],
        2 => [-1.0, 0.0, 0.0, -1.0, across + left, up + bottom],
        3 => [0.0, 1.0, -1.0, 0.0, up + bottom, -left],
        _ => [1.0, 0.0, 0.0, 1.0, -left, -bottom],
    };
    m.iter().map(|&v| Object::Real(v)).collect()
}

struct Sheet {
    path: PathBuf,
    page: u32,
}

struct Args {
    sheets: Vec<Sheet>,
    paths: Vec<PathBuf>,
    all: bool,
    out: PathBuf,
    strength: f32,
    check: bool,
    quick: bool,
    level: u32,
    width: u32,
}

/// One source document, loaded once however many of its pages are used, with
/// the colour it is drawn in, the layer it belongs to, and the objects
/// already brought across -- so a font shared by twenty sheets is imported
/// once rather than twenty times.
struct Source {
    path: PathBuf,
    doc: Document,
    tint: [f32; 3],
    ocg: ObjectId,
    done: HashMap<ObjectId, ObjectId>,
}

fn write_overlay(args: &Args) -> Result<String, String> {
    let mut report = String::new();
    let mut out = Document::with_version("1.7");
    let pages_id = out.new_object_id();

    // The documents, in the order they were named. Each is loaded once and
    // becomes one layer, so turning a revision off turns it off on every
    // sheet at once.
    let wanted: Vec<PathBuf> = if args.all { args.paths.clone() } else { unique(args.sheets.iter().map(|s| s.path.clone())) };
    let mut sources: Vec<Source> = Vec::new();
    for (n, path) in wanted.iter().enumerate() {
        let loading = Instant::now();
        let doc = Document::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path.file_name().map_or_else(|| path.display().to_string(), |f| f.to_string_lossy().into_owned());
        let ocg = out.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal(name.clone()) });
        let tint = Tint::nth(n).colour;
        report += &format!("  {name}: {} pages, in [{:.2}, {:.2}, {:.2}], read in {:.1} ms\n", doc.get_pages().len(), tint[0], tint[1], tint[2], loading.elapsed().as_secs_f64() * 1e3);
        sources.push(Source { path: path.clone(), doc, tint, ocg, done: HashMap::new() });
    }

    // Which sources each written page draws. `--all` takes the sheets in
    // order, as far as the shortest document goes; otherwise it is the single
    // page the pages named on the command line make between them.
    let plan: Vec<Vec<(usize, u32)>> = if args.all {
        let shortest = sources.iter().map(|s| s.doc.get_pages().len() as u32).min().unwrap_or(0);
        (1..=shortest).map(|page| (0..sources.len()).map(|s| (s, page)).collect()).collect()
    } else {
        vec![args
            .sheets
            .iter()
            .map(|sheet| (sources.iter().position(|s| s.path == sheet.path).unwrap_or(0), sheet.page))
            .collect()]
    };
    if plan.iter().all(Vec::is_empty) {
        return Err("no pages to overlay".into());
    }

    let mode = if args.strength > 0.0 { "Multiply" } else { "Normal" };
    let blend = out.add_object(dictionary! { "Type" => "ExtGState", "BM" => mode, "CA" => 1, "ca" => 1 });

    let mut page_ids: Vec<Object> = Vec::new();
    let mut size = [612.0f32, 792.0];
    let (mut tinted_total, mut layers) = (0usize, 0usize);
    let started = Instant::now();

    for (n, layout) in plan.iter().enumerate() {
        let mut xobjects = Dictionary::new();
        let mut properties = Dictionary::new();
        let mut content = String::new();
        let mut on_this_page = 0usize;

        for (place, &(which, number)) in layout.iter().enumerate() {
            let source = &mut sources[which];
            let Some(&page) = source.doc.get_pages().get(&number) else { continue };
            let (area, turns) = placed(&source.doc, page);
            if n == 0 && place == 0 {
                size = page_size(&source.doc, number)?;
            }

            let mut recoloured = 0usize;
            let raw = source.doc.get_page_content(page);
            let (tinted_content, here) = recolour(&raw, source.tint, args.strength);
            recoloured += here;
            let resources = match source.doc.get_dictionary(page).ok().and_then(|p| p.get(b"Resources").ok()) {
                Some(r) => import(&source.doc, &mut out, r, &mut source.done, source.tint, args.strength, &mut recoloured),
                None => Object::Dictionary(Dictionary::new()),
            };
            tinted_total += recoloured;

            let form = out.add_object(Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Form",
                    "FormType" => 1,
                    "BBox" => vec![Object::Real(area[0]), Object::Real(area[1]), Object::Real(area[2]), Object::Real(area[3])],
                    "Matrix" => matrix_for(area, turns),
                    "Resources" => resources,
                },
                tinted_content,
            ));
            let (name, oc) = (format!("Fm{place}"), format!("OC{place}"));
            xobjects.set(name.as_bytes().to_vec(), Object::Reference(form));
            properties.set(oc.as_bytes().to_vec(), Object::Reference(source.ocg));
            content += &format!("/OC /{oc} BDC q /GSm gs /{name} Do Q EMC\n");
            on_this_page += 1;
        }
        if on_this_page == 0 {
            continue;
        }
        layers += on_this_page;

        let contents = out.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        page_ids.push(Object::Reference(out.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "MediaBox" => vec![Object::Real(0.0), Object::Real(0.0), Object::Real(size[0]), Object::Real(size[1])],
            "Resources" => dictionary! {
                "XObject" => xobjects,
                "Properties" => properties,
                "ExtGState" => dictionary! { "GSm" => Object::Reference(blend) },
            },
            "Contents" => Object::Reference(contents),
        })));
    }

    let built_ms = started.elapsed().as_secs_f64() * 1e3;
    let count = page_ids.len();
    out.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => page_ids, "Count" => count as i64 }));

    let groups: Vec<Object> = sources.iter().map(|s| Object::Reference(s.ocg)).collect();
    let catalog = out.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
        "OCProperties" => dictionary! {
            "OCGs" => groups.clone(),
            "D" => dictionary! { "Order" => groups.clone(), "ON" => groups },
        },
    });
    out.trailer.set("Root", Object::Reference(catalog));

    // What actually has to be deflated: the content we recoloured, which had
    // to be decompressed to be read and so cannot be passed through. Streams
    // brought across untouched keep the filter they arrived with and are not
    // touched again.
    let raw: usize = out
        .objects
        .values()
        .map(|o| match o {
            Object::Stream(stream) if stream.dict.get(b"Filter").is_err() => stream.content.len(),
            _ => 0,
        })
        .sum();

    let deflating = Instant::now();
    if !args.quick {
        deflate_streams(&mut out, args.level);
    }
    let compress_ms = deflating.elapsed().as_secs_f64() * 1e3;
    let writing = Instant::now();
    out.save(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let save_ms = writing.elapsed().as_secs_f64() * 1e3;
    let written = std::fs::metadata(&args.out).map(|m| m.len()).unwrap_or(0);

    report += &format!("\n{count} pages, {layers} layers, {tinted_total} colours tinted, {:.0} x {:.0} pt\n", size[0], size[1]);
    report += &format!("  building                {built_ms:>9.1} ms\n");
    report += &format!("  deflating {:>5.1} MB      {compress_ms:>9.1} ms{}\n", raw as f64 / 1.048_576e6, if args.quick { "  (skipped)" } else { "" });
    report += &format!("  writing it out          {save_ms:>9.1} ms\n");
    report += &format!("Written to {} ({:.1} MB)\n", args.out.display(), written as f64 / 1.048_576e6);
    Ok(report)
}

/// Deflates every stream that hasn't got a filter already, across as many
/// threads as the machine has.
///
/// `Document::compress` does them one after another, and on a set of drawings
/// that is nearly all of the wall clock: the content had to be decompressed
/// to be recoloured, so tens of megabytes have to go back through zlib.
/// Nothing about that is sequential -- each stream is on its own -- and the
/// streams that arrived compressed and were never touched keep the filter
/// they came with and are left alone here.
fn deflate_streams(out: &mut Document, level: u32) {
    use std::io::Write;

    // Take the content out rather than copy it: tens of megabytes.
    let mut jobs: Vec<(ObjectId, Vec<u8>)> = Vec::new();
    for (id, object) in out.objects.iter_mut() {
        if let Object::Stream(stream) = object {
            if stream.dict.get(b"Filter").is_err() && stream.content.len() > 512 {
                jobs.push((*id, std::mem::take(&mut stream.content)));
            }
        }
    }

    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(jobs.len().max(1));
    let each = jobs.len().div_ceil(threads.max(1));
    let mut done: Vec<(ObjectId, Vec<u8>, bool)> = Vec::with_capacity(jobs.len());
    std::thread::scope(|scope| {
        let mut running = Vec::new();
        for chunk in jobs.chunks_mut(each.max(1)) {
            running.push(scope.spawn(move || {
                chunk
                    .iter_mut()
                    .map(|(id, content)| {
                        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(level));
                        match encoder.write_all(content).and_then(|_| encoder.finish()) {
                            // Only worth it if it came out smaller; a stream
                            // that won't deflate goes back as it was.
                            Ok(squeezed) if squeezed.len() < content.len() => (*id, squeezed, true),
                            _ => (*id, std::mem::take(content), false),
                        }
                    })
                    .collect::<Vec<_>>()
            }));
        }
        for thread in running {
            done.extend(thread.join().unwrap_or_default());
        }
    });

    for (id, content, deflated) in done {
        if let Some(Object::Stream(stream)) = out.objects.get_mut(&id) {
            stream.set_content(content);
            if deflated {
                stream.dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
            }
        }
    }
}

/// The paths in the order first seen, without repeats.
fn unique(paths: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

// ------------------------------------------------------------------ checking

/// Draws the written file with both renderers and says how far apart they
/// are. Ours reads the PDF back through `gpu-lines`; pdfium has never seen
/// any of this code.
fn check(args: &Args) {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([360.0, 120.0]).with_title("Checking..."),
        ..Default::default()
    };
    let out = args.out.clone();
    let width = args.width;
    let _ = eframe::run_native("overlay-pdf check", options, Box::new(move |cc| Ok(Box::new(Check { out, width, gl: cc.gl.clone(), done: None }))));
}

struct Check {
    out: PathBuf,
    width: u32,
    gl: Option<Arc<eframe::glow::Context>>,
    done: Option<String>,
}

impl eframe::App for Check {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(gl) = self.gl.take() {
            self.done = Some(compare_renderers(&gl, &self.out, self.width).unwrap_or_else(|e| format!("error: {e}")));
            println!("{}", self.done.as_deref().unwrap_or_default());
            ui.ctx().send_viewport_cmd(eframe::egui::ViewportCommand::Close);
        }
        ui.label(self.done.as_deref().unwrap_or("Checking..."));
    }
}

fn compare_renderers(gl: &eframe::glow::Context, path: &PathBuf, width: u32) -> Result<String, String> {
    use pdfium_render::prelude::*;

    let doc = Document::load(path).map_err(|e| e.to_string())?;
    let points = page_size(&doc, 1)?;
    let height = ((width as f32 * points[1] / points[0]).round() as u32).max(1);

    // Ours.
    let started = Instant::now();
    let shapes = page_shapes(&doc, 1, TOLERANCE, MOST_IMAGE_DENSITY)?;
    let unsupported: Vec<String> = shapes.not_drawn.iter().map(|(what, n)| format!("{n} {what}")).collect();
    let renderer = Renderer::new(gl)?;
    let uploaded = renderer.upload(gl, shapes)?;
    let ours = renderer.draw_to_image(gl, &uploaded, [width, height], points).ok_or("ours couldn't draw it")?;
    let ours_ms = started.elapsed().as_secs_f64() * 1e3;
    uploaded.destroy(gl);
    renderer.destroy(gl);

    // pdfium's.
    let started = Instant::now();
    let pdfium = Pdfium::bind_to_library("pdfium.dll").or_else(|_| Pdfium::bind_to_system_library()).map(Pdfium::new).map_err(|e| e.to_string())?;
    let document = pdfium.load_pdf_from_file(path, None).map_err(|e| e.to_string())?;
    let page = document.pages().get(0).map_err(|e| e.to_string())?;
    let config = PdfRenderConfig::new().set_target_size(width as Pixels, height as Pixels);
    let bitmap = page.render_with_config(&config).map_err(|e| e.to_string())?;
    let theirs = bitmap.as_rgba_bytes();
    let theirs_ms = started.elapsed().as_secs_f64() * 1e3;

    let mut report = format!("\nDrawing the file back\n  ours    {ours_ms:>8.1} ms\n  pdfium  {theirs_ms:>8.1} ms\n");
    if !unsupported.is_empty() {
        report += &format!("  ours couldn't draw: {}\n", unsupported.join(", "));
    }

    if ours.len() != theirs.len() {
        return Ok(report + &format!("  different sizes: {} against {} bytes, not compared\n", ours.len(), theirs.len()));
    }
    // How far apart, per channel, over pixels either one drew anything on.
    let (mut worst, mut sum, mut counted) = (0u32, 0u64, 0u64);
    for (a, b) in ours.chunks_exact(4).zip(theirs.chunks_exact(4)) {
        let ink = a[0] < 250 || a[1] < 250 || a[2] < 250 || b[0] < 250 || b[1] < 250 || b[2] < 250;
        if !ink {
            continue;
        }
        counted += 1;
        for c in 0..3 {
            let apart = a[c].abs_diff(b[c]) as u32;
            worst = worst.max(apart);
            sum += apart as u64;
        }
    }
    let mean = if counted == 0 { 0.0 } else { sum as f64 / (counted * 3) as f64 };
    report += &format!("  {counted} pixels with ink in either, {mean:.2} apart on average, {worst} at worst (of 255)\n");

    let side_by_side = path.with_extension("");
    write_png(&side_by_side.with_file_name(format!("{}-ours.png", side_by_side.file_name().unwrap_or_default().to_string_lossy())), &ours, width, height)?;
    write_png(&side_by_side.with_file_name(format!("{}-pdfium.png", side_by_side.file_name().unwrap_or_default().to_string_lossy())), &theirs, width, height)?;
    report += "  both written beside the PDF, to look at\n";
    Ok(report)
}

fn write_png(path: &PathBuf, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { sheets: Vec::new(), paths: Vec::new(), all: false, out: PathBuf::from("overlay.pdf"), strength: 1.0, check: false, quick: false, level: 6, width: 1600 };
    let mut rest = std::env::args().skip(1);
    let mut path: Option<PathBuf> = None;
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--out" => args.out = PathBuf::from(rest.next().ok_or("--out wants a file")?),
            "--strength" => args.strength = rest.next().ok_or("--strength wants a number")?.parse().map_err(|_| "--strength wants a number")?,
            "--width" => args.width = rest.next().ok_or("--width wants a number")?.parse().map_err(|_| "--width wants a number")?,
            "--and" => {
                let named = PathBuf::from(rest.next().ok_or("--and wants a file")?);
                if !args.paths.contains(&named) {
                    args.paths.push(named.clone());
                }
                path = Some(named);
            }
            "--check" => args.check = true,
            "--all" => args.all = true,
            "--quick" => args.quick = true,
            "--level" => args.level = rest.next().ok_or("--level wants 0-9")?.parse().map_err(|_| "--level wants 0-9")?,
            "-h" | "--help" => {
                println!("overlay_pdf A.pdf PAGE [PAGE...] [--and B.pdf PAGE] [--out FILE] [--all] [--quick] [--level 0-9] [--strength 0-1] [--check] [--width PX]");
                std::process::exit(0);
            }
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other => match other.parse::<u32>() {
                Ok(page) => args.sheets.push(Sheet { path: path.clone().ok_or("give a PDF before its page numbers")?, page }),
                Err(_) => {
                    let named = PathBuf::from(other);
                    if !args.paths.contains(&named) {
                        args.paths.push(named.clone());
                    }
                    path = Some(named);
                }
            },
        }
    }
    if args.sheets.is_empty() && !args.all {
        return Err("give a PDF and the pages to overlay; --help for the rest".into());
    }
    if args.all && args.paths.len() < 2 {
        return Err("--all overlays two documents: give one, then --and the other".into());
    }
    Ok(args)
}
