//! Tests of the bus row, editor part (SRS 4.8 P4-423..P4-427), driven
//! headlessly like `anchors_tests`: effects run synchronously with
//! `files` and `worker::serve`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::{Point, Segment};
use c4rizer_core::scene::{ItemKey, Scene, SceneLine};

use crate::camera::ScreenPoint;
use crate::canvas::{Primitive, Source};
use crate::files::{self, FileEvent, UiState};
use crate::state::{Button, Command, Effect, Event, Input, MarkKind, MenuAction, Modifiers, Overlay, State};
use crate::testutil::TempDir;
use crate::worker;

/// Three containers on top, three below, placed by hand; `{lines}` is
/// replaced by the view's extra lines.
const GRID: &str = r#"workspace "Grid" {
    model {
        s = softwareSystem "S" "Does things." {
            a = container "A" "Top one." "Rust"
            b = container "B" "Top two." "Rust"
            c = container "C" "Top three." "Rust"
            x = container "X" "Bottom one." "Rust"
            y = container "Y" "Bottom two." "Rust"
            z = container "Z" "Bottom three." "Rust"
        }
        a -> x "Feeds X"
        b -> y "Feeds Y"
        c -> z "Feeds Z"
    }
    views {
        container s "g" "Grid" {
            include *
            at a 400 100
            at b 800 100
            at c 1200 100
            at x 400 1100
            at y 800 1100
            at z 1200 1100
{lines}
        }
    }
}
"#;

const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
};
const SHIFT: Modifiers = Modifiers { shift: true, ..NONE };

/// `GRID` with `lines` (each a whole view line).
fn grid(lines: &[&str]) -> String {
    let text: String = lines.iter().fold(String::new(), |mut out, v| {
        out.push_str("            ");
        out.push_str(v);
        out.push('\n');
        out
    });
    GRID.replace("{lines}\n", &text)
}

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    _dir: TempDir,
}

impl Shell {
    fn open(text: &str) -> Shell {
        let dir = TempDir::new("bundles");
        let path = dir.join("grid.dsl");
        std::fs::write(&path, text).expect("fixture written");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
            _dir: dir,
        };
        shell.event(Event::Command(Command::Open(path)));
        assert_eq!(shell.scene().key, "g");
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
                Effect::Watch(_) | Effect::Dialog(_) | Effect::SaveUiState(_) | Effect::Fullscreen(_) | Effect::OpenUrl(_) => Vec::new(),
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

    /// The screen point in the middle of the longest segment of the line
    /// `from -> to`.
    fn on_line(&self, from: &str, to: &str) -> ScreenPoint {
        let l = self.line(from, to);
        let w = l
            .points
            .windows(2)
            .max_by_key(|w| Segment::new(w[0], w[1]).len_steps().unwrap_or(0))
            .expect("a segment");
        self.s.camera.to_screen(Point::new(i32::midpoint(w[0].x, w[1].x), i32::midpoint(w[0].y, w[1].y)))
    }

    fn click(&mut self, at: ScreenPoint, button: Button, mods: Modifiers) {
        self.event(Event::Input(Input::PointerDown { at, button, mods }));
        self.event(Event::Input(Input::PointerUp { at, button }));
    }

    /// A left click on the line `from -> to` with `mods`.
    fn click_line(&mut self, from: &str, to: &str, mods: Modifiers) {
        let at = self.on_line(from, to);
        self.click(at, Button::Left, mods);
    }

    /// The context menu of a right click on `from -> to`.
    fn menu_on(&mut self, from: &str, to: &str) -> Vec<(String, MenuAction)> {
        let at = self.on_line(from, to);
        self.click(at, Button::Right, NONE);
        let Some(Overlay::ContextMenu { entries }) = self.s.overlay.clone() else {
            return Vec::new();
        };
        entries.into_iter().map(|e| (e.label, e.action)).collect()
    }

    /// Picks the menu entry labelled `label`.
    fn pick(&mut self, menu: &[(String, MenuAction)], label: &str) {
        let action = menu.iter().find(|(l, _)| l == label).unwrap_or_else(|| panic!("no {label} in {menu:?}")).1.clone();
        self.event(Event::Command(Command::ContextAction(action)));
    }

    fn undo_steps(&self) -> usize {
        self.s.history.undo.len()
    }
}

/// The labels of `menu`.
fn labels(menu: &[(String, MenuAction)]) -> Vec<&str> {
    menu.iter().map(|(l, _)| l.as_str()).collect()
}

#[test]
fn p4_423_shift_click_toggles_lines() {
    let mut sh = Shell::open(&grid(&[]));
    sh.click_line("A", "X", NONE);
    assert_eq!(sh.s.selected_lines().len(), 1);
    sh.click_line("B", "Y", SHIFT);
    sh.click_line("C", "Z", SHIFT);
    assert_eq!(sh.s.selected_lines().len(), 3, "Shift-click adds");
    sh.click_line("B", "Y", SHIFT);
    assert_eq!(sh.s.selected_lines().len(), 2, "Shift-click on a selected line removes it");
    sh.click_line("B", "Y", NONE);
    assert_eq!(sh.s.selected_lines().len(), 1, "a plain click selects that line alone");
    assert_eq!(sh.undo_steps(), 0, "selecting writes nothing");
}

#[test]
fn p4_424_group_as_bus_from_menu_and_panel() {
    let mut sh = Shell::open(&grid(&[]));
    sh.click_line("A", "X", NONE);
    sh.click_line("B", "Y", SHIFT);
    sh.click_line("C", "Z", SHIFT);
    // The right panel offers it too.
    let panel: Vec<String> = sh.s.bus_entries().into_iter().map(|e| e.label).collect();
    assert_eq!(panel, ["Group as bus"]);
    let menu = sh.menu_on("B", "Y");
    assert_eq!(sh.s.selected_lines().len(), 3, "a right click on a selected line keeps the selection");
    assert_eq!(labels(&menu), ["Group as bus"]);
    sh.pick(&menu, "Group as bus");
    let text = sh.text();
    assert!(text.contains("            at z 1200 1100\n            bundle a -> x b -> y c -> z\n"), "{text}");
    assert_eq!(sh.undo_steps(), 1);
    assert_eq!(sh.scene().bundles.len(), 1);
    assert!(!sh.scene().bundles[0].fallback);
    // Two of them grouped anew: the old bus keeps one member, so it goes,
    // in the same step.
    sh.click_line("B", "Y", NONE);
    sh.click_line("C", "Z", SHIFT);
    let menu = sh.menu_on("C", "Z");
    sh.pick(&menu, "Group as bus");
    let text = sh.text();
    assert!(text.contains("            bundle b -> y c -> z\n") && !text.contains("a -> x b"), "{text}");
    assert_eq!(sh.undo_steps(), 2);
    // One line selected and off every bus: nothing to offer.
    sh.click_line("A", "X", NONE);
    assert!(sh.s.bus_entries().is_empty());
}

#[test]
fn p4_425_ungroup_and_remove_from_bus() {
    let mut sh = Shell::open(&grid(&["bundle a -> x b -> y c -> z"]));
    sh.click_line("B", "Y", NONE);
    let menu = sh.menu_on("B", "Y");
    assert_eq!(labels(&menu), ["Ungroup bus", "Remove from bus"]);
    sh.pick(&menu, "Remove from bus");
    assert!(sh.text().contains("            bundle a -> x c -> z\n"), "{}", sh.text());
    // Removing one of two removes the whole line.
    sh.click_line("A", "X", NONE);
    let menu = sh.menu_on("A", "X");
    sh.pick(&menu, "Remove from bus");
    assert!(!sh.text().contains("bundle"), "{}", sh.text());
    assert_eq!(sh.undo_steps(), 2);
    // Ungroup removes the whole bus.
    let mut sh = Shell::open(&grid(&["bundle a -> x b -> y c -> z"]));
    sh.click_line("C", "Z", NONE);
    let menu = sh.menu_on("C", "Z");
    sh.pick(&menu, "Ungroup bus");
    assert_eq!(sh.text(), grid(&[]));
    assert_eq!(sh.undo_steps(), 1);
}

#[test]
fn p4_426_selecting_a_member_marks_its_bus() {
    let mut sh = Shell::open(&grid(&["bundle a -> x b -> y c -> z"]));
    sh.click_line("B", "Y", NONE);
    let (ax, cz) = (ItemKey::Line(sh.line("A", "X").edge), ItemKey::Line(sh.line("C", "Z").edge));
    let list = sh.s.draw_list();
    let peers: Vec<ItemKey> = list.marks.iter().filter(|m| m.kind == MarkKind::BusPeer).map(|m| m.item).collect();
    assert_eq!(peers, [ax, cz]);
    let halos: Vec<_> = crate::canvas::paint_items(&list)
        .into_iter()
        .filter(|p| matches!(p.source, Source::Mark(_, MarkKind::BusPeer)))
        .collect();
    assert_eq!(halos.len(), 2);
    for h in &halos {
        assert!(h.opacity < 50, "lighter than the selection mark");
        assert!(matches!(h.primitive, Primitive::Path { .. }), "a halo along the line");
    }
    // Off every bus: no such marks.
    let mut plain = Shell::open(&grid(&[]));
    plain.click_line("B", "Y", NONE);
    assert!(plain.s.draw_list().marks.iter().all(|m| m.kind != MarkKind::BusPeer));
}

#[test]
fn p4_427_bus_edits_atomic_undoable_checked_on_disk() {
    let original = grid(&[]);
    let mut sh = Shell::open(&original);
    sh.click_line("A", "X", NONE);
    sh.click_line("B", "Y", SHIFT);
    let menu = sh.menu_on("A", "X");
    sh.pick(&menu, "Group as bus");
    let grouped = sh.text();
    assert!(grouped.contains("bundle a -> x b -> y\n"), "{grouped}");
    sh.event(Event::Command(Command::Undo));
    assert_eq!(sh.text(), original, "undo gives the bytes back");
    sh.event(Event::Command(Command::Redo));
    assert_eq!(sh.text(), grouped);
    // EDIT-24: a change on disk the editor has not seen is kept; the bus
    // edit is re-applied to it.
    let external = grouped.replace("\"Top one.\"", "\"Top one, renamed.\"");
    std::fs::write(&sh.path, &external).expect("external save");
    sh.click_line("A", "X", NONE);
    let menu = sh.menu_on("A", "X");
    sh.pick(&menu, "Ungroup bus");
    let text = sh.text();
    assert!(text.contains("Top one, renamed."), "the external change survives: {text}");
    assert!(!text.contains("bundle"), "{text}");
}
