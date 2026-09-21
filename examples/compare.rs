//! Comparing two PDF pages by their geometry rather than their pixels.
//!
//! The usual way to compare drawings -- Bluebeam's Compare Documents, and
//! everything like it -- is to render both pages to bitmaps at some DPI and
//! diff the pixels. That needs a sensitivity, a density and a threshold,
//! because antialiasing puts noise in every result, and it throws away what
//! the PDF already knew: that this line is *this* line, moved.
//!
//! A vector PDF doesn't need any of that. `gpu-lines` already turns a page
//! into `Shapes` -- flattened strokes, filled triangles, placed images -- so
//! two pages can be matched primitive by primitive:
//!
//! 1. Fingerprint each primitive by its *relative* geometry and the style it
//!    is painted with, keeping its position apart. Identical linework gets an
//!    identical fingerprint wherever it sits on the sheet.
//! 2. Vote for the offset between the pages, using only fingerprints that
//!    occur exactly once on each side, so the hundreds of thousands of
//!    identical hairline pieces don't drown out the distinctive geometry.
//! 3. Match A to B through a grid at the winning offset. Matched is
//!    unchanged; what's left is added, deleted or moved.
//! 4. Fit a similarity transform -- rotation, one scale, translation -- to
//!    the matched pairs in closed form. Horn's solution: what OpenCV's
//!    `estimateRigidTransform`/`estimateAffinePartial2D` solves for, in about
//!    twenty lines and no dependency.
//! 5. Cluster what's left into boxes, which is where clouds would go.
//!
//! Step 1's fingerprint is deliberately not invariant to rotation or scale:
//! making it so costs far more than it is worth, because two revisions of a
//! sheet from the same CAD system share an origin exactly. The cases that
//! don't are few and discrete -- a page turned by a right angle, a drawing
//! plotted to another sheet size -- so `candidates` tries them, scored by
//! the vote alone, which is cheap. Anything worse than that (a scan, a
//! photograph) belongs to a raster tier, not here.
//!
//! Run it:
//!
//! ```text
//! cargo run --release --example compare -- a.pdf b.pdf
//! cargo run --release --example compare -- a.pdf              (page 1 vs 2)
//! cargo run --release --example compare -- a.pdf --self       (a page vs itself)
//! cargo run --release --example compare -- a.pdf --self --shift 12 -5 --rotate 90
//! cargo run --release --example compare -- a.pdf --self --rotate 0.35 --sweep 1 0.05
//! ```
//!
//! `--shift`/`--scale`/`--rotate` move B before matching, to see whether the
//! candidate search puts it back. `--sweep MAX STEP` adds a fine rotation
//! sweep to the candidates, for the awkward case.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::time::Instant;

use gpu_lines::{lopdf::Document, page_shapes, Shapes, MOST_IMAGE_DENSITY};

/// Curves are flattened to within this many points, as the app does.
const TOLERANCE: f32 = 0.05;

/// Page units per matching cell. A drawing is in points, 72 to the inch, so
/// this is about a thousandth of an inch: far below anything anyone draws,
/// far above the last bits of an f32 after a matrix multiply.
const QUANT: f32 = 0.01;

/// Page units per offset-vote bucket. Coarser than `QUANT`: the vote only has
/// to get close enough for the neighbourhood search to finish the job.
const VOTE: f32 = 0.5;

/// Unmatched primitives within this many points of each other are one
/// difference. Bluebeam calls this the proximity range.
const CLUSTER: f32 = 6.0;

/// Boxes smaller than this either way are noise, not a difference.
const SMALLEST: f32 = 0.5;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- transforms

/// A similarity: turn, one scale, then a move. Everything alignment can
/// express, and everything `estimateAffinePartial2D` solves for.
#[derive(Clone, Copy, PartialEq)]
struct Sim {
    rot: f32,
    scale: f32,
    tx: f32,
    ty: f32,
}

impl Sim {
    const ID: Sim = Sim { rot: 0.0, scale: 1.0, tx: 0.0, ty: 0.0 };

    fn turn(degrees: f32) -> Sim {
        Sim { rot: degrees.to_radians(), ..Sim::ID }
    }

    fn is_id(&self) -> bool {
        *self == Sim::ID
    }

    #[inline]
    fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        let (sin, cos) = self.rot.sin_cos();
        [self.scale * (p[0] * cos - p[1] * sin) + self.tx, self.scale * (p[0] * sin + p[1] * cos) + self.ty]
    }

    /// `self` after `inner`: `a.after(b).apply(p) == a.apply(b.apply(p))`.
    fn after(&self, inner: Sim) -> Sim {
        let moved = self.apply([inner.tx, inner.ty]);
        Sim { rot: self.rot + inner.rot, scale: self.scale * inner.scale, tx: moved[0], ty: moved[1] }
    }

    fn describe(&self) -> String {
        if self.is_id() {
            return "as it is".to_owned();
        }
        let mut parts = Vec::new();
        if self.rot != 0.0 {
            parts.push(format!("turned {:+.3}deg", self.rot.to_degrees()));
        }
        if self.scale != 1.0 {
            parts.push(format!("scaled {:.5}", self.scale));
        }
        if self.tx != 0.0 || self.ty != 0.0 {
            parts.push(format!("moved [{:+.2}, {:+.2}]", self.tx, self.ty));
        }
        parts.join(", ")
    }
}

// -------------------------------------------------------------- fingerprints

/// One primitive, ready to match: what it looks like, where it is.
#[derive(Clone)]
struct Print {
    /// Its relative geometry and style, hashed. Position is not in here.
    shape: u64,
    /// Its first point, which is where the fingerprint sits.
    at: [f32; 2],
    /// Its other two points relative to `at`, quantised, to tell apart the
    /// primitives that happen to share a hash.
    rel: [i32; 4],
    /// Matched to the primitive at this index on the other side.
    pair: Option<u32>,
    taken: bool,
}

fn q(v: f32) -> i32 {
    (v / QUANT).round() as i32
}

/// A 64-bit mix (splitmix64's finaliser). The fingerprints go into hash maps
/// as their own hashes, so they have to be well spread on their own.
#[inline]
fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^= x >> 33;
    x
}

#[inline]
fn mix2(a: u64, b: u64) -> u64 {
    mix(a ^ mix(b).rotate_left(31))
}

/// Fingerprint every primitive, with `t` applied to it first.
fn fingerprint(shapes: &Shapes, t: Sim) -> Vec<Print> {
    let id = t.is_id();
    shapes
        .primitives
        .iter()
        .map(|p| {
            let style = shapes.style_of(p);
            let [p0, p1, p2] = if id { p.points } else { p.points.map(|c| t.apply(c)) };
            let rel = [q(p1[0] - p0[0]), q(p1[1] - p0[1]), q(p2[0] - p0[0]), q(p2[1] - p0[1])];
            // An image's colour is where it landed in the texture atlas,
            // which has nothing to do with the drawing, so it is left out.
            // Images are a raster tier's problem anyway.
            let paint = if style.is_image() {
                mix(0xa111_9a6e_0000_0005)
            } else {
                let c = style.colour;
                mix2(
                    (style.width.to_bits() as u64) << 32 | style.kind.to_bits() as u64,
                    (c[0].to_bits() as u64) << 32 | c[1].to_bits() as u64 ^ mix((c[2].to_bits() as u64) << 32 | c[3].to_bits() as u64),
                )
            };
            let geom = mix2((rel[0] as u32 as u64) << 32 | rel[1] as u32 as u64, (rel[2] as u32 as u64) << 32 | rel[3] as u32 as u64);
            Print { shape: mix2(geom, paint), at: p0, rel, pair: None, taken: false }
        })
        .collect()
}

/// A hasher for keys that are already well-mixed 64-bit values: it passes
/// them straight through rather than paying for SipHash a few million times.
#[derive(Default, Clone, Copy)]
struct Pass(u64);

impl Hasher for Pass {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ b as u64;
        }
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }
}

#[derive(Default, Clone, Copy)]
struct PassBuild;

impl BuildHasher for PassBuild {
    type Hasher = Pass;
    fn build_hasher(&self) -> Pass {
        Pass(0)
    }
}

type Map<V> = HashMap<u64, V, PassBuild>;

/// Fingerprints seen exactly once on each side, paired up. A sheet has
/// hundreds of thousands of identical hairline pieces, and they know nothing
/// about where the page has moved; these do.
fn distinctive(fa: &[Print], fb: &[Print]) -> Vec<(u32, u32)> {
    let once = |f: &[Print]| -> Map<(u32, u32)> {
        let mut seen: Map<(u32, u32)> = Map::default();
        seen.reserve(f.len());
        for (i, p) in f.iter().enumerate() {
            let e = seen.entry(p.shape).or_insert((0, 0));
            e.0 += 1;
            e.1 = i as u32;
        }
        seen
    };
    let (once_a, once_b) = (once(fa), once(fb));
    once_a
        .iter()
        .filter(|(_, &(n, _))| n == 1)
        .filter_map(|(shape, &(_, ia))| match once_b.get(shape) {
            Some(&(1, ib)) => Some((ia, ib)),
            _ => None,
        })
        .collect()
}

/// The offset the distinctive pairs agree on, and how many agreed.
fn offset_vote(fa: &[Print], fb: &[Print], pairs: &[(u32, u32)]) -> ([f32; 2], usize) {
    let bucket = |a: [f32; 2], b: [f32; 2]| mix2((((b[0] - a[0]) / VOTE).round() as i32 as u32 as u64) << 32, ((b[1] - a[1]) / VOTE).round() as i32 as u32 as u64);

    let mut votes: Map<usize> = Map::default();
    for &(ia, ib) in pairs {
        *votes.entry(bucket(fa[ia as usize].at, fb[ib as usize].at)).or_insert(0) += 1;
    }
    let Some((&best, &n)) = votes.iter().max_by_key(|(_, &n)| n) else {
        return ([0.0, 0.0], 0);
    };
    // The mean of the deltas in the winning bucket, so the offset isn't
    // itself stuck to the half-point grid.
    let (mut sx, mut sy, mut used) = (0.0f64, 0.0f64, 0usize);
    for &(ia, ib) in pairs {
        let (a, b) = (fa[ia as usize].at, fb[ib as usize].at);
        if bucket(a, b) == best {
            sx += (b[0] - a[0]) as f64;
            sy += (b[1] - a[1]) as f64;
            used += 1;
        }
    }
    let k = used.max(1) as f64;
    ([(sx / k) as f32, (sy / k) as f32], n)
}

/// Match A's primitives to B's at `offset`, one to one. A primitive matches
/// one with the same fingerprint whose position agrees to within a cell, and
/// the neighbouring cells are searched too, so a coordinate that rounds
/// across a boundary still finds its partner.
fn match_up(fa: &mut [Print], fb: &mut [Print], offset: [f32; 2]) -> usize {
    let cell = |shape: u64, x: i32, y: i32| mix2(shape, (x as u32 as u64) << 32 | y as u32 as u64);

    let mut grid: Map<Vec<u32>> = Map::default();
    grid.reserve(fb.len());
    for (i, f) in fb.iter().enumerate() {
        grid.entry(cell(f.shape, q(f.at[0]), q(f.at[1]))).or_default().push(i as u32);
    }

    let mut matched = 0usize;
    for i in 0..fa.len() {
        let (shape, at, rel) = (fa[i].shape, fa[i].at, fa[i].rel);
        let (x, y) = (q(at[0] + offset[0]), q(at[1] + offset[1]));
        let mut found = None;
        'search: for dy in -1..=1 {
            for dx in -1..=1 {
                let Some(bucket) = grid.get(&cell(shape, x + dx, y + dy)) else { continue };
                for &j in bucket {
                    let b = &fb[j as usize];
                    // Verified, not trusted: same relative geometry, and near
                    // enough in the same place. A hash collision cannot
                    // invent a match.
                    if !b.taken && b.rel == rel && (q(b.at[0]) - x).abs() <= 1 && (q(b.at[1]) - y).abs() <= 1 {
                        found = Some(j);
                        break 'search;
                    }
                }
            }
        }
        if let Some(j) = found {
            fa[i].taken = true;
            fa[i].pair = Some(j);
            fb[j as usize].taken = true;
            fb[j as usize].pair = Some(i as u32);
            matched += 1;
        }
    }
    matched
}

// ------------------------------------------------------------------- fitting

struct Transform {
    rotation: f32,
    scale: f32,
    tx: f32,
    ty: f32,
    mean: f32,
    worst: f32,
}

/// The least-squares similarity taking the first of each pair to the second.
/// Horn's closed form: two sums and an `atan2`. No matrices, no iteration, no
/// library.
fn similarity(pairs: &[([f32; 2], [f32; 2])]) -> Option<Transform> {
    if pairs.len() < 2 {
        return None;
    }
    let n = pairs.len() as f64;
    let (mut ax, mut ay, mut bx, mut by) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (a, b) in pairs {
        ax += a[0] as f64;
        ay += a[1] as f64;
        bx += b[0] as f64;
        by += b[1] as f64;
    }
    let (ax, ay, bx, by) = (ax / n, ay / n, bx / n, by / n);

    let (mut dot, mut cross, mut norm) = (0.0f64, 0.0f64, 0.0f64);
    for (a, b) in pairs {
        let (px, py) = (a[0] as f64 - ax, a[1] as f64 - ay);
        let (qx, qy) = (b[0] as f64 - bx, b[1] as f64 - by);
        dot += px * qx + py * qy;
        cross += px * qy - py * qx;
        norm += px * px + py * py;
    }
    if norm <= f64::EPSILON {
        return None;
    }
    let rotation = cross.atan2(dot);
    let scale = (dot * rotation.cos() + cross * rotation.sin()) / norm;
    let (sin, cos) = (rotation.sin(), rotation.cos());
    let tx = bx - scale * (ax * cos - ay * sin);
    let ty = by - scale * (ax * sin + ay * cos);

    let (mut sum, mut worst) = (0.0f64, 0.0f64);
    for (a, b) in pairs {
        let x = scale * (a[0] as f64 * cos - a[1] as f64 * sin) + tx;
        let y = scale * (a[0] as f64 * sin + a[1] as f64 * cos) + ty;
        let d = ((x - b[0] as f64).powi(2) + (y - b[1] as f64).powi(2)).sqrt();
        sum += d;
        worst = worst.max(d);
    }
    Some(Transform { rotation: rotation as f32, scale: scale as f32, tx: tx as f32, ty: ty as f32, mean: (sum / n) as f32, worst: worst as f32 })
}

// ---------------------------------------------------------------- clustering

struct Difference {
    min: [f32; 2],
    max: [f32; 2],
    count: usize,
    added: usize,
    deleted: usize,
}

/// Unmatched primitives gathered into boxes: everything within `CLUSTER`
/// points of something else in the box is one difference. Cells of that size
/// are joined to their eight neighbours through a union-find, which is the
/// cheap way -- no tree, one pass.
fn cluster_boxes(a: &[&Print], b: &[&Print]) -> Vec<Difference> {
    let key = |x: i32, y: i32| mix2((x as u32 as u64) << 32, y as u32 as u64);

    let mut cells: Map<usize> = Map::default();
    let mut parent: Vec<usize> = Vec::new();
    let mut at: Vec<(i32, i32)> = Vec::new();
    let mut members: Vec<usize> = Vec::with_capacity(a.len() + b.len());

    for list in [a, b] {
        for p in list {
            let (x, y) = ((p.at[0] / CLUSTER).floor() as i32, (p.at[1] / CLUSTER).floor() as i32);
            let id = *cells.entry(key(x, y)).or_insert_with(|| {
                parent.push(parent.len());
                at.push((x, y));
                parent.len() - 1
            });
            members.push(id);
        }
    }

    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }

    for id in 0..at.len() {
        let (x, y) = at[id];
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                if let Some(&other) = cells.get(&key(x + dx, y + dy)) {
                    let (i, j) = (find(&mut parent, id), find(&mut parent, other));
                    if i != j {
                        parent[i] = j;
                    }
                }
            }
        }
    }

    let mut boxes: HashMap<usize, Difference> = HashMap::new();
    let mut m = 0usize;
    for (added, list) in [(false, a), (true, b)] {
        for p in list {
            let root = find(&mut parent, members[m]);
            m += 1;
            let d = boxes.entry(root).or_insert(Difference { min: [f32::MAX; 2], max: [f32::MIN; 2], count: 0, added: 0, deleted: 0 });
            d.min[0] = d.min[0].min(p.at[0]);
            d.min[1] = d.min[1].min(p.at[1]);
            d.max[0] = d.max[0].max(p.at[0]);
            d.max[1] = d.max[1].max(p.at[1]);
            d.count += 1;
            if added {
                d.added += 1;
            } else {
                d.deleted += 1;
            }
        }
    }
    boxes.into_values().filter(|d| (d.max[0] - d.min[0]) > SMALLEST || (d.max[1] - d.min[1]) > SMALLEST).collect()
}

// ------------------------------------------------------------------- driving

fn bounds(shapes: &Shapes) -> ([f32; 2], [f32; 2]) {
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in &shapes.primitives {
        for c in p.points {
            min[0] = min[0].min(c[0]);
            min[1] = min[1].min(c[1]);
            max[0] = max[0].max(c[0]);
            max[1] = max[1].max(c[1]);
        }
    }
    (min, max)
}

/// The transforms worth trying. Two revisions of a sheet nearly always share
/// an origin, so identity comes first and usually wins outright. The rest are
/// the few discrete ways a drawing moves between revisions: turned by a right
/// angle, or plotted to another sheet size -- and that scale is read off how
/// far the linework spreads, not searched for.
fn candidates(a: &Shapes, b: &Shapes, sweep: Option<(f32, f32)>) -> Vec<Sim> {
    let mut out = vec![Sim::ID];
    for turn in [90.0, 180.0, 270.0] {
        out.push(Sim::turn(turn));
    }

    let (amin, amax) = bounds(a);
    let (bmin, bmax) = bounds(b);
    let diag = |min: [f32; 2], max: [f32; 2]| ((max[0] - min[0]).powi(2) + (max[1] - min[1]).powi(2)).sqrt();
    let (da, db) = (diag(amin, amax), diag(bmin, bmax));
    if da > 1.0 && db > 1.0 {
        let ratio = da / db;
        if (ratio - 1.0).abs() > 1e-3 {
            for turn in [0.0, 90.0, 180.0, 270.0] {
                out.push(Sim { scale: ratio, ..Sim::turn(turn) });
            }
        }
    }

    if let Some((max, step)) = sweep {
        let steps = (max / step).round() as i32;
        for n in -steps..=steps {
            let turn = n as f32 * step;
            if turn != 0.0 {
                out.push(Sim::turn(turn));
            }
        }
    }
    out
}

struct Args {
    a: PathBuf,
    b: Option<PathBuf>,
    page_a: u32,
    page_b: u32,
    self_test: bool,
    moved: Sim,
    sweep: Option<(f32, f32)>,
}

fn run() -> Result<(), String> {
    let args = parse_args()?;

    let doc_a = Document::load(&args.a).map_err(|e| format!("{}: {e}", args.a.display()))?;
    let name_b = args.b.clone().unwrap_or_else(|| args.a.clone());
    let doc_b = Document::load(&name_b).map_err(|e| format!("{}: {e}", name_b.display()))?;
    let page_b = if args.self_test { args.page_a } else { args.page_b };

    println!("A  {} page {}", args.a.display(), args.page_a);
    println!("B  {} page {}", name_b.display(), page_b);
    if !args.moved.is_id() {
        println!("   B {} before matching, to be found again", args.moved.describe());
    }
    println!();

    let read = Instant::now();
    let shapes_a = page_shapes(&doc_a, args.page_a, TOLERANCE, MOST_IMAGE_DENSITY)?;
    let a_ms = read.elapsed().as_secs_f64() * 1e3;
    let read = Instant::now();
    let shapes_b = page_shapes(&doc_b, page_b, TOLERANCE, MOST_IMAGE_DENSITY)?;
    let b_ms = read.elapsed().as_secs_f64() * 1e3;

    println!("Reading the pages");
    println!("  A  {:>9} primitives, {:>6} styles, {:>6} image pieces   {a_ms:>8.1} ms", shapes_a.primitives.len(), shapes_a.styles.len(), shapes_a.images);
    println!("  B  {:>9} primitives, {:>6} styles, {:>6} image pieces   {b_ms:>8.1} ms", shapes_b.primitives.len(), shapes_b.styles.len(), shapes_b.images);
    println!();

    let compare = Instant::now();

    let fp = Instant::now();
    let mut fa = fingerprint(&shapes_a, Sim::ID);
    let fp_ms = fp.elapsed().as_secs_f64() * 1e3;
    println!("Fingerprinting A          {fp_ms:>8.1} ms  {} primitives", fa.len());

    // Try each candidate, scored by how many distinctive pairs agree on one
    // offset. That costs a fingerprint and a vote -- no matching -- so a
    // whole sweep stays affordable.
    let search = Instant::now();
    let tries = candidates(&shapes_a, &shapes_b, args.sweep);
    let mut best: Option<(Sim, Vec<Print>, [f32; 2], usize, usize)> = None;
    for cand in &tries {
        let fb = fingerprint(&shapes_b, cand.after(args.moved));
        let pairs = distinctive(&fa, &fb);
        let (offset, votes) = offset_vote(&fa, &fb, &pairs);
        if best.as_ref().is_none_or(|(_, _, _, most, _)| votes > *most) {
            best = Some((*cand, fb, offset, votes, pairs.len()));
        }
    }
    let search_ms = search.elapsed().as_secs_f64() * 1e3;
    let Some((cand, mut fb, offset, votes, seen_once)) = best else {
        return Err("nothing to compare".into());
    };

    println!("Searching {:>3} candidates  {search_ms:>8.1} ms  best: B {}", tries.len(), cand.describe());
    println!("                                     {seen_once} fingerprints seen once each side, {votes} agreed on one offset");
    println!("                                     offset [{:.2}, {:.2}]", offset[0], offset[1]);

    let matching = Instant::now();
    let matched = match_up(&mut fa, &mut fb, offset);
    let match_ms = matching.elapsed().as_secs_f64() * 1e3;

    let unmatched_a: Vec<&Print> = fa.iter().filter(|f| !f.taken).collect();
    let unmatched_b: Vec<&Print> = fb.iter().filter(|f| !f.taken).collect();
    let pct = |n: usize, d: usize| if d == 0 { 100.0 } else { n as f64 * 100.0 / d as f64 };
    println!("Matching                  {match_ms:>8.1} ms  {matched} pairs");
    println!("                                     A {:.2}% matched, {} left", pct(matched, fa.len()), unmatched_a.len());
    println!("                                     B {:.2}% matched, {} left", pct(matched, fb.len()), unmatched_b.len());
    println!();

    let pairs: Vec<([f32; 2], [f32; 2])> = fa.iter().filter_map(|f| f.pair.map(|j| (f.at, fb[j as usize].at))).collect();
    if let Some(t) = similarity(&pairs) {
        println!("Transform the {} matched pairs agree on", pairs.len());
        println!("  rotation {:+.4}deg   scale {:.6}   translation [{:+.3}, {:+.3}]", t.rotation.to_degrees(), t.scale, t.tx, t.ty);
        println!("  residual: mean {:.5} pt, worst {:.5} pt", t.mean, t.worst);
        println!();
    }

    let cluster = Instant::now();
    let mut boxes = cluster_boxes(&unmatched_a, &unmatched_b);
    let cluster_ms = cluster.elapsed().as_secs_f64() * 1e3;
    boxes.sort_by(|x, y| y.count.cmp(&x.count));
    let compare_ms = compare.elapsed().as_secs_f64() * 1e3;

    println!("Clustering                {cluster_ms:>8.1} ms  {} differences", boxes.len());
    for (n, d) in boxes.iter().take(12).enumerate() {
        println!(
            "  {:>2}. {:>7} pieces  {:>6} added {:>6} deleted  at [{:.1}, {:.1}] {:.1} x {:.1} pt",
            n + 1,
            d.count,
            d.added,
            d.deleted,
            d.min[0],
            d.min[1],
            d.max[0] - d.min[0],
            d.max[1] - d.min[1]
        );
    }
    if boxes.len() > 12 {
        println!("  ... and {} more", boxes.len() - 12);
    }
    println!();

    println!("Reading both pages        {:>8.1} ms", a_ms + b_ms);
    println!("Comparing them            {compare_ms:>8.1} ms");
    println!("Altogether                {:>8.1} ms", a_ms + b_ms + compare_ms);

    let images = shapes_a.images + shapes_b.images;
    if images > 0 {
        println!();
        println!("{images} image pieces: the parts a raster pass would still have to look at.");
    }
    Ok(())
}

fn parse_args() -> Result<Args, String> {
    let mut a: Option<PathBuf> = None;
    let mut b: Option<PathBuf> = None;
    let mut args = Args { a: PathBuf::new(), b: None, page_a: 1, page_b: 1, self_test: false, moved: Sim::ID, sweep: None };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = |what: &str| -> Result<f32, String> { it.next().ok_or(format!("{what} wants a number"))?.parse::<f32>().map_err(|e| e.to_string()) };
        match arg.as_str() {
            "--self" => args.self_test = true,
            "--page-a" => args.page_a = next("--page-a")? as u32,
            "--page-b" => args.page_b = next("--page-b")? as u32,
            "--shift" => {
                args.moved.tx = next("--shift")?;
                args.moved.ty = next("--shift")?;
            }
            "--scale" => args.moved.scale = next("--scale")?,
            "--rotate" => args.moved.rot = next("--rotate")?.to_radians(),
            "--sweep" => args.sweep = Some((next("--sweep")?, next("--sweep")?)),
            "-h" | "--help" => {
                println!("compare A.pdf [B.pdf] [--page-a N] [--page-b N] [--self]");
                println!("        [--shift X Y] [--scale S] [--rotate DEG] [--sweep MAX STEP]");
                std::process::exit(0);
            }
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other if a.is_none() => a = Some(PathBuf::from(other)),
            other if b.is_none() => b = Some(PathBuf::from(other)),
            other => return Err(format!("unexpected {other}")),
        }
    }
    args.a = a.ok_or("give a PDF to compare; --help for the rest")?;
    args.b = b;
    // One file, no pages asked for: page 1 against page 2, a cheap way to see
    // what a page full of differences looks like.
    if args.b.is_none() && !args.self_test && args.page_a == 1 && args.page_b == 1 {
        args.page_b = 2;
    }
    Ok(args)
}
