//! Golden check 1 (CLI-5, CORE-11) on `tests/data/example.dsl`:
//! `pipeline::build_all` compared structurally with the prototype SVGs in
//! `tests/data/example/*.svg` (pipeline-02). Golden check 2 (byte-identical
//! against `tests/data/golden/`) is the CLI's `cli_5_example_golden`.

/// CLI-5 (1), CORE-11: every example view has the reference's box ids,
/// the same edge pairs, the same joined lines, a label wherever the
/// reference has one, and crossings no higher than the reference.
#[test]
fn golden_example_structure_matches_reference() {
    todo!()
}
