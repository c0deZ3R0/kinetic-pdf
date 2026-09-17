//! Snapping a point to what's already drawn: the ends and corners of the
//! drawing's own lines, where two of them cross, their middles, and the
//! nearest point along one.
//!
//! A measurement is only as good as where its points land, so snapping has to
//! be quick enough to run every frame on a sheet of a million lines, and
//! predictable enough to trust. The index is a flat list of segments with a
//! grid over it: a query looks only in the cells within reach of the pointer,
//! which is a few dozen segments however dense the page is. Crossings are
//! worked out among those few, rather than the millions on the page.
//!
//! Nothing here knows where the segments came from. The app fills it from the
//! lines it already reads to draw a page, in that page's own coordinates, and
//! converts the pointer into them.

use crate::geom::{self, Pt, Rect};

/// Segments to a grid cell, on average: enough that a query reads a handful
/// of cells, few enough that each holds little.
const PER_CELL: f64 = 4.0;

/// Candidate segments a query will look at, nearest cells first. A drawing
/// with thousands of lines under the pointer -- hatching, say -- would
/// otherwise cost more than the snap is worth.
const MOST_CANDIDATES: usize = 96;

/// What a snapped point landed on, in the order they're preferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SnapKind {
    /// A vertex of a markup already made.
    MarkupVertex,
    /// The end of a line, which is also a corner where lines meet.
    Endpoint,
    /// Where two lines cross.
    Intersection,
    /// The middle of a line.
    Midpoint,
    /// The nearest point along a line.
    OnLine,
}

impl SnapKind {
    /// What to call it in the interface.
    pub fn label(self) -> &'static str {
        match self {
            SnapKind::MarkupVertex => "markup point",
            SnapKind::Endpoint => "end",
            SnapKind::Intersection => "crossing",
            SnapKind::Midpoint => "middle",
            SnapKind::OnLine => "on the line",
        }
    }
}

/// Where a point snapped to, and what it caught.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub point: Pt,
    pub kind: SnapKind,
}

/// A page's lines, with a grid over them.
///
/// Segments are held as 32-bit coordinates: a dense sheet has hundreds of
/// thousands of them, and a thousandth of a point is far finer than anything
/// anyone snaps to.
#[derive(Debug, Default)]
pub struct SnapIndex {
    /// Each segment as x1, y1, x2, y2.
    segments: Vec<[f32; 4]>,
    /// Where the grid starts, and how big a cell is.
    origin: Pt,
    cell: f64,
    columns: usize,
    rows: usize,
    /// Segments in each cell: cell `i` holds `indices[starts[i]..starts[i+1]]`.
    starts: Vec<u32>,
    indices: Vec<u32>,
}

impl SnapIndex {
    /// Indexes `segments`, dropping those going nowhere.
    pub fn build(segments: impl IntoIterator<Item = [Pt; 2]>) -> SnapIndex {
        let kept: Vec<[f32; 4]> = segments
            .into_iter()
            .filter(|[a, b]| a.is_finite() && b.is_finite() && a != b)
            .map(|[a, b]| [a.x as f32, a.y as f32, b.x as f32, b.y as f32])
            .collect();
        let Some(bounds) = bounds_of(&kept) else { return SnapIndex::default() };
        let area = (bounds.width() * bounds.height()).max(1.0);
        let cell = (area * PER_CELL / kept.len() as f64).sqrt().max(1e-3);
        let columns = ((bounds.width() / cell).ceil() as usize + 1).max(1);
        let rows = ((bounds.height() / cell).ceil() as usize + 1).max(1);

        // Counted first, then filled: one pass each, no per-cell allocation.
        let mut counts = vec![0_u32; columns * rows + 1];
        let mut index = SnapIndex { segments: kept, origin: bounds.min, cell, columns, rows, starts: Vec::new(), indices: Vec::new() };
        for segment in &index.segments {
            index.walk(segment, |cell| counts[cell + 1] += 1);
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let mut indices = vec![0_u32; counts[counts.len() - 1] as usize];
        let mut at = counts.clone();
        for (i, segment) in index.segments.iter().enumerate() {
            index.walk(segment, |cell| {
                indices[at[cell] as usize] = i as u32;
                at[cell] += 1;
            });
        }
        index.starts = counts;
        index.indices = indices;
        index
    }

    /// Calls `each` for every cell a segment passes through, walking it cell
    /// by cell so a long diagonal doesn't fill the box around it.
    fn walk(&self, [x1, y1, x2, y2]: &[f32; 4], mut each: impl FnMut(usize)) {
        let (a, b) = (Pt::new(f64::from(*x1), f64::from(*y1)), Pt::new(f64::from(*x2), f64::from(*y2)));
        let (mut cx, mut cy) = self.cell_of(a);
        let (tx, ty) = self.cell_of(b);
        each(cy * self.columns + cx);
        // Step towards the far end, one cell at a time, taking whichever of
        // the two crossings comes first.
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let (step_x, step_y) = (dx.signum() as isize, dy.signum() as isize);
        let next_edge = |c: usize, step: isize, from: f64, origin: f64| -> f64 {
            if step == 0 {
                return f64::INFINITY;
            }
            let edge = origin + (c as f64 + if step > 0 { 1.0 } else { 0.0 }) * self.cell;
            (edge - from).abs()
        };
        let (mut run_x, mut run_y) = (next_edge(cx, step_x, a.x, self.origin.x), next_edge(cy, step_y, a.y, self.origin.y));
        let (span_x, span_y) = (dx.abs(), dy.abs());
        let (rate_x, rate_y) = (self.cell / span_x.max(f64::MIN_POSITIVE), self.cell / span_y.max(f64::MIN_POSITIVE));
        let (mut along_x, mut along_y) = (run_x / span_x.max(f64::MIN_POSITIVE), run_y / span_y.max(f64::MIN_POSITIVE));
        let mut guard = self.columns + self.rows + 2;
        while (cx, cy) != (tx, ty) && guard > 0 {
            guard -= 1;
            if along_x <= along_y {
                cx = cx.saturating_add_signed(step_x).min(self.columns - 1);
                along_x += rate_x;
            } else {
                cy = cy.saturating_add_signed(step_y).min(self.rows - 1);
                along_y += rate_y;
            }
            each(cy * self.columns + cx);
        }
        let _ = (&mut run_x, &mut run_y);
    }

    fn cell_of(&self, p: Pt) -> (usize, usize) {
        let x = ((p.x - self.origin.x) / self.cell).floor().max(0.0) as usize;
        let y = ((p.y - self.origin.y) / self.cell).floor().max(0.0) as usize;
        (x.min(self.columns - 1), y.min(self.rows - 1))
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Roughly what it holds in memory.
    pub fn bytes(&self) -> usize {
        self.segments.len() * size_of::<[f32; 4]>() + (self.starts.len() + self.indices.len()) * size_of::<u32>()
    }

    /// The segments whose cells are within `radius` of `p`, nearest first, up
    /// to `MOST_CANDIDATES`.
    pub fn near(&self, p: Pt, radius: f64) -> Vec<[Pt; 2]> {
        if self.segments.is_empty() {
            return Vec::new();
        }
        let reach = Rect::from_corners(p, p).expand(radius);
        let (from_x, from_y) = self.cell_of(reach.min);
        let (to_x, to_y) = self.cell_of(reach.max);
        let mut found: Vec<(f64, [Pt; 2])> = Vec::new();
        let mut seen: Vec<u32> = Vec::new();
        for row in from_y..=to_y {
            for column in from_x..=to_x {
                let cell = row * self.columns + column;
                let (start, end) = (self.starts[cell] as usize, self.starts[cell + 1] as usize);
                for &i in &self.indices[start..end] {
                    if seen.contains(&i) {
                        continue;
                    }
                    seen.push(i);
                    let [x1, y1, x2, y2] = self.segments[i as usize];
                    let segment = [Pt::new(f64::from(x1), f64::from(y1)), Pt::new(f64::from(x2), f64::from(y2))];
                    let distance = geom::distance_to_segment(p, segment[0], segment[1]);
                    if distance <= radius {
                        found.push((distance, segment));
                    }
                }
            }
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.truncate(MOST_CANDIDATES);
        found.into_iter().map(|(_, s)| s).collect()
    }
}

fn bounds_of(segments: &[[f32; 4]]) -> Option<Rect> {
    let points = segments.iter().flat_map(|[x1, y1, x2, y2]| {
        [Pt::new(f64::from(*x1), f64::from(*y1)), Pt::new(f64::from(*x2), f64::from(*y2))]
    });
    let mut bounds: Option<Rect> = None;
    for p in points {
        let around = Rect::from_corners(p, p);
        bounds = Some(bounds.map_or(around, |b: Rect| b.union(around)));
    }
    bounds
}

/// The best point to snap `p` to, within `radius`, or `None` for none.
///
/// `vertices` are points of markups already made, which win over anything in
/// the drawing: an estimator continuing from where they left off means that
/// corner exactly. After them come the ends of lines, then crossings, then
/// middles, then the nearest point along a line. Within a kind, the nearest
/// wins.
pub fn snap(p: Pt, radius: f64, vertices: impl IntoIterator<Item = Pt>, index: Option<&SnapIndex>) -> Option<Snap> {
    let mut best: Option<(SnapKind, f64, Pt)> = None;
    let mut offer = |kind: SnapKind, point: Pt| {
        let distance = p.dist(point);
        if distance > radius {
            return;
        }
        let better = match best {
            None => true,
            Some((best_kind, best_distance, _)) => (kind, distance) < (best_kind, best_distance),
        };
        if better {
            best = Some((kind, distance, point));
        }
    };
    for vertex in vertices {
        offer(SnapKind::MarkupVertex, vertex);
    }
    if let Some(index) = index {
        let candidates = index.near(p, radius);
        for [a, b] in &candidates {
            offer(SnapKind::Endpoint, *a);
            offer(SnapKind::Endpoint, *b);
            offer(SnapKind::Midpoint, a.midpoint(*b));
            offer(SnapKind::OnLine, geom::nearest_on_segment(p, *a, *b).0);
        }
        // Crossings, among the few lines near the pointer only.
        for (i, [a, b]) in candidates.iter().enumerate() {
            for [c, d] in candidates.iter().skip(i + 1) {
                if let Some(at) = crossing(*a, *b, *c, *d) {
                    offer(SnapKind::Intersection, at);
                }
            }
        }
    }
    best.map(|(kind, _, point)| Snap { point, kind })
}

/// Where two segments cross, if they do and aren't in line with each other.
fn crossing(a: Pt, b: Pt, c: Pt, d: Pt) -> Option<Pt> {
    let (r, s) = (b - a, d - c);
    let denominator = r.cross(s);
    if denominator == 0.0 {
        return None;
    }
    let t = (c - a).cross(s) / denominator;
    let u = (c - a).cross(r) / denominator;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then(|| a + r * t)
}

/// `p` pulled onto the horizontal, vertical or 45° line through `from`,
/// whichever it is nearest: what Shift does while drawing.
pub fn straighten(from: Pt, p: Pt) -> Pt {
    let d = p - from;
    let (dx, dy) = (d.x, d.y);
    let (adx, ady) = (dx.abs(), dy.abs());
    // Nearer the diagonal than either axis: keep the longer reach.
    if (adx - ady).abs() < adx.max(ady) * 0.5 {
        let reach = (adx + ady) / 2.0;
        Pt::new(from.x + reach * dx.signum(), from.y + reach * dy.signum())
    } else if adx >= ady {
        Pt::new(p.x, from.y)
    } else {
        Pt::new(from.x, p.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(x1: f64, y1: f64, x2: f64, y2: f64) -> [Pt; 2] {
        [Pt::new(x1, y1), Pt::new(x2, y2)]
    }

    /// A cross and a square: plenty of ends, middles and one crossing.
    fn drawing() -> SnapIndex {
        SnapIndex::build([
            seg(0.0, 0.0, 100.0, 0.0),
            seg(100.0, 0.0, 100.0, 100.0),
            seg(100.0, 100.0, 0.0, 100.0),
            seg(0.0, 100.0, 0.0, 0.0),
            seg(0.0, 0.0, 100.0, 100.0),
            seg(0.0, 100.0, 100.0, 0.0),
        ])
    }

    #[test]
    fn an_end_wins_over_the_line_it_is_on() {
        let index = drawing();
        let snapped = snap(Pt::new(98.0, 2.0), 5.0, [], Some(&index)).unwrap();
        assert_eq!(snapped, Snap { point: Pt::new(100.0, 0.0), kind: SnapKind::Endpoint });
    }

    #[test]
    fn a_markup_vertex_wins_over_the_drawing() {
        let index = drawing();
        let snapped = snap(Pt::new(99.0, 1.0), 5.0, [Pt::new(98.0, 3.0)], Some(&index)).unwrap();
        assert_eq!(snapped.kind, SnapKind::MarkupVertex);
        assert_eq!(snapped.point, Pt::new(98.0, 3.0));
    }

    #[test]
    fn lines_crossing_snap_to_the_crossing() {
        let index = drawing();
        let snapped = snap(Pt::new(51.0, 49.0), 4.0, [], Some(&index)).unwrap();
        assert_eq!(snapped.kind, SnapKind::Intersection);
        assert!(snapped.point.dist(Pt::new(50.0, 50.0)) < 1e-9);
    }

    #[test]
    fn a_middle_and_a_point_along_a_line() {
        let index = SnapIndex::build([seg(0.0, 0.0, 100.0, 0.0)]);
        let middle = snap(Pt::new(50.0, 2.0), 4.0, [], Some(&index)).unwrap();
        assert_eq!((middle.kind, middle.point), (SnapKind::Midpoint, Pt::new(50.0, 0.0)));
        let along = snap(Pt::new(30.0, 2.0), 4.0, [], Some(&index)).unwrap();
        assert_eq!((along.kind, along.point), (SnapKind::OnLine, Pt::new(30.0, 0.0)));
        assert!(snap(Pt::new(30.0, 40.0), 4.0, [], Some(&index)).is_none(), "nothing within reach");
    }

    #[test]
    fn nothing_to_snap_to_without_an_index() {
        assert!(snap(Pt::new(1.0, 1.0), 5.0, [], None).is_none());
        assert!(SnapIndex::build([]).is_empty());
        assert!(snap(Pt::new(1.0, 1.0), 5.0, [], Some(&SnapIndex::default())).is_none());
    }

    #[test]
    fn a_long_line_is_found_anywhere_along_it() {
        // One long diagonal among many short lines, so the grid is fine and
        // the diagonal crosses a great many cells.
        let mut segments: Vec<[Pt; 2]> = (0..2000).map(|i| seg(f64::from(i) * 0.5, 0.0, f64::from(i) * 0.5, 2.0)).collect();
        segments.push(seg(0.0, 0.0, 1000.0, 1000.0));
        let index = SnapIndex::build(segments);
        for at in [100.0, 500.0, 900.0] {
            let snapped = snap(Pt::new(at + 1.0, at - 1.0), 3.0, [], Some(&index));
            assert!(snapped.is_some(), "nothing found at {at} along the diagonal");
        }
    }

    #[test]
    fn straightening_holds_to_an_axis_or_the_diagonal() {
        let from = Pt::new(10.0, 10.0);
        assert_eq!(straighten(from, Pt::new(60.0, 12.0)), Pt::new(60.0, 10.0));
        assert_eq!(straighten(from, Pt::new(12.0, 60.0)), Pt::new(10.0, 60.0));
        let diagonal = straighten(from, Pt::new(60.0, 55.0));
        assert!((diagonal.x - diagonal.y).abs() < 1e-9 && diagonal.x > 50.0, "{diagonal:?}");
    }
}

/// What snapping costs on a dense drawing sheet:
/// `cargo test --release -p markup-model snap::timing -- --ignored --nocapture`.
#[cfg(test)]
mod timing {
    use std::time::Instant;

    use super::*;

    /// A sheet of about 400,000 short lines, as a dense drawing has, spread
    /// over an A1 sheet in points.
    fn sheet() -> Vec<[Pt; 2]> {
        let (w, h) = (2384.0, 1684.0);
        (0..400_000)
            .map(|i| {
                let x = f64::from(i % 1000) * (w / 1000.0);
                let y = f64::from(i / 1000) * (h / 400.0);
                [Pt::new(x, y), Pt::new(x + 2.0, y + 1.5)]
            })
            .collect()
    }

    #[test]
    #[ignore]
    fn timing() {
        let segments = sheet();
        let started = Instant::now();
        let index = SnapIndex::build(segments);
        let build = started.elapsed();

        let started = Instant::now();
        let mut found = 0;
        let queries = 1000;
        for i in 0..queries {
            let p = Pt::new(f64::from(i) * 2.3 % 2384.0, f64::from(i) * 1.7 % 1684.0);
            found += usize::from(snap(p, 6.0, [], Some(&index)).is_some());
        }
        let per_query = started.elapsed() / queries;
        println!(
            "{} segments: build {build:?}, {} MB; {per_query:?} a snap ({found}/{queries} found)",
            index.len(),
            index.bytes() >> 20
        );
    }
}
