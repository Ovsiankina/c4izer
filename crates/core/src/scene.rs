//! Builds a view's scene, the plain-data picture that the editor canvas
//! and the exporters draw: shapes, wrapped texts, polylines, chevrons,
//! arrowheads, join dots, labels, title, subtitle, key and boundary, each
//! tagged with its model id (CORE-1..4).
//!
//! Depends on: `view`, `route`, `label`, `geom`.

// Shape of the API: `build` takes `(&ViewPlan, &Routes, &Labels)` (box
// centres are `routes.centres`, the ones the lines were routed at and
// labels were placed against, so in a built scene boxes, lines and
// labels agree) and returns a `Scene`: plain data with pub fields, everything
// already measured, wrapped and positioned. `export` writes it as SVG,
// the editor canvas paints it, `lint` checks it with the model. Nobody
// downstream measures text, picks a colour or computes a shape again,
// so the canvas and the SVG of one view cannot differ (EDIT-12).
//
// Drag preview (EDIT-25, EDIT-20): `Scene::move_box` moves one box (rect,
// outline, texts, `centres`) at once and leaves lines and labels as they
// were; the editor calls it per dragged box on every drag event, then
// swaps in a fresh `build` when the worker's `route::reroute` returns.
// Only a moved scene may have boxes and lines that disagree; the scene
// after release is a plain `build` again (EDIT-25 equality).
// DECISION: `move_box` over a `centres` parameter on `build`: the UI
// thread must not wait on label placement or a full rebuild per event,
// and the translation rule stays in scene, not in the editor.
//
// Units: every coordinate and size in a scene is in px (i32), unlike
// `view`/`route`/`label`, which work in grid units. Exporters and the
// canvas draw px; CORE-25 wants integers. `geom::Point` and `geom::Rect`
// carry the px values: their operations (overlap, contains, segment
// direction, crossing tests) are scale free. `Scene::centres` is the one
// field kept in grid units, because `place::Keep::from_previous` and
// `route::reroute` take grid centres.
// DECISION: px, not grid units, in the scene: texts, arrowheads,
// chevrons and person heads sit off the 10px grid, and one unit for the
// whole picture is simpler for the three readers than a mix.
//
// View-local key (SRS 4.2 Interfaces): `ItemKey` names one drawn item
// within one scene (a box by `BoxId`, a line or its label by `EdgeId`,
// the title, subtitle, key or a boundary); `Scene::model_id` maps it to
// its model id. In phase 1 a box is one element and a line one winning
// relationship; phase 3 draws one element several times, which the
// index keys already allow. Hit-testing (EDIT-12, EDIT-28) is
// `Scene::hit` over the item rectangles in reverse paint order.
//
// Paint order (part of the contract, so SVG and canvas stack items the
// same way): boundaries, then lines (polyline, chevrons, tip), then
// boxes, then labels, then title, subtitle and key.
//
// DECISION: lint reads everything through the scene (HLD: lint depends
// on scene and model only): routed crossings (`Scene::crossings`, the
// same count as `Routes::crossings`, so L4 and CORE-11 agree), unroutable
// lines (L1), label fit with the relationship id (L23, L24; closes the
// docs/deferred.md row asking for it), text fit per text (L25), the
// boundary rectangle and members (L26), box kinds and rects (L6, L7,
// L16, L19, L21), line relationships (L14, L22).
// DECISION: the CORE-30 width table is `label::text_advance`; scene
// wraps and sizes every box text, the title, the boundary tag and the
// key with it and never keeps a second table.
// DECISION: the line colour is the destination box's fill (CORE-3);
// phase 2 relationship styles (P2-12) override it.
// DECISION: a joined line (CORE-8) ends in `Tip::Dot`, never an arrow;
// its trunk is `SceneLine::joins`.
//
// CORE-13: an unroutable line is drawn straight from its source box's
// edge to its destination box's edge, on the line between their centres,
// with an arrowhead at the destination, `routed: false` and no label
// (label.rs gives it none); it is outside the CORE-6..9 checks, so lint
// skips that geometry for it and L1 names it.
// CORE-1, CORE-2: box name 14px Inter Bold; type line and description
// 11px Inter Regular on a 14px pitch; 10px padding; lines 2px wide,
// arrowheads 10px, chevrons 6px, join dot radius 4px; title 18px bold
// and subtitle 12px regular at the bottom left below the drawing, the
// key below them. The values live in the scene items (`size_px`,
// `Stroke::width_px`, `Outline`), never as pub constants. Person and
// cylinder proportions are not in the SRS; they follow the prototype
// golden SVGs (a step-5 choice that changes no signature).
// CORE-1: one key entry per distinct box look (shape, fill, text colour)
// or line look (colour, end: arrow or dot, never the tip's coordinates),
// labelled with the most specific tag of the boxes drawn that way (the
// distinct last effective tag of its boxes in box order, so "Person",
// "Software System", "External", ...; several tags with the same look
// share one entry, names joined by commas); every line entry reads
// "Relationship".

use crate::geom::{Dir, Point, Rect};
use crate::label::{Fit, Labels, Weight};
use crate::model::{
    Colour, ElementId, ElementKind, RelationshipId, Shape, ViewKind, ViewOptions, ViewScope,
};
use crate::route::Routes;
use crate::view::{BoundaryOf, BoxId, EdgeId, PlanBox, ViewPlan};

/// The bundled font's family name (Inter, SIL OFL 1.1): the SVG names
/// it first in `font-family` (with a `sans-serif` fallback), the canvas
/// and the PNG rasterizer load the bundled file of that family. Every
/// `TextBlock` is measured with its width table (`label::text_advance`).
/// Serves CORE-30, EXP-1, EXP-2.
pub const FONT_FAMILY: &str = "Inter";

/// The bundled Inter Regular file (assets/fonts, version and sha256 in
/// its README): the face the PNG rasterizer (EXP-2) and the editor
/// canvas (EDIT-12) draw regular text with, and the face the CORE-30
/// width table was derived from. Serves CORE-30, EXP-2, EDIT-12.
pub const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

/// The bundled Inter Bold file, for `Weight::Bold` text (box names,
/// title, boundary tag). Serves CORE-30, EXP-2, EDIT-12.
pub const FONT_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-Bold.ttf");

/// A view-local key: one drawn item of one scene. Valid for that scene
/// only; identity across scenes is the model id (`Scene::model_id`).
/// `#[non_exhaustive]` so later phases add items (group boundaries,
/// vertices). Serves CORE-1, EDIT-5, EDIT-10, EDIT-12, EDIT-28.
// DECISION: no `repr` and no size pin: not a hot struct (one per
// selection or hit), and a pin without `repr` would rest on unspecified
// rustc layout. Listed at sign-off as a data-layout choice.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ItemKey {
    /// A box: `Scene::boxes[id]`.
    Box(BoxId),
    /// A line: the `SceneLine` whose `edge` is this id.
    Line(EdgeId),
    /// The label of a line: the `SceneLabel` whose `edge` is this id.
    /// Selecting it selects that line's relationship (EDIT-5).
    Label(EdgeId),
    /// The view title (EDIT-5 edits it).
    Title,
    /// The view subtitle.
    Subtitle,
    /// The generated key (CORE-1).
    Legend,
    /// `Scene::boundaries[i]`.
    Boundary(u32),
}

/// What a view-local key stands for in the model. `#[non_exhaustive]`
/// so later phases add model items (groups, deployment nodes). Serves
/// CORE-1, EDIT-5, EDIT-10.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelId {
    /// An element (a box, or the element a boundary stands for).
    Element(ElementId),
    /// A relationship: a line's winning relationship (CORE-5).
    Relationship(RelationshipId),
}

/// Horizontal alignment of a text block's lines on `TextBlock::at.x`.
/// `#[non_exhaustive]` for later alignments. Serves CORE-2, CORE-30.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Align {
    /// Lines start at `at.x` (SVG `text-anchor="start"`).
    Start,
    /// Lines are centred on `at.x` (SVG `text-anchor="middle"`).
    Middle,
}

/// Text already wrapped into its final lines (CORE-30): painters draw
/// each line as given, in `FONT_FAMILY` at `size_px` and `weight`, line
/// `i` with its baseline at `at.y + i * line_px`, aligned on `at.x` by
/// `align`; they never re-wrap or re-measure. Serves CORE-2, CORE-30,
/// CORE-14.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextBlock {
    /// The lines, top to bottom; never empty: an empty text (an element
    /// with an empty name, lint L9) is one empty line.
    pub lines: Vec<String>,
    /// Anchor in px: x per `align`, y the first line's baseline.
    pub at: Point,
    /// Alignment of every line on `at.x`.
    pub align: Align,
    /// Font size in px.
    pub size_px: i32,
    /// Baseline-to-baseline distance in px.
    pub line_px: i32,
    /// Regular or bold (`label::Weight`).
    pub weight: Weight,
    /// Text colour.
    pub colour: Colour,
    /// The rectangle in px the lines take, measured with
    /// `label::text_advance` (widest line by `lines.len() * line_px`):
    /// what the hit test, the CORE-14 title and key clearance and the
    /// fit test use.
    pub bounds: Rect,
    /// True when `bounds` lies inside the rectangle this text must fit
    /// (its box's text area, its label or tag rectangle); false is lint
    /// warning L25 (CORE-30).
    pub fits: bool,
}

/// How an outline or line is stroked. Serves CORE-1 (dashed boundary),
/// CORE-2, CORE-10 (solid lines).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Stroke {
    /// Stroke colour.
    pub colour: Colour,
    /// Stroke width in px.
    pub width_px: i32,
    /// `None` for solid, else `[dash, gap]` in px (SVG
    /// `stroke-dasharray`). Lines are always solid (CORE-10); boundaries
    /// are dashed (CORE-1).
    pub dash_px: Option<[i32; 2]>,
}

/// The exact geometry of a box shape in px, so the canvas and the SVG
/// draw the same outline (EDIT-12). `#[non_exhaustive]`: phase 2 adds
/// Structurizr's other shapes (P2-11). Serves CORE-2.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outline {
    /// A rectangle with rounded corners (`Shape::Box`).
    Rect {
        /// The rectangle.
        rect: Rect,
        /// Corner radius; 0 for square corners.
        radius_px: i32,
    },
    /// The person shape (`Shape::Person`): a circular head above a
    /// rounded body; the union fills the box rectangle's width.
    Person {
        /// Centre of the head circle.
        head_centre: Point,
        /// Radius of the head circle.
        head_radius_px: i32,
        /// The body rectangle; the texts sit in it.
        body: Rect,
        /// The body's corner radius.
        body_radius_px: i32,
    },
    /// A cylinder (`Shape::Cylinder`): `rect` with an elliptic cap top
    /// and bottom, each ellipse spanning `rect`'s width with vertical
    /// radius `cap_ry_px`; the top cap is drawn whole, the bottom one as
    /// its lower half.
    Cylinder {
        /// The cylinder's bounding rectangle.
        rect: Rect,
        /// Vertical radius of the caps.
        cap_ry_px: i32,
    },
}

/// One box (CORE-2): an element drawn with its shape, fill, name, type
/// line and wrapped description. Serves CORE-1, CORE-2, CORE-30,
/// EDIT-10.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneBox {
    /// Its plan box, and its index in `Scene::boxes`
    /// (`boxes[i].id == BoxId(i)`); its `ItemKey` is `ItemKey::Box(id)`.
    pub id: BoxId,
    /// The model element it draws (CORE-1 tagging, EDIT-10 details).
    pub element: ElementId,
    /// Person, software system, container or component (L16, L19, L21).
    pub kind: ElementKind,
    /// The resolved shape (CORE-2: people as persons, `Cylinder`
    /// elements as cylinders).
    pub shape: Shape,
    /// The box rectangle in px: `PlanBox::rect_at` of its centre, times
    /// `GRID_PX` (L6, L26, hit test).
    pub rect: Rect,
    /// The outline to draw inside `rect`.
    pub outline: Outline,
    /// The fill: the resolved style's background.
    pub fill: Colour,
    /// The outline stroke.
    pub stroke: Stroke,
    /// The name, bold, centred, wrapped to the box's text width.
    pub name: TextBlock,
    /// `[type: technology]`, or `[type]` with no technology; type
    /// `Person`, `Software System` (`External Software System` when
    /// tagged `External`), `Container` or `Component` (CORE-2).
    pub type_line: TextBlock,
    /// The description wrapped to the box's text width, `None` when the
    /// element has none.
    pub description: Option<TextBlock>,
}

/// An open V (two strokes meeting at the apex) pointing along its
/// segment, at the segment's middle or at 1/4, 1/2 and 3/4 (CORE-10).
/// Drawn with its line's stroke. Serves CORE-10.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chevron {
    /// Its point on the segment, in px.
    pub at: Point,
    /// The line's direction there.
    pub dir: Dir,
    /// The three points of the V, in px: arm end, apex, arm end.
    pub points: [Point; 3],
}

/// How a drawn line ends. `#[non_exhaustive]` for later end styles.
/// Serves CORE-8, CORE-10.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tip {
    /// A filled triangle in the line's colour: tip on the destination
    /// port, then the two back corners, in px.
    Arrow([Point; 3]),
    /// A filled dot in the line's colour where a joined line meets its
    /// trunk (CORE-8, `bus on`).
    Dot {
        /// The dot centre, in px: the line's last point.
        at: Point,
        /// The dot radius in px.
        radius_px: i32,
    },
}

/// One drawn line (CORE-3, CORE-10): a solid polyline in its
/// destination's colour, its chevrons and its tip. Serves CORE-1,
/// CORE-3, CORE-5, CORE-8, CORE-10, CORE-13.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneLine {
    /// The plan edge it draws; its `ItemKey` is `ItemKey::Line(edge)`.
    pub edge: EdgeId,
    /// The winning relationship (`PlanEdge::winner`): its model id.
    pub relationship: RelationshipId,
    /// Every relationship the line stands for, winner first
    /// (`PlanEdge::relationships`; EDIT-5 details, L22 coverage).
    pub relationships: Vec<RelationshipId>,
    /// The box it starts at.
    pub source: BoxId,
    /// The box it points to (CORE-3: it takes that box's fill).
    pub destination: BoxId,
    /// Vertices in px, source to destination: `Line::points` times
    /// `GRID_PX`, or the straight fallback when `routed` is false.
    pub points: Vec<Point>,
    /// Solid, in the destination box's fill colour (CORE-3).
    pub stroke: Stroke,
    /// Direction chevrons, in segment order (CORE-10); none within 8px
    /// of a label.
    pub chevrons: Vec<Chevron>,
    /// Arrowhead, or join dot when the line joins a trunk.
    pub tip: Tip,
    /// The trunk it joins (`Line::joins`), `None` for an arrowed line.
    pub joins: Option<EdgeId>,
    /// False when the router could not route it (CORE-13, L1): drawn as
    /// a straight fallback, outside the CORE-6..9 checks.
    pub routed: bool,
}

/// One placed label (CORE-14): `label::Label` in px, its text block and
/// its fit, tagged with the relationship lint names in L23 and L24.
/// Serves CORE-14, CORE-15, CORE-4.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneLabel {
    /// The edge it labels; its `ItemKey` is `ItemKey::Label(edge)`.
    pub edge: EdgeId,
    /// The edge's winning relationship (CORE-15: the finding names it).
    pub relationship: RelationshipId,
    /// The label rectangle in px (`label::Label` geometry).
    pub rect: Rect,
    /// Backing fill drawn under the text, `None` for none.
    pub fill: Option<Colour>,
    /// `label::Label::lines` at `LABEL_FONT_PX` and `LABEL_LINE_PX`,
    /// inside `rect` with the label paddings.
    pub text: TextBlock,
    /// Clear, over lines (L23) or no room (L24), as `label` placed it.
    pub fit: Fit,
}

/// One dashed boundary (CORE-1): `ViewPlan::boundary_rect` in px and its
/// name tag at the bottom left in `label::boundary_tag`. Serves CORE-1,
/// L26.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneBoundary {
    /// What it stands for (`view::Boundary::of`).
    pub of: BoundaryOf,
    /// The boxes inside, in box order (L26: any other box must not
    /// overlap `rect`).
    pub members: Vec<BoxId>,
    /// The boundary rectangle in px.
    pub rect: Rect,
    /// Dashed stroke, no fill.
    pub stroke: Stroke,
    /// The tag rectangle in px (`label::boundary_tag` times `GRID_PX`).
    pub tag_rect: Rect,
    /// The tag text, bold: the name, then the scope's type in brackets.
    pub tag: TextBlock,
}

/// The sample drawn beside a key entry. `#[non_exhaustive]` so phase 2
/// adds relationship styles (dashed, thickness). Serves CORE-1.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Swatch {
    /// A small box of one shape, fill and stroke.
    Box {
        /// The sample outline, in px.
        outline: Outline,
        /// The family's fill.
        fill: Colour,
        /// The family's stroke.
        stroke: Stroke,
    },
    /// A short line of one stroke and tip kind (arrow or dot).
    Line {
        /// Start, in px.
        from: Point,
        /// End, in px (where the tip sits).
        to: Point,
        /// The family's stroke.
        stroke: Stroke,
        /// Arrowhead or join dot.
        tip: Tip,
    },
}

/// One key entry: a swatch and the names it stands for. Two tags with the
/// same look share one entry with both names. Serves CORE-1.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LegendEntry {
    /// The sample.
    pub swatch: Swatch,
    /// The names, in first-drawn order; never empty.
    pub names: Vec<String>,
    /// The names as drawn beside the swatch.
    pub text: TextBlock,
}

/// The generated key (CORE-1): one entry per distinct box look (shape,
/// fill, text colour) and per distinct line look (colour and end, arrow
/// or dot; the tip's coordinates do not count) drawn in the view, box
/// entries named by the most specific tag of their boxes, line entries
/// `Relationship`; an unstyled element still gets an entry, in the
/// Structurizr default colours. Laid out clear of every box, line and
/// label (CORE-14). Serves CORE-1, CORE-14.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Legend {
    /// The key's rectangle in px (hit test, CORE-14 clearance).
    pub rect: Rect,
    /// Box entries first, then line entries, each in first-drawn order.
    pub entries: Vec<LegendEntry>,
}

/// The picture of one view: everything the canvas and the exporters
/// draw, and nothing else (CORE-1, EDIT-12). Painters draw, in this
/// order: `background` over `bounds`, `boundaries`, `lines` (polyline,
/// chevrons, tip), `boxes`, `labels`, `title`, `subtitle`, `legend`.
/// Same plan, routes and labels give an equal value on every run and
/// machine (CORE-25). Serves CORE-1..5, CORE-10, CORE-13..15, CORE-25,
/// CORE-26, CORE-30, EDIT-12, EDIT-25.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Scene {
    /// The view's effective key (names the exported file, CLI-1, EDIT-7).
    pub key: String,
    /// System context, container or component (L15..L21).
    pub kind: ViewKind,
    /// The element the view is about (L21, EDIT-16 breadcrumb).
    pub scope: ViewScope,
    /// The view options, `plan.options` (L4 `max_crossings`, L15/L20
    /// `technology`); also right for EDIT-15 default views, which are
    /// not in `Model::views`. Serves CORE-4, CORE-17.
    pub options: ViewOptions,
    /// The drawing area in px: contains every item, lines included
    /// (lines may leave the boxes' bounds, CORE-7), plus a margin. The
    /// SVG `viewBox`; may start at negative coordinates.
    pub bounds: Rect,
    /// Background fill of `bounds`.
    pub background: Colour,
    /// Box centres in grid units, `centres[i]` for `BoxId(i)`: the
    /// `routes.centres` the scene was built at, then changed by
    /// `move_box`. For
    /// `place::Keep::from_previous` (CORE-29) and `route::reroute`
    /// (EDIT-25), and for `at` lines (EDIT-26, times `GRID_PX`).
    pub centres: Vec<Point>,
    /// Every box, `boxes[i].id == BoxId(i)`.
    pub boxes: Vec<SceneBox>,
    /// One line per plan edge, in `EdgeId` order (unroutable ones with
    /// `routed: false`).
    pub lines: Vec<SceneLine>,
    /// One label per labelled edge, in `EdgeId` order (`Labels::labels`
    /// that are `Some`).
    pub labels: Vec<SceneLabel>,
    /// The dashed boundaries, `boundaries[i]` for `view` boundary `i`;
    /// phase 1 at most one, none when the scope has no drawn child.
    pub boundaries: Vec<SceneBoundary>,
    /// The title, `None` when the view has none (lint L8).
    pub title: Option<TextBlock>,
    /// The subtitle, if written.
    pub subtitle: Option<TextBlock>,
    /// The generated key.
    pub legend: Legend,
    /// Crossings between routed lines, `Routes::crossings` as routed
    /// (L4, CORE-11, CORE-17). `move_box` leaves it unchanged.
    pub crossings: u32,
}

impl Scene {
    /// The model id behind `key`: a box's element, a line's or label's
    /// winning relationship, a boundary's element; `None` for the title,
    /// subtitle, key, or a key not in this scene. Serves CORE-1, EDIT-5,
    /// EDIT-10.
    #[must_use]
    pub fn model_id(&self, key: ItemKey) -> Option<ModelId> {
        todo!()
    }

    /// The topmost item whose shape contains `at` (px), in reverse paint
    /// order: key, subtitle, title, labels, boxes (their outline), lines
    /// (within `tolerance_px` of a segment), boundaries (within
    /// `tolerance_px` of the dashed outline or on the tag). `None` on
    /// empty canvas. Serves EDIT-12, EDIT-28, EDIT-15.
    #[must_use]
    pub fn hit(&self, at: Point, tolerance_px: i32) -> Option<ItemKey> {
        todo!()
    }

    /// The px rectangle of `key`'s item (a line: the bounds of its
    /// points and tip), for selection marks and rubber-band selection;
    /// `None` when not in this scene. Serves EDIT-12, EDIT-20.
    #[must_use]
    pub fn item_bounds(&self, key: ItemKey) -> Option<Rect> {
        todo!()
    }

    /// Every box drawing `element`, in box order; phase 1 at most one.
    /// The editor re-finds a selection by element after a reload.
    /// Serves EDIT-1, EDIT-10.
    #[must_use]
    pub fn boxes_of(&self, element: ElementId) -> Vec<BoxId> {
        todo!()
    }

    /// Every line standing for `relationship` (as winner or merged), in
    /// edge order. Serves EDIT-1, EDIT-5.
    #[must_use]
    pub fn lines_of(&self, relationship: RelationshipId) -> Vec<EdgeId> {
        todo!()
    }

    /// The lines the router could not route, ascending (L1, CORE-13).
    #[must_use]
    pub fn unroutable(&self) -> Vec<EdgeId> {
        todo!()
    }

    /// Every pair of routed lines that cross, as `(a, b, n)` with
    /// `a < b` and `n > 0` crossings between them, ordered by `(a, b)`:
    /// the rule of `Routes::crossings` (`geom::crossings` per pair,
    /// minus the meeting where a joined line ends on its trunk), so the
    /// `n` sum to `crossings` on a built scene. Lines with `routed:
    /// false` take no part. Lint names the boxes involved from it
    /// (CORE-26) without a second crossing rule. Serves CORE-11, CORE-26.
    #[must_use]
    pub fn crossing_pairs(&self) -> Vec<(EdgeId, EdgeId, u32)> {
        todo!()
    }

    /// The drag preview (EDIT-25, EDIT-20): moves box `id` to `centre`
    /// (grid units) at once. Translates its `rect`, `outline` and every
    /// text block by the same px offset, sets `centres[id]`, and grows
    /// `bounds` to contain the moved box (never shrinks it). Lines,
    /// labels, boundaries, title, subtitle, key and `crossings` stay as
    /// they were, so lines may no longer meet the box until the editor
    /// swaps in a scene built from the re-routed lines. Moving to the
    /// current centre changes nothing. Serves EDIT-20, EDIT-25.
    ///
    /// # Panics
    ///
    /// When `id` is not a box of this scene.
    pub fn move_box(&mut self, id: BoxId, centre: Point) {
        todo!()
    }
}

/// True when `plan_box`'s texts (name, type line, description), wrapped
/// as `build` wraps them, fit a box of `half_width` by `half_height` grid
/// units. The editor never resizes below it (EDIT-18); `build` sets
/// `TextBlock::fits` with the same rule (CORE-30, L25). Monotone: if a
/// size fits, every size at least as wide and as tall fits, so the
/// editor finds the smallest size by searching. A box whose current
/// size already fails (an `at`/`size` line written by hand) may keep
/// that size and grow, but a resize never makes it smaller. Serves
/// EDIT-18, CORE-30.
#[must_use]
pub fn texts_fit(plan_box: &PlanBox, half_width: i32, half_height: i32) -> bool {
    todo!()
}

/// Builds the scene of `plan` with every box at `routes.centres[box]`,
/// the lines of `routes` and the labels of `labels`: boxes with their
/// outline, fill and wrapped texts (CORE-2, CORE-30); one line per edge
/// in its destination's colour with chevrons and tip (CORE-3, CORE-8,
/// CORE-10; unroutable ones as a straight fallback, CORE-13); labels in
/// px with their fit (CORE-14, CORE-15; technology per
/// `PlanEdge::shown_technology`, CORE-4); boundaries with their tags,
/// title, subtitle and key laid out clear of every box, line and label
/// (CORE-1, CORE-14); `options` copied from `plan.options`. Deterministic (CORE-25). Serves CORE-1..5,
/// CORE-10, CORE-13..15, CORE-25, CORE-30.
///
/// # Panics
///
/// When `routes` or `labels` belong to another plan (`routes.lines` or
/// `labels.labels` not as long as `plan.edges`, or `routes.centres`
/// shorter than `plan.boxes`).
#[must_use]
pub fn build(plan: &ViewPlan, routes: &Routes, labels: &Labels) -> Scene {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test per requirement.
    #[test]
    fn core_1_title_subtitle_key_boundary() {
        // Title and subtitle present; one boundary around the scope's
        // children plus 40px, tag at its bottom left.
        todo!()
    }

    #[test]
    fn core_1_key_covers_every_family() {
        // Review Focus: one box and no lines still lists that box's
        // family; two tags with the same look give one entry with both
        // names; an unstyled element gets the default colours, in the key.
        todo!()
    }

    #[test]
    fn core_1_no_boundary_without_children() {
        // Review Focus: a scope with no drawn child draws no boundary.
        todo!()
    }

    #[test]
    fn core_2_person_and_cylinder_shapes() {
        todo!()
    }

    #[test]
    fn core_2_box_texts_name_type_description() {
        // `[Container: Rust]`, `[Container]` with no technology,
        // `[External Software System]` for an `External` system.
        todo!()
    }

    #[test]
    fn core_3_line_takes_target_colour() {
        todo!()
    }

    #[test]
    fn core_4_technology_off() {
        // No label text holds the technology; box type lines keep it.
        todo!()
    }

    #[test]
    fn core_5_one_line_per_edge_with_all_relationships() {
        // A merged lifted line is drawn once, tagged with its winner and
        // listing every relationship it stands for.
        todo!()
    }

    #[test]
    fn core_7_bounds_contain_every_line() {
        todo!()
    }

    #[test]
    fn core_10_chevrons_along_line() {
        // One chevron at the middle of a 100px segment, three at 1/4,
        // 1/2, 3/4 of a 320px one, none within 8px of a label.
        todo!()
    }

    #[test]
    fn core_8_join_dot_drawn() {
        // CORE-8: a joined line ends in a dot on its trunk, no arrowhead.
        todo!()
    }

    #[test]
    fn core_13_unroutable_line_drawn_and_listed() {
        // CORE-13: drawn straight from source box edge to destination
        // box edge on the centre line, with an arrowhead, `routed:
        // false`, in `unroutable()`, no label.
        todo!()
    }

    #[test]
    fn core_14_title_key_clear_of_labels() {
        // Title, subtitle and key bounds are disjoint from every label,
        // box and line (docs/deferred.md row from label).
        todo!()
    }

    #[test]
    fn core_15_label_fit_and_relationship_carried() {
        // `Fit::OverLines` and `Fit::NoRoom` reach the scene label with
        // the edge's winning relationship (L23, L24).
        todo!()
    }

    #[test]
    fn core_25_same_input_same_scene() {
        todo!()
    }

    #[test]
    fn core_26_crossing_pairs_sum_to_crossings() {
        // Pairs ascending, `a < b`, counts sum to `crossings`; a joined
        // line ending on its trunk is no pair; an unroutable line is in
        // none.
        todo!()
    }

    #[test]
    fn core_30_texts_wrapped_and_fit_flagged() {
        // Lines measured with `label::text_advance`; a text that cannot
        // fit has `fits == false`; `texts_fit` agrees with `build`.
        todo!()
    }

    #[test]
    fn every_item_tagged_with_id() {
        // Every box, line, label and boundary maps through `model_id`;
        // title, subtitle and key map to `None`.
        todo!()
    }

    #[test]
    fn edit_1_reselect_by_element() {
        // `boxes_of` gives the box drawing an element (phase 1: one), and
        // nothing for an element not drawn in the view.
        todo!()
    }

    #[test]
    fn edit_5_lines_of_relationship_includes_merged() {
        // `lines_of` finds a merged lifted line both by its winner and by
        // a relationship merged into it; an undrawn one gives nothing.
        todo!()
    }

    #[test]
    fn edit_25_box_moves_before_lines() {
        // `move_box` shifts rect, outline and texts by the same offset
        // and sets `centres`; lines, labels and `crossings` are
        // unchanged; `bounds` contains the moved box; moving to the
        // current centre is a no-op.
        todo!()
    }

    #[test]
    fn edit_28_hit_test_topmost_item() {
        // `hit` returns the topmost item in reverse paint order; empty
        // canvas gives `None`; `item_bounds` contains every hit point.
        todo!()
    }
}
