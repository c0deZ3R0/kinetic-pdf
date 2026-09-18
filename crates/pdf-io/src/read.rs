//! Reading measurement markups, viewports and scales back from a PDF: ours in
//! full, and standard ISO dimension annotations from other programs as far as
//! the standard goes. Other vendors' own structures come with import
//! (milestone 9).

use std::collections::HashMap;

use markup_model::hash::geom_hash_hex;
use markup_model::markup::{Extras, MarkupMeta, MetaValue, Style, WidthUnit};
use markup_model::{Geometry, Markup, MarkupId, MarkupKind, PageIndex, Pt, Rect, ScaleId, ScaleRef, ScaleStore, Slope, Viewport, ViewportId};
use pdf_content::lopdf::{Dictionary, Document, Object, ObjectId};

use crate::measure::read_measure;
use crate::values::{get, number, numbers, parse_pdf_date, read_name, read_points, read_text, resolve, to_raw, unknown_entries};
use crate::write::kind_name;
use crate::Error;

/// What was read.
#[derive(Default)]
pub struct Read {
    pub markups: Vec<Markup>,
    pub scales: ScaleStore,
    /// Measurement annotations that couldn't be read, and why.
    pub skipped: Vec<(PageIndex, String)>,
}

/// Annotation keys this reads into the model. Everything else is kept in
/// `Extras::raw`. /P and /Parent aren't kept, since they point into this file.
const ANNOT_KEYS: &[&[u8]] = &[
    b"Type", b"Subtype", b"IT", b"Rect", b"P", b"Parent", b"NM", b"T", b"Subj", b"Contents", b"CreationDate", b"M", b"F", b"C", b"IC", b"CA", b"BS",
    b"AP", b"L", b"Vertices", b"Measure", b"KPDF",
];

const KPDF_KEYS: &[&[u8]] = &[
    b"V", b"Kind", b"Q", b"ScaleRef", b"Override", b"Depth", b"Slope", b"Holes", b"Points", b"Box", b"Label", b"Item", b"Status", b"Layer", b"Group",
    b"WidthUnit", b"LabelSize", b"Custom", b"GeomHash",
];

/// Scales already read, by the object or contents they came from.
#[derive(Default)]
struct Scales {
    store: ScaleStore,
    by_object: HashMap<ObjectId, ScaleId>,
}

impl Scales {
    /// The scale for a /Measure entry, read once per object, and merged with
    /// an identical scale read before.
    fn of(&mut self, doc: &Document, entry: &Object) -> Result<ScaleId, Error> {
        let object = entry.as_reference().ok();
        if let Some(id) = object.and_then(|o| self.by_object.get(&o)) {
            return Ok(*id);
        }
        let dict = resolve(doc, entry).and_then(|o| o.as_dict().ok()).ok_or_else(|| Error::Invalid("/Measure isn't a dictionary".into()))?;
        let scale = read_measure(doc, dict)?;
        let id = match self.store.scale(scale.id) {
            Some(_) => scale.id,
            None => match self.store.find_same(&scale) {
                // A copy of a /Measure written per annotation, as some programs do.
                Some(same) if object.is_none() => same,
                _ => {
                    let id = scale.id;
                    self.store.set_scale(scale);
                    id
                }
            },
        };
        if let Some(o) = object {
            self.by_object.insert(o, id);
        }
        Ok(id)
    }
}

/// The page's crop box, or media box, inherited from its parents if need be.
fn page_box(doc: &Document, page: ObjectId) -> Option<Rect> {
    let mut id = Some(page);
    let mut seen = 0;
    for key in [b"CropBox".as_slice(), b"MediaBox"] {
        while let Some(current) = id {
            let dict = doc.get_dictionary(current).ok()?;
            if let Some(v) = dict.get(key).ok().and_then(|o| numbers(doc, o)).filter(|v| v.len() == 4) {
                return Some(Rect::from_corners(Pt::new(v[0], v[1]), Pt::new(v[2], v[3])));
            }
            id = dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
            seen += 1;
            if seen > 64 {
                return None;
            }
        }
        id = Some(page);
    }
    None
}

pub fn read(doc: &Document) -> Read {
    let mut scales = Scales::default();
    let mut read = Read::default();
    for (&number, &page_id) in &doc.get_pages() {
        let page = number - 1;
        let Ok(page_dict) = doc.get_dictionary(page_id) else { continue };
        let crop = page_box(doc, page_id);
        if let Some(vps) = get(doc, page_dict, b"VP").and_then(|o| o.as_array().ok()) {
            for entry in vps {
                match viewport(doc, &mut scales, page, crop, entry) {
                    Ok(v) => {
                        scales.store.set_viewport(v);
                    }
                    Err(e) => read.skipped.push((page, format!("viewport: {e}"))),
                }
            }
        }
        let Some(annots) = get(doc, page_dict, b"Annots").and_then(|o| o.as_array().ok()) else { continue };
        for entry in annots {
            let Some(dict) = resolve(doc, entry).and_then(|o| o.as_dict().ok()) else { continue };
            if !is_measurement(doc, dict) {
                continue;
            }
            match markup(doc, &mut scales, page, dict) {
                Ok(m) => read.markups.push(m),
                Err(e) => read.skipped.push((page, e.to_string())),
            }
        }
    }
    read.scales = scales.store;
    // Now every viewport is known, a markup whose /Measure is the one its page
    // would give it follows the page; hashes are checked against that scale.
    for m in &mut read.markups {
        settle(&read.scales, m);
    }
    read
}

fn viewport(doc: &Document, scales: &mut Scales, page: PageIndex, crop: Option<Rect>, entry: &Object) -> Result<Viewport, Error> {
    let dict = resolve(doc, entry).and_then(|o| o.as_dict().ok()).ok_or_else(|| Error::Invalid("not a dictionary".into()))?;
    let bbox = dict.get(b"BBox").ok().and_then(|o| numbers(doc, o)).filter(|v| v.len() == 4).ok_or_else(|| Error::Invalid("no /BBox".into()))?;
    let bbox = Rect::from_corners(Pt::new(bbox[0], bbox[1]), Pt::new(bbox[2], bbox[3]));
    let scale = scales.of(doc, dict.get(b"Measure").map_err(|_| Error::Invalid("no /Measure".into()))?)?;
    let ours = get(doc, dict, b"KPDF").and_then(|o| o.as_dict().ok());
    let whole_page = match ours.and_then(|k| get(doc, k, b"WholePage")) {
        Some(flag) => flag.as_bool().unwrap_or(false),
        None => crop.is_some_and(|c| bbox.expand(0.5).contains_rect(&c)),
    };
    Ok(Viewport {
        id: read_text(doc, dict, b"NM").and_then(|nm| ViewportId::from_nm(&nm)).unwrap_or_default(),
        page,
        bbox,
        name: read_text(doc, dict, b"Name").unwrap_or_default(),
        scale,
        whole_page,
    })
}

fn is_measurement(doc: &Document, dict: &Dictionary) -> bool {
    // Ours say so; anything else has to carry a standard measurement intent.
    dict.has(b"KPDF") || matches!(read_name(doc, dict, b"IT"), Some(b"LineDimension" | b"PolyLineDimension" | b"PolygonDimension"))
}

fn kind_from(doc: &Document, dict: &Dictionary, kpdf: Option<&Dictionary>) -> Option<MarkupKind> {
    use MarkupKind::*;
    if let Some(kind) = kpdf.and_then(|k| read_name(doc, k, b"Kind")) {
        let all = [Length, Polylength, Area, Perimeter, Count, Angle, Radius, Diameter, Volume, Text, Cloud, Highlight, Pen, Box, Ellipse, Arrow];
        return all.into_iter().find(|k| kind_name(*k).as_bytes() == kind);
    }
    match read_name(doc, dict, b"IT")? {
        b"LineDimension" => Some(Length),
        b"PolyLineDimension" => Some(Polylength),
        b"PolygonDimension" => Some(Area),
        _ => None,
    }
}

fn colour(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<[f32; 3]> {
    let v = dict.get(key).ok().and_then(|o| numbers(doc, o))?;
    match v[..] {
        [r, g, b] => Some([r as f32, g as f32, b as f32]),
        [grey] => Some([grey as f32; 3]),
        _ => None,
    }
}

fn markup(doc: &Document, scales: &mut Scales, page: PageIndex, dict: &Dictionary) -> Result<Markup, Error> {
    let kpdf = get(doc, dict, b"KPDF").and_then(|o| o.as_dict().ok());
    let kind = kind_from(doc, dict, kpdf).ok_or_else(|| Error::Unsupported("a markup kind this doesn't know".into()))?;
    let points_of = |key: &[u8]| dict.get(key).ok().and_then(|o| read_points(doc, o));
    let geometry = match kind {
        MarkupKind::Length => match points_of(b"L").as_deref() {
            Some([a, b, ..]) => Geometry::Line { a: *a, b: *b },
            _ => return Err(Error::Invalid("a length without /L".into())),
        },
        MarkupKind::Polylength => Geometry::Polyline { pts: points_of(b"Vertices").ok_or_else(|| Error::Invalid("no /Vertices".into()))? },
        MarkupKind::Area | MarkupKind::Perimeter | MarkupKind::Volume => {
            let holes = kpdf
                .and_then(|k| get(doc, k, b"Holes"))
                .and_then(|o| o.as_array().ok())
                .map(|rings| rings.iter().filter_map(|r| read_points(doc, r)).collect())
                .unwrap_or_default();
            Geometry::Polygon { pts: points_of(b"Vertices").ok_or_else(|| Error::Invalid("no /Vertices".into()))?, holes }
        }
        MarkupKind::Count => {
            // A count's marks are its own, in /KPDF; /Vertices carries them too
            // for anything that only knows the standard.
            let marks = kpdf.and_then(|k| k.get(b"Points").ok()).and_then(|o| read_points(doc, o));
            Geometry::Points { pts: marks.or_else(|| points_of(b"Vertices")).unwrap_or_default() }
        }
        MarkupKind::Angle => Geometry::Polyline { pts: points_of(b"Vertices").ok_or_else(|| Error::Invalid("no /Vertices".into()))? },
        MarkupKind::Radius | MarkupKind::Diameter => {
            // Our own box before the annotation's, which is grown to hold the
            // line's width and the label.
            let box_ = kpdf.and_then(|k| k.get(b"Box").ok()).and_then(|o| numbers(doc, o)).and_then(|v| match v[..] {
                [x1, y1, x2, y2] => Some(Rect::from_corners(Pt::new(x1, y1), Pt::new(x2, y2))),
                _ => None,
            });
            match (points_of(b"L"), points_of(b"Vertices"), box_.or_else(|| rect_of(doc, dict))) {
                (Some(line), _, _) if line.len() >= 2 => Geometry::Line { a: line[0], b: line[1] },
                (_, Some(pts), _) if !pts.is_empty() => Geometry::Polyline { pts },
                (_, _, Some(rect)) => Geometry::Ellipse { rect },
                _ => return Err(Error::Invalid("a radius or diameter without a shape".into())),
            }
        }
        other => return Err(Error::Unsupported(format!("reading {other:?} markups"))),
    };

    let nm = read_text(doc, dict, b"NM");
    let (id, foreign_nm) = match nm.as_deref().and_then(MarkupId::from_nm) {
        Some(id) => (id, None),
        None => (MarkupId::new(), nm),
    };
    let scale_ref = match dict.get(b"Measure") {
        Ok(entry) => ScaleRef::Override(scales.of(doc, entry)?),
        Err(_) => ScaleRef::Page,
    };
    let text_of = |key: &[u8]| kpdf.and_then(|k| read_text(doc, k, key));
    let style_default = Style::default();
    let border = get(doc, dict, b"BS").and_then(|o| o.as_dict().ok());
    let style = Style {
        stroke: colour(doc, dict, b"C").unwrap_or(style_default.stroke),
        fill: colour(doc, dict, b"IC"),
        opacity: number(doc, dict, b"CA").map_or(1.0, |v| v as f32),
        width: border.and_then(|b| number(doc, b, b"W")).unwrap_or(1.0),
        width_unit: if kpdf.and_then(|k| read_name(doc, k, b"WidthUnit")) == Some(b"Px") { WidthUnit::ScreenPixels } else { WidthUnit::Points },
        dash: border.and_then(|b| b.get(b"D").ok()).and_then(|d| numbers(doc, d)).unwrap_or_default(),
        label_size: kpdf.and_then(|k| number(doc, k, b"LabelSize")).unwrap_or(style_default.label_size),
    };
    let custom = kpdf
        .and_then(|k| get(doc, k, b"Custom"))
        .and_then(|o| o.as_dict().ok())
        .map(|c| {
            c.iter()
                .filter_map(|(key, v)| {
                    let value = match resolve(doc, v)? {
                        Object::Boolean(b) => MetaValue::Bool(*b),
                        Object::Integer(_) | Object::Real(_) => MetaValue::Number(crate::values::number_of(doc, v)?),
                        s @ Object::String(..) => MetaValue::Text(pdf_content::lopdf::decode_text_string(s).ok()?),
                        _ => return None,
                    };
                    Some((String::from_utf8_lossy(key).into_owned(), value))
                })
                .collect()
        })
        .unwrap_or_default();
    let date = |key: &[u8]| read_text(doc, dict, key).and_then(|d| parse_pdf_date(&d));
    let meta = MarkupMeta {
        label: text_of(b"Label").unwrap_or_default(),
        subject: read_text(doc, dict, b"Subj").unwrap_or_default(),
        author: read_text(doc, dict, b"T").unwrap_or_default(),
        created_ms: date(b"CreationDate"),
        modified_ms: date(b"M"),
        layer: text_of(b"Layer"),
        status: text_of(b"Status"),
        item_code: text_of(b"Item"),
        custom,
    };
    let slope = kpdf.and_then(|k| k.get(b"Slope").ok()).and_then(|o| numbers(doc, o)).and_then(|v| match v[..] {
        [rise, run] => Some(Slope { rise, run }),
        _ => None,
    });
    let mut raw_kpdf = kpdf.map(|k| unknown_entries(k, KPDF_KEYS)).unwrap_or_default();
    // Kept for `settle`, which takes them out again: they aren't unknown keys.
    for key in [b"GeomHash".as_slice(), b"Override"] {
        if let Some(value) = kpdf.and_then(|k| k.get(key).ok()) {
            raw_kpdf.insert(key.to_vec(), to_raw(value));
        }
    }
    let extras = Extras {
        depth_m: kpdf.and_then(|k| number(doc, k, b"Depth")),
        slope,
        group: text_of(b"Group"),
        foreign_nm,
        changed_externally: false,
        raw: unknown_entries(dict, ANNOT_KEYS),
        raw_kpdf,
    };
    Ok(Markup { id, page, kind, geometry, style, meta, scale_ref, extras })
}

/// Turns an override that matches what the page gives into following the
/// page, and flags a markup whose geometry no longer matches its hash.
fn settle(scales: &ScaleStore, m: &mut Markup) {
    let chosen = m.extras.raw_kpdf.remove(b"Override".as_slice()) == Some(markup_model::markup::RawValue::Bool(true));
    if let (ScaleRef::Override(id), false) = (m.scale_ref, chosen) {
        let from_page = scales.resolve(m.page, m.geometry.first_point(), ScaleRef::Page).map(|(s, _)| s.id);
        if from_page == Some(id) {
            m.scale_ref = ScaleRef::Page;
        }
    }
    if let Some(markup_model::markup::RawValue::String(hash)) = m.extras.raw_kpdf.remove(b"GeomHash".as_slice()) {
        let scale = scales.resolve(m.page, m.geometry.first_point(), m.scale_ref).map(|(s, _)| s);
        m.extras.changed_externally = hash != geom_hash_hex(&m.geometry, scale).into_bytes();
    }
}

/// An annotation's /Rect as a box in user space.
fn rect_of(doc: &Document, dict: &Dictionary) -> Option<Rect> {
    let v = dict.get(b"Rect").ok().and_then(|o| numbers(doc, o))?;
    match v[..] {
        [x1, y1, x2, y2] => Some(Rect::from_corners(Pt::new(x1, y1), Pt::new(x2, y2))),
        _ => None,
    }
}
