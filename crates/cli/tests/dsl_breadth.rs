//! Step-4 contract tests of the dsl-breadth row, CLI part (P2-122,
//! P2-125, P2-136): end-to-end on `env!("CARGO_BIN_EXE_c4rizer")` like
//! `cli.rs`. Bodies are `todo!()` until step 5.

#![allow(clippy::todo)]

/// `render` and `lint` print `== <key>: <title>` then a three-space
/// indented line containing `not yet supported` for an unsupported view,
/// write no file for it and add nothing to the exit code (CLI-1, CLI-2).
#[test]
fn p2_122_cli_prints_not_yet_supported() {
    // TODO(srs): P2-122 the exact line text. Reading taken (B dsl-breadth
    // 24): `   not yet supported: <keyword> views are not drawn by this
    // version`.
    todo!()
}

/// `render` and `lint` print every notice before the first view, one
/// line each naming construct, file and line; notices add nothing to the
/// exit code.
#[test]
fn p2_125_cli_prints_notices_before_views() {
    // TODO(srs): P2-125 the exact line format. Reading taken (B
    // dsl-breadth 24): `notice: <path>:<line>:<column>: <construct> is
    // accepted and has no effect`.
    todo!()
}

/// A parse or model error in an included file is reported with that
/// file's path, line and column (SRS 5.1).
#[test]
fn p2_136_cli_error_names_included_file() {
    todo!()
}
