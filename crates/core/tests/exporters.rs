//! Tests of the exporters row, core part (SRS 4.7 P3-403, P3-406..P3-424):
//! the text formats against the Structurizr 6.2.4 reference outputs in
//! `tests/data/phase3/reference/`, JSON out and in, theme, static site.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use c4rizer_core::export::{self, Flavour};
use c4rizer_core::model::{self, FsLoader, Model};
use c4rizer_core::pipeline::{self, AllViews};

/// The repository's `tests/data`.
fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data")
}

/// The fixtures of ruling E1: reference folder and source file. `edge`
/// (fix round 1) holds groups, every shape, styled borders and lines,
/// quotes, angle brackets and non-ASCII text, a URL, an icon, a response
/// step and relationships to and from deployment nodes.
const FIXTURES: [(&str, &str); 4] = [
    ("example", "example.dsl"),
    ("deployment", "phase3/deployment.dsl"),
    ("dynamic", "phase3/dynamic.dsl"),
    ("edge", "phase3/edge.dsl"),
];

/// The model and built views of the file at `rel` (under `tests/data`).
fn load(rel: &str) -> (Model, AllViews) {
    let path = data().join(rel);
    let source = std::fs::read_to_string(&path).unwrap();
    let parsed = model::parse_with(&source, Some(&path), &mut FsLoader).unwrap();
    let all = pipeline::build_all(&parsed.model, &AtomicBool::new(false)).unwrap();
    (parsed.model, all)
}

/// The reference file `name` of format folder `format` of `fixture`, with
/// the final newline P3-403 adds where Structurizr writes none.
fn reference(fixture: &str, format: &str, name: &str) -> String {
    let path = data().join("phase3/reference").join(fixture).join(format).join(name);
    let mut text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// A fresh scratch folder for test `name`.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("c4rizer-exporters-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A JSON value, for reading what `export::json` writes (test-local:
/// core's reader is private).
#[derive(Debug, Clone, PartialEq)]
enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn items(&self) -> &[J] {
        match self {
            J::Arr(v) => v,
            _ => &[],
        }
    }
    fn str(&self) -> &str {
        match self {
            J::Str(s) => s,
            _ => "",
        }
    }
    fn num(&self) -> f64 {
        match self {
            J::Num(n) => *n,
            _ => f64::NAN,
        }
    }
}

fn parse_j(text: &str) -> J {
    fn ws(b: &[u8], i: &mut usize) {
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    }
    fn string(b: &[u8], i: &mut usize) -> String {
        *i += 1;
        let mut out = Vec::new();
        while b[*i] != b'"' {
            if b[*i] == b'\\' {
                *i += 1;
                match b[*i] {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        let h = std::str::from_utf8(&b[*i + 1..*i + 5]).unwrap();
                        let c = char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap();
                        out.extend(c.to_string().bytes());
                        *i += 4;
                    }
                    c => out.push(c),
                }
            } else {
                out.push(b[*i]);
            }
            *i += 1;
        }
        *i += 1;
        String::from_utf8(out).unwrap()
    }
    fn value(b: &[u8], i: &mut usize) -> J {
        ws(b, i);
        match b[*i] {
            b'{' => {
                *i += 1;
                let mut m = Vec::new();
                loop {
                    ws(b, i);
                    if b[*i] == b'}' {
                        *i += 1;
                        return J::Obj(m);
                    }
                    if b[*i] == b',' {
                        *i += 1;
                        ws(b, i);
                    }
                    let k = string(b, i);
                    ws(b, i);
                    *i += 1; // ':'
                    m.push((k, value(b, i)));
                }
            }
            b'[' => {
                *i += 1;
                let mut v = Vec::new();
                loop {
                    ws(b, i);
                    if b[*i] == b']' {
                        *i += 1;
                        return J::Arr(v);
                    }
                    if b[*i] == b',' {
                        *i += 1;
                    }
                    v.push(value(b, i));
                }
            }
            b'"' => J::Str(string(b, i)),
            b't' => {
                *i += 4;
                J::Bool(true)
            }
            b'f' => {
                *i += 5;
                J::Bool(false)
            }
            b'n' => {
                *i += 4;
                J::Null
            }
            _ => {
                let start = *i;
                while *i < b.len() && (b[*i] == b'-' || b[*i] == b'.' || b[*i].is_ascii_alphanumeric()) {
                    *i += 1;
                }
                J::Num(std::str::from_utf8(&b[start..*i]).unwrap().parse().unwrap())
            }
        }
    }
    let mut i = 0;
    value(text.as_bytes(), &mut i)
}

/// Every element of a workspace JSON as `id -> (name, parent id, tags)`
/// and every relationship as `id -> (source, destination, linked)`.
#[allow(clippy::type_complexity)]
fn json_model(
    root: &J,
) -> (
    std::collections::BTreeMap<String, (String, String, String)>,
    std::collections::BTreeMap<String, (String, String, String)>,
) {
    fn walk(
        e: &J,
        parent: &str,
        els: &mut std::collections::BTreeMap<String, (String, String, String)>,
        rels: &mut std::collections::BTreeMap<String, (String, String, String)>,
    ) {
        let id = e.get("id").map(J::str).unwrap_or_default().to_owned();
        let name = e.get("name").map(J::str).unwrap_or_default().to_owned();
        let tags = e.get("tags").map(J::str).unwrap_or_default().to_owned();
        els.insert(id.clone(), (name, parent.to_owned(), tags));
        for r in e.get("relationships").map(J::items).unwrap_or_default() {
            let f = |k: &str| r.get(k).map(J::str).unwrap_or_default().to_owned();
            rels.insert(f("id"), (f("sourceId"), f("destinationId"), f("linkedRelationshipId")));
        }
        for key in [
            "containers",
            "components",
            "children",
            "infrastructureNodes",
            "containerInstances",
            "softwareSystemInstances",
        ] {
            for c in e.get(key).map(J::items).unwrap_or_default() {
                walk(c, &id, els, rels);
            }
        }
    }
    let mut els = std::collections::BTreeMap::new();
    let mut rels = std::collections::BTreeMap::new();
    let m = root.get("model").unwrap();
    for key in ["people", "softwareSystems", "deploymentNodes", "customElements"] {
        for e in m.get(key).map(J::items).unwrap_or_default() {
            walk(e, "", &mut els, &mut rels);
        }
    }
    (els, rels)
}

/// Each view of a workspace JSON as `key -> (element ids, relationship
/// ids)`, both sorted.
fn json_views(root: &J) -> std::collections::BTreeMap<String, (Vec<String>, Vec<String>)> {
    let mut out = std::collections::BTreeMap::new();
    if let Some(J::Obj(lists)) = root.get("views") {
        for (k, list) in lists {
            if k == "configuration" {
                continue;
            }
            for v in list.items() {
                let ids = |key: &str| {
                    let mut l: Vec<String> = v
                        .get(key)
                        .map(J::items)
                        .unwrap_or_default()
                        .iter()
                        .map(|x| x.get("id").map(J::str).unwrap_or_default().to_owned())
                        .collect();
                    l.sort();
                    l
                };
                out.insert(v.get("key").map(J::str).unwrap_or_default().to_owned(), (ids("elements"), ids("relationships")));
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for c in text.bytes().filter(|&c| c != b'=') {
        acc = (acc << 6) | u32::try_from(ALPHABET.iter().position(|&a| a == c).unwrap()).unwrap();
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).unwrap());
        }
    }
    out
}

/// What P3-416 compares between two models: every element (name, kind,
/// parent, tags), every relationship (ends, texts, tags, copied or not),
/// every view (key, kind, scope) and every style block (as a set: block
/// order does not change what applies, by tag order).
fn summary(m: &Model) -> Vec<String> {
    let mut out = Vec::new();
    let mut styles = Vec::new();
    for e in &m.elements {
        let parent = e.parent.map(|p| m.element(p).name.value.clone()).unwrap_or_default();
        out.push(format!("element {} {:?} in {parent:?} {:?}", e.name.value, e.kind, e.tags));
    }
    for r in &m.relationships {
        let text = |l: &Option<model::Literal>| l.as_ref().map(|l| l.value.clone()).unwrap_or_default();
        out.push(format!(
            "relationship {} -> {} {:?} {:?} {:?} copied {}",
            m.element(r.source.value).name.value,
            m.element(r.destination.value).name.value,
            text(&r.description),
            text(&r.technology),
            r.tags,
            r.copied_from.is_some()
        ));
    }
    for v in &m.views {
        let scope = v.scope.element().map(|e| m.element(e).name.value.clone()).unwrap_or_default();
        out.push(format!("view {} {:?} {scope}", v.key, v.kind));
        for s in &v.steps {
            out.push(format!(
                "  step {} {} -> {}",
                s.order,
                m.element(s.source.value).name.value,
                m.element(s.destination.value).name.value
            ));
        }
    }
    for s in &m.element_styles {
        let v = |x: Option<String>| x.unwrap_or_default();
        styles.push(format!(
            "element style {} {:?} bg {} color {} stroke {} shape {:?} strokeWidth {:?} width {:?} height {:?} fontSize {:?} border {:?} opacity {:?} metadata {:?} description {:?} icon {:?} properties {:?}",
            s.tag.value,
            s.mode,
            v(s.background.map(|c| c.value.to_string())),
            v(s.color.map(|c| c.value.to_string())),
            v(s.stroke.map(|c| c.value.to_string())),
            s.shape.map(|c| c.value),
            s.stroke_width.map(|c| c.value),
            s.width.map(|c| c.value),
            s.height.map(|c| c.value),
            s.font_size.map(|c| c.value),
            s.border.map(|c| c.value),
            s.opacity.map(|c| c.value),
            s.metadata.map(|c| c.value),
            s.description.map(|c| c.value),
            s.icon.as_ref().map(|c| c.value.clone()),
            s.properties.iter().map(|p| (p.name.value.clone(), p.value.value.clone())).collect::<Vec<_>>(),
        ));
    }
    for s in &m.relationship_styles {
        styles.push(format!(
            "relationship style {} {:?} thickness {:?} color {:?} style {:?} routing {:?} jump {:?} fontSize {:?} width {:?} position {:?} opacity {:?} properties {:?}",
            s.tag.value,
            s.mode,
            s.thickness.map(|c| c.value),
            s.color.map(|c| c.value.to_string()),
            s.style.map(|c| c.value),
            s.routing.map(|c| c.value),
            s.jump.map(|c| c.value),
            s.font_size.map(|c| c.value),
            s.width.map(|c| c.value),
            s.position.map(|c| c.value),
            s.opacity.map(|c| c.value),
            s.properties.iter().map(|p| (p.name.value.clone(), p.value.value.clone())).collect::<Vec<_>>(),
        ));
    }
    styles.sort();
    out.extend(styles);
    out
}

/// Every file of `files` equals its reference in `format`, and the
/// reference folder holds no file c4rizer did not write.
fn assert_reference(fixture: &str, format: &str, files: &[(String, String)]) {
    let folder = data().join("phase3/reference").join(fixture).join(format);
    let mut expected: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    expected.sort();
    let mut written: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
    written.sort();
    assert_eq!(written, expected, "{fixture}/{format}: file names");
    for (name, text) in files {
        let want = reference(fixture, format, name);
        if *text != want {
            let line = text.lines().zip(want.lines()).position(|(a, b)| a != b);
            panic!(
                "{fixture}/{format}/{name} differs at line {:?}:\n--- c4rizer\n{}\n--- structurizr\n{}",
                line.map(|l| l + 1),
                line.and_then(|l| text.lines().nth(l)).unwrap_or("<end>"),
                line.and_then(|l| want.lines().nth(l)).unwrap_or("<end>")
            );
        }
    }
}

#[test]
fn p3_403_text_exports_deterministic() {
    let all_texts = |m: &Model, all: &AllViews| -> Vec<(String, String)> {
        let mut out = export::text::plantuml(m, all, Flavour::Structurizr);
        out.extend(export::text::plantuml(m, all, Flavour::C4PlantUml));
        out.extend(export::text::mermaid(m, all));
        out.extend(export::text::websequencediagrams(m, all));
        out.extend(export::text::dot(m, all, false));
        out.extend(export::text::dot(m, all, true));
        out.extend(export::text::ilograph(m, all));
        out.push(("json".into(), export::json(m, all, Some("workspace {}"))));
        out.push(("theme".into(), export::theme(m)));
        out
    };
    for (_, file) in FIXTURES.iter().copied().chain([("styles", "styles/all_styles.dsl")]) {
        // Two parses and builds, two exports: the same bytes.
        let (m1, a1) = load(file);
        let (m2, a2) = load(file);
        let first = all_texts(&m1, &a1);
        assert_eq!(first, all_texts(&m2, &a2), "{file}");
        assert_eq!(first, all_texts(&m1, &a1), "{file}");
        for (name, text) in &first {
            assert!(!text.contains('\r'), "{file}/{name}: \\n line endings only");
            assert!(text.ends_with('\n') && !text.ends_with("\n\n"), "{file}/{name}: one final newline");
        }
    }
}

#[test]
fn p3_406_plantuml_structurizr_equals_reference() {
    for (fixture, file) in FIXTURES {
        let (m, all) = load(file);
        assert_reference(fixture, "plantuml-structurizr", &export::text::plantuml(&m, &all, Flavour::Structurizr));
    }
}

#[test]
fn p3_407_c4plantuml_equals_reference() {
    for (fixture, file) in FIXTURES {
        let (m, all) = load(file);
        assert_reference(fixture, "plantuml-c4plantuml", &export::text::plantuml(&m, &all, Flavour::C4PlantUml));
    }
}

#[test]
fn p3_408_mermaid_equals_reference() {
    for (fixture, file) in FIXTURES {
        let (m, all) = load(file);
        assert_reference(fixture, "mermaid", &export::text::mermaid(&m, &all));
    }
}

#[test]
fn p3_409_websequencediagrams_equals_reference() {
    for fixture in ["dynamic", "edge"] {
        let (m, all) = load(&format!("phase3/{fixture}.dsl"));
        assert_reference(fixture, "websequencediagrams", &export::text::websequencediagrams(&m, &all));
    }
    // Static and deployment views give no file (P3-405).
    let (m, all) = load("example.dsl");
    assert!(export::text::websequencediagrams(&m, &all).is_empty());
}

#[test]
fn p3_410_dot_equals_reference() {
    for (fixture, file) in FIXTURES {
        let (m, all) = load(file);
        assert_reference(fixture, "dot", &export::text::dot(&m, &all, false));
    }
}

#[test]
fn p3_411_positions_only_with_flag() {
    let (m, all) = load("example.dsl");
    for (_, text) in export::text::dot(&m, &all, false) {
        assert!(!text.contains("pos="), "no position without the flag");
    }
    for (_, text) in export::text::plantuml(&m, &all, Flavour::Structurizr)
        .into_iter()
        .chain(export::text::mermaid(&m, &all))
        .chain(export::text::ilograph(&m, &all))
    {
        assert!(!text.contains("pos="), "no other text format carries positions");
    }
    let with = export::text::dot(&m, &all, true);
    let (_, c1) = with.iter().find(|(n, _)| n == "structurizr-C1.dot").unwrap();
    // `at worker 220 170` in example.dsl: the box centre, y up.
    assert!(c1.contains("1 [id=1,") && c1.contains(", pos=\"220,-170!\"]"), "{c1}");
    assert_eq!(c1.matches("pos=").count(), 5, "every node of C1 has its position");
}

#[test]
fn p3_412_ilograph_equals_reference() {
    for (fixture, file) in FIXTURES {
        let (m, all) = load(file);
        assert_reference(fixture, "ilograph", &export::text::ilograph(&m, &all));
    }
}

#[test]
fn p3_413_json_reads_back_to_same_plantuml() {
    for (fixture, file) in FIXTURES {
        let (m, all) = load(file);
        let json = export::json(&m, &all, None);
        let back = model::parse_json(json.as_bytes(), &mut FsLoader).unwrap_or_else(|e| panic!("{fixture}: {e}"));
        let all_back = pipeline::build_all(&back, &AtomicBool::new(false)).unwrap();
        assert_reference(fixture, "plantuml-structurizr", &export::text::plantuml(&back, &all_back, Flavour::Structurizr));
    }
}

#[test]
fn p3_414_json_holds_model_views_and_layout() {
    for (fixture, file) in FIXTURES {
        let (m, all) = load(file);
        let ours = parse_j(&export::json(&m, &all, None));
        let theirs = parse_j(&reference(fixture, "json", &format!("{fixture}.json")));
        // Every element (with its parent and tags) and every relationship,
        // implied and copied ones included, with Structurizr's ids.
        assert_eq!(json_model(&ours), json_model(&theirs), "{fixture}: model");
        // Every view with its elements and relationship views.
        assert_eq!(json_views(&ours), json_views(&theirs), "{fixture}: views");
        // Styles as the theme export writes them (P3-421).
        let styles = |j: &J| j.get("views").and_then(|v| v.get("configuration")).and_then(|c| c.get("styles")).cloned();
        assert_eq!(styles(&ours), styles(&theirs), "{fixture}: styles");
        // Each view element at its box's top-left, from the scene.
        for build in &all.views {
            let Some(v) = ours.get("views").and_then(|vs| match vs {
                J::Obj(lists) => lists.iter().flat_map(|(_, l)| l.items()).find(|v| v.get("key").map(J::str) == Some(build.plan.key.as_str())),
                _ => None,
            }) else {
                continue;
            };
            // A filtered view lists no elements of its own.
            if v.get("elements").is_none() {
                continue;
            }
            for b in &build.scene.boxes {
                let id = v
                    .get("elements")
                    .map(J::items)
                    .unwrap_or_default()
                    .iter()
                    .find(|e| {
                        e.get("x").map(J::num) == Some(f64::from(b.rect.min.x))
                            && e.get("y").map(J::num) == Some(f64::from(b.rect.min.y))
                            && e.get("width").map(J::num) == Some(f64::from(b.rect.max.x - b.rect.min.x))
                    });
                assert!(id.is_some(), "{fixture}/{}: a box at {:?}", build.plan.key, b.rect);
            }
        }
    }
    // A relationship view's vertices are the line's hand-added `via`
    // points (P3-318, vertices row): C2 of the example routes bent lines
    // but has no `via` line, so no relationship view carries vertices.
    let (m, all) = load("example.dsl");
    let ours = parse_j(&export::json(&m, &all, None));
    let c2 = all.view("C2").unwrap();
    assert!(c2.scene.lines.iter().any(|l| l.points.len() > 2), "C2 has bent lines");
    let views = ours.get("views").unwrap().get("containerViews").unwrap().items();
    let json_c2 = views.iter().find(|v| v.get("key").map(J::str) == Some("C2")).unwrap();
    assert!(
        json_c2.get("relationships").unwrap().items().iter().all(|r| r.get("vertices").is_none()),
        "router bends are not written as vertices"
    );
    // Theme references and properties at every level.
    let (m, all) = load("styles/all_styles.dsl");
    let j = parse_j(&export::json(&m, &all, None));
    let themes: Vec<&str> =
        j.get("views").unwrap().get("configuration").unwrap().get("themes").unwrap().items().iter().map(J::str).collect();
    assert_eq!(themes, ["default", "theme/theme.json"]);
    let (m, all) = load("phase3/edge.dsl");
    let j = parse_j(&export::json(&m, &all, None));
    assert_eq!(j.get("model").unwrap().get("properties").unwrap().get("structurizr.groupSeparator").map(J::str), Some("/"));
    let src = "workspace {\n    model {\n        s = softwareSystem \"S\" {\n            properties {\n                owner team\n            }\n        }\n    }\n    views {\n        properties {\n            look plain\n        }\n    }\n    properties {\n        stage beta\n    }\n}\n";
    let m = model::parse(src).unwrap().model;
    let all = pipeline::build_all(&m, &AtomicBool::new(false)).unwrap();
    let j = parse_j(&export::json(&m, &all, None));
    assert_eq!(j.get("properties").unwrap().get("stage").map(J::str), Some("beta"));
    assert_eq!(j.get("views").unwrap().get("configuration").unwrap().get("properties").unwrap().get("look").map(J::str), Some("plain"));
    let s = &j.get("model").unwrap().get("softwareSystems").unwrap().items()[0];
    assert_eq!(s.get("properties").unwrap().get("owner").map(J::str), Some("team"));
}

#[test]
fn p3_415_json_properties_hold_dsl_and_inspection() {
    let path = data().join("phase3/deployment.dsl");
    let source = std::fs::read_to_string(&path).unwrap();
    let (m, all) = load("phase3/deployment.dsl");
    let j = parse_j(&export::json(&m, &all, Some(&source)));
    let props = j.get("properties").unwrap();
    let decoded = base64_decode(props.get("structurizr.dsl").unwrap().str());
    assert_eq!(decoded, source.as_bytes(), "structurizr.dsl is the root file, base64");
    let violations = c4rizer_core::inspect::inspect(&m);
    for (name, severity) in [
        ("error", c4rizer_core::inspect::Severity::Error),
        ("warning", c4rizer_core::inspect::Severity::Warning),
        ("info", c4rizer_core::inspect::Severity::Info),
        ("ignore", c4rizer_core::inspect::Severity::Ignore),
    ] {
        let n = violations.iter().filter(|v| v.severity == severity).count();
        let key = format!("structurizr.inspection.{name}");
        assert_eq!(props.get(&key).unwrap().str(), n.to_string(), "{key}");
    }
    // Without the root file's text there is no `structurizr.dsl`.
    let j = parse_j(&export::json(&m, &all, None));
    assert!(j.get("properties").unwrap().get("structurizr.dsl").is_none());
}

#[test]
fn p3_416_json_gives_same_model() {
    for (fixture, file) in FIXTURES {
        let (dsl, _) = load(file);
        let text = reference(fixture, "json", &format!("{fixture}.json"));
        let json = model::parse_json(text.as_bytes(), &mut FsLoader).unwrap_or_else(|e| panic!("{fixture}: {e}"));
        let (a, b) = (summary(&json), summary(&dsl));
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x, y, "{fixture}");
        }
        assert_eq!(a.len(), b.len(), "{fixture}");
    }
}

#[test]
fn p3_417_json_render_gives_same_svg() {
    let source = r#"workspace "Shop" "Positions survive JSON." {
    model {
        customer = person "Customer" "Buys goods."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust"
            db = container "Db" "Stores orders." "SQL" "Database"
        }
        customer -> web "Browses" "HTTPS"
        web -> db "Reads and writes" "SQL"
    }
    views {
        systemContext shop "context" "The shop and its customers" {
            include *
            at customer 300 200
            at shop 300 600
        }
        container shop "containers" {
            include *
            title "Inside the shop"
            at customer 200 200
            at web 600 200
            at db 600 600
            size db 300 180
        }
        styles {
            element "Database" {
                shape cylinder
            }
        }
    }
}
"#;
    let m = model::parse(source).unwrap().model;
    let all = pipeline::build_all(&m, &AtomicBool::new(false)).unwrap();
    let json = export::json(&m, &all, Some(source));
    let back = model::parse_json(json.as_bytes(), &mut FsLoader).unwrap();
    let all_back = pipeline::build_all(&back, &AtomicBool::new(false)).unwrap();
    assert_eq!(all.views.len(), 2);
    for build in &all.views {
        let other = all_back.view(&build.plan.key).unwrap();
        // The positions come back as `at` and `size` lines.
        let spec = back.view(&build.plan.key).unwrap();
        assert!(!spec.positions.is_empty(), "{}: at lines", build.plan.key);
        assert_eq!(export::svg(&other.scene), export::svg(&build.scene), "{}", build.plan.key);
    }
}

#[test]
fn p3_418_malformed_json_is_parse_error() {
    // Not JSON: the error names the line and column.
    let err = model::parse_json(b"{\n  \"model\": {\n    \"people\": [ 1, ]\n", &mut FsLoader).unwrap_err();
    assert!(matches!(err.kind, model::ModelErrorKind::FileFormat { .. }), "{err:?}");
    assert_eq!(err.position.line, 3, "{err}");
    assert!(err.position.column > 1, "{err}");
    // Not UTF-8.
    let err = model::parse_json(b"{\"name\": \"\xff\"}", &mut FsLoader).unwrap_err();
    assert!(err.to_string().contains("UTF-8"), "{err}");
    // JSON, but not a workspace.
    for text in [&b"[1, 2]"[..], b"{\"name\": \"x\"}", b"\"model\""] {
        let err = model::parse_json(text, &mut FsLoader).unwrap_err();
        assert!(err.to_string().contains("workspace"), "{err}");
    }
    // An unknown key is ignored.
    let m = model::parse_json(
        br#"{"model": {"people": [{"id": "1", "name": "User", "tags": "Element,Person", "colour": "red"}]}, "flavour": 3}"#,
        &mut FsLoader,
    )
    .unwrap();
    assert_eq!(m.elements.len(), 1);
    assert_eq!(m.elements[0].name.value, "User");
}

#[test]
fn p3_419_extends_json_base() {
    // The pinned Structurizr corpus file extends a JSON base.
    let path = data().join("structurizr/extend/extend-workspace-from-json-file.dsl");
    let source = std::fs::read_to_string(&path).unwrap();
    let m = model::parse_with(&source, Some(&path), &mut FsLoader).unwrap_or_else(|e| panic!("{e}")).model;
    let names: Vec<&str> = m.elements.iter().map(|e| e.name.value.as_str()).collect();
    for name in ["User", "Software System 1", "Software System 2", "Software System 3", "Web Application", "Database"] {
        assert!(names.contains(&name), "{name} comes from the JSON base: {names:?}");
    }
    // The extending file adds to the base: `!element` blocks and lines.
    assert!(names.contains(&"Web Application 1") && names.contains(&"Web Application 2"), "{names:?}");
    let system1 = m.elements.iter().find(|e| e.name.value == "Software System 1").unwrap().id;
    let webapp1 = m.elements.iter().find(|e| e.name.value == "Web Application 1").unwrap();
    assert_eq!(webapp1.parent, Some(system1));
    let user = m.elements.iter().find(|e| e.name.value == "User").unwrap().id;
    assert!(m.relationships.iter().any(|r| r.source.value == user && r.destination.value == system1));
    // A c4rizer JSON as a base gives the model it was written from.
    let dir = scratch("p3_419");
    let (base, all) = load("phase3/deployment.dsl");
    std::fs::write(dir.join("base.json"), export::json(&base, &all, None)).unwrap();
    let child = "workspace extends base.json {\n    model {\n        auditor = person \"Auditor\"\n        auditor -> shop \"Audits\"\n    }\n}\n";
    let path = dir.join("child.dsl");
    let m = model::parse_with(child, Some(&path), &mut FsLoader).unwrap_or_else(|e| panic!("{e}")).model;
    assert_eq!(m.elements.len(), base.elements.len() + 1);
    assert_eq!(m.views.len(), base.views.len());
    assert!(m.elements.iter().any(|e| e.name.value == "Auditor"));
    // A base that is not a workspace JSON is an error naming the file.
    std::fs::write(dir.join("bad.json"), "[1, 2]").unwrap();
    let err = model::parse_with("workspace extends bad.json {\n}\n", Some(&dir.join("c.dsl")), &mut FsLoader).unwrap_err();
    assert!(err.to_string().contains("bad.json"), "{err}");
}

#[test]
fn p3_421_theme_equals_reference() {
    let (m, _) = load("styles/all_styles.dsl");
    let want = reference("styles", "", "all_styles-theme.json");
    assert_eq!(export::theme(&m), want);
}

#[test]
fn p3_422_theme_round_trips_styles() {
    // The styles fixture as Structurizr was given it (no theme lines).
    let path = data().join("phase3/reference/styles/structurizr.dsl");
    let source = std::fs::read_to_string(&path).unwrap();
    let dir = scratch("p3_422");
    std::fs::create_dir_all(dir.join("icons")).unwrap();
    std::fs::copy(data().join("styles/icons/database.svg"), dir.join("icons/database.svg")).unwrap();
    let a = model::parse_with(&source, Some(&dir.join("a.dsl")), &mut FsLoader).unwrap().model;
    std::fs::write(dir.join("workspace-theme.json"), export::theme(&a)).unwrap();
    // The same file with its styles block replaced by the theme.
    let start = source.find("        styles {").unwrap();
    let mut depth = 0;
    let mut end = start;
    for (i, c) in source[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    let themed = format!("{}theme workspace-theme.json{}", &source[..start], &source[end..]);
    let b = model::parse_with(&themed, Some(&dir.join("b.dsl")), &mut FsLoader).unwrap().model;
    assert!(b.element_styles.iter().all(|s| s.theme.is_some()), "every style comes from the theme");
    for mode in [model::Mode::Light, model::Mode::Dark] {
        for e in &a.elements {
            let (mut x, mut y) = (a.style_of_in(e.id, mode), b.style_of_in(e.id, mode));
            assert_eq!(
                x.icon.as_ref().map(|i| (i.mime, i.bytes.clone())),
                y.icon.as_ref().map(|i| (i.mime, i.bytes.clone())),
                "{} icon",
                e.name.value
            );
            (x.icon, y.icon) = (None, None);
            assert_eq!(x, y, "{} in {mode:?}", e.name.value);
        }
        for r in &a.relationships {
            assert_eq!(a.relationship_style_of_in(r.id, mode), b.relationship_style_of_in(r.id, mode));
        }
    }
}

/// The static site's EXP-1 scan (P3-423), on the markup, not on text
/// content: no `<script>` (or other active element), no event attribute,
/// no URI (`://`, `javascript:`, `data:`) in an attribute that fetches or
/// navigates, except the SVG namespace, no `url(` or `@import` in a `<style>` element or a `style`
/// attribute, and every `href`/`src` names a file of the site, or (in an
/// SVG, on an `<image>`) is an image `data:` URI with plain base64.
/// A URL written as text (a description) is no reference and passes.
fn static_scan(name: &str, text: &str, files: &[String]) -> Result<(), String> {
    let svg = Path::new(name).extension().is_some_and(|e| e == "svg");
    let mut rest = text;
    let mut in_style = false;
    while let Some(open) = rest.find('<') {
        let content = &rest[..open];
        if in_style && (content.contains("url(") || content.contains("@import")) {
            return Err(format!("{name}: url( or @import in a style element"));
        }
        let close = rest[open..].find('>').ok_or_else(|| format!("{name}: unclosed tag"))? + open;
        let tag = &rest[open + 1..close];
        rest = &rest[close + 1..];
        let element: String = tag.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '/' || *c == '!').collect::<String>().to_ascii_lowercase();
        match element.as_str() {
            "style" => in_style = true,
            "/style" => in_style = false,
            "script" | "iframe" | "object" | "embed" | "link" | "foreignobject" | "use" | "base" => {
                return Err(format!("{name}: <{element}> found"));
            }
            _ => {}
        }
        for (attr, value) in attributes(tag) {
            let attr = attr.to_ascii_lowercase();
            if attr.starts_with("on") {
                return Err(format!("{name}: event attribute {attr}"));
            }
            if attr == "xmlns" && value == "http://www.w3.org/2000/svg" {
                continue;
            }
            let lower = value.to_ascii_lowercase();
            if attr == "style" && (lower.contains("url(") || lower.contains("@import")) {
                return Err(format!("{name}: url( in a style attribute"));
            }
            if attr == "href" || attr == "src" || attr.ends_with(":href") {
                if let Some(data) = value.strip_prefix("data:image/") {
                    let payload = data.split_once(";base64,").map(|(_, p)| p).ok_or_else(|| format!("{name}: data URI not base64"))?;
                    if !(svg && element == "image") {
                        return Err(format!("{name}: data URI outside an SVG image"));
                    }
                    if payload.is_empty() || !payload.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)) {
                        return Err(format!("{name}: data URI payload is not plain base64"));
                    }
                    continue;
                }
                let file = value.split('#').next().unwrap_or_default();
                if !value.starts_with('#') && !files.iter().any(|f| *f == file) {
                    return Err(format!("{name}: {attr}=\"{value}\" names no file of the site"));
                }
                continue;
            }
            // Other attributes that fetch or navigate hold no URI either;
            // text attributes (`alt`, `title`) may quote a URL.
            let fetches = ["action", "formaction", "srcset", "poster", "data", "background", "cite", "ping", "codebase", "manifest", "xmlns"];
            if fetches.contains(&attr.as_str()) && (lower.contains("://") || lower.contains("javascript:") || lower.starts_with("data:")) {
                return Err(format!("{name}: {attr}=\"{value}\" holds a URI"));
            }
        }
    }
    Ok(())
}

/// The `name="value"` attributes of a tag's text (after `<`), values
/// with entities left as written.
fn attributes(tag: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = tag;
    while let Some(eq) = rest.find("=\"") {
        let name = rest[..eq].rsplit(|c: char| c.is_whitespace()).next().unwrap_or_default();
        let after = &rest[eq + 2..];
        let Some(end) = after.find('"') else { break };
        out.push((name, &after[..end]));
        rest = &after[end + 1..];
    }
    out
}

/// The site of the file at `rel` as `(path, text)` pairs.
fn site(rel: &str) -> (Model, AllViews, Vec<(String, String)>) {
    let (model, all) = load(rel);
    let files = export::static_site(&model, &all)
        .into_iter()
        .map(|(p, b)| (p.to_string_lossy().into_owned(), String::from_utf8(b).unwrap()))
        .collect();
    (model, all, files)
}

#[test]
fn p3_423_static_site_files_pass_exp1() {
    for (_, rel) in FIXTURES {
        let (_, all, files) = site(rel);
        let names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
        assert!(names.contains(&"index.html".to_owned()), "{rel}: {names:?}");
        // One `<key>.svg` per drawable view, the view's `name` mode SVG,
        // and one page per view.
        for view in &all.views {
            let key = &view.plan.key;
            let svg = files.iter().find(|(n, _)| *n == format!("{key}.svg")).unwrap_or_else(|| panic!("{rel}: {key}.svg"));
            assert_eq!(svg.1, export::svg(&view.scene), "{rel}: {key}.svg");
            assert!(names.contains(&format!("view-{key}.html")), "{rel}: page of {key}");
        }
        assert_eq!(files.len(), 1 + 2 * all.views.len(), "{rel}: {names:?}");
        for (name, text) in &files {
            static_scan(name, text, &names).unwrap();
            if Path::new(name).extension().is_some_and(|e| e == "html") {
                assert!(text.starts_with("<!DOCTYPE html>\n"), "{name}");
                assert!(text.ends_with("</html>\n"), "{name}");
            }
        }
        // Deterministic (P3-403).
        assert_eq!(site(rel).2, files, "{rel}");
    }
    // A link to anything outside the site fails the scan.
    assert!(static_scan("x.html", "<a href=\"https://example.org/\">x</a>", &[]).is_err());
    assert!(static_scan("x.html", "<script>alert(1)</script>", &[]).is_err());
    assert!(static_scan("x.html", "<p onclick=\"x()\">x</p>", &[]).is_err());
    assert!(static_scan("x.html", "<style>p { background: url(x.png) }</style>", &[]).is_err());
    assert!(static_scan("x.svg", "<image href=\"data:image/png;base64,AA==\"/>", &[]).is_ok());
    assert!(static_scan("x.svg", "<image href=\"data:image/png;base64,A%A\"/>", &[]).is_err());
    assert!(static_scan("x.html", "<img src=\"data:image/png;base64,AA==\">", &[]).is_err());
    assert!(static_scan("x.html", "<a href=\"data:text/html,x\">x</a>", &[]).is_err());
    // A URL written as text is no reference.
    assert!(static_scan("x.html", "<p>See https://example.org/docs</p>", &[]).is_ok());
    // A fixture whose descriptions hold URLs as text and whose style has
    // an icon (an SVG `data:` URI): every file passes.
    let dir = scratch("p3_423_urls");
    std::fs::create_dir_all(dir.join("icons")).unwrap();
    std::fs::copy(data().join("styles/icons/database.svg"), dir.join("icons/database.svg")).unwrap();
    let source = "workspace \"Docs\" \"Manual at https://example.org/manual?a=1&b=2.\" {\n    model {\n        u = person \"User\" \"Reads https://example.org/help.\"\n        s = softwareSystem \"Store\" \"Keeps data; see http://example.org/store.\" \"Db\"\n        u -> s \"Reads from https://example.org/api\"\n    }\n    views {\n        systemContext s \"ctx\" \"Context at https://example.org/ctx\" {\n            include *\n        }\n        styles {\n            element \"Db\" {\n                icon icons/database.svg\n            }\n        }\n    }\n}\n";
    let path = dir.join("urls.dsl");
    std::fs::write(&path, source).unwrap();
    let parsed = model::parse_with(source, Some(&path), &mut FsLoader).unwrap();
    let all = pipeline::build_all(&parsed.model, &AtomicBool::new(false)).unwrap();
    let files: Vec<(String, String)> = export::static_site(&parsed.model, &all)
        .into_iter()
        .map(|(p, b)| (p.to_string_lossy().into_owned(), String::from_utf8(b).unwrap()))
        .collect();
    let names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
    let svg = &files.iter().find(|(n, _)| n == "ctx.svg").unwrap().1;
    assert!(svg.contains("data:image/svg+xml;base64,"), "the icon is in the SVG");
    assert!(files.iter().any(|(_, t)| t.contains("https://example.org/")), "the URLs are in the text");
    for (name, text) in &files {
        static_scan(name, text, &names).unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn p3_424_static_site_index_and_links() {
    let (model, all, files) = site("example.dsl");
    let page = |name: &str| files.iter().find(|(n, _)| n == name).map_or_else(|| panic!("{name}"), |(_, t)| t.as_str());
    // The index lists every view in view list order with title and key.
    let index = page("index.html");
    assert!(index.contains("<title>Atomanja</title>"), "{index}");
    let mut at = 0;
    for spec in &model.views {
        let title = spec.drawn_title().unwrap().value.as_str();
        let entry = format!("<a href=\"view-{key}.html\">{title}</a> <code>{key}</code>", key = spec.key);
        let found = index[at..].find(&entry).unwrap_or_else(|| panic!("{entry} after byte {at} in {index}"));
        at += found + entry.len();
    }
    // Each view page shows its SVG and links back to the index.
    for view in &all.views {
        let key = &view.plan.key;
        let text = page(&format!("view-{key}.html"));
        assert!(text.contains(&format!("<img src=\"{key}.svg\"")), "{key}");
        assert!(text.contains("<a href=\"index.html\">"), "{key}");
    }
    // Enterable boxes link to the views they open (EDIT-15): the system
    // in C1 opens the container view C2, the container `app` in C2 opens
    // the first component view scoped to it.
    let c1 = page("view-C1.html");
    assert!(c1.contains("href=\"view-C2.html\" alt=\"Atomanja\""), "{c1}");
    let c2 = page("view-C2.html");
    assert!(c2.contains("href=\"view-C3-serving.html\" alt=\"Atomanja server\""), "{c2}");
    assert!(!c2.contains("view-C3-startup.html"), "only the first component view opens");
    // A person opens nothing.
    assert!(!c1.contains("alt=\"Worker\""), "{c1}");
    // The area sits on the box, in the SVG's own pixels.
    let scene = &all.view("C1").unwrap().scene;
    let system = model.find_written("system").unwrap();
    let b = scene.boxes.iter().find(|b| b.element == system).unwrap();
    let (x0, y0) = (b.rect.min.x - scene.bounds.min.x, b.rect.min.y - scene.bounds.min.y);
    let coords = format!("coords=\"{x0},{y0},{},{}\"", x0 + b.rect.width(), y0 + b.rect.height());
    assert!(c1.contains(&coords), "{coords} in {c1}");
}

/// A loader that serves nothing and records what was asked of it.
#[derive(Default)]
struct Watch {
    asked: std::cell::RefCell<Vec<String>>,
}

impl model::Loader for Watch {
    fn read(&mut self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.asked.borrow_mut().push(format!("read {}", path.display()));
        Err(std::io::Error::other("no files"))
    }
    fn read_dir(&mut self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        self.asked.borrow_mut().push(format!("dir {}", path.display()));
        Err(std::io::Error::other("no files"))
    }
    fn canonicalize(&mut self, path: &Path) -> std::io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }
    fn env(&self, name: &str) -> Option<String> {
        self.asked.borrow_mut().push(format!("env {name}"));
        Some("LEAKED".into())
    }
}

/// P3-418 with hostile values: no panic, no statement injected, no
/// variable substituted, no file or environment read.
#[test]
fn p3_418_hostile_json_never_injects() {
    let wrap = |people: &str, extra: &str| {
        format!(r#"{{"model": {{"people": [{people}]}}, "views": {{{extra}}}}}"#)
    };
    let person = |fields: &str| format!(r#"{{"id": "1", "name": "User", "tags": "Element,Person"{fields}}}"#);
    let inject = r"\n}\n!include /etc/passwd\nworkspace {\n";
    // Strings: `${NAME}` stays as written; a line break never ends the
    // string.
    for text in [
        wrap(&person(r#", "description": "Cost ${HOME} and ${USER}""#), ""),
        wrap(&person(&format!(r#", "description": "x{inject}""#)), ""),
        wrap(&person(&format!(r#", "properties": {{"k{inject}": "v{inject}"}}"#)), ""),
    ] {
        let mut watch = Watch::default();
        let m = model::parse_json(text.as_bytes(), &mut watch).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert!(watch.asked.borrow().is_empty(), "{:?}", watch.asked.borrow());
        assert_eq!(m.elements.len(), 1, "{text}");
    }
    let mut watch = Watch::default();
    let m = model::parse_json(wrap(&person(r#", "description": "Cost ${HOME}""#), "").as_bytes(), &mut watch).unwrap();
    assert_eq!(m.elements[0].description.as_ref().unwrap().value, "Cost ${HOME}");
    // Values the DSL takes unquoted are checked: nothing gets through.
    let node = |fields: &str| {
        format!(
            r#"{{"model": {{"deploymentNodes": [{{"id": "1", "name": "Box", "environment": "Live", "tags": "Element,Deployment Node"{fields}}}]}}}}"#
        )
    };
    let read = |text: &str| -> Model {
        let mut watch = Watch::default();
        let m = model::parse_json(text.as_bytes(), &mut watch).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert!(watch.asked.borrow().is_empty(), "{text}: {:?}", watch.asked.borrow());
        m
    };
    // `instances` that is not a number range is left out.
    let m = read(&node(&format!(r#", "instances": "2{inject}""#)));
    assert_eq!(m.elements.len(), 1);
    assert!(m.elements[0].instances.is_none());
    let m = read(&node(r#", "instances": "2..4""#));
    assert_eq!(m.elements[0].instances.as_ref().map(|i| i.value.as_str()), Some("2..4"));
    // Style values and terminology keys outside their grammar are left out.
    let m = read(&wrap(
        &person(""),
        &format!(r##""configuration": {{"styles": {{"elements": [{{"tag": "Person", "border": "Dashed{inject}", "shape": "Box{inject}", "background": "#123456"}}]}}, "terminology": {{"person{inject}": "Actor", "container": "Service"}}}}"##),
    ));
    assert_eq!(m.element_styles.len(), 1);
    let style = &m.element_styles[0];
    assert!(style.border.is_none() && style.shape.is_none(), "{style:?}");
    assert_eq!(style.background.map(|c| c.value.to_string()).as_deref(), Some("#123456"));
    assert!(m.terminology.person.is_none());
    assert_eq!(m.terminology.container.as_ref().map(|t| t.value.as_str()), Some("Service"));
    // A scope outside the three values is left out.
    let m = read(&format!(r#"{{"configuration": {{"scope": "landscape{inject}"}}, "model": {{}}}}"#));
    assert!(m.configuration_scope.is_none());
    assert!(m.elements.is_empty());
    // An order outside its grammar is left out: the step takes the next
    // number.
    let m = read(&wrap(
        &format!(r#"{}, {{"id": "2", "name": "Other", "tags": "Element,Person", "relationships": [{{"id": "3", "sourceId": "2", "destinationId": "1", "description": "Asks"}}]}}"#, person("")),
        &format!(r#""dynamicViews": [{{"key": "d", "relationships": [{{"id": "3", "order": "7{inject}"}}]}}]"#),
    ));
    assert_eq!(m.elements.len(), 2);
    assert_eq!(m.views.len(), 1);
    let steps = &m.views[0].steps;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].order, "1");
    assert!(steps[0].written_order.is_none());
    // A style number outside `i32` is an error naming the property.
    let text = wrap(&person(""), r#""configuration": {"styles": {"elements": [{"tag": "Person", "fontSize": 99999999999}]}}"#);
    let err = model::parse_json(text.as_bytes(), &mut Watch::default()).unwrap_err();
    assert!(err.to_string().contains("fontSize") && err.to_string().contains("out of range"), "{err}");
    // A fractional style number says so.
    let text = wrap(&person(""), r#""configuration": {"styles": {"elements": [{"tag": "Person", "fontSize": 12.5}]}}"#);
    let err = model::parse_json(text.as_bytes(), &mut Watch::default()).unwrap_err();
    assert!(err.to_string().contains("fontSize") && err.to_string().contains("not a whole number"), "{err}");
    // Ids: a number, or digits beyond u64, is an error, not a silent drop.
    for id in ["1", "\"99999999999999999999999\"", "\"x1\""] {
        let text = format!(r#"{{"model": {{"people": [{{"id": {id}, "name": "User"}}]}}}}"#);
        let err = model::parse_json(text.as_bytes(), &mut Watch::default()).unwrap_err();
        assert!(err.to_string().contains("id"), "{id}: {err}");
    }
    // A centre beyond i32 is an error, not an overflow.
    let text = wrap(
        &person(""),
        r#""systemLandscapeViews": [{"key": "l", "elements": [{"id": "1", "x": 2147483000, "y": 0, "width": 4000, "height": 100}]}]"#,
    );
    let err = model::parse_json(text.as_bytes(), &mut Watch::default()).unwrap_err();
    assert!(err.to_string().contains("out of range"), "{err}");
    // A written identifier `e7` never collides with a made-up one.
    let text = r#"{"model": {"people": [
        {"id": "3", "name": "A", "properties": {"structurizr.dsl.identifier": "e7"}},
        {"id": "7", "name": "B"},
        {"id": "8", "name": "C", "relationships": [{"id": "9", "sourceId": "8", "destinationId": "7", "description": "Uses"}]}]}}"#;
    let m = model::parse_json(text.as_bytes(), &mut Watch::default()).unwrap();
    assert_eq!(m.elements.len(), 3);
    let r = &m.relationships[0];
    assert_eq!(m.element(r.destination.value).name.value, "B");
}

/// P3-414, P3-416: perspectives (element and relationship), a view's
/// paper size and the default view survive JSON out and in.
#[test]
fn p3_416_json_keeps_perspectives_paper_size_and_default_view() {
    let src = r#"workspace {
    model {
        a = person "A" {
            perspectives {
                "Security" "Holds keys" "high"
            }
        }
        s = softwareSystem "S"
        a -> s "Uses" {
            perspectives {
                "Cost" "Per call"
            }
        }
    }
    views {
        systemContext s "ctx" {
            include *
            paperSize A4_Landscape
        }
        systemLandscape "all" {
            include *
            default
        }
    }
}
"#;
    let m = model::parse(src).unwrap().model;
    let all = pipeline::build_all(&m, &AtomicBool::new(false)).unwrap();
    let json = export::json(&m, &all, None);
    let j = parse_j(&json);
    assert_eq!(j.get("views").unwrap().get("configuration").unwrap().get("defaultView").map(J::str), Some("all"));
    let back = model::parse_json(json.as_bytes(), &mut FsLoader).unwrap();
    let persp = |p: &[model::Perspective]| -> Vec<(String, Option<String>, Option<String>)> {
        p.iter()
            .map(|x| (x.name.value.clone(), x.description.as_ref().map(|d| d.value.clone()), x.value.as_ref().map(|d| d.value.clone())))
            .collect()
    };
    assert_eq!(persp(&back.elements[0].perspectives), persp(&m.elements[0].perspectives));
    assert_eq!(persp(&back.relationships[0].perspectives), persp(&m.relationships[0].perspectives));
    assert!(!m.relationships[0].perspectives.is_empty());
    assert_eq!(back.view("ctx").unwrap().paper_size.map(|p| p.value), m.view("ctx").unwrap().paper_size.map(|p| p.value));
    assert!(back.view("ctx").unwrap().paper_size.is_some());
    assert!(back.view("all").unwrap().default.is_some());
    assert!(back.view("ctx").unwrap().default.is_none());
}

/// P3-406: a group's `http(s)` icon is drawn in its `PlantUML` boundary
/// (`startGroupBoundary`), scaled 0.5 (Structurizr's fallback; it would
/// measure the picture over the network). `Group`'s own icon draws its
/// colour instead, as Structurizr 6.2.4 does.
#[test]
fn p3_406_plantuml_group_icon() {
    let src = r#"workspace {
    model {
        group "Front" {
            a = softwareSystem "A"
        }
        group "Back" {
            b = softwareSystem "B"
        }
    }
    views {
        systemLandscape "l" {
            include *
        }
        styles {
            element "Group:Front" {
                icon https://example.com/front.png
            }
            element "Group" {
                icon https://example.com/all.png
                color #aa0000
            }
        }
    }
}
"#;
    let m = model::parse(src).unwrap().model;
    let all = pipeline::build_all(&m, &AtomicBool::new(false)).unwrap();
    let files = export::text::plantuml(&m, &all, Flavour::Structurizr);
    let text = &files[0].1;
    assert!(
        text.contains("rectangle \"Front\\n\\n<img:https://example.com/front.png{scale=0.5}>\" <<Group-RnJvbnQ=>> as group2 {"),
        "{text}"
    );
    assert!(text.contains("rectangle \"Back\\n\\n<img:#aa0000{scale=0.5}>\" <<Group-QmFjaw==>> as group1 {"), "{text}");
}

/// P3-416: identifiers c4rizer makes up (element, deployment group and
/// instance ids) never take one the JSON writes.
#[test]
fn p3_416_made_up_ids_never_take_written_ones() {
    let read = |text: &str| model::parse_json(text.as_bytes(), &mut Watch::default()).unwrap_or_else(|e| panic!("{e}\n{text}"));
    // A written `g0_0` beside a deployment group.
    let m = read(r#"{"model": {"softwareSystems": [{"id": "1", "name": "S", "properties": {"structurizr.dsl.identifier": "g0_0"}}],
        "deploymentNodes": [{"id": "2", "name": "DN", "environment": "Prod",
          "softwareSystemInstances": [{"id": "3", "softwareSystemId": "1", "environment": "Prod", "deploymentGroups": ["grp"], "instanceId": 1}]}]}}"#);
    assert_eq!(m.elements.len(), 3);
    assert_eq!(m.elements[2].deployment_groups, ["grp"]);
    // A written `zz_1` beside an instance of `zz` with no id: the instance
    // stays in the deployment view.
    let m = read(r#"{"model": {"softwareSystems": [{"id": "1", "name": "S", "properties": {"structurizr.dsl.identifier": "zz"}},
          {"id": "2", "name": "T", "properties": {"structurizr.dsl.identifier": "zz_1"}}],
        "deploymentNodes": [{"id": "4", "name": "DN", "environment": "Prod",
          "softwareSystemInstances": [{"id": "5", "softwareSystemId": "1", "environment": "Prod", "instanceId": 1}]}]},
      "views": {"deploymentViews": [{"key": "d", "environment": "Prod", "elements": [{"id": "4"}, {"id": "5"}]}]}}"#);
    let view = m.view("d").unwrap();
    let instance = m.elements.iter().find(|e| e.instance_of.is_some()).unwrap().id;
    assert!(m.included(view).contains(&instance), "{:?}", m.included(view));
    assert_eq!(m.find_written("zz_1").map(|e| m.element(e).name.value.clone()).as_deref(), Some("T"));
    // Written `e3` and `s_1` beside an element with no id, and a group.
    let m = read(r#"{"model": {"softwareSystems": [{"id": "1", "name": "S", "properties": {"structurizr.dsl.identifier": "zz"}},
          {"id": "2", "name": "T", "properties": {"structurizr.dsl.identifier": "e3"}}, {"id": "3", "name": "U"},
          {"id": "7", "name": "V", "properties": {"structurizr.dsl.identifier": "s_1"}}],
        "deploymentNodes": [{"id": "4", "name": "DN", "environment": "Prod", "children": [],
          "softwareSystemInstances": [{"id": "5", "softwareSystemId": "1", "environment": "Prod", "deploymentGroups": ["grp"], "instanceId": 1}]}]}}"#);
    assert_eq!(m.elements.len(), 6);
    assert_eq!(m.find_written("e3").map(|e| m.element(e).name.value.clone()).as_deref(), Some("T"));
}

/// P3-416 under `!identifiers hierarchical`: an instance whose implicit
/// id (`sys.api_1`) a written identifier takes gets an id of its own that
/// is a valid hierarchical one, stays in its deployment view and keeps it
/// through JSON out and in.
#[test]
fn p3_416_hierarchical_instance_id_taken() {
    let text = r#"{"model": {"softwareSystems": [{"id": "1", "name": "Sys", "properties": {"structurizr.dsl.identifier": "sys"},
          "containers": [{"id": "2", "name": "Api", "properties": {"structurizr.dsl.identifier": "sys.api"}},
            {"id": "3", "name": "Api one", "properties": {"structurizr.dsl.identifier": "sys.api_1"}}]}],
        "deploymentNodes": [{"id": "4", "name": "Box", "environment": "Live", "properties": {"structurizr.dsl.identifier": "box"},
          "containerInstances": [{"id": "5", "containerId": "2", "environment": "Live", "instanceId": 1}]}]},
      "views": {"deploymentViews": [{"key": "d", "environment": "Live", "elements": [{"id": "4"}, {"id": "5"}]}]}}"#;
    let m = model::parse_json(text.as_bytes(), &mut Watch::default()).unwrap_or_else(|e| panic!("{e}"));
    let instance = m.elements.iter().find(|e| e.instance_of.is_some()).unwrap().id;
    assert!(m.included(m.view("d").unwrap()).contains(&instance));
    assert_eq!(m.find_written("sys.api_1").map(|e| m.element(e).name.value.clone()).as_deref(), Some("Api one"));
    let all = pipeline::build_all(&m, &AtomicBool::new(false)).unwrap();
    let back = model::parse_json(export::json(&m, &all, None).as_bytes(), &mut Watch::default()).unwrap_or_else(|e| panic!("{e}"));
    let instance = back.elements.iter().find(|e| e.instance_of.is_some()).unwrap().id;
    assert!(back.included(back.view("d").unwrap()).contains(&instance));
}
