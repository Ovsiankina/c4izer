//! The label drag part of the editor state: a press on a line's label
//! (on-line plate or text, or a callout's text or attach dot) dragged
//! past `CLICK_SLOP_PX` slides the label along its line; release writes
//! the view's `line … position N` through the same action as Up and Down
//! (P2-708), so it is one batch and one undo step (EDIT-19, EDIT-24).
//!
//! Depends on: `state`, `worker`, `c4rizer-core` (geom, scene, view).

// DECISION: the pointer moves the label's own point on the line (the
// on-line label's centre, the callout's attach dot) by the pointer's
// travel, and that moved point is projected onto the drawn route
// (`SceneLine::points`, every segment): the label does not jump to the
// pointer when the press was off its centre, as on a callout's text.
// DECISION: the percent is rounded to a whole number (0..100), finer than
// the 5-unit Up/Down step; `label::place_labels` places on grid spots, so
// the label moves in 10px steps along the line either way.
// DECISION: the preview is the worker's `Job::DragLabel` (the shown build
// with the dragged line's `position` overridden and the labels placed
// again by the same `label::place_labels` the build after release runs),
// asked for only when the rounded percent changes, as the anchor drag
// asks only when the snapped point changes (EDIT-25).
// DECISION: Up and Down refuse no line (bus members and lifted lines
// write their winning relationship's `line` line), so neither does the
// drag.

use c4rizer_core::edit::RelationshipRef;
use c4rizer_core::geom::Point;
use c4rizer_core::scene::{ItemKey, ModelId};
use c4rizer_core::view::EdgeId;

use super::{Action, Effect, Gesture, HANDLE_PX, LineChange, Selected, State, round_point, vertices::along};
use crate::camera::ScreenPoint;
use crate::worker::{Job, Request};

/// Where scene px `p` lies along the polyline `points`, as a whole
/// percent of its length, 0..100; `None` for a line of no length.
#[must_use]
pub(crate) fn percent_along(points: &[Point], p: Point) -> Option<u8> {
    #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
    let total: f32 = points
        .windows(2)
        .map(|w| ((w[1].x - w[0].x) as f32).hypot((w[1].y - w[0].y) as f32))
        .sum();
    if total <= 0.0 {
        return None;
    }
    let percent = (along(points, p) / total * 100.0).round().clamp(0.0, 100.0);
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to 0..100")]
    Some(percent as u8)
}

impl State {
    /// The label a press at `at` grabs: a callout's attach dot within
    /// reach (it sits on the line, which would otherwise take the press),
    /// else the label under the pointer.
    pub(super) fn label_under(&self, at: ScreenPoint) -> Option<EdgeId> {
        let scene = self.scene()?;
        let reach = HANDLE_PX + 2.0;
        let dot = scene.labels.iter().rev().find_map(|l| {
            let c = l.callout.as_ref()?;
            let s = self.camera.to_screen(c.dot);
            ((at.x - s.x).hypot(at.y - s.y) <= reach).then_some(l.edge)
        });
        dot.or_else(|| match self.hit(at)? {
            ItemKey::Label(edge) => Some(edge),
            _ => None,
        })
    }

    /// The cursor over the canvas for labels: `Some(true)` (grabbing)
    /// while a label is dragged, `Some(false)` (grab) over a label that
    /// can be dragged, `None` otherwise.
    #[must_use]
    pub(crate) fn label_cursor(&self) -> Option<bool> {
        if matches!(self.gesture, Gesture::DraggingLabel { .. }) {
            return Some(true);
        }
        if self.read_only() || !matches!(self.gesture, Gesture::Idle) {
            return None;
        }
        self.pointer.and_then(|p| self.label_under(p)).map(|_| false)
    }

    /// The label's own point on its line, scene px: the callout's attach
    /// dot, else the label's centre.
    fn label_point(&self, edge: EdgeId) -> Option<Point> {
        let label = self.scene()?.labels.iter().find(|l| l.edge == edge)?;
        Some(label.callout.as_ref().map_or_else(|| label.rect.centre(), |c| c.dot))
    }

    /// The `position` the shown build gives `edge` (`None` when unset).
    fn line_position(&self, edge: EdgeId) -> Option<u8> {
        let (_, build) = self.shown.as_ref()?;
        build.plan.edges.get(edge.0 as usize)?.style.position
    }

    /// A left drag starting on the label of `edge` (P2-708 by drag): the
    /// line is selected (its selection mark), and the label follows.
    pub(super) fn start_label_drag(&mut self, edge: EdgeId, from: ScreenPoint, at: ScreenPoint) -> Vec<Effect> {
        let Some(anchor) = self.label_point(edge) else {
            self.gesture = Gesture::Idle;
            return Vec::new();
        };
        let selected = match (self.scene(), self.scene_model()) {
            (Some(scene), Some(model)) => match scene.model_id(ItemKey::Label(edge)) {
                Some(ModelId::Relationship(r)) => Some(Selected::Relationship(RelationshipRef::of(model, r))),
                _ => None,
            },
            _ => None,
        };
        if let Some(s) = selected {
            self.selection.clear();
            self.selection.insert(s);
        }
        self.text_edit = None;
        self.hover = None;
        self.gesture = Gesture::DraggingLabel {
            edge,
            from,
            anchor,
            position: None,
        };
        self.label_drag_to(at)
    }

    /// The dragged label's position for the pointer at `at`; each change
    /// of whole percent asks the worker for a preview (EDIT-25).
    pub(super) fn label_drag_to(&mut self, at: ScreenPoint) -> Vec<Effect> {
        let Gesture::DraggingLabel {
            edge,
            from,
            anchor,
            position: before,
        } = self.gesture.clone()
        else {
            return Vec::new();
        };
        let Some(points) = self.shown.as_ref().and_then(|(_, b)| b.scene.lines.iter().find(|l| l.edge == edge)).map(|l| l.points.clone())
        else {
            return Vec::new();
        };
        let zoom = self.camera.zoom;
        #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
        let moved = round_point(crate::camera::ScenePoint {
            x: anchor.x as f32 + (at.x - from.x) / zoom,
            y: anchor.y as f32 + (at.y - from.y) / zoom,
        });
        let Some(position) = percent_along(&points, moved) else {
            return Vec::new();
        };
        self.gesture = Gesture::DraggingLabel {
            edge,
            from,
            anchor,
            position: Some(position),
        };
        if before == Some(position) && self.preview.is_some() {
            return Vec::new();
        }
        let Some((_, previous)) = self.shown.clone() else { return Vec::new() };
        let generation = self.next_generation();
        self.awaited_drag = Some(generation);
        vec![Effect::Work(Request {
            generation,
            job: Job::DragLabel {
                previous,
                edge,
                position,
            },
        })]
    }

    /// Release of a label drag: the line's `position` is written through
    /// the Up/Down action (P2-708), unless it equals the one in the file
    /// (an unset position counts as 50, as for Up/Down).
    pub(super) fn finish_label_drag(&mut self, edge: EdgeId, position: Option<u8>) -> Vec<Effect> {
        self.dragged = None;
        self.awaited_drag = None;
        let Some(position) = position else {
            self.preview = None;
            return Vec::new();
        };
        if self.line_position(edge).unwrap_or(50) == position {
            self.preview = None;
            return Vec::new();
        }
        let relationship = match (self.shown.as_ref(), self.scene_model()) {
            (Some((_, build)), Some(model)) => build
                .scene
                .lines
                .iter()
                .find(|l| l.edge == edge)
                .map(|l| RelationshipRef::of(model, l.relationship)),
            _ => None,
        };
        let (Some(relationship), Some(view)) = (relationship, self.view.clone()) else {
            self.preview = None;
            return Vec::new();
        };
        // The preview stays until the saved file's build arrives (as for
        // a move), so release shows no jump.
        let effects = self.write(Action::SetLine {
            view,
            relationship,
            change: LineChange::Position(Some(position)),
        });
        if self.pending.is_none() {
            self.preview = None;
        }
        effects
    }
}
