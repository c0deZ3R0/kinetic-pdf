//! Writes the drawing set the Microsoft Store screenshots are taken of: a
//! house that doesn't exist, drawn for the purpose, so nobody's real drawings
//! are shown. Seven A1 sheets -- notes, plan, roof, elevations, schedules and
//! details -- with their scales set, a take-off measured, and highlights and
//! notes on the notes sheet, all written by the app's own save, as a user's
//! would be.
//!
//!     cargo run --release --example store_drawings -- out.pdf
//!
//! Everything is drawn in millimetres on the paper; `View` turns a drawing's
//! metres into paper at its scale.

use std::f64::consts::PI;

use kinetic_pdf::model::{Changes, Markup as Markup_, MarkupKind as DrawKind, MeasureChanges, NewHighlight, PdfBox, ScaleChanges};
use kinetic_pdf::{annots, selection, worker};
use markup_model::{Geometry, Markup, MarkupKind, Pt, Rect, Scale, ScaleId, ScaleStore};
use pdf_content::lopdf::{dictionary, Document, Object, Stream, StringFormat};

mod sample_ink;
use sample_ink::*;

/// A1 landscape.
const SHEET: (f64, f64) = (841.0, 594.0);
/// Where the drawing area ends and the title block starts.
const TITLE_X: f64 = 721.0;
/// Whose notes they are.
const AUTHOR: &str = "J. Morgan";

// ---------------------------------------------------------------------------
// The sheet around every drawing

struct Sheet {
    number: &'static str,
    title: &'static [&'static str],
    scale: &'static str,
}

fn frame(ink: &mut Ink, sheet: &Sheet) {
    let (w, h) = SHEET;
    ink.pen(0.7, BLACK);
    ink.rect(20.0, 10.0, w - 30.0, h - 20.0, "S");
    ink.line((TITLE_X, 10.0), (TITLE_X, h - 10.0));

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

    let x = TITLE_X + 4.0;
    let right = w - 14.0;
    let rule = |ink: &mut Ink, y: f64| {
        ink.pen(0.35, BLACK);
        ink.line((TITLE_X, y), (w - 10.0, y));
    };
    let label = |ink: &mut Ink, at: (f64, f64), s: &str| {
        ink.fill(grey(0.35));
        ink.text(at, 2.0, Font::Regular, Align::Left, s);
        ink.fill(BLACK);
    };

    ink.fill(BLACK);
    ink.text((x, 569.0), 6.5, Font::Bold, Align::Left, "SAMPLE STUDIO");
    ink.text((x, 562.0), 2.5, Font::Regular, Align::Left, "ARCHITECTURE  ·  INTERIORS");
    ink.fill(grey(0.35));
    ink.text((x, 554.0), 2.2, Font::Regular, Align::Left, "A practice and a project that don’t exist,");
    ink.text((x, 550.5), 2.2, Font::Regular, Align::Left, "drawn as a sample for Kinetic PDF.");
    rule(ink, 544.0);

    label(ink, (x, 539.0), "NOTES");
    let notes = [
        "Do not scale from drawings. Use figured dimensions only.",
        "Verify all dimensions on site before starting work or making shop drawings.",
        "Read with the general notes on A-001.",
    ];
    let mut y = 533.5;
    for note in notes {
        for line in wrap(note, 2.2, Font::Regular, right - x) {
            ink.text((x, y), 2.2, Font::Regular, Align::Left, &line);
            y -= 3.4;
        }
        y -= 1.2;
    }
    rule(ink, 500.0);

    label(ink, (x, 495.0), "REV");
    label(ink, (x + 12.0, 495.0), "DATE");
    label(ink, (x + 32.0, 495.0), "DESCRIPTION");
    let revisions = [("A", "02.09.26", "Preliminary"), ("B", "14.09.26", "Issued for approval"), ("C", "21.09.26", "Coordination issue")];
    for (i, (rev, date, what)) in revisions.iter().enumerate() {
        let y = 489.0 - i as f64 * 5.0;
        ink.text((x, y), 2.5, Font::Regular, Align::Left, rev);
        ink.text((x + 12.0, y), 2.5, Font::Regular, Align::Left, date);
        ink.text((x + 32.0, y), 2.5, Font::Regular, Align::Left, what);
    }
    rule(ink, 470.0);

    label(ink, (x, 465.0), "PROJECT");
    ink.text((x, 457.0), 4.5, Font::Bold, Align::Left, "KESTREL LANE HOUSE");
    ink.text((x, 450.5), 2.5, Font::Regular, Align::Left, "12 Kestrel Lane, Sampleton");
    ink.text((x, 446.0), 2.5, Font::Regular, Align::Left, "New single-storey dwelling");
    rule(ink, 440.0);

    label(ink, (x, 435.0), "DRAWING");
    let mut y = 427.0;
    for line in sheet.title {
        ink.text((x, y), 4.5, Font::Bold, Align::Left, line);
        y -= 6.5;
    }
    rule(ink, 405.0);

    let half = x + 52.0;
    label(ink, (x, 400.0), "SCALE");
    label(ink, (half, 400.0), "DATE");
    ink.text((x, 394.0), 3.0, Font::Regular, Align::Left, sheet.scale);
    ink.text((half, 394.0), 3.0, Font::Regular, Align::Left, "SEPT 2026");
    label(ink, (x, 388.0), "DRAWN");
    label(ink, (half, 388.0), "CHECKED");
    ink.text((x, 382.0), 3.0, Font::Regular, Align::Left, "JM");
    ink.text((half, 382.0), 3.0, Font::Regular, Align::Left, "AK");
    rule(ink, 377.0);

    // A key plan: the house, small, with north.
    label(ink, (x, 372.0), "KEY PLAN");
    let key = View { ox: TITLE_X + 19.0, oy: 300.0, k: 3.8 };
    ink.fill(grey(0.85));
    ink.pen(0.35, BLACK);
    key.rect(ink, 0.0, 0.0, 18.0, 12.0, "B");
    ink.pen(0.18, BLACK);
    ink.dash(&[1.0, 0.8]);
    key.rect(ink, -1.0, -3.6, 19.0, 13.0, "S");
    ink.dash(&[]);
    north(ink, (TITLE_X + 55.0, 272.0), 6.0);
    rule(ink, 250.0);

    label(ink, (x, 245.0), "STATUS");
    ink.fill(BLACK);
    ink.text((x, 236.0), 4.0, Font::Bold, Align::Left, "FOR COORDINATION");
    ink.fill(grey(0.35));
    for (i, line) in wrap("Sample drawing for demonstration. Not for construction.", 2.5, Font::Regular, right - x).iter().enumerate() {
        ink.text((x, 229.0 - i as f64 * 4.0), 2.5, Font::Regular, Align::Left, line);
    }
    ink.fill(BLACK);
    rule(ink, 60.0);

    label(ink, (x, 54.0), "DRAWING No.");
    label(ink, (right - 8.0, 54.0), "REV");
    ink.text((x, 28.0), 12.0, Font::Bold, Align::Left, sheet.number);
    ink.text((right, 28.0), 12.0, Font::Bold, Align::Right, "C");
}

// ---------------------------------------------------------------------------
// The house

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Hinged at its end (`b`) or its start, swinging to the side of the
    /// wall that is +1 or -1 along its normal.
    Door { hinge_end: bool, side: f64 },
    Slider,
    Garage,
    Window,
    /// A gap with a bulkhead over it.
    Void,
}

#[derive(Clone, Copy)]
struct Opening {
    a: f64,
    b: f64,
    kind: Kind,
    mark: &'static str,
    room: &'static str,
}

/// A straight wall along x or y, `from` its lower end `to` its higher one.
struct Wall {
    from: (f64, f64),
    to: (f64, f64),
    t: f64,
    /// Which face of the house an outside wall is on.
    face: Option<char>,
    openings: Vec<Opening>,
}

impl Wall {
    fn along(&self) -> (f64, f64) {
        if self.from.1 == self.to.1 { (1.0, 0.0) } else { (0.0, 1.0) }
    }

    /// +y for a wall along x, +x for one along y.
    fn normal(&self) -> (f64, f64) {
        if self.from.1 == self.to.1 { (0.0, 1.0) } else { (1.0, 0.0) }
    }

    fn len(&self) -> f64 {
        (self.to.0 - self.from.0) + (self.to.1 - self.from.1)
    }

    fn at(&self, s: f64, n: f64) -> (f64, f64) {
        let (a, nn) = (self.along(), self.normal());
        (self.from.0 + a.0 * s + nn.0 * n, self.from.1 + a.1 * s + nn.1 * n)
    }
}

const EXT: f64 = 0.27;
const INT: f64 = 0.11;

fn o(a: f64, b: f64, kind: Kind, mark: &'static str, room: &'static str) -> Opening {
    Opening { a, b, kind, mark, room }
}

fn door(hinge_end: bool, side: f64) -> Kind {
    Kind::Door { hinge_end, side }
}

fn walls() -> Vec<Wall> {
    use Kind::*;
    let wall = |from, to, t, face, openings| Wall { from, to, t, face, openings };
    vec![
        wall((0.0, 0.0), (18.0, 0.0), EXT, Some('S'), vec![
            o(0.6, 5.4, Garage, "GD01", "Garage"),
            o(6.7, 7.7, door(false, 1.0), "D01", "Entry"),
            o(9.0, 10.5, Window, "W01", "Study"),
            o(12.0, 14.5, Window, "W02", "Living"),
            o(15.0, 17.4, Slider, "D09", "Living"),
        ]),
        wall((0.0, 12.0), (18.0, 12.0), EXT, Some('N'), vec![
            o(1.5, 4.5, Window, "W03", "Bed 1"),
            o(9.3, 10.2, door(true, -1.0), "D08", "Laundry"),
            o(12.5, 14.5, Window, "W04", "Kitchen"),
            o(15.5, 17.2, Window, "W05", "Dining"),
        ]),
        wall((0.0, 0.0), (0.0, 12.0), EXT, Some('W'), vec![o(9.3, 11.2, Window, "W06", "Bed 1")]),
        wall((18.0, 0.0), (18.0, 12.0), EXT, Some('E'), vec![
            o(1.2, 4.2, Window, "W07", "Living"),
            o(7.0, 10.0, Window, "W08", "Dining"),
        ]),
        wall((6.0, 0.0), (6.0, 12.0), INT, None, vec![
            o(3.6, 4.42, door(false, 1.0), "D02", "Garage"),
            o(6.3, 7.12, door(false, -1.0), "D03", "Bath"),
            o(9.3, 10.12, door(true, -1.0), "D04", "Bed 1"),
        ]),
        wall((0.0, 5.5), (6.0, 5.5), INT, None, vec![]),
        wall((0.0, 8.3), (6.0, 8.3), INT, None, vec![o(0.9, 1.7, door(false, -1.0), "D05", "WIR")]),
        wall((2.5, 5.5), (2.5, 8.3), INT, None, vec![]),
        wall((8.5, 0.0), (8.5, 12.0), INT, None, vec![
            o(1.6, 2.42, door(false, 1.0), "D06", "Study"),
            o(4.2, 7.6, Void, "", ""),
            o(10.2, 11.02, door(true, 1.0), "D07", "Laundry"),
        ]),
        wall((8.5, 3.5), (11.0, 3.5), INT, None, vec![]),
        wall((11.0, 0.0), (11.0, 3.5), INT, None, vec![]),
        wall((8.5, 9.5), (11.0, 9.5), INT, None, vec![]),
        wall((11.0, 9.5), (11.0, 12.0), INT, None, vec![]),
    ]
}

/// The inside faces of the rooms, for labels and take-off.
const OPEN_PLAN: [(f64, f64); 8] =
    [(11.055, 0.135), (17.865, 0.135), (17.865, 11.865), (11.055, 11.865), (11.055, 9.555), (8.555, 9.555), (8.555, 3.555), (11.055, 3.555)];
const ISLAND: (f64, f64, f64, f64) = (12.0, 6.6, 14.4, 7.5);
const DECK: (f64, f64, f64, f64) = (12.0, -3.2, 18.0, -0.135);

fn rect_pts(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<(f64, f64)> {
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

fn draw_walls(ink: &mut Ink, v: View, walls: &[Wall]) {
    ink.fill(grey(0.3));
    for w in walls {
        let ext = if w.face.is_some() { w.t / 2.0 } else { 0.0 };
        let mut cuts: Vec<Opening> = w.openings.clone();
        cuts.sort_by(|a, b| a.a.total_cmp(&b.a));
        let mut start = -ext;
        let mut pieces = Vec::new();
        for op in &cuts {
            pieces.push((start, op.a));
            start = op.b;
        }
        pieces.push((start, w.len() + ext));
        for (s0, s1) in pieces.into_iter().filter(|(a, b)| b > a) {
            let pts = [w.at(s0, -w.t / 2.0), w.at(s1, -w.t / 2.0), w.at(s1, w.t / 2.0), w.at(s0, w.t / 2.0)];
            ink.poly(&v.all(&pts), true, "f");
        }
    }

    for w in walls {
        let h = w.t / 2.0;
        for op in &w.openings {
            ink.pen(0.13, BLACK);
            let jambs = |ink: &mut Ink| {
                v.line(ink, w.at(op.a, -h), w.at(op.a, h));
                v.line(ink, w.at(op.b, -h), w.at(op.b, h));
            };
            match op.kind {
                Kind::Window => {
                    jambs(ink);
                    for n in [-h, h, -0.02, 0.02] {
                        v.line(ink, w.at(op.a, n), w.at(op.b, n));
                    }
                }
                Kind::Door { hinge_end, side } => {
                    jambs(ink);
                    let (hinge, other) = if hinge_end { (op.b, op.a) } else { (op.a, op.b) };
                    let width = op.b - op.a;
                    let pivot = w.at(hinge, 0.0);
                    let tip = w.at(hinge, side * width);
                    ink.pen(0.25, BLACK);
                    v.line(ink, pivot, tip);
                    ink.pen(0.1, BLACK);
                    let jamb = w.at(other, 0.0);
                    let a0 = (tip.1 - pivot.1).atan2(tip.0 - pivot.0);
                    let a1 = (jamb.1 - pivot.1).atan2(jamb.0 - pivot.0);
                    let mut sweep = a1 - a0;
                    if sweep > PI {
                        sweep -= 2.0 * PI;
                    }
                    if sweep < -PI {
                        sweep += 2.0 * PI;
                    }
                    ink.arc(v.p(pivot.0, pivot.1), width * v.k, a0, a0 + sweep);
                }
                Kind::Slider => {
                    jambs(ink);
                    let mid = (op.a + op.b) / 2.0;
                    ink.fill(WHITE);
                    let one = [w.at(op.a, -0.04), w.at(mid + 0.05, -0.04), w.at(mid + 0.05, 0.0), w.at(op.a, 0.0)];
                    let two = [w.at(mid - 0.05, 0.0), w.at(op.b, 0.0), w.at(op.b, 0.04), w.at(mid - 0.05, 0.04)];
                    ink.poly(&v.all(&one), true, "B");
                    ink.poly(&v.all(&two), true, "B");
                }
                Kind::Garage => {
                    jambs(ink);
                    v.line(ink, w.at(op.a, 0.0), w.at(op.b, 0.0));
                    ink.dash(&[2.0, 1.2]);
                    v.line(ink, w.at(op.a, 0.45), w.at(op.b, 0.45));
                    ink.dash(&[]);
                }
                Kind::Void => {
                    ink.dash(&[1.5, 1.0]);
                    v.line(ink, w.at(op.a, -h), w.at(op.b, -h));
                    v.line(ink, w.at(op.a, h), w.at(op.b, h));
                    ink.dash(&[]);
                }
            }
        }
    }

    // Marks: windows outside the house, doors on the side they don't open to.
    for w in walls {
        for op in w.openings.iter().filter(|op| !op.mark.is_empty()) {
            let mid = (op.a + op.b) / 2.0;
            ink.fill(WHITE);
            ink.pen(0.18, BLACK);
            match op.kind {
                Kind::Window => {
                    let out = match w.face {
                        Some('S') | Some('W') => -0.5,
                        _ => 0.5,
                    };
                    let c = v.p(w.at(mid, out).0, w.at(mid, out).1);
                    ink.rect(c.0 - 4.5, c.1 - 2.2, 9.0, 4.4, "B");
                    ink.fill(BLACK);
                    ink.text((c.0, c.1 - 0.9), 2.4, Font::Regular, Align::Centre, op.mark);
                }
                _ => {
                    let side = match op.kind {
                        Kind::Door { side, .. } => -side,
                        Kind::Garage => 1.0,
                        _ => 1.0,
                    };
                    let off = if op.kind == Kind::Garage { 1.2 } else { 0.55 };
                    let c = v.p(w.at(mid, side * off).0, w.at(mid, side * off).1);
                    ink.circle(c, 3.0, "B");
                    ink.fill(BLACK);
                    ink.text((c.0, c.1 - 0.8), 2.2, Font::Regular, Align::Centre, op.mark);
                }
            }
        }
    }
}

fn room(ink: &mut Ink, v: View, at: (f64, f64), name: &str, area: Option<f64>) {
    let c = v.p(at.0, at.1);
    ink.fill(BLACK);
    ink.text((c.0, c.1 + 0.5), 3.5, Font::Bold, Align::Centre, name);
    if let Some(a) = area {
        ink.text((c.0, c.1 - 4.0), 2.5, Font::Regular, Align::Centre, &format!("{a:.1} m²"));
    }
}

fn rounded(ink: &mut Ink, v: View, x0: f64, y0: f64, x1: f64, y1: f64, r: f64, paint: &str) {
    let (a, b) = (v.p(x0, y0), v.p(x1, y1));
    let r = r * v.k;
    ink.op(&format!("{:.3} {:.3} m", a.0 + r, a.1));
    ink.op(&format!("{:.3} {:.3} l", b.0 - r, a.1));
    ink.arc_path((b.0 - r, a.1 + r), r, -PI / 2.0, 0.0, false);
    ink.op(&format!("{:.3} {:.3} l", b.0, b.1 - r));
    ink.arc_path((b.0 - r, b.1 - r), r, 0.0, PI / 2.0, false);
    ink.op(&format!("{:.3} {:.3} l", a.0 + r, b.1));
    ink.arc_path((a.0 + r, b.1 - r), r, PI / 2.0, PI, false);
    ink.op(&format!("{:.3} {:.3} l", a.0, a.1 + r));
    ink.arc_path((a.0 + r, a.1 + r), r, PI, 1.5 * PI, false);
    ink.op("h");
    ink.op(paint);
}

fn car(ink: &mut Ink, v: View, x: f64, y: f64) {
    let (w, l) = (1.85, 4.6);
    ink.pen(0.18, grey(0.25));
    ink.fill(WHITE);
    rounded(ink, v, x, y, x + w, y + l, 0.35, "B");
    rounded(ink, v, x + 0.2, y + 1.3, x + w - 0.2, y + 3.4, 0.2, "S");
    v.line(ink, (x + 0.15, y + 1.05), (x + w - 0.15, y + 1.05));
    v.line(ink, (x + 0.2, y + 3.75), (x + w - 0.2, y + 3.75));
    v.rect(ink, x - 0.12, y + 3.2, x, y + 3.45, "B");
    v.rect(ink, x + w, y + 3.2, x + w + 0.12, y + 3.45, "B");
}

fn downlight(ink: &mut Ink, v: View, at: (f64, f64)) {
    let c = v.p(at.0, at.1);
    let r = 0.11 * v.k;
    ink.pen(0.18, BLACK);
    ink.fill(WHITE);
    ink.circle(c, r, "B");
    let d = r * 0.7;
    ink.line((c.0 - d, c.1 - d), (c.0 + d, c.1 + d));
    ink.line((c.0 - d, c.1 + d), (c.0 + d, c.1 - d));
}

const DOWNLIGHTS_OPEN: [(f64, f64); 15] = [
    (12.3, 1.4), (14.4, 1.4), (16.5, 1.4),
    (12.3, 3.9), (14.4, 3.9), (16.5, 3.9),
    (12.3, 6.0), (14.4, 5.8), (16.5, 6.0),
    (12.3, 8.4), (14.4, 8.4), (16.5, 8.4),
    (12.3, 10.5), (14.4, 10.5), (16.5, 10.5),
];
const DOWNLIGHTS_HALL: [(f64, f64); 4] = [(7.3, 1.5), (7.3, 4.6), (7.3, 7.8), (7.3, 10.8)];

fn furniture(ink: &mut Ink, v: View) {
    ink.pen(0.13, grey(0.25));
    ink.fill(WHITE);
    // Bed 1: a queen bed against the north wall, with bedside tables.
    v.rect(ink, 2.235, 9.835, 3.765, 11.865, "B");
    v.rect(ink, 2.33, 11.35, 2.95, 11.75, "B");
    v.rect(ink, 3.05, 11.35, 3.67, 11.75, "B");
    v.line(ink, (2.235, 10.85), (3.765, 10.85));
    v.rect(ink, 1.6, 11.365, 2.1, 11.865, "B");
    v.rect(ink, 3.9, 11.365, 4.4, 11.865, "B");
    // Walk-in robe: shelves and a hanging rail.
    v.rect(ink, 0.135, 5.555, 0.735, 8.245, "B");
    ink.dash(&[1.0, 0.8]);
    v.line(ink, (1.05, 5.8), (1.05, 8.0));
    ink.dash(&[]);
    // Bath: tub, vanity, WC and shower.
    rounded(ink, v, 4.2, 5.555, 5.945, 6.355, 0.05, "B");
    rounded(ink, v, 4.3, 5.64, 5.845, 6.27, 0.2, "S");
    ink.circle(v.p(5.6, 5.955), 0.04 * v.k, "S");
    v.rect(ink, 2.9, 7.745, 4.1, 8.245, "B");
    let c = v.p(3.5, 7.98);
    ink.op(&format!("q 1 0 0 0.62 {:.3} {:.3} cm", c.0, c.1));
    ink.circle((0.0, 0.0), 0.26 * v.k, "S");
    ink.restore();
    v.rect(ink, 2.555, 6.4, 2.755, 6.9, "B");
    let c = v.p(3.02, 6.65);
    ink.op(&format!("q 1 0 0 0.68 {:.3} {:.3} cm", c.0, c.1));
    ink.circle((0.0, 0.0), 0.27 * v.k, "B");
    ink.restore();
    v.rect(ink, 5.045, 7.345, 5.945, 8.245, "B");
    v.line(ink, (5.045, 7.345), (5.945, 8.245));
    v.line(ink, (5.045, 8.245), (5.945, 7.345));
    ink.circle(v.p(5.495, 7.795), 0.05 * v.k, "B");
    // Laundry: bench and tub, washer.
    v.rect(ink, 10.345, 9.555, 10.945, 11.865, "B");
    rounded(ink, v, 10.42, 10.3, 10.87, 10.95, 0.05, "S");
    v.rect(ink, 8.6, 9.6, 9.2, 10.2, "B");
    ink.circle(v.p(8.9, 9.9), 0.22 * v.k, "S");
    // Kitchen: the back bench with sink and cooktop, the fridge, the island.
    v.rect(ink, 11.955, 11.265, 15.3, 11.865, "B");
    v.rect(ink, 11.055, 11.165, 11.955, 11.865, "B");
    v.line(ink, (11.055, 11.165), (11.955, 11.865));
    v.line(ink, (11.055, 11.865), (11.955, 11.165));
    rounded(ink, v, 12.85, 11.35, 13.3, 11.78, 0.05, "S");
    rounded(ink, v, 13.35, 11.35, 13.8, 11.78, 0.05, "S");
    for (x, y) in [(14.3, 11.42), (14.7, 11.42), (14.3, 11.7), (14.7, 11.7)] {
        ink.circle(v.p(x, y), 0.09 * v.k, "S");
    }
    let (ix0, iy0, ix1, iy1) = ISLAND;
    v.rect(ink, ix0, iy0, ix1, iy1, "B");
    ink.dash(&[1.0, 0.8]);
    v.line(ink, (ix0, iy0 + 0.3), (ix1, iy0 + 0.3));
    ink.dash(&[]);
    for x in [12.4, 13.2, 14.0] {
        ink.circle(v.p(x, 6.3), 0.2 * v.k, "B");
    }
    // Dining: a table for six.
    for (x, y) in [(15.35, 8.35), (16.05, 8.35), (16.75, 8.35), (15.35, 9.85), (16.05, 9.85), (16.75, 9.85), (14.8, 9.1), (17.3, 9.1)] {
        rounded(ink, v, x - 0.22, y - 0.22, x + 0.22, y + 0.22, 0.06, "B");
    }
    v.rect(ink, 15.1, 8.65, 17.0, 9.55, "B");
    // Living: TV unit, sofa facing it, coffee table, rug.
    ink.dash(&[1.2, 0.8]);
    v.rect(ink, 12.4, 0.9, 14.4, 3.6, "S");
    ink.dash(&[]);
    v.rect(ink, 11.055, 1.0, 11.5, 3.0, "B");
    rounded(ink, v, 14.6, 0.7, 15.55, 3.8, 0.1, "B");
    v.line(ink, (15.3, 0.8), (15.3, 3.7));
    for y in [1.73, 2.77] {
        v.line(ink, (14.6, y), (15.3, y));
    }
    v.rect(ink, 12.9, 1.6, 13.9, 2.9, "B");
    // Study: desk, chair, shelves.
    v.rect(ink, 8.8, 0.135, 10.7, 0.835, "B");
    ink.circle(v.p(9.75, 1.25), 0.25 * v.k, "B");
    v.rect(ink, 10.645, 1.2, 10.945, 3.3, "B");
    for y in [1.9, 2.6] {
        v.line(ink, (10.645, y), (10.945, y));
    }
    // Garage: two cars.
    car(ink, v, 0.6, 0.55);
    car(ink, v, 3.35, 0.55);
}

fn plan_sheet(ink: &mut Ink) -> View {
    let v = View { ox: 190.0, oy: 175.0, k: 20.0 };
    let walls = walls();

    // Floor finishes first, so everything else lies over them.
    ink.pen(0.08, grey(0.62));
    ink.clip(&v.all(&OPEN_PLAN));
    v.hatch(ink, 8.5, 0.0, 18.0, 12.0, 0.0, 0.6);
    v.hatch(ink, 8.5, 0.0, 18.0, 12.0, 90.0, 0.6);
    ink.restore();
    for (x0, y0, x1, y1) in [(2.555, 5.555, 5.945, 8.245), (8.555, 9.555, 10.945, 11.865)] {
        ink.clip(&v.all(&rect_pts(x0, y0, x1, y1)));
        v.hatch(ink, x0, y0, x1, y1, 0.0, 0.3);
        v.hatch(ink, x0, y0, x1, y1, 90.0, 0.3);
        ink.restore();
    }
    for (x0, y0, x1, y1, step) in [(6.055, 0.135, 8.445, 11.865, 0.13), (8.555, 0.135, 10.945, 3.445, 0.13)] {
        ink.clip(&v.all(&rect_pts(x0, y0, x1, y1)));
        v.hatch(ink, x0, y0, x1, y1, 90.0, step);
        ink.restore();
    }
    // The deck: boards, and its edge.
    let (dx0, dy0, dx1, dy1) = DECK;
    ink.clip(&v.all(&rect_pts(dx0, dy0, dx1, dy1)));
    ink.pen(0.1, grey(0.45));
    v.hatch(ink, dx0, dy0, dx1, dy1, 0.0, 0.14);
    ink.restore();
    ink.pen(0.35, BLACK);
    v.rect(ink, dx0, dy0, dx1, dy1, "S");
    ink.pen(0.18, BLACK);
    ink.dash(&[3.0, 1.0, 0.5, 1.0]);
    v.rect(ink, dx0 + 0.3, dy0 + 0.3, dx1 - 0.3, dy1 - 0.1, "S");
    ink.dash(&[]);

    // The path to the front door and the driveway.
    ink.pen(0.18, grey(0.3));
    v.rect(ink, 6.5, -3.2, 7.9, -0.135, "S");
    v.rect(ink, 0.0, -3.2, 6.0, -0.135, "S");
    ink.pen(0.08, grey(0.55));
    for i in 1..6 {
        let y = -0.135 - i as f64 * 0.51;
        v.line(ink, (6.5, y), (7.9, y));
    }

    furniture(ink, v);
    draw_walls(ink, v, &walls);
    for &p in DOWNLIGHTS_OPEN.iter().chain(&DOWNLIGHTS_HALL) {
        downlight(ink, v, p);
    }
    // Smoke alarms.
    for (x, y) in [(7.3, 6.2), (3.6, 10.0)] {
        let c = v.p(x, y);
        ink.pen(0.18, BLACK);
        ink.fill(WHITE);
        ink.circle(c, 2.6, "B");
        ink.fill(BLACK);
        ink.text((c.0, c.1 - 0.9), 2.2, Font::Bold, Align::Centre, "SA");
    }

    let area = |x0: f64, y0: f64, x1: f64, y1: f64| Some((x1 - x0) * (y1 - y0));
    room(ink, v, (3.0, 3.3), "GARAGE", area(0.135, 0.135, 5.945, 5.445));
    room(ink, v, (1.6, 7.2), "WIR", area(0.135, 5.555, 2.445, 8.245));
    room(ink, v, (3.6, 7.0), "BATH", area(2.555, 5.555, 5.945, 8.245));
    room(ink, v, (1.5, 9.6), "BED 1", area(0.135, 8.355, 5.945, 11.865));
    room(ink, v, (7.3, 3.1), "HALL", area(6.055, 0.135, 8.445, 11.865));
    room(ink, v, (9.75, 2.3), "STUDY", area(8.555, 0.135, 10.945, 3.445));
    room(ink, v, (9.6, 11.0), "LDRY", area(8.555, 9.555, 10.945, 11.865));
    room(ink, v, (13.2, 4.5), "LIVING", None);
    room(ink, v, (13.6, 9.6), "KITCHEN", None);
    room(ink, v, (16.1, 10.6), "DINING", None);
    room(ink, v, (15.0, -1.9), "ALFRESCO", area(dx0, dy0, dx1, dy1));

    // Grids.
    ink.pen(0.13, grey(0.4));
    ink.dash(&[6.0, 1.2, 1.0, 1.2]);
    for x in [0.0, 6.0, 8.5, 18.0] {
        v.line(ink, (x, -4.6), (x, 14.6));
    }
    for y in [0.0, 5.5, 12.0] {
        v.line(ink, (-3.6, y), (21.6, y));
    }
    ink.dash(&[]);
    for (i, x) in [0.0, 6.0, 8.5, 18.0].into_iter().enumerate() {
        for y in [-4.85, 14.85] {
            let c = v.p(x, y);
            ink.pen(0.25, BLACK);
            ink.fill(WHITE);
            ink.circle(c, 4.5, "B");
            ink.fill(BLACK);
            ink.text((c.0, c.1 - 1.3), 3.6, Font::Bold, Align::Centre, &format!("{}", i + 1));
        }
    }
    for (i, y) in [0.0, 5.5, 12.0].into_iter().enumerate() {
        for x in [-3.85, 21.85] {
            let c = v.p(x, y);
            ink.pen(0.25, BLACK);
            ink.fill(WHITE);
            ink.circle(c, 4.5, "B");
            ink.fill(BLACK);
            ink.text((c.0, c.1 - 1.3), 3.6, Font::Bold, Align::Centre, ["A", "B", "C"][i]);
        }
    }

    // Dimensions.
    let south = v.p(0.0, -3.2).1;
    v.dims_x(ink, &[0.0, 6.0, 8.5, 11.0, 18.0], south - 10.0, south - 1.0);
    v.dims_x(ink, &[0.0, 18.0], south - 18.0, south - 1.0);
    let top = v.p(0.0, 12.0).1;
    v.dims_x(ink, &[0.0, 2.5, 6.0, 8.5, 11.0, 18.0], top + 18.0, top + 3.0);
    v.dims_x(ink, &[0.0, 18.0], top + 26.0, top + 3.0);
    let left = v.p(0.0, 0.0).0;
    v.dims_y(ink, &[0.0, 5.5, 8.3, 12.0], left - 20.0, left - 3.0);
    v.dims_y(ink, &[0.0, 12.0], left - 28.0, left - 3.0);
    let right = v.p(18.0, 0.0).0;
    v.dims_y(ink, &[0.0, 1.2, 4.2, 7.0, 10.0, 12.0], right + 20.0, right + 3.0);
    v.dims_y(ink, &[0.0, 12.0], right + 28.0, right + 3.0);

    // A section line through the living room and hall.
    ink.pen(0.5, BLACK);
    ink.dash(&[4.0, 1.5, 1.0, 1.5]);
    v.line(ink, (-2.5, 4.0), (-1.2, 4.0));
    v.line(ink, (19.2, 4.0), (20.5, 4.0));
    ink.dash(&[]);
    for x in [-2.5, 20.5] {
        let c = v.p(x, 4.0);
        ink.fill(BLACK);
        ink.poly(&[(c.0 - 2.2, c.1), (c.0 + 2.2, c.1), (c.0, c.1 + 4.0)], true, "f");
        ink.text((c.0, c.1 + 5.5), 3.2, Font::Bold, Align::Centre, "A");
        ink.text((c.0, c.1 - 4.5), 2.2, Font::Regular, Align::Centre, "A-501");
    }

    north(ink, (680.0, 540.0), 10.0);
    view_title(ink, (40.0, 40.0), "1", "GROUND FLOOR PLAN", "1:50");
    scale_bar(ink, (190.0, 36.0), v.k, 5);
    v
}

fn roof_sheet(ink: &mut Ink) -> View {
    let v = View { ox: 280.0, oy: 250.0, k: 10.0 };
    let (e0, e1) = ((-0.6, -0.6), (18.6, 12.6));
    let ridge = [(6.0, 6.0), (12.0, 6.0)];
    let planes: [(Vec<(f64, f64)>, f64); 4] = [
        (vec![(e0.0, e0.1), (e1.0, e0.1), ridge[1], ridge[0]], 90.0),
        (vec![(e0.0, e1.1), ridge[0], ridge[1], (e1.0, e1.1)], 90.0),
        (vec![(e0.0, e0.1), ridge[0], (e0.0, e1.1)], 0.0),
        (vec![(e1.0, e0.1), (e1.0, e1.1), ridge[1]], 0.0),
    ];
    for (pts, angle) in &planes {
        ink.clip(&v.all(pts));
        ink.pen(0.08, grey(0.5));
        v.hatch(ink, e0.0, e0.1, e1.0, e1.1, *angle, 0.2);
        ink.restore();
        ink.pen(0.35, BLACK);
        ink.poly(&v.all(pts), true, "S");
    }
    ink.pen(0.5, BLACK);
    v.rect(ink, e0.0, e0.1, e1.0, e1.1, "S");
    ink.pen(0.18, BLACK);
    v.rect(ink, e0.0 + 0.15, e0.1 + 0.15, e1.0 - 0.15, e1.1 - 0.15, "S");
    ink.dash(&[1.5, 1.0]);
    v.rect(ink, 0.0, 0.0, 18.0, 12.0, "S");
    v.rect(ink, DECK.0, DECK.1, DECK.2, DECK.3, "S");
    ink.dash(&[]);

    // Solar panels on the north face.
    for row in [7.2, 9.0] {
        for i in 0..6 {
            let x = 5.9 + i as f64 * 1.05;
            ink.fill(WHITE);
            ink.pen(0.25, BLACK);
            v.rect(ink, x, row, x + 1.0, row + 1.7, "B");
            ink.pen(0.06, grey(0.4));
            for c in 1..6 {
                let cx = x + c as f64 / 6.0;
                v.line(ink, (cx, row), (cx, row + 1.7));
            }
            for r in 1..10 {
                let ry = row + r as f64 * 0.17;
                v.line(ink, (x, ry), (x + 1.0, ry));
            }
        }
    }

    // Downpipes, and which way each face falls.
    for (x, y) in [(0.2, -0.45), (17.8, -0.45), (0.2, 12.45), (17.8, 12.45), (9.0, -0.45)] {
        let c = v.p(x, y);
        ink.pen(0.25, BLACK);
        ink.fill(WHITE);
        ink.circle(c, 1.1, "B");
        ink.fill(BLACK);
        ink.text((c.0 + 2.0, c.1 - 3.5), 2.2, Font::Regular, Align::Left, "DP");
    }
    for (at, dir) in [((9.0, 2.0), (0.0, -1.0)), ((15.0, 11.0), (0.0, 1.0)), ((1.8, 6.0), (-1.0, 0.0)), ((16.2, 6.0), (1.0, 0.0))] {
        let a = v.p(at.0 - dir.0 * 1.2, at.1 - dir.1 * 1.2);
        let b = v.p(at.0 + dir.0 * 1.2, at.1 + dir.1 * 1.2);
        ink.pen(0.25, BLACK);
        ink.line(a, b);
        ink.fill(BLACK);
        let (px, py) = (-dir.1, dir.0);
        ink.poly(&[b, (b.0 - dir.0 * 3.0 + px * 1.2, b.1 - dir.1 * 3.0 + py * 1.2), (b.0 - dir.0 * 3.0 - px * 1.2, b.1 - dir.1 * 3.0 - py * 1.2)], true, "f");
        let t = v.p(at.0 + dir.1 * 0.8, at.1 - dir.0 * 0.8);
        ink.fill(WHITE);
        ink.rect(t.0 - 7.0, t.1 - 1.3, 14.0, 4.0, "f");
        ink.fill(BLACK);
        ink.text((t.0, t.1), 2.5, Font::Regular, Align::Centre, "FALL 22.5°");
    }
    ink.fill(WHITE);
    let t = v.p(9.0, 6.4);
    ink.rect(t.0 - 7.5, t.1 - 0.8, 15.0, 3.8, "f");
    ink.fill(BLACK);
    ink.text(t, 2.5, Font::Bold, Align::Centre, "RIDGE");
    let t = v.p(9.0, 11.3);
    ink.fill(WHITE);
    ink.rect(t.0 - 18.0, t.1 - 0.9, 36.0, 4.0, "f");
    ink.fill(BLACK);
    ink.text(t, 2.5, Font::Regular, Align::Centre, "12 × 440 W SOLAR PANELS");

    let bottom = v.p(0.0, -0.6).1;
    v.dims_x(ink, &[-0.6, 0.0, 18.0, 18.6], bottom - 10.0, bottom - 2.0);
    v.dims_x(ink, &[-0.6, 18.6], bottom - 18.0, bottom - 2.0);
    let left = v.p(-0.6, 0.0).0;
    v.dims_y(ink, &[-0.6, 0.0, 12.0, 12.6], left - 10.0, left - 2.0);
    v.dims_y(ink, &[-0.6, 12.6], left - 18.0, left - 2.0);

    // The roofing specification, beside the plan.
    ink.fill(BLACK);
    ink.text((520.0, 400.0), 3.5, Font::Bold, Align::Left, "ROOF");
    let spec = [
        "0.42 BMT metal roof sheeting on 40 × 35 steel battens at 900 centres.",
        "Roof blanket under the sheeting, foil face down, lapped 150.",
        "Hip roof at 22.5°, 600 eaves all round.",
        "Quad gutter to fascia, falling 1:500 to 90 dia. downpipes.",
        "Downpipes to the rainwater tank; overflow to the legal point of discharge.",
        "Solar panels on rails fixed through the sheeting crests to the battens.",
    ];
    let mut y = 393.0;
    for (i, line) in spec.iter().enumerate() {
        ink.text((520.0, y), 2.8, Font::Regular, Align::Left, &format!("{}.", i + 1));
        let wrapped = wrap(line, 2.8, Font::Regular, 180.0);
        y = ink.lines((527.0, y), 2.8, 4.2, Font::Regular, &wrapped) - 2.0;
    }

    north(ink, (680.0, 540.0), 10.0);
    view_title(ink, (40.0, 40.0), "1", "ROOF PLAN", "1:100");
    scale_bar(ink, (190.0, 36.0), v.k, 10);
    v
}

/// Heights above the natural ground, in metres.
const FFL: f64 = 0.3;
const SILL: f64 = 1.2;
const HEAD: f64 = 2.7;
const PLATE: f64 = 3.0;
const EAVE: f64 = 3.2;
const RIDGE: f64 = EAVE + 6.6 * 0.414_213_562;
const BRICK_TOP: f64 = 1.2;

/// An opening as seen on an elevation: from x0 to x1, and what it is.
fn elevation_openings(walls: &[Wall], face: char) -> Vec<(f64, f64, Kind)> {
    let wall = walls.iter().find(|w| w.face == Some(face)).expect("a wall on every face");
    let len = wall.len();
    wall.openings
        .iter()
        .map(|op| match face {
            'N' | 'W' => (len - op.b, len - op.a, op.kind),
            _ => (op.a, op.b, op.kind),
        })
        .collect()
}

fn opening_box(x0: f64, x1: f64, kind: Kind) -> (f64, f64, f64, f64) {
    match kind {
        Kind::Window => (x0, SILL, x1, HEAD),
        _ => (x0, FFL, x1, HEAD),
    }
}

fn elevation(ink: &mut Ink, v: View, len: f64, long: bool, openings: &[(f64, f64, Kind)]) {
    // The ground.
    ink.clip(&v.all(&rect_pts(-1.5, -0.45, len + 1.5, 0.0)));
    ink.pen(0.1, grey(0.45));
    v.hatch(ink, -1.5, -0.45, len + 1.5, 0.0, 45.0, 0.12);
    ink.restore();

    // Brick below, weatherboards above.
    ink.pen(0.08, grey(0.35));
    let mut course = 0;
    let mut y = FFL;
    while y < BRICK_TOP - 0.01 {
        let top = (y + 0.086).min(BRICK_TOP);
        v.line(ink, (0.0, top), (len, top));
        let mut x = if course % 2 == 0 { 0.23 } else { 0.115 };
        while x < len {
            v.line(ink, (x, y), (x, top));
            x += 0.23;
        }
        y = top;
        course += 1;
    }
    let mut y = BRICK_TOP + 0.18;
    while y < PLATE {
        v.line(ink, (0.0, y), (len, y));
        y += 0.18;
    }
    ink.pen(0.25, BLACK);
    v.line(ink, (0.0, BRICK_TOP), (len, BRICK_TOP));
    v.rect(ink, 0.0, 0.0, len, FFL, "S");

    for &(x0, x1, kind) in openings {
        let (bx0, by0, bx1, by1) = opening_box(x0, x1, kind);
        ink.fill(WHITE);
        ink.pen(0.3, BLACK);
        v.rect(ink, bx0, by0, bx1, by1, "B");
        ink.pen(0.13, BLACK);
        v.rect(ink, bx0 + 0.05, by0 + 0.05, bx1 - 0.05, by1 - 0.05, "S");
        let mid = (bx0 + bx1) / 2.0;
        match kind {
            Kind::Window => {
                if bx1 - bx0 > 1.6 {
                    v.line(ink, (mid, by0 + 0.05), (mid, by1 - 0.05));
                }
                ink.dash(&[1.0, 0.8]);
                let panes = if bx1 - bx0 > 1.6 { vec![(bx0, mid), (mid, bx1)] } else { vec![(bx0, bx1)] };
                for (a, b) in panes {
                    ink.poly(&v.all(&[(a + 0.1, by1 - 0.1), ((a + b) / 2.0, by0 + 0.1), (b - 0.1, by1 - 0.1)]), false, "S");
                }
                ink.dash(&[]);
                ink.pen(0.25, BLACK);
                v.rect(ink, bx0 - 0.06, by0 - 0.05, bx1 + 0.06, by0, "S");
            }
            Kind::Door { .. } => {
                v.rect(ink, bx0 + 0.18, by0 + 0.25, bx1 - 0.18, by1 - 0.35, "S");
                ink.circle(v.p(bx1 - 0.15, by0 + 1.05), 0.04 * v.k, "S");
            }
            Kind::Slider => {
                v.line(ink, (mid, by0 + 0.05), (mid, by1 - 0.05));
                let y = by0 + 1.1;
                v.line(ink, (bx0 + 0.3, y), (mid - 0.3, y));
                ink.fill(BLACK);
                let tip = v.p(mid - 0.3, y);
                ink.poly(&[tip, (tip.0 - 2.4, tip.1 + 1.0), (tip.0 - 2.4, tip.1 - 1.0)], true, "f");
            }
            Kind::Garage => {
                let mut y = by0 + 0.45;
                while y < by1 - 0.1 {
                    v.line(ink, (bx0 + 0.05, y), (bx1 - 0.05, y));
                    y += 0.4;
                }
            }
            Kind::Void => {}
        }
    }

    ink.pen(0.35, BLACK);
    v.rect(ink, 0.0, 0.0, len, PLATE, "S");

    // Downpipes.
    for x in [0.25, len - 0.34] {
        ink.fill(WHITE);
        ink.pen(0.18, BLACK);
        v.rect(ink, x, FFL, x + 0.09, EAVE - 0.05, "B");
    }

    // The roof, its sheeting, and the gutter along its foot.
    let roof = if long {
        vec![(-0.6, EAVE), (len + 0.6, EAVE), (len / 2.0 + 3.0, RIDGE), (len / 2.0 - 3.0, RIDGE)]
    } else {
        vec![(-0.6, EAVE), (len + 0.6, EAVE), (len / 2.0, RIDGE)]
    };
    ink.fill(WHITE);
    ink.pen(0.35, BLACK);
    ink.poly(&v.all(&roof), true, "B");
    ink.clip(&v.all(&roof));
    ink.pen(0.07, grey(0.4));
    v.hatch(ink, -0.6, EAVE, len + 0.6, RIDGE, 90.0, 0.19);
    ink.restore();
    ink.pen(0.35, BLACK);
    ink.poly(&v.all(&roof), true, "S");
    ink.fill(WHITE);
    v.rect(ink, -0.65, PLATE, len + 0.65, EAVE, "B");
    ink.pen(0.13, BLACK);
    v.line(ink, (-0.65, EAVE - 0.05), (len + 0.65, EAVE - 0.05));

    ink.pen(0.6, BLACK);
    v.line(ink, (-1.5, 0.0), (len + 1.5, 0.0));

    // Levels.
    for (name, h) in [("RIDGE", RIDGE), ("WALL PLATE", PLATE), ("FFL", FFL), ("NGL", 0.0)] {
        let a = v.p(len + 0.9, h);
        let b = v.p(len + 4.2, h);
        ink.pen(0.13, BLACK);
        ink.dash(&[2.0, 1.0]);
        ink.line(a, b);
        ink.dash(&[]);
        ink.fill(BLACK);
        ink.poly(&[b, (b.0 - 1.6, b.1 + 2.4), (b.0 + 1.6, b.1 + 2.4)], true, "f");
        ink.text((b.0 + 3.0, b.1 + 2.2), 2.5, Font::Bold, Align::Left, name);
        ink.text((b.0 + 3.0, b.1 - 1.4), 2.5, Font::Regular, Align::Left, &format!("RL {:.3}", 10.0 + h));
    }
}

fn elevation_sheet(ink: &mut Ink, walls: &[Wall], faces: [(char, &str); 2], long: bool) -> [View; 2] {
    let len = if long { 18.0 } else { 12.0 };
    let ox = 350.0 - len * 10.0;
    let views = [View { ox, oy: 345.0, k: 20.0 }, View { ox, oy: 105.0, k: 20.0 }];
    for (i, (face, name)) in faces.iter().enumerate() {
        let v = views[i];
        elevation(ink, v, len, long, &elevation_openings(walls, *face));
        view_title(ink, (v.ox - 12.0, v.oy - 24.0), &format!("{}", i + 1), name, "1:50");
    }
    scale_bar(ink, (560.0, 36.0), 20.0, 5);
    views
}

fn schedules_sheet(ink: &mut Ink, walls: &[Wall]) {
    let mut openings: Vec<Opening> = walls.iter().flat_map(|w| w.openings.iter().copied()).filter(|o| !o.mark.is_empty()).collect();
    openings.sort_by(|a, b| a.mark.cmp(b.mark));
    let mm = |m: f64| format!("{}", (m * 1000.0).round());

    ink.fill(BLACK);
    ink.text((30.0, 560.0), 6.0, Font::Bold, Align::Left, "DOOR SCHEDULE");
    let doors: Vec<Vec<String>> = openings
        .iter()
        .filter(|o| o.kind != Kind::Window)
        .map(|o| {
            let (kind, frame, hardware, height) = match (o.kind, o.room) {
                (Kind::Garage, _) => ("Panel lift, insulated", "Steel", "Motor, two remotes", 2.4),
                (Kind::Slider, _) => ("Sliding, two panels", "Aluminium", "Lockable handle set", 2.4),
                (_, "Entry") => ("Hinged, solid timber", "Timber", "Entrance set, deadbolt", 2.04),
                (_, "Laundry") if o.mark == "D08" => ("Hinged, glazed", "Aluminium", "Entrance set", 2.04),
                (_, "Bath" | "Bed 1") => ("Hinged, hollow core", "Timber", "Lever set, privacy", 2.04),
                _ => ("Hinged, hollow core", "Timber", "Lever set, passage", 2.04),
            };
            vec![o.mark.into(), o.room.into(), mm(o.b - o.a), mm(height), kind.into(), frame.into(), hardware.into()]
        })
        .collect();
    let cols = [("MARK", 22.0), ("ROOM", 40.0), ("WIDTH", 24.0), ("HEIGHT", 24.0), ("TYPE", 70.0), ("FRAME", 40.0), ("HARDWARE", 70.0)];
    let bottom = table(ink, 30.0, 550.0, &cols, &doors);

    ink.text((30.0, bottom - 20.0), 6.0, Font::Bold, Align::Left, "WINDOW SCHEDULE");
    let windows: Vec<Vec<String>> = openings
        .iter()
        .filter(|o| o.kind == Kind::Window)
        .map(|o| {
            let kind = if o.b - o.a > 1.6 { "Awning pair" } else { "Awning" };
            let glass = if matches!(o.room, "Living" | "Dining") { "Low-E double glazed" } else { "6.38 laminated" };
            vec![o.mark.into(), o.room.into(), mm(o.b - o.a), mm(HEAD - SILL), mm(SILL - FFL), kind.into(), glass.into()]
        })
        .collect();
    let cols = [("MARK", 22.0), ("ROOM", 40.0), ("WIDTH", 24.0), ("HEIGHT", 24.0), ("SILL", 24.0), ("TYPE", 50.0), ("GLAZING", 60.0)];
    let bottom = table(ink, 30.0, bottom - 30.0, &cols, &windows);

    ink.text((30.0, bottom - 20.0), 6.0, Font::Bold, Align::Left, "FINISHES SCHEDULE");
    let finishes = [
        ["Garage", "Sealed concrete", "Painted brick", "None", "None"],
        ["Hall", "Engineered oak", "Paint, low sheen", "Paint, flat", "68 × 18 pine"],
        ["Bed 1, WIR", "Carpet on underlay", "Paint, low sheen", "Paint, flat", "68 × 18 pine"],
        ["Bath", "Porcelain tiles, P3", "Tiles to 2100", "Paint, flat, mould-resistant", "Tiled cove"],
        ["Laundry", "Porcelain tiles, P3", "Tiles to 1200", "Paint, flat", "Tiled cove"],
        ["Study", "Engineered oak", "Paint, low sheen", "Paint, flat", "68 × 18 pine"],
        ["Kitchen, dining, living", "Porcelain tiles, 600 × 600", "Paint, low sheen", "Paint, flat", "68 × 18 pine"],
        ["Alfresco", "Hardwood decking", "Weatherboard", "Fibre cement lining", "None"],
    ];
    let rows: Vec<Vec<String>> = finishes.iter().map(|r| r.iter().map(|s| (*s).to_owned()).collect()).collect();
    let cols = [("ROOM", 60.0), ("FLOOR", 70.0), ("WALLS", 50.0), ("CEILING", 60.0), ("SKIRTING", 50.0)];
    table(ink, 30.0, bottom - 30.0, &cols, &rows);
}

// ---------------------------------------------------------------------------
// Details

/// A break line from `a` to `b`, with a zigzag in the middle.
fn break_line(ink: &mut Ink, v: View, a: (f64, f64), b: (f64, f64)) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    let m = (a.0 + dx / 2.0, a.1 + dy / 2.0);
    let z = 0.025;
    let pts = [
        a,
        (m.0 - ux * z, m.1 - uy * z),
        (m.0 - ux * z / 2.0 + nx * z * 1.5, m.1 - uy * z / 2.0 + ny * z * 1.5),
        (m.0 + ux * z / 2.0 - nx * z * 1.5, m.1 + uy * z / 2.0 - ny * z * 1.5),
        (m.0 + ux * z, m.1 + uy * z),
        b,
    ];
    ink.pen(0.18, BLACK);
    ink.poly(&v.all(&pts), false, "S");
}

/// A timber section: a box with a cross.
fn timber(ink: &mut Ink, v: View, pts: [(f64, f64); 4]) {
    ink.fill(WHITE);
    ink.pen(0.25, BLACK);
    ink.poly(&v.all(&pts), true, "B");
    ink.pen(0.1, BLACK);
    v.line(ink, pts[0], pts[2]);
    v.line(ink, pts[1], pts[3]);
}

/// Insulation batts across `x0..x1` from `y0` up to `y1`.
fn batts(ink: &mut Ink, v: View, x0: f64, x1: f64, y0: f64, y1: f64) {
    let mut pts = Vec::new();
    let step = (x1 - x0) * 0.35;
    let mut y = y0;
    let mut left = true;
    while y <= y1 {
        pts.push((if left { x0 + 0.004 } else { x1 - 0.004 }, y));
        y += step;
        left = !left;
    }
    ink.pen(0.13, grey(0.25));
    ink.poly(&v.all(&pts), false, "S");
}

fn brick(ink: &mut Ink, v: View, x0: f64, y0: f64, x1: f64, y1: f64) {
    ink.fill(WHITE);
    ink.pen(0.35, BLACK);
    v.rect(ink, x0, y0, x1, y1, "B");
    ink.clip(&v.all(&rect_pts(x0, y0, x1, y1)));
    ink.pen(0.1, grey(0.3));
    v.hatch(ink, x0, y0, x1, y1, 45.0, 0.012);
    ink.restore();
    ink.pen(0.35, BLACK);
    v.rect(ink, x0, y0, x1, y1, "S");
}

fn slab_detail(ink: &mut Ink, v: View) {
    let concrete = [(-0.02, -0.4), (0.4, -0.4), (0.55, 0.1), (0.8, 0.1), (0.8, 0.2), (0.155, 0.2), (0.155, 0.05), (-0.02, 0.05)];
    let fill = [(-0.4, -0.1), (-0.02, -0.1), (-0.02, -0.4), (0.4, -0.4), (0.55, 0.1), (0.8, 0.1), (0.8, -0.55), (-0.4, -0.55)];

    // Ground under and beside the slab.
    ink.clip(&v.all(&fill));
    ink.pen(0.1, grey(0.45));
    v.hatch(ink, -0.4, -0.55, 0.8, 0.1, 45.0, 0.03);
    v.hatch(ink, -0.4, -0.55, 0.8, 0.1, 135.0, 0.06);
    ink.restore();
    ink.pen(0.5, BLACK);
    v.line(ink, (-0.4, -0.1), (-0.02, -0.1));

    // Concrete: its outline, stipple and stones.
    ink.fill(WHITE);
    ink.pen(0.5, BLACK);
    ink.poly(&v.all(&concrete), true, "B");
    ink.clip(&v.all(&concrete));
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    ink.fill(grey(0.3));
    for _ in 0..9000 {
        let p = (-0.02 + rng.next() * 0.82, -0.4 + rng.next() * 0.6);
        if inside(&concrete, p) {
            ink.circle(v.p(p.0, p.1), 0.12 + rng.next() * 0.18, "f");
        }
    }
    ink.pen(0.13, grey(0.3));
    for _ in 0..700 {
        let p = (-0.02 + rng.next() * 0.82, -0.4 + rng.next() * 0.6);
        if inside(&concrete, p) {
            let r = 0.004 + rng.next() * 0.006;
            let a = rng.next() * PI;
            let tri: Vec<(f64, f64)> = (0..3).map(|i| {
                let t = a + i as f64 * 2.1 + rng.next() * 0.4;
                (p.0 + r * t.cos(), p.1 + r * t.sin())
            }).collect();
            ink.poly(&v.all(&tri), true, "S");
        }
    }
    ink.restore();
    ink.pen(0.5, BLACK);
    ink.poly(&v.all(&concrete), true, "S");

    // Reinforcement and the membrane under it all.
    ink.fill(BLACK);
    for (x, y) in [(0.05, -0.34), (0.33, -0.34), (0.22, 0.13), (0.34, 0.13)] {
        ink.circle(v.p(x, y), 0.006 * v.k, "f");
    }
    ink.pen(0.25, BLACK);
    ink.dash(&[2.0, 1.0]);
    v.line(ink, (0.2, 0.165), (0.8, 0.165));
    ink.dash(&[]);
    for x in [0.3, 0.5, 0.7] {
        ink.circle(v.p(x, 0.165), 0.004 * v.k, "f");
    }
    ink.pen(0.35, BLACK);
    ink.dash(&[3.0, 1.0]);
    ink.poly(&v.all(&[(-0.035, 0.05), (-0.035, -0.415), (0.41, -0.415), (0.56, 0.085), (0.8, 0.085)]), false, "S");
    ink.dash(&[]);

    // The wall: brick, cavity, frame, lining.
    brick(ink, v, 0.0, 0.05, 0.11, 0.8);
    ink.pen(0.1, grey(0.2));
    for i in 0..8 {
        let y = 0.05 + 0.086 * i as f64 + 0.04;
        v.line(ink, (0.11, y), (0.155, y + 0.005));
    }
    timber(ink, v, [(0.16, 0.2), (0.25, 0.2), (0.25, 0.245), (0.16, 0.245)]);
    batts(ink, v, 0.16, 0.25, 0.245, 0.8);
    ink.pen(0.13, BLACK);
    ink.dash(&[1.5, 0.8]);
    v.line(ink, (0.16, 0.245), (0.16, 0.8));
    v.line(ink, (0.25, 0.245), (0.25, 0.8));
    ink.dash(&[]);
    ink.pen(0.25, BLACK);
    v.line(ink, (0.158, 0.245), (0.158, 0.8));
    ink.fill(grey(0.85));
    ink.pen(0.25, BLACK);
    v.rect(ink, 0.25, 0.212, 0.263, 0.8, "B");
    ink.fill(WHITE);
    ink.poly(&v.all(&[(0.263, 0.212), (0.281, 0.212), (0.281, 0.27), (0.263, 0.28)]), true, "B");
    ink.fill(grey(0.6));
    v.rect(ink, 0.263, 0.2, 0.8, 0.21, "B");
    ink.pen(0.1, WHITE);
    for x in [0.45, 0.75] {
        v.line(ink, (x, 0.2), (x, 0.21));
    }
    // The damp-proof course, under the brick and up the frame.
    ink.pen(0.45, BLACK);
    ink.poly(&v.all(&[(0.0, 0.14), (0.14, 0.14), (0.14, 0.2), (0.155, 0.3)]), false, "S");

    break_line(ink, v, (-0.05, 0.8), (0.3, 0.8));
    break_line(ink, v, (0.8, 0.22), (0.8, -0.55));
    break_line(ink, v, (-0.4, -0.55), (0.8, -0.55));

    let dims = View { ..v };
    dims.dims_y(ink, &[-0.4, 0.05, 0.2], v.p(-0.02, 0.0).0 - 30.0, v.p(-0.02, 0.0).0 - 2.0);
    dims.dims_x(ink, &[0.0, 0.11, 0.16, 0.25], v.p(0.0, 0.8).1 + 12.0, v.p(0.0, 0.8).1 + 2.0);
}

fn eaves_detail(ink: &mut Ink, v: View) {
    let t = 22.5f64.to_radians().tan();
    let deep = 0.14 / 22.5f64.to_radians().cos();
    let yb = |x: f64| (x - 0.16) * t;
    let (x0, x1) = (-0.62, 0.72);

    brick(ink, v, 0.0, -0.65, 0.11, -0.36);
    batts(ink, v, 0.16, 0.25, -0.65, -0.09);
    ink.pen(0.13, BLACK);
    ink.dash(&[1.5, 0.8]);
    v.line(ink, (0.16, -0.65), (0.16, -0.09));
    v.line(ink, (0.25, -0.65), (0.25, -0.09));
    ink.dash(&[]);
    ink.pen(0.25, BLACK);
    v.line(ink, (0.158, -0.65), (0.158, -0.09));
    timber(ink, v, [(0.16, -0.09), (0.25, -0.09), (0.25, -0.045), (0.16, -0.045)]);
    timber(ink, v, [(0.16, -0.045), (0.25, -0.045), (0.25, 0.0), (0.16, 0.0)]);

    // Ceiling joist, lining and cornice.
    ink.fill(WHITE);
    ink.pen(0.25, BLACK);
    v.rect(ink, 0.25, 0.0, x1, 0.14, "B");
    ink.pen(0.08, grey(0.4));
    for y in [0.03, 0.07, 0.11] {
        v.line(ink, (0.27, y), (x1, y + 0.004));
    }
    ink.fill(grey(0.85));
    ink.pen(0.25, BLACK);
    v.rect(ink, 0.263, -0.013, x1, 0.0, "B");
    v.rect(ink, 0.25, -0.65, 0.263, -0.013, "B");
    ink.pen(0.25, BLACK);
    ink.arc(v.p(0.318, -0.068), 0.055 * v.k, PI / 2.0, PI);
    batts(ink, v, 0.3, 0.72, 0.14, 0.28);

    // Rafter, battens, blanket and sheeting.
    let rafter = [(x0, yb(x0)), (x1, yb(x1)), (x1, yb(x1) + deep), (x0, yb(x0) + deep)];
    ink.fill(WHITE);
    ink.pen(0.3, BLACK);
    ink.poly(&v.all(&rafter), true, "B");
    ink.pen(0.08, grey(0.4));
    for f in [0.3, 0.55, 0.8] {
        v.line(ink, (x0 + 0.02, yb(x0) + deep * f), (x1, yb(x1) + deep * f));
    }
    let (c, s) = (22.5f64.to_radians().cos(), 22.5f64.to_radians().sin());
    let top = |x: f64| (x, yb(x) + deep);
    for xc in [-0.42, 0.45] {
        let b = top(xc);
        let (w, h) = (0.05, 0.035);
        let p = |u: f64, n: f64| (b.0 + c * u - s * n, b.1 + s * u + c * n);
        timber(ink, v, [p(-w / 2.0, 0.0), p(w / 2.0, 0.0), p(w / 2.0, h), p(-w / 2.0, h)]);
    }
    let off = |x: f64, n: f64| (x - s * n, yb(x) + deep + c * n);
    ink.pen(0.35, BLACK);
    v.line(ink, off(x0 - 0.08, 0.037), off(x1, 0.037));
    v.line(ink, off(x0 - 0.08, 0.042), off(x1, 0.042));
    let wave: Vec<(f64, f64)> = (0..=160).map(|i| {
        let x = x0 + (x1 - x0) * i as f64 / 160.0;
        off(x, 0.03 + 0.003 * (i as f64 * 1.3).sin())
    }).collect();
    ink.pen(0.13, grey(0.3));
    ink.poly(&v.all(&wave), false, "S");

    // Fascia, gutter and soffit.
    let fy0 = yb(x0) - 0.04;
    let fy1 = yb(x0) + deep;
    ink.fill(WHITE);
    ink.pen(0.3, BLACK);
    v.rect(ink, x0 - 0.025, fy0, x0, fy1, "B");
    let g = x0 - 0.025;
    ink.pen(0.35, BLACK);
    ink.poly(&v.all(&[(g, fy1 - 0.02), (g, fy1 - 0.13), (g - 0.1, fy1 - 0.13), (g - 0.125, fy1 - 0.02), (g - 0.135, fy1 - 0.01)]), false, "S");
    ink.fill(grey(0.85));
    ink.pen(0.25, BLACK);
    v.rect(ink, x0, fy0 + 0.01, 0.0, fy0 + 0.016, "B");
    timber(ink, v, [(-0.045, fy0 + 0.016), (0.0, fy0 + 0.016), (0.0, fy0 + 0.06), (-0.045, fy0 + 0.06)]);

    break_line(ink, v, (x1, -0.03), (x1, yb(x1) + deep + 0.06));
    break_line(ink, v, (-0.02, -0.65), (0.3, -0.65));
}

fn details_sheet(ink: &mut Ink) -> [View; 2] {
    let one = View { ox: 110.0, oy: 300.0, k: 200.0 };
    slab_detail(ink, one);
    let notes_one = [
        ((0.055, 0.62), "110 face brick veneer, 10 mortar joints, raked"),
        ((0.158, 0.7), "Vapour-permeable wall wrap"),
        ((0.2, 0.55), "90 × 45 MGP10 studs at 450 centres, R2.5 insulation batts"),
        ((0.256, 0.45), "10 plasterboard, painted"),
        ((0.13, 0.4), "50 cavity, galvanised brick ties at 600 centres"),
        ((0.272, 0.25), "68 × 18 pine skirting"),
        ((0.5, 0.205), "Porcelain floor tiles on adhesive"),
        ((0.14, 0.165), "Damp-proof course, turned up the frame"),
        ((0.65, 0.15), "100 slab, SL82 mesh, 25 top cover"),
        ((0.15, -0.2), "400 deep edge beam, 4-N12 bars"),
        ((0.5, -0.12), "0.2 polyethylene vapour barrier, lapped and taped"),
        ((-0.25, -0.1), "Finished ground, falling 50 in the first 1000"),
        ((0.62, -0.42), "Compacted fill"),
    ];
    for (i, (target, _)) in notes_one.iter().enumerate() {
        let y = 0.75 - i as f64 * 0.105;
        keynote(ink, one.p(target.0, target.1), one.p(0.98, y), i + 1);
    }
    let mut y = 150.0;
    for (i, (_, text)) in notes_one.iter().enumerate() {
        let col = if i < 7 { 30.0 } else { 185.0 };
        if i == 7 {
            y = 150.0;
        }
        ink.fill(BLACK);
        ink.text((col, y), 2.8, Font::Bold, Align::Left, &format!("{}", i + 1));
        let lines = wrap(text, 2.8, Font::Regular, 140.0);
        y = ink.lines((col + 7.0, y), 2.8, 4.0, Font::Regular, &lines) - 2.5;
    }
    view_title(ink, (30.0, 172.0), "1", "SLAB EDGE, BRICK VENEER", "1:5");

    let two = View { ox: 565.0, oy: 340.0, k: 200.0 };
    eaves_detail(ink, two);
    let notes_two = [
        ((0.55, 0.37), "0.42 BMT metal roof sheeting"),
        ((0.3, 0.3), "Roof blanket, foil face down"),
        ((0.45, 0.33), "40 × 35 steel battens at 900 centres"),
        ((0.05, 0.08), "140 × 45 MGP10 rafters at 600 centres, 22.5° pitch"),
        ((-0.63, -0.25), "Timber fascia, quad gutter"),
        ((-0.35, -0.35), "4.5 fibre cement soffit lining"),
        ((0.2, -0.03), "Two 90 × 45 top plates"),
        ((0.5, 0.2), "Ceiling joists, R4.0 batts over"),
        ((0.5, -0.006), "10 plasterboard ceiling, 55 cove cornice"),
        ((0.055, -0.5), "110 brick veneer, tied to the frame"),
        ((0.2, -0.4), "R2.5 wall batts, wall wrap outside the frame"),
    ];
    for (i, (target, _)) in notes_two.iter().enumerate() {
        let bubble = if i < 4 { two.p(-0.55 + i as f64 * 0.3, 0.58) } else { two.p(-0.92, 0.1 - (i - 4) as f64 * 0.11) };
        keynote(ink, two.p(target.0, target.1), bubble, i + 1);
    }
    let mut y = 150.0;
    for (i, (_, text)) in notes_two.iter().enumerate() {
        let col = if i < 6 { 390.0 } else { 550.0 };
        if i == 6 {
            y = 150.0;
        }
        ink.fill(BLACK);
        ink.text((col, y), 2.8, Font::Bold, Align::Left, &format!("{}", i + 1));
        let lines = wrap(text, 2.8, Font::Regular, 140.0);
        y = ink.lines((col + 7.0, y), 2.8, 4.0, Font::Regular, &lines) - 2.5;
    }
    view_title(ink, (390.0, 172.0), "2", "EAVES", "1:5");
    [one, two]
}

// ---------------------------------------------------------------------------
// General notes

const NOTES: &[(&str, &[&str])] = &[
    ("GENERAL", &[
        "These drawings are to be read with the specification, the engineer's drawings and all other consultants' documents. Report any discrepancy to the architect before proceeding.",
        "Do not scale from drawings. Use figured dimensions only. Verify all dimensions on site before starting work or making shop drawings.",
        "All work is to comply with the National Construction Code and the relevant Australian Standards current at the time of construction.",
        "The builder is responsible for the stability of the works during construction, including temporary propping and bracing.",
        "Dimensions are in millimetres and levels in metres unless noted otherwise.",
    ]),
    ("SITE", &[
        "Set out from the survey pegs. Confirm boundary offsets with a registered surveyor before excavation.",
        "Grade the finished ground away from the building, falling 50 mm over the first 1 m.",
        "Pipe stormwater from the downpipes to the rainwater tank, with the overflow to the legal point of discharge.",
    ]),
    ("CONCRETE", &[
        "Slab and footings to the engineer's details. Concrete N25 minimum, 80 mm slump, 20 mm aggregate.",
        "Lay a 0.2 mm polyethylene vapour barrier under the slab, lapped 200 mm and taped at every joint and penetration.",
        "Provide 40 mm cover to reinforcement against the ground and 25 mm elsewhere.",
    ]),
    ("TIMBER FRAMING", &[
        "Wall frames 90 × 45 MGP10 studs at 450 mm centres with double top plates, to AS 1684.",
        "Brace walls and tie down the roof for wind classification N2.",
        "Fit R2.5 insulation batts to all external walls and R4.0 batts to the ceiling, with a vapour-permeable wall wrap behind the cladding.",
    ]),
    ("WET AREAS", &[
        "Waterproof shower recesses, bath surrounds and laundry floors to AS 3740, turning the membrane up 150 mm at the walls.",
        "Floor tiles in wet areas to have a P3 slip resistance rating or better, laid to fall to the floor waste.",
    ]),
    ("ELECTRICAL", &[
        "LED downlights to be IC-4 rated where insulation covers them. Keep them clear of ceiling joists, set out as shown on the plan.",
        "Smoke alarms to be hard-wired and interconnected, in the hall and in every bedroom.",
    ]),
    ("FINISHES", &[
        "Paint all plasterboard with one sealer coat and two finish coats: low sheen to walls, flat to ceilings.",
        "Skirtings 68 × 18 pine, primed and painted. Set out the kitchen island and joinery on site with the joiner.",
    ]),
];

fn notes_sheet(ink: &mut Ink, sheets: &[Sheet]) {
    ink.fill(BLACK);
    ink.text((30.0, 560.0), 8.0, Font::Bold, Align::Left, "GENERAL NOTES");
    let size = 3.6;
    let leading = 5.3;
    let width = 190.0;
    let columns = [30.0, 245.0];
    let mut col = 0;
    let mut y = 540.0;
    let mut n = 1;
    for (heading, notes) in NOTES {
        let needed = 9.0 + notes.iter().map(|s| wrap(s, size, Font::Regular, width - 9.0).len() as f64 * leading + 2.5).sum::<f64>();
        if y - needed < 40.0 && col + 1 < columns.len() {
            col += 1;
            y = 540.0;
        }
        let x = columns[col];
        ink.text((x, y), 4.2, Font::Bold, Align::Left, heading);
        y -= 7.5;
        for note in *notes {
            ink.text((x, y), size, Font::Regular, Align::Left, &format!("{n}."));
            let lines = wrap(note, size, Font::Regular, width - 9.0);
            y = ink.lines((x + 9.0, y), size, leading, Font::Regular, &lines) - 2.5;
            n += 1;
        }
        y -= 5.0;
    }

    ink.text((470.0, 560.0), 8.0, Font::Bold, Align::Left, "DRAWING LIST");
    let rows: Vec<Vec<String>> = sheets.iter().map(|s| vec![s.number.into(), s.title.join(" "), s.scale.into(), "C".into()]).collect();
    let bottom = table(ink, 470.0, 550.0, &[("SHEET", 25.0), ("TITLE", 115.0), ("SCALE", 55.0), ("REV", 20.0)], &rows);

    ink.text((470.0, bottom - 20.0), 8.0, Font::Bold, Align::Left, "LEGEND");
    let v = View { ox: 478.0, oy: bottom - 38.0, k: 20.0 };
    downlight(ink, v, (0.0, 0.0));
    ink.fill(BLACK);
    ink.text((490.0, bottom - 39.3), size, Font::Regular, Align::Left, "LED downlight, IC-4 rated");
    let c = (478.0, bottom - 50.0);
    ink.pen(0.18, BLACK);
    ink.fill(WHITE);
    ink.circle(c, 2.6, "B");
    ink.fill(BLACK);
    ink.text((c.0, c.1 - 0.9), 2.2, Font::Bold, Align::Centre, "SA");
    ink.text((490.0, bottom - 51.3), size, Font::Regular, Align::Left, "Smoke alarm, hard-wired, interconnected");
    ink.pen(0.18, BLACK);
    ink.fill(WHITE);
    ink.rect(473.5, bottom - 64.2, 9.0, 4.4, "B");
    ink.fill(BLACK);
    ink.text((478.0, bottom - 63.0), 2.4, Font::Regular, Align::Centre, "W01");
    ink.text((490.0, bottom - 63.3), size, Font::Regular, Align::Left, "Window mark, see schedule A-301");
    ink.fill(grey(0.3));
    ink.rect(473.5, bottom - 76.0, 9.0, 3.0, "f");
    ink.fill(BLACK);
    ink.text((490.0, bottom - 75.3), size, Font::Regular, Align::Left, "Wall, framed or masonry");
}

// ---------------------------------------------------------------------------
// The take-off, the notes, and saving

fn area(page: usize, v: View, name: &str, desc: &str, colour: [f64; 3], pts: &[(f64, f64)], holes: &[Vec<(f64, f64)>]) -> Markup {
    let geometry = Geometry::Polygon {
        pts: pts.iter().map(|&(x, y)| v.pt(x, y)).collect(),
        holes: holes.iter().map(|h| h.iter().map(|&(x, y)| v.pt(x, y)).collect()).collect(),
    };
    let mut m = Markup::new(page as u32, MarkupKind::Area, geometry);
    m.style.stroke = colour.map(|c| c as f32);
    m.style.fill = Some(colour.map(|c| c as f32));
    m.style.fill_opacity = 0.28;
    m.style.width = 1.5;
    m.meta.name = name.into();
    m.meta.label = desc.into();
    m.meta.author = AUTHOR.into();
    m
}

fn measured(page: usize, kind: MarkupKind, geometry: Geometry, name: &str, desc: &str, colour: [f64; 3]) -> Markup {
    let mut m = Markup::new(page as u32, kind, geometry);
    m.style.stroke = colour.map(|c| c as f32);
    m.style.width = 2.0;
    m.meta.name = name.into();
    m.meta.label = desc.into();
    m.meta.author = AUTHOR.into();
    m
}

const TILES: [f64; 3] = [0.13, 0.45, 0.85];
const CARPET: [f64; 3] = [0.93, 0.55, 0.13];
const TIMBER: [f64; 3] = [0.55, 0.35, 0.75];
const DECKING: [f64; 3] = [0.15, 0.62, 0.35];
const LIGHTS: [f64; 3] = [0.86, 0.15, 0.15];
const SKIRTING: [f64; 3] = [0.0, 0.58, 0.6];
const BENCH: [f64; 3] = [0.8, 0.2, 0.6];
const GUTTER: [f64; 3] = [0.1, 0.55, 0.45];
const WINDOWS: [f64; 3] = [0.8, 0.35, 0.25];
const SOLAR: [f64; 3] = [0.92, 0.65, 0.05];

/// The tools the take-off was drawn with, kept by name as a user would keep
/// them: group, description, the tool that draws it, and its colour.
const TAKE_OFF_TOOLS: [(&str, &str, &str, [f64; 3]); 10] = [
    ("Floors", "Floor tiles", "measure.area", TILES),
    ("Floors", "Carpet", "measure.area", CARPET),
    ("Floors", "Timber floor", "measure.area", TIMBER),
    ("Floors", "Decking", "measure.area", DECKING),
    ("Joinery", "Skirting", "measure.polylength", SKIRTING),
    ("Joinery", "Benchtop", "measure.length", BENCH),
    ("Electrical", "Downlights", "measure.count", LIGHTS),
    ("Openings", "Windows", "measure.count", WINDOWS),
    ("Roof", "Quad gutter", "measure.length", GUTTER),
    ("Roof", "Solar panels", "measure.count", SOLAR),
];

/// The tools file the app reads (`tools.json` beside its page cache), holding
/// `TAKE_OFF_TOOLS`, so the tools list shows what the drawing was measured
/// with.
fn tools_json() -> String {
    let saved: Vec<serde_json::Value> = TAKE_OFF_TOOLS
        .iter()
        .map(|&(group, name, key, colour)| {
            let area = key == "measure.area";
            serde_json::json!({
                "name": name,
                "group": group,
                "key": key,
                "settings": {
                    "style": {
                        "stroke": colour,
                        "fill": if area { Some(colour) } else { None },
                        "opacity": 1.0,
                        "fill_opacity": if area { 0.28 } else { 1.0 },
                        "pattern": "Solid",
                        "pattern_colour": null,
                        "pattern_opacity": 1.0,
                        "pattern_size": 6.0,
                        "width": if area { 1.5 } else { 2.0 },
                        "width_unit": "Points",
                        "dash": [],
                        "label_size": 10.0,
                        "label_colour": colour,
                        "label_font": "Sans",
                    },
                    "defaults": { "name": "", "description": name, "item_code": "", "layer": "", "status": "" },
                    "depth_m": null,
                    "slope": null,
                },
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({ "changed": {}, "saved": saved, "collapsed": [] })).expect("JSON writes")
}

fn take_off(plan: View, roof: View, north: View, south: View) -> Vec<Markup> {
    let (ix0, iy0, ix1, iy1) = ISLAND;
    let (dx0, dy0, dx1, dy1) = DECK;
    let mut take = vec![
        area(1, plan, "Kitchen, dining, living", "Floor tiles", TILES, &OPEN_PLAN, &[rect_pts(ix0, iy0, ix1, iy1)]),
        area(1, plan, "Bath", "Floor tiles", TILES, &rect_pts(2.555, 5.555, 5.945, 8.245), &[]),
        area(1, plan, "Laundry", "Floor tiles", TILES, &rect_pts(8.555, 9.555, 10.945, 11.865), &[]),
        area(1, plan, "Bed 1", "Carpet", CARPET, &rect_pts(0.135, 8.355, 5.945, 11.865), &[]),
        area(1, plan, "WIR", "Carpet", CARPET, &rect_pts(0.135, 5.555, 2.445, 8.245), &[]),
        area(1, plan, "Hall", "Timber floor", TIMBER, &rect_pts(6.055, 0.135, 8.445, 11.865), &[]),
        area(1, plan, "Study", "Timber floor", TIMBER, &rect_pts(8.555, 0.135, 10.945, 3.445), &[]),
        area(1, plan, "Alfresco", "Decking", DECKING, &rect_pts(dx0, dy0, dx1, dy1), &[]),
    ];
    let counts = [("Kitchen, dining, living", &DOWNLIGHTS_OPEN[..]), ("Hall", &DOWNLIGHTS_HALL[..])];
    for (name, lights) in counts {
        let pts = lights.iter().map(|&(x, y)| plan.pt(x, y)).collect();
        take.push(measured(1, MarkupKind::Count, Geometry::Points { pts }, name, "Downlights", LIGHTS));
    }
    let skirting = [(11.055, 3.555), (11.055, 0.135), (17.865, 0.135), (17.865, 11.865), (15.3, 11.865)];
    take.push(measured(1, MarkupKind::Polylength, Geometry::Polyline { pts: skirting.iter().map(|&(x, y)| plan.pt(x, y)).collect() }, "Living", "Skirting", SKIRTING));
    take.push(measured(1, MarkupKind::Length, Geometry::Line { a: plan.pt(ix0, iy1), b: plan.pt(ix1, iy1) }, "Island", "Benchtop", BENCH));
    take.push(measured(1, MarkupKind::Length, Geometry::Line { a: plan.pt(11.955, 11.265), b: plan.pt(15.3, 11.265) }, "Back bench", "Benchtop", BENCH));

    let panels = (0..12).map(|i| roof.pt(6.4 + (i % 6) as f64 * 1.05, if i < 6 { 8.05 } else { 9.85 })).collect();
    take.push(measured(2, MarkupKind::Count, Geometry::Points { pts: panels }, "North face", "Solar panels", SOLAR));

    // On the elevations: the gutter along each long side, and the windows.
    let walls = walls();
    for (v, face, name) in [(north, 'N', "North"), (south, 'S', "South")] {
        let gutter = Geometry::Line { a: v.pt(-0.65, EAVE - 0.1), b: v.pt(18.65, EAVE - 0.1) };
        take.push(measured(3, MarkupKind::Length, gutter, name, "Quad gutter", GUTTER));
        let windows = elevation_openings(&walls, face)
            .into_iter()
            .filter(|&(_, _, kind)| kind == Kind::Window)
            .map(|(a, b, _)| v.pt((a + b) / 2.0, (SILL + HEAD) / 2.0))
            .collect();
        take.push(measured(3, MarkupKind::Count, Geometry::Points { pts: windows }, name, "Windows", WINDOWS));
    }
    take
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "target/store-images/kestrel-lane.pdf".to_owned());
    let walls = walls();
    let sheets = [
        Sheet { number: "A-001", title: &["GENERAL NOTES"], scale: "—" },
        Sheet { number: "A-101", title: &["GROUND FLOOR PLAN"], scale: "1:50 @ A1" },
        Sheet { number: "A-102", title: &["ROOF PLAN"], scale: "1:100 @ A1" },
        Sheet { number: "A-201", title: &["ELEVATIONS", "NORTH AND SOUTH"], scale: "1:50 @ A1" },
        Sheet { number: "A-202", title: &["ELEVATIONS", "EAST AND WEST"], scale: "1:50 @ A1" },
        Sheet { number: "A-301", title: &["DOOR, WINDOW AND", "FINISHES SCHEDULES"], scale: "—" },
        Sheet { number: "A-501", title: &["DETAILS"], scale: "1:5 @ A1" },
    ];

    let mut inks: Vec<Ink> = (0..sheets.len()).map(|_| Ink::new()).collect();
    notes_sheet(&mut inks[0], &sheets);
    let plan = plan_sheet(&mut inks[1]);
    let roof = roof_sheet(&mut inks[2]);
    let [north, south] = elevation_sheet(&mut inks[3], &walls, [('N', "NORTH ELEVATION"), ('S', "SOUTH ELEVATION")], true);
    elevation_sheet(&mut inks[4], &walls, [('E', "EAST ELEVATION"), ('W', "WEST ELEVATION")], false);
    schedules_sheet(&mut inks[5], &walls);
    details_sheet(&mut inks[6]);
    for (ink, sheet) in inks.iter_mut().zip(&sheets) {
        frame(ink, sheet);
    }

    // The PDF, with each sheet's number as its page label.
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font = |doc: &mut Document, name: &str| {
        doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => name.to_owned(), "Encoding" => "WinAnsiEncoding" })
    };
    let regular = font(&mut doc, "Helvetica");
    let bold = font(&mut doc, "Helvetica-Bold");
    let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => regular, "F2" => bold } });
    let mut kids = Vec::new();
    for ink in inks {
        let content = doc.add_object(Stream::new(dictionary! {}, ink.0));
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real((SHEET.0 * PT) as f32), Object::Real((SHEET.1 * PT) as f32)],
            "Contents" => content,
            "Resources" => resources,
        });
        kids.push(Object::from(page));
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let mut nums = Vec::new();
    for (i, sheet) in sheets.iter().enumerate() {
        nums.push(Object::Integer(i as i64));
        nums.push(Object::Dictionary(dictionary! { "P" => Object::String(sheet.number.as_bytes().to_vec(), StringFormat::Literal) }));
    }
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id, "PageLabels" => dictionary! { "Nums" => nums } });
    doc.trailer.set("Root", catalog);
    let info = doc.add_object(dictionary! {
        "Title" => Object::String(b"Kestrel Lane House - sample drawing set".to_vec(), StringFormat::Literal),
        "Author" => Object::String(b"Sample Studio (fictional)".to_vec(), StringFormat::Literal),
    });
    doc.trailer.set("Info", info);
    doc.compress();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).expect("a document in memory saves");

    // Scales, the take-off, and highlights with notes, saved as the app does.
    let pdfium = worker::bind().expect("pdfium loads");
    let mut scales = ScaleStore::default();
    let mut pages = Vec::new();
    let page_box = Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(SHEET.0 * PT, SHEET.1 * PT));
    for (page, ratio) in [(1usize, 50.0), (2, 100.0), (3, 50.0), (4, 50.0), (6, 5.0)] {
        let scale = Scale::from_ratio(ScaleId::new(), ratio).expect("a positive ratio");
        let id = scales.find_same(&scale).unwrap_or_else(|| {
            let id = scale.id;
            scales.set_scale(scale);
            id
        });
        scales.set_page_scale(page as u32, page_box, id);
        pages.push(page);
    }

    let doc = pdfium.load_pdf_from_byte_slice(&bytes, None).expect("pdfium reads what was written");
    let chars = annots::page_chars(&doc, 0).expect("the notes sheet has text");
    let notes = [
        ("Verify all dimensions on site", [1.0, 0.86, 0.1], "Site measure is booked for Tuesday. Confirm access with the builder."),
        ("R2.5 insulation batts", [0.45, 0.85, 0.35], "Check against the energy report: it may need R2.7."),
        ("P3 slip resistance rating", [1.0, 0.5, 0.7], "The client wants a matt tile in the bathroom as well."),
        ("LED downlights to be IC-4 rated", [0.35, 0.7, 1.0], "19 on the plan take-off: 15 in the open plan and 4 in the hall."),
    ];
    let adds = notes
        .iter()
        .map(|(phrase, color, comment)| {
            let range = selection::find(&chars, phrase).into_iter().next().unwrap_or_else(|| panic!("{phrase:?} is on the notes sheet"));
            NewHighlight { page: 0, quads: selection::bands(&chars, range), color: color.map(|c| c as f32), comment: (*comment).into() }
        })
        .collect();

    // A box round the wet-area notes, and an arrow at the smoke alarms: marked
    // up with the drawing tools, with a note on each.
    let around = |phrase: &str| {
        let range = selection::find(&chars, phrase).into_iter().next().unwrap_or_else(|| panic!("{phrase:?} is on the notes sheet"));
        let bands = selection::bands(&chars, range);
        bands.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |(l, b, r, t), q| (l.min(q.left), b.min(q.bottom), r.max(q.right), t.max(q.top)))
    };
    let red = [0.86, 0.15, 0.15];
    let drawn = |kind, points: Vec<[f32; 2]>, comment: &str| {
        let (xs, ys): (Vec<f32>, Vec<f32>) = points.iter().map(|p| (p[0], p[1])).unzip();
        let min = |v: &[f32]| v.iter().copied().fold(f32::MAX, f32::min) - 3.0;
        let max = |v: &[f32]| v.iter().copied().fold(f32::MIN, f32::max) + 3.0;
        Markup_ {
            key: None,
            page: 0,
            kind,
            bounds: PdfBox { left: min(&xs), bottom: min(&ys), right: max(&xs), top: max(&ys) },
            points,
            color: red,
            width: 2.0,
            style: Default::default(),
            name: String::new(),
            comment: comment.into(),
            author: AUTHOR.into(),
        }
    };
    let (left, _, _, top) = around("Waterproof shower recesses");
    let (_, _, right, _) = around("turning the membrane up 150 mm at");
    let (_, bottom, _, _) = around("laid to fall to the floor waste");
    let boxed = drawn(DrawKind::Rectangle, vec![[left - 30.0, top + 6.0], [right + 12.0, bottom - 6.0]], "Tiler to confirm the membrane before the screed goes down.");
    let (_, b, r, t) = around("in the hall and in every bedroom.");
    let y = (b + t) / 2.0;
    let arrow = drawn(DrawKind::Arrow, vec![[r + 150.0, y + 8.0], [r + 10.0, y]], "Add one in the study as well.");
    drop(doc);

    let changes = Changes {
        adds,
        markups: vec![boxed, arrow],
        author: AUTHOR.into(),
        scales: Some(ScaleChanges { scales, pages }),
        measures: MeasureChanges { written: take_off(plan, roof, north, south), removed: Vec::new() },
        ..Default::default()
    };
    let saved = annots::save(&pdfium, &bytes, &changes).expect("saves");

    // Read back as the app reads a file, so a take-off that would open as
    // plain markups rather than measurements stops here.
    let written = changes.measures.written.len();
    let back = pdf_io::read(&Document::load_mem(&saved.bytes).expect("lopdf reads what was saved"));
    for (page, why) in &back.skipped {
        eprintln!("page {}: {why}", page + 1);
    }
    assert_eq!(back.markups.len(), written, "every measurement reads back as one");
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).expect("makes the folder");
    }
    std::fs::write(&out, &saved.bytes).expect("writes the file");
    println!("wrote {out}");
    let tools = format!("{}-tools.json", out.trim_end_matches(".pdf"));
    std::fs::write(&tools, tools_json()).expect("writes the tools");
    println!("wrote {tools}");

    // `--png` also draws each sheet to a picture beside it, to look over.
    if std::env::args().any(|a| a == "--png") {
        let doc = pdfium.load_pdf_from_byte_slice(&saved.bytes, None).expect("pdfium reads what was saved");
        for i in 0..sheets.len() {
            let ([w, h], rgba) = annots::render_page(&doc, i, 1.5).expect("draws");
            let path = format!("{}-{}.png", out.trim_end_matches(".pdf"), sheets[i].number);
            let file = std::io::BufWriter::new(std::fs::File::create(&path).expect("creates the picture"));
            let mut png = png::Encoder::new(file, w as u32, h as u32);
            png.set_color(png::ColorType::Rgba);
            png.write_header().and_then(|mut wr| wr.write_image_data(&rgba)).expect("writes the picture");
            println!("wrote {path}");
        }
    }
}
