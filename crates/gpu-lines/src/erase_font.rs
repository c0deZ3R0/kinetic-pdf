//! Text erasing needs character advances and ink bounds, not a drawable font.
//! In particular, Type 3 glyph procedures and custom CID encodings can be
//! edited even when the GPU renderer delegates their drawing to PDFium.

use pdf_content::lexer::{each_operation, Operand};
use pdf_content::lopdf::{Dictionary, Document, Object};
use pdf_content::objects::number;
use std::collections::HashMap;
use std::sync::Arc;
use crate::{font::Font, geometry::{Matrix, Piece}};

pub(crate) struct Glyph {
    pub bytes: usize,
    pub width: f32,
    pub space: bool,
    pub outline: Arc<[Piece]>,
}

/// Measurements in text space for a glyph shown at a font size of one.
#[derive(Clone, Copy, Debug)]
pub struct GlyphMetrics {
    pub bounds: [f32; 4],
    pub advance: [f32; 2],
}

/// The application supplies its PDF engine for fonts the fast reader cannot
/// measure. No pixels or substitute text are written into the document.
pub trait TextMetrics {
    fn measure(&mut self, doc: &Document, font: &Dictionary, codes: &[Vec<u8>]) -> Result<Vec<GlyphMetrics>, String>;
}

pub(crate) enum EraseFont {
    Normal { font: Font, outlines: HashMap<u32, Arc<[Piece]>> },
    Measured { font: Dictionary, map: Option<CodeMap>, cache: HashMap<Vec<u8>, GlyphMetrics> },
}

impl EraseFont {
    pub fn fallback(doc: &Document, font: &Dictionary) -> Option<Self> {
        let map = if font.get(b"Subtype").ok()?.as_name().ok()? == b"Type0" {
            Some(CodeMap::read(doc, font.get(b"Encoding").ok()?, 0)?)
        } else { None };
        Some(Self::Measured { font: font.clone(), map, cache: HashMap::new() })
    }

    pub fn vertical(&self) -> bool {
        matches!(self, Self::Measured { map: Some(map), .. } if map.vertical)
    }

    pub fn measure(&mut self, doc: &Document, bytes: &[u8], metrics: &mut Option<&mut dyn TextMetrics>) -> Result<Vec<Glyph>, String> {
        if let Self::Measured { font, map, cache } = self {
            let mut codes = Vec::new();
            let mut tail = bytes;
            while !tail.is_empty() {
                let length = match map { Some(map) => map.next(tail).map(|(n,_)| n).ok_or("Cannot decode a character in this font for erasing")?, None => 1 };
                codes.push(tail[..length].to_vec());
                tail = &tail[length..];
            }
            let mut missing: Vec<Vec<u8>> = codes.iter().filter(|code| !cache.contains_key(*code)).cloned().collect();
            missing.sort();
            missing.dedup();
            if !missing.is_empty() {
                let engine = metrics.as_deref_mut().ok_or("This font needs the PDF engine to erase text")?;
                let measured = engine.measure(doc, font, &missing)?;
                if measured.len() != missing.len() { return Err("Could not measure every character for erasing".into()); }
                cache.extend(missing.into_iter().zip(measured));
            }
            let vertical = map.as_ref().is_some_and(|m| m.vertical);
            return Ok(codes.iter().map(|code| {
                let m = cache[code];
                Glyph { bytes: code.len(), width: m.advance[usize::from(vertical)] * 1000.0,
                    space: code.as_slice() == b" ", outline: box_outline(m.bounds, Matrix::IDENTITY).into() }
            }).collect());
        }
        self.glyphs(bytes).ok_or_else(|| "Cannot decode a character in this font for erasing".into())
    }

    pub fn load(doc: &Document, font: &Dictionary) -> Option<Self> {
        match Font::load(doc, font) {
            Ok(font) => Some(Self::Normal { font, outlines: HashMap::new() }),
            Err(_) => Self::fallback(doc, font),
        }
    }

    fn glyphs(&mut self, bytes: &[u8]) -> Option<Vec<Glyph>> {
        let Self::Normal { font, outlines } = self else { return None };
        let length = font.code_length();
        if bytes.len() % length != 0 { return None; }
        Some(font.codes(bytes).into_iter().map(|c| Glyph {
            bytes: length, width: c.width, space: c.is_space,
            outline: Arc::clone(outlines.entry(c.glyph).or_insert_with(|| font.outline(c.glyph).into())),
        }).collect())
    }
}
fn box_outline([l,b,r,t]: [f32;4], units: Matrix) -> Vec<Piece> {
    vec![Piece::Move([l,b]), Piece::Line([r,b]), Piece::Line([r,t]), Piece::Line([l,t]), Piece::Close]
        .into_iter().map(|p| p.transformed(units)).collect()
}

#[derive(Default)]
pub(crate) struct CodeMap {
    spaces: Vec<(usize, u32, u32)>,
    ranges: Vec<(usize, u32, u32, u32)>,
    vertical: bool,
    predefined: Option<hayro_cmap::CMap>,
}

fn code(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || bytes.len() > 4 { return None; }
    Some(bytes.iter().fold(0, |n, b| (n << 8) | u32::from(*b)))
}

impl CodeMap {
    fn read(doc: &Document, object: &Object, depth: usize) -> Option<Self> {
        if depth > 16 { return None; }
        match doc.dereference(object).ok()?.1 {
            Object::Name(name) if name == b"Identity-H" || name == b"Identity-V" => Some(Self {
                spaces: vec![(2,0,65535)], ranges: vec![(2,0,65535,0)], vertical: name == b"Identity-V", predefined: None,
            }),
            Object::Name(name) => {
                let data = hayro_cmap::load_embedded(hayro_cmap::CMapName::from_bytes(name))?;
                let map = hayro_cmap::CMap::parse(data, hayro_cmap::load_embedded)?;
                Some(Self { vertical: map.metadata().writing_mode == Some(hayro_cmap::WritingMode::Vertical), predefined: Some(map), ..Self::default() })
            }
            Object::Stream(stream) => {
                let mut map = match stream.dict.get(b"UseCMap") {
                    Ok(base) => Self::read(doc, base, depth + 1)?,
                    Err(_) => Self::default(),
                };
                let content = stream.get_plain_content().ok()?;
                let mut valid = true;
                each_operation(&content, |op, operands, _| {
                    match op {
                        b"def" if operands.first().and_then(Operand::name) == Some(b"WMode") => {
                            map.vertical = operands.get(1).and_then(Operand::number) == Some(1.0);
                        }
                        b"usecmap" => {
                            let base = operands.last().and_then(Operand::name).and_then(|n| Self::read(doc, &Object::Name(n.to_vec()), depth + 1));
                            if let Some(base) = base { map = base; } else { valid = false; }
                        }
                        b"endcodespacerange" | b"endcidrange" | b"endcidchar" => {
                            let stride = if op == b"endcidrange" { 3 } else { 2 };
                            for pair in operands.chunks(stride) {
                                let entry = (|| {
                                    let low = pair.first()?.bytes()?;
                                    let start = code(&low)?;
                                    if op == b"endcidchar" {
                                        let cid = pair.get(1)?.number()? as u32;
                                        map.ranges.push((low.len(), start, start, cid));
                                    } else {
                                        let high = pair.get(1)?.bytes()?;
                                        let end = code(&high)?;
                                        if high.len() != low.len() || end < start { return None; }
                                        if op == b"endcodespacerange" { map.spaces.push((low.len(), start, end)); }
                                        else { map.ranges.push((low.len(), start, end, pair.get(2)?.number()? as u32)); }
                                    }
                                    Some(())
                                })();
                                valid &= entry.is_some();
                            }
                        }
                        _ => {}
                    }
                });
                if let Ok(mode) = stream.dict.get(b"WMode") { map.vertical = number(doc, mode)? == 1.0; }
                (valid && (!map.spaces.is_empty() || map.predefined.is_some())).then_some(map)
            }
            _ => None,
        }
    }

    fn next(&self, bytes: &[u8]) -> Option<(usize, u16)> {
        for length in 1..=4 {
            let Some(value) = bytes.get(..length).and_then(code) else { continue };
            if self.spaces.iter().any(|&(n,low,high)| n == length && (low..=high).contains(&value)) {
                let cid = self.ranges.iter().rev().find(|&&(n,low,high,_)| n == length && (low..=high).contains(&value))
                    .map_or(0, |&(_,low,_,cid)| cid + value - low);
                return Some((length, u16::try_from(cid).ok()?));
            }
            if let Some(cid) = self.predefined.as_ref().and_then(|m| m.lookup_cid_code(value, length as u8)) {
                return Some((length, u16::try_from(cid).ok()?));
            }
        }
        None
    }
}
