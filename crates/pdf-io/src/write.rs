//! Appending measurement markups, viewports and scales to a PDF as an
//! incremental update: the original bytes stay exactly as they were, with the
//! new and changed objects after them.

use std::collections::HashMap;

use markup_model::hash::geom_hash_hex;
use markup_model::markup::{MetaValue, WidthUnit};
use markup_model::{quantities, Geometry, Markup, MarkupKind, PageIndex, ScaleId, ScaleRef, ScaleStore};
use pdf_content::lopdf::{dictionary, Dictionary, Document, IncrementalDocument, Object, ObjectId, Stream};

use crate::appearance::{appearance, form_dict};
use crate::measure::measure_dict;
use crate::values::{name, pdf_date, points, real, reals, text};
use crate::Error;

/// The annotation subtype a kind is written as, and the ISO measurement
/// intent where the standard has one.
///
/// Only lengths, runs and areas have an intent of their own in ISO 32000; an
/// angle, a radius, a diameter or a count is written as the annotation whose
/// shape it has, with what it measures in /KPDF. Another program reading one
/// sees the shape and the appearance, and its quantity in /Contents.
fn subtype(kind: MarkupKind, geometry: &Geometry) -> Result<(&'static str, Option<&'static str>), Error> {
    match (kind, geometry) {
        (MarkupKind::Length, _) => Ok(("Line", Some("LineDimension"))),
        (MarkupKind::Polylength, _) => Ok(("PolyLine", Some("PolyLineDimension"))),
        (MarkupKind::Area | MarkupKind::Perimeter | MarkupKind::Volume, _) => Ok(("Polygon", Some("PolygonDimension"))),
        (MarkupKind::Angle, _) => Ok(("PolyLine", None)),
        (MarkupKind::Radius | MarkupKind::Diameter, Geometry::Line { .. }) => Ok(("Line", None)),
        (MarkupKind::Radius | MarkupKind::Diameter, Geometry::Ellipse { .. }) => Ok(("Circle", None)),
        (MarkupKind::Radius | MarkupKind::Diameter, _) => Ok(("PolyLine", None)),
        (MarkupKind::Count, _) => Ok(("Polygon", None)),
        (other, _) => Err(Error::Unsupported(format!("writing {other:?} markups"))),
    }
}

/// The /KPDF name of a kind.
pub fn kind_name(kind: MarkupKind) -> String {
    format!("{kind:?}")
}

/// Writes each scale's /Measure once, as an indirect object shared by every
/// viewport and markup that uses it.
struct Measures<'a> {
    scales: &'a ScaleStore,
    written: HashMap<ScaleId, ObjectId>,
}

impl Measures<'_> {
    fn reference(&mut self, update: &mut IncrementalDocument, id: ScaleId) -> Result<ObjectId, Error> {
        if let Some(&written) = self.written.get(&id) {
            return Ok(written);
        }
        let scale = self.scales.scale(id).ok_or_else(|| Error::Invalid(format!("no scale {id}")))?;
        let object = update.new_document.add_object(measure_dict(scale));
        self.written.insert(id, object);
        Ok(object)
    }
}

/// An annotation to take out of the file: which page it's on, and the /NM it
/// was written with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removal {
    pub page: PageIndex,
    pub nm: String,
}

/// What a save writes.
#[derive(Default)]
pub struct Changes<'a> {
    /// Pages whose /VP is written afresh from `scales`.
    pub viewport_pages: &'a [PageIndex],
    /// Markups written as new annotations. One whose /NM is also in
    /// `removed` replaces what was there.
    pub markups: &'a [&'a Markup],
    /// Annotations taken out of their pages' /Annots.
    pub removed: &'a [Removal],
}

/// `bytes` with `changes` appended as an incremental update: the original
/// bytes stay as they are, with the new and changed objects after them.
/// `now_ms` stamps what's written.
///
/// A markup that is in both `removed` and `markups` is written again in place
/// of what was there, which is how one is changed: the annotation is replaced
/// under the same /NM.
pub fn append(bytes: Vec<u8>, scales: &ScaleStore, changes: &Changes, now_ms: i64) -> Result<Vec<u8>, Error> {
    let Changes { viewport_pages, markups, removed } = *changes;
    let previous = Document::load_mem(&bytes)?;
    let pages = previous.get_pages();
    let page_id = |page: PageIndex| pages.get(&(page + 1)).copied().ok_or(Error::NoPage(page));
    let mut update = IncrementalDocument::create_from(bytes, previous);
    let mut measures = Measures { scales, written: HashMap::new() };

    // Taken out first, so a markup written again lands after what's left.
    let mut by_page: HashMap<PageIndex, Vec<&str>> = HashMap::new();
    for gone in removed {
        by_page.entry(gone.page).or_default().push(&gone.nm);
    }
    for (page, names) in by_page {
        remove_annotations(&mut update, page_id(page)?, &names)?;
    }

    for &page in viewport_pages {
        let id = page_id(page)?;
        let mut entries = Vec::new();
        for v in scales.viewports(page) {
            let b = v.bbox;
            let mut entry = dictionary! {
                "Type" => "Viewport",
                "BBox" => reals([b.min.x, b.min.y, b.max.x, b.max.y]),
                "Name" => text(&v.name),
                "Measure" => measures.reference(&mut update, v.scale)?,
                "NM" => text(&v.id.to_nm()),
            };
            if v.whole_page {
                entry.set("KPDF", dictionary! { "V" => 1, "WholePage" => true });
            }
            entries.push(Object::Dictionary(entry));
        }
        update.opt_clone_object_to_new_document(id)?;
        let dict = update.new_document.get_dictionary_mut(id)?;
        if entries.is_empty() {
            dict.remove(b"VP");
        } else {
            dict.set("VP", entries);
        }
    }

    for &m in markups {
        let page = page_id(m.page)?;
        let annot = annotation(&mut update, &mut measures, m, page, now_ms)?;
        let annot = update.new_document.add_object(annot);
        push_annotation(&mut update, page, annot)?;
    }

    let mut out = Vec::new();
    update.save_to(&mut out).map_err(|e| Error::Invalid(format!("writing the update: {e}")))?;
    Ok(out)
}

fn annotation(update: &mut IncrementalDocument, measures: &mut Measures, m: &Markup, page: ObjectId, now_ms: i64) -> Result<Dictionary, Error> {
    let (subtype, intent) = subtype(m.kind, &m.geometry)?;
    let resolved = measures.scales.resolve(m.page, m.geometry.first_point(), m.scale_ref);
    let scale = resolved.map(|(s, _)| s);
    let result = quantities(m, scale);
    let display = scale.map_or((Default::default(), Default::default()), |s| (s.display, s.precision));
    let quantity = match &result {
        Ok(q) => q.text(m.kind, &display.0, display.1),
        Err(e) => Some(e.to_string()),
    };
    let label = match (m.meta.label.as_str(), &quantity) {
        ("", q) => q.clone(),
        (l, Some(q)) => Some(format!("{l}: {q}")),
        (l, None) => Some(l.to_owned()),
    };

    let look = appearance(m, quantity.as_deref());
    let mut dict = form_dict(&look);
    // A tiling pattern has to be an object of its own, so it is added here and
    // named in the form's resources.
    if !look.patterns.is_empty() {
        let mut named = Dictionary::new();
        for p in &look.patterns {
            let mut stream = Stream::new(p.dict.clone(), p.content.clone());
            let _ = stream.compress();
            let id = update.new_document.add_object(stream);
            named.set(p.name.clone(), Object::Reference(id));
        }
        if let Ok(resources) = dict.get_mut(b"Resources").and_then(Object::as_dict_mut) {
            resources.set("Pattern", named);
        }
    }
    let mut form = Stream::new(dict, look.content.clone());
    let _ = form.compress();
    let form = update.new_document.add_object(form);
    let b = look.bbox;

    let created = m.meta.created_ms.unwrap_or(now_ms);
    let mut d = dictionary! {
        "Type" => "Annot",
        "Subtype" => subtype,
        "Rect" => reals([b.min.x, b.min.y, b.max.x, b.max.y]),
        "P" => page,
        "NM" => text(&m.extras.foreign_nm.clone().unwrap_or_else(|| m.id.to_nm())),
        "T" => text(&m.meta.author),
        "Subj" => text(&m.meta.subject),
        "Contents" => text(label.as_deref().unwrap_or_default()),
        "CreationDate" => pdf_date(created),
        "M" => pdf_date(m.meta.modified_ms.unwrap_or(now_ms)),
        // Printed.
        "F" => 4,
        "C" => reals(m.style.stroke.map(f64::from)),
        "CA" => real(f64::from(m.style.opacity)),
        "AP" => dictionary! { "N" => form },
    };
    if let Some(intent) = intent {
        d.set("IT", name(intent));
    }
    let mut border = dictionary! { "Type" => "Border", "W" => real(m.style.width) };
    if m.style.dash.is_empty() {
        border.set("S", name("S"));
    } else {
        border.set("S", name("D"));
        border.set("D", reals(m.style.dash.iter().copied()));
    }
    d.set("BS", border);
    if let Some(fill) = m.style.fill {
        d.set("IC", reals(fill.map(f64::from)));
    }
    match &m.geometry {
        Geometry::Line { a, b } => d.set("L", points(&[*a, *b])),
        Geometry::Polyline { pts } | Geometry::Polygon { pts, .. } => d.set("Vertices", points(pts)),
        // A count's marks go in /KPDF /Points, below; /Vertices carries them
        // too, since a /Polygon must have it, and every viewer draws the
        // appearance in preference to it.
        Geometry::Points { pts } => d.set("Vertices", points(pts)),
        // A circle is its box.
        Geometry::Ellipse { .. } => {}
        Geometry::Ink { .. } => return Err(Error::Invalid(format!("{:?} markup with the wrong geometry", m.kind))),
    }
    if let Some((s, _)) = resolved {
        d.set("Measure", measures.reference(update, s.id)?);
    }
    d.set("KPDF", kpdf(m, &result, resolved.map(|(s, v)| v.map_or_else(|| s.id.to_nm(), |v| v.to_nm())), scale));

    // Keys read from the file that nothing here models go back as they were,
    // unless we now write that key ourselves.
    for (key, value) in &m.extras.raw {
        if !d.has(key) {
            d.set(key.clone(), crate::values::from_raw(value));
        }
    }
    Ok(d)
}

fn kpdf(
    m: &Markup,
    result: &Result<markup_model::Quantities, markup_model::QuantityError>,
    scale_ref: Option<String>,
    scale: Option<&markup_model::Scale>,
) -> Dictionary {
    let mut k = dictionary! { "V" => 1, "Kind" => name(&kind_name(m.kind)) };
    if let Ok(q) = result {
        let mut cached = Dictionary::new();
        let fields = [("Length", q.length_m), ("Area", q.area_m2), ("Perim", q.perimeter_m), ("Volume", q.volume_m3)];
        for (key, value) in fields {
            if let Some(v) = value {
                cached.set(key, real(v));
            }
        }
        if !cached.is_empty() {
            cached.set("Unit", name("m"));
            k.set("Q", cached);
        }
    }
    if let Some(r) = scale_ref {
        k.set("ScaleRef", text(&r));
    }
    if m.scale_ref != ScaleRef::Page {
        k.set("Override", true);
    }
    if let Some(depth) = m.extras.depth_m {
        k.set("Depth", real(depth));
    }
    if let Some(slope) = m.extras.slope {
        k.set("Slope", reals([slope.rise, slope.run]));
    }
    if let Geometry::Polygon { holes, .. } = &m.geometry {
        if !holes.is_empty() {
            k.set("Holes", holes.iter().map(|h| points(h)).collect::<Vec<_>>());
        }
    }
    if let Geometry::Points { pts } = &m.geometry {
        k.set("Points", points(pts));
    }
    // A circle's own box: /Rect is grown to hold the line's width and the
    // label, so reading the circle back off it would make it bigger every
    // time.
    if let Geometry::Ellipse { rect } = &m.geometry {
        k.set("Box", reals([rect.min.x, rect.min.y, rect.max.x, rect.max.y]));
    }
    let texts = [("Name", Some(&m.meta.name)), ("Label", Some(&m.meta.label)), ("Item", m.meta.item_code.as_ref()), ("Status", m.meta.status.as_ref()), ("Layer", m.meta.layer.as_ref()), ("Group", m.extras.group.as_ref())];
    for (key, value) in texts {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            k.set(key, text(v));
        }
    }
    if m.style.width_unit == WidthUnit::ScreenPixels {
        k.set("WidthUnit", name("Px"));
    }
    if m.style.label_size != markup_model::Style::default().label_size {
        k.set("LabelSize", real(m.style.label_size));
    }
    // The inside's own transparency and pattern: /CA is the whole
    // annotation's, so a fill fainter than its outline has nowhere standard to
    // go. Written only when they differ from the line, so a plain shape costs
    // nothing.
    if (m.style.fill_opacity - m.style.opacity).abs() > f32::EPSILON {
        k.set("FillOpacity", real(f64::from(m.style.fill_opacity)));
    }
    if m.style.pattern.is_ruled() {
        k.set("Pattern", name(m.style.pattern.label()));
    }
    if !m.meta.custom.is_empty() {
        let custom: Dictionary = m
            .meta
            .custom
            .iter()
            .map(|(key, v)| {
                let value = match v {
                    MetaValue::Text(t) => text(t),
                    MetaValue::Number(n) => real(*n),
                    MetaValue::Bool(b) => Object::Boolean(*b),
                };
                (key.as_bytes().to_vec(), value)
            })
            .collect();
        k.set("Custom", custom);
    }
    k.set("GeomHash", text(&geom_hash_hex(&m.geometry, scale)));
    for (key, value) in &m.extras.raw_kpdf {
        if !k.has(key) {
            k.set(key.clone(), crate::values::from_raw(value));
        }
    }
    k
}

/// Adds annotation `annot` to the end of page `page`'s /Annots in `update`.
fn push_annotation(update: &mut IncrementalDocument, page: ObjectId, annot: ObjectId) -> Result<(), Error> {
    update.opt_clone_object_to_new_document(page)?;
    // An /Annots array that's an object of its own changes there; one written
    // into the page changes with the page.
    let own_object = update.new_document.get_dictionary(page)?.get(b"Annots").ok().and_then(|a| a.as_reference().ok());
    let annots = match own_object {
        Some(array) => {
            update.opt_clone_object_to_new_document(array)?;
            update.new_document.get_object_mut(array)?.as_array_mut()?
        }
        None => {
            let page = update.new_document.get_dictionary_mut(page)?;
            if !page.has(b"Annots") {
                page.set("Annots", Vec::<Object>::new());
            }
            page.get_mut(b"Annots")?.as_array_mut()?
        }
    };
    annots.push(annot.into());
    Ok(())
}

/// Takes the annotations named `names` out of page `page`'s /Annots.
fn remove_annotations(update: &mut IncrementalDocument, page: ObjectId, names: &[&str]) -> Result<(), Error> {
    update.opt_clone_object_to_new_document(page)?;
    let own_object = update.new_document.get_dictionary(page)?.get(b"Annots").ok().and_then(|a| a.as_reference().ok());
    if let Some(array) = own_object {
        update.opt_clone_object_to_new_document(array)?;
    }
    // Which entries to drop, read before the array is borrowed to change it.
    let annots = match own_object {
        Some(array) => update.new_document.get_object(array)?.as_array()?.clone(),
        None => update.new_document.get_dictionary(page)?.get(b"Annots").and_then(Object::as_array).cloned().unwrap_or_default(),
    };
    let dropped: Vec<usize> = annots
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            let dict = match entry {
                Object::Reference(id) => update.new_document.get_dictionary(*id).ok().or_else(|| update.get_prev_documents().get_dictionary(*id).ok()),
                Object::Dictionary(d) => Some(d),
                _ => None,
            };
            dict.and_then(|d| d.get(b"NM").ok()).and_then(|nm| pdf_content::lopdf::decode_text_string(nm).ok()).is_some_and(|nm| names.contains(&nm.as_str()))
        })
        .map(|(i, _)| i)
        .collect();
    if dropped.is_empty() {
        return Ok(());
    }
    let kept: Vec<Object> = annots.into_iter().enumerate().filter(|(i, _)| !dropped.contains(i)).map(|(_, entry)| entry).collect();
    match own_object {
        Some(array) => *update.new_document.get_object_mut(array)? = Object::Array(kept),
        None => update.new_document.get_dictionary_mut(page)?.set("Annots", kept),
    }
    Ok(())
}
