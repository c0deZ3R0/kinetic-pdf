//! Following references between a PDF's objects.

use std::collections::HashMap;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

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

/// `object`, from document `from`, copied into `to` with everything it refers
/// to: each object once, however often it's referred to, under a number of
/// `to`'s own. `copied` is what has been copied so far, by its number in
/// `from`, and can be carried from one call to the next so shared objects --
/// a font used on two pages -- go across once.
pub fn copy_object(from: &Document, object: &Object, to: &mut Document, copied: &mut HashMap<ObjectId, ObjectId>) -> Object {
    match object {
        Object::Reference(id) => {
            if let Some(&done) = copied.get(id) {
                return Object::Reference(done);
            }
            // Numbered before it's copied, so anything it refers back to
            // finds it.
            let number = to.new_object_id();
            copied.insert(*id, number);
            let copy = match from.get_object(*id) {
                Ok(inner) => copy_object(from, inner, to, copied),
                Err(_) => Object::Null,
            };
            to.objects.insert(number, copy);
            Object::Reference(number)
        }
        Object::Array(items) => Object::Array(items.iter().map(|item| copy_object(from, item, to, copied)).collect()),
        Object::Dictionary(dict) => Object::Dictionary(copy_dictionary(from, dict, to, copied)),
        Object::Stream(stream) => {
            let mut copy = Stream::new(copy_dictionary(from, &stream.dict, to, copied), stream.content.clone());
            // Its bytes are copied as they are, filters and all.
            copy.allows_compression = false;
            Object::Stream(copy)
        }
        other => other.clone(),
    }
}

/// `copy_object` for a dictionary's values.
pub fn copy_dictionary(from: &Document, dict: &Dictionary, to: &mut Document, copied: &mut HashMap<ObjectId, ObjectId>) -> Dictionary {
    dict.iter().map(|(key, value)| (key.clone(), copy_object(from, value, to, copied))).collect()
}
