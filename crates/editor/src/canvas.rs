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
use c4rizer_core::model::{Asset, Colour, Mode, Model, ViewKind, ViewSpec};
use c4rizer_core::scene::{
    Align, FONT_BOLD, FONT_REGULAR, ItemKey, Outline, Scene, Stroke, Swatch, TextBlock, Tip, WindowKind,
};
use c4rizer_core::view::EdgeId;

use crate::camera::{Camera, ScreenPoint};
use crate::state::{Button, DrawList, Input, Key, MarkKind, Modifiers};

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
    /// An alignment guide of the box drag under way (a mark, P5-507).
    // PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-18, #8).
    Guide,
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
        /// True for the chrome's mono face (Geist Mono, tracked 0.06em),
        /// which only marks use; diagram text is always Inter.
        mono: bool,
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
/// the alignment guides (P5-507), then the rubber band. Nothing that is not from `list.scene` or a mark
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
                mono: false,
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
    let paper = rgb(scene.background);
    for l in &scene.lines {
        let source = Source::Scene(ItemKey::Line(l.edge));
        opacity.set(l.opacity);
        let colour = rgb(l.stroke.colour);
        let written = list.written_thickness.contains(&l.edge);
        let width = edge_width(l.stroke.width_px, written) * cam.zoom;
        // P4-523: a bridged line's runs and T bars, as the SVG draws them
        // (P4-519).
        let whole = [l.points.clone()];
        let runs: &[Vec<Point>] = if l.runs.is_empty() { &whole } else { &l.runs };
        for points in route_paths(runs, &l.jumps, &l.stroke, &l.tip, cam) {
            push(source, Primitive::Path {
                points,
                colour,
                width,
                dash: dash(&l.stroke, cam),
            });
        }
        for bar in &l.bars {
            push(source, Primitive::Path {
                points: bar.iter().map(|&p| cam.to_screen(p)).collect(),
                colour,
                width,
                dash: None,
            });
        }
        // Chevrons and the tip stay solid on a dashed line (P2-420).
        for c in &l.chevrons {
            push(source, Primitive::Path {
                points: c.points.iter().map(|&p| cam.to_screen(p)).collect(),
                colour,
                width,
                dash: None,
            });
        }
        push(source, head(&l.tip, l.stroke.width_px, colour, cam));
        if let Some(&from) = l.points.first() {
            for primitive in port(from, edge_width(l.stroke.width_px, written), colour, paper, cam) {
                push(source, primitive);
            }
        }
    }
    for b in &scene.boxes {
        let source = Source::Scene(ItemKey::Box(b.id));
        opacity.set(b.opacity);
        // redesign 2026-11-19: a cylinder's lid line in the text colour at
        // 45% over the fill, as the SVG.
        let deco = Deco {
            paper,
            lid: Some(over(rgb(b.name.colour), LID_OPACITY, rgb(b.fill))),
        };
        for primitive in outline(&b.outline, Some(rgb(b.fill)), Some(&b.stroke), cam, deco) {
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
        let tech_colour = label_tech_colour(rgb(l.text.colour), scene.mode);
        // Design-system Edge "Step", as the SVG draws it (core export's
        // `step_disc`, `step_layout`): in a dynamic view a label's first
        // line `<number>: <text>` draws the number in a disc in its line's
        // colour, and every line is centred in the column right of it.
        let step = (scene.kind == ViewKind::Dynamic)
            .then(|| lines.first().and_then(|first| step_number(first)))
            .flatten();
        let column = step.map(|_| step_layout(&l.text));
        let shown: Vec<Primitive> = text_items(&l.text, cam)
            .into_iter()
            .enumerate()
            .filter(|&(i, _)| {
                let is_tech = tech && i + 1 == lines.len();
                !((is_tech && list.hide_metadata) || (!is_tech && list.hide_descriptions))
            })
            .map(|(i, mut t)| {
                if let (Some((_, column_x)), Primitive::Text { at, text, .. }) = (column, &mut t) {
                    *at = cam.to_screen(Point::new(column_x, l.text.at.y + i32::try_from(i).unwrap_or(0) * l.text.line_px));
                    if i == 0
                        && let Some((_, rest)) = step
                    {
                        rest.clone_into(text);
                    }
                }
                // redesign 2026-11-19: the technology line in `dg-ink-2`,
                // as the SVG (one line labels have no technology line).
                if tech
                    && i > 0
                    && i + 1 == lines.len()
                    && let Primitive::Text { colour, .. } = &mut t
                {
                    *colour = tech_colour;
                }
                t
            })
            .collect();
        // A label with every line hidden paints no background either: its
        // page-coloured box would cut a gap in the line (seen in the real
        // window).
        if let Some(fill) = l.fill
            && !shown.is_empty()
        {
            // redesign 2026-11-19: a plate 6x4px around the text, 3px
            // corners, as the SVG.
            push(source, polygon(rect_points(cam, label_plate(&l.text), PLATE_RADIUS_PX), Some(rgb(fill)), None, cam));
        }
        // P4-317: a callout's shelf, leader and dot, as the SVG draws them
        // (P4-316); not painted when every text line is hidden.
        if let Some(c) = &l.callout
            && !shown.is_empty()
        {
            let colour = rgb(c.stroke.colour);
            for seg in [c.shelf, c.leader] {
                push(source, Primitive::Path {
                    points: seg.iter().map(|&p| cam.to_screen(p)).collect(),
                    colour,
                    width: px(c.stroke.width_px) * cam.zoom,
                    dash: None,
                });
            }
            push(source, tip(&Tip::Dot { at: c.dot, radius_px: c.dot_radius_px }, colour, cam));
        }
        // The disc belongs to the description line: hidden with it.
        if let (Some((number, _)), Some((centre, _))) = (step, column)
            && !list.hide_descriptions
        {
            let colour = scene.lines.iter().find(|line| line.edge == l.edge).map_or(rgb(l.text.colour), |line| rgb(line.stroke.colour));
            push(source, tip(&Tip::Dot { at: centre, radius_px: STEP_DISC_RADIUS_PX }, colour, cam));
            push(source, Primitive::Text {
                at: cam.to_screen(Point::new(centre.x, centre.y + STEP_NUMBER_DROP_PX)),
                text: number.to_owned(),
                size: px(STEP_NUMBER_FONT_PX) * cam.zoom,
                bold: true,
                centred: true,
                colour: rgb(scene.background),
                mono: false,
            });
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
                for primitive in outline(o, Some(rgb(*fill)), Some(stroke), cam, Deco { paper, lid: None }) {
                    push(source, primitive);
                }
            }
            Swatch::Line { from, to, stroke, tip: t } => {
                let colour = rgb(stroke.colour);
                for points in route_paths(&[vec![*from, *to]], &[], stroke, t, cam) {
                    push(source, Primitive::Path {
                        points,
                        colour,
                        width: edge_width(stroke.width_px, false) * cam.zoom,
                        dash: dash(stroke, cam),
                    });
                }
                push(source, head(t, stroke.width_px, colour, cam));
                for primitive in port(*from, edge_width(stroke.width_px, false), colour, paper, cam) {
                    push(source, primitive);
                }
            }
            _ => {}
        }
        for t in text_items(&entry.text, cam) {
            push(source, t);
        }
    }

    let mk = mark_colours(scene.mode);
    for mark in &list.marks {
        let Some(rect) = scene.item_bounds(mark.item) else {
            continue;
        };
        let source = Source::Mark(mark.item, mark.kind);
        // redesign 2026-11-19 (behaviour change 4): a line's mark is a
        // halo that follows its route, 9px, `mark` at 30%; a bus peer's
        // (P4-426) the same, fainter; a hovered line's in `mark-hint`.
        if let ItemKey::Line(e) = mark.item
            && let Some(l) = scene.lines.iter().find(|l| l.edge == e)
        {
            let (colour, alpha) = match mark.kind {
                MarkKind::BusPeer => (mk.mark, BUS_HALO_OPACITY),
                MarkKind::Hover | MarkKind::Enterable => (mk.hint, HALO_OPACITY),
                _ => (mk.mark, HALO_OPACITY),
            };
            // The halo follows the whole route (`points`, the hit-test
            // route, P4-518), bridges ignored.
            opacity.set(alpha);
            push(source, Primitive::Path {
                points: sample(&edge_path(&l.points, &[], 0.0), cam),
                colour,
                width: HALO_PX * cam.zoom,
                dash: None,
            });
            opacity.set(100);
            continue;
        }
        if mark.kind == MarkKind::BusPeer {
            continue;
        }
        let (colour, width, dashed) = match mark.kind {
            MarkKind::Hover => (mk.hint, HOVER_WIDTH, None),
            MarkKind::Enterable => (mk.mark, MARK_WIDTH, Some([5.0, 4.0])),
            _ => (mk.mark, MARK_WIDTH, None),
        };
        push(source, Primitive::Polygon {
            points: rect_points(cam, rect.inflate(MARK_GAP_PX), 0),
            fill: None,
            stroke: Some((colour, width)),
            dash: dashed,
        });
        if mark.kind == MarkKind::Enterable {
            let outline = rect.inflate(MARK_GAP_PX);
            for p in enter_tab(cam.to_screen(Point::new(outline.max.x, outline.min.y)), mk) {
                push(source, p);
            }
        }
        if mark.kind == MarkKind::SelectedWithHandles && !list.read_only {
            // P2-735: a handle over a line end is drawn outside the edge.
            let ends = crate::state::line_ends(scene, cam);
            for (_, centre) in crate::state::handle_spots(rect, cam, &ends) {
                push(source, square(centre, HANDLE_DRAW_HALF, mk));
            }
        }
    }
    if let Some((a, b)) = list.resize_outline {
        push(Source::ResizeOutline, screen_rect(a, b, mk.mark, MARK_WIDTH, None));
    }
    // P5-507: the alignment guides, 1px solid in the mark colour, with
    // the marks and under the rubber band.
    for g in list.guides {
        let screen = |x: f32, y: f32| ScreenPoint {
            x: (x - cam.pan.x) * cam.zoom,
            y: (y - cam.pan.y) * cam.zoom,
        };
        let (a, b) = if g.vertical { (screen(g.at, g.from), screen(g.at, g.to)) } else { (screen(g.from, g.at), screen(g.to, g.at)) };
        push(Source::Guide, Primitive::Path {
            points: vec![a, b],
            colour: mk.mark,
            width: GUIDE_WIDTH,
            dash: None,
        });
    }
    if let Some((a, b)) = list.rubber_band {
        // Marks: 1px dashed outline, 10% fill (the fill is its own item so
        // the outline stays whole).
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        let q = |x: f32, y: f32| ScreenPoint { x, y };
        opacity.set(BAND_FILL_OPACITY);
        push(Source::RubberBand, Primitive::Polygon {
            points: vec![q(x0, y0), q(x1, y0), q(x1, y1), q(x0, y1)],
            fill: Some(mk.mark),
            stroke: None,
            dash: None,
        });
        opacity.set(100);
        push(Source::RubberBand, screen_rect(a, b, mk.mark, 1.0, Some([4.0, 3.0])));
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
    let mk = mark_colours(list.scene.map_or(Mode::Light, |s| s.mode));
    handles
        .iter()
        .map(|&centre| PaintItem {
            source: Source::Mark(item, MarkKind::Selected),
            primitive: square(centre, HANDLE_DRAW_HALF, mk),
            opacity: 100,
        })
        .collect()
}

/// The end handles of the selected line (P4-213), round so they read
/// apart from the square vertex handles, and during an end drag the snap
/// points of the dragged end's box (P4-214): small rings, the snapped one
/// large and filled. `ends` and `snaps` come from `State::end_handles`
/// and `State::snap_marks`.
pub(crate) fn anchor_items(list: &DrawList<'_>, ends: &[ScreenPoint], snaps: &[crate::state::SnapMark]) -> Vec<PaintItem> {
    let Some(item) = list.marks.iter().map(|m| m.item).find(|i| matches!(i, ItemKey::Line(_))) else {
        return Vec::new();
    };
    let mk = mark_colours(list.scene.map_or(Mode::Light, |s| s.mode));
    // A ring of `rim` around a dot of `fill` (Marks: handles and idle snap
    // points are `mark-handle` ringed 1.5px, the taken one fills with
    // `mark`).
    let dot = |centre: ScreenPoint, radius: f32, rim: [u8; 3], fill: [u8; 3]| {
        [
            PaintItem {
                source: Source::Mark(item, MarkKind::Selected),
                primitive: Primitive::Circle {
                    centre,
                    radius: radius + 1.5,
                    fill: rim,
                },
                opacity: 100,
            },
            PaintItem {
                source: Source::Mark(item, MarkKind::Selected),
                primitive: Primitive::Circle { centre, radius, fill },
                opacity: 100,
            },
        ]
    };
    let mut out = Vec::new();
    for m in snaps.iter().filter(|m| !m.snapped) {
        out.extend(dot(m.at, SNAP_RADIUS, mk.hint, mk.handle));
    }
    for &e in ends {
        out.extend(dot(e, END_RADIUS, mk.mark, mk.handle));
    }
    for m in snaps.iter().filter(|m| m.snapped) {
        out.extend(dot(m.at, END_RADIUS + 2.0, mk.mark, mk.mark));
    }
    out
}

/// Radius of an end handle, screen px (P4-213).
const END_RADIUS: f32 = 3.5;
/// Radius of a snap point, screen px (P4-214).
const SNAP_RADIUS: f32 = 3.0;

/// The editor marks' colours in one diagram mode (design-system Marks:
/// `mark`, `mark-handle`, `mark-hint`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MarkColours {
    /// Selection outline, handles' rim, halo, rubber band, enter mark.
    mark: [u8; 3],
    /// Fill of handles and snap points.
    handle: [u8; 3],
    /// Plain hover outline and idle snap points.
    hint: [u8; 3],
}

/// The mark colours of `mode`.
// redesign 2026-11-19: the design tokens replace the blue/grey/green
// marks; `mark` is the only purple on the canvas. Light: 5.2:1 on paper,
// dark: the chrome accent; `mark-hint` 3.8:1 (a 1px hover hint, over the
// 3:1 floor for graphics, EDIT-31).
const fn mark_colours(mode: Mode) -> MarkColours {
    match mode {
        Mode::Dark => MarkColours {
            mark: [0xa9, 0x9c, 0xf5],
            handle: [0x1e, 0x21, 0x24],
            hint: [0x73, 0x7c, 0x83],
        },
        _ => MarkColours {
            mark: [0x6a, 0x55, 0xe3],
            handle: [0xff, 0xff, 0xff],
            hint: [0x7b, 0x84, 0x8b],
        },
    }
}

/// Selection outline width, screen px (Marks: 1.5px).
const MARK_WIDTH: f32 = 1.5;
/// Hover outline width, screen px (Marks: 1px `mark-hint`).
const HOVER_WIDTH: f32 = 1.0;
/// Alignment guide width, screen px (P5-507: 1px solid).
const GUIDE_WIDTH: f32 = 1.0;
/// Half the side of a drawn box or vertex handle, screen px (Marks: 7px
/// handles). `state::HANDLE_PX` stays the hit size.
const HANDLE_DRAW_HALF: f32 = 3.5;
/// Width of a line's halo, scene px (behaviour change 4: 9px).
const HALO_PX: f32 = 9.0;
/// Opacity of a selected line's halo, 0..100 (`mark` at 30%).
const HALO_OPACITY: u8 = 30;
/// Opacity of a bus peer's halo, 0..100 (P4-426: fainter than the
/// selection's; `.mk-halo--faint`).
const BUS_HALO_OPACITY: u8 = 14;
/// Opacity of the rubber band's fill, 0..100.
const BAND_FILL_OPACITY: u8 = 10;
/// Gap between an item and its mark, scene px.
// DECISION: 3px (not the Marks sheet's 4px) so the outline runs through
// the handles, which `state::handle_spots` centres on the box's own
// corners and edges for the hit test.
const MARK_GAP_PX: i32 = 3;
/// Inter's ascender in em (hhea ascender 2728 / 2816): from a baseline to
/// the top egui places text at.
// DECISION: a constant of the bundled face rather than a font query, so
// `paint` needs no font lookup per text line.
const ASCENT_EM: f32 = 0.969;
/// The tracking of a mark's mono label, em (Marks `.mk-enter-t`).
const MONO_TRACKING_EM: f32 = 0.06;

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
fn outline(o: &Outline, fill: Option<[u8; 3]>, stroke: Option<&Stroke>, cam: &Camera, deco: Deco) -> Vec<Primitive> {
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
            let mut out = vec![polygon(rect_points(cam, body, body_radius_px), fill, stroke, cam)];
            // redesign 2026-11-19: the head cut from the body by a 2.5px
            // paper ring, as the SVG: on boxes only (a lid colour is given
            // for boxes), not on key swatches.
            if deco.lid.is_some() {
                out.push(Primitive::Circle {
                    centre: cam.to_screen(head_centre),
                    radius: (r + HEAD_RING_PX) * cam.zoom,
                    fill: deco.paper,
                });
            }
            out.push(polygon(head, fill, stroke, cam));
            out
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
                    colour: deco.lid.unwrap_or_else(|| rgb(s.colour)),
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

// ------------------------------------------------ edge drawing geometry
//
// redesign 2026-11-19: the same numbers and steps as core's export.rs
// (`edge_width`, `edge_path`, `notched_head`, `label_plate`), so the
// canvas keeps drawing what the export draws (EDIT-12). Keep in step.

/// Stroke width of a line `width_px` wide: the default 2px is 1.5px; a
/// thickness a style wrote (`written`) draws as written, 2px included.
fn edge_width(width_px: i32, written: bool) -> f32 {
    if width_px == 2 && !written { 1.5 } else { px(width_px) }
}

/// The tag core's renderer adds to a dynamic view's response step for
/// its style lookup (P3-201).
// DECISION: repeated here, core's constant is private; the editor finds
// a written thickness through the model's pub relationship styles only.
const RESPONSE_TAG: &str = "Relationship/Response";

/// The lines of `scene` whose thickness a relationship style wrote: a
/// block in `model.relationship_styles` with `thickness`, for one of the
/// winner's tags (plus `Relationship/Response` on a dynamic view's
/// response step), in the scene's mode. The SVG reads the same fact from
/// the plan (EDIT-12); these lines draw a written 2px at 2px.
// DECISION: any applying block counts, theme blocks included, as core's
// `Model::relationship_thickness_written_in`; which block wins does not
// matter, only that the width did not come from the default.
pub(crate) fn thickness_written(model: &Model, scene: &Scene) -> Vec<EdgeId> {
    let spec = (scene.kind == ViewKind::Dynamic).then(|| model.view(&scene.key)).flatten();
    let drawn = spec.map(ViewSpec::drawn_steps).unwrap_or_default();
    scene
        .lines
        .iter()
        .filter(|l| {
            let response = spec.is_some_and(|v| {
                drawn
                    .get(l.edge.0 as usize)
                    .and_then(|&i| v.steps.get(i))
                    .is_some_and(|s| s.response)
            });
            let tags = &model.relationship(l.relationship).tags;
            let extra = response.then_some(RESPONSE_TAG);
            tags.iter().map(String::as_str).chain(extra).any(|tag| {
                model
                    .relationship_styles
                    .iter()
                    .any(|s| s.tag.value == tag && s.mode.is_none_or(|m| m == scene.mode) && s.thickness.is_some())
            })
        })
        .map(|l| l.edge)
        .collect()
}

/// Corner radius of a turn, at most half the shorter leg.
const CORNER_PX: f32 = 10.0;
/// Hop radius, scene px (P2-422; the SVG's arc).
const HOP_PX: f32 = 6.0;
/// Arrowhead length, half width, notch; scaled by thickness/2.
const HEAD_LEN_PX: f32 = 9.0;
const HEAD_HALF_PX: f32 = 3.5;
const HEAD_NOTCH_PX: f32 = 2.5;
const HEAD_OVERLAP_PX: f32 = 0.5;
/// Port dot radius.
const PORT_RADIUS_PX: f32 = 2.5;
/// Paper ring between a person's head and body.
const HEAD_RING_PX: f32 = 2.5;
/// Label plate padding and corners.
const PLATE_PAD_X_PX: i32 = 6;
const PLATE_PAD_Y_PX: i32 = 4;
const PLATE_RADIUS_PX: i32 = 3;
/// A cylinder lid's line: the text colour at this opacity over the fill.
const LID_OPACITY: u8 = 45;
/// Segments per rounded corner.
const QUAD_STEPS: u8 = 6;

/// What an outline needs besides fill and stroke: the paper (a person's
/// head ring) and a cylinder's lid colour (else the stroke's).
#[derive(Clone, Copy)]
struct Deco {
    paper: [u8; 3],
    lid: Option<[u8; 3]>,
}

/// The scale of a line's head: thickness/2, never below 1 (P2-418).
fn head_scale(width_px: i32) -> f32 {
    px(width_px.max(2)) / 2.0
}

/// The unit vector from `a` to `b`, zero when they meet.
fn unit(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    if len < 1e-6 { (0.0, 0.0) } else { (dx / len, dy / len) }
}

/// One step of an edge's path, scene px (export.rs `Seg`).
#[derive(Clone, Copy)]
enum Seg {
    Move((f32, f32)),
    Line((f32, f32)),
    Quad((f32, f32), (f32, f32)),
    Hop((f32, f32), (f32, f32)),
}

/// The path of a run as export.rs's `edge_path`: last point pulled back
/// `trim`, turns rounded, a hop (centre, end) at each jump.
fn edge_path(points: &[Point], jumps: &[Point], trim: f32) -> Vec<Seg> {
    let mut pts: Vec<(f32, f32)> = points.iter().map(|p| (px(p.x), px(p.y))).collect();
    let n = pts.len();
    if n == 0 {
        return Vec::new();
    }
    if n >= 2 && trim > 0.0 {
        let (a, b) = (pts[n - 2], pts[n - 1]);
        let back = trim.min((b.0 - a.0).hypot(b.1 - a.1));
        let (dx, dy) = unit(a, b);
        pts[n - 1] = (b.0 - dx * back, b.1 - dy * back);
    }
    let dist = |a: (f32, f32), b: (f32, f32)| (b.0 - a.0).hypot(b.1 - a.1);
    let radius: Vec<f32> = (0..n)
        .map(|i| {
            if i == 0 || i + 1 == n {
                return 0.0;
            }
            let (u, v) = (unit(pts[i - 1], pts[i]), unit(pts[i], pts[i + 1]));
            if (u.0 * v.1 - u.1 * v.0).abs() < 1e-6 {
                return 0.0;
            }
            CORNER_PX.min(dist(pts[i - 1], pts[i]) / 2.0).min(dist(pts[i], pts[i + 1]) / 2.0)
        })
        .collect();
    let mut out = vec![Seg::Move(pts[0])];
    for i in 1..n {
        let (a, b) = (pts[i - 1], pts[i]);
        let (dx, dy) = unit(a, b);
        let mut on: Vec<Point> = jumps.iter().copied().filter(|j| on_segment(points[i - 1], points[i], *j)).collect();
        on.sort_by_key(|j| (j.x - points[i - 1].x).abs() + (j.y - points[i - 1].y).abs());
        for j in on {
            let (jx, jy) = (px(j.x), px(j.y));
            out.push(Seg::Line((jx - dx * HOP_PX, jy - dy * HOP_PX)));
            out.push(Seg::Hop((jx, jy), (jx + dx * HOP_PX, jy + dy * HOP_PX)));
        }
        let r = radius[i];
        out.push(Seg::Line((b.0 - dx * r, b.1 - dy * r)));
        if r > 0.0 {
            let (ex, ey) = unit(b, pts[i + 1]);
            out.push(Seg::Quad(b, (b.0 + ex * r, b.1 + ey * r)));
        }
    }
    out
}

/// `segs` as screen points: corners sampled, hops as the SVG's arc
/// (sweep 1: through the left of the heading, y down).
fn sample(segs: &[Seg], cam: &Camera) -> Vec<ScreenPoint> {
    let mut out = Vec::with_capacity(segs.len() * 4);
    let mut last = (0.0_f32, 0.0_f32);
    for seg in segs {
        match *seg {
            Seg::Move(p) | Seg::Line(p) => {
                out.push(at(cam, p.0, p.1));
                last = p;
            }
            Seg::Quad(ctrl, end) => {
                for k in 1..=QUAD_STEPS {
                    let along = f32::from(k) / f32::from(QUAD_STEPS);
                    let rest = 1.0 - along;
                    let mix = |s: f32, c: f32, e: f32| rest * rest * s + 2.0 * rest * along * c + along * along * e;
                    out.push(at(cam, mix(last.0, ctrl.0, end.0), mix(last.1, ctrl.1, end.1)));
                }
                last = end;
            }
            Seg::Hop(centre, e) => {
                let heading = (e.1 - last.1).atan2(e.0 - last.0).to_degrees();
                out.extend(arc(cam, centre, (HOP_PX, HOP_PX), heading + 180.0, heading + 360.0, 8));
                last = e;
            }
        }
    }
    out
}

/// The stroked paths of a line's runs: the run reaching the arrow's tip
/// stops in its notch.
fn route_paths(runs: &[Vec<Point>], jumps: &[Point], stroke: &Stroke, t: &Tip, cam: &Camera) -> Vec<Vec<ScreenPoint>> {
    runs.iter()
        .map(|run| {
            let trim = match t {
                Tip::Arrow(a) if run.last() == Some(&a[0]) => {
                    (HEAD_LEN_PX - HEAD_NOTCH_PX - HEAD_OVERLAP_PX) * head_scale(stroke.width_px)
                }
                _ => 0.0,
            };
            sample(&edge_path(run, jumps, trim), cam)
        })
        .collect()
}

/// The notched arrowhead on the scene's arrow, or the join dot.
fn head(t: &Tip, width_px: i32, colour: [u8; 3], cam: &Camera) -> Primitive {
    let Tip::Arrow(a) = t else {
        return tip(t, colour, cam);
    };
    let f = |p: Point| (px(p.x), px(p.y));
    let (tp, b1, b2) = (f(a[0]), f(a[1]), f(a[2]));
    let back = (f32::midpoint(b1.0, b2.0), f32::midpoint(b1.1, b2.1));
    let (dx, dy) = unit(back, tp);
    let (nx, ny) = (-dy, dx);
    let k = head_scale(width_px);
    let (len, half, notch) = (HEAD_LEN_PX * k, HEAD_HALF_PX * k, (HEAD_LEN_PX - HEAD_NOTCH_PX) * k);
    // Tip first: egui fans the fill from the first point, which covers
    // this star-shaped head exactly.
    Primitive::Polygon {
        points: vec![
            at(cam, tp.0, tp.1),
            at(cam, tp.0 - dx * len + nx * half, tp.1 - dy * len + ny * half),
            at(cam, tp.0 - dx * notch, tp.1 - dy * notch),
            at(cam, tp.0 - dx * len - nx * half, tp.1 - dy * len - ny * half),
        ],
        fill: Some(colour),
        stroke: None,
        dash: None,
    }
}

/// The port at a line's source: a paper dot ringed in the line's colour,
/// as two filled circles (the ring centred on radius 2.5).
/// `width` is the line's drawn width (`edge_width`), scene px.
fn port(from: Point, width: f32, colour: [u8; 3], paper: [u8; 3], cam: &Camera) -> [Primitive; 2] {
    let half = width / 2.0;
    let centre = cam.to_screen(from);
    [
        Primitive::Circle {
            centre,
            radius: (PORT_RADIUS_PX + half) * cam.zoom,
            fill: colour,
        },
        Primitive::Circle {
            centre,
            radius: (PORT_RADIUS_PX - half).max(0.0) * cam.zoom,
            fill: paper,
        },
    ]
}

/// The plate behind a label's text: its bounds plus the padding.
fn label_plate(text: &TextBlock) -> Rect {
    let b = text.bounds;
    Rect::from_corners(
        Point::new(b.min.x - PLATE_PAD_X_PX, b.min.y - PLATE_PAD_Y_PX),
        Point::new(b.max.x + PLATE_PAD_X_PX, b.max.y + PLATE_PAD_Y_PX),
    )
}

/// The step badge (core `scene::STEP_*`): disc radius, gap to the text
/// column, number size, disc centre over the first baseline, number
/// baseline under the disc centre.
const STEP_DISC_RADIUS_PX: i32 = 8;
const STEP_DISC_GAP_PX: i32 = 4;
const STEP_NUMBER_FONT_PX: i32 = 10;
const STEP_DISC_RISE_PX: i32 = 3;
const STEP_NUMBER_DROP_PX: i32 = 4;

/// A dynamic-view label's first line as step number and rest (core
/// `scene::step_number`): `None` unless it starts with `<number>:`.
// DECISION: the same rule as core, repeated because core keeps it
// crate-private (no new pub item): in a dynamic view every label's first
// line starts with its step's whole `<order>:` (P3-211).
fn step_number(line: &str) -> Option<(&str, &str)> {
    let (head, rest) = line.split_once(' ').unwrap_or((line, ""));
    let number = head.strip_suffix(':')?;
    (!number.is_empty()).then_some((number, rest))
}

/// A step label's disc centre and the x its lines are centred on, scene
/// px (core export's `step_layout`).
fn step_layout(text: &TextBlock) -> (Point, i32) {
    let b = text.bounds;
    let centre = Point::new(b.min.x + STEP_DISC_RADIUS_PX, text.at.y - STEP_DISC_RISE_PX);
    let column = b.min.x + 2 * STEP_DISC_RADIUS_PX + STEP_DISC_GAP_PX;
    (centre, (column + b.max.x).div_euclid(2))
}

/// A label's technology colour: `dg-ink-2` under default `dg-ink` text,
/// else the text's own colour (core `scene::label_tech_colour`).
fn label_tech_colour(text: [u8; 3], mode: Mode) -> [u8; 3] {
    let (ink, ink_2) = match mode {
        Mode::Dark => ([0xe8, 0xea, 0xec], [0x9a, 0xa2, 0xa8]),
        _ => ([0x14, 0x17, 0x1a], [0x5a, 0x62, 0x68]),
    };
    if text == ink { ink_2 } else { text }
}

/// `top` at `opacity`/100 over `under`, each channel rounded to nearest
/// (core export's `over`).
fn over(top: [u8; 3], opacity: u8, under: [u8; 3]) -> [u8; 3] {
    let a = u16::from(opacity.min(100));
    let mix = |t: u8, u: u8| u8::try_from((u16::from(t) * a + u16::from(u) * (100 - a) + 50) / 100).unwrap_or(u8::MAX);
    [mix(top[0], under[0]), mix(top[1], under[1]), mix(top[2], under[2])]
}

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

/// A handle: a square of half side `half` around `centre`, screen px,
/// filled `mark-handle` and ringed 1.5px in `mark` (Marks).
fn square(centre: ScreenPoint, half: f32, mk: MarkColours) -> Primitive {
    let p = |dx: f32, dy: f32| ScreenPoint {
        x: centre.x + dx,
        y: centre.y + dy,
    };
    Primitive::Polygon {
        points: vec![p(-half, -half), p(half, -half), p(half, half), p(-half, half)],
        fill: Some(mk.handle),
        stroke: Some((mk.mark, MARK_WIDTH)),
        dash: None,
    }
}

/// The text of the enter tab (design-system Marks: "a tab that names the
/// gesture"; a box's views are entered from its context menu,
/// editor-ux-02).
const ENTER_TAB_TEXT: &str = "RIGHT-CLICK TO ENTER";
/// The enter tab's size, screen px (Marks preview `.mk-enter-tab`).
const ENTER_TAB_W: f32 = 122.0;
const ENTER_TAB_H: f32 = 15.0;
/// The tab text's size and its baseline below the tab's top, screen px
/// (Marks preview `.mk-enter-t`: 9px mono, baseline 10.5px down).
const ENTER_TAB_FONT: f32 = 9.0;
const ENTER_TAB_BASELINE: f32 = 10.5;

/// The tab over an enterable box's dashed hover outline (design-system
/// Marks, "Hover · has views to enter"): a `mark` block standing on the
/// outline's top edge, flush with its right end at `top_right`, naming
/// the gesture in `mark-handle`. A mark: drawn on hover only, never
/// exported.
// DECISION: in screen px like the other marks' widths and handles, so the
// tab reads the same at any zoom; it may stick out left of a box narrower
// than the tab, as the design's right-aligned tab would.
fn enter_tab(top_right: ScreenPoint, mk: MarkColours) -> [Primitive; 2] {
    let (x1, y1) = (top_right.x, top_right.y);
    let (x0, y0) = (x1 - ENTER_TAB_W, y1 - ENTER_TAB_H);
    let p = |x: f32, y: f32| ScreenPoint { x, y };
    [
        Primitive::Polygon {
            points: vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)],
            fill: Some(mk.mark),
            stroke: None,
            dash: None,
        },
        Primitive::Text {
            at: p(x0 + ENTER_TAB_W / 2.0, y0 + ENTER_TAB_BASELINE),
            text: ENTER_TAB_TEXT.to_owned(),
            size: ENTER_TAB_FONT,
            bold: false,
            centred: true,
            colour: mk.handle,
            mono: true,
        },
    ]
}

/// An outlined rectangle between two screen corners.
fn screen_rect(a: ScreenPoint, b: ScreenPoint, colour: [u8; 3], width: f32, dash: Option<[f32; 2]>) -> Primitive {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    let p = |x: f32, y: f32| ScreenPoint { x, y };
    Primitive::Polygon {
        points: vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)],
        fill: None,
        stroke: Some((colour, width)),
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
            mono: false,
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

/// The scene's bounds and, on a view with a canvas size, its paper
/// (`State::paper_rect`) as the one rectangle that is the page.
// DECISION (editor-ux-01): the page is always ONE rectangle, the
// bounding rectangle of the paper (when the view has a `paperSize`) and
// the scene bounds (the drawing, legend and title included). It equals
// the paper while the drawing sits inside it, and grows only on the
// sides a box sticks out of, by exactly that overhang; on auto it is the
// scene bounds, as before. Before, the paper and the bounds were two
// overlapping rectangles that each moved on their own during a drag.
pub(crate) fn page_rect(scene_bounds: Rect, paper: Option<Rect>) -> Rect {
    let Some(p) = paper else { return scene_bounds };
    Rect::from_corners(
        Point::new(p.min.x.min(scene_bounds.min.x), p.min.y.min(scene_bounds.min.y)),
        Point::new(p.max.x.max(scene_bounds.max.x), p.max.y.max(scene_bounds.max.y)),
    )
}

/// The page under the scene: `page_rect` filled with
/// `scene.background`, the export's background (EDIT-31), so the canvas
/// looks like the exported SVG/PNG whatever the window theme; on a view
/// with a canvas size (P2-701) a thin grey dashed outline follows the
/// same rectangle, so the size shows on screen. Empty without a scene.
// DECISION: core exposes the export background as `Scene::background`
// (the SVG fills `bounds` with it), so the page takes it from there and
// no white is hard-coded here. Kept out of `paint_items` so its
// EDIT-31 contract (scene items and marks only, `Source` unchanged)
// holds; the shell paints it first. The outline is a thin grey dash,
// which reads as an edge and not as a diagram boundary.
pub(crate) fn page(list: &DrawList<'_>, paper: Option<Rect>) -> Vec<Primitive> {
    let Some(scene) = list.scene else { return Vec::new() };
    let points = rect_points(&list.camera, page_rect(scene.bounds, paper), 0);
    let mut out = vec![Primitive::Polygon {
        points: points.clone(),
        fill: Some(rgb(scene.background)),
        stroke: None,
        dash: None,
    }];
    if paper.is_some() {
        out.push(Primitive::Polygon {
            points,
            fill: None,
            stroke: Some(([0x99, 0x99, 0x99], 1.0)),
            dash: Some([6.0, 4.0]),
        });
    }
    out
}

/// Paints the page of `list` (`page`; `paper` is the view's canvas, if
/// it has a size) (P2-701, EDIT-31).
pub(crate) fn paint_page(painter: &eframe::egui::Painter, list: &DrawList<'_>, paper_rect: Option<Rect>) {
    for p in page(list, paper_rect) {
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
            mono,
        } => {
            if *size < 1.0 {
                return;
            }
            if *mono {
                // A mark's label: Geist Mono, tracked 0.06em (Marks
                // `.mk-enter-t`); its baseline on the anchor.
                let format = eframe::egui::TextFormat {
                    font_id: FontId::new(*size, FontFamily::Monospace),
                    color: colour(*c),
                    extra_letter_spacing: MONO_TRACKING_EM * size,
                    ..Default::default()
                };
                let galley = painter.layout_job(eframe::egui::text::LayoutJob::single_section(text.clone(), format));
                let ascent = galley
                    .rows
                    .first()
                    .and_then(|r| r.row.glyphs.first())
                    .map_or(ASCENT_EM * size, |g| g.pos.y);
                let top = Pos2::new(origin.x + anchor.x, origin.y + anchor.y - ascent);
                let align = if *centred { Align2::CENTER_TOP } else { Align2::LEFT_TOP };
                let min = align.anchor_size(top, galley.size()).min;
                painter.galley(min, galley, colour(*c));
                return;
            }
            // Diagram text is always Inter, never the chrome's
            // proportional face (redesign: the chrome is Geist).
            let family = FontFamily::Name(if *bold { BOLD_FAMILY } else { REGULAR_FAMILY }.into());
            let top = Pos2::new(origin.x + anchor.x, origin.y + anchor.y - ASCENT_EM * size);
            let align = if *centred { Align2::CENTER_TOP } else { Align2::LEFT_TOP };
            // While the zoom moves, the text is laid out (and its glyphs
            // rasterized) as at the rung zoom of `raster_zoom` and the
            // galley scaled to the exact size, as egui's own scene zoom does.
            // DECISION: the scene size is recovered rounded to a quarter
            // px, so float noise in `size / zoom` never makes a new font
            // size (a cache miss) at each notch.
            let raster = moving_zoom(painter.ctx())
                .map(|zoom| ((size / zoom) * 4.0).round() / 4.0 * raster_zoom(zoom))
                .filter(|r| *r >= 1.0);
            let Some(raster) = raster else {
                painter.text(top, align, text, FontId::new(*size, family), colour(*c));
                return;
            };
            let galley = painter.layout_no_wrap(text.clone(), FontId::new(raster, family), colour(*c));
            let mut shape = eframe::epaint::TextShape::new(Pos2::ZERO, galley, colour(*c));
            shape.transform(eframe::emath::TSTransform::from_scaling(size / raster));
            shape.pos = align.anchor_size(top, shape.galley.size()).min;
            painter.add(shape);
        }
    }
}

/// How long after the last zoom change text is still drawn from the
/// rungs of `raster_zoom`; the next frame draws it at its exact size.
const ZOOM_SETTLE_S: f64 = 0.25;

/// Rungs per doubling of the zoom in `raster_zoom`.
const RUNGS_PER_OCTAVE: f32 = 2.0;

/// The zoom text is laid out at while the zoom moves: the smallest rung
/// `2^(k / RUNGS_PER_OCTAVE)` at or above `zoom`.
// DECISION: egui lays out and rasterizes every glyph again at each new
// font size (its glyph and galley caches key on the exact size), which is
// what made each wheel notch cost 140-740 ms in a dev build. Rungs on the
// zoom (not on each font size) switch every text at the same notch, so a
// gesture lays out and rasterizes at a few sizes only, once each; half
// an octave, rounded up, scales a galley down by at most 1/sqrt 2, which
// keeps it crisp while it moves.
pub(crate) fn raster_zoom(zoom: f32) -> f32 {
    ((zoom.log2() * RUNGS_PER_OCTAVE).ceil() / RUNGS_PER_OCTAVE).exp2()
}

/// When the zoom last changed, kept in egui's temp data.
#[derive(Clone, Copy)]
struct ZoomMotion {
    /// The zoom of the last frame.
    zoom: f32,
    /// `egui::InputState::time` when it last differed.
    changed_at: f64,
}

/// The id of the `ZoomMotion` in egui's temp data.
fn zoom_motion_id() -> eframe::egui::Id {
    eframe::egui::Id::unique("c4rizer-zoom-motion")
}

/// Records this frame's camera `zoom` before the canvas is painted; while
/// it changed less than `ZOOM_SETTLE_S` ago, text is drawn from the rungs
/// of `raster_zoom` and a repaint is asked for when it settles.
pub(crate) fn track_zoom(ctx: &eframe::egui::Context, zoom: f32) {
    let now = ctx.input(|i| i.time);
    let id = zoom_motion_id();
    let motion = ctx.data_mut(|d| {
        let m = d.get_temp_mut_or_insert_with(id, || ZoomMotion {
            zoom,
            changed_at: f64::NEG_INFINITY,
        });
        if m.zoom.to_bits() != zoom.to_bits() {
            *m = ZoomMotion { zoom, changed_at: now };
        }
        *m
    });
    let left = motion.changed_at + ZOOM_SETTLE_S - now;
    if left > 0.0 {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
    }
}

/// The camera zoom while it changed less than `ZOOM_SETTLE_S` ago (by
/// `track_zoom`); `None` at rest.
fn moving_zoom(ctx: &eframe::egui::Context) -> Option<f32> {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.get_temp::<ZoomMotion>(zoom_motion_id()))
        .filter(|m| now - m.changed_at < ZOOM_SETTLE_S)
        .map(|m| m.zoom)
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
/// The egui family name of the bundled regular face: diagram text is
/// painted in this named family, so the chrome may set egui's
/// proportional family to another face.
pub(crate) const REGULAR_FAMILY: &str = "Inter";

/// The `state::Input`s of this frame over the canvas `response`: pointer
/// down, move, up (relative to the canvas's top-left), leave, wheel, and
/// the keys of `state::Key` with modifiers; `text_focus` true when an
/// egui text field has focus. Serves EDIT-9, EDIT-28.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one flat match over egui's events")]
pub fn inputs(ctx: &eframe::egui::Context, response: &eframe::egui::Response) -> Vec<Input> {
    use eframe::egui::{Event as E, Key as K, MouseWheelUnit, PointerButton};
    let rect = response.rect;
    // DECISION: only a focused text field holds the keys (as the shell's
    // F and Backspace rules read it, `app::fit_key_pressed`): egui's
    // "wants keyboard input" is true for any focused widget, so a button
    // focused in a side panel kept the canvas's arrows from it.
    let mut text_focus = ctx.text_edit_focused();
    // A press on the canvas ends any panel focus at once, so the keys of
    // the same frame and the next go to the canvas.
    let mut pressed_on_canvas = false;
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
                            pressed_on_canvas = true;
                            text_focus = false;
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
                        // DECISION: the sideways wheel pans. egui's delta
                        // says how the content moves (positive x: right), as
                        // `pan_by` does, so it passes through: tilting right
                        // moves the content left, like a scrollbar. A line is
                        // 40 points (egui's own native line scroll speed), a
                        // page the canvas width. Shift + vertical wheel is not
                        // turned sideways here: egui only rewrites its smoothed
                        // scroll delta, not this raw event, so it still zooms
                        // (on macOS the OS already sends it as delta.x: a pan).
                        let dx = match unit {
                            MouseWheelUnit::Point => delta.x,
                            MouseWheelUnit::Line => delta.x * 40.0,
                            MouseWheelUnit::Page => delta.x * rect.width(),
                        };
                        if dx != 0.0 {
                            out.push(Input::Pan { by: ScreenPoint { x: dx, y: 0.0 } });
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
    if pressed_on_canvas {
        ctx.memory_mut(eframe::egui::Memory::stop_text_input);
    }
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

/// Adds the bundled font (`scene::FONT_REGULAR` and `scene::FONT_BOLD`,
/// CORE-30, HLD §6) to `fonts` as the named families `REGULAR_FAMILY`
/// and `BOLD_FAMILY`, one face each and no fallback, so the canvas paints
/// with the same face the scene measured and the SVG names; the chrome's
/// own faces (`theme::font_definitions`) stay out of them. Serves
/// EDIT-12, CORE-30.
// DECISION: the canvas owns its family names and faces and `theme`
// calls this, so one place says which face the diagram is painted in;
// `theme::install` is the only `set_fonts` of the editor.
pub fn register_fonts(fonts: &mut eframe::egui::FontDefinitions) {
    use eframe::egui::{FontData, FontFamily};
    use std::sync::Arc;
    fonts
        .font_data
        .insert(REGULAR_NAME.to_owned(), Arc::new(FontData::from_static(FONT_REGULAR)));
    fonts
        .font_data
        .insert(BOLD_NAME.to_owned(), Arc::new(FontData::from_static(FONT_BOLD)));
    fonts
        .families
        .insert(FontFamily::Name(REGULAR_FAMILY.into()), vec![REGULAR_NAME.to_owned()]);
    fonts
        .families
        .insert(FontFamily::Name(BOLD_FAMILY.into()), vec![BOLD_NAME.to_owned()]);
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

    use super::{PaintItem, Primitive, Source, paint_items};
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
            written_thickness: Vec::new(),
            guides: &[],
        }
    }

    /// redesign 2026-11-19 (design-system Marks, "Hover · has views to
    /// enter"): an enterable box's hover mark is the dashed `mark`
    /// outline plus a 122x15 `mark` tab standing on the outline's top
    /// edge, flush right, naming the gesture in mono `mark-handle` text;
    /// a plain hover has no tab. Only marks carry it (never exported).
    #[test]
    fn enterable_hover_draws_the_gesture_tab() {
        let s = scene("context");
        let item = ItemKey::Box(BoxId(0));
        let tab = |kind: MarkKind| {
            let mut l = list(&s);
            l.marks.push(Mark { item, kind });
            paint_items(&l)
                .into_iter()
                .filter(|i| i.source == Source::Mark(item, kind))
                .map(|i| i.primitive)
                .collect::<Vec<_>>()
        };
        let enter = tab(MarkKind::Enterable);
        let outline = s.item_bounds(item).expect("box").inflate(super::MARK_GAP_PX);
        let cam = Camera::default();
        let top_right = cam.to_screen(c4rizer_core::geom::Point::new(outline.max.x, outline.min.y));
        let mk = super::mark_colours(s.mode);
        let block = enter
            .iter()
            .find_map(|p| match p {
                Primitive::Polygon { points, fill: Some(f), stroke: None, .. } if *f == mk.mark => Some(points.clone()),
                _ => None,
            })
            .expect("tab block");
        let (xs, ys): (Vec<f32>, Vec<f32>) = block.iter().map(|q| (q.x, q.y)).unzip();
        let max = |v: &[f32]| v.iter().copied().fold(f32::MIN, f32::max);
        let min = |v: &[f32]| v.iter().copied().fold(f32::MAX, f32::min);
        assert_eq!((max(&xs), max(&ys)), (top_right.x, top_right.y));
        assert_eq!((max(&xs) - min(&xs), max(&ys) - min(&ys)), (122.0, 15.0));
        assert!(enter.iter().any(|p| matches!(p,
            Primitive::Text { text, mono: true, centred: true, colour, .. }
                if text == "RIGHT-CLICK TO ENTER" && *colour == mk.handle)));
        assert!(enter.iter().any(|p| matches!(p, Primitive::Polygon { dash: Some(_), .. })));
        let hover = tab(MarkKind::Hover);
        assert!(!hover.iter().any(|p| matches!(p, Primitive::Text { .. })));
        assert!(!hover.iter().any(|p| matches!(p, Primitive::Polygon { fill: Some(_), .. })));
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
            Source::Guide => 10,
            Source::RubberBand => 11,
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
            // P5-507: a drag's alignment guide is a mark too.
            let guide = [crate::state::guides::Guide {
                vertical: true,
                at: 100.0,
                from: 0.0,
                to: 200.0,
            }];
            l.guides = &guide;
            let items = paint_items(&l);
            let expected = scene_items(&scene);
            assert!(items.iter().any(|i| i.source == Source::Guide));
            for item in &items {
                match item.source {
                    Source::Scene(k) => assert!(expected.contains(&k), "{k:?} not in the scene"),
                    Source::Mark(k, _) => assert_eq!(k, ItemKey::Box(BoxId(0))),
                    Source::Guide => {}
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

    /// EDIT-12, CORE-30: after the editor installs its fonts
    /// (`theme::install`), the named families the canvas paints diagram
    /// text in are the bundled face (`scene::FONT_REGULAR`,
    /// `scene::FONT_BOLD`), no other face.
    // DECISION: since the redesign egui's proportional family is the
    // chrome's Geist; the diagram is painted in the named families, so
    // those are what EDIT-12 pins.
    #[test]
    fn edit_12_canvas_uses_bundled_font() {
        use eframe::egui::{Context, FontFamily, RawInput};
        let ctx = Context::default();
        crate::theme::install(&ctx);
        let mut output = ctx.run_ui(RawInput::default(), |_| {});
        output.textures_delta.clear();
        let defs = ctx.fonts(|f| f.definitions().clone());
        let bold = &defs.families[&FontFamily::Name(super::BOLD_FAMILY.into())];
        assert_eq!(bold.len(), 1, "{bold:?}");
        // The named regular family the canvas paints diagram text in.
        let regular = &defs.families[&FontFamily::Name(super::REGULAR_FAMILY.into())];
        assert_eq!(regular.len(), 1, "{regular:?}");
        let bytes = |name: &str| (*defs.font_data[name].font).as_ref().to_vec();
        assert_eq!(bytes(&regular[0]), c4rizer_core::scene::FONT_REGULAR);
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

    /// EDIT-9: a wheel event over the canvas zooms on its y and pans on
    /// its x; a sideways line is 40 points, the content moving as egui's
    /// delta says.
    #[test]
    fn edit_9_sideways_wheel_reports_pan() {
        use crate::state::Input;
        use eframe::egui::{self, Event as E, Pos2};
        let ctx = eframe::egui::Context::default();
        let wheel = |unit, x: f32, y: f32| E::MouseWheel {
            unit,
            delta: egui::vec2(x, y),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::default(),
            source: egui::MouseWheelSource::Wheel,
        };
        let at = Pos2::new(150.0, 150.0);
        for _ in 0..3 {
            let _ = frame_inputs(&ctx, vec![E::PointerMoved(at)], false);
        }
        let side = frame_inputs(&ctx, vec![wheel(egui::MouseWheelUnit::Line, -1.0, 0.0)], false);
        assert_eq!(side, [Input::Pan { by: ScreenPoint { x: -40.0, y: 0.0 } }], "{side:?}");
        let both = frame_inputs(&ctx, vec![wheel(egui::MouseWheelUnit::Point, 12.0, 100.0)], false);
        assert_eq!(
            both,
            [
                Input::Wheel { at: ScreenPoint { x: 50.0, y: 50.0 }, notches: 2.0 },
                Input::Pan { by: ScreenPoint { x: 12.0, y: 0.0 } },
            ],
            "{both:?}"
        );
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

    /// One frame of a side panel (a button at (20, 20), a text field at
    /// (20, 60)) beside a 400x300 canvas at (100, 100); the canvas inputs.
    /// `tab` gives the button keyboard focus, as Tab in the panel does.
    fn panel_frame(ctx: &eframe::egui::Context, events: Vec<eframe::egui::Event>, tab: bool) -> Vec<crate::state::Input> {
        use eframe::egui::{self, Pos2, RawInput, Rect as ERect, Sense, vec2};
        let input = RawInput {
            screen_rect: Some(ERect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut got = Vec::new();
        let mut text = String::new();
        let mut output = ctx.run_ui(input, |ui| {
            let button = ui.put(ERect::from_min_size(Pos2::new(20.0, 20.0), vec2(60.0, 20.0)), egui::Button::new("b"));
            if tab {
                button.request_focus();
            }
            let _ = ui.put(
                ERect::from_min_size(Pos2::new(20.0, 60.0), vec2(60.0, 20.0)),
                egui::TextEdit::singleline(&mut text),
            );
            let canvas = ERect::from_min_size(Pos2::new(100.0, 100.0), vec2(400.0, 300.0));
            let response = ui.allocate_rect(canvas, Sense::click_and_drag());
            got = super::inputs(ui.ctx(), &response);
        });
        output.textures_delta.clear();
        got
    }

    /// The `text_focus` of the arrow key presses among `inputs`.
    fn arrow_focus(inputs: &[crate::state::Input]) -> Vec<bool> {
        inputs
            .iter()
            .filter_map(|i| match i {
                crate::state::Input::KeyDown {
                    key: crate::state::Key::ArrowRight,
                    text_focus,
                    ..
                } => Some(*text_focus),
                _ => None,
            })
            .collect()
    }

    /// Polish (merge review, P2-712): the first arrow key after a panel
    /// click that leaves no text field focused (a focused button), or
    /// after a click back on the canvas (a text field was focused), acts
    /// on the canvas as later ones do; a focused text field keeps its
    /// arrows.
    #[test]
    fn p2_712_first_arrow_after_panel_click_reaches_canvas() {
        use eframe::egui::{Event as E, Key as K, Modifiers as M, PointerButton as P, Pos2};
        let click = |at: Pos2| {
            vec![
                E::PointerMoved(at),
                E::PointerButton { pos: at, button: P::Primary, pressed: true, modifiers: M::NONE },
                E::PointerButton { pos: at, button: P::Primary, pressed: false, modifiers: M::NONE },
            ]
        };
        let arrow = || {
            vec![
                E::Key { key: K::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: M::NONE },
                E::Key { key: K::ArrowRight, physical_key: None, pressed: false, repeat: false, modifiers: M::NONE },
            ]
        };
        let ctx = eframe::egui::Context::default();
        let run = |events: Vec<E>, tab: bool| {
            let got = panel_frame(&ctx, events, tab);
            let _ = panel_frame(&ctx, Vec::new(), false);
            got
        };
        let _ = run(Vec::new(), false);

        // A button with keyboard focus is no text field.
        let _ = run(click(Pos2::new(40.0, 30.0)), true);
        for n in ["first", "second"] {
            assert_eq!(arrow_focus(&run(arrow(), false)), [false], "{n} arrow, a button focused");
        }

        // A focused text field keeps the arrows; a click back on the
        // canvas gives them to the canvas from the first one on.
        let _ = run(click(Pos2::new(40.0, 70.0)), false);
        assert_eq!(arrow_focus(&run(arrow(), false)), [true], "the text field's arrow");
        let _ = run(click(Pos2::new(300.0, 250.0)), false);
        for n in ["first", "second"] {
            assert_eq!(arrow_focus(&run(arrow(), false)), [false], "{n} arrow after a canvas click");
        }
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

    /// zoom-perf: the rung zoom is at or above the zoom and less than
    /// sqrt 2 above it, so a galley is only scaled down, by at most
    /// 1/sqrt 2; a wheel sweep over the whole zoom range lays text out at
    /// one size per half octave, not one per notch.
    #[test]
    fn zoom_perf_raster_zoom_rungs() {
        use crate::camera::{MAX_ZOOM, MIN_ZOOM, ZOOM_STEP};
        let mut zoom = MIN_ZOOM;
        let mut rungs = Vec::new();
        let mut notches = 0;
        while zoom <= MAX_ZOOM {
            let rung = super::raster_zoom(zoom);
            assert!(rung >= zoom && rung < zoom * std::f32::consts::SQRT_2 + 1e-4, "{zoom} -> {rung}");
            if rungs.last() != Some(&rung) {
                rungs.push(rung);
            }
            zoom *= ZOOM_STEP;
            notches += 1;
        }
        assert!(notches > 40, "{notches}");
        assert!(rungs.len() <= 14, "{} rungs over {notches} notches", rungs.len());
    }

    /// One egui frame at `time` s painting `text` at scene size 14 under
    /// camera `zoom` after `track_zoom`; returns the width of the painted
    /// text and the font size it was laid out at.
    fn zoom_frame(ctx: &eframe::egui::Context, time: f64, zoom: f32, text: &str) -> (f32, f32) {
        use eframe::egui::{Pos2, RawInput, Rect as ERect, vec2};
        let input = RawInput {
            screen_rect: Some(ERect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            time: Some(time),
            ..Default::default()
        };
        let mut got = (0.0, 0.0);
        let mut output = ctx.run_ui(input, |ui| {
            super::track_zoom(ui.ctx(), zoom);
            let painter = ui.painter().clone();
            let item = PaintItem {
                source: Source::RubberBand,
                opacity: 100,
                primitive: Primitive::Text {
                    at: ScreenPoint { x: 10.0, y: 40.0 },
                    text: text.to_owned(),
                    size: 14.0 * zoom,
                    bold: false,
                    centred: false,
                    colour: [0, 0, 0],
                    mono: false,
                },
            };
            super::paint(&painter, std::slice::from_ref(&item));
        });
        for c in &output.shapes {
            if let eframe::epaint::Shape::Text(t) = &c.shape {
                got.0 = t.galley.size().x;
                got.1 = t.galley.job.sections[0].format.font_id.size;
            }
        }
        output.textures_delta.clear();
        got
    }

    /// zoom-perf: while the wheel zooms within one rung, the text is laid
    /// out at one font size (egui's cached galley, its glyphs rasterized
    /// once) and keeps its exact on-screen size; once the zoom rests it is laid out at its
    /// exact size again.
    #[test]
    fn zoom_perf_no_relayout_while_zooming() {
        let ctx = eframe::egui::Context::default();
        // Diagram text is the named Inter family (redesign), which only
        // exists once the app's fonts are installed, as at startup.
        crate::theme::install(&ctx);
        let text = "Internet Banking System";
        let (rest_width, rest_size) = zoom_frame(&ctx, 0.0, 1.0, text);
        assert!((rest_size - 14.0).abs() < 1e-4, "{rest_size}");
        // 1.05 .. 1.4 share the rung sqrt 2.
        let (first, laid_out) = zoom_frame(&ctx, 1.0, 1.05, text);
        assert!((laid_out - 14.0 * std::f32::consts::SQRT_2).abs() < 1e-3, "{laid_out}");
        assert!((first / rest_width - 1.05).abs() < 0.03, "{first} vs {rest_width}");
        for (i, zoom) in [1.1_f32, 1.2, 1.3, 1.4].into_iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let time = 1.05 + 0.05 * i as f64;
            let (width, size) = zoom_frame(&ctx, time, zoom, text);
            assert!(size.to_bits() == laid_out.to_bits(), "zoom {zoom} laid out at {size}, not {laid_out}");
            assert!((width / rest_width - zoom).abs() < 0.03 * zoom, "zoom {zoom}: {width} vs {rest_width}");
        }
        // At rest: laid out at the exact size again.
        let (_, settled) = zoom_frame(&ctx, 2.0, 1.4, text);
        assert!((settled - 14.0 * 1.4).abs() < 1e-4, "{settled}");
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
        let prims = super::page(&l, None);
        assert_eq!(prims.len(), 1, "no paper: one filled rectangle, no outline");
        let page = prims.into_iter().next().expect("a page");
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
        assert!(super::page(&l, None).is_empty());
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

    /// EDIT-31: every mark colour stands out from the page, in both
    /// diagram modes: the selection `mark` (outlines, handles, resize
    /// outline, rubber band, enter mark) by at least 4.5:1, the 1px hover
    /// hint `mark-hint` by at least 3:1.
    // redesign 2026-11-19: the marks are the design tokens `mark` and
    // `mark-hint` per mode; the hint is held to the 3:1 graphics floor
    // (was every mark at 4.5:1 on white).
    #[test]
    fn edit_31_marks_contrast_with_page() {
        use c4rizer_core::model::Mode;
        for (mode, page) in [(Mode::Light, [0xff, 0xff, 0xff]), (Mode::Dark, [0x1e, 0x21, 0x24])] {
            let mk = super::mark_colours(mode);
            for (name, colour, floor) in [("mark", mk.mark, 4.5), ("hint", mk.hint, 3.0)] {
                let ratio = contrast(colour, page);
                assert!(ratio >= floor, "{mode:?} {name} {colour:?} on {page:?}: {ratio:.2}");
            }
        }
        // The light page is the scene's background.
        let bg = scene("containers").background;
        assert_eq!([bg.r, bg.g, bg.b], [0xff, 0xff, 0xff]);
    }
}
