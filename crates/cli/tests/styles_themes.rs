//! Step-4 contract tests of the styles-themes row, CLI part (P2-432):
//! end-to-end on `env!("CARGO_BIN_EXE_c4rizer")` like `cli.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

/// One system and one person, with a dark-mode fill for the system.
const MODED: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods."
    operator -> shop "Places orders in"
  }
  views {
    systemContext shop "ctx" "Shop context" {
      include *
    }
    styles {
      dark {
        element "Software System" {
          background #222222
        }
      }
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
        let dir = std::env::temp_dir().join(format!("c4rizer-cli-st-{}-{n}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        let scratch = Scratch(dir);
        fs::write(scratch.0.join("ws.dsl"), MODED).expect("fixture");
        scratch
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

fn c4rizer(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_c4rizer"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("binary runs")
}

/// The SVG `render` wrote for view `ctx` into `out`.
fn svg(dir: &Scratch, out: &str) -> String {
    fs::read_to_string(dir.path().join(out).join("ctx.svg")).expect("ctx.svg")
}

/// `c4rizer render FILE -o DIR --mode dark` writes the SVGs (and the PNGs
/// with `--png`) built with `pipeline::build_all_in(.., Mode::Dark, ..)`.
#[test]
fn p2_432_render_mode_dark() {
    let dir = Scratch::new("dark");
    let out = c4rizer(dir.path(), &["render", "ws.dsl", "-o", "dark", "--mode", "dark"]);
    assert!(out.status.code().is_some_and(|c| c < 2), "{}", String::from_utf8_lossy(&out.stderr));
    let dark = svg(&dir, "dark");
    assert!(dark.contains("#222222"), "{dark}");
    let out = c4rizer(dir.path(), &["render", "ws.dsl", "-o", "light", "--mode", "light"]);
    assert!(out.status.code().is_some_and(|c| c < 2));
    assert!(!svg(&dir, "light").contains("#222222"));
    // `--mode` may come anywhere after the subcommand.
    let out = c4rizer(dir.path(), &["render", "--mode", "dark", "ws.dsl", "-o", "again"]);
    assert!(out.status.code().is_some_and(|c| c < 2));
    assert_eq!(svg(&dir, "again"), dark);
}

/// With no `--mode`, `render` writes the same bytes as `--mode light`
/// (Structurizr `-mode`, default light).
#[test]
fn p2_432_mode_defaults_light() {
    let dir = Scratch::new("default");
    let a = c4rizer(dir.path(), &["render", "ws.dsl", "-o", "out"]);
    let plain = svg(&dir, "out");
    let b = c4rizer(dir.path(), &["render", "ws.dsl", "-o", "out", "--mode", "light"]);
    assert_eq!(a.status.code(), b.status.code());
    assert_eq!(a.stdout, b.stdout);
    assert_eq!(plain, svg(&dir, "out"));
}

/// `--mode` with any value other than `light` or `dark`, or with no
/// value, is a usage error: exit 2, nothing written.
#[test]
fn p2_432_bad_mode_usage_error() {
    let dir = Scratch::new("bad");
    for args in [
        &["render", "ws.dsl", "-o", "out", "--mode", "sepia"][..],
        &["render", "ws.dsl", "-o", "out", "--mode"][..],
        &["render", "ws.dsl", "-o", "out", "--mode", "dark", "--mode", "dark"][..],
        &["render", "ws.dsl", "-o", "out", "--mode", "Dark"][..],
    ] {
        let out = c4rizer(dir.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--mode") && err.contains("usage:"), "{err}");
        assert!(!dir.path().join("out").exists(), "{args:?} wrote files");
    }
    // `lint` takes no `--mode`.
    let out = c4rizer(dir.path(), &["lint", "ws.dsl", "--mode", "dark"]);
    assert_eq!(out.status.code(), Some(2));
}

/// `c4rizer render tests/data/styles/all_styles.dsl -o DIR` parses
/// through `parse_with` with `FsLoader`: the local theme file draws
/// (P2-13, P2-427), its icon is read relative to the theme (P2-428) and
/// the style's local icon file is drawn (P2-409), in SVG and PNG.
#[test]
fn p2_13_render_draws_local_theme_and_icons() {
    let dir = Scratch::new("fixture");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/styles/all_styles.dsl");
    let fixture = fs::canonicalize(fixture).expect("fixture");
    let mut args = vec!["render", fixture.to_str().expect("utf-8 path"), "-o", "out"];
    let png = cfg!(feature = "png");
    if png {
        args.push("--png");
    }
    let out = c4rizer(dir.path(), &args);
    assert!(
        out.status.code().is_some_and(|c| c < 2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let all = fs::read_to_string(dir.path().join("out/all.svg")).expect("all.svg");
    let ctx = svg(&dir, "out");
    // The local theme's container fill (theme.json `Container`).
    assert!(all.contains("#438dd5"), "theme colour missing");
    // The style's local SVG icon file, embedded.
    assert!(all.contains("<image") && all.contains("data:image/svg+xml;base64,"), "file icon missing");
    // The theme's PNG icon, relative to the theme's folder, on the
    // partner, besides the operator's `data:` URI icon.
    assert_eq!(ctx.matches("data:image/png;base64,").count(), 2, "theme icon missing");
    if png {
        assert!(dir.path().join("out/ctx.png").exists());
    }
}
