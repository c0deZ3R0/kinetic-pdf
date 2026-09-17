//! Drawing content streams: following the graphics state as the operators
//! change it, and turning what they paint into shapes.
//!
//! Drawn: saving and restoring the state, transforms, line widths, dash
//! patterns, line caps and joins, colours in gray, RGB, CMYK, ICC-based,
//! indexed, Separation and DeviceN spaces, alpha and Multiply blending from
//! graphics states, every path and painting operator, text in embedded
//! fonts (filled, stroked or clipping), image XObjects
//! (packed into the atlas once however often they're drawn), forms inside
//! forms, and marked content on layers, which is left out while its layer is
//! off. Everything else is counted in `Shapes::not_drawn`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use lyon_tessellation::{FillRule, FillTessellator};
use pdf_content::layers::Layers;
use pdf_content::lexer::{each_operation_while, Operand};
use pdf_content::lopdf::{Dictionary, Document, Object, Stream};
use pdf_content::objects::{dict, number};

use crate::atlas::Part;
use crate::colour::{space, Space};
use crate::font::Font;
use crate::geometry::{fill, Matrix, Piece};
use crate::image::{self, Raw};
use crate::pdf::{matrix, rectangle};
use crate::shapes::{Blend, Shape, Shapes, Unsupported};
use crate::stroke::{stroke, Cap, Dash, Join, Stroked, Style};
use crate::text::{TextObject, TextState};

/// Forms drawn inside forms go at most this deep, against forms that draw
/// themselves.
const DEEPEST: usize = 16;

/// Images are worth keeping at no more than this many pixels a point of the
/// page they're first drawn on, about 576 dpi: more than the deepest zoom
/// shows, and the stamps in Bluebeam overlays often carry many times it.
/// Pages are read at the density the zoom in use needs (`Interpreter::new`),
/// since a drawing sheet's photos at this one come to 208 MB against 13 MB at
/// fit width.
pub const MOST_IMAGE_DENSITY: f32 = 8.0;

/// Shapes one fill with a tiling pattern may come to, at most, against a fine
/// pattern over a large area coming to more than the GPU should hold. A
/// Bluebeam markup's dotted hatching over a whole site comes to a few hundred
/// thousand.
const MOST_PATTERN_SHAPES: usize = 4_000_000;

/// The graphics state that drawing needs.
#[derive(Clone, Debug)]
struct State {
    ctm: Matrix,
    stroke_space: Space,
    fill_space: Space,
    /// `None` while the colour is one that can't be drawn, a pattern say.
    stroke: Option<[f32; 3]>,
    fill: Option<[f32; 3]>,
    /// The tiling pattern filling with, while the fill colour is one: its
    /// stream's address in `Interpreter::patterns`.
    fill_pattern: Option<usize>,
    /// Where pattern space starts: the transform when the content stream
    /// began, which a pattern's own matrix is applied to.
    pattern_space: Matrix,
    stroke_alpha: f32,
    fill_alpha: f32,
    /// Line width, caps, joins and dashes, in user space.
    style: Style,
    blend: Blend,
    /// The clip set painting stays within (see `Clips`); `None` for anywhere.
    clip: Option<usize>,
    text: TextState,
}

impl State {
    fn new(ctm: Matrix) -> State {
        State {
            ctm,
            stroke_space: Space::Gray,
            fill_space: Space::Gray,
            stroke: Some([0.0; 3]),
            fill: Some([0.0; 3]),
            fill_pattern: None,
            pattern_space: ctm,
            stroke_alpha: 1.0,
            fill_alpha: 1.0,
            style: Style::default(),
            blend: Blend::Normal,
            clip: None,
            text: TextState::default(),
        }
    }
}

/// Where one content stream has got to.
struct Frame<'d> {
    state: State,
    saved: Vec<State>,
    resources: Option<&'d Dictionary>,
    /// The path being built, in page space, and its current point and
    /// subpath start.
    path: Vec<Piece>,
    current: [f32; 2],
    start: [f32; 2],
    /// The clip the next painting operator sets, by its fill rule.
    clipping: Option<FillRule>,
    /// For each marked-content section open, whether it's visible.
    marked: Vec<bool>,
    depth: usize,
    text: TextObject,
    /// The outlines of glyphs shown in a clipping mode since the text object
    /// began, in page space, clipped to when it ends.
    text_clip: Vec<Piece>,
}

impl Frame<'_> {
    fn place(&self, x: f32, y: f32) -> [f32; 2] {
        self.state.ctm.apply([x, y])
    }

    /// Whether what's painted now is inside marked content on a layer that's
    /// off.
    fn hidden(&self) -> bool {
        self.marked.contains(&false)
    }
}

/// The last `N` operands as numbers, or `None` if any of them isn't one.
fn last_numbers<const N: usize>(operands: &[Operand]) -> Option<[f32; N]> {
    let start = operands.len().checked_sub(N)?;
    let mut out = [0.0; N];
    for (slot, operand) in out.iter_mut().zip(&operands[start..]) {
        *slot = operand.number()?;
    }
    Some(out)
}

/// Reads content streams from a document into shapes.
pub struct Interpreter<'d> {
    doc: &'d Document,
    layers: Option<Layers>,
    tolerance: f32,
    /// Pixels a page point images are kept at; see `MOST_IMAGE_DENSITY`.
    image_density: f32,
    tessellator: FillTessellator,
    /// Fonts loaded, or why they couldn't be, by their dictionary's address.
    fonts: HashMap<usize, Result<Font, Unsupported>>,
    /// Images in the atlas, or why they couldn't be put there, by their
    /// stream's address and, for an image mask, the colour it's painted in.
    images: HashMap<(usize, Option<[u32; 3]>), Result<Vec<Part>, Unsupported>>,
    /// Images whose samples were read before drawing started
    /// (`decode_ahead`), by their stream's address, until each is drawn and
    /// goes into the atlas.
    decoded: HashMap<usize, Result<Raw, Unsupported>>,
    /// Colour spaces read from the resources, by their object's address: a
    /// spot ink's tint transform is a compressed stream, and a CAD sheet sets
    /// its colour space tens of thousands of times.
    spaces: HashMap<usize, Space>,
    /// Tiling patterns set as the fill colour, by their stream's address.
    patterns: HashMap<usize, &'d Stream>,
    /// Each pattern's cell read into shapes, by the pattern's address and
    /// where the cell is placed, or why it can't be.
    cells: HashMap<(usize, [u32; 6]), Result<Vec<Shape>, Unsupported>>,
    /// Set from another thread to have reading stop where it has got to.
    stop: Option<&'d AtomicBool>,
    pub shapes: Shapes,
}

impl<'d> Interpreter<'d> {
    /// Curves are flattened to within `tolerance` points, and images kept at
    /// no more than `image_density` pixels a page point (`MOST_IMAGE_DENSITY`
    /// is as dense as any zoom shows).
    pub fn new(doc: &'d Document, tolerance: f32, image_density: f32) -> Self {
        Interpreter {
            doc,
            layers: Layers::read(doc),
            tolerance,
            image_density: image_density.clamp(0.1, MOST_IMAGE_DENSITY),
            tessellator: FillTessellator::new(),
            fonts: HashMap::new(),
            images: HashMap::new(),
            decoded: HashMap::new(),
            spaces: HashMap::new(),
            patterns: HashMap::new(),
            cells: HashMap::new(),
            stop: None,
            shapes: Shapes::default(),
        }
    }

    /// Stops reading, between one operator and the next, once `flag` is set:
    /// a page of two million objects takes seconds to read, and a read nobody
    /// is waiting for shouldn't hold up one somebody is.
    pub fn stop_when(&mut self, flag: &'d AtomicBool) {
        self.stop = Some(flag);
    }

    /// Whether reading was told to stop, so the shapes are incomplete.
    pub fn stopped(&self) -> bool {
        self.stop.is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Undoes the compression of `images` on every core before drawing starts,
    /// rather than one at a time as the content stream reaches them: reading
    /// one image has nothing to do with the rest, and a drawing sheet's photos
    /// took most of the time preparing it. Their pixels are made as each is
    /// drawn, at the size it's drawn at, and go into the atlas in the page's
    /// own painting order, so what comes out is the same either way. Image
    /// masks aren't read here: they're painted in whatever fill colour is in
    /// force where they're drawn.
    pub fn decode_ahead(&mut self, images: &[&'d Stream]) {
        if images.is_empty() {
            return;
        }
        let (doc, stop) = (self.doc, self.stop);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(images.len());
        // Taken one at a time, so one huge photo doesn't leave a thread with
        // all the work while the others have finished.
        let decode_each = || {
            let mut decoded = Vec::new();
            loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(image) = images.get(index).filter(|_| !stop.is_some_and(|flag| flag.load(Ordering::Relaxed))) else { return decoded };
                decoded.push((*image as *const Stream as usize, image::raw(doc, image)));
            }
        };
        let decoded: Vec<(usize, Result<Raw, Unsupported>)> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..threads).map(|_| scope.spawn(decode_each)).collect();
            threads.into_iter().filter_map(|thread| thread.join().ok()).flatten().collect()
        });
        self.decoded.extend(decoded);
    }

    /// The document it reads.
    pub fn document(&self) -> &'d Document {
        self.doc
    }

    /// Whether optional content `oc` is on when the document opens.
    pub fn is_visible(&self, oc: &Object) -> bool {
        self.layers.as_ref().is_none_or(|layers| layers.is_visible(self.doc, oc))
    }

    /// Draws `content` using `resources`, from page space transformed by `ctm`.
    pub fn draw(&mut self, content: &[u8], resources: Option<&'d Dictionary>, ctm: Matrix) {
        self.run(content, resources, State::new(ctm), 0);
    }

    /// Draws form XObject `form` -- its matrix applied, its resources or
    /// else `inherited` -- from page space transformed by `ctm`.
    pub fn draw_form(&mut self, form: &'d Stream, inherited: Option<&'d Dictionary>, ctm: Matrix) {
        self.form(form, inherited, State::new(ctm), 0);
    }

    fn form(&mut self, form: &'d Stream, inherited: Option<&'d Dictionary>, mut state: State, depth: usize) {
        if depth > DEEPEST {
            self.shapes.not_drawn("forms nested too deep");
            return;
        }
        if let Some(form_matrix) = form.dict.get(b"Matrix").ok().and_then(|m| matrix(self.doc, m)) {
            state.ctm = form_matrix.then(state.ctm);
        }
        // What a form draws stays within its box.
        if let Some([left, bottom, right, top]) = form.dict.get(b"BBox").ok().and_then(|b| rectangle(self.doc, b)) {
            let [a, b, c, d] = [[left, bottom], [right, bottom], [right, top], [left, top]].map(|corner| state.ctm.apply(corner));
            state.clip = Some(self.shapes.clips.intersect(state.clip, &[[a, b, c], [a, c, d]]));
        }
        if form.dict.get(b"Group").is_ok() {
            self.shapes.not_drawn("transparency groups");
        }
        let resources = form.dict.get(b"Resources").ok().and_then(|r| dict(self.doc, r)).or(inherited);
        let Ok(content) = form.get_plain_content() else {
            self.shapes.not_drawn("unreadable streams");
            return;
        };
        self.run(&content, resources, state, depth);
    }

    fn run(&mut self, content: &[u8], resources: Option<&'d Dictionary>, mut state: State, depth: usize) {
        state.pattern_space = state.ctm;
        let mut frame = Frame {
            state,
            saved: Vec::new(),
            resources,
            path: Vec::new(),
            current: [0.0; 2],
            start: [0.0; 2],
            clipping: None,
            marked: Vec::new(),
            depth,
            text: TextObject::START,
            text_clip: Vec::new(),
        };
        each_operation_while(content, |operator, operands, _| {
            if self.stopped() {
                return false;
            }
            self.operate(&mut frame, operator, operands);
            true
        });
    }

    fn operate(&mut self, frame: &mut Frame<'d>, operator: &[u8], operands: &[Operand]) {
        let state = &mut frame.state;
        match operator {
            b"q" => frame.saved.push(state.clone()),
            b"Q" => {
                if let Some(saved) = frame.saved.pop() {
                    frame.state = saved;
                }
            }
            b"cm" => {
                if let Some(m) = last_numbers(operands) {
                    state.ctm = Matrix(m).then(state.ctm);
                }
            }
            b"w" => {
                if let Some([width]) = last_numbers(operands) {
                    state.style.width = width;
                }
            }
            b"J" => {
                if let Some([cap]) = last_numbers(operands) {
                    state.style.cap = Cap::numbered(cap);
                }
            }
            b"j" => {
                if let Some([join]) = last_numbers(operands) {
                    state.style.join = Join::numbered(join);
                }
            }
            b"M" => {
                if let Some([limit]) = last_numbers(operands) {
                    state.style.miter_limit = limit;
                }
            }
            b"d" => {
                if let [Operand::Array(lengths), phase] = operands {
                    state.style.dash = Dash::new(lengths.iter().filter_map(Operand::number).collect(), phase.number().unwrap_or(0.0));
                }
            }
            b"gs" => {
                let graphics_state = operands.last().and_then(Operand::name).and_then(|name| self.resource(frame.resources, b"ExtGState", name));
                if let Some(graphics_state) = graphics_state {
                    self.apply_state(&mut frame.state, graphics_state);
                }
            }

            // Colours.
            b"CS" | b"cs" => {
                let space = operands.last().and_then(Operand::name).map_or(Space::Unsupported, |name| self.named_space(frame.resources, name));
                let colour = space.initial();
                if operator == b"CS" {
                    (state.stroke_space, state.stroke) = (space, colour);
                } else {
                    (state.fill_space, state.fill, state.fill_pattern) = (space, colour, None);
                }
            }
            b"SC" | b"SCN" | b"sc" | b"scn" => {
                let values: Vec<f32> = operands.iter().filter_map(Operand::number).collect();
                let stroking = operator.starts_with(b"S");
                let space = if stroking { &state.stroke_space } else { &state.fill_space };
                // A pattern's name comes last. Filling with a tiling pattern is
                // drawn (`fill_with_pattern`); stroking with one isn't yet.
                let pattern = operands.last().and_then(Operand::name);
                let colour = if pattern.is_some() { None } else { space.colour(&values) };
                if !stroking {
                    let found = pattern.and_then(|name| self.resource(frame.resources, b"Pattern", name)).and_then(|p| self.doc.dereference(p).ok()).and_then(|(_, p)| p.as_stream().ok());
                    state.fill_pattern = found.map(|stream| {
                        let key = stream as *const Stream as usize;
                        self.patterns.insert(key, stream);
                        key
                    });
                }
                *(if stroking { &mut state.stroke } else { &mut state.fill }) = colour;
            }
            b"G" | b"g" | b"RG" | b"rg" | b"K" | b"k" => {
                let space = match operator {
                    b"G" | b"g" => Space::Gray,
                    b"RG" | b"rg" => Space::Rgb,
                    _ => Space::Cmyk,
                };
                let values: Vec<f32> = operands.iter().filter_map(Operand::number).collect();
                let colour = space.colour(&values);
                if operator[0].is_ascii_uppercase() {
                    (state.stroke_space, state.stroke) = (space, colour);
                } else {
                    (state.fill_space, state.fill, state.fill_pattern) = (space, colour, None);
                }
            }

            // Paths.
            b"m" => {
                if let Some([x, y]) = last_numbers(operands) {
                    let at = frame.place(x, y);
                    frame.path.push(Piece::Move(at));
                    (frame.current, frame.start) = (at, at);
                }
            }
            b"l" => {
                if let Some([x, y]) = last_numbers(operands) {
                    let at = frame.place(x, y);
                    frame.path.push(Piece::Line(at));
                    frame.current = at;
                }
            }
            b"c" | b"v" | b"y" => {
                let points: Option<[[f32; 2]; 3]> = match operator {
                    b"c" => last_numbers(operands).map(|[x1, y1, x2, y2, x3, y3]| [frame.place(x1, y1), frame.place(x2, y2), frame.place(x3, y3)]),
                    b"v" => last_numbers(operands).map(|[x2, y2, x3, y3]| [frame.current, frame.place(x2, y2), frame.place(x3, y3)]),
                    _ => last_numbers(operands).map(|[x1, y1, x3, y3]| {
                        let end = frame.place(x3, y3);
                        [frame.place(x1, y1), end, end]
                    }),
                };
                if let Some([c1, c2, end]) = points {
                    frame.path.push(Piece::Curve(c1, c2, end));
                    frame.current = end;
                }
            }
            b"h" => {
                frame.path.push(Piece::Close);
                frame.current = frame.start;
            }
            b"re" => {
                if let Some([x, y, w, h]) = last_numbers(operands) {
                    let corner = frame.place(x, y);
                    frame.path.extend([Piece::Move(corner), Piece::Line(frame.place(x + w, y)), Piece::Line(frame.place(x + w, y + h)), Piece::Line(frame.place(x, y + h)), Piece::Close]);
                    (frame.current, frame.start) = (corner, corner);
                }
            }
            b"W" => frame.clipping = Some(FillRule::NonZero),
            b"W*" => frame.clipping = Some(FillRule::EvenOdd),
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => self.paint(frame, operator),

            // Text.
            b"BT" => {
                frame.text = TextObject::START;
                frame.text_clip.clear();
            }
            b"ET" => {
                let outline = std::mem::take(&mut frame.text_clip);
                if !outline.is_empty() {
                    self.clip(&mut frame.state, &outline, FillRule::NonZero);
                }
            }
            b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Ts" | b"Tr" => {
                if let Some([value]) = last_numbers(operands) {
                    let text = &mut state.text;
                    match operator {
                        b"Tc" => text.char_spacing = value,
                        b"Tw" => text.word_spacing = value,
                        b"Tz" => text.scale = value / 100.0,
                        b"TL" => text.leading = value,
                        b"Ts" => text.rise = value,
                        _ => text.mode = value.clamp(0.0, 7.0) as u8,
                    }
                }
            }
            b"Tf" => {
                if let (Some(name), Some([size])) = (operands.first().and_then(Operand::name), last_numbers(operands)) {
                    state.text.size = size;
                    state.text.font = self.font(frame.resources, name);
                }
            }
            b"Td" | b"TD" => {
                if let Some([x, y]) = last_numbers(operands) {
                    if operator == b"TD" {
                        state.text.leading = -y;
                    }
                    frame.text.next_line(x, y);
                }
            }
            b"Tm" => {
                if let Some(m) = last_numbers(operands) {
                    frame.text.set(Matrix(m));
                }
            }
            b"T*" => frame.text.next_line(0.0, -state.text.leading),
            b"Tj" => self.show(frame, operands.last().map(std::slice::from_ref).unwrap_or_default()),
            b"TJ" => {
                if let Some(Operand::Array(items)) = operands.last() {
                    self.show(frame, items);
                }
            }
            b"'" | b"\"" => {
                if let [.., word_spacing, char_spacing, _] = operands {
                    if let (b"\"", Some(word), Some(char)) = (operator, word_spacing.number(), char_spacing.number()) {
                        (state.text.word_spacing, state.text.char_spacing) = (word, char);
                    }
                }
                frame.text.next_line(0.0, -frame.state.text.leading);
                self.show(frame, operands.last().map(std::slice::from_ref).unwrap_or_default());
            }

            // Forms, and what isn't drawn yet.
            b"Do" => {
                if let Some(name) = operands.last().and_then(Operand::name) {
                    self.draw_xobject(frame, name);
                }
            }
            b"sh" if !frame.hidden() => self.shapes.not_drawn("shadings"),
            b"BI" if !frame.hidden() => self.shapes.not_drawn("inline images"),

            // Marked content, on layers or not.
            b"BMC" => frame.marked.push(true),
            b"BDC" => {
                let visible = match operands {
                    [Operand::Name(b"OC"), Operand::Name(properties)] => {
                        self.resource(frame.resources, b"Properties", properties).is_none_or(|oc| self.is_visible(oc))
                    }
                    _ => true,
                };
                frame.marked.push(visible);
            }
            b"EMC" => {
                frame.marked.pop();
            }
            _ => {}
        }
    }

    /// Named resource `name` of kind `kind` (`ExtGState`, `XObject`, ...).
    fn resource(&self, resources: Option<&'d Dictionary>, kind: &[u8], name: &[u8]) -> Option<&'d Object> {
        resources?.get(kind).ok().and_then(|r| dict(self.doc, r))?.get(name).ok()
    }

    /// Paints the path built so far, as `operator` says, then starts a new
    /// one.
    fn paint(&mut self, frame: &mut Frame<'d>, operator: &[u8]) {
        let mut outline = std::mem::take(&mut frame.path);
        let clipping = frame.clipping.take();
        if matches!(operator, b"s" | b"b" | b"b*") {
            outline.push(Piece::Close);
        }
        if !outline.is_empty() && !frame.hidden() {
            self.paint_outline(&frame.state, &outline, operator);
        }
        // A clip takes effect after the path that sets it is painted.
        if let Some(rule) = clipping {
            self.clip(&mut frame.state, &outline, rule);
        }
    }

    /// Clips `state` to the area `outline` fills by `rule`, within its clip.
    fn clip(&mut self, state: &mut State, outline: &[Piece], rule: FillRule) {
        match fill(outline, rule, self.tolerance, &mut self.tessellator) {
            Some(triangles) => state.clip = Some(self.shapes.clips.intersect(state.clip, &triangles)),
            None => self.shapes.not_drawn("clips that wouldn't tessellate"),
        }
    }

    /// The font resource `name`, loaded the first time it's set; its key in
    /// `fonts`.
    fn font(&mut self, resources: Option<&'d Dictionary>, name: &[u8]) -> Option<usize> {
        let font = dict(self.doc, self.resource(resources, b"Font", name)?)?;
        let key = font as *const Dictionary as usize;
        let doc = self.doc;
        self.fonts.entry(key).or_insert_with(|| Font::load(doc, font));
        Some(key)
    }

    /// Shows `items` -- strings, and in a `TJ` array numbers moving between
    /// them -- in the text state's font, glyph by glyph along the line.
    fn show(&mut self, frame: &mut Frame<'d>, items: &[Operand]) {
        let text = frame.state.text;
        let Interpreter { fonts, shapes, tessellator, tolerance, .. } = self;
        let font = match text.font.and_then(|key| fonts.get_mut(&key)) {
            Some(Ok(font)) => font,
            Some(Err(why)) => return shapes.not_drawn(*why),
            None => return shapes.not_drawn("text without a font"),
        };
        let hidden = frame.hidden();
        for item in items {
            if let Some(adjustment) = item.number() {
                frame.text.along(text.adjust(adjustment));
                continue;
            }
            let Some(bytes) = item.bytes() else { continue };
            for code in font.codes(&bytes) {
                if !hidden && code.glyph != 0 && text.mode != 3 {
                    let placed = text.glyph_matrix(frame.text.matrix, frame.state.ctm);
                    if text.fills() {
                        let triangles = font.triangles(code.glyph, *tolerance / placed.length_scale().max(f32::EPSILON), tessellator);
                        fill_triangles(shapes, &frame.state, triangles.iter().map(|t| t.map(|p| placed.apply(p))));
                    }
                    if text.strokes() || text.clips() {
                        let outline: Vec<Piece> = font.outline(code.glyph).into_iter().map(|p| p.transformed(placed)).collect();
                        if text.strokes() {
                            stroke_outline(shapes, &frame.state, &outline, *tolerance);
                        }
                        if text.clips() {
                            frame.text_clip.extend(outline);
                        }
                    }
                }
                frame.text.along(text.advance(code.width, code.is_space));
            }
        }
    }

    /// Fills and strokes `outline` as painting operator `operator` says, in
    /// `state`.
    fn paint_outline(&mut self, state: &State, outline: &[Piece], operator: &[u8]) {
        let (rule, stroked) = match operator {
            b"S" | b"s" => (None, true),
            b"f" | b"F" => (Some(FillRule::NonZero), false),
            b"f*" => (Some(FillRule::EvenOdd), false),
            b"B" | b"b" => (Some(FillRule::NonZero), true),
            b"B*" | b"b*" => (Some(FillRule::EvenOdd), true),
            _ => (None, false),
        };
        if let Some(rule) = rule {
            match (state.fill, state.fill_pattern) {
                (None, Some(pattern)) => self.fill_with_pattern(state, outline, rule, pattern),
                _ => match fill(outline, rule, self.tolerance, &mut self.tessellator) {
                    Some(triangles) => fill_triangles(&mut self.shapes, state, triangles),
                    None => self.shapes.not_drawn("fills that wouldn't tessellate"),
                },
            }
        }
        if stroked {
            stroke_outline(&mut self.shapes, state, outline, self.tolerance);
        }
    }

    /// Fills `outline` by `rule` with tiling pattern `pattern`: its cell is
    /// read into shapes once (`pattern_cell`), and a copy of them put down in
    /// every tile over the area, within it. Only coloured patterns -- whose
    /// cells set their own colours -- in pattern space that isn't turned or
    /// skewed are drawn. Bluebeam fills its area markups this way, so a page
    /// with one of them went to pdfium whole, which took seconds at every step
    /// of a zoom.
    fn fill_with_pattern(&mut self, state: &State, outline: &[Piece], rule: FillRule, pattern: usize) {
        let Some(&stream) = self.patterns.get(&pattern) else { return self.shapes.not_drawn("fills in colours not drawn yet") };
        let doc = self.doc;
        let dict = &stream.dict;
        let value = |key: &[u8]| dict.get(key).ok().and_then(|v| number(doc, v)).map(|v| v as f32);
        if value(b"PatternType") != Some(1.0) {
            return self.shapes.not_drawn("shading patterns");
        }
        if value(b"PaintType") != Some(1.0) {
            return self.shapes.not_drawn("uncoloured tiling patterns");
        }
        let (Some(x_step), Some(y_step), Some(bbox)) = (value(b"XStep"), value(b"YStep"), dict.get(b"BBox").ok().and_then(|b| rectangle(doc, b))) else {
            return self.shapes.not_drawn("tiling patterns that are malformed");
        };
        let placed = dict.get(b"Matrix").ok().and_then(|m| matrix(doc, m)).unwrap_or(Matrix::IDENTITY).then(state.pattern_space);
        let [a, b, c, d, ..] = placed.0;
        if b.abs() > 1e-6 || c.abs() > 1e-6 || x_step <= 0.0 || y_step <= 0.0 {
            return self.shapes.not_drawn("tiling patterns turned, skewed or stepping backwards");
        }
        let Some(triangles) = fill(outline, rule, self.tolerance, &mut self.tessellator) else {
            return self.shapes.not_drawn("fills that wouldn't tessellate");
        };
        let Some(to_pattern) = placed.inverse() else { return };
        if triangles.is_empty() {
            return;
        }
        let cell = match self.pattern_cell(stream, placed, bbox) {
            Ok(cell) => cell,
            Err(why) => return self.shapes.not_drawn(why),
        };
        // The tiles the area reaches into, in pattern space: those overlapping
        // it, not only touching it.
        let (mut low, mut high) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for corner in triangles.iter().flatten() {
            let [u, v] = to_pattern.apply(*corner);
            low = [low[0].min(u), low[1].min(v)];
            high = [high[0].max(u), high[1].max(v)];
        }
        let columns = ((low[0] - bbox[2]) / x_step).floor() as i64 + 1..=((high[0] - bbox[0]) / x_step).ceil() as i64 - 1;
        let rows = ((low[1] - bbox[3]) / y_step).floor() as i64 + 1..=((high[1] - bbox[1]) / y_step).ceil() as i64 - 1;
        let tiles = (columns.end() - columns.start() + 1).max(0) as usize * (rows.end() - rows.start() + 1).max(0) as usize;
        if tiles.saturating_mul(cell.len()) > MOST_PATTERN_SHAPES {
            return self.shapes.not_drawn("tiling patterns too fine for their area");
        }
        let clip = Some(self.shapes.clips.intersect(state.clip, &triangles));
        for row in rows {
            for column in columns.clone() {
                let [dx, dy] = [a * column as f32 * x_step, d * row as f32 * y_step];
                for shape in &cell {
                    let mut shape = *shape;
                    shape.points = shape.points.map(|[x, y]| [x + dx, y + dy]);
                    shape.colour[3] *= state.fill_alpha;
                    self.shapes.push(shape, state.blend, clip);
                }
            }
        }
    }

    /// Pattern `pattern`'s cell -- its content stream, placed by `placed` --
    /// as the shapes it paints, cut to its box `bbox`, as a tile would draw
    /// them at the origin. `Err` if the cell draws anything a copy of its
    /// shapes can't stand for: images, clips, blending, or what can't be
    /// drawn at all.
    fn pattern_cell(&mut self, pattern: &'d Stream, placed: Matrix, bbox: [f32; 4]) -> Result<Vec<Shape>, Unsupported> {
        let key = (pattern as *const Stream as usize, placed.0.map(f32::to_bits));
        if let Some(cell) = self.cells.get(&key) {
            return cell.clone();
        }
        let resources = pattern.dict.get(b"Resources").ok().and_then(|r| dict(self.doc, r));
        let cell = match pattern.get_plain_content() {
            Err(_) => Err("unreadable streams"),
            Ok(content) => {
                // Read into shapes of their own, and without leaving images
                // behind that would point into their atlas.
                let outer = std::mem::take(&mut self.shapes);
                let images: std::collections::HashSet<_> = self.images.keys().copied().collect();
                self.run(&content, resources, State::new(placed), 1);
                self.images.retain(|key, _| images.contains(key));
                let inner = std::mem::replace(&mut self.shapes, outer);
                let plain = inner.not_drawn.is_empty()
                    && inner.images == 0
                    && inner.clips.sets.is_empty()
                    && inner.runs.iter().all(|run| run.blend == Blend::Normal && run.clip.is_none());
                if plain {
                    let [left, bottom] = placed.apply([bbox[0], bbox[1]]);
                    let [right, top] = placed.apply([bbox[2], bbox[3]]);
                    let within = [left.min(right), bottom.min(top), left.max(right), bottom.max(top)];
                    Ok(inner.primitives.iter().flat_map(|primitive| cut_to(within, &inner, primitive)).collect())
                } else {
                    Err("tiling patterns with images, clips or blending")
                }
            }
        };
        self.cells.insert(key, cell.clone());
        cell
    }

    /// Applies an `ExtGState` dictionary to `state`.
    fn apply_state(&mut self, state: &mut State, graphics_state: &Object) {
        let Some(graphics_state) = dict(self.doc, graphics_state) else { return };
        let value = |key: &[u8]| graphics_state.get(key).ok().and_then(|v| number(self.doc, v)).map(|v| v as f32);
        if let Some(alpha) = value(b"CA") {
            state.stroke_alpha = alpha;
        }
        if let Some(alpha) = value(b"ca") {
            state.fill_alpha = alpha;
        }
        if let Some(width) = value(b"LW") {
            state.style.width = width;
        }
        if let Some(cap) = value(b"LC") {
            state.style.cap = Cap::numbered(cap);
        }
        if let Some(join) = value(b"LJ") {
            state.style.join = Join::numbered(join);
        }
        if let Some(limit) = value(b"ML") {
            state.style.miter_limit = limit;
        }
        // `[[lengths] phase]`.
        let dash = graphics_state.get(b"D").ok().and_then(|d| self.doc.dereference(d).ok()).and_then(|(_, d)| d.as_array().ok());
        if let Some([lengths, phase]) = dash.map(Vec::as_slice) {
            let lengths = self.doc.dereference(lengths).ok().and_then(|(_, l)| l.as_array().ok());
            let lengths: Vec<f32> = lengths.into_iter().flatten().filter_map(|l| number(self.doc, l)).map(|l| l as f32).collect();
            state.style.dash = Dash::new(lengths, number(self.doc, phase).map_or(0.0, |p| p as f32));
        }
        if let Ok(blend) = graphics_state.get(b"BM") {
            let name = match blend {
                Object::Name(name) => Some(name.as_slice()),
                Object::Array(names) => names.first().and_then(|n| n.as_name().ok()),
                _ => None,
            };
            state.blend = match name {
                Some(b"Multiply") => Blend::Multiply,
                Some(b"Normal" | b"Compatible") | None => Blend::Normal,
                Some(_) => {
                    self.shapes.not_drawn("blend modes other than Multiply");
                    Blend::Normal
                }
            };
        }
        if graphics_state.get(b"SMask").is_ok_and(|mask| mask.as_name().map_or(true, |n| n != b"None")) {
            self.shapes.not_drawn("soft masks");
        }
    }

    /// The colour space `name` set with `cs` or `CS`: a device space, or one
    /// from the resources.
    fn named_space(&mut self, resources: Option<&'d Dictionary>, name: &[u8]) -> Space {
        match name {
            b"DeviceGray" => Space::Gray,
            b"DeviceRGB" => Space::Rgb,
            b"DeviceCMYK" => Space::Cmyk,
            b"Pattern" => Space::Pattern,
            _ => {
                let Some(object) = self.resource(resources, b"ColorSpace", name) else { return Space::Unsupported };
                let doc = self.doc;
                self.spaces.entry(object as *const Object as usize).or_insert_with(|| space(doc, object)).clone()
            }
        }
    }

    /// Draws XObject `name`: a form is drawn, anything else counted.
    fn draw_xobject(&mut self, frame: &mut Frame<'d>, name: &[u8]) {
        if frame.hidden() {
            return;
        }
        let Some(Ok((_, Object::Stream(xobject)))) = self.resource(frame.resources, b"XObject", name).map(|x| self.doc.dereference(x)) else { return };
        match xobject.dict.get(b"Subtype").and_then(Object::as_name) {
            Ok(b"Form") => self.form(xobject, frame.resources, frame.state.clone(), frame.depth + 1),
            Ok(b"Image") => self.draw_image(&frame.state, xobject),
            _ => {}
        }
    }

    /// Draws image XObject `image` over the unit square of the current
    /// transform, its top row at the top, putting it in the atlas the first
    /// time: in parts, if it's too big for an atlas page.
    fn draw_image(&mut self, state: &State, image: &'d Stream) {
        let is_mask = image.dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false);
        let fill = state.fill.filter(|_| is_mask);
        let key = (image as *const Stream as usize, fill.map(|colour| colour.map(f32::to_bits)));
        let Interpreter { doc, images, decoded, shapes, image_density, .. } = self;
        let image_density = *image_density;
        let parts = images.entry(key).or_insert_with(|| {
            // The pixels it's worth keeping: the size it's drawn at, at the
            // density in use. The image is made at that size rather than made
            // whole and shrunk after.
            let origin = state.ctm.apply([0.0, 0.0]);
            let points = |corner: [f32; 2]| {
                let [x, y] = state.ctm.apply(corner);
                ((x - origin[0]).powi(2) + (y - origin[1]).powi(2)).sqrt()
            };
            let most = |points: f32| (points * image_density).ceil().clamp(1.0, u32::MAX as f32) as u32;
            let target = [most(points([1.0, 0.0])), most(points([0.0, 1.0]))];
            // Its samples were read ahead of drawing, if they were; see
            // `decode_ahead`.
            let ready = decoded.remove(&(image as *const Stream as usize)).transpose();
            match ready {
                Ok(ready) => image::decode(doc, image, fill, Some(target), ready).map(|bitmap| {
                    let own = |key: &[u8]| image.dict.get(key).ok().and_then(|v| number(doc, v)).unwrap_or(1.0) as f32;
                    shapes.image_sizes.push([own(b"Width"), own(b"Height"), points([1.0, 0.0]), points([0.0, 1.0])]);
                    shapes.atlas.add(&bitmap)
                }),
                Err(why) => Err(why),
            }
        });
        match parts {
            Ok(parts) => {
                for part in parts.iter() {
                    // Image space has its top row at y = 1.
                    let [left, top, right, bottom] = part.of_image;
                    let at = |x: f32, from_top: f32| state.ctm.apply([x, 1.0 - from_top]);
                    let corners = [at(left, bottom), at(right, bottom), at(left, top)];
                    shapes.push(Shape::image(corners, part.placed, state.fill_alpha), state.blend, state.clip);
                }
            }
            Err(why) => shapes.not_drawn(why),
        }
    }
}

/// `primitive` of `shapes` as the shapes of it inside `within` -- left,
/// bottom, right, top: a line cut where it leaves, a triangle cut into the
/// triangles of what's left of it.
fn cut_to(within: [f32; 4], shapes: &Shapes, primitive: &crate::shapes::Primitive) -> Vec<Shape> {
    let style = shapes.style_of(primitive);
    let shape = |points| Shape { points, width: style.width, kind: style.kind, colour: style.colour };
    let [left, bottom, right, top] = within;
    if !style.is_triangle() {
        // Liang-Barsky: how far along the line it's inside each edge.
        let [from, to, _] = primitive.points;
        let delta = [to[0] - from[0], to[1] - from[1]];
        let (mut enter, mut leave) = (0.0_f32, 1.0_f32);
        for (p, q) in [(-delta[0], from[0] - left), (delta[0], right - from[0]), (-delta[1], from[1] - bottom), (delta[1], top - from[1])] {
            if p == 0.0 {
                if q < 0.0 {
                    return Vec::new();
                }
            } else if p < 0.0 {
                enter = enter.max(q / p);
            } else {
                leave = leave.min(q / p);
            }
        }
        if enter > leave {
            return Vec::new();
        }
        let at = |t: f32| [from[0] + delta[0] * t, from[1] + delta[1] * t];
        return vec![shape([at(enter), at(leave), at(leave)])];
    }
    // Sutherland-Hodgman, one edge at a time, then a fan.
    let mut polygon: Vec<[f32; 2]> = primitive.points.to_vec();
    let edges: [(fn([f32; 2], f32) -> f32, f32); 4] = [(|p, e| p[0] - e, left), (|p, e| e - p[0], right), (|p, e| p[1] - e, bottom), (|p, e| e - p[1], top)];
    for (inside, edge) in edges {
        let mut kept = Vec::with_capacity(polygon.len() + 2);
        for (i, &point) in polygon.iter().enumerate() {
            let previous = polygon[(i + polygon.len() - 1) % polygon.len()];
            let (now, before) = (inside(point, edge), inside(previous, edge));
            if (now >= 0.0) != (before >= 0.0) {
                let t = before / (before - now);
                kept.push([previous[0] + (point[0] - previous[0]) * t, previous[1] + (point[1] - previous[1]) * t]);
            }
            if now >= 0.0 {
                kept.push(point);
            }
        }
        polygon = kept;
        if polygon.len() < 3 {
            return Vec::new();
        }
    }
    (1..polygon.len() - 1).map(|i| shape([polygon[0], polygon[i], polygon[i + 1]])).collect()
}

/// Paints `triangles`, in page space, in `state`'s fill colour.
fn fill_triangles(shapes: &mut Shapes, state: &State, triangles: impl IntoIterator<Item = [[f32; 2]; 3]>) {
    let Some([r, g, b]) = state.fill else { return shapes.not_drawn("fills in colours not drawn yet") };
    for triangle in triangles {
        shapes.push(Shape::triangle(triangle, [r, g, b, state.fill_alpha]), state.blend, state.clip);
    }
}

/// Strokes `outline`, in page space, in `state`'s stroke colour and style,
/// curves within `tolerance`.
fn stroke_outline(shapes: &mut Shapes, state: &State, outline: &[Piece], tolerance: f32) {
    let Some([r, g, b]) = state.stroke else { return shapes.not_drawn("strokes in colours not drawn yet") };
    let scale = state.ctm.length_scale();
    let (width, colour) = (state.style.width * scale, [r, g, b, state.stroke_alpha]);
    stroke(outline, &state.style, scale, tolerance, |piece| {
        let primitive = match piece {
            Stroked::Line { from, to, round: false } => Shape::line(from, to, width, colour),
            Stroked::Line { from, to, round: true } => Shape::round_line(from, to, width, colour),
            Stroked::Triangle(corners) => Shape::triangle(corners, colour),
        };
        shapes.push(primitive, state.blend, state.clip);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_font::type0_font;
    use pdf_content::lopdf::{dictionary, Object};

    fn draw(content: &str, resources: Option<&Dictionary>) -> Shapes {
        let doc = Document::with_version("1.7");
        let mut interpreter = Interpreter::new(&doc, 0.05, MOST_IMAGE_DENSITY);
        interpreter.draw(content.as_bytes(), resources, Matrix::IDENTITY);
        interpreter.shapes
    }

    #[test]
    fn strokes_take_the_colour_width_and_transform_in_force() {
        let shapes = draw("1 0 0 RG 2 w 2 0 0 2 5 5 cm 0 0 m 1 0 l S", None);
        let [line] = &shapes.primitives[..] else { panic!("one line: {:?}", shapes.primitives) };
        assert_eq!(line.points, [[5.0, 5.0], [7.0, 5.0], [7.0, 5.0]]);
        let style = shapes.style_of(line);
        assert_eq!((style.width, style.colour), (4.0, [1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn a_tiling_pattern_puts_its_cell_in_every_tile_the_fill_reaches() {
        // A 10-point cell: its box filled, and a line poking out of it.
        let cell = Stream::new(
            dictionary! {
                "Type" => "Pattern", "PatternType" => 1, "PaintType" => 1, "TilingType" => 1,
                "BBox" => vec![0.into(), 0.into(), 10.into(), 10.into()], "XStep" => 10, "YStep" => 10,
            },
            b"1 0 0 rg 0 0 10 10 re f 0 0 1 RG 2 w -5 5 m 15 5 l S".to_vec(),
        );
        let faint = dictionary! { "ca" => Object::Real(0.5) };
        let resources = dictionary! { "Pattern" => dictionary! { "Dots" => Object::Stream(cell) }, "ExtGState" => dictionary! { "Faint" => faint } };
        // 25 x 10 points from the origin: three tiles across, one down.
        let shapes = draw("/Faint gs /Pattern cs /Dots scn 0 0 25 10 re f", Some(&resources));
        assert!(shapes.not_drawn.is_empty(), "{:?}", shapes.not_drawn);
        let styles: Vec<crate::shapes::Style> = shapes.primitives.iter().map(|p| shapes.style_of(p)).collect();
        assert_eq!(styles.iter().filter(|s| s.is_triangle()).count(), 3 * 2, "two triangles a tile");
        let lines: Vec<_> = shapes.primitives.iter().filter(|p| !shapes.style_of(p).is_triangle()).collect();
        assert_eq!(lines.len(), 3, "a line a tile");
        assert!(lines.iter().all(|line| line.points[0][0] >= -1e-4 && (line.points[1][0] - line.points[0][0] - 10.0).abs() < 1e-4), "cut to the cell: {lines:?}");
        assert!(styles.iter().all(|s| s.colour[3] == 0.5), "faded as the fill is");
        assert_eq!(shapes.clips.sets.len(), 1, "within the area filled");
        assert!(shapes.runs.iter().all(|run| run.clip.is_none()), "a rectangle, tested in the shader");
    }

    #[test]
    fn spot_inks_are_drawn_in_their_alternate_colour() {
        // A Separation ink that is full magenta in CMYK, and a DeviceN pair
        // whose second ink is yellow, both tinted by exponential functions.
        let magenta = dictionary! { "FunctionType" => 2, "Domain" => vec![0.into(), 1.into()], "C0" => vec![0.into(), 0.into(), 0.into(), 0.into()], "C1" => vec![0.into(), 1.into(), 0.into(), 0.into()], "N" => 1 };
        let program = Stream::new(
            dictionary! { "FunctionType" => 4, "Domain" => vec![0.into(), 1.into(), 0.into(), 1.into()], "Range" => vec![0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into()] },
            b"{ exch pop 0 0 3 -1 roll 0 }".to_vec(),
        );
        let resources = dictionary! { "ColorSpace" => dictionary! {
            "Spot" => vec!["Separation".into(), "PANTONE Rubine".into(), "DeviceCMYK".into(), Object::Dictionary(magenta)],
            "Pair" => vec!["DeviceN".into(), vec!["Cyan".into(), "Yellow".into()].into(), "DeviceCMYK".into(), Object::Stream(program)],
        } };
        let colours = |content: &str| {
            let shapes = draw(content, Some(&resources));
            assert!(shapes.not_drawn.is_empty(), "{:?}", shapes.not_drawn);
            shapes.primitives.iter().map(|p| shapes.style_of(p).colour).collect::<Vec<_>>()
        };
        // Setting the space starts at full ink.
        assert!(colours("/Spot cs 0 0 1 1 re f").iter().all(|&c| c == [1.0, 0.0, 1.0, 1.0]));
        assert!(colours("/Spot cs 0.5 scn 0 0 1 1 re f").iter().all(|&c| c == [1.0, 0.5, 1.0, 1.0]));
        assert!(colours("/Pair cs 1 0.25 scn 0 0 1 1 re f").iter().all(|&c| c == [1.0, 1.0, 0.75, 1.0]));
    }

    #[test]
    fn restoring_the_state_restores_the_colour() {
        let shapes = draw("q 0 1 0 rg Q 0 0 1 1 re f", None);
        assert_eq!(shapes.triangles, 2);
        assert!(shapes.primitives.iter().all(|p| shapes.style_of(p).colour == [0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn closing_fill_and_stroke_fills_first_then_strokes_the_closed_outline() {
        let shapes = draw("0 w 0 0 m 1 0 l 1 1 l b", None);
        let kinds: Vec<bool> = shapes.primitives.iter().map(|p| shapes.style_of(p).is_triangle()).collect();
        assert_eq!(kinds, [true, false, false, false], "one triangle, then three sides, hairlines without joins");
    }

    #[test]
    fn cmyk_and_gray_colours_become_rgb() {
        let cyan = draw("1 0 0 0 k 0 0 1 1 re f", None);
        assert_eq!(cyan.style_of(&cyan.primitives[0]).colour, [0.0, 1.0, 1.0, 1.0]);
        let gray = draw("0.5 G 0 0 m 1 1 l S", None);
        assert_eq!(gray.style_of(&gray.primitives[0]).colour, [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn graphics_states_set_alpha_and_multiply() {
        let resources = dictionary! { "ExtGState" => dictionary! { "Faint" => dictionary! { "ca" => Object::Real(0.5), "BM" => "Multiply" } } };
        let shapes = draw("/Faint gs 0 0 1 1 re f", Some(&resources));
        assert!(shapes.primitives.iter().all(|p| shapes.style_of(p).colour[3] == 0.5));
        assert_eq!(shapes.runs.len(), 1);
        assert_eq!(shapes.runs[0].blend, Blend::Multiply);
    }

    #[test]
    fn strokes_follow_dashes_caps_and_joins_from_operators_and_graphics_states() {
        let dashed = draw("[2 1] 0 d 0 w 0 0 m 10 0 l S", None);
        assert_eq!((dashed.lines, dashed.triangles), (4, 0));
        let joined = draw("2 w 2 j 0 0 m 10 0 l 10 10 l S", None);
        assert_eq!((joined.lines, joined.triangles), (2, 1), "a bevel");

        let round = dictionary! { "LC" => Object::Integer(1), "D" => Object::Array(vec![Object::Array(vec![Object::Integer(4)]), Object::Integer(0)]) };
        let resources = dictionary! { "ExtGState" => dictionary! { "Round" => round } };
        let capped = draw("/Round gs 2 w 0 0 m 10 0 l S", Some(&resources));
        assert_eq!(capped.lines, 2, "dashes 0-4 and 8-10");
        assert!(capped.triangles > 0, "with round caps");
        assert!(capped.not_drawn.is_empty(), "{:?}", capped.not_drawn);
    }

    #[test]
    fn what_isnt_drawn_yet_is_counted() {
        let shapes = draw("BT (hello) Tj ET /Pattern cs /P1 scn 0 0 1 1 re f", None);
        assert!(shapes.primitives.is_empty());
        assert_eq!(shapes.not_drawn.get("text without a font"), Some(&1));
        assert_eq!(shapes.not_drawn.get("fills in colours not drawn yet"), Some(&1));
    }

    /// Draws `content` with the square font as `/F1` and Helvetica, not
    /// embedded, as `/F2`.
    fn draw_text(content: &str) -> Shapes {
        let helvetica = dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" };
        draw(content, Some(&dictionary! { "Font" => dictionary! { "F1" => type0_font(), "F2" => helvetica } }))
    }

    /// Each triangle's leftmost and lowest corner.
    fn lowest_lefts(shapes: &Shapes) -> Vec<[f32; 2]> {
        shapes.primitives.iter().map(|p| p.points.iter().fold([f32::MAX; 2], |low, q| [low[0].min(q[0]), low[1].min(q[1])])).collect()
    }

    #[test]
    fn text_is_drawn_glyph_by_glyph_along_its_line() {
        let shapes = draw_text("BT /F1 10 Tf 5 5 Td <00010001> Tj [<0001> -500 <0001>] TJ 0 2 Td 3 Tr <0001> Tj ET");
        assert_eq!(shapes.triangles, 8, "four squares, and one invisible");
        let lefts: Vec<f32> = lowest_lefts(&shapes).chunks(2).map(|square| square[0][0].min(square[1][0])).collect();
        assert_eq!(lefts, [5.0, 10.0, 15.0, 25.0], "half an em each, and half an em more where TJ says");
        let top = shapes.primitives.iter().flat_map(|p| p.points).map(|[_, y]| y).fold(f32::MIN, f32::max);
        assert!((top - 6.0).abs() < 1e-4, "a tenth of an em tall at 10 points: {top}");
    }

    #[test]
    fn text_in_a_clipping_mode_clips_what_follows_it() {
        let shapes = draw_text("BT /F1 10 Tf 7 Tr <0001> Tj ET 0 0 20 20 re f");
        assert_eq!(shapes.triangles, 2, "only the square filled after");
        let set = clip_sets(&shapes)[0].expect("clipped to the glyph");
        let corners = &shapes.clips.vertices[shapes.clips.shapes[shapes.clips.sets[set][0]].clone()];
        assert!(corners.iter().all(|&[x, y]| (0.0..=1.0 + 1e-4).contains(&x) && (0.0..=1.0 + 1e-4).contains(&y)), "{corners:?}");
    }

    #[test]
    fn images_fill_the_unit_square_of_the_transform_and_are_packed_once() {
        let pixel = Stream::new(dictionary! { "Subtype" => "Image", "Width" => 1, "Height" => 1, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 }, vec![255]);
        let shapes = draw("q 20 0 0 10 5 5 cm /Im1 Do Q /Im1 Do", Some(&dictionary! { "XObject" => dictionary! { "Im1" => pixel } }));
        assert_eq!(shapes.images, 2);
        assert!(shapes.style_of(&shapes.primitives[0]).is_image());
        assert_eq!(shapes.primitives[0].points, [[5.0, 5.0], [25.0, 5.0], [5.0, 15.0]], "bottom left, bottom right, top left");
        assert_eq!(shapes.primitives[0].style, shapes.primitives[1].style, "the same place in the atlas");
        assert_eq!(shapes.atlas.pages.len(), 1);
    }

    #[test]
    fn images_are_kept_no_denser_than_the_page_can_show() {
        let dict = dictionary! { "Subtype" => "Image", "Width" => 100, "Height" => 40, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 };
        let stamp = Stream::new(dict, vec![128; 100 * 40]);
        let shapes = draw("q 2 0 0 3 0 0 cm /Im1 Do Q", Some(&dictionary! { "XObject" => dictionary! { "Im1" => stamp } }));
        let [left, top, right, bottom] = shapes.style_of(&shapes.primitives[0]).colour.map(|f| f * crate::atlas::ATLAS_SIZE as f32);
        assert_eq!([(right - left).round(), (bottom - top).round()], [16.0, 24.0], "2 x 3 points at 8 pixels a point");
    }

    /// Helvetica, which the fixture doesn't embed, is drawn in the system font
    /// standing in for it (`font::substitute`), so a page of ordinary text
    /// needn't fall to pdfium. Only where there are system fonts to stand in.
    #[test]
    #[cfg_attr(not(windows), ignore = "no system fonts to stand in")]
    fn text_in_a_font_that_isnt_embedded_is_drawn_in_one_from_the_system() {
        let shapes = draw_text("BT /F2 12 Tf (Hello) Tj (again) ' ET");
        assert!(shapes.not_drawn.is_empty(), "{:?}", shapes.not_drawn);
        assert!(shapes.triangles > 20, "ten letters of outlines: {} triangles", shapes.triangles);
        let xs: Vec<f32> = shapes.primitives.iter().flat_map(|p| p.points).map(|[x, _]| x).collect();
        let ys: Vec<f32> = shapes.primitives.iter().flat_map(|p| p.points).map(|[_, y]| y).collect();
        let span = |values: &[f32]| values.iter().fold(f32::MIN, |a, b| a.max(*b)) - values.iter().fold(f32::MAX, |a, b| a.min(*b));
        assert!((20.0..120.0).contains(&span(&xs)), "ten letters at 12 points run about 55 points along: {}", span(&xs));
        assert!((4.0..14.0).contains(&span(&ys)), "and stand about 9 points tall: {}", span(&ys));
    }

    /// The clip set each primitive is drawn within, where it's convex.
    fn clip_sets(shapes: &Shapes) -> Vec<Option<usize>> {
        shapes.primitives.iter().map(|p| shapes.style_of(p).clip).map(|clip| (clip > 0.0).then(|| clip as usize - 1)).collect()
    }

    #[test]
    fn a_clip_applies_after_its_path_is_painted_until_the_state_is_restored() {
        let shapes = draw("q 0 0 5 5 re W f 0 0 10 10 re f Q 0 0 1 1 re f", None);
        assert_eq!(clip_sets(&shapes), [None, None, Some(0), Some(0), None, None], "the clipping square itself isn't clipped, the next fill is, the last isn't");
        assert_eq!(shapes.clips.shapes[shapes.clips.sets[0][0]].len(), 6, "a square clip of two triangles");
        assert_eq!(shapes.runs.len(), 1, "a convex clip doesn't split the run");
    }

    #[test]
    fn a_clip_inside_a_clip_keeps_both() {
        let shapes = draw("0 0 5 5 re W n 1 1 5 5 re W n 0 0 10 10 re f", None);
        let set = clip_sets(&shapes)[0].expect("the fill is clipped");
        assert_eq!(shapes.clips.sets[set].len(), 2);
    }

    #[test]
    fn an_empty_clip_path_leaves_nothing_visible() {
        let shapes = draw("W n 0 0 1 1 re f", None);
        let set = clip_sets(&shapes)[0].expect("the fill is clipped");
        assert!(shapes.clips.sets[set].iter().all(|&s| shapes.clips.shapes[s].is_empty()));
    }
}
