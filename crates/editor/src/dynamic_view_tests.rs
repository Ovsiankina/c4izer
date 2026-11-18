//! Tests of the dynamic-view row, editor part (SRS 4.7 P3-218..P3-221),
//! driven headlessly like `state`'s own tests: effects run synchronously
//! with `files` and `worker::serve`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::model::ViewKind;
use c4rizer_core::scene::Scene;

use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState};
use crate::state::{Button, Command, Effect, Event, Input, Key, Modifiers, Selected, State};
use crate::testutil::TempDir;
use crate::worker::{self, ViewRef};

/// A checkout flow, roles only: a plain view, a view with a parallel
/// block and a view with written orders.
const FLOW: &str = r#"workspace "Shop" {
    model {
        customer = person "Customer" "Buys goods."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust"
            db = container "Db" "Stores orders." "SQL"
        }
        customer -> web "Browses" "HTTPS"
        web -> db "Reads orders" "SQL"
        db -> web "Returns rows" "SQL"
    }
    views {
        dynamic shop "flow" "Checkout" {
            customer -> web "Opens the shop"
            web -> db
            db -> web "Returns the basket"
        }
        dynamic shop "par" "Parallel" {
            customer -> web "Opens the shop"
            {
                web -> db "Loads the basket"
                db -> web "Returns the basket"
            }
        }
        dynamic shop "written" "Written" {
            1: customer -> web "Opens the shop"
            2: web -> db "Loads the basket"
        }
    }
}
"#;

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    _dir: TempDir,
}

impl Shell {
    fn open() -> Shell {
        let dir = TempDir::new("dynamic-view");
        let path = dir.join("flow.dsl");
        std::fs::write(&path, FLOW).expect("fixture written");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
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

    fn show(&mut self, key: &str) {
        self.event(Event::Command(Command::ShowView(ViewRef::Declared(key.to_owned()))));
        assert_eq!(self.scene().key, key);
        assert_eq!(self.scene().kind, ViewKind::Dynamic);
    }

    /// A click on the label of the `edge`-th line.
    fn click_label(&mut self, edge: usize) {
        let rect = self.scene().labels[edge].rect;
        let at: ScreenPoint = self.s.camera.to_screen(rect.centre());
        self.event(Event::Input(Input::PointerDown {
            at,
            button: Button::Left,
            mods: Modifiers::default(),
        }));
        self.event(Event::Input(Input::PointerUp { at, button: Button::Left }));
    }

    fn key(&mut self, key: Key) {
        self.event(Event::Input(Input::KeyDown {
            key,
            mods: Modifiers::default(),
            text_focus: false,
        }));
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.path).expect("file")
    }

    fn selection(&self) -> Vec<Selected> {
        self.s.selection.iter().cloned().collect()
    }
}

/// The details panel of a selected step shows number, texts, ends and relationship; Enter edits the step's own description.
#[test]
fn p3_218_step_details_and_in_place_edit() {
    let mut sh = Shell::open();
    sh.show("flow");
    // The second step has no description of its own: its line reads the
    // relationship's.
    sh.click_label(1);
    assert_eq!(sh.selection(), [Selected::Step(1)]);
    let d = sh.s.details().expect("details");
    assert_eq!(d.kind, "Step");
    assert_eq!(d.name, "2: Reads orders");
    assert_eq!(d.description.as_deref(), Some("Reads orders"));
    assert_eq!(d.technology.as_deref(), Some("SQL"));
    let step = d.step.expect("step details");
    assert_eq!(step.order, "2");
    assert_eq!(step.ends, "Web -> Db");
    assert_eq!(step.relationship, "Web -> Db: Reads orders");
    // Enter edits the step's own description and writes only its line.
    sh.key(Key::Enter);
    sh.event(Event::Text("Loads the basket".to_owned()));
    sh.key(Key::Enter);
    let expected = FLOW.replacen("            web -> db\n", "            web -> db \"Loads the basket\"\n", 1);
    assert_eq!(sh.text(), expected);
    // The relationship keeps its own text.
    assert!(sh.text().contains("web -> db \"Reads orders\" \"SQL\""));
    // One undo step gives the bytes back.
    sh.event(Event::Command(Command::Undo));
    assert_eq!(sh.text(), FLOW);
}

/// The Steps list shows the steps in number order; a row selects its line, and the line its row.
#[test]
fn p3_219_steps_list_selects_line_and_back() {
    let mut sh = Shell::open();
    sh.show("par");
    let rows = sh.s.steps();
    let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    // Numbers 1, 2, 3 (the block holds steps, so nothing follows it).
    assert_eq!(labels, ["1: Opens the shop", "2: Loads the basket", "3: Returns the basket"]);
    assert!(rows.iter().all(|r| !r.selected));
    // A row selects its line.
    sh.event(Event::Command(Command::SelectStep(2)));
    assert_eq!(sh.selection(), [Selected::Step(2)]);
    let marks: Vec<_> = sh.s.draw_list().marks.iter().map(|m| m.item).collect();
    assert!(marks.contains(&c4rizer_core::scene::ItemKey::Line(c4rizer_core::view::EdgeId(2))), "{marks:?}");
    assert!(sh.s.steps()[2].selected);
    // The line selects its row.
    sh.click_label(0);
    assert_eq!(sh.selection(), [Selected::Step(0)]);
    assert!(sh.s.steps()[0].selected);
    assert!(!sh.s.steps()[2].selected);
    // Another dynamic view lists its own steps.
    sh.show("flow");
    assert_eq!(sh.s.steps().len(), 3);
}

/// Moving a plain step past its neighbour swaps the two lines only, one undo step.
#[test]
fn p3_220_move_step_swaps_lines_one_undo() {
    let mut sh = Shell::open();
    sh.show("flow");
    sh.event(Event::Command(Command::SelectStep(2)));
    sh.event(Event::Command(Command::MoveStep { step: 2, up: true }));
    let expected = FLOW.replacen(
        "            web -> db\n            db -> web \"Returns the basket\"\n",
        "            db -> web \"Returns the basket\"\n            web -> db\n",
        1,
    );
    assert_eq!(sh.text(), expected);
    // The selection follows the moved step.
    assert_eq!(sh.selection(), [Selected::Step(1)]);
    assert_eq!(sh.s.steps()[1].label, "2: Returns the basket");
    // Inside one block it moves too.
    sh.show("par");
    sh.event(Event::Command(Command::MoveStep { step: 1, up: false }));
    assert!(sh.text().contains("            {\n                db -> web \"Returns the basket\"\n                web -> db \"Loads the basket\"\n"), "{}", sh.text());
    // Each move is one undo step.
    sh.event(Event::Command(Command::Undo));
    sh.event(Event::Command(Command::Undo));
    assert_eq!(sh.text(), FLOW);
}

/// A reorder with a written order or across a block border is refused with a status message; no file changes.
#[test]
fn p3_221_reorder_refused_with_written_order_or_block() {
    let mut sh = Shell::open();
    sh.show("written");
    sh.event(Event::Command(Command::MoveStep { step: 1, up: true }));
    assert_eq!(sh.text(), FLOW);
    let notice = sh.s.status().notice.expect("a notice");
    assert!(notice.contains("written orders"), "{notice}");
    sh.show("par");
    sh.event(Event::Command(Command::MoveStep { step: 1, up: true }));
    assert_eq!(sh.text(), FLOW);
    let notice = sh.s.status().notice.expect("a notice");
    assert!(notice.contains("parallel block"), "{notice}");
}
