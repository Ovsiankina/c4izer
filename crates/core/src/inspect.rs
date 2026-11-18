//! Structurizr's workspace inspections: model in, violations out, no I/O
//! (P2-615..P2-644).
//!
//! Depends on: `model`.

// Shape: one entry point, `inspect(&Model) -> Vec<Violation>`. The CLI's
// `inspect` command formats the violations (`report`), filters them by
// severity (P2-640) and turns the count into its exit code (P2-642).
// Nothing here places, routes or lints (P2-613).
//
// `Severity` is Structurizr's four-level inspection severity. It is a
// different type from `lint::Severity` (A.0 O12): callers name both by
// module path.
//
// The inspections follow Structurizr's `DefaultInspector`
// (structurizr-inspection 6.2.4, read from the installed CLI): one
// violation per failing subject, each with the severity its
// `structurizr.inspection.*` properties give (`PropertyBasedSeverityStrategy`,
// task 06). The workspace property `structurizr.inspection` set to `false`
// switches every inspection off, as there.
// DECISION: order (CORE-25, P2-644): workspace-level violations first
// (`model.empty`, `views.empty`), then elements, relationships, views and
// element styles, each in declaration order, and one subject's
// violations in `Inspection` order. Structurizr sorts by its own walk;
// only the set is compared with it.
// DECISION: implied relationships are not built as model items here.
// `model.element.disconnected` counts them (P2-622, below), but the
// relationship inspections look at declared relationships only, so a
// missing technology is reported once, on the relationship written in
// the file (Structurizr also reports each implied copy; deferred.md,
// differences).
// TODO(srs): P2-631, P2-632: `workspace.scope`, `workspace.tooling`,
// `views.view.key`, `views.view.layout`, `documentation.embeddedview` and
// the deployment inspections are not built (no `Inspection` variant).

use std::collections::BTreeSet;

use crate::model::{
    AttachmentKind, Element, ElementId, ElementKind, ImpliedRelationships, Model, Property,
    RelationshipId, Selector, ViewKind, ViewSpec,
};

/// Structurizr inspection severity, lowest first. Distinct from
/// `lint::Severity` (A.0 O12). Serves P2-633..P2-640.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// `ignore`: reported, but meant to be filtered out.
    Ignore,
    /// `info`.
    Info,
    /// `warning`.
    Warning,
    /// `error`: the default of every inspection (P2-633).
    Error,
}

impl Severity {
    /// The lower-case keyword Structurizr's properties and `-severity`
    /// use: `ignore`, `info`, `warning`, `error`. Serves P2-612, P2-634,
    /// P2-640.
    pub const fn keyword(self) -> &'static str {
        match self {
            Severity::Ignore => "ignore",
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// One Structurizr inspection. Serves P2-615..P2-630.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Inspection {
    /// `model.empty` (P2-615).
    ModelEmpty,
    /// `model.person.description` (P2-616).
    PersonDescription,
    /// `model.softwaresystem.description` (P2-617).
    SoftwareSystemDescription,
    /// `model.softwaresystem.documentation` (P2-628).
    SoftwareSystemDocumentation,
    /// `model.softwaresystem.decisions` (P2-629).
    SoftwareSystemDecisions,
    /// `model.container.description` (P2-618).
    ContainerDescription,
    /// `model.container.technology` (P2-619).
    ContainerTechnology,
    /// `model.component.description` (P2-620).
    ComponentDescription,
    /// `model.component.technology` (P2-621).
    ComponentTechnology,
    /// `model.element.disconnected` (P2-622).
    ElementDisconnected,
    /// `model.element.noview` (P2-623).
    ElementNoView,
    /// `model.relationship.description` (P2-624).
    RelationshipDescription,
    /// `model.relationship.technology` (P2-625).
    RelationshipTechnology,
    /// `views.empty` (P2-626).
    ViewsEmpty,
    /// `views.view.empty` (P2-627).
    ViewEmpty,
    /// `views.styles.element.metadata` (P2-630).
    StyleMetadata,
    /// `model.deploymentnode.description` (P3-128).
    DeploymentNodeDescription,
    /// `model.deploymentnode.technology` (P3-128).
    DeploymentNodeTechnology,
    /// `model.deploymentnode.empty`: no child node, infrastructure node
    /// or instance (P3-128).
    DeploymentNodeEmpty,
    /// `model.infrastructurenode.description` (P3-128).
    InfrastructureNodeDescription,
    /// `model.infrastructurenode.technology` (P3-128).
    InfrastructureNodeTechnology,
}

impl Inspection {
    /// The inspection key exactly as Structurizr writes it, e.g.
    /// `model.element.disconnected`. Serves P2-612, P2-634.
    pub const fn key(self) -> &'static str {
        match self {
            Inspection::ModelEmpty => "model.empty",
            Inspection::PersonDescription => "model.person.description",
            Inspection::SoftwareSystemDescription => "model.softwaresystem.description",
            Inspection::SoftwareSystemDocumentation => "model.softwaresystem.documentation",
            Inspection::SoftwareSystemDecisions => "model.softwaresystem.decisions",
            Inspection::ContainerDescription => "model.container.description",
            Inspection::ContainerTechnology => "model.container.technology",
            Inspection::ComponentDescription => "model.component.description",
            Inspection::ComponentTechnology => "model.component.technology",
            Inspection::ElementDisconnected => "model.element.disconnected",
            Inspection::ElementNoView => "model.element.noview",
            Inspection::RelationshipDescription => "model.relationship.description",
            Inspection::RelationshipTechnology => "model.relationship.technology",
            Inspection::ViewsEmpty => "views.empty",
            Inspection::ViewEmpty => "views.view.empty",
            Inspection::StyleMetadata => "views.styles.element.metadata",
            Inspection::DeploymentNodeDescription => "model.deploymentnode.description",
            Inspection::DeploymentNodeTechnology => "model.deploymentnode.technology",
            Inspection::DeploymentNodeEmpty => "model.deploymentnode.empty",
            Inspection::InfrastructureNodeDescription => "model.infrastructurenode.description",
            Inspection::InfrastructureNodeTechnology => "model.infrastructurenode.technology",
        }
    }
}

/// What a violation is about. Serves P2-612.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Subject {
    /// The workspace as a whole (`model.empty`, `views.empty`).
    Workspace,
    /// One element.
    Element(ElementId),
    /// One relationship.
    Relationship(RelationshipId),
    /// One view, by key.
    View(String),
    /// One element style, by tag.
    ElementStyle(String),
}

/// One inspection violation. Serves P2-611, P2-612.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Violation {
    /// The inspection that failed.
    pub inspection: Inspection,
    /// Its severity: `Error` unless a `structurizr.inspection.*` property
    /// sets another (P2-633..P2-639).
    pub severity: Severity,
    /// What it is about.
    pub subject: Subject,
}

/// Runs every inspection on `model` and returns the violations in a
/// fixed order, so equal models give equal lists (P2-644, CORE-25).
/// Serves P2-615..P2-639, P2-644.
pub fn inspect(model: &Model) -> Vec<Violation> {
    // Structurizr's `DefaultInspector` runs nothing when the workspace
    // property `structurizr.inspection` is `false` (any case).
    if last_value(&model.workspace_properties, "structurizr.inspection")
        .is_some_and(|v| v.eq_ignore_ascii_case("false"))
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    let workspace = [model.workspace_properties.as_slice()];
    if model.elements.is_empty() {
        let holders = [model.model_properties.as_slice(), workspace[0]];
        out.push(violation(Inspection::ModelEmpty, Subject::Workspace, &holders, None));
    }
    if model.views.is_empty() {
        let holders = [model.views_properties.as_slice(), workspace[0]];
        out.push(violation(Inspection::ViewsEmpty, Subject::Workspace, &holders, None));
    }
    let connected = connected(model);
    let shown = shown(model);
    for element in &model.elements {
        let holders = element_holders(model, element);
        for inspection in element_failures(model, element, &connected, &shown) {
            out.push(violation(inspection, Subject::Element(element.id), &holders, None));
        }
    }
    for r in &model.relationships {
        // DECISION: a relationship copied between instances (P3-106) is
        // not inspected again; the relationship it copies is, with the
        // same description and technology.
        if r.copied_from.is_some() {
            continue;
        }
        let source = model.element(r.source.value);
        let mut holders = vec![r.properties.as_slice(), source.properties.as_slice()];
        if let Some(parent) = source.parent {
            holders.push(model.element(parent).properties.as_slice());
        }
        holders.extend([model.model_properties.as_slice(), workspace[0]]);
        let typed = (
            type_name(source.kind),
            type_name(model.element(r.destination.value).kind),
        );
        for (inspection, value) in [
            (Inspection::RelationshipDescription, &r.description),
            (Inspection::RelationshipTechnology, &r.technology),
        ] {
            if missing(value.as_ref().map(|l| l.value.as_str())) {
                out.push(violation(inspection, Subject::Relationship(r.id), &holders, Some(typed)));
            }
        }
    }
    for view in &model.views {
        if is_model_view(view.kind) && view_elements(model, view).is_empty() {
            let holders = [
                view.properties.as_slice(),
                model.views_properties.as_slice(),
                workspace[0],
            ];
            out.push(violation(Inspection::ViewEmpty, Subject::View(view.key.clone()), &holders, None));
        }
    }
    for style in &model.element_styles {
        // DECISION: styles a theme brought in are not inspected:
        // Structurizr fetches themes in the renderer, so its inspector
        // never sees them.
        if style.theme.is_some() {
            continue;
        }
        if style.metadata.as_ref().is_some_and(|m| !m.value) {
            let holders = [
                style.properties.as_slice(),
                model.views_properties.as_slice(),
                workspace[0],
            ];
            out.push(violation(
                Inspection::StyleMetadata,
                Subject::ElementStyle(style.tag.value.clone()),
                &holders,
                None,
            ));
        }
    }
    out
}

/// A violation of `inspection` on `subject`, with the severity the
/// property `holders` give (nearest first). `typed` is a relationship's
/// (source, destination) type names for the `relationship[<s>-><d>]`
/// keys (P2-639).
fn violation(
    inspection: Inspection,
    subject: Subject,
    holders: &[&[Property]],
    typed: Option<(&str, &str)>,
) -> Violation {
    Violation {
        inspection,
        severity: severity(inspection.key(), holders, typed),
        subject,
    }
}

/// The severity of `key` from the `structurizr.inspection.*` properties
/// of `holders`, nearest first; `Error` when none sets it (P2-633..P2-639).
/// A relationship (`typed` = its end types) first tries the
/// `model.relationship[<source>-><destination>].<rest>` key and its own
/// wildcard, then the plain key (P2-639).
// TODO(srs): P2-635, P2-638, which key wins. Reading taken (B cli-breadth
// 6 [I], checked against Structurizr 6.2.4 `PropertyBasedSeverityStrategy`
// and its CLI: the same): the nearest owner that sets any matching key
// wins; at one owner the exact key beats the longest wildcard prefix,
// which beats `*`. So a wildcard on the element beats an exact key on the
// workspace.
// DECISION: a value other than the four severities (any case) is
// skipped and the search goes on; Structurizr stops with an exception.
fn severity(key: &str, holders: &[&[Property]], typed: Option<(&str, &str)>) -> Severity {
    if let Some((source, destination)) = typed {
        let typed_key = key.replacen(
            ".relationship.",
            &format!(".relationship[{source}->{destination}]."),
            1,
        );
        let names = property_names(&typed_key);
        // Structurizr drops the two shortest names (`model.*`, `*`) of the
        // typed form; the plain key's list below has them.
        let keep = names.len().saturating_sub(2);
        if let Some(found) = lookup(&names[..keep], holders) {
            return found;
        }
    }
    lookup(&property_names(key), holders).unwrap_or(Severity::Error)
}

/// The property names that can set `key`, most specific first:
/// `structurizr.inspection.<key>`, then `structurizr.inspection.<prefix>.*`
/// for each shorter dotted prefix, then `structurizr.inspection.*`.
fn property_names(key: &str) -> Vec<String> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut names = vec![format!("structurizr.inspection.{key}")];
    for end in (1..parts.len()).rev() {
        names.push(format!("structurizr.inspection.{}.*", parts[..end].join(".")));
    }
    names.push("structurizr.inspection.*".to_owned());
    names
}

/// The first severity found trying each holder nearest first and, in
/// each holder, each name most specific first.
fn lookup(names: &[String], holders: &[&[Property]]) -> Option<Severity> {
    holders.iter().find_map(|h| {
        names
            .iter()
            .find_map(|name| last_value(h, name).and_then(parse_severity))
    })
}

/// A severity keyword in any case (Structurizr upper-cases the value).
fn parse_severity(value: &str) -> Option<Severity> {
    [Severity::Ignore, Severity::Info, Severity::Warning, Severity::Error]
        .into_iter()
        .find(|s| s.keyword().eq_ignore_ascii_case(value.trim()))
}

/// The value of the last property named `name` (a later `properties`
/// line overrides an earlier one, as Structurizr's property map does).
fn last_value<'a>(properties: &'a [Property], name: &str) -> Option<&'a str> {
    properties
        .iter()
        .rev()
        .find(|p| p.name.value == name)
        .map(|p| p.value.value.as_str())
}

/// The lower-case Java class name Structurizr's typed relationship keys
/// use for an element kind (`person`, `softwaresystem`, ...).
fn type_name(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Person => "person",
        ElementKind::SoftwareSystem => "softwaresystem",
        ElementKind::Container => "container",
        ElementKind::Component => "component",
        ElementKind::DeploymentNode => "deploymentnode",
        ElementKind::InfrastructureNode => "infrastructurenode",
        ElementKind::SoftwareSystemInstance => "softwaresysteminstance",
        ElementKind::ContainerInstance => "containerinstance",
        _ => "customelement",
    }
}

/// The property holders of an element's inspections, nearest first: the
/// element, its ancestors, the model, the workspace (P2-636, P2-637).
// DECISION: every ancestor counts. Structurizr stops at the grandparent,
// which is the whole chain for a component, the deepest C4 element.
fn element_holders<'m>(model: &'m Model, element: &'m Element) -> Vec<&'m [Property]> {
    let mut holders = vec![element.properties.as_slice()];
    let mut parent = element.parent;
    while let Some(p) = parent {
        let ancestor = model.element(p);
        holders.push(ancestor.properties.as_slice());
        parent = ancestor.parent;
    }
    holders.extend([model.model_properties.as_slice(), model.workspace_properties.as_slice()]);
    holders
}

/// True for a description or technology that is absent or blank
/// (Structurizr's `StringUtils.isNullOrEmpty` trims).
fn missing(value: Option<&str>) -> bool {
    value.is_none_or(|v| v.trim().is_empty())
}

/// The inspections `element` fails, in `Inspection` order.
fn element_failures(
    model: &Model,
    element: &Element,
    connected: &BTreeSet<ElementId>,
    shown: &BTreeSet<ElementId>,
) -> Vec<Inspection> {
    let description = missing(element.description.as_ref().map(|l| l.value.as_str()));
    let technology = missing(element.technology.as_ref().map(|l| l.value.as_str()));
    let mut out = Vec::new();
    match element.kind {
        ElementKind::Person => {
            if description {
                out.push(Inspection::PersonDescription);
            }
        }
        ElementKind::SoftwareSystem => {
            if description {
                out.push(Inspection::SoftwareSystemDescription);
            }
            let has_containers = model
                .children(element.id)
                .iter()
                .any(|c| model.element(*c).kind == ElementKind::Container);
            // DECISION: a `!docs` / `!adrs` directive counts as attached
            // documentation or decisions; phase 2 does not read the files
            // (P2-159), so an empty folder still counts.
            let has = |kind| element.docs.iter().any(|d| d.kind == kind);
            if has_containers && !has(AttachmentKind::Docs) {
                out.push(Inspection::SoftwareSystemDocumentation);
            }
            if has_containers && !has(AttachmentKind::Adrs) {
                out.push(Inspection::SoftwareSystemDecisions);
            }
        }
        ElementKind::Container => {
            if description {
                out.push(Inspection::ContainerDescription);
            }
            if technology {
                out.push(Inspection::ContainerTechnology);
            }
        }
        ElementKind::Component => {
            if description {
                out.push(Inspection::ComponentDescription);
            }
            if technology {
                out.push(Inspection::ComponentTechnology);
            }
        }
        // P3-128 (Structurizr 6.2.4, probed).
        ElementKind::DeploymentNode => {
            if description {
                out.push(Inspection::DeploymentNodeDescription);
            }
            if technology {
                out.push(Inspection::DeploymentNodeTechnology);
            }
            if model.children(element.id).is_empty() {
                out.push(Inspection::DeploymentNodeEmpty);
            }
        }
        ElementKind::InfrastructureNode => {
            if description {
                out.push(Inspection::InfrastructureNodeDescription);
            }
            if technology {
                out.push(Inspection::InfrastructureNodeTechnology);
            }
        }
        _ => {}
    }
    // Structurizr 6.2.4 (probed): `model.element.disconnected` is never
    // reported for a deployment element.
    let deployment = element.environment.is_some();
    if !deployment && !connected.contains(&element.id) {
        out.push(Inspection::ElementDisconnected);
    }
    if !shown.contains(&element.id) {
        out.push(Inspection::ElementNoView);
    }
    out
}

/// Every element that is the source or destination of a relationship,
/// declared or implied (P2-622). Structurizr's DSL implies, for each
/// relationship declared while implied relationships are on, one between
/// every ancestor-or-self of its source and every ancestor-or-self of its
/// destination, except between an element and itself or one of its own
/// ancestors.
// TODO(srs): P2-622, whether children's and implied relationships count.
// Reading taken (Structurizr 6.2.4 `DisconnectedElementInspection`, which
// differs from B cli-breadth 8 [I]): only the element's own relationships
// count, implied ones included; a descendant's relationship counts only
// through the relationships it implies.
fn connected(model: &Model) -> BTreeSet<ElementId> {
    let lineage = |e: ElementId| {
        let mut out = vec![e];
        let mut parent = model.element(e).parent;
        while let Some(p) = parent {
            out.push(p);
            parent = model.element(p).parent;
        }
        out
    };
    let mut set = BTreeSet::new();
    for r in &model.relationships {
        let (s, d) = (r.source.value, r.destination.value);
        set.extend([s, d]);
        if r.implied_relationships == ImpliedRelationships::False {
            continue;
        }
        let (sources, destinations) = (lineage(s), lineage(d));
        for &a in &sources {
            for &b in &destinations {
                if a != b && !destinations.contains(&a) && !sources.contains(&b) {
                    set.extend([a, b]);
                }
            }
        }
    }
    set
}

/// The views Structurizr inspects as model views: not filtered or image
/// views, which hold no elements of their own.
fn is_model_view(kind: ViewKind) -> bool {
    !matches!(kind, ViewKind::Filtered | ViewKind::Image)
}

/// Every element some declared model view includes (P2-623). Default
/// views the editor offers and has not written (EDIT-15) are not views of
/// the model, so they never count.
fn shown(model: &Model) -> BTreeSet<ElementId> {
    let mut set: BTreeSet<ElementId> = model
        .views
        .iter()
        .filter(|v| is_model_view(v.kind))
        .flat_map(|v| view_elements(model, v))
        .collect();
    // P3-113: a deployment view draws every node above a shown box
    // (Structurizr 6.2.4 reports only the empty node as not in a view).
    let boxes: Vec<ElementId> = set.iter().copied().filter(|&e| model.element(e).environment.is_some()).collect();
    for e in boxes {
        let mut parent = model.element(e).parent;
        while let Some(p) = parent {
            set.insert(p);
            parent = model.element(p).parent;
        }
    }
    set
}

/// The elements `view` includes: `Model::included`, plus what `include *`
/// selects in a landscape (every person and software system) or custom
/// view (every custom element), which `included` leaves to their row.
// TODO(srs): P2-623, P2-627 for dynamic views: only their explicit
// `include` lines count until they are modelled, so `include *` alone
// leaves them empty. A deployment view counts what its `include *`
// selects (P3-129).
fn view_elements(model: &Model, view: &ViewSpec) -> BTreeSet<ElementId> {
    let mut set: BTreeSet<ElementId> = model.included(view).into_iter().collect();
    let star = |list: &[crate::model::Spanned<Selector>]| {
        list.iter().any(|s| matches!(s.value, Selector::All))
    };
    let wanted = match view.kind {
        ViewKind::SystemLandscape => {
            |k: ElementKind| matches!(k, ElementKind::Person | ElementKind::SoftwareSystem)
        }
        ViewKind::Custom => |k: ElementKind| k == ElementKind::Custom,
        _ => return set,
    };
    if star(&view.include) && !star(&view.exclude) {
        let excluded: BTreeSet<ElementId> = view
            .exclude
            .iter()
            .filter_map(|s| match s.value {
                Selector::Element(id) => Some(id),
                _ => None,
            })
            .collect();
        set.extend(
            model
                .elements
                .iter()
                .filter(|e| wanted(e.kind) && !excluded.contains(&e.id))
                .map(|e| e.id),
        );
    }
    set
}

#[cfg(test)]
mod tests {
    use super::{Inspection, Severity, Subject, Violation, inspect};
    use crate::model::{Model, parse};

    fn model_of(src: &str) -> Model {
        match parse(src) {
            Ok(p) => p.model,
            Err(e) => panic!("{e}\n{src}"),
        }
    }

    /// The subject as the CLI names it: element and relationship ends by
    /// id, views by key, styles by tag, the workspace as `-`.
    fn name(model: &Model, subject: &Subject) -> String {
        let element = |id| model.full_key(id).unwrap_or_default();
        match subject {
            Subject::Workspace => "-".into(),
            Subject::Element(id) => element(*id),
            Subject::Relationship(id) => {
                let r = model.relationship(*id);
                format!("{} -> {}", element(r.source.value), element(r.destination.value))
            }
            Subject::View(key) => format!("view {key}"),
            Subject::ElementStyle(tag) => format!("style {tag}"),
        }
    }

    /// Every violation of `src` as `<severity> <key> <subject>`.
    fn lines(src: &str) -> Vec<String> {
        let model = model_of(src);
        inspect(&model)
            .iter()
            .map(|v| {
                format!(
                    "{} {} {}",
                    v.severity.keyword(),
                    v.inspection.key(),
                    name(&model, &v.subject)
                )
            })
            .collect()
    }

    /// The subjects of the violations of `key` in `src`.
    fn subjects(src: &str, key: &str) -> Vec<String> {
        let model = model_of(src);
        inspect(&model)
            .iter()
            .filter(|v| v.inspection.key() == key)
            .map(|v| name(&model, &v.subject))
            .collect()
    }

    /// The severity of the one violation of `key` on `subject`.
    fn severity(src: &str, key: &str, subject: &str) -> Severity {
        let model = model_of(src);
        let all = inspect(&model);
        let found: Vec<&Violation> = all
            .iter()
            .filter(|v| v.inspection.key() == key && name(&model, &v.subject) == subject)
            .collect();
        assert_eq!(found.len(), 1, "{key} {subject}: {:?}", lines(src));
        found[0].severity
    }

    /// `ws` with a workspace-level `properties` block.
    fn ws_props(props: &str, model: &str, views: &str) -> String {
        format!(
            "workspace {{\n  properties {{\n{props}\n  }}\n  model {{\n{model}\n  }}\n  views {{\n{views}\n  }}\n}}\n"
        )
    }

    /// A `properties` block setting `key` to `value`.
    fn props(key: &str, value: &str) -> String {
        format!("properties {{\n  \"structurizr.inspection.{key}\" {value}\n}}\n")
    }

    /// A workspace around `model` and `views` block bodies.
    fn ws(model: &str, views: &str) -> String {
        format!("workspace {{\n  model {{\n{model}\n  }}\n  views {{\n{views}\n  }}\n}}\n")
    }

    /// A shop where every element and relationship is described and on a
    /// view: no violation except those a test adds.
    const SHOP_MODEL: &str = r#"
    operator = person "Operator" "Runs the shop."
    shop = softwareSystem "Shop" "Sells goods." {
      !docs docs
      !adrs adrs
      web = container "Web" "Serves pages." "Rust" {
        pages = component "Pages" "Renders pages." "Rust"
        cart = component "Cart" "Holds the cart." "Rust"
      }
    }
    operator -> pages "Browses" "HTTPS"
    pages -> cart "Fills" "Calls"
"#;

    const SHOP_VIEWS: &str = r#"
    systemContext shop "ctx" {
      include *
    }
    component web "comp" {
      include *
    }
    container shop "cont" {
      include *
    }
"#;

    #[test]
    fn p2_615_model_empty() {
        let empty = lines("workspace {\n  model {\n  }\n}\n");
        let n = empty.iter().filter(|l| l.contains("model.empty")).count();
        assert_eq!(n, 1, "{empty:?}");
        assert!(empty.contains(&"error model.empty -".to_owned()), "{empty:?}");
        assert!(subjects(&ws(SHOP_MODEL, SHOP_VIEWS), "model.empty").is_empty());
    }

    #[test]
    fn p2_616_person_description() {
        let src = ws(&format!("{SHOP_MODEL}    clerk = person \"Clerk\"\n    clerk -> pages \"Uses\" \"HTTPS\""), SHOP_VIEWS);
        assert_eq!(subjects(&src, "model.person.description"), ["clerk"]);
        assert!(subjects(&ws(SHOP_MODEL, SHOP_VIEWS), "model.person.description").is_empty());
    }

    #[test]
    fn p2_617_softwaresystem_description() {
        let src = ws(&format!("{SHOP_MODEL}    bank = softwareSystem \"Bank\"\n    cart -> bank \"Charges\" \"HTTPS\""), SHOP_VIEWS);
        assert_eq!(subjects(&src, "model.softwaresystem.description"), ["bank"]);
        assert!(subjects(&ws(SHOP_MODEL, SHOP_VIEWS), "model.softwaresystem.description").is_empty());
    }

    #[test]
    fn p2_618_container_description() {
        let src = ws(&SHOP_MODEL.replace("\"Web\" \"Serves pages.\" \"Rust\"", "\"Web\" \"\" \"Rust\""), SHOP_VIEWS);
        assert_eq!(subjects(&src, "model.container.description"), ["web"]);
        assert!(subjects(&src, "model.container.technology").is_empty());
    }

    #[test]
    fn p2_619_container_technology() {
        let src = ws(&SHOP_MODEL.replace("\"Web\" \"Serves pages.\" \"Rust\"", "\"Web\" \"Serves pages.\""), SHOP_VIEWS);
        assert_eq!(subjects(&src, "model.container.technology"), ["web"]);
        assert!(subjects(&src, "model.container.description").is_empty());
    }

    #[test]
    fn p2_620_component_description() {
        let src = ws(&SHOP_MODEL.replace("\"Cart\" \"Holds the cart.\" \"Rust\"", "\"Cart\" \"  \" \"Rust\""), SHOP_VIEWS);
        // Blank counts as missing, as Structurizr's `isNullOrEmpty` trims.
        assert_eq!(subjects(&src, "model.component.description"), ["cart"]);
        assert!(subjects(&src, "model.component.technology").is_empty());
    }

    #[test]
    fn p2_621_component_technology() {
        let src = ws(&SHOP_MODEL.replace("\"Cart\" \"Holds the cart.\" \"Rust\"", "\"Cart\" \"Holds the cart.\""), SHOP_VIEWS);
        assert_eq!(subjects(&src, "model.component.technology"), ["cart"]);
        assert!(subjects(&ws(SHOP_MODEL, SHOP_VIEWS), "model.component.technology").is_empty());
    }

    #[test]
    fn p2_622_element_disconnected() {
        // `lonely` has no relationship. `shop` and `web` are connected
        // only through the implied relationships `operator -> pages`
        // creates (Structurizr counts every relationship of the model,
        // implied ones included).
        let src = ws(&format!("{SHOP_MODEL}    lonely = softwareSystem \"Lonely\" \"Stands alone.\""), SHOP_VIEWS);
        assert_eq!(subjects(&src, "model.element.disconnected"), ["lonely"]);
        // A relationship inside a system implies none for the system
        // itself (Structurizr: never between an element and its parent),
        // so the system is disconnected though its children are not.
        let inner = ws(
            r#"    shop = softwareSystem "Shop" "Sells goods." {
      web = container "Web" "Serves pages." "Rust"
      db = container "Db" "Stores." "SQL"
    }
    web -> db "Reads" "SQL""#,
            "",
        );
        assert_eq!(subjects(&inner, "model.element.disconnected"), ["shop"]);
        // Under `!impliedRelationships false` nothing is implied.
        let direct = ws(
&format!("    !impliedRelationships false\n{SHOP_MODEL}"),
            SHOP_VIEWS,
        );
        assert_eq!(subjects(&direct, "model.element.disconnected"), ["shop", "web"]);
    }

    #[test]
    fn p2_623_element_noview() {
        // No view shows `web` here: the context view shows `shop` and
        // `operator`, the component view `pages` and `cart`.
        let views = "    systemContext shop \"ctx\" {\n      include *\n    }\n    component web \"comp\" {\n      include pages cart\n    }\n";
        let src = ws(SHOP_MODEL, views);
        assert_eq!(subjects(&src, "model.element.noview"), ["web"]);
        assert!(subjects(&ws(SHOP_MODEL, SHOP_VIEWS), "model.element.noview").is_empty());
        // A landscape's `include *` shows every person and system.
        let landscape = ws(
            SHOP_MODEL,
            "    systemLandscape \"land\" {\n      include *\n    }\n    component web \"comp\" {\n      include pages cart\n    }\n",
        );
        assert_eq!(subjects(&landscape, "model.element.noview"), ["web"]);
    }

    #[test]
    fn p2_623_editor_default_view_does_not_count() {
        // With no views block the editor offers default views (EDIT-15);
        // they are not in the file, so every element is on no view.
        let src = format!("workspace {{\n  model {{\n{SHOP_MODEL}\n  }}\n}}\n");
        assert_eq!(
            subjects(&src, "model.element.noview"),
            ["operator", "shop", "web", "pages", "cart"]
        );
    }

    #[test]
    fn p2_624_relationship_description() {
        let src = ws(&SHOP_MODEL.replace("pages -> cart \"Fills\" \"Calls\"", "pages -> cart \"\" \"Calls\""), SHOP_VIEWS);
        assert_eq!(subjects(&src, "model.relationship.description"), ["pages -> cart"]);
        assert!(subjects(&src, "model.relationship.technology").is_empty());
    }

    #[test]
    fn p2_625_relationship_technology() {
        let src = ws(&SHOP_MODEL.replace("pages -> cart \"Fills\" \"Calls\"", "pages -> cart \"Fills\""), SHOP_VIEWS);
        assert_eq!(subjects(&src, "model.relationship.technology"), ["pages -> cart"]);
        assert!(subjects(&src, "model.relationship.description").is_empty());
    }

    #[test]
    fn p2_626_views_empty() {
        let src = format!("workspace {{\n  model {{\n{SHOP_MODEL}\n  }}\n}}\n");
        assert_eq!(subjects(&src, "views.empty"), ["-"]);
        let empty_block = ws(SHOP_MODEL, "");
        assert_eq!(subjects(&empty_block, "views.empty"), ["-"]);
        assert!(subjects(&ws(SHOP_MODEL, SHOP_VIEWS), "views.empty").is_empty());
    }

    #[test]
    fn p2_627_view_empty() {
        let views = format!("{SHOP_VIEWS}    container shop \"none\" {{\n      include web\n      exclude web\n    }}\n");
        let src = ws(SHOP_MODEL, &views);
        assert_eq!(subjects(&src, "views.view.empty"), ["view none"]);
        assert!(subjects(&ws(SHOP_MODEL, SHOP_VIEWS), "views.view.empty").is_empty());
    }

    #[test]
    fn p2_628_softwaresystem_documentation() {
        let without = ws(&SHOP_MODEL.replace("      !docs docs\n", ""), SHOP_VIEWS);
        assert_eq!(subjects(&without, "model.softwaresystem.documentation"), ["shop"]);
        assert!(subjects(&without, "model.softwaresystem.decisions").is_empty());
        // A system with no containers needs none.
        let bare = ws(
            "    shop = softwareSystem \"Shop\" \"Sells goods.\"",
            "    systemContext shop \"ctx\" {\n      include *\n    }\n",
        );
        assert!(subjects(&bare, "model.softwaresystem.documentation").is_empty());
    }

    #[test]
    fn p2_629_softwaresystem_decisions() {
        let without = ws(&SHOP_MODEL.replace("      !adrs adrs\n", ""), SHOP_VIEWS);
        assert_eq!(subjects(&without, "model.softwaresystem.decisions"), ["shop"]);
        assert!(subjects(&without, "model.softwaresystem.documentation").is_empty());
    }

    #[test]
    fn p2_630_style_metadata_false() {
        let views = format!(
            "{SHOP_VIEWS}    styles {{\n      element \"Component\" {{\n        metadata false\n      }}\n      element \"Person\" {{\n        metadata true\n      }}\n    }}\n"
        );
        let src = ws(SHOP_MODEL, &views);
        assert_eq!(subjects(&src, "views.styles.element.metadata"), ["style Component"]);
    }

    #[test]
    fn p2_633_default_severity_error() {
        let src = format!("workspace {{\n  model {{\n{SHOP_MODEL}    lonely = person \"Lonely\"\n  }}\n}}\n");
        let model = model_of(&src);
        let all = inspect(&model);
        assert!(all.len() >= 4, "{all:?}");
        assert!(all.iter().all(|v| v.severity == Severity::Error), "{all:?}");
        assert_eq!(
            [Severity::Ignore, Severity::Info, Severity::Warning, Severity::Error].map(Severity::keyword),
            ["ignore", "info", "warning", "error"]
        );
        assert!(Severity::Ignore < Severity::Info && Severity::Warning < Severity::Error);
    }

    #[test]
    fn p2_634_workspace_property_sets_severity() {
        let model = format!("{SHOP_MODEL}    clerk = person \"Clerk\"");
        for (value, want) in [
            ("warning", Severity::Warning),
            ("info", Severity::Info),
            ("ignore", Severity::Ignore),
            ("error", Severity::Error),
            // Structurizr upper-cases the value before reading it.
            ("Warning", Severity::Warning),
        ] {
            let src = ws_props(
                &format!("    \"structurizr.inspection.model.person.description\" {value}"),
                &model,
                SHOP_VIEWS,
            );
            assert_eq!(severity(&src, "model.person.description", "clerk"), want, "{value}");
            // Other keys keep the default.
            assert_eq!(severity(&src, "model.element.disconnected", "clerk"), Severity::Error);
        }
        // `structurizr.inspection false` switches every inspection off.
        let off = ws_props("    structurizr.inspection false", &model, SHOP_VIEWS);
        assert!(lines(&off).is_empty(), "{:?}", lines(&off));
    }

    #[test]
    fn p2_635_wildcards_cover_prefix() {
        let model = format!(
            "{}    clerk = person \"Clerk\"",
            SHOP_MODEL.replace("\"Cart\" \"Holds the cart.\" \"Rust\"", "\"Cart\"")
        );
        let views = format!("{SHOP_VIEWS}    container shop \"none\" {{\n      include web\n      exclude web\n    }}\n");
        let all = ws_props("    structurizr.inspection.* info", &model, &views);
        assert!(inspect(&model_of(&all)).iter().all(|v| v.severity == Severity::Info));
        let src = ws_props(
            "    structurizr.inspection.model.* warning\n    structurizr.inspection.model.component.* ignore",
            &model,
            &views,
        );
        assert_eq!(severity(&src, "model.person.description", "clerk"), Severity::Warning);
        assert_eq!(severity(&src, "model.component.description", "cart"), Severity::Ignore);
        assert_eq!(severity(&src, "model.component.technology", "cart"), Severity::Ignore);
        assert_eq!(severity(&src, "views.view.empty", "view none"), Severity::Error);
        // The nearest owner wins (B cli-breadth 6, as Structurizr): the
        // element's wildcard beats the workspace's exact key.
        let exact = ws_props(
            "    structurizr.inspection.model.person.description info",
            &model.replace(
                "clerk = person \"Clerk\"",
                "clerk = person \"Clerk\" {\n      properties {\n        structurizr.inspection.model.* warning\n      }\n    }",
            ),
            &views,
        );
        assert_eq!(severity(&exact, "model.person.description", "clerk"), Severity::Warning);
        assert_eq!(severity(&exact, "model.element.disconnected", "clerk"), Severity::Warning);
        // At one owner the exact key beats the wildcard.
        let one = ws_props(
            "    structurizr.inspection.model.* warning\n    structurizr.inspection.model.person.description info",
            &model,
            &views,
        );
        assert_eq!(severity(&one, "model.person.description", "clerk"), Severity::Info);
        assert_eq!(severity(&one, "model.element.disconnected", "clerk"), Severity::Warning);
    }

    #[test]
    fn p2_636_element_relationship_view_property() {
        let model = format!(
            "{}    clerk = person \"Clerk\" {{\n      {}    }}\n    temp = person \"Temp\"",
            SHOP_MODEL.replace(
                "pages -> cart \"Fills\" \"Calls\"",
                &format!("pages -> cart \"Fills\" {{\n      {}    }}\n    cart -> pages \"Empties\"", props("model.relationship.technology", "info"))
            ),
            props("model.person.description", "warning")
        );
        let views = format!(
            "{SHOP_VIEWS}    container shop \"none\" {{\n      include web\n      exclude web\n      {}    }}\n    container shop \"void\" {{\n      include web\n      exclude web\n    }}\n",
            props("views.view.empty", "ignore")
        );
        let src = ws(&model, &views);
        assert_eq!(severity(&src, "model.person.description", "clerk"), Severity::Warning);
        assert_eq!(severity(&src, "model.person.description", "temp"), Severity::Error);
        assert_eq!(severity(&src, "model.relationship.technology", "pages -> cart"), Severity::Info);
        assert_eq!(severity(&src, "model.relationship.technology", "cart -> pages"), Severity::Error);
        assert_eq!(severity(&src, "views.view.empty", "view none"), Severity::Ignore);
        assert_eq!(severity(&src, "views.view.empty", "view void"), Severity::Error);
    }

    #[test]
    fn p2_637_element_property_applies_to_descendants() {
        let model = SHOP_MODEL
            .replace("\"Cart\" \"Holds the cart.\" \"Rust\"", "\"Cart\"")
            .replace(
                "      web = container \"Web\" \"Serves pages.\" \"Rust\" {\n",
                &format!("      web = container \"Web\" \"Serves pages.\" \"Rust\" {{\n        {}", props("model.component.technology", "info")),
            );
        let src = ws(&model, SHOP_VIEWS);
        assert_eq!(severity(&src, "model.component.technology", "cart"), Severity::Info);
        assert_eq!(severity(&src, "model.component.description", "cart"), Severity::Error);
        // The system above the container covers it too; the nearer owner wins.
        let system = model.replace(
            "      !docs docs\n",
            &format!("      !docs docs\n      {}", props("model.component.technology", "warning")),
        );
        let src = ws(&system, SHOP_VIEWS);
        assert_eq!(severity(&src, "model.component.technology", "cart"), Severity::Info);
        let src = ws(&system.replace("\"structurizr.inspection.model.component.technology\" info", "other x"), SHOP_VIEWS);
        assert_eq!(severity(&src, "model.component.technology", "cart"), Severity::Warning);
    }

    #[test]
    fn p2_638_model_and_viewset_levels() {
        // Model-level properties set model inspections, view-set ones view
        // inspections; each is nearer than the workspace's.
        let src = "workspace {\n  properties {\n    structurizr.inspection.* info\n  }\n  model {\n    properties {\n      structurizr.inspection.model.* warning\n    }\n  }\n  views {\n    properties {\n      structurizr.inspection.views.* ignore\n    }\n  }\n}\n";
        assert_eq!(severity(src, "model.empty", "-"), Severity::Warning);
        assert_eq!(severity(src, "views.empty", "-"), Severity::Ignore);
        // A model property does not reach view inspections.
        let views = format!("{SHOP_VIEWS}    container shop \"none\" {{\n      include web\n      exclude web\n    }}\n");
        let src = format!(
            "workspace {{\n  model {{\n    properties {{\n      structurizr.inspection.* warning\n    }}\n{SHOP_MODEL}    clerk = person \"Clerk\"\n  }}\n  views {{\n{views}  }}\n}}\n"
        );
        assert_eq!(severity(&src, "model.person.description", "clerk"), Severity::Warning);
        assert_eq!(severity(&src, "views.view.empty", "view none"), Severity::Error);
        // The element's own property beats the model's for the same key.
        let near = src.replace(
            "clerk = person \"Clerk\"",
            &format!("clerk = person \"Clerk\" {{\n      {}    }}", props("*", "info")),
        );
        assert_eq!(severity(&near, "model.person.description", "clerk"), Severity::Info);
    }

    #[test]
    fn p2_639_relationship_type_form() {
        let model = format!(
            "{}    pages -> cart \"Refills\"",
            SHOP_MODEL
                .replace("\"Browses\" \"HTTPS\"", "\"Browses\"")
                .replace("\"Fills\" \"Calls\"", "\"Fills\"")
        );
        let src = ws_props(
            "    \"structurizr.inspection.model.relationship[component->component].technology\" info\n    structurizr.inspection.model.relationship.technology warning",
            &model,
            SHOP_VIEWS,
        );
        let model_ = model_of(&src);
        let tech: Vec<(String, Severity)> = inspect(&model_)
            .iter()
            .filter(|v| v.inspection == Inspection::RelationshipTechnology)
            .map(|v| (name(&model_, &v.subject), v.severity))
            .collect();
        assert_eq!(
            tech,
            [
                ("operator -> pages".to_owned(), Severity::Warning),
                ("pages -> cart".to_owned(), Severity::Info),
                ("pages -> cart".to_owned(), Severity::Info),
            ]
        );
        // The typed wildcard covers every key of that pair.
        let star = ws_props(
            "    \"structurizr.inspection.model.relationship[person->component].*\" ignore",
            &model,
            SHOP_VIEWS,
        );
        assert_eq!(severity(&star, "model.relationship.technology", "operator -> pages"), Severity::Ignore);
    }

    #[test]
    fn p2_644_inspect_deterministic_order() {
        let src = format!(
            "workspace {{\n  model {{\n{}\n    lonely = person \"Lonely\"\n  }}\n  views {{\n    container shop \"none\" {{\n      include web\n      exclude web\n    }}\n    styles {{\n      element \"Person\" {{\n        metadata false\n      }}\n    }}\n  }}\n}}\n",
            SHOP_MODEL.replace("\"Fills\" \"Calls\"", "\"Fills\"")
        );
        let first = lines(&src);
        assert_eq!(lines(&src), first);
        assert_eq!(lines(&src), first);
        // Workspace first, then elements in declaration order (one
        // element's inspections in `Inspection` order), relationships,
        // views and styles.
        assert_eq!(
            first,
            [
                "error model.element.noview operator",
                "error model.element.noview shop",
                "error model.element.noview web",
                "error model.element.noview pages",
                "error model.element.noview cart",
                "error model.person.description lonely",
                "error model.element.disconnected lonely",
                "error model.element.noview lonely",
                "error model.relationship.technology pages -> cart",
                "error views.view.empty view none",
                "error views.styles.element.metadata style Person",
            ],
            "{first:#?}"
        );
    }

    #[test]
    fn inspect_keys_match_reference() {
        let all = [
            (Inspection::ModelEmpty, "model.empty"),
            (Inspection::PersonDescription, "model.person.description"),
            (Inspection::SoftwareSystemDescription, "model.softwaresystem.description"),
            (Inspection::SoftwareSystemDocumentation, "model.softwaresystem.documentation"),
            (Inspection::SoftwareSystemDecisions, "model.softwaresystem.decisions"),
            (Inspection::ContainerDescription, "model.container.description"),
            (Inspection::ContainerTechnology, "model.container.technology"),
            (Inspection::ComponentDescription, "model.component.description"),
            (Inspection::ComponentTechnology, "model.component.technology"),
            (Inspection::ElementDisconnected, "model.element.disconnected"),
            (Inspection::ElementNoView, "model.element.noview"),
            (Inspection::RelationshipDescription, "model.relationship.description"),
            (Inspection::RelationshipTechnology, "model.relationship.technology"),
            (Inspection::ViewsEmpty, "views.empty"),
            (Inspection::ViewEmpty, "views.view.empty"),
            (Inspection::StyleMetadata, "views.styles.element.metadata"),
        ];
        for (inspection, key) in all {
            assert_eq!(inspection.key(), key);
        }
    }

    #[test]
    fn inspect_example_violations_listed() {
        let src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/example/workspace.dsl"
        ));
        let got = lines(src);
        // Structurizr 6.2.4's `inspect` on the same file reports these 31
        // plus `workspace.scope` (not built, P2-631), all `ERROR`.
        let mut want = vec![
            "error model.softwaresystem.documentation system".to_owned(),
            "error model.softwaresystem.decisions system".to_owned(),
        ];
        want.extend(
            [
                "push -> worker",
                "push -> witness",
                "cli -> web",
                "cli -> sync",
                "cli -> devdata",
                "cli -> store",
                "cli -> auth",
                "cli -> notify",
                "web -> config",
                "web -> ledger",
                "web -> auth",
                "web -> store",
                "web -> notify",
                "web -> mail",
                "sync -> config",
                "sync -> vikunjaClient",
                "sync -> ledger",
                "sync -> store",
                "sync -> notify",
                "devdata -> ledger",
                "devdata -> auth",
                "devdata -> store",
                "vikunjaClient -> config",
                "notify -> config",
                "notify -> ledger",
                "notify -> auth",
                "mail -> config",
                "store -> ledger",
                "store -> auth",
            ]
            .map(|r| format!("error model.relationship.technology {r}")),
        );
        assert_eq!(got, want, "{got:#?}");
    }
}
