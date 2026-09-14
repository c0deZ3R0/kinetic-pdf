//! Reading geometry out of PDF objects: arrays of numbers, matrices and
//! rectangles.

use pdf_content::lopdf::{Document, Object};
use pdf_content::objects::number;

use crate::geometry::Matrix;

/// An array of numbers, following references; `None` if it isn't one.
pub(crate) fn numbers(doc: &Document, object: &Object) -> Option<Vec<f32>> {
    let (_, object) = doc.dereference(object).ok()?;
    object.as_array().ok()?.iter().map(|n| number(doc, n).map(|n| n as f32)).collect()
}

/// A matrix, `[a b c d e f]`.
pub(crate) fn matrix(doc: &Document, object: &Object) -> Option<Matrix> {
    Some(Matrix(numbers(doc, object)?.try_into().ok()?))
}

/// A rectangle as left, bottom, right, top, whichever corners it was written
/// with.
pub(crate) fn rectangle(doc: &Document, object: &Object) -> Option<[f32; 4]> {
    let [x1, y1, x2, y2]: [f32; 4] = numbers(doc, object)?.try_into().ok()?;
    Some([x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)])
}
