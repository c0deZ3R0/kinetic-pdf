//! Merging stroked lines in a PDF's drawing instructions, for a copy of the
//! document that's only ever drawn.
//!
//! Drawings exported from CAD, and Bluebeam overlays made from them, often
//! draw every line as a path of its own: `x y m x y l S`, hundreds of thousands
//! of times. pdfium spends a few microseconds on each path whatever its size, so
//! such a page takes over a second to draw at any zoom. Back-to-back strokes
//! with nothing between them share every setting -- colour, width, caps, clip
//! -- so they can be one path with many parts, `m l m l m l S`, which draws the
//! same.
//!
//! Only the `S` between two such paths is removed; every other byte stays as
//! it was. A run ends at anything that isn't path building (`m l c v y re h`)
//! or `S`: a clip, a fill, a state change. It also ends where strokes might be
//! see-through or blended, since overlapping parts of one path are painted
//! once where separate paths would be painted twice; and a form drawn while
//! things are see-through is left alone altogether.
//!
//! The copy also leaves out annotations on layers that are off (see
//! `pdf_content::layers`), which pdfium would otherwise draw.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use pdf_content::layers::hidden_annotations;
use pdf_content::lexer::each_operation;
use pdf_content::lopdf::{Document, Object, ObjectId};
use pdf_content::objects::{dict, is_form, number};

/// Operators that only build the current path.
fn builds_path(op: &[u8]) -> bool {
    matches!(op, b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h")
}

/// What merging did to one content stream.
#[derive(Debug, Default, PartialEq)]
pub struct Merged {
    /// Strokes before, and strokes merged into the one before them.
    pub strokes: usize,
    pub merged: usize,
}

/// One read of a content stream: the `S`s that can go, at most `most_parts`
/// paths merged into one, the counts, and the names of the forms it draws
/// (`Do`) while strokes would be see-through. `opaque` says whether a graphics
/// state named by `gs` leaves things opaque and unblended.
fn scan(content: &[u8], most_parts: usize, opaque: &dyn Fn(&[u8]) -> bool) -> (Vec<Range<usize>>, Merged, HashSet<Vec<u8>>) {
    let mut stats = Merged::default();
    let mut remove: Vec<Range<usize>> = Vec::new();
    let mut drawn_see_through = HashSet::new();
    // The last `S`, while only path building has followed it, and how many
    // paths it ends.
    let mut open: Option<(Range<usize>, usize)> = None;
    // Whether things are opaque, for each level of `q`.
    let mut safe = vec![true];

    each_operation(content, |op, operands, range| {
        let name = operands.last().and_then(|o| o.name());
        let here = *safe.last().unwrap_or(&true);
        match op {
            b"S" => {
                stats.strokes += 1;
                open = match open.take() {
                    Some((previous, parts)) if here && parts < most_parts => {
                        remove.push(previous);
                        stats.merged += 1;
                        Some((range, parts + 1))
                    }
                    _ => here.then_some((range, 1)),
                };
            }
            op if builds_path(op) => {}
            _ => {
                open = None;
                match op {
                    b"q" => safe.push(here),
                    b"Q" => {
                        if safe.len() > 1 {
                            safe.pop();
                        }
                    }
                    b"gs" => {
                        if let Some(top) = safe.last_mut() {
                            *top = name.is_some_and(opaque);
                        }
                    }
                    b"Do" if !here => {
                        if let Some(name) = name {
                            drawn_see_through.insert(name.to_vec());
                        }
                    }
                    _ => {}
                }
            }
        }
    });
    (remove, stats, drawn_see_through)
}

/// `content` without the byte ranges in `remove`, each replaced by a space.
fn without(content: &[u8], remove: &[Range<usize>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len());
    let mut at = 0;
    for range in remove {
        out.extend_from_slice(&content[at..range.start]);
        out.push(b' ');
        at = range.end;
    }
    out.extend_from_slice(&content[at..]);
    out
}

/// `content` with back-to-back stroked paths merged, at most `most_parts` to a
/// path; `None` if nothing merged. `opaque` says whether a graphics state named
/// by `gs` leaves strokes opaque and unblended; a stroke after one that
/// doesn't is left alone until the state is restored or replaced.
pub fn merge_strokes(content: &[u8], most_parts: usize, opaque: impl Fn(&[u8]) -> bool) -> (Option<Vec<u8>>, Merged) {
    let (remove, stats, _) = scan(content, most_parts, &opaque);
    if remove.is_empty() {
        return (None, stats);
    }
    (Some(without(content, &remove)), stats)
}

/// The names of the forms `content` draws while things are see-through.
pub fn forms_drawn_see_through(content: &[u8], opaque: impl Fn(&[u8]) -> bool) -> HashSet<Vec<u8>> {
    scan(content, 0, &opaque).2
}

/// What merging did to a whole document.
#[derive(Debug, Default)]
pub struct DocumentMerged {
    /// Annotations taken out because they're on a layer that's off.
    pub hidden: usize,
    /// Content streams looked at, and rewritten.
    pub streams: usize,
    pub rewritten: usize,
    /// Streams with strokes to merge that were left alone, because they're
    /// drawn while things are see-through.
    pub see_through: usize,
    pub strokes: usize,
    pub merged: usize,
}

/// Whether an `ExtGState` dictionary leaves what's drawn after it opaque and
/// unblended.
fn state_is_opaque(doc: &Document, state: &Object) -> bool {
    let Some(state) = dict(doc, state) else { return false };
    let alpha = |key: &[u8]| state.get(key).ok().and_then(|v| number(doc, v));
    let opaque_alpha = alpha(b"CA").is_none_or(|a| a >= 1.0) && alpha(b"ca").is_none_or(|a| a >= 1.0);
    let normal_blend = match state.get(b"BM") {
        Err(_) => true,
        Ok(Object::Name(n)) => n == b"Normal" || n == b"Compatible",
        Ok(Object::Array(a)) => a.first().and_then(|n| n.as_name().ok()).is_some_and(|n| n == b"Normal" || n == b"Compatible"),
        Ok(_) => false,
    };
    let no_mask = match state.get(b"SMask") {
        Err(_) => true,
        Ok(Object::Name(n)) => n == b"None",
        Ok(_) => false,
    };
    opaque_alpha && normal_blend && no_mask
}

/// The graphics states and forms a content stream can use.
#[derive(Clone, Default)]
struct Resources {
    /// Names of the graphics states that leave things opaque.
    opaque: HashSet<Vec<u8>>,
    /// Forms by name.
    forms: HashMap<Vec<u8>, ObjectId>,
}

fn read_resources(doc: &Document, resources: Option<&Object>) -> Resources {
    let mut out = Resources::default();
    let Some(resources) = resources.and_then(|r| dict(doc, r)) else { return out };
    if let Some(states) = resources.get(b"ExtGState").ok().and_then(|s| dict(doc, s)) {
        for (name, state) in states.iter() {
            if state_is_opaque(doc, state) {
                out.opaque.insert(name.clone());
            }
        }
    }
    if let Some(xobjects) = resources.get(b"XObject").ok().and_then(|x| dict(doc, x)) {
        for (name, value) in xobjects.iter() {
            if let Some(id) = value.as_reference().ok().filter(|&id| is_form(doc, id)) {
                out.forms.insert(name.clone(), id);
            }
        }
    }
    out
}

/// `bytes` with every page's and every form's back-to-back strokes merged (see
/// `merge_strokes`), at most `most_parts` to a path, as a new PDF; `None` if
/// nothing merged or the file is encrypted. For drawing only: the copy is
/// saved without object streams and with its merged streams compressed fast.
///
/// A form drawn while things are see-through -- after a graphics state with
/// transparency, a blend mode or a soft mask, or as the appearance of an
/// annotation with an opacity -- is left alone, along with every form inside
/// it, since a see-through stroke over itself looks different merged.
pub fn merge_document(bytes: &[u8], most_parts: usize) -> Result<Option<(Vec<u8>, DocumentMerged)>, String> {
    use std::io::Write;

    let mut doc = Document::load_mem(bytes).map_err(|e| e.to_string())?;
    if doc.is_encrypted() || doc.was_encrypted() {
        return Ok(None);
    }
    let mut stats = DocumentMerged::default();

    // Annotations on layers that are off come out of their pages' lists, so
    // pdfium doesn't draw them.
    let hidden = hidden_annotations(&doc);
    if !hidden.is_empty() {
        for (_, page_id) in doc.get_pages() {
            let annots = doc.get_dictionary(page_id).ok().and_then(|p| p.get(b"Annots").ok()).cloned();
            let list = match annots {
                Some(Object::Array(list)) => doc.get_dictionary_mut(page_id).ok().and_then(|p| p.get_mut(b"Annots").ok()).and_then(|a| a.as_array_mut().ok()).map(|a| (a, list.len())),
                Some(Object::Reference(id)) => doc.get_object_mut(id).ok().and_then(|a| a.as_array_mut().ok()).map(|a| {
                    let len = a.len();
                    (a, len)
                }),
                _ => None,
            };
            if let Some((list, before)) = list {
                list.retain(|a| a.as_reference().map_or(true, |id| !hidden.contains(&id)));
                stats.hidden += before - list.len();
            }
        }
    }

    // Only annotation appearances, and the forms they draw, are merged. Merging
    // a page's own drawing measured slower: pdfium skips each line outside the
    // area being drawn, and a merged path spanning the sheet can't be skipped,
    // so a dense drawing set's page at 800% took 145 ms merged against 97 ms.
    let mut roots: Vec<ObjectId> = Vec::new();
    let mut tainted: Vec<ObjectId> = Vec::new();
    for (_, page_id) in doc.get_pages() {
        for annot in doc.get_page_annotations(page_id).unwrap_or_default() {
            let alpha = annot.get(b"CA").ok().and_then(|v| number(&doc, v)).unwrap_or(1.0);
            let appearance = annot.get(b"AP").ok().and_then(|ap| dict(&doc, ap));
            // Normal, rollover and down appearances, each a stream or a
            // dictionary of streams by state.
            let mut streams: Vec<ObjectId> = Vec::new();
            for (_, value) in appearance.map(|ap| ap.iter().collect::<Vec<_>>()).unwrap_or_default() {
                if let Ok(id) = value.as_reference() {
                    if doc.get_object(id).and_then(Object::as_stream).is_ok() {
                        streams.push(id);
                    }
                }
                if let Ok((_, Object::Dictionary(states))) = doc.dereference(value) {
                    streams.extend(states.iter().filter_map(|(_, v)| v.as_reference().ok()));
                }
            }
            if alpha < 1.0 {
                tainted.extend(streams.iter().copied());
            }
            roots.extend(streams);
        }
    }

    // Those streams and every form inside them, with what each can use.
    let mut streams: HashMap<ObjectId, Resources> = HashMap::new();
    while let Some(id) = roots.pop() {
        if streams.contains_key(&id) {
            continue;
        }
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else { continue };
        let resources = read_resources(&doc, stream.dict.get(b"Resources").ok());
        roots.extend(resources.forms.values().copied());
        streams.insert(id, resources);
    }

    // Read every stream once: what could merge, and which forms it draws while
    // see-through.
    let mut to_merge: HashMap<ObjectId, (Vec<u8>, Vec<Range<usize>>, Merged)> = HashMap::new();
    for (id, resources) in &streams {
        stats.streams += 1;
        let Ok(content) = doc.get_object(*id).and_then(Object::as_stream).and_then(|s| s.get_plain_content()) else { continue };
        let (remove, counts, drawn) = scan(&content, most_parts, &|name: &[u8]| resources.opaque.contains(name));
        tainted.extend(drawn.iter().filter_map(|name| resources.forms.get(name)));
        if !remove.is_empty() {
            to_merge.insert(*id, (content, remove, counts));
        }
    }

    // Everything drawn while see-through, and all it draws, is left alone.
    let mut left_alone: HashSet<ObjectId> = HashSet::new();
    while let Some(id) = tainted.pop() {
        if left_alone.insert(id) {
            tainted.extend(streams.get(&id).into_iter().flat_map(|r| r.forms.values()).copied());
        }
    }

    for (id, (content, remove, counts)) in to_merge {
        if left_alone.contains(&id) {
            stats.see_through += 1;
            continue;
        }
        let Ok(object) = doc.get_object_mut(id).and_then(Object::as_stream_mut) else { continue };
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&without(&content, &remove)).map_err(|e| e.to_string())?;
        let compressed = encoder.finish().map_err(|e| e.to_string())?;
        object.set_plain_content(Vec::new());
        object.set_content(compressed);
        object.dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        stats.rewritten += 1;
        stats.strokes += counts.strokes;
        stats.merged += counts.merged;
    }
    if stats.rewritten == 0 && stats.hidden == 0 {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(bytes.len());
    doc.save_to(&mut out).map_err(|e| e.to_string())?;
    Ok(Some((out, stats)))
}

/// The argument that starts the exe to make a copy of a file for drawing, in a
/// process of its own, so a PDF that trips up the reader takes only that
/// process down.
pub const FLAG: &str = "--merge-copy";

/// Most paths merged into one. On a Bluebeam overlay anything from 64 up drew
/// the same; a cap keeps any one path from covering too much of a drawing.
pub const MOST_PARTS: usize = 256;

/// `pdf-annotate --merge-copy source fingerprint copy`: writes `copy` from the
/// source with its stamps' lines merged -- empty if there's nothing to merge,
/// or the reader couldn't make sense of the file, so it isn't tried again --
/// and returns 0. Returns 1 without writing if the source can't be read or no
/// longer has that fingerprint (hexadecimal, see `cache::fingerprint`).
pub fn run_copy(mut args: impl Iterator<Item = std::ffi::OsString>) -> i32 {
    let (Some(source), Some(fingerprint), Some(copy)) = (args.next(), args.next(), args.next()) else { return 1 };
    let Ok(bytes) = std::fs::read(&source) else { return 1 };
    if fingerprint.to_str().and_then(|f| u64::from_str_radix(f, 16).ok()) != Some(crate::cache::fingerprint(&bytes)) {
        return 1;
    }
    let out = match merge_document(&bytes, MOST_PARTS) {
        Ok(Some((out, _))) => out,
        Ok(None) | Err(_) => Vec::new(),
    };
    let copy = std::path::PathBuf::from(copy);
    let temporary = copy.with_extension(format!("{}.tmp", std::process::id()));
    match std::fs::write(&temporary, &out).and_then(|()| std::fs::rename(&temporary, &copy)) {
        Ok(()) => 0,
        Err(_) => {
            let _ = std::fs::remove_file(&temporary);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_content::fixtures::{layered_pdf, stamped_pdf};

    #[test]
    fn annotations_on_layers_that_are_off_are_taken_out_of_the_copy() {
        let (copy, stats) = merge_document(&layered_pdf(), MOST_PARTS).unwrap().expect("two stamps are hidden");
        assert_eq!((stats.hidden, stats.rewritten), (2, 0), "{stats:?}");
        let doc = Document::load_mem(&copy).unwrap();
        let page = *doc.get_pages().get(&1).unwrap();
        assert_eq!(doc.get_page_annotations(page).unwrap().len(), 1, "only the stamp on the layer that's on is left");
    }

    #[test]
    fn a_document_has_only_its_opaque_stamp_appearances_merged() {
        let (merged, stats) = merge_document(&stamped_pdf(), MOST_PARTS).unwrap().expect("the stamp has strokes to merge");
        assert_eq!((stats.rewritten, stats.merged, stats.see_through), (1, 1, 1), "{stats:?}");
        let doc = Document::load_mem(&merged).unwrap();
        let page = *doc.get_pages().get(&1).unwrap();
        let content = String::from_utf8(doc.get_page_content(page)).unwrap();
        assert_eq!(squash(&content), "1 1 m 5 5 l S 2 2 m 6 6 l S", "the page's own drawing is untouched");
    }

    fn merge(content: &str) -> (String, Merged) {
        let (out, stats) = merge_strokes(content.as_bytes(), usize::MAX, |name| name != b"Faint");
        (out.map(|o| String::from_utf8(o).unwrap()).unwrap_or_else(|| content.to_owned()), stats)
    }

    fn squash(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn back_to_back_strokes_become_one_path() {
        let (out, stats) = merge("1 2 m 3 4 l S\n5 6 m 7 8 l S\n9 10 m 11 12 l 13 14 l S");
        assert_eq!(squash(&out), "1 2 m 3 4 l 5 6 m 7 8 l 9 10 m 11 12 l 13 14 l S");
        assert_eq!(stats, Merged { strokes: 3, merged: 2 });
    }

    #[test]
    fn anything_between_strokes_ends_a_run() {
        for between in ["0 0 1 RG", "2 w", "q", "Q", "0 0 10 10 re W n", "/GS1 gs", "1 0 0 1 5 5 cm", "[3] 0 d"] {
            let content = format!("1 2 m 3 4 l S {between} 5 6 m 7 8 l S");
            let (out, stats) = merge(&content);
            assert_eq!(stats.merged, 0, "merged across {between}");
            assert_eq!(out, content);
        }
    }

    #[test]
    fn fills_and_clips_are_never_merged_into_strokes() {
        let (out, stats) = merge("1 2 m 3 4 l S 0 0 5 5 re f 1 2 m 3 4 l S 0 0 5 5 re W n 1 2 m 3 4 l S");
        assert_eq!(stats.merged, 0);
        assert!(out.contains("re W n"));
        let (_, stats) = merge("1 2 m 3 4 l S 5 6 m 7 8 l h S 0 0 9 9 re S");
        assert_eq!(stats.merged, 2, "closed paths and rectangles are path building too");
    }

    #[test]
    fn see_through_strokes_are_left_until_the_state_is_restored_or_replaced() {
        let (_, stats) = merge("q /Faint gs 1 2 m 3 4 l S 5 6 m 7 8 l S Q 1 2 m 3 4 l S 5 6 m 7 8 l S");
        assert_eq!(stats, Merged { strokes: 4, merged: 1 }, "only the pair after Q");
        let (_, stats) = merge("/Faint gs 1 2 m 3 4 l S 5 6 m 7 8 l S /Solid gs 1 2 m 3 4 l S 5 6 m 7 8 l S");
        assert_eq!(stats.merged, 1, "only the pair after an opaque state replaces the see-through one");
    }

    #[test]
    fn forms_drawn_while_see_through_are_named() {
        let drawn = forms_drawn_see_through(b"/A Do q /Faint gs /B Do Q /C Do q /Solid gs /D Do Q", |n| n != b"Faint");
        assert_eq!(drawn, HashSet::from([b"B".to_vec()]));
    }

    #[test]
    fn a_run_stops_at_the_most_parts() {
        let content = "1 1 m 2 2 l S ".repeat(7);
        let (out, stats) = merge_strokes(content.as_bytes(), 3, |_| true);
        assert_eq!(stats.merged, 4, "7 paths at 3 parts a path make 3 paths");
        assert_eq!(String::from_utf8(out.unwrap()).unwrap().matches('S').count(), 3);
    }

    #[test]
    fn strokes_are_found_past_strings_names_comments_and_inline_images() {
        let content = "BT (a S (nested) \\) S) Tj ET 1 2 m 3 4 l S % S in a comment\n\
                       /S gs 5 6 m 7 8 l S <53> Tj 1 2 m 3 4 l S \
                       BI /W 2 /H 1 /BPC 8 /CS /G ID \x00S EI 1 2 m 3 4 l S 5 6 m 7 8 l S";
        let (out, stats) = merge_strokes(content.as_bytes(), usize::MAX, |_| true);
        assert_eq!(stats.strokes, 5, "the S inside the string, comment, name and image aren't strokes");
        assert_eq!(stats.merged, 1, "only the last pair is back to back");
        let out = out.unwrap();
        assert!(out.windows(8).any(|w| w == b"ID \x00S EI"), "inline image data kept");
        assert_eq!(out.len(), content.len(), "each removed S leaves one space");
    }

    #[test]
    fn nothing_to_merge_gives_none() {
        assert_eq!(merge_strokes(b"0 0 5 5 re f", 16, |_| true).0, None);
        assert_eq!(merge_strokes(b"", 16, |_| true).0, None);
    }
}
