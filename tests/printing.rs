//! Shared print planning and image output, plus an opt-in Windows driver check.
mod common;
use common::{build_pdf_rotated, Spec};
use kinetic_pdf::{printing::*, worker};
use std::sync::{atomic::AtomicBool, Arc};

fn fake_printer() -> Printer {
    Printer {
        name: "Preview".into(),
        mode: vec![],
        papers: vec![Paper { id: 9, name: "A4".into(), mm: [210., 297.] }],
        default_paper: 9,
        landscape: false,
        colour: true,
        duplex: true,
        default_colour: Colour::Colour,
        default_duplex: Duplex::Single,
    }
}

#[test]
fn preview_reflects_markups_colour_layout_and_clipping() {
    let pdfium = worker::bind().unwrap();
    let bytes = build_pdf_rotated(&[0, 90], &[Spec::plain(0, "markup")]);
    let stripped = without_markups(&bytes).unwrap();
    let original = pdf_content::lopdf::Document::load_mem(&bytes).unwrap();
    let clean = pdf_content::lopdf::Document::load_mem(&stripped).unwrap();
    let first = *original.get_pages().values().next().unwrap();
    assert_eq!(original.get_dictionary(first).unwrap().get(b"Annots").unwrap().as_array().unwrap().len(), 1);
    let first = *clean.get_pages().values().next().unwrap();
    assert!(clean.get_dictionary(first).unwrap().get(b"Annots").unwrap().as_array().unwrap().is_empty());
    let marked = pdfium.load_pdf_from_byte_vec(bytes, None).unwrap();
    let document = pdfium.load_pdf_from_byte_vec(stripped, None).unwrap();
    let mut config = Configured {
        printer: fake_printer(),
        metrics: Metrics { paper: [595., 842.], printable: [12., 12., 571., 818.], dpi: [300, 300] },
        options: Options::default(),
    };
    let (marked_image, _) = preview(&marked, &[0], &config).unwrap();
    let (clean_image, _) = preview(&document, &[0], &config).unwrap();
    assert_ne!(marked_image.pixels, clean_image.pixels, "hiding markups changes the preview");
    config.options.pages_per_sheet = 2;
    config.options.colour = Colour::Grayscale;
    let (grey, positions) = preview(&marked, &[0, 1], &config).unwrap();
    assert_eq!(positions.len(), 2);
    assert!(grey.pixels.iter().all(|p| p.r() == p.g() && p.g() == p.b()));
    config.options.colour = Colour::BlackWhite;
    let (mono, _) = preview(&marked, &[0, 1], &config).unwrap();
    assert!(mono.pixels.iter().all(|p| p.r() == p.g() && p.g() == p.b() && (p.r() == 0 || p.r() == 255)));
    config.options.pages_per_sheet = 1;
    config.options.scaling = Scaling::Actual;
    config.options.auto_rotate = false;
    let (_, positions) = preview(&marked, &[1], &config).unwrap();
    assert!(positions[0].rect[0] < positions[0].clip[0], "actual-size landscape page clips on portrait paper");
}

#[cfg(windows)]
#[test]
#[ignore = "Exercises the installed Microsoft Print to PDF driver, writing only temporary PDFs"]
fn windows_pdf_driver_prints_vector_and_banded_image_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let pdfium = worker::bind().unwrap();
    let (names, _) = printers().unwrap();
    let name = names
        .iter()
        .find(|n| n.as_str() == "Microsoft Print to PDF")
        .expect("Microsoft Print to PDF must be installed");
    let printer = load_printer(name).unwrap();
    let paper = printer.papers.iter().find(|p| p.name.to_lowercase().contains("a4")).unwrap_or(&printer.papers[0]).id;
    let bytes = build_pdf_rotated(&[0, 90], &[Spec::plain(0, "test annotation")]);
    for (name, as_image, colour) in [
        ("vector", false, Colour::Colour),
        ("grayscale", true, Colour::Grayscale),
        ("monochrome", true, Colour::BlackWhite),
    ] {
        let path = dir.path().join(format!("{name}.pdf"));
        let options = Options { paper, as_image, orientation: Orientation::Auto, colour, ..Options::default() };
        let configured = configure(&printer, &options, [612., 792.]).unwrap();
        assert!(configured.metrics.paper[0] > 0. && configured.metrics.printable[2] > 0.);
        let job = Job {
            printer: printer.clone(),
            options,
            pages: vec![0, 1],
            cancelled: Arc::new(AtomicBool::new(false)),
            output: Some(path.clone()),
        };
        assert!(spool(pdfium, &bytes, &job, |_, _| {}).unwrap());
        let result = (0..100)
            .find_map(|_| match pdf_content::lopdf::Document::load(&path) {
                Ok(doc) if doc.get_pages().len() == 2 => Some(doc),
                _ => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    None
                }
            })
            .expect("The PDF driver finishes writing the output");
        assert_eq!(result.get_pages().len(), 2);
        let second = *result.get_pages().get(&2).unwrap();
        let media = result.get_dictionary(second).unwrap().get(b"MediaBox").unwrap().as_array().unwrap();
        assert!(media[2].as_float().unwrap() > media[3].as_float().unwrap(), "automatic landscape survives ResetDC");
        let printed = pdfium.load_pdf_from_file(&path, None).unwrap();
        for page in 0..2 {
            let image = render_page(&printed.pages().get(page).unwrap(), [400, 600], false, Colour::Colour).unwrap();
            assert!(image.chunks_exact(4).any(|p| p[0] < 200), "page {page} contains printed content");
        }
    }
    let cancelled = Arc::new(AtomicBool::new(true));
    let job = Job {
        printer,
        options: Options { paper, ..Options::default() },
        pages: vec![0],
        cancelled,
        output: Some(dir.path().join("cancelled.pdf")),
    };
    assert!(!spool(pdfium, &bytes, &job, |_, _| {}).unwrap());
    assert!(!dir.path().join("cancelled.pdf").exists());
}
