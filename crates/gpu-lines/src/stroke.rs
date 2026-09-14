//! Stroking an outline as PDF says: its subpaths cut into dashes, with caps
//! at their ends and joins where they turn. Lines stay pieces of line for the
//! shader to widen; caps and joins become triangles.

use crate::geometry::{distance, polylines, Piece};

/// How an open subpath's ends are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Cap {
    /// Square, at the end.
    #[default]
    Butt,
    /// A half disc past the end.
    Round,
    /// Square, half the line's width past the end.
    Square,
}

impl Cap {
    /// The cap PDF numbers `number`: 0 butt, 1 round, 2 square.
    pub fn numbered(number: f32) -> Cap {
        match number as i32 {
            1 => Cap::Round,
            2 => Cap::Square,
            _ => Cap::Butt,
        }
    }
}

/// How a subpath is drawn where it turns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Join {
    /// The outer edges carried on until they meet, unless that's further than
    /// the miter limit allows, when it's bevelled.
    #[default]
    Miter,
    Round,
    /// The outer corners joined straight across.
    Bevel,
}

impl Join {
    /// The join PDF numbers `number`: 0 miter, 1 round, 2 bevel.
    pub fn numbered(number: f32) -> Join {
        match number as i32 {
            1 => Join::Round,
            2 => Join::Bevel,
            _ => Join::Miter,
        }
    }
}

/// A dash pattern: lengths on and off in turn, starting `phase` into it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Dash {
    pub lengths: Vec<f32>,
    pub phase: f32,
}

impl Dash {
    /// The pattern, unless it draws a solid line: empty, all zero, or with
    /// lengths that make no sense.
    pub fn new(lengths: Vec<f32>, phase: f32) -> Option<Dash> {
        let usable = lengths.iter().all(|l| l.is_finite() && *l >= 0.0) && lengths.iter().sum::<f32>() > 0.0 && phase.is_finite();
        usable.then_some(Dash { lengths, phase })
    }
}

/// How strokes are drawn, in the units of the space they're drawn in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    /// 0 for a hairline, one pixel wide at any zoom.
    pub width: f32,
    pub cap: Cap,
    pub join: Join,
    /// How far past a line's width a miter join may reach, as a ratio.
    pub miter_limit: f32,
    pub dash: Option<Dash>,
}

impl Default for Style {
    fn default() -> Self {
        Style { width: 1.0, cap: Cap::Butt, join: Join::Miter, miter_limit: 10.0, dash: None }
    }
}

/// A piece of a stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Stroked {
    /// A straight piece of line, the stroke's width. With `round` ends, for a
    /// stroke with round caps and joins, it's drawn reaching half its width
    /// past each end in a half disc -- which makes the caps and joins itself,
    /// so they aren't made as triangles.
    Line { from: [f32; 2], to: [f32; 2], round: bool },
    /// A triangle of a cap or join.
    Triangle([[f32; 2]; 3]),
}

/// Calls `emit` with the pieces stroking `outline` in `style` makes, where
/// the outline is in page space and `scale` is how much the transform to it
/// scales lengths -- line widths and dashes. Curves, and round caps and
/// joins, are within `tolerance`; joins that would fill a gap no wider than
/// that are left out.
pub(crate) fn stroke(outline: &[Piece], style: &Style, scale: f32, tolerance: f32, mut emit: impl FnMut(Stroked)) {
    let round = style.cap == Cap::Round && style.join == Join::Round;
    let stroker = Stroker { style, half: style.width * scale * 0.5, tolerance, round };
    let dashes: Option<Vec<f32>> = style.dash.as_ref().map(|dash| dash.lengths.iter().map(|l| l * scale).collect());
    let mut dash_points = Vec::new();
    polylines(outline, tolerance, |points, closed| match (&dashes, &style.dash) {
        (Some(lengths), Some(dash)) => cut_dashes(points, closed, lengths, dash.phase * scale, &mut dash_points, |dash| stroker.polyline(dash, false, &mut emit)),
        _ => stroker.polyline(points, closed, &mut emit),
    });
}

/// Calls `visit` with each dash of the subpath through `points`: pieces of
/// it `lengths` long in turn, the first, third and so on drawn, starting
/// `phase` into the pattern. An odd number of lengths repeats to make an even
/// one, as PDF says. `piece` is a buffer to reuse.
fn cut_dashes(points: &[[f32; 2]], closed: bool, lengths: &[f32], phase: f32, piece: &mut Vec<[f32; 2]>, mut visit: impl FnMut(&[[f32; 2]])) {
    let count = if lengths.len() % 2 == 1 { lengths.len() * 2 } else { lengths.len() };
    let cycle: f32 = lengths.iter().sum::<f32>() * (count / lengths.len()) as f32;
    let (mut index, mut left) = (0, lengths[0]);
    let mut into = phase.rem_euclid(cycle);
    while into > 0.0 && into >= left {
        into -= left;
        index = (index + 1) % count;
        left = lengths[index % lengths.len()];
    }
    left -= into;
    let add = |piece: &mut Vec<[f32; 2]>, point: [f32; 2]| {
        if piece.last() != Some(&point) {
            piece.push(point);
        }
    };

    piece.clear();
    piece.push(points[0]);
    let closing = closed.then(|| (points[points.len() - 1], points[0]));
    for (a, b) in points.windows(2).map(|w| (w[0], w[1])).chain(closing) {
        let length = distance(a, b);
        let mut done = 0.0;
        while length - done > left {
            done += left;
            let t = done / length;
            let cut = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            if index % 2 == 0 {
                add(piece, cut);
                visit(piece);
            }
            piece.clear();
            piece.push(cut);
            index = (index + 1) % count;
            left = lengths[index % lengths.len()];
        }
        left -= length - done;
        if index % 2 == 0 {
            add(piece, b);
        }
    }
    if index % 2 == 0 {
        visit(piece);
    }
}

/// Strokes subpaths in one style, at one width.
struct Stroker<'s> {
    style: &'s Style,
    /// Half the line's width in page space.
    half: f32,
    tolerance: f32,
    /// Whether lines are drawn with round ends, making the caps and joins.
    round: bool,
}

impl Stroker<'_> {
    /// The pieces of the subpath through `points`, which has no point the
    /// same as the one before it.
    fn polyline(&self, points: &[[f32; 2]], closed: bool, emit: &mut impl FnMut(Stroked)) {
        let (half, cap) = (self.half, self.style.cap);
        let n = points.len();
        if n == 1 {
            // A subpath that goes nowhere shows only as a round cap's dot.
            if self.round {
                emit(Stroked::Line { from: points[0], to: points[0], round: true });
            } else if cap == Cap::Round && half > self.tolerance {
                self.fan(points[0], [half, 0.0], std::f32::consts::TAU, emit);
            }
            return;
        }

        let segments = if closed { n } else { n - 1 };
        for i in 0..segments {
            let (mut a, mut b) = (points[i], points[(i + 1) % n]);
            if !closed && cap == Cap::Square {
                let along = unit(a, b);
                if i == 0 {
                    a = offset(a, along, -half);
                }
                if i == segments - 1 {
                    b = offset(b, along, half);
                }
            }
            emit(Stroked::Line { from: a, to: b, round: self.round });
        }
        if half == 0.0 || self.round {
            return;
        }

        let turns = if closed { 0..n } else { 1..n - 1 };
        for i in turns {
            let (before, at, after) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
            self.join(at, unit(before, at), unit(at, after), emit);
        }
        if !closed && cap == Cap::Round && half > self.tolerance {
            for (end, inner) in [(points[0], points[1]), (points[n - 1], points[n - 2])] {
                let [ox, oy] = unit(inner, end);
                // From the left of the way out, round through it.
                self.fan(end, [-oy * half, ox * half], -std::f32::consts::PI, emit);
            }
        }
    }

    /// The join at `at`, where the stroke turns from heading `from` to
    /// heading `to`.
    fn join(&self, at: [f32; 2], from: [f32; 2], to: [f32; 2], emit: &mut impl FnMut(Stroked)) {
        let half = self.half;
        let cross = from[0] * to[1] - from[1] * to[0];
        let dot = from[0] * to[0] + from[1] * to[1];
        if dot > 0.0 && half * cross.abs() <= self.tolerance {
            return;
        }
        // The outside of the turn: right of a left turn, otherwise left.
        let side = if cross > 0.0 { -1.0 } else { 1.0 };
        let outward = |[x, y]: [f32; 2]| [-y * side, x * side];
        let (n0, n1) = (outward(from), outward(to));
        let (a, b) = (offset(at, n0, half), offset(at, n1, half));
        match self.style.join {
            Join::Round => self.fan(at, [n0[0] * half, n0[1] * half], -side * dot.clamp(-1.0, 1.0).acos(), emit),
            join => {
                // The miter's length over the line's width.
                let ratio = 1.0 / ((1.0 + dot) * 0.5).max(0.0).sqrt();
                let bisector = [n0[0] + n1[0], n0[1] + n1[1]];
                let length = (bisector[0] * bisector[0] + bisector[1] * bisector[1]).sqrt();
                if join == Join::Miter && ratio <= self.style.miter_limit && length > f32::EPSILON {
                    let tip = offset(at, [bisector[0] / length, bisector[1] / length], half * ratio);
                    emit(Stroked::Triangle([at, a, tip]));
                    emit(Stroked::Triangle([at, tip, b]));
                } else {
                    emit(Stroked::Triangle([at, a, b]));
                }
            }
        }
    }

    /// Triangles fanning from `centre` around an arc of `angle` radians
    /// (anticlockwise if positive), starting at `centre + from`.
    fn fan(&self, centre: [f32; 2], from: [f32; 2], angle: f32, emit: &mut impl FnMut(Stroked)) {
        let radius = (from[0] * from[0] + from[1] * from[1]).sqrt();
        let step = 2.0 * (1.0 - (self.tolerance / radius).min(1.0)).acos();
        let steps = ((angle.abs() / step.max(1e-3)).ceil() as usize).clamp(1, 64);
        let at = |i: usize| {
            let (sin, cos) = (angle * i as f32 / steps as f32).sin_cos();
            [centre[0] + from[0] * cos - from[1] * sin, centre[1] + from[0] * sin + from[1] * cos]
        };
        for i in 0..steps {
            emit(Stroked::Triangle([centre, at(i), at(i + 1)]));
        }
    }
}

/// The unit vector heading from `a` to `b`.
fn unit(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let length = distance(a, b).max(f32::EPSILON);
    [(b[0] - a[0]) / length, (b[1] - a[1]) / length]
}

/// `point` moved `by` along unit vector `direction`.
fn offset(point: [f32; 2], direction: [f32; 2], by: f32) -> [f32; 2] {
    [point[0] + direction[0] * by, point[1] + direction[1] * by]
}

#[cfg(test)]
mod tests {
    use super::*;
    use Piece::{Close, Line, Move};

    fn stroked(outline: &[Piece], style: &Style, scale: f32) -> Vec<Stroked> {
        let mut pieces = Vec::new();
        stroke(outline, style, scale, 0.05, |piece| pieces.push(piece));
        pieces
    }

    fn lines(pieces: &[Stroked]) -> Vec<([f32; 2], [f32; 2])> {
        pieces.iter().filter_map(|p| if let Stroked::Line { from, to, .. } = p { Some((*from, *to)) } else { None }).collect()
    }

    fn triangles(pieces: &[Stroked]) -> Vec<[[f32; 2]; 3]> {
        pieces.iter().filter_map(|p| if let Stroked::Triangle(t) = p { Some(*t) } else { None }).collect()
    }

    fn area(triangles: &[[[f32; 2]; 3]]) -> f32 {
        triangles.iter().map(|[a, b, c]| ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0).sum()
    }

    fn dashed(lengths: &[f32], phase: f32) -> Style {
        Style { width: 0.0, dash: Dash::new(lengths.to_vec(), phase), ..Style::default() }
    }

    const TEN: [Piece; 2] = [Move([0.0, 0.0]), Line([10.0, 0.0])];

    fn along(xs: &[(f32, f32)]) -> Vec<([f32; 2], [f32; 2])> {
        xs.iter().map(|&(a, b)| ([a, 0.0], [b, 0.0])).collect()
    }

    #[test]
    fn dashes_cut_lines_from_where_their_phase_says() {
        assert_eq!(lines(&stroked(&TEN, &dashed(&[2.0, 1.0], 0.0), 1.0)), along(&[(0.0, 2.0), (3.0, 5.0), (6.0, 8.0), (9.0, 10.0)]));
        assert_eq!(lines(&stroked(&TEN, &dashed(&[2.0, 1.0], 1.0), 1.0)), along(&[(0.0, 1.0), (2.0, 4.0), (5.0, 7.0), (8.0, 10.0)]));
        assert_eq!(lines(&stroked(&TEN, &dashed(&[2.0], 0.0), 1.0)), along(&[(0.0, 2.0), (4.0, 6.0), (8.0, 10.0)]), "one length is on and off alike");
        assert_eq!(lines(&stroked(&TEN, &dashed(&[1.0, 0.5], 0.0), 2.0)), along(&[(0.0, 2.0), (3.0, 5.0), (6.0, 8.0), (9.0, 10.0)]), "dashes scale with the transform");
        assert_eq!(Dash::new(vec![0.0, 0.0], 0.0), None, "all zero is solid");
    }

    #[test]
    fn a_dash_carries_on_around_corners() {
        let corner = [Move([0.0, 0.0]), Line([2.0, 0.0]), Line([2.0, 2.0])];
        let pieces = stroked(&corner, &dashed(&[3.0, 1.0], 0.0), 1.0);
        assert_eq!(lines(&pieces), [([0.0, 0.0], [2.0, 0.0]), ([2.0, 0.0], [2.0, 1.0])], "one dash round the corner, then a gap");
    }

    #[test]
    fn a_miter_join_reaches_the_corner_of_the_outer_edges() {
        let corner = [Move([0.0, 0.0]), Line([10.0, 0.0]), Line([10.0, 10.0])];
        let style = Style { width: 2.0, ..Style::default() };
        let joins = triangles(&stroked(&corner, &style, 1.0));
        assert_eq!(joins.len(), 2);
        assert!(joins.iter().flatten().any(|&[x, y]| (x - 11.0).abs() < 1e-4 && (y + 1.0).abs() < 1e-4), "the tip is at 11, -1: {joins:?}");
        assert!((area(&joins) - 1.0).abs() < 1e-4, "the one square of corner outside both lines");
    }

    #[test]
    fn a_join_sharper_than_the_miter_limit_is_bevelled() {
        let hairpin = [Move([0.0, 0.0]), Line([10.0, 0.0]), Line([0.0, 1.0])];
        let style = Style { width: 2.0, ..Style::default() };
        assert_eq!(triangles(&stroked(&hairpin, &style, 1.0)).len(), 1);
        let round = Style { join: Join::Round, ..style };
        assert!(triangles(&stroked(&hairpin, &round, 1.0)).len() > 4, "rounded in several steps");
    }

    #[test]
    fn caps_reach_past_the_ends() {
        let square = Style { width: 2.0, cap: Cap::Square, ..Style::default() };
        assert_eq!(lines(&stroked(&TEN, &square, 1.0)), along(&[(-1.0, 11.0)]));
        let round = Style { width: 2.0, cap: Cap::Round, ..Style::default() };
        let pieces = stroked(&TEN, &round, 1.0);
        assert_eq!(lines(&pieces), along(&[(0.0, 10.0)]));
        let caps = triangles(&pieces);
        assert!((area(&caps) - std::f32::consts::PI).abs() < 0.25, "two half discs of radius 1, within the tolerance: {}", area(&caps));
        assert!(caps.iter().flatten().all(|&[x, _]| x <= 0.0 + 1e-4 || x >= 10.0 - 1e-4), "outside the line");
    }

    #[test]
    fn round_caps_and_joins_are_left_to_round_ended_lines() {
        let style = Style { width: 2.0, cap: Cap::Round, join: Join::Round, ..Style::default() };
        let corner = [Move([0.0, 0.0]), Line([10.0, 0.0]), Line([10.0, 10.0])];
        let pieces = stroked(&corner, &style, 1.0);
        assert_eq!(pieces, [([0.0, 0.0], [10.0, 0.0]), ([10.0, 0.0], [10.0, 10.0])].map(|(from, to)| Stroked::Line { from, to, round: true }));
        let dot = stroked(&[Move([5.0, 5.0]), Close], &style, 1.0);
        assert_eq!(dot, [Stroked::Line { from: [5.0, 5.0], to: [5.0, 5.0], round: true }], "a dot is a round line going nowhere");
        assert!(matches!(stroked(&TEN, &Style::default(), 1.0)[..], [Stroked::Line { round: false, .. }]));
    }

    #[test]
    fn a_point_is_a_dot_only_with_round_caps() {
        let dot = [Move([5.0, 5.0]), Close];
        let round = Style { width: 2.0, cap: Cap::Round, ..Style::default() };
        assert!((area(&triangles(&stroked(&dot, &round, 1.0))) - std::f32::consts::PI).abs() < 0.25);
        assert!(stroked(&dot, &Style { width: 2.0, ..Style::default() }, 1.0).is_empty());
    }

    #[test]
    fn hairlines_and_gentle_turns_have_no_joins() {
        let corner = [Move([0.0, 0.0]), Line([10.0, 0.0]), Line([10.0, 10.0]), Close];
        let hairline = Style { width: 0.0, cap: Cap::Round, ..Style::default() };
        assert!(triangles(&stroked(&corner, &hairline, 1.0)).is_empty());
        let gentle = [Move([0.0, 0.0]), Line([10.0, 0.0]), Line([20.0, 0.01])];
        assert!(triangles(&stroked(&gentle, &Style::default(), 1.0)).is_empty());
    }
}
