//! Typical sections and the standard details, each drawn from its sizes.

use crate::draw::*;
use crate::geom::*;
use crate::sample_ink::{grey, text_width, Align, Font, Ink, View, BLACK, WHITE};
use crate::town::*;

/// A road's typical section at 1:50, one side drawn and mirrored.
pub fn cross_section(ink: &mut Ink, v: View, kind: Kind, name: &str, n: &str) {
    let (half, verge, median) = (kind.half(), kind.verge(), kind.median());
    let reserve = half + verge;
    let s = |x: f64| if x.abs() < median { 0.15 } else { -CROSSFALL * (x.abs().min(half) - median) };
    let lip = s(half);
    let invert = lip - 0.025;
    let kerb_top = invert + 0.15;
    let verge_at = |x: f64| kerb_top + 0.025 * (x.abs() - half - 0.45);
    let edge = half + 0.6;
    for side in [-1.0, 1.0] {
        let x = |d: f64| side * d;
        let pts = |list: &[(f64, f64)]| -> Vec<(f64, f64)> { list.iter().map(|&(a, b)| v.p(x(a), b)).collect() };
        let fall = |d: f64| -CROSSFALL * (d - median);
        let layer = |top: f64, bottom: f64| pts(&[(median, top), (half, fall(half) + top), (edge, fall(edge) + top), (edge, fall(edge) + bottom), (half, fall(half) + bottom), (median, bottom)]);
        aggregate(ink, &layer(-0.19, -0.39), 11 + side as u64, 0.9);
        aggregate(ink, &layer(-0.04, -0.19), 23 + side as u64, 0.5);
        ink.fill(grey(0.2));
        ink.poly(&pts(&[(median, 0.0), (half, lip), (half, lip - 0.04), (median, -0.04)]), true, "f");
        let kerb = pts(&[(half, lip), (half + 0.3, invert), (half + 0.32, kerb_top), (half + 0.45, kerb_top), (half + 0.45, invert - 0.15), (half, invert - 0.15)]);
        concrete(ink, &kerb, 7 + side as u64);
        if median > 0.0 {
            let m = pts(&[(0.0, 0.15), (median - 0.15, 0.15), (median - 0.05, 0.02), (median, 0.0), (median, -0.2), (0.0, -0.2)]);
            concrete(ink, &m, 5 + side as u64);
        }
        let sub_top = fall(edge) - 0.39;
        let trench = pts(&[(half + 0.15, sub_top), (half + 0.45, sub_top), (half + 0.45, sub_top - 0.6), (half + 0.15, sub_top - 0.6)]);
        aggregate(ink, &trench, 31 + side as u64, 0.35);
        ink.fill(WHITE);
        ink.pen(0.25, BLACK);
        ink.circle(v.p(x(half + 0.3), sub_top - 0.52), 0.05 * v.k, "B");
        earth(ink, &pts(&[(half + 0.45, kerb_top), (reserve, verge_at(reserve)), (reserve + 1.2, verge_at(reserve))]));
        let path = pts(&[(reserve - 1.5, verge_at(reserve - 1.5)), (reserve - 0.3, verge_at(reserve - 0.3)), (reserve - 0.3, verge_at(reserve - 0.3) - 0.1), (reserve - 1.5, verge_at(reserve - 1.5) - 0.1)]);
        concrete(ink, &path, 41 + side as u64);
        for (from, depth, colour) in [(0.6, 0.6, ELECTRICITY), (1.1, 0.45, COMMS), (2.3, 0.6, WATER), (2.8, 0.6, GAS)] {
            let d = reserve - from;
            ink.fill(colour);
            ink.circle(v.p(x(d), verge_at(d) - depth), 0.05 * v.k + 0.3, "f");
        }
        ink.pen(0.18, BLACK);
        ink.dash(&[3.0, 1.5]);
        ink.line(v.p(x(reserve), verge_at(reserve) - 1.4), v.p(x(reserve), verge_at(reserve) + 1.4));
        ink.dash(&[]);
        let t = half + 1.7;
        ink.pen(0.35, grey(0.35));
        ink.line(v.p(x(t), verge_at(t)), v.p(x(t), verge_at(t) + 1.6));
        ink.pen(0.25, TREES);
        ink.fill([0.88, 0.95, 0.88]);
        ink.circle(v.p(x(t), verge_at(t) + 2.6), 1.1 * v.k, "B");
    }
    let r = |d: f64, z: f64| v.p(d, z);
    let lx = v.p(reserve + 2.0, 0.0).0 + 6.0;
    let at = |z: f64| (lx, r(0.0, z).1);
    let mid = (median + half) / 2.0;
    callout(ink, r(mid, s(mid) - 0.02), at(1.6), 2.2, &["ASPHALT, SEE PAVEMENTS"]);
    callout(ink, r(mid + 1.0, s(mid) - 0.14), at(1.3), 2.2, &["150 BASECOURSE"]);
    callout(ink, r(mid + 1.5, s(mid) - 0.32), at(1.0), 2.2, &["SUBBASE, SEE PAVEMENTS"]);
    callout(ink, r(half + 0.4, kerb_top - 0.05), at(0.7), 2.2, &["BARRIER KERB, C-801"]);
    callout(ink, r(half + 0.3, lip - 0.95), at(-1.3), 2.2, &["100Ø SUBSOIL DRAIN, C-803"]);
    callout(ink, r(reserve - 0.9, verge_at(reserve - 0.9) - 0.05), at(1.9), 2.2, &["1.2 CONCRETE PATH, C-802"]);
    callout(ink, r(reserve - 2.3, verge_at(reserve - 2.3) - 0.6), at(-1.0), 2.2, &["SERVICES, SEE ALLOCATIONS"]);
    ink.fill(BLACK);
    for (x, text) in [(-mid, "3%"), (mid, "3%"), (-(half + verge / 2.0), "2.5%"), (half + verge / 2.0, "2.5%")] {
        let z = if x.abs() <= half { s(x) } else { verge_at(x) };
        let p = v.p(x, z);
        ink.text((p.0, p.1 + 2.6), 2.2, Font::Bold, Align::Centre, text);
    }
    let top = v.p(0.0, 2.3).1;
    let mut xs = vec![-reserve, -half];
    let mut labels = vec!["VERGE", "CARRIAGEWAY"];
    if median > 0.0 {
        xs.extend([-median, median]);
        labels.extend(["MEDIAN", "CARRIAGEWAY"]);
    } else {
        xs.push(0.0);
        labels.push("CARRIAGEWAY");
    }
    xs.extend([half, reserve]);
    labels.push("VERGE");
    let pts: Vec<f64> = xs.iter().map(|&x| v.p(x, 0.0).0).collect();
    ink.pen(0.13, BLACK);
    ink.line((pts[0] - 2.0, top), (pts[pts.len() - 1] + 2.0, top));
    ink.line((pts[0] - 2.0, top + 7.0), (pts[pts.len() - 1] + 2.0, top + 7.0));
    ink.pen(0.35, BLACK);
    for (i, &x) in pts.iter().enumerate() {
        ink.line((x - 1.1, top - 1.1), (x + 1.1, top + 1.1));
        if i == 0 || i + 1 == pts.len() {
            ink.line((x - 1.1, top + 5.9), (x + 1.1, top + 8.1));
        }
    }
    ink.fill(BLACK);
    for i in 0..pts.len() - 1 {
        let m = (pts[i] + pts[i + 1]) / 2.0;
        ink.text((m, top + 1.2), 2.3, Font::Regular, Align::Centre, &format!("{:.2}", xs[i + 1] - xs[i]));
        ink.text((m, top - 3.4), 1.8, Font::Regular, Align::Centre, labels[i]);
    }
    ink.text(((pts[0] + pts[pts.len() - 1]) / 2.0, top + 8.2), 2.4, Font::Bold, Align::Centre, &format!("{:.2} ROAD RESERVE", 2.0 * reserve));
    title(ink, (pts[0], v.p(0.0, -2.3).1), n, name, "1:50 @ A1");
}

/// The typical sections: the boulevard on one sheet, the collector and
/// local streets on the next.
pub fn typical_sheet(ink: &mut Ink, part: usize) -> Vec<View> {
    let mut views = Vec::new();
    if part == 0 {
        let a = View { ox: 290.0, oy: 400.0, k: 20.0 };
        cross_section(ink, a, Kind::Boulevard, "HERON RIDGE BOULEVARD", "A");
        views.push(a);
        let x = 40.0;
        heading(ink, (x, 250.0), "PAVEMENTS");
        let rows = vec![
            vec!["Design traffic".into(), "1 000 000 ESA".into(), "300 000 ESA".into(), "50 000 ESA".into()],
            vec!["Wearing course".into(), "50 AC14".into(), "40 AC14".into(), "30 AC10".into()],
            vec!["Basecourse".into(), "150".into(), "150".into(), "150".into()],
            vec!["Subbase".into(), "250".into(), "200".into(), "150".into()],
            vec!["Subgrade CBR".into(), "5%".into(), "5%".into(), "5%".into()],
        ];
        let bottom = table(ink, x, 244.0, &[("", 50.0), ("BOULEVARD", 40.0), ("COLLECTOR", 40.0), ("LOCAL", 40.0)], &rows, 6.0, 2.4);
        heading(ink, (x + 230.0, 250.0), "SERVICE ALLOCATIONS");
        let rows = vec![
            vec!["Electricity".into(), "0.6".into(), "0.6".into()],
            vec!["Communications".into(), "1.1".into(), "0.45".into()],
            vec!["Water".into(), "2.3".into(), "0.6".into()],
            vec!["Gas".into(), "2.8".into(), "0.6".into()],
        ];
        table(ink, x + 230.0, 244.0, &[("", 50.0), ("FROM BOUNDARY", 36.0), ("COVER", 30.0)], &rows, 6.0, 2.4);
        let _ = bottom;
    } else {
        let b = View { ox: 250.0, oy: 440.0, k: 20.0 };
        cross_section(ink, b, Kind::Collector, "PELICAN ROAD", "B");
        let c = View { ox: 250.0, oy: 205.0, k: 20.0 };
        cross_section(ink, c, Kind::Local, "LOCAL STREETS", "C");
        views.extend([b, c]);
    }
    views
}

// ---------------------------------------------------------------------------
// Details

fn kerb_profile(ink: &mut Ink, v: View, face: f64, top_w: f64, seed: u64) {
    let road = [(-0.3, 0.0), (0.0, 0.0), (0.0, -0.04), (-0.3, -0.04)];
    ink.fill(grey(0.2));
    ink.poly(&v.all(&road), true, "f");
    aggregate(ink, &v.all(&[(-0.3, -0.04), (0.0, -0.04), (0.0, -0.175), (0.6, -0.175), (0.6, -0.325), (-0.3, -0.325)]), seed, 1.4);
    let back = 0.3 + face + top_w;
    let kerb = [(0.0, 0.0), (0.3, -0.025), (0.3 + face, 0.125), (back, 0.125), (back, -0.175), (0.0, -0.175)];
    concrete(ink, &v.all(&kerb), seed + 7);
    earth(ink, &v.all(&[(back, 0.125), (back + 0.3, 0.135)]));
    v.dims_x(ink, &[0.0, 0.3, 0.3 + face, back], v.p(0.0, 0.3).1, v.p(0.0, 0.14).1);
    v.dims_y(ink, &[-0.175, -0.025, 0.125], v.p(back + 0.15, 0.0).0, v.p(back + 0.02, 0.0).0);
}

fn kerbs(ink: &mut Ink) -> Vec<(&'static str, Bx, f64)> {
    let one = View { ox: 90.0, oy: 470.0, k: 200.0 };
    kerb_profile(ink, one, 0.025, 0.125, 3);
    // The subsoil drain behind it.
    let trench = [(0.15, -0.525), (0.45, -0.525), (0.45, -1.125), (0.15, -1.125)];
    aggregate(ink, &one.all(&[(-0.3, -0.325), (0.6, -0.325), (0.6, -0.525), (-0.3, -0.525)]), 5, 2.6);
    aggregate(ink, &one.all(&trench), 19, 1.1);
    ink.fill(WHITE);
    ink.pen(0.4, BLACK);
    ink.circle(one.p(0.3, -1.06), 0.05 * one.k, "B");
    let r = |x: f64, z: f64| one.p(x, z);
    callout(ink, r(0.2, 0.0), (r(0.0, 0.0).0 - 20.0, r(0.0, 0.35).1), 2.3, &["32 MPa CONCRETE,", "EXTRUDED"]);
    callout(ink, r(0.3, -1.1), (r(0.0, 0.0).0 + 160.0, r(0.0, -1.0).1), 2.3, &["100Ø SLOTTED PVC IN", "FILTER SOCK, TO NEAREST PIT"]);
    callout(ink, r(-0.1, -0.45), (r(0.0, 0.0).0 - 20.0, r(0.0, -0.45).1), 2.3, &["SUBBASE"]);
    title(ink, (40.0, 240.0), "1", "BARRIER KERB AND GUTTER", "1:5");

    let two = View { ox: 520.0, oy: 500.0, k: 100.0 };
    kerb_profile(ink, two, 0.12, 0.05, 11);
    title(ink, (470.0, 440.0), "2", "SEMI-MOUNTABLE KERB, MEDIANS", "1:10");
    let three = View { ox: 520.0, oy: 360.0, k: 100.0 };
    let lay = [(0.0, 0.0), (0.3, -0.025), (0.75, 0.05), (0.9, 0.05), (0.9, -0.175), (0.0, -0.175)];
    concrete(ink, &three.all(&lay), 17);
    three.dims_x(ink, &[0.0, 0.3, 0.75, 0.9], three.p(0.0, 0.2).1, three.p(0.0, 0.06).1);
    title(ink, (470.0, 300.0), "3", "LAYBACK KERB AT DRIVEWAYS", "1:10");
    let four = View { ox: 560.0, oy: 150.0, k: 20.0 };
    // A kerb ramp in plan: the ramp, its flares and the tactile strip.
    ink.fill(PATH);
    ink.pen(0.3, BLACK);
    ink.poly(&four.all(&[(-1.2, 0.0), (1.2, 0.0), (2.2, 1.5), (-2.2, 1.5)]), true, "B");
    ink.fill([0.95, 0.8, 0.2]);
    four.rect(ink, -0.9, 0.15, 0.9, 0.75, "B");
    ink.pen(0.1, BLACK);
    for i in 0..6 {
        for j in 0..3 {
            ink.circle(four.p(-0.75 + i as f64 * 0.3, 0.3 + j as f64 * 0.2), 0.6, "S");
        }
    }
    four.dims_x(ink, &[-1.2, 1.2], four.p(0.0, -0.8).1, four.p(0.0, -0.1).1);
    title(ink, (470.0, 100.0), "4", "KERB RAMP, PLAN", "1:20");
    vec![("Detail 1", (30.0, 230.0, 460.0, 580.0), 5.0), ("Detail 4", (460.0, 62.0, 830.0, 290.0), 20.0)]
}

fn paths(ink: &mut Ink) -> Vec<(&'static str, Bx, f64)> {
    let one = View { ox: 80.0, oy: 470.0, k: 100.0 };
    let fall = |x: f64| -0.025 * x;
    earth(ink, &one.all(&[(-0.3, 0.0), (0.0, 0.0)]));
    earth(ink, &one.all(&[(1.2, fall(1.2)), (1.5, fall(1.2))]));
    aggregate(ink, &one.all(&[(0.0, -0.1), (1.2, fall(1.2) - 0.1), (1.2, fall(1.2) - 0.15), (0.0, -0.15)]), 29, 0.5);
    concrete(ink, &one.all(&[(0.0, 0.0), (1.2, fall(1.2)), (1.2, fall(1.2) - 0.1), (0.0, -0.1)]), 37);
    ink.pen(0.3, BLACK);
    ink.dash(&[2.0, 0.8]);
    ink.line(one.p(0.03, -0.05), one.p(1.17, fall(1.17) - 0.05));
    ink.dash(&[]);
    one.dims_x(ink, &[0.0, 1.2], one.p(0.0, 0.12).1, one.p(0.0, 0.02).1);
    one.dims_y(ink, &[-0.15, -0.1, 0.0], one.p(-0.12, 0.0).0, one.p(-0.02, 0.0).0);
    let r = |x: f64, z: f64| one.p(x, z);
    callout(ink, r(0.9, fall(0.9) - 0.05), (r(0.9, 0.0).0 + 40.0, r(0.0, 0.2).1), 2.3, &["SL72 MESH, CENTRAL"]);
    callout(ink, r(0.5, -0.125), (r(0.5, 0.0).0 + 40.0, r(0.0, -0.3).1), 2.3, &["50 SAND BEDDING"]);
    title(ink, (40.0, 400.0), "1", "CONCRETE FOOTPATH", "1:10");
    let two = View { ox: 80.0, oy: 250.0, k: 50.0 };
    let slope = |x: f64| 0.02 * x;
    aggregate(ink, &two.all(&[(0.0, -0.15), (4.3, slope(4.3) - 0.15), (4.3, slope(4.3) - 0.3), (0.0, -0.3)]), 43, 0.8);
    concrete(ink, &two.all(&[(0.0, 0.0), (4.3, slope(4.3)), (4.3, slope(4.3) - 0.15), (0.0, -0.15)]), 47);
    two.dims_x(ink, &[0.0, 0.9, 4.3], two.p(0.0, 0.35).1, two.p(0.0, 0.12).1);
    let r = |x: f64, z: f64| two.p(x, z);
    callout(ink, r(2.0, slope(2.0) - 0.07), (r(4.3, 0.0).0 + 20.0, r(0.0, 0.4).1), 2.3, &["150 CONCRETE, SL82 MESH"]);
    callout(ink, r(3.0, slope(3.0) - 0.22), (r(4.3, 0.0).0 + 20.0, r(0.0, -0.3).1), 2.3, &["150 CRUSHED ROCK BASE"]);
    title(ink, (40.0, 190.0), "2", "DRIVEWAY CROSSING, SECTION", "1:20");
    vec![("Detail 2", (30.0, 150.0, 830.0, 300.0), 20.0)]
}

fn bedding(ink: &mut Ink) -> Vec<(&'static str, Bx, f64)> {
    let mut vps = Vec::new();
    for (k, &d) in [0.375, 0.6, 0.9, 1.2].iter().enumerate() {
        let v = View { ox: 110.0 + k as f64 * 195.0, oy: 380.0, k: 50.0 };
        let wall = d / 10.0 + 0.03;
        let r0 = d / 2.0 + wall;
        let w = r0 + 0.3;
        aggregate(ink, &v.all(&[(-w, -r0 - 0.1), (w, -r0 - 0.1), (w, r0 + 0.15), (-w, r0 + 0.15)]), 43 + k as u64, 0.6);
        ink.fill(WHITE);
        ink.pen(0.5, BLACK);
        ink.circle(v.p(0.0, 0.0), r0 * v.k, "B");
        ink.pen(0.3, BLACK);
        ink.circle(v.p(0.0, 0.0), d / 2.0 * v.k, "S");
        ink.pen(0.13, BLACK);
        ink.dash(&[1.5, 1.0]);
        for z in [-r0, -r0 + 0.3 * 2.0 * r0, r0, r0 + 0.15] {
            ink.line(v.p(-w, z), v.p(w, z));
        }
        ink.dash(&[]);
        earth(ink, &v.all(&[(-w - 0.2, r0 + 0.8), (-w, r0 + 0.8), (-w, -r0 - 0.1), (w, -r0 - 0.1), (w, r0 + 0.8), (w + 0.2, r0 + 0.8)]));
        v.dims_x(ink, &[-w, -r0, r0, w], v.p(0.0, r0 + 1.0).1, v.p(0.0, r0 + 0.82).1);
        title(ink, (v.ox - 70.0, 230.0), &format!("{}", k + 1), &format!("{:.0}Ø RCP, HS2", d * 1000.0), "1:20");
    }
    let r = |x: f64, z: f64| View { ox: 695.0, oy: 380.0, k: 50.0 }.p(x, z);
    let _ = r;
    vps.push(("Details 1 to 4", (30.0, 200.0, 830.0, 580.0), 20.0));
    vps
}

fn pits(ink: &mut Ink) -> Vec<(&'static str, Bx, f64)> {
    let v = View { ox: 150.0, oy: 440.0, k: 100.0 };
    let (inner, wall, depth) = (0.45, 0.15, 1.5);
    let o = inner + wall;
    aggregate(ink, &v.all(&[(-o - 0.3, 0.0), (-o, 0.0), (-o, -depth - wall - 0.075), (o, -depth - wall - 0.075), (o, 0.15), (o + 0.3, 0.15), (o + 0.3, -depth - 0.3), (-o - 0.3, -depth - 0.3)]), 51, 1.6);
    concrete(ink, &v.all(&[(-o, 0.0), (-inner, 0.0), (-inner, -depth), (inner, -depth), (inner, 0.15), (o, 0.15), (o, -depth - wall), (-o, -depth - wall)]), 53);
    concrete(ink, &v.all(&[(-inner, -depth + 0.3), (-0.22, -depth + 0.05), (0.22, -depth + 0.05), (inner, -depth + 0.3), (inner, -depth), (-inner, -depth)]), 57);
    concrete(ink, &v.all(&[(inner - 0.15, 0.15), (o + 0.15, 0.15), (o + 0.15, 0.3), (inner - 0.15, 0.3)]), 59);
    ink.pen(0.3, BLACK);
    ink.fill(WHITE);
    ink.poly(&v.all(&[(-inner, 0.0), (inner - 0.15, 0.0), (inner - 0.15, -0.05), (-inner, -0.05)]), true, "B");
    for i in 1..12 {
        let x = -inner + i as f64 * (2.0 * inner - 0.15) / 12.0;
        v.line(ink, (x, 0.0), (x, -0.05));
    }
    ink.dash(&[1.5, 1.0]);
    ink.circle(v.p(0.0, -depth + 0.275), 0.225 * v.k, "S");
    ink.dash(&[]);
    for i in 0..4 {
        let z = -0.35 - i as f64 * 0.3;
        ink.pen(0.35, BLACK);
        ink.poly(&v.all(&[(inner, z), (inner - 0.12, z), (inner - 0.12, z - 0.02), (inner, z - 0.02)]), false, "S");
    }
    v.dims_x(ink, &[-inner, inner], v.p(0.0, 0.5).1, v.p(0.0, 0.32).1);
    v.dims_y(ink, &[-depth, 0.0], v.p(-o - 0.45, 0.0).0, v.p(-o - 0.32, 0.0).0);
    let r = |x: f64, z: f64| v.p(x, z);
    let lx = r(o, 0.0).0 + 26.0;
    callout(ink, r(inner + 0.1, 0.25), (lx, r(0.0, 0.4).1), 2.3, &["PRECAST LINTEL"]);
    callout(ink, r(0.0, -0.03), (lx, r(0.0, 0.1).1), 2.3, &["HEAVY-DUTY GRATE AND FRAME"]);
    callout(ink, r(inner - 0.06, -0.66), (lx, r(0.0, -0.5).1), 2.3, &["STEP IRONS AT 300"]);
    callout(ink, r(o - 0.07, -0.9), (lx, r(0.0, -0.85).1), 2.3, &["150 REINFORCED WALLS"]);
    callout(ink, r(0.3, -depth + 0.1), (lx, r(0.0, -1.3).1), 2.3, &["BENCHING TO HALF", "THE OUTLET"]);
    title(ink, (40.0, 230.0), "1", "KERB INLET PIT, SECTION", "1:10");
    // The pits in plan, one for each lintel.
    for (k, &lintel) in [1.8, 2.4, 3.0].iter().enumerate() {
        let p = View { ox: 540.0, oy: 500.0 - k as f64 * 120.0, k: 50.0 };
        ink.fill(WHITE);
        ink.pen(0.35, BLACK);
        p.rect(ink, -lintel / 2.0 - 0.15, -0.6, lintel / 2.0 + 0.15, 0.6, "B");
        p.rect(ink, -lintel / 2.0, -0.45, lintel / 2.0, 0.45, "S");
        ink.pen(0.8, BLACK);
        p.line(ink, (-lintel / 2.0, 0.6), (lintel / 2.0, 0.6));
        p.dims_x(ink, &[-lintel / 2.0, lintel / 2.0], p.p(0.0, 1.1).1, p.p(0.0, 0.7).1);
        ink.fill(BLACK);
        ink.text(p.p(-lintel / 2.0 - 0.15, -1.2), 2.8, Font::Bold, Align::Left, &format!("{} LINTEL {lintel:.1}", ["A", "B", "C"][k]));
    }
    title(ink, (470.0, 230.0), "2", "KERB INLET PITS, PLAN", "1:20");
    vec![("Detail 2", (460.0, 200.0, 830.0, 580.0), 20.0)]
}

fn headwalls(ink: &mut Ink) -> Vec<(&'static str, Bx, f64)> {
    for (k, &d) in [0.6, 0.9, 1.2, 1.5].iter().enumerate() {
        let (col, row) = (k % 2, k / 2);
        let v = View { ox: 160.0 + col as f64 * 380.0, oy: 430.0 - row as f64 * 250.0, k: 40.0 };
        let w = d + 1.2;
        let h = d + 0.6;
        concrete(ink, &v.all(&[(-w, 0.0), (w, 0.0), (w, h), (-w, h)]), 61 + k as u64);
        ink.fill(WHITE);
        ink.pen(0.4, BLACK);
        ink.circle(v.p(0.0, 0.3 + d / 2.0), d / 2.0 * v.k, "B");
        concrete(ink, &v.all(&[(-w - 0.3, -0.3), (w + 0.3, -0.3), (w + 0.3, 0.0), (-w - 0.3, 0.0)]), 71 + k as u64);
        // Wing walls in plan below.
        let py = -1.6;
        ink.pen(0.35, BLACK);
        v.rect(ink, -w, py - 0.3, w, py, "S");
        v.line(ink, (-w, py - 0.3), (-w - 0.8, py - 1.3));
        v.line(ink, (w, py - 0.3), (w + 0.8, py - 1.3));
        v.dims_x(ink, &[-w, -d / 2.0, d / 2.0, w], v.p(0.0, h + 0.5).1, v.p(0.0, h + 0.1).1);
        v.dims_y(ink, &[0.0, 0.3, 0.3 + d, h], v.p(w + 0.6, 0.0).0, v.p(w + 0.1, 0.0).0);
        title(ink, (v.ox - 120.0, v.oy - 125.0), &format!("{}", k + 1), &format!("HEADWALL, {:.0}Ø PIPE", d * 1000.0), "1:25");
    }
    vec![]
}

fn manholes(ink: &mut Ink) -> Vec<(&'static str, Bx, f64)> {
    for (k, &depth) in [1.5, 3.0].iter().enumerate() {
        let v = View { ox: 150.0 + k as f64 * 330.0, oy: 470.0, k: 40.0 };
        let (r, wall) = (0.525, 0.1);
        aggregate(ink, &v.all(&[(-r - wall - 0.3, 0.0), (r + wall + 0.3, 0.0), (r + wall + 0.3, -depth - 0.4), (-r - wall - 0.3, -depth - 0.4)]), 81 + k as u64, 2.0);
        concrete(ink, &v.all(&[(-r - wall, 0.0), (-r, 0.0), (-r, -depth), (r, -depth), (r, 0.0), (r + wall, 0.0), (r + wall, -depth - 0.2), (-r - wall, -depth - 0.2)]), 91 + k as u64);
        ink.fill(WHITE);
        ink.pen(0.35, BLACK);
        v.rect(ink, -0.3, 0.0, 0.3, 0.08, "B");
        ink.pen(0.3, SEWER);
        v.line(ink, (-r, -depth + 0.15), (r, -depth + 0.15));
        v.dims_y(ink, &[-depth, 0.0], v.p(-r - wall - 0.6, 0.0).0, v.p(-r - wall - 0.35, 0.0).0);
        v.dims_x(ink, &[-r, r], v.p(0.0, 0.5).1, v.p(0.0, 0.1).1);
        for i in 0..((depth / 0.3) as usize).saturating_sub(1) {
            let z = -0.4 - i as f64 * 0.3;
            ink.pen(0.3, BLACK);
            v.line(ink, (r, z), (r - 0.12, z));
        }
        title(ink, (v.ox - 110.0, 150.0), &format!("{}", k + 1), &format!("SEWER MANHOLE TO {depth:.1} DEEP"), "1:25");
    }
    vec![]
}

fn walls(ink: &mut Ink) -> Vec<(&'static str, Bx, f64)> {
    for (k, &h) in [0.3f64, 0.6, 0.9, 1.2].iter().enumerate() {
        let v = View { ox: 110.0 + k as f64 * 190.0, oy: 300.0, k: 100.0 };
        let courses = (h / 0.2).ceil() as usize;
        for c in 0..courses {
            let z = c as f64 * 0.2;
            let set_back = c as f64 * 0.02;
            concrete(ink, &v.all(&[(set_back, z), (set_back + 0.3, z), (set_back + 0.3, z + 0.2), (set_back, z + 0.2)]), 100 + (k * 10 + c) as u64);
        }
        concrete(ink, &v.all(&[(-0.1, -0.15), (0.5, -0.15), (0.5, 0.0), (-0.1, 0.0)]), 150 + k as u64);
        aggregate(ink, &v.all(&[(0.32, 0.0), (0.62, 0.0), (0.62, h), (0.32 + h * 0.1, h)]), 160 + k as u64, 0.8);
        ink.fill(WHITE);
        ink.pen(0.3, BLACK);
        ink.circle(v.p(0.45, 0.08), 0.05 * v.k, "B");
        earth(ink, &v.all(&[(-0.6, 0.0), (0.0, 0.0)]));
        earth(ink, &v.all(&[(0.3, h), (1.0, h)]));
        v.dims_y(ink, &[0.0, h], v.p(-0.3, 0.0).0, v.p(-0.05, 0.0).0);
        title(ink, (v.ox - 70.0, 200.0), &format!("{}", k + 1), &format!("BLOCK WALL TO {h:.1}"), "1:10");
    }
    notes_block(ink, 40.0, 150.0, 700.0, "NOTES", &[
        "Walls are between lots, on the higher lot's side, as the finished level plans.".into(),
        "Hollow concrete blocks, core filled, on a 150 reinforced strip footing.".into(),
        "Walls over 1.0 high to the engineer's design, with a certificate on completion.".into(),
    ]);
    vec![]
}

fn signs(ink: &mut Ink) -> Vec<(&'static str, Bx, f64)> {
    // The sign faces.
    let faces = [(80.0, "STOP"), (190.0, "GIVE WAY"), (300.0, "HUMP"), (410.0, "CROSSING")];
    for (k, (x, name)) in faces.iter().enumerate() {
        let c = (*x, 470.0);
        match k {
            0 => {
                let pts: Vec<(f64, f64)> = (0..8).map(|i| std::f64::consts::PI / 8.0 + i as f64 * std::f64::consts::PI / 4.0).map(|a| (c.0 + 30.0 * a.cos(), c.1 + 30.0 * a.sin())).collect();
                ink.fill([0.85, 0.1, 0.1]);
                ink.poly(&pts, true, "f");
                ink.fill(WHITE);
                ink.text((c.0, c.1 - 4.0), 12.0, Font::Bold, Align::Centre, name);
            }
            1 => {
                ink.fill(WHITE);
                ink.pen(4.0, [0.85, 0.1, 0.1]);
                ink.poly(&[(c.0 - 30.0, c.1 + 22.0), (c.0 + 30.0, c.1 + 22.0), (c.0, c.1 - 30.0)], true, "B");
                ink.fill(BLACK);
                ink.text((c.0, c.1 + 4.0), 5.0, Font::Bold, Align::Centre, name);
            }
            _ => {
                ink.fill([1.0, 0.85, 0.1]);
                ink.pen(1.0, BLACK);
                ink.poly(&[(c.0, c.1 + 30.0), (c.0 + 30.0, c.1), (c.0, c.1 - 30.0), (c.0 - 30.0, c.1)], true, "B");
                ink.fill(BLACK);
                ink.text((c.0, c.1 - 2.0), 6.0, Font::Bold, Align::Centre, name);
            }
        }
    }
    title(ink, (40.0, 420.0), "1", "SIGN FACES", "1:10");
    // Street name blade.
    ink.fill([0.1, 0.35, 0.2]);
    ink.rect(500.0, 455.0, 300.0, 30.0, "f");
    ink.fill(WHITE);
    ink.text((650.0, 464.0), 12.0, Font::Bold, Align::Centre, "HERON RIDGE BVD");
    title(ink, (500.0, 420.0), "2", "STREET NAME BLADE", "1:5");
    // Line marking patterns.
    let v = View { ox: 60.0, oy: 330.0, k: 20.0 };
    ink.fill(BLACK);
    for i in 0..4 {
        v.rect(ink, i as f64 * 12.0, 0.0, i as f64 * 12.0 + 3.0, 0.1, "f");
    }
    v.dims_x(ink, &[0.0, 3.0, 12.0], v.p(0.0, 1.0).1, v.p(0.0, 0.2).1);
    v.rect(ink, 0.0, -2.0, 36.0, -1.7, "f");
    ink.fill(BLACK);
    ink.text(v.p(37.0, -1.95), 2.6, Font::Regular, Align::Left, "HOLD LINE, 300 WIDE");
    ink.text(v.p(37.0, 0.0), 2.6, Font::Regular, Align::Left, "LANE LINE, 100 WIDE");
    title(ink, (40.0, 270.0), "3", "LINE MARKING", "1:20");
    // A road hump in section.
    let h = View { ox: 540.0, oy: 330.0, k: 40.0 };
    let hump: Vec<(f64, f64)> = (0..=40).map(|i| i as f64 / 40.0).map(|t| (-1.85 + 3.7 * t, 0.1 * (t * std::f64::consts::PI).sin())).collect();
    ink.fill(grey(0.3));
    let mut shape = h.all(&hump);
    shape.push(h.p(1.85, 0.0));
    ink.poly(&shape, true, "f");
    earth(ink, &h.all(&[(-3.0, 0.0), (3.0, 0.0)]));
    h.dims_x(ink, &[-1.85, 1.85], h.p(0.0, 0.5).1, h.p(0.0, 0.15).1);
    h.dims_y(ink, &[0.0, 0.1], h.p(2.4, 0.0).0, h.p(1.9, 0.0).0);
    title(ink, (500.0, 270.0), "4", "ROAD HUMP, SECTION", "1:25");
    vec![]
}

fn outlet(ink: &mut Ink) -> Vec<(&'static str, Bx, f64)> {
    let v = View { ox: 250.0, oy: 400.0, k: 50.0 };
    // A basin's outlet: the weir wall and its orifice, the pipe to the creek.
    earth(ink, &v.all(&[(-5.0, 1.5), (-3.0, 0.0), (0.0, 0.0)]));
    concrete(ink, &v.all(&[(0.0, -0.3), (1.5, -0.3), (1.5, 1.2), (1.3, 1.2), (1.3, 0.0), (0.0, 0.0)]), 201);
    ink.fill(WHITE);
    ink.pen(0.4, BLACK);
    ink.circle(v.p(1.4, 0.2), 0.15 * v.k, "B");
    concrete(ink, &v.all(&[(1.5, -0.3), (3.0, -0.3), (3.0, -0.2), (1.5, -0.2)]), 203);
    ink.pen(0.35, STORM);
    v.line(ink, (1.5, -0.2), (6.0, -0.4));
    v.line(ink, (1.5, 0.25), (6.0, 0.05));
    v.dims_y(ink, &[0.0, 1.0, 1.2], v.p(-0.8, 0.0).0, v.p(-0.1, 0.0).0);
    let r = |x: f64, z: f64| v.p(x, z);
    callout(ink, r(1.4, 0.35), (r(2.0, 0.0).0 + 80.0, r(0.0, 1.4).1), 2.3, &["300 ORIFICE PLATE, STAINLESS"]);
    callout(ink, r(1.4, 1.2), (r(2.0, 0.0).0 + 80.0, r(0.0, 1.8).1), 2.3, &["WEIR CREST AT TOP WATER LEVEL"]);
    callout(ink, r(4.0, -0.25), (r(2.0, 0.0).0 + 80.0, r(0.0, -0.8).1), 2.3, &["450Ø RCP TO CREEK HEADWALL"]);
    title(ink, (40.0, 300.0), "1", "BASIN OUTLET STRUCTURE", "1:20");
    let s = View { ox: 250.0, oy: 180.0, k: 25.0 };
    earth(ink, &s.all(&[(-8.0, 1.5), (-6.0, 1.5), (0.0, 0.0), (6.0, 0.0), (8.0, -0.3)]));
    aggregate(ink, &s.all(&[(-6.0, 1.5), (0.0, 0.0), (6.0, 0.0), (6.0, -0.4), (0.0, -0.4), (-6.0, 1.1)]), 211, 1.2);
    s.dims_x(ink, &[-6.0, 0.0, 6.0], s.p(0.0, 2.2).1, s.p(0.0, 1.6).1);
    title(ink, (40.0, 110.0), "2", "SPILLWAY, ROCK LINED", "1:50");
    vec![("Detail 2", (30.0, 62.0, 830.0, 240.0), 50.0)]
}

/// Which details are on which sheet, and each sheet's scale.
pub const DETAILS: [(&str, &str, f64); 9] = [
    ("KERBS AND KERB RAMPS", "AS SHOWN", 10.0),
    ("FOOTPATHS AND DRIVEWAYS", "AS SHOWN", 10.0),
    ("PIPE BEDDING", "1:20", 20.0),
    ("KERB INLET PITS", "AS SHOWN", 10.0),
    ("HEADWALLS", "1:25", 25.0),
    ("SEWER MANHOLES", "1:25", 25.0),
    ("RETAINING WALLS", "1:10", 10.0),
    ("SIGNS AND LINEMARKING", "AS SHOWN", 20.0),
    ("BASIN OUTLETS AND SPILLWAYS", "AS SHOWN", 20.0),
];

pub fn detail_sheet(ink: &mut Ink, k: usize) -> Vec<(&'static str, Bx, f64)> {
    let vps = match k {
        0 => kerbs(ink),
        1 => paths(ink),
        2 => bedding(ink),
        3 => pits(ink),
        4 => headwalls(ink),
        5 => manholes(ink),
        6 => walls(ink),
        7 => signs(ink),
        _ => outlet(ink),
    };
    let _ = text_width;
    vps
}
