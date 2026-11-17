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
//   build_view(model, key, keep, stop)  -> ViewBuild | BuildError
//                                          (UnknownView or Stopped)
//   build_spec(model, spec, keep, stop) -> ViewBuild | Stopped (any spec,
//                                          EDIT-15 default views too)
//   build_all(model, stop)              -> AllViews | Stopped (CLI)
//   rebuild_after_move(model, prev, moved, config, stop)
//                                       -> ViewBuild | Stopped
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
// Review Focus); `build_spec` and `build_all` fail only by being stopped.
// Cooperative stop (EDIT-25, owner's ruling): every build function takes
// the stop flag `route::route` / `route::reroute` read and passes it
// through; a stopped route makes the build return `Stopped` instead of a
// build, never a partial one. The editor's worker sets the flag; the CLI
// passes one never set, and a build that is not stopped gives the same
// result as before the flag existed (CORE-25).
// DECISION: `build_view` returns `BuildError` (unknown key or stopped),
// so the editor tells "no such view" from "superseded" in one match; the
// others return `route::Stopped` itself.
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
// selects them while the name still carries the requirement id. CORE-31
// got its budget at step 5 (proposed in SRS CORE-31 from the first
// measurement), so `perf_core_31_generated_view_under_budget` is a perf
// test like the others.
//
// EDIT-25: after release the scene shown equals the scene built from the
// saved file with the session's previous positions (CORE-29): `build_view`
// of the saved file with the same `Keep::previous` and an empty
// `Keep::moved`. With EDIT-26 every box has an `at` line after the first
// write, so from then on that equals the build from the file alone, and
// the editor and the CLI give the same scene.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::geom::Point;
use crate::label::Labels;
use crate::lint::{Finding, Report};
use crate::model::{
    ElementId, ElementKind, Mode, Model, Selector, Spanned, ViewKind, ViewScope, ViewSpec,
};
use crate::place::Keep;
use crate::route::{RouteConfig, Routes, Stopped};
use crate::scene::Scene;
use crate::text::{FileId, Span};
use crate::view::{BoxId, ViewPlan};
use crate::{label, lint, place, route, scene, view};

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
    /// Every view of `Model::views` whose kind is not
    /// `ViewKind::drawable`, in declaration order; none of them is in
    /// `views`, and none adds to `has_errors` (CLI-2). Serves P2-2,
    /// P2-122.
    pub unsupported: Vec<UnsupportedView>,
}

impl AllViews {
    /// The build of the view keyed `key`, `None` when there is none.
    /// Serves CLI-1, CLI-5.
    #[must_use]
    pub fn view(&self, key: &str) -> Option<&ViewBuild> {
        self.views.iter().find(|v| v.plan.key == key)
    }

    /// True when any view's report has an error or `coverage` is not
    /// empty: exit code 1 (CLI-2). Serves CLI-2, CORE-16, CORE-21.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.coverage.is_empty() || self.views.iter().any(|v| v.report.has_errors())
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
        write!(f, "no view \"{}\" — ", self.key)?;
        if self.valid.is_empty() {
            return f.write_str("the file has no views");
        }
        f.write_str("valid views: ")?;
        for (i, key) in self.valid.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "\"{key}\"")?;
        }
        Ok(())
    }
}

impl std::error::Error for UnknownView {}

/// A view c4rizer does not draw yet (`ViewKind::drawable` is false):
/// `dynamic`, `deployment`, and `systemLandscape`, `filtered`, `custom`
/// and `image` until landscape-filtered lands. Serves P2-2, P2-122,
/// P2-123, P2-180.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UnsupportedView {
    /// The view's effective key (`ViewSpec::key`).
    pub key: String,
    /// The drawn title (`ViewSpec::drawn_title`), if any.
    pub title: Option<String>,
    /// The view kind.
    pub kind: ViewKind,
}

impl fmt::Display for UnsupportedView {
    /// The CLI's indented line after `== <key>: <title>` (P2-122),
    /// without the three-space indent.
    // TODO(srs): P2-122, the exact line text. Reading taken (B
    // dsl-breadth 24): `not yet supported: <keyword> views are not drawn
    // by this version`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "not yet supported: {} views are not drawn by this version",
            self.kind.keyword()
        )
    }
}

impl UnsupportedView {
    /// The entry for `view`, a view whose kind is not drawable.
    fn of(view: &ViewSpec) -> UnsupportedView {
        UnsupportedView {
            key: view.key.clone(),
            title: view.drawn_title().map(|t| t.value.clone()),
            kind: view.kind,
        }
    }
}

/// Why `build_view` gave no build: the key is not in the file, or the
/// run was stopped through its stop flag (EDIT-25). `#[non_exhaustive]`
/// for later causes. Serves CORE-28, EDIT-25.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BuildError {
    /// No view of the file has that key.
    UnknownView(UnknownView),
    /// The stop flag was set; no build was produced.
    Stopped(Stopped),
    /// The key names a view whose kind is not drawable yet; nothing is
    /// planned, placed or routed (P2-2).
    Unsupported(UnsupportedView),
}

impl From<UnknownView> for BuildError {
    /// Wraps the unknown key. Serves CORE-28.
    fn from(e: UnknownView) -> BuildError {
        BuildError::UnknownView(e)
    }
}

impl From<Stopped> for BuildError {
    /// Wraps the stop. Serves EDIT-25.
    fn from(e: Stopped) -> BuildError {
        BuildError::Stopped(e)
    }
}

impl fmt::Display for BuildError {
    /// The `UnknownView` or `Stopped` text. Serves CORE-28, SRS 5.1.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::UnknownView(e) => e.fmt(f),
            BuildError::Stopped(e) => e.fmt(f),
            BuildError::Unsupported(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for BuildError {}

/// Builds the view keyed `key` of `model` and only that view: plan,
/// place with `keep`, route with `RouteConfig::DEFAULT`, labels, scene,
/// lint; no other view is planned or routed (CORE-28). The editor shows
/// its first view from this before routing the rest in the background
/// (EDIT-14). Deterministic: same model, key and keep give an equal
/// build (CORE-25). `stop` goes to `route::route`; a build that is not
/// stopped is the same as without the flag. Serves CORE-28, CORE-27,
/// CORE-24, CORE-29, EDIT-14, EDIT-25.
///
/// # Errors
///
/// `BuildError::UnknownView` when no view of `model.views` has that key,
/// listing the valid ones; `BuildError::Unsupported` when that view's
/// kind is not drawable (P2-2: nothing is planned, placed or routed);
/// `BuildError::Stopped` when `stop` was set while routing (EDIT-25).
pub fn build_view(
    model: &Model,
    key: &str,
    keep: &Keep,
    stop: &AtomicBool,
) -> Result<ViewBuild, BuildError> {
    // DECISION: the key is looked up before the stop flag is read, so an
    // unknown key is reported even by a superseded request (nothing to
    // route either way).
    let Some(view) = model.view(key) else {
        return Err(BuildError::UnknownView(UnknownView {
            key: key.to_owned(),
            valid: model.views.iter().map(|v| v.key.clone()).collect(),
        }));
    };
    // DECISION: like an unknown key, an unsupported view is reported
    // before the stop flag is read: there is nothing to route.
    if !drawn(model, view) {
        return Err(BuildError::Unsupported(UnsupportedView::of(view)));
    }
    Ok(build_spec(model, view, keep, stop)?)
}

/// Builds `view` of `model` as `build_view` does, for a spec that need
/// not be in `model.views`: the EDIT-15 default views of `default_view`.
/// Serves CORE-28, EDIT-15, EDIT-25.
///
/// # Errors
///
/// `Stopped` when `stop` was set while routing (EDIT-25).
///
/// # Panics
///
/// When `view` does not belong to `model` (its element ids index out of
/// range), or when `!view.kind.drawable()` (P2-2: callers check the kind,
/// or go through `build_view`, which returns `BuildError::Unsupported`).
pub fn build_spec(
    model: &Model,
    view: &ViewSpec,
    keep: &Keep,
    stop: &AtomicBool,
) -> Result<ViewBuild, Stopped> {
    assert!(
        view.kind.drawable(),
        "build_spec: {} views are not drawn (P2-2)",
        view.kind.keyword()
    );
    build_in(model, view, keep, Mode::Light, stop)
}

/// Labels, scene and lint report of a routed plan: the stages after
/// routing, shared by full builds and the drag preview.
fn finish(model: &Model, plan: ViewPlan, routes: Routes) -> ViewBuild {
    let words = label::Words::of(&model.terminology);
    let labels = label::place_labels_with(&plan, &routes, &words);
    let scene = scene::build_with(&plan, &routes, &labels, &words);
    let report = lint::lint(model, &scene);
    ViewBuild {
        plan,
        routes,
        labels,
        scene,
        report,
    }
}

/// Builds every drawable view of `model.views` in declaration order, each
/// from `Keep::default()` (the CLI's case: positions from the file only),
/// listing the others in `unsupported` (P2-2), then
/// runs L22 once over the scenes of every component view (CORE-21).
/// A file with no views gives empty `views` and `coverage`.
/// Deterministic (CORE-25). `stop` goes to every view's route; the CLI
/// passes a flag never set, so its run is never stopped and gives the
/// same result as before the flag existed. Serves CORE-11, CORE-21,
/// CORE-25, CLI-1, CLI-3, CLI-5.
///
/// # Errors
///
/// `Stopped` when `stop` was set while routing any view; no partial
/// `AllViews` is returned.
pub fn build_all(model: &Model, stop: &AtomicBool) -> Result<AllViews, Stopped> {
    let keep = Keep::default();
    let views = model
        .views
        .iter()
        .filter(|v| drawn(model, v))
        .map(|v| build_spec(model, v, &keep, stop))
        .collect::<Result<Vec<_>, _>>()?;
    let unsupported = model
        .views
        .iter()
        .filter(|v| !drawn(model, v))
        .map(UnsupportedView::of)
        .collect();
    // DECISION: every scene goes to `lint::coverage`, which itself keeps
    // only the component views; filtering here would duplicate its rule.
    let scenes: Vec<&Scene> = views.iter().map(|v| &v.scene).collect();
    let coverage = lint::coverage(model, &scenes);
    Ok(AllViews {
        views,
        coverage,
        unsupported,
    })
}

/// `build_view` in `mode`: the plan from `view::plan_in`, so styles
/// resolve over the top-level and `mode` blocks and the scene carries
/// `mode` (P2-430, P2-431). `build_view(..)` equals
/// `build_view_in(.., Mode::Light, ..)`. The CLI's `--mode` (P2-432) and
/// the editor's light/dark switch (P2-433) call it. Serves P2-430,
/// P2-432, P2-433.
///
/// # Errors
///
/// As `build_view`: `UnknownView`, `Unsupported` for a non-drawable
/// key, `Stopped`.
pub fn build_view_in(
    model: &Model,
    key: &str,
    keep: &Keep,
    mode: Mode,
    stop: &AtomicBool,
) -> Result<ViewBuild, BuildError> {
    let Some(view) = model.view(key) else {
        return Err(BuildError::UnknownView(UnknownView {
            key: key.to_owned(),
            valid: model.views.iter().map(|v| v.key.clone()).collect(),
        }));
    };
    build_spec_in(model, view, keep, mode, stop)
}

/// `build_spec` in `mode`, for any spec, drawable or not (O7). Serves
/// P2-430, P2-433, EDIT-15.
///
/// # Errors
///
/// `BuildError::Unsupported` when `!view.kind.drawable()`;
/// `BuildError::Stopped` when `stop` was set while routing.
///
/// # Panics
///
/// When `view` does not belong to `model`.
pub fn build_spec_in(
    model: &Model,
    view: &ViewSpec,
    keep: &Keep,
    mode: Mode,
    stop: &AtomicBool,
) -> Result<ViewBuild, BuildError> {
    if !drawn(model, view) {
        return Err(BuildError::Unsupported(UnsupportedView::of(view)));
    }
    Ok(build_in(model, view, keep, mode, stop)?)
}

/// Plan in `mode`, place, route, then `finish`.
fn build_in(
    model: &Model,
    view: &ViewSpec,
    keep: &Keep,
    mode: Mode,
    stop: &AtomicBool,
) -> Result<ViewBuild, Stopped> {
    let plan = view::plan_in(model, view, mode);
    let shared = shared_layout(model, view, keep, mode);
    let centres = place::place(&plan, shared.as_ref().unwrap_or(keep));
    let routes = route::route(&plan, &centres, &RouteConfig::DEFAULT, stop)?;
    Ok(finish(model, plan, routes))
}

/// For a `filtered` view, the `Keep` that puts every box where its base
/// view's build puts it (P2-9, P2-312): the base plan placed from its own
/// lines, each centre kept, plus `keep.moved` (a drag in the filtered
/// view). `None` for any other view.
// TODO(srs): P2-312, a box with no `at` line in the base view. DECISION:
// reading taken (B landscape-filtered 1): it takes the centre the base
// view's build gives it with nothing kept (`Keep::default()`, as `render`
// builds the base), so the filtered view and the base view agree.
fn shared_layout(model: &Model, view: &ViewSpec, keep: &Keep, mode: Mode) -> Option<Keep> {
    let base = model.base_view(view)?;
    let base_plan = view::plan_in(model, base, mode);
    let centres = place::place(&base_plan, &Keep::default());
    let mut shared = Keep::default();
    for b in &base_plan.boxes {
        shared.previous.entry(b.element).or_insert(centres[b.id.0 as usize]);
    }
    shared.moved.clone_from(&keep.moved);
    Some(shared)
}

/// True when `view` is drawn: its kind is drawable and, for a filtered
/// view, so is its base's (a deployment base waits for phase 3).
fn drawn(model: &Model, view: &ViewSpec) -> bool {
    view.kind.drawable() && model.base_view(view).is_none_or(|b| b.kind.drawable())
}

/// `build_all` in `mode`: every drawable view built with
/// `build_spec_in(.., mode, ..)`, then L22 once. `build_all(..)` equals
/// `build_all_in(.., Mode::Light, ..)`. Serves P2-430, P2-432.
///
/// # Errors
///
/// `Stopped` as `build_all`.
pub fn build_all_in(model: &Model, mode: Mode, stop: &AtomicBool) -> Result<AllViews, Stopped> {
    let keep = Keep::default();
    let mut views = Vec::new();
    let mut skipped = Vec::new();
    for v in &model.views {
        if drawn(model, v) {
            views.push(build_in(model, v, &keep, mode, stop)?);
        } else {
            skipped.push(UnsupportedView::of(v));
        }
    }
    let scenes: Vec<&Scene> = views.iter().map(|v| &v.scene).collect();
    let coverage = lint::coverage(model, &scenes);
    Ok(AllViews {
        views,
        coverage,
        unsupported: skipped,
    })
}

/// The drag preview (EDIT-2, EDIT-20, EDIT-25): `previous` with the boxes
/// in `moved` at their new centres (grid units) and every other box
/// where `previous` had it; only the lines the move broke are re-routed
/// (`route::reroute` with `config`), labels re-placed, the scene rebuilt
/// and re-linted, so findings follow the drag (EDIT-6). An empty `moved`,
/// or one whose centres equal `previous`'s, gives a build equal to
/// `previous`. A `BoxId` out of range is ignored. After release the
/// editor runs a full `build_view` on the saved file instead (EDIT-25).
/// Deterministic (CORE-25). `stop` goes to `route::reroute`. Serves
/// EDIT-2, EDIT-20, EDIT-25, CORE-24.
///
/// # Errors
///
/// `Stopped` when `stop` was set while re-routing (EDIT-25).
///
/// # Panics
///
/// When `previous` was not built from `model` (its plan's element ids
/// index out of range), or `config.orders == 0`.
pub fn rebuild_after_move(
    model: &Model,
    previous: &ViewBuild,
    moved: &BTreeMap<BoxId, Point>,
    config: &RouteConfig,
    stop: &AtomicBool,
) -> Result<ViewBuild, Stopped> {
    let mut centres = previous.routes.centres.clone();
    for (id, &at) in moved {
        if let Some(c) = centres.get_mut(id.0 as usize) {
            *c = at;
        }
    }
    // DECISION: nothing moved returns `previous` as is (after the stop
    // check, so a stopped preview still yields nothing): the stages after
    // routing are deterministic, so rebuilding them would give an equal
    // build at a cost the drag does not need.
    if centres == previous.routes.centres {
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        return Ok(previous.clone());
    }
    let routes = route::reroute(&previous.plan, &centres, &previous.routes, config, stop)?;
    Ok(finish(model, previous.plan.clone(), routes))
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
    let old = Keep::from_previous(old_plan, old_centres);
    let mut keep = Keep::default();
    for (&element, &centre) in &old.previous {
        let Some(key) = &old_model.element(element).key else {
            continue;
        };
        // DECISION: an identifier is unique across a phase-1 file (DSL-5),
        // so `find` by the bare key is the match; two old elements cannot
        // map to one new id, and the first box of an element wins as in
        // `Keep::from_previous`.
        if let Some(id) = new_model.find(&key.value) {
            keep.previous.entry(id).or_insert(centre);
        }
    }
    keep
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
/// EDIT-15: a default view has no title line; its drawn title is the
/// scope's name. Serves EDIT-15.
#[must_use]
pub fn default_view(model: &Model, scope: ElementId, kind: ViewKind) -> Option<ViewSpec> {
    let element = model.elements.get(scope.0 as usize)?;
    let fits = match kind {
        ViewKind::SystemContext | ViewKind::Container => {
            element.kind == ElementKind::SoftwareSystem
        }
        ViewKind::Component => element.kind == ElementKind::Container,
        // EDIT-15 default views are context, container and component
        // views only: a landscape, filtered, custom or image view has no
        // scope element to default from, and dynamic and deployment views
        // are not drawn in phase 2 (P2-2).
        ViewKind::SystemLandscape
        | ViewKind::Filtered
        | ViewKind::Custom
        | ViewKind::Image
        | ViewKind::Dynamic
        | ViewKind::Deployment => false,
    };
    if !fits {
        return None;
    }
    if model
        .views
        .iter()
        .any(|v| v.kind == kind && v.scope.element() == Some(scope))
    {
        return None;
    }
    // DECISION: an element with no identifier (none in phase 1) has no
    // default view key, so no default view.
    let id = &element.key.as_ref()?.value;
    let key = format!("{id}-{}", kind.keyword());
    if model.view(&key).is_some() {
        return None;
    }
    let none = Span::new(FileId::ROOT, 0, 0);
    Some(ViewSpec {
        kind,
        scope: ViewScope::Element(Spanned {
            value: scope,
            span: none,
        }),
        key,
        declared_key: None,
        description: Some(Spanned {
            value: element.name.value.clone(),
            span: none,
        }),
        title: None,
        subtitle: None,
        include: vec![Spanned {
            value: Selector::All,
            span: none,
        }],
        exclude: Vec::new(),
        auto_layout: None,
        focus: Vec::new(),
        hide: Vec::new(),
        technology: None,
        bus: None,
        label_width: None,
        max_crossings: None,
        positions: Vec::new(),
        sizes: Vec::new(),
        span: none,
        properties: Vec::new(),
        filter: None,
        image: None,
        default: None,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::geom::Point;
    use crate::lint::{Rule, Severity};
    use crate::model::{ElementKind, Model, ViewKind, parse};
    use crate::place::Keep;
    use crate::route::RouteConfig;
    use crate::view::BoxId;

    /// A shop with roles only: two views, `a` (containers) declared
    /// before `b` (the front's components), so deleting `b` shifts no
    /// span that `a`'s build carries.
    const TWO_VIEWS: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods." {
      front = container "Front" "Shows pages." "Rust" {
        pages = component "Pages" "Renders pages." "Rust"
        cart = component "Cart" "Holds the cart." "Rust"
      }
      store = container "Store" "Keeps orders." "SQLite"
    }
    bank = softwareSystem "Bank" "Takes payments." "External"
    operator -> pages "Browses"
    pages -> cart "Fills"
    cart -> store "Writes carts to" "SQL"
    front -> bank "Charges cards through" "HTTPS"
  }
  views {
    container shop "a" "Shop containers" {
      include *
    }
    component front "b" "Front components" {
      include *
    }
  }
}
"#;

    /// `TWO_VIEWS` with view `b` deleted.
    fn only_a() -> String {
        let start = TWO_VIEWS.find("    component front \"b\"").expect("view b");
        let end = start + TWO_VIEWS[start..].find("    }\n").expect("end of b") + 6;
        format!("{}{}", &TWO_VIEWS[..start], &TWO_VIEWS[end..])
    }

    /// A file with a model and no views.
    const NO_VIEWS: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods."
    operator -> shop "Places orders in"
  }
}
"#;

    /// A component view of `front` that leaves `cart` out, so
    /// `pages -> cart` and `cart -> store` are on no component view
    /// (L22).
    const UNCOVERED: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods." {
      front = container "Front" "Shows pages." "Rust" {
        pages = component "Pages" "Renders pages." "Rust"
        cart = component "Cart" "Holds the cart." "Rust"
      }
      store = container "Store" "Keeps orders." "SQLite"
    }
    operator -> pages "Browses"
    pages -> cart "Fills"
    cart -> store "Writes carts to" "SQL"
  }
  views {
    component front "c3" "Front components" {
      include *
      exclude cart
    }
  }
}
"#;

    /// A container without technology (L10, an error), no component view.
    const MISSING_TECH: &str = r#"workspace {
  model {
    operator = person "Operator" "Runs the shop day to day."
    shop = softwareSystem "Shop" "Sells goods online." {
      web = container "Web app" "Serves the shop pages." "Rust"
      worker = container "Worker" "Runs background jobs."
    }
    operator -> web "Manages orders in" "HTTPS"
    worker -> web "Reports job results to" "HTTPS"
  }
  views {
    container shop "c2" "Shop containers" {
      include *
    }
  }
}
"#;

    /// Two systems that use each other (L14, a warning) and nothing else
    /// wrong.
    const TWO_WAY: &str = r#"workspace {
  model {
    shop = softwareSystem "Shop" "Sells goods online."
    bank = softwareSystem "Bank" "Takes card payments."
    shop -> bank "Charges cards through"
    bank -> shop "Confirms payments to"
  }
  views {
    systemContext shop "c1" "Two way" {
      include *
    }
  }
}
"#;

    /// Four systems, every box placed by an `at` line: `a -> b` on the top
    /// row, `c -> d` far below it.
    const PLACED: &str = r#"workspace {
  model {
    a = softwareSystem "Alpha" "First system."
    b = softwareSystem "Beta" "Second system."
    c = softwareSystem "Gamma" "Third system."
    d = softwareSystem "Delta" "Fourth system."
    a -> b "Sends orders to"
    c -> d "Sends reports to"
  }
  views {
    systemContext a "ctx" "Placed" {
      include a b c d
      at a 200 200
      at b 700 200
      at c 200 900
      at d 700 900
    }
  }
}
"#;

    /// Three auto-placed systems in a context view.
    const AUTO: &str = r#"workspace {
  model {
    operator = person "Operator" "Runs the shop."
    shop = softwareSystem "Shop" "Sells goods online."
    bank = softwareSystem "Bank" "Takes card payments." "External"
    operator -> shop "Manages orders in"
    shop -> bank "Charges cards through"
  }
  views {
    systemContext shop "c1" "Shop context" {
      include *
    }
  }
}
"#;

    /// `AUTO` with a new system declared before every other element and
    /// included in the view: every old `ElementId` shifts by one.
    const AUTO_INSERTED: &str = r#"workspace {
  model {
    stock = softwareSystem "Stock" "Counts goods."
    operator = person "Operator" "Runs the shop."
    shop = softwareSystem "Shop" "Sells goods online."
    bank = softwareSystem "Bank" "Takes card payments." "External"
    operator -> shop "Manages orders in"
    shop -> bank "Charges cards through"
    shop -> stock "Reserves goods in"
  }
  views {
    systemContext shop "c1" "Shop context" {
      include *
    }
  }
}
"#;

    fn model(src: &str) -> Model {
        match parse(src) {
            Ok(p) => p.model,
            Err(e) => panic!("{e}"),
        }
    }

    fn go() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn build(m: &Model, key: &str) -> ViewBuild {
        build_view(m, key, &Keep::default(), &go()).expect("view builds")
    }

    fn element_box(b: &ViewBuild, m: &Model, key: &str) -> BoxId {
        let e = m.find(key).expect("element");
        b.plan.boxes_of(e)[0]
    }

    #[test]
    fn core_28_one_view_routes_alone() {
        let full = model(TWO_VIEWS);
        let alone = model(&only_a());
        assert_eq!(full.views.len(), 2);
        assert_eq!(alone.views.len(), 1);
        let a = build(&full, "a");
        assert_eq!(a.plan.key, "a");
        assert_eq!(a.scene.key, "a");
        assert_eq!(a, build(&alone, "a"));
        let all = build_all(&full, &go()).expect("never stopped");
        assert_eq!(all.view("a"), Some(&a));
    }

    #[test]
    fn unknown_view_key_lists_valid() {
        let m = model(TWO_VIEWS);
        let err = build_view(&m, "zz", &Keep::default(), &go()).expect_err("no such view");
        let BuildError::UnknownView(u) = &err else {
            panic!("expected UnknownView, got {err:?}");
        };
        assert_eq!(u.key, "zz");
        assert_eq!(u.valid, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(
            err.to_string(),
            r#"no view "zz" — valid views: "a", "b""#
        );
    }

    #[test]
    fn unknown_view_display_without_views() {
        let m = model(NO_VIEWS);
        let err = build_view(&m, "c1", &Keep::default(), &go()).expect_err("no views");
        let BuildError::UnknownView(u) = &err else {
            panic!("expected UnknownView, got {err:?}");
        };
        assert!(u.valid.is_empty());
        assert_eq!(u.to_string(), r#"no view "c1" — the file has no views"#);
        assert_eq!(err.to_string(), u.to_string());
    }

    #[test]
    fn build_all_without_views_is_empty() {
        let all = build_all(&model(NO_VIEWS), &go()).expect("never stopped");
        assert!(all.views.is_empty());
        assert!(all.coverage.is_empty());
        assert!(!all.has_errors());
        assert_eq!(all.view("c1"), None);
    }

    #[test]
    fn parse_error_leaks_no_model() {
        // Unclosed block, then an unknown construct: both are `Err` and
        // carry no model at all.
        assert!(parse("workspace {\n  model {\n").is_err());
        let bad = TWO_VIEWS.replace("include *", "include * nonsense {");
        assert!(parse(&bad).is_err());
        assert!(parse(TWO_VIEWS).is_ok());
    }

    #[test]
    fn core_21_build_all_reports_coverage_once() {
        let m = model(UNCOVERED);
        let all = build_all(&m, &go()).expect("never stopped");
        assert_eq!(all.views.len(), 1);
        assert_eq!(all.coverage.len(), 2, "{:#?}", all.coverage);
        assert!(
            all.coverage
                .iter()
                .all(|f| f.rule == Rule::Coverage && f.view.is_none())
        );
        let mut subjects: Vec<_> = all.coverage.iter().map(|f| f.subjects.clone()).collect();
        subjects.dedup();
        assert_eq!(subjects.len(), 2, "each relationship once");
        assert!(
            all.views
                .iter()
                .flat_map(|v| &v.report.findings)
                .all(|f| f.rule != Rule::Coverage),
            "no L22 inside a view report"
        );
        assert!(all.has_errors());
    }

    #[test]
    fn cli_2_has_errors_on_view_error_without_coverage() {
        let all = build_all(&model(MISSING_TECH), &go()).expect("never stopped");
        assert!(all.coverage.is_empty());
        assert!(all.views[0].report.has_errors());
        assert!(
            all.views[0]
                .report
                .findings
                .iter()
                .any(|f| f.rule == Rule::NoTechnology)
        );
        assert!(all.has_errors());

        let warn = build_all(&model(TWO_WAY), &go()).expect("never stopped");
        let report = &warn.views[0].report;
        assert!(
            report.findings.iter().any(|f| f.rule == Rule::TwoWay),
            "{:#?}",
            report.findings
        );
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.severity() == Severity::Warning),
            "{:#?}",
            report.findings
        );
        assert!(!warn.has_errors());
    }

    #[test]
    fn edit_25_rebuild_after_move_keeps_untouched_lines() {
        let m = model(PLACED);
        let prev = build(&m, "ctx");
        let a = element_box(&prev, &m, "a");
        let c = element_box(&prev, &m, "c");
        let d = element_box(&prev, &m, "d");
        let c_to_d = prev
            .plan
            .edges
            .iter()
            .find(|e| e.source == c && e.destination == d)
            .expect("c -> d")
            .id;
        let old = prev.routes.centres[a.0 as usize];
        let moved = BTreeMap::from([(a, Point::new(old.x, old.y + 3))]);
        let next = rebuild_after_move(&m, &prev, &moved, &PREVIEW_ROUTE, &go()).expect("runs");
        assert_eq!(next.routes.centres[a.0 as usize], Point::new(old.x, old.y + 3));
        assert_eq!(next.scene.centres[a.0 as usize], Point::new(old.x, old.y + 3));
        assert_eq!(
            next.routes.line(c_to_d),
            prev.routes.line(c_to_d),
            "the line away from the moved box keeps its points"
        );
        assert!(next.routes.line(c_to_d).is_some());
        assert_ne!(next.scene, prev.scene);
    }

    #[test]
    fn edit_25_rebuild_after_move_without_move_is_identity() {
        let m = model(PLACED);
        let prev = build(&m, "ctx");
        let next =
            rebuild_after_move(&m, &prev, &BTreeMap::new(), &PREVIEW_ROUTE, &go()).expect("runs");
        assert_eq!(next, prev);
        // Same centres as before: also the identity.
        let a = element_box(&prev, &m, "a");
        let same = BTreeMap::from([(a, prev.routes.centres[a.0 as usize])]);
        let next = rebuild_after_move(&m, &prev, &same, &PREVIEW_ROUTE, &go()).expect("runs");
        assert_eq!(next, prev);
    }

    #[test]
    fn edit_25_rebuild_after_move_ignores_out_of_range_box() {
        let m = model(PLACED);
        let prev = build(&m, "ctx");
        let a = element_box(&prev, &m, "a");
        let old = prev.routes.centres[a.0 as usize];
        let one = BTreeMap::from([(a, Point::new(old.x, old.y + 3))]);
        let mut two = one.clone();
        two.insert(BoxId(999), Point::new(0, 0));
        let with = rebuild_after_move(&m, &prev, &two, &PREVIEW_ROUTE, &go()).expect("runs");
        let without = rebuild_after_move(&m, &prev, &one, &PREVIEW_ROUTE, &go()).expect("runs");
        assert_eq!(with, without);
        let only_bad = BTreeMap::from([(BoxId(999), Point::new(0, 0))]);
        let none = rebuild_after_move(&m, &prev, &only_bad, &PREVIEW_ROUTE, &go()).expect("runs");
        assert_eq!(none, prev);
    }

    #[test]
    fn edit_20_rebuild_after_move_moves_several_boxes() {
        let m = model(PLACED);
        let prev = build(&m, "ctx");
        let a = element_box(&prev, &m, "a");
        let b = element_box(&prev, &m, "b");
        let pa = prev.routes.centres[a.0 as usize];
        let pb = prev.routes.centres[b.0 as usize];
        let to_a = Point::new(pa.x, pa.y + 4);
        let to_b = Point::new(pb.x, pb.y + 4);
        let moved = BTreeMap::from([(a, to_a), (b, to_b)]);
        let next = rebuild_after_move(&m, &prev, &moved, &PREVIEW_ROUTE, &go()).expect("runs");
        for (i, (&was, &now)) in prev
            .scene
            .centres
            .iter()
            .zip(&next.scene.centres)
            .enumerate()
        {
            let id = BoxId(u32::try_from(i).expect("small"));
            match moved.get(&id) {
                Some(&to) => assert_eq!(now, to, "{id:?} moved"),
                None => assert_eq!(now, was, "{id:?} stays"),
            }
        }
        let rect_a = next.scene.boxes[a.0 as usize].rect;
        let rect_a_was = prev.scene.boxes[a.0 as usize].rect;
        assert_eq!(rect_a.min.y - rect_a_was.min.y, 4 * crate::geom::GRID_PX);
    }

    #[test]
    fn edit_6_rebuild_after_move_relints() {
        let m = model(PLACED);
        let prev = build(&m, "ctx");
        assert!(
            prev.report.findings.iter().all(|f| f.rule != Rule::TooClose),
            "{:#?}",
            prev.report.findings
        );
        let a = element_box(&prev, &m, "a");
        let b = element_box(&prev, &m, "b");
        let pb = prev.routes.centres[b.0 as usize];
        // `a` dropped 5 cells left of `b`: their rectangles overlap.
        let moved = BTreeMap::from([(a, Point::new(pb.x - 5, pb.y))]);
        let next = rebuild_after_move(&m, &prev, &moved, &PREVIEW_ROUTE, &go()).expect("runs");
        assert!(
            next.report.findings.iter().any(|f| f.rule == Rule::TooClose),
            "{:#?}",
            next.report.findings
        );
    }

    #[test]
    fn core_24_preview_route_not_heavier_than_default() {
        let preview = PREVIEW_ROUTE;
        assert!(preview.orders >= 1);
        assert!(preview.orders <= RouteConfig::DEFAULT.orders);
        assert!(preview.max_iterations <= RouteConfig::DEFAULT.max_iterations);
    }

    #[test]
    fn core_29_previous_kept_across_reparse_with_element_inserted_earlier() {
        let old_model = model(AUTO);
        let old = build(&old_model, "c1");
        let new_model = model(AUTO_INSERTED);
        // The insertion shifts every old id.
        for key in ["operator", "shop", "bank"] {
            assert_ne!(old_model.find(key), new_model.find(key), "{key}");
        }
        let keep = keep_across_reparse(&old_model, &old.plan, &old.scene.centres, &new_model);
        assert!(keep.moved.is_empty());
        assert_eq!(keep.previous.len(), 3);
        let new = build_view(&new_model, "c1", &keep, &go()).expect("builds");
        assert_eq!(new.plan.boxes.len(), 4);
        for key in ["operator", "shop", "bank"] {
            let was = old.scene.centres[element_box(&old, &old_model, key).0 as usize];
            let now = new.scene.centres[element_box(&new, &new_model, key).0 as usize];
            assert_eq!(now, was, "{key} keeps its centre");
        }
        // Keyed by the new ids.
        let shop_old = old.scene.centres[element_box(&old, &old_model, "shop").0 as usize];
        assert_eq!(
            keep.previous.get(&new_model.find("shop").expect("shop")),
            Some(&shop_old)
        );
    }

    #[test]
    fn edit_25_release_build_equals_build_from_saved_file() {
        let m = model(PLACED);
        let first = build(&m, "ctx");
        let keep = Keep::from_previous(&first.plan, &first.scene.centres);
        let session = build_view(&m, "ctx", &keep, &go()).expect("builds");
        assert_eq!(session, first);
        assert_eq!(session, build(&m, "ctx"));
    }

    #[test]
    fn edit_15_default_view_key_scope_and_kind() {
        let m = model(TWO_VIEWS);
        let store = m.find("store").expect("store");
        let spec = default_view(&m, store, ViewKind::Component).expect("store has none");
        assert_eq!(spec.key, "store-component");
        assert_eq!(spec.kind, ViewKind::Component);
        assert_eq!(spec.scope.element(), Some(store));
        assert_eq!(spec.include.len(), 1);
        assert_eq!(spec.include[0].value, crate::model::Selector::All);
        assert!(spec.positions.is_empty() && spec.sizes.is_empty());
        assert!(spec.title.is_none());
        assert_eq!(
            spec.description.as_ref().map(|d| d.value.as_str()),
            Some("Store")
        );
        let b = build_spec(&m, &spec, &Keep::default(), &go()).expect("builds");
        assert_eq!(b.plan.key, "store-component");
        assert_eq!(b.plan.unplaced().len(), b.plan.boxes.len());
        assert!(
            b.report.findings.iter().all(|f| f.rule != Rule::NoTitle),
            "{:#?}",
            b.report.findings
        );
        // Wrong kind of scope: a container for a context view, a system
        // for a component view, a person for anything.
        assert!(default_view(&m, store, ViewKind::SystemContext).is_none());
        assert!(default_view(&m, store, ViewKind::Container).is_none());
        let bank = m.find("bank").expect("bank");
        assert!(default_view(&m, bank, ViewKind::Component).is_none());
        let operator = m.find("operator").expect("operator");
        assert_eq!(m.element(operator).kind, ElementKind::Person);
        assert!(default_view(&m, operator, ViewKind::SystemContext).is_none());
    }

    #[test]
    fn edit_15_default_view_system_context_key() {
        let m = model(TWO_VIEWS);
        let bank = m.find("bank").expect("bank");
        let spec = default_view(&m, bank, ViewKind::SystemContext).expect("bank has none");
        assert_eq!(spec.key, "bank-systemContext");
        assert_eq!(spec.kind, ViewKind::SystemContext);
        assert_eq!(spec.scope.element(), Some(bank));
        let c2 = default_view(&m, bank, ViewKind::Container).expect("bank has none");
        assert_eq!(c2.key, "bank-container");
    }

    #[test]
    fn edit_15_default_view_none_when_scoped_view_exists() {
        let m = model(TWO_VIEWS);
        // `a` is shop's container view, `b` front's component view: both
        // under keys other than `<scope id>-<kind>`.
        let shop = m.find("shop").expect("shop");
        let front = m.find("front").expect("front");
        assert!(default_view(&m, shop, ViewKind::Container).is_none());
        assert!(default_view(&m, front, ViewKind::Component).is_none());
        assert!(default_view(&m, shop, ViewKind::SystemContext).is_some());
    }

    #[test]
    fn edit_15_default_view_none_when_key_taken() {
        let src = TWO_VIEWS.replace(r#"container shop "a""#, r#"container shop "store-component""#);
        let m = model(&src);
        let store = m.find("store").expect("store");
        assert!(default_view(&m, store, ViewKind::Component).is_none());
        let src = TWO_VIEWS.replace(r#"container shop "a""#, r#"container shop "shop-systemContext""#);
        let m = model(&src);
        let shop = m.find("shop").expect("shop");
        assert!(default_view(&m, shop, ViewKind::SystemContext).is_none());
    }

    #[test]
    fn edit_15_default_view_none_for_out_of_range_scope() {
        let m = model(TWO_VIEWS);
        let out = ElementId(u32::try_from(m.elements.len()).expect("small"));
        assert!(default_view(&m, out, ViewKind::SystemContext).is_none());
        assert!(default_view(&m, ElementId(u32::MAX), ViewKind::Component).is_none());
    }

    #[test]
    fn edit_25_stopped_build_yields_no_build() {
        let m = model(PLACED);
        let stop = AtomicBool::new(true);
        assert!(matches!(
            build_view(&m, "ctx", &Keep::default(), &stop),
            Err(BuildError::Stopped(_))
        ));
        let spec = m.view("ctx").expect("ctx");
        assert!(build_spec(&m, spec, &Keep::default(), &stop).is_err());
        assert!(build_all(&m, &stop).is_err());
        let prev = build(&m, "ctx");
        let a = element_box(&prev, &m, "a");
        let old = prev.routes.centres[a.0 as usize];
        let moved = BTreeMap::from([(a, Point::new(old.x, old.y + 3))]);
        assert!(rebuild_after_move(&m, &prev, &moved, &PREVIEW_ROUTE, &stop).is_err());
        // The unknown key wins over the stop: nothing to route.
        assert!(matches!(
            build_view(&m, "zz", &Keep::default(), &stop),
            Err(BuildError::UnknownView(_))
        ));
        // Never set: same as an unstopped run.
        let all = build_all(&m, &go()).expect("never stopped");
        assert_eq!(all.views[0], prev);
        let spec_build = build_spec(&m, spec, &Keep::default(), &go()).expect("runs");
        assert_eq!(spec_build, prev);
        assert_eq!(
            BuildError::from(crate::route::Stopped).to_string(),
            crate::route::Stopped.to_string()
        );
    }

    #[test]
    fn core_25_build_view_deterministic() {
        let m = model(TWO_VIEWS);
        for key in ["a", "b"] {
            assert_eq!(build(&m, key), build(&m, key), "{key}");
        }
        let m2 = model(TWO_VIEWS);
        assert_eq!(build(&m, "b"), build(&m2, "b"));
    }
}
