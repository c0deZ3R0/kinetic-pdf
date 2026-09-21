//! Writes a PDF with one of each measurement markup written so far, at a
//! known scale, to look at by hand.
//!
//! cargo run --example measure_sample -- out.pdf

use markup_model::{Geometry, Markup, MarkupKind, Pt, Rect, Scale, ScaleId, ScaleStore};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "tmp/measure-sample.pdf".to_owned());
    // An A3 landscape sheet at 1:100: 1 cm on paper is 1 m.
    let mut scales = ScaleStore::default();
    let scale = Scale::from_ratio(ScaleId::new(), 100.0).unwrap();
    let id = scale.id;
    scales.set_scale(scale);
    scales.set_page_scale(0, Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(1191.0, 842.0)), id);

    let cm = 72.0 / 2.54;
    let at = |x: f64, y: f64| Pt::new(x * cm, y * cm);
    let mut area = Markup::new(0, MarkupKind::Area, Geometry::Polygon { pts: vec![at(2.0, 2.0), at(12.0, 2.0), at(12.0, 10.0), at(2.0, 10.0)], holes: vec![] });
    area.style.fill = Some([0.2, 0.6, 0.9]);
    area.style.opacity = 0.5;
    area.meta.label = "Area 80 m²".into();
    let mut length = Markup::new(0, MarkupKind::Length, Geometry::Line { a: at(2.0, 14.0), b: at(27.0, 14.0) });
    length.meta.label = "Length 25 m".into();
    let mut polylength = Markup::new(0, MarkupKind::Polylength, Geometry::Polyline { pts: vec![at(16.0, 2.0), at(26.0, 2.0), at(26.0, 10.0)] });
    polylength.meta.label = "Polylength 18 m".into();
    let markups = [area, length, polylength];
    let refs: Vec<&Markup> = markups.iter().collect();

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let changes = pdf_io::write::Changes { viewport_pages: &[0], markups: &refs, ..Default::default() };
    let bytes = pdf_io::append(pdf_content::fixtures::blank_page_pdf(1191, 842), &scales, &changes, now).expect("writes");
    std::fs::write(&out, bytes).expect("saves");
    println!("wrote {out}");
}
