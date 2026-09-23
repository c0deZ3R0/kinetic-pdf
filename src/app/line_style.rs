//! Line patterns shared by the editor preview and page drawing.

use super::egui::{Painter, Pos2, Stroke};

/// Presets are stored as PDF dash and gap lengths. An empty array is solid.
pub(super) const TYPES: [(&str, &[f64]); 4] = [
    ("Solid", &[]),
    ("Dashed", &[6.0, 4.0]),
    ("Dotted", &[1.0, 3.0]),
    ("Dash-dot", &[7.0, 3.0, 1.0, 3.0]),
];

pub(super) fn is_dashed(dash: &[f64], scale: f32) -> bool {
    !dash.is_empty() && scale.is_finite() && scale > 0.0
        && dash.iter().all(|&length| length.is_finite() && length > 0.0)
        && dash.iter().map(|&length| length as f32 * scale).sum::<f32>() >= 2.0
}

/// Paint a dash array continuously around an open or closed path. Returns
/// false for a solid or invalid array so the caller can draw its usual stroke.
pub(super) fn paint_dashed(painter: &Painter, points: &[Pos2], closed: bool, stroke: Stroke, dash: &[f64], scale: f32) -> bool {
    if points.len() < 2 || !is_dashed(dash, scale) {
        return false;
    }
    let lengths: Vec<f32> = dash.iter().map(|&length| length as f32 * scale).collect();
    let mut part = 0usize;
    let mut remaining = lengths[0];
    let edges = points.len() - usize::from(!closed);
    for edge in 0..edges {
        let (from, to) = (points[edge], points[(edge + 1) % points.len()]);
        let vector = to - from;
        let distance = vector.length();
        if distance <= f32::EPSILON { continue; }
        let along = vector / distance;
        let mut offset = 0.0;
        while offset < distance {
            let step = remaining.min(distance - offset);
            if part % 2 == 0 && step > 0.0 {
                painter.line_segment([from + along * offset, from + along * (offset + step)], stroke);
            }
            offset += step;
            remaining -= step;
            if remaining <= 0.001 {
                part += 1;
                remaining = lengths[part % lengths.len()];
            }
        }
    }
    true
}
