//! An overlay written out as a PDF, in vectors: two drawing sets, sheet by
//! sheet, each in a colour of its own and laid over the other.
//!
//! Every sheet of each set is drawn as it is -- vectors that print and zoom,
//! not a picture of a drawing -- with its colours rewritten: each colour the
//! content sets is followed by its tint, as much of the set's colour as the
//! original is dark, and every graphics state is made to multiply. So where
//! both sets draw comes out dark, and where only one does keeps its colour,
//! as Kinetic Compare's preview shows it. A luminosity soft mask would tint a
//! page without rewriting anything, but the GPU renderer can't draw those,
//! and an overlay drawn by pdfium is too slow to be worth having.
//!
//! Each set is an optional content group -- a layer -- of its own, so either
//! can be turned off in any viewer. And the catalogue says, under `/KPDFCompare`,
//! which layer is which, so this app can fade from one set to the other
//! (`compare_layers`).
//!
//! Recolouring means inflating every page's content and every form it uses,
//! and deflating it again; each stream is its own work, so it's done across
//! every thread the machine has. About 0.7 s for a set of nineteen sheets.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;

use gpu_lines::lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use gpu_lines::{page_size, Tint};
use pdf_content::lexer::each_operation;

/// Placement of the compared sheet relative to the original. Offsets are
/// fractions of the original's width and height, so a document alignment
/// also works for sheets of different paper sizes. Resizing keeps the centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

impl Default for Alignment {
    fn default() -> Self {
        Self { scale: [1.0; 2], offset: [0.0; 2] }
    }
}

/// Common top-down placement used by the live comparison and PDF export.
pub(crate) struct OverlayLayout {
    pub size: [f32; 2],
    pub origins: [[f32; 2]; 2],
    pub scales: [[f32; 2]; 2],
}

pub(crate) fn overlay_layout(sizes: [[f32; 2]; 2], alignment: Option<Alignment>) -> OverlayLayout {
    let [a, b] = sizes;
    let alignment = alignment.filter(|_| a[0] > 0.0 && b[0] > 0.0);
    let scale = alignment.map_or([1.0; 2], |v| v.scale);
    let origin = alignment.map_or([0.0; 2], |v| [
        (a[0] - b[0] * scale[0]) * 0.5 + v.offset[0] * a[0],
        (a[1] - b[1] * scale[1]) * 0.5 + v.offset[1] * a[1],
    ]);
    let min = [origin[0].min(0.0), origin[1].min(0.0)];
    OverlayLayout {
        size: [a[0].max(origin[0] + b[0] * scale[0]) - min[0], a[1].max(origin[1] + b[1] * scale[1]) - min[1]],
        origins: [[-min[0], -min[1]], [origin[0] - min[0], origin[1] - min[1]]],
        scales: [[1.0; 2], scale],
    }
}

/// Where the catalogue says which layer is which set.
pub const MARKER: &[u8] = b"KPDFCompare";

/// The layers of an overlay written here, by object number and generation:
/// the original's, then the compared set's. `None` for any other PDF.
pub fn compare_layers(doc: &Document) -> Option<[(u32, u16); 2]> {
    let catalog = doc.catalog().ok()?;
    let marker = doc.dereference(catalog.get(MARKER).ok()?).ok()?.1.as_dict().ok()?;
    let layers = doc.dereference(marker.get(b"Layers").ok()?).ok()?.1.as_array().ok()?;
    match layers.as_slice() {
        [Object::Reference(a), Object::Reference(b)] => Some([*a, *b]),
        _ => None,
    }
}

/// The colour a shape of colour `rgb` is drawn in, in `tint`: multiplied onto
/// white paper, the tint where the shape was black and nothing where it was
/// white, as the GPU overlay's shader works it out.
fn tinted(rgb: [f32; 3], tint: [f32; 3]) -> [f32; 3] {
    let luminance = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
    let ink = (1.0 - luminance).clamp(0.0, 1.0);
    [0, 1, 2].map(|c| 1.0 - ink * (1.0 - tint[c]))
}

/// What a colour operator's operands mean, as RGB. `None` for a pattern, or
/// a space it can't read, which are left alone.
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

/// `content` with every colour it sets followed by that colour's tint. The
/// original operator stays where it was -- the one after it wins -- so no
/// operand is rewritten and every other byte keeps its place.
fn recolour(content: &[u8], tint: [f32; 3]) -> Vec<u8> {
    let stroking = |op: &[u8]| op.iter().all(|b| b.is_ascii_uppercase());
    let mut insert: Vec<(usize, String)> = Vec::new();
    each_operation(content, |op, operands, range| {
        // Choosing a colour space sets its default colour, black, which would
        // wipe a tint written before it: so that's tinted too.
        let rgb = if matches!(op, b"cs" | b"CS") {
            Some([0.0; 3])
        } else if matches!(op, b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K" | b"sc" | b"SC" | b"scn" | b"SCN") {
            let named = operands.last().is_some_and(|o| o.name().is_some());
            let numbers: Vec<f32> = operands.iter().filter_map(|o| o.number()).collect();
            colour_of(op, &numbers, named)
        } else {
            None
        };
        if let Some(rgb) = rgb {
            let [r, g, b] = tinted(rgb, tint);
            let set = if stroking(op) { "RG" } else { "rg" };
            insert.push((range.end, format!(" {r:.4} {g:.4} {b:.4} {set}")));
        }
    });
    let mut out = Vec::with_capacity(content.len() + insert.len() * 24);
    let mut from = 0usize;
    for (at, bytes) in &insert {
        out.extend_from_slice(&content[from..*at]);
        out.extend_from_slice(bytes.as_bytes());
        from = *at;
    }
    out.extend_from_slice(&content[from..]);
    out
}

/// Copies `object` and everything it refers to into `to`, noting the forms
/// met on the way to recolour after, and making every graphics state
/// multiply: a drawing's own say `/BM /Normal`, which would switch the
/// multiply back off partway through a sheet. Ids are reserved before
/// contents are copied, so what refers back to itself doesn't go round for
/// ever.
fn import(from: &Document, to: &mut Document, object: &Object, done: &mut HashMap<ObjectId, ObjectId>, forms: &mut Vec<ObjectId>) -> Object {
    match object {
        Object::Reference(id) => {
            if let Some(&already) = done.get(id) {
                return Object::Reference(already);
            }
            let new_id = to.new_object_id();
            done.insert(*id, new_id);
            let copied = match from.get_object(*id) {
                Ok(target) => import(from, to, target, done, forms),
                Err(_) => Object::Null,
            };
            if matches!(&copied, Object::Stream(s) if s.dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Form")) {
                forms.push(new_id);
            }
            to.objects.insert(new_id, copied);
            Object::Reference(new_id)
        }
        Object::Array(items) => Object::Array(items.iter().map(|i| import(from, to, i, done, forms)).collect()),
        Object::Dictionary(d) => Object::Dictionary(import_dict(from, to, d, done, forms)),
        Object::Stream(stream) => {
            let mut copied = stream.clone();
            copied.dict = import_dict(from, to, &stream.dict, done, forms);
            Object::Stream(copied)
        }
        other => other.clone(),
    }
}

fn import_dict(from: &Document, to: &mut Document, d: &Dictionary, done: &mut HashMap<ObjectId, ObjectId>, forms: &mut Vec<ObjectId>) -> Dictionary {
    let mut out = Dictionary::new();
    for (key, value) in d.iter() {
        out.set(key.to_vec(), import(from, to, value, done, forms));
    }
    if out.get(b"Type").and_then(Object::as_name).is_ok_and(|t| t == b"ExtGState") {
        out.set(b"BM".to_vec(), Object::Name(b"Multiply".to_vec()));
    }
    out
}

/// The box a page shows, and the quarter turns it's shown through, as
/// `page_shapes` takes them: both inherited up the page tree.
fn placed(doc: &Document, page_id: ObjectId) -> ([f32; 4], i32) {
    let inherited = |key: &[u8]| {
        let mut at = page_id;
        for _ in 0..32 {
            let page = doc.get_dictionary(at).ok()?;
            if let Some(found) = page.get(key).ok().and_then(|b| doc.dereference(b).ok()).map(|(_, b)| b) {
                return Some(found);
            }
            at = page.get(b"Parent").ok()?.as_reference().ok()?;
        }
        None
    };
    let boxed = |key: &[u8]| {
        let numbers: Vec<f32> = inherited(key)?.as_array().ok()?.iter().filter_map(|n| n.as_float().ok().or_else(|| n.as_i64().ok().map(|i| i as f32))).collect();
        let [a, b, c, d] = numbers[..] else { return None };
        Some([a.min(c), b.min(d), a.max(c), b.max(d)])
    };
    let area = match (boxed(b"MediaBox"), boxed(b"CropBox")) {
        (Some(m), Some(c)) => [m[0].max(c[0]), m[1].max(c[1]), m[2].min(c[2]), m[3].min(c[3])],
        (m, c) => c.or(m).unwrap_or([0.0, 0.0, 612.0, 792.0]),
    };
    let turns = inherited(b"Rotate").and_then(|r| r.as_i64().ok()).map_or(0, |r| ((r / 90) % 4) as i32);
    (area, turns.rem_euclid(4))
}

/// A form's matrix: the page's own space moved so its visible corner is the
/// origin, turned by `/Rotate` -- which is where `page_shapes` puts it -- and
/// then raised `up`, to put its top where a taller sheet's is.
fn matrix_for(area: [f32; 4], turns: i32, up: f32) -> Vec<Object> {
    let (across, tall) = (area[2] - area[0], area[3] - area[1]);
    let (left, bottom) = (area[0], area[1]);
    // /Rotate turns the page clockwise to be shown, content and all: 90
    // takes (x, y) to (y, across - x).
    let m: [f32; 6] = match turns {
        1 => [0.0, -1.0, 1.0, 0.0, -bottom, across + left],
        2 => [-1.0, 0.0, 0.0, -1.0, across + left, tall + bottom],
        3 => [0.0, 1.0, -1.0, 0.0, tall + bottom, -left],
        _ => [1.0, 0.0, 0.0, 1.0, -left, -bottom],
    };
    [m[0], m[1], m[2], m[3], m[4], m[5] + up].iter().map(|&v| Object::Real(v)).collect()
}

/// One of the two sets.
struct Source {
    doc: Document,
    tint: [f32; 3],
    ocg: ObjectId,
    /// Objects already brought across, so what its sheets share comes once.
    done: HashMap<ObjectId, ObjectId>,
}

/// Writes the overlay of `original` and `compared` to `out`: a page for each
/// of `rows`, each naming the page -- from 0 -- of either set drawn on it, or
/// none. Each page is as big as the larger of its two, the two laid top left
/// to top left.
pub fn write(original: &Path, compared: &Path, rows: &[[Option<usize>; 2]], out: &Path) -> Result<(), String> {
    write_aligned(original, compared, rows, &vec![None; rows.len()], out)
}

/// Writes exactly the placements shown in the comparison; neither source is changed.
pub fn write_aligned(original: &Path, compared: &Path, rows: &[[Option<usize>; 2]], alignments: &[Option<Alignment>], out: &Path) -> Result<(), String> {
    if alignments.len() != rows.len() || alignments.iter().flatten().any(|a| a.scale.iter().any(|v| !v.is_finite() || *v <= 0.0) || a.offset.iter().any(|v| !v.is_finite())) {
        return Err("Invalid comparison alignment".into());
    }
    let mut written = Document::with_version("1.7");
    let pages_id = written.new_object_id();
    let mut sources = Vec::new();
    for (n, (path, which)) in [(original, "Original"), (compared, "Compared")].into_iter().enumerate() {
        let doc = Document::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path.file_name().map_or_else(|| path.display().to_string(), |f| f.to_string_lossy().into_owned());
        let ocg = written.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal(format!("{which}: {name}")) });
        sources.push(Source { doc, tint: Tint::nth(n).colour, ocg, done: HashMap::new() });
    }

    // Each sheet's content recoloured first, all at once across the threads;
    // what follows shares objects between sheets, so goes one at a time.
    let wanted: Vec<(usize, usize)> = rows.iter().flat_map(|row| (0..2).filter_map(move |side| row[side].map(|page| (side, page)))).collect();
    let recoloured: HashMap<(usize, usize), Vec<u8>> = {
        let sources = &sources;
        in_parallel(wanted, |(side, page)| {
            let source = &sources[side];
            let content = source.doc.get_pages().get(&(page as u32 + 1)).map(|&id| recolour(&source.doc.get_page_content(id), source.tint)).unwrap_or_default();
            ((side, page), content)
        })
        .into_iter()
        .collect()
    };
    let mut recoloured = recoloured;

    let blend = written.add_object(dictionary! { "Type" => "ExtGState", "BM" => "Multiply", "CA" => 1, "ca" => 1 });
    let mut forms_to_tint: Vec<(ObjectId, [f32; 3])> = Vec::new();
    let mut kids: Vec<Object> = Vec::new();
    for (row, alignment) in rows.iter().zip(alignments) {
        let sizes: Vec<Option<[f32; 2]>> = (0..2).map(|side| row[side].and_then(|page| page_size(&sources[side].doc, page as u32 + 1).ok())).collect();
        let layout = overlay_layout([sizes[0].unwrap_or([0.0; 2]), sizes[1].unwrap_or([0.0; 2])], *alignment);
        let [width, height] = layout.size;
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        let (mut xobjects, mut properties, mut content) = (Dictionary::new(), Dictionary::new(), String::new());
        for side in 0..2 {
            let (Some(page), Some(size)) = (row[side], sizes[side]) else { continue };
            let source = &mut sources[side];
            let Some(&page_id) = source.doc.get_pages().get(&(page as u32 + 1)) else { continue };
            let (area, turns) = placed(&source.doc, page_id);
            let mut brought = Vec::new();
            let resources = match source.doc.get_dictionary(page_id).ok().and_then(|p| p.get(b"Resources").ok()) {
                Some(r) => import(&source.doc, &mut written, r, &mut source.done, &mut brought),
                None => Object::Dictionary(Dictionary::new()),
            };
            forms_to_tint.extend(brought.into_iter().map(|id| (id, source.tint)));
            let form = written.add_object(Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Form",
                    "FormType" => 1,
                    "BBox" => area.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
                    "Matrix" => matrix_for(area, turns, 0.0),
                    "Resources" => resources,
                },
                recoloured.remove(&(side, page)).unwrap_or_default(),
            ));
            let (name, oc) = (format!("Fm{side}"), format!("OC{side}"));
            xobjects.set(name.as_bytes().to_vec(), Object::Reference(form));
            properties.set(oc.as_bytes().to_vec(), Object::Reference(source.ocg));
            let [sx, sy] = layout.scales[side];
            let [x, top] = layout.origins[side];
            let y = height - top - size[1] * sy;
            content += &format!("/OC /{oc} BDC q {sx} 0 0 {sy} {x} {y} cm /GSm gs /{name} Do Q EMC\n");
        }
        let contents = written.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        kids.push(Object::Reference(written.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "MediaBox" => vec![Object::Real(0.0), Object::Real(0.0), Object::Real(width), Object::Real(height)],
            "Resources" => dictionary! {
                "XObject" => xobjects,
                "Properties" => properties,
                "ExtGState" => dictionary! { "GSm" => Object::Reference(blend) },
            },
            "Contents" => Object::Reference(contents),
        })));
    }
    if kids.is_empty() {
        return Err("there are no sheets to lay over each other".into());
    }
    retint_forms(&mut written, &forms_to_tint);

    let count = kids.len() as i64;
    written.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let groups: Vec<Object> = sources.iter().map(|s| Object::Reference(s.ocg)).collect();
    let catalog = written.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
        "OCProperties" => dictionary! {
            "OCGs" => groups.clone(),
            "D" => dictionary! { "Order" => groups.clone(), "ON" => groups.clone() },
        },
        MARKER => dictionary! { "Layers" => groups },
    });
    written.trailer.set("Root", Object::Reference(catalog));
    deflate_streams(&mut written);
    written.save(out).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(())
}

/// Recolours the forms `import` noted, each inflated, rewritten and left
/// plain for `deflate_streams`, across the threads.
fn retint_forms(out: &mut Document, forms: &[(ObjectId, [f32; 3])]) {
    let mut jobs = Vec::new();
    for &(id, tint) in forms {
        if let Some(Object::Stream(stream)) = out.objects.get_mut(&id) {
            jobs.push((id, tint, std::mem::replace(stream, Stream::new(Dictionary::new(), Vec::new()))));
        }
    }
    for (id, stream) in in_parallel(jobs, |(id, tint, stream)| match stream.decompressed_content() {
        Ok(content) => {
            let mut plain = stream.dict;
            plain.remove(b"Filter");
            plain.remove(b"DecodeParms");
            (id, Stream::new(plain, recolour(&content, tint)))
        }
        // Not readable: left exactly as it came.
        Err(_) => (id, stream),
    }) {
        out.objects.insert(id, Object::Stream(stream));
    }
}

/// Deflates every stream without a filter -- what was recoloured -- across
/// the threads. Streams brought across untouched keep the filter they had.
fn deflate_streams(out: &mut Document) {
    let mut jobs: Vec<(ObjectId, Vec<u8>)> = Vec::new();
    for (id, object) in out.objects.iter_mut() {
        if let Object::Stream(stream) = object {
            if stream.dict.get(b"Filter").is_err() && stream.content.len() > 512 {
                jobs.push((*id, std::mem::take(&mut stream.content)));
            }
        }
    }
    for (id, content, deflated) in in_parallel(jobs, |(id, content)| {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
        match encoder.write_all(&content).and_then(|_| encoder.finish()) {
            Ok(squeezed) if squeezed.len() < content.len() => (id, squeezed, true),
            _ => (id, content, false),
        }
    }) {
        if let Some(Object::Stream(stream)) = out.objects.get_mut(&id) {
            stream.set_content(content);
            if deflated {
                stream.dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
            }
        }
    }
}

/// `work` over `items` on every thread the machine has, each taking the next
/// one free rather than a fixed share -- a few sheets of a set are many
/// times the rest -- and the results in the order of `items`.
fn in_parallel<T: Send, R: Send>(items: Vec<T>, work: impl Fn(T) -> R + Sync) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    let count = items.len();
    let slots: Vec<Mutex<Option<T>>> = items.into_iter().map(|item| Mutex::new(Some(item))).collect();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(count.max(1));
    let mut got: Vec<(usize, R)> = Vec::with_capacity(count);
    std::thread::scope(|scope| {
        let running: Vec<_> = (0..threads)
            .map(|_| {
                let (slots, next, work) = (&slots, &next, &work);
                scope.spawn(move || {
                    let mut mine = Vec::new();
                    loop {
                        let n = next.fetch_add(1, Ordering::Relaxed);
                        if n >= count {
                            break;
                        }
                        let Some(item) = slots[n].lock().ok().and_then(|mut slot| slot.take()) else { continue };
                        mine.push((n, work(item)));
                    }
                    mine
                })
            })
            .collect();
        for thread in running {
            got.extend(thread.join().unwrap_or_default());
        }
    });
    got.sort_by_key(|(n, _)| *n);
    got.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centred_alignment_keeps_both_sheets_inside_the_output() {
        let sizes = [[100.0, 200.0], [200.0, 400.0]];
        let aligned = overlay_layout(sizes, Some(Alignment { scale: [0.5; 2], offset: [-0.25, 0.1] }));
        assert_eq!(aligned.size, [125.0, 220.0]);
        assert_eq!(aligned.origins, [[25.0, 0.0], [0.0, 20.0]]);
        assert_eq!(aligned.scales, [[1.0; 2], [0.5; 2]]);
        let plain = overlay_layout(sizes, None);
        assert_eq!(plain.size, [200.0, 400.0]);
        assert_eq!(plain.origins, [[0.0; 2]; 2]);
        let alone = overlay_layout([[0.0; 2], sizes[1]], Some(Alignment { scale: [0.5; 2], offset: [-0.25, 0.1] }));
        assert_eq!(alone.size, sizes[1], "a missing partner must not shrink or move a sheet");
    }

    #[test]
    fn document_offsets_follow_the_reference_paper_size() {
        let alignment = Some(Alignment { scale: [0.75; 2], offset: [0.1, -0.2] });
        let sizes = [[100.0, 200.0], [120.0, 240.0]];
        let first = overlay_layout(sizes, alignment);
        let double = overlay_layout(sizes.map(|s| s.map(|v| v * 2.0)), alignment);
        assert_eq!(double.size, first.size.map(|v| v * 2.0));
        assert_eq!(double.origins, first.origins.map(|s| s.map(|v| v * 2.0)));
    }

    #[test]
    fn a_colour_is_tinted_as_much_as_it_is_dark_and_follows_the_one_it_replaces() {
        let red = Tint::nth(0).colour;
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(close(tinted([0.0; 3], red), red), "black takes the tint whole");
        assert_eq!(tinted([1.0; 3], red), [1.0; 3], "white stays paper");
        let out = String::from_utf8(recolour(b"0 0 0 RG 1 w 0.5 g", [0.0, 0.0, 1.0])).unwrap();
        assert!(out.starts_with("0 0 0 RG 0.0000 0.0000 1.0000 RG 1 w 0.5 g"), "{out}");
        assert!(out.ends_with(" rg"), "the fill tinted as well: {out}");
    }
}

#[cfg(test)]
mod written {
    use super::*;

    #[test]
    fn aligned_export_matches_preview_geometry_for_a_rotated_cropped_sheet() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        let b = dir.path().join("b.pdf");
        let out = dir.path().join("aligned.pdf");
        let mut source = Document::with_version("1.7");
        let pages = source.new_object_id();
        let content = source.add_object(Stream::new(Dictionary::new(), b"0 0 0 RG 2 w 50 50 m 100 80 l S".to_vec()));
        let page = source.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages,
            "MediaBox" => vec![10.into(), 20.into(), 210.into(), 120.into()],
            "CropBox" => vec![30.into(), 30.into(), 190.into(), 110.into()],
            "Rotate" => 90, "Resources" => Dictionary::new(), "Contents" => content,
        });
        source.objects.insert(pages, dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into());
        let root = source.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        source.trailer.set("Root", root);
        source.save(&a).unwrap();
        std::fs::copy(&a, &b).unwrap();
        let size = page_size(&source, 1).unwrap();
        let alignment = Alignment { scale: [1.5, 0.75], offset: [-0.7, 0.8] };
        let layout = overlay_layout([size; 2], Some(alignment));
        write_aligned(&a, &b, &[[Some(0), Some(0)]], &[Some(alignment)], &out).unwrap();
        let written = Document::load(&out).unwrap();
        assert_eq!(page_size(&written, 1).unwrap(), layout.size);
        let raw = gpu_lines::page_shapes(&source, 1, 0.05, 1.0).unwrap();
        let exported = gpu_lines::page_shapes(&written, 1, 0.05, 1.0).unwrap();
        for side in 0..2 {
            let primitives: Vec<_> = exported.runs.iter().filter(|r| r.layer == side as u16 + 1)
                .flat_map(|r| &exported.primitives[r.start..r.start + r.len]).collect();
            assert_eq!(primitives.len(), raw.primitives.len());
            for (actual, original) in primitives.iter().zip(&raw.primitives) {
                for (actual, point) in actual.points.iter().zip(original.points) {
                    let expected = [
                        layout.origins[side][0] + point[0] * layout.scales[side][0],
                        layout.size[1] - layout.origins[side][1] - (size[1] - point[1]) * layout.scales[side][1],
                    ];
                    assert!((actual[0] - expected[0]).abs() < 0.001 && (actual[1] - expected[1]).abs() < 0.001,
                        "side {side}: {actual:?} != {expected:?}");
                }
            }
        }
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap(), "the source PDFs are unchanged");
        assert!(write_aligned(&a, &b, &[[Some(0), Some(0)]], &[], &out).is_err());
        assert!(write_aligned(&a, &b, &[[Some(0), Some(0)]], &[Some(Alignment { scale: [f32::NAN; 2], offset: [0.0; 2] })], &out).is_err());
    }

    #[test]
    fn an_overlay_is_a_page_a_row_its_sets_on_layers_the_renderer_tells_apart() {
        let dir = std::env::temp_dir().join(format!("kpdf-overlay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, out) = (dir.join("a.pdf"), dir.join("b.pdf"), dir.join("over.pdf"));
        std::fs::write(&a, pdf_content::fixtures::stamped_pdf()).unwrap();
        std::fs::write(&b, pdf_content::fixtures::stamped_pdf()).unwrap();
        // Both sets on the first row; the original alone, opposite a blank,
        // on the second.
        write(&a, &b, &[[Some(0), Some(0)], [Some(0), None]], &out).unwrap();

        let doc = Document::load(&out).unwrap();
        assert_eq!(doc.get_pages().len(), 2, "a page a row");
        let [original, compared] = compare_layers(&doc).expect("marked as an overlay, its layers named");
        let both = gpu_lines::page_shapes(&doc, 1, 0.05, 1.0).unwrap();
        assert_eq!(both.layers, vec![original, compared], "each set drawn on its own layer");
        assert!(both.runs.iter().any(|r| r.layer == 1) && both.runs.iter().any(|r| r.layer == 2));
        let alone = gpu_lines::page_shapes(&doc, 2, 0.05, 1.0).unwrap();
        assert_eq!(alone.layers, vec![original], "only the original opposite a blank");
        assert!(compare_layers(&Document::load_mem(&pdf_content::fixtures::stamped_pdf()).unwrap()).is_none(), "any other PDF isn't one");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
