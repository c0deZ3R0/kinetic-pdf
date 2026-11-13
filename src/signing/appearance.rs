//! Visible cryptographic signature widget geometry, trusted vector mark and fonts.
//!
//! PATH: src/signing/appearance.rs
//! CONSUMED BY: signing::pdf in the isolated helper, before ByteRange hashing.
//! One per-signing-operation resource cache shares embedded fonts across blocks.
//! Coordinates are absolute unrotated PDF user space; appearance form matrices
//! counter page rotation. Only the compile-time K mark is accepted, never user SVG.
//! Font programs/CMaps preserve actual certificate Unicode; missing glyphs or
//! unreadably small fitting are errors, not transliteration or signer substitution.
//! Preferred detail size follows displayed box area (6–48 pt), then shrinks to fit.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;
use markup_model::{HAlign, Paragraph, Run, RunFormat, TextBox, VAlign};
use pdf_content::lopdf::{dictionary, Dictionary, Document, IncrementalDocument, Object, ObjectId, Stream, StringFormat};
use text_layout::{catalogue, Face};
use super::{IdentityInfo, SignatureAppearance, SignatureMode};

const WATERMARK: &str = include_str!("../../assets/signature-watermark.svg");
const MAX_TEXT_CHARS: usize = 8192;
const MIN_TEXT_SIZE: f32 = 6.0;
const MAX_TEXT_SIZE: f32 = 48.0;
const PADDING: f32 = 4.0;
const MAX_FONTS: usize = 8;

#[derive(Debug)]
pub(crate) struct Placement {
    pub page: ObjectId,
    pub rect: [f64; 4],
    pub size: [f64; 2],
    pub matrix: [f64; 6],
}

fn resolve<'a>(doc: &'a Document, value: &'a Object) -> Result<&'a Object, String> {
    doc.dereference(value).map(|(_, v)| v).map_err(|_| "Unresolved signature page geometry.".into())
}
fn inherited<'a>(doc: &'a Document, page: ObjectId, key: &[u8]) -> Result<Option<&'a Object>, String> {
    let mut at = page; let mut seen = HashSet::new();
    for _ in 0..64 {
        if !seen.insert(at) { return Err("Cyclic signature page inheritance.".into()); }
        let dict = doc.get_dictionary(at).map_err(|_| "Malformed signature page inheritance.")?;
        if let Ok(value) = dict.get(key) { return resolve(doc, value).map(Some); }
        match dict.get(b"Parent").ok() {
            None => return Ok(None),
            Some(parent) => at = parent.as_reference().map_err(|_| "Invalid page Parent reference.")?,
        }
    }
    Err("Signature page inheritance limit exceeded.".into())
}
fn number(value: &Object) -> Result<f64, String> {
    let n = match value { Object::Integer(i) => *i as f64, Object::Real(f) => f64::from(*f), _ => return Err("Page geometry must contain numbers.".into()) };
    if !n.is_finite() || n.abs() > 10_000_000.0 { return Err("Nonfinite/unsupported page geometry coordinate.".into()); }
    Ok(n)
}
fn page_box(doc: &Document, value: &Object) -> Result<[f64; 4], String> {
    let a = value.as_array().map_err(|_| "Page box must be an array.")?;
    if a.len() != 4 { return Err("Page box must have four coordinates.".into()); }
    let a = [number(resolve(doc, &a[0])?)?, number(resolve(doc, &a[1])?)?, number(resolve(doc, &a[2])?)?, number(resolve(doc, &a[3])?)?];
    let rect = [a[0].min(a[2]), a[1].min(a[3]), a[0].max(a[2]), a[1].max(a[3])];
    if rect[0] >= rect[2] || rect[1] >= rect[3] { return Err("Empty PDF page box.".into()); }
    Ok(rect)
}

pub(crate) fn placement(doc: &Document, spec: &SignatureAppearance) -> Result<Placement, String> {
    if !matches!(spec.rotation, 0 | 90 | 180 | 270) { return Err("Signature rotation must be 0, 90, 180 or 270 degrees, not quarter turns.".into()); }
    let page = *doc.get_pages().values().nth(spec.page).ok_or("Signature page is outside the saved PDF.")?;
    if let Some(unit) = inherited(doc, page, b"UserUnit")? {
        if (number(unit)? - 1.0).abs() > 0.000001 { return Err("Visible signatures on pages with UserUnit other than 1 are not supported.".into()); }
    }
    let rotation = match inherited(doc, page, b"Rotate")? {
        None => 0,
        Some(value) => value.as_i64().map_err(|_| "PDF page rotation must be an integer.")?,
    };
    if rotation % 90 != 0 || rotation.rem_euclid(360) != i64::from(spec.rotation) {
        return Err("Signature page rotation differs from the saved PDF; reopen/reselect the box.".into());
    }
    let media = page_box(doc, inherited(doc, page, b"MediaBox")?.ok_or("Saved page has no MediaBox.")?)?;
    let crop = match inherited(doc, page, b"CropBox")? { Some(b) => page_box(doc, b)?, None => media };
    let visible = [media[0].max(crop[0]), media[1].max(crop[1]), media[2].min(crop[2]), media[3].min(crop[3])];
    if visible[0] >= visible[2] || visible[1] >= visible[3] { return Err("CropBox/MediaBox have no visible intersection.".into()); }
    let mut rect = spec.rect.map(f64::from);
    if rect.iter().any(|n| !n.is_finite() || n.abs() > 10_000_000.0) || rect[0] >= rect[2] || rect[1] >= rect[3] {
        return Err("Signature box must be finite, ordered and nonempty.".into());
    }
    if rect[0] < visible[0] - 0.01 || rect[1] < visible[1] - 0.01 || rect[2] > visible[2] + 0.01 || rect[3] > visible[3] + 0.01 {
        return Err("Signature box is outside the saved page's visible CropBox/MediaBox intersection.".into());
    }
    // Account only for tiny f32/native geometry rounding, never silently move a
    // materially out-of-page box into place.
    rect = [rect[0].max(visible[0]), rect[1].max(visible[1]), rect[2].min(visible[2]), rect[3].min(visible[3])];
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let (size, matrix) = orientation(w, h, spec.rotation);
    if size[0] < 96.0 || size[1] < 40.0 { return Err("Signature box is too small; draw at least 96 by 40 points as displayed.".into()); }
    Ok(Placement { page, rect, size, matrix })
}
fn orientation(w: f64, h: f64, degrees: u16) -> ([f64; 2], [f64; 6]) {
    match degrees {
        90 => ([h, w], [0.0, 1.0, -1.0, 0.0, w, 0.0]),
        180 => ([w, h], [-1.0, 0.0, 0.0, -1.0, w, h]),
        270 => ([h, w], [0.0, -1.0, 1.0, 0.0, 0.0, h]),
        _ => ([w, h], [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
    }
}
fn reals(values: impl IntoIterator<Item = f64>) -> Object { Object::Array(values.into_iter().map(|n| Object::Real(n as f32)).collect()) }
pub(crate) fn pdf_text(text: &str) -> Object {
    let mut bytes = vec![0xfe, 0xff];
    bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    Object::String(bytes, StringFormat::Hexadecimal)
}

/// Shared preview/output preference in displayed PDF points, not screen pixels.
/// Doubling both dimensions doubles type size; metadata may require shrinking.
pub(crate) fn preferred_text_size(width_points: f32, height_points: f32) -> f32 {
    (10.0 * (width_points.max(0.0) * height_points.max(0.0) / (220.0 * 80.0)).sqrt()).clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE)
}
fn text_box(subject: &str, date: &str, reason: &str, mode: SignatureMode, size: f32) -> Result<TextBox, String> {
    let title = if mode == SignatureMode::CertifyNoChanges { "Digitally certified (no changes)" } else { "Digitally signed" };
    let mut text = format!("{title}\nSigned by: {subject}\nDate (UTC): {date}");
    if !reason.is_empty() { let _ = write!(text, "\nReason: {reason}"); }
    if text.chars().count() > MAX_TEXT_CHARS { return Err("Certificate details exceed the visible signature text limit.".into()); }
    if text.chars().any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t')) { return Err("Certificate details contain unsupported control characters.".into()); }
    let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
    let fonts = catalogue();
    let mut families = vec!["Arial".to_owned(), "Segoe UI".to_owned(), "Noto Sans".to_owned(), "DejaVu Sans".to_owned(), "Liberation Sans".to_owned()];
    families.extend(fonts.families().into_iter().take(64));
    let mut faces: Vec<Arc<Face>> = Vec::new();
    for family in families {
        if !fonts.has(&family) { continue; }
        if let Some(face) = fonts.face(&family, false, false) {
            if !face.entry.cff && !faces.iter().any(|f| f.entry.path == face.entry.path) { faces.push(face); }
        }
    }
    if faces.is_empty() { return Err("No embeddable TrueType font is installed for a visible signature.".into()); }
    let mut chosen = HashMap::<char, usize>::new();
    let mut paragraphs = vec![];
    for line in text.split('\n') {
        let mut runs: Vec<Run> = vec![];
        for ch in line.chars() {
            let at = match chosen.get(&ch) {
                Some(at) => *at,
                None => {
                    let at = faces.iter().position(|f| f.glyph(ch).0 != 0).ok_or_else(|| format!("No embeddable font covers U+{:04X}; visible signer text was not substituted.", ch as u32))?;
                    chosen.insert(ch, at); at
                }
            };
            let family = &faces[at].entry.family;
            if let Some(run) = runs.last_mut().filter(|r| r.format.font == *family) { run.text.push(ch); }
            else { runs.push(Run { text: ch.to_string(), format: RunFormat { font: family.clone(), size: f64::from(size), colour: [0.10, 0.13, 0.16], ..Default::default() } }); }
        }
        paragraphs.push(Paragraph { runs, align: HAlign::Left });
    }
    Ok(TextBox { paragraphs, valign: VAlign::Middle, padding: 0.0, fit: false, callout: None })
}
fn within_bounds(laid: &text_layout::Laid, width: f32, height: f32) -> bool {
    !laid.lines.is_empty() && laid.height.is_finite() && laid.height <= height + 0.02
        && laid.lines.iter().flat_map(|l| &l.words).all(|word| word.size.is_finite() && word.x.is_finite()
            && word.x >= -0.02 && word.x + word.width <= width + 0.02)
}
fn fit_details(subject: &str, date: &str, reason: &str, mode: SignatureMode, w: f32, h: f32) -> Result<text_layout::Laid, String> {
    let preferred = preferred_text_size(w, h);
    let (width, height) = (w - 2.0 * PADDING, h - 2.0 * PADDING);
    let mut text = text_box(subject, date, reason, mode, preferred)?;
    let mut laid = text_layout::layout(&text, width, height, catalogue());
    if !within_bounds(&laid, width, height) {
        text.fit = true;
        laid = text_layout::layout(&text, width, height, catalogue());
    }
    if !within_bounds(&laid, width, height) || laid.lines.iter().flat_map(|l| &l.words)
        .any(|word| word.size < MIN_TEXT_SIZE || word.size > preferred + 0.01)
    { return Err("Signature box is too small for readable certificate details; draw a larger box.".into()); }
    Ok(laid)
}

struct FontUse { face: Arc<Face>, chars: BTreeMap<char, (u16, u16, f32)> }
impl FontUse {
    fn code(&mut self, ch: char) -> Result<u16, String> {
        if let Some((cid, _, _)) = self.chars.get(&ch) { return Ok(*cid); }
        let cid = u16::try_from(self.chars.len() + 1).map_err(|_| "Too many visible signature characters.")?;
        let (gid, width) = self.face.glyph(ch);
        if gid == 0 { return Err("Missing glyph in visible signature; signer was not substituted.".into()); }
        self.chars.insert(ch, (cid, gid, width));
        Ok(cid)
    }
}
#[derive(Default)]
pub(crate) struct Resources { fonts: Vec<(FontUse, ObjectId)> }
impl Resources {
    fn font(&mut self, update: &mut IncrementalDocument, used: FontUse) -> Result<ObjectId, String> {
        if let Some((_, id)) = self.fonts.iter().find(|(cached, _)| Arc::ptr_eq(&cached.face, &used.face) && cached.chars == used.chars) { return Ok(*id); }
        let id = embed(update, &used)?;
        self.fonts.push((used, id));
        Ok(id)
    }
}
fn embed(update: &mut IncrementalDocument, used: &FontUse) -> Result<ObjectId, String> {
    let face = &used.face;
    if face.entry.cff || face.data.len() > 16 * 1024 * 1024 { return Err("Unsupported visible-signature font program.".into()); }
    let mut program = Stream::new(dictionary! { "Length1" => face.data.len() as i64 }, face.data.to_vec());
    program.compress().map_err(|e| e.to_string())?;
    let program = update.new_document.add_object(program);
    let postscript = Object::Name(face.entry.postscript.replace(' ', "").into_bytes());
    let em = |n: f32| Object::Real(n * 1000.0);
    let descriptor = update.new_document.add_object(dictionary! { "Type" => "FontDescriptor", "FontName" => postscript.clone(),
        "Flags" => 32, "FontBBox" => Object::Array(vec![em(-0.5), em(face.descent), em(1.5), em(face.ascent)]),
        "ItalicAngle" => 0, "Ascent" => em(face.ascent), "Descent" => em(face.descent), "CapHeight" => em(face.ascent * 0.9), "StemV" => 80,
        "FontFile2" => program, "FontFamily" => pdf_text(&face.entry.family),
    });
    let mut mapping = vec![0u8; (used.chars.len() + 1) * 2];
    let mut widths = vec![];
    let mut cmap = String::from("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /KineticSignatureUnicode def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
    let mut entries: Vec<_> = used.chars.iter().collect();
    entries.sort_by_key(|(_, (cid, _, _))| *cid);
    for chunk in entries.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        for (&ch, &(cid, gid, width)) in chunk {
            mapping[cid as usize * 2..cid as usize * 2 + 2].copy_from_slice(&gid.to_be_bytes());
            widths.push(Object::Integer(cid.into())); widths.push(Object::Array(vec![em(width)]));
            let encoded: String = ch.encode_utf16(&mut [0; 2]).iter().map(|u| format!("{u:04X}")).collect();
            let _ = writeln!(cmap, "<{cid:04X}> <{encoded}>");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let map = update.new_document.add_object(Stream::new(Dictionary::new(), mapping));
    let unicode = update.new_document.add_object(Stream::new(Dictionary::new(), cmap.into_bytes()));
    let cid = update.new_document.add_object(dictionary! { "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => postscript.clone(),
        "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
        "FontDescriptor" => descriptor, "CIDToGIDMap" => map, "DW" => 1000, "W" => widths,
    });
    Ok(update.new_document.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => postscript,
        "Encoding" => "Identity-H", "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => unicode }))
}

pub(crate) fn append(update: &mut IncrementalDocument, resources: &mut Resources, placed: &Placement, identity: &IdentityInfo,
    date: &str, reason: &str, mode: SignatureMode,
) -> Result<ObjectId, String> {
    let [w, h] = placed.size;
    let padding = f64::from(PADDING);
    let laid = fit_details(&identity.subject, date, reason, mode, w as f32, h as f32)?;
    let mut content = format!("q 0 0 {w:.4} {h:.4} re W n\n0.985 0.990 0.995 rg 0 0 {w:.4} {h:.4} re f\n");
    let scale = (w * 0.80 / 544.0).min(h * 0.85 / 696.0);
    let (x, y) = ((w - scale * 544.0) / 2.0, (h - scale * 696.0) / 2.0);
    let _ = writeln!(content, "q /KineticWatermark gs {scale:.8} 0 0 {:.8} {x:.4} {:.4} cm 0.411765 0.505882 0.588235 rg", -scale, y + 696.0 * scale);
    content.push_str(&watermark_ops()?);
    content.push_str("Q\n");
    let _ = writeln!(content, "0.411765 0.505882 0.588235 RG 0.5 w 0.25 0.25 {:.4} {:.4} re S", w - 0.5, h - 0.5);
    let mut used: Vec<FontUse> = vec![];
    for line in &laid.lines {
        for word in &line.words {
            let at = match used.iter().position(|u| Arc::ptr_eq(&u.face, &word.face)) {
                Some(at) => at,
                None => { if used.len() >= MAX_FONTS { return Err("Too many fallback fonts for visible signature.".into()); }
                    used.push(FontUse { face: word.face.clone(), chars: BTreeMap::new() }); used.len() - 1 }
            };
            let mut codes = String::new();
            for ch in word.text.chars() { let cid = used[at].code(ch)?; let _ = write!(codes, "{cid:04X}"); }
            let _ = writeln!(content, "BT /SF{at} {:.4} Tf 0.10 0.13 0.16 rg 1 0 0 1 {:.4} {:.4} Tm <{codes}> Tj ET",
                word.size, padding + f64::from(word.x), h - padding - f64::from(line.baseline));
        }
    }
    content.push_str("Q\n");
    let mut fonts = Dictionary::new();
    for (at, font) in used.into_iter().enumerate() { fonts.set(format!("SF{at}"), resources.font(update, font)?); }
    let form = Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "FormType" => 1,
        "BBox" => reals([0.0, 0.0, w, h]), "Matrix" => reals(placed.matrix),
        "Resources" => dictionary! { "Font" => fonts,
            "ExtGState" => dictionary! { "KineticWatermark" => dictionary! { "Type" => "ExtGState", "ca" => Object::Real(0.16) } } },
        "KineticSignatureAppearance" => true,
    }, content.into_bytes());
    Ok(update.new_document.add_object(form))
}

// Strict, small parser only for the trusted include_str asset's M/H/V/L/Z
// grammar. SVG Y-down is flipped by the watermark drawing matrix above.
fn watermark_ops() -> Result<String, String> {
    let mut out = String::new(); let mut paths = 0;
    for part in WATERMARK.split("<path d=\"").skip(1) {
        let data = part.split('"').next().ok_or("Malformed fixed signature mark.")?;
        out.push_str(&path_ops(data)?); out.push_str("f\n"); paths += 1;
    }
    if paths != 3 || !WATERMARK.contains("fill=\"#698196\"") { return Err("Unsupported fixed signature watermark asset.".into()); }
    Ok(out)
}
fn path_ops(data: &str) -> Result<String, String> {
    let mut tokens = Vec::<String>::new(); let mut number = String::new();
    for ch in data.chars() {
        if ch.is_ascii_alphabetic() {
            if !number.is_empty() { tokens.push(std::mem::take(&mut number)); }
            if !matches!(ch, 'M' | 'H' | 'V' | 'L' | 'Z') { return Err("Unsupported fixed SVG path command.".into()); }
            tokens.push(ch.to_string());
        } else if ch.is_ascii_digit() || ch == '.' || ch == '-' { number.push(ch); }
        else if ch.is_ascii_whitespace() || ch == ',' { if !number.is_empty() { tokens.push(std::mem::take(&mut number)); } }
        else { return Err("Invalid fixed SVG path token.".into()); }
    }
    if !number.is_empty() { tokens.push(number); }
    if tokens.len() > 1024 { return Err("Fixed SVG path exceeds limit.".into()); }
    let mut out = String::new(); let (mut at, mut command) = (0, ' ');
    let (mut x, mut y, mut start) = (0.0, 0.0, (0.0, 0.0));
    while at < tokens.len() {
        if tokens[at].len() == 1 && tokens[at].as_bytes()[0].is_ascii_alphabetic() {
            command = tokens[at].chars().next().unwrap(); at += 1;
            if command == 'Z' { out.push_str("h\n"); (x, y) = start; continue; }
        }
        let mut next = || -> Result<f64, String> {
            let value: f64 = tokens.get(at).ok_or("Truncated fixed SVG path.")?.parse().map_err(|_| "Invalid fixed SVG number.")?;
            if !value.is_finite() || value.abs() > 1000.0 { return Err("Invalid fixed SVG bounds.".into()); }
            at += 1; Ok(value)
        };
        match command {
            'M' | 'L' => { x = next()?; y = next()?; if command == 'M' { start = (x, y); let _ = writeln!(out, "{x} {y} m"); command = 'L'; }
                else { let _ = writeln!(out, "{x} {y} l"); } }
            'H' => { x = next()?; let _ = writeln!(out, "{x} {y} l"); }
            'V' => { y = next()?; let _ = writeln!(out, "{x} {y} l"); }
            _ => return Err("Invalid fixed SVG path state.".into()),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inherited_page(rotation: u16) -> Document {
        let mut doc = Document::with_version("1.7");
        let pages = doc.new_object_id();
        let first = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages,
            "MediaBox" => reals([0.0, 0.0, 612.0, 792.0]), "CropBox" => reals([0.0, 0.0, 612.0, 792.0]), "Rotate" => 0 });
        let second = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages });
        doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Count" => 2,
            "Kids" => vec![Object::Reference(first), Object::Reference(second)], "MediaBox" => reals([-40.0, -20.0, 600.0, 800.0]),
            "CropBox" => reals([20.0, 30.0, 650.0, 760.0]), "Rotate" => i64::from(rotation) }));
        let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", root); doc
    }
    #[test]
    fn inherited_crop_media_rotation_page_and_userunit_are_independently_checked() {
        for rotation in [0, 90, 180, 270] {
            let doc = inherited_page(rotation);
            let rect = if rotation % 180 == 0 { [40.0, 60.0, 320.0, 170.0] } else { [40.0, 60.0, 150.0, 340.0] };
            let spec = SignatureAppearance { page: 1, rect, rotation };
            let placed = placement(&doc, &spec).unwrap();
            assert_eq!(placed.page, *doc.get_pages().values().nth(1).unwrap());
            assert_eq!(placed.size, [280.0, 110.0]);
            assert!(placement(&doc, &SignatureAppearance { rotation: (rotation + 90) % 360, ..spec.clone() }).is_err());
            assert!(placement(&doc, &SignatureAppearance { page: 99, ..spec.clone() }).is_err());
            assert!(placement(&doc, &SignatureAppearance { rect: [0.0, 60.0, 300.0, 170.0], ..spec.clone() }).is_err());
            assert!(placement(&doc, &SignatureAppearance { rect: [40.0, 700.0, 320.0, 780.0], ..spec.clone() }).is_err());
            assert!(placement(&doc, &SignatureAppearance { rect: [590.0, 60.0, 650.0, 340.0], ..spec.clone() }).is_err());
            assert!(placement(&doc, &SignatureAppearance { rect: [40.0, 60.0, 42.0, 62.0], ..spec.clone() }).is_err());
            assert!(placement(&doc, &SignatureAppearance { rect: [f32::NAN, 60.0, 320.0, 170.0], ..spec.clone() }).is_err());
            assert!(placement(&doc, &SignatureAppearance { rotation: 1, ..spec.clone() }).is_err());
            let mut nonunit = doc;
            nonunit.get_dictionary_mut(placed.page).unwrap().set("UserUnit", 2);
            assert!(placement(&nonunit, &spec).unwrap_err().contains("UserUnit"));
        }
    }
    #[test]
    fn preferred_detail_size_scales_with_displayed_area_and_is_bounded() {
        assert_eq!(preferred_text_size(220.0, 80.0), 10.0);
        assert_eq!(preferred_text_size(440.0, 160.0), 20.0);
        assert_eq!(preferred_text_size(80.0, 220.0), 10.0);
        assert_eq!(preferred_text_size(0.0, 0.0), MIN_TEXT_SIZE);
        assert_eq!(preferred_text_size(10_000.0, 10_000.0), MAX_TEXT_SIZE);
    }
    #[cfg(windows)]
    #[test]
    fn detail_layout_grows_for_large_boxes_and_shrinks_or_refuses_long_metadata() {
        let subject = "CN=Renée Δοκιμή Жуков, O=Kinetic test";
        let date = "2026-11-14T12:34:56Z";
        let mode = SignatureMode::CertifyNoChanges;
        let size = |laid: &text_layout::Laid| laid.lines.iter().flat_map(|l| &l.words).map(|w| w.size).fold(0.0, f32::max);
        let small = fit_details(subject, date, "Résumé approved", mode, 240.0, 110.0).unwrap();
        let large = fit_details(subject, date, "Résumé approved", mode, 400.0, 400.0).unwrap();
        assert!(size(&large) > size(&small) * 1.5);
        assert!(size(&small) >= MIN_TEXT_SIZE && size(&large) <= MAX_TEXT_SIZE);
        let long_reason = "Measured résumé approved. ".repeat(16);
        let fitted = fit_details(subject, date, &long_reason, mode, 400.0, 400.0).unwrap();
        assert!(size(&fitted) < size(&large) && size(&fitted) >= MIN_TEXT_SIZE);
        assert!(within_bounds(&fitted, 392.0, 392.0));
        assert!(fit_details(subject, date, &long_reason, mode, 96.0, 40.0).err().unwrap().contains("too small"));
    }
    #[test]
    fn unicode_metadata_is_utf16_not_ascii_substitution() {
        assert_eq!(pdf_text("Renée Δ Ж").as_str().unwrap(), &[0xfe,0xff,0,82,0,101,0,110,0,233,0,101,0,32,3,148,0,32,4,22]);
    }

    #[test]
    fn fixed_watermark_uses_all_three_vector_paths() {
        let ops = watermark_ops().unwrap();
        assert_eq!(ops.lines().filter(|l| *l == "f").count(), 3);
        assert!(ops.contains("394 0 l\n394 48 l"));
        assert!(ops.contains("398 0 m\n544 140 l\n398 140 l\nh"));
    }
    #[test]
    fn matrix_counters_every_clockwise_page_rotation_without_bbox_scaling() {
        for rotation in [0, 90, 180, 270] {
            let (size, m) = orientation(240.0, 96.0, rotation);
            let display = |x: f64, y: f64| match rotation { 90 => (y, -x), 180 => (-x, -y), 270 => (-y, x), _ => (x, y) };
            assert_eq!(display(m[0], m[1]), (1.0, 0.0));
            assert_eq!(display(m[2], m[3]), (0.0, 1.0));
            let corners = [(0.0,0.0),(size[0],0.0),(0.0,size[1]),(size[0],size[1])].map(|(x,y)| (m[0]*x+m[2]*y+m[4],m[1]*x+m[3]*y+m[5]));
            assert_eq!(corners.iter().map(|p|p.0).fold(f64::INFINITY,f64::min),0.0);
            assert_eq!(corners.iter().map(|p|p.1).fold(f64::INFINITY,f64::min),0.0);
            assert_eq!(corners.iter().map(|p|p.0).fold(f64::NEG_INFINITY,f64::max),240.0);
            assert_eq!(corners.iter().map(|p|p.1).fold(f64::NEG_INFINITY,f64::max),96.0);
        }
    }
}
