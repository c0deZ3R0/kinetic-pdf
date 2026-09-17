//! The /AP /N appearance stream of a measurement markup, so viewers that
//! don't draw from /Vertices and /Measure -- and pdfium -- show what we show.
//! The app itself draws markups from the model, never from this.

use markup_model::markup::{Geometry, Markup, Style};
use markup_model::{Pt, Rect};
use pdf_content::lopdf::{dictionary, Dictionary, Object};

use crate::values::real;

pub struct Appearance {
    pub content: Vec<u8>,
    pub resources: Dictionary,
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
    let Style { stroke, fill, opacity, width, ref dash, label_size, .. } = m.style;
    let mut ops = String::from("q /GS0 gs\n");
    let [r, g, b] = stroke.map(f64::from);
    ops.push_str(&format!("{} {} {} RG {} w 1 J 1 j\n", n(r), n(g), n(b), n(width)));
    if !dash.is_empty() {
        let lengths: Vec<String> = dash.iter().map(|&d| n(d)).collect();
        ops.push_str(&format!("[{}] 0 d\n", lengths.join(" ")));
    }
    let closed = matches!(m.geometry, Geometry::Polygon { .. });
    match &m.geometry {
        Geometry::Line { a, b } => path(&mut ops, &[*a, *b], false),
        Geometry::Polyline { pts } | Geometry::Points { pts } => path(&mut ops, pts, false),
        Geometry::Polygon { pts, holes } => {
            path(&mut ops, pts, true);
            holes.iter().for_each(|h| path(&mut ops, h, true));
        }
        Geometry::Ink { strokes } => strokes.iter().for_each(|s| path(&mut ops, s, false)),
        Geometry::Ellipse { rect } => {
            ops.push_str(&format!("{} {} {} {} re\n", n(rect.min.x), n(rect.min.y), n(rect.width()), n(rect.height())));
        }
    }
    match (closed, fill) {
        (true, Some([fr, fg, fb])) => ops.push_str(&format!("{} {} {} rg B*\n", n(f64::from(fr)), n(f64::from(fg)), n(f64::from(fb)))),
        (true, None) => ops.push_str("s\n"),
        (false, _) => ops.push_str("S\n"),
    }
    // The label is drawn at full strength, whatever the shape's opacity.
    ops.push_str("Q\n");

    let mut bbox = m.geometry.bounds().unwrap_or(Rect::from_corners(Pt::default(), Pt::default())).expand(width.max(0.0) / 2.0 + 1.0);
    let mut resources = dictionary! {
        "ExtGState" => dictionary! { "GS0" => dictionary! { "Type" => "ExtGState", "CA" => real(f64::from(opacity)), "ca" => real(f64::from(opacity)) } },
    };
    if let (Some(label), Some(at)) = (label.filter(|l| !l.is_empty()), label_anchor(&m.geometry)) {
        // Helvetica's characters average about half the font size across.
        let half_width = 0.28 * label_size * label.chars().count() as f64;
        // Just above a line or path, so the text doesn't strike through it;
        // centred in an area.
        let lift = if closed { -label_size * 0.35 } else { width / 2.0 + label_size * 0.3 };
        let (x, y) = (at.x - half_width, at.y + lift);
        ops.push_str(&format!("BT /Helv {} Tf {} {} {} rg {} {} Td {} Tj ET\n", n(label_size), n(r), n(g), n(b), n(x), n(y), win_ansi(label)));
        bbox = bbox.union(Rect::from_corners(Pt::new(x - 1.0, y - label_size * 0.3), Pt::new(at.x + half_width + 1.0, y + label_size)));
        resources.set(
            "Font",
            dictionary! { "Helv" => dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" } },
        );
    }
    Appearance { content: ops.into_bytes(), resources, bbox }
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
        assert_eq!(content.matches(" h\n").count(), 2);
        assert!(content.contains("rg B*"));
        assert!(content.find("Q\nBT /Helv 10 Tf").is_some(), "the label comes after the see-through shape: {content}");
        assert!(a.bbox.max.y > 11.0 || a.bbox.min.x < -1.0, "grown to hold the label: {:?}", a.bbox);
        assert!(a.resources.has(b"Font"));
    }
}
