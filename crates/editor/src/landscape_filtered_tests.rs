//! Step-4 contract tests of the landscape-filtered row, editor part
//! (P2-305, P2-313, P2-314, P2-338), driven headlessly like `state`'s own
//! tests: effects run synchronously with `files` and `worker::serve`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::model::{Model, ViewKind};
use c4rizer_core::scene::Scene;

use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState};
use crate::state::{Button, Command, Effect, Event, Input, Modifiers, Overlay, State, initial_view};
use crate::testutil::TempDir;
use crate::worker::{self, ViewRef};

/// A shop estate, roles only: a landscape view first, a context view, a
/// container view with a filtered view on it, and a second landscape.
const ESTATE: &str = r#"workspace "Estate" {
    model {
        operator = person "Operator" "Runs the shop." "Staff"
        auditor = person "Auditor" "Checks the books."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust" "Front"
            db = container "Db" "Stores orders." "SQL"
        }
        ledger = softwareSystem "Ledger" "Keeps the books."
        operator -> web "Uses" "HTTPS"
        auditor -> ledger "Reads" "HTTPS"
        web -> db "Reads" "SQL"
        shop -> ledger "Posts sales to" "HTTPS"
    }
    views {
        systemContext shop "context" "The shop" {
            include *
        }
        systemLandscape "estate" "The estate" {
            include *
        }
        container shop "containers" "Inside the shop" {
            include *
            at operator 200 100
            at web 200 400
            at db 600 400
        }
        filtered "containers" include "Front,Person,Relationship" "front" "The front"
        systemLandscape "estate2" "The estate again" {
            include *
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
    fn with(text: &str) -> Shell {
        let dir = TempDir::new("landscape-filtered");
        let path = dir.join("estate.dsl");
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

    fn show(&mut self, key: &str) {
        self.event(Event::Command(Command::ShowView(declared(key))));
        assert_eq!(self.scene().key, key);
    }

    fn screen(&self, key: &str) -> ScreenPoint {
        let id = self.model().find_written(key).expect("element");
        let b = self.scene().boxes_of(id)[0];
        self.s.camera.to_screen(self.scene().boxes[b.0 as usize].rect.centre())
    }

    fn click(&mut self, key: &str) {
        let at = self.screen(key);
        let mods = Modifiers::default();
        self.event(Event::Input(Input::PointerDown {
            at,
            button: Button::Left,
            mods,
        }));
        self.event(Event::Input(Input::PointerUp { at, button: Button::Left }));
    }

    /// Enters `key`'s box: a right click opens the context menu, its
    /// first entry is picked (editor-ux-02).
    fn enter(&mut self, key: &str) {
        let at = self.screen(key);
        self.event(Event::Input(Input::PointerDown {
            at,
            button: Button::Right,
            mods: Modifiers::default(),
        }));
        self.event(Event::Input(Input::PointerUp { at, button: Button::Right }));
        let Some(Overlay::ContextMenu { entries }) = self.s.overlay.clone() else {
            panic!("no context menu on {key}");
        };
        self.event(Event::Command(Command::ContextAction(entries[0].action.clone())));
    }

    fn drag(&mut self, key: &str, dx: f32, dy: f32) {
        let from = self.screen(key);
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

/// The `at` line of `element` in view `view` of `text`, as written.
fn at_line(text: &str, view: &str, element: &str) -> Option<(i32, i32)> {
    let parsed = c4rizer_core::model::parse(text).expect("parses");
    let m = &parsed.model;
    let v = m.view(view)?;
    let id = m.find_written(element)?;
    v.position_of(id).map(|a| (a.x.value, a.y.value))
}

/// In a landscape view the context menu of an enterable box opens its view as
/// EDIT-15 says, and the breadcrumb is the workspace crumb alone.
// TODO(srs): P2-305, the workspace crumb. Reading taken (B
// landscape-filtered 8): it opens the first `systemLandscape` view when
// the file has one, else the first system context view (EDIT-16).
#[test]
fn p2_305_landscape_click_enters_breadcrumb_is_workspace() {
    let mut sh = Shell::with(ESTATE);
    sh.show("estate");
    assert_eq!(sh.scene().kind, ViewKind::SystemLandscape);
    let crumbs = sh.s.breadcrumb();
    assert_eq!(crumbs.len(), 1, "{crumbs:?}");
    assert_eq!(crumbs[0].scope, None);
    // The context menu on the shop (it has containers) opens its container
    // view (EDIT-15 as amended).
    sh.enter("shop");
    assert_eq!(sh.s.view, Some(declared("containers")));
    // A click on a person selects it and stays.
    sh.show("estate");
    sh.click("operator");
    assert_eq!(sh.s.view, Some(declared("estate")));
    // The workspace crumb opens the first landscape view, not the context
    // view declared before it.
    sh.show("context");
    sh.event(Event::Command(Command::Crumb(0)));
    assert_eq!(sh.s.view, Some(declared("estate")));
    // With no landscape view, the first context view as before.
    let mut plain = Shell::with(&ESTATE.replace("systemLandscape", "systemContext ledger"));
    plain.show("containers");
    plain.event(Event::Command(Command::Crumb(0)));
    assert_eq!(plain.s.view, Some(declared("context")));
}

/// A move or resize in a filtered view.
// TODO(srs): P2-313. Reading taken (B landscape-filtered 1): it writes
// the base view's `at`/`size` line, in one undo step (shared layout).
#[test]
fn p2_313_filtered_move_writes_base_view_line() {
    let mut sh = Shell::with(ESTATE);
    sh.show("front");
    assert_eq!(sh.scene().kind, ViewKind::Filtered);
    // Boxes sit where the base view's `at` lines put them.
    let web = sh.model().find_written("web").expect("web");
    let b = sh.scene().boxes_of(web)[0];
    assert_eq!(sh.scene().centres[b.0 as usize].x * 10, 200);
    sh.drag("web", 100.0, 0.0);
    let text = sh.text();
    let moved = at_line(&text, "containers", "web").expect("base line");
    assert_ne!(moved, (200, 400), "the base view's line was rewritten");
    assert!(!text.contains("filtered \"containers\" include \"Front,Person,Relationship\" \"front\" \"The front\" {"));
    // One undo step puts it back.
    sh.event(Event::Command(Command::Undo));
    assert_eq!(at_line(&sh.text(), "containers", "web"), Some((200, 400)));
    // The base view shows the move too.
    sh.event(Event::Command(Command::Redo));
    sh.show("containers");
    let b = sh.scene().boxes_of(web)[0];
    assert_eq!(sh.scene().centres[b.0 as usize].x * 10, moved.0);
}

/// A view that is the base of a filtered view is missing from the view
/// list (`Model::filtered_bases`).
// TODO(srs): P2-314. Reading taken (B landscape-filtered 5): `c4rizer
// render` still writes it; EDIT-15/16 may open it, and it then shows in
// the list until another view is opened.
#[test]
fn p2_314_base_view_hidden_from_view_list() {
    let mut sh = Shell::with(ESTATE);
    assert_eq!(sh.model().filtered_bases(), ["containers"]);
    let listed: Vec<String> = sh.s.views().iter().map(|v| v.view.key()).collect();
    assert_eq!(listed, ["context", "estate", "front", "estate2"]);
    // EDIT-15 may open it: the context menu on the shop in the context view.
    sh.show("context");
    sh.enter("shop");
    assert_eq!(sh.s.view, Some(declared("containers")));
    let listed: Vec<String> = sh.s.views().iter().map(|v| v.view.key()).collect();
    assert!(listed.contains(&"containers".to_owned()), "{listed:?}");
    // Gone again once another view is opened.
    sh.show("front");
    let listed: Vec<String> = sh.s.views().iter().map(|v| v.view.key()).collect();
    assert!(!listed.contains(&"containers".to_owned()), "{listed:?}");
}

/// With no last-viewed view, the editor opens the view carrying
/// `default` (`Model::default_view_key`) instead of the first declared.
// TODO(srs): P2-338, several `default` views. Reading taken (B
// landscape-filtered 16): the last one declared wins.
#[test]
fn p2_338_editor_opens_default_view() {
    let one = ESTATE.replace(
        "systemLandscape \"estate\" \"The estate\" {\n            include *\n",
        "systemLandscape \"estate\" \"The estate\" {\n            include *\n            default\n",
    );
    let sh = Shell::with(&one);
    assert_eq!(sh.s.view, Some(declared("estate")));
    let m = c4rizer_core::model::parse(&one).expect("parses").model;
    assert_eq!(m.default_view_key(), Some("estate"));
    assert_eq!(initial_view(&m, None), Some(declared("estate")));
    // A remembered view still wins (EDIT-13).
    assert_eq!(initial_view(&m, Some(&declared("context"))), Some(declared("context")));
    // Two `default` views: the last declared wins.
    let two = one.replace(
        "systemLandscape \"estate2\" \"The estate again\" {\n            include *\n",
        "systemLandscape \"estate2\" \"The estate again\" {\n            include *\n            default\n",
    );
    let m = c4rizer_core::model::parse(&two).expect("parses").model;
    assert_eq!(m.default_view_key(), Some("estate2"));
    assert_eq!(initial_view(&m, None), Some(declared("estate2")));
    // None written: the first declared view, as before.
    let m = c4rizer_core::model::parse(ESTATE).expect("parses").model;
    assert_eq!(m.default_view_key(), None);
    assert_eq!(initial_view(&m, None), Some(declared("context")));
}

/// A 4x3 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0xd, 0xa, 0x1a, 0xa, 0x0, 0x0, 0x0, 0xd, 0x49, 0x48, 0x44, 0x52, 0x0, 0x0, 0x0, 0x4,
    0x0, 0x0, 0x0, 0x3, 0x8, 0x2, 0x0, 0x0, 0x0, 0x3b, 0x96, 0x39, 0x91, 0x0, 0x0, 0x0, 0x10, 0x49, 0x44, 0x41,
    0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x0, 0x47, 0xc, 0x38, 0x39, 0x0, 0xf5, 0x31, 0xb, 0xf5, 0x35, 0x7b,
    0xfb, 0x82, 0x0, 0x0, 0x0, 0x0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// `ESTATE` plus an image view of the ledger (a local PNG) and one with
/// a `plantuml` source.
fn with_images() -> String {
    ESTATE.replace(
        "        systemLandscape \"estate2\"",
        "        image ledger \"books\" {\n            image books.png\n            title \"The books\"\n        }\n        image * \"uml\" {\n            plantuml flow.puml\n        }\n        systemLandscape \"estate2\"",
    )
}

/// P2-320: an image view on the canvas paints its file, the same bytes
/// the SVG export embeds; a placeholder source paints a dashed frame and
/// its name (P2-321).
#[test]
fn p2_320_image_view_on_canvas() {
    use crate::canvas::{Primitive, paint_items};
    let dir = TempDir::new("landscape-filtered-image");
    std::fs::write(dir.join("books.png"), PNG).expect("png written");
    let path = dir.join("estate.dsl");
    std::fs::write(&path, with_images()).expect("fixture written");
    let mut sh = Shell {
        s: State::new(UiState::default()),
        path: path.clone(),
        _dir: dir,
    };
    sh.event(Event::Command(Command::Open(path)));
    sh.show("books");
    let items = paint_items(&sh.s.draw_list());
    let images: Vec<_> = items
        .iter()
        .filter_map(|i| match &i.primitive {
            Primitive::Image { asset, .. } => Some(asset.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].bytes.as_ref(), PNG);
    assert!(crate::canvas::icon_image(&images[0], 40.0, 30.0).is_some());
    sh.show("uml");
    let items = paint_items(&sh.s.draw_list());
    assert!(items.iter().any(|i| matches!(&i.primitive, Primitive::Text { text, .. } if text == "plantuml flow.puml (not drawn)")));
    assert!(items.iter().any(|i| matches!(&i.primitive, Primitive::Polygon { dash: Some(_), .. })));
}

/// P2-322 (f): an element-scoped image view makes its element enterable;
/// a click on it in the landscape opens the image view.
// TODO(srs): P2-322 (f). Reading taken (B landscape-filtered 10f): when no
// container or component view of the element is declared.
#[test]
fn p2_322_image_view_makes_element_enterable() {
    let dir = TempDir::new("landscape-filtered-enter");
    std::fs::write(dir.join("books.png"), PNG).expect("png written");
    let path = dir.join("estate.dsl");
    std::fs::write(&path, with_images()).expect("fixture written");
    let mut sh = Shell {
        s: State::new(UiState::default()),
        path: path.clone(),
        _dir: dir,
    };
    sh.event(Event::Command(Command::Open(path)));
    sh.show("estate");
    let ledger = sh.model().find_written("ledger").expect("ledger");
    assert!(crate::state::enterable(sh.model(), ledger));
    sh.enter("ledger");
    assert_eq!(sh.s.view, Some(declared("books")));
}
