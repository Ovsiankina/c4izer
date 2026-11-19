//! Tests of the boundary-clear row (SRS 4.8: P4-600..P4-602): no line
//! runs along a boundary's stroke or the page edge.

use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::{Point, Rect};
use c4rizer_core::model::{self, Model};
use c4rizer_core::pipeline::build_all;
use c4rizer_core::scene::Scene;

/// One lane pitch (CORE-12) in px: the clearance of P4-600 and P4-601.
const CLEAR_PX: i32 = 20;

fn parse(src: &str) -> Model {
    match model::parse(src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e:?}"),
    }
}

/// Length along `lo..hi` (ascending) that two closed ranges share.
fn overlap(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.1.min(b.1) - a.0.max(b.0)).max(0)
}

/// Every stretch where a horizontal or vertical segment of a line of
/// `scene` runs parallel to a boundary side, or to the page edge
/// (`Scene::bounds`), less than `CLEAR_PX` from it (P4-600, P4-601):
/// `(line index, what, px of shared length)`.
fn band_hits(scene: &Scene) -> Vec<(usize, String, i32)> {
    let mut sides: Vec<(String, Rect)> = scene
        .boundaries
        .iter()
        .map(|b| (format!("boundary {}", b.tag.lines.join(" ")), b.rect))
        .collect();
    sides.push(("page edge".to_owned(), scene.bounds));
    let mut out = Vec::new();
    for (i, l) in scene.lines.iter().enumerate() {
        for w in l.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            for (what, r) in &sides {
                let shared = if a.x == b.x && a.y != b.y {
                    let span = (a.y.min(b.y), a.y.max(b.y));
                    [r.min.x, r.max.x]
                        .iter()
                        .filter(|&&sx| (a.x - sx).abs() < CLEAR_PX)
                        .map(|_| overlap(span, (r.min.y, r.max.y)))
                        .sum()
                } else if a.y == b.y && a.x != b.x {
                    let span = (a.x.min(b.x), a.x.max(b.x));
                    [r.min.y, r.max.y]
                        .iter()
                        .filter(|&&sy| (a.y - sy).abs() < CLEAR_PX)
                        .map(|_| overlap(span, (r.min.x, r.max.x)))
                        .sum()
                } else {
                    0
                };
                if shared > 0 {
                    out.push((i, what.clone(), shared));
                }
            }
        }
    }
    out
}

/// `band_hits` on every view of `src`, as `view: hits` strings.
fn hits_of(src: &str) -> Vec<String> {
    let views = build_all(&parse(src), &AtomicBool::new(false)).expect("never stopped");
    views
        .views
        .iter()
        .flat_map(|v| {
            band_hits(&v.scene)
                .into_iter()
                .map(move |(i, what, px)| format!("{} line {i}: {what}, {px}px", v.plan.key))
        })
        .collect()
}

macro_rules! data {
    ($p:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/", $p))
    };
}

const WORKSPACE: &str = data!("example/workspace.dsl");

/// `example/workspace.dsl` with `lines` added after the view header
/// `head`.
fn workspace_with(head: &str, lines: &str) -> String {
    assert!(WORKSPACE.contains(head), "no view header {head}");
    WORKSPACE.replacen(head, &format!("{head}\n{lines}"), 1)
}

/// P4-600: the reported case, C3-startup of `example/workspace.dsl`,
/// where a line ran 980px on the dashed stroke of "Atomanja server
/// [Container]", and every other view of that file: no line runs along
/// a boundary's stroke, and none was left unroutable for it.
#[test]
fn p4_600_reported_view_runs_clear_of_the_stroke() {
    let m = parse(WORKSPACE);
    let views = build_all(&m, &AtomicBool::new(false)).expect("never stopped");
    let startup = views.views.iter().find(|v| v.plan.key == "C3-startup").expect("C3-startup");
    assert!(!startup.scene.boundaries.is_empty());
    for v in &views.views {
        assert!(v.routes.unroutable().is_empty(), "{}: unroutable", v.plan.key);
        let hits: Vec<_> = band_hits(&v.scene).into_iter().filter(|h| h.1 != "page edge").collect();
        assert!(hits.is_empty(), "{}: {hits:?}", v.plan.key);
    }
}

/// P4-600 with bundles: every track and fan-out of a bus keeps the
/// clearance (bundled copies of C2, C3-serving and C3-startup; the
/// C3-startup bundle takes the two lines that ran along the stroke).
#[test]
fn p4_600_bundles_run_clear_of_the_stroke() {
    let cases = [
        ("\"C2\" \"Container diagram for Atomanja\" {", "            bundle admin -> app worker -> app witness -> app\n            bundle app -> devdb app -> db app -> vikunja"),
        (
            "\"C3-serving\" \"Component diagram for the Atomanja server: serving and polling\" {",
            "            bundle vikunjaClient -> vikunja store -> devdb store -> db\n            bundle witness -> web worker -> web admin -> web",
        ),
        (
            "\"C3-startup\" \"Component diagram for the Atomanja server: startup, settings and dev data\" {",
            "            bundle web -> ledger web -> auth",
        ),
    ];
    for (head, lines) in cases {
        let h = hits_of(&workspace_with(head, lines));
        assert!(h.is_empty(), "{head}: {h:?}");
    }
}

/// P4-600 in dynamic and deployment views (deployment node boundaries
/// nest), and on the `via` fixture (hand-placed vertices).
#[test]
fn p4_600_dynamic_and_deployment_views_run_clear() {
    for (name, src) in [
        ("deployment_view.dsl", data!("deployment_view.dsl")),
        ("dynamic_view.dsl", data!("dynamic_view.dsl")),
        ("dynamic_response.dsl", data!("dynamic_response.dsl")),
        ("vertices.dsl", data!("vertices.dsl")),
        ("landscape_filtered.dsl", data!("landscape_filtered.dsl")),
    ] {
        let h = hits_of(src);
        assert!(h.is_empty(), "{name}: {h:?}");
    }
}

/// P4-601: no line runs within 20px of the page edge, on every view of
/// both examples and the fixtures above.
#[test]
fn p4_601_no_line_along_the_page_edge() {
    for src in [data!("example.dsl"), WORKSPACE, data!("deployment_view.dsl"), data!("dynamic_view.dsl")] {
        let h: Vec<_> = hits_of(src).into_iter().filter(|h| h.contains("page edge")).collect();
        assert!(h.is_empty(), "{h:?}");
    }
}

/// P4-602: the measured length is 0 on every view of `example.dsl` and
/// `example/workspace.dsl` (the goldens themselves are compared byte for
/// byte by the CLI's `cli_5_example_golden`).
#[test]
fn p4_602_examples_measure_zero() {
    for (name, src) in [("example.dsl", data!("example.dsl")), ("example/workspace.dsl", WORKSPACE)] {
        let h = hits_of(src);
        assert!(h.is_empty(), "{name}: {h:?}");
    }
}

/// The measure itself: a segment on a stroke, 10px inside it, or along
/// the page edge counts; one 20px away, one crossing square, or one past
/// the side's ends does not.
#[test]
fn p4_600_band_measure() {
    let m = parse(WORKSPACE);
    let views = build_all(&m, &AtomicBool::new(false)).expect("never stopped");
    let mut scene = views.views.iter().find(|v| v.plan.key == "C3-startup").expect("C3-startup").scene.clone();
    let r = scene.boundaries[0].rect;
    let line = |pts: &[(i32, i32)]| pts.iter().map(|&(x, y)| Point::new(x, y)).collect::<Vec<_>>();
    let (x0, y0, y1) = (r.min.x, r.min.y, r.max.y);
    let probe = |scene: &mut Scene, pts: Vec<Point>| -> i32 {
        scene.lines.truncate(1);
        scene.lines[0].points = pts;
        band_hits(scene).iter().map(|h| h.2).sum()
    };
    assert_eq!(probe(&mut scene, line(&[(x0, y0 + 100), (x0, y0 + 300)])), 200);
    assert_eq!(probe(&mut scene, line(&[(x0 + 10, y0 + 100), (x0 + 10, y0 + 300)])), 200);
    assert_eq!(probe(&mut scene, line(&[(x0 - 19, y0 - 50), (x0 - 19, y0 + 30)])), 30);
    assert_eq!(probe(&mut scene, line(&[(x0 + 20, y0 + 100), (x0 + 20, y0 + 300)])), 0);
    assert_eq!(probe(&mut scene, line(&[(x0 - 100, y0 + 100), (x0 + 100, y0 + 100)])), 0);
    assert_eq!(probe(&mut scene, line(&[(x0, y1 + 10), (x0, y1 + 100)])), 0);
    let b = scene.bounds;
    assert!(probe(&mut scene, line(&[(b.min.x + 100, b.max.y - 5), (b.min.x + 300, b.max.y - 5)])) >= 200);
}
