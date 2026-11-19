//! Tests of the dynamic-view row, CLI part (SRS 4.7 P3-3): end-to-end on
//! `env!("CARGO_BIN_EXE_c4rizer")`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

/// A checkout flow, roles only, with a request and its response.
const FLOW: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods." {
      web = container "Web" "Serves pages." "Rust"
      db = container "Db" "Stores orders." "SQL"
    }
    operator -> web "Places orders in" "HTTPS"
    web -> db "Stores orders in" "SQL"
    db -> web "Confirms writes to" "SQL"
  }
  views {
    dynamic shop "flow" "Checkout flow" {
      operator -> web "Submits the order"
      web -> db "Saves the order"
      db -> web "Confirms the save"
    }
  }
}
"#;

/// A fresh, empty directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "c4rizer-cli-dynamic-view-{}-{n}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        Scratch(dir)
    }

    fn path(&self) -> &Path {
        &self.0
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

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("UTF-8 stdout")
}

/// `c4rizer render` writes an SVG for a dynamic view and lists it as drawn, not unsupported.
#[test]
fn p3_3_cli_renders_dynamic_view() {
    let dir = Scratch::new("p3-3");
    fs::write(dir.path().join("flow.dsl"), FLOW).expect("fixture written");
    let out = c4rizer(dir.path(), &["render", "flow.dsl", "-o", "out"]);
    let text = stdout(&out);
    assert!(text.contains("== flow: Checkout flow"), "{text}");
    assert!(!text.contains("not yet supported"), "{text}");
    let svg = fs::read_to_string(dir.path().join("out/flow.svg")).expect("flow.svg written");
    // Design-system Edge "Step": each number is drawn in its own disc
    // before the label text (the scene text stays `<n>: <text>`, P3-211).
    for (number, label) in [("1", "Submits the order"), ("2", "Saves the order"), ("3", "Confirms the save")] {
        assert!(svg.contains(&format!("text-anchor=\"middle\">{number}</text>")), "step {number} missing");
        assert!(svg.contains(&format!(">{label}</text>")), "{label} missing");
    }
    // The request/response pair is not L14 on a dynamic view (P3-216).
    assert!(!text.contains("two-way"), "{text}");
}

/// `tests/data/dynamic_view.dsl` renders with no error-level finding.
#[test]
fn dynamic_view_fixture_builds_clean() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/dynamic_view.dsl");
    let dir = Scratch::new("fixture");
    let path = fixture.canonicalize().expect("fixture exists");
    let out = c4rizer(dir.path(), &["lint", path.to_str().expect("UTF-8 path")]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(!text.contains("not yet supported"), "{text}");
}
