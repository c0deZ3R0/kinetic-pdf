//! A piece of a page lifted out as a PDF of its own, so it can be carried to
//! another page or another file and placed there as a markup.
//!
//! The piece is taken from the page's own content stream rather than drawn:
//! the stream is read once, operator by operator, following the transform
//! and line width, and whatever paints wholly outside the box is left out --
//! paths, text, images, and the insides of forms, which are cut down the same
//! way. Everything else is kept as it was written, and the fonts, images and
//! graphics states it names are copied across byte for byte. Reading a dense
//! sheet's 200,000 operators takes a few milliseconds, where drawing them
//! took a second, and what comes out is the page's own drawing: text stays
//! text and a photo stays the photo it was.
//!
//! Annotations shown on the page are cut down the same way, and what the app
//! draws over the page itself -- markups not saved yet, measurements,
//! highlights -- is laid over it from content streams it hands in.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use pdf_content::layers::Layers;
use pdf_content::lexer::{each_operation, Operand};
use pdf_content::lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use pdf_content::objects::{copy_object, dict, number};

use crate::geometry::Matrix;
use crate::page::{appearance, inherited, placed_page, LEFT_OUT, OFF_SCREEN, OUR_NAMES};
use crate::pdf::{matrix, rectangle};
use crate::shapes::Shapes;

/// Forms inside forms are cut down this deep; deeper ones are copied whole.
const DEEPEST_FORMS: usize = 16;

/// Something drawn over the page that isn't in the file, lifted with it.
pub enum ClipOverlay {
    /// A content stream in the page's user space, the resources it names, and
    /// the tiling patterns -- name, dictionary, cell -- to add to them.
    Content { content: Vec<u8>, resources: Dictionary, patterns: Vec<(String, Dictionary, Vec<u8>)> },
    /// A clip already on the page: its PDF, placed by `placement` from its
    /// own space into the page's user space.
    Pdf { pdf: Arc<Vec<u8>>, placement: Matrix },
}

/// A clip written as a PDF, and how many painting operators were kept and
/// left out.
pub struct Clipped {
    pub pdf: Vec<u8>,
    pub kept: usize,
    pub dropped: usize,
}

/// What a clip takes beyond the page's own drawing, and how it's cut.
#[derive(Default)]
pub struct ClipOptions<'a> {
    /// A polygon in the clip's space: only what's inside it shows, and the
    /// rest of the box is left clear. `None` for the whole box.
    pub outline: Option<&'a [[f32; 2]]>,
    /// Whether the annotations shown on the page come too.
    pub annotations: bool,
    /// Regions of the page, in its user space, erased but not yet written:
    /// its drawing is left out of them, as `erase_page` would.
    pub erased: &'a [Vec<[f32; 2]>],
    /// Regions erased from one layer alone -- an optional content group, by
    /// object number and generation -- but not yet written: only what's on
    /// that layer is left out of them.
    pub layer_erased: &'a [((u32, u16), Vec<[f32; 2]>)],
    /// The one layer to lift, if not all: what's marked as on any other
    /// layer is left out, as if it were off. What's on none comes anyway.
    pub only_layer: Option<(u32, u16)>,
    /// What goes over the page's drawing and its annotations, in order.
    pub overlays: &'a [ClipOverlay],
}

/// Lifts what page `page_number` (from 1) shows within a box `size` points
/// across and up, as a one-page PDF that size, taking what `options` say.
/// `to_clip` takes the page's user space to the box's: the origin at its
/// bottom left, y up.
pub fn clip_page(doc: &Document, page_number: u32, to_clip: Matrix, size: [f32; 2], options: &ClipOptions) -> Result<Clipped, String> {
    let ClipOptions { outline, annotations, erased, layer_erased, only_layer, overlays } = *options;
    if !(size[0] > 0.0 && size[1] > 0.0 && size[0].is_finite() && size[1].is_finite()) {
        return Err("the area has no size".to_owned());
    }
    let (page_id, _, _) = placed_page(doc, page_number)?;
    let mut lifter = Lifter::new(doc, size);
    lifter.only = only_layer;
    lifter.layer_clips = layer_clips(layer_erased);
    let mut ops = format!("0 0 {} {} re W n\n", n(size[0]), n(size[1]));
    if let Some([first, rest @ ..]) = outline.filter(|points| points.len() >= 3) {
        let _ = write!(ops, "{} {} m", n(first[0]), n(first[1]));
        for point in rest {
            let _ = write!(ops, " {} {} l", n(point[0]), n(point[1]));
        }
        ops.push_str(" h W n\n");
    }
    let mut xobjects = Dictionary::new();
    let page_box = ["MediaBox", "CropBox"]
        .into_iter()
        .find_map(|key| inherited(doc, page_id, key.as_bytes()).and_then(|b| rectangle(doc, b)))
        .unwrap_or([-1e5, -1e5, 1e5, 1e5]);

    // The page's own drawing.
    let content = doc.get_page_content(page_id);
    let resources = inherited(doc, page_id, b"Resources").and_then(|r| dict(doc, r));
    let (mut culled, kept) = lifter.cull(&content, resources, to_clip, 0);
    if !erased.is_empty() {
        let mut left = erased_clip(erased).into_bytes();
        left.append(&mut culled);
        left.extend_from_slice(b"
Q
");
        culled = left;
    }
    let page_form = lifter.form(page_box, culled, kept);
    xobjects.set("P", page_form);
    let _ = writeln!(ops, "q {} cm /P Do Q", m(to_clip));

    // The annotations shown over it.
    let annots = doc.get_dictionary(page_id).ok().and_then(|p| p.get(b"Annots").ok()).and_then(|a| doc.dereference(a).ok()).and_then(|(_, a)| a.as_array().ok());
    for (at, annot) in annots.into_iter().flatten().filter_map(|a| dict(doc, a)).enumerate() {
        if !annotations || !lifter.shows_annotation(annot) {
            continue;
        }
        let Some((form, fit)) = appearance(doc, annot) else { continue };
        let placed = fit.then(to_clip);
        if let Some(copied) = lifter.form_copy(appearance_id(doc, annot), form, None, placed, 0) {
            let name = format!("A{at}");
            xobjects.set(name.clone(), copied);
            let _ = writeln!(ops, "q {} cm /{name} Do Q", m(placed));
        }
    }

    // What the app draws over it.
    for (at, overlay) in overlays.iter().enumerate() {
        let name = format!("M{at}");
        match overlay {
            ClipOverlay::Content { content, resources, patterns } => {
                let mut resources = resources.clone();
                if !patterns.is_empty() {
                    let mut named = Dictionary::new();
                    for (pattern, dict, cell) in patterns {
                        let mut stream = Stream::new(dict.clone(), cell.clone());
                        squeeze(&mut stream);
                        named.set(pattern.clone(), lifter.out.add_object(stream));
                    }
                    resources.set("Pattern", named);
                }
                let form = lifter.form(page_box, content.clone(), resources);
                xobjects.set(name.clone(), form);
                let _ = writeln!(ops, "q {} cm /{name} Do Q", m(to_clip));
            }
            ClipOverlay::Pdf { pdf, placement } => {
                let Ok(clip) = Document::load_mem(pdf) else { continue };
                let Some(&clip_page) = clip.get_pages().get(&1) else { continue };
                let Ok(page) = clip.get_dictionary(clip_page) else { continue };
                let content = clip.get_page_content(clip_page);
                let clip_box = page.get(b"MediaBox").ok().and_then(|b| rectangle(&clip, b)).unwrap_or([0.0; 4]);
                let resources = page.get(b"Resources").ok().map_or_else(Dictionary::new, |r| match copy_object(&clip, r, &mut lifter.out, &mut HashMap::new()) {
                    Object::Dictionary(d) => d,
                    Object::Reference(id) => lifter.out.get_dictionary(id).cloned().unwrap_or_default(),
                    _ => Dictionary::new(),
                });
                let form = lifter.form(clip_box, content, resources);
                xobjects.set(name.clone(), form);
                let _ = writeln!(ops, "q {} cm /{name} Do Q", m(placement.then(to_clip)));
            }
        }
    }

    let (kept, dropped) = (lifter.kept, lifter.dropped);
    let mut out = lifter.out;
    let mut content = Stream::new(Dictionary::new(), ops.into_bytes());
    squeeze(&mut content);
    let content = out.add_object(content);
    let pages = out.new_object_id();
    let page = out.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages,
        "MediaBox" => vec![0.into(), 0.into(), Object::Real(size[0]), Object::Real(size[1])],
        "Resources" => dictionary! { "XObject" => xobjects },
        "Contents" => content,
    });
    out.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
    let catalog = out.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    out.trailer.set("Root", catalog);
    let mut pdf = Vec::new();
    out.save_to(&mut pdf).map_err(|e| e.to_string())?;
    Ok(Clipped { pdf, kept, dropped })
}

/// Erases what page `page_number` (from 1) draws within each of `regions`,
/// polygons in the page's user space: its own drawing, not the annotations
/// over it. What paints wholly within a region is taken out of the content
/// stream; what crosses its edge stays, and a clip round the page leaving
/// the regions out stops it at the edge. The page's resources are left as
/// they are. Gives how many painting operators went.
///
/// With `layer`, only what's on that layer -- an optional content group, by
/// object number and generation -- is erased: what paints wholly within a
/// region goes only there, and the clip goes inside that layer's marked
/// content rather than round the page, so every other layer stays whole.
pub fn erase_page(doc: &mut Document, page_number: u32, regions: &[Vec<[f32; 2]>], layer: Option<(u32, u16)>) -> Result<usize, String> {
    let regions: Vec<&Vec<[f32; 2]>> = regions.iter().filter(|region| region.len() >= 3).collect();
    if regions.is_empty() {
        return Ok(0);
    }
    let (page_id, _, _) = placed_page(doc, page_number)?;
    let mut content = doc.get_page_content(page_id);
    let mut dropped = 0;
    let owned: Vec<Vec<[f32; 2]>> = regions.iter().map(|r| (*r).clone()).collect();
    {
        let resources = inherited(doc, page_id, b"Resources").and_then(|r| dict(doc, r));
        for (at, region) in regions.iter().enumerate() {
            // Nothing is hidden when erasing: layers that are off stay in the
            // file, as they were, for whoever turns them on.
            let mut eraser = Lifter { layers: None, mode: Mode::Erase { region: (*region).clone(), only: layer }, ..Lifter::new(doc, [1.0, 1.0]) };
            // A layer's clip goes in once, on the first pass.
            if let (Some(layer), 0) = (layer, at) {
                eraser.layer_clips = HashMap::from([(layer, erased_clip(&owned))]);
            }
            content = eraser.cull(&content, resources, Matrix::IDENTITY, 0).0;
            dropped += eraser.dropped;
        }
    }
    let mut bytes = if layer.is_some() { Vec::new() } else { erased_clip(&owned).into_bytes() };
    bytes.append(&mut content);
    if layer.is_none() {
        bytes.extend_from_slice(b"\nQ\n");
    }
    let mut stream = Stream::new(Dictionary::new(), bytes);
    squeeze(&mut stream);
    let stream = doc.add_object(stream);
    doc.get_dictionary_mut(page_id).map_err(|e| e.to_string())?.set("Contents", stream);
    Ok(dropped)
}

/// Each layer's clip leaving its erased regions out, by the layer.
fn layer_clips(erased: &[((u32, u16), Vec<[f32; 2]>)]) -> HashMap<(u32, u16), String> {
    let mut by_layer: HashMap<(u32, u16), Vec<Vec<[f32; 2]>>> = HashMap::new();
    for (layer, region) in erased {
        by_layer.entry(*layer).or_default().push(region.clone());
    }
    by_layer.into_iter().map(|(layer, regions)| (layer, erased_clip(&regions))).collect()
}

/// A `q` and the clips leaving `regions` out of what's drawn after them: all
/// the page, less each, by the even-odd rule. One clip after another, so
/// where regions overlap both still count. What follows ends with `Q`.
fn erased_clip(regions: &[Vec<[f32; 2]>]) -> String {
    let mut ops = String::from("q
");
    for region in regions.iter().filter(|region| region.len() >= 3) {
        let _ = write!(ops, "-100000 -100000 200000 200000 re {} {} m", n(region[0][0]), n(region[0][1]));
        for point in &region[1..] {
            let _ = write!(ops, " {} {} l", n(point[0]), n(point[1]));
        }
        ops.push_str(" h W* n
");
    }
    ops
}

/// Whether the box `[left, bottom, right, top]` is wholly within `polygon`:
/// its corners are inside, and no edge of the polygon crosses into it.
fn rect_in_polygon([left, bottom, right, top]: [f32; 4], polygon: &[[f32; 2]]) -> bool {
    let inside = |[x, y]: [f32; 2]| {
        let mut inside = false;
        let mut j = polygon.len() - 1;
        for i in 0..polygon.len() {
            let (a, b) = (polygon[i], polygon[j]);
            if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
                inside = !inside;
            }
            j = i;
        }
        inside
    };
    if polygon.len() < 3 || ![[left, bottom], [right, bottom], [right, top], [left, top]].into_iter().all(inside) {
        return false;
    }
    // A concave polygon can hold all four corners and still cut into the box
    // between them: an edge passing through its inside says so.
    let crosses = |a: [f32; 2], b: [f32; 2]| {
        // Liang-Barsky: how much of the edge lies within the box.
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        for (p, q) in [(-dx, a[0] - left), (dx, right - a[0]), (-dy, a[1] - bottom), (dy, top - a[1])] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        t1 - t0 > 1e-4
    };
    !(0..polygon.len()).any(|i| crosses(polygon[i], polygon[(i + 1) % polygon.len()]))
}

/// A clip's shapes, read back from the PDF `clip_page` wrote, and its size
/// in points. Curves are flattened to within `tolerance` and images kept at
/// `image_density`, as `page_shapes` takes them.
pub fn clip_shapes(pdf: &[u8], tolerance: f32, image_density: f32) -> Result<(Shapes, [f32; 2]), String> {
    let doc = Document::load_mem(pdf).map_err(|e| e.to_string())?;
    let &page = doc.get_pages().get(&1).ok_or("the clip has no page")?;
    let media = doc.get_dictionary(page).ok().and_then(|p| p.get(b"MediaBox").ok()).and_then(|b| rectangle(&doc, b)).ok_or("the clip has no size")?;
    let shapes = crate::page::page_shapes(&doc, 1, tolerance, image_density)?;
    Ok((shapes, [media[2] - media[0], media[3] - media[1]]))
}

/// The object an annotation's normal appearance is, for keeping one copy of
/// it however often it's used; `None` if it's written into the annotation.
fn appearance_id(doc: &Document, annot: &Dictionary) -> Option<ObjectId> {
    let normal = dict(doc, annot.get(b"AP").ok()?)?.get(b"N").ok()?;
    match normal {
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Stream(_) => Some(*id),
            Object::Dictionary(by_state) => by_state.get(annot.get(b"AS").ok()?.as_name().ok()?).ok()?.as_reference().ok(),
            _ => None,
        },
        Object::Dictionary(by_state) => by_state.get(annot.get(b"AS").ok()?.as_name().ok()?).ok()?.as_reference().ok(),
        _ => None,
    }
}

/// A box on the clip: left, bottom, right, top.
#[derive(Clone, Copy, Debug)]
struct Bounds([f32; 4]);

impl Bounds {
    const EMPTY: Bounds = Bounds([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY]);

    fn add(&mut self, [x, y]: [f32; 2]) {
        let b = &mut self.0;
        *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }

    /// The box around `[left, bottom, right, top]` placed by `placed`.
    fn of(rect: [f32; 4], placed: Matrix) -> Bounds {
        let mut bounds = Bounds::EMPTY;
        for corner in [[rect[0], rect[1]], [rect[2], rect[1]], [rect[2], rect[3]], [rect[0], rect[3]]] {
            bounds.add(placed.apply(corner));
        }
        bounds
    }

    fn grown(self, by: f32) -> Bounds {
        let [l, b, r, t] = self.0;
        Bounds([l - by, b - by, r + by, t + by])
    }

    fn is_empty(&self) -> bool {
        !(self.0[0] <= self.0[2] && self.0[1] <= self.0[3])
    }
}

/// What the stream reading has got to: the transform and line width, and
/// the text state, which lasts from one text object to the next.
#[derive(Clone, Copy)]
struct Graphics {
    ctm: Matrix,
    width: f32,
    font_size: f32,
    /// Horizontal scaling, as a fraction.
    stretch: f32,
    leading: f32,
    rise: f32,
    char_spacing: f32,
    word_spacing: f32,
    /// Text rendering mode 4 to 7 clips with the text.
    clips_with_text: bool,
}

/// A text object being read, kept or left out whole at `ET`.
struct Text {
    /// Its operators as written.
    all: Vec<u8>,
    /// Those of them that change the state later drawing uses -- colours,
    /// the font, marked content -- which stay if the text itself goes.
    state: Vec<u8>,
    shows: bool,
    matrix: Matrix,
    line: Matrix,
    /// How far along the line the text may have got since the line was last
    /// placed: shown text moves on by its glyphs' widths, which aren't read,
    /// so this is the most it could be.
    slack: f32,
}

/// What a content stream is read for.
enum Mode {
    /// Lifting a clip: what paints within `area`, the box on the clip, is
    /// kept, and copied with what it uses into the clip.
    Lift { area: Bounds },
    /// Erasing: what paints wholly within `region`, a polygon in the
    /// stream's own space, goes -- only on layer `only`, if there's one --
    /// and the rest stays, naming its resources as it did.
    Erase { region: Vec<[f32; 2]>, only: Option<(u32, u16)> },
}

struct Lifter<'d> {
    doc: &'d Document,
    layers: Option<Layers>,
    out: Document,
    mode: Mode,
    /// Objects copied whole, by their number in `doc`.
    copied: HashMap<ObjectId, ObjectId>,
    /// Forms cut down, by their number and where they were drawn.
    culled: HashMap<(ObjectId, [u32; 6]), Option<ObjectId>>,
    kept: usize,
    dropped: usize,
    /// Lifting one layer alone: see `ClipOptions::only_layer`.
    only: Option<(u32, u16)>,
    /// The clips leaving erased regions out of one layer, written just inside
    /// its marked content (`erased_clip`), by the layer.
    layer_clips: HashMap<(u32, u16), String>,
    /// The layer of each marked-content section open, across forms too.
    layer_stack: Vec<Option<(u32, u16)>>,
}

impl<'d> Lifter<'d> {
    fn new(doc: &'d Document, size: [f32; 2]) -> Lifter<'d> {
        Lifter {
            doc,
            layers: Layers::read(doc),
            out: Document::with_version("1.7"),
            // A point's grace either side, for anti-aliasing at the edges.
            mode: Mode::Lift { area: Bounds([-1.0, -1.0, size[0] + 1.0, size[1] + 1.0]) },
            copied: HashMap::new(),
            culled: HashMap::new(),
            kept: 0,
            dropped: 0,
            only: None,
            layer_clips: HashMap::new(),
            layer_stack: Vec::new(),
        }
    }

    /// Whether what paints within `b` stays: lifting, if it reaches into the
    /// box; erasing, unless it's wholly within the region erased.
    fn keeps(&self, b: Bounds) -> bool {
        match &self.mode {
            Mode::Lift { area } => {
                let (a, b) = (area.0, b.0);
                !Bounds(b).is_empty() && b[0] <= a[2] && b[2] >= a[0] && b[1] <= a[3] && b[3] >= a[1]
            }
            // Erasing one layer, what's on any other stays.
            Mode::Erase { only: Some(only), .. } if self.layer_stack.last().copied().flatten() != Some(*only) => true,
            Mode::Erase { region, .. } => b.is_empty() || !rect_in_polygon(b.0, region),
        }
    }

    /// Whether what paints within `b` is wholly in the box, when lifting.
    fn within(&self, b: Bounds) -> bool {
        match &self.mode {
            Mode::Lift { area } => {
                let (a, b) = (area.0, b.0);
                b[0] >= a[0] && b[1] >= a[1] && b[2] <= a[2] && b[3] <= a[3]
            }
            Mode::Erase { .. } => false,
        }
    }

    fn erasing(&self) -> bool {
        matches!(self.mode, Mode::Erase { .. })
    }

    fn visible(&self, oc: &Object) -> bool {
        // Lifting one layer, any other is as good as off.
        if let (Some(only), Object::Reference(group)) = (self.only, oc) {
            if *group != only && self.doc.get_dictionary(*group).is_ok_and(|g| g.get(b"Type").and_then(Object::as_name).is_ok_and(|t| t == b"OCG")) {
                return false;
            }
        }
        self.layers.as_ref().is_none_or(|layers| layers.is_visible(self.doc, oc))
    }

    /// Whether an annotation shows, as `page::is_shown` has it.
    fn shows_annotation(&self, annot: &Dictionary) -> bool {
        if annot.get(b"NM").and_then(Object::as_str).is_ok_and(|nm| nm.starts_with(OUR_NAMES)) {
            return false;
        }
        let kind = annot.get(b"Subtype").and_then(Object::as_name).unwrap_or_default();
        let flags = annot.get(b"F").ok().and_then(|f| number(self.doc, f)).unwrap_or(0.0) as i64;
        !LEFT_OUT.contains(&kind) && flags & OFF_SCREEN == 0 && annot.get(b"OC").map_or(true, |oc| self.visible(oc))
    }

    /// A form with box `bbox` drawing `content` with `resources`, added to
    /// the clip.
    fn form(&mut self, bbox: [f32; 4], content: Vec<u8>, resources: Dictionary) -> ObjectId {
        let dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => bbox.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
            "Resources" => resources,
        };
        let mut stream = Stream::new(dict, content);
        squeeze(&mut stream);
        self.out.add_object(stream)
    }

    /// Form `form` (object `id`, if it's one of its own) drawn at `ctm`, as
    /// the clip needs it: nothing if it paints nowhere in the box, copied
    /// whole if it paints nowhere else, and otherwise cut down to what it
    /// paints there. `inherited` are the resources of what draws it, which a
    /// form without its own uses.
    fn form_copy(&mut self, id: Option<ObjectId>, form: &'d Stream, inherited: Option<&'d Dictionary>, ctm: Matrix, depth: usize) -> Option<Object> {
        if form.dict.get(b"OC").is_ok_and(|oc| !self.visible(oc)) {
            self.dropped += 1;
            return None;
        }
        let placed = form.dict.get(b"Matrix").ok().and_then(|m| matrix(self.doc, m)).unwrap_or(Matrix::IDENTITY).then(ctm);
        let bbox = form.dict.get(b"BBox").ok().and_then(|b| rectangle(self.doc, b));
        let bounds = bbox.map(|b| Bounds::of(b, placed));
        if bounds.is_some_and(|b| !self.keeps(b)) {
            self.dropped += 1;
            return None;
        }
        self.kept += 1;
        // Erasing leaves a form it keeps as it is: the clip round the page
        // hides what of it is in the region.
        if self.erasing() {
            return Some(Object::Null);
        }
        let has_own = form.dict.get(b"Resources").is_ok();
        // Wholly inside: as it is, once, however often it's drawn.
        if let (Some(id), true, true) = (id, bounds.is_some_and(|b| self.within(b)), has_own) {
            return Some(copy_object(self.doc, &Object::Reference(id), &mut self.out, &mut self.copied));
        }
        let key = id.map(|id| (id, placed.0.map(f32::to_bits)));
        if let Some(done) = key.and_then(|key| self.culled.get(&key)) {
            return done.map(Object::Reference);
        }
        let copy = if depth >= DEEPEST_FORMS {
            match id {
                Some(id) => copy_object(self.doc, &Object::Reference(id), &mut self.out, &mut self.copied).as_reference().ok(),
                None => None,
            }
        } else {
            let content = form.decompressed_content().unwrap_or_else(|_| form.content.clone());
            let resources = form.dict.get(b"Resources").ok().and_then(|r| dict(self.doc, r)).or(inherited);
            let (culled, resources) = self.cull(&content, resources, placed, depth + 1);
            let mut dict = Dictionary::new();
            for (key, value) in form.dict.iter() {
                if !matches!(key.as_slice(), b"Resources" | b"Length" | b"Filter" | b"DecodeParms") {
                    dict.set(key.clone(), copy_object(self.doc, value, &mut self.out, &mut self.copied));
                }
            }
            dict.set("Resources", resources);
            let mut stream = Stream::new(dict, culled);
            squeeze(&mut stream);
            Some(self.out.add_object(stream))
        };
        if let Some(key) = key {
            self.culled.insert(key, copy);
        }
        copy.map(Object::Reference)
    }

    /// Content stream `content`, drawn at `ctm` with `resources`, cut down to
    /// what paints within the box, and the resources what's left uses,
    /// copied into the clip.
    fn cull(&mut self, content: &[u8], resources: Option<&'d Dictionary>, ctm: Matrix, depth: usize) -> (Vec<u8>, Dictionary) {
        let doc = self.doc;
        let mut out = Vec::with_capacity(content.len() / 2);
        let mut fonts: HashSet<&[u8]> = HashSet::new();
        let mut xobjects: Vec<(Vec<u8>, Object)> = Vec::new();
        let mut state = Graphics { ctm, width: 1.0, font_size: 0.0, stretch: 1.0, leading: 0.0, rise: 0.0, char_spacing: 0.0, word_spacing: 0.0, clips_with_text: false };
        let mut saved: Vec<Graphics> = Vec::new();
        // The path being built, its box on the clip, and whether it clips.
        let mut path: Vec<u8> = Vec::new();
        let mut path_bounds = Bounds::EMPTY;
        let mut clipping = false;
        let mut text: Option<Text> = None;
        // Marked content open, and whether each was written: one on a layer
        // that is off isn't, nor is anything it paints.
        let mut marked: Vec<bool> = Vec::new();
        // For each, whether a layer's erased regions were clipped out just
        // inside it, to be let go of with a `Q` at its end.
        let mut clipped: Vec<bool> = Vec::new();
        let mut inline_hidden = false;
        // Set once a clip shuts out the whole box: nothing drawn within it
        // shows, so everything up to the `Q` that ends it goes, and with it
        // the `q`s and `Q`s nested inside, counted here.
        let mut shut: Option<usize> = None;
        let mut previous = 0;
        let xobject_of = |name: &[u8]| -> Option<(Option<ObjectId>, &'d Stream)> {
            let object = resources?.get(b"XObject").ok().and_then(|x| dict(doc, x))?.get(name).ok()?;
            let id = object.as_reference().ok();
            Some((id, doc.dereference(object).ok()?.1.as_stream().ok()?))
        };

        each_operation(content, |operator, operands, range| {
            let written = &content[previous..range.end];
            previous = range.end;
            let hidden = marked.iter().any(|shown| !shown);
            let numbers = || operands.iter().filter_map(Operand::number).collect::<Vec<f32>>();
            // Written where it belongs: into the text object being read, as
            // part of what stays if its text goes too when `lasting`.
            let write = |out: &mut Vec<u8>, text: &mut Option<Text>, bytes: &[u8], lasting: bool| match text {
                Some(text) => {
                    text.all.extend_from_slice(bytes);
                    if lasting {
                        text.state.extend_from_slice(bytes);
                    }
                }
                None => out.extend_from_slice(bytes),
            };

            // Within a clip that shuts the box out, only what keeps the
            // saves and marked content paired is followed.
            if let Some(nested) = shut {
                match operator {
                    b"q" => {
                        saved.push(state);
                        shut = Some(nested + 1);
                    }
                    b"Q" => {
                        state = saved.pop().unwrap_or(state);
                        shut = nested.checked_sub(1);
                        if nested == 0 {
                            write(&mut out, &mut text, written, true);
                        }
                    }
                    b"BMC" | b"BDC" => {
                        marked.push(false);
                        clipped.push(false);
                        self.layer_stack.push(self.layer_stack.last().copied().flatten());
                    }
                    b"EMC" => {
                        self.layer_stack.pop();
                        clipped.pop();
                        if marked.pop().is_none_or(|shown| shown) {
                            write(&mut out, &mut text, written, true);
                        }
                    }
                    b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"Do" | b"Tj" | b"TJ" | b"'" | b"\"" | b"sh" => self.dropped += 1,
                    _ => {}
                }
                return;
            }

            // A path is kept or left out as a whole when it's painted.
            match operator {
                b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h" => {
                    let values = numbers();
                    let points: Vec<[f32; 2]> = match operator {
                        b"re" if values.len() == 4 => {
                            let [x, y, w, h] = [values[0], values[1], values[2], values[3]];
                            vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
                        }
                        _ => values.chunks_exact(2).map(|p| [p[0], p[1]]).collect(),
                    };
                    for point in points {
                        path_bounds.add(state.ctm.apply(point));
                    }
                    path.extend_from_slice(written);
                    return;
                }
                b"W" | b"W*" => {
                    clipping = true;
                    path.extend_from_slice(written);
                    return;
                }
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    path.extend_from_slice(written);
                    let stroked = matches!(operator, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
                    let reach = if stroked { state.width.abs() * state.ctm.length_scale() / 2.0 + 1.0 } else { 0.0 };
                    let paints = operator != b"n" && !hidden && self.keeps(path_bounds.grown(reach));
                    if clipping && !self.keeps(path_bounds) {
                        // A clip wholly outside the box: nothing after it
                        // shows until the state is restored.
                        shut = Some(0);
                        self.dropped += 1;
                    } else if clipping || paints {
                        out.extend_from_slice(&path);
                        self.kept += 1;
                    } else {
                        self.dropped += 1;
                    }
                    path.clear();
                    path_bounds = Bounds::EMPTY;
                    clipping = false;
                    return;
                }
                _ => {}
            }
            // Anything else ends a path left unpainted, which is kept as it
            // was rather than guessed at.
            if !path.is_empty() {
                out.append(&mut path);
                path_bounds = Bounds::EMPTY;
                clipping = false;
            }

            match operator {
                b"q" => {
                    saved.push(state);
                    write(&mut out, &mut text, written, true);
                }
                b"Q" => {
                    state = saved.pop().unwrap_or(state);
                    write(&mut out, &mut text, written, true);
                }
                b"cm" => {
                    if let [a, b, c, d, e, f] = numbers()[..] {
                        state.ctm = Matrix([a, b, c, d, e, f]).then(state.ctm);
                    }
                    write(&mut out, &mut text, written, true);
                }
                b"w" => {
                    if let Some(width) = numbers().last() {
                        state.width = *width;
                    }
                    write(&mut out, &mut text, written, true);
                }

                // Text: kept whole if any of it shows in the box.
                b"BT" => {
                    text = Some(Text { all: written.to_vec(), state: Vec::new(), shows: false, matrix: Matrix::IDENTITY, line: Matrix::IDENTITY, slack: 0.0 });
                }
                b"ET" => {
                    if let Some(mut done) = text.take() {
                        if done.shows || state.clips_with_text {
                            done.all.extend_from_slice(written);
                            out.append(&mut done.all);
                            self.kept += 1;
                        } else {
                            // What changes the state stays; `BT` and `ET` and
                            // the text itself go.
                            out.append(&mut done.state);
                            self.dropped += 1;
                        }
                    }
                }
                b"Tf" => {
                    if let (Some(font), Some(size)) = (operands.first().and_then(Operand::name), operands.get(1).and_then(Operand::number)) {
                        fonts.insert(font);
                        state.font_size = size;
                    }
                    write(&mut out, &mut text, written, true);
                }
                b"Tz" | b"TL" | b"Ts" | b"Tc" | b"Tw" | b"Tr" => {
                    if let Some(&value) = numbers().last() {
                        match operator {
                            b"Tz" => state.stretch = value / 100.0,
                            b"TL" => state.leading = value,
                            b"Ts" => state.rise = value,
                            b"Tc" => state.char_spacing = value,
                            b"Tw" => state.word_spacing = value,
                            _ => state.clips_with_text = value >= 4.0,
                        }
                    }
                    write(&mut out, &mut text, written, true);
                }
                b"Td" | b"TD" | b"Tm" | b"T*" => {
                    if let Some(text) = text.as_mut() {
                        let values = numbers();
                        match (operator, &values[..]) {
                            (b"Tm", &[a, b, c, d, e, f]) => text.line = Matrix([a, b, c, d, e, f]),
                            (b"Td" | b"TD", &[x, y]) => {
                                if operator == b"TD" {
                                    state.leading = -y;
                                }
                                text.line = Matrix::translate(x, y).then(text.line);
                            }
                            (b"T*", _) => text.line = Matrix::translate(0.0, -state.leading).then(text.line),
                            _ => {}
                        }
                        text.matrix = text.line;
                        text.slack = 0.0;
                        text.all.extend_from_slice(written);
                    }
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    let Some(open) = text.as_mut() else { return };
                    if operator != b"Tj" && operator != b"TJ" {
                        if operator == b"\"" {
                            let values = numbers();
                            if let [word, char, ..] = values[..] {
                                state.word_spacing = word;
                                state.char_spacing = char;
                            }
                        }
                        open.line = Matrix::translate(0.0, -state.leading).then(open.line);
                        open.matrix = open.line;
                        open.slack = 0.0;
                    }
                    // How many bytes of glyph codes, and how far back any
                    // numbers in a `TJ` move the text on.
                    let (mut bytes, mut moved) = (0usize, 0f32);
                    let mut count = |operand: &Operand| match operand {
                        Operand::Array(items) => {
                            for item in items {
                                match item.number() {
                                    Some(adjustment) => moved += (-adjustment).max(0.0) / 1000.0,
                                    None => bytes += item.bytes().map_or(0, |b| b.len()),
                                }
                            }
                        }
                        other => bytes += other.bytes().map_or(0, |b| b.len()),
                    };
                    operands.iter().for_each(&mut count);
                    let size = state.font_size.abs();
                    // No glyph is wider than its em, and spacing comes on top.
                    let per_code = (size + state.char_spacing.abs() + state.word_spacing.abs()) * state.stretch.abs().max(0.01) * 1.1;
                    let across = bytes as f32 * per_code + moved * size * state.stretch.abs();
                    let reach = [-size, state.rise - size, open.slack + across + size, state.rise + size * 1.5];
                    let shows = size == 0.0 || !hidden && self.keeps(Bounds::of(reach, open.matrix.then(state.ctm)));
                    open.shows |= shows;
                    open.slack += across;
                    open.all.extend_from_slice(written);
                }

                // Images and forms.
                b"Do" => {
                    let Some(name) = operands.last().and_then(Operand::name) else { return };
                    let Some((id, xobject)) = xobject_of(name) else {
                        self.dropped += 1;
                        return;
                    };
                    let copied = if hidden || xobject.dict.get(b"OC").is_ok_and(|oc| !self.visible(oc)) {
                        None
                    } else {
                        match xobject.dict.get(b"Subtype").and_then(Object::as_name) {
                            Ok(b"Form") => self.form_copy(id, xobject, resources, state.ctm, depth),
                            _ if self.keeps(Bounds::of([0.0, 0.0, 1.0, 1.0], state.ctm)) => {
                                self.kept += 1;
                                if self.erasing() {
                                    return write(&mut out, &mut text, written, false);
                                }
                                let object = id.map_or_else(|| Object::Stream(xobject.clone()), Object::Reference);
                                Some(copy_object(doc, &object, &mut self.out, &mut self.copied))
                            }
                            _ => None,
                        }
                    };
                    let Some(copied) = copied else {
                        self.dropped += 1;
                        return;
                    };
                    // One name to one object: a form drawn twice, cut down
                    // differently each time, needs a name for each.
                    let name = match xobjects.iter().find(|(n, _)| n == name) {
                        None => name.to_vec(),
                        Some((_, same)) if *same == copied => name.to_vec(),
                        Some(_) => [name, format!("_{}", xobjects.len()).as_bytes()].concat(),
                    };
                    if !xobjects.iter().any(|(n, _)| *n == name) {
                        xobjects.push((name.clone(), copied));
                    }
                    let mut call = b"\n/".to_vec();
                    call.extend_from_slice(&name);
                    call.extend_from_slice(b" Do");
                    write(&mut out, &mut text, &call, false);
                }
                b"sh" => {
                    if !hidden {
                        write(&mut out, &mut text, written, false);
                    }
                }
                b"BI" | b"ID" | b"EI" => {
                    if operator == b"BI" {
                        inline_hidden = hidden;
                    }
                    if !inline_hidden {
                        write(&mut out, &mut text, written, false);
                    }
                }

                // Marked content on a layer that's off is left out whole.
                b"BMC" | b"BDC" => {
                    let group = match operands {
                        [Operand::Name(b"OC"), Operand::Name(properties)] => resources.and_then(|r| r.get(b"Properties").ok()).and_then(|p| dict(doc, p)).and_then(|p| p.get(properties).ok()),
                        _ => None,
                    };
                    let off = group.is_some_and(|oc| !self.visible(oc));
                    let shown = !hidden && !off;
                    marked.push(shown);
                    // What's inside is on the group's layer, or else still on
                    // the one it's within.
                    let layer = group.and_then(|oc| oc.as_reference().ok()).or(self.layer_stack.last().copied().flatten());
                    self.layer_stack.push(layer);
                    // A layer with regions erased from it alone: they're
                    // clipped out just inside, for what's on it.
                    let clip = group.and_then(|oc| oc.as_reference().ok()).and_then(|id| self.layer_clips.get(&id)).filter(|_| shown).cloned();
                    clipped.push(clip.is_some());
                    if shown {
                        write(&mut out, &mut text, written, true);
                    }
                    if let Some(clip) = clip {
                        write(&mut out, &mut text, format!("\n{clip}").as_bytes(), true);
                    }
                }
                b"EMC" => {
                    self.layer_stack.pop();
                    if clipped.pop().unwrap_or(false) {
                        write(&mut out, &mut text, b"\nQ\n", true);
                    }
                    if marked.pop().is_none_or(|shown| shown) {
                        write(&mut out, &mut text, written, true);
                    }
                }

                // Colours, graphics states and the rest: kept as they are.
                _ => write(&mut out, &mut text, written, true),
            }
        });
        if !path.is_empty() {
            out.append(&mut path);
        }
        if let Some(mut open) = text {
            out.append(&mut open.all);
        }

        // Erasing, the stream goes back where it was, naming what it did.
        if self.erasing() {
            return (out, Dictionary::new());
        }
        // Only the fonts and XObjects left in are copied; the rest of the
        // resources are small, and go across as they are.
        let mut kept = Dictionary::new();
        for (kind, entries) in resources.into_iter().flat_map(Dictionary::iter) {
            match kind.as_slice() {
                b"XObject" => {}
                b"Font" => {
                    let Some(entries) = dict(doc, entries) else { continue };
                    let mut used = Dictionary::new();
                    for (name, font) in entries.iter().filter(|(name, _)| fonts.contains(name.as_slice())) {
                        used.set(name.clone(), copy_object(doc, font, &mut self.out, &mut self.copied));
                    }
                    kept.set("Font", used);
                }
                _ => kept.set(kind.clone(), copy_object(doc, entries, &mut self.out, &mut self.copied)),
            }
        }
        if !xobjects.is_empty() {
            kept.set("XObject", xobjects.into_iter().collect::<Dictionary>());
        }
        (out, kept)
    }
}

/// Compresses `stream`'s content, quickly rather than as small as it can go:
/// a whole sheet's drawing took most of the time lifting it spent at the
/// usual setting, for a file a little smaller. Lossless either way.
fn squeeze(stream: &mut Stream) {
    use std::io::Write as _;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::with_capacity(stream.content.len() / 3), flate2::Compression::fast());
    if encoder.write_all(&stream.content).is_err() {
        return;
    }
    let Ok(squeezed) = encoder.finish() else { return };
    stream.set_content(squeezed);
    stream.dict.set("Filter", "FlateDecode");
    stream.allows_compression = false;
}

/// A matrix for `cm`, to six places: a sheet's coordinates run to
/// thousands of points, so a scale written short moves things visibly.
fn m(matrix: Matrix) -> String {
    let parts: Vec<String> = matrix.0.iter().map(|v| {
        let fixed = format!("{v:.6}");
        let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
        if trimmed == "-0" || trimmed.is_empty() { "0".to_owned() } else { trimmed.to_owned() }
    }).collect();
    parts.join(" ")
}

/// A number in a content stream, to a thousandth of a point.
fn n(v: f32) -> String {
    let fixed = format!("{v:.3}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    if trimmed == "-0" || trimmed.is_empty() { "0".to_owned() } else { trimmed.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page document, 200 points square, drawing `content` with
    /// resources `resources`.
    fn page_of(content: &str, resources: Dictionary) -> Document {
        let mut doc = Document::with_version("1.7");
        let stream = doc.add_object(Stream::new(Dictionary::new(), content.as_bytes().to_vec()));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => stream, "Resources" => resources,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
        });
        doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", catalog);
        doc
    }

    /// The bottom left quarter, 100 points square, lifted out of `doc`.
    fn lifted(doc: &Document) -> (Document, String) {
        let clipped = clip_page(doc, 1, Matrix::IDENTITY, [100.0, 100.0], &ClipOptions { annotations: true, ..Default::default() }).expect("it lifts");
        let clip = Document::load_mem(&clipped.pdf).expect("it reads");
        let page = clip.get_pages()[&1];
        let xobjects = clip.get_dictionary(page).unwrap().get(b"XObject").or_else(|_| {
            clip.get_dictionary(page).unwrap().get(b"Resources").and_then(Object::as_dict).and_then(|r| r.get(b"XObject"))
        });
        let form = xobjects.unwrap().as_dict().unwrap().get(b"P").unwrap().as_reference().unwrap();
        let content = clip.get_object(form).unwrap().as_stream().unwrap().decompressed_content().unwrap();
        (clip, String::from_utf8(content).unwrap())
    }

    #[test]
    fn what_paints_outside_the_box_is_left_out_and_the_rest_kept_as_written() {
        let doc = page_of("1 0 0 RG 2 w 10 10 m 50 50 l S 150 150 m 190 190 l S 0 0 1 rg 90 90 20 20 re f 120 120 m 130 130 l S", Dictionary::new());
        let (_, content) = lifted(&doc);
        assert!(content.contains("10 10 m 50 50 l S"), "{content}");
        assert!(content.contains("90 90 20 20 re f"), "reaching into the box, it's kept");
        assert!(!content.contains("150 150") && !content.contains("120 120"), "{content}");
        assert!(content.contains("1 0 0 RG") && content.contains("0 0 1 rg"), "colours are kept whatever they colour");
    }

    #[test]
    fn a_line_is_kept_by_its_width_and_the_transform_is_followed() {
        // Just past the box, but wide enough to reach into it.
        let doc = page_of("20 w 105 50 m 105 60 l S q 2 0 0 2 0 0 cm 20 20 m 30 30 l S 70 70 m 90 90 l S Q", Dictionary::new());
        let (_, content) = lifted(&doc);
        assert!(content.contains("105 50 m"), "{content}");
        assert!(content.contains("20 20 m 30 30 l S"), "doubled, still inside");
        assert!(!content.contains("70 70 m"), "doubled, width and all, it's out: {content}");
        assert!(content.contains("2 0 0 2 0 0 cm"));
    }

    #[test]
    fn a_clip_is_always_kept_and_text_is_kept_whole_or_left_out_leaving_its_state() {
        let font = dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" };
        let far = dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier" };
        let resources = dictionary! { "Font" => dictionary! { "F1" => font, "F2" => far, "F3" => dictionary! {} } };
        let doc = page_of(
            "0 0 90 90 re W n BT /F1 12 Tf 10 10 Td (near) Tj ET BT 1 0 0 rg /F2 12 Tf 150 150 Td (far away) Tj ET 0 0 5 5 re f",
            resources,
        );
        let (clip, content) = lifted(&doc);
        assert!(content.contains("0 0 90 90 re W n"), "a clip changes what's drawn after it");
        assert!(content.contains("(near) Tj"));
        assert!(!content.contains("far away"), "{content}");
        assert!(content.contains("1 0 0 rg") && content.contains("/F2 12 Tf"), "the text went, what it set stays: {content}");
        let page = clip.get_pages()[&1];
        let form = clip.get_dictionary(page).unwrap().get(b"Resources").unwrap().as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap().get(b"P").unwrap().as_reference().unwrap();
        let fonts = clip.get_object(form).unwrap().as_stream().unwrap().dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
        let names: Vec<&[u8]> = fonts.iter().map(|(name, _)| name.as_slice()).collect();
        assert_eq!(names, [b"F1".as_slice(), b"F2"], "only the fonts named, not F3");
    }

    #[test]
    fn a_form_is_cut_down_and_one_on_a_layer_that_is_off_left_out() {
        let mut doc = page_of("", Dictionary::new());
        let form = doc.add_object(Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 200.into(), 200.into()], "Resources" => dictionary! {} },
            b"10 10 m 20 20 l S 180 180 m 190 190 l S".to_vec(),
        ));
        let layer = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("Old") });
        let catalog = doc.catalog().unwrap().clone();
        let mut catalog = catalog;
        catalog.set("OCProperties", dictionary! { "OCGs" => vec![layer.into()], "D" => dictionary! { "OFF" => vec![layer.into()] } });
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        doc.objects.insert(root, Object::Dictionary(catalog));
        let page = doc.get_pages()[&1];
        let content = doc.add_object(Stream::new(Dictionary::new(), b"/Fm Do /OC /L BDC 30 30 m 40 40 l S EMC 50 50 m 60 60 l S".to_vec()));
        let page_dict = doc.get_dictionary_mut(page).unwrap();
        page_dict.set("Contents", content);
        page_dict.set("Resources", dictionary! { "XObject" => dictionary! { "Fm" => form }, "Properties" => dictionary! { "L" => layer } });
        let (clip, content) = lifted(&doc);
        assert!(!content.contains("30 30"), "on a layer that's off: {content}");
        assert!(!content.contains("BDC") && !content.contains("EMC"), "and its marking with it: {content}");
        assert!(content.contains("50 50 m 60 60 l S"));
        let page = clip.get_pages()[&1];
        let outer = clip.get_dictionary(page).unwrap().get(b"Resources").unwrap().as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap().get(b"P").unwrap().as_reference().unwrap();
        let inner = clip.get_object(outer).unwrap().as_stream().unwrap().dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap().get(b"Fm").unwrap().as_reference().unwrap();
        let form_content = String::from_utf8(clip.get_object(inner).unwrap().as_stream().unwrap().decompressed_content().unwrap()).unwrap();
        assert!(form_content.contains("10 10 m 20 20 l S") && !form_content.contains("180 180"), "{form_content}");
    }

    #[test]
    fn a_lifted_clip_reads_back_as_shapes_in_the_box() {
        let doc = page_of("1 0 0 RG 2 w 10 10 m 50 50 l S 150 150 m 190 190 l S", Dictionary::new());
        // Turned a quarter and moved, the way a turned sheet is lifted.
        let to_clip = Matrix([0.0, 1.0, -1.0, 0.0, 100.0, 0.0]);
        let clipped = clip_page(&doc, 1, to_clip, [100.0, 100.0], &ClipOptions { annotations: true, ..Default::default() }).unwrap();
        let (shapes, size) = clip_shapes(&clipped.pdf, 0.05, 1.0).unwrap();
        assert_eq!(size, [100.0, 100.0]);
        let lines: Vec<_> = shapes.primitives.iter().filter(|p| !shapes.style_of(p).is_triangle()).collect();
        assert_eq!(lines.len(), 1, "{:?}", shapes.primitives);
        assert_eq!((lines[0].points[0], lines[0].points[1]), ([90.0, 10.0], [50.0, 50.0]), "turned with the box");
        assert!(shapes.style_of(lines[0]).clip > 0.0, "and kept within it");
        assert!(clip_page(&doc, 1, to_clip, [0.0, 10.0], &ClipOptions { annotations: true, ..Default::default() }).is_err());
    }

    #[test]
    fn overlays_go_over_the_page() {
        let doc = page_of("0 0 50 50 re f", Dictionary::new());
        let over = ClipOverlay::Content { content: b"/M gs 0 1 0 rg 0 0 20 20 re f".to_vec(), resources: dictionary! { "ExtGState" => dictionary! { "M" => dictionary! { "BM" => "Multiply" } } }, patterns: Vec::new() };
        let clipped = clip_page(&doc, 1, Matrix::IDENTITY, [100.0, 100.0], &ClipOptions { annotations: true, overlays: &[over], ..Default::default() }).unwrap();
        let (shapes, _) = clip_shapes(&clipped.pdf, 0.05, 1.0).unwrap();
        let last = shapes.primitives.last().unwrap();
        assert_eq!(shapes.style_of(last).colour, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(shapes.runs.last().unwrap().blend, crate::shapes::Blend::Multiply);
    }

    #[test]
    fn a_clip_that_shuts_out_the_box_takes_what_it_clips_with_it() {
        // Within a clip far from the box, a line that would be in it shows
        // nowhere; after the clip ends, one does.
        let doc = page_of("q 150 150 10 10 re W n q 1 0 0 RG Q 50 50 m 60 60 l S Q 10 10 m 20 20 l S 150 150 10 10 re W n 30 30 m 40 40 l S", Dictionary::new());
        let (_, content) = lifted(&doc);
        assert!(!content.contains("50 50 m") && !content.contains("150 150"), "{content}");
        assert!(content.contains("10 10 m 20 20 l S"), "{content}");
        assert!(!content.contains("30 30 m"), "shut out to the end of the stream: {content}");
        assert_eq!(content.matches('q').count(), content.matches('Q').count(), "saves still pair: {content}");
    }

    #[test]
    fn an_outline_leaves_the_box_clear_outside_it() {
        // A filled square over the whole box, cut to the triangle below its
        // diagonal.
        let doc = page_of("0 0 100 100 re f", Dictionary::new());
        let triangle = [[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]];
        let clipped = clip_page(&doc, 1, Matrix::IDENTITY, [100.0, 100.0], &ClipOptions { outline: Some(&triangle), annotations: true, ..Default::default() }).unwrap();
        let (shapes, _) = clip_shapes(&clipped.pdf, 0.05, 1.0).unwrap();
        let set = shapes.primitives.iter().find_map(|p| {
            let style = shapes.style_of(p);
            (style.clip > 0.0).then(|| style.clip as usize - 1)
        });
        let corners: Vec<[f32; 2]> = set.map(|set| shapes.clips.sets[set].iter().flat_map(|&s| shapes.clips.vertices[shapes.clips.shapes[s].clone()].to_vec()).collect()).unwrap_or_default();
        assert!(corners.contains(&[100.0, 100.0]) && corners.contains(&[100.0, 0.0]), "clipped to the outline: {corners:?}");
        assert!(!corners.contains(&[0.0, 100.0]) || corners.len() > 6, "the top left corner is outside it");
    }

    /// Page 1's content once `regions` are erased from it.
    fn erased(doc: &mut Document, regions: &[Vec<[f32; 2]>]) -> String {
        erase_page(doc, 1, regions, None).expect("it erases");
        let page = doc.get_pages()[&1];
        String::from_utf8(doc.get_page_content(page)).unwrap()
    }

    #[test]
    fn erasing_takes_out_what_is_inside_and_clips_what_crosses_the_edge() {
        let mut doc = page_of("1 0 0 RG 2 w 20 20 m 40 40 l S 10 10 m 150 150 l S 160 160 m 190 190 l S BT /F1 12 Tf 30 30 Td (gone) Tj ET", Dictionary::new());
        let square = vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]];
        let content = erased(&mut doc, &[square]);
        assert!(!content.contains("20 20 m") && !content.contains("(gone)"), "wholly inside, gone: {content}");
        assert!(content.contains("10 10 m 150 150 l S"), "crossing the edge, kept: {content}");
        assert!(content.contains("160 160 m 190 190 l S"), "outside, kept");
        assert!(content.contains("0 0 m 100 0 l 100 100 l 0 100 l h W* n"), "and clipped out: {content}");
        assert!(content.starts_with("q\n") && content.trim_end().ends_with('Q'), "within a save of its own");
        let shapes = crate::page::page_shapes(&doc, 1, 0.05, 1.0).unwrap();
        assert!(shapes.not_drawn.is_empty(), "{:?}", shapes.not_drawn);
        assert_eq!(shapes.lines, 2, "two lines left to draw");
    }

    /// A page drawing the same lines on two layers, A and B, and the layers.
    fn two_layers() -> (Document, ObjectId, ObjectId) {
        let mut doc = page_of("", Dictionary::new());
        let (a, b) = (doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("A") }), doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("B") }));
        let page = doc.get_pages()[&1];
        let content = doc.add_object(Stream::new(Dictionary::new(), b"/OC /A BDC 10 10 m 20 20 l S EMC\n/OC /B BDC 10 10 m 20 20 l S EMC".to_vec()));
        let page_dict = doc.get_dictionary_mut(page).unwrap();
        page_dict.set("Contents", content);
        page_dict.set("Resources", dictionary! { "Properties" => dictionary! { "A" => a, "B" => b } });
        (doc, a, b)
    }

    #[test]
    fn erasing_one_layer_takes_out_and_clips_only_what_is_on_it() {
        let (mut doc, a, _) = two_layers();
        erase_page(&mut doc, 1, &[vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]]], Some(a)).expect("it erases");
        let content = String::from_utf8(doc.get_page_content(doc.get_pages()[&1])).unwrap();
        let (on_a, on_b) = content.split_once("/OC /B BDC").expect("both layers still marked");
        assert!(!on_a.contains("10 10 m"), "A's line, wholly inside, gone: {content}");
        let (clip, end) = (on_a.find("W* n").unwrap_or(usize::MAX), on_a.rfind("\nQ\n").unwrap_or(0));
        assert!(clip < end && on_a.trim_end().ends_with("EMC"), "and A clipped inside its own marking: {content}");
        assert!(on_b.contains("10 10 m 20 20 l S") && !on_b.contains("W*"), "B as it was: {content}");
        assert!(content.starts_with("/OC /A BDC"), "nothing round the whole page: {content}");
    }

    #[test]
    fn lifting_one_layer_leaves_the_others_out_and_its_own_erasures_clip_only_it() {
        let (doc, a, b) = two_layers();
        let lift = |options: &ClipOptions| {
            let clipped = clip_page(&doc, 1, Matrix::IDENTITY, [100.0, 100.0], options).expect("it lifts");
            let clip = Document::load_mem(&clipped.pdf).expect("it reads");
            let form = clip.objects.values().filter_map(|o| o.as_stream().ok()).find(|s| s.dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Form")).unwrap().decompressed_content().unwrap();
            String::from_utf8(form).unwrap()
        };
        let only_b = lift(&ClipOptions { only_layer: Some(b), ..Default::default() });
        assert!(!only_b.contains("/OC /A") && only_b.contains("/OC /B BDC 10 10 m"), "{only_b}");
        let region = [((a), vec![[0.0, 0.0], [5.0, 0.0], [5.0, 5.0]])];
        let both = lift(&ClipOptions { layer_erased: &region, ..Default::default() });
        let (on_a, on_b) = both.split_once("/OC /B BDC").expect("both lifted");
        assert!(on_a.contains("W* n") && !on_b.contains("W* n"), "A's erasure clips A alone: {both}");
    }

    #[test]
    fn erasing_leaves_layers_that_are_off_as_they_were() {
        let mut doc = page_of("", Dictionary::new());
        let layer = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("Old") });
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let mut catalog = doc.catalog().unwrap().clone();
        catalog.set("OCProperties", dictionary! { "OCGs" => vec![layer.into()], "D" => dictionary! { "OFF" => vec![layer.into()] } });
        doc.objects.insert(root, Object::Dictionary(catalog));
        let page = doc.get_pages()[&1];
        let content = doc.add_object(Stream::new(Dictionary::new(), b"/OC /L BDC 150 150 m 160 160 l S EMC".to_vec()));
        let page_dict = doc.get_dictionary_mut(page).unwrap();
        page_dict.set("Contents", content);
        page_dict.set("Resources", dictionary! { "Properties" => dictionary! { "L" => layer } });
        let content = erased(&mut doc, &[vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]]]);
        assert!(content.contains("/OC /L BDC 150 150 m 160 160 l S EMC"), "{content}");
    }

    #[test]
    fn a_box_in_the_notch_of_an_l_is_not_inside_it() {
        let l = [[0.0, 0.0], [100.0, 0.0], [100.0, 40.0], [40.0, 40.0], [40.0, 100.0], [0.0, 100.0]];
        assert!(rect_in_polygon([10.0, 10.0, 30.0, 30.0], &l));
        assert!(!rect_in_polygon([60.0, 60.0, 80.0, 80.0], &l), "in the notch");
        assert!(!rect_in_polygon([30.0, 30.0, 60.0, 60.0], &l), "its corners in the arms, its middle in the notch");
    }

    #[test]
    fn matrices_keep_six_places() {
        assert_eq!(m(Matrix([0.0123456, -0.0, 1.0, 2.5, -1000.0, 0.0])), "0.012346 0 1 2.5 -1000 0");
    }
}
