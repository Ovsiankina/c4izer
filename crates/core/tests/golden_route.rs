//! Router quality bar on `tests/data/example.dsl` (CORE-11, CORE-12,
//! route-06). No `perf_*` here: CORE-24, 27 and 31 belong to pipeline.

/// CORE-11: every view of the example routes with crossings no higher
/// than the prototype's: C1 0, C2 0, C3 <= 1, C3-startup 0.
#[test]
fn core_11_example_crossings_no_worse() {
    todo!()
}

/// CORE-12: every view of the example has a summed lane length
/// (`Routes::lanes`) no lower than the prototype's.
// TODO(srs): the per-view numbers are not in the SRS (docs/deferred.md).
#[test]
#[ignore = "deferred"]
fn core_12_example_lane_length_no_worse() {
    todo!()
}
