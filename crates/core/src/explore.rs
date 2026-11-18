//! Model exploration without UI (SRS 4.7 P3-9, P3-500..P3-517): the
//! relationship rows with implied ones, search, "in no view", usages,
//! the graph layout and tree of a view, the kinds that offer them, and
//! decision records parsed from `!adrs` files. Pure: no I/O (the editor
//! reads the files and hands over their text).
//!
//! Depends on: `model`, `view`, `geom`, `inspect`, `scene` (image sizes).

// DECISION: implied rows follow Structurizr's DSL default strategy
// (`CreateImpliedRelationshipsUnlessAnyRelationshipExistsStrategy`): for
// each relationship declared while implied relationships are on, one row
// per (ancestor-or-self of the source, ancestor-or-self of the
// destination) pair other than the relationship's own, skipping an
// element and itself or its own ancestor, a pair any model relationship
// already joins (in that direction, whatever its declaration order), and
// a pair an earlier relationship already implied. c4rizer keeps implied
// relationships out of the model (CORE-5 lifts at view time), so an
// implied row carries the id of the relationship it is implied by.
// DECISION: the id search and the tables show is the element's
// `Model::full_key`, else its index (`ElementId.0`); a relationship's
// key, else its index.

use std::collections::BTreeSet;

use crate::geom::Point;
use crate::model::{
    Element, ElementId, ImpliedRelationships, Model, Property, Relationship, RelationshipId, ViewKind, ViewSpec,
};
use crate::view::ViewPlan;

/// One row of the relationships table (P3-502) and of an element's
/// relationship list (P3-501): a relationship of the model, written,
/// copied (P3-106) or implied.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelationshipRow {
    /// The model relationship: the row's own for a written or copied
    /// one, the one it is implied by for an implied one.
    pub relationship: RelationshipId,
    /// Source element.
    pub source: ElementId,
    /// Destination element.
    pub destination: ElementId,
    /// True for an implied relationship (Structurizr's Model page
    /// "implied by").
    pub implied: bool,
}

/// Every relationship row of `model`: written and copied relationships
/// in model order, then the implied ones in the order their relationships
/// were declared. Serves P3-501, P3-502.
// APPROVER: new pub item (the sketch's tables need implied rows; core
// owns lifting, CORE-5).
pub fn relationship_rows(model: &Model) -> Vec<RelationshipRow> {
    let mut rows: Vec<RelationshipRow> = model
        .relationships
        .iter()
        .map(|r| RelationshipRow {
            relationship: r.id,
            source: r.source.value,
            destination: r.destination.value,
            implied: false,
        })
        .collect();
    let joined: BTreeSet<(ElementId, ElementId)> =
        model.relationships.iter().map(|r| (r.source.value, r.destination.value)).collect();
    let mut implied = BTreeSet::new();
    for r in &model.relationships {
        if r.implied_relationships == ImpliedRelationships::False {
            continue;
        }
        let (s, d) = (r.source.value, r.destination.value);
        let (sources, destinations) = (lineage(model, s), lineage(model, d));
        for &a in &sources {
            for &b in &destinations {
                if (a, b) == (s, d) || a == b || destinations.contains(&a) || sources.contains(&b) {
                    continue;
                }
                if joined.contains(&(a, b)) || !implied.insert((a, b)) {
                    continue;
                }
                rows.push(RelationshipRow {
                    relationship: r.id,
                    source: a,
                    destination: b,
                    implied: true,
                });
            }
        }
    }
    rows
}

/// `element`, then its parent, grandparent, ... up to the top.
fn lineage(model: &Model, element: ElementId) -> Vec<ElementId> {
    let mut out = vec![element];
    let mut parent = model.element(element).parent;
    while let Some(p) = parent {
        out.push(p);
        parent = model.element(p).parent;
    }
    out
}

/// The id the tables show and search matches for `element`: its full
/// key, else its index.
#[must_use]
fn element_id_text(model: &Model, element: &Element) -> String {
    model.full_key(element.id).unwrap_or_else(|| element.id.0.to_string())
}

/// The id the tables show and search matches for `relationship`: its
/// key, else its index.
fn relationship_id_text(relationship: &Relationship) -> String {
    relationship
        .key
        .as_ref()
        .map_or_else(|| relationship.id.0.to_string(), |k| k.value.clone())
}

/// True when `haystack` contains `needle` (already lower case), ignoring
/// case.
fn has(haystack: &str, needle: &str) -> bool {
    if haystack.is_ascii() && needle.is_ascii() {
        let (h, n) = (haystack.as_bytes(), needle.as_bytes());
        return n.is_empty() || h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n));
    }
    haystack.to_lowercase().contains(needle)
}

/// True when any property value contains `needle`.
fn any_property(properties: &[Property], needle: &str) -> bool {
    properties.iter().any(|p| has(&p.value.value, needle))
}

/// The elements and relationships a search matches (P3-503), each in
/// model order.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Hits {
    /// Matching elements.
    pub elements: Vec<ElementId>,
    /// Matching relationships (written or copied; an implied row matches
    /// when the relationship it is implied by does).
    pub relationships: Vec<RelationshipId>,
}

/// The elements and relationships whose name, id, description,
/// technology, tag or property value contains `text`, ignoring case.
/// An empty `text` matches everything. Serves P3-503.
pub fn search(model: &Model, text: &str) -> Hits {
    let needle = text.to_lowercase();
    let q = needle.as_str();
    let opt = |l: &Option<crate::model::Literal>| l.as_ref().is_some_and(|l| has(&l.value, q));
    let elements = model
        .elements
        .iter()
        .filter(|e| {
            q.is_empty()
                || has(&e.name.value, q)
                || has(&element_id_text(model, e), q)
                || opt(&e.description)
                || opt(&e.technology)
                || e.tags.iter().any(|t| has(t, q))
                || any_property(&e.properties, q)
        })
        .map(|e| e.id)
        .collect();
    let relationships = model
        .relationships
        .iter()
        .filter(|r| {
            q.is_empty()
                || has(&relationship_id_text(r), q)
                || opt(&r.description)
                || opt(&r.technology)
                || r.tags.iter().any(|t| has(t, q))
                || any_property(&r.properties, q)
        })
        .map(|r| r.id)
        .collect();
    Hits { elements, relationships }
}

/// The elements inspection `model.element.noview` reports, in model
/// order. Serves P3-505.
// APPROVER: new pub item (P3-505 needs the inspection's set without
// running every inspection).
pub fn in_no_view(model: &Model) -> Vec<ElementId> {
    let shown = crate::inspect::shown(model);
    model.elements.iter().map(|e| e.id).filter(|e| !shown.contains(e)).collect()
}

/// How a view draws an element (P3-504).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Drawn {
    /// As its own box.
    Directly,
    /// Not as a box: one of its relationships is drawn on a line of
    /// `onto`, its nearest drawn ancestor (CORE-5 lifting).
    Lifted {
        /// The drawn ancestor.
        onto: ElementId,
    },
}

/// One view that draws an element (P3-504).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Usage {
    /// The view's effective key.
    pub view: String,
    /// The view's title, else its description, if either is written.
    // DECISION: the description stands in for a missing `title`, as the
    // view list shows it (EDIT-8, P2-737).
    pub title: Option<String>,
    /// Directly or through lifting.
    pub drawn: Drawn,
}

/// Every declared view that draws `element`, in declaration order.
/// Serves P3-504.
// DECISION: only declared views count (as for "in no view"), each
// planned with `view::plan`, so a usage is exactly what the view draws;
// dynamic views count like the others since row dynamic-view merged.
// "Through lifting": the element has no box, and a drawn line
// carries a relationship whose end is the element or one of its
// descendants; `onto` is the element's nearest drawn ancestor.
pub fn usages(model: &Model, element: ElementId) -> Vec<Usage> {
    let mut out = Vec::new();
    for view in &model.views {
        if !view.kind.drawable() {
            continue;
        }
        let plan = crate::view::plan(model, view);
        let drawn = if plan.boxes_of(element).is_empty() {
            let onto = lineage(model, element)
                .into_iter()
                .skip(1)
                .find(|&a| !plan.boxes_of(a).is_empty());
            let carried = plan.edges.iter().flat_map(|e| &e.relationships).any(|&r| {
                let r = model.relationship(r);
                lineage(model, r.source.value).contains(&element)
                    || lineage(model, r.destination.value).contains(&element)
            });
            match onto {
                Some(onto) if carried => Drawn::Lifted { onto },
                _ => continue,
            }
        } else {
            Drawn::Directly
        };
        out.push(Usage {
            view: view.key.clone(),
            title: view.title.as_ref().or(view.description.as_ref()).map(|t| t.value.clone()),
            drawn,
        });
    }
    out
}

/// The centre of each box of `plan` (indexed by `BoxId`) in the graph
/// page's force layout, px: the same on every run (CORE-25). Serves
/// P3-508.
// DECISION: Fruchterman-Reingold in f64, a stand-in for graph.jsp's d3
// force simulation (link distance and a many-body charge): nodes start on
// a circle in name order (graph.jsp sorts nodes by name), 300 iterations
// with a linearly cooling step, repulsion k^2/d between every pair,
// attraction d^2/k along each drawn line (both ends distinct), and a weak
// pull to the centre so unlinked parts stay near. No randomness:
// coincident nodes are pushed apart along a direction fixed by their
// indices. Positions are rounded to px and centred on 0,0.
pub fn graph_layout(plan: &ViewPlan) -> Vec<Point> {
    const K: f64 = 180.0;
    const ROUNDS: u32 = 300;
    let n = plan.boxes.len();
    if n == 0 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| plan.boxes[a].name.cmp(&plan.boxes[b].name).then(a.cmp(&b)));
    #[expect(clippy::cast_precision_loss, reason = "node counts are small")]
    let nf = n as f64;
    let radius = K * nf / std::f64::consts::TAU + K;
    let mut pos = vec![(0.0_f64, 0.0_f64); n];
    for (rank, &i) in order.iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "node counts are small")]
        let angle = std::f64::consts::TAU * rank as f64 / nf;
        pos[i] = (radius * angle.cos(), radius * angle.sin());
    }
    let links: BTreeSet<(usize, usize)> = plan
        .edges
        .iter()
        .map(|e| (e.source.0 as usize, e.destination.0 as usize))
        .filter(|(a, b)| a != b)
        .map(|(a, b)| (a.min(b), a.max(b)))
        .collect();
    let start = radius.max(K);
    for round in 0..ROUNDS {
        let mut push = vec![(0.0_f64, 0.0_f64); n];
        for i in 0..n {
            for j in (i + 1)..n {
                let (mut dx, mut dy) = (pos[i].0 - pos[j].0, pos[i].1 - pos[j].1);
                let mut d = dx.hypot(dy);
                if d < 1e-6 {
                    #[expect(clippy::cast_precision_loss, reason = "node counts are small")]
                    let a = (i * 7 + j * 13) as f64;
                    (dx, dy, d) = (a.cos(), a.sin(), 1.0);
                }
                let f = K * K / d;
                let (fx, fy) = (dx / d * f, dy / d * f);
                push[i].0 += fx;
                push[i].1 += fy;
                push[j].0 -= fx;
                push[j].1 -= fy;
            }
        }
        for &(a, b) in &links {
            let (dx, dy) = (pos[a].0 - pos[b].0, pos[a].1 - pos[b].1);
            let d = dx.hypot(dy).max(1e-6);
            let f = d * d / K;
            let (fx, fy) = (dx / d * f, dy / d * f);
            push[a].0 -= fx;
            push[a].1 -= fy;
            push[b].0 += fx;
            push[b].1 += fy;
        }
        let heat = start * (1.0 - f64::from(round) / f64::from(ROUNDS)) + 1.0;
        for (p, f) in pos.iter_mut().zip(&push) {
            let (fx, fy) = (f.0 - p.0 * 0.05, f.1 - p.1 * 0.05);
            let len = fx.hypot(fy);
            if len > 1e-9 {
                let step = len.min(heat);
                p.0 += fx / len * step;
                p.1 += fy / len * step;
            }
        }
    }
    let (sx, sy) = pos.iter().fold((0.0, 0.0), |acc, p| (acc.0 + p.0, acc.1 + p.1));
    let (cx, cy) = (sx / nf, sy / nf);
    #[expect(clippy::cast_possible_truncation, reason = "layout coordinates are a few thousand px")]
    pos.iter()
        .map(|p| Point::new((p.0 - cx).round() as i32, (p.1 - cy).round() as i32))
        .collect()
}

/// One node of a view's tree (P3-510).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TreeNode {
    /// The element, `None` for the root (the view, or the environment of
    /// a deployment view).
    pub element: Option<ElementId>,
    /// The name shown.
    pub label: String,
    /// The children, by name.
    pub children: Vec<TreeNode>,
}

/// The tree of `view`: for a static view the scope hierarchy of its
/// elements (people, custom elements and systems under the root, then
/// containers, components); for a deployment view environment > node >
/// infrastructure node or instance. Siblings sorted by name (tree.jsp).
/// Serves P3-510.
// DECISION: the elements are what the view draws (`view::plan`), plus
// their ancestors, as tree.jsp filters by the view's elements and their
// parents. A filtered view shows its base view's tree (tree.jsp). Static
// trees put people and custom elements beside the systems at the top
// (SRS P3-510; tree.jsp shows systems only); every level sorted by name.
// Deployment trees: the environment at the root, its top nodes by name,
// then each node's child nodes, infrastructure nodes, system instances
// and container instances in model order (tree.jsp sorts only the top).
pub fn tree(model: &Model, view: &ViewSpec) -> TreeNode {
    let view = if view.kind == ViewKind::Filtered { model.base_view(view).unwrap_or(view) } else { view };
    let plan = crate::view::plan(model, view);
    let mut keep = BTreeSet::new();
    for b in &plan.boxes {
        keep.extend(lineage(model, b.element));
    }
    let deployment = view.kind == ViewKind::Deployment;
    let mut top: Vec<TreeNode> = model
        .elements
        .iter()
        .filter(|e| e.parent.is_none() && keep.contains(&e.id))
        .map(|e| if deployment { deployment_node(model, &keep, e.id) } else { static_node(model, &keep, e.id) })
        .collect();
    by_name(&mut top);
    let label = if deployment {
        let env = plan.boxes.iter().find_map(|b| model.element(b.element).environment);
        match (env, &view.scope) {
            (Some(env), _) => model.environments[env.0 as usize].name.value.clone(),
            (None, crate::model::ViewScope::Deployment { environment, .. }) => environment.value.clone(),
            (None, _) => view.key.clone(),
        }
    } else {
        view.key.clone()
    };
    TreeNode { element: None, label, children: top }
}

/// Sorts tree siblings by name, then by element.
fn by_name(nodes: &mut [TreeNode]) {
    nodes.sort_by(|a, b| a.label.cmp(&b.label).then(a.element.cmp(&b.element)));
}

/// The static tree under `element`: its kept children by name.
fn static_node(model: &Model, keep: &BTreeSet<ElementId>, element: ElementId) -> TreeNode {
    let mut children: Vec<TreeNode> = model
        .children(element)
        .into_iter()
        .filter(|c| keep.contains(c))
        .map(|c| static_node(model, keep, c))
        .collect();
    by_name(&mut children);
    TreeNode {
        element: Some(element),
        label: model.element(element).name.value.clone(),
        children,
    }
}

/// The deployment tree under `element`: child nodes, infrastructure
/// nodes, system instances, container instances, each in model order.
fn deployment_node(model: &Model, keep: &BTreeSet<ElementId>, element: ElementId) -> TreeNode {
    use crate::model::ElementKind as K;
    let mut children = Vec::new();
    for kind in [K::DeploymentNode, K::InfrastructureNode, K::SoftwareSystemInstance, K::ContainerInstance] {
        for c in model.children(element) {
            if keep.contains(&c) && model.element(c).kind == kind {
                children.push(deployment_node(model, keep, c));
            }
        }
    }
    TreeNode {
        element: Some(element),
        label: model.element(element).name.value.clone(),
        children,
    }
}

/// True for the kinds Structurizr offers a graph for: custom, landscape,
/// context, container and component views. Serves P3-511.
// APPROVER: new pub item (P3-511).
pub const fn graph_offered(kind: ViewKind) -> bool {
    matches!(
        kind,
        ViewKind::Custom | ViewKind::SystemLandscape | ViewKind::SystemContext | ViewKind::Container | ViewKind::Component
    )
}

/// True for the kinds Structurizr offers a tree for: the graph kinds
/// plus deployment views. Serves P3-511.
// APPROVER: new pub item (P3-511).
pub const fn tree_offered(kind: ViewKind) -> bool {
    graph_offered(kind) || matches!(kind, ViewKind::Deployment)
}

/// A link from one decision to another (P3-516), e.g. "Supersedes 2".
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DecisionLink {
    /// The words before the link, e.g. `Supersedes`, `Superseded by`.
    pub kind: String,
    /// The linked decision's id.
    pub target: String,
}

/// One architecture decision record from an `!adrs` folder (P3-516).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Decision {
    /// The id: the file name's leading number without leading zeros
    /// (adr-tools, MADR), the file stem otherwise.
    pub id: String,
    /// The title, without the `N. ` prefix.
    pub title: String,
    /// The date as written, if any.
    pub date: Option<String>,
    /// The status, `Proposed` when none is written.
    pub status: String,
    /// The links to other decisions, in file order.
    pub links: Vec<DecisionLink>,
}

/// Parses one decision file: `file_name` is its name in the folder,
/// `text` its content, `importer` the `!adrs` importer type if written
/// (`adrtools` by default, `madr`, `log4brains`). Serves P3-516.
// APPROVER: new pub item (the sketch's `Decision` is "parsed from `!adrs`
// content"; this is that parser).
// DECISION: one parser for the three importers Structurizr 6.2.4 ships
// (structurizr-import `AdrToolsDecisionImporter`, `MadrDecisionImporter`,
// `Log4brainsDecisionImporter`): YAML front matter `status:`/`date:`,
// list lines `- Status:`/`- Date:`, adr-tools `Date:` and the first word
// under `## Status` (`Superceded` read as `Superseded`), in that order of
// preference; the id is the file name's leading number without zeros
// (the file stem for log4brains, whose names start with a date); links
// are the `<words> [<title>](<file>)` lines of the status section.
pub fn parse_decision(file_name: &str, text: &str, importer: Option<&str>) -> Decision {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    let log4brains = importer.is_some_and(|i| i.eq_ignore_ascii_case("log4brains"));
    let id = if log4brains { stem.to_owned() } else { number_of(stem).unwrap_or_else(|| stem.to_owned()) };
    let text = text.replace("\r\n", "\n");
    let mut lines: Vec<&str> = text.lines().collect();
    let mut front: Vec<(String, String)> = Vec::new();
    if lines.first().is_some_and(|l| l.trim() == "---")
        && let Some(end) = lines.iter().skip(1).position(|l| l.trim() == "---")
    {
        for l in &lines[1..=end] {
            if let Some((k, v)) = l.split_once(':') {
                front.push((k.trim().to_ascii_lowercase(), v.trim().trim_matches('"').to_owned()));
            }
        }
        lines.drain(..=end + 1);
    }
    let field = |name: &str| front.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let listed = |name: &str| {
        lines.iter().find_map(|l| {
            let l = l.trim_start().strip_prefix("- ").or_else(|| l.trim_start().strip_prefix("* "))?;
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
    };
    let title = lines
        .iter()
        .find_map(|l| l.strip_prefix("# "))
        .map_or_else(|| stem.to_owned(), |t| strip_number(t.trim()).to_owned());
    let date = field("date")
        .or_else(|| listed("date"))
        .or_else(|| lines.iter().find_map(|l| l.strip_prefix("Date:").map(|d| d.trim().to_owned())));
    let status_at = lines.iter().position(|l| l.trim_end().eq_ignore_ascii_case("## status"));
    let section: &[&str] = status_at.map_or(&[], |at| {
        let rest = &lines[at + 1..];
        let end = rest.iter().position(|l| l.starts_with("## ")).unwrap_or(rest.len());
        &rest[..end]
    });
    let status = field("status")
        .or_else(|| listed("status"))
        .or_else(|| {
            let first = section.iter().find(|l| !l.trim().is_empty())?;
            first.split_whitespace().next().map(str::to_owned)
        })
        .map_or_else(
            || "Proposed".to_owned(),
            |s| if s.eq_ignore_ascii_case("superceded") { "Superseded".to_owned() } else { s },
        );
    Decision {
        id,
        title,
        date,
        status,
        links: section.iter().filter_map(|l| decision_link(l)).collect(),
    }
}

/// The leading number of `stem` without leading zeros (`0` for all
/// zeros), `None` when it starts with no digit.
fn number_of(stem: &str) -> Option<String> {
    let digits: String = stem.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let trimmed = digits.trim_start_matches('0');
    Some(if trimmed.is_empty() { "0".to_owned() } else { trimmed.to_owned() })
}

/// `title` without a leading `N. `.
fn strip_number(title: &str) -> &str {
    let digits = title.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && let Some(rest) = title[digits..].strip_prefix(". ")
    {
        return rest.trim_start();
    }
    title
}

/// A `<words> [<title>](<file>)` line linking to another decision file.
fn decision_link(line: &str) -> Option<DecisionLink> {
    let (kind, rest) = line.split_once('[')?;
    let kind = kind.trim();
    let (_, target) = rest.split_once("](")?;
    let target = target.split(')').next()?.trim();
    if kind.is_empty() || target.contains("://") {
        return None;
    }
    let name = target.rsplit('/').next().unwrap_or(target);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    Some(DecisionLink {
        kind: kind.to_owned(),
        target: number_of(stem).unwrap_or_else(|| stem.to_owned()),
    })
}

/// An image file a documentation section draws (P3-514): the file's
/// bytes as a `model::Asset` (for `export::rasterize_asset`) with its
/// declared width and height in px; `None` when the bytes are not a PNG,
/// JPEG or SVG image or declare no size.
// APPROVER: new pub item (core's `Asset` cannot be built outside core,
// and the editor needs the image's size to draw it at its own size).
#[must_use]
pub fn image_asset(path: &str, bytes: &[u8]) -> Option<(crate::model::Asset, i32, i32)> {
    let mime = crate::model::sniff_image(bytes)?;
    let (width, height) = crate::scene::intrinsic_size(mime, bytes)?;
    Some((
        crate::model::Asset {
            path: path.to_owned(),
            mime,
            bytes: bytes.into(),
        },
        width,
        height,
    ))
}
