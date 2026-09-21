//! Small conversions between PDF objects and plain values: numbers, points,
//! text, dates, and kept-as-is values.

use std::collections::BTreeMap;

use markup_model::markup::RawValue;
use markup_model::Pt;
use pdf_content::lopdf::{self, Dictionary, Document, Object, StringFormat};

/// A number as the file stores it: a 32-bit real, the only kind lopdf writes.
pub fn real(v: f64) -> Object {
    Object::Real(v as f32)
}

pub fn reals(values: impl IntoIterator<Item = f64>) -> Object {
    Object::Array(values.into_iter().map(real).collect())
}

/// Points as a flat array, `[x1 y1 x2 y2 ...]`, as /Vertices holds them.
pub fn points(pts: &[Pt]) -> Object {
    reals(pts.iter().flat_map(|p| [p.x, p.y]))
}

pub fn text(s: &str) -> Object {
    lopdf::text_string(s)
}

pub fn name(s: &str) -> Object {
    Object::Name(s.as_bytes().to_vec())
}

/// `obj`, following it if it's a reference.
pub fn resolve<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Object> {
    doc.dereference(obj).ok().map(|(_, o)| o)
}

pub fn get<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    resolve(doc, dict.get(key).ok()?)
}

pub fn number(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<f64> {
    number_of(doc, dict.get(key).ok()?)
}

pub fn number_of(doc: &Document, obj: &Object) -> Option<f64> {
    match resolve(doc, obj)? {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)),
        _ => None,
    }
}

pub fn numbers(doc: &Document, obj: &Object) -> Option<Vec<f64>> {
    resolve(doc, obj)?.as_array().ok()?.iter().map(|o| number_of(doc, o)).collect()
}

/// A flat array of numbers as points. `None` if any isn't a number; a
/// trailing odd number is ignored.
pub fn read_points(doc: &Document, obj: &Object) -> Option<Vec<Pt>> {
    Some(numbers(doc, obj)?.as_chunks::<2>().0.iter().map(|&[x, y]| Pt::new(x, y)).collect())
}

pub fn read_text(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<String> {
    lopdf::decode_text_string(get(doc, dict, key)?).ok()
}

pub fn read_name<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a [u8]> {
    get(doc, dict, key)?.as_name().ok()
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// A PDF date, `D:YYYYMMDDHHmmSSZ`, in UTC.
pub fn pdf_date(ms: i64) -> Object {
    let secs = ms.div_euclid(1000);
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    Object::String(format!("D:{y:04}{mo:02}{d:02}{:02}{:02}{:02}Z", s / 3600, s / 60 % 60, s % 60).into_bytes(), StringFormat::Literal)
}

/// Milliseconds since 1970 UTC from a PDF date. Missing trailing parts
/// default as ISO 32000 says; a time zone offset is applied.
pub fn parse_pdf_date(s: &str) -> Option<i64> {
    let s = s.strip_prefix("D:").unwrap_or(s);
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |from: usize, len: usize, default: i64| digits.get(from..from + len).map_or(Some(default), |p| p.parse().ok());
    let (y, mo, d) = (part(0, 4, 0)?, part(4, 2, 1)?, part(6, 2, 1)?);
    let (h, mi, sec) = (part(8, 2, 0)?, part(10, 2, 0)?, part(12, 2, 0)?);
    let rest = &s[digits.len()..];
    let offset_minutes = match rest.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let nums: Vec<i64> = rest[1..].split('\'').filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
            let minutes = nums.first().copied().unwrap_or(0) * 60 + nums.get(1).copied().unwrap_or(0);
            if sign == '+' {
                minutes
            } else {
                -minutes
            }
        }
        _ => 0,
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset_minutes * 60;
    Some(secs * 1000)
}

/// A PDF object as a plain value, to keep a key nothing here models.
pub fn to_raw(obj: &Object) -> RawValue {
    match obj {
        Object::Null => RawValue::Null,
        Object::Boolean(b) => RawValue::Bool(*b),
        Object::Integer(i) => RawValue::Integer(*i),
        Object::Real(r) => RawValue::Real(f64::from(*r)),
        Object::Name(n) => RawValue::Name(n.clone()),
        Object::String(s, _) => RawValue::String(s.clone()),
        Object::Array(a) => RawValue::Array(a.iter().map(to_raw).collect()),
        Object::Dictionary(d) => RawValue::Dictionary(d.iter().map(|(k, v)| (k.clone(), to_raw(v))).collect()),
        // A stream's data can't be kept as a plain value; keep its dictionary.
        Object::Stream(s) => RawValue::Dictionary(s.dict.iter().map(|(k, v)| (k.clone(), to_raw(v))).collect()),
        Object::Reference((n, g)) => RawValue::Reference(*n, *g),
    }
}

pub fn from_raw(value: &RawValue) -> Object {
    match value {
        RawValue::Null => Object::Null,
        RawValue::Bool(b) => Object::Boolean(*b),
        RawValue::Integer(i) => Object::Integer(*i),
        RawValue::Real(r) => real(*r),
        RawValue::Name(n) => Object::Name(n.clone()),
        RawValue::String(s) => Object::String(s.clone(), StringFormat::Literal),
        RawValue::Array(a) => Object::Array(a.iter().map(from_raw).collect()),
        RawValue::Dictionary(d) => Object::Dictionary(d.iter().map(|(k, v)| (k.clone(), from_raw(v))).collect::<Dictionary>()),
        RawValue::Reference(n, g) => Object::Reference((*n, *g)),
    }
}

/// The entries of `dict` whose keys aren't in `known`, kept as plain values.
pub fn unknown_entries(dict: &Dictionary, known: &[&[u8]]) -> BTreeMap<Vec<u8>, RawValue> {
    dict.iter().filter(|(k, _)| !known.contains(&k.as_slice())).map(|(k, v)| (k.clone(), to_raw(v))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_to_the_file_and_back() {
        // 2026-09-17 14:05:09 UTC.
        let ms = 1_789_653_909_000;
        let Object::String(bytes, _) = pdf_date(ms) else { panic!() };
        assert_eq!(String::from_utf8(bytes.clone()).unwrap(), "D:20260917140509Z");
        assert_eq!(parse_pdf_date("D:20260917140509Z"), Some(ms));
        assert_eq!(parse_pdf_date("D:20260918000509+10'00'"), Some(ms), "an offset east of UTC");
        assert_eq!(parse_pdf_date("D:2026"), Some(days_from_civil(2026, 1, 1) * 86_400_000));
        assert_eq!(parse_pdf_date("yesterday"), None);
        assert_eq!(civil_from_days(days_from_civil(1999, 12, 31)), (1999, 12, 31));
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
    }

    #[test]
    fn kept_values_come_back_as_they_were() {
        let dict: Dictionary = [
            (b"A".to_vec(), Object::Array(vec![Object::Integer(1), Object::Name(b"X".to_vec())])),
            (b"B".to_vec(), Object::Reference((12, 0))),
        ]
        .into_iter()
        .collect();
        let raw = to_raw(&Object::Dictionary(dict.clone()));
        assert_eq!(from_raw(&raw), Object::Dictionary(dict));
    }
}
