//! Tests of the anchors row, editor part (SRS 4.8 P4-213..P4-218),
//! driven headlessly like `vertices_tests`: effects run synchronously
//! with `files` and `worker::serve`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::Point;
use c4rizer_core::scene::{Scene, SceneLine};

use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState};
use crate::state::{Button, Command, Effect, Event, Input, MenuAction, Modifiers, Overlay, State};
use crate::testutil::{Ids, TempDir};
use crate::worker;

/// A shop, roles only, its container view placed by hand; `{lines}` is
/// replaced by the view's extra lines. Web is 240 x 140 px at (200,
/// 400), db at (200, 800): x 80..320, y 730..870.
const SHOP: &str = r#"workspace "Shop" {
    model {
        customer = person "Customer" "Buys goods."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust"
            api = container "Api" "Answers calls." "Rust"
            db = container "Db" "Stores orders." "SQL"
        }
        customer -> web "Browses" "HTTPS"
        web -> db "Reads orders" "SQL"
        api -> db "Writes orders" "SQL"
    }
    views {
        container shop "c2" "Shop" {
            include *
            at customer 200 100
            at web 200 400
            at api 700 400
            at db 200 800
{lines}
        }
    }
}
"#;

/// `SHOP` with `lines` (each a whole view line, written with flat ids),
/// under `ids`: under hierarchical ids every reference to a container,
/// `anchor` lines included, is its dotted full id.
fn shop(ids: Ids, lines: &[&str]) -> String {
    let text: String = lines.iter().fold(String::new(), |mut out, v| {
        out.push_str("            ");
        out.push_str(v);
        out.push('\n');
        out
    });
    ids.workspace(&SHOP.replace("{lines}\n", &text))
}

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    _dir: TempDir,
}

impl Shell {
    fn open(text: &str) -> Shell {
        Shell::open_as(text, "shop.dsl")
    }

    fn open_as(text: &str, name: &str) -> Shell {
        let dir = TempDir::new("anchors");
        let path = dir.join(name);
        std::fs::write(&path, text).expect("fixture written");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
            _dir: dir,
        };
        shell.event(Event::Command(Command::Open(path)));
        assert_eq!(shell.scene().key, "c2");
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

    fn scene(&self) -> &Scene {
        self.s.scene().expect("a scene")
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.path).expect("read back")
    }

    /// The scene line between the boxes named `from` and `to`.
    fn line(&self, from: &str, to: &str) -> SceneLine {
        let scene = self.scene();
        let model = self.s.model().expect("parsed");
        let name = |b: c4rizer_core::view::BoxId| model.element(scene.boxes[b.0 as usize].element).name.value.clone();
        scene
            .lines
            .iter()
            .find(|l| name(l.source) == from && name(l.destination) == to)
            .cloned()
            .expect("line drawn")
    }

    /// The screen point of scene px `(x, y)`.
    fn at(&self, x: i32, y: i32) -> ScreenPoint {
        self.s.camera.to_screen(Point::new(x, y))
    }

    fn down(&mut self, at: ScreenPoint) {
        self.event(Event::Input(Input::PointerDown {
            at,
            button: Button::Left,
            mods: Modifiers::default(),
        }));
    }

    fn move_to(&mut self, at: ScreenPoint) {
        self.event(Event::Input(Input::PointerMove { at }));
    }

    fn up(&mut self, at: ScreenPoint) {
        self.event(Event::Input(Input::PointerUp { at, button: Button::Left }));
    }

    /// A left drag in steps, released at `to`.
    fn drag(&mut self, from: ScreenPoint, to: ScreenPoint) {
        self.down(from);
        for k in 1..=4u8 {
            let t = f32::from(k) / 4.0;
            self.move_to(ScreenPoint {
                x: from.x + (to.x - from.x) * t,
                y: from.y + (to.y - from.y) * t,
            });
        }
        self.up(to);
    }

    /// Selects the line between `from` and `to` with a click on the
    /// middle of its first segment, from an empty selection.
    fn select_line(&mut self, from: &str, to: &str) {
        self.s.selection.clear();
        let l = self.line(from, to);
        let (a, b) = (l.points[0], l.points[1]);
        let at = self.at(i32::midpoint(a.x, b.x), i32::midpoint(a.y, b.y));
        self.down(at);
        self.up(at);
        assert_eq!(self.s.selection.len(), 1, "the line is selected");
    }

    /// The two ends of the line `from -> to` on screen.
    fn ends(&self, from: &str, to: &str) -> (ScreenPoint, ScreenPoint) {
        let l = self.line(from, to);
        let (a, b) = (l.points[0], *l.points.last().expect("end"));
        (self.at(a.x, a.y), self.at(b.x, b.y))
    }

    fn undo_steps(&self) -> usize {
        self.s.history.undo.len()
    }
}

#[test]
fn p4_213_selected_line_shows_end_handles() {
    p4_213(Ids::Flat);
}

/// As `p4_213_selected_line_shows_end_handles`, on nested containers under
/// `!identifiers hierarchical` (P2-142): the line's ends are named by their
/// full ids.
#[test]
fn p4_213_selected_line_shows_end_handles_hierarchical_ids() {
    p4_213(Ids::Hierarchical);
}

fn p4_213(ids: Ids) {
    let mut sh = Shell::open(&shop(ids, &[]));
    assert!(sh.s.end_handles().is_empty(), "no handles until a line is selected");
    sh.select_line("Web", "Db");
    let (a, b) = sh.ends("Web", "Db");
    assert_eq!(sh.s.end_handles(), [a, b], "one handle per end, source first");
    let items = crate::canvas::anchor_items(&sh.s.draw_list(), &sh.s.end_handles(), &sh.s.snap_marks());
    assert_eq!(items.len(), 4, "two round handles, each a white rim and a dot");
    assert!(sh.s.snap_marks().is_empty(), "snap points only while an end is dragged");
}

#[test]
fn p4_214_drag_shows_snap_points_and_snaps() {
    p4_214(Ids::Flat);
}

/// As `p4_214_drag_shows_snap_points_and_snaps`, on nested containers under
/// `!identifiers hierarchical` (P2-142): the line's ends are named by their
/// full ids.
#[test]
fn p4_214_drag_shows_snap_points_and_snaps_hierarchical_ids() {
    p4_214(Ids::Hierarchical);
}

fn p4_214(ids: Ids) {
    let mut sh = Shell::open(&shop(ids, &[]));
    sh.select_line("Web", "Db");
    let before = sh.text();
    let (_, end) = sh.ends("Web", "Db");
    sh.down(end);
    // Near db's top-left corner (80, 730), a few px off.
    let near = sh.at(84, 733);
    sh.move_to(near);
    let marks = sh.s.snap_marks();
    let db = sh.scene().boxes.iter().find(|b| b.rect.min == Point::new(80, 730)).expect("db").rect;
    let grid = c4rizer_core::geom::Rect::from_corners(Point::new(db.min.x / 10, db.min.y / 10), Point::new(db.max.x / 10, db.max.y / 10));
    assert_eq!(marks.len(), c4rizer_core::anchor::snap_points(grid).len(), "every snap point of db, none of another box");
    let snapped: Vec<_> = marks.iter().filter(|m| m.snapped).collect();
    assert_eq!(snapped.len(), 1);
    assert_eq!(snapped[0].at, sh.at(80, 730), "the corner");
    assert_eq!(sh.s.end_handles()[1], sh.at(80, 730), "the handle sits on the snapped point");
    // The preview routes the line to the corner already.
    assert_eq!(*sh.line("Web", "Db").points.last().expect("end"), Point::new(80, 730));
    assert_eq!(sh.text(), before, "nothing written mid-drag");
    // Far from every point: nothing snapped, the handle under the pointer.
    let far = sh.at(500, 620);
    sh.move_to(far);
    assert!(sh.s.snap_marks().iter().all(|m| !m.snapped));
    assert_eq!(sh.s.end_handles()[1], far);
    sh.up(far);
    assert_eq!(sh.text(), before, "released away from every point with no pin before: nothing to write");
    assert_eq!(sh.undo_steps(), 0);
}

#[test]
fn p4_215_release_on_snap_point_pins_end() {
    p4_215(Ids::Flat);
}

/// As `p4_215_release_on_snap_point_pins_end`, on nested containers under
/// `!identifiers hierarchical` (P2-142): the line's ends are named by their
/// full ids.
#[test]
fn p4_215_release_on_snap_point_pins_end_hierarchical_ids() {
    p4_215(Ids::Hierarchical);
}

fn p4_215(ids: Ids) {
    let mut sh = Shell::open(&shop(ids, &[]));
    sh.select_line("Web", "Db");
    let (_, end) = sh.ends("Web", "Db");
    // db's top side at a quarter: (140, 730).
    sh.drag(end, sh.at(142, 727));
    let text = sh.text();
    assert!(text.contains(&ids.refs("            at db 200 800\n            anchor web -> db to top 25\n")), "{text}");
    assert_eq!(sh.undo_steps(), 1);
    let l = sh.line("Web", "Db");
    assert_eq!(*l.points.last().expect("end"), Point::new(140, 730));
    // The other end next: the first pin is kept.
    sh.select_line("Web", "Db");
    let (start, _) = sh.ends("Web", "Db");
    sh.drag(start, sh.at(81, 402));
    let text = sh.text();
    assert!(text.contains(&ids.refs("anchor web -> db from left to top 25\n")), "{text}");
    assert_eq!(sh.line("Web", "Db").points[0], Point::new(80, 400));
    assert_eq!(sh.undo_steps(), 2);
}

#[test]
fn p4_216_release_away_removes_pin() {
    p4_216(Ids::Flat);
}

/// As `p4_216_release_away_removes_pin`, on nested containers under
/// `!identifiers hierarchical` (P2-142): the line's ends are named by their
/// full ids.
#[test]
fn p4_216_release_away_removes_pin_hierarchical_ids() {
    p4_216(Ids::Hierarchical);
}

fn p4_216(ids: Ids) {
    let mut sh = Shell::open(&shop(ids, &["anchor web -> db to top 25"]));
    assert_eq!(*sh.line("Web", "Db").points.last().expect("end"), Point::new(140, 730));
    sh.select_line("Web", "Db");
    let (_, end) = sh.ends("Web", "Db");
    sh.drag(end, sh.at(500, 620));
    assert_eq!(sh.text(), shop(ids, &[]), "the last pin removed deletes the anchor line");
    assert_eq!(sh.undo_steps(), 1);
    let plain = Shell::open(&shop(ids, &[]));
    assert_eq!(sh.scene().lines, plain.scene().lines, "the router's line again");
}

#[test]
fn p4_217_context_menu_releases_an_end() {
    p4_217(Ids::Flat);
}

/// As `p4_217_context_menu_releases_an_end`, on nested containers under
/// `!identifiers hierarchical` (P2-142): the line's ends are named by their
/// full ids.
#[test]
fn p4_217_context_menu_releases_an_end_hierarchical_ids() {
    p4_217(Ids::Hierarchical);
}

fn p4_217(ids: Ids) {
    let mut sh = Shell::open(&shop(ids, &["anchor web -> db from left to right"]));
    let l = sh.line("Web", "Db");
    let (a, b) = (l.points[1], l.points[2]);
    let at = sh.at(i32::midpoint(a.x, b.x), i32::midpoint(a.y, b.y));
    sh.event(Event::Input(Input::PointerDown {
        at,
        button: Button::Right,
        mods: Modifiers::default(),
    }));
    sh.event(Event::Input(Input::PointerUp { at, button: Button::Right }));
    let Some(Overlay::ContextMenu { entries }) = sh.s.overlay.clone() else {
        panic!("a menu opens: {:?}", sh.s.overlay);
    };
    let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(labels, ["Release start", "Release end"]);
    let release_end = entries[1].action.clone();
    assert!(matches!(release_end, MenuAction::ReleaseAnchor { destination: true, .. }));
    sh.event(Event::Command(Command::ContextAction(release_end)));
    assert!(sh.text().contains(&ids.refs("            anchor web -> db from left\n")), "{}", sh.text());
    assert_eq!(sh.undo_steps(), 1);
    // A line with no pin offers neither, but the menu opens.
    // owner decision 2026-11-19: behaviour change 5
    let mut plain = Shell::open(&shop(ids, &[]));
    let l = plain.line("Web", "Db");
    let at = plain.at(i32::midpoint(l.points[0].x, l.points[1].x), i32::midpoint(l.points[0].y, l.points[1].y));
    plain.event(Event::Input(Input::PointerDown {
        at,
        button: Button::Right,
        mods: Modifiers::default(),
    }));
    plain.event(Event::Input(Input::PointerUp { at, button: Button::Right }));
    let Some(Overlay::ContextMenu { entries }) = &plain.s.overlay else {
        panic!("a menu opens on every line: {:?}", plain.s.overlay);
    };
    assert!(entries.iter().all(|e| !matches!(e.action, MenuAction::ReleaseAnchor { .. })));
}

#[test]
fn p4_218_anchor_edit_atomic_one_undo_step() {
    p4_218(Ids::Flat);
}

/// As `p4_218_anchor_edit_atomic_one_undo_step`, on nested containers under
/// `!identifiers hierarchical` (P2-142): the line's ends are named by their
/// full ids.
#[test]
fn p4_218_anchor_edit_atomic_one_undo_step_hierarchical_ids() {
    p4_218(Ids::Hierarchical);
}

fn p4_218(ids: Ids) {
    let original = shop(ids, &[]);
    let mut sh = Shell::open(&original);
    sh.select_line("Web", "Db");
    let (_, end) = sh.ends("Web", "Db");
    sh.drag(end, sh.at(142, 727));
    let pinned = sh.text();
    assert!(pinned.contains(&ids.refs("anchor web -> db to top 25\n")), "{pinned}");
    sh.event(Event::Command(Command::Undo));
    assert_eq!(sh.text(), original, "undo gives the bytes back");
    sh.event(Event::Command(Command::Redo));
    assert_eq!(sh.text(), pinned);
    // EDIT-24: a change on disk the editor has not seen is kept; the
    // anchor edit is re-applied to it.
    let external = pinned.replace("\"Buys goods.\"", "\"Buys more goods.\"");
    std::fs::write(&sh.path, &external).expect("external save");
    sh.select_line("Web", "Db");
    let (_, end) = sh.ends("Web", "Db");
    sh.drag(end, sh.at(500, 620));
    let text = sh.text();
    assert!(text.contains("Buys more goods."), "the external change survives: {text}");
    assert!(!text.contains("anchor"), "{text}");
}

/// The workspace JSON (P3-413) the core exports for `dsl`.
fn json_of(dsl: &str) -> String {
    let m = c4rizer_core::model::parse(dsl).expect("parses").model;
    let all = c4rizer_core::pipeline::build_all(&m, &AtomicBool::new(false)).expect("never stopped");
    c4rizer_core::export::json(&m, &all, Some(dsl))
}

/// P3-420: on a workspace JSON no end handle is shown and an end drag is
/// refused with the JSON message; nothing is written.
#[test]
fn p4_213_no_end_handles_on_json_workspace() {
    let json = json_of(&shop(Ids::Flat, &[]));
    let mut sh = Shell::open_as(&json, "shop.json");
    assert!(sh.s.read_only());
    let before = sh.text();
    sh.select_line("Web", "Db");
    assert!(sh.s.end_handles().is_empty());
    let (_, end) = sh.ends("Web", "Db");
    sh.drag(end, sh.at(142, 727));
    assert!(sh.s.status().notice.is_some_and(|n| n.contains("JSON")), "{:?}", sh.s.status());
    assert_eq!(sh.text(), before);
    assert_eq!(sh.undo_steps(), 0);
}
