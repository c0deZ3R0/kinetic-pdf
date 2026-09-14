//! Small PDFs built for tests, here and in the crates that use this one (with
//! the `fixtures` feature).

use lopdf::{dictionary, Document, Object, ObjectId, Stream};

/// A 10 x 10 box, as a PDF rectangle.
fn square() -> Vec<Object> {
    vec![Object::from(0), 0.into(), 10.into(), 10.into()]
}

/// Saves a one-page document whose page has `annots` and, if given, content
/// stream `content`.
fn one_page(mut doc: Document, content: Option<ObjectId>, annots: Vec<ObjectId>, catalog: lopdf::Dictionary) -> Vec<u8> {
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

/// A one-page PDF whose page content strokes two lines, with a stamp whose
/// appearance strokes the same two lines and then draws a form of two more
/// while see-through.
pub fn stamped_pdf() -> Vec<u8> {
    let lines = b"1 1 m 5 5 l S 2 2 m 6 6 l S".to_vec();
    let mut doc = Document::with_version("1.7");
    let inner = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => square() }, lines.clone()));
    let faint = doc.add_object(dictionary! { "Type" => "ExtGState", "CA" => Object::Real(0.5) });
    let mut stamp = lines.clone();
    stamp.extend_from_slice(b" q /Faint gs /Inner Do Q");
    let appearance = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => square(),
            "Resources" => dictionary! {
                "ExtGState" => dictionary! { "Faint" => faint },
                "XObject" => dictionary! { "Inner" => inner },
            },
        },
        stamp,
    ));
    let content = doc.add_object(Stream::new(dictionary! {}, lines));
    let annot = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Stamp", "Rect" => square(), "AP" => dictionary! { "N" => appearance } });
    one_page(doc, Some(content), vec![annot], dictionary! {})
}

/// A one-page PDF with three stamps that draw nothing to merge: one on a
/// layer that's on, one on a layer that's off, and one shown only while any
/// of its layers -- just the one that's off -- is on.
pub fn layered_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let shown = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("current") });
    let old = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("old") });
    let mut stamp = |oc: Object| {
        let appearance = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => square() }, b"0 0 5 5 re f".to_vec()));
        doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Stamp", "Rect" => square(), "AP" => dictionary! { "N" => appearance }, "OC" => oc })
    };
    let annots = vec![
        stamp(Object::Reference(shown)),
        stamp(Object::Reference(old)),
        stamp(Object::Dictionary(dictionary! { "Type" => "OCMD", "OCGs" => vec![Object::Reference(old)], "P" => "AnyOn" })),
    ];
    let layers = dictionary! {
        "OCProperties" => dictionary! {
            "OCGs" => vec![Object::Reference(shown), Object::Reference(old)],
            "D" => dictionary! { "OFF" => vec![Object::Reference(old)] },
        },
    };
    one_page(doc, None, annots, layers)
}
