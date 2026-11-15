//! Automatic placement: gives a grid position to every box of a view
//! plan that has none, never moving a placed box, scoring candidates by
//! straight-line crossings (CORE-22, 23).
//!
//! Depends on: `view`, `geom`.

// Shape of the API: one function, `place(&ViewPlan, &Keep) -> Vec<Point>`,
// returning the centre of every box in grid units, indexed by `BoxId`
// (`centres[i]` is the centre of `plan.boxes[i]`). That vector is the
// `&[Point]` that `route`, `label`, `scene` and `ViewPlan::boundary_rect`
// take next to the plan. The plan stays immutable (view.rs header).
//
// Which centre a box gets, first match wins:
//   1. `Keep::moved`: a centre the editor sets while dragging (EDIT-2,
//      EDIT-20 several boxes) or after an auto-placed box was dragged
//      (EDIT-4);
//   2. its `at` line, `PlanBox::centre` (CORE-22: never moved);
//   3. `Keep::previous`: its centre in the previous scene of the same
//      session, matched by `PlanBox::element` (CORE-29);
//   4. otherwise it is auto-placed on candidate slots of a coarse grid
//      (CORE-23 pitch), clear of every box from 1..3 with L6 clearance,
//      scored by straight-line crossings then total edge length (place
//      plan, SKILL.md "Placing boxes").
// The CLI and a fresh build pass `Keep::default()`; the editor passes
// the last scene's centres (`Keep::from_previous`) plus the dragged
// boxes. When every box is covered by 1..3 nothing is searched, so the
// drag preview stays cheap (EDIT-25).
//
// DECISION: slot pitch is private and tunable (34 x 28 units = 340px x
// 280px, CORE-23 "at least"); it is not pub because no other module
// needs it and `at`/previous boxes are not bound by it.
// DECISION: `previous` is keyed by `ElementId` because view.rs names
// `PlanBox::element` the identity across plans (CORE-29). `ElementId` is
// an index in source order, stable for one parse only (model.rs); after
// a re-parse the caller remaps old ids to new ones through `Element::key`
// and `Model::find` before calling `place`. That remap is not owned by
// any contract yet: docs/deferred.md row "editor/pipeline, 4, remap
// Keep::previous across a re-parse".
// DECISION: in phase 1 an element has at most one box; a `previous`
// entry applies to the first box of its element (`ViewPlan::boxes_of`
// order); phase 3 may add a per-box key to `Keep` (it is
// `#[non_exhaustive]`).
// DECISION: a `moved` centre is taken as given (the editor snaps the
// pointer to the grid, EDIT-2); a moved box may overlap another box:
// CORE-22's no-overlap rule binds the boxes `place` positions, and lint
// reports overlaps of hand-set boxes.
// DECISION: `Keep` uses `BTreeMap`s: one entry per key by construction,
// deterministic iteration (CORE-25), no duplicate-resolution rule.
// DECISION: a stale `moved` entry (BoxId out of range, e.g. an EDIT-1
// reload during a drag shrank the plan) is ignored, as `previous`
// ignores undrawn elements; no panic.
//
// CORE-29: a kept position wins over CORE-22's no-overlap rule. When a
// kept previous centre now overlaps a box whose `at` line changed or a
// box that was dragged, the previous centre is kept and lint reports the
// overlap (L6).
//
// CORE-23's routed clauses (no unroutable line, no label without room,
// crossings within cap) need `route` and `label`, which depend on
// `place`; they live in crates/core/tests/core_23_example.rs, not here.

use std::collections::BTreeMap;

use crate::geom::Point;
use crate::model::ElementId;
use crate::view::{BoxId, ViewPlan};

/// Centres `place` must keep besides the `at` lines of the plan: the
/// previous scene's (CORE-29) and the ones the editor sets by dragging
/// (EDIT-2, EDIT-4, EDIT-20). `Keep::default()` keeps nothing but the
/// `at` lines (CLI, first build of a view). `#[non_exhaustive]` so a
/// later phase can add kinds of kept positions; build it with `Default`
/// or `Keep::from_previous` and insert into the maps. Serves CORE-29,
/// EDIT-4, EDIT-20.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Keep {
    /// Centre of each element drawn in the previous scene of the same
    /// session, in grid units, keyed by the element's id in the model
    /// the plan being placed was built from. Ids from an older parse
    /// must be remapped first (see the module header and the
    /// docs/deferred.md row on re-parse). Applies to a box with no `at`
    /// line and not in `moved`; an element the plan does not draw is
    /// ignored (CORE-29, EDIT-21).
    pub previous: BTreeMap<ElementId, Point>,
    /// Centre of each box the user is dragging or has dragged and not
    /// yet written back, in grid units; wins over its `at` line and
    /// `previous` (EDIT-2, EDIT-4, EDIT-20). Auto-placed boxes keep L6
    /// clearance from these too. A `BoxId` out of range for the plan is
    /// ignored.
    pub moved: BTreeMap<BoxId, Point>,
}

impl Keep {
    /// A `Keep` whose `previous` holds every box of `plan` at
    /// `centres[box]`: what the last `place` returned for it (CORE-29),
    /// with `moved` empty. Keys are `plan`'s own element ids, so it is
    /// valid as is for a plan from the same parse; across a re-parse the
    /// keys need the remap named on `Keep::previous`. The editor builds
    /// it from the scene on screen before a reload or a drag. Serves
    /// CORE-29.
    ///
    /// # Panics
    ///
    /// When `centres` is shorter than `plan.boxes`.
    pub fn from_previous(plan: &ViewPlan, centres: &[Point]) -> Self {
        todo!()
    }
}

/// The centre of every box of `plan`, in grid units, indexed by `BoxId`
/// (`result.len() == plan.boxes.len()`). A box in `keep.moved` gets that
/// centre; else a box with an `at` line keeps it (never moved, CORE-22);
/// else a box in `keep.previous` keeps its previous centre (CORE-29);
/// every other box (`ViewPlan::unplaced`, DSL-8) is auto-placed on the
/// grid so that its rectangle, inflated by 3 units (30px) on every side,
/// overlaps no other box's rectangle inflated by 3 units (CORE-22, no L6
/// on the result); no auto-placed box sits on a row above an auto-placed
/// person; boxes that are not boundary members are outside every
/// boundary rectangle (`ViewPlan::boundary_rect`); auto-placed boxes
/// sharing a row are at least 340px apart centre to centre and rows at
/// least 280px apart; slots are chosen to minimise straight-line
/// crossings between edge ends, then total edge length (CORE-23).
/// Deterministic: same inputs, same centres (CORE-25). When no box needs
/// auto-placing it returns the kept centres without searching (EDIT-25).
/// Out-of-range `keep.moved` entries and undrawn `keep.previous` entries
/// are ignored. Serves CORE-22, CORE-23, CORE-25, CORE-29, EDIT-4,
/// EDIT-20, EDIT-25.
pub fn place(plan: &ViewPlan, keep: &Keep) -> Vec<Point> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // CORE-22, one test per clause.
    #[test]
    fn core_22_places_every_unplaced_box() {
        // One centre per box; every unplaced box gets one on the grid.
        todo!()
    }

    #[test]
    fn core_22_never_moves_placed_box() {
        todo!()
    }

    #[test]
    fn core_22_no_overlap_with_l6_clearance() {
        // Every auto-placed rect inflated by 3 units overlaps no other
        // box (at, previous, moved or auto-placed) inflated by 3 units,
        // including an auto-placed box set next to an `at` box.
        todo!()
    }

    // CORE-23 pure-place clauses, on `tests/data/example.dsl` with every
    // `at` line removed. Routed clauses: tests/core_23_example.rs.
    #[test]
    fn core_23_people_on_top_row() {
        // No auto-placed box sits on a row above an auto-placed person.
        todo!()
    }

    #[test]
    fn core_23_outside_scope_outside_boundary() {
        todo!()
    }

    #[test]
    fn core_23_pitch_at_least_340_by_280() {
        todo!()
    }

    #[test]
    fn core_25_placement_deterministic() {
        // Two runs, same centres (fixed seed).
        todo!()
    }

    // CORE-29 and the drag re-place (EDIT-2, EDIT-4, EDIT-20, EDIT-25).
    #[test]
    fn core_29_previous_scene_boxes_keep_position() {
        // Adding an unplaced box changes no previous centre.
        todo!()
    }

    #[test]
    fn edit_4_dragged_auto_placed_box_keeps_position() {
        // `moved` wins over the previous centre and over the `at` line;
        // every other box keeps its centre.
        todo!()
    }

    #[test]
    fn edit_4_stale_moved_entry_ignored() {
        // A `moved` BoxId out of range for the plan is ignored, no panic.
        todo!()
    }

    #[test]
    fn edit_20_unplaced_box_avoids_moved_boxes() {
        // With several boxes in `moved`, an unplaced box lands with L6
        // clearance from each moved centre, not from its old `at` centre.
        todo!()
    }

    #[test]
    fn edit_25_all_covered_returns_kept_centres() {
        // Every box covered by moved/at/previous: result equals those
        // centres, box by box.
        todo!()
    }

    #[test]
    fn keep_from_previous_round_trips() {
        // `place(plan, &Keep::from_previous(plan, &centres)) == centres`.
        todo!()
    }

    // Review Focus of the place plan.
    #[test]
    fn all_placed_is_noop() {
        // Every box has an `at` line: centres equal the `at` values.
        todo!()
    }

    #[test]
    fn zero_edges_compact_grid() {
        // Boxes laid in a compact grid, not a single row.
        todo!()
    }

    #[test]
    fn single_neighbour_lands_next_to_it() {
        // A box whose only neighbour is placed lands in a slot beside it.
        todo!()
    }
}
