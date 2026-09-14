//! The benchmark, with `GPU_LINES_BENCH=1`: sweeps the zoom as soon as the
//! shapes are on the GPU, prints the frame and draw times to stderr after
//! `BENCH_FOR`, takes `SHOTS`, and closes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::vec2;

use crate::{Mode, Viewer};

const BENCH_FOR: Duration = Duration::from_secs(10);

/// Screenshots taken after the sweep, centred on `GPU_LINES_BENCH_AT` ("x,y"
/// in page points, the page's middle if unset), at a zoom relative to fit
/// width, into `GPU_LINES_BENCH_DIR` (the current folder if unset).
const SHOTS: [(Mode, f32, &str); 4] = [
    (Mode::GpuOverPage, 1.0, "gpu-fit"),
    (Mode::PdfiumSharp, 1.0, "pdfium-fit"),
    (Mode::GpuOverPage, 6.0, "gpu-600"),
    (Mode::PdfiumSharp, 6.0, "pdfium-600"),
];

fn save_png(path: &Path, size: [usize; 2], rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size[0] as u32, size[1] as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut w| w.write_image_data(rgba)).map_err(|e| e.to_string())
}

/// Median, 95th percentile and worst of a list of times in milliseconds.
fn summary(times: &[f32]) -> String {
    let mut sorted = times.to_vec();
    sorted.sort_by(f32::total_cmp);
    let pick = |q: f32| sorted.get(((sorted.len() as f32 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
    format!("median {:.2} ms, 95th percentile {:.2} ms, worst {:.2} ms over {} frames", pick(0.5), pick(0.95), pick(1.0), sorted.len())
}

impl Viewer {
    /// Starts sweeping once the shapes are on the GPU, and after `BENCH_FOR`
    /// prints what it measured and starts the screenshots.
    pub(crate) fn run_bench(&mut self, ctx: &egui::Context) {
        if !self.bench {
            return;
        }
        let failed = self.error.clone().or_else(|| match &*self.renderer.lock().unwrap() {
            Some(Err(e)) => Some(e.clone()),
            _ => None,
        });
        if let Some(error) = failed {
            eprintln!("bench failed: {error}");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.bench = false;
            return;
        }
        let uploaded = self.pending.lock().unwrap().is_none() && matches!(&*self.renderer.lock().unwrap(), Some(Ok(_)));
        match self.sweep {
            None if uploaded && self.shape_count > 0 => {
                self.mode = Mode::GpuOverPage;
                self.sweep = Some(Instant::now());
                self.frame_times.clear();
                self.draw_times.clear();
            }
            Some(started) if started.elapsed() >= BENCH_FOR => {
                for line in &self.report {
                    eprintln!("{line}");
                }
                eprintln!("{}", self.status(ctx.pixels_per_point()).split("; frames").next().unwrap_or_default());
                eprintln!(
                    "sweeping zoom between 141% and 1131% of fit width, vsync {}",
                    if std::env::var_os("GPU_LINES_VSYNC").is_some_and(|v| v == "0") { "off" } else { "on" }
                );
                eprintln!("frame to frame: {}", summary(&self.frame_times));
                eprintln!("GPU draw, waited for: {}", summary(&self.draw_times));
                self.sweep = None;
                self.shot = Some((0, self.frames + 1, false));
            }
            _ => {}
        }
    }

    /// Takes the screenshots one at a time: sets up the view, waits until it's
    /// drawn (for pdfium, the sharp drawing of exactly it), asks for a
    /// screenshot and saves it when it arrives. Closes after the last.
    pub(crate) fn take_shots(&mut self, ctx: &egui::Context) {
        let Some((index, set_at, asked)) = self.shot else { return };
        let Some(&(mode, factor, name)) = SHOTS.get(index) else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.shot = None;
            self.bench = false;
            return;
        };
        if self.frames == set_at {
            let at = std::env::var("GPU_LINES_BENCH_AT")
                .ok()
                .and_then(|s| s.split_once(',').and_then(|(x, y)| Some([x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?])))
                .unwrap_or([self.page_size[0] / 2.0, self.page_size[1] / 2.0]);
            self.mode = mode;
            self.zoom = self.fit * factor;
            self.offset = self.view_size / 2.0 - vec2(at[0], self.page_size[1] - at[1]) * self.zoom;
            return;
        }
        let ppp = ctx.pixels_per_point();
        if mode.wants_sharp() && !asked {
            self.ask_for_sharp(ppp, true);
        }
        let drawn = self.frames >= set_at + 3 && (!mode.wants_sharp() || self.sharp_is_current(ppp));
        if !asked && drawn {
            let pdfium = match (mode.wants_sharp(), self.sharp_took, self.sharp_asked) {
                (true, Some(took), Some((request, _))) => format!(
                    "; pdfium drew the {} x {} px view of the page at {} x {} px in {:.0} ms",
                    request.region[2],
                    request.region[3],
                    request.full[0],
                    request.full[1],
                    took.as_secs_f64() * 1000.0
                ),
                _ => String::new(),
            };
            eprintln!("{name}: zoom {:.0}% of fit width{pdfium}", self.zoom / self.fit.max(1e-6) * 100.0);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.shot = Some((index, set_at, true));
            return;
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(Arc::clone(image)),
                _ => None,
            })
        });
        if let Some(image) = image {
            let dir = std::env::var_os("GPU_LINES_BENCH_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            let path = dir.join(format!("{name}.png"));
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            match save_png(&path, image.size, &rgba) {
                Ok(()) => eprintln!("saved {}", path.display()),
                Err(e) => eprintln!("couldn't save {}: {e}", path.display()),
            }
            self.shot = Some((index + 1, self.frames + 1, false));
        }
    }
}
