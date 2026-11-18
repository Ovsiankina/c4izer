//! Tests of the deployment-view row, editor part (P3-131), driven
//! headlessly like `state`'s own tests: effects run synchronously with
//! `files` and `worker::serve`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::model::Model;
use c4rizer_core::scene::Scene;
use c4rizer_core::view::BoundaryOf;

use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState};
use crate::state::{Button, Command, Effect, Event, Input, Modifiers, Selected, State, enterable};
use crate::testutil::TempDir;
use crate::worker::{self, ViewRef};

/// The row's fixture.
const FIXTURE: &str = include_str!("../../../tests/data/deployment_view.dsl");

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    _dir: TempDir,
}

impl Shell {
    fn with(text: &str) -> Shell {
        let dir = TempDir::new("deployment-view");
        let path = dir.join("deployment.dsl");
        std::fs::write(&path, text).expect("fixture written");
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

    fn model(&self) -> &Model {
        &self.s.doc.as_ref().expect("a file").parsed.as_ref().expect("a parse").model
    }

    fn scene(&self) -> &Scene {
        self.s.scene().expect("a scene")
    }

    fn box_centre(&self, key: &str) -> ScreenPoint {
        let id = self.model().find(key).expect("element");
        let b = self.scene().boxes_of(id)[0];
        self.s.camera.to_screen(self.scene().boxes[b.0 as usize].rect.centre())
    }

    /// A point on the tag of deployment node `key`'s boundary.
    fn node_tag(&self, key: &str) -> ScreenPoint {
        let id = self.model().find(key).expect("node");
        let b = self
            .scene()
            .boundaries
            .iter()
            .find(|b| b.of == BoundaryOf::DeploymentNode(id))
            .expect("boundary");
        self.s.camera.to_screen(b.tag.bounds.centre())
    }

    fn click_at(&mut self, at: ScreenPoint) {
        let mods = Modifiers::default();
        self.event(Event::Input(Input::PointerDown {
            at,
            button: Button::Left,
            mods,
        }));
        self.event(Event::Input(Input::PointerUp { at, button: Button::Left }));
    }

    fn drag(&mut self, from: ScreenPoint, dx: f32, dy: f32) {
        let mods = Modifiers::default();
        self.event(Event::Input(Input::PointerDown {
            at: from,
            button: Button::Left,
            mods,
        }));
        for i in 1..=4_u8 {
            let f = f32::from(i) / 4.0;
            self.event(Event::Input(Input::PointerMove {
                at: ScreenPoint {
                    x: from.x + dx * f,
                    y: from.y + dy * f,
                },
            }));
        }
        self.event(Event::Input(Input::PointerUp {
            at: ScreenPoint {
                x: from.x + dx,
                y: from.y + dy,
            },
            button: Button::Left,
        }));
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.path).expect("file")
    }
}

fn declared(key: &str) -> ViewRef {
    ViewRef::Declared(key.to_owned())
}

/// Dragging an instance writes its `at` line; a click on a node boundary
/// selects it and the details show technology, `instances` and
/// environment; a node is never enterable.
#[test]
fn p3_131_drag_writes_at_click_selects_node_never_enters() {
    let mut sh = Shell::with(FIXTURE);
    sh.event(Event::Command(Command::ShowView(declared("live"))));
    assert_eq!(sh.scene().key, "live");
    // A drag writes the instance's `at` line into the view (DSL-9).
    let from = sh.box_centre("mailInstance");
    sh.drag(from, 200.0, 0.0);
    let text = sh.text();
    let at = text
        .lines()
        .find(|l| l.trim_start().starts_with("at mailInstance "))
        .unwrap_or_else(|| panic!("{text}"));
    assert_eq!(at.split_whitespace().count(), 4, "{at}");
    // Its node's boundary follows on the next build (EDIT-25).
    let provider = sh.model().find("provider").expect("node");
    let mail = sh.model().find("mailInstance").expect("instance");
    let scene = sh.scene();
    let boundary = scene
        .boundaries
        .iter()
        .find(|b| b.of == BoundaryOf::DeploymentNode(provider))
        .expect("boundary");
    let mail_rect = scene.boxes[scene.boxes_of(mail)[0].0 as usize].rect;
    assert!(boundary.rect.min.x < mail_rect.min.x && boundary.rect.max.x > mail_rect.max.x);
    // A click on a node's tag selects the node; the details name it.
    let tag = sh.node_tag("webHost");
    sh.click_at(tag);
    assert_eq!(
        sh.s.selection.iter().cloned().collect::<Vec<_>>(),
        [Selected::Element("webHost".to_owned())]
    );
    let d = sh.s.details().expect("details");
    assert_eq!(d.name, "Web host");
    assert_eq!(d.kind, "Deployment Node");
    assert_eq!(d.technology.as_deref(), Some("EC2"));
    assert_eq!(d.instances.as_deref(), Some("2"));
    assert_eq!(d.environment.as_deref(), Some("Live"));
    // Never enterable (EDIT-15): a double click on it opens no view.
    let web_host = sh.model().find("webHost").expect("node");
    assert!(!enterable(sh.model(), web_host));
    sh.click_at(tag);
    sh.event(Event::Input(Input::DoubleClick {
        at: tag,
        mods: Modifiers::default(),
    }));
    assert_eq!(sh.s.view, Some(declared("live")));
    assert!(!enterable(sh.model(), mail));
}
