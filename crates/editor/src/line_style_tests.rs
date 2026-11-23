//! Tests of the Style section of a line selection and the Tags row on
//! lines (SRS 4.9 P5-357, P5-358), driven headlessly like
//! `tags_groups_tests`: effects run synchronously with `files` and
//! `worker::serve`. Every test that names an element runs for both
//! `testutil::Ids` modes.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::edit::{LineStyleProperty, LineStyleTarget, RelationshipRef};
use c4rizer_core::model::Colour;

use crate::files::{self, FileEvent, UiState};
use crate::state::{Command, Effect, Event, Selected, State};
use crate::testutil::{Ids, S_NESTED, TempDir};
use crate::worker;

const SHOP: &str = r#"workspace "Grid" {
    model {
        u = person "U" "A user." "Staff"
        s = softwareSystem "S" "Does things." {
            a = container "A" "Top one." "Rust"
            b = container "B" "Top two." "Rust"
            c = container "C" "Top three." "Rust"
        }
        u -> a "Uses"
        a -> b "Feeds B" "JSON" "Sync"
        b -> c "Feeds C" "JSON" "Sync"
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
            relationship "Async" {
                color #00aa00
            }
        }
    }
}
"#;

const RED: Colour = Colour { r: 0xaa, g: 0, b: 0 };

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    ids: Ids,
    _dir: TempDir,
}

impl Shell {
    fn open(ids: Ids) -> Shell {
        let dir = TempDir::new("line-style");
        let path = dir.join("shop.dsl");
        std::fs::write(&path, ids.workspace_in(SHOP, S_NESTED)).expect("fixture written");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
            ids,
            _dir: dir,
        };
        shell.command(Command::Open(path));
        assert_eq!(shell.s.scene().expect("a scene").key, "g");
        shell
    }

    fn command(&mut self, command: Command) {
        let effects = self.s.handle(Event::Command(command));
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

    fn text(&self) -> String {
        std::fs::read_to_string(&self.path).expect("read back")
    }

    fn refs(&self, text: &str) -> String {
        self.ids.refs_in(text, S_NESTED)
    }

    fn line(&self, from: &str, to: &str) -> RelationshipRef {
        let model = self.s.model().expect("parsed");
        let (f, t) = (model.find_written(&self.refs(from)).expect("from"), model.find_written(&self.refs(to)).expect("to"));
        RelationshipRef::of(model, model.find_relationship(f, t)[0])
    }

    fn select_lines(&mut self, lines: &[(&str, &str)]) {
        let refs: Vec<Selected> = lines.iter().map(|(f, t)| Selected::Relationship(self.line(f, t))).collect();
        self.s.selection = refs.into_iter().collect();
    }

    /// The stroke colour the line `from -> to` is drawn in.
    fn drawn(&self, from: &str, to: &str) -> Colour {
        let model = self.s.model().expect("parsed");
        let (f, t) = (model.find_written(&self.refs(from)).expect("from"), model.find_written(&self.refs(to)).expect("to"));
        let id = model.find_relationship(f, t)[0];
        self.s.scene().expect("a scene").lines.iter().find(|l| l.relationship == id).expect("drawn").stroke.colour
    }

    fn undo_steps(&self) -> usize {
        self.s.history.undo.len()
    }
}

fn this_line(colour: Colour) -> Command {
    Command::SetLineStyle {
        target: LineStyleTarget::ThisLine,
        property: LineStyleProperty::Color(Some(colour)),
    }
}

// ------------------------------------------------------------ one line

#[test]
fn p5_357_style_section_on_one_line() {
    body_p5_357_style_section_on_one_line(Ids::Flat);
}

#[test]
fn p5_357_style_section_on_one_line_hierarchical_ids() {
    body_p5_357_style_section_on_one_line(Ids::Hierarchical);
}

fn body_p5_357_style_section_on_one_line(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.select_lines(&[("a", "b")]);
    let panel = sh.s.line_style_panel().expect("a line style section");
    assert_eq!(panel.lines, 1);
    assert_eq!(panel.tags, ["Relationship", "Sync"]);
    assert_eq!(panel.color, Some(sh.drawn("a", "b")), "the colour as drawn");
    // This line only: a per-line tag and block, one undo step.
    sh.command(this_line(RED));
    let text = sh.text();
    assert!(text.contains("tags \"s.a->s.b\""), "{text}");
    assert!(text.contains("            relationship \"s.a->s.b\" {\n                color #aa0000\n            }\n"), "{text}");
    assert_eq!(sh.undo_steps(), 1);
    assert_eq!(sh.drawn("a", "b"), RED);
    assert_ne!(sh.drawn("b", "c"), RED, "only this line");
    assert_eq!(sh.s.line_style_panel().expect("still lines").color, Some(RED));
    // Reset to tag style removes both; undo twice gives the bytes back.
    sh.command(Command::ResetStyle);
    assert_eq!(sh.text(), original);
    assert_eq!(sh.undo_steps(), 2);
    sh.command(Command::Undo);
    assert_eq!(sh.text(), text, "undo of the reset gives the per-line style back");
    sh.command(Command::Undo);
    assert_eq!(sh.text(), original, "undo gives the bytes back");
}

// ------------------------------------------------------- several lines

#[test]
fn p5_357_several_lines_one_undo_step() {
    body_p5_357_several_lines_one_undo_step(Ids::Flat);
}

#[test]
fn p5_357_several_lines_one_undo_step_hierarchical_ids() {
    body_p5_357_several_lines_one_undo_step(Ids::Hierarchical);
}

fn body_p5_357_several_lines_one_undo_step(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.select_lines(&[("a", "b"), ("b", "c"), ("u", "a")]);
    let panel = sh.s.line_style_panel().expect("a line style section");
    assert_eq!(panel.lines, 3);
    assert_eq!(panel.tags, ["Relationship"], "only the tags every line carries");
    sh.command(this_line(RED));
    assert_eq!(sh.undo_steps(), 1, "one undo step");
    for (f, t) in [("a", "b"), ("b", "c"), ("u", "a")] {
        assert_eq!(sh.drawn(f, t), RED);
    }
    sh.command(Command::Undo);
    assert_eq!(sh.text(), original);
    // `all Sync` on the two Sync lines: one block, one step.
    sh.select_lines(&[("a", "b"), ("b", "c")]);
    sh.command(Command::SetLineStyle {
        target: LineStyleTarget::Tag("Sync".to_owned()),
        property: LineStyleProperty::Color(Some(RED)),
    });
    assert_eq!(sh.text().matches("relationship \"Sync\"").count(), 1, "{}", sh.text());
    assert_eq!(sh.undo_steps(), 1);
    assert_eq!((sh.drawn("a", "b"), sh.drawn("b", "c")), (RED, RED));
    assert_ne!(sh.drawn("u", "a"), RED);
    sh.command(Command::Undo);
    assert_eq!(sh.text(), original);
}

#[test]
fn p5_357_no_line_section_for_boxes_or_a_mixed_selection() {
    body_p5_357_no_line_section_for_boxes_or_a_mixed_selection(Ids::Flat);
}

#[test]
fn p5_357_no_line_section_for_boxes_or_a_mixed_selection_hierarchical_ids() {
    body_p5_357_no_line_section_for_boxes_or_a_mixed_selection(Ids::Hierarchical);
}

fn body_p5_357_no_line_section_for_boxes_or_a_mixed_selection(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    assert!(sh.s.line_style_panel().is_none(), "nothing selected");
    let a = c4rizer_core::model::ElementKey::from_written(sh.refs("a"));
    sh.s.selection = [Selected::Element(a.clone())].into_iter().collect();
    assert!(sh.s.line_style_panel().is_none(), "a box");
    assert!(sh.s.style_panel().is_some());
    sh.select_lines(&[("a", "b")]);
    assert!(sh.s.style_panel().is_none(), "a line has no box section");
    sh.s.selection.insert(Selected::Element(a));
    assert!(sh.s.line_style_panel().is_none(), "boxes and lines together");
    // A line command with boxes selected writes nothing.
    sh.command(this_line(RED));
    assert_eq!(sh.text(), original);
}

// ----------------------------------------------------- tags row on lines

#[test]
fn p5_358_tags_row_on_several_lines_one_step() {
    body_p5_358_tags_row_on_several_lines_one_step(Ids::Flat);
}

#[test]
fn p5_358_tags_row_on_several_lines_one_step_hierarchical_ids() {
    body_p5_358_tags_row_on_several_lines_one_step(Ids::Hierarchical);
}

fn body_p5_358_tags_row_on_several_lines_one_step(ids: Ids) {
    let mut sh = Shell::open(ids);
    let original = sh.text();
    sh.select_lines(&[("a", "b"), ("b", "c")]);
    let row = sh.s.tag_row().expect("a tags row");
    assert!(row.lines);
    assert_eq!(row.tags, ["Relationship", "Sync"]);
    assert_eq!(row.removable, ["Sync"]);
    sh.command(Command::AddTag("Async".to_owned()));
    assert_eq!(sh.undo_steps(), 1, "one undo step");
    assert_eq!(sh.text().matches("\"Sync,Async\"").count() + sh.text().matches("tags \"Async\"").count(), 2, "{}", sh.text());
    // The tag's style colours both lines.
    let green = Colour { r: 0, g: 0xaa, b: 0 };
    assert_eq!((sh.drawn("a", "b"), sh.drawn("b", "c")), (green, green));
    sh.command(Command::RemoveTag("Sync".to_owned()));
    assert_eq!(sh.undo_steps(), 2);
    assert!(!sh.text().contains("Sync"), "{}", sh.text());
    sh.command(Command::Undo);
    sh.command(Command::Undo);
    assert_eq!(sh.text(), original);
}

// ---------------------------------------------------------------- panel

/// P5-357: the section paints the box section's rows, top to bottom,
/// and a pick on the swatch writes `SetLineStyle` for the chosen target.
#[test]
fn p5_357_line_style_section_rows() {
    use eframe::egui::{self, Pos2, RawInput, Rect, Shape, Vec2};

    use crate::state::LineStylePanel;
    fn texts(shape: &Shape, out: &mut Vec<String>) {
        match shape {
            Shape::Text(t) => out.push(t.galley.text().to_owned()),
            Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let panel = LineStylePanel {
        color: Some(RED),
        tags: vec!["Relationship".to_owned(), "Sync".to_owned()],
        lines: 2,
    };
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 400.0))),
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut output = ctx.run_ui(input, |ui| crate::app::line_style_section_for_tests(ui, &panel, &mut out));
    output.textures_delta.clear();
    let mut shown: Vec<(String, Rect)> = Vec::new();
    for c in &output.shapes {
        let mut t = Vec::new();
        texts(&c.shape, &mut t);
        shown.extend(t.into_iter().map(|t| (t, c.shape.visual_bounding_rect())));
    }
    let at = |text: &str| shown.iter().find(|(t, _)| t == text).map_or_else(|| panic!("{text:?} not in {shown:?}"), |(_, r)| *r);
    // Section titles and form labels paint in capitals.
    let (title, apply, colour, reset) = (at("STYLE · 2 LINES"), at("APPLY TO"), at("COLOUR"), at("Reset to tag style"));
    at("#aa0000");
    at("this line only");
    assert!(title.max.y <= apply.min.y && apply.max.y <= colour.min.y && colour.max.y <= reset.min.y, "{shown:?}");
    assert!((apply.min.x - colour.min.x).abs() < 0.5, "labels aligned: {apply:?} {colour:?}");
    assert!(out.is_empty());
}
