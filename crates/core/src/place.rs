//! Automatic placement: gives a grid position to every box of a view
//! plan that has none, never moving a placed box, scoring candidates by
//! straight-line crossings (CORE-22, 23).
//!
//! Depends on: `view`, `geom`, `model` (`ElementId` keys `Keep::previous`;
//! `ElementKind` tells a person box apart).

// Shape of the API: one function, `place(&ViewPlan, &Keep) -> Vec<Point>`,
// returning the centre of every box in grid units, indexed by `BoxId`
// (`centres[i]` is the centre of `plan.boxes[i]`). That vector is the
// `&[Point]` that `route`, `label`, `scene` and `ViewPlan::boundary_rect`
// take next to the plan. The plan stays immutable (view.rs header).
//
// Which centre a box gets, first match wins:
//   1. `Keep::moved`: a centre the editor sets while dragging (EDIT-2,
//      EDIT-20 several boxes) or after an auto-placed box was dragged
//      (EDIT-4);
//   2. its `at` line, `PlanBox::centre` (CORE-22: never moved);
//   3. `Keep::previous`: its centre in the previous scene of the same
//      session, matched by `PlanBox::element` (CORE-29);
//   4. otherwise it is auto-placed on candidate slots of a coarse grid
//      (CORE-23 pitch), clear of every box from 1..3 with L6 clearance,
//      scored by straight-line crossings then total edge length (place
//      plan, SKILL.md "Placing boxes").
// The CLI and a fresh build pass `Keep::default()`; the editor passes
// the last scene's centres (`Keep::from_previous`) plus the dragged
// boxes. When every box is covered by 1..3 nothing is searched, so the
// drag preview stays cheap (EDIT-25).
//
// DECISION: slot pitch is private and tunable (34 x 28 units = 340px x
// 280px, CORE-23 "at least"); it is not pub because no other module
// needs it and `at`/previous boxes are not bound by it.
// DECISION: `previous` is keyed by `ElementId` because view.rs names
// `PlanBox::element` the identity across plans (CORE-29). `ElementId` is
// an index in source order, stable for one parse only (model.rs); after
// a re-parse the caller remaps old ids to new ones through `Element::key`
// and `Model::find` before calling `place`. That remap is not owned by
// any contract yet: docs/deferred.md row "editor/pipeline, 4, remap
// Keep::previous across a re-parse".
// DECISION: in phase 1 an element has at most one box; a `previous`
// entry applies to the first box of its element (`ViewPlan::boxes_of`
// order); phase 3 may add a per-box key to `Keep` (it is
// `#[non_exhaustive]`).
// DECISION: a `moved` centre is taken as given (the editor snaps the
// pointer to the grid, EDIT-2); a moved box may overlap another box:
// CORE-22's no-overlap rule binds the boxes `place` positions, and lint
// reports overlaps of hand-set boxes.
// DECISION: `Keep` uses `BTreeMap`s: one entry per key by construction,
// deterministic iteration (CORE-25), no duplicate-resolution rule.
// DECISION: a stale `moved` entry (BoxId out of range, e.g. an EDIT-1
// reload during a drag shrank the plan) is ignored, as `previous`
// ignores undrawn elements; no panic.
//
// CORE-29: a kept position wins over CORE-22's no-overlap rule. When a
// kept previous centre now overlaps a box whose `at` line changed or a
// box that was dragged, the previous centre is kept and lint reports the
// overlap (L6).
//
// CORE-23's routed clauses (no unroutable line, no label without room,
// crossings within cap) need `route` and `label`, which depend on
// `place`; they live in crates/core/tests/core_23_example.rs, not here.

use std::collections::BTreeMap;

use crate::geom::{Point, Rect};
use crate::model::{ElementId, ElementKind};
use crate::view::{BoxId, ViewPlan};

/// Centres `place` must keep besides the `at` lines of the plan: the
/// previous scene's (CORE-29) and the ones the editor sets by dragging
/// (EDIT-2, EDIT-4, EDIT-20). `Keep::default()` keeps nothing but the
/// `at` lines (CLI, first build of a view). `#[non_exhaustive]` so a
/// later phase can add kinds of kept positions; build it with `Default`
/// or `Keep::from_previous` and insert into the maps. Serves CORE-29,
/// EDIT-4, EDIT-20.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Keep {
    /// Centre of each element drawn in the previous scene of the same
    /// session, in grid units, keyed by the element's id in the model
    /// the plan being placed was built from. Ids from an older parse
    /// must be remapped first (see the module header and the
    /// docs/deferred.md row on re-parse). Applies to a box with no `at`
    /// line and not in `moved`; an element the plan does not draw is
    /// ignored (CORE-29, EDIT-21).
    pub previous: BTreeMap<ElementId, Point>,
    /// Centre of each box the user is dragging or has dragged and not
    /// yet written back, in grid units; wins over its `at` line and
    /// `previous` (EDIT-2, EDIT-4, EDIT-20). Auto-placed boxes keep L6
    /// clearance from these too. A `BoxId` out of range for the plan is
    /// ignored.
    pub moved: BTreeMap<BoxId, Point>,
}

impl Keep {
    /// A `Keep` whose `previous` holds every box of `plan` at
    /// `centres[box]`: what the last `place` returned for it (CORE-29),
    /// with `moved` empty. Keys are `plan`'s own element ids, so it is
    /// valid as is for a plan from the same parse; across a re-parse the
    /// keys need the remap named on `Keep::previous`. The editor builds
    /// it from the scene on screen before a reload or a drag. Serves
    /// CORE-29.
    ///
    /// # Panics
    ///
    /// When `centres` is shorter than `plan.boxes`.
    pub fn from_previous(plan: &ViewPlan, centres: &[Point]) -> Self {
        assert!(
            centres.len() >= plan.boxes.len(),
            "from_previous: {} centres for {} boxes",
            centres.len(),
            plan.boxes.len()
        );
        let mut previous = BTreeMap::new();
        for b in &plan.boxes {
            // First box of its element wins (module header DECISION).
            previous
                .entry(b.element)
                .or_insert(centres[b.id.0 as usize]);
        }
        Self {
            previous,
            moved: BTreeMap::new(),
        }
    }
}

/// The centre of every box of `plan`, in grid units, indexed by `BoxId`
/// (`result.len() == plan.boxes.len()`). A box in `keep.moved` gets that
/// centre; else a box with an `at` line keeps it (never moved, CORE-22);
/// else a box in `keep.previous` keeps its previous centre (CORE-29);
/// every other box (`ViewPlan::unplaced`, DSL-8) is auto-placed on the
/// grid so that its rectangle, inflated by 3 units (30px) on every side,
/// overlaps no other box's rectangle inflated by 3 units (CORE-22, no L6
/// on the result); no auto-placed box sits on a row above an auto-placed
/// person; boxes that are not boundary members are outside every
/// boundary rectangle (`ViewPlan::boundary_rect`); auto-placed boxes
/// sharing a row are at least 340px apart centre to centre and rows at
/// least 280px apart; slots are chosen to minimise straight-line
/// crossings between edge ends, then total edge length (CORE-23).
/// Deterministic: same inputs, same centres (CORE-25). When no box needs
/// auto-placing it returns the kept centres without searching (EDIT-25).
/// Out-of-range `keep.moved` entries and undrawn `keep.previous` entries
/// are ignored. Serves CORE-22, CORE-23, CORE-25, CORE-29, EDIT-4,
/// EDIT-20, EDIT-25.
pub fn place(plan: &ViewPlan, keep: &Keep) -> Vec<Point> {
    let kept = kept_centres(plan, keep);
    if kept.iter().all(Option::is_some) {
        return kept.into_iter().flatten().collect();
    }
    let mut layout = Layout::new(plan, &kept);
    layout.seed();
    layout.search();
    layout.finish()
}

/// Column pitch of the slot lattice in grid units (340px, CORE-23).
const COL_PITCH: i32 = 34;
/// Row pitch of the slot lattice in grid units (280px, CORE-23).
const ROW_PITCH: i32 = 28;
/// L6 clearance in grid units (30px on every side, CORE-22).
const CLEAR: i32 = 3;
/// Margin left of and above the drawing when nothing is kept, in grid
/// units (40px).
const MARGIN: i32 = 4;
// DECISION: one weighted integer cost instead of a strict
// lexicographic order (crossings, then length): crossings weigh most,
// then a straight line through a third box (place.py `thru`, it forces
// a detour), then length in grid units; small terms for upward lines
// (callers above, place plan seed) and distance to the anchor (compact
// grid when there are no edges). Weights tuned on the place.py graph:
// 2 crossings against place.py's 4 (core_23_example_no_worse_than_place_py).
/// Cost of one straight-line crossing.
const W_CROSS: i64 = 1000;
/// Cost of one straight line passing through a box other than its ends.
const W_THROUGH: i64 = 700;
/// Cost of one line pointing up (callers sit above what they use).
const W_UP: i64 = 300;
/// Squared distance to the anchor is divided by this (compactness).
const COMPACT_DIV: i64 = 200;
// DECISION: threshold accepting (integer threshold falling linearly to
// 0) instead of place.py's annealing: `f64::exp` is not guaranteed
// bit-identical across platforms, and CORE-25 wants the same output on
// every machine.
/// Search moves per auto-placed box.
const MOVES_PER_BOX: usize = 1500;
/// Upper bound on search moves for one view.
const MAX_MOVES: usize = 120_000;
/// Starting threshold of the threshold-accepting search.
const THRESHOLD: i64 = 600;
/// Fixed search seed (CORE-25).
const SEED: u64 = 0x5EED_C4C4_0000_0001;
/// Window growths tried with every constraint before the seed falls back
/// to L6 clearance only.
const STRICT_GROWTHS: usize = 8;

/// The kept centre of every box, first match wins: `moved`, `at` line,
/// `previous` (first box of its element only).
fn kept_centres(plan: &ViewPlan, keep: &Keep) -> Vec<Option<Point>> {
    let mut seen = std::collections::BTreeSet::new();
    plan.boxes
        .iter()
        .map(|b| {
            let first = seen.insert(b.element);
            keep.moved.get(&b.id).copied().or(b.centre).or_else(|| {
                if first {
                    keep.previous.get(&b.element).copied()
                } else {
                    None
                }
            })
        })
        .collect()
}

/// Deterministic splitmix64 (CORE-25: no OS randomness).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform index below `n` (`n > 0`).
    fn below(&mut self, n: usize) -> usize {
        // DECISION: modulo bias is irrelevant for a search heuristic and
        // keeps the sequence platform independent (u64 arithmetic only).
        usize::try_from(self.next() % (n as u64)).unwrap_or(0)
    }
}

/// A lattice slot: column and row index.
type Slot = (i32, i32);

/// Placement state: every box's current centre, which boxes are kept,
/// which are active (kept or already seeded), and the slot of each
/// auto-placed box.
struct Layout<'a> {
    plan: &'a ViewPlan,
    pos: Vec<Point>,
    fixed: Vec<bool>,
    active: Vec<bool>,
    slot_of: Vec<Option<Slot>>,
    occupant: BTreeMap<Slot, usize>,
    /// Edge ends as box indices.
    ends: Vec<(usize, usize)>,
    /// Edge indices touching each box.
    incident: Vec<Vec<usize>>,
    /// Per boundary: membership of every box.
    member: Vec<Vec<bool>>,
    /// Per boundary: kept non-members already inside the rectangle of
    /// the kept members, exempt from the boundary rule.
    exempt: Vec<Vec<bool>>,
    origin: Point,
    anchor: Point,
    /// Window of lattice slots, inclusive: (min col, min row, max col,
    /// max row).
    window: (i32, i32, i32, i32),
    /// False once the seed had to drop the people and boundary rules.
    strict: bool,
}

impl<'a> Layout<'a> {
    fn new(plan: &'a ViewPlan, kept: &[Option<Point>]) -> Self {
        let n = plan.boxes.len();
        let fixed: Vec<bool> = kept.iter().map(Option::is_some).collect();
        let pos: Vec<Point> = kept.iter().map(|c| c.unwrap_or_default()).collect();
        let ends: Vec<(usize, usize)> = plan
            .edges
            .iter()
            .map(|e| (e.source.0 as usize, e.destination.0 as usize))
            .collect();
        let mut incident = vec![Vec::new(); n];
        for (i, &(s, d)) in ends.iter().enumerate() {
            incident[s].push(i);
            if d != s {
                incident[d].push(i);
            }
        }
        let member: Vec<Vec<bool>> = plan
            .boundaries
            .iter()
            .map(|b| {
                let mut m = vec![false; n];
                for id in &b.members {
                    m[id.0 as usize] = true;
                }
                m
            })
            .collect();
        let kept_pts: Vec<Point> = kept.iter().flatten().copied().collect();
        // DECISION: the lattice is anchored on the first kept box, so a
        // box beside a kept neighbour sits exactly one pitch away (plan
        // Review Focus, SKILL.md rule 3); with nothing kept it starts at 0
        // and the result is shifted to the margin at the end.
        let origin = kept.iter().flatten().next().copied().unwrap_or_default();
        let auto = n - kept_pts.len();
        let side = i32::try_from(auto.isqrt()).unwrap_or(i32::MAX / 4) + 2;
        let (window, anchor) = if kept_pts.is_empty() {
            let mid = side / 2;
            (
                (0, 0, side, side),
                Point::new(mid * COL_PITCH, mid * ROW_PITCH),
            )
        } else {
            let lo_c = kept_pts
                .iter()
                .map(|p| (p.x - origin.x).div_euclid(COL_PITCH))
                .min();
            let hi_c = kept_pts
                .iter()
                .map(|p| (p.x - origin.x).div_euclid(COL_PITCH) + 1)
                .max();
            let lo_r = kept_pts
                .iter()
                .map(|p| (p.y - origin.y).div_euclid(ROW_PITCH))
                .min();
            let hi_r = kept_pts
                .iter()
                .map(|p| (p.y - origin.y).div_euclid(ROW_PITCH) + 1)
                .max();
            let count = i64::try_from(kept_pts.len()).unwrap_or(1);
            let sx: i64 = kept_pts.iter().map(|p| i64::from(p.x)).sum();
            let sy: i64 = kept_pts.iter().map(|p| i64::from(p.y)).sum();
            let anchor = Point::new(
                i32::try_from(sx.div_euclid(count)).unwrap_or(0),
                i32::try_from(sy.div_euclid(count)).unwrap_or(0),
            );
            (
                (
                    lo_c.unwrap_or(0) - side,
                    lo_r.unwrap_or(0) - side,
                    hi_c.unwrap_or(0) + side,
                    hi_r.unwrap_or(0) + side,
                ),
                anchor,
            )
        };
        let mut layout = Self {
            plan,
            pos,
            active: fixed.clone(),
            fixed,
            slot_of: vec![None; n],
            occupant: BTreeMap::new(),
            ends,
            incident,
            exempt: Vec::new(),
            member,
            origin,
            anchor,
            window,
            strict: true,
        };
        layout.exempt = (0..layout.member.len())
            .map(|k| {
                let r = layout.boundary_rect(k);
                (0..n)
                    .map(|b| {
                        layout.fixed[b]
                            && !layout.member[k][b]
                            && r.is_some_and(|r| r.overlaps(&layout.rect(b)))
                    })
                    .collect()
            })
            .collect();
        layout
    }

    fn is_person(&self, b: usize) -> bool {
        self.plan.boxes[b].kind == ElementKind::Person
    }

    fn rect(&self, b: usize) -> Rect {
        self.plan.boxes[b].rect_at(self.pos[b])
    }

    fn centre_of(&self, s: Slot) -> Point {
        Point::new(
            self.origin.x + s.0 * COL_PITCH,
            self.origin.y + s.1 * ROW_PITCH,
        )
    }

    /// Rectangle of boundary `k` over its active members, `None` when
    /// none is active (`ViewPlan::boundary_rect` rule: bounds plus 40px).
    fn boundary_rect(&self, k: usize) -> Option<Rect> {
        let bounds = (0..self.pos.len())
            .filter(|&b| self.member[k][b] && self.active[b])
            .map(|b| self.rect(b))
            .reduce(|a, b| {
                Rect::from_corners(
                    Point::new(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
                    Point::new(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
                )
            })?;
        Some(bounds.inflate(4))
    }

    /// True when the active layout keeps the rules for the boxes in `m`
    /// (L6 clearance) and, when strict, the people and boundary rules.
    fn valid(&self, m: &[usize]) -> bool {
        for &b in m {
            let r = self.rect(b).inflate(CLEAR);
            let clash = (0..self.pos.len())
                .any(|o| o != b && self.active[o] && r.overlaps(&self.rect(o).inflate(CLEAR)));
            if clash {
                return false;
            }
        }
        if !self.strict {
            return true;
        }
        self.people_ok() && self.boundaries_ok()
    }

    /// Every auto-placed person on one row, no auto-placed box above it
    /// (CORE-23).
    // DECISION: "no auto-placed box above an auto-placed person" also
    // binds people among themselves, so all auto-placed people share the
    // top row of the auto-placed boxes.
    fn people_ok(&self) -> bool {
        let autos = (0..self.pos.len()).filter(|&b| self.active[b] && !self.fixed[b]);
        let top = autos.clone().map(|b| self.pos[b].y).min();
        autos
            .filter(|&b| self.is_person(b))
            .all(|b| Some(self.pos[b].y) == top)
    }

    /// No active non-member overlaps a boundary rectangle, except kept
    /// boxes already inside the kept members' rectangle (CORE-23, L26).
    // DECISION: a kept non-member already inside the kept members'
    // rectangle is the author's choice (lint L26 reports it); it does not
    // make every slot invalid.
    fn boundaries_ok(&self) -> bool {
        (0..self.member.len()).all(|k| {
            let Some(r) = self.boundary_rect(k) else {
                return true;
            };
            (0..self.pos.len()).all(|b| {
                !self.active[b]
                    || self.member[k][b]
                    || self.exempt[k][b]
                    || !r.overlaps(&self.rect(b))
            })
        })
    }

    /// Cost of every term that involves a box of `m`, over active boxes
    /// and edges with both ends active.
    fn cost(&self, moved: &[usize]) -> i64 {
        let in_m = |b: usize| moved.contains(&b);
        let edge_on = |e: usize| {
            let (s, d) = self.ends[e];
            self.active[s] && self.active[d]
        };
        let touches = |e: usize| {
            let (s, d) = self.ends[e];
            in_m(s) || in_m(d)
        };
        let mut touching: Vec<usize> = moved
            .iter()
            .flat_map(|&b| self.incident[b].iter().copied())
            .filter(|&e| edge_on(e))
            .collect();
        touching.sort_unstable();
        touching.dedup();
        let mut total = 0;
        for &e in &touching {
            let (s, d) = self.ends[e];
            let (a, b) = (self.pos[s], self.pos[d]);
            total += i64::from((a.x - b.x).abs() + (a.y - b.y).abs());
            if a.y > b.y {
                total += W_UP;
            }
            for f in 0..self.ends.len() {
                if f == e || !edge_on(f) || (touches(f) && f < e) {
                    continue;
                }
                let (s2, d2) = self.ends[f];
                if s2 == s || s2 == d || d2 == s || d2 == d {
                    continue;
                }
                if proper_cross(a, b, self.pos[s2], self.pos[d2]) {
                    total += W_CROSS;
                }
            }
            for o in 0..self.pos.len() {
                if self.active[o] && o != s && o != d && through(a, b, &self.rect(o)) {
                    total += W_THROUGH;
                }
            }
        }
        for f in (0..self.ends.len()).filter(|&f| edge_on(f) && !touches(f)) {
            let (s, d) = self.ends[f];
            for &o in moved {
                if through(self.pos[s], self.pos[d], &self.rect(o)) {
                    total += W_THROUGH;
                }
            }
        }
        for &o in moved {
            if !self.fixed[o] {
                let (dx, dy) = (
                    i64::from(self.pos[o].x - self.anchor.x),
                    i64::from(self.pos[o].y - self.anchor.y),
                );
                total += (dx * dx + dy * dy) / COMPACT_DIV;
            }
        }
        total
    }

    fn slots(&self) -> Vec<Slot> {
        let (c0, r0, c1, r1) = self.window;
        (r0..=r1)
            .flat_map(|r| (c0..=c1).map(move |c| (c, r)))
            .collect()
    }

    fn grow(&mut self) {
        let (c0, r0, c1, r1) = self.window;
        self.window = (c0 - 1, r0 - 1, c1 + 1, r1 + 1);
    }

    fn put(&mut self, b: usize, s: Slot) {
        if let Some(old) = self.slot_of[b] {
            self.occupant.remove(&old);
        }
        self.slot_of[b] = Some(s);
        self.occupant.insert(s, b);
        self.pos[b] = self.centre_of(s);
    }

    /// Greedy seed: boundary members first, then other boxes, then
    /// people, each in layer order (callers before the boxes they use),
    /// each into the cheapest valid free slot.
    fn seed(&mut self) {
        let layer = layers(self.pos.len(), &self.ends);
        let group = |b: usize| {
            if self.member.iter().any(|m| m[b]) {
                0
            } else if self.is_person(b) {
                2
            } else {
                1
            }
        };
        let mut order: Vec<usize> = (0..self.pos.len()).filter(|&b| !self.fixed[b]).collect();
        // DECISION: members first so the boundary is settled before the
        // boxes that must stay outside it; people last so their row can
        // always be found above everything else (CORE-23).
        order.sort_by_key(|&b| (group(b), layer[b], b));
        for b in order {
            self.insert(b);
        }
    }

    fn insert(&mut self, b: usize) {
        self.active[b] = true;
        let mut growths = 0;
        loop {
            let mut best: Option<(i64, Slot)> = None;
            for s in self.slots() {
                if self.occupant.contains_key(&s) {
                    continue;
                }
                self.pos[b] = self.centre_of(s);
                if !self.valid(&[b]) {
                    continue;
                }
                let c = self.cost(&[b]);
                if best.is_none_or(|(bc, _)| c < bc) {
                    best = Some((c, s));
                }
            }
            if let Some((_, s)) = best {
                self.put(b, s);
                return;
            }
            growths += 1;
            if growths > STRICT_GROWTHS {
                // DECISION: unreachable in practice; if the people or
                // boundary rule cannot be met near the drawing, keep L6
                // clearance only (always met far enough out) rather than
                // loop forever.
                self.strict = false;
            }
            self.grow();
        }
    }

    /// Threshold-accepting local search over slot moves and swaps,
    /// keeping the best layout seen.
    fn search(&mut self) {
        let autos: Vec<usize> = (0..self.pos.len()).filter(|&b| !self.fixed[b]).collect();
        let slots = self.slots();
        let moves = (MOVES_PER_BOX * autos.len()).min(MAX_MOVES);
        let mut rng = Rng(SEED);
        let mut cur = self.cost(&autos);
        let mut best = (cur, self.slot_of.clone());
        let total = i64::try_from(moves).unwrap_or(i64::MAX);
        for step in 0..total {
            let threshold = THRESHOLD * (total - step) / total;
            let b = autos[rng.below(autos.len())];
            let s = slots[rng.below(slots.len())];
            let Some(from) = self.slot_of[b] else {
                continue;
            };
            if from == s {
                continue;
            }
            let other = self.occupant.get(&s).copied();
            let m: Vec<usize> = other.map_or_else(|| vec![b], |o| vec![b, o]);
            let before = self.cost(&m);
            self.swap(b, from, s, other);
            if !self.valid(&m) {
                self.swap(b, s, from, other);
                continue;
            }
            let delta = self.cost(&m) - before;
            if delta <= 0 || delta < threshold {
                cur += delta;
                if cur < best.0 {
                    best = (cur, self.slot_of.clone());
                }
            } else {
                self.swap(b, s, from, other);
            }
        }
        self.occupant.clear();
        for (b, s) in best.1.into_iter().enumerate() {
            if let Some(s) = s {
                self.slot_of[b] = None;
                self.put(b, s);
            }
        }
    }

    /// Moves `b` from slot `from` to `to`; `other`, the box on `to`,
    /// goes to `from`.
    fn swap(&mut self, b: usize, from: Slot, to: Slot, other: Option<usize>) {
        self.occupant.remove(&from);
        self.occupant.remove(&to);
        self.slot_of[b] = Some(to);
        self.occupant.insert(to, b);
        self.pos[b] = self.centre_of(to);
        if let Some(o) = other {
            self.slot_of[o] = Some(from);
            self.occupant.insert(from, o);
            self.pos[o] = self.centre_of(from);
        }
    }

    /// The centres; with nothing kept, shifted so the drawing starts at
    /// the margin.
    fn finish(self) -> Vec<Point> {
        if self.fixed.iter().any(|&f| f) {
            return self.pos;
        }
        let min_x = (0..self.pos.len())
            .map(|b| self.rect(b).min.x)
            .min()
            .unwrap_or(0);
        let min_y = (0..self.pos.len())
            .map(|b| self.rect(b).min.y)
            .min()
            .unwrap_or(0);
        // DECISION: shift by whole grid units only, so pitch and lattice
        // are preserved; the origin is cosmetic (scene owns the canvas).
        self.pos
            .iter()
            .map(|p| Point::new(p.x - min_x + MARGIN, p.y - min_y + MARGIN))
            .collect()
    }
}

/// Longest-path layer of every box over the edges, back edges of a
/// depth-first walk (boxes in index order) ignored.
fn layers(n: usize, ends: &[(usize, usize)]) -> Vec<usize> {
    let mut out = vec![Vec::new(); n];
    for &(s, d) in ends {
        if s != d {
            out[s].push(d);
        }
    }
    // Post-order DFS; state 0 new, 1 on stack, 2 done.
    let mut state = vec![0_u8; n];
    let mut post = Vec::with_capacity(n);
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, 0_usize)];
        state[root] = 1;
        while let Some(&mut (v, ref mut i)) = stack.last_mut() {
            if let Some(&w) = out[v].get(*i) {
                *i += 1;
                if state[w] == 0 {
                    state[w] = 1;
                    stack.push((w, 0));
                }
            } else {
                state[v] = 2;
                post.push(v);
                stack.pop();
            }
        }
    }
    let mut rank = vec![0_usize; n];
    for (r, &v) in post.iter().rev().enumerate() {
        rank[v] = r;
    }
    let mut layer = vec![0_usize; n];
    for &v in post.iter().rev() {
        for &w in &out[v] {
            if rank[w] > rank[v] {
                layer[w] = layer[w].max(layer[v] + 1);
            }
        }
    }
    layer
}

/// Sign of the turn `a -> b -> c`, exact in i64.
fn orient(a: Point, b: Point, c: Point) -> i64 {
    let v = (i64::from(b.x) - i64::from(a.x)) * (i64::from(c.y) - i64::from(a.y))
        - (i64::from(b.y) - i64::from(a.y)) * (i64::from(c.x) - i64::from(a.x));
    v.signum()
}

/// True when segments `ab` and `cd` cross at a point inside both
/// (place.py `cross`).
fn proper_cross(a: Point, b: Point, c: Point, d: Point) -> bool {
    orient(a, b, c) * orient(a, b, d) < 0 && orient(c, d, a) * orient(c, d, b) < 0
}

/// True when segment `ab` passes through the interior of `r` (separating
/// axes: x, y and the segment's normal).
fn through(a: Point, b: Point, r: &Rect) -> bool {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    if !(x0 < r.max.x && x1 > r.min.x && y0 < r.max.y && y1 > r.min.y) {
        return false;
    }
    let corners = [
        r.min,
        Point::new(r.max.x, r.min.y),
        r.max,
        Point::new(r.min.x, r.max.y),
    ];
    let signs = corners.map(|p| orient(a, b, p));
    signs.contains(&1) && signs.contains(&-1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Model, parse};
    use crate::view::plan;
    use std::fmt::Write as _;

    /// A file under the repository root, read at compile time.
    macro_rules! repo_file {
        ($path:literal) => {
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../", $path))
        };
    }

    /// CORE-23 pitch in grid units: 340px between columns, 280px rows.
    const COL: i32 = 34;
    const ROW: i32 = 28;
    /// CORE-22 / L6 clearance in grid units (30px).
    const L6: i32 = 3;

    fn workspace(model_body: &str, views_body: &str) -> String {
        format!(
            "workspace {{\n    model {{\n{model_body}\n    }}\n    views {{\n{views_body}\n    }}\n}}\n"
        )
    }

    fn model(source: &str) -> Model {
        match parse(source) {
            Ok(parsed) => parsed.model,
            Err(e) => panic!("{e}"),
        }
    }

    fn plan_of(m: &Model, key: &str) -> ViewPlan {
        plan(m, m.view(key).expect("view exists"))
    }

    /// The box of the element keyed `key`.
    fn box_of(m: &Model, p: &ViewPlan, key: &str) -> BoxId {
        let e = p
            .boxes
            .iter()
            .find(|b| {
                m.element(b.element)
                    .key
                    .as_ref()
                    .is_some_and(|k| k.value == key)
            })
            .unwrap_or_else(|| panic!("no box for {key}"));
        e.id
    }

    fn at(centres: &[Point], id: BoxId) -> Point {
        centres[id.0 as usize]
    }

    /// `source` with every `at` line removed (CORE-23 setup).
    fn strip_at(source: &str) -> String {
        let mut out = String::new();
        for l in source
            .lines()
            .filter(|l| !l.trim_start().starts_with("at "))
        {
            out.push_str(l);
            out.push('\n');
        }
        out
    }

    /// The example with every `at` line removed, and its four plans.
    fn example_plans() -> Vec<ViewPlan> {
        let m = model(&strip_at(repo_file!("tests/data/example.dsl")));
        m.views.iter().map(|v| plan(&m, v)).collect()
    }

    fn rect(p: &ViewPlan, centres: &[Point], id: BoxId) -> Rect {
        p.boxes[id.0 as usize].rect_at(at(centres, id))
    }

    /// Asserts no `auto` box, grown by 3 units, overlaps any other box
    /// grown by 3 units.
    fn assert_l6_clear(p: &ViewPlan, centres: &[Point], auto: &[BoxId]) {
        for &a in auto {
            let ra = rect(p, centres, a).inflate(L6);
            for b in &p.boxes {
                if b.id == a {
                    continue;
                }
                let rb = rect(p, centres, b.id).inflate(L6);
                assert!(
                    !ra.overlaps(&rb),
                    "{}: box {} at {:?} too close to box {} at {:?}",
                    p.key,
                    a.0,
                    at(centres, a),
                    b.id.0,
                    at(centres, b.id)
                );
            }
        }
    }

    /// Small container view: a role, two containers, one external.
    /// Every box unplaced unless `ats` adds `at` lines.
    fn small(ats: &str) -> (Model, ViewPlan) {
        let src = workspace(
            r#"        user = person "User" "A role."
        sys = softwareSystem "System" "Does things." {
            api = container "API" "Serves requests." "Rust"
            db = container "Store" "Keeps records." "SQLite"
            worker = container "Worker" "Runs jobs." "Rust"
        }
        ext = softwareSystem "Mailer" "Sends mail." "External"
        user -> api "Calls"
        api -> db "Reads and writes"
        worker -> db "Reads"
        api -> ext "Sends through"
"#,
            &format!("        container sys \"V\" {{\n            include *\n{ats}        }}"),
        );
        let m = model(&src);
        let p = plan_of(&m, "V");
        (m, p)
    }

    /// Proper crossings between straight lines centre to centre, counting
    /// only pairs with four distinct end boxes (place.py `cost`).
    fn straight_crossings(p: &ViewPlan, centres: &[Point]) -> usize {
        fn orient(a: Point, b: Point, c: Point) -> i64 {
            let v = (i64::from(b.x) - i64::from(a.x)) * (i64::from(c.y) - i64::from(a.y))
                - (i64::from(b.y) - i64::from(a.y)) * (i64::from(c.x) - i64::from(a.x));
            v.signum()
        }
        let mut count = 0;
        for (i, e) in p.edges.iter().enumerate() {
            for f in &p.edges[i + 1..] {
                let ends = [e.source, e.destination, f.source, f.destination];
                if (0..4).any(|k| ends[k + 1..].contains(&ends[k])) {
                    continue;
                }
                let (e0, e1) = (at(centres, e.source), at(centres, e.destination));
                let (f0, f1) = (at(centres, f.source), at(centres, f.destination));
                if orient(e0, e1, f0) * orient(e0, e1, f1) < 0
                    && orient(f0, f1, e0) * orient(f0, f1, e1) < 0
                {
                    count += 1;
                }
            }
        }
        count
    }

    // CORE-22, one test per clause.
    #[test]
    fn core_22_places_every_unplaced_box() {
        // One centre per box; every unplaced box gets one, on a slot of
        // its own.
        let (_, p) = small("");
        assert_eq!(p.unplaced().len(), p.boxes.len());
        let centres = place(&p, &Keep::default());
        assert_eq!(centres.len(), p.boxes.len());
        for (i, a) in centres.iter().enumerate() {
            assert!(!centres[i + 1..].contains(a), "two boxes share {a:?}");
        }
    }

    #[test]
    fn core_22_never_moves_placed_box() {
        let (m, p) = small("            at api 600 500\n            at ext 1400 900\n");
        let centres = place(&p, &Keep::default());
        assert_eq!(at(&centres, box_of(&m, &p, "api")), Point::new(60, 50));
        assert_eq!(at(&centres, box_of(&m, &p, "ext")), Point::new(140, 90));
    }

    #[test]
    fn core_22_no_overlap_with_l6_clearance() {
        // Every auto-placed rect inflated by 3 units overlaps no other
        // box (at, previous, moved or auto-placed) inflated by 3 units,
        // including an auto-placed box set next to an `at` box.
        let (m, p) = small("            at api 600 500\n");
        let mut keep = Keep::default();
        keep.previous.insert(
            p.boxes[box_of(&m, &p, "ext").0 as usize].element,
            Point::new(95, 50),
        );
        keep.moved
            .insert(box_of(&m, &p, "worker"), Point::new(25, 50));
        let centres = place(&p, &keep);
        let auto = [box_of(&m, &p, "user"), box_of(&m, &p, "db")];
        assert_l6_clear(&p, &centres, &auto);
    }

    #[test]
    fn placed_on_grid_no_overlap() {
        // Auto-placed boxes sit on one lattice of 34 x 28 units, clear of
        // each other and of the `at` box.
        let (m, p) = small("            at api 600 500\n");
        let centres = place(&p, &Keep::default());
        let auto: Vec<BoxId> = p.unplaced();
        let o = at(&centres, auto[0]);
        for &a in &auto {
            let c = at(&centres, a);
            assert_eq!(
                (c.x - o.x).rem_euclid(COL),
                0,
                "{c:?} off the column lattice"
            );
            assert_eq!((c.y - o.y).rem_euclid(ROW), 0, "{c:?} off the row lattice");
        }
        assert_l6_clear(&p, &centres, &auto);
        assert_eq!(at(&centres, box_of(&m, &p, "api")), Point::new(60, 50));
    }

    // CORE-23 pure-place clauses, on `tests/data/example.dsl` with every
    // `at` line removed. Routed clauses: tests/core_23_example.rs.
    #[test]
    fn core_23_people_on_top_row() {
        // No auto-placed box sits on a row above an auto-placed person.
        for p in example_plans() {
            let centres = place(&p, &Keep::default());
            for person in p.boxes.iter().filter(|b| b.kind == ElementKind::Person) {
                for b in &p.boxes {
                    assert!(
                        at(&centres, b.id).y >= at(&centres, person.id).y,
                        "{}: {} above person {}",
                        p.key,
                        b.name,
                        person.name
                    );
                }
            }
        }
    }

    #[test]
    fn core_23_outside_scope_outside_boundary() {
        let plans = example_plans();
        assert!(plans.iter().any(|p| !p.boundaries.is_empty()));
        for p in plans {
            let centres = place(&p, &Keep::default());
            for (i, boundary) in p.boundaries.iter().enumerate() {
                let r = p.boundary_rect(i, &centres).expect("boundary");
                for b in p.boxes.iter().filter(|b| !boundary.members.contains(&b.id)) {
                    assert!(
                        !r.overlaps(&b.rect_at(at(&centres, b.id))),
                        "{}: {} inside the boundary",
                        p.key,
                        b.name
                    );
                }
            }
            assert_l6_clear(&p, &centres, &p.unplaced());
        }
    }

    #[test]
    fn core_23_pitch_at_least_340_by_280() {
        for p in example_plans() {
            let centres = place(&p, &Keep::default());
            for (i, a) in centres.iter().enumerate() {
                for b in &centres[i + 1..] {
                    if a.y == b.y {
                        assert!((a.x - b.x).abs() >= COL, "{}: {a:?} {b:?}", p.key);
                    } else {
                        assert!((a.y - b.y).abs() >= ROW, "{}: {a:?} {b:?}", p.key);
                    }
                }
            }
        }
    }

    #[test]
    fn core_23_example_no_worse_than_place_py() {
        // The place.py graph (11 components, 5 outer boxes, one role
        // above web), auto-placed: straight-line crossings no higher than
        // place.py's best over 16 seeds, run once and pinned here.
        // place.py output, 2026-11-15: best (145.5, 4 crossings, 3 through).
        const PLACE_PY_CROSSINGS: usize = 4;
        let src = workspace(
            r#"        ppl = person "Role" "Uses the server."
        vikExt = softwareSystem "Tasks" "Task source." "External"
        pushExt = softwareSystem "Push" "Push endpoints." "External"
        smtpExt = softwareSystem "Mail" "Mail relay." "External"
        sys = softwareSystem "Server system" "The system." {
            app = container "Server" "The server." "Rust" {
                cli = component "cli" "Entry." "Rust"
                web = component "web" "HTTP." "Rust"
                sync = component "sync" "Poll." "Rust"
                devdata = component "devdata" "Seed." "Rust"
                ledger = component "ledger" "Rules." "Rust"
                auth = component "auth" "Accounts." "Rust"
                config = component "config" "Settings." "Rust"
                vik = component "vik" "Client." "Rust"
                notify = component "notify" "Push." "Rust"
                mail = component "mail" "Mail." "Rust"
                store = component "store" "Persistence." "Rust"
            }
            dbC = container "Ledger db" "Records." "SQLite"
            devdbC = container "Dev db" "Fake data." "SQLite"
        }
        cli -> web "a"
        cli -> sync "a"
        cli -> devdata "a"
        cli -> store "a"
        cli -> auth "a"
        cli -> notify "a"
        web -> config "a"
        web -> ledger "a"
        web -> auth "a"
        web -> store "a"
        web -> notify "a"
        web -> mail "a"
        sync -> config "a"
        sync -> vik "a"
        sync -> ledger "a"
        sync -> store "a"
        sync -> notify "a"
        devdata -> ledger "a"
        devdata -> auth "a"
        devdata -> store "a"
        vik -> config "a"
        vik -> vikExt "a"
        notify -> config "a"
        notify -> ledger "a"
        notify -> auth "a"
        notify -> pushExt "a"
        mail -> config "a"
        mail -> smtpExt "a"
        store -> ledger "a"
        store -> auth "a"
        store -> dbC "a"
        store -> devdbC "a"
        ppl -> web "a"
"#,
            "        component app \"P\" {\n            include *\n        }",
        );
        let m = model(&src);
        let p = plan_of(&m, "P");
        assert_eq!(p.boxes.len(), 17);
        assert_eq!(p.edges.len(), 33);
        let centres = place(&p, &Keep::default());
        let n = straight_crossings(&p, &centres);
        assert!(
            n <= PLACE_PY_CROSSINGS,
            "{n} crossings, place.py {PLACE_PY_CROSSINGS}"
        );
    }

    #[test]
    fn core_25_placement_deterministic() {
        // Two runs, same centres (fixed seed).
        for p in example_plans() {
            assert_eq!(place(&p, &Keep::default()), place(&p, &Keep::default()));
        }
    }

    // CORE-29 and the drag re-place (EDIT-2, EDIT-4, EDIT-20, EDIT-25).
    #[test]
    fn core_29_previous_scene_boxes_keep_position() {
        // Adding an unplaced box changes no previous centre.
        let before = workspace(
            r#"        user = person "User" "A role."
        a = softwareSystem "Alpha" "First."
        b = softwareSystem "Beta" "Second."
        user -> a "Uses"
        a -> b "Calls"
"#,
            "        systemContext a \"L\" {\n            include user a b\n        }",
        );
        let after = before
            .replace(
                "        a -> b \"Calls\"\n",
                "        a -> b \"Calls\"\n        c = softwareSystem \"Gamma\" \"Third.\"\n        b -> c \"Feeds\"\n        user -> c \"Reads\"\n",
            )
            .replace("include user a b", "include user a b c");
        let (m1, m2) = (model(&before), model(&after));
        let (p1, p2) = (plan_of(&m1, "L"), plan_of(&m2, "L"));
        assert_eq!(p2.boxes.len(), p1.boxes.len() + 1);
        let c1 = place(&p1, &Keep::default());
        let c2 = place(&p2, &Keep::from_previous(&p1, &c1));
        for b in &p1.boxes {
            let id2 = p2.boxes_of(b.element)[0];
            assert_eq!(at(&c2, id2), at(&c1, b.id), "{} moved", b.name);
        }
        assert_l6_clear(&p2, &c2, &[box_of(&m2, &p2, "c")]);
    }

    #[test]
    fn edit_4_dragged_auto_placed_box_keeps_position() {
        // `moved` wins over the previous centre and over the `at` line;
        // every other box keeps its centre.
        let (m, p) = small("            at api 600 500\n");
        let c0 = place(&p, &Keep::default());
        let mut keep = Keep::from_previous(&p, &c0);
        let (api, db) = (box_of(&m, &p, "api"), box_of(&m, &p, "db"));
        keep.moved.insert(api, Point::new(200, 200));
        keep.moved.insert(db, Point::new(-150, 300));
        let c1 = place(&p, &keep);
        assert_eq!(at(&c1, api), Point::new(200, 200));
        assert_eq!(at(&c1, db), Point::new(-150, 300));
        for b in p.boxes.iter().filter(|b| b.id != api && b.id != db) {
            assert_eq!(at(&c1, b.id), at(&c0, b.id), "{} moved", b.name);
        }
    }

    #[test]
    fn edit_4_stale_moved_entry_ignored() {
        // A `moved` BoxId out of range for the plan is ignored, no panic.
        let (_, p) = small("            at api 600 500\n");
        let mut keep = Keep::default();
        keep.moved.insert(BoxId(999), Point::new(1, 1));
        assert_eq!(place(&p, &keep), place(&p, &Keep::default()));
    }

    #[test]
    fn edit_20_unplaced_box_avoids_moved_boxes() {
        // With several boxes in `moved`, an unplaced box lands with L6
        // clearance from each moved centre, not from its old `at` centre.
        let (m, p) = small(
            "            at api 600 500\n            at db 600 800\n            at ext 1000 500\n            at worker 1000 800\n",
        );
        let c0 = place(&p, &Keep::default());
        let user = box_of(&m, &p, "user");
        let mut keep = Keep::default();
        // Drag api and db onto the slot user took, and next to it.
        keep.moved.insert(box_of(&m, &p, "api"), at(&c0, user));
        keep.moved.insert(
            box_of(&m, &p, "db"),
            Point::new(at(&c0, user).x + COL, at(&c0, user).y),
        );
        let c1 = place(&p, &keep);
        assert_l6_clear(&p, &c1, &[user]);
        assert_ne!(at(&c1, user), at(&c0, user));
    }

    #[test]
    fn edit_25_all_covered_returns_kept_centres() {
        // Every box covered by moved/at/previous: result equals those
        // centres, box by box.
        let (m, p) = small("            at api 600 500\n            at db 600 800\n");
        let (user, worker, ext) = (
            box_of(&m, &p, "user"),
            box_of(&m, &p, "worker"),
            box_of(&m, &p, "ext"),
        );
        let mut keep = Keep::default();
        keep.moved.insert(user, Point::new(60, 10));
        keep.moved.insert(box_of(&m, &p, "db"), Point::new(20, 80));
        keep.previous
            .insert(p.boxes[worker.0 as usize].element, Point::new(100, 80));
        keep.previous
            .insert(p.boxes[ext.0 as usize].element, Point::new(140, 50));
        let c = place(&p, &keep);
        assert_eq!(at(&c, user), Point::new(60, 10));
        assert_eq!(at(&c, box_of(&m, &p, "api")), Point::new(60, 50));
        assert_eq!(at(&c, box_of(&m, &p, "db")), Point::new(20, 80));
        assert_eq!(at(&c, worker), Point::new(100, 80));
        assert_eq!(at(&c, ext), Point::new(140, 50));
    }

    #[test]
    fn keep_from_previous_round_trips() {
        // `place(plan, &Keep::from_previous(plan, &centres)) == centres`.
        for p in example_plans() {
            let centres = place(&p, &Keep::default());
            let keep = Keep::from_previous(&p, &centres);
            assert!(keep.moved.is_empty());
            assert_eq!(keep.previous.len(), p.boxes.len());
            assert_eq!(place(&p, &keep), centres);
        }
    }

    // Review Focus of the place plan.
    #[test]
    fn all_placed_is_noop() {
        // Every box has an `at` line: centres equal the `at` values.
        let m = model(repo_file!("tests/data/example.dsl"));
        for v in &m.views {
            let p = plan(&m, v);
            assert!(p.unplaced().is_empty());
            let want: Vec<Point> = p.boxes.iter().map(|b| b.centre.expect("at")).collect();
            assert_eq!(place(&p, &Keep::default()), want);
        }
    }

    #[test]
    fn zero_edges_compact_grid() {
        // Boxes laid in a compact grid, not a single row.
        let mut body = String::new();
        for i in 0..6 {
            writeln!(
                body,
                "        s{i} = softwareSystem \"System {i}\" \"Stands alone.\""
            )
            .expect("write to String");
        }
        let m = model(&workspace(
            &body,
            "        systemContext s0 \"L\" {\n            include s0 s1 s2 s3 s4 s5\n        }",
        ));
        let p = plan_of(&m, "L");
        assert!(p.edges.is_empty());
        let c = place(&p, &Keep::default());
        let rows: std::collections::BTreeSet<i32> = c.iter().map(|q| q.y).collect();
        let cols: std::collections::BTreeSet<i32> = c.iter().map(|q| q.x).collect();
        assert!(
            rows.len() >= 2 && cols.len() >= 2,
            "rows {rows:?} cols {cols:?}"
        );
        assert!(
            rows.len() <= 3 && cols.len() <= 3,
            "rows {rows:?} cols {cols:?}"
        );
        assert_l6_clear(&p, &c, &p.unplaced());
    }

    #[test]
    fn single_neighbour_lands_next_to_it() {
        // A box whose only neighbour is placed lands in a slot beside it.
        let (m, p) = small(
            "            at user 400 200\n            at api 400 500\n            at db 400 800\n            at worker 800 800\n",
        );
        let c = place(&p, &Keep::default());
        let (api, ext) = (at(&c, box_of(&m, &p, "api")), at(&c, box_of(&m, &p, "ext")));
        let (dx, dy) = ((ext.x - api.x).abs(), (ext.y - api.y).abs());
        assert!(
            dx <= COL && dy <= ROW && (dx, dy) != (0, 0),
            "ext {ext:?} api {api:?}"
        );
        assert_l6_clear(&p, &c, &[box_of(&m, &p, "ext")]);
    }
}
