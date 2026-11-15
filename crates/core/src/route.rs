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

use crate::geom::{CLEARANCE_UNITS, Dir, Point, Rect, Segment, crossings};
// DECISION: `Shape` is named from `model` directly: `PlanBox::style` is
// already a `model::ResolvedStyle`, so route reads model data either way,
// and a re-export from `view` would change view's frozen pub surface.
use crate::model::Shape;
use crate::view::{EdgeId, PlanBox, ViewPlan};

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
    let kept = vec![None; plan.edges.len()];
    let set: Vec<usize> = (0..plan.edges.len()).collect();
    solve(plan, centres, *config, stop, kept, &set)
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
    check(stop)?;
    let set = broken(plan, centres, previous);
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
    solve(plan, centres, *config, stop, kept, &set)
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

fn check(stop: &AtomicBool) -> Result<(), Stopped> {
    if stop.load(Ordering::Relaxed) {
        Err(Stopped)
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
        let edge_net = plan
            .edges
            .iter()
            .enumerate()
            .map(|(i, e)| {
                if bus {
                    e.destination.0
                } else {
                    u32::try_from(i).expect("edge index fits u32")
                }
            })
            .collect();
        Board {
            grid,
            bus,
            cells,
            edge_net,
            arrives: vec![None; plan.edges.len()],
        }
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
    last: u32,
    len: usize,
    /// Next entry of bucket 0 to pop: equal keys pop first in, first out.
    head: usize,
    buckets: [Vec<(u32, u32)>; 33],
}

impl Default for RadixHeap {
    fn default() -> Self {
        RadixHeap {
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
    /// Best cost for the other states (near a source port, bus on).
    // DECISION: a HashMap here is only looked up, never iterated, so its
    // random hashing cannot change any output (CORE-25).
    sparse: HashMap<u64, u32, BuildHasherDefault<KeyHasher>>,
    goal: Vec<u32>,
    goal_gen: Vec<u32>,
    /// Per cell: a lower bound of the cost from it to a target (see
    /// `cost_from`), valid when `bound_gen` is `generation`.
    bound: Vec<u32>,
    bound_gen: Vec<u32>,
    /// Source start cells `cost_from` has still to settle.
    source_gen: Vec<u32>,
    /// Stamps `goal_gen`, `bound_gen` and `source_gen` per search.
    generation: u32,
    heap: RadixHeap,
    /// The queue of `cost_from`.
    cells_heap: RadixHeap,
    /// The cells a search may step on: the whole board, or a large view's
    /// window.
    window: Rect,
}

impl Scratch {
    fn new(cells: usize) -> Scratch {
        Scratch {
            recs: Vec::new(),
            dense: vec![u32::MAX; cells * 16],
            sparse: HashMap::default(),
            goal: vec![0; cells],
            goal_gen: vec![0; cells],
            bound: vec![0; cells],
            bound_gen: vec![0; cells],
            source_gen: vec![0; cells],
            generation: 0,
            heap: RadixHeap::default(),
            cells_heap: RadixHeap::default(),
            window: Rect::default(),
        }
    }

    fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.goal_gen.fill(0);
            self.bound_gen.fill(0);
            self.source_gen.fill(0);
            self.generation = 1;
        }
        self.restart();
    }

    /// Forgets every search state, keeping the goals.
    fn restart(&mut self) {
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
    /// reached then: no cell left unsettled is cheaper.
    // DECISION: the bound is per cell, not per (cell, direction): 8 times
    // fewer states, and the turn costs it leaves out are small next to
    // CROSS and NEAR, which are what made the octile bound flood (CORE-31).
    #[allow(clippy::many_single_char_names)]
    fn cost_from(&mut self, board: &Board, seeds: &[usize], sources: &[usize], net: u32) -> u32 {
        let g = board.grid;
        let generation = self.generation;
        self.cells_heap.clear();
        for &i in seeds {
            self.bound_gen[i] = generation;
            self.bound[i] = 0;
            self.cells_heap.push(0, u32::try_from(i).expect("cell fits u32"));
        }
        let mut left = 0usize;
        for &i in sources {
            if self.source_gen[i] != generation {
                self.source_gen[i] = generation;
                left += 1;
            }
        }
        let mut reached = 0;
        while left > 0 {
            let Some(ni) = self.cells_heap.pop() else { break };
            let key = self.cells_heap.last;
            let ni = ni as usize;
            if key > self.bound[ni] {
                continue;
            }
            reached = key;
            if self.source_gen[ni] == generation {
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
            let n = g.point(ni);
            for d in Dir::ALL {
                let c = n.step(d.opposite());
                if !self.window.contains(c) {
                    continue;
                }
                let Some(ci) = g.index(c) else { continue };
                if board.cells[ci].blocked > 0 {
                    continue;
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
                if self.bound_gen[ci] == generation && self.bound[ci] <= cost {
                    continue;
                }
                self.bound_gen[ci] = generation;
                self.bound[ci] = cost;
                self.cells_heap.push(cost, u32::try_from(ci).expect("cell fits u32"));
            }
        }
        reached
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
}

/// Routes `edge` on `board`: the cheapest octilinear path from a free
/// port of its source to a free port of its destination, or with `bus`
/// onto a trunk into the same box (the prototype's `route`).
#[allow(clippy::too_many_lines)]
// DECISION: one function for the A* loop, as in the prototype, so the
// cost rules read in one place; splitting it would scatter them.
fn search(ctx: &Ctx<'_>, board: &Board, sc: &mut Scratch, edge: usize) -> Option<Found> {
    let e = &ctx.plan.edges[edge];
    if e.source == e.destination {
        return None;
    }
    let g = board.grid;
    let bus = board.bus;
    let net = board.edge_net[edge] + 1;
    let sbox = &ctx.plan.boxes[e.source.0 as usize];
    let dbox = &ctx.plan.boxes[e.destination.0 as usize];
    let drect = dbox.rect_at(ctx.centres[e.destination.0 as usize]);
    sc.reset();

    // Goals: the first free cell outside each free destination port.
    let dspots = spots(dbox, ctx.centres[e.destination.0 as usize]);
    let mut targets: Vec<usize> = Vec::new();
    // DECISION: a destination port whose arrival stub runs 10px or 30px
    // beside a same-way line is left out while any other port is free
    // (CORE-12); goals carry no cost, so this is a filter, not a charge.
    let free: Vec<(usize, usize, bool)> = dspots
        .iter()
        .enumerate()
        .filter_map(|(k, s)| {
            let d = side_dir(s.port.side);
            let c = board.port_free(s.port.at, d)?;
            Some((k, c, board.stub_clash(s.port.at, d, false)))
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
    if bus {
        for i in 0..g.len() {
            if trunk_at(i).is_some() {
                targets.push(i);
            }
        }
    }
    if targets.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (g.point(targets[0]), g.point(targets[0]));
    for &i in &targets {
        let p = g.point(i);
        lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let sspots = spots(sbox, ctx.centres[e.source.0 as usize]);
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
    for window in windows {
        sc.window = window;
        // A large view skips the octile attempt: on it most searches run
        // past POP_LIMIT and only then start the one that counts.
        for attempt in usize::from(large)..2 {
            let mut bound = Bound::Octile;
            if attempt == 1 {
                sc.restart();
                let sources: Vec<usize> = sspots
                    .iter()
                    .filter_map(|s| board.port_free(s.port.at, side_dir(s.port.side)))
                    .collect();
                bound = Bound::Cost(sc.cost_from(board, &targets, &sources, net));
            }
            match astar(board, sc, edge, (lo, hi), &sspots, &dspots, bound, &trunk_at) {
                Outcome::Found(f) => return Some(f),
                Outcome::NoPath => break,
                Outcome::GaveUp => {}
            }
        }
    }
    None
}

/// The heuristic of one A* attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bound {
    /// Octile distance to the targets' bounding box (`H_MAX`, `H_MIN`).
    Octile,
    /// `Scratch::bound` where `cost_from` reached, else the cost it
    /// stopped at.
    Cost(u32),
}

/// How one A* attempt ended.
enum Outcome {
    Found(Found),
    NoPath,
    /// Hit `POP_LIMIT` with the octile bound; retry with the cost bound.
    GaveUp,
}

/// The A* loop of `search`.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::many_single_char_names
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
) -> Outcome {
    let g = board.grid;
    let bus = board.bus;
    let net = board.edge_net[edge] + 1;
    let generation = sc.generation;
    let limited = bound == Bound::Octile;
    let window = sc.window;
    let h = |sc: &Scratch, p: Point, i: usize| -> u32 {
        if let Bound::Cost(reached) = bound {
            return if sc.bound_gen[i] == generation {
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
        let mut cost = START_OFFSET * u32::try_from(s.off).unwrap_or(0);
        if board.stub_clash(s.port.at, d, true) {
            cost += CLASH;

        }
        let r = Rec {
            cell: u32::try_from(c).expect("cell fits u32"),
            dir: d,
            run: 2,
            steps: 0,
            cost,
            parent: u32::MAX,
            spot: u32::try_from(k).expect("spot index fits u32"),
        };
        let hc = h(sc, g.point(c), c);
        sc.push(r, hc, &mut tie, bus);
    }

    while let Some(idx) = sc.heap.pop() {
        pops += 1;
        if limited && pops > POP_LIMIT {
            return Outcome::GaveUp;
        }
        let r = sc.recs[idx as usize];
        if r.steps == JOINED {
            return Outcome::Found(unwind(board, sc, idx, sspots, None, edge));
        }
        if sc.best(r.cell, r.dir, r.run, r.steps, bus) != Some(r.cost) {
            continue;
        }
        let ci = r.cell as usize;
        if sc.goal_gen[ci] == sc.generation {
            let spot = dspots[sc.goal[ci] as usize];
            if side_dir(spot.port.side).opposite() == r.dir {
                return Outcome::Found(unwind(board, sc, idx, sspots, Some(spot.port), edge));
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
            if board.cells[ni].blocked > 0 {
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
            let hn = h(sc, n, ni);
            sc.push(nr, hn, &mut tie, bus);
        }
    }
    Outcome::NoPath
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
    let mut n = 0u32;
    for (i, a) in runs.iter().enumerate() {
        for b in &runs[i + 1..] {
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
    let routed: Vec<&Line> = lines.iter().flatten().collect();
    let mut n = 0u32;
    for (i, a) in routed.iter().enumerate() {
        for b in &routed[i + 1..] {
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
    let mut board = Board::new(ctx.plan, ctx.centres, grid, history);
    for line in kept.iter().flatten() {
        board.add(&cells_of(&line.points), line.edge.0 as usize, &line.end);
    }
    let mut lines = kept.to_vec();
    let mut failed = Vec::new();
    let mut crossed = Vec::new();
    for &e in order {
        check(stop)?;
        match search(ctx, &board, sc, e) {
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
    let score = Score::of(&lines);
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
    stop: &AtomicBool,
) -> Result<Pass, Stopped> {
    let mut history = vec![0u32; grid.len()];
    let mut current = order.to_vec();
    let mut best = pass(ctx, grid, sc, kept, &current, &history, stop)?;
    let mut last_failed = best.failed.clone();
    let mut last_crossed = best.crossed.clone();
    let mut stall = 0;
    for _ in 0..rounds {
        check(stop)?;
        // DECISION: rip-up stops once nothing is unroutable and the
        // crossings are within the view's own budget (`maxCrossings`,
        // CORE-17): further rounds cost time (CORE-24) and the score
        // already meets what lint checks.
        let budget = ctx.plan.options.max_crossings;
        if best.score.within(budget) || (last_failed.is_empty() && last_crossed.is_empty()) {
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
) -> Result<Routes, Stopped> {
    let ctx = Ctx { plan, centres };
    let grid = Grid::new(plan, centres, &kept);
    let mut sc = Scratch::new(grid.len());
    let mut best: Option<Pass> = None;
    for order in orders(plan, centres, set, config.orders) {
        check(stop)?;
        // DECISION: once an order is within the view's budget with nothing
        // unroutable, later orders get their plain pass only, no rip-up:
        // rip-up exists to reach that bar, and the rounds are the
        // router's main cost (CORE-24).
        let budget = plan.options.max_crossings;
        let rounds = if best.as_ref().is_some_and(|b| b.score.within(budget)) {
            0
        } else {
            config.max_iterations
        };
        let p = improve(&ctx, grid, &mut sc, &kept, &order, rounds, stop)?;
        if best.as_ref().is_none_or(|b| p.score.better_than(&b.score)) {
            best = Some(p);
        }
    }
    let (lines, crossings) = if let Some(b) = best {
        (b.lines, b.score.crossings)
    } else {
        let crossings = count_crossings(&kept);
        (kept, crossings)
    };
    Ok(Routes {
        lines,
        centres: centres[..plan.boxes.len()].to_vec(),
        crossings,
    })
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
        let ctx = Ctx { plan: &p, centres: &c };
        let f = search(&ctx, &board, &mut sc, 0).expect("routes round the tall box");
        let reach = BOX.2 + WINDOW_MARGIN;
        assert!(
            f.cells.iter().any(|q| q.y.abs() > reach),
            "the path leaves the window: {:?}",
            f.cells
        );
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
}
