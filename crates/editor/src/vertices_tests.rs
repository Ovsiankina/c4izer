//! Tests of the vertices row, editor part (SRS 4.7 P3-310..P3-315),
//! driven headlessly like `state`'s own tests: effects run synchronously
//! with `files` and `worker::serve`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::Point;
use c4rizer_core::scene::{Scene, SceneLine};

use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState};
use crate::state::{Button, Command, Effect, Event, Input, Modifiers, State};
use crate::testutil::TempDir;
use crate::worker;

/// A shop, roles only, its container view placed by hand; `{vias}` is
/// replaced by the view's `via` lines.
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
{vias}
        }
    }
}
"#;

/// `SHOP` with `vias` (each a whole `via ...` line).
fn shop(vias: &[&str]) -> String {
    let lines: String = vias.iter().fold(String::new(), |mut out, v| {
        out.push_str("            ");
        out.push_str(v);
        out.push('\n');
        out
    });
    SHOP.replace("{vias}\n", &lines)
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
        let dir = TempDir::new("vertices");
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

    fn down(&mut self, at: ScreenPoint, mods: Modifiers) {
        self.event(Event::Input(Input::PointerDown {
            at,
            button: Button::Left,
            mods,
        }));
    }

    fn up(&mut self, at: ScreenPoint) {
        self.event(Event::Input(Input::PointerUp { at, button: Button::Left }));
    }

    fn click(&mut self, at: ScreenPoint, mods: Modifiers) {
        self.down(at, mods);
        self.up(at);
    }

    fn double_click(&mut self, at: ScreenPoint) {
        self.click(at, Modifiers::default());
        self.click(at, Modifiers::default());
        self.event(Event::Input(Input::DoubleClick {
            at,
            mods: Modifiers::default(),
        }));
    }

    /// A left drag in steps (so the click slop is passed and the preview
    /// is requested), released at `to`.
    fn drag(&mut self, from: ScreenPoint, to: ScreenPoint, mods: Modifiers) {
        self.down(from, mods);
        for k in 1..=4u8 {
            let t = f32::from(k) / 4.0;
            let at = ScreenPoint {
                x: from.x + (to.x - from.x) * t,
                y: from.y + (to.y - from.y) * t,
            };
            self.event(Event::Input(Input::PointerMove { at }));
        }
        self.up(to);
    }

    /// Selects the line between `from` and `to` with a click on the
    /// middle of its first segment, from an empty selection (a click on
    /// the line already selected would add a vertex).
    fn select_line(&mut self, from: &str, to: &str) {
        self.s.selection.clear();
        let l = self.line(from, to);
        let (a, b) = (l.points[0], l.points[1]);
        let at = self.at(i32::midpoint(a.x, b.x), i32::midpoint(a.y, b.y));
        self.click(at, Modifiers::default());
        assert_eq!(self.s.selection.len(), 1, "the line is selected");
    }

    fn undo_steps(&self) -> usize {
        self.s.history.undo.len()
    }
}

/// True when scene px `p` lies on `points`.
fn on_line(points: &[Point], p: Point) -> bool {
    points.windows(2).any(|w| {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let (px, py) = (p.x - a.x, p.y - a.y);
        dx * py == dy * px && px * dx + py * dy >= 0 && px * px + py * py <= dx * dx + dy * dy
    })
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::default()
    }
}

#[test]
fn p3_310_click_on_selected_line_adds_vertex() {
    let mut sh = Shell::open(&shop(&[]));
    let before = sh.text();
    sh.select_line("Web", "Db");
    assert_eq!(sh.text(), before, "selecting writes nothing");
    let l = sh.line("Web", "Db");
    let (a, b) = (l.points[0], l.points[1]);
    // A click a little off a grid point on the selected line, away from
    // its label.
    let mid = Point::new((a.x + (b.x - a.x) / 5) / 10 * 10, (a.y + (b.y - a.y) / 5) / 10 * 10);
    let mut at = sh.at(mid.x, mid.y);
    at.x += 2.0 * sh.s.camera.zoom;
    at.y += 3.0 * sh.s.camera.zoom;
    sh.click(at, Modifiers::default());
    let want = format!("            at db 200 800\n            via web -> db {} {}\n", mid.x, mid.y);
    assert!(sh.text().contains(&want), "{}", sh.text());
    assert_eq!(sh.undo_steps(), 1);
    assert!(on_line(&sh.line("Web", "Db").points, mid), "the new scene passes the vertex");
    // A second click further along adds a second vertex after the first.
    sh.select_line("Web", "Db");
    let l = sh.line("Web", "Db");
    let last = l.points[l.points.len() - 2];
    let end = l.points[l.points.len() - 1];
    let near_end = Point::new((last.x + (end.x - last.x) * 4 / 5) / 10 * 10, (last.y + (end.y - last.y) * 4 / 5) / 10 * 10);
    sh.click(sh.at(near_end.x, near_end.y), Modifiers::default());
    let want = format!("via web -> db {} {} {} {}\n", mid.x, mid.y, near_end.x, near_end.y);
    assert!(sh.text().contains(&want), "{}", sh.text());
    // A click on an unselected line only selects it.
    let text = sh.text();
    let other = sh.line("Api", "Db");
    let (a, b) = (other.points[0], other.points[1]);
    sh.click(sh.at(i32::midpoint(a.x, b.x), i32::midpoint(a.y, b.y)), Modifiers::default());
    assert_eq!(sh.text(), text);
}

#[test]
fn p3_311_drag_handle_moves_vertex() {
    let mut sh = Shell::open(&shop(&["via web -> db 0 600 0 700"]));
    assert!(sh.s.vertex_handles().is_empty(), "no handles until the line is selected");
    sh.select_line("Web", "Db");
    let handles = sh.s.vertex_handles();
    assert_eq!(handles, [sh.at(0, 600), sh.at(0, 700)], "one handle per vertex");
    assert_eq!(crate::canvas::vertex_handle_items(&sh.s.draw_list(), &handles).len(), 2, "painted");
    let before = sh.text();
    // Mid-drag: the preview routes the line through the moved vertex.
    let from = sh.at(0, 600);
    sh.down(from, Modifiers::default());
    let to = sh.at(-96, 612);
    sh.event(Event::Input(Input::PointerMove { at: to }));
    assert!(on_line(&sh.line("Web", "Db").points, Point::new(-100, 610)), "{:?}", sh.line("Web", "Db").points);
    assert_eq!(sh.s.vertex_handles()[0], sh.at(-100, 610), "the handle follows, snapped");
    assert_eq!(sh.text(), before, "nothing written mid-drag");
    sh.up(to);
    let after = sh.text();
    assert_eq!(after, before.replace("via web -> db 0 600 0 700", "via web -> db -100 610 0 700"), "only that via line changes");
    assert_eq!(sh.undo_steps(), 1);
    // A press and release on a handle without moving writes nothing.
    sh.select_line("Web", "Db");
    let at = sh.at(0, 700);
    sh.click(at, Modifiers::default());
    assert_eq!(sh.text(), after);
}

#[test]
fn p3_312_double_click_handle_removes_vertex() {
    let mut sh = Shell::open(&shop(&["via web -> db 0 600 0 700"]));
    sh.select_line("Web", "Db");
    sh.double_click(sh.at(0, 600));
    assert!(sh.text().contains("            via web -> db 0 700\n"), "{}", sh.text());
    assert_eq!(sh.undo_steps(), 1, "the double click is one step and its clicks added nothing");
    sh.select_line("Web", "Db");
    sh.double_click(sh.at(0, 700));
    assert!(!sh.text().contains("via"), "the last vertex deletes the via line: {}", sh.text());
    assert_eq!(sh.text(), shop(&[]));
}

#[test]
fn p3_313_remove_all_vertices_one_step() {
    let with = shop(&["via web -> db 0 600", "via api -> db 900 600"]);
    let mut sh = Shell::open(&with);
    sh.event(Event::Command(Command::RemoveAllVertices));
    assert_eq!(sh.text(), shop(&[]));
    assert_eq!(sh.undo_steps(), 1);
    // The canvas equals the routed view with no vertices.
    let mut plain = Shell::open(&shop(&[]));
    assert_eq!(sh.scene().lines, plain.scene().lines);
    sh.event(Event::Command(Command::Undo));
    let m = c4rizer_core::model::parse(&sh.text()).expect("parses").model;
    assert_eq!(m.view("c2").expect("view").vias.len(), 2, "one undo brings both back");
    // Nothing to remove writes nothing.
    plain.event(Event::Command(Command::RemoveAllVertices));
    assert_eq!(plain.undo_steps(), 0);
}

#[test]
fn p3_314_single_move_keeps_vertices_multi_move_carries_them() {
    let mut sh = Shell::open(&shop(&["via web -> db 0 600", "via api -> db 900 600"]));
    let zoom = sh.s.camera.zoom;
    // One box: the vertices stay where they are.
    let from = sh.at(200, 400);
    let to = ScreenPoint {
        x: from.x + 100.0 * zoom,
        y: from.y,
    };
    sh.drag(from, to, Modifiers::default());
    let text = sh.text();
    assert!(text.contains("at web 300 400"), "{text}");
    assert!(text.contains("via web -> db 0 600\n") && text.contains("via api -> db 900 600\n"), "{text}");
    assert_eq!(sh.undo_steps(), 1);
    // Web and db together: web -> db's vertices move with them, api ->
    // db's (api stays) do not; one write, one undo step.
    sh.click(sh.at(300, 400), Modifiers::default());
    sh.click(sh.at(200, 800), shift());
    assert_eq!(sh.s.selection.len(), 2);
    let from = sh.at(300, 400);
    let to = ScreenPoint {
        x: from.x + 100.0 * zoom,
        y: from.y + 50.0 * zoom,
    };
    sh.drag(from, to, Modifiers::default());
    let text = sh.text();
    assert!(text.contains("at web 400 450") && text.contains("at db 300 850"), "{text}");
    assert!(text.contains("via web -> db 100 650\n"), "{text}");
    assert!(text.contains("via api -> db 900 600\n"), "{text}");
    assert_eq!(sh.undo_steps(), 2);
    sh.event(Event::Command(Command::Undo));
    let back = sh.text();
    assert!(back.contains("at web 300 400") && back.contains("via web -> db 0 600\n"), "{back}");
}

#[test]
fn p3_315_vertex_edit_atomic_one_undo_step() {
    let original = shop(&["via web -> db 0 600"]);
    let mut sh = Shell::open(&original);
    sh.select_line("Web", "Db");
    sh.drag(sh.at(0, 600), sh.at(-100, 600), Modifiers::default());
    let moved = sh.text();
    assert!(moved.contains("via web -> db -100 600\n"), "{moved}");
    assert_eq!(sh.undo_steps(), 1, "a whole drag is one step");
    sh.event(Event::Command(Command::Undo));
    assert_eq!(sh.text(), original, "undo gives the bytes back");
    sh.event(Event::Command(Command::Redo));
    assert_eq!(sh.text(), moved);
    // EDIT-24: a change on disk the editor has not seen is kept; the
    // vertex edit is re-applied to it, never written over it.
    let external = moved.replace("\"Buys goods.\"", "\"Buys more goods.\"");
    std::fs::write(&sh.path, &external).expect("external save");
    sh.select_line("Web", "Db");
    sh.double_click(sh.at(-100, 600));
    let text = sh.text();
    assert!(text.contains("Buys more goods."), "the external change survives: {text}");
    assert!(!text.contains("via"), "{text}");
}

/// The workspace JSON (P3-413) the core exports for `dsl`.
fn json_of(dsl: &str) -> String {
    let m = c4rizer_core::model::parse(dsl).expect("parses").model;
    let all = c4rizer_core::pipeline::build_all(&m, &AtomicBool::new(false)).expect("never stopped");
    c4rizer_core::export::json(&m, &all, Some(dsl))
}

/// P3-420: on a workspace JSON the vertex gestures are refused with the
/// JSON message (the line's vertices are read back from the JSON) and
/// nothing is written.
#[test]
fn p3_420_vertex_edits_refused_on_json_workspace() {
    let json = json_of(&shop(&["via web -> db 0 600 0 700"]));
    let mut sh = Shell::open_as(&json, "shop.json");
    assert!(sh.s.read_only());
    let before = sh.text();
    let refused = |sh: &Shell| sh.s.status().notice.is_some_and(|n| n.contains("JSON"));
    sh.select_line("Web", "Db");
    assert!(sh.s.vertex_handles().is_empty(), "no handles on a read-only file");
    // Add: a click on the selected line, off its vertices.
    let l = sh.line("Web", "Db");
    let (a, b) = (l.points[0], l.points[1]);
    sh.click(sh.at(a.x + (b.x - a.x) / 5, a.y + (b.y - a.y) / 5), Modifiers::default());
    assert!(refused(&sh), "add refused with the JSON message: {:?}", sh.s.status());
    // Drag a vertex.
    sh.s.notice = None;
    sh.select_line("Web", "Db");
    sh.drag(sh.at(0, 600), sh.at(-100, 600), Modifiers::default());
    assert!(refused(&sh), "drag refused: {:?}", sh.s.status());
    // Delete a vertex.
    sh.s.notice = None;
    sh.select_line("Web", "Db");
    sh.double_click(sh.at(0, 700));
    assert!(refused(&sh), "delete refused: {:?}", sh.s.status());
    // Remove all vertices.
    sh.s.notice = None;
    sh.event(Event::Command(Command::RemoveAllVertices));
    assert!(refused(&sh), "remove all refused: {:?}", sh.s.status());
    assert_eq!(sh.text(), before, "nothing written");
    assert_eq!(sh.undo_steps(), 0);
}

/// P3-420: "remove all vertices" on a workspace JSON says why it does
/// nothing even when the view has no vertices.
#[test]
fn p3_420_remove_all_refused_on_json_without_vertices() {
    let json = json_of(&shop(&[]));
    let mut sh = Shell::open_as(&json, "shop.json");
    assert!(sh.s.read_only());
    sh.s.notice = None;
    sh.event(Event::Command(Command::RemoveAllVertices));
    assert!(sh.s.status().notice.is_some_and(|n| n.contains("JSON")), "{:?}", sh.s.status());
    assert_eq!(sh.undo_steps(), 0);
}

/// P3-507: with Explore open, no vertex gesture or command reaches the
/// state (the gate is an allow-list).
#[test]
fn p3_507_explore_gate_drops_vertex_events() {
    let at = ScreenPoint { x: 10.0, y: 10.0 };
    let events = vec![
        Event::Command(Command::RemoveAllVertices),
        Event::Input(Input::PointerDown {
            at,
            button: Button::Left,
            mods: Modifiers::default(),
        }),
        Event::Input(Input::PointerMove { at }),
        Event::Input(Input::PointerUp { at, button: Button::Left }),
        Event::Input(Input::DoubleClick {
            at,
            mods: Modifiers::default(),
        }),
    ];
    let (kept, leave) = crate::app::explore_gate(events);
    assert!(kept.is_empty(), "{kept:?}");
    assert!(!leave);
}
