// No console window behind the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! PDF Annotate, native edition -- a minimal PDF reader whose one trick is
//! highlighting text and attaching a note to it.
//!
//! pdfium renders pages and supplies each page's characters on a background
//! thread (worker.rs); egui draws them and handles selection (app/). The
//! highlights are real /Highlight annotations in the file (annots.rs), so they
//! open in any viewer, and highlights made elsewhere show up here.

use eframe::egui;
use pdf_annotate::app;

fn main() -> eframe::Result {
    // The app starts this exe again to draw pages in parallel; see helper.rs.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == pdf_annotate::helper::FLAG) {
        pdf_annotate::helper::run();
        return Ok(());
    }
    // ...and to make a copy of a file to draw from; see merge.rs.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == pdf_annotate::merge::FLAG) {
        std::process::exit(pdf_annotate::merge::run_copy(std::env::args_os().skip(2)));
    }

    // `pdf-annotate.exe some.pdf` opens that file, which is also what Windows
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

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title("PDF Annotate")
            .with_inner_size([1400.0, 900.0])
            .with_drag_and_drop(true)
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "PDF Annotate",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)))),
    )
}
