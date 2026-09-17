//! Writing measurement markups into a PDF and reading them back gives the
//! same model; the file keeps its original bytes, shares one /Measure per
//! scale, and shows edits made elsewhere.

use markup_model::markup::{MetaValue, RawValue, Slope};
use markup_model::{quantities, Geometry, Markup, MarkupKind, Pt, Rect, Scale, ScaleId, ScaleRef, ScaleStore, Viewport, ViewportId};
use pdf_content::fixtures::blank_page_pdf as blank_pdf;
use pdf_content::lopdf::{dictionary, Document, Object, ObjectId};

const NOW: i64 = 1_789_653_909_000;

/// An A3 landscape sheet at 1:100 with a 1:20 detail in one corner, and one
/// markup of each kind written so far, the area with a cutout.
struct Setup {
    scales: ScaleStore,
    page_scale: ScaleId,
    detail_scale: ScaleId,
    markups: Vec<Markup>,
}

fn setup() -> Setup {
    let mut scales = ScaleStore::default();
    let page_scale = Scale::from_ratio(ScaleId::new(), 100.0).unwrap();
    let mut detail_scale = Scale::from_ratio(ScaleId::new(), 20.0).unwrap();
    detail_scale.precision = markup_model::Precision::Decimals(3);
    let (page_id, detail_id) = (page_scale.id, detail_scale.id);
    scales.set_scale(page_scale);
    scales.set_scale(detail_scale);
    scales.set_page_scale(0, Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(1191.0, 842.0)), page_id);
    scales.set_viewport(Viewport {
        id: ViewportId::new(),
        page: 0,
        bbox: Rect::from_corners(Pt::new(800.0, 500.0), Pt::new(1150.0, 800.0)),
        name: "Detail A".into(),
        scale: detail_id,
        whole_page: false,
    });

    let mut area = Markup::new(
        0,
        MarkupKind::Area,
        Geometry::Polygon {
            pts: vec![Pt::new(100.0, 100.0), Pt::new(500.3, 100.0), Pt::new(500.3, 400.7), Pt::new(100.0, 400.7)],
            holes: vec![vec![Pt::new(200.0, 200.0), Pt::new(260.0, 200.0), Pt::new(260.0, 260.0)]],
        },
    );
    area.style.fill = Some([0.2, 0.6, 0.9]);
    area.style.opacity = 0.5;
    area.meta.label = "Asphalt".into();
    area.meta.author = "Estimator".into();
    area.meta.item_code = Some("A-120".into());
    area.meta.status = Some("Captured".into());
    area.meta.custom.insert("Rate".into(), MetaValue::Number(85.5));
    area.extras.slope = Some(Slope { rise: 1.0, run: 20.0 });

    let mut volume = Markup::new(
        0,
        MarkupKind::Volume,
        Geometry::Polygon { pts: vec![Pt::new(850.0, 550.0), Pt::new(950.0, 550.0), Pt::new(950.0, 650.0), Pt::new(850.0, 650.0)], holes: vec![] },
    );
    volume.extras.depth_m = Some(0.3);
    volume.meta.created_ms = Some(NOW - 86_400_000);

    let length = Markup::new(0, MarkupKind::Length, Geometry::Line { a: Pt::new(100.0, 600.0), b: Pt::new(383.46, 600.0) });
    let mut polylength = Markup::new(0, MarkupKind::Polylength, Geometry::Polyline { pts: vec![Pt::new(100.0, 700.0), Pt::new(300.0, 700.0), Pt::new(300.0, 780.0)] });
    // Measured at 1:20 though it's outside the detail.
    polylength.scale_ref = ScaleRef::Override(detail_id);
    polylength.style.dash = vec![6.0, 3.0];

    Setup { scales, page_scale: page_id, detail_scale: detail_id, markups: vec![area, volume, length, polylength] }
}

fn written(s: &Setup) -> (Vec<u8>, Vec<u8>) {
    let original = blank_pdf(1191, 842);
    let refs: Vec<&Markup> = s.markups.iter().collect();
    let bytes = pdf_io::append(original.clone(), &s.scales, &[0], &refs, NOW).unwrap();
    (original, bytes)
}

#[test]
fn the_original_bytes_come_first_untouched() {
    let s = setup();
    let (original, bytes) = written(&s);
    assert!(bytes.len() > original.len());
    assert!(bytes.starts_with(&original));
}

#[test]
fn each_scale_is_one_measure_object_shared_by_viewports_and_markups() {
    let s = setup();
    let (_, bytes) = written(&s);
    let doc = Document::load_mem(&bytes).unwrap();
    let measures: Vec<ObjectId> =
        doc.objects.iter().filter(|(_, o)| o.as_dict().is_ok_and(|d| d.has_type(b"Measure"))).map(|(id, _)| *id).collect();
    assert_eq!(measures.len(), 2, "one per scale");

    let page = doc.get_dictionary(doc.get_pages()[&1]).unwrap();
    let vp = page.get(b"VP").unwrap().as_array().unwrap();
    let vp_measures: Vec<ObjectId> = vp.iter().map(|v| v.as_dict().unwrap().get(b"Measure").unwrap().as_reference().unwrap()).collect();
    assert_eq!(vp.len(), 2);
    assert_ne!(vp_measures[0], vp_measures[1]);

    let annots = page.get(b"Annots").unwrap().as_array().unwrap();
    let annot_measures: Vec<ObjectId> =
        annots.iter().map(|a| doc.get_dictionary(a.as_reference().unwrap()).unwrap().get(b"Measure").unwrap().as_reference().unwrap()).collect();
    // Area and length at the page's scale; the volume in the detail; the
    // polylength overridden to the detail's.
    assert_eq!(annot_measures, [vp_measures[0], vp_measures[1], vp_measures[0], vp_measures[1]]);

    let area = doc.get_dictionary(annots[0].as_reference().unwrap()).unwrap();
    assert_eq!(area.get(b"Subtype").unwrap().as_name().unwrap(), b"Polygon");
    assert_eq!(area.get(b"IT").unwrap().as_name().unwrap(), b"PolygonDimension");
    assert!(area.get(b"AP").unwrap().as_dict().unwrap().has(b"N"));
    let contents = pdf_content::lopdf::decode_text_string(area.get(b"Contents").unwrap()).unwrap();
    assert!(contents.starts_with("Asphalt: ") && contents.ends_with(" m²"), "{contents}");
}

#[test]
fn reading_back_gives_the_same_markups_scales_and_quantities() {
    let s = setup();
    let (_, bytes) = written(&s);
    let read = pdf_io::read(&Document::load_mem(&bytes).unwrap());
    assert!(read.skipped.is_empty(), "{:?}", read.skipped);
    assert_eq!(read.markups.len(), 4);
    assert_eq!(read.scales.viewports(0).len(), 2);
    assert!(read.scales.page_default(0).is_some_and(|v| v.scale == s.page_scale));
    assert_eq!(read.scales.viewports(0)[1].name, "Detail A");
    assert_eq!(read.scales.scale(s.detail_scale).unwrap().precision, markup_model::Precision::Decimals(3));

    for original in &s.markups {
        let back = read.markups.iter().find(|m| m.id == original.id).expect("same ID");
        assert_eq!((back.kind, back.scale_ref), (original.kind, original.scale_ref), "{:?}", original.kind);
        assert!(!back.extras.changed_externally, "{:?}: our own save isn't an outside edit", original.kind);
        assert_eq!(back.meta.label, original.meta.label);
        assert_eq!(back.meta.item_code, original.meta.item_code);
        assert_eq!(back.meta.custom, original.meta.custom);
        assert_eq!(back.meta.created_ms, Some(original.meta.created_ms.unwrap_or(NOW)));
        assert_eq!(back.style.dash, original.style.dash);
        assert_eq!(back.extras.depth_m.map(|d| d as f32), original.extras.depth_m.map(|d| d as f32));

        let q = |m: &Markup, scales: &ScaleStore| {
            let scale = scales.resolve(m.page, m.geometry.first_point(), m.scale_ref).map(|(s, _)| s);
            quantities(m, scale).unwrap()
        };
        let (before, after) = (q(original, &s.scales), q(back, &read.scales));
        let near = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(a), Some(b)) => (a - b).abs() <= 1e-5 * a.abs().max(1.0),
            (a, b) => a == b,
        };
        assert!(near(before.area_m2, after.area_m2) && near(before.length_m, after.length_m) && near(before.volume_m3, after.volume_m3), "{before:?} vs {after:?}");
    }
    let length = read.markups.iter().find(|m| m.kind == MarkupKind::Length).unwrap();
    let scale = read.scales.resolve(0, length.geometry.first_point(), length.scale_ref).unwrap().0;
    // 283.46 pt is 10 cm on paper: 10 m at 1:100.
    assert!((quantities(length, Some(scale)).unwrap().length_m.unwrap() - 10.0).abs() < 1e-3);
}

#[test]
fn a_markup_moved_by_another_program_is_flagged_and_its_foreign_keys_kept() {
    let s = setup();
    let (_, bytes) = written(&s);
    let mut doc = Document::load_mem(&bytes).unwrap();
    let page = doc.get_pages()[&1];
    let annots = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap().clone();
    let length = doc.get_dictionary_mut(annots[2].as_reference().unwrap()).unwrap();
    length.set("L", vec![Object::Real(100.0), Object::Real(600.0), Object::Real(400.0), Object::Real(600.0)]);
    length.set("SomeoneElses", Object::Name(b"Value".to_vec()));
    let read = pdf_io::read(&doc);
    let flagged: Vec<MarkupKind> = read.markups.iter().filter(|m| m.extras.changed_externally).map(|m| m.kind).collect();
    assert_eq!(flagged, [MarkupKind::Length]);
    let moved = read.markups.iter().find(|m| m.kind == MarkupKind::Length).unwrap();
    assert_eq!(moved.extras.raw.get(b"SomeoneElses".as_slice()), Some(&RawValue::Name(b"Value".to_vec())));

    // Written again, the foreign key goes back as it was.
    let again = pdf_io::append(blank_pdf(1191, 842), &read.scales, &[0], &[moved], NOW).unwrap();
    let doc = Document::load_mem(&again).unwrap();
    let back = pdf_io::read(&doc);
    assert!(back.markups[0].extras.raw.contains_key(b"SomeoneElses".as_slice()));
    assert!(!back.markups[0].extras.changed_externally, "saved by us again, the hash is fresh");
}

#[test]
fn a_plain_iso_measurement_from_another_program_is_read() {
    let mut doc = Document::load_mem(&blank_pdf(612, 792)).unwrap();
    let measure = doc.add_object(dictionary! {
        "Type" => "Measure", "Subtype" => "RL", "R" => Object::string_literal("1 in = 10 ft"),
        "X" => vec![Object::Dictionary(dictionary! { "U" => Object::string_literal("ft"), "C" => Object::Real(10.0 / 72.0), "D" => 100 })],
    });
    let page = doc.get_pages()[&1];
    let annot = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Line", "IT" => "LineDimension", "NM" => Object::string_literal("OTHER-APP-1"),
        "L" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(72), Object::Integer(0)],
        "Measure" => measure,
        "Rect" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(72), Object::Integer(1)],
    });
    doc.get_dictionary_mut(page).unwrap().set("Annots", vec![Object::Reference(annot)]);
    let read = pdf_io::read(&doc);
    let m = &read.markups[0];
    assert_eq!((m.kind, m.extras.foreign_nm.as_deref(), m.extras.changed_externally), (MarkupKind::Length, Some("OTHER-APP-1"), false));
    let ScaleRef::Override(id) = m.scale_ref else { panic!("no viewports, so its own /Measure") };
    let length = quantities(m, read.scales.scale(id)).unwrap().length_m.unwrap();
    assert!((length - 3.048).abs() < 1e-5, "{length}");
}
