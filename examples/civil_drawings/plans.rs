//! The plans: each sector of the site at 1:500 in every discipline, the
//! whole site at 1:2500, and the basins.

use crate::draw::*;
use crate::geom::*;
use crate::sample_ink::{grey, north, text_width, Align, Font, Ink, View, BLACK, WHITE};
use crate::town::*;
use crate::{Disc, Sector, Set};

/// How a sector's plan sits on its sheet.
pub fn sector_view(s: &Sector) -> View {
    View { ox: 30.0 - s.bx.0 * 2.0, oy: 112.0 - s.bx.1 * 2.0, k: 2.0 }
}

/// The right-hand column.
const COL: f64 = 648.0;
const COL_W: f64 = 178.0;

#[derive(Clone, Copy)]
pub struct Base {
    /// Contours of the ground, and the lots' areas: the full plan.
    pub full: bool,
    /// Greyed back, under a discipline's own work.
    pub faded: bool,
    /// The carriageway and paths filled.
    pub fills: bool,
}

fn fade(faded: bool) -> [f64; 3] {
    if faded { grey(0.55) } else { BLACK }
}

/// The roads, kerbs, paths, lots, basins and creek in `window`, over the
/// ground's contours on a full plan.
pub fn base(ink: &mut Ink, v: View, town: &Town, window: Bx, style: Base) {
    let near = grow(window, 40.0);
    let view = v.all(&rect_pts(window));
    if style.full {
        ground_contours(ink, v, window, CONTOUR, true);
    }
    creek(ink, v, town, window);
    let islands: Vec<&Island> = town.islands.iter().filter(|i| overlaps(bounds(&i.lip), near)).collect();
    if style.fills {
        ink.fill(if style.faded { grey(0.93) } else { ROAD });
        ink.path(&view, true);
        for isl in &islands {
            ink.path(&v.all(&clip_polygon(&isl.lip, grow(window, 5.0))), true);
        }
        ink.op("f*");
        for isl in islands.iter().filter(|i| i.has_verge()) {
            let (outer, inner) = (isl.ring(|g| g - 0.3), isl.ring(|g| g - 1.5));
            ink.fill(if style.faded { grey(0.97) } else { PATH });
            ink.path(&v.all(&clip_polygon(&outer, grow(window, 5.0))), true);
            ink.path(&v.all(&clip_polygon(&inner, grow(window, 5.0))), true);
            ink.op("f*");
        }
    }
    let line = fade(style.faded);
    for isl in &islands {
        if isl.has_verge() && style.fills {
            ink.pen(0.08, grey(if style.faded { 0.72 } else { 0.5 }));
            for d in [0.3, 1.5] {
                for run in clip_polyline(&closed(&isl.ring(|g| g - d)), grow(window, 2.0)) {
                    ink.poly(&v.all(&run), false, "S");
                }
            }
        }
        ink.pen(if style.faded { 0.18 } else { 0.3 }, line);
        for run in clip_polyline(&closed(&isl.lip), grow(window, 2.0)) {
            ink.poly(&v.all(&run), false, "S");
        }
        if isl.use_ != Use::Median {
            ink.pen(0.12, line);
            for run in clip_polyline(&closed(&isl.back), grow(window, 2.0)) {
                ink.poly(&v.all(&run), false, "S");
            }
            ink.pen(if style.faded { 0.25 } else { 0.4 }, line);
            for run in clip_polyline(&closed(&isl.boundary), grow(window, 2.0)) {
                ink.poly(&v.all(&run), false, "S");
            }
        }
    }

    // Lots, cut to their blocks.
    let lots: Vec<&Lot> = town.lots.iter().filter(|l| overlaps(l.bx, near)).collect();
    for isl in islands.iter().filter(|i| i.use_ == Use::Lots) {
        let mine: Vec<&&Lot> = lots.iter().filter(|l| l.island == isl.id).collect();
        if mine.is_empty() {
            continue;
        }
        ink.clip(&v.all(&isl.boundary));
        ink.pen(if style.faded { 0.13 } else { 0.2 }, line);
        for lot in &mine {
            v.rect(ink, lot.bx.0, lot.bx.1, lot.bx.2, lot.bx.3, "S");
        }
        ink.restore();
    }
    for lot in &lots {
        let c = centre(lot.bx);
        let p = v.p(c.0, c.1 + if style.full { 1.5 } else { 0.0 });
        ink.fill(if style.faded { grey(0.55) } else { BLACK });
        ink.text((p.0, p.1 - 0.8), if style.faded { 2.2 } else { 3.0 }, Font::Bold, Align::Centre, &lot.number.to_string());
        if style.full {
            ink.text((p.0, p.1 - 4.6), 1.9, Font::Regular, Align::Centre, &format!("{:.0} m²", lot.area));
        }
    }
    if style.full {
        // Driveway crossings through the verge.
        for lot in &lots {
            let out = lot.front.out();
            let road = town.road_at(add(lot.driveway, mul(out, 2.0)));
            let Some(road) = road else { continue };
            let t = left(out);
            let verge = road.kind.verge() - 0.45;
            let pts = [add(lot.driveway, mul(t, 1.5)), add(lot.driveway, mul(t, -1.5)), add(add(lot.driveway, mul(t, -2.0)), mul(out, verge)), add(add(lot.driveway, mul(t, 2.0)), mul(out, verge))];
            ink.fill(grey(0.88));
            ink.pen(0.12, grey(0.3));
            ink.poly(&v.all(&pts), true, "B");
        }
    }

    // What's not lots.
    for isl in &islands {
        let label = isl.use_.label();
        if label.is_empty() || !overlaps(isl.property, window) {
            continue;
        }
        let pb = isl.property;
        let c = centre((pb.0.max(window.0), pb.1.max(window.1), pb.2.min(window.2).min(LOTS_EAST + 10.0), pb.3.min(window.3)));
        if isl.use_ == Use::Corridor || c.0 > LOTS_EAST + 5.0 {
            continue;
        }
        let area = (pb.2.min(LOTS_EAST) - pb.0.max(0.0)) * (pb.3.min(town.height) - pb.1.max(0.0)) / 10_000.0;
        let p = v.p(c.0, c.1);
        ink.fill(if style.faded { grey(0.5) } else { BLACK });
        ink.text((p.0, p.1 + 1.0), 4.0, Font::Bold, Align::Centre, label);
        ink.text((p.0, p.1 - 4.5), 2.4, Font::Regular, Align::Centre, &format!("{area:.2} ha"));
    }

    // Centrelines, chainages, and the names of the roads.
    for road in &town.roads {
        if !overlaps(road.reserve_box(), near) {
            continue;
        }
        let pts = road.centreline();
        ink.pen(0.16, if style.faded { grey(0.6) } else { BLACK });
        ink.dash(&[6.0, 1.2, 1.0, 1.2]);
        for run in clip_polyline(&pts, grow(window, 2.0)) {
            ink.poly(&v.all(&run), false, "S");
        }
        ink.dash(&[]);
        if !style.faded {
            let mut ch = 0.0;
            while ch <= road.length() {
                let p = road.point(ch, 0.0);
                if in_box(p, window) {
                    let n = left(road.dir());
                    v.line(ink, sub(p, mul(n, 1.0)), add(p, mul(n, 1.0)));
                    if style.full {
                        let at = add(p, mul(n, 1.5));
                        ink.fill(BLACK);
                        ink.text_rotated(v.p(at.0, at.1), 1.8, Font::Regular, Align::Left, &format!("{ch:.0}"), upright(n));
                    }
                }
                ch += 20.0;
            }
        }
        let mut ch = 60.0;
        while ch < road.length() - 20.0 {
            let clear = road.crossings.iter().all(|c| (c.0 - road.from - ch).abs() > 30.0);
            let p = road.point(ch, -road.kind.half() / 2.0 - road.kind.median() / 2.0);
            if clear && in_box(p, grow(window, -15.0)) {
                along(ink, v.p(p.0, p.1), upright(road.dir()), 3.0, Font::Bold, grey(if style.faded { 0.5 } else { 0.2 }), &road.name.to_uppercase());
            }
            ch += 170.0;
        }
    }

    for b in town.basins.iter().filter(|b| overlaps(b.bx(), near)) {
        basin(ink, v, b, style.faded);
    }

    // Outside the site, faded back.
    let site = town.site();
    ink.save();
    ink.op("/Fade gs");
    ink.fill(WHITE);
    ink.path(&view, true);
    ink.path(&v.all(&rect_pts(site)), true);
    ink.op("f*");
    ink.restore();
    ink.pen(0.8, BOUNDARY);
    ink.dash(&[8.0, 1.5, 1.5, 1.5]);
    v.rect(ink, site.0, site.1, site.2, site.3, "S");
    ink.dash(&[]);
}

pub fn ground_contours(ink: &mut Ink, v: View, window: Bx, colour: [f64; 3], labels: bool) {
    let lines = contours(grow(window, 4.0), 2.0, 0.25, ground);
    for (level, pts) in &lines {
        let major = (level - level.round()).abs() < 1e-6;
        ink.pen(if major { 0.18 } else { 0.07 }, colour);
        ink.poly(&v.all(pts), false, "S");
    }
    if labels {
        for (level, pts) in lines.iter().filter(|(l, _)| (l - l.round()).abs() < 1e-6) {
            for (p, u) in stations(pts, 40.0, 120.0) {
                if in_box(p, grow(window, -5.0)) {
                    tag(ink, v.p(p.0, p.1), upright(u), 1.7, Font::Regular, colour, &format!("{level:.1}"));
                }
            }
        }
    }
}

fn creek(ink: &mut Ink, v: View, town: &Town, window: Bx) {
    if window.2 < 1140.0 {
        return;
    }
    let line = |off: f64| -> Vec<P> {
        let n = ((town.height + 40.0) / 2.0) as usize;
        (0..=n).map(|i| -20.0 + i as f64 * 2.0).map(|y| (creek_x(y) + off, y)).collect()
    };
    let mut water = line(-3.0);
    water.extend(line(3.0).into_iter().rev());
    ink.fill(CREEK);
    ink.poly(&v.all(&water), true, "f");
    ink.pen(0.18, WATER);
    for off in [-3.0, 3.0] {
        ink.poly(&v.all(&line(off)), false, "S");
    }
    ink.pen(0.25, grey(0.35));
    ink.dash(&[3.0, 1.5]);
    for off in [-10.0, 10.0] {
        ink.poly(&v.all(&line(off)), false, "S");
    }
    ink.dash(&[]);
    ink.pen(0.1, grey(0.45));
    let mut i = 0;
    let mut y = window.1;
    while y < window.3 {
        let reach = if i % 2 == 0 { 6.0 } else { 3.0 };
        let x = creek_x(y);
        v.line(ink, (x - 10.0, y), (x - 10.0 + reach, y));
        v.line(ink, (x + 10.0, y), (x + 10.0 - reach, y));
        y += 2.5;
        i += 1;
    }
    let mut y = window.1 + 60.0;
    while y < window.3 - 30.0 {
        let slope = (creek_x(y + 1.0) - creek_x(y - 1.0)) / 2.0;
        tag(ink, v.p(creek_x(y), y), upright((slope, 1.0)), 2.8, Font::Bold, [0.1, 0.4, 0.7], "KINGFISHER CREEK");
        y += 160.0;
    }
}

pub fn basin(ink: &mut Ink, v: View, b: &Basin, faded: bool) {
    let top = b.ring(0.0);
    let reach = b.reach();
    let toe = b.ring(b.top - b.floor);
    ink.fill(if faded { grey(0.96) } else { [0.9, 0.95, 0.88] });
    ink.poly(&v.all(&toe), true, "f");
    ink.pen(0.1, grey(0.4));
    for (i, (p, u)) in stations(&closed(&top), 0.0, 2.0).into_iter().enumerate() {
        let l = if i % 2 == 0 { reach } else { reach / 2.0 };
        v.line(ink, p, add(p, mul(left(u), l)));
    }
    ink.pen(0.35, BLACK);
    ink.poly(&v.all(&top), true, "S");
    ink.pen(0.2, BLACK);
    ink.dash(&[2.0, 1.0]);
    ink.poly(&v.all(&toe), true, "S");
    ink.dash(&[]);
    let (x0, y0, x1, y1) = (b.corners[0].0, b.corners[0].1, b.corners[2].0, b.corners[2].1);
    let channel = [((x0 + x1) / 2.0, y0 + reach), ((x0 + x1) / 2.0, y1 - reach)];
    ink.pen(0.9, grey(0.55));
    ink.poly(&v.all(&channel), false, "S");
    ink.pen(0.5, grey(0.93));
    ink.poly(&v.all(&channel), false, "S");
    if faded {
        return;
    }
    let c = v.p((x0 + x1) / 2.0 - 8.0, (y0 + y1) / 2.0);
    ink.fill(BLACK);
    ink.text_turned((c.0, c.1), 3.2, Font::Bold, Align::Centre, &format!("BASIN {}", &b.name[1..]), true);
    ink.text_turned((c.0 + 4.0, c.1), 2.0, Font::Regular, Align::Centre, &format!("FLOOR RL {:.2}  ·  TWL {:.2}", b.floor, b.twl), true);
    ink.text_turned((c.0 + 7.0, c.1), 2.0, Font::Regular, Align::Centre, &format!("TOP OF BANK RL {:.2}", b.top), true);
}

/// The pits and pipes: light over a plan, or heavy and labelled.
pub fn drainage(ink: &mut Ink, v: View, town: &Town, window: Bx, heavy: bool) {
    let near = grow(window, 10.0);
    let colour = if heavy { STORM } else { grey(0.3) };
    for pipe in &town.pipes {
        let (a, b) = (town.pits[pipe.from].at, town.pits[pipe.to].at);
        if clip_segment(a, b, near).is_none() {
            continue;
        }
        ink.pen(if heavy { 0.2 + pipe.dia * 0.6 } else { 0.3 }, colour);
        v.line(ink, a, b);
        if heavy && pipe.length > 16.0 {
            let u = unit(sub(b, a));
            let (pa, pb) = (v.p(a.0, a.1), v.p(b.0, b.1));
            let tip = add(pa, mul(unit(sub(pb, pa)), dist(a, b) * v.k * 0.62));
            ink.fill(colour);
            arrowhead(ink, tip, u, 2.2);
            let mid = mul(add(a, b), 0.5);
            let n = left(u);
            let side = if n.1 > 0.0 || (n.1 == 0.0 && n.0 < 0.0) { 1.0 } else { -1.0 };
            let at = add(v.p(mid.0, mid.1), mul(n, 2.6 * side));
            if in_box(mid, window) {
                tag(ink, at, upright(u), 1.7, Font::Regular, colour, &format!("{:.0}Ø @ {:.2}%", pipe.dia * 1000.0, pipe.grade * 100.0));
            }
        }
    }
    for pit in town.pits.iter().filter(|p| in_box(p.at, near)) {
        let (u, n) = (pit.dir, left(pit.dir));
        let corners = |l: f64, w: f64| -> Vec<P> {
            [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().map(|&(a, b)| add(pit.at, add(mul(u, a * l / 2.0), mul(n, b * w / 2.0)))).collect()
        };
        ink.fill(WHITE);
        ink.pen(if heavy { 0.3 } else { 0.2 }, colour);
        match pit.kind {
            PitKind::Kerb(l) => {
                ink.poly(&v.all(&corners(l, 0.9)), true, "B");
                ink.pen(if heavy { 0.6 } else { 0.35 }, colour);
                let k = sub(pit.at, mul(pit.out, 0.45));
                v.line(ink, sub(k, mul(u, l / 2.0)), add(k, mul(u, l / 2.0)));
            }
            PitKind::Junction => ink.poly(&v.all(&corners(1.2, 1.2)), true, "B"),
            PitKind::Headwall => {
                ink.pen(if heavy { 0.7 } else { 0.4 }, colour);
                let (a, b) = (add(pit.at, mul(u, 1.6)), sub(pit.at, mul(u, 1.6)));
                v.line(ink, a, b);
                ink.pen(0.25, colour);
                v.line(ink, a, add(a, add(mul(pit.out, 1.2), mul(u, 0.8))));
                v.line(ink, b, add(b, sub(mul(pit.out, 1.2), mul(u, 0.8))));
            }
            PitKind::Outlet => {
                let c = corners(1.5, 1.5);
                ink.poly(&v.all(&c), true, "B");
                v.line(ink, c[0], c[2]);
                v.line(ink, c[1], c[3]);
            }
        }
        if heavy && in_box(pit.at, window) {
            let at = add(pit.at, mul(pit.out, 4.8));
            let p = v.p(at.0, at.1);
            ink.pen(0.12, colour);
            let edge = add(pit.at, mul(pit.out, 0.9));
            ink.line(v.p(edge.0, edge.1), p);
            let w = text_width(&pit.name, Font::Bold) * 1.9 + 1.8;
            ink.fill(WHITE);
            ink.pen(0.2, colour);
            ink.rect(p.0 - w / 2.0, p.1 - 1.6, w, 3.2, "B");
            ink.fill(colour);
            ink.text((p.0, p.1 - 0.7), 1.9, Font::Bold, Align::Centre, &pit.name);
        }
    }
}

/// Cut and fill in bands, red cut to blue fill.
pub const CUT_FILL: [(f64, [f64; 3], &str); 8] = [
    (-1.0, [0.72, 0.1, 0.1], "Cut over 1.0"),
    (-0.5, [0.9, 0.35, 0.3], "Cut 0.5 to 1.0"),
    (-0.2, [0.97, 0.65, 0.6], "Cut 0.2 to 0.5"),
    (0.0, [1.0, 0.88, 0.85], "Cut to 0.2"),
    (0.2, [0.87, 0.92, 1.0], "Fill to 0.2"),
    (0.5, [0.65, 0.78, 0.97], "Fill 0.2 to 0.5"),
    (1.0, [0.35, 0.55, 0.9], "Fill 0.5 to 1.0"),
    (f64::INFINITY, [0.12, 0.3, 0.75], "Fill over 1.0"),
];

pub fn cut_fill_colour(d: f64) -> [f64; 3] {
    if d < -1.0 {
        return CUT_FILL[0].1;
    }
    CUT_FILL.iter().skip(1).find(|b| d < b.0).map_or(CUT_FILL[7].1, |b| b.1)
}

/// The depth of cut (negative) or fill at a point, where anything is done.
pub fn cut_fill(town: &Town, p: P) -> Option<f64> {
    town.design(p).map(|d| d - ground(p.0, p.1))
}

/// Volumes of cut and fill in a box, from `step`-metre cells.
pub fn volumes(town: &Town, bx: Bx, step: f64) -> (f64, f64) {
    let (mut cut, mut fill) = (0.0, 0.0);
    let mut y = bx.1 + step / 2.0;
    while y < bx.3 {
        let mut x = bx.0 + step / 2.0;
        while x < bx.2 {
            if let Some(d) = cut_fill(town, (x, y)) {
                if d < 0.0 { cut -= d * step * step } else { fill += d * step * step }
            }
            x += step;
        }
        y += step;
    }
    (cut, fill)
}

// ---------------------------------------------------------------------------
// A sector's sheet

pub fn sector_sheet(ink: &mut Ink, set: &Set, town: &Town, disc: Disc, s: &Sector) -> View {
    let v = sector_view(s);
    let w = s.bx;
    let frame_pts = v.all(&rect_pts(w));
    ink.clip(&frame_pts);
    let (base_style, heavy_pits) = match disc {
        Disc::Ga => (Base { full: true, faded: false, fills: true }, false),
        Disc::CutFill => (Base { full: false, faded: true, fills: false }, false),
        _ => (Base { full: false, faded: true, fills: true }, false),
    };
    if disc == Disc::CutFill {
        heatmap(ink, v, town, w, 5.0);
    }
    base(ink, v, town, w, base_style);
    match disc {
        Disc::Ga => {
            drainage(ink, v, town, w, heavy_pits);
            for road in town.roads.iter().filter(|r| overlaps(r.reserve_box(), w)) {
                ga_dims(ink, v, road, w);
            }
        }
        Disc::Setout => setout_overlay(ink, v, town, w),
        Disc::CutFill => cut_fill_values(ink, v, town, w),
        Disc::Grading => grading_overlay(ink, v, town, w),
        Disc::Drainage => {
            catchments(ink, v, town, w);
            drainage(ink, v, town, w, true);
        }
        Disc::WaterSewer => water_sewer(ink, v, town, w),
        Disc::Electrical => electrical(ink, v, town, w),
        Disc::Signage => signage(ink, v, town, w),
        Disc::Landscape => landscape(ink, v, town, w),
    }
    ink.restore();
    ink.pen(0.35, BLACK);
    ink.poly(&frame_pts, true, "S");

    // Match lines to the sectors round it.
    for (dc, dr, edge) in [(-1i32, 0i32, 3usize), (1, 0, 1), (0, -1, 0), (0, 1, 2)] {
        let (c, r) = (s.col as i32 + dc, s.row as i32 + dr);
        let Some(next) = set.sectors.iter().find(|o| o.col as i32 == c && o.row as i32 == r) else { continue };
        let pts = rect_pts(w);
        let (a, b) = (pts[edge], pts[(edge + 1) % 4]);
        ink.pen(1.0, MATCH);
        ink.dash(&[10.0, 2.0, 2.0, 2.0]);
        v.line(ink, a, b);
        ink.dash(&[]);
        let mid = v.p((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let inward = mul(unit(sub(v.p(centre(w).0, centre(w).1), mid)), 4.0);
        let angle = upright(unit(sub(b, a)));
        tag(ink, add(mid, inward), angle, 2.6, Font::Bold, MATCH, &format!("MATCH LINE  ·  SEE {}", set.sector_sheet(disc, next.id)));
    }
    north(ink, (604.0, 505.0), 7.0);
    title(ink, (30.0, 88.0), &format!("{}", s.id + 1), &format!("{}  ·  SECTOR {}", disc.title(), s.id + 1), "1:500 @ A1");
    bar(ink, (330.0, 86.0), 2.0, 10.0, 5, "SCALE 1:500");

    // The column: legend, the discipline's table, notes, key plan.
    let y = legend_for(ink, disc, 560.0);
    let y = column_table(ink, set, town, disc, s, y - 6.0);
    let notes = disc_notes(set, disc);
    notes_block(ink, COL, y - 8.0, COL_W, "NOTES", &notes);
    key_plan(ink, set, town, disc, s.id, (COL, 64.0, COL + 170.0, 64.0 + 170.0 * town.height / SITE_W));
    v
}

fn ga_dims(ink: &mut Ink, v: View, road: &Road, window: Bx) {
    // Across the road once in the sector, clear of the junctions.
    let mut ch = 45.0;
    while ch < road.length() {
        let p = road.point(ch, 0.0);
        let clear = road.crossings.iter().all(|c| (c.0 - road.from - ch).abs() > 35.0);
        if clear && in_box(p, grow(window, -25.0)) {
            let k = road.kind;
            let mut offsets = vec![-k.reserve(), -k.half()];
            if k.median() > 0.0 {
                offsets.extend([-k.median(), k.median()]);
            }
            offsets.extend([k.half(), k.reserve()]);
            let n = left(road.dir());
            let pts: Vec<(f64, f64)> = offsets.iter().map(|&o| add(p, mul(n, o))).map(|q| v.p(q.0, q.1)).collect();
            ink.pen(0.13, BLACK);
            ink.line(pts[0], pts[pts.len() - 1]);
            let slash = mul(add(road.dir(), n), 1.0 / 2f64.sqrt());
            ink.pen(0.35, BLACK);
            for q in &pts {
                ink.line(sub(*q, mul(slash, 1.0)), add(*q, mul(slash, 1.0)));
            }
            for (i, w) in pts.windows(2).enumerate() {
                let mid = mul(add(w[0], w[1]), 0.5);
                let at = add(mid, mul(road.dir(), 1.5));
                ink.fill(BLACK);
                ink.text_rotated(at, 1.8, Font::Regular, Align::Centre, &format!("{:.2}", offsets[i + 1] - offsets[i]), upright(n));
            }
            return;
        }
        ch += 40.0;
    }
}

fn heatmap(ink: &mut Ink, v: View, town: &Town, w: Bx, step: f64) {
    let mut y = (w.1 / step).floor() * step;
    while y < w.3 {
        let mut x = (w.0 / step).floor() * step;
        while x < w.2 {
            if let Some(d) = cut_fill(town, (x + step / 2.0, y + step / 2.0)) {
                ink.fill(cut_fill_colour(d));
                v.rect(ink, x, y, x + step, y + step, "f");
            }
            x += step;
        }
        y += step;
    }
}

fn cut_fill_values(ink: &mut Ink, v: View, town: &Town, w: Bx) {
    // A grid every 10 metres, and the depth in each square.
    let step = 10.0;
    ink.pen(0.08, grey(0.45));
    let mut x = (w.0 / step).ceil() * step;
    while x < w.2 {
        v.line(ink, (x, w.1), (x, w.3));
        x += step;
    }
    let mut y = (w.1 / step).ceil() * step;
    while y < w.3 {
        v.line(ink, (w.0, y), (w.2, y));
        y += step;
    }
    let mut y = (w.1 / step).floor() * step;
    while y < w.3 {
        let mut x = (w.0 / step).floor() * step;
        while x < w.2 {
            let c = (x + step / 2.0, y + step / 2.0);
            if let Some(d) = cut_fill(town, c) {
                let p = v.p(c.0, c.1);
                ink.fill(if d.abs() > 0.5 { WHITE } else { BLACK });
                ink.text((p.0, p.1 + 0.4), 2.3, Font::Bold, Align::Centre, &format!("{d:+.2}"));
                let g = ground(c.0, c.1);
                ink.fill(if d.abs() > 0.5 { WHITE } else { grey(0.3) });
                ink.text((p.0, p.1 - 3.2), 1.5, Font::Regular, Align::Centre, &format!("{:.2}/{:.2}", g, g + d));
            }
            x += step;
        }
        y += step;
    }
    // The lines of no cut and no fill.
    let zero = contours(w, 2.0, 1000.0, |x, y| cut_fill(town, (x, y)).unwrap_or(0.0) + 1000.0);
    ink.pen(0.35, BLACK);
    ink.dash(&[1.2, 0.8]);
    for (_, pts) in &zero {
        ink.poly(&v.all(pts), false, "S");
    }
    ink.dash(&[]);
}

fn grading_overlay(ink: &mut Ink, v: View, town: &Town, w: Bx) {
    // The shaped surface's contours, the pads' levels, the retaining walls,
    // and the kerb levels along the roads.
    let lines = contours(grow(w, 4.0), 2.0, 0.25, shaped);
    for (level, pts) in &lines {
        let major = (level - level.round()).abs() < 1e-6;
        ink.pen(if major { 0.22 } else { 0.1 }, [0.3, 0.45, 0.7]);
        ink.dash(if major { &[] } else { &[1.5, 0.8] });
        ink.poly(&v.all(pts), false, "S");
    }
    ink.dash(&[]);
    for (level, pts) in lines.iter().filter(|(l, _)| (l - l.round()).abs() < 1e-6) {
        for (p, u) in stations(pts, 30.0, 110.0) {
            if in_box(p, grow(w, -5.0)) {
                tag(ink, v.p(p.0, p.1), upright(u), 1.7, Font::Regular, [0.3, 0.45, 0.7], &format!("{level:.1}"));
            }
        }
    }
    for lot in town.lots.iter().filter(|l| overlaps(l.bx, w)) {
        let c = centre(lot.bx);
        let p = v.p(c.0, c.1 - 3.0);
        tag(ink, (p.0, p.1 - 1.5), 0.0, 2.1, Font::Bold, [0.1, 0.3, 0.6], &format!("FSL {:.2}", lot.fsl));
        // It falls to its frontage.
        let out = lot.front.out();
        let from = add(c, mul(out, lot.depth() * 0.1));
        let to = add(c, mul(out, lot.depth() * 0.32));
        ink.pen(0.2, [0.1, 0.3, 0.6]);
        v.line(ink, from, to);
        ink.fill([0.1, 0.3, 0.6]);
        arrowhead(ink, v.p(to.0, to.1), out, 1.8);
        for corner in rect_pts(lot.bx).iter().filter(|q| in_box(**q, w)) {
            let q = v.p(corner.0, corner.1);
            ink.fill(grey(0.2));
            ink.circle(q, 0.35, "f");
        }
    }
    for wall in town.walls.iter().filter(|x| clip_segment(x.a, x.b, w).is_some()) {
        ink.pen(0.9, [0.55, 0.25, 0.1]);
        v.line(ink, wall.a, wall.b);
        let u = unit(sub(wall.b, wall.a));
        let len = dist(wall.a, wall.b);
        ink.pen(0.2, [0.55, 0.25, 0.1]);
        let mut s = 1.0;
        while s < len {
            let p = add(wall.a, mul(u, s));
            v.line(ink, p, add(p, mul(left(u), 0.8)));
            s += 1.5;
        }
        let mid = add(wall.a, mul(u, len / 2.0));
        tag(ink, v.p(mid.0, mid.1), upright(u), 1.6, Font::Bold, [0.55, 0.25, 0.1], &format!("RW {:.2}  TW {:.2}", wall.height, wall.top));
    }
    for road in town.roads.iter().filter(|r| overlaps(r.reserve_box(), w)) {
        let mut ch = 10.0;
        while ch < road.length() {
            for side in [-1.0, 1.0] {
                let p = road.point(ch, side * (road.kind.half() - 0.8));
                if in_box(p, grow(w, -3.0)) {
                    let q = v.p(p.0, p.1);
                    ink.fill([0.1, 0.3, 0.6]);
                    ink.text_rotated(q, 1.5, Font::Regular, Align::Centre, &format!("{:.2}", road.lip(ch)), upright(road.dir()));
                }
            }
            ch += 20.0;
        }
    }
}

fn setout_overlay(ink: &mut Ink, v: View, town: &Town, w: Bx) {
    for (id, p) in town.kerb_points().iter().filter(|(_, p)| in_box(*p, w)) {
        let q = v.p(p.0, p.1);
        ink.pen(0.2, BOUNDARY);
        ink.line((q.0 - 1.2, q.1), (q.0 + 1.2, q.1));
        ink.line((q.0, q.1 - 1.2), (q.0, q.1 + 1.2));
        ink.fill(BOUNDARY);
        ink.text((q.0 + 1.0, q.1 + 0.8), 1.5, Font::Regular, Align::Left, id);
    }
    for road in town.roads.iter().filter(|r| overlaps(r.reserve_box(), w)) {
        let mut ch = 0.0;
        while ch <= road.length() {
            let p = road.point(ch, 0.0);
            if in_box(p, w) {
                let q = v.p(p.0, p.1);
                ink.pen(0.25, BLACK);
                ink.circle(q, 0.7, "S");
                ink.fill(BLACK);
                ink.text_rotated(add(q, (0.9, 0.9)), 1.5, Font::Regular, Align::Left, &format!("{}-{ch:.0}", road.code), upright(road.dir()) + 0.6);
            }
            ch += 20.0;
        }
    }
}

fn catchments(ink: &mut Ink, v: View, town: &Town, w: Bx) {
    let tints = [[0.35, 0.65, 0.95], [0.4, 0.8, 0.55], [0.95, 0.75, 0.3], [0.85, 0.5, 0.8]];
    for (k, line) in town.lines.iter().enumerate() {
        let pits: Vec<&Pit> = line.pits.iter().map(|&i| &town.pits[i]).filter(|p| p.road.is_some()).collect();
        if pits.is_empty() {
            continue;
        }
        let road = &town.roads[line.road];
        let depth = if road.axis == Axis::EastWest { road.kind.reserve() + 30.0 } else { road.kind.reserve() };
        let chs: Vec<f64> = pits.iter().map(|p| p.ch).collect();
        let (c0, c1) = (chs.iter().cloned().fold(f64::MAX, f64::min) - 30.0, chs.iter().cloned().fold(f64::MIN, f64::max) + 5.0);
        let pts = [road.point(c0.max(0.0), -depth), road.point(c1.min(road.length()), -depth), road.point(c1.min(road.length()), depth), road.point(c0.max(0.0), depth)];
        let bx = bounds(&pts);
        if !overlaps(bx, w) {
            continue;
        }
        let poly = clip_polygon(&rect_pts(bx), grow(w, 2.0));
        ink.save();
        ink.op("/Tint gs");
        ink.fill(tints[k % tints.len()]);
        ink.poly(&v.all(&poly), true, "f");
        ink.restore();
        ink.pen(0.25, grey(0.35));
        ink.dash(&[2.5, 1.0, 0.5, 1.0]);
        ink.poly(&v.all(&poly), true, "S");
        ink.dash(&[]);
        // Where each pit's own part begins.
        ink.pen(0.12, grey(0.4));
        ink.dash(&[1.0, 1.0]);
        for w2 in chs.windows(2) {
            let mid = (w2[0] + w2[1]) / 2.0;
            v.line(ink, road.point(mid, -depth), road.point(mid, depth));
        }
        ink.dash(&[]);
        let ha: f64 = pits.iter().map(|p| p.catchment).sum();
        let at = road.point(((c0 + c1) / 2.0).clamp(0.0, road.length()), depth - 6.0);
        if in_box(at, w) {
            tag(ink, v.p(at.0, at.1), upright(road.dir()), 2.4, Font::Bold, grey(0.2), &format!("CATCHMENT {}  ·  {ha:.2} ha", line.name));
        }
    }
}

fn water_sewer(ink: &mut Ink, v: View, town: &Town, w: Bx) {
    for isl in town.islands.iter().filter(|i| i.has_verge() && i.use_ != Use::Median && overlaps(bounds(&i.lip), grow(w, 20.0))) {
        let ring = isl.ring(|g| g - 2.3);
        ink.pen(0.45, WATER);
        for run in clip_polyline(&closed(&ring), grow(w, 2.0)) {
            ink.poly(&v.all(&run), false, "S");
        }
        for (p, u) in stations(&closed(&ring), 30.0, 90.0) {
            if in_box(p, grow(w, -4.0)) {
                let size = if isl.sides.iter().flatten().any(|&r| town.roads[r].kind != Kind::Local) { "150 DICL" } else { "100 PVC" };
                tag(ink, v.p(p.0, p.1), upright(u), 1.6, Font::Bold, WATER, &format!("W {size}"));
            }
        }
    }
    for p in town.hydrants.iter().filter(|p| in_box(**p, w)) {
        let q = v.p(p.0, p.1);
        ink.fill(WATER);
        ink.circle(q, 1.3, "f");
        ink.fill(WHITE);
        ink.text((q.0, q.1 - 0.65), 1.8, Font::Bold, Align::Centre, "H");
    }
    for p in town.valves.iter().filter(|p| in_box(**p, w)) {
        let q = v.p(p.0, p.1);
        ink.fill(WHITE);
        ink.pen(0.3, WATER);
        ink.poly(&[(q.0 - 1.4, q.1 - 0.9), (q.0 + 1.4, q.1 + 0.9), (q.0 + 1.4, q.1 - 0.9), (q.0 - 1.4, q.1 + 0.9)], true, "B");
    }
    for sewer in &town.sewers {
        let pts: Vec<P> = sewer.manholes.iter().map(|m| m.at).collect();
        if clip_polyline(&pts, w).is_empty() {
            continue;
        }
        ink.pen(0.5, SEWER);
        ink.dash(&[4.0, 1.0, 1.0, 1.0]);
        ink.poly(&v.all(&pts), false, "S");
        ink.dash(&[]);
        for m in &sewer.manholes {
            let q = v.p(m.at.0, m.at.1);
            ink.fill(WHITE);
            ink.pen(0.3, SEWER);
            ink.circle(q, 1.4, "B");
            ink.fill(SEWER);
            ink.circle(q, 0.4, "f");
            if in_box(m.at, w) {
                ink.text((q.0 + 1.8, q.1 + 1.2), 1.7, Font::Bold, Align::Left, &m.name);
                ink.text((q.0 + 1.8, q.1 - 1.2), 1.4, Font::Regular, Align::Left, &format!("IL {:.2}", m.invert));
            }
        }
        let (a, b) = (pts[0], pts[pts.len() - 1]);
        let mid = mul(add(a, b), 0.5);
        if in_box(mid, w) {
            let u = unit(sub(b, a));
            let grade = (sewer.manholes[0].invert - sewer.manholes[sewer.manholes.len() - 1].invert) / dist(a, b);
            tag(ink, v.p(mid.0 + left(u).0 * 2.2, mid.1 + left(u).1 * 2.2), upright(u), 1.7, Font::Regular, SEWER, &format!("{}  150 PVC  1 IN {:.0}", sewer.name, 1.0 / grade));
            let (tip, dir) = (v.p(b.0, b.1), u);
            ink.fill(SEWER);
            arrowhead(ink, sub(tip, mul(dir, 2.5)), dir, 2.2);
        }
    }
}

fn service_ring(ink: &mut Ink, v: View, town: &Town, w: Bx, from_boundary: f64, colour: [f64; 3], letter: &str, width: f64) {
    for isl in town.islands.iter().filter(|i| i.has_verge() && i.use_ != Use::Median && overlaps(bounds(&i.lip), grow(w, 20.0))) {
        let ring = isl.ring(|g| g - from_boundary);
        ink.pen(width, colour);
        for run in clip_polyline(&closed(&ring), grow(w, 2.0)) {
            ink.poly(&v.all(&run), false, "S");
        }
        for (p, u) in stations(&closed(&ring), 14.0 + from_boundary * 9.0, 46.0) {
            if in_box(p, grow(w, -3.0)) {
                tag(ink, v.p(p.0, p.1), upright(u), 1.6, Font::Bold, colour, letter);
            }
        }
    }
}

fn electrical(ink: &mut Ink, v: View, town: &Town, w: Bx) {
    service_ring(ink, v, town, w, 0.6, ELECTRICITY, "E", 0.4);
    service_ring(ink, v, town, w, 1.1, COMMS, "C", 0.35);
    service_ring(ink, v, town, w, 2.8, GAS, "G", 0.35);
    for (p, arm) in town.lights.iter().filter(|(p, _)| in_box(*p, w)) {
        let c = v.p(p.0, p.1);
        let end = add(c, mul(*arm, 3.0));
        ink.pen(0.3, BLACK);
        ink.line(c, end);
        ink.fill(WHITE);
        ink.circle(c, 0.9, "B");
        ink.fill([1.0, 0.8, 0.1]);
        ink.circle(end, 0.9, "B");
    }
    for (i, p) in town.kiosks.iter().enumerate().filter(|(_, p)| in_box(**p, w)) {
        let c = v.p(p.0, p.1);
        ink.fill(WHITE);
        ink.pen(0.4, ELECTRICITY);
        ink.rect(c.0 - 3.0, c.1 - 2.2, 6.0, 4.4, "B");
        ink.fill(ELECTRICITY);
        ink.text((c.0, c.1 - 0.9), 2.2, Font::Bold, Align::Centre, "K");
        ink.text((c.0 + 3.8, c.1 + 1.0), 1.7, Font::Bold, Align::Left, &format!("KIOSK {}", i + 1));
    }
}

fn signage(ink: &mut Ink, v: View, town: &Town, w: Bx) {
    for m in &town.markings {
        for run in clip_polyline(&m.pts, grow(w, 2.0)) {
            ink.pen(if m.width > 0.2 { 0.9 } else { 0.35 }, BLACK);
            ink.dash(if m.broken && m.width <= 0.2 { &[6.0, 18.0] } else { &[] });
            ink.poly(&v.all(&run), false, "S");
        }
    }
    ink.dash(&[]);
    for (p, u) in town.humps.iter().filter(|(p, _)| in_box(*p, w)) {
        let n = left(*u);
        let road = town.road_at(*p);
        let half = road.map_or(3.5, |r| r.kind.half());
        let pts = [add(add(*p, mul(*u, -1.85)), mul(n, -half)), add(add(*p, mul(*u, 1.85)), mul(n, -half)), add(add(*p, mul(*u, 1.85)), mul(n, half)), add(add(*p, mul(*u, -1.85)), mul(n, half))];
        ink.fill(WHITE);
        ink.pen(0.3, BLACK);
        ink.poly(&v.all(&pts), true, "B");
        ink.clip(&v.all(&pts));
        ink.pen(0.2, BLACK);
        for k in -8..8 {
            let a = add(*p, mul(*u, k as f64 * 0.7));
            v.line(ink, add(a, mul(n, -half)), add(add(a, mul(*u, 2.0)), mul(n, half)));
        }
        ink.restore();
    }
    for sign in town.signs.iter().filter(|s| in_box(s.at, w)) {
        let c = v.p(sign.at.0, sign.at.1);
        match sign.kind {
            SignKind::Stop => {
                let pts: Vec<(f64, f64)> = (0..8).map(|k| std::f64::consts::PI / 8.0 + k as f64 * std::f64::consts::PI / 4.0).map(|a| (c.0 + 1.6 * a.cos(), c.1 + 1.6 * a.sin())).collect();
                ink.fill([0.85, 0.1, 0.1]);
                ink.poly(&pts, true, "f");
            }
            SignKind::GiveWay => {
                ink.fill(WHITE);
                ink.pen(0.45, [0.85, 0.1, 0.1]);
                ink.poly(&[(c.0 - 1.6, c.1 + 1.2), (c.0 + 1.6, c.1 + 1.2), (c.0, c.1 - 1.5)], true, "B");
            }
            SignKind::StreetName => {
                ink.fill([0.1, 0.35, 0.2]);
                ink.rect(c.0 - 2.0, c.1 - 0.6, 4.0, 1.2, "f");
            }
            SignKind::Crossing | SignKind::Hump => {
                ink.fill([1.0, 0.85, 0.1]);
                ink.pen(0.2, BLACK);
                ink.poly(&[(c.0, c.1 + 1.6), (c.0 + 1.6, c.1), (c.0, c.1 - 1.6), (c.0 - 1.6, c.1)], true, "B");
            }
            SignKind::BusZone => {
                ink.fill(WHITE);
                ink.pen(0.3, [0.1, 0.35, 0.7]);
                ink.rect(c.0 - 20.0, c.1 - 1.2, 40.0, 2.4, "B");
                ink.fill([0.1, 0.35, 0.7]);
                ink.text((c.0, c.1 - 0.7), 1.8, Font::Bold, Align::Centre, "BUS ZONE");
            }
        }
        ink.fill(BLACK);
        ink.text((c.0 + 2.0, c.1 + 1.4), 1.6, Font::Bold, Align::Left, &sign.id);
    }
}

fn landscape(ink: &mut Ink, v: View, town: &Town, w: Bx) {
    for isl in town.islands.iter().filter(|i| matches!(i.use_, Use::Park | Use::School | Use::Corridor) && overlaps(i.property, w)) {
        // Turf, and trees planted in groups.
        let pb = isl.property;
        let turf = clip_polygon(&rect_pts(pb), grow(w, 2.0));
        if turf.len() > 2 {
            ink.clip(&v.all(&turf));
            ink.pen(0.08, [0.5, 0.7, 0.45]);
            let mut rng = Rng(isl.id as u64 * 7919 + 13);
            let b = bounds(&turf);
            let n = ((b.2 - b.0) * (b.3 - b.1) / 18.0) as usize;
            for _ in 0..n.min(9000) {
                let p = (b.0 + rng.next() * (b.2 - b.0), b.1 + rng.next() * (b.3 - b.1));
                let q = v.p(p.0, p.1);
                ink.line(q, (q.0 + 0.4, q.1 + 0.8));
                ink.line(q, (q.0 - 0.4, q.1 + 0.8));
            }
            ink.restore();
        }
        if isl.use_ == Use::Park {
            let mut rng = Rng(isl.id as u64 * 104729 + 7);
            for _ in 0..90 {
                let p = (pb.0 + 6.0 + rng.next() * (pb.2 - pb.0 - 12.0), pb.1 + 6.0 + rng.next() * (pb.3 - pb.1 - 12.0));
                if !in_box(p, w) {
                    continue;
                }
                let r = 2.5 + rng.next() * 3.0;
                let q = v.p(p.0, p.1);
                ink.save();
                ink.op("/Tint gs");
                ink.fill(TREES);
                ink.circle(q, r * v.k, "f");
                ink.restore();
                ink.pen(0.2, TREES);
                ink.circle(q, r * v.k, "S");
            }
            // A path winding through.
            let path: Vec<P> = (0..=60).map(|i| i as f64 / 60.0).map(|t| (pb.0 + t * (pb.2 - pb.0), (pb.1 + pb.3) / 2.0 + 9.0 * (t * 9.0).sin())).collect();
            ink.pen(2.6, grey(0.6));
            ink.poly(&v.all(&path), false, "S");
            ink.pen(2.1, grey(0.95));
            ink.poly(&v.all(&path), false, "S");
        }
    }
    for (p, s) in town.trees.iter().filter(|(p, _)| in_box(*p, w)) {
        let c = v.p(p.0, p.1);
        let colour = SPECIES[*s].2;
        ink.save();
        ink.op("/Tint gs");
        ink.fill(colour);
        ink.circle(c, 3.0, "f");
        ink.restore();
        ink.pen(0.2, colour);
        ink.circle(c, 3.0, "S");
        ink.fill(colour);
        ink.text((c.0, c.1 - 0.6), 1.6, Font::Bold, Align::Centre, SPECIES[*s].0);
    }
}

// ---------------------------------------------------------------------------
// The column beside the plan

fn legend_for(ink: &mut Ink, disc: Disc, top: f64) -> f64 {
    let road_edge = |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.pen(0.3, BLACK);
        ink.line((x, y - 0.6), (x + 15.0, y - 0.6));
        ink.pen(0.12, BLACK);
        ink.line((x, y + 0.6), (x + 15.0, y + 0.6));
    };
    let pit = |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.fill(WHITE);
        ink.pen(0.3, STORM);
        ink.rect(x + 4.0, y - 1.0, 6.0, 2.0, "B");
        ink.pen(0.6, STORM);
        ink.line((x + 4.0, y + 1.0), (x + 10.0, y + 1.0));
    };
    let site = line_symbol(0.8, BOUNDARY, &[8.0, 1.5, 1.5, 1.5]);
    let matchline = line_symbol(1.0, MATCH, &[10.0, 2.0, 2.0, 2.0]);
    let common: Vec<(&str, Symbol)> = vec![("Site boundary", &site), ("Match line to the next sector", &matchline), ("Kerb and gutter", &road_edge)];
    let mut entries = common;
    let boundary = line_symbol(0.4, BLACK, &[]);
    let contour = line_symbol(0.18, CONTOUR, &[]);
    let centreline = line_symbol(0.16, BLACK, &[6.0, 1.2, 1.0, 1.2]);
    let pipe = line_symbol(0.5, STORM, &[]);
    let path = |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.fill(PATH);
        ink.pen(0.1, grey(0.45));
        ink.rect(x, y - 1.2, 15.0, 2.4, "B");
    };
    let design = line_symbol(0.22, [0.3, 0.45, 0.7], &[]);
    let wall = line_symbol(0.9, [0.55, 0.25, 0.1], &[]);
    let zero = line_symbol(0.35, BLACK, &[1.2, 0.8]);
    let water = line_symbol(0.45, WATER, &[]);
    let sewer = line_symbol(0.5, SEWER, &[4.0, 1.0, 1.0, 1.0]);
    let hydrant = |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.fill(WATER);
        ink.circle((x + 7.5, y), 1.3, "f");
    };
    let elec = line_symbol(0.4, ELECTRICITY, &[]);
    let comms = line_symbol(0.35, COMMS, &[]);
    let gas = line_symbol(0.35, GAS, &[]);
    let light = |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.pen(0.3, BLACK);
        ink.line((x + 4.0, y), (x + 11.0, y));
        ink.fill(WHITE);
        ink.circle((x + 4.0, y), 0.9, "B");
        ink.fill([1.0, 0.8, 0.1]);
        ink.circle((x + 11.0, y), 0.9, "B");
    };
    let hold = line_symbol(0.9, BLACK, &[]);
    let lane = line_symbol(0.35, BLACK, &[6.0, 18.0]);
    let stop = |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.fill([0.85, 0.1, 0.1]);
        let c = (x + 7.5, y);
        let pts: Vec<(f64, f64)> = (0..8).map(|k| std::f64::consts::PI / 8.0 + k as f64 * std::f64::consts::PI / 4.0).map(|a| (c.0 + 1.6 * a.cos(), c.1 + 1.6 * a.sin())).collect();
        ink.poly(&pts, true, "f");
    };
    let give = |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.fill(WHITE);
        ink.pen(0.45, [0.85, 0.1, 0.1]);
        ink.poly(&[(x + 5.9, y + 1.2), (x + 9.1, y + 1.2), (x + 7.5, y - 1.5)], true, "B");
    };
    let setout = |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.pen(0.2, BOUNDARY);
        ink.line((x + 6.3, y), (x + 8.7, y));
        ink.line((x + 7.5, y - 1.2), (x + 7.5, y + 1.2));
    };
    let chain_pt = |ink: &mut Ink, (x, y): (f64, f64)| {
        ink.pen(0.25, BLACK);
        ink.circle((x + 7.5, y), 0.7, "S");
    };
    let species: Vec<Box<dyn Fn(&mut Ink, (f64, f64))>> = SPECIES.iter().map(|s| Box::new(swatch(s.2)) as Box<dyn Fn(&mut Ink, (f64, f64))>).collect();
    let bands: Vec<Box<dyn Fn(&mut Ink, (f64, f64))>> = CUT_FILL.iter().map(|b| Box::new(swatch(b.1)) as Box<dyn Fn(&mut Ink, (f64, f64))>).collect();
    match disc {
        Disc::Ga => entries.extend([("Road reserve boundary", &boundary as Symbol), ("Concrete footpath, 1.2 wide", &path), ("Centreline, chainage every 20 m", &centreline), ("Existing contour, 0.25 interval", &contour), ("Stormwater pit and pipe", &pit)]),
        Disc::Setout => entries.extend([("Kerb setout point", &setout as Symbol), ("Centreline point, every 20 m", &chain_pt)]),
        Disc::CutFill => {
            for (b, s) in CUT_FILL.iter().zip(&bands) {
                entries.push((b.2, s.as_ref()));
            }
            entries.push(("No cut, no fill", &zero));
        }
        Disc::Grading => entries.extend([("Design contour, 0.25 interval", &design as Symbol), ("Retaining wall, height and top", &wall)]),
        Disc::Drainage => entries.extend([("Stormwater pipe and flow", &pipe as Symbol), ("Kerb inlet pit", &pit)]),
        Disc::WaterSewer => entries.extend([("Water main", &water as Symbol), ("Fire hydrant", &hydrant), ("Sewer and manhole", &sewer)]),
        Disc::Electrical => entries.extend([("Electricity", &elec as Symbol), ("Communications", &comms), ("Gas main", &gas), ("Street light and outreach", &light)]),
        Disc::Signage => entries.extend([("Hold line", &hold as Symbol), ("Lane line", &lane), ("Stop sign", &stop), ("Give way sign", &give)]),
        Disc::Landscape => {
            for (s, sym) in SPECIES.iter().zip(&species) {
                entries.push((s.1, sym.as_ref()));
            }
        }
    }
    legend(ink, COL, top, "LEGEND", &entries)
}

fn column_table(ink: &mut Ink, set: &Set, town: &Town, disc: Disc, s: &Sector, top: f64) -> f64 {
    let w = s.bx;
    let row_h = 4.2;
    let size = 2.0;
    match disc {
        Disc::Setout => {
            let points: Vec<(String, P)> = town.kerb_points().into_iter().filter(|(_, p)| in_box(*p, w)).collect();
            heading(ink, (COL, top - 4.0), "KERB SETOUT");
            let cols = [("POINT", 20.0), ("EASTING", 34.0), ("NORTHING", 34.0)];
            let per = 44;
            let mut bottom = top;
            for (k, chunk) in points.chunks(per).take(2).enumerate() {
                let rows: Vec<Vec<String>> = chunk.iter().map(|(id, p)| vec![id.clone(), format!("{:.3}", EASTING + p.0), format!("{:.3}", NORTHING + p.1)]).collect();
                bottom = table(ink, COL + k as f64 * 89.0, top - 8.0, &cols, &rows, row_h * 0.85, 1.7).min(bottom);
            }
            if points.len() > per * 2 {
                ink.fill(grey(0.35));
                ink.text((COL, bottom - 3.5), 2.0, Font::Regular, Align::Left, &format!("{} more points in the setout file.", points.len() - per * 2));
                bottom -= 4.0;
            }
            bottom
        }
        Disc::CutFill => {
            heading(ink, (COL, top - 4.0), "EARTHWORKS IN THIS SECTOR");
            let mut rows = Vec::new();
            let (mut tc, mut tf) = (0.0, 0.0);
            for (name, pick) in [("Lots", 0), ("Roads", 1), ("Basins", 2), ("Open space", 3)] {
                let (mut cut, mut fill) = (0.0, 0.0);
                let step = 5.0;
                let mut y = w.1 + step / 2.0;
                while y < w.3 {
                    let mut x = w.0 + step / 2.0;
                    while x < w.2 {
                        let p = (x, y);
                        let kind = if town.basins.iter().any(|b| b.surface(p).is_some()) {
                            2
                        } else if town.lot_at(p).is_some() {
                            0
                        } else if town.road_at(p).is_some() {
                            1
                        } else {
                            3
                        };
                        if kind == pick {
                            if let Some(d) = cut_fill(town, p) {
                                if d < 0.0 { cut -= d * step * step } else { fill += d * step * step }
                            }
                        }
                        x += step;
                    }
                    y += step;
                }
                tc += cut;
                tf += fill;
                rows.push(vec![name.into(), format!("{cut:.0}"), format!("{fill:.0}"), format!("{:+.0}", fill - cut)]);
            }
            rows.push(vec!["Total".into(), format!("{tc:.0}"), format!("{tf:.0}"), format!("{:+.0}", tf - tc)]);
            table(ink, COL, top - 8.0, &[("", 44.0), ("CUT m³", 40.0), ("FILL m³", 40.0), ("NET m³", 40.0)], &rows, 5.0, 2.2)
        }
        Disc::Grading => {
            let walls: Vec<&Wall> = town.walls.iter().filter(|x| in_box(mul(add(x.a, x.b), 0.5), w)).collect();
            heading(ink, (COL, top - 4.0), "RETAINING WALLS");
            let rows = vec![
                vec!["Walls".into(), format!("{}", walls.len())],
                vec!["Total length".into(), format!("{:.1} m", walls.iter().map(|x| dist(x.a, x.b)).sum::<f64>() + 0.0)],
                vec!["Up to 0.5 high".into(), format!("{:.1} m", walls.iter().filter(|x| x.height <= 0.5).map(|x| dist(x.a, x.b)).sum::<f64>() + 0.0)],
                vec!["0.5 to 1.0 high".into(), format!("{:.1} m", walls.iter().filter(|x| x.height > 0.5 && x.height <= 1.0).map(|x| dist(x.a, x.b)).sum::<f64>() + 0.0)],
                vec!["Over 1.0 high".into(), format!("{:.1} m", walls.iter().filter(|x| x.height > 1.0).map(|x| dist(x.a, x.b)).sum::<f64>() + 0.0)],
            ];
            table(ink, COL, top - 8.0, &[("", 80.0), ("", 60.0)], &rows, 5.0, 2.2)
        }
        Disc::Drainage => {
            let pits: Vec<&Pit> = town.pits.iter().filter(|p| in_box(p.at, w)).collect();
            heading(ink, (COL, top - 4.0), "IN THIS SECTOR");
            let rows = vec![
                vec!["Kerb inlet pits".into(), format!("{}", pits.iter().filter(|p| matches!(p.kind, PitKind::Kerb(_))).count())],
                vec!["Headwalls".into(), format!("{}", pits.iter().filter(|p| p.kind == PitKind::Headwall).count())],
                vec!["Pit schedule".into(), set.range_of("PIT SCHEDULE")],
                vec!["Longitudinal sections".into(), set.range_of("STORMWATER LONG")],
            ];
            table(ink, COL, top - 8.0, &[("", 80.0), ("", 60.0)], &rows, 5.0, 2.2)
        }
        Disc::WaterSewer => {
            let mhs = town.sewers.iter().flat_map(|s| &s.manholes).filter(|m| in_box(m.at, w)).count();
            heading(ink, (COL, top - 4.0), "IN THIS SECTOR");
            let rows = vec![
                vec!["Fire hydrants".into(), format!("{}", town.hydrants.iter().filter(|p| in_box(**p, w)).count())],
                vec!["Stop valves".into(), format!("{}", town.valves.iter().filter(|p| in_box(**p, w)).count())],
                vec!["Sewer manholes".into(), format!("{mhs}")],
                vec!["Sewer sections".into(), set.range_of("SEWER LONG")],
            ];
            table(ink, COL, top - 8.0, &[("", 80.0), ("", 60.0)], &rows, 5.0, 2.2)
        }
        Disc::Electrical => {
            heading(ink, (COL, top - 4.0), "IN THIS SECTOR");
            let rows = vec![
                vec!["Street lights, 6.5 m poles".into(), format!("{}", town.lights.iter().filter(|(p, _)| in_box(*p, w)).count())],
                vec!["Kiosks".into(), format!("{}", town.kiosks.iter().filter(|p| in_box(**p, w)).count())],
            ];
            table(ink, COL, top - 8.0, &[("", 100.0), ("", 40.0)], &rows, 5.0, 2.2)
        }
        Disc::Signage => {
            let signs: Vec<&Sign> = town.signs.iter().filter(|s| in_box(s.at, w)).collect();
            heading(ink, (COL, top - 4.0), "SIGN SCHEDULE");
            let rows: Vec<Vec<String>> = signs.iter().take(40).map(|s| vec![s.id.clone(), s.kind.name().into(), town.roads[s.road].place(s.ch)]).collect();
            table(ink, COL, top - 8.0, &[("ID", 18.0), ("SIGN", 42.0), ("LOCATION", 110.0)], &rows, row_h, size)
        }
        Disc::Landscape => {
            heading(ink, (COL, top - 4.0), "STREET TREES IN THIS SECTOR");
            let rows: Vec<Vec<String>> = SPECIES
                .iter()
                .enumerate()
                .map(|(k, s)| vec![s.0.into(), s.1.into(), format!("{}", town.trees.iter().filter(|(p, sp)| *sp == k && in_box(*p, w)).count())])
                .collect();
            table(ink, COL, top - 8.0, &[("", 14.0), ("SPECIES", 100.0), ("No.", 26.0)], &rows, 5.0, 2.2)
        }
        Disc::Ga => {
            let lots: Vec<&Lot> = town.lots.iter().filter(|l| in_box(centre(l.bx), w)).collect();
            heading(ink, (COL, top - 4.0), "IN THIS SECTOR");
            let rows = vec![
                vec!["Lots".into(), format!("{}", lots.len())],
                vec!["Lot area".into(), format!("{:.3} ha", lots.iter().map(|l| l.area).sum::<f64>() / 10_000.0)],
                vec!["Smallest lot".into(), format!("{:.0} m²", lots.iter().map(|l| l.area).fold(f64::MAX, f64::min))],
                vec!["Largest lot".into(), format!("{:.0} m²", lots.iter().map(|l| l.area).fold(0.0, f64::max))],
                vec!["Lot setout".into(), set.lot_setout_sheet(s.id)],
            ];
            table(ink, COL, top - 8.0, &[("", 80.0), ("", 60.0)], &rows, 5.0, 2.2)
        }
    }
}

fn disc_notes(set: &Set, disc: Disc) -> Vec<String> {
    let s = |x: &str| x.to_owned();
    match disc {
        Disc::Ga => vec![s("Lot areas are to the nearest square metre and subject to final survey."), format!("Typical sections on {}. Details on {}.", set.range_of("TYPICAL"), set.range_of("DETAILS")), s("Chainages are along the road centrelines from the start of each road.")],
        Disc::Setout => vec![s("Coordinates are on the project grid. Kerb points are to the lip."), s("Kerb returns are set out from their tangent points (a, b) and centre (c)."), format!("Lot corners are tabled on {}.", set.range_of("LOT SETOUT"))],
        Disc::CutFill => vec![s("Depths are finished surface less natural surface, at the centre of each 10 m square."), s("Small figures are natural and finished levels there."), s("Volumes are solid measure, from 5 m squares, before topsoil stripping."), format!("Overall earthworks: {}.", set.range_of("OVERALL EARTHWORKS"))],
        Disc::Grading => vec![s("Lots are benched to the finished surface level shown, falling to the street."), s("Retaining walls where neighbouring pads differ by 0.3 or more; details on C-807."), s("Levels along the kerbs are lip levels every 20 m.")],
        Disc::Drainage => vec![s("Pipes are RCP class 2 unless noted, on HS2 bedding."), format!("Pit and pipe schedules: {} and {}.", set.range_of("PIT SCHEDULE"), set.range_of("PIPE SCHEDULE")), format!("Basins: {}.", set.range_of("DETENTION BASIN"))],
        Disc::WaterSewer => vec![s("Water mains 2.3 from the boundary; sewers 1.5 inside the lots' rear boundaries."), format!("Sewer long sections on {}; manholes on {}.", set.range_of("SEWER LONG"), set.range_of("MANHOLE SCHEDULE"))],
        Disc::Electrical => vec![s("Electricity 0.6, communications 1.1 and gas 2.8 from the boundary."), s("Kiosks in 3.0 × 2.5 easements inside the lots."), s("Street lights to category P4 unless on the boulevard (V3).")],
        Disc::Signage => vec![s("Signs and markings to the relevant standard."), s("Hold lines 300 wide; lane lines 100 wide, 3 m marks and 9 m gaps."), s("Road humps on C-808.")],
        Disc::Landscape => vec![s("Street trees 1.7 from the kerb, clear of lights, pits and driveways."), s("Turf all verges, parks and basin batters.")],
    }
}

/// The whole site, small, with the sectors and their sheets, and this one
/// picked out.
fn key_plan(ink: &mut Ink, set: &Set, town: &Town, disc: Disc, current: usize, b: Bx) {
    let k = (b.2 - b.0) / SITE_W;
    let v = View { ox: b.0, oy: b.1, k };
    heading(ink, (b.0, b.3 + 4.0), "KEY PLAN");
    ink.pen(0.1, grey(0.6));
    for road in &town.roads {
        let pts = road.centreline();
        ink.pen(if road.kind == Kind::Local { 0.25 } else { 0.5 }, grey(0.6));
        ink.poly(&v.all(&pts), false, "S");
    }
    for s in &set.sectors {
        let r = paper(v, s.bx);
        if s.id == current {
            ink.save();
            ink.op("/Tint gs");
            ink.fill(BOUNDARY);
            ink.rect(r.0, r.1, r.2 - r.0, r.3 - r.1, "f");
            ink.restore();
        }
        ink.pen(0.3, BLACK);
        ink.rect(r.0, r.1, r.2 - r.0, r.3 - r.1, "S");
        ink.fill(BLACK);
        ink.text(((r.0 + r.2) / 2.0, (r.1 + r.3) / 2.0 - 1.0), 2.6, Font::Bold, Align::Centre, &set.sector_sheet(disc, s.id));
    }
}

// ---------------------------------------------------------------------------
// The whole site at 1:2500

pub const OVERALL: View = View { ox: 30.0, oy: 150.0, k: 0.4 };

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Overall {
    Layout,
    Staging,
    Lots,
    Earthworks,
    Stormwater,
    WaterSewer,
    Electrical,
    Landscape,
}

pub fn overall_sheet(ink: &mut Ink, set: &Set, town: &Town, which: Overall) {
    let v = OVERALL;
    let site = town.site();
    let window = (-20.0, -20.0, SITE_W + 20.0, town.height + 20.0);
    ink.clip(&v.all(&rect_pts(window)));
    if which == Overall::Earthworks {
        let step = 25.0;
        let mut y = 0.0;
        while y < town.height {
            let mut x = 0.0;
            while x < SITE_W {
                let c = (x + step / 2.0, y + step / 2.0);
                let mut sum = 0.0;
                let mut n = 0;
                for i in 0..5 {
                    for j in 0..5 {
                        if let Some(d) = cut_fill(town, (x + 2.5 + i as f64 * 5.0, y + 2.5 + j as f64 * 5.0)) {
                            sum += d;
                            n += 1;
                        }
                    }
                }
                if n > 0 {
                    let d = sum / n as f64;
                    ink.fill(cut_fill_colour(d));
                    v.rect(ink, x, y, x + step, y + step, "f");
                    let p = v.p(c.0, c.1);
                    ink.fill(if d.abs() > 0.5 { WHITE } else { BLACK });
                    ink.text((p.0, p.1 - 0.5), 1.5, Font::Regular, Align::Centre, &format!("{d:+.2}"));
                }
                x += step;
            }
            y += step;
        }
    }
    // The roads as their kerbs, and the lots.
    let staging = [[0.95, 0.8, 0.55], [0.7, 0.85, 0.95], [0.8, 0.92, 0.7], [0.95, 0.7, 0.75], [0.85, 0.78, 0.95], [0.75, 0.9, 0.85]];
    if matches!(which, Overall::Staging) {
        for lot in &town.lots {
            ink.fill(staging[(lot.stage - 1) as usize]);
            v.rect(ink, lot.bx.0, lot.bx.1, lot.bx.2, lot.bx.3, "f");
        }
    }
    let faded = !matches!(which, Overall::Layout | Overall::Lots | Overall::Staging);
    ink.fill(if which == Overall::Earthworks { WHITE } else { grey(if faded { 0.93 } else { 0.85 }) });
    if which != Overall::Earthworks {
        ink.path(&v.all(&rect_pts(window)), true);
        for isl in &town.islands {
            ink.path(&v.all(&isl.lip), true);
        }
        ink.op("f*");
    }
    ink.pen(0.12, if faded { grey(0.55) } else { BLACK });
    for isl in &town.islands {
        ink.poly(&v.all(&isl.lip), true, "S");
    }
    ink.pen(0.06, if faded { grey(0.6) } else { grey(0.2) });
    for lot in &town.lots {
        v.rect(ink, lot.bx.0, lot.bx.1, lot.bx.2, lot.bx.3, "S");
    }
    if which == Overall::Lots {
        for lot in &town.lots {
            let c = v.p(centre(lot.bx).0, centre(lot.bx).1);
            ink.fill(BLACK);
            ink.text_rotated((c.0 + 0.5, c.1 - 1.5), 1.25, Font::Regular, Align::Left, &lot.number.to_string(), if lot.frontage() < 17.0 && matches!(lot.front, Side::South | Side::North) { std::f64::consts::FRAC_PI_2 } else { 0.0 });
        }
    }
    let line_view = |ink: &mut Ink, pts: &[P], width: f64, colour: [f64; 3]| {
        ink.pen(width, colour);
        ink.poly(&v.all(pts), false, "S");
    };
    match which {
        Overall::Stormwater => {
            for pipe in &town.pipes {
                line_view(ink, &[town.pits[pipe.from].at, town.pits[pipe.to].at], 0.15 + pipe.dia * 0.5, STORM);
            }
            for line in &town.lines {
                let last = &town.pits[*line.pits.last().expect("a pit")];
                let p = v.p(last.at.0, last.at.1);
                ink.fill(STORM);
                ink.text((p.0 + 1.5, p.1), 1.8, Font::Bold, Align::Left, &line.name);
            }
        }
        Overall::WaterSewer => {
            for isl in town.islands.iter().filter(|i| i.has_verge() && i.use_ != Use::Median) {
                ink.pen(0.2, WATER);
                ink.poly(&v.all(&isl.ring(|g| g - 2.3)), true, "S");
            }
            for s in &town.sewers {
                let pts: Vec<P> = s.manholes.iter().map(|m| m.at).collect();
                line_view(ink, &pts, 0.35, SEWER);
                let p = v.p(pts[0].0, pts[0].1);
                ink.fill(SEWER);
                ink.text((p.0, p.1 + 0.8), 1.4, Font::Regular, Align::Left, &s.name);
            }
        }
        Overall::Electrical => {
            for (p, _) in &town.lights {
                let q = v.p(p.0, p.1);
                ink.fill([0.95, 0.7, 0.0]);
                ink.circle(q, 0.45, "f");
            }
            for p in &town.kiosks {
                let q = v.p(p.0, p.1);
                ink.fill(ELECTRICITY);
                ink.rect(q.0 - 1.0, q.1 - 0.8, 2.0, 1.6, "f");
            }
        }
        Overall::Landscape => {
            for (p, s) in &town.trees {
                let q = v.p(p.0, p.1);
                ink.fill(SPECIES[*s].2);
                ink.circle(q, 0.6, "f");
            }
        }
        _ => {}
    }
    for b in &town.basins {
        ink.pen(0.2, BLACK);
        ink.fill([0.85, 0.93, 0.85]);
        ink.poly(&v.all(&b.ring(0.0)), true, "B");
    }
    let creek: Vec<P> = (0..=((town.height + 40.0) / 5.0) as usize).map(|i| -20.0 + i as f64 * 5.0).map(|y| (creek_x(y), y)).collect();
    ink.pen(2.0, [0.6, 0.8, 0.95]);
    ink.poly(&v.all(&creek), false, "S");
    ink.restore();
    ink.pen(0.6, BOUNDARY);
    ink.dash(&[6.0, 1.2, 1.2, 1.2]);
    v.rect(ink, site.0, site.1, site.2, site.3, "S");
    ink.dash(&[]);
    // Road names, and the sectors over the plans that need them.
    for road in &town.roads {
        let at = road.point(road.length() * 0.3, 0.0);
        if in_box(at, site) {
            tag(ink, v.p(at.0, at.1), upright(road.dir()), 1.8, Font::Bold, grey(0.15), &road.name.to_uppercase());
        }
    }
    if matches!(which, Overall::Layout | Overall::Earthworks | Overall::Stormwater) {
        let disc = match which {
            Overall::Earthworks => Disc::CutFill,
            Overall::Stormwater => Disc::Drainage,
            _ => Disc::Ga,
        };
        for s in &set.sectors {
            let r = paper(v, s.bx);
            ink.pen(0.5, MATCH);
            ink.dash(&[4.0, 1.0, 1.0, 1.0]);
            ink.rect(r.0, r.1, r.2 - r.0, r.3 - r.1, "S");
            ink.dash(&[]);
            tag(ink, (r.0 + 12.0, r.3 - 5.0), 0.0, 3.2, Font::Bold, MATCH, &set.sector_sheet(disc, s.id));
        }
    }
    north(ink, (500.0, 505.0), 8.0);
    let name = match which {
        Overall::Layout => "OVERALL LAYOUT AND SECTOR KEY",
        Overall::Staging => "STAGING PLAN",
        Overall::Lots => "OVERALL SUBDIVISION PLAN",
        Overall::Earthworks => "OVERALL EARTHWORKS, CUT AND FILL",
        Overall::Stormwater => "OVERALL STORMWATER",
        Overall::WaterSewer => "OVERALL WATER AND SEWER",
        Overall::Electrical => "OVERALL ELECTRICAL AND LIGHTING",
        Overall::Landscape => "OVERALL STREET TREES",
    };
    title(ink, (30.0, 118.0), "1", name, "1:2500 @ A1");
    bar(ink, (330.0, 116.0), 0.4, 50.0, 5, "SCALE 1:2500");

    // The column: what each overall plan is summarised by.
    let x = 540.0;
    match which {
        Overall::Layout => {
            heading(ink, (x, 560.0), "SECTOR SHEETS");
            let rows: Vec<Vec<String>> = Disc::ALL.iter().map(|d| vec![d.title().to_owned(), set.range_of(d.title())]).collect();
            table(ink, x, 552.0, &[("DISCIPLINE", 170.0), ("SHEETS", 100.0)], &rows, 5.0, 2.2);
            heading(ink, (x, 480.0), "ROADS");
            let rows: Vec<Vec<String>> = town.roads.iter().map(|r| vec![r.code.clone(), r.name.into(), r.kind.name().into(), format!("{:.1}", r.length())]).collect();
            table(ink, x, 472.0, &[("", 14.0), ("NAME", 76.0), ("TYPE", 100.0), ("LENGTH", 30.0)], &rows, 4.6, 2.1);
        }
        Overall::Staging => {
            heading(ink, (x, 560.0), "STAGES");
            let rows: Vec<Vec<String>> = (1..=6)
                .map(|s| {
                    let lots: Vec<&Lot> = town.lots.iter().filter(|l| l.stage == s).collect();
                    vec![format!("Stage {s}"), format!("{}", lots.len()), format!("{:.3} ha", lots.iter().map(|l| l.area).sum::<f64>() / 10_000.0), format!("{}–{}", lots.iter().map(|l| l.number).min().unwrap_or(0), lots.iter().map(|l| l.number).max().unwrap_or(0))]
                })
                .collect();
            let bottom = table(ink, x, 552.0, &[("", 40.0), ("LOTS", 30.0), ("AREA", 50.0), ("NUMBERS", 60.0)], &rows, 5.0, 2.2);
            let sw: Vec<Box<dyn Fn(&mut Ink, (f64, f64))>> = staging.iter().map(|c| Box::new(swatch(*c)) as Box<dyn Fn(&mut Ink, (f64, f64))>).collect();
            let names: Vec<String> = (1..=6).map(|s| format!("Stage {s}")).collect();
            let entries: Vec<(&str, Symbol)> = names.iter().zip(&sw).map(|(n, s)| (n.as_str(), s.as_ref())).collect();
            legend(ink, x, bottom - 12.0, "LEGEND", &entries);
        }
        Overall::Lots => {
            heading(ink, (x, 560.0), "LOT SUMMARY");
            let areas: Vec<f64> = town.lots.iter().map(|l| l.area).collect();
            let rows = vec![
                vec!["Lots".into(), format!("{}", areas.len())],
                vec!["Total lot area".into(), format!("{:.3} ha", areas.iter().sum::<f64>() / 10_000.0)],
                vec!["Average lot".into(), format!("{:.0} m²", areas.iter().sum::<f64>() / areas.len() as f64)],
                vec!["Under 400 m²".into(), format!("{}", areas.iter().filter(|a| **a < 400.0).count())],
                vec!["400 to 500 m²".into(), format!("{}", areas.iter().filter(|a| **a >= 400.0 && **a < 500.0).count())],
                vec!["500 m² and over".into(), format!("{}", areas.iter().filter(|a| **a >= 500.0).count())],
                vec!["Lot setout".into(), set.range_of("LOT SETOUT")],
            ];
            table(ink, x, 552.0, &[("", 90.0), ("", 80.0)], &rows, 5.0, 2.2);
        }
        Overall::Earthworks => {
            heading(ink, (x, 560.0), "EARTHWORKS BY SECTOR");
            let mut rows = Vec::new();
            let (mut tc, mut tf) = (0.0, 0.0);
            for s in &set.sectors {
                let (c, f) = volumes(town, s.bx, 5.0);
                tc += c;
                tf += f;
                rows.push(vec![format!("Sector {}", s.id + 1), set.sector_sheet(Disc::CutFill, s.id), format!("{c:.0}"), format!("{f:.0}"), format!("{:+.0}", f - c)]);
            }
            rows.push(vec!["Site".into(), "".into(), format!("{tc:.0}"), format!("{tf:.0}"), format!("{:+.0}", tf - tc)]);
            let bottom = table(ink, x, 552.0, &[("", 36.0), ("SHEET", 28.0), ("CUT m³", 36.0), ("FILL m³", 36.0), ("NET m³", 36.0)], &rows, 5.0, 2.2);
            let sw: Vec<Box<dyn Fn(&mut Ink, (f64, f64))>> = CUT_FILL.iter().map(|b| Box::new(swatch(b.1)) as Box<dyn Fn(&mut Ink, (f64, f64))>).collect();
            let entries: Vec<(&str, Symbol)> = CUT_FILL.iter().zip(&sw).map(|(b, s)| (b.2, s.as_ref())).collect();
            legend(ink, x, bottom - 12.0, "LEGEND, MEAN OF EACH 25 m SQUARE", &entries);
        }
        Overall::Stormwater => {
            heading(ink, (x, 560.0), "DRAINAGE LINES");
            let rows: Vec<Vec<String>> = town
                .lines
                .iter()
                .map(|l| {
                    let pipes: Vec<&Pipe> = town.pipes.iter().filter(|p| l.pits.contains(&p.from) && l.pits.contains(&p.to)).collect();
                    let q = pipes.iter().map(|p| p.q100).fold(0.0, f64::max);
                    vec![l.name.clone(), format!("{}", l.pits.len()), format!("{:.0}", pipes.iter().map(|p| p.length).sum::<f64>()), format!("{:.0}", pipes.iter().map(|p| p.dia).fold(0.0, f64::max) * 1000.0), format!("{q:.0}")]
                })
                .collect();
            table(ink, x, 552.0, &[("LINE", 36.0), ("PITS", 24.0), ("LENGTH", 30.0), ("MAX Ø", 30.0), ("Q1% L/s", 34.0)], &rows, 4.1, 1.9);
        }
        Overall::WaterSewer => {
            heading(ink, (x, 560.0), "SEWER LINES");
            let rows: Vec<Vec<String>> = town
                .sewers
                .iter()
                .map(|s| {
                    let (a, b) = (&s.manholes[0], &s.manholes[s.manholes.len() - 1]);
                    vec![s.name.clone(), format!("{}", s.manholes.len()), format!("{:.1}", dist(a.at, b.at)), format!("{:.3}", b.invert), s.outlet.map_or("–".into(), |r| town.roads[r].short())]
                })
                .collect();
            let per = 60;
            for (k, chunk) in rows.chunks(per).enumerate().take(2) {
                table(ink, x + k as f64 * 145.0, 552.0, &[("LINE", 16.0), ("MH", 12.0), ("LENGTH", 22.0), ("OUT IL", 24.0), ("TO", 60.0)], chunk, 4.1, 1.8);
            }
        }
        Overall::Electrical => {
            heading(ink, (x, 560.0), "BY SECTOR");
            let rows: Vec<Vec<String>> = set
                .sectors
                .iter()
                .map(|s| vec![format!("Sector {}", s.id + 1), set.sector_sheet(Disc::Electrical, s.id), format!("{}", town.lights.iter().filter(|(p, _)| in_box(*p, s.bx)).count()), format!("{}", town.kiosks.iter().filter(|p| in_box(**p, s.bx)).count())])
                .collect();
            table(ink, x, 552.0, &[("", 36.0), ("SHEET", 30.0), ("LIGHTS", 30.0), ("KIOSKS", 30.0)], &rows, 5.0, 2.2);
        }
        Overall::Landscape => {
            heading(ink, (x, 560.0), "STREET TREES");
            let rows: Vec<Vec<String>> = SPECIES.iter().enumerate().map(|(k, s)| vec![s.0.into(), s.1.into(), format!("{}", town.trees.iter().filter(|t| t.1 == k).count())]).collect();
            table(ink, x, 552.0, &[("", 14.0), ("SPECIES", 100.0), ("No.", 26.0)], &rows, 5.0, 2.2);
        }
    }
}

// ---------------------------------------------------------------------------
// A basin

pub fn basin_window(b: &Basin) -> Bx {
    let bx = b.bx();
    let c = centre(bx);
    (c.0 - 75.0, c.1 - 105.0, c.0 + 75.0, c.1 + 105.0)
}

pub fn basin_view(b: &Basin) -> View {
    let w = basin_window(b);
    View { ox: 40.0 - w.0 * 2.0, oy: 110.0 - w.1 * 2.0, k: 2.0 }
}

pub fn basin_plan(ink: &mut Ink, town: &Town, b: &Basin) -> View {
    let w = basin_window(b);
    let v = basin_view(b);
    let frame_pts = v.all(&rect_pts(w));
    ink.clip(&frame_pts);
    ground_contours(ink, v, w, CONTOUR, true);
    base(ink, v, town, w, Base { full: false, faded: true, fills: true });
    basin(ink, v, b, false);
    // Its design contours every 0.2 m down the batters.
    let mut d = 0.2;
    while d < b.top - b.floor - 0.01 {
        ink.pen(0.12, [0.3, 0.45, 0.7]);
        ink.poly(&v.all(&b.ring(d)), true, "S");
        let ring = b.ring(d);
        let p = ring[ring.len() / 8];
        tag(ink, v.p(p.0, p.1), 0.0, 1.6, Font::Regular, [0.3, 0.45, 0.7], &format!("{:.2}", b.top - d));
        d += 0.2;
    }
    drainage(ink, v, town, w, true);
    ink.restore();
    ink.pen(0.35, BLACK);
    ink.poly(&frame_pts, true, "S");
    // The section lines.
    let bx = b.bx();
    let c = centre(bx);
    for (name, a, z) in [("A", (c.0, bx.1 - 15.0), (c.0, bx.3 + 15.0)), ("B", (bx.0 - 15.0, c.1), (bx.2 + 15.0, c.1))] {
        ink.pen(0.5, BOUNDARY);
        ink.dash(&[6.0, 1.5, 1.5, 1.5]);
        v.line(ink, a, z);
        ink.dash(&[]);
        for p in [a, z] {
            let q = v.p(p.0, p.1);
            ink.fill(BOUNDARY);
            ink.circle(q, 3.0, "f");
            ink.fill(WHITE);
            ink.text((q.0, q.1 - 1.2), 3.2, Font::Bold, Align::Centre, name);
        }
    }
    title(ink, (40.0, 88.0), "1", &format!("BASIN {}  ·  PLAN", &b.name[1..]), "1:500 @ A1");
    north(ink, (330.0, 505.0), 7.0);
    v
}
