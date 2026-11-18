//! Model exploration without UI (SRS 4.7 P3-9, P3-500..P3-517): the
//! relationship rows with implied ones, search, "in no view", usages,
//! the graph layout and tree of a view, the kinds that offer them, and
//! decision records parsed from `!adrs` files. Pure: no I/O (the editor
//! reads the files and hands over their text).
//!
//! Depends on: `model`, `view`, `geom`, `inspect`.

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
    /// The view's title, if written.
    pub title: Option<String>,
    /// Directly or through lifting.
    pub drawn: Drawn,
}

/// Every declared view that draws `element`, in declaration order.
/// Serves P3-504.
pub fn usages(model: &Model, element: ElementId) -> Vec<Usage> {
    let _ = (model, element);
    todo!()
}

/// The centre of each box of `plan` (indexed by `BoxId`) in the graph
/// page's force layout, px: the same on every run (CORE-25). Serves
/// P3-508.
pub fn graph_layout(plan: &ViewPlan) -> Vec<Point> {
    let _ = plan;
    todo!()
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
pub fn tree(model: &Model, view: &ViewSpec) -> TreeNode {
    let _ = (model, view);
    todo!()
}

/// True for the kinds Structurizr offers a graph for: custom, landscape,
/// context, container and component views. Serves P3-511.
// APPROVER: new pub item (P3-511).
pub const fn graph_offered(kind: ViewKind) -> bool {
    let _ = kind;
    todo!()
}

/// True for the kinds Structurizr offers a tree for: the graph kinds
/// plus deployment views. Serves P3-511.
// APPROVER: new pub item (P3-511).
pub const fn tree_offered(kind: ViewKind) -> bool {
    let _ = kind;
    todo!()
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
pub fn parse_decision(file_name: &str, text: &str, importer: Option<&str>) -> Decision {
    let _ = (file_name, text, importer);
    todo!()
}
