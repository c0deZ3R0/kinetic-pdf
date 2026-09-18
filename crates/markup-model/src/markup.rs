//! A markup as data: its kind, geometry in PDF points, style, and the facts
//! an estimator attaches to it.
//!
//! Style and metadata are plain serialisable data kept apart from any tool
//! logic, so reusable tool presets can be stored later without changing them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::geom::{Pt, Rect};
use crate::id::{MarkupId, PageIndex};
use crate::viewport::ScaleRef;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Markup {
    /// Written to /NM.
    pub id: MarkupId,
    pub page: PageIndex,
    pub kind: MarkupKind,
    /// In user space.
    pub geometry: Geometry,
    pub style: Style,
    pub meta: MarkupMeta,
    pub scale_ref: ScaleRef,
    pub extras: Extras,
}

impl Markup {
    /// A new markup with default style, metadata and extras.
    pub fn new(page: PageIndex, kind: MarkupKind, geometry: Geometry) -> Markup {
        Markup {
            id: MarkupId::new(),
            page,
            kind,
            geometry,
            style: Style::default(),
            meta: MarkupMeta::default(),
            scale_ref: ScaleRef::Page,
            extras: Extras::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarkupKind {
    Length,
    Polylength,
    Area,
    Perimeter,
    Count,
    Angle,
    Radius,
    Diameter,
    Volume,
    Text,
    Cloud,
    Highlight,
    Pen,
    Box,
    Ellipse,
    Arrow,
}

impl MarkupKind {
    /// Whether it's a measurement, with quantities, rather than a plain
    /// drawing or note.
    /// What this kind is called, for a list or a heading.
    pub fn label(self) -> &'static str {
        match self {
            MarkupKind::Length => "Length",
            MarkupKind::Polylength => "Polylength",
            MarkupKind::Area => "Area",
            MarkupKind::Perimeter => "Perimeter",
            MarkupKind::Count => "Count",
            MarkupKind::Angle => "Angle",
            MarkupKind::Radius => "Radius",
            MarkupKind::Diameter => "Diameter",
            MarkupKind::Volume => "Volume",
            MarkupKind::Text => "Text",
            MarkupKind::Cloud => "Cloud",
            MarkupKind::Highlight => "Highlight",
            MarkupKind::Pen => "Pen",
            MarkupKind::Box => "Box",
            MarkupKind::Ellipse => "Ellipse",
            MarkupKind::Arrow => "Arrow",
        }
    }

    pub fn is_measurement(self) -> bool {
        use MarkupKind::*;
        matches!(self, Length | Polylength | Area | Perimeter | Count | Angle | Radius | Diameter | Volume)
    }

    /// Whether its quantities need a scale. Counts and angles don't.
    pub fn needs_scale(self) -> bool {
        self.is_measurement() && !matches!(self, MarkupKind::Count | MarkupKind::Angle)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Geometry {
    Line { a: Pt, b: Pt },
    Polyline { pts: Vec<Pt> },
    /// A closed ring, not repeating its first point, with cutouts.
    Polygon { pts: Vec<Pt>, holes: Vec<Vec<Pt>> },
    /// A count's marks.
    Points { pts: Vec<Pt> },
    Ellipse { rect: Rect },
    Ink { strokes: Vec<Vec<Pt>> },
}

impl Geometry {
    /// Where scale resolution looks: the first point placed.
    pub fn first_point(&self) -> Option<Pt> {
        match self {
            Geometry::Line { a, .. } => Some(*a),
            Geometry::Polyline { pts } | Geometry::Polygon { pts, .. } | Geometry::Points { pts } => pts.first().copied(),
            Geometry::Ellipse { rect } => Some(rect.center()),
            Geometry::Ink { strokes } => strokes.iter().flatten().next().copied(),
        }
    }

    /// The box around every point, cutouts and an ellipse's whole box
    /// included. Strokes and labels aren't counted.
    pub fn bounds(&self) -> Option<Rect> {
        match self {
            Geometry::Line { a, b } => Some(Rect::from_corners(*a, *b)),
            Geometry::Polyline { pts } | Geometry::Points { pts } => Rect::around(pts),
            Geometry::Polygon { pts, holes } => Rect::around(pts.iter().chain(holes.iter().flatten())),
            Geometry::Ellipse { rect } => Some(*rect),
            Geometry::Ink { strokes } => Rect::around(strokes.iter().flatten()),
        }
    }

    /// Every point, in a fixed order: the order handles and hashes use.
    pub fn rings(&self) -> Vec<&[Pt]> {
        match self {
            Geometry::Line { a, b } => vec![std::slice::from_ref(a), std::slice::from_ref(b)],
            Geometry::Polyline { pts } | Geometry::Points { pts } => vec![pts],
            Geometry::Polygon { pts, holes } => std::iter::once(pts.as_slice()).chain(holes.iter().map(Vec::as_slice)).collect(),
            Geometry::Ellipse { rect } => vec![std::slice::from_ref(&rect.min), std::slice::from_ref(&rect.max)],
            Geometry::Ink { strokes } => strokes.iter().map(Vec::as_slice).collect(),
        }
    }

    pub fn is_finite(&self) -> bool {
        self.rings().iter().all(|r| r.iter().all(|p| p.is_finite()))
    }

    /// Moves every point by `d`.
    pub fn translate(&mut self, d: Pt) {
        self.for_each_point_mut(|p| *p = *p + d);
    }

    fn for_each_point_mut(&mut self, mut f: impl FnMut(&mut Pt)) {
        match self {
            Geometry::Line { a, b } => {
                f(a);
                f(b);
            }
            Geometry::Polyline { pts } | Geometry::Points { pts } => pts.iter_mut().for_each(f),
            Geometry::Polygon { pts, holes } => pts.iter_mut().chain(holes.iter_mut().flatten()).for_each(f),
            Geometry::Ellipse { rect } => {
                f(&mut rect.min);
                f(&mut rect.max);
                *rect = Rect::from_corners(rect.min, rect.max);
            }
            Geometry::Ink { strokes } => strokes.iter_mut().flatten().for_each(f),
        }
    }

    /// Puts `point` into ring `ring` at `index`, moving what follows along:
    /// how a point is added on an edge. Says whether it could.
    pub fn insert_vertex(&mut self, ring: usize, index: usize, point: Pt) -> bool {
        let pts = match self {
            Geometry::Polyline { pts } | Geometry::Points { pts } if ring == 0 => pts,
            Geometry::Polygon { pts, .. } if ring == 0 => pts,
            Geometry::Polygon { holes, .. } => match holes.get_mut(ring.wrapping_sub(1)) {
                Some(hole) => hole,
                None => return false,
            },
            Geometry::Ink { strokes } => match strokes.get_mut(ring) {
                Some(stroke) => stroke,
                None => return false,
            },
            // A line has two ends and no room for a third point; adding one
            // makes it a run, which is a different kind of measurement.
            Geometry::Line { .. } | Geometry::Ellipse { .. } | Geometry::Polyline { .. } | Geometry::Points { .. } => return false,
        };
        if index > pts.len() {
            return false;
        }
        pts.insert(index, point);
        true
    }

    /// Takes a point out of a ring, if the shape still has enough points
    /// afterwards: two for a run, three for an area, one for a cutout to be
    /// worth keeping. Says whether it could.
    pub fn remove_vertex(&mut self, ring: usize, index: usize) -> bool {
        let least = match (&self, ring) {
            (Geometry::Polygon { .. }, _) => 3,
            (Geometry::Points { .. }, _) => 1,
            _ => 2,
        };
        let pts = match self {
            Geometry::Polyline { pts } | Geometry::Points { pts } if ring == 0 => pts,
            Geometry::Polygon { pts, .. } if ring == 0 => pts,
            Geometry::Polygon { holes, .. } => match holes.get_mut(ring.wrapping_sub(1)) {
                Some(hole) => hole,
                None => return false,
            },
            Geometry::Ink { strokes } => match strokes.get_mut(ring) {
                Some(stroke) => stroke,
                None => return false,
            },
            Geometry::Line { .. } | Geometry::Ellipse { .. } | Geometry::Polyline { .. } | Geometry::Points { .. } => return false,
        };
        if index >= pts.len() || pts.len() <= least {
            return false;
        }
        pts.remove(index);
        true
    }

    /// Moves every point by `d`, which is how a whole measurement is moved.
    pub fn moved_by(&self, d: Pt) -> Geometry {
        let mut moved = self.clone();
        moved.translate(d);
        moved
    }

    /// A mutable handle to one vertex, addressed as `Hit` addresses it: ring
    /// 0 the outline, cutouts from 1 (or an ink stroke's number).
    pub fn vertex_mut(&mut self, ring: usize, index: usize) -> Option<&mut Pt> {
        match self {
            Geometry::Line { a, b } => match (ring, index) {
                (0, 0) => Some(a),
                (0, 1) => Some(b),
                _ => None,
            },
            Geometry::Polyline { pts } | Geometry::Points { pts } => (ring == 0).then(|| pts.get_mut(index)).flatten(),
            Geometry::Polygon { pts, holes } => match ring {
                0 => pts.get_mut(index),
                r => holes.get_mut(r - 1)?.get_mut(index),
            },
            Geometry::Ellipse { .. } => None,
            Geometry::Ink { strokes } => strokes.get_mut(ring)?.get_mut(index),
        }
    }

    pub fn vertex(&self, ring: usize, index: usize) -> Option<Pt> {
        match self {
            Geometry::Line { a, b } => match (ring, index) {
                (0, 0) => Some(*a),
                (0, 1) => Some(*b),
                _ => None,
            },
            Geometry::Polyline { pts } | Geometry::Points { pts } => (ring == 0).then(|| pts.get(index).copied()).flatten(),
            Geometry::Polygon { pts, holes } => match ring {
                0 => pts.get(index).copied(),
                r => holes.get(r - 1)?.get(index).copied(),
            },
            Geometry::Ellipse { .. } => None,
            Geometry::Ink { strokes } => strokes.get(ring)?.get(index).copied(),
        }
    }
}

/// A colour as 0..1 RGB, the form /C and /IC use.
pub type Rgb = [f32; 3];

/// What a stroke width is measured in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WidthUnit {
    /// Points on the page: thicker as you zoom in, as printed.
    #[default]
    Points,
    /// Pixels on screen, the same at any zoom. Written to the file as points
    /// at 100% zoom.
    ScreenPixels,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub stroke: Rgb,
    pub fill: Option<Rgb>,
    /// 0..1, for stroke and fill together, as /CA.
    pub opacity: f32,
    pub width: f64,
    pub width_unit: WidthUnit,
    /// Dash and gap lengths, in the width's unit. Empty for a solid line.
    pub dash: Vec<f64>,
    /// The label's font size in points.
    pub label_size: f64,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            stroke: [0.86, 0.15, 0.15],
            fill: None,
            opacity: 1.0,
            width: 1.0,
            width_unit: WidthUnit::Points,
            dash: Vec::new(),
            label_size: 10.0,
        }
    }
}

/// A value in a markup's custom columns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MetaValue {
    Text(String),
    Number(f64),
    Bool(bool),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MarkupMeta {
    /// What the measurement is called, shown ahead of the description.
    pub name: String,
    pub label: String,
    /// /Subj.
    pub subject: String,
    /// /T.
    pub author: String,
    /// Milliseconds since 1970 UTC; /CreationDate.
    pub created_ms: Option<i64>,
    /// Milliseconds since 1970 UTC; /M.
    pub modified_ms: Option<i64>,
    pub layer: Option<String>,
    pub status: Option<String>,
    /// A bill-of-quantities item code, `A-120`.
    pub item_code: Option<String>,
    /// Custom columns, by name.
    pub custom: BTreeMap<String, MetaValue>,
}

/// How steep the surface a measurement lies on is: `rise` over `run`, in any
/// one unit. Plan lengths and areas are divided by its cosine.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slope {
    pub rise: f64,
    pub run: f64,
}

impl Slope {
    pub fn from_degrees(degrees: f64) -> Slope {
        let radians = degrees.to_radians();
        Slope { rise: radians.sin(), run: radians.cos() }
    }

    pub fn degrees(self) -> f64 {
        self.rise.atan2(self.run).to_degrees()
    }

    /// What a plan length or area is multiplied by: 1/cos θ. `None` for a
    /// vertical or undefined slope, which has no finite factor.
    pub fn factor(self) -> Option<f64> {
        let factor = self.rise.hypot(self.run) / self.run.abs();
        (factor.is_finite() && self.run != 0.0).then_some(factor)
    }
}

/// A PDF value kept as it was, for keys read from a file that nothing here
/// models. Mirrors PDF's object types without depending on a PDF library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RawValue {
    Null,
    Bool(bool),
    Integer(i64),
    Real(f64),
    Name(Vec<u8>),
    String(Vec<u8>),
    Array(Vec<RawValue>),
    Dictionary(BTreeMap<Vec<u8>, RawValue>),
    /// Object number and generation: only meaningful in the file it came
    /// from.
    Reference(u32, u16),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Extras {
    /// For volumes: metres below (or above) the area.
    pub depth_m: Option<f64>,
    pub slope: Option<Slope>,
    /// The group the markup belongs to, if any.
    pub group: Option<String>,
    /// The /NM a markup from another program came with, written back
    /// unchanged.
    pub foreign_nm: Option<String>,
    /// Set on load when the geometry no longer matches the hash this app
    /// wrote: something else edited the markup.
    pub changed_externally: bool,
    /// Annotation keys read from the file that nothing here models, and
    /// unknown /KPDF keys from newer versions, preserved for writing back.
    pub raw: BTreeMap<Vec<u8>, RawValue>,
    /// Unknown keys found inside /KPDF, kept apart from the annotation's own.
    pub raw_kpdf: BTreeMap<Vec<u8>, RawValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slopes_as_factors_on_plan_measurements() {
        let s = Slope { rise: 3.0, run: 4.0 };
        assert!((s.factor().unwrap() - 1.25).abs() < 1e-12);
        assert!((Slope::from_degrees(60.0).factor().unwrap() - 2.0).abs() < 1e-12);
        assert!((Slope::from_degrees(30.0).degrees() - 30.0).abs() < 1e-12);
        assert_eq!(Slope { rise: 1.0, run: 0.0 }.factor(), None, "vertical");
        assert_eq!(Slope { rise: 0.0, run: 1.0 }.factor(), Some(1.0), "flat");
    }

    #[test]
    fn geometry_bounds_include_cutouts_and_translate_moves_everything() {
        let mut g = Geometry::Polygon {
            pts: vec![Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(10.0, 10.0)],
            holes: vec![vec![Pt::new(-1.0, 2.0), Pt::new(3.0, 2.0), Pt::new(3.0, 11.0)]],
        };
        assert_eq!(g.bounds(), Some(Rect::from_corners(Pt::new(-1.0, 0.0), Pt::new(10.0, 11.0))));
        g.translate(Pt::new(1.0, 1.0));
        assert_eq!(g.vertex(1, 0), Some(Pt::new(0.0, 3.0)));
        *g.vertex_mut(0, 2).unwrap() = Pt::new(20.0, 20.0);
        assert_eq!(g.first_point(), Some(Pt::new(1.0, 1.0)));
        assert_eq!(g.bounds().unwrap().max, Pt::new(20.0, 20.0));
    }

    #[test]
    fn points_are_added_on_an_edge_and_taken_out_again() {
        let mut area = Geometry::Polygon {
            pts: vec![Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(10.0, 10.0)],
            holes: vec![vec![Pt::new(2.0, 2.0), Pt::new(4.0, 2.0), Pt::new(4.0, 4.0)]],
        };
        assert!(area.insert_vertex(0, 1, Pt::new(5.0, -1.0)));
        assert_eq!(area.vertex(0, 1), Some(Pt::new(5.0, -1.0)));
        assert!(area.insert_vertex(1, 3, Pt::new(2.0, 4.0)), "into a cutout");
        assert!(!area.insert_vertex(2, 0, Pt::new(0.0, 0.0)), "no third ring");

        assert!(area.remove_vertex(0, 1));
        assert_eq!(area.vertex(0, 1), Some(Pt::new(10.0, 0.0)));
        // An area keeps three points, a cutout too.
        assert!(!area.remove_vertex(0, 0) && !area.remove_vertex(0, 1) || area.rings()[0].len() == 3);
        let mut three = Geometry::Polygon { pts: vec![Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(10.0, 10.0)], holes: vec![] };
        assert!(!three.remove_vertex(0, 0), "an area can't go below three points");
    }

    #[test]
    fn a_run_keeps_two_points_and_a_line_takes_none() {
        let mut run = Geometry::Polyline { pts: vec![Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(20.0, 0.0)] };
        assert!(run.remove_vertex(0, 1));
        assert!(!run.remove_vertex(0, 0), "a run can't go below two");
        let mut line = Geometry::Line { a: Pt::new(0.0, 0.0), b: Pt::new(10.0, 0.0) };
        assert!(!line.insert_vertex(0, 1, Pt::new(5.0, 5.0)) && !line.remove_vertex(0, 0));
    }

    #[test]
    fn moving_a_shape_moves_its_cutouts_with_it() {
        let area = Geometry::Polygon {
            pts: vec![Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(10.0, 10.0)],
            holes: vec![vec![Pt::new(2.0, 2.0), Pt::new(4.0, 2.0), Pt::new(4.0, 4.0)]],
        };
        let moved = area.moved_by(Pt::new(100.0, -5.0));
        assert_eq!(moved.vertex(0, 0), Some(Pt::new(100.0, -5.0)));
        assert_eq!(moved.vertex(1, 0), Some(Pt::new(102.0, -3.0)));
        assert_eq!(moved.bounds().unwrap().width(), area.bounds().unwrap().width(), "the same shape, elsewhere");
    }

    #[test]
    fn counts_and_angles_need_no_scale() {
        assert!(!MarkupKind::Count.needs_scale());
        assert!(!MarkupKind::Angle.needs_scale());
        assert!(MarkupKind::Area.needs_scale());
        assert!(!MarkupKind::Pen.is_measurement());
    }
}
