//! Box positions from a Structurizr workspace JSON, as a batch of
//! `edit::Edit::Move` plus notices (P2-15, P2-504..P2-511). No I/O: the
//! caller reads both files and applies the batch with one `edit::apply`.
//!
//! Depends on: `model`, `view`, `edit`.

use std::fmt;

use std::collections::BTreeMap;

use crate::edit::Edit;
use crate::model::{ElementId, ElementKind, Model, Notice, NoticeKind, Parsed, ViewKind, ViewSpec};
use crate::view;
use crate::text::Position;

mod json;

use json::Value;

/// What `import_layout` found: the edits to apply, one line per matched
/// view and every remark. Serves P2-15, P2-505..P2-511.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// One `Edit::Move { centre: Some(..) }` per matched box with an
    /// identifier, in JSON view order, then view-plan box order (P2-509).
    pub edits: Vec<Edit>,
    /// The matched views, in JSON view order (P2-505).
    pub views: Vec<ImportedView>,
    /// Skipped views, unmatched boxes and layout not imported, in JSON
    /// view order (P2-505, P2-507, P2-510, P2-511).
    pub notices: Vec<Notice>,
}

/// One view whose key is in both files. Serves P2-505.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedView {
    /// The view key.
    pub key: String,
    /// How many `at` positions the batch writes for it.
    pub positions: u32,
    /// True when the file's key was generated (`declared_key` is `None`).
    pub generated_key: bool,
}

/// Why the JSON gave no batch: not RFC 8259 JSON (with its line and
/// column), or not a Structurizr workspace (no top-level `views`
/// object, `position: None`). Serves P2-504.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    /// Line and column in the JSON text of a syntax error.
    pub position: Option<Position>,
    /// What is wrong and what to change (SRS 5.1).
    pub message: String,
}

impl fmt::Display for ImportError {
    /// `<line>:<column>: <message>`, or `<message>` without a position;
    /// the caller adds the path (P2-504).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(Position { line, column }) = self.position {
            write!(f, "{line}:{column}: ")?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for ImportError {}

/// Matches the views and boxes of `parsed` against `workspace_json`
/// and returns the `at` edits and notices. Never creates or deletes an
/// element, relationship or view. Serves P2-15, P2-504..P2-511.
///
/// # Errors
///
/// `ImportError` when `workspace_json` is not RFC 8259 JSON or has no
/// top-level `views` object.
pub fn import_layout(parsed: &Parsed, workspace_json: &str) -> Result<Import, ImportError> {
    let root = json::parse(workspace_json)
        .map_err(|e| ImportError { position: Some(e.position), message: e.message })?;
    let Some(views @ Value::Object(_)) = root.get("views") else {
        return Err(ImportError {
            position: None,
            message: "no top-level `views` object, so this is not a Structurizr workspace — export the workspace as JSON and pass that file".to_owned(),
        });
    };
    let model = &parsed.model;
    let json = JsonModel { elements: json_elements(root.get("model")), styles: size_styles(views) };
    let mut out = Import { edits: Vec::new(), views: Vec::new(), notices: Vec::new() };
    // P2-510 / P2-13: themes are named once and not fetched.
    let themes = views.get("configuration").and_then(|c| c.get("themes")).map(Value::items).unwrap_or_default();
    if !themes.is_empty() {
        out.notices.push(Notice {
            kind: NoticeKind::NotImported,
            subject: "themes".to_owned(),
            view: None,
            span: None,
            position: None,
        });
    }
    let Value::Object(members) = views else { return Ok(out) };
    for (array, list) in members {
        let array = array.as_str();
        // DECISION: `filteredViews` hold no positions and are ignored (a
        // file filtered view takes its base view's `at` lines, P2-9);
        // `configuration` and unknown members carry no views.
        let matched = MATCHED_ARRAYS.contains(&array);
        if !matched && !UNDRAWN_ARRAYS.contains(&array) {
            continue;
        }
        for json_view in list.items() {
            let Some(key) = json_view.get("key").and_then(Value::as_str) else { continue };
            if !matched {
                out.notices.push(notice(NoticeKind::ViewNotDrawable, key, key));
                continue;
            }
            let Some(spec) = model.view(key) else {
                out.notices.push(notice(NoticeKind::ViewNotInFile, key, key));
                continue;
            };
            // DECISION: a file view that holds no `at` lines of its own (a
            // filtered view, P2-9; an image view, which has no boxes) is
            // skipped like a JSON view c4rizer does not draw. Deployment views draw since the
            // deployment-view row but stay skipped here (approver ruling):
            // `deploymentViews` is in UNDRAWN_ARRAYS, with the notice.
            if !spec.kind.drawable() || matches!(spec.kind, ViewKind::Filtered | ViewKind::Image) {
                out.notices.push(notice(NoticeKind::ViewNotDrawable, key, key));
                continue;
            }
            import_view(model, spec, json_view, &json, &mut out);
        }
    }
    Ok(out)
}

/// The JSON view arrays `import` skips, each view with a
/// `ViewNotDrawable` notice (P2-510): image views are not drawn from
/// positions; deployment views draw but their positions are not imported
/// (approver ruling).
const UNDRAWN_ARRAYS: [&str; 2] = ["deploymentViews", "imageViews"];

/// The layout kinds of one JSON view that c4rizer cannot hold, each
/// named once, in this order: view `paperSize` and `dimensions`, then
/// relationship `vertices`, `routing` and `position` (P2-510).
fn not_imported(json_view: &Value) -> Vec<&'static str> {
    let mut out = Vec::new();
    for member in ["paperSize", "dimensions"] {
        if json_view.get(member).is_some_and(|v| *v != Value::Null) {
            out.push(member);
        }
    }
    let relationships = json_view.get("relationships").map(Value::items).unwrap_or_default();
    if relationships.iter().any(|r| r.get("vertices").is_some_and(|v| !v.items().is_empty())) {
        out.push("vertices");
    }
    for member in ["routing", "position"] {
        if relationships.iter().any(|r| r.get(member).is_some_and(|v| *v != Value::Null)) {
            out.push(member);
        }
    }
    out
}

/// The JSON view arrays whose views are matched by key (P2-505).
// DECISION: `dynamicViews` are matched since dynamic views draw and take
// `at` lines (dynamic-view row, P3-200); only box positions are taken,
// as for the other kinds (the boxes `view::plan` draws: the step ends).
const MATCHED_ARRAYS: [&str; 6] = [
    "systemLandscapeViews",
    "systemContextViews",
    "containerViews",
    "componentViews",
    "customViews",
    "dynamicViews",
];

/// What every view of the JSON shares: its model and element styles.
struct JsonModel {
    elements: BTreeMap<String, JsonElement>,
    styles: SizeStyles,
}

/// One element of the JSON model, as the merge strategy compares it.
struct JsonElement {
    /// `Person`, `SoftwareSystem`, `Container`, `Component` or `Custom`.
    kind: &'static str,
    name: String,
    description: String,
    /// Structurizr's canonical name (`CanonicalNameGenerator`).
    canonical: String,
    /// The comma-separated `tags`, in order.
    tags: Vec<String>,
}

/// Every static-structure element of the JSON model by id: people, then
/// software systems with their containers and components, then custom
/// elements (P2-506).
fn json_elements(model: Option<&Value>) -> BTreeMap<String, JsonElement> {
    let mut out = BTreeMap::new();
    let Some(model) = model else { return out };
    let mut add = |value: &Value, kind: &'static str, parents: &[&str]| {
        let Some(id) = value.get("id").and_then(Value::as_str) else { return };
        let name = value.get("name").and_then(Value::as_str).unwrap_or_default();
        let mut path: Vec<String> = parents.iter().map(|p| canonical_part(p)).collect();
        path.push(canonical_part(name));
        out.insert(
            id.to_owned(),
            JsonElement {
                kind,
                name: name.to_owned(),
                description: value.get("description").and_then(Value::as_str).unwrap_or_default().to_owned(),
                canonical: format!("{kind}://{}", path.join(".")),
                tags: value
                    .get("tags")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .split(',')
                    .map(|t| t.trim().to_owned())
                    .filter(|t| !t.is_empty())
                    .collect(),
            },
        );
    };
    for person in model.get("people").map(Value::items).unwrap_or_default() {
        add(person, "Person", &[]);
    }
    for system in model.get("softwareSystems").map(Value::items).unwrap_or_default() {
        add(system, "SoftwareSystem", &[]);
        let system_name = system.get("name").and_then(Value::as_str).unwrap_or_default();
        for container in system.get("containers").map(Value::items).unwrap_or_default() {
            add(container, "Container", &[system_name]);
            let container_name = container.get("name").and_then(Value::as_str).unwrap_or_default();
            for component in container.get("components").map(Value::items).unwrap_or_default() {
                add(component, "Component", &[system_name, container_name]);
            }
        }
    }
    for custom in model.get("customElements").map(Value::items).unwrap_or_default() {
        add(custom, "Custom", &[]);
    }
    out
}

/// A name as Structurizr puts it into a canonical name: `.` and `/`
/// removed (`CanonicalNameGenerator.formatName`).
// DECISION: follows the reference at the pinned commit, which differs
// from the plan's reading (names joined as written): Java wins (Part C).
fn canonical_part(name: &str) -> String {
    name.replace(['.', '/'], "")
}

/// The merge strategy's type of a c4rizer element.
const fn kind_name(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Person => "Person",
        ElementKind::SoftwareSystem => "SoftwareSystem",
        ElementKind::Container => "Container",
        ElementKind::Component => "Component",
        _ => "Custom",
    }
}

/// The canonical name of a file element, computed as `json_elements`
/// does (groups are not part of it).
fn file_canonical(model: &Model, id: ElementId) -> String {
    let mut parts = Vec::new();
    let mut current = Some(id);
    while let Some(e) = current {
        let element = model.element(e);
        parts.push(canonical_part(&element.name.value));
        current = element.parent;
    }
    parts.reverse();
    format!("{}://{}", kind_name(model.element(id).kind), parts.join("."))
}

/// A notice about view `view`.
fn notice(kind: NoticeKind, subject: &str, view: &str) -> Notice {
    Notice { kind, subject: subject.to_owned(), view: Some(view.to_owned()), span: None, position: None }
}

/// The JSON element view a drawn element takes its position from:
/// `DefaultLayoutMergeStrategy.findElementView`'s first three steps, each
/// over the JSON view's elements in order (P2-506).
fn find_match<'a>(
    model: &Model,
    id: ElementId,
    candidates: &[(&'a Value, &'a JsonElement)],
) -> Option<&'a Value> {
    let element = model.element(id);
    let canonical = file_canonical(model, id);
    let kind = kind_name(element.kind);
    let description = element.description.as_ref().map_or("", |d| d.value.as_str());
    candidates
        .iter()
        .find(|(_, j)| j.canonical == canonical)
        .or_else(|| candidates.iter().find(|(_, j)| j.kind == kind && j.name == element.name.value))
        .or_else(|| {
            if description.is_empty() {
                None
            } else {
                candidates.iter().find(|(_, j)| j.kind == kind && j.description == description)
            }
        })
        .map(|(v, _)| *v)
}

/// Adds the edits and notices of one JSON view matched to `spec`.
fn import_view(
    model: &Model,
    spec: &ViewSpec,
    json_view: &Value,
    json: &JsonModel,
    out: &mut Import,
) {
    let JsonModel { elements, styles } = json;
    let key = spec.key.as_str();
    let generated_key = spec.declared_key.is_none();
    if generated_key {
        out.notices.push(notice(NoticeKind::GeneratedKey, key, key));
    }
    let mut candidates = Vec::new();
    for element_view in json_view.get("elements").map(Value::items).unwrap_or_default() {
        let id = element_view.get("id").and_then(Value::as_str).unwrap_or_default();
        match elements.get(id) {
            Some(element) => candidates.push((element_view, element)),
            None => out.notices.push(notice(NoticeKind::UnknownJsonElement, id, key)),
        }
    }
    let plan = view::plan(model, spec);
    let mut positions = 0u32;
    for drawn in &plan.boxes {
        let Some(element_view) = find_match(model, drawn.element, &candidates) else {
            out.notices.push(notice(NoticeKind::UnmatchedBox, &drawn.name, key));
            continue;
        };
        let Some(element_key) = model.full_key(drawn.element) else {
            out.notices.push(notice(NoticeKind::NoIdentifier, &drawn.name, key));
            continue;
        };
        let json_id = element_view.get("id").and_then(Value::as_str).unwrap_or_default();
        let (w, h) = box_size(elements.get(json_id), styles);
        let x = element_view.get("x").and_then(Value::as_i32).unwrap_or(0);
        let y = element_view.get("y").and_then(Value::as_i32).unwrap_or(0);
        out.edits.push(Edit::Move { view: key.to_owned(), element: element_key, centre: Some((snap(x, w), snap(y, h))) });
        positions += 1;
    }
    for kind in not_imported(json_view) {
        out.notices.push(notice(NoticeKind::NotImported, kind, key));
    }
    out.views.push(ImportedView { key: key.to_owned(), positions, generated_key });
}

/// Structurizr's element size when no style sets one.
// TODO(srs): P2-508, the size when no JSON style sets width or height.
// Reading taken (Part B compat-fixtures 2): 450x300, Structurizr's
// default element style; one const so the ruling changes one line.
const DEFAULT_SIZE: (i32, i32) = (450, 300);

/// `views.configuration.styles.elements` as (tag, width, height), in
/// list order.
type SizeStyles = Vec<(String, Option<i32>, Option<i32>)>;

fn size_styles(views: &Value) -> SizeStyles {
    let list = views.get("configuration").and_then(|c| c.get("styles")).and_then(|s| s.get("elements"));
    list.map(Value::items)
        .unwrap_or_default()
        .iter()
        .filter_map(|style| {
            let tag = style.get("tag").and_then(Value::as_str)?;
            Some((
                tag.to_owned(),
                style.get("width").and_then(Value::as_i32),
                style.get("height").and_then(Value::as_i32),
            ))
        })
        .collect()
}

/// The width and height of a JSON element's box: each of its tags in
/// order, every style of that tag in list order, a later one winning per
/// property, as DSL-15 resolves styles (P2-508).
fn box_size(element: Option<&JsonElement>, styles: &SizeStyles) -> (i32, i32) {
    let (mut w, mut h) = DEFAULT_SIZE;
    for tag in element.map(|e| e.tags.as_slice()).unwrap_or_default() {
        for (style_tag, width, height) in styles {
            if style_tag == tag {
                w = width.unwrap_or(w);
                h = height.unwrap_or(h);
            }
        }
    }
    (w, h)
}

/// The `at` coordinate for a box at top-left `start` with extent
/// `extent`: its centre, snapped to the 10px grid (P2-508).
// DECISION: a half rounds toward positive infinity, computed in doubled
// integer units so no float rounding is involved: `(2v + 10) div 20 * 10`
// with `2v = 2 * start + extent`.
fn snap(start: i32, extent: i32) -> i32 {
    let doubled = i64::from(start) * 2 + i64::from(extent);
    let snapped = (doubled + 10).div_euclid(20) * 10;
    i32::try_from(snapped).unwrap_or(if snapped < 0 { i32::MIN } else { i32::MAX })
}

#[cfg(test)]
mod tests {
    use super::json::{self, Value};

    #[test]
    fn p2_504_json_reader_rfc8259() {
        let src = r#" {"a": [1, -2.5, 3e2, 4E-1, 0, -0.0], "s": "q\"b\\s\/f\bf\fn\nr\rt\t\u00e9\ud83d\ude00",
            "t": true, "f": false, "n": null, "o": {"k": {"k2": []}}, "dup": 1, "dup": 2} "#;
        let v = json::parse(src).expect("valid JSON");
        let nums: Vec<f64> = v.get("a").expect("a").items().iter().map(|x| match x {
            Value::Number(n) => *n,
            other => panic!("{other:?}"),
        }).collect();
        assert_eq!(nums, vec![1.0, -2.5, 300.0, 0.4, 0.0, -0.0]);
        assert_eq!(v.get("s").and_then(Value::as_str), Some("q\"b\\s/f\u{8}f\u{c}n\nr\rt\t\u{e9}\u{1F600}"));
        assert_eq!(v.get("t"), Some(&Value::Bool(true)));
        assert_eq!(v.get("f"), Some(&Value::Bool(false)));
        assert_eq!(v.get("n"), Some(&Value::Null));
        assert_eq!(v.get("o").and_then(|o| o.get("k")).and_then(|k| k.get("k2")), Some(&Value::Array(vec![])));
        assert_eq!(v.get("dup").and_then(Value::as_i32), Some(2), "a duplicate key takes its last value");
        if let Value::Object(members) = &v {
            let keys: Vec<&str> = members.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(keys, ["a", "s", "t", "f", "n", "o", "dup", "dup"], "key order kept");
        }
        assert_eq!(Value::Number(2.5).as_i32(), Some(3));
        for bad in ["", "{", "[1,]", "{\"a\" 1}", "01", "1.", "1e", "\"\\x\"", "\"\\ud800\"", "tru", "{} x", "\"a\nb\""] {
            assert!(json::parse(bad).is_err(), "{bad:?} is not RFC 8259 JSON");
        }
    }

    #[test]
    fn p2_504_json_syntax_error_line_column() {
        let err = json::parse("{\n  \"views\": {\n    \"x\": 1\n").expect_err("truncated");
        assert_eq!((err.position.line, err.position.column), (4, 1));
        let err = super::import_layout(&crate::model::parse("workspace {\n}\n").expect("parses"), "{\n  \"a\": tru\n}")
            .expect_err("bad word");
        let pos = err.position.expect("a syntax error has a position");
        assert_eq!((pos.line, pos.column), (2, 8));
        assert!(err.to_string().starts_with("2:8: "), "{err}");
    }
}
