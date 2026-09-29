//! What every sheet is drawn with: the title strip, labels, leaders,
//! legends, notes, tables, scale bars, and the hatches for concrete, rock
//! and ground.

use std::f64::consts::PI;

use crate::geom::*;
use crate::sample_ink::{grey, text_width, wrap, Align, Font, Ink, View, BLACK, WHITE};
use crate::Sheet;

pub const STORM: [f64; 3] = [0.05, 0.33, 0.72];
pub const WATER: [f64; 3] = [0.0, 0.6, 0.85];
pub const SEWER: [f64; 3] = [0.55, 0.3, 0.1];
pub const ELECTRICITY: [f64; 3] = [0.9, 0.2, 0.1];
pub const COMMS: [f64; 3] = [0.55, 0.25, 0.75];
pub const GAS: [f64; 3] = [0.85, 0.65, 0.0];
pub const TREES: [f64; 3] = [0.2, 0.55, 0.25];
pub const BOUNDARY: [f64; 3] = [0.75, 0.1, 0.45];
pub const CREEK: [f64; 3] = [0.8, 0.9, 0.97];
pub const CONTOUR: [f64; 3] = [0.74, 0.66, 0.58];
pub const ROAD: [f64; 3] = grey(0.84);
pub const PATH: [f64; 3] = grey(0.93);
pub const MATCH: [f64; 3] = [0.1, 0.45, 0.2];

/// Text centred on `at` along `angle`, on a white ground so it reads over
/// the linework.
pub fn tag(ink: &mut Ink, at: (f64, f64), angle: f64, size: f64, font: Font, colour: [f64; 3], s: &str) {
    let w = text_width(s, font) * size;
    let (c, sn) = (angle.cos(), angle.sin());
    ink.save();
    ink.op(&format!("{c:.5} {sn:.5} {:.5} {c:.5} {:.3} {:.3} cm", -sn, at.0, at.1));
    ink.fill(WHITE);
    ink.rect(-w / 2.0 - 0.7, -size * 0.6, w + 1.4, size * 1.2, "f");
    ink.fill(colour);
    ink.text((0.0, -size * 0.36), size, font, Align::Centre, s);
    ink.restore();
}

/// Text along `angle` centred on `at`, with nothing under it.
pub fn along(ink: &mut Ink, at: (f64, f64), angle: f64, size: f64, font: Font, colour: [f64; 3], s: &str) {
    ink.fill(colour);
    let down = (angle.sin() * size * 0.36, -angle.cos() * size * 0.36);
    ink.text_rotated(add(at, down), size, font, Align::Centre, s, angle);
}

/// A leader from `target` to where `lines` of text start: to the right of
/// `at` when it is right of the target, to its left otherwise.
pub fn callout(ink: &mut Ink, target: (f64, f64), at: (f64, f64), size: f64, lines: &[&str]) {
    let right = at.0 >= target.0;
    let land = if right { 3.0 } else { -3.0 };
    ink.pen(0.18, BLACK);
    ink.line(target, at);
    ink.line(at, (at.0 + land, at.1));
    ink.fill(BLACK);
    ink.circle(target, 0.45, "f");
    let (align, x) = if right { (Align::Left, at.0 + land + 1.0) } else { (Align::Right, at.0 + land - 1.0) };
    for (i, line) in lines.iter().enumerate() {
        ink.text((x, at.1 - size * 0.36 - i as f64 * size * 1.35), size, Font::Regular, align, line);
    }
}

/// A filled arrowhead at `tip`, pointing along `u`, `size` millimetres long.
pub fn arrowhead(ink: &mut Ink, tip: (f64, f64), u: P, size: f64) {
    let back = sub(tip, mul(u, size));
    let n = mul(left(u), size * 0.35);
    ink.poly(&[tip, add(back, n), sub(back, n)], true, "f");
}

pub type Symbol<'a> = &'a dyn Fn(&mut Ink, (f64, f64));

/// A legend: each entry's symbol drawn in a box at its left, and its text.
pub fn legend(ink: &mut Ink, x: f64, top: f64, title: &str, entries: &[(&str, Symbol)]) -> f64 {
    ink.fill(BLACK);
    ink.text((x, top), 4.0, Font::Bold, Align::Left, title);
    let mut y = top - 9.0;
    for (text, symbol) in entries {
        symbol(ink, (x, y));
        ink.fill(BLACK);
        ink.text((x + 20.0, y - 1.0), 2.5, Font::Regular, Align::Left, text);
        y -= 6.2;
    }
    y
}

pub fn line_symbol(width: f64, colour: [f64; 3], dash: &'static [f64]) -> impl Fn(&mut Ink, (f64, f64)) {
    move |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.pen(width, colour);
        ink.dash(dash);
        ink.line((x, y), (x + 15.0, y));
        ink.dash(&[]);
    }
}

pub fn swatch(colour: [f64; 3]) -> impl Fn(&mut Ink, (f64, f64)) {
    move |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.fill(colour);
        ink.pen(0.13, grey(0.4));
        ink.rect(x, y - 1.8, 15.0, 3.6, "B");
    }
}

pub fn heading(ink: &mut Ink, at: (f64, f64), s: &str) {
    ink.fill(BLACK);
    ink.text(at, 4.0, Font::Bold, Align::Left, s);
}

pub fn notes_block(ink: &mut Ink, x: f64, top: f64, width: f64, title: &str, notes: &[String]) -> f64 {
    heading(ink, (x, top), title);
    let mut y = top - 7.5;
    for (i, note) in notes.iter().enumerate() {
        ink.text((x, y), 2.5, Font::Regular, Align::Left, &format!("{}.", i + 1));
        let lines = wrap(note, 2.5, Font::Regular, width - 6.0);
        y = ink.lines((x + 6.0, y), 2.5, 3.6, Font::Regular, &lines) - 1.4;
    }
    y
}

/// A table, `size` text in rows `row_h` apart. Returns its foot.
pub fn table(ink: &mut Ink, x: f64, top: f64, cols: &[(&str, f64)], rows: &[Vec<String>], row_h: f64, size: f64) -> f64 {
    let width: f64 = cols.iter().map(|c| c.1).sum();
    ink.fill(grey(0.88));
    ink.rect(x, top - row_h, width, row_h, "f");
    ink.fill(BLACK);
    let mut cx = x;
    for (name, w) in cols {
        ink.text((cx + 1.2, top - row_h * 0.7), size, Font::Bold, Align::Left, name);
        cx += w;
    }
    for (i, row) in rows.iter().enumerate() {
        let y = top - row_h * (i as f64 + 1.7);
        if i % 2 == 1 {
            ink.fill(grey(0.965));
            ink.rect(x, top - row_h * (i as f64 + 2.0), width, row_h, "f");
            ink.fill(BLACK);
        }
        let mut cx = x;
        for (cell, (_, w)) in row.iter().zip(cols) {
            ink.text((cx + 1.2, y), size, Font::Regular, Align::Left, cell);
            cx += w;
        }
    }
    let bottom = top - row_h * (rows.len() as f64 + 1.0);
    ink.pen(0.1, grey(0.45));
    let mut cx = x;
    for (_, w) in &cols[..cols.len() - 1] {
        cx += w;
        ink.line((cx, top), (cx, bottom));
    }
    ink.line((x, top - row_h), (x + width, top - row_h));
    ink.pen(0.3, BLACK);
    ink.rect(x, bottom, width, top - bottom, "S");
    bottom
}

/// A scale bar of `blocks` blocks of `metres` each, at `k` millimetres to the
/// metre.
pub fn bar(ink: &mut Ink, at: (f64, f64), k: f64, metres: f64, blocks: usize, label: &str) {
    ink.pen(0.15, BLACK);
    let w = metres * k;
    for i in 0..blocks {
        ink.fill(if i % 2 == 0 { BLACK } else { WHITE });
        ink.rect(at.0 + i as f64 * w, at.1, w, 1.6, "B");
    }
    ink.fill(BLACK);
    for i in 0..=blocks {
        ink.text((at.0 + i as f64 * w, at.1 + 2.6), 1.9, Font::Regular, Align::Centre, &format!("{}", metres * i as f64));
    }
    ink.text((at.0 + blocks as f64 * w + 3.0, at.1 + 0.2), 1.9, Font::Regular, Align::Left, "m");
    ink.text((at.0, at.1 - 3.6), 2.0, Font::Bold, Align::Left, label);
}

/// A drawing's title: its number in a circle, its name, and its scale, with
/// a reference to where it's cut from, if any.
pub fn title(ink: &mut Ink, at: (f64, f64), n: &str, name: &str, scale: &str) {
    ink.pen(0.35, BLACK);
    ink.circle((at.0 + 5.5, at.1 + 1.5), 5.5, "S");
    ink.fill(BLACK);
    ink.text((at.0 + 5.5, at.1 - 0.2), 4.2, Font::Bold, Align::Centre, n);
    ink.text((at.0 + 15.0, at.1 + 1.5), 4.6, Font::Bold, Align::Left, name);
    let w = text_width(name, Font::Bold) * 4.6;
    ink.pen(0.45, BLACK);
    ink.line((at.0 + 15.0, at.1), (at.0 + 15.0 + w, at.1));
    ink.text((at.0 + 15.0, at.1 - 4.8), 2.6, Font::Regular, Align::Left, scale);
}

/// The table under a long section: a row of values at each station, or,
/// with one fewer value, between them. Returns its foot.
pub fn bands(ink: &mut Ink, left: f64, top: f64, right: f64, xs: &[f64], rows: &[(&str, Vec<String>)], h: f64) -> f64 {
    for (i, (name, values)) in rows.iter().enumerate() {
        let y0 = top - h * (i as f64 + 1.0);
        ink.pen(if i + 1 == rows.len() { 0.35 } else { 0.15 }, BLACK);
        ink.line((left, y0), (right, y0));
        ink.fill(BLACK);
        ink.text((left + 2.0, y0 + h / 2.0 - 0.9), 2.1, Font::Bold, Align::Left, name);
        if values.len() + 1 == xs.len() {
            for (w, value) in xs.windows(2).zip(values) {
                ink.text(((w[0] + w[1]) / 2.0, y0 + h / 2.0 - 0.8), 1.9, Font::Regular, Align::Centre, value);
                ink.pen(0.1, grey(0.5));
                ink.line((w[1], y0), (w[1], y0 + h));
            }
        } else {
            for (x, value) in xs.iter().zip(values) {
                ink.text_turned((x + 0.7, y0 + h / 2.0), 1.9, Font::Regular, Align::Centre, value, true);
            }
        }
    }
    let bottom = top - h * rows.len() as f64;
    ink.pen(0.35, BLACK);
    ink.line((left, top), (right, top));
    ink.line((left, top), (left, bottom));
    ink.line((left + 40.0, top), (left + 40.0, bottom));
    ink.line((right, top), (right, bottom));
    bottom
}

/// Concrete: dots and small stones scattered over `pts`, on the paper.
pub fn concrete(ink: &mut Ink, pts: &[(f64, f64)], seed: u64) {
    ink.fill(WHITE);
    ink.poly(pts, true, "f");
    ink.clip(pts);
    let (x0, y0, x1, y1) = bounds(pts);
    let mut rng = Rng(seed | 1);
    let n = ((x1 - x0) * (y1 - y0) / 5.0) as usize;
    ink.fill(grey(0.35));
    ink.pen(0.08, grey(0.35));
    for _ in 0..n {
        let p = (x0 + rng.next() * (x1 - x0), y0 + rng.next() * (y1 - y0));
        if rng.next() < 0.8 {
            ink.circle(p, 0.12, "f");
        } else {
            let a = rng.next() * 2.0 * PI;
            let tri: Vec<(f64, f64)> = (0..3).map(|k| a + k as f64 * 2.1).map(|t| (p.0 + 0.55 * t.cos(), p.1 + 0.55 * t.sin())).collect();
            ink.poly(&tri, true, "S");
        }
    }
    ink.restore();
    ink.pen(0.3, BLACK);
    ink.poly(pts, true, "S");
}

/// Crushed rock: small stones, outlined.
pub fn aggregate(ink: &mut Ink, pts: &[(f64, f64)], seed: u64, size: f64) {
    ink.fill(WHITE);
    ink.poly(pts, true, "f");
    ink.clip(pts);
    let (x0, y0, x1, y1) = bounds(pts);
    let mut rng = Rng(seed | 1);
    let n = ((x1 - x0) * (y1 - y0) / (size * size * 4.0)) as usize;
    ink.pen(0.1, grey(0.3));
    for _ in 0..n {
        let p = (x0 + rng.next() * (x1 - x0), y0 + rng.next() * (y1 - y0));
        ink.circle(p, size * (0.4 + rng.next() * 0.5), "S");
    }
    ink.restore();
    ink.pen(0.25, BLACK);
    ink.poly(pts, true, "S");
}

/// Natural ground: short strokes hanging under a line.
pub fn earth(ink: &mut Ink, pts: &[(f64, f64)]) {
    ink.pen(0.35, BLACK);
    ink.poly(pts, false, "S");
    ink.pen(0.13, BLACK);
    for w in pts.windows(2) {
        let l = dist(w[0], w[1]);
        let u = unit(sub(w[1], w[0]));
        let mut s = 1.0;
        while s < l {
            let p = add(w[0], mul(u, s));
            ink.line(p, (p.0 - 1.8, p.1 - 1.8));
            s += 2.5;
        }
    }
}

/// Where a view's metres land on the paper, as a box of paper.
pub fn paper(v: View, b: Bx) -> Bx {
    let (a, c) = (v.p(b.0, b.1), v.p(b.2, b.3));
    (a.0, a.1, c.0, c.1)
}

// ---------------------------------------------------------------------------
// The sheet around every drawing

/// The top of the title strip along the foot of every sheet.
pub const STRIP: f64 = 56.0;

const REVISIONS: [(&str, &str, &str); 3] =
    [("A", "05.08.26", "Preliminary, for comment"), ("B", "01.09.26", "For council review"), ("C", "22.09.26", "Issued for review")];

pub fn frame(ink: &mut Ink, sheet: &Sheet, page: usize, pages: usize) {
    let (w, h) = crate::SHEET;
    ink.pen(0.7, BLACK);
    ink.rect(20.0, 10.0, w - 30.0, h - 20.0, "S");
    ink.line((20.0, STRIP), (w - 10.0, STRIP));

    // Grid references in the margin.
    ink.pen(0.25, BLACK);
    ink.fill(BLACK);
    let cols = 8;
    for i in 0..cols {
        let x0 = 20.0 + (w - 30.0) * i as f64 / cols as f64;
        let mid = x0 + (w - 30.0) / cols as f64 / 2.0;
        ink.text((mid, h - 7.5), 3.0, Font::Regular, Align::Centre, &format!("{}", i + 1));
        ink.text((mid, 4.5), 3.0, Font::Regular, Align::Centre, &format!("{}", i + 1));
        if i > 0 {
            ink.line((x0, h - 10.0), (x0, h - 5.0));
            ink.line((x0, 10.0), (x0, 5.0));
        }
    }
    let rows = ["A", "B", "C", "D", "E", "F"];
    for (i, r) in rows.iter().enumerate() {
        let y0 = h - 10.0 - (h - 20.0) * i as f64 / rows.len() as f64;
        let mid = y0 - (h - 20.0) / rows.len() as f64 / 2.0;
        ink.text((14.0, mid - 1.0), 3.0, Font::Regular, Align::Centre, r);
        ink.text((w - 5.0, mid - 1.0), 3.0, Font::Regular, Align::Centre, r);
        if i > 0 {
            ink.line((20.0, y0), (15.0, y0));
            ink.line((w - 10.0, y0), (w - 5.0, y0));
        }
    }

    let columns = [20.0, 196.0, 322.0, 432.0, 512.0, 648.0, w - 10.0];
    ink.pen(0.35, BLACK);
    for x in &columns[1..columns.len() - 1] {
        ink.line((*x, 10.0), (*x, STRIP));
    }
    let label = |ink: &mut Ink, at: (f64, f64), s: &str| {
        ink.fill(grey(0.35));
        ink.text(at, 2.0, Font::Regular, Align::Left, s);
        ink.fill(BLACK);
    };

    // Revisions.
    let x = 24.0;
    for (dx, head) in [(0.0, "REV"), (10.0, "DATE"), (28.0, "DESCRIPTION"), (138.0, "DRN"), (152.0, "APP")] {
        label(ink, (x + dx, STRIP - 5.0), head);
    }
    ink.pen(0.13, BLACK);
    ink.line((20.0, STRIP - 7.0), (columns[1], STRIP - 7.0));
    for (i, (rev, date, what)) in REVISIONS.iter().enumerate() {
        let y = STRIP - 12.5 - i as f64 * 5.5;
        for (dx, s) in [(0.0, *rev), (10.0, *date), (28.0, *what), (138.0, "SJT"), (152.0, "LM")] {
            ink.text((x + dx, y), 2.4, Font::Regular, Align::Left, s);
        }
    }
    label(ink, (x, 18.0), "Do not scale. Use figured dimensions only.");
    label(ink, (x, 14.0), "Levels in metres above the local datum.");

    // The engineers.
    let x = columns[1] + 5.0;
    ink.fill(BLACK);
    ink.rect(x, STRIP - 20.0, 13.0, 13.0, "f");
    ink.pen(0.7, WHITE);
    for k in 0..3 {
        let y = STRIP - 17.0 + k as f64 * 3.4;
        ink.poly(&[(x + 1.5, y), (x + 5.0, y + 1.6), (x + 8.5, y + 0.4), (x + 11.5, y + 1.8)], false, "S");
    }
    ink.fill(BLACK);
    ink.text((x + 17.0, STRIP - 13.0), 6.0, Font::Bold, Align::Left, "SAMPLE CIVIL");
    ink.text((x + 17.0, STRIP - 18.5), 2.2, Font::Regular, Align::Left, "CIVIL ENGINEERS  ·  SURVEYORS");
    label(ink, (x, 22.0), "A practice and a project that don’t exist,");
    label(ink, (x, 18.5), "drawn as a sample for Kinetic PDF.");

    // The client and the council.
    let x = columns[2] + 5.0;
    label(ink, (x, STRIP - 5.0), "CLIENT");
    ink.text((x, STRIP - 12.0), 4.0, Font::Bold, Align::Left, "SAMPLE");
    ink.text((x, STRIP - 17.0), 4.0, Font::Bold, Align::Left, "DEVELOPMENTS");
    label(ink, (x, 25.0), "COUNCIL");
    ink.text((x, 19.0), 3.0, Font::Regular, Align::Left, "SAMPLETON COUNCIL");

    // Who did what.
    let x = columns[3] + 4.0;
    for (i, (role, who)) in [("DESIGNED", "RP"), ("DRAWN", "SJT"), ("CHECKED", "AK"), ("APPROVED", "LM"), ("DATE", "SEPT 2026")].iter().enumerate() {
        let y = STRIP - 8.0 - i as f64 * 8.5;
        label(ink, (x, y), role);
        ink.text((x + 26.0, y), 2.6, Font::Regular, Align::Left, who);
    }

    // The project and its status.
    let x = columns[4] + 5.0;
    label(ink, (x, STRIP - 5.0), "PROJECT");
    ink.text((x, STRIP - 12.0), 4.4, Font::Bold, Align::Left, "HERON RIDGE ESTATE");
    ink.text((x, STRIP - 17.5), 2.6, Font::Regular, Align::Left, "STAGES 1 TO 6  ·  SUBDIVISION WORKS");
    label(ink, (x, 25.0), "STATUS");
    ink.text((x, 19.0), 3.4, Font::Bold, Align::Left, "ISSUED FOR REVIEW");
    label(ink, (x, 14.0), "Sample drawing. Not for construction.");

    // The drawing.
    let x = columns[5] + 5.0;
    let right = w - 14.0;
    label(ink, (x, STRIP - 5.0), "DRAWING");
    for (i, line) in sheet.title.iter().enumerate() {
        ink.text((x, STRIP - 10.5 - i as f64 * 4.6), 3.3, Font::Bold, Align::Left, line);
    }
    label(ink, (x + 118.0, STRIP - 5.0), "SCALE");
    ink.text((x + 118.0, STRIP - 10.5), 2.5, Font::Regular, Align::Left, &sheet.scale);
    ink.pen(0.35, BLACK);
    ink.line((columns[5], 28.0), (w - 10.0, 28.0));
    label(ink, (x, 23.5), "DRAWING No.");
    label(ink, (x + 70.0, 23.5), &format!("SHEET {} OF {}", page + 1, pages));
    label(ink, (right - 8.0, 23.5), "REV");
    ink.text((x, 13.0), 9.0, Font::Bold, Align::Left, &sheet.number);
    ink.text((right, 13.0), 9.0, Font::Bold, Align::Right, "C");
}
