//! Drawing content streams: following the graphics state as the operators
//! change it, and turning what they paint into shapes.
//!
//! Drawn: saving and restoring the state, transforms, line widths, dash
//! patterns, line caps and joins, colours in
//! gray, RGB, CMYK, ICC-based and indexed spaces, alpha and Multiply blending
//! from graphics states, every path and painting operator, text in embedded
//! fonts (filled, stroked or clipping), image XObjects
//! (packed into the atlas once however often they're drawn), forms inside
//! forms, and marked content on layers, which is left out while its layer is
//! off. Everything else is counted in `Shapes::not_drawn`.

use std::collections::HashMap;

use lyon_tessellation::{FillRule, FillTessellator};
use pdf_content::layers::Layers;
use pdf_content::lexer::{each_operation, Operand};
use pdf_content::lopdf::{Dictionary, Document, Object, Stream};
use pdf_content::objects::{dict, number};

use crate::atlas::Part;
use crate::colour::{space, Space};
use crate::font::Font;
use crate::geometry::{fill, Matrix, Piece};
use crate::image::{self, Raw};
use crate::pdf::{matrix, rectangle};
use crate::shapes::{Blend, Primitive, Shapes, Unsupported};
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

/// The graphics state that drawing needs.
#[derive(Clone, Debug)]
struct State {
    ctm: Matrix,
    stroke_space: Space,
    fill_space: Space,
    /// `None` while the colour is one that can't be drawn, a pattern say.
    stroke: Option<[f32; 3]>,
    fill: Option<[f32; 3]>,
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
            shapes: Shapes::default(),
        }
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
        let doc = self.doc;
        let next = std::sync::atomic::AtomicUsize::new(0);
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(images.len());
        // Taken one at a time, so one huge photo doesn't leave a thread with
        // all the work while the others have finished.
        let decode_each = || {
            let mut decoded = Vec::new();
            loop {
                let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(image) = images.get(index) else { return decoded };
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

    fn run(&mut self, content: &[u8], resources: Option<&'d Dictionary>, state: State, depth: usize) {
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
        each_operation(content, |operator, operands, _| self.operate(&mut frame, operator, operands));
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
                    (state.fill_space, state.fill) = (space, colour);
                }
            }
            b"SC" | b"SCN" | b"sc" | b"scn" => {
                let values: Vec<f32> = operands.iter().filter_map(Operand::number).collect();
                let stroking = operator.starts_with(b"S");
                let space = if stroking { &state.stroke_space } else { &state.fill_space };
                // A pattern's name comes last; drawing patterns isn't done yet.
                let colour = if operands.last().and_then(Operand::name).is_some() { None } else { space.colour(&values) };
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
                    (state.fill_space, state.fill) = (space, colour);
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
            match fill(outline, rule, self.tolerance, &mut self.tessellator) {
                Some(triangles) => fill_triangles(&mut self.shapes, state, triangles),
                None => self.shapes.not_drawn("fills that wouldn't tessellate"),
            }
        }
        if stroked {
            stroke_outline(&mut self.shapes, state, outline, self.tolerance);
        }
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
    fn named_space(&self, resources: Option<&'d Dictionary>, name: &[u8]) -> Space {
        match name {
            b"DeviceGray" => Space::Gray,
            b"DeviceRGB" => Space::Rgb,
            b"DeviceCMYK" => Space::Cmyk,
            b"Pattern" => Space::Pattern,
            _ => self.resource(resources, b"ColorSpace", name).map_or(Space::Unsupported, |object| space(self.doc, object)),
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
                Ok(ready) => image::decode(doc, image, fill, Some(target), ready).map(|bitmap| shapes.atlas.add(&bitmap)),
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
                    shapes.push(Primitive::image(corners, part.placed, state.fill_alpha), state.blend, state.clip);
                }
            }
            Err(why) => shapes.not_drawn(why),
        }
    }
}

/// Paints `triangles`, in page space, in `state`'s fill colour.
fn fill_triangles(shapes: &mut Shapes, state: &State, triangles: impl IntoIterator<Item = [[f32; 2]; 3]>) {
    let Some([r, g, b]) = state.fill else { return shapes.not_drawn("fills in colours not drawn yet") };
    for triangle in triangles {
        shapes.push(Primitive::triangle(triangle, [r, g, b, state.fill_alpha]), state.blend, state.clip);
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
            Stroked::Line { from, to, round: false } => Primitive::line(from, to, width, colour),
            Stroked::Line { from, to, round: true } => Primitive::round_line(from, to, width, colour),
            Stroked::Triangle(corners) => Primitive::triangle(corners, colour),
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
        assert_eq!(shapes.primitives, [Primitive::line([5.0, 5.0], [7.0, 5.0], 4.0, [1.0, 0.0, 0.0, 1.0])]);
    }

    #[test]
    fn restoring_the_state_restores_the_colour() {
        let shapes = draw("q 0 1 0 rg Q 0 0 1 1 re f", None);
        assert_eq!(shapes.triangles, 2);
        assert!(shapes.primitives.iter().all(|p| p.colour == [0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn closing_fill_and_stroke_fills_first_then_strokes_the_closed_outline() {
        let shapes = draw("0 w 0 0 m 1 0 l 1 1 l b", None);
        let kinds: Vec<bool> = shapes.primitives.iter().map(Primitive::is_triangle).collect();
        assert_eq!(kinds, [true, false, false, false], "one triangle, then three sides, hairlines without joins");
    }

    #[test]
    fn cmyk_and_gray_colours_become_rgb() {
        let cyan = draw("1 0 0 0 k 0 0 1 1 re f", None);
        assert_eq!(cyan.primitives[0].colour, [0.0, 1.0, 1.0, 1.0]);
        let gray = draw("0.5 G 0 0 m 1 1 l S", None);
        assert_eq!(gray.primitives[0].colour, [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn graphics_states_set_alpha_and_multiply() {
        let resources = dictionary! { "ExtGState" => dictionary! { "Faint" => dictionary! { "ca" => Object::Real(0.5), "BM" => "Multiply" } } };
        let shapes = draw("/Faint gs 0 0 1 1 re f", Some(&resources));
        assert!(shapes.primitives.iter().all(|p| p.colour[3] == 0.5));
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
        assert!(shapes.primitives[0].is_image());
        assert_eq!(shapes.primitives[0].points, [[5.0, 5.0], [25.0, 5.0], [5.0, 15.0]], "bottom left, bottom right, top left");
        assert_eq!(shapes.primitives[0].colour, shapes.primitives[1].colour, "the same place in the atlas");
        assert_eq!(shapes.atlas.pages.len(), 1);
    }

    #[test]
    fn images_are_kept_no_denser_than_the_page_can_show() {
        let dict = dictionary! { "Subtype" => "Image", "Width" => 100, "Height" => 40, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 };
        let stamp = Stream::new(dict, vec![128; 100 * 40]);
        let shapes = draw("q 2 0 0 3 0 0 cm /Im1 Do Q", Some(&dictionary! { "XObject" => dictionary! { "Im1" => stamp } }));
        let [left, top, right, bottom] = shapes.primitives[0].colour.map(|f| f * crate::atlas::ATLAS_SIZE as f32);
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
        shapes.primitives.iter().map(|p| (p.clip > 0.0).then(|| p.clip as usize - 1)).collect()
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
