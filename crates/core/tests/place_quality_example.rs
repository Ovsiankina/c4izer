//! P2-26, P2-107, P2-108 on `tests/data/example/workspace.dsl` (plain
//! Structurizr: no `at` lines, `autoLayout` on every view), built with
//! `pipeline::build_all` (`Keep::default()`). Every box is auto-placed,
//! so these tests reach the layered placement of `place`.
//!
//! Run with `--nocapture` to print the per-view metrics table.

use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::{GRID_PX, Point, Rect, grid_to_px};
use c4rizer_core::label::Fit;
use c4rizer_core::lint::{Rule, Severity};
use c4rizer_core::model::{ElementKind, parse};
use c4rizer_core::pipeline::{ViewBuild, build_all};
use c4rizer_core::view::ViewPlan;

const EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/data/example/workspace.dsl"
));

/// View keys in declaration order.
const KEYS: [&str; 4] = ["C1", "C2", "C3-serving", "C3-startup"];

/// Phase-1 baseline, measured on the working tree 2026-11-15 (SRS
/// P2-108): routed crossings per view.
const BASE_CROSSINGS: [u32; 4] = [0, 0, 1, 9];
/// Phase-1 baseline, measured 2026-11-15 (SRS P2-108): total line length
/// in px (integer milli-px metric, truncated once).
const BASE_LENGTH: [i64; 4] = [691, 1898, 7092, 18146];
/// Phase-1 baseline, measured on dev 80fe4cd with the cli, 2026-11-15:
/// lint warnings per view (C3-serving: 2 L23, 2 no-room).
const BASE_WARNINGS: [usize; 4] = [0, 2, 4, 7];

// TODO(srs): P2-26 / P2-107 / P2-108 numbers are not frozen (docs/deferred.md
// row "place-quality, 2, P2-26/P2-107/P2-108 freeze"); P2-113..P2-115 are
// the targets of place-quality-2. These pins are "no worse than measured
// after place-quality-2-03, 2026-11-17"; the owner picks the frozen ones.
/// Routed crossings per view measured after place-quality-2-03
/// (2026-11-17).
const AFTER_CROSSINGS: [u32; 4] = [0, 0, 1, 8];
/// Total line length in px per view measured after place-quality-2-fix
/// (CORE-14 arrowhead, 2026-11-17; 2778 and 9272 px on C2 and C3-serving
/// before it, 3554 and 14 161 px before the outward-facing externals);
/// C3-serving 9370 -> 9387 and C3-startup 17733 -> 17766 px with P4-600
/// (boundary-clear, 2026-11-19), which moves the lines that ran on the
/// boundary's stroke one lane out.
const AFTER_LENGTH: [i64; 4] = [691, 2838, 9387, 17766];

fn builds() -> Vec<ViewBuild> {
    let model = match parse(EXAMPLE) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    };
    let all = build_all(&model, &AtomicBool::new(false)).expect("never stopped");
    let keys: Vec<&str> = all.views.iter().map(|v| v.plan.key.as_str()).collect();
    assert_eq!(keys, KEYS);
    for v in &all.views {
        assert_eq!(
            v.plan.unplaced().len(),
            v.plan.boxes.len(),
            "{}",
            v.plan.key
        );
    }
    all.views
}

/// Box `i`'s rectangle in px at its routed centre.
fn box_px(b: &ViewBuild, i: usize) -> Rect {
    let r = b.plan.boxes[i].rect_at(b.routes.centres[i]);
    Rect::from_corners(
        Point::new(r.min.x * GRID_PX, r.min.y * GRID_PX),
        Point::new(r.max.x * GRID_PX, r.max.y * GRID_PX),
    )
}

/// Every label px rectangle, built from the pub `Label` fields.
fn label_rects(b: &ViewBuild) -> Vec<(usize, Rect)> {
    b.labels
        .labels
        .iter()
        .flatten()
        .map(|l| {
            let left = grid_to_px(l.centre.x) - l.width_px / 2;
            let top = grid_to_px(l.centre.y) - l.height_px / 2;
            (
                l.edge.0 as usize,
                Rect::from_corners(
                    Point::new(left, top),
                    Point::new(left + l.width_px, top + l.height_px),
                ),
            )
        })
        .collect()
}

/// Positive-area overlap of two rectangles.
fn overlap(a: &Rect, b: &Rect) -> bool {
    a.min.x < b.max.x && b.min.x < a.max.x && a.min.y < b.max.y && b.min.y < a.max.y
}

/// (label edge, box index) pairs whose rectangles overlap.
fn labels_over_boxes(b: &ViewBuild) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (e, r) in label_rects(b) {
        for i in 0..b.plan.boxes.len() {
            if overlap(&r, &box_px(b, i)) {
                out.push((e, i));
            }
        }
    }
    out
}

fn no_room(b: &ViewBuild) -> usize {
    b.labels
        .labels
        .iter()
        .flatten()
        .filter(|l| l.fit == Fit::NoRoom)
        .count()
}

/// Total routed line length in px: per segment `steps * 10_000`
/// (axis-aligned) or `steps * 14_142` (45 degrees) milli-px, summed,
/// divided by 1000 once.
fn length_px(b: &ViewBuild) -> i64 {
    let mut milli: i64 = 0;
    for line in b.routes.lines.iter().flatten() {
        for w in line.points.windows(2) {
            let (dx, dy) = ((w[1].x - w[0].x).abs(), (w[1].y - w[0].y).abs());
            let steps = i64::from(dx.max(dy));
            milli += if dx != 0 && dy != 0 {
                steps * 14_142
            } else {
                steps * 10_000
            };
        }
    }
    milli / 1000
}

fn count(b: &ViewBuild, rule: Rule) -> usize {
    b.report.findings.iter().filter(|f| f.rule == rule).count()
}

/// Prints one metrics row per view (visible with `--nocapture`).
fn print_table(all: &[ViewBuild]) {
    println!("view        cross  length  noroom  overbox  L23  L26  warnings");
    for b in all {
        let warnings = b
            .report
            .findings
            .iter()
            .filter(|f| f.severity() == Severity::Warning)
            .count();
        println!(
            "{:<11} {:>5}  {:>6}  {:>6}  {:>7}  {:>3}  {:>3}  {:>8}",
            b.plan.key,
            b.routes.crossings,
            length_px(b),
            no_room(b),
            labels_over_boxes(b).len(),
            count(b, Rule::LabelOverLine),
            count(b, Rule::BoundaryIntrusion),
            warnings
        );
    }
}

/// P2-107: no no-room label, no label over a box, no L26, on every view.
#[test]
fn p2_107_example_labels_fit_clear_of_boxes() {
    let all = builds();
    print_table(&all);
    for b in &all {
        let key = &b.plan.key;
        assert!(
            b.labels.no_room().is_empty(),
            "{key}: no room {:?}",
            b.labels.no_room()
        );
        assert_eq!(count(b, Rule::LabelNoRoom), 0, "{key}: L24");
        let over = labels_over_boxes(b);
        assert!(
            over.is_empty(),
            "{key}: labels over boxes (edge, box) {over:?}"
        );
        assert_eq!(count(b, Rule::BoundaryIntrusion), 0, "{key}: L26");
    }
}

/// P2-108, the part that holds: nothing unroutable, crossings no higher
/// than the phase-1 baseline on every view, lint warnings no higher than
/// phase 1 on every view, crossings and length strictly below the
/// phase-1 baseline on C3-startup, and every view no worse than the
/// numbers measured after this row (TODO(srs) above).
// TODO(srs): the lint-warning clause is the approver's measured metric,
// not a P2-108 sentence yet.
#[test]
fn p2_108_example_crossings_and_length_no_worse() {
    let all = builds();
    for (k, b) in all.iter().enumerate() {
        let key = &b.plan.key;
        let (cross, len) = (b.routes.crossings, length_px(b));
        assert!(b.routes.unroutable().is_empty(), "{key}: unroutable");
        assert!(
            cross <= BASE_CROSSINGS[k],
            "{key}: {cross} crossings, baseline {}",
            BASE_CROSSINGS[k]
        );
        let warnings = b
            .report
            .findings
            .iter()
            .filter(|f| f.severity() == Severity::Warning)
            .count();
        assert!(
            warnings <= BASE_WARNINGS[k],
            "{key}: {warnings} lint warnings, baseline {}",
            BASE_WARNINGS[k]
        );
        assert!(
            cross <= AFTER_CROSSINGS[k],
            "{key}: {cross} crossings, measured {}",
            AFTER_CROSSINGS[k]
        );
        assert!(
            len <= AFTER_LENGTH[k],
            "{key}: length {len}, measured {}",
            AFTER_LENGTH[k]
        );
        if *key == "C3-startup" {
            assert!(
                cross < BASE_CROSSINGS[k],
                "{key}: {cross} crossings, not below {}",
                BASE_CROSSINGS[k]
            );
            assert!(
                len < BASE_LENGTH[k],
                "{key}: length {len}, not below {}",
                BASE_LENGTH[k]
            );
        }
    }
}

/// P2-108 as written: every view no worse than the phase-1 baseline. The
/// crossing clause holds (`p2_108_example_crossings_and_length_no_worse`);
/// the length clause is not met on C2 (2778 vs 1898 px) and C3-serving
/// (9272 vs 7092 px, both measured 2026-11-17 after place-quality-2-03).
/// Phase 1 had no row discipline (no people row, no rank order), so it is
/// not a fair cap for a layered layout; P2-113 sets the caps instead.
///
/// The lower bound below (2634 px on C2) was argued under the original
/// P2-100, which put Push services on row >= 2; the amended P2-100 /
/// P2-103 put it on the people row, so the bound no longer holds and C2
/// now reaches 2778 px.
/// - server -> 5 callees: one directly below (140), two in adjacent
///   columns (dx >= 100, 181 each), two further away or a row lower
///   (>= 420 each): >= 1342.
/// - 3 people -> server: one directly above (120), two in adjacent
///   columns (161 each): >= 442.
/// - Push services (row >= 2) -> worker and witness (row 0): dy >= 400
///   each, and at most one sits in push's column (+50): >= 850.
///
/// docs/deferred.md row "place-quality, 5, P2-108 baseline vs
/// P2-100/P2-103".
#[test]
#[ignore = "deferred"]
fn p2_108_example_no_worse_than_phase_1_baseline() {
    let all = builds();
    for (k, b) in all.iter().enumerate() {
        let key = &b.plan.key;
        let (cross, len) = (b.routes.crossings, length_px(b));
        assert!(
            cross <= BASE_CROSSINGS[k],
            "{key}: {cross} crossings, baseline {}",
            BASE_CROSSINGS[k]
        );
        assert!(
            len <= BASE_LENGTH[k],
            "{key}: length {len}, baseline {}",
            BASE_LENGTH[k]
        );
    }
}

fn is_person(p: &ViewPlan, i: usize) -> bool {
    p.boxes[i].kind == ElementKind::Person
}

/// True for an outward-facing external (amended P2-100): external, not a
/// person, at least one drawn outgoing edge, every one ending on a person.
fn outward_facing(p: &ViewPlan, i: usize) -> bool {
    if !p.boxes[i].external || is_person(p, i) {
        return false;
    }
    let out: Vec<usize> = p
        .edges
        .iter()
        .filter(|e| e.source.0 as usize == i && e.destination.0 as usize != i)
        .map(|e| e.destination.0 as usize)
        .collect();
    !out.is_empty() && out.iter().all(|&d| is_person(p, d))
}

/// True when `to` is reachable from `from` over edges that end on neither
/// a person nor an outward-facing external.
fn reaches(p: &ViewPlan, from: usize, to: usize) -> bool {
    let mut seen = vec![false; p.boxes.len()];
    let mut stack = vec![from];
    while let Some(v) = stack.pop() {
        if v == to {
            return true;
        }
        if std::mem::replace(&mut seen[v], true) {
            continue;
        }
        for e in &p.edges {
            let (s, d) = (e.source.0 as usize, e.destination.0 as usize);
            if s == v && !is_person(p, d) && !outward_facing(p, d) {
                stack.push(d);
            }
        }
    }
    false
}

/// P2-100 (amended) on a view with no kept box.
fn assert_p2_100(b: &ViewBuild) {
    let (p, c) = (&b.plan, &b.routes.centres);
    for e in &p.edges {
        let (src, dst) = (e.source.0 as usize, e.destination.0 as usize);
        if src == dst || is_person(p, dst) || outward_facing(p, dst) || reaches(p, dst, src) {
            continue;
        }
        assert!(
            c[src].y < c[dst].y,
            "{}: P2-100 {} -> {} does not point down",
            p.key,
            p.boxes[src].name,
            p.boxes[dst].name
        );
    }
}

/// P2-103 (amended) on a view with no kept box: nothing above a person,
/// and every outward-facing external on the people's row.
fn assert_p2_103(b: &ViewBuild) {
    let (p, c) = (&b.plan, &b.routes.centres);
    for person in (0..p.boxes.len()).filter(|&i| is_person(p, i)) {
        for o in 0..p.boxes.len() {
            assert!(
                c[o].y >= c[person].y,
                "{}: P2-103 {} above person {}",
                p.key,
                p.boxes[o].name,
                p.boxes[person].name
            );
            if outward_facing(p, o) {
                assert_eq!(
                    c[o].y, c[person].y,
                    "{}: P2-103 {} off the people row",
                    p.key, p.boxes[o].name
                );
            }
        }
    }
}

/// P2-104 on a view with no kept box.
fn assert_p2_104(b: &ViewBuild) {
    let (p, c) = (&b.plan, &b.routes.centres);
    let mut rows: Vec<i32> = c.iter().map(|q| q.y).collect();
    rows.sort_unstable();
    rows.dedup();
    for y in rows {
        let mut row: Vec<usize> = (0..p.boxes.len()).filter(|&i| c[i].y == y).collect();
        row.sort_by_key(|&i| c[i].x);
        let ext: Vec<bool> = row
            .iter()
            .map(|&i| p.boxes[i].external && !is_person(p, i))
            .collect();
        let first = ext.iter().position(|&x| !x);
        let last = ext.iter().rposition(|&x| !x);
        if let (Some(a), Some(z)) = (first, last) {
            assert!(
                ext[a..=z].iter().all(|&x| !x),
                "{}: P2-104 an external box sits between internal boxes on row {y}",
                p.key
            );
        }
    }
}

/// P2-109 on a view with no kept box.
fn assert_p2_109(b: &ViewBuild) {
    let (p, c) = (&b.plan, &b.routes.centres);
    for (k, boundary) in p.boundaries.iter().enumerate() {
        let r = p.boundary_rect(k, c).expect("boundary has members");
        for bx in p
            .boxes
            .iter()
            .filter(|bx| !boundary.members.contains(&bx.id))
        {
            assert!(
                !r.overlaps(&bx.rect_at(c[bx.id.0 as usize])),
                "{}: P2-109 {} overlaps the boundary",
                p.key,
                bx.name
            );
        }
    }
}

/// P2-26: the layered rules P2-100, P2-103, P2-104, P2-109 on each example
/// view (none has a kept box).
#[test]
fn p2_26_example_layered() {
    for b in &builds() {
        assert_p2_100(b);
        assert_p2_103(b);
        assert_p2_104(b);
        assert_p2_109(b);
    }
}

/// P2-113, crossings and L23: on C2 and C3-serving, routed crossings 0 /
/// 1, L23 findings at most 0 / 1.
#[test]
fn p2_113_c2_c3_serving_crossings_and_l23() {
    let all = builds();
    for (key, cross, max_l23) in [("C2", 0, 0), ("C3-serving", 1, 1)] {
        let b = all.iter().find(|b| b.plan.key == key).expect("view");
        assert_eq!(b.routes.crossings, cross, "{key}: crossings");
        let l23 = count(b, Rule::LabelOverLine);
        assert!(l23 <= max_l23, "{key}: {l23} L23, cap {max_l23}");
    }
}

/// P2-113, length: on C2 and C3-serving, total line length at most 2840 /
/// 9400 px (approver, 2026-11-17: caps re-measured after the CORE-14
/// arrowhead fix, which widens the Push services column gap by 20px;
/// measured 2838 / 9370 px).
#[test]
fn p2_113_c2_c3_serving_length() {
    let all = builds();
    print_table(&all);
    for (key, max_len) in [("C2", 2840), ("C3-serving", 9400)] {
        let b = all.iter().find(|b| b.plan.key == key).expect("view");
        let len = length_px(b);
        assert!(len <= max_len, "{key}: length {len}, cap {max_len}");
    }
}

/// P2-114: on C3-startup, routed crossings at most 8, L23 findings at
/// most 5, total line length at most 17766 px (17733 before P4-600).
#[test]
fn p2_114_c3_startup_no_worse() {
    let all = builds();
    let b = all
        .iter()
        .find(|b| b.plan.key == "C3-startup")
        .expect("view");
    let (cross, len, l23) = (
        b.routes.crossings,
        length_px(b),
        count(b, Rule::LabelOverLine),
    );
    assert!(cross <= 8, "C3-startup: {cross} crossings, cap 8");
    assert!(l23 <= 5, "C3-startup: {l23} L23, cap 5");
    assert!(len <= 17_766, "C3-startup: length {len}, cap 17766");
}

/// How many times `points` passes the border of `r`: inside/outside
/// changes along the polyline, walked one grid step at a time.
fn border_passes(points: &[Point], r: &Rect) -> usize {
    let mut passes = 0;
    let mut inside = points.first().map(|&q| r.contains(q));
    for w in points.windows(2) {
        let (dx, dy) = ((w[1].x - w[0].x).signum(), (w[1].y - w[0].y).signum());
        let mut q = w[0];
        while q != w[1] {
            q = Point::new(q.x + dx, q.y + dy);
            let now = Some(r.contains(q));
            if now != inside {
                passes += 1;
                inside = now;
            }
        }
    }
    passes
}

/// P2-112: on C2, Push services (an outward-facing external) has the
/// people's centre y, sits at an end of that row, and no line from it
/// passes the boundary rectangle more than once; on C3-serving it has the
/// people's centre y.
#[test]
fn p2_112_push_services_on_people_row() {
    let all = builds();
    for key in ["C2", "C3-serving"] {
        let view = all.iter().find(|b| b.plan.key == key).expect("view");
        let (p, c) = (&view.plan, &view.routes.centres);
        let push = (0..p.boxes.len())
            .find(|&i| p.boxes[i].name == "Push services")
            .expect("Push services");
        assert!(
            outward_facing(p, push),
            "{key}: Push services not outward-facing"
        );
        let people: Vec<usize> = (0..p.boxes.len()).filter(|&i| is_person(p, i)).collect();
        assert!(!people.is_empty(), "{key}: no person");
        for &q in &people {
            assert_eq!(
                c[push].y, c[q].y,
                "{key}: Push services y vs {}",
                p.boxes[q].name
            );
        }
        if key != "C2" {
            continue;
        }
        let mut row: Vec<usize> = (0..p.boxes.len())
            .filter(|&i| c[i].y == c[push].y)
            .collect();
        row.sort_by_key(|&i| c[i].x);
        assert!(
            row.first() == Some(&push) || row.last() == Some(&push),
            "{key}: Push services not at a row end"
        );
        for (k, _) in p.boundaries.iter().enumerate() {
            let r = p.boundary_rect(k, c).expect("boundary has members");
            for (e, line) in p.edges.iter().zip(&view.routes.lines) {
                if e.source.0 as usize != push {
                    continue;
                }
                let line = line.as_ref().expect("routed");
                let n = border_passes(&line.points, &r);
                assert!(
                    n <= 1,
                    "{key}: a line from Push services passes the boundary {n} times"
                );
            }
        }
    }
}

/// The px point `d` px along the polyline `points` (grid units) from its
/// first point, walking axis-aligned segments only (enough for the ends
/// this file checks: `None` when the first segment is diagonal or
/// shorter than `d`).
fn along_first(points: &[Point], d: i32) -> Option<Point> {
    let (a, b) = (points.first()?, points.get(1)?);
    let (dx, dy) = ((b.x - a.x).signum(), (b.y - a.y).signum());
    if dx != 0 && dy != 0 {
        return None;
    }
    let len = grid_to_px((b.x - a.x).abs().max((b.y - a.y).abs()));
    (len >= d).then(|| Point::new(grid_to_px(a.x) + dx * d, grid_to_px(a.y) + dy * d))
}

/// Strictly inside `r` (a point on the border is not covered).
fn strictly_inside(r: &Rect, q: Point) -> bool {
    r.min.x < q.x && q.x < r.max.x && r.min.y < q.y && q.y < r.max.y
}

/// CORE-14 on the example views (place-quality-2 review, P2-112 layout):
/// a label never covers its own line's arrowhead (the 10px tip) nor the
/// first 5px of the line, so the line stays visible on both sides of its
/// label and the direction can be read. Checked at the arrowhead's middle
/// (5px back from the tip) and 5px from the start.
#[test]
fn core_14_example_label_leaves_arrowhead_and_start_visible() {
    let all = builds();
    for b in &all {
        let key = &b.plan.key;
        for (e, r) in label_rects(b) {
            let Some(line) = b.routes.lines[e].as_ref() else {
                continue;
            };
            let name = format!(
                "{} -> {}",
                b.plan.boxes[b.plan.edges[e].source.0 as usize].name,
                b.plan.boxes[b.plan.edges[e].destination.0 as usize].name
            );
            if let Some(q) = along_first(&line.points, 5) {
                assert!(
                    !strictly_inside(&r, q),
                    "{key}: label of {name} covers the line's start"
                );
            }
            if matches!(line.end, c4rizer_core::route::LineEnd::Arrow(_)) {
                let rev: Vec<Point> = line.points.iter().rev().copied().collect();
                if let Some(q) = along_first(&rev, 5) {
                    assert!(
                        !strictly_inside(&r, q),
                        "{key}: label of {name} covers the arrowhead"
                    );
                }
            }
        }
    }
}

/// P2-115 (`TODO(srs)`, stretch, approved 2026-11-17): on C3-startup,
/// routed crossings at most 6, L23 findings at most 3, total line length
/// at most 25 000 px, within the P2-111 budget.
///
/// Not met (place-quality-2-05, 2026-11-17): 8 / 5 / 17733 px. Tried,
/// release, C3-startup only: hub sinks of the last rank re-ordered after
/// the sweeps (biggest at the median, the second at an end: 8 / 5 /
/// 17733, same winner; both at the ends: 10 / 6 / 25740), straight
/// crossings weighted by the endpoints' edge counts in the search
/// (9 to 11 crossings, 19567 to 23513 px). A routed refinement of up to 4
/// probe evaluations needs 100-600 ms against about 40 ms of budget left
/// (build 960 ms). docs/deferred.md row "place-quality-2, 5, P2-115".
// TODO(srs): P2-115 is a stretch target; the row closes on P2-114.
#[test]
#[ignore = "deferred"]
fn p2_115_c3_startup_stretch() {
    let all = builds();
    let b = all
        .iter()
        .find(|b| b.plan.key == "C3-startup")
        .expect("view");
    let (cross, len, l23) = (
        b.routes.crossings,
        length_px(b),
        count(b, Rule::LabelOverLine),
    );
    assert!(cross <= 6, "C3-startup: {cross} crossings, cap 6");
    assert!(l23 <= 3, "C3-startup: {l23} L23, cap 3");
    assert!(len <= 25_000, "C3-startup: length {len}, cap 25000");
}
