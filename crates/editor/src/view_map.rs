//! The view map (SRS 4.8 P4-100..P4-112): a small graph of the
//! workspace's views beside the diagram, one node per view of the view
//! list, rows by C4 level from top to bottom, a line from a view down to
//! each view one of its boxes enters. A click opens the view.
//!
//! Depends on: `state` (`listed_views`, `views_entered`), `worker`
//! (`ViewRef`), `c4rizer-core` (`model`, `view::plan`), `eframe`.

// Shape: `ViewMap::build` is pure (model in, nodes, links and slot
// positions out) and runs once per parse; `ViewMapPanel` keeps the built
// map with the parse it came from (`Arc::ptr_eq`, as `Explore::sync`),
// the panel's open flag, and draws it with egui. Drawing returns the view
// a click picked; the shell sends it as `Command::ShowView`, the view
// list's path.
//
// DECISION: a node shows the view's key, not its title: keys are short
// and unique (`C3-serving`, `C3-startup`), titles are long sentences that
// a node-wide cut leaves unreadable; the full title is the hover text.
// DECISION: positions are slots (column, row) laid out headless; the
// panel turns them into px from the measured text, so tests check the
// order without a window.
// DECISION: an EDIT-15 default view that is not written has no node: the
// map shows the views the file declares, like the view list.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use c4rizer_core::model::{ElementKind, Model, Parsed, ViewKind, ViewSpec};
use eframe::egui;

use crate::icons::{self, Icon};
use crate::state::{listed_views, views_entered};
use crate::theme;
use crate::worker::ViewRef;

/// The row of views with no C4 level (custom, image) (P4-102).
const OTHER_LEVEL: u8 = 4;

/// One node of the map (P4-101).
#[derive(Debug, Clone, PartialEq)]
pub struct MapNode {
    /// The view it opens.
    pub view: ViewRef,
    /// Its key: the node's text.
    pub key: String,
    /// Its drawn title, else its key: the hover text.
    pub title: String,
    /// Its kind.
    pub kind: ViewKind,
    /// The C4 level tag, e.g. `C2` or `C2 · dynamic` (P4-104).
    pub tag: String,
    /// 0 landscape, 1 context, 2 container, 3 component, 4 no level
    /// (P4-102, P4-103).
    pub level: u8,
    /// Its row among the rows that hold a node, top first.
    pub row: usize,
    /// Its column, in node slots from the left (P4-107).
    pub column: f32,
}

impl MapNode {
    /// The node's text lines (P4-104): the key, then the tag; the key
    /// alone when it is the tag (a view keyed `C2` at level C2).
    // DECISION: a key that repeats its tag is drawn once, centred, rather
    // than twice (`C2` over `C2`); the node keeps its size.
    #[must_use]
    pub fn lines(&self) -> Vec<&str> {
        if self.key.eq_ignore_ascii_case(&self.tag) {
            vec![&self.key]
        } else {
            vec![&self.key, &self.tag]
        }
    }
}

/// The map of one parse (P4-101..P4-107).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ViewMap {
    /// One per view of the view list, in view-list order.
    pub nodes: Vec<MapNode>,
    /// Every link from a node down to a node on a lower row, as indexes
    /// into `nodes` (P4-105).
    pub links: Vec<(usize, usize)>,
    /// The links drawn: `links` less those another path already covers
    /// (P4-106).
    pub drawn: Vec<(usize, usize)>,
    /// The tree list drawn when the graph cannot fit (P5-700..P5-702),
    /// one row per node. PENDING SIGN-OFF: new pub field (owner request
    /// 2026-11-23, issue #9).
    pub tree: Vec<TreeRow>,
}

/// One line of the map's tree list (P5-701). PENDING SIGN-OFF: new pub
/// item (owner request 2026-11-23, issue #9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// The node, as an index into `ViewMap::nodes`.
    pub node: usize,
    /// The `├─ ` / `└─ ` / `│  ` prefix, as `tree` draws it; empty for a
    /// root.
    pub prefix: String,
}

/// The tree list of `nodes` (P5-702): a node's parent is the node it is
/// entered from in `drawn` (the reduced links), the lowest level first,
/// then map order, when it has several; nodes with no parent are roots;
/// roots and each node's children in map order. PENDING SIGN-OFF: new
/// pub item.
// DECISION: "map order" is the node order, which is the view list's
// order (P4-101): the same order the graph's rows start from, and stable
// across a drag or a resize of the panel.
#[must_use]
pub fn tree_rows(nodes: &[MapNode], drawn: &[(usize, usize)]) -> Vec<TreeRow> {
    let mut parent: Vec<Option<usize>> = vec![None; nodes.len()];
    for &(a, b) in drawn {
        let better = parent[b].is_none_or(|p| (nodes[a].level, a) < (nodes[p].level, p));
        if better {
            parent[b] = Some(a);
        }
    }
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (b, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            children[*p].push(b);
        }
    }
    let mut out = Vec::with_capacity(nodes.len());
    // Depth first, iterative: (node, its own prefix, its children's).
    let mut stack: Vec<(usize, String, String)> = (0..nodes.len())
        .rev()
        .filter(|&i| parent[i].is_none())
        .map(|i| (i, String::new(), String::new()))
        .collect();
    while let Some((node, prefix, under)) = stack.pop() {
        out.push(TreeRow { node, prefix });
        let kids = &children[node];
        for (k, &child) in kids.iter().enumerate().rev() {
            let last = k + 1 == kids.len();
            let own = format!("{under}{}", if last { "└─ " } else { "├─ " });
            let next = format!("{under}{}", if last { "   " } else { "│  " });
            stack.push((child, own, next));
        }
    }
    out
}

/// The level and tag of `spec` (P4-102..P4-104).
// DECISION: a dynamic view sits on the row of what it shows: no scope
// (people and systems) C1, a system's containers C2, a container's
// components C3; a deployment view on C2, as its boxes are container
// instances; a filtered view on its base's row. Each keeps its kind in
// the tag so it is never mistaken for the plain view of that level.
pub(crate) fn level_of(model: &Model, spec: &ViewSpec) -> (u8, String) {
    let level = |n: u8| format!("C{n}");
    match spec.kind {
        ViewKind::SystemLandscape => (0, "Landscape".to_owned()),
        ViewKind::SystemContext => (1, level(1)),
        ViewKind::Container => (2, level(2)),
        ViewKind::Component => (3, level(3)),
        ViewKind::Filtered => match model.base_view(spec).filter(|b| b.kind != ViewKind::Filtered) {
            Some(base) => {
                let (n, tag) = level_of(model, base);
                (n, format!("{tag} · filtered"))
            }
            None => (OTHER_LEVEL, "filtered".to_owned()),
        },
        ViewKind::Dynamic => {
            let n = match spec.scope.element().map(|e| model.element(e).kind) {
                None => 1,
                Some(ElementKind::SoftwareSystem) => 2,
                Some(ElementKind::Container) => 3,
                Some(_) => return (OTHER_LEVEL, "dynamic".to_owned()),
            };
            (n, format!("{} · dynamic", level(n)))
        }
        ViewKind::Deployment => (2, format!("{} · deployment", level(2))),
        other => (OTHER_LEVEL, other.keyword().to_owned()),
    }
}

/// True when `spec` is drawn (pipeline's P2-2 rule): its kind, and a
/// filtered view's base kind, are drawable.
fn drawn(model: &Model, spec: &ViewSpec) -> bool {
    spec.kind.drawable() && model.base_view(spec).is_none_or(|b| b.kind.drawable())
}

impl ViewMap {
    /// The map of `model`: nodes, links and slot positions (P4-101..P4-107).
    #[must_use]
    pub fn build(model: &Model) -> ViewMap {
        let mut nodes: Vec<MapNode> = Vec::new();
        let mut specs: Vec<&ViewSpec> = Vec::new();
        for entry in listed_views(model, None) {
            let Some(spec) = model.view(&entry.view.key()) else { continue };
            let (level, tag) = level_of(model, spec);
            nodes.push(MapNode {
                key: spec.key.clone(),
                title: entry.label,
                kind: entry.kind,
                view: entry.view,
                tag,
                level,
                row: 0,
                column: 0.0,
            });
            specs.push(spec);
        }
        let index: BTreeMap<&ViewRef, usize> = nodes.iter().enumerate().map(|(i, n)| (&n.view, i)).collect();
        let mut links: BTreeSet<(usize, usize)> = BTreeSet::new();
        for (a, spec) in specs.iter().enumerate() {
            if !drawn(model, spec) {
                continue;
            }
            // P4-105: the context menu's rule, per element drawn in the
            // view (`views_entered`), so the map never disagrees with it.
            let plan = c4rizer_core::view::plan(model, spec);
            let mut seen = BTreeSet::new();
            for b in plan.boxes.iter().filter(|b| seen.insert(b.element)) {
                for target in views_entered(model, Some(spec.kind), b.element) {
                    if let Some(&to) = index.get(&target)
                        && nodes[to].level > nodes[a].level
                    {
                        links.insert((a, to));
                    }
                }
            }
        }
        let links: Vec<(usize, usize)> = links.into_iter().collect();
        let drawn = reduce(nodes.len(), &links);
        lay_out(&mut nodes, &drawn);
        // P5-705: built with the map, once per parse.
        let tree = tree_rows(&nodes, &drawn);
        ViewMap { nodes, links, drawn, tree }
    }
}

/// `links` less every link `a -> b` for which `b` is also reached from
/// `a` through another link first (P4-106). Links only go down a level,
/// so the graph has no cycle.
fn reduce(count: usize, links: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut out_of: Vec<Vec<usize>> = vec![Vec::new(); count];
    for &(a, b) in links {
        out_of[a].push(b);
    }
    // Everything reachable from each node, itself excluded.
    let mut reach: Vec<Option<BTreeSet<usize>>> = vec![None; count];
    links
        .iter()
        .copied()
        .filter(|&(a, b)| {
            !out_of[a]
                .iter()
                .any(|&c| c != b && reach_of(c, &out_of, &mut reach).contains(&b))
        })
        .collect()
}

/// Every node reachable from `n` through `out_of`, `n` excluded,
/// memoised in `reach`.
fn reach_of(n: usize, out_of: &[Vec<usize>], reach: &mut [Option<BTreeSet<usize>>]) -> BTreeSet<usize> {
    if let Some(r) = &reach[n] {
        return r.clone();
    }
    let mut r = BTreeSet::new();
    for &m in &out_of[n] {
        r.insert(m);
        r.extend(reach_of(m, out_of, reach));
    }
    reach[n] = Some(r.clone());
    r
}

/// Rows and columns (P4-102, P4-107): a row per level that holds a node;
/// within a row, the children of one parent side by side, centred under
/// it, groups in the order of their parents, nodes with no parent last,
/// each in view-list order; then each parent centred over its children
/// and the children centred again under the moved parents.
// DECISION: a three-sweep layered layout (down, up, down) with the
// row order fixed by the first sweep: enough to keep crossings low for
// a workspace's few dozen views, and deterministic. A node's parent for
// ordering is its drawn parent with the lowest column.
fn lay_out(nodes: &mut [MapNode], drawn: &[(usize, usize)]) {
    let levels: BTreeSet<u8> = nodes.iter().map(|n| n.level).collect();
    let level_row: BTreeMap<u8, usize> = levels.iter().enumerate().map(|(i, &l)| (l, i)).collect();
    for n in nodes.iter_mut() {
        n.row = level_row[&n.level];
    }
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for &(a, b) in drawn {
        parents[b].push(a);
        children[a].push(b);
    }
    let mut rows: Vec<Vec<usize>> = Vec::with_capacity(levels.len());
    let mut x = vec![0.0_f32; nodes.len()];
    // First sweep: the order of each row, and centred children.
    for r in 0..levels.len() {
        let mut row: Vec<usize> = (0..nodes.len()).filter(|&i| nodes[i].row == r).collect();
        let primary = |i: usize, x: &[f32]| -> Option<usize> {
            parents[i].iter().copied().min_by(|&p, &q| x[p].total_cmp(&x[q]).then(p.cmp(&q)))
        };
        row.sort_by(|&i, &j| {
            let key = |n: usize| primary(n, &x).map_or((1, 0.0, 0), |p| (0, x[p], p));
            let (ki, kj) = (key(i), key(j));
            ki.0.cmp(&kj.0).then(ki.1.total_cmp(&kj.1)).then(ki.2.cmp(&kj.2)).then(i.cmp(&j))
        });
        let desired: Vec<Option<f32>> = centred_under(&row, |i| primary(i, &x), &x);
        sweep(&row, &desired, &mut x);
        rows.push(row);
    }
    // Second sweep, bottom up: parents over their children.
    for row in rows.iter().rev().skip(1) {
        let desired: Vec<Option<f32>> = row
            .iter()
            .map(|&i| {
                let c = &children[i];
                #[expect(clippy::cast_precision_loss, reason = "a few dozen views")]
                let mean = (!c.is_empty()).then(|| c.iter().map(|&k| x[k]).sum::<f32>() / c.len() as f32);
                Some(mean.unwrap_or(x[i]))
            })
            .collect();
        sweep(row, &desired, &mut x);
    }
    // Third sweep, top down: children under the moved parents.
    for row in rows.iter().skip(1) {
        let primary = |i: usize| parents[i].iter().copied().min_by(|&p, &q| x[p].total_cmp(&x[q]).then(p.cmp(&q)));
        let desired = centred_under(row, primary, &x);
        sweep(row, &desired, &mut x);
    }
    let left = x.iter().copied().fold(f32::INFINITY, f32::min);
    for (n, column) in nodes.iter_mut().zip(x) {
        n.column = column - if left.is_finite() { left } else { 0.0 };
    }
}

/// Wanted columns for `row`: each run of nodes sharing a primary parent
/// centred on it, one slot apart; `None` for a node with no parent.
fn centred_under(row: &[usize], primary: impl Fn(usize) -> Option<usize>, x: &[f32]) -> Vec<Option<f32>> {
    let parent: Vec<Option<usize>> = row.iter().map(|&i| primary(i)).collect();
    let mut out = vec![None; row.len()];
    let mut start = 0;
    while start < row.len() {
        let mut end = start + 1;
        while end < row.len() && parent[end] == parent[start] {
            end += 1;
        }
        if let Some(p) = parent[start] {
            #[expect(clippy::cast_precision_loss, reason = "a few dozen views")]
            let half = (end - start - 1) as f32 / 2.0;
            for (k, slot) in out[start..end].iter_mut().enumerate() {
                #[expect(clippy::cast_precision_loss, reason = "a few dozen views")]
                let at = x[p] + k as f32 - half;
                *slot = Some(at);
            }
        }
        start = end;
    }
    out
}

/// Places `row` left to right at its wanted columns, keeping one slot
/// between neighbours; a node with no wish takes the next slot.
fn sweep(row: &[usize], desired: &[Option<f32>], x: &mut [f32]) {
    let mut prev: Option<f32> = None;
    for (&i, want) in row.iter().zip(desired) {
        x[i] = match (prev, *want) {
            (None, w) => w.unwrap_or(0.0),
            (Some(p), Some(w)) => w.max(p + 1.0),
            (Some(p), None) => p + 1.0,
        };
        prev = Some(x[i]);
    }
}

/// The hover text of `node` (P4-110): the full title, then the key and
/// the kind keyword.
#[must_use]
pub fn hover_text(node: &MapNode) -> String {
    format!("{}\n{} ({})", node.title, node.key, node.kind.keyword())
}

/// One node as last drawn (tests, P4-108..P4-111).
#[derive(Debug, Clone, PartialEq)]
pub struct DrawnNode {
    /// The view.
    pub view: ViewRef,
    /// Its rect, screen px.
    pub rect: egui::Rect,
    /// True when drawn highlighted (the view on screen).
    pub current: bool,
}

/// The view map section of the left panel (P4-100): its open flag, the
/// map of the last parse, and what the last frame drew.
#[derive(Debug, Clone)]
pub struct ViewMapPanel {
    /// Open (P4-100): kept for the session, whatever the view or parse.
    pub open: bool,
    /// The parse the map was built from, and the map.
    map: Option<(Arc<Parsed>, Arc<ViewMap>)>,
    /// How many maps were built this session (P4-112).
    pub builds: usize,
    /// The view on screen at the last frame: a change scrolls its node
    /// into sight.
    last_current: Option<ViewRef>,
    /// Each node as last drawn.
    pub drawn: Vec<DrawnNode>,
    /// The header as last drawn, screen px.
    pub header: egui::Rect,
    /// The map's size and the size of the area showing it, px, as last
    /// drawn (P4-111).
    pub sizes: (egui::Vec2, egui::Vec2),
    /// The font size of the node text as last drawn (P4-111).
    pub font_size: f32,
    /// True when the last frame drew the tree list instead of the graph
    /// (P5-700); `drawn` then holds the tree rows in sight. PENDING
    /// SIGN-OFF: new pub field (owner request 2026-11-23, issue #9).
    pub tree: bool,
}

impl Default for ViewMapPanel {
    fn default() -> Self {
        ViewMapPanel {
            // DECISION: open at start: the owner asked for the map, and
            // one click on the header folds it away.
            open: true,
            map: None,
            builds: 0,
            last_current: None,
            drawn: Vec::new(),
            header: egui::Rect::NOTHING,
            sizes: (egui::Vec2::ZERO, egui::Vec2::ZERO),
            font_size: 0.0,
            tree: false,
        }
    }
}

/// Room between nodes and around the map, px.
const GAP_X: f32 = 10.0;
/// Room between rows, px: the lines run here.
const GAP_Y: f32 = 22.0;
/// Room around the map, px.
const MARGIN: f32 = 6.0;
/// Node text padding, px.
const PAD: f32 = 6.0;
/// Narrowest and widest node, px.
const NODE_W: (f32, f32) = (56.0, 150.0);

impl ViewMapPanel {
    /// Rebuilds the map when `parsed` is not the parse it was built from
    /// (P4-112), and returns it.
    pub fn sync(&mut self, parsed: &Arc<Parsed>) -> Arc<ViewMap> {
        match &self.map {
            Some((p, map)) if Arc::ptr_eq(p, parsed) => Arc::clone(map),
            _ => {
                let map = Arc::new(ViewMap::build(&parsed.model));
                self.builds += 1;
                self.map = Some((Arc::clone(parsed), Arc::clone(&map)));
                map
            }
        }
    }

    /// Draws the section in a bottom strip of the left panel `ui`
    /// (P4-100): the header, and while open the map in a scroll area.
    /// Returns the view a click picked (P4-109).
    pub fn show_in_panel(&mut self, ui: &mut egui::Ui, parsed: &Arc<Parsed>, current: Option<&ViewRef>) -> Option<ViewRef> {
        let full = ui.available_height();
        let mut picked = None;
        // DECISION: two strips with their own ids: the open one is
        // resizable (default 40% of the panel, 25..75%), the folded one
        // is the header's height, so folding never leaves a gap and the
        // view list above always keeps at least a quarter.
        let header_h = theme::SIZE_ROW;
        if self.open {
            egui::Panel::bottom("c4rizer-view-map")
                .frame(egui::Frame::new().fill(theme::BG_PANEL))
                .resizable(true)
                .default_size((full * 0.4).max(header_h * 3.0))
                .size_range((full * 0.25).max(header_h * 2.0)..=(full * 0.75).max(header_h * 2.0))
                .show(ui, |ui| picked = self.show(ui, parsed, current));
        } else {
            egui::Panel::bottom("c4rizer-view-map-folded")
                .frame(egui::Frame::new().fill(theme::BG_PANEL))
                .resizable(false)
                .exact_size(header_h)
                .show(ui, |ui| picked = self.show(ui, parsed, current));
        }
        picked
    }

    /// The header and, while open, the map filling `ui` (P4-100,
    /// P4-108..P4-111). Returns the view a click picked. Redesign
    /// 2026-11-19 (`components/ViewMap`): a Section header; mono keys;
    /// each row's level label in a left gutter, a node showing its tag
    /// only where it differs from its row's; square links in
    /// `line-strong`.
    #[expect(clippy::too_many_lines, reason = "header, measure, then paint")]
    pub fn show(&mut self, ui: &mut egui::Ui, parsed: &Arc<Parsed>, current: Option<&ViewRef>) -> Option<ViewRef> {
        let (hr, header) = ui.allocate_exact_size(egui::vec2(ui.available_width(), theme::SIZE_ROW), egui::Sense::click());
        self.header = hr;
        if header.clicked() {
            self.open = !self.open;
        }
        let chevron = if self.open { Icon::ChevronDown } else { Icon::ChevronRight };
        icons::paint(ui.painter(), chevron, egui::Rect::from_min_size(egui::pos2(hr.min.x + 12.0, hr.center().y - 6.0), egui::Vec2::splat(12.0)), theme::INK_3);
        let label = egui::WidgetText::from(theme::label("View map")).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Small);
        ui.painter().galley(egui::pos2(hr.min.x + 32.0, hr.center().y - label.size().y / 2.0), label, theme::INK_3);
        self.drawn.clear();
        self.tree = false;
        if !self.open {
            return None;
        }
        let map = self.sync(parsed);
        if map.nodes.is_empty() {
            ui.label(egui::RichText::new("no views").color(theme::INK_3));
            return None;
        }
        let moved = self.last_current.as_ref() != current;
        self.last_current = current.cloned();
        let font = egui::FontId::monospace(10.0);
        self.font_size = font.size;
        let measure = |ui: &egui::Ui, text: &str| {
            ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE).size())
        };
        let line_h = measure(ui, "Hg").y;
        let rows = map.nodes.iter().map(|n| n.row).max().unwrap_or(0) + 1;
        // Each row's level tag: the tag of its first node.
        let row_tag: Vec<String> = (0..rows)
            .map(|r| map.nodes.iter().find(|n| n.row == r).map(|n| n.tag.clone()).unwrap_or_default())
            .collect();
        // A node's second line (`MapNode::lines`), unless its row's gutter
        // already says it.
        let inner_tag = |n: &MapNode| -> Option<String> {
            n.lines()
                .get(1)
                .filter(|tag| !tag.eq_ignore_ascii_case(&row_tag[n.row]))
                .map(|tag| (*tag).to_owned())
        };
        let two_lines = map.nodes.iter().any(|n| inner_tag(n).is_some());
        // DECISION: node widths come from what a node draws, its key and
        // its inner tag; a tag the gutter says is no longer in the node,
        // so it neither widens nor floors it (P4-111 "down to the widest
        // tag" is the widest tag a node shows).
        let tag_w = map.nodes.iter().filter_map(|n| inner_tag(n).map(|t| measure(ui, &t).x)).fold(0.0_f32, f32::max);
        let text_w = map.nodes.iter().map(|n| measure(ui, &n.key).x).fold(tag_w, f32::max);
        let columns = map.nodes.iter().map(|n| n.column).fold(0.0_f32, f32::max) + 1.0;
        // Each row's gutter label, measured in the font it is painted in.
        let gutter_font = egui::FontId::monospace(9.0);
        let row_label: Vec<&str> = (0..rows)
            .map(|r| map.nodes.iter().find(|n| n.row == r).map_or("", |n| level_label(n.level)))
            .collect();
        let gutter = row_label
            .iter()
            .map(|t| ui.fonts_mut(|f| f.layout_no_wrap((*t).to_owned(), gutter_font.clone(), egui::Color32::WHITE).size().x))
            .fold(0.0_f32, f32::max)
            + 12.0
            + 8.0;
        // P4-111: solid scroll bars, so a cut map always shows its bar;
        // the vertical bar's room is kept whether shown or not.
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        let room = ui.available_width() - ui.spacing().scroll.allocated_width() - gutter;
        let lines_in = if two_lines { 2.0 } else { 1.0 };
        let node = egui::vec2(node_width(text_w, tag_w, columns, room), lines_in * line_h + 2.0 * NODE_PAD_Y);
        // P5-700: wider than the room even at the floor width: the tree.
        self.tree = columns * node.x + (columns - 1.0) * GAP_X + 2.0 * MARGIN > room + 0.5;
        if self.tree {
            return self.show_tree(ui, &map, current, (&font, moved));
        }
        #[expect(clippy::cast_precision_loss, reason = "a few rows")]
        let content = egui::vec2(
            gutter + columns * node.x + (columns - 1.0) * GAP_X + 2.0 * MARGIN,
            rows as f32 * node.y + (rows - 1) as f32 * GAP_Y + 2.0 * MARGIN,
        );
        let area = ui.available_size();
        self.sizes = (content, area);
        let mut picked = None;
        let mut drawn = Vec::new();
        egui::ScrollArea::both()
            .id_salt("c4rizer-view-map-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let size = egui::vec2(content.x.max(ui.available_width()), content.y);
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                // A map narrower than the area is centred in what the
                // gutter leaves.
                let origin = rect.min + egui::vec2(gutter + (size.x - content.x) / 2.0 + MARGIN, MARGIN);
                #[expect(clippy::cast_precision_loss, reason = "a few rows")]
                let place = |n: &MapNode| {
                    let min = origin + egui::vec2(n.column * (node.x + GAP_X), n.row as f32 * (node.y + GAP_Y));
                    egui::Rect::from_min_size(min, node)
                };
                let painter = ui.painter().clone();
                for (r, label) in row_label.iter().enumerate() {
                    #[expect(clippy::cast_precision_loss, reason = "a few rows")]
                    let y = origin.y + r as f32 * (node.y + GAP_Y) + node.y / 2.0;
                    painter.text(
                        egui::pos2(rect.min.x + 12.0, y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        gutter_font.clone(),
                        theme::INK_3,
                    );
                }
                let line = egui::Stroke::new(1.0, theme::LINE_STRONG);
                for &(a, b) in &map.drawn {
                    let (from, to) = (place(&map.nodes[a]).center_bottom(), place(&map.nodes[b]).center_top());
                    let mid = f32::midpoint(from.y, to.y).round() + 0.5;
                    painter.add(egui::Shape::line(
                        vec![from, egui::pos2(from.x, mid), egui::pos2(to.x, mid), to],
                        line,
                    ));
                }
                for (i, n) in map.nodes.iter().enumerate() {
                    let r = place(n);
                    let is_current = current == Some(&n.view);
                    let response = ui
                        .interact(r, ui.scope_id().with(("c4rizer-view-map-node", i)), egui::Sense::click())
                        .on_hover_text(hover_text(n))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    paint_node(ui, r, (&n.key, inner_tag(n).as_deref()), &font, (is_current, response.hovered()));
                    if is_current && moved {
                        ui.scroll_to_rect(r.expand(MARGIN), None);
                    }
                    if response.clicked() && !is_current {
                        picked = Some(n.view.clone());
                    }
                    drawn.push(DrawnNode { view: n.view.clone(), rect: r, current: is_current });
                }
            });
        self.drawn = drawn;
        picked
    }

    /// The tree list in place of the graph (P5-700..P5-704): one view per
    /// `TREE_ROW` line, `tree` prefixes in `ink-3`, the key, the title cut
    /// to the room; the current row in `accent-soft` with its key in
    /// `accent`, scrolled into sight when `moved`; a hovered row in
    /// `bg-active` with the node's hover text. Scrolls vertically only and
    /// lays out the rows in sight. Returns the view a click picked.
    fn show_tree(&mut self, ui: &mut egui::Ui, map: &ViewMap, current: Option<&ViewRef>, (font, moved): (&egui::FontId, bool)) -> Option<ViewRef> {
        let count = map.tree.len();
        #[expect(clippy::cast_precision_loss, reason = "a few hundred views")]
        let height = count as f32 * TREE_ROW;
        let area = ui.available_size();
        // Never wider than the section: no sideways scroll (P5-704).
        self.sizes = (egui::vec2(area.x - ui.spacing().scroll.allocated_width(), height), area);
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("c4rizer-view-map-tree")
            .auto_shrink([false, false]);
        if moved && let Some(at) = map.tree.iter().position(|r| current == Some(&map.nodes[r.node].view)) {
            #[expect(clippy::cast_precision_loss, reason = "a few hundred views")]
            let top = at as f32 * TREE_ROW;
            scroll = scroll.vertical_scroll_offset((top - (area.y - TREE_ROW) / 2.0).clamp(0.0, (height - area.y).max(0.0)));
        }
        let mut picked = None;
        let mut drawn = Vec::new();
        scroll.show_rows(ui, TREE_ROW, count, |ui, range| {
            for row in &map.tree[range] {
                let n = &map.nodes[row.node];
                let is_current = current == Some(&n.view);
                let (r, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), TREE_ROW), egui::Sense::click());
                let painter = ui.painter();
                if is_current {
                    painter.rect_filled(r, theme::RADIUS_1, theme::ACCENT_SOFT);
                } else if response.hovered() {
                    painter.rect_filled(r, theme::RADIUS_1, theme::BG_ACTIVE);
                }
                let right = r.max.x - MARGIN;
                let mut x = r.min.x + MARGIN;
                let width = |t: &str| ui.fonts_mut(|f| f.layout_no_wrap(t.to_owned(), font.clone(), egui::Color32::WHITE).size().x);
                if !row.prefix.is_empty() {
                    x = painter.text(egui::pos2(x, r.center().y), egui::Align2::LEFT_CENTER, &row.prefix, font.clone(), theme::INK_3).max.x;
                }
                let key_ink = if is_current {
                    theme::ACCENT
                } else if response.hovered() {
                    theme::INK
                } else {
                    theme::INK_2
                };
                let key = fit_text(&n.key, right - x, width);
                x = painter.text(egui::pos2(x, r.center().y), egui::Align2::LEFT_CENTER, &key, font.clone(), key_ink).max.x + PAD;
                if n.title != n.key && right - x > 0.0 {
                    let title = fit_text(&n.title, right - x, width);
                    if width(&title) <= right - x {
                        painter.text(egui::pos2(x, r.center().y), egui::Align2::LEFT_CENTER, &title, font.clone(), theme::INK_3);
                    }
                }
                let response = if response.hovered() {
                    response.on_hover_text(hover_text(n)).on_hover_cursor(egui::CursorIcon::PointingHand)
                } else {
                    response
                };
                if response.clicked() && !is_current {
                    picked = Some(n.view.clone());
                }
                drawn.push(DrawnNode { view: n.view.clone(), rect: r, current: is_current });
            }
        });
        self.drawn = drawn;
        picked
    }
}

/// One line of the tree list, px (P5-701).
const TREE_ROW: f32 = 20.0;

/// The gutter label of a row at `level` (redesign 2026-11-19,
/// `components/ViewMap`): `LS`, `C1`..`C3`, `—` for views with no level.
// DECISION: the design pack's short labels rather than the row's first
// tag: `Landscape` or `C2 · deployment` in the gutter would take the
// width the nodes need to fit the panel (P4-111).
fn level_label(level: u8) -> &'static str {
    match level {
        0 => "LS",
        1 => "C1",
        2 => "C2",
        3 => "C3",
        _ => "—",
    }
}

/// Node text padding above and below, px.
const NODE_PAD_Y: f32 = 3.0;

/// Paints a node in `r` with its `key`, and `tag` under it when the row's
/// gutter does not already say it (P4-104); `(current, hovered)` pick the
/// colours (P4-108).
fn paint_node(ui: &egui::Ui, r: egui::Rect, (key, tag): (&str, Option<&str>), font: &egui::FontId, (current, hovered): (bool, bool)) {
    let painter = ui.painter();
    let (fill, stroke, text) = if current {
        (theme::ACCENT_SOFT, theme::ACCENT, theme::ACCENT)
    } else if hovered {
        (theme::BG_ACTIVE, theme::LINE_STRONG, theme::INK)
    } else {
        (theme::BG_RAISED, theme::LINE_2, theme::INK_2)
    };
    painter.rect(r, theme::RADIUS_1, fill, egui::Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    let room = r.width() - 2.0 * PAD;
    let fit = |t: &str| {
        fit_text(t, room, |t| {
            ui.fonts_mut(|f| f.layout_no_wrap(t.to_owned(), font.clone(), egui::Color32::WHITE).size().x)
        })
    };
    match tag {
        Some(tag) => {
            painter.text(r.center_top() + egui::vec2(0.0, NODE_PAD_Y), egui::Align2::CENTER_TOP, fit(key), font.clone(), text);
            painter.text(r.center_bottom() - egui::vec2(0.0, NODE_PAD_Y), egui::Align2::CENTER_BOTTOM, fit(tag), font.clone(), theme::INK_3);
        }
        None => {
            painter.text(r.center(), egui::Align2::CENTER_CENTER, fit(key), font.clone(), text);
        }
    }
}

/// The node width, px (P4-111): wide enough for the widest key and tag
/// (`NODE_W` bounds), shrunk evenly so `columns` nodes fit `room`, but
/// never under the widest tag (tags stay whole; keys take an ellipsis);
/// below that the map scrolls sideways.
// DECISION: the readable minimum is the widest tag plus padding (at
// least `NODE_W.0`): a tag is a few characters and must stay whole,
// while a key cut to `shop-co…` still reads, with the full name on hover.
#[must_use]
pub fn node_width(widest_text: f32, widest_tag: f32, columns: f32, room: f32) -> f32 {
    let natural = (widest_text + 2.0 * PAD).clamp(NODE_W.0, NODE_W.1);
    let minimum = (widest_tag + 2.0 * PAD).max(NODE_W.0).min(natural);
    let fits = (room - 2.0 * MARGIN - (columns - 1.0) * GAP_X) / columns;
    natural.min(fits).max(minimum)
}


/// `text` cut with an ellipsis to fit `room` px as `width` measures it;
/// one character is always kept.
fn fit_text(text: &str, room: f32, width: impl Fn(&str) -> f32) -> String {
    if width(text) <= room {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    (1..chars.len())
        .rev()
        .map(|n| chars[..n].iter().collect::<String>() + "…")
        .find(|t| width(t) <= room)
        .unwrap_or_else(|| chars.first().map(|c| format!("{c}…")).unwrap_or_default())
}
