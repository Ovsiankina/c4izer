//! Custom line colour and line tags (owner requests 2026-11-23, OR-15 /
//! issue #5, OR-16 / issue #6), core part: `Edit::SetLineStyle`,
//! `Edit::ResetLineStyle`, their inverses, the colour precedence and the
//! benchmark (SRS 4.9 P5-350..P5-360). Every test that names an element
//! runs with flat and hierarchical identifiers.

#[path = "../src/bench_util.rs"]
mod bench_util;

use std::sync::atomic::AtomicBool;

use c4rizer_core::edit::{self, Applied, Edit, EditErrorKind, LineStyleProperty, LineStyleTarget, RelationshipRef};
use c4rizer_core::export;
use c4rizer_core::model::{self, Colour, ElementKey, Model, Parsed};
use c4rizer_core::pipeline::{self, ViewBuild};
use c4rizer_core::place::Keep;

const SHOP: &str = r#"workspace {
    model {
        user = person "User"
        shop = softwareSystem "Shop" {
            web = container "Web"
            api = container "API"
            db = container "Store"
        }
        user -> web "Browses"
        web -> api "Calls" "JSON" "Sync"
        api -> db "Reads" {
            tag "Hot"
        }
    }
    views {
        container shop "c2" {
            include *
        }
        systemContext shop "c1" {
            include *
        }
        styles {
            relationship "Async" {
                color #00aa00
            }
        }
    }
}
"#;

const RED: Colour = Colour { r: 0xaa, g: 0, b: 0 };
const BLUE: Colour = Colour { r: 0, g: 0, b: 0xaa };

/// Identifier mode of a fixture: `SHOP` as written, or with
/// `!identifiers hierarchical` and dotted references.
#[derive(Clone, Copy)]
enum Ids {
    Flat,
    Hierarchical,
}

impl Ids {
    fn src(self, src: &str) -> String {
        match self {
            Ids::Flat => src.to_owned(),
            Ids::Hierarchical => src
                .replace("workspace {\n    model {", "workspace {\n    !identifiers hierarchical\n    model {")
                .replace("user -> web", "user -> shop.web")
                .replace("user -> api", "user -> shop.api")
                .replace("web -> api", "shop.web -> shop.api")
                .replace("api -> db", "shop.api -> shop.db"),
        }
    }

    /// The written key of `k` (`web`, or `shop.web`).
    fn key(self, k: &str) -> ElementKey {
        match (self, k) {
            (Ids::Hierarchical, "web" | "api" | "db") => ElementKey::from_written(format!("shop.{k}")),
            _ => ElementKey::from_written(k),
        }
    }

    fn rel(self, from: &str, to: &str) -> RelationshipRef {
        RelationshipRef::new(self.key(from), self.key(to), 0)
    }
}

fn parse(src: &str) -> Parsed {
    model::parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn apply(src: &str, edits: &[Edit]) -> Applied {
    let applied = edit::apply(&parse(src), edits).unwrap_or_else(|e| panic!("{e}\n{src}"));
    parse(&applied.text);
    applied
}

fn refused(src: &str, edits: &[Edit]) -> EditErrorKind {
    match edit::apply(&parse(src), edits) {
        Ok(a) => panic!("expected a refusal, got:\n{}", a.text),
        Err(e) => e.kind,
    }
}

/// Applies `edits`, checks undo gives `src` back byte for byte and redo
/// the change again, and returns the changed text (P5-354).
fn round_trip(src: &str, edits: &[Edit]) -> String {
    let done = apply(src, edits);
    let back = apply(&done.text, &done.inverse);
    assert_eq!(back.text, src, "undo did not restore the bytes; changed text was:\n{}", done.text);
    let again = apply(&back.text, &back.inverse);
    assert_eq!(again.text, done.text, "redo differs");
    done.text
}

fn this_line(relationship: RelationshipRef, colour: Option<Colour>) -> Edit {
    Edit::SetLineStyle {
        relationship,
        target: LineStyleTarget::ThisLine,
        property: LineStyleProperty::Color(colour),
    }
}

fn all_tag(relationship: RelationshipRef, tag: &str, colour: Option<Colour>) -> Edit {
    Edit::SetLineStyle {
        relationship,
        target: LineStyleTarget::Tag(tag.to_owned()),
        property: LineStyleProperty::Color(colour),
    }
}

fn reset(relationship: RelationshipRef) -> Edit {
    Edit::ResetLineStyle { relationship }
}

fn build(m: &Model, key: &str) -> ViewBuild {
    pipeline::build_view(m, key, &Keep::default(), &AtomicBool::new(false)).unwrap_or_else(|e| panic!("{e:?}"))
}

/// The stroke colour of the drawn line `from -> to` (box names) in view
/// `key` of `src`.
fn colour(src: &str, key: &str, from: &str, to: &str) -> Colour {
    let b = build(&parse(src).model, key);
    let name = |id: c4rizer_core::view::BoxId| b.plan.boxes[id.0 as usize].name.clone();
    b.scene
        .lines
        .iter()
        .find(|l| name(l.source) == from && name(l.destination) == to)
        .unwrap_or_else(|| panic!("no line {from} -> {to}"))
        .stroke
        .colour
}

fn invalid(kind: &EditErrorKind) -> bool {
    matches!(kind, EditErrorKind::InvalidValue { .. })
}

// ------------------------------------------------------------- this line

#[test]
fn p5_350_this_line_writes_a_per_line_tag_and_block() {
    body_p5_350_this_line_writes_a_per_line_tag_and_block(Ids::Flat);
}

#[test]
fn p5_350_this_line_writes_a_per_line_tag_and_block_hierarchical_ids() {
    body_p5_350_this_line_writes_a_per_line_tag_and_block(Ids::Hierarchical);
}

fn body_p5_350_this_line_writes_a_per_line_tag_and_block(ids: Ids) {
    let src = ids.src(SHOP);
    let r = ids.rel("user", "web");
    let text = round_trip(&src, &[this_line(r.clone(), Some(RED))]);
    let head = match ids {
        Ids::Flat => "        user -> web \"Browses\" {\n",
        Ids::Hierarchical => "        user -> shop.web \"Browses\" {\n",
    };
    assert!(text.contains(&format!("{head}            tags \"user->shop.web\"\n        }}\n")), "{text}");
    assert!(
        text.contains("            relationship \"Async\" {\n                color #00aa00\n            }\n            relationship \"user->shop.web\" {\n                color #aa0000\n            }\n        }\n"),
        "{text}"
    );
    assert_eq!(colour(&text, "c2", "User", "Web"), RED);
    // A second change rewrites the value in place.
    let again = round_trip(&text, &[this_line(r, Some(BLUE))]);
    assert_eq!(again, text.replace("color #aa0000", "color #0000aa"));
    assert_eq!(colour(&again, "c2", "User", "Web"), BLUE);
}

#[test]
fn p5_350_tag_is_suffixed_when_taken_and_blocks_are_created() {
    // Two `user -> web` relationships: the second line's tag is `-2`.
    let src = SHOP.replace("        user -> web \"Browses\"\n", "        user -> web \"Browses\"\n        user -> web \"Buys\" {\n            tags \"user->shop.web\"\n        }\n");
    let text = round_trip(&src, &[this_line(Ids::Flat.rel("user", "web"), Some(RED))]);
    assert!(text.contains("        user -> web \"Browses\" {\n            tags \"user->shop.web-2\"\n        }\n"), "{text}");
    assert!(text.contains("relationship \"user->shop.web-2\" {"), "{text}");
    // No `styles` block: one is created as the last block of `views`.
    let bare = SHOP.replace("        styles {\n            relationship \"Async\" {\n                color #00aa00\n            }\n        }\n", "");
    let text = round_trip(&bare, &[this_line(Ids::Flat.rel("web", "api"), Some(RED))]);
    assert!(text.contains("        styles {\n            relationship \"shop.web->shop.api\" {\n                color #aa0000\n            }\n        }\n    }\n}\n"), "{text}");
}

// ------------------------------------------------------------- all <tag>

#[test]
fn p5_351_tag_target_writes_the_tag_block() {
    body_p5_351_tag_target_writes_the_tag_block(Ids::Flat);
}

#[test]
fn p5_351_tag_target_writes_the_tag_block_hierarchical_ids() {
    body_p5_351_tag_target_writes_the_tag_block(Ids::Hierarchical);
}

fn body_p5_351_tag_target_writes_the_tag_block(ids: Ids) {
    let src = ids.src(SHOP).replace("\"JSON\" \"Sync\"", "\"JSON\" \"Sync,Async\"");
    // An existing block: its value is rewritten in place.
    let text = round_trip(&src, &[all_tag(ids.rel("web", "api"), "Async", Some(RED))]);
    assert_eq!(text, src.replace("color #00aa00", "color #aa0000"));
    // No block for the tag: one is added at the end of `styles`.
    let text = round_trip(&src, &[all_tag(ids.rel("api", "db"), "Hot", Some(BLUE))]);
    assert!(text.contains("            relationship \"Hot\" {\n                color #0000aa\n            }\n        }\n"), "{text}");
    assert_eq!(colour(&text, "c2", "API", "Store"), BLUE);
    // `Relationship` is carried by every line.
    let text = round_trip(&src, &[all_tag(ids.rel("user", "web"), "Relationship", Some(BLUE))]);
    assert_eq!(colour(&text, "c2", "User", "Web"), BLUE);
    // Refusals write nothing.
    assert!(invalid(&refused(&src, &[all_tag(ids.rel("user", "web"), "Hot", Some(RED))])));
    let gone = RelationshipRef::new(ids.key("db"), ids.key("user"), 0);
    assert!(matches!(refused(&src, &[this_line(gone, Some(RED))]), EditErrorKind::TargetGone { .. }));
}

#[test]
fn p5_351_copied_relationship_is_refused() {
    let src = r#"workspace {
    model {
        a = softwareSystem "A"
        b = softwareSystem "B"
        a -> b "Calls"
        live = deploymentEnvironment "Live" {
            deploymentNode "N" {
                ia = softwareSystemInstance a
                ib = softwareSystemInstance b
            }
        }
    }
}
"#;
    let m = parse(src).model;
    let copied = m.relationships.iter().find(|r| r.copied_from.is_some()).expect("a copied relationship");
    let r = RelationshipRef::of(&m, copied.id);
    assert!(invalid(&refused(src, &[this_line(r, Some(RED))])));
}

// ---------------------------------------------------------------- remove

#[test]
fn p5_352_none_removes_the_line_block_and_tag() {
    body_p5_352_none_removes_the_line_block_and_tag(Ids::Flat);
}

#[test]
fn p5_352_none_removes_the_line_block_and_tag_hierarchical_ids() {
    body_p5_352_none_removes_the_line_block_and_tag(Ids::Hierarchical);
}

fn body_p5_352_none_removes_the_line_block_and_tag(ids: Ids) {
    let src = ids.src(SHOP);
    let r = ids.rel("web", "api");
    let styled = apply(&src, &[this_line(r.clone(), Some(RED))]).text;
    // `None` on the per-line style: property, block and tag go.
    assert_eq!(apply(&styled, &[this_line(r.clone(), None)]).text, src);
    // `None` with no per-line tag writes nothing.
    let done = apply(&src, &[this_line(r.clone(), None)]);
    assert_eq!(done.text, src);
    assert!(done.inverse.is_empty());
    // `None` on a tag: the line goes, then the emptied block and `styles`
    // and `views`... only what holds nothing else.
    let text = apply(&src, &[all_tag(r, "Sync", None)]).text;
    assert_eq!(text, src, "Sync sets no colour: nothing to remove");
    let tagged = src.replace("\"JSON\" \"Sync\"", "\"JSON\" \"Async\"");
    let text = apply(&tagged, &[all_tag(ids.rel("web", "api"), "Async", None)]).text;
    assert!(!text.contains("relationship \"Async\""), "{text}");
    assert!(!text.contains("styles"), "{text}");
}

#[test]
fn p5_353_reset_removes_block_and_tag() {
    body_p5_353_reset_removes_block_and_tag(Ids::Flat);
}

#[test]
fn p5_353_reset_removes_block_and_tag_hierarchical_ids() {
    body_p5_353_reset_removes_block_and_tag(Ids::Hierarchical);
}

fn body_p5_353_reset_removes_block_and_tag(ids: Ids) {
    let src = ids.src(SHOP);
    let r = ids.rel("api", "db");
    // Nothing per-line: nothing written.
    let done = apply(&src, &[reset(r.clone())]);
    assert_eq!(done.text, src);
    assert!(done.inverse.is_empty());
    let styled = apply(&src, &[this_line(r.clone(), Some(RED))]).text;
    assert!(styled.contains("            tag \"Hot\"\n            tags \"shop.api->shop.db\"\n        }"), "{styled}");
    // Reset: block and tag go, the bytes come back; its undo restores
    // the per-line style byte for byte (P5-354).
    let back = round_trip(&styled, &[reset(r.clone())]);
    assert_eq!(back, src);
    // A per-line block holding a comment keeps it; the property goes.
    let commented = styled.replace(
        "            relationship \"shop.api->shop.db\" {\n",
        "            relationship \"shop.api->shop.db\" {\n                // hand note\n",
    );
    let text = apply(&commented, &[reset(r)]).text;
    assert!(text.contains("// hand note"), "{text}");
    assert!(!text.contains("color #aa0000"), "{text}");
    assert!(!text.contains("tags \"shop.api->shop.db\""), "{text}");
}

#[test]
fn p5_354_several_lines_one_batch_undo_restores_the_bytes() {
    body_p5_354_several_lines_one_batch_undo_restores_the_bytes(Ids::Flat);
}

#[test]
fn p5_354_several_lines_one_batch_undo_restores_the_bytes_hierarchical_ids() {
    body_p5_354_several_lines_one_batch_undo_restores_the_bytes(Ids::Hierarchical);
}

fn body_p5_354_several_lines_one_batch_undo_restores_the_bytes(ids: Ids) {
    let src = ids.src(SHOP);
    let lines = [ids.rel("user", "web"), ids.rel("web", "api"), ids.rel("api", "db")];
    let edits: Vec<Edit> = lines.iter().map(|r| this_line(r.clone(), Some(RED))).collect();
    // Undo only: a redo of several new blocks writes them in the inverse's
    // order, the same model (as for several boxes, `Applied::inverse`).
    let done = apply(&src, &edits);
    assert_eq!(apply(&done.text, &done.inverse).text, src, "undo restores the bytes");
    let text = done.text;
    for (from, to) in [("User", "Web"), ("Web", "API"), ("API", "Store")] {
        assert_eq!(colour(&text, "c2", from, to), RED);
    }
    let resets: Vec<Edit> = lines.iter().map(|r| reset(r.clone())).collect();
    let undone = apply(&text, &resets);
    assert_eq!(undone.text, src);
    assert_eq!(apply(&undone.text, &undone.inverse).text.len(), text.len());
}

// ------------------------------------------------------------ precedence

#[test]
fn p5_355_per_line_beats_tag_beats_relationship_beats_box() {
    body_p5_355_per_line_beats_tag_beats_relationship_beats_box(Ids::Flat);
}

#[test]
fn p5_355_per_line_beats_tag_beats_relationship_beats_box_hierarchical_ids() {
    body_p5_355_per_line_beats_tag_beats_relationship_beats_box(Ids::Hierarchical);
}

fn body_p5_355_per_line_beats_tag_beats_relationship_beats_box(ids: Ids) {
    let fill = Colour { r: 0x43, g: 0x8d, b: 0xd5 };
    let src = ids.src(SHOP).replace(
        "        styles {\n",
        "        styles {\n            element \"Container\" {\n                background #438dd5\n            }\n",
    );
    let plain = colour(&src, "c2", "Web", "API");
    assert_ne!(plain, fill);
    // `lineColours box`: the end box's fill.
    let boxed = src.replace("        container shop \"c2\" {\n            include *\n", "        container shop \"c2\" {\n            include *\n            lineColours box\n");
    assert_eq!(colour(&boxed, "c2", "Web", "API"), fill);
    // `Relationship` style beats the box.
    let rel = boxed.replace(
        "        styles {\n",
        "        styles {\n            relationship \"Relationship\" {\n                color #111111\n            }\n",
    );
    assert_eq!(colour(&rel, "c2", "Web", "API"), Colour { r: 0x11, g: 0x11, b: 0x11 });
    // A tag's style beats `Relationship`; of two, the later declared.
    let tagged = rel.replace("\"JSON\" \"Sync\"", "\"JSON\" \"Sync,Async\"").replace(
        "        styles {\n",
        "        styles {\n            relationship \"Sync\" {\n                color #222222\n            }\n",
    );
    assert_eq!(colour(&tagged, "c2", "Web", "API"), Colour { r: 0, g: 0xaa, b: 0 }, "Async is declared after Sync");
    // The per-line colour beats them all, also when written with `Tag`
    // on an earlier tag afterwards.
    let r = ids.rel("web", "api");
    let text = apply(&tagged, &[this_line(r.clone(), Some(RED)), all_tag(r.clone(), "Async", Some(BLUE))]).text;
    assert_eq!(colour(&text, "c2", "Web", "API"), RED);
    // Reset gives the tag colour back.
    let text = apply(&text, &[reset(r)]).text;
    assert_eq!(colour(&text, "c2", "Web", "API"), BLUE);
}

// -------------------------------------------------- several relationships

#[test]
fn p5_356_a_line_of_several_relationships_styles_its_winner() {
    body_p5_356_a_line_of_several_relationships_styles_its_winner(Ids::Flat);
}

#[test]
fn p5_356_a_line_of_several_relationships_styles_its_winner_hierarchical_ids() {
    body_p5_356_a_line_of_several_relationships_styles_its_winner(Ids::Hierarchical);
}

fn body_p5_356_a_line_of_several_relationships_styles_its_winner(ids: Ids) {
    // In the context view `user -> web` and `user -> api` are one lifted
    // line `User -> Shop`.
    let src = ids.src(&SHOP.replace("        user -> web \"Browses\"\n", "        user -> web \"Browses\"\n        user -> api \"Calls\"\n"));
    let m = parse(&src).model;
    let b = build(&m, "c1");
    let line = b
        .plan
        .edges
        .iter()
        .find(|e| b.plan.boxes[e.source.0 as usize].name == "User" && b.plan.boxes[e.destination.0 as usize].name == "Shop")
        .expect("the lifted line");
    assert_eq!(line.relationships.len(), 2);
    let winner = RelationshipRef::of(&m, line.relationships[0]);
    let text = round_trip(&src, &[this_line(winner, Some(RED))]);
    assert_eq!(colour(&text, "c1", "User", "Shop"), RED);
}

// ------------------------------------------------------ nothing else moves

#[test]
fn p5_359_goldens_unchanged_after_a_style_and_its_undo() {
    const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl"));
    const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/golden");
    let m = parse(EXAMPLE).model;
    let r = RelationshipRef::of(&m, m.relationships[0].id);
    let done = apply(EXAMPLE, &[this_line(r, Some(RED))]);
    assert_ne!(done.text, EXAMPLE);
    let undone = apply(&done.text, &done.inverse).text;
    assert_eq!(undone, EXAMPLE);
    let m = parse(&undone).model;
    for key in ["C1", "C2", "C3-serving", "C3-startup"] {
        let want = std::fs::read_to_string(format!("{GOLDEN}/{key}.svg")).expect("golden");
        assert!(export::svg(&build(&m, key).scene) == want, "{key} differs from its golden");
    }
}

// ------------------------------------------------------------- benchmark

#[test]
#[ignore = "perf: release perf oracle only"]
fn perf_p5_360_set_line_style_this_line() {
    const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example/workspace.dsl"));
    if bench_util::debug_build("perf_p5_360_set_line_style_this_line") {
        return;
    }
    let parsed = parse(EXAMPLE);
    let m = &parsed.model;
    let r = RelationshipRef::of(m, m.relationships[0].id);
    let set = [this_line(r, Some(RED))];
    bench_util::bench(["P5-360", "line style", "SetLineStyle this line, apply", "example/workspace.dsl"], 10_000, || {
        let done = edit::apply(&parsed, &set).expect("applies");
        std::hint::black_box(done);
    });
}
