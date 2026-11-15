//! Prints the per-view report: boxes, lines, crossings against budget,
//! warnings and errors (CLI-1).
//!
//! Depends on: `c4rizer-core` (lint).

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
// is; lint owns the wording (lint.rs header `TODO(srs)`), the report only
// adds the prefix. Lines end in `\n` on every platform (CORE-25 spirit:
// same bytes everywhere).
// DECISION: `wrote <path>` prints the path as `main` built it
// (`<out dir>/<key>.svg`, `Path::display`), not canonicalized, like the
// prototype's `Path(args.out) / f'{key}.svg'`.

use std::io::{self, Write};
use std::path::Path;

use c4rizer_core::lint::{Finding, Report};

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
    todo!()
}

/// Writes `   wrote <path>` for one file just written (CLI-1, CLI-4).
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn wrote(out: &mut impl Write, path: &Path) -> io::Result<()> {
    todo!()
}

/// Writes one unindented `ERROR: <text>` per L22 finding, in the order
/// given (`pipeline::AllViews::coverage`, by relationship id), after all
/// views (CLI-1). Writes nothing for an empty slice. Serves CLI-1, CORE-21.
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn coverage(out: &mut impl Write, findings: &[Finding]) -> io::Result<()> {
    todo!()
}

/// Writes `no views` for a file without views (pipeline plan: the CLI
/// reports "no views"; exit 0). Serves CLI-1, CLI-2.
///
/// # Errors
///
/// The first error of `out`.
pub(crate) fn no_views(out: &mut impl Write) -> io::Result<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    /// CLI-1: on `tests/data/example.dsl` (read from
    /// `concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl")`,
    /// through `pipeline::build_all`),
    /// `view` + `wrote` + `coverage` print, per view in declaration order,
    /// `== <key>: <title>`, the two count lines indented by three spaces,
    /// warnings then errors, `wrote <path>`, and L22 lines unindented after
    /// all views: the same line shapes as the prototype's output.
    #[test]
    fn cli_1_report_matches_prototype_format() {
        todo!()
    }

    /// Header DECISION: a view without a title prints `== <key>: ` and
    /// nothing else on that line.
    #[test]
    fn cli_1_untitled_view_header() {
        todo!()
    }

    /// CLI-1: `coverage` of an empty slice writes nothing; of two findings
    /// writes two unindented `ERROR: ` lines in the given order.
    #[test]
    fn cli_1_coverage_lines_unindented() {
        todo!()
    }

    /// CLI-1: on an inline fixture with a container that has a component
    /// view and a relationship between two of its components that is on
    /// no component view, `coverage` of `pipeline::build_all(..).coverage`
    /// writes exactly the one line
    /// `ERROR: <src> -> <dst> is in the model but on no component view of "<scope>"\n`,
    /// full text compared. `<src>`, `<dst>`, `<scope>` are filled as lint
    /// renders them (names or ids: docs/deferred.md lint row on L22).
    #[test]
    fn cli_1_coverage_line_exact_text() {
        todo!()
    }
}
