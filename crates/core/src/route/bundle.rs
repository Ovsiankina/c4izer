//! Bundles, the editor's "Bus" (P4-404..P4-417): the members of a bundle
//! share one corridor. A spine is searched between the two groups of
//! member ends on a board where every box is grown by the bundle's half
//! width; each track is an offset of the spine (20px on axis segments,
//! three `x ± y` units on diagonal ones); each member then gets a fan-out
//! from its port to its track's start and from its track's end to its
//! other port, routed as vertex legs. The other lines of the view are
//! routed afterwards around the bundled lines, and may cross a bundle
//! only square across all of its tracks (`Board::corridor`).
//!
//! Depends on: the router (`super`), its boards, legs and solve.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use super::{
    Board, Ctx, Dir, Goal, Grid, Line, LineEnd, Point, Port, Rect, RouteConfig, RoutedBundle, Routes, SHUT, Segment,
    Side, Start, Stopped, AtomicBool, BEND, CLASH, MARGIN, START_OFFSET, STEP_DIAGONAL, STEP_STRAIGHT, H_MAX, H_MIN,
    along_bans, along_ok, axis_bit, cells_of, check, crossings, edge_id, end_ports, leg, pinned_any, pinned_grid, side_dir, solve,
    solve_pinned, vertices,
};
use crate::view::{BoxId, ViewPlan};

/// P4-406: grid units between neighbouring tracks on an axis segment
/// (20px, the CORE-12 lane pitch).
const PITCH: i32 = 2;
/// P4-406: units of `x - y` (or `x + y`) between neighbouring tracks on a
/// diagonal segment: 3 / sqrt 2 = 2.12 units, 21.2px, the grid offset
/// nearest the 20px pitch.
const DIAG: i32 = 3;
/// P4-408: the shortest track, in grid steps (40px).
const MIN_STRETCH: i32 = 4;
/// Cells between the outermost track's start and the near boxes, so a
/// fan-out has its port stub and one turn before the track.
// DECISION: 2 cells past the clearance: room for the 2-cell port stub of
// a member straight behind its track (P4-410); a longer gap only
// shortens the shared stretch between close rows of boxes.
const GAP: i32 = 1;
/// A turn of the spine: dearer than a vertex leg's, so the corridor has
/// few corners (a bus reads best straight).
const SPINE_BEND: u32 = 4 * BEND;
/// States a spine search pops before it gives up (CORE-32).
const SPINE_POPS: usize = 400_000;
/// The stop flag of the member legs: never set.
// DECISION: a bundle's member legs keep reading no stop flag (they never
// did): threading it would change every bundle helper's return type, and
// bundled views are small; `solve_bundled` still reads the real flag per
// bundle and passes it to the lines routed after the bundles.
static NEVER: AtomicBool = AtomicBool::new(false);

/// `route` and `reroute` for a view with bundles (P4-404): each bundle in
/// plan order on the board left by the ones before it, then every other
/// line as the router without bundles routes it, the bundled lines kept
/// as obstacles and their corridors stamped on every board.
pub(super) fn solve_bundled(plan: &ViewPlan, centres: &[Point], config: RouteConfig, stop: &AtomicBool) -> Result<Routes, Stopped> {
    let n = plan.edges.len();
    let mut kept: Vec<Option<Line>> = vec![None; n];
    let mut corridor: Vec<(Point, u8)> = Vec::new();
    let mut infos = Vec::with_capacity(plan.bundles.len());
    let grid = bundle_grid(plan, centres);
    let mut done = vec![false; n];
    for members in &plan.bundles {
        check(stop)?;
        let ms: Vec<usize> = members.iter().map(|e| e.0 as usize).filter(|&e| e < n && !done[e]).collect();
        let mut stay = Vec::new();
        let mut left = Vec::new();
        for &e in &ms {
            match vertex_split(plan, centres, e) {
                Some(k) => stay.push((e, k)),
                None => left.push(edge_id(e)),
            }
        }
        let mut info = RoutedBundle {
            members: members.clone(),
            tracks: Vec::new(),
            shared: Vec::new(),
            crossings: 0,
            fallback: false,
            left,
        };
        let routed = if stay.len() >= 2 {
            route_bundle(plan, centres, grid, &kept, &corridor, &stay)
        } else {
            None
        };
        match routed {
            Some(b) => {
                for (k, &e) in b.order.iter().enumerate() {
                    kept[e] = Some(b.lines[k].clone());
                    done[e] = true;
                }
                let mut x = 0;
                for (i, a) in b.lines.iter().enumerate() {
                    for c in &b.lines[i + 1..] {
                        x += crossings(&a.points, &c.points);
                    }
                }
                info.crossings = x;
                info.tracks = b.order.iter().map(|&e| edge_id(e)).collect();
                info.shared = b.tracks;
                corridor.extend(b.corridor);
            }
            None => info.fallback = true,
        }
        infos.push(info);
    }
    let rest: Vec<usize> = (0..n).filter(|&e| kept[e].is_none()).collect();
    let mut routes = if pinned_any(plan) {
        solve_pinned(plan, centres, config, stop, kept, &rest, &corridor, None, None)?
    } else {
        solve(plan, centres, config, stop, kept, &rest, &corridor, None, None)?
    };
    routes.bundles = infos;
    Ok(routes)
}

/// The routing area for bundles: `pinned_grid` grown by the widest
/// bundle's half width and fan-out gap, so a spine may run round a box at
/// the edge of the view.
fn bundle_grid(plan: &ViewPlan, centres: &[Point]) -> Grid {
    let g = pinned_grid(plan, centres, &[]);
    let widest = plan.bundles.iter().map(Vec::len).max().unwrap_or(2);
    let extra = PITCH * i32::try_from(widest).unwrap_or(i32::MAX / 4) + GAP + MARGIN;
    Grid {
        x0: g.x0 - extra,
        y0: g.y0 - extra,
        w: g.w + 2 * extra,
        h: g.h + 2 * extra,
    }
}

/// P4-416: how many of `e`'s vertices go to its source fan-out (each
/// vertex to the end whose box centre is nearer, Manhattan, ties to the
/// source), `None` when that split breaks their order (P4-417).
fn vertex_split(plan: &ViewPlan, centres: &[Point], e: usize) -> Option<usize> {
    let pe = &plan.edges[e];
    let (s, d) = (centres[pe.source.0 as usize], centres[pe.destination.0 as usize]);
    let near_source = |v: &Point| (v.x - s.x).abs() + (v.y - s.y).abs() <= (v.x - d.x).abs() + (v.y - d.y).abs();
    let k = pe.vertices.iter().take_while(|v| near_source(v)).count();
    pe.vertices[k..].iter().all(|v| !near_source(v)).then_some(k)
}

/// One bundle routed on its shared stretch.
struct Bundled {
    /// Member edges in track order.
    order: Vec<usize>,
    /// Their lines, same order.
    lines: Vec<Line>,
    /// Their tracks, same order, each in its member's direction.
    tracks: Vec<Vec<Point>>,
    /// The corridor cells between the tracks (P4-413).
    corridor: Vec<(Point, u8)>,
}

/// The unit step of axis or diagonal direction `d`.
fn unit(d: Dir) -> (i32, i32) {
    d.delta()
}

/// The left normal of `d` seen along it (y grows down): `(dy, -dx)`.
fn left(d: Dir) -> (i32, i32) {
    let (dx, dy) = unit(d);
    (dy, -dx)
}

/// The across value of `p` for lines heading `d`: grows to the left of
/// `d` by 1 per axis unit (axis `d`) or per `x ± y` unit (diagonal `d`).
fn across(d: Dir, p: Point) -> i32 {
    let (dx, dy) = unit(d);
    dy * p.x - dx * p.y
}

/// The along value of `p` for lines heading `d`, growing along `d`.
fn along(d: Dir, p: Point) -> i32 {
    let (dx, dy) = unit(d);
    dx * p.x + dy * p.y
}

/// The offset of track `j` (signed, 0 on the spine) from the spine on a
/// segment heading `d`, in `across` units (P4-406).
fn offset(d: Dir, j: i32) -> i32 {
    if d.is_diagonal() { DIAG * j } else { PITCH * j }
}

/// The meeting point of the line heading `d1` with across value `c1`
/// and the line heading `d2` with across value `c2`; `None` for parallel
/// lines or a point off the grid.
fn meet(d1: Dir, c1: i32, d2: Dir, c2: i32) -> Option<Point> {
    let (dx1, dy1) = unit(d1);
    let (dx2, dy2) = unit(d2);
    let (a1, b1, a2, b2) = (dy1, -dx1, dy2, -dx2);
    let det = a1 * b2 - a2 * b1;
    if det == 0 {
        return None;
    }
    let xn = c1 * b2 - c2 * b1;
    let yn = a1 * c2 - a2 * c1;
    (xn % det == 0 && yn % det == 0).then(|| Point::new(xn / det, yn / det))
}

/// The direction of `a -> b`.
fn dir(a: Point, b: Point) -> Option<Dir> {
    Segment::new(a, b).dir()
}

/// Chebyshev distance from `p` to the rectangle `r` (0 inside).
fn cheb(r: &Rect, p: Point) -> i32 {
    let dx = (r.min.x - p.x).max(0).max(p.x - r.max.x);
    let dy = (r.min.y - p.y).max(0).max(p.y - r.max.y);
    dx.max(dy)
}

/// Routes one bundle (`ms`: member edge, source-side vertex count), or
/// `None` when it has no shared stretch of useful length (P4-408).
#[allow(clippy::too_many_lines, clippy::too_many_arguments, clippy::many_single_char_names)]
fn route_bundle(
    plan: &ViewPlan,
    centres: &[Point],
    grid: Grid,
    kept: &[Option<Line>],
    corridor: &[(Point, u8)],
    ms: &[(usize, usize)],
) -> Option<Bundled> {
    let nt = i32::try_from(ms.len()).ok()?;
    let c = (nt - 1) / 2;
    let jm = nt - 1 - c;
    let cen = |b: BoxId| centres[b.0 as usize];
    // P4-411: turn each member to agree with the sum of the ones before.
    let mut sum = (0i64, 0i64);
    let mut flip = vec![false; ms.len()];
    for (i, &(e, _)) in ms.iter().enumerate() {
        let pe = &plan.edges[e];
        let (s, d) = (cen(pe.source), cen(pe.destination));
        let v = (i64::from(d.x - s.x), i64::from(d.y - s.y));
        if i > 0 && sum.0 * v.0 + sum.1 * v.1 < 0 {
            flip[i] = true;
            sum = (sum.0 - v.0, sum.1 - v.1);
        } else {
            sum = (sum.0 + v.0, sum.1 + v.1);
        }
    }
    let end = |e: usize, at_source: bool| -> (BoxId, Point) {
        let pe = &plan.edges[e];
        let (b, a) = if at_source {
            (pe.source, pe.source_anchor)
        } else {
            (pe.destination, pe.destination_anchor)
        };
        let r = plan.boxes[b.0 as usize].rect_at(cen(b));
        (b, a.map_or(cen(b), |a| a.point(r)))
    };
    let near: Vec<(BoxId, Point)> = ms.iter().zip(&flip).map(|(&(e, _), &f)| end(e, !f)).collect();
    let far: Vec<(BoxId, Point)> = ms.iter().zip(&flip).map(|(&(e, _), &f)| end(e, f)).collect();
    let mean = |v: &[(BoxId, Point)]| {
        let n = i32::try_from(v.len()).unwrap_or(1).max(1);
        let (x, y) = v.iter().fold((0, 0), |a, (_, p)| (a.0 + p.x, a.1 + p.y));
        Point::new(x.div_euclid(n), y.div_euclid(n))
    };
    let (ca, cb) = (mean(&near), mean(&far));

    // The fat board (P4-409): boxes grown by the half width + clearance,
    // earlier bundles' lines and corridors as walls.
    let half = PITCH * jm;
    let mut fat = vec![false; grid.len()];
    let wall = |p: Point, r: i32, fat: &mut Vec<bool>| {
        for y in p.y - r..=p.y + r {
            for x in p.x - r..=p.x + r {
                if let Some(i) = grid.index(Point::new(x, y)) {
                    fat[i] = true;
                }
            }
        }
    };
    for b in &plan.boxes {
        let r = b.rect_at(cen(b.id)).inflate(1 + half);
        for y in r.min.y..=r.max.y {
            for x in r.min.x..=r.max.x {
                if let Some(i) = grid.index(Point::new(x, y)) {
                    fat[i] = true;
                }
            }
        }
    }
    for line in kept.iter().flatten() {
        for p in cells_of(&line.points) {
            wall(p, half + 1, &mut fat);
        }
    }
    for &(p, _) in corridor {
        wall(p, half + 1, &mut fat);
    }

    // The spine's ends (P4-411): where every member has the same box at
    // an end, unpinned, the tracks run straight into ports side by side on
    // the box side facing the other group; elsewhere the spine ends at the
    // free cell nearest the segment `ca..cb`, then nearest its own group,
    // clear of both groups' boxes by the half width, clearance and
    // fan-out gap.
    let rects = |v: &[(BoxId, Point)]| -> Vec<Rect> { v.iter().map(|(b, _)| plan.boxes[b.0 as usize].rect_at(cen(*b))).collect() };
    let (near_r, far_r) = (rects(&near), rects(&far));
    // A member's near end is its source unless it is turned.
    let pinned_at = |near_end: bool| {
        ms.iter().zip(&flip).any(|(&(e, _), &f)| {
            let pe = &plan.edges[e];
            if near_end == f { pe.destination_anchor.is_some() } else { pe.source_anchor.is_some() }
        })
    };
    let near_side = shared_side(plan, centres, &near, cb, nt, c, pinned_at(true));
    let far_side = shared_side(plan, centres, &far, ca, nt, c, pinned_at(false));
    for (side, _) in [near_side, far_side].into_iter().flatten() {
        // A channel out of the box, as wide as the bundle, long enough
        // for the first segment.
        let out = side_dir(side);
        let (lx, ly) = left(out);
        let Some((_, s0)) = [near_side, far_side].into_iter().flatten().find(|&(s, _)| s == side) else {
            continue;
        };
        for t in 0..=(2 * half + 4) {
            for k in -half..=half {
                let p = Point::new(s0.x + t * unit(out).0 + k * lx, s0.y + t * unit(out).1 + k * ly);
                if plan.boxes.iter().any(|b| cheb(&b.rect_at(cen(b.id)), p) <= 1) {
                    continue;
                }
                if let Some(i) = grid.index(p) {
                    fat[i] = false;
                }
            }
        }
    }
    // DECISION: one row more per track on the wider side of the spine,
    // so the fan-outs coming in from one side can nest, the member nearer
    // the spine on the row nearer its boxes, without crossing.
    let clear = 1 + half + GAP + jm;
    // DECISION: a general end lies past every box of its own group along
    // `ca -> cb` (before every box of the other group), each box grown by
    // `clear`, so the spine never starts between its group's boxes; then
    // nearest the segment, then nearest its own group's centre.
    let axis = (i64::from(cb.x - ca.x), i64::from(cb.y - ca.y));
    let along_ab = |p: Point| i64::from(p.x - ca.x) * axis.0 + i64::from(p.y - ca.y) * axis.1;
    let corners = |r: &Rect| {
        let r = r.inflate(clear);
        [r.min, r.max, Point::new(r.min.x, r.max.y), Point::new(r.max.x, r.min.y)]
    };
    let past_near = near_r.iter().flat_map(corners).map(along_ab).max().unwrap_or(0);
    let before_far = far_r.iter().flat_map(corners).map(along_ab).min().unwrap_or(0);
    // DECISION: among those cells the end takes the least `2 x distance
    // from the line ca..cb + distance past its own group's limit` (both
    // scaled by |cb - ca|, integers): near the bundle's axis, and as near
    // its own group as allowed, so the stretch between the ends is long.
    let pick = |own: &[Rect], other: &[Rect], near_end: bool| -> Option<Point> {
        let mut best: Option<(i64, Point)> = None;
        for (i, &blocked) in fat.iter().enumerate() {
            if blocked {
                continue;
            }
            let p = grid.point(i);
            let a = along_ab(p);
            let past = if near_end { a - past_near } else { before_far - a };
            if past < 0 {
                continue;
            }
            if own.iter().chain(other).any(|r| cheb(r, p) < clear) {
                continue;
            }
            let cross = (i64::from(p.x - ca.x) * axis.1 - i64::from(p.y - ca.y) * axis.0).abs();
            let k = 2 * cross + past;
            if best.is_none_or(|(b, _)| k < b) {
                best = Some((k, p));
            }
        }
        best.map(|(_, p)| p)
    };
    // Facing a shared side, the other end first tries the straight line
    // out of that side, as far out as its own group allows, so the
    // stretch is one straight run.
    let straight = |own: &[Rect], from: (Side, Point), near_end: bool| -> Option<Point> {
        let (side, p) = from;
        let (ox, oy) = unit(side_dir(side));
        let mut best = None;
        for t in MIN_STRETCH..grid.w.max(grid.h) {
            let q = Point::new(p.x + t * ox, p.y + t * oy);
            let Some(i) = grid.index(q) else { break };
            if fat[i] || own.iter().any(|r| cheb(r, q) < clear) {
                break;
            }
            let a = along_ab(q);
            let past = if near_end { a - past_near } else { before_far - a };
            if past < 0 {
                break;
            }
            best = Some(q);
        }
        best
    };
    let (sa, start_dir) = match (near_side, far_side) {
        (Some((side, p)), _) => (p, Some(side_dir(side))),
        (None, Some(f)) => (straight(&near_r, f, true).or_else(|| pick(&near_r, &far_r, true))?, None),
        (None, None) => (pick(&near_r, &far_r, true)?, None),
    };
    let (sb, end_dir) = match (far_side, near_side) {
        (Some((side, p)), _) => (p, Some(side_dir(side).opposite())),
        (None, Some(n)) => (straight(&far_r, n, false).or_else(|| pick(&far_r, &near_r, false))?, None),
        (None, None) => {
            // DECISION: with two general ends the far end is first sought
            // straight on from the near end along the axis nearest
            // `ca -> cb` (the furthest such cell before the far group's
            // limit), so the stretch is one straight run when the groups
            // face each other.
            let d = if (cb.x - ca.x).abs() >= (cb.y - ca.y).abs() {
                if cb.x >= ca.x { Dir::E } else { Dir::W }
            } else if cb.y >= ca.y {
                Dir::S
            } else {
                Dir::N
            };
            let (ox, oy) = unit(d);
            let mut found = None;
            for t in MIN_STRETCH..grid.w.max(grid.h) {
                let q = Point::new(sa.x + t * ox, sa.y + t * oy);
                let Some(i) = grid.index(q) else { break };
                if fat[i] || before_far - along_ab(q) < 0 {
                    break;
                }
                if !near_r.iter().chain(&far_r).any(|r| cheb(r, q) < clear) {
                    found = Some(q);
                }
            }
            (found.or_else(|| pick(&far_r, &near_r, false))?, None)
        }
    };
    // P4-408: ends too close for a useful stretch, or the far end not
    // further along `ca -> cb` than the near end (the groups overlap).
    if (sa.x - sb.x).abs().max((sa.y - sb.y).abs()) < MIN_STRETCH || along_ab(sb) <= along_ab(sa) {
        return None;
    }
    // P4-600: every track, the outermost `half` from the spine, keeps the
    // clearance from a boundary's stroke.
    let bans = along_bans(plan, centres, grid, half);
    let spine = spine(grid, (&fat, &bans), (sa, start_dir), (sb, end_dir), 2 + 2 * jm)?;

    // Tracks (P4-406, P4-407): offsets of the spine.
    let (dirs, tracks) = tracks_of(&spine, nt)?;
    let m = dirs.len();

    // Track order (P4-411): near rank, then far rank, then edge.
    let (l0, ln) = (left(dirs[0]), left(dirs[m - 1]));
    let proj = |p: Point, o: Point, l: (i32, i32)| i64::from(p.x - o.x) * i64::from(l.0) + i64::from(p.y - o.y) * i64::from(l.1);
    let mut idx: Vec<usize> = (0..ms.len()).collect();
    idx.sort_by_key(|&i| (proj(near[i].1, sa, l0), proj(far[i].1, sb, ln), ms[i].0));

    // The corridor between neighbouring tracks (P4-413).
    let on_track: HashSet<Point> = tracks.iter().flat_map(|t| cells_of(t)).collect();
    let mut cor: Vec<(Point, u8)> = Vec::new();
    for k in 0..tracks.len() - 1 {
        let (t, u) = (&tracks[k], &tracks[k + 1]);
        for (i, &d) in dirs.iter().enumerate() {
            let (a, b, a2, b2) = (t[i], t[i + 1], u[i], u[i + 1]);
            let (lo_c, hi_c) = (across(d, a), across(d, a2));
            let span = (along(d, a).min(along(d, a2)), along(d, b).max(along(d, b2)));
            let inner = (along(d, a).max(along(d, a2)), along(d, b).min(along(d, b2)));
            let open = if d.is_diagonal() {
                SHUT
            } else {
                let (lx, ly) = left(d);
                let across_dir = Segment::new(Point::new(0, 0), Point::new(lx, ly)).dir().unwrap_or(Dir::N);
                axis_bit(across_dir)
            };
            let (x0, x1) = (a.x.min(b.x).min(a2.x).min(b2.x), a.x.max(b.x).max(a2.x).max(b2.x));
            let (y0, y1) = (a.y.min(b.y).min(a2.y).min(b2.y), a.y.max(b.y).max(a2.y).max(b2.y));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let p = Point::new(x, y);
                    let (cc, al) = (across(d, p), along(d, p));
                    if cc <= lo_c || cc >= hi_c || al < span.0 || al > span.1 || on_track.contains(&p) {
                        continue;
                    }
                    let v = if al > inner.0 && al < inner.1 { open } else { SHUT };
                    cor.push((p, v));
                }
            }
        }
        // Corners: every cell between the two tracks' bends.
        for i in 1..m {
            let (a, b) = (t[i], u[i]);
            for y in a.y.min(b.y)..=a.y.max(b.y) {
                for x in a.x.min(b.x)..=a.x.max(b.x) {
                    let p = Point::new(x, y);
                    if !on_track.contains(&p) {
                        cor.push((p, SHUT));
                    }
                }
            }
        }
    }
    // DECISION: a cell listed twice keeps the stricter value (`SHUT`
    // wins), as `Ctx::board` does across bundles.
    cor.sort_unstable_by_key(|&(p, v)| (p.y, p.x, Reverse(v)));
    cor.dedup_by_key(|&mut (p, _)| p);

    // Fan-outs (P4-410), in track order, on a board holding the earlier
    // bundles, every track of this one and both corridors.
    let mut all_cor = corridor.to_vec();
    all_cor.extend_from_slice(&cor);
    let ctx = Ctx {
        plan,
        centres,
        corridor: &all_cor,
        preview: false,
        previous: None,
    };
    let mut board = ctx.board(grid, &[]);
    for line in kept.iter().flatten() {
        board.add(&cells_of(&line.points), line.edge.0 as usize, &line.end);
    }
    let member_track = |i: usize, k: usize| -> Vec<Point> {
        let mut t = tracks[k].clone();
        if flip[i] {
            t.reverse();
        }
        t
    };
    for (k, &i) in idx.iter().enumerate() {
        let t = member_track(i, k);
        let last = *t.last()?;
        board.add(&cells_of(&t), ms[i].0, &LineEnd::Arrow(Port { at: last, side: Side::N }));
    }
    // DECISION: fan-outs are routed nearest-first (the member whose near
    // end lies nearest the spine's axis first), so it takes the row next
    // to its boxes and the ones from further out pass outside it (nested,
    // crossing-free); the result is stored in track order.
    let mut by_reach: Vec<usize> = (0..idx.len()).collect();
    by_reach.sort_by_key(|&k| (proj(near[idx[k]].1, sa, l0).abs(), proj(far[idx[k]].1, sb, ln).abs(), ms[idx[k]].0));
    let mut routed: Vec<Option<Line>> = vec![None; idx.len()];
    for &k in &by_reach {
        let i = idx[k];
        let (e, nsrc) = ms[i];
        let t = member_track(i, k);
        let line = member_line(&ctx, &board, e, &t, nsrc)?;
        // DECISION: a member whose line runs more than twice the
        // Manhattan distance between its boxes (plus 20 cells) is no
        // shared stretch but a detour: the bundle falls back (P4-408).
        let pe = &plan.edges[e];
        let (a, b) = (cen(pe.source), cen(pe.destination));
        let direct = (a.x - b.x).abs() + (a.y - b.y).abs();
        let steps: i32 = line.segments().iter().filter_map(Segment::len_steps).sum();
        if steps > 2 * direct + 20 {
            return None;
        }
        board.add(&cells_of(&line.points), e, &line.end);
        routed[k] = Some(line);
    }
    let mut order = Vec::with_capacity(ms.len());
    let mut lines = Vec::with_capacity(ms.len());
    let mut out_tracks = Vec::with_capacity(ms.len());
    for (k, &i) in idx.iter().enumerate() {
        let (e, _) = ms[i];
        let t = member_track(i, k);
        let line = routed[k].take()?;
        order.push(e);
        lines.push(line);
        out_tracks.push(t);
    }
    Some(Bundled {
        order,
        lines,
        tracks: out_tracks,
        corridor: cor,
    })
}

/// The side of the box every member of `group` ends at, facing `other`,
/// and the spine's end in front of it: 2 cells out from the side's port
/// for the spine's own track `c`, so each track `j` ends in front of its
/// own port, 20px from the next (P4-406). `None` when the members end at
/// different boxes, an end is `pinned`, or the side has no port for some
/// track (CORE-9: a person's top, a cylinder's narrow top and bottom, a
/// short side).
#[allow(clippy::too_many_arguments)]
fn shared_side(
    plan: &ViewPlan,
    centres: &[Point],
    group: &[(BoxId, Point)],
    other: Point,
    nt: i32,
    c: i32,
    pinned: bool,
) -> Option<(Side, Point)> {
    let b = group.first()?.0;
    if pinned || group.iter().any(|&(x, _)| x != b) {
        return None;
    }
    let pb = &plan.boxes[b.0 as usize];
    let centre = centres[b.0 as usize];
    let r = pb.rect_at(centre);
    let (dx, dy) = (other.x - centre.x, other.y - centre.y);
    let (hw, hh) = ((r.max.x - r.min.x).max(1), (r.max.y - r.min.y).max(1));
    // DECISION: the side the other group lies beyond, relative to the
    // box's own proportions, so a wide box takes its bottom for a group
    // below and to one side.
    let side = if i64::from(dx.abs()) * i64::from(hh) > i64::from(dy.abs()) * i64::from(hw) {
        if dx > 0 { Side::E } else { Side::W }
    } else if dy > 0 {
        Side::S
    } else {
        Side::N
    };
    let spots = super::spots(pb, centre);
    let out = side_dir(side);
    let along_side = left(out.opposite());
    // The spine's track sits nearest the side's centre.
    let mid = match side {
        Side::N => Point::new(centre.x, r.min.y),
        Side::S => Point::new(centre.x, r.max.y),
        Side::W => Point::new(r.min.x, (r.min.y + r.max.y).div_euclid(2)),
        Side::E => Point::new(r.max.x, (r.min.y + r.max.y).div_euclid(2)),
    };
    let person_head = matches!(pb.style.shape, crate::model::Shape::Person) && matches!(side, Side::E | Side::W);
    let mid = if person_head { Point::new(mid.x, mid.y + super::PERSON_HEAD / 2) } else { mid };
    for j in -c..nt - c {
        let at = Point::new(mid.x + PITCH * j * along_side.0, mid.y + PITCH * j * along_side.1);
        if !spots.iter().any(|s| s.port.side == side && s.port.at == at) {
            return None;
        }
    }
    let (ox, oy) = unit(out);
    Some((side, Point::new(mid.x + 2 * ox, mid.y + 2 * oy)))
}

/// The tracks `0..nt` of `spine` (P4-406, P4-407), track `(nt - 1) / 2`
/// on the spine, and the spine's segment directions; `None` when a track
/// segment would be shorter than 2 steps or turn round, or a track
/// shorter than `MIN_STRETCH` (P4-408).
fn tracks_of(spine: &[Point], nt: i32) -> Option<(Vec<Dir>, Vec<Vec<Point>>)> {
    let c = (nt - 1) / 2;
    let dirs: Vec<Dir> = spine.windows(2).map(|w| dir(w[0], w[1])).collect::<Option<_>>()?;
    let m = dirs.len();
    if m == 0 {
        return None;
    }
    let track = |j: i32| -> Option<Vec<Point>> {
        let mut out = Vec::with_capacity(m + 1);
        let (lx, ly) = left(dirs[0]);
        out.push(Point::new(spine[0].x + PITCH * j * lx, spine[0].y + PITCH * j * ly));
        for i in 1..m {
            let c1 = across(dirs[i - 1], spine[i]) + offset(dirs[i - 1], j);
            let c2 = across(dirs[i], spine[i]) + offset(dirs[i], j);
            out.push(meet(dirs[i - 1], c1, dirs[i], c2)?);
        }
        let (lx, ly) = left(dirs[m - 1]);
        out.push(Point::new(spine[m].x + PITCH * j * lx, spine[m].y + PITCH * j * ly));
        let mut steps = 0;
        for (i, w) in out.windows(2).enumerate() {
            let s = Segment::new(w[0], w[1]);
            if s.dir() != Some(dirs[i]) {
                return None;
            }
            let n = s.len_steps()?;
            if n < 2 {
                return None;
            }
            steps += n;
        }
        (steps >= MIN_STRETCH).then_some(out)
    };
    let tracks = (0..nt).map(|k| track(k - c)).collect::<Option<_>>()?;
    Some((dirs, tracks))
}

/// One spine search state.
#[derive(Clone, Copy)]
struct SpineRec {
    cell: u32,
    dir: Dir,
    run: u8,
    cost: u32,
    parent: u32,
}

/// The spine from `sa` to `sb` on the cells `fat` leaves free: its bends,
/// first and last segment on an axis, every segment at least `min`
/// steps, each bend 45° (CORE-6); `None` when there is none.
fn spine(grid: Grid, (fat, bans): (&[bool], &[u8]), (sa, start_dir): (Point, Option<Dir>), (sb, end_dir): (Point, Option<Dir>), min: i32) -> Option<Vec<Point>> {
    let start = grid.index(sa)?;
    let goal = grid.index(sb)?;
    let min = u8::try_from(min.clamp(1, 200)).ok()?;
    let h = |p: Point| -> u32 {
        let dx = (p.x - sb.x).unsigned_abs();
        let dy = (p.y - sb.y).unsigned_abs();
        H_MAX * dx.max(dy) + H_MIN * dx.min(dy)
    };
    let mut best: HashMap<(u32, u8, u8), u32> = HashMap::new();
    let mut recs: Vec<SpineRec> = Vec::new();
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    let push = |r: SpineRec, best: &mut HashMap<(u32, u8, u8), u32>, recs: &mut Vec<SpineRec>, heap: &mut BinaryHeap<Reverse<(u32, u32)>>| {
        let key = (r.cell, r.dir as u8, r.run);
        if best.get(&key).is_some_and(|&c| c <= r.cost) {
            return;
        }
        best.insert(key, r.cost);
        let i = u32::try_from(recs.len()).unwrap_or(u32::MAX);
        recs.push(r);
        heap.push(Reverse((r.cost.saturating_add(h(grid.point(r.cell as usize))), i)));
    };
    let start_cell = u32::try_from(start).ok()?;
    for d in [Dir::E, Dir::N, Dir::W, Dir::S] {
        if start_dir.is_some_and(|s| s != d) {
            continue;
        }
        push(
            SpineRec {
                cell: start_cell,
                dir: d,
                run: 0,
                cost: 0,
                parent: u32::MAX,
            },
            &mut best,
            &mut recs,
            &mut heap,
        );
    }
    let mut pops = 0;
    while let Some(Reverse((_, i))) = heap.pop() {
        pops += 1;
        if pops > SPINE_POPS {
            return None;
        }
        let r = recs[i as usize];
        if best.get(&(r.cell, r.dir as u8, r.run)) != Some(&r.cost) {
            continue;
        }
        if r.cell as usize == goal && r.run >= min && !r.dir.is_diagonal() && end_dir.is_none_or(|d| d == r.dir) {
            let mut cells = Vec::new();
            let mut k = i;
            loop {
                let q = recs[k as usize];
                cells.push(grid.point(q.cell as usize));
                if q.parent == u32::MAX {
                    break;
                }
                k = q.parent;
            }
            cells.reverse();
            cells.dedup();
            if cells.len() < 2 {
                return None;
            }
            return Some(vertices(&cells));
        }
        let c = grid.point(r.cell as usize);
        for by in [0i8, 1, -1] {
            if by != 0 && r.run < min {
                continue;
            }
            let nd = super::turn(r.dir, by);
            let n = c.step(nd);
            let Some(ni) = grid.index(n) else { continue };
            if fat[ni] || !along_ok(bans, r.cell as usize, ni, nd) {
                continue;
            }
            let mut step = if nd.is_diagonal() { STEP_DIAGONAL } else { STEP_STRAIGHT };
            if by != 0 {
                step += SPINE_BEND;
            }
            push(
                SpineRec {
                    cell: u32::try_from(ni).ok()?,
                    dir: nd,
                    run: if by == 0 { r.run.saturating_add(1).min(min) } else { 1 },
                    cost: r.cost + step,
                    parent: i,
                },
                &mut best,
                &mut recs,
                &mut heap,
            );
        }
    }
    None
}

/// The line of member `e` on `board`: a fan-out from its source port
/// through its first `nsrc` vertices to its track's start, the track
/// `t` (in the member's direction), and a fan-out through its other
/// vertices to its destination port (P4-410, P4-415, P4-416).
#[allow(clippy::too_many_lines)]
fn member_line(ctx: &Ctx<'_>, board: &Board, e: usize, t: &[Point], nsrc: usize) -> Option<Line> {
    let plan = ctx.plan;
    let g = board.grid;
    let pe = &plan.edges[e];
    let (sv, dv) = pe.vertices.split_at(nsrc);
    let t0 = *t.first()?;
    let tn = *t.last()?;
    let d0 = dir(t[0], t[1])?;
    let dn = dir(t[t.len() - 2], t[t.len() - 1])?;
    let sbox = &plan.boxes[pe.source.0 as usize];
    let dbox = &plan.boxes[pe.destination.0 as usize];
    let (scentre, dcentre) = (ctx.centres[pe.source.0 as usize], ctx.centres[pe.destination.0 as usize]);
    let starts: Vec<Start> = end_ports(board, sbox, scentre, pe.source_anchor, sv.first().copied().unwrap_or(t0))
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
    let mut at: Option<Start> = None;
    let mark = |cells: &[Point], own: &mut Vec<bool>| {
        for &c in cells {
            if let Some(i) = g.index(c) {
                own[i] = true;
            }
        }
    };
    // A track ending 2 cells in front of a port of its box (a shared side)
    // runs straight into that port: the fan-out is the port stub alone.
    let direct = |b: &crate::view::PlanBox, centre: Point, at: Point, out: Dir, anchor: bool, vertices: bool| -> Option<Port> {
        if anchor || vertices {
            return None;
        }
        let port = super::spots(b, centre).into_iter().map(|s| s.port).find(|p| p.at == at && side_dir(p.side) == out)?;
        let mid = g.index(at.step(out))?;
        (board.cells[mid].net == 0 || board.cells[mid].owner as usize == e + 1).then_some(port)
    };
    let back0 = Point::new(t0.x - 2 * unit(d0).0, t0.y - 2 * unit(d0).1);
    let src_direct = direct(sbox, scentre, back0, d0, pe.source_anchor.is_some(), !sv.is_empty());
    // Source fan-out: vertices, then the track's start.
    let mut goals: Vec<Goal<'_>> = sv.iter().map(|&v| g.index(v).map(Goal::Cell)).collect::<Option<_>>()?;
    goals.push(Goal::Entry(g.index(t0)?, d0));
    if let Some(p) = src_direct {
        source = Some(p);
        cells.push(p.at);
        cells.push(p.at.step(d0));
        cells.push(t0);
        goals.clear();
    }
    for goal in &goals {
        let found = match at {
            None => leg(board, e, &starts, false, goal, &own, &NEVER).ok()??,
            Some(s) => leg(board, e, &[s], true, goal, &own, &NEVER).ok()??,
        };
        if at.is_none() {
            let p = found.start?;
            source = Some(p);
            cells.push(p.at);
            cells.push(p.at.step(side_dir(p.side)));
            cells.extend_from_slice(&found.cells);
        } else {
            cells.extend_from_slice(&found.cells[1..]);
        }
        mark(&cells, &mut own);
        let here = *found.cells.last()?;
        at = Some(Start {
            cell: g.index(here)?,
            dir: found.dir,
            cost: 0,
            port: None,
        });
    }
    // The track.
    cells.extend_from_slice(&cells_of(t)[1..]);
    mark(&cells, &mut own);
    // Destination fan-out: from the track's end, vertices, then a port.
    let mut at = Start {
        cell: g.index(tn)?,
        dir: dn,
        cost: 0,
        port: None,
    };
    let mut first = true;
    for &v in dv {
        let found = leg(board, e, &[at], !first, &Goal::Cell(g.index(v)?), &own, &NEVER).ok()??;
        cells.extend_from_slice(&found.cells[1..]);
        mark(&cells, &mut own);
        at = Start {
            cell: g.index(v)?,
            dir: found.dir,
            cost: 0,
            port: None,
        };
        first = false;
    }
    let ahead = Point::new(tn.x + 2 * unit(dn).0, tn.y + 2 * unit(dn).1);
    if let Some(port) = direct(dbox, dcentre, ahead, dn.opposite(), pe.destination_anchor.is_some(), !dv.is_empty()) {
        cells.push(port.at.step(side_dir(port.side)));
        cells.push(port.at);
        return Some(Line {
            edge: edge_id(e),
            points: vertices(&cells),
            source: source?,
            end: LineEnd::Arrow(port),
        });
    }
    let mut ports: Vec<(usize, Port)> = end_ports(board, dbox, dcentre, pe.destination_anchor, dv.last().copied().unwrap_or(tn))
        .into_iter()
        .map(|(s, c)| (c, s.port))
        .collect();
    ports.sort_unstable_by_key(|&(c, _)| c);
    let found = leg(board, e, &[at], !first, &Goal::Ports(&ports), &own, &NEVER).ok()??;
    cells.extend_from_slice(&found.cells[1..]);
    let port = found.end?;
    cells.push(port.at.step(side_dir(port.side)));
    cells.push(port.at);
    Some(Line {
        edge: edge_id(e),
        points: vertices(&cells),
        source: source?,
        end: LineEnd::Arrow(port),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P4-406, P4-407 on a spine that turns right then left: offsets of
    /// 2 on axis segments and 3 on diagonal ones, and at each bend the
    /// inner track bends first along the incoming direction.
    #[test]
    fn p4_407_tracks_of_a_bent_spine() {
        let spine = [Point::new(0, 0), Point::new(20, 0), Point::new(30, 10), Point::new(30, 30), Point::new(40, 40), Point::new(60, 40)];
        let (dirs, tracks) = tracks_of(&spine, 3).expect("tracks");
        assert_eq!(tracks[1], spine.to_vec(), "track 1 on the spine");
        for k in 0..2 {
            let (t, u) = (&tracks[k], &tracks[k + 1]);
            for (i, &d) in dirs.iter().enumerate() {
                assert_eq!(across(d, u[i]) - across(d, t[i]), offset(d, 1), "pitch, segment {i}");
                assert_eq!(dir(t[i], t[i + 1]), Some(d));
            }
            for i in 1..dirs.len() {
                let left_turn = Dir::ALL[(dirs[i - 1] as usize + 1) % 8] == dirs[i];
                let (inner, outer) = if left_turn { (u, t) } else { (t, u) };
                assert!(along(dirs[i - 1], inner[i]) < along(dirs[i - 1], outer[i]), "inner bends first at bend {i}");
            }
        }
    }
}
