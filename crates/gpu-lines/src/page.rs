//! The annotations shown on a page, each appearance placed where PDF says it
//! goes, drawn into shapes.

use std::collections::HashSet;

use pdf_content::lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use pdf_content::objects::{dict, number};

use crate::geometry::Matrix;
use crate::interpret::Interpreter;
use crate::pdf::{matrix, rectangle};
use crate::shapes::Shapes;

/// Annotation kinds left out: highlights, which the app draws itself, and
/// popups, which only show when opened.
const LEFT_OUT: [&[u8]; 2] = [b"Highlight", b"Popup"];

/// Annotation flags that keep one off screen: Hidden and NoView.
const OFF_SCREEN: i64 = (1 << 1) | (1 << 5);

/// Page tree entries a page inherits, looked up at most this far up.
const DEEPEST_TREE: usize = 32;

/// Pixels an image needs before it's decoded ahead of drawing on a thread of
/// its own; see `images_ahead`.
const WORTH_DECODING_AHEAD: u64 = 250_000;

/// Forms inside forms are followed this far looking for images, as
/// `Interpreter` draws them.
const DEEPEST_FORMS: usize = 16;

/// The shapes of every annotation shown on page `page_number` (from 1), in
/// points on the page as it's displayed: the origin at the bottom left of its
/// visible area -- its crop box within its media box, as pdfium takes it --
/// turned by its `/Rotate`. Curves are flattened to within `tolerance` points.
pub fn annotation_shapes(doc: &Document, page_number: u32, tolerance: f32) -> Result<Shapes, String> {
    let (page_id, to_page) = placed_page(doc, page_number)?;
    let mut interpreter = Interpreter::new(doc, tolerance);
    let ahead = images_ahead(doc, &interpreter, page_id, false);
    interpreter.decode_ahead(&ahead);
    draw_annotations(&mut interpreter, page_id, to_page);
    Ok(interpreter.shapes)
}

/// The shapes of page `page_number`'s own content, then of its annotations
/// shown over it: everything the page draws, placed as `annotation_shapes`
/// places them.
pub fn page_shapes(doc: &Document, page_number: u32, tolerance: f32) -> Result<Shapes, String> {
    let (page_id, to_page) = placed_page(doc, page_number)?;
    let mut interpreter = Interpreter::new(doc, tolerance);
    let ahead = images_ahead(doc, &interpreter, page_id, true);
    interpreter.decode_ahead(&ahead);
    // Every content stream of the page, decoded and joined.
    let content = doc.get_page_content(page_id);
    let resources = inherited(doc, page_id, b"Resources").and_then(|r| dict(doc, r));
    interpreter.draw(&content, resources, to_page);
    draw_annotations(&mut interpreter, page_id, to_page);
    Ok(interpreter.shapes)
}

/// Page `page_number`'s object, and the matrix from its user space to the page
/// as displayed: the origin at the bottom left of its visible area -- its crop
/// box within its media box, as pdfium takes it -- turned by its `/Rotate`.
fn placed_page(doc: &Document, page_number: u32) -> Result<(ObjectId, Matrix), String> {
    let page_id = *doc.get_pages().get(&page_number).ok_or_else(|| format!("there's no page {page_number}"))?;
    let boxed = |key: &[u8]| inherited(doc, page_id, key).and_then(|b| rectangle(doc, b));
    let [left, bottom, right, top] = match (boxed(b"MediaBox"), boxed(b"CropBox")) {
        (Some(media), Some(crop)) => [media[0].max(crop[0]), media[1].max(crop[1]), media[2].min(crop[2]), media[3].min(crop[3])],
        (media, crop) => crop.or(media).unwrap_or([0.0; 4]),
    };
    let quarter_turns = inherited(doc, page_id, b"Rotate").and_then(|r| number(doc, r)).map_or(0, |r| (r / 90.0).round() as i32);
    Ok((page_id, Matrix::translate(-left, -bottom).then(rotated(quarter_turns, right - left, top - bottom))))
}

/// Draws the annotations shown on page `page_id`, each appearance placed in
/// its rectangle and then by `to_page`.
fn draw_annotations(interpreter: &mut Interpreter<'_>, page_id: ObjectId, to_page: Matrix) {
    let doc = interpreter.document();
    let Ok(page) = doc.get_dictionary(page_id) else { return };
    let annots = page.get(b"Annots").ok().and_then(|a| doc.dereference(a).ok()).and_then(|(_, a)| a.as_array().ok());
    for annot in annots.into_iter().flatten().filter_map(|a| dict(doc, a)) {
        if !is_shown(doc, interpreter, annot) {
            continue;
        }
        if let Some((appearance, placed)) = appearance(doc, annot) {
            interpreter.draw_form(appearance, None, placed.then(to_page));
        }
    }
}

/// The images worth decoding before drawing starts (`decode_ahead`): those
/// the page's own content can draw, with `with_content`, and those in the
/// appearances of the annotations shown on it. Small ones are left for drawing
/// to decode as it reaches them, since handing one to another thread costs
/// more than decoding it.
fn images_ahead<'d>(doc: &'d Document, interpreter: &Interpreter<'d>, page_id: ObjectId, with_content: bool) -> Vec<&'d Stream> {
    let mut resources: Vec<(&'d Dictionary, usize)> = Vec::new();
    if with_content {
        resources.extend(inherited(doc, page_id, b"Resources").and_then(|r| dict(doc, r)).map(|r| (r, 0)));
    }
    if let Ok(page) = doc.get_dictionary(page_id) {
        let annots = page.get(b"Annots").ok().and_then(|a| doc.dereference(a).ok()).and_then(|(_, a)| a.as_array().ok());
        for annot in annots.into_iter().flatten().filter_map(|a| dict(doc, a)) {
            if !is_shown(doc, interpreter, annot) {
                continue;
            }
            let appearance = appearance(doc, annot).and_then(|(form, _)| form.dict.get(b"Resources").ok());
            resources.extend(appearance.and_then(|r| dict(doc, r)).map(|r| (r, 1)));
        }
    }

    let mut images = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    while let Some((next, depth)) = resources.pop() {
        let Some(xobjects) = next.get(b"XObject").ok().and_then(|x| dict(doc, x)) else { continue };
        for (_, object) in xobjects.iter() {
            let Ok((_, Object::Stream(xobject))) = doc.dereference(object) else { continue };
            if !seen.insert(xobject as *const Stream as usize) {
                continue;
            }
            match xobject.dict.get(b"Subtype").and_then(Object::as_name) {
                Ok(b"Image") => {
                    let side = |key: &[u8]| xobject.dict.get(key).ok().and_then(|v| number(doc, v)).unwrap_or(0.0) as u64;
                    let mask = xobject.dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false);
                    if !mask && side(b"Width") * side(b"Height") >= WORTH_DECODING_AHEAD {
                        images.push(xobject);
                    }
                }
                // Forms drawn inside forms, as deep as they're drawn.
                Ok(b"Form") if depth < DEEPEST_FORMS => {
                    resources.extend(xobject.dict.get(b"Resources").ok().and_then(|r| dict(doc, r)).map(|r| (r, depth + 1)));
                }
                _ => {}
            }
        }
    }
    images
}

/// The matrix turning a page's visible area, `width` by `height` from the
/// origin, `quarter_turns` quarter turns clockwise, as `/Rotate` displays it,
/// with its new bottom left at the origin.
fn rotated(quarter_turns: i32, width: f32, height: f32) -> Matrix {
    match quarter_turns.rem_euclid(4) {
        // The top edge becomes the right: (x, y) to (y, width - x).
        1 => Matrix([0.0, -1.0, 1.0, 0.0, 0.0, width]),
        2 => Matrix([-1.0, 0.0, 0.0, -1.0, width, height]),
        // The top edge becomes the left: (x, y) to (height - y, x).
        3 => Matrix([0.0, 1.0, -1.0, 0.0, height, 0.0]),
        _ => Matrix::IDENTITY,
    }
}

/// A page's entry `key`, or the nearest ancestor's in the page tree.
fn inherited<'d>(doc: &'d Document, page_id: ObjectId, key: &[u8]) -> Option<&'d Object> {
    let mut node = doc.get_dictionary(page_id).ok()?;
    for _ in 0..DEEPEST_TREE {
        if let Ok(value) = node.get(key) {
            return Some(value);
        }
        node = dict(doc, node.get(b"Parent").ok()?)?;
    }
    None
}

/// Whether an annotation shows: not a kind left out, not flagged hidden, and
/// not on a layer that's off.
fn is_shown(doc: &Document, interpreter: &Interpreter, annot: &Dictionary) -> bool {
    let kind = annot.get(b"Subtype").and_then(Object::as_name).unwrap_or_default();
    let flags = annot.get(b"F").ok().and_then(|f| number(doc, f)).unwrap_or(0.0) as i64;
    !LEFT_OUT.contains(&kind) && flags & OFF_SCREEN == 0 && annot.get(b"OC").map_or(true, |oc| interpreter.is_visible(oc))
}

/// An annotation's normal appearance -- for the state it's in, if it has
/// several -- and the matrix fitting its box, once transformed by its own
/// matrix, to the annotation's rectangle (PDF's algorithm for placing
/// appearances). The form's own matrix is applied as it's drawn.
fn appearance<'d>(doc: &'d Document, annot: &'d Dictionary) -> Option<(&'d Stream, Matrix)> {
    let normal = annot.get(b"AP").ok().and_then(|ap| dict(doc, ap))?.get(b"N").ok()?;
    let form = match doc.dereference(normal).ok()?.1 {
        Object::Stream(form) => form,
        Object::Dictionary(by_state) => {
            let state = annot.get(b"AS").ok()?.as_name().ok()?;
            doc.dereference(by_state.get(state).ok()?).ok()?.1.as_stream().ok()?
        }
        _ => return None,
    };
    let [rect_left, rect_bottom, rect_right, rect_top] = rectangle(doc, annot.get(b"Rect").ok()?)?;
    let [box_left, box_bottom, box_right, box_top] = rectangle(doc, form.dict.get(b"BBox").ok()?)?;
    let form_matrix = form.dict.get(b"Matrix").ok().and_then(|m| matrix(doc, m)).unwrap_or(Matrix::IDENTITY);

    let corners = [[box_left, box_bottom], [box_right, box_bottom], [box_left, box_top], [box_right, box_top]].map(|p| form_matrix.apply(p));
    let (left, right) = corners.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
    let (bottom, top) = corners.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
    if right - left <= 0.0 || top - bottom <= 0.0 {
        return None;
    }
    let fit = Matrix::translate(-left, -bottom)
        .then(Matrix::scale((rect_right - rect_left) / (right - left), (rect_top - rect_bottom) / (top - bottom)))
        .then(Matrix::translate(rect_left, rect_bottom));
    Some((form, fit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_content::fixtures::{layered_pdf, marked_content_pdf, placed_stamp_pdf};

    fn shapes_of(bytes: &[u8]) -> Shapes {
        annotation_shapes(&Document::load_mem(bytes).unwrap(), 1, 0.05).unwrap()
    }

    #[test]
    fn an_appearance_is_fitted_to_its_rectangle() {
        let shapes = shapes_of(&placed_stamp_pdf());
        let [line] = &shapes.primitives[..] else { panic!("one line: {:?}", shapes.primitives) };
        assert_eq!((line.points[0], line.points[1], line.width), ([10.0, 10.0], [30.0, 30.0], 2.0), "a 10 x 10 box in a 20 x 20 rectangle at 10, 10");
        assert!(line.clip > 0.0, "an appearance stays within its box");
        let set = line.clip as usize - 1;
        let corners = &shapes.clips.vertices[shapes.clips.shapes[shapes.clips.sets[set][0]].clone()];
        assert!(corners.contains(&[10.0, 10.0]) && corners.contains(&[30.0, 30.0]), "its box is the rectangle: {corners:?}");
    }

    /// The placed stamp's line, on its page with `entries` set.
    fn line_with(entries: &[(&str, Object)]) -> [[f32; 2]; 2] {
        let mut doc = Document::load_mem(&placed_stamp_pdf()).unwrap();
        let page_id = doc.get_pages()[&1];
        let page = doc.get_object_mut(page_id).unwrap().as_dict_mut().unwrap();
        for (key, value) in entries {
            page.set(*key, value.clone());
        }
        let shapes = annotation_shapes(&doc, 1, 0.05).unwrap();
        let [line] = &shapes.primitives[..] else { panic!("one line: {:?}", shapes.primitives) };
        [line.points[0], line.points[1]]
    }

    fn rectangle_of(numbers: [i64; 4]) -> Object {
        Object::Array(numbers.map(Object::Integer).to_vec())
    }

    #[test]
    fn shapes_are_placed_on_the_page_as_displayed() {
        let media = ("MediaBox", rectangle_of([0, 0, 100, 50]));
        assert_eq!(line_with(&[media.clone()]), [[10.0, 10.0], [30.0, 30.0]]);
        assert_eq!(line_with(&[media.clone(), ("Rotate", Object::Integer(90))]), [[10.0, 90.0], [30.0, 70.0]], "the top edge to the right");
        assert_eq!(line_with(&[media.clone(), ("Rotate", Object::Integer(180))]), [[90.0, 40.0], [70.0, 20.0]]);
        assert_eq!(line_with(&[media.clone(), ("Rotate", Object::Integer(-90))]), [[40.0, 10.0], [20.0, 30.0]], "the top edge to the left");
        assert_eq!(line_with(&[media, ("CropBox", rectangle_of([5, 5, 200, 40]))]), [[5.0, 5.0], [25.0, 25.0]], "from the crop box, within the media box");
    }

    #[test]
    fn a_page_draws_its_own_content_then_its_annotations() {
        let mut doc = Document::load_mem(&placed_stamp_pdf()).unwrap();
        let content = doc.add_object(Stream::new(pdf_content::lopdf::dictionary! {}, b"1 0 0 RG 0 0 m 5 5 l S".to_vec()));
        let page_id = doc.get_pages()[&1];
        doc.get_object_mut(page_id).unwrap().as_dict_mut().unwrap().set("Contents", Object::Reference(content));
        let everything = page_shapes(&doc, 1, 0.05).unwrap();
        let lines: Vec<([f32; 2], [f32; 2], [f32; 4])> = everything.primitives.iter().map(|p| (p.points[0], p.points[1], p.colour)).collect();
        assert_eq!(lines, [([0.0, 0.0], [5.0, 5.0], [1.0, 0.0, 0.0, 1.0]), ([10.0, 10.0], [30.0, 30.0], [0.0, 0.0, 0.0, 1.0])], "the page's red line, then the stamp's");
        assert_eq!(annotation_shapes(&doc, 1, 0.05).unwrap().lines, 1, "annotations alone leave the page's content out");
    }

    #[test]
    fn annotations_on_layers_that_are_off_are_left_out() {
        assert_eq!(shapes_of(&layered_pdf()).triangles, 2, "one square of two triangles, from the one stamp shown");
    }

    #[test]
    fn marked_content_on_a_layer_that_is_off_is_left_out() {
        let shapes = shapes_of(&marked_content_pdf());
        assert_eq!(shapes.triangles, 2, "only the square outside the marked content");
        assert!(shapes.primitives.iter().all(|p| p.points.iter().all(|&[x, y]| x >= 5.0 && y >= 5.0)));
    }
}
