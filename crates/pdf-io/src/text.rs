//! Text boxes written as FreeText annotations: an appearance drawn from the
//! same layout the app draws them with, in the fonts they're set in,
//! embedded so every viewer shows the same thing; a callout line to where an
//! arrow points; the plain words in /Contents for anything that reads only
//! those; and the words with their formats in /KPDF, so the box comes back
//! as it was typed.
//!
//! Each font is embedded once a file: a save looks for the program in the
//! file before putting it in again, so boxes written on later saves share it.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use markup_model::{callout_start, quantities, Frame, Geometry, Markup, Pt, TextBox};
use pdf_content::lopdf::{dictionary, Dictionary, IncrementalDocument, Object, ObjectId, Stream};
use text_layout::{catalogue, layout, Face};

use crate::values::{name, pdf_date, real, reals, text};
use crate::Error;

/// Font programs in the file, by the font file they came from: put in by
/// this save, or found from an earlier one.
#[derive(Default)]
pub(crate) struct Embedded {
    programs: HashMap<PathBuf, ObjectId>,
    /// The file's own, by their tag, looked for once a save.
    in_file: Option<HashMap<Vec<u8>, ObjectId>>,
}

/// What each embedded program is tagged with, to find it again.
const TAG: &[u8] = b"KPDFFont";

/// A text box as a FreeText annotation, on page `page`.
pub(crate) fn text_annotation(update: &mut IncrementalDocument, embedded: &mut Embedded, m: &Markup, page: ObjectId, now_ms: i64) -> Result<Dictionary, Error> {
    let words = m.extras.text.as_ref().ok_or_else(|| Error::Invalid("a text box without its text".into()))?;
    let Geometry::Polygon { pts: corners, .. } = &m.geometry else {
        return Err(Error::Invalid("a text box whose geometry isn't its corners".into()));
    };
    let frame = Frame::of(corners).ok_or_else(|| Error::Invalid("a text box with no size".into()))?;
    let drawn = drawing(words, m, &frame);

    // Each face's font, as the appearance names it.
    let mut fonts = Dictionary::new();
    for (at, used) in drawn.faces.iter().enumerate() {
        let font = font(update, embedded, used)?;
        fonts.set(format!("F{at}"), font);
    }
    let mut resources = dictionary! { "ExtGState" => drawn.states.clone() };
    if !fonts.is_empty() {
        resources.set("Font", fonts);
    }
    let [left, bottom, right, top] = drawn.bbox;
    let mut form = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => reals([left, bottom, right, top]),
            "Matrix" => reals(frame.matrix()),
            "Resources" => resources,
        },
        drawn.content.into_bytes(),
    );
    let _ = form.compress();
    let form = update.new_document.add_object(form);

    // The box, and everything drawn, in user space.
    let placed: Vec<Pt> = [(left, bottom), (right, bottom), (right, top), (left, top)].iter().map(|&(x, y)| frame.to_user(x, y)).collect();
    let rect = markup_model::Rect::around(&placed).ok_or_else(|| Error::Invalid("a text box with no size".into()))?;
    let inner = markup_model::Rect::around(corners).ok_or_else(|| Error::Invalid("a text box with no size".into()))?;
    let created = m.meta.created_ms.unwrap_or(now_ms);
    let mut d = dictionary! {
        "Type" => "Annot",
        "Subtype" => "FreeText",
        "Rect" => reals([rect.min.x, rect.min.y, rect.max.x, rect.max.y]),
        "P" => page,
        "NM" => text(&m.extras.foreign_nm.clone().unwrap_or_else(|| m.id.to_nm())),
        "T" => text(&m.meta.author),
        "Subj" => text(if m.meta.subject.is_empty() { "Text box" } else { &m.meta.subject }),
        "Contents" => text(&words.text()),
        "CreationDate" => pdf_date(created),
        "M" => pdf_date(m.meta.modified_ms.unwrap_or(now_ms)),
        // Printed.
        "F" => 4,
        "C" => reals(m.style.stroke.map(f64::from)),
        "CA" => real(f64::from(m.style.opacity)),
        // What a viewer that draws the box itself sets new text in.
        "DA" => Object::string_literal(format!("0 g /Helv {} Tf", n(words.format().size))),
        "AP" => dictionary! { "N" => form },
        // How far the box is inside /Rect: left, top, right, bottom.
        "RD" => reals([inner.min.x - rect.min.x, rect.max.y - inner.max.y, rect.max.x - inner.max.x, inner.min.y - rect.min.y]),
    };
    if let Some(fill) = m.style.fill {
        d.set("IC", reals(fill.map(f64::from)));
    }
    let mut border = dictionary! { "Type" => "Border", "W" => real(m.style.width) };
    if m.style.dash.is_empty() {
        border.set("S", name("S"));
    } else {
        border.set("S", name("D"));
        border.set("D", reals(m.style.dash.iter().copied()));
    }
    d.set("BS", border);
    if let (Some(tip), Some((x, y))) = (words.callout, drawn.callout_from) {
        let from = frame.to_user(x, y);
        d.set("IT", name("FreeTextCallout"));
        d.set("CL", reals([tip.x, tip.y, from.x, from.y]));
        d.set("LE", name("ClosedArrow"));
    }
    let result = quantities(m, None);
    let mut kpdf = crate::write::kpdf(m, &result, None, None);
    kpdf.set("Corners", crate::values::points(corners));
    let json = serde_json::to_string(words).map_err(|e| Error::Invalid(format!("the text: {e}")))?;
    kpdf.set("Text", text(&json));
    d.set("KPDF", kpdf);
    for (key, value) in &m.extras.raw {
        if !d.has(key) {
            d.set(key.clone(), crate::values::from_raw(value));
        }
    }
    Ok(d)
}

/// A face used in a box, and the glyphs used of it: each one's width in
/// ems, and the character it stands for.
struct Used {
    face: Arc<Face>,
    glyphs: BTreeMap<u16, (f32, char)>,
}

/// What a box's appearance draws, in the box's own space: its content, the
/// faces and graphics states it names, its box, and where its arrow leaves
/// it.
struct Drawn {
    content: String,
    faces: Vec<Used>,
    states: Dictionary,
    bbox: [f64; 4],
    callout_from: Option<(f64, f64)>,
}

/// The appearance of box `words`, styled as `m`, in the box's own space.
fn drawing(words: &TextBox, m: &Markup, frame: &Frame) -> Drawn {
    let (w, h) = (frame.width, frame.height);
    let style = &m.style;
    let mut ops = String::new();
    let mut states = Dictionary::new();
    states.set("GSline", dictionary! { "Type" => "ExtGState", "CA" => real(f64::from(style.opacity)), "ca" => real(f64::from(style.opacity)) });
    states.set("GSfill", dictionary! { "Type" => "ExtGState", "ca" => real(f64::from(style.fill_opacity)) });

    // The background, under everything.
    if let Some([r, g, b]) = style.fill {
        let _ = writeln!(ops, "q /GSfill gs {} {} {} rg 0 0 {} {} re f Q", n(r.into()), n(g.into()), n(b.into()), n(w), n(h));
    }

    // The text, set as the app sets it, and kept within the box.
    let pad = words.padding.max(0.0);
    let laid = layout(words, (w - 2.0 * pad).max(1.0) as f32, (h - 2.0 * pad).max(1.0) as f32, catalogue());
    let mut faces: Vec<Used> = Vec::new();
    let _ = writeln!(ops, "q 0 0 {} {} re W n", n(w), n(h));
    for line in &laid.lines {
        for word in &line.words {
            let at = match faces.iter().position(|u| Arc::ptr_eq(&u.face, &word.face)) {
                Some(at) => at,
                None => {
                    faces.push(Used { face: Arc::clone(&word.face), glyphs: BTreeMap::new() });
                    faces.len() - 1
                }
            };
            let mut codes = String::new();
            for ((glyph, advance), ch) in word.glyphs.iter().zip(word.text.chars()) {
                let _ = write!(codes, "{glyph:04X}");
                faces[at].glyphs.entry(*glyph).or_insert((*advance / word.size.max(f32::EPSILON), ch));
            }
            let (x, y) = (pad + f64::from(word.x), h - pad - f64::from(line.baseline));
            let [r, g, b] = word.colour.map(f64::from);
            let colour = format!("{} {} {}", n(r), n(g), n(b));
            // A face's bold or italic that isn't installed is made up: the
            // outline stroked as well as filled, the letters leant over.
            let bold = if word.fake_bold { format!(" 2 Tr {} w {colour} RG", n(f64::from(word.size) * 0.03)) } else { String::new() };
            let lean = if word.fake_italic { 0.21 } else { 0.0 };
            let _ = writeln!(ops, "BT /F{at} {} Tf {colour} rg{bold} 1 0 {lean} 1 {} {} Tm <{codes}> Tj ET", n(word.size.into()), n(x), n(y));
            if word.underline {
                let face = &word.face;
                let (offset, thickness) = (f64::from(face.underline_offset * word.size), f64::from(face.underline_thickness * word.size));
                let _ = writeln!(ops, "{colour} rg {} {} {} {} re f", n(x), n(y + offset - thickness / 2.0), n(f64::from(word.width)), n(thickness));
            }
        }
    }
    ops.push_str("Q\n");

    // The border, inside the box's edge, and the arrow out of it.
    let [r, g, b] = style.stroke.map(f64::from);
    let line_colour = format!("{} {} {}", n(r), n(g), n(b));
    let dash = if style.dash.is_empty() { String::new() } else { format!(" [{}] 0 d", style.dash.iter().map(|&d| n(d)).collect::<Vec<_>>().join(" ")) };
    if style.width > 0.0 {
        let half = style.width / 2.0;
        let _ = writeln!(ops, "q /GSline gs {line_colour} RG {} w{dash} {} {} {} {} re S Q", n(style.width), n(half), n(half), n(w - style.width), n(h - style.width));
    }
    let mut bbox = [0.0, 0.0, w, h];
    let mut callout_from = None;
    if let Some(tip) = words.callout {
        let (tx, ty) = frame.to_box(tip);
        let (sx, sy) = callout_start(w, h, (tx, ty));
        callout_from = Some((sx, sy));
        let width = style.width.max(0.75);
        let (dx, dy) = (tx - sx, ty - sy);
        let length = dx.hypot(dy).max(f64::EPSILON);
        let (ux, uy) = (dx / length, dy / length);
        let head = (width * 4.0).max(6.0);
        let (bx, by) = (tx - ux * head, ty - uy * head);
        let (px, py) = (-uy * head * 0.4, ux * head * 0.4);
        let _ = writeln!(ops, "q /GSline gs {line_colour} RG {line_colour} rg {} w 1 J {} {} m {} {} l S", n(width), n(sx), n(sy), n(bx), n(by));
        let _ = writeln!(ops, "{} {} m {} {} l {} {} l h f Q", n(tx), n(ty), n(bx + px), n(by + py), n(bx - px), n(by - py));
        let reach = head + width;
        bbox = [bbox[0].min(tx - reach), bbox[1].min(ty - reach), bbox[2].max(tx + reach), bbox[3].max(ty + reach)];
    }
    Drawn { content: ops, faces, states, bbox, callout_from }
}

/// A Type 0 font for the glyphs of `used`, its program embedded -- once a
/// file -- with the widths of the glyphs used and what characters they are.
fn font(update: &mut IncrementalDocument, embedded: &mut Embedded, used: &Used) -> Result<ObjectId, Error> {
    let face = &used.face;
    let program = program(update, embedded, face);
    let em = |v: f32| real(f64::from(v * 1000.0));
    let postscript = Object::Name(face.entry.postscript.replace(' ', "").into_bytes());
    let mut flags = 32;
    if face.entry.italic {
        flags += 64;
    }
    let mut descriptor = dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => postscript.clone(),
        "Flags" => flags,
        "FontBBox" => Object::Array(vec![em(-0.5), em(face.descent), em(1.5), em(face.ascent)]),
        "ItalicAngle" => if face.entry.italic { -12 } else { 0 },
        "Ascent" => em(face.ascent),
        "Descent" => em(face.descent),
        "CapHeight" => em(face.ascent * 0.9),
        "StemV" => 80,
    };
    descriptor.set(if face.entry.cff { "FontFile3" } else { "FontFile2" }, program);
    let descriptor = update.new_document.add_object(descriptor);

    // Each glyph's width, in thousandths of an em, by glyph: CIDs are glyphs.
    let widths: Vec<Object> = used.glyphs.iter().flat_map(|(&glyph, &(width, _))| [Object::Integer(i64::from(glyph)), Object::Array(vec![em(width)])]).collect();
    let mut cid_font = dictionary! {
        "Type" => "Font",
        "Subtype" => if face.entry.cff { "CIDFontType0" } else { "CIDFontType2" },
        "BaseFont" => postscript.clone(),
        "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
        "FontDescriptor" => descriptor,
        "DW" => 1000,
        "W" => widths,
    };
    if !face.entry.cff {
        cid_font.set("CIDToGIDMap", name("Identity"));
    }
    let cid_font = update.new_document.add_object(cid_font);
    let mut to_unicode = Stream::new(Dictionary::new(), to_unicode(&used.glyphs).into_bytes());
    let _ = to_unicode.compress();
    let to_unicode = update.new_document.add_object(to_unicode);
    Ok(update.new_document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => postscript,
        "Encoding" => name("Identity-H"),
        "DescendantFonts" => vec![Object::Reference(cid_font)],
        "ToUnicode" => to_unicode,
    }))
}

/// The font program of `face`, as an object of the file: one this save put
/// in, or an earlier save did, or else put in now.
fn program(update: &mut IncrementalDocument, embedded: &mut Embedded, face: &Face) -> ObjectId {
    if let Some(&found) = embedded.programs.get(&face.entry.path) {
        return found;
    }
    let tag = format!("{} {}", face.entry.postscript, face.data.len()).into_bytes();
    let in_file = embedded.in_file.get_or_insert_with(|| {
        update
            .get_prev_documents()
            .objects
            .iter()
            .filter_map(|(&id, object)| {
                let tag = object.as_stream().ok()?.dict.get(TAG).ok()?.as_str().ok()?;
                Some((tag.to_vec(), id))
            })
            .collect()
    });
    let id = match in_file.get(&tag) {
        Some(&id) => id,
        None => {
            let mut dict = dictionary! { TAG => Object::string_literal(tag.clone()) };
            if face.entry.cff {
                dict.set("Subtype", name("OpenType"));
            } else {
                dict.set("Length1", face.data.len() as i64);
            }
            let mut stream = Stream::new(dict, face.data.to_vec());
            let _ = stream.compress();
            let id = update.new_document.add_object(stream);
            in_file.insert(tag, id);
            id
        }
    };
    embedded.programs.insert(face.entry.path.clone(), id);
    id
}

/// A ToUnicode CMap saying which character each glyph used stands for, so
/// the text copies and searches as the words it is.
fn to_unicode(glyphs: &BTreeMap<u16, (f32, char)>) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries: Vec<(&u16, &(f32, char))> = glyphs.iter().collect();
    for chunk in entries.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        for (glyph, (_, ch)) in chunk {
            let units: String = ch.encode_utf16(&mut [0; 2]).iter().map(|u| format!("{u:04X}")).collect();
            let _ = writeln!(cmap, "<{glyph:04X}> <{units}>");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap
}

/// A number in a content stream, to a thousandth.
fn n(v: f64) -> String {
    let fixed = format!("{v:.3}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    if trimmed == "-0" || trimmed.is_empty() { "0".to_owned() } else { trimmed.to_owned() }
}
