//! The lint resolve part of the editor state (SRS 4.9 lint-resolve,
//! P5-406..P5-409): which findings of the shown build have a resolution
//! (`c4rizer_core::resolve`), computed once when the build arrives, and
//! the commands that write one finding's resolution or every finding's
//! at once through the move and resize write path, one undo step each.
//!
//! Depends on: `state`, `worker`, `c4rizer-core` (resolve, pipeline,
//! geom, view).

// DECISION: the resolutions are tied to the shown build by
// `Arc::ptr_eq` (as `unbuilt` is) rather than cleared wherever `shown`
// changes: any other build on screen makes them void by itself, and the
// Lint section then shows no Resolve button until they are recomputed.

use std::collections::BTreeMap;
use std::sync::Arc;

use c4rizer_core::geom::{GRID_PX, Point};
use c4rizer_core::pipeline::ViewBuild;
use c4rizer_core::resolve::{Resolution, resolve, resolve_all};
use c4rizer_core::view::BoxId;

use super::{Action, Effect, Gesture, State, edit_key};

/// The resolutions of one shown build's findings.
#[derive(Debug, Clone)]
pub(super) struct Resolutions {
    /// The build they were computed for.
    of: Arc<ViewBuild>,
    /// One per finding of `of.report`, `None` when it has none.
    each: Vec<Option<Resolution>>,
    /// `resolve_all` of the report.
    all: Option<Resolution>,
}

impl State {
    /// Recomputes the resolutions for the shown build (P5-406): called
    /// when a build is shown, never per frame.
    pub(super) fn refresh_resolutions(&mut self) {
        self.resolutions = self.shown.as_ref().map(|(parsed, build)| {
            let (model, scene, findings) = (&parsed.model, &build.scene, &build.report.findings);
            Resolutions {
                of: Arc::clone(build),
                each: findings.iter().map(|f| resolve(model, scene, f)).collect(),
                all: resolve_all(model, scene, findings),
            }
        });
    }

    /// The resolutions of the shown build, `None` while they do not
    /// match `findings` (a drag shows its own report; another build is on
    /// screen).
    fn current_resolutions(&self) -> Option<&Resolutions> {
        if matches!(self.gesture, Gesture::Dragging { .. }) && self.dragged.is_some() {
            return None;
        }
        match (&self.resolutions, &self.shown) {
            (Some(r), Some((_, shown))) if Arc::ptr_eq(&r.of, shown) => Some(r),
            _ => None,
        }
    }

    /// The resolution of the `n`-th finding of `findings`, `None` when it
    /// has none (the Lint row then shows no Resolve button). Serves
    /// P5-406.
    // PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-17, #7).
    #[must_use]
    pub fn resolution(&self, n: usize) -> Option<&Resolution> {
        self.current_resolutions()?.each.get(n)?.as_ref()
    }

    /// How many findings have a resolution, `0` with none. The Lint
    /// section offers "Resolve all" from two. Serves P5-408.
    // PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-17, #7).
    #[must_use]
    pub fn resolvable(&self) -> usize {
        self.current_resolutions().map_or(0, |r| r.each.iter().filter(|e| e.is_some()).count())
    }

    /// Writes the resolution of the `n`-th finding (P5-407).
    pub(super) fn resolve_finding(&mut self, n: usize) -> Vec<Effect> {
        let Some(r) = self.resolution(n).cloned() else { return Vec::new() };
        self.write_resolution(&r)
    }

    /// Writes the resolution of every finding at once (P5-408).
    pub(super) fn resolve_every_finding(&mut self) -> Vec<Effect> {
        let Some(r) = self.current_resolutions().and_then(|r| r.all.clone()) else { return Vec::new() };
        self.write_resolution(&r)
    }

    /// One resolution, one write, one undo step: moves alone go through
    /// `write_moves` (EDIT-26 first write, carried vertices); with sizes,
    /// an `Action::Arrange` of the moves and sizes.
    fn write_resolution(&mut self, r: &Resolution) -> Vec<Effect> {
        let Some(scene) = self.shown.as_ref().map(|(_, b)| &b.scene) else { return Vec::new() };
        let to: BTreeMap<BoxId, Point> = r.moves.iter().copied().collect();
        if r.sizes.is_empty() {
            let from: BTreeMap<BoxId, Point> =
                to.keys().filter_map(|id| scene.centres.get(id.0 as usize).map(|c| (*id, *c))).collect();
            return self.write_moves(&to, Some(&from));
        }
        let (Some(view), Some((parsed, build))) = (self.view.clone(), self.shown.as_ref()) else {
            return Vec::new();
        };
        let (model, scene) = (&parsed.model, &build.scene);
        let key = |id: &BoxId| scene.boxes.get(id.0 as usize).and_then(|b| edit_key(model, b.element));
        let boxes: BTreeMap<_, _> =
            to.iter().filter_map(|(id, c)| Some((key(id)?, (c.x * GRID_PX, c.y * GRID_PX)))).collect();
        let sizes: BTreeMap<_, _> = r.sizes.iter().filter_map(|(id, s)| Some((key(id)?, *s))).collect();
        let mut layout = self.layout();
        for (k, c) in &boxes {
            layout.insert(k.clone(), *c);
        }
        self.write(Action::Arrange {
            view,
            boxes,
            sizes,
            layout,
        })
    }
}
