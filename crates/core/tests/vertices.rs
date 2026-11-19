//! Tests of the vertices row (SRS 4.7: P3-4, P3-5, P3-300..P3-318), core
//! part. One test per requirement at least, named after it. The editor
//! tests (P3-310..P3-315) are in `crates/editor/src/vertices_tests.rs`.

// Tests name the model `m`, the build `b`, a line `l`, as the router does.
#![allow(clippy::many_single_char_names, clippy::similar_names)]

use std::sync::atomic::AtomicBool;

use c4rizer_core::edit;
use c4rizer_core::export;
use c4rizer_core::geom::{Contact, Point, Segment};
use c4rizer_core::lint::Rule;
use c4rizer_core::model::{self, Model, ModelError, ModelErrorKind, ViewSpec};
use c4rizer_core::pipeline::{self, ViewBuild};
use c4rizer_core::place::Keep;
use c4rizer_core::route::{self, LineEnd, RouteConfig, Side};
use c4rizer_core::scene::SceneLine;
use c4rizer_core::view::{BoxId, EdgeId};

const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl"));
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/golden");

/// A shop, roles only. Relationship order: 0 customer -> web, 1 web -> db,
/// 2 web -> api, 3 api -> db, 4 admin -> web, 5 web -> bank.
const SHOP: &str = r#"        customer = person "Customer" "Buys goods."
        admin = person "Admin" "Runs the shop."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust"
            api = container "Api" "Answers calls." "Rust"
            db = container "Db" "Stores orders." "SQL" "Database"
        }
        bank = softwareSystem "Bank" "Moves money." "External"
        customer -> web "Browses" "HTTPS"
        web -> db "Reads orders" "SQL"
        web -> api "Calls" "JSON"
        api -> db "Writes orders" "SQL"
        admin -> web "Manages" "HTTPS"
        web -> bank "Charges cards" "HTTPS""#;

/// `views_body` in a workspace over `SHOP`.
fn ws(views_body: &str) -> String {
    format!("workspace {{\n    model {{\n{SHOP}\n    }}\n    views {{\n{views_body}\n    }}\n}}\n")
}

/// A container view of the shop holding `lines`, every box placed.
fn c2(lines: &str) -> String {
    ws(&format!(
        "        container shop \"c2\" \"Shop\" {{\n            include *\n            at customer 200 100\n            at admin 700 100\n            at web 200 400\n            at api 700 400\n            at db 200 800\n            at bank 700 800\n{lines}\n        }}"
    ))
}

/// The parse of `src`, or its error.
fn try_parse(src: &str) -> Result<Model, ModelError> {
    model::parse(src).map(|p| p.model)
}

/// The model of `src`, panicking with the error text.
fn parse(src: &str) -> Model {
    match try_parse(src) {
        Ok(m) => m,
        Err(e) => panic!("{e}"),
    }
}

/// The error of `src`, panicking when it parses.
fn error(src: &str) -> ModelError {
    match try_parse(src) {
        Ok(_) => panic!("expected an error"),
        Err(e) => e,
    }
}

/// The view keyed `key`.
fn view<'m>(m: &'m Model, key: &str) -> &'m ViewSpec {
    m.view(key).expect("view exists")
}

/// The build of view `key` of `m`.
fn build(m: &Model, key: &str) -> ViewBuild {
    match pipeline::build_view(m, key, &Keep::default(), &AtomicBool::new(false)) {
        Ok(b) => b,
        Err(e) => panic!("{e:?}"),
    }
}

/// The build of the `c2` view of `c2(lines)`.
fn c2_build(lines: &str) -> (Model, ViewBuild) {
    let m = parse(&c2(lines));
    let b = build(&m, "c2");
    (m, b)
}

/// The scene line drawn from the box named `from` to the box named `to`.
fn line<'b>(m: &Model, b: &'b ViewBuild, from: &str, to: &str) -> &'b SceneLine {
    let name = |id: BoxId| m.element(b.plan.boxes[id.0 as usize].element).name.value.as_str();
    b.scene
        .lines
        .iter()
        .find(|l| name(l.source) == from && name(l.destination) == to)
        .expect("line drawn")
}

/// Where `p` lies along `points` (segment index, steps into it), if on it.
fn position(points: &[Point], p: Point) -> Option<(usize, i32)> {
    points.windows(2).enumerate().find_map(|(i, w)| {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let (px, py) = (p.x - a.x, p.y - a.y);
        if dx * py != dy * px {
            return None;
        }
        let along = if dx == 0 { py * dy.signum() } else { px * dx.signum() };
        let len = dx.abs().max(dy.abs());
        (0..=len).contains(&along).then_some((i, along))
    })
}

/// The turn at each interior point of `points`, in 45° steps.
fn turns(points: &[Point]) -> Vec<(Point, u8)> {
    points
        .windows(3)
        .map(|w| {
            let d1 = Segment::new(w[0], w[1]).dir().expect("octilinear");
            let d2 = Segment::new(w[1], w[2]).dir().expect("octilinear");
            (w[1], d1.turn_steps(d2))
        })
        .collect()
}

/// Every vertex of `vertices` (px) lies on `points`, in order: each
/// one found at or after the place of the one before (a line may pass a
/// point twice).
fn passes_in_order(points: &[Point], vertices: &[(i32, i32)]) {
    let mut last = (0, 0);
    for &(x, y) in vertices {
        let p = Point::new(x, y);
        let at = (last.0..points.len().saturating_sub(1))
            .find_map(|i| {
                let (seg, along) = position(&points[i..=i + 1], p)?;
                let here = (i + seg, along);
                (here >= last).then_some(here)
            })
            .unwrap_or_else(|| panic!("({x}, {y}) not on {points:?} after {last:?}"));
        last = at;
    }
}

#[test]
fn p3_4_line_with_vertices_routed() {
    let (m0, plain) = c2_build("");
    let (m, b) = c2_build("            via web -> db 0 600");
    let l = line(&m, &b, "Web", "Db");
    assert!(l.routed, "{:?}", b.report);
    passes_in_order(&l.points, &[(0, 600)]);
    assert_ne!(l.points, line(&m0, &plain, "Web", "Db").points, "the vertex changed the line");
    assert!(b.scene.lines.iter().all(|l| l.routed), "every line still routed");
}

#[test]
fn p3_5_no_via_scene_unchanged() {
    // The four goldens: no `via` anywhere gives the phase-2 scenes.
    let m = parse(EXAMPLE);
    for key in ["C1", "C2", "C3-serving", "C3-startup"] {
        let golden = std::fs::read_to_string(format!("{GOLDEN}/{key}.svg")).expect("golden");
        assert!(export::svg(&build(&m, key).scene) == golden, "{key}: svg differs from its golden");
    }
    // A `via` in another view leaves this one as it was.
    let (_, plain) = c2_build("");
    let other = ws(
        "        container shop \"c2\" \"Shop\" {\n            include *\n            at customer 200 100\n            at admin 700 100\n            at web 200 400\n            at api 700 400\n            at db 200 800\n            at bank 700 800\n        }\n        container shop \"other\" {\n            include *\n            via web -> db 0 600\n        }",
    );
    let m = parse(&other);
    assert_eq!(build(&m, "c2").scene, plain.scene);
}

#[test]
fn p3_5_removing_vertices_returns_line_to_router() {
    let (_, plain) = c2_build("");
    let (_, b) = c2_build("            via web -> db 0 600");
    assert_ne!(b.routes, plain.routes);
    let mut plan = b.plan.clone();
    for e in &mut plan.edges {
        e.vertices.clear();
    }
    let routes = route::route(&plan, &b.routes.centres, &RouteConfig::DEFAULT, &AtomicBool::new(false)).expect("never stopped");
    assert_eq!(routes, plain.routes, "the router's own line again");
}

#[test]
fn p3_300_via_line_parsed() {
    let src = c2("            via web -> db 100 500 100 700");
    let m = parse(&src);
    let v = view(&m, "c2");
    assert_eq!(v.vias.len(), 1);
    let via = &v.vias[0];
    assert_eq!(m.element(via.source.value).name.value, "Web");
    assert_eq!(m.element(via.destination.value).name.value, "Db");
    assert_eq!(via.order, None);
    let points: Vec<(i32, i32)> = via.points.iter().map(|p| p.value).collect();
    assert_eq!(points, [(100, 500), (100, 700)]);
    assert_eq!(&src[via.span.range()], "via web -> db 100 500 100 700");
    assert_eq!(&src[via.points[1].span.range()], "100 700");
    // Negative coordinates are px like any other, multiples of 10.
    let m = parse(&c2("            via web -> db -100 500"));
    assert_eq!(view(&m, "c2").vias[0].points[0].value, (-100, 500));
    // The keyword ignores case, as every keyword does (SRS 4.1).
    let m = parse(&c2("            VIA web -> db 100 500"));
    assert_eq!(view(&m, "c2").vias.len(), 1);
}

/// A dynamic view over `SHOP`: steps 1 customer -> web, 2 web -> db,
/// 3 web -> db (the same relationship again), each box placed.
fn dynamic(lines: &str) -> String {
    ws(&format!(
        "        dynamic shop \"flow\" \"Flow\" {{\n            customer -> web \"Opens\"\n            web -> db \"Loads\"\n            web -> db \"Loads again\"\n            at customer 200 100\n            at web 200 400\n            at db 200 800\n{lines}\n        }}"
    ))
}

#[test]
fn p3_301_dynamic_via_names_step() {
    let m = parse(&dynamic("            via 3: web -> db 400 500 400 700\n            via web -> db 0 500 0 700"));
    let v = view(&m, "flow");
    assert_eq!(v.vias[0].order.as_ref().map(|o| o.value.as_str()), Some("3"));
    assert_eq!(v.vias[1].order, None);
    let plan = c4rizer_core::view::plan(&m, v);
    let of = |order: &str| {
        plan.edges
            .iter()
            .find(|e| e.order.as_deref() == Some(order))
            .expect("edge")
            .vertices
            .clone()
    };
    assert_eq!(of("3"), [Point::new(40, 50), Point::new(40, 70)], "`via 3:` names step 3");
    assert_eq!(of("2"), [Point::new(0, 50), Point::new(0, 70)], "the plain form names the lowest-numbered step");
    assert!(of("1").is_empty());
    // The two forms naming the same step are two `via` lines for one line.
    let e = error(&dynamic("            via 2: web -> db 0 500\n            via web -> db 0 600"));
    assert!(matches!(e.kind, ModelErrorKind::DuplicateVia { .. }), "{e}");
    // A step number the view does not have names no drawn line.
    let e = error(&dynamic("            via 9: web -> db 0 500"));
    assert!(matches!(e.kind, ModelErrorKind::NotInView { .. }), "{e}");
    // `<n>:` is refused outside a dynamic view.
    let e = error(&c2("            via 1: web -> db 100 500"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e}");
}

#[test]
fn p3_302_via_errors() {
    // A line the view does not draw: no relationship from db to web.
    let e = error(&c2("            via db -> web 100 500"));
    match &e.kind {
        ModelErrorKind::NotInView { id, view } => {
            assert_eq!(id, "db -> web");
            assert_eq!(view, "c2");
        }
        other => panic!("{other:?}"),
    }
    assert!(e.to_string().contains("db -> web"), "{e}");
    // A hidden line is not drawn either.
    let e = error(&c2("            hide web -> db\n            via web -> db 100 500"));
    assert!(matches!(e.kind, ModelErrorKind::NotInView { .. }), "{e}");
    // An odd number of coordinates names the lone value.
    let e = error(&c2("            via web -> db 100 500 120"));
    match &e.kind {
        ModelErrorKind::InvalidValue { value, .. } => assert_eq!(value, "120"),
        other => panic!("{other:?}"),
    }
    // A coordinate off the 10 px grid names the value.
    let e = error(&c2("            via web -> db 105 500"));
    match &e.kind {
        ModelErrorKind::InvalidValue { value, .. } => assert_eq!(value, "105"),
        other => panic!("{other:?}"),
    }
    let e = error(&c2("            via web -> db 100 x"));
    assert!(matches!(&e.kind, ModelErrorKind::InvalidValue { value, .. } if value == "x"), "{e}");
    // No coordinate at all.
    let e = error(&c2("            via web -> db"));
    assert!(matches!(e.kind, ModelErrorKind::MissingArgument { .. }), "{e}");
    // Two `via` lines for one line name both.
    let src = c2("            via web -> db 100 500\n            via web -> db 100 600");
    let e = error(&src);
    match &e.kind {
        ModelErrorKind::DuplicateVia { line, first, .. } => {
            assert_eq!(line, "web -> db");
            assert_eq!(&src[first.range()], "via web -> db 100 500");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(&src[e.span.range()], "via web -> db 100 600");
    assert!(e.to_string().contains("web -> db"), "{e}");
}

#[test]
fn p3_4_plan_edge_carries_vertices() {
    let m = parse(&c2("            via web -> db 100 500 -100 700"));
    let plan = c4rizer_core::view::plan(&m, view(&m, "c2"));
    let element = |b: c4rizer_core::view::BoxId| m.element(plan.boxes[b.0 as usize].element).name.value.clone();
    for e in &plan.edges {
        if element(e.source) == "Web" && element(e.destination) == "Db" {
            assert_eq!(e.vertices, [Point::new(10, 50), Point::new(-10, 70)], "grid units, in order");
        } else {
            assert!(e.vertices.is_empty(), "only the named line carries vertices");
        }
    }
}

#[test]
fn p3_303_legs_octilinear_on_grid() {
    let (m, b) = c2_build("            via web -> db 0 600 450 650");
    let l = line(&m, &b, "Web", "Db");
    assert!(l.routed);
    for p in &l.points {
        assert!(p.x % 10 == 0 && p.y % 10 == 0, "{p:?} off the grid");
    }
    for w in l.points.windows(2) {
        let s = Segment::new(w[0], w[1]);
        assert!(s.dir().is_some(), "{s:?} not at 0, 45 or 90 degrees");
        assert!(s.len_px_squared().is_some_and(|n| n >= 400), "{s:?} shorter than 20px");
    }
    assert!(!b.report.findings.iter().any(|f| f.rule == Rule::SegmentAngle));
}

#[test]
fn p3_304_line_passes_through_vertices() {
    for vias in [
        &[(0, 600)][..],
        &[(0, 600), (450, 650)],
        &[(450, 400), (450, 800)],
        &[(-100, 300), (-100, 900), (200, 1000)],
    ] {
        let text: Vec<String> = vias.iter().map(|(x, y)| format!("{x} {y}")).collect();
        let (m, b) = c2_build(&format!("            via web -> db {}", text.join(" ")));
        let l = line(&m, &b, "Web", "Db");
        assert!(l.routed, "{vias:?}");
        passes_in_order(&l.points, vias);
    }
}

#[test]
fn p3_305_sharp_turn_only_at_vertex() {
    let vias = [(200, 600), (650, 600)];
    let (m, b) = c2_build("            via web -> api 200 600 650 600");
    let l = line(&m, &b, "Web", "Api");
    assert!(l.routed);
    passes_in_order(&l.points, &vias);
    let mut sharp = 0;
    for (at, steps) in turns(&l.points) {
        assert!(steps < 4, "the line never doubles back, at {at:?}");
        if steps > 1 {
            assert!(vias.contains(&(at.x, at.y)), "a turn over 45 degrees at {at:?}, not a vertex");
            sharp += 1;
        }
    }
    assert!(sharp > 0, "the square corners asked for are drawn: {:?}", l.points);
    // Every other line bends by 45 degrees only (CORE-6).
    for other in b.scene.lines.iter().filter(|o| o.edge != l.edge) {
        assert!(turns(&other.points).iter().all(|t| t.1 == 1), "{:?}", other.points);
    }
}

#[test]
fn p3_306_ports_face_first_and_last_vertex() {
    let (m, b) = c2_build("            via web -> db 450 400 450 800");
    let l = line(&m, &b, "Web", "Db");
    let r = b.routes.line(l.edge).expect("routed");
    assert_eq!(r.source.side, Side::E, "v1 lies east of web");
    match r.end {
        LineEnd::Arrow(port) => assert_eq!(port.side, Side::E, "vk lies east of db"),
        other => panic!("{other:?}"),
    }
    let (m, b) = c2_build("            via web -> db -100 400 -100 800");
    let r = b.routes.line(line(&m, &b, "Web", "Db").edge).expect("routed");
    assert_eq!(r.source.side, Side::W);
    assert!(matches!(r.end, LineEnd::Arrow(p) if p.side == Side::W));
}

#[test]
fn p3_307_other_lines_avoid_vertex_legs() {
    // The vertex line runs down between web and api, across web -> api.
    let (m, b) = c2_build("            via web -> db 450 400 450 800");
    let v = line(&m, &b, "Web", "Db");
    assert!(b.scene.lines.iter().all(|l| l.routed));
    let segs = |l: &SceneLine| l.points.windows(2).map(|w| Segment::new(w[0], w[1])).collect::<Vec<_>>();
    for other in b.scene.lines.iter().filter(|o| o.edge != v.edge) {
        for s in segs(other) {
            for t in segs(v) {
                assert_ne!(s.contact(&t), Contact::Overlap, "{s:?} runs along the vertex leg {t:?}");
            }
        }
    }
    // The vertex line keeps its route whatever else the view draws: it is
    // routed first, the others around it.
    let r = b.routes.line(v.edge).expect("routed").clone();
    let alone = ws("        container shop \"c2\" {\n            include web db\n            at web 200 400\n            at db 200 800\n            via web -> db 450 400 450 800\n        }");
    let m2 = parse(&alone);
    let b2 = build(&m2, "c2");
    let r2 = b2.routes.lines.iter().flatten().next().expect("one line");
    assert_eq!(r.points, r2.points);
}

#[test]
fn p3_308_vertex_line_never_trunk_or_joined() {
    // Without vertices, api -> db joins web -> db with `bus on`.
    let (_, plain) = c2_build("            bus on");
    assert!(plain.routes.lines.iter().flatten().any(|l| l.joins().is_some()), "the fixture joins");
    for via in ["via web -> db -100 600", "via api -> db 900 600"] {
        let (m, b) = c2_build(&format!("            bus on\n            {via}"));
        let pinned: Vec<EdgeId> = b.plan.edges.iter().filter(|e| !e.vertices.is_empty()).map(|e| e.id).collect();
        assert_eq!(pinned.len(), 1);
        for l in b.routes.lines.iter().flatten() {
            if pinned.contains(&l.edge) {
                assert!(l.joins().is_none(), "{via}: a vertex line never joins");
            }
            assert!(l.joins().is_none_or(|t| !pinned.contains(&t)), "{via}: never a trunk");
        }
        let _ = m;
    }
}

#[test]
fn p3_309_l28_vertex_in_box_or_repeated() {
    let l28 = |b: &ViewBuild| -> Vec<c4rizer_core::lint::Finding> {
        b.report.findings.iter().filter(|f| f.rule == Rule::VertexPlacement).cloned().collect()
    };
    let (_, clean) = c2_build("            via web -> db 0 600 450 650");
    assert!(l28(&clean).is_empty(), "{:?}", l28(&clean));
    // (700, 400) lies inside api; (0, 600) twice in a row.
    let src = c2("            via web -> db 700 400 0 600 0 600");
    let m = parse(&src);
    let b = build(&m, "c2");
    let found = l28(&b);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(Rule::VertexPlacement.code(), "L28");
    assert_eq!(Rule::VertexPlacement.severity(), c4rizer_core::lint::Severity::Warning);
    assert!(found[0].message.contains("Api"), "{}", found[0].message);
    assert_eq!(&src[found[0].spans[0].range()], "700 400");
    assert_eq!(&src[found[1].spans[0].range()], "0 600");
    assert_eq!(found[1].spans[0].range().start, src.rfind("0 600").expect("written"), "the repeat is named");
    let line = line(&m, &b, "Web", "Db");
    assert!(found.iter().all(|f| f.items == [c4rizer_core::scene::ItemKey::Line(line.edge)]));
}

/// `edits` applied to `src`: the new text and its inverse.
fn applied(src: &str, edits: &[edit::Edit]) -> edit::Applied {
    let parsed = model::parse(src).unwrap_or_else(|e| panic!("{e}"));
    edit::apply(&parsed, edits).unwrap_or_else(|e| panic!("{e:?}"))
}

/// `SetVia` on view `c2` for `src -> dst`.
fn set_via(src: &str, dst: &str, points: Option<Vec<(i32, i32)>>) -> edit::Edit {
    edit::Edit::SetVia {
        view: "c2".to_owned(),
        source: src.to_owned(),
        destination: dst.to_owned(),
        order: None,
        points,
    }
}

#[test]
fn p3_310_set_via_adds_after_last_at() {
    let src = c2("");
    let done = applied(&src, &[set_via("web", "db", Some(vec![(0, 600), (450, 650)]))]);
    assert!(
        done.text.contains("            at bank 700 800\n            via web -> db 0 600 450 650\n"),
        "{}",
        done.text
    );
    let m = parse(&done.text);
    assert_eq!(view(&m, "c2").vias.len(), 1);
    // The inverse removes it again, byte for byte.
    assert_eq!(applied(&done.text, &done.inverse).text, src);
    // A second line's `via` goes with the first.
    let two = applied(&done.text, &[set_via("api", "db", Some(vec![(900, 600)]))]);
    assert!(two.text.contains("via web -> db 0 600 450 650\n            via api -> db 900 600\n"), "{}", two.text);
    // A dynamic step by number.
    let src = dynamic("");
    let done = applied(
        &src,
        &[edit::Edit::SetVia {
            view: "flow".to_owned(),
            source: "web".to_owned(),
            destination: "db".to_owned(),
            order: Some("3".to_owned()),
            points: Some(vec![(400, 600)]),
        }],
    );
    assert!(done.text.contains("            at db 200 800\n            via 3: web -> db 400 600\n"), "{}", done.text);
    // A line the view does not draw is refused; the text is untouched.
    let parsed = model::parse(&c2("")).expect("parses");
    assert!(edit::apply(&parsed, &[set_via("db", "web", Some(vec![(0, 600)]))]).is_err());
}

#[test]
fn p3_312_set_via_rewrites_or_removes_line() {
    let src = c2("            via web -> db 0 600");
    // Rewrite: only the points change, the line stays where it is.
    let done = applied(&src, &[set_via("web", "db", Some(vec![(0, 600), (0, 700)]))]);
    assert_eq!(done.text, src.replace("via web -> db 0 600", "via web -> db 0 600 0 700"));
    assert_eq!(done.inverse, [set_via("web", "db", Some(vec![(0, 600)]))]);
    assert_eq!(applied(&done.text, &done.inverse).text, src);
    // Remove: `None` and an empty list both delete the line (P3-5).
    for points in [None, Some(Vec::new())] {
        let gone = applied(&src, &[set_via("web", "db", points)]);
        assert_eq!(gone.text, src.replace("            via web -> db 0 600\n", ""), "the via line and its line ending go");
        assert_eq!(gone.inverse, [set_via("web", "db", Some(vec![(0, 600)]))]);
        assert_eq!(applied(&gone.text, &gone.inverse).text, src);
    }
    // Removing what is not there changes nothing; its inverse is a removal.
    let none = applied(&c2(""), &[set_via("web", "db", None)]);
    assert_eq!(none.text, c2(""));
    assert_eq!(none.inverse, [set_via("web", "db", None)]);
    // EDIT-21: no element or relationship is created or deleted.
    let before = parse(&src);
    let after = parse(&done.text);
    assert_eq!(before.elements.len(), after.elements.len());
    assert_eq!(before.relationships.len(), after.relationships.len());
}

#[test]
fn p3_316_export_draws_scene_lines() {
    let (m, b) = c2_build("            via web -> db 0 600");
    let l = line(&m, &b, "Web", "Db");
    // redesign 2026-11-19: the route is drawn with rounded corners, so
    // the SVG holds its start and each turn as a corner's control point
    // (was the plain polyline).
    let svg = export::svg(&b.scene);
    let mut want = vec![format!("d=\"M{} {} ", l.points[0].x, l.points[0].y)];
    want.extend(l.points.windows(3).filter(|w| {
        (w[1].x - w[0].x) * (w[2].y - w[1].y) != (w[1].y - w[0].y) * (w[2].x - w[1].x)
    }).map(|w| format!("Q{} {} ", w[1].x, w[1].y)));
    for w in &want {
        assert!(svg.contains(w.as_str()), "the SVG draws the scene's line: {w} of {:?}", l.points);
    }
    // PNG is the SVG rasterized (P2-436): same scene, same line.
    #[cfg(feature = "png")]
    {
        // The PNG draws the bent line: pixels at the vertex and along its
        // first leg are line-coloured with the vertex, page-white without.
        let (_, plain) = c2_build("");
        let ink = |scene: &c4rizer_core::scene::Scene, p: Point| -> bool {
            let width = u32::try_from(scene.bounds.width()).expect("width");
            let png = export::png(scene, width).expect("png");
            let pixmap = resvg::tiny_skia::Pixmap::decode_png(&png).expect("decodes");
            let (x0, y0) = (p.x - scene.bounds.min.x, p.y - scene.bounds.min.y);
            (-1..=1).any(|dy| {
                (-1..=1).any(|dx| {
                    let (x, y) = (u32::try_from(x0 + dx), u32::try_from(y0 + dy));
                    matches!((x, y), (Ok(x), Ok(y)) if pixmap.pixel(x, y).is_some_and(|c| (c.red(), c.green(), c.blue()) != (255, 255, 255)))
                })
            })
        };
        // The label sits on the vertex (its middle), so sample the middle
        // of each leg into and out of it.
        let vertex = Point::new(0, 600);
        let at = l.points.iter().position(|p| *p == vertex).expect("the vertex is a bend");
        for other in [l.points[at - 1], l.points[at + 1]] {
            let mid = Point::new(i32::midpoint(other.x, vertex.x), i32::midpoint(other.y, vertex.y));
            assert!(ink(&b.scene, mid), "the leg through {mid:?} is drawn");
            assert!(!ink(&plain.scene, mid), "nothing is drawn at {mid:?} without the vertex");
        }
    }
}

/// A Structurizr JSON for the shop's `c2` view: every box where `c2`
/// puts it, the `web -> db` line with two vertices off the 10px grid, and
/// `db -> web` (not drawn by the view) with one.
const SHOP_JSON: &str = r#"{"model": {
    "people": [{"id": "1", "name": "Customer", "relationships": [{"id": "r1", "sourceId": "1", "destinationId": "4"}]},
               {"id": "2", "name": "Admin"}],
    "softwareSystems": [
      {"id": "3", "name": "Shop", "containers": [
        {"id": "4", "name": "Web", "relationships": [{"id": "r2", "sourceId": "4", "destinationId": "6"}]},
        {"id": "5", "name": "Api"},
        {"id": "6", "name": "Db", "relationships": [{"id": "r9", "sourceId": "6", "destinationId": "4"}]}]},
      {"id": "7", "name": "Bank"}]},
  "views": {
    "containerViews": [{"key": "c2",
      "elements": [{"id": "1", "x": 0, "y": 0}, {"id": "4", "x": 0, "y": 400}, {"id": "6", "x": 0, "y": 800}],
      "relationships": [{"id": "r1"}, {"id": "r2", "vertices": [{"x": 4, "y": 596}, {"x": 455, "y": 644}]},
                        {"id": "r9", "vertices": [{"x": 900, "y": 600}]}]}]
  }
}"#;

#[test]
fn p3_317_import_layout_copies_vertices() {
    use c4rizer_core::import;
    use c4rizer_core::model::NoticeKind;
    let src = c2("");
    let parsed = model::parse(&src).expect("parses");
    let imported = import::import_layout(&parsed, SHOP_JSON).expect("imports");
    let vias: Vec<&edit::Edit> = imported.edits.iter().filter(|e| matches!(e, edit::Edit::SetVia { .. })).collect();
    assert_eq!(vias, [&set_via("web", "db", Some(vec![(0, 600), (460, 640)]))], "rounded to the nearest 10px");
    // The vertices of a line the view does not draw stay a P2-510 notice.
    assert!(
        imported.notices.iter().any(|n| n.kind == NoticeKind::NotImported && n.subject == "vertices"),
        "{:?}",
        imported.notices
    );
    let text = edit::apply(&parsed, &imported.edits).expect("applies").text;
    let m = parse(&text);
    assert_eq!(view(&m, "c2").vias.len(), 1, "{text}");
    // All vertices matched: no notice.
    let matched = SHOP_JSON.replace(r#"{"id": "r9", "vertices": [{"x": 900, "y": 600}]}"#, r#"{"id": "r9"}"#);
    let imported = import::import_layout(&parsed, &matched).expect("imports");
    assert!(!imported.notices.iter().any(|n| n.subject == "vertices"), "{:?}", imported.notices);
}

/// Every relationship-view `vertices` array of an exported workspace
/// JSON, in file order, as `(x, y)` lists (`automaticLayout`'s boolean
/// `vertices` is skipped).
fn json_vertices(json: &str) -> Vec<Vec<(i32, i32)>> {
    let mut out = Vec::new();
    let mut rest = json;
    while let Some(at) = rest.find("\"vertices\" : [") {
        rest = &rest[at + "\"vertices\" : [".len()..];
        let end = rest.find(']').expect("closed");
        let numbers: Vec<i32> = rest[..end]
            .split(|c: char| !(c.is_ascii_digit() || c == '-'))
            .filter(|t| !t.is_empty())
            .map(|t| t.parse().expect("number"))
            .collect();
        // Keys are sorted: each vertex is written `x` then `y`.
        out.push(numbers.chunks(2).map(|c| (c[0], c[1])).collect());
        rest = &rest[end..];
    }
    out
}

/// The workspace JSON (P3-413) of `src`.
fn json_of(src: &str) -> String {
    let m = parse(src);
    let all = pipeline::build_all(&m, &AtomicBool::new(false)).expect("never stopped");
    export::json(&m, &all, Some(src))
}

#[test]
fn p3_318_json_export_writes_vertices() {
    // The vertices the user set, in order: not the router's bends.
    let json = json_of(&c2("            via web -> db 0 600 450 650"));
    assert_eq!(json_vertices(&json), [vec![(0, 600), (450, 650)]], "only the line with a via line carries vertices");
    // A view without via lines writes no vertices, bent lines or not.
    let json = json_of(&c2(""));
    assert!(json_vertices(&json).is_empty(), "the router's bends are not vertices");
    // A dynamic step's vertices go on its relationship view (by order).
    let json = json_of(&dynamic("            via 3: web -> db 400 600"));
    assert_eq!(json_vertices(&json), [vec![(400, 600)]]);
}

#[test]
fn p3_311_vertex_drag_preview() {
    let never = AtomicBool::new(false);
    let (m, b) = c2_build("            via web -> db 0 600");
    let edge = line(&m, &b, "Web", "Db").edge;
    let config = pipeline::PREVIEW_ROUTE;
    // The vertex dragged to (-100, 600) px: the line passes there.
    let moved = pipeline::rebuild_with_vertices(&m, &b, edge, &[Point::new(-10, 60)], &config, &never).expect("never stopped");
    passes_in_order(&line(&m, &moved, "Web", "Db").points, &[(-100, 600)]);
    assert_eq!(moved.plan.edges[edge.0 as usize].vertices, [Point::new(-10, 60)]);
    assert_eq!(moved.routes.centres, b.routes.centres, "no box moves");
    // The same vertices, or an edge out of range, give `previous`.
    let same = pipeline::rebuild_with_vertices(&m, &b, edge, &[Point::new(0, 60)], &config, &never).expect("never stopped");
    assert_eq!(same.scene, b.scene);
    let out = pipeline::rebuild_with_vertices(&m, &b, EdgeId(99), &[], &config, &never).expect("never stopped");
    assert_eq!(out.scene, b.scene);
    // No vertices: the line is the router's again.
    let (m0, plain) = c2_build("");
    let cleared = pipeline::rebuild_with_vertices(&m, &b, edge, &[], &config, &never).expect("never stopped");
    assert!(cleared.plan.edges.iter().all(|e| e.vertices.is_empty()));
    assert!(position(&line(&m, &cleared, "Web", "Db").points, Point::new(0, 600)).is_none());
    let _ = (m0, plain);
    // A set stop flag yields nothing.
    let stop = AtomicBool::new(true);
    assert!(pipeline::rebuild_with_vertices(&m, &b, edge, &[Point::new(-10, 60)], &config, &stop).is_err());
}

/// A swap of two steps (P3-219) takes each step's `via <n>:` line with
/// it: the numbers follow the statements, in one edit, undone by its
/// inverse.
#[test]
fn p3_301_swap_steps_renumbers_via_lines() {
    let swap = |first| edit::Edit::SwapSteps {
        view: "flow".to_owned(),
        first,
    };
    // Same ends: without renumbering, the vertices would move to the
    // other statement.
    let src = dynamic("            via 3: web -> db 400 600");
    let done = applied(&src, &[swap(1)]);
    assert!(done.text.contains("via 2: web -> db 400 600"), "{}", done.text);
    let m = parse(&done.text);
    let v = view(&m, "flow");
    let plan = c4rizer_core::view::plan(&m, v);
    let again = plan
        .edges
        .iter()
        .find(|e| !e.vertices.is_empty())
        .expect("one line keeps the vertices");
    let step = &v.steps[1];
    assert_eq!(again.order.as_deref(), Some(step.order.as_str()));
    assert_eq!(step.description.as_ref().map(|d| d.value.as_str()), Some("Loads again"), "the vertices stay with their statement");
    assert_eq!(applied(&done.text, &done.inverse).text, src, "undo gives the bytes back");
    // Different ends: without renumbering, the swap would be refused.
    let src = ws(
        "        dynamic shop \"flow\" \"Flow\" {\n            customer -> web \"Opens\"\n            web -> db \"Loads\"\n            web -> api \"Asks\"\n            at customer 200 100\n            at web 200 400\n            at db 200 800\n            at api 700 400\n            via 3: web -> api 450 300\n        }",
    );
    let done = applied(&src, &[swap(1)]);
    assert!(done.text.contains("via 2: web -> api 450 300"), "{}", done.text);
    assert_eq!(applied(&done.text, &done.inverse).text, src);
}

/// A vertex lying on the line's own earlier leg is still reached: the
/// line crosses itself there rather than being reported unroutable.
#[test]
fn p3_304_vertex_on_own_earlier_leg() {
    let vias = [(200, 600), (200, 500), (200, 700)];
    let (m, b) = c2_build("            via web -> db 200 600 200 500 200 700");
    let l = line(&m, &b, "Web", "Db");
    assert!(l.routed, "{:?}", b.report.findings.iter().map(|f| &f.message).collect::<Vec<_>>());
    passes_in_order(&l.points, &vias);
    assert!(!b.report.findings.iter().any(|f| f.rule == Rule::Unroutable));
}


/// P3-309, second sentence: L1..L4 and L23 count lines with vertices
/// like any other line: the same findings whether lint is told the lines
/// have vertices or not, and real L1 and L4 findings on vertex lines.
#[test]
fn p3_309_vertex_lines_count_like_any_other() {
    let rules = [Rule::Unroutable, Rule::SegmentAngle, Rule::ThroughBox, Rule::Crossings, Rule::LabelOverLine];
    let of = |r: &c4rizer_core::lint::Report| -> Vec<(Rule, String)> {
        r.findings.iter().filter(|f| rules.contains(&f.rule)).map(|f| (f.rule, f.message.clone())).collect()
    };
    // L4: api -> db must cross web -> db to reach its vertex, which lies
    // in the region web -> db's own vertex closes off.
    let crossing = "            maxCrossings 0\n            via web -> db 600 600\n            via api -> db 400 600";
    // L1: a vertex inside a box leaves its line unroutable.
    let boxed = "            via web -> db 700 400";
    for lines in [crossing, boxed] {
        let (_, b) = c2_build(lines);
        let without: String = lines.lines().filter(|l| !l.trim_start().starts_with("via ")).collect::<Vec<_>>().join("\n");
        let plain_model = parse(&c2(&without));
        let relinted = c4rizer_core::lint::lint(&plain_model, &b.scene);
        assert_eq!(of(&b.report), of(&relinted), "{lines}: lint treats vertex lines like the rest");
    }
    let (_, b) = c2_build(crossing);
    assert_eq!(b.scene.crossings, 1, "the crossing of two vertex lines is counted");
    let l4: Vec<_> = b.report.findings.iter().filter(|f| f.rule == Rule::Crossings).collect();
    assert_eq!(l4.len(), 1);
    assert!(l4[0].message.contains("\"Web\"") && l4[0].message.contains("\"Api\""), "{}", l4[0].message);
    let (m, b) = c2_build(boxed);
    let web_db = line(&m, &b, "Web", "Db").edge;
    assert!(
        b.report
            .findings
            .iter()
            .any(|f| f.rule == Rule::Unroutable && f.items == [c4rizer_core::scene::ItemKey::Line(web_db)]),
        "{:?}",
        b.report.findings
    );
}

/// P3-5 through the edit the editor writes: removing the `via` line with
/// `SetVia` gives the router's own line back.
#[test]
fn p3_5_set_via_removal_returns_line_to_router() {
    let src = c2("            via web -> db 0 600");
    let done = applied(&src, &[set_via("web", "db", None)]);
    let m = parse(&done.text);
    let after = build(&m, "c2");
    let (_, plain) = c2_build("");
    assert_eq!(after.routes, plain.routes);
    assert_eq!(after.scene.lines, plain.scene.lines);
}

/// A swap inside a parallel block renumbers only the `via` lines of the
/// two swapped steps: another step with the same number keeps its line.
#[test]
fn p3_301_swap_in_parallel_block_renumbers_only_its_steps() {
    let swap = |first| edit::Edit::SwapSteps {
        view: "p".to_owned(),
        first,
    };
    let flow = |vias: &str| {
        ws(&format!(
            "        dynamic shop \"p\" \"Parallel\" {{\n            customer -> web \"Opens\"\n            {{\n                web -> db \"Loads\"\n                web -> api \"Asks\"\n            }}\n            api -> db \"Writes\"\n            at customer 200 100\n            at web 200 400\n            at db 200 800\n            at api 700 400\n{vias}\n        }}"
        ))
    };
    let orders = |src: &str| -> Vec<String> { view(&parse(src), "p").steps.iter().map(|s| s.order.clone()).collect() };
    // The probe: the step outside the block shares a number with one inside.
    let src = flow("            via 2: api -> db 900 600");
    let before = orders(&src);
    assert_eq!(before[1], before[3], "the fixture repeats a number: {before:?}");
    let done = applied(&src, &[swap(1)]);
    assert!(done.text.contains("via 2: api -> db 900 600"), "the outside step's via is untouched: {}", done.text);
    assert_eq!(applied(&done.text, &done.inverse).text, src, "undo is byte-exact");
    // A swapped step inside the block carries its via with it.
    let (upper, lower) = (&before[1], &before[2]);
    let src = flow(&format!("            via {lower}: web -> api 450 300\n            via 2: api -> db 900 600"));
    let done = applied(&src, &[swap(1)]);
    assert!(done.text.contains(&format!("via {upper}: web -> api 450 300")), "{}", done.text);
    assert!(done.text.contains("via 2: api -> db 900 600"), "{}", done.text);
    let m = parse(&done.text);
    let v = view(&m, "p");
    let plan = c4rizer_core::view::plan(&m, v);
    let carried = plan.edges.iter().find(|e| e.vertices == [Point::new(45, 30)]).expect("web -> api keeps its vertex");
    let step = v.steps.iter().find(|s| s.order == carried.order.clone().unwrap_or_default() && m.element(s.destination.value).name.value == "Api").expect("step");
    assert_eq!(step.description.as_ref().map(|d| d.value.as_str()), Some("Asks"));
    assert_eq!(applied(&done.text, &done.inverse).text, src);
}

/// One `via` line as written: source name, destination name, step
/// number, points.
type ViaLine = (String, String, Option<String>, Vec<(i32, i32)>);

/// The `via` lines of view `key`.
fn vias_of(m: &Model, key: &str) -> Vec<ViaLine> {
    view(m, key)
        .vias
        .iter()
        .map(|v| {
            (
                m.element(v.source.value).name.value.clone(),
                m.element(v.destination.value).name.value.clone(),
                v.order.as_ref().map(|o| o.value.clone()),
                v.points.iter().map(|p| p.value).collect(),
            )
        })
        .collect()
}

/// The model of a workspace JSON text (P3-416).
fn from_json(json: &str) -> Model {
    model::parse_json(json.as_bytes(), &mut model::FsLoader).unwrap_or_else(|e| panic!("{e}"))
}

/// JSON out then JSON in keeps every line's hand-added vertices as `via`
/// lines (P3-318, P3-416), so the views draw the same lines (P3-417).
#[test]
fn p3_318_json_round_trip_keeps_via_lines() {
    let src = c2("            via web -> db 0 600 450 650\n            via api -> db 900 600");
    let m = parse(&src);
    let back = from_json(&json_of(&src));
    assert_eq!(vias_of(&back, "c2"), vias_of(&m, "c2"));
    assert_eq!(build(&back, "c2").scene.lines, build(&m, "c2").scene.lines, "the same picture");
    let src = dynamic("            via 3: web -> db 400 600");
    let m = parse(&src);
    let back = from_json(&json_of(&src));
    assert_eq!(vias_of(&back, "flow"), vias_of(&m, "flow"));
    assert_eq!(build(&back, "flow").scene.lines, build(&m, "flow").scene.lines);
}

/// Hostile vertices in a workspace JSON never fail the read or panic:
/// off-grid points snap to 10 px, out-of-range or malformed ones and
/// vertices of lines the view does not draw are dropped.
#[test]
fn p3_318_json_in_hostile_vertices() {
    let json = |relationships: &str| {
        format!(
            r#"{{"model": {{"softwareSystems": [{{"id": "1", "name": "A", "relationships": [{{"id": "11", "sourceId": "1", "destinationId": "2"}}, {{"id": "13", "sourceId": "1", "destinationId": "3"}}]}}, {{"id": "2", "name": "B"}}, {{"id": "3", "name": "C"}}]}},
  "views": {{"systemLandscapeViews": [{{"key": "l", "elements": [{{"id": "1", "x": 0, "y": 0}}, {{"id": "2", "x": 0, "y": 600}}],
    "relationships": [{relationships}]}}]}}}}"#
        )
    };
    let read = |rels: &str| from_json(&json(rels));
    let points = |m: &Model| vias_of(m, "l").into_iter().map(|v| v.3).collect::<Vec<_>>();
    // Off the grid: snapped, as import-layout does (P3-317).
    assert_eq!(points(&read(r#"{"id": "11", "vertices": [{"x": 604, "y": 296}]}"#)), [vec![(600, 300)]]);
    for bad in [
        r#"{"id": "11", "vertices": [{"x": 1e12, "y": 0}]}"#,
        r#"{"id": "11", "vertices": [{"x": "600", "y": 300}]}"#,
        r#"{"id": "11", "vertices": [{"y": 300}]}"#,
        r#"{"id": "11", "vertices": [{"x": 2147483647, "y": 2147483647}]}"#,
        r#"{"id": "11", "vertices": "none"}"#,
        r#"{"id": "99", "vertices": [{"x": 600, "y": 300}]}"#,
    ] {
        let m = read(bad);
        assert!(points(&m).is_empty(), "{bad}: dropped, the workspace still reads");
    }
    // A line the view does not draw (C is not in the view): dropped.
    assert!(points(&read(r#"{"id": "13", "vertices": [{"x": 600, "y": 300}]}"#)).is_empty());
    // Two relationship views on one drawn line: the first wins, no
    // DuplicateVia.
    let m = read(r#"{"id": "11", "vertices": [{"x": 600, "y": 300}]}, {"id": "11", "vertices": [{"x": 700, "y": 300}]}"#);
    assert_eq!(points(&m), [vec![(600, 300)]]);
}

/// A line lifted from a component relationship keeps its `via` line
/// through JSON: the generated line names the ends the view draws
/// (`web -> db`), not the relationship's own (`cart -> db`).
#[test]
fn p3_318_json_round_trip_keeps_via_on_lifted_line() {
    let src = "workspace {\n    model {\n        shop = softwareSystem \"Shop\" {\n            web = container \"Web\" {\n                cart = component \"Cart\"\n            }\n            db = container \"Db\"\n        }\n        cart -> db \"Reads\"\n    }\n    views {\n        container shop \"c2\" \"Shop\" {\n            include *\n            at web 200 400\n            at db 200 800\n            via web -> db 200 600\n        }\n    }\n}\n";
    let m = parse(src);
    assert_eq!(vias_of(&m, "c2").len(), 1);
    let json = json_of(src);
    assert_eq!(json_vertices(&json), [vec![(200, 600)]]);
    let back = from_json(&json);
    assert_eq!(vias_of(&back, "c2"), vias_of(&m, "c2"), "{json}");
    assert_eq!(build(&back, "c2").scene.lines, build(&m, "c2").scene.lines);
}

/// A workspace JSON of systems A (1) and B (2) with two good vertex lines
/// (`A -> B`, `B -> A`), `bad` vertex lines to systems the view does not
/// draw; A carries the DSL identifier `identifier`.
fn many_bad_vias_json(bad: usize, identifier: &str) -> String {
    use std::fmt::Write as _;
    let mut systems = String::new();
    let mut rels = String::from(r#"{"id": "11", "sourceId": "1", "destinationId": "2"}"#);
    let mut view_rels = String::from(r#"{"id": "11", "vertices": [{"x": 600, "y": 300}]}, {"id": "12", "vertices": [{"x": 700, "y": 300}]}"#);
    for i in 0..bad {
        let (id, rid) = (100 + i, 200 + i);
        let _ = write!(systems, r#", {{"id": "{id}", "name": "S{i}"}}"#);
        let _ = write!(rels, r#", {{"id": "{rid}", "sourceId": "1", "destinationId": "{id}"}}"#);
        let _ = write!(view_rels, r#", {{"id": "{rid}", "vertices": [{{"x": 600, "y": 300}}]}}"#);
    }
    format!(
        r#"{{"model": {{"softwareSystems": [{{"id": "1", "name": "A", "properties": {{"structurizr.dsl.identifier": "{identifier}"}}, "relationships": [{rels}]}}, {{"id": "2", "name": "B", "relationships": [{{"id": "12", "sourceId": "2", "destinationId": "1"}}]}}{systems}]}},
  "views": {{"systemLandscapeViews": [{{"key": "l", "elements": [{{"id": "1", "x": 0, "y": 0}}, {{"id": "2", "x": 0, "y": 600}}], "relationships": [{view_rels}]}}]}}}}"#
    )
}

/// Past the one-by-one retries, only the refused `via` lines go: the
/// good ones of the same view stay.
#[test]
fn p3_318_json_in_many_bad_vias_keep_good_ones() {
    for bad in [3, 20, 60] {
        let m = from_json(&many_bad_vias_json(bad, "a"));
        let points: Vec<_> = vias_of(&m, "l").into_iter().map(|v| (v.0, v.1, v.3)).collect();
        assert_eq!(
            points,
            [("A".to_owned(), "B".to_owned(), vec![(600, 300)]), ("B".to_owned(), "A".to_owned(), vec![(700, 300)])],
            "{bad} bad lines"
        );
    }
}

/// An element identified `via` is not a `via` line: its relationships
/// survive the dropping of many refused lines.
#[test]
fn p3_318_json_in_element_named_via_is_not_a_via_line() {
    let m = from_json(&many_bad_vias_json(20, "via"));
    assert_eq!(m.relationships.len(), 22, "every relationship survives");
    let points: Vec<_> = vias_of(&m, "l").into_iter().map(|v| v.3).collect();
    assert_eq!(points, [vec![(600, 300)], vec![(700, 300)]]);
}
