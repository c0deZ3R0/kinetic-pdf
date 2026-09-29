//! Drawings along a line: the roads' long sections, their cross sections
//! every 20 metres, and the long sections of the stormwater and sewer
//! lines.

use crate::draw::*;
use crate::geom::*;
use crate::sample_ink::{grey, text_width, Align, Font, Ink, PT, BLACK, WHITE};
use crate::town::*;
use crate::Set;
use markup_model::Pt;

/// A section's own scales: along at `kh` and up at `kv` millimetres to the
/// metre, from a datum, starting at chainage `ch0`.
#[derive(Clone, Copy)]
pub struct Long {
    pub ox: f64,
    pub oy: f64,
    pub kh: f64,
    pub kv: f64,
    pub datum: f64,
    pub ch0: f64,
}

impl Long {
    pub fn p(&self, ch: f64, rl: f64) -> (f64, f64) {
        (self.ox + (ch - self.ch0) * self.kh, self.oy + (rl - self.datum) * self.kv)
    }

    pub fn pt(&self, ch: f64, rl: f64) -> Pt {
        let (a, b) = self.p(ch, rl);
        Pt::new(a * PT, b * PT)
    }
}

/// The longest stretch one panel of a long section shows, at 1:500.
pub const PANEL: f64 = 360.0;

fn heading_under(ink: &mut Ink, at: (f64, f64), name: &str, sub: &str) {
    ink.fill(BLACK);
    ink.text(at, 4.4, Font::Bold, Align::Left, name);
    let w = text_width(name, Font::Bold) * 4.4;
    ink.pen(0.45, BLACK);
    ink.line((at.0, at.1 - 1.2), (at.0 + w, at.1 - 1.2));
    ink.text((at.0, at.1 - 6.0), 2.6, Font::Regular, Align::Left, sub);
}

/// One panel of a road's long section, its datum line at `oy`.
pub fn road_panel(ink: &mut Ink, set: &Set, town: &Town, road: &Road, ch0: f64, ch1: f64, oy: f64) -> Long {
    let levels = &road.levels;
    let existing = |ch: f64| {
        let p = road.point(ch, 0.0);
        ground(p.0, p.1)
    };
    let lo = (0..=((ch1 - ch0) as usize)).map(|i| ch0 + i as f64).map(|c| levels.level(c).min(existing(c))).fold(f64::MAX, f64::min);
    let l = Long { ox: 100.0, oy, kh: 2.0, kv: 10.0, datum: (lo - 1.0).floor(), ch0 };
    let mut chs: Vec<f64> = Vec::new();
    let mut c = (ch0 / 10.0).ceil() * 10.0;
    while c < ch1 - 3.0 {
        chs.push(c);
        c += 10.0;
    }
    chs.insert(0, ch0);
    chs.push(ch1);
    for &(c, _, vc) in &levels.ips {
        for ch in [c - vc / 2.0, c, c + vc / 2.0] {
            if ch > ch0 && ch < ch1 && chs.iter().all(|&o| (o - ch).abs() > 1.5) {
                chs.push(ch);
            }
        }
    }
    chs.sort_by(f64::total_cmp);
    let xs: Vec<f64> = chs.iter().map(|&c| l.p(c, 0.0).0).collect();

    ink.pen(0.1, grey(0.6));
    for &c in &chs {
        ink.line(l.p(c, l.datum), l.p(c, levels.level(c)));
    }
    ink.pen(0.35, BLACK);
    ink.line(l.p(ch0, l.datum), l.p(ch1, l.datum));
    let sampled = |f: &dyn Fn(f64) -> f64| -> Vec<(f64, f64)> {
        let n = (ch1 - ch0).ceil() as usize;
        (0..=n).map(|i| (ch0 + i as f64).min(ch1)).map(|c| l.p(c, f(c))).collect()
    };
    ink.pen(0.25, [0.45, 0.3, 0.15]);
    ink.dash(&[2.0, 1.0]);
    ink.poly(&sampled(&existing), false, "S");
    ink.dash(&[]);
    ink.pen(0.6, BLACK);
    ink.poly(&sampled(&|c| levels.level(c)), false, "S");

    // Grades, and each intersection point with its curve.
    let ips = &levels.ips;
    for i in 0..ips.len() - 1 {
        let (a, b) = (ips[i], ips[i + 1]);
        let from = (a.0 + a.2 / 2.0).max(ch0);
        let to = (b.0 - b.2 / 2.0).min(ch1);
        if to - from < 18.0 {
            continue;
        }
        let mid = (from + to) / 2.0;
        let g = levels.grade(i);
        let angle = (g * l.kv / l.kh).atan();
        let at = add(l.p(mid, levels.level(mid)), (-angle.sin() * 2.4, angle.cos() * 2.4));
        ink.fill(BLACK);
        ink.text_rotated(at, 2.1, Font::Bold, Align::Centre, &format!("{:+.2}%", g * 100.0), angle);
    }
    for &(c, rl, vc) in ips.iter() {
        if c < ch0 - 0.1 || c > ch1 + 0.1 {
            continue;
        }
        let top = l.p(c, rl);
        let (x, y) = (top.0, top.1 + 11.0);
        ink.pen(0.15, BLACK);
        ink.line(top, (x, y));
        if vc > 0.0 {
            ink.poly(&[l.p(c - vc / 2.0, levels.level(c - vc / 2.0)), top, l.p(c + vc / 2.0, levels.level(c + vc / 2.0))], false, "S");
        }
        ink.fill(BLACK);
        ink.text((x + 0.8, y + 5.4), 1.9, Font::Bold, Align::Left, &format!("IP {c:.2}"));
        ink.text((x + 0.8, y + 2.7), 1.8, Font::Regular, Align::Left, &format!("RL {rl:.3}"));
        if vc > 0.0 {
            ink.text((x + 0.8, y), 1.8, Font::Regular, Align::Left, &format!("VC {vc:.1}"));
        }
    }
    // The roads it meets.
    for &(at, j) in &road.crossings {
        let c = at - road.from;
        if c >= ch0 && c <= ch1 {
            let p = l.p(c, levels.level(c));
            ink.pen(0.3, [0.1, 0.45, 0.2]);
            ink.line((p.0, p.1 + 9.0), (p.0, p.1 + 26.0));
            ink.fill([0.1, 0.45, 0.2]);
            ink.text_turned((p.0 - 0.6, p.1 + 27.0), 1.9, Font::Bold, Align::Left, &town.roads[j].name.to_uppercase(), true);
        }
    }

    let left = l.ox - 60.0;
    let right = l.p(ch1, 0.0).0 + 4.0;
    ink.fill(BLACK);
    ink.text((left + 2.0, l.oy + 2.0), 2.1, Font::Bold, Align::Left, &format!("DATUM RL {:.2}", l.datum));
    let f3 = |f: &dyn Fn(f64) -> f64| chs.iter().map(|&c| format!("{:.3}", f(c))).collect::<Vec<_>>();
    let rows = vec![
        ("DESIGN LEVEL", f3(&|c| levels.level(c))),
        ("NATURAL SURFACE", f3(&existing)),
        ("CUT (-) FILL (+)", f3(&|c| levels.level(c) - existing(c))),
        ("LEFT LIP", f3(&|c| road.surface(c, road.kind.half()))),
        ("CHAINAGE", chs.iter().map(|c| format!("{c:.2}")).collect()),
    ];
    let bottom = bands(ink, left, l.oy, right, &xs, &rows, 10.0);
    let sheets = set.plans_for(Disc2::Ga, town, road, ch0, ch1);
    heading_under(ink, (left, bottom - 10.0), &format!("{}  ·  CH {ch0:.2} TO {ch1:.2}", road.name.to_uppercase()), &format!("H 1:500  V 1:100 @ A1  ·  PLAN ON {sheets}"));
    l
}

pub use crate::Disc as Disc2;

/// Cross sections: `list` of (road, chainage), three across and six down.
pub fn cross_sheet(ink: &mut Ink, town: &Town, list: &[(usize, f64)]) -> Vec<Long> {
    let mut out = Vec::new();
    for (i, &(r, ch)) in list.iter().enumerate() {
        let (col, row) = (i % 3, i / 3);
        let cell = (30.0 + col as f64 * 265.0, 566.0 - row as f64 * 85.0);
        out.push(cross_section(ink, town, &town.roads[r], ch, cell));
    }
    out
}

/// A section across a road at a chainage, at H 1:200 V 1:100, its cell's top
/// left at `cell`.
fn cross_section(ink: &mut Ink, town: &Town, road: &Road, ch: f64, cell: (f64, f64)) -> Long {
    let reach = road.kind.reserve() + 8.0;
    let n = (reach * 2.0 / 0.25) as usize;
    let offs: Vec<f64> = (0..=n).map(|i| -reach + i as f64 * 0.25).collect();
    let at = |o: f64| road.point(ch, o);
    let design: Vec<Option<f64>> = offs.iter().map(|&o| town.design(at(o))).collect();
    let existing: Vec<f64> = offs.iter().map(|&o| ground(at(o).0, at(o).1)).collect();
    let lo = existing.iter().chain(design.iter().flatten()).cloned().fold(f64::MAX, f64::min);
    // Offsets run left to right as looking up the chainage: left of the
    // road on the left.
    let l = Long { ox: cell.0 + 34.0 + reach * 5.0, oy: cell.1 - 46.0, kh: -5.0, kv: 10.0, datum: (lo - 0.5).floor(), ch0: 0.0 };
    // Cut red, fill blue, hatched between the surfaces.
    let (mut cut, mut fill) = (0.0, 0.0);
    for (i, &o) in offs.iter().enumerate() {
        let Some(d) = design[i] else { continue };
        let e = existing[i];
        if (d - e).abs() > 0.005 {
            if i % 2 == 0 {
                ink.pen(0.12, if d < e { [0.85, 0.2, 0.2] } else { [0.2, 0.4, 0.85] });
                ink.line(l.p(o, e), l.p(o, d));
            }
            if d < e { cut += (e - d) * 0.25 } else { fill += (d - e) * 0.25 }
        }
    }
    ink.pen(0.25, [0.45, 0.3, 0.15]);
    ink.dash(&[1.5, 0.8]);
    let e_pts: Vec<(f64, f64)> = offs.iter().zip(&existing).map(|(&o, &e)| l.p(o, e)).collect();
    ink.poly(&e_pts, false, "S");
    ink.dash(&[]);
    ink.pen(0.45, BLACK);
    let mut run: Vec<(f64, f64)> = Vec::new();
    for (i, &o) in offs.iter().enumerate() {
        match design[i] {
            Some(d) => run.push(l.p(o, d)),
            None => {
                if run.len() > 1 {
                    ink.poly(&run, false, "S");
                }
                run.clear();
            }
        }
    }
    if run.len() > 1 {
        ink.poly(&run, false, "S");
    }
    // The pavement's depth under the carriageway.
    let k = road.kind;
    ink.pen(0.15, grey(0.3));
    ink.dash(&[0.8, 0.6]);
    for side in [-1.0, 1.0] {
        let (a, b) = (side * k.median().max(0.001), side * (k.half() + 0.6));
        ink.line(l.p(a, road.surface(ch, a) - 0.39), l.p(b, road.surface(ch, k.half()) - 0.39 - 0.02));
    }
    ink.dash(&[]);
    // The centreline and the boundaries.
    ink.pen(0.15, BLACK);
    ink.dash(&[3.0, 0.8, 0.6, 0.8]);
    let top = l.p(0.0, road.surface(ch, 0.0));
    ink.line((top.0, l.oy), (top.0, top.1 + 7.0));
    ink.dash(&[1.5, 1.0]);
    for side in [-1.0, 1.0] {
        let o = side * k.reserve();
        let p = l.p(o, road.surface(ch, o));
        ink.line((p.0, l.oy), (p.0, p.1 + 5.0));
    }
    ink.dash(&[]);
    ink.pen(0.25, BLACK);
    ink.line(l.p(-reach, l.datum), l.p(reach, l.datum));
    ink.fill(BLACK);
    ink.text((top.0, top.1 + 8.0), 1.8, Font::Bold, Align::Centre, "CL");

    // Levels at the kerbs and boundaries, below.
    let keys = [-k.reserve(), -k.half(), 0.0, k.half(), k.reserve()];
    let key_pts: Vec<(f64, f64)> = keys.iter().map(|&o| l.p(o, 0.0)).collect();
    let xs: Vec<f64> = key_pts.iter().map(|p| p.0).collect();
    let rows = vec![
        ("DESIGN", keys.iter().map(|&o| format!("{:.3}", road.surface(ch, o))).collect::<Vec<_>>()),
        ("NATURAL", keys.iter().map(|&o| format!("{:.3}", ground(at(o).0, at(o).1))).collect()),
        ("OFFSET", keys.iter().map(|&o| format!("{:.2}", 0.0 - o + 0.0)).collect()),
    ];
    let left_x = cell.0;
    let right_x = cell.0 + 258.0;
    let bottom = bands(ink, left_x, l.oy - 1.5, right_x, &xs, &rows, 10.0);
    ink.fill(BLACK);
    ink.text((left_x + 2.0, l.oy + 1.0), 1.8, Font::Regular, Align::Left, &format!("DATUM RL {:.2}", l.datum));
    ink.text((cell.0, cell.1 - 2.0), 3.0, Font::Bold, Align::Left, &format!("{}  CH {ch:.2}", road.name.to_uppercase()));
    ink.fill(grey(0.3));
    ink.text((cell.0, cell.1 - 6.2), 2.0, Font::Regular, Align::Left, &format!("CUT {cut:.2} m²   FILL {fill:.2} m²   CROSSFALL 3%"));
    let _ = bottom;
    l
}

/// The pits along a line in panels of up to PANEL metres each.
pub fn pit_panels(town: &Town, line: &Line) -> Vec<Vec<usize>> {
    let mut panels = Vec::new();
    let mut run = vec![line.pits[0]];
    let mut length = 0.0;
    for w in line.pits.windows(2) {
        let d = dist(town.pits[w[0]].at, town.pits[w[1]].at);
        if length + d > PANEL && run.len() > 1 {
            panels.push(std::mem::take(&mut run));
            run.push(w[0]);
            length = 0.0;
        }
        run.push(w[1]);
        length += d;
    }
    panels.push(run);
    panels
}

/// One panel of a pipe line's long section: `path` of pits, top to bottom,
/// from `left_x`, its datum line at `oy`. `names` and levels from `pts`.
pub struct Station {
    pub name: String,
    pub at: P,
    pub surface: f64,
    pub invert: f64,
    /// A pit to its base, or just a line for a headwall.
    pub width: f64,
}

pub struct Run {
    pub dia: f64,
    pub up: f64,
    pub down: f64,
    pub grade: f64,
    pub length: f64,
    pub label: String,
    pub hgl: Option<(f64, f64)>,
}

pub fn pipe_panel(ink: &mut Ink, left_x: f64, oy: f64, stations: &[Station], runs: &[Run], title: (&str, &str), grade_as_ratio: bool) -> Long {
    let mut chs = vec![0.0];
    for w in stations.windows(2) {
        chs.push(chs.last().expect("a chainage") + dist(w[0].at, w[1].at));
    }
    let lo = stations.iter().map(|s| s.invert - 0.3).fold(f64::MAX, f64::min);
    let l = Long { ox: left_x + 60.0, oy, kh: 2.0, kv: 10.0, datum: (lo - 0.5).floor(), ch0: 0.0 };
    let end = *chs.last().expect("a chainage");
    let xs: Vec<f64> = chs.iter().map(|&c| l.p(c, 0.0).0).collect();
    ink.pen(0.35, BLACK);
    ink.line(l.p(0.0, l.datum), l.p(end, l.datum));
    ink.pen(0.1, grey(0.6));
    for (s, &c) in stations.iter().zip(&chs) {
        ink.line(l.p(c, l.datum), l.p(c, s.surface));
    }
    ink.pen(0.45, BLACK);
    let surface: Vec<(f64, f64)> = stations.iter().zip(&chs).map(|(s, &c)| l.p(c, s.surface)).collect();
    ink.poly(&surface, false, "S");
    for (k, r) in runs.iter().enumerate() {
        let (a, b) = (chs[k], chs[k + 1]);
        ink.fill(CREEK);
        ink.poly(&[l.p(a, r.up), l.p(b, r.down), l.p(b, r.down + r.dia), l.p(a, r.up + r.dia)], true, "f");
        ink.pen(0.3, STORM);
        ink.line(l.p(a, r.up), l.p(b, r.down));
        ink.line(l.p(a, r.up + r.dia), l.p(b, r.down + r.dia));
        if let Some((h0, h1)) = r.hgl {
            ink.pen(0.25, STORM);
            ink.dash(&[1.5, 1.0]);
            ink.line(l.p(a, h0), l.p(b, h1));
            ink.dash(&[]);
        }
    }
    for (s, &c) in stations.iter().zip(&chs) {
        let (x, top) = l.p(c, s.surface);
        let (_, floor) = l.p(c, s.invert - 0.15);
        ink.fill(WHITE);
        ink.pen(0.3, BLACK);
        ink.rect(x - s.width / 2.0, floor, s.width, top - floor, "B");
        ink.fill(BLACK);
        ink.text_turned((x + 0.7, top + 2.0), 2.0, Font::Bold, Align::Left, &s.name, true);
    }
    let left = left_x;
    let right = l.p(end, 0.0).0 + 6.0;
    ink.fill(BLACK);
    ink.text((left + 2.0, l.oy + 2.0), 2.1, Font::Bold, Align::Left, &format!("DATUM RL {:.2}", l.datum));
    let grade = |r: &Run| if grade_as_ratio { format!("1 IN {:.0}", 1.0 / r.grade) } else { format!("{:.2}", r.grade * 100.0) };
    let rows = vec![
        ("SURFACE RL", stations.iter().map(|s| format!("{:.3}", s.surface)).collect::<Vec<_>>()),
        ("INVERT RL", stations.iter().map(|s| format!("{:.3}", s.invert)).collect()),
        ("DEPTH", stations.iter().map(|s| format!("{:.2}", s.surface - s.invert)).collect()),
        ("PIPE", runs.iter().map(|r| r.label.clone()).collect()),
        (if grade_as_ratio { "GRADE" } else { "GRADE %" }, runs.iter().map(grade).collect()),
        ("LENGTH", runs.iter().map(|r| format!("{:.2}", r.length)).collect()),
        ("CHAINAGE", chs.iter().map(|c| format!("{c:.2}")).collect()),
    ];
    let bottom = bands(ink, left, l.oy, right, &xs, &rows, 8.0);
    heading_under(ink, (left, bottom - 9.0), title.0, title.1);
    l
}

pub fn drain_stations(town: &Town, path: &[usize]) -> (Vec<Station>, Vec<Run>) {
    let stations = path
        .iter()
        .map(|&i| {
            let p = &town.pits[i];
            Station { name: p.name.clone(), at: p.at, surface: p.surface, invert: p.invert, width: if p.kind == PitKind::Headwall { 0.6 } else { 1.8 } }
        })
        .collect();
    let runs = path
        .windows(2)
        .map(|w| {
            let pipe = town.pipes.iter().find(|p| p.from == w[0] && p.to == w[1]).expect("a pipe between them");
            let full = (pipe.q100 / pipe.capacity).min(1.3);
            Run {
                dia: pipe.dia,
                up: pipe.up,
                down: pipe.down,
                grade: pipe.grade,
                length: pipe.length,
                label: format!("{:.0}Ø", pipe.dia * 1000.0),
                hgl: Some((pipe.up + pipe.dia * (0.55 + 0.4 * full), pipe.down + pipe.dia * (0.5 + 0.4 * full))),
            }
        })
        .collect();
    (stations, runs)
}

pub fn sewer_stations(sewer: &Sewer, from: usize, to: usize) -> (Vec<Station>, Vec<Run>) {
    let mhs = &sewer.manholes[from..=to];
    let stations = mhs.iter().map(|m| Station { name: m.name.clone(), at: m.at, surface: m.surface, invert: m.invert, width: 2.0 }).collect();
    let runs = mhs
        .windows(2)
        .map(|w| {
            let length = dist(w[0].at, w[1].at);
            let down = w[1].invert + 0.02;
            Run { dia: 0.15, up: w[0].invert, down, grade: (w[0].invert - down) / length, length, label: "150 PVC".into(), hgl: None }
        })
        .collect();
    (stations, runs)
}

/// Pack panels of these lengths into rows no longer than PANEL, two rows a
/// sheet. Returns each sheet's rows of indices.
pub fn pack(lengths: &[f64]) -> Vec<Vec<Vec<usize>>> {
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut used = f64::MAX;
    for (i, &len) in lengths.iter().enumerate() {
        let need = len + 45.0;
        if used + need > PANEL + 60.0 {
            rows.push(Vec::new());
            used = 0.0;
        }
        rows.last_mut().expect("a row").push(i);
        used += need;
    }
    rows.chunks(2).map(|c| c.to_vec()).collect()
}
