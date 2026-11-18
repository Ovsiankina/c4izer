//! The explore pages (SRS 4.7 P3-9, P3-500..P3-517), Structurizr's
//! Explore equivalent: the model as a nested list, the relationships
//! table, search, usages, graph and tree of a view, the documentation and
//! decisions viewers. Read-only: nothing here writes the file (P3-507).
//!
//! Depends on: `state` (`kind_label`), `camera`, `canvas` (embedded views,
//! images, fonts), `worker` (`ViewRef`), `c4rizer-core` (`explore`,
//! `view`, `pipeline`), `eframe`; children `docs` (reads `!docs`/`!adrs`
//! files) and `markdown` (pulldown-cmark to blocks).

// Shape: `Explore` is the headless page state (page, search text, sort,
// selected element, graph focus, cameras) plus a cache of what it derives
// from one parse (rows, "in no view", docs, decisions, embedded view
// builds, images), rebuilt when the parse changes (`Arc::ptr_eq`). Its
// readers return plain rows the tests check; `show` draws them with egui
// and returns the actions clicks make (`Action::Open` opens a view with a
// box selected, `Action::Copy` copies text). The shell runs the actions.
// No action writes: the pages can only open views and copy text (P3-507).
//
// DECISION: the explore pages replace the canvas while the toolbar's
// "Explore" toggle is on (Structurizr's Explore is its own page); a usage
// click or a node double click turns it off and shows the view.
// DECISION: the model page indents nested elements instead of drawing
// boxes inside boxes (model.jsp nests `div`s); each row keeps the
// element's style colours as model.jsp does.
// DECISION: an expanded row lists outgoing relationships, then incoming
// ones (SRS "with their direction"; model.jsp lists outgoing only).
// DECISION: the search keeps a matching element's ancestors on the model
// page (greyed), so the nesting stays readable.
// DECISION: an implied row's tags show `Relationship` only, as the
// implied relationship Structurizr creates carries the default tag.
// DECISION: graph nodes are circles of graph.jsp's radii (20 px for
// people, systems and custom elements, 17 containers, 11 components, 14
// deployment elements) with the name and `[type]` to their right; tree
// nodes are 6 px circles with the label to the right, columns as wide as
// their longest label (tree.jsp spaces columns by window width).
// DECISION: clicking the focused graph node again clears the focus
// (graph.jsp `showConnectedNodes` with the same node).

pub mod docs;
pub mod markdown;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use c4rizer_core::explore::{self, Drawn, RelationshipRow, TreeNode, Usage};
use c4rizer_core::geom::{Point, Rect};
use c4rizer_core::model::{Asset, Colour, ElementId, ElementKind, Mode, Model, Parsed};
use c4rizer_core::pipeline::ViewBuild;
use c4rizer_core::place::Keep;
use c4rizer_core::text::Span;
use c4rizer_core::view::{BoxId, ViewPlan};
use eframe::egui;

use crate::camera::{Camera, ScenePoint, ScreenPoint};
use crate::state::{DrawList, kind_label};
use crate::worker::ViewRef;

/// One explore page (P3-9).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Page {
    /// The model as a nested list (P3-500, P3-501).
    #[default]
    Model,
    /// The relationships table (P3-502).
    Relationships,
    /// The graph of the view with this key (P3-508).
    Graph(String),
    /// The tree of the view with this key (P3-510).
    Tree(String),
    /// The documentation viewer (P3-513).
    Documentation,
    /// The decisions viewer (P3-516).
    Decisions,
}

/// A column of the relationships table (P3-502).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Column {
    /// The relationship's id.
    #[default]
    Id,
    /// Source name.
    Source,
    /// Destination name.
    Destination,
    /// Description.
    Description,
    /// Technology.
    Technology,
    /// Tags.
    Tags,
}

impl Column {
    /// Every column, in table order.
    pub const ALL: [Column; 6] = [
        Column::Id,
        Column::Source,
        Column::Destination,
        Column::Description,
        Column::Technology,
        Column::Tags,
    ];

    /// The header text.
    fn title(self) -> &'static str {
        match self {
            Column::Id => "Id",
            Column::Source => "Source",
            Column::Destination => "Destination",
            Column::Description => "Description",
            Column::Technology => "Technology",
            Column::Tags => "Tags",
        }
    }
}

/// Which section of the model page a row is in (P3-500).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Section {
    /// The static model.
    Static,
    /// The deployment model of the environment with this name.
    Deployment(String),
}

/// One row of the model page (P3-500).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRow {
    /// The element.
    pub element: ElementId,
    /// Nesting depth, 0 at the top of its section.
    pub depth: usize,
    /// The section.
    pub section: Section,
    /// The id shown (the element's key, else its index).
    pub id: String,
    /// The name.
    pub name: String,
    /// The type line, e.g. `[Container: Rust]`.
    pub type_line: String,
    /// The description, empty when none.
    pub description: String,
    /// Every tag.
    pub tags: Vec<String>,
    /// True when no view draws it (P3-505).
    pub in_no_view: bool,
    /// True when the search matched it (not only one of its children).
    pub matched: bool,
}

/// One relationship under an expanded element row (P3-501).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationshipLine {
    /// True when the element is the source.
    pub outgoing: bool,
    /// The other end.
    pub other: ElementId,
    /// Description, empty when none.
    pub description: String,
    /// Technology, empty when none.
    pub technology: String,
    /// True for an implied relationship.
    pub implied: bool,
}

/// One row of the relationships table (P3-502).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRow {
    /// The model row.
    pub row: RelationshipRow,
    /// The id shown: the relationship's key or index; an implied row
    /// shows the id of the relationship it is implied by.
    pub id: String,
    /// Source name.
    pub source: String,
    /// Destination name.
    pub destination: String,
    /// Description.
    pub description: String,
    /// Technology.
    pub technology: String,
    /// Tags, comma-separated.
    pub tags: String,
}

impl TableRow {
    /// The cell of `column`.
    fn cell(&self, column: Column) -> &str {
        match column {
            Column::Id => &self.id,
            Column::Source => &self.source,
            Column::Destination => &self.destination,
            Column::Description => &self.description,
            Column::Technology => &self.technology,
            Column::Tags => &self.tags,
        }
    }
}

/// What a click on an explore page asks the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Close Explore and open `view` with the box of `element` (its
    /// canonical key) selected (P3-504, P3-512).
    Open {
        /// The view.
        view: ViewRef,
        /// The element's canonical key.
        element: String,
    },
    /// Copy this text to the clipboard (P3-506).
    Copy(String),
}

/// The graph page's state (P3-508, P3-509).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphState {
    /// The view's key.
    pub key: String,
    /// The view's plan.
    pub plan: Arc<ViewPlan>,
    /// Node centres, px, by `BoxId`.
    pub positions: Vec<Point>,
    /// The clicked node, if any (P3-509).
    pub focus: Option<BoxId>,
    /// Pan and zoom (P3-512).
    pub camera: Camera,
    /// True once the camera was fitted to the canvas.
    pub fitted: bool,
    /// The canvas's top-left corner in window px, as last drawn.
    pub origin: egui::Pos2,
}

/// One laid-out node of the tree page (P3-510).
#[derive(Debug, Clone, PartialEq)]
pub struct TreeSpot {
    /// The element, `None` for the root.
    pub element: Option<ElementId>,
    /// The label.
    pub label: String,
    /// Centre, px: depth grows to the right.
    pub at: Point,
    /// The parent's index in the spot list.
    pub parent: Option<usize>,
}

/// The tree page's state (P3-510, P3-512).
#[derive(Debug, Clone, PartialEq)]
pub struct TreeState {
    /// The view's key.
    pub key: String,
    /// The laid-out nodes.
    pub spots: Vec<TreeSpot>,
    /// Pan and zoom.
    pub camera: Camera,
    /// True once the camera was fitted to the canvas.
    pub fitted: bool,
    /// The canvas's top-left corner in window px, as last drawn.
    pub origin: egui::Pos2,
}

/// What an embedded view reference resolves to (P3-517).
#[derive(Debug, Clone)]
pub enum Embed {
    /// The view's build, drawn at its fitted size.
    View(Arc<ViewBuild>),
    /// The one-line placeholder naming an unknown (or undrawable) key.
    Placeholder(String),
}

/// The explore pages' state.
#[derive(Debug, Default)]
pub struct Explore {
    /// True while the explore pages replace the canvas.
    pub open: bool,
    /// The page shown.
    pub page: Page,
    /// The search text (P3-503).
    pub query: String,
    /// The table's sort column and direction, descending when true (so
    /// the default is the id column, ascending).
    pub sort: (Column, bool),
    /// The selected element (P3-504).
    pub selected: Option<ElementId>,
    /// Expanded element rows (P3-501).
    pub expanded: BTreeSet<ElementId>,
    /// The graph page, once opened.
    pub graph: Option<GraphState>,
    /// The tree page, once opened.
    pub tree: Option<TreeState>,
    /// The selected documentation section (index).
    pub section: usize,
    /// The selected decision (index).
    pub decision: usize,
    /// Light or dark: the mode embedded views are built in.
    pub mode: Mode,
    /// What the pages derive from one parse.
    cache: Option<Derived>,
}

/// What the pages derive from one parse, rebuilt when it changes.
#[derive(Debug)]
struct Derived {
    /// The parse it was derived from.
    parsed: Arc<Parsed>,
    /// Every relationship row.
    rows: Vec<RelationshipRow>,
    /// The elements in no view.
    no_view: BTreeSet<ElementId>,
    /// The documentation sections.
    sections: Vec<docs::Section>,
    /// The decisions.
    decisions: Vec<docs::DecisionFile>,
    /// Embedded view builds by key and mode.
    embeds: BTreeMap<(String, bool), Option<Arc<ViewBuild>>>,
    /// Images read from docs folders, with their size.
    images: BTreeMap<PathBuf, Option<(Asset, i32, i32)>>,
}

/// The canonical key of `element`, empty when it has none.
fn key_of(model: &Model, element: ElementId) -> String {
    model.full_key(element).unwrap_or_default()
}

/// The id shown for `element`: its full key, else its index (as
/// `explore::search` matches it).
fn element_id_text(model: &Model, element: ElementId) -> String {
    model.full_key(element).unwrap_or_else(|| element.0.to_string())
}

/// The type line shown for `element`: `[Kind]` or `[Kind: technology]`;
/// a custom element shows its metadata (model.jsp metadata).
fn type_line(model: &Model, element: ElementId) -> String {
    let e = model.element(element);
    let kind = match (e.kind, &e.metadata) {
        (ElementKind::Custom, Some(m)) => m.value.clone(),
        (kind, _) => kind_label(kind, false).to_owned(),
    };
    match &e.technology {
        Some(t) if !t.value.is_empty() => format!("[{kind}: {}]", t.value),
        _ => format!("[{kind}]"),
    }
}

/// Graph node radius by kind, px (graph.jsp `nodeSizes`).
fn node_radius(kind: ElementKind) -> f32 {
    match kind {
        ElementKind::Container => 17.0,
        ElementKind::Component => 11.0,
        ElementKind::Person | ElementKind::SoftwareSystem | ElementKind::Custom => 20.0,
        _ => 14.0,
    }
}

impl Explore {
    /// Brings the derived data up to date with `parsed` (`Arc::ptr_eq`);
    /// a new parse clears the selection and re-opens the graph and tree
    /// of the same keys.
    pub fn sync(&mut self, parsed: &Arc<Parsed>) {
        if self.cache.as_ref().is_some_and(|c| Arc::ptr_eq(&c.parsed, parsed)) {
            return;
        }
        let model = &parsed.model;
        self.cache = Some(Derived {
            parsed: Arc::clone(parsed),
            rows: explore::relationship_rows(model),
            no_view: explore::in_no_view(model).into_iter().collect(),
            sections: docs::sections(parsed),
            decisions: docs::decisions(parsed),
            embeds: BTreeMap::new(),
            images: BTreeMap::new(),
        });
        self.selected = None;
        self.expanded.clear();
        if let Some(key) = self.graph.as_ref().map(|g| g.key.clone()) {
            self.graph = None;
            self.open_graph(model, &key);
        }
        if let Some(key) = self.tree.as_ref().map(|t| t.key.clone()) {
            self.tree = None;
            self.open_tree(model, &key);
        }
        let sections = self.sections().len();
        let decisions = self.decisions().len();
        self.section = self.section.min(sections.saturating_sub(1));
        self.decision = self.decision.min(decisions.saturating_sub(1));
    }

    /// The relationship rows of the last parse.
    fn rows(&self) -> &[RelationshipRow] {
        self.cache.as_ref().map_or(&[], |c| &c.rows)
    }

    /// True when no view draws `element` (P3-505).
    fn in_no_view(&self, element: ElementId) -> bool {
        self.cache.as_ref().is_some_and(|c| c.no_view.contains(&element))
    }

    /// The model page's rows: custom elements, people, systems with
    /// their containers and components, then one section per
    /// environment nested by node; only rows matching the search (and
    /// their ancestors) (P3-500, P3-503).
    #[must_use]
    pub fn model_rows(&self, model: &Model) -> Vec<ModelRow> {
        let mut order: Vec<(ElementId, usize, Section)> = Vec::new();
        for kind in [ElementKind::Custom, ElementKind::Person, ElementKind::SoftwareSystem] {
            for e in model.elements.iter().filter(|e| e.kind == kind && e.parent.is_none()) {
                order.push((e.id, 0, Section::Static));
                if kind == ElementKind::SoftwareSystem {
                    for c in model.children(e.id) {
                        order.push((c, 1, Section::Static));
                        for k in model.children(c) {
                            order.push((k, 2, Section::Static));
                        }
                    }
                }
            }
        }
        for env in &model.environments {
            let section = Section::Deployment(env.name.value.clone());
            for e in model
                .elements
                .iter()
                .filter(|e| e.environment == Some(env.id) && e.parent.is_none())
            {
                deployment_rows(model, e.id, 0, &section, &mut order);
            }
        }
        let hits: Option<BTreeSet<ElementId>> = (!self.query.trim().is_empty())
            .then(|| explore::search(model, self.query.trim()).elements.into_iter().collect());
        let keep: Option<BTreeSet<ElementId>> = hits.as_ref().map(|hits| {
            let mut keep = BTreeSet::new();
            for &h in hits {
                let mut at = Some(h);
                while let Some(e) = at {
                    keep.insert(e);
                    at = model.element(e).parent;
                }
            }
            keep
        });
        order
            .into_iter()
            .filter(|(e, _, _)| keep.as_ref().is_none_or(|k| k.contains(e)))
            .map(|(e, depth, section)| {
                let el = model.element(e);
                ModelRow {
                    element: e,
                    depth,
                    section,
                    id: element_id_text(model, e),
                    name: el.name.value.clone(),
                    type_line: type_line(model, e),
                    description: el.description.as_ref().map(|d| d.value.clone()).unwrap_or_default(),
                    tags: el.tags.clone(),
                    in_no_view: self.in_no_view(e),
                    matched: hits.as_ref().is_none_or(|h| h.contains(&e)),
                }
            })
            .collect()
    }

    /// The relationships under `element`'s expanded row: outgoing, then
    /// incoming, implied ones marked (P3-501).
    #[must_use]
    pub fn relationship_lines(&self, model: &Model, element: ElementId) -> Vec<RelationshipLine> {
        let line = |r: &RelationshipRow, outgoing: bool| {
            let rel = model.relationship(r.relationship);
            RelationshipLine {
                outgoing,
                other: if outgoing { r.destination } else { r.source },
                description: rel.description.as_ref().map(|d| d.value.clone()).unwrap_or_default(),
                technology: rel.technology.as_ref().map(|d| d.value.clone()).unwrap_or_default(),
                implied: r.implied,
            }
        };
        let rows = self.rows();
        rows.iter()
            .filter(|r| r.source == element)
            .map(|r| line(r, true))
            .chain(rows.iter().filter(|r| r.destination == element).map(|r| line(r, false)))
            .collect()
    }

    /// The relationships table: matching rows sorted by `sort` (P3-502,
    /// P3-503).
    #[must_use]
    pub fn table(&self, model: &Model) -> Vec<TableRow> {
        let hits: Option<BTreeSet<_>> = (!self.query.trim().is_empty())
            .then(|| explore::search(model, self.query.trim()).relationships.into_iter().collect());
        let mut rows: Vec<TableRow> = self
            .rows()
            .iter()
            .filter(|r| hits.as_ref().is_none_or(|h| h.contains(&r.relationship)))
            .map(|r| {
                let rel = model.relationship(r.relationship);
                let id = rel.key.as_ref().map_or_else(|| rel.id.0.to_string(), |k| k.value.clone());
                TableRow {
                    row: r.clone(),
                    id,
                    source: model.element(r.source).name.value.clone(),
                    destination: model.element(r.destination).name.value.clone(),
                    description: rel.description.as_ref().map(|d| d.value.clone()).unwrap_or_default(),
                    technology: rel.technology.as_ref().map(|d| d.value.clone()).unwrap_or_default(),
                    tags: if r.implied { "Relationship".to_owned() } else { rel.tags.join(", ") },
                }
            })
            .collect();
        let (column, descending) = self.sort;
        // A stable sort: equal cells keep model order.
        rows.sort_by(|a, b| {
            let o = if column == Column::Id {
                id_order(&a.id, &b.id)
            } else {
                a.cell(column).to_lowercase().cmp(&b.cell(column).to_lowercase())
            };
            if descending { o.reverse() } else { o }
        });
        rows
    }

    /// The selected element's usages (P3-504).
    #[must_use]
    pub fn usages(&self, model: &Model) -> Vec<Usage> {
        self.selected.map_or_else(Vec::new, |e| explore::usages(model, e))
    }

    /// The pages the explore menu offers for `model`: Model,
    /// Relationships, Documentation, Decisions, then Graph and Tree per
    /// view where offered (P3-9, P3-511).
    #[must_use]
    pub fn pages(model: &Model) -> Vec<Page> {
        let mut out = vec![Page::Model, Page::Relationships, Page::Documentation, Page::Decisions];
        for v in &model.views {
            if explore::graph_offered(v.kind) {
                out.push(Page::Graph(v.key.clone()));
            }
            if explore::tree_offered(v.kind) {
                out.push(Page::Tree(v.key.clone()));
            }
        }
        out
    }

    /// Opens the graph of `key`: plans the view and lays it out (P3-508).
    pub fn open_graph(&mut self, model: &Model, key: &str) {
        self.page = Page::Graph(key.to_owned());
        if self.graph.as_ref().is_some_and(|g| g.key == key) {
            return;
        }
        let Some(view) = model.view(key).filter(|v| explore::graph_offered(v.kind)) else {
            self.graph = None;
            return;
        };
        let plan = c4rizer_core::view::plan_in(model, view, self.mode);
        let positions = explore::graph_layout(&plan);
        self.graph = Some(GraphState {
            key: key.to_owned(),
            plan: Arc::new(plan),
            positions,
            focus: None,
            camera: Camera::default(),
            fitted: false,
            origin: egui::Pos2::ZERO,
        });
    }

    /// Opens the tree of `key` (P3-510).
    pub fn open_tree(&mut self, model: &Model, key: &str) {
        self.page = Page::Tree(key.to_owned());
        if self.tree.as_ref().is_some_and(|t| t.key == key) {
            return;
        }
        let spots = self.tree_spots(model, key);
        self.tree = (!spots.is_empty()).then(|| TreeState {
            key: key.to_owned(),
            spots,
            camera: Camera::default(),
            fitted: false,
            origin: egui::Pos2::ZERO,
        });
    }

    /// The graph node under `at` (screen px over the graph canvas).
    fn graph_node_at(&self, at: ScreenPoint) -> Option<BoxId> {
        let g = self.graph.as_ref()?;
        let scene = g.camera.to_scene(at);
        g.plan
            .boxes
            .iter()
            .zip(&g.positions)
            .rev()
            .find(|(b, p)| {
                #[expect(clippy::cast_precision_loss, reason = "layout px fit f32")]
                let (dx, dy) = (scene.x - p.x as f32, scene.y - p.y as f32);
                dx.hypot(dy) <= node_radius(b.kind) + 4.0
            })
            .map(|(b, _)| b.id)
    }

    /// A click at `at` (screen px over the graph): focuses the node
    /// under it, or clears the focus on empty space or on the focused
    /// node (P3-509).
    pub fn graph_click(&mut self, at: ScreenPoint) {
        let hit = self.graph_node_at(at);
        if let Some(g) = &mut self.graph {
            g.focus = if hit == g.focus { None } else { hit };
        }
    }

    /// The opacity of each node and edge of the graph, 1.0 or 0.1
    /// (P3-509).
    #[must_use]
    pub fn graph_opacity(&self) -> (Vec<f32>, Vec<f32>) {
        let Some(g) = &self.graph else { return (Vec::new(), Vec::new()) };
        let Some(focus) = g.focus else {
            return (vec![1.0; g.plan.boxes.len()], vec![1.0; g.plan.edges.len()]);
        };
        let mut near = BTreeSet::from([focus]);
        for e in &g.plan.edges {
            if e.source == focus {
                near.insert(e.destination);
            }
            if e.destination == focus {
                near.insert(e.source);
            }
        }
        let nodes = g.plan.boxes.iter().map(|b| if near.contains(&b.id) { 1.0 } else { 0.1 }).collect();
        let edges = g
            .plan
            .edges
            .iter()
            .map(|e| if e.source == focus || e.destination == focus { 1.0 } else { 0.1 })
            .collect();
        (nodes, edges)
    }

    /// The tree of `key`, laid out left to right (P3-510).
    #[must_use]
    #[expect(clippy::unused_self, reason = "a page reader like the others")]
    pub fn tree_spots(&self, model: &Model, key: &str) -> Vec<TreeSpot> {
        model
            .view(key)
            .filter(|v| explore::tree_offered(v.kind))
            .map_or_else(Vec::new, |v| lay_out_tree(&explore::tree(model, v)))
    }

    /// The documentation sections of the last parse (P3-513).
    #[must_use]
    pub fn sections(&self) -> &[docs::Section] {
        self.cache.as_ref().map_or(&[], |c| &c.sections)
    }

    /// The decisions of the last parse, sorted by id (P3-516).
    #[must_use]
    pub fn decisions(&self) -> &[docs::DecisionFile] {
        self.cache.as_ref().map_or(&[], |c| &c.decisions)
    }

    /// The index of the decision a link to `target` opens: the same
    /// owner's decision with that id, else any with that id (P3-516).
    #[must_use]
    pub fn linked_decision(&self, from: usize, target: &str) -> Option<usize> {
        let all = self.decisions();
        let owner = all.get(from).map(|d| d.owner);
        all.iter()
            .position(|d| Some(d.owner) == owner && d.decision.id == target)
            .or_else(|| all.iter().position(|d| d.decision.id == target))
    }

    /// What `![](embed:<key>)` draws (P3-517): the view built in the
    /// current mode, or a placeholder naming the key.
    pub fn embed(&mut self, model: &Model, key: &str) -> Embed {
        let placeholder = || Embed::Placeholder(format!("Embedded view `{key}` does not exist in this workspace"));
        let Some(view) = model.view(key) else { return placeholder() };
        if !view.kind.drawable() {
            return Embed::Placeholder(format!("Embedded view `{key}` is a {} view, which is not drawn yet", view.kind.keyword()));
        }
        let mode = self.mode;
        let Some(cache) = &mut self.cache else { return placeholder() };
        let build = cache.embeds.entry((key.to_owned(), mode == Mode::Dark)).or_insert_with(|| {
            // DECISION: built on the UI thread on first sight and kept
            // until the next parse; the background worker serves the
            // canvas and its cache keys views, not documents.
            c4rizer_core::pipeline::build_view_in(model, key, &Keep::default(), mode, &AtomicBool::new(false))
                .ok()
                .map(Arc::new)
        });
        build.clone().map_or_else(placeholder, Embed::View)
    }

    /// The image at `path`, read once per parse, with its size.
    fn image(&mut self, path: &Path) -> Option<(Asset, i32, i32)> {
        let cache = self.cache.as_mut()?;
        cache
            .images
            .entry(path.to_owned())
            .or_insert_with(|| {
                let bytes = std::fs::read(path).ok()?;
                explore::image_asset(&path.to_string_lossy(), &bytes)
            })
            .clone()
    }

    /// The parse the pages were derived from.
    fn parsed(&self) -> Option<Arc<Parsed>> {
        self.cache.as_ref().map(|c| Arc::clone(&c.parsed))
    }

    /// Draws the page and returns the actions its clicks make.
    pub fn show(&mut self, ui: &mut egui::Ui, model: &Model) -> Vec<Action> {
        let mut out = Vec::new();
        self.menu(ui, model);
        ui.separator();
        match self.page.clone() {
            Page::Model => self.model_page(ui, model, &mut out),
            Page::Relationships => self.relationships_page(ui, model, &mut out),
            Page::Graph(key) => {
                self.open_graph(model, &key);
                self.graph_page(ui, model, &mut out);
            }
            Page::Tree(key) => {
                self.open_tree(model, &key);
                self.tree_page(ui, model, &mut out);
            }
            Page::Documentation => self.documentation_page(ui, model),
            Page::Decisions => self.decisions_page(ui, model),
        }
        out
    }

    /// The page choice and the search field.
    fn menu(&mut self, ui: &mut egui::Ui, model: &Model) {
        ui.horizontal_wrapped(|ui| {
            for (page, label) in [
                (Page::Model, "Model"),
                (Page::Relationships, "Relationships"),
                (Page::Documentation, "Documentation"),
                (Page::Decisions, "Decisions"),
            ] {
                if ui.selectable_label(self.page == page, label).clicked() {
                    self.page = page;
                }
            }
            ui.separator();
            let pages = Explore::pages(model);
            for (graph, label) in [(true, "Graph"), (false, "Tree")] {
                let current = match &self.page {
                    Page::Graph(k) if graph => Some(k.clone()),
                    Page::Tree(k) if !graph => Some(k.clone()),
                    _ => None,
                };
                let text = current.as_ref().map_or_else(|| format!("{label}…"), |k| format!("{label}: {k}"));
                egui::ComboBox::from_id_salt(("c4rizer-explore-pick", graph))
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        for p in &pages {
                            let key = match p {
                                Page::Graph(k) if graph => k,
                                Page::Tree(k) if !graph => k,
                                _ => continue,
                            };
                            let title = model
                                .view(key)
                                .and_then(|v| v.title.as_ref().or(v.description.as_ref()))
                                .map_or_else(|| key.clone(), |t| format!("{key} · {}", t.value));
                            if ui.selectable_label(current.as_deref() == Some(key.as_str()), title).clicked() {
                                if graph {
                                    self.open_graph(model, key);
                                } else {
                                    self.open_tree(model, key);
                                }
                            }
                        }
                    });
            }
            if matches!(self.page, Page::Model | Page::Relationships) {
                ui.separator();
                ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search name, id, description, technology, tag, property")
                        .desired_width(320.0),
                );
                if !self.query.is_empty() && ui.small_button("✕").on_hover_text("Clear the search").clicked() {
                    self.query.clear();
                }
            }
        });
    }

    /// The model page: nested rows and, for the selected element, its
    /// usages (P3-500, P3-501, P3-504, P3-505, P3-506).
    fn model_page(&mut self, ui: &mut egui::Ui, model: &Model, out: &mut Vec<Action>) {
        let rows = self.model_rows(model);
        let width = ui.available_width();
        let usages_width = (width * 0.3).clamp(220.0, 420.0);
        egui::Panel::right("c4rizer-explore-usages")
            .resizable(true)
            .default_size(usages_width)
            .show(ui, |ui| self.usages_panel(ui, model, out));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let mut section: Option<Section> = None;
                if rows.is_empty() {
                    ui.weak(if self.query.is_empty() { "The model is empty." } else { "Nothing matches the search." });
                }
                for row in &rows {
                    if section.as_ref() != Some(&row.section) {
                        ui.add_space(6.0);
                        ui.heading(match &row.section {
                            Section::Static => "Static model".to_owned(),
                            Section::Deployment(env) => format!("Deployment model: {env}"),
                        });
                        section = Some(row.section.clone());
                    }
                    self.model_row(ui, model, row, out);
                }
            });
        });
    }

    /// One element row, with its relationships when expanded.
    fn model_row(&mut self, ui: &mut egui::Ui, model: &Model, row: &ModelRow, out: &mut Vec<Action>) {
        let style = model.style_of_in(row.element, self.mode);
        let fill = colour(style.background, if row.matched { 255 } else { 110 });
        let stroke = colour(style.stroke.unwrap_or_else(|| darker(style.background)), 255);
        let selected = self.selected == Some(row.element);
        #[expect(clippy::cast_precision_loss, reason = "nesting depth is tiny")]
        let indent = row.depth as f32 * 24.0;
        // The frame's content width: the column minus the indent, the
        // inner margin and the stroke, so no row runs under the usages
        // panel.
        let inner = (ui.available_width() - indent - 2.0 * 8.0 - 6.0).max(120.0);
        ui.horizontal(|ui| {
            ui.add_space(indent);
            let frame = egui::Frame::new()
                .fill(fill)
                .stroke(egui::Stroke::new(if selected { 3.0 } else { 1.5 }, stroke))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 5));
            // DECISION: no click sense on the whole frame: it would sit
            // on top of the row's buttons and take their clicks; the name
            // and "Usages" select the element.
            frame
                .show(ui, |ui| {
                    ui.set_width(inner);
                    // The frame sits in a horizontal row (the indent):
                    // its content is laid out top to bottom.
                    ui.vertical(|ui| self.row_content(ui, model, row, out));
                });
        });
        if self.expanded.contains(&row.element) {
            let lines = self.relationship_lines(model, row.element);
            ui.horizontal(|ui| {
                ui.add_space(indent + 24.0);
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.set_width((inner - 24.0).max(100.0));
                        ui.vertical(|ui| {
                            if lines.is_empty() {
                                ui.weak("No relationships.");
                            }
                            for l in &lines {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(if l.outgoing { "→" } else { "←" });
                                    let desc = if l.description.is_empty() { "—" } else { &l.description };
                                    ui.label(desc);
                                    if !l.technology.is_empty() {
                                        ui.weak(format!("[{}]", l.technology));
                                    }
                                    ui.label(if l.outgoing { "to" } else { "from" });
                                    let other = model.element(l.other);
                                    if ui.link(&other.name.value).clicked() {
                                        self.selected = Some(l.other);
                                    }
                                    ui.weak(type_line(model, l.other));
                                    if l.implied {
                                        badge(ui, "implied", egui::Color32::from_gray(120), egui::Color32::WHITE);
                                    }
                                });
                            }
                        });
                    });
            });
        }
        ui.add_space(3.0);
    }

    /// The lines of one element row: id, name, type, marks, tags and
    /// buttons, then the description.
    fn row_content(&mut self, ui: &mut egui::Ui, model: &Model, row: &ModelRow, out: &mut Vec<Action>) {
        let text = colour(model.style_of_in(row.element, self.mode).color, 255);
        ui.horizontal_wrapped(|ui| {
            let open = self.expanded.contains(&row.element);
            let arrow = if open { "▾" } else { "▸" };
            if ui
                .add(egui::Button::new(egui::RichText::new(arrow).color(text)).frame(false))
                .on_hover_text("Show the relationships")
                .clicked()
            {
                if open {
                    self.expanded.remove(&row.element);
                } else {
                    self.expanded.insert(row.element);
                }
            }
            badge(ui, &row.id, egui::Color32::from_gray(70), egui::Color32::WHITE);
            let name = egui::Button::new(egui::RichText::new(&row.name).color(text).family(bold()).size(14.0)).frame(false);
            if ui.add(name).on_hover_text("Show its usages").clicked() {
                self.selected = Some(row.element);
            }
            ui.label(egui::RichText::new(&row.type_line).color(text).size(11.0));
            if row.in_no_view {
                badge(ui, "in no view", egui::Color32::from_rgb(0xb0, 0x30, 0x30), egui::Color32::WHITE)
                    .on_hover_text("No view draws this element (inspection model.element.noview)");
            }
            ui.add_space(12.0);
            for tag in &row.tags {
                badge(ui, tag, egui::Color32::from_black_alpha(40), text);
            }
            ui.add_space(12.0);
            if ui.small_button("Usages").clicked() {
                self.selected = Some(row.element);
            }
            if ui.small_button("Reveal in file").on_hover_text("Copy path:line:column").clicked()
                && let Some(parsed) = self.parsed()
            {
                out.push(Action::Copy(reveal_text(&parsed, model.element(row.element).span)));
            }
        });
        if !row.description.is_empty() {
            ui.label(egui::RichText::new(&row.description).color(text));
        }
    }

    /// The usages of the selected element (P3-504).
    fn usages_panel(&mut self, ui: &mut egui::Ui, model: &Model, out: &mut Vec<Action>) {
        ui.heading("Usages");
        let Some(selected) = self.selected else {
            ui.weak("Select an element to see the views that draw it.");
            return;
        };
        let element = model.element(selected);
        ui.label(egui::RichText::new(&element.name.value).family(bold()));
        ui.weak(type_line(model, selected));
        ui.add_space(4.0);
        let usages = self.usages(model);
        if usages.is_empty() {
            ui.label("In no view.");
        }
        egui::ScrollArea::vertical().id_salt("c4rizer-explore-usages-list").show(ui, |ui| {
            for u in &usages {
                let (how, target) = match u.drawn {
                    Drawn::Directly => ("drawn directly".to_owned(), selected),
                    Drawn::Lifted { onto } => (format!("through {}", model.element(onto).name.value), onto),
                    _ => ("drawn".to_owned(), selected),
                };
                let text = match &u.title {
                    Some(t) => format!("{} · {t}", u.view),
                    None => u.view.clone(),
                };
                ui.horizontal_wrapped(|ui| {
                    if ui.link(text).on_hover_text("Open this view with the box selected").clicked() {
                        out.push(Action::Open {
                            view: ViewRef::Declared(u.view.clone()),
                            element: key_of(model, target),
                        });
                    }
                    ui.weak(how);
                });
            }
        });
    }

    /// The relationships table (P3-502, P3-503, P3-506).
    fn relationships_page(&mut self, ui: &mut egui::Ui, model: &Model, out: &mut Vec<Action>) {
        let rows = self.table(model);
        let width = ui.available_width() - 110.0;
        let widths = [0.08, 0.16, 0.16, 0.3, 0.14, 0.16].map(|f: f32| (width * f).max(40.0));
        ui.horizontal(|ui| {
            for (column, w) in Column::ALL.into_iter().zip(widths) {
                let mark = match self.sort {
                    (c, false) if c == column => " ▲",
                    (c, true) if c == column => " ▼",
                    _ => "",
                };
                let button = egui::Button::new(egui::RichText::new(format!("{}{mark}", column.title())).family(bold()));
                if ui.add_sized([w, 22.0], button).on_hover_text("Sort by this column").clicked() {
                    self.sort = if self.sort.0 == column { (column, !self.sort.1) } else { (column, false) };
                }
            }
        });
        ui.weak(format!("{} relationships", rows.len()));
        let row_height = 22.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                for row in &rows[range] {
                    ui.horizontal(|ui| {
                        for (column, w) in Column::ALL.into_iter().zip(widths) {
                            let mut text = row.cell(column).to_owned();
                            if column == Column::Id && row.row.implied {
                                text = format!("{text} (implied)");
                            }
                            let label = egui::Label::new(text.clone()).truncate();
                            ui.allocate_ui_with_layout(
                                egui::vec2(w, row_height - 4.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_width(w);
                                    ui.add(label).on_hover_text(text);
                                },
                            );
                        }
                        if ui.small_button("Reveal").on_hover_text("Copy path:line:column").clicked()
                            && let Some(parsed) = self.parsed()
                        {
                            out.push(Action::Copy(reveal_text(&parsed, model.relationship(row.row.relationship).span)));
                        }
                    });
                }
            });
    }

    /// The graph page (P3-508, P3-509, P3-512).
    #[expect(clippy::many_single_char_names, reason = "geometry: points, ends, radii")]
    fn graph_page(&mut self, ui: &mut egui::Ui, model: &Model, out: &mut Vec<Action>) {
        let Some(graph) = &self.graph else {
            ui.weak("No graph for this view: graphs are offered for custom, landscape, context, container and component views.");
            return;
        };
        let title = model
            .view(&graph.key)
            .and_then(|v| v.title.as_ref().or(v.description.as_ref()))
            .map_or_else(|| graph.key.clone(), |t| t.value.clone());
        ui.horizontal(|ui| {
            ui.strong(title);
            ui.weak("Click a node to show its neighbours · double-click opens the diagram · wheel zooms · middle drag or Space+drag pans");
        });
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let canvas = ScreenPoint { x: response.rect.width(), y: response.rect.height() };
        let rel = |p: egui::Pos2| ScreenPoint { x: p.x - response.rect.min.x, y: p.y - response.rect.min.y };
        if let Some(g) = &mut self.graph {
            g.origin = response.rect.min;
            if !g.fitted && canvas.x > 0.0 {
                let bounds = points_bounds(&g.positions, 60, 160);
                g.camera = fit_page(bounds, canvas);
                g.fitted = true;
            }
            pan_zoom(ui, &response, &mut g.camera, rel);
        }
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            self.graph_click(rel(p));
        }
        if response.double_clicked()
            && let Some(p) = response.interact_pointer_pos()
            && let Some(hit) = self.graph_node_at(rel(p))
            && let Some(g) = &self.graph
        {
            let element = g.plan.boxes[hit.0 as usize].element;
            out.push(Action::Open {
                view: ViewRef::Declared(g.key.clone()),
                element: key_of(model, element),
            });
        }
        let (node_alpha, edge_alpha) = self.graph_opacity();
        let Some(g) = &self.graph else { return };
        let origin = response.rect.min;
        let at = |p: Point| {
            let s = g.camera.to_screen(p);
            egui::pos2(origin.x + s.x, origin.y + s.y)
        };
        let painter = painter.with_clip_rect(response.rect);
        let zoom = g.camera.zoom;
        let fg = ui.visuals().text_color();
        for (i, e) in g.plan.edges.iter().enumerate() {
            let (s, d) = (e.source.0 as usize, e.destination.0 as usize);
            if s == d {
                continue;
            }
            let both = g.plan.edges.iter().any(|o| o.source == e.destination && o.destination == e.source);
            let (a, b) = (at(g.positions[s]), at(g.positions[d]));
            let dir = (b - a).normalized();
            let side = egui::vec2(-dir.y, dir.x) * if both { 4.0 * zoom } else { 0.0 };
            let ra = node_radius(g.plan.boxes[s].kind) * zoom;
            let rb = node_radius(g.plan.boxes[d].kind) * zoom;
            let (from, to) = (a + dir * ra + side, b - dir * (rb + 2.0) + side);
            let c = e.style.color.unwrap_or(Colour { r: 0x70, g: 0x70, b: 0x70 });
            let col = colour(c, alpha(edge_alpha[i]));
            painter.line_segment([from, to], egui::Stroke::new(1.5, col));
            let tip = 8.0 * zoom.max(0.5);
            let left = to - dir * tip + egui::vec2(-dir.y, dir.x) * tip * 0.45;
            let right = to - dir * tip - egui::vec2(-dir.y, dir.x) * tip * 0.45;
            painter.add(egui::Shape::convex_polygon(vec![to, left, right], col, egui::Stroke::NONE));
        }
        for (i, (b, &p)) in g.plan.boxes.iter().zip(&g.positions).enumerate() {
            let a = alpha(node_alpha[i]);
            let centre = at(p);
            let r = node_radius(b.kind) * zoom;
            let stroke = b.style.stroke.unwrap_or_else(|| darker(b.style.background));
            let width = if g.focus == Some(b.id) { 4.0 } else { 2.0 };
            painter.circle(centre, r, colour(b.style.background, a), egui::Stroke::new(width, colour(stroke, a)));
            let name_font = egui::FontId::new((13.0 * zoom).clamp(8.0, 22.0), egui::FontFamily::Name(crate::canvas::BOLD_FAMILY.into()));
            let meta_font = egui::FontId::proportional((10.5 * zoom).clamp(7.0, 18.0));
            let text = fg.gamma_multiply(node_alpha[i]);
            painter.text(centre + egui::vec2(r + 5.0, -1.0), egui::Align2::LEFT_BOTTOM, &b.name, name_font, text);
            let kind = format!("[{}]", kind_label(b.kind, false));
            painter.text(centre + egui::vec2(r + 5.0, 1.0), egui::Align2::LEFT_TOP, kind, meta_font, text);
        }
    }

    /// The tree page (P3-510, P3-512).
    fn tree_page(&mut self, ui: &mut egui::Ui, model: &Model, out: &mut Vec<Action>) {
        let Some(tree) = &self.tree else {
            ui.weak("No tree for this view: trees are offered for custom, landscape, context, container, component and deployment views.");
            return;
        };
        let title = model
            .view(&tree.key)
            .and_then(|v| v.title.as_ref().or(v.description.as_ref()))
            .map_or_else(|| tree.key.clone(), |t| t.value.clone());
        ui.horizontal(|ui| {
            ui.strong(title);
            ui.weak("Double-click a node to open the diagram · wheel zooms · middle drag or Space+drag pans");
        });
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let canvas = ScreenPoint { x: response.rect.width(), y: response.rect.height() };
        let rel = |p: egui::Pos2| ScreenPoint { x: p.x - response.rect.min.x, y: p.y - response.rect.min.y };
        let Some(t) = &mut self.tree else { return };
        t.origin = response.rect.min;
        if !t.fitted && canvas.x > 0.0 {
            let pts: Vec<Point> = t.spots.iter().map(|s| s.at).collect();
            let bounds = points_bounds(&pts, 40, label_room(&t.spots));
            t.camera = fit_page(bounds, canvas);
            t.fitted = true;
        }
        pan_zoom(ui, &response, &mut t.camera, rel);
        let cam = t.camera;
        let origin = response.rect.min;
        let at = |p: Point| {
            let s = cam.to_screen(p);
            egui::pos2(origin.x + s.x, origin.y + s.y)
        };
        if response.double_clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let hit = t.spots.iter().find(|s| {
                let c = at(s.at);
                let text_w = label_px(&s.label) * cam.zoom;
                let r = egui::Rect::from_min_max(c - egui::vec2(8.0, 9.0), c + egui::vec2(text_w + 14.0, 9.0));
                r.contains(p)
            });
            if let Some(element) = hit.and_then(|s| s.element) {
                out.push(Action::Open {
                    view: ViewRef::Declared(t.key.clone()),
                    element: key_of(model, element),
                });
            }
        }
        let painter = painter.with_clip_rect(response.rect);
        let link = ui.visuals().weak_text_color();
        for s in &t.spots {
            if let Some(parent) = s.parent {
                let (a, b) = (at(t.spots[parent].at), at(s.at));
                let mid = f32::midpoint(a.x, b.x);
                let curve = egui::epaint::CubicBezierShape::from_points_stroke(
                    [a, egui::pos2(mid, a.y), egui::pos2(mid, b.y), b],
                    false,
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::new(1.2, link),
                );
                painter.add(curve);
            }
        }
        let fg = ui.visuals().text_color();
        let bg = ui.visuals().panel_fill;
        let font = egui::FontId::proportional((13.0 * cam.zoom).clamp(8.0, 22.0));
        for s in &t.spots {
            let c = at(s.at);
            let (fill, stroke) = s.element.map_or((egui::Color32::from_gray(0x77), egui::Color32::BLACK), |e| {
                let st = model.style_of_in(e, self.mode);
                (colour(st.background, 255), colour(st.stroke.unwrap_or_else(|| darker(st.background)), 255))
            });
            painter.circle(c, 6.0 * cam.zoom.max(0.6), fill, egui::Stroke::new(1.5, stroke));
            let galley = painter.layout_no_wrap(s.label.clone(), font.clone(), fg);
            let pos = c + egui::vec2(10.0 * cam.zoom.max(0.6), -galley.size().y / 2.0);
            painter.rect_filled(egui::Rect::from_min_size(pos, galley.size()).expand(1.0), 2.0, bg.gamma_multiply(0.85));
            painter.galley(pos, galley, fg);
        }
    }

    /// The documentation viewer (P3-513..P3-515, P3-517).
    fn documentation_page(&mut self, ui: &mut egui::Ui, model: &Model) {
        let sections = self.sections().to_vec();
        if sections.is_empty() {
            ui.weak("No documentation: attach a folder with `!docs <path>` to the workspace or a software system.");
            return;
        }
        egui::Panel::left("c4rizer-explore-sections")
            .resizable(true)
            .default_size(240.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut owner: Option<Option<ElementId>> = None;
                    for (i, s) in sections.iter().enumerate() {
                        if owner != Some(s.owner) {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(owner_name(model, s.owner)).family(bold()));
                            owner = Some(s.owner);
                        }
                        if ui.selectable_label(self.section == i, format!("  {}", s.title)).clicked() {
                            self.section = i;
                        }
                    }
                });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(section) = sections.get(self.section) else { return };
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.weak(format!("{} · {}", owner_name(model, section.owner), section.path.display()));
                ui.add_space(4.0);
                let folder = section.path.parent().map(Path::to_path_buf).unwrap_or_default();
                match section.format {
                    docs::Format::Markdown => self.render_markdown(ui, model, &section.text, &folder),
                    docs::Format::AsciiDoc => asciidoc(ui, &section.text),
                }
            });
        });
    }

    /// The decisions viewer (P3-516).
    fn decisions_page(&mut self, ui: &mut egui::Ui, model: &Model) {
        let decisions = self.decisions().to_vec();
        if decisions.is_empty() {
            ui.weak("No decisions: attach a folder with `!adrs <path>` to the workspace or a software system.");
            return;
        }
        egui::Panel::left("c4rizer-explore-decisions")
            .resizable(true)
            .default_size(360.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut owner: Option<Option<ElementId>> = None;
                    egui::Grid::new("c4rizer-decision-list").striped(true).num_columns(4).show(ui, |ui| {
                        for (i, d) in decisions.iter().enumerate() {
                            if owner != Some(d.owner) {
                                ui.label(egui::RichText::new(owner_name(model, d.owner)).family(bold()));
                                ui.end_row();
                                ui.weak("Id");
                                ui.weak("Title");
                                ui.weak("Date");
                                ui.weak("Status");
                                ui.end_row();
                                owner = Some(d.owner);
                            }
                            let picked = self.decision == i;
                            if ui.selectable_label(picked, &d.decision.id).clicked() {
                                self.decision = i;
                            }
                            if ui.selectable_label(picked, &d.decision.title).clicked() {
                                self.decision = i;
                            }
                            ui.label(d.decision.date.as_deref().unwrap_or(""));
                            status_badge(ui, &d.decision.status);
                            ui.end_row();
                        }
                    });
                });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(d) = decisions.get(self.decision) else { return };
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.weak(format!("{} · decision {}", owner_name(model, d.owner), d.decision.id));
                    status_badge(ui, &d.decision.status);
                    if let Some(date) = &d.decision.date {
                        ui.weak(date);
                    }
                });
                if !d.decision.links.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for link in &d.decision.links {
                            let text = format!("{} {}", link.kind, link.target);
                            match self.linked_decision(self.decision, &link.target) {
                                Some(to) => {
                                    if ui.link(text).on_hover_text("Open the linked decision").clicked() {
                                        self.decision = to;
                                    }
                                }
                                None => {
                                    ui.weak(text);
                                }
                            }
                        }
                    });
                }
                ui.separator();
                let folder = d.path.parent().map(Path::to_path_buf).unwrap_or_default();
                self.render_markdown(ui, model, &d.text, &folder);
            });
        });
    }

    /// Draws Markdown blocks (P3-514, P3-517).
    fn render_markdown(&mut self, ui: &mut egui::Ui, model: &Model, text: &str, folder: &Path) {
        let blocks = markdown::blocks(text);
        let remote = count_remote(&blocks);
        if remote > 0 {
            ui.label(
                egui::RichText::new(format!(
                    "{remote} remote image{} not fetched: network is off for this file (P3-600).",
                    if remote == 1 { "" } else { "s" }
                ))
                .color(egui::Color32::from_rgb(0xb0, 0x70, 0x00)),
            );
        }
        for b in &blocks {
            self.block(ui, model, b, folder);
        }
    }

    /// Draws one block.
    fn block(&mut self, ui: &mut egui::Ui, model: &Model, block: &markdown::Block, folder: &Path) {
        use markdown::{Block, ImageSource};
        match block {
            Block::Heading { level, spans } => {
                let size = match level {
                    1 => 24.0,
                    2 => 20.0,
                    3 => 17.0,
                    _ => 15.0,
                };
                ui.add_space(6.0);
                ui.label(egui::RichText::new(markdown::plain(spans)).family(bold()).size(size));
                ui.add_space(2.0);
            }
            Block::Paragraph(spans) => {
                inline(ui, spans);
                ui.add_space(4.0);
            }
            Block::List { start, items } => {
                for (n, item) in items.iter().enumerate() {
                    let marker = start.map_or_else(|| "•".to_owned(), |s| format!("{}.", s + n as u64));
                    ui.horizontal(|ui| {
                        ui.add_sized([22.0, 16.0], egui::Label::new(marker));
                        ui.vertical(|ui| {
                            for b in item {
                                self.block(ui, model, b, folder);
                            }
                        });
                    });
                }
                ui.add_space(4.0);
            }
            Block::Code(code) => {
                egui::Frame::new()
                    .fill(ui.visuals().code_bg_color)
                    .corner_radius(4.0)
                    .inner_margin(8)
                    .show(ui, |ui| {
                        // The frame's 8px margins come on top.
                        ui.set_width((ui.available_width() - 16.0).max(40.0));
                        ui.label(egui::RichText::new(code).monospace());
                    });
                ui.add_space(4.0);
            }
            Block::Table { header, rows } => {
                let id = ui.next_auto_id();
                egui::Frame::new().stroke(ui.visuals().widgets.noninteractive.bg_stroke).inner_margin(4).show(ui, |ui| {
                    egui::Grid::new(id).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
                        for cell in header {
                            ui.label(egui::RichText::new(markdown::plain(cell)).family(bold()));
                        }
                        ui.end_row();
                        for row in rows {
                            for cell in row {
                                cell_text(ui, cell);
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.add_space(4.0);
            }
            Block::Image { alt, source } => {
                match source {
                    ImageSource::Embed(key) => match self.embed(model, key) {
                        Embed::View(build) => embedded_view(ui, &build),
                        Embed::Placeholder(text) => placeholder(ui, &text),
                    },
                    ImageSource::Relative(path) => {
                        let file = folder.join(path);
                        match self.image(&file) {
                            Some((asset, w, h)) => draw_image(ui, &asset, w, h),
                            None => placeholder(ui, &format!("Image `{path}` cannot be read")),
                        }
                    }
                    ImageSource::Remote(url) => match fetch_remote(url) {
                        Some(_) => {}
                        None => placeholder(ui, &format!("Remote image not fetched (network off): {url}")),
                    },
                }
                if !alt.is_empty() {
                    ui.weak(alt);
                }
                ui.add_space(4.0);
            }
            Block::Quote(blocks) => {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(3.0, 18.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 0.0, ui.visuals().weak_text_color());
                    ui.vertical(|ui| {
                        for b in blocks {
                            self.block(ui, model, b, folder);
                        }
                    });
                });
            }
            Block::Rule => {
                ui.separator();
            }
        }
    }
}

/// Appends the rows of a deployment element and what it holds:
/// child nodes, infrastructure nodes, system instances, container
/// instances (model.jsp `renderDeploymentNode`).
fn deployment_rows(model: &Model, element: ElementId, depth: usize, section: &Section, out: &mut Vec<(ElementId, usize, Section)>) {
    out.push((element, depth, section.clone()));
    for kind in [
        ElementKind::DeploymentNode,
        ElementKind::InfrastructureNode,
        ElementKind::SoftwareSystemInstance,
        ElementKind::ContainerInstance,
    ] {
        for c in model.children(element) {
            if model.element(c).kind == kind {
                deployment_rows(model, c, depth + 1, section, out);
            }
        }
    }
}

/// Ids that are numbers sort as numbers, before keys.
fn id_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// `<path>:<line>:<column>` of the declaration at `span` (P3-506).
#[must_use]
pub fn reveal_text(parsed: &Parsed, span: Span) -> String {
    let index = span.file.0 as usize;
    let path = parsed.paths.get(index).map_or_else(String::new, |p| p.display().to_string());
    let at = parsed
        .trees
        .get(index)
        .map(|t| c4rizer_core::text::position(t.source(), span.start));
    match at {
        Some(p) => format!("{path}:{}:{}", p.line, p.column),
        None => path,
    }
}

/// The tree's nodes laid out left to right: depth on x, leaves on
/// consecutive rows, parents centred on their children (P3-510).
#[must_use]
pub fn lay_out_tree(root: &TreeNode) -> Vec<TreeSpot> {
    const ROW: i32 = 30;
    fn walk(n: &TreeNode, depth: usize, parent: Option<usize>, next_row: &mut i32, spots: &mut Vec<(TreeSpot, usize)>) -> i32 {
        let me = spots.len();
        spots.push((
            TreeSpot {
                element: n.element,
                label: n.label.clone(),
                at: Point::new(0, 0),
                parent,
            },
            depth,
        ));
        let y = if n.children.is_empty() {
            let y = *next_row * ROW;
            *next_row += 1;
            y
        } else {
            let ys: Vec<i32> = n.children.iter().map(|c| walk(c, depth + 1, Some(me), next_row, spots)).collect();
            i32::midpoint(ys[0], ys[ys.len() - 1])
        };
        spots[me].0.at.y = y;
        y
    }
    let mut spots = Vec::new();
    let mut next_row = 0;
    walk(root, 0, None, &mut next_row, &mut spots);
    let depth_max = spots.iter().map(|(_, d)| *d).max().unwrap_or(0);
    let mut x = vec![0_i32; depth_max + 1];
    for d in 1..=depth_max {
        let widest = spots
            .iter()
            .filter(|(_, sd)| *sd == d - 1)
            .map(|(s, _)| label_px(&s.label))
            .fold(0.0_f32, f32::max);
        #[expect(clippy::cast_possible_truncation, reason = "label widths are a few hundred px")]
        let step = (widest.round() as i32 + 60).max(120);
        x[d] = x[d - 1] + step;
    }
    spots
        .into_iter()
        .map(|(mut s, d)| {
            s.at.x = x[d];
            s
        })
        .collect()
}

/// The width a label takes at the tree's 13px font, estimated, px.
fn label_px(label: &str) -> f32 {
    #[expect(clippy::cast_precision_loss, reason = "label lengths are small")]
    let n = label.chars().count() as f32;
    n * 7.2 + 14.0
}

/// Room to the right of the tree's last column for its labels, px.
fn label_room(spots: &[TreeSpot]) -> i32 {
    #[expect(clippy::cast_possible_truncation, reason = "label widths are a few hundred px")]
    let widest = spots.iter().map(|s| label_px(&s.label)).fold(0.0_f32, f32::max).round() as i32;
    widest + 20
}

/// The bounds of `points` with `margin` around and `right` extra to the
/// right (labels), px.
fn points_bounds(points: &[Point], margin: i32, right: i32) -> Rect {
    let (mut min, mut max) = (Point::new(0, 0), Point::new(0, 0));
    if let Some(first) = points.first() {
        (min, max) = (*first, *first);
    }
    for p in points {
        min = Point::new(min.x.min(p.x), min.y.min(p.y));
        max = Point::new(max.x.max(p.x), max.y.max(p.y));
    }
    Rect::from_corners(Point::new(min.x - margin, min.y - margin), Point::new(max.x + margin + right, max.y + margin))
}

/// The camera showing `bounds` centred in `canvas`, up to 200% zoom:
/// graphs and trees are small, so unlike the diagram fit (P2-738, at most
/// 100%) they may be enlarged to fill the page.
// DECISION: 200% at most and 20% at least; the wheel goes beyond.
fn fit_page(bounds: Rect, canvas: ScreenPoint) -> Camera {
    #[expect(clippy::cast_precision_loss, reason = "layout px fit f32")]
    let (w, h) = (bounds.width().max(1) as f32, bounds.height().max(1) as f32);
    let zoom = (canvas.x / w).min(canvas.y / h).clamp(0.2, 2.0);
    #[expect(clippy::cast_precision_loss, reason = "layout px fit f32")]
    let (cx, cy) = (
        (bounds.min.x + bounds.max.x) as f32 / 2.0,
        (bounds.min.y + bounds.max.y) as f32 / 2.0,
    );
    Camera {
        pan: ScenePoint {
            x: cx - canvas.x / 2.0 / zoom,
            y: cy - canvas.y / 2.0 / zoom,
        },
        zoom,
    }
}

/// The EDIT-28 gestures on a graph or tree canvas: the wheel zooms at the
/// cursor, middle drag or Space+drag pans (P3-512).
fn pan_zoom(ui: &egui::Ui, response: &egui::Response, camera: &mut Camera, rel: impl Fn(egui::Pos2) -> ScreenPoint) {
    let space = ui.input(|i| i.key_down(egui::Key::Space)) && !ui.ctx().text_edit_focused();
    if response.dragged_by(egui::PointerButton::Middle) || (space && response.dragged_by(egui::PointerButton::Primary)) {
        let d = response.drag_delta();
        camera.pan_by(ScreenPoint { x: d.x, y: d.y });
    }
    if response.hovered() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = (scroll / 300.0).exp() * pinch;
        if (factor - 1.0).abs() > 1e-4
            && let Some(p) = response.hover_pos()
        {
            camera.zoom_at(rel(p), factor);
        }
    }
}

/// A model colour at `alpha`.
fn colour(c: Colour, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, alpha)
}

/// `c` a third darker (a stroke when the style sets none).
fn darker(c: Colour) -> Colour {
    let d = |v: u8| u8::try_from(u16::from(v) * 2 / 3).unwrap_or(v);
    Colour { r: d(c.r), g: d(c.g), b: d(c.b) }
}

/// An opacity 0..1 as an alpha byte.
fn alpha(opacity: f32) -> u8 {
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to 0..=255")]
    let a = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    a
}

/// The bold font family.
fn bold() -> egui::FontFamily {
    egui::FontFamily::Name(crate::canvas::BOLD_FAMILY.into())
}

/// A small rounded label.
fn badge(ui: &mut egui::Ui, text: &str, fill: egui::Color32, colour: egui::Color32) -> egui::Response {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(4, 1))
        .show(ui, |ui| ui.label(egui::RichText::new(text).size(11.0).color(colour)))
        .response
}

/// A decision status as a coloured badge (decisions.jsp colours).
fn status_badge(ui: &mut egui::Ui, status: &str) {
    let fill = match status.to_ascii_lowercase().as_str() {
        "accepted" => egui::Color32::from_rgb(0x2e, 0x7d, 0x32),
        "superseded" | "deprecated" | "rejected" => egui::Color32::from_rgb(0x9e, 0x9e, 0x9e),
        _ => egui::Color32::from_rgb(0x1e, 0x88, 0xe5),
    };
    badge(ui, status, fill, egui::Color32::WHITE);
}

/// The name shown for a docs owner: the workspace or the element.
fn owner_name(model: &Model, owner: Option<ElementId>) -> String {
    owner.map_or_else(
        || "Workspace".to_owned(),
        |e| format!("{} {}", model.element(e).name.value, type_line(model, e)),
    )
}

/// Inline spans as wrapped text (P3-514).
fn inline(ui: &mut egui::Ui, spans: &[markdown::Span]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for s in spans {
            let mut text = egui::RichText::new(&s.text);
            if s.strong {
                text = text.family(bold());
            }
            if s.emphasis {
                text = text.italics();
            }
            if s.code {
                text = text.code();
            }
            match &s.link {
                Some(url) => {
                    ui.hyperlink_to(text, url);
                }
                None => {
                    ui.label(text);
                }
            }
        }
    });
}

/// A table cell's spans on one line, never wrapped (a grid cell has no
/// width to wrap in).
fn cell_text(ui: &mut egui::Ui, spans: &[markdown::Span]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for s in spans {
            let mut text = egui::RichText::new(&s.text);
            if s.strong {
                text = text.family(bold());
            }
            if s.emphasis {
                text = text.italics();
            }
            if s.code {
                text = text.code();
            }
            ui.add(egui::Label::new(text).extend());
        }
    });
}

/// An `AsciiDoc` section: its source in a monospace block under a one-line
/// note (P3-515).
fn asciidoc(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(ASCIIDOC_NOTE).italics().color(egui::Color32::from_rgb(0xb0, 0x70, 0x00)));
    egui::Frame::new()
        .fill(ui.visuals().code_bg_color)
        .corner_radius(4.0)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width((ui.available_width() - 16.0).max(40.0));
            ui.label(egui::RichText::new(text).monospace());
        });
}

/// The note above an `AsciiDoc` section (P3-515).
pub const ASCIIDOC_NOTE: &str = "AsciiDoc is not rendered (no maintained Rust AsciiDoc renderer exists); shown as source.";

/// A one-line placeholder in a dashed frame.
fn placeholder(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
        .corner_radius(4.0)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.weak(text);
        });
}

/// The number of remote images among `blocks`, nested ones included.
fn count_remote(blocks: &[markdown::Block]) -> usize {
    use markdown::{Block, ImageSource};
    blocks
        .iter()
        .map(|b| match b {
            Block::Image { source: ImageSource::Remote(_), .. } => 1,
            Block::List { items, .. } => items.iter().map(|i| count_remote(i)).sum(),
            Block::Quote(inner) => count_remote(inner),
            _ => 0,
        })
        .sum()
}

/// SEAM(network, P3-600): the fetch of a remote image in a documentation
/// section. Row `network` is not merged on this branch, so nothing is
/// fetched and the caller draws the placeholder and the notice; once
/// `network` lands, this calls its fetch when the file's "Allow network"
/// switch is on (deferred row "explore remote images").
fn fetch_remote(_url: &str) -> Option<Vec<u8>> {
    None
}

/// An image at its own size, scaled down to the column width.
fn draw_image(ui: &mut egui::Ui, asset: &Asset, width: i32, height: i32) {
    #[expect(clippy::cast_precision_loss, reason = "image sizes are at most 20000 px")]
    let (w, h) = (width as f32, height as f32);
    let scale = (ui.available_width() / w).min(1.0);
    let size = egui::vec2(w * scale, h * scale);
    let Some(image) = crate::canvas::icon_image(asset, size.x, size.y) else {
        placeholder(ui, &format!("Image `{}` cannot be decoded", asset.path));
        return;
    };
    let texture = ui.ctx().load_texture(format!("doc-image-{}", asset.path), image, egui::TextureOptions::LINEAR);
    ui.add(egui::Image::new((texture.id(), size)));
}

/// An embedded view's scene at its fitted size: the column width, never
/// above 100% (P3-517).
fn embedded_view(ui: &mut egui::Ui, build: &ViewBuild) {
    let scene = &build.scene;
    #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
    let (w, h) = (scene.bounds.width() as f32, scene.bounds.height() as f32);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let zoom = (ui.available_width() / w).min(1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w * zoom, h * zoom), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    // The painter's clip rect may be cut by the scroll area; the canvas
    // paints relative to the clip rect's corner, so the camera makes up
    // the difference.
    let clip = painter.clip_rect();
    #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
    let camera = Camera {
        pan: ScenePoint {
            x: scene.bounds.min.x as f32 + (clip.min.x - rect.min.x) / zoom,
            y: scene.bounds.min.y as f32 + (clip.min.y - rect.min.y) / zoom,
        },
        zoom,
    };
    let list = DrawList {
        scene: Some(scene),
        camera,
        marks: Vec::new(),
        rubber_band: None,
        resize_outline: None,
        editing: None,
        read_only: true,
        hide_descriptions: false,
        hide_metadata: false,
        hide_key: false,
        faded: Vec::new(),
        tooltip: None,
    };
    crate::canvas::paint_page(&painter, &list, None);
    crate::canvas::paint(&painter, &crate::canvas::paint_items(&list));
}
