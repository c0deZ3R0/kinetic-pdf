//! Fonts for drawing text: which glyph each character code shows, how wide it
//! is, and its outline, from the TrueType and OpenType fonts embedded in a PDF
//! (read with ttf-parser).

use std::collections::HashMap;

use lyon_tessellation::{FillRule, FillTessellator};
use pdf_content::lopdf::{Dictionary, Document, Object, Stream};
use pdf_content::objects::{dict, number};
use ttf_parser::{cmap, Face, GlyphId, OutlineBuilder, PlatformId};

use crate::geometry::{fill, Piece};

/// A CID font's glyphs are this wide, in thousandths of an em, unless it says.
const DEFAULT_CID_WIDTH: f32 = 1000.0;

/// Why a font can't be drawn, as counted in `Shapes::not_drawn`.
pub(crate) type Unsupported = &'static str;

/// Which glyph each code shows, and how wide.
enum Codes {
    /// One byte a code.
    Simple { glyphs: Box<[u16; 256]>, widths: Box<[f32; 256]> },
    /// Two bytes a code, a CID, as the Identity-H encoding has them; glyphs
    /// by the CID-to-glyph map, or numbered the same without one.
    Cid { glyphs: Option<Vec<u16>>, widths: HashMap<u16, f32>, default_width: f32 },
}

/// One code in a string shown in a font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Code {
    pub glyph: u16,
    /// In thousandths of an em.
    pub width: f32,
    /// A single-byte space, which word spacing applies after.
    pub is_space: bool,
}

pub(crate) struct Font {
    data: Vec<u8>,
    units_per_em: f32,
    codes: Codes,
    /// Glyphs' filled areas, in ems, by glyph and the power of two they were
    /// tessellated within.
    triangles: HashMap<(u16, i8), Vec<[[f32; 2]; 3]>>,
}

impl Font {
    /// The font dictionary `font`, ready to draw text in.
    pub fn load(doc: &Document, font: &Dictionary) -> Result<Font, Unsupported> {
        fn name<'d>(doc: &'d Document, d: &'d Dictionary, key: &[u8]) -> Option<&'d [u8]> {
            d.get(key).ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_name().ok())
        }
        match name(doc, font, b"Subtype") {
            Some(b"Type0") => {
                if name(doc, font, b"Encoding") != Some(b"Identity-H") {
                    return Err("text in encodings other than Identity-H");
                }
                let descendant = font
                    .get(b"DescendantFonts")
                    .ok()
                    .and_then(|d| doc.dereference(d).ok())
                    .and_then(|(_, d)| d.as_array().ok()?.first())
                    .and_then(|d| dict(doc, d))
                    .ok_or("text in fonts that couldn't be read")?;
                let data = embedded(doc, descendant)?;
                let glyphs = stream(doc, descendant, b"CIDToGIDMap")
                    .and_then(|map| map.get_plain_content().ok())
                    .map(|map| map.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect());
                let widths = cid_widths(doc, descendant.get(b"W").ok());
                let default_width = descendant.get(b"DW").ok().and_then(|w| number(doc, w)).map_or(DEFAULT_CID_WIDTH, |w| w as f32);
                Font::new(data, |_| Codes::Cid { glyphs, widths, default_width })
            }
            Some(b"TrueType" | b"Type1" | b"MMType1") => {
                let data = embedded(doc, font)?;
                Font::new(data, |face| {
                    let glyphs = simple_glyphs(doc, font, face);
                    let widths = simple_widths(doc, font, face, &glyphs);
                    Codes::Simple { glyphs, widths }
                })
            }
            Some(b"Type3") => Err("text in Type 3 fonts"),
            _ => Err("text in fonts that couldn't be read"),
        }
    }

    fn new(data: Vec<u8>, codes: impl FnOnce(&Face) -> Codes) -> Result<Font, Unsupported> {
        let face = Face::parse(&data, 0).map_err(|_| "text in fonts that couldn't be read")?;
        let (units_per_em, codes) = (f32::from(face.units_per_em()), codes(&face));
        Ok(Font { data, units_per_em, codes, triangles: HashMap::new() })
    }

    /// The codes in `bytes`, a string shown in this font.
    pub fn codes(&self, bytes: &[u8]) -> Vec<Code> {
        match &self.codes {
            Codes::Simple { glyphs, widths } => {
                bytes.iter().map(|&b| Code { glyph: glyphs[b as usize], width: widths[b as usize], is_space: b == b' ' }).collect()
            }
            Codes::Cid { glyphs, widths, default_width } => bytes
                .chunks_exact(2)
                .map(|pair| {
                    let cid = u16::from_be_bytes([pair[0], pair[1]]);
                    let glyph = glyphs.as_ref().map_or(cid, |map| map.get(cid as usize).copied().unwrap_or(0));
                    Code { glyph, width: widths.get(&cid).copied().unwrap_or(*default_width), is_space: false }
                })
                .collect(),
        }
    }

    /// Glyph `glyph`'s outline, in ems.
    pub fn outline(&self, glyph: u16) -> Vec<Piece> {
        let mut builder = Outline { pieces: Vec::new(), scale: 1.0 / self.units_per_em, current: [0.0; 2] };
        if let Ok(face) = Face::parse(&self.data, 0) {
            face.outline_glyph(GlyphId(glyph), &mut builder);
        }
        builder.pieces
    }

    /// Glyph `glyph`'s filled area as triangles, in ems, within `tolerance`
    /// ems. Kept, so each glyph is tessellated once for the sizes it's shown
    /// at: to the power of two at or under the tolerance.
    pub fn triangles(&mut self, glyph: u16, tolerance: f32, tessellator: &mut FillTessellator) -> &[[[f32; 2]; 3]] {
        let bucket = tolerance.max(1e-9).log2().floor().max(-100.0) as i8;
        if !self.triangles.contains_key(&(glyph, bucket)) {
            let triangles = fill(&self.outline(glyph), FillRule::NonZero, 2_f32.powi(bucket.into()), tessellator).unwrap_or_default();
            self.triangles.insert((glyph, bucket), triangles);
        }
        &self.triangles[&(glyph, bucket)]
    }
}

/// Collects a glyph's outline from ttf-parser, scaled to ems, quadratic
/// curves raised to cubic ones.
struct Outline {
    pieces: Vec<Piece>,
    scale: f32,
    current: [f32; 2],
}

impl Outline {
    fn at(&self, x: f32, y: f32) -> [f32; 2] {
        [x * self.scale, y * self.scale]
    }

    fn push(&mut self, piece: Piece, end: [f32; 2]) {
        self.pieces.push(piece);
        self.current = end;
    }
}

impl OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        let at = self.at(x, y);
        self.push(Piece::Move(at), at);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let at = self.at(x, y);
        self.push(Piece::Line(at), at);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (control, end, start) = (self.at(x1, y1), self.at(x, y), self.current);
        let toward = |from: [f32; 2]| [from[0] + (control[0] - from[0]) * 2.0 / 3.0, from[1] + (control[1] - from[1]) * 2.0 / 3.0];
        self.push(Piece::Curve(toward(start), toward(end), end), end);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let end = self.at(x, y);
        self.push(Piece::Curve(self.at(x1, y1), self.at(x2, y2), end), end);
    }

    fn close(&mut self) {
        self.pieces.push(Piece::Close);
    }
}

/// Stream `key` of `d`, following a reference to it.
fn stream<'d>(doc: &'d Document, d: &'d Dictionary, key: &[u8]) -> Option<&'d Stream> {
    d.get(key).ok().and_then(|s| doc.dereference(s).ok()).and_then(|(_, s)| s.as_stream().ok())
}

/// The font program embedded for `font` (a simple font, or a Type 0 font's
/// descendant), if it's one ttf-parser reads.
fn embedded(doc: &Document, font: &Dictionary) -> Result<Vec<u8>, Unsupported> {
    let descriptor = font.get(b"FontDescriptor").ok().and_then(|d| dict(doc, d)).ok_or("text in fonts that aren't embedded")?;
    let program = match (stream(doc, descriptor, b"FontFile2"), stream(doc, descriptor, b"FontFile3")) {
        (Some(truetype), _) => truetype,
        (None, Some(compact)) if compact.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"OpenType") => compact,
        (None, Some(_)) => return Err("text in CFF fonts"),
        (None, None) if descriptor.has(b"FontFile") => return Err("text in Type 1 fonts"),
        (None, None) => return Err("text in fonts that aren't embedded"),
    };
    program.get_plain_content().map_err(|_| "text in fonts that couldn't be read")
}

/// A CID font's `W` array: `c [w1 w2 ...]` gives the CIDs from `c` each
/// width in turn, and `first last w` gives them all one.
fn cid_widths(doc: &Document, w: Option<&Object>) -> HashMap<u16, f32> {
    let mut widths = HashMap::new();
    let Some(items) = w.and_then(|w| doc.dereference(w).ok()).and_then(|(_, w)| w.as_array().ok()) else { return widths };
    let value = |i: usize| items.get(i).and_then(|o| number(doc, o));
    let mut i = 0;
    while let Some(first) = value(i).map(|f| f.clamp(0.0, 65535.0) as u32) {
        match items.get(i + 1).and_then(|o| doc.dereference(o).ok()).map(|(_, o)| o) {
            Some(Object::Array(each)) => {
                for (cid, width) in (first..=0xFFFF).zip(each) {
                    widths.extend(number(doc, width).map(|w| (cid as u16, w as f32)));
                }
                i += 2;
            }
            _ => {
                let (Some(last), Some(width)) = (value(i + 1), value(i + 2)) else { break };
                widths.extend((first..=(last.clamp(0.0, 65535.0) as u32)).map(|cid| (cid as u16, width as f32)));
                i += 3;
            }
        }
    }
    widths
}

/// A simple font's widths: `Widths` from `FirstChar`, and the font's own
/// advances for its `glyphs` where that doesn't cover them.
fn simple_widths(doc: &Document, font: &Dictionary, face: &Face, glyphs: &[u16; 256]) -> Box<[f32; 256]> {
    let em = 1000.0 / f32::from(face.units_per_em());
    let mut widths = Box::new(std::array::from_fn(|code| face.glyph_hor_advance(GlyphId(glyphs[code])).map_or(0.0, |a| f32::from(a) * em)));
    let first = font.get(b"FirstChar").ok().and_then(|f| number(doc, f)).unwrap_or(0.0).max(0.0) as usize;
    if let Some(listed) = font.get(b"Widths").ok().and_then(|w| doc.dereference(w).ok()).and_then(|(_, w)| w.as_array().ok()) {
        for (slot, width) in widths.iter_mut().skip(first).zip(listed) {
            if let Some(width) = number(doc, width) {
                *slot = width as f32;
            }
        }
    }
    widths
}

/// A simple font's glyph for each code: by the name its encoding's
/// differences give it, else through the font's own character map -- a
/// symbol map, a Unicode one taking codes as Windows' Latin encoding, or a
/// Macintosh one -- else, without any map, numbered the same.
fn simple_glyphs(doc: &Document, font: &Dictionary, face: &Face) -> Box<[u16; 256]> {
    let differences = differences(doc, font);
    let subtables: Vec<cmap::Subtable> = face.tables().cmap.into_iter().flat_map(|c| c.subtables).collect();
    let find = |platform: PlatformId, encoding: u16| subtables.iter().find(|s| s.platform_id == platform && s.encoding_id == encoding);
    let (symbol, unicode, mac) = (find(PlatformId::Windows, 0), find(PlatformId::Windows, 1).or_else(|| find(PlatformId::Unicode, 3)), find(PlatformId::Macintosh, 0));
    Box::new(std::array::from_fn(|code| {
        let named = differences.get(&(code as u8)).and_then(|name| {
            let name = std::str::from_utf8(name).ok()?;
            face.glyph_index_by_name(name).or_else(|| unicode?.glyph_index(unicode_of_name(name)?))
        });
        let glyph = named
            .or_else(|| symbol.and_then(|s| s.glyph_index(0xF000 + code as u32).or_else(|| s.glyph_index(code as u32))))
            .or_else(|| unicode.and_then(|s| s.glyph_index(win_ansi(code as u8))))
            .or_else(|| mac.and_then(|s| s.glyph_index(code as u32)));
        glyph.map_or(if subtables.is_empty() { code as u16 } else { 0 }, |g| g.0)
    }))
}

/// The glyph names an encoding's `Differences` array gives codes.
fn differences<'d>(doc: &'d Document, font: &'d Dictionary) -> HashMap<u8, &'d [u8]> {
    let mut names = HashMap::new();
    let encoding = font.get(b"Encoding").ok().and_then(|e| dict(doc, e));
    let Some(items) = encoding.and_then(|e| e.get(b"Differences").ok()).and_then(|d| doc.dereference(d).ok()).and_then(|(_, d)| d.as_array().ok()) else {
        return names;
    };
    let mut code = 0_usize;
    for item in items {
        match item {
            Object::Name(name) => {
                if let Ok(code) = u8::try_from(code) {
                    names.insert(code, name.as_slice());
                }
                code += 1;
            }
            other => code = number(doc, other).map_or(code, |n| n.max(0.0) as usize),
        }
    }
    names
}

/// The character a glyph name stands for, where the name says it plainly:
/// `uniXXXX`, `uXXXX`, or a single letter or digit.
fn unicode_of_name(name: &str) -> Option<u32> {
    let hex = name.strip_prefix("uni").or_else(|| name.strip_prefix('u'));
    match hex.and_then(|h| u32::from_str_radix(h, 16).ok()) {
        Some(code) => Some(code),
        None => (name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric()).then(|| u32::from(name.as_bytes()[0])),
    }
}

/// The character code `code` stands for in Windows' Latin encoding, which PDF
/// takes for a TrueType font's codes; Latin-1 but for 0x80 to 0x9F.
fn win_ansi(code: u8) -> u32 {
    const FROM_0X80: [u16; 32] = [
        0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0, 0x017D, 0, //
        0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
    ];
    match code {
        0x80..=0x9F => u32::from(FROM_0X80[code as usize - 0x80]),
        _ => u32::from(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_font::{square_font, type0_font};
    use pdf_content::lopdf::dictionary;

    fn load(font: &Dictionary) -> Result<Font, Unsupported> {
        Font::load(&Document::with_version("1.7"), font)
    }

    #[test]
    fn a_cid_font_shows_two_byte_codes_through_its_glyph_map() {
        let identity = load(&type0_font()).expect("the font loads");
        assert_eq!(identity.codes(&[0, 1, 0, 0]), [Code { glyph: 1, width: 500.0, is_space: false }, Code { glyph: 0, width: 500.0, is_space: false }]);

        let mut mapped = type0_font();
        let Ok(Object::Array(descendants)) = mapped.get_mut(b"DescendantFonts") else { panic!() };
        let Object::Dictionary(descendant) = &mut descendants[0] else { panic!() };
        descendant.set("CIDToGIDMap", Stream::new(dictionary! {}, vec![0, 1, 0, 0]));
        descendant.set("W", vec![Object::Integer(1), Object::Array(vec![Object::Integer(250)])]);
        let mapped = load(&mapped).unwrap();
        assert_eq!(mapped.codes(&[0, 0, 0, 1]).iter().map(|c| (c.glyph, c.width)).collect::<Vec<_>>(), [(1, 500.0), (0, 250.0)]);
    }

    #[test]
    fn widths_are_read_in_both_forms() {
        let int = Object::Integer;
        let w = Object::Array(vec![int(1), Object::Array(vec![int(500), int(600)]), int(3), int(5), int(300)]);
        let widths = cid_widths(&Document::with_version("1.7"), Some(&w));
        let mut listed: Vec<(u16, f32)> = widths.into_iter().collect();
        listed.sort_by_key(|(cid, _)| *cid);
        assert_eq!(listed, [(1, 500.0), (2, 600.0), (3, 300.0), (4, 300.0), (5, 300.0)]);
    }

    #[test]
    fn a_glyph_becomes_triangles_in_ems_once() {
        let mut font = load(&type0_font()).unwrap();
        let mut tessellator = FillTessellator::new();
        let area: f32 = font.triangles(1, 0.001, &mut tessellator).iter().map(|[a, b, c]| ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0).sum();
        assert!((area - 0.01).abs() < 1e-5, "a tenth of an em square: {area}");
        font.triangles(1, 0.0011, &mut tessellator);
        assert_eq!(font.triangles.len(), 1, "a similar tolerance reuses them");
        assert!(font.triangles(0, 0.001, &mut tessellator).is_empty());
    }

    #[test]
    fn a_simple_font_without_a_character_map_numbers_glyphs_by_code() {
        let descriptor = dictionary! { "FontFile2" => Stream::new(dictionary! {}, square_font()) };
        let font = load(&dictionary! { "Subtype" => "TrueType", "FirstChar" => Object::Integer(1), "Widths" => vec![Object::Integer(700)], "FontDescriptor" => descriptor }).unwrap();
        assert_eq!(font.codes(b"\x01 "), [Code { glyph: 1, width: 700.0, is_space: false }, Code { glyph: 32, width: 0.0, is_space: true }]);
    }

    #[test]
    fn fonts_that_cant_be_drawn_say_why() {
        assert_eq!(load(&dictionary! { "Subtype" => "Type1", "BaseFont" => "Helvetica" }).err(), Some("text in fonts that aren't embedded"));
        assert_eq!(load(&dictionary! { "Subtype" => "Type3" }).err(), Some("text in Type 3 fonts"));
        let mut vertical = type0_font();
        vertical.set("Encoding", "Identity-V");
        assert_eq!(load(&vertical).err(), Some("text in encodings other than Identity-H"));
    }

    #[test]
    fn glyph_names_that_say_their_character_are_read() {
        assert_eq!((unicode_of_name("uni20AC"), unicode_of_name("A"), unicode_of_name("Aacute")), (Some(0x20AC), Some(65), None));
        assert_eq!((win_ansi(0x80), win_ansi(b'A')), (0x20AC, 65));
    }
}
