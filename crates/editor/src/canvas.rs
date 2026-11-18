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

use c4rizer_core::geom::{Point, Rect};
use c4rizer_core::label::Weight;
use c4rizer_core::model::{Asset, Colour};
use c4rizer_core::scene::{
    Align, FONT_BOLD, FONT_REGULAR, ItemKey, Outline, Stroke, Swatch, TextBlock, Tip, WindowKind,
};

use crate::camera::{Camera, ScreenPoint};
use crate::state::{Button, DrawList, HANDLE_PX, Input, Key, MarkKind, Modifiers};

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
    /// An icon (P2-409): its rectangle and the image file, painted as a
    /// texture of `export::rasterize_asset` (`icon_image`).
    Image {
        /// Top-left, screen px.
        min: ScreenPoint,
        /// Bottom-right, screen px.
        max: ScreenPoint,
        /// The icon's file, as the SVG embeds it.
        asset: Asset,
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
    /// 0..100: the item's opacity (P2-406, P2-426); 100 for marks.
    pub opacity: u8,
}

/// Every primitive of `list`, in paint order: the scene items in the
/// scene's paint order, then the marks, then the resize outline, then
/// the rubber band. Nothing that is not from `list.scene` or a mark
/// (EDIT-12). Empty when the list has no scene. Serves EDIT-12, EDIT-9,
/// EDIT-15, EDIT-18.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one flat pass in the scene's paint order")]
pub fn paint_items(list: &DrawList<'_>) -> Vec<PaintItem> {
    let Some(scene) = list.scene else {
        return Vec::new();
    };
    let cam = &list.camera;
    let mut out = Vec::new();
    let opacity = std::cell::Cell::new(100_u8);
    let mut push = |source: Source, primitive: Primitive| {
        // P2-725, P2-726: a faded item is painted at 20% of its opacity.
        let faded = matches!(source, Source::Scene(k) if list.faded.contains(&k));
        let opacity = if faded { opacity.get() / 5 } else { opacity.get() };
        out.push(PaintItem {
            source,
            primitive,
            opacity,
        });
    };

    if let Some(image) = &scene.image {
        // DECISION: an image view's picture belongs to no box or line; it
        // is painted under the title's source (EDIT-12: from the scene).
        let source = Source::Scene(ItemKey::Title);
        if let Some(asset) = image.image.asset.as_ref().filter(|_| image.placeholder.is_none()) {
            // P2-320: the same file as the SVG and PNG exports.
            push(source, Primitive::Image {
                min: cam.to_screen(image.rect.min),
                max: cam.to_screen(image.rect.max),
                asset: asset.clone(),
            });
        } else {
            // P2-321: the dashed placeholder naming the source.
            let grey = [0x66, 0x66, 0x66];
            push(source, Primitive::Polygon {
                points: rect_points(cam, image.rect, 0),
                fill: None,
                stroke: Some((grey, cam.zoom)),
                dash: Some([8.0 * cam.zoom, 6.0 * cam.zoom]),
            });
            let centre = image.rect.centre();
            push(source, Primitive::Text {
                at: cam.to_screen(Point::new(centre.x, centre.y + 5)),
                text: format!("{} (not drawn)", image.placeholder.as_deref().unwrap_or("image")),
                size: 14.0 * cam.zoom,
                bold: false,
                centred: true,
                colour: grey,
            });
        }
    }
    for (i, b) in scene.boundaries.iter().enumerate() {
        let source = Source::Scene(ItemKey::Boundary(u32::try_from(i).unwrap_or(u32::MAX)));
        opacity.set(b.opacity);
        push(source, polygon(rect_points(cam, b.rect, 0), b.fill.map(rgb), Some(&b.stroke), cam));
        for t in text_items(&b.tag, cam) {
            push(source, t);
        }
        // P3-115: a deployment node's instance count.
        for t in b.instances.iter().flat_map(|c| text_items(c, cam)) {
            push(source, t);
        }
    }
    for l in &scene.lines {
        let source = Source::Scene(ItemKey::Line(l.edge));
        opacity.set(l.opacity);
        let colour = rgb(l.stroke.colour);
        let width = px(l.stroke.width_px) * cam.zoom;
        push(source, Primitive::Path {
            points: route_points(&l.points, &l.jumps, cam),
            colour,
            width,
            dash: dash(&l.stroke, cam),
        });
        // Chevrons and the tip stay solid on a dashed line (P2-420).
        for c in &l.chevrons {
            push(source, Primitive::Path {
                points: c.points.iter().map(|&p| cam.to_screen(p)).collect(),
                colour,
                width,
                dash: None,
            });
        }
        push(source, tip(&l.tip, colour, cam));
    }
    for b in &scene.boxes {
        let source = Source::Scene(ItemKey::Box(b.id));
        opacity.set(b.opacity);
        for primitive in outline(&b.outline, Some(rgb(b.fill)), Some(&b.stroke), cam) {
            push(source, primitive);
        }
        if let Some(icon) = &b.icon {
            push(source, Primitive::Image {
                min: cam.to_screen(icon.rect.min),
                max: cam.to_screen(icon.rect.max),
                asset: icon.asset.clone(),
            });
        }
        // P2-721, P2-722: hidden texts are not painted; the box keeps its
        // geometry.
        let type_line = if list.hide_metadata { Vec::new() } else { text_items(&b.type_line, cam) };
        let description = match &b.description {
            Some(d) if !list.hide_descriptions => text_items(d, cam),
            _ => Vec::new(),
        };
        for t in text_items(&b.name, cam).into_iter().chain(type_line).chain(description) {
            push(source, t);
        }
    }
    for l in &scene.labels {
        let source = Source::Scene(ItemKey::Label(l.edge));
        opacity.set(l.opacity);
        // P2-721, P2-722: the description lines and the technology line.
        // DECISION: the technology line is the last line when it is
        // bracketed, as `label::label_lines` writes it (`[<technology>]`);
        // the draw list has no plan to ask.
        let lines = &l.text.lines;
        let tech = lines.last().is_some_and(|t| t.starts_with('[') && t.ends_with(']'));
        let shown: Vec<Primitive> = text_items(&l.text, cam)
            .into_iter()
            .enumerate()
            .filter(|&(i, _)| {
                let is_tech = tech && i + 1 == lines.len();
                !((is_tech && list.hide_metadata) || (!is_tech && list.hide_descriptions))
            })
            .map(|(_, t)| t)
            .collect();
        // A label with every line hidden paints no background either: its
        // page-coloured box would cut a gap in the line (seen in the real
        // window).
        if let Some(fill) = l.fill
            && !shown.is_empty()
        {
            push(source, polygon(rect_points(cam, l.rect, 0), Some(rgb(fill)), None, cam));
        }
        for t in shown {
            push(source, t);
        }
    }
    opacity.set(100);
    if let Some(title) = &scene.title {
        for t in text_items(title, cam) {
            push(Source::Scene(ItemKey::Title), t);
        }
    }
    if let Some(subtitle) = &scene.subtitle {
        for t in text_items(subtitle, cam) {
            push(Source::Scene(ItemKey::Subtitle), t);
        }
    }
    // P2-728: the key may be hidden.
    let legend: &[c4rizer_core::scene::LegendEntry] = if list.hide_key { &[] } else { &scene.legend.entries };
    for entry in legend {
        let source = Source::Scene(ItemKey::Legend);
        match &entry.swatch {
            Swatch::Box { outline: o, fill, stroke } => {
                for primitive in outline(o, Some(rgb(*fill)), Some(stroke), cam) {
                    push(source, primitive);
                }
            }
            Swatch::Line { from, to, stroke, tip: t } => {
                let colour = rgb(stroke.colour);
                push(source, Primitive::Path {
                    points: vec![cam.to_screen(*from), cam.to_screen(*to)],
                    colour,
                    width: px(stroke.width_px) * cam.zoom,
                    dash: dash(stroke, cam),
                });
                push(source, tip(t, colour, cam));
            }
            _ => {}
        }
        for t in text_items(&entry.text, cam) {
            push(source, t);
        }
    }

    for mark in &list.marks {
        let Some(rect) = scene.item_bounds(mark.item) else {
            continue;
        };
        let source = Source::Mark(mark.item, mark.kind);
        let (colour, dashed) = match mark.kind {
            MarkKind::Selected | MarkKind::SelectedWithHandles => (SELECTION, false),
            MarkKind::Hover => (HOVER, false),
            MarkKind::Enterable => (ENTERABLE, true),
        };
        push(source, Primitive::Polygon {
            points: rect_points(cam, rect.inflate(MARK_GAP_PX), 0),
            fill: None,
            stroke: Some((colour, MARK_WIDTH)),
            dash: dashed.then_some([6.0, 4.0]),
        });
        if mark.kind == MarkKind::SelectedWithHandles && !list.read_only {
            // P2-735: a handle over a line end is drawn outside the edge.
            let ends = crate::state::line_ends(scene, cam);
            for (_, centre) in crate::state::handle_spots(rect, cam, &ends) {
                push(source, square(centre, HANDLE_PX, SELECTION));
            }
        }
    }
    if let Some((a, b)) = list.resize_outline {
        push(Source::ResizeOutline, screen_rect(a, b, SELECTION, None));
    }
    if let Some((a, b)) = list.rubber_band {
        push(Source::RubberBand, screen_rect(a, b, SELECTION, Some([4.0, 3.0])));
    }
    out
}

/// The vertex handles of the selected line (P3-311), painted over the
/// marks: one square per vertex at `handles` (screen px,
/// `State::vertex_handles`), as a selection mark on that line.
pub(crate) fn vertex_handle_items(list: &DrawList<'_>, handles: &[ScreenPoint]) -> Vec<PaintItem> {
    let Some(item) = list.marks.iter().map(|m| m.item).find(|i| matches!(i, ItemKey::Line(_))) else {
        return Vec::new();
    };
    handles
        .iter()
        .map(|&centre| PaintItem {
            source: Source::Mark(item, MarkKind::Selected),
            primitive: square(centre, HANDLE_PX, SELECTION),
            opacity: 100,
        })
        .collect()
}

// DECISION: mark colours are dark enough for 4.5:1 against the white
// page (EDIT-31): 2px strokes need more than the 3:1 WCAG floor for
// graphics to read at a glance. Selection 6.4:1, hover 5.3:1,
// enterable 5.4:1; handles keep their white rim for box fills.
/// Selection mark colour.
const SELECTION: [u8; 3] = [0x0b, 0x57, 0xd0];
/// Hover mark colour.
const HOVER: [u8; 3] = [0x6b, 0x6b, 0x6b];
/// Hover mark colour of an enterable box (EDIT-15).
const ENTERABLE: [u8; 3] = [0x0f, 0x7a, 0x40];
/// Mark stroke width, screen px.
const MARK_WIDTH: f32 = 2.0;
/// Gap between an item and its mark, scene px.
const MARK_GAP_PX: i32 = 3;
/// Inter's ascender in em (hhea ascender 2728 / 2816): from a baseline to
/// the top egui places text at.
// DECISION: a constant of the bundled face rather than a font query, so
// `paint` needs no font lookup per text line.
const ASCENT_EM: f32 = 0.969;

/// `i32` px to `f32`.
#[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
fn px(v: i32) -> f32 {
    v as f32
}

/// A colour as `[r, g, b]`.
fn rgb(c: Colour) -> [u8; 3] {
    [c.r, c.g, c.b]
}

/// The stroke's dash scaled to the screen.
fn dash(stroke: &Stroke, cam: &Camera) -> Option<[f32; 2]> {
    stroke.dash_px.map(|[d, g]| [px(d) * cam.zoom, px(g) * cam.zoom])
}

/// A scene point given in `f32` scene px, through the camera.
fn at(cam: &Camera, x: f32, y: f32) -> ScreenPoint {
    ScreenPoint {
        x: (x - cam.pan.x) * cam.zoom,
        y: (y - cam.pan.y) * cam.zoom,
    }
}

/// Points of `rect` (scene px) with rounded corners of `radius` px.
fn rect_points(cam: &Camera, rect: Rect, radius: i32) -> Vec<ScreenPoint> {
    let (x0, y0, x1, y1) = (px(rect.min.x), px(rect.min.y), px(rect.max.x), px(rect.max.y));
    let r = px(radius).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
    if r == 0.0 {
        return vec![at(cam, x0, y0), at(cam, x1, y0), at(cam, x1, y1), at(cam, x0, y1)];
    }
    let mut points = Vec::with_capacity(4 * (CORNER_STEPS + 1));
    // Corners clockwise from top right, each a quarter arc.
    let corners = [
        (x1 - r, y0 + r, -90.0_f32),
        (x1 - r, y1 - r, 0.0),
        (x0 + r, y1 - r, 90.0),
        (x0 + r, y0 + r, 180.0),
    ];
    for (cx, cy, start) in corners {
        for k in 0..=CORNER_STEPS {
            #[expect(clippy::cast_precision_loss, reason = "small step count")]
            let a = (start + 90.0 * k as f32 / CORNER_STEPS as f32).to_radians();
            points.push(at(cam, cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    points
}

/// Segments per rounded corner.
const CORNER_STEPS: usize = 4;
/// Segments per full ellipse.
const ELLIPSE_STEPS: usize = 32;

/// Points of an elliptic arc from `from` to `to` degrees (y down).
#[expect(clippy::too_many_arguments, reason = "an arc is centre, radii, angles and steps")]
fn arc(cam: &Camera, centre: (f32, f32), radii: (f32, f32), from: f32, to: f32, steps: usize) -> Vec<ScreenPoint> {
    (0..=steps)
        .map(|k| {
            #[expect(clippy::cast_precision_loss, reason = "small step count")]
            let a = (from + (to - from) * k as f32 / steps as f32).to_radians();
            at(cam, centre.0 + radii.0 * a.cos(), centre.1 + radii.1 * a.sin())
        })
        .collect()
}

/// A closed outline primitive.
fn polygon(points: Vec<ScreenPoint>, fill: Option<[u8; 3]>, stroke: Option<&Stroke>, cam: &Camera) -> Primitive {
    Primitive::Polygon {
        points,
        fill,
        stroke: stroke.map(|s| (rgb(s.colour), px(s.width_px) * cam.zoom)),
        dash: stroke.and_then(|s| dash(s, cam)),
    }
}

/// The primitives of a box outline (CORE-2), as the SVG draws it.
fn outline(o: &Outline, fill: Option<[u8; 3]>, stroke: Option<&Stroke>, cam: &Camera) -> Vec<Primitive> {
    match *o {
        Outline::Rect { rect, radius_px } => vec![polygon(rect_points(cam, rect, radius_px), fill, stroke, cam)],
        Outline::Person {
            head_centre,
            head_radius_px,
            body,
            body_radius_px,
        } => {
            let r = px(head_radius_px);
            let head = arc(cam, (px(head_centre.x), px(head_centre.y)), (r, r), 0.0, 360.0, ELLIPSE_STEPS);
            vec![
                polygon(rect_points(cam, body, body_radius_px), fill, stroke, cam),
                polygon(head, fill, stroke, cam),
            ]
        }
        Outline::Cylinder { rect, cap_ry_px } => {
            let (x0, x1) = (px(rect.min.x), px(rect.max.x));
            let (y0, y1) = (px(rect.min.y), px(rect.max.y));
            let (cx, rx, ry) = (f32::midpoint(x0, x1), (x1 - x0) / 2.0, px(cap_ry_px));
            let half = ELLIPSE_STEPS / 2;
            // Body: top cap's upper half, right side, bottom cap's lower
            // half, left side.
            let mut body = arc(cam, (cx, y0 + ry), (rx, ry), 180.0, 360.0, half);
            body.extend(arc(cam, (cx, y1 - ry), (rx, ry), 0.0, 180.0, half));
            let rim = arc(cam, (cx, y0 + ry), (rx, ry), 0.0, 180.0, half);
            let mut out = vec![polygon(body, fill, stroke, cam)];
            if let Some(s) = stroke {
                out.push(Primitive::Path {
                    points: rim,
                    colour: rgb(s.colour),
                    width: px(s.width_px) * cam.zoom,
                    dash: dash(s, cam),
                });
            }
            out
        }
        _ => shape_outline(o, fill, stroke, cam),
    }
}

/// An open stroke in `stroke`'s colour, width and dash.
fn stroke_path(points: Vec<ScreenPoint>, stroke: Option<&Stroke>, cam: &Camera) -> Option<Primitive> {
    stroke.map(|s| Primitive::Path {
        points,
        colour: rgb(s.colour),
        width: px(s.width_px) * cam.zoom,
        dash: dash(s, cam),
    })
}

/// The phase-2 outlines (P2-11), the same parts the SVG draws
/// (`export.rs`).
fn shape_outline(o: &Outline, fill: Option<[u8; 3]>, stroke: Option<&Stroke>, cam: &Camera) -> Vec<Primitive> {
    let half = ELLIPSE_STEPS / 2;
    match *o {
        Outline::Ellipse { rect, .. } => {
            let (x0, y0, x1, y1) = (px(rect.min.x), px(rect.min.y), px(rect.max.x), px(rect.max.y));
            let points = arc(cam, (f32::midpoint(x0, x1), f32::midpoint(y0, y1)), ((x1 - x0) / 2.0, (y1 - y0) / 2.0), 0.0, 360.0, ELLIPSE_STEPS);
            vec![polygon(points, fill, stroke, cam)]
        }
        Outline::Hexagon { rect, inset_px } => {
            let (x0, y0, x1, y1) = (px(rect.min.x), px(rect.min.y), px(rect.max.x), px(rect.max.y));
            let (i, cy) = (px(inset_px), f32::midpoint(y0, y1));
            let points = vec![at(cam, x0 + i, y0), at(cam, x1 - i, y0), at(cam, x1, cy), at(cam, x1 - i, y1), at(cam, x0 + i, y1), at(cam, x0, cy)];
            vec![polygon(points, fill, stroke, cam)]
        }
        Outline::Diamond { rect } => {
            let (x0, y0, x1, y1) = (px(rect.min.x), px(rect.min.y), px(rect.max.x), px(rect.max.y));
            let (cx, cy) = (f32::midpoint(x0, x1), f32::midpoint(y0, y1));
            vec![polygon(vec![at(cam, cx, y0), at(cam, x1, cy), at(cam, cx, y1), at(cam, x0, cy)], fill, stroke, cam)]
        }
        Outline::Bucket { rect, cap_ry_px } => {
            let (x0, y0, x1, y1) = (px(rect.min.x), px(rect.min.y), px(rect.max.x), px(rect.max.y));
            let (cx, rx, ry, n) = (f32::midpoint(x0, x1), (x1 - x0) / 2.0, px(cap_ry_px), px(rect.width() / 10));
            let mut body = arc(cam, (cx, y0 + ry), (rx, ry), 180.0, 360.0, half);
            body.extend([at(cam, x1 - n, y1), at(cam, x0 + n, y1)]);
            let rim = arc(cam, (cx, y0 + ry), (rx, ry), 0.0, 180.0, half);
            let mut out = vec![polygon(body, fill, stroke, cam)];
            out.extend(stroke_path(rim, stroke, cam));
            out
        }
        Outline::Pipe { rect, cap_rx_px } => {
            let (x0, y0, x1, y1) = (px(rect.min.x), px(rect.min.y), px(rect.max.x), px(rect.max.y));
            let (rx, ry, cy) = (px(cap_rx_px), (y1 - y0) / 2.0, f32::midpoint(y0, y1));
            let mut body = arc(cam, (x1 - rx, cy), (rx, ry), -90.0, 90.0, half);
            body.extend(arc(cam, (x0 + rx, cy), (rx, ry), 90.0, 270.0, half));
            let rim = arc(cam, (x1 - rx, cy), (rx, ry), 90.0, 270.0, half);
            let mut out = vec![polygon(body, fill, stroke, cam)];
            out.extend(stroke_path(rim, stroke, cam));
            out
        }
        Outline::Robot { head, body, radius_px } => vec![
            polygon(rect_points(cam, head, radius_px), fill, stroke, cam),
            polygon(rect_points(cam, body, radius_px), fill, stroke, cam),
        ],
        Outline::Folder { rect, tab } => vec![
            polygon(rect_points(cam, tab, 0), fill, stroke, cam),
            polygon(rect_points(cam, rect, 0), fill, stroke, cam),
        ],
        Outline::Window { rect, bar_px, kind } => {
            let mut out = vec![polygon(rect_points(cam, rect, 0), fill, stroke, cam)];
            for marks in window_marks(rect, bar_px, kind, cam) {
                out.extend(stroke_path(marks, stroke, cam));
            }
            out
        }
        Outline::MobileDevice {
            rect,
            portrait,
            inset_px,
        } => vec![
            polygon(rect_points(cam, rect, inset_px), fill, stroke, cam),
            polygon(rect_points(cam, mobile_screen(rect, portrait, inset_px), 0), None, stroke, cam),
        ],
        Outline::Component { rect, tabs } => {
            let mut out = vec![polygon(rect_points(cam, rect, 0), fill, stroke, cam)];
            out.extend(tabs.iter().map(|t| polygon(rect_points(cam, *t, 0), fill, stroke, cam)));
            out
        }
        _ => Vec::new(),
    }
}

/// The screen of a phone outline, as core's `scene::mobile_screen` gives
/// it to the SVG: deep bezel on the short sides (top and bottom when
/// portrait).
fn mobile_screen(rect: Rect, portrait: bool, inset_px: i32) -> Rect {
    let (dx, dy) = if portrait { (inset_px, 2 * inset_px) } else { (2 * inset_px, inset_px) };
    Rect::from_corners(
        Point::new(rect.min.x + dx, rect.min.y + dy),
        Point::new(rect.max.x - dx, rect.max.y - dy),
    )
}

/// The decoration strokes of a window-like outline, as the SVG's
/// `export::window_marks` draws them: the bar edge (not for a shell), then
/// an address field, three buttons or a prompt.
fn window_marks(rect: Rect, bar_px: i32, kind: WindowKind, cam: &Camera) -> Vec<Vec<ScreenPoint>> {
    let (left, right) = (px(rect.min.x), px(rect.max.x));
    let bar = px(rect.min.y + bar_px);
    let mid = px(rect.min.y) + px(bar_px) / 2.0;
    let unit = px((bar_px / 5).max(1));
    let mut out = Vec::new();
    if kind != WindowKind::Shell {
        out.push(vec![at(cam, left, bar), at(cam, right, bar)]);
    }
    match kind {
        WindowKind::WebBrowser => {
            let (from, to) = (left + 4.0 * unit + px(bar_px), right - 2.0 * unit);
            out.push(vec![at(cam, from, mid - unit), at(cam, to, mid - unit), at(cam, to, mid + unit), at(cam, from, mid + unit), at(cam, from, mid - unit)]);
        }
        WindowKind::Window => {
            for i in 1..=3_u8 {
                let cx = left + f32::from(i) * 3.0 * unit;
                out.push(arc(cam, (cx, mid), (unit, unit), 0.0, 360.0, 12));
            }
        }
        _ => {
            let (x, y) = (left + px(bar_px) / 2.0, bar + px(bar_px) / 2.0);
            out.push(vec![at(cam, x, y), at(cam, x + 2.0 * unit, y + 2.0 * unit), at(cam, x, y + 4.0 * unit)]);
            if kind == WindowKind::Terminal {
                out.push(vec![at(cam, x + 3.0 * unit, y + 4.0 * unit), at(cam, x + 6.0 * unit, y + 4.0 * unit)]);
            }
        }
    }
    out
}

/// A route's points through the camera, with a 6px half-circle hop at
/// each of `jumps` (P2-422), as the SVG draws it.
fn route_points(points: &[Point], jumps: &[Point], cam: &Camera) -> Vec<ScreenPoint> {
    let mut out = Vec::with_capacity(points.len() + jumps.len() * 10);
    for (i, &p) in points.iter().enumerate() {
        if i > 0 {
            let a = points[i - 1];
            let (dx, dy) = (px(p.x - a.x), px(p.y - a.y));
            let mut on: Vec<Point> = jumps.iter().copied().filter(|j| on_segment(a, p, *j)).collect();
            on.sort_by_key(|j| (j.x - a.x).abs() + (j.y - a.y).abs());
            let heading = dy.atan2(dx).to_degrees();
            for j in on {
                // From behind the crossing, over it (sweep as the SVG's
                // arc flag 1: through the left of the heading, y down).
                out.extend(arc(cam, (px(j.x), px(j.y)), (HOP_PX, HOP_PX), heading + 180.0, heading + 360.0, 8));
            }
        }
        out.push(cam.to_screen(p));
    }
    out
}

/// The hop radius, scene px (P2-422; the SVG's arc).
const HOP_PX: f32 = 6.0;

/// True when `p` is on the segment `a`..`b`, ends excluded.
fn on_segment(a: Point, b: Point, p: Point) -> bool {
    let cross = i64::from(b.x - a.x) * i64::from(p.y - a.y) - i64::from(b.y - a.y) * i64::from(p.x - a.x);
    let within = |v: i32, l: i32, h: i32| v >= l.min(h) && v <= l.max(h);
    cross == 0 && within(p.x, a.x, b.x) && within(p.y, a.y, b.y) && p != a && p != b
}

/// A line's arrowhead or join dot.
fn tip(t: &Tip, colour: [u8; 3], cam: &Camera) -> Primitive {
    match *t {
        Tip::Dot { at: centre, radius_px } => Primitive::Circle {
            centre: cam.to_screen(centre),
            radius: px(radius_px) * cam.zoom,
            fill: colour,
        },
        Tip::Arrow(points) => Primitive::Polygon {
            points: points.iter().map(|&p| cam.to_screen(p)).collect(),
            fill: Some(colour),
            stroke: None,
            dash: None,
        },
        _ => Primitive::Path {
            points: Vec::new(),
            colour,
            width: 0.0,
            dash: None,
        },
    }
}

/// A filled square of half side `half` around `centre`, screen px.
fn square(centre: ScreenPoint, half: f32, colour: [u8; 3]) -> Primitive {
    let p = |dx: f32, dy: f32| ScreenPoint {
        x: centre.x + dx,
        y: centre.y + dy,
    };
    Primitive::Polygon {
        points: vec![p(-half, -half), p(half, -half), p(half, half), p(-half, half)],
        fill: Some(colour),
        stroke: Some(([0xff, 0xff, 0xff], 1.0)),
        dash: None,
    }
}

/// An outlined rectangle between two screen corners.
fn screen_rect(a: ScreenPoint, b: ScreenPoint, colour: [u8; 3], dash: Option<[f32; 2]>) -> Primitive {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    let p = |x: f32, y: f32| ScreenPoint { x, y };
    Primitive::Polygon {
        points: vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)],
        fill: None,
        stroke: Some((colour, MARK_WIDTH)),
        dash,
    }
}

/// The text lines of `block` as `Primitive::Text`s through `camera`,
/// line `i` at `block.at` plus `i * line_px` (scene px), sizes scaled by
/// the zoom. Serves EDIT-12, CORE-30.
#[must_use]
pub fn text_items(block: &TextBlock, camera: &Camera) -> Vec<Primitive> {
    let bold = matches!(block.weight, Weight::Bold);
    let centred = matches!(block.align, Align::Middle);
    block
        .lines
        .iter()
        .zip(0_i32..)
        .map(|(line, i)| Primitive::Text {
            at: camera.to_screen(Point::new(block.at.x, block.at.y + i * block.line_px)),
            text: line.clone(),
            size: px(block.size_px) * camera.zoom,
            bold,
            centred,
            colour: rgb(block.colour),
        })
        .collect()
}

/// Paints `items` with `painter`, clipped to its rect, whose top-left is
/// screen 0,0 of the state's coordinates. Serves EDIT-12.
pub fn paint(painter: &eframe::egui::Painter, items: &[PaintItem]) {
    for item in items {
        paint_primitive(painter, &item.primitive, item.opacity);
    }
}

/// The page under the scene: `scene.bounds` filled with
/// `scene.background`, the export's background (EDIT-31), so the canvas
/// looks like the exported SVG/PNG whatever the window theme. `None`
/// without a scene.
// DECISION: core exposes the export background as `Scene::background`
// (the SVG fills `bounds` with it), so the page takes it from there and
// no white is hard-coded here. Kept out of `paint_items` so its
// EDIT-12 contract (scene items and marks only, `Source` unchanged)
// holds; the shell paints it first.
pub(crate) fn page(list: &DrawList<'_>) -> Option<Primitive> {
    let scene = list.scene?;
    Some(polygon(
        rect_points(&list.camera, scene.bounds, 0),
        Some(rgb(scene.background)),
        None,
        &list.camera,
    ))
}

/// The paper of a view with a canvas size (`State::paper_rect`, scene
/// px): filled with the export background and outlined in grey, so the
/// canvas size shows on screen (P2-701). Empty without a scene.
// DECISION: painted under the page (the scene bounds), so a drawing that
// spills over the paper still has its page; the outline is a thin grey
// dash, which reads as an edge and not as a diagram boundary.
pub(crate) fn paper(list: &DrawList<'_>, paper: Rect) -> Vec<Primitive> {
    let Some(scene) = list.scene else { return Vec::new() };
    let points = rect_points(&list.camera, paper, 0);
    vec![
        Primitive::Polygon {
            points: points.clone(),
            fill: Some(rgb(scene.background)),
            stroke: None,
            dash: None,
        },
        Primitive::Polygon {
            points,
            fill: None,
            stroke: Some(([0x99, 0x99, 0x99], 1.0)),
            dash: Some([6.0, 4.0]),
        },
    ]
}

/// Paints the paper (`paper`, when the view has a canvas size) and the
/// page of `list` (`page`), if any (P2-701, EDIT-31).
pub(crate) fn paint_page(painter: &eframe::egui::Painter, list: &DrawList<'_>, paper_rect: Option<Rect>) {
    if let Some(r) = paper_rect {
        for p in paper(list, r) {
            paint_primitive(painter, &p, 100);
        }
    }
    if let Some(p) = page(list) {
        paint_primitive(painter, &p, 100);
    }
}

/// Paints one primitive at `opacity`/100; screen 0,0 is the painter's
/// clip rect top-left.
fn paint_primitive(painter: &eframe::egui::Painter, primitive: &Primitive, opacity: u8) {
    use eframe::egui::{Align2, Color32, FontFamily, FontId, Pos2, Shape, Stroke as EStroke};
    let origin = painter.clip_rect().min;
    let pos = |p: &ScreenPoint| Pos2::new(origin.x + p.x, origin.y + p.y);
    let alpha = u8::try_from(u16::from(opacity.min(100)) * 255 / 100).unwrap_or(u8::MAX);
    let colour = |c: [u8; 3]| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], alpha);
    match primitive {
        Primitive::Polygon { points, fill, stroke, dash } => {
            let pts: Vec<Pos2> = points.iter().map(pos).collect();
            if let Some(f) = fill {
                painter.add(Shape::convex_polygon(pts.clone(), colour(*f), EStroke::NONE));
            }
            if let Some((c, w)) = stroke {
                let s = EStroke::new(*w, colour(*c));
                let mut closed = pts;
                if let Some(&first) = closed.first() {
                    closed.push(first);
                }
                match dash {
                    Some([d, g]) => {
                        painter.extend(Shape::dashed_line(&closed, s, *d, *g));
                    }
                    None => {
                        painter.add(Shape::line(closed, s));
                    }
                }
            }
        }
        Primitive::Path { points, colour: c, width, dash } => {
            let pts: Vec<Pos2> = points.iter().map(pos).collect();
            let s = EStroke::new(*width, colour(*c));
            match dash {
                Some([d, g]) => {
                    painter.extend(Shape::dashed_line(&pts, s, *d, *g));
                }
                None => {
                    painter.add(Shape::line(pts, s));
                }
            }
        }
        Primitive::Circle { centre, radius, fill } => {
            painter.circle_filled(pos(centre), *radius, colour(*fill));
        }
        Primitive::Image { min, max, asset } => {
            let rect = eframe::egui::Rect::from_min_max(pos(min), pos(max));
            let ppp = painter.ctx().pixels_per_point();
            if let Some(texture) = icon_texture(painter.ctx(), asset, rect.width() * ppp, rect.height() * ppp) {
                let uv = eframe::egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(texture.id(), rect, uv, Color32::from_white_alpha(alpha));
            }
        }
        Primitive::Text {
            at: anchor,
            text,
            size,
            bold,
            centred,
            colour: c,
        } => {
            if *size < 1.0 {
                return;
            }
            let family = if *bold {
                FontFamily::Name(BOLD_FAMILY.into())
            } else {
                FontFamily::Proportional
            };
            let top = Pos2::new(origin.x + anchor.x, origin.y + anchor.y - ASCENT_EM * size);
            let align = if *centred { Align2::CENTER_TOP } else { Align2::LEFT_TOP };
            painter.text(top, align, text, FontId::new(*size, family), colour(*c));
        }
    }
}

/// `asset` as an egui image of `width` x `height` px (rounded, at least
/// 1), from `export::rasterize_asset`; `None` when the asset does not
/// decode (the icon is then not drawn, as in the SVG and PNG).
pub(crate) fn icon_image(asset: &Asset, width: f32, height: f32) -> Option<eframe::egui::ColorImage> {
    let size = |v: f32| {
        // Screen sizes are small positive floats; the cast saturates.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let px = v.round().max(1.0) as u32;
        px
    };
    let (w, h) = (size(width), size(height));
    let rgba = c4rizer_core::export::rasterize_asset(asset, w, h)?;
    Some(eframe::egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        &rgba,
    ))
}

/// The texture of `asset` at `width` x `height` physical px, kept in the
/// egui context so each icon is rasterized once per size, not per frame.
// DECISION: textures live in egui's temp data under one id, keyed by the
// asset and the pixel size, in a `BTreeMap` cleared when it passes 256
// entries (zooming through many sizes), so memory stays bounded.
fn icon_texture(
    ctx: &eframe::egui::Context,
    asset: &Asset,
    width: f32,
    height: f32,
) -> Option<eframe::egui::TextureHandle> {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    type Cache = Arc<Mutex<BTreeMap<(Asset, u32, u32), Option<eframe::egui::TextureHandle>>>>;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let key = (asset.clone(), width.round().max(1.0) as u32, height.round().max(1.0) as u32);
    let cache: Cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(eframe::egui::Id::unique("c4rizer-icons")).clone());
    let mut cache = cache.lock().ok()?;
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }
    if cache.len() >= 256 {
        cache.clear();
    }
    #[allow(clippy::cast_precision_loss)]
    let texture = icon_image(asset, key.1 as f32, key.2 as f32).map(|image| {
        ctx.load_texture(format!("icon-{}x{}-{}", key.1, key.2, asset.path.len()), image, eframe::egui::TextureOptions::LINEAR)
    });
    cache.insert(key, texture.clone());
    texture
}

/// The egui family name of the bundled bold face.
pub(crate) const BOLD_FAMILY: &str = "Inter Bold";

/// The `state::Input`s of this frame over the canvas `response`: pointer
/// down, move, up (relative to the canvas's top-left), leave, wheel, and
/// the keys of `state::Key` with modifiers; `text_focus` true when an
/// egui text field has focus. Serves EDIT-9, EDIT-28.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one flat match over egui's events")]
pub fn inputs(ctx: &eframe::egui::Context, response: &eframe::egui::Response) -> Vec<Input> {
    use eframe::egui::{Event as E, Key as K, MouseWheelUnit, PointerButton};
    let rect = response.rect;
    let text_focus = ctx.egui_wants_keyboard_input();
    let rel = |p: eframe::egui::Pos2| ScreenPoint {
        x: p.x - rect.min.x,
        y: p.y - rect.min.y,
    };
    let mods = |m: eframe::egui::Modifiers| Modifiers {
        shift: m.shift,
        ctrl: m.command || m.ctrl,
        alt: m.alt,
    };
    let hovered = response.hovered() || response.contains_pointer();
    let mut out = Vec::new();
    ctx.input(|i| {
        for event in &i.events {
            match event {
                E::PointerMoved(p) => {
                    // P2-712: off the canvas, or over a panel or overlay
                    // drawn above it, nothing on the canvas is hovered; a
                    // drag keeps its moves wherever the pointer goes.
                    if (rect.contains(*p) && hovered) || i.pointer.any_down() {
                        out.push(Input::PointerMove { at: rel(*p) });
                    } else if out.last() != Some(&Input::PointerLeft) {
                        out.push(Input::PointerLeft);
                    }
                }
                E::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } => {
                    let button = match button {
                        PointerButton::Primary => Button::Left,
                        PointerButton::Middle => Button::Middle,
                        PointerButton::Secondary => Button::Right,
                        _ => continue,
                    };
                    if *pressed {
                        if rect.contains(*pos) && hovered {
                            out.push(Input::PointerDown {
                                at: rel(*pos),
                                button,
                                mods: mods(*modifiers),
                            });
                        }
                    } else {
                        out.push(Input::PointerUp { at: rel(*pos), button });
                    }
                }
                E::PointerGone => out.push(Input::PointerLeft),
                E::MouseWheel { unit, delta, .. } => {
                    if let Some(p) = i.pointer.latest_pos().filter(|p| rect.contains(*p)) {
                        // DECISION: a line or page counts as one notch, 50
                        // points of a touchpad's smooth scroll as one.
                        let notches = match unit {
                            MouseWheelUnit::Point => delta.y / 50.0,
                            MouseWheelUnit::Line | MouseWheelUnit::Page => delta.y,
                        };
                        if notches != 0.0 {
                            out.push(Input::Wheel { at: rel(p), notches });
                        }
                    }
                }
                E::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } => {
                    let key = match key {
                        K::Enter => Key::Enter,
                        K::F2 => Key::F2,
                        K::Tab => Key::Tab,
                        K::Escape => Key::Escape,
                        K::Backspace => Key::Backspace,
                        K::ArrowLeft => Key::ArrowLeft,
                        K::Space => Key::Space,
                        K::Z => Key::Z,
                        K::Y => Key::Y,
                        // P2-705..P2-729: the viewer and editor keys,
                        // one-to-one; `=` is `+` (Structurizr binds both).
                        K::ArrowUp => Key::ArrowUp,
                        K::ArrowRight => Key::ArrowRight,
                        K::ArrowDown => Key::ArrowDown,
                        K::Plus | K::Equals => Key::Plus,
                        K::Minus => Key::Minus,
                        K::B => Key::B,
                        K::C => Key::C,
                        K::D => Key::D,
                        K::G => Key::G,
                        K::H => Key::H,
                        K::I => Key::I,
                        K::J => Key::J,
                        K::L => Key::L,
                        K::M => Key::M,
                        K::N => Key::N,
                        K::P => Key::P,
                        K::R => Key::R,
                        K::T => Key::T,
                        K::V => Key::V,
                        K::W => Key::W,
                        K::F11 => Key::F11,
                        _ => continue,
                    };
                    if *pressed && !*repeat {
                        out.push(Input::KeyDown {
                            key,
                            mods: mods(*modifiers),
                            text_focus,
                        });
                    } else if !*pressed {
                        out.push(Input::KeyUp { key });
                    }
                }
                _ => {}
            }
        }
    });
    // P2-724: egui sends no key event for a modifier, so Alt turning on
    // or off between frames is reported as `Key::Alt`. Read outside
    // `ctx.input`: the context's lock is not re-entrant.
    let (modifiers, pointer) = ctx.input(|i| (i.modifiers, i.pointer.interact_pos()));
    let id = eframe::egui::Id::unique("c4rizer-alt-held");
    let was = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if modifiers.alt != was {
        ctx.data_mut(|d| d.insert_temp(id, modifiers.alt));
        if modifiers.alt {
            out.push(Input::KeyDown {
                key: Key::Alt,
                mods: mods(modifiers),
                text_focus,
            });
        } else {
            out.push(Input::KeyUp { key: Key::Alt });
        }
    }
    // P2-719, P2-720: a double click over the canvas.
    if response.double_clicked()
        && let Some(p) = pointer
    {
        out.push(Input::DoubleClick {
            at: rel(p),
            mods: mods(modifiers),
        });
    }
    out
}

/// Registers the bundled font (`scene::FONT_REGULAR` and
/// `scene::FONT_BOLD`, CORE-30, HLD §6) with egui as the proportional
/// family, regular and bold, so the canvas paints with the same face the
/// scene measured and the SVG names. No fallback face is part of this
/// contract. Serves EDIT-12, CORE-30.
pub fn install_fonts(ctx: &eframe::egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(REGULAR_NAME.to_owned(), Arc::new(FontData::from_static(FONT_REGULAR)));
    fonts
        .font_data
        .insert(BOLD_NAME.to_owned(), Arc::new(FontData::from_static(FONT_BOLD)));
    fonts
        .families
        .insert(FontFamily::Proportional, vec![REGULAR_NAME.to_owned()]);
    fonts
        .families
        .insert(FontFamily::Name(BOLD_FAMILY.into()), vec![BOLD_NAME.to_owned()]);
    ctx.set_fonts(fonts);
}

/// The egui font-data name of the bundled regular face.
const REGULAR_NAME: &str = "Inter-Regular";
/// The egui font-data name of the bundled bold face.
const BOLD_NAME: &str = "Inter-Bold";

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use c4rizer_core::model::parse;
    use c4rizer_core::pipeline::build_view;
    use c4rizer_core::place::Keep;
    use c4rizer_core::scene::{ItemKey, Scene};
    use c4rizer_core::view::BoxId;

    use super::{PaintItem, Primitive, Source, install_fonts, paint_items};
    use crate::camera::{Camera, ScreenPoint};
    use crate::state::{DrawList, Mark, MarkKind};
    use crate::testutil::SHOP;

    fn scene(key: &str) -> Scene {
        let parsed = parse(SHOP).expect("fixture parses");
        build_view(&parsed.model, key, &Keep::default(), &AtomicBool::new(false))
            .expect("builds")
            .scene
    }

    fn list(scene: &Scene) -> DrawList<'_> {
        DrawList {
            scene: Some(scene),
            camera: Camera::default(),
            marks: Vec::new(),
            rubber_band: None,
            resize_outline: None,
            editing: None,
            read_only: false,
            hide_descriptions: false,
            hide_metadata: false,
            hide_key: false,
            faded: Vec::new(),
            tooltip: None,
        }
    }

    /// Paint-order rank of a source (scene.rs header), marks after.
    fn rank(source: Source) -> u8 {
        match source {
            Source::Scene(ItemKey::Boundary(_)) => 0,
            Source::Scene(ItemKey::Line(_)) => 1,
            Source::Scene(ItemKey::Box(_)) => 2,
            Source::Scene(ItemKey::Label(_)) => 3,
            Source::Scene(ItemKey::Title) => 4,
            Source::Scene(ItemKey::Subtitle) => 5,
            Source::Scene(ItemKey::Legend) => 6,
            Source::Scene(_) => 7,
            Source::Mark(..) => 8,
            Source::ResizeOutline => 9,
            Source::RubberBand => 10,
        }
    }

    /// Every scene item of `scene`.
    fn scene_items(scene: &Scene) -> Vec<ItemKey> {
        let mut keys: Vec<ItemKey> = Vec::new();
        keys.extend((0..scene.boundaries.len()).map(|i| ItemKey::Boundary(u32::try_from(i).unwrap())));
        keys.extend(scene.lines.iter().map(|l| ItemKey::Line(l.edge)));
        keys.extend(scene.boxes.iter().map(|b| ItemKey::Box(b.id)));
        keys.extend(scene.labels.iter().map(|l| ItemKey::Label(l.edge)));
        if scene.title.is_some() {
            keys.push(ItemKey::Title);
        }
        if scene.subtitle.is_some() {
            keys.push(ItemKey::Subtitle);
        }
        if !scene.legend.entries.is_empty() {
            keys.push(ItemKey::Legend);
        }
        keys
    }

    /// EDIT-12 (plan editor-02): every `PaintItem` comes from a scene item
    /// (`Source::Scene`, one or more per box, line, label, boundary,
    /// title, subtitle, key) or a mark; every scene item is painted; the
    /// scene items are in the scene's paint order.
    #[test]
    fn edit_12_canvas_items_match_scene() {
        for key in ["context", "containers"] {
            let scene = scene(key);
            let mut l = list(&scene);
            l.marks.push(Mark {
                item: ItemKey::Box(BoxId(0)),
                kind: MarkKind::SelectedWithHandles,
            });
            let items = paint_items(&l);
            let expected = scene_items(&scene);
            for item in &items {
                match item.source {
                    Source::Scene(k) => assert!(expected.contains(&k), "{k:?} not in the scene"),
                    Source::Mark(k, _) => assert_eq!(k, ItemKey::Box(BoxId(0))),
                    other => panic!("unexpected source {other:?}"),
                }
            }
            for k in &expected {
                assert!(items.iter().any(|i| i.source == Source::Scene(*k)), "{k:?} not painted");
            }
            let ranks: Vec<u8> = items.iter().map(|i| rank(i.source)).collect();
            assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "paint order: {ranks:?}");
            // Each box's text lines are painted as the scene wrapped them.
            let b = &scene.boxes[0];
            for line in &b.name.lines {
                assert!(items.iter().any(|i| i.source == Source::Scene(ItemKey::Box(b.id))
                    && matches!(&i.primitive, Primitive::Text { text, bold: true, .. } if text == line)));
            }
        }
    }

    /// EDIT-12: a draw list without scene paints nothing.
    #[test]
    fn edit_12_no_scene_paints_nothing() {
        let scene = scene("context");
        let mut l = list(&scene);
        l.scene = None;
        l.marks.push(Mark {
            item: ItemKey::Box(BoxId(0)),
            kind: MarkKind::Selected,
        });
        l.rubber_band = Some((ScreenPoint::default(), ScreenPoint { x: 5.0, y: 5.0 }));
        assert!(paint_items(&l).is_empty());
    }

    /// EDIT-18: while resizing, the outline is painted as
    /// `Source::ResizeOutline` after the marks, and nothing else is
    /// added.
    #[test]
    fn edit_18_resize_outline_painted_as_mark() {
        let scene = scene("containers");
        let mut l = list(&scene);
        l.marks.push(Mark {
            item: ItemKey::Box(BoxId(1)),
            kind: MarkKind::Selected,
        });
        let before = paint_items(&l);
        l.resize_outline = Some((ScreenPoint { x: 10.0, y: 10.0 }, ScreenPoint { x: 90.0, y: 70.0 }));
        let after = paint_items(&l);
        assert_eq!(after.len(), before.len() + 1);
        assert_eq!(&after[..before.len()], &before[..]);
        let last: &PaintItem = after.last().unwrap();
        assert_eq!(last.source, Source::ResizeOutline);
        assert!(matches!(&last.primitive, Primitive::Polygon { points, fill: None, .. } if points.len() == 4));
    }

    /// EDIT-12, CORE-30: after `install_fonts`, egui's proportional
    /// family is the bundled face (`scene::FONT_REGULAR`,
    /// `scene::FONT_BOLD`), no other face.
    #[test]
    fn edit_12_canvas_uses_bundled_font() {
        use eframe::egui::{Context, FontFamily, RawInput};
        let ctx = Context::default();
        install_fonts(&ctx);
        let mut output = ctx.run_ui(RawInput::default(), |_| {});
        output.textures_delta.clear();
        let defs = ctx.fonts(|f| f.definitions().clone());
        let proportional = &defs.families[&FontFamily::Proportional];
        assert_eq!(proportional.len(), 1, "{proportional:?}");
        let bold = &defs.families[&FontFamily::Name(super::BOLD_FAMILY.into())];
        assert_eq!(bold.len(), 1, "{bold:?}");
        let bytes = |name: &str| (*defs.font_data[name].font).as_ref().to_vec();
        assert_eq!(bytes(&proportional[0]), c4rizer_core::scene::FONT_REGULAR);
        assert_eq!(bytes(&bold[0]), c4rizer_core::scene::FONT_BOLD);
    }

    /// The canvas inputs of one frame: a 400x300 canvas at (100, 100) in
    /// a central panel, with an optional window covering its middle.
    fn frame_inputs(ctx: &eframe::egui::Context, events: Vec<eframe::egui::Event>, overlay: bool) -> Vec<crate::state::Input> {
        use eframe::egui::{self, Pos2, RawInput, Rect as ERect, Sense, vec2};
        let input = RawInput {
            screen_rect: Some(ERect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut got = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            let canvas = ERect::from_min_size(Pos2::new(100.0, 100.0), vec2(400.0, 300.0));
            let response = ui.allocate_rect(canvas, Sense::click_and_drag());
            if overlay {
                egui::Area::new(egui::Id::unique("test-overlay"))
                    .fixed_pos(Pos2::new(200.0, 200.0))
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        ui.allocate_exact_size(vec2(200.0, 100.0), Sense::click());
                    });
            }
            got = super::inputs(ui.ctx(), &response);
        });
        output.textures_delta.clear();
        got
    }

    /// Fix round (P2-712): the pointer moving off the canvas is reported
    /// as `PointerLeft`, so the state drops its hover.
    #[test]
    fn p2_712_pointer_moving_off_canvas_reports_left() {
        use eframe::egui::{Event as E, Pos2};
        let ctx = eframe::egui::Context::default();
        // egui hit-tests against the widgets of the frame before.
        let _ = frame_inputs(&ctx, Vec::new(), false);
        let first = frame_inputs(&ctx, vec![E::PointerMoved(Pos2::new(150.0, 150.0))], false);
        assert!(matches!(first.as_slice(), [crate::state::Input::PointerMove { .. }]), "{first:?}");
        let _ = frame_inputs(&ctx, Vec::new(), false);
        let off = frame_inputs(&ctx, vec![E::PointerMoved(Pos2::new(650.0, 150.0))], false);
        assert!(off.contains(&crate::state::Input::PointerLeft), "{off:?}");
    }

    /// Fix round (P2-712): over a window drawn above the canvas, the
    /// canvas is not hovered: `PointerLeft`, not a move.
    #[test]
    fn p2_712_pointer_over_overlay_reports_left() {
        use eframe::egui::{Event as E, Pos2};
        let ctx = eframe::egui::Context::default();
        for _ in 0..3 {
            let _ = frame_inputs(&ctx, vec![E::PointerMoved(Pos2::new(150.0, 150.0))], true);
        }
        let mut over = Vec::new();
        for _ in 0..3 {
            over = frame_inputs(&ctx, vec![E::PointerMoved(Pos2::new(250.0, 250.0))], true);
        }
        assert!(over.contains(&crate::state::Input::PointerLeft), "{over:?}");
        assert!(!over.iter().any(|i| matches!(i, crate::state::Input::PointerMove { .. })), "{over:?}");
    }

    /// Every screen point of a primitive, plus its sizes.
    fn numbers(p: &Primitive) -> Vec<f32> {
        match p {
            Primitive::Polygon { points, stroke, .. } => points
                .iter()
                .flat_map(|q| [q.x, q.y])
                .chain(stroke.map(|s| s.1))
                .collect(),
            Primitive::Path { points, width, .. } => points.iter().flat_map(|q| [q.x, q.y]).chain([*width]).collect(),
            Primitive::Circle { centre, radius, .. } => vec![centre.x, centre.y, *radius],
            Primitive::Text { at, size, .. } => vec![at.x, at.y, *size],
            Primitive::Image { min, max, .. } => vec![min.x, min.y, max.x, max.y],
        }
    }

    /// EDIT-9: zoom 2 doubles every screen distance between painted
    /// points and changes no scene value.
    #[test]
    fn edit_9_zoom_scales_paint_only() {
        let scene = scene("containers");
        let copy = scene.clone();
        let one = paint_items(&list(&scene));
        let mut l = list(&scene);
        l.camera.zoom = 2.0;
        let two = paint_items(&l);
        assert_eq!(one.len(), two.len());
        for (a, b) in one.iter().zip(&two) {
            let (na, nb) = (numbers(&a.primitive), numbers(&b.primitive));
            assert_eq!(na.len(), nb.len());
            for (x, y) in na.iter().zip(&nb) {
                assert!((x * 2.0 - y).abs() < 1e-3, "{x} vs {y} in {:?}", a.source);
            }
        }
        assert_eq!(scene, copy);
    }

    /// EDIT-31: the page is the scene's bounds filled with the export
    /// background, through the camera; no scene, no page.
    #[test]
    fn edit_31_page_is_bounds_in_export_background() {
        let scene = scene("containers");
        let mut l = list(&scene);
        l.camera.zoom = 0.5;
        l.camera.pan = crate::camera::ScenePoint { x: -40.0, y: 10.0 };
        let page = super::page(&l).expect("a page");
        let b = scene.bounds;
        let (a, z) = (l.camera.to_screen(b.min), l.camera.to_screen(b.max));
        let bg = scene.background;
        match page {
            Primitive::Polygon {
                points,
                fill,
                stroke,
                dash,
            } => {
                assert_eq!(fill, Some([bg.r, bg.g, bg.b]));
                assert_eq!((stroke, dash), (None, None));
                let xs: Vec<f32> = points.iter().map(|p| p.x).collect();
                let ys: Vec<f32> = points.iter().map(|p| p.y).collect();
                assert!((xs.iter().copied().fold(f32::INFINITY, f32::min) - a.x).abs() < 1e-3);
                assert!((xs.iter().copied().fold(f32::NEG_INFINITY, f32::max) - z.x).abs() < 1e-3);
                assert!((ys.iter().copied().fold(f32::INFINITY, f32::min) - a.y).abs() < 1e-3);
                assert!((ys.iter().copied().fold(f32::NEG_INFINITY, f32::max) - z.y).abs() < 1e-3);
            }
            other => panic!("not a polygon: {other:?}"),
        }
        l.scene = None;
        assert!(super::page(&l).is_none());
    }

    /// WCAG relative luminance of an sRGB colour.
    fn luminance(c: [u8; 3]) -> f32 {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    }

    fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// EDIT-31: every mark colour (selection, hover, enterable hover,
    /// handles, resize outline, rubber band) stands out from the page by
    /// at least 4.5:1, so 2px marks stay visible on the white page.
    #[test]
    fn edit_31_marks_contrast_with_page() {
        let scene = scene("containers");
        let bg = scene.background;
        let page = [bg.r, bg.g, bg.b];
        for (name, colour) in [
            ("selection", super::SELECTION),
            ("hover", super::HOVER),
            ("enterable", super::ENTERABLE),
        ] {
            let ratio = contrast(colour, page);
            assert!(ratio >= 4.5, "{name} {colour:?} on {page:?}: {ratio:.2}");
        }
    }
}
