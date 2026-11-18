//! The vertices part of the editor state: handles on the selected line,
//! click to add, drag to move, double click to remove, "Remove all
//! vertices", and the vertices a multi-select move carries along (SRS 4.7
//! P3-310..P3-315). Every vertex edit writes `via` lines only, one batch,
//! one undo step.
//!
//! Depends on: `state`, `worker`, `c4rizer-core` (geom, scene, view).

// DECISION: a line's vertices are read from the shown build's plan
// (`PlanEdge::vertices`), which `view::plan` fills from the file's `via`
// lines, so the canvas, the handles and the file never disagree; during
// a handle drag the gesture's own list is shown instead.
// DECISION: handles are drawn and hit only while the file is editable,
// like the resize handles (EDIT-11).

use std::collections::BTreeMap;

use c4rizer_core::geom::{GRID_PX, Point};
use c4rizer_core::scene::ItemKey;
use c4rizer_core::view::{BoxId, EdgeId};

use super::{Action, Effect, Gesture, HANDLE_PX, Modifiers, State, ViaChange, items_of, snap_centre};
use crate::camera::ScreenPoint;
use crate::worker::{Job, Request};

/// Where scene px `p` lies along the polyline `points`: the length from
/// the first point to `p`'s nearest point on it.
#[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
fn along(points: &[Point], p: Point) -> f32 {
    let mut best = (f32::INFINITY, 0.0f32);
    let mut start = 0.0f32;
    for w in points.windows(2) {
        let (ax, ay) = (w[0].x as f32, w[0].y as f32);
        let (dx, dy) = (w[1].x as f32 - ax, w[1].y as f32 - ay);
        let len2 = dx * dx + dy * dy;
        let len = len2.sqrt();
        let t = if len2 > 0.0 {
            (((p.x as f32 - ax) * dx + (p.y as f32 - ay) * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (qx, qy) = (ax + t * dx - p.x as f32, ay + t * dy - p.y as f32);
        let d2 = qx * qx + qy * qy;
        if d2 < best.0 {
            best = (d2, start + t * len);
        }
        start += len;
    }
    best.1
}

impl State {
    /// The screen position of every vertex handle of the selected line,
    /// in vertex order; empty when no single line is selected or the file
    /// is read-only (P3-311). The canvas paints them.
    pub(crate) fn vertex_handles(&self) -> Vec<ScreenPoint> {
        if self.read_only() {
            return Vec::new();
        }
        self.vertex_spots()
    }

    /// Where the selected line's vertices are on screen, drawn or not: a
    /// read-only workspace JSON still answers a press on one with its
    /// refusal (P3-420).
    fn vertex_spots(&self) -> Vec<ScreenPoint> {
        let Some(edge) = self.selected_edge() else {
            return Vec::new();
        };
        self.line_vertices(edge)
            .iter()
            .map(|v| self.camera.to_screen(Point::new(v.x * GRID_PX, v.y * GRID_PX)))
            .collect()
    }

    /// The line being edited: the one a handle drag holds, else the only
    /// line of a selection of one line.
    fn selected_edge(&self) -> Option<EdgeId> {
        if let Gesture::DraggingVertex { edge, .. } = &self.gesture {
            return Some(*edge);
        }
        if self.selection.len() != 1 {
            return None;
        }
        let (scene, model) = (self.scene()?, self.model()?);
        let lines: Vec<EdgeId> = self
            .selection
            .iter()
            .flat_map(|s| items_of(scene, model, s))
            .filter_map(|k| match k {
                ItemKey::Line(e) => Some(e),
                _ => None,
            })
            .collect();
        match lines.as_slice() {
            [e] => Some(*e),
            _ => None,
        }
    }

    /// The vertices of `edge`, grid units: the dragged list during a
    /// handle drag of it, else the shown build's.
    fn line_vertices(&self, edge: EdgeId) -> Vec<Point> {
        if let Gesture::DraggingVertex { edge: e, vertices, .. } = &self.gesture
            && *e == edge
        {
            return vertices.clone();
        }
        self.shown
            .as_ref()
            .and_then(|(_, b)| b.plan.edges.get(edge.0 as usize))
            .map(|e| e.vertices.clone())
            .unwrap_or_default()
    }

    /// The handle under `at`: the selected line and the vertex index.
    pub(super) fn vertex_under(&self, at: ScreenPoint) -> Option<(EdgeId, usize)> {
        let edge = self.selected_edge()?;
        self.vertex_spots()
            .iter()
            .position(|h| (at.x - h.x).abs() <= HANDLE_PX && (at.y - h.y).abs() <= HANDLE_PX)
            .map(|i| (edge, i))
    }

    /// A left drag starting on a handle (P3-311).
    pub(super) fn start_vertex_drag(&mut self, edge: EdgeId, index: usize, at: ScreenPoint) -> Vec<Effect> {
        self.text_edit = None;
        self.gesture = Gesture::DraggingVertex {
            edge,
            index,
            vertices: self.line_vertices(edge),
        };
        self.vertex_drag_to(at)
    }

    /// The dragged vertex follows the pointer on the 10 px grid; the legs
    /// are re-routed by the worker (P3-311, EDIT-25).
    pub(super) fn vertex_drag_to(&mut self, at: ScreenPoint) -> Vec<Effect> {
        let Gesture::DraggingVertex {
            edge,
            index,
            mut vertices,
        } = self.gesture.clone()
        else {
            return Vec::new();
        };
        let p = snap_centre(self.camera.to_scene(at));
        if vertices.get(index).is_none_or(|v| *v == p) {
            return Vec::new();
        }
        vertices[index] = p;
        self.gesture = Gesture::DraggingVertex {
            edge,
            index,
            vertices: vertices.clone(),
        };
        let Some((parsed, previous)) = self.shown.clone() else {
            return Vec::new();
        };
        let generation = self.next_generation();
        self.awaited_drag = Some(generation);
        vec![Effect::Work(Request {
            generation,
            job: Job::DragVertex {
                parsed,
                previous,
                edge,
                vertices,
            },
        })]
    }

    /// Release of a handle drag: the line's `via` line is rewritten, when
    /// the vertex moved (P3-311).
    pub(super) fn finish_vertex_drag(&mut self, edge: EdgeId, vertices: &[Point]) -> Vec<Effect> {
        self.dragged = None;
        self.awaited_drag = None;
        if self.line_vertices(edge) == vertices {
            self.preview = None;
            return Vec::new();
        }
        // The preview stays until the saved file's build arrives.
        self.write_vias(self.via_change(edge, vertices).into_iter().collect(), BTreeMap::new())
    }

    /// A plain left click on the selected line, off its handles, adds a
    /// vertex at the nearest grid point, in order along the line (P3-310).
    /// `None` when the click is not that.
    pub(super) fn vertex_click(&mut self, at: ScreenPoint, mods: Modifiers, hit: Option<ItemKey>) -> Option<Vec<Effect>> {
        if mods.shift || mods.ctrl || mods.alt {
            return None;
        }
        let Some(ItemKey::Line(edge)) = hit else { return None };
        if self.selected_edge() != Some(edge) || self.vertex_under(at).is_some() {
            return None;
        }
        // P3-420: on a workspace JSON the add is refused with its message.
        if self.refuse_json() {
            return Some(Vec::new());
        }
        if self.read_only() {
            return None;
        }
        let p = snap_centre(self.camera.to_scene(at));
        let mut vertices = self.line_vertices(edge);
        if vertices.contains(&p) {
            return Some(Vec::new());
        }
        let points = self.scene()?.lines.iter().find(|l| l.edge == edge)?.points.clone();
        let px = |v: Point| Point::new(v.x * GRID_PX, v.y * GRID_PX);
        let here = along(&points, px(p));
        // DECISION: the new vertex goes after every vertex lying before
        // the click along the drawn line, so a click between two vertices
        // inserts between them (jointjs `addVertex`).
        let index = vertices.iter().filter(|&&v| along(&points, px(v)) <= here).count();
        vertices.insert(index, p);
        Some(self.write_vias(self.via_change(edge, &vertices).into_iter().collect(), BTreeMap::new()))
    }

    /// A double click on a handle removes that vertex; the last one
    /// removes the `via` line (P3-312).
    pub(super) fn remove_vertex(&mut self, edge: EdgeId, index: usize) -> Vec<Effect> {
        let mut vertices = self.line_vertices(edge);
        if index >= vertices.len() {
            return Vec::new();
        }
        vertices.remove(index);
        self.write_vias(self.via_change(edge, &vertices).into_iter().collect(), BTreeMap::new())
    }

    /// "Remove all vertices": every `via` line of the open view, one
    /// step (P3-313).
    pub(super) fn remove_all_vertices(&mut self) -> Vec<Effect> {
        // DECISION: refused up front on a workspace JSON (P3-420), so the
        // notice shows although the view has no vertices to remove.
        if self.refuse_json() {
            return Vec::new();
        }
        let Some((_, build)) = &self.shown else { return Vec::new() };
        let edges: Vec<EdgeId> = build.plan.edges.iter().filter(|e| !e.vertices.is_empty()).map(|e| e.id).collect();
        let vias: Vec<ViaChange> = edges.into_iter().filter_map(|e| self.via_change(e, &[])).collect();
        self.write_vias(vias, BTreeMap::new())
    }

    /// The vertices a box drag from `from` to `to` (grid centres) carries:
    /// those of each line whose two ends both moved by the same delta
    /// (P3-314, ruling V2). A line with one end moved keeps its vertices,
    /// as Structurizr does.
    pub(super) fn carried_vias(&self, from: &BTreeMap<BoxId, Point>, to: &BTreeMap<BoxId, Point>) -> Vec<ViaChange> {
        let Some((_, build)) = &self.shown else { return Vec::new() };
        let delta = |b: BoxId| -> Option<(i32, i32)> {
            let (f, t) = (from.get(&b)?, to.get(&b)?);
            Some((t.x - f.x, t.y - f.y))
        };
        build
            .plan
            .edges
            .iter()
            .filter(|e| !e.vertices.is_empty())
            .filter_map(|e| {
                let d = delta(e.source)?;
                if delta(e.destination)? != d || d == (0, 0) {
                    return None;
                }
                let moved: Vec<Point> = e.vertices.iter().map(|v| Point::new(v.x + d.0, v.y + d.1)).collect();
                self.via_change(e.id, &moved)
            })
            .collect()
    }

    /// The `ViaChange` giving `edge` the vertices `vertices` (grid units;
    /// empty removes its `via` line). `None` when an end has no key.
    fn via_change(&self, edge: EdgeId, vertices: &[Point]) -> Option<ViaChange> {
        let (_, build) = self.shown.as_ref()?;
        let model = self.scene_model()?;
        let e = build.plan.edges.get(edge.0 as usize)?;
        let key = |b: BoxId| -> Option<String> {
            let element = build.plan.boxes.get(b.0 as usize)?.element;
            model.element(element).key.as_ref().map(|k| k.value.clone())
        };
        Some(ViaChange {
            source: key(e.source)?,
            destination: key(e.destination)?,
            order: e.order.clone(),
            points: (!vertices.is_empty()).then(|| vertices.iter().map(|v| (v.x * GRID_PX, v.y * GRID_PX)).collect()),
        })
    }

    /// Writes `vias` with the boxes `boxes` moved (canonical key to centre
    /// px), one batch, one undo step (P3-315). Nothing to write, nothing
    /// written.
    pub(super) fn write_vias(&mut self, vias: Vec<ViaChange>, boxes: BTreeMap<String, (i32, i32)>) -> Vec<Effect> {
        if vias.is_empty() && boxes.is_empty() {
            self.preview = None;
            return Vec::new();
        }
        let Some(view) = self.view.clone() else { return Vec::new() };
        let mut layout = self.layout();
        for (k, c) in &boxes {
            layout.insert(k.clone(), *c);
        }
        self.write(Action::Vertices {
            view,
            boxes,
            vias,
            layout,
        })
    }
}
