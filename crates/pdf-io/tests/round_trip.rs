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
    let bytes = pdf_io::append(original.clone(), &s.scales, &pdf_io::write::Changes { viewport_pages: &[0], markups: &refs, ..Default::default() }, NOW).unwrap();
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
    let again = pdf_io::append(blank_pdf(1191, 842), &read.scales, &pdf_io::write::Changes { viewport_pages: &[0], markups: &[moved], ..Default::default() }, NOW).unwrap();
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

#[test]
fn a_markup_can_be_taken_out_of_the_file_or_written_again_in_place() {
    let s = setup();
    let (_, bytes) = written(&s);
    let read = pdf_io::read(&Document::load_mem(&bytes).unwrap());
    let area = read.markups.iter().find(|m| m.kind == MarkupKind::Area).unwrap();
    let length = read.markups.iter().find(|m| m.kind == MarkupKind::Length).unwrap();

    // The area goes; the length is written again, moved.
    let mut moved = length.clone();
    moved.geometry = Geometry::Line { a: Pt::new(50.0, 50.0), b: Pt::new(150.0, 50.0) };
    let removed = [
        pdf_io::write::Removal { page: 0, nm: area.id.to_nm() },
        pdf_io::write::Removal { page: 0, nm: moved.id.to_nm() },
    ];
    let changes = pdf_io::write::Changes { markups: &[&moved], removed: &removed, ..Default::default() };
    let after = pdf_io::append(bytes, &read.scales, &changes, NOW).unwrap();

    let read = pdf_io::read(&Document::load_mem(&after).unwrap());
    assert!(read.markups.iter().all(|m| m.kind != MarkupKind::Area), "the area is gone");
    let lengths: Vec<&Markup> = read.markups.iter().filter(|m| m.kind == MarkupKind::Length).collect();
    assert_eq!(lengths.len(), 1, "written again, not twice");
    assert_eq!(lengths[0].id, length.id, "under the same name");
    assert_eq!(lengths[0].geometry, moved.geometry);
    assert!(!lengths[0].extras.changed_externally);
    // The others are untouched.
    assert_eq!(read.markups.len(), 3);
}

/// A drawing as a clip carries one: a page `size` across with a red line
/// corner to corner, painted through a graphics state kept as an object of
/// its own, so copying it has something to follow.
fn clip_drawing(size: [f64; 2]) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let state = doc.add_object(dictionary! { "Type" => "ExtGState", "CA" => Object::Real(0.5) });
    let content = doc.add_object(pdf_content::lopdf::Stream::new(dictionary! {}, format!("/G0 gs 1 0 0 RG 0 0 m {} {} l S", size[0], size[1]).into_bytes()));
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages, "Contents" => content,
        "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Real(size[0] as f32), Object::Real(size[1] as f32)],
        "Resources" => dictionary! { "ExtGState" => dictionary! { "G0" => state } },
    });
    doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn a_clip_is_written_as_a_stamp_of_its_drawing_and_reads_back_placed_as_it_was() {
    let art = markup_model::ClipArt::new(clip_drawing([100.0, 50.0]), [100.0, 50.0]);
    // Placed turned a quarter, half size: its right runs up the page.
    let corners = art.corners(Pt::new(300.0, 100.0), Pt::new(0.0, 1.0), Pt::new(-1.0, 0.0), 0.5);
    let mut clip = Markup::new(0, MarkupKind::Clip, Geometry::Polygon { pts: corners.clone(), holes: vec![] });
    clip.meta.author = "Estimator".into();
    clip.extras.clip = Some(art);
    let bytes = pdf_io::append(blank_pdf(612, 792), &ScaleStore::default(), &pdf_io::write::Changes { markups: &[&clip], ..Default::default() }, NOW).unwrap();

    let doc = Document::load_mem(&bytes).unwrap();
    let page = doc.get_pages()[&1];
    let annots = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap().clone();
    let stamp = doc.get_dictionary(annots[0].as_reference().unwrap()).unwrap();
    assert_eq!(stamp.get(b"Subtype").unwrap().as_name().unwrap(), b"Stamp", "any viewer shows it from its appearance");
    let rect: Vec<f32> = stamp.get(b"Rect").unwrap().as_array().unwrap().iter().map(|n| n.as_float().unwrap()).collect();
    assert_eq!(rect, [275.0, 100.0, 300.0, 150.0], "the box round its corners");

    let read = pdf_io::read(&doc);
    assert!(read.skipped.is_empty(), "{:?}", read.skipped);
    let back = &read.markups[0];
    assert_eq!((back.id, back.kind, &back.geometry), (clip.id, MarkupKind::Clip, &clip.geometry));
    assert!(!back.extras.changed_externally);
    let art = back.extras.clip.as_ref().expect("its drawing");
    assert_eq!(art.size, [100.0, 50.0]);
    let drawing = Document::load_mem(&art.pdf).unwrap();
    let content = String::from_utf8(drawing.get_page_content(drawing.get_pages()[&1])).unwrap();
    assert!(content.contains("0 0 m 100 50 l S"), "{content}");
    let resources = drawing.get_dictionary(drawing.get_pages()[&1]).unwrap().get(b"Resources").unwrap().as_dict().unwrap();
    let state = resources.get(b"ExtGState").unwrap().as_dict().unwrap().get(b"G0").unwrap();
    assert_eq!(drawing.get_dictionary(state.as_reference().unwrap()).unwrap().get(b"CA").unwrap().as_float().unwrap(), 0.5, "what it refers to came with it");
}

#[test]
fn a_label_s_and_a_ruling_s_own_looks_survive_a_save() {
    let mut area = Markup::new(0, MarkupKind::Area, Geometry::Polygon { pts: vec![Pt::new(0.0, 0.0), Pt::new(100.0, 0.0), Pt::new(100.0, 100.0)], holes: vec![] });
    area.style.fill = Some([0.2, 0.6, 0.9]);
    area.style.pattern = markup_model::FillPattern::Cross;
    area.style.pattern_colour = Some([0.1, 0.2, 0.3]);
    area.style.pattern_opacity = 0.4;
    area.style.pattern_size = 9.0;
    area.style.label_colour = Some([0.5, 0.25, 0.0]);
    area.style.label_font = markup_model::LabelFont::Mono;
    let bytes = pdf_io::append(blank_pdf(612, 792), &ScaleStore::default(), &pdf_io::write::Changes { markups: &[&area], ..Default::default() }, NOW).unwrap();
    let back = pdf_io::read(&Document::load_mem(&bytes).unwrap()).markups.remove(0);
    assert_eq!(back.style.pattern_colour, area.style.pattern_colour);
    assert!((back.style.pattern_opacity - 0.4).abs() < 1e-6);
    assert!((back.style.pattern_size - 9.0).abs() < 1e-6);
    assert_eq!(back.style.label_colour, area.style.label_colour);
    assert_eq!(back.style.label_font, markup_model::LabelFont::Mono);
}

fn on_layer(z: f64, layer: markup_model::LayerId) -> Markup {
    let mut m = Markup::new(0, MarkupKind::Area, Geometry::Polygon { pts: vec![Pt::new(0.0, 0.0), Pt::new(100.0, 0.0), Pt::new(100.0, 100.0)], holes: vec![] });
    m.extras.z = z;
    m.layer = layer;
    m
}

fn annotation_oc(doc: &Document, nm: &str) -> Option<ObjectId> {
    let page = *doc.get_pages().get(&1).unwrap();
    let annots = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap().clone();
    annots.iter().find_map(|a| {
        let d = doc.get_dictionary(a.as_reference().ok()?).ok()?;
        let name = pdf_content::lopdf::decode_text_string(d.get(b"NM").ok()?).ok()?;
        (name == nm).then(|| d.get(b"OC").ok().and_then(|o| o.as_reference().ok())).flatten()
    })
}

#[test]
fn layers_are_written_as_optional_content_and_read_back_as_they_were() {
    use markup_model::LayerStack;
    let mut stack = LayerStack::default();
    let (walls, notes) = (stack.add("Walls"), stack.add("Notes"));
    let doors = stack.add_in(Some(walls), "Doors");
    stack.set_locked(doors, true);
    stack.set_colour(walls, Some([0.25, 0.5, 0.75]));
    stack.set_visible(notes, false);
    stack.set_locked(walls, true);
    stack.rename(markup_model::LayerId::DEFAULT, "Base");
    stack.move_to(notes, 1);
    assert_eq!(stack.depth(doors), 1);
    let (back, front, plain) = (on_layer(-2.5, walls), on_layer(7.25, walls), on_layer(0.0, markup_model::LayerId::DEFAULT));
    let changes = pdf_io::write::Changes { markups: &[&back, &front, &plain], layers: Some(&stack), ..Default::default() };
    let bytes = pdf_io::append(blank_pdf(612, 792), &ScaleStore::default(), &changes, NOW).unwrap();
    let doc = Document::load_mem(&bytes).unwrap();
    let read = pdf_io::read(&doc);

    assert_eq!(read.layers, stack, "order, names, on and locked");
    for original in [&back, &front, &plain] {
        let found = read.markups.iter().find(|m| m.id == original.id).unwrap();
        assert_eq!((found.extras.z, found.layer), (original.extras.z, original.layer));
    }
    // Another program hides a layer by its group, and finds each markup on its own.
    let (a, b) = (annotation_oc(&doc, &back.id.to_nm()), annotation_oc(&doc, &front.id.to_nm()));
    assert!(a.is_some() && a == b, "both on the Walls group");
    assert_ne!(annotation_oc(&doc, &plain.id.to_nm()), a, "and the plain one on the default layer's");
}

#[test]
fn a_later_save_changes_layers_without_losing_the_files_own_or_others() {
    use markup_model::LayerStack;
    let mut stack = LayerStack::default();
    let (walls, notes) = (stack.add("Walls"), stack.add("Notes"));
    let first = pdf_io::append(blank_pdf(612, 792), &ScaleStore::default(), &pdf_io::write::Changes { layers: Some(&stack), ..Default::default() }, NOW).unwrap();

    // A markup added later, with the layers left alone, still finds its group.
    let m = on_layer(0.0, notes);
    let second = pdf_io::append(first, &ScaleStore::default(), &pdf_io::write::Changes { markups: &[&m], ..Default::default() }, NOW).unwrap();
    assert!(annotation_oc(&Document::load_mem(&second).unwrap(), &m.id.to_nm()).is_some());

    // Taking a layer out, hiding another.
    stack.remove(walls);
    stack.set_visible(notes, false);
    let third = pdf_io::append(second, &ScaleStore::default(), &pdf_io::write::Changes { layers: Some(&stack), ..Default::default() }, NOW).unwrap();
    let read = pdf_io::read(&Document::load_mem(&third).unwrap());
    assert_eq!(read.layers, stack);
    assert_eq!(read.markups[0].layer, notes);
}

#[test]
fn a_layer_named_in_text_by_an_older_file_is_the_same_layer_every_time() {
    let mut m = on_layer(0.0, markup_model::LayerId::DEFAULT);
    m.id = markup_model::MarkupId::new();
    let bytes = pdf_io::append(blank_pdf(612, 792), &ScaleStore::default(), &pdf_io::write::Changes { markups: &[&m], ..Default::default() }, NOW).unwrap();
    // As an older build wrote it: the name in /KPDF, no ID, no group.
    let mut doc = Document::load_mem(&bytes).unwrap();
    let page = *doc.get_pages().get(&1).unwrap();
    let annot = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap()[0].as_reference().unwrap();
    let Object::Dictionary(kpdf) = doc.get_dictionary_mut(annot).unwrap().get_mut(b"KPDF").unwrap() else { panic!("no /KPDF") };
    kpdf.set("Layer", Object::string_literal("Structure"));
    let mut old = Vec::new();
    doc.save_to(&mut old).unwrap();

    let (first, second) = (pdf_io::read(&Document::load_mem(&old).unwrap()), pdf_io::read(&Document::load_mem(&old).unwrap()));
    let layer = first.markups[0].layer;
    assert!(!layer.is_default() && layer == second.markups[0].layer && layer == markup_model::LayerId::from_name("Structure"));
    assert_eq!(first.layers.name(layer), "Structure", "listed, though nothing recorded it");
}

/// One of each shape the drawing tools make, written and read back.
fn drawn_shapes() -> Vec<Markup> {
    let corners = vec![Pt::new(100.0, 100.0), Pt::new(300.0, 100.0), Pt::new(300.0, 220.0), Pt::new(100.0, 220.0)];
    let mut frame = Markup::new(0, MarkupKind::Box, Geometry::Polygon { pts: corners.clone(), holes: vec![] });
    frame.style.fill = Some([0.9, 0.9, 0.2]);
    frame.style.fill_opacity = 0.4;
    frame.meta.label = "Check this".into();
    // Turned, as a shape that has been rotated is: still a box.
    let turned = vec![Pt::new(400.0, 300.0), Pt::new(480.0, 340.0), Pt::new(440.0, 420.0), Pt::new(360.0, 380.0)];
    let mut oval = Markup::new(0, MarkupKind::Ellipse, Geometry::Polygon { pts: turned, holes: vec![] });
    oval.style.dash = vec![4.0, 2.0];
    let line = Markup::new(0, MarkupKind::Line, Geometry::Line { a: Pt::new(50.0, 500.0), b: Pt::new(250.0, 520.0) });
    let mut arrow = Markup::new(0, MarkupKind::Arrow, Geometry::Line { a: Pt::new(50.0, 600.0), b: Pt::new(250.0, 640.0) });
    arrow.style.width = 3.0;
    let pen = Markup::new(0, MarkupKind::Pen, Geometry::Ink { strokes: vec![vec![Pt::new(10.0, 10.0), Pt::new(20.0, 30.0), Pt::new(45.0, 25.0)], vec![Pt::new(60.0, 60.0), Pt::new(70.0, 90.0)]] });
    vec![frame, oval, line, arrow, pen]
}

#[test]
fn shapes_drawn_on_the_page_are_written_as_the_annotations_other_programs_write_and_read_back() {
    let shapes = drawn_shapes();
    let refs: Vec<&Markup> = shapes.iter().collect();
    let bytes = pdf_io::append(blank_pdf(612, 792), &ScaleStore::default(), &pdf_io::write::Changes { markups: &refs, ..Default::default() }, NOW).unwrap();
    let doc = Document::load_mem(&bytes).unwrap();
    let read = pdf_io::read(&doc);
    assert!(read.skipped.is_empty(), "{:?}", read.skipped);
    assert_eq!(read.markups.len(), 5);
    for original in &shapes {
        let back = read.markups.iter().find(|m| m.id == original.id).expect("same ID");
        assert_eq!(back.kind, original.kind);
        assert!(!back.extras.changed_externally, "{:?}: our own save isn't an outside edit", original.kind);
        assert_eq!(back.style.dash, original.style.dash);
        assert_eq!(back.meta.label, original.meta.label);
        assert_eq!(back.style.fill, original.style.fill);
        let close = |a: Pt, b: Pt| a.dist(b) < 1e-3;
        match (&back.geometry, &original.geometry) {
            (Geometry::Polygon { pts: a, .. }, Geometry::Polygon { pts: b, .. }) => assert!(a.iter().zip(b).all(|(a, b)| close(*a, *b)), "{:?}", original.kind),
            (Geometry::Line { a, b }, Geometry::Line { a: c, b: d }) => assert!(close(*a, *c) && close(*b, *d)),
            (Geometry::Ink { strokes: a }, Geometry::Ink { strokes: b }) => {
                assert_eq!(a.len(), b.len());
                assert!(a.iter().flatten().zip(b.iter().flatten()).all(|(a, b)| close(*a, *b)));
            }
            (a, b) => panic!("{:?} read back as {a:?}, not {b:?}", original.kind),
        }
    }
    // What another program sees: the standard subtype and an appearance,
    // with no measurement on any of them.
    let page = *doc.get_pages().get(&1).unwrap();
    let annots = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap().clone();
    let mut subtypes: Vec<String> = annots
        .iter()
        .map(|a| {
            let d = doc.get_dictionary(a.as_reference().unwrap()).unwrap();
            assert!(d.has(b"AP") && !d.has(b"Measure"), "an appearance, and nothing measured");
            String::from_utf8(d.get(b"Subtype").unwrap().as_name().unwrap().to_vec()).unwrap()
        })
        .collect();
    subtypes.sort();
    assert_eq!(subtypes, ["Circle", "Ink", "Line", "Line", "Square"]);
}

/// A text box with an arrow, set in Arial with a bold line.
fn text_box() -> Markup {
    let mut words = markup_model::TextBox::plain("Existing kerb 45°\nto be removed", &markup_model::RunFormat { font: "Arial".into(), size: 12.0, ..Default::default() }, markup_model::HAlign::Centre);
    words.paragraphs[1].runs[0].format.bold = true;
    words.callout = Some(Pt::new(50.0, 50.0));
    let frame_corners = vec![Pt::new(200.0, 300.0), Pt::new(360.0, 300.0), Pt::new(360.0, 360.0), Pt::new(200.0, 360.0)];
    let mut m = Markup::new(0, MarkupKind::Text, Geometry::Polygon { pts: frame_corners, holes: vec![] });
    m.style.fill = Some([1.0, 1.0, 0.8]);
    m.style.width = 1.0;
    m.extras.text = Some(words);
    m
}

#[test]
fn a_text_box_is_written_as_free_text_in_its_embedded_font_and_reads_back_as_typed() {
    if !text_layout::catalogue().has("Arial") {
        return;
    }
    let m = text_box();
    let bytes = pdf_io::append(blank_pdf(612, 792), &ScaleStore::default(), &pdf_io::write::Changes { markups: &[&m], ..Default::default() }, NOW).unwrap();
    let doc = Document::load_mem(&bytes).unwrap();
    let read = pdf_io::read(&doc);
    assert!(read.skipped.is_empty(), "{:?}", read.skipped);
    let back = &read.markups[0];
    assert_eq!((back.id, back.kind, &back.geometry), (m.id, MarkupKind::Text, &m.geometry));
    assert_eq!(back.extras.text, m.extras.text, "the words and formats as typed, degree sign and all");
    assert!(!back.extras.changed_externally);

    let page = doc.get_pages()[&1];
    let annots = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap().clone();
    let annot = doc.get_dictionary(annots[0].as_reference().unwrap()).unwrap();
    assert_eq!(annot.get(b"Subtype").unwrap().as_name().unwrap(), b"FreeText");
    assert_eq!(annot.get(b"IT").unwrap().as_name().unwrap(), b"FreeTextCallout");
    let callout: Vec<f32> = annot.get(b"CL").unwrap().as_array().unwrap().iter().map(|n| n.as_float().unwrap()).collect();
    assert_eq!(&callout[..2], &[50.0, 50.0], "the arrow points where it was put");
    let contents = pdf_content::lopdf::decode_text_string(annot.get(b"Contents").unwrap()).unwrap();
    assert_eq!(contents, "Existing kerb 45°\nto be removed");

    // Two faces used, regular and bold, each embedded once, with a map back
    // to the characters.
    let programs = doc.objects.values().filter_map(|o| o.as_stream().ok()).filter(|s| s.dict.has(b"KPDFFont")).count();
    assert_eq!(programs, 2, "Arial and Arial Bold");
    let type0 = doc.objects.values().filter_map(|o| o.as_dict().ok()).filter(|d| d.get(b"Subtype").and_then(|s| s.as_name()).ok() == Some(b"Type0")).count();
    assert_eq!(type0, 2);
    assert!(doc.objects.values().filter_map(|o| o.as_dict().ok()).filter(|d| d.get(b"Subtype").and_then(|s| s.as_name()).ok() == Some(b"Type0")).all(|d| d.has(b"ToUnicode")));

    // A machine without Arial sets the box in the file's own copies, which
    // measure the words just as the installed ones do.
    let mut carried = pdf_io::read::text_box_fonts(&doc, annot, |_, _, _| true);
    carried.sort_by_key(|f| f.bold);
    let faces: Vec<_> = carried.iter().map(|f| (f.family.as_str(), f.bold, f.italic)).collect();
    assert_eq!(faces, vec![("Arial", false, false), ("Arial", true, false)]);
    let bare = text_layout::Catalogue::scan(&[]);
    for font in carried {
        assert!(bare.take_in(&font.family, font.bold, font.italic, &font.postscript, font.data));
    }
    for bold in [false, true] {
        let (theirs, ours) = (bare.face("Arial", bold, false).unwrap(), text_layout::catalogue().face("Arial", bold, false).unwrap());
        assert_eq!(theirs.width("Existing kerb 45°", 12.0), ours.width("Existing kerb 45°", 12.0));
        assert_eq!(theirs.entry.postscript, ours.entry.postscript);
    }
    assert!(!text_layout::catalogue().only_embedded("Arial"), "installed here, the installed one is used");

    // Drawn back from the file, as another viewer would: the name changed
    // so the renderer doesn't leave it to the app.
    let mut shown = doc.clone();
    shown.get_dictionary_mut(annots[0].as_reference().unwrap()).unwrap().set("NM", Object::string_literal("OTHER"));
    let shapes = gpu_lines::annotation_shapes(&shown, 1, 0.05, 1.0).unwrap();
    assert!(shapes.not_drawn.is_empty(), "{:?}", shapes.not_drawn);
    assert!(shapes.triangles > 200, "the letters are there: {}", shapes.triangles);

    // Saved again, the fonts already in the file are used, not put in twice.
    let again = pdf_io::append(bytes.clone(), &read.scales, &pdf_io::write::Changes { markups: &[&text_box()], ..Default::default() }, NOW).unwrap();
    let doc = Document::load_mem(&again).unwrap();
    let programs = doc.objects.values().filter_map(|o| o.as_stream().ok()).filter(|s| s.dict.has(b"KPDFFont")).count();
    assert_eq!(programs, 2, "the second save shares the first's fonts");
}
