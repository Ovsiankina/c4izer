//! The bridges part of the editor state (SRS 4.8 P4-521, P4-522): the
//! right panel's "Crossings" choice writes the view's `crossings` line,
//! and a right-click on a crossing offers "Bridge on the other line",
//! which rewrites the view's `bridge` lines. Every edit is one batch, one
//! undo step.
//!
//! Depends on: `state`, `c4rizer-core` (bridge, edit, model, scene).

// DECISION: the crossing under the pointer is read from the shown scene
// (`Scene::bridges`) and the new `bridge` lines from the shown build's
// plan (`ViewPlan::bridges`) through `bridge::flipped`, so the menu, the
// drawing and the file never disagree, as for buses.

use c4rizer_core::bridge;
use c4rizer_core::model::CrossingStyle;
use c4rizer_core::view::EdgeId;

use super::{Action, Effect, HIT_TOLERANCE_PX, MenuAction, MenuEntry, State, round_point};
use crate::camera::ScreenPoint;

impl State {
    /// The crossing style of the open view (P4-521), `None` when it has
    /// no layout to edit.
    pub(crate) fn crossing_style(&self) -> Option<CrossingStyle> {
        self.layout_spec().map(|(_, spec)| spec.options().crossings)
    }

    /// The "Crossings" choice (P4-521): `crossings jump|plain` written, or
    /// the line removed for `gap`; nothing when the view already has that
    /// style.
    // DECISION: `gap` removes the line, as `labels line` does (P4-318):
    // the default needs no line.
    pub(super) fn set_crossing_style(&mut self, style: CrossingStyle) -> Vec<Effect> {
        let Some((view, spec)) = self.layout_spec() else { return Vec::new() };
        if spec.options().crossings == style || self.refuse_json() || self.read_only() {
            return Vec::new();
        }
        let style = (style != CrossingStyle::Gap).then_some(style);
        self.write(Action::SetCrossingStyle { view, style })
    }

    /// "Bridge on the other line" when a crossing of the shown scene lies
    /// within the hit tolerance of `at` (P4-522); none on a read-only
    /// file.
    pub(super) fn bridge_entries(&self, at: ScreenPoint) -> Vec<MenuEntry> {
        if self.read_only() {
            return Vec::new();
        }
        let Some(scene) = self.scene() else { return Vec::new() };
        let p = round_point(self.camera.to_scene(at));
        scene
            .bridge_near(p, HIT_TOLERANCE_PX)
            .map(|b| MenuEntry {
                label: "Bridge on the other line".to_owned(),
                action: MenuAction::FlipBridge {
                    carrier: b.carrier,
                    passing: b.passing,
                },
            })
            .into_iter()
            .collect()
    }

    /// "Bridge on the other line" (P4-522): the view's `bridge` lines
    /// rewritten so `passing` carries where it crosses `carrier`.
    pub(super) fn flip_bridge(&mut self, carrier: EdgeId, passing: EdgeId) -> Vec<Effect> {
        if self.refuse_json() || self.read_only() {
            return Vec::new();
        }
        let Some((_, build)) = self.shown.as_ref() else { return Vec::new() };
        let Some(site) = build.scene.bridges.iter().find(|b| b.carrier == carrier && b.passing == passing) else {
            return Vec::new();
        };
        let next = bridge::flipped(&build.plan, &build.scene, site);
        let Some(bridges) = next.iter().map(|&(c, o)| Some((self.line_ref(c)?, self.line_ref(o)?))).collect::<Option<Vec<_>>>() else {
            return Vec::new();
        };
        let Some(view) = self.view.clone() else { return Vec::new() };
        let layout = self.layout();
        self.write(Action::Bridges { view, bridges, layout })
    }
}
