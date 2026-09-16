//! Markups made with the drawing tools -- pen strokes, rectangles, ellipses,
//! lines and arrows -- their shapes, and writing them into a PDF.
//!
//! Each is written as the annotation Acrobat and Bluebeam write for it (Ink,
//! Square, Circle or Line) with an appearance stream, so every viewer draws it
//! the same way, the app's own GPU drawing included. They're appended to the
//! file as an incremental update with lopdf: pdfium can't create lines, and
//! the appearances it makes for the rest don't look like other programs'.

use chrono::Utc;
use pdf_content::lopdf::{self, dictionary, Dictionary, Document, IncrementalDocument, Object, ObjectId, Stream, StringFormat};

use crate::model::{AnnotKey, Markup, MarkupKind, PdfBox, Rgb};

/// The colour shown for a markup read from a file, whose own colour pdfium
/// won't report while it has an appearance.
pub const DEFAULT_COLOR: Rgb = [0.86, 0.15, 0.15];

/// Control points this far along a quarter ellipse's tangents make a Bezier
/// curve within 0.03% of it.
const KAPPA: f32 = 0.552_284_8;

impl MarkupKind {
    /// The drawing tools, in the order the toolbar shows them.
    pub const TOOLS: [MarkupKind; 5] = [MarkupKind::Pen, MarkupKind::Rectangle, MarkupKind::Ellipse, MarkupKind::Line, MarkupKind::Arrow];

    pub fn label(self) -> &'static str {
        match self {
            MarkupKind::Pen => "Pen",
            MarkupKind::Rectangle => "Rectangle",
            MarkupKind::Ellipse => "Ellipse",
            MarkupKind::Line => "Line",
            MarkupKind::Arrow => "Arrow",
            MarkupKind::Other => "Markup",
        }
    }

    /// The annotation subtype it's written as.
    fn subtype(self) -> &'static str {
        match self {
            MarkupKind::Pen => "Ink",
            MarkupKind::Rectangle => "Square",
            MarkupKind::Ellipse => "Circle",
            MarkupKind::Line | MarkupKind::Arrow => "Line",
            MarkupKind::Other => "Polygon",
        }
    }
}

impl PdfBox {
    /// The box with corners `a` and `b`.
    pub fn spanning(a: [f32; 2], b: [f32; 2]) -> PdfBox {
        PdfBox { left: a[0].min(b[0]), bottom: a[1].min(b[1]), right: a[0].max(b[0]), top: a[1].max(b[1]) }
    }
}

/// The ends of an arrow's head at `to`, pointing away from `from`: two
/// points back along the line, either side of it, for a stroke `width` wide.
pub fn arrow_head(from: [f32; 2], to: [f32; 2], width: f32) -> [[f32; 2]; 2] {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let length = dx.hypot(dy).max(f32::EPSILON);
    let (x, y) = (dx / length, dy / length);
    // Each side 30 degrees off the line.
    let size = (width * 4.0).max(8.0);
    let (back, side) = (size * 0.866, size * 0.5);
    [[to[0] - x * back - y * side, to[1] - y * back + x * side], [to[0] - x * back + y * side, to[1] - y * back - x * side]]
}

/// The box a markup of `kind` through `points` covers with a stroke `width`
/// wide: its points, an arrow's head, and half the stroke around them.
pub fn bounds(kind: MarkupKind, points: &[[f32; 2]], width: f32) -> PdfBox {
    let mut all = points.to_vec();
    if let (MarkupKind::Arrow, [from, to, ..]) = (kind, points) {
        all.extend(arrow_head(*from, *to, width));
    }
    let first = all.first().copied().unwrap_or_default();
    let spanned = all.iter().fold(PdfBox::spanning(first, first), |b, &p| {
        let with = PdfBox::spanning(p, p);
        PdfBox { left: b.left.min(with.left), bottom: b.bottom.min(with.bottom), right: b.right.max(with.right), top: b.top.max(with.top) }
    });
    let half = width / 2.0;
    PdfBox { left: spanned.left - half, bottom: spanned.bottom - half, right: spanned.right + half, top: spanned.top + half }
}

/// Whether a drag through `points` makes a markup of `kind` rather than a
/// slip of the mouse: it reaches `least` points across, both ways for a box.
pub fn is_drawn(kind: MarkupKind, points: &[[f32; 2]], least: f32) -> bool {
    let reach = bounds(MarkupKind::Line, points, 0.0);
    match kind {
        MarkupKind::Rectangle | MarkupKind::Ellipse => reach.width() >= least && reach.height() >= least,
        _ => points.len() >= 2 && reach.width().max(reach.height()) >= least,
    }
}

/// `markups`, by `author`, appended to the PDF `bytes` as an incremental
/// update, and where each went, in order.
pub fn append(bytes: Vec<u8>, markups: &[Markup], author: &str) -> Result<(Vec<u8>, Vec<AnnotKey>), String> {
    if markups.is_empty() {
        return Ok((bytes, Vec::new()));
    }
    let previous = Document::load_mem(&bytes).map_err(|e| e.to_string())?;
    let pages = previous.get_pages();
    let mut update = IncrementalDocument::create_from(bytes, previous);
    let now = Utc::now();
    let date = Object::string_literal(now.format("D:%Y%m%d%H%M%SZ").to_string());

    let mut keys = Vec::with_capacity(markups.len());
    for (i, m) in markups.iter().enumerate() {
        let page = *pages.get(&(m.page as u32 + 1)).ok_or_else(|| format!("there's no page {}", m.page + 1))?;
        let b = m.bounds;
        let rect = numbers(&[b.left, b.bottom, b.right, b.top]);
        let form = dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => rect.clone(), "Resources" => Dictionary::new() };
        let mut form = Stream::new(form, appearance(m));
        let _ = form.compress();
        let form = update.new_document.add_object(form);

        let mut annot = dictionary! {
            "Type" => "Annot",
            "Subtype" => m.kind.subtype(),
            "Rect" => rect,
            "P" => page,
            "NM" => text(&format!("kinetic-pdf-{}-{i}", now.timestamp_micros())),
            "T" => text(author),
            "Contents" => text(&m.comment),
            "CreationDate" => date.clone(),
            "M" => date.clone(),
            // Printed.
            "F" => 4_i64,
            "C" => numbers(&m.color),
            "BS" => dictionary! { "W" => m.width, "S" => "S" },
            "AP" => dictionary! { "N" => form },
        };
        match (m.kind, &m.points[..]) {
            (MarkupKind::Pen, points) => annot.set("InkList", vec![numbers(points.as_flattened())]),
            (MarkupKind::Line | MarkupKind::Arrow, [from, to, ..]) => {
                annot.set("L", numbers(&[from[0], from[1], to[0], to[1]]));
                let end = if m.kind == MarkupKind::Arrow { "OpenArrow" } else { "None" };
                annot.set("LE", vec![Object::from("None"), Object::from(end)]);
            }
            _ => {}
        }
        let annot = update.new_document.add_object(annot);
        let index = push_annotation(&mut update, page, annot).map_err(|e| e.to_string())?;
        keys.push(AnnotKey { page: m.page, index });
    }

    let mut out = Vec::new();
    update.save_to(&mut out).map_err(|e| e.to_string())?;
    Ok((out, keys))
}

/// Adds annotation `annot` to the end of page `page`'s /Annots in `update`,
/// and says where in it it went.
fn push_annotation(update: &mut IncrementalDocument, page: ObjectId, annot: ObjectId) -> lopdf::Result<usize> {
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
    Ok(annots.len() - 1)
}

/// The content stream drawing a markup, in PDF user space.
fn appearance(m: &Markup) -> Vec<u8> {
    let [r, g, b] = m.color.map(number);
    let mut ops = format!("{r} {g} {b} RG {} w 1 J 1 j\n", number(m.width));
    match (m.kind, &m.points[..]) {
        (MarkupKind::Rectangle, [a, b, ..]) => {
            let q = PdfBox::spanning(*a, *b);
            ops += &format!("{} {} re S\n", point([q.left, q.bottom]), point([q.width(), q.height()]));
        }
        (MarkupKind::Ellipse, [a, b, ..]) => ops += &ellipse(PdfBox::spanning(*a, *b)),
        (MarkupKind::Arrow, [from, to, ..]) => {
            let [left, right] = arrow_head(*from, *to, m.width);
            ops += &polyline(&[*from, *to]);
            ops += &polyline(&[left, *to, right]);
        }
        (_, points) => ops += &polyline(points),
    }
    ops.into_bytes()
}

/// A stroked path through `points`.
fn polyline(points: &[[f32; 2]]) -> String {
    let Some((first, rest)) = points.split_first() else { return String::new() };
    let mut path = format!("{} m", point(*first));
    for p in rest {
        path += &format!(" {} l", point(*p));
    }
    path + " S\n"
}

/// A stroked ellipse filling `q`, as four Bezier curves.
fn ellipse(q: PdfBox) -> String {
    let (x, y) = q.center();
    let (rx, ry) = (q.width() / 2.0, q.height() / 2.0);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let curve = |c1: [f32; 2], c2: [f32; 2], end: [f32; 2]| format!(" {} {} {} c", point(c1), point(c2), point(end));
    let mut path = format!("{} m", point([x + rx, y]));
    path += &curve([x + rx, y + ky], [x + kx, y + ry], [x, y + ry]);
    path += &curve([x - kx, y + ry], [x - rx, y + ky], [x - rx, y]);
    path += &curve([x - rx, y - ky], [x - kx, y - ry], [x, y - ry]);
    path += &curve([x + kx, y - ry], [x + rx, y - ky], [x + rx, y]);
    path + " S\n"
}

/// A number as a content stream writes it, to a hundredth of a point.
fn number(v: f32) -> String {
    let fixed = format!("{v:.2}");
    fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn point([x, y]: [f32; 2]) -> String {
    format!("{} {}", number(x), number(y))
}

fn numbers(values: &[f32]) -> Object {
    Object::Array(values.iter().map(|&v| Object::Real(v)).collect())
}

/// A text string: as it is when it's ASCII, otherwise in UTF-16.
fn text(s: &str) -> Object {
    if s.is_ascii() {
        return Object::String(s.as_bytes().to_vec(), StringFormat::Literal);
    }
    let mut bytes = vec![0xfe, 0xff];
    bytes.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
    Object::String(bytes, StringFormat::Hexadecimal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_content::fixtures::placed_stamp_pdf;

    fn markup(kind: MarkupKind, points: Vec<[f32; 2]>) -> Markup {
        Markup {
            key: None,
            page: 0,
            kind,
            bounds: bounds(kind, &points, 1.0),
            points,
            color: [1.0, 0.0, 0.0],
            width: 1.0,
            comment: "a note".to_owned(),
            author: String::new(),
        }
    }

    #[test]
    fn a_box_covers_its_stroke_and_an_arrow_its_head() {
        let b = bounds(MarkupKind::Rectangle, &[[10.0, 20.0], [30.0, 5.0]], 2.0);
        assert_eq!([b.left, b.bottom, b.right, b.top], [9.0, 4.0, 31.0, 21.0]);
        let arrow = bounds(MarkupKind::Arrow, &[[0.0, 0.0], [100.0, 0.0]], 1.0);
        assert!(arrow.top > 3.0 && arrow.bottom < -3.0 && arrow.right == 100.5, "the head spreads either side: {arrow:?}");
    }

    #[test]
    fn a_click_is_not_a_markup() {
        assert!(!is_drawn(MarkupKind::Rectangle, &[[5.0, 5.0], [5.2, 40.0]], 1.0), "a box needs both sides");
        assert!(is_drawn(MarkupKind::Line, &[[5.0, 5.0], [5.0, 40.0]], 1.0));
        assert!(!is_drawn(MarkupKind::Pen, &[[5.0, 5.0]], 1.0));
    }

    #[test]
    fn markups_go_after_the_annotations_already_there_in_an_update() {
        let original = placed_stamp_pdf();
        let drawn = [markup(MarkupKind::Rectangle, vec![[1.0, 1.0], [4.0, 3.0]]), markup(MarkupKind::Arrow, vec![[5.0, 5.0], [9.0, 9.0]])];
        let (bytes, keys) = append(original.clone(), &drawn, "tester").unwrap();
        assert_eq!(keys, [AnnotKey { page: 0, index: 1 }, AnnotKey { page: 0, index: 2 }]);
        assert!(bytes.starts_with(&original), "the file as it was comes first");

        let doc = Document::load_mem(&bytes).unwrap();
        let page = doc.get_dictionary(doc.get_pages()[&1]).unwrap();
        let subtypes: Vec<&[u8]> = page
            .get(b"Annots")
            .and_then(Object::as_array)
            .unwrap()
            .iter()
            .map(|a| doc.get_dictionary(a.as_reference().unwrap()).unwrap().get(b"Subtype").and_then(Object::as_name).unwrap())
            .collect();
        assert_eq!(subtypes, [b"Stamp".as_slice(), b"Square", b"Line"]);
    }

    #[test]
    fn a_markup_draws_from_its_appearance_where_it_was_made() {
        let (bytes, _) = append(placed_stamp_pdf(), &[markup(MarkupKind::Rectangle, vec![[2.0, 2.0], [6.0, 6.0]])], "tester").unwrap();
        let shapes = gpu_lines::annotation_shapes(&Document::load_mem(&bytes).unwrap(), 1, 0.05, gpu_lines::MOST_IMAGE_DENSITY).unwrap();
        let red: Vec<_> = shapes.primitives.iter().filter(|p| shapes.style_of(p).colour == [1.0, 0.0, 0.0, 1.0]).collect();
        assert!(red.len() >= 4, "four sides at least: {red:?}");
        let inside = |v: f32| (1.5..=6.5).contains(&v);
        assert!(red.iter().all(|p| p.points[..2].iter().all(|&[x, y]| inside(x) && inside(y))), "{red:?}");
    }
}
