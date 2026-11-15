//! CORE-23 routed clauses on `tests/data/example.dsl` with every `at`
//! line removed, auto-placed then routed and labelled. These need
//! `place`, `route` and `label` together, so they live here and not in
//! place.rs (HLD: place depends on `view` and `geom` only). Owned by
//! `pipeline` (docs/deferred.md row): each view built with
//! `pipeline::build_spec` from `Keep::default()`. Pure-place clauses
//! (people on top row, outside boundary, pitch) are in place.rs.

use std::sync::atomic::AtomicBool;

use c4rizer_core::model::parse;
use c4rizer_core::pipeline::{ViewBuild, build_spec};
use c4rizer_core::place::Keep;

const EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/data/example.dsl"
));

/// The example with every `at` line removed (`size` lines kept).
fn unplaced_example() -> String {
    let mut out = String::with_capacity(EXAMPLE.len());
    for l in EXAMPLE
        .lines()
        .filter(|l| !l.trim_start().starts_with("at "))
    {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Every view of the unplaced example, built from `Keep::default()`, in
/// declaration order, with its key.
fn builds() -> Vec<(String, ViewBuild)> {
    let model = match parse(&unplaced_example()) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    };
    let stop = AtomicBool::new(false);
    model
        .views
        .iter()
        .map(|v| {
            assert!(v.positions.is_empty(), "{}: an `at` line is left", v.key);
            let b = build_spec(&model, v, &Keep::default(), &stop).expect("never stopped");
            assert_eq!(b.plan.unplaced().len(), b.plan.boxes.len(), "{}", v.key);
            (v.key.clone(), b)
        })
        .collect()
}

/// CORE-23: no line of any view is unroutable.
#[test]
fn core_23_no_unroutable_line() {
    let all = builds();
    assert_eq!(all.len(), 4);
    for (key, b) in &all {
        assert!(
            b.routes.unroutable().is_empty(),
            "{key}: unroutable {:?}",
            b.routes.unroutable()
        );
        assert!(b.scene.unroutable().is_empty(), "{key}: scene unroutable");
    }
}

/// CORE-23: no label of any view is without room.
#[test]
fn core_23_no_label_without_room() {
    for (key, b) in &builds() {
        assert!(
            b.labels.no_room().is_empty(),
            "{key}: labels without room {:?}",
            b.labels.no_room()
        );
    }
}

/// CORE-23: routed crossings per view within the per-view cap (C1 0, C2
/// 0, C3-serving 1, C3-startup 0: the prototype's routed crossings on its
/// hand layout, SRS CORE-23).
#[test]
fn core_23_crossings_within_cap() {
    let caps = [("C1", 0), ("C2", 0), ("C3-serving", 1), ("C3-startup", 0)];
    let all = builds();
    assert_eq!(all.len(), caps.len());
    for ((key, b), (cap_key, cap)) in all.iter().zip(caps) {
        assert_eq!(key, cap_key);
        assert_eq!(b.routes.crossings, b.scene.crossings, "{key}");
        assert!(
            b.routes.crossings <= cap,
            "{key}: {} crossings after auto-placement, cap {cap}",
            b.routes.crossings
        );
    }
}
