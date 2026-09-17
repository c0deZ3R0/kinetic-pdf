//! Measurement markups written by `pdf-io` open in pdfium as the annotations
//! they claim to be, and draw from their appearance streams where they are.

use markup_model::{Geometry, Markup, MarkupKind, Pt, Rect, Scale, ScaleId, ScaleStore};
use pdf_content::fixtures::blank_page_pdf;
use pdfium_render::prelude::*;

const WIDTH: u16 = 1191;
const HEIGHT: u16 = 842;

fn written() -> (Vec<u8>, Vec<Markup>) {
    let mut scales = ScaleStore::default();
    let scale = Scale::from_ratio(ScaleId::new(), 100.0).unwrap();
    let id = scale.id;
    scales.set_scale(scale);
    scales.set_page_scale(0, Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(f64::from(WIDTH), f64::from(HEIGHT))), id);

    let mut area = Markup::new(
        0,
        MarkupKind::Area,
        Geometry::Polygon {
            pts: vec![Pt::new(100.0, 100.0), Pt::new(500.0, 100.0), Pt::new(500.0, 400.0), Pt::new(100.0, 400.0)],
            holes: vec![vec![Pt::new(200.0, 200.0), Pt::new(260.0, 200.0), Pt::new(260.0, 260.0)]],
        },
    );
    area.style.fill = Some([0.2, 0.6, 0.9]);
    area.style.opacity = 0.5;
    let mut length = Markup::new(0, MarkupKind::Length, Geometry::Line { a: Pt::new(100.0, 600.0), b: Pt::new(700.0, 600.0) });
    length.style.width = 4.0;
    let polylength = Markup::new(0, MarkupKind::Polylength, Geometry::Polyline { pts: vec![Pt::new(800.0, 100.0), Pt::new(1000.0, 100.0), Pt::new(1000.0, 300.0)] });
    let markups = vec![area, length, polylength];
    let refs: Vec<&Markup> = markups.iter().collect();
    (pdf_io::append(blank_page_pdf(i64::from(WIDTH), i64::from(HEIGHT)), &scales, &pdf_io::write::Changes { viewport_pages: &[0], markups: &refs, ..Default::default() }, 0).unwrap(), markups)
}

/// The page drawn a pixel a point, and a function giving the RGB at a point
/// in user space.
fn render(page: &PdfPage, annotations: bool) -> impl Fn(f64, f64) -> [u8; 3] {
    let config = PdfRenderConfig::new().set_target_width(i32::from(WIDTH)).render_annotations(annotations);
    let bitmap = page.render_with_config(&config).unwrap();
    let (w, rgba) = (bitmap.width() as usize, bitmap.as_rgba_bytes());
    move |x, y| {
        let at = ((f64::from(HEIGHT) - y) as usize * w + x as usize) * 4;
        [rgba[at], rgba[at + 1], rgba[at + 2]]
    }
}

#[test]
fn pdfium_opens_the_markups_as_measurement_annotations_and_draws_them() {
    let (bytes, markups) = written();
    let pdfium = kinetic_pdf::worker::bind().unwrap();
    let doc = pdfium.load_pdf_from_byte_slice(&bytes, None).unwrap();
    let page = doc.pages().get(0).unwrap();

    let annotations: Vec<_> = page.annotations().iter().collect();
    let types: Vec<_> = annotations.iter().map(|a| a.annotation_type()).collect();
    assert_eq!(types, [PdfPageAnnotationType::Polygon, PdfPageAnnotationType::Line, PdfPageAnnotationType::Polyline]);
    let names: Vec<_> = annotations.iter().map(|a| a.name()).collect();
    let ids: Vec<_> = markups.iter().map(|m| Some(m.id.to_nm())).collect();
    assert_eq!(names, ids);

    let plain = render(&page, false);
    let drawn = render(&page, true);
    let bluish = |[r, _, b]: [u8; 3]| i32::from(b) - i32::from(r) > 40;
    let white = |[r, g, b]: [u8; 3]| r > 240 && g > 240 && b > 240;
    assert!(white(plain(150.0, 150.0)), "without annotations the page is blank there");
    assert!(bluish(drawn(150.0, 150.0)), "the area's fill: {:?}", drawn(150.0, 150.0));
    assert!(white(drawn(250.0, 210.0)), "the cutout isn't filled: {:?}", drawn(250.0, 210.0));
    let [r, g, b] = drawn(200.0, 600.0);
    assert!(r > 150 && g < 100 && b < 100, "the length's red stroke: {:?}", [r, g, b]);
}
