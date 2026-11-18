//! Step-4 contract tests of the dsl-breadth row, editor part (P2-123,
//! P2-126, P2-136, P2-141, P2-180), driven headlessly like `state`'s own
//! tests: effects run synchronously with `files` and `worker::serve`.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use crate::files::{self, FileEvent, UiState};
use crate::state::{Command, Effect, Event, Input, Key, Modifiers, Problem, Selected, State, initial_view};
use crate::testutil::TempDir;
use crate::worker::{self, ExportFormat, ViewRef};

/// A dynamic view first, then a clean context view, then a deployment
/// view. Every kind is drawn since dynamic-view (P3-200) and
/// deployment-view (P3-2), so no view of it is unsupported any more.
const UNSUPPORTED: &str = r#"workspace "Shop" {
    model {
        customer = person "Customer" "Buys things."
        shop = softwareSystem "Shop" "Sells things." {
            web = container "Web" "Serves pages." "Rust"
        }
        customer -> shop "Buys from"
        customer -> web "Browses"
        live = deploymentEnvironment "Live" {
            node = deploymentNode "Node" {
                containerInstance web
            }
        }
    }
    views {
        dynamic shop "flow" "Checkout flow" {
            customer -> web "Opens the shop"
        }
        systemContext shop "ctx" "Shop context" {
            include *
        }
        deployment * live "live" {
            include *
        }
    }
}
"#;

/// A root file whose people come from `people.dsl`.
const ROOT: &str = r#"workspace "Shop" {
    model {
        !include people.dsl
        shop = softwareSystem "Shop" "Sells things."
        customer -> shop "Buys from"
    }
    views {
        systemContext shop "ctx" "Shop context" {
            include *
        }
    }
}
"#;

const PEOPLE: &str = "customer = person \"Customer\" \"Buys things.\"\n";

/// The shell without a window: every effect at once, write paths kept.
struct Shell {
    s: State,
    writes: Vec<PathBuf>,
}

impl Shell {
    fn open(path: &Path) -> Shell {
        let mut shell = Shell {
            s: State::new(UiState::default()),
            writes: Vec::new(),
        };
        shell.event(Event::Command(Command::Open(path.to_path_buf())));
        shell
    }

    fn event(&mut self, event: Event) -> usize {
        let effects = self.s.handle(event);
        self.run(effects)
    }

    /// Runs `effects`; the number of effects run, nested ones included.
    fn run(&mut self, effects: Vec<Effect>) -> usize {
        let mut count = 0;
        for effect in effects {
            count += 1;
            let next = match effect {
                Effect::Read(path) => {
                    let (text, hash) = files::read(&path).expect("read fixture");
                    self.s.handle(Event::File(FileEvent::Loaded { path, text, hash }))
                }
                Effect::Write(request) => {
                    self.writes.push(request.path.clone());
                    self.s.handle(Event::File(files::write_checked(&request)))
                }
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
            count += self.run(next);
        }
        count
    }

    /// Feeds an included file's watcher event; the number of effects run.
    fn included(&mut self, path: &Path, event: &FileEvent) -> usize {
        let effects = self.s.included_file(path, event);
        self.run(effects)
    }

    fn key(&mut self, key: Key) {
        self.event(Event::Input(Input::KeyDown {
            key,
            mods: Modifiers::default(),
            text_focus: false,
        }));
    }

    fn name_of(&self, key: &str) -> String {
        let model = &self.s.doc.as_ref().unwrap().parsed.as_ref().unwrap().model;
        model.element(model.find(key).unwrap()).name.value.clone()
    }
}

fn declared(key: &str) -> ViewRef {
    ViewRef::Declared(key.to_owned())
}

/// The next watcher event for an included file, within a few seconds.
fn next(rx: &mpsc::Receiver<(PathBuf, FileEvent)>) -> (PathBuf, FileEvent) {
    rx.recv_timeout(Duration::from_secs(5)).expect("watcher event")
}

/// A root including `people.dsl`, in a fresh directory.
fn with_include(tag: &str) -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new(tag);
    let root = dir.join("shop.dsl");
    std::fs::write(&root, ROOT).unwrap();
    std::fs::write(dir.join("people.dsl"), PEOPLE).unwrap();
    let people = std::fs::canonicalize(dir.join("people.dsl")).unwrap();
    (dir, root, people)
}

/// The view list marks no view "not yet supported" now that every kind
/// is drawn; selecting the dynamic view draws it (formerly P2-123's
/// unsupported case).
#[test]
fn p2_123_view_list_marks_unsupported() {
    let dir = TempDir::new("p2-123-list");
    let path = dir.join("shop.dsl");
    std::fs::write(&path, UNSUPPORTED).unwrap();
    let mut shell = Shell::open(&path);
    let views = shell.s.views();
    let labels: Vec<String> = views.iter().map(crate::app::view_label).collect();
    assert_eq!(views.len(), 3);
    // dynamic-view (P3-200) and deployment-view (P3-2) draw both.
    assert!(labels.iter().all(|l| !l.contains("not yet supported")), "{labels:?}");
    assert!(labels[0].starts_with("Checkout flow"), "{labels:?}");
    shell.event(Event::Command(Command::ShowView(declared("flow"))));
    assert_eq!(shell.s.view, Some(declared("flow")));
    assert!(shell.s.scene().is_some());
    shell.event(Event::Command(Command::ShowView(declared("live"))));
    assert!(shell.s.scene().is_some());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), UNSUPPORTED);
    assert!(shell.writes.is_empty(), "{:?}", shell.writes);
}

/// At startup and in the EDIT-27 fallback the editor opens the first
/// declared view; every kind is drawn now, so that is the dynamic view.
#[test]
fn p2_123_never_opens_unsupported_view_by_itself() {
    let dir = TempDir::new("p2-123-open");
    let path = dir.join("shop.dsl");
    std::fs::write(&path, UNSUPPORTED).unwrap();
    let mut shell = Shell::open(&path);
    assert_eq!(shell.s.view, Some(declared("flow")), "startup opens the first view");
    assert!(shell.s.scene().is_some());
    let model = shell.s.doc.as_ref().unwrap().parsed.as_ref().unwrap().model.clone();
    assert_eq!(initial_view(&model, None), Some(declared("flow")));
    // A remembered view is reopened, whatever its kind.
    assert_eq!(initial_view(&model, Some(&declared("live"))), Some(declared("live")));
    // EDIT-27: the open view disappears; the first remaining view opens.
    let without_flow = UNSUPPORTED.replace(
        "        dynamic shop \"flow\" \"Checkout flow\" {\n            customer -> web \"Opens the shop\"\n        }\n",
        "",
    );
    assert_ne!(without_flow, UNSUPPORTED);
    std::fs::write(&path, &without_flow).unwrap();
    shell.event(Event::File(FileEvent::Changed {
        hash: files::hash(without_flow.as_bytes()),
        text: without_flow.clone(),
    }));
    assert_eq!(shell.s.view, Some(declared("ctx")));
    assert!(shell.s.scene().is_some());
}

/// The findings panel (EDIT-6) shows every notice of the open file,
/// naming the construct.
#[test]
fn p2_126_notices_in_findings_panel() {
    let dir = TempDir::new("p2-126");
    let path = dir.join("shop.dsl");
    let text = UNSUPPORTED.replace(
        "        customer -> shop \"Buys from\"\n",
        "        customer -> shop \"Buys from\"\n        !plugin com.example.Plugin\n",
    );
    std::fs::write(&path, &text).unwrap();
    let shell = Shell::open(&path);
    let notices = shell.s.notices();
    assert_eq!(
        notices,
        [format!("{}:8:9: !plugin is accepted and has no effect", path.display())]
    );
}

/// A change to any included file reloads as EDIT-1 does; a save by
/// rename is seen as EDIT-23 says.
#[test]
fn p2_141_included_file_change_reloads() {
    let (dir, root, people) = with_include("p2-141-reload");
    let mut shell = Shell::open(&root);
    assert_eq!(shell.s.included_files(), std::slice::from_ref(&people));
    assert_eq!(shell.name_of("customer"), "Customer");
    let (tx, rx) = mpsc::channel();
    let _watcher = files::watch_files(&shell.s.included_files(), tx, Arc::new(|| {})).expect("watch");
    // In place.
    let edited = PEOPLE.replace("\"Customer\"", "\"Buyer\"");
    std::fs::write(&people, &edited).unwrap();
    let (path, event) = next(&rx);
    assert_eq!(path, people);
    assert!(shell.included(&path, &event) > 0, "reloads");
    assert_eq!(shell.name_of("customer"), "Buyer");
    // By rename (EDIT-23).
    let renamed = PEOPLE.replace("\"Customer\"", "\"Shopper\"");
    let tmp = dir.join(".people.dsl.swp");
    std::fs::write(&tmp, &renamed).unwrap();
    std::fs::rename(&tmp, &people).unwrap();
    let (path, event) = next(&rx);
    assert_eq!(path, people);
    shell.included(&path, &event);
    assert_eq!(shell.name_of("customer"), "Shopper");
    // The root file is untouched by any of it.
    assert_eq!(std::fs::read_to_string(&root).unwrap(), ROOT);
    assert!(shell.writes.is_empty());
}

/// The app's own write to an included file does not reload (EDIT-22,
/// per-file own-write hash).
#[test]
fn p2_141_own_write_to_included_file_ignored() {
    let (_dir, root, people) = with_include("p2-141-own");
    let mut shell = Shell::open(&root);
    let (tx, rx) = mpsc::channel();
    let _watcher = files::watch_files(&shell.s.included_files(), tx, Arc::new(|| {})).expect("watch");
    // A text edit of an element declared in the included file (P2-139).
    shell.s.selection.insert(Selected::Element("customer".to_owned()));
    shell.key(Key::Enter);
    shell.event(Event::Text("Shopper".to_owned()));
    shell.key(Key::Enter);
    assert_eq!(shell.writes, std::slice::from_ref(&people), "only the included file is written");
    assert_eq!(std::fs::read_to_string(&root).unwrap(), ROOT);
    let on_disk = std::fs::read_to_string(&people).unwrap();
    assert_eq!(on_disk, PEOPLE.replace("\"Customer\"", "\"Shopper\""));
    assert_eq!(shell.name_of("customer"), "Shopper");
    // The watcher's echo of that write is ignored: nothing is rebuilt.
    let (path, event) = next(&rx);
    assert_eq!(path, people);
    assert_eq!(shell.included(&path, &event), 0);
    // Undo writes the included file back, as one step.
    shell.event(Event::Command(Command::Undo));
    assert_eq!(std::fs::read_to_string(&people).unwrap(), PEOPLE);
    assert_eq!(std::fs::read_to_string(&root).unwrap(), ROOT);
    assert_eq!(shell.name_of("customer"), "Customer");
}

/// "Export all views" (EDIT-7) writes every view now that every kind
/// is drawn (dynamic-view P3-200, deployment-view P3-2).
#[test]
fn p2_180_export_all_skips_unsupported() {
    let dir = TempDir::new("p2-180");
    let path = dir.join("shop.dsl");
    std::fs::write(&path, UNSUPPORTED).unwrap();
    let mut shell = Shell::open(&path);
    let folder = dir.join("out");
    std::fs::create_dir_all(&folder).unwrap();
    let targets = crate::app::export_all_targets(&shell.s, &folder, ExportFormat::Svg);
    assert_eq!(
        targets,
        [
            (declared("flow"), folder.join("flow.svg")),
            (declared("ctx"), folder.join("ctx.svg")),
            (declared("live"), folder.join("live.svg")),
        ]
    );
    shell.event(Event::Command(Command::Export {
        targets,
        format: ExportFormat::Svg,
    }));
    let mut written: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(written, ["ctx.svg", "flow.svg", "live.svg"]);
}

/// An error in an included file shows that file's path, line and column
/// (EDIT-11).
#[test]
fn p2_136_editor_error_names_included_file() {
    let (_dir, root, people) = with_include("p2-136");
    std::fs::write(&people, "customer = person \"Customer\"\n  bogus line\n").unwrap();
    let mut shell = Shell::open(&root);
    let status = shell.s.status();
    let Some(Problem::Parse(e)) = &status.problem else {
        panic!("{status:?}");
    };
    assert_eq!((e.position.line, e.position.column), (2, 3));
    assert_eq!(shell.s.parse_error_file(), Some(people.as_path()));
    let text = crate::app::problem_text(&shell.s).expect("a problem");
    assert!(
        text.starts_with(&format!("parse error at {}:2:3: ", people.display())),
        "{text}"
    );
    // The included file is watched even though the parse failed, and a
    // fix reloads.
    assert_eq!(shell.s.included_files(), std::slice::from_ref(&people));
    std::fs::write(&people, PEOPLE).unwrap();
    shell.included(
        &people,
        &FileEvent::Changed {
            text: PEOPLE.to_owned(),
            hash: files::hash(PEOPLE.as_bytes()),
        },
    );
    assert!(shell.s.status().problem.is_none(), "{:?}", shell.s.status());
    assert_eq!(shell.s.parse_error_file(), None);
    // An error in the root file names no included file.
    let bad_root = ROOT.replace("    views {", "    bogus\n    views {");
    std::fs::write(&root, &bad_root).unwrap();
    shell.event(Event::File(FileEvent::Changed {
        hash: files::hash(bad_root.as_bytes()),
        text: bad_root.clone(),
    }));
    assert_eq!(shell.s.parse_error_file(), None);
    let text = crate::app::problem_text(&shell.s).expect("a problem");
    assert!(text.starts_with("parse error at 7:5: "), "{text}");
}
