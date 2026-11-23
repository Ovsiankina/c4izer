//! Tests of the exporters row, editor part (SRS 4.7 P3-420, P3-431),
//! driven headlessly like `state`'s own tests: effects run synchronously
//! with `files` and `worker::serve`.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use c4rizer_core::edit::{StyleProperty, StyleTarget};
use c4rizer_core::model::{self, Colour, Model};
use c4rizer_core::scene::Scene;

use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState, WriteRequest};
use crate::state::{Button, Command, Dialog, Effect, Event, Input, Key, Modifiers, State, handle_spots, line_ends};
use crate::testutil::{SHOP, TempDir};
use crate::worker::{self, ExportFormat, ViewRef};

/// The shell without a window: every effect at once, writes recorded.
struct Shell {
    s: State,
    writes: Vec<WriteRequest>,
    dialogs: Vec<Dialog>,
}

impl Shell {
    fn open(path: &Path) -> Shell {
        let mut shell = Shell {
            s: State::new(UiState::default()),
            writes: Vec::new(),
            dialogs: Vec::new(),
        };
        shell.cmd(Command::Open(path.to_path_buf()));
        shell
    }

    fn event(&mut self, event: Event) {
        let effects = self.s.handle(event);
        self.run(effects);
    }

    fn cmd(&mut self, command: Command) {
        self.event(Event::Command(command));
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
                Effect::Dialog(dialog) => {
                    self.dialogs.push(dialog);
                    Vec::new()
                }
                Effect::Watch(_)
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
        let id = self.model().find_written(key).expect("element");
        let b = self.scene().boxes_of(id)[0];
        self.s.camera.to_screen(self.scene().boxes[b.0 as usize].rect.centre())
    }

    fn press(&mut self, at: ScreenPoint) {
        let mods = Modifiers::default();
        self.event(Event::Input(Input::PointerDown {
            at,
            button: Button::Left,
            mods,
        }));
    }

    fn drag(&mut self, from: ScreenPoint, dx: f32, dy: f32) {
        self.press(from);
        let to = ScreenPoint {
            x: from.x + dx,
            y: from.y + dy,
        };
        self.event(Event::Input(Input::PointerMove { at: to }));
        self.event(Event::Input(Input::PointerUp { at: to, button: Button::Left }));
    }

    fn click(&mut self, at: ScreenPoint) {
        self.press(at);
        self.event(Event::Input(Input::PointerUp { at, button: Button::Left }));
    }

    fn notice(&self) -> String {
        self.s.status().notice.unwrap_or_default()
    }
}

/// `SHOP` exported as Structurizr workspace JSON (P3-413).
fn shop_json() -> String {
    let parsed = model::parse(SHOP).expect("fixture parses");
    let all = c4rizer_core::pipeline::build_all(&parsed.model, &AtomicBool::new(false)).expect("builds");
    c4rizer_core::export::json(&parsed.model, &all, Some(SHOP))
}

#[test]
fn p3_420_json_workspace_is_read_only() {
    let dir = TempDir::new("p3-420");
    let path = dir.join("shop.json");
    let json = shop_json();
    std::fs::write(&path, &json).expect("written");
    let mut h = Shell::open(&path);
    // Views draw: the JSON's views, with the DSL's boxes.
    assert!(h.s.status().problem.is_none(), "{:?}", h.s.status().problem);
    let keys: Vec<String> = h.s.views().iter().map(|v| v.view.key()).collect();
    assert_eq!(keys, ["context", "containers"]);
    assert!(h.scene().boxes.len() >= 3);
    assert!(h.s.read_only());
    assert!(h.s.json_doc());
    // A move is refused with one message saying the file is JSON.
    let at = h.box_centre("customer");
    h.drag(at, 200.0, 120.0);
    assert!(h.notice().contains("workspace JSON"), "{}", h.notice());
    // A style edit too, and undo.
    h.click(h.box_centre("shop"));
    h.s.notice = None;
    h.cmd(Command::SetStyle {
        target: StyleTarget::ThisBox,
        property: StyleProperty::Background(Some(Colour { r: 0x11, g: 0x22, b: 0x33 })),
    });
    assert!(h.notice().contains("workspace JSON"), "{}", h.notice());
    h.s.notice = None;
    h.cmd(Command::Undo);
    assert!(h.notice().contains("workspace JSON"), "{}", h.notice());
    // A resize: a press on the selected box's handle says why, and starts
    // no rubber band.
    h.click(h.box_centre("shop"));
    h.s.notice = None;
    let shop = h.model().find_written("shop").expect("shop");
    let rect = h.scene().boxes[h.scene().boxes_of(shop)[0].0 as usize].rect;
    let ends = line_ends(h.scene(), &h.s.camera);
    // Just outside the box, still on its NW handle.
    let nw = handle_spots(rect, &h.s.camera, &ends)[0].1;
    let corner = ScreenPoint { x: nw.x - 3.0, y: nw.y - 3.0 };
    h.press(corner);
    h.event(Event::Input(Input::PointerMove { at: ScreenPoint { x: corner.x + 60.0, y: corner.y + 60.0 } }));
    assert!(h.notice().contains("workspace JSON"), "{}", h.notice());
    assert!(h.s.draw_list().rubber_band.is_none(), "no rubber band from a handle press");
    h.event(Event::Input(Input::PointerUp { at: ScreenPoint { x: corner.x + 60.0, y: corner.y + 60.0 }, button: Button::Left }));
    assert_eq!(h.scene().boxes[h.scene().boxes_of(shop)[0].0 as usize].rect, rect);
    // A text edit (F2 on the selected box).
    h.click(h.box_centre("shop"));
    h.s.notice = None;
    h.event(Event::Input(Input::KeyDown { key: Key::F2, mods: Modifiers::default(), text_focus: false }));
    assert!(h.notice().contains("workspace JSON"), "{}", h.notice());
    assert!(h.s.text_edit.is_none());
    assert!(h.writes.is_empty(), "the editor never writes JSON");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), json);
    // R1: exporting `json` into the JSON's own folder would rewrite the
    // open file: refused, the file unchanged.
    h.cmd(Command::Export {
        targets: vec![(ViewRef::Declared("context".into()), dir.join("context.json"))],
        format: ExportFormat::Json,
    });
    assert!(h.notice().contains("export refused") && h.notice().contains("shop.json"), "{}", h.notice());
    assert_eq!(std::fs::read_to_string(&path).expect("read"), json);
    // Views export.
    let out = dir.join("context.svg");
    h.cmd(Command::Export {
        targets: vec![(ViewRef::Declared("context".into()), out.clone())],
        format: ExportFormat::Svg,
    });
    assert_eq!(std::fs::read_to_string(&out).expect("exported"), c4rizer_core::export::svg(h.scene()));
}

/// A workspace JSON whose file name holds `"` and `\\` opens as itself,
/// not as another file.
#[test]
fn p3_420_json_name_with_quote_and_backslash() {
    let dir = TempDir::new("p3-420-name");
    let path = dir.join("a\"b\\c.json");
    std::fs::write(&path, shop_json()).expect("written");
    // A file the stripped name would point at.
    std::fs::write(dir.join("abc.json"), "{ \"views\": [ }").expect("written");
    let h = Shell::open(&path);
    assert!(h.s.status().problem.is_none(), "{:?}", h.s.status().problem);
    assert_eq!(h.s.views().len(), 2);
}

#[test]
fn p3_420_json_base_is_never_written() {
    // P3-419 with P3-420: a view of a `workspace extends <file>.json` base
    // lives in DSL text written from the JSON; a move there is refused,
    // and the JSON is never written.
    let dir = TempDir::new("p3-420-base");
    let base = dir.join("base.json");
    let json = shop_json();
    std::fs::write(&base, &json).expect("written");
    let path = dir.join("extended.dsl");
    let dsl = "workspace extends base.json {\n}\n";
    std::fs::write(&path, dsl).expect("written");
    let mut h = Shell::open(&path);
    assert!(h.s.status().problem.is_none(), "{:?}", h.s.status().problem);
    assert!(!h.s.read_only());
    let at = h.box_centre("customer");
    h.drag(at, 200.0, 120.0);
    assert!(h.notice().contains("workspace JSON"), "{}", h.notice());
    assert!(h.writes.is_empty(), "{:?}", h.writes.iter().map(|w| &w.path).collect::<Vec<_>>());
    assert_eq!(std::fs::read_to_string(&base).expect("read"), json);
    assert_eq!(std::fs::read_to_string(&path).expect("read"), dsl);
}

/// The `export -f` name of an editor format.
fn cli_name(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::PlantUml => "plantuml/structurizr",
        ExportFormat::C4PlantUml => "plantuml/c4plantuml",
        ExportFormat::Mermaid => "mermaid",
        ExportFormat::WebSequenceDiagrams => "websequencediagrams",
        ExportFormat::Dot => "dot",
        ExportFormat::Ilograph => "ilograph",
        ExportFormat::Json => "json",
        ExportFormat::Theme => "theme",
        ExportFormat::Static => "static",
        _ => unreachable!("image formats are not compared here"),
    }
}

/// What `c4rizer export -f <format>` writes for the file at `path`, as
/// `(name, bytes)`, sorted: read as the CLI reads it (`parse_model`:
/// JSON when it starts with `{`) and written by the CLI's own
/// `export_files::files`.
fn cli_files(path: &Path, format: ExportFormat) -> Vec<(String, Vec<u8>)> {
    let source = std::fs::read_to_string(path).expect("read");
    let json = source.trim_start().starts_with('{');
    let model = if json {
        model::parse_json(source.as_bytes(), &mut model::FsLoader).expect("parses")
    } else {
        model::parse_with(&source, Some(path), &mut model::FsLoader).expect("parses").model
    };
    let all = c4rizer_core::pipeline::build_all(&model, &AtomicBool::new(false)).expect("builds");
    let from = crate::export_files::Source {
        model: &model,
        all: &all,
        path: Some(path),
        dsl: (!json).then_some(source.as_str()),
    };
    let mut files: Vec<(String, Vec<u8>)> = crate::export_files::files(cli_name(format), &from, false)
        .expect("a CLI format")
        .into_iter()
        .map(|(p, b)| (p.to_string_lossy().into_owned(), b))
        .collect();
    files.sort();
    files
}

/// Every file under `dir`, `(relative name, bytes)`, sorted.
fn written(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read dir") {
            let p = entry.expect("entry").path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let name = p.strip_prefix(dir).expect("inside").to_string_lossy().into_owned();
                out.push((name, std::fs::read(&p).expect("read")));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn p3_431_export_dialog_offers_every_format() {
    // The panel offers every format of `c4rizer export -f` (P3-400),
    // animation aside (P3-432, deferred).
    let names: Vec<&str> = crate::app::export_choices().iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        ["svg", "png", "plantuml", "plantuml/c4plantuml", "mermaid", "websequencediagrams", "dot", "ilograph", "json", "theme", "static"]
    );
    let dir = TempDir::new("p3-431");
    let path = dir.join("dynamic.dsl");
    std::fs::write(&path, include_str!("../../../tests/data/phase3/dynamic.dsl")).expect("written");
    let mut h = Shell::open(&path);
    assert!(h.s.status().problem.is_none(), "{:?}", h.s.status().problem);
    // "Export all views" writes into the chosen folder what the CLI writes.
    for (name, format) in crate::app::export_choices() {
        if matches!(format, ExportFormat::Svg | ExportFormat::Png) {
            continue;
        }
        h.cmd(Command::ExportAll(format));
        assert!(matches!(h.dialogs.last(), Some(Dialog::ExportAll { format: f, .. }) if *f == format), "{name}");
        let folder = dir.join(&format!("out-{}", name.replace('/', "-")));
        std::fs::create_dir_all(&folder).expect("folder");
        let targets = crate::app::export_all_targets(&h.s, &folder, format);
        h.cmd(Command::Export { targets, format });
        assert!(h.notice().starts_with("exported"), "{name}: {}", h.notice());
        let got = written(&folder);
        let want = cli_files(&path, format);
        assert_eq!(got.iter().map(|(n, _)| n).collect::<Vec<_>>(), want.iter().map(|(n, _)| n).collect::<Vec<_>>(), "{name}");
        assert!(got == want, "{name}: bytes differ from the CLI's");
    }
    // "Export view" writes the open view's file under Structurizr's name.
    let key = h.s.view.as_ref().expect("a view").key();
    h.cmd(Command::ExportView(ExportFormat::Mermaid));
    let Some(Dialog::ExportView { view, name, .. }) = h.dialogs.last().cloned() else { panic!("{:?}", h.dialogs.last()) };
    assert_eq!(name, format!("structurizr-{key}.mmd"));
    let one = dir.join("one");
    std::fs::create_dir_all(&one).expect("folder");
    h.cmd(Command::Export { targets: vec![(view, one.join(&name))], format: ExportFormat::Mermaid });
    let want: Vec<_> = cli_files(&path, ExportFormat::Mermaid).into_iter().filter(|(n, _)| *n == name).collect();
    assert_eq!(written(&one), want);
    // A PlantUML view comes with its key file.
    h.cmd(Command::ExportView(ExportFormat::PlantUml));
    let Some(Dialog::ExportView { view, name, .. }) = h.dialogs.last().cloned() else { panic!() };
    let puml = dir.join("puml");
    std::fs::create_dir_all(&puml).expect("folder");
    h.cmd(Command::Export { targets: vec![(view, puml.join(&name))], format: ExportFormat::PlantUml });
    let want: Vec<_> = cli_files(&path, ExportFormat::PlantUml)
        .into_iter()
        .filter(|(n, _)| *n == name || *n == name.replace(".puml", "-key.puml"))
        .collect();
    assert_eq!(want.len(), 2);
    assert_eq!(written(&puml), want);
    // A whole-workspace format asks for a folder even from "Export view".
    h.cmd(Command::ExportView(ExportFormat::Json));
    assert!(matches!(h.dialogs.last(), Some(Dialog::ExportAll { format: ExportFormat::Json, .. })));
    // From a workspace JSON document: the files `c4rizer export` writes
    // from that JSON.
    let json_dir = dir.join("json-doc");
    std::fs::create_dir_all(&json_dir).expect("folder");
    let json_path = json_dir.join("dynamic.json");
    let exported = cli_files(&path, ExportFormat::Json);
    std::fs::write(&json_path, &exported[0].1).expect("written");
    let mut j = Shell::open(&json_path);
    assert!(j.s.json_doc(), "{:?}", j.s.status().problem);
    for format in [ExportFormat::Json, ExportFormat::Mermaid, ExportFormat::Static] {
        let folder = dir.join(&format!("json-out-{}", cli_name(format).replace('/', "-")));
        std::fs::create_dir_all(&folder).expect("folder");
        let targets = crate::app::export_all_targets(&j.s, &folder, format);
        j.cmd(Command::Export { targets, format });
        assert!(j.notice().starts_with("exported"), "{format:?}: {}", j.notice());
        let got = written(&folder);
        let want = cli_files(&json_path, format);
        assert_eq!(got.iter().map(|(n, _)| n).collect::<Vec<_>>(), want.iter().map(|(n, _)| n).collect::<Vec<_>>(), "{format:?}");
        assert!(got == want, "{format:?}: bytes differ from the CLI's");
    }
}

/// CLI-1's guard in the editor's export (P3-431): view keys equal up to
/// case would overwrite each other's files, so nothing is written and the
/// status bar says why; JSON (one file) is not concerned.
#[test]
fn p3_431_export_refuses_keys_equal_up_to_case() {
    let dir = TempDir::new("p3-431-case");
    let path = dir.join("case.dsl");
    std::fs::write(
        &path,
        "workspace {\n    model {\n        s = softwareSystem \"S\"\n    }\n    views {\n        systemContext s \"Ab\" {\n            include *\n        }\n        systemContext s \"ab\" {\n            include *\n        }\n    }\n}\n",
    )
    .expect("written");
    let mut h = Shell::open(&path);
    for format in [ExportFormat::Mermaid, ExportFormat::Static] {
        let folder = dir.join(&format!("out-{}", cli_name(format).replace('/', "-")));
        std::fs::create_dir_all(&folder).expect("folder");
        let targets = crate::app::export_all_targets(&h.s, &folder, format);
        h.cmd(Command::Export { targets, format });
        assert!(h.notice().contains("\"Ab\" and \"ab\" differ only in case"), "{format:?}: {}", h.notice());
        assert!(written(&folder).is_empty(), "{format:?}: nothing written");
    }
    let folder = dir.join("out-json");
    std::fs::create_dir_all(&folder).expect("folder");
    let targets = crate::app::export_all_targets(&h.s, &folder, ExportFormat::Json);
    h.cmd(Command::Export { targets, format: ExportFormat::Json });
    assert!(h.notice().starts_with("exported"), "{}", h.notice());
}
