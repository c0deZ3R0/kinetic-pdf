//! Optional content -- layers -- and which of it is visible when a document
//! opens.
//!
//! pdfium draws a page's annotations whatever their layer, so a markup
//! overlay that kept an old version of its stamps on a hidden layer showed
//! both versions, one out of line with the other. Viewers that honour layers,
//! as many do, show only what's on.

use std::collections::HashSet;

use lopdf::{Document, Object, ObjectId};

use crate::objects::dict;

/// A document's layers, as its default configuration turns them on and off.
pub struct Layers {
    base_off: bool,
    turned_on: HashSet<ObjectId>,
    turned_off: HashSet<ObjectId>,
}

impl Layers {
    /// The document's layers; `None` if it has none.
    pub fn read(doc: &Document) -> Option<Layers> {
        let properties = doc.catalog().ok()?.get(b"OCProperties").ok().and_then(|p| dict(doc, p))?;
        let config = properties.get(b"D").ok().and_then(|d| dict(doc, d));
        let groups = |key: &[u8]| -> HashSet<ObjectId> {
            config
                .and_then(|c| c.get(key).ok())
                .and_then(|a| doc.dereference(a).ok())
                .and_then(|(_, a)| a.as_array().ok())
                .map(|a| a.iter().filter_map(|g| g.as_reference().ok()).collect())
                .unwrap_or_default()
        };
        Some(Layers {
            base_off: config.and_then(|c| c.get(b"BaseState").ok()).and_then(|b| b.as_name().ok()) == Some(b"OFF"),
            turned_on: groups(b"ON"),
            turned_off: groups(b"OFF"),
        })
    }

    /// Whether layer (optional content group) `id` is on.
    pub fn is_on(&self, id: ObjectId) -> bool {
        if self.base_off {
            self.turned_on.contains(&id)
        } else {
            !self.turned_off.contains(&id)
        }
    }

    /// Whether optional content `oc` -- a group, a membership dictionary, or a
    /// visibility expression -- is visible. Anything it can't make sense of
    /// counts as visible.
    pub fn is_visible(&self, doc: &Document, oc: &Object) -> bool {
        self.visible(doc, oc, 0)
    }

    fn visible(&self, doc: &Document, oc: &Object, depth: usize) -> bool {
        if depth > 8 {
            return true;
        }
        if let Ok(Object::Array(expression)) = doc.dereference(oc).map(|(_, o)| o) {
            let Some(op) = expression.first().and_then(|o| o.as_name().ok()) else { return true };
            let mut operands = expression[1..].iter().map(|o| self.visible(doc, o, depth + 1));
            return match op {
                b"And" => operands.all(|v| v),
                b"Or" => operands.any(|v| v),
                b"Not" => !operands.next().unwrap_or(false),
                _ => true,
            };
        }
        let Some(d) = dict(doc, oc) else { return true };
        match d.get(b"Type").and_then(Object::as_name) {
            Ok(b"OCG") => oc.as_reference().map_or(true, |id| self.is_on(id)),
            Ok(b"OCMD") => {
                if let Ok(expression) = d.get(b"VE") {
                    return self.visible(doc, expression, depth + 1);
                }
                let groups: Vec<bool> = match d.get(b"OCGs").ok().map(|g| doc.dereference(g).map(|(_, g)| g)) {
                    Some(Ok(Object::Array(a))) => a.iter().filter_map(|g| g.as_reference().ok()).map(|id| self.is_on(id)).collect(),
                    Some(Ok(Object::Dictionary(_))) => {
                        d.get(b"OCGs").ok().and_then(|g| g.as_reference().ok()).map(|id| self.is_on(id)).into_iter().collect()
                    }
                    _ => return true,
                };
                if groups.is_empty() {
                    return true;
                }
                match d.get(b"P").and_then(Object::as_name) {
                    Ok(b"AllOn") => groups.iter().all(|&v| v),
                    Ok(b"AnyOff") => groups.iter().any(|&v| !v),
                    Ok(b"AllOff") => groups.iter().all(|&v| !v),
                    _ => groups.iter().any(|&v| v),
                }
            }
            _ => true,
        }
    }
}

/// The annotations shown only on layers that are off when the document opens.
pub fn hidden_annotations(doc: &Document) -> HashSet<ObjectId> {
    let mut hidden = HashSet::new();
    let Some(layers) = Layers::read(doc) else { return hidden };
    for (_, page_id) in doc.get_pages() {
        let Some(annots) = doc.get_dictionary(page_id).ok().and_then(|p| p.get(b"Annots").ok()) else { continue };
        let Ok((_, Object::Array(annots))) = doc.dereference(annots) else { continue };
        for reference in annots {
            let (Ok(id), Some(annot)) = (reference.as_reference(), dict(doc, reference)) else { continue };
            if annot.get(b"OC").is_ok_and(|oc| !layers.is_visible(doc, oc)) {
                hidden.insert(id);
            }
        }
    }
    hidden
}

/// Whether a file might have annotations on layers that are off, from its
/// bytes alone: it has layers, or keeps objects compressed where their names
/// can't be seen. False means it certainly doesn't.
pub fn may_hide_annotations(bytes: &[u8]) -> bool {
    let mut at = 0;
    while let Some(slash) = bytes[at..].iter().position(|&b| b == b'/') {
        let rest = &bytes[at + slash..];
        if rest.starts_with(b"/OCProperties") || rest.starts_with(b"/ObjStm") {
            return true;
        }
        at += slash + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{layered_pdf, stamped_pdf};

    #[test]
    fn annotations_on_layers_that_are_off_are_found() {
        let bytes = layered_pdf();
        let doc = Document::load_mem(&bytes).unwrap();
        assert_eq!(hidden_annotations(&doc).len(), 2, "the one on the layer that's off, and the one shown only while it's on");
        assert!(may_hide_annotations(&bytes));
        assert!(!may_hide_annotations(&stamped_pdf()), "no layers and no compressed objects");
    }

    #[test]
    fn a_document_without_layers_hides_nothing() {
        let doc = Document::load_mem(&stamped_pdf()).unwrap();
        assert!(Layers::read(&doc).is_none());
        assert!(hidden_annotations(&doc).is_empty());
    }
}
