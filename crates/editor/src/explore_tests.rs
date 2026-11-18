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
    // In a narrow window a row's marks wrap whole: the `Legacy` tag stays
    // one line of text, and no row runs past the column (every row frame
    // ends at the same right edge).
    let mut e = e;
    e.page = Page::Model;
    for width in (640_u16..=1200).step_by(40) {
        let (legacy, rights) = model_page_marks(&mut e, m, f32::from(width));
        assert_eq!(legacy, [1], "the Legacy tag is drawn once, on one line, at {width}px");
        assert_eq!(rights.len(), 1, "row frames end at one right edge at {width}px: {rights:?}");
    }
}

/// Draws the model page in a `width` x 3000 window; returns the line
/// count of each `Legacy` text and the distinct right edges of the row
/// frames.
fn model_page_marks(e: &mut Explore, m: &Model, width: f32) -> (Vec<usize>, Vec<i32>) {
    let ctx = context();
    let mut shapes = Vec::new();
    for t in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 3000.0))),
            time: Some(f64::from(t)),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| e.show(ui, m));
        });
        output.textures_delta.clear();
        shapes = output.shapes.into_iter().map(|c| c.shape).collect();
    }
    let mut flat = Vec::new();
    while let Some(s) = shapes.pop() {
        match s {
            egui::Shape::Vec(v) => shapes.extend(v),
            s => flat.push(s),
        }
    }
    let legacy = flat
        .iter()
        .filter_map(|s| match s {
            egui::Shape::Text(t) if t.galley.text() == "Legacy" => Some(t.galley.rows.len()),
            _ => None,
        })
        .collect();
    let mut rights: Vec<i32> = flat
        .iter()
        .filter_map(|s| match s {
            // Row frames: rounded 4px, stroked 1.5px (3px when selected).
            #[expect(clippy::cast_possible_truncation, reason = "screen px")]
            egui::Shape::Rect(r) if r.corner_radius == egui::CornerRadius::same(4) && r.stroke.width >= 1.5 => {
                Some(r.rect.right().round() as i32)
            }
            _ => None,
        })
        .collect();
    rights.sort_unstable();
    rights.dedup();
    (legacy, rights)
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

/// A workspace of `n` software systems in a chain, with a landscape view
/// that draws them all (P3-503's 1000-element model).
fn big_model(n: usize) -> Arc<Parsed> {
    use std::fmt::Write as _;
    let mut src = String::from("workspace {\n model {\n");
    for i in 0..n {
        let _ = writeln!(src, "  e{i} = softwareSystem \"System {i}\" \"Does thing {i}.\" \"Tag{}\"", i % 7);
    }
    for i in 0..n - 1 {
        let _ = writeln!(src, "  e{i} -> e{} \"Calls {i}\" \"HTTPS\"", i + 1);
    }
    src.push_str(" }\n views {\n  systemLandscape \"all\" {\n   include *\n  }\n }\n}\n");
    Arc::new(model::parse_with(&src, None, &mut FsLoader).expect("generated model parses"))
}

/// The shapes of one headless frame of the explore pages in a `size`
/// window, nested shape lists flattened.
fn frame_shapes(ctx: &egui::Context, e: &mut Explore, m: &Model, size: egui::Vec2, time: f64) -> Vec<egui::Shape> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        time: Some(time),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| e.show(ui, m));
    });
    output.textures_delta.clear();
    let mut shapes: Vec<egui::Shape> = output.shapes.into_iter().map(|c| c.shape).collect();
    let mut flat = Vec::new();
    while let Some(s) = shapes.pop() {
        match s {
            egui::Shape::Vec(v) => shapes.extend(v),
            s => flat.push(s),
        }
    }
    flat
}

/// The texts of `shapes`.
fn texts(shapes: &[egui::Shape]) -> Vec<String> {
    shapes
        .iter()
        .filter_map(|s| match s {
            egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
            _ => None,
        })
        .collect()
}

/// The textured shapes of `shapes` (images, not the font atlas), with
/// their texture and screen rectangle.
fn textured(shapes: &[egui::Shape]) -> Vec<(egui::TextureId, egui::Rect)> {
    shapes
        .iter()
        .filter_map(|s| match s {
            egui::Shape::Rect(r) => r.brush.as_ref().map(|b| (b.fill_texture_id, r.rect)),
            egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some((m.texture_id, m.calc_bounds())),
            _ => None,
        })
        .collect()
}

/// The documentation page on section `section` in a `size` window, after
/// two frames: its shapes.
fn docs_page(e: &mut Explore, m: &Model, section: usize, size: egui::Vec2) -> Vec<egui::Shape> {
    e.page = Page::Documentation;
    e.section = section;
    let ctx = context();
    let _ = frame_shapes(&ctx, e, m, size, 0.0);
    frame_shapes(&ctx, e, m, size, 1.0)
}

/// P3-503: an unchanged frame reuses the page's rows, table and usages
/// (rebuilt only when the parse, the search text, the sort or the
/// selection changes), and a 1000-element model page draws only the rows
/// in sight.
#[test]
fn p3_503_page_data_cached_and_rows_in_sight_only() {
    let (mut e, p) = explore();
    let m = &p.model;
    let ctx = context();
    let size = egui::vec2(1600.0, 900.0);
    e.page = Page::Model;
    let _ = frame_shapes(&ctx, &mut e, m, size, 0.0);
    let a = e.cached_model_rows(m);
    let _ = frame_shapes(&ctx, &mut e, m, size, 1.0);
    assert!(Arc::ptr_eq(&a, &e.cached_model_rows(m)), "rows kept on an unchanged frame");
    e.query = "web".to_owned();
    let b = e.cached_model_rows(m);
    assert!(!Arc::ptr_eq(&a, &b), "a new search text rebuilds the rows");
    e.selected = Some(id(m, "web"));
    let u = e.cached_usages(m);
    let _ = frame_shapes(&ctx, &mut e, m, size, 2.0);
    assert!(Arc::ptr_eq(&u, &e.cached_usages(m)), "usages kept for the same selection");
    e.selected = Some(id(m, "db"));
    assert!(!Arc::ptr_eq(&u, &e.cached_usages(m)), "a new selection rebuilds the usages");
    e.page = Page::Relationships;
    let _ = frame_shapes(&ctx, &mut e, m, size, 3.0);
    let t = e.cached_table(m);
    let _ = frame_shapes(&ctx, &mut e, m, size, 4.0);
    assert!(Arc::ptr_eq(&t, &e.cached_table(m)), "table kept on an unchanged frame");
    e.sort = (Column::Source, false);
    assert!(!Arc::ptr_eq(&t, &e.cached_table(m)), "a new sort rebuilds the table");
    // A new parse drops everything.
    let p2 = parsed();
    e.sync(&p2);
    assert!(!Arc::ptr_eq(&t, &e.cached_table(&p2.model)));

    let big = big_model(1000);
    let mut e = Explore::default();
    e.sync(&big);
    e.page = Page::Model;
    for t in 0..3 {
        let _ = frame_shapes(&ctx, &mut e, &big.model, size, 10.0 + f64::from(t));
    }
    let laid_out = e.rows_laid_out();
    assert!(laid_out > 5, "the rows in sight are laid out: {laid_out}");
    assert!(laid_out < 60, "rows out of sight are not laid out: {laid_out}");
}

/// P3-503: a model page frame after the search text changes stays within
/// one 60 Hz frame (16 ms) for a 1000-element model, release build.
#[test]
#[ignore = "perf: run with --release -- --ignored perf_"]
fn perf_p3_503_page_frame_1000_elements() {
    if cfg!(debug_assertions) {
        eprintln!("perf_p3_503_page_frame_1000_elements: skipped in a debug build");
        return;
    }
    let big = big_model(1000);
    let m = &big.model;
    let mut e = Explore::default();
    e.sync(&big);
    e.page = Page::Model;
    let ctx = context();
    let size = egui::vec2(1600.0, 900.0);
    for t in 0..5 {
        let _ = frame_shapes(&ctx, &mut e, m, size, f64::from(t));
    }
    let mut times = Vec::new();
    let mut t = 10.0;
    for round in 0..5 {
        for q in ["s", "sy", "sys", "syst", "system 99", "tag3", "calls 5", ""] {
            e.query = if round % 2 == 0 { q.to_owned() } else { q.to_uppercase() };
            t += 1.0;
            let start = std::time::Instant::now();
            let _ = frame_shapes(&ctx, &mut e, m, size, t);
            times.push(start.elapsed());
        }
    }
    times.sort();
    let median = times[times.len() / 2];
    let worst = times[times.len() - 1];
    println!("perf_p3_503_page_frame_1000_elements: median {:.2} ms, worst {:.2} ms", median.as_secs_f64() * 1e3, worst.as_secs_f64() * 1e3);
    assert!(median.as_millis() < 16, "median {median:?}");
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
    // While Explore is open, the side panels' and toolbar's file-writing
    // controls are off: their events are dropped before the state sees
    // them (and the details panel is drawn disabled).
    let writers = vec![
        Event::Command(Command::Undo),
        Event::Command(Command::Redo),
        Event::Command(Command::AutoPlace),
        Event::Command(Command::ResetStyle),
        Event::Command(Command::Centre),
        Event::Command(Command::BringOntoCanvas),
        Event::Text("typed into a field".to_owned()),
    ];
    let (kept, leave) = crate::app::explore_gate(writers);
    assert!(kept.is_empty(), "{kept:?}");
    assert!(!leave);
    // Picking a view in the view list (or a breadcrumb) leaves Explore and
    // opens that view.
    let (kept, leave) = crate::app::explore_gate(vec![Event::Command(Command::ShowView(ViewRef::Declared("context".to_owned())))]);
    assert!(matches!(&kept[..], [Event::Command(Command::ShowView(ViewRef::Declared(k)))] if k == "context"), "{kept:?}");
    assert!(leave);
    let (kept, leave) = crate::app::explore_gate(vec![Event::Command(Command::Crumb(0))]);
    assert_eq!(kept.len(), 1);
    assert!(leave);
    // Fitting, full screen and opening another file write nothing and stay.
    let (kept, leave) = crate::app::explore_gate(vec![Event::Command(Command::Fullscreen), Event::Command(Command::ShowOpenDialog)]);
    assert_eq!(kept.len(), 2);
    assert!(!leave);
    // The kept view pick opens the view in the shell without a write.
    for event in crate::app::explore_gate(vec![Event::Command(Command::ShowView(ViewRef::Declared("context".to_owned())))]).0 {
        shell.event(event);
    }
    assert_eq!(shell.writes, 0);
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
    // Light/dark: the graph is re-planned in the new mode (the node
    // colours follow), the layout and the focus stay.
    let light: Vec<_> = e.graph.as_ref().expect("graph").plan.boxes.iter().map(|b| b.style.background).collect();
    let db = e.graph.as_ref().expect("graph").plan.boxes_of(id(m, "db"))[0];
    e.graph.as_mut().expect("graph").focus = Some(db);
    e.mode = model::Mode::Dark;
    frame(&ctx, &mut e, m, Vec::new(), 5.0);
    let g = e.graph.as_ref().expect("graph");
    let dark_plan = c4rizer_core::view::plan_in(m, m.view("containers").expect("view"), model::Mode::Dark);
    let dark: Vec<_> = g.plan.boxes.iter().map(|b| b.style.background).collect();
    assert_eq!(dark, dark_plan.boxes.iter().map(|b| b.style.background).collect::<Vec<_>>());
    assert_ne!(dark, light, "the fixture's colours differ in dark mode");
    assert_eq!(g.positions, before);
    assert_eq!(g.focus, Some(db));
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
    // Double click on `db` (two clicks in two frames) opens the diagram
    // with it selected; the second click does not toggle the focus off.
    let at = graph_node(&e, m, "db");
    t += 2.0;
    let first = frame(&ctx, &mut e, m, click(at, egui::PointerButton::Primary), t);
    assert!(first.is_empty(), "{first:?}");
    let db = e.graph.as_ref().expect("graph").plan.boxes_of(id(m, "db"))[0];
    assert_eq!(e.graph.as_ref().expect("graph").focus, Some(db));
    t += 0.1;
    let actions = frame(&ctx, &mut e, m, click(at, egui::PointerButton::Primary), t);
    assert!(
        actions.contains(&Action::Open {
            view: ViewRef::Declared("containers".to_owned()),
            element: "db".to_owned()
        }),
        "{actions:?}"
    );
    assert_eq!(e.graph.as_ref().expect("graph").focus, Some(db), "the double click keeps the focus");
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
    // A relative link to another section's file opens that section, from
    // the section's own folder or another docs folder.
    // TODO(srs): P3-513/P3-514 say nothing about links between sections;
    // Structurizr's documentation viewer follows them in-app.
    assert_eq!(e.section_for_link(0, "02-glossary.md"), Some(1));
    assert_eq!(e.section_for_link(0, "./02-glossary.md#terms"), Some(1));
    assert_eq!(e.section_for_link(2, "../docs/01-overview.md"), Some(0));
    assert_eq!(e.section_for_link(2, "02-operations.adoc"), Some(3));
    assert_eq!(e.section_for_link(0, "https://example.com/02-glossary.md"), None);
    assert_eq!(e.section_for_link(0, "missing.md"), None);
    assert_eq!(e.section_for_link(0, "#overview"), None);
    // Clicking the overview's link to the glossary opens it.
    let mut e = e;
    e.page = Page::Documentation;
    e.section = 0;
    let ctx = context();
    let mut t = 0.0;
    for _ in 0..2 {
        t += 1.0;
        frame(&ctx, &mut e, m, Vec::new(), t);
    }
    'scan: for y in (60..300).step_by(3) {
        for x in (250..1000).step_by(15) {
            t += 0.5;
            #[expect(clippy::cast_precision_loss, reason = "small test coordinates")]
            frame(&ctx, &mut e, m, click(egui::pos2(x as f32, y as f32), egui::PointerButton::Primary), t);
            if e.section != 0 {
                break 'scan;
            }
        }
    }
    assert_eq!(e.section, 1, "the glossary link opens the glossary");
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
    // The page draws, and the relative image decodes and is drawn at
    // its own size.
    let logo = phase3().join("explore/shop-docs/logo.png");
    let (_, w, h) = c4rizer_core::explore::image_asset("logo.png", &std::fs::read(logo).expect("logo")).expect("decodes");
    assert_eq!((w, h), (120, 40));
    let shapes = docs_page(&mut e, &p.model, 2, egui::vec2(1600.0, 4000.0));
    let images: Vec<egui::Vec2> = textured(&shapes).iter().map(|(_, r)| r.size()).collect();
    assert!(images.contains(&egui::vec2(120.0, 40.0)), "the logo is drawn at 120x40: {images:?}");
    // Its texture is made once and kept, not re-made every frame.
    let ctx = context();
    let logo_texture = |shapes: &[egui::Shape]| {
        textured(shapes).into_iter().find(|(_, r)| r.size() == egui::vec2(120.0, 40.0)).map(|(t, _)| t)
    };
    let size = egui::vec2(1600.0, 4000.0);
    let first = logo_texture(&frame_shapes(&ctx, &mut e, &p.model, size, 10.0));
    let second = logo_texture(&frame_shapes(&ctx, &mut e, &p.model, size, 11.0));
    assert!(first.is_some());
    assert_eq!(first, second, "the logo's texture is kept between frames");
    // Drawn at three widths in one parse (a column narrower than the
    // logo scales it down), the cache keeps one texture for the file, the
    // last size's; an unchanged width keeps the same texture.
    let path = phase3().join("explore/shop-docs/logo.png");
    let image = c4rizer_core::explore::image_asset("logo.png", &std::fs::read(&path).expect("logo")).expect("decodes");
    let draw_at = |e: &mut Explore, width: f32, time: f64| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            time: Some(time),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.allocate_ui(egui::vec2(width, 200.0), |ui| {
                    ui.set_max_width(width);
                    e.draw_image(ui, &path, &image);
                });
            });
        });
        output.textures_delta.clear();
        let shapes: Vec<egui::Shape> = output.shapes.into_iter().map(|c| c.shape).collect();
        textured(&shapes).into_iter().next().expect("the logo is drawn")
    };
    let mut seen = Vec::new();
    for (i, width) in [60.0_f32, 80.0, 100.0].into_iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "small loop counter")]
        seen.push(draw_at(&mut e, width, 30.0 + i as f64));
    }
    let sizes: Vec<f32> = seen.iter().map(|(_, r)| r.width()).collect();
    assert_eq!(sizes, [60.0, 80.0, 100.0], "drawn at the column width");
    assert_eq!(e.texture_entries(), 1, "one texture kept for the logo");
    let again = draw_at(&mut e, 100.0, 40.0);
    assert_eq!(again.0, seen[2].0, "an unchanged width keeps the texture");
    let shown = texts(&shapes);
    for text in ["Context", "Parts", "Shop logo", "customer -> web \"Orders through\"", "Technology", "PostgreSQL"] {
        assert!(shown.iter().any(|s| s == text), "{text:?} is drawn: {shown:?}");
    }
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
    // returns nothing on this branch): a placeholder naming the URL, and
    // the notice above the section.
    let (mut e, p) = explore();
    let shown = texts(&docs_page(&mut e, &p.model, 2, egui::vec2(1600.0, 4000.0)));
    for text in [
        "Remote image not fetched (network off): https://example.com/badge.png",
        "1 remote image not fetched: network is off for this file (P3-600).",
    ] {
        assert!(shown.iter().any(|s| s == text), "{text:?} is drawn: {shown:?}");
    }
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
    // Drawn: the note, then the source as one monospace block.
    let shapes = docs_page(&mut e, &p.model, 3, egui::vec2(1600.0, 900.0));
    let shown = texts(&shapes);
    assert!(shown.iter().any(|s| s == explore::ASCIIDOC_NOTE), "the note is drawn: {shown:?}");
    let source = shapes.iter().find_map(|s| match s {
        egui::Shape::Text(t) if t.galley.text().trim_end() == file.trim_end() => Some(t),
        _ => None,
    });
    let source = source.unwrap_or_else(|| panic!("the source is drawn whole: {shown:?}"));
    assert!(
        source.galley.job.sections.iter().all(|s| s.format.font_id.family == egui::FontFamily::Monospace),
        "the source is monospace"
    );
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
    // A Markdown link in the text to another decision's file opens that
    // decision (adr-tools writes `[3. ...](0003-....md)`), as does `#3`.
    assert_eq!(e.decision_for_link(0, "0003-keep-decisions-in-the-repository.md"), Some(2));
    assert_eq!(e.decision_for_link(0, "./0002-use-postgresql.md#context"), Some(1));
    assert_eq!(e.decision_for_link(2, "#1"), Some(0));
    assert_eq!(e.decision_for_link(0, "https://example.com/0002-use-postgresql.md"), None);
    assert_eq!(e.decision_for_link(0, "0009-missing.md"), None);
    // Clicking it in the rendered text follows it: the scan starts below
    // the header line, so only the body's link can be hit.
    e.decision = 0;
    for _ in 0..2 {
        t += 0.5;
        frame(&ctx, &mut e, m, Vec::new(), t);
    }
    'body: for y in (150..420).step_by(3) {
        for x in (500..1200).step_by(20) {
            t += 0.5;
            #[expect(clippy::cast_precision_loss, reason = "small test coordinates")]
            frame(&ctx, &mut e, m, click(egui::pos2(x as f32, y as f32), egui::PointerButton::Primary), t);
            if e.decision != 0 {
                break 'body;
            }
        }
    }
    assert_eq!(e.decision, 2, "the body's link to 0003-...md opens decision 3");
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
    // Dynamic views are drawn since dynamic-view merged; Structurizr
    // embeds them like any other diagram.
    match e.embed(m, "checkout") {
        Embed::View(build) => assert!(!build.scene.boxes.is_empty()),
        Embed::Placeholder(t) => panic!("{t}"),
    }
    // Drawn on the shop's Context section: the context view at its fitted
    // size (100% when the column is wide enough, else scaled down to the
    // column with its proportions kept), its boxes painted inside, and the
    // unknown key's placeholder.
    let Embed::View(build) = e.embed(m, "context") else { panic!("context view") };
    #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
    let scene = egui::vec2(build.scene.bounds.width() as f32, build.scene.bounds.height() as f32);
    let shapes = docs_page(&mut e, m, 2, egui::vec2(1600.0, 4000.0));
    let rect = e.drawn_embeds.iter().find(|(k, _)| k == "context").map(|(_, r)| *r).expect("context embed drawn");
    assert!((rect.size() - scene).length() < 0.5, "100%: {:?} vs scene {scene:?}", rect.size());
    let inside = shapes.iter().any(|s| match s {
        egui::Shape::Text(t) => t.galley.text() == "Customer" && rect.contains(t.pos),
        _ => false,
    });
    assert!(inside, "the view's boxes are painted inside the embed");
    let shown = texts(&shapes);
    let unknown = "Embedded view `no-such-view` does not exist in this workspace";
    assert!(shown.iter().any(|s| s == unknown), "the placeholder is drawn: {shown:?}");
    // A narrow window scales the view down to the column.
    let _ = docs_page(&mut e, m, 2, egui::vec2(700.0, 4000.0));
    let narrow = e.drawn_embeds.iter().find(|(k, _)| k == "context").map(|(_, r)| *r).expect("context embed drawn");
    assert!(narrow.width() < scene.x - 1.0, "scaled down: {narrow:?}");
    assert!((narrow.width() / narrow.height() - scene.x / scene.y).abs() < 0.01, "proportions kept");
    // TODO(srs): P3-517's "matches inspection `documentation.embeddedview`"
    // is not checked: that inspection is not built (deferred row
    // "explore embed inspection").
}
