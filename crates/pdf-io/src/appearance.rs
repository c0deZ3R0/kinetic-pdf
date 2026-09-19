//! The /AP /N appearance stream of a measurement markup, so viewers that
//! don't draw from /Vertices and /Measure -- and pdfium -- show what we show.
//! The app itself draws markups from the model, never from this.

use markup_model::markup::{FillPattern, Geometry, Markup, MarkupKind, Style};
#[cfg(test)]
use markup_model::LabelFont;
use markup_model::{Pt, Rect};
use pdf_content::lopdf::{dictionary, Dictionary, Object};

use crate::values::real;

pub struct Appearance {
    pub content: Vec<u8>,
    pub resources: Dictionary,
    /// Tiling patterns the content fills with, to be added as objects and
    /// named in the resources' /Pattern. A pattern is drawn once by the viewer
    /// and repeated, so a hatch costs one small stream however large the shape
    /// is -- where ruling the lines into the content stream would cost
    /// hundreds of them per shape.
    pub patterns: Vec<TilingPattern>,
    /// Where the appearance draws, in user space: the annotation's /Rect and
    /// the form's /BBox.
    pub bbox: Rect,
}

/// A number in a content stream, to a thousandth of a point.
fn n(v: f64) -> String {
    let fixed = format!("{v:.3}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    if trimmed == "-0" {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn path(ops: &mut String, pts: &[Pt], close: bool) {
    let Some((first, rest)) = pts.split_first() else { return };
    ops.push_str(&format!("{} {} m", n(first.x), n(first.y)));
    for p in rest {
        ops.push_str(&format!(" {} {} l", n(p.x), n(p.y)));
    }
    ops.push_str(if close { " h\n" } else { "\n" });
}

/// Text in a Helvetica string with WinAnsiEncoding: Latin-1 characters as
/// their byte (², ³ and ° included), anything else as `?`.
fn win_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('(');
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            '\u{a0}'..='\u{ff}' => out.push_str(&format!("\\{:03o}", c as u32)),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

/// Where the label sits: the middle of a line, the middle segment of a path,
/// the middle of an area's box.
fn label_anchor(geometry: &Geometry) -> Option<Pt> {
    match geometry {
        Geometry::Line { a, b } => Some(a.midpoint(*b)),
        Geometry::Polyline { pts } if pts.len() >= 2 => {
            let i = (pts.len() - 1) / 2;
            Some(pts[i].midpoint(pts[i + 1]))
        }
        _ => geometry.bounds().map(|b| b.center()),
    }
}

pub fn appearance(m: &Markup, label: Option<&str>) -> Appearance {
    let Style { stroke, fill, opacity, fill_opacity, pattern, pattern_colour, pattern_opacity, width, ref dash, label_size, label_colour, label_font, .. } = m.style;
    // A hatch is a pattern the viewer repeats, not lines written out one by
    // one; see `tiling`.
    let tile = fill.and_then(|_| tiling(pattern, m.style.pattern_size));
    let mut ops = String::from("q /GS0 gs\n");
    let [r, g, b] = stroke.map(f64::from);
    ops.push_str(&format!("{} {} {} RG {} w 1 J 1 j\n", n(r), n(g), n(b), n(width)));
    if !dash.is_empty() {
        let lengths: Vec<String> = dash.iter().map(|&d| n(d)).collect();
        ops.push_str(&format!("[{}] 0 d\n", lengths.join(" ")));
    }
    // A radius or a diameter shows the circle it comes off: around the first
    // point for a radius, around the middle of the line for a diameter, which
    // is drawn right across.
    let circle = match (m.kind, &m.geometry) {
        (MarkupKind::Radius, Geometry::Line { a, b }) => Some((*a, a.dist(*b))),
        (MarkupKind::Diameter, Geometry::Line { a, b }) => Some((a.midpoint(*b), a.dist(*b) / 2.0)),
        _ => None,
    };
    let circle = circle.filter(|&(_, r)| r > 0.0).map(|(c, r)| Rect::from_corners(Pt::new(c.x - r, c.y - r), Pt::new(c.x + r, c.y + r)));
    // A circle closes itself; a count's crosses are strokes.
    let closed = matches!(m.geometry, Geometry::Polygon { .. } | Geometry::Ellipse { .. });
    shape(&mut ops, m, circle, width);
    // Three layers, each with its own colour and its own transparency: the
    // fill, the ruling over it, and the line round it. The path is restated
    // for each, since painting it consumes it.
    match (closed, fill) {
        (true, Some([fr, fg, fb])) => {
            let (fr, fg, fb) = (n(f64::from(fr)), n(f64::from(fg)), n(f64::from(fb)));
            ops.push_str(&format!("q /GSfill gs {fr} {fg} {fb} rg f*\nQ\n"));
            if let Some(t) = &tile {
                let [pr, pg, pb] = pattern_colour.unwrap_or(stroke).map(f64::from);
                shape(&mut ops, m, circle, width);
                ops.push_str(&format!("q /GSpat gs /Pattern cs {} {} {} /{} scn f*\nQ\n", n(pr), n(pg), n(pb), t.name));
            }
            shape(&mut ops, m, circle, width);
            ops.push_str("S\n");
        }
        (true, None) => ops.push_str("s\n"),
        (false, _) => ops.push_str("S\n"),
    }
    // The label is drawn at full strength, whatever the shape's opacity.
    ops.push_str("Q\n");

    let drawn = m.geometry.bounds().unwrap_or(Rect::from_corners(Pt::default(), Pt::default()));
    // The circle reaches past the line that measures it.
    let drawn = circle.map_or(drawn, |c| drawn.union(c));
    let mut bbox = drawn.expand(width.max(0.0) / 2.0 + 1.0);
    let mut resources = dictionary! {
        // One graphics state per layer, so the line, the fill and the ruling
        // over it each carry their own transparency.
        "ExtGState" => dictionary! {
            "GS0" => dictionary! { "Type" => "ExtGState", "CA" => real(f64::from(opacity)), "ca" => real(f64::from(opacity)) },
            "GSfill" => dictionary! { "Type" => "ExtGState", "ca" => real(f64::from(fill_opacity)) },
            "GSpat" => dictionary! { "Type" => "ExtGState", "ca" => real(f64::from(pattern_opacity)) },
        },
    };
    if let (Some(label), Some(at)) = (label.filter(|l| !l.is_empty()), label_anchor(&m.geometry)) {
        // Helvetica's characters average about half the font size across.
        let half_width = 0.28 * label_size * label.chars().count() as f64;
        // Just above a line or path, so the text doesn't strike through it;
        // centred in an area.
        let lift = if closed { -label_size * 0.35 } else { width / 2.0 + label_size * 0.3 };
        let (x, y) = (at.x - half_width, at.y + lift);
        // Its own colour, falling back to the line's.
        let [lr, lg, lb] = label_colour.unwrap_or(stroke).map(f64::from);
        ops.push_str(&format!("BT /Lbl {} Tf {} {} {} rg {} {} Td {} Tj ET\n", n(label_size), n(lr), n(lg), n(lb), n(x), n(y), win_ansi(label)));
        bbox = bbox.union(Rect::from_corners(Pt::new(x - 1.0, y - label_size * 0.3), Pt::new(at.x + half_width + 1.0, y + label_size)));
        resources.set(
            "Font",
            dictionary! {
                "Lbl" => dictionary! {
                    "Type" => "Font",
                    "Subtype" => "Type1",
                    "BaseFont" => label_font.base_font(),
                    "Encoding" => "WinAnsiEncoding",
                },
            },
        );
    }
    Appearance { content: ops.into_bytes(), resources, bbox, patterns: tile.into_iter().collect() }
}

/// The form XObject dictionary for an appearance.
pub fn form_dict(a: &Appearance) -> Dictionary {
    let b = a.bbox;
    dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "BBox" => Object::Array([b.min.x, b.min.y, b.max.x, b.max.y].into_iter().map(real).collect()),
        "Resources" => a.resources.clone(),
    }
}

/// The markup's path, laid down ready to be painted. Restated for every layer
/// that paints it, since filling or stroking consumes the path.
fn shape(ops: &mut String, m: &Markup, circle: Option<Rect>, width: f64) {
    match &m.geometry {
        Geometry::Line { a, b } => {
            path(ops, &[*a, *b], false);
            if let Some(rect) = circle {
                ellipse(ops, rect);
            }
        }
        Geometry::Polyline { pts } => path(ops, pts, false),
        // A count is a mark at each place counted, not a path through them.
        Geometry::Points { pts } => pts.iter().for_each(|p| marks(ops, *p, width.max(1.0) * 3.0)),
        Geometry::Polygon { pts, holes } => {
            path(ops, pts, true);
            holes.iter().for_each(|h| path(ops, h, true));
        }
        Geometry::Ink { strokes } => strokes.iter().for_each(|s| path(ops, s, false)),
        Geometry::Ellipse { rect } => ellipse(ops, *rect),
    }
}

/// A cross at `at`, `size` points across: what a count marks each thing with.
fn marks(ops: &mut String, at: Pt, size: f64) {
    let r = size / 2.0;
    ops.push_str(&format!("{} {} m {} {} l\n", n(at.x - r), n(at.y), n(at.x + r), n(at.y)));
    ops.push_str(&format!("{} {} m {} {} l\n", n(at.x), n(at.y - r), n(at.x), n(at.y + r)));
}

/// The ellipse filling `rect`, as the four Bezier curves PDF draws one with.
fn ellipse(ops: &mut String, rect: Rect) {
    // Control points this far along the tangents put a curve within 0.03% of
    // a quarter ellipse.
    const KAPPA: f64 = 0.552_284_8;
    let (c, rx, ry) = (rect.center(), rect.width() / 2.0, rect.height() / 2.0);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let curve = |c1: Pt, c2: Pt, to: Pt| format!(" {} {} {} {} {} {} c", n(c1.x), n(c1.y), n(c2.x), n(c2.y), n(to.x), n(to.y));
    ops.push_str(&format!("{} {} m", n(c.x + rx), n(c.y)));
    ops.push_str(&curve(Pt::new(c.x + rx, c.y + ky), Pt::new(c.x + kx, c.y + ry), Pt::new(c.x, c.y + ry)));
    ops.push_str(&curve(Pt::new(c.x - kx, c.y + ry), Pt::new(c.x - rx, c.y + ky), Pt::new(c.x - rx, c.y)));
    ops.push_str(&curve(Pt::new(c.x - rx, c.y - ky), Pt::new(c.x - kx, c.y - ry), Pt::new(c.x, c.y - ry)));
    ops.push_str(&curve(Pt::new(c.x + kx, c.y - ry), Pt::new(c.x + rx, c.y - ky), Pt::new(c.x + rx, c.y)));
    ops.push_str(" h\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use markup_model::MarkupKind;

    #[test]
    fn labels_encode_units_and_escape_brackets() {
        assert_eq!(win_ansi("12 m² (net)"), "(12 m\\262 \\(net\\))");
        assert_eq!(win_ansi("45°"), "(45\\260)");
        assert_eq!(win_ansi("R→"), "(R?)");
    }

    #[test]
    fn a_count_marks_each_place_and_a_circle_is_round() {
        let count = Markup::new(0, MarkupKind::Count, Geometry::Points { pts: vec![Pt::new(10.0, 10.0), Pt::new(50.0, 20.0)] });
        let content = String::from_utf8(appearance(&count, Some("2")).content).unwrap();
        // Two crosses, four strokes, and no path joining them up.
        assert_eq!(content.matches(" l\n").count(), 4, "{content}");
        assert!(!content.contains(" h\n"));

        let circle = Markup::new(0, MarkupKind::Diameter, Geometry::Ellipse { rect: Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(100.0, 100.0)) });
        let content = String::from_utf8(appearance(&circle, Some("Ø 10 m")).content).unwrap();
        assert_eq!(content.matches(" c").count(), 4, "four curves make a circle: {content}");
        assert!(!content.contains(" re"), "not a box");
    }

    #[test]
    fn an_area_fills_its_outline_and_cutout_by_even_odd_and_covers_its_label() {
        let mut m = Markup::new(
            0,
            MarkupKind::Area,
            Geometry::Polygon {
                pts: vec![Pt::new(0.0, 0.0), Pt::new(100.0, 0.0), Pt::new(100.0, 10.0), Pt::new(0.0, 10.0)],
                holes: vec![vec![Pt::new(10.0, 2.0), Pt::new(20.0, 2.0), Pt::new(20.0, 8.0)]],
            },
        );
        m.style.fill = Some([0.0, 0.5, 1.0]);
        let a = appearance(&m, Some("a long label wider than the shape is tall, surely"));
        let content = String::from_utf8(a.content).unwrap();
        // The outline and its cutout are both closed, and the pair is stated
        // again for each layer that paints them: the fill, then the line.
        assert_eq!(content.matches(" h\n").count(), 4);
        assert!(content.contains("rg f*"), "filled by even-odd: {content}");
        assert!(content.contains("S\n"), "and outlined: {content}");
        assert!(content.find("Q\nBT /Lbl 10 Tf").is_some(), "the label comes after the see-through shape: {content}");
        assert!(a.bbox.max.y > 11.0 || a.bbox.min.x < -1.0, "grown to hold the label: {:?}", a.bbox);
        assert!(a.resources.has(b"Font"));
    }
}

/// One tiling pattern: its name in the resources, its dictionary and the
/// content that draws a single cell.
pub struct TilingPattern {
    pub name: String,
    pub dict: Dictionary,
    pub content: Vec<u8>,
}

/// A pattern that fills with `pattern`, or nothing for a solid fill. Painted
/// uncoloured (`/PaintType 2`), so the one pattern serves every colour and the
/// colour is set where it is used.
/// The pattern that rules a fill, with `cell` points between rulings.
fn tiling(pattern: FillPattern, cell: f64) -> Option<TilingPattern> {
    let c = cell.clamp(1.0, 72.0);
    let mut ops = String::from("0.5 w 1 J\n");
    let line = |ops: &mut String, (x1, y1): (f64, f64), (x2, y2): (f64, f64)| {
        ops.push_str(&format!("{} {} m {} {} l S\n", n(x1), n(y1), n(x2), n(y2)));
    };
    match pattern {
        FillPattern::Solid => return None,
        // Drawn corner to corner, and again through both far corners, so the
        // line carries on unbroken into the cells either side of this one.
        FillPattern::Diagonal => {
            line(&mut ops, (0.0, 0.0), (c, c));
            line(&mut ops, (-c, 0.0), (0.0, c));
            line(&mut ops, (c, 0.0), (2.0 * c, c));
        }
        FillPattern::Cross => {
            line(&mut ops, (0.0, 0.0), (c, c));
            line(&mut ops, (-c, 0.0), (0.0, c));
            line(&mut ops, (c, 0.0), (2.0 * c, c));
            line(&mut ops, (0.0, c), (c, 0.0));
            line(&mut ops, (-c, c), (0.0, 0.0));
            line(&mut ops, (c, c), (2.0 * c, 0.0));
        }
        FillPattern::Horizontal => line(&mut ops, (0.0, c / 2.0), (c, c / 2.0)),
        FillPattern::Vertical => line(&mut ops, (c / 2.0, 0.0), (c / 2.0, c)),
        // A filled dot in the middle of the cell.
        FillPattern::Dots => {
            let r = 0.6;
            let (x, y) = (c / 2.0, c / 2.0);
            ops.push_str(&format!("{} {} m ", n(x + r), n(y)));
            ops.push_str(&format!("{} {} {} {} {} {} c ", n(x + r), n(y + r), n(x - r), n(y + r), n(x - r), n(y)));
            ops.push_str(&format!("{} {} {} {} {} {} c f\n", n(x - r), n(y - r), n(x + r), n(y - r), n(x + r), n(y)));
        }
    }
    Some(TilingPattern {
        name: "P0".to_owned(),
        dict: dictionary! {
            "Type" => "Pattern",
            "PatternType" => 1,
            // Uncoloured: the colour comes from where it is used.
            "PaintType" => 2,
            "TilingType" => 1,
            "BBox" => Object::Array([0.0, 0.0, c, c].into_iter().map(real).collect()),
            "XStep" => real(c),
            "YStep" => real(c),
            "Resources" => Dictionary::new(),
        },
        content: ops.into_bytes(),
    })
}

#[cfg(test)]
mod pattern_tests {
    use super::*;
    use markup_model::markup::FillPattern;
    use markup_model::Geometry;

    fn area(pattern: FillPattern) -> Markup {
        let pts = vec![Pt::new(0.0, 0.0), Pt::new(100.0, 0.0), Pt::new(100.0, 60.0)];
        let mut m = Markup::new(0, MarkupKind::Area, Geometry::Polygon { pts, holes: Vec::new() });
        m.style.fill = Some([0.0, 0.5, 1.0]);
        m.style.pattern = pattern;
        m
    }

    /// A hatch is one small pattern the viewer repeats, not lines written out
    /// across the shape: the content stays a fixed size however large the area.
    #[test]
    fn a_hatch_is_a_pattern_rather_than_ruled_lines() {
        let small = appearance(&area(FillPattern::Diagonal), None);
        let mut big = area(FillPattern::Diagonal);
        big.geometry = Geometry::Polygon {
            pts: vec![Pt::new(0.0, 0.0), Pt::new(5000.0, 0.0), Pt::new(5000.0, 4000.0)],
            holes: Vec::new(),
        };
        let large = appearance(&big, None);
        assert_eq!(small.patterns.len(), 1);
        assert_eq!(large.patterns.len(), 1);
        // Ten times the shape, no more drawing to do.
        assert!(large.content.len() < small.content.len() + 64, "{} vs {}", large.content.len(), small.content.len());
    }

    #[test]
    fn a_patterned_fill_names_the_pattern_and_a_solid_one_does_not() {
        let hatched = appearance(&area(FillPattern::Cross), None);
        let content = String::from_utf8(hatched.content.clone()).unwrap();
        assert!(content.contains("/Pattern cs"), "{content}");
        assert!(content.contains("/P0 scn"), "{content}");

        let solid = appearance(&area(FillPattern::Solid), None);
        let content = String::from_utf8(solid.content.clone()).unwrap();
        assert!(!content.contains("/Pattern"), "{content}");
        assert!(content.contains("rg f*"), "{content}");
        assert!(solid.patterns.is_empty());
    }

    /// The line's transparency and the inside's are written apart, so a shape
    /// can be outlined solidly and filled faintly.
    #[test]
    fn the_line_and_the_inside_carry_their_own_transparency() {
        let mut m = area(FillPattern::Solid);
        m.style.opacity = 1.0;
        m.style.fill_opacity = 0.2;
        let look = appearance(&m, None);
        let states = look.resources.get(b"ExtGState").unwrap().as_dict().unwrap();
        let alpha = |state: &[u8], key: &[u8]| states.get(state).unwrap().as_dict().unwrap().get(key).unwrap().as_float().unwrap();
        // The line solid, the inside faint, and the ruling with its own.
        assert_eq!(alpha(b"GS0", b"CA"), 1.0);
        assert_eq!(alpha(b"GSfill", b"ca"), 0.2);
        assert_eq!(alpha(b"GSpat", b"ca"), 1.0);
    }

    /// An unfilled shape has nothing to pattern.
    #[test]
    fn a_shape_with_no_fill_has_no_pattern() {
        let mut m = area(FillPattern::Dots);
        m.style.fill = None;
        assert!(appearance(&m, None).patterns.is_empty());
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;
    use markup_model::Geometry;

    fn measured() -> Markup {
        Markup::new(0, MarkupKind::Length, Geometry::Line { a: Pt::new(0.0, 0.0), b: Pt::new(100.0, 0.0) })
    }

    /// The quantity takes its own colour, size and face, and the face is one
    /// every viewer has without the file carrying it.
    #[test]
    fn a_quantity_is_written_in_the_face_and_colour_it_is_set_to() {
        let mut m = measured();
        m.style.label_colour = Some([0.0, 0.0, 1.0]);
        m.style.label_size = 14.0;
        m.style.label_font = LabelFont::Mono;
        let look = appearance(&m, Some("12.0 m"));
        let content = String::from_utf8(look.content).unwrap();
        assert!(content.contains("BT /Lbl 14 Tf 0 0 1 rg"), "{content}");
        let font = look.resources.get(b"Font").unwrap().as_dict().unwrap().get(b"Lbl").unwrap().as_dict().unwrap();
        assert_eq!(font.get(b"BaseFont").unwrap().as_name().unwrap(), b"Courier");
    }

    /// Without a colour of its own it follows the line, as it always did.
    #[test]
    fn a_quantity_with_no_colour_of_its_own_follows_the_line() {
        let mut m = measured();
        m.style.stroke = [1.0, 0.0, 0.0];
        assert_eq!(m.style.label_colour, None);
        let content = String::from_utf8(appearance(&m, Some("12.0 m")).content).unwrap();
        assert!(content.contains("BT /Lbl 10 Tf 1 0 0 rg"), "{content}");
    }
}
