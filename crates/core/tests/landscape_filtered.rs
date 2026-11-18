//! Tests of the landscape-filtered row (SRS 4.6: P2-8, P2-9, P2-19,
//! P2-20, P2-300..P2-343). One `#[test]` per requirement, named after
//! it. The fixture is `tests/data/landscape_filtered.dsl`. The editor
//! tests (P2-305, P2-313, P2-314, P2-320 canvas, P2-322 f, P2-338) are in
//! `crates/editor/src/landscape_filtered_tests.rs`; the CLI's P2-314 test
//! is in `crates/cli/tests/cli.rs`.
//!
//! Where a test says "Structurizr 6.2.4", its expected boxes and lines
//! were checked by parsing the same DSL with the installed Structurizr
//! (`~/.local/share/structurizr`, structurizr-dsl 6.2.4).
//!
//! Each `TODO(srs)` names the SRS hole and the reading taken (the
//! Structurizr-faithful one, from docs/plans/phase2-decisions.md Part B
//! and Part C, checked against Structurizr DSL 6.2.4 where noted),
//! pending the owner's yes.


use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

use c4rizer_core::lint::Rule;
use c4rizer_core::model::{
    self, ElementId, FilterMode, ImageSource, Loader, Model, ModelError, ModelErrorKind,
    NoticeKind, Selector, ViewKind, ViewScope,
};
use c4rizer_core::pipeline::{self, ViewBuild};
use c4rizer_core::place::Keep;
use c4rizer_core::export;

/// `model_body` and `views_body` inside one workspace.
fn ws(model_body: &str, views_body: &str) -> String {
    format!("workspace {{\n    model {{\n{model_body}\n    }}\n    views {{\n{views_body}\n    }}\n}}\n")
}

fn parse(src: &str) -> Model {
    match model::parse(src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e:?}\n{src}"),
    }
}

fn error(src: &str) -> ModelError {
    match model::parse(src) {
        Ok(_) => panic!("parsed:\n{src}"),
        Err(e) => e,
    }
}

fn build(m: &Model, key: &str) -> ViewBuild {
    match pipeline::build_view(m, key, &Keep::default(), &AtomicBool::new(false)) {
        Ok(b) => b,
        Err(e) => panic!("{key}: {e:?}"),
    }
}

/// The element key of `id`.
fn key(m: &Model, id: ElementId) -> String {
    m.element(id).key.as_ref().map_or_else(String::new, |k| k.value.clone())
}

/// The keys of the boxes of `b`, in box order.
fn boxes(m: &Model, b: &ViewBuild) -> Vec<String> {
    b.plan.boxes.iter().map(|x| key(m, x.element)).collect()
}

/// `src->dst` per drawn line, in edge order.
fn lines(m: &Model, b: &ViewBuild) -> Vec<String> {
    b.plan
        .edges
        .iter()
        .map(|e| {
            let s = b.plan.boxes[e.source.0 as usize].element;
            let d = b.plan.boxes[e.destination.0 as usize].element;
            format!("{}->{}", key(m, s), key(m, d))
        })
        .collect()
}

/// The lint codes of `b`'s findings, each once, sorted.
fn codes(b: &ViewBuild) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = b.report.findings.iter().map(|f| f.rule.code()).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The kind-scoped rules L15..L21.
const KIND_RULES: [Rule; 7] = [
    Rule::C1Technology,
    Rule::C1BoxKind,
    Rule::C1TooManyBoxes,
    Rule::C1Protocol,
    Rule::C2Component,
    Rule::C2NoTechnology,
    Rule::C3NoScopeComponent,
];

fn has_rule(b: &ViewBuild, rule: Rule) -> bool {
    b.report.findings.iter().any(|f| f.rule == rule)
}

/// A shop estate, roles only: two people, an external system, two
/// internal systems (one with containers and components), tags on some
/// elements and relationships.
const ESTATE: &str = r#"        operator = person "Operator" "Runs the shop." "Tag 1"
        auditor = person "Auditor" "Checks the books."
        mail = softwareSystem "Mail" "Sends mail." "External,Tag 2"
        ledger = softwareSystem "Ledger" "Keeps the books."
        shop = softwareSystem "Shop" "Sells goods." "Tag 1" {
            web = container "Web" "Serves pages." "Rust" "Tag 2" {
                api = component "Api" "Answers calls." "Rust"
                auth = component "Auth" "Checks users." "Rust"
            }
            db = container "Db" "Stores orders." "SQL"
        }
        lonely = softwareSystem "Lonely" "Talks to nobody."
        operator -> web "Uses" "HTTPS" "Tag 1"
        auditor -> ledger "Reads"
        api -> mail "Sends mail with" "SMTP" "Tag 2"
        api -> db "Reads" "SQL"
        shop -> ledger "Posts sales to" "" "Tag 1"
        auth -> mail "Notifies through" "SMTP""#;

/// An in-memory `Loader`: files by absolute path.
#[derive(Default)]
struct MemLoader {
    files: BTreeMap<PathBuf, Vec<u8>>,
}

impl MemLoader {
    fn with(files: &[(&str, &[u8])]) -> MemLoader {
        MemLoader {
            files: files
                .iter()
                .map(|(p, b)| (PathBuf::from(p), b.to_vec()))
                .collect(),
        }
    }

    fn normal(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for c in path.components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other),
            }
        }
        out
    }
}

impl Loader for MemLoader {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        self.files
            .get(&Self::normal(path))
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
    }

    fn read_dir(&mut self, _path: &Path) -> io::Result<Vec<PathBuf>> {
        Err(io::Error::new(io::ErrorKind::NotFound, "not a folder"))
    }

    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        let normal = Self::normal(path);
        if self.files.keys().any(|k| k.starts_with(&normal)) {
            Ok(normal)
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "no such file"))
        }
    }

    fn env(&self, _name: &str) -> Option<String> {
        None
    }
}

/// `ESTATE` with a landscape view `estate` (`shop` placed by hand, the
/// rest auto-placed) and `filtered` lines after it.
fn filtered_ws(filtered: &str) -> String {
    ws(
        ESTATE,
        &format!(
            "        systemLandscape \"estate\" \"The estate\" {{\n            include *\n            subtitle \"Base subtitle\"\n            labelWidth 18\n            at shop 400 300\n            size shop 280 160\n        }}\n{filtered}"
        ),
    )
}

/// The centre in px of the box drawing element `k` in `b`.
fn centre_px(m: &Model, b: &ViewBuild, k: &str) -> (i32, i32) {
    let id = m.find(k).expect("element");
    let bx = b.plan.boxes_of(id)[0];
    let c = b.scene.centres[bx.0 as usize];
    (c.x * 10, c.y * 10)
}

/// The estate probed against Structurizr 6.2.4 (the expected boxes and
/// lines of the expression tests are what it put in each view).
const PROBED: &str = r#"        operator = person "Operator" "Runs the shop." "Tag 1"
        mail = softwareSystem "Mail" "Sends mail." "External"
        ledger = softwareSystem "Ledger" "Keeps the books."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust" "Tag 2" {
                api = component "Api" "Answers calls." "Rust"
                auth = component "Auth" "Checks users." "Go"
            }
            db = container "Db" "Stores orders." "SQL"
        }
        operator -> web "Uses"
        operator -> ledger "Audits"
        operator -> mail "Reads mail"
        r1 = api -> mail "Sends" "SMTP" "Tag 2"
        auth -> mail "Notifies"
        api -> db "Reads"
        shop -> ledger "Posts"
        db -> ledger "Exports""#;

/// One view per `(kind and scope, key, body)` over `PROBED`.
fn probed(views: &[(&str, &str, &str)]) -> Model {
    let mut body = String::new();
    for (head, key, lines) in views {
        let _ = writeln!(body, "        {head} \"{key}\" {{\n{lines}\n        }}");
    }
    parse(&ws(PROBED, &body))
}

/// Sorted box keys and sorted lines of view `k`.
fn drawn(m: &Model, k: &str) -> (Vec<String>, Vec<String>) {
    let b = build(m, k);
    let mut bx = boxes(m, &b);
    bx.sort();
    let mut ls = lines(m, &b);
    ls.sort();
    (bx, ls)
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

/// Custom elements next to the estate, roles only.
const BOARD: &str = r#"        operator = person "Operator" "Runs the shop."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust"
        }
        board = element "Board" "Whiteboard" "Holds the plan." "Tag 1"
        pin = element "Pin" "" "Marks a task."
        lonely = element "Lonely"
        board -> pin "Holds"
        board -> operator "Shown to"
        operator -> shop "Uses""#;

/// A 4x3 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0xd, 0xa, 0x1a, 0xa, 0x0, 0x0, 0x0, 0xd, 0x49, 0x48, 0x44, 0x52, 0x0, 0x0, 0x0, 0x4,
    0x0, 0x0, 0x0, 0x3, 0x8, 0x2, 0x0, 0x0, 0x0, 0x3b, 0x96, 0x39, 0x91, 0x0, 0x0, 0x0, 0x10, 0x49, 0x44, 0x41,
    0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x0, 0x47, 0xc, 0x38, 0x39, 0x0, 0xf5, 0x31, 0xb, 0xf5, 0x35, 0x7b,
    0xfb, 0x82, 0x0, 0x0, 0x0, 0x0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A 120x80 SVG.
const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" height=\"80\"><rect width=\"120\" height=\"80\" fill=\"#3388aa\"/></svg>";

/// An image view `key` with source line `source` over `BOARD`.
fn image_ws(key: &str, source: &str) -> String {
    ws(BOARD, &format!("        image * \"{key}\" {{\n            {source}\n        }}"))
}

/// The files of the image tests, under `/project`.
fn image_files() -> MemLoader {
    MemLoader::with(&[
        (ROOT, b""),
        ("/project/pics/pic.png", PNG),
        ("/project/pic.svg", SVG.as_bytes()),
        ("/project/notes.txt", b"not an image"),
        ("/outside.png", PNG),
    ])
}

fn build_files(src: &str, key: &str) -> ViewBuild {
    let parsed = parse_files(src, &mut image_files()).unwrap_or_else(|e| panic!("{e:?}"));
    build(&parsed.model, key)
}

/// The diagram file the file-reading tests parse as.
const ROOT: &str = "/project/workspace.dsl";

fn parse_files(src: &str, loader: &mut MemLoader) -> Result<model::Parsed, ModelError> {
    model::parse_with(src, Some(Path::new(ROOT)), loader)
}

/// DSL-11: parse, then print the tree back with no edits, gives the
/// file byte for byte.
fn assert_roundtrip(src: &str) {
    let parsed = model::parse(src).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(parsed.trees[0].print(), src, "DSL-11 round trip");
}

// ---- System landscape (P2-8)

/// `systemLandscape` with `include *` draws every person and software
/// system as a box, no boundary and no name tag; nested relationships are
/// lifted (CORE-5); CORE-1 title, subtitle and key unchanged.
#[test]
fn p2_8_landscape_draws_people_and_systems() {
    let src = ws(
        ESTATE,
        "        systemLandscape \"estate\" \"The estate\" {\n            include *\n            subtitle \"All of it\"\n        }",
    );
    let m = parse(&src);
    let b = build(&m, "estate");
    assert_eq!(b.plan.kind, ViewKind::SystemLandscape);
    assert_eq!(boxes(&m, &b), ["operator", "auditor", "mail", "ledger", "shop", "lonely"]);
    // No scope boundary, no name tag.
    assert!(b.plan.boundaries.is_empty());
    assert!(b.scene.boundaries.is_empty());
    // Nested relationships lifted (CORE-5): web, api and auth lift to shop.
    assert_eq!(
        lines(&m, &b),
        ["operator->shop", "auditor->ledger", "shop->mail", "shop->ledger"]
    );
    // The two lifted lines into mail merge; the first declared wins.
    let to_mail = &b.plan.edges[2];
    assert_eq!(to_mail.relationships.len(), 2);
    assert_eq!(to_mail.description.as_deref(), Some("Sends mail with"));
    // CORE-1 title, subtitle and key.
    assert_eq!(b.plan.title.as_deref(), Some("The estate"));
    assert_eq!(b.plan.subtitle.as_deref(), Some("All of it"));
    assert!(b.scene.title.is_some() && b.scene.subtitle.is_some());
    assert!(!b.scene.legend.entries.is_empty());
    // Every line routed.
    assert!(b.scene.lines.iter().all(|l| l.routed));
    let svg = export::svg(&b.scene);
    assert!(svg.contains("The estate"));
}

/// `systemLandscape ["key"] ["description"]` parses with
/// `ViewScope::Unscoped`; its body accepts `include`, `exclude`,
/// `autoLayout`, `default`, `title`, `description`, `properties` and the
/// c4rizer view extensions; `animation` is kept byte for byte with a P2-3
/// notice; the file round-trips (DSL-11).
#[test]
fn p2_300_landscape_parses_without_scope() {
    let body = "        systemLandscape \"estate\" \"The estate\" {\n            include *\n            exclude lonely\n            autoLayout lr\n            default\n            title \"Estate\"\n            description \"Everything\"\n            properties {\n                \"structurizr.groups\" false\n            }\n            subtitle \"Sub\"\n            technology off\n            bus on\n            labelWidth 20\n            maxCrossings 2\n            focus shop\n            hide operator -> shop\n            at shop 400 300\n            size shop 260 160\n            animation {\n                shop\n                mail\n            }\n        }";
    let src = ws(ESTATE, body);
    let parsed = model::parse(&src).expect("parses");
    let m = &parsed.model;
    let v = m.view("estate").expect("view");
    assert_eq!(v.kind, ViewKind::SystemLandscape);
    assert_eq!(v.scope, ViewScope::Unscoped);
    assert_eq!(v.title.as_ref().unwrap().value, "Estate");
    assert_eq!(v.description.as_ref().unwrap().value, "Everything");
    assert_eq!(v.subtitle.as_ref().unwrap().value, "Sub");
    assert!(v.default.is_some());
    assert_eq!(v.properties.len(), 1);
    assert_eq!(v.positions.len(), 1);
    assert_eq!(v.sizes.len(), 1);
    assert_eq!(v.focus.len(), 1);
    assert_eq!(v.hide.len(), 1);
    assert!(v.auto_layout.is_some());
    assert_eq!(v.options().label_width, 20);
    // `animation` is kept with a P2-3 notice naming it and the view.
    let n: Vec<_> = m.notices.iter().filter(|n| n.subject == "animation").collect();
    assert_eq!(n.len(), 1, "{:?}", m.notices);
    assert_eq!(n[0].kind, NoticeKind::NoEffect);
    assert_eq!(n[0].view.as_deref(), Some("estate"));
    // Key and description are optional; no scope argument is taken.
    let bare = parse(&ws(ESTATE, "        systemLandscape {\n            include *\n        }"));
    assert_eq!(bare.views[0].scope, ViewScope::Unscoped);
    assert!(bare.views[0].declared_key.is_none());
    // A third argument is one too many.
    let e = error(&ws(ESTATE, "        systemLandscape \"k\" \"d\" \"x\" {\n            include *\n        }"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
    // DSL-11.
    assert_roundtrip(&src);
}

/// A `systemLandscape` view with no key gets `SystemLandscape-001`,
/// numbered per kind in declaration order, skipping taken keys.
#[test]
fn p2_301_landscape_generated_key() {
    let src = ws(
        ESTATE,
        "        systemLandscape \"SystemLandscape-001\" {\n            include *\n        }\n        systemLandscape {\n            include *\n        }\n        systemContext shop {\n            include *\n        }\n        systemLandscape {\n            include *\n        }",
    );
    let m = parse(&src);
    let keys: Vec<&str> = m.views.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(
        keys,
        ["SystemLandscape-001", "SystemLandscape-002", "SystemContext-001", "SystemLandscape-003"]
    );
}

/// On a landscape view `include *` selects every person and software
/// system, connected or not; `include <container>` keeps the container.
#[test]
fn p2_302_landscape_include_star_selects_all_people_and_systems() {
    let m = parse(&ws(
        ESTATE,
        "        systemLandscape \"estate\" {\n            include *\n        }\n        systemLandscape \"plus\" {\n            include *\n            include web\n        }\n        systemLandscape \"some\" {\n            include operator mail\n        }",
    ));
    let estate = m.included(m.view("estate").unwrap());
    let names: Vec<String> = estate.iter().map(|&e| key(&m, e)).collect();
    // Connected or not: `lonely` has no relationship.
    assert_eq!(names, ["operator", "auditor", "mail", "ledger", "shop", "lonely"]);
    let plus: Vec<String> = m.included(m.view("plus").unwrap()).iter().map(|&e| key(&m, e)).collect();
    assert!(plus.contains(&"web".to_owned()), "{plus:?}");
    let b = build(&m, "plus");
    // The explicit container keeps its own box and its own lines.
    assert!(lines(&m, &b).contains(&"operator->web".to_owned()), "{:?}", lines(&m, &b));
    let some: Vec<String> = m.included(m.view("some").unwrap()).iter().map(|&e| key(&m, e)).collect();
    assert_eq!(some, ["operator", "mail"]);
}

/// On landscape, custom and image views lint runs L1..L14 and L23..L26
/// and none of L19..L21.
#[test]
fn p2_303_unscoped_views_run_generic_rules_only() {
    // A landscape, a custom and an image view, each failing generic
    // rules (no title) and holding what C1..C3 rules would flag.
    let src = ws(
        &format!("{BOARD}\n        rest = softwareSystem \"REST gateway\" \"Routes calls.\""),
        "        systemLandscape \"land\" {\n            include *\n            include web\n        }\n        custom \"board\" {\n            include *\n        }\n        image * \"pic\" {\n            plantuml diagram.puml\n        }",
    );
    let m = parse(&src);
    let land = build(&m, "land");
    assert!(has_rule(&land, Rule::NoTitle));
    for rule in [Rule::C2Component, Rule::C2NoTechnology, Rule::C3NoScopeComponent] {
        assert!(!has_rule(&land, rule), "{rule:?}");
    }
    for k in ["board", "pic"] {
        let b = build(&m, k);
        assert!(has_rule(&b, Rule::NoTitle), "{k}: {:?}", codes(&b));
        for rule in KIND_RULES {
            assert!(!has_rule(&b, rule), "{k} {rule:?}");
        }
    }
    // Generic rules run on the custom view: an empty description (L9).
    assert!(has_rule(&build(&m, "board"), Rule::NameOrDescription));
}

/// C1 rules on a landscape view.
// TODO(srs): P2-304. Reading taken (B landscape-filtered 7): L15, L16 and
// L18 run; L17 (more than 8 boxes) does not.
#[test]
fn p2_304_landscape_runs_c1_rules_except_l17() {
    // Nine systems, one line with a technology, a container box and a
    // protocol word in a name.
    let mut model_body = String::from(ESTATE);
    for i in 0..6 {
        let _ = write!(model_body, "\n        extra{i} = softwareSystem \"Extra {i}\" \"One more.\"");
    }
    model_body.push_str("\n        rest = softwareSystem \"REST gateway\" \"Routes calls.\"");
    let m = parse(&ws(
        &model_body,
        "        systemLandscape \"estate\" \"The estate\" {\n            include *\n            include web\n        }",
    ));
    let b = build(&m, "estate");
    assert!(b.plan.boxes.len() > 8);
    assert!(has_rule(&b, Rule::C1Technology), "{:?}", codes(&b));
    assert!(has_rule(&b, Rule::C1BoxKind), "{:?}", codes(&b));
    assert!(has_rule(&b, Rule::C1Protocol), "{:?}", codes(&b));
    assert!(!has_rule(&b, Rule::C1TooManyBoxes), "{:?}", codes(&b));
    for rule in [Rule::C2Component, Rule::C2NoTechnology, Rule::C3NoScopeComponent] {
        assert!(!has_rule(&b, rule), "{rule:?}");
    }
}

// ---- Filtered view (P2-9)

/// A filtered view draws its base view's subset, every box at the base
/// view's `at` position and `size`.
#[test]
fn p2_9_filtered_draws_base_subset_at_base_positions() {
    let m = parse(&filtered_ws(
        "        filtered \"estate\" include \"Tag 1,Relationship\" \"tagged\" \"Tagged things\"",
    ));
    let v = m.view("tagged").unwrap();
    assert_eq!(v.kind, ViewKind::Filtered);
    assert_eq!(v.scope, ViewScope::Unscoped);
    let f = v.filter.as_ref().unwrap();
    assert_eq!(f.base.value, "estate");
    assert_eq!(f.mode.value, FilterMode::Include);
    assert_eq!(f.tags(), ["Tag 1", "Relationship"]);
    assert_eq!(m.base_view(v).map(|b| b.key.as_str()), Some("estate"));
    let base = build(&m, "estate");
    let b = build(&m, "tagged");
    // Tag 1: operator and shop.
    assert_eq!(boxes(&m, &b), ["operator", "shop"]);
    assert_eq!(lines(&m, &b), ["operator->shop"]);
    // Every box at the base view's `at` and `size` lines.
    assert_eq!(centre_px(&m, &b, "shop"), (400, 300));
    let shop = &b.scene.boxes[b.plan.boxes_of(m.find("shop").unwrap())[0].0 as usize];
    assert_eq!((shop.rect.width(), shop.rect.height()), (280, 160));
    // The auto-placed operator sits where the base view placed it.
    assert_eq!(centre_px(&m, &b, "operator"), centre_px(&m, &base, "operator"));
    // The filtered view's own key and title.
    assert_eq!(b.scene.key, "tagged");
    assert_eq!(b.plan.title.as_deref(), Some("Tagged things"));
}

/// A base key naming no view is `UnknownView`; a base of another kind is
/// `InvalidBaseView`; a base declared later is `UnknownView`.
// TODO(srs): P2-306, base kinds and order. Reading taken (B
// landscape-filtered 2; Structurizr 6.2.4 `FilteredViewParser`: "must be
// a System Landscape, System Context, Container, Component, or Deployment
// view", base looked up at parse time): landscape, context, container or
// component, declared before.
#[test]
fn p2_306_filtered_base_must_exist_and_be_static() {
    // Unknown key.
    let e = error(&filtered_ws("        filtered \"nowhere\" include \"Tag 1\""));
    assert!(matches!(&e.kind, ModelErrorKind::UnknownView { key } if key == "nowhere"), "{e:?}");
    assert!(e.to_string().contains("nowhere"));
    // Declared later: unknown when the line is read.
    let later = ws(
        ESTATE,
        "        filtered \"estate\" include \"Tag 1\" \"f\"\n        systemLandscape \"estate\" {\n            include *\n        }",
    );
    let e = error(&later);
    assert!(matches!(&e.kind, ModelErrorKind::UnknownView { key } if key == "estate"), "{e:?}");
    // Another kind: a filtered view, a custom view, an image view.
    for (decl, kind) in [
        ("        filtered \"estate\" include \"Tag 1\" \"f1\"", ViewKind::Filtered),
        ("        custom \"f1\" {\n            include *\n        }", ViewKind::Custom),
        ("        image * \"f1\" {\n            plantuml diagram.puml\n        }", ViewKind::Image),
    ] {
        let src = filtered_ws(&format!("{decl}\n        filtered \"f1\" exclude \"Tag 1\" \"f2\""));
        let e = error(&src);
        assert!(
            matches!(&e.kind, ModelErrorKind::InvalidBaseView { key, kind: k } if key == "f1" && *k == kind),
            "{e:?}"
        );
        assert!(e.to_string().contains("f1"), "{e}");
    }
    // Context, container and component bases are accepted.
    let m = parse(&ws(
        ESTATE,
        "        systemContext shop \"c1\" {\n            include *\n        }\n        container shop \"c2\" {\n            include *\n        }\n        component web \"c3\" {\n            include *\n        }\n        filtered \"c1\" include \"Element,Relationship\" \"f1\"\n        filtered \"c2\" include \"Element,Relationship\" \"f2\"\n        filtered \"c3\" include \"Element,Relationship\" \"f3\"",
    ));
    for k in ["f1", "f2", "f3"] {
        let b = build(&m, k);
        assert!(!b.plan.boxes.is_empty(), "{k}");
    }
}

/// `include`, `exclude`, `autoLayout` and `animation` in a filtered body
/// are errors naming the construct; `default`, `title`, `description`
/// and `properties` are accepted.
// TODO(srs): P2-307, c4rizer extensions. Reading taken (B
// landscape-filtered 1): any line other than the four is an error naming
// it.
#[test]
fn p2_307_filtered_body_accepts_only_reference_children() {
    let ok = filtered_ws(
        "        filtered \"estate\" include \"Tag 1\" \"f\" {\n            default\n            title \"Own title\"\n            description \"Own description\"\n            properties {\n                a b\n            }\n        }",
    );
    let m = parse(&ok);
    let v = m.view("f").unwrap();
    assert!(v.default.is_some());
    assert_eq!(v.title.as_ref().unwrap().value, "Own title");
    assert_eq!(v.description.as_ref().unwrap().value, "Own description");
    assert_eq!(v.properties.len(), 1);
    assert_roundtrip(&ok);
    for line in [
        "include *",
        "exclude shop",
        "autoLayout",
        "animation {\n                shop\n            }",
        "at shop 100 100",
        "size shop 200 100",
        "subtitle \"x\"",
        "technology off",
        "focus shop",
        "labelWidth 20",
    ] {
        let src = filtered_ws(&format!("        filtered \"estate\" include \"Tag 1\" \"f\" {{\n            {line}\n        }}"));
        let e = error(&src);
        let word = line.split_whitespace().next().unwrap();
        assert!(
            matches!(&e.kind, ModelErrorKind::UnknownConstruct { construct } if construct == word),
            "{line}: {e:?}"
        );
        assert!(e.to_string().contains(word), "{e}");
    }
}

/// `include` keeps a box whose element has any listed tag, `exclude`
/// drops it; tags are comma-separated and trimmed; implicit tags count.
#[test]
fn p2_308_filtered_any_listed_tag() {
    let m = parse(&filtered_ws(
        "        filtered \"estate\" include \" Tag 1 , Tag 2 ,\" \"inc\"\n        filtered \"estate\" exclude \"Tag 1,Tag 2\" \"exc\"\n        filtered \"estate\" include \"Person\" \"people\"\n        filtered \"estate\" exclude \"Element\" \"nothing\"",
    ));
    let inc = m.view("inc").unwrap();
    // Comma-separated, trimmed, empty entries dropped.
    assert_eq!(inc.filter.as_ref().unwrap().tags(), ["Tag 1", "Tag 2"]);
    let keys = |v: &str| -> Vec<String> { m.included(m.view(v).unwrap()).iter().map(|&e| key(&m, e)).collect() };
    // Any listed tag: operator, shop (Tag 1), mail (Tag 2).
    assert_eq!(keys("inc"), ["operator", "mail", "shop"]);
    assert_eq!(keys("exc"), ["auditor", "ledger", "lonely"]);
    // Implicit tags count.
    assert_eq!(keys("people"), ["operator", "auditor"]);
    assert!(keys("nothing").is_empty());
    let b = build(&m, "nothing");
    assert!(b.plan.boxes.is_empty() && b.plan.edges.is_empty());
}

/// A line is kept only when both boxes are kept and its relationship
/// passes the tag rule; `include "Element,Relationship"` equals the base
/// view.
#[test]
fn p2_309_filtered_line_needs_both_boxes_and_tag() {
    let m = parse(&filtered_ws(
        "        filtered \"estate\" include \"Element,Relationship\" \"all\"\n        filtered \"estate\" include \"Element,Tag 1\" \"tagged-lines\"\n        filtered \"estate\" exclude \"Tag 1\" \"untagged\"\n        filtered \"estate\" include \"Person,Relationship\" \"people\"",
    ));
    let base = build(&m, "estate");
    // `include "Element,Relationship"` is the base view unchanged.
    let all = build(&m, "all");
    assert_eq!(boxes(&m, &all), boxes(&m, &base));
    assert_eq!(lines(&m, &all), lines(&m, &base));
    assert_eq!(all.scene.centres, base.scene.centres);
    assert_eq!(all.scene.lines, base.scene.lines);
    assert_eq!(all.scene.boxes, base.scene.boxes);
    // Include mode: the line's relationship needs a listed tag.
    let t = build(&m, "tagged-lines");
    assert_eq!(lines(&m, &t), ["operator->shop", "shop->ledger"]);
    // Exclude mode: none of them; shop is dropped, so are its lines.
    let u = build(&m, "untagged");
    assert_eq!(boxes(&m, &u), ["auditor", "mail", "ledger", "lonely"]);
    assert_eq!(lines(&m, &u), ["auditor->ledger"]);
    // Both boxes needed.
    let p = build(&m, "people");
    assert!(p.plan.edges.is_empty());
}

/// A lifted line merging several relationships.
// TODO(srs): P2-310. Reading taken (B landscape-filtered 3): kept when at
// least one merged relationship passes; drawn re-merged from the
// survivors (label and winner from them).
#[test]
fn p2_310_filtered_lifted_line_remerged_from_survivors() {
    // shop->mail merges api->mail (Tag 2, first) and auth->mail.
    let m = parse(&filtered_ws(
        "        filtered \"estate\" exclude \"Tag 2\" \"no-tag-2\"\n        filtered \"estate\" include \"Element,Tag 2\" \"only-tag-2\"",
    ));
    let base = build(&m, "estate");
    let lifted = base.plan.edges.iter().find(|e| e.relationships.len() == 2).expect("merged line");
    assert_eq!(lifted.description.as_deref(), Some("Sends mail with"));
    // Excluding Tag 2 drops mail itself (it has Tag 2).
    let without = build(&m, "no-tag-2");
    assert!(!lines(&m, &without).iter().any(|l| l.ends_with("->mail")));
    // Only Tag 2 relationships: the line survives from api->mail alone.
    let only = build(&m, "only-tag-2");
    let line = only.plan.edges.iter().find(|e| lines(&m, &only)[e.id.0 as usize] == "shop->mail").expect("line");
    assert_eq!(line.relationships.len(), 1);
    assert_eq!(line.description.as_deref(), Some("Sends mail with"));
    // A survivor that is not the base winner becomes the winner.
    let m2 = parse(&filtered_ws("        filtered \"estate\" include \"Element,Untagged\" \"second\""));
    let s = build(&m2, "second");
    assert!(s.plan.edges.is_empty());
    let src = filtered_ws("        filtered \"estate\" exclude \"Tag 2\" \"second\"").replace("\"Notifies through\" \"SMTP\"", "\"Notifies through\" \"SMTP\" \"Late\"");
    let m3 = parse(&src.replace("exclude \"Tag 2\" \"second\"", "include \"Element,Late\" \"second\""));
    let s = build(&m3, "second");
    assert_eq!(lines(&m3, &s), ["shop->mail"]);
    assert_eq!(s.plan.edges[0].description.as_deref(), Some("Notifies through"));
    assert_eq!(s.plan.edges[0].winner(), m3.relationships[5].id);
}

/// The drawn title is the filtered view's own `title`, else its own
/// description; L8 applies.
// TODO(srs): P2-311, options. Reading taken (B landscape-filtered 4): the
// base view's c4rizer options.
#[test]
fn p2_311_filtered_own_title_base_options() {
    let m = parse(&filtered_ws(
        "        filtered \"estate\" include \"Element,Relationship\" \"titled\" \"Described\" {\n            title \"Own title\"\n        }\n        filtered \"estate\" include \"Element,Relationship\" \"described\" \"Only a description\"\n        filtered \"estate\" include \"Element,Relationship\" \"bare\"",
    ));
    let t = build(&m, "titled");
    assert_eq!(t.plan.title.as_deref(), Some("Own title"));
    let d = build(&m, "described");
    assert_eq!(d.plan.title.as_deref(), Some("Only a description"));
    // L8 applies: no title of its own, not the base's.
    let bare = build(&m, "bare");
    assert_eq!(bare.plan.title, None);
    assert!(has_rule(&bare, Rule::NoTitle));
    // The base view's options and subtitle.
    assert_eq!(t.plan.options.label_width, 18);
    assert_eq!(t.plan.subtitle.as_deref(), Some("Base subtitle"));
}

/// A box with no `at` line in the base view.
// TODO(srs): P2-312. Reading taken (B landscape-filtered 1): it sits at
// the centre the base view's scene gives it (shared layout).
#[test]
fn p2_312_filtered_unplaced_box_takes_base_centre() {
    let m = parse(&filtered_ws(
        "        filtered \"estate\" exclude \"Person\" \"no-people\"\n        filtered \"estate\" include \"Software System\" \"systems\"",
    ));
    let base = build(&m, "estate");
    for k in ["no-people", "systems"] {
        let b = build(&m, k);
        for e in ["mail", "ledger", "lonely", "shop"] {
            assert_eq!(centre_px(&m, &b, e), centre_px(&m, &base, e), "{k} {e}");
        }
    }
}

/// Generated keys of `filtered`, `custom` and `image` views.
// TODO(srs): P2-315. Reading taken (B landscape-filtered 6; Structurizr
// 6.2.4 `ViewSet.generateViewKey`: `<Prefix>-` + `DecimalFormat("000")`,
// skipping taken keys): `Filtered-001`, `Custom-001`, `Image-001`.
#[test]
fn p2_315_filtered_custom_image_generated_keys() {
    let src = ws(
        BOARD,
        "        systemLandscape {\n            include *\n        }\n        filtered \"SystemLandscape-001\" include \"Tag 1\"\n        filtered \"SystemLandscape-001\" exclude \"Tag 1\"\n        custom \"Custom-002\" {\n            include *\n        }\n        custom {\n            include *\n        }\n        custom {\n            include board\n        }\n        image * {\n            plantuml diagram.puml\n        }",
    );
    let m = parse(&src);
    let keys: Vec<&str> = m.views.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(
        keys,
        ["SystemLandscape-001", "Filtered-001", "Filtered-002", "Custom-002", "Custom-001", "Custom-003", "Image-001"]
    );
}

/// Kind-scoped lint on a filtered view.
// TODO(srs): P2-342. Reading taken (B landscape-filtered 4): only the
// P2-303 set (L1..L14, L23..L26).
#[test]
fn p2_342_filtered_runs_generic_rules_only() {
    // A context base with a technology on a line (L15) and a container
    // base with a component (L19): the filtered views run neither.
    let m = parse(&ws(
        ESTATE,
        "        systemContext shop \"c1\" \"C1\" {\n            include *\n            include web\n        }\n        container shop \"c2\" \"C2\" {\n            include *\n            include api\n        }\n        filtered \"c1\" include \"Element,Relationship\" \"f1\" \"F1\"\n        filtered \"c2\" include \"Element,Relationship\" \"f2\" \"F2\"",
    ));
    let c1 = build(&m, "c1");
    assert!(has_rule(&c1, Rule::C1Technology) && has_rule(&c1, Rule::C1BoxKind), "{:?}", codes(&c1));
    let c2 = build(&m, "c2");
    assert!(has_rule(&c2, Rule::C2Component), "{:?}", codes(&c2));
    for k in ["f1", "f2"] {
        let b = build(&m, k);
        for rule in KIND_RULES {
            assert!(!has_rule(&b, rule), "{k} {rule:?}");
        }
    }
}

// ---- Custom and image views (P2-19)

/// A `custom` view draws custom elements; an `image` view with a local
/// file shows that PNG or SVG.
#[test]
fn p2_19_custom_and_image_views_draw() {
    let m = parse(&ws(BOARD, "        custom \"board\" \"The board\" {\n            include *\n        }"));
    let b = build(&m, "board");
    assert_eq!(b.plan.kind, ViewKind::Custom);
    assert_eq!(boxes(&m, &b), ["board", "pin", "lonely"]);
    assert_eq!(lines(&m, &b), ["board->pin"]);
    assert!(b.scene.boundaries.is_empty());
    let svg = export::svg(&b.scene);
    assert!(svg.contains(">Board<") && svg.contains("[Whiteboard]"));
    // An image view with a local SVG shows it.
    let i = build_files(&image_ws("pic", "image pic.svg"), "pic");
    let image = i.scene.image.as_ref().expect("picture");
    assert!(image.placeholder.is_none());
    assert_eq!(image.image.asset.as_ref().unwrap().bytes.as_ref(), SVG.as_bytes());
}

/// `custom ["key"] ["title"] ["description"]` takes the title second;
/// body as P2-300; `animation` as P2-300.
#[test]
fn p2_316_custom_title_is_second_argument() {
    let body = "        custom \"board\" \"The board\" \"What is on it\" {\n            include *\n            exclude lonely\n            autoLayout\n            default\n            properties {\n                a b\n            }\n            subtitle \"Sub\"\n            at board 200 200\n            size pin 240 140\n            animation {\n                board\n            }\n        }";
    let src = ws(BOARD, body);
    let m = parse(&src);
    let v = m.view("board").unwrap();
    assert_eq!(v.kind, ViewKind::Custom);
    assert_eq!(v.scope, ViewScope::Unscoped);
    assert_eq!(v.title.as_ref().unwrap().value, "The board");
    assert_eq!(v.description.as_ref().unwrap().value, "What is on it");
    assert!(v.default.is_some() && v.auto_layout.is_some());
    assert_eq!((v.positions.len(), v.sizes.len()), (1, 1));
    let n: Vec<_> = m.notices.iter().filter(|n| n.subject == "animation").collect();
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].view.as_deref(), Some("board"));
    assert_eq!(build(&m, "board").plan.title.as_deref(), Some("The board"));
    assert_roundtrip(&src);
}

/// A `title` line in the body of a custom view with a positional title
/// is no second title: the body line wins, as the later one (corpus
/// test.dsl; Structurizr 6.2.4 probed: `custom "K" "Positional" { title
/// "Body" }` has title `Body`).
#[test]
fn p2_316_custom_title_line_overrides_positional_title() {
    let body = "        custom \"board\" \"Positional\" {\n            title \"Body\"\n            include *\n        }";
    let m = parse(&ws(BOARD, body));
    assert_eq!(m.view("board").unwrap().title.as_ref().unwrap().value, "Body");
    // Two title lines in the body: the last wins (P2-400), the first gets
    // a no-effect notice.
    let twice = "        custom \"board\" {\n            title \"A\"\n            title \"B\"\n            include *\n        }";
    let m = parse(&ws(BOARD, twice));
    assert_eq!(m.view("board").unwrap().title.as_ref().unwrap().value, "B");
    assert!(m.notices.iter().any(|n| n.subject.contains("`title`")), "{:?}", m.notices);
}

/// `include <id>` of a non-custom element in a custom view is
/// `NotCustomElement` naming the id.
#[test]
fn p2_317_custom_rejects_non_custom_element() {
    for id in ["operator", "web", "shop"] {
        let e = error(&ws(BOARD, &format!("        custom {{\n            include board {id}\n        }}")));
        assert!(
            matches!(&e.kind, ModelErrorKind::NotCustomElement { id: got } if got == id),
            "{e:?}"
        );
        assert!(e.to_string().contains(id), "{e}");
    }
    // Structurizr 6.2.4: an element type other than Custom is refused too.
    let e = error(&ws(BOARD, "        custom {\n            include \"element.type==Person\"\n        }"));
    assert!(matches!(&e.kind, ModelErrorKind::InvalidValue { value, .. } if value == "Person"), "{e:?}");
    // Other expressions keep only the custom elements (Structurizr 6.2.4:
    // `->operator` gives the board alone).
    let m = parse(&ws(BOARD, "        custom \"k\" {\n            include \"->operator\"\n        }"));
    assert_eq!(boxes(&m, &build(&m, "k")), ["board"]);
}

/// `include *` on a custom view selects every custom element and draws
/// the relationships between them.
#[test]
fn p2_318_custom_include_star_selects_custom_elements() {
    let m = parse(&ws(
        BOARD,
        "        custom \"all\" {\n            include *\n        }\n        custom \"less\" {\n            include *\n            exclude board\n        }\n        custom \"one\" {\n            include board\n        }\n        custom \"eff\" {\n            include \"board->\"\n        }",
    ));
    // Checked against Structurizr 6.2.4, view by view.
    assert_eq!(drawn(&m, "all"), (strs(&["board", "lonely", "pin"]), strs(&["board->pin"])));
    assert_eq!(drawn(&m, "less"), (strs(&["lonely", "pin"]), Vec::new()));
    assert_eq!(drawn(&m, "one"), (strs(&["board"]), Vec::new()));
    assert_eq!(drawn(&m, "eff"), (strs(&["board", "pin"]), strs(&["board->pin"])));
}

/// Box size and type line of a custom element.
// TODO(srs): P2-319. Reading taken (B landscape-filtered 12): the
// software system's CORE-2 default size; type line `[<metadata>]` when it
// has metadata, else none.
#[test]
fn p2_319_custom_element_box_size_and_type_line() {
    let m = parse(&ws(BOARD, "        custom \"all\" {\n            include *\n        }\n        systemLandscape \"land\" {\n            include *\n        }"));
    let b = build(&m, "all");
    let land = build(&m, "land");
    let shop = &land.scene.boxes[land.plan.boxes_of(m.find("shop").unwrap())[0].0 as usize];
    for k in ["board", "pin"] {
        let bx = &b.scene.boxes[b.plan.boxes_of(m.find(k).unwrap())[0].0 as usize];
        // The software system's CORE-2 size.
        assert_eq!((bx.rect.width(), bx.rect.height()), (shop.rect.width(), shop.rect.height()), "{k}");
    }
    let board = &b.scene.boxes[b.plan.boxes_of(m.find("board").unwrap())[0].0 as usize];
    assert_eq!(board.type_line.lines, ["[Whiteboard]"]);
    // No metadata: no type line at all (Structurizr's renderer).
    let pin = &b.scene.boxes[b.plan.boxes_of(m.find("pin").unwrap())[0].0 as usize];
    assert!(pin.type_line.lines.is_empty(), "{:?}", pin.type_line.lines);
}

/// `image <*|element> ["key"] { image <file> }` with a local PNG or SVG
/// builds a scene holding the image, title, subtitle and an empty key;
/// the SVG export embeds it; the body accepts `default`, `title`,
/// `description` and `properties`.
#[test]
fn p2_320_image_view_local_file_in_scene() {
    let src = ws(
        BOARD,
        "        image * \"png\" {\n            image pics/pic.png\n            title \"Picture\"\n            default\n            description \"Drawn\"\n            properties {\n                a b\n            }\n        }\n        image shop \"svg\" {\n            image pic.svg\n        }",
    );
    let parsed = parse_files(&src, &mut image_files()).expect("parses");
    let m = &parsed.model;
    let png_view = m.view("png").unwrap();
    assert_eq!(png_view.kind, ViewKind::Image);
    assert!(matches!(png_view.scope, ViewScope::Wildcard(_)));
    assert!(matches!(&png_view.image.as_ref().unwrap().source, ImageSource::File(f) if f.value == "pics/pic.png"));
    assert!(m.assets.iter().any(|a| a.path == "pics/pic.png"));
    assert!(matches!(m.view("svg").unwrap().scope, ViewScope::Element(_)));
    let b = build(m, "png");
    assert!(b.plan.boxes.is_empty() && b.plan.edges.is_empty() && b.plan.boundaries.is_empty());
    let image = b.scene.image.as_ref().expect("picture");
    // Intrinsic size.
    assert_eq!((image.rect.width(), image.rect.height()), (4, 3));
    assert_eq!(b.plan.title.as_deref(), Some("Picture"));
    assert!(b.scene.title.is_some());
    // An empty key.
    assert!(b.scene.legend.entries.is_empty());
    let s = build(m, "svg");
    let image = s.scene.image.as_ref().expect("picture");
    assert_eq!((image.rect.width(), image.rect.height()), (120, 80));
    // No title written: the file name (Structurizr 6.2.4).
    assert_eq!(s.plan.title.as_deref(), Some("pic.svg"));
    // The SVG export embeds the file.
    let svg = export::svg(&b.scene);
    // The 73-byte PNG, base64: its signature and IHDR open the URI.
    let uri = svg.split("href=\"").nth(1).expect("an href");
    assert!(uri.starts_with("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAAD"), "{uri}");
    let payload = &uri["data:image/png;base64,".len()..uri.find('"').unwrap()];
    assert_eq!(payload.len(), 100);
    // Other lines in an image body are refused.
    let e = parse_files(&image_ws("x", "image pic.svg\n            include *"), &mut image_files()).unwrap_err();
    assert!(matches!(&e.kind, ModelErrorKind::UnknownConstruct { construct } if construct == "include"), "{e:?}");
    // Canvas and PNG show the same image: the PNG export rasterizes the
    // SVG export, which holds it.
    #[cfg(feature = "png")]
    {
        let png = export::png(&b.scene, 200).expect("png");
        assert!(png.starts_with(b"\x89PNG"));
        let svg_png = export::png(&s.scene, 200).expect("png");
        assert!(svg_png.starts_with(b"\x89PNG"));
    }
    assert_roundtrip(&src);
}

/// `light { <source> }` and `dark { <source> }` in an image view.
#[test]
fn p2_320_image_view_light_and_dark_blocks() {
    // TODO(srs): P2-320 names no `light`/`dark` block. Reading taken
    // (Structurizr 6.2.4 opens corpus test.dsl, whose image view has
    // `light { image logo.png }` and `dark { ... }`): each block holds
    // one source line, checked as the plain source is, with a P2-3
    // notice; the plain source is drawn, else the `light` one, else the
    // `dark` one.
    let both = "light {\n                image pics/pic.png\n            }\n            dark {\n                image pics/pic.png\n            }\n            image pic.svg";
    let parsed = parse_files(&image_ws("pic", both), &mut image_files()).expect("parses");
    let v = parsed.model.view("pic").unwrap();
    assert!(matches!(&v.image.as_ref().unwrap().source, ImageSource::File(f) if f.value == "pic.svg"));
    let subjects: Vec<&str> = parsed.model.notices.iter().map(|n| n.subject.as_str()).collect();
    assert_eq!(subjects, ["light", "dark"]);
    let only_dark = "dark {\n                image pics/pic.png\n            }";
    let parsed = parse_files(&image_ws("pic", only_dark), &mut image_files()).expect("parses");
    let v = parsed.model.view("pic").unwrap();
    assert!(matches!(&v.image.as_ref().unwrap().source, ImageSource::File(f) if f.value == "pics/pic.png"));
    // Anything but a source line in the block is refused.
    let e = error(&image_ws("pic", "light {\n                title \"T\"\n            }"));
    assert!(matches!(e.kind, ModelErrorKind::UnknownConstruct { .. }), "{e:?}");
}

/// `plantuml` and `mermaid` sources draw a placeholder naming the source
/// kind and the file, URL or key; `kroki` is parsed, draws nothing and
/// shows a P2-3 notice.
#[test]
fn p2_321_plantuml_mermaid_placeholder_kroki_notice() {
    for (source, text, subject) in [
        ("plantuml diagram.puml", "plantuml diagram.puml", "plantuml"),
        ("mermaid diagram.mmd", "mermaid diagram.mmd", "mermaid"),
        ("kroki graphviz diagram.dot", "kroki graphviz diagram.dot", "kroki"),
        ("plantuml OtherView", "plantuml OtherView", "plantuml"),
    ] {
        let m = parse(&image_ws("pic", source));
        let b = build(&m, "pic");
        let image = b.scene.image.as_ref().expect("placeholder");
        assert_eq!(image.placeholder.as_deref(), Some(text));
        assert!(image.image.asset.is_none());
        let svg = export::svg(&b.scene);
        assert!(svg.contains(&format!("{text} (not drawn)")), "{svg}");
        let n: Vec<_> = m.notices.iter().filter(|n| n.subject == subject).collect();
        assert_eq!(n.len(), 1, "{source}");
        assert_eq!(n[0].kind, NoticeKind::NoEffect);
        assert_eq!(n[0].view.as_deref(), Some("pic"));
    }
    let src = image_ws("pic", "kroki graphviz diagram.dot");
    let v = parse(&src);
    assert!(matches!(&v.views[0].image.as_ref().unwrap().source, ImageSource::Kroki { format, source } if format.value == "graphviz" && source.value == "diagram.dot"));
}

/// Image view paths, errors, size, URLs, scope and enterability.
// TODO(srs): P2-322. Readings taken (B landscape-filtered 10): (a)
// relative to the diagram file's directory, `PathOutsideRoot` outside it;
// (b) missing `FileRead`, not PNG/SVG `FileFormat`, naming the path; (c)
// intrinsic size under the title; (d) an `https://` source draws a
// placeholder with a P2-3 notice; (e) `*` has no scope; (f) an
// element-scoped image view makes the element enterable when it has no
// container or component view.
#[test]
fn p2_322_image_view_paths_errors_size_scope() {
    // (a) outside the diagram's folder.
    let e = parse_files(&image_ws("pic", "image ../outside.png"), &mut image_files()).unwrap_err();
    assert!(matches!(&e.kind, ModelErrorKind::PathOutsideRoot { path } if path.contains("outside.png")), "{e:?}");
    // (b) missing, and not an image.
    let e = parse_files(&image_ws("pic", "image nowhere.png"), &mut image_files()).unwrap_err();
    assert!(matches!(&e.kind, ModelErrorKind::FileRead { path, .. } if path.contains("nowhere.png")), "{e:?}");
    let e = parse_files(&image_ws("pic", "image notes.txt"), &mut image_files()).unwrap_err();
    assert!(matches!(&e.kind, ModelErrorKind::FileFormat { path, .. } if path == "notes.txt"), "{e:?}");
    assert!(e.to_string().contains("notes.txt"));
    // (c) intrinsic size, title below.
    let b = build_files(&image_ws("pic", "image pics/pic.png"), "pic");
    let image = b.scene.image.as_ref().unwrap();
    assert_eq!(image.rect.min, c4rizer_core::geom::Point::new(0, 0));
    assert!(b.scene.title.as_ref().unwrap().bounds.min.y >= image.rect.max.y);
    // (d) a URL draws a placeholder naming it, with a notice; no fetch.
    let m = parse(&image_ws("pic", "image https://example.com/pic.png"));
    let b = build(&m, "pic");
    assert_eq!(b.scene.image.as_ref().unwrap().placeholder.as_deref(), Some("image https://example.com/pic.png"));
    assert!(m.notices.iter().any(|n| n.subject.contains("https://example.com/pic.png")), "{:?}", m.notices);
    // (e) `*` has no scope, an element scopes it.
    let m = parse(&ws(BOARD, "        image * \"a\" {\n            plantuml x.puml\n        }\n        image shop \"b\" {\n            plantuml x.puml\n        }"));
    assert!(matches!(m.view("a").unwrap().scope, ViewScope::Wildcard(_)));
    assert_eq!(m.view("b").unwrap().scope.element(), m.find("shop"));
    // Parsed with no file path: kept as written, a placeholder.
    let m = parse(&image_ws("pic", "image pic.svg"));
    let b = build(&m, "pic");
    assert_eq!(b.scene.image.as_ref().unwrap().placeholder.as_deref(), Some("image pic.svg"));
    // A second source line is refused.
    let e = error(&image_ws("pic", "plantuml a.puml\n            mermaid b.mmd"));
    assert!(matches!(e.kind, ModelErrorKind::DuplicateLine { .. }), "{e:?}");
}

/// The SVG export of an image view passes the EXP-1 scan, the image
/// carried as a data URI.
// TODO(srs): P2-340, SVG source with a script or external reference.
// Reading taken (B landscape-filtered 11): embedded as a
// `data:image/svg+xml;base64` `<image>`; the scan checks the exported
// document only.
#[test]
fn p2_340_image_view_svg_passes_exp_1_scan() {
    // An SVG source holding a script and an external reference is carried
    // only inside the base64 data URI.
    let evil = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><script>alert(1)</script><image href=\"https://example.com/x.png\"/></svg>";
    let mut files = image_files();
    files.files.insert(PathBuf::from("/project/evil.svg"), evil.as_bytes().to_vec());
    let parsed = parse_files(&image_ws("pic", "image evil.svg"), &mut files).expect("parses");
    let b = build(&parsed.model, "pic");
    let svg = export::svg(&b.scene);
    assert!(!svg.contains("<script"), "{svg}");
    assert!(!svg.contains("https://"), "{svg}");
    assert!(!svg.contains("<foreignObject") && !svg.contains("url("));
    // The only href is the data URI.
    let hrefs: Vec<&str> = svg.match_indices("href=\"").map(|(i, _)| &svg[i + 6..]).collect();
    assert_eq!(hrefs.len(), 1);
    assert!(hrefs[0].starts_with("data:image/svg+xml;base64,"));
    // The only other URI is the namespace.
    let uris = svg.matches("http://").count();
    assert_eq!(uris, 1);
}

/// `image <element>` with an undeclared id is `UndeclaredId` naming it.
#[test]
fn p2_343_image_unknown_element_is_error() {
    let e = error(&ws(BOARD, "        image nobody \"pic\" {\n            plantuml x.puml\n        }"));
    assert!(matches!(&e.kind, ModelErrorKind::UndeclaredId { id } if id == "nobody"), "{e:?}");
    assert!(e.to_string().contains("nobody"));
}

// ---- View statements beyond phase 1 (P2-20)

/// `include *?`, expressions with `&&`/`||`, relationship patterns,
/// `default` and view `properties` parse and round-trip (DSL-11).
#[test]
fn p2_20_view_statements_parse_and_roundtrip() {
    let src = ws(
        PROBED,
        "        container shop \"c2\" \"C2\" {\n            include *? \"->web\" \"element.type==Container && element.parent==shop\" r1\n            exclude \"relationship.tag==Tag 2\" \"* -> mail\" r1\n            default\n            properties {\n                \"structurizr.groups\" false\n            }\n        }",
    );
    let parsed = model::parse(&src).expect("parses");
    let v = &parsed.model.views[0];
    assert_eq!(v.include.len(), 4);
    assert_eq!(v.include[0].value, Selector::AllReluctant);
    assert!(matches!(v.include[1].value, Selector::Expression(model::Expression::Afferent(_))));
    assert!(matches!(&v.include[2].value, Selector::Expression(model::Expression::And(p)) if p.len() == 2));
    assert!(matches!(v.include[3].value, Selector::Relationship(_)));
    assert!(matches!(v.exclude[0].value, Selector::Expression(model::Expression::RelationshipTag { .. })));
    assert!(matches!(v.exclude[1].value, Selector::Expression(model::Expression::Relationships { source: None, .. })));
    // Each entry's span is the token as written.
    assert_eq!(&src[v.include[1].span.range()], "\"->web\"");
    assert!(v.default.is_some());
    assert_eq!(v.properties.len(), 1);
    assert_roundtrip(&src);
}

/// `*?` on C1, C2 and C3 selects the elements of `*` but draws only the
/// relationships to or from the scope (C1) or its children (C2, C3).
// TODO(srs): P2-323, `*?` on other kinds. Reading taken (B
// landscape-filtered 15; Structurizr 6.2.4 calls `addDefaultElements` on
// every static view): the same as `*`.
#[test]
fn p2_323_reluctant_wildcard_scope_relationships_only() {
    let m = probed(&[
        ("systemContext shop", "c1r", "            include *?"),
        ("systemContext shop", "c1", "            include *"),
        ("container shop", "c2r", "            include *?"),
        ("component web", "c3r", "            include *?"),
        ("systemLandscape", "lr", "            include *?"),
        ("systemLandscape", "l", "            include *"),
    ]);
    // Same boxes as `*`.
    assert_eq!(drawn(&m, "c1r").0, drawn(&m, "c1").0);
    // C1: lines to or from the scope only (Structurizr 6.2.4).
    assert_eq!(drawn(&m, "c1r").1, strs(&["operator->shop", "shop->ledger", "shop->mail"]));
    assert!(drawn(&m, "c1").1.contains(&"operator->ledger".to_owned()));
    // C2: lines touching the scope's containers.
    assert_eq!(
        drawn(&m, "c2r"),
        (
            strs(&["db", "ledger", "mail", "operator", "web"]),
            strs(&["db->ledger", "operator->web", "web->db", "web->mail"])
        )
    );
    // C3: lines touching the components.
    assert_eq!(
        drawn(&m, "c3r"),
        (strs(&["api", "auth", "db", "mail"]), strs(&["api->db", "api->mail", "auth->mail"]))
    );
    // Any other kind: the same as `*`.
    assert_eq!(drawn(&m, "lr"), drawn(&m, "l"));
}

/// `->x`, `x->`, `->x->` and the `element==` forms add x and its
/// afferent/efferent couplings; in `exclude` they remove them; a nested
/// element's relationship counts for its ancestors.
#[test]
fn p2_324_coupling_expressions() {
    let m = probed(&[
        ("container shop", "aff", "            include \"->web\""),
        ("container shop", "aff-eq", "            include \"element==->web\""),
        ("container shop", "eff", "            include \"web->\""),
        ("container shop", "both", "            include \"->web->\""),
        ("container shop", "nested", "            include \"->mail\""),
        ("container shop", "exc", "            include *\n            exclude \"->ledger\""),
    ]);
    assert_eq!(drawn(&m, "aff"), (strs(&["operator", "web"]), strs(&["operator->web"])));
    assert_eq!(drawn(&m, "aff-eq"), drawn(&m, "aff"));
    // web-> : web, db (api -> db) and mail (api -> mail), as relationships
    // of nested elements count for their ancestors.
    assert_eq!(
        drawn(&m, "eff"),
        (strs(&["db", "mail", "web"]), strs(&["web->db", "web->mail"]))
    );
    assert_eq!(drawn(&m, "both").0, strs(&["db", "mail", "operator", "web"]));
    // Structurizr 6.2.4 draws the same: api and auth count for web.
    assert_eq!(drawn(&m, "nested"), (strs(&["mail", "operator", "web"]), strs(&["operator->mail", "operator->web", "web->mail"])));
    // In exclude they remove the element and its couplings.
    assert_eq!(drawn(&m, "exc").0, strs(&["mail", "web"]));
}

/// A coupled element nested below the view's level.
// TODO(srs): P2-325. Reading taken (B landscape-filtered 13): drawn as
// its nearest ancestor the view kind draws, as `include *` does (CORE-5);
// dropped when it has none.
#[test]
fn p2_325_nested_coupled_element_drawn_as_ancestor() {
    let m = probed(&[
        ("systemLandscape", "land", "            include \"->mail\""),
        ("systemContext shop", "c1", "            include \"element.type==Component\""),
        ("container shop", "c2", "            include \"element.type==Component\""),
        ("component web", "c3", "            include \"element.type==Container\""),
    ]);
    // api and auth are drawn as shop on a landscape view.
    assert_eq!(drawn(&m, "land").0, strs(&["mail", "operator", "shop"]));
    assert_eq!(drawn(&m, "c1").0, strs(&["shop"]));
    // On a container view of shop, components are drawn as their
    // container.
    assert_eq!(drawn(&m, "c2").0, strs(&["web"]));
    // The scope container of a component view is its boundary, not a
    // box; the other container is drawn.
    assert_eq!(drawn(&m, "c3").0, strs(&["db"]));
}

/// `element.type==` matches `Person`, `SoftwareSystem`, `Container`, `Component`
/// and `Custom`; deployment types match nothing in phase 2.
#[test]
fn p2_326_element_type_expression() {
    let src = ws(
        &format!("{PROBED}\n        board = element \"Board\" \"Whiteboard\" \"A board.\""),
        "        systemLandscape \"people\" {\n            include \"element.type==Person\"\n        }\n        systemLandscape \"systems\" {\n            include \"element.type==softwaresystem\"\n        }\n        container shop \"containers\" {\n            include \"element.type==Container\"\n        }\n        component web \"components\" {\n            include \"element.type==Component\"\n        }\n        systemLandscape \"nodes\" {\n            include \"element.type==DeploymentNode\"\n        }",
    );
    let m = parse(&src);
    let inc = |k: &str| -> Vec<String> { m.included(m.view(k).unwrap()).iter().map(|&e| key(&m, e)).collect() };
    assert_eq!(inc("people"), ["operator"]);
    assert_eq!(inc("systems"), ["mail", "ledger", "shop"]);
    assert_eq!(inc("containers"), ["web", "db"]);
    assert_eq!(inc("components"), ["api", "auth"]);
    // A deployment type matches nothing in phase 2.
    assert!(inc("nodes").is_empty());
    let custom = m.select_elements(&model::Expression::ElementType {
        kind: Some(model::ElementKind::Custom),
        name: "Custom".into(),
    });
    assert_eq!(custom.len(), 1);
}

/// `element.parent==<id>` matches the direct children of id.
#[test]
fn p2_327_element_parent_expression() {
    let m = probed(&[
        ("container shop", "par", "            include \"element.parent==shop\""),
        ("component web", "par3", "            include \"element.parent==web\""),
    ]);
    assert_eq!(drawn(&m, "par"), (strs(&["db", "web"]), strs(&["web->db"])));
    assert_eq!(drawn(&m, "par3").0, strs(&["api", "auth"]));
    let e = error(&ws(PROBED, "        container shop {\n            include \"element.parent==nowhere\"\n        }"));
    assert!(matches!(&e.kind, ModelErrorKind::UndeclaredId { id } if id == "nowhere"), "{e:?}");
}

/// `element.tag==a,b` matches elements with every listed tag; `!=` those
/// without every listed tag.
#[test]
fn p2_328_element_tag_all_listed() {
    let m = probed(&[
        ("container shop", "all", "            include \"element.tag==Tag 2,Container\""),
        ("container shop", "one", "            include \"element.tag==Container\""),
        ("container shop", "not", "            include *\n            exclude \"element.tag!=Tag 2,Container\""),
    ]);
    assert_eq!(drawn(&m, "all").0, strs(&["web"]));
    assert_eq!(drawn(&m, "one").0, strs(&["db", "web"]));
    // `!=`: those without every listed tag are removed.
    assert_eq!(drawn(&m, "not").0, strs(&["web"]));
}

/// `element.technology==t` and `!=t`.
#[test]
fn p2_329_element_technology_expression() {
    let m = probed(&[
        ("container shop", "eq", "            include \"element.technology==Rust\""),
        ("container shop", "ne", "            include *\n            exclude \"element.technology!=Rust\""),
    ]);
    assert_eq!(drawn(&m, "eq").0, strs(&["web"]));
    // Structurizr 6.2.4: people and systems (no technology) are kept.
    assert_eq!(drawn(&m, "ne").0, strs(&["ledger", "mail", "operator", "web"]));
}

/// `element.properties[name]==value`.
#[test]
fn p2_330_element_properties_expression() {
    let src = ws(
        "        a = softwareSystem \"A\" {\n            properties {\n                tier gold\n            }\n        }\n        b = softwareSystem \"B\" {\n            properties {\n                tier silver\n            }\n        }\n        c = softwareSystem \"C\"",
        "        systemLandscape \"gold\" {\n            include \"element.properties[tier]==gold\"\n        }",
    );
    let m = parse(&src);
    let b = build(&m, "gold");
    assert_eq!(boxes(&m, &b), ["a"]);
}

/// `element.group==<name>`.
#[test]
fn p2_331_element_group_expression() {
    let src = ws(
        "        g = group \"Back office\" {\n            a = softwareSystem \"A\"\n            b = softwareSystem \"B\"\n        }\n        c = softwareSystem \"C\"\n        a -> c \"Calls\"",
        "        systemLandscape \"office\" {\n            include \"element.group==Back office\"\n        }\n        systemLandscape \"by-id\" {\n            include g\n        }",
    );
    let m = parse(&src);
    assert_eq!(boxes(&m, &build(&m, "office")), ["a", "b"]);
    // A group id selects its members, as Structurizr 6.2.4 does.
    assert_eq!(boxes(&m, &build(&m, "by-id")), ["a", "b"]);
    let e = error(&ws("        a = softwareSystem \"A\"", "        systemLandscape {\n            include g\n        }"));
    assert!(matches!(e.kind, ModelErrorKind::UndeclaredId { .. }), "{e:?}");
}

/// Each relationship expression form in `exclude` removes the matching
/// drawn lines and keeps every box.
#[test]
fn p2_332_exclude_relationship_expressions_keep_boxes() {
    let all = strs(&["db", "ledger", "mail", "operator", "web"]);
    let cases: [(&str, &[&str]); 12] = [
        ("*->*", &[]),
        ("relationship==*", &[]),
        ("relationship==*->*", &[]),
        ("web->*", &["db->ledger", "operator->ledger", "operator->mail", "operator->web"]),
        ("*->ledger", &["operator->mail", "operator->web", "web->db", "web->mail"]),
        ("relationship.tag==Tag 2", &["db->ledger", "operator->ledger", "operator->mail", "operator->web", "web->db"]),
        ("relationship.tag!=Tag 2", &["web->mail"]),
        ("relationship.source==operator", &["db->ledger", "web->db", "web->mail"]),
        ("relationship.destination==ledger", &["operator->mail", "operator->web", "web->db", "web->mail"]),
        ("relationship==web->*", &["db->ledger", "operator->ledger", "operator->mail", "operator->web"]),
        ("relationship==*->mail", &["db->ledger", "operator->ledger", "operator->web", "web->db"]),
        ("relationship==operator->web", &["db->ledger", "operator->ledger", "operator->mail", "web->db", "web->mail"]),
    ];
    let views: Vec<(String, String)> = cases
        .iter()
        .enumerate()
        .map(|(i, (x, _))| (format!("v{i}"), format!("            include *\n            exclude \"{x}\"")))
        .collect();
    let refs: Vec<(&str, &str, &str)> = views.iter().map(|(k, b)| ("container shop", k.as_str(), b.as_str())).collect();
    let m = probed(&refs);
    for (i, (x, want)) in cases.iter().enumerate() {
        let (bx, ls) = drawn(&m, &format!("v{i}"));
        assert_eq!(bx, all, "{x}: boxes kept");
        assert_eq!(ls, strs(want), "{x}");
    }
    // A relationship-tag line on a lifted line: Structurizr matches the
    // drawn (implied) relationship, whose source is web, not api.
    let m = probed(&[("container shop", "src", "            include web db\n            exclude \"relationship.source==api\"")]);
    assert_eq!(drawn(&m, "src").1, strs(&["web->db"]));
}

/// `exclude "<lhs> -> <rhs>"` removes drawn lines whose source box
/// matches lhs and destination box matches rhs, only among elements in
/// the view.
#[test]
fn p2_333_exclude_relationship_pattern() {
    let m = probed(&[
        ("container shop", "pat", "            include *\n            exclude \"* -> mail\""),
        ("container shop", "expr", "            include *\n            exclude \"web -> element.type==SoftwareSystem\""),
        ("container shop", "none", "            include *\n            exclude \"web -> element.type==Person\""),
        ("container shop", "outside", "            include web db\n            exclude \"operator -> *\""),
    ]);
    assert_eq!(
        drawn(&m, "pat").1,
        strs(&["db->ledger", "operator->ledger", "operator->web", "web->db"])
    );
    assert_eq!(drawn(&m, "pat").0.len(), 5);
    assert_eq!(
        drawn(&m, "expr").1,
        strs(&["db->ledger", "operator->ledger", "operator->mail", "operator->web", "web->db"])
    );
    assert_eq!(drawn(&m, "none").1.len(), 6);
    // Only among elements drawn in the view: nothing to remove, nothing
    // added.
    assert_eq!(drawn(&m, "outside"), (strs(&["db", "web"]), strs(&["web->db"])));
}

/// A relationship expression in `include` adds no box.
#[test]
fn p2_334_include_relationship_expression_adds_no_box() {
    let m = probed(&[
        ("container shop", "inc", "            include web\n            include \"relationship.source==web\""),
        ("container shop", "pat", "            include \"web -> *\""),
        ("container shop", "id", "            include r1"),
    ]);
    assert_eq!(drawn(&m, "inc"), (strs(&["web"]), Vec::new()));
    assert_eq!(drawn(&m, "pat"), (Vec::new(), Vec::new()));
    assert_eq!(drawn(&m, "id"), (Vec::new(), Vec::new()));
}

/// `a && b` matches what both match, `a || b` what either matches.
#[test]
fn p2_335_and_or_combine() {
    let m = probed(&[
        ("container shop", "and", "            include \"element.type==Container && element.parent==shop\""),
        ("container shop", "or", "            include \"element==web || element==operator\""),
        ("container shop", "rel", "            include *\n            exclude \"relationship.source==web && relationship.destination==db\""),
    ]);
    assert_eq!(drawn(&m, "and").0, strs(&["db", "web"]));
    assert_eq!(drawn(&m, "or"), (strs(&["operator", "web"]), strs(&["operator->web"])));
    assert!(!drawn(&m, "rel").1.contains(&"web->db".to_owned()));
    assert!(drawn(&m, "rel").1.contains(&"web->mail".to_owned()));
}

/// More than two operands, mixed operators, parentheses.
// TODO(srs): P2-336. Reading taken (B dsl-breadth 25 / landscape-filtered
// 14 as amended by Part C, Java decides). Structurizr 6.2.4
// `ExpressionParser.parseExpression` splits on " && " first, else on
// " || ", and evaluates only parts [0] and [1]: a mixed expression puts an
// operator inside one operand, which then fails to resolve, so it is
// refused (`MixedOperators`); no parentheses. Java silently drops a third
// operand of the same operator; c4rizer evaluates all of them (n-ary
// `And`/`Or`), pending the owner (DSL-4 forbids a silent drop).
#[test]
fn p2_336_operands_and_mixed_operators() {
    // Three operands: all are evaluated (Structurizr drops the third).
    let m = probed(&[("container shop", "three", "            include \"element==web || element==db || element==operator\"")]);
    assert_eq!(drawn(&m, "three").0, strs(&["db", "operator", "web"]));
    // Mixed: split on `&&` first, each side read whole; the `||` side is
    // no id, so it is an error naming it, as in Structurizr 6.2.4.
    let e = error(&ws(
        PROBED,
        "        container shop {\n            include \"element==web || element==db && element.type==Container\"\n        }",
    ));
    assert!(matches!(&e.kind, ModelErrorKind::UndeclaredId { id } if id.contains("||")), "{e:?}");
}

/// An expression with whitespace must be quoted; unquoted, it is an
/// error naming the token.
#[test]
fn p2_337_unquoted_whitespace_expression_is_error() {
    for (line, token) in [
        ("include element.type==Container && element.parent==shop", "&&"),
        ("include element==web || element==db", "||"),
        ("exclude * -> mail", "->"),
    ] {
        let e = error(&ws(PROBED, &format!("        container shop {{\n            include *\n            {line}\n        }}")));
        assert!(
            matches!(&e.kind, ModelErrorKind::InvalidValue { value, .. } if value == token),
            "{line}: {e:?}"
        );
        assert!(e.to_string().contains(token), "{e}");
    }
    // Quoted, the same text parses.
    parse(&ws(PROBED, "        container shop {\n            include \"element.type==Container && element.parent==shop\"\n        }"));
}

/// A view `properties` block is parsed, kept byte for byte and changes
/// nothing in the scene.
#[test]
fn p2_339_view_properties_kept_no_effect() {
    let with = ws(
        PROBED,
        "        container shop \"c2\" \"C2\" {\n            include *\n            properties {\n                \"some.name\" \"some value\"\n                other thing\n            }\n        }",
    );
    let without = ws(PROBED, "        container shop \"c2\" \"C2\" {\n            include *\n        }");
    let parsed = model::parse(&with).expect("parses");
    let props = &parsed.model.views[0].properties;
    assert_eq!(props.len(), 2);
    assert_eq!((props[0].name.value.as_str(), props[0].value.value.as_str()), ("some.name", "some value"));
    assert_roundtrip(&with);
    let (a, b) = (parse(&with), parse(&without));
    assert_eq!(build(&a, "c2").scene, build(&b, "c2").scene);
}

/// `include r` / `exclude r` for a named relationship adds or removes
/// its line, drawn only when both ends are in the view.
#[test]
fn p2_341_relationship_identifier_include_exclude() {
    let m = probed(&[
        ("container shop", "exc", "            include *\n            exclude r1"),
        ("container shop", "inc", "            include web mail\n            include r1"),
        ("component web", "exc3", "            include *\n            exclude r1"),
    ]);
    // Structurizr 6.2.4: the line r1 is drawn on goes, boxes stay.
    assert_eq!(
        drawn(&m, "exc"),
        (
            strs(&["db", "ledger", "mail", "operator", "web"]),
            strs(&["db->ledger", "operator->ledger", "operator->mail", "operator->web", "web->db"])
        )
    );
    assert_eq!(drawn(&m, "inc"), (strs(&["mail", "web"]), strs(&["web->mail"])));
    assert_eq!(drawn(&m, "exc3").1, strs(&["api->db", "auth->mail"]));
    let v = m.view("exc").unwrap();
    assert!(matches!(v.exclude[0].value, Selector::Relationship(r) if r == m.relationships[3].id));
}

// ---- Fixture (landscape-filtered-16)

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/landscape_filtered.dsl");

fn fixture() -> model::Parsed {
    let text = std::fs::read_to_string(FIXTURE).expect("fixture");
    model::parse_with(&text, Some(Path::new(FIXTURE)), &mut model::FsLoader).unwrap_or_else(|e| panic!("{e}"))
}

/// The fixture builds every view (P2-8, P2-9, P2-19, P2-20), with no
/// unsupported one, and the landscape opens by `default`.
#[test]
fn p2_8_fixture_renders_every_view() {
    let parsed = fixture();
    let all = pipeline::build_all(&parsed.model, &AtomicBool::new(false)).expect("built");
    assert!(all.unsupported.is_empty());
    let keys: Vec<&str> = all.views.iter().map(|v| v.scene.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "landscape",
            "landscape-tagged",
            "landscape-untagged",
            "landscape-all",
            "containers",
            "board",
            "shop-picture",
            "flow"
        ]
    );
    assert_eq!(parsed.model.default_view_key(), Some("landscape"));
    assert_eq!(parsed.model.filtered_bases(), ["landscape"]);
    for v in &all.views {
        assert!(export::svg(&v.scene).starts_with("<svg"), "{}", v.scene.key);
    }
    let picture = &all.views[6].scene;
    assert!(picture.image.as_ref().is_some_and(|i| i.placeholder.is_none()));
    assert!(all.views[7].scene.image.as_ref().is_some_and(|i| i.placeholder.is_some()));
}

/// Every filtered view's boxes are a subset of its base view's, at the
/// same centres; `include "Element,Relationship"` draws the base.
#[test]
fn p2_9_fixture_filtered_svg_subset_of_base() {
    let parsed = fixture();
    let m = &parsed.model;
    let base = build(m, "landscape");
    for k in ["landscape-tagged", "landscape-untagged", "landscape-all"] {
        let b = build(m, k);
        for (i, bx) in b.plan.boxes.iter().enumerate() {
            let at = base.plan.boxes_of(bx.element)[0];
            assert_eq!(b.scene.centres[i], base.scene.centres[at.0 as usize], "{k}");
            assert_eq!(b.scene.boxes[i].rect, base.scene.boxes[at.0 as usize].rect, "{k}");
        }
    }
    assert_eq!(boxes(m, &build(m, "landscape-tagged")), ["operator", "auditor", "mail", "shop"]);
    assert_eq!(boxes(m, &build(m, "landscape-untagged")), ["auditor", "mail", "ledger"]);
    let all = build(m, "landscape-all");
    assert_eq!(all.scene.boxes, base.scene.boxes);
    assert_eq!(all.scene.lines, base.scene.lines);
}

/// DSL-11 on the fixture.
#[test]
fn dsl_11_fixture_roundtrip() {
    let text = std::fs::read_to_string(FIXTURE).expect("fixture");
    assert_eq!(fixture().trees[0].print(), text);
}
