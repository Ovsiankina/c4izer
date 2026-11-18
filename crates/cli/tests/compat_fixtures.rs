//! End-to-end tests of `c4rizer import-layout JSON FILE` (SRS 4.6:
//! P2-15, P2-504), run through `std::process::Command` on
//! `env!("CARGO_BIN_EXE_c4rizer")`. Hand-made data uses roles (SRS 5.4);
//! `p2_15_big_bank_import` copies the pinned Structurizr corpus files.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/structurizr");

/// A small file whose context view the JSON below positions.
const DSL: &str = r#"workspace "Shop" {
    model {
        operator = person "Operator" "Places orders."
        shop = softwareSystem "Shop" "Sells goods."
        operator -> shop "Places orders in"
    }
    views {
        systemContext shop "ctx" "Shop context" {
            include *
        }
    }
}
"#;

const JSON: &str = r#"{"model": {
    "people": [{"id": "1", "name": "Operator", "tags": "Element,Person"}],
    "softwareSystems": [{"id": "2", "name": "Shop", "tags": "Element,Software System"}]},
  "views": {
    "systemContextViews": [{"key": "ctx", "elements": [{"id": "1", "x": 0, "y": 0}, {"id": "2", "x": 0, "y": 600}]}],
    "dynamicViews": [{"key": "flow", "elements": []}],
    "configuration": {"styles": {"elements": [{"tag": "Element", "width": 400, "height": 200}]}}
  }
}"#;

/// A fresh, empty directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("c4rizer-import-{}-{n}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        Scratch(dir)
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).expect("write");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn c4rizer(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_c4rizer")).arg("import-layout").args(args).output().expect("runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The words of every line that is not an `at` line.
fn non_at_words(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("at "))
        .flat_map(str::split_whitespace)
        .map(str::to_owned)
        .collect()
}

/// The `at` lines' coordinates.
fn at_values(text: &str) -> Vec<(i32, i32)> {
    text.lines()
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            (words.next() == Some("at")).then(|| {
                let _ = words.next();
                let x = words.next().and_then(|w| w.parse().ok()).expect("x");
                let y = words.next().and_then(|w| w.parse().ok()).expect("y");
                (x, y)
            })
        })
        .collect()
}

#[test]
fn p2_504_import_layout_exit_codes() {
    let dir = Scratch::new("exit");
    let dsl = dir.file("w.dsl", DSL);
    let json = dir.file("w.json", JSON);
    let ok = c4rizer(&[&json, &dsl]);
    assert_eq!(ok.status.code(), Some(0), "{}", text(&ok.stderr));
    // Usage: no file, one file, three files, a flag.
    for args in [vec![], vec![json.as_path()], vec![json.as_path(), dsl.as_path(), dsl.as_path()]] {
        let out = c4rizer(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("import-layout JSON FILE"), "usage on stderr: {}", text(&out.stderr));
    }
    let out = c4rizer(&[Path::new("--merge"), &json, &dsl]);
    assert_eq!(out.status.code(), Some(2));
    // Missing file, DSL parse error, bad JSON, JSON that is not a workspace.
    let missing = dir.0.join("missing.json");
    let broken_dsl = dir.file("broken.dsl", "workspace {\n  model {\n");
    let bad_json = dir.file("bad.json", "{\"views\": {");
    let not_ws = dir.file("notws.json", "{\"model\": {}}");
    for (j, d) in [(&missing, &dsl), (&json, &broken_dsl), (&bad_json, &dsl), (&not_ws, &dsl)] {
        let out = c4rizer(&[j, d]);
        assert_eq!(out.status.code(), Some(2), "{} {}: {}", j.display(), d.display(), text(&out.stderr));
        assert!(text(&out.stderr).starts_with("c4rizer: "), "{}", text(&out.stderr));
    }
}

#[test]
fn p2_504_json_error_names_path_line_column() {
    let dir = Scratch::new("jsonerr");
    let dsl = dir.file("w.dsl", DSL);
    let json = dir.file("w.json", "{\n  \"views\": tru\n}\n");
    let out = c4rizer(&[&json, &dsl]);
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.starts_with(&format!("c4rizer: {}:2:12: ", json.display())), "{err}");
    assert!(err.contains('—'), "says what to change: {err}");
    let not_ws = dir.file("n.json", "[]");
    let err = text(&c4rizer(&[&not_ws, &dsl]).stderr);
    assert!(err.starts_with(&format!("c4rizer: {}: ", not_ws.display())) && err.contains("views"), "{err}");
}

#[test]
fn p2_504_file_untouched_on_error() {
    let dir = Scratch::new("untouched");
    let dsl = dir.file("w.dsl", DSL);
    for bad in ["{\"views\": {", "{\"model\": {}}", "not json"] {
        let json = dir.file("bad.json", bad);
        let before = fs::read(&dsl).expect("read");
        assert_eq!(c4rizer(&[&json, &dsl]).status.code(), Some(2));
        assert_eq!(fs::read(&dsl).expect("read"), before, "{bad}");
    }
    let leftovers: Vec<_> = fs::read_dir(&dir.0).expect("dir").map(|e| e.expect("entry").file_name()).collect();
    assert_eq!(leftovers.len(), 2, "no temporary file left behind: {leftovers:?}");
    // A clean run writes the `at` lines and reports.
    let json = dir.file("w.json", JSON);
    let out = c4rizer(&[&json, &dsl]);
    let stdout = text(&out.stdout);
    assert!(stdout.contains("== ctx\n   wrote 2 positions\n"), "{stdout}");
    assert!(stdout.contains("skipped flow: "), "{stdout}");
    assert_eq!(at_values(&fs::read_to_string(&dsl).expect("read")), [(200, 100), (200, 700)]);
    // One position is reported in the singular.
    let dsl = dir.file("one.dsl", DSL);
    let one = JSON.replace(r#", {"id": "2", "x": 0, "y": 600}"#, "");
    let json = dir.file("one.json", &one);
    let stdout = text(&c4rizer(&[&json, &dsl]).stdout);
    assert!(stdout.contains("== ctx\n   wrote 1 position\n"), "{stdout}");
}

#[test]
fn p2_15_big_bank_import() {
    let dir = Scratch::new("bigbank");
    let original = fs::read_to_string(Path::new(CORPUS).join("big-bank-plc.dsl")).expect("corpus");
    let dsl = dir.file("big-bank-plc.dsl", &original);
    let json = dir.file("big-bank-plc.json", &fs::read_to_string(Path::new(CORPUS).join("big-bank-plc.json")).expect("corpus"));
    let out = c4rizer(&[&json, &dsl]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let after = fs::read_to_string(&dsl).expect("read");
    let parsed = c4rizer_core::model::parse(&after).unwrap_or_else(|e| panic!("the result parses: {e}"));
    assert_eq!(non_at_words(&after), non_at_words(&original), "only `at` lines (and DSL-9 splits) differ");
    let ats = at_values(&after);
    assert!(!ats.is_empty());
    assert!(ats.iter().all(|(x, y)| x % 10 == 0 && y % 10 == 0), "on the 10px grid: {ats:?}");
    // Keys read from the pinned files: drawn in both, and skipped.
    for key in ["SystemLandscape", "SystemContext", "Containers", "Components"] {
        let view = parsed.model.view(key).expect("view in the file");
        assert!(!view.positions.is_empty(), "{key} gains `at` lines");
    }
    let stdout = text(&out.stdout);
    for key in ["SignIn", "LiveDeployment", "DevelopmentDeployment", "MainframeBankingSystemFacade"] {
        assert!(stdout.contains(&format!("skipped {key}: ")), "{key} reported skipped:\n{stdout}");
    }
}
