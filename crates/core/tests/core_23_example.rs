//! CORE-23 routed clauses on `tests/data/example.dsl` with every `at`
//! line removed, auto-placed then routed and labelled. These need
//! `place`, `route` and `label` together, so they live here and not in
//! place.rs (HLD: place depends on `view` and `geom` only). Owned by
//! `pipeline` (docs/deferred.md row): each view built with
//! `pipeline::build_spec` from `Keep::default()`. Pure-place clauses
//! (people on top row, outside boundary, pitch) are in place.rs.

/// CORE-23: no line of any view is unroutable.
#[test]
fn core_23_no_unroutable_line() {
    todo!()
}

/// CORE-23: no label of any view is without room.
#[test]
fn core_23_no_label_without_room() {
    todo!()
}

/// CORE-23: routed crossings per view within the per-view cap.
#[test]
#[ignore = "deferred"]
fn core_23_crossings_within_cap() {
    // TODO(srs): CORE-23 per-view crossing caps are unset; set them after
    // the first measurement (docs/deferred.md row, place, step 4).
    todo!()
}
