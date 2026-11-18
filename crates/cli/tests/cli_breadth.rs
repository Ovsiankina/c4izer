//! End-to-end tests of the cli-breadth commands on the `c4rizer` binary:
//! `validate`, `inspect`, `list`, `version`, `help` (P2-600..P2-650) and
//! `render --svg-text` (P2-651..P2-662).

// DECISION: a file of its own beside `cli.rs`, as the other phase-2 rows
// did (`dsl_breadth.rs`, `styles_themes.rs`), so parallel rows never edit
// one test file.

use std::fs;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

const EXAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl");
const EXAMPLE_WS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/data/example/workspace.dsl"
);
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/golden");

/// The example's view keys (CLI-5 goldens).
const KEYS: [&str; 4] = ["C1", "C2", "C3-serving", "C3-startup"];

/// Renders the example into a fresh `out` directory under `dir` with
/// `extra` flags.
fn render_example(dir: &Scratch, out: &str, extra: &[&str]) -> (Output, PathBuf) {
    let out_dir = dir.path().join(out);
    let mut args = vec!["render", EXAMPLE, "-o", out_dir.to_str().unwrap()];
    args.extend_from_slice(extra);
    (run_in(dir.path(), &args, ""), out_dir)
}

/// The seven subcommands (P2-648, P2-650).
const SEVEN: [&str; 7] = ["render", "lint", "validate", "inspect", "list", "version", "help"];

/// Two boxes, one labelled line, every description and technology set,
/// one view holding both: lints clean, and its only inspection
/// violations are the two missing technologies.
const CLEAN: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods."
    operator -> shop "Places orders in"
  }
  views {
    systemContext shop "ctx" "Shop context" {
      include *
      technology off
      at operator 200 150
      at shop 200 450
    }
  }
}
"#;

/// `CLEAN` with a stop-word label: lint error L13, still parses.
fn vague() -> String {
    CLEAN.replace("\"Places orders in\"", "\"uses\"")
}

/// An unknown construct on line 2.
const BROKEN: &str = "workspace {\n  bogus\n}\n";

/// A fresh, empty directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "c4rizer-cli-breadth-{}-{n}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        Scratch(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn file(&self, name: &str, text: &str) -> String {
        let path = self.0.join(name);
        fs::write(&path, text).expect("fixture written");
        path.to_str().expect("UTF-8 temp path").to_owned()
    }

    /// The sorted file names in the directory.
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.0)
            .expect("dir readable")
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs the binary in `cwd` with `args` and `stdin` piped in.
fn run_in(cwd: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_c4rizer"))
        .current_dir(cwd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("binary runs");
    // A command that never reads stdin may close it first.
    let _ = child
        .stdin
        .take()
        .expect("stdin piped")
        .write_all(stdin.as_bytes());
    child.wait_with_output().expect("binary finishes")
}

/// Runs the binary in a fresh scratch directory with an empty stdin.
fn run(args: &[&str]) -> Output {
    let cwd = Scratch::new("run");
    run_in(cwd.path(), args, "")
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exit code")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 stdout")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("UTF-8 stderr")
}

/// `CLEAN` with the relationship's technology set: no violation at all.
fn spotless() -> String {
    CLEAN.replace("\"Places orders in\"", "\"Places orders in\" \"HTTPS\"")
}

/// P2-649: a usage error exits 2 (255 for `inspect`) and prints
/// `c4rizer: <error>` and then the usage text on stderr, nothing on
/// stdout.
fn assert_usage_error(output: &Output, want_code: i32, what: &str) {
    let err = stderr(output);
    assert_eq!(code(output), want_code, "{what}: {err}");
    assert!(stdout(output).is_empty(), "{what}: {}", stdout(output));
    assert!(err.starts_with("c4rizer: "), "{what}: {err}");
    let (first, rest) = err.split_once('\n').expect("two parts");
    assert!(!first.contains("usage:"), "{what}: {err}");
    assert!(rest.starts_with("usage: c4rizer render"), "{what}: {err}");
}

#[test]
fn p2_645_version_prints_package_version() {
    let out = run(&["version"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stderr(&out).is_empty());
    let want = format!("c4rizer {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(stdout(&out), want);
}

#[test]
fn p2_646_version_extra_argument_is_usage_error() {
    for extra in [&["version", "x"][..], &["version", "--png"][..]] {
        assert_usage_error(&run(extra), 2, &format!("{extra:?}"));
    }
}

#[test]
fn p2_647_help_prints_usage_to_stdout_exit_0() {
    let out = run(&["help"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stderr(&out).is_empty());
    let text = stdout(&out);
    assert!(text.starts_with("usage: c4rizer render"), "{text}");
    // The same text a usage error prints after its first line.
    let usage = stderr(&run(&["bogus"]));
    assert_eq!(usage.split_once('\n').unwrap().1, text);
}

#[test]
fn p2_647_help_command_and_dash_h() {
    let full = stdout(&run(&["help"]));
    for flag in ["-h", "--help"] {
        let out = run(&[flag]);
        assert_eq!(code(&out), 0, "{flag}");
        assert_eq!(stdout(&out), full, "{flag}");
    }
    for command in SEVEN {
        let out = run(&["help", command]);
        assert_eq!(code(&out), 0, "{command}: {}", stderr(&out));
        let text = stdout(&out);
        assert!(text.starts_with("usage: c4rizer "), "{text}");
        assert!(text.contains(&format!("c4rizer {command}")), "{text}");
        for other in SEVEN.iter().filter(|c| **c != command) {
            assert!(!text.contains(&format!("c4rizer {other} ")), "{command}: {text}");
            assert!(!text.contains(&format!("c4rizer {other}\n")), "{command}: {text}");
        }
    }
    assert_usage_error(&run(&["help", "draw"]), 2, "help draw");
    assert_usage_error(&run(&["help", "lint", "list"]), 2, "help lint list");
}

#[test]
fn p2_648_usage_names_every_subcommand_and_flag() {
    let text = stdout(&run(&["help"]));
    for command in SEVEN {
        assert!(text.contains(&format!("c4rizer {command}")), "{command}: {text}");
    }
    for flag in [
        "-o DIR",
        "--png",
        "--width N",
        "--mode light|dark",
        "--svg-text name|embed|outline",
        "-workspace FILE",
        "-severity",
        "error,warning,info,ignore",
        "[COMMAND]",
    ] {
        assert!(text.contains(flag), "{flag}: {text}");
    }
}

#[test]
fn p2_649_usage_error_prints_error_then_usage_exit_2() {
    for args in [
        &["render", EXAMPLE][..],
        &["lint", EXAMPLE, "--pdf"][..],
        &["validate"][..],
        &["list", "-workspace"][..],
        &["draw"][..],
    ] {
        assert_usage_error(&run(args), 2, &format!("{args:?}"));
    }
}

#[test]
fn p2_650_no_and_unknown_subcommand_name_all_seven() {
    for args in [&[][..], &["draw"][..]] {
        let out = run(args);
        assert_usage_error(&out, 2, &format!("{args:?}"));
        let first = stderr(&out).lines().next().unwrap().to_owned();
        for command in SEVEN {
            assert!(first.contains(command), "{command}: {first}");
        }
    }
}

#[test]
fn p2_600_workspace_flag_names_file() {
    let dir = Scratch::new("ws");
    let file = dir.file("clean.dsl", CLEAN);
    for command in ["validate", "list"] {
        // `-workspace` as the parity table spells it, Structurizr's `-w`
        // and `--workspace`, and a bare FILE (B cli-breadth 2).
        for args in [
            vec![command, "-workspace", file.as_str()],
            vec![command, "-w", file.as_str()],
            vec![command, "--workspace", file.as_str()],
            vec![command, file.as_str()],
        ] {
            let out = run(&args);
            assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
            assert!(stderr(&out).is_empty(), "{args:?}: {}", stderr(&out));
        }
    }
    let out = run(&["inspect", "-workspace", file.as_str()]);
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    assert_ne!(code(&out), 255);
    // The file named is the file read: a missing one is an I/O error.
    let missing = dir.path().join("absent.dsl");
    let out = run(&["validate", "-workspace", missing.to_str().unwrap()]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("absent.dsl"), "{}", stderr(&out));
}

#[test]
fn p2_601_workspace_dash_reads_stdin() {
    let cwd = Scratch::new("stdin");
    let out = run_in(cwd.path(), &["validate", "-workspace", "-"], CLEAN);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = run_in(cwd.path(), &["list", "-workspace", "-"], CLEAN);
    assert!(stdout(&out).contains("operator"), "{}", stdout(&out));
    let out = run_in(cwd.path(), &["validate", "-workspace", "-"], BROKEN);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).starts_with("c4rizer: <stdin>:2:3: "), "{}", stderr(&out));
}

#[test]
fn p2_602_missing_workspace_is_usage_error() {
    for (command, want) in [("validate", 2), ("list", 2), ("inspect", 255)] {
        let out = run(&[command]);
        assert_usage_error(&out, want, command);
        assert!(stderr(&out).contains("-workspace"), "{}", stderr(&out));
    }
}

#[test]
fn p2_602_workspace_without_value_is_usage_error() {
    for (command, want) in [("validate", 2), ("list", 2), ("inspect", 255)] {
        for flag in ["-workspace", "-w", "--workspace"] {
            let out = run(&[command, flag]);
            assert_usage_error(&out, want, &format!("{command} {flag}"));
        }
    }
}

#[test]
fn p2_602_workspace_repeated_is_usage_error() {
    for (command, want) in [("validate", 2), ("list", 2), ("inspect", 255)] {
        for args in [
            vec![command, "-workspace", "a.dsl", "-workspace", "b.dsl"],
            vec![command, "-workspace", "a.dsl", "-w", "a.dsl"],
            vec![command, "-workspace", "a.dsl", "b.dsl"],
            vec![command, "a.dsl", "-workspace", "b.dsl"],
        ] {
            assert_usage_error(&run(&args), want, &format!("{args:?}"));
        }
    }
}

#[test]
fn p2_602_unknown_argument_is_usage_error() {
    for (command, want) in [("validate", 2), ("list", 2), ("inspect", 255)] {
        for flag in ["--png", "-o", "--mode", "--svg-text", "-x"] {
            let out = run(&[command, "-workspace", "a.dsl", flag, "v"]);
            assert_usage_error(&out, want, &format!("{command} {flag}"));
        }
    }
}

#[test]
fn p2_603_url_is_usage_error_exit_2() {
    for url in ["https://example.com/workspace.dsl", "HTTP://example.com/w.dsl"] {
        let out = run(&["validate", "-workspace", url]);
        assert_usage_error(&out, 2, url);
        assert!(stderr(&out).contains(url), "{}", stderr(&out));
    }
    assert_usage_error(
        &run(&["inspect", "-workspace", "https://example.com/w.dsl"]),
        255,
        "inspect URL",
    );
}

#[test]
fn p2_604_validate_clean_exits_0() {
    for file in [EXAMPLE, EXAMPLE_WS] {
        let out = run(&["validate", "-workspace", file]);
        assert_eq!(code(&out), 0, "{file}: {}", stderr(&out));
        // B cli-breadth 4: nothing printed on success.
        assert!(stdout(&out).is_empty(), "{}", stdout(&out));
        assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    }
}

#[test]
fn p2_605_validate_writes_nothing_ignores_lint_errors() {
    let dir = Scratch::new("validate-lint");
    let file = dir.file("vague.dsl", &vague());
    let lint = run(&["lint", file.as_str()]);
    assert_eq!(code(&lint), 1, "the fixture has a lint error");
    let before = dir.names();
    let out = run_in(dir.path(), &["validate", "-workspace", file.as_str()], "");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).is_empty(), "no report: {}", stdout(&out));
    assert_eq!(dir.names(), before, "validate wrote a file");
}

#[test]
fn p2_606_validate_parse_error_exit_2_with_position() {
    let dir = Scratch::new("validate-broken");
    let file = dir.file("broken.dsl", BROKEN);
    let out = run(&["validate", "-workspace", file.as_str()]);
    assert_eq!(code(&out), 2);
    assert!(stdout(&out).is_empty());
    let err = stderr(&out);
    assert!(err.starts_with(&format!("c4rizer: {file}:2:3: ")), "{err}");
    assert!(err.contains(" — "), "{err}");
    assert_eq!(err.lines().count(), 1, "{err}");
}

#[test]
fn p2_607_validate_scope_violation_exit_2() {
    let src = r#"workspace {
  model {
    shop = softwareSystem "Shop" "Sells goods." {
      web = container "Web" "Serves pages." "Rust"
    }
    bank = softwareSystem "Bank" "Takes payments." {
      api = container "Api" "Charges cards." "Java"
    }
  }
  configuration {
    scope landscape
  }
}
"#;
    let dir = Scratch::new("scope");
    let file = dir.file("scope.dsl", src);
    let out = run(&["validate", "-workspace", file.as_str()]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.starts_with(&format!("c4rizer: {file}:")), "{err}");
    assert!(err.contains("shop") || err.contains("Shop"), "names the element: {err}");
    let ok = dir.file("ok.dsl", &src.replace("scope landscape", "scope none"));
    assert_eq!(code(&run(&["validate", "-workspace", ok.as_str()])), 0);
}

#[test]
fn p2_608_list_every_element_once_in_declaration_order() {
    let out = run(&["list", "-workspace", EXAMPLE_WS]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    let text = stdout(&out);
    // Provisional format (B cli-breadth 4): `<keyword> <id> "<name>"`,
    // two spaces of indent per nesting level.
    let rows: Vec<(usize, &str, &str)> = text
        .lines()
        .map(|line| {
            let indent = line.len() - line.trim_start().len();
            let mut words = line.trim_start().splitn(3, ' ');
            let keyword = words.next().unwrap();
            let id = words.next().unwrap();
            let name = words.next().unwrap();
            assert!(name.starts_with('"') && name.ends_with('"'), "{line}");
            (indent, keyword, id)
        })
        .collect();
    let ids: Vec<&str> = rows.iter().map(|r| r.2).collect();
    assert_eq!(
        ids,
        [
            "worker", "witness", "admin", "vikunja", "push", "smtp", "system", "app", "cli",
            "web", "sync", "devdata", "ledger", "auth", "config", "vikunjaClient", "notify",
            "mail", "store", "db", "devdb"
        ],
        "{text}"
    );
    let row = |id: &str| *rows.iter().find(|r| r.2 == id).unwrap();
    assert_eq!(row("worker"), (0, "person", "worker"));
    assert_eq!(row("system"), (0, "softwareSystem", "system"));
    assert_eq!(row("app"), (2, "container", "app"));
    assert_eq!(row("ledger"), (4, "component", "ledger"));
    assert!(text.contains("softwareSystem system \"Atomanja\"\n"), "{text}");
}

#[test]
fn p2_609_list_writes_nothing_ignores_lint_errors() {
    let dir = Scratch::new("list-lint");
    let file = dir.file("vague.dsl", &vague());
    assert_eq!(code(&run(&["lint", file.as_str()])), 1, "the fixture has a lint error");
    let before = dir.names();
    let out = run_in(dir.path(), &["list", "-workspace", file.as_str()], "");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "person operator \"Operator\"\nsoftwareSystem shop \"Shop\"\n",
        "no report lines"
    );
    assert_eq!(dir.names(), before, "list wrote a file");
}

#[test]
fn p2_610_list_parse_error_exit_2() {
    let dir = Scratch::new("list-broken");
    let file = dir.file("broken.dsl", BROKEN);
    let out = run(&["list", "-workspace", file.as_str()]);
    assert_eq!(code(&out), 2);
    assert!(stdout(&out).is_empty());
    assert!(stderr(&out).starts_with(&format!("c4rizer: {file}:2:3: ")), "{}", stderr(&out));
    let missing = dir.path().join("absent.dsl");
    let out = run(&["list", "-workspace", missing.to_str().unwrap()]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("cannot read"), "{}", stderr(&out));
}

#[test]
fn p2_611_inspect_one_line_per_violation() {
    let dir = Scratch::new("inspect-lines");
    let file = dir.file("clean.dsl", CLEAN);
    let out = run(&["inspect", "-workspace", file.as_str()]);
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    // Provisional format (B cli-breadth 4): one line per violation, then
    // `<n> violations shown`.
    assert_eq!(
        stdout(&out),
        "error model.relationship.technology operator -> shop\n1 violations shown\n"
    );
    let none = dir.file("spotless.dsl", &spotless());
    let out = run(&["inspect", "-workspace", none.as_str()]);
    assert_eq!(stdout(&out), "0 violations shown\n");
}

#[test]
fn p2_612_line_gives_severity_key_subject() {
    let src = r#"workspace {
  model {
    clerk = person "Clerk"
    shop = softwareSystem "Shop" "Sells goods."
    clerk -> shop "Uses" "HTTPS"
  }
  views {
    systemContext shop "ctx" {
      include *
    }
    systemContext shop "none" {
      include shop
      exclude shop
    }
    styles {
      element "Person" {
        metadata false
      }
    }
  }
}
"#;
    let dir = Scratch::new("inspect-subjects");
    let file = dir.file("subjects.dsl", src);
    let text = stdout(&run(&["inspect", "-workspace", file.as_str()]));
    assert_eq!(
        text,
        "error model.person.description clerk\nerror views.view.empty none\nerror views.styles.element.metadata \"Person\"\n3 violations shown\n"
    );
    // Workspace-level violations have no subject.
    let empty = dir.file("empty.dsl", "workspace {\n  model {\n  }\n}\n");
    let text = stdout(&run(&["inspect", "-workspace", empty.as_str()]));
    assert_eq!(text, "error model.empty\nerror views.empty\n2 violations shown\n");
}

#[test]
fn p2_613_inspect_writes_nothing_ignores_lint_errors() {
    let dir = Scratch::new("inspect-lint");
    let file = dir.file("vague.dsl", &vague());
    assert_eq!(code(&run(&["lint", file.as_str()])), 1, "the fixture has a lint error");
    let before = dir.names();
    let out = run_in(dir.path(), &["inspect", "-workspace", file.as_str()], "");
    // CLI-2 does not apply: one violation, exit 1 from it, not from lint.
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(!stdout(&out).contains("ERROR"), "no lint report: {}", stdout(&out));
    assert_eq!(dir.names(), before, "inspect wrote a file");
    let fixed = dir.file("vague-tech.dsl", &vague().replace("\"uses\"", "\"uses\" \"HTTPS\""));
    assert_eq!(code(&run(&["lint", fixed.as_str()])), 1);
    assert_eq!(code(&run(&["inspect", "-workspace", fixed.as_str()])), 0);
}

#[test]
fn p2_614_inspect_parse_error_exit_255() {
    let dir = Scratch::new("inspect-broken");
    let file = dir.file("broken.dsl", BROKEN);
    let out = run(&["inspect", "-workspace", file.as_str()]);
    // B cli-breadth 5: every inspect error exits 255.
    assert_eq!(code(&out), 255);
    assert!(stdout(&out).is_empty());
    assert!(stderr(&out).starts_with(&format!("c4rizer: {file}:2:3: ")), "{}", stderr(&out));
    let missing = dir.path().join("absent.dsl");
    let out = run(&["inspect", "-workspace", missing.to_str().unwrap()]);
    assert_eq!(code(&out), 255);
}

#[test]
fn p2_640_severity_filter_shows_and_counts_listed_only() {
    // The relationship technology is a warning, the person description an
    // error.
    let src = CLEAN
        .replace("workspace {\n", "workspace {\n  properties {\n    structurizr.inspection.model.relationship.technology warning\n  }\n")
        .replace("\"Operator\" \"Places orders.\"", "\"Operator\"");
    let dir = Scratch::new("severity");
    let file = dir.file("mixed.dsl", &src);
    let all = run(&["inspect", "-workspace", file.as_str()]);
    assert_eq!(
        stdout(&all),
        "error model.person.description operator\nwarning model.relationship.technology operator -> shop\n2 violations shown\n"
    );
    // The exit code counts the error-severity violations shown.
    assert_eq!(code(&all), 1);
    let errors = run(&["inspect", "-workspace", file.as_str(), "-severity", "error"]);
    assert_eq!(stdout(&errors), "error model.person.description operator\n1 violations shown\n");
    assert_eq!(code(&errors), 1);
    let warnings = run(&["inspect", "-severity", "warning,info", "-workspace", file.as_str()]);
    assert_eq!(
        stdout(&warnings),
        "warning model.relationship.technology operator -> shop\n1 violations shown\n"
    );
    assert_eq!(code(&warnings), 0);
    // Structurizr's spellings: `-s`, `--severity`, any case, spaces.
    for args in [
        ["-s", "Error"],
        ["--severity", "ERROR"],
        ["-severity", " error , error"],
    ] {
        let out = run(&["inspect", "-workspace", file.as_str(), args[0], args[1]]);
        assert_eq!(stdout(&out), stdout(&errors), "{args:?}");
    }
}

#[test]
fn p2_641_severity_bad_value_missing_repeated_or_other_command_is_usage_error() {
    for args in [
        vec!["inspect", "-workspace", "a.dsl", "-severity", "fatal"],
        vec!["inspect", "-workspace", "a.dsl", "-severity", "error,,info"],
        vec!["inspect", "-workspace", "a.dsl", "-severity", ""],
        vec!["inspect", "-workspace", "a.dsl", "-severity"],
        vec!["inspect", "-workspace", "a.dsl", "-severity", "error", "-s", "info"],
    ] {
        assert_usage_error(&run(&args), 255, &format!("{args:?}"));
    }
    for command in ["validate", "list"] {
        let out = run(&[command, "-workspace", "a.dsl", "-severity", "error"]);
        assert_usage_error(&out, 2, command);
    }
    let out = run(&["lint", "a.dsl", "-severity", "error"]);
    assert_usage_error(&out, 2, "lint");
}

#[test]
fn p2_642_exit_code_is_error_violations_shown() {
    let dir = Scratch::new("exit-count");
    let none = dir.file("spotless.dsl", &spotless());
    let out = run(&["inspect", "-workspace", none.as_str()]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    // Three violations: two missing descriptions, one missing technology.
    let three = dir.file(
        "three.dsl",
        &CLEAN
            .replace("\"Operator\" \"Places orders.\"", "\"Operator\"")
            .replace("\"Shop\" \"Sells goods.\"", "\"Shop\""),
    );
    let out = run(&["inspect", "-workspace", three.as_str()]);
    assert!(stdout(&out).ends_with("\n3 violations shown\n"), "{}", stdout(&out));
    assert_eq!(code(&out), 3);
}

#[test]
fn p2_642_exit_code_capped_at_254() {
    // 300 people with no relationship and on no view: 600 violations
    // plus `views.empty`.
    let mut src = String::from("workspace {\n  model {\n");
    for i in 0..300 {
        let _ = writeln!(src, "    p{i} = person \"Role {i}\" \"A role.\"");
    }
    src.push_str("  }\n}\n");
    let dir = Scratch::new("exit-cap");
    let file = dir.file("many.dsl", &src);
    let out = run(&["inspect", "-workspace", file.as_str()]);
    assert!(stdout(&out).ends_with("\n601 violations shown\n"), "{}", stderr(&out));
    assert_eq!(code(&out), 254);
}

#[test]
fn p2_643_inspector_refused_exit_2() {
    // P2-643: a refused `-inspector` is not a usage error (no usage text),
    // so B cli-breadth 5's 255 does not apply; it exits 2.
    for flag in ["-inspector", "-i", "--inspector"] {
        let out = run(&["inspect", "-workspace", EXAMPLE_WS, flag, "com.example.Checks"]);
        assert_eq!(code(&out), 2, "{flag}");
        assert!(stdout(&out).is_empty());
        let err = stderr(&out);
        assert_eq!(err.lines().count(), 1, "one line, no usage: {err}");
        assert!(err.contains("com.example.Checks") && err.contains("JVM"), "{err}");
    }
    assert_usage_error(&run(&["inspect", "-workspace", EXAMPLE_WS, "-inspector"]), 255, "no value");
}

#[test]
fn p2_644_inspect_output_identical_twice() {
    let first = run(&["inspect", "-workspace", EXAMPLE_WS]);
    let second = run(&["inspect", "-workspace", EXAMPLE_WS]);
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(code(&first), code(&second));
    let text = stdout(&first);
    assert!(text.ends_with("\n31 violations shown\n"), "{text}");
    assert_eq!(code(&first), 31);
}

#[test]
fn p2_651_svg_text_embed_writes_embedded_svg() {
    let dir = Scratch::new("embed");
    let (out, out_dir) = render_example(&dir, "embed", &["--svg-text", "embed"]);
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    for key in KEYS {
        let svg = fs::read_to_string(out_dir.join(format!("{key}.svg"))).expect("written");
        assert_eq!(svg.matches("@font-face{").count(), 2, "{key}");
        assert!(svg.contains("src:url(data:font/ttf;base64,"), "{key}");
        // Without its style block it is the golden named SVG.
        let start = svg.find("<style>").expect("style");
        let end = svg.find("</style>\n").expect("style end") + "</style>\n".len();
        let mut rest = svg.clone();
        rest.replace_range(start..end, "");
        let golden = fs::read_to_string(format!("{GOLDEN}/{key}.svg")).unwrap();
        assert!(rest == golden, "{key}");
    }
}

#[test]
fn p2_652_svg_text_default_and_name_match_golden() {
    let dir = Scratch::new("name");
    let (plain, plain_dir) = render_example(&dir, "plain", &[]);
    let (named, named_dir) = render_example(&dir, "named", &["--svg-text", "name"]);
    assert_eq!(code(&plain), code(&named));
    for key in KEYS {
        let golden = fs::read(format!("{GOLDEN}/{key}.svg")).unwrap();
        assert!(fs::read(plain_dir.join(format!("{key}.svg"))).unwrap() == golden, "{key}");
        assert!(fs::read(named_dir.join(format!("{key}.svg"))).unwrap() == golden, "{key}");
    }
    // The report is the same apart from the output directory.
    assert_eq!(
        stdout(&plain).replace("plain", "X"),
        stdout(&named).replace("named", "X")
    );
}

#[test]
fn p2_653_svg_text_on_other_command_repeated_missing_or_unknown_is_usage_error() {
    for (args, want) in [
        (vec!["lint", EXAMPLE, "--svg-text", "embed"], 2),
        (vec!["validate", "-workspace", EXAMPLE, "--svg-text", "embed"], 2),
        (vec!["list", "-workspace", EXAMPLE, "--svg-text", "embed"], 2),
        (vec!["inspect", "-workspace", EXAMPLE, "--svg-text", "embed"], 255),
        (vec!["render", EXAMPLE, "-o", "out", "--svg-text", "embed", "--svg-text", "name"], 2),
        (vec!["render", EXAMPLE, "-o", "out", "--svg-text"], 2),
        (vec!["render", EXAMPLE, "-o", "out", "--svg-text", "path"], 2),
        (vec!["render", EXAMPLE, "-o", "out", "--svg-text", "Embed"], 2),
    ] {
        let dir = Scratch::new("svg-text-usage");
        let out = run_in(dir.path(), &args, "");
        assert_usage_error(&out, want, &format!("{args:?}"));
        assert!(dir.names().is_empty(), "{args:?} wrote files");
    }
    let out = run(&["render", EXAMPLE, "-o", "out", "--svg-text", "path"]);
    assert!(stderr(&out).contains("\"path\""), "{}", stderr(&out));
}

#[cfg(feature = "png")]
#[test]
fn p2_661_png_unchanged_by_svg_text() {
    let dir = Scratch::new("png");
    let mut modes = vec!["name", "embed"];
    if cfg!(feature = "outline") {
        modes.push("outline");
    }
    let runs: Vec<PathBuf> = modes
        .iter()
        .map(|mode| {
            let (out, out_dir) = render_example(&dir, mode, &["--png", "--svg-text", mode]);
            assert!(stderr(&out).is_empty(), "{mode}: {}", stderr(&out));
            out_dir
        })
        .collect();
    for key in KEYS {
        let first = fs::read(runs[0].join(format!("{key}.png"))).expect("png");
        for (mode, out_dir) in modes.iter().zip(&runs).skip(1) {
            let png = fs::read(out_dir.join(format!("{key}.png"))).expect("png");
            assert!(png == first, "{key}: --svg-text {mode} changed the PNG");
        }
    }
}

#[cfg(not(feature = "outline"))]
#[test]
fn p2_662_outline_without_feature_exit_2() {
    let dir = Scratch::new("no-outline");
    let (out, out_dir) = render_example(&dir, "out", &["--svg-text", "outline"]);
    assert_eq!(code(&out), 2);
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    let err = stderr(&out);
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(err.contains("--features outline"), "{err}");
    assert!(!out_dir.exists(), "nothing written");
}

#[cfg(feature = "outline")]
#[test]
fn p2_658_render_outline_writes_outlined_svg() {
    let dir = Scratch::new("outline");
    let (out, out_dir) = render_example(&dir, "out", &["--svg-text", "outline"]);
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    for key in KEYS {
        let svg = fs::read_to_string(out_dir.join(format!("{key}.svg"))).expect("written");
        assert!(!svg.contains("<text") && !svg.contains("font-family"), "{key}");
        assert!(svg.contains("<path"), "{key}");
    }
}
