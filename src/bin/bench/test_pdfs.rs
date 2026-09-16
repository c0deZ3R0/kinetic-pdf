//! Drawing sets built to order, so a benchmark measures what it means to.
//!
//! Benchmarking against real drawing sets turned out to measure the sampling
//! more than the renderer: sheets in one set ran from 4 MB of shapes to 66 MB,
//! so which sheets a run happened to visit moved the answer more than the thing
//! being compared did. These are uniform -- every sheet in a set costs the same
//! to draw, and differs only in where its lines and photographs fall -- so runs
//! are comparable, and each set isolates one kind of work:
//!
//! - `lines`  -- nothing but strokes. The geometry path, which is what drawing
//!               from shapes is for, and which costs the same at every zoom.
//! - `text`   -- dense schedules in a font the file doesn't embed, which is the
//!               substitution path (`gpu-lines/src/font.rs`).
//! - `photos` -- big JPEGs, which is the image atlas: the part that grows with
//!               the square of the zoom and decides whether a sheet stays on
//!               the GPU at all.
//! - `mixed`  -- a title block, line work and two photographs: a real sheet.
//!
//! Sizes are chosen to sit in the range measured on real sets, so what is
//! measured here still says something about the work. As built:
//!
//! ```text
//!            shapes at 0.125   at 2 px/pt   pdfium renders a sheet in
//!   lines              4 MB         4 MB                      686 ms
//!   text              (see below)                               86 ms
//!   photos             0 MB        58 MB                      177 ms
//!   mixed              2 MB        20 MB                      327 ms
//!   a real sheet    4-10 MB      7-102 MB                   64-84 ms
//! ```
//!
//! Shapes line up with real sheets; `lines` is heavier than a real one for
//! pdfium to rasterise, so read it as the top end of line-work complexity
//! rather than the middle. `text` was first built at 8 MB of shapes, with its
//! text in the top-left of the sheet only; it now covers the sheet, about three
//! times the characters, and its shapes haven't been measured again. The 86 ms
//! is the median time for its first sheet to come sharp in the app, not a bare
//! render. Written by `bench --make-test-pdfs`.

use std::path::{Path, PathBuf};

use pdf_content::lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

/// A1 landscape in PDF points, the usual drawing sheet.
const SHEET: (f32, f32) = (2384.0, 1684.0);

/// Sheets in each set. Enough that drawing ahead can't have done the work for
/// a sample, few enough that a run doesn't take all afternoon.
const SHEETS: usize = 40;

/// Strokes on a `lines` sheet. Tuned so its shapes land near a real sheet's.
const STROKES: usize = 180_000;

/// Segments in one drawn run -- a wall, a kerb, a length of hatching. Real
/// drawings join their strokes up; scattered single segments would be the worst
/// case for a rasteriser and would flatter us against pdfium.
const SEGMENTS_A_RUN: usize = 12;

/// Rows of text in each block on a `text` sheet, characters in each row, and
/// blocks across the sheet. Together they cover the whole sheet: an earlier
/// version filled only its top-left, so zoomed in on the middle a benchmark
/// timed blank paper.
const TEXT_ROWS: usize = 130;
const TEXT_COLUMNS: usize = 80;
const TEXT_BLOCKS: usize = 5;

/// Photographs on a `photos` sheet, and how many pixels across each one is.
/// Four of these at 2,200 px come to about 70 MB in the atlas when the sheet is
/// drawn at two pixels a point, so the sheet is handed back to pdfium at four --
/// the same place a real photo-laden sheet gives up.
const PHOTOS: usize = 4;
const PHOTO_PIXELS: u32 = 2_200;

/// Strokes and photographs on a `mixed` sheet.
const MIXED_STROKES: usize = 60_000;
const MIXED_PHOTOS: usize = 2;

/// A deterministic sequence, so a set built today matches one built last week.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number from 0 up to but not including `top`.
    fn upto(&mut self, top: f32) -> f32 {
        (self.next() >> 11) as f32 / (1u64 << 53) as f32 * top
    }
}

/// Everything a page needs written into the document, and the page dictionary
/// entries that point at it.
struct Page {
    content: Vec<u8>,
    resources: Dictionary,
}

/// The sets this builds, by name.
pub fn sets() -> [&'static str; 4] {
    ["lines", "text", "photos", "mixed"]
}

/// Writes every set into `dir`, returning what it wrote. Files already there
/// with the right size are left alone, so repeated runs are quick.
pub fn write_all(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut made = Vec::new();
    for name in sets() {
        let path = dir.join(format!("{name}.pdf"));
        if !path.exists() {
            let bytes = build(name)?;
            std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        }
        made.push((format!("{name} ({SHEETS} sheets)"), path));
    }
    Ok(made)
}

fn build(name: &str) -> Result<Vec<u8>, String> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    // One font object shared by every sheet that needs it: Helvetica, which the
    // file doesn't embed, so the renderer has to find something to stand in.
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
    });

    let mut kids = Vec::new();
    for sheet in 0..SHEETS {
        // Seeded per sheet, so sheets differ but each costs the same to draw.
        let mut rng = Rng(0x5eed_0000_0000_0001 ^ (sheet as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let page = match name {
            "lines" => lines_sheet(&mut rng, STROKES),
            "text" => text_sheet(&mut rng, font),
            "photos" => photo_sheet(&mut doc, &mut rng, PHOTOS)?,
            "mixed" => mixed_sheet(&mut doc, &mut rng, font)?,
            other => return Err(format!("no such set: {other}")),
        };
        let content = doc.add_object(Stream::new(dictionary! {}, page.content).with_compression(true));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content,
            "Resources" => page.resources,
            "MediaBox" => vec![0.into(), 0.into(), SHEET.0.into(), SHEET.1.into()],
        });
        kids.push(page_id.into());
    }

    let count = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! {
        "Type" => "Pages", "Kids" => kids, "Count" => count,
    }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);

    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/* ------------------------------------------------------------------ *
 * The sheets
 * ------------------------------------------------------------------ */

/// A sheet of nothing but line work: a border, a grid of rooms, and hatching,
/// in a few weights and colours so the style table has something to do.
fn lines_sheet(rng: &mut Rng, strokes: usize) -> Page {
    let mut c = String::with_capacity(strokes * 34);
    border(&mut c);
    // Strokes are batched between paint operators, as a drawing's own content
    // stream does, rather than one path each.
    const PER_PATH: usize = 200;
    let mut drawn = 0;
    while drawn < strokes {
        let batch = PER_PATH.min(strokes - drawn);
        // A weight and a colour per batch: enough variety to intern styles,
        // not so much that every stroke is its own.
        let weight = [0.25f32, 0.5, 0.7, 1.0, 1.4][(rng.next() % 5) as usize];
        let grey = [0.0f32, 0.1, 0.25, 0.45][(rng.next() % 4) as usize];
        c.push_str(&format!("{weight} w {grey} {grey} {grey} RG\n"));
        let mut in_batch = 0;
        while in_batch < batch {
            // Segments come in runs that share a start, as a drawn object does:
            // a wall, a kerb, a run of hatching. Scattering single segments
            // instead would be the worst case for any rasteriser and would
            // flatter us -- a real sheet's strokes join up.
            let segments = (SEGMENTS_A_RUN / 2 + (rng.next() as usize % SEGMENTS_A_RUN)).min(batch - in_batch);
            let (mut x, mut y) = (rng.upto(SHEET.0 - 160.0) + 80.0, rng.upto(SHEET.1 - 160.0) + 80.0);
            c.push_str(&format!("{x:.1} {y:.1} m"));
            for _ in 0..segments {
                // Orthogonal steps, as plans and hatching are, with the odd
                // long one for a grid or dimension line.
                let long = rng.next() % 24 == 0;
                let run = if long { rng.upto(500.0) + 120.0 } else { rng.upto(60.0) + 5.0 };
                let sign = if rng.next() % 2 == 0 { 1.0 } else { -1.0 };
                if rng.next() % 2 == 0 {
                    x = (x + run * sign).clamp(40.0, SHEET.0 - 40.0);
                } else {
                    y = (y + run * sign).clamp(40.0, SHEET.1 - 40.0);
                }
                c.push_str(&format!(" {x:.1} {y:.1} l"));
            }
            c.push('\n');
            in_batch += segments;
        }
        c.push_str("S\n");
        drawn += batch;
    }
    Page { content: c.into_bytes(), resources: dictionary! {} }
}

/// A sheet of dense schedules and notes, in a font the file doesn't embed.
fn text_sheet(rng: &mut Rng, font: ObjectId) -> Page {
    const WORDS: [&str; 24] = [
        "SETTING", "OUT", "LEVEL", "DATUM", "REFER", "TO", "ENGINEER", "DETAIL", "SLAB", "EDGE", "REINFORCEMENT",
        "COVER", "GRID", "LINE", "SECTION", "ELEVATION", "TYPICAL", "UNLESS", "NOTED", "OTHERWISE", "DIMENSION",
        "VERIFY", "ON", "SITE",
    ];
    let mut c = String::with_capacity(TEXT_ROWS * TEXT_COLUMNS * 2);
    c.push_str("BT /F1 7 Tf 12 TL 60 1620 Td\n");
    for _ in 0..TEXT_ROWS {
        let mut line = String::with_capacity(TEXT_COLUMNS + 8);
        while line.len() < TEXT_COLUMNS {
            line.push_str(WORDS[(rng.next() % WORDS.len() as u64) as usize]);
            line.push(' ');
        }
        line.truncate(TEXT_COLUMNS);
        c.push_str(&format!("({line}) Tj T*\n"));
    }
    c.push_str("ET\n");
    // Blocks of it side by side, as a schedule sheet has, inside one border.
    let one = std::mem::take(&mut c);
    border(&mut c);
    c.push_str(&one);
    for block in 1..TEXT_BLOCKS {
        c.push_str(&format!("q 1 0 0 1 {} 0 cm\n{one}Q\n", block * 460));
    }
    Page { content: c.into_bytes(), resources: dictionary! { "Font" => dictionary! { "F1" => font } } }
}

/// A sheet of photographs, as a survey or condition report carries.
fn photo_sheet(doc: &mut Document, rng: &mut Rng, count: usize) -> Result<Page, String> {
    let mut c = String::new();
    border(&mut c);
    let mut xobjects = Dictionary::new();
    // Laid out two across, filling the sheet, as printed photographs are.
    let across = 2usize;
    let (cell_w, cell_h) = (SHEET.0 / across as f32 - 90.0, SHEET.1 / ((count + 1) / across) as f32 - 90.0);
    for i in 0..count {
        let id = doc.add_object(photograph(rng, PHOTO_PIXELS)?);
        let name = format!("Im{i}");
        xobjects.set(name.as_bytes().to_vec(), id);
        let (col, row) = (i % across, i / across);
        let x = 60.0 + col as f32 * (SHEET.0 / across as f32);
        let y = 60.0 + row as f32 * (SHEET.1 / ((count + 1) / across) as f32);
        c.push_str(&format!("q {cell_w:.1} 0 0 {cell_h:.1} {x:.1} {y:.1} cm /{name} Do Q\n"));
    }
    Ok(Page { content: c.into_bytes(), resources: dictionary! { "XObject" => xobjects } })
}

/// A real sheet: a title block, line work, and a couple of photographs.
fn mixed_sheet(doc: &mut Document, rng: &mut Rng, font: ObjectId) -> Result<Page, String> {
    let lines = lines_sheet(rng, MIXED_STROKES);
    let mut c = String::from_utf8(lines.content).map_err(|e| e.to_string())?;
    let mut xobjects = Dictionary::new();
    for i in 0..MIXED_PHOTOS {
        let id = doc.add_object(photograph(rng, PHOTO_PIXELS)?);
        let name = format!("Im{i}");
        xobjects.set(name.as_bytes().to_vec(), id);
        let y = 120.0 + i as f32 * 620.0;
        c.push_str(&format!("q 800 0 0 600 {:.1} {y:.1} cm /{name} Do Q\n", SHEET.0 - 900.0));
    }
    // A title block along the bottom right, as every sheet has.
    c.push_str("0.8 w 0 0 0 RG\n");
    c.push_str(&format!("{} 40 m {} 40 l {} 300 l {} 300 l h S\n", SHEET.0 - 900.0, SHEET.0 - 40.0, SHEET.0 - 40.0, SHEET.0 - 900.0));
    c.push_str("BT /F1 11 Tf 13 TL ");
    c.push_str(&format!("{} 270 Td\n", SHEET.0 - 880.0));
    for line in ["PROJECT  SYNTHETIC TEST SET", "DRAWING  GENERAL ARRANGEMENT", "SCALE  1:50 AT A1", "REVISION  P01", "STATUS  FOR BENCHMARK"] {
        c.push_str(&format!("({line}) Tj T*\n"));
    }
    c.push_str("ET\n");
    Ok(Page {
        content: c.into_bytes(),
        resources: dictionary! { "Font" => dictionary! { "F1" => font }, "XObject" => xobjects },
    })
}

/* ------------------------------------------------------------------ *
 * Pieces
 * ------------------------------------------------------------------ */

/// The sheet border and trim marks every drawing carries.
fn border(c: &mut String) {
    c.push_str("1.4 w 0 0 0 RG\n");
    c.push_str(&format!("20 20 {} {} re S\n", SHEET.0 - 40.0, SHEET.1 - 40.0));
    c.push_str(&format!("40 40 {} {} re S\n", SHEET.0 - 80.0, SHEET.1 - 80.0));
}

/// A photograph: smooth shapes with grain over them, so it compresses like a
/// picture rather than to nothing, encoded as the JPEG a scan or a camera
/// would give. This is the work the image atlas and the decoder actually do.
fn photograph(rng: &mut Rng, pixels: u32) -> Result<Stream, String> {
    let (w, h) = (pixels, pixels * 3 / 4);
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    // A few soft blobs of colour, then grain: enough detail that JPEG keeps a
    // realistic amount of data, without looking like pure noise, which would
    // not compress at all and would flatter nobody.
    let blobs: Vec<(f32, f32, f32, [f32; 3])> = (0..7)
        .map(|_| {
            (
                rng.upto(w as f32),
                rng.upto(h as f32),
                rng.upto(w as f32 / 3.0) + w as f32 / 8.0,
                [rng.upto(1.0), rng.upto(1.0), rng.upto(1.0)],
            )
        })
        .collect();
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32, y as f32);
            let mut colour = [0.45f32, 0.47, 0.5];
            for &(bx, by, r, tint) in &blobs {
                let d = ((fx - bx).powi(2) + (fy - by).powi(2)).sqrt();
                if d < r {
                    let weight = 1.0 - d / r;
                    for channel in 0..3 {
                        colour[channel] += (tint[channel] - colour[channel]) * weight * 0.55;
                    }
                }
            }
            let grain = (rng.next() % 37) as f32 / 37.0 - 0.5;
            let at = ((y * w + x) * 3) as usize;
            for channel in 0..3 {
                rgb[at + channel] = ((colour[channel] + grain * 0.09).clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 82)
        .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w as i64,
            "Height" => h as i64,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        jpeg,
    ))
}
