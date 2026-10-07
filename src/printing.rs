//! Shared print planning, used by the preview and the Windows spooler.
use pdfium_render::prelude::*;
use std::sync::{atomic::AtomicBool, Arc};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{configure, load_printer, printers, properties, spool};

pub const PAPER_SIZES: [(&str, [f32; 2]); 11] = [
    ("A0", [841., 1189.]),
    ("A1", [594., 841.]),
    ("A2", [420., 594.]),
    ("A3", [297., 420.]),
    ("A4", [210., 297.]),
    ("A5", [148., 210.]),
    ("Letter", [215.9, 279.4]),
    ("Legal", [215.9, 355.6]),
    ("Tabloid", [279.4, 431.8]),
    ("ARCH D", [609.6, 914.4]),
    ("ARCH E", [914.4, 1219.2]),
];
#[derive(Clone, Debug, PartialEq)]
pub struct Paper {
    pub id: i16,
    pub name: String,
    pub mm: [f32; 2],
}
#[derive(Clone, Debug)]
pub struct Printer {
    pub name: String,
    pub mode: Vec<u32>,
    pub papers: Vec<Paper>,
    pub default_paper: i16,
    pub landscape: bool,
    pub colour: bool,
    pub duplex: bool,
    pub default_colour: Colour,
    pub default_duplex: Duplex,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Auto,
    Portrait,
    Landscape,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scaling {
    Fit,
    Shrink,
    Actual,
    Custom,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Colour {
    Colour,
    Grayscale,
    BlackWhite,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Duplex {
    Single,
    LongEdge,
    ShortEdge,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageFilter {
    All,
    Odd,
    Even,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub paper: i16,
    pub match_paper: bool,
    pub orientation: Orientation,
    pub scaling: Scaling,
    pub percent: f32,
    pub auto_rotate: bool,
    pub center: bool,
    pub colour: Colour,
    pub include_markups: bool,
    pub copies: u16,
    pub collate: bool,
    pub duplex: Duplex,
    pub reverse: bool,
    pub pages_per_sheet: usize,
    pub as_image: bool,
    pub dpi: u16,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            paper: 9,
            match_paper: false,
            orientation: Orientation::Auto,
            scaling: Scaling::Fit,
            percent: 100.,
            auto_rotate: true,
            center: true,
            colour: Colour::Colour,
            include_markups: true,
            copies: 1,
            collate: true,
            duplex: Duplex::Single,
            reverse: false,
            pages_per_sheet: 1,
            as_image: false,
            dpi: 300,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Metrics {
    pub paper: [f32; 2],
    pub printable: [f32; 4],
    pub dpi: [i32; 2],
}
#[derive(Clone, Debug)]
pub struct Configured {
    pub printer: Printer,
    pub metrics: Metrics,
    pub options: Options,
}
#[derive(Clone, Debug)]
pub struct Placement {
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub rotate: bool,
    pub scale: f32,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: u64,
    pub changes: crate::domain::Changes,
    pub arrangement: Vec<crate::arrange::Sheet>,
    pub new_pages: Vec<[f32; 2]>,
}
#[derive(Clone)]
pub struct Job {
    pub printer: Printer,
    pub options: Options,
    pub pages: Vec<usize>,
    pub cancelled: Arc<AtomicBool>,
    pub output: Option<std::path::PathBuf>,
}

pub fn page_range(input: &str, count: usize) -> Result<Vec<usize>, String> {
    let mut pages = Vec::new();
    if input.trim().is_empty() {
        return Err("Enter pages, for example 1-3, 7, 10-12".into());
    }
    let number = |s: &str| -> Result<usize, String> {
        let n = s.trim().parse::<usize>().map_err(|_| format!("Invalid page number: {}", s.trim()))?;
        if n == 0 || n > count {
            return Err(format!("Pages must be between 1 and {count}"));
        }
        Ok(n - 1)
    };
    for part in input.split(',') {
        let span: Vec<_> = part.trim().split('-').collect();
        let (a, b) = match span.as_slice() {
            [one] => {
                let n = number(one)?;
                (n, n)
            }
            [a, b] => (number(a)?, number(b)?),
            _ => return Err("Use commas between pages and a dash for ranges".into()),
        };
        if a > b {
            return Err("A page range must run from the lower number to the higher number".into());
        }
        for page in a..=b {
            if !pages.contains(&page) {
                pages.push(page);
            }
        }
    }
    Ok(pages)
}
pub fn filter_pages(mut pages: Vec<usize>, filter: PageFilter, reverse: bool) -> Vec<usize> {
    pages.retain(|p| match filter {
        PageFilter::All => true,
        PageFilter::Odd => p % 2 == 0,
        PageFilter::Even => p % 2 == 1,
    });
    if reverse {
        pages.reverse();
    }
    pages
}
pub fn grid(n: usize) -> (usize, usize) {
    match n {
        2 => (2, 1),
        4 => (2, 2),
        6 => (3, 2),
        9 => (3, 3),
        16 => (4, 4),
        _ => (1, 1),
    }
}
/// A blank back separates odd-length duplex copies.
pub fn copy_order(sheets: usize, options: &Options) -> Vec<Option<usize>> {
    if options.collate || options.duplex != Duplex::Single {
        (0..options.copies)
            .flat_map(|copy| {
                let mut sides: Vec<_> = (0..sheets).map(Some).collect();
                if options.duplex != Duplex::Single && sheets % 2 == 1 && copy + 1 < options.copies {
                    sides.push(None);
                }
                sides
            })
            .collect()
    } else {
        (0..sheets).flat_map(|sheet| std::iter::repeat_n(Some(sheet), options.copies as usize)).collect()
    }
}
pub fn placements(sizes: &[[f32; 2]], metrics: &Metrics, options: &Options) -> Vec<Placement> {
    let (cols, rows) = grid(options.pages_per_sheet);
    let [px, py, pw, ph] = metrics.printable;
    let gap = if options.pages_per_sheet > 1 { 6. } else { 0. };
    let cw = (pw - gap * (cols - 1) as f32) / cols as f32;
    let ch = (ph - gap * (rows - 1) as f32) / rows as f32;
    sizes
        .iter()
        .enumerate()
        .map(|(i, size)| {
            let clip = [px + (i % cols) as f32 * (cw + gap), py + (i / cols) as f32 * (ch + gap), cw, ch];
            let normal = (cw / size[0]).min(ch / size[1]);
            let turned = (cw / size[1]).min(ch / size[0]);
            let rotate = options.auto_rotate && turned > normal + 0.001;
            let [w, h] = if rotate { [size[1], size[0]] } else { *size };
            let fit = (cw / w).min(ch / h);
            let scale = if options.pages_per_sheet > 1 {
                fit
            } else {
                match options.scaling {
                    Scaling::Fit => fit,
                    Scaling::Shrink => fit.min(1.),
                    Scaling::Actual => 1.,
                    Scaling::Custom => options.percent / 100.,
                }
            };
            let (w, h) = (w * scale, h * scale);
            let x = clip[0] + if options.center { (cw - w) / 2. } else { 0. };
            let y = clip[1] + if options.center { (ch - h) / 2. } else { 0. };
            Placement { rect: [x, y, w, h], clip, rotate, scale }
        })
        .collect()
}
pub fn paper_for(printer: &Printer, options: &Options, source: [f32; 2]) -> Result<(i16, bool), String> {
    if source.iter().any(|v| !v.is_finite() || *v <= 0.) {
        return Err("The source page has invalid dimensions".into());
    }
    if options.match_paper && options.pages_per_sheet > 1 {
        return Err("Choose a paper size when printing multiple pages per sheet".into());
    }
    let id = if options.match_paper {
        let mut wanted = source.map(|pt| pt * 25.4 / 72.);
        wanted.sort_by(f32::total_cmp);
        printer
            .papers
            .iter()
            .find(|p| {
                let mut mm = p.mm;
                mm.sort_by(f32::total_cmp);
                (mm[0] - wanted[0]).abs() < 2. && (mm[1] - wanted[1]).abs() < 2.
            })
            .map(|p| p.id)
            .ok_or_else(|| {
                format!(
                    "{} has no paper matching {:.0} × {:.0} mm. Choose a supported paper size.",
                    printer.name, wanted[0], wanted[1]
                )
            })?
    } else {
        options.paper
    };
    if !printer.papers.iter().any(|p| p.id == id) {
        return Err("Choose a paper size supported by this printer".into());
    }
    let landscape = match options.orientation {
        Orientation::Portrait => false,
        Orientation::Landscape => true,
        Orientation::Auto => {
            if options.pages_per_sheet > 1 {
                matches!(options.pages_per_sheet, 2 | 6)
            } else {
                source[0] > source[1]
            }
        }
    };
    Ok((id, landscape))
}
/// Removes annotation markups while retaining interactive forms and links.
/// Flattened content belongs to the drawing and cannot be separated here.
pub fn without_markups(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use pdf_content::lopdf::{Document, Object};
    let mut doc = Document::load_mem(bytes).map_err(|e| e.to_string())?;
    for page in doc.get_pages().values() {
        let annotations = doc.get_dictionary(*page).ok().and_then(|d| d.get(b"Annots").ok()).cloned();
        let annotations = match annotations {
            Some(Object::Reference(id)) => doc.get_object(id).ok().cloned(),
            other => other,
        };
        if let Some(Object::Array(items)) = annotations {
            let keep: Vec<_> = items
                .into_iter()
                .filter(|obj| {
                    let dict = match obj {
                        Object::Reference(id) => doc.get_dictionary(*id).ok(),
                        Object::Dictionary(d) => Some(d),
                        _ => None,
                    };
                    dict.and_then(|d| d.get(b"Subtype").ok())
                        .and_then(|o| o.as_name().ok())
                        .is_some_and(|name| name == b"Widget" || name == b"Link")
                })
                .collect();
            doc.get_dictionary_mut(*page).map_err(|e| e.to_string())?.set("Annots", keep);
        }
    }
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}
pub fn render_page(page: &PdfPage<'_>, size: [i32; 2], rotate: bool, colour: Colour) -> Result<Vec<u8>, String> {
    let mut config = PdfRenderConfig::new()
        .set_fixed_size(size[0], size[1])
        .set_format(PdfBitmapFormat::BGRA)
        .set_clear_color(PdfColor::WHITE)
        .use_print_quality(true)
        .use_grayscale_rendering(colour != Colour::Colour);
    if rotate {
        config = config.rotate(PdfPageRenderRotation::Degrees90, false);
    }
    let bitmap = page.render_with_config(&config).map_err(|e| e.to_string())?;
    let mut pixels = bitmap.as_rgba_bytes();
    if colour == Colour::BlackWhite {
        black_white(&mut pixels);
    }
    Ok(pixels)
}
pub fn black_white(pixels: &mut [u8]) {
    for p in pixels.chunks_exact_mut(4) {
        let gray = (u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000;
        let v = if gray >= 180 { 255 } else { 0 };
        p[0] = v;
        p[1] = v;
        p[2] = v;
    }
}
/// Indexed monochrome/grayscale bands reduce the raster data sent to drivers.
/// Windows DIB scanlines are padded to a four-byte boundary.
pub fn raster_data(pixels: &[u8], width: usize, height: usize, colour: Colour) -> (u16, Vec<u8>) {
    if colour == Colour::Colour {
        return (32, pixels.to_vec());
    }
    let bits = if colour == Colour::BlackWhite { 1 } else { 8 };
    let stride = (width * bits).div_ceil(32) * 4;
    let mut out = vec![0u8; stride * height];
    for y in 0..height {
        for x in 0..width {
            let gray = pixels[(y * width + x) * 4];
            if bits == 8 {
                out[y * stride + x] = gray;
            } else if gray >= 180 {
                out[y * stride + x / 8] |= 1 << (7 - x % 8);
            }
        }
    }
    (bits as u16, out)
}
/// Same placement and clipping as the spooler, at screen resolution.
pub fn preview(
    doc: &PdfDocument<'_>,
    pages: &[usize],
    configured: &Configured,
) -> Result<(eframe::egui::ColorImage, Vec<Placement>), String> {
    let sizes: Vec<_> = pages
        .iter()
        .map(|&p| doc.pages().get(p as i32).map(|p| [p.width().value, p.height().value]).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let positions = placements(&sizes, &configured.metrics, &configured.options);
    let paper = configured.metrics.paper;
    let scale = (680. / paper[0]).min(850. / paper[1]);
    let width = (paper[0] * scale).round().max(1.) as usize;
    let height = (paper[1] * scale).round().max(1.) as usize;
    let mut pixels = vec![255u8; width * height * 4];
    for (&index, pos) in pages.iter().zip(&positions) {
        let page = doc.pages().get(index as i32).map_err(|e| e.to_string())?;
        let [x, y, w, h] = pos.rect.map(|v| (v * scale).round() as i32);
        let w = w.max(1);
        let h = h.max(1);
        // Very large custom scales are clipped before rendering in the spooler;
        // prevent an accidental enormous preview allocation as well.
        let reduction = (4096. / w.max(h) as f32).min(1.);
        let bw = (w as f32 * reduction).round().max(1.) as i32;
        let bh = (h as f32 * reduction).round().max(1.) as i32;
        let image = render_page(&page, [bw, bh], pos.rotate, configured.options.colour)?;
        let [cx, cy, cw, ch] = pos.clip.map(|v| (v * scale).round() as i32);
        for row in y.max(cy).max(0)..(y + h).min(cy + ch).min(height as i32) {
            for col in x.max(cx).max(0)..(x + w).min(cx + cw).min(width as i32) {
                let sx = ((col - x) as f32 * reduction) as usize;
                let sy = ((row - y) as f32 * reduction) as usize;
                let src = (sy.min(bh as usize - 1) * bw as usize + sx.min(bw as usize - 1)) * 4;
                let dst = (row as usize * width + col as usize) * 4;
                pixels[dst..dst + 4].copy_from_slice(&image[src..src + 4]);
            }
        }
    }
    Ok((eframe::egui::ColorImage::from_rgba_unmultiplied([width, height], &pixels), positions))
}
#[cfg(not(windows))]
pub fn printers() -> Result<(Vec<String>, String), String> {
    Err("Printing requires Windows".into())
}
#[cfg(not(windows))]
pub fn load_printer(_: &str) -> Result<Printer, String> {
    Err("Printing requires Windows".into())
}
#[cfg(not(windows))]
pub fn properties(_: &Printer) -> Result<Option<Printer>, String> {
    Err("Printing requires Windows".into())
}
#[cfg(not(windows))]
pub fn configure(_: &Printer, _: &Options, _: [f32; 2]) -> Result<Configured, String> {
    Err("Printing requires Windows".into())
}
#[cfg(not(windows))]
pub fn spool(_: &Pdfium, _: &[u8], _: &Job, _: impl Fn(usize, usize)) -> Result<bool, String> {
    Err("Printing requires Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_are_validated_and_deduplicated() {
        assert_eq!(page_range("1-3, 2, 7", 7).unwrap(), vec![0, 1, 2, 6]);
        for bad in ["", "0", "8", "3-1", "1-2-3", "2,", "x"] {
            assert!(page_range(bad, 7).is_err(), "{bad}");
        }
        assert_eq!(filter_pages(vec![0, 1, 2, 3, 4], PageFilter::Odd, true), vec![4, 2, 0]);
    }
    #[test]
    fn scaling_and_nup_preserve_paper_geometry() {
        let metrics = Metrics { paper: [600., 800.], printable: [10., 20., 580., 760.], dpi: [300, 600] };
        let mut options = Options::default();
        options.auto_rotate = false;
        options.scaling = Scaling::Actual;
        assert_eq!(placements(&[[720., 360.]], &metrics, &options)[0].rect, [-60., 220., 720., 360.]);
        options.pages_per_sheet = 2;
        let positions = placements(&[[720., 360.], [720., 360.]], &metrics, &options);
        assert_eq!(positions.len(), 2);
        assert!(positions[0].rect[0] + positions[0].rect[2] < positions[1].rect[0]);
    }
    #[test]
    fn black_white_has_no_grey_pixels() {
        let mut pixels = vec![255, 0, 0, 255, 250, 250, 250, 255];
        black_white(&mut pixels);
        assert_eq!(pixels, vec![0, 0, 0, 255, 255, 255, 255, 255]);
    }
    #[test]
    fn duplex_copies_begin_on_the_front_of_a_new_sheet() {
        let options = Options { copies: 2, duplex: Duplex::LongEdge, ..Options::default() };
        assert_eq!(copy_order(3, &options), vec![Some(0), Some(1), Some(2), None, Some(0), Some(1), Some(2)]);
        let options = Options { copies: 2, collate: false, ..Options::default() };
        assert_eq!(copy_order(2, &options), vec![Some(0), Some(0), Some(1), Some(1)]);
    }
    #[test]
    fn monochrome_bands_are_bit_packed_and_scanlines_are_padded() {
        let pixels = vec![255, 255, 255, 255, 0, 0, 0, 255, 200, 200, 200, 255];
        let (bits, mono) = raster_data(&pixels, 3, 1, Colour::BlackWhite);
        assert_eq!(bits, 1);
        assert_eq!(mono, vec![0b10100000, 0, 0, 0]);
        let (bits, grey) = raster_data(&pixels, 3, 1, Colour::Grayscale);
        assert_eq!(bits, 8);
        assert_eq!(grey, vec![255, 0, 200, 0]);
    }
}
