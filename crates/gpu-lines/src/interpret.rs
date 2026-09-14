//! Drawing content streams: following the graphics state as the operators
//! change it, and turning what they paint into shapes.
//!
//! Drawn: saving and restoring the state, transforms, line widths, colours in
//! gray, RGB, CMYK, ICC-based and indexed spaces, alpha and Multiply blending
//! from graphics states, every path and painting operator, forms inside
//! forms, and marked content on layers, which is left out while its layer is
//! off. Everything else is counted in `Shapes::not_drawn`.

use lyon_tessellation::{FillRule, FillTessellator};
use pdf_content::layers::Layers;
use pdf_content::lexer::{each_operation, Operand};
use pdf_content::lopdf::{Dictionary, Document, Object, Stream};
use pdf_content::objects::{dict, number};

use crate::geometry::{fill, stroke, Matrix, Piece};
use crate::pdf::matrix;
use crate::shapes::{Blend, Primitive, Shapes};

/// Forms drawn inside forms go at most this deep, against forms that draw
/// themselves.
const DEEPEST: usize = 16;

/// A colour space, as far as drawing needs it.
#[derive(Clone, Debug, PartialEq)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// Colours looked up in a table of `base` colours, one byte a component.
    Indexed { base: Box<Space>, highest: usize, table: Vec<u8> },
    Pattern,
    /// Separation, DeviceN, Lab and the like.
    Unsupported,
}

impl Space {
    fn components(&self) -> usize {
        match self {
            Space::Rgb => 3,
            Space::Cmyk => 4,
            Space::Pattern => 0,
            _ => 1,
        }
    }

    /// The colour a space starts with when it's set: black, or the table's
    /// first entry.
    fn initial(&self) -> Option<[f32; 3]> {
        match self {
            Space::Cmyk => self.colour(&[0.0, 0.0, 0.0, 1.0]),
            _ => self.colour(&vec![0.0; self.components().max(1)]),
        }
    }

    /// The RGB colour of `values` in this space; `None` if it can't be drawn.
    fn colour(&self, values: &[f32]) -> Option<[f32; 3]> {
        match self {
            Space::Gray => values.first().map(|&g| [g, g, g]),
            Space::Rgb => match values {
                [r, g, b, ..] => Some([*r, *g, *b]),
                _ => None,
            },
            Space::Cmyk => match values {
                [c, m, y, k, ..] => Some([(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]),
                _ => None,
            },
            Space::Indexed { base, highest, table } => {
                let index = values.first()?.round().clamp(0.0, *highest as f32) as usize;
                let size = base.components();
                let entry = table.get(index * size..(index + 1) * size)?;
                base.colour(&entry.iter().map(|&b| b as f32 / 255.0).collect::<Vec<_>>())
            }
            Space::Pattern | Space::Unsupported => None,
        }
    }
}

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
    width: f32,
    blend: Blend,
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
            width: 1.0,
            blend: Blend::Normal,
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
    /// A clip is set by the next painting operator.
    clipping: bool,
    /// For each marked-content section open, whether it's visible.
    marked: Vec<bool>,
    depth: usize,
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
    tessellator: FillTessellator,
    pub shapes: Shapes,
}

impl<'d> Interpreter<'d> {
    /// Curves are flattened to within `tolerance` points.
    pub fn new(doc: &'d Document, tolerance: f32) -> Self {
        Interpreter { doc, layers: Layers::read(doc), tolerance, tessellator: FillTessellator::new(), shapes: Shapes::default() }
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
        let mut frame = Frame { state, saved: Vec::new(), resources, path: Vec::new(), current: [0.0; 2], start: [0.0; 2], clipping: false, marked: Vec::new(), depth };
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
                    state.width = width;
                }
            }
            b"d" => {
                if matches!(operands.first(), Some(Operand::Array(dashes)) if !dashes.is_empty()) {
                    self.shapes.not_drawn("dash patterns");
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
            b"W" | b"W*" => frame.clipping = true,
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => self.paint(frame, operator),

            // Forms, and what isn't drawn yet.
            b"Do" => {
                if let Some(name) = operands.last().and_then(Operand::name) {
                    self.draw_xobject(frame, name);
                }
            }
            b"Tj" | b"TJ" | b"'" | b"\"" if !frame.hidden() => self.shapes.not_drawn("text"),
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
        if std::mem::take(&mut frame.clipping) {
            self.shapes.not_drawn("clips");
        }
        if outline.is_empty() || frame.hidden() {
            return;
        }
        if matches!(operator, b"s" | b"b" | b"b*") {
            outline.push(Piece::Close);
        }
        let (rule, stroked) = match operator {
            b"S" | b"s" => (None, true),
            b"f" | b"F" => (Some(FillRule::NonZero), false),
            b"f*" => (Some(FillRule::EvenOdd), false),
            b"B" | b"b" => (Some(FillRule::NonZero), true),
            b"B*" | b"b*" => (Some(FillRule::EvenOdd), true),
            _ => (None, false),
        };
        let state = &frame.state;
        if let Some(rule) = rule {
            match (state.fill, fill(&outline, rule, self.tolerance, &mut self.tessellator)) {
                (Some([r, g, b]), Some(triangles)) => {
                    for triangle in triangles {
                        self.shapes.push(Primitive::triangle(triangle, [r, g, b, state.fill_alpha]), state.blend);
                    }
                }
                (None, _) => self.shapes.not_drawn("fills in colours not drawn yet"),
                (_, None) => self.shapes.not_drawn("fills that wouldn't tessellate"),
            }
        }
        if stroked {
            match state.stroke {
                Some([r, g, b]) => {
                    let (width, colour, blend) = (state.width * state.ctm.length_scale(), [r, g, b, state.stroke_alpha], state.blend);
                    let shapes = &mut self.shapes;
                    stroke(&outline, self.tolerance, |from, to| shapes.push(Primitive::line(from, to, width, colour), blend));
                }
                None => self.shapes.not_drawn("strokes in colours not drawn yet"),
            }
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
            state.width = width;
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
        if graphics_state.get(b"D").is_ok() {
            self.shapes.not_drawn("dash patterns");
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
            _ => self.resource(resources, b"ColorSpace", name).map_or(Space::Unsupported, |space| self.space(space)),
        }
    }

    /// A colour space written out: a name, or an array like
    /// `[/ICCBased stream]` or `[/Indexed base highest table]`.
    fn space(&self, object: &Object) -> Space {
        let Ok((_, object)) = self.doc.dereference(object) else { return Space::Unsupported };
        let (family, rest) = match object {
            Object::Name(name) => (name.as_slice(), &[][..]),
            Object::Array(items) => match items.split_first() {
                Some((Object::Name(name), rest)) => (name.as_slice(), rest),
                _ => return Space::Unsupported,
            },
            _ => return Space::Unsupported,
        };
        match family {
            b"DeviceGray" | b"CalGray" | b"G" => Space::Gray,
            b"DeviceRGB" | b"CalRGB" | b"RGB" => Space::Rgb,
            b"DeviceCMYK" | b"CMYK" => Space::Cmyk,
            b"Pattern" => Space::Pattern,
            b"ICCBased" => {
                let components = rest
                    .first()
                    .and_then(|stream| self.doc.dereference(stream).ok())
                    .and_then(|(_, stream)| stream.as_stream().ok())
                    .and_then(|stream| stream.dict.get(b"N").ok().and_then(|n| number(self.doc, n)));
                match components.map(|n| n as usize) {
                    Some(1) => Space::Gray,
                    Some(3) => Space::Rgb,
                    Some(4) => Space::Cmyk,
                    _ => Space::Unsupported,
                }
            }
            b"Indexed" | b"I" => {
                let [base, highest, table] = rest else { return Space::Unsupported };
                let table = match self.doc.dereference(table).map(|(_, t)| t) {
                    Ok(Object::String(bytes, _)) => bytes.clone(),
                    Ok(Object::Stream(stream)) => stream.get_plain_content().unwrap_or_default(),
                    _ => return Space::Unsupported,
                };
                match (self.space(base), number(self.doc, highest)) {
                    (Space::Indexed { .. } | Space::Pattern | Space::Unsupported, _) | (_, None) => Space::Unsupported,
                    (base, Some(highest)) => Space::Indexed { base: Box::new(base), highest: highest as usize, table },
                }
            }
            _ => Space::Unsupported,
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
            Ok(b"Image") => self.shapes.not_drawn("images"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_content::lopdf::{dictionary, Object};

    fn draw(content: &str, resources: Option<&Dictionary>) -> Shapes {
        let doc = Document::with_version("1.7");
        let mut interpreter = Interpreter::new(&doc, 0.05);
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
        let shapes = draw("0 0 m 1 0 l 1 1 l b", None);
        let kinds: Vec<bool> = shapes.primitives.iter().map(Primitive::is_triangle).collect();
        assert_eq!(kinds, [true, false, false, false], "one triangle, then three sides");
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
    fn what_isnt_drawn_yet_is_counted() {
        let shapes = draw("BT (hello) Tj ET 0 0 1 1 re W n /Pattern cs /P1 scn 0 0 1 1 re f", None);
        assert!(shapes.primitives.is_empty());
        assert_eq!(shapes.not_drawn.get("text"), Some(&1));
        assert_eq!(shapes.not_drawn.get("clips"), Some(&1));
        assert_eq!(shapes.not_drawn.get("fills in colours not drawn yet"), Some(&1));
    }
}
