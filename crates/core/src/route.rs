//! Octilinear grid router: 0/45/90 degree lines on the 10px grid, one
//! port per line, bus lanes and joins, fewest crossings, rip-up and
//! retry, deterministic output; can re-route only what a box move broke
//! (CORE-6..13, 25).
//!
//! Depends on: `view`, `geom`.

// Shape of the API: `route` takes `(&ViewPlan, &[Point], &RouteConfig,
// &AtomicBool)` (the plan, the centres `place` returned, indexed by
// `BoxId`, and a stop flag) and returns `Result<Routes, Stopped>`;
// `Routes` is plain data indexed by `EdgeId`, which `label`,
// `scene` and `pipeline` read. `Routes` records the centres it was
// built at, so `reroute` works out by itself which boxes moved and
// re-routes only the lines that move broke (EDIT-2, EDIT-25 drag
// preview). Routing one view needs only its plan (CORE-28).
//
// Units: every point is in grid units (1 unit = 10px, `geom`). A line is
// its vertex list: first vertex on the source port, last vertex on the
// destination port, or on the trunk for a joined line (CORE-8); only
// bends are vertices, so consecutive segments differ in direction.
//
// DECISION: `Routes::lines` is indexed by `EdgeId` like `ViewPlan::edges`,
// with `None` for an unroutable edge, so `label` and `scene` look a line
// up in O(1) and the unroutable set is visible in one place.
// DECISION: a line's destination end is an enum (`LineEnd`): an arrow
// on a port of the destination box, or a join dot on a trunk line; a
// joined line has no port on the destination box (CORE-8, CORE-9).
// DECISION: the costs (BEND 4, CROSS 150, NEAR 12, LANE x0.7) and the
// PRNG stay private: they are the router's internals (HLD §5); only the
// iteration cap and the number of edge orders are configurable, because
// the editor's drag preview needs a cheaper run than the CLI (EDIT-25).
//
// Data layout for owner sign-off (CLAUDE.md: not the agent's call):
// `Side` is `repr(u8)` with discriminants N 0, E 1, S 2, W 3; `Port` is
// `repr(C)`, fields `at: Point` then `side: Side`, 12 bytes, pinned by
// `port_size_is_pinned`.
//
// CORE-13: route gives an unroutable line no polyline
// (`Routes::lines[e] == None`); `scene` draws it straight from its source
// box's edge to its destination box's edge, along the line between their
// centres, with an arrowhead and no label, outside the CORE-6..9 checks,
// and L1 names it.
//
// Cooperative stop (EDIT-25, owner's ruling): `route` and `reroute` take
// a `&AtomicBool` stop flag (std only). They read it at least once per
// rip-up round and once per edge order; once it reads true they return
// `Err(Stopped)` promptly and yield no routes, never a partial `Routes`.
// A run whose flag is never set returns exactly what it returned before
// the flag existed (CORE-25). The editor's worker sets it to drop a stale
// build (crates/editor/src/worker.rs); the CLI passes a flag never set.
// DECISION: `Result<Routes, Stopped>` over `Option<Routes>`: the
// pipeline forwards it with `?`, and `Stopped` says why there is no value.
// DECISION: the flag is read with `Ordering::Relaxed`: it carries no data,
// only "give up", so no ordering with other memory is needed.

use std::fmt;
use std::sync::atomic::AtomicBool;

use crate::geom::{Point, Segment};
use crate::view::{EdgeId, ViewPlan};

/// A side of a box: where a port sits and which way the line leaves it,
/// square to the side (CORE-9). Discriminants are fixed so a port can be
/// stored per grid cell. Serves CORE-9.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side {
    /// Top side (-y); a person's single top port (CORE-9).
    N = 0,
    /// Right side (+x).
    E = 1,
    /// Bottom side (+y).
    S = 2,
    /// Left side (-x).
    W = 3,
}

/// Where one line meets one box: a grid point on the box's outline and
/// the side it is on. Each line has its own port on each box it touches
/// (no two arrowheads at the same point), at least 20px from a corner;
/// a person has at most one top port, at its centre, and side ports at
/// least 80px below its top; a cylinder's top and bottom ports sit
/// within 30px of its centre line (shape from `PlanBox.style.shape`).
/// Serves CORE-9.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Port {
    /// The port point, on the box outline, in grid units.
    pub at: Point,
    /// The box side the port is on.
    pub side: Side,
}

/// Where a joined line meets its trunk with `bus on`: the joined line
/// ends there in a dot, not an arrowhead; the meeting is not a crossing
/// (CORE-8, L4). The trunk is a line into the same box, arriving on the
/// same side, and always ends in an arrowhead: no join chains, so a
/// trunk is never itself a joined line. Serves CORE-8, CORE-10, CORE-14
/// (a joined line whose label matches its trunk's has none).
// Not size-pinned: one per joined line, never stored per grid cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Join {
    /// The line joined into; it has the same destination box and its
    /// `end` is `LineEnd::Arrow`.
    pub trunk: EdgeId,
    /// The join dot: the joined line's last vertex, on the trunk.
    pub at: Point,
}

/// How a line ends at its destination (CORE-10: one arrowhead, or a join
/// dot). `#[non_exhaustive]` so later phases may add ends without a
/// breaking change. Serves CORE-8, CORE-9, CORE-10.
// Not size-pinned: one per line, never stored per grid cell.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LineEnd {
    /// An arrowhead on its own port of the destination box (CORE-9).
    Arrow(Port),
    /// A dot where the line joins a trunk (CORE-8, `bus on` only).
    Join(Join),
}

/// One routed line: its vertices on the grid and its two ends. Every
/// segment is at 0°, 45° or 90° and at least 20px long, every bend
/// turns by 45° (CORE-6); it passes through no box but its own ends and
/// keeps 10px from every other box (CORE-7); it shares no segment with
/// another line, and crosses others only straight through, never at a
/// bend (CORE-8). Serves CORE-6, CORE-7, CORE-8, CORE-9, CORE-10.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Line {
    /// The plan edge this line draws; always equals its index in
    /// `Routes::lines` (`lines[i].edge == EdgeId(i)`), so a `&Line`
    /// passed on alone still names its edge.
    pub edge: EdgeId,
    /// Vertices in grid units, source to destination: the source port,
    /// each bend, then the destination port or the join point. At least
    /// two; consecutive segments differ in direction.
    pub points: Vec<Point>,
    /// The line's own port on the source box.
    pub source: Port,
    /// The arrowhead port on the destination box, or the join on a trunk.
    pub end: LineEnd,
}

impl Line {
    /// The segments of `points`, in order (CORE-6, CORE-10 chevrons,
    /// CORE-14 label runs; `scene` takes the arrowhead direction from
    /// the last one).
    #[must_use]
    pub fn segments(&self) -> Vec<Segment> {
        todo!()
    }

    /// The trunk this line joins, `None` when it ends in an arrowhead
    /// (CORE-8, CORE-14).
    #[must_use]
    pub const fn joins(&self) -> Option<EdgeId> {
        todo!()
    }
}

/// A stretch where two lines heading the same way run side by side
/// exactly 20px apart (one lane, CORE-12). Derived from the routed
/// lines by `Routes::lanes`; the CORE-12 quality measure is the summed
/// length of these stretches. Serves CORE-12.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Lane {
    /// The lower-numbered of the two lines.
    pub a: EdgeId,
    /// The higher-numbered of the two lines.
    pub b: EdgeId,
    /// The stretch of `a` that runs beside `b`.
    pub a_run: Segment,
    /// The stretch of `b` that runs beside `a`, parallel to `a_run`.
    pub b_run: Segment,
}

/// Router limits. `DEFAULT` is what the CLI and the full re-route after
/// a drag use (the editor and the CLI then give the same scene,
/// EDIT-25); the drag preview may pass a smaller cap. Serves CORE-32,
/// CORE-13, CORE-25, CORE-24.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RouteConfig {
    /// Number of edge orders tried, the best kept by (unroutable count,
    /// crossings, total length): two by Manhattan distance between box
    /// centres (ascending, descending), the rest shuffles from a
    /// fixed-seed PRNG, so the set is the same on every run (CORE-13,
    /// CORE-25). Length is compared exactly as the pair (straight steps,
    /// diagonal steps), no floats; a full tie keeps the lowest order
    /// index. At least 1: `route` and `reroute` panic on 0.
    pub orders: u32,
    /// Cap on rip-up-and-retry rounds per edge order; routing stops at
    /// the cap and returns what it has, reporting the rest (CORE-32).
    /// 0 means one routing pass with no rip-up.
    pub max_iterations: u32,
}

impl RouteConfig {
    /// The prototype's settings: 8 edge orders (2 sorted + 6 shuffles)
    /// and the default rip-up cap. Only `orders: 8` is part of the
    /// contract; the `max_iterations` value is not, and step 5 may tune
    /// it without a contract change, as long as CORE-11 and CORE-24
    /// hold. Serves CORE-11, CORE-32.
    pub const DEFAULT: RouteConfig = RouteConfig {
        orders: 8,
        max_iterations: 32,
    };
}

/// The routed lines of one view. Same plan, centres and config give an
/// equal value on every run and machine (CORE-25). Serves CORE-6..13,
/// CORE-25, CORE-32.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Routes {
    /// One entry per plan edge, `lines[i]` for `EdgeId(i)`; `None` for an
    /// edge that could not be routed (CORE-13).
    pub lines: Vec<Option<Line>>,
    /// The box centres these lines were routed at, one per plan box
    /// (`centres[i]` for `BoxId(i)`, the first `plan.boxes.len()` entries
    /// of the `centres` given). `reroute` compares against it to find
    /// the boxes that moved (EDIT-25).
    pub centres: Vec<Point>,
    /// Crossings between the routed lines: the sum over each pair of
    /// lines of `geom::crossings`, minus the one meeting where a joined
    /// line ends on its trunk. Whoever counts L4 for this view (`lint`,
    /// from the scene) must use the same rule or read this value through
    /// the scene, so the CORE-11 bar and L4 agree. Serves CORE-11.
    pub crossings: u32,
}

impl Routes {
    /// The line of `edge`, `None` when it is unroutable or out of range.
    /// Serves CORE-13, CORE-14, CORE-10.
    #[must_use]
    pub fn line(&self, edge: EdgeId) -> Option<&Line> {
        todo!()
    }

    /// The edges that could not be routed, ascending: exactly the indices
    /// where `lines[i]` is `None`. Reported as L1 by id, never dropped
    /// silently (CORE-13, CORE-32).
    #[must_use]
    pub fn unroutable(&self) -> Vec<EdgeId> {
        todo!()
    }

    /// Every lane (two lines heading the same way exactly 20px apart),
    /// ordered by `(a, b)` then by run start. Serves CORE-12.
    #[must_use]
    pub fn lanes(&self) -> Vec<Lane> {
        todo!()
    }
}

/// A routing run gave up because its stop flag was set (EDIT-25): no
/// routes are returned, partial or otherwise. Serves EDIT-25.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Stopped;

impl fmt::Display for Stopped {
    /// `routing stopped` (a newer request superseded this one).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl std::error::Error for Stopped {}

/// Routes every edge of `plan` with every box at `centres[box]`: ports
/// per box side, A* on the grid with 45° turns, negotiated-congestion
/// rip-up and retry over `config.orders` edge orders, up to
/// `config.max_iterations` rounds each; always returns, listing what it
/// could not route (CORE-13, CORE-32). Joins only when
/// `plan.options.bus` (CORE-8). Lines may leave the bounding rectangle
/// of all boxes; they are never clipped, and `scene` sizes the canvas
/// to contain every line point (CORE-7). Deterministic (CORE-25).
///
/// `stop` is read at least once per rip-up round and once per edge
/// order; a run that never sees it set returns `Ok` with the same
/// `Routes` as a run without the flag (CORE-25). Serves CORE-6..13,
/// CORE-25, CORE-32, EDIT-25.
///
/// # Errors
///
/// `Stopped` when `stop` reads true at one of those checks: the run
/// returns promptly and yields no routes (EDIT-25).
///
/// # Panics
///
/// When `centres` is shorter than `plan.boxes`, or `config.orders == 0`.
pub fn route(
    plan: &ViewPlan,
    centres: &[Point],
    config: &RouteConfig,
    stop: &AtomicBool,
) -> Result<Routes, Stopped> {
    todo!()
}

/// Re-routes, with every box at `centres[box]`, only the lines of
/// `previous` that the move broke, keeping every line still valid at
/// the new centres exactly as it is (its points and ports stay
/// obstacles and are not ripped up): the drag preview of EDIT-2 and
/// EDIT-25. The re-routed set is, in edge order:
///
/// 1. every edge touching a box whose centre differs from
///    `previous.centres`;
/// 2. every other routed line that a moved box now covers or comes
///    within 10px of (CORE-7);
/// 3. repeated until nothing is added, every line whose `Join::trunk`
///    is in the set (CORE-8: the dot must stay on the trunk).
///
/// An edge unroutable in `previous` is retried only when it is in the
/// set; the full `route` after the drag retries the rest. When
/// `centres` equals `previous.centres` the set is empty and the result
/// equals `previous`. `crossings` is recounted over the whole view and
/// `centres` set to the new centres. Deterministic (CORE-25).
///
/// Both functions stop cooperatively: `stop` is read at least once per
/// rip-up round and once per edge order, and a set flag ends the run
/// promptly with `Err(Stopped)`, so the editor's worker can drop a stale
/// preview or build without waiting for it (EDIT-25); preview callers
/// still bound the cost with a smaller `RouteConfig` cap (CORE-24). A
/// run that never sees the flag set returns the same `Routes` as before
/// (CORE-25). Serves EDIT-2, EDIT-25, CORE-13, CORE-25, CORE-32.
///
/// # Errors
///
/// `Stopped` when `stop` reads true at one of those checks; no routes are
/// yielded (EDIT-25).
///
/// # Panics
///
/// When `centres` is shorter than `plan.boxes`, `config.orders == 0`,
/// or `previous` is a `Routes` of another plan (`previous.lines` not as
/// long as `plan.edges`, or `previous.centres` not as long as
/// `plan.boxes`).
pub fn reroute(
    plan: &ViewPlan,
    centres: &[Point],
    previous: &Routes,
    config: &RouteConfig,
    stop: &AtomicBool,
) -> Result<Routes, Stopped> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Size pins: ports are kept per line end, per grid cell, in the
    // router's board. Layout pending owner sign-off (header comment).
    #[test]
    fn port_size_is_pinned() {
        assert_eq!(std::mem::size_of::<Port>(), 12);
    }

    #[test]
    fn side_size_is_pinned() {
        assert_eq!(std::mem::size_of::<Side>(), 1);
    }

    // One test per requirement.
    #[test]
    fn core_6_segments_octilinear_on_grid() {
        // Also: every bend turns by 45° only, every segment >= 20px
        // (min run before a turn); route-02 plan tests turns_by_45_only
        // and min_run_before_turn are folded in here.
        todo!()
    }

    #[test]
    fn core_7_never_through_box() {
        // Also 10px clearance from other boxes; boxes at the canvas edge:
        // lines may leave the bounding rectangle of all boxes, never
        // clipped (Review Focus).
        todo!()
    }

    #[test]
    fn core_8_no_shared_segments() {
        todo!()
    }

    #[test]
    fn core_8_bus_join_with_dot() {
        // Join after 90px of own run, at <= 90° to the trunk, same box and
        // same side only (Review Focus); the join is not a crossing; the
        // trunk ends in an arrowhead (no chains).
        todo!()
    }

    #[test]
    fn core_8_no_join_when_bus_off() {
        // Same plan as core_8_bus_join_with_dot with `options.bus` off:
        // every routed line ends in `LineEnd::Arrow`.
        todo!()
    }

    #[test]
    fn core_8_crossing_never_at_bend() {
        todo!()
    }

    #[test]
    fn core_9_one_port_per_line() {
        // Also A->B and B->A: two lines, separate ports (Review Focus);
        // square to the side, >= 20px from a corner; person and cylinder
        // port rules.
        todo!()
    }

    #[test]
    fn core_10_one_arrowhead_or_join_dot() {
        // Route's part of CORE-10; chevrons are scene's.
        todo!()
    }

    #[test]
    fn core_11_crossings_counted_like_l4() {
        // The example quality bar itself is core_11_example_crossings_no_worse
        // in tests/golden_route.rs.
        todo!()
    }

    #[test]
    fn core_12_parallel_lines_constant_gap() {
        // TODO(srs): CORE-12 per-view lane-length numbers from the
        // prototype are not in the SRS yet; this test checks the 20px
        // gap only until they are (docs/deferred.md).
        todo!()
    }

    #[test]
    fn core_13_rip_up_resolves_conflict() {
        todo!()
    }

    #[test]
    fn core_13_unroutable_reported_by_id() {
        // Two boxes too close for any path: reported by id, the rest
        // still route (Review Focus). Asserts the invariants:
        // `unroutable()` == indices of `None` in `lines`, and
        // `lines[i].edge == EdgeId(i)` for every routed line.
        todo!()
    }

    #[test]
    fn core_25_same_input_same_routes() {
        todo!()
    }

    #[test]
    fn core_32_iteration_cap_always_returns() {
        // Also `max_iterations: 0` returns one pass with no rip-up.
        todo!()
    }

    #[test]
    fn edit_25_stopped_route_returns_promptly_without_routes() {
        // A flag already set, and one set from another thread while a
        // heavy plan routes: `route` and `reroute` return `Err(Stopped)`
        // promptly (within one rip-up round or edge order) and yield no
        // routes. The same inputs with a flag never set give `Ok` with
        // the routes of an unstopped run (CORE-25).
        todo!()
    }

    #[test]
    fn edit_25_reroute_keeps_other_lines() {
        // Every line still valid at the new centres stays as it is;
        // a kept line the moved box now covers, and a line joined to a
        // re-routed trunk, are re-routed too.
        todo!()
    }

    // Plan test names (route-01, route-05).
    #[test]
    fn ports_spread_along_side() {
        todo!()
    }

    #[test]
    fn lines_avoid_every_box_at_any_centre() {
        todo!()
    }

    #[test]
    fn incremental_matches_full_when_unchanged() {
        // `reroute` with `centres == previous.centres` returns `previous`.
        todo!()
    }

    #[test]
    fn lanes_listed_in_order() {
        todo!()
    }
}
