//! Real-world units: converting, formatting and reading what the user types.
//!
//! Quantities are always held in metres, square metres and cubic metres.
//! Other units exist only at the edges: turning a quantity into text for
//! display, and turning typed text like `25 m` or `12' 6 1/2"` into metres.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LengthUnit {
    Millimetre,
    Centimetre,
    Metre,
    Kilometre,
    Inch,
    Foot,
    Yard,
    Mile,
    /// Feet and inches together, as `12' 6 1/2"`.
    FeetInches,
}

impl LengthUnit {
    pub const ALL: [LengthUnit; 9] = [
        LengthUnit::Millimetre,
        LengthUnit::Centimetre,
        LengthUnit::Metre,
        LengthUnit::Kilometre,
        LengthUnit::Inch,
        LengthUnit::Foot,
        LengthUnit::Yard,
        LengthUnit::Mile,
        LengthUnit::FeetInches,
    ];

    /// Metres in one of the unit. Feet and inches count in feet.
    pub fn metres(self) -> f64 {
        match self {
            LengthUnit::Millimetre => 0.001,
            LengthUnit::Centimetre => 0.01,
            LengthUnit::Metre => 1.0,
            LengthUnit::Kilometre => 1000.0,
            LengthUnit::Inch => 0.0254,
            LengthUnit::Foot | LengthUnit::FeetInches => 0.3048,
            LengthUnit::Yard => 0.9144,
            LengthUnit::Mile => 1609.344,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            LengthUnit::Millimetre => "mm",
            LengthUnit::Centimetre => "cm",
            LengthUnit::Metre => "m",
            LengthUnit::Kilometre => "km",
            LengthUnit::Inch => "in",
            LengthUnit::Foot => "ft",
            LengthUnit::Yard => "yd",
            LengthUnit::Mile => "mi",
            LengthUnit::FeetInches => "ft-in",
        }
    }

    pub fn is_metric(self) -> bool {
        matches!(self, LengthUnit::Millimetre | LengthUnit::Centimetre | LengthUnit::Metre | LengthUnit::Kilometre)
    }

    /// The unit a typed name or symbol stands for, in any case.
    pub fn parse(name: &str) -> Option<LengthUnit> {
        Some(match name.to_ascii_lowercase().as_str() {
            "mm" | "millimetre" | "millimetres" | "millimeter" | "millimeters" => LengthUnit::Millimetre,
            "cm" | "centimetre" | "centimetres" | "centimeter" | "centimeters" => LengthUnit::Centimetre,
            "m" | "metre" | "metres" | "meter" | "meters" => LengthUnit::Metre,
            "km" | "kilometre" | "kilometres" | "kilometer" | "kilometers" => LengthUnit::Kilometre,
            "in" | "inch" | "inches" | "\"" => LengthUnit::Inch,
            "ft" | "foot" | "feet" | "'" => LengthUnit::Foot,
            "yd" | "yard" | "yards" => LengthUnit::Yard,
            "mi" | "mile" | "miles" => LengthUnit::Mile,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AreaUnit {
    SquareMillimetre,
    SquareMetre,
    Hectare,
    SquareKilometre,
    SquareInch,
    SquareFoot,
    SquareYard,
    Acre,
}

impl AreaUnit {
    pub const ALL: [AreaUnit; 8] = [
        AreaUnit::SquareMillimetre,
        AreaUnit::SquareMetre,
        AreaUnit::Hectare,
        AreaUnit::SquareKilometre,
        AreaUnit::SquareInch,
        AreaUnit::SquareFoot,
        AreaUnit::SquareYard,
        AreaUnit::Acre,
    ];

    /// Square metres in one of the unit.
    pub fn square_metres(self) -> f64 {
        match self {
            AreaUnit::SquareMillimetre => 1e-6,
            AreaUnit::SquareMetre => 1.0,
            AreaUnit::Hectare => 1e4,
            AreaUnit::SquareKilometre => 1e6,
            AreaUnit::SquareInch => 0.0254 * 0.0254,
            AreaUnit::SquareFoot => 0.3048 * 0.3048,
            AreaUnit::SquareYard => 0.9144 * 0.9144,
            AreaUnit::Acre => 4_046.856_422_4,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            AreaUnit::SquareMillimetre => "mm²",
            AreaUnit::SquareMetre => "m²",
            AreaUnit::Hectare => "ha",
            AreaUnit::SquareKilometre => "km²",
            AreaUnit::SquareInch => "sq in",
            AreaUnit::SquareFoot => "sq ft",
            AreaUnit::SquareYard => "sq yd",
            AreaUnit::Acre => "ac",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VolumeUnit {
    CubicMetre,
    Litre,
    CubicFoot,
    CubicYard,
}

impl VolumeUnit {
    pub const ALL: [VolumeUnit; 4] = [VolumeUnit::CubicMetre, VolumeUnit::Litre, VolumeUnit::CubicFoot, VolumeUnit::CubicYard];

    /// Cubic metres in one of the unit.
    pub fn cubic_metres(self) -> f64 {
        match self {
            VolumeUnit::CubicMetre => 1.0,
            VolumeUnit::Litre => 0.001,
            VolumeUnit::CubicFoot => 0.3048 * 0.3048 * 0.3048,
            VolumeUnit::CubicYard => 0.9144 * 0.9144 * 0.9144,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            VolumeUnit::CubicMetre => "m³",
            VolumeUnit::Litre => "L",
            VolumeUnit::CubicFoot => "cu ft",
            VolumeUnit::CubicYard => "cu yd",
        }
    }
}

/// How finely a quantity is shown. Rounding only ever happens here, when
/// formatting; stored quantities keep every digit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Precision {
    /// This many decimal places.
    Decimals(u8),
    /// To the nearest 1/n, as a whole number and a reduced fraction:
    /// `6 1/2`. Only lengths use it; areas and volumes fall back to two
    /// decimal places.
    Fraction(u16),
}

impl Default for Precision {
    fn default() -> Self {
        Precision::Decimals(2)
    }
}

/// The units each kind of quantity is shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DisplayUnits {
    pub length: LengthUnit,
    pub area: AreaUnit,
    pub volume: VolumeUnit,
}

impl DisplayUnits {
    pub const METRIC: DisplayUnits = DisplayUnits { length: LengthUnit::Metre, area: AreaUnit::SquareMetre, volume: VolumeUnit::CubicMetre };
    pub const IMPERIAL: DisplayUnits =
        DisplayUnits { length: LengthUnit::FeetInches, area: AreaUnit::SquareFoot, volume: VolumeUnit::CubicYard };
}

impl Default for DisplayUnits {
    fn default() -> Self {
        DisplayUnits::METRIC
    }
}

/// `value` to `decimals` places with thousands separated by commas, and never
/// a minus sign on something that rounds to zero.
pub fn group_thousands(value: f64, decimals: usize) -> String {
    let fixed = format!("{:.*}", decimals, value.abs());
    let (whole, fraction) = fixed.split_once('.').map_or((fixed.as_str(), None), |(w, f)| (w, Some(f)));
    let mut grouped = String::with_capacity(fixed.len() + whole.len() / 3 + 1);
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if let Some(fraction) = fraction {
        grouped.push('.');
        grouped.push_str(fraction);
    }
    let is_zero = fixed.bytes().all(|b| b == b'0' || b == b'.');
    if value.is_sign_negative() && !is_zero {
        grouped.insert(0, '-');
    }
    grouped
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// A non-negative `value` to the nearest 1/`denominator`, as `6`, `1/2` or
/// `6 1/2`.
fn mixed_number(value: f64, denominator: u16) -> String {
    let denominator = u64::from(denominator.max(1));
    let parts = (value * denominator as f64).round() as u64;
    let (whole, numerator) = (parts / denominator, parts % denominator);
    if numerator == 0 {
        return group_thousands(whole as f64, 0);
    }
    let common = gcd(numerator, denominator);
    let fraction = format!("{}/{}", numerator / common, denominator / common);
    if whole == 0 {
        fraction
    } else {
        format!("{} {fraction}", group_thousands(whole as f64, 0))
    }
}

/// A length in metres as text in `unit`: `12.35 m`, `3 1/4 in`, `12' 6 1/2"`.
pub fn format_length(metres: f64, unit: LengthUnit, precision: Precision) -> String {
    if !metres.is_finite() {
        return "—".to_owned();
    }
    let sign = if metres < 0.0 { "-" } else { "" };
    if unit == LengthUnit::FeetInches {
        let inches = metres.abs() / 0.0254;
        // Round the inches first so 11.999" carries into the feet.
        let (step, decimals) = match precision {
            Precision::Decimals(d) => (10f64.powi(-i32::from(d)), usize::from(d)),
            Precision::Fraction(n) => (1.0 / f64::from(n.max(1)), 0),
        };
        let inches = (inches / step).round() * step;
        let feet = (inches / 12.0 + 1e-9).floor();
        let rest = (inches - feet * 12.0).max(0.0);
        let rest = match precision {
            Precision::Decimals(_) => format!("{rest:.decimals$}"),
            Precision::Fraction(n) => mixed_number(rest, n),
        };
        let sign = if feet == 0.0 && rest.trim_start_matches(['0', '.']).is_empty() { "" } else { sign };
        return format!("{sign}{}' {rest}\"", group_thousands(feet, 0));
    }
    let value = metres / unit.metres();
    let number = match precision {
        Precision::Decimals(d) => group_thousands(value, usize::from(d)),
        Precision::Fraction(n) => {
            let text = mixed_number(value.abs(), n);
            if text == "0" {
                text
            } else {
                format!("{sign}{text}")
            }
        }
    };
    format!("{number} {}", unit.symbol())
}

/// Decimal places for a quantity with no sensible fraction: areas and volumes.
fn decimals_of(precision: Precision) -> usize {
    match precision {
        Precision::Decimals(d) => usize::from(d),
        Precision::Fraction(_) => 2,
    }
}

pub fn format_area(square_metres: f64, unit: AreaUnit, precision: Precision) -> String {
    format!("{} {}", group_thousands(square_metres / unit.square_metres(), decimals_of(precision)), unit.symbol())
}

pub fn format_volume(cubic_metres: f64, unit: VolumeUnit, precision: Precision) -> String {
    format!("{} {}", group_thousands(cubic_metres / unit.cubic_metres(), decimals_of(precision)), unit.symbol())
}

pub fn format_angle(degrees: f64, precision: Precision) -> String {
    format!("{}°", group_thousands(degrees, decimals_of(precision)))
}

/// What was wrong with typed text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    /// No number where one was expected.
    NotANumber(String),
    UnknownUnit(String),
    /// A unit was needed and none was given or assumed.
    MissingUnit,
    /// Zero or negative where only a positive value makes sense.
    NotPositive,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "nothing was entered"),
            ParseError::NotANumber(s) => write!(f, "\"{s}\" isn't a number"),
            ParseError::UnknownUnit(s) => write!(f, "\"{s}\" isn't a unit this knows"),
            ParseError::MissingUnit => write!(f, "give a unit, such as m or ft"),
            ParseError::NotPositive => write!(f, "it must be more than zero"),
        }
    }
}

impl std::error::Error for ParseError {}

/// A number as typed: `12`, `12.5`, `1/2` or `6 1/2`.
fn parse_mixed(text: &str) -> Result<f64, ParseError> {
    let simple = |s: &str| -> Result<f64, ParseError> {
        let value = match s.split_once('/') {
            Some((n, d)) => {
                let (n, d) = (n.trim().parse::<f64>(), d.trim().parse::<f64>());
                match (n, d) {
                    (Ok(n), Ok(d)) if d != 0.0 => n / d,
                    _ => return Err(ParseError::NotANumber(s.to_owned())),
                }
            }
            None => s.parse::<f64>().map_err(|_| ParseError::NotANumber(s.to_owned()))?,
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(ParseError::NotANumber(s.to_owned()))
        }
    };
    let words: Vec<&str> = text.split_whitespace().collect();
    match words[..] {
        [] => Err(ParseError::Empty),
        [one] => simple(one),
        // "6 1/2", and "6-1/2" as it's often written, below.
        [whole, fraction] if fraction.contains('/') && !whole.contains('/') => Ok(simple(whole)? + simple(fraction)?),
        _ => Err(ParseError::NotANumber(text.to_owned())),
    }
}

/// A length typed by the user, in metres: `25 m`, `2,500mm`, `82.5 ft`,
/// `12'`, `12' 6"`, `12'-6 1/2"`, `6 1/2"`. A bare number is taken to be in
/// `assumed`, or refused when that's `None`.
pub fn parse_length(text: &str, assumed: Option<LengthUnit>) -> Result<f64, ParseError> {
    let text = text.trim().replace(',', "").replace(['′', '’'], "'").replace(['″', '”'], "\"");
    if text.is_empty() {
        return Err(ParseError::Empty);
    }
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.trim_start().to_owned()),
        None => (false, text),
    };
    let metres = if let Some((feet, rest)) = text.split_once('\'') {
        let feet = if feet.trim().is_empty() { 0.0 } else { parse_mixed(feet.trim())? };
        let rest = rest.trim().trim_start_matches('-').trim();
        let rest = rest.strip_suffix('"').or_else(|| rest.strip_suffix("in")).unwrap_or(rest).trim();
        let inches = if rest.is_empty() { 0.0 } else { parse_mixed(&rest.replacen('-', " ", 1))? };
        feet * 0.3048 + inches * 0.0254
    } else {
        let split = text.find(|c: char| c.is_alphabetic() || c == '"').unwrap_or(text.len());
        let (number, unit) = (text[..split].trim(), text[split..].trim());
        let unit = if unit.is_empty() {
            assumed.ok_or(ParseError::MissingUnit)?
        } else {
            LengthUnit::parse(unit).ok_or_else(|| ParseError::UnknownUnit(unit.to_owned()))?
        };
        parse_mixed(&number.replacen('-', " ", 1))? * unit.metres()
    };
    Ok(if negative { -metres } else { metres })
}

/// A drawing scale typed as a ratio, `1:100` or `1 : 250`, as the real
/// distance one unit on paper stands for: 100 or 250.
pub fn parse_ratio(text: &str) -> Result<f64, ParseError> {
    let text = text.trim().replace(',', "");
    let (paper, real) = text.split_once(':').ok_or_else(|| ParseError::NotANumber(text.clone()))?;
    let (paper, real) = (parse_mixed(paper.trim())?, parse_mixed(real.trim())?);
    if paper <= 0.0 || real <= 0.0 {
        return Err(ParseError::NotPositive);
    }
    Ok(real / paper)
}

/// A drawing scale written as two lengths, paper first: `1/4" = 1'-0"` or
/// `10.58 cm = 100 m`. Gives the paper and real lengths in metres. A bare
/// number on either side is taken to be in the other side's unit.
pub fn parse_equivalence(text: &str) -> Result<(f64, f64), ParseError> {
    let (paper, real) = text.split_once('=').ok_or_else(|| ParseError::NotANumber(text.trim().to_owned()))?;
    let (paper, real) = (paper.trim(), real.trim());
    let paper_m = parse_length(paper, None);
    let real_m = parse_length(real, None);
    let (paper_m, real_m) = match (paper_m, real_m) {
        (Ok(p), Ok(r)) => (p, r),
        (Err(ParseError::MissingUnit), Ok(r)) => (parse_length(paper, Some(guess_unit(real)?))?, r),
        (Ok(p), Err(ParseError::MissingUnit)) => (p, parse_length(real, Some(guess_unit(paper)?))?),
        (Err(e), _) | (_, Err(e)) => return Err(e),
    };
    if paper_m <= 0.0 || real_m <= 0.0 {
        return Err(ParseError::NotPositive);
    }
    Ok((paper_m, real_m))
}

/// The unit written in `text`, for a bare number beside it.
fn guess_unit(text: &str) -> Result<LengthUnit, ParseError> {
    if text.contains('\'') {
        return Ok(LengthUnit::Foot);
    }
    let start = text.find(|c: char| c.is_alphabetic() || c == '"').ok_or(ParseError::MissingUnit)?;
    let unit = text[start..].trim();
    LengthUnit::parse(unit).ok_or_else(|| ParseError::UnknownUnit(unit.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn lengths_typed_in_any_common_form() {
        let m = |s| parse_length(s, None).unwrap();
        assert!(close(m("25 m"), 25.0));
        assert!(close(m("25m"), 25.0));
        assert!(close(m("2,500 mm"), 2.5));
        assert!(close(m("12.5 ft"), 3.81));
        assert!(close(m("12'"), 3.6576));
        assert!(close(m("12' 6\""), 3.81));
        assert!(close(m("12'-6 1/2\""), 12.0 * 0.3048 + 6.5 * 0.0254));
        assert!(close(m("12'6-1/2\""), 12.0 * 0.3048 + 6.5 * 0.0254));
        assert!(close(m("6 1/2\""), 6.5 * 0.0254));
        assert!(close(m("1/2 in"), 0.0127));
        assert!(close(m("3 Metres"), 3.0));
        assert!(close(parse_length("40", Some(LengthUnit::Metre)).unwrap(), 40.0));
    }

    #[test]
    fn a_length_without_a_unit_or_a_number_is_refused() {
        assert_eq!(parse_length("40", None), Err(ParseError::MissingUnit));
        assert_eq!(parse_length("", None), Err(ParseError::Empty));
        assert_eq!(parse_length("40 furlongs", None), Err(ParseError::UnknownUnit("furlongs".into())));
        assert!(matches!(parse_length("1.2.3 m", None), Err(ParseError::NotANumber(_))));
        assert!(matches!(parse_length("1/0 m", None), Err(ParseError::NotANumber(_))));
    }

    #[test]
    fn scales_as_ratios_and_as_equivalences() {
        assert_eq!(parse_ratio("1:100").unwrap(), 100.0);
        assert_eq!(parse_ratio(" 1 : 2,500 ").unwrap(), 2500.0);
        assert_eq!(parse_ratio("1:0"), Err(ParseError::NotPositive));
        let (paper, real) = parse_equivalence("1/4\" = 1'-0\"").unwrap();
        assert!(close(real / paper, 48.0), "quarter inch to the foot is 1:48");
        let (paper, real) = parse_equivalence("10.58 cm = 100 m").unwrap();
        assert!(close(paper, 0.1058) && close(real, 100.0));
        let (paper, real) = parse_equivalence("1 = 200 mm").unwrap();
        assert!(close(real / paper, 200.0));
    }

    #[test]
    fn lengths_shown_in_metric_units() {
        assert_eq!(format_length(1845.3949, LengthUnit::Metre, Precision::Decimals(2)), "1,845.39 m");
        assert_eq!(format_length(0.0254, LengthUnit::Millimetre, Precision::Decimals(1)), "25.4 mm");
        assert_eq!(format_length(-0.001, LengthUnit::Metre, Precision::Decimals(2)), "0.00 m", "no minus zero");
        assert_eq!(format_length(-12.5, LengthUnit::Metre, Precision::Decimals(1)), "-12.5 m");
        assert_eq!(format_length(1234567.0, LengthUnit::Metre, Precision::Decimals(0)), "1,234,567 m");
    }

    #[test]
    fn lengths_shown_in_feet_and_inches() {
        let ft_in = |m, p| format_length(m, LengthUnit::FeetInches, p);
        let m = |ft: f64, inches: f64| ft * 0.3048 + inches * 0.0254;
        assert_eq!(ft_in(m(12.0, 6.5), Precision::Fraction(16)), "12' 6 1/2\"");
        assert_eq!(ft_in(m(12.0, 0.0), Precision::Fraction(16)), "12' 0\"");
        assert_eq!(ft_in(m(0.0, 3.25), Precision::Fraction(8)), "0' 3 1/4\"");
        assert_eq!(ft_in(m(4.0, 11.999), Precision::Fraction(16)), "5' 0\"", "carries into the feet");
        assert_eq!(ft_in(m(4.0, 11.96), Precision::Decimals(1)), "5' 0.0\"");
        assert_eq!(ft_in(-m(2.0, 1.0), Precision::Fraction(2)), "-2' 1\"");
        assert_eq!(format_length(m(0.0, 3.25), LengthUnit::Inch, Precision::Fraction(4)), "3 1/4 in");
    }

    #[test]
    fn areas_volumes_and_angles() {
        assert_eq!(format_area(15_000.0, AreaUnit::Hectare, Precision::Decimals(3)), "1.500 ha");
        assert_eq!(format_area(1.0, AreaUnit::SquareFoot, Precision::Fraction(8)), "10.76 sq ft");
        assert_eq!(format_volume(0.7645548579840001, VolumeUnit::CubicYard, Precision::Decimals(2)), "1.00 cu yd");
        assert_eq!(format_angle(89.996, Precision::Decimals(1)), "90.0°");
    }

    #[test]
    fn what_is_shown_reads_back_as_the_same_length() {
        for unit in LengthUnit::ALL {
            let shown = format_length(3.81, unit, Precision::Decimals(6));
            let back = parse_length(&shown, None).unwrap_or_else(|e| panic!("{shown}: {e}"));
            assert!((back - 3.81).abs() < 1e-3 * unit.metres().max(0.001), "{unit:?}: {shown} read as {back}");
        }
    }
}
