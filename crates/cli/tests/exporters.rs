//! Tests of the exporters row, CLI part (SRS 4.7 P3-6, P3-8, P3-400..P3-405,
//! P3-416, P3-425..P3-430, P3-432): end-to-end on
//! `env!("CARGO_BIN_EXE_c4rizer")`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data");

/// A fresh, empty directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("c4rizer-export-{}-{n}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        Scratch(dir)
    }

    /// Copies `tests/data/<rel>` into the scratch folder as `name`.
    fn copy(&self, rel: &str, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::copy(Path::new(DATA).join(rel), &path).expect("copy");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn c4rizer(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_c4rizer"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run c4rizer")
}

/// The reference file of `fixture`/`format`/`name`, with the final
/// newline P3-403 adds.
fn reference(fixture: &str, format: &str, name: &str) -> String {
    let path = Path::new(DATA).join("phase3/reference").join(fixture).join(format).join(name);
    let mut t = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    if !t.ends_with('\n') {
        t.push('\n');
    }
    t
}

/// The file names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|r| r.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Runs `export -f <format> -o <out> <file>` and checks exit 0.
fn export(dir: &Path, file: &Path, format: &str, out: &Path) -> Output {
    let o = c4rizer(&["export", "-f", format, "-o", out.to_str().unwrap(), file.to_str().unwrap()], dir);
    assert_eq!(o.status.code(), Some(0), "{format}: {}", text(&o.stderr));
    o
}

fn text(o: &[u8]) -> String {
    String::from_utf8_lossy(o).into_owned()
}

#[test]
fn p3_6_every_format_written_by_cli() {
    // Every text format of the dynamic fixture equals the reference.
    let dir = Scratch::new("p3_6");
    let file = dir.copy("phase3/dynamic.dsl", "dynamic.dsl");
    for (format, folder) in [
        ("plantuml", "plantuml-structurizr"),
        ("plantuml/c4plantuml", "plantuml-c4plantuml"),
        ("mermaid", "mermaid"),
        ("websequencediagrams", "websequencediagrams"),
        ("dot", "dot"),
        ("ilograph", "ilograph"),
    ] {
        let out = dir.0.join(folder);
        export(&dir.0, &file, format, &out);
        let written = names(&out);
        let expected = names(&Path::new(DATA).join("phase3/reference/dynamic").join(folder));
        assert_eq!(written, expected, "{format}");
        for name in written {
            assert_eq!(fs::read_to_string(out.join(&name)).unwrap(), reference("dynamic", folder, &name), "{format}/{name}");
        }
    }
    // `json` holds the `structurizr.dsl` property: the file, base64.
    let out = dir.0.join("json");
    export(&dir.0, &file, "json", &out);
    let json = fs::read_to_string(out.join("dynamic.json")).unwrap();
    assert!(json.contains("\"structurizr.dsl\" : \""), "{json}");
    // `theme` writes the workspace's styles.
    let styles = Scratch::new("p3_6_theme");
    let file = styles.copy("phase3/reference/styles/structurizr.dsl", "styles.dsl");
    fs::create_dir_all(styles.0.join("icons")).unwrap();
    fs::copy(Path::new(DATA).join("styles/icons/database.svg"), styles.0.join("icons/database.svg")).unwrap();
    let out = styles.0.join("theme");
    export(&styles.0, &file, "theme", &out);
    assert_eq!(
        fs::read_to_string(out.join("workspace-theme.json")).unwrap(),
        reference("styles", "", "all_styles-theme.json")
    );
    // `static` writes c4rizer's own site (P3-423): the index, then per
    // drawable view its SVG (as `render` writes it) and its page.
    let out = dir.0.join("static");
    export(&dir.0, &file_dynamic(&dir), "static", &out);
    let rendered = dir.0.join("rendered");
    let o = c4rizer(&["render", file_dynamic(&dir).to_str().unwrap(), "-o", rendered.to_str().unwrap()], &dir.0);
    assert!(o.status.code().is_some_and(|c| c < 2), "{}", text(&o.stderr));
    let svgs = names(&rendered);
    let mut expected: Vec<String> = svgs.iter().map(|n| format!("view-{}", n.replace(".svg", ".html"))).collect();
    expected.extend(svgs.iter().cloned());
    expected.push("index.html".to_owned());
    expected.sort();
    assert_eq!(names(&out), expected);
    for svg in &svgs {
        assert_eq!(fs::read(out.join(svg)).unwrap(), fs::read(rendered.join(svg)).unwrap(), "{svg}");
    }
    let index = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(index.contains("<a href=\"view-seq.html\">"), "{index}");
}

/// The dynamic fixture's copy in `dir`.
fn file_dynamic(dir: &Scratch) -> PathBuf {
    dir.0.join("dynamic.dsl")
}

#[test]
fn p3_8_merge_and_generate_commands() {
    // Both commands are in the usage text and in `help`.
    let dir = Scratch::new("p3_8");
    let help = c4rizer(&["help"], &dir.0);
    for line in ["c4rizer merge -w FILE -l LAYOUT.json [-v KEY] -o OUT", "c4rizer generate system-landscape -i DIR -o OUT.dsl"] {
        assert!(text(&help.stdout).contains(line), "{}", text(&help.stdout));
    }
    for name in ["merge", "generate"] {
        let o = c4rizer(&["help", name], &dir.0);
        assert_eq!(o.status.code(), Some(0));
        assert!(text(&o.stdout).starts_with(&format!("usage: c4rizer {name} ")), "{}", text(&o.stdout));
    }
    // `merge` applies a layout, `generate` merges workspaces; the outputs
    // render.
    let (dir, _, _) = merge_setup("p3_8_merge");
    let o = c4rizer(&["merge", "-w", "file.dsl", "-l", "layout.json", "-o", "out.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    fs::create_dir_all(dir.0.join("in")).unwrap();
    fs::copy(dir.0.join("out.dsl"), dir.0.join("in/out.dsl")).unwrap();
    fs::copy(dir.0.join("layout.json"), dir.0.join("in/layout.json")).unwrap();
    let o = c4rizer(&["generate", "system-landscape", "-i", "in", "-o", "land.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    for file in ["out.dsl", "land.dsl"] {
        let o = c4rizer(&["render", file, "-o", "svg"], &dir.0);
        assert!(o.status.code().is_some_and(|c| c < 2), "{file}: {}", text(&o.stderr));
    }
    assert!(dir.0.join("svg/SystemLandscape.svg").exists());
}

#[test]
fn p3_400_export_writes_into_dir() {
    let dir = Scratch::new("p3_400");
    let file = dir.copy("phase3/dynamic.dsl", "dynamic.dsl");
    let out = dir.0.join("out");
    let o = export(&dir.0, &file, "mermaid", &out);
    assert_eq!(names(&out), ["structurizr-comp.mmd", "structurizr-seq.mmd", "structurizr-star.mmd"]);
    let stdout = text(&o.stdout);
    assert_eq!(stdout.matches("   wrote ").count(), 3, "{stdout}");
    // The long spellings and `-w` say the same.
    let long = dir.0.join("long");
    let o = c4rizer(
        &["export", "--format", "mermaid", "--output", long.to_str().unwrap(), "-w", file.to_str().unwrap()],
        &dir.0,
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(names(&long), names(&out));
}

#[test]
fn p3_400_default_output_is_file_folder() {
    let dir = Scratch::new("p3_400_default");
    let sub = dir.0.join("model");
    fs::create_dir_all(&sub).unwrap();
    fs::copy(Path::new(DATA).join("phase3/dynamic.dsl"), sub.join("dynamic.dsl")).unwrap();
    let o = c4rizer(&["export", "-f", "websequencediagrams", "model/dynamic.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(
        names(&sub),
        ["dynamic.dsl", "structurizr-comp.wsd", "structurizr-seq.wsd", "structurizr-star.wsd"]
    );
}

#[test]
fn p3_401_unknown_format_is_usage_error() {
    let dir = Scratch::new("p3_401");
    let file = dir.copy("phase3/dynamic.dsl", "dynamic.dsl");
    let file = file.to_str().unwrap();
    for args in [
        vec!["export", "-f", "fqcn", file],
        vec!["export", "-f", "com.example.MyExporter", file],
        vec!["export", "-f", "pdf", file],
        vec!["export", file],
    ] {
        let o = c4rizer(&args, &dir.0);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        let err = text(&o.stderr);
        for format in ["plantuml/structurizr", "plantuml/c4plantuml", "mermaid", "websequencediagrams", "dot", "ilograph", "json", "theme", "static", "svg", "png"] {
            assert!(err.contains(format), "{args:?} lists {format}: {err}");
        }
        assert!(err.contains("usage: c4rizer"), "{err}");
    }
    assert_eq!(names(&dir.0), ["dynamic.dsl"], "nothing written");
}

#[test]
fn p3_402_export_svg_png_match_render() {
    let dir = Scratch::new("p3_402");
    let file = dir.copy("phase3/dynamic.dsl", "dynamic.dsl");
    let path = file.to_str().unwrap();
    let exported = dir.0.join("export");
    let rendered = dir.0.join("render");
    let export_run = c4rizer(&["export", "-f", "svg", "-o", exported.to_str().unwrap(), path], &dir.0);
    let render_run = c4rizer(&["render", path, "-o", rendered.to_str().unwrap()], &dir.0);
    assert_eq!(export_run.status.code(), Some(0), "{}", text(&export_run.stderr));
    assert!(matches!(render_run.status.code(), Some(0 | 1)));
    assert_eq!(names(&exported), names(&rendered));
    assert_eq!(names(&exported), ["comp.svg", "seq.svg", "star.svg"]);
    for name in names(&exported) {
        assert_eq!(fs::read(exported.join(&name)).unwrap(), fs::read(rendered.join(&name)).unwrap(), "{name}");
    }
    // `-f png` is `render --png`: the same files, or the same refusal
    // without the `png` feature.
    let exported = dir.0.join("export-png");
    let rendered = dir.0.join("render-png");
    let export_run = c4rizer(&["export", "-f", "png", "-o", exported.to_str().unwrap(), path], &dir.0);
    let render_run = c4rizer(&["render", path, "-o", rendered.to_str().unwrap(), "--png"], &dir.0);
    if render_run.status.code() == Some(2) {
        assert_eq!(export_run.status.code(), Some(2));
        assert_eq!(text(&export_run.stderr), text(&render_run.stderr));
    } else {
        assert_eq!(export_run.status.code(), Some(0), "{}", text(&export_run.stderr));
        assert_eq!(names(&exported), names(&rendered));
        for name in names(&exported) {
            assert_eq!(fs::read(exported.join(&name)).unwrap(), fs::read(rendered.join(&name)).unwrap(), "{name}");
        }
    }
}

#[test]
fn p3_404_files_named_as_structurizr() {
    let dir = Scratch::new("p3_404");
    let file = dir.copy("phase3/deployment.dsl", "deployment.dsl");
    for (format, folder) in [
        ("plantuml/structurizr", "plantuml-structurizr"),
        ("plantuml/c4plantuml", "plantuml-c4plantuml"),
        ("mermaid", "mermaid"),
        ("dot", "dot"),
        ("ilograph", "ilograph"),
        ("json", "json"),
    ] {
        let out = dir.0.join(folder);
        export(&dir.0, &file, format, &out);
        let expected = names(&Path::new(DATA).join("phase3/reference/deployment").join(folder));
        assert_eq!(names(&out), expected, "{format}");
    }
}

#[test]
fn p3_405_unsupported_views_skipped() {
    let dir = Scratch::new("p3_405");
    let file = dir.copy("phase3/deployment.dsl", "deployment.dsl");
    let o = export(&dir.0, &file, "websequencediagrams", &dir.0.join("wsd"));
    let out = text(&o.stdout);
    for line in [
        "== live: skipped (websequencediagrams does not support deployment views)",
        "== staging: skipped (websequencediagrams does not support deployment views)",
        "== live-apps: skipped (websequencediagrams does not support filtered views)",
    ] {
        assert!(out.contains(line), "{line}: {out}");
    }
    assert!(names(&dir.0.join("wsd")).is_empty());
    let o = export(&dir.0, &file, "mermaid", &dir.0.join("mmd"));
    let out = text(&o.stdout);
    assert!(out.contains("== live-apps: skipped (mermaid does not support filtered views)"), "{out}");
    assert!(!out.contains("== live: skipped"), "{out}");
    // An image view is skipped by every text format.
    let image = dir.0.join("image.dsl");
    fs::write(
        &image,
        "workspace {\n    model {\n        s = softwareSystem \"S\"\n    }\n    views {\n        systemContext s \"ctx\" {\n            include *\n        }\n        image * \"pic\" {\n            image https://example.com/a.png\n        }\n    }\n}\n",
    )
    .unwrap();
    for format in ["plantuml", "plantuml/c4plantuml", "mermaid", "dot", "ilograph"] {
        let o = export(&dir.0, &image, format, &dir.0.join("img"));
        assert!(
            text(&o.stdout).contains(&format!("== pic: skipped ({} does not support image views)", if format == "plantuml" { "plantuml/structurizr" } else { format })),
            "{format}: {}",
            text(&o.stdout)
        );
    }
}

#[test]
fn p3_416_every_command_reads_json() {
    let dir = Scratch::new("json");
    // Structurizr 6.2.4's own JSON of the deployment fixture, and the DSL.
    let json = dir.copy("phase3/reference/deployment/json/deployment.json", "shop.json");
    let dsl = dir.copy("phase3/deployment.dsl", "shop.dsl");
    let (json, dsl) = (json.to_str().unwrap(), dsl.to_str().unwrap());
    // `validate`, `list` and `inspect` read it as the DSL it came from.
    let v = c4rizer(&["validate", json], &dir.0);
    assert_eq!(v.status.code(), Some(0), "{}", text(&v.stderr));
    for command in ["list", "inspect"] {
        let a = c4rizer(&[command, json], &dir.0);
        let b = c4rizer(&[command, dsl], &dir.0);
        assert_eq!(a.status.code(), b.status.code(), "{command}: {}", text(&a.stderr));
        assert_eq!(text(&a.stdout), text(&b.stdout), "{command}");
    }
    // `lint` and `render` build and draw every view.
    let lint = c4rizer(&["lint", json], &dir.0);
    assert!(matches!(lint.status.code(), Some(0 | 1)), "{}", text(&lint.stderr));
    let out = dir.0.join("svg");
    let render = c4rizer(&["render", json, "-o", out.to_str().unwrap()], &dir.0);
    assert!(matches!(render.status.code(), Some(0 | 1)), "{}", text(&render.stderr));
    for key in ["live", "staging", "live-apps"] {
        assert!(out.join(format!("{key}.svg")).exists(), "{key}.svg");
    }
    // `export` writes from it what it writes from the DSL.
    let out = dir.0.join("puml");
    let o = c4rizer(&["export", "-f", "plantuml", "-o", out.to_str().unwrap(), json], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    for name in names(&out) {
        assert_eq!(
            fs::read_to_string(out.join(&name)).unwrap(),
            reference("deployment", "plantuml-structurizr", &name),
            "{name}"
        );
    }
    assert_eq!(names(&out).len(), 4);
    // A file starting with `{` that is not JSON is a parse error (exit 2)
    // naming the file, line and column.
    let bad = dir.0.join("bad.json");
    fs::write(&bad, "{\n  \"model\": [\n").unwrap();
    let e = c4rizer(&["validate", bad.to_str().unwrap()], &dir.0);
    assert_eq!(e.status.code(), Some(2));
    assert!(text(&e.stderr).contains("bad.json:3:"), "{}", text(&e.stderr));
}

/// The layout source of the merge tests: two views with `at` lines.
const LAYOUT_DSL: &str = "workspace {
    model {
        a = person \"Admin\"
        s = softwareSystem \"Store\" \"Sells goods.\"
        a -> s \"Uses\"
    }
    views {
        systemContext s \"ctx\" \"Context\" {
            include *
            at a 300 200
            at s 300 600
        }
        systemLandscape \"land\" \"Landscape\" {
            include *
            at a 900 200
            at s 1500 200
        }
    }
}
";

/// FILE of the merge tests: the same elements under other identifiers
/// and descriptions, no `at` lines, a comment and an element that is not
/// in the layout, with its own position.
const MERGE_DSL: &str = "workspace {
    // The store and its users.
    model {
        admin = person \"Admin\" \"Runs the store.\"
        ops = person \"Operator\"
        store = softwareSystem \"Store\"
        admin -> store \"Uses\"
        ops -> store \"Watches\"
    }
    views {
        systemContext store \"ctx\" \"Context\" {
            include *
            at ops 1300 200
        }
        systemLandscape \"land\" \"Landscape\" {
            include *
        }
    }
}
";

/// A folder with `layout.json` (exported from `LAYOUT_DSL`) and
/// `file.dsl` (`MERGE_DSL`), and the paths of both.
fn merge_setup(name: &str) -> (Scratch, PathBuf, PathBuf) {
    let dir = Scratch::new(name);
    let source = dir.0.join("layout.dsl");
    fs::write(&source, LAYOUT_DSL).unwrap();
    export(&dir.0, &source, "json", &dir.0);
    let file = dir.0.join("file.dsl");
    fs::write(&file, MERGE_DSL).unwrap();
    let layout = dir.0.join("layout.json");
    (dir, layout, file)
}

/// `text` from the line holding `from` to the end.
fn from<'a>(text: &'a str, from: &str) -> &'a str {
    &text[text.find(from).unwrap_or_else(|| panic!("{from} in {text}"))..]
}

/// `text` without its `at`, `size` and `via` lines.
fn without_layout(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| !["at ", "size ", "via "].iter().any(|k| l.trim_start().starts_with(k)))
        .collect()
}

#[test]
fn p3_425_merge_writes_layout_copy() {
    let (dir, layout, file) = merge_setup("p3_425");
    // What `import-layout` adds to a copy of FILE is what `merge` writes.
    let copy = dir.0.join("copy.dsl");
    fs::write(&copy, MERGE_DSL).unwrap();
    let o = c4rizer(&["import-layout", layout.to_str().unwrap(), copy.to_str().unwrap()], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let imported = fs::read_to_string(&copy).unwrap();
    assert_ne!(imported, MERGE_DSL);
    let out = dir.0.join("out.dsl");
    let o = c4rizer(&["merge", "-w", "file.dsl", "-l", "layout.json", "-o", "out.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("wrote out.dsl"), "{}", text(&o.stdout));
    let merged = fs::read_to_string(&out).unwrap();
    assert_eq!(merged, imported);
    assert!(merged.contains("at admin "), "{merged}");
    // Long spellings, flags in any order.
    let o = c4rizer(&["merge", "--output", "long.dsl", "--layout", "layout.json", "--workspace", "file.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(fs::read_to_string(dir.0.join("long.dsl")).unwrap(), imported);
    // With `-v`, only that view changes.
    let o = c4rizer(&["merge", "-w", "file.dsl", "-l", "layout.json", "-v", "ctx", "-o", "ctx.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let ctx = fs::read_to_string(dir.0.join("ctx.dsl")).unwrap();
    assert_eq!(from(&ctx, "systemLandscape"), from(MERGE_DSL, "systemLandscape"), "land unchanged");
    let ctx_block = |t: &str| t[t.find("systemContext").unwrap()..t.find("systemLandscape").unwrap()].to_owned();
    assert_eq!(ctx_block(&ctx), ctx_block(&imported), "ctx as import-layout writes it");
    assert_ne!(ctx, MERGE_DSL);
    assert_eq!(fs::read_to_string(file).unwrap(), MERGE_DSL);
}

/// R1: hard links and symlinks to an input count as the input.
#[cfg(unix)]
#[test]
fn p3_426_merge_refuses_links_to_its_inputs() {
    let (dir, _, file) = merge_setup("p3_426_links");
    fs::hard_link(&file, dir.0.join("hard.dsl")).unwrap();
    std::os::unix::fs::symlink(&file, dir.0.join("soft.dsl")).unwrap();
    fs::write(dir.0.join("inc.dsl"), "workspace {\n    !include part.dsl\n}\n").unwrap();
    fs::write(dir.0.join("part.dsl"), "model {\n    a = person \"Admin\"\n}\n").unwrap();
    let layout = fs::read_to_string(dir.0.join("layout.json")).unwrap();
    for (input, out) in [
        ("file.dsl", "hard.dsl"),
        ("file.dsl", "soft.dsl"),
        ("file.dsl", "layout.json"),
        ("inc.dsl", "part.dsl"),
    ] {
        let o = c4rizer(&["merge", "-w", input, "-l", "layout.json", "-o", out], &dir.0);
        assert_eq!(o.status.code(), Some(2), "{out}: {}", text(&o.stderr));
        let e = text(&o.stderr);
        assert!(e.contains(out), "{out}: {e}");
        // Both paths are named: OUT, and the input it is.
        let named = if out == "part.dsl" || out == "layout.json" { out } else { input };
        assert!(e.contains("is an input") && e.matches(named).count() >= 1, "{out}: {e}");
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), MERGE_DSL);
    assert_eq!(fs::read_to_string(dir.0.join("layout.json")).unwrap(), layout);
    assert!(fs::read_to_string(dir.0.join("part.dsl")).unwrap().contains("Admin"));
    assert!(fs::symlink_metadata(dir.0.join("soft.dsl")).unwrap().file_type().is_symlink());
}

/// P3-430: `merge` refusals are exit 2, nothing written.
#[test]
fn p3_430_merge_refusals_exit_2() {
    let (dir, _, _) = merge_setup("p3_430_refusals");
    let o = c4rizer(&["merge", "-w", "layout.json", "-l", "layout.json", "-o", "out.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
    assert!(text(&o.stderr).contains("workspace JSON"), "{}", text(&o.stderr));
    let o = c4rizer(&["merge", "-w", "file.dsl", "-l", "layout.json", "-v", "nope", "-o", "out.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
    assert!(text(&o.stderr).contains("no view \"nope\""), "{}", text(&o.stderr));
    assert!(!dir.0.join("out.dsl").exists());
}

/// R1: `export` and `render` never write a file that is one of their
/// inputs.
#[test]
fn p3_400_export_never_writes_its_input() {
    let dir = Scratch::new("p3_400_inputs");
    let source = dir.copy("phase3/dynamic.dsl", "dynamic.dsl");
    export(&dir.0, &source, "json", &dir.0);
    let json = dir.0.join("dynamic.json");
    let bytes = fs::read(&json).unwrap();
    // `-f json` of `x.json` without `-o` would write `x.json`.
    let o = c4rizer(&["export", "-f", "json", json.to_str().unwrap()], &dir.0);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
    assert!(text(&o.stderr).contains("dynamic.json"), "{}", text(&o.stderr));
    assert_eq!(fs::read(&json).unwrap(), bytes);
    // `-f theme` of a file named `workspace-theme.json`.
    let theme = dir.0.join("workspace-theme.json");
    fs::copy(&source, &theme).unwrap();
    let o = c4rizer(&["export", "-f", "theme", theme.to_str().unwrap()], &dir.0);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
    assert_eq!(fs::read(&theme).unwrap(), fs::read(&source).unwrap());
    // A view's file that is the diagram file itself (`render`).
    let named = dir.0.join("seq.svg");
    fs::copy(&source, &named).unwrap();
    let o = c4rizer(&["render", named.to_str().unwrap(), "-o", dir.0.to_str().unwrap()], &dir.0);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
    assert_eq!(fs::read(&named).unwrap(), fs::read(&source).unwrap());
    // An included file, as a per-view text file.
    let inc = dir.0.join("inc");
    fs::create_dir_all(&inc).unwrap();
    fs::write(inc.join("main.dsl"), "workspace {\n    !include structurizr-ctx.mmd\n    views {\n        systemContext s \"ctx\" \"Ctx\" {\n            include *\n        }\n    }\n}\n").unwrap();
    let part = "model {\n    s = softwareSystem \"S\"\n}\n";
    fs::write(inc.join("structurizr-ctx.mmd"), part).unwrap();
    let o = c4rizer(&["export", "-f", "mermaid", inc.join("main.dsl").to_str().unwrap()], &dir.0);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
    assert_eq!(fs::read_to_string(inc.join("structurizr-ctx.mmd")).unwrap(), part);
}

/// P3-404 with CLI-1's guard: view keys equal up to case would overwrite
/// each other's files, so text and static exports refuse them too.
#[test]
fn p3_404_case_colliding_keys_refused() {
    let dir = Scratch::new("p3_404_case");
    let file = dir.0.join("case.dsl");
    fs::write(&file, "workspace {\n    model {\n        s = softwareSystem \"S\"\n    }\n    views {\n        systemContext s \"Ctx\" \"A\" {\n            include *\n        }\n        systemContext s \"ctx\" \"B\" {\n            include *\n        }\n    }\n}\n").unwrap();
    for format in ["mermaid", "plantuml", "dot", "static"] {
        let out = dir.0.join(format);
        let o = c4rizer(&["export", "-f", format, "-o", out.to_str().unwrap(), file.to_str().unwrap()], &dir.0);
        assert_eq!(o.status.code(), Some(2), "{format}: {}", text(&o.stderr));
        assert!(text(&o.stderr).contains("\"Ctx\" and \"ctx\""), "{format}: {}", text(&o.stderr));
        assert!(names(&out).is_empty(), "{format}");
    }
}

#[test]
fn p3_426_merge_never_changes_input() {
    let (dir, _, file) = merge_setup("p3_426");
    let before = fs::metadata(&file).unwrap().modified().unwrap();
    let o = c4rizer(&["merge", "-w", "file.dsl", "-l", "layout.json", "-o", "out.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(fs::read_to_string(&file).unwrap(), MERGE_DSL);
    assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), before);
    // OUT equals FILE except for `at`/`size`/`via` lines.
    let merged = fs::read_to_string(dir.0.join("out.dsl")).unwrap();
    assert_eq!(without_layout(&merged), without_layout(MERGE_DSL));
    assert_ne!(merged, MERGE_DSL);
    // OUT naming FILE is refused and FILE stays as it was.
    let o = c4rizer(&["merge", "-w", "file.dsl", "-l", "layout.json", "-o", "./file.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stdout));
    assert!(text(&o.stderr).contains("file.dsl"), "{}", text(&o.stderr));
    assert_eq!(fs::read_to_string(&file).unwrap(), MERGE_DSL);
}

#[test]
fn p3_427_merge_matches_canonical_names() {
    let (dir, _, _) = merge_setup("p3_427");
    let o = c4rizer(&["merge", "-w", "file.dsl", "-l", "layout.json", "-o", "out.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let merged = fs::read_to_string(dir.0.join("out.dsl")).unwrap();
    // `admin` and `store` match `Person://Admin` and
    // `SoftwareSystem://Store` of the layout, whatever their identifiers
    // and descriptions; in both views.
    let ctx = &merged[merged.find("systemContext").unwrap()..merged.find("systemLandscape").unwrap()];
    let land = from(&merged, "systemLandscape");
    for (block, name) in [(ctx, "ctx"), (land, "land")] {
        assert!(block.contains("at admin "), "{name}: {block}");
        assert!(block.contains("at store "), "{name}: {block}");
    }
    // The layout's positions differ per view, so do the merged ones.
    let at = |block: &str, id: &str| block.lines().find(|l| l.trim_start().starts_with(&format!("at {id} "))).unwrap().trim().to_owned();
    assert_ne!(at(ctx, "store"), at(land, "store"));
    // `ops` is not in the layout: its own position stays, and it gets
    // none where it had none.
    assert_eq!(at(ctx, "ops"), "at ops 1300 200");
    assert!(!land.contains("at ops"), "{land}");
}

/// Runs `generate system-landscape -i in -o land.dsl` in `dir` and
/// returns the written file.
fn generate(dir: &Path) -> String {
    let o = c4rizer(&["generate", "system-landscape", "-i", "in", "-o", "land.dsl"], dir);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("wrote land.dsl"), "{}", text(&o.stdout));
    let v = c4rizer(&["validate", "land.dsl"], dir);
    assert_eq!(v.status.code(), Some(0), "{}", text(&v.stderr));
    fs::read_to_string(dir.join("land.dsl")).unwrap()
}

#[test]
fn p3_428_generate_landscape() {
    let dir = Scratch::new("p3_428");
    let input = dir.0.join("in");
    fs::create_dir_all(&input).unwrap();
    // A DSL workspace: a person using a container of a system.
    fs::write(
        input.join("a.dsl"),
        "workspace {\n    model {\n        u = person \"Customer\" \"Buys goods.\"\n        shop = softwareSystem \"Shop\" {\n            web = container \"Web\"\n        }\n        u -> web \"Orders through\" \"HTTPS\"\n    }\n}\n",
    )
    .unwrap();
    // A JSON workspace naming the shop again and a payment system.
    let other = dir.0.join("b.dsl");
    fs::write(
        &other,
        "workspace {\n    model {\n        shop = softwareSystem \"Shop\" \"Sells goods.\"\n        pay = softwareSystem \"Payments\" \"Takes \\\"cards\\\".\"\n        shop -> pay \"Charges cards with\"\n        u = person \"Customer\"\n    }\n}\n",
    )
    .unwrap();
    export(&dir.0, &other, "json", &input);
    // Anything else in DIR is not read.
    fs::write(input.join("notes.txt"), "not a workspace {").unwrap();
    fs::create_dir_all(input.join("sub")).unwrap();
    fs::write(input.join("sub/c.dsl"), "not read either {").unwrap();
    let land = generate(&dir.0);
    assert_eq!(
        land,
        "workspace \"System Landscape\" {
    model {
        person_customer = person \"Customer\" \"Buys goods.\"
        system_shop = softwareSystem \"Shop\" \"Sells goods.\"
        system_payments = softwareSystem \"Payments\" \"Takes \\\"cards\\\".\"

        person_customer -> system_shop \"Orders through\" \"HTTPS\"
        system_shop -> system_payments \"Charges cards with\"
    }
    views {
        systemLandscape \"SystemLandscape\" \"System Landscape\" {
            include *
        }
    }
}
"
    );
    // OUT inside DIR is refused (approver ruling R2), and so is OUT
    // naming an input of DIR; nothing is written.
    let before = names(&input);
    for out in ["in/land.dsl", "in/a.dsl", "in/sub/new.dsl"] {
        let o = c4rizer(&["generate", "system-landscape", "-i", "in", "-o", out], &dir.0);
        assert_eq!(o.status.code(), Some(2), "{out}: {}", text(&o.stderr));
        assert!(text(&o.stderr).contains(out.rsplit('/').next().unwrap()), "{out}: {}", text(&o.stderr));
    }
    assert_eq!(names(&input), before);
    assert!(fs::read_to_string(input.join("a.dsl")).unwrap().contains("Customer"));
    assert_eq!(fs::read_to_string(dir.0.join("land.dsl")).unwrap(), land);
}

#[test]
fn p3_429_generate_keeps_first_description() {
    let dir = Scratch::new("p3_429");
    let input = dir.0.join("in");
    fs::create_dir_all(&input).unwrap();
    for (file, description) in [("1.dsl", ""), ("2.dsl", " \"Second.\""), ("3.dsl", " \"Third.\"")] {
        fs::write(
            input.join(file),
            format!("workspace {{\n    model {{\n        s = softwareSystem \"Shop\"{description}\n    }}\n}}\n"),
        )
        .unwrap();
    }
    let land = generate(&dir.0);
    assert!(land.contains("system_shop = softwareSystem \"Shop\" \"Second.\"\n"), "{land}");
    assert_eq!(land.matches("softwareSystem").count(), 1, "{land}");
}

#[test]
fn p3_430_merge_generate_exit_codes() {
    let (dir, _, _) = merge_setup("p3_430");
    let usage = |args: &[&str]| {
        let o = c4rizer(args, &dir.0);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", text(&o.stderr));
        assert!(text(&o.stderr).contains("usage:"), "{args:?}: {}", text(&o.stderr));
    };
    // Usage errors: exit 2 with the usage text.
    usage(&["merge", "-w", "file.dsl", "-o", "out.dsl"]);
    usage(&["merge", "-l", "layout.json", "-o", "out.dsl"]);
    usage(&["merge", "-w", "file.dsl", "-l", "layout.json"]);
    usage(&["merge", "-w", "file.dsl", "-l", "layout.json", "-o", "out.dsl", "--png"]);
    usage(&["merge", "-w", "file.dsl", "-w", "file.dsl", "-l", "layout.json", "-o", "out.dsl"]);
    usage(&["merge", "-w", "file.dsl", "-l", "layout.json", "-o", "out.dsl", "extra"]);
    usage(&["generate"]);
    usage(&["generate", "container-landscape", "-i", ".", "-o", "x.dsl"]);
    usage(&["generate", "system-landscape", "-o", "x.dsl"]);
    usage(&["generate", "system-landscape", "-i", "."]);
    usage(&["generate", "system-landscape", "-i", ".", "-o", "x.dsl", "-v", "k"]);
    // A parse error of any input: exit 1 naming the file.
    let parse_error = |args: &[&str], name: &str| {
        let o = c4rizer(args, &dir.0);
        assert_eq!(o.status.code(), Some(1), "{args:?}: {}", text(&o.stderr));
        assert!(text(&o.stderr).contains(name), "{args:?}: {}", text(&o.stderr));
    };
    fs::write(dir.0.join("broken.dsl"), "workspace {\n    model {\n        a = person\n").unwrap();
    fs::write(dir.0.join("broken.json"), "{\n  \"views\": [\n").unwrap();
    parse_error(&["merge", "-w", "broken.dsl", "-l", "layout.json", "-o", "out.dsl"], "broken.dsl:");
    parse_error(&["merge", "-w", "file.dsl", "-l", "broken.json", "-o", "out.dsl"], "broken.json:");
    assert!(!dir.0.join("out.dsl").exists(), "nothing written on an error");
    let landscape = dir.0.join("in");
    fs::create_dir_all(&landscape).unwrap();
    fs::copy(dir.0.join("file.dsl"), landscape.join("a.dsl")).unwrap();
    fs::copy(dir.0.join("broken.dsl"), landscape.join("b.dsl")).unwrap();
    parse_error(&["generate", "system-landscape", "-i", "in", "-o", "land.dsl"], "b.dsl:");
    fs::remove_file(landscape.join("b.dsl")).unwrap();
    fs::copy(dir.0.join("broken.json"), landscape.join("c.json")).unwrap();
    parse_error(&["generate", "system-landscape", "-i", "in", "-o", "land.dsl"], "c.json:");
    assert!(!dir.0.join("land.dsl").exists(), "nothing written on an error");
    // A missing input is an I/O error, exit 2.
    let o = c4rizer(&["merge", "-w", "missing.dsl", "-l", "layout.json", "-o", "out.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
    let o = c4rizer(&["generate", "system-landscape", "-i", "missing", "-o", "land.dsl"], &dir.0);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
}

#[test]
#[ignore = "deferred"]
fn p3_432_animation_export() {
    // Deferred until the animation work of dynamic-view (P3-7,
    // P3-222..P3-226) lands (docs/deferred.md): `--animation` is not
    // accepted yet, so this fails until then. The per-step file names of
    // P3-225 are checked when it is built.
    let dir = Scratch::new("p3_432");
    let file = dir.copy("phase3/dynamic.dsl", "dynamic.dsl");
    let out = dir.0.join("out");
    let o = c4rizer(&["export", "-f", "svg", "--animation", "-o", out.to_str().unwrap(), file.to_str().unwrap()], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert!(names(&out).len() > 1, "one image per step");
}

/// P3-419 with P3-420: a view that comes from a `workspace extends
/// <file>.json` base lives in DSL text written from the JSON;
/// `import-layout` refuses to write that text over the JSON.
#[test]
fn p3_419_import_layout_never_writes_a_json_base() {
    let dir = Scratch::new("p3_419");
    let base = dir.0.join("base.dsl");
    fs::write(
        &base,
        "workspace {\n    model {\n        a = person \"A\"\n        s = softwareSystem \"S\"\n        a -> s \"Uses\"\n    }\n    views {\n        systemContext s \"ctx\" {\n            include *\n        }\n    }\n}\n",
    )
    .unwrap();
    let o = c4rizer(&["export", "-f", "json", base.to_str().unwrap()], &dir.0);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let json = dir.0.join("base.json");
    let before = fs::read(&json).unwrap();
    let child = dir.0.join("child.dsl");
    fs::write(&child, "workspace extends base.json {\n}\n").unwrap();
    let layout = dir.0.join("layout.json");
    fs::write(
        &layout,
        r#"{"model": {"people": [{"id": "1", "name": "A"}], "softwareSystems": [{"id": "2", "name": "S"}]},
  "views": {"systemContextViews": [{"key": "ctx", "elements": [{"id": "1", "x": 100, "y": 100}, {"id": "2", "x": 100, "y": 600}]}]}}"#,
    )
    .unwrap();
    let o = c4rizer(&["import-layout", layout.to_str().unwrap(), child.to_str().unwrap()], &dir.0);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stdout));
    assert!(text(&o.stderr).contains("base.json"), "{}", text(&o.stderr));
    assert_eq!(fs::read(&json).unwrap(), before, "the JSON is unchanged");
}
