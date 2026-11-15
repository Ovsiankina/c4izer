//! End-to-end checks on every view of `tests/data/example.dsl` through
//! `pipeline::build_all`: the requirements that need place, route, label,
//! scene and export together, so no single stage can own them
//! (docs/deferred.md rows for CORE-25 and CORE-30).

/// CORE-25: each example view built twice gives byte-identical SVG
/// (`export::svg`).
#[test]
fn core_25_example_svg_bytes_identical() {
    todo!()
}

/// CORE-30: every `TextBlock::fits` is true on every example view (no
/// L25 finding).
#[test]
fn core_30_example_texts_fit() {
    todo!()
}
