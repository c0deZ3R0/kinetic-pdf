//! What the sample drawing sets are drawn with: a page's content stream in
//! millimetres, text in the standard Helvetica, drawings at a scale, and the
//! dimensions, tables and symbols every set has. Shared by the examples that
//! write them.

#![allow(dead_code)]

use std::f64::consts::PI;

use markup_model::Pt;

/// Points in a millimetre.
pub const PT: f64 = 72.0 / 25.4;

pub const BLACK: [f64; 3] = [0.0, 0.0, 0.0];
pub const WHITE: [f64; 3] = [1.0, 1.0, 1.0];
pub const fn grey(g: f64) -> [f64; 3] {
    [g, g, g]
}

// ---------------------------------------------------------------------------
// Drawing on a page

#[derive(Clone, Copy)]
pub enum Font {
    Regular,
    Bold,
}

#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Centre,
    Right,
}

/// A page's content stream, in millimetres.
pub struct Ink(pub Vec<u8>);

impl Ink {
    pub fn new() -> Ink {
        let mut ink = Ink(Vec::new());
        ink.op(&format!("{PT:.6} 0 0 {PT:.6} 0 0 cm 1 J 1 j"));
        ink
    }

    pub fn op(&mut self, s: &str) {
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(b'\n');
    }

    pub fn save(&mut self) {
        self.op("q");
    }

    pub fn restore(&mut self) {
        self.op("Q");
    }

    /// A solid line of `mm` in `colour`.
    pub fn pen(&mut self, mm: f64, colour: [f64; 3]) {
        let [r, g, b] = colour;
        self.op(&format!("{mm:.3} w {r:.3} {g:.3} {b:.3} RG [] 0 d"));
    }

    pub fn dash(&mut self, pattern: &[f64]) {
        let d: Vec<String> = pattern.iter().map(|v| format!("{v:.2}")).collect();
        self.op(&format!("[{}] 0 d", d.join(" ")));
    }

    pub fn fill(&mut self, [r, g, b]: [f64; 3]) {
        self.op(&format!("{r:.3} {g:.3} {b:.3} rg"));
    }

    pub fn line(&mut self, a: (f64, f64), b: (f64, f64)) {
        self.op(&format!("{:.3} {:.3} m {:.3} {:.3} l S", a.0, a.1, b.0, b.1));
    }

    pub fn path(&mut self, pts: &[(f64, f64)], close: bool) {
        for (i, p) in pts.iter().enumerate() {
            self.op(&format!("{:.3} {:.3} {}", p.0, p.1, if i == 0 { "m" } else { "l" }));
        }
        if close {
            self.op("h");
        }
    }

    pub fn poly(&mut self, pts: &[(f64, f64)], close: bool, paint: &str) {
        self.path(pts, close);
        self.op(paint);
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, paint: &str) {
        self.op(&format!("{x:.3} {y:.3} {w:.3} {h:.3} re {paint}"));
    }

    /// Clips what follows to `pts`, until the matching `restore`.
    pub fn clip(&mut self, pts: &[(f64, f64)]) {
        self.save();
        self.path(pts, true);
        self.op("W n");
    }

    pub fn arc_path(&mut self, c: (f64, f64), r: f64, a0: f64, a1: f64, start: bool) {
        let n = ((a1 - a0).abs() / (PI / 2.0)).ceil().max(1.0) as usize;
        let step = (a1 - a0) / n as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        let at = |a: f64| (c.0 + r * a.cos(), c.1 + r * a.sin());
        if start {
            let p = at(a0);
            self.op(&format!("{:.3} {:.3} m", p.0, p.1));
        }
        for i in 0..n {
            let s = a0 + step * i as f64;
            let e = s + step;
            let (p0, p3) = (at(s), at(e));
            let p1 = (p0.0 - k * r * s.sin(), p0.1 + k * r * s.cos());
            let p2 = (p3.0 + k * r * e.sin(), p3.1 - k * r * e.cos());
            self.op(&format!("{:.3} {:.3} {:.3} {:.3} {:.3} {:.3} c", p1.0, p1.1, p2.0, p2.1, p3.0, p3.1));
        }
    }

    pub fn arc(&mut self, c: (f64, f64), r: f64, a0: f64, a1: f64) {
        self.arc_path(c, r, a0, a1, true);
        self.op("S");
    }

    pub fn circle(&mut self, c: (f64, f64), r: f64, paint: &str) {
        self.arc_path(c, r, 0.0, 2.0 * PI, true);
        self.op("h");
        self.op(paint);
    }

    pub fn text(&mut self, at: (f64, f64), size: f64, font: Font, align: Align, s: &str) {
        self.text_turned(at, size, font, align, s, false);
    }

    /// Text reading upwards when `up`, as a vertical dimension's does.
    pub fn text_turned(&mut self, at: (f64, f64), size: f64, font: Font, align: Align, s: &str, up: bool) {
        let w = text_width(s, font) * size;
        let shift = match align {
            Align::Left => 0.0,
            Align::Centre => w / 2.0,
            Align::Right => w,
        };
        let (m, x, y) = if up { ("0 1 -1 0", at.0, at.1 - shift) } else { ("1 0 0 1", at.0 - shift, at.1) };
        let name = match font {
            Font::Regular => "F1",
            Font::Bold => "F2",
        };
        self.0.extend_from_slice(format!("BT /{name} {size:.2} Tf {m} {x:.3} {y:.3} Tm (").as_bytes());
        self.0.extend(encode(s));
        self.0.extend_from_slice(b") Tj ET\n");
    }

    /// Text running at `angle` radians from the x-axis, anticlockwise.
    pub fn text_rotated(&mut self, at: (f64, f64), size: f64, font: Font, align: Align, s: &str, angle: f64) {
        let w = text_width(s, font) * size;
        let shift = match align {
            Align::Left => 0.0,
            Align::Centre => w / 2.0,
            Align::Right => w,
        };
        let (c, sn) = (angle.cos(), angle.sin());
        let (x, y) = (at.0 - shift * c, at.1 - shift * sn);
        let name = match font {
            Font::Regular => "F1",
            Font::Bold => "F2",
        };
        self.0.extend_from_slice(format!("BT /{name} {size:.2} Tf {c:.5} {sn:.5} {:.5} {c:.5} {x:.3} {y:.3} Tm (", -sn).as_bytes());
        self.0.extend(encode(s));
        self.0.extend_from_slice(b") Tj ET\n");
    }

    /// `lines` from the top down, `leading` apart. Returns where the next
    /// line would go.
    pub fn lines(&mut self, at: (f64, f64), size: f64, leading: f64, font: Font, lines: &[String]) -> f64 {
        let mut y = at.1;
        for line in lines {
            self.text((at.0, y), size, font, Align::Left, line);
            y -= leading;
        }
        y
    }
}

/// Helvetica's advance widths for ' ' to '~', in thousandths of an em.
pub const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // ' ' to '/'
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // digits
    278, 278, 584, 584, 584, 556, 1015, // ':' to '@'
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944,
    667, 667, 611, // capitals
    278, 278, 278, 469, 556, 333, // '[' to '`'
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722,
    500, 500, 500, // small letters
    334, 260, 334, 584, // '{' to '~'
];

/// How wide `s` is at a size of 1.
pub fn text_width(s: &str, font: Font) -> f64 {
    let em: f64 = s
        .chars()
        .map(|c| match c as u32 {
            32..=126 => HELVETICA[c as usize - 32] as f64,
            _ => 556.0,
        })
        .sum();
    em / 1000.0 * if matches!(font, Font::Bold) { 1.07 } else { 1.0 }
}

/// `s` as a PDF string in WinAnsiEncoding, brackets escaped.
pub fn encode(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => out.extend([b'\\', c as u8]),
            ' '..='~' => out.push(c as u8),
            '²' => out.push(0xB2),
            '°' => out.push(0xB0),
            '–' => out.push(0x96),
            '—' => out.push(0x97),
            '×' => out.push(0xD7),
            '’' => out.push(0x92),
            '·' => out.push(0xB7),
            'Ø' => out.push(0xD8),
            _ => out.push(b'?'),
        }
    }
    out
}

/// `s` broken into lines no wider than `width`.
pub fn wrap(s: &str, size: f64, font: Font, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let tried = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
        if text_width(&tried, font) * size > width && !line.is_empty() {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = tried;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A small, repeatable random number generator, so the concrete's stipple is
/// the same on every run.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

pub fn inside(pts: &[(f64, f64)], p: (f64, f64)) -> bool {
    let mut odd = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            odd = !odd;
        }
        j = i;
    }
    odd
}

// ---------------------------------------------------------------------------
// A drawing at a scale

/// Where a drawing's origin sits on the paper, and millimetres of paper to
/// a metre.
#[derive(Clone, Copy)]
pub struct View {
    pub ox: f64,
    pub oy: f64,
    pub k: f64,
}

impl View {
    pub fn p(&self, x: f64, y: f64) -> (f64, f64) {
        (self.ox + x * self.k, self.oy + y * self.k)
    }

    pub fn all(&self, pts: &[(f64, f64)]) -> Vec<(f64, f64)> {
        pts.iter().map(|&(x, y)| self.p(x, y)).collect()
    }

    /// A point on the page in PDF points, as a markup stores it.
    pub fn pt(&self, x: f64, y: f64) -> Pt {
        let (a, b) = self.p(x, y);
        Pt::new(a * PT, b * PT)
    }

    pub fn rect(&self, ink: &mut Ink, x0: f64, y0: f64, x1: f64, y1: f64, paint: &str) {
        let (a, b) = self.p(x0, y0);
        ink.rect(a, b, (x1 - x0) * self.k, (y1 - y0) * self.k, paint);
    }

    pub fn line(&self, ink: &mut Ink, a: (f64, f64), b: (f64, f64)) {
        ink.line(self.p(a.0, a.1), self.p(b.0, b.1));
    }

    /// Lines at `angle` degrees, `step` apart, across the box `x0..x1`,
    /// `y0..y1`. Clip first to keep them inside a shape.
    #[allow(clippy::too_many_arguments)]
    pub fn hatch(&self, ink: &mut Ink, x0: f64, y0: f64, x1: f64, y1: f64, angle: f64, step: f64) {
        let (dx, dy) = (angle.to_radians().cos(), angle.to_radians().sin());
        let (nx, ny) = (-dy, dx);
        let corners = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)];
        let along = |p: &(f64, f64)| p.0 * dx + p.1 * dy;
        let across = |p: &(f64, f64)| p.0 * nx + p.1 * ny;
        let (lo, hi) = corners.iter().map(across).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
        let (s0, s1) = corners.iter().map(along).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
        let mut c = (lo / step).floor() * step;
        while c <= hi {
            let a = (s0 * dx + c * nx, s0 * dy + c * ny);
            let b = (s1 * dx + c * nx, s1 * dy + c * ny);
            self.line(ink, a, b);
            c += step;
        }
    }

    /// A chain of dimensions along x, its line `at` millimetres up the paper
    /// and its extension lines starting from `from`.
    pub fn dims_x(&self, ink: &mut Ink, xs: &[f64], at: f64, from: f64) {
        ink.pen(0.13, BLACK);
        let first = self.p(xs[0], 0.0).0;
        let last = self.p(xs[xs.len() - 1], 0.0).0;
        ink.line((first - 2.0, at), (last + 2.0, at));
        let up = if from > at { 1.0 } else { -1.0 };
        for &x in xs {
            let px = self.p(x, 0.0).0;
            ink.line((px, from - up * 2.0), (px, at - up * 2.0));
            ink.pen(0.35, BLACK);
            ink.line((px - 1.2, at - 1.2), (px + 1.2, at + 1.2));
            ink.pen(0.13, BLACK);
        }
        ink.fill(BLACK);
        for pair in xs.windows(2) {
            let mid = self.p((pair[0] + pair[1]) / 2.0, 0.0).0;
            let mm = ((pair[1] - pair[0]) * 1000.0).round();
            ink.text((mid, at + 1.2), 2.5, Font::Regular, Align::Centre, &format!("{mm}"));
        }
    }

    pub fn dims_y(&self, ink: &mut Ink, ys: &[f64], at: f64, from: f64) {
        ink.pen(0.13, BLACK);
        let first = self.p(0.0, ys[0]).1;
        let last = self.p(0.0, ys[ys.len() - 1]).1;
        ink.line((at, first - 2.0), (at, last + 2.0));
        let right = if from > at { 1.0 } else { -1.0 };
        for &y in ys {
            let py = self.p(0.0, y).1;
            ink.line((from - right * 2.0, py), (at - right * 2.0, py));
            ink.pen(0.35, BLACK);
            ink.line((at - 1.2, py - 1.2), (at + 1.2, py + 1.2));
            ink.pen(0.13, BLACK);
        }
        ink.fill(BLACK);
        for pair in ys.windows(2) {
            let mid = self.p(0.0, (pair[0] + pair[1]) / 2.0).1;
            let mm = ((pair[1] - pair[0]) * 1000.0).round();
            ink.text_turned((at - 1.2, mid), 2.5, Font::Regular, Align::Centre, &format!("{mm}"), true);
        }
    }
}

// ---------------------------------------------------------------------------
// On every sheet

pub fn north(ink: &mut Ink, c: (f64, f64), r: f64) {
    ink.pen(0.25, BLACK);
    ink.circle(c, r, "S");
    ink.fill(BLACK);
    ink.poly(&[(c.0, c.1 + r), (c.0 - r * 0.45, c.1 - r * 0.7), (c.0, c.1 - r * 0.35)], true, "f");
    ink.poly(&[(c.0, c.1 + r), (c.0 + r * 0.45, c.1 - r * 0.7), (c.0, c.1 - r * 0.35)], true, "S");
    ink.text((c.0, c.1 + r + 2.0), r * 0.55, Font::Bold, Align::Centre, "N");
}

/// The title under a drawing: its number in a circle, its name and scale.
pub fn view_title(ink: &mut Ink, at: (f64, f64), n: &str, name: &str, scale: &str) {
    ink.pen(0.35, BLACK);
    ink.circle((at.0 + 6.0, at.1 + 1.5), 6.0, "S");
    ink.fill(BLACK);
    ink.text((at.0 + 6.0, at.1 - 0.3), 5.0, Font::Bold, Align::Centre, n);
    ink.text((at.0 + 16.0, at.1 + 1.5), 5.0, Font::Bold, Align::Left, name);
    let w = text_width(name, Font::Bold) * 5.0;
    ink.pen(0.5, BLACK);
    ink.line((at.0 + 16.0, at.1), (at.0 + 16.0 + w, at.1));
    ink.text((at.0 + 16.0, at.1 - 5.0), 2.8, Font::Regular, Align::Left, scale);
}

/// A bar `metres` long in one-metre blocks at `k` millimetres to the metre.
pub fn scale_bar(ink: &mut Ink, at: (f64, f64), k: f64, metres: usize) {
    ink.pen(0.18, BLACK);
    for i in 0..metres {
        ink.fill(if i % 2 == 0 { BLACK } else { WHITE });
        ink.rect(at.0 + i as f64 * k, at.1, k, 2.0, "B");
    }
    ink.fill(BLACK);
    for i in 0..=metres {
        ink.text((at.0 + i as f64 * k, at.1 + 3.0), 2.2, Font::Regular, Align::Centre, &format!("{i}"));
    }
    ink.text((at.0 + metres as f64 * k + 3.0, at.1 + 0.3), 2.2, Font::Regular, Align::Left, "m");
}

/// A numbered bubble on a leader, pointing at `target`.
pub fn keynote(ink: &mut Ink, target: (f64, f64), bubble: (f64, f64), n: usize) {
    ink.pen(0.18, BLACK);
    let (dx, dy) = (target.0 - bubble.0, target.1 - bubble.1);
    let len = (dx * dx + dy * dy).sqrt();
    ink.line((bubble.0 + dx / len * 3.2, bubble.1 + dy / len * 3.2), target);
    ink.fill(BLACK);
    ink.circle(target, 0.6, "f");
    ink.fill(WHITE);
    ink.circle(bubble, 3.2, "B");
    ink.fill(BLACK);
    ink.text((bubble.0, bubble.1 - 0.9), 2.6, Font::Bold, Align::Centre, &format!("{n}"));
}

pub fn table(ink: &mut Ink, x: f64, top: f64, cols: &[(&str, f64)], rows: &[Vec<String>]) -> f64 {
    let size = 2.8;
    let row_h = 6.5;
    let width: f64 = cols.iter().map(|c| c.1).sum();
    ink.fill(grey(0.88));
    ink.rect(x, top - row_h, width, row_h, "f");
    ink.fill(BLACK);
    let mut cx = x;
    for (name, w) in cols {
        ink.text((cx + 2.0, top - row_h + 2.2), size, Font::Bold, Align::Left, name);
        cx += w;
    }
    for (i, row) in rows.iter().enumerate() {
        let y = top - row_h * (i as f64 + 2.0);
        let mut cx = x;
        for (cell, (_, w)) in row.iter().zip(cols) {
            ink.text((cx + 2.0, y + 2.2), size, Font::Regular, Align::Left, cell);
            cx += w;
        }
    }
    let bottom = top - row_h * (rows.len() as f64 + 1.0);
    ink.pen(0.13, BLACK);
    for i in 1..=rows.len() {
        let y = top - row_h * i as f64;
        ink.line((x, y), (x + width, y));
    }
    let mut cx = x;
    for (_, w) in &cols[..cols.len() - 1] {
        cx += w;
        ink.line((cx, top), (cx, bottom));
    }
    ink.pen(0.35, BLACK);
    ink.rect(x, bottom, width, top - bottom, "S");
    bottom
}
