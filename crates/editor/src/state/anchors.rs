//! The anchors part of the editor state: a handle at each end of the
//! selected line, dragging one shows its box's snap points and snaps to
//! the nearest, release pins the end there or, away from every point,
//! gives it back to the router; "Release start" / "Release end" in the
//! line's context menu (SRS 4.8 P4-213..P4-218). Every anchor edit writes
//! one `anchor` line, one batch, one undo step.
//!
//! Depends on: `state`, `worker`, `c4rizer-core` (anchor, geom, view).

// DECISION: a line's anchors are read from the shown build's plan
// (`PlanEdge::source_anchor`, `destination_anchor`), which `view::plan`
// fills from the file's `anchor` lines, so the canvas, the handles and
// the file never disagree, as for vertices.
// DECISION: the preview is asked for only when the snapped point changes,
// not on every pointer move: one re-route per snap point crossed keeps
// the drag at interactive speed (EDIT-25) whatever the pointer does.

use c4rizer_core::anchor::{Anchor, snap_points};
use c4rizer_core::geom::{GRID_PX, Point};
use c4rizer_core::view::EdgeId;

use super::{Action, AnchorChange, Effect, Gesture, HANDLE_PX, MenuAction, MenuEntry, State};
use crate::camera::ScreenPoint;
use crate::worker::{Job, Request};

/// How near a snap point the pointer must come for the dragged end to
/// snap to it, screen px (P4-214).
pub const SNAP_RADIUS_PX: f32 = 20.0;

/// One snap point shown during an end drag: where it is on screen and
/// whether the dragged end sits on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapMark {
    /// Screen position.
    pub at: ScreenPoint,
    /// True for the point the handle snapped to.
    pub snapped: bool,
}

impl State {
    /// The screen position of the two end handles of the selected line,
    /// source end first; empty when no single line is selected or the
    /// file is read-only (P4-213). During an end drag the dragged handle
    /// sits on its snapped point, else under the pointer.
    pub(crate) fn end_handles(&self) -> Vec<ScreenPoint> {
        if self.read_only() {
            return Vec::new();
        }
        let mut spots = self.end_spots();
        if let Gesture::DraggingEnd {
            destination, snapped, at, ..
        } = &self.gesture
            && spots.len() == 2
        {
            let marks = self.snap_marks();
            let on = snapped.and_then(|_| marks.iter().find(|m| m.snapped)).map_or(*at, |m| m.at);
            spots[usize::from(*destination)] = on;
        }
        spots
    }

    /// The selected line's two ends on screen, drawn or not.
    fn end_spots(&self) -> Vec<ScreenPoint> {
        let Some(edge) = self.selected_edge() else { return Vec::new() };
        let Some(line) = self.scene().and_then(|s| s.lines.iter().find(|l| l.edge == edge)) else {
            return Vec::new();
        };
        match (line.points.first(), line.points.last()) {
            (Some(&a), Some(&b)) if line.points.len() >= 2 => vec![self.camera.to_screen(a), self.camera.to_screen(b)],
            _ => Vec::new(),
        }
    }

    /// The end handle under `at`: the selected line and whether it is the
    /// destination end.
    pub(super) fn end_under(&self, at: ScreenPoint) -> Option<(EdgeId, bool)> {
        let edge = self.selected_edge()?;
        let reach = HANDLE_PX + 2.0;
        // DECISION: the destination handle is tried first, as the arrow
        // end is the one users grab most and both may touch on a short line.
        self.end_spots()
            .iter()
            .enumerate()
            .rev()
            .find(|(_, h)| (at.x - h.x).abs() <= reach && (at.y - h.y).abs() <= reach)
            .map(|(i, _)| (edge, i == 1))
    }

    /// The snap points of the box the dragged end belongs to, on screen
    /// (P4-214); empty outside an end drag.
    pub(crate) fn snap_marks(&self) -> Vec<SnapMark> {
        let Gesture::DraggingEnd {
            edge,
            destination,
            snapped,
            ..
        } = &self.gesture
        else {
            return Vec::new();
        };
        self.snaps_of(*edge, *destination)
            .into_iter()
            .map(|(a, p)| SnapMark {
                at: self.camera.to_screen(Point::new(p.x * GRID_PX, p.y * GRID_PX)),
                snapped: *snapped == Some(a),
            })
            .collect()
    }

    /// The snap points (grid units) of the box at the `destination` end
    /// (else the source end) of `edge`, from the shown build.
    fn snaps_of(&self, edge: EdgeId, destination: bool) -> Vec<(Anchor, Point)> {
        let Some((_, build)) = &self.shown else { return Vec::new() };
        let Some(e) = build.plan.edges.get(edge.0 as usize) else { return Vec::new() };
        let b = if destination { e.destination } else { e.source };
        let (Some(pb), Some(&centre)) = (build.plan.boxes.get(b.0 as usize), build.routes.centres.get(b.0 as usize)) else {
            return Vec::new();
        };
        snap_points(pb.rect_at(centre))
    }

    /// The anchors of `edge` (source, destination) in the shown build.
    fn line_anchors(&self, edge: EdgeId) -> (Option<Anchor>, Option<Anchor>) {
        self.shown
            .as_ref()
            .and_then(|(_, b)| b.plan.edges.get(edge.0 as usize))
            .map_or((None, None), |e| (e.source_anchor, e.destination_anchor))
    }

    /// A left drag starting on an end handle (P4-214).
    pub(super) fn start_end_drag(&mut self, edge: EdgeId, destination: bool, at: ScreenPoint) -> Vec<Effect> {
        self.text_edit = None;
        let (s, d) = self.line_anchors(edge);
        self.gesture = Gesture::DraggingEnd {
            edge,
            destination,
            snapped: if destination { d } else { s },
            at,
        };
        self.end_drag_to(at)
    }

    /// The dragged end follows the pointer and snaps to the nearest snap
    /// point within `SNAP_RADIUS_PX`; each change of snapped point asks
    /// the worker for a preview (P4-214, EDIT-25).
    pub(super) fn end_drag_to(&mut self, at: ScreenPoint) -> Vec<Effect> {
        let Gesture::DraggingEnd {
            edge,
            destination,
            snapped: before,
            ..
        } = self.gesture.clone()
        else {
            return Vec::new();
        };
        let near = self
            .snaps_of(edge, destination)
            .into_iter()
            .map(|(a, p)| {
                let s = self.camera.to_screen(Point::new(p.x * GRID_PX, p.y * GRID_PX));
                (a, (s.x - at.x).hypot(s.y - at.y))
            })
            .filter(|(_, d)| *d <= SNAP_RADIUS_PX)
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(a, _)| a);
        self.gesture = Gesture::DraggingEnd {
            edge,
            destination,
            snapped: near,
            at,
        };
        if near == before && self.preview.is_some() {
            return Vec::new();
        }
        let (s, d) = self.with_end(edge, destination, near);
        let Some((parsed, previous)) = self.shown.clone() else { return Vec::new() };
        let generation = self.next_generation();
        self.awaited_drag = Some(generation);
        vec![Effect::Work(Request {
            generation,
            job: Job::DragAnchor {
                parsed,
                previous,
                edge,
                source: s,
                destination: d,
            },
        })]
    }

    /// The anchors of `edge` with the `destination` (else source) end set
    /// to `to`.
    fn with_end(&self, edge: EdgeId, destination: bool, to: Option<Anchor>) -> (Option<Anchor>, Option<Anchor>) {
        let (s, d) = self.line_anchors(edge);
        if destination { (s, to) } else { (to, d) }
    }

    /// Release of an end drag: the end is pinned to its snapped point, or
    /// given back to the router away from every point; the line's
    /// `anchor` line is written when that changed something (P4-215,
    /// P4-216).
    pub(super) fn finish_end_drag(&mut self, edge: EdgeId, destination: bool, snapped: Option<Anchor>) -> Vec<Effect> {
        self.dragged = None;
        self.awaited_drag = None;
        let (s, d) = self.with_end(edge, destination, snapped);
        if (s, d) == self.line_anchors(edge) {
            self.preview = None;
            return Vec::new();
        }
        // The preview stays until the saved file's build arrives.
        self.write_anchor(edge, s, d)
    }

    /// "Release start" / "Release end" (P4-217): the pin of that end goes.
    pub(super) fn release_anchor(&mut self, edge: EdgeId, destination: bool) -> Vec<Effect> {
        if self.refuse_json() || self.read_only() {
            return Vec::new();
        }
        let (s, d) = self.with_end(edge, destination, None);
        if (s, d) == self.line_anchors(edge) {
            return Vec::new();
        }
        self.write_anchor(edge, s, d)
    }

    /// The context menu entries of `edge`: one per pinned end (P4-217);
    /// none on a read-only file.
    pub(super) fn anchor_entries(&self, edge: EdgeId) -> Vec<MenuEntry> {
        if self.read_only() {
            return Vec::new();
        }
        let (s, d) = self.line_anchors(edge);
        let mut out = Vec::new();
        if s.is_some() {
            out.push(MenuEntry {
                label: "Release start".to_owned(),
                action: MenuAction::ReleaseAnchor { edge, destination: false },
            });
        }
        if d.is_some() {
            out.push(MenuEntry {
                label: "Release end".to_owned(),
                action: MenuAction::ReleaseAnchor { edge, destination: true },
            });
        }
        out
    }

    /// Writes the `anchor` line of `edge` with ends `from` and `to` (both
    /// `None` removes it), one batch, one undo step (P4-218).
    fn write_anchor(&mut self, edge: EdgeId, from: Option<Anchor>, to: Option<Anchor>) -> Vec<Effect> {
        let Some(change) = self.anchor_change(edge, from, to) else {
            self.preview = None;
            return Vec::new();
        };
        let Some(view) = self.view.clone() else { return Vec::new() };
        let layout = self.layout();
        self.write(Action::Anchor { view, change, layout })
    }

    /// The `AnchorChange` giving `edge` the ends `from` and `to`. `None`
    /// when an end has no key.
    fn anchor_change(&self, edge: EdgeId, from: Option<Anchor>, to: Option<Anchor>) -> Option<AnchorChange> {
        let (_, build) = self.shown.as_ref()?;
        let model = self.scene_model()?;
        let e = build.plan.edges.get(edge.0 as usize)?;
        let key = |b: c4rizer_core::view::BoxId| -> Option<String> {
            let element = build.plan.boxes.get(b.0 as usize)?.element;
            model.element(element).key.as_ref().map(|k| k.value.clone())
        };
        Some(AnchorChange {
            source: key(e.source)?,
            destination: key(e.destination)?,
            order: e.order.clone(),
            from,
            to,
        })
    }
}
