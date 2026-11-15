//! Grid geometry: 10px grid units, points, rects, the eight octilinear
//! directions, segments, segment and polyline crossing tests.
//!
//! `geom` works in grid units. Sub-grid pixel rules (the CORE-10 8px
//! label gap, CORE-30 text widths) are measured in px by `scene` and
//! `label`, not here; label rectangles are rounded outward to the grid
//! when geom tests (overlap, clearance) are applied to them.
//!
//! Depends on: nothing.

// DECISION: every coordinate is an `i32` in grid units (1 unit = 10px);
// y grows downwards as in SVG, so `Dir::N` is -y. All tests are exact
// integer arithmetic, no floats.

use std::cmp::Ordering;

/// Pixels per grid unit (CORE-6: every vertex sits on the 10px grid).
pub const GRID_PX: i32 = 10;

/// Shortest allowed segment, in grid steps: 2 steps = 20px (CORE-6).
/// Compare with [`Segment::len_steps`].
pub const MIN_SEGMENT_STEPS: i32 = 2;

/// Clearance between a line and any box other than at its own ports, in
/// grid units: 1 unit = 10px (CORE-7). A line keeps it when it does not
/// [`Segment::passes_through`] the box [`Rect::inflate`]d by this much.
pub const CLEARANCE_UNITS: i32 = 1;

/// Grid units to pixels; always exact (CORE-6).
#[must_use]
pub const fn grid_to_px(units: i32) -> i32 {
    units * GRID_PX
}

/// Pixels to grid units, exact only: `None` when `px` is not a multiple
/// of [`GRID_PX`] (CORE-6; the CORE-18 / L5 off-grid check of an `at`
/// centre given in px, DSL-7).
#[must_use]
pub const fn px_to_grid(px: i32) -> Option<i32> {
    if px % GRID_PX == 0 {
        Some(px / GRID_PX)
    } else {
        None
    }
}

/// Pixels to the nearest grid unit; a tie (`px % 10 == 5`) rounds
/// towards +infinity, so negative and positive values behave the same
/// under translation (CORE-6, CORE-22, EDIT-2 snapping).
#[must_use]
pub const fn snap_px_to_grid(px: i32) -> i32 {
    // DECISION: floor((px + 5) / 10) in i64, so ties go to +infinity and
    // values near i32::MAX do not overflow.
    let snapped = (px as i64 + (GRID_PX / 2) as i64).div_euclid(GRID_PX as i64);
    // The quotient of an i32-range value by 10 always fits in i32.
    #[allow(clippy::cast_possible_truncation)]
    let units = snapped as i32;
    units
}

// Private const helpers (`Ord::min` / `max` are not const).
const fn min_i32(a: i32, b: i32) -> i32 {
    if a < b { a } else { b }
}

const fn max_i32(a: i32, b: i32) -> i32 {
    if a > b { a } else { b }
}

/// Midpoint of `lo..=hi`, rounded towards -infinity, without overflow.
const fn mid_floor(lo: i32, hi: i32) -> i32 {
    let mid = (lo as i64 + hi as i64).div_euclid(2);
    // The midpoint of two i32 values is an i32.
    #[allow(clippy::cast_possible_truncation)]
    let mid = mid as i32;
    mid
}

/// One axis of [`Rect::inflate`]: grow both ends by `by`, collapsing to
/// the centre line when a shrink would cross the ends over.
// DECISION: saturating arithmetic so a huge `by` clamps instead of
// panicking; the collapse point is the original centre line.
const fn inflate_axis(lo: i32, hi: i32, by: i32) -> (i32, i32) {
    let new_lo = lo.saturating_sub(by);
    let new_hi = hi.saturating_add(by);
    if new_lo > new_hi {
        let mid = mid_floor(lo, hi);
        (mid, mid)
    } else {
        (new_lo, new_hi)
    }
}

/// A point on the grid, in grid units (CORE-6). Ordered by `x`, then
/// `y`, which is the router's tie-break order (CORE-25). A box position
/// (`at`, DSL-7) is the box centre as a `Point`.
// DECISION: hot (stored per router cell and per segment end): repr(C), 8 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Point {
    /// Horizontal grid coordinate; may be negative.
    pub x: i32,
    /// Vertical grid coordinate, growing downwards; may be negative.
    pub y: i32,
}

impl Point {
    /// The point `(x, y)` in grid units (CORE-6).
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// The neighbouring grid point one step away in `dir` (CORE-6).
    #[must_use]
    pub const fn step(self, dir: Dir) -> Self {
        let (dx, dy) = dir.delta();
        Self::new(self.x + dx, self.y + dy)
    }
}

/// An axis-aligned rectangle in grid units, corners inclusive, with
/// `min.x <= max.x` and `min.y <= max.y` (box bounds for CORE-7, CORE-14,
/// CORE-18, CORE-22). A box is built from its `at` centre and half its
/// size (DSL-7: sizes are multiples of 20px, so half-sizes are whole
/// grid units) with [`Rect::from_centre`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Rect {
    /// Top-left corner.
    pub min: Point,
    /// Bottom-right corner.
    pub max: Point,
}

impl Rect {
    /// The rectangle spanned by two opposite corners, in any order
    /// (corners are normalised so `min <= max` per axis) (CORE-6).
    #[must_use]
    pub const fn from_corners(a: Point, b: Point) -> Self {
        Self {
            min: Point::new(min_i32(a.x, b.x), min_i32(a.y, b.y)),
            max: Point::new(max_i32(a.x, b.x), max_i32(a.y, b.y)),
        }
    }

    /// The rectangle centred on `centre` reaching `half_w` units left and
    /// right and `half_h` units up and down; a negative half-size counts
    /// as its absolute value (DSL-7 box from `at` and `size`, CORE-2
    /// default sizes, CORE-18).
    #[must_use]
    pub const fn from_centre(centre: Point, half_w: i32, half_h: i32) -> Self {
        let hw = half_w.saturating_abs();
        let hh = half_h.saturating_abs();
        Self {
            min: Point::new(centre.x.saturating_sub(hw), centre.y.saturating_sub(hh)),
            max: Point::new(centre.x.saturating_add(hw), centre.y.saturating_add(hh)),
        }
    }

    /// The centre point; exact when width and height are even, otherwise
    /// rounded towards -infinity per axis (DSL-7, CORE-18, CORE-23 rows
    /// and columns).
    #[must_use]
    pub const fn centre(&self) -> Point {
        Point::new(
            mid_floor(self.min.x, self.max.x),
            mid_floor(self.min.y, self.max.y),
        )
    }

    /// Width in grid units (`max.x - min.x`) (CORE-2, CORE-18).
    #[must_use]
    pub const fn width(&self) -> i32 {
        self.max.x - self.min.x
    }

    /// Height in grid units (`max.y - min.y`) (CORE-2, CORE-18).
    #[must_use]
    pub const fn height(&self) -> i32 {
        self.max.y - self.min.y
    }

    /// True when `p` lies inside or on the border (CORE-7, CORE-9 ports).
    #[must_use]
    pub const fn contains(&self, p: Point) -> bool {
        self.min.x <= p.x && p.x <= self.max.x && self.min.y <= p.y && p.y <= self.max.y
    }

    /// True when the interiors share positive area; rects that only
    /// share an edge or a corner do not overlap (CORE-14, CORE-18 / L6,
    /// CORE-22, L26).
    #[must_use]
    pub const fn overlaps(&self, other: &Rect) -> bool {
        max_i32(self.min.x, other.min.x) < min_i32(self.max.x, other.max.x)
            && max_i32(self.min.y, other.min.y) < min_i32(self.max.y, other.max.y)
    }

    /// Grown by `by` units on every side; a negative `by` shrinks, and
    /// clamps at a zero-size rect around the centre line (clearance for
    /// CORE-7 with [`CLEARANCE_UNITS`], spacing margins for CORE-14,
    /// CORE-18 / L6, CORE-22, the CORE-1 boundary margin).
    #[must_use]
    pub const fn inflate(&self, by: i32) -> Self {
        let (min_x, max_x) = inflate_axis(self.min.x, self.max.x, by);
        let (min_y, max_y) = inflate_axis(self.min.y, self.max.y, by);
        Self {
            min: Point::new(min_x, min_y),
            max: Point::new(max_x, max_y),
        }
    }
}

/// One of the eight octilinear directions, counter-clockwise from east,
/// with y growing downwards (CORE-6).
// DECISION: repr(u8) with fixed discriminants 0..=7 so the router can
// store one per grid cell and index `Dir::ALL` by discriminant.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dir {
    /// +x.
    E = 0,
    /// +x, -y.
    NE = 1,
    /// -y.
    N = 2,
    /// -x, -y.
    NW = 3,
    /// -x.
    W = 4,
    /// -x, +y.
    SW = 5,
    /// +y.
    S = 6,
    /// +x, +y.
    SE = 7,
}

impl Dir {
    /// All eight directions in discriminant order; the fixed order the
    /// router expands neighbours in (CORE-6, CORE-25).
    pub const ALL: [Dir; 8] = [
        Dir::E,
        Dir::NE,
        Dir::N,
        Dir::NW,
        Dir::W,
        Dir::SW,
        Dir::S,
        Dir::SE,
    ];

    /// One-step offset `(dx, dy)`, each in `-1..=1` (CORE-6).
    #[must_use]
    pub const fn delta(self) -> (i32, i32) {
        match self {
            Dir::E => (1, 0),
            Dir::NE => (1, -1),
            Dir::N => (0, -1),
            Dir::NW => (-1, -1),
            Dir::W => (-1, 0),
            Dir::SW => (-1, 1),
            Dir::S => (0, 1),
            Dir::SE => (1, 1),
        }
    }

    /// True for the four 45° directions (CORE-6; diagonal step cost).
    #[must_use]
    pub const fn is_diagonal(self) -> bool {
        (self as u8) % 2 == 1
    }

    /// The direction turned by 180° (CORE-6, CORE-9 square-to-side ports).
    #[must_use]
    pub const fn opposite(self) -> Dir {
        Dir::ALL[((self as u8 + 4) % 8) as usize]
    }

    /// The turn from `self` to `to` in 45° steps, `0..=4`, regardless of
    /// side (CORE-6: every bend is exactly 1 step; CORE-8: a join is at
    /// most 2 steps from the trunk's direction; router BEND cost).
    #[must_use]
    pub const fn turn_steps(self, to: Dir) -> u8 {
        let diff = (to as u8 + 8 - self as u8) % 8;
        if diff > 4 { 8 - diff } else { diff }
    }
}

/// A straight segment between two grid points, `a` to `b` (CORE-6).
// DECISION: hot (one per polyline segment, many per view): repr(C), 16 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Segment {
    /// Start point.
    pub a: Point,
    /// End point.
    pub b: Point,
}

/// How two segments meet (CORE-8, CORE-17 / L4). Exactly one applies.
/// Counting crossings between whole lines uses [`crossings`], not a sum
/// of segment contacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Contact {
    /// No common point.
    Disjoint,
    /// Common points exist but only where an endpoint of one lies on the
    /// other (a join dot, a T, or a shared bend vertex); not a proper
    /// crossing of the two segments.
    Touch,
    /// The interiors meet in exactly one point and the segments are not
    /// collinear: a crossing (CORE-17).
    Cross,
    /// Collinear and sharing a stretch of positive length: shared
    /// segment, not a crossing (CORE-8).
    Overlap,
}

impl Segment {
    /// The segment from `a` to `b` (CORE-6).
    #[must_use]
    pub const fn new(a: Point, b: Point) -> Self {
        Self { a, b }
    }

    /// Direction from `a` to `b`, or `None` when the segment has zero
    /// length or is not at 0°, 45° or 90° (octilinear iff `dx == 0`,
    /// `dy == 0` or `|dx| == |dy|`) (CORE-6, L2). The two `None` cases
    /// are told apart by [`Segment::len_steps`]: `Some(0)` for zero
    /// length, `None` for non-octilinear.
    #[must_use]
    pub const fn dir(&self) -> Option<Dir> {
        let dx = self.b.x - self.a.x;
        let dy = self.b.y - self.a.y;
        if !(dx == 0 || dy == 0 || dx.abs() == dy.abs()) {
            return None;
        }
        match (dx.signum(), dy.signum()) {
            (1, 0) => Some(Dir::E),
            (1, -1) => Some(Dir::NE),
            (0, -1) => Some(Dir::N),
            (-1, -1) => Some(Dir::NW),
            (-1, 0) => Some(Dir::W),
            (-1, 1) => Some(Dir::SW),
            (0, 1) => Some(Dir::S),
            (1, 1) => Some(Dir::SE),
            _ => None,
        }
    }

    /// Length in grid steps (`max(|dx|, |dy|)`) of an octilinear segment,
    /// `Some(0)` for zero length, `None` when not octilinear (CORE-6:
    /// at least [`MIN_SEGMENT_STEPS`]).
    #[must_use]
    pub const fn len_steps(&self) -> Option<i32> {
        let dx = (self.b.x - self.a.x).abs();
        let dy = (self.b.y - self.a.y).abs();
        if dx == 0 || dy == 0 || dx == dy {
            Some(max_i32(dx, dy))
        } else {
            None
        }
    }

    /// Exact squared length in px² (`(dx² + dy²) * GRID_PX²`) of an
    /// octilinear segment, `Some(0)` for zero length, `None` when not
    /// octilinear. A diagonal step is 10√2 px, so px rules compare
    /// squares: the CORE-8 90px run, CORE-10 chevrons at 100px and 320px,
    /// the CORE-14 30px and 50px spacings.
    #[must_use]
    pub const fn len_px_squared(&self) -> Option<i64> {
        if self.len_steps().is_none() {
            return None;
        }
        let dx = self.b.x as i64 - self.a.x as i64;
        let dy = self.b.y as i64 - self.a.y as i64;
        let px = GRID_PX as i64;
        Some((dx * dx + dy * dy) * px * px)
    }

    /// How `self` and `other` meet, by exact integer orientation tests
    /// (CORE-8, CORE-17).
    #[must_use]
    pub fn contact(&self, other: &Segment) -> Contact {
        let (s, t) = (self, other);
        if s.a == s.b {
            return if on_segment(s.a, t) {
                Contact::Touch
            } else {
                Contact::Disjoint
            };
        }
        if t.a == t.b {
            return if on_segment(t.a, s) {
                Contact::Touch
            } else {
                Contact::Disjoint
            };
        }
        let o1 = orient(s.a, s.b, t.a);
        let o2 = orient(s.a, s.b, t.b);
        if o1 == 0 && o2 == 0 {
            // Collinear: collinear points sort along the line in `Point` order.
            let lo = s.a.min(s.b).max(t.a.min(t.b));
            let hi = s.a.max(s.b).min(t.a.max(t.b));
            return match lo.cmp(&hi) {
                Ordering::Less => Contact::Overlap,
                Ordering::Equal => Contact::Touch,
                Ordering::Greater => Contact::Disjoint,
            };
        }
        let o3 = orient(t.a, t.b, s.a);
        let o4 = orient(t.a, t.b, s.b);
        if o1 * o2 < 0 && o3 * o4 < 0 {
            return Contact::Cross;
        }
        if (o1 == 0 && on_segment(t.a, s))
            || (o2 == 0 && on_segment(t.b, s))
            || (o3 == 0 && on_segment(s.a, t))
            || (o4 == 0 && on_segment(s.b, t))
        {
            Contact::Touch
        } else {
            Contact::Disjoint
        }
    }

    /// True when the segment has a point strictly inside `rect`; running
    /// along the border or touching it at an endpoint (a port) does not
    /// count (CORE-7 / L3; with `rect.inflate(CLEARANCE_UNITS)`, the
    /// CORE-7 10px clearance).
    #[must_use]
    pub fn passes_through(&self, rect: &Rect) -> bool {
        // DECISION: separating-axis test against the open rectangle: the
        // closed segment misses the open interior iff the x axis, the y axis or
        // the segment's own line weakly separates them. Exact in i64.
        if rect.width() == 0 || rect.height() == 0 {
            // A flat rect has no inside.
            return false;
        }
        let (a, b) = (self.a, self.b);
        let x_overlap = a.x.max(b.x) > rect.min.x && a.x.min(b.x) < rect.max.x;
        let y_overlap = a.y.max(b.y) > rect.min.y && a.y.min(b.y) < rect.max.y;
        if !(x_overlap && y_overlap) {
            return false;
        }
        if a == b {
            // The two overlaps above already say the point is strictly inside.
            return true;
        }
        let corners = [
            rect.min,
            Point::new(rect.max.x, rect.min.y),
            rect.max,
            Point::new(rect.min.x, rect.max.y),
        ];
        let sides = corners.map(|c| orient(a, b, c));
        sides.iter().any(|&o| o > 0) && sides.iter().any(|&o| o < 0)
    }
}

/// Number of places where two polylines (vertex lists, consecutive
/// vertices joined by segments) meet: each connected piece of their
/// common points counts once, whether a straight-through crossing, a
/// meeting at a bend vertex of either line (counted once, not once per
/// segment pair and not zero), a touch, or a shared stretch (CORE-17 /
/// L4, CORE-8). A meeting at a line's end is counted too: L4's
/// exception for a joined line ending on its trunk needs destinations,
/// which only the caller knows, so it subtracts that meeting itself.
/// A polyline of fewer than two vertices meets nothing.
// DECISION: one count per connected meeting, not per segment pair, so
// the count matches the prototype's crossing count (CORE-11 parity).
#[must_use]
pub fn crossings(a: &[Point], b: &[Point]) -> u32 {
    // DECISION: collect every common piece of each segment pair (a point
    // or a shared stretch), join pieces that touch with a union-find, and
    // count the groups. Exact: cross points are kept as rationals in i128.
    let mut pieces: Vec<Piece> = Vec::new();
    for sa in a.windows(2).map(|w| Segment::new(w[0], w[1])) {
        for sb in b.windows(2).map(|w| Segment::new(w[0], w[1])) {
            if let Some(piece) = common_piece(&sa, &sb) {
                pieces.push(piece);
            }
        }
    }
    let mut parent: Vec<usize> = (0..pieces.len()).collect();
    for i in 0..pieces.len() {
        for j in (i + 1)..pieces.len() {
            if pieces[i].meets(&pieces[j]) {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[rj] = ri;
                }
            }
        }
    }
    let groups = (0..pieces.len())
        .filter(|&i| find(&mut parent, i) == i)
        .count();
    u32::try_from(groups).unwrap_or(u32::MAX)
}

/// Sign of the turn `p -> q -> r`: positive, negative or zero when
/// collinear. Exact in i64.
fn orient(p: Point, q: Point, r: Point) -> i64 {
    let (ux, uy) = (
        i64::from(q.x) - i64::from(p.x),
        i64::from(q.y) - i64::from(p.y),
    );
    let (vx, vy) = (
        i64::from(r.x) - i64::from(p.x),
        i64::from(r.y) - i64::from(p.y),
    );
    (ux * vy - uy * vx).signum()
}

/// True when `p` lies on the closed segment `s`.
fn on_segment(p: Point, s: &Segment) -> bool {
    orient(s.a, s.b, p) == 0 && Rect::from_corners(s.a, s.b).contains(p)
}

/// A point with rational coordinates `(x / d, y / d)`, `d > 0`: the
/// exact spot where two segments cross.
#[derive(Debug, Clone, Copy)]
struct RatPoint {
    x: i128,
    y: i128,
    d: i128,
}

impl RatPoint {
    fn int(p: Point) -> Self {
        Self {
            x: i128::from(p.x),
            y: i128::from(p.y),
            d: 1,
        }
    }

    fn same(&self, o: &RatPoint) -> bool {
        self.x * o.d == o.x * self.d && self.y * o.d == o.y * self.d
    }

    /// True when this point lies on the closed integer segment `s`.
    fn on(&self, s: &Segment) -> bool {
        let (ax, ay) = (i128::from(s.a.x), i128::from(s.a.y));
        let (ux, uy) = (i128::from(s.b.x) - ax, i128::from(s.b.y) - ay);
        let (vx, vy) = (self.x - ax * self.d, self.y - ay * self.d);
        let r = Rect::from_corners(s.a, s.b);
        ux * vy - uy * vx == 0
            && i128::from(r.min.x) * self.d <= self.x
            && self.x <= i128::from(r.max.x) * self.d
            && i128::from(r.min.y) * self.d <= self.y
            && self.y <= i128::from(r.max.y) * self.d
    }
}

/// One connected piece of the common points of two segments.
#[derive(Debug, Clone, Copy)]
enum Piece {
    Point(RatPoint),
    Stretch(Segment),
}

impl Piece {
    fn meets(&self, other: &Piece) -> bool {
        match (self, other) {
            (Piece::Point(p), Piece::Point(q)) => p.same(q),
            (Piece::Point(p), Piece::Stretch(s)) | (Piece::Stretch(s), Piece::Point(p)) => p.on(s),
            (Piece::Stretch(s), Piece::Stretch(t)) => s.contact(t) != Contact::Disjoint,
        }
    }
}

/// The common points of `s` and `t` as one piece, if any.
fn common_piece(s: &Segment, t: &Segment) -> Option<Piece> {
    match s.contact(t) {
        Contact::Disjoint => None,
        Contact::Touch => [(s.a, t), (s.b, t), (t.a, s), (t.b, s)]
            .into_iter()
            .find(|(p, seg)| on_segment(*p, seg))
            .map(|(p, _)| Piece::Point(RatPoint::int(p))),
        Contact::Overlap => {
            let lo = s.a.min(s.b).max(t.a.min(t.b));
            let hi = s.a.max(s.b).min(t.a.max(t.b));
            Some(Piece::Stretch(Segment::new(lo, hi)))
        }
        Contact::Cross => {
            // s.a + r * (num / den), r = s.b - s.a, q = t.b - t.a.
            let (ax, ay) = (i128::from(s.a.x), i128::from(s.a.y));
            let (rx, ry) = (i128::from(s.b.x) - ax, i128::from(s.b.y) - ay);
            let (qx, qy) = (
                i128::from(t.b.x) - i128::from(t.a.x),
                i128::from(t.b.y) - i128::from(t.a.y),
            );
            let (wx, wy) = (i128::from(t.a.x) - ax, i128::from(t.a.y) - ay);
            let mut den = rx * qy - ry * qx;
            let mut num = wx * qy - wy * qx;
            if den < 0 {
                den = -den;
                num = -num;
            }
            Some(Piece::Point(RatPoint {
                x: ax * den + rx * num,
                y: ay * den + ry * num,
                d: den,
            }))
        }
    }
}

/// Union-find root with path halving.
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    // CORE-6: segments at 0°, 45° or 90°, vertices on the 10px grid,
    // bends of 45°, segments at least 20px (len_steps >= MIN_SEGMENT_STEPS).
    #[test]
    fn core_6_octilinear_grid_bends_and_min_length() {
        // A route as the router would emit it: grid vertices, octilinear
        // segments, 45° bends, every segment at least 20px.
        let route = [
            Point::new(0, 0),
            Point::new(4, 0),
            Point::new(7, 3),
            Point::new(7, 6),
            Point::new(5, 8),
        ];
        let segs: Vec<Segment> = route.windows(2).map(|w| Segment::new(w[0], w[1])).collect();
        for s in &segs {
            assert!(s.dir().is_some(), "{s:?} is octilinear");
            assert!(
                s.len_steps().unwrap() >= MIN_SEGMENT_STEPS,
                "{s:?} is at least 20px"
            );
        }
        for w in segs.windows(2) {
            assert_eq!(
                w[0].dir().unwrap().turn_steps(w[1].dir().unwrap()),
                1,
                "45° bend"
            );
        }
        // Vertices given in px are on the grid only at multiples of 10px.
        assert_eq!(px_to_grid(70), Some(7));
        assert_eq!(px_to_grid(75), None);
        // Violations are visible: a 1-step (10px) segment is too short, a
        // 90° bend is 2 steps, a 30° segment has no direction (L2).
        let short = Segment::new(Point::new(0, 0), Point::new(1, 0));
        assert!(short.len_steps().unwrap() < MIN_SEGMENT_STEPS);
        assert_eq!(Dir::E.turn_steps(Dir::S), 2);
        assert_eq!(Segment::new(Point::new(0, 0), Point::new(3, 1)).dir(), None);
        // 20px horizontal is exactly the minimum; 2 diagonal steps are 2*10√2 px.
        let min = Segment::new(Point::new(0, 0), Point::new(2, 0));
        assert_eq!(min.len_steps(), Some(MIN_SEGMENT_STEPS));
        assert_eq!(min.len_px_squared(), Some(400));
        let diag = Segment::new(Point::new(0, 0), Point::new(2, 2));
        assert_eq!(diag.len_steps(), Some(2));
        assert_eq!(diag.len_px_squared(), Some(800));
    }

    // CORE-7: L3 through-box test and the 10px clearance via inflate.
    #[test]
    fn core_7_through_box_and_clearance() {
        // A 40x40px box (4x4 units) that is not the line's own end.
        let other = Rect::from_centre(Point::new(2, 2), 2, 2);
        let clear = other.inflate(CLEARANCE_UNITS);
        // L3: straight through the box.
        let through = Segment::new(Point::new(-3, 2), Point::new(7, 2));
        assert!(through.passes_through(&other));
        // Running along the edge is not through the box, but breaks the 10px
        // clearance.
        let on_edge = Segment::new(Point::new(-3, 0), Point::new(7, 0));
        assert!(!on_edge.passes_through(&other));
        assert!(on_edge.passes_through(&clear));
        // Exactly 10px away keeps the clearance.
        let ten_px = Segment::new(Point::new(-3, -1), Point::new(7, -1));
        assert!(!ten_px.passes_through(&other));
        assert!(!ten_px.passes_through(&clear));
        // A line ending at its own port on the border is not through the box.
        let port = Segment::new(Point::new(2, -5), Point::new(2, 0));
        assert!(!port.passes_through(&other));
        // A diagonal cutting a corner goes through; one grazing it does not.
        let cut = Segment::new(Point::new(2, -1), Point::new(5, 2));
        assert!(cut.passes_through(&other));
        let graze = Segment::new(Point::new(3, -1), Point::new(5, 1));
        assert!(!graze.passes_through(&other));
        assert!(graze.passes_through(&clear));
    }

    // CORE-8: a shared stretch is sharing; a join is a touch, not a cross.
    #[test]
    fn core_8_sharing_and_join_are_not_crosses() {
        let trunk = Segment::new(Point::new(0, 0), Point::new(10, 0));
        // Shared stretch (bus): sharing, not a crossing.
        let shared = Segment::new(Point::new(4, 0), Point::new(10, 0));
        assert_eq!(trunk.contact(&shared), Contact::Overlap);
        // A join: the joining line ends (dot) on the trunk at 90° or 45°.
        let join_90 = Segment::new(Point::new(5, 6), Point::new(5, 0));
        let join_45 = Segment::new(Point::new(1, 4), Point::new(5, 0));
        assert_eq!(trunk.contact(&join_90), Contact::Touch);
        assert_eq!(join_45.contact(&trunk), Contact::Touch);
        // A crossing goes straight through.
        let cross = Segment::new(Point::new(5, 6), Point::new(5, -6));
        assert_eq!(trunk.contact(&cross), Contact::Cross);
        // Whole lines: one meeting each, so the caller can tell a join (that
        // it then subtracts) from a crossing elsewhere.
        let trunk_line = [Point::new(0, 0), Point::new(10, 0), Point::new(10, 8)];
        let joined = [
            Point::new(2, 8),
            Point::new(2, 4),
            Point::new(6, 0),
            Point::new(10, 0),
        ];
        assert_eq!(
            crossings(&trunk_line, &joined),
            1,
            "join + shared stretch is one meeting"
        );
    }

    // CORE-17: crossings are counted once per meeting between two lines.
    #[test]
    fn core_17_crossings_counted_once_per_meeting() {
        let h = [Point::new(0, 0), Point::new(10, 0)];
        let v = [Point::new(5, -5), Point::new(5, 5)];
        assert_eq!(crossings(&h, &v), 1);
        assert_eq!(crossings(&v, &h), 1);
        // A zig-zag crossing a straight line twice: two meetings.
        let zig = [
            Point::new(2, -2),
            Point::new(2, 2),
            Point::new(6, 2),
            Point::new(6, -2),
        ];
        assert_eq!(crossings(&h, &zig), 2);
        // Crossing at a bend vertex of one line counts once, not twice.
        let bend = [
            Point::new(3, -3),
            Point::new(6, -3),
            Point::new(9, 0),
            Point::new(6, 3),
        ];
        assert_eq!(crossings(&h, &bend), 1);
        // Disjoint lines and degenerate polylines meet nothing.
        let far = [Point::new(0, 4), Point::new(10, 4)];
        assert_eq!(crossings(&h, &far), 0);
        assert_eq!(crossings(&h, &[]), 0);
        assert_eq!(crossings(&[Point::new(5, 0)], &v), 0);
    }

    // geom-01. Also CORE-6 (vertices on the 10px grid).
    #[test]
    fn grid_px_round_trip() {
        for units in [-300, -7, -1, 0, 1, 2, 7, 300] {
            let px = grid_to_px(units);
            assert_eq!(px, units * 10);
            assert_eq!(px_to_grid(px), Some(units));
            assert_eq!(snap_px_to_grid(px), units);
        }
        // Off-grid px is rejected by the exact conversion (CORE-18 / L5).
        for px in [-15, -1, 1, 5, 9, 11, 25] {
            assert_eq!(px_to_grid(px), None, "{px}px is off the grid");
        }
        // Snapping goes to the nearest unit, ties towards +infinity.
        assert_eq!(snap_px_to_grid(4), 0);
        assert_eq!(snap_px_to_grid(5), 1);
        assert_eq!(snap_px_to_grid(14), 1);
        assert_eq!(snap_px_to_grid(-4), 0);
        assert_eq!(snap_px_to_grid(-5), 0);
        assert_eq!(snap_px_to_grid(-6), -1);
        assert_eq!(snap_px_to_grid(-15), -1);
        // Same answer under translation by whole units.
        for px in -40..40 {
            assert_eq!(snap_px_to_grid(px + 100), snap_px_to_grid(px) + 10);
        }
        // Points step one grid unit at a time.
        let p = Point::new(3, -4);
        assert_eq!((p.x, p.y), (3, -4));
        assert_eq!(p.step(Dir::E), Point::new(4, -4));
        assert_eq!(p.step(Dir::N), Point::new(3, -5));
        assert_eq!(p.step(Dir::SW), Point::new(2, -3));
        // Ordered by x, then y (CORE-25 tie-break).
        assert!(Point::new(0, 9) < Point::new(1, -9));
        assert!(Point::new(1, -9) < Point::new(1, 0));
    }

    #[test]
    fn rect_contains_and_overlap() {
        let r = Rect::from_corners(Point::new(4, 6), Point::new(0, 2));
        assert_eq!(r.min, Point::new(0, 2));
        assert_eq!(r.max, Point::new(4, 6));
        assert_eq!((r.width(), r.height()), (4, 4));
        // Mixed corners normalise per axis.
        let m = Rect::from_corners(Point::new(0, 6), Point::new(4, 2));
        assert_eq!(m, r);
        assert!(r.contains(Point::new(2, 4)));
        assert!(r.contains(Point::new(0, 2)), "corner is on the border");
        assert!(r.contains(Point::new(4, 3)), "edge is on the border");
        assert!(!r.contains(Point::new(5, 3)));
        assert!(!r.contains(Point::new(2, 1)));
        // Overlap needs positive shared area.
        let inside = Rect::from_corners(Point::new(1, 3), Point::new(2, 4));
        assert!(r.overlaps(&inside) && inside.overlaps(&r));
        let partial = Rect::from_corners(Point::new(3, 5), Point::new(8, 9));
        assert!(r.overlaps(&partial) && partial.overlaps(&r));
        let edge = Rect::from_corners(Point::new(4, 2), Point::new(8, 6));
        assert!(!r.overlaps(&edge) && !edge.overlaps(&r), "shared edge");
        let corner = Rect::from_corners(Point::new(4, 6), Point::new(8, 9));
        assert!(!r.overlaps(&corner), "shared corner");
        let apart = Rect::from_corners(Point::new(10, 10), Point::new(12, 12));
        assert!(!r.overlaps(&apart));
        // A zero-size rect has no interior, so overlaps nothing.
        let dot = Rect::from_corners(Point::new(2, 4), Point::new(2, 4));
        assert!(!r.overlaps(&dot) && !dot.overlaps(&r));
        assert!(r.overlaps(&r));
    }

    #[test]
    fn rect_inflate() {
        let r = Rect::from_corners(Point::new(0, 0), Point::new(4, 2));
        assert_eq!(
            r.inflate(CLEARANCE_UNITS),
            Rect::from_corners(Point::new(-1, -1), Point::new(5, 3))
        );
        assert_eq!(r.inflate(0), r);
        assert_eq!(r.inflate(3).inflate(-3), r);
        // Shrinking clamps per axis at a zero-size rect on the centre line.
        assert_eq!(
            r.inflate(-1),
            Rect::from_corners(Point::new(1, 1), Point::new(3, 1))
        );
        assert_eq!(
            r.inflate(-2),
            Rect::from_corners(Point::new(2, 1), Point::new(2, 1))
        );
        assert_eq!(
            r.inflate(-50),
            Rect::from_corners(Point::new(2, 1), Point::new(2, 1))
        );
        // Odd extent: the centre line rounds towards -infinity.
        let odd = Rect::from_corners(Point::new(-3, 0), Point::new(0, 0));
        assert_eq!(
            odd.inflate(-5),
            Rect::from_corners(Point::new(-2, 0), Point::new(-2, 0))
        );
    }

    // DSL-7: `at` is the box centre; sizes are multiples of 20px.
    #[test]
    fn rect_from_centre_and_centre() {
        // A 200x100px box (sizes multiples of 20px) at centre (300px, 120px).
        let at = Point::new(px_to_grid(300).unwrap(), px_to_grid(120).unwrap());
        let b = Rect::from_centre(
            at,
            px_to_grid(200).unwrap() / 2,
            px_to_grid(100).unwrap() / 2,
        );
        assert_eq!(b, Rect::from_corners(Point::new(20, 7), Point::new(40, 17)));
        assert_eq!(b.centre(), at);
        assert_eq!((b.width(), b.height()), (20, 10));
        // Negative half-sizes count as their absolute value.
        assert_eq!(Rect::from_centre(at, -10, -5), b);
        // Left of / above the origin behaves the same.
        let neg = Rect::from_centre(Point::new(-30, -12), 10, 5);
        assert_eq!(neg.min, Point::new(-40, -17));
        assert_eq!(neg.max, Point::new(-20, -7));
        assert_eq!(neg.centre(), Point::new(-30, -12));
        // Odd extent rounds towards -infinity per axis.
        let odd = Rect::from_corners(Point::new(-3, 0), Point::new(0, 3));
        assert_eq!(odd.centre(), Point::new(-2, 1));
    }

    // geom-02.
    #[test]
    fn direction_of_octilinear_segment() {
        let o = Point::new(-2, 3);
        for dir in Dir::ALL {
            let (dx, dy) = dir.delta();
            assert!((-1..=1).contains(&dx) && (-1..=1).contains(&dy));
            assert_ne!((dx, dy), (0, 0));
            for len in [1, 2, 9] {
                let s = Segment::new(o, Point::new(o.x + dx * len, o.y + dy * len));
                assert_eq!(s.dir(), Some(dir), "{s:?}");
                assert_eq!(s.len_steps(), Some(len));
            }
            assert_eq!(dir.is_diagonal(), dx != 0 && dy != 0);
            assert_eq!(dir.opposite().delta(), (-dx, -dy));
            assert_eq!(dir.opposite().opposite(), dir);
            // Discriminant indexes ALL.
            assert_eq!(Dir::ALL[dir as usize], dir);
            assert_eq!(o.step(dir), Point::new(o.x + dx, o.y + dy));
        }
        // y grows downwards: north is -y.
        assert_eq!(
            Segment::new(Point::new(0, 0), Point::new(0, -3)).dir(),
            Some(Dir::N)
        );
        assert_eq!(
            Segment::new(Point::new(0, 0), Point::new(3, 3)).dir(),
            Some(Dir::SE)
        );
        let s = Segment::new(Point::new(1, 2), Point::new(3, 4));
        assert_eq!((s.a, s.b), (Point::new(1, 2), Point::new(3, 4)));
    }

    // Also CORE-6 (every segment at 0°, 45° or 90°).
    #[test]
    fn non_octilinear_segment_has_no_direction() {
        for (dx, dy) in [(1, 2), (2, 1), (3, -1), (-5, 4), (-1, -7)] {
            let s = Segment::new(Point::new(4, 4), Point::new(4 + dx, 4 + dy));
            assert_eq!(s.dir(), None, "{s:?}");
            assert_eq!(s.len_steps(), None, "{s:?}");
            assert_eq!(s.len_px_squared(), None, "{s:?}");
        }
    }

    // Review S13: zero-length vs non-octilinear told apart by len_steps.
    #[test]
    fn zero_length_segment_has_zero_steps() {
        let p = Point::new(-3, 7);
        let s = Segment::new(p, p);
        assert_eq!(s.dir(), None);
        assert_eq!(s.len_steps(), Some(0));
        assert_eq!(s.len_px_squared(), Some(0));
    }

    #[test]
    fn turn_angle_45_steps() {
        assert_eq!(Dir::E.turn_steps(Dir::E), 0);
        assert_eq!(Dir::E.turn_steps(Dir::NE), 1);
        assert_eq!(Dir::E.turn_steps(Dir::SE), 1);
        assert_eq!(Dir::E.turn_steps(Dir::N), 2);
        assert_eq!(Dir::E.turn_steps(Dir::S), 2);
        assert_eq!(Dir::E.turn_steps(Dir::NW), 3);
        assert_eq!(Dir::E.turn_steps(Dir::W), 4);
        // Wrap-around between SE (7) and E (0) / NE (1).
        assert_eq!(Dir::SE.turn_steps(Dir::NE), 2);
        for a in Dir::ALL {
            assert_eq!(a.turn_steps(a.opposite()), 4);
            for b in Dir::ALL {
                let t = a.turn_steps(b);
                assert!(t <= 4);
                assert_eq!(t, b.turn_steps(a), "symmetric");
            }
        }
    }

    // geom-03. Also CORE-17 (crossings are counted).
    #[test]
    fn proper_crossing_detected() {
        let h = Segment::new(Point::new(0, 0), Point::new(4, 0));
        let v = Segment::new(Point::new(2, -2), Point::new(2, 2));
        assert_eq!(h.contact(&v), Contact::Cross);
        assert_eq!(v.contact(&h), Contact::Cross);
        // Two diagonals crossing between grid points.
        let d1 = Segment::new(Point::new(0, 0), Point::new(1, 1));
        let d2 = Segment::new(Point::new(0, 1), Point::new(1, 0));
        assert_eq!(d1.contact(&d2), Contact::Cross);
        assert_eq!(crossings(&[d1.a, d1.b], &[d2.a, d2.b]), 1);
        // Diagonal crossing a horizontal.
        let d3 = Segment::new(Point::new(1, -2), Point::new(4, 1));
        assert_eq!(h.contact(&d3), Contact::Cross);
        // Missing each other.
        let miss = Segment::new(Point::new(5, -2), Point::new(5, 2));
        assert_eq!(h.contact(&miss), Contact::Disjoint);
        assert_eq!(crossings(&[h.a, h.b], &[miss.a, miss.b]), 0);
    }

    // Review Focus: a join dot is not a crossing.
    #[test]
    fn endpoint_touch_is_not_crossing() {
        let h = Segment::new(Point::new(0, 0), Point::new(4, 0));
        // T: one segment ends on the other's interior (a join dot).
        let t = Segment::new(Point::new(2, 3), Point::new(2, 0));
        assert_eq!(h.contact(&t), Contact::Touch);
        assert_eq!(t.contact(&h), Contact::Touch);
        // L: shared endpoint (a shared bend vertex).
        let l = Segment::new(Point::new(4, 0), Point::new(4, 3));
        assert_eq!(h.contact(&l), Contact::Touch);
        // Collinear, end to end.
        let next = Segment::new(Point::new(4, 0), Point::new(8, 0));
        assert_eq!(h.contact(&next), Contact::Touch);
        // A zero-length segment on the other.
        let dot = Segment::new(Point::new(3, 0), Point::new(3, 0));
        assert_eq!(h.contact(&dot), Contact::Touch);
        assert_eq!(dot.contact(&h), Contact::Touch);
        assert_eq!(dot.contact(&dot), Contact::Touch);
        let off = Segment::new(Point::new(3, 1), Point::new(3, 1));
        assert_eq!(h.contact(&off), Contact::Disjoint);
        // Parallel, apart.
        let par = Segment::new(Point::new(0, 1), Point::new(4, 1));
        assert_eq!(h.contact(&par), Contact::Disjoint);
        // A touch is still a meeting for `crossings` (L4 counts touches).
        assert_eq!(crossings(&[h.a, h.b], &[t.a, t.b]), 1);
    }

    // Also CORE-8 (shared segment is sharing, not crossing).
    #[test]
    fn collinear_overlap_is_sharing() {
        let s = Segment::new(Point::new(0, 0), Point::new(4, 0));
        for other in [
            Segment::new(Point::new(2, 0), Point::new(6, 0)),
            Segment::new(Point::new(6, 0), Point::new(2, 0)),
            Segment::new(Point::new(1, 0), Point::new(3, 0)),
            Segment::new(Point::new(-1, 0), Point::new(9, 0)),
            s,
        ] {
            assert_eq!(s.contact(&other), Contact::Overlap, "{other:?}");
            assert_eq!(other.contact(&s), Contact::Overlap, "{other:?}");
        }
        let d = Segment::new(Point::new(0, 0), Point::new(3, 3));
        assert_eq!(
            d.contact(&Segment::new(Point::new(2, 2), Point::new(5, 5))),
            Contact::Overlap
        );
        let vert = Segment::new(Point::new(1, 0), Point::new(1, 5));
        assert_eq!(
            vert.contact(&Segment::new(Point::new(1, 4), Point::new(1, 2))),
            Contact::Overlap
        );
        // Collinear with a gap: no common point.
        assert_eq!(
            s.contact(&Segment::new(Point::new(5, 0), Point::new(8, 0))),
            Contact::Disjoint
        );
        // Two lines sharing a stretch meet once.
        let a = [
            Point::new(0, 3),
            Point::new(0, 0),
            Point::new(6, 0),
            Point::new(6, 3),
        ];
        let b = [
            Point::new(2, -3),
            Point::new(2, 0),
            Point::new(4, 0),
            Point::new(4, -3),
        ];
        assert_eq!(crossings(&a, &b), 1);
    }

    // Also CORE-7 (no line through a box other than its own ends).
    #[test]
    fn segment_passes_through_rect() {
        let r = Rect::from_corners(Point::new(0, 0), Point::new(4, 4));
        let yes = [
            Segment::new(Point::new(-2, 2), Point::new(6, 2)),
            Segment::new(Point::new(2, -3), Point::new(2, 1)),
            Segment::new(Point::new(1, 1), Point::new(3, 3)),
            Segment::new(Point::new(2, -1), Point::new(5, 2)),
            Segment::new(Point::new(-1, -1), Point::new(5, 5)),
            Segment::new(Point::new(2, 2), Point::new(2, 2)),
        ];
        for s in yes {
            assert!(s.passes_through(&r), "{s:?}");
            assert!(Segment::new(s.b, s.a).passes_through(&r), "{s:?} reversed");
        }
        let no = [
            Segment::new(Point::new(-2, 0), Point::new(6, 0)),
            Segment::new(Point::new(4, -2), Point::new(4, 6)),
            Segment::new(Point::new(2, -3), Point::new(2, 0)),
            Segment::new(Point::new(3, -1), Point::new(5, 1)),
            Segment::new(Point::new(-1, 3), Point::new(1, 5)),
            Segment::new(Point::new(6, 0), Point::new(9, 3)),
            Segment::new(Point::new(0, 0), Point::new(0, 0)),
            Segment::new(Point::new(4, 2), Point::new(4, 2)),
        ];
        for s in no {
            assert!(!s.passes_through(&r), "{s:?}");
            assert!(!Segment::new(s.b, s.a).passes_through(&r), "{s:?} reversed");
        }
        // A zero-size rect has no inside.
        let flat = Rect::from_corners(Point::new(0, 2), Point::new(4, 2));
        assert!(!Segment::new(Point::new(2, 0), Point::new(2, 4)).passes_through(&flat));
    }

    // Review B1: a line crossing another at its bend vertex counts once.
    #[test]
    fn crossing_at_bend_vertex_counts() {
        let trunk = [Point::new(0, 2), Point::new(8, 2)];
        // A line bending exactly on the trunk and going on to the other
        // side: one crossing.
        let through = [Point::new(2, 0), Point::new(4, 2), Point::new(6, 4)];
        assert_eq!(crossings(&trunk, &through), 1);
        let square = [Point::new(4, 0), Point::new(4, 2), Point::new(6, 4)];
        assert_eq!(crossings(&trunk, &square), 1);
        assert_eq!(crossings(&square, &trunk), 1);
        // Bending on the trunk and coming back to the same side: still one
        // meeting.
        let bounce = [Point::new(2, 0), Point::new(4, 2), Point::new(6, 0)];
        assert_eq!(crossings(&trunk, &bounce), 1);
        // Both lines bend at the same vertex.
        let left = [Point::new(0, 0), Point::new(2, 2), Point::new(0, 4)];
        let right = [Point::new(4, 0), Point::new(2, 2), Point::new(4, 4)];
        assert_eq!(crossings(&left, &right), 1);
        // A straight line through the bend vertex of a V-shaped line.
        let vee = [Point::new(0, 0), Point::new(2, 2), Point::new(4, 0)];
        let up = [Point::new(2, -1), Point::new(2, 4)];
        assert_eq!(crossings(&vee, &up), 1);
    }

    // Review Focus: boxes left of / above the origin.
    #[test]
    fn negative_coordinates_behave_like_positive() {
        let shift = |p: Point| Point::new(p.x - 100, p.y - 57);
        let seg = |s: Segment| Segment::new(shift(s.a), shift(s.b));
        let rect = |r: Rect| Rect::from_corners(shift(r.min), shift(r.max));
        let r = Rect::from_corners(Point::new(0, 0), Point::new(4, 6));
        let q = Rect::from_corners(Point::new(3, 5), Point::new(9, 9));
        assert_eq!(rect(r).overlaps(&rect(q)), r.overlaps(&q));
        assert_eq!(rect(r).centre(), shift(r.centre()));
        assert_eq!(rect(r).inflate(1), rect(r.inflate(1)));
        assert_eq!(rect(r).inflate(-9), rect(r.inflate(-9)));
        assert!(rect(r).contains(shift(Point::new(4, 6))));
        let odd = Rect::from_corners(Point::new(1, 1), Point::new(4, 4));
        assert_eq!(rect(odd).centre(), shift(odd.centre()));
        let segs = [
            Segment::new(Point::new(0, 0), Point::new(4, 0)),
            Segment::new(Point::new(2, -2), Point::new(2, 2)),
            Segment::new(Point::new(2, 3), Point::new(2, 0)),
            Segment::new(Point::new(2, 0), Point::new(6, 0)),
            Segment::new(Point::new(0, 1), Point::new(1, 0)),
            Segment::new(Point::new(-1, 3), Point::new(5, -3)),
        ];
        for s in segs {
            assert_eq!(seg(s).dir(), s.dir());
            assert_eq!(seg(s).len_steps(), s.len_steps());
            assert_eq!(
                seg(s).passes_through(&rect(r)),
                s.passes_through(&r),
                "{s:?}"
            );
            for t in segs {
                assert_eq!(seg(s).contact(&seg(t)), s.contact(&t), "{s:?} {t:?}");
            }
        }
        let a = [Point::new(0, 2), Point::new(8, 2)];
        let b = [
            Point::new(2, 0),
            Point::new(4, 2),
            Point::new(6, 0),
            Point::new(6, 4),
        ];
        let a2: Vec<Point> = a.iter().copied().map(shift).collect();
        let b2: Vec<Point> = b.iter().copied().map(shift).collect();
        assert_eq!(crossings(&a2, &b2), crossings(&a, &b));
        assert_eq!(crossings(&a, &b), 2);
        // Snapping is translation-invariant across zero.
        for px in -30..30 {
            assert_eq!(snap_px_to_grid(px - 1000), snap_px_to_grid(px) - 100);
        }
    }

    // Brute force on the half-grid lattice (coordinates doubled): every
    // common point of two octilinear grid segments, and every point
    // where one enters a grid rect's interior, lies on it.
    fn half_points(s: Segment) -> Vec<(i32, i32)> {
        let (a, b) = ((s.a.x * 2, s.a.y * 2), (s.b.x * 2, s.b.y * 2));
        let n = (b.0 - a.0).abs().max((b.1 - a.1).abs());
        if n == 0 {
            return vec![a];
        }
        let (dx, dy) = ((b.0 - a.0) / n, (b.1 - a.1) / n);
        (0..=n).map(|i| (a.0 + dx * i, a.1 + dy * i)).collect()
    }

    fn all_octilinear(lo: i32, hi: i32) -> Vec<Segment> {
        let pts: Vec<Point> = (lo..=hi)
            .flat_map(|x| (lo..=hi).map(move |y| Point::new(x, y)))
            .collect();
        let mut out = Vec::new();
        for &a in &pts {
            for &b in &pts {
                let s = Segment::new(a, b);
                if s.len_steps().is_some() {
                    out.push(s);
                }
            }
        }
        out
    }

    #[test]
    fn contact_matches_brute_force() {
        let segs = all_octilinear(-1, 2);
        for &s in &segs {
            let ps = half_points(s);
            let ends_s = [(s.a.x * 2, s.a.y * 2), (s.b.x * 2, s.b.y * 2)];
            for &t in &segs {
                let ends_t = [(t.a.x * 2, t.a.y * 2), (t.b.x * 2, t.b.y * 2)];
                let common: Vec<_> = half_points(t)
                    .into_iter()
                    .filter(|p| ps.contains(p))
                    .collect();
                let want = match common.as_slice() {
                    [] => Contact::Disjoint,
                    [p] if ends_s.contains(p) || ends_t.contains(p) => Contact::Touch,
                    [_] => Contact::Cross,
                    _ => Contact::Overlap,
                };
                assert_eq!(s.contact(&t), want, "{s:?} {t:?}");
            }
        }
    }

    #[test]
    fn passes_through_matches_brute_force() {
        let segs = all_octilinear(-1, 4);
        let rects = [
            Rect::from_corners(Point::new(0, 0), Point::new(3, 3)),
            Rect::from_corners(Point::new(1, 0), Point::new(2, 3)),
            Rect::from_corners(Point::new(0, 1), Point::new(1, 2)),
            Rect::from_corners(Point::new(1, 1), Point::new(1, 3)),
        ];
        for r in rects {
            for &s in &segs {
                let want = half_points(s).iter().any(|&(x, y)| {
                    r.min.x * 2 < x && x < r.max.x * 2 && r.min.y * 2 < y && y < r.max.y * 2
                });
                assert_eq!(s.passes_through(&r), want, "{s:?} {r:?}");
            }
        }
    }

    #[test]
    fn hot_struct_sizes() {
        assert_eq!(std::mem::size_of::<Point>(), 8);
        assert_eq!(std::mem::size_of::<Segment>(), 16);
        assert_eq!(std::mem::size_of::<Dir>(), 1);
    }
}
