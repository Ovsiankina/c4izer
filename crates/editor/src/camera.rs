//! Screen and scene points and the camera between them (pan and zoom):
//! the view of the diagram, never the diagram (EDIT-9). Shared by
//! `state` (input, drag), `canvas` (painting) and `files` (remembered per
//! file, EDIT-13).
//!
//! Depends on: `c4rizer-core` (geom).

// DECISION: split out of `state` (review of the step-4 draft) so `files`
// can store a `Camera` without depending on `state`; no behaviour moves.
// DECISION: two point types with the same layout, `ScreenPoint` and
// `ScenePoint`, so the unit is in the type: screen px are pointer and
// paint coordinates, scene px are the scene's i32 px unrounded.

use c4rizer_core::geom::{Point, Rect};

/// Smallest zoom factor (EDIT-9).
pub const MIN_ZOOM: f32 = 0.1;

/// Largest zoom factor (EDIT-9).
pub const MAX_ZOOM: f32 = 8.0;

/// Zoom factor per wheel notch (EDIT-9).
pub const ZOOM_STEP: f32 = 1.1;

/// Margin kept around the scene by "Fit view", screen px (EDIT-30).
const FIT_MARGIN_PX: f32 = 20.0;

/// A point in screen px, origin at the canvas's top-left corner, y down.
/// Every pointer event carries one (EDIT-28).
// DECISION: hot (one per pointer event, EDIT-25): repr(C), 8 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScreenPoint {
    /// Horizontal, px.
    pub x: f32,
    /// Vertical, px, growing downwards.
    pub y: f32,
}

/// A point in scene px (the scene's coordinates, `scene` module header),
/// unrounded: a pointer position mapped through the camera, or the pan
/// (EDIT-9, EDIT-28).
// DECISION: hot (one per pointer event while dragging): repr(C), 8 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScenePoint {
    /// Horizontal, scene px.
    pub x: f32,
    /// Vertical, scene px, growing downwards.
    pub y: f32,
}

/// Pan and zoom of the canvas: the view of the diagram, never the diagram
/// (EDIT-9). Kept across reloads (EDIT-1) and remembered per file
/// (EDIT-13).
// DECISION: hot (read by every coordinate conversion while dragging):
// repr(C), 12 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// The scene point shown at the canvas's top-left corner, scene px.
    pub pan: ScenePoint,
    /// Screen px per scene px.
    pub zoom: f32,
}

impl Default for Camera {
    /// Pan 0,0, zoom 1 (EDIT-9).
    fn default() -> Self {
        Camera {
            pan: ScenePoint { x: 0.0, y: 0.0 },
            zoom: 1.0,
        }
    }
}

impl Camera {
    /// Scene px to screen px. Serves EDIT-9, EDIT-12.
    #[must_use]
    #[expect(clippy::wrong_self_convention, reason = "contract signature")]
    pub fn to_screen(&self, scene_px: Point) -> ScreenPoint {
        // DECISION: scene px are small integers (a few thousand), exact in
        // f32, so the cast loses nothing.
        #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
        let (x, y) = (scene_px.x as f32, scene_px.y as f32);
        ScreenPoint {
            x: (x - self.pan.x) * self.zoom,
            y: (y - self.pan.y) * self.zoom,
        }
    }

    /// Screen px to scene px, unrounded. Serves EDIT-9, EDIT-28.
    #[must_use]
    #[expect(clippy::wrong_self_convention, reason = "contract signature")]
    pub fn to_scene(&self, screen: ScreenPoint) -> ScenePoint {
        ScenePoint {
            x: screen.x / self.zoom + self.pan.x,
            y: screen.y / self.zoom + self.pan.y,
        }
    }

    /// Multiplies the zoom by `factor`, clamped to
    /// `MIN_ZOOM..=MAX_ZOOM`, keeping the scene point under
    /// `cursor` under it. Serves EDIT-9.
    pub fn zoom_at(&mut self, cursor: ScreenPoint, factor: f32) {
        let under = self.to_scene(cursor);
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if !zoom.is_finite() {
            return;
        }
        self.zoom = zoom;
        self.pan = ScenePoint {
            x: under.x - cursor.x / zoom,
            y: under.y - cursor.y / zoom,
        };
    }

    /// The camera that shows all of `bounds` (scene px) centred in a
    /// canvas of `canvas` screen px, with `FIT_MARGIN_PX` around it,
    /// never above zoom 1 nor outside `MIN_ZOOM..=MAX_ZOOM`. Serves
    /// EDIT-30.
    // DECISION: crate-private, so the step-4 contract gains no pub item;
    // `state` calls it on first show and on "Fit view".
    pub(crate) fn fit(bounds: Rect, canvas: ScreenPoint) -> Camera {
        #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
        let (w, h) = (bounds.width() as f32, bounds.height() as f32);
        let room = |side: f32| (side - 2.0 * FIT_MARGIN_PX).max(0.0);
        let mut zoom: f32 = 1.0;
        if w > 0.0 {
            zoom = zoom.min(room(canvas.x) / w);
        }
        if h > 0.0 {
            zoom = zoom.min(room(canvas.y) / h);
        }
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
        let (cx, cy) = (
            (bounds.min.x + bounds.max.x) as f32 / 2.0,
            (bounds.min.y + bounds.max.y) as f32 / 2.0,
        );
        Camera {
            pan: ScenePoint {
                x: cx - canvas.x / 2.0 / zoom,
                y: cy - canvas.y / 2.0 / zoom,
            },
            zoom,
        }
    }

    /// Moves the view by `delta` screen px (the diagram follows the
    /// pointer). Serves EDIT-9.
    pub fn pan_by(&mut self, delta: ScreenPoint) {
        self.pan.x -= delta.x / self.zoom;
        self.pan.y -= delta.y / self.zoom;
    }
}

#[cfg(test)]
mod tests {
    // Layout pins for the hot structs.

    #[test]
    fn screen_point_is_8_bytes() {
        assert_eq!(std::mem::size_of::<super::ScreenPoint>(), 8);
    }

    #[test]
    fn scene_point_is_8_bytes() {
        assert_eq!(std::mem::size_of::<super::ScenePoint>(), 8);
    }

    #[test]
    fn camera_is_12_bytes() {
        assert_eq!(std::mem::size_of::<super::Camera>(), 12);
    }

    use super::{Camera, MAX_ZOOM, MIN_ZOOM, ScreenPoint};
    use c4rizer_core::geom::Point;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// EDIT-9: screen and scene px convert both ways through pan and zoom.
    #[test]
    fn edit_9_round_trip_through_camera() {
        let mut camera = Camera::default();
        camera.pan_by(ScreenPoint { x: 30.0, y: -10.0 });
        camera.zoom_at(ScreenPoint { x: 100.0, y: 50.0 }, 2.0);
        let s = camera.to_screen(Point::new(240, 130));
        let back = camera.to_scene(s);
        assert!(close(back.x, 240.0) && close(back.y, 130.0), "{back:?}");
    }

    /// EDIT-9: the scene point under the cursor stays under it; zoom is
    /// clamped.
    #[test]
    fn edit_9_zoom_at_keeps_cursor_point() {
        let mut camera = Camera::default();
        let cursor = ScreenPoint { x: 120.0, y: 80.0 };
        let before = camera.to_scene(cursor);
        camera.zoom_at(cursor, 1.1);
        let after = camera.to_scene(cursor);
        assert!(close(before.x, after.x) && close(before.y, after.y));
        camera.zoom_at(cursor, 1000.0);
        assert!(close(camera.zoom, MAX_ZOOM));
        camera.zoom_at(cursor, 0.000_01);
        assert!(close(camera.zoom, MIN_ZOOM));
    }

    /// EDIT-9: panning moves the diagram with the pointer.
    #[test]
    fn edit_9_pan_follows_pointer() {
        let mut camera = Camera::default();
        let p = camera.to_screen(Point::new(50, 50));
        camera.pan_by(ScreenPoint { x: 10.0, y: 5.0 });
        let q = camera.to_screen(Point::new(50, 50));
        assert!(close(q.x - p.x, 10.0) && close(q.y - p.y, 5.0));
    }

    use c4rizer_core::geom::Rect;

    /// EDIT-30: a scene larger than the canvas is fitted with a 20px
    /// margin on the tighter axis and centred on the other.
    #[test]
    fn edit_30_fit_large_scene_into_canvas_with_margin() {
        let bounds = Rect::from_corners(Point::new(-100, 50), Point::new(1900, 1050));
        let canvas = ScreenPoint {
            x: 848.0,
            y: 1400.0,
        };
        let camera = Camera::fit(bounds, canvas);
        assert!(close(camera.zoom, (848.0 - 40.0) / 2000.0), "{camera:?}");
        let a = camera.to_screen(bounds.min);
        let b = camera.to_screen(bounds.max);
        assert!(close(a.x, 20.0) && close(b.x, 828.0), "{a:?} {b:?}");
        assert!(
            close(a.y + b.y, canvas.y),
            "centred vertically: {a:?} {b:?}"
        );
        assert!(a.y >= 20.0 && b.y <= canvas.y - 20.0);
    }

    /// EDIT-30: a scene smaller than the canvas is never zoomed above
    /// 100%; it is centred.
    #[test]
    fn edit_30_fit_never_zooms_above_100_percent() {
        let bounds = Rect::from_corners(Point::new(0, 0), Point::new(300, 200));
        let canvas = ScreenPoint {
            x: 1600.0,
            y: 900.0,
        };
        let camera = Camera::fit(bounds, canvas);
        assert!(close(camera.zoom, 1.0), "{camera:?}");
        let c = camera.to_screen(bounds.centre());
        assert!(close(c.x, 800.0) && close(c.y, 450.0), "{c:?}");
    }

    /// EDIT-30: a canvas too small for the margin still gives a finite
    /// camera within the zoom limits.
    #[test]
    fn edit_30_fit_tiny_canvas_stays_in_zoom_limits() {
        let bounds = Rect::from_corners(Point::new(0, 0), Point::new(3000, 2000));
        for canvas in [
            ScreenPoint { x: 10.0, y: 10.0 },
            ScreenPoint { x: 0.0, y: 0.0 },
        ] {
            let camera = Camera::fit(bounds, canvas);
            assert!(close(camera.zoom, MIN_ZOOM), "{camera:?}");
            assert!(camera.pan.x.is_finite() && camera.pan.y.is_finite());
        }
        let empty = Rect::from_corners(Point::new(5, 5), Point::new(5, 5));
        let camera = Camera::fit(empty, ScreenPoint { x: 400.0, y: 300.0 });
        assert!(close(camera.zoom, 1.0) && camera.pan.x.is_finite());
    }
}
