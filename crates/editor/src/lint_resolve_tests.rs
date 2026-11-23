//! Tests of lint resolve, editor part (SRS 4.9 lint-resolve,
//! P5-406..P5-409), driven headlessly like `bundle_tests`: effects run
//! synchronously with `files` and `worker::serve`; every test that names
//! an element runs for both `Ids` modes.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use c4rizer_core::lint::Rule;

use crate::files::{self, FileEvent, UiState, WriteRequest};
use crate::state::{Command, Effect, Event, State};
use crate::testutil::{Ids, S_NESTED, TempDir};
use crate::worker;

/// Three containers: `a` and `b` 40px apart (L6), `c` with no `at` line
/// (auto-placed, so a resolve is an EDIT-26 first write); `{lines}` is
/// replaced by extra view lines.
const CLOSE: &str = r#"workspace "R" {
    model {
        s = softwareSystem "S" "Does things." {
            a = container "Alpha" "First one." "Rust"
            b = container "Beta" "Second one." "Rust"
            c = container "Gamma" "Third one." "Rust"
            d = container "Delta" "Fourth one, with a description far too long for a tiny box." "Rust"
        }
    }
    views {
        container s "v" "View" {
            include *
            at a 200 200
            at b 480 200
            at d 1400 900
{lines}
        }
    }
}
"#;

fn fixture(ids: Ids, lines: &[&str]) -> String {
    let text: String = lines.iter().fold(String::new(), |mut out, v| {
        out.push_str("            ");
        out.push_str(v);
        out.push('\n');
        out
    });
    ids.workspace_in(&CLOSE.replace("{lines}\n", &text), S_NESTED)
}

/// The shell without a window: every effect at once, writes held when
/// asked.
struct Shell {
    s: State,
    path: PathBuf,
    hold: bool,
    held: Vec<WriteRequest>,
    _dir: TempDir,
}

impl Shell {
    fn open(text: &str) -> Shell {
        let dir = TempDir::new("lint-resolve");
        let path = dir.join("r.dsl");
        std::fs::write(&path, text).expect("fixture written");
        let mut shell = Shell {
            s: State::new(UiState::default()),
            path: path.clone(),
            hold: false,
            held: Vec::new(),
            _dir: dir,
        };
        shell.cmd(Command::Open(path));
        assert_eq!(shell.s.scene().expect("a scene").key, "v");
        shell
    }

    fn cmd(&mut self, command: Command) {
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
                Effect::Write(request) if self.hold => {
                    self.held.push(request);
                    Vec::new()
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

    /// Index in `State::findings` of the first finding of `rule`, on the
    /// box named `name` when given.
    fn finding(&self, rule: Rule, name: Option<&str>) -> Option<usize> {
        let model = self.s.model().expect("parsed");
        self.s.findings().iter().position(|f| {
            f.rule == rule
                && name.is_none_or(|n| {
                    f.subjects.iter().any(|m| {
                        matches!(m, c4rizer_core::scene::ModelId::Element(e) if model.element(*e).name.value == n)
                    })
                })
        })
    }

    fn count(&self, rule: Rule) -> usize {
        self.s.findings().iter().filter(|f| f.rule == rule).count()
    }
}

/// P5-406: when a build is shown, its L5, L6 and L25 (box) findings
/// have a resolution with its label and the others none.
#[test]
fn p5_406_resolutions_are_known_when_the_report_arrives() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let sh = Shell::open(&fixture(ids, &[&ids.refs_in("size d 100 60", S_NESTED)]));
        let l6 = sh.finding(Rule::TooClose, None).expect("an L6");
        assert_eq!(sh.s.resolution(l6).map(|r| r.label.as_str()), Some("Move apart"));
        let l25 = sh.finding(Rule::TextFit, Some("Delta")).expect("an L25");
        assert_eq!(sh.s.resolution(l25).map(|r| r.label.as_str()), Some("Fit text"));
        let others: Vec<usize> = (0..sh.s.findings().len())
            .filter(|&i| !matches!(sh.s.findings()[i].rule, Rule::TooClose | Rule::OffGrid | Rule::TextFit))
            .collect();
        assert!(others.iter().all(|&i| sh.s.resolution(i).is_none()));
        let want = (0..sh.s.findings().len()).filter(|&i| sh.s.resolution(i).is_some()).count();
        assert_eq!(sh.s.resolvable(), want);
        assert!(want >= 2);
    }
}

/// P5-407: Resolve writes the moves as one undo step through the move
/// path: the first write gives every box an `at` line (EDIT-26), the L6
/// is gone from the new build, and Undo restores the file byte for byte.
#[test]
fn p5_407_resolve_moves_is_one_undo_step() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let text = fixture(ids, &[]);
        let mut sh = Shell::open(&text);
        let l6 = sh.finding(Rule::TooClose, Some("Alpha")).expect("an L6");
        sh.cmd(Command::Resolve(l6));
        let after = sh.text();
        assert_ne!(after, text);
        assert!(after.contains(&ids.refs_in("at c ", S_NESTED)), "first write adds c's at line:\n{after}");
        assert!(after.contains(&ids.refs_in("at a 190 200", S_NESTED)), "{after}");
        assert!(after.contains(&ids.refs_in("at b 490 200", S_NESTED)), "{after}");
        assert_eq!(sh.count(Rule::TooClose), 0, "{:?}", sh.s.findings());
        sh.cmd(Command::Undo);
        assert_eq!(sh.text(), text);
    }
}

/// P5-407: a resolve with a size (L25) writes the `size` line in the same
/// way, one undo step.
#[test]
fn p5_407_resolve_size_is_one_undo_step() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let size = ids.refs_in("size d 100 60", S_NESTED);
        let text = fixture(ids, &[&ids.refs_in("at c 200 700", S_NESTED), &size]);
        let mut sh = Shell::open(&text);
        let l25 = sh.finding(Rule::TextFit, Some("Delta")).expect("an L25");
        let r = sh.s.resolution(l25).cloned().expect("resolvable");
        sh.cmd(Command::Resolve(l25));
        let after = sh.text();
        let (w, h) = r.sizes[0].1;
        assert!(after.contains(&ids.refs_in(&format!("size d {w} {h}"), S_NESTED)), "{after}");
        assert_eq!(sh.finding(Rule::TextFit, Some("Delta")), None);
        sh.cmd(Command::Undo);
        assert_eq!(sh.text(), text);
    }
}

/// P5-408: Resolve all writes every resolution at once, one undo step;
/// the new build has no L5 and no L6.
#[test]
fn p5_408_resolve_all_is_one_undo_step() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let text = fixture(ids, &[&ids.refs_in("at c 755 205", S_NESTED)]);
        let mut sh = Shell::open(&text);
        assert!(sh.count(Rule::TooClose) >= 1 && sh.count(Rule::OffGrid) == 1);
        assert!(sh.s.resolvable() >= 2);
        sh.cmd(Command::ResolveAll);
        assert_ne!(sh.text(), text);
        assert_eq!(sh.count(Rule::TooClose) + sh.count(Rule::OffGrid), 0, "{:?}", sh.s.findings());
        sh.cmd(Command::Undo);
        assert_eq!(sh.text(), text);
    }
}

/// P5-409: a resolve refused by the write path (here: the previous write
/// still in flight) shows the usual notice and writes nothing.
#[test]
fn p5_409_refused_resolve_shows_the_notice() {
    for ids in [Ids::Flat, Ids::Hierarchical] {
        let text = fixture(ids, &[]);
        let mut sh = Shell::open(&text);
        let l6 = sh.finding(Rule::TooClose, None).expect("an L6");
        sh.hold = true;
        sh.cmd(Command::Resolve(l6));
        assert_eq!(sh.held.len(), 1);
        sh.cmd(Command::Resolve(l6));
        assert_eq!(sh.held.len(), 1, "the second resolve wrote");
        let notice = sh.s.notice.clone().unwrap_or_default();
        assert!(notice.contains("still being written"), "{notice}");
    }
}
