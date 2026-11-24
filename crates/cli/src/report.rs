//! Prints the per-view report: boxes, lines, crossings against budget,
//! warnings and errors (CLI-1); the element list of `list` (P2-608) and
//! the violations of `inspect` (P2-611).
//!
//! Depends on: `c4rizer-core` (lint, model notices, pipeline's
//! unsupported views).

// Shape: plain functions writing lines to any `io::Write`, in the order
// CLI-1 gives, so `main::run` calls them as it goes and the unit test
// checks bytes. Report input is `lint::Report` plus the drawn title, so
// this module needs `lint` only (its `//!` line); `main` takes the title
// from `pipeline::ViewBuild::plan.title`.
//
// One view's block, exactly as the prototype `c4.py` prints it:
//
//   == <key>: <title>
//      <n> boxes, <m> lines, <c> crossings (budget <b>)
//      alignment: <r> rows, <k> columns for <n> boxes
//      warning: <text>          (one per warning, report order)
//      ERROR: <text>            (one per error, report order)
//      wrote <path>             (render only, one per file written)
//
// then, after every view, one unindented `ERROR: <text>` per L22 finding.
//
// DECISION: the `warning:`, `ERROR:` and `wrote` lines are indented by
// three spaces like the two count lines. CLI-1 says "indented by three
// spaces" right after the header and lists the rest in the same breath;
// the prototype indents all of them, and only the L22 lines (which CLI-1
// puts "after all views") are unindented.
// DECISION: `<title>` is the drawn title, `pipeline::ViewBuild::plan.title`,
// description fallback included (view.rs), so a view with a description
// and no `title` line prints its description, where the prototype prints
// `spec.get("title", "")` (empty). The report shows what the SVG shows.
// DECISION: a view with no title prints `== <key>: ` (the prototype
// prints `spec.get("title", "")`); lint reports it as L8.
// DECISION: `<text>` is the finding's `Display` (`Finding::message`) as
// is; lint owns the wording (CORE-16, lint.rs header), the report only
// adds the prefix. Lines end in `\n` on every platform (CORE-25 spirit:
// same bytes everywhere).
// DECISION: `wrote <path>` prints the path as `main` built it
// (`<out dir>/<key>.svg`, `Path::display`), not canonicalized, like the
// prototype's `Path(args.out) / f'{key}.svg'`.

use std::io::{self, Write};
use std::path::Path;

use c4rizer_core::import::Import;
use c4rizer_core::inspect::{Subject, Violation};
use c4rizer_core::lint::{Finding, Report};
use c4rizer_core::model::{Model, Notice, NoticeKind};
use c4rizer_core::pipeline::UnsupportedView;

/// Writes one view's block (CLI-1): `== <key>: <title>` with `key` =
/// `report.view` and `title` empty when `None`; then, indented by three
/// spaces, `<n> boxes, <m> lines, <c> crossings (budget <b>)` and
/// `alignment: <r> rows, <k> columns for <n> boxes` from `report.stats`,
/// one `warning: <text>` per `report.warnings()`, one `ERROR: <text>` per
/// `report.errors()`. The `wrote` lines are `wrote`'s. Serves CLI-1,
/// CLI-3.
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn view(out: &mut impl Write, report: &Report, title: Option<&str>) -> io::Result<()> {
    let s = &report.stats;
    writeln!(out, "== {}: {}", report.view, title.unwrap_or(""))?;
    writeln!(
        out,
        "   {} boxes, {} lines, {} crossings (budget {})",
        s.boxes, s.lines, s.crossings, s.budget
    )?;
    // DECISION: `for <n> boxes` is `stats.boxes`; the prototype counts the
    // same boxes it placed, and `Stats` keeps no second box count.
    writeln!(
        out,
        "   alignment: {} rows, {} columns for {} boxes",
        s.rows, s.columns, s.boxes
    )?;
    for w in report.warnings() {
        writeln!(out, "   warning: {w}")?;
    }
    for e in report.errors() {
        writeln!(out, "   ERROR: {e}")?;
    }
    Ok(())
}

/// Writes `   wrote <path>` for one file just written (CLI-1, CLI-4).
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn wrote(out: &mut impl Write, path: &Path) -> io::Result<()> {
    writeln!(out, "   wrote {}", path.display())
}

/// `== <key>: skipped (<format> does not support <kind> views)`: a view
/// `export` cannot write in that format (P3-405).
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn skipped(out: &mut impl Write, key: &str, format: &str, kind: &str) -> io::Result<()> {
    writeln!(out, "== {key}: skipped ({format} does not support {kind} views)")
}

/// Writes one unindented `ERROR: <text>` per L22 finding, in the order
/// given (`pipeline::AllViews::coverage`, by relationship id), after all
/// views (CLI-1). Writes nothing for an empty slice. Serves CLI-1, CORE-21.
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn coverage(out: &mut impl Write, findings: &[Finding]) -> io::Result<()> {
    for f in findings {
        writeln!(out, "ERROR: {f}")?;
    }
    Ok(())
}

/// Writes one notice line (P2-125): `notice: <file>:<line>:<column>:
/// <construct> is accepted and has no effect`, `file` being the path of
/// the notice's file as the CLI names it; without a span, `notice:
/// <construct> is accepted and has no effect`. Serves P2-125.
// TODO(srs): P2-125, the exact line format. Reading taken (B dsl-breadth
// 24): `notice: <path>:<line>:<column>: <construct> is accepted and has
// no effect`.
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn notice(out: &mut impl Write, notice: &Notice, file: Option<&str>) -> io::Result<()> {
    match file {
        Some(file) => writeln!(out, "notice: {file}:{notice}"),
        None => writeln!(out, "notice: {notice}"),
    }
}

/// Writes an unsupported view's block (P2-122): `== <key>: <title>` as
/// for a drawn view, then `   ` and the `UnsupportedView` line. No count
/// lines, no findings, no file. Serves P2-122, CLI-1.
// TODO(srs): P2-122, the exact line text. Reading taken (B dsl-breadth
// 24): `not yet supported: <keyword> views are not drawn by this
// version`, from `UnsupportedView`'s `Display`.
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn unsupported(out: &mut impl Write, view: &UnsupportedView) -> io::Result<()> {
    writeln!(out, "== {}: {}", view.key, view.title.as_deref().unwrap_or(""))?;
    writeln!(out, "   {view}")
}

/// Writes `no views` for a file without views (pipeline plan: the CLI
/// reports "no views"; exit 0). Serves CLI-1, CLI-2.
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn no_views(out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "no views")
}

/// Writes one line per element of `model` in declaration order
/// (`Model::elements`, parents before children): `<keyword> <id>
/// "<name>"`, indented two spaces per nesting level. Serves P2-608.
// TODO(srs): P2-608, the line format. Reading taken (B cli-breadth 4):
// `<keyword> <id> "<name>"`, indented two spaces per level. Structurizr
// prints ` - <Type>://<canonical name>` sorted by name (deferred.md).
// DECISION: `<id>` is `Model::full_key` (the dotted id under
// `!identifiers hierarchical`); an element declared without an id prints
// `<keyword> "<name>"`. `"` and `\` in the name are escaped with `\` as
// the DSL writes them, so the line reads back as one quoted string.
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn elements(out: &mut impl Write, model: &Model) -> io::Result<()> {
    for element in &model.elements {
        let mut depth = 0;
        let mut parent = element.parent;
        while let Some(p) = parent {
            depth += 1;
            parent = model.element(p).parent;
        }
        write!(out, "{:width$}{}", "", element.kind.keyword(), width = depth * 2)?;
        if let Some(id) = model.full_key(element.id) {
            write!(out, " {id}")?;
        }
        writeln!(out, " {}", Quoted(&element.name.value))?;
    }
    Ok(())
}

/// `Display` of a name in double quotes, `"` and `\` escaped with `\`.
struct Quoted<'a>(&'a str);

impl std::fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("\"")?;
        for c in self.0.chars() {
            if matches!(c, '"' | '\\') {
                f.write_str("\\")?;
            }
            write!(f, "{c}")?;
        }
        f.write_str("\"")
    }
}

/// Writes one line per violation, `<severity> <key> <subject>`, then
/// `<n> violations shown` (P2-611, P2-612). The subject is an element's
/// id, a relationship's `<source id> -> <destination id>`, a view's key,
/// an element style's tag in double quotes, or nothing (no trailing
/// space) for a workspace-level violation.
// TODO(srs): P2-611, the line format and summary. Reading taken (B
// cli-breadth 4): `<severity> <key> <subject>` per violation, then `<n>
// violations shown`. Structurizr prints `<SEVERITY> | <key> | <message>`
// in padded columns and no summary (deferred.md, differences).
// DECISION: an element without an id is named by its name in double
// quotes, as `elements` would print it; a style's tag is quoted because a
// tag may hold spaces.
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn violations(out: &mut impl Write, model: &Model, shown: &[Violation]) -> io::Result<()> {
    let element = |id| {
        model
            .full_key(id)
            .map_or_else(|| Quoted(&model.element(id).name.value).to_string(), c4rizer_core::model::ElementKey::into_string)
    };
    for v in shown {
        write!(out, "{} {}", v.severity.keyword(), v.inspection.key())?;
        match &v.subject {
            Subject::Element(id) => write!(out, " {}", element(*id))?,
            Subject::Relationship(id) => {
                let r = model.relationship(*id);
                write!(out, " {} -> {}", element(r.source.value), element(r.destination.value))?;
            }
            Subject::View(key) => write!(out, " {key}")?,
            Subject::ElementStyle(tag) => write!(out, " {}", Quoted(tag))?,
            _ => {}
        }
        writeln!(out)?;
    }
    writeln!(out, "{} violations shown", shown.len())
}

/// The `import-layout` report (P2-15, P2-504; plan task 08 DECISION):
/// per matched view `== <key>`, `   wrote <n> position(s)` and its
/// notices as `   warning: <notice>`; one `skipped <key>: <why>` per
/// skipped view; then `warning: <notice>` for every other notice.
///
/// # Errors
///
/// Any write error of `out`.
pub(crate) fn imported(out: &mut impl Write, import: &Import) -> io::Result<()> {
    for view in &import.views {
        writeln!(out, "== {}", view.key)?;
        let plural = if view.positions == 1 { "" } else { "s" };
        writeln!(out, "   wrote {} position{plural}", view.positions)?;
        for notice in import.notices.iter().filter(|n| n.view.as_deref() == Some(view.key.as_str())) {
            if !is_skip(notice) {
                writeln!(out, "   warning: {notice}")?;
            }
        }
    }
    for notice in import.notices.iter().filter(|n| is_skip(n)) {
        let why = if notice.kind == NoticeKind::ViewNotInFile {
            "no view with this key in FILE"
        } else {
            "c4rizer does not draw or position this view yet"
        };
        writeln!(out, "skipped {}: {why}", notice.subject)?;
    }
    let matched = |n: &Notice| import.views.iter().any(|v| n.view.as_deref() == Some(v.key.as_str()));
    for notice in import.notices.iter().filter(|n| !is_skip(n) && !matched(n)) {
        writeln!(out, "warning: {notice}")?;
    }
    Ok(())
}

/// A notice that says a whole view was skipped.
fn is_skip(notice: &Notice) -> bool {
    matches!(notice.kind, NoticeKind::ViewNotInFile | NoticeKind::ViewNotDrawable)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    use c4rizer_core::model;
    use c4rizer_core::pipeline::{self, AllViews};

    use super::{coverage, no_views, view, wrote};

    const EXAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl");

    /// One container whose only component view hides two relationships
    /// between its components: two L22 findings, in relationship order.
    // DECISION: `hide` takes the relationships off the only component
    // view, the smallest file that gives L22 through the pipeline.
    const HIDDEN: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods." {
      front = container "Front" "Shows pages." "Rust" {
        pages = component "Pages" "Renders pages." "Rust"
        cart = component "Cart" "Holds the cart." "Rust"
        tally = component "Tally" "Sums the cart." "Rust"
      }
    }
    operator -> pages "Browses"
    pages -> cart "Fills"
    cart -> tally "Asks totals from"
  }
  views {
    component front "front" "Front components" {
      include *
      hide pages -> cart
      hide cart -> tally
    }
  }
}
"#;

    fn build(source: &str) -> AllViews {
        let parsed = model::parse(source).expect("fixture parses");
        pipeline::build_all(&parsed.model, &AtomicBool::new(false)).expect("never stopped")
    }

    fn text(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("report is UTF-8")
    }

    /// CLI-1: on `tests/data/example.dsl` (read from
    /// `concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl")`,
    /// through `pipeline::build_all`),
    /// `view` + `wrote` + `coverage` print, per view in declaration order,
    /// `== <key>: <title>`, the two count lines indented by three spaces,
    /// warnings then errors, `wrote <path>`, and L22 lines unindented after
    /// all views: the same line shapes as the prototype's output.
    #[test]
    fn cli_1_report_matches_prototype_format() {
        let source = std::fs::read_to_string(EXAMPLE).expect("example readable");
        let all = build(&source);
        let mut out = Vec::new();
        for v in &all.views {
            view(&mut out, &v.report, v.plan.title.as_deref()).unwrap();
            wrote(
                &mut out,
                &Path::new("out").join(format!("{}.svg", v.plan.key)),
            )
            .unwrap();
        }
        coverage(&mut out, &all.coverage).unwrap();
        let got = text(out);

        // The expected text, built line by line from the prototype's
        // f-strings (c4.py `main` and `lint`).
        let mut want = String::new();
        for v in &all.views {
            let s = &v.report.stats;
            let title = v.plan.title.as_deref().unwrap_or("");
            writeln!(want, "== {}: {title}", v.plan.key).unwrap();
            writeln!(
                want,
                "   {} boxes, {} lines, {} crossings (budget {})",
                s.boxes, s.lines, s.crossings, s.budget
            )
            .unwrap();
            writeln!(
                want,
                "   alignment: {} rows, {} columns for {} boxes",
                s.rows, s.columns, s.boxes
            )
            .unwrap();
            for w in v.report.warnings() {
                writeln!(want, "   warning: {}", w.message).unwrap();
            }
            for e in v.report.errors() {
                writeln!(want, "   ERROR: {}", e.message).unwrap();
            }
            writeln!(want, "   wrote out/{}.svg", v.plan.key).unwrap();
        }
        for f in &all.coverage {
            writeln!(want, "ERROR: {}", f.message).unwrap();
        }
        assert_eq!(got, want);

        // Fixed facts of the example, independent of the builder above.
        let keys: Vec<&str> = got
            .lines()
            .filter_map(|l| l.strip_prefix("== "))
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(keys, ["C1", "C2", "C3-serving", "C3-startup"]);
        assert!(got.starts_with("== C1: System Context diagram for Atomanja\n   "));
        for line in got.lines() {
            assert!(
                line.starts_with("== ") || line.starts_with("   ") || line.starts_with("ERROR: "),
                "unexpected line shape: {line:?}"
            );
        }
    }

    /// Header DECISION: a view without a title prints `== <key>: ` and
    /// nothing else on that line.
    #[test]
    fn cli_1_untitled_view_header() {
        let all = build(HIDDEN);
        let mut out = Vec::new();
        view(&mut out, &all.views[0].report, None).unwrap();
        let got = text(out);
        assert_eq!(got.lines().next(), Some("== front: "));
    }

    /// CLI-1: `coverage` of an empty slice writes nothing; of two findings
    /// writes two unindented `ERROR: ` lines in the given order.
    #[test]
    fn cli_1_coverage_lines_unindented() {
        let mut out = Vec::new();
        coverage(&mut out, &[]).unwrap();
        assert!(out.is_empty());

        let all = build(HIDDEN);
        assert_eq!(all.coverage.len(), 2, "{:?}", all.coverage);
        coverage(&mut out, &all.coverage).unwrap();
        let got = text(out);
        let want = format!(
            "ERROR: {}\nERROR: {}\n",
            all.coverage[0].message, all.coverage[1].message
        );
        assert_eq!(got, want);
        let mut reversed = Vec::new();
        let swapped = [all.coverage[1].clone(), all.coverage[0].clone()];
        coverage(&mut reversed, &swapped).unwrap();
        assert_ne!(text(reversed), got, "order follows the slice");
    }

    /// CLI-1: on an inline fixture with a container that has a component
    /// view and a relationship between two of its components that is on
    /// no component view, `coverage` of `pipeline::build_all(..).coverage`
    /// writes exactly the one line
    /// `ERROR: <src> -> <dst> is in the model but on no component view of "<scope>"\n`,
    /// full text compared. `<src>`, `<dst>`, `<scope>` are filled as lint
    /// renders them: element names (CORE-16, CORE-21).
    #[test]
    fn cli_1_coverage_line_exact_text() {
        let source = HIDDEN.replace("      hide cart -> tally\n", "");
        let all = build(&source);
        let mut out = Vec::new();
        coverage(&mut out, &all.coverage).unwrap();
        assert_eq!(
            text(out),
            "ERROR: Pages -> Cart is in the model but on no component view of \"Front\"\n"
        );
    }

    /// Pipeline plan: a file without views reports `no views`.
    #[test]
    fn cli_1_no_views_line() {
        let mut out = Vec::new();
        no_views(&mut out).unwrap();
        assert_eq!(text(out), "no views\n");
    }
}

