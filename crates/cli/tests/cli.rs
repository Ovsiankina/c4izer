//! End-to-end tests of the `c4rizer` binary on `tests/data/example.dsl`
//! (always addressed as
//! `concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl")`:
//! cargo runs integration tests with cwd `crates/cli`), run through `std::process::Command` on `env!("CARGO_BIN_EXE_c4rizer")`
//! (plan cli-03): CLI-3, CLI-4, CLI-5.
//!
//! Golden check 1 of CLI-5 (structure against the prototype SVGs) is
//! core's `golden_example_structure_matches_reference`
//! (crates/core/tests/golden.rs); golden check 2 (byte-identical against
//! c4rizer's own approved SVGs in `tests/data/golden/`, i.e.
//! `concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/golden")`) is
//! here.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const EXAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl");
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/golden");
const KEYS: [&str; 4] = ["C1", "C2", "C3-serving", "C3-startup"];

/// Two boxes on the grid, one labelled line: lints clean.
const CLEAN: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods."
    operator -> shop "Places orders in"
  }
  views {
    systemContext shop "ctx" "Shop context" {
      include *
      at operator 200 150
      at shop 200 450
    }
  }
}
"#;

/// A fresh, empty directory under the system temp dir, removed on drop.
// DECISION: hand-made (pid, counter, test name); no dev-dependency.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("c4rizer-cli-e2e-{}-{n}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        Scratch(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).expect("fixture written");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs the binary in `cwd` with `args`.
fn c4rizer(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_c4rizer"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 stdout")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("UTF-8 stderr")
}

/// The sorted file names in `dir`.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("dir readable")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Renders the example into `out` and returns the run.
fn render_example(cwd: &Path, out: &Path, extra: &[&str]) -> Output {
    let out = out.to_str().expect("UTF-8 temp path");
    let mut args = vec!["render", EXAMPLE, "-o", out];
    args.extend_from_slice(extra);
    c4rizer(cwd, &args)
}

/// CLI-3: `c4rizer lint <example.dsl>`, run in an empty temporary
/// directory, prints the same report as `render` minus the `wrote` lines,
/// exits with the same code, leaves the directory empty, and leaves
/// FILE's bytes unchanged.
#[test]
fn cli_3_lint_writes_nothing() {
    let before = fs::read(EXAMPLE).expect("example readable");
    let cwd = Scratch::new("lint");
    let lint = c4rizer(cwd.path(), &["lint", EXAMPLE]);
    assert!(names(cwd.path()).is_empty(), "lint wrote files");
    assert_eq!(fs::read(EXAMPLE).unwrap(), before, "FILE changed");
    assert!(stderr(&lint).is_empty(), "{}", stderr(&lint));

    let other = Scratch::new("lint-render");
    let render = render_example(other.path(), &other.path().join("out"), &[]);
    let without_wrote: String = stdout(&render)
        .split_inclusive('\n')
        .filter(|l| !l.starts_with("   wrote "))
        .collect();
    assert_eq!(stdout(&lint), without_wrote);
    assert!(stdout(&lint).starts_with("== C1: "), "{}", stdout(&lint));
    assert_eq!(lint.status.code(), render.status.code());
    assert!(matches!(lint.status.code(), Some(0 | 1)));
}

/// The `width="N"` of an SVG's root element.
#[cfg(feature = "png")]
fn svg_width(svg: &str) -> u32 {
    let rest = &svg[svg.find(" width=\"").expect("width attribute") + 8..];
    rest[..rest.find('"').unwrap()]
        .parse()
        .expect("integer width")
}

/// The width in a PNG's IHDR chunk, after checking the signature.
#[cfg(feature = "png")]
fn png_width(png: &[u8]) -> u32 {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");
    assert_eq!(&png[12..16], b"IHDR");
    u32::from_be_bytes(png[16..20].try_into().unwrap())
}

/// CLI-4: `render ... -o DIR --png` writes `<view key>.png` beside each
/// `<view key>.svg`, a valid PNG twice the SVG width; with `--width 400`
/// it is 400px wide.
// Needs this crate's `png` feature (`cargo test -p c4rizer-cli --features
// png`); without it `--png` exits 2 (`png_without_feature_exits_2`).
#[cfg(feature = "png")]
#[test]
fn cli_4_png_written() {
    let dir = Scratch::new("png");
    let out = dir.path().join("out");
    let run = render_example(dir.path(), &out, &["--png"]);
    assert!(matches!(run.status.code(), Some(0 | 1)), "{}", stderr(&run));
    for key in KEYS {
        let svg = fs::read_to_string(out.join(format!("{key}.svg"))).unwrap();
        let png = fs::read(out.join(format!("{key}.png"))).expect("PNG written");
        assert_eq!(png_width(&png), 2 * svg_width(&svg), "{key}");
        assert!(stdout(&run).contains(&format!(
            "   wrote {}\n",
            out.join(format!("{key}.png")).display()
        )));
    }

    let narrow = dir.path().join("narrow");
    let run = render_example(dir.path(), &narrow, &["--png", "--width", "400"]);
    assert!(matches!(run.status.code(), Some(0 | 1)), "{}", stderr(&run));
    for key in KEYS {
        let png = fs::read(narrow.join(format!("{key}.png"))).expect("PNG written");
        assert_eq!(png_width(&png), 400, "{key}");
    }
}

/// CLI-5 (2): `render <example.dsl> -o DIR` writes the four example
/// views' SVGs byte-identical to `tests/data/golden/<key>.svg`. Check (1)
/// is core's golden test (header).
///
/// Bless mechanism: with `C4RIZER_BLESS=1` in the environment the test
/// copies each rendered SVG to `tests/data/golden/<key>.svg` (creating
/// the directory) and passes; without it, a missing golden file fails
/// with a message saying to run `C4RIZER_BLESS=1 cargo test -p
/// c4rizer-cli cli_5_example_golden`. Blessed files are not approved
/// until the owner accepts them in the step-6 PR diff (CLI-5: "approved
/// once by the owner"); a deliberate change re-blesses and the PR shows
/// the diff.
// DECISION: bless via an environment variable (the usual snapshot-test
// pattern, no new dependency); approval stays the owner's, in the PR.
#[test]
fn cli_5_example_golden() {
    let dir = Scratch::new("golden");
    let out = dir.path().join("out");
    let run = render_example(dir.path(), &out, &[]);
    assert!(matches!(run.status.code(), Some(0 | 1)), "{}", stderr(&run));
    let golden = Path::new(GOLDEN);
    let bless = std::env::var_os("C4RIZER_BLESS").is_some_and(|v| v == "1");
    if bless {
        fs::create_dir_all(golden).expect("golden dir");
    }
    for key in KEYS {
        let name = format!("{key}.svg");
        let got = fs::read(out.join(&name)).expect("SVG written");
        let path = golden.join(&name);
        if bless {
            fs::write(&path, &got).expect("golden written");
            continue;
        }
        let Ok(want) = fs::read(&path) else {
            panic!(
                "{} is missing — run `C4RIZER_BLESS=1 cargo test -p c4rizer-cli cli_5_example_golden` and have the owner approve the files",
                path.display()
            );
        };
        assert!(
            got == want,
            "{name} differs from {} — if the change is deliberate, re-bless with C4RIZER_BLESS=1 and show the diff in the PR",
            path.display()
        );
    }
}

/// CLI-1: `render` writes exactly one `<view key>.svg` per view of the
/// file and nothing else into DIR, prints one `wrote <path>` line per
/// file, and leaves FILE's bytes unchanged.
#[test]
fn cli_1_render_writes_one_svg_per_view() {
    let before = fs::read(EXAMPLE).expect("example readable");
    let dir = Scratch::new("render");
    let out = dir.path().join("out");
    let run = render_example(dir.path(), &out, &[]);
    assert!(matches!(run.status.code(), Some(0 | 1)), "{}", stderr(&run));
    let mut want: Vec<String> = KEYS.iter().map(|k| format!("{k}.svg")).collect();
    want.sort();
    assert_eq!(names(&out), want);
    let wrote: Vec<String> = stdout(&run)
        .lines()
        .filter_map(|l| l.strip_prefix("   wrote "))
        .map(str::to_owned)
        .collect();
    let paths: Vec<String> = KEYS
        .iter()
        .map(|k| out.join(format!("{k}.svg")).display().to_string())
        .collect();
    assert_eq!(wrote, paths, "one wrote line per file, in view order");
    for key in KEYS {
        let svg = fs::read_to_string(out.join(format!("{key}.svg"))).unwrap();
        assert!(svg.starts_with("<svg"), "{key}");
    }
    assert_eq!(fs::read(EXAMPLE).unwrap(), before, "FILE changed");
}

/// CLI-2 end to end: the binary exits 0 on a file whose views lint clean,
/// 1 on one with a lint error (inline fixtures written to a temporary
/// directory), and 2 on a missing FILE, on `render` without `-o` and on
/// an unknown flag (usage on stderr, nothing on stdout).
#[test]
fn cli_2_binary_exit_codes() {
    let dir = Scratch::new("exit");
    let clean = dir.file("clean.dsl", CLEAN);
    let vague = dir.file(
        "vague.dsl",
        &CLEAN.replace("\"Places orders in\"", "\"uses\""),
    );
    let clean = clean.to_str().unwrap();
    let vague = vague.to_str().unwrap();

    let run = c4rizer(dir.path(), &["lint", clean]);
    assert_eq!(run.status.code(), Some(0), "{}", stdout(&run));
    let run = c4rizer(dir.path(), &["lint", vague]);
    assert_eq!(run.status.code(), Some(1), "{}", stdout(&run));
    assert!(stdout(&run).contains("   ERROR: "));

    let run = c4rizer(dir.path(), &["lint", "absent.dsl"]);
    assert_eq!(run.status.code(), Some(2));
    assert!(
        stderr(&run).starts_with("c4rizer: absent.dsl: "),
        "{}",
        stderr(&run)
    );
    assert!(stdout(&run).is_empty());

    for args in [
        &["render", clean][..],
        &["lint", clean, "--pdf"][..],
        &[][..],
    ] {
        let run = c4rizer(dir.path(), args);
        assert_eq!(run.status.code(), Some(2), "{args:?}");
        assert!(stdout(&run).is_empty(), "{args:?}");
        let err = stderr(&run);
        assert!(err.starts_with("c4rizer: "), "{err}");
        assert!(err.contains("usage: c4rizer render FILE -o DIR"), "{err}");
    }
}
