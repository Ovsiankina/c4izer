//! Step-4 contract tests of the styles-themes row, CLI part (P2-432):
//! end-to-end on `env!("CARGO_BIN_EXE_c4rizer")` like `cli.rs`. Bodies
//! are `todo!()` until step 5.

#![allow(clippy::todo)]

/// `c4rizer render FILE -o DIR --mode dark` writes the SVGs (and the PNGs
/// with `--png`) built with `pipeline::build_all_in(.., Mode::Dark, ..)`.
#[test]
fn p2_432_render_mode_dark() {
    todo!()
}

/// With no `--mode`, `render` writes the same bytes as `--mode light`
/// (Structurizr `-mode`, default light).
#[test]
fn p2_432_mode_defaults_light() {
    todo!()
}

/// `--mode` with any value other than `light` or `dark`, or with no
/// value, is a usage error: exit 2, nothing written.
#[test]
fn p2_432_bad_mode_usage_error() {
    todo!()
}
