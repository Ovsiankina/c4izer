//! Tags and groups from the editor (owner requests 2026-11-23, OR-11 /
//! issue #1, OR-12 / issue #2): `Edit::AddTag`, `Edit::RemoveTag` and
//! `Edit::SetGroup`, their inverses, and their benchmarks (P5-100..109,
//! P5-200..206, P5-900).

#[path = "../src/bench_util.rs"]
mod bench_util;

use c4rizer_core::edit::{self, Applied, Edit, EditErrorKind, RelationshipRef, TagTarget};
use c4rizer_core::model::{self, ElementKey, Model, Parsed};

const SHOP: &str = r#"workspace {
    model {
        user = person "User"
        admin = person "Admin" "Runs the shop" "Staff"
        shop = softwareSystem "Shop" {
            web = container "Web" {
                tags "Front"
            }
            api = container "API"
            db = container "Store" "Keeps orders" "SQL" "Data,Slow"
        }
        mail = softwareSystem "Mail"
        user -> web "Browses"
        web -> api "Calls" "JSON" "Sync"
        api -> db "Reads" {
            tag "Hot"
        }
    }
    views {
        container shop "c2" {
            include *
            at user 100 100
            at web 300 100
            at api 500 100
        }
        styles {
            element "Front" {
                background #aa0000
            }
            relationship "Async" {
                color #00aa00
            }
        }
    }
}
"#;

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

/// Applies `edits`, checks the inverse gives `src` back byte for byte,
/// and returns the changed text.
fn round_trip(src: &str, edits: &[Edit]) -> String {
    let done = apply(src, edits);
    let back = apply(&done.text, &done.inverse);
    assert_eq!(back.text, src, "undo did not restore the bytes; changed text was:\n{}", done.text);
    // Redo: the inverse of the inverse gives the change again.
    let again = apply(&back.text, &back.inverse);
    assert_eq!(again.text, done.text, "redo differs");
    done.text
}

fn key(k: &str) -> ElementKey {
    ElementKey::from_written(k)
}

fn el(k: &str) -> TagTarget {
    TagTarget::Element(key(k))
}

fn rel(s: &str, d: &str) -> TagTarget {
    TagTarget::Relationship(RelationshipRef::new(key(s), key(d), 0))
}

fn add(target: TagTarget, tag: &str) -> Edit {
    Edit::AddTag { target, tag: tag.to_owned() }
}

fn remove(target: TagTarget, tag: &str) -> Edit {
    Edit::RemoveTag { target, tag: tag.to_owned() }
}

fn group(elements: &[&str], name: Option<&str>) -> Edit {
    Edit::SetGroup {
        elements: elements.iter().map(|k| key(k)).collect(),
        group: name.map(str::to_owned),
    }
}

fn tags_of(src: &str, k: &str) -> Vec<String> {
    let m = parse(src).model;
    m.element(m.find_written(k).expect("element")).tags.clone()
}

fn rel_tags(src: &str, from: &str, to: &str) -> Vec<String> {
    let m = parse(src).model;
    let (source, destination) = (m.find_written(from).expect("source"), m.find_written(to).expect("destination"));
    m.relationship(m.find_relationship(source, destination)[0]).tags.clone()
}

fn group_of(src: &str, k: &str) -> Option<String> {
    let m = parse(src).model;
    let e = m.element(m.find_written(k).expect("element"));
    e.group.map(|g| m.groups[g.0 as usize].name.value.clone())
}

fn invalid(kind: &EditErrorKind) -> bool {
    matches!(kind, EditErrorKind::InvalidValue { .. })
}

/// The canonical keys and relationship ends of a model: what EDIT-21 and
/// P5-103 keep.
fn identity(m: &Model) -> (Vec<String>, Vec<(String, String)>) {
    let elements = m.elements.iter().filter_map(|e| m.full_key(e.id)).map(|k| k.to_string()).collect();
    let relationships = m
        .relationships
        .iter()
        .map(|r| {
            let k = |id| m.full_key(id).map(|k| k.to_string()).unwrap_or_default();
            (k(r.source.value), k(r.destination.value))
        })
        .collect();
    (elements, relationships)
}

// ------------------------------------------------------------------ tags

#[test]
fn p5_200_add_tag_writes_tags_line_and_creates_block() {
    // An element with a block gets a last line; one without gets a block.
    let text = round_trip(SHOP, &[add(el("web"), "Edge")]);
    assert!(text.contains("            web = container \"Web\" {\n                tags \"Front\"\n                tags \"Edge\"\n            }\n"), "{text}");
    assert_eq!(tags_of(&text, "web").last().map(String::as_str), Some("Edge"));
    let text = round_trip(SHOP, &[add(el("api"), "Edge")]);
    assert!(text.contains("            api = container \"API\" {\n                tags \"Edge\"\n            }\n"), "{text}");
    // A relationship, with and without a block.
    let text = round_trip(SHOP, &[add(rel("user", "web"), "Async")]);
    assert!(text.contains("        user -> web \"Browses\" {\n            tags \"Async\"\n        }\n"), "{text}");
    assert!(rel_tags(&text, "user", "web").contains(&"Async".to_owned()));
    let text = round_trip(SHOP, &[add(rel("api", "db"), "Async")]);
    assert!(text.contains("            tag \"Hot\"\n            tags \"Async\"\n        }"), "{text}");
}

#[test]
fn p5_200_add_tag_already_carried_is_a_no_op() {
    for (target, tag) in [(el("web"), "Front"), (el("web"), "Element"), (el("web"), "Container"), (rel("user", "web"), "Relationship")] {
        let done = apply(SHOP, &[add(target, tag)]);
        assert_eq!(done.text, SHOP);
        assert!(done.inverse.is_empty());
    }
}

#[test]
fn p5_200_add_tag_on_hierarchical_ids() {
    let src = SHOP.replace("workspace {\n    model {", "workspace {\n    !identifiers hierarchical\n    model {")
        .replace("user -> web", "user -> shop.web")
        .replace("web -> api", "shop.web -> shop.api")
        .replace("api -> db", "shop.api -> shop.db")
        .replace("at web", "at shop.web")
        .replace("at api", "at shop.api");
    let text = round_trip(&src, &[add(el("shop.api"), "Edge"), add(rel("shop.web", "shop.api"), "Async")]);
    assert!(tags_of(&text, "shop.api").contains(&"Edge".to_owned()));
    assert!(rel_tags(&text, "shop.web", "shop.api").contains(&"Async".to_owned()));
}

#[test]
fn p5_201_remove_tag_wherever_written() {
    // `tags` line holding only it: the line and the block go.
    let text = round_trip(SHOP, &[remove(el("web"), "Front")]);
    assert!(text.contains("            web = container \"Web\"\n            api"), "{text}");
    assert!(!tags_of(&text, "web").contains(&"Front".to_owned()));
    // Positional, last argument.
    let text = round_trip(SHOP, &[remove(el("admin"), "Staff")]);
    assert!(text.contains("admin = person \"Admin\" \"Runs the shop\"\n"), "{text}");
    // Comma list: only that tag goes.
    let text = round_trip(SHOP, &[remove(el("db"), "Data")]);
    assert!(text.contains("\"SQL\" \"Slow\"\n"), "{text}");
    assert_eq!(&tags_of(&text, "db")[2..], ["Slow"]);
    // Relationship: positional, then a `tag` line in its block.
    let text = round_trip(SHOP, &[remove(rel("web", "api"), "Sync")]);
    assert!(text.contains("web -> api \"Calls\" \"JSON\"\n"), "{text}");
    let text = round_trip(SHOP, &[remove(rel("api", "db"), "Hot")]);
    assert!(text.contains("        api -> db \"Reads\"\n    }"), "{text}");
}

#[test]
fn p5_201_remove_tag_every_occurrence() {
    let src = SHOP.replace("                tags \"Front\"\n", "                tags \"Front\"\n                tag \"Front\"\n                tags \"Other,Front\"\n");
    let text = round_trip(&src, &[remove(el("web"), "Front")]);
    assert!(!tags_of(&text, "web").contains(&"Front".to_owned()));
    assert!(tags_of(&text, "web").contains(&"Other".to_owned()));
}

#[test]
fn p5_201_remove_undeclared_tag_is_refused() {
    for (target, tag) in [(el("web"), "Element"), (el("web"), "Container"), (el("api"), "Front"), (rel("user", "web"), "Relationship")] {
        assert!(invalid(&refused(SHOP, &[remove(target, tag)])));
    }
}

#[test]
fn p5_202_undo_of_remove_keeps_tag_order() {
    // Order decides the style cascade: undo puts "Data" back first.
    let done = apply(SHOP, &[remove(el("db"), "Data")]);
    let back = apply(&done.text, &done.inverse);
    assert_eq!(back.text, SHOP);
    assert_eq!(&tags_of(&back.text, "db")[2..], ["Data", "Slow"]);
    // A batch over several targets, one nested in the other, is one step.
    let src = SHOP.replace("shop = softwareSystem \"Shop\" {", "shop = softwareSystem \"Shop\" \"\" \"Front\" {");
    round_trip(&src, &[remove(el("shop"), "Front"), remove(el("web"), "Front")]);
}

#[test]
fn p5_202_undo_refused_when_statement_changed() {
    let done = apply(SHOP, &[remove(el("db"), "Data")]);
    let changed = done.text.replace("\"Keeps orders\"", "\"Keeps all orders\"");
    assert!(invalid(&refused(&changed, &done.inverse)));
}

#[test]
fn p5_203_tag_text_is_checked() {
    for bad in ["", "   ", "a\"b", "a,b", "a\nb", "a\rb"] {
        assert!(invalid(&refused(SHOP, &[add(el("api"), bad)])), "{bad:?}");
    }
    let done = apply(SHOP, &[add(el("api"), "  Edge  ")]);
    assert!(done.text.contains("tags \"Edge\""));
    assert_eq!(apply(&done.text, &done.inverse).text, SHOP);
}

#[test]
fn p5_200_edit_21_tags_create_or_delete_nothing() {
    let before = identity(&parse(SHOP).model);
    let done = apply(SHOP, &[add(el("api"), "Edge"), add(rel("user", "web"), "Async"), remove(el("db"), "Slow")]);
    assert_eq!(identity(&parse(&done.text).model), before);
    assert_eq!(identity(&parse(&apply(&done.text, &done.inverse).text).model), before);
}

// ---------------------------------------------------------------- groups

#[test]
fn p5_100_set_group_creates_group_where_first_stood() {
    let text = round_trip(SHOP, &[group(&["user", "admin"], Some("Customers"))]);
    assert!(
        text.contains(
            "    model {\n        group \"Customers\" {\n            user = person \"User\"\n            admin = person \"Admin\" \"Runs the shop\" \"Staff\"\n        }\n        shop = "
        ),
        "{text}"
    );
    assert_eq!(group_of(&text, "user").as_deref(), Some("Customers"));
    assert_eq!(group_of(&text, "admin").as_deref(), Some("Customers"));
}

#[test]
fn p5_100_set_group_moves_whole_blocks_in_source_order() {
    // Listed out of order and apart: they land in source order, the
    // block of `web` re-indented with it.
    let text = round_trip(SHOP, &[group(&["db", "web"], Some("Edge"))]);
    assert!(
        text.contains(
            "        shop = softwareSystem \"Shop\" {\n            group \"Edge\" {\n                web = container \"Web\" {\n                    tags \"Front\"\n                }\n                db = container \"Store\" \"Keeps orders\" \"SQL\" \"Data,Slow\"\n            }\n            api = container \"API\"\n        }"
        ),
        "{text}"
    );
}

#[test]
fn p5_100_set_group_into_existing_group_appends() {
    let src = SHOP.replace(
        "        mail = softwareSystem \"Mail\"\n",
        "        group \"External\" {\n            mail = softwareSystem \"Mail\"\n        }\n",
    );
    let text = round_trip(&src, &[group(&["user"], Some("External"))]);
    assert!(text.contains("        group \"External\" {\n            mail = softwareSystem \"Mail\"\n            user = person \"User\"\n        }\n"), "{text}");
    assert!(!text.contains("\n        user = person"), "{text}");
    // Already in it: nothing written.
    let done = apply(&src, &[group(&["mail"], Some("External"))]);
    assert_eq!(done.text, src);
    assert!(done.inverse.is_empty());
}

#[test]
fn p5_101_set_group_none_moves_out_after_the_group() {
    let src = SHOP.replace(
        "        mail = softwareSystem \"Mail\"\n",
        "        group \"External\" {\n            mail = softwareSystem \"Mail\"\n            // payment provider\n        }\n",
    )
    .replace("        user = person \"User\"\n", "        group \"People\" {\n            user = person \"User\"\n        }\n");
    let text = round_trip(&src, &[group(&["mail", "admin"], None)]);
    assert!(text.contains("        group \"External\" {\n            // payment provider\n        }\n        mail = softwareSystem \"Mail\"\n"), "{text}");
    assert_eq!(group_of(&text, "mail"), None);
    // In no group: nothing.
    let done = apply(&src, &[group(&["admin"], None)]);
    assert_eq!(done.text, src);
}

#[test]
fn p5_102_emptied_group_is_removed() {
    let src = SHOP.replace("        user = person \"User\"\n", "        group \"People\" {\n            user = person \"User\"\n        }\n");
    // Out: the group goes.
    let text = round_trip(&src, &[group(&["user"], None)]);
    assert!(text.contains("    model {\n        user = person \"User\"\n        admin"), "{text}");
    assert!(parse(&text).model.groups.is_empty());
    // Moved to another group: the old one goes.
    let text = round_trip(&src, &[group(&["user", "admin"], Some("Staff"))]);
    assert!(!text.contains("\"People\""), "{text}");
    // A group with an identifier or a comment stays.
    let keyed = src.replace("group \"People\" {", "people = group \"People\" {");
    let text = round_trip(&keyed, &[group(&["user"], None)]);
    assert!(text.contains("people = group \"People\" {\n        }"), "{text}");
}

#[test]
fn p5_102_nested_group_member_moves_to_top_group() {
    let src = SHOP.replace(
        "    model {\n        user = person \"User\"\n",
        "    model {\n        properties {\n            \"structurizr.groupSeparator\" \"/\"\n        }\n        group \"A\" {\n            group \"B\" {\n                user = person \"User\"\n            }\n        }\n",
    );
    let text = round_trip(&src, &[group(&["user"], None)]);
    assert!(text.contains("        }\n        user = person \"User\"\n        admin"), "{text}");
    assert!(parse(&text).model.groups.is_empty(), "{text}");
}

#[test]
fn p5_100_one_line_block_is_split_first() {
    // DSL-9: a one-line block is first split onto lines.
    let src = SHOP.replace("        mail = softwareSystem \"Mail\"\n", "        mail = softwareSystem \"Mail\" { x = container \"X\" }\n");
    let done = apply(&src, &[group(&["x"], Some("G"))]);
    assert!(done.text.contains("        mail = softwareSystem \"Mail\" {\n            group \"G\" {\n                x = container \"X\"\n            }\n        }\n"), "{}", done.text);
    assert_eq!(group_of(&done.text, "x").as_deref(), Some("G"));
}

#[test]
fn p5_103_keys_and_layout_unchanged() {
    let hier = SHOP.replace("workspace {\n    model {", "workspace {\n    !identifiers hierarchical\n    model {")
        .replace("user -> web", "user -> shop.web")
        .replace("web -> api", "shop.web -> shop.api")
        .replace("api -> db", "shop.api -> shop.db")
        .replace("at web", "at shop.web")
        .replace("at api", "at shop.api");
    for (src, members) in [(SHOP.to_owned(), ["web", "api"]), (hier, ["shop.web", "shop.api"])] {
        let before = parse(&src).model;
        let text = round_trip(&src, &[group(&members, Some("Core"))]);
        let after = parse(&text).model;
        assert_eq!(identity(&after), identity(&before));
        let views = |t: &str| t[t.find("    views {").expect("views")..].to_owned();
        assert_eq!(views(&text), views(&src));
        let out = round_trip(&text, &[group(&members, None)]);
        assert_eq!(identity(&parse(&out).model), identity(&before));
    }
}

#[test]
fn p5_104_refusals_write_nothing() {
    assert!(invalid(&refused(SHOP, &[group(&[], Some("G"))])));
    assert!(invalid(&refused(SHOP, &[group(&["user"], Some("  "))])));
    assert!(invalid(&refused(SHOP, &[group(&["user"], Some("a\nb"))])));
    // Different parents.
    assert!(invalid(&refused(SHOP, &[group(&["user", "web"], Some("G"))])));
    // Declared in an included file, away from the model's block.
    let dir = std::env::temp_dir().join(format!("c4rizer-p5-104-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("people.dsl"), "guest = person \"Guest\"\n").expect("include");
    let root = SHOP.replace("        user = person \"User\"\n", "        user = person \"User\"\n        !include people.dsl\n");
    let parsed = model::parse_with(&root, Some(&dir.join("workspace.dsl")), &mut model::FsLoader).expect("parses");
    let err = edit::apply(&parsed, &[group(&["guest"], Some("G"))]).expect_err("refused");
    assert!(invalid(&err.kind), "{err}");
    let err = edit::apply(&parsed, &[group(&["user", "guest"], Some("G"))]).expect_err("refused");
    assert!(invalid(&err.kind), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
    // Another kind: a deployment node.
    let deployment = SHOP.replace(
        "        mail = softwareSystem \"Mail\"\n",
        "        mail = softwareSystem \"Mail\"\n        live = deploymentEnvironment \"Live\" {\n            node = deploymentNode \"Node\"\n        }\n",
    );
    assert!(invalid(&refused(&deployment, &[group(&["node"], Some("G"))])));
    // Unknown element.
    assert!(matches!(refused(SHOP, &[group(&["nobody"], Some("G"))]), EditErrorKind::TargetGone { .. }));
}

#[test]
fn p5_105_undo_refused_when_scope_changed() {
    let done = apply(SHOP, &[group(&["user"], Some("People"))]);
    let changed = done.text.replace("softwareSystem \"Mail\"", "softwareSystem \"Post\"");
    assert!(invalid(&refused(&changed, &done.inverse)));
    // A change outside the scope's body does not stop the undo.
    let views = done.text.replace("at api 500 100", "at api 600 100");
    let back = apply(&views, &done.inverse);
    assert_eq!(back.text, SHOP.replace("at api 500 100", "at api 600 100"));
}

// ------------------------------------------------------------ benchmarks

// DECISION: budgets are about 4x the medians measured on 2026-11-23
// (P5-200: 1129 us, P5-100: 1615 us), rounded up (design section 9);
// both are dominated by the re-parse after each step.
const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example/workspace.dsl"));

#[test]
#[ignore = "perf: release perf oracle only"]
fn perf_p5_200_add_then_remove_tag() {
    if bench_util::debug_build("perf_p5_200_add_then_remove_tag") {
        return;
    }
    let parsed = parse(EXAMPLE);
    let m = &parsed.model;
    let target = TagTarget::Element(m.full_key(m.elements.iter().find(|e| e.parent.is_some()).expect("a nested element").id).expect("key"));
    bench_util::bench(["P5-200", "tags", "AddTag then RemoveTag apply", "example/workspace.dsl"], 4_600, || {
        let added = edit::apply(&parsed, &[add(target.clone(), "Bench")]).expect("adds");
        let again = parse(&added.text);
        let removed = edit::apply(&again, &added.inverse).expect("removes");
        assert_eq!(removed.text.len(), EXAMPLE.len());
    });
}

#[test]
#[ignore = "perf: release perf oracle only"]
fn perf_p5_100_set_group_create_in_out() {
    if bench_util::debug_build("perf_p5_100_set_group_create_in_out") {
        return;
    }
    let parsed = parse(EXAMPLE);
    let m = &parsed.model;
    // Three boxes of one scope: the first scope with three children.
    let scope = m
        .elements
        .iter()
        .find(|s| m.elements.iter().filter(|e| e.parent == Some(s.id)).count() >= 3)
        .expect("a scope with three children")
        .id;
    let three: Vec<ElementKey> = m.elements.iter().filter(|e| e.parent == Some(scope)).take(3).filter_map(|e| m.full_key(e.id)).collect();
    let create = Edit::SetGroup { elements: three[..2].to_vec(), group: Some("Bench".to_owned()) };
    let move_in = Edit::SetGroup { elements: three[2..].to_vec(), group: Some("Bench".to_owned()) };
    let move_out = Edit::SetGroup { elements: three.clone(), group: None };
    bench_util::bench(["P5-100", "groups", "SetGroup create, move in, move out", "example/workspace.dsl, 3 boxes"], 6_500, || {
        let a = edit::apply(&parsed, std::slice::from_ref(&create)).expect("creates");
        let a = parse(&a.text);
        let b = edit::apply(&a, std::slice::from_ref(&move_in)).expect("moves in");
        let b = parse(&b.text);
        let c = edit::apply(&b, std::slice::from_ref(&move_out)).expect("moves out");
        assert!(!c.text.contains("\"Bench\""));
    });
}
