//! Entry points: parse a file; build one view's scene and lint report
//! without routing the other views; build every view (CORE-24, 27, 28).
//!
//! Depends on: `model`, `view`, `place`, `route`, `label`, `scene`, `lint`.

// Shape of the API: plain functions over plain data, no state, no
// threads (the editor's worker owns the threading and the generation
// numbers, HLD §6).
//
//   model::parse(text)                  -> Parsed | ModelError
//                                          (not wrapped here)
//   build_view(model, key, keep)        -> ViewBuild | UnknownView
//   build_spec(model, spec, keep)       -> ViewBuild          (any spec,
//                                          EDIT-15 default views too)
//   build_all(model)                    -> AllViews           (CLI)
//   rebuild_after_move(model, prev, moved, config) -> ViewBuild
//                                          (drag preview, EDIT-2/25)
//   keep_across_reparse(old, plan, centres, new) -> Keep
//                                          (CORE-29 after a reload)
//   default_view(model, scope, kind)    -> Option<ViewSpec>   (EDIT-15)
//
// One view is `view::plan` -> `place::place` -> `route::route` ->
// `label::place_labels` -> `scene::build` -> `lint::lint`, from that
// view's spec alone: no other view is planned or routed (CORE-28).
// `ViewBuild` keeps every stage's output, because the editor's drag
// preview needs the plan and the routes (`route::reroute` takes both)
// and the CLI needs the scene and the report.
//
// DECISION: no `pipeline::parse`: callers (CLI, editor, speed tests) use
// `model::parse` directly. A wrapper with the same signature would be a
// pub item no requirement asks for; `model::ModelError` is the only
// failure of the text stage, its `Display` already follows SRS 5.1, and
// its `Result` means no partial model reaches the caller (plan Review
// Focus, test `parse_error_leaks_no_model`).
// DECISION: an unknown view key is its own error type (`UnknownView`),
// naming the key and listing the valid keys in declaration order (plan
// Review Focus); `build_spec` and `build_all` cannot fail.
// DECISION: a file with no views gives an empty `AllViews` (the CLI says
// "no views", plan Review Focus), not an error.
// DECISION: full builds (`build_view`, `build_spec`, `build_all`) always
// route with `RouteConfig::DEFAULT`, so the editor after release and the
// CLI give the same scene (EDIT-25); only the drag preview takes a
// config, and `PREVIEW_ROUTE` is the cheaper one the editor passes.
// DECISION: `build_view` and `build_spec` take a `place::Keep` (closes
// the docs/deferred.md row asking for it): the CLI passes
// `Keep::default()`, the editor the previous scene's centres (CORE-29)
// plus dragged boxes (EDIT-4, EDIT-20).
// DECISION: the remap of `Keep::previous` across a re-parse
// (`keep_across_reparse`) lives here, keyed by `Element::key` through
// `Model::find` (closes the docs/deferred.md "editor/pipeline" row).
// DECISION: EDIT-15 default views are not in `Model::views`, and the
// editor cannot build a `ViewSpec` (`#[non_exhaustive]`, other crate),
// so `default_view` builds one here: key `<scope id>-<kind>` as `edit`
// writes it (`ViewKind::keyword`), `include *`, no `at`/`size` lines.
// DECISION: speed tests are named `perf_<mod>_<n>_<slug>` and `#[ignore]`d
// (SRS 5.2), so the release perf oracle's `--ignored perf_` filter
// selects them while the name still carries the requirement id. A speed
// test with no budget yet (CORE-31) has no `perf_` prefix and is
// `#[ignore = "deferred"]`, so the oracle does not run it until the
// budget exists (docs/deferred.md row).
//
// TODO(srs): EDIT-25 ("after release ... equals the scene the core builds
// from the saved file alone") and CORE-29 (a box drawn in the previous
// scene keeps its position) disagree for auto-placed boxes: from the
// file alone they are placed afresh, with the session's `Keep` they stay
// where they were. Stubbed against CORE-29 (Structurizr never moves a box
// the user has seen): the release build equals `build_view` of the saved
// file with the same `Keep::previous` and an empty `Keep::moved`; it
// equals the build from the file alone whenever every box has an `at`
// line. Row in docs/deferred.md.

use std::collections::BTreeMap;
use std::fmt;

use crate::geom::Point;
use crate::label::Labels;
use crate::lint::{Finding, Report};
use crate::model::{ElementId, Model, ViewKind, ViewSpec};
use crate::place::Keep;
use crate::route::{RouteConfig, Routes};
use crate::scene::Scene;
use crate::view::{BoxId, ViewPlan};

/// Router limits for the editor's drag preview: fewer edge orders and a
/// lower rip-up cap than `RouteConfig::DEFAULT`, so `rebuild_after_move`
/// keeps up with drag events (EDIT-2, EDIT-25, CORE-24). Only "no more
/// orders and no higher cap than `DEFAULT`" is the contract; the values
/// may be tuned at step 5 without a contract change. Full builds never
/// use it (EDIT-25 equality after release).
// DECISION: 2 orders (the two Manhattan sorts) and 8 rounds: a quarter
// of the full run; tuned against CORE-24 at step 5.
pub const PREVIEW_ROUTE: RouteConfig = RouteConfig {
    orders: 2,
    max_iterations: 8,
};

/// Everything one view's build produced, stage by stage. The scene is
/// what the canvas and the exporters draw (CORE-1, EDIT-12); the plan
/// and routes are what `rebuild_after_move` re-routes from (EDIT-25);
/// the report is the view's lint findings (CORE-16, CLI-1, EDIT-6).
/// `#[non_exhaustive]` so later stages can add outputs; built only by
/// this module. Serves CORE-28, CORE-24, CORE-27, EDIT-25.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewBuild {
    /// The view plan (`view::plan`): boxes, edges, boundaries, title.
    pub plan: ViewPlan,
    /// The routed lines (`route::route`, or `route::reroute` for a drag
    /// preview); `routes.centres` are the box centres of this build.
    pub routes: Routes,
    /// The placed labels (`label::place_labels`).
    pub labels: Labels,
    /// The scene (`scene::build`); `scene.key == plan.key`.
    pub scene: Scene,
    /// The lint report of `scene` against the model (`lint::lint`); no
    /// L22, which looks across views (`AllViews::coverage`).
    pub report: Report,
}

/// Every view of a file, built: the CLI's input (CLI-1, CLI-2, CLI-3).
/// `#[non_exhaustive]`; built only by `build_all`. Serves CORE-11,
/// CORE-21, CLI-1, CLI-2, CLI-5.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AllViews {
    /// One build per view of `Model::views`, in declaration order; empty
    /// when the file has no views (the CLI reports "no views").
    pub views: Vec<ViewBuild>,
    /// L22 findings (`lint::coverage` over the scenes of every component
    /// view), printed after all views (CLI-1); `view: None` on each.
    pub coverage: Vec<Finding>,
}

impl AllViews {
    /// The build of the view keyed `key`, `None` when there is none.
    /// Serves CLI-1, CLI-5.
    #[must_use]
    pub fn view(&self, key: &str) -> Option<&ViewBuild> {
        todo!()
    }

    /// True when any view's report has an error or `coverage` is not
    /// empty: exit code 1 (CLI-2). Serves CLI-2, CORE-16, CORE-21.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        todo!()
    }
}

/// A view key that is not in the file. `Display` names the key and lists
/// the valid keys (SRS 5.1: says what to change). Serves CORE-28, CLI-1.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UnknownView {
    /// The key asked for, as given.
    pub key: String,
    /// Every view key of the file (`ViewSpec::key`), in declaration
    /// order; empty when the file has no views.
    pub valid: Vec<String>,
}

impl fmt::Display for UnknownView {
    /// `no view "<key>" — valid views: "<a>", "<b>"` (or `— the file has
    /// no views`). Serves CORE-28, SRS 5.1.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl std::error::Error for UnknownView {}

/// Builds the view keyed `key` of `model` and only that view: plan,
/// place with `keep`, route with `RouteConfig::DEFAULT`, labels, scene,
/// lint; no other view is planned or routed (CORE-28). The editor shows
/// its first view from this before routing the rest in the background
/// (EDIT-14). Deterministic: same model, key and keep give an equal
/// build (CORE-25). Serves CORE-28, CORE-27, CORE-24, CORE-29, EDIT-14.
///
/// # Errors
///
/// `UnknownView` when no view of `model.views` has that key, listing the
/// valid ones.
pub fn build_view(model: &Model, key: &str, keep: &Keep) -> Result<ViewBuild, UnknownView> {
    todo!()
}

/// Builds `view` of `model` as `build_view` does, for a spec that need
/// not be in `model.views`: the EDIT-15 default views of `default_view`.
/// Serves CORE-28, EDIT-15.
///
/// # Panics
///
/// When `view` does not belong to `model` (its element ids index out of
/// range).
#[must_use]
pub fn build_spec(model: &Model, view: &ViewSpec, keep: &Keep) -> ViewBuild {
    todo!()
}

/// Builds every view of `model.views` in declaration order, each from
/// `Keep::default()` (the CLI's case: positions from the file only), then
/// runs L22 once over the scenes of every component view (CORE-21).
/// A file with no views gives empty `views` and `coverage`.
/// Deterministic (CORE-25). Serves CORE-11, CORE-21, CORE-25, CLI-1,
/// CLI-3, CLI-5.
#[must_use]
pub fn build_all(model: &Model) -> AllViews {
    todo!()
}

/// The drag preview (EDIT-2, EDIT-20, EDIT-25): `previous` with the boxes
/// in `moved` at their new centres (grid units) and every other box
/// where `previous` had it; only the lines the move broke are re-routed
/// (`route::reroute` with `config`), labels re-placed, the scene rebuilt
/// and re-linted, so findings follow the drag (EDIT-6). An empty `moved`,
/// or one whose centres equal `previous`'s, gives a build equal to
/// `previous`. A `BoxId` out of range is ignored. After release the
/// editor runs a full `build_view` on the saved file instead (EDIT-25).
/// Deterministic (CORE-25). Serves EDIT-2, EDIT-20, EDIT-25, CORE-24.
///
/// # Panics
///
/// When `previous` was not built from `model` (its plan's element ids
/// index out of range), or `config.orders == 0`.
#[must_use]
pub fn rebuild_after_move(
    model: &Model,
    previous: &ViewBuild,
    moved: &BTreeMap<BoxId, Point>,
    config: &RouteConfig,
) -> ViewBuild {
    todo!()
}

/// The `Keep` for building the same view from `new_model` after a
/// re-parse (EDIT-1 reload, EDIT-3 release, EDIT-24 re-apply): every box
/// of `old_plan` (built from `old_model`) at `old_centres[box]` (the
/// scene on screen, `Scene::centres`), re-keyed from its old
/// `ElementId` to the id of the element with the same key in
/// `new_model` (`Element::key`, `Model::find`), so an element inserted
/// earlier in the file does not shift centres onto other boxes
/// (CORE-29). Elements with no key or no match in `new_model` are
/// dropped; `moved` is empty. Serves CORE-29, EDIT-1, EDIT-4.
///
/// # Panics
///
/// When `old_centres` is shorter than `old_plan.boxes`, or `old_plan` was
/// not built from `old_model`.
// TODO(p2): elements with no key are dropped, which is lossless in phase
// 1 (`Element::key` is always `Some`). Phase-2 hierarchical ids and
// unkeyed elements need a fallback match (e.g. by parent key and
// name), or CORE-29 breaks for them.
#[must_use]
pub fn keep_across_reparse(
    old_model: &Model,
    old_plan: &ViewPlan,
    old_centres: &[Point],
    new_model: &Model,
) -> Keep {
    todo!()
}

/// The EDIT-15 default view of `scope`: a `kind` view keyed
/// `<scope id>-<kind>` (`<kind>` is `ViewKind::keyword`, as `edit`'s
/// `AddDefaultView` writes it), with `include *`, no title line (the
/// description is the scope's name, so it has a drawn title), no
/// options, no `at` or `size` line (every box auto-placed). It is not
/// written in the file, so every span of the spec itself (scope,
/// `include *`) is the placeholder `Span { file: FileId::ROOT, start: 0,
/// end: 0 }`; lint findings on its elements and relationships still
/// carry their file spans, and only L5 (which points at view lines) is
/// skipped for it.
///
/// `None` when no default view opens:
/// - `scope`'s kind does not fit `kind` (a software system for
///   `SystemContext` and `Container`, a container for `Component`);
/// - `scope` is out of range;
/// - `model.views` already has a `kind` view scoped to `scope`: the
///   editor opens the first declared one instead (EDIT-15);
/// - the key `<scope id>-<kind>` is held by a view of another scope or
///   kind: the default view cannot be opened, because `edit`'s
///   `AddDefaultView` would refuse its first write.
///
/// Serves EDIT-15.
// TODO(srs): EDIT-15 does not say what a default view's drawn title is
// (L8 needs one). Stubbed against Structurizr's generated view
// description: the scope's name (docs/deferred.md row).
#[must_use]
pub fn default_view(model: &Model, scope: ElementId, kind: ViewKind) -> Option<ViewSpec> {
    todo!()
}

#[cfg(test)]
mod tests {
    /// CORE-28: in a file with views `a` and `b`, `build_view(.., "a",
    /// ..)` equals `build_view` of the same file with view `b` deleted:
    /// nothing of `b` (plan, routes) reaches `a`'s build. It also equals
    /// `a`'s entry in `build_all`.
    #[test]
    fn core_28_one_view_routes_alone() {
        todo!()
    }

    /// Plan Review Focus: an unknown key errors naming the key and
    /// listing the valid ones in declaration order.
    #[test]
    fn unknown_view_key_lists_valid() {
        todo!()
    }

    /// SRS 5.1: `UnknownView` on a file with no views displays
    /// `no view "<key>" — the file has no views`.
    #[test]
    fn unknown_view_display_without_views() {
        todo!()
    }

    /// Plan Review Focus: a file with no views builds to an empty
    /// `AllViews`, not an error.
    #[test]
    fn build_all_without_views_is_empty() {
        todo!()
    }

    /// Plan Review Focus: `model::parse` of an invalid file returns `Err`
    /// and no model (the pipeline takes a `Model` only from `Ok`).
    #[test]
    fn parse_error_leaks_no_model() {
        todo!()
    }

    /// CORE-21 wiring: `build_all` reports L22 once, outside any view,
    /// and `has_errors` is then true (CLI-2).
    #[test]
    fn core_21_build_all_reports_coverage_once() {
        todo!()
    }

    /// CLI-2: a view report with an error and an empty `coverage` makes
    /// `has_errors` true; a file with warnings only makes it false.
    #[test]
    fn cli_2_has_errors_on_view_error_without_coverage() {
        todo!()
    }

    /// EDIT-25: `rebuild_after_move` re-routes only the lines the move
    /// broke; lines away from the moved box keep their points.
    #[test]
    fn edit_25_rebuild_after_move_keeps_untouched_lines() {
        todo!()
    }

    /// EDIT-25: an empty `moved` gives a build equal to `previous`.
    #[test]
    fn edit_25_rebuild_after_move_without_move_is_identity() {
        todo!()
    }

    /// EDIT-25: a `BoxId` out of range in `moved` is ignored: the build
    /// equals the one without that entry.
    #[test]
    fn edit_25_rebuild_after_move_ignores_out_of_range_box() {
        todo!()
    }

    /// EDIT-20: two entries in `moved` move both boxes together; every
    /// other box keeps its centre from `previous`.
    #[test]
    fn edit_20_rebuild_after_move_moves_several_boxes() {
        todo!()
    }

    /// EDIT-6: a drag that makes two boxes overlap gives an L6 finding in
    /// the preview's `report`, absent from `previous.report`.
    #[test]
    fn edit_6_rebuild_after_move_relints() {
        todo!()
    }

    /// CORE-24 / EDIT-25: `PREVIEW_ROUTE` has `orders >= 1`, and neither
    /// more orders nor a higher `max_iterations` than
    /// `RouteConfig::DEFAULT`, so step-5 tuning cannot break its contract.
    #[test]
    fn core_24_preview_route_not_heavier_than_default() {
        todo!()
    }

    /// CORE-29 across a re-parse (docs/deferred.md row): an element
    /// inserted earlier in the file shifts `ElementId`s, yet every
    /// previously drawn auto-placed box keeps its centre.
    #[test]
    fn core_29_previous_kept_across_reparse_with_element_inserted_earlier() {
        todo!()
    }

    /// EDIT-25 after release (see TODO(srs) in the header): the full
    /// build of the saved file with the session's `Keep` equals the build
    /// from the file alone when every box has an `at` line.
    #[test]
    fn edit_25_release_build_equals_build_from_saved_file() {
        todo!()
    }

    /// EDIT-15: the default view of a container is keyed
    /// `<id>-component`, includes everything, builds with every box
    /// auto-placed, and is `None` for a scope of the wrong kind.
    #[test]
    fn edit_15_default_view_key_scope_and_kind() {
        todo!()
    }

    /// EDIT-15: the system context default view of a software system is
    /// keyed `<id>-systemContext`.
    #[test]
    fn edit_15_default_view_system_context_key() {
        todo!()
    }

    /// EDIT-15: `None` when a `kind` view scoped to `scope` is already in
    /// `model.views`, even under a key other than `<scope id>-<kind>`.
    #[test]
    fn edit_15_default_view_none_when_scoped_view_exists() {
        todo!()
    }

    /// EDIT-15: `None` when the key `<scope id>-<kind>` is held by a view
    /// of another scope or another kind (`edit` would refuse the write).
    #[test]
    fn edit_15_default_view_none_when_key_taken() {
        todo!()
    }

    /// EDIT-15: `None` for a scope `ElementId` out of range.
    #[test]
    fn edit_15_default_view_none_for_out_of_range_scope() {
        todo!()
    }

    /// CORE-25: `build_view` twice on the same input gives equal builds.
    #[test]
    fn core_25_build_view_deterministic() {
        todo!()
    }
}
