//! End-to-end drag timing through the real `State` and the real `Worker`
//! thread (EDIT-25): pointer moves at a fixed rate on a multi-box
//! selection, the time from each move to its routed `Reply::Dragged`, how
//! many moves got one, the time from release to the final `Built`, and
//! the "drop, then drag another box at once" sequence. Numbers are printed
//! only; every test is `#[ignore]`d and named `perf_e2e_`:
//! `cargo test -p c4rizer-editor --release -- --ignored perf_e2e_ --nocapture --test-threads 1`.
// DECISION: printed measurements, no budget: the editor has no SRS
// number for these yet, and a budget on a shared machine would flake.
// DECISION: the report casts counts and offsets to floats and back and
// writes its waits in seconds or milliseconds as read; those pedantic
// lints are allowed for this file only.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::duration_suboptimal_units
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use c4rizer_core::geom::Point;
use c4rizer_core::scene::Scene;
use c4rizer_core::view::BoxId;

use crate::camera::ScreenPoint;
use crate::files::{self, FileEvent, UiState};
use crate::state::{Button, Command, Effect, Event, Input, Modifiers, State};
use crate::testutil::TempDir;
use crate::worker::{Generation, Job, Reply, ViewRef, Worker};

const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
};
const SHIFT: Modifiers = Modifiers { shift: true, ..NONE };

const GENERATED: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../core/tests/data/core_31_generated.dsl"));
/// Read at run time; skipped when missing or not parsing.
const DOCS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/workspace.dsl");

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// The shell with the real worker thread: effects run at once, work goes
/// to the thread, replies are handled as they arrive.
struct Live {
    s: State,
    worker: Worker,
    _dir: TempDir,
    /// When each drag request was sent, by generation.
    sent: BTreeMap<Generation, Instant>,
    /// When each drag reply was handled, by generation.
    dragged: BTreeMap<Generation, Instant>,
    /// When the last shown `Built` was handled.
    built: Option<Instant>,
}

impl Live {
    fn open(text: &str, view: &str) -> Option<Live> {
        let dir = TempDir::new("e2e");
        let path: PathBuf = dir.join("w.dsl");
        std::fs::write(&path, text).expect("fixture written");
        let mut live = Live {
            s: State::new(UiState::default()),
            worker: Worker::spawn(Arc::new(|| {})),
            _dir: dir,
            sent: BTreeMap::new(),
            dragged: BTreeMap::new(),
            built: None,
        };
        live.event(Event::Command(Command::Open(path)));
        live.until_built(Duration::from_secs(60));
        live.built = None;
        live.event(Event::Command(Command::ShowView(ViewRef::Declared(view.to_owned()))));
        live.until_built(Duration::from_secs(60));
        (live.s.scene().map(|s| s.key.clone()) == Some(view.to_owned())).then_some(live)
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
                Effect::Work(request) => {
                    if std::env::var("E2E_VERBOSE").is_ok() && !matches!(request.job, Job::Drag { .. }) {
                        let what = match &request.job {
                            Job::Build { view, shown, .. } => format!("Build {view:?} shown {shown}"),
                            _ => "other".to_owned(),
                        };
                        println!("  send {:?} {what}", request.generation);
                    }
                    if matches!(request.job, Job::Drag { .. }) {
                        self.sent.insert(request.generation, Instant::now());
                    }
                    self.worker.send(request);
                    Vec::new()
                }
                Effect::Watch(_) | Effect::Dialog(_) | Effect::SaveUiState(_) | Effect::Fullscreen(_) | Effect::OpenUrl(_) => Vec::new(),
            };
            self.run(next);
        }
    }

    /// Handles replies as they arrive until `deadline`.
    fn pump_until(&mut self, deadline: Instant) {
        loop {
            let now = Instant::now();
            if now >= deadline {
                return;
            }
            let Ok(reply) = self.worker.replies.recv_timeout(deadline - now) else { return };
            self.reply(reply);
        }
    }

    fn reply(&mut self, reply: Reply) {
        if std::env::var("E2E_VERBOSE").is_ok() {
            let what = match &reply {
                Reply::Built { generation, view, shown, .. } => format!("Built {generation:?} {view:?} shown {shown}"),
                Reply::Dragged { generation, .. } => format!("Dragged {generation:?}"),
                Reply::NoView { .. } => "NoView".to_owned(),
                Reply::Exported { .. } => "Exported".to_owned(),
            };
            println!("  reply {what}");
        }
        match &reply {
            Reply::Dragged { generation, .. } => {
                self.dragged.insert(*generation, Instant::now());
            }
            Reply::Built { shown: true, .. } => self.built = Some(Instant::now()),
            _ => {}
        }
        self.event(Event::Worker(reply));
    }

    /// Handles replies until a shown `Built` (or `limit`).
    fn until_built(&mut self, limit: Duration) {
        let end = Instant::now() + limit;
        while self.built.is_none() && Instant::now() < end {
            let Ok(reply) = self.worker.replies.recv_timeout(end - Instant::now()) else { return };
            self.reply(reply);
        }
    }

    fn scene(&self) -> &Scene {
        self.s.scene().expect("a scene")
    }

    fn screen_of(&self, b: BoxId, dx: i32, dy: i32) -> ScreenPoint {
        let c = self.scene().boxes[b.0 as usize].rect.centre();
        self.s.camera.to_screen(Point::new(c.x + dx, c.y + dy))
    }

    fn input(&mut self, input: Input) {
        self.event(Event::Input(input));
    }
}

/// The box with the most lines, and the `k` boxes nearest it (itself
/// first), as the core drag measurements pick them.
fn hub_cluster(scene: &Scene, k: usize) -> Vec<BoxId> {
    let degree = |b: BoxId| scene.lines.iter().filter(|l| l.source == b || l.destination == b).count();
    let mut hub = scene.boxes[0].id;
    for b in &scene.boxes {
        if degree(b.id) > degree(hub) {
            hub = b.id;
        }
    }
    let c = scene.centres[hub.0 as usize];
    let mut all: Vec<BoxId> = scene.boxes.iter().map(|b| b.id).collect();
    all.sort_by_key(|&b| {
        let p = scene.centres[b.0 as usize];
        (i64::from(p.x - c.x).pow(2) + i64::from(p.y - c.y).pow(2), b.0)
    });
    all.truncate(k);
    all
}

/// The snapped offset, scene px, of step `i` of a 40-step drag: 20 steps
/// of 20px right, then 20 of 20px down (as `drag_perf::path_offset`).
fn offset(i: i32) -> (i32, i32) {
    (20 * i.min(20), 20 * (i - 20).max(0))
}

fn report(name: &str, live: &Live, from: usize, steps: usize, release: Option<Instant>) {
    let gens: Vec<(&Generation, &Instant)> = live.sent.iter().skip(from).collect();
    let mut lat: Vec<f64> = gens.iter().filter_map(|(g, t)| live.dragged.get(g).map(|d| ms(d.saturating_duration_since(**t)))).collect();
    let got = lat.len();
    lat.sort_by(f64::total_cmp);
    let pick = |q: f64| lat.get(((lat.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(f64::NAN);
    let rel = match (release, live.built) {
        (Some(r), Some(b)) if b >= r => format!("{:.0} ms", ms(b - r)),
        _ => "none".to_owned(),
    };
    println!(
        "e2e {name:<40} {steps:>2} moves, {:>2} drag requests, {got:>2} routed replies ({:>2} superseded) | move->reply median {:>7.1} p90 {:>7.1} max {:>7.1} ms | release->Built {rel}",
        gens.len(),
        gens.len() - got,
        pick(0.5),
        pick(0.9),
        pick(1.0),
    );
}

/// A 40-step drag of `boxes` (grabbed by the first) with one move every
/// `every`, then the release, waiting for its `Built`.
fn drag(live: &mut Live, boxes: &[BoxId], every: Duration, name: &str) {
    // Select: click the first, Shift-click the rest.
    for (k, &b) in boxes.iter().enumerate() {
        let at = live.screen_of(b, 0, 0);
        let mods = if k == 0 { NONE } else { SHIFT };
        live.input(Input::PointerDown { at, button: Button::Left, mods });
        live.input(Input::PointerUp { at, button: Button::Left });
    }
    let grab = boxes[0];
    let from = live.sent.len();
    live.input(Input::PointerDown {
        at: live.screen_of(grab, 0, 0),
        button: Button::Left,
        mods: NONE,
    });
    let start = live.screen_of(grab, 0, 0);
    let zoom = live.s.camera.zoom;
    #[allow(clippy::cast_precision_loss, reason = "small offsets")]
    let at = |(dx, dy): (i32, i32)| ScreenPoint {
        x: start.x + dx as f32 * zoom,
        y: start.y + dy as f32 * zoom,
    };
    let mut next = Instant::now();
    for i in 1..=40 {
        live.input(Input::PointerMove { at: at(offset(i)) });
        next += every;
        live.pump_until(next);
    }
    live.built = None;
    let released = Instant::now();
    live.input(Input::PointerUp { at: at(offset(40)), button: Button::Left });
    live.until_built(Duration::from_secs(60));
    // Replies still in flight for the drag count as late, not lost.
    live.pump_until(Instant::now() + Duration::from_millis(1));
    report(name, live, from, 40, Some(released));
}

/// Drop a drag, then at once drag another box: the wait until the second
/// drag's first routed preview, and its release.
fn drop_then_drag(live: &mut Live, first: &[BoxId], second: BoxId, name: &str) {
    for (k, &b) in first.iter().enumerate() {
        let at = live.screen_of(b, 0, 0);
        let mods = if k == 0 { NONE } else { SHIFT };
        live.input(Input::PointerDown { at, button: Button::Left, mods });
        live.input(Input::PointerUp { at, button: Button::Left });
    }
    let start = live.screen_of(first[0], 0, 0);
    let zoom = live.s.camera.zoom;
    #[allow(clippy::cast_precision_loss, reason = "small offsets")]
    let shift = |p: ScreenPoint, dx: i32, dy: i32| ScreenPoint {
        x: p.x + dx as f32 * zoom,
        y: p.y + dy as f32 * zoom,
    };
    live.input(Input::PointerDown { at: start, button: Button::Left, mods: NONE });
    for i in 1..=5 {
        live.input(Input::PointerMove { at: shift(start, 20 * i, 0) });
        live.pump_until(Instant::now() + Duration::from_millis(16));
    }
    live.built = None;
    let released = Instant::now();
    live.input(Input::PointerUp {
        at: shift(start, 100, 0),
        button: Button::Left,
    });
    // At once: grab another box and move it every 16 ms.
    let from = live.sent.len();
    let other = live.screen_of(second, 0, 0);
    live.input(Input::PointerDown { at: other, button: Button::Left, mods: NONE });
    let began = Instant::now();
    let mut first_reply = None;
    for i in 1..=20 {
        live.input(Input::PointerMove { at: shift(other, 0, 20 * i) });
        live.pump_until(Instant::now() + Duration::from_millis(16));
        if first_reply.is_none() && live.sent.iter().skip(from).any(|(g, _)| live.dragged.contains_key(g)) {
            first_reply = Some(began.elapsed());
        }
    }
    let up = Instant::now();
    live.input(Input::PointerUp {
        at: shift(other, 0, 400),
        button: Button::Left,
    });
    live.built = None;
    live.until_built(Duration::from_secs(60));
    println!(
        "e2e {name:<40} first drop->first Built {} | second drag: first routed preview after {} | second release->Built {:.0} ms",
        if live.built.is_some() { format!("{:.0} ms (after the second release)", ms(live.built.unwrap_or(up) - released)) } else { "none".into() },
        first_reply.map_or_else(|| "none in 20 moves".to_owned(), |d| format!("{:.0} ms", ms(d))),
        live.built.map_or(f64::NAN, |b| ms(b - up)),
    );
    report(&format!("{name} (second drag)"), live, from, 20, Some(up));
}

fn views() -> Vec<(&'static str, String, &'static str)> {
    let mut out = vec![("generated", GENERATED.to_owned(), "generated")];
    if let Ok(text) = std::fs::read_to_string(DOCS) {
        out.push(("C3-entry", text, "C3-entry"));
    }
    out
}

fn debug_build(test: &str) -> bool {
    if cfg!(debug_assertions) {
        println!("{test}: skipped, timing needs `cargo test --release`");
        return true;
    }
    false
}

/// EDIT-25 end to end: an 8-box drag on the generated view and C3-entry,
/// one snapped step every 16 ms and every 50 ms.
#[test]
#[ignore = "perf: end-to-end drag measurement, release only"]
fn perf_e2e_multi_box_drag() {
    if debug_build("perf_e2e_multi_box_drag") {
        return;
    }
    for (name, text, view) in views() {
        for every in [16, 50] {
            let Some(mut live) = Live::open(&text, view) else {
                println!("e2e {name}: does not open, skipped");
                continue;
            };
            let boxes = hub_cluster(live.scene(), 8);
            drag(&mut live, &boxes, Duration::from_millis(every), &format!("{name} 8-box, a move every {every} ms"));
            let boxes = hub_cluster(live.scene(), 4);
            drag(&mut live, &boxes, Duration::from_millis(every), &format!("{name} 4-box, a move every {every} ms"));
        }
    }
}

/// EDIT-25 end to end: drop a 4-box drag, then at once drag another box
/// (the original intermittent lag).
#[test]
#[ignore = "perf: end-to-end drag measurement, release only"]
fn perf_e2e_drop_then_drag() {
    if debug_build("perf_e2e_drop_then_drag") {
        return;
    }
    for (name, text, view) in views() {
        let Some(mut live) = Live::open(&text, view) else {
            continue;
        };
        let cluster = hub_cluster(live.scene(), 4);
        let other = live.scene().boxes.iter().map(|b| b.id).rfind(|b| !cluster.contains(b)).expect("another box");
        drop_then_drag(&mut live, &cluster, other, &format!("{name} drop, then drag"));
    }
}
