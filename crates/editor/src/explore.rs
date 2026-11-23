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
// boxes inside boxes (model.jsp nests `div`s); each row shows the
// element's fill as a 10 px swatch on the panel ground (redesign
// 2026-11-19, `components/ModelRow`) instead of filling the whole row.
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

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use c4rizer_core::explore::{self, Drawn, RelationshipRow, TreeNode, Usage};
use c4rizer_core::geom::{Point, Rect};
use c4rizer_core::model::{Asset, Colour, ElementId, ElementKey, ElementKind, Mode, Model, Parsed, ResolvedStyle};
use c4rizer_core::pipeline::ViewBuild;
use c4rizer_core::place::Keep;
use c4rizer_core::text::Span;
use c4rizer_core::view::{BoxId, ViewPlan};
use eframe::egui;

use crate::camera::{Camera, ScenePoint, ScreenPoint};
use crate::icons::{self, Icon};
use crate::kit::{self, ChipKind, Look};
use crate::state::{DrawList, kind_label};
use crate::theme::{
    self, ACCENT, ACCENT_SOFT, BG_CANVAS, BG_PANEL, BG_RAISED, INK, INK_2, INK_3, LINE, LINE_2, PAD, SIZE_CONTROL, SIZE_ICON,
    SIZE_ROW,
};
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
        /// The element's canonical key, `None` when it has none.
        element: Option<ElementKey>,
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
    /// The light or dark mode the plan was built in.
    pub mode: Mode,
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
    /// The relative Markdown link clicked during this frame, if any.
    followed: Option<String>,
    /// The pages' data, kept between frames (P3-503).
    memo: Memo,
    /// The embedded views drawn in the last frame: key and screen
    /// rectangle (P3-517).
    pub drawn_embeds: Vec<(String, egui::Rect)>,
    /// The docs images' textures, kept until the next parse (P3-514).
    textures: Textures,
}

/// Docs image textures by file, each with the size it was made at (an
/// SVG is rasterised at the size it is drawn); a new drawn size replaces
/// the file's texture, so dragging a panel edge keeps one per file.
#[derive(Default)]
struct Textures(BTreeMap<PathBuf, ((u32, u32), egui::TextureHandle)>);

impl std::fmt::Debug for Textures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

/// The pages' data, rebuilt only when its inputs change, so an unchanged
/// frame costs drawing only (P3-503): the model rows per search text,
/// the table per search text and sort, the usages per selection; cleared
/// on a new parse.
// DECISION: memoised in the shell, not in core: the inputs are the
// page's own (search text, sort, selection), and core stays stateless.
#[derive(Debug, Default)]
struct Memo {
    /// The model rows and the trimmed search text they were built for.
    rows: Option<(String, Arc<Vec<ModelRow>>)>,
    /// The table rows and the (search text, column, descending) key.
    table: Option<(TableKey, Arc<Vec<TableRow>>)>,
    /// The usages and the element they were built for.
    usages: Option<(ElementId, Arc<Vec<Usage>>)>,
    /// Each model row's height in the last frame it was drawn, px (`None`
    /// before that), so the rows out of sight keep their place without
    /// being drawn.
    heights: Vec<Option<f32>>,
}

/// What the table rows depend on: search text, sort column, descending.
type TableKey = (String, Column, bool);

/// The height a model row is given before it was first drawn, px.
const ROW_ESTIMATE: f32 = 52.0;

/// The bar's height with its hairline, px (`.c4-explore__bar`).
const BAR: f32 = 36.0;

/// The usages panel's width, px (`.c4-explore__side`).
const SIDE: f32 = 260.0;

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
    /// Each drawn element's style as the canvas draws it, for one mode;
    /// built on first use and again when the mode changes.
    styles: Option<DrawnStyles>,
}

/// The style each element is drawn in, in `mode`: the house palette
/// where the file sets no colour, as on the canvas (redesign 2026-11-19).
// DECISION: read from `view::plan_in`, one plan per drawable view, as the
// house palette is applied there privately in core; it depends on the
// element and mode only, so any view drawing an element gives its style.
#[derive(Debug)]
struct DrawnStyles {
    /// The mode the styles were built in.
    mode: Mode,
    /// The style by element, for every element some view draws.
    by_element: BTreeMap<ElementId, ResolvedStyle>,
}

impl DrawnStyles {
    /// Plans every drawable view of `model` in `mode` and keeps each
    /// box's style.
    fn build(model: &Model, mode: Mode) -> Self {
        let mut by_element = BTreeMap::new();
        for view in model.views.iter().filter(|v| v.kind.drawable()) {
            for b in c4rizer_core::view::plan_in(model, view, mode).boxes {
                by_element.entry(b.element).or_insert(b.style);
            }
        }
        Self { mode, by_element }
    }
}

/// The style `element` is drawn in, in `mode`, from `styles` when they
/// were built in that mode.
// DECISION: an element no view draws (or styles not built yet) falls back
// to `Model::style_of_in`, the file's own colours without the house
// palette: there is no drawn box to match, and core keeps the palette
// private.
fn drawn_style<'a>(styles: Option<&'a DrawnStyles>, model: &Model, element: ElementId, mode: Mode) -> Cow<'a, ResolvedStyle> {
    styles
        .filter(|s| s.mode == mode)
        .and_then(|s| s.by_element.get(&element))
        .map_or_else(|| Cow::Owned(model.style_of_in(element, mode)), Cow::Borrowed)
}

/// The canonical key of `element`, `None` when it has none.
fn key_of(model: &Model, element: ElementId) -> Option<ElementKey> {
    model.full_key(element)
}

/// The id shown for `element`: its full key, else its index (as
/// `explore::search` matches it).
fn element_id_text(model: &Model, element: ElementId) -> String {
    model.full_key(element).map_or_else(|| element.0.to_string(), ElementKey::into_string)
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
            styles: None,
        });
        self.memo = Memo::default();
        self.textures = Textures::default();
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

    /// Brings the drawn styles up to date with the parse and the mode;
    /// they are rebuilt only when either changed (a new parse clears
    /// them in `sync`).
    fn refresh_styles(&mut self) {
        let mode = self.mode;
        let Some(cache) = &mut self.cache else { return };
        if cache.styles.as_ref().is_none_or(|s| s.mode != mode) {
            cache.styles = Some(DrawnStyles::build(&cache.parsed.model, mode));
        }
    }

    /// The drawn styles of the last parse, if built.
    fn styles(&self) -> Option<&DrawnStyles> {
        self.cache.as_ref().and_then(|c| c.styles.as_ref())
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

    /// `model_rows`, kept until the parse or the search text changes
    /// (P3-503).
    pub(crate) fn cached_model_rows(&mut self, model: &Model) -> Arc<Vec<ModelRow>> {
        let query = self.query.trim().to_owned();
        if let Some((q, rows)) = &self.memo.rows
            && *q == query
        {
            return Arc::clone(rows);
        }
        let rows = Arc::new(self.model_rows(model));
        self.memo.heights = vec![None; rows.len()];
        self.memo.rows = Some((query, Arc::clone(&rows)));
        rows
    }

    /// How many model rows have been laid out since the rows were built
    /// (the rest were never in sight).
    #[cfg(test)]
    pub(crate) fn rows_laid_out(&self) -> usize {
        self.memo.heights.iter().flatten().count()
    }

    /// How many docs image textures are kept.
    #[cfg(test)]
    pub(crate) fn texture_entries(&self) -> usize {
        self.textures.0.len()
    }

    /// `table`, kept until the parse, the search text or the sort
    /// changes (P3-503).
    pub(crate) fn cached_table(&mut self, model: &Model) -> Arc<Vec<TableRow>> {
        let key = (self.query.trim().to_owned(), self.sort.0, self.sort.1);
        if let Some((k, rows)) = &self.memo.table
            && *k == key
        {
            return Arc::clone(rows);
        }
        let rows = Arc::new(self.table(model));
        self.memo.table = Some((key, Arc::clone(&rows)));
        rows
    }

    /// `usages`, kept until the parse or the selection changes (P3-503,
    /// P3-504): each build plans every declared view.
    pub(crate) fn cached_usages(&mut self, model: &Model) -> Arc<Vec<Usage>> {
        let Some(selected) = self.selected else { return Arc::new(Vec::new()) };
        if let Some((e, usages)) = &self.memo.usages
            && *e == selected
        {
            return Arc::clone(usages);
        }
        let usages = Arc::new(self.usages(model));
        self.memo.usages = Some((selected, Arc::clone(&usages)));
        usages
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
        if self.graph.as_ref().is_some_and(|g| g.key == key && g.mode == self.mode) {
            return;
        }
        let Some(view) = model.view(key).filter(|v| explore::graph_offered(v.kind)) else {
            self.graph = None;
            return;
        };
        let plan = c4rizer_core::view::plan_in(model, view, self.mode);
        // The same view in the other mode: new colours, same nodes, so
        // the layout, the camera and the focus stay.
        if let Some(g) = self.graph.as_mut().filter(|g| g.key == key) {
            g.plan = Arc::new(plan);
            g.mode = self.mode;
            return;
        }
        let positions = explore::graph_layout(&plan);
        self.graph = Some(GraphState {
            key: key.to_owned(),
            plan: Arc::new(plan),
            positions,
            focus: None,
            camera: Camera::default(),
            fitted: false,
            origin: egui::Pos2::ZERO,
            mode: self.mode,
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

    /// The index of the decision a Markdown link in decision `from`'s
    /// text opens (P3-516): a relative link to another decision's file
    /// of the same owner (adr-tools writes `[3. Title](0003-title.md)`),
    /// or `#<id>`. `None` for anything else, which stays an outside link.
    // DECISION: Structurizr's importer rewrites links to other decision
    // files into in-app links; here the file name is matched at click
    // time instead, query and fragment ignored.
    #[must_use]
    pub fn decision_for_link(&self, from: usize, url: &str) -> Option<usize> {
        if let Some(id) = url.strip_prefix('#') {
            return self.linked_decision(from, id);
        }
        if url.contains("://") || url.starts_with("mailto:") {
            return None;
        }
        let path = url.split(['#', '?']).next().unwrap_or_default();
        let name = Path::new(path).file_name()?;
        let all = self.decisions();
        let owner = all.get(from).map(|d| d.owner);
        all.iter()
            .position(|d| Some(d.owner) == owner && d.path.file_name() == Some(name))
    }

    /// The index of the section a Markdown link in section `from` opens:
    /// a relative link to another section's file, resolved from `from`'s
    /// folder (P3-513). `None` for anything else, which stays an outside
    /// link or does nothing.
    // TODO(srs): P3-513/P3-514 give links between sections no behaviour;
    // Structurizr's documentation viewer follows them in-app, so they open
    // the linked section here.
    #[must_use]
    pub fn section_for_link(&self, from: usize, url: &str) -> Option<usize> {
        if url.contains("://") || url.starts_with("mailto:") {
            return None;
        }
        let path = url.split(['#', '?']).next().unwrap_or_default();
        if path.is_empty() {
            return None;
        }
        let all = self.sections();
        let folder = all.get(from)?.path.parent()?;
        let target = lexical(&folder.join(path));
        all.iter().position(|s| lexical(&s.path) == target)
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
        self.drawn_embeds.clear();
        self.refresh_styles();
        ui.spacing_mut().item_spacing.y = 0.0;
        self.bar(ui, model);
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

    /// The bar (`.c4-explore__bar`): the page tabs, the graph and tree
    /// selects, the search field at the right; a hairline under it.
    fn bar(&mut self, ui: &mut egui::Ui, model: &Model) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), BAR), egui::Sense::hover());
        let rule = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 1.0), rect.max);
        ui.painter().rect_filled(rule, 0.0, LINE);
        let inner = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.max.y - 1.0)).shrink2(egui::vec2(PAD, 0.0));
        let mut bar = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        bar.set_clip_rect(inner.intersect(ui.clip_rect()));
        bar.spacing_mut().item_spacing.x = 16.0;
        for (page, label) in [
            (Page::Model, "Model"),
            (Page::Relationships, "Relationships"),
            (Page::Documentation, "Documentation"),
            (Page::Decisions, "Decisions"),
        ] {
            if tab(&mut bar, label, self.page == page).clicked() {
                self.page = page;
            }
        }
        let search = matches!(self.page, Page::Model | Page::Relationships);
        // DECISION: the selects take 123 px each (the design's 250 px
        // pair) and the search 280 px; in a narrow window both shrink,
        // the selects to 70 px, the search to 100 px, before anything
        // is cut at the bar's end.
        let room = bar.available_width() - if search { 16.0 + 100.0 } else { 0.0 };
        let select_w = ((room - 4.0) / 2.0).clamp(70.0, 123.0);
        bar.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for graph in [true, false] {
                self.view_select(ui, model, graph, select_w);
            }
        });
        if search {
            let w = bar.available_width().clamp(100.0, 280.0);
            bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(w, SIZE_CONTROL),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| self.search_field(ui, w),
                );
            });
        }
    }

    /// The `Graph…` or `Tree…` select: one row per view offering it.
    fn view_select(&mut self, ui: &mut egui::Ui, model: &Model, graph: bool, width: f32) {
        let pages = Explore::pages(model);
        let label = if graph { "Graph" } else { "Tree" };
        let current = match &self.page {
            Page::Graph(k) if graph => Some(k.clone()),
            Page::Tree(k) if !graph => Some(k.clone()),
            _ => None,
        };
        let keys: Vec<&String> = pages
            .iter()
            .filter_map(|p| match p {
                Page::Graph(k) if graph => Some(k),
                Page::Tree(k) if !graph => Some(k),
                _ => None,
            })
            .collect();
        let titles: Vec<String> = keys
            .iter()
            .map(|key| {
                model
                    .view(key)
                    .and_then(|v| v.title.as_ref().or(v.description.as_ref()))
                    .map_or_else(|| (*key).clone(), |t| format!("{key} · {}", t.value))
            })
            .collect();
        let text = current.as_ref().map_or_else(|| format!("{label}…"), |k| format!("{label}: {k}"));
        let selected = keys.iter().position(|k| current.as_deref() == Some(k.as_str()));
        let picked = ui
            .allocate_ui_with_layout(
                egui::vec2(width, SIZE_CONTROL),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_width(width);
                    kit::select(ui, if graph { "explore-graph" } else { "explore-tree" }, &text, &titles, selected)
                },
            )
            .inner;
        if let Some(key) = picked.and_then(|i| keys.get(i)).map(|k| (*k).clone()) {
            if graph {
                self.open_graph(model, &key);
            } else {
                self.open_tree(model, &key);
            }
        }
    }

    /// The search field (`components/Field`): `bg-canvas`, `line-strong`
    /// around (`accent` while typing), a search icon, a clear button once
    /// something is typed (P3-503).
    fn search_field(&mut self, ui: &mut egui::Ui, width: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, SIZE_CONTROL), egui::Sense::hover());
        let ground = ui.painter().add(egui::Shape::Noop);
        let mut field = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        field.spacing_mut().item_spacing.x = 4.0;
        let (icon, _) = field.allocate_exact_size(egui::vec2(SIZE_ICON, SIZE_ICON), egui::Sense::hover());
        icons::paint(field.painter(), Icon::Search, icon, INK_3);
        let clear = if self.query.is_empty() { 0.0 } else { SIZE_ICON + 4.0 };
        // DECISION: the design's shorter hint; the search still matches
        // technology and properties too (P3-503).
        let edit = field.add(
            egui::TextEdit::singleline(&mut self.query)
                .id(egui::Id::unique("c4rizer-explore-search"))
                .hint_text(egui::RichText::new("Search name, id, description, tag").color(INK_3))
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .align(egui::Align2::LEFT_CENTER)
                .min_size(egui::vec2(0.0, SIZE_CONTROL - 2.0))
                .text_color(INK)
                .desired_width((field.available_width() - clear).max(20.0)),
        );
        if !self.query.is_empty() {
            let (r, response) = field.allocate_exact_size(egui::vec2(SIZE_ICON, SIZE_ICON), egui::Sense::click());
            icons::paint(field.painter(), Icon::Close, r, if response.hovered() { INK } else { INK_3 });
            if kit::tip(response, "Clear the search", None).clicked() {
                self.query.clear();
            }
        }
        let border = if edit.has_focus() { ACCENT } else { theme::LINE_STRONG };
        ui.painter().set(
            ground,
            egui::epaint::RectShape::new(
                rect,
                theme::RADIUS_1,
                BG_CANVAS,
                egui::Stroke::new(1.0, border),
                egui::StrokeKind::Inside,
            ),
        );
    }

    /// The model page: nested rows and, for the selected element, its
    /// usages (P3-500, P3-501, P3-504, P3-505, P3-506).
    fn model_page(&mut self, ui: &mut egui::Ui, model: &Model, out: &mut Vec<Action>) {
        let rows = self.cached_model_rows(model);
        egui::Panel::right("c4rizer-explore-usages")
            .resizable(true)
            .frame(egui::Frame::new().fill(BG_PANEL))
            .default_size(SIDE)
            .show(ui, |ui| self.usages_panel(ui, model, out));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG_PANEL)).show(ui, |ui| {
            // DECISION: the rows out of sight are not drawn: each takes the
            // height it had when last drawn (an estimate before that), so a
            // frame costs the rows in sight only (P3-503). Rows vary in
            // height (wrapping, expanded relationships), so egui's
            // `show_rows` (one fixed height) does not fit.
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if rows.is_empty() {
                    empty(ui, if self.query.is_empty() { "The model is empty." } else { "Nothing matches the search." });
                }
                if self.memo.heights.len() != rows.len() {
                    self.memo.heights = vec![None; rows.len()];
                }
                let (above, below) = (viewport.min.y - 200.0, viewport.max.y + 200.0);
                let mut y = 0.0;
                let mut section: Option<&Section> = None;
                for (i, row) in rows.iter().enumerate() {
                    let first = section != Some(&row.section);
                    section = Some(&row.section);
                    let h = self.memo.heights[i].unwrap_or(ROW_ESTIMATE);
                    if y + h < above || y > below {
                        ui.add_space(h);
                        y += h;
                        continue;
                    }
                    let top = ui.cursor().top();
                    if first {
                        let n = rows.iter().filter(|r| r.section == row.section).count();
                        let title = match &row.section {
                            Section::Static => "Static model".to_owned(),
                            Section::Deployment(env) => format!("Deployment model: {env}"),
                        };
                        group(ui, &title, &format!("{n} element{}", if n == 1 { "" } else { "s" }));
                    }
                    self.model_row(ui, model, row, out);
                    let drawn = ui.cursor().top() - top;
                    self.memo.heights[i] = Some(drawn);
                    y += drawn;
                }
            });
        });
    }

    /// One element row (`components/ModelRow`), with its relationships
    /// when expanded: chevron, swatch of the element's fill, the lines,
    /// then `Usages` and "Reveal in file" at the right.
    fn model_row(&mut self, ui: &mut egui::Ui, model: &Model, row: &ModelRow, out: &mut Vec<Action>) {
        let selected = self.selected == Some(row.element);
        let open = self.expanded.contains(&row.element);
        #[expect(clippy::cast_precision_loss, reason = "nesting depth is tiny")]
        let indent = row.depth as f32 * 24.0;
        let full = ui.available_rect_before_wrap();
        let (left, right, top) = (full.min.x, full.max.x, full.min.y);
        let x0 = left + PAD + indent;
        let usages_w = text_px(ui, "Usages", &theme::ui_font()) + 16.0;
        let acts_w = usages_w + 4.0 + SIZE_CONTROL;
        let main_x = x0 + 16.0 + 8.0 + 10.0 + 8.0;
        // The lines' width: the column minus the indent, the leading marks
        // and the buttons, so no row runs under the usages panel.
        let main_w = (right - PAD - acts_w - 8.0 - main_x).max(80.0);
        let ground = ui.painter().add(egui::Shape::Noop);
        let mut main = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(egui::pos2(main_x, top + 4.0), egui::vec2(main_w, 4096.0)))
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        if !row.matched {
            main.multiply_opacity(0.45);
        }
        self.row_lines(&mut main, row, selected);
        let h = (main.min_rect().height() + 8.0).max(36.0);
        let rect = egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, top + h));
        ui.allocate_rect(rect, egui::Sense::hover());
        let ground_fill = if selected {
            ACCENT_SOFT
        } else if ui.rect_contains_pointer(rect) {
            BG_RAISED
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().set(ground, egui::Shape::rect_filled(rect, 0.0, ground_fill));
        row_rule(ui, rect);
        let cy = rect.center().y;
        let chevron = egui::Rect::from_center_size(egui::pos2(x0 + 8.0, cy), egui::vec2(SIZE_ICON, SIZE_ICON));
        let id = egui::Id::unique(("c4rizer-explore-chevron", row.element.0));
        let response = ui.interact(chevron.expand(4.0), id, egui::Sense::click());
        icons::paint(ui.painter(), if open { Icon::ChevronDown } else { Icon::ChevronRight }, chevron, if response.hovered() { INK } else { INK_3 });
        if response.on_hover_text("Show the relationships").clicked() {
            if open {
                self.expanded.remove(&row.element);
            } else {
                self.expanded.insert(row.element);
            }
        }
        let style = drawn_style(self.styles(), model, row.element, self.mode);
        let swatch = egui::Rect::from_center_size(egui::pos2(x0 + 16.0 + 8.0 + 5.0, cy), egui::vec2(10.0, 10.0));
        ui.painter().rect_filled(swatch, 1.0, colour(style.background, if row.matched { 255 } else { 110 }));
        let copy = egui::Rect::from_min_size(egui::pos2(right - PAD - SIZE_CONTROL, cy - SIZE_CONTROL / 2.0), egui::vec2(SIZE_CONTROL, SIZE_CONTROL));
        let usages = egui::Rect::from_min_size(egui::pos2(copy.min.x - 4.0 - usages_w, copy.min.y), egui::vec2(usages_w, SIZE_CONTROL));
        let r = put(ui, usages, |ui| kit::Button::new("Usages").look(Look::Ghost).on(selected).show(ui));
        if r.on_hover_text("Show the views that draw it").clicked() {
            self.selected = Some(row.element);
        }
        let r = put(ui, copy, |ui| kit::Button::icon(Icon::Copy).look(Look::Ghost).show(ui));
        if kit::tip(r, "Reveal in file: copy path:line:column", None).clicked()
            && let Some(parsed) = self.parsed()
        {
            out.push(Action::Copy(reveal_text(&parsed, model.element(row.element).span)));
        }
        if open {
            self.relationship_rows(ui, model, row.element, indent);
        }
    }

    /// The lines of one element row: key, name, type, marks and tags,
    /// then the description.
    fn row_lines(&mut self, ui: &mut egui::Ui, row: &ModelRow, selected: bool) {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 1.0);
        // A wrapped line's height comes from the parent's interact size.
        ui.spacing_mut().interact_size.y = 20.0;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
            kit::chip(ui, &row.id, ChipKind::Key);
            let name = egui::Label::new(egui::RichText::new(&row.name).font(theme::strong_font()).color(INK)).sense(egui::Sense::click());
            let response = ui.add(name).on_hover_text("Show its usages");
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                self.selected = Some(row.element);
            }
            let kind = row.type_line.trim_start_matches('[').trim_end_matches(']');
            ui.label(theme::label(kind).color(if selected { INK_2 } else { INK_3 }));
            if row.in_no_view {
                kit::chip(ui, "in no view", ChipKind::Warn).on_hover_text("No view draws this element (inspection model.element.noview)");
            }
            // DECISION: the generic `Element` tag every element carries is
            // not shown (the design's rows leave it out); the rest are.
            for tag in row.tags.iter().filter(|t| *t != "Element") {
                kit::chip(ui, tag, ChipKind::Plain);
            }
        });
        if !row.description.is_empty() {
            // DECISION: the description wraps instead of being cut to one
            // line, so a long one stays readable.
            ui.label(egui::RichText::new(&row.description).color(INK_2));
        }
    }

    /// The relationships under an expanded row (`.c4-mrow--rel`): arrow,
    /// other end (a click selects it), description, technology, `implied`.
    fn relationship_rows(&mut self, ui: &mut egui::Ui, model: &Model, element: ElementId, indent: f32) {
        let lines = self.relationship_lines(model, element);
        let full = ui.available_rect_before_wrap();
        let (left, right) = (full.min.x, full.max.x);
        let arrow_x = left + PAD + indent + 24.0 + 16.0 + 8.0;
        let main_x = arrow_x + 16.0 + 8.0;
        let main_w = (right - PAD - main_x).max(80.0);
        for i in 0..lines.len().max(1) {
            let top = ui.cursor().top();
            let ground = ui.painter().add(egui::Shape::Noop);
            let mut main = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_size(egui::pos2(main_x, top + 4.0), egui::vec2(main_w, 4096.0)))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            main.spacing_mut().interact_size.y = 20.0;
            main.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
                let Some(l) = lines.get(i) else {
                    ui.label(egui::RichText::new("No relationships.").color(INK_3));
                    return;
                };
                let other = model.element(l.other);
                let name = egui::Label::new(egui::RichText::new(&other.name.value).color(ACCENT)).sense(egui::Sense::click());
                let tip = format!("{} {} {}", if l.outgoing { "To" } else { "From" }, other.name.value, type_line(model, l.other));
                let response = ui.add(name).on_hover_text(tip);
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() {
                    self.selected = Some(l.other);
                }
                if l.description.is_empty() {
                    ui.label(egui::RichText::new("—").color(INK_3));
                } else {
                    ui.label(egui::RichText::new(&l.description).color(INK_2));
                }
                if !l.technology.is_empty() {
                    ui.label(theme::code(&l.technology, INK_3));
                }
                if l.implied {
                    kit::chip(ui, "implied", ChipKind::Ok);
                }
            });
            let h = (main.min_rect().height() + 8.0).max(SIZE_ROW);
            let rect = egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, top + h));
            ui.allocate_rect(rect, egui::Sense::hover());
            ui.painter().set(ground, egui::Shape::rect_filled(rect, 0.0, BG_CANVAS));
            row_rule(ui, rect);
            if let Some(l) = lines.get(i) {
                let r = egui::Rect::from_center_size(egui::pos2(arrow_x + 8.0, rect.center().y), egui::vec2(SIZE_ICON, SIZE_ICON));
                icons::paint(ui.painter(), if l.outgoing { Icon::ArrowDown } else { Icon::ArrowUp }, r, INK_3);
                let id = egui::Id::unique(("c4rizer-explore-direction", element.0, i));
                let _ = ui.interact(r, id, egui::Sense::hover()).on_hover_text(if l.outgoing { "Outgoing" } else { "Incoming" });
            }
        }
    }

    /// The usages of the selected element (P3-504): a header naming it
    /// with the count, one row per view (a click opens it with the box
    /// selected), then how each view draws it.
    fn usages_panel(&mut self, ui: &mut egui::Ui, model: &Model, out: &mut Vec<Action>) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let usages = self.cached_usages(model);
        let title = self.selected.map_or_else(|| "Usages".to_owned(), |e| format!("Usages · {}", model.element(e).name.value));
        let count = self.selected.map(|_| usages.len().to_string());
        side_head(ui, &title, count.as_deref());
        let Some(selected) = self.selected else {
            empty(ui, "Select an element to see the views that draw it.");
            return;
        };
        if usages.is_empty() {
            empty(ui, "In no view.");
            return;
        }
        let code = theme::code_font();
        let key_w = usages.iter().map(|u| text_px(ui, &u.view, &code)).fold(0.0_f32, f32::max).min(104.0);
        egui::ScrollArea::vertical().id_salt("c4rizer-explore-usages-list").show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 0)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for u in usages.iter() {
                    let target = match u.drawn {
                        Drawn::Lifted { onto } => onto,
                        _ => selected,
                    };
                    let response = usage_row(ui, &u.view, u.title.as_deref(), key_w);
                    if response.on_hover_text("Open this view with the box selected").clicked() {
                        out.push(Action::Open {
                            view: ViewRef::Declared(u.view.clone()),
                            element: key_of(model, target),
                        });
                    }
                }
            });
            egui::Frame::new().inner_margin(egui::Margin { left: 12, right: 12, top: 12, bottom: 12 }).show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 8.0);
                for u in usages.iter() {
                    let how = match u.drawn {
                        Drawn::Directly => "drawn directly".to_owned(),
                        Drawn::Lifted { onto } => format!("through {}", model.element(onto).name.value),
                        _ => "drawn".to_owned(),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(theme::code(&u.view, INK_2));
                        ui.label(egui::RichText::new(how).color(INK_2));
                    });
                }
            });
        });
    }

    /// The relationships table (`components/Table`; P3-502, P3-503,
    /// P3-506): a header that sorts, rows of elided cells, "Reveal in
    /// file" at each row's end.
    fn relationships_page(&mut self, ui: &mut egui::Ui, model: &Model, out: &mut Vec<Action>) {
        let rows = self.cached_table(model);
        group(ui, "Relationships", &format!("{} relationship{}", rows.len(), if rows.len() == 1 { "" } else { "s" }));
        let full = ui.available_width();
        let reveal = SIZE_CONTROL + 2.0 * 8.0;
        let widths = [0.08, 0.16, 0.16, 0.3, 0.14, 0.16].map(|f: f32| ((full - reveal) * f).max(40.0));
        self.table_head(ui, widths);
        if rows.is_empty() {
            empty(ui, if self.query.is_empty() { "No relationships." } else { "Nothing matches the search." });
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, SIZE_ROW, rows.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for row in &rows[range] {
                    if table_row(ui, row, widths) && let Some(parsed) = self.parsed() {
                        out.push(Action::Copy(reveal_text(&parsed, model.relationship(row.row.relationship).span)));
                    }
                }
            });
    }

    /// The table's header row: a click sorts by that column, again
    /// reverses (P3-502).
    fn table_head(&mut self, ui: &mut egui::Ui, widths: [f32; 6]) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), SIZE_ROW), egui::Sense::hover());
        let mut x = rect.min.x;
        for (column, w) in Column::ALL.into_iter().zip(widths) {
            let cell = egui::Rect::from_min_size(egui::pos2(x, rect.min.y), egui::vec2(w, SIZE_ROW));
            x += w;
            let response = ui.interact(cell, egui::Id::unique(("c4rizer-explore-sort", column)), egui::Sense::click());
            let sorted = self.sort.0 == column;
            let ink = if sorted || response.hovered() { INK } else { INK_3 };
            let galley = egui::WidgetText::from(theme::label(column.title()).color(ink)).into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                (w - 2.0 * PAD - 16.0).max(8.0),
                egui::TextStyle::Small,
            );
            let at = egui::pos2(cell.min.x + PAD, cell.center().y - galley.size().y / 2.0);
            let end = at.x + galley.size().x;
            ui.painter().galley(at, galley, ink);
            if sorted {
                let icon = if self.sort.1 { Icon::ArrowDown } else { Icon::ArrowUp };
                let r = egui::Rect::from_min_size(egui::pos2(end + 4.0, cell.center().y - 6.0), egui::vec2(12.0, 12.0));
                icons::paint(ui.painter(), icon, r, INK);
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.on_hover_text("Sort by this column").clicked() {
                self.sort = if sorted { (column, !self.sort.1) } else { (column, false) };
            }
        }
        let rule = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 1.0), rect.max);
        ui.painter().rect_filled(rule, 0.0, LINE_2);
    }

    /// The graph page (P3-508, P3-509, P3-512).
    #[expect(clippy::many_single_char_names, reason = "geometry: points, ends, radii")]
    fn graph_page(&mut self, ui: &mut egui::Ui, model: &Model, out: &mut Vec<Action>) {
        let Some(graph) = &self.graph else {
            empty(ui, "No graph for this view: graphs are offered for custom, landscape, context, container and component views.");
            return;
        };
        let title = model
            .view(&graph.key)
            .and_then(|v| v.title.as_ref().or(v.description.as_ref()))
            .map_or_else(|| graph.key.clone(), |t| t.value.clone());
        page_title(
            ui,
            &title,
            "Click a node to show its neighbours · double-click opens the diagram · wheel zooms · middle drag or Space+drag pans",
        );
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
        // The second click of a double click opens the view (below); it
        // does not toggle the focus the first click set.
        if response.clicked()
            && !response.double_clicked()
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
            empty(ui, "No tree for this view: trees are offered for custom, landscape, context, container, component and deployment views.");
            return;
        };
        let title = model
            .view(&tree.key)
            .and_then(|v| v.title.as_ref().or(v.description.as_ref()))
            .map_or_else(|| tree.key.clone(), |t| t.value.clone());
        page_title(ui, &title, "Double-click a node to open the diagram · wheel zooms · middle drag or Space+drag pans");
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
        let styles = self.cache.as_ref().and_then(|c| c.styles.as_ref());
        for s in &t.spots {
            let c = at(s.at);
            let (fill, stroke) = s.element.map_or((egui::Color32::from_gray(0x77), egui::Color32::BLACK), |e| {
                let st = drawn_style(styles, model, e, self.mode);
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
            empty(ui, "No documentation: attach a folder with `!docs <path>` to the workspace or a software system.");
            return;
        }
        egui::Panel::left("c4rizer-explore-sections")
            .resizable(true)
            .frame(egui::Frame::new().fill(BG_PANEL).inner_margin(egui::Margin::same(8)))
            .default_size(240.0)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
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
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG_PANEL).inner_margin(egui::Margin::same(12))).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let Some(section) = sections.get(self.section) else { return };
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.weak(format!("{} · {}", owner_name(model, section.owner), section.path.display()));
                ui.add_space(4.0);
                let folder = section.path.parent().map(Path::to_path_buf).unwrap_or_default();
                match section.format {
                    docs::Format::Markdown => {
                        self.followed = None;
                        self.render_markdown(ui, model, &section.text, &folder);
                        if let Some(to) = self.followed.take().and_then(|url| self.section_for_link(self.section, &url)) {
                            self.section = to;
                        }
                    }
                    docs::Format::AsciiDoc => asciidoc(ui, &section.text),
                }
            });
        });
    }

    /// The decisions viewer (P3-516).
    fn decisions_page(&mut self, ui: &mut egui::Ui, model: &Model) {
        let decisions = self.decisions().to_vec();
        if decisions.is_empty() {
            empty(ui, "No decisions: attach a folder with `!adrs <path>` to the workspace or a software system.");
            return;
        }
        egui::Panel::left("c4rizer-explore-decisions")
            .resizable(true)
            .frame(egui::Frame::new().fill(BG_PANEL).inner_margin(egui::Margin::same(8)))
            .default_size(360.0)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
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
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG_PANEL).inner_margin(egui::Margin::same(12))).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
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
                self.followed = None;
                self.render_markdown(ui, model, &d.text, &folder);
                if let Some(to) = self.followed.take().and_then(|url| self.decision_for_link(self.decision, &url)) {
                    self.decision = to;
                }
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
                .color(theme::WARN),
            );
        }
        for b in &blocks {
            self.block(ui, model, b, folder);
        }
    }

    /// Draws an image block: an embedded view (P3-517), a relative image
    /// from the docs folder, or a remote image's placeholder (P3-514).
    fn image_block(&mut self, ui: &mut egui::Ui, model: &Model, source: &markdown::ImageSource, folder: &Path) {
        use markdown::ImageSource;
        match source {
            ImageSource::Embed(key) => match self.embed(model, key) {
                Embed::View(build) => {
                    if let Some(rect) = embedded_view(ui, model, &build) {
                        self.drawn_embeds.push((key.clone(), rect));
                    }
                }
                Embed::Placeholder(text) => placeholder(ui, &text),
            },
            ImageSource::Relative(path) => {
                let file = folder.join(path);
                match self.image(&file) {
                    Some(image) => self.draw_image(ui, &file, &image),
                    None => placeholder(ui, &format!("Image `{path}` cannot be read")),
                }
            }
            ImageSource::Remote(url) => match fetch_remote(url) {
                Some(_) => {}
                None => placeholder(ui, &format!("Remote image not fetched (network off): {url}")),
            },
        }
    }

    /// Draws a docs image at its own size, scaled down to the column
    /// width; the texture is made on first sight and kept (P3-514).
    pub(crate) fn draw_image(&mut self, ui: &mut egui::Ui, file: &Path, image: &(Asset, i32, i32)) {
        let (asset, width, height) = image;
        #[expect(clippy::cast_precision_loss, reason = "image sizes are at most 20000 px")]
        let (w, h) = (*width as f32, *height as f32);
        let scale = (ui.available_width() / w).min(1.0);
        let size = egui::vec2(w * scale, h * scale);
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a drawn size in px")]
        let drawn = (size.x.round() as u32, size.y.round() as u32);
        let texture = if let Some((_, t)) = self.textures.0.get(file).filter(|(at, _)| *at == drawn) {
            t.clone()
        } else {
            let Some(pixels) = crate::canvas::icon_image(asset, size.x, size.y) else {
                placeholder(ui, &format!("Image `{}` cannot be decoded", asset.path));
                return;
            };
            let t = ui.ctx().load_texture(format!("doc-image-{}", asset.path), pixels, egui::TextureOptions::LINEAR);
            self.textures.0.insert(file.to_path_buf(), (drawn, t.clone()));
            t
        };
        ui.add(egui::Image::new((texture.id(), size)));
    }

    /// Draws one block.
    fn block(&mut self, ui: &mut egui::Ui, model: &Model, block: &markdown::Block, folder: &Path) {
        use markdown::Block;
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
                if let Some(url) = inline(ui, spans) {
                    self.followed = Some(url);
                }
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
                self.image_block(ui, model, source, folder);
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

/// `path` with `.` and `dir/..` steps taken out, without touching the
/// disk, so two spellings of one file compare equal.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if out.file_name().is_some() => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
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
/// cursor, middle drag, right drag or Space+drag pans (P3-512).
fn pan_zoom(ui: &egui::Ui, response: &egui::Response, camera: &mut Camera, rel: impl Fn(egui::Pos2) -> ScreenPoint) {
    let space = ui.input(|i| i.key_down(egui::Key::Space)) && !ui.ctx().text_edit_focused();
    if response.dragged_by(egui::PointerButton::Middle)
        || response.dragged_by(egui::PointerButton::Secondary)
        || (space && response.dragged_by(egui::PointerButton::Primary))
    {
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

/// The bold font family: the chrome's strong face (Geist, weight 600).
fn bold() -> egui::FontFamily {
    egui::FontFamily::Name(theme::SEMIBOLD.into())
}

/// The width of `text` in `font`, px.
fn text_px(ui: &egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), INK).size().x)
}

/// Runs `add` in a child placed at `rect`, without moving `ui`'s cursor.
fn put(ui: &mut egui::Ui, rect: egui::Rect, add: impl FnOnce(&mut egui::Ui) -> egui::Response) -> egui::Response {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    add(&mut child)
}

/// A page tab (`components/Tabs`): text, `ink-2` (`ink` when hovered or
/// on), a 2 px `accent` underline when on.
fn tab(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let font = theme::ui_font();
    let w = text_px(ui, text, &font);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(w, ui.available_height()), egui::Sense::click());
    let ink = if on || response.hovered() { INK } else { INK_2 };
    ui.painter().text(egui::pos2(rect.min.x, rect.center().y), egui::Align2::LEFT_CENTER, text, font, ink);
    if on {
        let line = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 2.0), rect.max);
        ui.painter().rect_filled(line, 0.0, ACCENT);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    kit::focus_ring(ui, &response);
    response
}

/// A group header (`.c4-group`): 28 px on `bg-canvas`, a label at the
/// left, a count at the right, a hairline under it.
fn group(ui: &mut egui::Ui, title: &str, count: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), SIZE_ROW), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, BG_CANVAS);
    let galley = egui::WidgetText::from(theme::label(title)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.width() / 2.0).max(40.0),
        egui::TextStyle::Small,
    );
    ui.painter()
        .galley(egui::pos2(rect.min.x + PAD, rect.center().y - galley.size().y / 2.0), galley, INK_3);
    ui.painter().text(
        egui::pos2(rect.max.x - PAD, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count,
        theme::code_font(),
        INK_3,
    );
    row_rule(ui, rect);
}

/// The 1 px `line` under a model row or group header, at the bottom of
/// `rect`, as wide as it.
fn row_rule(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().hline(rect.x_range(), rect.max.y - 0.5, egui::Stroke::new(1.0, LINE));
}

/// A side panel's header (`.c4-sec__head` without the fold): 28 px, a
/// label cut to fit, an optional count at the right, a hairline under.
fn side_head(ui: &mut egui::Ui, title: &str, count: Option<&str>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), SIZE_ROW), egui::Sense::hover());
    let room = (rect.width() - 2.0 * PAD - if count.is_some() { 32.0 } else { 0.0 }).max(16.0);
    let galley = egui::WidgetText::from(theme::label(title)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        room,
        egui::TextStyle::Small,
    );
    ui.painter()
        .galley(egui::pos2(rect.min.x + PAD, rect.center().y - galley.size().y / 2.0), galley, INK_3);
    if let Some(count) = count {
        ui.painter().text(
            egui::pos2(rect.max.x - PAD, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            count,
            egui::FontId::monospace(10.0),
            INK_3,
        );
    }
}

/// A short note in `ink-3`, padded 12.
fn empty(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.label(egui::RichText::new(text).color(INK_3));
    });
}

/// One view of the usages panel: key (mono) and title, 28 px, a click
/// opens it.
fn usage_row(ui: &mut egui::Ui, key: &str, title: Option<&str>, key_w: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), SIZE_ROW), egui::Sense::click());
    let p = ui.painter();
    if response.hovered() {
        p.rect_filled(rect, theme::RADIUS_1, BG_RAISED);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let code = theme::code_font();
    let x0 = rect.min.x + 8.0;
    let key_text = kit::elide(ui, key, &code, key_w);
    p.text(egui::pos2(x0, rect.center().y), egui::Align2::LEFT_CENTER, key_text, code, INK);
    if let Some(title) = title {
        let font = theme::ui_font();
        let x = x0 + key_w + 8.0;
        let shown = kit::elide(ui, title, &font, (rect.max.x - 8.0 - x).max(0.0));
        p.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, shown, font, INK);
    }
    kit::focus_ring(ui, &response);
    response
}

/// One row of the relationships table: cells cut to their column (the
/// whole text on hover), `implied` marked, the reveal button at the end.
/// True when "Reveal in file" was clicked.
fn table_row(ui: &mut egui::Ui, row: &TableRow, widths: [f32; 6]) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), SIZE_ROW), egui::Sense::hover());
    if response.hovered() || ui.rect_contains_pointer(rect) {
        ui.painter().rect_filled(rect, 0.0, BG_RAISED);
    }
    let rule = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 1.0), rect.max);
    ui.painter().rect_filled(rule, 0.0, LINE);
    let mut x = rect.min.x;
    for (column, w) in Column::ALL.into_iter().zip(widths) {
        let cell = egui::Rect::from_min_size(egui::pos2(x, rect.min.y), egui::vec2(w, SIZE_ROW));
        x += w;
        let text = row.cell(column);
        let (font, ink) = if column == Column::Id { (theme::code_font(), INK) } else { (theme::ui_font(), INK_2) };
        let implied = column == Column::Id && row.row.implied;
        let chip = if implied { text_px(ui, "implied", &theme::ui_font()) + 12.0 + 8.0 } else { 0.0 };
        let room = (w - 2.0 * PAD - chip).max(8.0);
        let shown = kit::elide(ui, text, &font, room);
        let end = cell.min.x + PAD + text_px(ui, &shown, &font);
        ui.painter().text(egui::pos2(cell.min.x + PAD, cell.center().y), egui::Align2::LEFT_CENTER, &shown, font, ink);
        if implied {
            let at = egui::Rect::from_min_size(egui::pos2(end + 8.0, cell.center().y - 9.0), egui::vec2(chip - 8.0, 18.0));
            let _ = put(ui, at, |ui| kit::chip(ui, "implied", ChipKind::Ok));
        }
        if shown != text {
            let id = egui::Id::unique(("c4rizer-explore-cell", row.row.relationship.0, column));
            let _ = ui.interact(cell, id, egui::Sense::hover()).on_hover_text(text);
        }
    }
    let at = egui::Rect::from_min_size(egui::pos2(rect.max.x - 8.0 - SIZE_CONTROL, rect.center().y - SIZE_CONTROL / 2.0), egui::vec2(SIZE_CONTROL, SIZE_CONTROL));
    let r = put(ui, at, |ui| kit::Button::icon(Icon::Copy).look(Look::Ghost).show(ui));
    kit::tip(r, "Reveal in file: copy path:line:column", None).clicked()
}

/// A graph or tree page's title line: the view's title, then the
/// gestures in `ink-3`, cut to fit; a hairline under it.
fn page_title(ui: &mut egui::Ui, title: &str, hint: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), SIZE_ROW), egui::Sense::hover());
    let strong = theme::strong_font();
    let x0 = rect.min.x + PAD;
    let title = kit::elide(ui, title, &strong, (rect.width() - 2.0 * PAD).max(0.0));
    let w = text_px(ui, &title, &strong);
    ui.painter().text(egui::pos2(x0, rect.center().y), egui::Align2::LEFT_CENTER, &title, strong, INK);
    let font = theme::ui_font();
    let x = x0 + w + PAD;
    let hint = kit::elide(ui, hint, &font, (rect.max.x - PAD - x).max(0.0));
    ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, hint, font, INK_3);
    let rule = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 1.0), rect.max);
    ui.painter().rect_filled(rule, 0.0, LINE);
}

/// A decision status as a chip: accepted reads fine, superseded,
/// deprecated and rejected plain, anything else current (decisions.jsp's
/// green, grey and blue).
fn status_badge(ui: &mut egui::Ui, status: &str) {
    let kind = match status.to_ascii_lowercase().as_str() {
        "accepted" => ChipKind::Ok,
        "superseded" | "deprecated" | "rejected" => ChipKind::Plain,
        _ => ChipKind::Accent,
    };
    kit::chip(ui, status, kind);
}

/// The name shown for a docs owner: the workspace or the element.
fn owner_name(model: &Model, owner: Option<ElementId>) -> String {
    owner.map_or_else(
        || "Workspace".to_owned(),
        |e| format!("{} {}", model.element(e).name.value, type_line(model, e)),
    )
}

/// Inline spans as wrapped text (P3-514). Returns the target of a
/// relative link clicked this frame: outside links open in the browser,
/// relative ones are the page's to follow.
fn inline(ui: &mut egui::Ui, spans: &[markdown::Span]) -> Option<String> {
    let mut clicked = None;
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
                Some(url) if url.contains("://") || url.starts_with("mailto:") => {
                    ui.hyperlink_to(text, url);
                }
                Some(url) => {
                    if ui.link(text).on_hover_text(url).clicked() {
                        clicked = Some(url.clone());
                    }
                }
                None => {
                    ui.label(text);
                }
            }
        }
    });
    clicked
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
    ui.label(egui::RichText::new(ASCIIDOC_NOTE).italics().color(theme::WARN));
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
        .stroke(egui::Stroke::new(1.0, LINE_2))
        .corner_radius(theme::RADIUS_2)
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



/// An embedded view's scene at its fitted size: the column width, never
/// above 100% (P3-517); the rectangle it is drawn in, `None` for an empty
/// scene.
// TODO(srs): P3-517 also asks the placeholder to match inspection
// `documentation.embeddedview`, which is not built (deferred row
// "explore embed inspection").
fn embedded_view(ui: &mut egui::Ui, model: &Model, build: &ViewBuild) -> Option<egui::Rect> {
    let scene = &build.scene;
    #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
    let (w, h) = (scene.bounds.width() as f32, scene.bounds.height() as f32);
    if w <= 0.0 || h <= 0.0 {
        return None;
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
        written_thickness: crate::canvas::thickness_written(model, scene),
        guides: &[],
    };
    crate::canvas::paint_page(&painter, &list, None);
    crate::canvas::paint(&painter, &crate::canvas::paint_items(&list));
    Some(rect)
}
