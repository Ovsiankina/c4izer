//! Tests of alignment guides (SRS 4.9 align-guides, P5-500..P5-507),
//! driven headlessly like `bundle_tests`; every test that names an
//! element runs for both `Ids` modes. Benchmarks `perf_p5_5*` (P5-900).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::Point;
use c4rizer_core::scene::Scene;
use c4rizer_core::view::BoxId;

use crate::camera::ScreenPoint;
use crate::canvas::{Primitive, Source, paint_items};
use crate::files::{self, FileEvent, UiState};
use crate::state::guides::{GUIDE_OVERHANG_PX, Guide, GuideIndex};
use crate::state::{Button, Command, Effect, Event, Gesture, Input, Modifiers, State};
use crate::testutil::{Ids, S_NESTED, TempDir, bench, debug_build};
use crate::worker::{self, ViewRef};

const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
};
const SHIFT: Modifiers = Modifiers { shift: true, ..NONE };

/// `a` top left, `b` and `c` further right and down, `x` in a second view;
/// containers are 240x140.
const FIELD: &str = r#"workspace "G" {
    model {
        s = softwareSystem "S" "Does things." {
            a = container "Alpha" "First one." "Rust"
            b = container "Beta" "Second one." "Rust"
            c = container "Gamma" "Third one." "Rust"
        }
    }
    views {
        container s "g" "Guides" {
            include *
            at a 200 200
            at b 600 600
            at c 1000 600
        }
        systemContext s "ctx" "Context" {
            include *
        }
    }
}
"#;

/// The shell without a window: every effect at once; UI state saves
/// kept.
struct Shell {
    s: State,
    path: PathBuf,
    saved: Vec<UiState>,
    _dir: TempDir,
}

impl Shell {
    fn open(text: &str, view: &str) -> Shell {
        let dir = TempDir::new("guides");
        let path = dir.join("g.dsl");
        std::fs::write(&path, text).expect("fixture written");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
            saved: Vec::new(),
            _dir: dir,
        };
        shell.event(Event::Command(Command::Open(path)));
        shell.event(Event::Command(Command::ShowView(ViewRef::Declared(view.to_owned()))));
        assert_eq!(shell.scene().key, view);
        shell
    }

    fn event(&mut self, event: Event) {
        let effects = self.s.handle(event);
        self.run(effects);
    }

    fn input(&mut self, input: Input) {
        self.event(Event::Input(input));
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
                Effect::SaveUiState(ui) => {
                    self.saved.push(ui);
                    Vec::new()
                }
                Effect::Watch(_) | Effect::Dialog(_) | Effect::Fullscreen(_) | Effect::OpenUrl(_) => Vec::new(),
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

    fn id(&self, name: &str) -> BoxId {
        let model = self.s.model().expect("parsed");
        self.scene().boxes.iter().find(|b| model.element(b.element).name.value == name).expect("drawn").id
    }

    /// The screen point of scene px `(x, y)`.
    fn screen(&self, x: i32, y: i32) -> ScreenPoint {
        self.s.camera.to_screen(Point::new(x, y))
    }

    /// Presses on the centre of `name` and moves by `(dx, dy)` scene px,
    /// not releasing.
    fn drag(&mut self, name: &str, dx: i32, dy: i32, mods: Modifiers) -> ScreenPoint {
        let c = self.scene().boxes[self.id(name).0 as usize].rect.centre();
        let from = self.screen(c.x, c.y);
        self.input(Input::PointerDown {
            at: from,
            button: Button::Left,
            mods,
        });
        let to = self.screen(c.x + dx, c.y + dy);
        self.input(Input::PointerMove { at: to });
        to
    }

    fn release(&mut self, at: ScreenPoint) {
        self.input(Input::PointerUp {
            at,
            button: Button::Left,
        });
    }

    fn guides(&self) -> Vec<Guide> {
        self.s.draw_list().guides.to_vec()
    }
}

fn field(ids: Ids) -> String {
    ids.workspace_in(FIELD, S_NESTED)
}

/// P5-500: the index holds the three lines per axis of every box not
/// dragged, sorted, and the dragged boxes' bounding rect.
#[test]
fn p5_500_index_of_the_boxes_not_dragged() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let sh = Shell::open(&field(ids), "g");
        let scene = sh.scene();
        let (a, b) = (sh.id("Alpha"), sh.id("Beta"));
        let dragged: BTreeMap<BoxId, Point> = [(b, scene.centres[b.0 as usize])].into();
        let index = GuideIndex::build(scene, &dragged).expect("an index");
        // At its start, b shares its row with c (not dragged, indexed):
        // three horizontal guides; a lines up with nothing.
        let mut out = Vec::new();
        index.guides((0, 0), &mut out);
        assert_eq!(out.len(), 3, "{out:?}");
        assert!(out.iter().all(|g| !g.vertical));
        // Moved by a's offset, b lines up with a on both axes.
        let (ca, cb) = (scene.centres[a.0 as usize], scene.centres[b.0 as usize]);
        index.guides((ca.x - cb.x, ca.y - cb.y), &mut out);
        assert_eq!(out.iter().filter(|g| g.vertical).count(), 3);
        assert_eq!(out.iter().filter(|g| !g.vertical).count(), 3);
        assert!(GuideIndex::build(scene, &BTreeMap::new()).is_none(), "no dragged box, no index");
    }
}

/// P5-501: zoomed out, a drag ending 10px from a line of another box is
/// pulled onto it (6 screen px tolerance, here 12 scene px), and the
/// release writes the aligned centre.
#[test]
fn p5_501_pull_onto_a_line_within_tolerance() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let mut sh = Shell::open(&field(ids), "g");
        sh.s.camera.zoom = 0.5;
        // b's left 480 goes to 90: 10px right of a's left, 80.
        let at = sh.drag("Beta", -390, 0, NONE);
        sh.release(at);
        assert!(sh.text().contains(&ids.refs_in("at b 200 600", S_NESTED)), "{}", sh.text());
    }
}

/// P5-501: at zoom 1 the tolerance is 6 scene px: 10px off is not pulled.
#[test]
fn p5_501_no_pull_beyond_tolerance() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let mut sh = Shell::open(&field(ids), "g");
        let at = sh.drag("Beta", -390, 0, NONE);
        sh.release(at);
        assert!(sh.text().contains(&ids.refs_in("at b 210 600", S_NESTED)), "{}", sh.text());
    }
}

/// Boxes for P5-502: `q` and `p` dragged together have a bounding rect
/// 80..730, centre 405 (a 5px step); `r` and `t` stand still.
const STEPS: &str = r#"workspace "G" {
    model {
        sys = softwareSystem "S" "Does things." {
            q = container "Q" "Dragged." "Rust"
            p = container "P" "Dragged." "Rust"
            r = container "R" "Still." "Rust"
            t = container "T" "Still." "Rust"
        }
    }
    views {
        container sys "g" "Steps" {
            include *
            at q 200 200
            at p 610 200
            at r 1500 800
            at t 1310 1400
        }
    }
}
"#;

/// P5-502: a pull is taken only when it is a whole number of 10px grid
/// steps: a line 5px away is skipped for one 10px away, and with no such
/// line nothing is pulled.
#[test]
fn p5_502_pull_keeps_the_grid() {
    let parsed = c4rizer_core::model::parse(STEPS).expect("parses");
    let scene = c4rizer_core::pipeline::build_view(&parsed.model, "g", &c4rizer_core::place::Keep::default(), &AtomicBool::new(false))
        .expect("builds")
        .scene;
    let named = |name: &str| {
        let b = scene.boxes.iter().find(|b| parsed.model.element(b.element).name.value == name).expect("drawn");
        (b.id, scene.centres[b.id.0 as usize])
    };
    let dragged: BTreeMap<BoxId, Point> = [named("Q"), named("P")].into();
    let index = GuideIndex::build(&scene, &dragged).expect("an index");
    // dx 110 steps: bounding lines 1180, 1505, 1830. R's centre 1500 is
    // 5px off (skipped); T's left 1190 is 10px off (taken).
    assert_eq!(index.snap((110, 0), 12.0, true), (1, 0));
    // dx 66 steps: lines 740, 1065, 1390; R's left 1380 is 10px off.
    assert_eq!(index.snap((66, 0), 12.0, true), (-1, 0));
    // dx 31 steps: lines 390, 715, 1040; nothing within 12px.
    assert_eq!(index.snap((31, 0), 12.0, true), (0, 0));
    // Never a pull past the tolerance, whatever the offset.
    for dx in 0..200 {
        let (px, _) = index.snap((dx, 0), 12.0, true);
        assert!(px.abs() <= 1, "dx {dx}: pull of {px} steps exceeds 12px");
    }
}

/// P5-503: on exact alignment the guides are drawn: one segment per
/// aligned line, spanning the dragged box and the aligned boxes plus
/// 12px at each end.
#[test]
fn p5_503_guides_on_exact_alignment() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let mut sh = Shell::open(&field(ids), "g");
        // b to (600, 200): its top, centre and bottom equal a's.
        sh.drag("Beta", 0, -400, NONE);
        let guides = sh.guides();
        let horizontal: Vec<&Guide> = guides.iter().filter(|g| !g.vertical).collect();
        assert_eq!(horizontal.len(), 3, "{guides:?}");
        assert!(guides.iter().all(|g| !g.vertical), "{guides:?}");
        let ys: Vec<f32> = horizontal.iter().map(|g| g.at).collect();
        assert_eq!(ys, vec![130.0, 200.0, 270.0]);
        for g in horizontal {
            assert!((g.from - (80.0 - GUIDE_OVERHANG_PX)).abs() < f32::EPSILON);
            assert!((g.to - (720.0 + GUIDE_OVERHANG_PX)).abs() < f32::EPSILON);
        }
    }
}

/// P5-504: a multi-box drag uses the bounding rect of the dragged boxes.
#[test]
fn p5_504_multi_box_drag_uses_the_bounding_rect() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let mut sh = Shell::open(&field(ids), "g");
        let b = sh.scene().boxes[sh.id("Beta").0 as usize].rect.centre();
        let c = sh.scene().boxes[sh.id("Gamma").0 as usize].rect.centre();
        let tap = |sh: &mut Shell, p: Point, mods: Modifiers| {
            let at = sh.screen(p.x, p.y);
            sh.input(Input::PointerDown { at, button: Button::Left, mods });
            sh.input(Input::PointerUp { at, button: Button::Left });
        };
        tap(&mut sh, b, NONE);
        tap(&mut sh, c, SHIFT);
        // Both boxes up by 400: the pair's top, centre and bottom equal
        // a's; vertical lines: b's left (480) is not a's (80).
        sh.drag("Beta", 0, -400, NONE);
        let guides = sh.guides();
        assert_eq!(guides.iter().filter(|g| !g.vertical).count(), 3, "{guides:?}");
        let g = guides.iter().find(|g| !g.vertical).expect("a guide");
        assert!((g.to - (1120.0 + GUIDE_OVERHANG_PX)).abs() < f32::EPSILON, "spans c too: {g:?}");
    }
}

/// P5-505: with "Snap to alignment guides" off, nothing is pulled but
/// guides still show on exact alignment; the setting is saved in the UI
/// state as `pref\tsnap-guides\toff` and read back.
#[test]
fn p5_505_preference_off_keeps_guides_drops_the_pull() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let mut sh = Shell::open(&field(ids), "g");
        sh.event(Event::Command(Command::SetSnapGuides(false)));
        let saved = sh.saved.last().expect("UI state saved").clone();
        assert!(saved.snap_guides_off);
        assert!(saved.to_text().contains("pref\tsnap-guides\toff\n"));
        assert_eq!(UiState::parse(&saved.to_text()), saved);
        sh.s.camera.zoom = 0.5;
        sh.drag("Beta", 0, -400, NONE);
        assert_eq!(sh.guides().len(), 3, "exact alignment still shows");
        let at = sh.screen(600 - 390, 200);
        sh.input(Input::PointerMove { at });
        sh.release(at);
        assert!(sh.text().contains(&ids.refs_in("at b 210 200", S_NESTED)), "{}", sh.text());
        sh.event(Event::Command(Command::SetSnapGuides(true)));
        assert!(!sh.saved.last().expect("saved").snap_guides_off);
        assert!(!sh.saved.last().expect("saved").to_text().contains("pref\t"), "on is the default, not written");
    }
    assert!(UiState::parse("pref\tsnap-guides\toff\n").snap_guides_off);
    assert!(!UiState::parse("pref\tsnap-guides\ton\n").snap_guides_off);
}

/// P5-506: guides clear on pointer up and on a cancelled drag.
#[test]
fn p5_506_guides_clear_on_release_and_cancel() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let mut sh = Shell::open(&field(ids), "g");
        let at = sh.drag("Beta", 0, -400, NONE);
        assert!(!sh.guides().is_empty());
        sh.release(at);
        assert!(sh.guides().is_empty());
        sh.drag("Beta", 400, 0, NONE);
        assert!(!sh.guides().is_empty(), "b lines up with c's x lines");
        sh.event(Event::Command(Command::ShowView(ViewRef::Declared("ctx".to_owned()))));
        assert!(sh.guides().is_empty());
    }
}

/// P5-507: guides are painted as `Source::Guide`, 1px solid paths in the
/// mark colour, after the marks and before the rubber band (EDIT-12).
#[test]
fn p5_507_guides_painted_as_marks() {
    let mut sh = Shell::open(&field(Ids::Flat), "g");
    sh.drag("Beta", 0, -400, NONE);
    let list = sh.s.draw_list();
    let items = paint_items(&list);
    let guides: Vec<_> = items.iter().filter(|i| i.source == Source::Guide).collect();
    assert_eq!(guides.len(), 3);
    for g in &guides {
        assert!(matches!(&g.primitive, Primitive::Path { points, width, dash: None, .. } if points.len() == 2 && (*width - 1.0).abs() < f32::EPSILON));
    }
    let last_mark = items.iter().rposition(|i| matches!(i.source, Source::Mark(..))).expect("marks");
    let first_guide = items.iter().position(|i| i.source == Source::Guide).expect("guides");
    assert!(first_guide > last_mark);
    assert!(items.iter().all(|i| i.source != Source::RubberBand));
}

// ---------------------------------------------------------------- P5-900

/// A scene of `n` boxes on a grid, built once.
fn grid_scene(n: usize) -> Scene {
    use std::fmt::Write as _;
    let mut s = String::from("workspace {\n    model {\n        s = softwareSystem \"S\" {\n");
    for i in 0..n {
        let _ = writeln!(s, "            c{i} = container \"C{i}\" \"Does {i}.\" \"Rust\"");
    }
    s.push_str("        }\n    }\n    views {\n        container s \"g\" \"Grid\" {\n            include *\n");
    for i in 0..n {
        let _ = writeln!(s, "            at c{i} {} {}", 200 + (i % 32) * 300, 200 + (i / 32) * 200);
    }
    s.push_str("        }\n    }\n}\n");
    let parsed = c4rizer_core::model::parse(&s).expect("parses");
    c4rizer_core::pipeline::build_view(&parsed.model, "g", &c4rizer_core::place::Keep::default(), &AtomicBool::new(false))
        .expect("builds")
        .scene
}

fn bench_index(n: usize, budgets: (u64, u64)) {
    let scene = grid_scene(n);
    let dragged: BTreeMap<BoxId, Point> = [(BoxId(0), scene.centres[0])].into();
    let input = format!("{n} boxes");
    bench(["P5-500", "guides", "index build", &input], budgets.0, || {
        std::hint::black_box(GuideIndex::build(&scene, &dragged));
    });
    let index = GuideIndex::build(&scene, &dragged).expect("an index");
    let mut out = Vec::with_capacity(6);
    let mut dx = 0;
    bench(["P5-501", "guides", "snap query + guides", &input], budgets.1, || {
        dx = (dx + 7) % 200;
        let pull = index.snap((dx, 3), 6.0, true);
        index.guides((dx + pull.0, 3 + pull.1), &mut out);
        std::hint::black_box(&out);
    });
}

#[test]
#[ignore = "timing: cargo test --release -- --ignored perf_p5"]
fn perf_p5_500_guides_40_boxes() {
    if debug_build("perf_p5_500_guides_40_boxes") {
        return;
    }
    bench_index(40, (50, 50));
}

#[test]
#[ignore = "timing: cargo test --release -- --ignored perf_p5"]
fn perf_p5_500_guides_1000_boxes() {
    if debug_build("perf_p5_500_guides_1000_boxes") {
        return;
    }
    bench_index(1000, (1_000, 50));
}

/// `drag_to` (one pointer move during a box drag) on the example's c3,
/// with the guide index and without it.
#[test]
#[ignore = "timing: cargo test --release -- --ignored perf_p5"]
fn perf_p5_501_drag_to_with_and_without_guides() {
    const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl"));
    if debug_build("perf_p5_501_drag_to_with_and_without_guides") {
        return;
    }
    for (guides, what, budget) in [(true, "drag_to with guides", 100), (false, "drag_to without guides", 100)] {
        let mut sh = Shell::open(EXAMPLE, "C3-serving");
        let b = sh.scene().boxes[0].rect.centre();
        let from = sh.screen(b.x, b.y);
        sh.s.handle(Event::Input(Input::PointerDown {
            at: from,
            button: Button::Left,
            mods: NONE,
        }));
        let _ = sh.s.handle(Event::Input(Input::PointerMove {
            at: ScreenPoint { x: from.x + 20.0, y: from.y },
        }));
        assert!(matches!(sh.s.gesture, Gesture::Dragging { .. }), "a box drag");
        if !guides {
            sh.s.drop_guide_index();
        }
        let mut step = 0.0_f32;
        bench(["P5-501", "guides", what, "example C3-serving"], budget, || {
            step = (step + 13.0) % 400.0;
            let effects = sh.s.handle(Event::Input(Input::PointerMove {
                at: ScreenPoint { x: from.x + 20.0 + step, y: from.y + step / 3.0 },
            }));
            std::hint::black_box(effects);
        });
    }
}
