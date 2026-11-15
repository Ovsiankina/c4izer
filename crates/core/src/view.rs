//! Resolves one view spec into a view plan: the boxes (id, size,
//! position if any) and edges left after include, exclude, lifting,
//! de-duplication, focus and hide (CORE-5).
//!
//! Depends on: `model`, `geom`.

// Shape of the API: `plan` turns one `ViewSpec` of a `Model` into a
// `ViewPlan`, plain data with pub fields that `place`, `route`, `label`
// and `scene` read without going back to the model (HLD §4: `route`,
// `label` and `scene` do not depend on `model`'s functions). So the plan
// carries everything they draw or test: per box its element, kind,
// resolved style (shape for CORE-9 ports), texts, size and position; per
// edge its ends, winning relationship, texts; the title, subtitle,
// options and the scope boundary. Routing one view needs only its plan
// (CORE-28).
//
// Boxes and edges are addressed by per-plan indices (`BoxId`,
// `EdgeId`) into `ViewPlan::boxes` / `ViewPlan::edges`. They are not the
// stable view-local key of SRS 4.2 (that key is what `at` and `size`
// lines address); identity across plans (CORE-29 previous scene,
// EDIT-25) is `PlanBox::element`. In phase 1 a box maps to exactly one
// element; phase 3 (deployment, dynamic views) draws one element or
// relationship several times, which the index keys already allow.
//
// Positions: the plan is immutable. `PlanBox::centre` holds the `at`
// value only; `place` returns every centre as a `Vec<Point>` indexed by
// `BoxId`, and `route`, `label` and `scene` take `(&ViewPlan, &[Point])`.
// A drag (EDIT-2, EDIT-25) changes the centres, never the plan.
//
// Units: every coordinate and size in the plan is in grid units (1 unit =
// 10px, `geom`), as `place` and `route` work in grid units. The `at` and
// `size` lines are px (`model::At`, `model::Size`); lint checks the px
// values as written (CORE-18, L5) from the model, not from the plan.
//
// DECISION: an off-grid `at` centre is snapped to the nearest grid point
// (`geom::snap_px_to_grid`) and a `size` that is not a multiple of 20px
// gives half-sizes rounded up to whole grid units, so every planned box
// is on the grid; lint still reports the line as written (L5).
// DECISION: boxes are in `Model::included` order (sorted by element id);
// edges are sorted by their winning relationship's id (source order), so
// the plan is deterministic (CORE-25).
// DECISION: the CORE-2 default size depends on the element kind only, as
// CORE-2 states; a `shape Person` container still gets 240x140.
// DECISION: `technology off` (CORE-4) is not applied to the plan data:
// edges keep the technology as written, and `label` and `scene` both
// read it through `PlanEdge::shown_technology`, so they cannot disagree.
// DECISION: CORE-5 "drawn once" applies to lifted lines only. Each
// relationship declared between two drawn elements is its own edge (two
// direct `a -> b` are two lines, as in Structurizr and as `model`'s
// `find_relationship` ordinal treats them). Lifted relationships on an
// ordered box pair merge into the first declared direct edge on that
// pair when there is one, else into one edge whose winner is the first
// declared of them.
// DECISION: `hide <a> -> <b>` removes every drawn edge whose source box
// is `a` and destination box is `b` (one direction only), after lifting.
// DECISION: two relationships in opposite directions between the same
// two boxes are two edges (CORE-5 de-duplicates per ordered pair).
// DECISION: a phase-1 boundary exists only on container and component
// views, around the scope's children that are drawn; a context view's
// scope is a box, not a boundary (CORE-1).
// DECISION: a boundary rectangle is its members' bounds inflated by 4
// grid units (40px, CORE-1), computed by `ViewPlan::boundary_rect` so
// `place` (CORE-23), `label` (CORE-14), `scene` and `lint` (L26) agree.
//
// DSL-7: a `focus` or `hide` id with no box of its own in the view (not
// included, or included only through a drawn ancestor) is lifted to its
// nearest drawn ancestor; when it has none it is ignored.

use crate::geom::{Point, Rect};
use crate::model::{
    ElementId, ElementKind, Model, RelationshipId, ResolvedStyle, ViewKind, ViewOptions,
    ViewScope, ViewSpec,
};

/// Index of a box in `ViewPlan::boxes`, valid for one plan only. It is
/// not the stable view-local key of SRS 4.2 (what `at` and `size` lines
/// address); identity across plans is `PlanBox::element`. `place`,
/// `route`, `label` and `scene` key their per-box data by it. Serves
/// CORE-5, CORE-1.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BoxId(pub u32);

/// Index of an edge in `ViewPlan::edges`, valid for one plan only;
/// identity across plans is `PlanEdge::winner`. `route` reports an
/// unroutable line by it (CORE-13) and `label` reports a label by it
/// (CORE-15). Serves CORE-5.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeId(pub u32);

/// One box to draw: an element the view includes after `include` and
/// `exclude` (DSL-6), with its size (`size` line or CORE-2 default) and
/// its `at` position (DSL-7) if written (else unplaced, DSL-8). Carries
/// the texts, tags and style `scene` draws (CORE-1 key, CORE-2) and the
/// shape `route` needs for ports (CORE-9). Serves CORE-5, CORE-2, DSL-6,
/// DSL-7, DSL-8.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanBox {
    /// Its own index in `ViewPlan::boxes`.
    pub id: BoxId,
    /// The model element it draws: its identity across plans (CORE-29,
    /// EDIT-25), EDIT hit-testing, CORE-1 tagging.
    pub element: ElementId,
    /// Person, software system, container or component (CORE-2 type
    /// line, CORE-19 level checks).
    pub kind: ElementKind,
    /// True when the element is tagged `External` (CORE-2: type
    /// `External Software System`).
    pub external: bool,
    /// The element's effective tags in model order (`Element`, the kind
    /// tag, then declared tags): the CORE-1 key groups colours by them.
    pub tags: Vec<String>,
    /// The element's resolved style: fill, text colour, shape (DSL-15,
    /// CORE-2, CORE-3 line colour, CORE-9 ports).
    pub style: ResolvedStyle,
    /// Element name, unquoted.
    pub name: String,
    /// Element description, unquoted, if written.
    pub description: Option<String>,
    /// Element technology, unquoted, if written.
    pub technology: Option<String>,
    /// Box centre in grid units from its `at` line, `None` when the view
    /// has none (DSL-8). Never filled in later: `place` returns every
    /// centre as a `Vec<Point>` indexed by `BoxId` (CORE-22).
    pub centre: Option<Point>,
    /// Half the width in grid units, from the `size` line or the CORE-2
    /// default.
    pub half_width: i32,
    /// Half the height in grid units, from the `size` line or the CORE-2
    /// default.
    pub half_height: i32,
}

impl PlanBox {
    /// The box rectangle in grid units at its `at` centre, `None` when
    /// it has no `at` line. Serves DSL-7, CORE-18.
    pub const fn rect(&self) -> Option<Rect> {
        todo!()
    }

    /// The box rectangle in grid units at `centre` (from `place`). Serves
    /// CORE-7, CORE-18, CORE-22.
    pub const fn rect_at(&self, centre: Point) -> Rect {
        todo!()
    }
}

/// One line to draw: one relationship declared between two drawn
/// elements, or one or more lifted relationships on the same ordered
/// pair of boxes, drawn once (CORE-5). Lifted relationships merge into
/// the first declared direct edge on their pair when there is one. Texts
/// are the winner's: the direct relationship, otherwise the first
/// declared lifted one. Serves CORE-5, CORE-14.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanEdge {
    /// Its own index in `ViewPlan::edges`.
    pub id: EdgeId,
    /// The box the line starts at.
    pub source: BoxId,
    /// The box the line points to (CORE-3: the line takes its colour).
    pub destination: BoxId,
    /// Every relationship this line stands for, winner first, then the
    /// merged lifted ones in source order; never empty (CORE-21
    /// coverage, L22).
    pub relationships: Vec<RelationshipId>,
    /// The winner's description, unquoted, if written (CORE-14 label).
    pub description: Option<String>,
    /// The winner's technology, unquoted, as written; read it through
    /// `shown_technology` (CORE-4).
    pub technology: Option<String>,
}

impl PlanEdge {
    /// The winning relationship, `relationships[0]`: the line's model id
    /// and its identity across plans (CORE-1 tagging, CORE-15 and L1
    /// report names). Serves CORE-5.
    pub fn winner(&self) -> RelationshipId {
        todo!()
    }

    /// The technology to draw and size the label with: `technology`,
    /// or `None` under `technology off`. `label` and `scene` both call
    /// it. Serves CORE-4, CORE-14.
    pub fn shown_technology(&self, options: &ViewOptions) -> Option<&str> {
        todo!()
    }
}

/// What a boundary stands for. `#[non_exhaustive]` so phase 2 adds group
/// boundaries (P2-4), which have no element, without a breaking change.
/// Serves CORE-1.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoundaryOf {
    /// An element's drawn children (the view's scope in phase 1).
    Element(ElementId),
}

/// A dashed boundary around drawn boxes (CORE-1). Its rectangle is the
/// members' bounds plus 40px (`ViewPlan::boundary_rect`); the name tag
/// sits at its bottom left. Phase 1 has at most one, around the scope;
/// phase 2 adds nested boundaries (groups, enclosing systems). Serves
/// CORE-1.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Boundary {
    /// What the boundary stands for.
    pub of: BoundaryOf,
    /// Its name, unquoted, drawn as the boundary's name tag.
    pub name: String,
    /// The boxes inside, in `ViewPlan::boxes` order; never empty (a
    /// scope with no drawn child has no boundary).
    pub members: Vec<BoxId>,
}

/// Everything one view draws, before placement and routing: boxes,
/// edges, boundaries, title. Immutable once built: `place` returns the
/// centres, `route` routes the edges, `label` places their labels and
/// `scene` draws it all, each from `(&ViewPlan, &[Point])`. Serves
/// CORE-5, CORE-1, DSL-6, DSL-7, DSL-8.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewPlan {
    /// The view's effective key (names the SVG file, CLI-1).
    pub key: String,
    /// System context, container or component.
    pub kind: ViewKind,
    /// The element the view is about.
    pub scope: ViewScope,
    /// The drawn title: the `title` line, else the description; `None`
    /// is lint L8 (DSL-6, CORE-1).
    pub title: Option<String>,
    /// The `subtitle` line, if written (CORE-1).
    pub subtitle: Option<String>,
    /// The view options with defaults filled in (CORE-4, CORE-8,
    /// CORE-14, CORE-17).
    pub options: ViewOptions,
    /// Every box, `boxes[i].id == BoxId(i)`.
    pub boxes: Vec<PlanBox>,
    /// Every line after lifting, de-duplication, `focus` and `hide`,
    /// `edges[i].id == EdgeId(i)`.
    pub edges: Vec<PlanEdge>,
    /// The dashed boundaries; phase 1 holds at most one (CORE-1).
    pub boundaries: Vec<Boundary>,
}

impl ViewPlan {
    /// Every box drawing `element`, in `boxes` order; empty when the
    /// view does not draw it. Phase 1 yields at most one; phase 3
    /// (P3-2, P3-3) may yield several. Serves CORE-5, EDIT hit-testing.
    pub fn boxes_of(&self, element: ElementId) -> Vec<BoxId> {
        todo!()
    }

    /// The boxes with no `at` line, in `boxes` order: the ones `place`
    /// positions (DSL-8, CORE-22). Same set as `Model::unplaced`; it is
    /// here because `place` must not call `model`'s functions (HLD §4).
    pub fn unplaced(&self) -> Vec<BoxId> {
        todo!()
    }

    /// The rectangle of `boundaries[index]` in grid units with every box
    /// at `centres[box]`: its members' bounds inflated by 4 grid units
    /// (40px). `None` when `index` is out of range. `place` (CORE-23),
    /// `label` (CORE-14 name tag), `scene` (CORE-1) and `lint` (L26) all
    /// use it. Serves CORE-1.
    ///
    /// # Panics
    ///
    /// When `centres` is shorter than `boxes`.
    pub fn boundary_rect(&self, index: usize, centres: &[Point]) -> Option<Rect> {
        todo!()
    }
}

/// Resolves `view` of `model` into its plan: boxes from
/// `Model::included` with their `size` lines (else the CORE-2 default by
/// kind) and `at` lines; every relationship lifted to the nearest drawn
/// ancestors of its ends, dropped when both ends lift to the same box or
/// either has no drawn ancestor; each direct relationship its own edge,
/// lifted ones merged per ordered box pair (CORE-5); then only lines
/// touching a `focus` box are kept, and `hide` lines removed; a `focus`
/// or `hide` id with no box of its own is lifted to its nearest drawn
/// ancestor, and ignored when it has none (DSL-7). Serves
/// CORE-5, CORE-1, CORE-2, DSL-6, DSL-7, DSL-8.
///
/// # Panics
///
/// When `view` does not belong to `model`: its element ids index out of
/// range.
pub fn plan(model: &Model, view: &ViewSpec) -> ViewPlan {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Size pins: per-plan keys index per-box and per-edge arrays in
    // place, route, label and scene.
    #[test]
    fn box_id_size_is_pinned() {
        assert_eq!(std::mem::size_of::<BoxId>(), 4);
    }

    #[test]
    fn edge_id_size_is_pinned() {
        assert_eq!(std::mem::size_of::<EdgeId>(), 4);
    }

    // CORE-5, one test per clause.
    #[test]
    fn core_5_lifted_to_nearest_drawn_ancestor() {
        todo!()
    }

    #[test]
    fn core_5_declared_between_drawn_wins_over_lifted() {
        // The lifted relationship merges into the direct edge.
        todo!()
    }

    #[test]
    fn core_5_first_declared_wins_among_lifted() {
        // Review Focus: its label is the first by source order.
        todo!()
    }

    #[test]
    fn core_5_duplicate_lifted_lines_drawn_once() {
        todo!()
    }

    #[test]
    fn core_5_two_direct_relationships_same_pair() {
        // Two direct `a -> b` are two edges; not merged.
        todo!()
    }

    #[test]
    fn core_5_internal_lift_dropped() {
        // Review Focus: ends lifting to the same box, no self-loop.
        todo!()
    }

    #[test]
    fn core_5_no_drawn_ancestor_dropped() {
        // E.g. a relationship to the scope system on a container view.
        todo!()
    }

    #[test]
    fn core_5_element_and_ancestor_both_drawn() {
        todo!()
    }

    // view-01: include and exclude per view kind (DSL-6). The box set
    // comes from `Model::included`; view-01 implements nothing in
    // view.rs for these, they check that `plan` draws exactly that set.
    #[test]
    fn include_star_context_view() {
        todo!()
    }

    #[test]
    fn include_star_container_view() {
        todo!()
    }

    #[test]
    fn include_star_component_view() {
        // Review Focus: external containers and people linked to the
        // scope's components come in.
        todo!()
    }

    #[test]
    fn exclude_removes_box_and_edges() {
        todo!()
    }

    #[test]
    fn exclude_of_not_included_is_no_error() {
        // Review Focus.
        todo!()
    }

    // view-03: focus, hide, sizes, positions (DSL-6, DSL-7, DSL-8).
    #[test]
    fn focus_keeps_only_touching_lines() {
        todo!()
    }

    #[test]
    fn hide_drops_pairs() {
        todo!()
    }

    #[test]
    fn dsl_7_focus_hide_lift_to_ancestor() {
        // A `focus` or `hide` id with no box of its own acts on its
        // nearest drawn ancestor; with no drawn ancestor it is ignored.
        todo!()
    }

    #[test]
    fn core_2_default_sizes_by_kind() {
        todo!()
    }

    #[test]
    fn dsl_8_unplaced_boxes_marked() {
        todo!()
    }

    #[test]
    fn dsl_7_at_and_size_lines_in_grid_units() {
        todo!()
    }

    // Plan contents the next stages read.
    #[test]
    fn plan_box_carries_kind_external_style_texts() {
        // CORE-2 data, plus effective tags for the CORE-1 key.
        todo!()
    }

    #[test]
    fn boxes_of_and_rect() {
        // `boxes_of`, `rect` (at line) and `rect_at` (placed centre).
        todo!()
    }

    #[test]
    fn edge_winner_and_shown_technology() {
        // CORE-4: `technology off` hides it, the plan keeps it.
        todo!()
    }

    #[test]
    fn opposite_directions_are_two_edges() {
        todo!()
    }

    #[test]
    fn core_1_boundary_holds_drawn_scope_children() {
        // Container and component views only; none on a context view or
        // when no child is drawn.
        todo!()
    }

    #[test]
    fn core_1_boundary_rect_inflates_member_bounds() {
        todo!()
    }

    #[test]
    fn dsl_6_title_line_else_description() {
        // Also subtitle and options with defaults.
        todo!()
    }

    #[test]
    fn core_25_plan_is_deterministic() {
        // Boxes by element id, edges by winning relationship id.
        todo!()
    }
}
