//! Tests of the explore row, core part (SRS 4.7: P3-500..P3-517). One
//! `#[test]` per requirement, named after it. The fixture is
//! `tests/data/phase3/explore.dsl`. The editor tests are in
//! `crates/editor/src/explore_tests.rs`.

use c4rizer_core::model::ElementKey;
use std::fmt::Write as _;

use c4rizer_core::explore::{self, RelationshipRow};
use c4rizer_core::inspect::{self, Inspection, Subject};
use c4rizer_core::model::{self, ElementId, Model};

/// The row's fixture.
const FIXTURE: &str = include_str!("../../../tests/data/phase3/explore.dsl");

fn parse(src: &str) -> Model {
    match model::parse(src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e:?}\n{src}"),
    }
}

/// The element `key` names.
fn id(m: &Model, key: &str) -> ElementId {
    m.find_written(key).unwrap_or_else(|| panic!("no element {key}"))
}

/// `source -> destination` by keys, with `(implied)` for implied rows.
fn pair(m: &Model, row: &RelationshipRow) -> String {
    let k = |e| m.full_key(e).unwrap_or_default();
    let mark = if row.implied { " (implied)" } else { "" };
    format!("{} -> {}{mark}", k(row.source), k(row.destination))
}

/// P3-501: an element's relationships include the implied ones, marked.
#[test]
fn p3_501_element_relationships_mark_implied() {
    // Structurizr's Model page lists, under `web`, the implied `web ->
    // bank` (from `pay -> bank`), marked "implied by".
    let m = parse(FIXTURE);
    let web = id(&m, "web");
    let rows = explore::relationship_rows(&m);
    let of_web: Vec<String> = rows
        .iter()
        .filter(|r| r.source == web || r.destination == web)
        .map(|r| pair(&m, r))
        .collect();
    assert!(of_web.contains(&"web -> bank (implied)".to_owned()), "{of_web:?}");
    assert!(of_web.contains(&"web -> db".to_owned()), "{of_web:?}");
    assert!(of_web.contains(&"customer -> web".to_owned()), "{of_web:?}");
    // The implied row names the relationship it is implied by.
    let implied = rows.iter().find(|r| pair(&m, r) == "web -> bank (implied)").expect("implied row");
    let by = m.relationship(implied.relationship);
    assert_eq!((by.source.value, by.destination.value), (id(&m, "pay"), id(&m, "bank")));
}

/// P3-502: the rows hold every written, copied and implied relationship.
#[test]
fn p3_502_relationship_rows_include_implied_and_copied() {
    let m = parse(FIXTURE);
    let rows = explore::relationship_rows(&m);
    let pairs: Vec<String> = rows.iter().map(|r| pair(&m, r)).collect();
    // Every model relationship (written and copied) once, in model order,
    // with its own id.
    let written: Vec<_> = rows.iter().filter(|r| !r.implied).map(|r| r.relationship).collect();
    let all: Vec<_> = m.relationships.iter().map(|r| r.id).collect();
    assert_eq!(written, all);
    assert!(m.relationships.iter().any(|r| r.copied_from.is_some()), "fixture has a copied relationship");
    assert!(pairs.contains(&"webLive -> dbLive".to_owned()), "{pairs:?}");
    // Implied ones after them (Structurizr's default strategy: one per
    // ancestor pair, unless any relationship already joins the pair).
    let first_implied = rows.iter().position(|r| r.implied).expect("implied rows");
    assert!(rows[first_implied..].iter().all(|r| r.implied));
    for want in ["customer -> shop", "web -> bank", "shop -> bank", "gate -> host", "host -> store"] {
        assert!(pairs.contains(&format!("{want} (implied)")), "{want}: {pairs:?}");
    }
    // Never to or from an ancestor, never twice, never over a written
    // relationship (`clerk -> shop` is written).
    for bad in ["cart -> web (implied)", "web -> shop (implied)", "clerk -> shop (implied)"] {
        assert!(!pairs.contains(&bad.to_owned()), "{bad}: {pairs:?}");
    }
    let mut seen = std::collections::BTreeSet::new();
    assert!(pairs.iter().all(|p| seen.insert(p.clone())), "a pair twice: {pairs:?}");
    // `!impliedRelationships false` before a relationship: nothing implied
    // from it.
    let off = parse(
        "workspace {\n model {\n a = softwareSystem \"A\" {\n x = container \"X\"\n }\n b = softwareSystem \"B\"\n !impliedRelationships false\n x -> b \"Uses\"\n }\n}\n",
    );
    assert!(explore::relationship_rows(&off).iter().all(|r| !r.implied));
}

/// P3-503: search matches name, id, description, technology, tag and
/// property value, ignoring case.
#[test]
fn p3_503_search_matches_every_field_case_insensitively() {
    let m = parse(FIXTURE);
    let keys = |hits: &explore::Hits| -> Vec<String> {
        hits.elements.iter().map(|&e| m.full_key(e).unwrap_or_default().into_string()).collect()
    };
    // Name, id, description, technology, tag, property value; any case.
    assert_eq!(keys(&explore::search(&m, "payMENTS")), ["pay"]);
    assert_eq!(keys(&explore::search(&m, "scanner")), ["scanner"]);
    assert_eq!(keys(&explore::search(&m, "old orders")), ["archive"]);
    assert_eq!(keys(&explore::search(&m, "postgres")), ["db"]);
    assert_eq!(keys(&explore::search(&m, "weblive")), ["webLive"]);
    assert_eq!(keys(&explore::search(&m, "legacy")), ["archive"]);
    assert_eq!(keys(&explore::search(&m, "dock 3")), ["sensor"]);
    // Relationships by description, technology and tag.
    let rels = |q: &str| -> Vec<String> {
        explore::search(&m, q)
            .relationships
            .iter()
            .map(|&r| {
                let r = m.relationship(r);
                format!("{} -> {}", m.full_key(r.source.value).unwrap_or_default(), m.full_key(r.destination.value).unwrap_or_default())
            })
            .collect()
    };
    assert_eq!(rels("charges CARDS"), ["pay -> bank"]);
    assert_eq!(rels("mqtt"), ["sensor -> shop"]);
    assert_eq!(rels("payment"), ["pay -> bank"]);
    // Empty text: everything.
    let all = explore::search(&m, "");
    assert_eq!(all.elements.len(), m.elements.len());
    assert_eq!(all.relationships.len(), m.relationships.len());
}

/// P3-503: a search over a 1000-element model takes under one frame.
#[test]
#[ignore = "perf: run with --release -- --ignored perf_"]
fn perf_p3_503() {
    if cfg!(debug_assertions) {
        eprintln!("perf_p3_503: skipped in a debug build");
        return;
    }
    let mut src = String::from("workspace {\n model {\n");
    for i in 0..1000 {
        let _ = writeln!(src, "  e{i} = softwareSystem \"System {i}\" \"Does thing {i}.\" \"Tag{}\"", i % 7);
    }
    for i in 0..999 {
        let _ = writeln!(src, "  e{i} -> e{} \"Calls {i}\" \"HTTPS\"", i + 1);
    }
    src.push_str(" }\n}\n");
    let m = parse(&src);
    let start = std::time::Instant::now();
    let mut hits = 0;
    for q in ["s", "sy", "sys", "syst", "system 99", "tag3", "calls 5"] {
        hits += explore::search(&m, q).elements.len();
    }
    let per = start.elapsed() / 7;
    assert!(hits > 0);
    println!("perf_p3_503: {:.2} ms per search", per.as_secs_f64() * 1e3);
    assert!(per.as_millis() < 16, "{per:?}");
}

/// P3-504: usages name each view drawing the element, directly or lifted.
#[test]
fn p3_504_usages_direct_and_lifted() {
    use c4rizer_core::explore::Drawn;
    let m = parse(FIXTURE);
    let show = |key: &str| -> Vec<String> {
        explore::usages(&m, id(&m, key))
            .iter()
            .map(|u| {
                let how = match u.drawn {
                    Drawn::Directly => "directly".to_owned(),
                    Drawn::Lifted { onto } => format!("lifted onto {}", m.full_key(onto).unwrap_or_default()),
                    _ => "other".to_owned(),
                };
                format!("{} ({}): {how}", u.view, u.title.as_deref().unwrap_or("-"))
            })
            .collect()
    };
    assert_eq!(
        show("pay"),
        [
            "landscape (Everything): lifted onto shop",
            "context (The shop and its users): lifted onto shop",
            "containers (Inside the shop): lifted onto web",
            "components (Inside the web app): directly",
        ]
    );
    assert_eq!(
        show("db"),
        [
            // `web -> db` stays inside `shop`: no line on the context view.
            "containers (Inside the shop): directly",
            "databases (The databases): directly",
            // Dynamic views draw their boxes since dynamic-view merged.
            "checkout (How a checkout runs): directly",
        ]
    );
    assert_eq!(show("archive"), Vec::<String>::new());
    assert_eq!(show("webLive"), ["live (The shop in production): directly"]);
}

/// P3-505: "in no view" is what `model.element.noview` reports.
#[test]
fn p3_505_in_no_view_matches_inspection() {
    for src in [FIXTURE, include_str!("../../../tests/data/structurizr/big-bank-plc.dsl"), include_str!("../../../tests/data/example.dsl")] {
        let m = parse(src);
        let reported: Vec<ElementId> = inspect::inspect(&m)
            .into_iter()
            .filter(|v| v.inspection == Inspection::ElementNoView)
            .filter_map(|v| match v.subject {
                Subject::Element(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(explore::in_no_view(&m), reported);
    }
    let m = parse(FIXTURE);
    assert_eq!(explore::in_no_view(&m), [id(&m, "archive")]);
}

/// P3-508: the force layout gives the same positions on every run.
#[test]
fn p3_508_graph_layout_is_deterministic() {
    let m = parse(FIXTURE);
    for key in ["landscape", "context", "containers", "components", "devices"] {
        let view = m.view(key).expect("view");
        let plan = c4rizer_core::view::plan(&m, view);
        let first = explore::graph_layout(&plan);
        assert_eq!(first.len(), plan.boxes.len(), "{key}");
        // Same positions on every run and on a fresh parse (CORE-25).
        assert_eq!(explore::graph_layout(&plan), first, "{key}");
        let again = parse(FIXTURE);
        let plan2 = c4rizer_core::view::plan(&again, again.view(key).expect("view"));
        assert_eq!(explore::graph_layout(&plan2), first, "{key}");
        // No two nodes on top of each other.
        for (i, a) in first.iter().enumerate() {
            for b in &first[i + 1..] {
                let d = f64::from(a.x - b.x).hypot(f64::from(a.y - b.y));
                assert!(d >= 60.0, "{key}: {a:?} {b:?}");
            }
        }
    }
    // A linked pair sits closer than an unlinked one on the landscape.
    let plan = c4rizer_core::view::plan(&m, m.view("landscape").expect("view"));
    let at = explore::graph_layout(&plan);
    let spot = |k: &str| at[plan.boxes_of(id(&m, k))[0].0 as usize];
    let d = |a: c4rizer_core::geom::Point, b: c4rizer_core::geom::Point| f64::from(a.x - b.x).hypot(f64::from(a.y - b.y));
    assert!(d(spot("shop"), spot("bank")) < d(spot("bank"), spot("clerk")));
}

/// P3-510: static and deployment trees.
#[test]
fn p3_510_tree_static_and_deployment() {
    use c4rizer_core::explore::TreeNode;
    fn lines(m: &Model, n: &TreeNode, depth: usize, out: &mut Vec<String>) {
        let key = n.element.and_then(|e| m.full_key(e)).map_or_else(|| "-".to_owned(), ElementKey::into_string);
        out.push(format!("{}{} ({key})", "  ".repeat(depth), n.label));
        for c in &n.children {
            lines(m, c, depth + 1, out);
        }
    }
    let m = parse(FIXTURE);
    let tree = |key: &str| {
        let mut out = Vec::new();
        lines(&m, &explore::tree(&m, m.view(key).expect("view")), 0, &mut out);
        out
    };
    // Static: people and systems as roots by name, then containers and
    // components by name, only what the view draws (and its ancestors).
    assert_eq!(
        tree("components"),
        [
            "components (-)",
            "  Bank (bank)",
            "  Shop (shop)",
            "    Web (web)",
            "      Cart (cart)",
            "      Payments (pay)",
        ]
    );
    assert_eq!(
        tree("context"),
        ["context (-)", "  Bank (bank)", "  Clerk (clerk)", "  Customer (customer)", "  Sensor (sensor)", "  Shop (shop)"]
    );
    // Deployment: environment > node > infrastructure node / instance.
    assert_eq!(
        tree("live"),
        [
            "Live (-)",
            "  Cloud (cloud)",
            "    Host (host)",
            "      Web (webLive)",
            "    Store (store)",
            "      Database (dbLive)",
            "    Gateway (gate)",
        ]
    );
}

/// P3-511: graph and tree are offered for Structurizr's kinds only.
#[test]
fn p3_511_graph_and_tree_offered_kinds() {
    use c4rizer_core::model::ViewKind;
    let graph = [ViewKind::Custom, ViewKind::SystemLandscape, ViewKind::SystemContext, ViewKind::Container, ViewKind::Component];
    for kind in graph {
        assert!(explore::graph_offered(kind) && explore::tree_offered(kind), "{kind:?}");
    }
    assert!(!explore::graph_offered(ViewKind::Deployment));
    assert!(explore::tree_offered(ViewKind::Deployment));
    for kind in [ViewKind::Dynamic, ViewKind::Filtered, ViewKind::Image] {
        assert!(!explore::graph_offered(kind) && !explore::tree_offered(kind), "{kind:?}");
    }
}

/// P3-516: decisions parse from adr-tools and MADR files.
#[test]
fn p3_516_decision_parsed_from_adr_tools_and_madr() {
    let adrs = "../../tests/data/phase3/explore/adrs";
    let read = |dir: &str, name: &str| std::fs::read_to_string(format!("{dir}/{name}")).expect("adr file");
    let show = |d: &explore::Decision| {
        let links: Vec<String> = d.links.iter().map(|l| format!("{} {}", l.kind, l.target)).collect();
        format!("{} | {} | {} | {} | {}", d.id, d.title, d.date.as_deref().unwrap_or("-"), d.status, links.join(", "))
    };
    // adr-tools (the default importer): id from the file name, title
    // without `N. `, `Date:`, the first word under `## Status`
    // (Structurizr's `AdrToolsDecisionImporter`), links in the status.
    let name = "0001-record-architecture-decisions.md";
    assert_eq!(
        show(&explore::parse_decision(name, &read(adrs, name), None)),
        "1 | Record architecture decisions | 2026-01-05 | Superseded | Superseded by 3"
    );
    let name = "0003-keep-decisions-in-the-repository.md";
    assert_eq!(
        show(&explore::parse_decision(name, &read(adrs, name), Some("adrtools"))),
        "3 | Keep decisions in the repository | 2026-02-02 | Accepted | Supersedes 1"
    );
    // No status: Proposed; "Superceded" is read as Superseded.
    let d = explore::parse_decision("0007-x.md", "# 7. X\n\n## Status\n\nSuperceded\n", None);
    assert_eq!(show(&d), "7 | X | - | Superseded | ");
    assert_eq!(explore::parse_decision("0000-y.md", "# Y\n", None).status, "Proposed");
    assert_eq!(explore::parse_decision("0000-y.md", "# Y\n", None).id, "0");
    // MADR with YAML front matter, log4brains with list metadata.
    let madr = "../../tests/data/structurizr/decisions/madr";
    let name = "0013-use-yaml-front-matter-for-meta-data.md";
    assert_eq!(
        show(&explore::parse_decision(name, &read(madr, name), Some("madr"))),
        "13 | Use YAML front matter for metadata | - | Proposed | "
    );
    let d = explore::parse_decision("0020-z.md", "---\nstatus: accepted\ndate: 2026-03-01\n---\n# Z\n", Some("madr"));
    assert_eq!(show(&d), "20 | Z | 2026-03-01 | accepted | ");
    let l4b = "../../tests/data/structurizr/decisions/log4brains";
    let name = "20240111-use-log4brains-to-manage-the-adrs.md";
    assert_eq!(
        show(&explore::parse_decision(name, &read(l4b, name), Some("log4brains"))),
        "20240111-use-log4brains-to-manage-the-adrs | Use Log4brains to manage the ADRs | 2024-01-10 | accepted | "
    );
}
