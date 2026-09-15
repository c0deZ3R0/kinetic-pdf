//! The loader thread: prepares the page, then keeps it open and has pdfium
//! draw the views the viewer asks for.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use gpu_lines::{annotation_shapes, lopdf, page_shapes, Shapes};
use kinetic_pdf::{annots, merge, worker};
use pdfium_render::prelude::*;

/// Pdfium's drawings of the whole page are this wide, for the background and
/// for comparing.
const PAGE_PIXELS: f32 = 4096.0;

/// An image as width and height in pixels, and its RGBA bytes.
pub type Image = ([usize; 2], Vec<u8>);

pub struct Loaded {
    pub page_size: [f32; 2],
    /// The page drawn without its annotations, and with them.
    pub background: Image,
    pub reference: Image,
    pub shapes: Shapes,
    pub report: Vec<String>,
}

/// A part of the page for pdfium to draw: the page drawn `full` pixels in
/// size, and the `region` of it (x, y, width, height).
#[derive(Clone, Copy, PartialEq)]
pub struct SharpRequest {
    pub id: u64,
    pub full: [u32; 2],
    pub region: [u32; 4],
}

pub enum FromLoader {
    Loaded(Result<Loaded, String>),
    Sharp { id: u64, image: Result<Image, String>, took: Duration },
}

/// Prepares the page -- all of it for the GPU with `whole_page`, or its
/// annotations -- then draws the views asked for, only ever the latest.
pub fn run(
    path: PathBuf,
    page_number: usize,
    whole_page: bool,
    tx: mpsc::Sender<FromLoader>,
    requests: mpsc::Receiver<SharpRequest>,
    ctx: egui::Context,
) {
    let send = |message: FromLoader| {
        let _ = tx.send(message);
        ctx.request_repaint();
    };
    let pdfium = match worker::bind() {
        Ok(pdfium) => pdfium,
        Err(e) => return send(FromLoader::Loaded(Err(e))),
    };
    let drawing = match prepare(&pdfium, &path, page_number, whole_page) {
        Ok((loaded, drawing)) => {
            send(FromLoader::Loaded(Ok(loaded)));
            drawing
        }
        Err(e) => return send(FromLoader::Loaded(Err(e))),
    };
    let Ok(doc) = pdfium.load_pdf_from_byte_slice(&drawing, None) else { return };
    let Ok(mut page) = doc.pages().get(index_of(page_number)) else { return };
    // As the app draws it: its own highlights drawn separately.
    annots::strip_loaded_page(&mut page);

    while let Ok(mut request) = requests.recv() {
        while let Ok(newer) = requests.try_recv() {
            request = newer;
        }
        let started = Instant::now();
        let image = annots::render_region_in_steps(&page, request.full, request.region, true, |_| true)
            .and_then(|drawn| drawn.ok_or_else(|| "the drawing was stopped".to_owned()));
        send(FromLoader::Sharp { id: request.id, image, took: started.elapsed() });
    }
}

fn index_of(page_number: usize) -> PdfPageIndex {
    page_number.saturating_sub(1) as PdfPageIndex
}

fn milliseconds(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Reads the page's annotations -- or with `whole_page`, everything it draws --
/// into shapes, makes the drawing copy, and has pdfium draw the page with its
/// annotations, and without them unless the GPU draws the whole page over
/// white. Returns the copy's bytes too, for drawing views from later.
fn prepare(pdfium: &Pdfium, path: &Path, page_number: usize, whole_page: bool) -> Result<(Loaded, Vec<u8>), String> {
    let mut report = Vec::new();
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;

    let started = Instant::now();
    let doc = lopdf::Document::load_mem(&bytes).map_err(|e| e.to_string())?;
    let (shapes, what) = match whole_page {
        true => (page_shapes(&doc, page_number as u32, 0.05)?, "the page's"),
        false => (annotation_shapes(&doc, page_number as u32, 0.05)?, "the annotations'"),
    };
    drop(doc);
    report.push(format!(
        "Read {what} drawing instructions in {:.0} ms: {} line pieces, {} triangles and {} images on {} atlas pages, within {} clip shapes in {} sets ({} tested in the shader), drawn in {} runs",
        milliseconds(started),
        shapes.lines,
        shapes.triangles,
        shapes.images,
        shapes.atlas.pages.len(),
        shapes.clips.shapes.len(),
        shapes.clips.sets.len(),
        (0..shapes.clips.sets.len()).filter(|&set| shapes.clips.is_convex(set)).count(),
        shapes.runs.len()
    ));
    if !shapes.not_drawn.is_empty() {
        let listed: Vec<String> = shapes.not_drawn.iter().map(|(what, n)| format!("{what} ({n})")).collect();
        report.push(format!("Not drawn on the GPU yet: {}", listed.join(", ")));
    }

    let started = Instant::now();
    let drawing = match merge::merge_document(&bytes, merge::MOST_PARTS)? {
        Some((copy, stats)) => {
            report.push(format!(
                "Drawing copy made in {:.0} ms: {} hidden annotations left out, {} of {} strokes merged",
                milliseconds(started),
                stats.hidden,
                stats.merged,
                stats.strokes
            ));
            copy
        }
        None => bytes,
    };

    let (page_size, reference, background) = {
        let doc = pdfium.load_pdf_from_byte_slice(&drawing, None).map_err(|e| e.to_string())?;
        let page = doc.pages().get(index_of(page_number)).map_err(|e| e.to_string())?;
        let page_size = [page.width().value, page.height().value];
        let scale = PAGE_PIXELS / page_size[0];
        let started = Instant::now();
        let reference = annots::render_loaded_page(&page, scale)?;
        report.push(format!("pdfium drew the whole page {} x {} px in {:.0} ms", reference.0[0], reference.0[1], milliseconds(started)));
        let background = if whole_page {
            ([1, 1], vec![255; 4])
        } else {
            let config = PdfRenderConfig::new().scale_page_by_factor(scale).render_annotations(false).render_form_data(false);
            let bitmap = page.render_with_config(&config).map_err(|e| e.to_string())?;
            ([bitmap.width() as usize, bitmap.height() as usize], bitmap.as_rgba_bytes())
        };
        (page_size, reference, background)
    };

    Ok((Loaded { page_size, background, reference, shapes, report }, drawing))
}
