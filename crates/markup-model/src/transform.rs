//! Between PDF user space and the screen, for one page as shown.
//!
//! Geometry is stored in unrotated user space. The page is shown turned by
//! its /Rotate and trimmed to its CropBox, whose corner needn't be at the
//! origin, then zoomed and panned. Every conversion goes through a
//! `PageTransform`, so no tool or renderer does its own.

use serde::{Deserialize, Serialize};

use crate::geom::{Pt, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageTransform {
    /// Quarter turns clockwise, 0 to 3: /Rotate 0, 90, 180, 270.
    pub rotation: u8,
    /// The CropBox, in user space.
    pub crop: Rect,
    /// Screen pixels per point.
    pub zoom: f64,
    /// Where the top-left corner of the page as shown lands on screen, in
    /// pixels.
    pub origin: Pt,
}

impl PageTransform {
    /// The page as shown, in points: width and height, swapped for a quarter
    /// turn.
    pub fn view_size(&self) -> (f64, f64) {
        let (w, h) = (self.crop.width(), self.crop.height());
        if self.rotation % 2 == 1 {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// A user-space point as points from the top-left of the page as shown,
    /// y down.
    pub fn to_view(&self, p: Pt) -> Pt {
        let c = &self.crop;
        let (u, v) = (p.x - c.min.x, c.max.y - p.y);
        let (w, h) = (c.width(), c.height());
        match self.rotation % 4 {
            0 => Pt::new(u, v),
            1 => Pt::new(h - v, u),
            2 => Pt::new(w - u, h - v),
            _ => Pt::new(v, w - u),
        }
    }

    pub fn from_view(&self, q: Pt) -> Pt {
        let c = &self.crop;
        let (w, h) = (c.width(), c.height());
        let (u, v) = match self.rotation % 4 {
            0 => (q.x, q.y),
            1 => (q.y, h - q.x),
            2 => (w - q.x, h - q.y),
            _ => (w - q.y, q.x),
        };
        Pt::new(c.min.x + u, c.max.y - v)
    }

    pub fn to_screen(&self, p: Pt) -> Pt {
        self.origin + self.to_view(p) * self.zoom
    }

    pub fn from_screen(&self, s: Pt) -> Pt {
        self.from_view((s - self.origin) * (1.0 / self.zoom))
    }

    /// Screen pixels as points, for a pick radius or snap distance.
    pub fn pixels_to_points(&self, pixels: f64) -> f64 {
        pixels / self.zoom
    }

    /// The whole mapping as an affine matrix `[a, b, c, d, e, f]`, so that
    /// screen x = a·x + c·y + e and y = b·x + d·y + f: a GPU uniform, making
    /// pan and zoom cost nothing but its update.
    pub fn to_screen_matrix(&self) -> [f64; 6] {
        let o = self.to_screen(Pt::new(0.0, 0.0));
        let x = self.to_screen(Pt::new(1.0, 0.0)) - o;
        let y = self.to_screen(Pt::new(0.0, 1.0)) - o;
        [x.x, x.y, y.x, y.y, o.x, o.y]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform(rotation: u8) -> PageTransform {
        // A crop box off the origin, as real drawing sets have.
        PageTransform {
            rotation,
            crop: Rect::from_corners(Pt::new(-50.0, 100.0), Pt::new(550.0, 500.0)),
            zoom: 2.5,
            origin: Pt::new(30.0, -40.0),
        }
    }

    fn close(a: Pt, b: Pt) -> bool {
        a.dist(b) < 1e-9
    }

    #[test]
    fn user_space_to_screen_and_back_at_every_rotation() {
        for rotation in 0..4 {
            let t = transform(rotation);
            for p in [Pt::new(-50.0, 100.0), Pt::new(123.4, 456.7), Pt::new(550.0, 500.0), Pt::new(900.0, -3.0)] {
                assert!(close(t.from_screen(t.to_screen(p)), p), "rotation {rotation}, {p:?}");
            }
        }
    }

    #[test]
    fn the_crop_box_top_left_is_the_view_origin_for_each_rotation() {
        let top_left = [Pt::new(-50.0, 500.0), Pt::new(-50.0, 100.0), Pt::new(550.0, 100.0), Pt::new(550.0, 500.0)];
        for (rotation, corner) in top_left.into_iter().enumerate() {
            let t = transform(rotation as u8);
            assert!(close(t.to_view(corner), Pt::new(0.0, 0.0)), "rotation {rotation}: {:?}", t.to_view(corner));
        }
        assert_eq!(transform(1).view_size(), (400.0, 600.0));
        // Turned clockwise, the page's left edge runs along the top.
        let t = transform(1);
        assert!(close(t.to_view(Pt::new(-50.0, 300.0)), Pt::new(200.0, 0.0)));
    }

    #[test]
    fn the_matrix_agrees_with_the_point_mapping() {
        for rotation in 0..4 {
            let t = transform(rotation);
            let [a, b, c, d, e, f] = t.to_screen_matrix();
            let p = Pt::new(77.0, 333.0);
            assert!(close(Pt::new(a * p.x + c * p.y + e, b * p.x + d * p.y + f), t.to_screen(p)));
        }
    }
}
