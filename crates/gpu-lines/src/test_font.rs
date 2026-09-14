//! A tiny TrueType font for tests, and a PDF font embedding it.

use pdf_content::lopdf::{dictionary, Dictionary, Object, Stream};

/// Big-endian bytes of 16-bit values.
fn u16s(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// A TrueType font with a 1000-unit em and two glyphs, each advancing 500
/// units: glyph 0 is empty, glyph 1 a square 100 units a side at the origin.
pub(crate) fn square_font() -> Vec<u8> {
    let mut head = vec![0; 54];
    head[0..4].copy_from_slice(&[0, 1, 0, 0]);
    head[12..16].copy_from_slice(&0x5F0F_3CF5_u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
    head[40..44].copy_from_slice(&u16s(&[100, 100]));
    let mut hhea = vec![0; 36];
    hhea[0..4].copy_from_slice(&[0, 1, 0, 0]);
    hhea[34..36].copy_from_slice(&2_u16.to_be_bytes());
    let maxp = [&[0, 0, 0x50, 0][..], &u16s(&[2])].concat();
    let hmtx = u16s(&[500, 0, 500, 0]);
    // One contour, its box, its last point's index, no instructions; four
    // points on the curve, each coordinate a 16-bit step from the last.
    let square = [u16s(&[1, 0, 0, 100, 100, 3, 0]), vec![1; 4], u16s(&[0, 100, 0, -100_i16 as u16, 0, 0, 100, 0])].concat();
    let loca = u16s(&[0, 0, (square.len() / 2) as u16]);
    let tables: [(&[u8; 4], Vec<u8>); 6] = [(b"glyf", square), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp)];

    let header = 12 + 16 * tables.len();
    let mut font = [&[0, 1, 0, 0][..], &u16s(&[tables.len() as u16, 64, 2, 32])].concat();
    let mut data = Vec::new();
    for (tag, table) in &tables {
        font.extend_from_slice(*tag);
        font.extend([0; 4]);
        font.extend(((header + data.len()) as u32).to_be_bytes());
        font.extend((table.len() as u32).to_be_bytes());
        data.extend_from_slice(table);
        data.resize(data.len().next_multiple_of(4), 0);
    }
    font.extend(data);
    font
}

/// A Type 0 font embedding `square_font`, showing two-byte codes as the glyphs
/// they number, each half an em wide.
pub(crate) fn type0_font() -> Dictionary {
    let descriptor = dictionary! { "Type" => "FontDescriptor", "FontName" => "Square", "FontFile2" => Stream::new(dictionary! {}, square_font()) };
    let descendant = dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => "Square",
        "CIDToGIDMap" => "Identity",
        "DW" => Object::Integer(500),
        "FontDescriptor" => descriptor,
    };
    dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "Square",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Dictionary(descendant)],
    }
}
