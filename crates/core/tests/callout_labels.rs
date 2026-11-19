//! Tests of the callout-labels row (SRS 4.8: P4-300..P4-316, P4-319),
//! core part. One test per requirement at least, named after it. The
//! editor tests (P4-317, P4-318) are in
//! `crates/editor/src/callout_labels_tests.rs`.

// Tests name the model `m`, the build `b`, a label `l`, as the other rows do.
// `broken` checks every geometry property in one pass over a scene.
#![allow(clippy::many_single_char_names, clippy::similar_names, clippy::too_many_lines)]

use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

use c4rizer_core::edit::{self, Edit};
use c4rizer_core::export;
use c4rizer_core::geom::{Contact, Point, Rect, Segment};
use c4rizer_core::label::{CALLOUT_CLEAR_PX, CALLOUT_SLOPES, LEADER_MAX_PX, LEADER_MIN_PX};
use c4rizer_core::lint::Rule;
use c4rizer_core::model::{self, LabelMode, Model};
use c4rizer_core::pipeline::{self, ViewBuild};
use c4rizer_core::place::Keep;
use c4rizer_core::scene::{Scene, SceneLabel, Tip};

const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl"));
const WORKSPACE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example/workspace.dsl"));
const DYNAMIC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/dynamic_view.dsl"));
const DEPLOYMENT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/deployment_view.dsl"));
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/golden");
const KEYS: [&str; 4] = ["C1", "C2", "C3-serving", "C3-startup"];

/// Two boxes side by side, one long horizontal line between them.
const PAIR: &str = r#"workspace {
    model {
        a = softwareSystem "Alpha" "First system."
        b = softwareSystem "Beta" "Second system."
        a -> b "Sends orders to"
    }
    views {
        systemContext a "pair" "Pair" {
            include a b
            at a 200 300
            at b 900 300
SETTING
        }
    }
}
"#;

/// `src` with `line` added as the first line of every view block.
fn every_view(src: &str, line: &str) -> String {
    let mut out = String::new();
    let mut in_views = false;
    for l in src.lines() {
        out.push_str(l);
        out.push('\n');
        let t = l.trim_start();
        if t.starts_with("views") && t.ends_with('{') {
            in_views = true;
            continue;
        }
        let kinds = ["systemContext ", "container ", "component ", "dynamic ", "deployment ", "systemLandscape "];
        if in_views && t.ends_with('{') && kinds.iter().any(|k| t.starts_with(k)) {
            out.push_str("            ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn parse(src: &str) -> Model {
    match model::parse(src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    }
}

fn build(m: &Model, key: &str) -> ViewBuild {
    match pipeline::build_view(m, key, &Keep::default(), &AtomicBool::new(false)) {
        Ok(b) => b,
        Err(e) => panic!("{e:?}"),
    }
}

/// The build of the `pair` view with `setting` in its block.
fn pair(setting: &str) -> ViewBuild {
    build(&parse(&PAIR.replace("SETTING", setting)), "pair")
}

/// Every view of `src` with `labels callout`, built.
fn callout_builds(src: &str) -> Vec<ViewBuild> {
    let m = parse(&every_view(src, "labels callout"));
    m.views.iter().map(|v| build(&m, &v.key)).collect()
}

/// The four example views with `labels callout`, built once for every
/// test (auto-placement is slow in debug builds).
fn example() -> &'static [ViewBuild] {
    static BUILDS: OnceLock<Vec<ViewBuild>> = OnceLock::new();
    BUILDS.get_or_init(|| callout_builds(WORKSPACE))
}

/// The segments of a polyline.
fn segments(points: &[Point]) -> Vec<Segment> {
    points.windows(2).map(|w| Segment::new(w[0], w[1])).collect()
}

/// True when segment `s` and `t` share any point.
fn meet(s: &Segment, t: &Segment) -> bool {
    s.contact(t) != Contact::Disjoint
}

/// True when the segment `s` has a point strictly inside `r`.
fn through(s: &Segment, r: &Rect) -> bool {
    s.passes_through(r)
}

/// Every P4-303..P4-309 property broken by the callouts of `scene`, as
/// readable lines; empty when all hold.
fn broken(scene: &Scene) -> Vec<String> {
    let mut out = Vec::new();
    for l in &scene.labels {
        let Some(c) = &l.callout else { continue };
        let name = format!("{} edge {}", scene.key, l.edge.0);
        let r = l.rect;
        let pad = c4rizer_core::label::LABEL_PAD_X_PX;
        // P4-302: no backing.
        if l.fill.is_some() {
            out.push(format!("{name}: backing fill"));
        }
        // P4-303: the shelf.
        if c.shelf != [Point::new(r.min.x + pad, r.max.y), Point::new(r.max.x - pad, r.max.y)] {
            out.push(format!("{name}: shelf {:?} for text {r:?}", c.shelf));
        }
        // P4-304: the leader goes away from the text.
        let [s, a] = c.leader;
        let right = s == c.shelf[1] && a.x > s.x;
        let left = s == c.shelf[0] && a.x < s.x;
        if !(right || left) {
            out.push(format!("{name}: leader {:?} not outward from shelf {:?}", c.leader, c.shelf));
        }
        // P4-305: slope and length.
        let (dx, dy) = ((a.x - s.x).abs(), (a.y - s.y).abs());
        let slope_ok = CALLOUT_SLOPES
            .iter()
            .any(|&(sx, sy)| dx % sx == 0 && dx / sx >= 1 && dy == dx / sx * sy);
        if !slope_ok {
            out.push(format!("{name}: leader slope {dx}:{dy}"));
        }
        let len2 = i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy);
        let (lo, hi) = (i64::from(LEADER_MIN_PX), i64::from(LEADER_MAX_PX));
        if len2 < lo * lo || len2 > hi * hi {
            out.push(format!("{name}: leader length^2 {len2}"));
        }
        if c.dot != a {
            out.push(format!("{name}: dot {:?} not at attach {a:?}", c.dot));
        }
        // P4-306: A on its own line, inside the gaps.
        let own = scene.lines.iter().find(|x| x.edge == l.edge).expect("own line");
        let own_segs = segments(&own.points);
        let mut along = None;
        let mut walked = 0.0_f64;
        for seg in &own_segs {
            let point = Segment::new(a, a);
            let len = f64::from((seg.b.x - seg.a.x).pow(2) + (seg.b.y - seg.a.y).pow(2)).sqrt();
            if along.is_none() && meet(seg, &point) {
                along = Some(walked + f64::from((a.x - seg.a.x).pow(2) + (a.y - seg.a.y).pow(2)).sqrt());
            }
            walked += len;
        }
        match along {
            None => out.push(format!("{name}: attach {a:?} not on its line")),
            Some(d) => {
                let end_gap = if matches!(own.tip, Tip::Dot { .. }) { 0.0 } else { 50.0 };
                if d < 30.0 - 1e-6 || walked - d < end_gap - 1e-6 {
                    out.push(format!("{name}: attach {d:.1}px along a {walked:.1}px line"));
                }
            }
        }
        if a.x % 10 != 0 || a.y % 10 != 0 {
            out.push(format!("{name}: attach {a:?} off the grid"));
        }
        // P4-307: colour and width.
        if c.stroke.colour != own.stroke.colour
            || c.stroke.width_px != (own.stroke.width_px / 2).max(1)
            || c.stroke.dash_px.is_some()
            || c.dot_radius_px != 3
        {
            out.push(format!("{name}: stroke {:?} dot {}", c.stroke, c.dot_radius_px));
        }
        // P4-308: the grown text block.
        let grown = r.inflate(CALLOUT_CLEAR_PX);
        for b in &scene.boxes {
            if grown.overlaps(&b.rect) {
                out.push(format!("{name}: text over box {:?}", b.id));
            }
        }
        for t in &scene.boundaries {
            if grown.overlaps(&t.tag_rect) {
                out.push(format!("{name}: text over boundary tag"));
            }
            // The dashed stroke, 2px each side.
            let b = t.rect;
            let band = |x0, y0, x1, y1| Rect::from_corners(Point::new(x0, y0), Point::new(x1, y1));
            let bands = [
                band(b.min.x - 2, b.min.y - 2, b.max.x + 2, b.min.y + 2),
                band(b.min.x - 2, b.max.y - 2, b.max.x + 2, b.max.y + 2),
                band(b.min.x - 2, b.min.y - 2, b.min.x + 2, b.max.y + 2),
                band(b.max.x - 2, b.min.y - 2, b.max.x + 2, b.max.y + 2),
            ];
            if bands.iter().any(|x| x.overlaps(&r)) {
                out.push(format!("{name}: text across a boundary stroke"));
            }
        }
        for o in scene.labels.iter().filter(|o| o.edge != l.edge) {
            if grown.overlaps(&o.rect) {
                out.push(format!("{name}: text over label of edge {}", o.edge.0));
            }
        }
        for x in &scene.lines {
            if segments(&x.points).iter().any(|s| through(s, &grown)) {
                out.push(format!("{name}: line {} through text", x.edge.0));
            }
        }
        // P4-309: the leader.
        let leader = Segment::new(s, a);
        for b in &scene.boxes {
            if through(&leader, &b.rect) {
                out.push(format!("{name}: leader through box {:?}", b.id));
            }
        }
        for o in scene.labels.iter().filter(|o| o.edge != l.edge) {
            if through(&leader, &o.rect) {
                out.push(format!("{name}: leader through label of edge {}", o.edge.0));
            }
            if let Some(oc) = &o.callout {
                let marks = [Segment::new(oc.shelf[0], oc.shelf[1]), Segment::new(oc.leader[0], oc.leader[1])];
                if marks.iter().any(|m| meet(m, &leader)) {
                    out.push(format!("{name}: leader meets callout of edge {}", o.edge.0));
                }
            }
        }
        for seg in &own_segs {
            let on_a = meet(seg, &Segment::new(a, a));
            if meet(seg, &leader) && !on_a {
                out.push(format!("{name}: leader meets its own line away from A"));
            }
        }
        // P4-310: the crossings are the other lines the leader meets.
        let mut crossed: Vec<u32> = scene
            .lines
            .iter()
            .filter(|x| x.edge != l.edge && segments(&x.points).iter().any(|t| meet(t, &leader)))
            .map(|x| x.edge.0)
            .collect();
        crossed.sort_unstable();
        let said: Vec<u32> = c.crossings.iter().map(|e| e.0).collect();
        if crossed != said {
            out.push(format!("{name}: crossings {said:?}, drawn {crossed:?}"));
        }
    }
    out
}

/// The callout labels of `scene`.
fn callouts(scene: &Scene) -> Vec<&SceneLabel> {
    scene.labels.iter().filter(|l| l.callout.is_some()).collect()
}

// ------------------------------------------------------------------ file

#[test]
fn p4_300_labels_line_parsed() {
    for (setting, want) in [
        ("labels callout", LabelMode::Callout),
        ("LABELS Callout", LabelMode::Callout),
        ("labels line", LabelMode::Line),
        ("", LabelMode::Line),
    ] {
        let m = parse(&PAIR.replace("SETTING", setting));
        let v = m.view("pair").expect("view");
        assert_eq!(v.options().labels, want, "{setting:?}");
        assert_eq!(v.labels.map(|l| l.value), (!setting.is_empty()).then_some(want), "{setting:?}");
    }
    assert_eq!(LabelMode::Callout.keyword(), "callout");
    assert_eq!(LabelMode::Line.keyword(), "line");
    // A second line overrides the first, with a notice.
    let m = parse(&PAIR.replace("SETTING", "labels callout\n            labels line"));
    assert_eq!(m.view("pair").expect("view").options().labels, LabelMode::Line);
    assert!(m.notices.iter().any(|n| n.to_string().contains("earlier `labels` line")), "{:?}", m.notices);
    // Any other value is an error.
    let e = model::parse(&PAIR.replace("SETTING", "labels boxes")).expect_err("refused");
    assert!(e.to_string().contains("labels"), "{e}");
}

#[test]
fn p4_301_line_mode_is_byte_identical() {
    let m = parse(&every_view(EXAMPLE, "labels line"));
    let all = pipeline::build_all(&m, &AtomicBool::new(false)).expect("builds");
    for key in KEYS {
        let b = all.views.iter().find(|v| v.plan.key == key).expect("view");
        let want = std::fs::read_to_string(format!("{GOLDEN}/{key}.svg")).expect("golden");
        assert!(export::svg(&b.scene) == want, "{key} differs from its golden with `labels line`");
        assert!(b.labels.labels.iter().flatten().all(|l| l.callout.is_none()));
        assert!(b.labels.fallbacks.is_empty());
    }
}

// -------------------------------------------------------------- geometry

#[test]
fn p4_302_text_has_no_backing() {
    let b = pair("labels callout");
    let l = &b.scene.labels[0];
    assert!(l.callout.is_some(), "a free page: a callout");
    assert_eq!(l.fill, None);
    let on_line = pair("");
    let want = &on_line.scene.labels[0];
    assert_eq!(l.text.lines, want.text.lines, "same wrapping");
    assert_eq!((l.text.size_px, l.text.colour), (want.text.size_px, want.text.colour), "same font and colour");
    assert_eq!((l.rect.width(), l.rect.height()), (want.rect.width(), want.rect.height()), "same size");
    assert!(broken(&b.scene).is_empty(), "{:?}", broken(&b.scene));
}

#[test]
fn p4_303_shelf_under_the_text() {
    let b = pair("labels callout");
    let l = &b.scene.labels[0];
    let c = l.callout.as_ref().expect("callout");
    assert_eq!(c.shelf[0].y, l.rect.max.y);
    assert_eq!(c.shelf[1].y, l.rect.max.y);
    assert_eq!(c.shelf[1].x - c.shelf[0].x, l.rect.width() - 10);
}

#[test]
fn p4_304_leader_goes_outward() {
    for b in example() {
        for l in callouts(&b.scene) {
            let c = l.callout.as_ref().expect("callout");
            let [s, a] = c.leader;
            assert!(
                (s == c.shelf[1] && a.x > s.x) || (s == c.shelf[0] && a.x < s.x),
                "{} edge {}: {:?} {:?}",
                b.plan.key,
                l.edge.0,
                c.shelf,
                c.leader
            );
        }
    }
}

#[test]
fn p4_305_slope_band_and_length() {
    // Every slope lies within 14.9..25.1 degrees.
    for (dx, dy) in CALLOUT_SLOPES {
        let deg = f64::from(dy).atan2(f64::from(dx)).to_degrees();
        assert!((14.9..=25.1).contains(&deg), "{dx}:{dy} is {deg:.2} degrees");
    }
    assert_eq!(CALLOUT_SLOPES[0], (11, 4), "20 degrees preferred");
    assert_eq!((LEADER_MIN_PX, LEADER_MAX_PX), (20, 140));
    for b in example() {
        let bad: Vec<String> = broken(&b.scene).into_iter().filter(|s| s.contains("slope") || s.contains("length")).collect();
        assert!(bad.is_empty(), "{bad:?}");
    }
}

#[test]
fn p4_306_attach_inside_the_gaps() {
    for b in example() {
        let bad: Vec<String> = broken(&b.scene).into_iter().filter(|s| s.contains("attach")).collect();
        assert!(bad.is_empty(), "{bad:?}");
    }
}

#[test]
fn p4_307_drawn_in_line_colour_thinner() {
    let b = pair("labels callout");
    let l = &b.scene.labels[0];
    let c = l.callout.as_ref().expect("callout");
    let line = &b.scene.lines[0];
    assert_eq!(c.stroke.colour, line.stroke.colour);
    assert!(c.stroke.width_px < line.stroke.width_px, "thinner than the line");
    assert_eq!(c.dot, c.leader[1]);
    assert_eq!(c.dot_radius_px, 3);
    // A thick line: half its width.
    let thick = PAIR
        .replace("SETTING", "labels callout")
        .replace("    views {", "    views {\n        styles {\n            relationship \"Relationship\" {\n                thickness 6\n            }\n        }");
    let b = build(&parse(&thick), "pair");
    let c = b.scene.labels[0].callout.as_ref().expect("callout");
    assert_eq!(c.stroke.width_px, 3);
}

// ------------------------------------------------------------ no overlap

#[test]
fn p4_308_text_clear_on_example_views() {
    for b in example() {
        let bad: Vec<String> = broken(&b.scene).into_iter().filter(|s| s.contains("text")).collect();
        assert!(bad.is_empty(), "{bad:?}");
    }
}

#[test]
fn p4_309_leader_clear_on_example_views() {
    for b in example() {
        let bad: Vec<String> = broken(&b.scene).into_iter().filter(|s| s.contains("leader")).collect();
        assert!(bad.is_empty(), "{bad:?}");
    }
}

#[test]
fn p4_310_leader_crossings_counted() {
    let mut total = 0;
    for b in example() {
        let bad: Vec<String> = broken(&b.scene).into_iter().filter(|s| s.contains("crossings")).collect();
        assert!(bad.is_empty(), "{bad:?}");
        for l in b.labels.labels.iter().flatten() {
            if let Some(c) = &l.callout {
                assert!(c.crossings.windows(2).all(|w| w[0] < w[1]), "ascending");
                total += c.crossings.len();
            }
        }
    }
    let labels: usize = example().iter().map(|b| callouts(&b.scene).len()).sum();
    assert!(total <= labels, "{total} leader crossings for {labels} callouts");
}

// ---------------------------------------------------------------- search

#[test]
fn p4_311_least_cost_on_a_free_page() {
    // A long free horizontal line: the cheapest callout is the shortest
    // 20-degree leader that clears the line, at the middle of the line.
    let b = pair("labels callout");
    let c = b.scene.labels[0].callout.as_ref().expect("callout");
    let [s, a] = c.leader;
    assert_eq!(((a.x - s.x).abs(), (a.y - s.y).abs()), (22, 8), "two (11, 4) steps");
    let line = &b.scene.lines[0];
    let middle = i32::midpoint(line.points[0].x, line.points[line.points.len() - 1].x);
    assert!((a.x - middle).abs() <= 10, "attach {a:?} near the middle x {middle}");
    assert!(c.crossings.is_empty());
    // Deterministic.
    assert_eq!(pair("labels callout").scene, b.scene);
}

#[test]
fn p4_311_position_moves_the_attach_point() {
    let b = pair("labels callout\n            line a -> b position 25");
    let c = b.scene.labels[0].callout.as_ref().expect("callout");
    let line = &b.scene.lines[0];
    let (x0, x1) = (line.points[0].x, line.points[line.points.len() - 1].x);
    let want = x0 + (x1 - x0) / 4;
    assert!((c.leader[1].x - want).abs() <= 10, "attach {:?} near x {want}", c.leader[1]);
}

#[test]
fn p4_312_callouts_keep_clear_of_earlier_ones() {
    for b in example() {
        let bad: Vec<String> = broken(&b.scene).into_iter().filter(|s| s.contains("label of edge") || s.contains("callout of")).collect();
        assert!(bad.is_empty(), "{bad:?}");
        // Shared trunk labels stay shared.
        for (joined, _) in &b.labels.shared {
            assert!(b.labels.label(*joined).is_none());
        }
    }
}

#[test]
fn p4_313_fallback_on_line_reported_l29() {
    // Two boxes 60px apart: no attach point clears the 30px and 50px gaps.
    let src = PAIR.replace("at b 900 300", "at b 500 300").replace("SETTING", "labels callout");
    let m = parse(&src);
    let b = build(&m, "pair");
    let l = b.labels.labels[0].as_ref().expect("a label");
    assert!(l.callout.is_none(), "no room: on its line");
    assert_eq!(b.labels.fallbacks, vec![l.edge]);
    let l29: Vec<_> = b.report.findings.iter().filter(|f| f.rule == Rule::CalloutFallback).collect();
    assert_eq!(l29.len(), 1);
    assert_eq!(Rule::CalloutFallback.code(), "L29");
    assert_eq!(
        l29[0].message,
        "no free place for callout label \"Sends orders to\" (\"Alpha\" -> \"Beta\"); drawn on its line"
    );
    // Never in line mode.
    let b = pair("");
    assert!(b.report.findings.iter().all(|f| f.rule != Rule::CalloutFallback));
}

#[test]
fn p4_313_fallbacks_keep_clear_of_callouts() {
    for b in example() {
        let marks: Vec<Segment> = b
            .scene
            .labels
            .iter()
            .filter_map(|l| l.callout.as_ref())
            .flat_map(|c| [Segment::new(c.shelf[0], c.shelf[1]), Segment::new(c.leader[0], c.leader[1])])
            .collect();
        for l in b.scene.labels.iter().filter(|l| l.callout.is_none()) {
            assert!(!marks.iter().any(|m| through(m, &l.rect)), "{} edge {}: on-line label over a callout", b.plan.key, l.edge.0);
            for o in callouts(&b.scene) {
                assert!(!o.rect.overlaps(&l.rect), "{} edge {}", b.plan.key, l.edge.0);
            }
        }
    }
}

#[test]
fn p4_314_dynamic_and_deployment_views() {
    for src in [DYNAMIC, DEPLOYMENT] {
        let builds = callout_builds(src);
        let count: usize = builds.iter().map(|b| callouts(&b.scene).len()).sum();
        assert!(count > 0, "callouts drawn");
        for b in &builds {
            assert!(broken(&b.scene).is_empty(), "{:?}", broken(&b.scene));
        }
    }
}

#[test]
fn p4_315_example_views_clean_with_few_fallbacks() {
    let builds = example();
    assert_eq!(builds.iter().map(|b| b.plan.key.as_str()).collect::<Vec<_>>(), KEYS);
    for b in builds {
        assert!(broken(&b.scene).is_empty(), "{:?}", broken(&b.scene));
        let labels = b.scene.labels.len();
        let fallbacks = b.labels.fallbacks.len();
        assert!(4 * fallbacks <= labels, "{}: {fallbacks} fallbacks of {labels} labels", b.plan.key);
    }
}

// ---------------------------------------------------------------- export

#[test]
fn p4_316_svg_draws_the_callout_in_the_label_group() {
    let b = pair("labels callout");
    let svg = export::svg(&b.scene);
    let start = svg.find("<g class=\"label\"").expect("label group");
    let group = &svg[start..start + svg[start..].find("</g>").expect("closed")];
    let text = group.find("<text").expect("text");
    let marks = &group[..text];
    assert_eq!(marks.matches("<polyline").count() + marks.matches("<path").count(), 2, "shelf and leader: {group}");
    assert!(marks.contains("<circle"), "the dot: {group}");
    assert!(!marks.contains("<rect"), "no backing: {group}");
    // The scene bounds hold the leader.
    let c = b.scene.labels[0].callout.as_ref().expect("callout");
    for p in c.leader.iter().chain(&c.shelf) {
        assert!(b.scene.bounds.contains(*p));
    }
}

#[test]
fn p4_319_workspace_json_has_no_label_mode() {
    let plain = parse(&PAIR.replace("SETTING", ""));
    let callout = parse(&PAIR.replace("SETTING", "labels callout"));
    let j = |m: &Model| {
        let all = pipeline::build_all(m, &AtomicBool::new(false)).expect("builds");
        export::json(m, &all, None)
    };
    assert_eq!(j(&plain), j(&callout));
}

// ------------------------------------------------------------------ edit

#[test]
fn p4_318_set_label_mode_edit_and_inverse() {
    let src = PAIR.replace("SETTING", "            labelWidth 20");
    let parsed = model::parse(&src).expect("parses");
    let done = edit::apply(
        &parsed,
        &[Edit::SetLabelMode {
            view: "pair".to_owned(),
            mode: Some(LabelMode::Callout),
        }],
    )
    .expect("applies");
    assert!(done.text.contains("labelWidth 20\n            labels callout\n"), "{}", done.text);
    let back = edit::apply(&model::parse(&done.text).expect("parses"), &done.inverse).expect("applies");
    assert_eq!(back.text, src);
    // Rewritten in place, then removed.
    let again = edit::apply(
        &model::parse(&done.text).expect("parses"),
        &[Edit::SetLabelMode {
            view: "pair".to_owned(),
            mode: None,
        }],
    )
    .expect("applies");
    assert_eq!(again.text, src);
    // No setting line: after the last include line.
    let src = PAIR.replace("SETTING", "");
    let done = edit::apply(
        &model::parse(&src).expect("parses"),
        &[Edit::SetLabelMode {
            view: "pair".to_owned(),
            mode: Some(LabelMode::Callout),
        }],
    )
    .expect("applies");
    assert!(done.text.contains("include a b\n            labels callout\n"), "{}", done.text);
}
