//! Exact glyph measurements for the eraser's uncommon-font fallback.
//! Runs on the existing PDFium worker; the probe PDF is never saved to disk.

use std::{collections::HashMap, fmt::Write as _};
use pdf_content::{lopdf::{dictionary, Dictionary, Document, Object, Stream}, objects::copy_object};
use pdfium_render::prelude::*;

pub(crate) struct Metrics<'a>(pub &'a Pdfium);

impl gpu_lines::TextMetrics for Metrics<'_> {
    fn measure(&mut self, source: &Document, font: &Dictionary, codes: &[Vec<u8>]) -> Result<Vec<gpu_lines::GlyphMetrics>, String> {
        let mut probe = Document::with_version("1.7");
        let font = copy_object(source, &Object::Dictionary(font.clone()), &mut probe, &mut HashMap::new());
        // A marker in a different font follows the glyph without moving the
        // text position. Its matrix gives the advance, including vertical
        // writing and custom CMaps, independently of Unicode extraction.
        let mut content = String::new();
        for code in codes {
            content.push_str("BT /Glyph 1000 Tf 1 0 0 1 2000 2000 Tm <");
            for byte in code { let _ = write!(content, "{byte:02X}"); }
            content.push_str("> Tj /Marker 1 Tf (X) Tj ET\n");
        }
        let stream = probe.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let pages = probe.new_object_id();
        let page = probe.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages,
            "MediaBox" => vec![0.into(),0.into(),10000.into(),10000.into()],
            "Contents" => stream,
            "Resources" => dictionary! { "Font" => dictionary! {
                "Glyph" => font,
                "Marker" => dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" }
            } }
        });
        probe.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
        let root = probe.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        probe.trailer.set("Root", root);
        let mut bytes = Vec::new();
        probe.save_to(&mut bytes).map_err(|e| e.to_string())?;
        let document = self.0.load_pdf_from_byte_vec(bytes, None).map_err(|e| e.to_string())?;
        let page = document.pages().first().map_err(|e| e.to_string())?;
        let objects = page.objects();
        if objects.len() as usize != codes.len() * 2 {
            return Err("The PDF engine could not measure this font safely; the original file has not been replaced".into());
        }
        (0..codes.len()).map(|index| {
            let glyph = objects.get((index * 2) as _).map_err(|e| e.to_string())?;
            let marker = objects.get((index * 2 + 1) as _).map_err(|e| e.to_string())?;
            let bounds = glyph.bounds().map_err(|e| e.to_string())?;
            let matrix = marker.matrix().map_err(|e| e.to_string())?;
            let normalize = |value: f32| (value - 2000.0) / 1000.0;
            let measured = gpu_lines::GlyphMetrics {
                bounds: [bounds.left().value, bounds.bottom().value, bounds.right().value, bounds.top().value].map(normalize),
                advance: [normalize(matrix.e()), normalize(matrix.f())],
            };
            if measured.bounds.iter().chain(&measured.advance).all(|v| v.is_finite()) { Ok(measured) }
            else { Err("The PDF engine returned invalid character positions; the original file has not been replaced".into()) }
        }).collect()
    }
}
