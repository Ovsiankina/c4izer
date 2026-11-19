//! Tests of the callout-labels row, editor part (SRS 4.8 P4-317,
//! P4-318), driven headlessly like `anchors_tests`: effects run
//! synchronously with `files` and `worker::serve`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::model::LabelMode;
use c4rizer_core::scene::{ItemKey, Scene};

use crate::canvas::{Primitive, Source};
use crate::files::{self, FileEvent, UiState};
use crate::state::{Command, Effect, Event, Input, Modifiers, Selected, State};
use crate::testutil::TempDir;
use crate::worker;

/// A shop container view placed by hand, roomy enough for callouts;
/// `{lines}` is replaced by the view's extra lines.
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

fn shop(lines: &[&str]) -> String {
    let text: String = lines.iter().fold(String::new(), |mut out, v| {
        out.push_str("            ");
        out.push_str(v);
        out.push('\n');
        out
    });
    SHOP.replace("{lines}\n", &text)
}

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    _dir: TempDir,
}

impl Shell {
    fn open(text: &str) -> Shell {
        let dir = TempDir::new("callouts");
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
}

/// P4-317: the canvas paints each callout's shelf, leader and dot from
/// the scene, in the label's own paint items.
#[test]
fn p4_317_canvas_draws_callouts_from_the_scene() {
    let sh = Shell::open(&shop(&["labels callout"]));
    let scene = sh.scene();
    let label = scene.labels.iter().find(|l| l.callout.is_some()).expect("a callout");
    let c = label.callout.as_ref().expect("callout");
    let list = sh.s.draw_list();
    let items: Vec<_> = crate::canvas::paint_items(&list)
        .into_iter()
        .filter(|i| i.source == Source::Scene(ItemKey::Label(label.edge)))
        .collect();
    let cam = &sh.s.camera;
    let paths: Vec<_> = items
        .iter()
        .filter_map(|i| match &i.primitive {
            Primitive::Path { points, .. } => Some(points.clone()),
            _ => None,
        })
        .collect();
    let screen = |p| cam.to_screen(p);
    assert_eq!(paths, vec![c.shelf.map(screen).to_vec(), c.leader.map(screen).to_vec()]);
    let dots = items
        .iter()
        .filter(|i| matches!(&i.primitive, Primitive::Circle { centre, .. } if *centre == cam.to_screen(c.dot)))
        .count();
    assert_eq!(dots, 1, "the dot at the attach point");
    assert!(
        !items.iter().any(|i| matches!(i.primitive, Primitive::Polygon { .. })),
        "no backing rectangle"
    );
}

/// P4-317: a callout label is selected by a click on its text block and
/// a double click edits its text in place.
#[test]
fn p4_317_callout_selectable_and_editable() {
    let mut sh = Shell::open(&shop(&["labels callout"]));
    let label = sh.scene().labels.iter().find(|l| l.callout.is_some()).expect("a callout").clone();
    let at = sh.s.camera.to_screen(label.rect.centre());
    assert_eq!(sh.scene().hit(label.rect.centre(), 0), Some(ItemKey::Label(label.edge)));
    sh.event(Event::Input(Input::DoubleClick {
        at,
        mods: Modifiers::default(),
    }));
    let edit = sh.s.text_edit.as_ref().expect("label edit");
    assert!(matches!(edit.item, Selected::Relationship(_)), "{:?}", edit.item);
    assert!(label.text.lines.iter().any(|l| l.contains(&edit.fields[0].buffer)), "{:?}", edit.fields[0].buffer);
}

/// P4-318: the checkbox writes `labels callout`, unchecking removes the
/// line, each one undo step.
#[test]
fn p4_318_label_mode_toggle_writes_and_undoes() {
    let original = shop(&["labelWidth 20"]);
    let mut sh = Shell::open(&original);
    assert_eq!(sh.s.label_mode(), Some(LabelMode::Line));
    assert!(sh.scene().labels.iter().all(|l| l.callout.is_none()));
    let steps = sh.s.history.undo.len();
    sh.event(Event::Command(Command::SetLabelMode(LabelMode::Callout)));
    assert!(sh.text().contains("labelWidth 20\n            labels callout\n"), "{}", sh.text());
    assert_eq!(sh.s.history.undo.len(), steps + 1, "one undo step");
    assert_eq!(sh.s.label_mode(), Some(LabelMode::Callout));
    assert!(sh.scene().labels.iter().any(|l| l.callout.is_some()), "the scene follows");
    // Setting the mode it has writes nothing.
    sh.event(Event::Command(Command::SetLabelMode(LabelMode::Callout)));
    assert_eq!(sh.s.history.undo.len(), steps + 1);
    sh.event(Event::Command(Command::Undo));
    assert_eq!(sh.text(), original);
    sh.event(Event::Command(Command::Redo));
    assert!(sh.text().contains("labels callout"));
    sh.event(Event::Command(Command::SetLabelMode(LabelMode::Line)));
    assert_eq!(sh.text(), original, "unchecked: the line is removed");
}
