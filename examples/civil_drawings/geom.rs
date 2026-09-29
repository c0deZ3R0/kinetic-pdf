//! Geometry in metres on the site: points, outlines rounded at their
//! corners, offsets, clipping, walking along lines, contours, and a road's
//! vertical profile.

use std::collections::HashMap;
use std::f64::consts::PI;

pub type P = (f64, f64);
/// A corner of an outline, and the radius it's rounded to.
pub type Corner = (f64, f64, f64);
/// Left, bottom, right, top.
pub type Bx = (f64, f64, f64, f64);

pub fn sub(a: P, b: P) -> P {
    (a.0 - b.0, a.1 - b.1)
}

pub fn add(a: P, b: P) -> P {
    (a.0 + b.0, a.1 + b.1)
}

pub fn mul(a: P, k: f64) -> P {
    (a.0 * k, a.1 * k)
}

pub fn dist(a: P, b: P) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

pub fn unit(a: P) -> P {
    mul(a, 1.0 / a.0.hypot(a.1))
}

pub fn cross(a: P, b: P) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

/// A quarter turn anticlockwise.
pub fn left(a: P) -> P {
    (-a.1, a.0)
}

pub fn rect_pts((x0, y0, x1, y1): Bx) -> Vec<P> {
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

pub fn in_box(p: P, b: Bx) -> bool {
    p.0 >= b.0 && p.0 <= b.2 && p.1 >= b.1 && p.1 <= b.3
}

pub fn overlaps(a: Bx, b: Bx) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

pub fn grow(b: Bx, by: f64) -> Bx {
    (b.0 - by, b.1 - by, b.2 + by, b.3 + by)
}

pub fn bounds(pts: &[P]) -> Bx {
    pts.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1)))
}

pub fn centre(b: Bx) -> P {
    ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0)
}

pub fn polygon_area(pts: &[P]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| cross(pts[i], pts[(i + 1) % n])).sum::<f64>().abs() / 2.0
}

pub fn polyline_len(pts: &[P]) -> f64 {
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// A ring with its first point again at the end, to walk round.
pub fn closed(pts: &[P]) -> Vec<P> {
    let mut out = pts.to_vec();
    out.push(pts[0]);
    out
}

/// Whether `p` is inside the polygon `pts`.
pub fn inside(pts: &[P], p: P) -> bool {
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

/// `corners` with each rounded to its radius, as points close enough
/// together to draw and measure as the curve. An open line keeps its ends.
pub fn rounded(corners: &[Corner], closed: bool) -> Vec<P> {
    let n = corners.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (x, y, r) = corners[i];
        let v = (x, y);
        if r <= 0.0 || (!closed && (i == 0 || i == n - 1)) {
            out.push(v);
            continue;
        }
        let (prev, next) = (corners[(i + n - 1) % n], corners[(i + 1) % n]);
        let u1 = unit(sub(v, (prev.0, prev.1)));
        let u2 = unit(sub((next.0, next.1), v));
        let turn = cross(u1, u2).atan2(u1.0 * u2.0 + u1.1 * u2.1);
        if turn.abs() < 1e-9 {
            out.push(v);
            continue;
        }
        let t1 = sub(v, mul(u1, r * (turn.abs() / 2.0).tan()));
        let c = add(t1, mul(left(u1), r * turn.signum()));
        let a0 = (t1.1 - c.1).atan2(t1.0 - c.0);
        let steps = ((turn.abs() * r / 0.3).ceil() as usize).max((turn.abs() / 3f64.to_radians()).ceil() as usize);
        for s in 0..=steps {
            let a = a0 + turn * s as f64 / steps as f64;
            out.push((c.0 + r * a.cos(), c.1 + r * a.sin()));
        }
    }
    out
}

/// `corners` with edge i (from corner i to the next) moved `d[i]` to its
/// left: into an outline that runs anticlockwise. Each corner's radius grows
/// or shrinks with it, so the curves stay concentric.
pub fn offset(corners: &[Corner], d: &[f64], closed: bool) -> Vec<Corner> {
    let n = corners.len();
    let pt = |i: usize| (corners[i].0, corners[i].1);
    let line = |e: usize| {
        let (a, b) = (pt(e), pt((e + 1) % n));
        let u = unit(sub(b, a));
        (add(a, mul(left(u), d[e])), u)
    };
    (0..n)
        .map(|i| {
            let v = pt(i);
            let (has_prev, has_next) = (closed || i > 0, closed || i + 1 < n);
            let before = (i + n - 1) % n;
            let p = match (has_prev, has_next) {
                (true, true) => {
                    let ((a, u), (b, w)) = (line(before), line(i));
                    let den = cross(u, w);
                    if den.abs() < 1e-12 { add(v, mul(left(w), d[i])) } else { add(a, mul(u, cross(sub(b, a), w) / den)) }
                }
                (false, true) => add(v, mul(left(line(i).1), d[i])),
                (true, false) => add(v, mul(left(line(before).1), d[before])),
                _ => v,
            };
            let r = corners[i].2;
            let r = if r > 0.0 && has_prev && has_next {
                let (u, w) = (line(before).1, line(i).1);
                let dd = (d[before] + d[i]) / 2.0;
                if cross(u, w) > 0.0 { (r - dd).max(0.0) } else { r + dd }
            } else {
                r
            };
            (p.0, p.1, r)
        })
        .collect()
}

/// A polygon cut to a box.
pub fn clip_polygon(pts: &[P], b: Bx) -> Vec<P> {
    let mut out = pts.to_vec();
    for side in 0..4 {
        let inside = |p: P| match side {
            0 => p.0 >= b.0,
            1 => p.0 <= b.2,
            2 => p.1 >= b.1,
            _ => p.1 <= b.3,
        };
        let cut = |a: P, c: P| {
            let t = match side {
                0 => (b.0 - a.0) / (c.0 - a.0),
                1 => (b.2 - a.0) / (c.0 - a.0),
                2 => (b.1 - a.1) / (c.1 - a.1),
                _ => (b.3 - a.1) / (c.1 - a.1),
            };
            add(a, mul(sub(c, a), t))
        };
        let input = std::mem::take(&mut out);
        for i in 0..input.len() {
            let (a, c) = (input[(i + input.len() - 1) % input.len()], input[i]);
            match (inside(a), inside(c)) {
                (true, true) => out.push(c),
                (true, false) => out.push(cut(a, c)),
                (false, true) => {
                    out.push(cut(a, c));
                    out.push(c);
                }
                _ => {}
            }
        }
    }
    out
}

pub fn clip_segment(a: P, b: P, bx: Bx) -> Option<(P, P)> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let d = sub(b, a);
    for (p, q) in [(-d.0, a.0 - bx.0), (d.0, bx.2 - a.0), (-d.1, a.1 - bx.1), (d.1, bx.3 - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return None;
            }
            t0 = t0.max(r);
        } else {
            if r < t0 {
                return None;
            }
            t1 = t1.min(r);
        }
    }
    Some((add(a, mul(d, t0)), add(a, mul(d, t1))))
}

/// The stretches of a line inside a box.
pub fn clip_polyline(pts: &[P], bx: Bx) -> Vec<Vec<P>> {
    let mut runs: Vec<Vec<P>> = Vec::new();
    let mut run: Vec<P> = Vec::new();
    for w in pts.windows(2) {
        match clip_segment(w[0], w[1], bx) {
            Some((a, b)) => {
                if run.last().is_some_and(|&l| dist(l, a) > 1e-9) {
                    runs.push(std::mem::take(&mut run));
                }
                if run.is_empty() {
                    run.push(a);
                }
                run.push(b);
            }
            None => {
                if !run.is_empty() {
                    runs.push(std::mem::take(&mut run));
                }
            }
        }
    }
    runs.push(run);
    runs.retain(|r| r.len() > 1 && polyline_len(r) > 0.5);
    runs
}

/// Points `step` apart along a line from `start` in, each with the way the
/// line runs there.
pub fn stations(pts: &[P], start: f64, step: f64) -> Vec<(P, P)> {
    let mut out = Vec::new();
    let (mut next, mut walked) = (start, 0.0);
    for w in pts.windows(2) {
        let l = dist(w[0], w[1]);
        if l < 1e-12 {
            continue;
        }
        let u = unit(sub(w[1], w[0]));
        while next <= walked + l {
            out.push((add(w[0], mul(u, next - walked)), u));
            next += step;
        }
        walked += l;
    }
    out
}

/// The angle of `u`, turned half round if text along it would read upside
/// down.
pub fn upright(u: P) -> f64 {
    let a = u.1.atan2(u.0);
    if u.0 < -1e-9 || (u.0.abs() <= 1e-9 && u.1 < 0.0) { a + PI } else { a }
}

/// Contour lines of `z` over the box at every `interval`, found on a grid
/// `step` apart and joined into lines: (level, points) for each.
pub fn contours(bx: Bx, step: f64, interval: f64, z: impl Fn(f64, f64) -> f64) -> Vec<(f64, Vec<P>)> {
    let nx = ((bx.2 - bx.0) / step).ceil() as usize;
    let ny = ((bx.3 - bx.1) / step).ceil() as usize;
    let grid = |i: usize, j: usize| (bx.0 + i as f64 * step, bx.1 + j as f64 * step);
    let zs: Vec<Vec<f64>> = (0..=nx).map(|i| (0..=ny).map(|j| z(grid(i, j).0, grid(i, j).1)).collect()).collect();
    let (lo, hi) = zs.iter().flatten().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let mut out = Vec::new();
    let mut level = (lo / interval).ceil() * interval;
    while level <= hi {
        // Each crossing is named by the grid edge it's on, so the two cells
        // either side of an edge agree on it and the pieces join up.
        type Edge = (usize, usize, bool);
        let mut links: HashMap<Edge, Vec<Edge>> = HashMap::new();
        let mut at: HashMap<Edge, P> = HashMap::new();
        for i in 0..nx {
            for j in 0..ny {
                let edges: [(Edge, (usize, usize), (usize, usize)); 4] = [
                    ((i, j, true), (i, j), (i + 1, j)),
                    ((i + 1, j, false), (i + 1, j), (i + 1, j + 1)),
                    ((i, j + 1, true), (i, j + 1), (i + 1, j + 1)),
                    ((i, j, false), (i, j), (i, j + 1)),
                ];
                let mut cuts = Vec::new();
                for (edge, a, b) in edges {
                    let (za, zb) = (zs[a.0][a.1], zs[b.0][b.1]);
                    if (za < level) != (zb < level) {
                        let (pa, pb) = (grid(a.0, a.1), grid(b.0, b.1));
                        at.entry(edge).or_insert_with(|| add(pa, mul(sub(pb, pa), (level - za) / (zb - za))));
                        cuts.push(edge);
                    }
                }
                for pair in cuts.chunks(2).filter(|c| c.len() == 2) {
                    links.entry(pair[0]).or_default().push(pair[1]);
                    links.entry(pair[1]).or_default().push(pair[0]);
                }
            }
        }
        // Walk each line from an end, then round any loops left.
        let mut starts: Vec<Edge> = links.iter().filter(|(_, v)| v.len() == 1).map(|(k, _)| *k).collect();
        starts.sort();
        let mut rest: Vec<Edge> = links.keys().copied().collect();
        rest.sort();
        starts.extend(rest);
        for start in starts {
            if links.get(&start).is_none_or(|v| v.is_empty()) {
                continue;
            }
            let mut line = vec![at[&start]];
            let mut here = start;
            while let Some(next) = links.get_mut(&here).and_then(|v| v.pop()) {
                if let Some(back) = links.get_mut(&next) {
                    if let Some(k) = back.iter().position(|e| *e == here) {
                        back.swap_remove(k);
                    }
                }
                line.push(at[&next]);
                here = next;
            }
            if line.len() > 1 {
                out.push((level, line));
            }
        }
        level += interval;
    }
    out
}

/// A road's design levels along its centreline: intersection points of the
/// grades, each with the length of the vertical curve round it.
pub struct Profile {
    pub ips: Vec<(f64, f64, f64)>,
}

impl Profile {
    pub fn grade(&self, i: usize) -> f64 {
        let (a, b) = (self.ips[i], self.ips[i + 1]);
        (b.1 - a.1) / (b.0 - a.0)
    }

    pub fn level(&self, ch: f64) -> f64 {
        let ips = &self.ips;
        for i in 1..ips.len() - 1 {
            let (c, y, l) = ips[i];
            if l > 0.0 && (ch - c).abs() <= l / 2.0 {
                let (g1, g2) = (self.grade(i - 1), self.grade(i));
                let x = ch - (c - l / 2.0);
                return y - g1 * l / 2.0 + g1 * x + (g2 - g1) * x * x / (2.0 * l);
            }
        }
        let i = ips.windows(2).position(|w| ch <= w[1].0).unwrap_or(ips.len() - 2);
        let (a, b) = (ips[i], ips[i + 1]);
        a.1 + (b.1 - a.1) * (ch - a.0) / (b.0 - a.0)
    }
}

/// A small, repeatable random number generator.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}
