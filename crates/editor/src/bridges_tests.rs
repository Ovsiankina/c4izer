//! Tests of the bridges row, editor part (SRS 4.8 P4-521..P4-523), driven
//! headlessly like `bundle_tests`: effects run synchronously with `files`
//! and `worker::serve`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::Point;
use c4rizer_core::model::CrossingStyle;
use c4rizer_core::scene::{ItemKey, Scene};
use c4rizer_core::view::EdgeId;

use crate::canvas::{Primitive, Source};
use crate::files::{self, FileEvent, UiState};
use crate::state::{Button, Command, Effect, Event, Input, MenuAction, Modifiers, Overlay, State};
use crate::testutil::TempDir;
use crate::worker;

/// Two lines pinned inside a ring of four touching boxes, crossing square
/// at (630, 410); `{lines}` is replaced by more view lines.
const RING: &str = r#"workspace "Ring" {
    model {
        s = softwareSystem "S" "Does things." {
            a = container "A" "West." "Rust"
            b = container "B" "East." "Rust"
            c = container "C" "North." "Rust"
            d = container "D" "South." "Rust"
        }
        a -> b
        c -> d
    }
    views {
        container s "ring" "Ring" {
            include a b c d
            maxCrossings 1
            at a 300 500
            at b 1100 500
            at c 700 100
            at d 700 900
            size c 2400 100
            size d 2400 100
            size a 200 900
            size b 200 900
            anchor c -> d from bottom 47 to top 47
            anchor a -> b from right 40 to left 40
{lines}
        }
    }
}
"#;

/// Where the two lines of `RING` cross, px.
const CROSSING: Point = Point::new(630, 410);

const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
};

fn ring(lines: &[&str]) -> String {
    let text = lines.iter().fold(String::new(), |mut out, l| {
        out.push_str("            ");
        out.push_str(l);
        out.push('\n');
        out
    });
    RING.replace("{lines}\n", &text)
}

/// The shell without a window: every effect at once.
struct Shell {
    s: State,
    path: PathBuf,
    _dir: TempDir,
}

impl Shell {
    fn open(text: &str) -> Shell {
        let dir = TempDir::new("bridges");
        let path = dir.join("ring.dsl");
        std::fs::write(&path, text).expect("fixture written");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
            _dir: dir,
        };
        shell.event(Event::Command(Command::Open(path)));
        assert_eq!(shell.scene().key, "ring");
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

    /// The context menu of a right click at `at` (px).
    fn menu_at(&mut self, at: Point) -> Vec<(String, MenuAction)> {
        let at = self.s.camera.to_screen(at);
        self.event(Event::Input(Input::PointerDown { at, button: Button::Right, mods: NONE }));
        self.event(Event::Input(Input::PointerUp { at, button: Button::Right }));
        let Some(Overlay::ContextMenu { entries }) = self.s.overlay.clone() else {
            return Vec::new();
        };
        entries.into_iter().map(|e| (e.label, e.action)).collect()
    }

    fn pick(&mut self, menu: &[(String, MenuAction)], label: &str) {
        let action = menu.iter().find(|(l, _)| l == label).unwrap_or_else(|| panic!("no {label} in {menu:?}")).1.clone();
        self.event(Event::Command(Command::ContextAction(action)));
    }

    fn carrier(&self) -> EdgeId {
        assert_eq!(self.scene().bridges.len(), 1);
        self.scene().bridges[0].carrier
    }

    fn undo_steps(&self) -> usize {
        self.s.history.undo.len()
    }
}

const FLIP: &str = "Bridge on the other line";

#[test]
fn p4_521_crossings_choice_writes_the_setting() {
    let original = ring(&[]);
    let mut sh = Shell::open(&original);
    assert_eq!(sh.s.crossing_style(), Some(CrossingStyle::Gap));
    sh.event(Event::Command(Command::SetCrossingStyle(CrossingStyle::Jump)));
    assert!(sh.text().contains("            maxCrossings 1\n            crossings jump\n"), "{}", sh.text());
    assert_eq!(sh.s.crossing_style(), Some(CrossingStyle::Jump));
    let carrier = sh.scene().lines.iter().find(|l| l.edge == sh.carrier()).expect("carrier").clone();
    assert!(carrier.bars.is_empty() && carrier.runs.len() == 1, "a hop");
    sh.event(Event::Command(Command::SetCrossingStyle(CrossingStyle::Plain)));
    assert!(sh.text().contains("            crossings plain\n"));
    assert!(sh.scene().bridges.is_empty());
    sh.event(Event::Command(Command::SetCrossingStyle(CrossingStyle::Gap)));
    assert_eq!(sh.text(), original, "gap, the default, removes the line");
    assert_eq!(sh.undo_steps(), 3);
    // Picking the style the view has writes nothing.
    sh.event(Event::Command(Command::SetCrossingStyle(CrossingStyle::Gap)));
    assert_eq!(sh.undo_steps(), 3);
    sh.event(Event::Command(Command::Undo));
    assert!(sh.text().contains("crossings plain"));
}

#[test]
fn p4_522_bridge_on_the_other_line() {
    let original = ring(&[]);
    let mut sh = Shell::open(&original);
    assert_eq!(sh.carrier(), EdgeId(0));
    // Off the crossing, on a line: not offered.
    let menu = sh.menu_at(Point::new(500, 410));
    assert!(menu.iter().all(|(l, _)| l != FLIP), "{menu:?}");
    let menu = sh.menu_at(CROSSING);
    sh.pick(&menu, FLIP);
    let flipped = sh.text();
    assert!(flipped.contains("            anchor a -> b from right 40 to left 40\n            bridge c -> d a -> b\n"), "{flipped}");
    assert_eq!(sh.carrier(), EdgeId(1));
    assert_eq!(sh.undo_steps(), 1);
    // Back: the line goes.
    let menu = sh.menu_at(CROSSING);
    sh.pick(&menu, FLIP);
    assert_eq!(sh.text(), original);
    assert_eq!(sh.carrier(), EdgeId(0));
    // Undo and redo give the bytes back.
    sh.event(Event::Command(Command::Undo));
    assert_eq!(sh.text(), flipped);
    sh.event(Event::Command(Command::Redo));
    assert_eq!(sh.text(), original);
    // EDIT-24: a change on disk the editor has not seen is kept.
    let external = original.replace("\"West.\"", "\"West, renamed.\"");
    std::fs::write(&sh.path, &external).expect("external save");
    let menu = sh.menu_at(CROSSING);
    sh.pick(&menu, FLIP);
    let text = sh.text();
    assert!(text.contains("West, renamed.") && text.contains("bridge c -> d a -> b"), "{text}");
}

#[test]
fn p4_523_canvas_paints_the_scene_runs_and_bars() {
    for lines in [&[][..], &["crossings jump"][..]] {
        let sh = Shell::open(&ring(lines));
        let scene = sh.scene();
        let carrier = scene.lines.iter().find(|l| l.edge == sh.carrier()).expect("carrier");
        let list = sh.s.draw_list();
        let paths: Vec<Vec<(f32, f32)>> = crate::canvas::paint_items(&list)
            .into_iter()
            .filter(|p| p.source == Source::Scene(ItemKey::Line(carrier.edge)))
            .filter_map(|p| match p.primitive {
                Primitive::Path { points, .. } => Some(points.iter().map(|q| (q.x, q.y)).collect()),
                _ => None,
            })
            .collect();
        let cam = &sh.s.camera;
        let screen = |pts: &[c4rizer_core::geom::Point]| -> Vec<(f32, f32)> {
            pts.iter().map(|&p| cam.to_screen(p)).map(|q| (q.x, q.y)).collect()
        };
        // redesign 2026-11-19: each run is painted from its first point
        // with rounded corners (and the run reaching the arrow stops in
        // its notch, within 6px of its end), so runs are compared by their
        // ends; bars and chevrons are painted exactly as the scene gives
        // them.
        let runs = carrier.runs.len();
        let expect_rest: Vec<Vec<(f32, f32)>> = carrier
            .bars
            .iter()
            .map(|b| screen(b))
            .chain(carrier.chevrons.iter().map(|c| screen(&c.points)))
            .collect();
        assert_eq!(paths.len(), runs + expect_rest.len(), "{lines:?}");
        assert_eq!(&paths[runs..], &expect_rest[..], "{lines:?}");
        for (painted, run) in paths[..runs].iter().zip(&carrier.runs) {
            let want = screen(run);
            assert_eq!(painted.first(), want.first(), "{lines:?}");
            let (a, b) = (painted.last().unwrap(), want.last().unwrap());
            assert!((a.0 - b.0).hypot(a.1 - b.1) <= 6.0 * cam.zoom + 1e-3, "{lines:?}: {a:?} vs {b:?}");
        }
        assert!(!carrier.runs.is_empty());
    }
}
