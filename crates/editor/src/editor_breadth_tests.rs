//! Step-4 contract tests of the editor-breadth row, editor part (SRS 4.6:
//! P2-663..P2-665, P2-700..P2-739), driven headlessly like `state`'s own
//! tests: effects run synchronously with `files` and `worker::serve`. One
//! `#[test]` per requirement, named after it (plan
//! docs/plans/editor-breadth.md). The core tests (P2-701, P2-708..P2-710
//! syntax and edits) are in `crates/core/tests/editor_breadth.rs`.
//!
//! Each `TODO(srs)` names the hole and the reading taken
//! (docs/plans/phase2-decisions.md Part B editor-breadth rows, checked
//! against Structurizr 6.2.4's `diagrams.jsp` and `structurizr-diagram.js`
//! where noted). Test data uses roles, never personal names.

use c4rizer_core::model::ElementKey;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::{GRID_PX, Point, Rect};
use c4rizer_core::model::{Model, PaperSize, Parsed, Routing, ViewKind, parse};
use c4rizer_core::scene::{ItemKey, Scene};

use crate::camera::{Camera, Fit, ScreenPoint, ZOOM_STEP};
use crate::files::{self, FileEvent, UiState, WriteRequest};
use crate::state::{
    Alignment, Axis, Button, CanvasStep, Command, Effect, Event, Input, Key, Modifiers, NavLevel, Overlay, Selected,
    State,
};
use crate::testutil::TempDir;
use crate::worker::{self, ExportFormat, ViewRef};

/// A shop, roles only: two people (one with a URL, a perspective and the
/// tag `Buyer`), a system with two containers, an external bank with a
/// non-HTTP URL, a named relationship with a URL and a perspective. The
/// context view is placed; the container and landscape views are not.
const SHOP: &str = r#"workspace "Shop" "An online shop." {
    model {
        customer = person "Customer" "Buys things." "Buyer" {
            url "https://example.com/customer"
            perspectives {
                "Security" "Signs in with a password."
            }
        }
        clerk = person "Clerk" "Packs orders."
        shop = softwareSystem "Shop" "Sells things." {
            web = container "Web app" "Serves pages." "Rust" {
                cart = component "Cart" "Holds items." "Rust module"
                pay = component "Payments" "Charges cards." "Rust module"
            }
            db = container "Database" "Stores orders." "SQLite" "Database"
        }
        bank = softwareSystem "Bank" "Moves money." "External" {
            url "ftp://bank.example/"
        }
        visits = customer -> shop "Visits" "HTTPS" "Web" {
            url "https://example.com/visits"
            perspectives {
                "Security" "TLS 1.3."
            }
        }
        clerk -> shop "Packs orders with"
        customer -> web "Orders through" "HTTPS"
        web -> db "Reads and writes" "SQL"
        pay -> bank "Charges through" "HTTPS"
        cart -> pay "Checks out with"
    }
    views {
        systemContext shop "context" "The shop and its users" {
            include *
            at customer 300 200
            at clerk 1000 200
            at shop 300 700
            at bank 1000 700
        }
        container shop "containers" "Inside the shop" {
            include *
        }
        systemLandscape "estate" "The estate" {
            include *
        }
        styles {
            element "Database" {
                shape Cylinder
            }
        }
    }
}
"#;

const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
};
const SHIFT: Modifiers = Modifiers {
    shift: true,
    ctrl: false,
    alt: false,
};
const CTRL: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: false,
};
const ALT: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: true,
};

/// The canvas size every test reports.
const CANVAS: ScreenPoint = ScreenPoint { x: 1200.0, y: 800.0 };

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    dir: TempDir,
    writes: Vec<WriteRequest>,
    fullscreen: Vec<bool>,
    urls: Vec<String>,
}

impl Shell {
    fn with(text: &str) -> Shell {
        let dir = TempDir::new("editor-breadth");
        let path = dir.join("shop.dsl");
        std::fs::write(&path, text).expect("fixture written");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
            dir,
            writes: Vec::new(),
            fullscreen: Vec::new(),
            urls: Vec::new(),
        };
        shell.input(Input::CanvasSize { size: CANVAS });
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

    fn run(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            let next = match effect {
                Effect::Read(path) => {
                    let (text, hash) = files::read(&path).expect("read fixture");
                    self.s.handle(Event::File(FileEvent::Loaded { path, text, hash }))
                }
                Effect::Write(request) => {
                    self.writes.push(request.clone());
                    self.s.handle(Event::File(files::write_checked(&request)))
                }
                Effect::Work(request) => match worker::serve(request, &AtomicBool::new(false)) {
                    Some(reply) => self.s.handle(Event::Worker(reply)),
                    None => Vec::new(),
                },
                Effect::Fullscreen(on) => {
                    self.fullscreen.push(on);
                    Vec::new()
                }
                Effect::OpenUrl(url) => {
                    self.urls.push(url);
                    Vec::new()
                }
                Effect::Watch(_) | Effect::Dialog(_) | Effect::SaveUiState(_) => Vec::new(),
            };
            self.run(next);
        }
    }

    fn model(&self) -> &Model {
        &self.s.doc.as_ref().expect("a file").parsed.as_ref().expect("a parse").model
    }

    fn scene(&self) -> &Scene {
        self.s.scene().expect("a scene")
    }

    fn show(&mut self, key: &str) {
        self.cmd(Command::ShowView(declared(key)));
        assert_eq!(self.scene().key, key);
    }

    fn rect(&self, key: &str) -> Rect {
        let id = self.model().find_written(key).expect("element");
        let b = self.scene().boxes_of(id)[0];
        self.scene().boxes[b.0 as usize].rect
    }

    fn screen(&self, key: &str) -> ScreenPoint {
        self.s.camera.to_screen(self.rect(key).centre())
    }

    fn click_at(&mut self, at: ScreenPoint, mods: Modifiers) {
        self.click_button(at, Button::Left, mods);
    }

    fn click_button(&mut self, at: ScreenPoint, button: Button, mods: Modifiers) {
        self.input(Input::PointerDown { at, button, mods });
        self.input(Input::PointerUp { at, button });
    }

    /// The entries of the open context menu.
    fn menu(&self) -> Vec<crate::state::MenuEntry> {
        match &self.s.overlay {
            Some(Overlay::ContextMenu { entries }) => entries.clone(),
            other => panic!("no context menu: {other:?}"),
        }
    }

    fn click(&mut self, key: &str, mods: Modifiers) {
        let at = self.screen(key);
        self.click_at(at, mods);
    }

    fn double_click_at(&mut self, at: ScreenPoint, mods: Modifiers) {
        self.click_at(at, mods);
        self.click_at(at, mods);
        self.input(Input::DoubleClick { at, mods });
    }

    /// A screen point on the line drawing the first relationship from
    /// `source` to `destination`, away from its label.
    fn line_point(&self, source: &str, destination: &str) -> ScreenPoint {
        let model = self.model();
        let (from, to) = (model.find_written(source).expect("source"), model.find_written(destination).expect("destination"));
        let id = model.find_relationship(from, to)[0];
        let line = self
            .scene()
            .lines
            .iter()
            .find(|l| l.relationships.contains(&id))
            .expect("line drawn");
        let (a, b) = (line.points[0], line.points[1]);
        let p = Point::new(a.x + (b.x - a.x) / 5, a.y + (b.y - a.y) / 5);
        self.s.camera.to_screen(p)
    }

    fn hover(&mut self, at: ScreenPoint) {
        self.input(Input::PointerMove { at });
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.path).expect("file")
    }

    fn on_disk(&self) -> Parsed {
        parse(&self.text()).expect("file parses")
    }

    /// The `at` lines of `view` on disk, by element key.
    fn at_lines(&self, view: &str) -> BTreeMap<String, (i32, i32)> {
        let parsed = self.on_disk();
        let m = &parsed.model;
        m.view(view)
            .expect("view on disk")
            .positions
            .iter()
            .map(|a| {
                let key = m.element(a.element.value).key.as_ref().expect("key").value.clone();
                (key, (a.x.value, a.y.value))
            })
            .collect()
    }

    fn external(&mut self, text: &str) {
        std::fs::write(&self.path, text).expect("external write");
        self.event(Event::File(FileEvent::Changed {
            text: text.to_owned(),
            hash: files::hash(text.as_bytes()),
        }));
    }

    fn selection(&self) -> Vec<Selected> {
        self.s.selection.iter().cloned().collect()
    }
}

fn declared(key: &str) -> ViewRef {
    ViewRef::Declared(key.to_owned())
}

fn el(key: &str) -> Selected {
    Selected::Element(ElementKey::from_written(key))
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

// ------------------------------------------------------------ task 01

/// With two or more boxes selected, `l c r t m b h v` align and
/// distribute exactly as the EDIT-20 commands, one undo step each.
// TODO(srs): P2-707, the alignment reference. Reading taken (B
// editor-breadth 4, Structurizr 6.2.4 `alignSelectedElementsLeft`): the
// first box selected.
#[test]
fn p2_707_align_distribute_keys_match_commands() {
    let pairs = [
        (Key::L, Command::Align(Alignment::Left)),
        (Key::C, Command::Align(Alignment::Centre)),
        (Key::R, Command::Align(Alignment::Right)),
        (Key::T, Command::Align(Alignment::Top)),
        (Key::M, Command::Align(Alignment::Middle)),
        (Key::B, Command::Align(Alignment::Bottom)),
        (Key::H, Command::Distribute(Axis::Horizontal)),
        (Key::V, Command::Distribute(Axis::Vertical)),
    ];
    for (key, command) in pairs {
        let select = |sh: &mut Shell| {
            sh.click("clerk", NONE);
            sh.click("customer", SHIFT);
            sh.click("bank", SHIFT);
        };
        let mut by_key = Shell::with(SHOP);
        select(&mut by_key);
        by_key.key(key, NONE);
        let mut by_command = Shell::with(SHOP);
        select(&mut by_command);
        by_command.cmd(command.clone());
        assert_eq!(by_key.text(), by_command.text(), "{key:?}");
        assert_eq!(by_key.s.history.undo.len(), 1, "{key:?}: one step");
        assert_eq!(by_key.s.view, Some(declared("context")));
    }
    // The first selected box is the reference: aligning left puts every
    // box's left edge on the clerk's.
    let mut sh = Shell::with(SHOP);
    sh.click("clerk", NONE);
    sh.click("customer", SHIFT);
    let left = sh.rect("clerk").min.x;
    sh.key(Key::L, NONE);
    assert_eq!(sh.rect("customer").min.x, left);
    assert_eq!(sh.rect("clerk").min.x, left);
}

/// `+` and `-` zoom one `ZOOM_STEP` about the canvas widget's centre,
/// clamped, and write nothing.
#[test]
fn p2_711_plus_minus_zoom_at_canvas_centre() {
    let mut sh = Shell::with(SHOP);
    let centre = ScreenPoint {
        x: CANVAS.x / 2.0,
        y: CANVAS.y / 2.0,
    };
    let before = sh.s.camera;
    let under = before.to_scene(centre);
    sh.key(Key::Plus, NONE);
    assert!(close(sh.s.camera.zoom, (before.zoom * ZOOM_STEP).min(crate::camera::MAX_ZOOM)));
    let after = sh.s.camera.to_scene(centre);
    assert!(close(under.x, after.x) && close(under.y, after.y), "{under:?} {after:?}");
    sh.key(Key::Minus, NONE);
    assert!(close(sh.s.camera.zoom, before.zoom));
    for _ in 0..100 {
        sh.key(Key::Minus, NONE);
    }
    assert!(close(sh.s.camera.zoom, crate::camera::MIN_ZOOM));
    assert!(sh.writes.is_empty());
}

/// A line under the pointer takes Up, Down, `r` and `j`: they change
/// that line, never the view and never the selection's alignment.
// TODO(srs): P2-712, the key order. Reading taken (B editor-breadth 2):
// quick navigation, else the hovered line, else two or more selected
// boxes, else the viewer.
#[test]
fn p2_712_hovered_line_takes_r_and_arrows() {
    let mut sh = Shell::with(SHOP);
    sh.click("clerk", NONE);
    sh.click("customer", SHIFT);
    let at = sh.line_point("clerk", "shop");
    sh.hover(at);
    let before = sh.text();
    sh.key(Key::ArrowUp, NONE);
    assert_eq!(sh.s.view, Some(declared("context")), "Up moved the label, not the view");
    sh.key(Key::R, NONE);
    let after = parse(&sh.text()).expect("parses").model;
    let line = &after.view("context").expect("view").lines;
    assert_eq!(line.len(), 1, "one `line` line for the hovered line");
    assert!(line[0].routing.is_some() && line[0].position.is_some());
    // Nothing but the `line` line changed: no box moved.
    let now = sh.text();
    let added: Vec<&str> = now.lines().filter(|l| !before.contains(l)).collect();
    assert_eq!(added.len(), 1, "{added:?}");
    assert!(added[0].trim_start().starts_with("line "), "{added:?}");
}

/// Fix round (P2-712, P2-708): once the pointer has left the canvas,
/// nothing is hovered and Up, Down, `r` and `j` take their view-level
/// meaning (P2-713), even right after a write the hovered line made.
#[test]
fn p2_712_pointer_off_canvas_hovers_nothing() {
    let mut sh = Shell::with(SHOP);
    let at = sh.line_point("clerk", "shop");
    sh.hover(at);
    sh.key(Key::R, NONE);
    assert_eq!(sh.writes.len(), 1, "r over the line wrote its routing");
    // The line runs on past the canvas's right edge; the pointer follows
    // it there (the shell reports moves outside the rect while a button
    // is down, or from a stale frame).
    let edge = sh.line_point("clerk", "shop");
    sh.s.camera.pan_by(ScreenPoint { x: CANVAS.x + 20.0 - edge.x, y: 0.0 });
    let outside = sh.line_point("clerk", "shop");
    assert!(outside.x > CANVAS.x);
    sh.hover(outside);
    assert_eq!(sh.s.hover, None, "off the canvas nothing is hovered");
    sh.key(Key::J, NONE);
    assert_eq!(sh.writes.len(), 1, "j did not touch the line");
    sh.key(Key::ArrowDown, NONE);
    assert_eq!(sh.s.view, Some(declared("containers")), "Down opened the next view");
    assert_eq!(sh.writes.len(), 1);
}

/// Fix round (P2-712): `PointerLeft` (pointer gone, or over a panel or
/// overlay) clears the hover for good: the re-hit after a write finds
/// nothing, and the arrows navigate views.
#[test]
fn p2_712_pointer_left_survives_a_rebuild() {
    let mut sh = Shell::with(SHOP);
    let at = sh.line_point("clerk", "shop");
    sh.hover(at);
    sh.input(Input::PointerLeft);
    // A write from elsewhere (an external edit) rebuilds the scene.
    let text = sh.text().replace("at bank 1000 700", "at bank 1000 760");
    sh.external(&text);
    assert_eq!(sh.s.hover, None);
    sh.key(Key::ArrowDown, NONE);
    assert_eq!(sh.s.view, Some(declared("containers")));
}

/// With one box selected, `l c r t m b h v` keep their viewer meaning
/// (`c` fits content, `h` fits height, `l` opens the navigator) and
/// never align.
#[test]
fn p2_712_single_selection_leaves_keys_to_viewer() {
    let mut sh = Shell::with(SHOP);
    sh.click("customer", NONE);
    let text = sh.text();
    sh.s.camera = Camera::default();
    sh.key(Key::C, NONE);
    assert_eq!(Some(sh.s.camera), crate::camera::fit(sh.scene().bounds, CANVAS, Fit::Content));
    sh.key(Key::H, NONE);
    assert_eq!(Some(sh.s.camera), crate::camera::fit(sh.scene().bounds, CANVAS, Fit::Height));
    sh.key(Key::L, NONE);
    assert_eq!(sh.s.overlay, Some(Overlay::Navigator));
    sh.key(Key::Escape, NONE);
    assert_eq!(sh.s.overlay, None);
    for k in [Key::R, Key::T, Key::M, Key::B, Key::V] {
        sh.key(k, NONE);
    }
    assert_eq!(sh.text(), text, "no key aligned or distributed");
}

/// No key of this row acts while a shell text field has focus or the
/// in-place editor is open (EDIT-5).
#[test]
fn p2_712_keys_ignored_with_text_focus_or_text_edit() {
    let mut sh = Shell::with(SHOP);
    let camera = sh.s.camera;
    for key in [Key::Plus, Key::D, Key::M, Key::T, Key::N, Key::I, Key::ArrowDown, Key::B, Key::W] {
        sh.input(Input::KeyDown {
            key,
            mods: NONE,
            text_focus: true,
        });
    }
    assert_eq!(sh.s.camera, camera);
    assert_eq!(sh.s.aids, crate::state::ViewAids::default());
    assert_eq!(sh.s.view, Some(declared("context")));
    sh.click("customer", CTRL);
    sh.key(Key::Enter, NONE);
    assert!(sh.s.text_edit.is_some());
    sh.key(Key::D, NONE);
    sh.key(Key::ArrowDown, NONE);
    assert!(sh.s.aids.descriptions);
    assert_eq!(sh.s.view, Some(declared("context")));
}

/// Ctrl+Z, Ctrl+Shift+Z, Ctrl+Y, Backspace, Enter, F2 and Escape keep
/// their EDIT-16/19/28 meaning; Alt+Left is Back (P4-114).
#[test]
fn p2_712_chords_keep_meaning() {
    let mut sh = Shell::with(SHOP);
    sh.click("clerk", NONE);
    sh.click("shop", SHIFT);
    sh.key(Key::T, NONE);
    let aligned = sh.text();
    assert_ne!(aligned, SHOP);
    sh.key(Key::Z, CTRL);
    assert_eq!(sh.text(), SHOP, "Ctrl+Z undoes");
    sh.key(Key::Z, Modifiers { shift: true, ..CTRL });
    assert_eq!(sh.text(), aligned, "Ctrl+Shift+Z redoes");
    sh.key(Key::Z, CTRL);
    sh.key(Key::Y, CTRL);
    assert_eq!(sh.text(), aligned, "Ctrl+Y redoes");
    sh.key(Key::Escape, NONE);
    assert!(sh.s.selection.is_empty(), "Escape deselects");
    sh.click("customer", CTRL);
    sh.key(Key::F2, NONE);
    assert!(sh.s.text_edit.is_some(), "F2 edits");
    sh.key(Key::Escape, NONE);
    sh.show("containers");
    // P4-114: Alt+Left is Back now, to the view shown before.
    sh.key(Key::ArrowLeft, ALT);
    assert_eq!(sh.s.view, Some(declared("context")), "Alt+Left goes back");
    sh.show("containers");
    // The workspace crumb opens the first landscape view (P2-305).
    sh.key(Key::Backspace, NONE);
    assert_eq!(sh.s.view, Some(declared("estate")), "Backspace goes up");
}

// ------------------------------------------------------------ task 02

/// `w`, `h` and `c` fit the scene's width, height or both, centred,
/// with the EDIT-30 margin, never above 100%, writing nothing.
#[test]
fn p2_705_fit_width_height_content() {
    // Amended by P2-738: 1100 wide, so the width fit stays above the
    // 9px text floor.
    let bounds = Rect::from_corners(Point::new(0, 0), Point::new(1100, 1000));
    let canvas = ScreenPoint { x: 1000.0, y: 1000.0 };
    let w = crate::camera::fit(bounds, canvas, Fit::Width).expect("fits");
    assert!(close(w.zoom, (1000.0 - 40.0) / 1100.0), "{w:?}");
    let h = crate::camera::fit(bounds, canvas, Fit::Height).expect("fits");
    assert!(close(h.zoom, 960.0 / 1000.0), "{h:?}");
    let c = crate::camera::fit(bounds, canvas, Fit::Content).expect("fits");
    assert!(close(c.zoom, w.zoom.min(h.zoom)));
    for cam in [w, h, c] {
        let mid = cam.to_screen(bounds.centre());
        assert!(close(mid.x, 500.0) && close(mid.y, 500.0), "centred: {mid:?}");
    }
    let small = Rect::from_corners(Point::new(0, 0), Point::new(100, 100));
    assert!(close(crate::camera::fit(small, canvas, Fit::Width).expect("fits").zoom, 1.0));
    let mut sh = Shell::with(SHOP);
    for (key, how) in [(Key::W, Fit::Width), (Key::H, Fit::Height), (Key::C, Fit::Content)] {
        sh.s.camera = Camera::default();
        sh.key(key, NONE);
        assert_eq!(Some(sh.s.camera), crate::camera::fit(sh.scene().bounds, CANVAS, how));
    }
    assert!(sh.writes.is_empty());
}

/// `c` ("zoom to fit content") is EDIT-30's Fit view.
#[test]
fn p2_705_c_equals_edit_30_fit_view() {
    let mut sh = Shell::with(SHOP);
    sh.s.camera = Camera::default();
    sh.key(Key::C, NONE);
    let by_key = sh.s.camera;
    sh.s.camera = Camera::default();
    sh.cmd(Command::Fit(Fit::Content));
    assert_eq!(sh.s.camera, by_key);
    assert_eq!(by_key, Camera::fit(sh.scene().bounds, CANVAS));
}

/// Fitting an empty scene keeps the camera (no division by zero).
#[test]
fn p2_705_fit_empty_scene_keeps_camera() {
    let empty = Rect::from_corners(Point::new(40, 40), Point::new(40, 40));
    for how in [Fit::Width, Fit::Height, Fit::Content] {
        assert_eq!(crate::camera::fit(empty, CANVAS, how), None);
        assert_eq!(crate::camera::fit(Rect::default(), ScreenPoint::default(), how), None);
    }
}

/// The Fullscreen command toggles fullscreen and keeps view, zoom, pan
/// and selection; F11 does the same; `f` stays EDIT-30's Fit view.
// TODO(srs): P2-706, the key. Reading taken (B editor-breadth 1): F11 and
// a View-menu item.
#[test]
fn p2_706_fullscreen_command_toggles_and_keeps_view() {
    let mut sh = Shell::with(SHOP);
    sh.click("customer", CTRL);
    let (view, camera, selection) = (sh.s.view.clone(), sh.s.camera, sh.selection());
    sh.cmd(Command::Fullscreen);
    assert_eq!(sh.fullscreen, [true]);
    assert!(sh.s.aids.fullscreen);
    sh.key(Key::F11, NONE);
    assert_eq!(sh.fullscreen, [true, false]);
    assert!(!sh.s.aids.fullscreen);
    assert_eq!((sh.s.view.clone(), sh.s.camera, sh.selection()), (view, camera, selection));
    assert!(sh.writes.is_empty());
}

// ------------------------------------------------------------ task 03

/// Up and Right open the previous view of the view list, Down and Left
/// the next, as P2-713 is written; at an end the list stops.
// TODO(srs): P2-713, the ends. Reading taken (Structurizr 6.2.4
// diagrams.jsp: `navigateToPreviousDiagram` stops at index 0): no wrap;
// B editor-breadth 9 proposed wrapping, Structurizr's code wins. The
// Left/Right mapping follows the SRS sentence, not diagrams.jsp (fix
// round, docs/deferred.md).
#[test]
fn p2_713_previous_next_follow_view_list() {
    let mut sh = Shell::with(SHOP);
    assert_eq!(sh.s.view, Some(declared("context")));
    // Fix round: the SRS sentence as written, Up or Right previous, Down
    // or Left next.
    sh.key(Key::ArrowDown, NONE);
    assert_eq!(sh.s.view, Some(declared("containers")));
    sh.key(Key::ArrowLeft, NONE);
    assert_eq!(sh.s.view, Some(declared("estate")), "Left opens the next view");
    sh.key(Key::ArrowDown, NONE);
    assert_eq!(sh.s.view, Some(declared("estate")), "stops at the end");
    sh.key(Key::ArrowRight, NONE);
    assert_eq!(sh.s.view, Some(declared("containers")), "Right opens the previous view");
    sh.key(Key::ArrowUp, NONE);
    assert_eq!(sh.s.view, Some(declared("context")));
    sh.cmd(Command::NextView);
    sh.cmd(Command::PreviousView);
    assert_eq!(sh.s.view, Some(declared("context")));
    sh.cmd(Command::PreviousView);
    assert_eq!(sh.s.view, Some(declared("context")), "stops at the start");
    sh.cmd(Command::NextView);
    assert_eq!(sh.s.view, Some(declared("containers")));
    assert!(sh.writes.is_empty());
}

/// `b` opens the view shown before the current one in this session,
/// not the parent crumb.
#[test]
fn p2_714_b_returns_to_last_viewed() {
    let mut sh = Shell::with(SHOP);
    sh.show("estate");
    sh.show("containers");
    sh.key(Key::B, NONE);
    assert_eq!(sh.s.view, Some(declared("estate")), "the last viewed, not the crumb");
    sh.key(Key::B, NONE);
    assert_eq!(sh.s.view, Some(declared("context")));
    sh.cmd(Command::Back);
    assert_eq!(sh.s.view, Some(declared("context")), "nothing before the first view");
}

/// A Space tap opens quick navigation; Space held during a drag pans and
/// never opens it.
#[test]
fn p2_715_space_tap_opens_quick_nav_space_drag_pans() {
    let mut sh = Shell::with(SHOP);
    sh.input(Input::KeyDown {
        key: Key::Space,
        mods: NONE,
        text_focus: false,
    });
    let at = ScreenPoint { x: 50.0, y: 50.0 };
    let pan = sh.s.camera.pan;
    sh.input(Input::PointerDown {
        at,
        button: Button::Left,
        mods: NONE,
    });
    sh.input(Input::PointerMove {
        at: ScreenPoint { x: 52.0, y: 50.0 },
    });
    sh.input(Input::PointerUp {
        at: ScreenPoint { x: 52.0, y: 50.0 },
        button: Button::Left,
    });
    sh.input(Input::KeyUp { key: Key::Space });
    assert_ne!(sh.s.camera.pan, pan, "a drag of any length pans");
    assert_eq!(sh.s.overlay, None, "a drag never opens quick navigation");
    sh.key(Key::Space, NONE);
    assert!(matches!(sh.s.overlay, Some(Overlay::QuickNavigation { .. })));
}

/// Quick navigation lists the views matching the typed text (key or
/// title, case-insensitive substring); Enter opens the highlighted one,
/// Escape closes and changes nothing; while open it takes every key.
// TODO(srs): P2-715, the matching rule. Reading taken (B editor-breadth
// 9): a case-insensitive substring of key or title.
#[test]
fn p2_715_quick_navigation_filters_opens_and_escapes() {
    let mut sh = Shell::with(SHOP);
    sh.cmd(Command::QuickNavigation);
    let all: Vec<String> = sh.s.quick_nav_entries().iter().map(|e| e.view.key()).collect();
    assert_eq!(all, ["context", "containers", "estate"]);
    sh.event(Event::Text("INSIDE".to_owned()));
    let hits: Vec<String> = sh.s.quick_nav_entries().iter().map(|e| e.view.key()).collect();
    assert_eq!(hits, ["containers"]);
    sh.key(Key::D, NONE);
    assert!(sh.s.aids.descriptions, "keys go to the list");
    sh.key(Key::Enter, NONE);
    assert_eq!(sh.s.view, Some(declared("containers")));
    assert_eq!(sh.s.overlay, None);
    sh.cmd(Command::QuickNavigation);
    sh.event(Event::Text("est".to_owned()));
    sh.key(Key::Escape, NONE);
    assert_eq!(sh.s.overlay, None);
    assert_eq!(sh.s.view, Some(declared("containers")), "Escape changes nothing");
}

// ------------------------------------------------------------ task 04

/// `structurizr.sort` orders the view list: `key` by key, `created` and
/// none by declaration, `type` by kind then key.
// TODO(srs): P2-716, the type order and the default. Reading taken (B
// editor-breadth 8): no property keeps declaration order; `type` is
// landscape, context, container, component, filtered, custom, image,
// dynamic, deployment, then key.
#[test]
fn p2_716_structurizr_sort_orders_view_list() {
    let with = |sort: &str| {
        SHOP.replacen(
            "        styles {",
            &format!("        properties {{\n            \"structurizr.sort\" \"{sort}\"\n        }}\n        styles {{"),
            1,
        )
    };
    let order = |text: &str| -> Vec<String> {
        let sh = Shell::with(text);
        sh.s.views().iter().map(|v| v.view.key()).collect()
    };
    assert_eq!(order(SHOP), ["context", "containers", "estate"]);
    assert_eq!(order(&with("key")), ["containers", "context", "estate"]);
    assert_eq!(order(&with("created")), ["context", "containers", "estate"]);
    assert_eq!(order(&with("type")), ["estate", "context", "containers"]);
    // P2-713 follows the sorted list.
    let mut sh = Shell::with(&with("type"));
    sh.show("estate");
    sh.key(Key::ArrowDown, NONE);
    assert_eq!(sh.s.view, Some(declared("context")));
}

/// A file with a `default` view opens on it when nothing is remembered;
/// a remembered view wins, and the EDIT-27 fallback prefers `default`.
// TODO(srs): P2-717. Reading taken (B editor-breadth 10): a remembered
// view beats `default`, and EDIT-27's fallback prefers it.
#[test]
fn p2_717_default_view_opens_when_none_remembered() {
    let text = SHOP.replacen(
        "        systemLandscape \"estate\" \"The estate\" {\n            include *\n",
        "        systemLandscape \"estate\" \"The estate\" {\n            include *\n            default\n",
        1,
    );
    let mut sh = Shell::with(&text);
    assert_eq!(sh.s.view, Some(declared("estate")));
    sh.show("containers");
    // Reopening remembers the view (EDIT-13).
    let path = sh.path.clone();
    let ui = sh.s.ui.clone();
    sh.s = State::new(ui);
    sh.input(Input::CanvasSize { size: CANVAS });
    sh.cmd(Command::Open(path));
    assert_eq!(sh.s.view, Some(declared("containers")));
    // The open view disappears: the fallback is the default view.
    let gone = text.replacen(
        "        container shop \"containers\" \"Inside the shop\" {\n            include *\n        }\n",
        "",
        1,
    );
    sh.external(&gone);
    assert_eq!(sh.s.view, Some(declared("estate")));
}

// ------------------------------------------------------------ task 05

/// Auto-place rewrites every box's `at` line with fresh placement.
#[test]
fn p2_700_auto_place_rewrites_every_at_line() {
    let crowded = SHOP.replacen("at clerk 1000 200", "at clerk 340 240", 1);
    let mut sh = Shell::with(&crowded);
    sh.cmd(Command::AutoPlace);
    let at = sh.at_lines("context");
    assert_eq!(at.len(), 4);
    let model = parse(&crowded).expect("parses").model;
    let mut unplaced = crowded.clone();
    for line in ["at customer 300 200", "at clerk 340 240", "at shop 300 700", "at bank 1000 700"] {
        unplaced = unplaced.replacen(&format!("            {line}\n"), "", 1);
    }
    let fresh = parse(&unplaced).expect("parses").model;
    let plan = c4rizer_core::view::plan(&fresh, fresh.view("context").expect("view"));
    let centres = c4rizer_core::place::place(&plan, &c4rizer_core::place::Keep::default());
    for b in &plan.boxes {
        let key = &fresh.element(b.element).key.as_ref().expect("key").value;
        let c = centres[b.id.0 as usize];
        assert_eq!(at[key], (c.x * GRID_PX, c.y * GRID_PX), "{key}");
    }
    assert!(model.view("context").is_some());
}

/// Fix round (P2-700): after Auto-place the camera fits the re-placed
/// drawing, so it never ends up off screen.
#[test]
fn p2_700_auto_place_fits_camera_to_content() {
    let crowded = SHOP.replacen("at clerk 1000 200", "at clerk 340 240", 1);
    let mut sh = Shell::with(&crowded);
    sh.s.camera.pan_by(ScreenPoint { x: 5000.0, y: -4000.0 });
    sh.cmd(Command::AutoPlace);
    assert_eq!(sh.writes.len(), 1);
    assert_eq!(sh.s.camera, Camera::fit(sh.scene().bounds, CANVAS), "fitted to the new scene");
}

/// Auto-place is one undo step.
#[test]
fn p2_700_auto_place_one_undo_step() {
    let mut sh = Shell::with(SHOP);
    sh.cmd(Command::AutoPlace);
    assert_eq!(sh.s.history.undo.len(), 1);
    sh.cmd(Command::Undo);
    assert_eq!(sh.text(), SHOP);
}

/// Auto-place twice on an unchanged file leaves it byte-identical.
#[test]
fn p2_700_auto_place_twice_is_byte_identical() {
    let mut sh = Shell::with(SHOP);
    sh.cmd(Command::AutoPlace);
    let once = sh.text();
    sh.cmd(Command::AutoPlace);
    assert_eq!(sh.text(), once);
}

/// In a default view not yet written, auto-place writes the view first,
/// in the same batch; one undo removes both.
#[test]
fn p2_700_auto_place_in_default_view_writes_view_first() {
    let mut sh = Shell::with(SHOP);
    sh.cmd(Command::ShowView(ViewRef::Default {
        scope: ElementKey::from_written("web"),
        kind: ViewKind::Component,
    }));
    sh.cmd(Command::AutoPlace);
    assert_eq!(sh.writes.len(), 1, "one write");
    let at = sh.at_lines("web-component");
    assert_eq!(at.len(), sh.scene().boxes.len());
    sh.cmd(Command::Undo);
    assert_eq!(sh.text(), SHOP);
}

// ------------------------------------------------------------ task 06

/// Decrease, Auto and Increase step through the size list (ordered by
/// area) and write the `paperSize` line, one undo step each.
// TODO(srs): P2-701 (a). Reading taken (B editor-breadth 5a): ordered by
// area. Structurizr 6.2.4 has dropped paper sizes for free dimensions
// (`structurizr-diagram.js` `increasePageSize`); the list stays.
#[test]
fn p2_701_canvas_size_steps_and_auto() {
    let mut sh = Shell::with(SHOP);
    let size = |sh: &Shell| sh.on_disk().model.view("context").and_then(|v| v.paper_size).map(|p| p.value);
    sh.cmd(Command::Canvas {
        step: CanvasStep::Auto,
        keep_positions: false,
    });
    let auto = size(&sh).expect("auto writes a size");
    let (w, h) = auto.size_px();
    let b = sh.scene().bounds;
    assert!(b.width() <= w && b.height() <= h, "holds the scene");
    sh.cmd(Command::Canvas {
        step: CanvasStep::Increase,
        keep_positions: true,
    });
    let bigger = size(&sh).expect("a size");
    let area = |p: PaperSize| i64::from(p.size_px().0) * i64::from(p.size_px().1);
    assert!(area(bigger) >= area(auto) && bigger != auto);
    sh.cmd(Command::Canvas {
        step: CanvasStep::Decrease,
        keep_positions: true,
    });
    assert_eq!(size(&sh), Some(auto));
    assert_eq!(sh.s.history.undo.len(), 3);
    // The smallest size cannot shrink.
    let mut small = Shell::with(&SHOP.replacen("            at bank 1000 700\n", "            at bank 1000 700\n            paperSize A6_Portrait\n", 1));
    small.cmd(Command::Canvas {
        step: CanvasStep::Decrease,
        keep_positions: true,
    });
    assert!(small.writes.is_empty());
}

/// Decrease and Increase move every box by one shared grid offset that
/// keeps the drawing centred on the new canvas, in the same step; with
/// Alt every box keeps its position.
// TODO(srs): P2-702, Auto. Reading taken (B editor-breadth 5d, and
// Structurizr 6.2.4 `autoPageSize` calls `centreDiagram`): Auto
// re-centres too.
#[test]
fn p2_702_resize_keeps_drawing_centred_alt_keeps_positions() {
    let mut sh = Shell::with(SHOP);
    let before = sh.at_lines("context");
    sh.cmd(Command::Canvas {
        step: CanvasStep::Increase,
        keep_positions: true,
    });
    assert_eq!(sh.at_lines("context"), before, "Alt keeps positions");
    sh.cmd(Command::Canvas {
        step: CanvasStep::Increase,
        keep_positions: false,
    });
    let size = sh.on_disk().model.view("context").and_then(|v| v.paper_size).expect("size").value;
    let (w, h) = size.size_px();
    let b = sh.scene().bounds;
    let (cx, cy) = (b.min.x + b.max.x, b.min.y + b.max.y);
    assert!((cx - w).abs() <= 2 * GRID_PX && (cy - h).abs() <= 2 * GRID_PX, "centred: {b:?} on {w}x{h}");
    let after = sh.at_lines("context");
    let d: Vec<(i32, i32)> = before.iter().map(|(k, a)| (after[k].0 - a.0, after[k].1 - a.1)).collect();
    assert!(d.windows(2).all(|p| p[0] == p[1]), "one shared offset: {d:?}");
    assert_eq!(d[0].0 % GRID_PX, 0);
    sh.cmd(Command::Undo);
    assert_eq!(sh.at_lines("context"), before, "one step with the size");
}

/// The canvas size is visible: with a `paperSize` the page under the
/// drawing is the paper from (0, 0), outlined, in the export background;
/// on auto there is no paper (found in the real-window check, where a
/// size step changed nothing on screen).
#[test]
fn p2_701_canvas_shows_paper_extent() {
    use crate::canvas::Primitive;
    let mut sh = Shell::with(SHOP);
    assert_eq!(sh.s.paper_rect(), None, "auto: no paper");
    sh.cmd(Command::Canvas {
        step: CanvasStep::Increase,
        keep_positions: false,
    });
    let size = sh.on_disk().model.view("context").and_then(|v| v.paper_size).expect("size").value;
    let (w, h) = size.size_px();
    let paper = sh.s.paper_rect().expect("paper");
    assert_eq!(paper, Rect::from_corners(Point::new(0, 0), Point::new(w, h)));
    let list = sh.s.draw_list();
    let prims = crate::canvas::page(&list, Some(paper));
    let corner = list.camera.to_screen(paper.max);
    assert!(prims.iter().any(|p| matches!(p, Primitive::Polygon { fill: Some(_), points, .. }
        if points.iter().any(|q| (q.x - corner.x).abs() < 1e-3 && (q.y - corner.y).abs() < 1e-3))));
    assert!(prims.iter().any(|p| matches!(p, Primitive::Polygon { fill: None, stroke: Some(_), .. })), "outlined");
}

/// editor-ux-01 (OR-1): on a view with a canvas size the page is ONE
/// rectangle, during and after a drag of a box out of the paper: a
/// single fill and a single outline over the same corners, spanning the
/// paper and the scene bounds, never two stacked rectangles.
#[test]
fn p2_701_page_is_one_rectangle_while_a_box_leaves_the_paper() {
    use crate::canvas::Primitive;
    let mut sh = Shell::with(SHOP);
    sh.cmd(Command::Canvas {
        step: CanvasStep::Increase,
        keep_positions: false,
    });
    let paper = sh.s.paper_rect().expect("paper");
    let check = |sh: &Shell, when: &str| {
        let list = sh.s.draw_list();
        let prims = crate::canvas::page(&list, Some(paper));
        let polys: Vec<&Vec<ScreenPoint>> = prims
            .iter()
            .map(|p| match p {
                Primitive::Polygon { points, .. } => points,
                other => panic!("{when}: not a polygon: {other:?}"),
            })
            .collect();
        assert_eq!(polys.len(), 2, "{when}: one fill and one outline");
        assert_eq!(polys[0], polys[1], "{when}: same rectangle");
        let b = sh.scene().bounds;
        let want = Rect::from_corners(
            Point::new(paper.min.x.min(b.min.x), paper.min.y.min(b.min.y)),
            Point::new(paper.max.x.max(b.max.x), paper.max.y.max(b.max.y)),
        );
        let (a, z) = (list.camera.to_screen(want.min), list.camera.to_screen(want.max));
        let xs = polys[0].iter().map(|p| p.x);
        let ys = polys[0].iter().map(|p| p.y);
        assert!((xs.clone().fold(f32::INFINITY, f32::min) - a.x).abs() < 1e-3, "{when}");
        assert!((xs.fold(f32::NEG_INFINITY, f32::max) - z.x).abs() < 1e-3, "{when}");
        assert!((ys.clone().fold(f32::INFINITY, f32::min) - a.y).abs() < 1e-3, "{when}");
        assert!((ys.fold(f32::NEG_INFINITY, f32::max) - z.y).abs() < 1e-3, "{when}");
        want
    };
    let inside = check(&sh, "at rest");
    assert_eq!(inside, paper, "drawing inside: the page is the paper");
    let from = sh.screen("shop");
    sh.input(Input::PointerDown {
        at: from,
        button: Button::Left,
        mods: NONE,
    });
    let to = ScreenPoint {
        x: from.x + (f32::from(i16::try_from(paper.width()).unwrap_or(0)) + 600.0) * sh.s.camera.zoom,
        y: from.y,
    };
    sh.input(Input::PointerMove { at: to });
    let mid = check(&sh, "mid drag");
    assert!(mid.max.x > paper.max.x, "the page grew to hold the box");
    assert_eq!(mid.min, paper.min, "and only on the side the box left by");
    sh.input(Input::PointerUp { at: to, button: Button::Left });
    check(&sh, "after the drop");
}

/// A canvas size step that re-centres the boxes keeps the drawing where
/// it was on screen: the camera follows the shared offset (found in the
/// real-window check, where the drawing jumped off screen).
#[test]
fn p2_702_canvas_step_keeps_drawing_on_screen() {
    let mut sh = Shell::with(SHOP);
    let screen_of_shop = |sh: &Shell| {
        let model = parse(&sh.text()).expect("parses").model;
        let id = model.find_written("shop").expect("shop");
        let b = sh.scene().boxes.iter().find(|b| b.element == id).expect("drawn").rect;
        sh.s.camera.to_screen(b.min)
    };
    let before = screen_of_shop(&sh);
    for step in [CanvasStep::Increase, CanvasStep::Increase, CanvasStep::Decrease, CanvasStep::Auto] {
        sh.cmd(Command::Canvas {
            step,
            keep_positions: false,
        });
        let now = screen_of_shop(&sh);
        assert!((now.x - before.x).abs() < 0.5 && (now.y - before.y).abs() < 0.5, "{step:?}: {before:?} -> {now:?}");
    }
}

/// The magnet moves each box lying partly outside the canvas to the
/// nearest grid position inside it.
// TODO(srs): P2-703, "nearest" and auto. Reading taken (B editor-breadth
// 5d): nearest inside; on auto the scene bounds are the canvas.
#[test]
fn p2_703_magnet_brings_boxes_inside_canvas() {
    let text = SHOP.replacen("at bank 1000 700", "at bank 3400 700", 1).replacen(
        "            at bank 3400 700\n",
        "            at bank 3400 700\n            paperSize A5_Landscape\n",
        1,
    );
    let mut sh = Shell::with(&text);
    let (w, h) = PaperSize::A5Landscape.size_px();
    sh.cmd(Command::BringOntoCanvas);
    let r = sh.rect("bank");
    assert!(r.min.x >= 0 && r.max.x <= w && r.min.y >= 0 && r.max.y <= h, "{r:?}");
    assert!(r.max.x > w - 2 * GRID_PX, "nearest: against the right edge {r:?}");
    let at = sh.at_lines("context");
    assert_eq!(at["customer"], (300, 200), "boxes inside stay");
    assert_eq!(sh.s.history.undo.len(), 1);
}

/// The magnet writes nothing when every box is inside.
#[test]
fn p2_703_magnet_all_inside_writes_nothing() {
    let mut sh = Shell::with(&SHOP.replacen("            at bank 1000 700\n", "            at bank 1000 700\n            paperSize A3_Landscape\n", 1));
    sh.cmd(Command::BringOntoCanvas);
    assert!(sh.writes.is_empty());
    let mut auto = Shell::with(SHOP);
    auto.cmd(Command::BringOntoCanvas);
    assert!(auto.writes.is_empty(), "auto: the scene bounds hold every box");
}

/// Centre moves the selection, else every box, by one grid offset that
/// puts their bounding rectangle's centre on the canvas centre.
// TODO(srs): P2-704, auto. Reading taken (B editor-breadth 5d): the scene
// bounds' centre.
#[test]
fn p2_704_centre_diagram_and_selection() {
    let text = SHOP.replacen("            at bank 1000 700\n", "            at bank 1000 700\n            paperSize A3_Landscape\n", 1);
    let (w, h) = PaperSize::A3Landscape.size_px();
    let mut sh = Shell::with(&text);
    sh.cmd(Command::Centre);
    let boxes = ["customer", "clerk", "shop", "bank"].map(|k| sh.rect(k));
    let union = boxes
        .iter()
        .skip(1)
        .fold(boxes[0], |a, r| Rect::from_corners(Point::new(a.min.x.min(r.min.x), a.min.y.min(r.min.y)), Point::new(a.max.x.max(r.max.x), a.max.y.max(r.max.y))));
    let c = union.centre();
    assert!((c.x - w / 2).abs() <= GRID_PX && (c.y - h / 2).abs() <= GRID_PX, "{c:?}");
    assert_eq!(sh.s.history.undo.len(), 1);
    // A selection: only it moves.
    let mut sh = Shell::with(&text);
    sh.click("bank", CTRL);
    let customer = sh.at_lines("context")["customer"];
    sh.cmd(Command::Centre);
    let c = sh.rect("bank").centre();
    assert!((c.x - w / 2).abs() <= GRID_PX && (c.y - h / 2).abs() <= GRID_PX, "{c:?}");
    assert_eq!(sh.at_lines("context")["customer"], customer);
}

// ------------------------------------------------------------ task 07

/// Alt+click toggles a box in the selection as Shift+click does and
/// never enters; an Alt+double-click leaves view and selection as they
/// were.
#[test]
fn p2_718_alt_click_toggles_selection_never_enters() {
    let mut sh = Shell::with(SHOP);
    sh.click("customer", CTRL);
    sh.click("shop", ALT);
    assert_eq!(sh.s.view, Some(declared("context")), "Alt+click never enters");
    assert_eq!(sh.selection(), vec![el("customer"), el("shop")]);
    sh.click("shop", ALT);
    assert_eq!(sh.selection(), vec![el("customer")]);
    let at = sh.screen("shop");
    sh.double_click_at(at, ALT);
    assert_eq!(sh.s.view, Some(declared("context")));
    assert_eq!(sh.selection(), vec![el("customer")]);
}

/// Alt+double-click on a box whose element has an `http(s)` URL opens it.
// TODO(srs): P2-719, the schemes. Reading taken (B editor-breadth 10):
// only `http` and `https`.
#[test]
fn p2_719_alt_double_click_opens_element_url() {
    let mut sh = Shell::with(SHOP);
    let at = sh.screen("customer");
    sh.double_click_at(at, ALT);
    assert_eq!(sh.urls, ["https://example.com/customer"]);
    // A plain double click on a box that is not enterable opens nothing.
    sh.double_click_at(at, NONE);
    assert_eq!(sh.urls.len(), 1);
}

/// With no URL, or a non-HTTP one, nothing opens and the status bar says
/// so.
#[test]
fn p2_719_no_url_sets_notice() {
    let mut sh = Shell::with(SHOP);
    let at = sh.screen("clerk");
    sh.double_click_at(at, ALT);
    assert!(sh.urls.is_empty());
    let notice = sh.s.status().notice.expect("a notice");
    assert!(notice.contains("Clerk") && notice.contains("URL"), "{notice}");
    let at = sh.screen("bank");
    sh.double_click_at(at, ALT);
    assert!(sh.urls.is_empty(), "ftp is not opened");
    assert!(sh.s.status().notice.expect("a notice").contains("ftp://bank.example/"));
}

/// Alt+double-click on a line opens its winning relationship's URL.
#[test]
fn p2_720_alt_double_click_line_opens_winner_url() {
    let mut sh = Shell::with(SHOP);
    let at = sh.line_point("customer", "shop");
    sh.double_click_at(at, ALT);
    assert_eq!(sh.urls, ["https://example.com/visits"]);
    let at = sh.line_point("clerk", "shop");
    sh.double_click_at(at, ALT);
    assert_eq!(sh.urls.len(), 1);
    assert!(sh.s.status().notice.is_some());
}

// ------------------------------------------------------------ task 08

/// `d` hides every description; geometry is unchanged.
#[test]
fn p2_721_d_hides_descriptions_keeps_geometry() {
    use crate::canvas::{Primitive, paint_items};
    let mut sh = Shell::with(SHOP);
    let texts = |sh: &Shell| -> Vec<String> {
        paint_items(&sh.s.draw_list())
            .into_iter()
            .filter_map(|i| match i.primitive {
                Primitive::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    };
    assert!(texts(&sh).iter().any(|t| t.contains("Buys things.")));
    let scene = sh.scene().clone();
    sh.key(Key::D, NONE);
    assert!(sh.s.draw_list().hide_descriptions);
    assert!(!texts(&sh).iter().any(|t| t.contains("Buys things.")));
    assert!(texts(&sh).iter().any(|t| t == "Customer"));
    assert_eq!(sh.scene(), &scene, "geometry unchanged");
    sh.key(Key::D, NONE);
    assert!(texts(&sh).iter().any(|t| t.contains("Buys things.")));
    assert!(sh.writes.is_empty());
}

/// A line label with every line hidden by `d` and `m` paints no
/// background, so no page-coloured gap cuts its line (found in the
/// real-window check).
#[test]
fn p2_721_hidden_label_paints_no_background() {
    use crate::canvas::{Primitive, Source, paint_items};
    let mut sh = Shell::with(SHOP);
    let label_fills = |sh: &Shell| {
        paint_items(&sh.s.draw_list())
            .into_iter()
            .filter(|i| {
                matches!(
                    (&i.source, &i.primitive),
                    (Source::Scene(ItemKey::Label(_)), Primitive::Polygon { fill: Some(_), .. })
                )
            })
            .count()
    };
    let before = label_fills(&sh);
    assert!(before > 0, "the labels have a background");
    sh.key(Key::D, NONE);
    sh.key(Key::M, NONE);
    assert_eq!(label_fills(&sh), 0);
}

/// `m` hides each box's type line and each label's technology; geometry
/// is unchanged.
// TODO(srs): P2-722, what metadata is. Reading taken (B editor-breadth 9):
// the type line and the technology.
#[test]
fn p2_722_m_hides_metadata_keeps_geometry() {
    use crate::canvas::{Primitive, paint_items};
    let mut sh = Shell::with(SHOP);
    let texts = |sh: &Shell| -> Vec<String> {
        paint_items(&sh.s.draw_list())
            .into_iter()
            .filter_map(|i| match i.primitive {
                Primitive::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    };
    assert!(texts(&sh).iter().any(|t| t.contains("[Person]")));
    // The `visits` line's label carries its technology, HTTPS.
    assert!(texts(&sh).iter().any(|t| t.contains("[HTTPS]")), "{:?}", texts(&sh));
    let scene = sh.scene().clone();
    sh.key(Key::M, NONE);
    assert!(!texts(&sh).iter().any(|t| t.contains("[Person]") || t.contains("[Software System]")));
    assert!(!texts(&sh).iter().any(|t| t.contains("HTTPS")), "the line label's technology is hidden");
    assert!(texts(&sh).iter().any(|t| t.contains("Visits")), "the label's description stays");
    assert!(texts(&sh).iter().any(|t| t == "Customer"));
    assert_eq!(sh.scene(), &scene);
    assert!(sh.writes.is_empty());
}

// ------------------------------------------------------------ task 09

/// With tooltips on (`t`), hovering a box shows its details.
// TODO(srs): P2-723, the default and the content. Reading taken (B
// editor-breadth 9, Structurizr 6.2.4 diagrams.jsp: `tooltip.disable()` at
// start): off; name, type, description, technology, tags, URL,
// properties.
#[test]
fn p2_723_tooltip_toggle_shows_details_on_hover() {
    let mut sh = Shell::with(SHOP);
    let at = sh.screen("customer");
    sh.hover(at);
    assert!(sh.s.draw_list().tooltip.is_none(), "off by default");
    sh.key(Key::T, NONE);
    sh.hover(at);
    let tip = sh.s.draw_list().tooltip.expect("a tooltip");
    assert_eq!(tip.details.name, "Customer");
    assert_eq!(tip.details.description.as_deref(), Some("Buys things."));
    let empty = ScreenPoint { x: 1.0, y: 1.0 };
    sh.hover(empty);
    assert!(sh.s.draw_list().tooltip.is_none());
    assert!(sh.writes.is_empty());
}

/// `structurizr.tooltips` on the view, else on the view set, sets the
/// initial state.
#[test]
fn p2_723_structurizr_tooltips_sets_initial() {
    let on = SHOP.replacen(
        "        styles {",
        "        properties {\n            \"structurizr.tooltips\" \"true\"\n        }\n        styles {",
        1,
    );
    let mut sh = Shell::with(&on);
    let at = sh.screen("customer");
    sh.hover(at);
    assert!(sh.s.draw_list().tooltip.is_some(), "the view set turns them on");
    let off_here = on.replacen(
        "            at bank 1000 700\n",
        "            at bank 1000 700\n            properties {\n                \"structurizr.tooltips\" \"false\"\n            }\n",
        1,
    );
    let mut sh = Shell::with(&off_here);
    let at = sh.screen("customer");
    sh.hover(at);
    assert!(sh.s.draw_list().tooltip.is_none(), "the view's property wins");
}

/// Holding Alt while a tooltip shows makes it sticky; Escape releases.
// TODO(srs): P2-724, the release. Reading taken (B editor-breadth 9):
// Escape or a click elsewhere.
#[test]
fn p2_724_alt_makes_tooltip_sticky() {
    let mut sh = Shell::with(SHOP);
    sh.key(Key::T, NONE);
    let at = sh.screen("customer");
    sh.hover(at);
    assert!(sh.s.draw_list().tooltip.is_some());
    // Alt with the tooltip up: sticky; it stays when the pointer leaves.
    sh.key(Key::Alt, ALT);
    let away = ScreenPoint { x: 1.0, y: 1.0 };
    sh.hover(away);
    let tip = sh.s.draw_list().tooltip.expect("stays");
    assert!(tip.sticky);
    assert_eq!(tip.details.name, "Customer");
    // Escape releases it.
    sh.key(Key::Escape, NONE);
    assert!(sh.s.draw_list().tooltip.is_none());
    // Alt with no tooltip up does nothing.
    sh.key(Key::Alt, ALT);
    assert!(sh.s.draw_list().tooltip.is_none());
}

// ------------------------------------------------------------ task 10

/// The tag filter lists the open view's user-defined tags only.
// TODO(srs): P2-725, the built-ins. Reading taken (P2-725 as written, fix
// round): the six built-ins the sentence names (`Element`, `Person`,
// `Software System`, `Container`, `Component`, `Relationship`) are
// hidden; `External` is listed, as Structurizr 6.2.4 lists it (B
// editor-breadth 9 hid it).
#[test]
fn p2_725_tag_filter_lists_user_defined_tags_only() {
    let mut sh = Shell::with(SHOP);
    sh.key(Key::G, NONE);
    assert_eq!(sh.s.overlay, Some(Overlay::TagFilter));
    // Fix round: the sentence's built-ins are the six it names, so the
    // Bank box's `External` is listed (as Structurizr 6.2.4 lists it).
    assert_eq!(sh.s.tag_choices(), ["Buyer", "External", "Web"]);
    sh.cmd(Command::TagFilter(vec!["External".to_owned()]));
    let bank = sh.model().find_written("bank").expect("bank");
    let bank_box = ItemKey::Box(sh.scene().boxes_of(bank)[0]);
    assert!(!sh.s.draw_list().faded.contains(&bank_box), "External picks the external box");
}

/// Choosing tags fades every box and line carrying none of them;
/// clearing restores the view.
// TODO(srs): P2-725, fade or hide, any or all. Reading taken (B
// editor-breadth 9): faded to 20%, any-of.
#[test]
fn p2_725_tag_filter_marks_untagged_items() {
    let mut sh = Shell::with(SHOP);
    sh.cmd(Command::TagFilter(vec!["Buyer".to_owned()]));
    let faded = sh.s.draw_list().faded;
    let customer = sh.model().find_written("customer").expect("customer");
    let box_of = |sh: &Shell, id| ItemKey::Box(sh.scene().boxes_of(id)[0]);
    assert!(!faded.contains(&box_of(&sh, customer)));
    let clerk = sh.model().find_written("clerk").expect("clerk");
    assert!(faded.contains(&box_of(&sh, clerk)));
    assert!(faded.iter().any(|k| matches!(k, ItemKey::Line(_))));
    sh.cmd(Command::TagFilter(Vec::new()));
    assert!(sh.s.draw_list().faded.is_empty());
    assert!(sh.writes.is_empty());
}

/// `p` lists the perspectives of the open view's items; choosing one
/// fades every item without it.
// TODO(srs): P2-726, what is drawn. Reading taken (B editor-breadth 9):
// items without the perspective at 20%.
#[test]
fn p2_726_perspective_list_and_marks() {
    let mut sh = Shell::with(SHOP);
    sh.key(Key::P, NONE);
    assert_eq!(sh.s.overlay, Some(Overlay::Perspectives));
    assert_eq!(sh.s.perspective_choices(), ["Security"]);
    sh.cmd(Command::Perspective(Some("Security".to_owned())));
    let faded = sh.s.draw_list().faded;
    let clerk = sh.model().find_written("clerk").expect("clerk");
    assert!(faded.contains(&ItemKey::Box(sh.scene().boxes_of(clerk)[0])));
    let customer = sh.model().find_written("customer").expect("customer");
    assert!(!faded.contains(&ItemKey::Box(sh.scene().boxes_of(customer)[0])));
    sh.cmd(Command::Perspective(None));
    assert!(sh.s.draw_list().faded.is_empty());
}

// ------------------------------------------------------------ task 11

/// `n` shows or hides the view list panel without changing view, zoom,
/// pan or file.
#[test]
fn p2_727_n_toggles_view_list_panel() {
    let mut sh = Shell::with(SHOP);
    let (view, camera) = (sh.s.view.clone(), sh.s.camera);
    assert!(sh.s.aids.view_list);
    sh.key(Key::N, NONE);
    assert!(!sh.s.aids.view_list);
    sh.key(Key::N, NONE);
    assert!(sh.s.aids.view_list);
    assert_eq!((sh.s.view.clone(), sh.s.camera), (view, camera));
    assert!(sh.writes.is_empty());
}

/// `i` toggles the drawn key.
// TODO(srs): P2-728. Reading taken (B editor-breadth 7): `i` toggles the
// key's visibility on the canvas.
#[test]
fn p2_728_i_shows_key() {
    use crate::canvas::{Source, paint_items};
    let mut sh = Shell::with(SHOP);
    let has_key = |sh: &Shell| paint_items(&sh.s.draw_list()).iter().any(|i| i.source == Source::Scene(ItemKey::Legend));
    assert!(has_key(&sh));
    sh.key(Key::I, NONE);
    assert!(sh.s.draw_list().hide_key);
    assert!(!has_key(&sh));
    sh.key(Key::I, NONE);
    assert!(has_key(&sh));
    assert!(sh.writes.is_empty());
}

/// `l` (no selection of two or more) opens the navigator: views above,
/// next to and below the open one by C4 level.
// TODO(srs): P2-729, the form. Reading taken (B editor-breadth 7 asked
// for "a searchable tree of views grouped by kind"; the fix-round ruling
// keeps the groups Above / Next to / Below, classified by C4 level
// relative to the open view, with a search field).
#[test]
fn p2_729_l_opens_navigator() {
    let mut sh = Shell::with(SHOP);
    sh.show("containers");
    sh.key(Key::L, NONE);
    assert_eq!(sh.s.overlay, Some(Overlay::Navigator));
    let nav = sh.s.navigator();
    let keys = |level: NavLevel| -> Vec<String> {
        nav.iter().filter(|(l, _)| *l == level).map(|(_, e)| e.view.key()).collect()
    };
    // The landscape and the shop's context view contain the shop.
    assert_eq!(keys(NavLevel::Above), ["context", "estate"]);
    assert!(keys(NavLevel::NextTo).is_empty(), "no other container view");
    assert_eq!(keys(NavLevel::Below), ["web-component"]);
}

/// Fix round (P2-729): the levels are C4 levels. On a context view the
/// container view of the system shown is below, never next to; the
/// landscape is above.
#[test]
fn p2_729_levels_follow_c4() {
    let nav_keys = |sh: &Shell, level: NavLevel| -> Vec<String> {
        sh.s.navigator().iter().filter(|(l, _)| *l == level).map(|(_, e)| e.view.key()).collect()
    };
    let mut sh = Shell::with(SHOP);
    sh.show("context");
    assert_eq!(nav_keys(&sh, NavLevel::Above), ["estate"]);
    assert!(nav_keys(&sh, NavLevel::NextTo).is_empty());
    assert_eq!(nav_keys(&sh, NavLevel::Below), ["containers"]);
    sh.show("estate");
    assert!(nav_keys(&sh, NavLevel::Above).is_empty());
    assert_eq!(nav_keys(&sh, NavLevel::Below), ["context"], "the landscape is above the context view");

    let mut ex = Shell::with(include_str!("../../../tests/data/example.dsl"));
    ex.show("C1");
    assert!(nav_keys(&ex, NavLevel::Above).is_empty());
    assert!(nav_keys(&ex, NavLevel::NextTo).is_empty());
    assert_eq!(nav_keys(&ex, NavLevel::Below), ["C2"], "C2 sits below C1");
    ex.show("C2");
    assert_eq!(nav_keys(&ex, NavLevel::Above), ["C1"]);
    assert_eq!(nav_keys(&ex, NavLevel::Below), ["C3-serving", "C3-startup"]);
    ex.show("C3-serving");
    assert_eq!(nav_keys(&ex, NavLevel::Above), ["C1", "C2"]);
    assert_eq!(nav_keys(&ex, NavLevel::NextTo), ["C3-startup"]);
    assert!(nav_keys(&ex, NavLevel::Below).is_empty());
}

/// Fix round (P2-729, B editor-breadth 7 "searchable"): the navigator's
/// search field keeps, in all three groups, the views whose key or title
/// contains the typed text, ignoring case; reopening clears it.
#[test]
fn p2_729_search_filters_all_groups() {
    let mut sh = Shell::with(SHOP);
    sh.show("containers");
    sh.key(Key::L, NONE);
    let keys = |sh: &Shell| -> Vec<String> { sh.s.navigator().iter().map(|(_, e)| e.view.key()).collect() };
    assert_eq!(keys(&sh).len(), 3);
    sh.event(Event::Text("ESTATE".to_owned()));
    assert_eq!(keys(&sh), ["estate"], "key match, any case");
    sh.event(Event::Text("users".to_owned()));
    assert_eq!(keys(&sh), ["context"], "title match: The shop and its users");
    sh.event(Event::Text("web".to_owned()));
    assert_eq!(keys(&sh), ["web-component"]);
    sh.event(Event::Text("nothing like it".to_owned()));
    assert!(keys(&sh).is_empty());
    sh.key(Key::Escape, NONE);
    sh.key(Key::L, NONE);
    assert_eq!(keys(&sh).len(), 3, "a new navigator starts unfiltered");
    assert!(sh.writes.is_empty());
}

/// Owner must-have (EDIT-15 as amended): a single click on an enterable
/// box selects it like any other box; it never opens a view.
#[test]
fn edit_15_single_click_selects_enterable_box() {
    let mut sh = Shell::with(SHOP);
    sh.click("shop", NONE);
    assert_eq!(sh.s.view, Some(declared("context")), "a single click stays");
    assert_eq!(sh.selection(), [el("shop")]);
}

/// Owner ruling (editor-ux-02, EDIT-15): entering moves to the context
/// menu. A right click on an enterable box lists "Enter <view>"; picking
/// it opens exactly one level down: C1 to C2, C2 to the container's C3,
/// never C1 to C3.
#[test]
fn edit_15_context_menu_enters_one_level() {
    let mut ex = Shell::with(include_str!("../../../tests/data/example.dsl"));
    ex.show("C1");
    let at = ex.screen("system");
    ex.click_button(at, Button::Right, NONE);
    assert_eq!(ex.s.view, Some(declared("C1")), "the menu alone enters nothing");
    let entries = ex.menu();
    assert!(entries[0].label.starts_with("Enter "), "{entries:?}");
    ex.cmd(Command::ContextAction(entries[0].action.clone()));
    assert_eq!(ex.s.view, Some(declared("C2")), "one level down, not two");
    assert!(ex.s.overlay.is_none(), "picking closes the menu");
    let at = ex.screen("app");
    ex.click_button(at, Button::Right, NONE);
    let entries = ex.menu();
    ex.cmd(Command::ContextAction(entries[0].action.clone()));
    assert_eq!(ex.s.view, Some(declared("C3-serving")), "the first declared component view");
    assert!(ex.writes.is_empty());
}

/// A double click no longer enters a level: on an enterable box it
/// edits the name in place, as Enter does (EDIT-5), and the view stays.
#[test]
fn edit_15_double_click_edits_in_place_and_never_enters() {
    let mut sh = Shell::with(SHOP);
    let at = sh.screen("shop");
    sh.double_click_at(at, NONE);
    assert_eq!(sh.s.view, Some(declared("context")), "no level entered");
    let edit = sh.s.text_edit.as_ref().expect("the in-place editor opened");
    assert_eq!(edit.item, el("shop"));
    assert_eq!(edit.fields[0].buffer, "Shop");
}

/// Editor-ux-02: a double click edits whatever has editable text: a
/// line label (the relationship's description) and the view title.
#[test]
fn edit_5_double_click_edits_label_and_title() {
    use c4rizer_core::scene::ItemKey;
    let mut sh = Shell::with(SHOP);
    let model = sh.model();
    let (from, to) = (model.find_written("customer").expect("customer"), model.find_written("shop").expect("shop"));
    let rel = model.find_relationship(from, to)[0];
    let edge = sh.scene().lines.iter().find(|l| l.relationships.contains(&rel)).expect("line").edge;
    let at = sh.s.camera.to_screen(sh.scene().item_bounds(ItemKey::Label(edge)).expect("label").centre());
    sh.double_click_at(at, NONE);
    let edit = sh.s.text_edit.as_ref().expect("label edit");
    assert!(matches!(edit.item, Selected::Relationship(_)), "{:?}", edit.item);
    assert_eq!(edit.fields[0].buffer, "Visits");
    sh.key(Key::Escape, NONE);
    assert!(sh.s.text_edit.is_none());
    let at = sh.s.camera.to_screen(sh.scene().item_bounds(ItemKey::Title).expect("title").centre());
    sh.double_click_at(at, NONE);
    let edit = sh.s.text_edit.as_ref().expect("title edit");
    assert_eq!(edit.item, Selected::Title);
    assert_eq!(sh.s.view, Some(declared("context")));
}

/// Several views scoped to one element: the menu has one "Enter" entry
/// per view, the first declared first; a box with a `url` also offers
/// the URL, which opens through the same http/https check as
/// Alt+double-click did (P2-719).
#[test]
fn p2_305_context_menu_lists_every_reachable_view_and_url() {
    let text = SHOP.replacen(
        "        systemLandscape \"estate\"",
        "        container shop \"more\" \"More of the shop\" {\n            include *\n        }\n        systemLandscape \"estate\"",
        1,
    );
    let mut sh = Shell::with(&text);
    let at = sh.screen("shop");
    sh.click_button(at, Button::Right, NONE);
    let labels: Vec<String> = sh.menu().into_iter().map(|e| e.label).collect();
    assert_eq!(labels, ["Enter Inside the shop", "Enter More of the shop"]);
    // owner decision 2026-11-19: behaviour change 5
    // A box with neither views nor URL opens a menu with no entry (all
    // rows greyed).
    let at = sh.screen("clerk");
    sh.click_button(at, Button::Right, NONE);
    assert_eq!(sh.s.overlay, Some(Overlay::ContextMenu { entries: Vec::new() }));
    sh.key(Key::Escape, NONE);
    // URL entries on a box and on a line.
    let at = sh.screen("customer");
    sh.click_button(at, Button::Right, NONE);
    let entries = sh.menu();
    assert_eq!(entries.len(), 1);
    sh.cmd(Command::ContextAction(entries[0].action.clone()));
    assert_eq!(sh.urls, ["https://example.com/customer"]);
    let at = sh.line_point("customer", "shop");
    sh.click_button(at, Button::Right, NONE);
    let entries = sh.menu();
    sh.cmd(Command::ContextAction(entries[0].action.clone()));
    assert_eq!(sh.urls.len(), 2);
    assert_eq!(sh.urls[1], "https://example.com/visits");
    let at = sh.screen("bank");
    sh.click_button(at, Button::Right, NONE);
    let entries = sh.menu();
    sh.cmd(Command::ContextAction(entries[0].action.clone()));
    assert_eq!(sh.urls.len(), 2, "ftp is not opened");
    assert!(sh.s.status().notice.expect("a notice").contains("ftp://bank.example/"));
}

/// editor-ux-03 (OR-4): holding the right button and dragging pans the
/// canvas as the middle button does, by the same amount, and a right
/// drag never opens the context menu, even when it ends on a box.
#[test]
fn edit_28_right_drag_pans_like_middle_and_opens_no_menu() {
    let drag = |button: Button, from: ScreenPoint, to: ScreenPoint| {
        let mut sh = Shell::with(SHOP);
        let pan = sh.s.camera.pan;
        sh.input(Input::PointerDown { at: from, button, mods: NONE });
        sh.input(Input::PointerMove { at: ScreenPoint { x: from.x + 3.0, y: from.y } });
        sh.input(Input::PointerMove { at: to });
        sh.input(Input::PointerUp { at: to, button });
        assert_ne!(sh.s.camera.pan, pan, "{button:?} drag pans");
        assert!(sh.s.overlay.is_none(), "{button:?} drag opens no menu");
        assert!(sh.selection().is_empty(), "{button:?} drag selects nothing");
        (sh.s.camera.pan.x - pan.x, sh.s.camera.pan.y - pan.y)
    };
    let probe = Shell::with(SHOP);
    let on_box = probe.screen("shop");
    let to = ScreenPoint { x: on_box.x + 60.0, y: on_box.y + 25.0 };
    let by_middle = drag(Button::Middle, on_box, to);
    let by_right = drag(Button::Right, on_box, to);
    assert_eq!(by_right, by_middle, "the same pan as the middle button");
    // Back on a box the release is far from the press: still no menu.
    let mut sh = Shell::with(SHOP);
    let from = ScreenPoint { x: on_box.x + 200.0, y: on_box.y };
    sh.input(Input::PointerDown { at: from, button: Button::Right, mods: NONE });
    sh.input(Input::PointerMove { at: on_box });
    sh.input(Input::PointerUp { at: on_box, button: Button::Right });
    assert!(sh.s.overlay.is_none());
}

/// A right press and release under the click distance opens the menu
/// (EDIT-28); a travel of 3 screen px is still a click.
#[test]
fn edit_28_right_click_under_slop_opens_menu() {
    let mut sh = Shell::with(SHOP);
    let at = sh.screen("shop");
    let pan = sh.s.camera.pan;
    sh.input(Input::PointerDown { at, button: Button::Right, mods: NONE });
    let near = ScreenPoint { x: at.x + 3.0, y: at.y };
    sh.input(Input::PointerMove { at: near });
    sh.input(Input::PointerUp { at: near, button: Button::Right });
    assert_eq!(sh.menu().len(), 1);
    assert_eq!(sh.s.camera.pan, pan, "no pan");
    assert_eq!(sh.selection(), vec![el("shop")]);
}

/// A left click elsewhere or Escape closes the menu.
#[test]
fn p2_305_context_menu_closes_on_click_and_escape() {
    let mut sh = Shell::with(SHOP);
    let at = sh.screen("shop");
    sh.click_button(at, Button::Right, NONE);
    assert!(sh.s.overlay.is_some());
    sh.key(Key::Escape, NONE);
    assert!(sh.s.overlay.is_none());
    sh.click_button(at, Button::Right, NONE);
    sh.click_button(ScreenPoint { x: 5.0, y: 5.0 }, Button::Left, NONE);
    assert!(sh.s.overlay.is_none());
}

/// Behaviour change 5: a right click on a box with no deeper view and no
/// URL still opens the menu, with no entry, so every row is disabled
/// (`app::frame::menu_rows`). It is harmless: Enter runs nothing, a
/// release over a greyed row (which the canvas sees without its press)
/// changes nothing, and Escape only closes it.
// owner decision 2026-11-19: behaviour change 5
#[test]
fn p2_305_menu_without_entries_opens_disabled_and_harmless() {
    let mut sh = Shell::with(SHOP);
    let at = sh.screen("clerk");
    sh.click_button(at, Button::Right, NONE);
    let empty = Some(Overlay::ContextMenu { entries: Vec::new() });
    assert_eq!(sh.s.overlay, empty, "the menu opens with every entry disabled");
    assert_eq!(sh.selection(), [el("clerk")]);
    sh.key(Key::Enter, NONE);
    assert_eq!(sh.s.overlay, empty, "Enter leaves the menu");
    assert!(sh.s.text_edit.is_none(), "Enter opens no editor under the menu");
    sh.input(Input::PointerUp {
        at: ScreenPoint { x: at.x + 20.0, y: at.y + 20.0 },
        button: Button::Left,
    });
    assert_eq!(sh.s.overlay, empty, "a click on a greyed row runs nothing");
    sh.key(Key::Escape, NONE);
    assert!(sh.s.overlay.is_none(), "Escape closes it");
    assert_eq!(sh.selection(), [el("clerk")], "and keeps the selection");
    assert_eq!(sh.s.view, Some(declared("context")));
    assert!(sh.writes.is_empty() && sh.urls.is_empty());
}

// ------------------------------------------------------------ task 12

/// Up and Down over a line move its label 5 position units toward the
/// source and destination, written as the view's `line` line.
// TODO(srs): P2-708, direction and step. Reading taken (B editor-breadth
// 6): Up 5 toward the source, Down 5 toward the destination, 0..100.
#[test]
fn p2_708_up_down_moves_hovered_label_position() {
    let mut sh = Shell::with(SHOP);
    let at = sh.line_point("clerk", "shop");
    sh.hover(at);
    let position = |sh: &Shell| sh.on_disk().model.view("context").expect("view").lines.first().and_then(|l| l.position).map(|p| p.value);
    sh.key(Key::ArrowDown, NONE);
    assert_eq!(position(&sh), Some(55));
    sh.key(Key::ArrowUp, NONE);
    sh.key(Key::ArrowUp, NONE);
    assert_eq!(position(&sh), Some(45));
    assert_eq!(sh.s.history.undo.len(), 3);
    assert_eq!(sh.s.view, Some(declared("context")));
}

/// `r` over a line cycles Direct, Orthogonal, Curved.
#[test]
fn p2_709_r_cycles_hovered_line_routing() {
    let mut sh = Shell::with(SHOP);
    let at = sh.line_point("clerk", "shop");
    sh.hover(at);
    let routing = |sh: &Shell| sh.on_disk().model.view("context").expect("view").lines.first().and_then(|l| l.routing).map(|r| r.value);
    let mut seen = Vec::new();
    for _ in 0..3 {
        sh.key(Key::R, NONE);
        seen.push(routing(&sh).expect("written"));
    }
    assert!(seen[0] != seen[1] && seen[1] != seen[2] && seen[0] != seen[2], "{seen:?}");
    let next = |r: Routing| match r {
        Routing::Direct => Routing::Orthogonal,
        Routing::Orthogonal => Routing::Curved,
        _ => Routing::Direct,
    };
    assert_eq!(next(seen[0]), seen[1]);
    assert_eq!(next(seen[1]), seen[2]);
}

/// `j` over a line toggles its jumps.
#[test]
fn p2_710_j_toggles_hovered_line_jumps() {
    let mut sh = Shell::with(SHOP);
    let at = sh.line_point("clerk", "shop");
    sh.hover(at);
    let jump = |sh: &Shell| sh.on_disk().model.view("context").expect("view").lines.first().and_then(|l| l.jump).map(|j| j.value);
    sh.key(Key::J, NONE);
    assert_eq!(jump(&sh), Some(true));
    sh.key(Key::J, NONE);
    assert_eq!(jump(&sh), Some(false));
    sh.key(Key::J, NONE);
    assert_eq!(jump(&sh), Some(true));
}

// ------------------------------------------------------------ task 13

/// No viewing aid writes the file or adds an undo step.
#[test]
fn p2_730_viewing_aids_never_write() {
    let mut sh = Shell::with(SHOP);
    let commands = [
        Command::Fit(Fit::Width),
        Command::Fit(Fit::Height),
        Command::Fit(Fit::Content),
        Command::Fullscreen,
        Command::NextView,
        Command::PreviousView,
        Command::Back,
        Command::QuickNavigation,
        Command::ToggleDescriptions,
        Command::ToggleMetadata,
        Command::ToggleTooltips,
        Command::TagFilterPanel,
        Command::TagFilter(vec!["Buyer".to_owned()]),
        Command::PerspectivePanel,
        Command::Perspective(Some("Security".to_owned())),
        Command::ToggleViewList,
        Command::ToggleKey,
        Command::Navigator,
    ];
    for c in commands {
        sh.cmd(c);
    }
    for k in [Key::Plus, Key::Minus, Key::W, Key::D, Key::M, Key::T, Key::G, Key::P, Key::N, Key::I, Key::Space] {
        sh.key(k, NONE);
        sh.key(Key::Escape, NONE);
    }
    assert!(sh.writes.is_empty());
    assert!(!sh.s.history.can_undo());
    assert_eq!(sh.text(), SHOP);
}

/// After an external reload the toggles, filter and perspective stay; a
/// tag or perspective gone from the open view is dropped.
// TODO(srs): P2-731, persistence. Reading taken (B editor-breadth 10):
// session only, export ignores them.
#[test]
fn p2_731_viewing_aids_survive_reload_stale_dropped() {
    let mut sh = Shell::with(SHOP);
    sh.cmd(Command::ToggleDescriptions);
    sh.cmd(Command::ToggleMetadata);
    sh.cmd(Command::TagFilter(vec!["Buyer".to_owned(), "Web".to_owned()]));
    sh.cmd(Command::Perspective(Some("Security".to_owned())));
    sh.external(&SHOP.replacen(" \"Buyer\" {", " {", 1));
    assert!(!sh.s.aids.descriptions && !sh.s.aids.metadata);
    assert_eq!(sh.s.aids.tags, ["Web"], "the gone tag is dropped");
    assert_eq!(sh.s.aids.perspective.as_deref(), Some("Security"));
    let no_perspectives = SHOP
        .replacen("            perspectives {\n                \"Security\" \"Signs in with a password.\"\n            }\n", "", 1)
        .replacen("            perspectives {\n                \"Security\" \"TLS 1.3.\"\n            }\n", "", 1);
    sh.external(&no_perspectives);
    assert_eq!(sh.s.aids.perspective, None);
}

/// The canvas differs from the SVG export only by marks and overlays:
/// with every aid on, the painted scene items are those of the scene,
/// and the extras are marks, the tooltip and the hidden texts.
#[test]
fn p2_732_draw_list_extras_are_only_overlays_and_marks() {
    use crate::canvas::{Source, paint_items};
    let mut sh = Shell::with(SHOP);
    sh.cmd(Command::TagFilter(vec!["Buyer".to_owned()]));
    sh.key(Key::T, NONE);
    let at = sh.screen("customer");
    sh.hover(at);
    let list = sh.s.draw_list();
    assert!(list.tooltip.is_some() && !list.faded.is_empty());
    for item in paint_items(&list) {
        match item.source {
            Source::Scene(k) => assert!(sh.scene().item_bounds(k).is_some(), "{k:?} is a scene item"),
            Source::Mark(..) | Source::RubberBand | Source::ResizeOutline | Source::Guide => {}
        }
    }
    // The scene (what the export draws) is untouched by the aids.
    assert_eq!(sh.scene().boxes.len(), 4, "the filter hides nothing from the scene");
    assert!(c4rizer_core::export::svg(sh.scene()).contains("Buys things."));
}

/// Every write of this row lands in the open `.dsl` as lines of the view
/// it was made in; none creates or deletes an element or relationship.
#[test]
fn p2_733_writes_only_view_lines_in_dsl() {
    let mut sh = Shell::with(SHOP);
    let base = parse(SHOP).expect("parses").model;
    sh.cmd(Command::AutoPlace);
    sh.cmd(Command::Canvas {
        step: CanvasStep::Auto,
        keep_positions: false,
    });
    sh.cmd(Command::BringOntoCanvas);
    sh.cmd(Command::Centre);
    let at = sh.line_point("clerk", "shop");
    sh.hover(at);
    sh.key(Key::R, NONE);
    sh.key(Key::J, NONE);
    sh.key(Key::ArrowUp, NONE);
    assert!(sh.writes.len() >= 5);
    for w in &sh.writes {
        assert_eq!(w.path, sh.path, "only the open file");
        let m = parse(&w.text).expect("parses").model;
        assert_eq!(m.elements.len(), base.elements.len());
        assert_eq!(m.relationships.len(), base.relationships.len());
        assert_eq!(m.views.len(), base.views.len());
    }
    let files: Vec<_> = std::fs::read_dir(sh.dir.path()).expect("dir").collect();
    assert_eq!(files.len(), 1, "no side file");
    let text = sh.text();
    for line in text.lines().filter(|l| !SHOP.contains(l)) {
        let t = line.trim_start();
        assert!(t.starts_with("at ") || t.starts_with("paperSize ") || t.starts_with("line "), "{line}");
    }
}

// ------------------------------------------------------------ task 14

/// The editor's SVG export offers `name`, `embed` and `outline`, `name`
/// the default.
#[test]
fn p2_663_export_offers_name_embed_outline() {
    let choices = crate::app::svg_text_choices();
    assert_eq!(choices.iter().map(|(label, _)| *label).collect::<Vec<_>>(), ["name", "embed", "outline"]);
    assert_eq!(choices[0].1, ExportFormat::Svg);
    assert_eq!(choices[1].1, ExportFormat::SvgEmbedded);
    assert_eq!(choices[2].1, ExportFormat::SvgOutlined);
    assert_eq!(ExportFormat::SvgEmbedded.extension(), "svg");
    assert_eq!(ExportFormat::SvgOutlined.extension(), "svg");
}

/// An SVG exported with a choice is byte-identical to `c4rizer render
/// --svg-text <choice>` (both call the same `export` function on the
/// same build).
#[test]
fn p2_664_editor_svg_matches_cli_render() {
    for format in [ExportFormat::Svg, ExportFormat::SvgEmbedded, ExportFormat::SvgOutlined] {
        let mut sh = Shell::with(SHOP);
        let out = sh.dir.join("context.svg");
        sh.cmd(Command::Export {
            targets: vec![(declared("context"), out.clone())],
            format,
        });
        let written = std::fs::read_to_string(&out).expect("exported");
        let parsed = parse(SHOP).expect("parses");
        let build = c4rizer_core::pipeline::build_view(
            &parsed.model,
            "context",
            &c4rizer_core::place::Keep::default(),
            &AtomicBool::new(false),
        )
        .expect("builds");
        let expected = match format {
            ExportFormat::Svg => c4rizer_core::export::svg(&build.scene),
            ExportFormat::SvgEmbedded => c4rizer_core::export::svg_embedded(&build.scene),
            _ => c4rizer_core::export::svg_outlined(&build.scene).expect("outlines"),
        };
        assert_eq!(written, expected, "{format:?}");
    }
}

/// Picking an SVG text choice writes nothing to the diagram file.
#[test]
fn p2_665_svg_text_choice_writes_nothing() {
    let mut sh = Shell::with(SHOP);
    let out = sh.dir.join("out.svg");
    sh.cmd(Command::Export {
        targets: vec![(declared("context"), out)],
        format: ExportFormat::SvgOutlined,
    });
    sh.cmd(Command::ExportView(ExportFormat::SvgEmbedded));
    assert!(sh.writes.is_empty());
    assert_eq!(sh.text(), SHOP);
}

// ------------------------------------------------------------ task 15

/// The breadcrumb names each crumb's C4 level with its element.
#[test]
fn p2_734_breadcrumb_names_c4_level() {
    let mut sh = Shell::with(SHOP);
    sh.cmd(Command::ShowView(ViewRef::Default {
        scope: ElementKey::from_written("web"),
        kind: ViewKind::Component,
    }));
    // Amended at step 5: a crumb names the level of the view it opens
    // (the SRS example `Context: <workspace> › Container: <system> ›
    // Component: <container>`), and the workspace crumb opens the
    // landscape view here.
    let labels: Vec<String> = sh
        .s
        .breadcrumb()
        .iter()
        .enumerate()
        .map(|(i, c)| crate::app::crumb_label(&sh.s, i, c))
        .collect();
    assert_eq!(labels, ["Landscape: Shop", "Container: Shop", "Component: Web app"]);
}

/// The current level is always visible: the last crumb names the level
/// of the view on screen, so a context view and a container view of the
/// same system read differently (P2-734, fix round).
#[test]
fn p2_734_last_crumb_names_level_on_screen() {
    let labels = |sh: &Shell| -> Vec<String> {
        sh.s.breadcrumb()
            .iter()
            .enumerate()
            .map(|(i, c)| crate::app::crumb_label(&sh.s, i, c))
            .collect()
    };
    let mut sh = Shell::with(SHOP);
    sh.cmd(Command::ShowView(declared("context")));
    assert_eq!(labels(&sh), ["Landscape: Shop", "Context: Shop"]);
    sh.cmd(Command::ShowView(declared("containers")));
    assert_eq!(labels(&sh), ["Landscape: Shop", "Container: Shop"]);
    sh.cmd(Command::ShowView(declared("estate")));
    assert_eq!(labels(&sh), ["Landscape: Shop"]);
    // Without a landscape view the workspace crumb opens the context view
    // itself; on that view it must not repeat the level on screen.
    let no_estate = SHOP.replace("systemLandscape \"estate\" \"The estate\" {\n            include *\n        }\n", "");
    assert_ne!(no_estate, SHOP);
    let mut sh = Shell::with(&no_estate);
    sh.cmd(Command::ShowView(declared("context")));
    assert_eq!(labels(&sh), ["Workspace: Shop", "Context: Shop"]);
    sh.cmd(Command::ShowView(declared("containers")));
    assert_eq!(labels(&sh), ["Context: Shop", "Container: Shop"]);
}

/// Resize handles never cover a line end or arrowhead: such a handle is
/// drawn outside the box edge.
#[test]
fn p2_735_handles_never_cover_line_ends() {
    use crate::canvas::{Primitive, Source, paint_items};
    let mut sh = Shell::with(SHOP);
    sh.click("shop", CTRL);
    let list = sh.s.draw_list();
    let items = paint_items(&list);
    let ends: Vec<ScreenPoint> = sh
        .scene()
        .lines
        .iter()
        .flat_map(|l| [l.points[0], l.points[l.points.len() - 1]])
        .map(|p| list.camera.to_screen(p))
        .collect();
    for item in &items {
        if let (Source::Mark(..), Primitive::Polygon { points, fill: Some(_), .. }) = (&item.source, &item.primitive) {
            let (lo_x, hi_x) = points.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
            let (lo_y, hi_y) = points.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
            for e in &ends {
                assert!(!(e.x >= lo_x && e.x <= hi_x && e.y >= lo_y && e.y <= hi_y), "a handle covers {e:?}");
            }
        }
    }
}

/// The style panel's shape choice is one drop-down.
#[test]
fn p2_736_shape_choice_is_one_dropdown() {
    assert!(crate::app::shape_choice_is_dropdown());
}

/// The view list shows each view on one line, the title truncated with
/// an ellipsis, then the key; the full title is the tooltip.
#[test]
fn p2_737_view_list_one_line_with_tooltip() {
    let sh = Shell::with(SHOP);
    let entry = &sh.s.views()[0];
    let (line, tooltip) = crate::app::view_list_line(entry, 12);
    assert!(!line.contains('\n'));
    assert!(line.contains('…') && line.ends_with("context"), "{line}");
    assert_eq!(tooltip, "The shop and its users");
}

/// Drawn in the real left panel, every view list entry is one widget row
/// high, the panel keeps its width frame after frame, and a title that
/// fits keeps more than its ellipsis (P2-737, fix round: the rendered
/// widget wrapped where `view_list_line` had no newline).
#[test]
fn p2_737_rendered_view_list_one_row_per_view() {
    for (name, text) in [
        ("landscape_filtered", include_str!("../../../tests/data/landscape_filtered.dsl")),
        ("example", include_str!("../../../tests/data/example.dsl")),
    ] {
        let sh = Shell::with(text);
        for width in [100.0_f32, 120.0, 220.0] {
            let drawn = crate::app::draw_view_list(&sh.s, width, 6);
            assert_eq!(drawn.rows.len(), sh.s.views().len(), "{name} {width}");
            for (line, height) in &drawn.rows {
                assert!(*height <= drawn.row_height + 0.5, "{name} {width}: {line:?} is {height}px high");
            }
            assert!(drawn.panel_width >= width - 1.0, "{name} {width}: panel shrank to {}", drawn.panel_width);
            if width >= 220.0 {
                for (line, _) in &drawn.rows {
                    assert!(!line.starts_with('…'), "{name} {width}: {line:?} lost its whole title");
                }
            }
        }
    }
}

/// A fitted view keeps text at least 9px high on screen: the fit stops at
/// that zoom.
// TODO(srs): P2-738, 9px. Reading taken: 9px, as written.
#[test]
fn p2_738_fit_keeps_text_at_least_9px() {
    let huge = Rect::from_corners(Point::new(0, 0), Point::new(40_000, 40_000));
    let cam = crate::camera::fit(huge, CANVAS, Fit::Content).expect("fits");
    let smallest_text_px = 11.0;
    assert!(cam.zoom * smallest_text_px >= 9.0 - 1e-3, "{cam:?}");
}

/// The details panel shows an edit affordance on name, description and
/// technology; a double click or Enter edits them in place.
#[test]
fn p2_739_details_fields_show_edit_affordance() {
    let mut sh = Shell::with(SHOP);
    sh.click("customer", CTRL);
    let fields = crate::app::editable_detail_fields(&sh.s);
    assert_eq!(fields, ["name", "description"]);
    sh.key(Key::Enter, NONE);
    assert!(sh.s.text_edit.is_some());
}
