//! Paints the draw list (a scene plus selection and hover marks) with
//! egui shapes and the bundled font, and turns egui pointer and key input
//! into `state::Input`; draws nothing outside the scene but selection and
//! hover marks (EDIT-12). Hit-testing is `Scene::hit`, called by `state`.
//!
//! Depends on: `state`, `camera`, `c4rizer-core` (scene), `eframe`
//! (egui).

// Shape: two steps, so EDIT-12 is tested without a window. `paint_items`
// flattens a `DrawList` into `PaintItem`s in the scene's paint order
// (scene.rs header: boundaries, lines, boxes, labels, title, subtitle,
// key), each tagged with the scene item it comes from, then the marks;
// `paint` turns those into egui shapes. `inputs` reads one frame of egui
// input over the canvas rect.
//
// DECISION: the canvas paints in screen px through `camera::Camera`; text
// is drawn line by line as `TextBlock::lines` gives it, never re-wrapped
// by egui (CORE-30: the scene already wrapped it), at `size_px * zoom`.

use c4rizer_core::scene::{ItemKey, TextBlock};

use crate::camera::{Camera, ScreenPoint};
use crate::state::{DrawList, Input, MarkKind};

/// What one painted item stands for: a scene item, or a mark; nothing
/// else is ever painted (EDIT-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// Drawn from this scene item.
    Scene(ItemKey),
    /// A selection or hover mark on this item.
    Mark(ItemKey, MarkKind),
    /// The rubber band (a selection mark, EDIT-20).
    RubberBand,
    /// The outline of the box being resized at its release size (a
    /// selection mark, EDIT-18).
    ResizeOutline,
}

/// One primitive to paint, in screen px: the egui-free form of a scene
/// item or mark, so EDIT-12 is tested without a window.
#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    /// A closed outline, filled or not (box shapes, person, cylinder
    /// approximated by points, boundaries, marks).
    Polygon {
        /// Vertices, screen px.
        points: Vec<ScreenPoint>,
        /// Fill `#rrggbb` as `[r, g, b]`, `None` for no fill.
        fill: Option<[u8; 3]>,
        /// Stroke colour and width px, `None` for no stroke.
        stroke: Option<([u8; 3], f32)>,
        /// Dash and gap, px, `None` for solid.
        dash: Option<[f32; 2]>,
    },
    /// An open polyline (lines, chevrons).
    Path {
        /// Vertices, screen px.
        points: Vec<ScreenPoint>,
        /// Colour.
        colour: [u8; 3],
        /// Width, px.
        width: f32,
        /// Dash and gap, px, `None` for solid.
        dash: Option<[f32; 2]>,
    },
    /// A filled circle (join dots, person heads).
    Circle {
        /// Centre, screen px.
        centre: ScreenPoint,
        /// Radius, px.
        radius: f32,
        /// Fill.
        fill: [u8; 3],
    },
    /// One text line.
    Text {
        /// Anchor, screen px (start or middle per the block's `Align`).
        at: ScreenPoint,
        /// The line, as wrapped by the scene.
        text: String,
        /// Font size, px.
        size: f32,
        /// True for the bold face.
        bold: bool,
        /// True for middle alignment.
        centred: bool,
        /// Colour.
        colour: [u8; 3],
    },
}

/// One painted item and where it comes from (EDIT-12).
#[derive(Debug, Clone, PartialEq)]
pub struct PaintItem {
    /// The scene item or mark.
    pub source: Source,
    /// The primitive.
    pub primitive: Primitive,
}

/// Every primitive of `list`, in paint order: the scene items in the
/// scene's paint order, then the marks, then the resize outline, then
/// the rubber band. Nothing that is not from `list.scene` or a mark
/// (EDIT-12). Empty when the list has no scene. Serves EDIT-12, EDIT-9,
/// EDIT-15, EDIT-18.
#[must_use]
pub fn paint_items(list: &DrawList<'_>) -> Vec<PaintItem> {
    todo!()
}

/// The text lines of `block` as `Primitive::Text`s through `camera`,
/// line `i` at `block.at` plus `i * line_px` (scene px), sizes scaled by
/// the zoom. Serves EDIT-12, CORE-30.
#[must_use]
pub fn text_items(block: &TextBlock, camera: &Camera) -> Vec<Primitive> {
    todo!()
}

/// Paints `items` with `painter`, clipped to its rect, whose top-left is
/// screen 0,0 of the state's coordinates. Serves EDIT-12.
pub fn paint(painter: &eframe::egui::Painter, items: &[PaintItem]) {
    todo!()
}

/// The `state::Input`s of this frame over the canvas `response`: pointer
/// down, move, up (relative to the canvas's top-left), leave, wheel, and
/// the keys of `state::Key` with modifiers; `text_focus` true when an
/// egui text field has focus. Serves EDIT-9, EDIT-28.
#[must_use]
pub fn inputs(ctx: &eframe::egui::Context, response: &eframe::egui::Response) -> Vec<Input> {
    todo!()
}

/// Registers the bundled font (CORE-30, HLD §6) with egui as the
/// proportional family, regular and bold, so the canvas paints with the
/// same face the scene measured and the SVG names. Serves EDIT-12,
/// CORE-30.
// Blocked: no pub item gives the font bytes yet (docs/deferred.md row
// "editor: EDIT-7 PNG and canvas font", on top of the scene/export
// `FONT_BYTES` and assets rows); `edit_12_canvas_uses_bundled_font` is
// ignored until then. No fallback face is part of this contract.
pub fn install_fonts(ctx: &eframe::egui::Context) {
    todo!()
}

#[cfg(test)]
mod tests {
    /// EDIT-12 (plan editor-02): every `PaintItem` comes from a scene item
    /// (`Source::Scene`, one or more per box, line, label, boundary,
    /// title, subtitle, key) or a mark; every scene item is painted; the
    /// scene items are in the scene's paint order.
    #[test]
    fn edit_12_canvas_items_match_scene() {
        todo!()
    }

    /// EDIT-12: a draw list without scene paints nothing.
    #[test]
    fn edit_12_no_scene_paints_nothing() {
        todo!()
    }

    /// EDIT-18: while resizing, the outline is painted as
    /// `Source::ResizeOutline` after the marks, and nothing else is
    /// added.
    #[test]
    fn edit_18_resize_outline_painted_as_mark() {
        todo!()
    }

    /// EDIT-12, CORE-30: after `install_fonts`, egui's proportional
    /// family is the bundled face (regular and bold), no other face.
    /// Needs the font bytes as a pub item (docs/deferred.md row "editor:
    /// EDIT-7 PNG and canvas font").
    #[test]
    #[ignore = "deferred"]
    fn edit_12_canvas_uses_bundled_font() {
        todo!()
    }

    /// EDIT-9: zoom 2 doubles every screen distance between painted
    /// points and changes no scene value.
    #[test]
    fn edit_9_zoom_scales_paint_only() {
        todo!()
    }
}
