//! The explore pages (SRS 4.7 P3-9, P3-500..P3-517), Structurizr's
//! Explore equivalent: the model as a nested list, the relationships
//! table, search, usages, graph and tree of a view, the documentation and
//! decisions viewers. Read-only: nothing here writes the file (P3-507).
//!
//! Depends on: `state` (`ViewRef` targets, `kind_label`), `camera`,
//! `canvas` (embedded views), `worker`, `c4rizer-core` (`explore`),
//! `eframe`; children `docs` (reads `!docs`/`!adrs` files) and
//! `markdown` (pulldown-cmark to blocks).

// Shape: `Explore` is the headless page state (page, search text, sort,
// selected element, graph focus, cameras) plus a cache of what it derives
// from one parse (rows, "in no view", docs, decisions), rebuilt when the
// parse changes (`Arc::ptr_eq`). Its readers return plain rows the tests
// check; `show` draws them with egui and returns the actions clicks make
// (`Action::Open` opens a view with a box selected, `Action::Copy`
// copies text). The shell runs the actions.

pub mod docs;
pub mod markdown;

use std::path::PathBuf;
use std::sync::Arc;

use c4rizer_core::explore::{RelationshipRow, TreeNode, Usage};
use c4rizer_core::geom::Point;
use c4rizer_core::model::{ElementId, Model, Parsed};
use c4rizer_core::text::Span;
use c4rizer_core::view::{BoxId, ViewPlan};
use eframe::egui;

use crate::camera::{Camera, ScreenPoint};
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

/// The explore pages' state.
#[derive(Debug, Default)]
pub struct Explore {
    /// True while the explore pages replace the canvas.
    pub open: bool,
    /// The page shown.
    pub page: Page,
    /// The search text (P3-503).
    pub query: String,
    /// The table's sort column and direction, ascending when true.
    pub sort: (Column, bool),
    /// The selected element (P3-504).
    pub selected: Option<ElementId>,
    /// Expanded element rows (P3-501).
    pub expanded: std::collections::BTreeSet<ElementId>,
    /// The graph page, once opened.
    pub graph: Option<GraphState>,
    /// The tree page's camera (P3-512).
    pub tree_camera: Camera,
    /// The selected documentation section (index).
    pub section: usize,
    /// The selected decision (index).
    pub decision: usize,
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
    no_view: std::collections::BTreeSet<ElementId>,
    /// The documentation sections.
    sections: Vec<docs::Section>,
    /// The decisions.
    decisions: Vec<docs::DecisionFile>,
}

impl Explore {
    /// Brings the derived data up to date with `parsed` (`Arc::ptr_eq`);
    /// a new parse clears the selection and the graph page.
    pub fn sync(&mut self, parsed: &Arc<Parsed>) {
        let _ = parsed;
        todo!()
    }

    /// The model page's rows: custom elements, people, systems with
    /// their containers and components, then one section per
    /// environment nested by node; only rows matching the search (and
    /// their ancestors) (P3-500, P3-503).
    #[must_use]
    pub fn model_rows(&self, model: &Model) -> Vec<ModelRow> {
        let _ = model;
        todo!()
    }

    /// The relationships under `element`'s expanded row: outgoing, then
    /// incoming, implied ones marked (P3-501).
    #[must_use]
    pub fn relationship_lines(&self, model: &Model, element: ElementId) -> Vec<RelationshipLine> {
        let _ = (model, element);
        todo!()
    }

    /// The relationships table: matching rows sorted by `sort` (P3-502,
    /// P3-503).
    #[must_use]
    pub fn table(&self, model: &Model) -> Vec<TableRow> {
        let _ = model;
        todo!()
    }

    /// The selected element's usages (P3-504).
    #[must_use]
    pub fn usages(&self, model: &Model) -> Vec<Usage> {
        let _ = model;
        todo!()
    }

    /// The pages the explore menu offers for `model`: Model,
    /// Relationships, Documentation, Decisions, then Graph and Tree per
    /// view where offered (P3-9, P3-511).
    #[must_use]
    pub fn pages(model: &Model) -> Vec<Page> {
        let _ = model;
        todo!()
    }

    /// Opens the graph of `key`: plans the view and lays it out (P3-508).
    pub fn open_graph(&mut self, model: &Model, key: &str) {
        let _ = (model, key);
        todo!()
    }

    /// A click at `at` (screen px over the graph): focuses the node
    /// under it, or clears the focus on empty space (P3-509).
    pub fn graph_click(&mut self, at: ScreenPoint) {
        let _ = at;
        todo!()
    }

    /// The opacity of each node and edge of the graph, 1.0 or 0.1
    /// (P3-509).
    #[must_use]
    pub fn graph_opacity(&self) -> (Vec<f32>, Vec<f32>) {
        todo!()
    }

    /// The tree of `key`, laid out left to right (P3-510).
    #[must_use]
    pub fn tree_spots(&self, model: &Model, key: &str) -> Vec<TreeSpot> {
        let _ = (model, key);
        todo!()
    }

    /// The documentation sections of the last parse (P3-513).
    #[must_use]
    pub fn sections(&self) -> &[docs::Section] {
        todo!()
    }

    /// The decisions of the last parse, sorted by id (P3-516).
    #[must_use]
    pub fn decisions(&self) -> &[docs::DecisionFile] {
        todo!()
    }

    /// Draws the page and returns the actions its clicks make.
    pub fn show(&mut self, ui: &mut egui::Ui, model: &Model, embeds: &dyn Fn(&str) -> Option<Arc<c4rizer_core::pipeline::ViewBuild>>) -> Vec<Action> {
        let _ = (ui, model, embeds);
        todo!()
    }
}

/// `<path>:<line>:<column>` of the declaration at `span` (P3-506).
#[must_use]
pub fn reveal_text(parsed: &Parsed, span: Span) -> String {
    let _ = (parsed, span);
    todo!()
}

/// The folder an attachment path written in file `span.file` resolves
/// against.
#[must_use]
pub fn folder_of(parsed: &Parsed, span: Span) -> Option<PathBuf> {
    let _ = (parsed, span);
    todo!()
}

/// The tree's nodes laid out left to right: depth on x, leaves on
/// consecutive rows, parents centred on their children (P3-510).
#[must_use]
pub fn lay_out_tree(root: &TreeNode) -> Vec<TreeSpot> {
    let _ = root;
    todo!()
}
