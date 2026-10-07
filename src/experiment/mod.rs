//! Deliberately versioned markup placement experiments, separate from stable commands.
use crate::{
    control::{Error, ErrorCode},
    domain::{PageGeometry, PdfBox, TextChar},
};
#[cfg(feature = "mcp")]
use rmcp::schemars;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Preview {
        version: u32,
        page: u32,
        kind: String,
        anchor: Anchor,
        saved_tool: Option<SavedTool>,
        comment: String,
    },
    Commit {
        preview: String,
    },
    Discard,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SavedTool {
    pub name: String,
    #[serde(default)]
    pub group: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[serde(tag = "space", rename_all = "snake_case", deny_unknown_fields)]
pub enum Anchor {
    PdfPoints {
        points: Vec<[f32; 2]>,
    },
    /// Top-left origin in the displayed crop box, including its current rotation.
    PageFractions {
        points: Vec<[f32; 2]>,
    },
    /// Exact, case-sensitive quote in selectable text. Occurrences are one-based.
    Text {
        quote: String,
        occurrence: u32,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Preview {
    pub id: String,
    pub page: u32,
    pub kind: String,
    pub points: Vec<[f32; 2]>,
    pub quads: Vec<crate::control::PdfRect>,
    pub settings: serde_json::Value,
}

pub struct Placement {
    pub points: Vec<[f32; 2]>,
    pub quads: Vec<PdfBox>,
}

pub fn resolve(
    anchor: &Anchor,
    geometry: PageGeometry,
    chars: Option<&[TextChar]>,
) -> Result<Placement, Error> {
    let invalid = |s| Error::new(ErrorCode::InvalidParameters, s);
    let (points, quads) = match anchor {
        Anchor::PdfPoints { points } => (points.clone(), Vec::new()),
        Anchor::PageFractions { points } => {
            if points
                .iter()
                .flatten()
                .any(|n| !n.is_finite() || !(0.0..=1.0).contains(n))
            {
                return Err(invalid("Page fractions must be between 0 and 1"));
            }
            (
                points
                    .iter()
                    .map(|&[x, y]| {
                        let (x, y) = geometry.from_view(x, y);
                        [x, y]
                    })
                    .collect(),
                Vec::new(),
            )
        }
        Anchor::Text { quote, occurrence } => {
            if quote.is_empty() || quote.len() > 4096 || *occurrence == 0 {
                return Err(invalid(
                    "Text anchor needs a quote and one-based occurrence",
                ));
            }
            let chars = chars.ok_or_else(|| {
                Error::new(
                    ErrorCode::Busy,
                    "Read this page's text before previewing a text anchor",
                )
            })?;
            let needle: Vec<char> = quote.chars().collect();
            let Some(start) = chars
                .windows(needle.len())
                .enumerate()
                .filter(|(_, run)| run.iter().map(|c| c.ch).eq(needle.iter().copied()))
                .nth(*occurrence as usize - 1)
                .map(|(at, _)| at)
            else {
                return Err(Error::new(
                    ErrorCode::NotFound,
                    "Text anchor occurrence not found",
                ));
            };
            let bands = crate::selection::bands(chars, start..start + needle.len());
            if bands.is_empty() {
                return Err(Error::new(
                    ErrorCode::Unsupported,
                    "Matched text has no selectable glyph bounds",
                ));
            }
            (Vec::new(), bands)
        }
    };
    if points.len() > 4096
        || points
            .iter()
            .any(|&[x, y]| !x.is_finite() || !y.is_finite() || !geometry.bounds.contains(x, y))
        || quads.iter().any(|b| {
            !geometry.bounds.contains(b.left, b.bottom) || !geometry.bounds.contains(b.right, b.top)
        })
    {
        return Err(invalid(
            "Placement lies outside the visible page or has too many points",
        ));
    }
    Ok(Placement { points, quads })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractions_roundtrip_through_all_rotations_and_offset_crop_boxes() {
        for rotation in 0..4 {
            let geometry = PageGeometry {
                rotation,
                bounds: PdfBox {
                    left: 30.0,
                    bottom: 50.0,
                    right: 430.0,
                    top: 650.0,
                },
            };
            let resolved = resolve(
                &Anchor::PageFractions {
                    points: vec![[0.2, 0.3], [0.8, 0.9]],
                },
                geometry,
                None,
            )
            .unwrap();
            for (pdf, fraction) in resolved.points.iter().zip([[0.2, 0.3], [0.8, 0.9]]) {
                let view = geometry.to_view(pdf[0], pdf[1]);
                assert!(
                    (view.0 - fraction[0]).abs() < 0.00001
                        && (view.1 - fraction[1]).abs() < 0.00001
                );
            }
            assert!(resolve(
                &Anchor::PdfPoints {
                    points: vec![[0.0, 0.0]]
                },
                geometry,
                None
            )
            .is_err());
        }
    }
    #[test]
    fn unicode_quotes_are_exact_and_ambiguous_text_requires_an_occurrence() {
        let bounds = PdfBox {
            left: 10.0,
            bottom: 10.0,
            right: 20.0,
            top: 20.0,
        };
        let chars: Vec<_> = "測定 測定"
            .chars()
            .enumerate()
            .map(|(i, ch)| TextChar {
                ch,
                bounds: Some(PdfBox {
                    left: 10.0 + i as f32 * 12.0,
                    right: 20.0 + i as f32 * 12.0,
                    ..bounds
                }),
                ink_bounds: None,
            })
            .collect();
        let geometry = PageGeometry {
            rotation: 0,
            bounds: PdfBox {
                left: 0.0,
                bottom: 0.0,
                right: 100.0,
                top: 100.0,
            },
        };
        let second = resolve(
            &Anchor::Text {
                quote: "測定".into(),
                occurrence: 2,
            },
            geometry,
            Some(&chars),
        )
        .unwrap();
        assert!(second.quads[0].left >= 46.0);
        assert_eq!(
            resolve(
                &Anchor::Text {
                    quote: "測定".into(),
                    occurrence: 3
                },
                geometry,
                Some(&chars)
            )
            .err()
            .unwrap()
            .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            resolve(
                &Anchor::Text {
                    quote: "測定".into(),
                    occurrence: 1
                },
                geometry,
                None
            )
            .err()
            .unwrap()
            .code,
            ErrorCode::Busy
        );
    }
}
