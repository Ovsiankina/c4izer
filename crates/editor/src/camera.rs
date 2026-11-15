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

use c4rizer_core::geom::Point;

/// Smallest zoom factor (EDIT-9).
pub const MIN_ZOOM: f32 = 0.1;

/// Largest zoom factor (EDIT-9).
pub const MAX_ZOOM: f32 = 8.0;

/// Zoom factor per wheel notch (EDIT-9).
pub const ZOOM_STEP: f32 = 1.1;

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
        todo!()
    }
}

impl Camera {
    /// Scene px to screen px. Serves EDIT-9, EDIT-12.
    #[must_use]
    pub fn to_screen(&self, scene_px: Point) -> ScreenPoint {
        todo!()
    }

    /// Screen px to scene px, unrounded. Serves EDIT-9, EDIT-28.
    #[must_use]
    pub fn to_scene(&self, screen: ScreenPoint) -> ScenePoint {
        todo!()
    }

    /// Multiplies the zoom by `factor`, clamped to
    /// `MIN_ZOOM..=MAX_ZOOM`, keeping the scene point under
    /// `cursor` under it. Serves EDIT-9.
    pub fn zoom_at(&mut self, cursor: ScreenPoint, factor: f32) {
        todo!()
    }

    /// Moves the view by `delta` screen px (the diagram follows the
    /// pointer). Serves EDIT-9.
    pub fn pan_by(&mut self, delta: ScreenPoint) {
        todo!()
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
}
