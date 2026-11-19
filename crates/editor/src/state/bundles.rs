//! The bus part of the editor state (SRS 4.8 P4-423..P4-427): with two or
//! more lines selected, "Group as bus" writes one `bundle` line; with a
//! bundled line selected, "Ungroup bus" removes its bundle and "Remove
//! from bus" that line only; the other members of a selected line's bus
//! are marked lightly. Every bus edit is one `edit::Edit::SetBundles`,
//! one batch, one undo step.
//!
//! Depends on: `state`, `c4rizer-core` (edit, view, scene).

// DECISION: the bundles are read from the shown build's plan
// (`ViewPlan::bundles`), which `view::plan` fills from the file's
// `bundle` lines, so the menu, the marks and the file never disagree, as
// for vertices and anchors.

use c4rizer_core::edit::LineRef;
use c4rizer_core::scene::ItemKey;
use c4rizer_core::view::EdgeId;

use super::{Action, Effect, MenuAction, MenuEntry, State, items_of};

impl State {
    /// The lines of the selection in the shown scene, ascending (P4-423).
    pub(crate) fn selected_lines(&self) -> Vec<EdgeId> {
        let (Some(scene), Some(model)) = (self.scene(), self.model()) else {
            return Vec::new();
        };
        let mut out: Vec<EdgeId> = self
            .selection
            .iter()
            .flat_map(|s| items_of(scene, model, s))
            .filter_map(|k| match k {
                ItemKey::Line(e) => Some(e),
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The bundles of the shown build, each its members.
    fn bundles(&self) -> Vec<Vec<EdgeId>> {
        self.shown.as_ref().map(|(_, b)| b.plan.bundles.clone()).unwrap_or_default()
    }

    /// The members of the bus `edge` belongs to, when it is bundled.
    pub(crate) fn bus_of(&self, edge: EdgeId) -> Option<Vec<EdgeId>> {
        self.bundles().into_iter().find(|b| b.contains(&edge))
    }

    /// The bus actions on offer for the selection, in menu order:
    /// "Group as bus" with two lines or more selected (P4-424), "Ungroup
    /// bus" and "Remove from bus" with one bundled line selected
    /// (P4-425); none on a read-only file.
    pub(crate) fn bus_entries(&self) -> Vec<MenuEntry> {
        if self.read_only() {
            return Vec::new();
        }
        let lines = self.selected_lines();
        let mut out = Vec::new();
        if lines.len() >= 2 && self.bus_of(lines[0]).is_none_or(|b| b != lines) {
            out.push(MenuEntry {
                label: "Group as bus".to_owned(),
                action: MenuAction::GroupBus,
            });
        }
        if let [edge] = lines.as_slice()
            && self.bus_of(*edge).is_some()
        {
            out.push(MenuEntry {
                label: "Ungroup bus".to_owned(),
                action: MenuAction::UngroupBus { edge: *edge },
            });
            out.push(MenuEntry {
                label: "Remove from bus".to_owned(),
                action: MenuAction::RemoveFromBus { edge: *edge },
            });
        }
        out
    }

    /// The other members of the buses of the selected lines, not
    /// selected themselves (P4-426).
    pub(crate) fn bus_peers(&self) -> Vec<EdgeId> {
        let lines = self.selected_lines();
        let mut out: Vec<EdgeId> = lines
            .iter()
            .filter_map(|&e| self.bus_of(e))
            .flatten()
            .filter(|e| !lines.contains(e))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// "Group as bus" (P4-424): the selected lines become one bundle, in
    /// `EdgeId` order, taken out of any bundle they were in.
    pub(super) fn group_bus(&mut self) -> Vec<Effect> {
        let lines = self.selected_lines();
        if lines.len() < 2 {
            return Vec::new();
        }
        let mut next: Vec<Vec<EdgeId>> = self
            .bundles()
            .into_iter()
            .map(|b| b.into_iter().filter(|e| !lines.contains(e)).collect::<Vec<_>>())
            .filter(|b| b.len() >= 2)
            .collect();
        next.push(lines);
        self.write_bundles(&next)
    }

    /// "Ungroup bus" (P4-425): the bundle of `edge` goes.
    pub(super) fn ungroup_bus(&mut self, edge: EdgeId) -> Vec<Effect> {
        let next: Vec<Vec<EdgeId>> = self.bundles().into_iter().filter(|b| !b.contains(&edge)).collect();
        self.write_bundles(&next)
    }

    /// "Remove from bus" (P4-425): `edge` leaves its bundle; a bundle left
    /// with one member goes.
    pub(super) fn remove_from_bus(&mut self, edge: EdgeId) -> Vec<Effect> {
        let next: Vec<Vec<EdgeId>> = self
            .bundles()
            .into_iter()
            .map(|b| b.into_iter().filter(|&e| e != edge).collect::<Vec<_>>())
            .filter(|b| b.len() >= 2)
            .collect();
        self.write_bundles(&next)
    }

    /// Writes the view's bundles as `next`, one batch, one undo step
    /// (P4-427); nothing when they are unchanged.
    fn write_bundles(&mut self, next: &[Vec<EdgeId>]) -> Vec<Effect> {
        if self.refuse_json() || self.read_only() || next == self.bundles().as_slice() {
            return Vec::new();
        }
        let Some(bundles) = next.iter().map(|b| b.iter().map(|&e| self.line_ref(e)).collect::<Option<Vec<_>>>()).collect::<Option<Vec<_>>>() else {
            return Vec::new();
        };
        let Some(view) = self.view.clone() else { return Vec::new() };
        let layout = self.layout();
        self.write(Action::Bundles { view, bundles, layout })
    }

    /// `edge` of the shown build named by its ends' keys and step number.
    fn line_ref(&self, edge: EdgeId) -> Option<LineRef> {
        let (_, build) = self.shown.as_ref()?;
        let model = self.scene_model()?;
        let e = build.plan.edges.get(edge.0 as usize)?;
        let key = |b: c4rizer_core::view::BoxId| -> Option<String> {
            let element = build.plan.boxes.get(b.0 as usize)?.element;
            model.full_key(element)
        };
        Some(LineRef::new(key(e.source)?, key(e.destination)?, e.order.clone()))
    }
}
