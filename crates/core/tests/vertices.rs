//! Tests of the vertices row (SRS 4.7: P3-4, P3-5, P3-300..P3-318), core
//! part. One test per requirement at least, named after it. The editor
//! tests (P3-310..P3-315) are in `crates/editor/src/vertices_tests.rs`.

use c4rizer_core::model::{self, Model, ModelError, ModelErrorKind, ViewSpec};

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

#[test]
fn p3_4_line_with_vertices_routed() {
    todo!()
}

#[test]
fn p3_5_no_via_scene_unchanged() {
    todo!()
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
    use c4rizer_core::geom::Point;
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
fn p3_303_legs_octilinear_on_grid() {
    todo!()
}

#[test]
fn p3_304_line_passes_through_vertices() {
    todo!()
}

#[test]
fn p3_305_sharp_turn_only_at_vertex() {
    todo!()
}

#[test]
fn p3_306_ports_face_first_and_last_vertex() {
    todo!()
}

#[test]
fn p3_307_other_lines_avoid_vertex_legs() {
    todo!()
}

#[test]
fn p3_308_vertex_line_never_trunk_or_joined() {
    todo!()
}

#[test]
fn p3_309_l28_vertex_in_box_or_repeated() {
    todo!()
}

#[test]
fn p3_316_export_draws_scene_lines() {
    todo!()
}

#[test]
fn p3_317_import_layout_copies_vertices() {
    todo!()
}

#[test]
fn p3_318_json_export_writes_vertices() {
    todo!()
}
