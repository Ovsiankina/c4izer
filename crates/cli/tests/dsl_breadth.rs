//! Step-4 contract tests of the dsl-breadth row, CLI part (P2-122,
//! P2-125, P2-136): end-to-end on `env!("CARGO_BIN_EXE_c4rizer")` like
//! `cli.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

/// A clean context view (boxes on the grid, one labelled line) between a
/// dynamic view and a deployment view.
const WITH_UNSUPPORTED: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods." {
      web = container "Web" "Serves pages." "Rust"
    }
    operator -> shop "Places orders in"
    live = deploymentEnvironment "Live" {
      node = deploymentNode "Node" {
        containerInstance web
      }
    }
  }
  views {
    dynamic * "flow" "Checkout flow" {
      operator -> shop "Opens the shop"
    }
    systemContext shop "ctx" "Shop context" {
      include *
      at operator 200 150
      at shop 200 450
    }
    deployment * live "live" {
      include *
      title "Live"
    }
  }
}
"#;

/// A fresh, empty directory under the system temp dir, removed on drop.
// DECISION: hand-made like cli.rs's (pid, counter, test name); no
// dev-dependency.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "c4rizer-cli-dsl-breadth-{}-{n}-{name}",
            std::process::id()
        ));
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

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("UTF-8 stdout")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("UTF-8 stderr")
}

/// `render` and `lint` print `== <key>: <title>` then a three-space
/// indented line containing `not yet supported` for an unsupported view,
/// write no file for it and add nothing to the exit code (CLI-1, CLI-2).
#[test]
fn p2_122_cli_prints_not_yet_supported() {
    // TODO(srs): P2-122 the exact line text. Reading taken (B dsl-breadth
    // 24): `   not yet supported: <keyword> views are not drawn by this
    // version`.
    let dir = Scratch::new("p2-122");
    dir.file("shop.dsl", WITH_UNSUPPORTED);
    for args in [&["lint", "shop.dsl"][..], &["render", "shop.dsl", "-o", "out"][..]] {
        let out = c4rizer(dir.path(), args);
        let text = stdout(&out);
        assert_eq!(out.status.code(), Some(0), "{args:?}\n{text}\n{}", stderr(&out));
        // dynamic-view (P3-200) and deployment-view (P3-2) draw their
        // views, so no view of this file is unsupported any more.
        assert!(text.contains("== flow: Checkout flow\n"), "{text}");
        assert!(text.contains("== live: Live\n"), "{text}");
        assert!(!text.contains("not yet supported"), "{text}");
        // Views in declaration order: flow, ctx, live.
        let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s}\n{text}"));
        assert!(at("== flow:") < at("== ctx:") && at("== ctx:") < at("== live:"), "{text}");
    }
    let mut written: Vec<String> = fs::read_dir(dir.path().join("out"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(written, ["ctx.svg", "flow.svg", "live.svg"]);
}

/// `render` and `lint` print every notice before the first view, one
/// line each naming construct, file and line; notices add nothing to the
/// exit code.
#[test]
fn p2_125_cli_prints_notices_before_views() {
    // TODO(srs): P2-125 the exact line format. Reading taken (B
    // dsl-breadth 24): `notice: <path>:<line>:<column>: <construct> is
    // accepted and has no effect`.
    let dir = Scratch::new("p2-125");
    dir.file(
        "plugin.dsl",
        "!plugin com.example.Plugin\noperator -> shop \"Places orders in\"\n",
    );
    dir.file(
        "shop.dsl",
        r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods."
    !include plugin.dsl
  }
  views {
    systemContext shop "ctx" "Shop context" {
      include *
      at operator 200 150
      at shop 200 450
    }
    properties {
      plantuml.url https://plantuml.example.com
    }
  }
}
"#,
    );
    let included = dir.path().join("plugin.dsl");
    let included = fs::canonicalize(&included).unwrap_or(included);
    for args in [&["lint", "shop.dsl"][..], &["render", "shop.dsl", "-o", "out"][..]] {
        let out = c4rizer(dir.path(), args);
        let text = stdout(&out);
        assert_eq!(out.status.code(), Some(0), "{args:?}\n{text}\n{}", stderr(&out));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            format!(
                "notice: {}:1:1: !plugin is accepted and has no effect",
                included.display()
            ),
            "{text}"
        );
        assert_eq!(
            lines[1], "notice: shop.dsl:14:7: plantuml.url is accepted and has no effect",
            "{text}"
        );
        assert!(lines[2].starts_with("== ctx: "), "{text}");
    }
}

/// A parse or model error in an included file is reported with that
/// file's path, line and column (SRS 5.1).
#[test]
fn p2_136_cli_error_names_included_file() {
    let dir = Scratch::new("p2-136");
    fs::create_dir_all(dir.path().join("parts")).unwrap();
    dir.file("parts/good.dsl", "operator = person \"Operator\"\n");
    dir.file("parts/bad.dsl", "shop = softwareSystem \"Shop\"\n  bogus line\n");
    dir.file(
        "shop.dsl",
        "workspace {\n  model {\n    !include parts/good.dsl\n    !include parts/bad.dsl\n  }\n}\n",
    );
    let bad = dir.path().join("parts/bad.dsl");
    let bad = fs::canonicalize(&bad).unwrap_or(bad);
    for args in [&["lint", "shop.dsl"][..], &["render", "shop.dsl", "-o", "out"][..]] {
        let out = c4rizer(dir.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = stderr(&out);
        assert!(
            err.starts_with(&format!("c4rizer: {}:2:3: `bogus`", bad.display())),
            "{err}"
        );
    }
    // An error in the root file still names the root file as given.
    dir.file("root.dsl", "workspace {\n  model {\n    bogus\n  }\n}\n");
    let out = c4rizer(dir.path(), &["lint", "root.dsl"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("c4rizer: root.dsl:3:5: "), "{}", stderr(&out));
    // From stdin an include has no folder to resolve against (NeedsFile).
    let mut child = Command::new(env!("CARGO_BIN_EXE_c4rizer"))
        .current_dir(dir.path())
        .args(["lint", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(b"workspace {\n  model {\n    !include parts/good.dsl\n  }\n}\n")
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.starts_with("c4rizer: <stdin>:3:") && err.contains("`!include`"), "{err}");
}
