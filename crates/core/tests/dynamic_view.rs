//! Tests of the dynamic-view row (SRS 4.7: P3-3, P3-200..P3-221), core
//! part. One `#[test]` per requirement at least, named after it. The
//! fixture is `tests/data/phase3/dynamic.dsl`. The editor tests (P3-218..
//! P3-221) are in `crates/editor/src/dynamic_view_tests.rs`, the CLI's in
//! `crates/cli/tests/dynamic_view.rs`. Animation (P3-7, P3-222..P3-226)
//! is not in this build and has no test here.
//!
//! "Structurizr 6.2.4 (probed)" marks expectations checked by exporting
//! the same DSL to JSON with the installed Structurizr.

use c4rizer_core::model::{self, Model, ModelError, ModelErrorKind, ViewKind, ViewSpec};

/// A shop, roles only. Relationship order: 0 customer -> web, 1 web -> db
/// "Reads orders" SQL, 2 web -> db "Writes orders" JDBC, 3 cart -> pay,
/// 4 pay -> bank, 5 db -> web, 6 `refund = web -> bank`, 7 admin -> cart.
const SHOP: &str = r#"        customer = person "Customer" "Buys goods."
        admin = person "Admin" "Runs the shop."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust" {
                cart = component "Cart" "Holds items." "Rust"
                pay = component "Pay" "Takes payment." "Rust"
            }
            db = container "Db" "Stores orders." "SQL"
        }
        bank = softwareSystem "Bank" "Moves money." "External"
        customer -> web "Browses" "HTTPS"
        web -> db "Reads orders" "SQL"
        web -> db "Writes orders" "JDBC"
        cart -> pay "Hands over the basket"
        pay -> bank "Charges the card" "HTTPS"
        db -> web "Returns rows"
        refund = web -> bank "Refunds" "HTTPS"
        admin -> cart "Inspects carts""#;

/// `views_body` in a workspace over `SHOP`.
fn ws(views_body: &str) -> String {
    format!("workspace {{\n    model {{\n{SHOP}\n    }}\n    views {{\n{views_body}\n    }}\n}}\n")
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

/// The numbers of the steps of view `key`, in file order.
fn orders(m: &Model, key: &str) -> Vec<String> {
    view(m, key).steps.iter().map(|s| s.order.clone()).collect()
}

/// The full build of view `key`.
fn build(m: &Model, key: &str) -> c4rizer_core::pipeline::ViewBuild {
    let stop = std::sync::atomic::AtomicBool::new(false);
    c4rizer_core::pipeline::build_view(m, key, &c4rizer_core::place::Keep::default(), &stop).expect("builds")
}

/// `tests/data/phase3/dynamic.dsl`.
fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/phase3/dynamic.dsl")
}

/// The element named `name`.
fn named(m: &Model, name: &str) -> model::ElementId {
    m.elements.iter().find(|e| e.name.value == name).map(|e| e.id).expect("element exists")
}

/// Each step is stored in the view in file order with its ends, relationship, texts, number and span.
#[test]
fn p3_200_steps_stored_in_file_order() {
    let src = ws(r#"        dynamic shop "flow" "Checkout" {
            customer -> web "Opens the shop"
            web -> db "Loads the basket" "SQL"
        }"#);
    let m = parse(&src);
    let v = view(&m, "flow");
    assert_eq!(v.steps.len(), 2);
    let first = &v.steps[0];
    assert_eq!(first.source.value, named(&m, "Customer"));
    assert_eq!(first.destination.value, named(&m, "Web"));
    assert_eq!(first.relationship, model::RelationshipId(0));
    assert_eq!(first.description.as_ref().map(|d| d.value.as_str()), Some("Opens the shop"));
    assert_eq!(first.technology, None);
    assert_eq!(first.order, "1");
    assert_eq!(&src[first.span.range()], "customer -> web \"Opens the shop\"");
    let second = &v.steps[1];
    assert_eq!(second.relationship, model::RelationshipId(1));
    assert_eq!(second.technology.as_ref().map(|t| t.value.as_str()), Some("SQL"));
    assert_eq!(second.order, "2");
    assert!(second.span.start > first.span.start);
}

/// `ViewKind::Dynamic` is drawable.
#[test]
fn p3_200_dynamic_is_drawable() {
    assert!(ViewKind::Dynamic.drawable());
    // deployment-view draws deployment views too (P3-2).
    assert!(ViewKind::Deployment.drawable());
}

/// A step `a -> b` stands for the first-declared relationship from `a` to `b`.
#[test]
fn p3_201_step_takes_first_declared_relationship() {
    // Two `web -> db`: the first declared ("Reads orders") wins; a
    // written technology selects among them (Structurizr 6.2.4, probed).
    let m = parse(&ws(r#"        dynamic shop "flow" {
            web -> db
            web -> db "Saves" "JDBC"
        }"#));
    let v = view(&m, "flow");
    assert_eq!(v.steps[0].relationship, model::RelationshipId(1));
    assert_eq!(v.steps[1].relationship, model::RelationshipId(2));
}

/// A step `a -> b` with no direct relationship stands for one implied through descendants (P2-7).
#[test]
fn p3_201_step_takes_implied_relationship() {
    // `customer -> shop` is implied by `customer -> web`; `shop -> bank`
    // by `pay -> bank` (Structurizr 6.2.4 creates both, probed).
    let m = parse(&ws(r#"        dynamic * "estate" {
            customer -> shop
            shop -> bank
        }"#));
    let v = view(&m, "estate");
    assert_eq!(v.steps[0].relationship, model::RelationshipId(0));
    assert_eq!(v.steps[0].destination.value, named(&m, "Shop"));
    assert_eq!(v.steps[1].relationship, model::RelationshipId(4));
}

/// No relationship from `a` to `b`: an error naming both ends.
#[test]
fn p3_201_missing_relationship_is_error() {
    let e = error(&ws(r#"        dynamic shop "flow" {
            db -> customer
        }"#));
    assert!(matches!(&e.kind, ModelErrorKind::NoRelationship { source, destination, technology: None }
        if source == "Db" && destination == "Customer"), "{e:?}");
    let text = e.to_string();
    assert!(text.contains("a relationship between \"Db\" and \"Customer\" does not exist in the model"), "{text}");
    // A technology no relationship has (Structurizr 6.2.4, probed).
    let e = error(&ws(r#"        dynamic shop "flow" {
            web -> db "Saves" "SOAP"
        }"#));
    assert!(e.to_string().contains("with technology \"SOAP\""), "{e}");
}

/// A step `a -> b` with no relationship from `a` to `b` but one from `b`
/// to `a` that an earlier step of the view uses is a response step
/// standing for that relationship (Structurizr 6.2.4 `DynamicView.add`).
#[test]
fn p3_201_reverse_step_is_response() {
    let m = parse(&ws(r#"        dynamic shop "flow" {
            customer -> web "Opens the shop"
            web -> customer "Shows the shop"
            web -> customer "Shows it again" "HTTPS"
        }"#));
    let v = view(&m, "flow");
    assert!(!v.steps[0].response);
    for step in &v.steps[1..] {
        assert!(step.response);
        // The relationship declared `customer -> web`; the step's ends as
        // written.
        assert_eq!(step.relationship, model::RelationshipId(0));
        assert_eq!(step.source.value, named(&m, "Web"));
        assert_eq!(step.destination.value, named(&m, "Customer"));
    }
    // A forward relationship wins over a reverse one: `db -> web` is
    // declared, so that step is no response.
    let m = parse(&ws(r#"        dynamic shop "flow" {
            web -> db
            db -> web
        }"#));
    assert!(!view(&m, "flow").steps[1].response);
    assert_eq!(view(&m, "flow").steps[1].relationship, model::RelationshipId(5));
}

/// A reverse relationship no earlier step uses, or one without the
/// written technology, is still the "does not exist" error (6.2.4 only
/// takes a reverse relationship already in the view).
#[test]
fn p3_201_response_needs_the_relationship_in_the_view() {
    for views in [
        "        dynamic shop \"flow\" {\n            web -> customer\n        }",
        "        dynamic shop \"flow\" {\n            web -> customer\n            customer -> web\n        }",
    ] {
        let e = error(&ws(views));
        assert!(matches!(&e.kind, ModelErrorKind::NoRelationship { source, destination, technology: None }
            if source == "Web" && destination == "Customer"), "{e:?}");
    }
    let e = error(&ws(r#"        dynamic shop "flow" {
            customer -> web
            web -> customer "Shows" "SOAP"
        }"#));
    assert!(e.to_string().contains("between \"Web\" and \"Customer\" with technology \"SOAP\""), "{e}");
}

/// `tests/data/dynamic_response.dsl`, as text.
fn response_fixture_text() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/dynamic_response.dsl");
    std::fs::read_to_string(path).expect("fixture")
}

/// A response line runs from the step's source to its destination (the
/// arrow points back along the relationship) and is dashed, as the
/// Structurizr renderer draws it; its label is the step's.
#[test]
fn p3_201_response_drawn_back_and_dashed() {
    let m = parse(&response_fixture_text());
    let b = build(&m, "signin");
    assert_eq!(b.plan.edges.len(), 4);
    let name = |id: c4rizer_core::view::BoxId| m.element(b.plan.boxes[id.0 as usize].element).name.value.clone();
    let ends: Vec<(String, String)> = b.plan.edges.iter().map(|e| (name(e.source), name(e.destination))).collect();
    let want = [("User", "App"), ("App", "Database"), ("Database", "App"), ("App", "User")];
    assert_eq!(ends, want.map(|(s, d)| (s.to_owned(), d.to_owned())));
    let labels: Vec<Option<&str>> = b.plan.edges.iter().map(|e| e.description.as_deref()).collect();
    assert_eq!(
        labels,
        [
            Some("1: Submits credentials"),
            Some("2: Looks up the user"),
            Some("3: Returns the user"),
            Some("4: Sends back a token"),
        ]
    );
    // Requests solid (CORE-10), responses dashed; each scene line points
    // at its step's destination.
    let dashed: Vec<bool> = b.scene.lines.iter().map(|l| l.stroke.dash_px.is_some()).collect();
    assert_eq!(dashed, [false, false, true, true]);
    for (line, edge) in b.scene.lines.iter().zip(&b.plan.edges) {
        assert_eq!((line.source, line.destination), (edge.source, edge.destination));
    }
    // A `Relationship/Response` style applies to responses only (the
    // renderer adds that tag for a response's style lookup).
    let styled = response_fixture_text().replace(
        "    views {\n",
        "    views {\n        styles {\n            relationship \"Relationship/Response\" {\n                style dotted\n            }\n        }\n",
    );
    let dotted = build(&parse(&styled), "signin");
    let dash: Vec<Option<[i32; 2]>> = dotted.scene.lines.iter().map(|l| l.stroke.dash_px).collect();
    assert_eq!(dash[0], None);
    assert_eq!(dash[2], dash[3]);
    assert!(dash[2].is_some());
    assert_ne!(dash[2], b.scene.lines[2].stroke.dash_px);
}

/// A response step takes the next number like any step, plain or parallel.
#[test]
fn p3_201_response_numbering() {
    let m = parse(&response_fixture_text());
    assert_eq!(orders(&m, "signin"), ["1", "2", "3", "4"]);
    let m = parse(&ws(r#"        dynamic shop "flow" {
            customer -> web
            {
                {
                    web -> customer
                }
                {
                    web -> bank
                }
            }
            5: web -> customer
            web -> customer
        }"#));
    assert_eq!(orders(&m, "flow"), ["1", "2", "2", "5", "6"]);
    let responses: Vec<bool> = view(&m, "flow").steps.iter().map(|s| s.response).collect();
    assert_eq!(responses, [false, true, false, true, true]);
}

/// A step `<relationship id> [desc]` stands for that relationship.
#[test]
fn p3_202_relationship_id_step() {
    let src = ws(r#"        dynamic shop "flow" {
            refund "Pays back"
            2: refund
        }"#);
    let m = parse(&src);
    let v = view(&m, "flow");
    assert_eq!(v.steps[0].relationship, model::RelationshipId(6));
    assert_eq!(v.steps[0].source.value, named(&m, "Web"));
    assert_eq!(v.steps[0].destination.value, named(&m, "Bank"));
    assert_eq!(v.steps[0].description.as_ref().map(|d| d.value.as_str()), Some("Pays back"));
    assert_eq!(&src[v.steps[0].source.span.range()], "refund");
    assert_eq!(v.steps[1].description, None);
}

/// An id naming no relationship is a DSL-5 error.
#[test]
fn p3_202_unknown_relationship_id_is_error() {
    let e = error(&ws(r#"        dynamic shop "flow" {
            missing "Pays back"
        }"#));
    assert!(matches!(&e.kind, ModelErrorKind::UndeclaredId { id } if id == "missing"), "{e:?}");
}

/// A step with no description or technology takes the relationship's.
#[test]
fn p3_203_step_falls_back_to_relationship_texts() {
    let m = parse(&ws(r#"        dynamic shop "flow" {
            technology on
            customer -> web
        }"#));
    let scene = build(&m, "flow").scene;
    let label = scene.labels.first().expect("one label");
    let text: Vec<&str> = label.text.lines.iter().map(String::as_str).collect();
    // "1: Browses" then the relationship's technology (probed: "1: Uses").
    assert_eq!(text.first().copied(), Some("1: Browses"), "{text:?}");
    assert!(text.iter().any(|l| l.contains("HTTPS")), "{text:?}");
}

/// In a system-scoped view a component end is an error; people, other systems and containers are fine.
#[test]
fn p3_204_system_scope_refuses_component() {
    // People, other systems and containers of any system are fine.
    let m = parse(&ws(r#"        dynamic shop "flow" {
            customer -> web
            web -> db
            refund
        }"#));
    assert_eq!(view(&m, "flow").steps.len(), 3);
    let e = error(&ws(r#"        dynamic shop "flow" {
            cart -> pay
        }"#));
    assert!(matches!(&e.kind, ModelErrorKind::NotPermittedInView { name, .. } if name == "Cart"), "{e:?}");
    assert!(e.to_string().contains("components can't be added to a dynamic view when the scope is a software system"), "{e}");
}

/// In a container-scoped view a component of another container is an error naming it.
#[test]
fn p3_205_container_scope_refuses_foreign_component() {
    let m = parse(&ws(r#"        dynamic web "inside" {
            admin -> cart
            cart -> pay
            pay -> bank
        }"#));
    assert_eq!(view(&m, "inside").steps.len(), 3);
    // The system holding the scope is refused like the scope (6.2.4).
    let e = error(&ws(r#"        dynamic web "inside" {
            customer -> shop
        }"#));
    assert!(e.to_string().contains("\"Shop\": is already the scope"), "{e}");
    // A component of another container: an error naming it.
    let src = ws(r#"        dynamic db "store" {
            cart -> pay
        }"#);
    let e = error(&src);
    assert!(matches!(&e.kind, ModelErrorKind::NotPermittedInView { name, .. } if name == "Cart"), "{e:?}");
    assert!(e.to_string().contains("only components of \"Db\""), "{e}");
    assert_eq!(&src[e.span.range()], "cart");
}

/// In `dynamic *` only people and software systems are step ends.
#[test]
fn p3_206_wildcard_takes_people_and_systems_only() {
    let m = parse(&ws(r#"        dynamic * "estate" {
            customer -> shop
        }"#));
    assert_eq!(view(&m, "estate").steps.len(), 1);
    let e = error(&ws(r#"        dynamic * "estate" {
            customer -> web
        }"#));
    assert!(matches!(&e.kind, ModelErrorKind::NotPermittedInView { name, .. } if name == "Web"), "{e:?}");
    assert!(e.to_string().contains("only people and software systems can be added to this dynamic view"), "{e}");
}

/// Deployment elements are no step ends (P3-204..P3-206 list the kinds a
/// dynamic view takes, and none is a deployment element): a dynamic view
/// over container instances is refused, whatever the scope, with the
/// error on the instance.
#[test]
fn p3_204_206_deployment_elements_are_refused() {
    let live = r#"        live = deploymentEnvironment "Live" {
            host = deploymentNode "Host" "A machine." "EC2" {
                webLive = containerInstance web
                dbLive = containerInstance db
            }
        }"#;
    let src = |views: &str| ws(views).replacen("        admin -> cart \"Inspects carts\"", &format!("        admin -> cart \"Inspects carts\"\n{live}"), 1);
    // The deployment block parses, and a view of plain elements still works.
    let m = parse(&src(r#"        dynamic shop "flow" {
            web -> db
        }"#));
    assert_eq!(view(&m, "flow").steps.len(), 1);
    for scope in ["shop", "web", "*"] {
        let text = src(&format!("        dynamic {scope} \"over-instances\" {{\n            webLive -> dbLive\n        }}"));
        let e = error(&text);
        assert!(
            matches!(&e.kind, ModelErrorKind::NotPermittedInView { name, .. } if name == "Web"),
            "{scope}: {e:?}"
        );
        assert_eq!(&text[e.span.range()], "webLive", "{scope}");
        if scope != "*" {
            assert!(e.to_string().contains("only people, software systems, containers and components can be added to a dynamic view"), "{e}");
        }
    }
}

/// A step end equal to the scope is an error.
#[test]
fn p3_207_scope_as_step_end_is_error() {
    let e = error(&ws(r#"        dynamic shop "flow" {
            customer -> shop
        }"#));
    assert!(e.to_string().contains("\"Shop\": is already the scope of this view"), "{e}");
    let e = error(&ws(r#"        dynamic web "inside" {
            customer -> web
        }"#));
    assert!(e.to_string().contains("\"Web\": is already the scope of this view"), "{e}");
}

/// Steps with no written order are numbered 1, 2, 3 in file order.
#[test]
fn p3_208_plain_steps_numbered_in_file_order() {
    let m = parse(&ws(r#"        dynamic shop "flow" {
            customer -> web
            web -> db
            db -> web
            web -> db
        }"#));
    assert_eq!(orders(&m, "flow"), ["1", "2", "3", "4"]);
}

/// A written `n:` sets the number as written; later plain steps continue from n + 1.
#[test]
fn p3_209_written_order_kept_and_continued() {
    let m = parse(&ws(r#"        dynamic shop "flow" {
            customer -> web
            5: web -> db
            db -> web
            2a: refund
            web -> db
        }"#));
    // An integer order sets the counter (P3-209; 6.2.4 would give 3 and
    // 5 to the third and fifth steps). A non-integer order still uses up
    // one count, as 6.2.4's `getNext` does before `setOrder`.
    assert_eq!(orders(&m, "flow"), ["1", "5", "6", "2a", "8"]);
    let v = view(&m, "flow");
    assert!(v.steps[1].written_order.is_some());
    assert!(v.steps[0].written_order.is_none());
}

/// Parallel blocks follow `ParallelSequenceCounter`: `seq` numbers 1, 2, 2, 3, 3, 4.
#[test]
fn p3_210_parallel_numbering_matches_structurizr() {
    // The fixture view `seq` (probed: 1, 2, 2, 3, 3, 4).
    let m = parse(&std::fs::read_to_string(fixture()).expect("fixture"));
    assert_eq!(orders(&m, "seq"), ["1", "2", "2", "3", "3", "4"]);
    // A block holding steps hands back the number it started from; a
    // block holding only blocks continues from it plus one; an empty
    // block counts one (Structurizr 6.2.4, probed `p` and `q`).
    let m = parse(&ws(r#"        dynamic shop "p" {
            customer -> web
            {
                web -> db
                refund
            }
            db -> web
        }
        dynamic shop "q" {
            customer -> web
            {
                {
                    web -> db
                }
            }
            db -> web
            {
            }
            refund
        }"#));
    assert_eq!(orders(&m, "p"), ["1", "2", "3", "2"]);
    assert_eq!(orders(&m, "q"), ["1", "2", "3", "5"]);
    let v = view(&m, "p");
    assert!(v.steps[0].parallel.is_none());
    assert!(v.steps[1].parallel.is_some());
    assert_eq!(v.steps[1].parallel, v.steps[2].parallel);
}

/// A dynamic view draws its steps in order, labels prefixed with their numbers.
#[test]
fn p3_3_dynamic_view_draws_numbered_steps() {
    let m = parse(&ws(r#"        dynamic shop "flow" "Checkout" {
            customer -> web "Opens the shop"
            web -> db "Loads the basket"
            db -> web "Returns the basket"
        }"#));
    let b = build(&m, "flow");
    assert_eq!(b.plan.edges.len(), 3);
    let labels: Vec<Option<&str>> = b.plan.edges.iter().map(|e| e.description.as_deref()).collect();
    assert_eq!(labels, [Some("1: Opens the shop"), Some("2: Loads the basket"), Some("3: Returns the basket")]);
    let orders: Vec<Option<&str>> = b.plan.edges.iter().map(|e| e.order.as_deref()).collect();
    assert_eq!(orders, [Some("1"), Some("2"), Some("3")]);
    assert_eq!(b.scene.lines.len(), 3);
    assert_eq!(b.scene.labels.len(), 3);
    // The same relationship listed twice gets two keys.
    let m = parse(&ws(r#"        dynamic shop "twice" {
            web -> db "Loads"
            web -> db "Saves"
        }"#));
    let v = view(&m, "twice");
    assert_eq!(v.step_key(&m, 0).as_deref(), Some("web->db#1"));
    assert_eq!(v.step_key(&m, 1).as_deref(), Some("web->db#2"));
}

/// Each label is `<number>: <description>`; technology per CORE-14 and `technology`.
#[test]
fn p3_211_label_is_number_and_description() {
    let m = parse(&ws(r#"        dynamic shop "flow" {
            customer -> web "Opens the shop"
            web -> db
        }
        dynamic shop "quiet" {
            technology off
            customer -> web "Opens the shop"
        }"#));
    let b = build(&m, "flow");
    let text = |b: &c4rizer_core::pipeline::ViewBuild, i: usize| b.scene.labels[i].text.lines.clone();
    // The step's own description; else the relationship's; technology
    // on its own line (CORE-14).
    assert_eq!(text(&b, 0), ["1: Opens the shop", "[HTTPS]"]);
    assert_eq!(text(&b, 1)[0], "2: Reads orders");
    assert_eq!(text(&b, 1).last().map(String::as_str), Some("[SQL]"));
    // `technology off` drops the technology line.
    assert_eq!(text(&build(&m, "quiet"), 0), ["1: Opens the shop"]);
}

/// The same relationship twice is two lines with keys `<rel>#<n>`, drawn as lanes.
#[test]
fn p3_212_same_relationship_twice_two_lines() {
    let m = parse(&ws(r#"        dynamic shop "flow" {
            web -> db "Loads"
            db -> web "Returns"
            web -> db "Saves"
            refund
            refund
        }"#));
    let b = build(&m, "flow");
    assert_eq!(b.plan.edges.len(), 5);
    assert_eq!(b.plan.edges[0].winner(), b.plan.edges[2].winner());
    assert_eq!(b.plan.edges[0].source, b.plan.edges[2].source);
    // Two lines, routed apart as lanes (CORE-12): their points differ.
    let lines = &b.scene.lines;
    assert_ne!(lines[0].points, lines[2].points);
    let v = view(&m, "flow");
    let keys: Vec<String> = (0..5).map(|i| v.step_key(&m, i).expect("drawn")).collect();
    assert_eq!(keys, ["web->db#1", "db->web#2", "web->db#3", "refund#4", "refund#5"]);
    // A step with the same relationship and number as an earlier one is
    // drawn once (6.2.4 keeps one line, probed `w`).
    let m = parse(&ws(r#"        dynamic shop "dup" {
            2: web -> db
            2: web -> db
        }"#));
    assert_eq!(view(&m, "dup").drawn_steps(), [0]);
    assert_eq!(build(&m, "dup").plan.edges.len(), 1);
}

/// The boxes are exactly the step ends; no lifting.
#[test]
fn p3_213_boxes_are_step_ends_no_lifting() {
    // `cart -> pay` is drawn between the components themselves; `web`
    // and `db` are not drawn though they hold or touch the ends.
    let m = parse(&ws(r#"        dynamic web "inside" {
            admin -> cart "Opens a cart"
            cart -> pay
        }"#));
    let b = build(&m, "inside");
    let names: Vec<&str> = b.plan.boxes.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["Admin", "Cart", "Pay"]);
    assert_eq!(m.included(view(&m, "inside")).len(), 3);
    let e = &b.plan.edges[1];
    assert_eq!(b.plan.boxes[e.source.0 as usize].name, "Cart");
    assert_eq!(b.plan.boxes[e.destination.0 as usize].name, "Pay");
}

/// A system-scoped view draws a dashed boundary around the drawn containers of the scope.
#[test]
fn p3_214_system_scope_boundary() {
    let m = parse(&ws(r#"        dynamic shop "flow" {
            customer -> web
            web -> db
            refund
        }"#));
    let b = build(&m, "flow");
    assert_eq!(b.plan.boundaries.len(), 1);
    let boundary = &b.plan.boundaries[0];
    assert_eq!(boundary.name, "Shop");
    let members: Vec<&str> = boundary.members.iter().map(|m| b.plan.boxes[m.0 as usize].name.as_str()).collect();
    assert_eq!(members, ["Web", "Db"]);
    assert!(b.scene.boundaries[0].tag.lines[0].contains("Shop [Software System]"), "{:?}", b.scene.boundaries[0].tag.lines);
}

/// A container-scoped view draws the container boundary inside its system's boundary.
#[test]
fn p3_214_container_scope_nested_boundaries() {
    let m = parse(&ws(r#"        dynamic web "inside" {
            admin -> cart
            cart -> pay
            pay -> bank
        }"#));
    let b = build(&m, "inside");
    let names: Vec<&str> = b.plan.boundaries.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["Shop", "Web"]);
    let rect = |i: usize| b.plan.boundary_rect(i, &b.scene.centres).expect("rect");
    let (outer, inner) = (rect(0), rect(1));
    assert!(outer.min.x < inner.min.x && outer.max.x > inner.max.x, "{outer:?} {inner:?}");
    assert!(outer.min.y < inner.min.y && outer.max.y > inner.max.y, "{outer:?} {inner:?}");
    assert!(b.scene.boundaries[1].tag.lines[0].contains("Web [Container]"), "{:?}", b.scene.boundaries[1].tag.lines);
}

/// `dynamic *` has no boundary.
#[test]
fn p3_214_wildcard_has_no_boundary() {
    let m = parse(&ws(r#"        dynamic * "estate" {
            customer -> shop
            shop -> bank
        }"#));
    assert!(build(&m, "estate").plan.boundaries.is_empty());
}

/// `tests/data/dynamic_view.dsl` (the row fixture).
fn row_fixture() -> Model {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/dynamic_view.dsl");
    parse(&std::fs::read_to_string(path).expect("fixture"))
}

/// On every view of the row fixture no label lies across a dashed scope
/// boundary where a spot off it exists (approver finding: "4: Charges the
/// card" sat on the Shop boundary), and the checkout view's L23 findings
/// are exactly the three labels on the 20px lanes between Web App and
/// Database, which no spot on their own line can avoid (docs/deferred.md).
#[test]
fn p3_212_fixture_labels_keep_off_boundaries() {
    let m = row_fixture();
    // A stroke band 2px either side of each boundary side, in px.
    let on_stroke = |label: &c4rizer_core::geom::Rect, b: &c4rizer_core::geom::Rect| {
        let (l, r) = (label, b);
        let across_x = |x: i32| l.min.x < x + 2 && x - 2 < l.max.x && l.max.y > r.min.y && l.min.y < r.max.y;
        let across_y = |y: i32| l.min.y < y + 2 && y - 2 < l.max.y && l.max.x > r.min.x && l.min.x < r.max.x;
        across_x(r.min.x) || across_x(r.max.x) || across_y(r.min.y) || across_y(r.max.y)
    };
    for key in ["checkout", "pricing", "estate", "written"] {
        let b = build(&m, key);
        for label in &b.scene.labels {
            for boundary in &b.scene.boundaries {
                assert!(
                    !on_stroke(&label.rect, &boundary.rect),
                    "{key}: {:?} on boundary {:?}",
                    label.text.lines,
                    boundary.rect
                );
            }
        }
    }
    let b = build(&m, "checkout");
    let mut l23: Vec<String> = b
        .report
        .findings
        .iter()
        .filter(|f| f.rule.code() == "L23")
        .map(|f| f.message.clone())
        .collect();
    l23.sort();
    assert_eq!(l23.len(), 3, "{l23:?}");
    for (msg, step) in l23.iter().zip(["2: Loads the basket", "3: Returns the basket", "5: Saves the order"]) {
        assert!(msg.contains(step), "{msg}");
    }
    // Every other label of the view is clear.
    let clear = b.scene.labels.iter().filter(|l| l.fit == c4rizer_core::label::Fit::Clear).count();
    assert_eq!(clear, b.scene.labels.len() - 3);
}

/// The box that first appears as a step source ranks no lower than the other.
#[test]
fn p3_215_first_source_ranks_first() {
    let src = std::fs::read_to_string(fixture()).expect("fixture");
    let m = parse(&src);
    for key in ["seq", "comp", "star"] {
        let built = build(&m, key);
        let spec = view(&m, key);
        // The step index at which each element first is a source.
        let first_source = |e: model::ElementId| spec.steps.iter().position(|st| st.source.value == e);
        for edge in &built.plan.edges {
            let (from, to) = (edge.source.0 as usize, edge.destination.0 as usize);
            let earlier = match (
                first_source(built.plan.boxes[from].element),
                first_source(built.plan.boxes[to].element),
            ) {
                (Some(x), Some(y)) => x < y,
                (Some(_), None) => true,
                _ => false,
            };
            if earlier {
                assert!(
                    built.scene.centres[from].y <= built.scene.centres[to].y,
                    "{key}: {} placed below {}",
                    built.plan.boxes[from].name,
                    built.plan.boxes[to].name
                );
            }
        }
    }
}

/// The fixture's `seq` view has no L1, no L3 and no L24.
#[test]
fn p3_215_fixture_seq_clean() {
    let m = parse(&std::fs::read_to_string(fixture()).expect("fixture"));
    let b = build(&m, "seq");
    let codes: Vec<&str> = b.report.findings.iter().map(|f| f.rule.code()).collect();
    for bad in ["L1", "L3", "L24"] {
        assert!(!codes.contains(&bad), "{bad} in {codes:?}");
    }
}

/// Lint on a dynamic view runs none of L14..L21.
#[test]
fn p3_216_dynamic_lint_skips_two_way_and_kind_rules() {
    // A request and its response (L14 elsewhere); a technology on a line
    // of a system-level view and a container drawn (C1/C2 rules
    // elsewhere): none of L14..L21 on a dynamic view.
    let m = parse(&ws(r#"        dynamic shop "flow" "Checkout" {
            customer -> web "Opens the shop"
            web -> db "Loads the basket"
            db -> web "Returns the basket"
        }
        dynamic * "estate" "Estate" {
            customer -> shop "Buys from"
            shop -> bank "Charges via"
        }"#));
    for key in ["flow", "estate"] {
        let codes: Vec<&str> = build(&m, key).report.findings.iter().map(|f| f.rule.code()).collect();
        for skipped in ["L14", "L15", "L16", "L17", "L18", "L19", "L20", "L21"] {
            assert!(!codes.contains(&skipped), "{key}: {skipped} in {codes:?}");
        }
    }
}

/// L12 and L13 read the step's own description on a dynamic view.
#[test]
fn p3_216_dynamic_lint_reads_step_labels() {
    // `db -> web` has the model description "Returns rows"; the steps
    // say "Uses" (L13) and nothing (falls back, no L12); `cart -> pay` in
    // a container view is described by its step only.
    let src = ws(r#"        dynamic shop "flow" "Checkout" {
            customer -> web "Uses"
            db -> web
        }"#);
    let m = parse(&src);
    let report = build(&m, "flow").report;
    let vague: Vec<&c4rizer_core::lint::Finding> = report.findings.iter().filter(|f| f.rule.code() == "L13").collect();
    assert_eq!(vague.len(), 1, "{:?}", report.findings);
    assert_eq!(&src[vague[0].spans[0].range()], "customer -> web \"Uses\"");
    assert!(!report.findings.iter().any(|f| f.rule.code() == "L12"), "{:?}", report.findings);
    // The same relationship with a model description of "Browses" but
    // an empty step text: the step's text wins, so L12.
    let m = parse(&ws(r#"        dynamic shop "blank" "Blank" {
            customer -> web " "
        }"#));
    let codes: Vec<&str> = build(&m, "blank").report.findings.iter().map(|f| f.rule.code()).collect();
    assert!(codes.contains(&"L12"), "{codes:?}");
}

/// `views.view.empty` counts steps: a dynamic view with no step is one violation.
#[test]
fn p3_217_empty_dynamic_view_is_violation() {
    let m = parse(&ws(r#"        dynamic shop "empty" "Nothing yet" {
        }
        dynamic shop "flow" "Checkout" {
            customer -> web
        }"#));
    let found: Vec<String> = c4rizer_core::inspect::inspect(&m)
        .iter()
        .filter(|v| v.inspection.key() == "views.view.empty")
        .map(|v| format!("{:?}", v.subject))
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("empty"), "{found:?}");
}

/// `TextTarget::Step` rewrites only that step's description literal.
#[test]
fn p3_218_set_step_description_writes_only_that_line() {
    use c4rizer_core::edit::{Edit, TextTarget, apply};
    let src = ws(r#"        dynamic shop "flow" {
            customer -> web "Opens the shop"
            web -> db
            {
                2: refund
            }
        }"#);
    let parsed = model::parse(&src).expect("parses");
    let set = |step: u32, value: Option<&str>, expected: Option<&str>| Edit::SetText {
        target: TextTarget::Step { view: "flow".into(), step },
        value: value.map(str::to_owned),
        expected: expected.map(str::to_owned),
    };
    // Rewrite an existing literal: only those bytes change.
    let applied = apply(&parsed, &[set(0, Some("Enters the shop"), Some("Opens the shop"))]).expect("applies");
    assert_eq!(applied.text, src.replace("\"Opens the shop\"", "\"Enters the shop\""));
    // The relationship's own description is untouched.
    assert!(applied.text.contains("customer -> web \"Browses\" \"HTTPS\""));
    // Insert a missing one, after the ends, and after a relationship id
    // with a written order.
    let applied = apply(&parsed, &[set(1, Some("Loads"), None), set(2, Some("Pays back"), None)]).expect("applies");
    assert!(applied.text.contains("            web -> db \"Loads\"\n"), "{}", applied.text);
    assert!(applied.text.contains("                2: refund \"Pays back\"\n"), "{}", applied.text);
    // The inverse gives the bytes back.
    let again = model::parse(&applied.text).expect("parses");
    assert_eq!(apply(&again, &applied.inverse).expect("undo").text, src);
    // A drifted value is a conflict, nothing written.
    assert!(apply(&parsed, &[set(0, Some("x"), Some("Something else"))]).is_err());
}

/// `Edit::SwapSteps` swaps two step lines byte for byte, keeping indentation; its own inverse.
#[test]
fn p3_220_swap_steps_swaps_lines_keeping_bytes() {
    use c4rizer_core::edit::{Edit, apply};
    let src = ws("        dynamic shop \"flow\" {\n            customer -> web \"Opens\"\n\t\t\tweb -> db   \"Loads\"  \"SQL\"\n            db -> web\n            {\n                refund\n            }\n            3: web -> db\n        }");
    let parsed = model::parse(&src).expect("parses");
    let swap = |first: u32| Edit::SwapSteps { view: "flow".into(), first };
    let applied = apply(&parsed, &[swap(0)]).expect("applies");
    // Each line keeps its place's indentation and its own bytes.
    let expected = src.replacen("            customer -> web \"Opens\"\n\t\t\tweb -> db   \"Loads\"  \"SQL\"", "            web -> db   \"Loads\"  \"SQL\"\n\t\t\tcustomer -> web \"Opens\"", 1);
    assert_eq!(applied.text, expected);
    assert_eq!(applied.inverse, [swap(0)]);
    let again = model::parse(&applied.text).expect("parses");
    assert_eq!(apply(&again, &applied.inverse).expect("undo").text, src);
    // Across a block border, or with a written order: refused.
    assert!(apply(&parsed, &[swap(2)]).is_err());
    assert!(apply(&parsed, &[swap(3)]).is_err());
}
