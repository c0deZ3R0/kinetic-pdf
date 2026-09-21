//! Overlaying two or three pages, each drawn in a colour of its own.
//!
//! The way every other program does this is to build a new PDF: each source
//! becomes a layer, tinted, and the result is a document you then have to
//! open. Bluebeam's Overlay Pages works that way, which is why it needs a
//! dialogue asking what to do with the layer captions when you flatten it.
//!
//! None of that is necessary. An overlay is a way of *drawing* pages, not a
//! document: paint each one in a single colour, multiplied into what's
//! already there, and coincident linework goes darker than either page while
//! anything only one page draws keeps that page's colour. It is the same draw
//! the viewer already does, once per page, so it costs nothing more at any
//! zoom and there is nothing to save, open or flatten.
//!
//! This example does it offscreen and writes a PNG, so the result can be
//! looked at without a viewer. `Renderer::paint_tinted` is the whole of it;
//! on screen the same calls go straight into the viewport.
//!
//! ```text
//! cargo run --release -p gpu-lines --example overlay -- a.pdf 1 2
//! cargo run --release -p gpu-lines --example overlay -- a.pdf 1 2 --out over.png --width 2400
//! cargo run --release -p gpu-lines --example overlay -- a.pdf 1 --and b.pdf 1
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpu_lines::{lopdf::Document, page_shapes, page_size, Renderer, Tint, MOST_IMAGE_DENSITY};

const TOLERANCE: f32 = 0.05;

struct Sheet {
    path: PathBuf,
    page: u32,
}

struct Args {
    sheets: Vec<Sheet>,
    out: PathBuf,
    width: u32,
    strength: f32,
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    // A window has to exist for there to be a GL context to draw with; it is
    // closed as soon as the overlay is drawn.
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([320.0, 120.0]).with_title("Overlaying..."),
        ..Default::default()
    };
    let run = eframe::run_native("overlay", options, Box::new(move |cc| Ok(Box::new(Overlay::new(cc, args)))));
    if let Err(e) = run {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

struct Overlay {
    args: Option<Args>,
    gl: Option<Arc<glow::Context>>,
    said: Option<String>,
}

impl Overlay {
    fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        Overlay { args: Some(args), gl: cc.gl.clone(), said: None }
    }
}

impl eframe::App for Overlay {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        if let (Some(args), Some(gl)) = (self.args.take(), self.gl.clone()) {
            self.said = Some(match draw(&gl, &args) {
                Ok(said) => said,
                Err(e) => format!("error: {e}"),
            });
            println!("{}", self.said.as_deref().unwrap_or_default());
            ui.ctx().send_viewport_cmd(eframe::egui::ViewportCommand::Close);
        }
        ui.label(self.said.as_deref().unwrap_or("Drawing..."));
    }
}

fn draw(gl: &glow::Context, args: &Args) -> Result<String, String> {
    let renderer = Renderer::new(gl)?;
    let mut report = String::new();

    // Read every sheet, and take the page size from the first: an overlay
    // puts them all in one page's coordinates, which is what the alignment
    // step in `examples/compare.rs` is for when they don't already agree.
    let mut uploaded = Vec::new();
    let mut points = [0.0f32; 2];
    for (n, sheet) in args.sheets.iter().enumerate() {
        let read = Instant::now();
        let doc = Document::load(&sheet.path).map_err(|e| format!("{}: {e}", sheet.path.display()))?;
        let shapes = page_shapes(&doc, sheet.page, TOLERANCE, MOST_IMAGE_DENSITY)?;
        let size = page_size(&doc, sheet.page)?;
        if n == 0 {
            points = size;
        }
        let count = shapes.primitives.len();
        let read_ms = read.elapsed().as_secs_f64() * 1e3;
        let tint = Tint { strength: args.strength, ..Tint::nth(n) };
        report += &format!(
            "  {} page {}: {count} primitives, {:.0} x {:.0} pt, in [{:.2}, {:.2}, {:.2}]   read in {read_ms:.1} ms\n",
            sheet.path.display(),
            sheet.page,
            size[0],
            size[1],
            tint.colour[0],
            tint.colour[1],
            tint.colour[2]
        );
        uploaded.push((renderer.upload(gl, shapes)?, tint));
    }

    let height = ((args.width as f32 * points[1] / points[0].max(f32::EPSILON)).round() as u32).max(1);
    let pages: Vec<(&gpu_lines::Uploaded, Tint)> = uploaded.iter().map(|(page, tint)| (page, *tint)).collect();

    let drew = Instant::now();
    let rgba = renderer.overlay_to_image(gl, &pages, [args.width, height], points).ok_or("the overlay couldn't be drawn")?;
    let draw_ms = drew.elapsed().as_secs_f64() * 1e3;

    write_png(&args.out, &rgba, args.width, height)?;
    let count = pages.len();
    drop(pages);
    for (page, _) in uploaded {
        page.destroy(gl);
    }
    renderer.destroy(gl);

    report += &format!("\nOverlaid {count} pages into {}x{} in {draw_ms:.1} ms\n", args.width, height);
    report += &format!("Written to {}\n", args.out.display());
    Ok(report)
}

/// The image, with its colours taken back out of their alpha, since the
/// renderer leaves them premultiplied and paper is opaque anyway.
fn write_png(path: &PathBuf, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { sheets: Vec::new(), out: PathBuf::from("overlay.png"), width: 1600, strength: 1.0 };
    let mut rest = std::env::args().skip(1).peekable();
    let mut path: Option<PathBuf> = None;
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--out" => args.out = PathBuf::from(rest.next().ok_or("--out wants a file")?),
            "--width" => args.width = rest.next().ok_or("--width wants a number")?.parse().map_err(|_| "--width wants a number")?,
            "--strength" => args.strength = rest.next().ok_or("--strength wants a number")?.parse().map_err(|_| "--strength wants a number")?,
            // Another file's page, rather than another page of this one.
            "--and" => path = Some(PathBuf::from(rest.next().ok_or("--and wants a file")?)),
            "-h" | "--help" => {
                println!("overlay A.pdf PAGE [PAGE...] [--and B.pdf PAGE] [--out FILE] [--width PX] [--strength 0-1]");
                std::process::exit(0);
            }
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other => match other.parse::<u32>() {
                Ok(page) => {
                    let of = path.clone().ok_or("give a PDF before its page numbers")?;
                    args.sheets.push(Sheet { path: of, page });
                }
                Err(_) => path = Some(PathBuf::from(other)),
            },
        }
    }
    if args.sheets.is_empty() {
        return Err("give a PDF and at least two page numbers; --help for the rest".into());
    }
    Ok(args)
}
