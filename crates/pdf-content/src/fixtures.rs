//! Small PDFs built for tests, here and in the crates that use this one (with
//! the `fixtures` feature). Each is one 10 x 10 page with stamps on it.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

/// A rectangle, as PDF writes one.
fn rectangle(left: i64, bottom: i64, right: i64, top: i64) -> Vec<Object> {
    vec![left.into(), bottom.into(), right.into(), top.into()]
}

/// A 10 x 10 box.
fn square() -> Vec<Object> {
    rectangle(0, 0, 10, 10)
}

/// Adds a form XObject drawing `content` in a 10 x 10 box, with `extra`
/// entries (resources, say) in its dictionary.
fn form(doc: &mut Document, content: &[u8], extra: Dictionary) -> ObjectId {
    let mut entries = dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => square() };
    entries.extend(&extra);
    doc.add_object(Stream::new(entries, content.to_vec()))
}

/// Adds a stamp annotation showing `appearance` in `rect`, with `extra`
/// entries (a layer, say).
fn stamp(doc: &mut Document, appearance: ObjectId, rect: Vec<Object>, extra: Dictionary) -> ObjectId {
    let mut annot = dictionary! { "Type" => "Annot", "Subtype" => "Stamp", "Rect" => rect, "AP" => dictionary! { "N" => appearance } };
    annot.extend(&extra);
    doc.add_object(annot)
}

/// Saves a one-page document whose page has `annots` and, if given, content
/// stream `content`; `catalog` adds entries to the document catalog.
fn one_page(mut doc: Document, content: Option<ObjectId>, annots: Vec<ObjectId>, catalog: Dictionary) -> Vec<u8> {
    let pages = doc.new_object_id();
    let mut page = dictionary! {
        "Type" => "Page",
        "Parent" => pages,
        "MediaBox" => square(),
        "Annots" => annots.into_iter().map(Object::from).collect::<Vec<_>>(),
    };
    if let Some(content) = content {
        page.set("Contents", content);
    }
    let page = doc.add_object(page);
    doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::from(page)], "Count" => 1 }));
    let mut catalog = catalog;
    catalog.set("Type", "Catalog");
    catalog.set("Pages", pages);
    let catalog = doc.add_object(catalog);
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("a document in memory saves");
    out
}

/// The catalog entries for layers `groups`, with `off` turned off.
fn layers(groups: &[ObjectId], off: &[ObjectId]) -> Dictionary {
    let references = |ids: &[ObjectId]| ids.iter().map(|&id| Object::Reference(id)).collect::<Vec<_>>();
    dictionary! { "OCProperties" => dictionary! { "OCGs" => references(groups), "D" => dictionary! { "OFF" => references(off) } } }
}

fn layer(doc: &mut Document, name: &str) -> ObjectId {
    doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal(name) })
}

/// A page whose content strokes two lines, with a stamp whose appearance
/// strokes the same two lines and then draws a form of two more while
/// see-through.
pub fn stamped_pdf() -> Vec<u8> {
    let lines = b"1 1 m 5 5 l S 2 2 m 6 6 l S";
    let mut doc = Document::with_version("1.7");
    let inner = form(&mut doc, lines, dictionary! {});
    let faint = doc.add_object(dictionary! { "Type" => "ExtGState", "CA" => Object::Real(0.5) });
    let resources = dictionary! {
        "Resources" => dictionary! {
            "ExtGState" => dictionary! { "Faint" => faint },
            "XObject" => dictionary! { "Inner" => inner },
        },
    };
    let appearance = form(&mut doc, &[&lines[..], b" q /Faint gs /Inner Do Q"].concat(), resources);
    let content = doc.add_object(Stream::new(dictionary! {}, lines.to_vec()));
    let annot = stamp(&mut doc, appearance, square(), dictionary! {});
    one_page(doc, Some(content), vec![annot], dictionary! {})
}

/// Three stamps each filling a 5 x 5 square: one on a layer that's on, one on
/// a layer that's off, and one shown only while any of its layers -- just the
/// one that's off -- is on.
pub fn layered_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let (shown, old) = (layer(&mut doc, "current"), layer(&mut doc, "old"));
    let mut on_layer = |oc: Object| {
        let appearance = form(&mut doc, b"0 0 5 5 re f", dictionary! {});
        stamp(&mut doc, appearance, square(), dictionary! { "OC" => oc })
    };
    let annots = vec![
        on_layer(Object::Reference(shown)),
        on_layer(Object::Reference(old)),
        on_layer(Object::Dictionary(dictionary! { "Type" => "OCMD", "OCGs" => vec![Object::Reference(old)], "P" => "AnyOn" })),
    ];
    one_page(doc, None, annots, layers(&[shown, old], &[old]))
}

/// A stamp whose 10 x 10 appearance, a diagonal line, is placed in a 20 x 20
/// rectangle at 10, 10.
pub fn placed_stamp_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let appearance = form(&mut doc, b"0 0 m 10 10 l S", dictionary! {});
    let annot = stamp(&mut doc, appearance, rectangle(10, 10, 30, 30), dictionary! {});
    one_page(doc, None, vec![annot], dictionary! {})
}

/// A stamp whose appearance fills one 5 x 5 square inside marked content on a
/// layer that's off, and another outside it.
pub fn marked_content_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let old = layer(&mut doc, "old");
    let resources = dictionary! { "Resources" => dictionary! { "Properties" => dictionary! { "Old" => old } } };
    let appearance = form(&mut doc, b"/OC /Old BDC 0 0 5 5 re f EMC 5 5 5 5 re f", resources);
    let annot = stamp(&mut doc, appearance, square(), dictionary! {});
    one_page(doc, None, vec![annot], layers(&[old], &[old]))
}
