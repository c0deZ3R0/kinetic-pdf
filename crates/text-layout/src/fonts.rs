//! The fonts installed on this machine: what families there are, each with
//! its regular, bold, italic and bold italic faces, and a face's glyphs and
//! metrics once it's wanted.
//!
//! Finding them reads only the few bytes of each file that name it -- its
//! table directory and its `name`, `OS/2` and `head` tables -- so looking
//! through a few hundred fonts takes milliseconds, not the seconds reading
//! them whole would. A face is read whole only when text is set in it.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

/// One face of an installed font, as found.
#[derive(Clone, Debug, PartialEq)]
pub struct FontEntry {
    /// The family it's one of: the name a font menu lists.
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// Its PostScript name, which a PDF names it by.
    pub postscript: String,
    pub path: PathBuf,
    /// Its glyphs are CFF outlines, in an OpenType file, rather than
    /// TrueType ones: a PDF embeds the two differently.
    pub cff: bool,
}

/// A face read whole: its program, and what setting text in it needs.
pub struct Face {
    pub entry: FontEntry,
    pub data: Arc<Vec<u8>>,
    /// Its design units to an em.
    pub units_per_em: f32,
    /// Above the baseline, below it (negative) and the gap between lines, as
    /// fractions of an em.
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    /// Where an underline goes below the baseline (negative), and how thick,
    /// as fractions of an em.
    pub underline_offset: f32,
    pub underline_thickness: f32,
    /// Its glyphs, by character: the glyph and its advance in ems. Glyph 0,
    /// the font's own "missing" mark, for a character it hasn't got.
    glyphs: Mutex<HashMap<char, (u16, f32)>>,
}

impl Face {
    /// `data` read as a face, if it reads as one.
    pub fn read(entry: FontEntry, data: Vec<u8>) -> Option<Face> {
        let font = FontRef::new(&data).ok()?;
        let metrics = font.metrics(Size::unscaled(), LocationRef::default());
        let em = f32::from(metrics.units_per_em.max(1));
        let underline = metrics.underline.map_or((-0.1 * em, 0.05 * em), |u| (u.offset, u.thickness));
        Some(Face {
            entry,
            units_per_em: em,
            ascent: metrics.ascent / em,
            descent: metrics.descent / em,
            line_gap: metrics.leading / em,
            underline_offset: underline.0 / em,
            underline_thickness: (underline.1 / em).max(0.02),
            data: Arc::new(data),
            glyphs: Mutex::new(HashMap::new()),
        })
    }

    /// The glyph character `ch` is drawn with, and its advance in ems.
    pub fn glyph(&self, ch: char) -> (u16, f32) {
        if let Some(&found) = self.glyphs.lock().ok().as_ref().and_then(|g| g.get(&ch)) {
            return found;
        }
        let found = FontRef::new(&self.data)
            .ok()
            .map(|font| {
                let glyph = font.charmap().map(ch).unwrap_or_default();
                let advance = font.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(glyph).unwrap_or(0.0);
                (glyph.to_u32() as u16, advance / self.units_per_em)
            })
            .unwrap_or((0, 0.5));
        if let Ok(mut glyphs) = self.glyphs.lock() {
            glyphs.insert(ch, found);
        }
        found
    }

    /// How wide `text` is at `size` points, in points.
    pub fn width(&self, text: &str, size: f32) -> f32 {
        text.chars().map(|ch| self.glyph(ch).1).sum::<f32>() * size
    }
}

/// Every font found, and the faces read so far.
pub struct Catalogue {
    entries: Vec<FontEntry>,
    read: Mutex<HashMap<PathBuf, Option<Arc<Face>>>>,
}

/// The fonts installed here, found the first time they're asked for.
pub fn catalogue() -> &'static Catalogue {
    static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();
    CATALOGUE.get_or_init(|| Catalogue::scan(&font_folders()))
}

/// Where fonts are installed: for everyone, and for this user alone.
fn font_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(windows) = std::env::var_os("WINDIR") {
        folders.push(Path::new(&windows).join("Fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        folders.push(Path::new(&local).join("Microsoft").join("Windows").join("Fonts"));
    }
    folders
}

impl Catalogue {
    /// The fonts in `folders` that may be embedded in a PDF. Font collections
    /// (`.ttc`) are passed over: a PDF can't embed one as it is.
    pub fn scan(folders: &[PathBuf]) -> Catalogue {
        let mut entries = Vec::new();
        for folder in folders {
            let Ok(files) = std::fs::read_dir(folder) else { continue };
            for file in files.flatten() {
                let path = file.path();
                let font = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"));
                if font {
                    entries.extend(describe(&path));
                }
            }
        }
        entries.sort_by(|a, b| a.family.to_lowercase().cmp(&b.family.to_lowercase()).then(a.bold.cmp(&b.bold)).then(a.italic.cmp(&b.italic)));
        entries.dedup_by(|a, b| a.family == b.family && a.bold == b.bold && a.italic == b.italic);
        Catalogue { entries, read: Mutex::new(HashMap::new()) }
    }

    /// Every family there is, in order.
    pub fn families(&self) -> Vec<String> {
        let mut families: Vec<String> = self.entries.iter().map(|e| e.family.clone()).collect();
        families.dedup();
        families
    }

    pub fn has(&self, family: &str) -> bool {
        self.entries.iter().any(|e| e.family.eq_ignore_ascii_case(family))
    }

    /// The face of `family` nearest to `bold` and `italic`: the one asked
    /// for, else the family's regular, else any of it. A family not
    /// installed here gives Arial's, or failing that the first font there
    /// is, so text always has something to be set in.
    pub fn face(&self, family: &str, bold: bool, italic: bool) -> Option<Arc<Face>> {
        let pick = |family: &str| -> Option<FontEntry> {
            let of = || self.entries.iter().filter(|e| e.family.eq_ignore_ascii_case(family));
            of().find(|e| e.bold == bold && e.italic == italic)
                .or_else(|| of().find(|e| e.bold == bold))
                .or_else(|| of().find(|e| !e.bold && !e.italic))
                .or_else(|| of().next())
                .cloned()
        };
        let entry = pick(family).or_else(|| pick("Arial")).or_else(|| self.entries.first().cloned())?;
        self.read(entry)
    }

    /// `entry` read whole, once.
    fn read(&self, entry: FontEntry) -> Option<Arc<Face>> {
        let mut read = self.read.lock().ok()?;
        read.entry(entry.path.clone())
            .or_insert_with(|| std::fs::read(&entry.path).ok().and_then(|data| Face::read(entry, data)).map(Arc::new))
            .clone()
    }
}

/// A font file's family, style and PostScript name, read from its tables,
/// if it's a single font that may be embedded.
fn describe(path: &Path) -> Option<FontEntry> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header).ok()?;
    let version = u32::from_be_bytes(header[0..4].try_into().ok()?);
    let cff = version == u32::from_be_bytes(*b"OTTO");
    if version != 0x0001_0000 && !cff && version != u32::from_be_bytes(*b"true") {
        return None;
    }
    let count = usize::from(u16::from_be_bytes([header[4], header[5]]));
    let mut directory = vec![0u8; count * 16];
    file.read_exact(&mut directory).ok()?;
    let mut table = |tag: &[u8; 4]| -> Option<Vec<u8>> {
        let record = directory.chunks_exact(16).find(|r| &r[0..4] == tag)?;
        let offset = u32::from_be_bytes(record[8..12].try_into().ok()?);
        let length = u32::from_be_bytes(record[12..16].try_into().ok()?).min(1 << 20);
        file.seek(SeekFrom::Start(u64::from(offset))).ok()?;
        let mut bytes = vec![0u8; length as usize];
        file.read_exact(&mut bytes).ok()?;
        Some(bytes)
    };
    let os2 = table(b"OS/2")?;
    let names = table(b"name")?;
    let u16_at = |bytes: &[u8], at: usize| bytes.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    // Licensed for viewing only, or bitmaps only: not to be embedded.
    let embedding = u16_at(&os2, 8)?;
    if embedding & 0x0002 != 0 || embedding & 0x0200 != 0 {
        return None;
    }
    let selection = u16_at(&os2, 62).unwrap_or(0);
    let name = |id: u16| name_record(&names, id);
    Some(FontEntry {
        family: name(1)?,
        bold: selection & 0x20 != 0,
        italic: selection & 0x01 != 0,
        postscript: name(6).unwrap_or_else(|| name(1).unwrap_or_default().replace(' ', "")),
        path: path.to_path_buf(),
        cff,
    })
}

/// Name `id` from a `name` table: the Windows English one, else any Windows
/// one, else a Mac Roman one.
fn name_record(table: &[u8], id: u16) -> Option<String> {
    let u16_at = |at: usize| table.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let (count, strings) = (usize::from(u16_at(2)?), usize::from(u16_at(4)?));
    let mut best: Option<(u8, String)> = None;
    for record in 0..count {
        let at = 6 + record * 12;
        let (platform, encoding, language, name_id) = (u16_at(at)?, u16_at(at + 2)?, u16_at(at + 4)?, u16_at(at + 6)?);
        if name_id != id {
            continue;
        }
        let (length, offset) = (usize::from(u16_at(at + 8)?), usize::from(u16_at(at + 10)?));
        let Some(bytes) = table.get(strings + offset..strings + offset + length) else { continue };
        let (rank, text) = match (platform, encoding) {
            (3, 0 | 1) => {
                let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                (if language == 0x0409 { 0 } else { 1 }, String::from_utf16_lossy(&units))
            }
            (1, 0) => (2, bytes.iter().map(|&b| b as char).collect()),
            _ => continue,
        };
        if best.as_ref().is_none_or(|(have, _)| rank < *have) {
            best = Some((rank, text));
        }
    }
    best.map(|(_, text)| text).filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_s_fonts_are_found_with_their_styles() {
        let fonts = catalogue();
        // Every Windows machine has Arial, in all four styles.
        if !fonts.has("Arial") {
            return;
        }
        let regular = fonts.face("Arial", false, false).expect("Arial reads");
        let bold = fonts.face("Arial", true, false).expect("so does its bold");
        assert!(!regular.entry.bold && bold.entry.bold);
        assert_eq!(regular.entry.postscript, "ArialMT");
        assert!(regular.ascent > 0.5 && regular.descent < 0.0);
        let (glyph, advance) = regular.glyph('M');
        assert!(glyph != 0 && advance > 0.5 && advance < 1.0, "{glyph} {advance}");
        assert!(bold.width("Mm", 10.0) > regular.width("Mm", 10.0), "bold is wider");
        assert_eq!(fonts.face("No Such Font", false, false).map(|f| f.entry.family.clone()), Some("Arial".to_owned()), "a missing family falls back");
    }
}
