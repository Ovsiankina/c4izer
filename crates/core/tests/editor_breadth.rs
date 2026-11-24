//! Core tests of the editor-breadth row (SRS 4.6: P2-701, P2-708..P2-710):
//! the `paperSize` and `line` view extension lines, their `edit::Edit`
//! variants and the O9 folding of a `line` into `PlanEdge::style`. One
//! `#[test]` per requirement, named after it. The editor tests are in
//! `crates/editor/src/editor_breadth_tests.rs`.
//!
//! Each `TODO(srs)` names the SRS hole and the reading taken
//! (docs/plans/phase2-decisions.md Part B, editor-breadth rows 5 and 6).
//! Test data uses roles, never personal names.

use c4rizer_core::model::ElementKey;
use c4rizer_core::edit::{Edit, RelationshipRef, apply};
use c4rizer_core::model::{self, Model, ModelErrorKind, PaperSize, Parsed, Routing};
use c4rizer_core::view;

/// A shop with two people, a system with two containers and a named
/// relationship; one container view.
const SHOP: &str = r#"workspace "Shop" {
    model {
        customer = person "Customer" "Buys things."
        clerk = person "Clerk" "Packs orders."
        shop = softwareSystem "Shop" "Sells things." {
            web = container "Web" "Serves pages." "Rust"
            db = container "Db" "Stores orders." "SQL"
        }
        orders = customer -> web "Orders through" "HTTPS"
        customer -> web "Pays through" "HTTPS"
        clerk -> web "Packs with"
        web -> db "Reads and writes" "SQL"
    }
    views {
        container shop "containers" "Inside the shop" {
            include *
            at customer 200 100
            at clerk 600 100
            at web 200 500
            at db 600 500
        }
    }
}
"#;

fn parsed(src: &str) -> Parsed {
    match model::parse(src) {
        Ok(p) => p,
        Err(e) => panic!("parse failed: {e}\n{src}"),
    }
}

fn model_of(src: &str) -> Model {
    parsed(src).model
}

/// `SHOP` with `extra` lines added to the container view, after its `at`
/// lines.
fn shop_with(extra: &str) -> String {
    let anchor = "            at db 600 500\n";
    let out = SHOP.replacen(anchor, &format!("{anchor}{extra}"), 1);
    assert_ne!(out, SHOP, "fixture edit applied");
    out
}

fn kind_of(src: &str) -> ModelErrorKind {
    match model::parse(src) {
        Ok(_) => panic!("expected an error\n{src}"),
        Err(e) => e.kind,
    }
}

/// The text after applying `edits` to `src`, and its inverse applied back.
fn apply_and_undo(src: &str, edits: &[Edit]) -> (String, String) {
    let p = parsed(src);
    let applied = apply(&p, edits).expect("edits apply");
    let back = apply(&parsed(&applied.text), &applied.inverse).expect("inverse applies");
    (applied.text, back.text)
}

fn rel(source: &str, destination: &str, ordinal: u32) -> RelationshipRef {
    RelationshipRef::new(ElementKey::from_written(source), ElementKey::from_written(destination), ordinal)
}

/// The view's line for the relationship `r` resolves to, in `src`.
fn line_of(src: &str, r: &RelationshipRef) -> Option<model::LineLayout> {
    let m = model_of(src);
    let s = m.find(&r.source).expect("source");
    let d = m.find(&r.destination).expect("destination");
    let id = m.find_relationship(s, d)[r.ordinal as usize];
    m.view("containers")
        .expect("view")
        .lines
        .iter()
        .find(|l| l.relationship.value == id)
        .cloned()
}

/// `paperSize <keyword>` parses into `ViewSpec::paper_size`, keyword
/// ignoring case; the keywords and pixel sizes are Structurizr's
/// `PaperSize` enum (structurizr-core 6.2.4); a second line or an unknown
/// keyword is an error naming it.
// TODO(srs): P2-701 (a), (b). Reading taken (B editor-breadth 5).
#[test]
fn p2_701_canvas_size_line_parsed() {
    let m = model_of(&shop_with("            paperSize A4_Landscape\n"));
    let v = m.view("containers").expect("view");
    assert_eq!(v.paper_size.map(|p| p.value), Some(PaperSize::A4Landscape));
    let m = model_of(&shop_with("            papersize slide_16_9\n"));
    assert_eq!(
        m.view("containers").and_then(|v| v.paper_size).map(|p| p.value),
        Some(PaperSize::Slide16x9)
    );
    assert_eq!(model_of(SHOP).view("containers").and_then(|v| v.paper_size), None);
    // Structurizr's names and 300 dpi sizes.
    for (size, keyword, px) in [
        (PaperSize::A6Portrait, "A6_Portrait", (1240, 1748)),
        (PaperSize::A5Landscape, "A5_Landscape", (2480, 1748)),
        (PaperSize::A4Portrait, "A4_Portrait", (2480, 3508)),
        (PaperSize::A3Landscape, "A3_Landscape", (4961, 3508)),
        (PaperSize::A0Landscape, "A0_Landscape", (14043, 9933)),
        (PaperSize::LetterPortrait, "Letter_Portrait", (2550, 3300)),
        (PaperSize::LegalLandscape, "Legal_Landscape", (4200, 2550)),
        (PaperSize::Slide4x3, "Slide_4_3", (3306, 2480)),
        (PaperSize::Slide16x9, "Slide_16_9", (3508, 1973)),
        (PaperSize::Slide16x10, "Slide_16_10", (3508, 2193)),
    ] {
        assert_eq!(size.keyword(), keyword);
        assert_eq!(size.size_px(), px, "{keyword}");
        let m = model_of(&shop_with(&format!("            paperSize {keyword}\n")));
        assert_eq!(m.view("containers").and_then(|v| v.paper_size).map(|p| p.value), Some(size));
    }
    // P2-400: the last `paperSize` line wins.
    let m = model_of(&shop_with("            paperSize A4_Landscape\n            paperSize A3_Landscape\n"));
    assert_eq!(m.view("containers").and_then(|v| v.paper_size).map(|p| p.value), Some(PaperSize::A3Landscape));
    assert!(matches!(
        kind_of(&shop_with("            paperSize B5\n")),
        ModelErrorKind::InvalidValue { .. }
    ));
}

/// `Edit::SetCanvasSize` adds, rewrites and removes the `paperSize` line
/// (after the view's `at` lines), touching nothing else, with an inverse
/// that gives the bytes back.
#[test]
fn p2_701_set_canvas_size_edit() {
    let set = |size| Edit::SetCanvasSize {
        view: "containers".to_owned(),
        size,
    };
    let (added, back) = apply_and_undo(SHOP, &[set(Some(PaperSize::A3Landscape))]);
    assert_eq!(added, shop_with("            paperSize A3_Landscape\n"));
    assert_eq!(back, SHOP);
    let (rewritten, back) = apply_and_undo(&added, &[set(Some(PaperSize::Slide16x9))]);
    assert_eq!(rewritten, shop_with("            paperSize Slide_16_9\n"));
    assert_eq!(back, added);
    let (removed, back) = apply_and_undo(&added, &[set(None)]);
    assert_eq!(removed, SHOP);
    assert_eq!(back, added);
    // Auto on a view with no line changes nothing.
    assert_eq!(apply(&parsed(SHOP), &[set(None)]).expect("applies").text, SHOP);
    // An unknown view is gone.
    let gone = apply(&parsed(SHOP), &[Edit::SetCanvasSize {
        view: "nope".to_owned(),
        size: Some(PaperSize::A4Portrait),
    }]);
    assert!(gone.is_err());
}

/// `line <id | src -> dst> position N`: parsed into `ViewSpec::lines`,
/// written by `Edit::SetLinePosition` (named relationships by id, others
/// as `src -> dst`), folded over the relationship style into
/// `PlanEdge::style.position` by `view::plan` (O9).
// TODO(srs): P2-708, the per-view syntax. Reading taken (B editor-breadth
// 6).
#[test]
fn p2_708_set_line_position_edit() {
    let set = |r: RelationshipRef, position| Edit::SetLinePosition {
        view: "containers".to_owned(),
        relationship: r,
        position,
    };
    // A named relationship is written by its id.
    let (text, back) = apply_and_undo(SHOP, &[set(rel("customer", "web", 0), Some(30))]);
    assert_eq!(text, shop_with("            line orders position 30\n"));
    assert_eq!(back, SHOP);
    let layout = line_of(&text, &rel("customer", "web", 0)).expect("line parsed");
    assert_eq!(layout.position.map(|p| p.value), Some(30));
    // An unnamed one as `src -> dst`.
    let (text, back) = apply_and_undo(SHOP, &[set(rel("clerk", "web", 0), Some(70))]);
    assert_eq!(text, shop_with("            line clerk -> web position 70\n"));
    assert_eq!(back, SHOP);
    // Rewritten in place, removed when it would hold nothing.
    let (moved, _) = apply_and_undo(&text, &[set(rel("clerk", "web", 0), Some(45))]);
    assert_eq!(moved, shop_with("            line clerk -> web position 45\n"));
    let (cleared, back) = apply_and_undo(&text, &[set(rel("clerk", "web", 0), None)]);
    assert_eq!(cleared, SHOP);
    assert_eq!(back, text);
    // An unnamed relationship that is not the first `src -> dst` cannot
    // be named: refused, nothing written.
    assert!(apply(&parsed(SHOP), &[set(rel("customer", "web", 1), Some(10))]).is_err());
    // O9: the plan folds the view's line over the relationship style.
    let m = model_of(&text);
    let plan = view::plan(&m, m.view("containers").expect("view"));
    let clerk = m.find_written("clerk").expect("clerk");
    let web = m.find_written("web").expect("web");
    let id = m.find_relationship(clerk, web)[0];
    let edge = plan.edges.iter().find(|e| e.winner() == id).expect("edge");
    assert_eq!(edge.style.position, Some(70));
    // Out of range is an error naming the value; two lines for one
    // relationship are a duplicate.
    assert!(matches!(
        kind_of(&shop_with("            line clerk -> web position 101\n")),
        ModelErrorKind::InvalidValue { .. }
    ));
    assert!(matches!(
        kind_of(&shop_with("            line orders position 10\n            line customer -> web jump on\n")),
        ModelErrorKind::DuplicateLine { .. }
    ));
}

/// `line ... routing direct|orthogonal|curved`, written by
/// `Edit::SetLineRouting` and folded into `PlanEdge::style.routing`.
// TODO(srs): P2-709, the per-view syntax. Reading taken (B editor-breadth
// 6).
#[test]
fn p2_709_set_line_routing_edit() {
    let set = |routing| Edit::SetLineRouting {
        view: "containers".to_owned(),
        relationship: rel("web", "db", 0),
        routing,
    };
    let (text, back) = apply_and_undo(SHOP, &[set(Some(Routing::Curved))]);
    assert_eq!(text, shop_with("            line web -> db routing curved\n"));
    assert_eq!(back, SHOP);
    // A second setting joins the same line.
    let p = parsed(&text);
    let joined = apply(&p, &[Edit::SetLinePosition {
        view: "containers".to_owned(),
        relationship: rel("web", "db", 0),
        position: Some(20),
    }])
    .expect("applies")
    .text;
    assert_eq!(joined, shop_with("            line web -> db position 20 routing curved\n"));
    let layout = line_of(&joined, &rel("web", "db", 0)).expect("line");
    assert_eq!(layout.routing.map(|r| r.value), Some(Routing::Curved));
    assert_eq!(layout.position.map(|p| p.value), Some(20));
    let m = model_of(&joined);
    let plan = view::plan(&m, m.view("containers").expect("view"));
    let edge = plan
        .edges
        .iter()
        .find(|e| m.relationship(e.winner()).destination.value == m.find_written("db").expect("db"))
        .expect("edge");
    assert_eq!(edge.style.routing, Some(Routing::Curved));
    // Case is ignored when parsing.
    let m = model_of(&shop_with("            line web -> db routing Orthogonal\n"));
    let l = &m.view("containers").expect("view").lines[0];
    assert_eq!(l.routing.map(|r| r.value), Some(Routing::Orthogonal));
}

/// `line ... jump on|off`, written by `Edit::SetLineJump` and folded into
/// `PlanEdge::style.jump`.
// TODO(srs): P2-710, the per-view syntax. Reading taken (B editor-breadth
// 6).
#[test]
fn p2_710_set_line_jump_edit() {
    let set = |jump| Edit::SetLineJump {
        view: "containers".to_owned(),
        relationship: rel("customer", "web", 0),
        jump,
    };
    let (text, back) = apply_and_undo(SHOP, &[set(Some(true))]);
    assert_eq!(text, shop_with("            line orders jump on\n"));
    assert_eq!(back, SHOP);
    let (off, _) = apply_and_undo(&text, &[set(Some(false))]);
    assert_eq!(off, shop_with("            line orders jump off\n"));
    let m = model_of(&text);
    let plan = view::plan(&m, m.view("containers").expect("view"));
    let orders = (0..m.relationships.len())
        .map(|i| model::RelationshipId(u32::try_from(i).expect("small")))
        .find(|&r| m.relationship(r).key.as_ref().is_some_and(|k| k.value == "orders"))
        .expect("named relationship");
    let edge = plan.edges.iter().find(|e| e.relationships.contains(&orders)).expect("edge");
    assert!(edge.style.jump);
    // The line does not leak into another view.
    let other = text.replace(
        "    views {\n",
        "    views {\n        systemContext shop \"context\" {\n            include *\n        }\n",
    );
    let m = model_of(&other);
    let plan = view::plan(&m, m.view("context").expect("view"));
    assert!(plan.edges.iter().all(|e| !e.style.jump));
}

/// Extra fix (editor-breadth brief): a box key entry is named by the most
/// specific tag of its boxes that has an element style, else the box's
/// kind tag; a tag without a style names no entry, as in Structurizr's
/// key (structurizr-diagram.js `registerElementStyle` names an entry by
/// the style's tags). Seen before the fix in
/// `tests/data/landscape_filtered.dsl`: a person entry read `Tag 1, Tag 2`
/// and a system entry `Tag 2, Software System, Tag 1`.
#[test]
fn core_1_key_box_entry_named_by_styled_tag() {
    use c4rizer_core::pipeline::build_view;
    use c4rizer_core::place::Keep;
    use c4rizer_core::scene::Swatch;
    use std::sync::atomic::AtomicBool;
    let names = |src: &str, view: &str| -> Vec<String> {
        let m = model_of(src);
        let build = build_view(&m, view, &Keep::default(), &AtomicBool::new(false)).expect("builds");
        build
            .scene
            .legend
            .entries
            .iter()
            .filter(|e| matches!(e.swatch, Swatch::Box { .. }))
            .map(|e| e.text.lines.join(" "))
            .collect()
    };
    let tagged = SHOP
        .replacen(
            "customer = person \"Customer\" \"Buys things.\"",
            "customer = person \"Customer\" \"Buys things.\" \"Tag 1\"",
            1,
        )
        .replacen(
            "clerk = person \"Clerk\" \"Packs orders.\"",
            "clerk = person \"Clerk\" \"Packs orders.\" \"Tag 2\"",
            1,
        );
    let entries = names(&tagged, "containers");
    assert!(entries.contains(&"Person".to_owned()), "{entries:?}");
    assert!(entries.iter().all(|e| !e.contains("Tag")), "{entries:?}");
    // A styled tag names its entry.
    let styled = tagged.replacen(
        "    views {\n",
        "    views {\n        styles {\n            element \"Tag 1\" {\n                background #ff0000\n            }\n        }\n",
        1,
    );
    let entries = names(&styled, "containers");
    assert!(entries.contains(&"Tag 1".to_owned()), "{entries:?}");
    assert!(entries.contains(&"Person".to_owned()), "{entries:?}");
    // The landscape fixture.
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/landscape_filtered.dsl"))
        .expect("fixture");
    let parsed = model::parse_with(
        &src,
        Some(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/landscape_filtered.dsl"))),
        &mut model::FsLoader,
    )
    .expect("parses");
    let build = build_view(&parsed.model, "landscape", &Keep::default(), &AtomicBool::new(false)).expect("builds");
    let entries: Vec<String> = build
        .scene
        .legend
        .entries
        .iter()
        .filter(|e| matches!(e.swatch, Swatch::Box { .. }))
        .map(|e| e.text.lines.join(" "))
        .collect();
    // redesign 2026-11-19: unstyled External systems draw in the house
    // slate, a look of their own, so they get an `External` entry.
    assert_eq!(entries, ["Person", "External", "Software System"]);
}
