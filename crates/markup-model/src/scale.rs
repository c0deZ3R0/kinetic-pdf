//! Scales: how many real metres a point on the page stands for, and the ways
//! of setting one -- two points on a known dimension, a printed ratio, or
//! separate horizontal and vertical calibrations -- with the checks that
//! catch a bad calibration before it prices a job.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::geom::Pt;
use crate::id::ScaleId;
use crate::units::{format_length, group_thousands, DisplayUnits, LengthUnit, Precision};

/// Metres in one PDF point on paper: 1/72 inch.
pub const METRES_PER_POINT: f64 = 0.0254 / 72.0;

/// Calibration points closer than this on screen give a poor scale.
pub const MIN_CALIBRATION_PIXELS: f64 = 20.0;

/// A calibration line more than this far off horizontal or vertical is
/// suspect for a single-axis calibration.
pub const MAX_AXIS_SKEW_DEGREES: f64 = 2.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    pub id: ScaleId,
    /// Metres per point along user-space x, as /Measure /X /C with /U m.
    pub metres_per_point_x: f64,
    /// Metres per point along user-space y. The same as x unless the drawing
    /// exaggerates one direction, as long sections do.
    pub metres_per_point_y: f64,
    /// As shown to the user and written to /Measure /R: `1:100`,
    /// `10.58 cm = 100 m`.
    pub label: String,
    pub precision: Precision,
    pub display: DisplayUnits,
}

/// Why a scale couldn't be made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScaleError {
    /// The two calibration points are the same point.
    ZeroDistance,
    /// The real length, or the ratio, isn't more than zero.
    NotPositive,
    NotFinite,
}

impl fmt::Display for ScaleError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ScaleError::ZeroDistance => write!(f, "the two points are the same point"),
            ScaleError::NotPositive => write!(f, "the length must be more than zero"),
            ScaleError::NotFinite => write!(f, "the numbers are out of range"),
        }
    }
}

impl std::error::Error for ScaleError {}

fn positive(v: f64) -> Result<f64, ScaleError> {
    if !v.is_finite() {
        Err(ScaleError::NotFinite)
    } else if v <= 0.0 {
        Err(ScaleError::NotPositive)
    } else {
        Ok(v)
    }
}

impl Scale {
    /// The same number of metres per point both ways.
    pub fn uniform(id: ScaleId, metres_per_point: f64, label: impl Into<String>) -> Result<Scale, ScaleError> {
        let mpp = positive(metres_per_point)?;
        Ok(Scale {
            id,
            metres_per_point_x: mpp,
            metres_per_point_y: mpp,
            label: label.into(),
            precision: Precision::default(),
            display: DisplayUnits::default(),
        })
    }

    /// A drawing printed at its true paper size at 1:`ratio`.
    pub fn from_ratio(id: ScaleId, ratio: f64) -> Result<Scale, ScaleError> {
        let ratio = positive(ratio)?;
        Scale::uniform(id, ratio * METRES_PER_POINT, format!("1:{}", group_thousands(ratio, if ratio.fract() == 0.0 { 0 } else { 2 })))
    }

    /// From two points on a dimension the user knows is `real_metres` long.
    pub fn from_two_points(id: ScaleId, a: Pt, b: Pt, real_metres: f64, display: DisplayUnits) -> Result<Scale, ScaleError> {
        if !a.is_finite() || !b.is_finite() {
            return Err(ScaleError::NotFinite);
        }
        let points = a.dist(b);
        if points == 0.0 {
            return Err(ScaleError::ZeroDistance);
        }
        let real = positive(real_metres)?;
        let mut scale = Scale::uniform(id, real / points, equivalence_label(points, real, display.length))?;
        scale.display = display;
        Ok(scale)
    }

    /// From separate horizontal and vertical calibrations, each a distance
    /// along that axis of the page as the user sees it, in points, and the
    /// real metres it stands for. `rotation` is the page's quarter turns
    /// clockwise: on a page turned 90°, what looks horizontal runs along
    /// user-space y.
    pub fn from_view_axes(
        id: ScaleId,
        horizontal: (f64, f64),
        vertical: (f64, f64),
        rotation: u8,
        display: DisplayUnits,
    ) -> Result<Scale, ScaleError> {
        let per_axis = |(points, metres): (f64, f64)| -> Result<f64, ScaleError> {
            let points = positive(points.abs()).map_err(|_| ScaleError::ZeroDistance)?;
            Ok(positive(metres)? / points)
        };
        let (h, v) = (per_axis(horizontal)?, per_axis(vertical)?);
        let (x, y) = if rotation % 2 == 1 { (v, h) } else { (h, v) };
        let label = format!(
            "H {} V {}",
            equivalence_label(horizontal.0.abs(), horizontal.1, display.length),
            equivalence_label(vertical.0.abs(), vertical.1, display.length)
        );
        Ok(Scale { id, metres_per_point_x: x, metres_per_point_y: y, label, precision: Precision::default(), display })
    }

    /// The metres a point stands for across the page as it is seen, and up
    /// it. On a page turned a quarter turn, what looks horizontal runs along
    /// user-space y, so the two are swapped.
    pub fn view_axes(&self, rotation: u8) -> (f64, f64) {
        match rotation % 2 {
            1 => (self.metres_per_point_y, self.metres_per_point_x),
            _ => (self.metres_per_point_x, self.metres_per_point_y),
        }
    }

    fn set_view_axes(&mut self, rotation: u8, across: f64, up: f64) {
        let (x, y) = if rotation % 2 == 1 { (up, across) } else { (across, up) };
        self.metres_per_point_x = x;
        self.metres_per_point_y = y;
    }

    /// The same scale with its vertical axis set on its own: `points` up the
    /// page as it is displayed standing for `metres`, the horizontal left as
    /// it stands.
    ///
    /// A section or a long section is usually drawn with the vertical
    /// exaggerated -- 1:1000 along and 1:100 up is ordinary -- so heights off
    /// one measure at their own scale. A plan needs none of this: it measures
    /// the same both ways, which is what every scale starts out as.
    pub fn with_vertical(&self, points: f64, metres: f64, rotation: u8) -> Result<Scale, ScaleError> {
        let points = positive(points.abs()).map_err(|_| ScaleError::ZeroDistance)?;
        let up = positive(metres)? / points;
        let (across, _) = self.view_axes(rotation);
        let mut out = self.clone();
        out.set_view_axes(rotation, across, up);
        out.label = format!("H {} V {}", ratio_of(across), ratio_of(up));
        Ok(out)
    }

    /// The same scale measuring the same both ways again, the vertical set
    /// back to the horizontal: a section read as a plan.
    pub fn without_vertical(&self, rotation: u8) -> Scale {
        let (across, _) = self.view_axes(rotation);
        let mut out = self.clone();
        out.set_view_axes(rotation, across, across);
        out.label = ratio_of(across);
        out
    }

    pub fn is_uniform(&self) -> bool {
        relative_eq(self.metres_per_point_x, self.metres_per_point_y, 1e-9)
    }

    /// 1:ratio for a uniform scale on a drawing at true paper size.
    pub fn ratio(&self) -> Option<f64> {
        self.is_uniform().then(|| self.metres_per_point_x / METRES_PER_POINT)
    }

    /// The real vector a user-space vector stands for.
    pub fn to_real(&self, v: Pt) -> Pt {
        v.scaled(self.metres_per_point_x, self.metres_per_point_y)
    }

    /// The real distance between two user-space points, in metres.
    pub fn distance(&self, a: Pt, b: Pt) -> f64 {
        self.to_real(b - a).len()
    }

    /// Whether two scales give the same numbers, for merging identical
    /// /Measure dictionaries on import. Labels and formatting don't count.
    pub fn same_measure(&self, o: &Scale) -> bool {
        relative_eq(self.metres_per_point_x, o.metres_per_point_x, 1e-6)
            && relative_eq(self.metres_per_point_y, o.metres_per_point_y, 1e-6)
    }
}

/// Whether `a` and `b` agree to within `tolerance` of their size. The
/// tolerance for merging allows for /Measure factors stored as 32-bit reals.
fn relative_eq(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance * a.abs().max(b.abs())
}

/// One axis as the ratio it is drawn at: `1:100`, to a hundredth where it
/// isn't a round one. What a two-axis scale is read as, an axis at a time.
fn ratio_of(metres_per_point: f64) -> String {
    let ratio = metres_per_point / METRES_PER_POINT;
    format!("1:{}", group_thousands(ratio, if ratio.fract() < 0.005 { 0 } else { 2 }))
}

/// A calibration as `10.58 cm = 100 m`: the paper length the points span,
/// in centimetres for metric display and inches otherwise, and the real one.
fn equivalence_label(points: f64, real_metres: f64, unit: LengthUnit) -> String {
    let paper = points * METRES_PER_POINT;
    let (paper_unit, real_precision) = if unit.is_metric() {
        (LengthUnit::Centimetre, Precision::Decimals(2))
    } else {
        (LengthUnit::Inch, Precision::Fraction(16))
    };
    let paper = format_length(paper, paper_unit, Precision::Decimals(2));
    format!("{paper} = {}", format_length(real_metres, unit, real_precision))
}

/// Something about a calibration that makes it worth a second look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CalibrationWarning {
    /// The points were this few pixels apart on screen, so a pixel's error is
    /// a large share of the length.
    ShortOnScreen { pixels: f64 },
    /// A single-axis calibration line runs this many degrees off its axis.
    OffAxis { degrees: f64 },
    /// The page isn't a standard sheet, so a printed ratio may not hold: the
    /// PDF may have been scaled to fit when it was made.
    NonStandardSheet,
}

/// Warnings for two calibration points `screen_pixels` apart on screen.
pub fn calibration_warnings(screen_pixels: f64) -> Vec<CalibrationWarning> {
    if screen_pixels < MIN_CALIBRATION_PIXELS {
        vec![CalibrationWarning::ShortOnScreen { pixels: screen_pixels }]
    } else {
        Vec::new()
    }
}

/// How far the line from `a` to `b` runs off the nearest of horizontal and
/// vertical, in degrees, warning past `MAX_AXIS_SKEW_DEGREES`.
pub fn axis_warning(a: Pt, b: Pt) -> Option<CalibrationWarning> {
    let d = b - a;
    let angle = d.y.abs().atan2(d.x.abs()).to_degrees();
    let off = angle.min(90.0 - angle);
    (off > MAX_AXIS_SKEW_DEGREES).then_some(CalibrationWarning::OffAxis { degrees: off })
}

/// A second known dimension measured with a scale, to check it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verification {
    pub measured_metres: f64,
    pub known_metres: f64,
    /// Signed: positive when the scale measures long.
    pub percent_error: f64,
}

pub fn verify(scale: &Scale, a: Pt, b: Pt, known_metres: f64) -> Result<Verification, ScaleError> {
    let known = positive(known_metres)?;
    let measured = scale.distance(a, b);
    Ok(Verification { measured_metres: measured, known_metres: known, percent_error: (measured - known) / known * 100.0 })
}

/// A standard drawing sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sheet {
    A0,
    A1,
    A2,
    A3,
    A4,
    AnsiA,
    AnsiB,
    AnsiC,
    AnsiD,
    AnsiE,
    ArchA,
    ArchB,
    ArchC,
    ArchD,
    ArchE,
    ArchE1,
}

impl Sheet {
    pub const ALL: [Sheet; 16] = [
        Sheet::A0,
        Sheet::A1,
        Sheet::A2,
        Sheet::A3,
        Sheet::A4,
        Sheet::AnsiA,
        Sheet::AnsiB,
        Sheet::AnsiC,
        Sheet::AnsiD,
        Sheet::AnsiE,
        Sheet::ArchA,
        Sheet::ArchB,
        Sheet::ArchC,
        Sheet::ArchD,
        Sheet::ArchE,
        Sheet::ArchE1,
    ];

    /// Short side and long side, in millimetres.
    pub fn millimetres(self) -> (f64, f64) {
        let inches = |a: f64, b: f64| (a * 25.4, b * 25.4);
        match self {
            Sheet::A0 => (841.0, 1189.0),
            Sheet::A1 => (594.0, 841.0),
            Sheet::A2 => (420.0, 594.0),
            Sheet::A3 => (297.0, 420.0),
            Sheet::A4 => (210.0, 297.0),
            Sheet::AnsiA => inches(8.5, 11.0),
            Sheet::AnsiB => inches(11.0, 17.0),
            Sheet::AnsiC => inches(17.0, 22.0),
            Sheet::AnsiD => inches(22.0, 34.0),
            Sheet::AnsiE => inches(34.0, 44.0),
            Sheet::ArchA => inches(9.0, 12.0),
            Sheet::ArchB => inches(12.0, 18.0),
            Sheet::ArchC => inches(18.0, 24.0),
            Sheet::ArchD => inches(24.0, 36.0),
            Sheet::ArchE => inches(36.0, 48.0),
            Sheet::ArchE1 => inches(30.0, 42.0),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Sheet::A0 => "A0",
            Sheet::A1 => "A1",
            Sheet::A2 => "A2",
            Sheet::A3 => "A3",
            Sheet::A4 => "A4",
            Sheet::AnsiA => "ANSI A",
            Sheet::AnsiB => "ANSI B",
            Sheet::AnsiC => "ANSI C",
            Sheet::AnsiD => "ANSI D",
            Sheet::AnsiE => "ANSI E",
            Sheet::ArchA => "ARCH A",
            Sheet::ArchB => "ARCH B",
            Sheet::ArchC => "ARCH C",
            Sheet::ArchD => "ARCH D",
            Sheet::ArchE => "ARCH E",
            Sheet::ArchE1 => "ARCH E1",
        }
    }
}

/// The standard sheet a page `width` by `height` points is, either way up,
/// within 2 mm on each side, which allows for rounding by whatever made it.
pub fn standard_sheet(width: f64, height: f64) -> Option<Sheet> {
    let to_mm = METRES_PER_POINT * 1000.0;
    let (short, long) = (width.min(height) * to_mm, width.max(height) * to_mm);
    Sheet::ALL.into_iter().find(|s| {
        let (s_short, s_long) = s.millimetres();
        (short - s_short).abs() <= 2.0 && (long - s_long).abs() <= 2.0
    })
}

/// A scale from a printed ratio for a page `width` by `height` points, with a
/// warning when the page isn't a standard sheet.
pub fn ratio_for_page(id: ScaleId, ratio: f64, width: f64, height: f64) -> Result<(Scale, Option<CalibrationWarning>), ScaleError> {
    let warning = standard_sheet(width, height).is_none().then_some(CalibrationWarning::NonStandardSheet);
    Ok((Scale::from_ratio(id, ratio)?, warning))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> ScaleId {
        ScaleId(7)
    }

    #[test]
    fn two_points_on_a_known_dimension() {
        // 25 m drawn 283.4646 pt long (10 cm on paper, so 1:250).
        let points = 0.1 / METRES_PER_POINT;
        let s = Scale::from_two_points(id(), Pt::new(100.0, 100.0), Pt::new(100.0 + points, 100.0), 25.0, DisplayUnits::METRIC).unwrap();
        assert!((s.metres_per_point_x - 25.0 / points).abs() < 1e-15);
        assert!((s.ratio().unwrap() - 250.0).abs() < 1e-9);
        assert_eq!(s.label, "10.00 cm = 25.00 m");
        assert!((s.distance(Pt::new(0.0, 0.0), Pt::new(0.0, points * 2.0)) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn a_bad_calibration_is_refused() {
        let p = Pt::new(1.0, 1.0);
        assert_eq!(Scale::from_two_points(id(), p, p, 25.0, DisplayUnits::METRIC), Err(ScaleError::ZeroDistance));
        assert_eq!(Scale::from_two_points(id(), p, Pt::new(2.0, 1.0), 0.0, DisplayUnits::METRIC), Err(ScaleError::NotPositive));
        assert_eq!(Scale::from_two_points(id(), p, Pt::new(f64::NAN, 1.0), 1.0, DisplayUnits::METRIC), Err(ScaleError::NotFinite));
        assert_eq!(Scale::from_ratio(id(), -100.0), Err(ScaleError::NotPositive));
    }

    #[test]
    fn preset_ratios_at_true_paper_size() {
        let s = Scale::from_ratio(id(), 100.0).unwrap();
        assert_eq!(s.label, "1:100");
        // An inch on paper at 1:100 is 2.54 m.
        assert!((s.distance(Pt::new(0.0, 0.0), Pt::new(72.0, 0.0)) - 2.54).abs() < 1e-12);
        let s = Scale::from_ratio(id(), 1250.0).unwrap();
        assert_eq!(s.label, "1:1,250");
    }

    #[test]
    fn standard_sheets_either_way_up() {
        let mm = |v: f64| v / 1000.0 / METRES_PER_POINT;
        assert_eq!(standard_sheet(mm(841.0), mm(594.0)), Some(Sheet::A1));
        assert_eq!(standard_sheet(mm(594.0), mm(841.0)), Some(Sheet::A1));
        assert_eq!(standard_sheet(2448.0, 1584.0), Some(Sheet::AnsiD), "34 x 22 in");
        assert_eq!(standard_sheet(612.0, 792.0), Some(Sheet::AnsiA));
        // A1 shrunk to fit A3 when it was printed to PDF.
        let (_, warning) = ratio_for_page(id(), 100.0, mm(700.0), mm(500.0)).unwrap();
        assert_eq!(warning, Some(CalibrationWarning::NonStandardSheet));
        let (_, warning) = ratio_for_page(id(), 100.0, mm(420.0), mm(297.0)).unwrap();
        assert_eq!(warning, None);
    }

    #[test]
    fn separate_horizontal_and_vertical_scales() {
        // 1:500 along the section, 1:50 up it.
        let h = (72.0, 500.0 * 0.0254);
        let v = (72.0, 50.0 * 0.0254);
        let s = Scale::from_view_axes(id(), h, v, 0, DisplayUnits::METRIC).unwrap();
        assert!(!s.is_uniform() && s.ratio().is_none());
        assert!((s.distance(Pt::new(0.0, 0.0), Pt::new(72.0, 0.0)) - 12.7).abs() < 1e-12);
        assert!((s.distance(Pt::new(0.0, 0.0), Pt::new(0.0, 72.0)) - 1.27).abs() < 1e-12);
        // Turned a quarter, what's horizontal on screen is user-space y.
        let turned = Scale::from_view_axes(id(), h, v, 1, DisplayUnits::METRIC).unwrap();
        assert_eq!(turned.metres_per_point_y, s.metres_per_point_x);
        assert_eq!(turned.metres_per_point_x, s.metres_per_point_y);
    }

    /// The vertical is set on its own, over a scale already in place, and
    /// put back again -- what a section needs and a plan doesn't.
    #[test]
    fn the_vertical_is_set_on_its_own_and_put_back() {
        let plan = Scale::from_ratio(id(), 500.0).unwrap();
        // 72 points up the page standing for 1.27 m is 1:50.
        let section = plan.with_vertical(72.0, 1.27, 0).unwrap();
        assert!(!section.is_uniform(), "the two axes differ now");
        assert_eq!(section.metres_per_point_x, plan.metres_per_point_x, "the horizontal is untouched");
        assert!((section.distance(Pt::new(0.0, 0.0), Pt::new(0.0, 72.0)) - 1.27).abs() < 1e-9);
        assert_eq!(section.label, "H 1:500 V 1:50");

        // Turned a quarter, what looks vertical runs along user-space x.
        let turned = plan.with_vertical(72.0, 1.27, 1).unwrap();
        assert_eq!(turned.metres_per_point_x, section.metres_per_point_y);
        assert_eq!(turned.metres_per_point_y, section.metres_per_point_x);

        let back = section.without_vertical(0);
        assert!(back.is_uniform() && back.ratio().is_some());
        assert_eq!(back.metres_per_point_y, plan.metres_per_point_x);
        assert_eq!(back.label, "1:500");
    }

    /// A vertical that measures nothing is no scale at all.
    #[test]
    fn a_vertical_needs_a_distance_and_a_length() {
        let plan = Scale::from_ratio(id(), 100.0).unwrap();
        assert_eq!(plan.with_vertical(0.0, 5.0, 0), Err(ScaleError::ZeroDistance));
        assert_eq!(plan.with_vertical(50.0, 0.0, 0), Err(ScaleError::NotPositive));
    }

    #[test]
    fn warnings_for_short_and_skewed_calibrations() {
        assert_eq!(calibration_warnings(12.0), vec![CalibrationWarning::ShortOnScreen { pixels: 12.0 }]);
        assert!(calibration_warnings(400.0).is_empty());
        assert_eq!(axis_warning(Pt::new(0.0, 0.0), Pt::new(100.0, 1.0)), None);
        assert!(matches!(axis_warning(Pt::new(0.0, 0.0), Pt::new(100.0, 10.0)), Some(CalibrationWarning::OffAxis { .. })));
        assert_eq!(axis_warning(Pt::new(0.0, 0.0), Pt::new(-1.0, -100.0)), None, "near vertical, either direction");
    }

    #[test]
    fn verifying_against_a_second_dimension() {
        let s = Scale::from_ratio(id(), 100.0).unwrap();
        let v = verify(&s, Pt::new(0.0, 0.0), Pt::new(72.0, 0.0), 2.5).unwrap();
        assert!((v.measured_metres - 2.54).abs() < 1e-12);
        assert!((v.percent_error - 1.6).abs() < 1e-9);
    }

    #[test]
    fn identical_measures_from_different_annotations_merge() {
        let a = Scale::from_ratio(ScaleId(1), 100.0).unwrap();
        let mut b = Scale::from_ratio(ScaleId(2), 100.0).unwrap();
        // As a 32-bit /C factor would give it back.
        b.metres_per_point_x = f64::from(b.metres_per_point_x as f32);
        b.metres_per_point_y = b.metres_per_point_x;
        b.label = "one to one hundred".into();
        assert!(a.same_measure(&b));
        assert!(!a.same_measure(&Scale::from_ratio(ScaleId(3), 101.0).unwrap()));
    }
}
