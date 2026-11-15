//! Checks a scene against the model: angles, crossings against the
//! budget, box grid and spacing, C4 level rules, intent labels, coverage
//! across views, and the split hint (CORE-16..21, 26); it also reports
//! what routing, labelling and text fitting could not do: unroutable
//! lines (CORE-13, L1), labels over lines or with no room (CORE-15, L23,
//! L24) and text that does not fit (CORE-30, L25).
//!
//! Depends on: `scene`, `model`, `text` (for `Span`).

// Shape of the API: `lint(&Model, &Scene) -> Report` runs every per-view
// rule of the SRS 4.2 lint table (L1..L21, L23..L26) over one built scene;
// `coverage(&Model, &[&Scene]) -> Vec<Finding>` runs L22, the only rule
// that looks across views, and is reported outside any view (CLI-1 prints
// it after all views). Pipeline calls `lint` per view and `coverage` once
// over every scene it built; the CLI prints `Report::stats` and the
// findings; the editor lists the findings beside the view (EDIT-6) and
// uses `Finding::items` to mark them on the canvas and `Finding::spans`
// to jump to the source.
//
// DECISION: lint reads geometry only through the scene (HLD: lint
// depends on scene and model): `Scene::unroutable` (L1), line points (L2,
// L3), `Scene::crossings` and `Scene::crossing_pairs` (L4, CORE-26; same
// count as the router, so L4 and CORE-11 agree; lint never recounts),
// box rects (L6, L7, L26), `SceneLabel::fit` (L23, L24),
// `TextBlock::fits` (L25). Names, descriptions, technologies, kinds,
// parents and spans come from the model by the ids the scene items carry.
// DECISION: L5 does not read the scene: `view` snaps every centre and
// size to the grid, so `SceneBox::rect` and `Scene::centres` are always on
// it. L5 reads the lines as written, `model.view(&scene.key)` ->
// `ViewSpec::positions` (`At::x`, `At::y`) and `ViewSpec::sizes`
// (`Size::width`, `Size::height`), with `At::span` / `Size::span` as the
// finding's span. A view not in `Model::views` (EDIT-15 default views)
// has no `at`/`size` lines and so no L5.
// DECISION: L12 reads the model, not `Scene::labels`: a line has no label
// when its winning relationship's description is empty or absent. A line
// joining a trunk with the same label (CORE-14) and an unroutable line
// draw no `SceneLabel` and are not L12 for that.
// DECISION: `coverage` takes the scenes, not the view plans: which
// relationships a view draws is `SceneLine::relationships` after lifting
// (CORE-5), and lint does not depend on `view`. Callers pass every
// component view's scene; a container with a component view of
// `Model::views` that has no scene in the slice is skipped (no L22 for
// it), so a partial slice (the editor routing views in the background,
// EDIT-14, CORE-28) gives no false errors.
// DECISION: `lint` expects a scene from `scene::build`. After
// `Scene::move_box` (drag preview) boxes, lines and `crossings` disagree,
// so its report is not meaningful.
// DECISION: no hot struct here (the SRS and HLD name none, a view has
// tens of findings), so no `#[repr]` and no size pin.
// DECISION: the rule is an enum named after what it checks, with its
// table id from `Rule::code` and its level from `Rule::severity`, so the
// level lives in one place (CORE-16: "at the level the table gives").
// DECISION: findings of one report come in rule order (L1 first), then in
// the order of the first item involved (box, then edge order); same
// scene and model give an equal report (CORE-25).
//
// TODO(srs): the SRS fixes the message text of L22 only (CLI-1:
// `<src> -> <dst> is in the model but on no component view of "<scope>"`).
// The text of every other rule, and of the CORE-26 split hint inside the
// L4 message, is open. Stubbed against the prototype's `lint` in `c4.py`
// as the plan asks: one sentence naming the ids (element names, quoted)
// and, for L4, `<c> crossings (budget <b>): split the view (focus, hide);
// boxes involved: "<name>", ...`. `<src>`, `<dst>` and `<scope>` of L22 are
// element names (Structurizr shows names, keys are optional). L22's one
// `<scope>` for a relationship that fails two containers is stubbed as
// in `coverage`. Row in docs/deferred.md.
// TODO(srs): L18 says "as a whole word" without defining a word boundary,
// and the plan's Review Focus says case-insensitive where L18 says
// case-sensitive. Stubbed against L18 (the SRS wins over the plan):
// case-sensitive, a word is a maximal run of ASCII letters and digits, so
// `HTTP/2` and `gRPC-web` match and `HTTPServer` does not. Row in
// docs/deferred.md.

use std::fmt;

use crate::model::{ElementId, Model, RelationshipId};
use crate::scene::{ItemKey, ModelId, Scene};
use crate::text::Span;

/// The level of a finding. Errors fail the run (CLI-2), warnings do not.
/// Ordered `Warning < Error`. Serves CORE-16, CLI-1, CLI-2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// Printed as `warning: <text>` (CLI-1); does not change the exit code.
    Warning,
    /// Printed as `ERROR: <text>` (CLI-1); exit code 1 (CLI-2).
    Error,
}

/// One rule of the SRS 4.2 lint table; `code` gives its table id and
/// `severity` its level. `#[non_exhaustive]`: later phases add rules.
/// Serves CORE-16..21, CORE-26, CORE-13, CORE-15, CORE-30.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Rule {
    /// L1, error: a line could not be routed (CORE-13).
    Unroutable,
    /// L2, error: a segment is not at 0°, 45° or 90° (CORE-6).
    SegmentAngle,
    /// L3, error: a segment passes through the inside of a box other
    /// than its own two ends (CORE-7).
    ThroughBox,
    /// L4, error: crossings above `maxCrossings`; carries the CORE-26
    /// split hint (CORE-17, CORE-26).
    Crossings,
    /// L5, error: a box centre, half-width or half-height is not a
    /// multiple of 10 (CORE-18), read from the `at` and `size` lines as
    /// written, not from the snapped scene.
    OffGrid,
    /// L6, error: two boxes, each grown by 30px on every side, overlap
    /// (CORE-18).
    TooClose,
    /// L7, warning: more than 3 boxes, and distinct centre rows plus
    /// distinct centre columns exceed boxes + 1.
    Alignment,
    /// L8, error: the view has no title.
    NoTitle,
    /// L9, error: a box has an empty name or an empty description
    /// (CORE-20).
    NameOrDescription,
    /// L10, error: a container or component has no technology (CORE-20).
    NoTechnology,
    /// L11, warning: a box description is over 110 characters (CORE-20),
    /// counted as `char`s.
    // DECISION: chars, not bytes; the SRS does not say, and a byte count
    // would flag short non-ASCII descriptions.
    LongDescription,
    /// L12, error: a line has no label (CORE-20): the winning
    /// relationship's description is empty or absent in the model. Not
    /// the absence of a `SceneLabel`: a line joining a trunk with the same
    /// label (CORE-14) and an unroutable line draw none.
    NoLabel,
    /// L13, error: a label is only a stop word (`uses`, `use`, `calls`,
    /// `sends`, `reads`, `writes`) (CORE-20).
    VagueLabel,
    /// L14, warning: both A -> B and B -> A are drawn, once per pair.
    TwoWay,
    /// L15, error: a C1 line shows a technology (CORE-19).
    C1Technology,
    /// L16, error: a C1 box is not a person or a software system
    /// (CORE-19).
    C1BoxKind,
    /// L17, warning: a C1 view has more than 8 boxes (CORE-19).
    C1TooManyBoxes,
    /// L18, error: a C1 box name or line label holds a protocol word
    /// (CORE-19).
    C1Protocol,
    /// L19, error: a C2 view shows a component (CORE-19).
    C2Component,
    /// L20, warning: a C2 line has a container at one end, no person at
    /// either end, and no technology (CORE-19).
    C2NoTechnology,
    /// L21, error: a C3 view shows no component whose parent is the
    /// scope (CORE-19).
    C3NoScopeComponent,
    /// L22, error: a relationship of a component is on none of its
    /// container's component views; outside any view (CORE-21).
    Coverage,
    /// L23, warning: a label covers another line (CORE-15).
    LabelOverLine,
    /// L24, warning: a label has no room (CORE-15).
    LabelNoRoom,
    /// L25, warning: a text does not fit its box or label rectangle
    /// (CORE-30).
    TextFit,
    /// L26, error: a box that is not a child of the scope overlaps the
    /// boundary rectangle.
    BoundaryIntrusion,
}

impl Rule {
    /// The table id, `"L1"` to `"L26"`. Serves CORE-16.
    #[must_use]
    pub const fn code(self) -> &'static str {
        todo!()
    }

    /// The level the lint table gives this rule. Serves CORE-16.
    #[must_use]
    pub const fn severity(self) -> Severity {
        todo!()
    }
}

/// The CORE-26 suggestion carried by an L4 finding: split the view with
/// `focus` or `hide`, naming what takes part in the crossings. Serves
/// CORE-26.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SplitHint {
    /// The elements of the boxes at either end of a crossing line, each
    /// once, in box order.
    pub boxes: Vec<ElementId>,
    /// The winning relationships of the crossing lines, each once, in
    /// edge order.
    pub relationships: Vec<RelationshipId>,
}

/// One broken rule. Serves CORE-16 ("with the ids involved, at the level
/// the table gives"), CORE-26, EDIT-6.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Finding {
    /// The rule broken; its level is `rule.severity()`.
    pub rule: Rule,
    /// The view's effective key (`Scene::key`); `None` for a finding
    /// outside any view (L22).
    pub view: Option<String>,
    /// The model ids involved, in the order the message names them.
    pub subjects: Vec<ModelId>,
    /// The scene items to mark on the canvas (EDIT-6), view-local to the
    /// scene linted. Empty for L22 and for the view-wide rules L7, L8
    /// and L17; L4 marks its crossing lines.
    pub items: Vec<ItemKey>,
    /// Source spans the editor can point at, most specific first: the
    /// `at`/`size` line for L5 (`At::span`, `Size::span`), the element or relationship
    /// declaration, the view declaration for view-wide rules. Empty for a
    /// view not written in the file (EDIT-15 default views).
    pub spans: Vec<Span>,
    /// The text the CLI prints after `warning: ` or `ERROR: ` (CLI-1).
    pub message: String,
    /// The CORE-26 split hint, on an L4 finding only.
    pub hint: Option<SplitHint>,
}

impl Finding {
    /// `rule.severity()`. Serves CORE-16, CLI-2.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        todo!()
    }
}

impl fmt::Display for Finding {
    /// Writes `message` only; the CLI adds the `warning: ` or `ERROR: `
    /// prefix (CLI-1). Serves CLI-1, EDIT-6.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

/// The counts the CLI prints per view (CLI-1): `<n> boxes, <m> lines,
/// <c> crossings (budget <b>)` and `alignment: <r> rows, <k> columns for
/// <n> boxes`. Serves CLI-1, CORE-17, L7.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Stats {
    /// Boxes drawn.
    pub boxes: u32,
    /// Lines drawn, unroutable ones included.
    pub lines: u32,
    /// `Scene::crossings`.
    pub crossings: u32,
    /// The view's `maxCrossings` (`Scene::options`).
    pub budget: u32,
    /// Distinct box centre rows (L7).
    pub rows: u32,
    /// Distinct box centre columns (L7).
    pub columns: u32,
}

/// The lint result of one view: its counts and its findings, errors and
/// warnings mixed in rule order. Serves CORE-16, CLI-1, CLI-2, EDIT-6.
///
/// Built only by `lint` (`#[non_exhaustive]`, as `Finding` and `Stats`):
/// tests outside core get a report through the pipeline.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Report {
    /// The view's effective key (`Scene::key`).
    pub view: String,
    /// The counts for the CLI header lines.
    pub stats: Stats,
    /// Every broken per-view rule, in rule order then item order.
    pub findings: Vec<Finding>,
}

impl Report {
    /// True when any finding is an error (CLI-2 exit code 1). Serves
    /// CORE-16, CLI-2.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        todo!()
    }

    /// The warnings, in report order (CLI-1 prints them first). Serves
    /// CLI-1.
    // DECISION: a `Vec`, not `impl Iterator`: a `todo!()` stub cannot
    // return an opaque iterator, and `Model::children` makes the same
    // choice; a view has tens of findings.
    #[must_use]
    pub fn warnings(&self) -> Vec<&Finding> {
        todo!()
    }

    /// The errors, in report order (CLI-1 prints them after the
    /// warnings). Serves CLI-1, CLI-2.
    #[must_use]
    pub fn errors(&self) -> Vec<&Finding> {
        todo!()
    }
}

/// Lints one built view: every rule of the lint table except L22, over
/// what `scene` draws (after lifting) and the model behind its ids.
/// Geometry rules (L2, L3) skip lines with `routed: false`, which are
/// L1 only. L4 and `Stats::crossings` use `Scene::crossings` and
/// `Scene::crossing_pairs` as given and never recount; L6, L7 and L26 use
/// the box rects. L5 reads the view's `at` and `size` lines as written
/// (`model.view(&scene.key)` -> `ViewSpec::positions` / `sizes`, spans
/// `At::span` / `Size::span`), since the scene is snapped to the grid; a
/// view not in `Model::views` (EDIT-15 default views) has no L5. L12 reads
/// the winning relationship's description in the model, not
/// `Scene::labels`. An L4 finding carries the split hint (CORE-26).
/// Deterministic (CORE-25). Serves CORE-16..20, CORE-26, CORE-13,
/// CORE-15, CORE-30.
///
/// Expects a scene from `scene::build`, not one changed by
/// `Scene::move_box` (a drag preview), whose boxes, lines and crossings
/// disagree.
///
/// # Panics
///
/// When `scene` was not built from `model` (an element or relationship
/// id it carries is not in `model`).
#[must_use]
pub fn lint(model: &Model, scene: &Scene) -> Report {
    todo!()
}

/// L22 (CORE-21): for each container with at least one component view
/// in `model.views`, every relationship with one of its components at
/// either end must be drawn (`SceneLine::relationships`) on at least one
/// of that container's component views among `scenes`; each one that is
/// not gives one error, once per relationship, with `view: None`. Scenes
/// of other kinds, or of other scopes, are ignored for that container.
/// Ordered by relationship id. Serves CORE-21, CLI-1.
///
/// Callers pass the scene of every component view in `model.views`. A
/// container with a component view that has no scene in `scenes` is
/// skipped, so a partial slice gives no false L22 (it gives no L22 for
/// that container at all).
///
/// The `"<scope>"` of the CLI-1 message is one container: the source's
/// container (when the source is a component of a checked container) if
/// the relationship fails there, else the destination's.
/// A relationship that fails both containers is still one finding, named
/// with the source's container.
// TODO(srs): CORE-21 / CLI-1 do not say which container is `<scope>` when
// a relationship joins components of two checked containers and fails
// one or both. Stubbed as above; row 43 in docs/deferred.md.
///
/// # Panics
///
/// When a scene was not built from `model`.
#[must_use]
pub fn coverage(model: &Model, scenes: &[&Scene]) -> Vec<Finding> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test per requirement.
    #[test]
    fn core_16_errors_and_warnings_split_like_prototype() {
        // Every rule's level is the lint table's; `has_errors` is true
        // only with an error; warnings alone keep it false.
        todo!()
    }

    #[test]
    fn core_16_rule_codes_l1_to_l26() {
        todo!()
    }

    #[test]
    fn core_16_findings_name_ids_items_and_spans() {
        // Each finding names its model ids; per-view findings carry the
        // view key and canvas items; spans point at the declarations.
        todo!()
    }

    #[test]
    fn core_16_deterministic_order() {
        // Same model and scene give an equal report, rule order first.
        todo!()
    }

    #[test]
    fn core_13_unroutable_line_l1_only() {
        // An unroutable line is L1 and never L2 or L3.
        todo!()
    }

    #[test]
    fn core_16_off_angle_segment_l2() {
        todo!()
    }

    #[test]
    fn core_16_segment_through_box_l3() {
        todo!()
    }

    #[test]
    fn core_16_alignment_warning_l7() {
        todo!()
    }

    #[test]
    fn core_16_missing_title_l8() {
        todo!()
    }

    #[test]
    fn core_16_two_way_lines_once_per_pair_l14() {
        todo!()
    }

    #[test]
    fn core_15_label_over_line_and_no_room_l23_l24() {
        // Both name the relationship (CORE-15).
        todo!()
    }

    #[test]
    fn core_30_text_does_not_fit_l25() {
        todo!()
    }

    #[test]
    fn core_16_box_intrudes_boundary_l26() {
        todo!()
    }

    #[test]
    fn core_16_stats_rows_columns_budget() {
        // `Stats` counts as CLI-1 prints them.
        todo!()
    }

    #[test]
    fn core_17_crossings_over_budget_error() {
        // `crossings > max_crossings` is L4; equal to the budget is not.
        todo!()
    }

    #[test]
    fn core_17_crossings_taken_from_scene() {
        // L4 and `Stats::crossings` use `Scene::crossings` as given and
        // never recount (counting is geom's and route's, tested there). A
        // scene fixture with a crossing at a bend (counted once) and a
        // joined line ending on its trunk (no crossing) gives L4 and
        // `Stats::crossings` equal to `Scene::crossings`.
        todo!()
    }

    #[test]
    fn core_18_off_grid_and_too_close_error() {
        // L5 for an `at` centre or `size` half-size off the 10px grid as
        // written (the scene is snapped, so L5 reads `ViewSpec`), span
        // `At::span` / `Size::span`; no L5 for a view not in
        // `Model::views`. L6 for edges 50px apart, none at 60px.
        todo!()
    }

    #[test]
    fn core_19_c1_rejects_containers_and_protocols() {
        // L15, L16, L17, L18.
        todo!()
    }

    #[test]
    fn core_19_protocol_word_whole_word_case_sensitive() {
        // TODO(srs) stub: `HTTPS` and `HTTP/2` match; `https`,
        // `HTTPServer` and "chattiness" do not.
        todo!()
    }

    #[test]
    fn core_19_c2_rejects_components() {
        // L19, and L20 as a warning.
        todo!()
    }

    #[test]
    fn core_19_c3_needs_scope_component() {
        // L21.
        todo!()
    }

    #[test]
    fn core_20_name_description_technology_intent() {
        // L9, L10, L11 (over 110 characters, counted as chars), L12, L13.
        // L12 comes from the model: a joined line with the same label as
        // its trunk (CORE-14) and an unroutable line have no `SceneLabel`
        // and are not L12; a relationship with an empty description is.
        todo!()
    }

    #[test]
    fn core_20_stop_word_label_only() {
        // Review Focus: "uses" alone is L13; " Uses " trimmed and
        // lowercased too; "uses the API to fetch" is not.
        todo!()
    }

    #[test]
    fn core_21_relationship_on_no_view_error() {
        // L22 once per relationship, `view: None`, message as CLI-1.
        todo!()
    }

    #[test]
    fn core_21_coverage_same_scope_only() {
        // Review Focus: a relationship drawn only on another container's
        // component view still fails; a container with no component view
        // is not checked; a lifted line covers the relationships it
        // stands for. A relationship between components of two checked
        // containers that neither covers is one L22, `<scope>` the
        // source's container; covered by the source's only, `<scope>` the
        // destination's (TODO(srs) stub). A container with a component
        // view missing from `scenes` is skipped.
        todo!()
    }

    #[test]
    fn core_26_split_hint_names_boxes() {
        // The L4 finding carries the boxes and relationships of the
        // crossing lines (`Scene::crossing_pairs`) and names them.
        todo!()
    }
}
