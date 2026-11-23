//! Tests of the Tags and Group rows, popups and menu entries (SRS 4.9
//! P5-106..P5-109, P5-204..P5-206), driven headlessly like
//! `bundle_tests`: effects run synchronously with `files` and
//! `worker::serve`. Every test that names an element runs for both
//! `testutil::Ids` modes.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState};
use crate::state::{Button, Command, Effect, Event, Input, Key, MenuAction, Modifiers, Overlay, Selected, State};
use crate::testutil::{Ids, S_NESTED, TempDir};
use crate::worker;

const SHOP: &str = r#"workspace "Grid" {
    model {
        u = person "U" "A user." "Staff"
        s = softwareSystem "S" "Does things." {
            a = container "A" "Top one." "Rust" {
                tags "Front"
            }
            b = container "B" "Top two." "Rust"
            c = container "C" "Top three." "Rust"
        }
        u -> a "Uses"
        a -> b "Feeds B" "JSON" "Sync"
        b -> c "Feeds C"
    }
    views {
        container s "g" "Grid" {
            include *
            at u 400 100
            at a 400 500
            at b 800 500
            at c 1200 500
        }
        styles {
            element "Edge" {
                background #aa0000
            }
            relationship "Async" {
                color #00aa00
            }
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
const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };
const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, shift: true, alt: false };

fn shop(ids: Ids) -> String {
    ids.workspace_in(SHOP, S_NESTED)
}

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    ids: Ids,
    _dir: TempDir,
}

impl Shell {
    fn open(ids: Ids) -> Shell {
        let dir = TempDir::new("tags-groups");
        let path = dir.join("shop.dsl");
        std::fs::write(&path, shop(ids)).expect("fixture written");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
            ids,
            _dir: dir,
        };
        shell.event(Event::Command(Command::Open(path)));
        assert_eq!(shell.s.scene().expect("a scene").key, "g");
        shell
    }

    fn event(&mut self, event: Event) {
        let effects = self.s.handle(event);
        self.run(effects);
    }

    fn command(&mut self, command: Command) {
        self.event(Event::Command(command));
    }

    fn key(&mut self, key: Key, mods: Modifiers) {
        self.event(Event::Input(Input::KeyDown { key, mods, text_focus: false }));
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

    fn text(&self) -> String {
        std::fs::read_to_string(&self.path).expect("read back")
    }

    /// `text` with nested references in this file's id mode.
    fn refs(&self, text: &str) -> String {
        self.ids.refs_in(text, S_NESTED)
    }

    fn box_centre(&self, key: &str) -> ScreenPoint {
        let model = self.s.model().expect("parsed");
        let scene = self.s.scene().expect("a scene");
        let id = model.find_written(&self.refs(key)).expect("element");
        let b = scene.boxes_of(id)[0];
        self.s.camera.to_screen(scene.boxes[b.0 as usize].rect.centre())
    }

    fn click(&mut self, at: ScreenPoint, button: Button, mods: Modifiers) {
        self.event(Event::Input(Input::PointerDown { at, button, mods }));
        self.event(Event::Input(Input::PointerUp { at, button }));
    }

    fn click_box(&mut self, key: &str, mods: Modifiers) {
        let at = self.box_centre(key);
        self.click(at, Button::Left, mods);
    }

    /// The context menu of a right click on box `key`.
    fn menu_on(&mut self, key: &str) -> Vec<(String, MenuAction)> {
        let at = self.box_centre(key);
        self.click(at, Button::Right, NONE);
        let Some(Overlay::ContextMenu { entries }) = self.s.overlay.clone() else {
            return Vec::new();
        };
        entries.into_iter().map(|e| (e.label, e.action)).collect()
    }

    /// Selects the line `from -> to` by its winning relationship.
    fn select_line(&mut self, from: &str, to: &str) {
        let model = self.s.model().expect("parsed");
        let (f, t) = (model.find_written(&self.refs(from)).expect("from"), model.find_written(&self.refs(to)).expect("to"));
        let r = c4rizer_core::edit::RelationshipRef::of(model, model.find_relationship(f, t)[0]);
        self.s.selection = [Selected::Relationship(r)].into_iter().collect();
    }

    fn undo_steps(&self) -> usize {
        self.s.history.undo.len()
    }

    fn selected_keys(&self) -> Vec<String> {
        self.s
            .selection
            .iter()
            .filter_map(|s| match s {
                Selected::Element(k) => Some(k.to_string()),
                _ => None,
            })
            .collect()
    }
}

fn labels(menu: &[(String, MenuAction)]) -> Vec<&str> {
    menu.iter().map(|(l, _)| l.as_str()).collect()
}

// ------------------------------------------------------------------ tags

#[test]
fn p5_204_tags_row_add_and_remove_on_a_box() {
    body_p5_204_tags_row_add_and_remove_on_a_box(Ids::Flat);
}

#[test]
fn p5_204_tags_row_add_and_remove_on_a_box_hierarchical_ids() {
    body_p5_204_tags_row_add_and_remove_on_a_box(Ids::Hierarchical);
}

fn body_p5_204_tags_row_add_and_remove_on_a_box(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.click_box("a", NONE);
    let row = sh.s.tag_row().expect("a tags row");
    assert_eq!(row.tags, ["Element", "Container", "Front"]);
    assert_eq!(row.removable, ["Front"], "implicit tags have no remove mark");
    assert!(!row.lines);
    // "+" opens the popup; typing and Enter adds the typed tag.
    sh.command(Command::TagPopup);
    assert!(matches!(sh.s.overlay, Some(Overlay::AddTag { .. })));
    sh.event(Event::Text("Edge".to_owned()));
    sh.key(Key::Enter, NONE);
    assert!(sh.s.overlay.is_none(), "Enter closes the popup");
    assert!(sh.text().contains("                tags \"Front\"\n                tags \"Edge\"\n"), "{}", sh.text());
    assert_eq!(sh.undo_steps(), 1);
    assert_eq!(sh.selected_keys(), [sh.refs("a")], "the selection is kept");
    // The remove mark writes `RemoveTag`.
    sh.command(Command::RemoveTag("Front".to_owned()));
    assert!(!sh.text().contains("\"Front\""), "{}", sh.text());
    assert_eq!(sh.undo_steps(), 2);
    sh.command(Command::Undo);
    sh.command(Command::Undo);
    assert_eq!(sh.text(), original, "undo gives the bytes back");
}

#[test]
fn p5_204_tags_row_on_a_line() {
    body_p5_204_tags_row_on_a_line(Ids::Flat);
}

#[test]
fn p5_204_tags_row_on_a_line_hierarchical_ids() {
    body_p5_204_tags_row_on_a_line(Ids::Hierarchical);
}

fn body_p5_204_tags_row_on_a_line(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.select_line("a", "b");
    let row = sh.s.tag_row().expect("a tags row");
    assert!(row.lines);
    assert_eq!(row.tags, ["Relationship", "Sync"]);
    assert_eq!(row.removable, ["Sync"]);
    sh.command(Command::RemoveTag("Sync".to_owned()));
    assert!(sh.text().contains(&sh.refs("        a -> b \"Feeds B\" \"JSON\"\n")), "{}", sh.text());
    sh.command(Command::AddTag("Async".to_owned()));
    assert!(sh.text().contains(&sh.refs("        a -> b \"Feeds B\" \"JSON\" {\n            tags \"Async\"\n        }\n")), "{}", sh.text());
    assert_eq!(sh.undo_steps(), 2);
    sh.command(Command::Undo);
    sh.command(Command::Undo);
    assert_eq!(sh.text(), original);
}

#[test]
fn p5_205_popup_lists_filters_and_adds_a_row() {
    body_p5_205_popup_lists_filters_and_adds_a_row(Ids::Flat);
}

#[test]
fn p5_205_popup_lists_filters_and_adds_a_row_hierarchical_ids() {
    body_p5_205_popup_lists_filters_and_adds_a_row(Ids::Hierarchical);
}

fn body_p5_205_popup_lists_filters_and_adds_a_row(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.click_box("a", NONE);
    sh.command(Command::TagPopup);
    // Element tags declared anywhere or named by a style, minus those A
    // carries ("Front"); no relationship tag.
    assert_eq!(sh.s.tag_popup_rows(), ["Edge", "Staff"]);
    sh.event(Event::Text("sT".to_owned()));
    assert_eq!(sh.s.tag_popup_rows(), ["Staff"], "filtered by the typed text, any case");
    // Escape closes, nothing written.
    sh.key(Key::Escape, NONE);
    assert!(sh.s.overlay.is_none());
    assert_eq!(sh.text(), original);
    assert_eq!(sh.selected_keys(), [sh.refs("a")], "Escape on the popup keeps the selection");
    // A click on a row adds that tag.
    sh.command(Command::TagPopup);
    sh.command(Command::AddTag("Staff".to_owned()));
    assert!(sh.s.overlay.is_none());
    assert!(sh.text().contains("                tags \"Front\"\n                tags \"Staff\"\n"), "{}", sh.text());
    // Lines: relationship tags only.
    sh.select_line("a", "b");
    sh.command(Command::TagPopup);
    assert_eq!(sh.s.tag_popup_rows(), ["Async"]);
}

#[test]
fn p5_206_several_boxes_one_step_and_mixed_selection() {
    body_p5_206_several_boxes_one_step_and_mixed_selection(Ids::Flat);
}

#[test]
fn p5_206_several_boxes_one_step_and_mixed_selection_hierarchical_ids() {
    body_p5_206_several_boxes_one_step_and_mixed_selection(Ids::Hierarchical);
}

fn body_p5_206_several_boxes_one_step_and_mixed_selection(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.click_box("a", NONE);
    sh.click_box("b", SHIFT);
    let row = sh.s.tag_row().expect("a tags row");
    assert_eq!(row.tags, ["Element", "Container"], "only the shared tags");
    assert!(row.removable.is_empty());
    sh.command(Command::AddTag("Edge".to_owned()));
    let text = sh.text();
    assert_eq!(text.matches("tags \"Edge\"").count(), 2, "{text}");
    assert_eq!(sh.undo_steps(), 1, "one undo step");
    let row = sh.s.tag_row().expect("a tags row");
    assert_eq!(row.removable, ["Edge"]);
    sh.command(Command::RemoveTag("Edge".to_owned()));
    assert_eq!(sh.text(), original);
    sh.command(Command::Undo);
    sh.command(Command::Undo);
    assert_eq!(sh.text(), original);
    // Boxes and lines together: no Tags row.
    let model = sh.s.model().expect("parsed");
    let (a, b) = (model.find_written(&sh.refs("a")).expect("a"), model.find_written(&sh.refs("b")).expect("b"));
    let r = c4rizer_core::edit::RelationshipRef::of(model, model.find_relationship(a, b)[0]);
    sh.s.selection.insert(Selected::Relationship(r));
    assert!(sh.s.tag_row().is_none());
}

// ---------------------------------------------------------------- groups

#[test]
fn p5_106_group_row_choices_and_pick() {
    body_p5_106_group_row_choices_and_pick(Ids::Flat);
}

#[test]
fn p5_106_group_row_choices_and_pick_hierarchical_ids() {
    body_p5_106_group_row_choices_and_pick(Ids::Hierarchical);
}

fn body_p5_106_group_row_choices_and_pick(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.click_box("a", NONE);
    sh.click_box("b", SHIFT);
    let row = sh.s.group_row().expect("a group row");
    assert!(row.enabled && !row.grouped && !row.mixed);
    assert_eq!(row.current, None);
    assert!(row.choices.is_empty());
    // "New group…" opens the popup, prefilled.
    sh.command(Command::GroupPopup);
    assert_eq!(sh.s.overlay, Some(Overlay::GroupName { name: "Group".to_owned() }));
    sh.event(Event::Text("Core".to_owned()));
    sh.key(Key::Enter, NONE);
    assert!(sh.s.overlay.is_none());
    let text = sh.text();
    assert!(
        text.contains("        s = softwareSystem \"S\" \"Does things.\" {\n            group \"Core\" {\n                a = container \"A\" \"Top one.\" \"Rust\" {\n                    tags \"Front\"\n                }\n                b = container \"B\" \"Top two.\" \"Rust\"\n            }\n            c = "),
        "{text}"
    );
    assert_eq!(sh.undo_steps(), 1);
    assert_eq!(sh.selected_keys().len(), 2, "the selection is kept");
    assert_eq!(sh.s.group_row().expect("row").current.as_deref(), Some("Core"));
    // The views are untouched: keys did not change.
    let views = |t: &str| t[t.find("    views {").expect("views")..].to_owned();
    assert_eq!(views(&text), views(&original));
    // C joins the existing group from the select.
    sh.click_box("c", NONE);
    let row = sh.s.group_row().expect("a group row");
    assert_eq!(row.choices, ["Core"]);
    sh.command(Command::SetGroup(Some("Core".to_owned())));
    assert!(sh.text().contains("                b = container \"B\" \"Top two.\" \"Rust\"\n                c = container \"C\" \"Top three.\" \"Rust\"\n            }\n"), "{}", sh.text());
    // "None" takes it out again, right after the group.
    sh.command(Command::SetGroup(None));
    assert!(sh.text().contains("            }\n            c = container \"C\" \"Top three.\" \"Rust\"\n        }\n"), "{}", sh.text());
    assert_eq!(sh.undo_steps(), 3);
    for _ in 0..3 {
        sh.command(Command::Undo);
    }
    assert_eq!(sh.text(), original, "undo gives the bytes back");
}

#[test]
fn p5_107_name_popup_refuses_empty_and_escape_cancels() {
    body_p5_107_name_popup_refuses_empty_and_escape_cancels(Ids::Flat);
}

#[test]
fn p5_107_name_popup_refuses_empty_and_escape_cancels_hierarchical_ids() {
    body_p5_107_name_popup_refuses_empty_and_escape_cancels(Ids::Hierarchical);
}

fn body_p5_107_name_popup_refuses_empty_and_escape_cancels(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.click_box("a", NONE);
    sh.key(Key::G, CTRL);
    assert_eq!(sh.s.overlay, Some(Overlay::GroupName { name: "Group".to_owned() }), "Ctrl+G opens the popup");
    sh.event(Event::Text("   ".to_owned()));
    sh.key(Key::Enter, NONE);
    assert!(matches!(sh.s.overlay, Some(Overlay::GroupName { .. })), "an empty name keeps the popup open");
    assert_eq!(sh.s.notice.as_deref(), Some("a group needs a name"));
    sh.key(Key::Escape, NONE);
    assert!(sh.s.overlay.is_none());
    assert_eq!(sh.text(), original, "nothing written");
    assert_eq!(sh.undo_steps(), 0);
    // Enter on the prefilled name writes a group "Group".
    sh.key(Key::G, CTRL);
    sh.key(Key::Enter, NONE);
    assert!(sh.text().contains("            group \"Group\" {\n"), "{}", sh.text());
}

#[test]
fn p5_108_menu_group_and_remove_from_group() {
    body_p5_108_menu_group_and_remove_from_group(Ids::Flat);
}

#[test]
fn p5_108_menu_group_and_remove_from_group_hierarchical_ids() {
    body_p5_108_menu_group_and_remove_from_group(Ids::Hierarchical);
}

fn body_p5_108_menu_group_and_remove_from_group(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.click_box("a", NONE);
    sh.click_box("b", SHIFT);
    let menu = sh.menu_on("b");
    assert_eq!(sh.selected_keys().len(), 2, "a right click on a selected box keeps the selection");
    assert!(labels(&menu).contains(&"Group…"));
    assert!(!labels(&menu).contains(&"Remove from group"), "no selected box is in a group");
    let group = menu.iter().find(|(l, _)| l == "Group…").expect("Group…").1.clone();
    sh.command(Command::ContextAction(group));
    assert!(matches!(sh.s.overlay, Some(Overlay::GroupName { .. })));
    sh.key(Key::Enter, NONE);
    assert!(sh.text().contains("            group \"Group\" {\n"), "{}", sh.text());
    let menu = sh.menu_on("a");
    assert!(labels(&menu).contains(&"Remove from group"), "{menu:?}");
    let remove = menu.iter().find(|(l, _)| l == "Remove from group").expect("entry").1.clone();
    sh.command(Command::ContextAction(remove));
    assert_eq!(sh.text(), original, "both out, the emptied group removed");
    // Ctrl+Shift+G does the same.
    sh.key(Key::G, CTRL);
    sh.key(Key::Enter, NONE);
    assert_ne!(sh.text(), original);
    sh.key(Key::G, CTRL_SHIFT);
    assert_eq!(sh.text(), original);
    assert_eq!(sh.undo_steps(), 4);
}

#[test]
fn p5_109_boxes_of_different_parents() {
    body_p5_109_boxes_of_different_parents(Ids::Flat);
}

#[test]
fn p5_109_boxes_of_different_parents_hierarchical_ids() {
    body_p5_109_boxes_of_different_parents(Ids::Hierarchical);
}

fn body_p5_109_boxes_of_different_parents(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.click_box("u", NONE);
    sh.click_box("a", SHIFT);
    let row = sh.s.group_row().expect("a group row");
    assert!(!row.enabled, "the row is disabled");
    assert!(row.choices.is_empty());
    let menu = sh.menu_on("a");
    assert!(!labels(&menu).contains(&"Group…"), "greyed: not offered");
    sh.key(Key::Escape, NONE);
    sh.click_box("u", NONE);
    sh.click_box("a", SHIFT);
    sh.key(Key::G, CTRL);
    assert!(sh.s.overlay.is_none(), "Ctrl+G does nothing");
    sh.command(Command::SetGroup(Some("G".to_owned())));
    assert_eq!(sh.text(), original);
}

// ---------------------------------------------------------------- widget

/// P5-204: the tag row widget: a click on a removable chip's mark says
/// Remove, on the "+" chip Add, on a chip's text nothing.
#[test]
fn p5_204_tag_row_widget_marks() {
    use eframe::egui::{self, Event as E, FontId, PointerButton, Pos2, RawInput, Rect, Vec2};

    use crate::kit::{self, TagRowAction};
    use crate::theme::{INK, SIZE_ICON};

    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let tags = vec!["Element".to_owned(), "Front".to_owned()];
    let removable = vec!["Front".to_owned()];
    let origin = std::cell::Cell::new(Pos2::ZERO);
    let widths = std::cell::Cell::new((0.0_f32, 0.0_f32));
    let run = |events: Vec<E>| -> Option<TagRowAction> {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))),
            events,
            ..Default::default()
        };
        let mut action = None;
        let mut output = ctx.run_ui(input, |ui| {
            let font = FontId::proportional(11.0);
            let w = |t: &str| ui.fonts_mut(|f| f.layout_no_wrap(t.to_owned(), font.clone(), INK).size().x);
            widths.set((w("Element"), w("Front")));
            origin.set(ui.cursor().min);
            action = kit::tag_row(ui, &tags, &removable, true);
        });
        output.textures_delta.clear();
        action
    };
    let click = |p: Pos2| {
        vec![
            E::PointerMoved(p),
            E::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            E::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
        ]
    };
    assert_eq!(run(Vec::new()), None);
    let (o, (element, front)) = (origin.get(), widths.get());
    let front_min = o.x + element + 12.0 + 4.0;
    let front_max = front_min + front + 12.0 + SIZE_ICON;
    let mark = Pos2::new(front_max - 2.0 - SIZE_ICON / 2.0, o.y + 9.0);
    let plus = Pos2::new(front_max + 4.0 + 9.0, o.y + 9.0);
    assert_eq!(run(click(plus)), Some(TagRowAction::Add));
    run(vec![E::PointerMoved(mark)]);
    assert_eq!(run(click(mark)), Some(TagRowAction::Remove("Front".to_owned())));
    let text = Pos2::new(front_min + 8.0, o.y + 9.0);
    run(vec![E::PointerMoved(text)]);
    assert_eq!(run(click(text)), None, "a click on the tag's text removes nothing");
}
