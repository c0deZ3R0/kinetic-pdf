//! Colour spaces, as far as drawing needs them: turning colour values into RGB.

use std::sync::Arc;

use pdf_content::lopdf::{Document, Object};
use pdf_content::objects::number;

use crate::function::Function;

/// A colour space.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// Colours looked up in a table of `base` colours, one byte a component.
    Indexed { base: Box<Space>, highest: usize, table: Vec<u8> },
    Pattern,
    /// Separation and DeviceN: `inputs` tints, one an ink, drawn as `tint`
    /// turns them into `alternate`, as a screen shows spot inks.
    Inks { inputs: usize, alternate: Box<Space>, tint: Arc<Function> },
    /// Lab and the like.
    Unsupported,
}

impl Space {
    /// How many values make a colour.
    pub fn components(&self) -> usize {
        match self {
            Space::Rgb => 3,
            Space::Cmyk => 4,
            Space::Pattern => 0,
            Space::Inks { inputs, .. } => *inputs,
            _ => 1,
        }
    }

    /// The device space with `components` values a colour, for data that says
    /// only that much, a JPEG's.
    pub fn with_components(components: usize) -> Space {
        match components {
            1 => Space::Gray,
            3 => Space::Rgb,
            4 => Space::Cmyk,
            _ => Space::Unsupported,
        }
    }

    /// The colour a space starts with when it's set: black, the table's
    /// first entry, or every ink at full strength.
    pub fn initial(&self) -> Option<[f32; 3]> {
        match self {
            Space::Cmyk => self.colour(&[0.0, 0.0, 0.0, 1.0]),
            Space::Inks { inputs, .. } => self.colour(&vec![1.0; *inputs]),
            _ => self.colour(&vec![0.0; self.components().max(1)]),
        }
    }

    /// The RGB colour of `values` in this space, each from 0 to 1 -- or for
    /// an indexed space, the index; `None` if it can't be drawn.
    pub fn colour(&self, values: &[f32]) -> Option<[f32; 3]> {
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
                let mut base_values = [0.0; 4];
                for (value, &byte) in base_values.iter_mut().zip(entry) {
                    *value = f32::from(byte) / 255.0;
                }
                base.colour(&base_values[..size])
            }
            Space::Inks { inputs, alternate, tint } => {
                let tints = values.get(..*inputs)?;
                let converted = tint.eval(tints)?;
                if converted.len() < alternate.components() {
                    return None;
                }
                alternate.colour(&converted)
            }
            Space::Pattern | Space::Unsupported => None,
        }
    }
}

/// A colour space written out: a name, or an array like `[/ICCBased stream]`
/// or `[/Indexed base highest table]`.
pub(crate) fn space(doc: &Document, object: &Object) -> Space {
    let Ok((_, object)) = doc.dereference(object) else { return Space::Unsupported };
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
                .and_then(|stream| doc.dereference(stream).ok())
                .and_then(|(_, stream)| stream.as_stream().ok())
                .and_then(|stream| stream.dict.get(b"N").ok().and_then(|n| number(doc, n)));
            components.map_or(Space::Unsupported, |n| Space::with_components(n as usize))
        }
        b"Indexed" | b"I" => {
            let [base, highest, table] = rest else { return Space::Unsupported };
            let table = match doc.dereference(table).map(|(_, t)| t) {
                Ok(Object::String(bytes, _)) => bytes.clone(),
                Ok(Object::Stream(stream)) => stream.get_plain_content().unwrap_or_default(),
                _ => return Space::Unsupported,
            };
            match (space(doc, base), number(doc, highest)) {
                (Space::Indexed { .. } | Space::Pattern | Space::Unsupported, _) | (_, None) => Space::Unsupported,
                (base, Some(highest)) => Space::Indexed { base: Box::new(base), highest: highest as usize, table },
            }
        }
        b"Separation" | b"DeviceN" => {
            // [/Separation name alternate tint] or [/DeviceN names alternate tint ...]
            let [names, alternate, tint, ..] = rest else { return Space::Unsupported };
            let inputs = match (family, doc.dereference(names).map(|(_, n)| n)) {
                (b"Separation", _) => 1,
                (_, Ok(Object::Array(names))) => names.len(),
                _ => return Space::Unsupported,
            };
            let alternate = match space(doc, alternate) {
                Space::Inks { .. } | Space::Indexed { .. } | Space::Pattern | Space::Unsupported => return Space::Unsupported,
                alternate => alternate,
            };
            match Function::read(doc, tint) {
                Some(tint) if tint.inputs() == inputs && inputs > 0 => Space::Inks { inputs, alternate: Box::new(alternate), tint: Arc::new(tint) },
                _ => Space::Unsupported,
            }
        }
        _ => Space::Unsupported,
    }
}
