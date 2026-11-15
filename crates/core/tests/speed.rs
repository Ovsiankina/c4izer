//! Speed budgets of the core (SRS 5.2), run by the release perf oracle:
//! `cargo test --workspace --release -- --ignored perf_`. Every test is
//! `#[ignore]`d so a debug `cargo test` skips it; the timed view is `c3`
//! of `tests/data/example.dsl`, the heaviest (pipeline-03). A test whose
//! budget is not set yet has no `perf_` prefix and is
//! `#[ignore = "deferred"]`, so the perf oracle does not select it.

/// CORE-24: on view `c3` of the example, one box moved then a full
/// route (`pipeline::build_view` with that box in `Keep::moved`) takes
/// under 100 ms in a release build.
// TODO(srs): CORE-24 budget to be confirmed after the first release
// measurement (already open in SRS 4.2).
#[test]
#[ignore = "perf: release perf oracle only"]
fn perf_core_24_rerender_after_move_under_100ms() {
    todo!()
}

/// CORE-27: from file text to the routed scene of view `c3`
/// (`model::parse` then `pipeline::build_view`) takes under 300 ms in
/// a release build.
// TODO(srs): 300 ms to be confirmed after the first release measurement
// (already open in SRS 4.2).
#[test]
#[ignore = "perf: release perf oracle only"]
fn perf_core_27_first_view_under_300ms() {
    todo!()
}

/// EDIT-29: the core's share of a reload of `tests/data/example.dsl`
/// after a save from another program: `model::parse` of the new text,
/// `pipeline::keep_across_reparse` from the scene on screen, then
/// `pipeline::build_view` of `c3` with that `Keep`, under 200 ms in a
/// release build. The file-event latency and the hand-off to the canvas
/// outside the core and left to the editor contract (not timed here).
#[test]
#[ignore = "perf: release perf oracle only"]
fn perf_edit_29_reload_view_under_200ms() {
    todo!()
}

/// CORE-31: the generated 40-box, 60-line view (fixed seed, checked in
/// under `tests/data/`) routes under its budget in a release build. Not
/// `perf_`-prefixed until the budget exists, so the perf oracle stays
/// green (docs/deferred.md row).
// TODO(srs): CORE-31 budget and the generated file are not set yet
// (SRS 4.2: "measure once and write the budget"; docs/deferred.md row).
#[test]
#[ignore = "deferred"]
fn core_31_generated_view_under_budget() {
    todo!()
}
