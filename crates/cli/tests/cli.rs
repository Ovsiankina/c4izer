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

/// CLI-3: `c4rizer lint <example.dsl>`, run in an empty temporary
/// directory, prints the same report as `render` minus the `wrote` lines,
/// exits with the same code, leaves the directory empty, and leaves
/// FILE's bytes unchanged.
#[test]
fn cli_3_lint_writes_nothing() {
    todo!()
}

/// CLI-4: `render ... -o DIR --png` writes `<view key>.png` beside each
/// `<view key>.svg`, a valid PNG twice the SVG width; with `--width 400`
/// it is 400px wide.
// Ignored until docs/deferred.md rows "cli ... png feature" (this crate
// forwards `c4rizer-core/png`) and "scene/export" (FONT_BYTES) are done.
#[test]
#[ignore = "deferred"]
fn cli_4_png_written() {
    todo!()
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
    todo!()
}

/// CLI-1: `render` writes exactly one `<view key>.svg` per view of the
/// file and nothing else into DIR, prints one `wrote <path>` line per
/// file, and leaves FILE's bytes unchanged.
#[test]
fn cli_1_render_writes_one_svg_per_view() {
    todo!()
}

/// CLI-2 end to end: the binary exits 0 on a file whose views lint clean,
/// 1 on one with a lint error (inline fixtures written to a temporary
/// directory), and 2 on a missing FILE, on `render` without `-o` and on
/// an unknown flag (usage on stderr, nothing on stdout).
#[test]
fn cli_2_binary_exit_codes() {
    todo!()
}
