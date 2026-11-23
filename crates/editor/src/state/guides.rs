//! Alignment guides for a box drag (SRS 4.9 align-guides, P5-500..):
//! an index of the left, centre and right (x) and top, centre and bottom
//! (y) lines of every box not being dragged, built once when the drag
//! starts; per pointer move, the pull of the dragged boxes' bounding
//! rect onto a line within tolerance, kept only when it leaves every
//! centre on the 10px grid, and the guide segments to draw. Pure
//! functions on scene geometry: no egui, no allocation per query beyond
//! the caller's reused guide list.
//!
//! Depends on: `c4rizer-core` (geom, scene, view).

// DECISION: lines are kept in doubled scene px (`2 * px`): a bounding
// rect of several boxes can have its centre on a 5px step, and doubling
// keeps every comparison exact in integers.

use std::collections::BTreeMap;

use c4rizer_core::geom::{GRID_PX, Point, Rect};
use c4rizer_core::scene::Scene;
use c4rizer_core::view::BoxId;

/// How near a line the dragged rect must come to be pulled onto it,
/// screen px (P5-501); divided by the zoom for scene px.
pub const GUIDE_TOLERANCE_PX: f32 = 6.0;

/// How far a guide runs past the outermost box at each end, scene px
/// (P5-503).
pub const GUIDE_OVERHANG_PX: f32 = 12.0;

/// One drawn guide, scene px: a vertical segment at `x = at` from
/// `y = from` to `y = to`, or a horizontal one at `y = at` from `x =
/// from` to `x = to`. Serves P5-503.
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-18, #8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    /// True for a vertical segment (an x line).
    pub vertical: bool,
    /// The line, scene px.
    pub at: f32,
    /// Where the segment starts along it, scene px.
    pub from: f32,
    /// Where it ends, scene px.
    pub to: f32,
}

/// One indexed line: its position (doubled px), the box's extent on the
/// other axis (px), and the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Line {
    at2: i32,
    lo: i32,
    hi: i32,
    of: BoxId,
}

/// The lines of every box not being dragged, sorted, and the dragged
/// boxes' bounding rect at the drag's start (P5-500). Built once per
/// drag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideIndex {
    /// x lines: left, centre, right of each box, by position.
    xs: Vec<Line>,
    /// y lines: top, centre, bottom of each box, by position.
    ys: Vec<Line>,
    /// The bounding rect of the dragged boxes at their start centres, px.
    dragged: Rect,
}

impl GuideIndex {
    /// The index of `scene`'s boxes, `dragged` (box to start centre,
    /// grid units) left out; `None` when no dragged box is in `scene`.
    #[must_use]
    pub fn build(scene: &Scene, dragged: &BTreeMap<BoxId, Point>) -> Option<GuideIndex> {
        let mut bound: Option<Rect> = None;
        let mut xs = Vec::with_capacity(3 * scene.boxes.len());
        let mut ys = Vec::with_capacity(3 * scene.boxes.len());
        for b in &scene.boxes {
            let r = b.rect;
            if let Some(start) = dragged.get(&b.id) {
                // The rect at the drag's start centre (the scene may show
                // the box elsewhere, e.g. the file's position).
                let now = scene.centres.get(b.id.0 as usize).copied().unwrap_or(*start);
                let r = shifted(&r, start.x - now.x, start.y - now.y);
                bound = Some(bound.map_or(r, |u| union(&u, &r)));
                continue;
            }
            for at2 in [2 * r.min.x, r.min.x + r.max.x, 2 * r.max.x] {
                xs.push(Line {
                    at2,
                    lo: r.min.y,
                    hi: r.max.y,
                    of: b.id,
                });
            }
            for at2 in [2 * r.min.y, r.min.y + r.max.y, 2 * r.max.y] {
                ys.push(Line {
                    at2,
                    lo: r.min.x,
                    hi: r.max.x,
                    of: b.id,
                });
            }
        }
        xs.sort_unstable_by_key(|l| (l.at2, l.of));
        ys.sort_unstable_by_key(|l| (l.at2, l.of));
        Some(GuideIndex {
            xs,
            ys,
            dragged: bound?,
        })
    }

    /// The pull, grid units per axis, that moves the dragged rect, already
    /// moved by `delta` (grid units), onto the nearest index line within
    /// `tolerance` scene px of one of its three lines, among the pulls
    /// that are whole grid steps (P5-501, P5-502); `(0, 0)` on an axis
    /// with none, and on both when `pull` is false (P5-505). No
    /// allocation.
    #[must_use]
    pub fn snap(&self, delta: (i32, i32), tolerance: f32, pull: bool) -> (i32, i32) {
        if !pull {
            return (0, 0);
        }
        #[expect(clippy::cast_possible_truncation, reason = "a few screen px over the zoom")]
        let tol2 = (2.0 * tolerance).floor() as i32;
        let r = shifted(&self.dragged, delta.0, delta.1);
        let x = nearest(&self.xs, [2 * r.min.x, r.min.x + r.max.x, 2 * r.max.x], tol2);
        let y = nearest(&self.ys, [2 * r.min.y, r.min.y + r.max.y, 2 * r.max.y], tol2);
        (x, y)
    }

    /// The guides of the dragged rect moved by `delta` (grid units): per
    /// axis, one segment per line of the rect that equals an index line,
    /// spanning the rect and every box on that line, plus
    /// `GUIDE_OVERHANG_PX` at each end (P5-503). Clears `out` first;
    /// allocates only when `out` grows.
    pub fn guides(&self, delta: (i32, i32), out: &mut Vec<Guide>) {
        out.clear();
        let r = shifted(&self.dragged, delta.0, delta.1);
        let axes = [
            (true, &self.xs, [2 * r.min.x, r.min.x + r.max.x, 2 * r.max.x], (r.min.y, r.max.y)),
            (false, &self.ys, [2 * r.min.y, r.min.y + r.max.y, 2 * r.max.y], (r.min.x, r.max.x)),
        ];
        for (vertical, lines, mine, (lo, hi)) in axes {
            for at2 in mine {
                let start = lines.partition_point(|l| l.at2 < at2);
                let on = lines[start..].iter().take_while(|l| l.at2 == at2);
                let (mut a, mut b, mut any) = (lo, hi, false);
                for l in on {
                    (a, b, any) = (a.min(l.lo), b.max(l.hi), true);
                }
                if any {
                    #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
                    let (at, a, b) = (at2 as f32 / 2.0, a as f32, b as f32);
                    out.push(Guide {
                        vertical,
                        at,
                        from: a - GUIDE_OVERHANG_PX,
                        to: b + GUIDE_OVERHANG_PX,
                    });
                }
            }
        }
    }
}

// DECISION: among pulls of equal size the dragged rect's first line wins
// (left before centre before right, top before centre before bottom): a
// fixed order keeps a drag deterministic.
/// The pull, grid units, onto the nearest line of `lines` within `tol2`
/// (doubled px) of one of `mine`, among the pulls that are whole grid
/// steps; ties go to the smaller pull, then to the first of `mine`; `0`
/// when none.
fn nearest(lines: &[Line], mine: [i32; 3], tol2: i32) -> i32 {
    let step2 = 2 * GRID_PX;
    let mut best: Option<i32> = None;
    for v in mine {
        let start = lines.partition_point(|l| l.at2 < v - tol2);
        let mut last = None;
        for l in lines[start..].iter().take_while(|l| l.at2 <= v + tol2) {
            if last == Some(l.at2) {
                continue;
            }
            last = Some(l.at2);
            let d = l.at2 - v;
            if d.rem_euclid(step2) == 0 && best.is_none_or(|b| d.abs() < b.abs()) {
                best = Some(d);
            }
        }
    }
    best.map_or(0, |d| d / step2)
}

/// `r` moved by `(dx, dy)` grid units.
fn shifted(r: &Rect, dx: i32, dy: i32) -> Rect {
    let (dx, dy) = (dx * GRID_PX, dy * GRID_PX);
    Rect {
        min: Point::new(r.min.x + dx, r.min.y + dy),
        max: Point::new(r.max.x + dx, r.max.y + dy),
    }
}

impl super::State {
    /// After the grid snap of a box drag (P5-501, P5-502, P5-505): pulls
    /// `next` (the dragged centres, grid units) onto a guide of the index
    /// when one is in reach and pulling is on, and refills the guides at
    /// the final position (P5-503). Allocation-free apart from growing
    /// the reused guide list.
    pub(super) fn pull_to_guides(&mut self, boxes: &BTreeMap<BoxId, Point>, next: &mut BTreeMap<BoxId, Point>) {
        let Some(index) = &self.guide_index else {
            self.guides.clear();
            return;
        };
        // Every dragged centre moves by the same snapped delta.
        let Some((at, start)) = boxes.iter().next().and_then(|(id, s)| Some((next.get(id)?, s))) else {
            return;
        };
        let delta = (at.x - start.x, at.y - start.y);
        let pull = index.snap(delta, GUIDE_TOLERANCE_PX / self.camera.zoom, !self.ui.snap_guides_off);
        if pull != (0, 0) {
            for c in next.values_mut() {
                *c = Point::new(c.x + pull.0, c.y + pull.1);
            }
        }
        index.guides((delta.0 + pull.0, delta.1 + pull.1), &mut self.guides);
    }

    /// Drops the drag's guides and index (P5-506: pointer up, cancelled
    /// drag).
    pub(super) fn drop_guides(&mut self) {
        self.guide_index = None;
        self.guides.clear();
    }

    /// Test hook for the P5-900 "without guides" benchmark: the drag under
    /// way goes on with no guide index.
    #[cfg(test)]
    pub(crate) fn drop_guide_index(&mut self) {
        self.guide_index = None;
    }
}

/// The smallest rect holding `a` and `b`.
fn union(a: &Rect, b: &Rect) -> Rect {
    Rect {
        min: Point::new(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
        max: Point::new(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
    }
}
