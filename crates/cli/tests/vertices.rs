//! End-to-end tests of the vertices row (SRS 4.7: P3-4, P3-316, P3-317)
//! through the `c4rizer` binary. Data uses roles (SRS 5.4).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

/// A fresh, empty directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("c4rizer-vertices-{}-{n}-{name}", std::process::id()));
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
    Command::new(env!("CARGO_BIN_EXE_c4rizer")).args(args).output().expect("runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// An operator and a shop, the shop's context view placed by hand.
const DSL: &str = r#"workspace "Shop" {
    model {
        operator = person "Operator" "Places orders."
        shop = softwareSystem "Shop" "Sells goods."
        operator -> shop "Places orders in"
    }
    views {
        systemContext shop "ctx" "Shop context" {
            include *
            at operator 200 100
            at shop 200 700
        }
    }
}
"#;

/// The same view as Structurizr JSON, the line bent through two
/// vertices off the 10px grid.
const JSON: &str = r#"{"model": {
    "people": [{"id": "1", "name": "Operator", "relationships": [{"id": "r1", "sourceId": "1", "destinationId": "2"}]}],
    "softwareSystems": [{"id": "2", "name": "Shop"}]},
  "views": {
    "systemContextViews": [{"key": "ctx",
      "elements": [{"id": "1", "x": -25, "y": -50}, {"id": "2", "x": -25, "y": 550}],
      "relationships": [{"id": "r1", "vertices": [{"x": 604, "y": 296}, {"x": 596, "y": 505}]}]}]
  }
}"#;

#[test]
fn p3_317_cli_import_layout_writes_via_lines() {
    let dir = Scratch::new("import");
    let dsl = dir.file("w.dsl", DSL);
    let json = dir.file("w.json", JSON);
    let out = c4rizer(&[Path::new("import-layout"), &json, &dsl]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let written = fs::read_to_string(&dsl).expect("read back");
    assert!(written.contains("            at shop 200 700\n            via operator -> shop 600 300 600 510\n"), "{written}");
    // The file still builds, the line through its vertices.
    let check = c4rizer(&[Path::new("validate"), &dsl]);
    assert!(check.status.success(), "{}{}", text(&check.stdout), text(&check.stderr));
}

/// The row fixtures (`tests/data/vertices.dsl`, `tests/data/phase3/
/// vertices.dsl`) lint with no error-level finding and render.
#[test]
fn p3_4_fixtures_build_without_errors() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data");
    for fixture in [data.join("vertices.dsl"), data.join("phase3/vertices.dsl")] {
        let src = fs::read_to_string(&fixture).expect("fixture");
        assert!(src.contains("\n            via "), "{} has via lines", fixture.display());
        let lint = c4rizer(&[Path::new("lint"), &fixture]);
        assert_eq!(lint.status.code(), Some(0), "{}: {}{}", fixture.display(), text(&lint.stdout), text(&lint.stderr));
        let dir = Scratch::new("render");
        let out = c4rizer(&[Path::new("render"), &fixture, Path::new("-o"), &dir.0]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        assert!(fs::read_dir(&dir.0).expect("rendered").count() > 0);
    }
}

/// `merge` (P3-425) writes the `via` lines `import-layout` would add,
/// to OUT only; FILE never changes (P3-426), and `-v` limits the view.
#[test]
fn p3_425_merge_writes_via_lines() {
    let dir = Scratch::new("merge");
    let dsl = dir.file("w.dsl", DSL);
    let json = dir.file("w.json", JSON);
    let out = dir.0.join("out.dsl");
    let run = c4rizer(&[Path::new("merge"), Path::new("-w"), &dsl, Path::new("-l"), &json, Path::new("-o"), &out]);
    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    let merged = fs::read_to_string(&out).expect("OUT written");
    assert!(merged.contains("            at shop 200 700\n            via operator -> shop 600 300 600 510\n"), "{merged}");
    assert_eq!(fs::read_to_string(&dsl).expect("FILE"), DSL, "FILE never changes");
    // OUT equals FILE but for the added `via` line (its `at` lines match).
    let added: Vec<&str> = merged.lines().filter(|l| !DSL.lines().any(|d| d == *l)).collect();
    assert_eq!(added, ["            via operator -> shop 600 300 600 510"]);
    // `-v` naming another view leaves this one's vertices out.
    let two = DSL.replace("        systemContext shop", "        systemContext shop \"other\" {\n            include *\n        }\n        systemContext shop");
    let dsl2 = dir.file("w2.dsl", &two);
    let out2 = dir.0.join("out2.dsl");
    let run = c4rizer(&[Path::new("merge"), Path::new("-w"), &dsl2, Path::new("-l"), &json, Path::new("-v"), Path::new("other"), Path::new("-o"), &out2]);
    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(!fs::read_to_string(&out2).expect("OUT").contains("via "), "only view `other` may change");
}

/// A line whose layout vertex is out of range, or lacks a coordinate,
/// gets no `via` line, as in JSON in; the other lines' vertices are
/// still written and the command exits 0 (P3-317, P3-425).
#[test]
fn p3_317_cli_bad_layout_vertex_drops_only_its_line() {
    let dsl = DSL
        .replace("        operator -> shop \"Places orders in\"", "        bank = softwareSystem \"Bank\" \"Moves money.\"\n        operator -> shop \"Places orders in\"\n        shop -> bank \"Charges\"")
        .replace("            at shop 200 700\n", "            at shop 200 700\n            at bank 700 700\n");
    let json = |bad: &str| {
        format!(
            r#"{{"model": {{
    "people": [{{"id": "1", "name": "Operator", "relationships": [{{"id": "r1", "sourceId": "1", "destinationId": "2"}}]}}],
    "softwareSystems": [{{"id": "2", "name": "Shop", "relationships": [{{"id": "r2", "sourceId": "2", "destinationId": "3"}}]}}, {{"id": "3", "name": "Bank"}}]}},
  "views": {{"systemContextViews": [{{"key": "ctx",
      "elements": [{{"id": "1", "x": -25, "y": -50}}, {{"id": "2", "x": -25, "y": 550}}, {{"id": "3", "x": 475, "y": 550}}],
      "relationships": [{{"id": "r1", "vertices": [{{"x": 600, "y": 300}}]}}, {{"id": "r2", "vertices": [{bad}]}}]}}]}}
}}"#
        )
    };
    for bad in [r#"{"x": 2147483647, "y": 300}"#, r#"{"x": 300}"#, r#"{"x": "300", "y": 300}"#] {
        let dir = Scratch::new("bad");
        let file = dir.file("w.dsl", &dsl);
        let layout = dir.file("w.json", &json(bad));
        let out = dir.0.join("out.dsl");
        let run = c4rizer(&[Path::new("merge"), Path::new("-w"), &file, Path::new("-l"), &layout, Path::new("-o"), &out]);
        assert_eq!(run.status.code(), Some(0), "{bad}: {}", text(&run.stderr));
        let merged = fs::read_to_string(&out).expect("OUT written");
        assert!(merged.contains("via operator -> shop 600 300\n"), "{bad}: {merged}");
        assert!(!merged.contains("via shop -> bank"), "{bad}: {merged}");
        let run = c4rizer(&[Path::new("import-layout"), &layout, &file]);
        assert_eq!(run.status.code(), Some(0), "{bad}: {}", text(&run.stderr));
        let written = fs::read_to_string(&file).expect("read back");
        assert!(written.contains("via operator -> shop 600 300\n") && !written.contains("via shop -> bank"), "{bad}: {written}");
    }
}
