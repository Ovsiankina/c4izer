//! Lint resolve (OR-17): for a finding of a built scene, the box moves
//! and resizes that remove it, as plain geometry; the editor writes them
//! through its move and resize path. L6 (too close): move the two boxes
//! apart; L5 (off grid): snap the written `at` / `size`; L25 (text does
//! not fit a box): grow the box to the smallest grid size its text fits.
//! Every other rule has no resolution. Serves P5-400..P5-405.
//!
//! Depends on: `lint` (findings, rules), `scene` (box rects and centres,
//! `texts_fit`), `geom` (rects, the grid), `model` (written `at` / `size`
//! lines, view specs), `view` (box ids, plans), `pipeline` (EDIT-15
//! default view specs).

// Shape of the API: `resolve` answers one finding, `resolve_all` every
// finding of a report at once (one undo step in the editor). Both read
// a scene from `scene::build` (as `lint` does), never write text, and
// return centres in grid units and sizes in px, the units
// `State::write_moves` and `Action::Resize` take.

use crate::geom::{GRID_PX, Point, Rect};
use crate::lint::{Finding, Rule};
use crate::model::{Model, ViewSpec};
use crate::scene::{ItemKey, Scene, texts_fit};
use crate::view::{BoxId, PlanBox, ViewPlan, plan_in};

/// L6: each box keeps this many px clear on every side, so two boxes are
/// apart when their edges are `2 * SPACING_PX` apart (lint's
/// `SPACING_PX`).
const SPACING_PX: i32 = 30;

/// `resolve_all`: the most relaxation passes over the boxes.
const MAX_PASSES: usize = 32;

/// L25: the largest side the search grows a box to, px, unless the box
/// is larger already (as `State::min_size` bounds its search).
const MAX_SIDE_PX: i32 = 1200;

/// What a resolve does: box centres to write (grid units, as
/// `Scene::centres`) and box sizes to write (px `(width, height)`,
/// multiples of 20), each list in `BoxId` order, one box at most once.
/// Serves P5-400..P5-405.
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-17, #7).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Resolution {
    /// What it does, for the button's tip: "Move apart", "Snap to grid",
    /// "Fit text", "Resolve all".
    pub label: String,
    /// New centres, grid units.
    pub moves: Vec<(BoxId, Point)>,
    /// New sizes, px `(width, height)`.
    pub sizes: Vec<(BoxId, (i32, i32))>,
}

/// The resolution of `finding`, a finding of `lint(model, scene)`:
/// L6 moves the two boxes apart until their edges are 60px apart, on
/// the grid (P5-400, P5-401); L5 snaps the written `at` or `size` the
/// finding points at (P5-402); L25 on a box grows it to the smallest
/// grid size its text fits (P5-403). `None` for every other rule, for
/// an L25 finding on a label or boundary, and for an L25 box no size up
/// to 1200px fits (P5-404). Serves P5-400..P5-404.
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-17, #7).
#[must_use]
pub fn resolve(model: &Model, scene: &Scene, finding: &Finding) -> Option<Resolution> {
    match finding.rule {
        Rule::TooClose => {
            let (a, b) = pair(finding)?;
            move_apart(scene, a, b)
        }
        Rule::OffGrid => {
            let id = first_box(finding)?;
            let b = scene.boxes.get(id.0 as usize)?;
            let spec = written_view(model, scene)?;
            let span = finding.spans.first()?;
            // The finding's first span is the `at` or the `size` line.
            if spec.position_of(b.element).is_some_and(|at| at.span == *span) {
                let centre = *scene.centres.get(id.0 as usize)?;
                return Some(Resolution {
                    label: "Snap to grid".to_owned(),
                    moves: vec![(id, centre)],
                    sizes: Vec::new(),
                });
            }
            if spec.size_of(b.element).is_some_and(|s| s.span == *span) {
                return Some(Resolution {
                    label: "Snap to grid".to_owned(),
                    moves: Vec::new(),
                    sizes: vec![(id, (b.rect.width(), b.rect.height()))],
                });
            }
            None
        }
        Rule::TextFit => {
            let id = first_box(finding)?;
            let plan = plan_of(model, scene)?;
            let b = scene.boxes.get(id.0 as usize)?;
            let size = fit_size(plan.boxes.get(id.0 as usize)?, (b.rect.width(), b.rect.height()))?;
            Some(Resolution {
                label: "Fit text".to_owned(),
                moves: Vec::new(),
                sizes: vec![(id, size)],
            })
        }
        _ => None,
    }
}

/// One resolution for every resolvable finding of `findings` (a report
/// of `lint(model, scene)`): the L5 snaps first, then the L25 boxes grown
/// to fit their text, then the boxes moved apart by relaxation over a
/// working copy of every box rect (at most 32 passes, stopping when no
/// two boxes are closer than 60px). Label "Resolve all". `None` when no
/// finding has a resolution. Serves P5-405.
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-17, #7).
// DECISION: L25 grows the working rects before the L6 relaxation, not
// after it as the design lists: a grown box can come closer than 60px to
// a neighbour, and relaxing last makes the result free of L6 as well.
#[must_use]
pub fn resolve_all(model: &Model, scene: &Scene, findings: &[Finding]) -> Option<Resolution> {
    let n = scene.boxes.len();
    let mut centres: Vec<Point> = scene.centres.clone();
    let mut rects: Vec<Rect> = scene.boxes.iter().map(|b| b.rect).collect();
    let mut sizes: Vec<Option<(i32, i32)>> = vec![None; n];
    let mut snapped: Vec<bool> = vec![false; n];
    let mut any = false;
    let mut plan: Option<ViewPlan> = None;
    for f in findings {
        match f.rule {
            Rule::OffGrid => {
                if let Some(r) = resolve(model, scene, f) {
                    any = true;
                    for (id, _) in &r.moves {
                        snapped[id.0 as usize] = true;
                    }
                    for (id, s) in r.sizes {
                        sizes[id.0 as usize] = Some(s);
                    }
                }
            }
            Rule::TooClose => any |= pair(f).is_some(),
            _ => {}
        }
    }
    for f in findings.iter().filter(|f| f.rule == Rule::TextFit) {
        let Some(id) = first_box(f) else { continue };
        if plan.is_none() {
            plan = plan_of(model, scene);
        }
        let Some(pb) = plan.as_ref().and_then(|p| p.boxes.get(id.0 as usize)) else { continue };
        let r = rects[id.0 as usize];
        if let Some(size) = fit_size(pb, (r.width(), r.height())) {
            any = true;
            sizes[id.0 as usize] = Some(size);
        }
    }
    if !any {
        return None;
    }
    // Rects at their snapped or grown size, around their centre.
    for (i, s) in sizes.iter().enumerate() {
        if let Some((w, h)) = *s {
            rects[i] = Rect::from_centre(rects[i].centre(), w / 2, h / 2);
        }
    }
    relax(&mut centres, &mut rects);
    let moves = (0..n)
        .filter(|&i| snapped[i] || centres[i] != scene.centres[i])
        .map(|i| (box_id(i), centres[i]))
        .collect();
    let sizes = sizes
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.map(|s| (box_id(i), s)))
        .collect();
    Some(Resolution {
        label: "Resolve all".to_owned(),
        moves,
        sizes,
    })
}

/// `BoxId` of index `i`; a scene holds far fewer than `u32::MAX` boxes.
fn box_id(i: usize) -> BoxId {
    BoxId(u32::try_from(i).expect("box index fits in u32"))
}

/// The first box an L5 or L25 finding marks.
fn first_box(finding: &Finding) -> Option<BoxId> {
    match finding.items.first()? {
        ItemKey::Box(id) => Some(*id),
        _ => None,
    }
}

/// The two boxes of an L6 finding, in its order.
fn pair(finding: &Finding) -> Option<(BoxId, BoxId)> {
    match finding.items.as_slice() {
        [ItemKey::Box(a), ItemKey::Box(b)] => Some((*a, *b)),
        _ => None,
    }
}

/// The view whose `at` and `size` lines the scene's boxes take: the
/// declared view, or its base for a filtered view (as lint L5 reads).
fn written_view<'m>(model: &'m Model, scene: &Scene) -> Option<&'m ViewSpec> {
    model.view(&scene.key).map(|v| model.base_view(v).unwrap_or(v))
}

/// The plan of the scene's view: declared, or the EDIT-15 default view
/// of its scope and kind. Its boxes are the scene's, in `BoxId` order.
fn plan_of(model: &Model, scene: &Scene) -> Option<ViewPlan> {
    if let Some(v) = model.view(&scene.key) {
        return Some(plan_in(model, v, scene.mode));
    }
    let scope = scene.scope.element()?;
    let spec = crate::pipeline::default_view(model, scope, scene.kind)?;
    Some(plan_in(model, &spec, scene.mode))
}

/// True when `a` and `b`, each grown by 30px, overlap (L6).
fn too_close(a: &Rect, b: &Rect) -> bool {
    a.inflate(SPACING_PX).overlaps(&b.inflate(SPACING_PX))
}

/// The push that moves `b` clear of `a` along x and along y, in whole
/// grid units, signed in the direction `b` goes (away from `a`; when the
/// centres are level, `b` goes +). Each is the smallest that leaves 60px
/// between the two edges.
// DECISION: level centres push `b` towards +x / +y (right, down): a
// fixed rule keeps the result deterministic (CORE-25).
fn pushes(a: &Rect, b: &Rect) -> (i32, i32) {
    let axis = |a_min: i32, a_max: i32, b_min: i32, b_max: i32| -> i32 {
        let (ac, bc) = (a_min + a_max, b_min + b_max);
        let px = if bc >= ac {
            2 * SPACING_PX - (b_min - a_max)
        } else {
            2 * SPACING_PX - (a_min - b_max)
        };
        let units = px.max(0).div_euclid(GRID_PX) + i32::from(px.max(0).rem_euclid(GRID_PX) != 0);
        if bc >= ac { units } else { -units }
    };
    (
        axis(a.min.x, a.max.x, b.min.x, b.max.x),
        axis(a.min.y, a.max.y, b.min.y, b.max.y),
    )
}

/// A rect moved by `(dx, dy)` grid units.
fn shifted(r: &Rect, dx: i32, dy: i32) -> Rect {
    let (dx, dy) = (dx * GRID_PX, dy * GRID_PX);
    Rect {
        min: Point::new(r.min.x + dx, r.min.y + dy),
        max: Point::new(r.max.x + dx, r.max.y + dy),
    }
}

/// L6 for one pair: the first candidate that brings no other box closer
/// than 60px to a moved box than it was, else the first (P5-401).
// DECISION: "the axis needing the smaller push" compares the pushes in
// whole grid units; a tie takes x. "Only the second box" and "only the
// first" go along that same axis, the first in the opposite direction.
fn move_apart(scene: &Scene, a: BoxId, b: BoxId) -> Option<Resolution> {
    let (ia, ib) = (a.0 as usize, b.0 as usize);
    let (ra, rb) = (scene.boxes.get(ia)?.rect, scene.boxes.get(ib)?.rect);
    let (ca, cb) = (*scene.centres.get(ia)?, *scene.centres.get(ib)?);
    let (px, py) = pushes(&ra, &rb);
    let x_first = px.abs() <= py.abs();
    // Split a push in two: `a` takes the smaller half backwards, `b` the
    // rest forwards.
    let half = |p: i32| -> (i32, i32) { (-(p / 2), p - p / 2) };
    let along = |x: bool, p: (i32, i32)| -> ((i32, i32), (i32, i32)) {
        let (da, db) = half(if x { p.0 } else { p.1 });
        if x { ((da, 0), (db, 0)) } else { ((0, da), (0, db)) }
    };
    let (small, other) = (x_first, !x_first);
    let full = if small { (px, 0) } else { (0, py) };
    let candidates: [((i32, i32), (i32, i32)); 4] = [
        along(small, (px, py)),
        along(other, (px, py)),
        ((0, 0), full),
        ((-full.0, -full.1), (0, 0)),
    ];
    let clean = |(da, db): &((i32, i32), (i32, i32))| -> bool {
        let moved = [(ia, ra, shifted(&ra, da.0, da.1)), (ib, rb, shifted(&rb, db.0, db.1))];
        scene.boxes.iter().enumerate().all(|(i, o)| {
            i == ia
                || i == ib
                || moved
                    .iter()
                    .all(|(_, old, new)| !too_close(new, &o.rect) || too_close(old, &o.rect))
        })
    };
    let (da, db) = candidates.iter().find(|c| clean(c)).copied().unwrap_or(candidates[0]);
    let mut moves = Vec::new();
    if da != (0, 0) {
        moves.push((a, Point::new(ca.x + da.0, ca.y + da.1)));
    }
    if db != (0, 0) {
        moves.push((b, Point::new(cb.x + db.0, cb.y + db.1)));
    }
    moves.sort_by_key(|(id, _)| *id);
    Some(Resolution {
        label: "Move apart".to_owned(),
        moves,
        sizes: Vec::new(),
    })
}

/// Relaxation over every box (P5-405): each pass visits the boxes in
/// centre order (x, then y) and moves the later box of each too-close
/// pair clear of the earlier one along the axis needing the smaller
/// push; stops after a pass that moved nothing, or after `MAX_PASSES`.
// DECISION: the later box of a pair takes the whole push (not half
// each, as a single resolve does): a crowded row then clears in one
// pass instead of one pass per box, so 32 passes suffice for hundreds of
// boxes.
fn relax(centres: &mut [Point], rects: &mut [Rect]) {
    let n = rects.len();
    let mut order: Vec<usize> = (0..n).collect();
    for _ in 0..MAX_PASSES {
        order.sort_by_key(|&i| (centres[i].x, centres[i].y, i));
        let mut moved = false;
        for oi in 0..n {
            for oj in oi + 1..n {
                let (i, j) = (order[oi], order[oj]);
                if !too_close(&rects[i], &rects[j]) {
                    continue;
                }
                let (px, py) = pushes(&rects[i], &rects[j]);
                let (dx, dy) = if px.abs() <= py.abs() { (px, 0) } else { (0, py) };
                rects[j] = shifted(&rects[j], dx, dy);
                centres[j] = Point::new(centres[j].x + dx, centres[j].y + dy);
                moved = true;
            }
        }
        if !moved {
            return;
        }
    }
}

/// The smallest size in 20px steps, at least `current` on each side and
/// at most `MAX_SIDE_PX` (or `current`, when larger), at which `pb`'s
/// texts fit (`scene::texts_fit`); `None` when none does or `current`
/// already fits. Smallest area wins, ties to the narrower (as the
/// editor's `min_size`).
// DECISION: a staircase walk (the fitting height never grows with the
// width, `texts_fit` being monotone), stopped once a width at the
// current height cannot beat the best area: tens of fits, not thousands.
fn fit_size(pb: &PlanBox, current: (i32, i32)) -> Option<(i32, i32)> {
    let fits = |w: i32, h: i32| texts_fit(pb, w / 2 / GRID_PX, h / 2 / GRID_PX);
    let step = 2 * GRID_PX;
    let up = |v: i32| (v.max(step) + step - 1) / step * step;
    let (w0, h0) = (up(current.0), up(current.1));
    if fits(w0, h0) {
        return None;
    }
    let (w_max, h_max) = (w0.max(MAX_SIDE_PX), h0.max(MAX_SIDE_PX));
    let area = |w: i32, h: i32| i64::from(w) * i64::from(h);
    let mut best: Option<(i32, i32)> = None;
    let mut h = h_max;
    let mut w = w0;
    while w <= w_max {
        if best.is_some_and(|(bw, bh)| area(w, h0) >= area(bw, bh)) {
            break;
        }
        if fits(w, h) {
            while h - step >= h0 && fits(w, h - step) {
                h -= step;
            }
            if best.is_none_or(|(bw, bh)| area(w, h) < area(bw, bh)) {
                best = Some((w, h));
            }
        }
        w += step;
    }
    best
}
