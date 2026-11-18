//! Tests of the explore row, core part (SRS 4.7: P3-500..P3-517). One
//! `#[test]` per requirement, named after it. The fixture is
//! `tests/data/phase3/explore.dsl`. The editor tests are in
//! `crates/editor/src/explore_tests.rs`.

/// P3-501: an element's relationships include the implied ones, marked.
#[test]
fn p3_501_element_relationships_mark_implied() {
    todo!()
}

/// P3-502: the rows hold every written, copied and implied relationship.
#[test]
fn p3_502_relationship_rows_include_implied_and_copied() {
    todo!()
}

/// P3-503: search matches name, id, description, technology, tag and
/// property value, ignoring case.
#[test]
fn p3_503_search_matches_every_field_case_insensitively() {
    todo!()
}

/// P3-503: a search over a 1000-element model takes under one frame.
#[test]
#[ignore = "perf: run with --release -- --ignored perf_"]
fn perf_p3_503() {
    todo!()
}

/// P3-504: usages name each view drawing the element, directly or lifted.
#[test]
fn p3_504_usages_direct_and_lifted() {
    todo!()
}

/// P3-505: "in no view" is what `model.element.noview` reports.
#[test]
fn p3_505_in_no_view_matches_inspection() {
    todo!()
}

/// P3-508: the force layout gives the same positions on every run.
#[test]
fn p3_508_graph_layout_is_deterministic() {
    todo!()
}

/// P3-510: static and deployment trees.
#[test]
fn p3_510_tree_static_and_deployment() {
    todo!()
}

/// P3-511: graph and tree are offered for Structurizr's kinds only.
#[test]
fn p3_511_graph_and_tree_offered_kinds() {
    todo!()
}

/// P3-516: decisions parse from adr-tools and MADR files.
#[test]
fn p3_516_decision_parsed_from_adr_tools_and_madr() {
    todo!()
}
