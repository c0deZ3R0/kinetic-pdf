// No console window behind the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Kinetic PDF -- a minimal, fast PDF reader whose one trick is highlighting
//! text and attaching a note to it.
//!
//! pdfium renders pages and supplies each page's characters on a background
//! thread (worker.rs); egui draws them and handles selection (app/). The
//! highlights are real /Highlight annotations in the file (annots.rs), so they
//! open in any viewer, and highlights made elsewhere show up here.

use eframe::egui;
use kinetic_pdf::app;

// On a laptop with integrated and discrete graphics, NVIDIA's and AMD's
// drivers give an exe exporting these the discrete GPU, where otherwise Windows
// may pick the integrated one. build.rs exports them.
#[cfg(windows)]
#[allow(non_upper_case_globals)]
#[no_mangle]
#[used]
pub static NvOptimusEnablement: u32 = 1;
#[cfg(windows)]
#[allow(non_upper_case_globals)]
#[no_mangle]
#[used]
pub static AmdPowerXpressRequestHighPerformance: i32 = 1;

fn main() -> eframe::Result {
    // The app starts this exe again to draw pages in parallel; see helper.rs.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == kinetic_pdf::helper::FLAG) {
        kinetic_pdf::helper::run();
        return Ok(());
    }
    // ...and to make a copy of a file to draw from; see merge.rs.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == kinetic_pdf::merge::FLAG) {
        std::process::exit(kinetic_pdf::merge::run_copy(std::env::args_os().skip(2)));
    }

    // `kinetic-pdf.exe some.pdf` opens that file, which is also what Windows
    // does when the exe is used through "Open with".
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);

    // The exe's own icon comes from assets/icon.ico via build.rs; the window
    // and taskbar need theirs set at runtime. Raw RGBA, so there's nothing to
    // decode. Regenerate both with assets/make-icon.ps1.
    let icon = egui::IconData {
        rgba: include_bytes!("../assets/icon-128.rgba").to_vec(),
        width: 128,
        height: 128,
    };

    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title("Kinetic PDF")
            .with_inner_size([1400.0, 900.0])
            .with_drag_and_drop(true)
            .with_icon(icon),
        ..Default::default()
    };
    // Annotations drawn on the GPU (app/gpu.rs) smooth the edges of their
    // filled shapes by multisampling, and some clips go through the stencil.
    options.multisampling = 4;
    options.stencil_buffer = 8;
    // On, as people use it. The benchmark can turn it off to see what the
    // frame rate was hiding, and reports which it measured.
    options.glow_options.vsync = std::env::var_os("KINETIC_PDF_VSYNC").is_none_or(|v| v != "0");

    eframe::run_native(
        "Kinetic PDF",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)))),
    )
}
