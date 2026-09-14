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

use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// Operators that only build the current path.
fn builds_path(op: &[u8]) -> bool {
    matches!(op, b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h")
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0')
}

fn is_delimiter(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// One piece of a content stream.
#[derive(Debug, PartialEq)]
enum Token<'a> {
    /// A number, name, string, array or dictionary part: anything that isn't
    /// an operator. Names are kept, for `gs` and `Do`.
    Operand(Option<&'a [u8]>),
    /// An operator, and where it lies in the stream.
    Operator(&'a [u8], Range<usize>),
}

/// Reads a content stream token by token. Anything it doesn't understand ends
/// it, leaving the rest of the stream untouched.
struct Tokens<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Tokens<'a> {
    fn new(data: &'a [u8]) -> Self {
        Tokens { data, at: 0 }
    }

    /// Skips an inline image's data, which follows `ID` and a single
    /// whitespace byte and runs to `EI` standing on its own.
    fn skip_inline_image(&mut self) {
        let data = self.data;
        let mut i = self.at + 1;
        while i + 2 <= data.len() {
            if data[i] == b'E'
                && data[i + 1] == b'I'
                && is_whitespace(data[i - 1])
                && (i + 2 == data.len() || is_whitespace(data[i + 2]) || is_delimiter(data[i + 2]))
            {
                self.at = i;
                return;
            }
            i += 1;
        }
        self.at = data.len();
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let data = self.data;
        loop {
            while self.at < data.len() && is_whitespace(data[self.at]) {
                self.at += 1;
            }
            if self.at >= data.len() {
                return None;
            }
            if data[self.at] == b'%' {
                while self.at < data.len() && !matches!(data[self.at], b'\r' | b'\n') {
                    self.at += 1;
                }
                continue;
            }
            break;
        }
        let start = self.at;
        match data[start] {
            b'(' => {
                // A literal string: balanced parentheses, backslash escapes.
                let mut depth = 0usize;
                while self.at < data.len() {
                    match data[self.at] {
                        b'\\' => self.at += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                self.at += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    self.at += 1;
                }
                Some(Token::Operand(None))
            }
            b'<' if data.get(start + 1) == Some(&b'<') => {
                self.at += 2;
                Some(Token::Operand(None))
            }
            b'>' if data.get(start + 1) == Some(&b'>') => {
                self.at += 2;
                Some(Token::Operand(None))
            }
            b'<' => {
                while self.at < data.len() && data[self.at] != b'>' {
                    self.at += 1;
                }
                self.at += 1;
                Some(Token::Operand(None))
            }
            b'[' | b']' | b'{' | b'}' => {
                self.at += 1;
                Some(Token::Operand(None))
            }
            b'/' => {
                self.at += 1;
                while self.at < data.len() && !is_whitespace(data[self.at]) && !is_delimiter(data[self.at]) {
                    self.at += 1;
                }
                Some(Token::Operand(Some(&data[start + 1..self.at])))
            }
            b')' | b'>' => {
                // Unbalanced; stop reading.
                self.at = data.len();
                None
            }
            _ => {
                while self.at < data.len() && !is_whitespace(data[self.at]) && !is_delimiter(data[self.at]) {
                    self.at += 1;
                }
                let word = &data[start..self.at];
                let number = word.iter().all(|&b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.'));
                if number || matches!(word, b"true" | b"false" | b"null") {
                    Some(Token::Operand(None))
                } else {
                    if word == b"ID" {
                        self.skip_inline_image();
                    }
                    Some(Token::Operator(word, start..self.at))
                }
            }
        }
    }
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
    let mut last_name: Option<&[u8]> = None;

    for token in Tokens::new(content) {
        let (op, range) = match token {
            Token::Operand(name) => {
                if name.is_some() {
                    last_name = name;
                }
                continue;
            }
            Token::Operator(op, range) => (op, range),
        };
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
                            *top = last_name.is_some_and(opaque);
                        }
                    }
                    b"Do" if !here => {
                        if let Some(name) = last_name {
                            drawn_see_through.insert(name.to_vec());
                        }
                    }
                    _ => {}
                }
            }
        }
        last_name = None;
    }
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

/// Whether a file might have annotations on layers that are off (see
/// `hidden_annotations`), from its bytes alone: it has layers, or keeps
/// objects compressed where their names can't be seen. False means it
/// certainly doesn't.
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

/// The annotations shown only on a layer (optional content) that's off when
/// the document opens. Viewers that honour layers, Bluebeam among them, don't
/// show these, but pdfium draws a page's annotations whatever their layer: a
/// Bluebeam overlay that kept an old version of its stamps on a hidden layer
/// showed both versions, one out of line with the other.
fn hidden_annotations(doc: &lopdf::Document) -> HashSet<lopdf::ObjectId> {
    use lopdf::{Dictionary, Document, Object, ObjectId};

    fn dict<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
        doc.dereference(o).ok().and_then(|(_, o)| o.as_dict().ok())
    }

    /// Whether optional content `oc` -- a group, a membership dictionary, or
    /// a visibility expression -- is visible, given which groups are on.
    fn visible(doc: &Document, oc: &Object, on: &dyn Fn(ObjectId) -> bool, depth: usize) -> bool {
        if depth > 8 {
            return true;
        }
        if let Ok(Object::Array(expression)) = doc.dereference(oc).map(|(_, o)| o) {
            let Some(op) = expression.first().and_then(|o| o.as_name().ok()) else { return true };
            let mut operands = expression[1..].iter().map(|o| visible(doc, o, on, depth + 1));
            return match op {
                b"And" => operands.all(|v| v),
                b"Or" => operands.any(|v| v),
                b"Not" => !operands.next().unwrap_or(false),
                _ => true,
            };
        }
        let Some(d) = dict(doc, oc) else { return true };
        match d.get(b"Type").and_then(Object::as_name) {
            Ok(b"OCG") => oc.as_reference().map_or(true, on),
            Ok(b"OCMD") => {
                if let Ok(expression) = d.get(b"VE") {
                    return visible(doc, expression, on, depth + 1);
                }
                let groups: Vec<bool> = match d.get(b"OCGs").ok().map(|g| doc.dereference(g).map(|(_, g)| g)) {
                    Some(Ok(Object::Array(a))) => a.iter().filter_map(|g| g.as_reference().ok()).map(on).collect(),
                    Some(Ok(Object::Dictionary(_))) => d.get(b"OCGs").ok().and_then(|g| g.as_reference().ok()).map(on).into_iter().collect(),
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

    let mut hidden = HashSet::new();
    let Some(properties) = doc.catalog().ok().and_then(|c| c.get(b"OCProperties").ok()).and_then(|p| dict(doc, p)) else {
        return hidden;
    };
    let config = properties.get(b"D").ok().and_then(|d| dict(doc, d));
    let groups = |key: &[u8]| -> HashSet<ObjectId> {
        config
            .and_then(|c| c.get(key).ok())
            .and_then(|a| doc.dereference(a).ok())
            .and_then(|(_, a)| a.as_array().ok())
            .map(|a| a.iter().filter_map(|g| g.as_reference().ok()).collect())
            .unwrap_or_default()
    };
    let base_off = config.and_then(|c| c.get(b"BaseState").ok()).and_then(|b| b.as_name().ok()) == Some(b"OFF");
    let (turned_on, turned_off) = (groups(b"ON"), groups(b"OFF"));
    let on = |id: ObjectId| if base_off { turned_on.contains(&id) } else { !turned_off.contains(&id) };

    for (_, page_id) in doc.get_pages() {
        let Some(annots) = doc.get_dictionary(page_id).ok().and_then(|p| p.get(b"Annots").ok()) else { continue };
        let Ok((_, Object::Array(annots))) = doc.dereference(annots) else { continue };
        for reference in annots {
            let (Ok(id), Some(annot)) = (reference.as_reference(), dict(doc, reference)) else { continue };
            if let Ok(oc) = annot.get(b"OC") {
                if !visible(doc, oc, &on, 0) {
                    hidden.insert(id);
                }
            }
        }
    }
    hidden
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
fn state_is_opaque(doc: &lopdf::Document, state: &lopdf::Object) -> bool {
    use lopdf::Object;
    let Ok((_, state)) = doc.dereference(state) else { return false };
    let Ok(state) = state.as_dict() else { return false };
    let number = |key: &[u8]| match state.get(key).ok().and_then(|v| doc.dereference(v).ok()).map(|(_, v)| v) {
        Some(Object::Integer(i)) => Some(*i as f64),
        Some(Object::Real(r)) => Some(*r as f64),
        _ => None,
    };
    let opaque_alpha = number(b"CA").is_none_or(|a| a >= 1.0) && number(b"ca").is_none_or(|a| a >= 1.0);
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
    forms: HashMap<Vec<u8>, lopdf::ObjectId>,
}

fn read_resources(doc: &lopdf::Document, resources: Option<&lopdf::Object>) -> Resources {
    use lopdf::Object;
    fn dict<'a>(doc: &'a lopdf::Document, o: Option<&'a Object>) -> Option<&'a lopdf::Dictionary> {
        o.and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_dict().ok())
    }
    let mut out = Resources::default();
    let Some(resources) = dict(doc, resources) else { return out };
    if let Some(states) = dict(doc, resources.get(b"ExtGState").ok()) {
        for (name, state) in states.iter() {
            if state_is_opaque(doc, state) {
                out.opaque.insert(name.clone());
            }
        }
    }
    if let Some(xobjects) = dict(doc, resources.get(b"XObject").ok()) {
        for (name, value) in xobjects.iter() {
            let Ok(id) = value.as_reference() else { continue };
            let is_form = doc
                .get_object(id)
                .and_then(Object::as_stream)
                .is_ok_and(|s| s.dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Form"));
            if is_form {
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
    use lopdf::{Document, Object, ObjectId};
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
            let alpha = match annot.get(b"CA") {
                Ok(Object::Integer(i)) => *i as f64,
                Ok(Object::Real(r)) => *r as f64,
                _ => 1.0,
            };
            let appearance = annot.get(b"AP").ok().and_then(|ap| doc.dereference(ap).ok()).and_then(|(_, ap)| ap.as_dict().ok());
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

    /// A one-page PDF whose page content strokes two lines, with a stamp whose
    /// appearance strokes the same two lines and then draws a form of two more
    /// while see-through.
    fn stamped_pdf() -> Vec<u8> {
        use lopdf::{dictionary, Document, Object, Stream};
        let lines = b"1 1 m 5 5 l S 2 2 m 6 6 l S".to_vec();
        let bbox = || vec![Object::from(0), 0.into(), 10.into(), 10.into()];
        let mut doc = Document::with_version("1.7");
        let inner = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => bbox() }, lines.clone()));
        let faint = doc.add_object(dictionary! { "Type" => "ExtGState", "CA" => Object::Real(0.5) });
        let mut stamp = lines.clone();
        stamp.extend_from_slice(b" q /Faint gs /Inner Do Q");
        let appearance = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => bbox(),
                "Resources" => dictionary! {
                    "ExtGState" => dictionary! { "Faint" => faint },
                    "XObject" => dictionary! { "Inner" => inner },
                },
            },
            stamp,
        ));
        let content = doc.add_object(Stream::new(dictionary! {}, lines));
        let annot = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Stamp", "Rect" => bbox(), "AP" => dictionary! { "N" => appearance } });
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages,
            "MediaBox" => bbox(),
            "Contents" => content,
            "Annots" => vec![Object::from(annot)],
        });
        doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::from(page)], "Count" => 1 }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    /// A one-page PDF with three stamps that draw nothing to merge: one on a
    /// layer that's on, one on a layer that's off, and one shown only while
    /// any of its layers -- just the one that's off -- is on.
    fn layered_pdf() -> Vec<u8> {
        use lopdf::{dictionary, Document, Object, Stream};
        let bbox = || vec![Object::from(0), 0.into(), 10.into(), 10.into()];
        let mut doc = Document::with_version("1.7");
        let shown = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("current") });
        let old = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("old") });
        let mut stamp = |oc: Object| {
            let appearance = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => bbox() }, b"0 0 5 5 re f".to_vec()));
            doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Stamp", "Rect" => bbox(), "AP" => dictionary! { "N" => appearance }, "OC" => oc })
        };
        let visible = stamp(Object::Reference(shown));
        let hidden = stamp(Object::Reference(old));
        let membership = stamp(Object::Dictionary(dictionary! { "Type" => "OCMD", "OCGs" => vec![Object::Reference(old)], "P" => "AnyOn" }));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages,
            "MediaBox" => bbox(),
            "Annots" => vec![Object::from(visible), Object::from(hidden), Object::from(membership)],
        });
        doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::from(page)], "Count" => 1 }));
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages,
            "OCProperties" => dictionary! {
                "OCGs" => vec![Object::Reference(shown), Object::Reference(old)],
                "D" => dictionary! { "OFF" => vec![Object::Reference(old)] },
            },
        });
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn annotations_on_layers_that_are_off_are_taken_out_of_the_copy() {
        let bytes = layered_pdf();
        assert!(may_hide_annotations(&bytes));
        let (copy, stats) = merge_document(&bytes, MOST_PARTS).unwrap().expect("two stamps are hidden");
        assert_eq!((stats.hidden, stats.rewritten), (2, 0), "{stats:?}");
        let doc = lopdf::Document::load_mem(&copy).unwrap();
        let page = *doc.get_pages().get(&1).unwrap();
        let annots = doc.get_page_annotations(page).unwrap();
        assert_eq!(annots.len(), 1, "only the stamp on the layer that's on is left");
        assert!(!may_hide_annotations(&stamped_pdf()), "no layers and no compressed objects");
    }

    #[test]
    fn a_document_has_only_its_opaque_stamp_appearances_merged() {
        let (merged, stats) = merge_document(&stamped_pdf(), MOST_PARTS).unwrap().expect("the stamp has strokes to merge");
        assert_eq!((stats.rewritten, stats.merged, stats.see_through), (1, 1, 1), "{stats:?}");
        let doc = lopdf::Document::load_mem(&merged).unwrap();
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
    fn strings_names_comments_and_inline_images_are_skipped_whole() {
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
