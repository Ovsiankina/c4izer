//! Tests of the map-tree row (SRS 4.9: P5-700..P5-705): the view map's
//! tree list, drawn instead of the node graph when the graph cannot fit
//! its section, and its benchmarks. One `#[test]` per requirement, named
//! after it. The map is drawn in a windowless egui context, as
//! `view_map_tests` does. Test data uses roles, never personal names.

use std::sync::Arc;

use c4rizer_core::model::{Parsed, ViewKind, parse};
use eframe::egui;

use crate::testutil::{bench, many_views};
use crate::view_folders_tests::{ESTATE, Shell, declared};
use crate::view_map::{MapNode, ViewMap, ViewMapPanel, tree_rows};
use crate::worker::ViewRef;

fn parsed(text: &str) -> Arc<Parsed> {
    Arc::new(parse(text).expect("fixture parses"))
}

/// Draws the map section in a window `size` px big, with `events` as the
/// frame's input; returns the view a click picked.
#[expect(clippy::too_many_arguments, reason = "one frame's whole input, spelled out at each call")]
fn frame(
    ctx: &egui::Context,
    panel: &mut ViewMapPanel,
    parsed: &Arc<Parsed>,
    current: Option<&ViewRef>,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> Option<ViewRef> {
    let mut picked = None;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        picked = panel.show(ui, parsed, current);
    });
    output.textures_delta.clear();
    picked
}

fn press(at: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn wheel(delta: egui::Vec2) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: egui::Modifiers::NONE,
    }
}

/// The tree of `map` as text, one line per row: prefix then key.
fn listing(map: &ViewMap) -> Vec<String> {
    map.tree.iter().map(|r| format!("{}{}", r.prefix, map.nodes[r.node].key)).collect()
}

/// A node at `level`, keyed `key`, for the pure parent rule.
fn node(key: &str, level: u8) -> MapNode {
    MapNode {
        view: declared(key),
        key: key.to_owned(),
        title: key.to_owned(),
        kind: ViewKind::Container,
        tag: String::new(),
        level,
        row: 0,
        column: 0.0,
    }
}

// ------------------------------------------------------------ the tests

/// P5-700: the tree replaces the graph exactly when the graph cannot fit
/// the section at its floor width, and the graph comes back when it can.
#[test]
fn p5_700_tree_only_when_the_graph_cannot_fit() {
    let ctx = egui::Context::default();
    let p = parsed(ESTATE);
    let nodes = ViewMap::build(&p.model).nodes.len();
    let mut panel = ViewMapPanel::default();
    let wide = egui::vec2(800.0, 600.0);
    frame(&ctx, &mut panel, &p, None, wide, Vec::new());
    assert!(!panel.tree, "800 px: the graph fits");
    assert_eq!(panel.drawn.len(), nodes);
    let narrow = egui::vec2(220.0, 600.0);
    frame(&ctx, &mut panel, &p, None, narrow, Vec::new());
    assert!(panel.tree, "220 px: four Other views side by side cannot fit");
    assert_eq!(panel.drawn.len(), nodes, "one tree row per view");
    assert!(panel.drawn.iter().all(|d| (d.rect.height() - 20.0).abs() < 0.5), "rows are 20 px");
    frame(&ctx, &mut panel, &p, None, wide, Vec::new());
    assert!(!panel.tree, "wide again: the graph comes back");
    // A map that fits 220 px once its nodes shrink stays a graph.
    let two_columns = parsed(
        "workspace {\n    model {\n        a = softwareSystem \"A\" {\n            w = container \"W\"\n        }\n        b = softwareSystem \"B\"\n        a -> b \"Uses\"\n    }\n    views {\n        systemContext a \"a-context\" {\n            include *\n        }\n        container a \"a-containers\" {\n            include *\n        }\n        systemContext b \"b-context\" {\n            include *\n        }\n    }\n}\n",
    );
    let mut panel = ViewMapPanel::default();
    frame(&ctx, &mut panel, &two_columns, None, narrow, Vec::new());
    assert!(!panel.tree, "fits: no tree");
}

/// P5-701: one view per line, `tree` prefixes, then the key and the
/// title.
#[test]
fn p5_701_tree_lines_read_like_tree() {
    let map = ViewMap::build(&parsed(ESTATE).model);
    assert_eq!(
        listing(&map),
        // `web-components` is entered from `checkout` (C2 · dynamic) and
        // from `shop-containers`: same level, `checkout` comes first in
        // the map (P5-702).
        [
            "checkout",
            "└─ web-components",
            "   └─ cart-code",
            "estate",
            "└─ shop-context",
            "   └─ shop-containers",
            "ledger-front",
            "shop-picture",
            "live",
            "board",
        ],
        "{:#?}",
        listing(&map)
    );
    let map = ViewMap::build(&parsed(&many_views(10)).model);
    // `con1` is entered from `ctx0` (its system's neighbour) and `ctx1`:
    // `ctx0` first; `ctx1` is left a root with no child.
    assert_eq!(
        listing(&map),
        [
            "ctx0",
            "├─ con0",
            "│  ├─ cmp0a",
            "│  ├─ cmp0b",
            "│  └─ cmp0c",
            "└─ con1",
            "   ├─ cmp1a",
            "   ├─ cmp1b",
            "   └─ cmp1c",
            "ctx1",
        ],
        "{:#?}",
        listing(&map)
    );
    // Drawn: prefix, key and title, none past the row.
    let ctx = egui::Context::default();
    let mut panel = ViewMapPanel::default();
    let p = parsed(ESTATE);
    let size = egui::vec2(220.0, 600.0);
    let mut texts: Vec<(String, f32)> = Vec::new();
    for _ in 0..2 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            panel.show(ui, &p, None);
        });
        output.textures_delta.clear();
        texts = output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.visual_bounding_rect().max.x)),
                _ => None,
            })
            .collect();
    }
    assert!(texts.iter().any(|(t, _)| t == "   └─ "), "{texts:?}");
    assert!(texts.iter().any(|(t, _)| t == "shop-containers"), "{texts:?}");
    assert!(texts.iter().any(|(t, _)| t.starts_with("The ")), "titles follow the keys: {texts:?}");
    for (t, right) in &texts {
        assert!(*right <= size.x + 0.5, "{t:?} runs to {right}");
    }
}

/// P5-702: a view's parent is the node it is entered from; with
/// several, the lowest level, then map order; no parent, a root.
#[test]
fn p5_702_first_parent_by_level_then_map_order() {
    let nodes = [node("a", 1), node("b", 1), node("c", 2), node("d", 3), node("e", 0)];
    // `c` from `b` and `a` (same level: `a` comes first in the map);
    // `d` from `c` (level 2) and `e` (level 0: lowest wins).
    let rows = tree_rows(&nodes, &[(1, 2), (0, 2), (2, 3), (4, 3)]);
    let lines: Vec<String> = rows.iter().map(|r| format!("{}{}", r.prefix, nodes[r.node].key)).collect();
    assert_eq!(lines, ["a", "└─ c", "b", "e", "└─ d"]);
    // Siblings in map order, the last one `└─`.
    let nodes = [node("r", 0), node("x", 1), node("y", 1), node("z", 2)];
    let rows = tree_rows(&nodes, &[(0, 2), (0, 1), (1, 3)]);
    let lines: Vec<String> = rows.iter().map(|r| format!("{}{}", r.prefix, nodes[r.node].key)).collect();
    assert_eq!(lines, ["r", "├─ x", "│  └─ z", "└─ y"]);
}

/// P5-703: the current row is highlighted and scrolled into sight; a
/// click on a row opens its view as a node click does.
#[test]
fn p5_703_current_row_shown_and_click_navigates() {
    let text = many_views(60);
    let mut sh = Shell::with(&text);
    let p = sh.s.doc.as_ref().and_then(|d| d.parsed.clone()).expect("a parse");
    let ctx = egui::Context::default();
    let mut panel = ViewMapPanel::default();
    let size = egui::vec2(220.0, 300.0);
    // The last view, far down the list.
    let last = declared("cmp11c");
    for _ in 0..3 {
        frame(&ctx, &mut panel, &p, Some(&last), size, Vec::new());
    }
    assert!(panel.tree);
    let current: Vec<&crate::view_map::DrawnNode> = panel.drawn.iter().filter(|d| d.current).collect();
    assert_eq!(current.len(), 1, "the current row is drawn");
    assert_eq!(current[0].view, last);
    assert!(current[0].rect.min.y >= 0.0 && current[0].rect.max.y <= size.y, "in sight: {:?}", current[0].rect);
    // A click on another row picks its view; the shell opens it.
    // A row wholly in sight, under the header.
    let top = panel.header.max.y;
    let target = panel
        .drawn
        .iter()
        .find(|d| !d.current && d.rect.min.y >= top && d.rect.max.y <= size.y)
        .expect("another row")
        .clone();
    let at = target.rect.center();
    frame(&ctx, &mut panel, &p, Some(&last), size, vec![egui::Event::PointerMoved(at), press(at, true)]);
    let picked = frame(&ctx, &mut panel, &p, Some(&last), size, vec![press(at, false)]).expect("a click picks the row");
    assert_eq!(picked, target.view);
    sh.cmd(crate::state::Command::ShowView(picked.clone()));
    assert_eq!(sh.view(), picked.key());
    // The current row picks nothing.
    let mine = panel.drawn.iter().find(|d| d.current).expect("current").rect.center();
    frame(&ctx, &mut panel, &p, Some(&last), size, vec![egui::Event::PointerMoved(mine), press(mine, true)]);
    assert_eq!(frame(&ctx, &mut panel, &p, Some(&last), size, vec![press(mine, false)]), None);
}

/// P5-704: the tree is never wider than its section and scrolls
/// vertically only.
#[test]
fn p5_704_tree_scrolls_vertically_only() {
    let ctx = egui::Context::default();
    let p = parsed(&many_views(60));
    let mut panel = ViewMapPanel::default();
    let size = egui::vec2(160.0, 200.0);
    for _ in 0..2 {
        frame(&ctx, &mut panel, &p, None, size, Vec::new());
    }
    assert!(panel.tree);
    let (content, area) = panel.sizes;
    assert!(content.x <= area.x, "no sideways room: {content:?} in {area:?}");
    for d in &panel.drawn {
        assert!(d.rect.max.x <= size.x && d.rect.min.x >= 0.0, "{} past the edge: {:?}", d.view.key(), d.rect);
    }
    let before = panel.drawn[0].rect;
    let first_view = panel.drawn[0].view.clone();
    let inside = egui::pos2(80.0, 100.0);
    frame(&ctx, &mut panel, &p, None, size, vec![egui::Event::PointerMoved(inside), wheel(egui::vec2(-80.0, 0.0))]);
    for _ in 0..10 {
        frame(&ctx, &mut panel, &p, None, size, Vec::new());
    }
    let first = panel.drawn.iter().find(|d| d.view == first_view).expect("the same first row").rect;
    assert!((first.min.x - before.min.x).abs() < 1e-3, "a sideways wheel moves nothing: {before:?} -> {first:?}");
    assert!((first.min.y - before.min.y).abs() < 1e-3, "nor up and down: {before:?} -> {first:?}");
    frame(&ctx, &mut panel, &p, None, size, vec![egui::Event::PointerMoved(inside), wheel(egui::vec2(0.0, -80.0))]);
    for _ in 0..10 {
        frame(&ctx, &mut panel, &p, None, size, Vec::new());
    }
    let top = &panel.drawn[0];
    assert!(top.view != first_view || (top.rect.min.y - before.min.y).abs() > 1.0, "a vertical wheel scrolls");
}

/// P5-705: the tree rows are built with the map, once per parse.
#[test]
fn p5_705_tree_built_with_the_map_once_per_parse() {
    let ctx = egui::Context::default();
    let p = parsed(ESTATE);
    let mut panel = ViewMapPanel::default();
    for _ in 0..5 {
        frame(&ctx, &mut panel, &p, None, egui::vec2(220.0, 600.0), Vec::new());
    }
    assert!(panel.tree);
    assert_eq!(panel.builds, 1);
    let map = panel.sync(&p);
    let mut seen: Vec<usize> = map.tree.iter().map(|r| r.node).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..map.nodes.len()).collect::<Vec<_>>(), "every node once");
    assert_eq!(map.tree, tree_rows(&map.nodes, &map.drawn));
}

// ------------------------------------------------------------ benchmarks

/// P5-705, design section 9: the tree rows build, 50 and 500 views.
#[test]
#[ignore = "perf: run with --release -- --ignored perf_p5"]
fn perf_p5_705_tree_rows_build() {
    if cfg!(debug_assertions) {
        eprintln!("perf_p5_705_tree_rows_build: skipped in a debug build");
        return;
    }
    // Budgets: about 4x the medians measured 2026-11-23 (14 us, 163 us).
    for (n, budget) in [(50, 60.0), (500, 700.0)] {
        let map = ViewMap::build(&parsed(&many_views(n)).model);
        assert_eq!(map.nodes.len(), n);
        bench(["P5-705", "map-tree", "tree rows build", &format!("{n} views")], 21, budget, || {
            std::hint::black_box(tree_rows(std::hint::black_box(&map.nodes), &map.drawn));
        });
    }
}

/// P5-705, design section 9: one map panel frame in tree mode, 500
/// views.
#[test]
#[ignore = "perf: run with --release -- --ignored perf_p5"]
fn perf_p5_705_map_tree_frame() {
    if cfg!(debug_assertions) {
        eprintln!("perf_p5_705_map_tree_frame: skipped in a debug build");
        return;
    }
    let p = parsed(&many_views(500));
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let mut panel = ViewMapPanel::default();
    let size = egui::vec2(220.0, 400.0);
    let current = declared("cmp50a");
    for _ in 0..3 {
        frame(&ctx, &mut panel, &p, Some(&current), size, Vec::new());
    }
    assert!(panel.tree);
    // Budget: about 4x the 208 us median measured 2026-11-23.
    bench(["P5-705", "map-tree", "map panel frame, tree mode", "500 views"], 21, 900.0, || {
        frame(&ctx, &mut panel, &p, Some(&current), size, Vec::new());
    });
    assert_eq!(panel.builds, 1, "no rebuild per frame");
}
