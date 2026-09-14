//! Following references between a PDF's objects.

use lopdf::{Dictionary, Document, Object, ObjectId};

/// A dictionary, following a reference to it.
pub fn dict<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Dictionary> {
    doc.dereference(object).ok().and_then(|(_, o)| o.as_dict().ok())
}

/// A number, integer or real, following a reference to it.
pub fn number(doc: &Document, object: &Object) -> Option<f64> {
    match doc.dereference(object).ok()?.1 {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

/// Whether object `id` is a form XObject.
pub fn is_form(doc: &Document, id: ObjectId) -> bool {
    doc.get_object(id)
        .and_then(Object::as_stream)
        .is_ok_and(|s| s.dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Form"))
}
