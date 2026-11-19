//! Tests of the view-map row (SRS 4.8: P4-100..P4-116): the view map in
//! the left panel and the view history with Back and Forward. One
//! `#[test]` per requirement, named after it. The map is checked headless
//! (`ViewMap::build`) and drawn in a windowless egui context; the history
//! is driven like `state`'s own tests, effects run synchronously with
//! `files` and `worker::serve`. Test data uses roles, never personal
//! names.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use c4rizer_core::model::{Model, Parsed, parse};
use c4rizer_core::scene::Scene;
use eframe::egui;

use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState};
use crate::state::{Button, Command, Effect, Event, Input, Key, MenuAction, Modifiers, Overlay, State};
use crate::testutil::TempDir;
use crate::view_map::{ViewMap, ViewMapPanel, hover_text};
use crate::worker::{self, ViewRef};

/// An estate of every view kind, roles only: a landscape, two context
/// views, a container view, a filtered view on the ledger's container
/// view (which leaves that base out of the view list, P2-314), a
/// component view, a dynamic view of the shop and an unscoped one, a
/// deployment view, a custom view of a custom element; `bare` has containers but no view, so
/// it enters an EDIT-15 default view.
const ESTATE: &str = r#"workspace "Estate" {
    model {
        staff = person "Operator" "Runs the shop."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust" "Front" {
                cart = component "Cart" "Holds items."
            }
            db = container "Db" "Stores orders." "SQL"
        }
        ledger = softwareSystem "Ledger" "Keeps the books." {
            books = container "Books" "Stores entries." "SQL" "Front"
        }
        bare = softwareSystem "Bare" "Has parts and no view." {
            part = container "Part" "A part."
        }
        kiosk = element "Kiosk" "Hardware" "Shows prices."
        staff -> web "Uses"
        web -> db "Reads"
        cart -> db "Saves"
        web -> books "Posts entries to"
        sales = shop -> ledger "Posts sales to"
        live = deploymentEnvironment "Live" {
            node = deploymentNode "Node" {
                containerInstance web
            }
        }
    }
    views {
        systemLandscape "estate" "The whole estate" {
            include *
        }
        systemContext shop "shop-context" "The shop and its neighbours" {
            include *
        }
        systemContext ledger "ledger-context" "The ledger and its neighbours" {
            include *
        }
        container shop "shop-containers" "Inside the shop" {
            include *
        }
        container ledger "ledger-containers" "Inside the ledger" {
            include *
        }
        filtered "ledger-containers" include "Front,Relationship" "ledger-front" "The ledger's front"
        component web "web-components" "Inside the web app" {
            include *
        }
        dynamic shop "checkout" "Checkout" {
            staff -> web "Opens the shop"
        }
        dynamic * "overview" "Selling" {
            sales "Posts the day's sales"
        }
        deployment shop live "live" "Live" {
            include *
        }
        custom "board" "Whiteboard" {
            include kiosk
        }
    }
}
"#;

/// The keys of the map's nodes, in view-list order.
const KEYS: [&str; 10] = [
    "estate",
    "shop-context",
    "ledger-context",
    "shop-containers",
    "ledger-front",
    "web-components",
    "checkout",
    "overview",
    "live",
    "board",
];

fn declared(key: &str) -> ViewRef {
    ViewRef::Declared(key.to_owned())
}

fn parsed(text: &str) -> Arc<Parsed> {
    Arc::new(parse(text).expect("fixture parses"))
}

fn map() -> ViewMap {
    ViewMap::build(&parsed(ESTATE).model)
}

/// The index of the node keyed `key`.
fn at(map: &ViewMap, key: &str) -> usize {
    map.nodes.iter().position(|n| n.key == key).unwrap_or_else(|| panic!("no node {key}"))
}

/// The links of `links` as key pairs.
fn pairs(map: &ViewMap, links: &[(usize, usize)]) -> Vec<(String, String)> {
    links.iter().map(|&(a, b)| (map.nodes[a].key.clone(), map.nodes[b].key.clone())).collect()
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_owned(), b.to_owned())
}

// ------------------------------------------------------------ the shell

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    dir: TempDir,
}

impl Shell {
    fn with(text: &str) -> Shell {
        let dir = TempDir::new("view-map");
        let path = dir.join("estate.dsl");
        std::fs::write(&path, text).expect("fixture written");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
            dir,
        };
        shell.input(Input::CanvasSize {
            size: ScreenPoint { x: 1200.0, y: 800.0 },
        });
        shell.cmd(Command::Open(path));
        shell
    }

    fn event(&mut self, event: Event) {
        let effects = self.s.handle(event);
        self.run(effects);
    }

    fn cmd(&mut self, command: Command) {
        self.event(Event::Command(command));
    }

    fn input(&mut self, input: Input) {
        self.event(Event::Input(input));
    }

    fn key(&mut self, key: Key, mods: Modifiers) {
        self.input(Input::KeyDown {
            key,
            mods,
            text_focus: false,
        });
        self.input(Input::KeyUp { key });
    }

    fn forward(&mut self) {
        let effects = self.s.forward();
        self.run(effects);
    }

    fn run(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            let next = match effect {
                Effect::Read(path) => {
                    let (text, hash) = files::read(&path).expect("read fixture");
                    self.s.handle(Event::File(FileEvent::Loaded { path, text, hash }))
                }
                Effect::Write(request) => self.s.handle(Event::File(files::write_checked(&request))),
                Effect::Work(request) => match worker::serve(request, &AtomicBool::new(false)) {
                    Some(reply) => self.s.handle(Event::Worker(reply)),
                    None => Vec::new(),
                },
                Effect::Watch(_)
                | Effect::Dialog(_)
                | Effect::SaveUiState(_)
                | Effect::Fullscreen(_)
                | Effect::OpenUrl(_) => Vec::new(),
            };
            self.run(next);
        }
    }

    /// The file changed on disk to `text` (EDIT-1).
    fn reload(&mut self, text: &str) {
        std::fs::write(&self.path, text).expect("fixture rewritten");
        let (text, hash) = files::read(&self.path).expect("read fixture");
        self.event(Event::File(FileEvent::Changed { text, hash }));
    }

    fn model(&self) -> &Model {
        &self.s.doc.as_ref().expect("a file").parsed.as_ref().expect("a parse").model
    }

    fn scene(&self) -> &Scene {
        self.s.scene().expect("a scene")
    }

    fn show(&mut self, key: &str) {
        self.cmd(Command::ShowView(declared(key)));
        assert_eq!(self.s.view, Some(declared(key)));
    }

    fn view(&self) -> String {
        self.s.view.as_ref().map(ViewRef::key).unwrap_or_default()
    }

    /// The views the context menu of the box at `at` enters, in menu
    /// order (editor-ux-02); empty when no menu opens.
    fn menu_targets(&mut self, at: ScreenPoint) -> Vec<ViewRef> {
        self.s.overlay = None;
        self.input(Input::PointerDown {
            at,
            button: Button::Right,
            mods: Modifiers::default(),
        });
        self.input(Input::PointerUp { at, button: Button::Right });
        let out = match &self.s.overlay {
            Some(Overlay::ContextMenu { entries }) => entries
                .iter()
                .filter_map(|e| match &e.action {
                    MenuAction::Show(v) => Some(v.clone()),
                    MenuAction::OpenUrl { .. }
                    | MenuAction::ReleaseAnchor { .. }
                    | MenuAction::GroupBus
                    | MenuAction::UngroupBus { .. }
                    | MenuAction::RemoveFromBus { .. }
                    | MenuAction::FlipBridge { .. } => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        self.s.overlay = None;
        out
    }
}

// ------------------------------------------------------- the egui frame

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

/// A click at `at` over two frames; what the release frame picked.
fn click(
    ctx: &egui::Context,
    panel: &mut ViewMapPanel,
    parsed: &Arc<Parsed>,
    current: Option<&ViewRef>,
    at: egui::Pos2,
) -> Option<ViewRef> {
    let size = egui::vec2(800.0, 600.0);
    frame(ctx, panel, parsed, current, size, vec![egui::Event::PointerMoved(at), press(at, true)]);
    frame(ctx, panel, parsed, current, size, vec![press(at, false)])
}

// ------------------------------------------------------------ the tests

/// P4-100: a click on the header folds the section and another opens
/// it; the state survives view changes and a new parse.
#[test]
fn p4_100_view_map_folds_and_keeps_state() {
    let ctx = egui::Context::default();
    let mut panel = ViewMapPanel::default();
    let first = parsed(ESTATE);
    let size = egui::vec2(800.0, 600.0);
    frame(&ctx, &mut panel, &first, None, size, Vec::new());
    assert!(panel.open, "open at start");
    assert_eq!(panel.drawn.len(), KEYS.len());
    let header = panel.header.center();
    click(&ctx, &mut panel, &first, None, header);
    assert!(!panel.open, "a header click folds it");
    frame(&ctx, &mut panel, &first, Some(&declared("checkout")), size, Vec::new());
    assert!(panel.drawn.is_empty(), "folded: no node drawn");
    let second = parsed(&ESTATE.replace("Runs the shop.", "Runs the estate."));
    frame(&ctx, &mut panel, &second, Some(&declared("live")), size, Vec::new());
    assert!(!panel.open, "a view change and a new parse keep it folded");
    let header = panel.header.center();
    click(&ctx, &mut panel, &second, None, header);
    assert!(panel.open, "another click opens it");
    frame(&ctx, &mut panel, &second, None, size, Vec::new());
    assert_eq!(panel.drawn.len(), KEYS.len());
}

/// P4-101: one node per view of the view list, a filtered view's base
/// and an unwritten default view left out.
#[test]
fn p4_101_one_node_per_listed_view() {
    let m = map();
    let keys: Vec<&str> = m.nodes.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(keys, KEYS);
    let sh = Shell::with(ESTATE);
    let listed: Vec<String> = sh.s.views().iter().map(|e| e.view.key()).collect();
    assert_eq!(listed, KEYS, "the view list's views, in its order");
    assert!(!keys.contains(&"ledger-containers"), "P2-314: the filtered base is not listed");
    assert!(!keys.iter().any(|k| k.starts_with("bare")), "no default view node");
}

/// P4-102: rows by C4 level, top to bottom, the levelless row last.
#[test]
fn p4_102_rows_follow_c4_levels() {
    let m = map();
    let row = |k: &str| m.nodes[at(&m, k)].row;
    assert_eq!(row("estate"), 0, "landscape");
    assert_eq!(row("shop-context"), 1, "C1");
    assert_eq!(row("ledger-context"), 1);
    assert_eq!(row("shop-containers"), 2, "C2");
    assert_eq!(row("web-components"), 3, "C3");
    assert_eq!(row("board"), 4, "custom: no level, last");
    // A row with no node is left out: no landscape, so C1 is the top.
    let small = ViewMap::build(&parsed(crate::testutil::SHOP).model);
    let rows: Vec<(&str, usize)> = small.nodes.iter().map(|n| (n.key.as_str(), n.row)).collect();
    assert_eq!(rows, [("context", 0), ("containers", 1)]);
}

/// P4-103: filtered views on their base's row, dynamic views on the row
/// of what they show, deployment views on C2.
#[test]
fn p4_103_other_kinds_take_a_row() {
    let m = map();
    let level = |k: &str| m.nodes[at(&m, k)].level;
    assert_eq!(level("ledger-front"), 2, "the base is a container view");
    assert_eq!(level("checkout"), 2, "a system's dynamic view shows containers");
    assert_eq!(level("overview"), 1, "an unscoped dynamic view shows systems");
    assert_eq!(level("live"), 2, "deployment");
    let text = ESTATE.replace(
        "custom \"board\"",
        "dynamic web \"inside\" \"Inside\" {\n            cart -> db \"Saves\"\n        }\n        custom \"board\"",
    );
    let m = ViewMap::build(&parsed(&text).model);
    assert_eq!(m.nodes[at(&m, "inside")].level, 3, "a container's dynamic view shows components");
}

/// P4-104: the key and the level tag, the kind after it for filtered,
/// dynamic and deployment views, the keyword alone with no level; a key
/// equal to its tag once.
#[test]
fn p4_104_nodes_show_key_and_tag() {
    let m = map();
    let lines = |k: &str| m.nodes[at(&m, k)].lines().iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(lines("estate"), ["estate", "Landscape"]);
    assert_eq!(lines("shop-context"), ["shop-context", "C1"]);
    assert_eq!(lines("shop-containers"), ["shop-containers", "C2"]);
    assert_eq!(lines("web-components"), ["web-components", "C3"]);
    assert_eq!(lines("ledger-front"), ["ledger-front", "C2 · filtered"]);
    assert_eq!(lines("checkout"), ["checkout", "C2 · dynamic"]);
    assert_eq!(lines("overview"), ["overview", "C1 · dynamic"]);
    assert_eq!(lines("live"), ["live", "C2 · deployment"]);
    assert_eq!(lines("board"), ["board", "custom"]);
    let example = ViewMap::build(&parsed(include_str!("../../../tests/data/example/workspace.dsl")).model);
    let c2 = example.nodes.iter().find(|n| n.key == "C2").expect("C2");
    assert_eq!(c2.lines(), ["C2"], "a key equal to its tag is shown once");
}

/// P4-105: A links to B exactly when a box of A offers "Enter B" in its
/// context menu and B sits lower; checked against the real menu of every
/// box of every view.
#[test]
fn p4_105_links_follow_the_enter_menu() {
    let m = map();
    let mut sh = Shell::with(ESTATE);
    let mut from_menu = Vec::new();
    for (a, node) in m.nodes.iter().enumerate() {
        sh.show(&node.key);
        let centres: Vec<ScreenPoint> = sh.scene().boxes.iter().map(|b| sh.s.camera.to_screen(b.rect.centre())).collect();
        for c in centres {
            for target in sh.menu_targets(c) {
                if let Some(b) = m.nodes.iter().position(|n| n.view == target)
                    && m.nodes[b].level > node.level
                    && !from_menu.contains(&(a, b))
                {
                    from_menu.push((a, b));
                }
            }
        }
    }
    from_menu.sort_unstable();
    assert_eq!(m.links, from_menu, "the map and the menu agree");
    let links = pairs(&m, &m.links);
    for expected in [
        pair("estate", "shop-context"),
        pair("estate", "ledger-context"),
        pair("estate", "shop-containers"),
        pair("shop-context", "shop-containers"),
        pair("shop-containers", "web-components"),
        pair("checkout", "web-components"),
        pair("overview", "shop-containers"),
    ] {
        assert!(links.contains(&expected), "{expected:?} in {links:?}");
    }
    assert!(!links.iter().any(|(a, b)| a == "board" || b == "board"), "the custom view enters nothing");
}

/// P4-106: a link another path covers is not drawn.
#[test]
fn p4_106_covered_links_are_not_drawn() {
    let m = map();
    let drawn = pairs(&m, &m.drawn);
    let links = pairs(&m, &m.links);
    assert!(links.contains(&pair("estate", "shop-containers")));
    assert!(!drawn.contains(&pair("estate", "shop-containers")), "estate reaches it through shop-context");
    assert!(drawn.contains(&pair("estate", "shop-context")));
    assert!(drawn.contains(&pair("shop-context", "shop-containers")));
    assert!(drawn.iter().all(|l| links.contains(l)));
}

/// P4-107: children side by side centred under their parent, parentless
/// nodes last; a parent centred over its children.
#[test]
fn p4_107_children_sit_under_their_parent() {
    let example = ViewMap::build(&parsed(include_str!("../../../tests/data/example/workspace.dsl")).model);
    let col = |m: &ViewMap, k: &str| m.nodes[at(m, k)].column;
    assert!(
        (col(&example, "C3-serving") + 1.0 - col(&example, "C3-startup")).abs() < 1e-3,
        "side by side, view-list order"
    );
    let mid = f32::midpoint(col(&example, "C3-serving"), col(&example, "C3-startup"));
    assert!((col(&example, "C2") - mid).abs() < 1e-3, "C2 centred over its two children");
    assert!((col(&example, "C1") - col(&example, "C2")).abs() < 1e-3, "one child straight under");
    let m = map();
    let mut row2: Vec<_> = m.nodes.iter().filter(|n| n.row == 2).collect();
    row2.sort_by(|a, b| a.column.total_cmp(&b.column));
    let row2: Vec<&str> = row2.iter().map(|n| n.key.as_str()).collect();
    // Nothing enters a filtered, dynamic or deployment view: they follow
    // shop-containers (under shop-context) in view-list order.
    assert_eq!(row2, ["shop-containers", "ledger-front", "checkout", "live"], "parentless nodes last");
    for row in 0..=4 {
        let mut cols: Vec<f32> = m.nodes.iter().filter(|n| n.row == row).map(|n| n.column).collect();
        cols.sort_by(f32::total_cmp);
        assert!(cols.windows(2).all(|w| w[1] - w[0] >= 1.0 - 1e-3), "no overlap in row {row}: {cols:?}");
    }
}

/// P4-108: the node of the view on screen is drawn highlighted, and
/// only it.
#[test]
fn p4_108_current_view_is_highlighted() {
    let ctx = egui::Context::default();
    let mut panel = ViewMapPanel::default();
    let p = parsed(ESTATE);
    frame(&ctx, &mut panel, &p, Some(&declared("checkout")), egui::vec2(800.0, 600.0), Vec::new());
    let current: Vec<String> = panel.drawn.iter().filter(|d| d.current).map(|d| d.view.key()).collect();
    assert_eq!(current, ["checkout"]);
}

/// P4-109: a click on a node opens its view as the view list does.
#[test]
fn p4_109_click_opens_the_view() {
    let ctx = egui::Context::default();
    let mut panel = ViewMapPanel::default();
    let mut sh = Shell::with(ESTATE);
    let p = sh.s.doc.as_ref().and_then(|d| d.parsed.clone()).expect("a parse");
    let current = sh.s.view.clone();
    frame(&ctx, &mut panel, &p, current.as_ref(), egui::vec2(800.0, 600.0), Vec::new());
    let target = panel.drawn.iter().find(|d| d.view.key() == "web-components").expect("drawn").rect.center();
    let picked = click(&ctx, &mut panel, &p, current.as_ref(), target).expect("a click picks the node");
    assert_eq!(picked, declared("web-components"));
    // The shell sends it as the view list's pick.
    sh.cmd(Command::ShowView(picked));
    assert_eq!(sh.view(), "web-components");
    assert_eq!(sh.scene().key, "web-components");
    // The current node picks nothing.
    let current = sh.s.view.clone();
    assert_eq!(click(&ctx, &mut panel, &p, current.as_ref(), target), None);
}

/// P4-110: the hover text is the view's full title.
#[test]
fn p4_110_hover_shows_full_title() {
    let m = map();
    let sh = Shell::with(ESTATE);
    for entry in sh.s.views() {
        let node = &m.nodes[at(&m, &entry.view.key())];
        assert!(hover_text(node).starts_with(&format!("{}\n", entry.label)), "{}", hover_text(node));
    }
    assert!(hover_text(&m.nodes[at(&m, "shop-context")]).starts_with("The shop and its neighbours\n"));
}

/// P4-111: a map larger than its section scrolls both ways, with the
/// view list's font size.
#[test]
fn p4_111_large_map_scrolls_with_readable_text() {
    let ctx = egui::Context::default();
    let mut panel = ViewMapPanel::default();
    let p = parsed(ESTATE);
    let size = egui::vec2(160.0, 120.0);
    frame(&ctx, &mut panel, &p, None, size, Vec::new());
    let (content, area) = panel.sizes;
    assert!(content.x > area.x && content.y > area.y, "{content:?} over {area:?}");
    // redesign 2026-11-19: the view list draws its keys in
    // `theme::code_font()` (mono 11), no longer in egui's Button style, and
    // the design pack's map keys are mono 10 (`.c4-map__key`): the map
    // text is the view list's key size or at most 1 px under it.
    let list = crate::theme::code_font().size;
    assert!(
        panel.font_size >= list - 1.0 && panel.font_size <= list,
        "the view list's font size: {} against {list}",
        panel.font_size
    );
    let before = panel.drawn[0].rect;
    let inside = egui::pos2(80.0, 80.0);
    frame(
        &ctx,
        &mut panel,
        &p,
        None,
        size,
        vec![
            egui::Event::PointerMoved(inside),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-60.0, -60.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    for _ in 0..30 {
        frame(&ctx, &mut panel, &p, None, size, Vec::new());
    }
    let after = panel.drawn[0].rect;
    assert!(after.min.x < before.min.x && after.min.y < before.min.y, "scrolled both ways: {before:?} -> {after:?}");
    assert!((after.size() - before.size()).length() < 1e-3, "scrolling never scales the nodes");
}

/// P4-111: at the default 220 px panel the nodes shrink to fit its
/// width, keys cut and tags whole; below the tag minimum they scroll.
#[test]
fn p4_111_map_fits_the_panel_width() {
    let ctx = egui::Context::default();
    let size = egui::vec2(220.0, 600.0);
    // Two columns of long keys (`shop-context`, `ledger-context`): shrunk to fit, nothing cut by the edge.
    let mut text = ESTATE.to_owned();
    for cut in [
        "        filtered \"ledger-containers\" include \"Front,Relationship\" \"ledger-front\" \"The ledger's front\"\n",
        "        dynamic shop \"checkout\" \"Checkout\" {\n            staff -> web \"Opens the shop\"\n        }\n",
        "        deployment shop live \"live\" \"Live\" {\n            include *\n        }\n",
        "        dynamic * \"overview\" \"Selling\" {\n            sales \"Posts the day's sales\"\n        }\n",
    ] {
        assert!(text.contains(cut), "{cut}");
        text = text.replace(cut, "");
    }
    let narrow = parsed(&text);
    let mut panel = ViewMapPanel::default();
    frame(&ctx, &mut panel, &narrow, None, size, Vec::new());
    let (content, _) = panel.sizes;
    assert!(content.x <= size.x, "no sideways scroll: {content:?}");
    for d in &panel.drawn {
        assert!(d.rect.max.x <= size.x && d.rect.min.x >= 0.0, "{} cut by the edge: {:?}", d.view.key(), d.rect);
    }
    // Four nodes in a row with `C2 · deployment`: floored at the tag,
    // so the map scrolls sideways instead.
    let mut panel = ViewMapPanel::default();
    frame(&ctx, &mut panel, &parsed(ESTATE), None, size, Vec::new());
    let (content, _) = panel.sizes;
    assert!(content.x > size.x, "below the minimum it scrolls: {content:?}");
    let m = map();
    assert!(m.nodes.iter().all(|n| hover_text(n).contains(&n.key)), "the full key on hover");
    // The pure rule: natural width when it fits, shrunk evenly, floored
    // at the widest tag.
    assert!((crate::view_map::node_width(100.0, 40.0, 2.0, 1000.0) - 112.0).abs() < 1e-3);
    let shrunk = crate::view_map::node_width(100.0, 40.0, 4.0, 300.0);
    assert!(shrunk < 112.0 && shrunk > 52.0, "{shrunk}");
    assert!((crate::view_map::node_width(100.0, 80.0, 10.0, 200.0) - 92.0).abs() < 1e-3, "never under the tag");
}

/// P4-112: one build per parse, reused by every frame.
#[test]
fn p4_112_map_built_once_per_parse() {
    let ctx = egui::Context::default();
    let mut panel = ViewMapPanel::default();
    let p = parsed(ESTATE);
    for _ in 0..5 {
        frame(&ctx, &mut panel, &p, None, egui::vec2(800.0, 600.0), Vec::new());
    }
    assert_eq!(panel.builds, 1);
    let again = panel.sync(&p);
    assert_eq!(panel.builds, 1);
    assert_eq!(again.nodes.len(), KEYS.len());
    let next = parsed(ESTATE);
    frame(&ctx, &mut panel, &next, None, egui::vec2(800.0, 600.0), Vec::new());
    assert_eq!(panel.builds, 2, "a new parse builds once");
}

/// P4-113: every view change other than Back and Forward is recorded and
/// drops Forward's views.
#[test]
fn p4_113_every_view_change_is_recorded() {
    let mut sh = Shell::with(ESTATE);
    assert_eq!(sh.view(), "estate");
    assert!(!sh.s.can_go_back(), "nothing before the first view");
    // View list (and the map, which sends the same command).
    sh.show("shop-context");
    // Context menu Enter.
    let at = {
        let id = sh.model().find("shop").expect("shop");
        let b = sh.scene().boxes_of(id)[0];
        sh.s.camera.to_screen(sh.scene().boxes[b.0 as usize].rect.centre())
    };
    let target = sh.menu_targets(at)[0].clone();
    sh.cmd(Command::ContextAction(MenuAction::Show(target)));
    assert_eq!(sh.view(), "shop-containers");
    // Breadcrumb.
    sh.cmd(Command::Crumb(0));
    assert_eq!(sh.view(), "estate");
    // Quick navigation.
    sh.cmd(Command::QuickNavigation);
    sh.cmd(Command::ShowView(declared("live")));
    // Next view, Backspace.
    sh.cmd(Command::NextView);
    assert_eq!(sh.view(), "board");
    sh.show("web-components");
    sh.key(Key::Backspace, Modifiers::default());
    assert_eq!(sh.view(), "shop-containers");
    let mut seen = Vec::new();
    while sh.s.can_go_back() {
        sh.cmd(Command::Back);
        seen.push(sh.view());
    }
    assert_eq!(
        seen,
        ["web-components", "board", "live", "estate", "shop-containers", "shop-context", "estate"],
        "each change recorded, newest first"
    );
    // A new change drops the views Forward would open.
    assert!(sh.s.can_go_forward());
    sh.show("overview");
    assert!(!sh.s.can_go_forward(), "a view change clears Forward");
}

/// P4-114: Back (command, Alt+Left, `b`) opens the view before; disabled
/// with none; Alt+Left no longer goes up.
#[test]
fn p4_114_back_returns_to_the_previous_view() {
    let mut sh = Shell::with(ESTATE);
    sh.show("web-components");
    sh.show("live");
    sh.key(Key::ArrowLeft, Modifiers { alt: true, ..Modifiers::default() });
    assert_eq!(sh.view(), "web-components", "Alt+Left is Back, not the parent crumb");
    sh.key(Key::B, Modifiers::default());
    assert_eq!(sh.view(), "estate", "b is Back (P2-714)");
    assert!(!sh.s.can_go_back(), "the toolbar button is disabled");
    sh.cmd(Command::Back);
    assert_eq!(sh.view(), "estate", "Back with no history does nothing");
}

/// P4-115: Forward (Alt+Right, toolbar) reopens what Back left; nothing
/// without a Back since the last change.
#[test]
fn p4_115_forward_reopens_what_back_left() {
    let mut sh = Shell::with(ESTATE);
    sh.show("shop-context");
    sh.show("shop-containers");
    assert!(!sh.s.can_go_forward(), "disabled before any Back");
    sh.forward();
    assert_eq!(sh.view(), "shop-containers");
    sh.cmd(Command::Back);
    sh.cmd(Command::Back);
    assert_eq!(sh.view(), "estate");
    sh.key(Key::ArrowRight, Modifiers { alt: true, ..Modifiers::default() });
    assert_eq!(sh.view(), "shop-context", "Alt+Right is Forward");
    sh.forward();
    assert_eq!(sh.view(), "shop-containers");
    assert!(!sh.s.can_go_forward());
    sh.cmd(Command::Back);
    assert_eq!(sh.view(), "shop-context", "Back after Forward");
    // Plain Right keeps P2-713's previous view.
    sh.key(Key::ArrowRight, Modifiers::default());
    assert_eq!(sh.view(), "estate");
}

/// P4-116: a reload keeps the history less vanished views; another file
/// starts empty.
#[test]
fn p4_116_reload_keeps_history() {
    let mut sh = Shell::with(ESTATE);
    sh.show("checkout");
    sh.show("live");
    sh.show("board");
    let without_live = ESTATE.replace("        deployment shop live \"live\" \"Live\" {\n            include *\n        }\n", "");
    assert_ne!(without_live, ESTATE);
    sh.reload(&without_live);
    assert_eq!(sh.view(), "board");
    sh.cmd(Command::Back);
    assert_eq!(sh.view(), "checkout", "the vanished view is skipped");
    sh.cmd(Command::Back);
    assert_eq!(sh.view(), "estate");
    assert!(!sh.s.can_go_back());
    assert!(sh.s.can_go_forward(), "Forward survives too");
    // Another file: a new history.
    let other = sh.dir.join("other.dsl");
    std::fs::write(&other, crate::testutil::SHOP).expect("second file");
    sh.cmd(Command::Open(other));
    assert_eq!(sh.view(), "context");
    assert!(!sh.s.can_go_back() && !sh.s.can_go_forward(), "another file starts empty");
}
