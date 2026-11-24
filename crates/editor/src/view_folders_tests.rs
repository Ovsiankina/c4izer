//! Tests of the view-folders row (SRS 4.9: P5-600..P5-607): the C-level
//! folders of the view list, their fold state, Previous/Next over them,
//! the recap card, and their benchmarks. One `#[test]` per requirement,
//! named after it. State is driven like `view_map_tests`, effects run
//! synchronously; the list is drawn in a windowless egui context
//! (`app::draw_view_list`). Test data uses roles, never personal names.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::model::parse;
use eframe::egui;

use crate::app::{draw_view_list, draw_view_list_with};
use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState};
use crate::state::{Command, Effect, Event, Input, State};
use crate::testutil::{TempDir, bench, debug_build, many_views};
use crate::view_folders::{FolderCache, ViewFolder, folder_of, folders};
use crate::worker::{self, ViewRef};

/// Every folder: views declared out of folder order, a filtered view
/// whose base (C2) is hidden, an image view of a component (C4) and one
/// of a system (Other).
pub(crate) const ESTATE: &str = r#"workspace "Estate" {
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
        kiosk = element "Kiosk" "Hardware" "Shows prices."
        staff -> web "Uses"
        web -> db "Reads"
        cart -> db "Saves"
        web -> books "Posts entries to"
        live = deploymentEnvironment "Live" {
            node = deploymentNode "Node" {
                containerInstance web
            }
        }
    }
    views {
        dynamic shop "checkout" "Checkout" {
            staff -> web "Opens the shop"
        }
        component web "web-components" "Inside the web app" {
            include *
        }
        systemLandscape "estate" "The whole estate" {
            include *
        }
        container shop "shop-containers" "Inside the shop" {
            include *
        }
        image cart "cart-code" {
            image cart.svg
            title "The cart's classes"
        }
        systemContext shop "shop-context" "The shop and its neighbours" {
            include *
        }
        container ledger "ledger-containers" "Inside the ledger" {
            include *
        }
        filtered "ledger-containers" include "Front,Relationship" "ledger-front" "The ledger's front"
        image shop "shop-picture" {
            image shop.svg
            title "The shop, drawn by hand"
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

/// `ESTATE`'s folders and their views, in list order.
const FOLDERS: [(ViewFolder, &[&str]); 5] = [
    (ViewFolder::C1, &["estate", "shop-context"]),
    (ViewFolder::C2, &["shop-containers", "ledger-front"]),
    (ViewFolder::C3, &["web-components"]),
    (ViewFolder::C4, &["cart-code"]),
    (ViewFolder::Other, &["checkout", "shop-picture", "live", "board"]),
];

pub(crate) fn declared(key: &str) -> ViewRef {
    ViewRef::Declared(key.to_owned())
}

// ------------------------------------------------------------ the shell

/// The shell without a window: every effect at once; the UI state saved
/// last is kept.
pub(crate) struct Shell {
    pub(crate) s: State,
    pub(crate) path: PathBuf,
    pub(crate) saved: Option<UiState>,
    _dir: TempDir,
}

impl Shell {
    pub(crate) fn with(text: &str) -> Shell {
        Shell::with_ui(text, UiState::default())
    }

    fn with_ui(text: &str, ui: UiState) -> Shell {
        let dir = TempDir::new("view-folders");
        let path = dir.join("estate.dsl");
        std::fs::write(&path, text).expect("fixture written");
        // The image views' sources, beside the file.
        for name in ["cart.svg", "shop.svg"] {
            std::fs::write(dir.join(name), "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"/>").expect("image written");
        }
        let mut shell = Shell {
            s: State::new(ui),
            path: path.clone(),
            saved: None,
            _dir: dir,
        };
        shell.event(Event::Input(Input::CanvasSize {
            size: ScreenPoint { x: 1200.0, y: 800.0 },
        }));
        shell.cmd(Command::Open(path));
        shell
    }

    fn event(&mut self, event: Event) {
        let effects = self.s.handle(event);
        self.run(effects);
    }

    pub(crate) fn cmd(&mut self, command: Command) {
        self.event(Event::Command(command));
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
                Effect::SaveUiState(ui) => {
                    self.saved = Some(ui);
                    Vec::new()
                }
                _ => Vec::new(),
            };
            self.run(next);
        }
    }

    pub(crate) fn view(&self) -> String {
        self.s.view.as_ref().map(ViewRef::key).unwrap_or_default()
    }

    fn show(&mut self, key: &str) {
        self.cmd(Command::ShowView(declared(key)));
        assert_eq!(self.view(), key);
    }

    /// Previous or Next view, then the view on screen.
    fn step(&mut self, next: bool) -> String {
        self.cmd(if next { Command::NextView } else { Command::PreviousView });
        self.view()
    }
}

fn keys(state: &State) -> Vec<(ViewFolder, Vec<String>)> {
    state
        .view_folders()
        .into_iter()
        .map(|f| (f.folder, f.views.iter().map(|e| e.view.key()).collect()))
        .collect()
}

/// The commands among `events`.
fn commands(events: &[Event]) -> Vec<Command> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Command(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

fn expected() -> Vec<(ViewFolder, Vec<String>)> {
    FOLDERS
        .iter()
        .map(|(f, ks)| (*f, ks.iter().map(|k| (*k).to_owned()).collect()))
        .collect()
}

// ------------------------------------------------------------ the tests

/// P5-600: one function puts each view kind in its folder; a filtered
/// view goes with its base.
#[test]
fn p5_600_folder_of_each_view_kind() {
    let parsed = parse(ESTATE).expect("parses");
    let m = &parsed.model;
    for (folder, ks) in FOLDERS {
        for k in ks {
            assert_eq!(folder_of(m, &declared(k)), folder, "{k}");
        }
    }
    // The hidden base of `ledger-front` is a C2 view too.
    assert_eq!(folder_of(m, &declared("ledger-containers")), ViewFolder::C2);
    // An unwritten default view goes by its kind.
    let scope = m.full_key(m.find_written("web").expect("web")).expect("a key");
    assert_eq!(folder_of(m, &ViewRef::Default { scope, kind: c4rizer_core::model::ViewKind::Component }), ViewFolder::C3);
}

/// P5-601: only non-empty folders, C1..C4 then Other; inside a folder
/// the list's order (declaration, or `structurizr.sort`).
#[test]
fn p5_601_folders_in_order_non_empty_only() {
    let sh = Shell::with(ESTATE);
    assert_eq!(keys(&sh.s), expected());
    // No image view of a component: no C4 folder.
    let without = ESTATE.replace("image cart \"cart-code\" {\n            image cart.svg\n            title \"The cart's classes\"\n        }\n", "");
    assert_ne!(without, ESTATE);
    let sh = Shell::with(&without);
    let shown: Vec<ViewFolder> = sh.s.view_folders().iter().map(|f| f.folder).collect();
    assert_eq!(shown, [ViewFolder::C1, ViewFolder::C2, ViewFolder::C3, ViewFolder::Other]);
    // `structurizr.sort key` orders inside each folder.
    let sorted = ESTATE.replace("    views {\n", "    views {\n        properties {\n            \"structurizr.sort\" \"key\"\n        }\n");
    let sh = Shell::with(&sorted);
    let other = sh.s.view_folders().into_iter().find(|f| f.folder == ViewFolder::Other).expect("Other");
    let ks: Vec<String> = other.views.iter().map(|e| e.view.key()).collect();
    assert_eq!(ks, ["board", "checkout", "live", "shop-picture"]);
    let c1: Vec<String> = sh.s.view_folders()[0].views.iter().map(|e| e.view.key()).collect();
    assert_eq!(c1, ["estate", "shop-context"]);
}

/// P5-606: the list build (one key map per call) puts every view in the
/// folder `folder_of` gives it, filtered views and unwritten default
/// views included.
#[test]
fn p5_606_folders_agree_with_folder_of() {
    for src in [ESTATE.to_owned(), many_views(60)] {
        let sh = Shell::with(&src);
        let model = sh.s.model().expect("a model").clone();
        let mut entries = sh.s.views();
        let scope = model.full_key(model.elements[0].id).expect("a key");
        entries.push(crate::state::ViewEntry {
            view: ViewRef::Default { scope, kind: c4rizer_core::model::ViewKind::Container },
            ..entries[0].clone()
        });
        for f in folders(&model, entries.clone()) {
            for e in &f.views {
                assert_eq!(folder_of(&model, &e.view), f.folder, "{}", e.view.key());
            }
        }
        assert_eq!(folders(&model, entries.clone()).iter().map(|f| f.views.len()).sum::<usize>(), entries.len());
    }
}

/// P5-602: a header per folder then its views, every row one line of
/// `SIZE_ROW`, nothing painted past the panel's edge at 100, 120 and
/// 220 px.
#[test]
fn p5_602_headers_and_rows_fit_narrow_panels() {
    for (name, text) in [
        ("estate", ESTATE.to_owned()),
        ("example", include_str!("../../../tests/data/example.dsl").to_owned()),
        ("landscape_filtered", include_str!("../../../tests/data/landscape_filtered.dsl").to_owned()),
    ] {
        let sh = Shell::with(&text);
        let folders = sh.s.view_folders();
        let views: usize = folders.iter().map(|f| f.views.len()).sum();
        for width in [100.0_f32, 120.0, 220.0] {
            let drawn = draw_view_list(&sh.s, width, 6);
            assert_eq!(drawn.headers, folders.len(), "{name} {width}: one header per folder");
            assert_eq!(drawn.rows.len(), views + folders.len(), "{name} {width}");
            for ((line, height), rect) in drawn.rows.iter().zip(&drawn.rects) {
                assert!(*height <= drawn.row_height + 0.5, "{name} {width}: {line:?} is {height}px high");
                assert!(!line.contains('\n'), "{name} {width}: {line:?}");
                assert!(rect.max.x <= drawn.panel_width + 0.5, "{name} {width}: {line:?} ends at {}", rect.max.x);
            }
            assert!(
                drawn.text_right <= drawn.panel_width + 0.5,
                "{name} {width}: text runs to {} in a {} px panel",
                drawn.text_right,
                drawn.panel_width
            );
            assert!(drawn.panel_width >= width - 1.0, "{name} {width}: panel shrank to {}", drawn.panel_width);
        }
    }
    // The header line: chevron, key, label, count.
    let sh = Shell::with(ESTATE);
    let drawn = draw_view_list(&sh.s, 220.0, 3);
    assert_eq!(drawn.rows[0].0, "▾ C1  Context  02");
    assert!(drawn.rows[1].0.starts_with("estate"), "{:?}", drawn.rows[1]);
    let other = drawn.rows.iter().find(|(l, _)| l.starts_with("▾ Other")).expect("Other header");
    assert_eq!(other.0, "▾ Other  Other  04");
}

/// Twelve component views keyed `Components-service-0` ..
/// `Components-service-11` with no title, a titled container view `api`
/// and a container view with a long key and no title.
fn untitled_keys() -> String {
    use std::fmt::Write as _;
    let mut s = String::from("workspace {\n    model {\n        s = softwareSystem \"Service\" {\n");
    for i in 0..12 {
        let _ = writeln!(s, "            c{i} = container \"Part {i}\" {{\n                k{i} = component \"Piece {i}\"\n            }}");
    }
    s.push_str("        }\n    }\n    views {\n");
    for i in 0..12 {
        let _ = writeln!(s, "        component c{i} \"Components-service-{i}\" {{\n            include *\n        }}");
    }
    s.push_str("        container s \"api\" \"Service parts\" {\n            include *\n        }\n");
    s.push_str("        container s \"a-container-view-key-long-enough-to-fill-the-row\" {\n            include *\n        }\n");
    s.push_str("    }\n}\n");
    s
}

/// P5-602: a view row with no title of its own gives its key the whole
/// row: at 220 px the twelve `Components-service-N` rows each paint a
/// distinct text holding their number; a long untitled key does not
/// widen the key column, so a titled row keeps its title; at 100 and
/// 120 px every row stays one line inside the panel.
#[test]
fn p5_602_untitled_view_key_takes_the_row() {
    let sh = Shell::with(&untitled_keys());
    let drawn = draw_view_list(&sh.s, 220.0, 6);
    let rows: Vec<&str> = drawn.rows.iter().map(|(l, _)| l.trim_end()).filter(|l| l.starts_with("Components-service")).collect();
    assert_eq!(rows.len(), 12, "{:?}", drawn.rows);
    for i in 0..12 {
        let number = format!("-{i}");
        assert_eq!(rows.iter().filter(|l| l.ends_with(&number)).count(), 1, "row {i} reads its number: {rows:?}");
    }
    let mut distinct = rows.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 12, "{rows:?}");
    let api = drawn.rows.iter().find(|(l, _)| l.starts_with("api")).expect("the api row");
    assert!(api.0.contains("Service parts"), "the titled row keeps its title: {:?}", api.0);
    let long = drawn.rows.iter().find(|(l, _)| l.starts_with("a-container")).expect("the long row");
    assert!(!long.0.contains('\n'), "{:?}", long.0);
    for width in [100.0_f32, 120.0, 220.0] {
        let drawn = draw_view_list(&sh.s, width, 6);
        for ((line, height), rect) in drawn.rows.iter().zip(&drawn.rects) {
            assert!(*height <= drawn.row_height + 0.5, "{width}: {line:?} is {height}px high");
            assert!(!line.contains('\n'), "{width}: {line:?}");
            assert!(rect.max.x <= drawn.panel_width + 0.5, "{width}: {line:?} ends at {}", rect.max.x);
        }
        assert!(drawn.text_right <= drawn.panel_width + 0.5, "{width}: text runs to {} in a {} px panel", drawn.text_right, drawn.panel_width);
    }
}

/// P5-603: a click on a header folds it (its rows go), another unfolds
/// it; Enter on a focused header does the same; a folded folder holding
/// the view on screen shows the dot.
#[test]
fn p5_603_header_click_folds_and_dot_marks_current() {
    let mut sh = Shell::with(ESTATE);
    sh.show("estate");
    let before = draw_view_list(&sh.s, 220.0, 2);
    let header = before.rects[0].center();
    let click = vec![
        vec![egui::Event::PointerMoved(header)],
        vec![egui::Event::PointerButton {
            pos: header,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
        vec![egui::Event::PointerButton {
            pos: header,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    ];
    let drawn = draw_view_list_with(&egui::Context::default(), &sh.s, 220.0, click);
    assert_eq!(commands(&drawn.events), [Command::ToggleFolder(ViewFolder::C1)]);
    for e in drawn.events {
        sh.event(e);
    }
    assert!(sh.s.is_folded(ViewFolder::C1));
    let after = draw_view_list(&sh.s, 220.0, 2);
    assert_eq!(after.rows.len(), before.rows.len() - 2, "C1's two rows are hidden");
    assert_eq!(after.rows[0].0, "▸ C1  Context  02 •", "folded, holding the view on screen: the dot");
    assert!(after.rows[1].0.starts_with("▾ C2"), "{:?}", after.rows[1]);
    // Another view: no dot on the folded C1.
    sh.show("web-components");
    assert_eq!(draw_view_list(&sh.s, 220.0, 2).rows[0].0, "▸ C1  Context  02");
    // Keyboard: Tab focuses the first header, Enter toggles it.
    let key = |key: egui::Key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let drawn = draw_view_list_with(&egui::Context::default(), &sh.s, 220.0, vec![vec![], vec![key(egui::Key::Tab)], vec![key(egui::Key::Enter)]]);
    assert_eq!(commands(&drawn.events), [Command::ToggleFolder(ViewFolder::C1)]);
    sh.cmd(Command::ToggleFolder(ViewFolder::C1));
    assert!(!sh.s.is_folded(ViewFolder::C1));
    assert_eq!(draw_view_list(&sh.s, 220.0, 2).rows.len(), before.rows.len());
}

/// P5-604: fold state is per file, saved as `fold` lines and restored
/// when the file is opened again.
#[test]
fn p5_604_fold_state_saved_per_file() {
    let mut sh = Shell::with(ESTATE);
    sh.cmd(Command::ToggleFolder(ViewFolder::Other));
    sh.cmd(Command::ToggleFolder(ViewFolder::C2));
    let saved = sh.saved.clone().expect("the UI state is saved");
    let path = sh.s.doc.as_ref().expect("a file").path.clone();
    let text = saved.to_text();
    assert!(text.contains(&format!("fold\tC2\t{}\n", path.display())), "{text}");
    assert!(text.contains(&format!("fold\tOther\t{}\n", path.display())), "{text}");
    let back = UiState::parse(&text);
    assert_eq!(back, saved, "round trip");
    // Unknown folder names are skipped, as unknown lines are.
    assert!(UiState::parse("fold\tC9\t/x.dsl\n").folded.is_empty());
    // Another file has its own state.
    let other = Shell::with_ui(ESTATE, back.clone());
    assert!(other.s.folded().is_empty(), "a different path: nothing folded");
    // Reopening the same path restores it.
    let mut again = State::new(back);
    let effects = again.handle(Event::Command(Command::Open(path)));
    let mut sh2 = Shell { s: again, path: sh.path.clone(), saved: None, _dir: TempDir::new("view-folders-again") };
    sh2.run(effects);
    assert_eq!(sh2.s.folded(), [ViewFolder::C2, ViewFolder::Other].into_iter().collect());
    // Unfolding the last folder drops the file's entry.
    sh.cmd(Command::ToggleFolder(ViewFolder::Other));
    sh.cmd(Command::ToggleFolder(ViewFolder::C2));
    assert!(sh.s.ui.folded.is_empty());
    assert!(!sh.saved.expect("saved").to_text().contains("fold\t"));
}

/// P5-605: Previous/Next view walk the list as displayed (folder order)
/// and skip the views of folded folders.
#[test]
fn p5_605_previous_next_walk_folders_and_skip_folded() {
    let mut sh = Shell::with(ESTATE);
    // Folder order, not declaration order (the file opens on its first
    // declared view, `checkout`).
    sh.show("estate");
    let mut seen = vec![sh.view()];
    while seen.len() < 12 {
        let next = sh.step(true);
        if seen.last() == Some(&next) {
            break;
        }
        seen.push(next);
    }
    let all: Vec<&str> = FOLDERS.iter().flat_map(|(_, ks)| ks.iter().copied()).collect();
    assert_eq!(seen, all, "Next walks the folders as displayed and stops at the end");
    // Folded C2: skipped both ways.
    sh.show("shop-context");
    sh.cmd(Command::ToggleFolder(ViewFolder::C2));
    assert_eq!(sh.step(true), "web-components");
    assert_eq!(sh.step(false), "shop-context");
    // The view on screen in a folded folder keeps its place: Previous
    // skips the folded `estate`, Next goes on to the open folders.
    sh.cmd(Command::ToggleFolder(ViewFolder::C1));
    assert_eq!(sh.step(false), "shop-context", "the folded folder's other views are skipped");
    assert_eq!(sh.step(true), "web-components");
    assert_eq!(sh.step(false), "web-components", "everything before is folded: the list stops");
}

/// P5-606: the folder list is built once per parse and view, not per
/// frame.
#[test]
fn p5_606_folder_list_built_per_parse_and_view() {
    let mut sh = Shell::with(ESTATE);
    let drawn = draw_view_list(&sh.s, 220.0, 8);
    assert_eq!(drawn.builds, 1, "eight frames, one build");
    let mut cache = FolderCache::default();
    let first = cache.sync(&sh.s, |_| 0.0);
    let again = cache.sync(&sh.s, |_| 0.0);
    assert!(std::sync::Arc::ptr_eq(&first, &again));
    assert_eq!(cache.builds, 1);
    sh.show("live");
    let moved = cache.sync(&sh.s, |_| 0.0);
    assert_eq!(cache.builds, 2, "a view change rebuilds");
    assert!(moved.iter().any(|f| f.views.iter().any(|e| e.current && e.view.key() == "live")));
    // Folding needs no rebuild: the list reads the fold state as it draws.
    sh.cmd(Command::ToggleFolder(ViewFolder::C1));
    cache.sync(&sh.s, |_| 0.0);
    assert_eq!(cache.builds, 2);
}

/// P5-607: hovering a header shows the recap card: the title, then the
/// sentence of issue #10, verbatim.
#[test]
fn p5_607_header_hover_shows_recap() {
    let texts = [
        (ViewFolder::C1, "C1 · System context", "The starting point; how the software system in scope fits into the world around it."),
        (ViewFolder::C2, "C2 · Container", "Zooms into the system; the applications and data stores inside it."),
        (ViewFolder::C3, "C3 · Component", "Zooms into one container; the components inside it."),
        (
            ViewFolder::C4,
            "C4 · Code",
            "Zooms into one component; how it is implemented at the code level (e.g. UML class diagram).",
        ),
        (ViewFolder::Other, "Other", "Supplementary diagrams: dynamic, deployment, custom and image views."),
    ];
    for (folder, title, sentence) in texts {
        assert_eq!(folder.recap_title(), title);
        assert_eq!(folder.recap(), sentence);
    }
    // Drawn: rest the pointer on the C3 header past the tooltip delay.
    let sh = Shell::with(ESTATE);
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let first = draw_view_list_with(&ctx, &sh.s, 220.0, vec![vec![]]);
    let at = first.rows.iter().position(|(l, _)| l.starts_with("▾ C3")).expect("C3 header");
    let pos = first.rects[at].center();
    let mut frames = vec![vec![egui::Event::PointerMoved(pos)]];
    frames.extend((0..60).map(|_| Vec::new()));
    let drawn = draw_view_list_with(&ctx, &sh.s, 220.0, frames);
    assert!(drawn.texts.iter().any(|t| t == "C3 · Component"), "{:?}", drawn.texts);
    let card: String = drawn.texts.iter().filter(|t| !drawn.rows.iter().any(|(l, _)| l.contains(t.as_str()))).cloned().collect::<Vec<_>>().join(" ");
    assert!(card.contains("Zooms into one container"), "{card}");
}

// ------------------------------------------------------------ benchmarks

/// P5-606, design section 9: the folder list build, 50 and 500 views.
#[test]
#[ignore = "perf: run with --release -- --ignored perf_p5"]
fn perf_p5_606_folder_list_build() {
    if debug_build("perf_p5_606_folder_list_build") {
        return;
    }
    // Budgets: about 4x the highest of three medians measured 2026-11-23
    // on a quiet machine after the linear build (8 us, 100 us).
    for (n, budget) in [(50, 50), (500, 450)] {
        let sh = Shell::with(&many_views(n));
        let model = sh.s.model().expect("a model").clone();
        let entries = sh.s.views();
        assert_eq!(entries.len(), n);
        bench(["P5-606", "view folders", "folder list build", &format!("{n} views")], budget, || {
            std::hint::black_box(folders(&model, std::hint::black_box(entries.clone())));
        });
    }
}

/// P5-606, design section 9: one view-list frame with 500 views, median
/// over frames, well under 16 ms.
#[test]
#[ignore = "perf: run with --release -- --ignored perf_p5"]
fn perf_p5_606_view_list_frame() {
    if debug_build("perf_p5_606_view_list_frame") {
        return;
    }
    let sh = Shell::with(&many_views(500));
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let mut cache = FolderCache::default();
    // Warm the folder cache and the font atlas.
    for _ in 0..3 {
        crate::app::view_list_frame(&ctx, &sh.s, 220.0, &mut cache);
    }
    // Budget: about 4x the highest of three medians, 53 us (2026-11-23).
    bench(["P5-606", "view folders", "view list frame", "500 views"], 250, || {
        crate::app::view_list_frame(&ctx, &sh.s, 220.0, &mut cache);
    });
    assert_eq!(cache.builds, 1, "no rebuild per frame");
}
