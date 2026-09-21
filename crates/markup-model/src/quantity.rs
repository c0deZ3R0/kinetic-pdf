//! Quantities from geometry and a scale: the numbers a tender is priced on.
//!
//! Pure functions of a markup and the scale it resolves to. Stored numbers
//! are only ever a cache of these. A shape that can't give a trustworthy
//! number -- an area whose outline crosses itself, a cutout outside its area --
//! gives an error for the UI to show, never a wrong number.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::geom::{self, Pt};
use crate::hit::{self, Hit};
use crate::markup::{Geometry, Markup, MarkupKind};
use crate::scale::Scale;
use crate::units::{format_angle, format_area, format_length, format_volume, group_thousands, DisplayUnits, Precision};

/// What a markup measures, in metres, square metres and cubic metres. Only
/// the fields its kind gives are set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Quantities {
    pub length_m: Option<f64>,
    pub area_m2: Option<f64>,
    pub perimeter_m: Option<f64>,
    pub volume_m3: Option<f64>,
    pub count: Option<u64>,
    pub angle_deg: Option<f64>,
    pub radius_m: Option<f64>,
    pub diameter_m: Option<f64>,
    /// The kind needs a scale and none applies: every scaled field is `None`,
    /// and the markup is left out of totals.
    pub uncalibrated: bool,
}

impl Quantities {
    /// The quantity a markup of `kind` is about, as text for its label and
    /// the file's /Contents: `188.00 m²`, `12' 6"`, `42`, `45.0°`. An area,
    /// perimeter or volume markup shows the one its kind names. `None` for a
    /// kind with no quantity; `uncalibrated` when it needs a scale.
    pub fn text(&self, kind: MarkupKind, units: &DisplayUnits, precision: Precision) -> Option<String> {
        if self.uncalibrated {
            return Some("uncalibrated".to_owned());
        }
        let length = |m: Option<f64>| m.map(|m| format_length(m, units.length, precision));
        match kind {
            MarkupKind::Length | MarkupKind::Polylength => length(self.length_m),
            MarkupKind::Area => self.area_m2.map(|a| format_area(a, units.area, precision)),
            MarkupKind::Perimeter => length(self.perimeter_m),
            MarkupKind::Volume => match self.volume_m3 {
                Some(v) => Some(format_volume(v, units.volume, precision)),
                None => self.area_m2.map(|a| format!("{} (no depth)", format_area(a, units.area, precision))),
            },
            MarkupKind::Count => self.count.map(|c| group_thousands(c as f64, 0)),
            MarkupKind::Angle => self.angle_deg.map(|a| format_angle(a, precision)),
            MarkupKind::Radius => length(self.radius_m).map(|r| format!("R {r}")),
            MarkupKind::Diameter => length(self.diameter_m).map(|d| format!("Ø {d}")),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuantityError {
    /// The geometry isn't the shape this kind is measured from.
    WrongGeometry,
    TooFewPoints { need: usize, got: usize },
    /// An area's outline crosses or touches itself.
    SelfIntersecting,
    /// A cutout crosses itself, its area's outline or another cutout, or
    /// lies outside the area.
    BadCutout { index: usize },
    /// Three points of an arc lie on a line, so there's no circle through them.
    Collinear,
    /// A diameter's box isn't square once scaled, so it isn't a circle.
    NotCircular,
    /// A coordinate, depth or slope is infinite or not a number, or the
    /// slope is vertical.
    NotFinite,
}

impl fmt::Display for QuantityError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            QuantityError::WrongGeometry => write!(f, "this shape can't be measured this way"),
            QuantityError::TooFewPoints { need, got } => write!(f, "needs {need} points, has {got}"),
            QuantityError::SelfIntersecting => write!(f, "the outline crosses itself, so its area can't be trusted"),
            QuantityError::BadCutout { index } => write!(f, "cutout {} crosses the outline or lies outside it", index + 1),
            QuantityError::Collinear => write!(f, "the three points are in a straight line"),
            QuantityError::NotCircular => write!(f, "the shape isn't a circle at this scale"),
            QuantityError::NotFinite => write!(f, "a number is out of range"),
        }
    }
}

impl std::error::Error for QuantityError {}

/// A kind of markup's measuring and picking. Rendering and PDF reading and
/// writing are added per kind by the crates that own them, through their own
/// traits, so this crate stays free of GPU and PDF code.
pub trait Measure: Send + Sync {
    /// The markup's quantities with `scale`, or with none.
    fn quantities(&self, m: &Markup, scale: Option<&Scale>) -> Result<Quantities, QuantityError>;

    /// What of the markup, if anything, is within `tolerance` points of `p`.
    fn hit_test(&self, m: &Markup, p: Pt, tolerance: f64) -> Option<Hit> {
        hit::hit_test(&m.geometry, p, tolerance)
    }
}

impl MarkupKind {
    /// How this kind is measured.
    pub fn measure(self) -> &'static dyn Measure {
        use MarkupKind::*;
        match self {
            Length | Polylength => &Linear,
            Area | Perimeter | Volume => &Areal,
            Count => &Counted,
            Angle => &Angular,
            Radius | Diameter => &Radial,
            Text | Cloud | Highlight | Pen | Box | Ellipse | Arrow => &Unmeasured,
        }
    }
}

/// The quantities of `m` measured with `scale`, if it has one.
pub fn quantities(m: &Markup, scale: Option<&Scale>) -> Result<Quantities, QuantityError> {
    if !m.geometry.is_finite() {
        return Err(QuantityError::NotFinite);
    }
    m.kind.measure().quantities(m, scale)
}

/// Metres per point along x and y, or an uncalibrated result for a kind that
/// needs them.
fn per_point(scale: Option<&Scale>) -> Option<(f64, f64)> {
    scale.map(|s| (s.metres_per_point_x, s.metres_per_point_y))
}

fn uncalibrated() -> Quantities {
    Quantities { uncalibrated: true, ..Quantities::default() }
}

fn slope_factor(m: &Markup) -> Result<f64, QuantityError> {
    match m.extras.slope {
        None => Ok(1.0),
        Some(s) => s.factor().ok_or(QuantityError::NotFinite),
    }
}

fn need(points: &[Pt], least: usize) -> Result<(), QuantityError> {
    if points.len() < least {
        Err(QuantityError::TooFewPoints { need: least, got: points.len() })
    } else {
        Ok(())
    }
}

struct Linear;

impl Measure for Linear {
    fn quantities(&self, m: &Markup, scale: Option<&Scale>) -> Result<Quantities, QuantityError> {
        let line;
        let points: &[Pt] = match &m.geometry {
            Geometry::Line { a, b } => {
                line = [*a, *b];
                &line
            }
            Geometry::Polyline { pts } => pts,
            _ => return Err(QuantityError::WrongGeometry),
        };
        need(points, 2)?;
        let slope = slope_factor(m)?;
        let Some((sx, sy)) = per_point(scale) else { return Ok(uncalibrated()) };
        Ok(Quantities { length_m: Some(geom::path_length(points, sx, sy) * slope), ..Quantities::default() })
    }
}

struct Areal;

/// Checks an area's outline and cutouts make a shape whose area is the
/// outline's less the cutouts', giving the cleaned-up rings.
fn valid_rings(pts: &[Pt], holes: &[Vec<Pt>]) -> Result<(Vec<Pt>, Vec<Vec<Pt>>), QuantityError> {
    let outline = geom::dedup_ring(pts);
    need(&outline, 3)?;
    if !geom::is_simple(&outline) {
        return Err(QuantityError::SelfIntersecting);
    }
    let holes: Vec<Vec<Pt>> = holes.iter().map(|h| geom::dedup_ring(h)).collect();
    for (index, hole) in holes.iter().enumerate() {
        let bad = !geom::is_simple(hole)
            || geom::rings_cross(hole, &outline)
            || !geom::point_in_ring(hole[0], &outline)
            || holes[..index].iter().any(|other| geom::rings_cross(hole, other) || geom::point_in_ring(hole[0], other) || geom::point_in_ring(other[0], hole));
        if bad {
            return Err(QuantityError::BadCutout { index });
        }
    }
    Ok((outline, holes))
}

impl Measure for Areal {
    fn quantities(&self, m: &Markup, scale: Option<&Scale>) -> Result<Quantities, QuantityError> {
        let Geometry::Polygon { pts, holes } = &m.geometry else { return Err(QuantityError::WrongGeometry) };
        let (outline, holes) = valid_rings(pts, holes)?;
        let slope = slope_factor(m)?;
        let depth = m.extras.depth_m;
        if depth.is_some_and(|d| !d.is_finite()) {
            return Err(QuantityError::NotFinite);
        }
        let Some((sx, sy)) = per_point(scale) else { return Ok(uncalibrated()) };
        let plan_points2 = geom::signed_area(&outline).abs() - holes.iter().map(|h| geom::signed_area(h).abs()).sum::<f64>();
        let plan = plan_points2 * sx * sy;
        Ok(Quantities {
            area_m2: Some(plan * slope),
            // The outline only: cutouts are voids within it, not edges to
            // price, and the slope isn't applied along an edge of unknown
            // direction.
            perimeter_m: Some(geom::ring_perimeter(&outline, sx, sy)),
            // The plan area times a vertical depth, which is the volume
            // whatever the slope.
            volume_m3: match (m.kind, depth) {
                (MarkupKind::Volume, Some(d)) => Some(plan * d),
                _ => None,
            },
            ..Quantities::default()
        })
    }
}

struct Counted;

impl Measure for Counted {
    fn quantities(&self, m: &Markup, _: Option<&Scale>) -> Result<Quantities, QuantityError> {
        let Geometry::Points { pts } = &m.geometry else { return Err(QuantityError::WrongGeometry) };
        Ok(Quantities { count: Some(pts.len() as u64), ..Quantities::default() })
    }
}

struct Angular;

impl Measure for Angular {
    fn quantities(&self, m: &Markup, scale: Option<&Scale>) -> Result<Quantities, QuantityError> {
        let Geometry::Polyline { pts } = &m.geometry else { return Err(QuantityError::WrongGeometry) };
        let [a, vertex, b] = pts[..] else { return Err(QuantityError::TooFewPoints { need: 3, got: pts.len() }) };
        // Measured on the real shape, so an exaggerated vertical scale gives
        // the real angle rather than the drawn one.
        let real = |p: Pt| scale.map_or(p, |s| s.to_real(p));
        let angle = geom::angle_between(real(a), real(vertex), real(b)).ok_or_else(|| {
            // An arm with no length: fewer than three distinct points.
            let distinct = 1 + usize::from(a != vertex) + usize::from(b != vertex && b != a);
            QuantityError::TooFewPoints { need: 3, got: distinct }
        })?;
        Ok(Quantities { angle_deg: Some(angle), ..Quantities::default() })
    }
}

struct Radial;

impl Measure for Radial {
    fn quantities(&self, m: &Markup, scale: Option<&Scale>) -> Result<Quantities, QuantityError> {
        let Some(s) = scale else {
            return match m.geometry {
                Geometry::Line { .. } | Geometry::Polyline { .. } | Geometry::Ellipse { .. } => Ok(uncalibrated()),
                _ => Err(QuantityError::WrongGeometry),
            };
        };
        let radius = match &m.geometry {
            // A line drawn for a radius runs from the centre out; one drawn
            // for a diameter runs right across, so it is already the whole
            // width and counts as two radii.
            Geometry::Line { a, b } => s.distance(*a, *b) / if m.kind == MarkupKind::Diameter { 2.0 } else { 1.0 },
            // Three points on the arc.
            Geometry::Polyline { pts } => {
                let [a, b, c] = pts[..] else { return Err(QuantityError::TooFewPoints { need: 3, got: pts.len() }) };
                geom::circle_through(s.to_real(a), s.to_real(b), s.to_real(c)).ok_or(QuantityError::Collinear)?.1
            }
            Geometry::Ellipse { rect } => {
                let (w, h) = (rect.width() * s.metres_per_point_x, rect.height() * s.metres_per_point_y);
                if (w - h).abs() > 0.01 * w.max(h) {
                    return Err(QuantityError::NotCircular);
                }
                (w + h) / 4.0
            }
            _ => return Err(QuantityError::WrongGeometry),
        };
        Ok(match m.kind {
            MarkupKind::Diameter => Quantities { diameter_m: Some(radius * 2.0), ..Quantities::default() },
            _ => Quantities { radius_m: Some(radius), ..Quantities::default() },
        })
    }
}

struct Unmeasured;

impl Measure for Unmeasured {
    fn quantities(&self, _: &Markup, _: Option<&Scale>) -> Result<Quantities, QuantityError> {
        Ok(Quantities::default())
    }
}

/// Sums of measurements that can be added up: lengths, areas, perimeters,
/// volumes and counts. Uncalibrated and failed measurements are counted but
/// left out of the sums.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    pub length_m: f64,
    pub area_m2: f64,
    pub perimeter_m: f64,
    pub volume_m3: f64,
    pub count: u64,
    /// Measurements in the sums.
    pub included: usize,
    /// Measurements left out: uncalibrated, or with an error.
    pub excluded: usize,
}

impl Totals {
    pub fn add(&mut self, result: &Result<Quantities, QuantityError>) {
        match result {
            Ok(q) if !q.uncalibrated => {
                self.length_m += q.length_m.unwrap_or(0.0);
                self.area_m2 += q.area_m2.unwrap_or(0.0);
                self.perimeter_m += q.perimeter_m.unwrap_or(0.0);
                self.volume_m3 += q.volume_m3.unwrap_or(0.0);
                self.count += q.count.unwrap_or(0);
                self.included += 1;
            }
            _ => self.excluded += 1,
        }
    }
}

impl<'a> FromIterator<&'a Result<Quantities, QuantityError>> for Totals {
    fn from_iter<I: IntoIterator<Item = &'a Result<Quantities, QuantityError>>>(iter: I) -> Totals {
        let mut totals = Totals::default();
        iter.into_iter().for_each(|r| totals.add(r));
        totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ScaleId;
    use crate::markup::Slope;
    use crate::units::DisplayUnits;

    fn p(x: f64, y: f64) -> Pt {
        Pt::new(x, y)
    }

    /// A scale where one point is one metre, so hand-calculated numbers read
    /// straight off the coordinates.
    fn metre_per_point() -> Scale {
        Scale::uniform(ScaleId(1), 1.0, "1 pt = 1 m").unwrap()
    }

    fn polygon(kind: MarkupKind, pts: &[(f64, f64)], holes: &[&[(f64, f64)]]) -> Markup {
        let ring = |r: &[(f64, f64)]| r.iter().map(|&(x, y)| p(x, y)).collect::<Vec<_>>();
        Markup::new(0, kind, Geometry::Polygon { pts: ring(pts), holes: holes.iter().map(|h| ring(h)).collect() })
    }

    #[test]
    fn a_length_at_1_to_100() {
        let s = Scale::from_ratio(ScaleId(1), 100.0).unwrap();
        // Two inches on paper: 5.08 m.
        let m = Markup::new(0, MarkupKind::Length, Geometry::Line { a: p(10.0, 10.0), b: p(10.0, 154.0) });
        let q = quantities(&m, Some(&s)).unwrap();
        assert!((q.length_m.unwrap() - 5.08).abs() < 1e-12);
    }

    #[test]
    fn a_polylength_adds_its_segments_and_its_slope() {
        let mut m = Markup::new(0, MarkupKind::Polylength, Geometry::Polyline { pts: vec![p(0.0, 0.0), p(3.0, 4.0), p(3.0, 10.0)] });
        assert_eq!(quantities(&m, Some(&metre_per_point())).unwrap().length_m, Some(11.0));
        m.extras.slope = Some(Slope { rise: 3.0, run: 4.0 });
        assert!((quantities(&m, Some(&metre_per_point())).unwrap().length_m.unwrap() - 13.75).abs() < 1e-12);
    }

    #[test]
    fn an_area_with_a_cutout() {
        let m = polygon(MarkupKind::Area, &[(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)], &[&[(2.0, 2.0), (2.0, 5.0), (6.0, 5.0), (6.0, 2.0)]]);
        let q = quantities(&m, Some(&metre_per_point())).unwrap();
        assert_eq!(q.area_m2, Some(200.0 - 12.0), "either winding of the cutout");
        assert_eq!(q.perimeter_m, Some(60.0), "the outline only");
        assert_eq!(q.volume_m3, None, "an area isn't a volume");
    }

    #[test]
    fn an_area_at_a_real_scale_and_a_non_uniform_one() {
        // A 1-inch square on paper at 1:500 is 12.7 m square.
        let s = Scale::from_ratio(ScaleId(1), 500.0).unwrap();
        let m = polygon(MarkupKind::Area, &[(0.0, 0.0), (72.0, 0.0), (72.0, 72.0), (0.0, 72.0)], &[]);
        assert!((quantities(&m, Some(&s)).unwrap().area_m2.unwrap() - 161.29).abs() < 1e-9);
        let section = Scale::from_view_axes(ScaleId(2), (72.0, 12.7), (72.0, 1.27), 0, DisplayUnits::METRIC).unwrap();
        let q = quantities(&m, Some(&section)).unwrap();
        assert!((q.area_m2.unwrap() - 16.129).abs() < 1e-9);
        assert!((q.perimeter_m.unwrap() - 27.94).abs() < 1e-9);
    }

    #[test]
    fn a_crossed_outline_or_stray_cutout_gives_no_number() {
        let bow_tie = polygon(MarkupKind::Area, &[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)], &[]);
        assert_eq!(quantities(&bow_tie, Some(&metre_per_point())), Err(QuantityError::SelfIntersecting));
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let outside = polygon(MarkupKind::Area, &square, &[&[(20.0, 20.0), (22.0, 20.0), (22.0, 22.0)]]);
        assert_eq!(quantities(&outside, Some(&metre_per_point())), Err(QuantityError::BadCutout { index: 0 }));
        let crossing = polygon(MarkupKind::Area, &square, &[&[(5.0, 5.0), (15.0, 5.0), (15.0, 8.0)]]);
        assert_eq!(quantities(&crossing, Some(&metre_per_point())), Err(QuantityError::BadCutout { index: 0 }));
        let overlapping = polygon(
            MarkupKind::Area,
            &square,
            &[&[(1.0, 1.0), (5.0, 1.0), (5.0, 5.0), (1.0, 5.0)], &[(2.0, 2.0), (3.0, 2.0), (3.0, 3.0), (2.0, 3.0)]],
        );
        assert_eq!(quantities(&overlapping, Some(&metre_per_point())), Err(QuantityError::BadCutout { index: 1 }), "one inside another");
        let two_points = polygon(MarkupKind::Area, &[(0.0, 0.0), (1.0, 1.0)], &[]);
        assert_eq!(quantities(&two_points, None), Err(QuantityError::TooFewPoints { need: 3, got: 2 }));
    }

    #[test]
    fn a_volume_is_plan_area_times_depth() {
        let mut m = polygon(MarkupKind::Volume, &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], &[]);
        assert_eq!(quantities(&m, Some(&metre_per_point())).unwrap().volume_m3, None, "no depth yet");
        m.extras.depth_m = Some(0.3);
        m.extras.slope = Some(Slope { rise: 3.0, run: 4.0 });
        let q = quantities(&m, Some(&metre_per_point())).unwrap();
        assert!((q.volume_m3.unwrap() - 30.0).abs() < 1e-12);
        assert!((q.area_m2.unwrap() - 125.0).abs() < 1e-12, "the sloped surface");
        m.extras.depth_m = Some(f64::NAN);
        assert_eq!(quantities(&m, Some(&metre_per_point())), Err(QuantityError::NotFinite));
    }

    #[test]
    fn uncalibrated_measurements_have_no_scaled_numbers() {
        let m = polygon(MarkupKind::Area, &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], &[]);
        let q = quantities(&m, None).unwrap();
        assert!(q.uncalibrated && q.area_m2.is_none() && q.perimeter_m.is_none());
        let bad = polygon(MarkupKind::Area, &[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)], &[]);
        assert_eq!(quantities(&bad, None), Err(QuantityError::SelfIntersecting), "a crossed outline is reported even without a scale");
    }

    #[test]
    fn counts_and_angles_work_without_a_scale() {
        let count = Markup::new(0, MarkupKind::Count, Geometry::Points { pts: vec![p(1.0, 1.0); 42] });
        assert_eq!(quantities(&count, None).unwrap(), Quantities { count: Some(42), ..Quantities::default() });
        let angle = Markup::new(0, MarkupKind::Angle, Geometry::Polyline { pts: vec![p(10.0, 0.0), p(0.0, 0.0), p(10.0, 10.0)] });
        assert!((quantities(&angle, None).unwrap().angle_deg.unwrap() - 45.0).abs() < 1e-12);
        // Drawn at 45°, but the vertical scale is exaggerated tenfold, so the
        // real slope is much shallower.
        let section = Scale::from_view_axes(ScaleId(2), (1.0, 10.0), (1.0, 1.0), 0, DisplayUnits::METRIC).unwrap();
        let real = quantities(&angle, Some(&section)).unwrap().angle_deg.unwrap();
        assert!((real - (0.1f64).atan().to_degrees()).abs() < 1e-9, "{real}");
    }

    #[test]
    fn radius_and_diameter_from_a_line_an_arc_or_a_circle() {
        let s = metre_per_point();
        let from_centre = Markup::new(0, MarkupKind::Radius, Geometry::Line { a: p(0.0, 0.0), b: p(3.0, 4.0) });
        assert_eq!(quantities(&from_centre, Some(&s)).unwrap().radius_m, Some(5.0));
        let across = Markup::new(0, MarkupKind::Diameter, Geometry::Line { a: p(0.0, 0.0), b: p(6.0, 8.0) });
        assert_eq!(quantities(&across, Some(&s)).unwrap().diameter_m, Some(10.0), "a diameter is drawn right across, not from the middle");
        let arc = Markup::new(0, MarkupKind::Diameter, Geometry::Polyline { pts: vec![p(5.0, 0.0), p(0.0, 5.0), p(-5.0, 0.0)] });
        assert!((quantities(&arc, Some(&s)).unwrap().diameter_m.unwrap() - 10.0).abs() < 1e-12);
        let straight = Markup::new(0, MarkupKind::Radius, Geometry::Polyline { pts: vec![p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0)] });
        assert_eq!(quantities(&straight, Some(&s)), Err(QuantityError::Collinear));
        let circle = crate::geom::Rect::from_corners(p(0.0, 0.0), p(72.0, 72.0));
        let circle = Markup::new(0, MarkupKind::Diameter, Geometry::Ellipse { rect: circle });
        let at_100 = Scale::from_ratio(ScaleId(3), 100.0).unwrap();
        assert!((quantities(&circle, Some(&at_100)).unwrap().diameter_m.unwrap() - 2.54).abs() < 1e-12);
        let section = Scale::from_view_axes(ScaleId(2), (1.0, 2.0), (1.0, 1.0), 0, DisplayUnits::METRIC).unwrap();
        assert_eq!(quantities(&circle, Some(&section)), Err(QuantityError::NotCircular));
    }

    #[test]
    fn the_wrong_shape_for_a_kind_or_bad_numbers_are_errors() {
        let m = Markup::new(0, MarkupKind::Area, Geometry::Line { a: p(0.0, 0.0), b: p(1.0, 1.0) });
        assert_eq!(quantities(&m, None), Err(QuantityError::WrongGeometry));
        let m = Markup::new(0, MarkupKind::Length, Geometry::Line { a: p(0.0, f64::INFINITY), b: p(1.0, 1.0) });
        assert_eq!(quantities(&m, None), Err(QuantityError::NotFinite));
        let pen = Markup::new(0, MarkupKind::Pen, Geometry::Ink { strokes: vec![vec![p(0.0, 0.0)]] });
        assert_eq!(quantities(&pen, None), Ok(Quantities::default()));
    }

    #[test]
    fn quantities_as_label_text() {
        let q = Quantities { area_m2: Some(1845.394), perimeter_m: Some(172.4), ..Quantities::default() };
        assert_eq!(q.text(MarkupKind::Area, &DisplayUnits::METRIC, Precision::Decimals(2)).as_deref(), Some("1,845.39 m²"));
        assert_eq!(q.text(MarkupKind::Perimeter, &DisplayUnits::METRIC, Precision::Decimals(1)).as_deref(), Some("172.4 m"));
        assert_eq!(q.text(MarkupKind::Volume, &DisplayUnits::METRIC, Precision::Decimals(0)).as_deref(), Some("1,845 m² (no depth)"));
        assert_eq!(q.text(MarkupKind::Pen, &DisplayUnits::METRIC, Precision::Decimals(0)), None);
        assert_eq!(uncalibrated().text(MarkupKind::Length, &DisplayUnits::METRIC, Precision::Decimals(0)).as_deref(), Some("uncalibrated"));
    }

    #[test]
    fn totals_leave_out_what_cannot_be_trusted() {
        let s = metre_per_point();
        let square = polygon(MarkupKind::Area, &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], &[]);
        let bow_tie = polygon(MarkupKind::Area, &[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)], &[]);
        let results = [quantities(&square, Some(&s)), quantities(&square, Some(&s)), quantities(&square, None), quantities(&bow_tie, Some(&s))];
        let totals: Totals = results.iter().collect();
        assert_eq!(totals.area_m2, 200.0);
        assert_eq!((totals.included, totals.excluded), (2, 2));
    }
}
