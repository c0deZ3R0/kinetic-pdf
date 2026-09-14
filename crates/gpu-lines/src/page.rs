//! The annotations shown on a page, each appearance placed where PDF says it
//! goes, drawn into shapes.

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

/// The shapes of every annotation shown on page `page_number` (from 1), in
/// page space with the origin at the bottom left of the page's visible area,
/// as pdfium draws it. Curves are flattened to within `tolerance` points.
pub fn annotation_shapes(doc: &Document, page_number: u32, tolerance: f32) -> Result<Shapes, String> {
    let page_id = *doc.get_pages().get(&page_number).ok_or_else(|| format!("there's no page {page_number}"))?;
    let page = doc.get_dictionary(page_id).map_err(|e| e.to_string())?;
    let [left, bottom, ..] = inherited(doc, page_id, b"CropBox")
        .or_else(|| inherited(doc, page_id, b"MediaBox"))
        .and_then(|b| rectangle(doc, b))
        .unwrap_or([0.0; 4]);
    let to_page = Matrix::translate(-left, -bottom);

    let mut interpreter = Interpreter::new(doc, tolerance);
    if inherited(doc, page_id, b"Rotate").and_then(|r| number(doc, r)).is_some_and(|r| r % 360.0 != 0.0) {
        interpreter.shapes.not_drawn("page rotation");
    }
    let annots = page.get(b"Annots").ok().and_then(|a| doc.dereference(a).ok()).and_then(|(_, a)| a.as_array().ok());
    for annot in annots.into_iter().flatten().filter_map(|a| dict(doc, a)) {
        if !is_shown(doc, &interpreter, annot) {
            continue;
        }
        if let Some((appearance, placed)) = appearance(doc, annot) {
            interpreter.draw_form(appearance, None, placed.then(to_page));
        }
    }
    Ok(interpreter.shapes)
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
