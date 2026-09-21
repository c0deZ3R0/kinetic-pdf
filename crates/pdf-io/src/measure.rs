//! A scale as a /Measure dictionary (ISO 32000-2 §12.9, rectilinear, /RL),
//! and back.
//!
//! Number formats are written in the scale's display units, so a viewer that
//! only knows the standard shows the same units we do. Feet and inches are a
//! chain of two formats, feet truncated then inches, as the standard
//! describes. What the standard has no place for -- the scale's ID, its
//! volume unit -- goes in a /KPDF dictionary inside it.

use markup_model::units::{AreaUnit, LengthUnit, VolumeUnit};
use markup_model::{DisplayUnits, Precision, Scale, ScaleId};
use pdf_content::lopdf::{dictionary, Dictionary, Document, Object};

use crate::values::{get, name, number, read_name, read_text, real, text};
use crate::Error;

/// A /NumberFormat for `unit`, `factor` of them per unit before it.
fn number_format(unit: &str, factor: f64, precision: Precision, truncate: bool) -> Object {
    let mut format = dictionary! { "Type" => "NumberFormat", "U" => text(unit), "C" => real(factor) };
    match (truncate, precision) {
        (true, _) => format.set("F", name("T")),
        (false, Precision::Decimals(d)) => {
            format.set("F", name("D"));
            format.set("D", Object::Integer(10_i64.pow(u32::from(d.min(9)))));
        }
        (false, Precision::Fraction(n)) => {
            format.set("F", name("F"));
            format.set("D", Object::Integer(i64::from(n.max(1))));
        }
    }
    Object::Dictionary(format)
}

/// The largest unit lengths are counted in: feet for feet and inches.
fn largest(unit: LengthUnit) -> LengthUnit {
    if unit == LengthUnit::FeetInches {
        LengthUnit::Foot
    } else {
        unit
    }
}

/// A number format array for lengths: `factor` of the largest unit per unit
/// before it.
fn length_formats(unit: LengthUnit, factor: f64, precision: Precision) -> Object {
    if unit == LengthUnit::FeetInches {
        Object::Array(vec![number_format("ft", factor, precision, true), number_format("in", 12.0, precision, false)])
    } else {
        Object::Array(vec![number_format(unit.symbol(), factor, precision, false)])
    }
}

pub fn measure_dict(scale: &Scale) -> Dictionary {
    let (units, precision) = (scale.display, scale.precision);
    let unit = largest(units.length);
    let mut d = dictionary! {
        "Type" => "Measure",
        "Subtype" => "RL",
        "R" => text(&scale.label),
        "X" => length_formats(units.length, scale.metres_per_point_x / unit.metres(), precision),
        "D" => length_formats(units.length, 1.0, precision),
        "A" => Object::Array(vec![number_format(units.area.symbol(), unit.metres() * unit.metres() / units.area.square_metres(), precision, false)]),
        "T" => Object::Array(vec![number_format("°", 1.0, precision, false)]),
        "KPDF" => dictionary! { "V" => 1, "NM" => text(&scale.id.to_nm()), "VolumeUnit" => text(units.volume.symbol()) },
    };
    if !scale.is_uniform() {
        d.set("Y", length_formats(units.length, scale.metres_per_point_y / unit.metres(), precision));
        // Both axes are counted in the same unit.
        d.set("CYX", real(1.0));
    }
    d
}

/// The first number format of array `key`, and how many there are.
fn first_format<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<(&'a Dictionary, usize)> {
    let array = get(doc, dict, key)?.as_array().ok()?;
    let first = crate::values::resolve(doc, array.first()?)?.as_dict().ok()?;
    Some((first, array.len()))
}

/// The length unit and metres per point along one axis, from its number
/// format array.
fn axis(doc: &Document, dict: &Dictionary, key: &[u8]) -> Result<(LengthUnit, f64), Error> {
    let (format, count) = first_format(doc, dict, key).ok_or_else(|| Error::Invalid(format!("/Measure has no /{}", String::from_utf8_lossy(key))))?;
    let symbol = read_text(doc, format, b"U").unwrap_or_default();
    let unit = LengthUnit::parse(symbol.trim()).ok_or_else(|| Error::Unsupported(format!("length unit \"{symbol}\"")))?;
    let factor = number(doc, format, b"C").filter(|c| c.is_finite() && *c > 0.0).ok_or_else(|| Error::Invalid("/Measure factor isn't a positive number".into()))?;
    let unit_shown = if unit == LengthUnit::Foot && count >= 2 { LengthUnit::FeetInches } else { unit };
    Ok((unit_shown, factor * unit.metres()))
}

fn precision_of(doc: &Document, format: &Dictionary) -> Precision {
    let denominator = number(doc, format, b"D").unwrap_or(100.0).max(1.0);
    match read_name(doc, format, b"F") {
        Some(b"F") => Precision::Fraction(denominator.min(f64::from(u16::MAX)) as u16),
        _ => Precision::Decimals(denominator.log10().round().clamp(0.0, 9.0) as u8),
    }
}

/// A scale from a /Measure dictionary: its ID from /KPDF when we wrote it,
/// otherwise a new one.
pub fn read_measure(doc: &Document, dict: &Dictionary) -> Result<Scale, Error> {
    if read_name(doc, dict, b"Subtype").is_some_and(|s| s != b"RL") {
        return Err(Error::Unsupported("a geospatial /Measure".into()));
    }
    let (length, mppx) = axis(doc, dict, b"X")?;
    let mppy = match axis(doc, dict, b"Y") {
        Ok((_, per_y_unit)) => per_y_unit * number(doc, dict, b"CYX").unwrap_or(1.0),
        Err(_) => mppx,
    };
    let kpdf = get(doc, dict, b"KPDF").and_then(|o| o.as_dict().ok());
    let id = kpdf.and_then(|k| read_text(doc, k, b"NM")).and_then(|nm| ScaleId::from_nm(&nm)).unwrap_or_default();
    // The precision is the last format's: the inches of feet and inches.
    let precision = get(doc, dict, b"D")
        .or_else(|| get(doc, dict, b"X"))
        .and_then(|o| o.as_array().ok())
        .and_then(|a| a.last())
        .and_then(|o| crate::values::resolve(doc, o)?.as_dict().ok())
        .map_or(Precision::default(), |f| precision_of(doc, f));
    let metric = length.is_metric();
    let area = first_format(doc, dict, b"A")
        .and_then(|(f, _)| read_text(doc, f, b"U"))
        .and_then(|s| AreaUnit::ALL.into_iter().find(|u| u.symbol() == s.trim()))
        .unwrap_or(if metric { AreaUnit::SquareMetre } else { AreaUnit::SquareFoot });
    let volume = kpdf
        .and_then(|k| read_text(doc, k, b"VolumeUnit"))
        .and_then(|s| VolumeUnit::ALL.into_iter().find(|u| u.symbol() == s))
        .unwrap_or(if metric { VolumeUnit::CubicMetre } else { VolumeUnit::CubicYard });
    Ok(Scale {
        id,
        metres_per_point_x: mppx,
        metres_per_point_y: mppy,
        label: read_text(doc, dict, b"R").unwrap_or_default(),
        precision,
        display: DisplayUnits { length, area, volume },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(scale: &Scale) -> Scale {
        let doc = Document::with_version("1.7");
        read_measure(&doc, &measure_dict(scale)).unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-6 * a.abs().max(b.abs())
    }

    #[test]
    fn a_metric_scale_comes_back_the_same() {
        let mut s = Scale::from_ratio(ScaleId::new(), 250.0).unwrap();
        s.precision = Precision::Decimals(3);
        s.display.area = AreaUnit::Hectare;
        let back = round_trip(&s);
        assert_eq!((back.id, &back.label, back.precision, back.display), (s.id, &s.label, s.precision, s.display));
        assert!(close(back.metres_per_point_x, s.metres_per_point_x) && back.is_uniform());
    }

    #[test]
    fn feet_and_inches_are_a_chain_of_two_formats() {
        let mut s = Scale::from_ratio(ScaleId::new(), 48.0).unwrap();
        s.display = DisplayUnits::IMPERIAL;
        s.precision = Precision::Fraction(16);
        let dict = measure_dict(&s);
        let x = dict.get(b"X").unwrap().as_array().unwrap();
        assert_eq!(x.len(), 2);
        let feet = x[0].as_dict().unwrap();
        assert_eq!(feet.get(b"F").unwrap().as_name().unwrap(), b"T");
        // A point at 1:48 is 2/3 inch real, 1/18 foot.
        assert!(close(f64::from(feet.get(b"C").unwrap().as_float().unwrap()), 1.0 / 18.0));
        let back = round_trip(&s);
        assert_eq!((back.display, back.precision), (DisplayUnits::IMPERIAL, Precision::Fraction(16)));
        assert!(close(back.metres_per_point_x, s.metres_per_point_x));
    }

    #[test]
    fn a_non_uniform_scale_writes_y_separately() {
        let s = Scale::from_view_axes(ScaleId::new(), (72.0, 12.7), (72.0, 1.27), 0, DisplayUnits::METRIC).unwrap();
        let dict = measure_dict(&s);
        assert!(dict.has(b"Y") && dict.has(b"CYX"));
        let back = round_trip(&s);
        assert!(close(back.metres_per_point_x, s.metres_per_point_x) && close(back.metres_per_point_y, s.metres_per_point_y));
    }

    #[test]
    fn a_measure_from_elsewhere_gets_a_new_id_and_unknown_units_are_refused() {
        let doc = Document::with_version("1.7");
        let plain = dictionary! {
            "Type" => "Measure", "Subtype" => "RL", "R" => text("1 in = 10 ft"),
            "X" => vec![Object::Dictionary(dictionary! { "U" => text("ft"), "C" => real(10.0 / 72.0) })],
        };
        let s = read_measure(&doc, &plain).unwrap();
        assert!(close(s.metres_per_point_x, 10.0 * 0.3048 / 72.0));
        assert_eq!(s.display.length, LengthUnit::Foot);
        let furlongs = dictionary! { "Subtype" => "RL", "X" => vec![Object::Dictionary(dictionary! { "U" => text("fur"), "C" => real(1.0) })] };
        assert!(matches!(read_measure(&doc, &furlongs), Err(Error::Unsupported(_))));
    }
}
