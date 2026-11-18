//! JSON text as Structurizr writes it (Jackson's default pretty printer):
//! the value type and printer shared by the workspace JSON (P3-413) and
//! the theme (P3-421).

// DECISION: an in-house writer (no serde, no new dependency); objects
// keep the member order they are built in, so each caller decides
// Structurizr's order (sorted keys for a workspace, the theme's own
// order for a theme).

use std::fmt::Write as _;

/// One JSON value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub(crate) fn str(s: impl Into<String>) -> Value {
        Value::Str(s.into())
    }

    pub(crate) fn int(n: impl Into<i64>) -> Value {
        Value::Int(n.into())
    }
}

/// An object under construction.
#[derive(Debug, Default)]
pub(crate) struct Obj(pub(crate) Vec<(String, Value)>);

impl Obj {
    pub(crate) fn put(&mut self, key: &str, value: Value) -> &mut Obj {
        self.0.push((key.to_owned(), value));
        self
    }

    /// Puts `value` when it is `Some`.
    pub(crate) fn opt(&mut self, key: &str, value: Option<Value>) -> &mut Obj {
        if let Some(v) = value {
            self.put(key, v);
        }
        self
    }

    /// The object with its members sorted by key, as Structurizr's
    /// workspace writer orders them.
    pub(crate) fn sorted(mut self) -> Value {
        self.0.sort_by(|a, b| a.0.cmp(&b.0));
        Value::Obj(self.0)
    }

    pub(crate) fn value(self) -> Value {
        Value::Obj(self.0)
    }
}

/// `value` as Jackson's `DefaultPrettyPrinter` writes it: two spaces per
/// object level, arrays inline (`[ {`, `}, {`, `} ]`), `" : "` between a
/// key and its value, `{ }` and `[ ]` when empty; with a final newline
/// (P3-403).
pub(crate) fn print(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out.push('\n');
    out
}

fn write_value(out: &mut String, value: &Value, level: usize) {
    match value {
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Str(s) => string(out, s),
        Value::Arr(items) => {
            if items.is_empty() {
                out.push_str("[ ]");
                return;
            }
            out.push_str("[ ");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, item, level);
            }
            out.push_str(" ]");
        }
        Value::Obj(members) => {
            if members.is_empty() {
                out.push_str("{ }");
                return;
            }
            out.push('{');
            for (i, (k, v)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('\n');
                indent(out, level + 1);
                string(out, k);
                out.push_str(" : ");
                write_value(out, v, level + 1);
            }
            out.push('\n');
            indent(out, level);
            out.push('}');
        }
    }
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

/// A JSON string as Jackson escapes it.
fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ------------------------------------------------------------ workspace

// DECISION: members are written in Structurizr's sorted-key order and the
// model's arrays in id order; JSON is compared semantically (P3-406), so
// this order is a convenience, not a contract.
// DECISION: each element view carries `width` and `height` (its scene
// box, px) beside Structurizr's `x`/`y` (top-left), so a JSON read back
// gives the same `at` centre and `size` (P3-417); Structurizr ignores
// keys it does not know.
// TODO(srs): P3-414, documentation and decisions. Reading taken: core
// never reads the `!docs`/`!adrs` files, so `documentation` lists each
// attachment as a section titled by its path, with no content.
// TODO(srs): P3-417, c4rizer-only view lines (`subtitle`, `technology`,
// `bus`, `labelWidth`, `maxCrossings`, `focus`, `hide`) have no place in
// Structurizr's JSON. Reading taken: not written, so a JSON read back
// draws without them.

use crate::inspect::{self, Severity};
use crate::model::{
    Element, ElementId, ElementKind, Model, Property, Relationship, ViewKind, ViewScope, ViewSpec,
};
use crate::pipeline::AllViews;

use super::structurizr::{Workspace, base64};

/// The workspace JSON of `model` (`export::json`).
pub(crate) fn workspace(model: &Model, all: &AllViews, dsl: Option<&str>) -> String {
    let ws = Workspace::new(model);
    let mut root = Obj::default();
    root.put("configuration", configuration_scope(model));
    if let Some(d) = &model.description {
        root.put("description", Value::str(&d.value));
    }
    root.put("documentation", documentation(&model.docs));
    root.put("id", Value::int(0));
    root.put("model", model_json(&ws));
    if let Some(n) = &model.name {
        root.put("name", Value::str(&n.value));
    }
    let mut props = props_obj(&model.workspace_properties);
    if let Some(dsl) = dsl {
        props.put("structurizr.dsl", Value::str(base64(dsl.as_bytes())));
    }
    let violations = inspect::inspect(model);
    for (name, severity) in [
        ("error", Severity::Error),
        ("warning", Severity::Warning),
        ("info", Severity::Info),
        ("ignore", Severity::Ignore),
    ] {
        let n = violations.iter().filter(|v| v.severity == severity).count();
        props.put(&format!("structurizr.inspection.{name}"), Value::str(n.to_string()));
    }
    root.put("properties", props.sorted());
    root.put("views", views_json(&ws, all));
    print(&root.sorted())
}

fn configuration_scope(model: &Model) -> Value {
    let mut o = Obj::default();
    if let Some(s) = &model.configuration_scope {
        let scope = match s.value {
            crate::model::ConfigurationScope::Landscape => "Landscape",
            crate::model::ConfigurationScope::SoftwareSystem => "SoftwareSystem",
            _ => "None",
        };
        o.put("scope", Value::str(scope));
    }
    if !model.configuration_properties.is_empty() {
        o.put("properties", props_obj(&model.configuration_properties).sorted());
    }
    o.sorted()
}

fn documentation(docs: &[crate::model::Attachment]) -> Value {
    let mut o = Obj::default();
    let sections: Vec<Value> = docs
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let mut s = Obj::default();
            s.put("filename", Value::str(&a.path.value));
            s.put("order", Value::int(i64::try_from(i + 1).unwrap_or(i64::MAX)));
            s.put("title", Value::str(&a.path.value));
            s.sorted()
        })
        .collect();
    if !sections.is_empty() {
        o.put("sections", Value::Arr(sections));
    }
    o.sorted()
}

fn props_obj(list: &[Property]) -> Obj {
    let mut o = Obj::default();
    for p in list {
        // A later line sets the property again (P2-400).
        o.0.retain(|(k, _)| *k != p.name.value);
        o.put(&p.name.value, Value::str(&p.value.value));
    }
    o
}

fn id(n: u32) -> Value {
    Value::str(n.to_string())
}

fn model_json(ws: &Workspace<'_>) -> Value {
    let model = ws.model;
    let mut o = Obj::default();
    let top = |k: ElementKind| -> Vec<Value> {
        let mut list: Vec<&Element> = model.elements.iter().filter(|e| e.kind == k && e.parent.is_none()).collect();
        list.sort_by_key(|e| ws.id(e.id));
        list.into_iter().map(|e| element_json(ws, e)).collect()
    };
    let arrays = [
        ("customElements", ElementKind::Custom),
        ("deploymentNodes", ElementKind::DeploymentNode),
        ("people", ElementKind::Person),
        ("softwareSystems", ElementKind::SoftwareSystem),
    ];
    for (key, kind) in arrays {
        let list = top(kind);
        if !list.is_empty() {
            o.put(key, Value::Arr(list));
        }
    }
    if !model.model_properties.is_empty() {
        o.put("properties", props_obj(&model.model_properties).sorted());
    }
    o.sorted()
}

fn children_of(ws: &Workspace<'_>, e: ElementId, kind: ElementKind) -> Vec<Value> {
    let mut list: Vec<&Element> = ws
        .model
        .elements
        .iter()
        .filter(|c| c.parent == Some(e) && c.kind == kind)
        .collect();
    list.sort_by_key(|c| ws.id(c.id));
    list.into_iter().map(|c| element_json(ws, c)).collect()
}

#[allow(clippy::too_many_lines)] // one arm per element kind, as Structurizr's JSON
fn element_json(ws: &Workspace<'_>, e: &Element) -> Value {
    let model = ws.model;
    let mut o = Obj::default();
    o.put("id", id(ws.id(e.id)));
    let instance = e.instance_of.is_some();
    if !instance {
        o.put("name", Value::str(&e.name.value));
        if let Some(d) = &e.description {
            o.put("description", Value::str(&d.value));
        }
        if let Some(t) = &e.technology {
            o.put("technology", Value::str(&t.value));
        }
    }
    let tags: Vec<String> = if instance {
        std::iter::once(e.kind.default_tag().to_owned())
            .chain(e.declared_tags.iter().flat_map(|t| t.value.split(',').map(|x| x.trim().to_owned())))
            .filter(|t| !t.is_empty())
            .collect()
    } else {
        e.tags.clone()
    };
    if !tags.is_empty() {
        o.put("tags", Value::str(tags.join(",")));
    }
    if let Some(u) = &e.url {
        o.put("url", Value::str(&u.value));
    }
    let mut props = Obj::default();
    if let Some(k) = model.full_key(e.id) {
        props.put("structurizr.dsl.identifier", Value::str(k));
    }
    for (k, v) in props_obj(&e.properties).0 {
        props.put(&k, v);
    }
    if !props.0.is_empty() {
        o.put("properties", props.sorted());
    }
    if !e.perspectives.is_empty() {
        o.put("perspectives", perspectives(&e.perspectives));
    }
    if let Some(g) = e.group {
        o.put("group", Value::str(&model.groups[g.0 as usize].full_name));
    }
    if let Some(m) = &e.metadata {
        o.put("metadata", Value::str(&m.value));
    }
    let rels: Vec<Value> = ws
        .rels
        .iter()
        .enumerate()
        .filter(|(_, r)| r.source == e.id)
        .map(|(i, _)| relationship_json(ws, i))
        .collect();
    if !rels.is_empty() {
        o.put("relationships", Value::Arr(rels));
    }
    let env = || {
        e.environment
            .map(|i| Value::str(&model.environments[i.0 as usize].name.value))
    };
    match e.kind {
        ElementKind::SoftwareSystem => {
            o.put("documentation", Obj::default().value());
            let list = children_of(ws, e.id, ElementKind::Container);
            if !list.is_empty() {
                o.put("containers", Value::Arr(list));
            }
        }
        ElementKind::Container => {
            o.put("documentation", Obj::default().value());
            let list = children_of(ws, e.id, ElementKind::Component);
            if !list.is_empty() {
                o.put("components", Value::Arr(list));
            }
        }
        ElementKind::Component => {
            o.put("documentation", Obj::default().value());
        }
        ElementKind::DeploymentNode => {
            o.opt("environment", env());
            if let Some(i) = &e.instances {
                o.put("instances", Value::str(&i.value));
            }
            for (key, kind) in [
                ("children", ElementKind::DeploymentNode),
                ("containerInstances", ElementKind::ContainerInstance),
                ("infrastructureNodes", ElementKind::InfrastructureNode),
                ("softwareSystemInstances", ElementKind::SoftwareSystemInstance),
            ] {
                let list = children_of(ws, e.id, kind);
                if !list.is_empty() {
                    o.put(key, Value::Arr(list));
                }
            }
        }
        ElementKind::InfrastructureNode => {
            o.opt("environment", env());
        }
        ElementKind::ContainerInstance | ElementKind::SoftwareSystemInstance => {
            o.opt("environment", env());
            if let Some(of) = e.instance_of {
                let key = if e.kind == ElementKind::ContainerInstance { "containerId" } else { "softwareSystemId" };
                o.put(key, id(ws.id(of)));
            }
            o.put("instanceId", Value::int(e.instance_number.unwrap_or(1)));
            if !e.deployment_groups.is_empty() {
                o.put(
                    "deploymentGroups",
                    Value::Arr(e.deployment_groups.iter().map(Value::str).collect()),
                );
            }
        }
        _ => {}
    }
    o.sorted()
}

fn perspectives(list: &[crate::model::Perspective]) -> Value {
    Value::Arr(
        list.iter()
            .map(|p| {
                let mut x = Obj::default();
                x.put("name", Value::str(&p.name.value));
                x.opt("description", p.description.as_ref().map(|d| Value::str(&d.value)));
                x.opt("value", p.value.as_ref().map(|d| Value::str(&d.value)));
                x.opt("url", p.url.as_ref().map(|d| Value::str(&d.value)));
                x.sorted()
            })
            .collect(),
    )
}

fn relationship_json(ws: &Workspace<'_>, index: usize) -> Value {
    let r = &ws.rels[index];
    let mut out = Obj::default();
    out.put("id", id(r.id));
    out.put("sourceId", id(ws.id(r.source)));
    out.put("destinationId", id(ws.id(r.destination)));
    if !r.description.is_empty() {
        out.put("description", Value::str(&r.description));
    }
    if !r.technology.is_empty() {
        out.put("technology", Value::str(&r.technology));
    }
    if let Some(l) = r.linked {
        out.put("linkedRelationshipId", id(l));
    }
    let tags = ws.rel_tags(index);
    if !tags.is_empty() {
        out.put("tags", Value::str(tags.join(",")));
    }
    if let Some(own) = r.own.filter(|_| r.linked.is_none()) {
        let written: &Relationship = ws.model.relationship(own);
        if let Some(u) = &written.url {
            out.put("url", Value::str(&u.value));
        }
        if !written.perspectives.is_empty() {
            out.put("perspectives", perspectives(&written.perspectives));
        }
        if let Some(key) = &written.key {
            let mut props = Obj::default();
            props.put("structurizr.dsl.identifier", Value::str(&key.value));
            for (k, v) in props_obj(&written.properties).0 {
                props.put(&k, v);
            }
            out.put("properties", props.sorted());
        } else if !written.properties.is_empty() {
            out.put("properties", props_obj(&written.properties).sorted());
        }
    }
    out.sorted()
}

fn views_json(ws: &Workspace<'_>, all: &AllViews) -> Value {
    let model = ws.model;
    let mut out = Obj::default();
    let mut lists: Vec<(&str, Vec<Value>)> = Vec::new();
    for (i, spec) in model.views.iter().enumerate() {
        let key = match spec.kind {
            ViewKind::SystemLandscape => "systemLandscapeViews",
            ViewKind::SystemContext => "systemContextViews",
            ViewKind::Container => "containerViews",
            ViewKind::Component => "componentViews",
            ViewKind::Dynamic => "dynamicViews",
            ViewKind::Deployment => "deploymentViews",
            ViewKind::Filtered => "filteredViews",
            ViewKind::Image => "imageViews",
            _ => "customViews",
        };
        let v = view_json(ws, all, spec, i + 1);
        match lists.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(v),
            None => lists.push((key, vec![v])),
        }
    }
    for (key, list) in lists {
        out.put(key, Value::Arr(list));
    }
    let mut config = Obj::default();
    let mut styles = Obj::default();
    // A workspace sorts the keys a theme keeps in Structurizr's order.
    let sort = |v: Value| match v {
        Value::Obj(m) => Obj(m).sorted(),
        other => other,
    };
    let elements: Vec<Value> = super::theme_out::element_styles(model).into_iter().map(sort).collect();
    if !elements.is_empty() {
        styles.put("elements", Value::Arr(elements));
    }
    let relationships: Vec<Value> = super::theme_out::relationship_styles(model).into_iter().map(sort).collect();
    if !relationships.is_empty() {
        styles.put("relationships", Value::Arr(relationships));
    }
    config.put("styles", styles.sorted());
    let themes: Vec<Value> = model.themes.iter().map(|theme| Value::str(&theme.source.value)).collect();
    if !themes.is_empty() {
        config.put("themes", Value::Arr(themes));
    }
    let mut terms = Obj::default();
    let terminology = &model.terminology;
    for (name, value) in [
        ("person", &terminology.person),
        ("softwareSystem", &terminology.software_system),
        ("container", &terminology.container),
        ("component", &terminology.component),
        ("deploymentNode", &terminology.deployment_node),
        ("infrastructureNode", &terminology.infrastructure_node),
        ("relationship", &terminology.relationship),
    ] {
        terms.opt(name, value.as_ref().map(|l| Value::str(&l.value)));
    }
    config.put("terminology", terms.sorted());
    if let Some(v) = model.views.iter().rev().find(|v| v.default.is_some()) {
        config.put("defaultView", Value::str(&v.key));
    }
    if !model.views_properties.is_empty() {
        config.put("properties", props_obj(&model.views_properties).sorted());
    }
    out.put("configuration", config.sorted());
    out.sorted()
}

#[allow(clippy::too_many_lines)] // one key per Structurizr view field
fn view_json(ws: &Workspace<'_>, all: &AllViews, spec: &ViewSpec, order: usize) -> Value {
    let model = ws.model;
    let mut o = Obj::default();
    o.put("key", Value::str(&spec.key));
    o.put("order", Value::int(i64::try_from(order).unwrap_or(i64::MAX)));
    o.opt("title", spec.title.as_ref().map(|t| Value::str(&t.value)));
    o.opt("description", spec.description.as_ref().map(|t| Value::str(&t.value)));
    if spec.kind != ViewKind::Filtered && spec.kind != ViewKind::Image {
        o.put("name", Value::str(ws.view_name(spec)));
    }
    match (&spec.kind, &spec.scope) {
        (ViewKind::SystemContext | ViewKind::Container, ViewScope::Element(e)) => {
            o.put("softwareSystemId", id(ws.id(e.value)));
        }
        (ViewKind::Component, ViewScope::Element(e)) => {
            o.put("containerId", id(ws.id(e.value)));
        }
        (ViewKind::Dynamic | ViewKind::Image, ViewScope::Element(e)) => {
            o.put("elementId", id(ws.id(e.value)));
        }
        (
            ViewKind::Deployment,
            ViewScope::Deployment {
                software_system,
                environment,
            },
        ) => {
            if let Some(s) = software_system {
                o.put("softwareSystemId", id(ws.id(s.value)));
            }
            let env = model
                .environments
                .iter()
                .find(|e| e.name.value == environment.value || e.key.as_ref().is_some_and(|k| k.value == environment.value))
                .map_or_else(|| environment.value.clone(), |e| e.name.value.clone());
            o.put("environment", Value::str(env));
        }
        _ => {}
    }
    if let Some(f) = &spec.filter {
        o.put("baseViewKey", Value::str(&f.base.value));
        let mode = if matches!(f.mode.value, crate::model::FilterMode::Include) { "Include" } else { "Exclude" };
        o.put("mode", Value::str(mode));
        let tags = f.tags.value.split(',').map(|t| Value::str(t.trim())).collect();
        o.put("tags", Value::Arr(tags));
    }
    if spec.auto_layout.is_some() {
        let mut a = Obj::default();
        a.put("edgeSeparation", Value::int(0));
        a.put("implementation", Value::str("Graphviz"));
        a.put("nodeSeparation", Value::int(300));
        a.put("rankDirection", Value::str("TopBottom"));
        a.put("rankSeparation", Value::int(300));
        a.put("vertices", Value::Bool(false));
        o.put("automaticLayout", a.sorted());
    }
    if let Some(p) = &spec.paper_size {
        o.put("paperSize", Value::str(p.value.keyword()));
    }
    if !spec.properties.is_empty() {
        o.put("properties", props_obj(&spec.properties).sorted());
    }
    let scene = all.view(&spec.key).map(|b| &b.scene);
    if let Some(view) = ws.view(spec) {
        let elements = view
            .elements
            .iter()
            .map(|&e| {
                let mut x = Obj::default();
                x.put("id", id(ws.id(e)));
                let rect = scene.and_then(|s| s.boxes.iter().find(|b| b.element == e)).map(|b| b.rect);
                x.put("x", Value::int(rect.map_or(0, |r| r.min.x)));
                x.put("y", Value::int(rect.map_or(0, |r| r.min.y)));
                if let Some(r) = rect {
                    x.put("width", Value::int(r.max.x - r.min.x));
                    x.put("height", Value::int(r.max.y - r.min.y));
                }
                x.sorted()
            })
            .collect();
        o.put("elements", Value::Arr(elements));
        let relationships = view
            .relationships
            .iter()
            .map(|vr| {
                let r = &ws.rels[vr.rel];
                let mut x = Obj::default();
                x.put("id", id(r.id));
                if let Some(order) = &vr.order {
                    x.put("order", Value::str(order));
                    x.put("description", Value::str(&vr.description));
                    x.put("response", Value::Bool(vr.response));
                }
                let line = scene.and_then(|s| {
                    s.lines
                        .iter()
                        .find(|l| Some(l.relationship) == r.own && vr.order.is_none())
                });
                if let Some(l) = line.filter(|l| l.points.len() > 2) {
                    let vertices = l.points[1..l.points.len() - 1]
                        .iter()
                        .map(|p| {
                            let mut v = Obj::default();
                            v.put("x", Value::int(p.x));
                            v.put("y", Value::int(p.y));
                            v.sorted()
                        })
                        .collect();
                    x.put("vertices", Value::Arr(vertices));
                }
                x.sorted()
            })
            .collect();
        o.put("relationships", Value::Arr(relationships));
    }
    o.sorted()
}
