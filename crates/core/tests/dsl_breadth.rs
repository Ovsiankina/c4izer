//! Step-4 contract tests of the dsl-breadth row (SRS 4.6: P2-1..7,
//! P2-16..18, P2-120..181). One `#[test]` per requirement, named after
//! it; bodies are `todo!()` until step 5. Fixtures go under
//! `tests/data/dsl-breadth/`. The CLI tests are in
//! `crates/cli/tests/dsl_breadth.rs`, the editor tests in
//! `crates/editor/src/dsl_breadth_tests.rs`.
//!
//! Each `TODO(srs)` names the SRS hole and the reading taken (the
//! Structurizr-faithful one, from docs/plans/phase2-decisions.md Part B,
//! pending the owner's yes).

#![allow(clippy::todo)]

use c4rizer_core::model::ElementKey;
use c4rizer_core::model::{self, ElementKind, Model, ModelError, ModelErrorKind, Parsed};

/// `source` parsed, panicking with the error text.
fn parsed(source: &str) -> Parsed {
    model::parse(source).unwrap_or_else(|e| panic!("{e}\n{source}"))
}

/// The model of `source`.
fn model_of(source: &str) -> Model {
    parsed(source).model
}

/// The error of `source`, panicking when it parses.
fn error_of(source: &str) -> ModelError {
    match model::parse(source) {
        Ok(p) => panic!("expected an error, got {:?}", p.model),
        Err(e) => e,
    }
}

/// `body` inside `workspace { model { ... } }`.
fn in_model(body: &str) -> String {
    format!("workspace {{\n    model {{\n{body}\n    }}\n}}\n")
}

/// `model_body` and `views_body` inside one workspace.
fn in_workspace(model_body: &str, views_body: &str) -> String {
    format!(
        "workspace {{\n    model {{\n{model_body}\n    }}\n    views {{\n{views_body}\n    }}\n}}\n"
    )
}

/// An in-memory `Loader`: files by path, symlinks (link -> target) and an
/// environment, so the include and `${NAME}` tests stay hermetic.
#[derive(Default)]
struct Mem {
    files: std::collections::BTreeMap<std::path::PathBuf, String>,
    links: std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf>,
    env: std::collections::BTreeMap<String, String>,
}

impl Mem {
    fn file(mut self, path: &str, text: &str) -> Mem {
        self.files.insert(path.into(), text.into());
        self
    }

    fn var(mut self, name: &str, value: &str) -> Mem {
        self.env.insert(name.into(), value.into());
        self
    }

    /// `path` with `.` and `..` resolved and links followed.
    fn resolve(&self, path: &std::path::Path) -> std::path::PathBuf {
        let mut out = std::path::PathBuf::new();
        for c in path.components() {
            match c {
                std::path::Component::ParentDir => {
                    out.pop();
                }
                std::path::Component::CurDir => {}
                other => out.push(other),
            }
            if let Some(target) = self.links.get(&out) {
                out = self.resolve(target);
            }
        }
        out
    }

    fn is_dir(&self, path: &std::path::Path) -> bool {
        self.files.keys().any(|f| f.starts_with(path) && f != path)
    }
}

impl model::Loader for Mem {
    fn read(&mut self, path: &std::path::Path) -> std::io::Result<Vec<u8>> {
        let path = self.resolve(path);
        self.files
            .get(&path)
            .map(|t| t.as_bytes().to_vec())
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }

    fn read_dir(&mut self, path: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        let path = self.resolve(path);
        if !self.is_dir(&path) {
            return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
        }
        let mut out: Vec<std::path::PathBuf> = self
            .files
            .keys()
            .filter_map(|f| f.strip_prefix(&path).ok())
            .filter_map(|rest| rest.components().next())
            .map(|c| path.join(c))
            .collect();
        out.dedup();
        // Reversed, so core's sorting is what the test sees.
        out.reverse();
        Ok(out)
    }

    fn canonicalize(&mut self, path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
        let path = self.resolve(path);
        if self.files.contains_key(&path) || self.is_dir(&path) {
            Ok(path)
        } else {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
    }

    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }
}

/// `source` parsed as `path` through `loader`.
fn parsed_with(source: &str, path: &str, loader: &mut Mem) -> Parsed {
    model::parse_with(source, Some(std::path::Path::new(path)), loader)
        .unwrap_or_else(|e| panic!("{e}\n{source}"))
}

/// A model with people, a system with containers, and named
/// relationships, for the deployment and dynamic view tests.
const SHOP: &str = "        user = person \"User\"
        shop = softwareSystem \"Shop\" {
            web = container \"Web\"
            db = container \"Db\"
        }
        browses = user -> web \"Browses\"
        reads = web -> db \"Reads\"";

/// A deployment environment using every P2-120 construct.
const LIVE: &str = r#"        live = deploymentEnvironment "Live" {
            edge = deploymentGroup "Edge"
            group "Region" {
                aws = deploymentNode "AWS" "Cloud" "EC2" "Cloud" 2 {
                    lb = infrastructureNode "Load balancer" "Spreads load" "ELB" {
                        tags "Infra"
                        url https://example.com/lb
                    }
                    host = deploymentNode "Host" {
                        instances 3
                        technology "Linux"
                        properties {
                            zone a
                        }
                        webInstance = containerInstance web "Edge" "Tagged" {
                            healthCheck "Web up" https://example.com/health 60 1000
                            description "The web tier"
                        }
                        shopInstance = softwareSystemInstance shop {
                            perspectives {
                                Ops "Run by ops"
                            }
                        }
                        dbInstance = instanceOf db "Edge"
                    }
                }
            }
            lb -> webInstance "Forwards" "HTTPS"
            webInstance -/> dbInstance
        }"#;

/// The /dsl/archetypes example: element and relationship archetypes.
const ARCHETYPES: &str = r#"        archetypes {
            service = container {
                technology "Go"
                description "A service."
                tags "Service"
                properties {
                    tier backend
                }
            }
            api = service {
                technology "Go REST"
                tags "Api"
            }
            sync = -> {
                tags "Synchronous"
            }
            https = --sync-> {
                technology "HTTPS"
            }
            hardware = element {
                metadata "Hardware"
                tags "Kit"
            }
            staff = person {
                description "Works here."
            }
        }"#;

/// The expression fixture: the model of the Structurizr probe used to
/// check every expression form against Structurizr for Java.
const EXPRESSIONS: &str = r#"        properties {
            structurizr.groupSeparator /
        }
        u = person "U" "" "a"
        v = person "V" "" "a,b"
        team = group "G" {
            w = person "W" "" "b"
        }
        s = softwareSystem "S" {
            c1 = container "C1" "" "Java"
            c2 = container "C2" "" "Go" {
                properties {
                    tier back
                }
            }
        }
        t = softwareSystem "T"
        u -> c1 "uses"
        c1 -> c2 "calls" "" "x"
        c2 -> t "sends" "" "y,x"
        k = element "Kiosk""#;

/// The keys of the elements tagged `tag`.
fn tagged(m: &Model, tag: &str) -> Vec<String> {
    m.elements
        .iter()
        .filter(|e| e.has_tag(tag))
        .map(|e| e.key.as_ref().unwrap().value.clone())
        .collect()
}

/// A system context view over a user calling a container of `shop`, and
/// a container of `shop` calling a container of `mail`, with `directive`
/// at the top of `model`.
fn implied(directive: &str) -> String {
    in_workspace(
        &format!(
            "{directive}        user = person \"User\"\n        shop = softwareSystem \"Shop\" {{\n            web = container \"Web\"\n        }}\n        mail = softwareSystem \"Mail\" {{\n            smtp = container \"Smtp\"\n        }}\n        user -> web \"Browses\"\n        web -> smtp \"Sends\"\n        user -> shop \"Uses\""
        ),
        "        systemContext shop \"c1\" {\n            include *\n        }\n        systemContext shop \"c1-all\" {\n            include user shop mail\n        }\n        container shop \"c2\" {\n            include *\n        }",
    )
}

/// (included keys, drawn line count) of each view.
fn builds(m: &Model) -> Vec<(Vec<String>, usize)> {
    m.views
        .iter()
        .map(|v| {
            let keys = m
                .included(v)
                .iter()
                .map(|&e| m.element(e).key.as_ref().unwrap().value.clone())
                .collect();
            (keys, c4rizer_core::view::plan(m, v).edges.len())
        })
        .collect()
}

/// The scene of view `key`, built through the pipeline.
fn scene_of(m: &Model, key: &str) -> c4rizer_core::scene::Scene {
    let stop = std::sync::atomic::AtomicBool::new(false);
    c4rizer_core::pipeline::build_view(m, key, &c4rizer_core::place::Keep::default(), &stop)
        .unwrap_or_else(|e| panic!("{e}"))
        .scene
}

/// `terminology { ... }` in `views` over a person, an internal and an
/// external system, a container and a component, with views of each.
fn terminology(lines: &str) -> String {
    in_workspace(
        "        user = person \"User\"\n        shop = softwareSystem \"Shop\" {\n            web = container \"Web\" \"\" \"Rust\" {\n                api = component \"Api\"\n            }\n        }\n        bank = softwareSystem \"Bank\" \"\" \"External\"\n        user -> web \"Uses\"\n        api -> bank \"Pays\"",
        &format!(
            "        terminology {{\n{lines}\n        }}\n        systemContext shop \"c1\" {{\n            include *\n        }}\n        container shop \"c2\" {{\n            include *\n        }}\n        component web \"c3\" {{\n            include *\n        }}"
        ),
    )
}

/// The type line of each box of view `key`, by element key.
fn type_lines(m: &Model, key: &str) -> Vec<(String, String)> {
    scene_of(m, key)
        .boxes
        .iter()
        .map(|b| {
            (
                m.element(b.element).key.as_ref().unwrap().value.clone(),
                b.type_line.lines.join(" "),
            )
        })
        .collect()
}

/// The `extends` fixture parsed with `FsLoader`.
fn extends_fixture() -> Parsed {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data/dsl-breadth/extends/workspace.dsl");
    let source = std::fs::read_to_string(&path).unwrap();
    model::parse_with(&source, Some(&path), &mut model::FsLoader).unwrap_or_else(|e| panic!("{e}"))
}

/// A root file including `people.dsl` and declaring a view, parsed with
/// an in-memory loader.
fn included_pair() -> (String, String, Parsed) {
    let root = "workspace {\n    model {\n        !include people.dsl\n        shop = softwareSystem \"Shop\"\n        user -> shop \"Buys\"\n    }\n    views {\n        systemContext shop \"c1\" {\n            include *\n        }\n    }\n}\n".to_owned();
    let people = "user = person \"User\" \"Buys things.\"\n".to_owned();
    let mut loader = Mem::default().file("/w/people.dsl", &people);
    let p = parsed_with(&root, "/w/workspace.dsl", &mut loader);
    (root, people, p)
}

/// The element keyed `key`.
fn el<'m>(m: &'m Model, key: &str) -> &'m model::Element {
    m.element(m.find_written(key).unwrap_or_else(|| panic!("no element {key}")))
}

/// Element and relationship values without spans, to compare two parses.
fn values(m: &Model) -> Vec<String> {
    let opt = |l: &Option<model::Literal>| l.as_ref().map(|l| l.value.clone());
    let mut out: Vec<String> = m
        .elements
        .iter()
        .map(|e| {
            format!(
                "{:?} {:?} {} {:?} {:?} {:?} {:?}",
                e.kind,
                e.key.as_ref().map(|k| k.value.clone()),
                e.name.value,
                opt(&e.description),
                opt(&e.technology),
                e.tags,
                e.parent
            )
        })
        .collect();
    out.extend(m.relationships.iter().map(|r| {
        format!(
            "{:?}->{:?} {:?} {:?} {:?}",
            r.source.value,
            r.destination.value,
            opt(&r.description),
            opt(&r.technology),
            r.tags
        )
    }));
    out
}

/// `model::GroupId` is a `u32` newtype like `ElementId` (data layout,
/// owner sign-off pending). Serves P2-4.
#[test]
fn group_id_is_4_bytes() {
    assert_eq!(std::mem::size_of::<c4rizer_core::model::GroupId>(), 4);
}

// ---- Grammar and lexing (dsl-breadth-01, -02, -03, -20)

/// tests/data/dsl-breadth/grammar.dsl, parsed through `FsLoader`.
fn grammar() -> (std::path::PathBuf, Parsed) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data/dsl-breadth/grammar.dsl");
    let text = std::fs::read_to_string(&path).unwrap();
    let parsed = model::parse_with(&text, Some(&path), &mut model::FsLoader)
        .unwrap_or_else(|e| panic!("grammar.dsl: {e}"));
    (path, parsed)
}

/// Every P2-1 keyword, by block, as the fixture must write it: the
/// keyword and a snippet of the line it starts (checked in the text), so
/// a fixture edit that drops one fails here.
const P2_1_KEYWORDS: &[(&str, &str)] = &[
    // workspace
    ("workspace", "workspace extends grammar/base.dsl {"),
    ("workspace", "    name \"Grammar\""),
    ("workspace", "    description \"Every keyword"),
    ("workspace", "    properties {"),
    ("workspace", "    !identifiers flat"),
    ("workspace", "    !docs grammar/docs"),
    ("workspace", "    !adrs grammar/adrs"),
    ("workspace", "    model {"),
    ("workspace", "    views {"),
    ("workspace", "    configuration {"),
    // model
    ("model", "        !identifiers flat"),
    ("model", "        archetypes {"),
    ("model", "        group \"${ORG} partners\" {"),
    ("model", "    customer = person \"Customer\""),
    ("model", "        shop = softwareSystem \"Shop\""),
    ("model", "        live = deploymentEnvironment \"Live\" {"),
    ("model", "        scanner = element \"Scanner\""),
    ("model", "        customer -> web"),
    // element blocks
    ("element", "            description \"Sells goods online.\""),
    ("element", "                    technology \"Rust 2024\""),
    ("element", "            tags \"Internal\""),
    ("element", "            tag \"Core\""),
    ("element", "            url https://example.com/shop"),
    ("element", "            properties {"),
    ("element", "            perspectives {"),
    ("element", "                        group \"State\""),
    ("element", "            !docs grammar/docs"),
    ("element", "            !adrs grammar/adrs"),
    ("element", "                    !components {"),
    ("element", "                        -> pages \"Feeds\""),
    // directives
    ("directive", "    !const ORG"),
    ("directive", "    !var TEAM"),
    ("directive", "        !include grammar/people.dsl"),
    ("directive", "    !script groovy {"),
    ("directive", "    !plugin com.example.Plugin {"),
    ("directive", "        !element shop {"),
    ("directive", "        !elements \"element.tag==Core\" {"),
    ("directive", "        !relationship uses {"),
    ("directive", "        !relationships \"*->shop\" {"),
    ("directive", "        !extend web {"),
    ("directive", "        !ref uses {"),
    ("directive", "        !impliedRelationships true"),
    ("directive", "            !decisions grammar/adrs"),
    // views
    ("views", "        systemLandscape \"landscape\""),
    ("views", "        systemContext shop \"context\""),
    ("views", "        container shop \"containers\""),
    ("views", "        component web \"components\""),
    ("views", "        filtered \"landscape\""),
    ("views", "        dynamic shop \"flow\""),
    ("views", "        deployment * live \"live\""),
    ("views", "        custom \"board\""),
    ("views", "        image * \"picture\""),
    ("views", "        styles {"),
    ("views", "        theme default"),
    ("views", "        themes https://"),
    ("views", "        terminology {"),
    ("views", "        properties {"),
    // view bodies
    ("view", "            include *"),
    ("view", "            exclude scanner"),
    ("view", "            autoLayout lr"),
    ("view", "            default"),
    ("view", "            animation {"),
    ("view", "            title \"Shop context\""),
    ("view", "            description \"The shop and its users.\""),
    ("view", "            properties {"),
    // styles
    ("styles", "            light {"),
    ("styles", "            dark {"),
    ("styles", "            element \"Person\" {"),
    ("styles", "            relationship \"Relationship\" {"),
    // configuration
    ("configuration", "        scope softwaresystem"),
    ("configuration", "        visibility public"),
    ("configuration", "        users {"),
    ("configuration", "        properties {"),
];

/// tests/data/dsl-breadth/grammar.dsl (plus include and base files) uses every P2-1 keyword in each block the reference allows; `parse_with(FsLoader)` succeeds.
#[test]
fn p2_1_every_reference_keyword_parses() {
    let (path, parsed) = grammar();
    let texts: Vec<String> = parsed.trees.iter().map(c4rizer_core::text::SyntaxTree::print).collect();
    for (block, snippet) in P2_1_KEYWORDS {
        assert!(
            texts.iter().any(|t| t.lines().any(|l| l.starts_with(snippet))),
            "{block}: no line starting {snippet:?} in the grammar fixture"
        );
    }
    let m = &parsed.model;
    // Root, base, include: three files, the root first (P2-135).
    assert_eq!(parsed.trees.len(), 3);
    assert_eq!(parsed.paths[0], path);
    // The base's element and view are in the model (P2-16), and so is the
    // included file's (P2-5).
    assert!(m.find_written("legacy").is_some() && m.view("legacy").is_some());
    assert!(m.find_written("customer").is_some() && m.find_written("clerk").is_some());
    // `${ORG}` was substituted (P2-150), the custom element and the
    // archetype applied (P2-170, P2-155).
    assert!(m.groups.iter().any(|g| g.name.value == "Example partners"));
    assert_eq!(m.element(m.find_written("scanner").unwrap()).kind, ElementKind::Custom);
    assert!(m.element(m.find_written("api").unwrap()).tags.iter().any(|t| t == "Service"));
    // Every view kind is there; the drawable ones build, the others are
    // listed as unsupported (P2-2).
    let kinds: Vec<&str> = m.views.iter().map(|v| v.kind.keyword()).collect();
    for kind in [
        "systemLandscape",
        "systemContext",
        "container",
        "component",
        "filtered",
        "dynamic",
        "deployment",
        "custom",
        "image",
    ] {
        assert!(kinds.contains(&kind), "{kind}: {kinds:?}");
    }
    let never = std::sync::atomic::AtomicBool::new(false);
    let all = c4rizer_core::pipeline::build_all(m, &never).unwrap();
    assert_eq!(all.views.len() + all.unsupported.len(), m.views.len());
    assert!(all.views.iter().all(|v| !c4rizer_core::export::svg(&v.scene).is_empty()));
}

/// DSL-11 on every file of the grammar fixture: each tree prints back byte for byte.
#[test]
fn p2_1_grammar_fixture_round_trips() {
    let (_, parsed) = grammar();
    for (tree, path) in parsed.trees.iter().zip(&parsed.paths) {
        let on_disk = std::fs::read_to_string(path).unwrap();
        assert_eq!(tree.print(), on_disk, "{}", path.display());
    }
}

/// Style properties and shapes, light/dark, theme/themes, view expressions and relationship patterns, default, animation, Group/Group:Name style tags: no error, kept byte for byte.
#[test]
fn p2_178_other_row_keywords_accepted_and_kept() {
    let src = in_workspace(
        "        user = person \"User\"\n        shop = softwareSystem \"Shop\" {\n            web = container \"Web\"\n        }\n        user -> web \"Uses\"",
        r#"        container shop "c2" {
            include *
            include ->web element.tag==Web "user -> web"
            exclude relationship.tag==Old
            default
            animation {
                user
                web
            }
        }
        theme default
        themes https://example.com/a.json https://example.com/b.json
        styles {
            light {
                element "Person" {
                    background #ffffff
                }
            }
            dark {
                element "Person" {
                    background #000000
                }
            }
            element "Group" {
                shape RoundedBox
                icon icon.png
                width 300
                height 200
                stroke #333333
                strokeWidth 2
                fontSize 22
                border dashed
                opacity 50
                metadata false
                description false
                colour #ff0000
                properties {
                    key value
                }
            }
            element "Group:Team" {
                color #ffffff
            }
            relationship "Relationship" {
                thickness 2
                style dashed
                routing Orthogonal
            }
        }"#,
    );
    let p = parsed(&src);
    assert_eq!(p.trees[0].print(), src);
    let m = &p.model;
    // Accepted with no model effect: the view still includes what `*` gives.
    assert_eq!(m.views.len(), 1);
    assert_eq!(m.included(&m.views[0]).len(), 2);
    // The styles-themes keywords now take effect (P2-13, P2-21, P2-11):
    // `theme default` adds the bundled theme's styles before the file's
    // own, and the `light`/`dark` blocks and the shape are read.
    let own: Vec<_> = m.element_styles.iter().filter(|s| s.theme.is_none()).collect();
    assert_eq!(own.len(), 4);
    let group = own.iter().find(|s| s.tag.value == "Group").unwrap();
    assert_eq!(group.shape.map(|s| s.value), Some(model::Shape::RoundedBox));
}

/// A trailing `\` continues the statement; the model equals the one-line form.
#[test]
fn p2_147_line_continuation_joins_lines() {
    let one = in_model(
        "        a = person \"User\" \"Uses it.\" \"Staff\"\n        s = softwareSystem \"Shop\"\n        a -> s \"Buys\" \"HTTPS\"",
    );
    let continued = in_model(
        "        a = person \"User\" \\\n            \"Uses it.\" \\\n            \"Staff\"\n        s = softwareSystem \"Shop\"\n        a -> s \\\n            \"Buys\" \"HTTPS\"",
    );
    let p = parsed(&continued);
    assert_eq!(values(&model_of(&one)), values(&p.model));
    assert_eq!(p.trees[0].print(), continued);
    // The name literal still points at the bytes written.
    let a = el(&p.model, "a");
    assert_eq!(&continued[a.description.as_ref().unwrap().span.range()], "\"Uses it.\"");
}

/// A `"""` text block is accepted wherever a string is.
#[test]
fn p2_148_text_block_is_a_string_value() {
    // TODO(srs): P2-148 value rules (/dsl/text-blocks is 404). Reading taken (B dsl-breadth 1): common leading whitespace of non-blank lines removed, no final line break, no escapes.
    let src = in_model(
        "        a = person \"User\" \"\"\"\n            First line\n              second \\\"line\\\"\n            \"\"\"\n        s = softwareSystem \"\"\"Shop\"\"\"",
    );
    let m = model_of(&src);
    let a = el(&m, "a");
    assert_eq!(
        a.description.as_ref().unwrap().value,
        "First line\n  second \"line\""
    );
    assert!(src[a.description.as_ref().unwrap().span.range()].starts_with("\"\"\""));
    assert_eq!(el(&m, "s").name.value, "Shop");
}

/// `person User` gives the same value as `person "User"`.
#[test]
fn p2_149_unquoted_string_arguments() {
    let bare = model_of(&in_model(
        "        user = person User Operates Staff\n        shop = softwareSystem Shop\n        user -> shop Buys HTTPS",
    ));
    let quoted = model_of(&in_model(
        "        user = person \"User\" \"Operates\" \"Staff\"\n        shop = softwareSystem \"Shop\"\n        user -> shop \"Buys\" \"HTTPS\"",
    ));
    assert_eq!(values(&bare), values(&quoted));
    // A DSL-10 rewrite of a bare word writes a quoted literal in its place.
    let src = in_model("        user = person User");
    let applied = c4rizer_core::edit::apply(
        &parsed(&src),
        &[c4rizer_core::edit::Edit::SetText {
            target: c4rizer_core::edit::TextTarget::Element {
                element: ElementKey::from_written("user"),
                field: c4rizer_core::edit::ElementField::Name,
            },
            value: Some("Night operator".into()),
            expected: None,
        }],
    )
    .unwrap();
    assert!(applied.text.contains("user = person \"Night operator\""), "{}", applied.text);
}

/// `!const NAME value` substitutes every later `${NAME}`, inside quoted strings too; the literal's span stays the written bytes.
#[test]
fn p2_150_const_substitutes_in_tokens() {
    let src = "workspace {\n    !const NAME \"Shop\"\n    !const TECH_1.x Rust\n    model {\n        early = person \"${LATER}\"\n        s = softwareSystem \"${NAME} system\" {\n            api = container ${NAME}-api \"\" \"${TECH_1.x}\"\n        }\n    }\n}\n";
    let m = model_of(src);
    let s = el(&m, "s");
    assert_eq!(s.name.value, "Shop system");
    assert_eq!(&src[s.name.span.range()], "\"${NAME} system\"");
    let api = el(&m, "api");
    assert_eq!(api.name.value, "Shop-api");
    assert_eq!(api.technology.as_ref().unwrap().value, "Rust");
    // A reference before its `!const` is left as written (lines in order).
    assert_eq!(el(&m, "early").name.value, "${LATER}");
}

/// A quoted name in `!const "NAME" "v"` / `!var "NAME" "v"` is the same
/// name (corpus constants-and-variables-from-workspace-extension-parent.dsl;
/// Structurizr 6.2.4 opens it).
// TODO(srs): P2-150 shows only a bare name. Reading taken (Structurizr
// 6.2.4): a quoted name is accepted, its unquoted text checked as a bare
// one, in the builder and in `!include` paths alike.
#[test]
fn p2_150_quoted_const_and_var_names() {
    let m = model_of(&in_model(
        "        !const \"NAME\" \"Shop\"\n        !var \"DESC\" \"Sells.\"\n        s = softwareSystem \"${NAME}\" \"${DESC}\"",
    ));
    let s = el(&m, "s");
    assert_eq!(s.name.value, "Shop");
    assert_eq!(s.description.as_ref().unwrap().value, "Sells.");
    let e = error_of(&in_model("        !const \"BAD NAME\" \"v\""));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

/// A later `!var NAME other` redefines; each `${NAME}` takes the value in force at its line.
#[test]
fn p2_151_var_redefinition_applies_to_later_lines() {
    let m = model_of(&in_model(
        "        !var V \"one\"\n        a = person \"${V}\"\n        !var V \"two\"\n        b = person \"${V}\"",
    ));
    assert_eq!(el(&m, "a").name.value, "one");
    assert_eq!(el(&m, "b").name.value, "two");
}

/// Redefining a `!const` name is `ModelErrorKind::DuplicateConstant` naming both lines.
#[test]
fn p2_151_const_redefinition_is_error() {
    // TODO(srs): P2-151 whether redefining a `!const` is an error. Reading taken (B dsl-breadth 2): yes, DuplicateConstant.
    for second in ["!const", "!var"] {
        let src = in_model(&format!(
            "        !const N \"one\"\n        {second} N \"two\""
        ));
        let e = error_of(&src);
        let ModelErrorKind::DuplicateConstant {
            name,
            first_position,
            ..
        } = &e.kind
        else {
            panic!("{e:?}");
        };
        assert_eq!(name, "N");
        assert_eq!(first_position.line, 3);
        assert_eq!(e.position.line, 4);
        assert!(e.to_string().contains("3:9"), "{e}");
    }
}

/// A DSL-10 edit of a literal whose written bytes hold `${` is `EditErrorKind::Substituted` and no file changes.
#[test]
fn p2_151_edit_of_substituted_literal_is_refused() {
    // TODO(srs): P2-151 what a DSL-10 edit does to a `${...}` literal. Reading taken (B dsl-breadth 3): refused.
    use c4rizer_core::edit::{self, EditErrorKind};
    let src = in_model("        !const N \"Shop\"\n        s = softwareSystem \"${N} system\"");
    let e = edit::apply(
        &parsed(&src),
        &[edit::Edit::SetText {
            target: edit::TextTarget::Element {
                element: ElementKey::from_written("s"),
                field: edit::ElementField::Name,
            },
            value: Some("Store".into()),
            expected: None,
        }],
    )
    .unwrap_err();
    assert_eq!(e.kind, EditErrorKind::Substituted { name: "N".into() });
    assert!(e.to_string().contains('N'), "{e}");
}

/// `${NAME}` with no constant takes `Loader::env(NAME)`; with neither it is left as written.
#[test]
fn p2_152_env_var_and_unknown_name_left_as_written() {
    // TODO(srs): P2-152 precedence. Reading taken (B dsl-breadth 4): an earlier constant or variable wins over the environment.
    let src = "workspace {\n    !const SHADOW \"from const\"\n    model {\n        a = person \"${REGION}\"\n        b = person \"${NOWHERE}\"\n        c = person \"${SHADOW}\"\n    }\n}\n";
    let mut loader = Mem::default()
        .var("REGION", "north")
        .var("SHADOW", "from env");
    let m = parsed_with(src, "/w/workspace.dsl", &mut loader).model;
    assert_eq!(el(&m, "a").name.value, "north");
    assert_eq!(el(&m, "b").name.value, "${NOWHERE}");
    assert_eq!(el(&m, "c").name.value, "from const");
    // `parse` sees no environment.
    assert_eq!(el(&model_of(src), "a").name.value, "${REGION}");
}

// ---- Body statements and settings (dsl-breadth-03)

/// `tag "X"` in an element or relationship block appends one tag after the earlier ones (DSL-15).
#[test]
fn p2_18_tag_line_appends_one_tag() {
    let m = model_of(&in_model(
        "        a = person \"A\" \"\" \"First\" {\n            tag \"Second\"\n            tags \"Third,Fourth\"\n            tag \"Fifth\"\n        }\n        b = person \"B\"\n        a -> b \"Uses\" \"\" \"R1\" {\n            tag \"R2\"\n        }",
    ));
    assert_eq!(
        el(&m, "a").tags,
        ["Element", "Person", "First", "Second", "Third", "Fourth", "Fifth"]
    );
    assert_eq!(m.relationships[0].tags, ["Relationship", "R1", "R2"]);
}

/// `url <url>` sets `Element::url` and `Relationship::url`.
#[test]
fn p2_171_url_on_element_and_relationship() {
    let src = in_model(
        "        a = person \"A\" {\n            url https://example.com/a\n        }\n        b = person \"B\"\n        a -> b {\n            url \"https://example.com/r\"\n        }",
    );
    let m = model_of(&src);
    let url = el(&m, "a").url.as_ref().unwrap();
    assert_eq!(url.value, "https://example.com/a");
    assert_eq!(&src[url.span.range()], "https://example.com/a");
    assert_eq!(
        m.relationships[0].url.as_ref().unwrap().value,
        "https://example.com/r"
    );
}

/// workspace, model, configuration, element, relationship, view and views properties land in `workspace_properties`, `model_properties`, `configuration_properties`, `Element::properties`, `Relationship::properties`, `ViewSpec::properties`, `views_properties`, in file order.
#[test]
fn p2_172_properties_stored_in_order_per_owner() {
    let src = r#"workspace {
    properties {
        w1 one
        "w 2" "two"
    }
    model {
        properties {
            m1 one
        }
        a = person "A" {
            properties {
                e1 one
                e2 two
            }
        }
        b = softwareSystem "B"
        a -> b {
            properties {
                r1 one
            }
        }
    }
    views {
        properties {
            v1 one
        }
        systemContext b {
            include *
            properties {
                s1 one
            }
        }
    }
    configuration {
        properties {
            c1 one
        }
    }
}
"#;
    let m = model_of(src);
    let pairs = |ps: &[model::Property]| -> Vec<(String, String)> {
        ps.iter()
            .map(|p| (p.name.value.clone(), p.value.value.clone()))
            .collect()
    };
    let one = |n: &str| vec![(n.to_owned(), "one".to_owned())];
    assert_eq!(
        pairs(&m.workspace_properties),
        [("w1".into(), "one".into()), ("w 2".into(), "two".into())]
    );
    assert_eq!(pairs(&m.model_properties), one("m1"));
    assert_eq!(
        pairs(&el(&m, "a").properties),
        [("e1".into(), "one".into()), ("e2".into(), "two".into())]
    );
    assert_eq!(pairs(&m.relationships[0].properties), one("r1"));
    assert_eq!(pairs(&m.views_properties), one("v1"));
    assert_eq!(pairs(&m.views[0].properties), one("s1"));
    assert_eq!(pairs(&m.configuration_properties), one("c1"));
    assert_eq!(&src[m.workspace_properties[1].span.range()], "\"w 2\" \"two\"");
}

/// One-line and `perspective <name> { ... }` forms both fill Perspective name, description, value, url.
#[test]
fn p2_173_perspectives_inline_and_block() {
    let m = model_of(&in_model(
        r#"        a = person "A" {
            perspectives {
                "Security" "Encrypted at rest." "High"
                Ops "Run by ops"
                perspective "Cost" {
                    description "Pay per use."
                    value "Low"
                    url https://example.com/cost
                }
            }
        }
        b = person "B"
        a -> b {
            perspectives {
                "Security" "TLS"
            }
        }"#,
    ));
    let opt = |l: &Option<model::Literal>| l.as_ref().map(|l| l.value.clone());
    let got: Vec<_> = el(&m, "a")
        .perspectives
        .iter()
        .map(|p| (p.name.value.clone(), opt(&p.description), opt(&p.value), opt(&p.url)))
        .collect();
    assert_eq!(
        got,
        [
            ("Security".into(), Some("Encrypted at rest.".into()), Some("High".into()), None),
            ("Ops".into(), Some("Run by ops".into()), None, None),
            (
                "Cost".into(),
                Some("Pay per use.".into()),
                Some("Low".into()),
                Some("https://example.com/cost".into())
            ),
        ]
    );
    assert_eq!(m.relationships[0].perspectives[0].name.value, "Security");
}

/// `name` and `description` lines in the workspace block set `Model::name` and `Model::description`.
#[test]
fn p2_174_workspace_name_and_description_lines() {
    let m = model_of(
        "workspace {\n    name \"Shop\"\n    description \"Sells things.\"\n    model {\n    }\n}\n",
    );
    assert_eq!(m.name.as_ref().unwrap().value, "Shop");
    assert_eq!(m.description.as_ref().unwrap().value, "Sells things.");
}

/// `configuration { scope landscape|softwaresystem|none }` sets `Model::configuration_scope`.
#[test]
fn p2_175_configuration_scope_stored() {
    for (word, scope) in [
        ("landscape", model::ConfigurationScope::Landscape),
        ("softwaresystem", model::ConfigurationScope::SoftwareSystem),
        ("none", model::ConfigurationScope::None),
    ] {
        let m = model_of(&format!(
            "workspace {{\n    model {{\n        s = softwareSystem \"S\"\n    }}\n    configuration {{\n        scope {word}\n    }}\n}}\n"
        ));
        assert_eq!(m.configuration_scope.unwrap().value, scope, "{word}");
    }
    assert_eq!(model_of(&in_model("")).configuration_scope, None);
}

/// A model that breaks its configuration scope is `ModelErrorKind::ScopeViolation`.
#[test]
fn p2_175_scope_violation_is_error() {
    // TODO(srs): P2-175 the validation the scope applies. Reading taken (Structurizr's workspace-scope validators, checked; differs from B dsl-breadth 20): landscape fails on any system with containers or docs; softwaresystem when more than one has them; none checks nothing.
    let with = |scope: &str, model_body: &str| {
        format!(
            "workspace {{\n    model {{\n{model_body}\n    }}\n    configuration {{\n        scope {scope}\n    }}\n}}\n"
        )
    };
    let one_with_containers = "        s = softwareSystem \"S\" {\n            c = container \"C\"\n        }\n        t = softwareSystem \"T\"";
    let two_with_containers = "        s = softwareSystem \"S\" {\n            c = container \"C\"\n        }\n        t = softwareSystem \"T\" {\n            d = container \"D\"\n        }";
    let e = error_of(&with("landscape", one_with_containers));
    assert_eq!(
        e.kind,
        ModelErrorKind::ScopeViolation {
            scope: model::ConfigurationScope::Landscape,
            element: "s".into()
        }
    );
    assert!(e.to_string().contains("`s`"), "{e}");
    let e = error_of(&with("softwaresystem", two_with_containers));
    assert!(
        matches!(&e.kind, ModelErrorKind::ScopeViolation { element, .. } if element == "t"),
        "{e:?}"
    );
    // Structurizr's validators: one system with containers is fine under
    // softwaresystem, none is fine too, and `none` checks nothing.
    model_of(&with("softwaresystem", one_with_containers));
    model_of(&with("softwaresystem", "        s = softwareSystem \"S\""));
    model_of(&with("landscape", "        s = softwareSystem \"S\""));
    model_of(&with("none", two_with_containers));
}

/// structurizr.locale/timezone/sort/tooltips/title/description/metadata/groups of `views` are in `views_properties`.
#[test]
fn p2_176_view_set_properties_stored() {
    // TODO(srs): P2-176 one requirement per property for its effect (B dsl-breadth 21 lists the readings; effects belong to other rows).
    let names = [
        "structurizr.locale",
        "structurizr.timezone",
        "structurizr.sort",
        "structurizr.tooltips",
        "structurizr.title",
        "structurizr.description",
        "structurizr.metadata",
        "structurizr.groups",
    ];
    let lines: String = names
        .iter()
        .flat_map(|n| ["            ", n, " x\n"])
        .collect();
    let m = model_of(&in_workspace(
        "        s = softwareSystem \"S\"",
        &format!("        properties {{\n{lines}        }}"),
    ));
    let got: Vec<&str> = m.views_properties.iter().map(|p| p.name.value.as_str()).collect();
    assert_eq!(got, names);
}

// ---- Notices (dsl-breadth-04)

/// One source per P2-3 construct: (source, subject, line, view key).
fn notice_cases() -> Vec<(String, &'static str, u32, Option<&'static str>)> {
    vec![
        (
            in_model("        !script groovy {\n            workspace.name = \"x\" }\n        }\n        a = person \"A\""),
            "!script",
            3,
            None,
        ),
        (in_model("        !script scripts/setup.groovy"), "!script", 3, None),
        (
            "workspace {\n    !plugin com.example.Plugin {\n        key value\n    }\n}\n".into(),
            "!plugin",
            2,
            None,
        ),
        (
            in_model("        s = softwareSystem \"S\" {\n            c = container \"C\" {\n                !components {\n                    classes x.jar\n                }\n            }\n        }"),
            "!components",
            5,
            None,
        ),
        (
            in_model(&format!("{SHOP}\n{LIVE}")),
            "healthCheck",
            25,
            None,
        ),
        (
            "workspace {\n    configuration {\n        visibility public\n    }\n}\n".into(),
            "configuration visibility",
            3,
            None,
        ),
        (
            "workspace {\n    configuration {\n        users {\n            someone write\n        }\n    }\n}\n".into(),
            "configuration users",
            3,
            None,
        ),
        (
            in_workspace(SHOP, "        image * \"pic\" {\n            plantuml diagram.puml\n        }"),
            "plantuml",
            13,
            Some("pic"),
        ),
        (
            in_workspace(SHOP, "        image * \"pic\" {\n            mermaid diagram.mmd\n        }"),
            "mermaid",
            13,
            Some("pic"),
        ),
        (
            in_workspace(SHOP, "        image * \"pic\" {\n            kroki graphviz diagram.dot\n        }"),
            "kroki",
            13,
            Some("pic"),
        ),
        (
            in_workspace(SHOP, "        properties {\n            plantuml.url https://plantuml.example.com\n        }"),
            "plantuml.url",
            13,
            None,
        ),
        (
            in_workspace(SHOP, "        properties {\n            mermaid.format svg\n        }"),
            "mermaid.format",
            13,
            None,
        ),
        (
            in_workspace(SHOP, "        properties {\n            kroki.url https://kroki.example.com\n        }"),
            "kroki.url",
            13,
            None,
        ),
        (
            in_model("        !impliedRelationships com.example.Strategy"),
            "!impliedRelationships",
            3,
            None,
        ),
        (
            "workspace {\n    !docs docs com.example.DocsImporter\n}\n".into(),
            "!docs",
            2,
            None,
        ),
        (
            "workspace {\n    !adrs decisions com.example.AdrImporter\n}\n".into(),
            "!adrs",
            2,
            None,
        ),
    ]
}

/// One case per P2-3 construct: exactly one `Model::notices` entry, `NoticeKind::NoEffect`, naming the construct, its file and line; no other effect.
#[test]
fn p2_3_no_effect_constructs_yield_notices() {
    for (src, subject, line, view) in notice_cases() {
        let m = model_of(&src);
        assert_eq!(m.notices.len(), 1, "{src}: {:?}", m.notices);
        let n = &m.notices[0];
        assert_eq!(n.kind, model::NoticeKind::NoEffect);
        assert_eq!(n.subject, subject, "{src}");
        assert_eq!(n.position.unwrap().line, line, "{src}");
        assert_eq!(n.span.unwrap().file, c4rizer_core::text::FileId::ROOT);
        assert_eq!(n.view.as_deref(), view, "{src}");
        assert_eq!(
            n.to_string(),
            format!("{line}:{}: {subject} is accepted and has no effect", n.position.unwrap().column)
        );
    }
    // Notices come in file order, and a !script body is skipped whole.
    let m = model_of(&in_workspace(
        "        !script groovy {\n            not { dsl\n        }\n        a = person \"A\"",
        "        properties {\n            kroki.url x\n            plantuml.url y\n        }",
    ));
    let subjects: Vec<&str> = m.notices.iter().map(|n| n.subject.as_str()).collect();
    assert_eq!(subjects, ["!script", "kroki.url", "plantuml.url"]);
}

// ---- Deployment, dynamic and new view kinds (dsl-breadth-05)

/// Every P2-120 deployment construct parses; no element they declare is drawn or selected by a static `include *`.
// Phase 3 (P3-1, D2): deployment nodes, infrastructure nodes and instances
// are model elements now; a static view still selects none of them.
#[test]
fn p2_120_deployment_constructs_parse_and_are_not_drawn() {
    let src = in_workspace(
        &format!("{SHOP}\n{LIVE}"),
        "        container shop \"c2\" {\n            include *\n        }",
    );
    let p = parsed(&src);
    assert_eq!(p.trees[0].print(), src);
    let m = &p.model;
    // P3-1: what a deployment environment declares are elements of the
    // deployment kinds (2 nodes, 1 infrastructure node, 3 instances).
    assert_eq!(m.elements.len(), 10);
    assert!(m.elements[4..].iter().all(|e| e.environment.is_some()));
    // The written `lb -> webInstance` is kept; `reads` is copied to
    // `webInstance -> dbInstance` and removed again by `-/>`.
    assert_eq!(m.relationships.len(), 3);
    assert_eq!(
        m.included(&m.views[0]),
        vec![m.find_written("user").unwrap(), m.find_written("web").unwrap(), m.find_written("db").unwrap()]
    );
    assert_eq!(m.element(m.find_written("lb").unwrap()).kind, model::ElementKind::InfrastructureNode);
}

/// An undeclared id is `UndeclaredId` (DSL-5); an id already used is `DuplicateId`.
#[test]
fn p2_120_undeclared_or_duplicate_id_in_deployment_is_error() {
    let env = |inner: &str| {
        in_model(&format!(
            "{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            node = deploymentNode \"Node\" {{\n{inner}\n            }}\n        }}"
        ))
    };
    let e = error_of(&env("                softwareSystemInstance nobody"));
    assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "nobody".into() });
    let e = error_of(&env("                lb = infrastructureNode \"LB\"\n                lb -> ghost"));
    assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "ghost".into() });
    for dup in [
        "                web = infrastructureNode \"LB\"",
        "                node = infrastructureNode \"LB\"",
        "                LIVE = containerInstance web",
        "                browses = deploymentNode \"Inner\"",
    ] {
        let e = error_of(&env(dup));
        assert!(matches!(e.kind, ModelErrorKind::DuplicateId { .. }), "{dup}: {e:?}");
    }
}

/// Under `!identifiers hierarchical`, a deployment id is scoped by the
/// deployment environment and nodes around it, as static ids are by their
/// parents (P2-142): the same id under two parents, or in two
/// environments, is no duplicate, and a relationship end resolves inside
/// its block first, then outwards, then as a full id (corpus
/// hierarchical-identifiers-and-deployment-nodes-1/2/3,
/// deployment-groups-hierarchical, no-relationship; Structurizr 6.2.4
/// opens them).
// TODO(srs): P2-142 covers static ids only. Reading taken (Structurizr
// 6.2.4): deployment ids follow the same rule; an environment or node
// with no id still scopes its children.
#[test]
fn p2_142_hierarchical_ids_scope_deployment_children() {
    let m = model_of(&in_model(
        "        !identifiers hierarchical\n        ss = softwareSystem \"SS\" {\n            c = container \"C\"\n        }\n        deploymentEnvironment \"A\" {\n            g = deploymentGroup \"G\"\n            dn = deploymentNode \"DN\" {\n                dn1 = deploymentNode \"DN1\" {\n                    softwareSystemInstance ss\n                }\n                dn2 = deploymentNode \"DN2\"\n                dn1 -> dn2\n            }\n            dn1 = deploymentNode \"DN1\"\n            dn1 -> dn.dn2\n        }\n        deploymentEnvironment \"B\" {\n            g = deploymentGroup \"G\"\n        }\n        live = deploymentEnvironment \"Live\" {\n            ss = deploymentNode \"Node\" {\n                c = deploymentNode \"Inner\" {\n                    i = containerInstance ss.c\n                }\n                lb = infrastructureNode \"LB\"\n                c.i -> lb\n            }\n            ss.lb -> ss.c.i\n        }",
    ));
    assert!(m.find_written("ss").is_some());
    // Flat mode keeps one global register.
    let e = error_of(&in_model(
        "        deploymentEnvironment \"A\" {\n            g = deploymentGroup \"G\"\n        }\n        deploymentEnvironment \"B\" {\n            g = deploymentGroup \"G\"\n        }",
    ));
    assert!(matches!(e.kind, ModelErrorKind::DuplicateId { .. }), "{e:?}");
    // Hierarchical: a same-level duplicate is still one.
    let e = error_of(&in_model(
        "        !identifiers hierarchical\n        live = deploymentEnvironment \"Live\" {\n            dn = deploymentNode \"A\"\n            dn = deploymentNode \"B\"\n        }",
    ));
    assert!(matches!(e.kind, ModelErrorKind::DuplicateId { .. }), "{e:?}");
    // An id from another scope does not resolve by its short form.
    let e = error_of(&in_model(
        "        !identifiers hierarchical\n        live = deploymentEnvironment \"Live\" {\n            dn = deploymentNode \"A\" {\n                inner = deploymentNode \"I\"\n            }\n            other = deploymentNode \"B\"\n            other -> inner\n        }",
    ));
    assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "inner".into() });
}

/// `dynamic <*|system|container>` with ordered steps, relationship-id steps and parallel `{ }` blocks parses.
#[test]
fn p2_121_dynamic_view_body_parses() {
    let src = in_workspace(
        SHOP,
        r#"        dynamic shop "signup" "Signing up" {
            1: user -> web "Opens the page"
            2: web -> db "Stores"
            browses "Browses again"
            {
                {
                    reads
                }
                {
                    user -> web
                }
            }
            autoLayout lr
            title "Sign-up flow"
            description "How a user signs up."
            default
            properties {
                plantuml.sequenceDiagram true
            }
        }
        dynamic shop {
            user -> web
        }
        dynamic db {
            user -> web
        }"#,
    );
    let m = model_of(&src);
    let keys: Vec<&str> = m.views.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(keys, ["signup", "Dynamic-001", "Dynamic-002"]);
    let v = &m.views[0];
    assert_eq!(v.kind, model::ViewKind::Dynamic);
    // dynamic-view (P3-204, P3-206): a container step end needs a
    // system scope, so the view is scoped to `shop`; `*` is covered by
    // p2_124 and the P2-121 undeclared-id test.
    assert_eq!(v.scope.element(), m.find_written("shop"));
    assert_eq!(v.title.as_ref().unwrap().value, "Sign-up flow");
    assert_eq!(v.properties.len(), 1);
    assert_eq!(m.views[1].scope.element(), m.find_written("shop"));
    assert_eq!(m.views[2].scope.element(), m.find_written("db"));
}

/// A dynamic step may carry a `{ url ...; properties { } }` block, as in
/// corpus test.dsl; any other line there is refused (Structurizr 6.2.4
/// probed: "expected: url").
#[test]
fn p2_121_dynamic_step_block_takes_url_and_properties() {
    // dynamic-view (P3-206): a container end needs a system scope, so the
    // views are scoped to `shop`, as in p2_121_dynamic_view_body_parses.
    let src = in_workspace(
        SHOP,
        "        dynamic shop {\n            user -> web \"Opens\" {\n                url \"https://example.org\"\n                properties {\n                    \"Name\" \"Value\"\n                }\n            }\n            1: user -> web {\n            }\n        }",
    );
    let m = model_of(&src);
    assert_eq!(m.views[0].kind, model::ViewKind::Dynamic);
    let e = error_of(&in_workspace(
        SHOP,
        "        dynamic shop {\n            user -> web {\n                tag \"x\"\n            }\n        }",
    ));
    assert_eq!(e.kind, ModelErrorKind::UnknownConstruct { construct: "tag".into() }, "{e:?}");
}

/// An undeclared id in a dynamic view's scope or steps is `UndeclaredId`.
#[test]
fn p2_121_undeclared_id_in_dynamic_is_error() {
    for (views, id) in [
        ("        dynamic nowhere {\n        }", "nowhere"),
        ("        dynamic * {\n            user -> ghost\n        }", "ghost"),
        ("        dynamic * {\n            1: unnamed \"step\"\n        }", "unnamed"),
        ("        dynamic * {\n            {\n                ghost -> web\n            }\n        }", "ghost"),
    ] {
        let e = error_of(&in_workspace(SHOP, views));
        assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: id.into() }, "{views}");
    }
    let e = error_of(&in_workspace(SHOP, "        dynamic user {\n        }"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidScope { .. }), "{e:?}");
}

/// `deployment <*|system> <environment>` with every P2-179 body line parses; scope is `ViewScope::Deployment`.
#[test]
fn p2_179_deployment_view_parses() {
    let src = in_workspace(
        &format!("{SHOP}\n{LIVE}"),
        r#"        deployment * "Live" "everything" "All of it" {
            include *
            exclude lb
            autoLayout
            default
            animation {
                aws
                lb webInstance
            }
            title "Live"
            description "Production."
            properties {
                structurizr.groups false
            }
        }
        deployment shop live {
            include *
        }"#,
    );
    let m = model_of(&src);
    let v = &m.views[0];
    assert_eq!((v.kind, v.key.as_str()), (model::ViewKind::Deployment, "everything"));
    let model::ViewScope::Deployment {
        software_system,
        environment,
    } = &v.scope
    else {
        panic!("{:?}", v.scope);
    };
    assert_eq!((*software_system, environment.value.as_str()), (None, "Live"));
    let model::ViewScope::Deployment {
        software_system,
        environment,
    } = &m.views[1].scope
    else {
        panic!("{:?}", m.views[1].scope);
    };
    assert_eq!(software_system.unwrap().value, m.find_written("shop").unwrap());
    assert_eq!(environment.value, "live");
    assert_eq!(m.views[1].key, "Deployment-001");
}

/// An undeclared scope or environment is `UndeclaredId`.
#[test]
fn p2_179_undeclared_environment_is_error() {
    let model_body = format!("{SHOP}\n{LIVE}");
    let e = error_of(&in_workspace(&model_body, "        deployment * \"Staging\" {\n        }"));
    assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "Staging".into() });
    let e = error_of(&in_workspace(&model_body, "        deployment ghost \"Live\" {\n        }"));
    assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "ghost".into() });
    let e = error_of(&in_workspace(
        &model_body,
        "        deployment * \"Live\" {\n            include nobody\n        }",
    ));
    assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "nobody".into() });
}

/// Keyless systemLandscape, dynamic and deployment views get SystemLandscape-NNN, Dynamic-NNN, Deployment-NNN, per kind in order.
#[test]
fn p2_124_generated_keys_for_new_view_kinds() {
    // TODO(srs): P2-124 keys for custom, image and filtered. Reading taken (B dsl-breadth 22): Custom-NNN, Image-NNN, Filtered-NNN.
    let m = model_of(&in_workspace(
        &format!("{SHOP}\n{LIVE}"),
        r#"        systemLandscape {
            include *
        }
        systemLandscape "SystemLandscape-002" {
            include *
        }
        systemLandscape {
            include *
        }
        dynamic * {
            user -> shop
        }
        deployment * "Live" {
            include *
        }
        custom "Custom-001" "Written" {
            include *
        }
        custom {
        }
        image * {
            image diagram.png
        }
        filtered "SystemLandscape-001" include "Person"
        dynamic shop {
            user -> web
        }"#,
    ));
    let keys: Vec<&str> = m.views.iter().map(|v| v.key.as_str()).collect();
    // A generated key skips one already written (DSL-7); numbering is per
    // kind in declaration order.
    assert_eq!(
        keys,
        [
            "SystemLandscape-001",
            "SystemLandscape-002",
            "SystemLandscape-003",
            "Dynamic-001",
            "Deployment-001",
            "Custom-001",
            "Custom-002",
            "Image-001",
            "Filtered-001",
            "Dynamic-002",
        ]
    );
}

/// A dynamic or deployment view (and the landscape-filtered kinds until that row lands) is in `Model::views` with its key and title; the rest of the file builds.
#[test]
fn p2_2_unsupported_view_kinds_in_model_views() {
    use model::ViewKind::{Container, Custom, Deployment, Dynamic, Filtered, Image, SystemLandscape};
    let m = model_of(&in_workspace(
        &format!("{SHOP}\n{LIVE}"),
        r#"        container shop "c2" {
            include *
        }
        systemLandscape "landscape" "Everything" {
            include *
        }
        filtered "landscape" exclude "Person" "people-free" "No people"
        custom "board" "Whiteboard" {
            include *
        }
        image web "picture" {
            image diagram.png
            title "A picture"
        }
        dynamic shop "flow" "The flow" {
            user -> web
        }
        deployment shop "Live" "live" {
            include *
            title "Live system"
        }"#,
    ));
    let got: Vec<(model::ViewKind, &str, Option<&str>, bool)> = m
        .views
        .iter()
        .map(|v| {
            (
                v.kind,
                v.key.as_str(),
                v.drawn_title().map(|t| t.value.as_str()),
                v.kind.drawable(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (Container, "c2", None, true),
            // landscape-filtered draws the landscape view (P2-8).
            (SystemLandscape, "landscape", Some("Everything"), true),
            (Filtered, "people-free", Some("No people"), true),
            (Custom, "board", Some("Whiteboard"), true),
            (Image, "picture", Some("A picture"), true),
            // dynamic-view draws dynamic views (P3-200).
            (Dynamic, "flow", Some("The flow"), true),
            // deployment-view draws deployment views (P3-2).
            (Deployment, "live", Some("Live system"), true),
        ]
    );
    // The drawable view is built as before.
    assert_eq!(m.included(&m.views[0]).len(), 3);
}

// ---- Identifiers (dsl-breadth-06)

/// Under `!identifiers hierarchical`, two `api` containers in s1 and s2 are found as s1.api and s2.api; the same full id twice is `DuplicateId`.
#[test]
fn p2_6_hierarchical_ids_scope_by_parent() {
    for place in ["workspace", "model"] {
        let directive = "!identifiers hierarchical";
        let body = "        s1 = softwareSystem \"One\" {\n            api = container \"Api\"\n        }\n        s2 = softwareSystem \"Two\" {\n            api = container \"Api\"\n        }";
        let src = if place == "workspace" {
            format!("workspace {{\n    {directive}\n    model {{\n{body}\n    }}\n}}\n")
        } else {
            in_model(&format!("        {directive}\n{body}"))
        };
        let m = model_of(&src);
        assert_eq!(m.identifiers, model::Identifiers::Hierarchical, "{place}");
        let (a1, a2) = (m.find_written("s1.api").unwrap(), m.find_written("s2.api").unwrap());
        assert_ne!(a1, a2);
        assert_eq!(m.element(a1).parent, m.find_written("s1"));
        assert_eq!(m.full_key(a1).as_ref().map(ElementKey::as_str), Some("s1.api"));
        assert_eq!(m.full_key(m.find_written("s1").unwrap()).as_ref().map(ElementKey::as_str), Some("s1"));
    }
    let e = error_of(&in_model(
        "        !identifiers hierarchical\n        s1 = softwareSystem \"One\" {\n            api = container \"Api\"\n            api = container \"Other\"\n        }",
    ));
    assert!(
        matches!(&e.kind, ModelErrorKind::DuplicateId { id, .. } if id == "s1.api"),
        "{e:?}"
    );
}

/// Full ids work in relationships, include, exclude, at, size, focus, hide and `!element`.
#[test]
fn p2_142_full_id_in_relationships_views_at_size_focus_hide() {
    // TODO(srs): P2-142 bare id inside the parent's block. Reading taken (B dsl-breadth 11): looked up as <parent full id>.<id> first, then as a full id.
    let src = in_workspace(
        "        !identifiers hierarchical\n        user = person \"User\"\n        s1 = softwareSystem \"One\" {\n            api = container \"Api\"\n            db = container \"Db\"\n            api -> db \"Reads\"\n        }\n        s2 = softwareSystem \"Two\" {\n            api = container \"Api\"\n        }\n        user -> s1.api \"Uses\"\n        s1.api -> s2.api \"Calls\"\n        !element s1.db {\n            tags \"Store\"\n        }",
        "        container s1 \"c2\" {\n            include s1.api s1.db s2.api\n            exclude user\n            focus s1.api\n            hide s1.api -> s2.api\n            at s1.api 100 100\n            size s1.db 200 120\n        }",
    );
    let m = model_of(&src);
    let (api, db, other) = (
        m.find_written("s1.api").unwrap(),
        m.find_written("s1.db").unwrap(),
        m.find_written("s2.api").unwrap(),
    );
    // A bare id inside the parent's block is looked up there first.
    assert_eq!(
        (m.relationships[0].source.value, m.relationships[0].destination.value),
        (api, db)
    );
    assert_eq!(m.relationships[1].destination.value, api);
    assert_eq!(
        (m.relationships[2].source.value, m.relationships[2].destination.value),
        (api, other)
    );
    assert!(m.element(db).has_tag("Store"));
    let v = &m.views[0];
    assert_eq!(m.included(v), vec![api, db, other]);
    assert_eq!(v.focus[0].value, api);
    assert_eq!(v.hide[0].destination.value, other);
    assert_eq!(v.position_of(api).unwrap().x.value, 100);
    assert_eq!(v.size_of(db).unwrap().width.value, 200);
    // Identifier lookup ignores case (Structurizr).
    assert_eq!(m.find_written("S1.API"), Some(api));
}

/// `r = a -> b` inside s1 is found as `r`, not `s1.r`.
#[test]
fn p2_143_relationship_ids_stay_flat() {
    let m = model_of(&in_model(
        "        !identifiers hierarchical\n        s1 = softwareSystem \"One\" {\n            a = container \"A\"\n            b = container \"B\"\n            r = a -> b \"Uses\"\n        }\n        !relationship r {\n            tag \"Found\"\n        }",
    ));
    let r = &m.relationships[0];
    assert_eq!(r.key.as_ref().unwrap().value, "r");
    assert!(r.tags.iter().any(|t| t == "Found"));
    let e = error_of(&in_model(
        "        !identifiers hierarchical\n        s1 = softwareSystem \"One\" {\n            a = container \"A\"\n            b = container \"B\"\n            r = a -> b \"Uses\"\n        }\n        !relationship s1.r {\n            tag \"Found\"\n        }",
    ));
    assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "s1.r".into() });
}

/// In flat mode a second element with a used id is `DuplicateId` naming both lines.
#[test]
fn p2_144_flat_duplicate_id_is_error() {
    for directive in ["", "        !identifiers flat\n"] {
        let src = in_model(&format!(
            "{directive}        s1 = softwareSystem \"One\" {{\n            api = container \"Api\"\n        }}\n        s2 = softwareSystem \"Two\" {{\n            API = container \"Api\"\n        }}"
        ));
        let e = error_of(&src);
        let ModelErrorKind::DuplicateId {
            id,
            first_position,
            ..
        } = &e.kind
        else {
            panic!("{e:?}");
        };
        assert_eq!(id, "API");
        let lines = u32::try_from(directive.lines().count()).unwrap();
        assert_eq!(first_position.line, 4 + lines);
        assert_eq!(e.position.line, 7 + lines);
    }
}

/// An element without `<id> =` parses, has key None and `full_key` None, and `include *` selects it as its kind allows.
#[test]
fn p2_145_anonymous_element_selected_by_star() {
    // TODO(srs): P2-145 how at, size and edits address it. Reading taken (B dsl-breadth 10): never; auto-placed, the editor refuses moves/resizes/per-box styles.
    let m = model_of(&in_workspace(
        "        person \"Visitor\"\n        shop = softwareSystem \"Shop\" {\n            container \"Web\"\n            db = container \"Db\"\n        }",
        "        container shop \"c2\" {\n            include *\n        }",
    ));
    let visitor = &m.elements[0];
    assert_eq!(visitor.key, None);
    assert_eq!(visitor.name.value, "Visitor");
    assert_eq!(m.full_key(visitor.id), None);
    let web = &m.elements[2];
    assert_eq!((web.key.as_ref(), web.parent), (None, m.find_written("shop")));
    assert_eq!(m.included(&m.views[0]), vec![web.id, m.find_written("db").unwrap()]);
}

/// `r = a -> b` sets `Relationship::key` to r and `!relationship r` resolves it.
#[test]
fn p2_146_named_relationship_key() {
    let src = in_model(
        "        a = person \"A\"\n        b = softwareSystem \"B\"\n        uses = a -> b \"Uses\"\n        !relationship uses {\n            tag \"Main\"\n        }",
    );
    let m = model_of(&src);
    let r = &m.relationships[0];
    let key = r.key.as_ref().unwrap();
    assert_eq!(key.value, "uses");
    assert_eq!(&src[key.span.range()], "uses");
    assert_eq!(r.tags, ["Relationship", "Main"]);
    assert_eq!((r.source.value, r.destination.value), (m.find_written("a").unwrap(), m.find_written("b").unwrap()));
}

/// A relationship key declared twice is `DuplicateId` naming both lines.
#[test]
fn p2_146_duplicate_relationship_key_is_error() {
    // TODO(srs): P2-146 relationship id equal to an element id. Reading taken (B dsl-breadth 12): one register, DuplicateId.
    for second in ["r = b -> a", "r = softwareSystem \"C\"", "R = b -> a"] {
        let src = in_model(&format!(
            "        a = person \"A\"\n        b = softwareSystem \"B\"\n        r = a -> b\n        {second}"
        ));
        let e = error_of(&src);
        let ModelErrorKind::DuplicateId { first_position, .. } = &e.kind else {
            panic!("{second}: {e:?}");
        };
        assert_eq!((first_position.line, e.position.line), (5, 6), "{second}");
    }
    // An element id first, then the same id on a relationship.
    let e = error_of(&in_model(
        "        a = person \"A\"\n        b = softwareSystem \"B\"\n        a = b -> a",
    ));
    assert!(matches!(e.kind, ModelErrorKind::DuplicateId { .. }), "{e:?}");
}

/// Same-name people/systems, containers in one system, components in one container: `DuplicateName` naming both lines.
#[test]
fn p2_177_duplicate_names_are_errors() {
    // TODO(srs): P2-177 is an open question (Java model rule). Reading taken (B dsl-breadth 13): DuplicateName.
    let cases = [
        "        a = person \"Same\"\n        b = person \"Same\"",
        "        a = softwareSystem \"Same\"\n        b = softwareSystem \"Same\"",
        "        s = softwareSystem \"S\" {\n            a = container \"Same\"\n            b = container \"Same\"\n        }",
        "        s = softwareSystem \"S\" {\n            c = container \"C\" {\n                a = component \"Same\"\n                b = component \"Same\"\n            }\n        }",
    ];
    for body in cases {
        let e = error_of(&in_model(body));
        let ModelErrorKind::DuplicateName {
            name,
            first_position,
            ..
        } = &e.kind
        else {
            panic!("{body}: {e:?}");
        };
        assert_eq!(name, "Same");
        assert_eq!(first_position.line + 1, e.position.line, "{body}");
        assert!(e.to_string().contains(&format!("{}:", first_position.line)), "{e}");
    }
    // Structurizr allows the same name on a person and a system, and on
    // containers of two systems.
    model_of(&in_model(
        "        a = person \"Same\"\n        b = softwareSystem \"Same\" {\n            c = container \"C\"\n        }\n        d = softwareSystem \"D\" {\n            e = container \"C\"\n        }",
    ));
}

// ---- Groups (dsl-breadth-07, -08)

/// `group` in model, a software system and a container: every element inside has `Element::group`.
#[test]
fn p2_4_group_members_carry_group() {
    let src = in_model(
        r#"        team = group "Team" {
            user = person "User"
            shop = softwareSystem "Shop" {
                group "Front" {
                    web = container "Web" {
                        group "Handlers" {
                            api = component "Api"
                        }
                        job = component "Job"
                    }
                }
                db = container "Db"
            }
            user -> shop "Uses"
        }
        other = person "Other""#,
    );
    let m = model_of(&src);
    let names: Vec<(&str, Option<&str>, Option<model::GroupId>)> = m
        .groups
        .iter()
        .map(|g| (g.name.value.as_str(), g.key.as_ref().map(|k| k.value.as_str()), g.parent))
        .collect();
    assert_eq!(
        names,
        [("Team", Some("team"), None), ("Front", None, None), ("Handlers", None, None)]
    );
    for (i, g) in m.groups.iter().enumerate() {
        assert_eq!(g.id, model::GroupId(u32::try_from(i).unwrap()));
        assert_eq!(g.full_name, g.name.value);
    }
    assert_eq!(m.groups[0].owner, None);
    assert_eq!(m.groups[1].owner, m.find_written("shop"));
    assert_eq!(m.groups[2].owner, m.find_written("web"));
    let group_of = |k: &str| el(&m, k).group;
    let g = |i: u32| Some(model::GroupId(i));
    assert_eq!(group_of("user"), g(0));
    assert_eq!(group_of("shop"), g(0));
    assert_eq!(group_of("web"), g(1));
    assert_eq!(group_of("api"), g(2));
    // Children of a member are not members themselves.
    assert_eq!(group_of("job"), None);
    assert_eq!(group_of("db"), None);
    assert_eq!(group_of("other"), None);
    assert_eq!(m.relationships.len(), 1);
    assert!(src[m.groups[0].span.range()].starts_with("team = group"));
}

/// An element of another level inside a group is `InvalidNesting`.
#[test]
fn p2_127_group_holds_one_level_only() {
    // TODO(srs): P2-127 custom element in a model-level group. Reading taken (B dsl-breadth 14a): allowed.
    let cases = [
        (
            "        s = softwareSystem \"S\" {\n            group \"G\" {\n                p = person \"P\"\n            }\n        }",
            ElementKind::Person,
            Some(ElementKind::SoftwareSystem),
        ),
        (
            "        group \"G\" {\n            c = container \"C\"\n        }",
            ElementKind::Container,
            None,
        ),
        (
            "        s = softwareSystem \"S\" {\n            c = container \"C\" {\n                group \"G\" {\n                    d = container \"D\"\n                }\n            }\n        }",
            ElementKind::Container,
            Some(ElementKind::Container),
        ),
    ];
    for (body, kind, parent) in cases {
        let e = error_of(&in_model(body));
        assert_eq!(e.kind, ModelErrorKind::InvalidNesting { kind, parent }, "{body}");
    }
    // A custom element may sit in a model-level group (B dsl-breadth 14a,
    // checked against Structurizr for Java).
    model_of(&in_model("        group \"G\" {\n            e = element \"E\"\n        }"));
    // A group holds no element-block lines.
    let e = error_of(&in_model(
        "        s = softwareSystem \"S\" {\n            group \"G\" {\n                description \"x\"\n            }\n        }",
    ));
    assert!(matches!(e.kind, ModelErrorKind::UnknownConstruct { .. }), "{e:?}");
}

/// `group "<name>"` inside a component block sets that component's group.
#[test]
fn p2_128_group_line_in_component_block() {
    let m = model_of(&in_model(
        "        s = softwareSystem \"S\" {\n            c = container \"C\" {\n                a = component \"A\" {\n                    group \"Line\"\n                }\n                b = component \"B\" {\n                    group \"Line\"\n                }\n                d = component \"D\"\n            }\n        }",
    ));
    let (a, b) = (el(&m, "a").group, el(&m, "b").group);
    assert!(a.is_some());
    assert_eq!(a, b);
    assert_eq!(el(&m, "d").group, None);
    let g = &m.groups[a.unwrap().0 as usize];
    assert_eq!((g.name.value.as_str(), g.owner), ("Line", m.find_written("c")));
}

/// With structurizr.groupSeparator `/`, nested groups A and B give full name A/B.
#[test]
fn p2_129_nested_groups_with_separator() {
    let m = model_of(&in_model(
        "        properties {\n            structurizr.groupSeparator /\n        }\n        group \"A\" {\n            group \"B\" {\n                p = person \"P\"\n            }\n            q = person \"Q\"\n        }",
    ));
    let full: Vec<&str> = m.groups.iter().map(|g| g.full_name.as_str()).collect();
    assert_eq!(full, ["A", "A/B"]);
    assert_eq!(m.groups[1].parent, Some(model::GroupId(0)));
    assert_eq!(el(&m, "p").group, Some(model::GroupId(1)));
    assert_eq!(el(&m, "q").group, Some(model::GroupId(0)));
}

/// A nested group with no separator set is `NestedGroupNeedsSeparator`.
#[test]
fn p2_129_nested_group_without_separator_is_error() {
    // TODO(srs): P2-129 nested group with no separator. Reading taken (B dsl-breadth 14b): this error.
    let src = in_model(
        "        group \"A\" {\n            group \"B\" {\n                p = person \"P\"\n            }\n        }",
    );
    let e = error_of(&src);
    assert_eq!(e.kind, ModelErrorKind::NestedGroupNeedsSeparator);
    assert_eq!(e.position.line, 4);
    assert!(e.to_string().contains("structurizr.groupSeparator"), "{e}");
}

/// A container api inside a group in s1 is s1.api under hierarchical ids.
#[test]
fn p2_131_group_not_in_hierarchical_id() {
    let m = model_of(&in_model(
        "        !identifiers hierarchical\n        s1 = softwareSystem \"One\" {\n            group \"G\" {\n                api = container \"Api\"\n            }\n        }",
    ));
    let api = m.find_written("s1.api").unwrap();
    assert_eq!(m.full_key(api).as_ref().map(ElementKey::as_str), Some("s1.api"));
    assert!(m.element(api).group.is_some());
}

/// The build of view `key` of `source`, from no kept position.
fn build(m: &Model, key: &str) -> c4rizer_core::pipeline::ViewBuild {
    let never = std::sync::atomic::AtomicBool::new(false);
    c4rizer_core::pipeline::build_view(m, key, &c4rizer_core::place::Keep::default(), &never)
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Index of the group boundary named `name` in `scene.boundaries`.
fn group_boundary(m: &Model, scene: &c4rizer_core::scene::Scene, name: &str) -> usize {
    scene
        .boundaries
        .iter()
        .position(|b| {
            matches!(b.of, c4rizer_core::view::BoundaryOf::Group(g)
                if m.groups[g.0 as usize].name.value == name)
        })
        .unwrap_or_else(|| panic!("no boundary for group {name}"))
}

/// True when `inner` lies inside `outer` with room on every side.
fn strictly_inside(inner: &c4rizer_core::geom::Rect, outer: &c4rizer_core::geom::Rect) -> bool {
    outer.min.x < inner.min.x
        && outer.min.y < inner.min.y
        && inner.max.x < outer.max.x
        && inner.max.y < outer.max.y
}

/// The px bounds of `members` in `scene`, inflated by 40px (CORE-1).
fn members_bounds(
    scene: &c4rizer_core::scene::Scene,
    members: &[c4rizer_core::view::BoxId],
) -> c4rizer_core::geom::Rect {
    use c4rizer_core::geom::{Point, Rect};
    members
        .iter()
        .map(|id| scene.boxes[id.0 as usize].rect)
        .reduce(|a, b| {
            Rect::from_corners(
                Point::new(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
                Point::new(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
            )
        })
        .expect("members")
        .inflate(40)
}

/// A group with a drawn member gives one `BoundaryOf::Group` boundary around its drawn members (CORE-1 bounds + 40px), its name as tag, no bracketed type.
#[test]
fn p2_130_group_boundary_around_drawn_members() {
    // TODO(srs): P2-130 nested name tag, default colour, structurizr.groups. Readings taken: last segment (Structurizr UI); phase-1 boundary look (B 14c); `structurizr.groups false` hides it (B 14d).
    let src = in_workspace(
        "        user = person \"User\"
        shop = softwareSystem \"Shop\" {
            group \"Front\" {
                web = container \"Web\"
                app = container \"App\"
            }
            db = container \"Db\"
        }
        user -> web \"Uses\"
        web -> app \"Calls\"
        app -> db \"Writes\"",
        "        container shop \"c2\" {
            include *
            title \"Shop\"
        }
        container shop \"hidden\" {
            include *
            title \"Shop\"
            properties {
                structurizr.groups false
            }
        }",
    );
    let m = model_of(&src);
    let b = build(&m, "c2");
    let scene = &b.scene;
    assert_eq!(scene.boundaries.len(), 2, "{:?}", b.plan.boundaries);
    let g = group_boundary(&m, scene, "Front");
    let boundary = &scene.boundaries[g];
    let web = m.find_written("web").unwrap();
    let app = m.find_written("app").unwrap();
    let mut want: Vec<_> = [web, app].iter().flat_map(|&e| b.plan.boxes_of(e)).collect();
    want.sort();
    assert_eq!(boundary.members, want);
    assert_eq!(boundary.rect, members_bounds(scene, &boundary.members));
    assert!(boundary.stroke.dash_px.is_some(), "dashed");
    assert_eq!(boundary.tag.lines, vec!["Front".to_owned()]);
    assert!(boundary.tag.fits);
    assert_eq!(boundary.tag_rect.min.x, boundary.rect.min.x);
    assert_eq!(boundary.tag_rect.max.y, boundary.rect.max.y);
    // The scope boundary keeps its phase-1 tag and comes first.
    assert_eq!(scene.boundaries[0].tag.lines, vec!["Shop [Software System]".to_owned()]);
    assert!(g > 0);
    // `structurizr.groups false` on the view hides the group boundary (B 14d).
    let hidden = build(&m, "hidden");
    assert_eq!(hidden.scene.boundaries.len(), 1);
    assert!(b.report.findings.iter().all(|f| f.rule != c4rizer_core::lint::Rule::BoundaryIntrusion));
}

/// A nested group's boundary sits inside its enclosing group's boundary; outer before inner in `ViewPlan::boundaries`.
#[test]
fn p2_130_nested_group_boundary_inside_outer() {
    let src = in_workspace(
        "        properties {
            \"structurizr.groupSeparator\" \"/\"
        }
        shop = softwareSystem \"Shop\" {
            group \"Outer\" {
                group \"Inner\" {
                    a = container \"A\"
                    b = container \"B\"
                }
                c = container \"C\"
            }
            d = container \"D\"
        }
        a -> b \"Calls\"
        b -> c \"Calls\"
        c -> d \"Calls\"",
        "        container shop \"c2\" {
            include *
            title \"Shop\"
        }",
    );
    let m = model_of(&src);
    let b = build(&m, "c2");
    let scene = &b.scene;
    assert_eq!(scene.boundaries.len(), 3);
    let outer = group_boundary(&m, scene, "Outer");
    let inner = group_boundary(&m, scene, "Inner");
    assert!(0 < outer && outer < inner, "outer before inner");
    // The outer group's members include the inner group's.
    for id in &scene.boundaries[inner].members {
        assert!(scene.boundaries[outer].members.contains(id));
    }
    assert!(strictly_inside(&scene.boundaries[inner].rect, &scene.boundaries[outer].rect));
    assert!(strictly_inside(&scene.boundaries[outer].rect, &scene.boundaries[0].rect));
    // The nested group's tag is its own name (last segment).
    assert_eq!(scene.boundaries[inner].tag.lines, vec!["Inner".to_owned()]);
    // Tags do not overlap each other.
    assert!(!scene.boundaries[inner].tag_rect.overlaps(&scene.boundaries[outer].tag_rect));
    assert!(!scene.boundaries[outer].tag_rect.overlaps(&scene.boundaries[0].tag_rect));
}

/// A group of out-of-scope elements does not overlap the scope boundary; one of scope children sits inside it.
#[test]
fn p2_130_out_of_scope_group_clear_of_scope_boundary() {
    let src = in_workspace(
        "        group \"Customers\" {
            buyer = person \"Buyer\"
            seller = person \"Seller\"
        }
        group \"Partners\" {
            bank = softwareSystem \"Bank\"
        }
        shop = softwareSystem \"Shop\" {
            group \"Front\" {
                web = container \"Web\"
            }
            api = container \"Api\"
        }
        buyer -> web \"Uses\"
        seller -> web \"Uses\"
        web -> api \"Calls\"
        api -> bank \"Charges\"",
        "        container shop \"c2\" {
            include *
            title \"Shop\"
        }",
    );
    let m = model_of(&src);
    let b = build(&m, "c2");
    let scene = &b.scene;
    let scope = &scene.boundaries[0];
    assert!(matches!(scope.of, c4rizer_core::view::BoundaryOf::Element(_)));
    let customers = &scene.boundaries[group_boundary(&m, scene, "Customers")];
    let partners = &scene.boundaries[group_boundary(&m, scene, "Partners")];
    let front = &scene.boundaries[group_boundary(&m, scene, "Front")];
    assert!(!customers.rect.overlaps(&scope.rect), "{customers:?} {scope:?}");
    assert!(!partners.rect.overlaps(&scope.rect), "{partners:?} {scope:?}");
    assert!(!customers.rect.overlaps(&partners.rect));
    assert!(strictly_inside(&front.rect, &scope.rect));
    assert!(b.report.findings.iter().all(|f| f.rule != c4rizer_core::lint::Rule::BoundaryIntrusion), "{:?}", b.report.findings);
    // With one box kept (the editor's case), the others auto-placed keep
    // the groups apart too.
    let mut keep = c4rizer_core::place::Keep::default();
    let api = m.find_written("api").unwrap();
    keep.previous.insert(api, c4rizer_core::geom::Point::new(60, 60));
    let never = std::sync::atomic::AtomicBool::new(false);
    let kept = c4rizer_core::pipeline::build_view(&m, "c2", &keep, &never).unwrap();
    let scene = &kept.scene;
    let scope = &scene.boundaries[0];
    for name in ["Customers", "Partners"] {
        let g = &scene.boundaries[group_boundary(&m, scene, name)];
        assert!(!g.rect.overlaps(&scope.rect), "{name}: {g:?} {scope:?}");
    }
    assert!(kept.report.findings.iter().all(|f| f.rule != c4rizer_core::lint::Rule::BoundaryIntrusion), "{:?}", kept.report.findings);
}

/// A group with no drawn member emits no boundary.
#[test]
fn p2_130_group_without_drawn_member_has_no_boundary() {
    let src = in_workspace(
        "        user = person \"User\"
        group \"Elsewhere\" {
            other = softwareSystem \"Other\"
        }
        shop = softwareSystem \"Shop\" {
            group \"Gone\" {
                old = container \"Old\"
            }
            web = container \"Web\"
        }
        user -> web \"Uses\"",
        "        container shop \"c2\" {
            include *
            exclude old
            title \"Shop\"
        }
        systemContext shop \"c1\" {
            include shop user
            title \"Shop\"
        }",
    );
    let m = model_of(&src);
    let c2 = build(&m, "c2");
    assert_eq!(c2.plan.boundaries.len(), 1, "{:?}", c2.plan.boundaries);
    assert!(matches!(c2.plan.boundaries[0].of, c4rizer_core::view::BoundaryOf::Element(_)));
    let c1 = build(&m, "c1");
    assert!(c1.plan.boundaries.is_empty(), "{:?}", c1.plan.boundaries);
}

/// tests/data/dsl-breadth/groups.dsl with no `at` lines: no non-member box overlaps a group rectangle.
#[test]
fn p2_132_auto_place_keeps_non_members_out_of_groups() {
    // TODO(srs): P2-132 whether L26 covers group rectangles. Reading taken (B dsl-breadth 14e): yes.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data/dsl-breadth/groups.dsl");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(" at "), "fixture has no at lines");
    let m = model_of(&text);
    let never = std::sync::atomic::AtomicBool::new(false);
    let all = c4rizer_core::pipeline::build_all(&m, &never).unwrap();
    assert_eq!(all.views.len(), 3);
    let groups = assert_groups_clear(&m, &all);
    // Context: Customers, Partners; containers: Customers, Partners,
    // Front, Apps, Storage; components: Orders.
    assert!(groups >= 8, "{groups} group boundaries");
}

/// Checks P2-130/P2-132 on every view of `all`: no box overlaps a
/// boundary it is not a member of, two boundaries either nest or keep
/// apart, a group boundary whose members are not all children of the
/// scope does not overlap the scope boundary, and lint raises no L26.
/// Returns the number of group boundaries seen.
fn assert_groups_clear(m: &Model, all: &c4rizer_core::pipeline::AllViews) -> usize {
    use c4rizer_core::view::BoundaryOf;
    let mut groups = 0;
    for v in &all.views {
        let scene = &v.scene;
        for bd in &scene.boundaries {
            if let BoundaryOf::Group(_) = bd.of {
                groups += 1;
                if let Some(scope) = scene.boundaries.iter().find(|s| matches!(s.of, BoundaryOf::Element(_))) {
                    let inside = bd.members.iter().all(|x| scope.members.contains(x));
                    if !inside {
                        assert!(!bd.rect.overlaps(&scope.rect), "{}: {:?} overlaps scope {:?}", scene.key, bd.tag.lines, scope.tag.lines);
                    }
                }
            }
            for bx in &scene.boxes {
                if !bd.members.contains(&bx.id) {
                    assert!(!bx.rect.overlaps(&bd.rect), "{}: {:?} overlaps {:?}", scene.key, bx.name, bd.tag.lines);
                }
            }
        }
        // Two boundaries either nest or keep apart.
        for (i, a) in scene.boundaries.iter().enumerate() {
            for b in &scene.boundaries[i + 1..] {
                let nested = b.members.iter().all(|x| a.members.contains(x));
                if nested {
                    assert!(strictly_inside(&b.rect, &a.rect), "{}: {:?} in {:?}", scene.key, b.tag.lines, a.tag.lines);
                } else {
                    assert!(!a.rect.overlaps(&b.rect), "{}: {:?} and {:?}", scene.key, a.tag.lines, b.tag.lines);
                }
            }
        }
        assert!(
            v.report.findings.iter().all(|f| f.rule != c4rizer_core::lint::Rule::BoundaryIntrusion),
            "{}: {:?}",
            scene.key,
            v.report.findings
        );
    }
    let _ = m;
    groups
}

/// tests/data/dsl-breadth/grammar.dsl, whose `containers` view draws a
/// nested out-of-scope group (`${ORG} partners` > `Couriers`): every
/// group boundary keeps clear as P2-130 and P2-132 say.
#[test]
fn p2_130_grammar_fixture_groups_clear_of_scope() {
    let (_, parsed) = grammar();
    let never = std::sync::atomic::AtomicBool::new(false);
    let all = c4rizer_core::pipeline::build_all(&parsed.model, &never).unwrap();
    let containers = all.view("containers").expect("containers view");
    assert!(
        containers.scene.boundaries.iter().any(|b| b.tag.lines.join(" ") == "Couriers"),
        "{:?}",
        containers.scene.boundaries.iter().map(|b| &b.tag.lines).collect::<Vec<_>>()
    );
    assert!(assert_groups_clear(&parsed.model, &all) >= 3);
}

// ---- Custom elements and archetypes (dsl-breadth-09)

/// `element <name> [metadata] [description] [tags]` gives `ElementKind::Custom` with only tag Element and metadata kept; its children are accepted.
#[test]
fn p2_170_custom_element_parsed() {
    // TODO(srs): P2-170 include * selecting custom elements. Reading taken (Structurizr for Java, checked; differs from B dsl-breadth 16, Part C: Java wins): include * selects a custom element linked to the scope; explicit include adds one.
    let src = in_workspace(
        r#"        board = element "Whiteboard" "Hardware" "Where plans are drawn." "Office" {
            description "A real whiteboard."
            tag "Wall"
            url https://example.com/board
            properties {
                size large
            }
            perspectives {
                Cost "Cheap"
            }
            -> shop "Shows"
        }
        group "Room" {
            pen = element "Pen"
        }
        shop = softwareSystem "Shop"
        other = softwareSystem "Other""#,
        "        systemContext shop \"c1\" {\n            include *\n        }\n        systemContext other \"c1-other\" {\n            include * pen\n        }",
    );
    let m = model_of(&src);
    let board = el(&m, "board");
    assert_eq!(board.kind, ElementKind::Custom);
    assert_eq!(board.metadata.as_ref().unwrap().value, "Hardware");
    assert_eq!(board.description.as_ref().unwrap().value, "A real whiteboard.");
    assert_eq!(board.tags, ["Element", "Office", "Wall"]);
    assert_eq!(board.url.as_ref().unwrap().value, "https://example.com/board");
    assert_eq!((board.properties.len(), board.perspectives.len()), (1, 1));
    assert_eq!(m.relationships[0].source.value, board.id);
    let pen = el(&m, "pen");
    assert_eq!((pen.kind, pen.group.is_some(), pen.metadata.as_ref()), (ElementKind::Custom, true, None));
    assert_eq!(pen.tags, ["Element"]);
    // Structurizr for Java (checked): `include *` on a system context view
    // selects a custom element linked to the scope; an explicit id adds one.
    assert!(m.included(&m.views[0]).contains(&board.id));
    assert!(m.included(&m.views[1]).contains(&pen.id));
    // A custom element only sits in `model`.
    let e = error_of(&in_model(
        "        s = softwareSystem \"S\" {\n            e = element \"E\"\n        }",
    ));
    assert!(matches!(e.kind, ModelErrorKind::InvalidNesting { kind: ElementKind::Custom, .. }), "{e:?}");
}

/// `service = container { technology "Go"; tags "Service" }`: `x = service "Name"` is a container with technology Go and tag Service; positional/body values override.
#[test]
fn p2_155_element_archetype_defaults() {
    let m = model_of(&in_model(&format!(
        "{ARCHETYPES}\n        s = softwareSystem \"S\" {{\n            x = service \"X\"\n            y = service \"Y\" \"Own description.\" \"Rust\"\n            z = service \"Z\" {{\n                technology \"Java\"\n                description \"Body description.\"\n            }}\n        }}"
    )));
    let x = el(&m, "x");
    assert_eq!(x.kind, ElementKind::Container);
    assert_eq!(x.technology.as_ref().unwrap().value, "Go");
    assert_eq!(x.description.as_ref().unwrap().value, "A service.");
    assert_eq!(x.tags, ["Element", "Container", "Service"]);
    assert_eq!(x.properties[0].name.value, "tier");
    let y = el(&m, "y");
    assert_eq!(y.technology.as_ref().unwrap().value, "Rust");
    assert_eq!(y.description.as_ref().unwrap().value, "Own description.");
    let z = el(&m, "z");
    assert_eq!(z.technology.as_ref().unwrap().value, "Java");
    assert_eq!(z.description.as_ref().unwrap().value, "Body description.");
}

/// `api = service { ... }` takes the base defaults and overrides its own.
#[test]
fn p2_156_archetype_inheritance() {
    // TODO(srs): P2-156 tag combination. Reading taken (B dsl-breadth 15a): base tags then its own.
    let m = model_of(&in_model(&format!(
        "{ARCHETYPES}\n        s = softwareSystem \"S\" {{\n            a = api \"A\" \"\" \"\" \"Own\"\n        }}"
    )));
    let a = el(&m, "a");
    assert_eq!(a.kind, ElementKind::Container);
    // Overridden by `api`, inherited from `service`.
    assert_eq!(a.technology.as_ref().unwrap().value, "");
    let m = model_of(&in_model(&format!(
        "{ARCHETYPES}\n        s = softwareSystem \"S\" {{\n            a = api \"A\"\n        }}"
    )));
    let a = el(&m, "a");
    assert_eq!(a.technology.as_ref().unwrap().value, "Go REST");
    assert_eq!(a.description.as_ref().unwrap().value, "A service.");
    assert_eq!(a.properties.len(), 1);
    assert_eq!(a.tags, ["Element", "Container", "Service", "Api"]);
}

/// `sync = -> { tags "Synchronous" }`, `https = --sync-> { technology "HTTPS" }`: `a --https-> b "d"` has tag Synchronous, technology HTTPS, description d.
#[test]
fn p2_157_relationship_archetype_arrow() {
    let m = model_of(&in_model(&format!(
        "{ARCHETYPES}\n        a = staff \"A\"\n        b = softwareSystem \"B\"\n        a --https-> b \"d\"\n        b --sync-> a \"back\" \"Queue\""
    )));
    let r = &m.relationships[0];
    assert_eq!(r.tags, ["Relationship", "Synchronous"]);
    assert_eq!(r.technology.as_ref().unwrap().value, "HTTPS");
    assert_eq!(r.description.as_ref().unwrap().value, "d");
    assert_eq!((r.source.value, r.destination.value), (m.find_written("a").unwrap(), m.find_written("b").unwrap()));
    let back = &m.relationships[1];
    assert_eq!(back.technology.as_ref().unwrap().value, "Queue");
    assert_eq!(el(&m, "a").description.as_ref().unwrap().value, "Works here.");
}

/// `softwareSystem { ... }` and `-> { ... }` with no name in `archetypes`
/// set the defaults of every later element of that kind and every later
/// relationship; an archetype based on the keyword after it starts from
/// that default (corpus archetypes-for-defaults.dsl and
/// archetypes-for-extension.dsl; Structurizr 6.2.4 probed).
// TODO(srs): P2-155..158 name no unnamed default archetype. Reading
// taken: as Structurizr 6.2.4, including that an archetype declared
// before the default does not take it.
#[test]
fn p2_155_unnamed_default_archetypes() {
    let m = model_of(&in_model(
        "        archetypes {\n            early = softwareSystem {\n                tag \"Early\"\n            }\n            softwaresystem {\n                description \"D\"\n                tag \"Default\"\n                properties {\n                    \"k\" \"v\"\n                }\n            }\n            late = softwareSystem {\n                tag \"Late\"\n            }\n            -> {\n                technology \"T\"\n                tag \"Default\"\n            }\n            sync = -> {\n                tag \"Sync\"\n            }\n        }\n        a = softwareSystem \"A\"\n        b = early \"B\"\n        c = late \"C\" \"Own.\"\n        p = person \"P\"\n        a -> b\n        a -> c \"x\" \"own\"\n        p --sync-> a",
    ));
    let a = el(&m, "a");
    assert_eq!(a.description.as_ref().unwrap().value, "D");
    assert_eq!(a.tags, ["Element", "Software System", "Default"]);
    assert_eq!(a.properties.len(), 1);
    assert_eq!(el(&m, "b").tags, ["Element", "Software System", "Early"]);
    assert!(el(&m, "b").description.is_none());
    let c = el(&m, "c");
    assert_eq!(c.tags, ["Element", "Software System", "Default", "Late"]);
    assert_eq!(c.description.as_ref().unwrap().value, "Own.");
    assert_eq!(el(&m, "p").tags, ["Element", "Person"]);
    let r = &m.relationships;
    assert_eq!(r[0].technology.as_ref().unwrap().value, "T");
    assert_eq!(r[0].tags, ["Relationship", "Default"]);
    assert_eq!(r[1].technology.as_ref().unwrap().value, "own");
    assert_eq!(r[2].tags, ["Relationship", "Default", "Sync"]);
}

/// A base outside the P2-158 list is `InvalidArchetype` naming it.
#[test]
fn p2_158_archetype_base_types() {
    // TODO(srs): P2-158 tag order of an element from an archetype. Reading taken (B dsl-breadth 15b): Element, kind tag, archetype chain root down, own tags.
    for base in [
        "person",
        "softwareSystem",
        "container",
        "component",
        "deploymentNode",
        "infrastructureNode",
        "group",
        "element",
    ] {
        model_of(&in_model(&format!(
            "        archetypes {{\n            a = {base} {{\n            }}\n            b = a {{\n            }}\n        }}"
        )));
    }
    let e = error_of(&in_model(
        "        archetypes {\n            bad = view {\n            }\n        }",
    ));
    assert_eq!(e.kind, ModelErrorKind::InvalidArchetype { base: "view".into() });
    assert!(e.to_string().contains("`view`"), "{e}");
    // Element, kind tag, the archetype chain root down, own tags (DSL-15).
    let m = model_of(&in_model(&format!(
        "{ARCHETYPES}\n        s = softwareSystem \"S\" {{\n            a = api \"A\" \"\" \"\" \"Own\" {{\n                tags \"Later\"\n            }}\n        }}"
    )));
    assert_eq!(el(&m, "a").tags, ["Element", "Container", "Service", "Api", "Own", "Later"]);
}

/// `--name->` naming no relationship archetype is `UnknownArchetype` naming it.
#[test]
fn p2_158_unknown_relationship_archetype_is_error() {
    let e = error_of(&in_model(
        "        a = person \"A\"\n        b = person \"B\"\n        a --nope-> b",
    ));
    assert_eq!(e.kind, ModelErrorKind::UnknownArchetype { name: "nope".into() });
    assert_eq!(e.position.line, 5);
}

/// Technology defaults apply only to containers and components, metadata only to custom elements.
#[test]
fn p2_181_technology_and_metadata_defaults_by_kind() {
    // TODO(srs): P2-181 technology on another base. Reading taken (B dsl-breadth 15c): UnknownConstruct naming `technology`.
    let m = model_of(&in_model(&format!(
        "{ARCHETYPES}\n        k = hardware \"Kiosk\"\n        s = softwareSystem \"S\""
    )));
    let k = el(&m, "k");
    assert_eq!(k.kind, ElementKind::Custom);
    assert_eq!(k.metadata.as_ref().unwrap().value, "Hardware");
    assert_eq!(k.technology, None);
    assert_eq!(k.tags, ["Element", "Kit"]);
    // `technology` in an archetype not based on a container or component,
    // and `metadata` in one not based on `element`, are unknown there.
    for (base, line) in [("person", "technology \"Go\""), ("container", "metadata \"M\"")] {
        let e = error_of(&in_model(&format!(
            "        archetypes {{\n            a = {base} {{\n                {line}\n            }}\n        }}"
        )));
        assert!(matches!(e.kind, ModelErrorKind::UnknownConstruct { .. }), "{base}: {e:?}");
    }
}

// ---- Directives that modify the model (dsl-breadth-10, -11)

/// `!element <id> { ... }` adds tags, properties, children and relationships to that element.
#[test]
fn p2_17_element_directive_adds_tags_children_relationships() {
    let src = in_model(
        r#"        shop = softwareSystem "Shop" "" "First"
        mail = softwareSystem "Mail"
        !element shop {
            tags "Second"
            tag "Third"
            description "Sells."
            properties {
                owner sales
            }
            web = container "Web" {
                -> mail "Sends"
            }
            -> mail "Notifies"
        }"#,
    );
    let m = model_of(&src);
    let shop = el(&m, "shop");
    assert_eq!(shop.tags, ["Element", "Software System", "First", "Second", "Third"]);
    assert_eq!(shop.description.as_ref().unwrap().value, "Sells.");
    assert_eq!(shop.properties[0].value.value, "sales");
    let web = el(&m, "web");
    assert_eq!(web.parent, Some(shop.id));
    let ends: Vec<_> = m
        .relationships
        .iter()
        .map(|r| (r.source.value, r.destination.value))
        .collect();
    let mail = m.find_written("mail").unwrap();
    assert_eq!(ends, [(web.id, mail), (shop.id, mail)]);
    let e = error_of(&in_model("        !element ghost {\n        }"));
    assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "ghost".into() });
}

/// `!element` names a deployment environment, node or infrastructure node
/// by id or by canonical name (`DeploymentNode://Env/Node/...`,
/// `InfrastructureNode://...`), and its block is read as that owner's
/// deployment block (corpus deployment-environment-empty.dsl and
/// find-element.dsl; Structurizr 6.2.4 opens both).
// TODO(srs): P2-17 and P2-120 do not say `!element` reaches deployment
// elements. Reading taken (Structurizr 6.2.4): it does, with the same
// statements the owner's own block allows.
#[test]
fn p2_17_element_directive_on_deployment_elements() {
    let m = model_of(&in_model(
        "        db = softwareSystem \"DB\"\n        de = deploymentEnvironment \"Live\"\n        !element de {\n            aws = deploymentNode \"AWS\" {\n                region = deploymentNode \"US-East-1\" {\n                    dns = infrastructureNode \"Route 53\"\n                }\n            }\n        }\n        !element \"DeploymentNode://Live/AWS/US-East-1\" {\n            deploymentNode \"Extra\" {\n                infrastructureNode \"Extra infra\" {\n                    -> dns\n                }\n            }\n        }\n        !element region {\n            softwareSystemInstance db\n        }\n        !element \"InfrastructureNode://Live/AWS/US-East-1/Route 53\" {\n            technology \"DNS\"\n        }",
    ));
    assert!(m.find_written("db").is_some());
    // A node's block refuses what a node refuses; an environment's block
    // refuses an infrastructure node.
    let e = error_of(&in_model(
        "        de = deploymentEnvironment \"Live\"\n        !element de {\n            infrastructureNode \"X\"\n        }",
    ));
    assert_eq!(e.kind, ModelErrorKind::UnknownConstruct { construct: "infrastructureNode".into() });
    let e = error_of(&in_model(
        "        de = deploymentEnvironment \"Live\"\n        !element \"DeploymentNode://Live/Nowhere\" {\n        }",
    ));
    assert!(matches!(e.kind, ModelErrorKind::UndeclaredId { .. }), "{e:?}");
}

/// `!relationship <id> { ... }` adds tag, tags, url, properties, perspectives.
#[test]
fn p2_167_relationship_directive_adds_tag_url_properties() {
    let m = model_of(&in_model(
        r#"        a = person "A"
        b = softwareSystem "B"
        uses = a -> b "Uses" "" "First"
        !relationship uses {
            tag "Second"
            tags "Third,Fourth"
            url https://example.com/uses
            properties {
                latency low
            }
            perspectives {
                Security "TLS"
            }
        }"#,
    ));
    let r = &m.relationships[0];
    assert_eq!(r.tags, ["Relationship", "First", "Second", "Third", "Fourth"]);
    assert_eq!(r.url.as_ref().unwrap().value, "https://example.com/uses");
    assert_eq!(r.properties[0].name.value, "latency");
    assert_eq!(r.perspectives[0].name.value, "Security");
    let e = error_of(&in_model("        a = person \"A\"\n        !relationship a {\n        }"));
    assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "a".into() });
}

/// `!extend` and `!ref` behave as `!element`, or `!relationship` for a relationship id.
#[test]
fn p2_169_extend_and_ref_are_aliases() {
    // TODO(srs): P2-169 `<id> = !element <canonical name>`. Reading taken (B dsl-breadth 23): binds to the item with that canonical name; unknown is UndeclaredId.
    let m = model_of(&in_model(
        r#"        shop = softwareSystem "Shop"
        person "Visitor"
        uses = shop -> shop "Self"
        !extend shop {
            tag "Extended"
        }
        !ref shop {
            tag "Referenced"
        }
        !extend uses {
            tag "ExtendedLine"
        }
        !ref uses {
            tag "ReferencedLine"
        }
        !element "SoftwareSystem://Shop" {
            tag "ByName"
        }
        visitor = !element "Person://Visitor" {
            tag "Bound"
        }
        visitor -> shop "Visits""#,
    ));
    assert_eq!(
        el(&m, "shop").tags,
        ["Element", "Software System", "Extended", "Referenced", "ByName"]
    );
    assert_eq!(
        m.relationships[0].tags,
        ["Relationship", "ExtendedLine", "ReferencedLine"]
    );
    let visitor = el(&m, "visitor");
    assert_eq!(visitor.name.value, "Visitor");
    assert!(visitor.has_tag("Bound"));
    assert_eq!(m.relationships[1].source.value, visitor.id);
    let e = error_of(&in_model("        !element \"Person://Nobody\" {\n        }"));
    assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "Person://Nobody".into() });
}

/// `!elements <expr> { ... }` applies its children to every element `Model::select_elements` returns; one case per element-expression form, plus && and ||.
#[test]
fn p2_166_elements_directive_per_expression_form() {
    // TODO(srs): mixed && and ||. Reading taken (owner ruling Part C, overriding B dsl-breadth 25): Structurizr for Java's split, checked below.
    // (expression, keys selected) as Structurizr for Java selects them.
    let cases: &[(&str, &[&str])] = &[
        ("element.type==Person", &["u", "v", "w"]),
        ("element.type==softwareSystem", &["s", "t"]),
        ("element.type==Custom", &["k"]),
        ("element.type==DeploymentNode", &[]),
        ("element.tag==a,b", &["v"]),
        ("element.tag!=a,b", &["u", "w", "s", "c1", "c2", "t", "k"]),
        ("element.technology==Java", &["c1"]),
        ("element.technology!=Java", &["c2"]),
        ("element.parent==s", &["c1", "c2"]),
        ("element.properties[tier]==back", &["c2"]),
        ("element.group==G", &["w"]),
        ("team", &["w"]),
        ("c1", &["c1"]),
        ("->s", &["u", "s"]),
        ("c1->", &["c1", "c2"]),
        ("->c2->", &["c1", "c2", "t"]),
        ("element==->t", &["s", "c2", "t"]),
        ("element==c1->", &["c1", "c2"]),
        ("element==->c2->", &["c1", "c2", "t"]),
        ("element.type==Person && element.tag==a", &["u", "v"]),
        ("element.tag==a || element.tag==b", &["u", "v", "w"]),
        ("element.type==Person && element.tag==a && element.tag!=b", &["u"]),
        // Mixed operators split on `&&` first and read each side whole,
        // as Structurizr for Java does: `a || b` is one tag value here.
        ("element.type==Person && element.tag==a || b", &[]),
    ];
    for (expression, keys) in cases {
        let src = in_model(&format!(
            "{EXPRESSIONS}\n        !elements \"{expression}\" {{\n            tag \"Hit\"\n        }}"
        ));
        if keys.is_empty() {
            // Structurizr refuses an expression that selects nothing.
            let e = error_of(&src);
            assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{expression}: {e:?}");
            continue;
        }
        let m = model_of(&src);
        assert_eq!(tagged(&m, "Hit"), *keys, "{expression}");
    }
    // Every child form, applied to each selected element.
    let m = model_of(&in_model(&format!(
        "{EXPRESSIONS}\n        !elements \"element.parent==s\" {{\n            tags \"One,Two\"\n            url https://example.com\n            properties {{\n                zone a\n            }}\n            perspectives {{\n                Ops \"Run\"\n            }}\n            -> t \"Reports\"\n        }}"
    )));
    for key in ["c1", "c2"] {
        let e = el(&m, key);
        assert!(e.has_tag("One") && e.has_tag("Two"), "{key}");
        assert_eq!(e.url.as_ref().unwrap().value, "https://example.com");
        assert!(e.properties.iter().any(|p| p.name.value == "zone"));
        assert_eq!(e.perspectives.len(), 1);
    }
    let reports: Vec<_> = m
        .relationships
        .iter()
        .filter(|r| r.description.as_ref().is_some_and(|d| d.value == "Reports"))
        .map(|r| r.source.value)
        .collect();
    assert_eq!(reports, [m.find_written("c1").unwrap(), m.find_written("c2").unwrap()]);
    // The evaluator is public.
    let expr = model::Expression::Afferent(Box::new(model::Expression::Element(m.find_written("s").unwrap())));
    assert_eq!(m.select_elements(&expr), [m.find_written("u").unwrap(), m.find_written("s").unwrap()]);
}

/// `!relationships <expr> { ... }` applies its children to every relationship `Model::select_relationships` returns; one case per relationship-expression form, plus && and ||.
#[test]
fn p2_168_relationships_directive_per_expression_form() {
    // Indices of `u -> c1`, `c1 -> c2`, `c2 -> t`.
    let cases: &[(&str, &[usize])] = &[
        ("*->*", &[0, 1, 2]),
        ("relationship==*", &[0, 1, 2]),
        ("c1->*", &[1]),
        ("*->t", &[2]),
        ("u->c1", &[0]),
        ("relationship==c1->*", &[1]),
        ("relationship.source==c1", &[1]),
        ("relationship.destination==c2", &[1]),
        ("relationship.tag==x,y", &[2]),
        ("relationship.tag!=x", &[0]),
        ("relationship.properties[latency]==low", &[1]),
        ("relationship.tag==x && relationship.source==c2", &[2]),
        ("relationship.source==u || relationship.destination==t", &[0, 2]),
    ];
    for (expression, indices) in cases {
        let src = in_model(&format!(
            "{EXPRESSIONS}\n        !relationship calls {{\n        }}\n        !relationships \"{expression}\" {{\n            tag \"Hit\"\n        }}"
        )
        .replace("c1 -> c2 \"calls\" \"\" \"x\"", "calls = c1 -> c2 \"calls\" \"\" \"x\" {\n            properties {\n                latency low\n            }\n        }"));
        let m = model_of(&src);
        let got: Vec<usize> = m
            .relationships
            .iter()
            .enumerate()
            .filter(|(_, r)| r.tags.iter().any(|t| t == "Hit"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(got, *indices, "{expression}");
    }
    let m = model_of(&in_model(&format!(
        "{EXPRESSIONS}\n        !relationships \"*->t\" {{\n            tags \"One\"\n            url https://example.com/r\n            properties {{\n                a b\n            }}\n            perspectives {{\n                Ops \"Run\"\n            }}\n        }}"
    )));
    let r = &m.relationships[2];
    assert!(r.tags.iter().any(|t| t == "One"));
    assert_eq!(r.url.as_ref().unwrap().value, "https://example.com/r");
    assert_eq!((r.properties.len(), r.perspectives.len()), (1, 1));
    let e = error_of(&in_model(&format!(
        "{EXPRESSIONS}\n        !relationships \"t->u\" {{\n            tag \"Hit\"\n        }}"
    )));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

// ---- Implied relationships (dsl-breadth-12)

/// With `!impliedRelationships false`, no lifted line is drawn.
#[test]
fn p2_7_implied_false_draws_no_lifted_line() {
    let m = model_of(&implied("        !impliedRelationships false\n"));
    assert_eq!(m.implied_relationships, model::ImpliedRelationships::False);
    let all = c4rizer_core::view::plan(&m, &m.views[1]);
    // Only `user -> shop` is drawn; `web -> smtp` would lift to shop -> mail.
    assert_eq!(all.edges.len(), 1);
    assert_eq!(all.edges[0].relationships, [model::RelationshipId(2)]);
    let lifted = c4rizer_core::view::plan(&model_of(&implied("")), &m.views[1]);
    assert_eq!(lifted.edges.len(), 2);
}

/// With `!impliedRelationships false`, `include *` counts only relationships declared on the element itself.
#[test]
fn p2_7_implied_false_star_counts_direct_only() {
    // TODO(srs): P2-7 a directive between relationships. Reading taken (B dsl-breadth 17): applies to later relationships only.
    let m = model_of(&implied("        !impliedRelationships false\n"));
    let b = builds(&m);
    // `mail` is linked to `shop` only through `web -> smtp`.
    assert_eq!(b[0].0, ["user", "shop"]);
    // `user -> web` is direct; `web -> smtp` reaches `mail` only by lifting.
    assert_eq!(b[2].0, ["user", "web"]);
    let d = builds(&model_of(&implied("")));
    assert_eq!(d[0].0, ["user", "shop", "mail"]);
    assert_eq!(d[2].0, ["user", "web", "mail"]);
}

/// `!impliedRelationships` between relationships applies only to the
/// relationships declared after it (Structurizr; B dsl-breadth 17):
/// `Relationship::implied_relationships` records the strategy in force.
#[test]
fn p2_7_implied_strategy_applies_to_later_relationships() {
    let between = |first: &str, second: &str| {
        in_workspace(
            &format!(
                "        user = person \"User\"\n        shop = softwareSystem \"Shop\" {{\n            web = container \"Web\"\n        }}\n        mail = softwareSystem \"Mail\" {{\n            smtp = container \"Smtp\"\n        }}\n{first}        user -> web \"Browses\"\n{second}        web -> smtp \"Sends\""
            ),
            "        systemContext shop \"c1\" {\n            include *\n        }\n        systemContext shop \"c1-all\" {\n            include user shop mail\n        }",
        )
    };
    let lines = |m: &Model| -> Vec<Vec<model::RelationshipId>> {
        c4rizer_core::view::plan(m, &m.views[1])
            .edges
            .into_iter()
            .map(|e| e.relationships)
            .collect()
    };
    // `false` after the first relationship: it still lifts, the second
    // does not.
    let m = model_of(&between("", "        !impliedRelationships false\n"));
    assert_eq!(m.relationships[0].implied_relationships, model::ImpliedRelationships::Default);
    assert_eq!(m.relationships[1].implied_relationships, model::ImpliedRelationships::False);
    assert_eq!(m.implied_relationships, model::ImpliedRelationships::False);
    assert_eq!(lines(&m), [vec![model::RelationshipId(0)]]);
    let keys: Vec<_> = m.included(&m.views[0]).iter().map(|&e| m.element(e).name.value.clone()).collect();
    assert_eq!(keys, ["User", "Shop"]);
    // The other way round: only the second lifts.
    let m = model_of(&between("        !impliedRelationships false\n", "        !impliedRelationships true\n"));
    assert_eq!(lines(&m), [vec![model::RelationshipId(1)]]);
    let keys: Vec<_> = m.included(&m.views[0]).iter().map(|&e| m.element(e).name.value.clone()).collect();
    assert_eq!(keys, ["Shop", "Mail"]);
}

/// `!impliedRelationships true` gives the same builds as no directive (CORE-5).
#[test]
fn p2_153_implied_true_equals_default() {
    let default = model_of(&implied(""));
    let m = model_of(&implied("        !impliedRelationships true\n"));
    assert_eq!(m.implied_relationships, model::ImpliedRelationships::True);
    assert_eq!(builds(&m), builds(&default));
    assert!(m.notices.is_empty());
}

/// `!impliedRelationships <fqcn>` behaves as default and adds one notice.
#[test]
fn p2_154_implied_fqcn_default_with_notice() {
    let fqcn = "com.structurizr.model.CreateImpliedRelationshipsUnlessAnyRelationshipExistsStrategy";
    let m = model_of(&implied(&format!("        !impliedRelationships {fqcn}\n")));
    let model::ImpliedRelationships::Strategy(name) = &m.implied_relationships else {
        panic!("{:?}", m.implied_relationships);
    };
    assert_eq!(name.value, fqcn);
    assert_eq!(builds(&m), builds(&model_of(&implied(""))));
    assert_eq!(m.notices.len(), 1);
    assert_eq!(m.notices[0].subject, "!impliedRelationships");
}

// ---- Terminology (dsl-breadth-13)

/// `terminology { softwareSystem "Service" }` (and person, container, component) replaces the word on box type lines and boundary tags.
#[test]
fn p2_161_terminology_replaces_type_words() {
    // TODO(srs): P2-161 External Software System. Reading taken (B dsl-breadth 18b): `External <word>`.
    let m = model_of(&terminology(
        "            person \"Actor\"\n            softwareSystem \"Service\"\n            container \"Unit\"\n            component \"Part\"\n            deploymentNode \"Machine\"\n            infrastructureNode \"Gear\"",
    ));
    let t = &m.terminology;
    let word = |l: &Option<model::Literal>| l.as_ref().map(|l| l.value.clone());
    assert_eq!(word(&t.person).as_deref(), Some("Actor"));
    assert_eq!(word(&t.software_system).as_deref(), Some("Service"));
    assert_eq!(word(&t.container).as_deref(), Some("Unit"));
    assert_eq!(word(&t.component).as_deref(), Some("Part"));
    assert_eq!(word(&t.deployment_node).as_deref(), Some("Machine"));
    assert_eq!(word(&t.infrastructure_node).as_deref(), Some("Gear"));
    let c1 = type_lines(&m, "c1");
    assert!(c1.contains(&("user".into(), "[Actor]".into())), "{c1:?}");
    assert!(c1.contains(&("shop".into(), "[Service]".into())), "{c1:?}");
    assert!(c1.contains(&("bank".into(), "[External Service]".into())), "{c1:?}");
    let c2 = type_lines(&m, "c2");
    assert!(c2.contains(&("web".into(), "[Unit: Rust]".into())), "{c2:?}");
    let c3 = type_lines(&m, "c3");
    assert!(c3.contains(&("api".into(), "[Part]".into())), "{c3:?}");
    // Boundary name tags take the word too.
    let tag = scene_of(&m, "c2").boundaries[0].tag.lines.join(" ");
    assert_eq!(tag, "Shop [Service]");
    let tag = scene_of(&m, "c3").boundaries[0].tag.lines.join(" ");
    assert_eq!(tag, "Web [Unit]");
    // grammar.dsl writes `terminology { person "Actor" }`.
    let (_, parsed) = grammar();
    let customer = parsed.model.find_written("customer").unwrap();
    let scene = scene_of(&parsed.model, "containers");
    let b = scene.boxes.iter().find(|b| b.element == customer).unwrap();
    assert_eq!(b.type_line.lines.join(" "), "[Actor]");
}

/// `terminology { relationship "Link" }` makes the key's line entries read Link.
#[test]
fn p2_162_terminology_relationship_key_label() {
    let m = model_of(&terminology("            relationship \"Link\""));
    assert_eq!(m.terminology.relationship.as_ref().unwrap().value, "Link");
    let names: Vec<String> = scene_of(&m, "c2")
        .legend
        .entries
        .iter()
        .flat_map(|e| e.names.clone())
        .collect();
    assert!(names.iter().any(|n| n == "Link"), "{names:?}");
    assert!(!names.iter().any(|n| n == "Relationship"), "{names:?}");
}

/// metadata round/curly/angle/none gives (…), {…}, <…>, no brackets; square is default.
#[test]
fn p2_163_terminology_metadata_brackets() {
    // TODO(srs): P2-163 double-angle glyphs. Reading taken (B dsl-breadth 18a): « and ».
    let cases = [
        ("round", model::MetadataSymbols::Round, "(Container: Rust)"),
        ("curly", model::MetadataSymbols::Curly, "{Container: Rust}"),
        ("angle", model::MetadataSymbols::Angle, "<Container: Rust>"),
        ("double-angle", model::MetadataSymbols::DoubleAngle, "«Container: Rust»"),
        ("square", model::MetadataSymbols::Square, "[Container: Rust]"),
        ("none", model::MetadataSymbols::None, "Container: Rust"),
    ];
    for (word, symbols, line) in cases {
        let m = model_of(&terminology(&format!("            metadata {word}")));
        assert_eq!(m.terminology.metadata.unwrap().value, symbols, "{word}");
        let c2 = type_lines(&m, "c2");
        assert!(c2.contains(&("web".into(), line.into())), "{word}: {c2:?}");
    }
    assert_eq!(model_of(&terminology("")).terminology.metadata, None);
}

// ---- Docs and ADRs (dsl-breadth-14)

/// `!docs <path> [fqcn]` on workspace, software system, container, component records an Attachment on that owner.
#[test]
fn p2_159_docs_path_recorded_on_owner() {
    // TODO(srs): P2-159 whether phase 2 reads or checks the files. Reading taken (B dsl-breadth 19): a missing path or one outside the folder is an error; content not read.
    let src = r#"workspace {
    !docs docs
    model {
        s = softwareSystem "S" {
            !docs docs/system com.example.Importer
            c = container "C" {
                !docs docs/container
                k = component "K" {
                    !docs "docs/component"
                }
            }
        }
    }
}
"#;
    let check = |m: &Model| {
        let paths = |d: &[model::Attachment]| -> Vec<String> {
            d.iter().map(|a| a.path.value.clone()).collect()
        };
        assert_eq!(paths(&m.docs), ["docs"]);
        assert_eq!(paths(&el(m, "s").docs), ["docs/system"]);
        assert_eq!(paths(&el(m, "c").docs), ["docs/container"]);
        assert_eq!(paths(&el(m, "k").docs), ["docs/component"]);
        let system = &el(m, "s").docs[0];
        assert_eq!(system.kind, model::AttachmentKind::Docs);
        assert_eq!(system.importer.as_ref().unwrap().value, "com.example.Importer");
        // The importer FQCN is accepted with no effect (P2-3).
        assert_eq!(m.notices.len(), 1);
        assert_eq!(m.notices[0].subject, "!docs");
    };
    // Parsed without a file: recorded, not checked.
    check(&model_of(src));
    let mut loader = Mem::default()
        .file("/w/docs/a.md", "x")
        .file("/w/docs/system/a.md", "x")
        .file("/w/docs/container/a.md", "x")
        .file("/w/docs/component/a.md", "x");
    check(&parsed_with(src, "/w/workspace.dsl", &mut loader).model);
    // A missing path, or one outside the diagram's folder, is an error.
    let missing = "workspace {\n    !docs nowhere\n}\n";
    let e = model::parse_with(missing, Some(std::path::Path::new("/w/workspace.dsl")), &mut loader)
        .unwrap_err();
    assert!(matches!(&e.kind, ModelErrorKind::FileRead { path, .. } if path == "nowhere"), "{e:?}");
    let mut outside = Mem::default().file("/elsewhere/a.md", "x");
    let src = "workspace {\n    !adrs ../elsewhere\n}\n";
    let e = model::parse_with(src, Some(std::path::Path::new("/w/workspace.dsl")), &mut outside)
        .unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::PathOutsideRoot { path: "../elsewhere".into() });
}

/// A `!docs`/`!adrs` path in an included file resolves against that
/// file's folder and must lie inside the diagram's (the root file's)
/// folder, not the included file's (corpus include-directory.dsl, whose
/// include/model/software-system/model.dsl has `!docs ../../docs`;
/// Structurizr 6.2.4 opens it).
#[test]
fn p2_159_attachment_path_inside_the_diagram_folder() {
    let root = "workspace {\n    model {\n        !include sub/deeper/model.dsl\n    }\n}\n";
    let mut loader = Mem::default()
        .file("/w/docs/a.md", "x")
        .file("/w/sub/deeper/model.dsl", "s = softwareSystem \"S\" {\n    !docs ../../docs\n}\n");
    let p = parsed_with(root, "/w/workspace.dsl", &mut loader);
    assert_eq!(el(&p.model, "s").docs[0].path.value, "../../docs");
    let mut outside = Mem::default()
        .file("/elsewhere/a.md", "x")
        .file("/w/sub/model.dsl", "s = softwareSystem \"S\" {\n    !adrs ../../elsewhere\n}\n");
    let root = "workspace {\n    model {\n        !include sub/model.dsl\n    }\n}\n";
    let e = model::parse_with(root, Some(std::path::Path::new("/w/workspace.dsl")), &mut outside)
        .unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::PathOutsideRoot { path: "../../elsewhere".into() });
}

/// `!docs <path> { exclude ... }` and `!adrs|!decisions <path> [importer]
/// { exclude ... }` (corpus docs/workspace.dsl, decisions/workspace.dsl;
/// Structurizr 6.2.4 opens both): the attachment is recorded as without
/// the block, and only `exclude <pattern>` lines are allowed in it.
// TODO(srs): P2-159, P2-160 name no block. Reading taken (Structurizr
// 6.2.4): `exclude` lines of one or more strings, kept nowhere, since
// phase 2 reads no documentation file.
#[test]
fn p2_159_attachment_block_with_exclude() {
    let m = model_of(
        "workspace {\n    !adrs adrs com.example.Importer {\n        exclude \"README.md\"\n    }\n    model {\n        s = softwareSystem \"S\" {\n            !docs docs {\n                exclude \"README.*\" \"*.txt\"\n            }\n            !decisions madr madr {\n                exclude \"README.md\"\n            }\n        }\n    }\n}\n",
    );
    assert_eq!(m.docs.len(), 1);
    assert_eq!(el(&m, "s").docs.len(), 2);
    assert_eq!(el(&m, "s").docs[0].path.value, "docs");
    let e = error_of("workspace {\n    !docs docs {\n        include \"x\"\n    }\n}\n");
    assert_eq!(e.kind, ModelErrorKind::UnknownConstruct { construct: "include".into() }, "{e:?}");
}

/// `!adrs <path> [type|fqcn]` and `!decisions` record path and importer, `AttachmentKind::Adrs`.
#[test]
fn p2_160_adrs_and_decisions_recorded() {
    use model::AttachmentKind::Adrs;
    let m = model_of(
        r#"workspace {
    !adrs decisions
    !adrs decisions/madr madr
    model {
        s = softwareSystem "S" {
            !decisions adr com.example.AdrImporter
        }
    }
}
"#,
    );
    let got: Vec<(model::AttachmentKind, String, Option<String>)> = m
        .docs
        .iter()
        .chain(&el(&m, "s").docs)
        .map(|a| (a.kind, a.path.value.clone(), a.importer.as_ref().map(|i| i.value.clone())))
        .collect();
    assert_eq!(
        got,
        [
            (Adrs, "decisions".into(), None),
            (Adrs, "decisions/madr".into(), Some("madr".into())),
            (Adrs, "adr".into(), Some("com.example.AdrImporter".into())),
        ]
    );
    // A type name is no FQCN; only the FQCN gives a notice.
    assert_eq!(m.notices.len(), 1);
    assert_eq!(m.notices[0].subject, "!decisions");
}

// ---- !include and workspace extends (dsl-breadth-15, -16)

/// `!include <relative file>` gives the same model as the text pasted in place.
#[test]
fn p2_5_include_file_inlines_statements() {
    let root = "workspace {\n    model {\n        !include people.dsl\n        user -> shop \"Buys\"\n    }\n}\n";
    let mut loader = Mem::default().file(
        "/w/people.dsl",
        "user = person \"User\"\nshop = softwareSystem \"Shop\" {\n    web = container \"Web\"\n}\n",
    );
    let p = parsed_with(root, "/w/workspace.dsl", &mut loader);
    let pasted = model_of(
        "workspace {\n    model {\nuser = person \"User\"\nshop = softwareSystem \"Shop\" {\n    web = container \"Web\"\n}\n        user -> shop \"Buys\"\n    }\n}\n",
    );
    assert_eq!(values(&p.model), values(&pasted));
    // `parse` has no file to include from.
    let e = error_of(root);
    assert_eq!(e.kind, ModelErrorKind::NeedsFile { construct: "!include".into() });
    assert_eq!(e.position.line, 3);
}

/// A `!const` before `workspace`, and one in an including file, fill a
/// `${NAME}` in an `!include` path inside an included file (corpus
/// big-bank-plc/internet-banking-system.dsl and system-landscape.dsl;
/// Structurizr 6.2.4 opens both).
// DECISION: constants and variables are one set for the whole parse, in
// document order with included files read where their `!include`
// stands, as the builder sees them (P2-150, P2-5).
#[test]
fn p2_5_include_path_takes_constants_of_including_files() {
    let root = "!const PART \"detail.dsl\"\n\nworkspace {\n    model {\n        !include model/people.dsl\n    }\n}\n";
    let mut loader = Mem::default()
        .file("/w/model/people.dsl", "s = softwareSystem \"S\" {\n    !include \"parts/${PART}\"\n}\n")
        .file("/w/model/parts/detail.dsl", "web = container \"Web\"\n")
        .file("/w/model/parts/summary.dsl", "");
    let p = parsed_with(root, "/w/workspace.dsl", &mut loader);
    assert_eq!(el(&p.model, "web").name.value, "Web");
}

/// `!include <dir>` inlines every DSL file of the directory.
#[test]
fn p2_133_include_directory() {
    // TODO(srs): P2-133 order, recursion, extensions. Reading taken (B dsl-breadth 5): recursive, ascending relative path byte order, dot-names skipped.
    let root = "workspace {\n    model {\n        !include parts\n    }\n}\n";
    let mut loader = Mem::default()
        .file("/w/parts/b.dsl", "b = person \"B\"")
        .file("/w/parts/a.txt", "a = person \"A\"")
        .file("/w/parts/0sub/c.dsl", "c = person \"C\"")
        .file("/w/parts/.hidden/h.dsl", "h = person \"Hidden\"")
        .file("/w/parts/.dot.dsl", "d = person \"Dot\"");
    let p = parsed_with(root, "/w/workspace.dsl", &mut loader);
    let names: Vec<&str> = p.model.elements.iter().map(|e| e.name.value.as_str()).collect();
    // Recursive, by relative path byte order, dot names skipped.
    assert_eq!(names, ["C", "A", "B"]);
    assert_eq!(
        p.paths[1..],
        ["/w/parts/0sub/c.dsl", "/w/parts/a.txt", "/w/parts/b.dsl"].map(std::path::PathBuf::from)
    );
}

/// A path resolving (after `..` and symlinks, `Loader::canonicalize`) outside the including file's directory is `PathOutsideRoot`.
#[test]
fn p2_134_include_outside_parent_dir_is_error() {
    let root = |inc: &str| format!("workspace {{\n    model {{\n        !include {inc}\n    }}\n}}\n");
    let mut loader = Mem::default()
        .file("/w/outside.dsl", "x = person \"X\"")
        .file("/w/sub/inside.dsl", "!include ../outside.dsl");
    let path = Some(std::path::Path::new("/w/sub/workspace.dsl"));
    let e = model::parse_with(&root("../outside.dsl"), path, &mut loader).unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::PathOutsideRoot { path: "../outside.dsl".into() });
    assert!(e.to_string().contains("../outside.dsl"), "{e}");
    // The check is per including file: inside.dsl may not climb either.
    let e = model::parse_with(&root("inside.dsl"), path, &mut loader).unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::PathOutsideRoot { path: "../outside.dsl".into() });
    assert_eq!(e.span.file, c4rizer_core::text::FileId(1));
    // A symlink inside the folder pointing outside it is refused.
    loader.links.insert("/w/sub/link.dsl".into(), "/w/outside.dsl".into());
    let e = model::parse_with(&root("link.dsl"), path, &mut loader).unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::PathOutsideRoot { path: "link.dsl".into() });
}

/// Included files get `FileId` 1.. in first-read order, `Parsed::paths` is indexed by `FileId`, every span names its file, each tree prints back byte for byte.
#[test]
fn p2_135_included_files_numbered_and_round_trip() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/dsl-breadth/include");
    let root_path = dir.join("workspace.dsl");
    let source = std::fs::read_to_string(&root_path).unwrap();
    let p = model::parse_with(&source, Some(&root_path), &mut model::FsLoader)
        .unwrap_or_else(|e| panic!("{e}"));
    let names: Vec<String> = p.paths[1..]
        .iter()
        .map(|p| p.strip_prefix(dir.canonicalize().unwrap()).unwrap().display().to_string())
        .collect();
    assert_eq!(names, ["people.dsl", "systems/mail.dsl", "systems/shop.dsl"]);
    assert_eq!(p.trees.len(), 4);
    for (i, tree) in p.trees.iter().enumerate() {
        assert_eq!(tree.file(), c4rizer_core::text::FileId(u32::try_from(i).unwrap()));
        let on_disk = if i == 0 {
            source.clone()
        } else {
            std::fs::read_to_string(&p.paths[i]).unwrap()
        };
        assert_eq!(tree.print(), on_disk, "{}", p.paths[i].display());
    }
    let m = &p.model;
    // Every span names the file it is in.
    let file = |k: &str| el(m, k).span.file.0;
    assert_eq!((file("user"), file("mail"), file("shop"), file("web")), (1, 2, 3, 3));
    assert_eq!(m.relationships[0].span.file.0, 0);
    let web = el(m, "web");
    assert_eq!(&p.trees[3].source()[web.name.span.range()], "\"Web\"");
    assert_eq!(m.included(&m.views[0]).len(), 3);
}

/// A parse or model error in an included file carries that file's `FileId`, line and column.
#[test]
fn p2_136_error_in_included_file_names_it() {
    let root = "workspace {\n    model {\n        !include good.dsl\n        !include bad.dsl\n    }\n}\n";
    let path = Some(std::path::Path::new("/w/workspace.dsl"));
    let mut loader = Mem::default()
        .file("/w/good.dsl", "a = person \"A\"\n")
        .file("/w/bad.dsl", "b = person \"B\"\n  bogus line\n");
    let e = model::parse_with(root, path, &mut loader).unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::UnknownConstruct { construct: "bogus".into() });
    assert_eq!(e.span.file, c4rizer_core::text::FileId(2));
    assert_eq!((e.position.line, e.position.column), (2, 3));
    // A syntax error too.
    let mut loader = Mem::default().file("/w/good.dsl", "a = person \"A\n").file("/w/bad.dsl", "");
    let e = model::parse_with(root, path, &mut loader).unwrap_err();
    assert!(matches!(e.kind, ModelErrorKind::Syntax(_)), "{e:?}");
    assert_eq!(e.span.file, c4rizer_core::text::FileId(1));
    assert_eq!(e.position.line, 1);
    // A missing file names the path written.
    let mut loader = Mem::default().file("/w/good.dsl", "");
    let e = model::parse_with(root, path, &mut loader).unwrap_err();
    assert!(matches!(&e.kind, ModelErrorKind::FileRead { path, .. } if path == "bad.dsl"), "{e:?}");
    assert_eq!((e.span.file.0, e.position.line), (0, 4));
}

/// A cycle, self-include included, is `IncludeCycle` naming the closing file, and never hangs.
#[test]
fn p2_137_include_cycle_is_error() {
    let root = "workspace {\n    model {\n        !include a.dsl\n    }\n}\n";
    let path = Some(std::path::Path::new("/w/workspace.dsl"));
    let mut loader = Mem::default()
        .file("/w/a.dsl", "!include b.dsl")
        .file("/w/b.dsl", "!include a.dsl");
    let e = model::parse_with(root, path, &mut loader).unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::IncludeCycle { path: "a.dsl".into() });
    assert_eq!(e.span.file, c4rizer_core::text::FileId(2));
    // A file including itself, and the root included again.
    let mut loader = Mem::default().file("/w/a.dsl", "!include a.dsl");
    let e = model::parse_with(root, path, &mut loader).unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::IncludeCycle { path: "a.dsl".into() });
    let mut loader = Mem::default().file("/w/a.dsl", "!include workspace.dsl");
    loader.files.insert("/w/workspace.dsl".into(), root.into());
    let e = model::parse_with(root, path, &mut loader).unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::IncludeCycle { path: "workspace.dsl".into() });
}

/// `!include http://...` is `InsecureUrl` naming the URL.
#[test]
fn p2_138_non_https_url_include_is_error() {
    // TODO(srs): P2-138 HTTPS fetch/cache/offline. Reading taken (B dsl-breadth 6): UrlNotSupported.
    let root = |inc: &str| format!("workspace {{\n    model {{\n        !include {inc}\n    }}\n}}\n");
    let path = Some(std::path::Path::new("/w/workspace.dsl"));
    let url = "http://example.com/model.dsl";
    let e = model::parse_with(&root(url), path, &mut Mem::default()).unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::InsecureUrl { url: url.into() });
    assert!(e.to_string().contains(url), "{e}");
    let url = "https://example.com/model.dsl";
    let e = model::parse_with(&root(url), path, &mut Mem::default()).unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::UrlNotSupported { url: url.into() });
}

/// `workspace extends <relative file>` puts the base's elements, relationships and views in the model.
#[test]
fn p2_16_extends_loads_base() {
    // TODO(srs): P2-16 URL and JSON bases, edits to base literals. Readings taken (B dsl-breadth 6, 9): UrlNotSupported; JSON base read (P3-419); edits patch the base file.
    let p = extends_fixture();
    let m = &p.model;
    assert_eq!(m.name.as_ref().unwrap().value, "Base");
    let keys: Vec<&str> = m.elements.iter().map(|e| e.key.as_ref().unwrap().value.as_str()).collect();
    assert_eq!(keys, ["user", "shop", "web", "mail"]);
    assert_eq!(m.relationships.len(), 2);
    let views: Vec<&str> = m.views.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(views, ["base-c1", "c2"]);
    // The base is the next file, its spans name it, and it prints back.
    assert_eq!(p.trees.len(), 2);
    assert_eq!(el(m, "user").span.file.0, 1);
    assert_eq!(el(m, "mail").span.file.0, 0);
    assert_eq!(p.trees[1].print(), std::fs::read_to_string(&p.paths[1]).unwrap());
    // URL and JSON bases (B dsl-breadth 6), and no file at all.
    let child = |base: &str| format!("workspace extends {base} {{\n}}\n");
    let path = Some(std::path::Path::new("/w/workspace.dsl"));
    let e = model::parse_with(&child("https://example.com/w.dsl"), path, &mut Mem::default()).unwrap_err();
    assert_eq!(e.kind, ModelErrorKind::UrlNotSupported { url: "https://example.com/w.dsl".into() });
    // A JSON base is read since the exporters row (P3-419); `{}` is JSON
    // but no workspace.
    let mut loader = Mem::default().file("/w/base.json", "{}");
    let e = model::parse_with(&child("base.json"), path, &mut loader).unwrap_err();
    assert!(matches!(e.kind, ModelErrorKind::FileFormat { ref path, .. } if path == "base.json"), "{e:?}");
    let e = error_of(&child("base.dsl"));
    assert_eq!(e.kind, ModelErrorKind::NeedsFile { construct: "workspace extends".into() });
}

/// A relationship or view in the extending file may reference a base id.
#[test]
fn p2_164_child_references_base_ids() {
    let p = extends_fixture();
    let m = &p.model;
    let r = &m.relationships[1];
    assert_eq!((r.source.value, r.destination.value), (m.find_written("web").unwrap(), m.find_written("mail").unwrap()));
    let c2 = m.view("c2").unwrap();
    assert_eq!(c2.scope.element(), m.find_written("shop"));
    assert_eq!(
        m.included(c2),
        vec![m.find_written("user").unwrap(), m.find_written("web").unwrap(), m.find_written("mail").unwrap()]
    );
    // A base id declared again in the child is a duplicate.
    let mut loader = Mem::default().file(
        "/w/base.dsl",
        "workspace {\n    model {\n        user = person \"User\"\n    }\n}\n",
    );
    let e = model::parse_with(
        "workspace extends base.dsl {\n    model {\n        user = person \"Other\"\n    }\n}\n",
        Some(std::path::Path::new("/w/workspace.dsl")),
        &mut loader,
    )
    .unwrap_err();
    assert!(matches!(e.kind, ModelErrorKind::DuplicateId { .. }), "{e:?}");
}

/// `!element <base id> { tags "X" }` adds tag X to the base element.
#[test]
fn p2_165_element_directive_modifies_base() {
    let p = extends_fixture();
    let shop = el(&p.model, "shop");
    assert_eq!(shop.tags, ["Element", "Software System", "Extended"]);
    // The element is still declared in the base file.
    assert_eq!(shop.span.file.0, 1);
}

// ---- Edits across files (dsl-breadth-17)

/// A DSL-10 edit of a literal from an included file patches that file only (`Applied::files`); `Applied::text` equals the root input.
#[test]
fn p2_139_text_edit_patches_included_file() {
    // TODO(srs): P2-139 per-box tags line for an included element. Reading taken (B dsl-breadth 8): the file holding the declaration.
    use c4rizer_core::edit::{self, Edit, ElementField, TextTarget};
    let (root, people, p) = included_pair();
    let set = |field, value: &str| Edit::SetText {
        target: TextTarget::Element {
            element: ElementKey::from_written("user"),
            field,
        },
        value: Some(value.into()),
        expected: None,
    };
    let applied = edit::apply(&p, &[set(ElementField::Name, "Customer"), set(ElementField::Description, "Pays.")])
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(applied.text, root);
    assert_eq!(applied.files.len(), 1);
    let (file, text) = &applied.files[0];
    assert_eq!(file.0, 1);
    assert_eq!(text, &people.replace("\"User\" \"Buys things.\"", "\"Customer\" \"Pays.\""));
    // The inverse gives the bytes back.
    let mut loader = Mem::default().file("/w/people.dsl", text);
    let next = parsed_with(&applied.text, "/w/workspace.dsl", &mut loader);
    let back = edit::apply(&next, &applied.inverse).unwrap();
    assert_eq!(back.files, [(*file, people.clone())]);
    // A per-box tag goes into the file declaring the element (B dsl-breadth 8).
    let styled = edit::apply(
        &p,
        &[Edit::SetStyle {
            element: ElementKey::from_written("user"),
            target: edit::StyleTarget::ThisBox,
            property: edit::StyleProperty::Background(Some(model::Colour { r: 1, g: 2, b: 3 })),
        }],
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(styled.files.len(), 1);
    assert!(styled.files[0].1.contains("tags "), "{}", styled.files[0].1);
    assert!(styled.text.contains("background #010203"), "{}", styled.text);
}

/// An at or size edit of a view declared in the root file is written to the root file.
#[test]
fn p2_140_at_and_size_go_to_root_file() {
    // TODO(srs): P2-140 a view declared in an included file. Reading taken (B dsl-breadth 7): the file holding the view block.
    use c4rizer_core::edit::{self, Edit};
    let (root, _, p) = included_pair();
    let applied = edit::apply(
        &p,
        &[
            Edit::Move {
                view: "c1".into(),
                element: ElementKey::from_written("user"),
                centre: Some((100, 200)),
            },
            Edit::Resize {
                view: "c1".into(),
                element: ElementKey::from_written("user"),
                size: Some((240, 160)),
            },
        ],
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(applied.files.is_empty());
    assert_ne!(applied.text, root);
    assert!(applied.text.contains("at user 100 200"), "{}", applied.text);
    assert!(applied.text.contains("size user 240 160"), "{}", applied.text);
    // A view declared in an included file gets its lines there (B
    // dsl-breadth 7).
    let views = "systemContext shop \"inc\" {\n    include *\n}\n";
    let root = "workspace {\n    model {\n        user = person \"User\"\n        shop = softwareSystem \"Shop\"\n        user -> shop\n    }\n    views {\n        !include views.dsl\n    }\n}\n";
    let mut loader = Mem::default().file("/w/views.dsl", views);
    let p = parsed_with(root, "/w/workspace.dsl", &mut loader);
    let applied = edit::apply(
        &p,
        &[Edit::Move {
            view: "inc".into(),
            element: ElementKey::from_written("shop"),
            centre: Some((40, 60)),
        }],
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(applied.text, root);
    assert!(applied.files[0].1.contains("at shop 40 60"), "{}", applied.files[0].1);
}

/// Under hierarchical ids every edit element key is `Model::full_key` and edit writes the full id.
#[test]
fn p2_142_edit_keys_are_full_hierarchical_ids() {
    use c4rizer_core::edit::{self, Edit, RelationshipRef};
    let src = in_workspace(
        "        !identifiers hierarchical\n        s1 = softwareSystem \"One\" {\n            api = container \"Api\"\n        }\n        s2 = softwareSystem \"Two\" {\n            api = container \"Api\"\n        }\n        s1.api -> s2.api \"Calls\"",
        "        container s1 \"c2\" {\n            include *\n        }",
    );
    let p = parsed(&src);
    let m = &p.model;
    let api = m.find_written("s1.api").unwrap();
    let key = m.full_key(api).unwrap();
    assert_eq!(key.as_str(), "s1.api");
    let applied = edit::apply(
        &p,
        &[Edit::Move {
            view: "c2".into(),
            element: key.clone(),
            centre: Some((100, 100)),
        }],
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(applied.text.contains("at s1.api 100 100"), "{}", applied.text);
    // The inverse names the element by its full id too.
    assert!(
        matches!(&applied.inverse[0], Edit::Move { element, .. } if element.as_str() == "s1.api"),
        "{:?}",
        applied.inverse
    );
    let r = RelationshipRef::of(m, m.relationships[0].id);
    assert_eq!((r.source.as_str(), r.destination.as_str()), ("s1.api", "s2.api"));
    let again = parsed(&applied.text);
    assert_eq!(again.model.views[0].position_of(api).unwrap().x.value, 100);
}

// ---- Pipeline (dsl-breadth-18)

/// A shop with a drawable container view between a dynamic and a
/// deployment view.
fn shop_with_unsupported_views() -> String {
    in_workspace(
        &format!("{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            node = deploymentNode \"Node\" {{\n                containerInstance web\n            }}\n        }}"),
        "        dynamic shop \"flow\" \"Checkout flow\" {
            user -> web \"Opens\"
            web -> db \"Saves\"
        }
        container shop \"c2\" {
            include *
            title \"Shop containers\"
        }
        deployment * live {
            include *
        }",
    )
}

/// `build_all` lists every non-drawable view in `AllViews::unsupported` (key, title, kind) and builds the rest as before; `has_errors` is unaffected.
#[test]
fn p2_2_build_all_lists_unsupported_views() {
    use c4rizer_core::model::ViewKind;
    let m = model_of(&shop_with_unsupported_views());
    let never = std::sync::atomic::AtomicBool::new(false);
    let all = c4rizer_core::pipeline::build_all(&m, &never).unwrap();
    let keys: Vec<&str> = all.views.iter().map(|v| v.plan.key.as_str()).collect();
    // dynamic-view (P3-200) and deployment-view (P3-2) draw both.
    assert_eq!(keys, ["flow", "c2", "Deployment-001"]);
    let listed: Vec<(&str, Option<&str>, ViewKind)> = all
        .unsupported
        .iter()
        .map(|u| (u.key.as_str(), u.title.as_deref(), u.kind))
        .collect();
    assert!(listed.is_empty(), "{listed:?}");
    // The drawn view is the same as in a file without the unsupported ones.
    let plain = model_of(&in_workspace(
        SHOP,
        "        container shop \"c2\" {\n            include *\n            title \"Shop containers\"\n        }",
    ));
    let alone = c4rizer_core::pipeline::build_all(&plain, &never).unwrap();
    // Spans differ (the views sit elsewhere in the file), the drawing not.
    assert_eq!(
        c4rizer_core::export::svg(&all.view("c2").expect("built").scene),
        c4rizer_core::export::svg(&alone.views[0].scene)
    );
    assert_eq!(all.has_errors(), alone.has_errors());
    assert!(alone.unsupported.is_empty());
    // Every view kind is drawable now (dynamic-view, deployment-view), so
    // no file reaches the CLI line text (P2-122) any more.
    for kind in [
        ViewKind::SystemContext,
        ViewKind::Container,
        ViewKind::Component,
        ViewKind::SystemLandscape,
        ViewKind::Filtered,
        ViewKind::Custom,
        ViewKind::Image,
        ViewKind::Dynamic,
        ViewKind::Deployment,
    ] {
        assert!(kind.drawable(), "{kind:?}");
    }
}

/// `build_view` on a non-drawable key is `BuildError::Unsupported`, never a panic; nothing is planned or routed.
#[test]
fn p2_2_build_view_on_unsupported_key_is_error() {
    use c4rizer_core::pipeline::BuildError;
    let m = model_of(&shop_with_unsupported_views());
    let never = std::sync::atomic::AtomicBool::new(false);
    let keep = c4rizer_core::place::Keep::default();
    // dynamic-view (P3-200) and deployment-view (P3-2) draw both, so
    // no key of this file is unsupported any more.
    assert!(c4rizer_core::pipeline::build_view(&m, "flow", &keep, &never).is_ok());
    assert!(c4rizer_core::pipeline::build_view(&m, "Deployment-001", &keep, &never).is_ok());
    assert!(c4rizer_core::pipeline::build_view(&m, "c2", &keep, &never).is_ok());
    // A stopped request on a formerly unsupported key is `Stopped` now.
    let stopped = std::sync::atomic::AtomicBool::new(true);
    assert!(matches!(
        c4rizer_core::pipeline::build_view(&m, "flow", &keep, &stopped),
        Err(BuildError::Stopped(_))
    ));

}
