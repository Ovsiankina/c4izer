//! Lint resolve, core part (SRS 4.9 lint-resolve, P5-400..P5-405): the
//! property that matters is checked end to end: a resolution written as
//! `at` / `size` lines and the view rebuilt leaves the finding gone and,
//! for a clean move, no new L6. Benchmarks `perf_p5_*` (P5-900).

// DECISION: short names for the parse, scene and resolution under test
// (`p`, `s`, `r`, then `q`, `t` after the write) read best side by side.
#![allow(clippy::many_single_char_names)]

#[path = "bench_util/bench.rs"]
mod bench;

use std::fmt::Write as _;
use std::sync::atomic::AtomicBool;

use c4rizer_core::edit::{Edit, apply};
use c4rizer_core::geom::{GRID_PX, Point};
use c4rizer_core::lint::{Finding, Rule, lint};
use c4rizer_core::model::{Model, Parsed, parse};
use c4rizer_core::pipeline::build_view;
use c4rizer_core::place::Keep;
use c4rizer_core::resolve::{Resolution, resolve, resolve_all};
use c4rizer_core::scene::Scene;
use c4rizer_core::view::BoxId;

/// Both identifier modes: a box is named by its bare id, or by its full
/// id `s.<id>` under `!identifiers hierarchical`.
const MODES: [bool; 2] = [false, true];

/// A container view of system `s` with one container per entry of
/// `boxes` (`(id, x, y)`, `at` lines in that order) and `extra` view
/// lines; `title: false` leaves the view untitled (L8).
fn dsl(hier: bool, boxes: &[(&str, i32, i32)], extra: &str, title: bool) -> String {
    let mut s = String::from("workspace \"R\" {\n");
    if hier {
        s.push_str("    !identifiers hierarchical\n");
    }
    s.push_str("    model {\n        s = softwareSystem \"S\" \"Does things.\" {\n");
    for (id, ..) in boxes {
        let _ = writeln!(s, "            {id} = container \"Box {id}\" \"Does {id}.\" \"Rust\"");
    }
    s.push_str("        }\n    }\n    views {\n");
    let _ = writeln!(s, "        container s \"v\" {}{{", if title { "\"View\" " } else { "" });
    s.push_str("            include *\n");
    let key = |id: &str| if hier { format!("s.{id}") } else { id.to_owned() };
    for (id, x, y) in boxes {
        let _ = writeln!(s, "            at {} {x} {y}", key(id));
    }
    for line in extra.lines() {
        let _ = writeln!(s, "            {line}");
    }
    s.push_str("        }\n    }\n}\n");
    s
}

fn parsed(text: &str) -> Parsed {
    parse(text).unwrap_or_else(|e| panic!("{e}\n{text}"))
}

fn scene(model: &Model) -> Scene {
    build_view(model, "v", &Keep::default(), &AtomicBool::new(false)).expect("builds").scene
}

fn findings(model: &Model, scene: &Scene, rule: Rule) -> Vec<Finding> {
    lint(model, scene).findings.into_iter().filter(|f| f.rule == rule).collect()
}

fn id(model: &Model, scene: &Scene, name: &str) -> BoxId {
    let e = model.elements.iter().position(|e| e.name.value == format!("Box {name}")).expect("element");
    scene.boxes.iter().find(|b| b.element.0 as usize == e).expect("drawn").id
}

/// `text` with `r` written as `at` and `size` lines.
fn written(text: &str, r: &Resolution) -> String {
    let p = parsed(text);
    let s = scene(&p.model);
    let key = |b: BoxId| p.model.full_key(s.boxes[b.0 as usize].element).expect("key");
    let mut edits: Vec<Edit> = r
        .moves
        .iter()
        .map(|&(b, c)| Edit::Move {
            view: "v".to_owned(),
            element: key(b),
            centre: Some((c.x * GRID_PX, c.y * GRID_PX)),
        })
        .collect();
    edits.extend(r.sizes.iter().map(|&(b, size)| Edit::Resize {
        view: "v".to_owned(),
        element: key(b),
        size: Some(size),
    }));
    apply(&p, &edits).expect("applies").text
}

/// The L6 pairs of a model's view, as element id pairs.
fn l6_pairs(model: &Model, scene: &Scene) -> Vec<(usize, usize)> {
    findings(model, scene, Rule::TooClose)
        .iter()
        .map(|f| match f.subjects.as_slice() {
            [c4rizer_core::scene::ModelId::Element(a), c4rizer_core::scene::ModelId::Element(b)] => {
                (a.0 as usize, b.0 as usize)
            }
            other => panic!("{other:?}"),
        })
        .collect()
}

/// Resolves the single L6 of `boxes` and returns the resolution, after
/// checking the property: rebuilt, that pair is no longer too close and,
/// when `clean`, no pair is too close that was not before.
fn resolve_l6(hier: bool, boxes: &[(&str, i32, i32)], clean: bool) -> (Resolution, Model, Scene) {
    let text = dsl(hier, boxes, "", true);
    let p = parsed(&text);
    let s = scene(&p.model);
    let before = l6_pairs(&p.model, &s);
    let l6 = findings(&p.model, &s, Rule::TooClose);
    assert_eq!(l6.len(), 1, "fixture has one L6: {l6:?}");
    let r = resolve(&p.model, &s, &l6[0]).expect("L6 resolves");
    assert_eq!(r.label, "Move apart");
    assert!(r.sizes.is_empty());
    let after_text = written(&text, &r);
    let q = parsed(&after_text);
    let t = scene(&q.model);
    let after = l6_pairs(&q.model, &t);
    assert!(!after.contains(&before[0]), "the pair is still too close:\n{after_text}");
    if clean {
        assert!(after.iter().all(|p| before.contains(p)), "new L6 {after:?}:\n{after_text}");
    }
    (r, p.model, s)
}

fn at(x: i32, y: i32) -> Point {
    Point::new(x / GRID_PX, y / GRID_PX)
}

/// P5-400: an L6 pair is moved apart until its edges are 60px apart,
/// on the grid; rebuilt, the finding is gone and no new L6 appears.
#[test]
fn p5_400_l6_moves_the_pair_apart_and_the_finding_goes() {
    for hier in MODES {
        // a.max.x 320, b.min.x 360: 40px apart.
        let boxes = [("a", 200, 200), ("b", 480, 200), ("c", 200, 800)];
        let (r, model, s) = resolve_l6(hier, &boxes, true);
        let (a, b) = (id(&model, &s, "a"), id(&model, &s, "b"));
        let moved: Vec<BoxId> = r.moves.iter().map(|m| m.0).collect();
        assert_eq!(moved, vec![a.min(b), a.max(b)]);
        let centre = |b: BoxId| r.moves.iter().find(|m| m.0 == b).expect("moved").1;
        let gap = (centre(b).x - centre(a).x) * GRID_PX - 240;
        assert!(gap >= 60, "{gap}");
    }
}

/// P5-401: the first candidate is both boxes half each along the axis
/// needing the smaller push.
#[test]
fn p5_401_first_candidate_half_each_on_the_smaller_axis() {
    for hier in MODES {
        let boxes = [("a", 200, 200), ("b", 480, 200), ("c", 200, 800)];
        let (r, model, s) = resolve_l6(hier, &boxes, true);
        let (a, b) = (id(&model, &s, "a"), id(&model, &s, "b"));
        let mut want = vec![(a, at(190, 200)), (b, at(490, 200))];
        want.sort_by_key(|m| m.0);
        assert_eq!(r.moves, want);
    }
}

/// P5-401: when the first candidate brings a third box too close, the
/// second (half each along the other axis) is taken.
#[test]
fn p5_401_second_candidate_when_the_first_is_not_clean() {
    for hier in MODES {
        // c sits exactly 60px right of b: b moving right would crowd it.
        let boxes = [("a", 200, 200), ("b", 480, 200), ("c", 780, 200)];
        let (r, model, s) = resolve_l6(hier, &boxes, true);
        let (a, b) = (id(&model, &s, "a"), id(&model, &s, "b"));
        let mut want = vec![(a, at(200, 100)), (b, at(480, 300))];
        want.sort_by_key(|m| m.0);
        assert_eq!(r.moves, want);
    }
}

/// P5-401: when both half-each candidates crowd a third box, only the
/// second box moves.
#[test]
fn p5_401_third_candidate_moves_only_the_second_box() {
    for hier in MODES {
        // f 60px left of a, e 60px above a: a cannot move left or up.
        let boxes = [("a", 200, 200), ("b", 480, 200), ("e", 200, 0), ("f", -100, 200)];
        let (r, model, s) = resolve_l6(hier, &boxes, true);
        let b = id(&model, &s, "b");
        assert_eq!(r.moves, vec![(b, at(500, 200))]);
    }
}

/// P5-401: with no clean candidate the first is taken; the pair itself
/// is still resolved.
#[test]
fn p5_401_no_clean_candidate_takes_the_first() {
    for hier in MODES {
        // Every side of a and b is held 60px away by another box.
        let boxes = [
            ("a", 200, 200),
            ("b", 480, 200),
            ("f", -100, 200),
            ("g", 780, 200),
            ("e", 200, 0),
            ("h", 480, 400),
        ];
        let (r, model, s) = resolve_l6(hier, &boxes, false);
        let (a, b) = (id(&model, &s, "a"), id(&model, &s, "b"));
        let mut want = vec![(a, at(190, 200)), (b, at(490, 200))];
        want.sort_by_key(|m| m.0);
        assert_eq!(r.moves, want);
    }
}

/// P5-402: an L5 `at` line is snapped to the grid, and an L5 `size` line
/// to 20px steps; rebuilt, no L5 is left.
#[test]
fn p5_402_l5_snaps_at_and_size() {
    for hier in MODES {
        let key = if hier { "s.a" } else { "a" };
        let text = dsl(hier, &[("a", 205, 200), ("b", 800, 200)], &format!("size {key} 250 140"), true);
        let p = parsed(&text);
        let s = scene(&p.model);
        let l5 = findings(&p.model, &s, Rule::OffGrid);
        assert_eq!(l5.len(), 2, "{l5:?}");
        let a = id(&p.model, &s, "a");
        let rs: Vec<Resolution> = l5.iter().map(|f| resolve(&p.model, &s, f).expect("L5 resolves")).collect();
        assert!(rs.iter().all(|r| r.label == "Snap to grid"));
        assert_eq!(rs[0].moves, vec![(a, at(210, 200))]);
        assert!(rs[0].sizes.is_empty());
        assert_eq!(rs[1].sizes, vec![(a, (260, 140))]);
        assert!(rs[1].moves.is_empty());
        for r in &rs {
            let q = parsed(&written(&text, r));
            let t = scene(&q.model);
            assert_eq!(findings(&q.model, &t, Rule::OffGrid).len(), 1, "one L5 resolved, one left");
        }
    }
}

/// P5-403: an L25 box grows to the smallest 20px-step size, no side
/// smaller than now, at which its text fits; rebuilt, no L25 on it.
#[test]
fn p5_403_l25_grows_the_box_to_fit() {
    for hier in MODES {
        let key = if hier { "s.a" } else { "a" };
        let text = dsl(hier, &[("a", 300, 300), ("b", 1200, 300)], &format!("size {key} 100 60"), true);
        let p = parsed(&text);
        let s = scene(&p.model);
        let a = id(&p.model, &s, "a");
        let l25: Vec<Finding> =
            findings(&p.model, &s, Rule::TextFit).into_iter().filter(|f| f.items == [c4rizer_core::scene::ItemKey::Box(a)]).collect();
        assert_eq!(l25.len(), 1);
        let r = resolve(&p.model, &s, &l25[0]).expect("L25 resolves");
        assert_eq!(r.label, "Fit text");
        assert!(r.moves.is_empty());
        let [(b, (w, h))] = r.sizes[..] else { panic!("{r:?}") };
        assert_eq!(b, a);
        assert!(w >= 100 && h >= 60 && w % 20 == 0 && h % 20 == 0, "{w}x{h}");
        // Smallest: one step less on either side, still at least the
        // current size, does not fit.
        let spec = p.model.view("v").expect("view");
        let pb = &c4rizer_core::view::plan(&p.model, spec).boxes[a.0 as usize];
        let fits = |w: i32, h: i32| c4rizer_core::scene::texts_fit(pb, w / 20, h / 20);
        assert!(fits(w, h));
        for (sw, sh) in [(w - 20, h), (w, h - 20)] {
            assert!(sw < 100 || sh < 60 || !fits(sw, sh), "{sw}x{sh} fits too");
        }
        let q = parsed(&written(&text, &r));
        let t = scene(&q.model);
        let a2 = id(&q.model, &t, "a");
        assert!(
            !findings(&q.model, &t, Rule::TextFit).iter().any(|f| f.items == [c4rizer_core::scene::ItemKey::Box(a2)]),
            "L25 left on the grown box"
        );
    }
}

/// P5-404: a rule with no resolution gives `None` (here L8, no title).
#[test]
fn p5_404_other_rules_have_no_resolution() {
    let text = dsl(false, &[("a", 200, 200), ("b", 800, 200)], "", false);
    let p = parsed(&text);
    let s = scene(&p.model);
    let report = lint(&p.model, &s);
    let l8: Vec<&Finding> = report.findings.iter().filter(|f| f.rule == Rule::NoTitle).collect();
    assert_eq!(l8.len(), 1);
    assert_eq!(resolve(&p.model, &s, l8[0]), None);
    assert!(report.findings.iter().all(|f| !matches!(f.rule, Rule::OffGrid | Rule::TooClose | Rule::TextFit)));
    assert_eq!(resolve_all(&p.model, &s, &report.findings), None, "nothing to resolve");
}

/// A crowded grid of `cols` x `rows` containers, 10px apart across and
/// 30px apart down (every neighbour L6), the first one off the grid and
/// the second with an off-grid size.
fn crowded(hier: bool, cols: i32, rows: i32) -> String {
    let names: Vec<String> = (0..cols * rows).map(|i| format!("n{i}")).collect();
    let boxes: Vec<(&str, i32, i32)> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let i = i32::try_from(i).expect("small");
            let (x, y) = (200 + (i % cols) * 250, 200 + (i / cols) * 170);
            (n.as_str(), if i == 0 { x + 5 } else { x }, y)
        })
        .collect();
    let key = if hier { "s.n1" } else { "n1" };
    dsl(hier, &boxes, &format!("size {key} 250 140"), true)
}

/// P5-405: `resolve_all` on a crowded generated view resolves every L5
/// and L6 in one resolution; rebuilt, none is left.
#[test]
fn p5_405_resolve_all_clears_l5_and_l6() {
    for hier in MODES {
        let text = crowded(hier, 8, 5);
        let p = parsed(&text);
        let s = scene(&p.model);
        let report = lint(&p.model, &s);
        assert!(report.findings.iter().filter(|f| f.rule == Rule::TooClose).count() > 40);
        assert_eq!(report.findings.iter().filter(|f| f.rule == Rule::OffGrid).count(), 2);
        let r = resolve_all(&p.model, &s, &report.findings).expect("resolves");
        assert_eq!(r.label, "Resolve all");
        let q = parsed(&written(&text, &r));
        let t = scene(&q.model);
        let left: Vec<String> = lint(&q.model, &t)
            .findings
            .iter()
            .filter(|f| matches!(f.rule, Rule::OffGrid | Rule::TooClose))
            .map(|f| f.message.clone())
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }
}

// ---------------------------------------------------------------- P5-900

/// Times `resolve` of the first L6 and `resolve_all` on a crowded
/// `cols` x `rows` view.
fn bench_resolve(cols: i32, rows: i32, budgets: (u64, u64)) {
    let text = crowded(false, cols, rows);
    let p = parsed(&text);
    let s = scene(&p.model);
    let report = lint(&p.model, &s);
    let first = report.findings.iter().find(|f| f.rule == Rule::TooClose).expect("an L6").clone();
    let input = format!("generated {} boxes", cols * rows);
    bench::bench("P5-400", "lint resolve", "resolve L6", &input, budgets.0, || {
        std::hint::black_box(resolve(&p.model, &s, &first));
    });
    bench::bench("P5-405", "lint resolve", "resolve_all", &input, budgets.1, || {
        std::hint::black_box(resolve_all(&p.model, &s, &report.findings));
    });
}

#[test]
#[ignore = "timing: cargo test --release -- --ignored perf_p5"]
fn perf_p5_400_resolve_40_boxes() {
    if bench::debug_build("perf_p5_400_resolve_40_boxes") {
        return;
    }
    bench_resolve(8, 5, (50, 100));
}

#[test]
#[ignore = "timing: cargo test --release -- --ignored perf_p5"]
fn perf_p5_400_resolve_400_boxes() {
    if bench::debug_build("perf_p5_400_resolve_400_boxes") {
        return;
    }
    bench_resolve(20, 20, (50, 4_000));
}
