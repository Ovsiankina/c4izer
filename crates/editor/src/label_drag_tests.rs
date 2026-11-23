//! Tests of the label drag (P2-708 by drag): a label pressed and dragged
//! past the click slop slides along its line, release writes the view's
//! `line … position N` through the Up/Down action, driven headlessly like
//! `anchors_tests`: effects run synchronously with `files` and
//! `worker::serve`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::Point;
use c4rizer_core::scene::{ItemKey, Scene, SceneLabel, SceneLine};

use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState};
use crate::state::{
    Button, Command, Effect, Event, Gesture, Input, Key, Mark, MarkKind, Modifiers, Selected, State,
};
use crate::testutil::{Ids, SHOP_NESTED, TempDir};
use crate::worker;

/// A shop container view placed by hand, roomy enough for callouts:
/// web -> db runs straight right from x 320 to 780 at y 500; `{lines}` is
/// replaced by the view's extra lines.
const SHOP: &str = r#"workspace "Shop" {
    model {
        customer = person "Customer" "Buys goods."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust"
            db = container "Db" "Stores orders." "SQL"
        }
        customer -> web "Browses" "HTTPS"
        web -> db "Reads orders" "SQL"
    }
    views {
        container shop "c2" "Shop" {
            include *
            at customer 200 100
            at web 200 500
            at db 900 500
{lines}
        }
    }
}
"#;

fn shop(ids: Ids, lines: &[&str]) -> String {
    let text: String = lines.iter().fold(String::new(), |mut out, v| {
        out.push_str("            ");
        out.push_str(v);
        out.push('\n');
        out
    });
    ids.workspace_in(&SHOP.replace("{lines}\n", &text), SHOP_NESTED)
}

const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
};

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    _dir: TempDir,
}

impl Shell {
    fn open(text: &str) -> Shell {
        let dir = TempDir::new("labeldrag");
        let path = dir.join("shop.dsl");
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

    /// The scene line web -> db (the horizontal one).
    fn line(&self) -> SceneLine {
        self.scene()
            .lines
            .iter()
            .find(|l| l.points.iter().all(|p| p.y == l.points[0].y))
            .cloned()
            .expect("web -> db drawn straight")
    }

    /// Its label.
    fn label(&self) -> SceneLabel {
        let edge = self.line().edge;
        self.scene().labels.iter().find(|l| l.edge == edge).cloned().expect("labelled")
    }

    fn at(&self, p: Point) -> ScreenPoint {
        self.s.camera.to_screen(p)
    }

    fn down(&mut self, at: ScreenPoint) {
        self.event(Event::Input(Input::PointerDown {
            at,
            button: Button::Left,
            mods: NONE,
        }));
    }

    fn move_to(&mut self, at: ScreenPoint) {
        self.event(Event::Input(Input::PointerMove { at }));
    }

    fn up(&mut self, at: ScreenPoint) {
        self.event(Event::Input(Input::PointerUp { at, button: Button::Left }));
    }

    /// Presses at `from` and moves in steps to `to`, without releasing.
    fn press_and_move(&mut self, from: ScreenPoint, to: ScreenPoint) {
        self.down(from);
        for k in 1..=4u8 {
            let t = f32::from(k) / 4.0;
            self.move_to(ScreenPoint {
                x: from.x + (to.x - from.x) * t,
                y: from.y + (to.y - from.y) * t,
            });
        }
    }

    /// `p` moved right by `dx` scene px, on screen.
    fn right_of(&self, p: Point, dx: i32) -> ScreenPoint {
        self.at(Point::new(p.x + dx, p.y))
    }

    /// The whole percent of the straight web -> db line at scene x `x`.
    fn percent_at(&self, x: i32) -> u8 {
        let line = self.line();
        let (start, end) = (line.points[0], *line.points.last().expect("end"));
        let percent = f64::from(x - start.x) / f64::from(end.x - start.x) * 100.0;
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "0..100")]
        let whole = percent.round().clamp(0.0, 100.0) as u8;
        whole
    }

    fn undo_steps(&self) -> usize {
        self.s.history.undo.len()
    }
}

/// A label dragged along its line slides there in the preview, wears the
/// selection mark and the grabbing cursor, and release writes `position N`
/// in the view's `line` line, one undo step; undo restores the file.
#[test]
fn label_drag_writes_position_and_undoes() {
    body_label_drag_writes_position_and_undoes(Ids::Flat);
}

/// As `label_drag_writes_position_and_undoes`, on nested containers under
/// `!identifiers hierarchical` (P2-142): the `line` line names the
/// relationship by its ends' full ids.
#[test]
fn label_drag_writes_position_and_undoes_hierarchical_ids() {
    body_label_drag_writes_position_and_undoes(Ids::Hierarchical);
}

fn body_label_drag_writes_position_and_undoes(ids: Ids) {
    let mut sh = Shell::open(&shop(ids, &[]));
    let before = sh.text();
    let label = sh.label();
    let edge = label.edge;
    let centre = label.rect.centre();
    assert!(label.callout.is_none());
    let from = sh.at(centre);
    sh.move_to(from);
    assert_eq!(sh.s.label_cursor(), Some(false), "a grab hand over the label");
    let to = sh.right_of(centre, 150);
    sh.press_and_move(from, to);
    assert!(matches!(sh.s.gesture, Gesture::DraggingLabel { .. }), "{:?}", sh.s.gesture);
    assert_eq!(sh.s.label_cursor(), Some(true), "a closed hand while it slides");
    assert_eq!(sh.text(), before, "nothing written mid-drag");
    let moved = sh.label().rect.centre();
    assert!((moved.x - (centre.x + 150)).abs() <= 10, "the preview slides it: {centre:?} -> {moved:?}");
    assert_eq!(moved.y, centre.y, "along the line");
    assert!(
        sh.s.draw_list().marks.contains(&Mark {
            item: ItemKey::Label(edge),
            kind: MarkKind::Selected,
        }),
        "the dragged label wears the selection mark"
    );
    sh.up(to);
    let n = sh.percent_at(centre.x + 150);
    let text = sh.text();
    assert!(text.contains(&ids.refs_in(&format!("            line web -> db position {n}\n"), SHOP_NESTED)), "{text}");
    assert_eq!(sh.undo_steps(), 1);
    assert!(matches!(sh.s.selection.iter().next(), Some(Selected::Relationship(_))));
    let after = sh.label().rect.centre();
    assert!((after.x - (centre.x + 150)).abs() <= 10, "the build of the saved file keeps it there: {after:?}");
    sh.event(Event::Command(Command::Undo));
    assert_eq!(sh.text(), before, "undo restores the file");
    assert_eq!(sh.label().rect.centre(), centre);
}

/// A callout label dragged by its text (or by its attach dot) moves its
/// attach point along the line and writes `position N` the same way.
#[test]
fn callout_label_drag_writes_position() {
    let base = shop(Ids::Flat, &["labels callout"]);
    let mut sh = Shell::open(&base);
    let label = sh.label();
    let c = label.callout.clone().expect("a callout");
    let from = sh.at(label.rect.centre());
    let to = ScreenPoint {
        x: from.x - 120.0 * sh.s.camera.zoom,
        y: from.y,
    };
    sh.press_and_move(from, to);
    assert!(matches!(sh.s.gesture, Gesture::DraggingLabel { .. }));
    let dot = sh.label().callout.expect("still a callout").dot;
    assert!((dot.x - (c.dot.x - 120)).abs() <= 10, "the attach point slides: {:?} -> {dot:?}", c.dot);
    sh.up(to);
    let n = sh.percent_at(c.dot.x - 120);
    let text = sh.text();
    assert!(text.contains(&format!("            line web -> db position {n}\n")), "{text}");
    assert_eq!(sh.undo_steps(), 1);

    // The attach dot grabs the label too (the line under it does not).
    let mut sh = Shell::open(&base);
    let c = sh.label().callout.expect("a callout");
    let from = sh.at(c.dot);
    let to = sh.right_of(c.dot, 100);
    sh.press_and_move(from, to);
    assert!(matches!(sh.s.gesture, Gesture::DraggingLabel { .. }), "{:?}", sh.s.gesture);
    sh.up(to);
    let n = sh.percent_at(c.dot.x + 100);
    assert!(sh.text().contains(&format!("line web -> db position {n}\n")), "{}", sh.text());
}

/// A press on a label released within the click slop is a click: it
/// selects the line and writes nothing.
#[test]
fn label_press_under_slop_selects_line_only() {
    let mut sh = Shell::open(&shop(Ids::Flat, &[]));
    let before = sh.text();
    let from = sh.at(sh.label().rect.centre());
    let near = ScreenPoint {
        x: from.x + 2.0,
        y: from.y + 1.0,
    };
    sh.down(from);
    sh.move_to(near);
    sh.up(near);
    assert_eq!(sh.s.gesture, Gesture::Idle);
    assert_eq!(sh.s.selection.len(), 1);
    assert!(matches!(sh.s.selection.iter().next(), Some(Selected::Relationship(_))));
    assert_eq!(sh.text(), before);
    assert_eq!(sh.undo_steps(), 0);
}

/// Escape during a label drag drops it: the label is back where it was
/// and the release writes nothing.
#[test]
fn escape_cancels_label_drag() {
    let mut sh = Shell::open(&shop(Ids::Flat, &[]));
    let before = sh.text();
    let centre = sh.label().rect.centre();
    let from = sh.at(centre);
    let to = sh.right_of(centre, 150);
    sh.press_and_move(from, to);
    assert_ne!(sh.label().rect.centre(), centre);
    sh.event(Event::Input(Input::KeyDown {
        key: Key::Escape,
        mods: NONE,
        text_focus: false,
    }));
    assert_eq!(sh.s.gesture, Gesture::Idle);
    assert_eq!(sh.label().rect.centre(), centre, "the preview is dropped");
    sh.up(to);
    assert_eq!(sh.text(), before);
    assert_eq!(sh.undo_steps(), 0);
}

/// While the file does not parse (read-only), a drag on a label starts
/// nothing and writes nothing, as on a box.
#[test]
fn read_only_refuses_label_drag() {
    let mut sh = Shell::open(&shop(Ids::Flat, &[]));
    let broken = shop(Ids::Flat, &[]).replacen("    model {\n", "    model {\n        )\n", 1);
    std::fs::write(&sh.path, &broken).expect("external write");
    sh.event(Event::File(FileEvent::Changed {
        text: broken.clone(),
        hash: files::hash(broken.as_bytes()),
    }));
    assert!(sh.s.read_only());
    let centre = sh.label().rect.centre();
    let from = sh.at(centre);
    sh.move_to(from);
    assert_eq!(sh.s.label_cursor(), None, "no grab hand while read-only");
    let to = sh.right_of(centre, 150);
    sh.press_and_move(from, to);
    assert_eq!(sh.s.gesture, Gesture::Idle, "no drag, no rubber band");
    assert_eq!(sh.label().rect.centre(), centre);
    sh.up(to);
    assert_eq!(sh.text(), broken);
}

/// A double click on a label still opens the in-place editor.
#[test]
fn double_click_on_label_still_edits() {
    let mut sh = Shell::open(&shop(Ids::Flat, &[]));
    let at = sh.at(sh.label().rect.centre());
    sh.down(at);
    sh.up(at);
    sh.event(Event::Input(Input::DoubleClick { at, mods: NONE }));
    let edit = sh.s.text_edit.as_ref().expect("label edit");
    assert!(matches!(edit.item, Selected::Relationship(_)), "{:?}", edit.item);
}
