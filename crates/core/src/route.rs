//! Octilinear grid router: 0/45/90 degree lines on the 10px grid, one
//! port per line, bus lanes and joins, fewest crossings, rip-up and
//! retry, deterministic output; can re-route only what a box move broke
//! (CORE-6..13, 25).
//!
//! Depends on: `view`, `geom`, `model` (`Shape`, for the person and
//! cylinder port rules of CORE-9).

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

use std::cmp::{Ordering as CmpOrdering, Reverse};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::geom::{CLEARANCE_UNITS, Contact, Dir, Point, Rect, Segment, crossings};
// DECISION: `Shape` is named from `model` directly: `PlanBox::style` is
// already a `model::ResolvedStyle`, so route reads model data either way,
// and a re-export from `view` would change view's frozen pub surface.
use crate::model::Shape;
use crate::anchor::{Anchor, AnchorSide};
use crate::view::{BoxId, EdgeId, PlanBox, PlanEdge, ViewPlan};

// P4-400..P4-417: bundles (the editor's "Bus") are routed first, in
// `route/bundle.rs`; a view without bundles never enters it (P4-403).
mod bundle;
// Test-only: counters of one router call and the drag preview
// measurements that read them (no hook exists outside `cfg(test)`).
#[cfg(test)]
mod drag_perf;
#[cfg(test)]
mod probe;

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
        self.points
            .windows(2)
            .map(|w| Segment::new(w[0], w[1]))
            .collect()
    }

    /// The trunk this line joins, `None` when it ends in an arrowhead
    /// (CORE-8, CORE-14).
    #[must_use]
    pub const fn joins(&self) -> Option<EdgeId> {
        match self.end {
            LineEnd::Join(j) => Some(j.trunk),
            LineEnd::Arrow(_) => None,
        }
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
    /// One entry per `ViewPlan::bundles` entry, in the same order; empty
    /// on a view without bundles. Serves P4-405, P4-408, P4-412, P4-417.
    // APPROVER: new pub item
    pub bundles: Vec<RoutedBundle>,
}

/// How one bundle was routed (P4-405..P4-417). Points are grid units, as
/// in `Line::points`. Serves P4-405, P4-408, P4-412, P4-417, P4-420.
// APPROVER: new pub item
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoutedBundle {
    /// The bundle's members, as `ViewPlan::bundles` lists them.
    pub members: Vec<EdgeId>,
    /// The members routed on the shared stretch, in track order (track
    /// 0, the rightmost seen along the spine, first); empty when the
    /// bundle fell back to ordinary lines (`fallback`).
    pub tracks: Vec<EdgeId>,
    /// Per entry of `tracks`: that member's track, a run of consecutive
    /// segments of its line, in the member's own direction (P4-405).
    pub shared: Vec<Vec<Point>>,
    /// Crossings between members (the L4 rule, every pair of `tracks`),
    /// reported as L30 when above 0 (P4-412).
    pub crossings: u32,
    /// True when no shared stretch of useful length exists and the
    /// members were routed as ordinary lines (P4-408, L31).
    pub fallback: bool,
    /// Members that kept their vertices and left the bundle (P4-417,
    /// L31), ascending.
    pub left: Vec<EdgeId>,
}

impl RoutedBundle {
    /// The bundle `edge` belongs to in `bundles`, if any. Serves P4-418,
    /// P4-426.
    #[must_use]
    pub fn of(bundles: &[RoutedBundle], edge: EdgeId) -> Option<&RoutedBundle> {
        bundles.iter().find(|b| b.members.contains(&edge))
    }

    /// The track of `edge` (P4-405), `None` when it is not on the shared
    /// stretch.
    #[must_use]
    pub fn track(&self, edge: EdgeId) -> Option<&[Point]> {
        let k = self.tracks.iter().position(|&e| e == edge)?;
        self.shared.get(k).map(Vec::as_slice)
    }
}

impl Routes {
    /// The line of `edge`, `None` when it is unroutable or out of range.
    /// Serves CORE-13, CORE-14, CORE-10.
    #[must_use]
    pub fn line(&self, edge: EdgeId) -> Option<&Line> {
        self.lines.get(edge.0 as usize).and_then(Option::as_ref)
    }

    /// The edges that could not be routed, ascending: exactly the indices
    /// where `lines[i]` is `None`. Reported as L1 by id, never dropped
    /// silently (CORE-13, CORE-32).
    #[must_use]
    pub fn unroutable(&self) -> Vec<EdgeId> {
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_none())
            .map(|(i, _)| edge_id(i))
            .collect()
    }

    /// Every lane (two lines heading the same way exactly 20px apart),
    /// ordered by `(a, b)` then by run start. Serves CORE-12.
    #[must_use]
    pub fn lanes(&self) -> Vec<Lane> {
        let routed: Vec<&Line> = self.lines.iter().flatten().collect();
        let mut out = Vec::new();
        for (i, a) in routed.iter().enumerate() {
            for b in &routed[i + 1..] {
                for s in a.segments() {
                    for t in b.segments() {
                        if let Some((a_run, b_run)) = lane_runs(s, t) {
                            out.push(Lane {
                                a: a.edge,
                                b: b.edge,
                                a_run,
                                b_run,
                            });
                        }
                    }
                }
            }
        }
        // `Lane`'s derived order is (a, b, a_run start, ...): the contract order.
        out.sort();
        out
    }
}

/// A routing run gave up because its stop flag was set (EDIT-25): no
/// routes are returned, partial or otherwise. Serves EDIT-25.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Stopped;

impl fmt::Display for Stopped {
    /// `routing stopped` (a newer request superseded this one).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("routing stopped")
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
    assert!(
        centres.len() >= plan.boxes.len(),
        "route: fewer centres than boxes"
    );
    assert!(config.orders >= 1, "route: config.orders must be at least 1");
    check(stop)?;
    if !plan.bundles.is_empty() {
        return bundle::solve_bundled(plan, centres, *config, stop);
    }
    let kept = vec![None; plan.edges.len()];
    let set: Vec<usize> = (0..plan.edges.len()).collect();
    if pinned_any(plan) {
        return solve_pinned(plan, centres, *config, stop, kept, &set, &[], None, None);
    }
    solve(plan, centres, *config, stop, kept, &set, &[], None, None)
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
    assert!(
        centres.len() >= plan.boxes.len(),
        "reroute: fewer centres than boxes"
    );
    assert!(config.orders >= 1, "reroute: config.orders must be at least 1");
    assert!(
        previous.lines.len() == plan.edges.len() && previous.centres.len() == plan.boxes.len(),
        "reroute: previous routes belong to another plan"
    );
    reroute_with(plan, centres, previous, &[], *config, stop)
}

/// `reroute` with the edges of `forced` re-routed too, broken or not:
/// the vertex drag preview, where one line's vertices changed and no box
/// moved (P3-311, EDIT-25). An index out of range is ignored.
///
/// # Errors
///
/// As `reroute`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reroute_with(
    plan: &ViewPlan,
    centres: &[Point],
    previous: &Routes,
    forced: &[usize],
    config: RouteConfig,
    stop: &AtomicBool,
) -> Result<Routes, Stopped> {
    check(stop)?;
    // DECISION: a view with bundles re-routes whole: a moved box shifts a
    // spine, and with it every track of the bundle and the corridor the
    // other lines cross (P4-404); bundled views are small, the preview's
    // cheaper `config` still bounds the cost.
    if !plan.bundles.is_empty() {
        #[cfg(test)]
        probe::with(|s| {
            s.broken = broken(plan, centres, previous).len();
            s.path = "bundled";
        });
        return bundle::solve_bundled(plan, centres, config, stop);
    }
    let mut set = broken(plan, centres, previous);
    #[cfg(test)]
    probe::with(|s| {
        s.broken = set.len();
        s.path = if set.is_empty() { "none" } else if pinned_any(plan) { "pinned" } else { "solve" };
    });
    if !forced.is_empty() {
        set.extend(forced.iter().copied().filter(|&e| e < plan.edges.len()));
        set.sort_unstable();
        set.dedup();
    }
    if set.is_empty() {
        // DECISION: nothing broke, so the lines and crossings are
        // `previous`'s, but `centres` still takes the new centres: a box
        // with no edges and clear of every line may have moved, and the
        // editor draws boxes at `Routes::centres` (EDIT-2).
        return Ok(Routes {
            centres: centres[..plan.boxes.len()].to_vec(),
            ..previous.clone()
        });
    }
    let mut kept = previous.lines.clone();
    for &e in &set {
        kept[e] = None;
    }
    let carried = carry(plan, centres, previous, &set, forced, &mut kept);
    #[cfg(test)]
    probe::with(|s| s.translated = carried.len());
    set.retain(|e| !carried.contains(e));
    // The preview's bar: no worse than the routes the drag started from.
    let before = Some(Score::of(&previous.lines));
    if pinned_any(plan) {
        return solve_pinned(plan, centres, config, stop, kept, &set, &[], before, Some(previous));
    }
    solve(plan, centres, config, stop, kept, &set, &[], before, Some(previous))
}

// ------------------------------------------------------------ internals
//
// Costs are the prototype's (`c4.py`: STEP 1 / 1.45, BEND 4, CROSS 150,
// NEAR 12, LANE x0.7, start offset 0.4 per cell), scaled by 100.
// DECISION: integer costs (x100) instead of floats: every sum and the
// LANE x0.7 product (`* 7 / 10`, rounded down) are exact, so the search
// order is the same on every machine (CORE-25).
const STEP_STRAIGHT: u32 = 100;
const STEP_DIAGONAL: u32 = 145;
const BEND: u32 = 400;
const CROSS: u32 = 15_000;
const NEAR: u32 = 1_200;
const LANE_NUM: u32 = 7;
const LANE_DEN: u32 = 10;
/// An axis step running 10px or 30px beside a step of another line that
/// heads the same way (CORE-12: same-way neighbours are exactly 20px
/// apart).
// DECISION: a cost equal to CROSS, not a ban: the rule is kept wherever
// any other routing exists, but a line is never left unroutable for it
// (CORE-13), and it weighs as much as a crossing, so the CORE-11
// example crossings stay as they were (measured).
const CLASH: u32 = 2 * CROSS;
const START_OFFSET: u32 = 40;
/// Heuristic weights, x100: octile distance (max + 0.45 min) to the
/// targets' bounding box.
// DECISION: the plain octile distance, not the prototype's LANE x (max +
// 0.4 min): it can overestimate where lanes discount a step, but it halves
// the states searched, and on `example/` it gives the CORE-11 crossings
// and more CORE-12 lane length than the admissible one (measured).
const H_MAX: u32 = 100;
const H_MIN: u32 = 45;
/// States a search pops before it restarts with the cost bound.
const POP_LIMIT: u32 = 12_000;
/// Steps of own run from the first free cell before a join (CORE-8: the
/// two port cells plus 9 steps = 110px >= 90px; the prototype's `MIN_JOIN`).
const MIN_JOIN: u8 = 9;
/// `Rec::steps` of a state that has just joined its trunk.
const JOINED: u8 = u8::MAX;
/// Ports keep this many cells from a corner (CORE-9: 20px).
const CORNER: i32 = 2;
/// Cells taken by a person's head: side ports start below it (CORE-9).
const PERSON_HEAD: i32 = 6;
/// Max offset from the centre line of a narrow top/bottom port (CORE-9: 30px).
const NARROW: i32 = 3;
// DECISION: the board is the boxes' bounding rectangle grown by 6 cells
// (the prototype's 60px margin) on every side, plus any kept line point,
// so lines may leave the boxes' bounds (CORE-7) without an unbounded search.
const MARGIN: i32 = 6;
// DECISION: rip-up raises the history cost of each contested cell by 30
// steps per round and stops an order after a round without a better
// score; with the iteration cap that bounds the work (CORE-32, CORE-24).
const HISTORY: u32 = 3_000;
/// P3-123: crossing a deployment node boundary that holds neither or both
/// ends of the line. Above `CROSS`, so a foreign node is skirted whenever
/// a way round exists; a charge, not a wall, so no line becomes
/// unroutable (L27 reports what is left).
const FENCE: u32 = 40_000;
/// P3-123: crossing a node boundary that holds one end (needed once);
/// each extra crossing pays it again.
const FENCE_NEEDED: u32 = BEND;
/// Each step on a deployment node's name tag, so lines keep off the
/// text (fixture review).
const ON_TAG: u32 = 3_000;
const STALL_ROUNDS: u32 = 1;
/// A view with more edges than this routes as a large view: each search
/// runs once, with the cost bound, inside its two boxes' window.
// DECISION: a size rule (CORE-25: never a clock): the example views have
// at most 21 edges and keep their routing unchanged (same SVG bytes);
// the CORE-31 view (60 edges) is the large case it is for.
const LARGE_EDGES: usize = 32;
/// Cells a large view's search window reaches past the bounding
/// rectangle of the line's two boxes (the board's own `MARGIN`).
// DECISION: the board margin, so a line has the same room around its
// two boxes as the board gives every box; a search that finds no path in
// its window searches the whole board (CORE-13: never unroutable for it).
// On the CORE-31 view it also gives fewer crossings (232 -> 215, measured).
const WINDOW_MARGIN: i32 = MARGIN;
/// Seed of the edge-order shuffles (the prototype's default seed).
const SEED: u64 = 7;
/// States a long search loop (`astar`, `cost_from`, `leg`) handles
/// between two reads of the stop flag (EDIT-25).
const STOP_EVERY: u32 = 1024;

#[cfg_attr(test, track_caller)]
fn check(stop: &AtomicBool) -> Result<(), Stopped> {
    #[cfg(test)]
    probe::check();
    if stop.load(Ordering::Relaxed) {
        Err(Stopped)
    } else {
        Ok(())
    }
}

/// `check` once every `STOP_EVERY` states of a long loop, `n` being the
/// states it has handled so far (EDIT-25: a stale run gives way within
/// about a millisecond, not after its current search).
// DECISION: a count, not a clock (CORE-25), and 1024 states (~0.1 ms at
// the measured 7-17k pops/ms): one relaxed load per 1024 pops costs
// nothing measurable, and the flag only ever ends a run with `Stopped`.
// DECISION: the loops that now take `stop` keep their `step` (a step's
// cost) as named; `clippy::similar_names` is allowed on those functions.
#[inline]
#[cfg_attr(test, track_caller)]
fn tick(n: u32, stop: &AtomicBool) -> Result<(), Stopped> {
    if n.is_multiple_of(STOP_EVERY) {
        #[cfg(test)]
        probe::work(STOP_EVERY);
        check(stop)
    } else {
        Ok(())
    }
}

fn edge_id(i: usize) -> EdgeId {
    EdgeId(u32::try_from(i).expect("edge index fits u32"))
}

const fn side_dir(side: Side) -> Dir {
    match side {
        Side::N => Dir::N,
        Side::E => Dir::E,
        Side::S => Dir::S,
        Side::W => Dir::W,
    }
}

/// Axis bit of a direction: E/W, NE/SW, N/S, NW/SE.
const fn axis_bit(d: Dir) -> u8 {
    1 << (d as u8 % 4)
}

/// `d` turned by `by` 45° steps, `by` in `-1..=1` (positive is
/// counter-clockwise, `Dir::ALL` order).
const fn turn(d: Dir, by: i8) -> Dir {
    let k = match by {
        1 => 1,
        -1 => 7,
        _ => 0,
    };
    Dir::ALL[(d as usize + k) % 8]
}

fn step_n(p: Point, d: Dir, n: i32) -> Point {
    let (dx, dy) = d.delta();
    Point::new(p.x + dx * n, p.y + dy * n)
}

/// Two parallel axis runs heading the same way exactly 2 cells (20px)
/// apart, cut to their common stretch (CORE-12).
// DECISION: only horizontal and vertical runs form lanes: two diagonal
// runs on the 10px grid are never exactly 20px apart (14px or 28px).
fn lane_runs(s: Segment, t: Segment) -> Option<(Segment, Segment)> {
    let d = s.dir()?;
    if t.dir()? != d || d.is_diagonal() {
        return None;
    }
    let horizontal = matches!(d, Dir::E | Dir::W);
    let (sk, tk) = if horizontal { (s.a.y, t.a.y) } else { (s.a.x, t.a.x) };
    if (sk - tk).abs() != 2 {
        return None;
    }
    let along = |p: Point| if horizontal { p.x } else { p.y };
    let lo = along(s.a).min(along(s.b)).max(along(t.a).min(along(t.b)));
    let hi = along(s.a).max(along(s.b)).min(along(t.a).max(along(t.b)));
    if hi <= lo {
        return None;
    }
    let forward = along(s.b) > along(s.a);
    let (from, to) = if forward { (lo, hi) } else { (hi, lo) };
    let at = |k: i32, v: i32| {
        if horizontal {
            Point::new(v, k)
        } else {
            Point::new(k, v)
        }
    };
    Some((
        Segment::new(at(sk, from), at(sk, to)),
        Segment::new(at(tk, from), at(tk, to)),
    ))
}

/// A candidate port and its distance from the side's centre.
#[derive(Clone, Copy)]
struct Spot {
    port: Port,
    off: i32,
}

/// Every port a box offers at `centre`, in the prototype's order: along
/// x the top then bottom port, then along y the left then right port
/// (CORE-9).
fn spots(b: &PlanBox, centre: Point) -> Vec<Spot> {
    let r = b.rect_at(centre);
    let person = b.style.shape == Shape::Person;
    let cylinder = b.style.shape == Shape::Cylinder;
    let head = if person { PERSON_HEAD } else { 0 };
    let mut out = Vec::new();
    for x in (r.min.x + CORNER)..=(r.max.x - CORNER) {
        let off = (x - centre.x).abs();
        let top_ok = if person {
            off == 0
        } else {
            !cylinder || off <= NARROW
        };
        if top_ok {
            out.push(Spot {
                port: Port {
                    at: Point::new(x, r.min.y),
                    side: Side::N,
                },
                off,
            });
        }
        if !cylinder || off <= NARROW {
            out.push(Spot {
                port: Port {
                    at: Point::new(x, r.max.y),
                    side: Side::S,
                },
                off,
            });
        }
    }
    let ymid = (r.min.y + head + r.max.y).div_euclid(2);
    for y in (r.min.y + CORNER + head)..=(r.max.y - CORNER) {
        let off = (y - ymid).abs();
        out.push(Spot {
            port: Port {
                at: Point::new(r.min.x, y),
                side: Side::W,
            },
            off,
        });
        out.push(Spot {
            port: Port {
                at: Point::new(r.max.x, y),
                side: Side::E,
            },
            off,
        });
    }
    out
}

/// The router's side for an anchor's side.
const fn anchor_side(side: AnchorSide) -> Side {
    match side {
        AnchorSide::Top => Side::N,
        AnchorSide::Right => Side::E,
        AnchorSide::Bottom => Side::S,
        AnchorSide::Left => Side::W,
    }
}

/// True when `e` has a pinned end (P4-204).
const fn anchored(e: &PlanEdge) -> bool {
    e.source_anchor.is_some() || e.destination_anchor.is_some()
}

/// The ports `anchor` offers on `b` at `centre` (P4-206, P4-207): its
/// point, square to each side it lies on (two for a corner), at offset 0
/// so no start cost is added.
// DECISION: a corner offers both its sides and the search takes the
// cheaper one (P4-207): the line still leaves square to a side, and the
// side facing the other end wins without a rule of its own.
fn anchor_spots(b: &PlanBox, centre: Point, anchor: Anchor) -> Vec<Spot> {
    let at = anchor.point(b.rect_at(centre));
    anchor
        .sides()
        .into_iter()
        .map(|side| Spot {
            port: Port {
                at,
                side: anchor_side(side),
            },
            off: 0,
        })
        .collect()
}

/// The ports a line end may use on `b` at `centre`: the anchor's when the
/// end is pinned, else every port of the box (CORE-9).
fn end_spots(b: &PlanBox, centre: Point, anchor: Option<Anchor>) -> Vec<Spot> {
    match anchor {
        Some(a) => anchor_spots(b, centre, a),
        None => spots(b, centre),
    }
}

/// The routing area in grid units.
#[derive(Debug, Clone, Copy)]
struct Grid {
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
}

impl Grid {
    fn new(plan: &ViewPlan, centres: &[Point], kept: &[Option<Line>]) -> Grid {
        let mut lo = Point::new(i32::MAX, i32::MAX);
        let mut hi = Point::new(i32::MIN, i32::MIN);
        let mut grow = |p: Point| {
            lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
        };
        for b in &plan.boxes {
            let r = b.rect_at(centres[b.id.0 as usize]).inflate(MARGIN);
            grow(r.min);
            grow(r.max);
        }
        for line in kept.iter().flatten() {
            for &p in &line.points {
                grow(p);
            }
        }
        // P3-123: room to skirt a deployment node boundary. P4-600: two
        // lanes outside every boundary, clear of its stroke.
        // DECISION: the board, not placement, gives the room: it reaches
        // 4 cells past every other boundary, so two lanes run outside it at
        // 20px and 40px from the stroke (BOUNDARY_MARGIN_UNITS unchanged,
        // the goldens stay byte-identical); measured best of 2..5 cells for
        // C3-startup's routing time (one lane: +40%, two: +20%).
        for k in 0..plan.boundaries.len() {
            let deployment = matches!(plan.boundaries[k].of, crate::view::BoundaryOf::DeploymentNode(_));
            if let Some(r) = plan.boundary_rect(k, centres) {
                let r = r.inflate(if deployment { MARGIN / 2 } else { 2 * STROKE_CLEAR });
                grow(r.min);
                grow(r.max);
            }
        }
        if lo.x > hi.x {
            return Grid {
                x0: 0,
                y0: 0,
                w: 1,
                h: 1,
            };
        }
        Grid {
            x0: lo.x,
            y0: lo.y,
            w: hi.x - lo.x + 1,
            h: hi.y - lo.y + 1,
        }
    }

    fn len(self) -> usize {
        usize::try_from(self.w).unwrap_or(0) * usize::try_from(self.h).unwrap_or(0)
    }

    fn index(self, p: Point) -> Option<usize> {
        let (x, y) = (p.x - self.x0, p.y - self.y0);
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        usize::try_from(y * self.w + x).ok()
    }

    fn point(self, i: usize) -> Point {
        let i = i32::try_from(i).expect("cell index fits i32");
        Point::new(self.x0 + i % self.w, self.y0 + i / self.w)
    }
}

/// The square a diagonal step crosses (its min corner) and the
/// step's orientation: 0 for NW-SE, 1 for NE-SW.
fn square(a: Point, b: Point) -> (Point, usize) {
    let o = usize::from((b.x - a.x) != (b.y - a.y));
    (Point::new(a.x.min(b.x), a.y.min(b.y)), o)
}

/// What the board knows about one grid cell.
// DECISION: one 32-byte record per cell instead of one array per field,
// so a neighbour test touches one cache line (CORE-24).
#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    /// How many boxes, grown by the 10px clearance, cover it.
    blocked: u16,
    /// Axis bits of the lines through it.
    axes: u8,
    /// Direction + 1 a line leaves it in, towards its target.
    down: u8,
    /// Axis bits of lines two cells away (lanes, CORE-12).
    lane: u8,
    /// Lines of two nets or more in its 3x3 neighbourhood.
    near_multi: bool,
    /// One bit per `Dir` (`1 << dir`) a line steps in into or out of it
    /// (same-way neighbours, CORE-12).
    heads: u8,
    /// One bit per axis `Dir` `d`: `Board::clash(this cell, d)`, kept up
    /// to date by `Board::add`.
    clash: u8,
    /// Edge + 1 of the line through it, 0 when free.
    owner: u32,
    /// Net + 1 of the line through it.
    net: u32,
    /// Net + 1 of the first line in its 3x3 neighbourhood.
    near_net: u32,
    /// Rip-up history cost.
    history: u32,
    /// As the min corner of a square, per orientation: edge + 1 of a
    /// diagonal step through the square.
    diag: [u32; 2],
}

/// Grid occupancy for one routing pass (the prototype's `Board`).
struct Board {
    grid: Grid,
    bus: bool,
    cells: Vec<Cell>,
    /// Per edge: its net (destination box with `bus`, else the edge).
    edge_net: Vec<u32>,
    /// Per edge: the side its arrowhead arrives on; `None` when it is
    /// not routed or ends in a join (so it cannot be a trunk).
    arrives: Vec<Option<Side>>,
    /// Deployment node boundaries (P3-123); empty on every other view, so
    /// their routes are unchanged.
    fences: Vec<Fence>,
    /// Per edge: its source and destination boxes.
    ends: Vec<(BoxId, BoxId)>,
    /// Per edge: true when it has hand-added vertices (P3-308: never a
    /// trunk) or a pinned end (P4-209); all false on a view without `via`
    /// or `anchor` lines. A bundle member counts as pinned too (P4-414).
    pinned: Vec<bool>,
    /// Per cell, on a view with bundles only (empty otherwise, P4-403):
    /// 0 off every corridor, `SHUT` where no other line may pass, else
    /// the axis bit a line may cross the bundle along (P4-413).
    corridor: Vec<u8>,
    /// Per cell: the axis bits a line may not run along there, because
    /// the cell lies within one lane pitch of a boundary side parallel to
    /// that axis (P4-600, `along_bans`); empty on a view without
    /// boundaries.
    along: Vec<u8>,
}

/// P4-600: grid units a line keeps from a boundary's stroke when it runs
/// parallel to it, one lane pitch (CORE-12, 20px).
const STROKE_CLEAR: i32 = 2;
// DECISION: P4-601 (page edge) needs no rule here: `Scene::bounds` is
// everything drawn, lines included, grown by 40px, so no line is ever
// nearer the edge than that; the boundary_clear test checks it.

/// P4-600: per cell of `grid`, the axis bits (`axis_bit`) a line may not
/// run along there: `axis_bit(N)` within `STROKE_CLEAR + widen - 1` cells
/// across a vertical boundary side, between its two ends grown by
/// `widen`, `axis_bit(E)` likewise for horizontal sides; a step along an
/// axis is banned when both its cells carry that axis's bit. `widen` is
/// a bundle spine's half width (every track keeps the clearance). Empty
/// when the view has no boundaries.
// DECISION: a ban on running along the stroke, not a cost: crossing a
// boundary square or at 45° stays as cheap as before, and a step the ban
// removes was one no rule allows; every boundary keeps a free lane two
// cells inside its stroke (BOUNDARY_MARGIN_UNITS 4, box clearance 1) and
// the board reaches two cells past it, so no line of the examples loses
// its way round (measured).
fn along_bans(plan: &ViewPlan, centres: &[Point], grid: Grid, widen: i32) -> Vec<u8> {
    if plan.boundaries.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; grid.len()];
    let reach = STROKE_CLEAR - 1 + widen;
    let mut mark = |x: i32, y: i32, bit: u8| {
        if let Some(i) = grid.index(Point::new(x, y)) {
            out[i] |= bit;
        }
    };
    for k in 0..plan.boundaries.len() {
        let Some(r) = plan.boundary_rect(k, centres) else { continue };
        for sx in [r.min.x, r.max.x] {
            for x in sx - reach..=sx + reach {
                for y in r.min.y - widen..=r.max.y + widen {
                    mark(x, y, axis_bit(Dir::N));
                }
            }
        }
        for sy in [r.min.y, r.max.y] {
            for y in sy - reach..=sy + reach {
                for x in r.min.x - widen..=r.max.x + widen {
                    mark(x, y, axis_bit(Dir::E));
                }
            }
        }
    }
    out
}

/// True when the step from cell `ci` to cell `ni` heading `nd` does not
/// run along a boundary's stroke (P4-600).
fn along_ok(bans: &[u8], ci: usize, ni: usize, nd: Dir) -> bool {
    bans.is_empty() || nd.is_diagonal() || bans[ci] & bans[ni] & axis_bit(nd) == 0
}

/// `Board::corridor`: a cell between two tracks no other line may enter.
const SHUT: u8 = 0x80;

/// True when a line may step from cell `ci` to cell `ni` heading `nd`,
/// having `turned` there, as far as bundle corridors go (P4-413): it never
/// turns inside a corridor and enters one only square across it.
fn corridor_ok(board: &Board, ci: usize, ni: usize, nd: Dir, turned: bool) -> bool {
    if board.corridor.is_empty() {
        return true;
    }
    if turned && board.corridor[ci] != 0 {
        return false;
    }
    let there = board.corridor[ni];
    there == 0 || (there != SHUT && !nd.is_diagonal() && there & axis_bit(nd) != 0)
}

/// One deployment node boundary as routing sees it (P3-123).
struct Fence {
    rect: Rect,
    tag: Option<Rect>,
    members: Vec<BoxId>,
}

impl Board {
    fn new(plan: &ViewPlan, centres: &[Point], grid: Grid, history: &[u32]) -> Board {
        let n = grid.len();
        let bus = plan.options.bus;
        let mut cells: Vec<Cell> = history
            .iter()
            .map(|&h| Cell {
                history: h,
                ..Cell::default()
            })
            .collect();
        cells.resize(n, Cell::default());
        for b in &plan.boxes {
            let r = b.rect_at(centres[b.id.0 as usize]).inflate(1);
            for y in r.min.y..=r.max.y {
                for x in r.min.x..=r.max.x {
                    if let Some(i) = grid.index(Point::new(x, y)) {
                        cells[i].blocked = cells[i].blocked.saturating_add(1);
                    }
                }
            }
        }
        // P3-308: with `bus`, a line with vertices gets a net of its own
        // (past every box id), so no line ever joins it or it them.
        let boxes = u32::try_from(plan.boxes.len()).expect("box count fits u32");
        // P4-414: a bundle member never joins and is never joined.
        let mut bundled = vec![false; plan.edges.len()];
        for e in plan.bundles.iter().flatten() {
            if let Some(b) = bundled.get_mut(e.0 as usize) {
                *b = true;
            }
        }
        let edge_net = plan
            .edges
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let i = u32::try_from(i).expect("edge index fits u32");
                if bus && e.vertices.is_empty() && !anchored(e) && !bundled[i as usize] {
                    e.destination.0
                } else if bus {
                    boxes + i
                } else {
                    i
                }
            })
            .collect();
        let fences = (0..plan.boundaries.len())
            .filter(|&k| matches!(plan.boundaries[k].of, crate::view::BoundaryOf::DeploymentNode(_)))
            .filter_map(|k| {
                Some(Fence {
                    rect: plan.boundary_rect(k, centres)?,
                    tag: crate::label::boundary_tag(plan, k, centres),
                    members: plan.boundaries[k].members.clone(),
                })
            })
            .collect();
        Board {
            grid,
            bus,
            cells,
            edge_net,
            arrives: vec![None; plan.edges.len()],
            fences,
            ends: plan.edges.iter().map(|e| (e.source, e.destination)).collect(),
            pinned: plan
                .edges
                .iter()
                .zip(&bundled)
                .map(|(e, &b)| !e.vertices.is_empty() || anchored(e) || b)
                .collect(),
            corridor: Vec::new(),
            along: along_bans(plan, centres, grid, 0),
        }
    }

    /// The deployment-node charge of `edge` stepping from `c` to `n`
    /// (P3-123): `FENCE` per boundary crossed that holds neither or both
    /// ends, `FENCE_NEEDED` per one holding one end, `ON_TAG` on a tag.
    fn fence_cost(&self, edge: usize, c: Point, n: Point) -> u32 {
        let mut cost = 0;
        let Some(&(s, d)) = self.ends.get(edge) else {
            return 0;
        };
        for f in &self.fences {
            // The outline counts as inside, so a line never runs along it
            // or cuts a corner between two grid points.
            let inside = |p: Point| p.x >= f.rect.min.x && p.x <= f.rect.max.x && p.y >= f.rect.min.y && p.y <= f.rect.max.y;
            if inside(c) != inside(n) {
                let held = usize::from(f.members.contains(&s)) + usize::from(f.members.contains(&d));
                cost += if held == 1 { FENCE_NEEDED } else { FENCE };
            }
            if f.tag.is_some_and(|t| t.contains(n)) {
                cost += ON_TAG;
            }
        }
        cost
    }

    /// Lays a line's cells (source port first) on the board.
    fn add(&mut self, cells: &[Point], edge: usize, end: &LineEnd) {
        let tag = u32::try_from(edge + 1).expect("edge index fits u32");
        let net = self.edge_net[edge] + 1;
        let g = self.grid;
        for (i, &c) in cells.iter().enumerate() {
            let ci = g.index(c);
            if let Some(ci) = ci {
                self.cells[ci].owner = tag;
                self.cells[ci].net = net;
            }
            if i > 0 {
                let p = cells[i - 1];
                if let Some(d) = Segment::new(p, c).dir() {
                    if let Some(ci) = ci {
                        self.cells[ci].axes |= axis_bit(d);
                        self.set_head(c, ci, d);
                    }
                    if let Some(pi) = g.index(p) {
                        self.cells[pi].axes |= axis_bit(d);
                        self.set_head(p, pi, d);
                        self.cells[pi].down = d as u8 + 1;
                    }
                    if d.is_diagonal() {
                        let (sq, o) = square(p, c);
                        if let Some(si) = g.index(sq) {
                            self.cells[si].diag[o] = tag;
                        }
                    }
                }
            }
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(ni) = g.index(Point::new(c.x + dx, c.y + dy)) {
                        let first = self.cells[ni].near_net;
                        if first == 0 {
                            self.cells[ni].near_net = net;
                        } else if first != net {
                            self.cells[ni].near_multi = true;
                        }
                    }
                }
            }
        }
        for &c in cells {
            let Some(ci) = g.index(c) else { continue };
            let a = self.cells[ci].axes;
            for dy in [-2, 0, 2] {
                for dx in [-2, 0, 2] {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    if let Some(ni) = g.index(Point::new(c.x + dx, c.y + dy)) {
                        self.cells[ni].lane |= a;
                    }
                }
            }
        }
        self.arrives[edge] = match end {
            LineEnd::Arrow(_) if self.pinned[edge] => None,
            LineEnd::Arrow(port) => Some(port.side),
            LineEnd::Join(_) => None,
        };
    }

    /// Sets head bit `d` on cell `i` at `q`, and the clash bits it turns
    /// on: `clash(p, d)` holds when some `k` in {-3, -1, 1, 3} cells
    /// across `d` has `p + k` and `p + d + k` both heading `d`, and head
    /// bits are only ever set, so a new head at `q` can only switch on
    /// `p = q - k` (with `q + d`) and `p = q - d - k` (with `q - d`).
    // DECISION: clash bits kept per cell as lines are laid, instead of
    // eight neighbour lookups on every axis step the search tries, which
    // cost the CORE-31 view about 8% of its time (measured).
    #[allow(clippy::many_single_char_names)]
    fn set_head(&mut self, q: Point, i: usize, d: Dir) {
        let bit = 1u8 << d as u8;
        if self.cells[i].heads & bit != 0 {
            return;
        }
        self.cells[i].heads |= bit;
        if d.is_diagonal() {
            return;
        }
        let g = self.grid;
        let heads = |cells: &[Cell], q: Point| g.index(q).is_some_and(|j| cells[j].heads & bit != 0);
        let ahead = heads(&self.cells, q.step(d));
        let behind = heads(&self.cells, q.step(d.opposite()));
        let horizontal = matches!(d, Dir::E | Dir::W);
        for k in [-3, -1, 1, 3] {
            let (dx, dy) = if horizontal { (0, k) } else { (k, 0) };
            if ahead && let Some(j) = g.index(Point::new(q.x - dx, q.y - dy)) {
                self.cells[j].clash |= bit;
            }
            let b = q.step(d.opposite());
            if behind && let Some(j) = g.index(Point::new(b.x - dx, b.y - dy)) {
                self.cells[j].clash |= bit;
            }
        }
    }

    /// True when a line of another net than `net` (net + 1) is in the
    /// 3x3 neighbourhood of cell `i` (the NEAR cost).
    fn near_other(&self, i: usize, net: u32) -> bool {
        self.cells[i].near_multi || (self.cells[i].near_net != 0 && self.cells[i].near_net != net)
    }

    /// True when the axis step `p -> p.step(d)` would run 10px or 30px
    /// beside a step of another line heading `d` too: both cells 1 or 3
    /// cells across carry `d` (CORE-12). Diagonal steps never form lanes.
    // DECISION: the neighbour's two cells both carrying `d` stands for its
    // step in `d`; it may also fire where a neighbour line turns at one
    // of them, which only steers a little further away.
    fn clash(&self, p: Point, d: Dir) -> bool {
        let bit = 1u8 << d as u8;
        self.grid
            .index(p)
            .is_some_and(|i| self.cells[i].clash & bit != 0)
    }

    /// `clash` worked out from the head bits, to check the kept bits.
    #[cfg(test)]
    fn clash_from_heads(&self, p: Point, d: Dir) -> bool {
        if d.is_diagonal() {
            return false;
        }
        let g = self.grid;
        let c = p.step(d);
        let bit = 1u8 << d as u8;
        let heads = |q: Point| g.index(q).is_some_and(|i| self.cells[i].heads & bit != 0);
        let horizontal = matches!(d, Dir::E | Dir::W);
        [-3, -1, 1, 3].into_iter().any(|k| {
            let (dx, dy) = if horizontal { (0, k) } else { (k, 0) };
            heads(Point::new(p.x + dx, p.y + dy)) && heads(Point::new(c.x + dx, c.y + dy))
        })
    }

    /// True when a port at `at` whose two stub steps head `d` (out of the
    /// box when `out`, else into it) runs beside a same-way line
    /// (CORE-12).
    fn stub_clash(&self, at: Point, side: Dir, out: bool) -> bool {
        if out {
            self.clash(at, side) || self.clash(at.step(side), side)
        } else {
            let o = side.opposite();
            self.clash(step_n(at, side, 2), o) || self.clash(at.step(side), o)
        }
    }

    /// Whether a line of `net` can arrive at the port at `at` whose side
    /// faces `d`: its last step runs from three cells out to the free cell
    /// two out, heading into the port. `false` only when `astar` can take
    /// no such step.
    // Mirrors `astar`'s step rules: never into a box; onto the own net
    // only as a join (which ends the line there); onto another line only
    // square across a single straight run, never from a line cell, and
    // leaving it straight on, so the step onto it comes from four cells
    // out, which must then be clear.
    fn can_enter(&self, at: Point, d: Dir, net: u32) -> bool {
        let heading = d.opposite();
        let cell = |k: i32| self.grid.index(step_n(at, d, k)).map(|i| &self.cells[i]);
        let Some(three) = cell(3) else {
            return false;
        };
        if three.blocked > 0 {
            return false;
        }
        if three.net == 0 {
            return true;
        }
        if three.net == net || three.axes.count_ones() != 1 || three.axes & axis_bit(heading) != 0 {
            return false;
        }
        cell(4).is_some_and(|four| four.blocked == 0 && four.net == 0)
    }

    /// True when a port at `at` leaving towards `d` is free: the port and
    /// the next cell clear of other boxes, the first free cell unblocked
    /// and unused (CORE-7, CORE-9: own port per line).
    #[allow(clippy::many_single_char_names)]
    fn port_free(&self, at: Point, d: Dir) -> Option<usize> {
        let g = self.grid;
        let a = g.index(at)?;
        let b = g.index(at.step(d))?;
        let c = g.index(step_n(at, d, 2))?;
        let free = self.cells[a].blocked <= 1
            && self.cells[b].blocked <= 1
            && self.cells[c].blocked == 0
            && self.cells[c].net == 0
            && self.cells[b].net == 0
            && self.cells[a].net == 0;
        free.then_some(c)
    }
}

/// One search state, kept in an arena so a path unwinds by parent index.
#[derive(Clone, Copy)]
struct Rec {
    cell: u32,
    dir: Dir,
    /// 2 after a straight step (may turn), 1 right after a turn.
    run: u8,
    /// Own steps so far, capped at `MIN_JOIN`; `JOINED` once on a trunk.
    steps: u8,
    cost: u32,
    /// Arena index of the parent, `u32::MAX` for a start state.
    parent: u32,
    /// For a start state: index of its source spot.
    spot: u32,
}

/// What one search found.
struct Found {
    cells: Vec<Point>,
    source: Port,
    end: LineEnd,
    /// Cells where it crosses another line, each once.
    crossed: Vec<usize>,
}

/// A multiplicative hasher for the sparse state keys: one `u64` per key,
/// so `SipHash`'s cost buys nothing here (std only, no `rustc-hash`).
#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(8) ^ u64::from(b)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (n ^ (n >> 29)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// A monotone radix heap of `(key, arena index)`: integer keys, popped
/// smallest first. A key below the last popped one is raised to it (as
/// pathmax does for an inconsistent heuristic), which keeps the heap
/// monotone.
// DECISION: a radix heap instead of `BinaryHeap`: the search pushes about
// three states per pop into a queue of 10^5 entries, and that was the
// router's main cost (CORE-24). Equal keys pop in the heap's own fixed
// order, so the output is still the same on every run (CORE-25).
struct RadixHeap {
    /// Equal keys pop last in, first out (a preview's A*: depth first
    /// among equal priorities); else first in, first out.
    lifo: bool,
    last: u32,
    len: usize,
    /// Next entry of bucket 0 to pop: equal keys pop first in, first out.
    head: usize,
    buckets: [Vec<(u32, u32)>; 33],
}

impl Default for RadixHeap {
    fn default() -> Self {
        RadixHeap {
            lifo: false,
            last: 0,
            len: 0,
            head: 0,
            buckets: std::array::from_fn(|_| Vec::new()),
        }
    }
}

impl RadixHeap {
    fn bucket(&self, key: u32) -> usize {
        (32 - (key ^ self.last).leading_zeros()) as usize
    }

    fn clear(&mut self) {
        for b in &mut self.buckets {
            b.clear();
        }
        self.last = 0;
        self.len = 0;
        self.head = 0;
    }

    fn push(&mut self, key: u32, value: u32) {
        let key = key.max(self.last);
        let b = self.bucket(key);
        self.buckets[b].push((key, value));
        self.len += 1;
    }

    fn pop(&mut self) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if self.lifo {
            if self.buckets[0].is_empty() {
                self.refill()?;
            }
            self.len -= 1;
            return self.buckets[0].pop().map(|e| e.1);
        }
        if self.head == self.buckets[0].len() {
            self.buckets[0].clear();
            self.head = 0;
            let i = (1..33).find(|&i| !self.buckets[i].is_empty())?;
            let moved = std::mem::take(&mut self.buckets[i]);
            self.last = moved.iter().map(|e| e.0).min()?;
            for &(k, v) in &moved {
                let b = self.bucket(k);
                self.buckets[b].push((k, v));
            }
            // Hand the emptied allocation back for reuse.
            let mut spare = moved;
            spare.clear();
            if self.buckets[i].is_empty() {
                self.buckets[i] = spare;
            }
        }
        self.len -= 1;
        let v = self.buckets[0].get(self.head).map(|e| e.1);
        self.head += 1;
        v
    }

    /// Moves the smallest non-empty bucket's entries down (LIFO mode,
    /// bucket 0 empty).
    fn refill(&mut self) -> Option<()> {
        let i = (1..33).find(|&i| !self.buckets[i].is_empty())?;
        let moved = std::mem::take(&mut self.buckets[i]);
        self.last = moved.iter().map(|e| e.0).min()?;
        for &(k, v) in &moved {
            let b = self.bucket(k);
            self.buckets[b].push((k, v));
        }
        let mut spare = moved;
        spare.clear();
        if self.buckets[i].is_empty() {
            self.buckets[i] = spare;
        }
        Some(())
    }
}

/// Slots of `DialQueue`'s ring: more than any one step's cost, so every
/// key queued lies within one turn of the ring from the last popped key.
const DIAL_SLOTS: usize = 1 << 17;

/// A bucket queue (Dial's) of `(key, value)` for `cost_from_dir`: integer
/// keys popped smallest first, one ring slot per key value, a bit per slot
/// to find the next one, values chained per slot (equal keys pop last in,
/// first out). A key `DIAL_SLOTS` or more above the last popped one waits
/// in `far` until the ring is empty. Keys below the last popped one are
/// raised to it, as `RadixHeap` does.
// DECISION: Dial's queue instead of `RadixHeap` for the preview's flood:
// its step costs are bounded integers, so a push is two stores and a bit,
// a pop a bit scan; the radix heap's redistribution was 178 instructions a
// pop, 30% of a C3-entry drag's preview (callgrind). The full build keeps
// `RadixHeap` everywhere, so its pop order and routes are unchanged.
#[derive(Default)]
struct DialQueue {
    last: u32,
    len: usize,
    /// Per slot: index in `items` of the newest entry, `u32::MAX` if none.
    head: Vec<u32>,
    /// One bit per slot with an entry.
    bits: Vec<u64>,
    /// `(value, index of the next entry of the slot)`.
    items: Vec<(u32, u32)>,
    /// Entries too far ahead for the ring.
    far: Vec<(u32, u32)>,
}

impl DialQueue {
    fn clear(&mut self) {
        if self.head.is_empty() {
            self.head = vec![u32::MAX; DIAL_SLOTS];
            self.bits = vec![0; DIAL_SLOTS / 64];
        }
        if self.len > 0 {
            for (w, word) in self.bits.iter_mut().enumerate() {
                let mut b = *word;
                while b != 0 {
                    let i = w * 64 + b.trailing_zeros() as usize;
                    self.head[i] = u32::MAX;
                    b &= b - 1;
                }
                *word = 0;
            }
        }
        self.items.clear();
        self.far.clear();
        self.last = 0;
        self.len = 0;
    }

    fn push(&mut self, key: u32, value: u32) {
        let key = key.max(self.last);
        self.len += 1;
        if (key - self.last) as usize >= DIAL_SLOTS {
            self.far.push((key, value));
            return;
        }
        let slot = key as usize & (DIAL_SLOTS - 1);
        let idx = u32::try_from(self.items.len()).expect("queue fits u32");
        self.items.push((value, self.head[slot]));
        self.head[slot] = idx;
        self.bits[slot / 64] |= 1 << (slot % 64);
    }

    fn pop(&mut self) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        let ring = self.len - self.far.len();
        if ring == 0 {
            // Only far entries: restart the ring at the least of them.
            self.last = self.far.iter().map(|e| e.0).min()?;
            let far = std::mem::take(&mut self.far);
            self.len -= far.len();
            for &(k, v) in &far {
                self.push(k, v);
            }
            self.far = far;
            self.far.retain(|&(k, _)| (k - self.last) as usize >= DIAL_SLOTS);
        }
        // The next slot with an entry, from the last key's slot on, round
        // the ring.
        let start = self.last as usize & (DIAL_SLOTS - 1);
        let words = DIAL_SLOTS / 64;
        let mut w = start / 64;
        let mut word = self.bits[w] & (!0u64 << (start % 64));
        let mut turned = 0;
        while word == 0 {
            w = (w + 1) % words;
            turned += 1;
            word = self.bits[w];
            if turned > words {
                return None;
            }
        }
        let slot = w * 64 + word.trailing_zeros() as usize;
        let ahead = (slot + DIAL_SLOTS - start) % DIAL_SLOTS;
        self.last += u32::try_from(ahead).expect("slot fits u32");
        let idx = self.head[slot] as usize;
        let (value, next) = self.items[idx];
        self.head[slot] = next;
        if next == u32::MAX {
            self.bits[slot / 64] &= !(1 << (slot % 64));
        }
        self.len -= 1;
        Some(value)
    }
}

/// Search buffers reused across the searches of one call.
struct Scratch {
    recs: Vec<Rec>,
    /// Best cost per (cell, dir, run) for states that no longer track
    /// their step count (`steps == MIN_JOIN`, or always without bus),
    /// `u32::MAX` when none; `restart` resets the entries its states set.
    // DECISION: the cost alone, not `[stamp, arena index, cost]`: a state's
    // recorded cost only ever drops, so the latest record of a state is
    // the one whose cost equals it (the same test as the index), and the
    // 16 states of a cell fit one cache line (CORE-24, CORE-31).
    dense: Vec<u32>,
    /// Cells of the board this scratch serves.
    cells: usize,
    /// Best cost for the other states (near a source port, bus on).
    // DECISION: a HashMap here is only looked up, never iterated, so its
    // random hashing cannot change any output (CORE-25).
    sparse: HashMap<u64, u32, BuildHasherDefault<KeyHasher>>,
    goal: Vec<u32>,
    goal_gen: Vec<u32>,
    /// Per cell: a lower bound of the cost from it to a target (see
    /// `cost_from`), valid when `bound_gen` is `flood`.
    bound: Vec<u32>,
    bound_gen: Vec<u32>,
    /// Source start cells `cost_from` has still to settle.
    source_gen: Vec<u32>,
    /// Stamps `goal_gen` per search.
    generation: u32,
    /// Stamps `bound_gen` and `source_gen`: per search, and per
    /// `cost_from` of a preview (`fresh_flood`).
    flood: u32,
    heap: RadixHeap,
    /// The queue of `cost_from`.
    cells_heap: RadixHeap,
    /// The cells a search may step on: the whole board, or a large view's
    /// window.
    window: Rect,
    /// A* states popped by every search of this scratch so far (a
    /// preview's rip-up budget, `Effort::pops`).
    spent: u64,
    /// Per (cell, heading) state, a preview's: `[stamp, cost]`, the cost
    /// from the state to a goal state (`cost_from_dir`), valid when the
    /// stamp is `flood`. Sized on first use.
    dbound: Vec<[u32; 2]>,
    /// The queue of `cost_from_dir`.
    dheap: DialQueue,
    /// Per cell: its grid coordinates (`cost_from_dir`), sized on first use.
    coords: Vec<(u16, u16)>,
}

impl Scratch {
    fn new(cells: usize) -> Scratch {
        Scratch {
            recs: Vec::new(),
            // DECISION: sized by the first search (`restart`): a preview
            // whose lines all rejoin runs no `astar`, and filling
            // 64 bytes a cell cost it about 1 ms (measured).
            dense: Vec::new(),
            cells,
            sparse: HashMap::default(),
            goal: vec![0; cells],
            goal_gen: vec![0; cells],
            bound: vec![0; cells],
            bound_gen: vec![0; cells],
            source_gen: vec![0; cells],
            generation: 0,
            flood: 0,
            heap: RadixHeap::default(),
            cells_heap: RadixHeap::default(),
            window: Rect::default(),
            spent: 0,
            dbound: Vec::new(),
            dheap: DialQueue::default(),
            coords: Vec::new(),
        }
    }

    fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.goal_gen.fill(0);
            self.generation = 1;
        }
        self.fresh_flood();
        self.restart();
    }

    /// Forgets every `cost_from` bound and settled source, keeping the
    /// goals: the next `cost_from` floods from scratch.
    fn fresh_flood(&mut self) {
        self.flood = self.flood.wrapping_add(1);
        if self.flood == 0 {
            self.bound_gen.fill(0);
            self.source_gen.fill(0);
            self.flood = 1;
        }
    }

    /// Forgets every search state, keeping the goals.
    fn restart(&mut self) {
        if self.dense.is_empty() {
            self.dense = vec![u32::MAX; self.cells * 16];
        }
        // Every recorded state's dense entry (also those kept in `sparse`,
        // whose entry is then already `u32::MAX`).
        for r in &self.recs {
            self.dense[Self::dense_key(r.cell, r.dir, r.run)] = u32::MAX;
        }
        self.recs.clear();
        self.sparse.clear();
        self.heap.clear();
    }

    /// Runs a Dijkstra search backwards from the target cells `seeds`
    /// over cells, each step charged what `astar` charges for it less the
    /// BEND and CLASH costs and the turn and crossing rules, so
    /// `bound[cell]` is a lower bound of the cost from `cell` to a target
    /// (consistent, as each step costs at least as much in `astar`). It
    /// stops once every cell of `sources` is settled and returns the cost
    /// reached then: no cell left unsettled is cheaper. Also false when the
    /// flood ran out without settling any source: every step `astar`
    /// may take is one of these steps (reversed) or is barred, so then no
    /// source reaches a target and `astar` finds no path.
    // DECISION: the bound is per cell, not per (cell, direction): 8 times
    // fewer states, and the turn costs it leaves out are small next to
    // CROSS and NEAR, which are what made the octile bound flood (CORE-31).
    #[allow(clippy::many_single_char_names, clippy::too_many_arguments, clippy::similar_names)]
    fn cost_from(&mut self, board: &Board, seeds: &[usize], sources: &[usize], net: u32, strict: bool, stop: &AtomicBool) -> Result<(u32, bool), Stopped> {
        let g = board.grid;
        let stamp = self.flood;
        self.cells_heap.clear();
        for &i in seeds {
            self.bound_gen[i] = stamp;
            self.bound[i] = 0;
            self.cells_heap.push(0, u32::try_from(i).expect("cell fits u32"));
        }
        let mut left = 0usize;
        for &i in sources {
            if self.source_gen[i] != stamp {
                self.source_gen[i] = stamp;
                left += 1;
            }
        }
        let wanted = left;
        let mut reached = 0;
        let mut popped = 0u32;
        while left > 0 {
            let Some(ni) = self.cells_heap.pop() else { break };
            popped = popped.wrapping_add(1);
            tick(popped, stop)?;
            let key = self.cells_heap.last;
            let ni = ni as usize;
            if key > self.bound[ni] {
                continue;
            }
            reached = key;
            if self.source_gen[ni] == stamp {
                self.source_gen[ni] = 0;
                left -= 1;
            }
            // Every step into `ni` costs, in `astar`, at least: the step,
            // then CROSS onto another line, else NEAR beside one, else
            // the lane discount, then the history; joining a trunk (own
            // net) costs the step only.
            let cell = &board.cells[ni];
            let own = cell.net == net;
            let other = cell.net != 0 && !own;
            let near = !own && !other && board.near_other(ni, net);
            // DECISION: preview only, as it changes the bound and with it
            // which of equal-cost paths A* finds; the optimal cost stays
            // (a lower bound still). Ties cannot help here: in the 100k+
            // pop searches all but a handful of pops lie below the optimal
            // cost (measured), the gap being what the per-cell bound misses.
            // `strict` (a preview): leave out the steps `astar` never takes,
            // so the bound is still a lower bound and nearer the cost: onto
            // another line where it bends or crosses, or along it; from one
            // line onto another; on through the own net (a join ends the
            // line); diagonally through the own net's diagonal.
            if strict && ((other && cell.axes.count_ones() != 1) || (own && self.bound[ni] != 0)) {
                continue;
            }
            let n = g.point(ni);
            for d in Dir::ALL {
                let c = n.step(d.opposite());
                if !self.window.contains(c) {
                    continue;
                }
                let Some(ci) = g.index(c) else { continue };
                // P4-600: `astar` never takes a step along a stroke, so the
                // bound leaves it out too (still a lower bound, tighter).
                if board.cells[ci].blocked > 0 || !along_ok(&board.along, ci, ni, d) {
                    continue;
                }
                if strict {
                    let from = &board.cells[ci];
                    if other && (cell.axes & axis_bit(d) != 0 || (from.net != 0 && from.net != net)) {
                        continue;
                    }
                    if d.is_diagonal() {
                        let (sq, o) = square(c, n);
                        if g.index(sq).is_some_and(|si| {
                            let t = board.cells[si].diag[1 - o];
                            t != 0 && board.edge_net[t as usize - 1] + 1 == net
                        }) {
                            continue;
                        }
                    }
                }
                let mut step = if d.is_diagonal() {
                    STEP_DIAGONAL
                } else {
                    STEP_STRAIGHT
                };
                if !own {
                    // As in `astar`, the lane discount also takes off
                    // part of a diagonal crossing's CROSS.
                    if d.is_diagonal() {
                        let (sq, o) = square(c, n);
                        if g.index(sq).is_some_and(|si| board.cells[si].diag[1 - o] != 0) {
                            step += CROSS;
                        }
                    }
                    if other {
                        step += CROSS;
                    } else if near {
                        step += NEAR;
                    } else if cell.lane & axis_bit(d) != 0 {
                        step = step * LANE_NUM / LANE_DEN;
                    }
                    step += cell.history;
                }
                let cost = key.saturating_add(step);
                if self.bound_gen[ci] == stamp && self.bound[ci] <= cost {
                    continue;
                }
                self.bound_gen[ci] = stamp;
                self.bound[ci] = cost;
                self.cells_heap.push(cost, u32::try_from(ci).expect("cell fits u32"));
            }
        }
        #[cfg(test)]
        probe::work(popped % STOP_EVERY);
        #[cfg(test)]
        probe::with(|s| s.cf_cells += u64::from(popped));
        Ok((reached, wanted == 0 || left < wanted))
    }

    /// A preview's bound (EDIT-25): a Dijkstra search backwards from the
    /// goal states `seeds` (cell, heading into its port; with bus every
    /// heading at a trunk cell `trunks`) over (cell, heading) states,
    /// each step charged exactly what `astar` charges for it, turns,
    /// crossings and clashes included, only the rule that a turn follows a
    /// straight step left out. So `dbound` is a lower bound of the cost
    /// from an `astar` state to a goal, consistent, and nearer the true cost
    /// than `cost_from`'s per-cell one, which leaves out the turn and
    /// crossing rules and so under-counts whole crossings. It stops once
    /// every state of `sources` is settled and returns the cost reached
    /// then, and whether any source was settled: when none was, no
    /// `astar` path exists (every `astar` step is a step here).
    // DECISION: per (cell, heading) for the preview only: on C3-entry the
    // per-cell bound was 2-6 crossings (30k-96k) below the found cost on
    // the 200k-350k pop searches, which then popped most of their window.
    #[allow(clippy::many_single_char_names, clippy::too_many_arguments, clippy::similar_names, clippy::too_many_lines)]
    fn cost_from_dir(&mut self, board: &Board, edge: usize, seeds: &[(usize, Dir)], trunks: &[usize], sources: &[(usize, Dir, u32)], history: bool, stop: &AtomicBool) -> Result<(u32, bool), Stopped> {
        #[cfg(test)]
        let t0 = std::time::Instant::now();
        let g = board.grid;
        let net = board.edge_net[edge] + 1;
        let stamp = self.flood;
        if self.dbound.len() != g.len() * 8 {
            self.dbound = vec![[0, 0]; g.len() * 8];
        }
        let window = self.window;
        self.dheap.clear();
        let key = |cell: usize, d: Dir| cell * 8 + d as usize;
        let set = |dbound: &mut Vec<[u32; 2]>, heap: &mut DialQueue, k: usize, cost: u32| {
            let e = &mut dbound[k];
            if e[0] == stamp && e[1] <= cost {
                return;
            }
            *e = [stamp, cost];
            heap.push(cost, u32::try_from(k).expect("state fits u32"));
        };
        for &(c, d) in seeds {
            set(&mut self.dbound, &mut self.dheap, key(c, d), 0);
        }
        for &c in trunks {
            for d in Dir::ALL {
                set(&mut self.dbound, &mut self.dheap, key(c, d), 0);
            }
        }
        // The best start found so far, its start cost included: once the
        // flood has passed it less the cheapest start cost, no start left
        // can beat it, and every state `astar` would pop on the way to it
        // has its bound (the stop rule of a bidirectional search).
        let least_start = sources.iter().map(|s| s.2).min().unwrap_or(0);
        for &(c, _, _) in sources {
            self.source_gen[c] = stamp;
        }
        let mut best = u32::MAX;
        let mut settled_any = false;
        let mut reached = 0;
        let mut popped = 0u32;
        // DECISION: the hot loop works on cell indices (one division per
        // state for the window test, offsets per heading for the cell
        // behind and the square a diagonal crosses) and skips the corridor
        // and stroke tests on a board without them: the same costs as
        // the point-based tests, about twice as fast (measured).
        let w = g.w;
        let step_of: [(i32, i32); 8] = Dir::ALL.map(|d| {
            let p = Point::new(0, 0).step(d);
            (p.x, p.y)
        });
        let plain_board = board.corridor.is_empty() && board.along.is_empty();
        let no_fences = board.fences.is_empty();
        let cells: &[Cell] = &board.cells;
        let n_cells = cells.len();
        if self.coords.len() != n_cells {
            self.coords = (0..n_cells)
                .map(|i| {
                    let i = i32::try_from(i).expect("cell fits i32");
                    (u16::try_from(i % w).expect("grid fits u16"), u16::try_from(i / w).expect("grid fits u16"))
                })
                .collect();
        }
        // The window in grid coordinates, clipped to the grid.
        let (wx0, wx1) = ((window.min.x - g.x0).max(0), (window.max.x - g.x0).min(w - 1));
        let (wy0, wy1) = ((window.min.y - g.y0).max(0), (window.max.y - g.y0).min(g.h - 1));
        let dbound: &mut [[u32; 2]] = &mut self.dbound;
        let coords: &[(u16, u16)] = &self.coords;
        let source_gen: &[u32] = &self.source_gen;
        let heap = &mut self.dheap;
        assert!(dbound.len() == n_cells * 8 && coords.len() == n_cells && source_gen.len() >= n_cells, "scratch sized for the board");
        // DECISION: unchecked indexing in this loop (owner approval for
        // `unsafe` in the hot path): every index is a state or cell index
        // proven in range where it is made (see each SAFETY note); the
        // bounds checks were a sixth of the loop's instructions.
        #[allow(unsafe_code)]
        let push = |dbound: &mut [[u32; 2]], heap: &mut DialQueue, k: usize, cost: u32| {
            // SAFETY: `k = c * 8 + d` with `c < n_cells` (a cell inside the
            // grid, checked where `c` is made) and `d < 8`, and
            // `dbound.len() == n_cells * 8` (asserted above).
            let e = unsafe { dbound.get_unchecked_mut(k) };
            if e[0] == stamp && e[1] <= cost {
                return;
            }
            *e = [stamp, cost];
            heap.push(cost, u32::try_from(k).expect("state fits u32"));
        };
        while !sources.is_empty() {
            let Some(k) = heap.pop() else { break };
            popped = popped.wrapping_add(1);
            tick(popped, stop)?;
            let kv = heap.last;
            let k = k as usize;
            // SAFETY: `k` was pushed by `push`, so `k < dbound.len()`.
            #[allow(unsafe_code)]
            let here = unsafe { *dbound.get_unchecked(k) };
            if kv > here[1] {
                #[cfg(test)]
                probe::with(|s| s.dir_stale += 1);
                continue;
            }
            if best != u32::MAX && kv.saturating_add(least_start) >= best {
                break;
            }
            reached = kv;
            let (ni, nd) = (k / 8, Dir::ALL[k % 8]);
            // SAFETY: `ni = k / 8 < n_cells`, and `source_gen` and `coords`
            // hold at least `n_cells` entries (asserted above).
            #[allow(unsafe_code)]
            let (src, (nx, ny)) = unsafe { (*source_gen.get_unchecked(ni), *coords.get_unchecked(ni)) };
            if src == stamp {
                for &(sc, sd, st) in sources {
                    if (sc, sd) == (ni, nd) {
                        best = best.min(kv.saturating_add(st));
                        settled_any = true;
                    }
                }
            }
            let (nx, ny) = (i32::from(nx), i32::from(ny));
            let (dx, dy) = step_of[nd as usize];
            let (cx, cy) = (nx - dx, ny - dy);
            if cx < wx0 || cx > wx1 || cy < wy0 || cy > wy1 {
                continue;
            }
            let ci = usize::try_from(cy * w + cx).expect("in the grid");
            // SAFETY: `0 <= cx < w` and `0 <= cy < h` (the clipped window),
            // so `ci < n_cells`; `ni < n_cells` as above.
            #[allow(unsafe_code)]
            let (cc, nc) = unsafe { (cells.get_unchecked(ci), cells.get_unchecked(ni)) };
            if cc.blocked > 0 || nc.blocked > 0 || (!plain_board && !along_ok(&board.along, ci, ni, nd)) {
                continue;
            }
            let on_other = cc.net != 0 && cc.net != net;
            // The parts of the step's cost that do not depend on the turn.
            let diagonal = dx != 0 && dy != 0;
            let mut base = if diagonal { STEP_DIAGONAL } else { STEP_STRAIGHT };
            if cc.clash & (1 << nd as u8) != 0 {
                base += CLASH;
            }
            if !no_fences {
                base += board.fence_cost(edge, Point::new(cx + g.x0, cy + g.y0), Point::new(nx + g.x0, ny + g.y0));
            }
            if diagonal {
                // The square's min corner, and its orientation (`square`).
                let si = usize::try_from(cx.min(nx) + cy.min(ny) * w).expect("in the grid");
                let o = usize::from(dx != dy);
                // SAFETY: the square's corner lies between `c` and `n`, both
                // in the grid, so `si < n_cells`.
                #[allow(unsafe_code)]
                let other = unsafe { cells.get_unchecked(si) }.diag[1 - o];
                if other != 0 {
                    if board.edge_net[other as usize - 1] + 1 == net {
                        continue;
                    }
                    base += CROSS;
                }
            }
            // How the cell is entered changes the step, as in `astar`:
            // 0 join, 1 cross, 2 near, 3 lane, 4 plain.
            let into: u8 = if nc.net != 0 {
                if nc.net == net {
                    // Own net: only a join onto a trunk cell (a seed).
                    if !board.bus || on_other || here[1] != 0 {
                        continue;
                    }
                    0
                } else {
                    if on_other || nc.axes.count_ones() != 1 || nc.axes & axis_bit(nd) != 0 {
                        continue;
                    }
                    1
                }
            } else if nc.near_multi || (nc.near_net != 0 && nc.near_net != net) {
                2
            } else if nc.lane & axis_bit(nd) != 0 {
                3
            } else {
                4
            };
            let hist = if history { nc.history } else { 0 };
            for by in [0i8, 1, -1] {
                let turned = by != 0;
                if turned && (on_other || into == 0) {
                    continue;
                }
                if !plain_board && !corridor_ok(board, ci, ni, nd, turned) {
                    continue;
                }
                // The state at `c` that steps `nd` after turning `by`.
                let d = turn(nd, -by);
                let mut step = base + if turned { BEND } else { 0 };
                match into {
                    0 => {}
                    1 => step += CROSS + hist,
                    2 => step += NEAR + hist,
                    3 => step = step * LANE_NUM / LANE_DEN + hist,
                    _ => step += hist,
                }
                push(dbound, heap, ci * 8 + d as usize, kv.saturating_add(step));
            }
        }
        // DECISION: a third of the flood's states count against the
        // preview's rip-up budget (`Effort::pops`, A* pops): a flood state
        // costs about a third of an A* pop (measured), and with this bound
        // the floods are most of a search's work.
        self.spent += u64::from(popped / 3);
        #[cfg(test)]
        probe::work(popped % STOP_EVERY);
        #[cfg(test)]
        probe::with(|s| {
            s.cf_cells += u64::from(popped);
            s.dir_states += u64::from(popped);
            s.dir_time += t0.elapsed();
        });
        Ok((reached, settled_any || sources.is_empty()))
    }

    /// The best cost recorded for this state, if any.
    #[allow(clippy::too_many_arguments)]
    fn best(&self, cell: u32, dir: Dir, run: u8, steps: u8, bus: bool) -> Option<u32> {
        if !bus || steps == MIN_JOIN {
            let cost = self.dense[Self::dense_key(cell, dir, run)];
            (cost != u32::MAX).then_some(cost)
        } else {
            self.sparse
                .get(&Self::sparse_key(cell, dir, run, steps))
                .copied()
        }
    }

    fn set_best(&mut self, r: &Rec, bus: bool) {
        if !bus || r.steps == MIN_JOIN {
            let k = Self::dense_key(r.cell, r.dir, r.run);
            self.dense[k] = r.cost;
        } else {
            self.sparse
                .insert(Self::sparse_key(r.cell, r.dir, r.run, r.steps), r.cost);
        }
    }

    fn dense_key(cell: u32, dir: Dir, run: u8) -> usize {
        (cell as usize * 8 + dir as usize) * 2 + usize::from(run - 1)
    }

    fn sparse_key(cell: u32, dir: Dir, run: u8, steps: u8) -> u64 {
        (u64::from(cell) << 24) | ((dir as u64) << 16) | (u64::from(run) << 8) | u64::from(steps)
    }

    /// Records `r` when it beats the best cost of its state, and queues
    /// it with priority `cost + h`.
    fn push(&mut self, r: Rec, h: u32, tie: &mut u32, bus: bool) {
        if let Some(cost) = self.best(r.cell, r.dir, r.run, r.steps, bus)
            && cost <= r.cost
        {
            return;
        }
        let idx = u32::try_from(self.recs.len()).expect("arena fits u32");
        self.recs.push(r);
        self.set_best(&r, bus);
        self.heap.push(r.cost.saturating_add(h), idx);
        *tie += 1;
    }
}

/// Everything a search needs to know about the view.
struct Ctx<'a> {
    plan: &'a ViewPlan,
    centres: &'a [Point],
    /// The bundle corridors of the view (P4-413): cell, `Board::corridor`
    /// value; empty without bundles.
    corridor: &'a [(Point, u8)],
    /// A drag preview's search (`reroute`), which may take the shortcuts
    /// a full build does not (EDIT-25).
    preview: bool,
    /// A drag preview's routes before the drag (`rejoin`).
    previous: Option<&'a Routes>,
}

impl Ctx<'_> {
    /// `Board::new` with this view's corridors stamped on it.
    fn board(&self, grid: Grid, history: &[u32]) -> Board {
        let mut board = Board::new(self.plan, self.centres, grid, history);
        if !self.corridor.is_empty() {
            board.corridor = vec![0; grid.len()];
            for &(p, v) in self.corridor {
                if let Some(i) = grid.index(p) {
                    board.corridor[i] = if board.corridor[i] == 0 { v } else { SHUT };
                }
            }
        }
        board
    }
}

/// Routes `edge` on `board`: the cheapest octilinear path from a free
/// port of its source to a free port of its destination, or with `bus`
/// onto a trunk into the same box (the prototype's `route`).
#[allow(clippy::too_many_lines)]
// DECISION: one function for the A* loop, as in the prototype, so the
// cost rules read in one place; splitting it would scatter them.
fn search(ctx: &Ctx<'_>, board: &Board, sc: &mut Scratch, edge: usize, stop: &AtomicBool) -> Result<Option<Found>, Stopped> {
    let e = &ctx.plan.edges[edge];
    if e.source == e.destination {
        return Ok(None);
    }
    #[cfg(test)]
    probe::with(|st| st.reason = if ctx.preview { "search: other" } else { "full build" });
    if ctx.preview
        && let Some(f) = rejoin(ctx, board, sc, edge, stop)?
    {
        return Ok(Some(f));
    }
    let g = board.grid;
    let bus = board.bus;
    let net = board.edge_net[edge] + 1;
    let sbox = &ctx.plan.boxes[e.source.0 as usize];
    let dbox = &ctx.plan.boxes[e.destination.0 as usize];
    let drect = dbox.rect_at(ctx.centres[e.destination.0 as usize]);
    sc.reset();

    // Goals: the first free cell outside each free destination port.
    let dspots = end_spots(dbox, ctx.centres[e.destination.0 as usize], e.destination_anchor);
    let mut targets: Vec<usize> = Vec::new();
    // DECISION: a destination port whose arrival stub runs 10px or 30px
    // beside a same-way line is left out while any other port is free
    // (CORE-12); goals carry no cost, so this is a filter, not a charge.
    let sspots = end_spots(sbox, ctx.centres[e.source.0 as usize], e.source_anchor);
    let sources: Vec<usize> = sspots
        .iter()
        .filter_map(|s| board.port_free(s.port.at, side_dir(s.port.side)))
        .collect();
    // DECISION: a preview leaves out a port no line can enter: the goal
    // is reached heading into the port, so from the cell just beyond the
    // free one, and when a box covers that cell (another box two cells
    // off the side, as a dragged box often is) only a start on the goal
    // itself could reach it. A* proved that only after popping every
    // state it could reach, 150k-1.5M pops a search (EDIT-25); a goal
    // that cannot be reached changes no answer. The full build keeps
    // every port, so its work and lines are as before.
    let enterable = |at: Point, d: Dir, c: usize| -> bool { !ctx.preview || sources.contains(&c) || board.can_enter(at, d, net) };
    let free: Vec<(usize, usize, bool)> = dspots
        .iter()
        .enumerate()
        .filter_map(|(k, s)| {
            let d = side_dir(s.port.side);
            let c = board.port_free(s.port.at, d)?;
            enterable(s.port.at, d, c).then(|| (k, c, board.stub_clash(s.port.at, d, false)))
        })
        .collect();
    let any_clean = free.iter().any(|f| !f.2);
    for &(k, c, clashes) in &free {
        if clashes && any_clean {
            continue;
        }
        sc.goal[c] = u32::try_from(k).expect("spot index fits u32");
        sc.goal_gen[c] = sc.generation;
        targets.push(c);
    }
    // Trunks: cells of arrow-ended lines into the same box, on a straight
    // stretch, beyond the side they arrive on (CORE-8: same box, same side).
    let trunk_at = |i: usize| -> Option<usize> {
        if board.cells[i].net != net || board.cells[i].down == 0 || board.cells[i].axes.count_ones() != 1 {
            return None;
        }
        let t = board.cells[i].owner.checked_sub(1)? as usize;
        let side = board.arrives[t]?;
        let p = g.point(i);
        let beyond = match side {
            Side::N => p.y < drect.min.y,
            Side::S => p.y > drect.max.y,
            Side::W => p.x < drect.min.x,
            Side::E => p.x > drect.max.x,
        };
        beyond.then_some(t)
    };
    if bus && !anchored(e) && !board.pinned[edge] {
        for i in 0..g.len() {
            if trunk_at(i).is_some() {
                targets.push(i);
            }
        }
    }
    if targets.is_empty() {
        return Ok(None);
    }
    sc.heap.lifo = ctx.preview;
    #[cfg(test)]
    if probe::variant() & 8 != 0 {
        sc.heap.lifo = false;
    }
    let (mut lo, mut hi) = (g.point(targets[0]), g.point(targets[0]));
    for &i in &targets {
        let p = g.point(i);
        lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    // DECISION: a search that runs past POP_LIMIT states is usually one
    // whose targets are walled in by other lines, where the CROSS and
    // NEAR costs make A* exhaust everything cheaper first. It restarts
    // with `cost_from`'s per-cell cost bound as its heuristic: a lower
    // bound, so the flood is gone (CORE-24, CORE-31). On `example/` it
    // gives the same lines as the former crossing-count bound (the SVG
    // bytes are unchanged, measured).
    let whole = Rect::from_corners(
        Point::new(g.x0, g.y0),
        Point::new(g.x0 + g.w - 1, g.y0 + g.h - 1),
    );
    let large = ctx.plan.edges.len() > LARGE_EDGES;
    let mut windows = vec![whole];
    if large {
        let s = sbox.rect_at(ctx.centres[e.source.0 as usize]);
        let w = Rect::from_corners(
            Point::new(s.min.x.min(drect.min.x), s.min.y.min(drect.min.y)),
            Point::new(s.max.x.max(drect.max.x), s.max.y.max(drect.max.y)),
        )
        .inflate(WINDOW_MARGIN);
        if w != whole {
            windows.insert(0, w);
        }
    }
    let mut flooded = false;
    for window in windows {
        sc.window = window;
        // A large view skips the octile attempt: on it most searches run
        // past POP_LIMIT and only then start the one that counts.
        for attempt in usize::from(large)..2 {
            let mut bound = Bound::Octile;
            if attempt == 1 {
                sc.restart();
                // DECISION: a preview floods afresh for each window. The
                // full build's second flood (the whole board, after the
                // window found no path) starts from the window's bounds
                // and settled sources: it settles nothing new and its A*
                // runs with a bound of 0, a Dijkstra over the board, the
                // 100k-1.5M pop searches; the full build keeps it as it
                // is, so its lines stay the same bytes.
                // The backward no-path proof (`no_way_back`); not with bus,
                // where a join is a goal anywhere on a trunk.
                let no_way = |sc: &Scratch| -> Result<bool, Stopped> {
                    if bus {
                        return Ok(false);
                    }
                    let starts: Vec<(usize, Dir)> = sspots
                        .iter()
                        .filter_map(|s| Some((board.port_free(s.port.at, side_dir(s.port.side))?, side_dir(s.port.side))))
                        .collect();
                    let goals: Vec<(usize, Dir)> = targets
                        .iter()
                        .map(|&t| (t, side_dir(dspots[sc.goal[t] as usize].port.side).opposite()))
                        .collect();
                    no_way_back(board, edge, window, &starts, &goals, stop)
                };
                if ctx.preview {
                    sc.fresh_flood();
                    // DECISION: a preview's cost-bounded search first tries
                    // to prove there is no path by walking A*'s steps
                    // backwards from the goals, at most `BACK_BUDGET`
                    // states: a port that no line can enter square (turns
                    // are 45° at most) is a pocket of a few states
                    // backwards, while forwards A* pops every state it can
                    // reach, up to 1.3M on the whole board, before it gives
                    // up (EDIT-25). The same answer, `NoPath`.
                    if no_way(sc)? {
                        break;
                    }
                }
                #[cfg(test)]
                let t0 = std::time::Instant::now();
                #[cfg(test)]
                let dir_bound = ctx.preview && probe::variant() & 1 == 0;
                #[cfg(not(test))]
                let dir_bound = ctx.preview;
                let (reached, reachable) = if dir_bound {
                    let mut seeds: Vec<(usize, Dir)> = Vec::new();
                    let mut trunks: Vec<usize> = Vec::new();
                    for &t in &targets {
                        if sc.goal_gen[t] == sc.generation {
                            seeds.push((t, side_dir(dspots[sc.goal[t] as usize].port.side).opposite()));
                        } else {
                            trunks.push(t);
                        }
                    }
                    let starts: Vec<(usize, Dir, u32)> = sspots
                        .iter()
                        .filter_map(|s| {
                            let d = side_dir(s.port.side);
                            let c = board.port_free(s.port.at, d)?;
                            Some((c, d, start_cost(board, s)))
                        })
                        .collect();
                    sc.cost_from_dir(board, edge, &seeds, &trunks, &starts, true, stop)?
                } else {
                    sc.cost_from(board, &targets, &sources, net, ctx.preview, stop)?
                };
                // A full build's second flood (the whole board after its
                // window) starts from the first one's bounds and settled
                // sources and may run out short of them: its `reachable`
                // proves nothing.
                let fresh = ctx.preview || !flooded;
                flooded = true;
                #[cfg(test)]
                probe::with(|s| {
                    s.cost_from += 1;
                    s.cost_from_time += t0.elapsed();
                });
                // DECISION: the full build takes both no-path proofs too,
                // after its `cost_from` as before (whose bounds and settled
                // sources the next window's search inherits), skipping only
                // an A* proven to end in `NoPath`: the same answer and the
                // same scratch state for what follows, so the same routes
                // (goldens, and every `Routes` of the drag scenarios with
                // and without, compared); a parked full build is restarted
                // during drags, so its time decides when the final picture
                // lands.
                #[cfg(test)]
                let full_proofs = !ctx.preview && !probe::no_full_proofs();
                #[cfg(not(test))]
                let full_proofs = !ctx.preview;
                // DECISION: the full build tries `no_way_back` on the whole
                // board only (a large view's retry, a small view's cost-
                // bounded attempt), where its walled-in searches flood;
                // before every window search it cost 3-5% (measured).
                if (!reachable && fresh) || (full_proofs && window == whole && no_way(sc)?) {
                    // DECISION: `cost_from`'s proof that no source reaches a
                    // target (the same answer, `NoPath`) skips the forward
                    // A*, which would pop every state reachable from the
                    // sources first: the walled-in searches that cost
                    // 150k-1.5M pops each (EDIT-25).
                    if ctx.preview || full_proofs {
                        #[cfg(test)]
                        if window != whole {
                            probe::with(|s| s.retried += 1);
                        }
                        break;
                    }
                }
                bound = if dir_bound { Bound::Dir(reached) } else { Bound::Cost(reached) };
            }
            let out = astar(board, sc, edge, (lo, hi), &sspots, &dspots, bound, &trunk_at, stop)?;
            match out {
                Outcome::Found(f) => return Ok(Some(f)),
                Outcome::NoPath => {
                    #[cfg(test)]
                    if window != whole {
                        probe::with(|s| s.retried += 1);
                    }
                    break;
                }
                Outcome::GaveUp => {}
            }
        }
    }
    Ok(None)
}

/// Cells round the moved box's old and new places in which a preview
/// looks for a cell of the previous route to rejoin.
const REJOIN_MARGIN: i32 = 10;
/// Pops a rejoin leg may take before the line is searched as usual.
// DECISION: 3000: a rejoin is a local search (the successful ones on the
// drag scenarios took a few hundred to a few thousand pops); at 20 000
// the legs that fail (no way in round the moved box) cost more than the
// searches they replace (C3-entry hub +40y 194 -> 238 ms, measured).
const REJOIN_POPS: u32 = 3_000;

/// Preview only (EDIT-25): the line of `edge` with exactly one moved box,
/// made of its previous route from the fixed box up to a rejoin cell and a
/// new leg from there to a free port of the moved box; `None` when that
/// does not apply or finds nothing in `REJOIN_POPS`, and the line is then
/// searched from box to box. The rejoin cell lies on the previous route
/// within `REJOIN_MARGIN` cells of the moved box's old or new rectangle,
/// with the kept part up to it still legal by `astar`'s step rules on
/// `board`, free itself, and on a straight stretch (its two cells towards
/// the fixed box run the same way); the leg leaves it straight on or 45°
/// off, as `astar` turns, keeps off the kept part, and each rejoin cell
/// starts at the length of the kept part (steps and bends), so the leg
/// takes the cheapest whole line among the rejoin cells. The leg is bounded by
/// `cost_from_dir`, takes the kept port first, then any free port whose
/// stub clashes with no line (CORE-12), and a result crossing more lines
/// than the previous route did before the drag is dropped. Vertex lines
/// and joined lines are never rejoined.
// DECISION: a line with one moved end is rejoined to its previous route,
// not searched from box to box: the far part of that route was the
// router's own choice and is still legal, and the leg's pops scale with
// how far the box moved, not with the line's length. The junction is held
// to `astar`'s 45° rule on a straight stretch, so it reads as a bend of
// the line, not a kink. Anything that does not fit is searched as before.
#[allow(clippy::too_many_lines, clippy::many_single_char_names, clippy::similar_names)]
fn rejoin(ctx: &Ctx<'_>, board: &Board, sc: &mut Scratch, edge: usize, stop: &AtomicBool) -> Result<Option<Found>, Stopped> {
    let Some(prev) = ctx.previous else { return Ok(None) };
    let pe = &ctx.plan.edges[edge];
    let (s, d) = (pe.source.0 as usize, pe.destination.0 as usize);
    let (s_moved, d_moved) = (prev.centres[s] != ctx.centres[s], prev.centres[d] != ctx.centres[d]);
    let Some(old) = prev.lines.get(edge).and_then(Option::as_ref) else {
        #[cfg(test)]
        probe::with(|st| st.reason = "search: was unroutable");
        return Ok(None);
    };
    if s_moved == d_moved || !pe.vertices.is_empty() || !matches!(old.end, LineEnd::Arrow(_)) {
        #[cfg(test)]
        probe::with(|st| {
            st.reason = if s_moved && d_moved {
                "search: both ends moved, not carried"
            } else if !s_moved && !d_moved {
                "search: no end moved (grazed/covered)"
            } else {
                "search: vertex or joined line"
            };
        });
        return Ok(None);
    }
    let g = board.grid;
    let net = board.edge_net[edge] + 1;
    // The previous route's cells from the fixed box to the moved one.
    let mut path = cells_of(&old.points);
    if s_moved {
        path.reverse();
    }
    let (m, anchor) = if d_moved { (d, pe.destination_anchor) } else { (s, pe.source_anchor) };
    let mbox = &ctx.plan.boxes[m];
    let idx: Vec<Option<usize>> = path.iter().map(|&q| g.index(q)).collect();
    let dir_at = |k: usize| Segment::new(path[k - 1], path[k]).dir();
    // How far the kept part stays legal, cell by cell from the fixed box.
    let mut legal = 1;
    for k in 1..path.len() {
        let (Some(pi), Some(i), Some(dk)) = (idx[k - 1], idx[k], dir_at(k)) else { break };
        let (pc, c) = (&board.cells[pi], &board.cells[i]);
        let pc_other = pc.net != 0 && pc.net != net;
        if (k >= 2 && c.blocked > 0) || !along_ok(&board.along, pi, i, dk) {
            break;
        }
        if c.net != 0 && (c.net == net || pc_other || c.axes.count_ones() != 1 || c.axes & axis_bit(dk) != 0) {
            break;
        }
        if k >= 2 && dir_at(k - 1) != Some(dk) && pc_other {
            break;
        }
        if dk.is_diagonal() {
            let (sq, o) = square(path[k - 1], path[k]);
            if g.index(sq).is_some_and(|si| {
                let t = board.cells[si].diag[1 - o];
                t != 0 && board.edge_net[t as usize - 1] + 1 == net
            }) {
                break;
            }
        }
        legal = k;
    }
    let (ra, rb) = (mbox.rect_at(prev.centres[m]), mbox.rect_at(ctx.centres[m]));
    let near = Rect::from_corners(
        Point::new(ra.min.x.min(rb.min.x), ra.min.y.min(rb.min.y)),
        Point::new(ra.max.x.max(rb.max.x), ra.max.y.max(rb.max.y)),
    )
    .inflate(REJOIN_MARGIN);
    // The kept part's length up to each cell: steps, and a bend per turn.
    let mut length = vec![0u32; path.len()];
    for k in 1..path.len() {
        let step = if dir_at(k).is_some_and(Dir::is_diagonal) { STEP_DIAGONAL } else { STEP_STRAIGHT };
        let bend = if k >= 2 && dir_at(k) != dir_at(k - 1) { BEND } else { 0 };
        length[k] = length[k - 1] + step + bend;
    }
    let open = |k: usize| idx[k].is_some_and(|i| board.cells[i].net == 0 && board.cells[i].blocked == 0) && dir_at(k) == dir_at(k - 1);
    let upto = legal.min(path.len().saturating_sub(1));
    let cands: Vec<usize> = (2..=upto).filter(|&k| near.contains(path[k]) && open(k)).collect();
    let Some(&first) = cands.first() else {
        #[cfg(test)]
        probe::with(|st| {
            st.rejoin_nocand += 1;
            st.reason = "search: rejoin, no rejoin cell";
        });
        return Ok(None);
    };
    let starts: Vec<Start> = cands
        .iter()
        .filter_map(|&k| {
            Some(Start {
                cell: idx[k]?,
                dir: dir_at(k)?,
                cost: length[k] - length[first],
                port: None,
            })
        })
        .collect();
    let mut goals: Vec<(usize, Port)> = end_spots(mbox, ctx.centres[m], anchor)
        .into_iter()
        .filter_map(|sp| Some((board.port_free(sp.port.at, side_dir(sp.port.side))?, sp.port)))
        .collect();
    goals.sort_unstable_by_key(|&(c, _)| c);
    goals.dedup_by_key(|x| x.0);
    // DECISION: as `search` does, a port whose stub runs 10px or 30px
    // beside a same-way line is left out while any other is free (CORE-12):
    // the leg cannot steer the stub away, and the clashes it left counted
    // against the preview (measured on the goal series).
    let stub = |p: &Port| board.stub_clash(p.at, side_dir(p.side), s_moved);
    #[cfg(test)]
    let old_rejoin = probe::variant() & 16 != 0;
    #[cfg(not(test))]
    let old_rejoin = false;
    if !old_rejoin && goals.iter().any(|(_, p)| !stub(p)) {
        goals.retain(|(_, p)| !stub(p));
    }
    // The moved box's old port, moved with it, when it is free: the lines
    // of one box keep their order round it, so a small drag adds no
    // crossings between them.
    let old_port = if d_moved {
        match old.end {
            LineEnd::Arrow(p) => p,
            LineEnd::Join(_) => return Ok(None),
        }
    } else {
        old.source
    };
    let shift = Point::new(ctx.centres[m].x - prev.centres[m].x, ctx.centres[m].y - prev.centres[m].y);
    let same = Point::new(old_port.at.x + shift.x, old_port.at.y + shift.y);
    let all_goals = goals.clone();
    if let Some(&kept_port) = goals.iter().find(|(_, p)| p.at == same && p.side == old_port.side) {
        goals = vec![kept_port];
    }
    if goals.is_empty() {
        return Ok(None);
    }
    // The leg keeps off the kept part (up to the furthest rejoin cell).
    let mut own = vec![false; g.len()];
    let last = *cands.last().expect("non-empty");
    for i in idx[..last].iter().flatten() {
        own[*i] = true;
    }
    // The leg's bound: `cost_from_dir` from the goal ports back to the
    // rejoin cells, inside a window round both (history left out, as the
    // leg charges none); the kept port first, then every free port.
    let mut span = near;
    for &k in &cands {
        let p = path[k];
        span = Rect::from_corners(
            Point::new(span.min.x.min(p.x), span.min.y.min(p.y)),
            Point::new(span.max.x.max(p.x), span.max.y.max(p.y)),
        );
    }
    let sources: Vec<(usize, Dir, u32)> = starts.iter().map(|s| (s.cell, s.dir, s.cost)).collect();
    let mut found = None;
    if old_rejoin {
        found = leg_capped(board, edge, &starts, false, &Goal::Ports(&goals), &own, REJOIN_POPS, None, stop)?;
    }
    for round in 0..2 {
        if old_rejoin {
            break;
        }
        let goals_now: &[(usize, Port)] = if round == 0 { &goals } else { &all_goals };
        #[cfg(test)]
        if round == 1 && probe::variant() & 4 != 0 {
            break;
        }
        if round == 1 && goals.len() == all_goals.len() {
            break;
        }
        let seeds: Vec<(usize, Dir)> = goals_now.iter().map(|&(c, p)| (c, side_dir(p.side).opposite())).collect();
        sc.fresh_flood();
        sc.window = span.inflate(WINDOW_MARGIN);
        let (reached, any) = sc.cost_from_dir(board, edge, &seeds, &[], &sources, false, stop)?;
        if !any {
            continue;
        }
        let stamp = sc.flood;
        let dbound = &sc.dbound;
        let h = |cell: u32, d: Dir| -> u32 {
            let e = dbound[cell as usize * 8 + d as usize];
            if e[0] == stamp { e[1].min(reached) } else { reached }
        };
        found = leg_capped(board, edge, &starts, false, &Goal::Ports(goals_now), &own, REJOIN_POPS, Some(&h), stop)?;
        if found.is_some() {
            break;
        }
    }
    let Some(found) = found else {
        #[cfg(test)]
        probe::with(|st| {
            st.rejoin_noleg += 1;
            st.reason = "search: rejoin, no leg in cap";
        });
        return Ok(None);
    };
    let Some(port) = found.end else { return Ok(None) };
    let Some(at) = found.cells.first().and_then(|&q| path.iter().position(|&p| p == q)) else { return Ok(None) };
    #[cfg(test)]
    probe::with(|st| {
        st.rejoined += 1;
        st.reason = "rejoined";
    });
    let mut cells: Vec<Point> = path[..=at].to_vec();
    cells.extend_from_slice(&found.cells[1..]);
    cells.push(port.at.step(side_dir(port.side)));
    cells.push(port.at);
    let (source, end) = if d_moved {
        (old.source, LineEnd::Arrow(port))
    } else {
        cells.reverse();
        (port, old.end)
    };
    let crossed = crossed_cells(board, &cells, net);
    // DECISION: a rejoined line that crosses more lines on this board than
    // its previous route does is searched instead: the rejoin is a
    // shortcut for the search, not a worse line (the preview may not
    // cross more than before the drag, EDIT-25; measured on the goal
    // series).
    #[cfg(test)]
    let guard = probe::variant() & 16 == 0;
    #[cfg(not(test))]
    let guard = true;
    // The previous route's crossings before the drag, as L4 counts them.
    let before = || -> usize {
        prev.lines
            .iter()
            .flatten()
            .filter(|l| l.edge != old.edge)
            .map(|l| crossings(&old.points, &l.points) as usize)
            .sum()
    };
    #[cfg(test)]
    let on_board = probe::variant() & 32 != 0;
    #[cfg(not(test))]
    let on_board = false;
    let limit = if on_board { crossed_cells(board, &path, net).len() } else { before() };
    if guard && crossed.len() > limit {
        #[cfg(test)]
        probe::with(|st| {
            st.rejoin_worse += 1;
            st.reason = "search: rejoin rejected (crossings)";
        });
        return Ok(None);
    }
    Ok(Some(Found { cells, source, end, crossed }))
}

/// The cells where a line of `net` along `cells` crosses another line on
/// `board`, each once per crossing, as `unwind` lists them.
fn crossed_cells(board: &Board, cells: &[Point], net: u32) -> Vec<usize> {
    let g = board.grid;
    let mut crossed = Vec::new();
    for (i, &c) in cells.iter().enumerate() {
        if let Some(ci) = g.index(c)
            && board.cells[ci].net != 0
            && board.cells[ci].net != net
        {
            crossed.push(ci);
        }
        if i > 0 {
            let p = cells[i - 1];
            if p.x != c.x && p.y != c.y {
                let (sq, o) = square(p, c);
                if let Some(si) = g.index(sq)
                    && board.cells[si].diag[1 - o] != 0
                {
                    crossed.push(si);
                }
            }
        }
    }
    crossed
}

/// The heuristic of one A* attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bound {
    /// Octile distance to the targets' bounding box (`H_MAX`, `H_MIN`).
    Octile,
    /// `Scratch::bound` where `cost_from` reached, else the cost it
    /// stopped at.
    Cost(u32),
    /// `Scratch::dbound` where `cost_from_dir` reached, else the cost it
    /// stopped at (a preview's).
    Dir(u32),
}

/// How one A* attempt ended.
enum Outcome {
    Found(Found),
    NoPath,
    /// Hit `POP_LIMIT` with the octile bound; retry with the cost bound.
    GaveUp,
}

/// States `no_way_back` may visit before it gives up proving.
// DECISION: 1024: the proofs measured on the drag scenarios took at most
// 28 states (a port no line can enter is a tiny pocket backwards), so the
// budget only bounds the waste of a search that does have a path.
const BACK_BUDGET: usize = 1024;

/// Preview only (EDIT-25): true when `astar` provably finds no path for
/// `edge`, shown by walking its steps backwards from the goal states
/// (`goals`: cell and the heading that enters its port) until no state is
/// left, never meeting a start state (`starts`: cell and heading); false
/// when it meets one or visits `BACK_BUDGET` states (no proof). Mirrors
/// `astar`'s step rules for a view without bus, steps ending in `window`:
/// no step into a box, a turn of 45° only after a straight step
/// and never on another line, onto another line only square across a
/// single straight run and never from a line cell, no diagonal through
/// the own net's diagonal; costs play no part.
#[allow(clippy::many_single_char_names, clippy::too_many_arguments)]
fn no_way_back(board: &Board, edge: usize, window: Rect, starts: &[(usize, Dir)], goals: &[(usize, Dir)], stop: &AtomicBool) -> Result<bool, Stopped> {
    let g = board.grid;
    let net = board.edge_net[edge] + 1;
    let key = |cell: usize, dir: Dir, run: u8| (cell * 8 + dir as usize) * 2 + usize::from(run - 1);
    // DECISION: a hashed set of the states seen, not a flag per state of
    // the board: at most `BACK_BUDGET` of them, and zeroing 16 flags per
    // cell for every search cost more than the walk (CORE-25: only looked
    // up, never iterated).
    let mut seen: std::collections::HashSet<usize, BuildHasherDefault<KeyHasher>> = std::collections::HashSet::default();
    let mut queue: Vec<(usize, Dir, u8)> = Vec::new();
    let is_start = |cell: usize, dir: Dir, run: u8| run == 2 && starts.contains(&(cell, dir));
    for &(cell, dir) in goals {
        for run in [1, 2] {
            if is_start(cell, dir, run) {
                return Ok(false);
            }
            if seen.insert(key(cell, dir, run)) {
                queue.push((cell, dir, run));
            }
        }
    }
    let mut popped = 0u32;
    #[cfg(test)]
    probe::with(|s| s.back += 1);
    while let Some((ni, nd, run)) = queue.pop() {
        #[cfg(test)]
        probe::with(|s| s.back_states += 1);
        popped = popped.wrapping_add(1);
        tick(popped, stop)?;
        if popped as usize > BACK_BUDGET {
            #[cfg(test)]
            probe::work(popped % STOP_EVERY);
            return Ok(false);
        }
        // The step into `ni` heading `nd`, from `ci`.
        let n = g.point(ni);
        if !window.contains(n) {
            continue;
        }
        let c = n.step(nd.opposite());
        let Some(ci) = g.index(c) else { continue };
        let (cc, nc) = (&board.cells[ci], &board.cells[ni]);
        let turned = run == 1;
        if cc.blocked > 0 || nc.blocked > 0 || !corridor_ok(board, ci, ni, nd, turned) || !along_ok(&board.along, ci, ni, nd) {
            continue;
        }
        if nd.is_diagonal() {
            let (sq, o) = square(c, n);
            if let Some(si) = g.index(sq) {
                let other = board.cells[si].diag[1 - o];
                if other != 0 && board.edge_net[other as usize - 1] + 1 == net {
                    continue;
                }
            }
        }
        let on_other = cc.net != 0 && cc.net != net;
        if nc.net != 0 && (nc.net == net || on_other || nc.axes.count_ones() != 1 || nc.axes & axis_bit(nd) != 0) {
            continue;
        }
        // The states at `ci` that take this step: straight on from either
        // run, or a 45° turn after a straight step, off any line.
        let mut preds: [(Dir, u8); 2] = [(nd, 1), (nd, 2)];
        let count = if turned {
            if on_other {
                0
            } else {
                preds = [(turn(nd, -1), 2), (turn(nd, 1), 2)];
                2
            }
        } else {
            2
        };
        for &(pd, pr) in &preds[..count] {
            if is_start(ci, pd, pr) {
                #[cfg(test)]
                probe::work(popped % STOP_EVERY);
                return Ok(false);
            }
            if seen.insert(key(ci, pd, pr)) {
                queue.push((ci, pd, pr));
            }
        }
    }
    #[cfg(test)]
    probe::work(popped % STOP_EVERY);
    #[cfg(test)]
    probe::with(|s| {
        s.back_proved += 1;
        s.back_max_proof = s.back_max_proof.max(popped);
    });
    Ok(true)
}

/// What `astar` charges a start state at the source spot `s`: the
/// offset from the side's middle, and CLASH when its stub runs beside a
/// same-way line (CORE-12).
fn start_cost(board: &Board, s: &Spot) -> u32 {
    let mut cost = START_OFFSET * u32::try_from(s.off).unwrap_or(0);
    if board.stub_clash(s.port.at, side_dir(s.port.side), true) {
        cost += CLASH;
    }
    cost
}

/// The A* loop of `search`.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::similar_names
)]
fn astar(
    board: &Board,
    sc: &mut Scratch,
    edge: usize,
    (lo, hi): (Point, Point),
    sspots: &[Spot],
    dspots: &[Spot],
    bound: Bound,
    trunk_at: &impl Fn(usize) -> Option<usize>,
    stop: &AtomicBool,
) -> Result<Outcome, Stopped> {
    let g = board.grid;
    let bus = board.bus;
    let net = board.edge_net[edge] + 1;
    let flood = sc.flood;
    let limited = bound == Bound::Octile;
    let window = sc.window;
    let h = |sc: &Scratch, p: Point, i: usize, d: Dir| -> u32 {
        if let Bound::Dir(reached) = bound {
            let e = sc.dbound[i * 8 + d as usize];
            return if e[0] == flood { e[1].min(reached) } else { reached };
        }
        if let Bound::Cost(reached) = bound {
            return if sc.bound_gen[i] == flood {
                sc.bound[i].min(reached)
            } else {
                reached
            };
        }
        let dx = u32::try_from((lo.x - p.x).max(0).max(p.x - hi.x)).unwrap_or(0);
        let dy = u32::try_from((lo.y - p.y).max(0).max(p.y - hi.y)).unwrap_or(0);
        H_MAX * dx.max(dy) + H_MIN * dx.min(dy)
    };
    let mut pops = 0u32;
    let mut tie = 0u32;
    for (k, s) in sspots.iter().enumerate() {
        let d = side_dir(s.port.side);
        let Some(c) = board.port_free(s.port.at, d) else {
            continue;
        };
        let cost = start_cost(board, s);
        let r = Rec {
            cell: u32::try_from(c).expect("cell fits u32"),
            dir: d,
            run: 2,
            steps: 0,
            cost,
            parent: u32::MAX,
            spot: u32::try_from(k).expect("spot index fits u32"),
        };
        let hc = h(sc, g.point(c), c, d);
        sc.push(r, hc, &mut tie, bus);
    }

    while let Some(idx) = sc.heap.pop() {
        pops += 1;
        tick(pops, stop)?;
        if limited && pops > POP_LIMIT {
            sc.spent += u64::from(pops);
            #[cfg(test)]
            probe::pops(pops, true);
            return Ok(Outcome::GaveUp);
        }
        let r = sc.recs[idx as usize];
        if r.steps == JOINED {
            sc.spent += u64::from(pops);
            #[cfg(test)]
            probe::pops(pops, false);
            return Ok(Outcome::Found(unwind(board, sc, idx, sspots, None, edge)));
        }
        if sc.best(r.cell, r.dir, r.run, r.steps, bus) != Some(r.cost) {
            continue;
        }
        let ci = r.cell as usize;
        if sc.goal_gen[ci] == sc.generation {
            let spot = dspots[sc.goal[ci] as usize];
            if side_dir(spot.port.side).opposite() == r.dir {
                sc.spent += u64::from(pops);
                #[cfg(test)]
                probe::pops(pops, false);
                #[cfg(test)]
                probe::with(|s| s.last_cost = r.cost);
                return Ok(Outcome::Found(unwind(board, sc, idx, sspots, Some(spot.port), edge)));
            }
        }
        let c = g.point(ci);
        let on_other = board.cells[ci].net != 0 && board.cells[ci].net != net;
        for by in [0i8, 1, -1] {
            if by != 0 && (on_other || r.run < 2) {
                continue;
            }
            let nd = turn(r.dir, by);
            let n = c.step(nd);
            if !window.contains(n) {
                continue;
            }
            let Some(ni) = g.index(n) else { continue };
            if board.cells[ni].blocked > 0 || !corridor_ok(board, ci, ni, nd, by != 0) || !along_ok(&board.along, ci, ni, nd) {
                continue;
            }
            let mut step = if nd.is_diagonal() {
                STEP_DIAGONAL
            } else {
                STEP_STRAIGHT
            };
            if by != 0 {
                step += BEND;
            }
            if board.clash(c, nd) {
                step += CLASH;
            }
            if !board.fences.is_empty() {
                step += board.fence_cost(edge, c, n);
            }
            if nd.is_diagonal() {
                let (sq, o) = square(c, n);
                if let Some(si) = g.index(sq) {
                    let other = board.cells[si].diag[1 - o];
                    if other != 0 {
                        if board.edge_net[other as usize - 1] + 1 == net {
                            continue;
                        }
                        step += CROSS;
                    }
                }
            }
            if board.cells[ni].net != 0 {
                if board.cells[ni].net == net {
                    // A join: straight on (so the last run is >= 20px,
                    // CORE-6), after MIN_JOIN own steps, <= 90° to the trunk.
                    if !bus || on_other || by != 0 || r.steps < MIN_JOIN {
                        continue;
                    }
                    if trunk_at(ni).is_none() {
                        continue;
                    }
                    let dd = Dir::ALL[usize::from(board.cells[ni].down - 1)];
                    if nd.turn_steps(dd) > 2 {
                        continue;
                    }
                    let nr = Rec {
                        cell: u32::try_from(ni).expect("cell fits u32"),
                        dir: nd,
                        run: 2,
                        steps: JOINED,
                        cost: r.cost + step,
                        parent: idx,
                        spot: 0,
                    };
                    sc.push(nr, 0, &mut tie, bus);
                    continue;
                }
                let ax = board.cells[ni].axes;
                if on_other || ax.count_ones() != 1 || ax & axis_bit(nd) != 0 {
                    continue;
                }
                step += CROSS;
            } else if board.near_other(ni, net) {
                step += NEAR;
            } else if board.cells[ni].lane & axis_bit(nd) != 0 {
                step = step * LANE_NUM / LANE_DEN;
            }
            step += board.cells[ni].history;
            let nr = Rec {
                cell: u32::try_from(ni).expect("cell fits u32"),
                dir: nd,
                run: if by == 0 { 2 } else { 1 },
                steps: if bus { (r.steps + 1).min(MIN_JOIN) } else { 0 },
                cost: r.cost + step,
                parent: idx,
                spot: 0,
            };
            let hn = h(sc, n, ni, nd);
            sc.push(nr, hn, &mut tie, bus);
        }
    }
    sc.spent += u64::from(pops);
    #[cfg(test)]
    probe::pops(pops, false);
    Ok(Outcome::NoPath)
}

/// Builds the found path from the arena: source port cells, the searched
/// cells, then the destination port cells (or nothing after a join).
#[allow(clippy::too_many_arguments, clippy::many_single_char_names)]
fn unwind(
    board: &Board,
    sc: &Scratch,
    last: u32,
    sspots: &[Spot],
    arrow: Option<Port>,
    edge: usize,
) -> Found {
    let g = board.grid;
    let mut rev = Vec::new();
    let mut idx = last;
    let spot = loop {
        let r = sc.recs[idx as usize];
        rev.push(g.point(r.cell as usize));
        if r.parent == u32::MAX {
            break sspots[r.spot as usize];
        }
        idx = r.parent;
    };
    let d = side_dir(spot.port.side);
    let mut cells = vec![spot.port.at, spot.port.at.step(d)];
    cells.extend(rev.into_iter().rev());
    let end = if let Some(port) = arrow {
        let back = side_dir(port.side);
        cells.push(port.at.step(back));
        cells.push(port.at);
        LineEnd::Arrow(port)
    } else {
        let at = *cells.last().expect("a joined path has cells");
        let gi = g.index(at).expect("join on the board");
        let trunk = board.cells[gi].owner as usize - 1;
        LineEnd::Join(Join {
            trunk: edge_id(trunk),
            at,
        })
    };
    let net = board.edge_net[edge] + 1;
    let joined = matches!(end, LineEnd::Join(_));
    let mut crossed = Vec::new();
    let upto = if joined { cells.len() - 1 } else { cells.len() };
    for i in 0..upto {
        let c = cells[i];
        if let Some(ci) = g.index(c)
            && board.cells[ci].net != 0
            && board.cells[ci].net != net
        {
            crossed.push(ci);
        }
        if i > 0 {
            let p = cells[i - 1];
            if p.x != c.x && p.y != c.y {
                let (sq, o) = square(p, c);
                if let Some(si) = g.index(sq)
                    && board.cells[si].diag[1 - o] != 0
                {
                    crossed.push(si);
                }
            }
        }
    }
    Found {
        cells,
        source: spot.port,
        end,
        crossed,
    }
}

/// The bend points of a cell path: first, every cell where the direction
/// changes, last.
fn vertices(cells: &[Point]) -> Vec<Point> {
    let mut out = vec![cells[0]];
    for w in cells.windows(3) {
        if Segment::new(w[0], w[1]).dir() != Segment::new(w[1], w[2]).dir() {
            out.push(w[1]);
        }
    }
    out.push(*cells.last().expect("non-empty"));
    out
}

/// The cells of a vertex list, one per grid step.
fn cells_of(points: &[Point]) -> Vec<Point> {
    let mut out = vec![points[0]];
    for w in points.windows(2) {
        let s = Segment::new(w[0], w[1]);
        let (Some(d), Some(n)) = (s.dir(), s.len_steps()) else {
            continue;
        };
        for k in 1..=n {
            out.push(step_n(w[0], d, k));
        }
    }
    out
}

/// How good one routing of the view is: fewer unroutable edges, then
/// fewer same-way neighbours off the 20px lane, then fewer crossings,
/// then shorter (CORE-13, CORE-12, CORE-11).
// DECISION: clashes rank above crossings to match CLASH > CROSS in the
// search, so the order choice and the search pull the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Score {
    unroutable: usize,
    /// Same-way neighbour pairs 10px or 30px apart (CORE-12).
    clashes: u32,
    crossings: u32,
    straight: u64,
    diagonal: u64,
}

impl Score {
    fn of(lines: &[Option<Line>]) -> Score {
        let mut s = Score {
            unroutable: lines.iter().filter(|l| l.is_none()).count(),
            clashes: count_clashes(lines),
            crossings: count_crossings(lines),
            straight: 0,
            diagonal: 0,
        };
        for line in lines.iter().flatten() {
            for seg in line.segments() {
                let n = u64::try_from(seg.len_steps().unwrap_or(0)).unwrap_or(0);
                if seg.dir().is_some_and(Dir::is_diagonal) {
                    s.diagonal += n;
                } else {
                    s.straight += n;
                }
            }
        }
        s
    }

    /// Total length `straight + diagonal * sqrt 2`, compared exactly.
    fn cmp_length(&self, other: &Score) -> CmpOrdering {
        let a = i128::from(self.straight) - i128::from(other.straight);
        let b = i128::from(other.diagonal) - i128::from(self.diagonal);
        // self - other = a - b * sqrt 2; its sign:
        match (a.signum(), b.signum()) {
            (0, 0) => CmpOrdering::Equal,
            (x, y) if x >= 0 && y <= 0 => CmpOrdering::Greater,
            (x, y) if x <= 0 && y >= 0 => CmpOrdering::Less,
            (1, _) => (a * a).cmp(&(2 * b * b)),
            _ => (2 * b * b).cmp(&(a * a)),
        }
    }

    fn better_than(&self, other: &Score) -> bool {
        (self.unroutable, self.clashes, self.crossings)
            .cmp(&(other.unroutable, other.clashes, other.crossings))
            .then_with(|| self.cmp_length(other))
            == CmpOrdering::Less
    }

    const fn within(&self, budget: u32) -> bool {
        self.unroutable == 0 && self.crossings <= budget
    }

    /// No more unroutable lines, clashes or crossings than `other`, each.
    const fn no_worse_than(&self, other: &Score) -> bool {
        self.unroutable <= other.unroutable && self.clashes <= other.clashes && self.crossings <= other.crossings
    }
}

/// When a solve has a good enough routing and skips the rest of its work.
#[derive(Debug, Clone, Copy)]
struct Enough {
    /// The view's crossing budget (`maxCrossings`, CORE-17).
    budget: u32,
    /// A drag preview's: the score of the routes the drag started from.
    before: Option<Score>,
    /// A drag preview's cap on its work when it is not good enough.
    effort: Option<Effort>,
}

/// How much a drag preview that is worse than before the drag may still
/// spend on rip-up and other edge orders (EDIT-25).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Effort {
    /// Edge orders tried at most.
    orders: u32,
    /// Rip-up rounds per order at most.
    rounds: u32,
    /// Rounds only while some line is unroutable.
    unroutable_only: bool,
    /// Rounds only while the solve has popped fewer A* states than this.
    pops: u64,
}

/// The preview's `Effort`.
// DECISION: one edge order, and rip-up rounds only while a line is left
// unroutable (an unroutable line is drawn straight through boxes, the
// one fault the eye catches in a passing frame) and only while the solve
// has popped under 300k A* states (a count, never a clock: CORE-25).
// Measured against the former preview on the six representative drags
// and every 4th step of two drag series: the same crossings / clashes /
// unroutable on all six; over the 20 series steps within 4% of the
// crossings, +2 clashes and +4 unroutable lines in all (the release build
// replaces them); preview time a quarter to a half. A first pass alone
// left up to 8 lines unroutable on one drag; the second order paid for
// itself on none of the six; the pop budget halves the generated 8-box
// series' p90 (412 -> 193 ms) for 2 more unroutable lines over 10 steps.
const PREVIEW_EFFORT: Effort = Effort {
    orders: 1,
    rounds: u32::MAX,
    unroutable_only: true,
    pops: 300_000,
};

impl Enough {
    /// Nothing unroutable and the crossings within the budget, or, for a
    /// preview, no worse than before the drag.
    // DECISION: a preview is good enough once it is no worse than the
    // picture the drag started from, each of unroutable, clashes and
    // crossings no higher (componentwise, so no count is traded for
    // another). The absolute budget alone is never met on a view that
    // already has crossings, so every preview ran every rip-up round of
    // every order (4-12 passes) to reach a bar the full build itself
    // missed; the full route after release still runs them all (EDIT-25).
    fn met(&self, s: &Score) -> bool {
        s.within(self.budget) || self.before.is_some_and(|b| s.no_worse_than(&b))
    }
}

/// Pairs of axis runs of two routed lines that head the same way, share
/// a stretch and lie 1 or 3 cells apart: the lanes CORE-12 forbids.
fn count_clashes(lines: &[Option<Line>]) -> u32 {
    let runs: Vec<Vec<(Dir, Point, Point)>> = lines
        .iter()
        .flatten()
        .map(|l| {
            l.points
                .windows(2)
                .filter_map(|w| {
                    let d = Segment::new(w[0], w[1]).dir()?;
                    (!d.is_diagonal()).then_some((d, w[0], w[1]))
                })
                .collect()
        })
        .collect();
    // DECISION: each line's runs' box, grown by 3 cells, first: two lines
    // whose boxes are apart have no runs 1 or 3 cells apart (same count,
    // CORE-24).
    let boxes: Vec<Option<(Point, Point)>> = runs
        .iter()
        .map(|r| {
            let pts: Vec<Point> = r.iter().flat_map(|&(_, a, b)| [a, b]).collect();
            (!pts.is_empty()).then(|| {
                let (lo, hi) = crate::geom::bounds_of(&pts);
                (Point::new(lo.x - 3, lo.y - 3), Point::new(hi.x + 3, hi.y + 3))
            })
        })
        .collect();
    let mut n = 0u32;
    for (i, a) in runs.iter().enumerate() {
        for (j, b) in runs.iter().enumerate().skip(i + 1) {
            let (Some(ba), Some(bb)) = (boxes[i], boxes[j]) else { continue };
            if !crate::geom::boxes_meet(ba, bb) {
                continue;
            }
            for &(da, a0, a1) in a {
                for &(db, b0, b1) in b {
                    if da != db {
                        continue;
                    }
                    let horizontal = matches!(da, Dir::E | Dir::W);
                    let (across, along) = if horizontal {
                        ((a0.y - b0.y).abs(), [a0.x, a1.x, b0.x, b1.x])
                    } else {
                        ((a0.x - b0.x).abs(), [a0.y, a1.y, b0.y, b1.y])
                    };
                    let lo = along[0].min(along[1]).max(along[2].min(along[3]));
                    let hi = along[0].max(along[1]).min(along[2].max(along[3]));
                    if hi > lo && (across == 1 || across == 3) {
                        n = n.saturating_add(1);
                    }
                }
            }
        }
    }
    n
}

/// The L4 count over routed lines (`Routes::crossings`): every pair, a
/// joined line's last segment left out against its own trunk.
fn count_crossings(lines: &[Option<Line>]) -> u32 {
    let routed: Vec<&Line> = lines.iter().flatten().filter(|l| !l.points.is_empty()).collect();
    // DECISION: lines whose boxes are apart cross nowhere: skipped before
    // `crossings` (same count, CORE-24).
    let boxes: Vec<(Point, Point)> = routed.iter().map(|l| crate::geom::bounds_of(&l.points)).collect();
    let mut n = 0u32;
    for (i, a) in routed.iter().enumerate() {
        for (j, b) in routed.iter().enumerate().skip(i + 1) {
            if !crate::geom::boxes_meet(boxes[i], boxes[j]) {
                continue;
            }
            let c = if a.joins() == Some(b.edge) {
                crossings(&b.points, &a.points[..a.points.len() - 1])
            } else if b.joins() == Some(a.edge) {
                crossings(&a.points, &b.points[..b.points.len() - 1])
            } else {
                crossings(&a.points, &b.points)
            };
            n = n.saturating_add(c);
        }
    }
    n
}

/// One routing pass: kept lines first, then `order`.
#[derive(Clone)]
struct Pass {
    lines: Vec<Option<Line>>,
    score: Score,
    failed: Vec<usize>,
    crossed: Vec<usize>,
}

#[allow(clippy::too_many_arguments)]
fn pass(
    ctx: &Ctx<'_>,
    grid: Grid,
    sc: &mut Scratch,
    kept: &[Option<Line>],
    order: &[usize],
    history: &[u32],
    stop: &AtomicBool,
) -> Result<Pass, Stopped> {
    #[cfg(test)]
    let t0 = std::time::Instant::now();
    let mut board = ctx.board(grid, history);
    for line in kept.iter().flatten() {
        board.add(&cells_of(&line.points), line.edge.0 as usize, &line.end);
    }
    #[cfg(test)]
    probe::with(|s| {
        s.board += t0.elapsed();
        s.passes += 1;
    });
    let mut lines = kept.to_vec();
    let mut failed = Vec::new();
    let mut crossed = Vec::new();
    for &e in order {
        check(stop)?;
        #[cfg(test)]
        let t0 = std::time::Instant::now();
        #[cfg(test)]
        let p0 = probe::pops_so_far();
        #[cfg(test)]
        let (f0, ps0) = probe::with_get(|s| (s.dir_states, s.passes));
        let found = search(ctx, &board, sc, e, stop)?;
        #[cfg(test)]
        probe::categorise(t0, f0, p0, ps0);
        #[cfg(test)]
        probe::searched(p0, found.is_some());
        #[cfg(test)]
        probe::with(|s| {
            let t = t0.elapsed();
            s.search += t;
            s.max_search = s.max_search.max(t);
            s.searches += 1;
        });
        match found {
            Some(f) => {
                board.add(&f.cells, e, &f.end);
                crossed.extend_from_slice(&f.crossed);
                lines[e] = Some(Line {
                    edge: edge_id(e),
                    points: vertices(&f.cells),
                    source: f.source,
                    end: f.end,
                });
            }
            None => failed.push(e),
        }
    }
    #[cfg(test)]
    let t0 = std::time::Instant::now();
    let score = Score::of(&lines);
    #[cfg(test)]
    probe::with(|s| s.score += t0.elapsed());
    Ok(Pass {
        lines,
        score,
        failed,
        crossed,
    })
}

/// One edge order with negotiated-congestion rip-up: raise the history
/// cost where lines crossed or blocked a failed line's ports, route the
/// failed lines first, repeat; keep the best pass (CORE-13, CORE-32).
#[allow(clippy::too_many_arguments)]
fn improve(
    ctx: &Ctx<'_>,
    grid: Grid,
    sc: &mut Scratch,
    kept: &[Option<Line>],
    order: &[usize],
    rounds: u32,
    enough: Enough,
    stop: &AtomicBool,
) -> Result<Pass, Stopped> {
    improve_from(ctx, grid, sc, kept, order, rounds, enough, stop).map(|(_, best)| best)
}

/// `improve`, also giving its first pass (what `improve` gives with no
/// rounds): `(first pass, best pass)`.
#[allow(clippy::too_many_arguments)]
fn improve_from(
    ctx: &Ctx<'_>,
    grid: Grid,
    sc: &mut Scratch,
    kept: &[Option<Line>],
    order: &[usize],
    rounds: u32,
    enough: Enough,
    stop: &AtomicBool,
) -> Result<(Pass, Pass), Stopped> {
    let mut history = vec![0u32; grid.len()];
    let mut current = order.to_vec();
    let mut best = pass(ctx, grid, sc, kept, &current, &history, stop)?;
    let first = best.clone();
    #[cfg(test)]
    probe::with(|s| {
        s.first_pass.push((best.score.crossings, best.score.clashes, best.score.unroutable));
        s.first_crossed.push(best.crossed.len());
    });
    let mut last_failed = best.failed.clone();
    let mut last_crossed = best.crossed.clone();
    let mut stall = 0;
    for round in 0..rounds {
        check(stop)?;
        // DECISION: rip-up stops once nothing is unroutable and the
        // crossings are within the view's own budget (`maxCrossings`,
        // CORE-17): further rounds cost time (CORE-24) and the score
        // already meets what lint checks.
        if enough.met(&best.score) || (last_failed.is_empty() && last_crossed.is_empty()) {
            break;
        }
        if let Some(e) = enough.effort
            && (round >= e.rounds || (e.unroutable_only && best.score.unroutable == 0) || sc.spent >= e.pops)
        {
            break;
        }
        for &c in &last_crossed {
            history[c] = history[c].saturating_add(HISTORY);
        }
        for &e in &last_failed {
            let pe = &ctx.plan.edges[e];
            for b in [pe.source, pe.destination] {
                let bx = &ctx.plan.boxes[b.0 as usize];
                for s in spots(bx, ctx.centres[b.0 as usize]) {
                    let d = side_dir(s.port.side);
                    for k in 1..=2 {
                        if let Some(i) = grid.index(step_n(s.port.at, d, k)) {
                            history[i] = history[i].saturating_add(HISTORY);
                        }
                    }
                }
            }
        }
        let mut next: Vec<usize> = last_failed.clone();
        next.extend(current.iter().filter(|e| !last_failed.contains(e)));
        current = next;
        let p = pass(ctx, grid, sc, kept, &current, &history, stop)?;
        last_failed.clone_from(&p.failed);
        last_crossed.clone_from(&p.crossed);
        if p.score.better_than(&best.score) {
            best = p;
            stall = 0;
        } else {
            stall += 1;
            if stall >= STALL_ROUNDS {
                break;
            }
        }
    }
    Ok((first, best))
}

/// Threads a full build routes its `n` edge orders on: one per order up
/// to `available_parallelism`, one (the sequential loop) for a preview,
/// for one order, or where the platform gives no thread count.
// DECISION: a preview stays on one thread: it is one order, and the
// editor's worker must not take every core from the UI for a frame that
// is replaced a moment later.
fn order_threads(n: usize, preview: bool) -> usize {
    #[cfg(test)]
    if probe::sequential() {
        return 1;
    }
    if preview || n < 2 {
        return 1;
    }
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get).min(n)
}

/// The edge orders of a full build routed on `threads` scoped threads,
/// each with its own `Scratch`, then chosen exactly as the sequential loop
/// of `solve` chooses: in order, the first strictly better wins, and an
/// order after one within the view's budget takes its first pass only.
/// Each order gives both its first pass and its rip-up result, so which
/// one the sequential loop would have used is decided afterwards without
/// re-routing; the result is the sequential one, bit for bit.
// DECISION: the orders are independent apart from that rule (each pass
// starts from the kept lines and a fresh history, and a `Scratch` carries
// nothing between searches but stamps that only have to differ), so they
// run at once; an order whose rounds turn out not to be needed costs a
// core, not time. The stop flag is shared: every thread gives up within
// `STOP_EVERY` states of it being set.
#[allow(clippy::too_many_arguments)]
fn parallel_orders(
    ctx: &Ctx<'_>,
    grid: Grid,
    kept: &[Option<Line>],
    all_orders: &[Vec<usize>],
    config: RouteConfig,
    enough: Enough,
    threads: usize,
    stop: &AtomicBool,
) -> Result<Option<Pass>, Stopped> {
    use std::sync::atomic::AtomicUsize;
    type Done = Vec<(usize, Result<(Pass, Pass), Stopped>)>;
    let next = AtomicUsize::new(0);
    #[cfg(test)]
    let settings = probe::settings();
    let results: Vec<Done> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    #[cfg(test)]
                    {
                        probe::set_settings(settings);
                        probe::reset();
                    }
                    let mut sc = Scratch::new(grid.len());
                    let mut out: Done = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(order) = all_orders.get(k) else { break };
                        let r = improve_from(ctx, grid, &mut sc, kept, order, config.max_iterations, enough, stop);
                        let failed = r.is_err();
                        out.push((k, r));
                        if failed {
                            break;
                        }
                    }
                    #[cfg(test)]
                    let out = (out, probe::take());
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                let joined = h.join().unwrap_or_else(|e| std::panic::resume_unwind(e));
                #[cfg(test)]
                {
                    let (out, mut st) = joined;
                    st.orders = u32::try_from(out.len()).unwrap_or(u32::MAX);
                    probe::merge(&st);
                    out
                }
                #[cfg(not(test))]
                joined
            })
            .collect()
    });
    let mut by_order: Vec<Option<Result<(Pass, Pass), Stopped>>> = (0..all_orders.len()).map(|_| None).collect();
    for (k, r) in results.into_iter().flatten() {
        by_order[k] = Some(r);
    }
    let mut best: Option<Pass> = None;
    for r in by_order {
        let (first, full) = r.ok_or(Stopped)??;
        let p = if best.as_ref().is_some_and(|b| enough.met(&b.score)) { first } else { full };
        if best.as_ref().is_none_or(|b| p.score.better_than(&b.score)) {
            best = Some(p);
        }
    }
    Ok(best)
}

/// `SplitMix64`: a small fixed PRNG written in core (no `rand`, HLD §5).
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        let n64 = u64::try_from(n).expect("usize fits u64");
        usize::try_from(self.next() % n64).expect("below n fits usize")
    }
}

/// The edge orders of CORE-13: by centre distance ascending, descending,
/// then fixed-seed shuffles of `set`.
fn orders(plan: &ViewPlan, centres: &[Point], set: &[usize], n: u32) -> Vec<Vec<usize>> {
    let dist = |e: usize| {
        let pe = &plan.edges[e];
        let (a, b) = (
            centres[pe.source.0 as usize],
            centres[pe.destination.0 as usize],
        );
        i64::from((a.x - b.x).abs()) + i64::from((a.y - b.y).abs())
    };
    let mut asc = set.to_vec();
    asc.sort_by_key(|&e| (dist(e), e));
    let mut out = vec![asc];
    if n >= 2 {
        let mut desc = set.to_vec();
        desc.sort_by_key(|&e| (Reverse(dist(e)), e));
        out.push(desc);
    }
    let mut rng = SplitMix(SEED);
    for _ in 2..n {
        let mut o = set.to_vec();
        for i in (1..o.len()).rev() {
            let j = rng.below(i + 1);
            o.swap(i, j);
        }
        out.push(o);
    }
    out
}

/// Routes `set` around the `kept` lines over every edge order and keeps
/// the best (unroutable, crossings, length); a tie keeps the first order.
#[allow(clippy::too_many_arguments)]
fn solve(
    plan: &ViewPlan,
    centres: &[Point],
    config: RouteConfig,
    stop: &AtomicBool,
    kept: Vec<Option<Line>>,
    set: &[usize],
    corridor: &[(Point, u8)],
    before: Option<Score>,
    previous: Option<&Routes>,
) -> Result<Routes, Stopped> {
    let ctx = Ctx {
        plan,
        centres,
        corridor,
        preview: before.is_some(),
        previous,
    };
    let grid = Grid::new(plan, centres, &kept);
    let effort = before.map(|_| {
        #[cfg(test)]
        if let Some(e) = probe::effort() {
            return e;
        }
        PREVIEW_EFFORT
    });
    let enough = Enough {
        budget: plan.options.max_crossings,
        before,
        effort,
    };
    #[cfg(test)]
    let t0 = std::time::Instant::now();
    let mut sc = Scratch::new(grid.len());
    #[cfg(test)]
    probe::with(|s| {
        s.scratch += t0.elapsed();
        s.solves += 1;
        s.grid_cells = s.grid_cells.max(grid.len());
    });
    let mut best: Option<Pass> = None;
    let any_anchor = set.iter().any(|&e| anchored(&plan.edges[e]));
    let mut all_orders = orders(plan, centres, set, config.orders);
    if any_anchor {
        for order in &mut all_orders {
            order.sort_by_key(|&e| !anchored(&plan.edges[e]));
        }
    }
    let threads = order_threads(all_orders.len(), before.is_some());
    if threads > 1 {
        best = parallel_orders(&ctx, grid, &kept, &all_orders, config, enough, threads, stop)?;
    }
    for (orders_done, order) in all_orders.into_iter().enumerate() {
        if threads > 1 {
            break;
        }
        check(stop)?;
        // P4-209: anchored lines first, each group in the order's own
        // order (stable), so a pinned port is not taken by a free line
        // (sorted above). A view without anchors keeps its orders
        // untouched (P4-210).
        // DECISION: once an order is within the view's budget with nothing
        // unroutable, later orders get their plain pass only, no rip-up:
        // rip-up exists to reach that bar, and the rounds are the
        // router's main cost (CORE-24).
        let met = best.as_ref().is_some_and(|b| enough.met(&b.score));
        // DECISION: a preview that has a good enough routing skips the
        // later orders too: their plain passes cost as much as the first
        // and the preview only has to follow the drag (EDIT-25).
        if met && enough.before.is_some() {
            break;
        }
        if enough.effort.is_some_and(|e| best.is_some() && orders_done >= usize::try_from(e.orders).unwrap_or(usize::MAX)) {
            break;
        }
        let rounds = if met {
            0
        } else {
            config.max_iterations
        };
        #[cfg(test)]
        probe::with(|s| s.orders += 1);
        let p = improve(&ctx, grid, &mut sc, &kept, &order, rounds, enough, stop)?;
        if best.as_ref().is_none_or(|b| p.score.better_than(&b.score)) {
            best = Some(p);
        }
    }
    let (lines, crossings) = if let Some(mut b) = best {
        // P4-212: a pinned end walled in by other lines is still honoured,
        // crossing them, as a vertex line is (`route_pinned`).
        if any_anchor && set.iter().any(|&e| b.lines[e].is_none() && anchored(&plan.edges[e])) {
            let walled: Vec<usize> = set.iter().copied().filter(|&e| b.lines[e].is_none() && anchored(&plan.edges[e])).collect();
            for e in walled {
                b.lines[e] = pinned_line(&ctx, &ctx.board(grid, &[]), e, stop)?;
            }
            let crossings = count_crossings(&b.lines);
            (b.lines, crossings)
        } else {
            (b.lines, b.score.crossings)
        }
    } else {
        let crossings = count_crossings(&kept);
        (kept, crossings)
    };
    Ok(Routes {
        lines,
        centres: centres[..plan.boxes.len()].to_vec(),
        crossings,
        bundles: Vec::new(),
    })
}

// ------------------------------------------------------ hand-added vertices
//
// P3-4, P3-303..P3-308. A line with vertices (`PlanEdge::vertices`) is
// routed before every other line, in edge order, as legs: source port ->
// v1 -> ... -> vk -> destination port, each leg an A* on the board with
// the same steps and costs as `astar` (no bus). Only at a vertex may the
// line turn by more than 45° (never back on itself, P3-305). The lines
// without vertices are then routed by `solve`, unchanged, with the vertex
// lines kept as obstacles (P3-307). A view with no vertices never comes
// here (P3-5).

/// True when some edge of `plan` has hand-added vertices.
fn pinned_any(plan: &ViewPlan) -> bool {
    plan.edges.iter().any(|e| !e.vertices.is_empty())
}

/// `solve` for a view with vertex lines: the vertex lines of `set` first
/// (P3-307), then the rest of `set` around them and the `kept` lines.
#[allow(clippy::too_many_arguments)]
fn solve_pinned(
    plan: &ViewPlan,
    centres: &[Point],
    config: RouteConfig,
    stop: &AtomicBool,
    mut kept: Vec<Option<Line>>,
    set: &[usize],
    corridor: &[(Point, u8)],
    before: Option<Score>,
    previous: Option<&Routes>,
) -> Result<Routes, Stopped> {
    let (pinned, rest): (Vec<usize>, Vec<usize>) = set.iter().copied().partition(|&e| !plan.edges[e].vertices.is_empty());
    let grid = pinned_grid(plan, centres, &kept);
    let ctx = Ctx {
        plan,
        centres,
        corridor,
        preview: before.is_some(),
        previous,
    };
    for &e in &pinned {
        check(stop)?;
        kept[e] = route_pinned(&ctx, grid, &kept, e, stop)?;
    }
    solve(plan, centres, config, stop, kept, &rest, corridor, before, previous)
}

/// `Grid::new` grown to hold every vertex with the board's margin round it.
fn pinned_grid(plan: &ViewPlan, centres: &[Point], kept: &[Option<Line>]) -> Grid {
    let g = Grid::new(plan, centres, kept);
    let mut lo = Point::new(g.x0, g.y0);
    let mut hi = Point::new(g.x0 + g.w - 1, g.y0 + g.h - 1);
    for v in plan.edges.iter().flat_map(|e| &e.vertices) {
        lo = Point::new(lo.x.min(v.x - MARGIN), lo.y.min(v.y - MARGIN));
        hi = Point::new(hi.x.max(v.x + MARGIN), hi.y.max(v.y + MARGIN));
    }
    Grid {
        x0: lo.x,
        y0: lo.y,
        w: hi.x - lo.x + 1,
        h: hi.y - lo.y + 1,
    }
}

/// The vertex line of `edge` on a board holding the `kept` lines.
// DECISION: when the kept lines wall a vertex in (or sit on it where the
// line must turn), the line is routed again on a board of boxes only and
// crosses them: a vertex is the user's explicit request (P3-304), so it
// wins over CORE-8 neatness; with no way even then the line is
// unroutable (CORE-13, L1), e.g. for a vertex inside a box (L28).
fn route_pinned(ctx: &Ctx<'_>, grid: Grid, kept: &[Option<Line>], edge: usize, stop: &AtomicBool) -> Result<Option<Line>, Stopped> {
    let mut board = ctx.board(grid, &[]);
    let mut any = false;
    for line in kept.iter().flatten() {
        board.add(&cells_of(&line.points), line.edge.0 as usize, &line.end);
        any = true;
    }
    if let Some(line) = pinned_line(ctx, &board, edge, stop)? {
        return Ok(Some(line));
    }
    if any {
        return pinned_line(ctx, &ctx.board(grid, &[]), edge, stop);
    }
    Ok(None)
}

/// True when a port on `side` of `r` faces `v` (P3-306: CORE-9 port
/// rules applied to the vertex instead of the other box): `v` lies
/// beyond that side. A vertex inside the box's span faces every side.
fn faces(r: Rect, v: Point, side: Side) -> bool {
    let outside = v.y < r.min.y || v.y > r.max.y || v.x < r.min.x || v.x > r.max.x;
    !outside
        || match side {
            Side::N => v.y < r.min.y,
            Side::S => v.y > r.max.y,
            Side::W => v.x < r.min.x,
            Side::E => v.x > r.max.x,
        }
}

/// The free ports of `b` at `centre` facing `v`, each with the first free
/// cell outside it.
// DECISION: when no facing port is free (a person's single top port
// taken, say), every free port is offered instead, so the line is still
// routed through its vertices (P3-304 over P3-306).
fn facing_ports(board: &Board, b: &PlanBox, centre: Point, v: Point) -> Vec<(Spot, usize)> {
    let r = b.rect_at(centre);
    let all: Vec<(Spot, usize)> = spots(b, centre)
        .into_iter()
        .filter_map(|s| Some((s, board.port_free(s.port.at, side_dir(s.port.side))?)))
        .collect();
    let facing: Vec<(Spot, usize)> = all.iter().copied().filter(|(s, _)| faces(r, v, s.port.side)).collect();
    if facing.is_empty() { all } else { facing }
}

/// The free ports a line end may use on `b` at `centre`, each with the
/// first free cell outside it: the anchor's when the end is pinned
/// (P4-206), else those facing `v` (P3-306).
fn end_ports(board: &Board, b: &PlanBox, centre: Point, anchor: Option<Anchor>, v: Point) -> Vec<(Spot, usize)> {
    match anchor {
        Some(a) => anchor_spots(b, centre, a)
            .into_iter()
            .filter_map(|s| Some((s, board.port_free(s.port.at, side_dir(s.port.side))?)))
            .collect(),
        None => facing_ports(board, b, centre, v),
    }
}

/// Routes the vertex line of `edge` on `board` (P3-303..P3-306), or an
/// anchored line without vertices (P4-212).
fn pinned_line(ctx: &Ctx<'_>, board: &Board, edge: usize, stop: &AtomicBool) -> Result<Option<Line>, Stopped> {
    // `Ok(None)` when a step finds nothing, as the former `?` on `Option`.
    macro_rules! got {
        ($e:expr) => {
            match $e {
                Some(x) => x,
                None => return Ok(None),
            }
        };
    }
    let e = &ctx.plan.edges[edge];
    if e.source == e.destination {
        return Ok(None);
    }
    let g = board.grid;
    let sbox = &ctx.plan.boxes[e.source.0 as usize];
    let dbox = &ctx.plan.boxes[e.destination.0 as usize];
    let (scentre, dcentre) = (ctx.centres[e.source.0 as usize], ctx.centres[e.destination.0 as usize]);
    // With no vertex (an anchored line routed past other lines, P4-212),
    // each free end faces the other box.
    let first = e.vertices.first().copied().unwrap_or(dcentre);
    let last = e.vertices.last().copied().unwrap_or(scentre);
    let starts: Vec<Start> = end_ports(board, sbox, scentre, e.source_anchor, first)
        .into_iter()
        .map(|(s, c)| {
            let d = side_dir(s.port.side);
            let mut cost = START_OFFSET * u32::try_from(s.off).unwrap_or(0);
            if board.stub_clash(s.port.at, d, true) {
                cost += CLASH;
            }
            Start {
                cell: c,
                dir: d,
                cost,
                port: Some(s.port),
            }
        })
        .collect();
    let mut own = vec![false; g.len()];
    let mut cells: Vec<Point> = Vec::new();
    let mut source = None;
    let mut at = Start {
        cell: 0,
        dir: Dir::E,
        cost: 0,
        port: None,
    };
    for (k, &v) in e.vertices.iter().enumerate() {
        let goal = Goal::Cell(got!(g.index(v)));
        let found = if k == 0 {
            got!(leg(board, edge, &starts, false, &goal, &own, stop)?)
        } else {
            got!(leg(board, edge, &[at], true, &goal, &own, stop)?)
        };
        if k == 0 {
            let p = got!(found.start);
            source = Some(p);
            cells.push(p.at);
            cells.push(p.at.step(side_dir(p.side)));
            cells.extend_from_slice(&found.cells);
        } else {
            cells.extend_from_slice(&found.cells[1..]);
        }
        for &c in &cells {
            if let Some(i) = g.index(c) {
                own[i] = true;
            }
        }
        at = Start {
            cell: got!(g.index(v)),
            dir: found.dir,
            cost: 0,
            port: None,
        };
    }
    let mut goals: Vec<(usize, Port)> = end_ports(board, dbox, dcentre, e.destination_anchor, last)
        .into_iter()
        .map(|(s, c)| (c, s.port))
        .collect();
    goals.sort_unstable_by_key(|&(c, _)| c);
    let port = if e.vertices.is_empty() {
        let found = got!(leg(board, edge, &starts, false, &Goal::Ports(&goals), &own, stop)?);
        let p = got!(found.start);
        source = Some(p);
        cells.push(p.at);
        cells.push(p.at.step(side_dir(p.side)));
        cells.extend_from_slice(&found.cells);
        got!(found.end)
    } else {
        let found = got!(leg(board, edge, &[at], true, &Goal::Ports(&goals), &own, stop)?);
        cells.extend_from_slice(&found.cells[1..]);
        got!(found.end)
    };
    cells.push(port.at.step(side_dir(port.side)));
    cells.push(port.at);
    Ok(Some(Line {
        edge: edge_id(edge),
        points: vertices(&cells),
        source: got!(source),
        end: LineEnd::Arrow(port),
    }))
}

/// Where a leg search starts: a cell, the way the line heads there, the
/// cost so far and, for the first leg, the source port it leaves.
#[derive(Clone, Copy)]
struct Start {
    cell: usize,
    dir: Dir,
    cost: u32,
    port: Option<Port>,
}

/// Where a leg ends: on a vertex cell, or outside a destination port
/// heading into it (cells sorted ascending).
enum Goal<'a> {
    Cell(usize),
    Ports(&'a [(usize, Port)]),
    /// A bundle track's first cell, reached on a straight run heading at
    /// most 45° from the track's direction (P4-410); the cell may be the
    /// line's own.
    Entry(usize, Dir),
}

/// One routed leg: its cells from the start cell to the goal cell, the
/// way it arrives, the source port it left (first leg) and the
/// destination port it reaches (last leg).
struct Leg {
    cells: Vec<Point>,
    dir: Dir,
    start: Option<Port>,
    end: Option<Port>,
}

/// One leg search state.
#[derive(Clone, Copy)]
struct LegRec {
    cell: u32,
    dir: Dir,
    run: u8,
    cost: u32,
    parent: u32,
    start: u32,
}

/// A* for one leg, with `astar`'s steps and costs (no bus, no history).
/// With `free`, the first step from a start may turn by up to 135° (the
/// start is a vertex, P3-305), never back on itself, and never where the
/// vertex sits on another line (CORE-8: no crossing at a bend). The leg
/// keeps off the cells in `own` (the line's earlier legs). A vertex goal
/// is reached on a straight run of 20px or more (CORE-6).
#[allow(clippy::too_many_lines, clippy::many_single_char_names, clippy::too_many_arguments, clippy::similar_names)]
fn leg(board: &Board, edge: usize, starts: &[Start], free: bool, goal: &Goal<'_>, own: &[bool], stop: &AtomicBool) -> Result<Option<Leg>, Stopped> {
    leg_capped(board, edge, starts, free, goal, own, u32::MAX, None, stop)
}

/// `leg`, giving up (`None`) after `cap` pops.
// DECISION: the best cost per state is a hashed map, not a vector of 16
// entries per cell: a preview's rejoin leg pops a few thousand states,
// and filling a board-sized vector cost more than its search; only looked
// up, never iterated, so no output changes (CORE-25).
#[allow(clippy::too_many_lines, clippy::many_single_char_names, clippy::too_many_arguments, clippy::similar_names)]
fn leg_capped(
    board: &Board,
    edge: usize,
    starts: &[Start],
    free: bool,
    goal: &Goal<'_>,
    own: &[bool],
    cap: u32,
    bound: Option<&dyn Fn(u32, Dir) -> u32>,
    stop: &AtomicBool,
) -> Result<Option<Leg>, Stopped> {
    let g = board.grid;
    let net = board.edge_net[edge] + 1;
    let targets: Vec<Point> = match goal {
        Goal::Cell(c) | Goal::Entry(c, _) => vec![g.point(*c)],
        Goal::Ports(p) => p.iter().map(|&(c, _)| g.point(c)).collect(),
    };
    let Some(&first) = targets.first() else {
        return Ok(None);
    };
    let (mut lo, mut hi) = (first, first);
    for p in &targets {
        lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let h = |p: Point| -> u32 {
        let dx = u32::try_from((lo.x - p.x).max(0).max(p.x - hi.x)).unwrap_or(0);
        let dy = u32::try_from((lo.y - p.y).max(0).max(p.y - hi.y)).unwrap_or(0);
        H_MAX * dx.max(dy) + H_MIN * dx.min(dy)
    };
    let reached = |r: &LegRec| -> Option<Option<Port>> {
        match goal {
            Goal::Cell(c) => (r.cell as usize == *c && r.run == 2).then_some(None),
            Goal::Entry(c, d) => (r.cell as usize == *c && r.run == 2 && r.dir.turn_steps(*d) <= 1).then_some(None),
            Goal::Ports(p) => {
                let k = p.binary_search_by_key(&(r.cell as usize), |&(c, _)| c).ok()?;
                let port = p[k].1;
                (side_dir(port.side).opposite() == r.dir).then_some(Some(port))
            }
        }
    };
    let key = |cell: u32, dir: Dir, run: u8| (cell as usize * 8 + dir as usize) * 2 + usize::from(run - 1);
    let mut best: HashMap<usize, u32, BuildHasherDefault<KeyHasher>> = HashMap::default();
    let mut recs: Vec<LegRec> = Vec::new();
    // (priority, sequence, arena index): ties pop first in, so the search
    // order is fixed (CORE-25).
    let mut heap: std::collections::BinaryHeap<Reverse<(u32, u32, u32)>> = std::collections::BinaryHeap::new();
    let push = |r: LegRec, best: &mut HashMap<usize, u32, BuildHasherDefault<KeyHasher>>, recs: &mut Vec<LegRec>, heap: &mut std::collections::BinaryHeap<Reverse<(u32, u32, u32)>>| {
        let k = key(r.cell, r.dir, r.run);
        if best.get(&k).is_some_and(|&b| b <= r.cost) {
            return;
        }
        best.insert(k, r.cost);
        let idx = u32::try_from(recs.len()).expect("arena fits u32");
        recs.push(r);
        let prio = r.cost.saturating_add(bound.map_or_else(|| h(g.point(r.cell as usize)), |b| b(r.cell, r.dir)));
        heap.push(Reverse((prio, idx, idx)));
    };
    for (k, s) in starts.iter().enumerate() {
        let r = LegRec {
            cell: u32::try_from(s.cell).expect("cell fits u32"),
            dir: s.dir,
            run: 2,
            cost: s.cost,
            parent: u32::MAX,
            start: u32::try_from(k).expect("start index fits u32"),
        };
        push(r, &mut best, &mut recs, &mut heap);
    }
    let mut popped = 0u32;
    while let Some(Reverse((_, _, idx))) = heap.pop() {
        popped = popped.wrapping_add(1);
        tick(popped, stop)?;
        if popped > cap {
            #[cfg(test)]
            probe::work(popped % STOP_EVERY);
            #[cfg(test)]
            probe::with(|s| s.rejoin_pops += u64::from(popped));
            return Ok(None);
        }
        let r = recs[idx as usize];
        if best.get(&key(r.cell, r.dir, r.run)) != Some(&r.cost) {
            continue;
        }
        if let Some(end) = reached(&r) {
            #[cfg(test)]
            probe::work(popped % STOP_EVERY);
            #[cfg(test)]
            probe::with(|s| s.rejoin_pops += u64::from(popped));
            let mut rev = Vec::new();
            let mut i = idx;
            let start = loop {
                let q = recs[i as usize];
                rev.push(g.point(q.cell as usize));
                if q.parent == u32::MAX {
                    break starts[q.start as usize].port;
                }
                i = q.parent;
            };
            rev.reverse();
            return Ok(Some(Leg {
                cells: rev,
                dir: r.dir,
                start,
                end,
            }));
        }
        let ci = r.cell as usize;
        let c = g.point(ci);
        let on_other = board.cells[ci].net != 0 && board.cells[ci].net != net;
        let at_vertex = free && r.parent == u32::MAX;
        for nd in Dir::ALL {
            let t = r.dir.turn_steps(nd);
            let allowed = if at_vertex {
                t < 4 && (t == 0 || !on_other)
            } else {
                t == 0 || (t == 1 && r.run >= 2 && !on_other)
            };
            if !allowed {
                continue;
            }
            let n = c.step(nd);
            let Some(ni) = g.index(n) else { continue };
            // A vertex on an earlier leg of the same line is still a goal:
            // the line crosses itself there (the user asked for that point).
            let goal_cell = matches!(goal, Goal::Cell(c) | Goal::Entry(c, _) if *c == ni);
            let entry = matches!(goal, Goal::Entry(c, _) if *c == ni);
            // DECISION: a step onto a hand-placed vertex is exempt from
            // P4-600, so a vertex the user put beside a stroke is still
            // reached (P3-311); the router never puts one there itself.
            if board.cells[ni].blocked > 0
                || (own[ni] && !goal_cell)
                || !corridor_ok(board, ci, ni, nd, t != 0)
                || (!goal_cell && !along_ok(&board.along, ci, ni, nd))
            {
                continue;
            }
            let mut step = if nd.is_diagonal() {
                STEP_DIAGONAL
            } else {
                STEP_STRAIGHT
            };
            step += BEND * u32::from(t);
            if board.clash(c, nd) {
                step += CLASH;
            }
            if !board.fences.is_empty() {
                step += board.fence_cost(edge, c, n);
            }
            if nd.is_diagonal() {
                let (sq, o) = square(c, n);
                if let Some(si) = g.index(sq) {
                    let other = board.cells[si].diag[1 - o];
                    if other != 0 {
                        if board.edge_net[other as usize - 1] + 1 == net {
                            continue;
                        }
                        step += CROSS;
                    }
                }
            }
            if entry {
                // The track's own first cell (P4-410): no crossing there.
            } else if board.cells[ni].net != 0 {
                let ax = board.cells[ni].axes;
                if board.cells[ni].net == net || on_other || ax.count_ones() != 1 || ax & axis_bit(nd) != 0 {
                    continue;
                }
                step += CROSS;
            } else if board.near_other(ni, net) {
                step += NEAR;
            } else if board.cells[ni].lane & axis_bit(nd) != 0 {
                step = step * LANE_NUM / LANE_DEN;
            }
            let nr = LegRec {
                cell: u32::try_from(ni).expect("cell fits u32"),
                dir: nd,
                run: if t == 0 { 2 } else { 1 },
                cost: r.cost + step,
                parent: idx,
                start: r.start,
            };
            push(nr, &mut best, &mut recs, &mut heap);
        }
    }
    #[cfg(test)]
    probe::work(popped % STOP_EVERY);
    Ok(None)
}

/// The preview's carried lines (EDIT-25): each edge of `set` whose two
/// boxes moved by one same delta, routed in `previous`, keeps its
/// previous route translated by that delta when that route is still legal
/// on the new board: no step into a box or along a boundary stroke past
/// its port stubs (as `astar` steps), no shared stretch or touch with any
/// other line kept so far, and no more crossings or same-way clashes with
/// those lines than the previous route had. Accepted lines go into `kept`
/// in edge order and are returned, ascending; the rest stay searched.
/// Lines that join a trunk or are a trunk, vertex lines and `forced`
/// edges are never carried.
// DECISION: carry, do not search, the lines a multi-selection drag moves
// whole: the search would find their old route shifted, or a near one,
// at a cost of one search each; legality is checked on the router's own
// board and the line rules of CORE-7/8/12, and the crossing count may not
// rise, so the preview is no worse than before for these lines.
#[allow(clippy::too_many_arguments)]
fn carry(plan: &ViewPlan, centres: &[Point], previous: &Routes, set: &[usize], forced: &[usize], kept: &mut [Option<Line>]) -> Vec<usize> {
    let delta = |b: BoxId| {
        let (p, q) = (previous.centres[b.0 as usize], centres[b.0 as usize]);
        Point::new(q.x - p.x, q.y - p.y)
    };
    let is_trunk = |e: usize| previous.lines.iter().flatten().any(|l| l.joins() == Some(edge_id(e)));
    let mut cands: Vec<(usize, Line)> = Vec::new();
    for &e in set {
        let pe = &plan.edges[e];
        let d = delta(pe.source);
        let Some(old) = &previous.lines[e] else { continue };
        if d == Point::new(0, 0) || delta(pe.destination) != d || forced.contains(&e) || !pe.vertices.is_empty() || old.joins().is_some() || is_trunk(e) {
            continue;
        }
        let shift = |q: Point| Point::new(q.x + d.x, q.y + d.y);
        let end = match old.end {
            LineEnd::Arrow(port) => LineEnd::Arrow(Port { at: shift(port.at), side: port.side }),
            LineEnd::Join(_) => continue,
        };
        cands.push((
            e,
            Line {
                edge: old.edge,
                points: old.points.iter().map(|&q| shift(q)).collect(),
                source: Port { at: shift(old.source.at), side: old.source.side },
                end,
            },
        ));
    }
    if cands.is_empty() {
        return Vec::new();
    }
    let mut all: Vec<Option<Line>> = kept.to_vec();
    for (e, l) in &cands {
        all[*e] = Some(l.clone());
    }
    let grid = Grid::new(plan, centres, &all);
    let board = Board::new(plan, centres, grid, &[]);
    let segs = |l: &Line| -> Vec<Segment> { l.points.windows(2).map(|w| Segment::new(w[0], w[1])).collect() };
    let pair = |a: &Line, b: &Line| [Some(a.clone()), Some(b.clone())];
    let mut out = Vec::new();
    for (e, line) in cands {
        let cells = cells_of(&line.points);
        let n = cells.len();
        let on_board = cells.iter().enumerate().all(|(k, &q)| {
            let Some(i) = grid.index(q) else { return false };
            // The port cell and the stub cell next to it lie on the box's
            // margin (`Board::port_free`).
            let ok = k < 2 || k + 2 >= n || board.cells[i].blocked == 0;
            ok && (k == 0 || {
                let p = cells[k - 1];
                grid.index(p).is_some_and(|pi| Segment::new(p, q).dir().is_some_and(|d| along_ok(&board.along, pi, i, d)))
            })
        });
        if !on_board {
            continue;
        }
        let old = previous.lines[e].as_ref().expect("a candidate was routed");
        let mine = segs(&line);
        let legal = kept.iter().flatten().all(|other| {
            let theirs = segs(other);
            let apart = mine.iter().all(|s| theirs.iter().all(|t| matches!(s.contact(t), Contact::Disjoint | Contact::Cross)));
            // The other line's previous route: unchanged when it was kept,
            // its own previous route when it was carried.
            let other_old = previous.lines[other.edge.0 as usize].as_ref().unwrap_or(other);
            apart
                && crossings(&line.points, &other.points) <= crossings(&old.points, &other_old.points)
                && count_clashes(&pair(&line, other)) <= count_clashes(&pair(old, other_old))
        });
        if legal {
            kept[e] = Some(line);
            out.push(e);
        }
    }
    out
}

/// The edges a move from `previous.centres` to `centres` broke, ascending
/// (`reroute`'s set: touching a moved box, covered by one, or joined to
/// a broken trunk).
fn broken(plan: &ViewPlan, centres: &[Point], previous: &Routes) -> Vec<usize> {
    let moved: Vec<usize> = (0..plan.boxes.len())
        .filter(|&b| centres[b] != previous.centres[b])
        .collect();
    let mut in_set: Vec<bool> = plan
        .edges
        .iter()
        .map(|e| moved.contains(&(e.source.0 as usize)) || moved.contains(&(e.destination.0 as usize)))
        .collect();
    let zones: Vec<Rect> = moved
        .iter()
        .map(|&b| plan.boxes[b].rect_at(centres[b]).inflate(CLEARANCE_UNITS))
        .collect();
    for (e, line) in previous.lines.iter().enumerate() {
        if in_set[e] {
            continue;
        }
        if let Some(line) = line
            && line
                .segments()
                .iter()
                .any(|s| zones.iter().any(|z| s.passes_through(z)))
        {
            in_set[e] = true;
        }
    }
    loop {
        let mut changed = false;
        for (e, line) in previous.lines.iter().enumerate() {
            if in_set[e] {
                continue;
            }
            if let Some(t) = line.as_ref().and_then(Line::joins)
                && in_set.get(t.0 as usize).copied().unwrap_or(false)
            {
                in_set[e] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (0..plan.edges.len()).filter(|&e| in_set[e]).collect()
}

#[cfg(test)]
#[allow(clippy::many_single_char_names)]
// Tests name the plan `p`, the centres `c`, the routes `r`, as the router does.
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use crate::geom::{CLEARANCE_UNITS, Contact, Dir, MIN_SEGMENT_STEPS};
    use crate::model::{Shape, parse};
    use crate::view::{BoxId, PlanBox, PlanEdge, plan};

    /// One template box and one template edge, from a two-box context
    /// view written with roles only.
    fn template() -> (PlanBox, PlanEdge, ViewPlan) {
        let src = "workspace {\n  model {\n    operator = person \"Operator\"\n    shop = softwareSystem \"Shop\"\n    operator -> shop \"Uses\"\n  }\n  views {\n    systemContext shop \"ctx\" {\n      include *\n    }\n  }\n}\n";
        let model = match parse(src) {
            Ok(p) => p.model,
            Err(e) => panic!("{e}"),
        };
        let p = plan(&model, model.view("ctx").expect("view"));
        (p.boxes[1].clone(), p.edges[0].clone(), p)
    }

    /// A plan of `boxes` (shape, half width, half height, in grid units)
    /// and `edges` (source, destination box indices).
    fn mk(boxes: &[(Shape, i32, i32)], edges: &[(u32, u32)], bus: bool) -> ViewPlan {
        let (tb, te, mut p) = template();
        p.boxes = boxes
            .iter()
            .enumerate()
            .map(|(i, &(shape, hw, hh))| {
                let mut b = tb.clone();
                b.id = BoxId(u32::try_from(i).expect("small"));
                b.style.shape = shape;
                b.half_width = hw;
                b.half_height = hh;
                b.centre = None;
                b
            })
            .collect();
        p.edges = edges
            .iter()
            .enumerate()
            .map(|(i, &(s, d))| {
                let mut e = te.clone();
                e.id = EdgeId(u32::try_from(i).expect("small"));
                e.source = BoxId(s);
                e.destination = BoxId(d);
                e
            })
            .collect();
        p.options.bus = bus;
        p
    }

    const BOX: (Shape, i32, i32) = (Shape::Box, 12, 7);
    const PERSON: (Shape, i32, i32) = (Shape::Person, 10, 9);
    const CYL: (Shape, i32, i32) = (Shape::Cylinder, 10, 7);

    fn pt(x: i32, y: i32) -> Point {
        Point::new(x, y)
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn run(p: &ViewPlan, c: &[Point]) -> Routes {
        route(p, c, &RouteConfig::DEFAULT, &never()).expect("not stopped")
    }

    /// A cheaper run for the invariant sweeps over the grid plans: the
    /// invariants hold for every order and round, so fewer suffice.
    /// The editor's preview settings (`pipeline::PREVIEW_ROUTE`).
    const PREVIEW: RouteConfig = RouteConfig {
        orders: 2,
        max_iterations: 8,
    };

    const CHEAP: RouteConfig = RouteConfig {
        orders: 2,
        max_iterations: 1,
    };

    fn run_cheap(p: &ViewPlan, c: &[Point]) -> Routes {
        route(p, c, &CHEAP, &never()).expect("not stopped")
    }

    /// A 3x3 grid of boxes with a dense set of edges: the heavier plan.
    fn grid_plan(bus: bool) -> (ViewPlan, Vec<Point>) {
        let boxes = [BOX; 9];
        let edges = [
            (0, 1),
            (1, 2),
            (3, 4),
            (4, 5),
            (6, 7),
            (7, 8),
            (0, 3),
            (3, 6),
            (1, 4),
            (4, 7),
            (2, 5),
            (5, 8),
            (0, 4),
            (2, 4),
            (6, 4),
            (8, 4),
            (0, 8),
            (6, 2),
        ];
        let p = mk(&boxes, &edges, bus);
        let mut c = Vec::new();
        for row in 0..3 {
            for col in 0..3 {
                c.push(pt(col * 50, row * 40));
            }
        }
        (p, c)
    }

    /// The box sides' outward direction.
    fn out(side: Side) -> Dir {
        match side {
            Side::N => Dir::N,
            Side::E => Dir::E,
            Side::S => Dir::S,
            Side::W => Dir::W,
        }
    }

    /// CORE-6: octilinear, 45° bends, every segment >= 2 steps.
    fn check_core_6(r: &Routes) {
        for line in r.lines.iter().flatten() {
            assert!(line.points.len() >= 2);
            let segs = line.segments();
            for s in &segs {
                assert!(s.dir().is_some(), "{:?}: {s:?} not octilinear", line.edge);
                assert!(
                    s.len_steps().expect("octilinear") >= MIN_SEGMENT_STEPS,
                    "{:?}: {s:?} shorter than 20px",
                    line.edge
                );
            }
            for w in segs.windows(2) {
                let (a, b) = (w[0].dir().expect("dir"), w[1].dir().expect("dir"));
                assert_eq!(a.turn_steps(b), 1, "{:?}: bend not 45°", line.edge);
            }
        }
    }

    /// CORE-7: through no box but its own ends, 10px from other boxes.
    fn check_core_7(p: &ViewPlan, c: &[Point], r: &Routes) {
        for line in r.lines.iter().flatten() {
            let e = &p.edges[line.edge.0 as usize];
            for b in &p.boxes {
                let rect = b.rect_at(c[b.id.0 as usize]);
                let own = b.id == e.source || b.id == e.destination;
                let test = if own {
                    rect
                } else {
                    rect.inflate(CLEARANCE_UNITS)
                };
                for s in line.segments() {
                    assert!(
                        !s.passes_through(&test),
                        "{:?} passes through box {:?}",
                        line.edge,
                        b.id
                    );
                }
            }
        }
    }

    /// CORE-8: no shared segment; lines meet only by a proper crossing
    /// (never at a bend), or where a joined line ends on its trunk.
    fn check_core_8(r: &Routes) {
        let lines: Vec<&Line> = r.lines.iter().flatten().collect();
        for (i, a) in lines.iter().enumerate() {
            for b in &lines[i + 1..] {
                for s in a.segments() {
                    for t in b.segments() {
                        match s.contact(&t) {
                            Contact::Disjoint | Contact::Cross => {}
                            Contact::Overlap => panic!("{:?} and {:?} share", a.edge, b.edge),
                            Contact::Touch => {
                                let dot = |l: &Line, other: &Line| match l.end {
                                    LineEnd::Join(j) => {
                                        j.trunk == other.edge
                                            && (s.a == j.at || s.b == j.at || t.a == j.at || t.b == j.at)
                                    }
                                    _ => false,
                                };
                                assert!(
                                    dot(a, b) || dot(b, a),
                                    "{:?} and {:?} touch at a bend or end",
                                    a.edge,
                                    b.edge
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// CORE-9: own port per line per box, on the outline, square to the
    /// side, 20px from corners; person and cylinder rules.
    fn check_core_9(p: &ViewPlan, c: &[Point], r: &Routes) {
        let mut used = BTreeSet::new();
        let mut person_tops: Vec<u32> = vec![0; p.boxes.len()];
        for line in r.lines.iter().flatten() {
            let e = &p.edges[line.edge.0 as usize];
            let mut ends = vec![(e.source, line.source, true)];
            if let LineEnd::Arrow(port) = line.end {
                ends.push((e.destination, port, false));
            }
            for (bid, port, is_source) in ends {
                let b = &p.boxes[bid.0 as usize];
                let centre = c[bid.0 as usize];
                let rect = b.rect_at(centre);
                assert!(used.insert((bid, port.at)), "two lines share a port on {bid:?}");
                let on_side = match port.side {
                    Side::N => port.at.y == rect.min.y,
                    Side::S => port.at.y == rect.max.y,
                    Side::W => port.at.x == rect.min.x,
                    Side::E => port.at.x == rect.max.x,
                };
                assert!(on_side, "port not on its side");
                match port.side {
                    Side::N | Side::S => {
                        assert!(port.at.x >= rect.min.x + 2 && port.at.x <= rect.max.x - 2);
                    }
                    Side::E | Side::W => {
                        assert!(port.at.y >= rect.min.y + 2 && port.at.y <= rect.max.y - 2);
                    }
                }
                let segs = line.segments();
                let seg = if is_source { segs[0] } else { segs[segs.len() - 1] };
                let want = if is_source {
                    out(port.side)
                } else {
                    out(port.side).opposite()
                };
                assert_eq!(seg.dir(), Some(want), "line not square to the side");
                match b.style.shape {
                    Shape::Person => {
                        if port.side == Side::N {
                            assert_eq!(port.at.x, centre.x, "person top port off centre");
                            person_tops[bid.0 as usize] += 1;
                        }
                        if matches!(port.side, Side::E | Side::W) {
                            assert!(port.at.y >= rect.min.y + 8, "person side port above 80px");
                        }
                    }
                    Shape::Cylinder => {
                        if matches!(port.side, Side::N | Side::S) {
                            assert!((port.at.x - centre.x).abs() <= 3, "cylinder port off centre");
                        }
                    }
                    Shape::Box => {}
                    // The helper builds phase-1 shapes only; P2-414 checks
                    // every shape in tests/styles_themes.rs.
                    _ => unreachable!("phase-1 shapes only"),
                }
            }
            assert_eq!(line.points[0], line.source.at);
        }
        assert!(person_tops.iter().all(|&n| n <= 1), "person has two top ports");
    }

    /// CORE-10 and the bookkeeping invariants of `Routes`.
    fn check_core_10(p: &ViewPlan, r: &Routes) {
        assert_eq!(r.lines.len(), p.edges.len());
        for (i, line) in r.lines.iter().enumerate() {
            let Some(line) = line else { continue };
            assert_eq!(line.edge, EdgeId(u32::try_from(i).expect("small")));
            let last = *line.points.last().expect("points");
            match line.end {
                LineEnd::Arrow(port) => {
                    assert_eq!(port.at, last);
                    assert_eq!(line.joins(), None);
                }
                LineEnd::Join(j) => {
                    assert!(p.options.bus, "join with bus off");
                    assert_eq!(j.at, last);
                    assert_eq!(line.joins(), Some(j.trunk));
                    let trunk = r.line(j.trunk).expect("trunk routed");
                    assert!(matches!(trunk.end, LineEnd::Arrow(_)), "join chain");
                    let (me, tr) = (&p.edges[i], &p.edges[j.trunk.0 as usize]);
                    assert_eq!(me.destination, tr.destination);
                    assert!(trunk.segments().iter().any(|s| s.contact(&Segment::new(last, last)) != Contact::Disjoint));
                }
            }
        }
        let none: Vec<EdgeId> = (0..r.lines.len())
            .filter(|&i| r.lines[i].is_none())
            .map(|i| EdgeId(u32::try_from(i).expect("small")))
            .collect();
        assert_eq!(r.unroutable(), none);
    }

    fn check_all(p: &ViewPlan, c: &[Point], r: &Routes) {
        check_core_6(r);
        check_core_7(p, c, r);
        check_core_8(r);
        check_core_9(p, c, r);
        check_core_10(p, r);
        assert_eq!(r.crossings, l4_count(r));
        assert_eq!(r.centres, c[..p.boxes.len()].to_vec());
    }

    /// The L4 rule written out independently: every pair of lines,
    /// minus a joined line's last segment against its own trunk.
    fn l4_count(r: &Routes) -> u32 {
        let lines: Vec<&Line> = r.lines.iter().flatten().collect();
        let mut n = 0;
        for (i, a) in lines.iter().enumerate() {
            for b in &lines[i + 1..] {
                n += if a.joins() == Some(b.edge) {
                    crate::geom::crossings(&b.points, &a.points[..a.points.len() - 1])
                } else if b.joins() == Some(a.edge) {
                    crate::geom::crossings(&a.points, &b.points[..b.points.len() - 1])
                } else {
                    crate::geom::crossings(&a.points, &b.points)
                };
            }
        }
        n
    }

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
        // A diagonal offset forces bends.
        let p = mk(&[BOX, BOX], &[(0, 1)], false);
        let c = [pt(0, 0), pt(60, 37)];
        let r = run(&p, &c);
        let line = r.line(EdgeId(0)).expect("routed");
        assert!(line.points.len() > 2, "an offset pair needs bends");
        check_core_6(&r);
        let (p, c) = grid_plan(false);
        let r = run_cheap(&p, &c);
        assert!(r.unroutable().is_empty());
        check_core_6(&r);
    }

    #[test]
    fn core_7_never_through_box() {
        // Also 10px clearance from other boxes; boxes at the canvas edge:
        // lines may leave the bounding rectangle of all boxes, never
        // clipped (Review Focus).
        // A blocker sits right between the two ends.
        let p = mk(&[BOX, BOX, BOX], &[(0, 1)], false);
        let c = [pt(0, 0), pt(80, 0), pt(40, 0)];
        let r = run(&p, &c);
        assert!(r.line(EdgeId(0)).is_some());
        check_core_7(&p, &c, &r);
        // Two boxes touching on top of a wide one: the line between the
        // outer two must go around, beyond the boxes' bounding box.
        let p = mk(&[BOX, BOX, (Shape::Box, 40, 7)], &[(0, 1)], false);
        let c = [pt(-30, 0), pt(30, 0), pt(0, 0)];
        let r = run(&p, &c);
        check_all(&p, &c, &r);
        if let Some(line) = r.line(EdgeId(0)) {
            let (lo, hi) = (pt(-40, -7), pt(40, 7));
            let outside = line
                .points
                .iter()
                .any(|q| q.x < lo.x || q.x > hi.x || q.y < lo.y || q.y > hi.y);
            assert!(outside);
        }
        let (p, c) = grid_plan(false);
        let r = run_cheap(&p, &c);
        check_core_7(&p, &c, &r);
    }

    #[test]
    fn core_8_no_shared_segments() {
        let (p, c) = grid_plan(false);
        let r = run_cheap(&p, &c);
        check_core_8(&r);
        let (p, c) = grid_plan(true);
        let r = run_cheap(&p, &c);
        check_core_8(&r);
        // Two lines on the same ordered pair: two lines, nothing shared.
        let p = mk(&[BOX, BOX], &[(0, 1), (0, 1)], false);
        let c = [pt(0, 0), pt(60, 0)];
        let r = run(&p, &c);
        assert!(r.unroutable().is_empty());
        check_all(&p, &c, &r);
    }

    /// Three sources high above one target: lines that can share.
    fn fan_in(bus: bool) -> (ViewPlan, Vec<Point>) {
        let p = mk(&[BOX, BOX, BOX, (Shape::Box, 30, 7)], &[(0, 3), (1, 3), (2, 3)], bus);
        let c = vec![pt(-60, 0), pt(0, 0), pt(60, 0), pt(0, 60)];
        (p, c)
    }

    #[test]
    fn core_8_bus_join_with_dot() {
        // Join after 90px of own run, at <= 90° to the trunk, same box and
        // same side only (Review Focus); the join is not a crossing; the
        // trunk ends in an arrowhead (no chains).
        let (p, c) = fan_in(true);
        let r = run(&p, &c);
        check_all(&p, &c, &r);
        let joined: Vec<&Line> = r.lines.iter().flatten().filter(|l| l.joins().is_some()).collect();
        assert!(!joined.is_empty(), "bus on: a line joins");
        for line in joined {
            let LineEnd::Join(j) = line.end else { unreachable!() };
            // 90px of own run: summed squared lengths are exact per segment.
            let run_px: f64 = line
                .segments()
                .iter()
                .map(|s| {
                    let sq = s.len_px_squared().expect("oct");
                    f64::from(u32::try_from(sq).expect("short")).sqrt()
                })
                .sum();
            assert!(run_px >= 90.0, "joined after {run_px}px");
            let trunk = r.line(j.trunk).expect("trunk");
            let LineEnd::Arrow(tport) = trunk.end else { panic!("chain") };
            let at = Segment::new(j.at, j.at);
            let tseg = trunk
                .segments()
                .into_iter()
                .find(|s| s.contact(&at) != Contact::Disjoint)
                .expect("dot on trunk");
            let last = *line.segments().last().expect("seg");
            assert!(
                last.dir().expect("d").turn_steps(tseg.dir().expect("d")) <= 2,
                "join over 90°"
            );
            // Same side: the dot is beyond the trunk's arrival side.
            let rect = p.boxes[3].rect_at(c[3]);
            let beyond = match tport.side {
                Side::N => j.at.y < rect.min.y,
                Side::S => j.at.y > rect.max.y,
                Side::W => j.at.x < rect.min.x,
                Side::E => j.at.x > rect.max.x,
            };
            assert!(beyond, "join not on the trunk's side");
        }
        assert_eq!(r.crossings, 0, "a join is not a crossing");
    }

    #[test]
    fn core_8_no_join_when_bus_off() {
        // Same plan as core_8_bus_join_with_dot with `options.bus` off:
        // every routed line ends in `LineEnd::Arrow`.
        let (p, c) = fan_in(false);
        let r = run(&p, &c);
        check_all(&p, &c, &r);
        assert!(r.unroutable().is_empty());
        for line in r.lines.iter().flatten() {
            assert!(matches!(line.end, LineEnd::Arrow(_)));
        }
    }

    /// A cross: left to right and top to bottom, with a long wall box
    /// running outwards from each end, so going round costs far more
    /// than one crossing.
    fn cross_plan() -> (ViewPlan, Vec<Point>) {
        let (flat, thin) = ((Shape::Box, 80, 3), (Shape::Box, 3, 80));
        let p = mk(
            &[BOX, BOX, BOX, BOX, flat, flat, thin, thin],
            &[(0, 1), (2, 3)],
            false,
        );
        let c = vec![
            pt(-40, 0),
            pt(40, 0),
            pt(0, -30),
            pt(0, 30),
            pt(-135, 0),
            pt(135, 0),
            pt(0, -120),
            pt(0, 120),
        ];
        (p, c)
    }

    #[test]
    fn core_8_crossing_never_at_bend() {
        let (p, c) = cross_plan();
        let r = run(&p, &c);
        check_all(&p, &c, &r);
        assert!(r.crossings >= 1, "{r:?}");
        let (a, b) = (r.line(EdgeId(0)).expect("a"), r.line(EdgeId(1)).expect("b"));
        let mut proper = 0;
        for s in a.segments() {
            for t in b.segments() {
                match s.contact(&t) {
                    Contact::Cross => proper += 1,
                    Contact::Disjoint => {}
                    other => panic!("lines meet by {other:?}"),
                }
            }
        }
        assert_eq!(proper, r.crossings);
        let (p, c) = grid_plan(false);
        check_core_8(&run_cheap(&p, &c));
    }

    #[test]
    fn core_9_one_port_per_line() {
        // Also A->B and B->A: two lines, separate ports (Review Focus);
        // square to the side, >= 20px from a corner; person and cylinder
        // port rules.
        let p = mk(&[BOX, BOX], &[(0, 1), (1, 0)], false);
        let c = [pt(0, 0), pt(60, 0)];
        let r = run(&p, &c);
        assert!(r.unroutable().is_empty());
        check_all(&p, &c, &r);
        // A person above three boxes and a cylinder below: the person's
        // single top port, its side ports below the head, the cylinder's
        // narrow top and bottom ports.
        let p = mk(
            &[PERSON, BOX, BOX, CYL, BOX],
            &[(0, 1), (0, 2), (0, 3), (4, 0), (1, 3), (2, 3), (3, 4)],
            false,
        );
        let c = [pt(0, 0), pt(-50, 40), pt(50, 40), pt(0, 80), pt(0, -60)];
        let r = run(&p, &c);
        assert!(r.unroutable().is_empty());
        check_all(&p, &c, &r);
        let (p, c) = grid_plan(true);
        check_all(&p, &c, &run_cheap(&p, &c));
    }

    #[test]
    fn core_10_one_arrowhead_or_join_dot() {
        // Route's part of CORE-10; chevrons are scene's.
        for bus in [false, true] {
            let (p, c) = fan_in(bus);
            check_core_10(&p, &run(&p, &c));
            let (p, c) = grid_plan(bus);
            check_core_10(&p, &run_cheap(&p, &c));
        }
    }

    #[test]
    fn core_11_crossings_counted_like_l4() {
        // The example quality bar itself is core_11_example_crossings_no_worse
        // in tests/golden_route.rs.
        let (p, c) = cross_plan();
        let r = run(&p, &c);
        assert_eq!(r.crossings, 1);
        assert_eq!(r.crossings, l4_count(&r));
        for bus in [false, true] {
            let (p, c) = grid_plan(bus);
            let r = run_cheap(&p, &c);
            assert_eq!(r.crossings, l4_count(&r));
            let (p, c) = fan_in(bus);
            let r = run_cheap(&p, &c);
            assert_eq!(r.crossings, l4_count(&r));
        }
    }

    /// Two tall boxes side by side with three lines between them.
    fn lane_plan() -> (ViewPlan, Vec<Point>) {
        let tall = (Shape::Box, 10, 14);
        let p = mk(&[tall, tall], &[(0, 1), (0, 1), (0, 1)], false);
        (p, vec![pt(0, 0), pt(80, 0)])
    }

    /// Every perpendicular gap, in cells, between two axis runs of
    /// different lines that head the same way, share a stretch and lie at
    /// most 3 cells (30px) apart. Works on raw vertex lists, not on
    /// `Routes::lanes`, so a 10px or 30px neighbour shows up (CORE-12).
    // DECISION: only gaps of 1..=3 cells count as "running beside": 0 is
    // an overlap (CORE-6 checks), and runs 40px or more apart are
    // separate lines, not a lane; diagonal runs are left out as in
    // `lane_runs` (never exactly 20px apart on the grid).
    fn same_way_gaps(lines: &[Vec<Point>]) -> Vec<i32> {
        let runs = |pts: &Vec<Point>| -> Vec<(Dir, Point, Point)> {
            pts.windows(2)
                .filter_map(|w| {
                    let d = Segment::new(w[0], w[1]).dir()?;
                    (!d.is_diagonal()).then_some((d, w[0], w[1]))
                })
                .collect()
        };
        let mut gaps = Vec::new();
        for (i, a) in lines.iter().enumerate() {
            for b in &lines[i + 1..] {
                for &(da, a0, a1) in &runs(a) {
                    for &(db, b0, b1) in &runs(b) {
                        if da != db {
                            continue;
                        }
                        let horizontal = matches!(da, Dir::E | Dir::W);
                        let (across, along) = if horizontal {
                            ((a0.y - b0.y).abs(), [a0.x, a1.x, b0.x, b1.x])
                        } else {
                            ((a0.x - b0.x).abs(), [a0.y, a1.y, b0.y, b1.y])
                        };
                        let lo = along[0].min(along[1]).max(along[2].min(along[3]));
                        let hi = along[0].max(along[1]).min(along[2].max(along[3]));
                        if hi > lo && (1..=3).contains(&across) {
                            gaps.push(across);
                        }
                    }
                }
            }
        }
        gaps
    }

    fn routed_points(r: &Routes) -> Vec<Vec<Point>> {
        r.lines.iter().flatten().map(|l| l.points.clone()).collect()
    }

    #[test]
    fn core_12_gap_scanner_catches_off_lane_runs() {
        // The scanner itself must fail a 10px or 30px same-way neighbour,
        // even next to a correct 20px pair.
        let run = |y: i32| vec![pt(0, y), pt(10, y)];
        assert_eq!(same_way_gaps(&[run(0), run(2)]), vec![2]);
        assert!(same_way_gaps(&[run(0), run(2), run(3)]).contains(&1));
        assert!(same_way_gaps(&[run(0), run(3)]).contains(&3));
        // Opposite ways and disjoint stretches are not lanes.
        let back = vec![pt(10, 1), pt(0, 1)];
        assert!(same_way_gaps(&[run(0), back]).is_empty());
        assert!(same_way_gaps(&[run(0), vec![pt(20, 1), pt(30, 1)]]).is_empty());
    }

    #[test]
    fn core_12_parallel_lines_constant_gap() {
        // Every same-way run beside another runs exactly 20px from it; the
        // example lane lengths against the prototype are
        // core_12_example_lane_length_no_worse in tests/golden_route.rs
        // (numbers in SRS CORE-12).
        let (p, c) = lane_plan();
        let r = run(&p, &c);
        check_all(&p, &c, &r);
        assert_eq!(r.lines.iter().flatten().count(), 3);
        let gaps = same_way_gaps(&routed_points(&r));
        assert!(gaps.len() >= 2, "three lines run as lanes: {gaps:?}");
        assert!(gaps.iter().all(|&g| g == 2), "same-way gaps are 20px: {gaps:?}");
        for bus in [false, true] {
            for (p, c) in [grid_plan(bus), fan_in(bus)] {
                let r = run(&p, &c);
                let gaps = same_way_gaps(&routed_points(&r));
                assert!(gaps.iter().all(|&g| g == 2), "bus {bus}: {gaps:?}");
            }
        }
    }

    #[test]
    fn core_12_adjacent_ports_never_run_10px_apart() {
        // Two lines from box 0 to box 1 must not leave adjacent ports on
        // one side and run 10px apart (found by a random sweep).
        let p = mk(
            &[(Shape::Box, 9, 4), (Shape::Cylinder, 8, 5), (Shape::Box, 13, 9)],
            &[(0, 1), (1, 2), (0, 2), (2, 1), (0, 1)],
            false,
        );
        let c = [pt(40, 10), pt(11, 6), pt(73, 58)];
        let r = run(&p, &c);
        check_all(&p, &c, &r);
        let gaps = same_way_gaps(&routed_points(&r));
        assert!(gaps.iter().all(|&g| g == 2), "same-way gaps are 20px: {gaps:?}");
    }

    /// A small seeded random plan: three to five boxes of mixed shapes
    /// and sizes, not overlapping, and four to seven edges.
    fn random_plan(seed: u64, bus: bool) -> (ViewPlan, Vec<Point>) {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = |n: u64| -> i32 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            i32::try_from(s % n).expect("small")
        };
        let shapes = [Shape::Box, Shape::Cylinder, Shape::Person];
        let n = 3 + next(3);
        let mut boxes = Vec::new();
        let mut c: Vec<Point> = Vec::new();
        while boxes.len() < usize::try_from(n).expect("small") {
            let shape = shapes[usize::try_from(next(3)).expect("small")];
            let b = (shape, 6 + next(9), 4 + next(6));
            let at = pt(next(100), next(80));
            let clear = boxes.iter().zip(&c).all(|(o, q): (&(Shape, i32, i32), &Point)| {
                (at.x - q.x).abs() > b.1 + o.1 + 8 || (at.y - q.y).abs() > b.2 + o.2 + 8
            });
            if clear {
                boxes.push(b);
                c.push(at);
            }
        }
        let k = u32::try_from(n).expect("small");
        let mut edges = Vec::new();
        for _ in 0..4 + next(4) {
            let a = u32::try_from(next(u64::from(k))).expect("small");
            let d = (a + 1 + u32::try_from(next(u64::from(k - 1))).expect("small")) % k;
            edges.push((a, d));
        }
        (mk(&boxes, &edges, bus), c)
    }

    #[test]
    fn core_12_random_plans_same_way_gaps_are_20px() {
        // Every same-way neighbour 10px or 30px away breaks CORE-12; a
        // seeded sweep of small plans, bus off and on.
        for seed in 0..120 {
            for bus in [false, true] {
                let (p, c) = random_plan(seed, bus);
                let r = run_cheap(&p, &c);
                let gaps = same_way_gaps(&routed_points(&r));
                assert!(
                    gaps.iter().all(|&g| g == 2),
                    "seed {seed} bus {bus}: same-way gaps {gaps:?}"
                );
            }
        }
    }

    /// A plan where the plain pass of the ascending order crosses once
    /// and rip-up and retry finds a routing without the crossing (found
    /// by a seeded search over small random plans).
    fn rip_up_plan() -> (ViewPlan, Vec<Point>) {
        let p = mk(
            &[
                (Shape::Box, 7, 6),
                (Shape::Box, 5, 5),
                (Shape::Box, 7, 3),
                (Shape::Box, 9, 3),
                (Shape::Box, 4, 6),
            ],
            &[(3, 4), (1, 2), (1, 3), (2, 4), (0, 2)],
            false,
        );
        let c = vec![pt(52, 37), pt(55, 13), pt(4, 10), pt(15, 44), pt(67, 6)];
        (p, c)
    }

    #[test]
    fn core_13_rip_up_resolves_conflict() {
        let (p, c) = rip_up_plan();
        let one = RouteConfig {
            orders: 1,
            max_iterations: 0,
        };
        let first = route(&p, &c, &one, &never()).expect("not stopped");
        let retry = RouteConfig {
            orders: 1,
            max_iterations: 8,
        };
        let r = route(&p, &c, &retry, &never()).expect("not stopped");
        assert!(
            !first.unroutable().is_empty() || first.crossings > 0,
            "the plain pass has a conflict"
        );
        assert!(
            r.unroutable().len() < first.unroutable().len()
                || (r.unroutable() == first.unroutable() && r.crossings < first.crossings),
            "rip-up improves on the plain pass"
        );
        check_all(&p, &c, &r);
    }

    #[test]
    fn core_13_unroutable_reported_by_id() {
        // Two boxes too close for any path: reported by id, the rest
        // still route (Review Focus). Asserts the invariants:
        // `unroutable()` == indices of `None` in `lines`, and
        // `lines[i].edge == EdgeId(i)` for every routed line.
        // Box 1 sits inside box 0: no port of box 1 is reachable.
        let p = mk(
            &[(Shape::Box, 20, 12), (Shape::Box, 8, 4), BOX, BOX],
            &[(2, 3), (0, 1), (3, 2)],
            false,
        );
        let c = [pt(0, 0), pt(0, 0), pt(-60, 60), pt(60, 60)];
        let r = run(&p, &c);
        assert_eq!(r.unroutable(), vec![EdgeId(1)]);
        assert!(r.line(EdgeId(1)).is_none());
        assert!(r.line(EdgeId(0)).is_some() && r.line(EdgeId(2)).is_some());
        assert!(r.line(EdgeId(9)).is_none(), "out of range");
        check_core_10(&p, &r);
    }

    #[test]
    fn core_25_same_input_same_routes() {
        for bus in [false, true] {
            let (p, c) = grid_plan(bus);
            let a = run_cheap(&p, &c);
            let b = run_cheap(&p, &c);
            assert_eq!(a, b);
            let moved: Vec<Point> = c.iter().map(|q| pt(q.x + 3, q.y - 2)).collect();
            let a = reroute(&p, &moved, &a, &CHEAP, &never()).expect("ok");
            let b = reroute(&p, &moved, &b, &CHEAP, &never()).expect("ok");
            assert_eq!(a, b);
        }
    }

    #[test]
    fn core_32_iteration_cap_always_returns() {
        // Also `max_iterations: 0` returns one pass with no rip-up.
        let (p, c) = grid_plan(true);
        for max_iterations in [0, 1, 3] {
            let cfg = RouteConfig {
                orders: 2,
                max_iterations,
            };
            let r = route(&p, &c, &cfg, &never()).expect("returns");
            check_all(&p, &c, &r);
        }
        let (p, c) = rip_up_plan();
        let cfg = RouteConfig {
            orders: 1,
            max_iterations: 0,
        };
        let r = route(&p, &c, &cfg, &never()).expect("returns");
        check_core_10(&p, &r);
    }

    /// A heavy plan: a 6x5 grid of boxes, every neighbour linked both
    /// ways plus long diagonals.
    fn heavy_plan() -> (ViewPlan, Vec<Point>) {
        let (cols, rows) = (6u32, 5u32);
        let boxes = vec![BOX; (cols * rows) as usize];
        let mut edges = Vec::new();
        for r in 0..rows {
            for k in 0..cols {
                let i = r * cols + k;
                if k + 1 < cols {
                    edges.push((i, i + 1));
                    edges.push((i + 1, i));
                }
                if r + 1 < rows {
                    edges.push((i, i + cols));
                }
                if k + 2 < cols && r + 2 < rows {
                    edges.push((i, i + 2 * cols + 2));
                }
            }
        }
        let p = mk(&boxes, &edges, true);
        let mut c = Vec::new();
        for r in 0..rows {
            for k in 0..cols {
                c.push(pt(i32::try_from(k).expect("s") * 45, i32::try_from(r).expect("s") * 35));
            }
        }
        (p, c)
    }

    #[test]
    fn edit_25_stopped_route_returns_promptly_without_routes() {
        // A flag already set, and one set from another thread while a
        // heavy plan routes: `route` and `reroute` return `Err(Stopped)`
        // promptly (within one rip-up round or edge order) and yield no
        // routes. The same inputs with a flag never set give `Ok` with
        // the routes of an unstopped run (CORE-25).
        let (p, c) = grid_plan(false);
        let set = AtomicBool::new(true);
        assert_eq!(route(&p, &c, &RouteConfig::DEFAULT, &set), Err(Stopped));
        let prev = run_cheap(&p, &c);
        let mut moved = c.clone();
        moved[4] = pt(moved[4].x + 2, moved[4].y);
        assert_eq!(
            reroute(&p, &moved, &prev, &RouteConfig::DEFAULT, &set),
            Err(Stopped)
        );
        assert_eq!(Stopped.to_string(), "routing stopped");

        // Never set: the same routes as an unstopped run.
        assert_eq!(run_cheap(&p, &c), prev);

        // Set from another thread while a heavy plan routes.
        let (hp, hc) = heavy_plan();
        let delay = Duration::from_millis(20);
        let flag = AtomicBool::new(false);
        let t0 = Instant::now();
        let got = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(delay);
                flag.store(true, Ordering::Relaxed);
            });
            route(&hp, &hc, &RouteConfig::DEFAULT, &flag)
        });
        assert_eq!(got, Err(Stopped));
        assert!(t0.elapsed() < delay + Duration::from_secs(2), "stopped late");

        let prev = Routes {
            lines: vec![None; hp.edges.len()],
            centres: hc.clone(),
            crossings: 0,
            bundles: Vec::new(),
        };
        // Every box moved: every edge is re-routed.
        let all: Vec<Point> = hc.iter().map(|q| pt(q.x, q.y + 1)).collect();
        let flag = AtomicBool::new(false);
        let t0 = Instant::now();
        let got = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(delay);
                flag.store(true, Ordering::Relaxed);
            });
            reroute(&hp, &all, &prev, &RouteConfig::DEFAULT, &flag)
        });
        assert_eq!(got, Err(Stopped));
        assert!(t0.elapsed() < delay + Duration::from_secs(2), "stopped late");
    }

    #[test]
    fn edit_25_stop_read_within_a_few_thousand_states() {
        // The stop flag is read inside the long loops (A*, `cost_from`,
        // the vertex legs), not only between searches: however long one
        // search runs, at most a few thousand search states are handled
        // between two reads, so a stale preview gives way within about a
        // millisecond (EDIT-25). Counted in states, not time, so the
        // bound holds on any machine.
        let limit = u64::from(4 * STOP_EVERY);
        let one = RouteConfig {
            orders: 1,
            max_iterations: 0,
        };
        // A large view (cost-bounded A* in windows) and a small one
        // (octile attempts up to POP_LIMIT, then `cost_from`).
        for (name, (p, c)) in [("heavy", heavy_plan()), ("grid", grid_plan(false))] {
            probe::reset();
            route(&p, &c, &one, &never()).expect("not stopped");
            let st = probe::take();
            assert!(st.pops > limit, "{name}: the run is long enough to tell ({} pops)", st.pops);
            assert!(st.max_work <= limit, "{name}: {} states between two stop reads", st.max_work);
        }
        // A vertex inside a box: the leg search floods the whole board
        // before the line is given up (P3-304, L28).
        let (mut p, c) = grid_plan(false);
        p.edges[0].vertices = vec![c[4]];
        probe::reset();
        route(&p, &c, &one, &never()).expect("not stopped");
        let st = probe::take();
        assert!(st.max_work <= limit, "vertex legs: {} states between two stop reads", st.max_work);
    }

    #[test]
    fn edit_25_preview_stops_once_no_worse_than_before_the_drag() {
        // A preview (`reroute`) whose first pass is no worse than the
        // routes the drag started from (unroutable, clashes and crossings,
        // each no higher) keeps it: no rip-up round, no other order. On a
        // view with crossings the absolute budget (`maxCrossings` 0) is
        // never met, so without this every preview ran every round.
        let cfg = RouteConfig {
            orders: 2,
            max_iterations: 8,
        };
        for (name, (p, c)) in [("grid", grid_plan(false)), ("heavy", heavy_plan())] {
            let prev = run_cheap(&p, &c);
            let base = Score::of(&prev.lines);
            assert!(base.crossings > 0, "{name}: a view with crossings");
            let mut moved = c.clone();
            moved[0] = pt(c[0].x + 1, c[0].y);
            probe::reset();
            let r = reroute(&p, &moved, &prev, &cfg, &never()).expect("ok");
            let st = probe::take();
            let (x, k, u) = st.first_pass[0];
            assert!(u <= base.unroutable && k <= base.clashes && x <= base.crossings, "{name}: first pass no worse ({x}/{k}/{u})");
            assert_eq!(st.passes, 1, "{name}: one pass");
            check_all(&p, &moved, &r);
            // The full route is not a preview: its rounds still run.
            probe::reset();
            route(&p, &moved, &cfg, &never()).expect("ok");
            assert!(probe::take().passes > 1, "{name}: full route keeps its rounds");
        }
    }

    /// A box walled in by four others with only a closed channel round
    /// it (wide enough to enter every port), and a box outside linked to
    /// it: the line has no path.
    fn walled_plan() -> (ViewPlan, Vec<Point>) {
        let p = mk(
            &[BOX, (Shape::Box, 30, 3), (Shape::Box, 30, 3), (Shape::Box, 3, 14), (Shape::Box, 3, 14), BOX],
            &[(5, 0)],
            false,
        );
        let c = vec![pt(0, 0), pt(0, -15), pt(0, 15), pt(-23, 0), pt(23, 0), pt(60, 0)];
        (p, c)
    }

    #[test]
    fn edit_25_worse_preview_rips_up_only_for_unroutable_lines() {
        // A preview worse than before the drag but with every line routed
        // keeps its first pass: one order, no rip-up round (the release
        // build does the real work). Rip-up still runs while a line is
        // left unroutable.
        let cfg = RouteConfig {
            orders: 2,
            max_iterations: 8,
        };
        let (p, c) = heavy_plan();
        let prev = run_cheap(&p, &c);
        let base = Score::of(&prev.lines);
        let mut moved = c.clone();
        moved[15] = pt(c[15].x, c[15].y + 1);
        probe::reset();
        let r = reroute(&p, &moved, &prev, &cfg, &never()).expect("ok");
        let st = probe::take();
        let (x, k, u) = st.first_pass[0];
        assert!(u == 0 && !(k <= base.clashes && x <= base.crossings), "worse, all routed ({x}/{k}/{u})");
        assert_eq!((st.orders, st.passes), (1, 1), "one order, one pass");
        check_all(&p, &moved, &r);
        // A drag that leaves a line unroutable (its box dropped inside
        // the walls): rounds still run.
        let (p, c) = walled_plan();
        let mut before = c.clone();
        before[0] = pt(60, 40);
        let prev = run_cheap(&p, &before);
        assert!(prev.lines[0].is_some(), "routed before the drag");
        let moved = c;
        probe::reset();
        reroute(&p, &moved, &prev, &cfg, &never()).expect("ok");
        let st = probe::take();
        assert_eq!(st.orders, 1, "one order");
        assert!(st.passes > 1, "rip-up for the unroutable line");
    }

    #[test]
    fn edit_25_preview_carries_lines_moved_with_both_ends() {
        // A multi-selection drag moves boxes 0, 1, 3 and 4 by one delta:
        // the lines between them keep their previous route, translated
        // by it, wherever that is still legal; they are not searched.
        let (p, c) = grid_plan(false);
        let prev = run_cheap(&p, &c);
        let delta = pt(2, 3);
        let mut moved = c.clone();
        for b in [0, 1, 3, 4] {
            moved[b] = pt(c[b].x + delta.x, c[b].y + delta.y);
        }
        probe::reset();
        let r = reroute(&p, &moved, &prev, &PREVIEW, &never()).expect("ok");
        let st = probe::take();
        check_all(&p, &moved, &r);
        let inside: Vec<usize> = (0..p.edges.len())
            .filter(|&e| [0, 1, 3, 4].contains(&p.edges[e].source.0) && [0, 1, 3, 4].contains(&p.edges[e].destination.0))
            .collect();
        let carried: Vec<usize> = inside
            .iter()
            .copied()
            .filter(|&e| {
                let (Some(a), Some(b)) = (&prev.lines[e], &r.lines[e]) else { return false };
                b.points == a.points.iter().map(|q| pt(q.x + delta.x, q.y + delta.y)).collect::<Vec<_>>()
            })
            .collect();
        assert!(st.translated >= 3, "translated {} of {inside:?}", st.translated);
        assert_eq!(carried.len(), st.translated, "the carried lines are the translated ones");
    }

    #[test]
    fn edit_25_preview_rejoins_the_previous_route_from_a_moved_end() {
        // One box of a line moved: the preview keeps the far part of the
        // previous route and searches only from the moved box back to it,
        // joining on a straight stretch with at most a 45° turn; the
        // fixed end is untouched.
        let (p, c) = grid_plan(false);
        let prev = run_cheap(&p, &c);
        let mut moved = c.clone();
        moved[8] = pt(c[8].x + 3, c[8].y + 1);
        probe::reset();
        let r = reroute(&p, &moved, &prev, &PREVIEW, &never()).expect("ok");
        let st = probe::take();
        check_all(&p, &moved, &r);
        assert!(st.rejoined >= 1, "rejoined {}", st.rejoined);
        let mut kept_far = 0;
        for (e, pe) in p.edges.iter().enumerate() {
            let one_end = (pe.source.0 == 8) != (pe.destination.0 == 8);
            let (Some(a), Some(b)) = (&prev.lines[e], &r.lines[e]) else { continue };
            if !one_end || a == b {
                continue;
            }
            // The fixed end's port and the cells next to it are the old ones.
            let fixed_first = pe.destination.0 == 8;
            let (oa, ob) = if fixed_first { (a.points[..2].to_vec(), b.points[..2].to_vec()) } else { (a.points[a.points.len() - 2..].to_vec(), b.points[b.points.len() - 2..].to_vec()) };
            kept_far += usize::from(oa == ob);
        }
        assert!(kept_far >= 1, "a far part kept");
    }

    #[test]
    fn edit_25_preview_proves_a_walled_in_line_unroutable_without_flooding() {
        // The preview's cost-bounded search first floods backwards from
        // the targets (`cost_from`); when that flood ends without reaching
        // any source, no path exists and the forward A* is skipped
        // instead of exhausting every state outside the wall. The answer
        // (unroutable, CORE-13) is the same.
        let (p, c) = walled_plan();
        let prev = run_cheap(&p, &c);
        assert!(prev.lines[0].is_none(), "walled in");
        let mut moved = c.clone();
        moved[5] = pt(61, 0);
        probe::reset();
        let full = route(&p, &moved, &CHEAP, &never()).expect("ok");
        let full_pops = probe::take().pops;
        probe::reset();
        let r = reroute(&p, &moved, &prev, &CHEAP, &never()).expect("ok");
        let st = probe::take();
        assert_eq!(r.lines, full.lines, "same answer: unroutable");
        // Only the octile attempt runs, to POP_LIMIT; `cost_from` then
        // proves the rest.
        assert_eq!((st.gave_up, st.pops), (1, u64::from(POP_LIMIT) + 1), "preview (full route {full_pops} pops)");
        assert!(full_pops > 2 * u64::from(POP_LIMIT), "the full route floods ({full_pops})");
    }

    #[test]
    fn core_25_full_build_no_path_proofs_change_no_route() {
        // The full build skips the A* of a search proven to have no path
        // (`cost_from` reaching no source, `no_way_back`): every route is
        // the same as with the A* run, and the walled-in line is given up
        // without flooding the board.
        let flood = |p: &ViewPlan, c: &[Point], off: bool| {
            probe::set_no_full_proofs(off);
            probe::reset();
            let r = route(p, c, &RouteConfig::DEFAULT, &never()).expect("ok");
            let pops = probe::take().pops;
            probe::set_no_full_proofs(false);
            (r, pops)
        };
        for (name, (p, c)) in [("grid", grid_plan(false)), ("bus", grid_plan(true)), ("walled", walled_plan()), ("large", large_plan(false))] {
            let (with, pops_with) = flood(&p, &c, false);
            let (without, pops_without) = flood(&p, &c, true);
            assert_eq!(with, without, "{name}: same routes");
            assert!(pops_with <= pops_without, "{name}");
            if name == "walled" {
                assert!(pops_with * 2 < pops_without, "walled: {pops_with} vs {pops_without} pops");
            }
        }
    }

    #[test]
    fn core_25_parallel_orders_route_as_sequential() {
        // The full build's edge orders on threads give the routes of the
        // sequential loop, bit for bit, with and without bus, on a large
        // view and on one with a walled-in line; stopped, they stop.
        let one = |p: &ViewPlan, c: &[Point], seq: bool| {
            probe::set_sequential(seq);
            let r = route(p, c, &RouteConfig::DEFAULT, &never()).expect("ok");
            probe::set_sequential(false);
            r
        };
        for (name, (p, c)) in [("grid", grid_plan(false)), ("bus", grid_plan(true)), ("walled", walled_plan()), ("large", large_plan(false)), ("heavy", heavy_plan())] {
            assert_eq!(one(&p, &c, false), one(&p, &c, true), "{name}");
        }
        let (p, c) = heavy_plan();
        assert_eq!(route(&p, &c, &RouteConfig::DEFAULT, &AtomicBool::new(true)), Err(Stopped));
    }

    #[test]
    fn edit_25_preview_skips_ports_no_line_can_enter() {
        // A box with a neighbour two cells off every side: each port has
        // its free cell, but the cell a line must come from to enter it
        // straight is inside the neighbour, so no line can arrive. The
        // gaps at the corners leave the board open, so only the A* itself
        // found that out, after popping every state. A preview sees it
        // before searching; the answer (unroutable) is the same.
        let side = (Shape::Box, 12, 7);
        let p = mk(&[BOX, side, side, side, side, BOX], &[(5, 0)], false);
        let c = vec![pt(0, 0), pt(0, -18), pt(0, 18), pt(-28, 0), pt(28, 0), pt(80, 40)];
        let prev = run_cheap(&p, &c);
        assert!(prev.lines[0].is_none(), "no way in");
        let mut moved = c.clone();
        moved[5] = pt(81, 40);
        probe::reset();
        let full = route(&p, &moved, &CHEAP, &never()).expect("ok");
        let full_pops = probe::take().pops;
        probe::reset();
        let r = reroute(&p, &moved, &prev, &CHEAP, &never()).expect("ok");
        let st = probe::take();
        assert_eq!(r.lines, full.lines, "same answer: unroutable");
        assert_eq!(st.pops, 0, "preview searched (full route {full_pops} pops)");
        assert!(full_pops > u64::from(POP_LIMIT), "the full route floods ({full_pops})");
    }

    #[test]
    fn edit_25_port_entry_needs_a_step_from_three_cells_out() {
        // The cell three out from a port is where a line's last step
        // starts: a box there, or another line running the way the step
        // heads, bars the port; a line across it does not, nor does an
        // empty cell.
        let p = mk(&[BOX, BOX, BOX], &[(1, 0), (1, 2)], false);
        let c = [pt(0, 0), pt(60, 0), pt(0, 40)];
        let kept = vec![None; p.edges.len()];
        let grid = Grid::new(&p, &c, &kept);
        let net = 1;
        // The top port of box 0 at (0, -7); three out is (0, -10).
        let at = pt(0, -7);
        let mut board = Board::new(&p, &c, grid, &[]);
        assert!(board.can_enter(at, Dir::N, net), "empty");
        let end = LineEnd::Arrow(Port { at: pt(30, -10), side: Side::W });
        board.add(&[pt(-5, -10), pt(0, -10), pt(5, -10)], 1, &end);
        assert!(board.can_enter(at, Dir::N, net), "a line across");
        // A line across both three and four cells out: crossing the first
        // the step must come straight from the second, and no step goes
        // from one line onto another.
        board.add(&[pt(-5, -11), pt(0, -11), pt(5, -11)], 1, &end);
        assert!(!board.can_enter(at, Dir::N, net), "two lines across");
        let mut board = Board::new(&p, &c, grid, &[]);
        board.add(&[pt(0, -10), pt(0, -15), pt(0, -20)], 1, &end);
        assert!(!board.can_enter(at, Dir::N, net), "a line along the step");
        // Box 2's top port at (0, 33): three out is (0, 30), inside box 0's
        // neighbour when one sits there.
        let c2 = [pt(0, 0), pt(60, 0), pt(0, 18)];
        let board = Board::new(&p, &c2, Grid::new(&p, &c2, &kept), &[]);
        assert!(!board.can_enter(pt(0, 11), Dir::N, net), "a box");
    }

    #[test]
    fn edit_25_reroute_keeps_other_lines() {
        // Every line still valid at the new centres stays as it is;
        // a kept line the moved box now covers, and a line joined to a
        // re-routed trunk, are re-routed too.
        let (p, c) = grid_plan(false);
        let prev = run_cheap(&p, &c);
        let mut moved = c.clone();
        moved[8] = pt(c[8].x + 6, c[8].y + 4);
        let r = reroute(&p, &moved, &prev, &CHEAP, &never()).expect("ok");
        check_all(&p, &moved, &r);
        for (i, e) in p.edges.iter().enumerate() {
            let touches = e.source == BoxId(8) || e.destination == BoxId(8);
            let rect = p.boxes[8].rect_at(moved[8]).inflate(CLEARANCE_UNITS);
            let covered = prev.lines[i]
                .as_ref()
                .is_some_and(|l| l.segments().iter().any(|s| s.passes_through(&rect)));
            if !touches && !covered {
                assert_eq!(r.lines[i], prev.lines[i], "edge {i} kept");
            }
        }

        // A box moved onto a line between two other boxes: that line is
        // re-routed around it.
        let p = mk(&[BOX, BOX, BOX], &[(0, 1)], false);
        let c = [pt(0, 0), pt(80, 0), pt(40, 60)];
        let prev = run(&p, &c);
        let moved = [pt(0, 0), pt(80, 0), pt(40, 0)];
        let r = reroute(&p, &moved, &prev, &RouteConfig::DEFAULT, &never()).expect("ok");
        assert_ne!(r.lines[0], prev.lines[0]);
        check_all(&p, &moved, &r);

        // A box with no edges moved clear of every line: nothing is
        // re-routed, yet `centres` holds the new centres (the editor
        // draws boxes there during a drag).
        let moved = [pt(0, 0), pt(80, 0), pt(40, 70)];
        let r = reroute(&p, &moved, &prev, &RouteConfig::DEFAULT, &never()).expect("ok");
        assert_eq!(r.centres[2], pt(40, 70));
        assert_eq!(r.lines, prev.lines);
        check_all(&p, &moved, &r);

        // Bus on: moving a trunk's source re-routes the lines joined to it.
        let (p, c) = fan_in(true);
        let prev = run(&p, &c);
        let joined = prev
            .lines
            .iter()
            .flatten()
            .find_map(|l| l.joins().map(|t| (l.edge, t)));
        let (j, t) = joined.expect("a join");
        let src = p.edges[t.0 as usize].source.0 as usize;
        let mut moved = c.clone();
        moved[src] = pt(c[src].x, c[src].y - 3);
        let r = reroute(&p, &moved, &prev, &RouteConfig::DEFAULT, &never()).expect("ok");
        check_all(&p, &moved, &r);
        if let Some(line) = r.line(j)
            && let LineEnd::Join(jn) = line.end
        {
            let trunk = r.line(jn.trunk).expect("trunk");
            let at = Segment::new(jn.at, jn.at);
            assert!(trunk.segments().iter().any(|s| s.contact(&at) != Contact::Disjoint));
        }
    }

    // Plan test names (route-01, route-05).
    #[test]
    fn ports_spread_along_side() {
        // One box feeding four boxes straight below it: four distinct
        // ports, none sharing a point, all lines routed.
        let p = mk(
            &[(Shape::Box, 30, 7), BOX, BOX, BOX, BOX],
            &[(0, 1), (0, 2), (0, 3), (0, 4)],
            false,
        );
        let c = [pt(0, 0), pt(-90, 50), pt(-30, 50), pt(30, 50), pt(90, 50)];
        let r = run(&p, &c);
        assert!(r.unroutable().is_empty());
        check_all(&p, &c, &r);
        let ports: BTreeSet<Point> = r.lines.iter().flatten().map(|l| l.source.at).collect();
        assert_eq!(ports.len(), 4);
    }

    #[test]
    fn lines_avoid_every_box_at_any_centre() {
        // An obstacle swept across the gap between two linked boxes.
        let p = mk(&[BOX, BOX, BOX, PERSON], &[(0, 1), (3, 0), (1, 3)], false);
        for dx in (-10..=90).step_by(10) {
            for dy in [-30, -10, 0, 7, 20] {
                let c = [pt(0, 0), pt(80, 0), pt(dx, dy), pt(40, -60)];
                let overlapping = p.boxes.iter().enumerate().any(|(i, a)| {
                    p.boxes.iter().enumerate().any(|(k, b)| {
                        i < k && a.rect_at(c[i]).inflate(2).overlaps(&b.rect_at(c[k]))
                    })
                });
                if overlapping {
                    continue;
                }
                let r = run(&p, &c);
                check_all(&p, &c, &r);
            }
        }
    }

    /// A 4x3 grid of boxes, every neighbour linked both ways: 34 edges,
    /// more than `LARGE_EDGES`, so it routes as a large view.
    fn large_plan(bus: bool) -> (ViewPlan, Vec<Point>) {
        let (cols, rows) = (4u32, 3u32);
        let boxes = vec![BOX; (cols * rows) as usize];
        let mut edges = Vec::new();
        for r in 0..rows {
            for k in 0..cols {
                let i = r * cols + k;
                if k + 1 < cols {
                    edges.push((i, i + 1));
                    edges.push((i + 1, i));
                }
                if r + 1 < rows {
                    edges.push((i, i + cols));
                    edges.push((i + cols, i));
                }
            }
        }
        let p = mk(&boxes, &edges, bus);
        let mut c = Vec::new();
        for r in 0..rows {
            for k in 0..cols {
                c.push(pt(i32::try_from(k).expect("s") * 45, i32::try_from(r).expect("s") * 35));
            }
        }
        (p, c)
    }

    #[test]
    fn large_view_routes_every_line_within_the_rules() {
        // The large-view search (cost bound only, in a window) keeps the
        // CORE-6..10 rules, routes every line and is deterministic.
        for bus in [false, true] {
            let (p, c) = large_plan(bus);
            assert!(p.edges.len() > LARGE_EDGES);
            let r = run_cheap(&p, &c);
            check_all(&p, &c, &r);
            assert!(r.unroutable().is_empty(), "bus {bus}: {:?}", r.unroutable());
            assert_eq!(r, run_cheap(&p, &c), "bus {bus}");
        }
    }

    #[test]
    fn large_view_search_leaves_its_window_when_walled_in() {
        // A tall box between the two boxes of a line reaches far past
        // their window: the only paths go round it outside the window, so
        // the search retries on the whole board and still finds one.
        let tall = (Shape::Box, 4, 40);
        let edges = vec![(0, 1); LARGE_EDGES + 1];
        let p = mk(&[BOX, BOX, tall], &edges, false);
        let c = [pt(0, 0), pt(60, 0), pt(30, 0)];
        let kept = vec![None; p.edges.len()];
        let grid = Grid::new(&p, &c, &kept);
        let board = Board::new(&p, &c, grid, &[]);
        let mut sc = Scratch::new(grid.len());
        let ctx = Ctx {
            plan: &p,
            centres: &c,
            corridor: &[],
            preview: false,
            previous: None,
        };
        let f = search(&ctx, &board, &mut sc, 0, &never()).expect("not stopped").expect("routes round the tall box");
        let reach = BOX.2 + WINDOW_MARGIN;
        assert!(
            f.cells.iter().any(|q| q.y.abs() > reach),
            "the path leaves the window: {:?}",
            f.cells
        );
    }

    #[test]
    fn edit_25_preview_window_retry_floods_afresh() {
        // When the window has no path, a preview's whole-board retry
        // floods `cost_from` afresh instead of reusing the window's
        // partial flood: its A* has a real bound (not 0, a Dijkstra over
        // the board) and its no-path proof is sound. It still finds the
        // way round the tall box.
        let tall = (Shape::Box, 4, 40);
        let edges = vec![(0, 1); LARGE_EDGES + 1];
        let p = mk(&[BOX, BOX, tall], &edges, false);
        let c = [pt(0, 0), pt(60, 0), pt(30, 0)];
        let kept = vec![None; p.edges.len()];
        let grid = Grid::new(&p, &c, &kept);
        let board = Board::new(&p, &c, grid, &[]);
        let mut pops = Vec::new();
        for preview in [false, true] {
            let mut sc = Scratch::new(grid.len());
            let ctx = Ctx {
                plan: &p,
                centres: &c,
                corridor: &[],
                preview,
                previous: None,
            };
            probe::reset();
            let f = search(&ctx, &board, &mut sc, 0, &never()).expect("not stopped").expect("routes round the tall box");
            pops.push(probe::take().pops);
            assert!(f.cells.iter().any(|q| q.y.abs() > BOX.2 + WINDOW_MARGIN), "preview {preview}: leaves the window");
        }
        assert!(pops[1] * 4 < pops[0], "preview pops {} vs full {}", pops[1], pops[0]);
    }

    #[test]
    fn edit_25_preview_retry_proves_no_way_in_backwards() {
        // A box with a line three cells off every side (crossable), a
        // free lane four off, and walls five off, open at the corners:
        // every port has its free cell and its step from three out, and
        // the corners let the cell flood (`cost_from`) in, but a line
        // must come square in from four out, and nothing reaches four out
        // heading in (a turn is 45° at most, and the walls bar the cells
        // behind). A preview's whole-board retry proves that by searching
        // A*'s steps backwards from the goals, a handful of states,
        // instead of popping every state on the board.
        let wall_h = (Shape::Box, 14, 2);
        let wall_v = (Shape::Box, 2, 8);
        let far = BOX;
        let edges = vec![(5, 0); LARGE_EDGES + 1];
        let mut all = edges.clone();
        all.push((6, 7));
        let p = mk(&[BOX, wall_h, wall_h, wall_v, wall_v, far, far, far], &all, false);
        let c = [pt(0, 0), pt(0, -15), pt(0, 15), pt(-20, 0), pt(20, 0), pt(60, 30), pt(-60, 40), pt(-90, 40)];
        let ring = [pt(-15, -10), pt(15, -10), pt(15, 10), pt(-15, 10), pt(-15, -10)];
        let kept = vec![None; p.edges.len()];
        let grid = Grid::new(&p, &c, &kept);
        let mut board = Board::new(&p, &c, grid, &[]);
        let ring_edge = p.edges.len() - 1;
        board.add(&cells_of(&ring), ring_edge, &LineEnd::Arrow(Port { at: pt(-15, -10), side: Side::N }));
        let mut pops = Vec::new();
        for preview in [false, true] {
            let mut sc = Scratch::new(grid.len());
            let ctx = Ctx {
                plan: &p,
                centres: &c,
                corridor: &[],
                preview,
                previous: None,
            };
            // Against the A* itself: the full build's proofs off.
            probe::set_no_full_proofs(!preview);
            probe::reset();
            let f = search(&ctx, &board, &mut sc, 0, &never()).expect("not stopped");
            pops.push(probe::take().pops);
            probe::set_no_full_proofs(false);
            assert!(f.is_none(), "preview {preview}: no way in");
        }
        assert!(pops[1] * 2 < pops[0], "preview pops {} vs full {}", pops[1], pops[0]);
    }

    #[test]
    fn edit_25_strict_cost_bound_is_never_lower() {
        // A preview's `cost_from` leaves out the steps `astar` never takes
        // (onto a line where it bends or along it, line to line, ...): on
        // a board full of lines its bound is never below the plain one at
        // any cell both settled, and above it somewhere.
        let (p, c) = heavy_plan();
        let r = run_cheap(&p, &c);
        let grid = Grid::new(&p, &c, &r.lines);
        let mut board = Board::new(&p, &c, grid, &[]);
        for line in r.lines.iter().flatten().skip(1) {
            board.add(&cells_of(&line.points), line.edge.0 as usize, &line.end);
        }
        let net = board.edge_net[0] + 1;
        let free: Vec<usize> = (0..grid.len()).filter(|&i| board.cells[i].blocked == 0 && board.cells[i].net == 0).collect();
        let (seed, source) = (free[free.len() / 3], free[free.len() * 2 / 3]);
        let whole = Rect::from_corners(Point::new(grid.x0, grid.y0), Point::new(grid.x0 + grid.w - 1, grid.y0 + grid.h - 1));
        let mut bounds = Vec::new();
        let mut settled = u32::MAX;
        for strict in [false, true] {
            let mut sc = Scratch::new(grid.len());
            sc.window = whole;
            sc.reset();
            let (reached, _) = sc.cost_from(&board, &[seed], &[source], net, strict, &never()).expect("not stopped");
            settled = settled.min(reached);
            bounds.push((0..grid.len()).map(|i| (sc.bound_gen[i] == sc.flood).then_some(sc.bound[i])).collect::<Vec<_>>());
        }
        let mut higher = 0;
        for (i, (&a, &b)) in bounds[0].iter().zip(&bounds[1]).enumerate() {
            // Final values only: below where either flood stopped.
            if let (Some(a), Some(b)) = (a, b)
                && a < settled
                && b < settled
            {
                assert!(b >= a, "cell {i}: strict {b} < plain {a}");
                higher += usize::from(b > a);
            }
        }
        assert!(higher > 0, "the strict bound is tighter somewhere");
    }

    #[test]
    fn kept_clash_bits_match_the_head_bits() {
        // `Board::add` keeps `Cell::clash` as lines are laid; after every
        // line it equals the eight-neighbour test on the head bits.
        for (p, c) in [grid_plan(false), grid_plan(true), large_plan(false)] {
            let r = run_cheap(&p, &c);
            let grid = Grid::new(&p, &c, &r.lines);
            let mut board = Board::new(&p, &c, grid, &[]);
            for line in r.lines.iter().flatten() {
                board.add(&cells_of(&line.points), line.edge.0 as usize, &line.end);
                for i in 0..grid.len() {
                    let q = grid.point(i);
                    for d in Dir::ALL {
                        assert_eq!(board.clash(q, d), board.clash_from_heads(q, d), "{q:?} {d:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn incremental_matches_full_when_unchanged() {
        // `reroute` with `centres == previous.centres` returns `previous`.
        for bus in [false, true] {
            let (p, c) = grid_plan(bus);
            let prev = run_cheap(&p, &c);
            let again = reroute(&p, &c, &prev, &CHEAP, &never()).expect("ok");
            assert_eq!(again, prev);
        }
    }

    #[test]
    fn lanes_listed_in_order() {
        let (p, c) = lane_plan();
        let r = run(&p, &c);
        let lanes = r.lanes();
        assert!(!lanes.is_empty());
        for lane in &lanes {
            assert!(lane.a < lane.b);
        }
        let mut sorted = lanes.clone();
        sorted.sort_by_key(|l| (l.a, l.b, l.a_run.a));
        assert_eq!(lanes, sorted);
        let (p, c) = grid_plan(false);
        let lanes = run_cheap(&p, &c).lanes();
        let mut sorted = lanes.clone();
        sorted.sort_by_key(|l| (l.a, l.b, l.a_run.a));
        assert_eq!(lanes, sorted);
    }

    #[test]
    fn core_25_radix_heap_lifo_pops_equal_keys_last_in_first() {
        // FIFO (the full build's order, unchanged) and LIFO (a preview's)
        // both pop keys in ascending order; among equal keys FIFO pops in
        // push order, LIFO the reverse.
        let pushes = [(5, 0), (3, 1), (3, 2), (9, 3), (3, 4), (5, 5)];
        let pop_all = |lifo: bool| {
            let mut h = RadixHeap { lifo, ..RadixHeap::default() };
            for &(k, v) in &pushes {
                h.push(k, v);
            }
            let mut out = Vec::new();
            while let Some(v) = h.pop() {
                out.push((h.last, v));
            }
            out
        };
        assert_eq!(pop_all(false), vec![(3, 1), (3, 2), (3, 4), (5, 0), (5, 5), (9, 3)]);
        assert_eq!(pop_all(true), vec![(3, 4), (3, 2), (3, 1), (5, 5), (5, 0), (9, 3)]);
        // Random keys, pushes between pops, never below the last popped.
        let mut s = 7u64;
        for lifo in [false, true] {
            let mut h = RadixHeap { lifo, ..RadixHeap::default() };
            let mut last = 0;
            for i in 0..5_000u32 {
                s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                h.push(last + u32::try_from(s >> 54).expect("small"), i);
                if i % 3 == 0 {
                    h.pop().expect("non-empty");
                    assert!(h.last >= last);
                    last = h.last;
                }
            }
            while h.pop().is_some() {
                assert!(h.last >= last);
                last = h.last;
            }
        }
    }

    #[test]
    fn edit_25_heading_bound_is_never_above_the_true_cost() {
        // The preview's per-(cell, heading) bound (`cost_from_dir`) is a
        // lower bound: A* with it finds lines exactly as cheap as with the
        // full build's per-cell bound (both optimal), and the bound at the
        // best start is never above the cost found.
        let (p, c) = large_plan(false);
        let routes = run(&p, &c);
        let mut compared = 0;
        for e in 0..p.edges.len() {
            let mut kept = routes.lines.clone();
            kept[e] = None;
            let grid = Grid::new(&p, &c, &kept);
            let mut board = Board::new(&p, &c, grid, &[]);
            for line in kept.iter().flatten() {
                board.add(&cells_of(&line.points), line.edge.0 as usize, &line.end);
            }
            let mut costs = Vec::new();
            for preview in [false, true] {
                let mut sc = Scratch::new(grid.len());
                let ctx = Ctx { plan: &p, centres: &c, corridor: &[], preview, previous: None };
                probe::reset();
                probe::with(|st| st.last_cost = u32::MAX);
                let found = search(&ctx, &board, &mut sc, e, &never()).expect("not stopped");
                let cost = probe::take().last_cost;
                costs.push(found.map(|_| cost));
                if preview && let Some(Some(cost)) = costs.last().copied() {
                    let pe = &p.edges[e];
                    let sb = &p.boxes[pe.source.0 as usize];
                    let best = end_spots(sb, c[pe.source.0 as usize], None)
                        .iter()
                        .filter_map(|s| {
                            let d = side_dir(s.port.side);
                            let at = board.port_free(s.port.at, d)?;
                            let b = sc.dbound[at * 8 + d as usize];
                            (b[0] == sc.flood).then(|| start_cost(&board, s) + b[1])
                        })
                        .min()
                        .expect("a start was flooded");
                    assert!(best <= cost, "edge {e}: bound {best} above the cost {cost}");
                }
            }
            assert_eq!(costs[0], costs[1], "edge {e}: preview and full build costs");
            compared += usize::from(costs[0].is_some());
        }
        assert!(compared >= p.edges.len() / 2, "compared {compared}");
    }

    #[test]
    fn core_25_score_box_cuts_count_as_the_plain_loops() {
        // `count_crossings` and `count_clashes` with their box cuts give
        // the counts of the plain pair loops on routed views.
        let plain_crossings = |lines: &[Option<Line>]| -> u32 {
            let routed: Vec<&Line> = lines.iter().flatten().collect();
            let mut n = 0;
            for (i, a) in routed.iter().enumerate() {
                for b in &routed[i + 1..] {
                    n += if a.joins() == Some(b.edge) {
                        crossings(&b.points, &a.points[..a.points.len() - 1])
                    } else if b.joins() == Some(a.edge) {
                        crossings(&a.points, &b.points[..b.points.len() - 1])
                    } else {
                        crossings(&a.points, &b.points)
                    };
                }
            }
            n
        };
        let plain_clashes = |lines: &[Option<Line>]| -> u32 {
            let mut n = 0;
            let routed: Vec<&Line> = lines.iter().flatten().collect();
            for (i, a) in routed.iter().enumerate() {
                for b in &routed[i + 1..] {
                    n += count_clashes(&[Some((*a).clone()), Some((*b).clone())]);
                }
            }
            n
        };
        for seed in 0..12 {
            let (p, c) = random_plan(seed, seed % 2 == 0);
            let r = run_cheap(&p, &c);
            assert_eq!(count_crossings(&r.lines), plain_crossings(&r.lines), "seed {seed}");
            assert_eq!(count_clashes(&r.lines), plain_clashes(&r.lines), "seed {seed}");
        }
        let (p, c) = grid_plan(true);
        let r = run_cheap(&p, &c);
        assert_eq!(count_crossings(&r.lines), plain_crossings(&r.lines));
        assert_eq!(count_clashes(&r.lines), plain_clashes(&r.lines));
    }

    #[test]
    fn edit_25_dial_queue_pops_as_the_radix_heap() {
        // `DialQueue` (the preview flood's) pops the same keys in the same
        // ascending order as `RadixHeap`, each key with the same values
        // (only the order among equal keys may differ), with pushes
        // between pops, keys never below the last popped, and steps past
        // the ring (`DIAL_SLOTS`) waiting their turn.
        let mut s = 11u64;
        let mut dial = DialQueue::default();
        dial.clear();
        let mut radix = RadixHeap::default();
        let mut a: Vec<(u32, u32)> = Vec::new();
        let mut b: Vec<(u32, u32)> = Vec::new();
        let mut last = 0u32;
        for i in 0..20_000u32 {
            s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let step = if i % 97 == 0 { 200_000 } else { u32::try_from(s >> 49).expect("small") };
            dial.push(last + step, i);
            radix.push(last + step, i);
            if i % 3 == 0 {
                let (x, y) = (dial.pop().expect("dial"), radix.pop().expect("radix"));
                assert_eq!(dial.last, radix.last);
                a.push((dial.last, x));
                b.push((radix.last, y));
                last = dial.last;
            }
        }
        while let Some(x) = dial.pop() {
            a.push((dial.last, x));
            let y = radix.pop().expect("as many");
            b.push((radix.last, y));
            assert_eq!(dial.last, radix.last);
        }
        assert!(radix.pop().is_none());
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
        // A cleared queue is empty and reusable.
        dial.clear();
        assert!(dial.pop().is_none());
        dial.push(5, 1);
        assert_eq!(dial.pop(), Some(1));
    }
}
