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
    todo!()
}

/// Pixels to grid units, exact only: `None` when `px` is not a multiple
/// of [`GRID_PX`] (CORE-6; the CORE-18 / L5 off-grid check of an `at`
/// centre given in px, DSL-7).
#[must_use]
pub const fn px_to_grid(px: i32) -> Option<i32> {
    todo!()
}

/// Pixels to the nearest grid unit; a tie (`px % 10 == 5`) rounds
/// towards +infinity, so negative and positive values behave the same
/// under translation (CORE-6, CORE-22, EDIT-2 snapping).
#[must_use]
pub const fn snap_px_to_grid(px: i32) -> i32 {
    todo!()
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
        todo!()
    }

    /// The neighbouring grid point one step away in `dir` (CORE-6).
    #[must_use]
    pub const fn step(self, dir: Dir) -> Self {
        todo!()
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
        todo!()
    }

    /// The rectangle centred on `centre` reaching `half_w` units left and
    /// right and `half_h` units up and down; a negative half-size counts
    /// as its absolute value (DSL-7 box from `at` and `size`, CORE-2
    /// default sizes, CORE-18).
    #[must_use]
    pub const fn from_centre(centre: Point, half_w: i32, half_h: i32) -> Self {
        todo!()
    }

    /// The centre point; exact when width and height are even, otherwise
    /// rounded towards -infinity per axis (DSL-7, CORE-18, CORE-23 rows
    /// and columns).
    #[must_use]
    pub const fn centre(&self) -> Point {
        todo!()
    }

    /// Width in grid units (`max.x - min.x`) (CORE-2, CORE-18).
    #[must_use]
    pub const fn width(&self) -> i32 {
        todo!()
    }

    /// Height in grid units (`max.y - min.y`) (CORE-2, CORE-18).
    #[must_use]
    pub const fn height(&self) -> i32 {
        todo!()
    }

    /// True when `p` lies inside or on the border (CORE-7, CORE-9 ports).
    #[must_use]
    pub const fn contains(&self, p: Point) -> bool {
        todo!()
    }

    /// True when the interiors share positive area; rects that only
    /// share an edge or a corner do not overlap (CORE-14, CORE-18 / L6,
    /// CORE-22, L26).
    #[must_use]
    pub const fn overlaps(&self, other: &Rect) -> bool {
        todo!()
    }

    /// Grown by `by` units on every side; a negative `by` shrinks, and
    /// clamps at a zero-size rect around the centre line (clearance for
    /// CORE-7 with [`CLEARANCE_UNITS`], spacing margins for CORE-14,
    /// CORE-18 / L6, CORE-22, the CORE-1 boundary margin).
    #[must_use]
    pub const fn inflate(&self, by: i32) -> Self {
        todo!()
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
        todo!()
    }

    /// True for the four 45° directions (CORE-6; diagonal step cost).
    #[must_use]
    pub const fn is_diagonal(self) -> bool {
        todo!()
    }

    /// The direction turned by 180° (CORE-6, CORE-9 square-to-side ports).
    #[must_use]
    pub const fn opposite(self) -> Dir {
        todo!()
    }

    /// The turn from `self` to `to` in 45° steps, `0..=4`, regardless of
    /// side (CORE-6: every bend is exactly 1 step; CORE-8: a join is at
    /// most 2 steps from the trunk's direction; router BEND cost).
    #[must_use]
    pub const fn turn_steps(self, to: Dir) -> u8 {
        todo!()
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
        todo!()
    }

    /// Direction from `a` to `b`, or `None` when the segment has zero
    /// length or is not at 0°, 45° or 90° (octilinear iff `dx == 0`,
    /// `dy == 0` or `|dx| == |dy|`) (CORE-6, L2). The two `None` cases
    /// are told apart by [`Segment::len_steps`]: `Some(0)` for zero
    /// length, `None` for non-octilinear.
    #[must_use]
    pub const fn dir(&self) -> Option<Dir> {
        todo!()
    }

    /// Length in grid steps (`max(|dx|, |dy|)`) of an octilinear segment,
    /// `Some(0)` for zero length, `None` when not octilinear (CORE-6:
    /// at least [`MIN_SEGMENT_STEPS`]).
    #[must_use]
    pub const fn len_steps(&self) -> Option<i32> {
        todo!()
    }

    /// Exact squared length in px² (`(dx² + dy²) * GRID_PX²`) of an
    /// octilinear segment, `Some(0)` for zero length, `None` when not
    /// octilinear. A diagonal step is 10√2 px, so px rules compare
    /// squares: the CORE-8 90px run, CORE-10 chevrons at 100px and 320px,
    /// the CORE-14 30px and 50px spacings.
    #[must_use]
    pub const fn len_px_squared(&self) -> Option<i64> {
        todo!()
    }

    /// How `self` and `other` meet, by exact integer orientation tests
    /// (CORE-8, CORE-17).
    #[must_use]
    pub fn contact(&self, other: &Segment) -> Contact {
        todo!()
    }

    /// True when the segment has a point strictly inside `rect`; running
    /// along the border or touching it at an endpoint (a port) does not
    /// count (CORE-7 / L3; with `rect.inflate(CLEARANCE_UNITS)`, the
    /// CORE-7 10px clearance).
    #[must_use]
    pub fn passes_through(&self, rect: &Rect) -> bool {
        todo!()
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
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // CORE-6: segments at 0°, 45° or 90°, vertices on the 10px grid,
    // bends of 45°, segments at least 20px (len_steps >= MIN_SEGMENT_STEPS).
    #[test]
    fn core_6_octilinear_grid_bends_and_min_length() {
        todo!()
    }

    // CORE-7: L3 through-box test and the 10px clearance via inflate.
    #[test]
    fn core_7_through_box_and_clearance() {
        todo!()
    }

    // CORE-8: a shared stretch is sharing; a join is a touch, not a cross.
    #[test]
    fn core_8_sharing_and_join_are_not_crosses() {
        todo!()
    }

    // CORE-17: crossings are counted once per meeting between two lines.
    #[test]
    fn core_17_crossings_counted_once_per_meeting() {
        todo!()
    }

    // geom-01. Also CORE-6 (vertices on the 10px grid).
    #[test]
    fn grid_px_round_trip() {
        todo!()
    }

    #[test]
    fn rect_contains_and_overlap() {
        todo!()
    }

    #[test]
    fn rect_inflate() {
        todo!()
    }

    // DSL-7: `at` is the box centre; sizes are multiples of 20px.
    #[test]
    fn rect_from_centre_and_centre() {
        todo!()
    }

    // geom-02.
    #[test]
    fn direction_of_octilinear_segment() {
        todo!()
    }

    // Also CORE-6 (every segment at 0°, 45° or 90°).
    #[test]
    fn non_octilinear_segment_has_no_direction() {
        todo!()
    }

    // Review S13: zero-length vs non-octilinear told apart by len_steps.
    #[test]
    fn zero_length_segment_has_zero_steps() {
        todo!()
    }

    #[test]
    fn turn_angle_45_steps() {
        todo!()
    }

    // geom-03. Also CORE-17 (crossings are counted).
    #[test]
    fn proper_crossing_detected() {
        todo!()
    }

    // Review Focus: a join dot is not a crossing.
    #[test]
    fn endpoint_touch_is_not_crossing() {
        todo!()
    }

    // Also CORE-8 (shared segment is sharing, not crossing).
    #[test]
    fn collinear_overlap_is_sharing() {
        todo!()
    }

    // Also CORE-7 (no line through a box other than its own ends).
    #[test]
    fn segment_passes_through_rect() {
        todo!()
    }

    // Review B1: a line crossing another at its bend vertex counts once.
    #[test]
    fn crossing_at_bend_vertex_counts() {
        todo!()
    }

    // Review Focus: boxes left of / above the origin.
    #[test]
    fn negative_coordinates_behave_like_positive() {
        todo!()
    }

    #[test]
    fn hot_struct_sizes() {
        assert_eq!(std::mem::size_of::<Point>(), 8);
        assert_eq!(std::mem::size_of::<Segment>(), 16);
        assert_eq!(std::mem::size_of::<Dir>(), 1);
    }
}
