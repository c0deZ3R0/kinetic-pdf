//! The /KPDF /GeomHash: a fingerprint of a markup's geometry and scale, so
//! that on load a markup edited by another program can be told apart from one
//! exactly as this app saved it.
//!
//! Numbers are hashed as the 32-bit reals a PDF stores them as (lopdf writes
//! each as the shortest text that reads back to the same 32-bit value). A
//! markup saved and read back hashes the same, though its 64-bit coordinates
//! in memory were rounded on the way; one moved by anything visible does not.

use twox_hash::XxHash3_64;

use crate::markup::Geometry;
use crate::scale::Scale;

/// Bumped if what's hashed ever changes, so old hashes read as stale rather
/// than as a match.
const VERSION: u8 = 1;

struct Bytes(Vec<u8>);

impl Bytes {
    fn tag(&mut self, t: u8) {
        self.0.push(t);
    }

    fn count(&mut self, n: usize) {
        self.0.extend_from_slice(&(n as u32).to_le_bytes());
    }

    fn real(&mut self, v: f64) {
        // Both zeros as one, since a file may write either.
        let v = if v == 0.0 { 0.0f32 } else { v as f32 };
        self.0.extend_from_slice(&v.to_bits().to_le_bytes());
    }
}

/// XXH3 (64-bit) of the geometry and the scale's factors. Labels, styles and
/// metadata aren't included: changing those doesn't change a quantity.
pub fn geom_hash(geometry: &Geometry, scale: Option<&Scale>) -> u64 {
    let mut b = Bytes(Vec::with_capacity(64));
    b.tag(VERSION);
    let (tag, rings) = match geometry {
        Geometry::Line { .. } => (1, geometry.rings()),
        Geometry::Polyline { .. } => (2, geometry.rings()),
        Geometry::Polygon { .. } => (3, geometry.rings()),
        Geometry::Points { .. } => (4, geometry.rings()),
        Geometry::Ellipse { .. } => (5, geometry.rings()),
        Geometry::Ink { .. } => (6, geometry.rings()),
    };
    b.tag(tag);
    b.count(rings.len());
    for ring in rings {
        b.count(ring.len());
        for p in ring {
            b.real(p.x);
            b.real(p.y);
        }
    }
    match scale {
        None => b.tag(0),
        Some(s) => {
            b.tag(1);
            b.real(s.metres_per_point_x);
            b.real(s.metres_per_point_y);
        }
    }
    XxHash3_64::oneshot(&b.0)
}

/// The hash as written to the file: 16 lowercase hex digits.
pub fn geom_hash_hex(geometry: &Geometry, scale: Option<&Scale>) -> String {
    format!("{:016x}", geom_hash(geometry, scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Pt;
    use crate::id::ScaleId;

    fn line(bx: f64) -> Geometry {
        Geometry::Line { a: Pt::new(10.0, 20.0), b: Pt::new(bx, 40.0) }
    }

    #[test]
    fn saving_and_reading_back_keeps_the_hash() {
        let original = line(3_121.123_456_789);
        // What reading the file back gives: the 32-bit value as a 64-bit one.
        let read_back = line(f64::from(3_121.123_456_789_f64 as f32));
        assert_ne!(original, read_back);
        assert_eq!(geom_hash(&original, None), geom_hash(&read_back, None));
    }

    #[test]
    fn moving_a_point_or_changing_the_scale_changes_the_hash() {
        let s100 = Scale::from_ratio(ScaleId(1), 100.0).unwrap();
        let s200 = Scale::from_ratio(ScaleId(1), 200.0).unwrap();
        assert_ne!(geom_hash(&line(30.0), None), geom_hash(&line(30.01), None));
        assert_ne!(geom_hash(&line(30.0), Some(&s100)), geom_hash(&line(30.0), Some(&s200)));
        assert_ne!(geom_hash(&line(30.0), Some(&s100)), geom_hash(&line(30.0), None));
        let as_polyline = Geometry::Polyline { pts: vec![Pt::new(10.0, 20.0), Pt::new(30.0, 40.0)] };
        assert_ne!(geom_hash(&line(30.0), None), geom_hash(&as_polyline, None), "the shape's kind counts");
    }

    #[test]
    fn the_hash_is_stable_across_versions_of_the_app() {
        // If this changes, files saved by earlier versions will all read as
        // edited elsewhere: bump VERSION deliberately instead.
        let g = Geometry::Polygon { pts: vec![Pt::new(0.0, 0.0), Pt::new(72.0, 0.0), Pt::new(72.0, 72.0)], holes: vec![] };
        let s = Scale::from_ratio(ScaleId(9), 100.0).unwrap();
        assert_eq!(geom_hash_hex(&g, Some(&s)), "2d910b37a09f81d2");
    }
}
