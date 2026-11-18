//! Structurizr workspace JSON in (SRS 4.7 P3-416..P3-419): the JSON is
//! read with the shared reader (`text::json`) and written back as the DSL
//! text Structurizr's DSL would need to build that workspace, which the
//! builder then reads like any file (`parse_json`, `workspace extends
//! <file>.json`).

// DECISION: JSON becomes DSL text, not a model built field by field: the
// builder then makes the implied and copied relationships, resolves the
// styles and the views exactly as for a DSL file, and creation order (so
// Structurizr's ids, P3-408) is the text order: elements and written
// relationships are written in id order, each element inside its
// parent's block, each deployment node inside its environment's block.
// Implied and copied relationships (those with `linkedRelationshipId`)
// are left for the builder to make again.
// Spans of JSON-sourced items point into the DSL text written here
// (`FileId::ROOT` for `parse_json`, the base's own file for `extends`),
// as ruled; errors in the JSON itself carry its line and column.
// DECISION: an element keeps its `structurizr.dsl.identifier` as its id
// when that is a plain identifier not used before, else gets `e<id>`
// (with `_` added until no written identifier has it).
// DECISION: every JSON string lands in the DSL as a quoted string, `"`
// and `\` escaped and `${` written as `${c4rizer_dollar}{` (a constant
// holding `$`), so no JSON text substitutes a variable, reads the
// environment or starts a statement; values the DSL takes unquoted
// (instances, orders, style values, terminology keys, scope) are checked
// against their grammar and left out when they do not match. A line
// break inside a string becomes a space: a DSL string cannot hold one.
// TODO(srs): P3-419, the `!const c4rizer_dollar` the escape needs is
// visible to a file that `extends` the JSON (docs/deferred.md): the DSL
// has no other way to write `${`.
// DECISION: every identifier made up here (`e<id>`, deployment groups,
// an instance's `<element>_<n>`) goes through `fresh`, so none takes one
// the JSON writes; an instance whose implicit id is taken gets one of its
// own.
// DECISION: a style number outside `i32` is an error, like an
// out-of-range position.
// DECISION: an element or relationship id that is not a string of digits
// fitting `u64`, a position whose centre overflows `i32`, is an error.
// DECISION: a view element's `at` is the centre of the box at `x`, `y`
// (top-left) of size `width` x `height` when the JSON gives them (c4rizer
// writes them, P3-417) and then a `size` line too; with only `x`, `y` the
// box is Structurizr's default 450x300; `0, 0` everywhere (a workspace
// never laid out) writes no `at`.
// TODO(srs): P3-416, image views, documentation, relationship views a
// view excludes and theme files are not read back. Reading taken: left
// out (a theme URL is kept).
// A relationship view's `vertices` become the view's `via` line for that
// line (P3-318, P3-417), each point snapped to the 10 px grid; a vertex
// without two in-range numbers drops that line's vertices, and the first
// relationship view of a drawn line wins. `model::parse_json` drops any
// generated `via` line the built view refuses (a line it does not draw).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::text::json::{self, Value};
use crate::text::Position;

/// Why a JSON text is not a workspace, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct JsonError {
    pub(super) position: Position,
    pub(super) message: String,
}

/// The DSL text of the workspace JSON `source`.
pub(super) fn to_dsl(source: &str) -> Result<String, JsonError> {
    let root = json::parse(source).map_err(|e| JsonError {
        position: e.position,
        message: e.message,
    })?;
    let workspace = matches!(root, Value::Object(_))
        && (matches!(root.get("model"), Some(Value::Object(_))) || matches!(root.get("views"), Some(Value::Object(_))));
    if !workspace {
        return Err(JsonError {
            position: Position { line: 1, column: 1 },
            message: "the JSON is not a Structurizr workspace (no `model` or `views` object)".into(),
        });
    }
    let w = Writer::new(&root);
    let text = w.workspace()?;
    Ok(text)
}

/// The style keys written back as DSL style lines.
const STYLE_KEYS: [&str; 19] = [
    "background", "color", "stroke", "strokeWidth", "shape", "icon", "width", "height", "fontSize", "border",
    "opacity", "metadata", "description", "thickness", "style", "routing", "jump", "position", "dashed",
];

fn s(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or("")
}

/// A DSL string literal.
fn q(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out.replace("${", "${c4rizer_dollar}{")
}

/// True when `text` is all of `allowed` characters (and not empty).
fn plain(text: &str, allowed: impl Fn(char) -> bool) -> bool {
    !text.is_empty() && text.chars().all(allowed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Person,
    SoftwareSystem,
    Container,
    Component,
    Custom,
    DeploymentNode,
    InfrastructureNode,
    SoftwareSystemInstance,
    ContainerInstance,
}

impl Kind {
    fn default_tag(self) -> &'static str {
        match self {
            Kind::Person => "Person",
            Kind::SoftwareSystem => "Software System",
            Kind::Container => "Container",
            Kind::Component => "Component",
            Kind::Custom => "Element",
            Kind::DeploymentNode => "Deployment Node",
            Kind::InfrastructureNode => "Infrastructure Node",
            Kind::SoftwareSystemInstance => "Software System Instance",
            Kind::ContainerInstance => "Container Instance",
        }
    }
}

/// One element of the JSON.
struct El<'v> {
    kind: Kind,
    parent: Option<u64>,
    json: &'v Value,
}

/// What a block of the text stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Frame {
    Element(u64),
    Environment(String),
    /// A `group` block, by the group's full name.
    Group(String),
}

struct Writer<'v> {
    root: &'v Value,
    elements: BTreeMap<u64, El<'v>>,
    /// Every relationship by id: (source, destination, json).
    relationships: BTreeMap<u64, (u64, u64, &'v Value)>,
    keys: BTreeMap<u64, String>,
    out: String,
    stack: Vec<Frame>,
    /// Deployment group name -> its identifier, per environment.
    groups: BTreeMap<(String, String), String>,
    /// Instances written without an id (their key is implicit).
    anonymous: BTreeSet<u64>,
    /// The id each element is declared with (its last segment under
    /// `!identifiers hierarchical`); `keys` holds the full one.
    locals: BTreeMap<u64, String>,
    /// True when the JSON's identifiers are hierarchical (`a.b`).
    hierarchical: bool,
    /// The first thing found that makes the JSON unreadable.
    error: Option<String>,
    /// Every identifier, lowercased, the JSON gives or the text uses.
    taken: BTreeSet<String>,
}

/// A JSON vertex `{"x": .., "y": ..}` snapped to the 10 px grid (a half
/// toward +infinity, as import-layout snaps, P3-317); `None` unless both
/// are numbers within `i32` and the snapped point is too.
fn snapped_vertex(v: &Value) -> Option<(i32, i32)> {
    let snap = |n: i32| -> Option<i32> { i32::try_from((i64::from(n) * 2 + 10).div_euclid(20) * 10).ok() };
    let x = v.get("x").filter(|n| matches!(n, Value::Number(_))).and_then(Value::as_i32)?;
    let y = v.get("y").filter(|n| matches!(n, Value::Number(_))).and_then(Value::as_i32)?;
    Some((snap(x)?, snap(y)?))
}

fn num(v: Option<&Value>) -> Option<u64> {
    let t = s(v);
    if plain(t, |c| c.is_ascii_digit()) { t.parse().ok() } else { None }
}

/// A JSON value as an error message shows it.
fn show(v: Option<&Value>) -> String {
    match v {
        None => "none".into(),
        Some(Value::String(t)) => format!("\"{t}\""),
        Some(Value::Number(n)) => n.to_string(),
        Some(_) => "a value of another kind".into(),
    }
}

fn is_identifier(k: &str) -> bool {
    let mut chars = k.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl<'v> Writer<'v> {
    fn new(root: &'v Value) -> Writer<'v> {
        let mut w = Writer {
            root,
            elements: BTreeMap::new(),
            relationships: BTreeMap::new(),
            keys: BTreeMap::new(),
            out: String::new(),
            stack: Vec::new(),
            groups: BTreeMap::new(),
            anonymous: BTreeSet::new(),
            locals: BTreeMap::new(),
            hierarchical: false,
            error: None,
            taken: BTreeSet::new(),
        };
        if let Some(model) = root.get("model") {
            for (key, kind) in [
                ("people", Kind::Person),
                ("softwareSystems", Kind::SoftwareSystem),
                ("customElements", Kind::Custom),
                ("deploymentNodes", Kind::DeploymentNode),
            ] {
                for e in model.get(key).map(Value::items).unwrap_or_default() {
                    w.collect(e, kind, None);
                }
            }
        }
        let mut used = BTreeSet::new();
        let ids: Vec<u64> = w.elements.keys().copied().collect();
        let identifier = |el: &El<'_>| s(el.json.get("properties").and_then(|p| p.get("structurizr.dsl.identifier"))).to_owned();
        w.hierarchical = w.elements.values().any(|el| identifier(el).contains('.'));
        // Every identifier the JSON gives, so a made-up one never takes
        // it (`e7` written for element 3 and element 7 without one).
        let written_rels: Vec<String> = w
            .relationships
            .values()
            .map(|(_, _, r)| s(r.get("properties").and_then(|p| p.get("structurizr.dsl.identifier"))).to_ascii_lowercase())
            .collect();
        w.taken = w
            .elements
            .values()
            .flat_map(|el| {
                let id = identifier(el).to_ascii_lowercase();
                let last = id.rsplit('.').next().unwrap_or_default().to_owned();
                [id, last]
            })
            .chain(written_rels)
            .filter(|t| !t.is_empty())
            .collect();
        for &id in &ids {
            let el = &w.elements[&id];
            let written = identifier(el);
            let parent = el.parent.and_then(|p| w.keys.get(&p)).filter(|_| w.hierarchical).cloned();
            let local = match &parent {
                Some(pf) => written.strip_prefix(&format!("{pf}.")).unwrap_or(&written).to_owned(),
                None => written.clone(),
            };
            let instance = matches!(el.kind, Kind::ContainerInstance | Kind::SoftwareSystemInstance);
            let local = if is_identifier(&local) {
                local
            } else if instance {
                continue;
            } else {
                w.fresh(&format!("e{id}"))
            };
            let full = parent.map_or_else(|| local.clone(), |pf| format!("{pf}.{local}"));
            if !used.insert(full.to_ascii_lowercase()) {
                if instance {
                    continue;
                }
                let local = w.fresh(&format!("e{id}"));
                let el = &w.elements[&id];
                let full = el
                    .parent
                    .and_then(|p| w.keys.get(&p))
                    .filter(|_| w.hierarchical)
                    .map_or_else(|| local.clone(), |pf| format!("{pf}.{local}"));
                used.insert(full.to_ascii_lowercase());
                w.locals.insert(id, local);
                w.keys.insert(id, full);
                continue;
            }
            w.locals.insert(id, local);
            w.keys.insert(id, full);
        }
        w.instance_keys(&ids);
        w
    }

    /// The key of each instance the JSON gives no id: its implicit
    /// `<element id>_<n>` (P3-117) when free, else one of its own.
    fn instance_keys(&mut self, ids: &[u64]) {
        for &id in ids {
            if self.keys.contains_key(&id) {
                continue;
            }
            let el = &self.elements[&id];
            let of = num(el.json.get("containerId")).or_else(|| num(el.json.get("softwareSystemId")));
            let n = el.json.get("instanceId").and_then(Value::as_i32).unwrap_or(1);
            match of.and_then(|o| self.keys.get(&o)).map(|k| format!("{k}_{n}")) {
                // Its implicit id is free: the instance is written without one.
                Some(key) if !self.taken.contains(&key.to_ascii_lowercase()) => {
                    self.taken.insert(key.to_ascii_lowercase());
                    self.anonymous.insert(id);
                    self.keys.insert(id, key);
                }
                // Taken (a written identifier such as `zz_1`): an id of its own.
                // Under `!identifiers hierarchical` it is declared with its
                // last segment and found by its deployment node's key and
                // that segment, as the main loop keys elements.
                implicit => {
                    let base = implicit.map_or_else(
                        || format!("e{id}"),
                        |k| k.rsplit('.').next().unwrap_or(&k).to_owned(),
                    );
                    let local = self.fresh(&base);
                    let full = self.elements[&id]
                        .parent
                        .and_then(|p| self.keys.get(&p))
                        .filter(|_| self.hierarchical)
                        .map_or_else(|| local.clone(), |pf| format!("{pf}.{local}"));
                    self.taken.insert(full.to_ascii_lowercase());
                    self.locals.insert(id, local);
                    self.keys.insert(id, full);
                }
            }
        }
    }

    /// `base`, with `_` added until no identifier of the JSON or the
    /// text has it; then taken.
    fn fresh(&mut self, base: &str) -> String {
        let mut name = base.to_owned();
        while self.taken.contains(&name.to_ascii_lowercase()) {
            name.push('_');
        }
        self.taken.insert(name.to_ascii_lowercase());
        name
    }

    fn fail(&mut self, message: String) {
        if self.error.is_none() {
            self.error = Some(message);
        }
    }

    fn collect(&mut self, e: &'v Value, kind: Kind, parent: Option<u64>) {
        let Some(id) = num(e.get("id")) else {
            self.fail(format!("an element id must be a string of digits, found {}", show(e.get("id"))));
            return;
        };
        self.elements.insert(id, El { kind, parent, json: e });
        for r in e.get("relationships").map(Value::items).unwrap_or_default() {
            match (num(r.get("id")), num(r.get("sourceId")), num(r.get("destinationId"))) {
                (Some(rid), Some(src), Some(dst)) => {
                    self.relationships.insert(rid, (src, dst, r));
                }
                _ => self.fail(format!("a relationship id must be a string of digits, found {}", show(r.get("id")))),
            }
        }
        let children: &[(&str, Kind)] = match kind {
            Kind::SoftwareSystem => &[("containers", Kind::Container)],
            Kind::Container => &[("components", Kind::Component)],
            Kind::DeploymentNode => &[
                ("children", Kind::DeploymentNode),
                ("infrastructureNodes", Kind::InfrastructureNode),
                ("softwareSystemInstances", Kind::SoftwareSystemInstance),
                ("containerInstances", Kind::ContainerInstance),
            ],
            _ => &[],
        };
        for (key, k) in children {
            for c in e.get(key).map(Value::items).unwrap_or_default() {
                self.collect(c, *k, Some(id));
            }
        }
    }

    fn line(&mut self, text: &str) {
        for _ in 0..self.stack.len() + 2 {
            self.out.push_str("    ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn close(&mut self) {
        self.stack.pop();
        self.line("}");
    }

    fn key(&self, id: u64) -> String {
        self.keys.get(&id).cloned().unwrap_or_else(|| format!("e{id}"))
    }

    /// The written tags of `json` beyond the implicit ones.
    fn tags(json: &Value, implicit: &[&str]) -> String {
        s(json.get("tags"))
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty() && !implicit.contains(t))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn workspace(mut self) -> Result<String, JsonError> {
        let root = self.root;
        let mut head = String::from("workspace");
        let name = s(root.get("name"));
        let description = s(root.get("description"));
        if !name.is_empty() || !description.is_empty() {
            let _ = write!(head, " {}", q(name));
        }
        if !description.is_empty() {
            let _ = write!(head, " {}", q(description));
        }
        let mut out = format!("{head} {{\n");
        let props = Self::properties(root.get("properties"), &["structurizr.dsl", "structurizr.inspection."]);
        let scope = root.get("configuration").and_then(|c| c.get("scope")).and_then(Value::as_str);
        if let Some(scope) = scope.map(str::to_ascii_lowercase).filter(|s| ["landscape", "softwaresystem", "none"].contains(&s.as_str())) {
            let _ = writeln!(out, "    configuration {{\n        scope {scope}\n    }}");
        }
        out.push_str("    model {\n");
        if self.hierarchical {
            out.push_str("        !identifiers hierarchical\n");
        }
        let model_props = Self::properties(root.get("model").and_then(|m| m.get("properties")), &[]);
        if !model_props.is_empty() {
            out.push_str("        properties {\n");
            for l in model_props.lines() {
                let _ = writeln!(out, "            {}", l.trim());
            }
            out.push_str("        }\n");
        }
        self.model();
        out.push_str(&self.out);
        out.push_str("    }\n");
        self.out.clear();
        self.views();
        out.push_str(&self.out);
        if !props.is_empty() {
            out.push_str("    properties {\n");
            out.push_str(&props);
            out.push_str("    }\n");
        }
        out.push_str("}\n");
        if let Some(message) = self.error {
            return Err(JsonError {
                position: Position { line: 1, column: 1 },
                message,
            });
        }
        if out.contains("${c4rizer_dollar}") {
            out = format!("!const c4rizer_dollar \"$\"\n{out}");
        }
        Ok(out)
    }

    /// `"name" "value"` lines of a properties object, leaving out keys
    /// starting with one of `skip`, at three levels of indentation.
    fn properties(v: Option<&Value>, skip: &[&str]) -> String {
        let mut out = String::new();
        if let Some(Value::Object(members)) = v {
            for (k, v) in members {
                if skip.iter().any(|p| k.starts_with(p)) || k == "structurizr.dsl.identifier" {
                    continue;
                }
                if let Some(v) = v.as_str() {
                    let _ = writeln!(out, "        {} {}", q(k), q(v));
                }
            }
        }
        out
    }

    fn model(&mut self) {
        let mut items: Vec<(u64, bool)> = self.elements.keys().map(|&k| (k, true)).collect();
        for (&rid, &(_, _, r)) in &self.relationships {
            let linked = r.get("linkedRelationshipId").is_some();
            if !linked {
                items.push((rid, false));
            }
        }
        items.sort_unstable();
        for (id, element) in items {
            if element {
                self.element(id);
            } else {
                self.relationship(id);
            }
        }
        while !self.stack.is_empty() {
            self.close();
        }
    }

    /// The frame an element is written in.
    fn home(&self, id: u64) -> Option<Frame> {
        let el = &self.elements[&id];
        match el.parent {
            Some(p) => Some(Frame::Element(p)),
            None if el.kind == Kind::DeploymentNode => Some(Frame::Environment(s(el.json.get("environment")).to_owned())),
            None => None,
        }
    }

    /// Closes blocks until `frame` is the innermost (or none is open);
    /// false when `frame` is not open.
    fn unwind_to(&mut self, frame: Option<&Frame>) -> bool {
        match frame {
            None => {
                while !self.stack.is_empty() {
                    self.close();
                }
                true
            }
            Some(f) => {
                if !self.stack.contains(f) {
                    return false;
                }
                while self.stack.last() != Some(f) {
                    self.close();
                }
                true
            }
        }
    }

    fn open_environment(&mut self, env: &str) {
        self.unwind_to(None);
        self.line(&format!("deploymentEnvironment {} {{", q(env)));
        self.stack.push(Frame::Environment(env.to_owned()));
        // Its deployment groups, each once.
        let mut names: Vec<String> = Vec::new();
        for el in self.elements.values() {
            if s(el.json.get("environment")) == env {
                for g in el.json.get("deploymentGroups").map(Value::items).unwrap_or_default() {
                    if let Some(g) = g.as_str().filter(|g| !names.iter().any(|n| n == g)) {
                        names.push(g.to_owned());
                    }
                }
            }
        }
        for (i, g) in names.into_iter().enumerate() {
            let key = self.fresh(&format!("g{}_{i}", self.groups.len()));
            self.line(&format!("{key} = deploymentGroup {}", q(&g)));
            self.groups.insert((env.to_owned(), g), key);
        }
    }

    /// The `group` frames `id` is written in (outermost first).
    fn group_frames(&self, id: u64) -> Vec<Frame> {
        let group = s(self.elements[&id].json.get("group"));
        if group.is_empty() {
            return Vec::new();
        }
        let sep = s(self
            .root
            .get("model")
            .and_then(|m| m.get("properties"))
            .and_then(|p| p.get("structurizr.groupSeparator")));
        if sep.is_empty() {
            return vec![Frame::Group(group.to_owned())];
        }
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(i) = group[at..].find(sep) {
            out.push(Frame::Group(group[..at + i].to_owned()));
            at += i + sep.len();
        }
        out.push(Frame::Group(group.to_owned()));
        out
    }

    /// Opens the group frames `groups` from `from` on.
    fn open_groups(&mut self, groups: &[Frame], from: usize) {
        let sep = s(self
            .root
            .get("model")
            .and_then(|m| m.get("properties"))
            .and_then(|p| p.get("structurizr.groupSeparator")))
        .to_owned();
        for f in &groups[from..] {
            if let Frame::Group(full) = f {
                let name = if sep.is_empty() { full.as_str() } else { full.rsplit(sep.as_str()).next().unwrap_or(full) };
                self.line(&format!("group {} {{", q(name)));
                self.stack.push(f.clone());
            }
        }
    }

    fn element(&mut self, id: u64) {
        let home = self.home(id);
        let groups = self.group_frames(id);
        let base = match &home {
            Some(f) => self.stack.iter().rposition(|x| x == f).map(|p| p + 1),
            None => Some(0),
        };
        if let Some(b) = base {
            let mut m = 0;
            while b + m < self.stack.len() && m < groups.len() && self.stack[b + m] == groups[m] {
                m += 1;
            }
            while self.stack.len() > b + m {
                self.close();
            }
            self.open_groups(&groups, m);
        } else {
            match &home {
                Some(Frame::Environment(env)) => {
                    let env = env.clone();
                    self.open_environment(&env);
                }
                Some(Frame::Element(p)) => {
                    // The parent's block was closed: open it again.
                    let p = *p;
                    let parent_home = self.home(p);
                    if !self.unwind_to(parent_home.as_ref())
                        && let Some(Frame::Environment(env)) = &parent_home
                    {
                        let env = env.clone();
                        self.open_environment(&env);
                    }
                    let key = self.key(p);
                    self.line(&format!("!element {key} {{"));
                    self.stack.push(Frame::Element(p));
                }
                Some(Frame::Group(_)) | None => {}
            }
            self.open_groups(&groups, 0);
        }
        let el = &self.elements[&id];
        let json = el.json;
        let key = self.locals.get(&id).cloned().unwrap_or_else(|| self.key(id));
        let name = q(s(json.get("name")));
        let description = q(s(json.get("description")));
        let technology = q(s(json.get("technology")));
        let implicit = ["Element", el.kind.default_tag()];
        let tags = q(&Self::tags(json, &implicit));
        let head = match el.kind {
            Kind::Person => format!("{key} = person {name} {description} {tags}"),
            Kind::SoftwareSystem => format!("{key} = softwareSystem {name} {description} {tags}"),
            Kind::Container => format!("{key} = container {name} {description} {technology} {tags}"),
            Kind::Component => format!("{key} = component {name} {description} {technology} {tags}"),
            Kind::Custom => format!("{key} = element {name} {} {description} {tags}", q(s(json.get("metadata")))),
            Kind::DeploymentNode => format!("{key} = deploymentNode {name} {description} {technology} {tags}"),
            Kind::InfrastructureNode => format!("{key} = infrastructureNode {name} {description} {technology} {tags}"),
            Kind::SoftwareSystemInstance | Kind::ContainerInstance => {
                let of_key = if el.kind == Kind::ContainerInstance { "containerId" } else { "softwareSystemId" };
                let of = num(json.get(of_key)).map(|o| self.key(o)).unwrap_or_default();
                let env = s(json.get("environment")).to_owned();
                let groups: Vec<String> = json
                    .get("deploymentGroups")
                    .map(Value::items)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|g| self.groups.get(&(env.clone(), s(Some(g)).to_owned())).cloned())
                    .collect();
                let keyword = if el.kind == Kind::ContainerInstance { "containerInstance" } else { "softwareSystemInstance" };
                let implicit = [el.kind.default_tag()];
                let assign = if self.anonymous.contains(&id) { String::new() } else { format!("{key} = ") };
                format!("{assign}{keyword} {of} {} {}", q(&groups.join(",")), q(&Self::tags(json, &implicit)))
            }
        };
        self.line(&format!("{head} {{"));
        self.stack.push(Frame::Element(id));
        if let Some(n) = json
            .get("instances")
            .and_then(Value::as_str)
            .filter(|n| plain(n, |c| c.is_ascii_digit() || c == '.'))
        {
            self.line(&format!("instances {n}"));
        }
        if let Some(u) = json.get("url").and_then(Value::as_str) {
            self.line(&format!("url {}", q(u)));
        }
        let props = Self::properties(json.get("properties"), &[]);
        if !props.is_empty() {
            self.line("properties {");
            for l in props.lines() {
                self.line(&format!("    {}", l.trim()));
            }
            self.line("}");
        }
        for l in Self::perspectives(json) {
            self.line(&l);
        }
    }

    /// The `perspectives { "name" "description" ["value"] }` block of an
    /// element or relationship, empty when it has none.
    fn perspectives(json: &Value) -> Vec<String> {
        let list = json.get("perspectives").map(Value::items).unwrap_or_default();
        if list.is_empty() {
            return Vec::new();
        }
        let mut out = vec!["perspectives {".to_owned()];
        for p in list {
            let mut l = format!("    {} {}", q(s(p.get("name"))), q(s(p.get("description"))));
            if let Some(v) = p.get("value").and_then(Value::as_str) {
                let _ = write!(l, " {}", q(v));
            }
            out.push(l);
        }
        out.push("}".to_owned());
        out
    }

    fn relationship(&mut self, id: u64) {
        let (src, dst, json) = self.relationships[&id];
        // Relationships are written at the model's (or the
        // environment's) level.
        let env = self.stack.iter().find_map(|f| match f {
            Frame::Environment(e) => Some(Frame::Environment(e.clone())),
            _ => None,
        });
        self.unwind_to(env.as_ref());
        let mut line = String::new();
        if let Some(k) = json
            .get("properties")
            .and_then(|p| p.get("structurizr.dsl.identifier"))
            .and_then(Value::as_str)
            .filter(|k| is_identifier(k))
        {
            let _ = write!(line, "{k} = ");
        }
        let _ = write!(
            line,
            "{} -> {} {} {} {}",
            self.key(src),
            self.key(dst),
            q(s(json.get("description"))),
            q(s(json.get("technology"))),
            q(&Self::tags(json, &["Relationship"]))
        );
        let url = json.get("url").and_then(Value::as_str);
        let perspectives = Self::perspectives(json);
        if url.is_some() || !perspectives.is_empty() {
            line.push_str(" {");
            self.line(&line);
            if let Some(u) = url {
                self.line(&format!("    url {}", q(u)));
            }
            for l in perspectives {
                self.line(&format!("    {l}"));
            }
            self.line("}");
        } else {
            self.line(&line);
        }
    }

    // ------------------------------------------------------------ views

    fn views(&mut self) {
        let Some(views) = self.root.get("views") else { return };
        self.out.push_str("    views {\n");
        let mut all: Vec<(i64, &str, &Value)> = Vec::new();
        if let Value::Object(lists) = views {
            for (kind, list) in lists {
                for v in list.items() {
                    let order = v.get("order").and_then(Value::as_i32).map_or(i64::MAX, i64::from);
                    all.push((order, kind.as_str(), v));
                }
            }
        }
        all.sort_by_key(|(o, _, _)| *o);
        for (_, kind, v) in all {
            self.view(kind, v);
        }
        if let Some(config) = views.get("configuration") {
            self.styles(config.get("styles"));
            let themes: Vec<String> = config
                .get("themes")
                .map(Value::items)
                .unwrap_or_default()
                .iter()
                .filter_map(Value::as_str)
                .filter(|t| t.starts_with("https://"))
                .map(q)
                .collect();
            if !themes.is_empty() {
                let _ = writeln!(self.out, "        themes {}", themes.join(" "));
            }
            let mut terms = String::new();
            if let Some(Value::Object(members)) = config.get("terminology") {
                for (k, v) in members {
                    const TERMS: [&str; 7] = [
                        "person",
                        "softwareSystem",
                        "container",
                        "component",
                        "deploymentNode",
                        "infrastructureNode",
                        "relationship",
                    ];
                    if let Some(v) = v.as_str().filter(|_| TERMS.contains(&k.as_str())) {
                        let _ = writeln!(terms, "            {k} {}", q(v));
                    }
                }
            }
            if !terms.is_empty() {
                let _ = write!(self.out, "        terminology {{\n{terms}        }}\n");
            }
        }
        self.out.push_str("    }\n");
    }

    fn element_key(&self, v: Option<&Value>) -> Option<String> {
        num(v).map(|id| self.key(id))
    }

    fn view(&mut self, kind: &str, v: &Value) {
        let key = q(s(v.get("key")));
        let description = q(s(v.get("description")));
        let head = match kind {
            "systemLandscapeViews" => format!("systemLandscape {key} {description}"),
            "systemContextViews" => {
                let Some(e) = self.element_key(v.get("softwareSystemId")) else { return };
                format!("systemContext {e} {key} {description}")
            }
            "containerViews" => {
                let Some(e) = self.element_key(v.get("softwareSystemId")) else { return };
                format!("container {e} {key} {description}")
            }
            "componentViews" => {
                let Some(e) = self.element_key(v.get("containerId")) else { return };
                format!("component {e} {key} {description}")
            }
            "dynamicViews" => {
                let e = self.element_key(v.get("elementId")).unwrap_or_else(|| "*".into());
                format!("dynamic {e} {key} {description}")
            }
            "deploymentViews" => {
                let e = self.element_key(v.get("softwareSystemId")).unwrap_or_else(|| "*".into());
                format!("deployment {e} {} {key} {description}", q(s(v.get("environment"))))
            }
            "filteredViews" => {
                let mode = if s(v.get("mode")).eq_ignore_ascii_case("include") { "include" } else { "exclude" };
                let tags: Vec<&str> = v.get("tags").map(Value::items).unwrap_or_default().iter().filter_map(Value::as_str).collect();
                let _ = writeln!(
                    self.out,
                    "        filtered {} {mode} {} {key} {description}",
                    q(s(v.get("baseViewKey"))),
                    q(&tags.join(","))
                );
                return;
            }
            "customViews" => format!("custom {key} {} {description}", q(s(v.get("title")))),
            _ => return,
        };
        let _ = writeln!(self.out, "        {head} {{");
        let mut body = Vec::new();
        if kind == "dynamicViews" {
            for r in v.get("relationships").map(Value::items).unwrap_or_default() {
                body.push(self.step(r));
            }
        } else {
            let includes: Vec<String> = v
                .get("elements")
                .map(Value::items)
                .unwrap_or_default()
                .iter()
                .filter_map(|e| num(e.get("id")))
                .filter(|id| {
                    kind != "deploymentViews"
                        || self.elements.get(id).is_some_and(|e| e.kind != Kind::DeploymentNode)
                })
                .map(|id| self.key(id))
                .collect();
            if includes.is_empty() {
                body.push("include *".to_owned());
            } else {
                body.push(format!("include {}", includes.join(" ")));
            }
        }
        if let Some(t) = v.get("title").and_then(Value::as_str).filter(|_| kind != "customViews") {
            body.push(format!("title {}", q(t)));
        }
        if v.get("automaticLayout").is_some() {
            body.push("autoLayout".to_owned());
        }
        if let Some(p) = v
            .get("paperSize")
            .and_then(Value::as_str)
            .filter(|p| plain(p, |c| c.is_ascii_alphanumeric() || c == '_'))
        {
            body.push(format!("paperSize {p}"));
        }
        let default = self
            .root
            .get("views")
            .and_then(|vs| vs.get("configuration"))
            .and_then(|c| c.get("defaultView"))
            .and_then(Value::as_str);
        if default.is_some_and(|d| Some(d) == v.get("key").and_then(Value::as_str)) {
            body.push("default".to_owned());
        }
        for e in v.get("elements").map(Value::items).unwrap_or_default() {
            body.extend(self.layout(e));
        }
        body.extend(self.vias(v, kind == "dynamicViews"));
        let props = Self::properties(v.get("properties"), &[]);
        if !props.is_empty() {
            body.push("properties {".to_owned());
            body.extend(props.lines().map(|l| format!("    {}", l.trim())));
            body.push("}".to_owned());
        }
        for b in body {
            let _ = writeln!(self.out, "            {b}");
        }
        self.out.push_str("        }\n");
    }

    /// The `at` and `size` lines of one view element (module comment).
    fn layout(&mut self, view_element: &Value) -> Vec<String> {
        let Some(id) = num(view_element.get("id")) else { return Vec::new() };
        if self.elements.get(&id).is_some_and(|el| el.kind == Kind::DeploymentNode) {
            return Vec::new();
        }
        let x = view_element.get("x").and_then(Value::as_i32).unwrap_or(0);
        let y = view_element.get("y").and_then(Value::as_i32).unwrap_or(0);
        let size = match (view_element.get("width").and_then(Value::as_i32), view_element.get("height").and_then(Value::as_i32)) {
            (Some(width), Some(height)) => Some((width, height)),
            _ => None,
        };
        if size.is_none() && x == 0 && y == 0 {
            return Vec::new();
        }
        let (width, height) = size.unwrap_or((450, 300));
        let key = self.key(id);
        let (Some(cx), Some(cy)) = (x.checked_add(width / 2), y.checked_add(height / 2)) else {
            self.fail(format!("the position of element {id} is out of range"));
            return Vec::new();
        };
        let mut out = vec![format!("at {key} {cx} {cy}")];
        if size.is_some() {
            out.push(format!("size {key} {width} {height}"));
        }
        out
    }

    /// The `via` lines of one view: one per relationship view with
    /// vertices (module comment). In a dynamic view the step's number and
    /// drawn direction name the line, as `step` writes it.
    fn vias(&self, view: &Value, dynamic: bool) -> Vec<String> {
        let mut out = Vec::new();
        let mut named: BTreeSet<String> = BTreeSet::new();
        for r in view.get("relationships").map(Value::items).unwrap_or_default() {
            let Some(&(src, dst, _)) = num(r.get("id")).and_then(|id| self.relationships.get(&id)) else {
                continue;
            };
            let Some(points) = r.get("vertices").and_then(|v| match v {
                Value::Array(items) => items.iter().map(snapped_vertex).collect::<Option<Vec<(i32, i32)>>>(),
                _ => None,
            }) else {
                continue;
            };
            if points.is_empty() {
                continue;
            }
            let name = if dynamic {
                let order = s(r.get("order"));
                if !plain(order, |c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
                    continue;
                }
                let response = matches!(r.get("response"), Some(Value::Bool(true)));
                let (from, to) = if response { (dst, src) } else { (src, dst) };
                format!("{order}: {} -> {}", self.key(from), self.key(to))
            } else {
                format!("{} -> {}", self.key(src), self.key(dst))
            };
            // DECISION: the first relationship view of a line wins; a
            // second one for the same line would be a `DuplicateVia`.
            if !named.insert(name.clone()) {
                continue;
            }
            let mut line = format!("via {name}");
            for (x, y) in points {
                let _ = write!(line, " {x} {y}");
            }
            out.push(line);
        }
        out
    }

    /// One dynamic step: `<order>: <src> -> <dst> "<description>"`, the
    /// technology added when the first relationship between the two is
    /// another one.
    fn step(&self, r: &Value) -> String {
        let Some(&(src, dst, rel)) = num(r.get("id")).and_then(|id| self.relationships.get(&id)) else {
            return String::new();
        };
        let response = matches!(r.get("response"), Some(Value::Bool(true)));
        let (from, to) = if response { (dst, src) } else { (src, dst) };
        let order = s(r.get("order"));
        let mut line = String::new();
        if plain(order, |c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
            let _ = write!(line, "{order}: ");
        }
        let _ = write!(line, "{} -> {}", self.key(from), self.key(to));
        let description = r.get("description").and_then(Value::as_str).unwrap_or_else(|| s(rel.get("description")));
        let _ = write!(line, " {}", q(description));
        let first = self
            .relationships
            .values()
            .find(|(a, b, _)| *a == src && *b == dst)
            .map(|(_, _, j)| *j);
        let tech = s(rel.get("technology"));
        if first.is_some_and(|f| !std::ptr::eq(f, rel)) && !tech.is_empty() {
            let _ = write!(line, " {}", q(tech));
        }
        line
    }

    fn styles(&mut self, styles: Option<&Value>) {
        let Some(styles) = styles else { return };
        let mut by_scheme: BTreeMap<&str, String> = BTreeMap::new();
        for (list, keyword) in [("elements", "element"), ("relationships", "relationship")] {
            for style in styles.get(list).map(Value::items).unwrap_or_default() {
                let scheme = s(style.get("colorScheme"));
                let block = by_scheme.entry(match scheme {
                    "Light" => "light",
                    "Dark" => "dark",
                    _ => "",
                }).or_default();
                let _ = writeln!(block, "{keyword} {} {{", q(s(style.get("tag"))));
                if let Value::Object(members) = style {
                    for (field, value) in members {
                        let text = match value {
                            Value::String(t) if field == "icon" => q(t),
                            Value::String(t) if plain(t, |c| c.is_ascii_alphanumeric() || c == '#') => t.clone(),
                            Value::Number(x) => {
                                if x.fract() != 0.0 {
                                    self.fail(format!("the style value of `{field}` is not a whole number"));
                                    continue;
                                }
                                let Some(n) = value.as_i32() else {
                                    self.fail(format!("the style value of `{field}` is out of range"));
                                    continue;
                                };
                                n.to_string()
                            }
                            Value::Bool(b) => b.to_string(),
                            _ => continue,
                        };
                        if STYLE_KEYS.contains(&field.as_str()) {
                            let _ = writeln!(block, "    {field} {text}");
                        }
                    }
                }
                let props = Self::properties(style.get("properties"), &[]);
                if !props.is_empty() {
                    let _ = writeln!(block, "    properties {{");
                    for l in props.lines() {
                        let _ = writeln!(block, "        {}", l.trim());
                    }
                    let _ = writeln!(block, "    }}");
                }
                block.push_str("}\n");
            }
        }
        if by_scheme.is_empty() {
            return;
        }
        self.out.push_str("        styles {\n");
        for (scheme, block) in by_scheme {
            let indent = if scheme.is_empty() { "            " } else { "                " };
            if !scheme.is_empty() {
                let _ = writeln!(self.out, "            {scheme} {{");
            }
            for l in block.lines() {
                let _ = writeln!(self.out, "{indent}{l}");
            }
            if !scheme.is_empty() {
                self.out.push_str("            }\n");
            }
        }
        self.out.push_str("        }\n");
    }
}
