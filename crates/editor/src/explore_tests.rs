//! Tests of the explore row, editor part (SRS 4.7: P3-9,
//! P3-500..P3-517), driven headlessly: the pages' readers directly, the
//! egui pages through `Context::run_ui` with synthetic input, and the
//! shell's effects synchronously as `state`'s own tests do. The fixture
//! is `tests/data/phase3/explore.dsl` with its `explore/` folder.

#![allow(clippy::many_single_char_names, reason = "short test bindings: e, p, m, g, t")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use c4rizer_core::model::{self, ElementId, FsLoader, Model, Parsed};
use eframe::egui;

use crate::camera::ScreenPoint;
use crate::explore::markdown::{self, Block, ImageSource};
use crate::explore::{self, Action, Column, Embed, Explore, Page, Section};
use crate::files::{self, FileEvent, UiState};
use crate::state::{Command, Effect, Event, Selected, State};
use crate::testutil::TempDir;
use crate::worker::{self, ViewRef};

/// The fixture's folder.
fn phase3() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/phase3")
}

/// The fixture parsed from its path, so `!docs` and `!adrs` resolve.
fn parsed() -> Arc<Parsed> {
    let path = phase3().join("explore.dsl");
    let text = std::fs::read_to_string(&path).expect("fixture");
    Arc::new(model::parse_with(&text, Some(&path), &mut FsLoader).expect("fixture parses"))
}

/// An `Explore` synced to the fixture.
fn explore() -> (Explore, Arc<Parsed>) {
    let p = parsed();
    let mut e = Explore::default();
    e.sync(&p);
    (e, p)
}

fn id(m: &Model, key: &str) -> ElementId {
    m.find(key).unwrap_or_else(|| panic!("no element {key}"))
}

/// One headless frame of the explore pages in a 1600x900 window.
fn frame(ctx: &egui::Context, e: &mut Explore, m: &Model, events: Vec<egui::Event>, time: f64) -> Vec<Action> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))),
        events,
        time: Some(time),
        ..Default::default()
    };
    let mut actions = Vec::new();
    let mut output = ctx.run_ui(input, |ui| {
        actions = egui::CentralPanel::default().show(ui, |ui| e.show(ui, m)).inner;
    });
    output.textures_delta.clear();
    actions
}

/// A context with the app's fonts.
fn context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::canvas::install_fonts(&ctx);
    ctx
}

/// Press and release of `button` at `at`.
fn click(at: egui::Pos2, button: egui::PointerButton) -> Vec<egui::Event> {
    let m = egui::Modifiers::default();
    vec![
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton { pos: at, button, pressed: true, modifiers: m },
        egui::Event::PointerButton { pos: at, button, pressed: false, modifiers: m },
    ]
}

/// The window position of the graph node of `key`.
fn graph_node(e: &Explore, m: &Model, key: &str) -> egui::Pos2 {
    let g = e.graph.as_ref().expect("graph open");
    let b = g.plan.boxes_of(id(m, key))[0];
    let s = g.camera.to_screen(g.positions[b.0 as usize]);
    g.origin + egui::vec2(s.x, s.y)
}

/// Copies `from` into `to`, folders included.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read dir").flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

/// The shell without a window, on a copy of the fixture: every effect at
/// once; `writes` counts the write effects.
struct Shell {
    s: State,
    path: PathBuf,
    writes: usize,
    _dir: TempDir,
}

impl Shell {
    fn new() -> Shell {
        let dir = TempDir::new("explore");
        copy_tree(&phase3(), dir.path());
        let path = dir.join("explore.dsl");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
            writes: 0,
            _dir: dir,
        };
        shell.event(Event::Command(Command::Open(path)));
        shell
    }

    fn event(&mut self, event: Event) {
        let effects = self.s.handle(event);
        self.run(effects);
    }

    fn run(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            let next = match effect {
                Effect::Read(path) => {
                    let (text, hash) = files::read(&path).expect("read fixture");
                    self.s.handle(Event::File(FileEvent::Loaded { path, text, hash }))
                }
                Effect::Write(request) => {
                    self.writes += 1;
                    self.s.handle(Event::File(files::write_checked(&request)))
                }
                Effect::Work(request) => match worker::serve(request, &AtomicBool::new(false)) {
                    Some(reply) => self.s.handle(Event::Worker(reply)),
                    None => Vec::new(),
                },
                Effect::Watch(_) | Effect::Dialog(_) | Effect::SaveUiState(_) | Effect::Fullscreen(_) | Effect::OpenUrl(_) => {
                    Vec::new()
                }
            };
            self.run(next);
        }
    }

    fn parsed(&self) -> Arc<Parsed> {
        self.s.doc.as_ref().expect("a file").parsed.clone().expect("a parse")
    }

    /// Runs an explore action as the shell does.
    fn act(&mut self, action: Action) {
        if let Action::Open { view, element } = action {
            let effects = self.s.reveal(view, element);
            self.run(effects);
        }
    }
}

/// P3-9: the explore pages are listed, and each one draws.
#[test]
fn p3_9_explore_pages_listed() {
    let (mut e, p) = explore();
    let m = &p.model;
    let pages = Explore::pages(m);
    assert_eq!(&pages[..4], &[Page::Model, Page::Relationships, Page::Documentation, Page::Decisions]);
    let ctx = context();
    for page in pages {
        e.page = page.clone();
        for t in 0..2 {
            let actions = frame(&ctx, &mut e, m, Vec::new(), f64::from(t));
            assert!(actions.is_empty(), "{page:?}");
        }
    }
}

/// P3-500: the model page nests like Structurizr's Model page: custom
/// elements, people, systems with containers and components, then one
/// deployment section per environment nested by node.
#[test]
fn p3_500_model_page_nested_like_structurizr() {
    let (e, p) = explore();
    let m = &p.model;
    let rows = e.model_rows(m);
    let shown: Vec<String> = rows
        .iter()
        .map(|r| {
            let section = match &r.section {
                Section::Static => "static".to_owned(),
                Section::Deployment(env) => env.clone(),
            };
            format!("{section} {}{}", "  ".repeat(r.depth), r.id)
        })
        .collect();
    assert_eq!(
        shown,
        [
            "static sensor",
            "static scanner",
            "static customer",
            "static clerk",
            "static shop",
            "static   web",
            "static     cart",
            "static     pay",
            "static   db",
            "static bank",
            "static archive",
            "Live cloud",
            "Live   host",
            "Live     webLive",
            "Live   store",
            "Live     dbLive",
            "Live   gate",
        ]
    );
    let web = rows.iter().find(|r| r.id == "web").expect("web row");
    assert_eq!(web.name, "Web");
    assert_eq!(web.type_line, "[Container: Rust]");
    assert_eq!(web.description, "Serves the pages.");
    assert_eq!(web.tags, ["Element", "Container"]);
    let sensor = rows.iter().find(|r| r.id == "sensor").expect("sensor row");
    assert_eq!(sensor.type_line, "[Probe]");
}

/// P3-501: an element row expands to its relationships, with direction,
/// other end, description and technology; implied ones marked.
#[test]
fn p3_501_rows_expand_to_relationships() {
    let (e, p) = explore();
    let m = &p.model;
    let lines: Vec<String> = e
        .relationship_lines(m, id(m, "web"))
        .iter()
        .map(|l| {
            format!(
                "{} {} | {} | {}{}",
                if l.outgoing { "->" } else { "<-" },
                m.full_key(l.other).unwrap_or_default(),
                l.description,
                l.technology,
                if l.implied { " (implied)" } else { "" }
            )
        })
        .collect();
    assert_eq!(
        lines,
        [
            "-> db | Reads and writes | SQL",
            "-> bank | Charges cards through | HTTPS (implied)",
            "<- customer | Orders through | HTTPS",
        ]
    );
}

/// P3-502: the table lists every relationship, implied and copied ones
/// included, sorts by any column both ways, and takes ids from the model.
#[test]
fn p3_502_table_sorts_by_every_column() {
    let (mut e, p) = explore();
    let m = &p.model;
    let rows = e.table(m);
    assert_eq!(rows.len(), c4rizer_core::explore::relationship_rows(m).len());
    assert!(rows.iter().any(|r| r.row.implied));
    assert!(rows.iter().any(|r| m.relationship(r.row.relationship).copied_from.is_some()));
    for r in &rows {
        let rel = m.relationship(r.row.relationship);
        let want = rel.key.as_ref().map_or_else(|| rel.id.0.to_string(), |k| k.value.clone());
        assert_eq!(r.id, want);
    }
    for column in Column::ALL {
        for ascending in [true, false] {
            e.sort = (column, !ascending);
            let rows = e.table(m);
            let cells: Vec<String> = rows
                .iter()
                .map(|r| match column {
                    Column::Id => format!("{:08}", r.id.parse::<u64>().unwrap_or(u64::MAX)),
                    Column::Source => r.source.to_lowercase(),
                    Column::Destination => r.destination.to_lowercase(),
                    Column::Description => r.description.to_lowercase(),
                    Column::Technology => r.technology.to_lowercase(),
                    Column::Tags => r.tags.to_lowercase(),
                })
                .collect();
            let mut sorted = cells.clone();
            sorted.sort();
            if !ascending {
                sorted.reverse();
            }
            assert_eq!(cells, sorted, "{column:?} {ascending}");
        }
    }
}

/// P3-503: the search field filters both tables, ignoring case; a hit's
/// ancestors stay on the model page, marked as not matching.
#[test]
fn p3_503_search_filters_both_tables() {
    let (mut e, p) = explore();
    let m = &p.model;
    e.query = "POSTGRES".to_owned();
    let rows: Vec<(String, bool)> = e.model_rows(m).iter().map(|r| (r.id.clone(), r.matched)).collect();
    assert_eq!(rows, [("shop".to_owned(), false), ("db".to_owned(), true)]);
    e.query = "mqtt".to_owned();
    let table: Vec<String> = e.table(m).iter().map(|r| format!("{} -> {}", r.source, r.destination)).collect();
    assert_eq!(table, ["Sensor -> Shop"]);
    // The implied rows follow the relationship they are implied by.
    e.query = "charges cards".to_owned();
    let table: Vec<String> = e.table(m).iter().map(|r| format!("{} -> {}", r.source, r.destination)).collect();
    assert_eq!(table, ["Payments -> Bank", "Web -> Bank", "Shop -> Bank"]);
    e.query.clear();
    assert_eq!(e.model_rows(m).len(), m.elements.len());
}

/// P3-504: selecting an element shows its usages; a click on one opens
/// that view with the box selected.
#[test]
fn p3_504_usage_click_opens_view_with_box_selected() {
    let mut shell = Shell::new();
    let p = shell.parsed();
    let m = &p.model;
    let mut e = Explore::default();
    e.sync(&p);
    e.selected = Some(id(m, "pay"));
    let usages: Vec<String> = e.usages(m).iter().map(|u| u.view.clone()).collect();
    assert_eq!(usages, ["landscape", "context", "containers", "components"]);
    // Click the "containers" usage in the usages panel.
    let ctx = context();
    let mut opened = None;
    for t in 0..2 {
        frame(&ctx, &mut e, m, Vec::new(), f64::from(t));
    }
    let mut t = 10.0;
    'scan: for y in (40..400).step_by(6) {
        for x in (1180..1590).step_by(25) {
            t += 0.5;
            // A click left of the panel selects a row; keep `pay`.
            e.selected = Some(id(m, "pay"));
            #[expect(clippy::cast_precision_loss, reason = "small test coordinates")]
            let at = egui::pos2(x as f32, y as f32);
            let actions = frame(&ctx, &mut e, m, click(at, egui::PointerButton::Primary), t);
            if let Some(a) = actions.into_iter().find(|a| matches!(a, Action::Open { view, .. } if view.key() == "containers")) {
                opened = Some(a);
                break 'scan;
            }
        }
    }
    let action = opened.expect("a usage link opens containers");
    // `pay` is drawn on `containers` through `web`: that box is selected.
    assert_eq!(
        action,
        Action::Open {
            view: ViewRef::Declared("containers".to_owned()),
            element: "web".to_owned()
        }
    );
    shell.act(action);
    assert_eq!(shell.s.view, Some(ViewRef::Declared("containers".to_owned())));
    assert_eq!(shell.s.selection.iter().cloned().collect::<Vec<_>>(), [Selected::Element("web".to_owned())]);
    assert_eq!(shell.s.draw_list().marks.len(), 1, "the web box is marked selected");
}

/// P3-505: an element no view draws is marked "in no view", as inspection
/// `model.element.noview` says.
#[test]
fn p3_505_in_no_view_marked() {
    let (e, p) = explore();
    let marked: Vec<String> = e.model_rows(&p.model).iter().filter(|r| r.in_no_view).map(|r| r.id.clone()).collect();
    assert_eq!(marked, ["archive"]);
}

/// P3-506: "Reveal in file" gives `<path>:<line>:<column>` of the
/// declaration.
#[test]
fn p3_506_reveal_in_file_location() {
    let p = parsed();
    let m = &p.model;
    let text = std::fs::read_to_string(phase3().join("explore.dsl")).expect("fixture");
    let line_of = |needle: &str| text.lines().position(|l| l.contains(needle)).expect("line") + 1;
    let at = explore::reveal_text(&p, m.element(id(m, "web")).span);
    let path = phase3().join("explore.dsl").display().to_string();
    let (file, rest) = at.rsplit_once(':').and_then(|(a, col)| a.rsplit_once(':').map(|(f, l)| (f.to_owned(), (l.to_owned(), col.to_owned())))).expect("path:line:column");
    assert_eq!(file, path);
    assert_eq!(rest.0, line_of("web = container").to_string());
    let column: usize = rest.1.parse().expect("column");
    let web_line = text.lines().nth(line_of("web = container") - 1).expect("line");
    assert_eq!(column, web_line.find("web").expect("web") + 1);
    let r = m.relationships.iter().find(|r| r.source.value == id(m, "pay")).expect("pay -> bank");
    assert!(explore::reveal_text(&p, r.span).ends_with(&format!(":{}:9", line_of("pay -> bank"))));
}

/// P3-507: nothing on the explore pages writes the file: clicking around
/// every page and following an action leaves the file and the undo
/// history as they were.
#[test]
fn p3_507_explore_never_writes() {
    let mut shell = Shell::new();
    let before = std::fs::read(&shell.path).expect("file");
    let p = shell.parsed();
    let m = &p.model;
    let mut e = Explore::default();
    e.sync(&p);
    let ctx = context();
    let mut t = 0.0;
    for page in Explore::pages(m) {
        e.page = page;
        for y in (60..880).step_by(37) {
            for x in [40.0, 400.0, 900.0, 1400.0] {
                t += 1.0;
                #[expect(clippy::cast_precision_loss, reason = "small test coordinates")]
                let at = egui::pos2(x, y as f32);
                for action in frame(&ctx, &mut e, m, click(at, egui::PointerButton::Primary), t) {
                    shell.act(action);
                }
            }
        }
    }
    assert_eq!(shell.writes, 0);
    assert!(!shell.s.history.can_undo());
    assert_eq!(std::fs::read(&shell.path).expect("file"), before);
}

/// P3-508: the graph page draws the view's elements and relationships as
/// nodes and links, with no boundary, at the core's force layout.
#[test]
fn p3_508_graph_page_draws_view_without_boundaries() {
    let (mut e, p) = explore();
    let m = &p.model;
    e.open_graph(m, "containers");
    let g = e.graph.as_ref().expect("graph");
    let plan = c4rizer_core::view::plan(m, m.view("containers").expect("view"));
    assert!(!plan.boundaries.is_empty(), "the diagram has the shop boundary");
    let keys: Vec<String> = g.plan.boxes.iter().map(|b| m.full_key(b.element).unwrap_or_default()).collect();
    assert_eq!(keys, ["customer", "web", "db", "bank"]);
    assert_eq!(g.plan.edges.len(), plan.edges.len());
    assert_eq!(g.positions, c4rizer_core::explore::graph_layout(&g.plan));
    let ctx = context();
    let before = g.positions.clone();
    for t in 0..3 {
        frame(&ctx, &mut e, m, Vec::new(), f64::from(t));
    }
    assert_eq!(e.graph.as_ref().expect("graph").positions, before, "drawing moves no node");
}

/// P3-509: a click on a node keeps it, its neighbours and their links
/// opaque and fades the rest to 10%; empty space (or the node again)
/// clears it.
#[test]
fn p3_509_click_fades_unconnected_to_10_percent() {
    let (mut e, p) = explore();
    let m = &p.model;
    e.open_graph(m, "containers");
    let ctx = context();
    for t in 0..2 {
        frame(&ctx, &mut e, m, Vec::new(), f64::from(t));
    }
    let at = graph_node(&e, m, "db");
    frame(&ctx, &mut e, m, click(at, egui::PointerButton::Primary), 5.0);
    let (nodes, edges) = e.graph_opacity();
    // Boxes: customer, web, db, bank; edges: customer->web, web->db,
    // web->bank.
    assert_eq!(nodes, [0.1, 1.0, 1.0, 0.1]);
    let g = e.graph.as_ref().expect("graph");
    let edge_names: Vec<String> = g
        .plan
        .edges
        .iter()
        .map(|x| format!("{}->{}", g.plan.boxes[x.source.0 as usize].name, g.plan.boxes[x.destination.0 as usize].name))
        .collect();
    let want: Vec<f32> = edge_names.iter().map(|n| if n == "Web->Database" { 1.0 } else { 0.1 }).collect();
    assert_eq!(edges, want, "{edge_names:?}");
    // Empty space clears it.
    let empty = e.graph.as_ref().expect("graph").origin + egui::vec2(5.0, 5.0);
    frame(&ctx, &mut e, m, click(empty, egui::PointerButton::Primary), 7.0);
    assert_eq!(e.graph_opacity().0, [1.0; 4]);
    // The same node twice: on, then off.
    e.graph_click(ScreenPoint { x: 0.0, y: 0.0 });
    let at = graph_node(&e, m, "web");
    frame(&ctx, &mut e, m, click(at, egui::PointerButton::Primary), 9.0);
    assert!(e.graph.as_ref().expect("graph").focus.is_some());
    frame(&ctx, &mut e, m, click(at, egui::PointerButton::Primary), 11.0);
    assert!(e.graph.as_ref().expect("graph").focus.is_none());
}

/// P3-510: the tree page lays the tree out left to right: depth on x,
/// every child right of its parent, leaves on their own rows.
#[test]
fn p3_510_tree_page_left_to_right() {
    let (e, p) = explore();
    let m = &p.model;
    for key in ["components", "live"] {
        let spots = e.tree_spots(m, key);
        assert!(spots.len() > 3, "{key}");
        assert_eq!(spots[0].parent, None);
        assert_eq!(spots[0].at.x, 0);
        for s in &spots[1..] {
            let parent = &spots[s.parent.expect("parent")];
            assert!(s.at.x > parent.at.x, "{key}: {} right of {}", s.label, parent.label);
        }
        let mut rows: Vec<i32> = spots
            .iter()
            .enumerate()
            .filter(|(i, _)| !spots.iter().any(|c| c.parent == Some(*i)))
            .map(|(_, s)| s.at.y)
            .collect();
        let n = rows.len();
        rows.dedup();
        assert_eq!(rows.len(), n, "{key}: leaves on distinct rows");
    }
    let live: Vec<String> = e.tree_spots(m, "live").into_iter().map(|s| s.label).collect();
    assert_eq!(live, ["Live", "Cloud", "Host", "Web", "Store", "Database", "Gateway"]);
}

/// P3-511: graph and tree are offered for exactly Structurizr's kinds.
#[test]
fn p3_511_pages_offered_per_kind() {
    let p = parsed();
    let pages = Explore::pages(&p.model);
    let graphs: Vec<&str> = pages.iter().filter_map(|p| if let Page::Graph(k) = p { Some(k.as_str()) } else { None }).collect();
    let trees: Vec<&str> = pages.iter().filter_map(|p| if let Page::Tree(k) = p { Some(k.as_str()) } else { None }).collect();
    assert_eq!(graphs, ["landscape", "context", "containers", "components", "devices"]);
    assert_eq!(trees, ["landscape", "context", "containers", "components", "devices", "live"]);
    let mut e = Explore::default();
    e.sync(&p);
    e.open_graph(&p.model, "databases");
    assert!(e.graph.is_none(), "no graph for a filtered view");
    e.open_tree(&p.model, "checkout");
    assert!(e.tree.is_none(), "no tree for a dynamic view");
}

/// P3-512: the graph and tree pan and zoom with EDIT-28 gestures (wheel
/// at the cursor, middle drag), and a double click on a node opens the
/// diagram view with that box selected.
#[test]
fn p3_512_pan_zoom_and_double_click_opens_view() {
    let (mut e, p) = explore();
    let m = &p.model;
    e.open_graph(m, "containers");
    let ctx = context();
    for t in 0..2 {
        frame(&ctx, &mut e, m, Vec::new(), f64::from(t));
    }
    // Wheel: zoom changes, the point under the cursor stays.
    let cursor = graph_node(&e, m, "web");
    let g = e.graph.as_ref().expect("graph");
    let rel = ScreenPoint { x: cursor.x - g.origin.x, y: cursor.y - g.origin.y };
    let (zoom, under) = (g.camera.zoom, g.camera.to_scene(rel));
    let wheel = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 120.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::default(),
        source: egui::MouseWheelSource::Wheel,
    };
    let mut t = 3.0;
    frame(&ctx, &mut e, m, vec![egui::Event::PointerMoved(cursor), wheel], t);
    for _ in 0..30 {
        t += 0.02;
        frame(&ctx, &mut e, m, vec![egui::Event::PointerMoved(cursor)], t);
    }
    let g = e.graph.as_ref().expect("graph");
    assert!(g.camera.zoom > zoom, "{} > {zoom}", g.camera.zoom);
    let now = g.camera.to_scene(rel);
    assert!((now.x - under.x).abs() < 1.0 && (now.y - under.y).abs() < 1.0, "{now:?} {under:?}");
    // Middle drag pans by the pointer travel.
    let (pan, zoom) = (g.camera.pan, g.camera.zoom);
    let start = g.origin + egui::vec2(20.0, 20.0);
    let mods = egui::Modifiers::default();
    t += 1.0;
    frame(&ctx, &mut e, m, vec![egui::Event::PointerMoved(start), egui::Event::PointerButton { pos: start, button: egui::PointerButton::Middle, pressed: true, modifiers: mods }], t);
    for step in 1..=4_u8 {
        t += 0.02;
        frame(&ctx, &mut e, m, vec![egui::Event::PointerMoved(start + egui::vec2(20.0 * f32::from(step), 0.0))], t);
    }
    t += 0.02;
    let end = start + egui::vec2(80.0, 0.0);
    frame(&ctx, &mut e, m, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Middle, pressed: false, modifiers: mods }], t);
    let g = e.graph.as_ref().expect("graph");
    assert!((pan.x - g.camera.pan.x - 80.0 / zoom).abs() < 1.0, "{pan:?} -> {:?}", g.camera.pan);
    // Double click on `db` opens the diagram with it selected.
    let at = graph_node(&e, m, "db");
    t += 2.0;
    let mut events = click(at, egui::PointerButton::Primary);
    events.extend(click(at, egui::PointerButton::Primary));
    let actions = frame(&ctx, &mut e, m, events, t);
    assert!(
        actions.contains(&Action::Open {
            view: ViewRef::Declared("containers".to_owned()),
            element: "db".to_owned()
        }),
        "{actions:?}"
    );
    // The tree: double click on a node opens its view.
    e.open_tree(m, "live");
    for _ in 0..2 {
        t += 1.0;
        frame(&ctx, &mut e, m, Vec::new(), t);
    }
    let tr = e.tree.as_ref().expect("tree");
    let spot = tr.spots.iter().find(|s| s.label == "Gateway").expect("gateway");
    let s = tr.camera.to_screen(spot.at);
    let at = tr.origin + egui::vec2(s.x + 12.0, s.y);
    t += 2.0;
    let mut events = click(at, egui::PointerButton::Primary);
    events.extend(click(at, egui::PointerButton::Primary));
    let actions = frame(&ctx, &mut e, m, events, t);
    assert!(
        actions.contains(&Action::Open {
            view: ViewRef::Declared("live".to_owned()),
            element: "gate".to_owned()
        }),
        "{actions:?}"
    );
}

/// P3-513: the documentation lists the workspace's sections, then each
/// system's, in file-name order, Markdown and `AsciiDoc` only.
#[test]
fn p3_513_sections_in_file_name_order() {
    let (e, p) = explore();
    let m = &p.model;
    let shown: Vec<String> = e
        .sections()
        .iter()
        .map(|s| {
            let owner = s.owner.and_then(|o| m.full_key(o)).unwrap_or_else(|| "workspace".to_owned());
            format!("{owner}: {} ({})", s.title, s.path.file_name().unwrap_or_default().to_string_lossy())
        })
        .collect();
    assert_eq!(
        shown,
        [
            "workspace: Overview (01-overview.md)",
            "workspace: Glossary (02-glossary.md)",
            "shop: Context (01-context.md)",
            "shop: Operations (02-operations.adoc)",
        ]
    );
}

/// P3-514: Markdown is rendered with headings, paragraphs, emphasis,
/// lists, code blocks, tables, links and images; a relative image is
/// read from the docs folder.
#[test]
fn p3_514_markdown_blocks_rendered() {
    let (mut e, p) = explore();
    let text = &e.sections()[2].text;
    let blocks = markdown::blocks(text);
    let kinds: Vec<String> = blocks
        .iter()
        .map(|b| match b {
            Block::Heading { level, spans } => format!("h{level} {}", markdown::plain(spans)),
            Block::Paragraph(_) => "p".to_owned(),
            Block::List { start, items } => format!("list {start:?} {}", items.len()),
            Block::Code(_) => "code".to_owned(),
            Block::Table { header, rows } => format!("table {}x{}", header.len(), rows.len()),
            Block::Image { source, .. } => format!("image {source:?}"),
            Block::Quote(_) => "quote".to_owned(),
            Block::Rule => "rule".to_owned(),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "h1 Context",
            "p",
            "image Embed(\"context\")",
            "h2 Parts",
            "list Some(1) 2",
            "table 2x2",
            "code",
            "image Relative(\"logo.png\")",
            "image Remote(\"https://example.com/badge.png\")",
            "image Embed(\"no-such-view\")",
        ]
    );
    let Block::Paragraph(spans) = &blocks[1] else { panic!("paragraph") };
    let look: Vec<(String, bool, bool, bool)> = spans.iter().map(|s| (s.text.clone(), s.strong, s.emphasis, s.code)).collect();
    assert!(look.contains(&("customers".to_owned(), true, false, false)), "{look:?}");
    assert!(look.contains(&("clerks".to_owned(), false, true, false)), "{look:?}");
    assert!(look.contains(&("HTTPS".to_owned(), false, false, true)), "{look:?}");
    assert!(spans.iter().any(|s| s.link.as_deref() == Some("https://structurizr.com")));
    let Block::List { items, .. } = &blocks[4] else { panic!("list") };
    assert!(matches!(&items[1][..], [Block::Paragraph(_), Block::List { start: None, .. }]), "{:?}", items[1]);
    let Block::Code(code) = &blocks[6] else { panic!("code") };
    assert_eq!(code, "customer -> web \"Orders through\"");
    // The page draws, and the relative image decodes at its own size.
    e.page = Page::Documentation;
    e.section = 2;
    let ctx = context();
    for t in 0..2 {
        frame(&ctx, &mut e, &p.model, Vec::new(), f64::from(t));
    }
    let logo = phase3().join("explore/shop-docs/logo.png");
    let (_, w, h) = c4rizer_core::explore::image_asset("logo.png", &std::fs::read(logo).expect("logo")).expect("decodes");
    assert_eq!((w, h), (120, 40));
}

/// P3-514: a remote image is not fetched while network (P3-600) is off:
/// it is classified remote and drawn as a placeholder naming the URL.
#[test]
fn p3_514_remote_image_not_fetched() {
    assert_eq!(ImageSource::of("https://example.com/a.png"), ImageSource::Remote("https://example.com/a.png".to_owned()));
    assert_eq!(ImageSource::of("HTTP://example.com/a.png"), ImageSource::Remote("HTTP://example.com/a.png".to_owned()));
    assert_eq!(ImageSource::of("img/a.png"), ImageSource::Relative("img/a.png".to_owned()));
    let blocks = markdown::blocks("![badge](https://example.com/badge.png)\n");
    assert_eq!(
        blocks,
        [Block::Image {
            alt: "badge".to_owned(),
            source: ImageSource::Remote("https://example.com/badge.png".to_owned())
        }]
    );
    // The page draws it without touching the network (the fetch seam
    // returns nothing on this branch).
    let (mut e, p) = explore();
    e.page = Page::Documentation;
    e.section = 2;
    let ctx = context();
    frame(&ctx, &mut e, &p.model, Vec::new(), 0.0);
}

/// P3-515: an `AsciiDoc` section is shown as its source text under a
/// one-line note.
#[test]
fn p3_515_asciidoc_as_source_with_note() {
    let (mut e, p) = explore();
    let section = &e.sections()[3];
    assert_eq!(section.format, explore::docs::Format::AsciiDoc);
    let file = std::fs::read_to_string(phase3().join("explore/shop-docs/02-operations.adoc")).expect("adoc");
    assert_eq!(section.text, file);
    assert!(explore::ASCIIDOC_NOTE.contains("not rendered"));
    assert_eq!(explore::docs::format_of(Path::new("a.asciidoc")), Some(explore::docs::Format::AsciiDoc));
    assert_eq!(explore::docs::format_of(Path::new("a.markdown")), Some(explore::docs::Format::Markdown));
    assert_eq!(explore::docs::format_of(Path::new("logo.png")), None);
    e.page = Page::Documentation;
    e.section = 3;
    frame(&context(), &mut e, &p.model, Vec::new(), 0.0);
}

/// P3-516: the decisions are listed with id, title, date and status,
/// sorted by id; a link opens the linked decision.
#[test]
fn p3_516_decisions_listed_sorted_and_linked() {
    let (mut e, p) = explore();
    let m = &p.model;
    let shown: Vec<String> = e
        .decisions()
        .iter()
        .map(|d| {
            let owner = d.owner.and_then(|o| m.full_key(o)).unwrap_or_default();
            format!("{owner} {} {} {} {}", d.decision.id, d.decision.title, d.decision.date.as_deref().unwrap_or("-"), d.decision.status)
        })
        .collect();
    assert_eq!(
        shown,
        [
            "shop 1 Record architecture decisions 2026-01-05 Superseded",
            "shop 2 Use PostgreSQL 2026-01-12 Accepted",
            "shop 3 Keep decisions in the repository 2026-02-02 Accepted",
        ]
    );
    // "Superseded by 3" opens decision 3, "Supersedes 1" opens 1.
    assert_eq!(e.linked_decision(0, "3"), Some(2));
    assert_eq!(e.linked_decision(2, "1"), Some(0));
    assert_eq!(e.linked_decision(0, "9"), None);
    // Clicking the link on the page follows it.
    e.page = Page::Decisions;
    e.decision = 0;
    let ctx = context();
    for t in 0..2 {
        frame(&ctx, &mut e, m, Vec::new(), f64::from(t));
    }
    let mut t = 3.0;
    'scan: for y in (40..300).step_by(3) {
        for x in (500..1200).step_by(20) {
            t += 0.5;
            #[expect(clippy::cast_precision_loss, reason = "small test coordinates")]
            frame(&ctx, &mut e, m, click(egui::pos2(x as f32, y as f32), egui::PointerButton::Primary), t);
            if e.decision == 2 {
                break 'scan;
            }
        }
    }
    assert_eq!(e.decision, 2, "the Superseded by 3 link opens decision 3");
}

/// P3-517: `![](embed:<key>)` draws that view's scene; an unknown key
/// shows a one-line placeholder naming it.
#[test]
fn p3_517_embed_draws_view_unknown_key_placeholder() {
    let (mut e, p) = explore();
    let m = &p.model;
    match e.embed(m, "context") {
        Embed::View(build) => assert!(!build.scene.boxes.is_empty()),
        Embed::Placeholder(t) => panic!("{t}"),
    }
    match e.embed(m, "no-such-view") {
        Embed::Placeholder(t) => assert!(t.contains("`no-such-view`"), "{t}"),
        Embed::View(_) => panic!("no view"),
    }
    match e.embed(m, "checkout") {
        Embed::Placeholder(t) => assert!(t.contains("dynamic"), "{t}"),
        Embed::View(_) => panic!("dynamic views are not drawn on this branch"),
    }
    // Drawn on the workspace's overview page.
    e.page = Page::Documentation;
    e.section = 0;
    let ctx = context();
    for t in 0..2 {
        frame(&ctx, &mut e, m, Vec::new(), f64::from(t));
    }
}
