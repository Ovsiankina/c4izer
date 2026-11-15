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
// row "place-quality, 2, P2-26/P2-107/P2-108 freeze"). These pins are
// "no worse than measured after place-quality-07, 2026-11-15"; the owner
// picks whether they or the phase-1 baseline become the frozen targets.
/// Routed crossings per view measured after place-quality-07.
const AFTER_CROSSINGS: [u32; 4] = [0, 0, 1, 8];
/// Total line length in px per view measured after place-quality-07.
const AFTER_LENGTH: [i64; 4] = [691, 3554, 14161, 17733];

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
/// the length clause is not met on C2 (3554 vs 1898 px) and C3-serving
/// (14 161 vs 7092 px), and cannot be under P2-100, P2-103 and P2-106: the
/// phase-1 layouts put "Push services" on the people row and most callees
/// beside their callers, against P2-100. The best layered layouts a routed
/// hill climb found were 3523 px (C2) and 13 091 px (C3-serving).
///
/// Lower bound for C2 under the rules, border to border, 45-degree steps
/// allowed (a segment of grid deltas dx <= dy costs at least
/// `dy + 0.414 dx`). Boxes are 240x140, people 200x180; P2-106 gives
/// row pitch >= 280 and column pitch >= 340, one box per lattice cell.
/// P2-100/P2-103 put the people on row 0, the server on row >= 1 and all
/// five callees (three externals, two databases) on row >= 2.
/// - server -> 5 callees: one directly below (140), two in adjacent
///   columns (dx >= 100, 181 each), two further away or a row lower
///   (>= 420 each): >= 1342.
/// - 3 people -> server: one directly above (120), two in adjacent
///   columns (161 each): >= 442.
/// - Push services (row >= 2) -> worker and witness (row 0): dy >= 400
///   each, and at most one sits in push's column (+50): >= 850.
///
/// Total >= 2634 px, above the 1898 px baseline, so no placement that
/// keeps P2-100, P2-103 and P2-106 meets this test on C2.
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

/// True when `to` is reachable from `from` over edges that do not end on
/// a person.
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
            if s == v && !is_person(p, d) {
                stack.push(d);
            }
        }
    }
    false
}

/// P2-100 on a view with no kept box.
fn assert_p2_100(b: &ViewBuild) {
    let (p, c) = (&b.plan, &b.routes.centres);
    for e in &p.edges {
        let (src, dst) = (e.source.0 as usize, e.destination.0 as usize);
        if src == dst || is_person(p, dst) || reaches(p, dst, src) {
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

/// P2-103 on a view with no kept box.
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
