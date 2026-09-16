//! Fonts for drawing text: which glyph each character code shows, how wide it
//! is, and its outline. TrueType and OpenType fonts are read with skrifa, and
//! the bare CFF and Type 1 fonts PDFs embed with hayro-font.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use hayro_font::{cff, type1};
use lyon_tessellation::{FillRule, FillTessellator};
use pdf_content::lopdf::{Dictionary, Document, Object, Stream};
use pdf_content::objects::{dict, number};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::tables::cmap::{CmapSubtable, PlatformId};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::geometry::{fill, Matrix, Piece};
use crate::shapes::Unsupported;

/// A CID font's glyphs are this wide, in thousandths of an em, unless it says.
const DEFAULT_CID_WIDTH: f32 = 1000.0;

/// Why a font couldn't be read, as counted in `Shapes::not_drawn`.
const UNREADABLE: Unsupported = "text in fonts that couldn't be read";

/// A font the PDF doesn't embed, which is drawn in a system font instead
/// (`substitute`) unless there's none on this computer to stand in for it.
const NOT_EMBEDDED: Unsupported = "text in fonts that aren't embedded";

/// A font program embedded in a PDF, or one from the system standing in for a
/// font that isn't embedded (`substitute`).
enum Program {
    /// TrueType or OpenType, glyphs by index. Shared, since a system font
    /// stands in for every font of its family on the page.
    OpenType(Arc<Vec<u8>>),
    /// Bare CFF, glyphs by index.
    Cff(Vec<u8>),
    /// Type 1, glyphs by name: glyph `n` is the `n`th of `names`, from 1.
    Type1 { table: type1::Table, names: Vec<String> },
}

impl Program {
    /// The matrix from its glyph units to ems; `None` if it can't be read.
    fn units(&self) -> Option<Matrix> {
        match self {
            Program::OpenType(data) => {
                let units_per_em = f32::from(FontRef::new(data).ok()?.head().ok()?.units_per_em());
                Some(Matrix::scale(1.0 / units_per_em, 1.0 / units_per_em))
            }
            Program::Cff(data) => Some(font_matrix(cff::Table::parse(data)?.matrix())),
            Program::Type1 { table, .. } => Some(font_matrix(table.matrix())),
        }
    }

    /// Draws glyph `glyph` into `outline`; nothing if there's no such glyph.
    fn draw(&self, glyph: u32, outline: &mut Outline) {
        match self {
            Program::OpenType(data) => {
                if let Some(found) = FontRef::new(data).ok().and_then(|font| font.outline_glyphs().get(GlyphId::new(glyph))) {
                    // A glyph that won't draw is left as far as it got.
                    let _ = found.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), outline);
                }
            }
            Program::Cff(data) => {
                if let (Some(table), Ok(glyph)) = (cff::Table::parse(data), u16::try_from(glyph)) {
                    let _ = table.outline(hayro_font::GlyphId(glyph), outline);
                }
            }
            Program::Type1 { table, names } => {
                if let Some(name) = (glyph as usize).checked_sub(1).and_then(|i| names.get(i)) {
                    table.outline(name, outline);
                }
            }
        }
    }
}

/// A PDF-style matrix from a CFF or Type 1 font matrix.
fn font_matrix(m: hayro_font::Matrix) -> Matrix {
    Matrix([m.sx, m.ky, m.kx, m.sy, m.tx, m.ty])
}

/// Which glyph each code shows, and how wide.
enum Codes {
    /// One byte a code.
    Simple { glyphs: Box<[u32; 256]>, widths: Box<[f32; 256]> },
    /// Two bytes a code, a CID, as the Identity-H encoding has them; glyphs
    /// by a map from CIDs, or numbered the same without one.
    Cid { glyphs: Option<Vec<u16>>, widths: HashMap<u16, f32>, default_width: f32 },
}

/// One code in a string shown in a font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Code {
    /// 0 for none.
    pub glyph: u32,
    /// In thousandths of an em.
    pub width: f32,
    /// A single-byte space, which word spacing applies after.
    pub is_space: bool,
}

pub(crate) struct Font {
    program: Program,
    /// From glyph units to ems.
    units: Matrix,
    codes: Codes,
    /// Glyphs' filled areas, in ems, by glyph and the power of two they were
    /// tessellated within.
    triangles: HashMap<(u32, i8), Vec<[[f32; 2]; 3]>>,
}

impl Font {
    /// The font dictionary `font`, ready to draw text in.
    pub fn load(doc: &Document, font: &Dictionary) -> Result<Font, Unsupported> {
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
                    .ok_or(UNREADABLE)?;
                let program = embedded(doc, descendant)?;
                let units = program.units().ok_or(UNREADABLE)?;
                let glyphs = match &program {
                    Program::Cff(data) => cff_glyphs_by_cid(data),
                    _ => stream(doc, descendant, b"CIDToGIDMap")
                        .and_then(|map| map.get_plain_content().ok())
                        .map(|map| map.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect()),
                };
                let widths = cid_widths(doc, descendant.get(b"W").ok());
                let default_width = descendant.get(b"DW").ok().and_then(|w| number(doc, w)).map_or(DEFAULT_CID_WIDTH, |w| w as f32);
                Ok(Font::new(program, units, Codes::Cid { glyphs, widths, default_width }))
            }
            Some(b"TrueType" | b"Type1" | b"MMType1") => {
                // A font the PDF doesn't embed is drawn in one from the
                // system, so a page of ordinary text doesn't fall to pdfium.
                let mut program = match embedded(doc, font) {
                    Err(NOT_EMBEDDED) => substitute(doc, font).ok_or(NOT_EMBEDDED)?,
                    embedded => embedded?,
                };
                let units = program.units().ok_or(UNREADABLE)?;
                let glyphs = simple_glyphs(&differences(doc, font), &mut program);
                let widths = simple_widths(doc, font, &program, units, &glyphs);
                Ok(Font::new(program, units, Codes::Simple { glyphs, widths }))
            }
            Some(b"Type3") => Err("text in Type 3 fonts"),
            _ => Err(UNREADABLE),
        }
    }

    fn new(program: Program, units: Matrix, codes: Codes) -> Font {
        Font { program, units, codes, triangles: HashMap::new() }
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
                    Code { glyph: glyph.into(), width: widths.get(&cid).copied().unwrap_or(*default_width), is_space: false }
                })
                .collect(),
        }
    }

    /// Glyph `glyph`'s outline, in ems.
    pub fn outline(&self, glyph: u32) -> Vec<Piece> {
        let mut outline = Outline { pieces: Vec::new(), units: self.units, current: [0.0; 2] };
        self.program.draw(glyph, &mut outline);
        outline.pieces
    }

    /// Glyph `glyph`'s filled area as triangles, in ems, within `tolerance`
    /// ems. Kept, so each glyph is tessellated once for the sizes it's shown
    /// at: to the power of two at or under the tolerance.
    pub fn triangles(&mut self, glyph: u32, tolerance: f32, tessellator: &mut FillTessellator) -> &[[[f32; 2]; 3]] {
        let bucket = tolerance.max(1e-9).log2().floor().max(-100.0) as i8;
        if !self.triangles.contains_key(&(glyph, bucket)) {
            let triangles = fill(&self.outline(glyph), FillRule::NonZero, 2_f32.powi(bucket.into()), tessellator).unwrap_or_default();
            self.triangles.insert((glyph, bucket), triangles);
        }
        &self.triangles[&(glyph, bucket)]
    }
}

/// Collects a glyph's outline from either font library, in ems, quadratic
/// curves raised to cubic ones.
struct Outline {
    pieces: Vec<Piece>,
    units: Matrix,
    current: [f32; 2],
}

impl Outline {
    fn step(&mut self, piece: Piece, end: [f32; 2]) {
        self.pieces.push(piece);
        self.current = end;
    }

    fn begin(&mut self, x: f32, y: f32) {
        let at = self.units.apply([x, y]);
        self.step(Piece::Move(at), at);
    }

    fn line(&mut self, x: f32, y: f32) {
        let at = self.units.apply([x, y]);
        self.step(Piece::Line(at), at);
    }

    fn quadratic(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (control, end, start) = (self.units.apply([x1, y1]), self.units.apply([x, y]), self.current);
        let toward = |from: [f32; 2]| [from[0] + (control[0] - from[0]) * 2.0 / 3.0, from[1] + (control[1] - from[1]) * 2.0 / 3.0];
        self.step(Piece::Curve(toward(start), toward(end), end), end);
    }

    fn cubic(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let end = self.units.apply([x, y]);
        self.step(Piece::Curve(self.units.apply([x1, y1]), self.units.apply([x2, y2]), end), end);
    }

    fn end(&mut self) {
        self.pieces.push(Piece::Close);
    }
}

/// Lets `Outline` collect outlines through a font library's pen trait, whose
/// methods are the same in each.
macro_rules! pen_for_outline {
    ($pen:path) => {
        impl $pen for Outline {
            fn move_to(&mut self, x: f32, y: f32) {
                self.begin(x, y);
            }

            fn line_to(&mut self, x: f32, y: f32) {
                self.line(x, y);
            }

            fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
                self.quadratic(x1, y1, x, y);
            }

            fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
                self.cubic(x1, y1, x2, y2, x, y);
            }

            fn close(&mut self) {
                self.end();
            }
        }
    };
}

pen_for_outline!(OutlinePen);
pen_for_outline!(hayro_font::OutlineBuilder);

/// Name `key` of `d`, following a reference to it.
fn name<'d>(doc: &'d Document, d: &'d Dictionary, key: &[u8]) -> Option<&'d [u8]> {
    d.get(key).ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_name().ok())
}

/// Stream `key` of `d`, following a reference to it.
fn stream<'d>(doc: &'d Document, d: &'d Dictionary, key: &[u8]) -> Option<&'d Stream> {
    d.get(key).ok().and_then(|s| doc.dereference(s).ok()).and_then(|(_, s)| s.as_stream().ok())
}

/// The font program embedded for `font` (a simple font, or a Type 0 font's
/// descendant), checked that it can be read.
fn embedded(doc: &Document, font: &Dictionary) -> Result<Program, Unsupported> {
    let descriptor = font.get(b"FontDescriptor").ok().and_then(|d| dict(doc, d)).ok_or(NOT_EMBEDDED)?;
    let content = |s: &Stream| s.get_plain_content().map_err(|_| UNREADABLE);
    if let Some(truetype) = stream(doc, descriptor, b"FontFile2") {
        let data = content(truetype)?;
        return FontRef::new(&data).is_ok().then_some(Program::OpenType(Arc::new(data))).ok_or(UNREADABLE);
    }
    if let Some(compact) = stream(doc, descriptor, b"FontFile3") {
        let data = content(compact)?;
        return match compact.dict.get(b"Subtype").and_then(Object::as_name) {
            Ok(b"OpenType") => FontRef::new(&data).is_ok().then_some(Program::OpenType(Arc::new(data))).ok_or(UNREADABLE),
            _ => cff::Table::parse(&data).is_some().then_some(Program::Cff(data)).ok_or(UNREADABLE),
        };
    }
    if let Some(type1) = stream(doc, descriptor, b"FontFile") {
        let table = type1::Table::parse(&content(type1)?).ok_or(UNREADABLE)?;
        return Ok(Program::Type1 { table, names: Vec::new() });
    }
    Err(NOT_EMBEDDED)
}

/// The font families a font that isn't embedded is drawn in, by a word in its
/// name: the file's name for the plain, bold, italic and bold italic of each.
/// The first whose word the name holds wins, so the more particular names come
/// first.
const FAMILIES: [(&str, [&str; 4]); 12] = [
    ("courier", ["cour", "courbd", "couri", "courbi"]),
    ("consol", ["consola", "consolab", "consolai", "consolaz"]),
    ("timesnewroman", ["times", "timesbd", "timesi", "timesbi"]),
    ("times", ["times", "timesbd", "timesi", "timesbi"]),
    ("georgia", ["georgia", "georgiab", "georgiai", "georgiaz"]),
    ("garamond", ["times", "timesbd", "timesi", "timesbi"]),
    ("verdana", ["verdana", "verdanab", "verdanai", "verdanaz"]),
    ("tahoma", ["tahoma", "tahomabd", "tahoma", "tahomabd"]),
    ("trebuchet", ["trebuc", "trebucbd", "trebucit", "trebucbi"]),
    ("calibri", ["calibri", "calibrib", "calibrii", "calibriz"]),
    ("segoeui", ["segoeui", "segoeuib", "segoeuii", "segoeuiz"]),
    ("symbol", ["symbol", "symbol", "symbol", "symbol"]),
];

/// Font descriptor flags: fixed pitch, serif, and italic.
const FIXED_PITCH: i64 = 1;
const SERIF: i64 = 1 << 1;
const ITALIC: i64 = 1 << 6;

/// The name of the system font file standing in for a font named `base` --
/// without its subset prefix -- whose descriptor has `flags` and `weight`.
/// Arial, Times New Roman and Courier New carry the widths of the Helvetica,
/// Times and Courier that PDF names, so text set in those keeps its lines.
/// Anything else goes by the flags: fixed pitch, serif, or neither.
fn system_font(base: &str, flags: i64, weight: f32) -> String {
    let name: String = base.chars().filter(|c| c.is_ascii_alphanumeric()).flat_map(char::to_lowercase).collect();
    let bold = weight >= 600.0 || ["bold", "black", "heavy", "semibold"].iter().any(|heavy| name.contains(heavy));
    let italic = flags & ITALIC != 0 || name.contains("italic") || name.contains("oblique");
    let files = FAMILIES
        .iter()
        .find(|(word, _)| name.contains(word))
        .map(|(_, files)| *files)
        .unwrap_or(match flags {
            _ if flags & FIXED_PITCH != 0 => ["cour", "courbd", "couri", "courbi"],
            _ if flags & SERIF != 0 => ["times", "timesbd", "timesi", "timesbi"],
            _ => ["arial", "arialbd", "ariali", "arialbi"],
        });
    format!("{}.ttf", files[usize::from(bold) + 2 * usize::from(italic)])
}

/// A system font file, read once however many fonts of a document stand in
/// from it.
fn system_font_data(file: &str) -> Option<Arc<Vec<u8>>> {
    static READ: OnceLock<Mutex<HashMap<String, Option<Arc<Vec<u8>>>>>> = OnceLock::new();
    let fonts = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    let mut read = READ.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner());
    read.entry(file.to_owned()).or_insert_with(|| std::fs::read(fonts.join(file)).ok().map(Arc::new)).clone()
}

/// A system font to draw `font` in, when the PDF doesn't embed one. Its codes
/// are then read as they would be for an embedded TrueType font: through the
/// encoding's `Differences` and the font's character map, taking codes as
/// Windows' Latin encoding, which is what such fonts almost always use.
fn substitute(doc: &Document, font: &Dictionary) -> Option<Program> {
    let base = name(doc, font, b"BaseFont").map(String::from_utf8_lossy).unwrap_or_else(|| "Helvetica".into());
    // A subset's name is six letters and a plus in front of the real one.
    let base = base.split_once('+').map_or(base.as_ref(), |(prefix, rest)| if prefix.len() == 6 { rest } else { base.as_ref() });
    let descriptor = font.get(b"FontDescriptor").ok().and_then(|d| dict(doc, d));
    let entry = |key: &[u8]| descriptor.and_then(|d| d.get(key).ok()).and_then(|v| number(doc, v));
    let file = system_font(base, entry(b"Flags").unwrap_or(0.0) as i64, entry(b"FontWeight").unwrap_or(0.0) as f32);
    let data = system_font_data(&file)?;
    FontRef::new(data.as_slice()).is_ok().then_some(Program::OpenType(data))
}

/// A CID-keyed CFF font's glyph for each CID, from its charset; `None` for a
/// CFF font that isn't CID-keyed, whose glyphs are numbered by CID.
fn cff_glyphs_by_cid(data: &[u8]) -> Option<Vec<u16>> {
    let table = cff::Table::parse(data).filter(cff::Table::is_cid)?;
    let mut glyphs = Vec::new();
    for glyph in 0..table.number_of_glyphs() {
        if let Some(cid) = table.glyph_cid(hayro_font::GlyphId(glyph)).map(usize::from) {
            if glyphs.len() <= cid {
                glyphs.resize(cid + 1, 0);
            }
            glyphs[cid] = glyph;
        }
    }
    Some(glyphs)
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

/// A simple font's widths: `Widths` from `FirstChar`, and the font program's
/// own advances for its `glyphs` where that doesn't cover them.
fn simple_widths(doc: &Document, font: &Dictionary, program: &Program, units: Matrix, glyphs: &[u32; 256]) -> Box<[f32; 256]> {
    let own: Vec<f32> = match program {
        Program::OpenType(data) => FontRef::new(data)
            .map(|face| {
                let metrics = face.glyph_metrics(Size::unscaled(), LocationRef::default());
                glyphs.iter().map(|&glyph| metrics.advance_width(GlyphId::new(glyph)).unwrap_or(0.0)).collect()
            })
            .unwrap_or_default(),
        Program::Cff(data) => cff::Table::parse(data)
            .map(|table| glyphs.iter().map(|&glyph| u16::try_from(glyph).ok().and_then(|g| table.glyph_width(hayro_font::GlyphId(g))).map_or(0.0, f32::from)).collect())
            .unwrap_or_default(),
        Program::Type1 { .. } => Vec::new(),
    };
    let mut widths = Box::new(std::array::from_fn(|code| own.get(code).copied().unwrap_or(0.0) * units.0[0] * 1000.0));
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

/// A simple font's glyph for each code, the name its encoding's `differences`
/// give it first, then the font program's own encoding.
fn simple_glyphs(differences: &HashMap<u8, &[u8]>, program: &mut Program) -> Box<[u32; 256]> {
    let named = |code: usize| differences.get(&(code as u8)).and_then(|name| std::str::from_utf8(name).ok());
    Box::new(match program {
        Program::OpenType(data) => opentype_glyphs(data, named),
        Program::Cff(data) => match cff::Table::parse(data) {
            Some(table) => std::array::from_fn(|code| {
                let glyph = named(code).and_then(|name| table.glyph_index_by_name(name)).or_else(|| table.glyph_index(code as u8));
                glyph.map_or(0, |g| g.0.into())
            }),
            None => [0; 256],
        },
        Program::Type1 { table, names } => std::array::from_fn(|code| match named(code).or_else(|| table.code_to_string(code as u8)) {
            Some(".notdef") | None => 0,
            Some(name) => {
                let index = names.iter().position(|known| known == name).unwrap_or_else(|| {
                    names.push(name.to_owned());
                    names.len() - 1
                });
                index as u32 + 1
            }
        }),
    })
}

/// A TrueType or OpenType simple font's glyph for each code: by the name
/// `named` gives it, else through the font's character map -- a symbol map,
/// a Unicode one taking codes as Windows' Latin encoding, or a Macintosh one
/// -- else, without any map, numbered the same.
fn opentype_glyphs<'n>(data: &[u8], named: impl Fn(usize) -> Option<&'n str>) -> [u32; 256] {
    let Ok(font) = FontRef::new(data) else { return [0; 256] };
    let subtables: Vec<(PlatformId, u16, CmapSubtable)> = font
        .cmap()
        .map(|cmap| cmap.encoding_records().iter().filter_map(|r| Some((r.platform_id(), r.encoding_id(), r.subtable(cmap.offset_data()).ok()?))).collect())
        .unwrap_or_default();
    let find = |platform: PlatformId, encoding: u16| subtables.iter().find(|(p, e, _)| *p == platform && *e == encoding).map(|(_, _, s)| s);
    let (symbol, unicode, mac) = (find(PlatformId::Windows, 0), find(PlatformId::Windows, 1).or_else(|| find(PlatformId::Unicode, 3)), find(PlatformId::Macintosh, 0));
    let mut names: Option<HashMap<String, GlyphId>> = None;
    std::array::from_fn(|code| {
        let by_name = named(code).and_then(|name| {
            let names = names.get_or_insert_with(|| font.glyph_names().iter().map(|(glyph, name)| (name.as_str().to_owned(), glyph)).collect());
            names.get(name).copied().or_else(|| unicode?.map_codepoint(unicode_of_name(name)?))
        });
        let glyph = by_name
            .or_else(|| symbol.and_then(|s| s.map_codepoint(0xF000 + code as u32).or_else(|| s.map_codepoint(code as u32))))
            .or_else(|| unicode.and_then(|s| s.map_codepoint(win_ansi(code as u8))))
            .or_else(|| mac.and_then(|s| s.map_codepoint(code as u32)));
        glyph.map_or(if subtables.is_empty() { code as u32 } else { 0 }, GlyphId::to_u32)
    })
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

    /// A simple font of `subtype` embedding `program` as `file`.
    fn embedding(subtype: &str, file: &str, program: Stream) -> Dictionary {
        dictionary! { "Subtype" => subtype, "FontDescriptor" => dictionary! { file => program } }
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
    fn a_subset_prefix_doesnt_hide_the_family() {
        assert_eq!(system_font("ABCDEF+TimesNewRomanPSMT", 0, 0.0), "times.ttf", "a name after a subset prefix");
        // The prefix is only stripped where it's the six letters PDF says.
        assert_eq!(system_font("Courier", 0, 0.0), "cour.ttf");
    }

    #[test]
    fn a_simple_font_without_a_character_map_numbers_glyphs_by_code() {
        let mut font = embedding("TrueType", "FontFile2", Stream::new(dictionary! {}, square_font()));
        font.set("FirstChar", 1);
        font.set("Widths", vec![Object::Integer(700)]);
        let font = load(&font).unwrap();
        assert_eq!(font.codes(b"\x01 "), [Code { glyph: 1, width: 700.0, is_space: false }, Code { glyph: 32, width: 0.0, is_space: true }]);
    }

    #[test]
    fn a_font_that_isnt_embedded_is_drawn_in_one_from_the_system() {
        let mut font = load(&dictionary! { "Subtype" => "Type1", "BaseFont" => "Helvetica" }).expect("Arial stands in for Helvetica");
        let codes = font.codes(b"Ag");
        assert!(codes.iter().all(|code| code.glyph != 0), "both letters have glyphs: {codes:?}");
        assert!(codes[0].width > 0.0 && codes[1].width > 0.0, "and widths from the font: {codes:?}");
        let mut tessellator = FillTessellator::new();
        assert!(!font.triangles(codes[0].glyph, 0.001, &mut tessellator).is_empty(), "and an outline to draw");
    }

    #[test]
    fn the_font_standing_in_follows_the_name_then_the_flags() {
        // The standard fourteen, in the metric-compatible Windows families.
        assert_eq!(system_font("Helvetica", 0, 0.0), "arial.ttf");
        assert_eq!(system_font("Helvetica-BoldOblique", 0, 0.0), "arialbi.ttf");
        assert_eq!(system_font("Times-Roman", SERIF, 0.0), "times.ttf");
        assert_eq!(system_font("TimesNewRomanPS-ItalicMT", SERIF, 0.0), "timesi.ttf");
        assert_eq!(system_font("Courier", FIXED_PITCH, 0.0), "cour.ttf");
        assert_eq!(system_font("Symbol", 0, 0.0), "symbol.ttf");
        // Then the descriptor's flags and weight.
        assert_eq!(system_font("SomeUnknownFace", SERIF, 0.0), "times.ttf");
        assert_eq!(system_font("SomeUnknownFace", FIXED_PITCH, 0.0), "cour.ttf");
        assert_eq!(system_font("SomeUnknownFace", 0, 0.0), "arial.ttf");
        assert_eq!(system_font("SomeUnknownFace", ITALIC, 700.0), "arialbi.ttf");
    }

    #[test]
    fn fonts_that_cant_be_drawn_say_why() {
        assert_eq!(load(&dictionary! { "Subtype" => "Type3" }).err(), Some("text in Type 3 fonts"));
        let mut vertical = type0_font();
        vertical.set("Encoding", "Identity-V");
        assert_eq!(load(&vertical).err(), Some("text in encodings other than Identity-H"));
        let garbage = || Stream::new(dictionary! { "Subtype" => "Type1C" }, b"not a font".to_vec());
        for (subtype, file) in [("Type1", "FontFile3"), ("Type1", "FontFile"), ("TrueType", "FontFile2")] {
            assert_eq!(load(&embedding(subtype, file, garbage())).err(), Some(UNREADABLE), "{file}");
        }
    }

    #[test]
    fn glyph_names_that_say_their_character_are_read() {
        assert_eq!((unicode_of_name("uni20AC"), unicode_of_name("A"), unicode_of_name("Aacute")), (Some(0x20AC), Some(65), None));
        assert_eq!((win_ansi(0x80), win_ansi(b'A')), (0x20AC, 65));
    }
}
