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
// arrowheads 10px, chevrons 6px, join dot radius 4px; title 15px bold
// and subtitle 11px regular at the bottom left below the drawing, the
// key below them (owner decision 2026-11-19: the redesign's values). The values live in the scene items (`size_px`,
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

use crate::geom::{Contact, Dir, GRID_PX, Point, Rect, Segment, crossings};
use crate::label::{
    EM, Fit, LABEL_PAD_X_PX, LABEL_PAD_Y_PX, Label, Labels, Weight, Words, boundary_tag_font_px,
    boundary_tag_text, boundary_tag_with, text_advance,
};
use crate::model::{
    Asset, Border, Colour, ElementId, ElementKind, ImageSource, Mime, Mode, RelationshipId, Shape,
    ViewKind, ViewOptions, ViewImage, ViewScope,
};
use crate::route::{Line, Routes};
use crate::view::{BoundaryOf, BoxId, EdgeId, PlanBox, PlanEdge, ViewPlan};

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
/// draw the same outline (EDIT-12). Every outline lies inside the box
/// rectangle and the box texts inside its text area (P2-413); lines end
/// on the box rectangle, not the outline (P2-414). Serves CORE-2, P2-11,
/// P2-413..P2-416.
// TODO(srs): P2-413/P2-414. Reading taken (B styles-themes 6):
// proportions per Structurizr's notation page; lines end on the box
// rectangle, so CORE-6's grid ports hold for every shape.
// TODO(srs): P2-415. Reading taken (B styles-themes 3): `Box` keeps the
// phase-1 6px corner radius, `RoundedBox` uses 20px.
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
    /// `Shape::Circle` (a circle centred in `rect`, diameter its shorter
    /// side) and `Shape::Ellipse` (the ellipse filling `rect`): told
    /// apart by `circle`.
    Ellipse {
        /// The ellipse's bounding rectangle (a square for a circle).
        rect: Rect,
        /// True for `Shape::Circle`.
        circle: bool,
    },
    /// `Shape::Hexagon`: flat top and bottom, the left and right points
    /// `inset_px` in from the slanted sides' ends.
    Hexagon {
        /// Bounding rectangle.
        rect: Rect,
        /// Horizontal inset of the top and bottom corners.
        inset_px: i32,
    },
    /// `Shape::Diamond`: the four side midpoints of `rect`.
    Diamond {
        /// Bounding rectangle.
        rect: Rect,
    },
    /// `Shape::Bucket`: a cylinder whose bottom is narrower than its top.
    Bucket {
        /// Bounding rectangle.
        rect: Rect,
        /// Vertical radius of the top cap.
        cap_ry_px: i32,
    },
    /// `Shape::Pipe`: a horizontal cylinder with caps left and right.
    Pipe {
        /// Bounding rectangle.
        rect: Rect,
        /// Horizontal radius of the caps.
        cap_rx_px: i32,
    },
    /// `Shape::Robot`: a head above a body, with the texts in the body.
    Robot {
        /// The head rectangle.
        head: Rect,
        /// The body rectangle; the texts sit in it.
        body: Rect,
        /// Corner radius of both.
        radius_px: i32,
    },
    /// `Shape::Folder`: a box with a tab on its top left.
    Folder {
        /// The folder body.
        rect: Rect,
        /// The tab above the body.
        tab: Rect,
    },
    /// `Shape::WebBrowser`, `Window`, `Terminal`, `Shell`: a box with a
    /// title bar `bar_px` tall, decorated per `kind`.
    Window {
        /// Bounding rectangle.
        rect: Rect,
        /// Title bar height.
        bar_px: i32,
        /// Which of the four.
        kind: WindowKind,
    },
    /// `Shape::MobileDevicePortrait`/`MobileDeviceLandscape`: a rounded
    /// device with the screen `inset_px` inside it.
    MobileDevice {
        /// Bounding rectangle.
        rect: Rect,
        /// True for portrait.
        portrait: bool,
        /// Bezel width.
        inset_px: i32,
    },
    /// `Shape::Component`: a box with two small tabs on its left side.
    Component {
        /// The component body.
        rect: Rect,
        /// The two tabs, top first.
        tabs: [Rect; 2],
    },
}

/// The decoration of an `Outline::Window`. Serves P2-11.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// `Shape::WebBrowser`: bar with an address field.
    WebBrowser,
    /// `Shape::Window`: bar with three buttons.
    Window,
    /// `Shape::Terminal`: dark screen.
    Terminal,
    /// `Shape::Shell`: dark screen with a prompt.
    Shell,
}

/// An icon drawn inside a box or beside a boundary's name tag, the same
/// in canvas, SVG (`<image>` with a `data:` URI) and PNG. Serves P2-409.
// TODO(srs): P2-409 icon placement. Reading taken (B styles-themes 7a,
// 7b): centred in the box's bottom band, at most 1/4 of the box height,
// clear of the texts; a boundary icon left of its name tag.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneIcon {
    /// Where it is drawn, in px.
    pub rect: Rect,
    /// The image file.
    pub asset: Asset,
}

/// The picture of an `image` view, drawn under the title (P2-320), or
/// the placeholder of a source c4rizer does not draw (P2-321). Serves
/// P2-19, P2-320, P2-321, P2-340.
// TODO(srs): P2-340, an SVG source holding a script or an external
// reference. Reading taken (B landscape-filtered 11): embedded as
// `<image href="data:image/svg+xml;base64,...">`, which browsers render
// without running scripts or loading references; the EXP-1 scan checks
// the exported document, not the bytes inside the data URI.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneImage {
    /// Where the picture (or placeholder) is drawn, in px.
    pub rect: Rect,
    /// The view's source line and, for a local file, its bytes.
    pub image: ViewImage,
    /// The placeholder text naming the source kind and the file, URL or
    /// view key as written (`plantuml`, `mermaid`, an `https://` image);
    /// `None` when the picture itself is drawn.
    pub placeholder: Option<String>,
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
    /// 0..100: fill, outline, texts and icon drawn at `opacity`/100;
    /// 100 draws no opacity attribute (P2-406).
    pub opacity: u8,
    /// The style's icon, inside `rect` and clear of the texts (P2-409).
    pub icon: Option<SceneIcon>,
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
    /// 0..100: polyline, chevrons and tip at `opacity`/100 (P2-426).
    pub opacity: u8,
    /// The crossing points (px) where this line hops over a line drawn
    /// before it, or a later line that does not jump (`jump true`,
    /// P2-422); empty otherwise. L4's crossing
    /// count is unchanged.
    pub jumps: Vec<Point>,
    /// The polylines stroked when a bridge cuts or bends this line
    /// (P4-506, P4-508): the pieces between its gaps, or its whole route
    /// with each hop; empty when it carries no bridge, and `points` is
    /// stroked. `points` stays the route for hit-testing (P4-518).
    // APPROVER: new pub item
    pub runs: Vec<Vec<Point>>,
    /// The T bars at its cut ends (P4-506, P4-510), each from end to end,
    /// drawn solid with the line's colour and width; empty without a gap.
    // APPROVER: new pub item
    pub bars: Vec<[Point; 2]>,
    /// `PlanEdge::thickness_written`: a style wrote the line's thickness,
    /// so the SVG draws `stroke.width_px` as written, even 2px.
    // DECISION: crate-private (the struct is `#[non_exhaustive]`, so no
    // literal outside the crate breaks): the export reads it; the editor
    // finds the same fact through the model's pub relationship styles.
    pub(crate) thickness_written: bool,
}

/// One crossing of two routed lines in a view that marks crossings
/// (`crossings gap|jump`): where, which line carries the bridge, which
/// passes whole. Serves P4-503, P4-517, P4-522.
// APPROVER: new pub item
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneBridge {
    /// The crossing point, px, rounded.
    pub at: Point,
    /// The line cut or hopping there.
    pub carrier: EdgeId,
    /// The line passing whole.
    pub passing: EdgeId,
    /// False when the crossing is drawn plain (P4-515, P4-516).
    pub drawn: bool,
    /// True when the default rule chose the carrier (P4-504), false when
    /// a `bridge` line did (P4-505).
    pub by_default: bool,
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
    /// 0..100, the line's opacity (P2-426).
    pub opacity: u8,
    /// In a `labels callout` view, the shelf, leader and dot drawn with
    /// the text (P4-303..P4-307); `None` for an on-line label.
    // APPROVER: new pub item
    pub callout: Option<SceneCallout>,
}

/// The marks of a callout label (`label::Callout`), px: shelf and leader
/// solid in the line's colour at half its width (at least 1px), and a
/// filled dot at the attach point. Serves P4-303..P4-307, P4-316.
// APPROVER: new pub item
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneCallout {
    /// Left and right end of the shelf (P4-303).
    pub shelf: [Point; 2],
    /// Shelf end, then attach point on the line (P4-304).
    pub leader: [Point; 2],
    /// The attach point: the dot's centre (P4-307).
    pub dot: Point,
    /// The dot's radius, `CALLOUT_DOT_RADIUS_PX` (P4-307).
    pub dot_radius_px: i32,
    /// Solid, the line's colour, half the line's width, at least 1px.
    pub stroke: Stroke,
    /// The other lines the leader crosses, ascending (P4-310).
    pub crossings: Vec<EdgeId>,
}

/// The radius, px, of the dot at a callout's attach point (P4-307).
// APPROVER: new pub item
pub const CALLOUT_DOT_RADIUS_PX: i32 = 3;

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
    /// Fill from the boundary style's `background`; `None`: no fill
    /// (P2-411, P2-412).
    pub fill: Option<Colour>,
    /// 0..100 (P2-411).
    pub opacity: u8,
    /// A boundary icon (P2-411; TODO(srs) on `ResolvedBoundaryStyle`).
    pub icon: Option<SceneIcon>,
    /// A deployment node's instance count (`x2`, `x1..3`) at the bottom
    /// right inside `rect`, in the tag's colour; `None` for every other
    /// boundary. Serves P3-115.
    // APPROVER: new pub item
    pub instances: Option<TextBlock>,
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
///
/// Phase 2 (styles-themes step 5): a box look adds stroke colour,
/// stroke width and border (P2-435); line entries are one per
/// relationship-style tag (the last tag of the winner, in DSL-15 order,
/// that has a relationship style block and is not `Relationship`), named
/// by it and drawn with that style's colour, thickness and dash, plus one
/// `Relationship` entry for every other drawn line (P2-27, P2-434).
// TODO(srs): P2-27/P2-434/P2-435. Readings taken (B styles-themes 12):
// join-dot lines keep their own entry; the `Relationship` entry's swatch
// is #707070; opacity and icons do not split box entries.
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
    /// The plan's mode: the colour defaults it was drawn with (P2-431);
    /// `pipeline::rebuild_after_move` keeps it. Serves P2-430, P2-433.
    // TODO(srs): P2-431, dark-mode defaults. Reading taken (B
    // styles-themes 10c): background #111111, box text #ffffff, default
    // box fill #333333, line and boundary #aaaaaa; every other default
    // unchanged.
    pub mode: Mode,
    /// The picture of an `image` view, from `plan.image`; `None` for
    /// every other kind. Serves P2-19, P2-320, P2-321.
    pub image: Option<SceneImage>,
    /// The view's bundles as routed (`Routes::bundles`, points in grid
    /// units): members in track order, tracks, member crossings, fallbacks.
    /// Empty without bundles. Serves P4-420, L30, L31, P4-426.
    // APPROVER: new pub item
    pub bundles: Vec<crate::route::RoutedBundle>,
    /// Every crossing of routed lines with its bridge, in a view that
    /// marks crossings; empty with `crossings plain` or no crossing.
    /// Serves P4-503, P4-517.
    // APPROVER: new pub item
    pub bridges: Vec<SceneBridge>,
}

impl Scene {
    /// How many crossings are drawn plain for want of room (P4-515,
    /// P4-516). Serves P4-517.
    // APPROVER: new pub item
    #[must_use]
    pub fn bridge_fallbacks(&self) -> u32 {
        u32::try_from(self.bridges.iter().filter(|b| !b.drawn).count()).unwrap_or(u32::MAX)
    }

    /// The crossing nearest `at` (px) within `tolerance_px`, for the
    /// editor's right-click (P4-522).
    // APPROVER: new pub item
    #[must_use]
    pub fn bridge_near(&self, at: Point, tolerance_px: i32) -> Option<&SceneBridge> {
        let d2 = |b: &SceneBridge| {
            let (dx, dy) = (i64::from(b.at.x - at.x), i64::from(b.at.y - at.y));
            dx * dx + dy * dy
        };
        let tol = i64::from(tolerance_px.max(0));
        self.bridges.iter().filter(|b| d2(b) <= tol * tol).min_by_key(|b| d2(b))
    }

    /// The model id behind `key`: a box's element, a line's or label's
    /// winning relationship, a boundary's element; `None` for the title,
    /// subtitle, key, or a key not in this scene. Serves CORE-1, EDIT-5,
    /// EDIT-10.
    #[must_use]
    pub fn model_id(&self, key: ItemKey) -> Option<ModelId> {
        match key {
            ItemKey::Box(id) => self
                .boxes
                .get(id.0 as usize)
                .map(|b| ModelId::Element(b.element)),
            ItemKey::Line(edge) => self
                .line(edge)
                .map(|l| ModelId::Relationship(l.relationship)),
            ItemKey::Label(edge) => self
                .label(edge)
                .map(|l| ModelId::Relationship(l.relationship)),
            // DECISION: a group boundary has no model id: a group is not an
            // element or a relationship, and `ModelId` gains no variant in
            // phase 2 (pub surface frozen); the editor selects nothing.
            ItemKey::Boundary(i) => self.boundaries.get(i as usize).and_then(|b| match b.of {
                BoundaryOf::Element(e) | BoundaryOf::DeploymentNode(e) => Some(ModelId::Element(e)),
                BoundaryOf::Group(_) => None,
            }),
            ItemKey::Title | ItemKey::Subtitle | ItemKey::Legend => None,
        }
    }

    /// The line drawing `edge`.
    fn line(&self, edge: EdgeId) -> Option<&SceneLine> {
        self.lines.iter().find(|l| l.edge == edge)
    }

    /// The label of `edge`.
    fn label(&self, edge: EdgeId) -> Option<&SceneLabel> {
        self.labels.iter().find(|l| l.edge == edge)
    }

    /// The topmost item whose shape contains `at` (px), in reverse paint
    /// order: key, subtitle, title, labels, boxes (their outline), lines
    /// (within `tolerance_px` of a segment), boundaries (within
    /// `tolerance_px` of the dashed outline or on the tag). `None` on
    /// empty canvas. Serves EDIT-12, EDIT-28, EDIT-15.
    #[must_use]
    pub fn hit(&self, at: Point, tolerance_px: i32) -> Option<ItemKey> {
        let tol = tolerance_px.max(0);
        if self.legend.rect.contains(at) {
            return Some(ItemKey::Legend);
        }
        if self
            .subtitle
            .as_ref()
            .is_some_and(|t| t.bounds.contains(at))
        {
            return Some(ItemKey::Subtitle);
        }
        if self.title.as_ref().is_some_and(|t| t.bounds.contains(at)) {
            return Some(ItemKey::Title);
        }
        if let Some(l) = self.labels.iter().rev().find(|l| l.rect.contains(at)) {
            return Some(ItemKey::Label(l.edge));
        }
        if let Some(b) = self
            .boxes
            .iter()
            .rev()
            .find(|b| outline_contains(&b.outline, at))
        {
            return Some(ItemKey::Box(b.id));
        }
        if let Some(l) = self.lines.iter().rev().find(|l| line_near(l, at, tol)) {
            return Some(ItemKey::Line(l.edge));
        }
        self.boundaries
            .iter()
            .enumerate()
            .rev()
            .find(|(_, b)| b.tag_rect.contains(at) || outline_near(&b.rect, at, tol))
            .map(|(i, _)| ItemKey::Boundary(u32::try_from(i).unwrap_or(u32::MAX)))
    }

    /// The px rectangle of `key`'s item (a line: the bounds of its
    /// points and tip), for selection marks and rubber-band selection;
    /// `None` when not in this scene. Serves EDIT-12, EDIT-20.
    #[must_use]
    pub fn item_bounds(&self, key: ItemKey) -> Option<Rect> {
        match key {
            ItemKey::Box(id) => self.boxes.get(id.0 as usize).map(|b| b.rect),
            ItemKey::Line(edge) => self.line(edge).map(line_bounds),
            ItemKey::Label(edge) => self.label(edge).map(|l| l.rect),
            ItemKey::Title => self.title.as_ref().map(|t| t.bounds),
            ItemKey::Subtitle => self.subtitle.as_ref().map(|t| t.bounds),
            ItemKey::Legend => Some(self.legend.rect),
            ItemKey::Boundary(i) => self.boundaries.get(i as usize).map(|b| b.rect),
        }
    }

    /// Every box drawing `element`, in box order; phase 1 at most one.
    /// The editor re-finds a selection by element after a reload.
    /// Serves EDIT-1, EDIT-10.
    #[must_use]
    pub fn boxes_of(&self, element: ElementId) -> Vec<BoxId> {
        self.boxes
            .iter()
            .filter(|b| b.element == element)
            .map(|b| b.id)
            .collect()
    }

    /// Every line standing for `relationship` (as winner or merged), in
    /// edge order. Serves EDIT-1, EDIT-5.
    #[must_use]
    pub fn lines_of(&self, relationship: RelationshipId) -> Vec<EdgeId> {
        let mut out: Vec<EdgeId> = self
            .lines
            .iter()
            .filter(|l| l.relationships.contains(&relationship))
            .map(|l| l.edge)
            .collect();
        out.sort_unstable();
        out
    }

    /// The lines the router could not route, ascending (L1, CORE-13).
    #[must_use]
    pub fn unroutable(&self) -> Vec<EdgeId> {
        let mut out: Vec<EdgeId> = self
            .lines
            .iter()
            .filter(|l| !l.routed)
            .map(|l| l.edge)
            .collect();
        out.sort_unstable();
        out
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
        let mut routed: Vec<&SceneLine> = self.lines.iter().filter(|l| l.routed).collect();
        routed.sort_by_key(|l| l.edge);
        let mut out = Vec::new();
        for (i, a) in routed.iter().enumerate() {
            for b in &routed[i + 1..] {
                // The rule of `Routes::crossings`: a joined line's last
                // segment does not count against its own trunk.
                let n = if a.joins == Some(b.edge) {
                    crossings(&b.points, &a.points[..a.points.len() - 1])
                } else if b.joins == Some(a.edge) {
                    crossings(&a.points, &b.points[..b.points.len() - 1])
                } else {
                    crossings(&a.points, &b.points)
                };
                if n > 0 {
                    out.push((a.edge, b.edge, n));
                }
            }
        }
        out
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
        let i = id.0 as usize;
        assert!(
            i < self.boxes.len(),
            "move_box: {id:?} is not a box of this scene"
        );
        let old = self.centres[i];
        let (dx, dy) = ((centre.x - old.x) * GRID_PX, (centre.y - old.y) * GRID_PX);
        if (dx, dy) == (0, 0) {
            return;
        }
        let b = &mut self.boxes[i];
        b.rect = shift_rect(b.rect, dx, dy);
        b.outline = shift_outline(b.outline, dx, dy);
        if let Some(icon) = b.icon.as_mut() {
            icon.rect = shift_rect(icon.rect, dx, dy);
        }
        b.name.translate(dx, dy);
        b.type_line.translate(dx, dy);
        if let Some(d) = b.description.as_mut() {
            d.translate(dx, dy);
        }
        self.centres[i] = centre;
        let mut extent = Extent(Some(self.bounds));
        extent.add_rect(&b.rect.inflate(MARGIN_PX));
        self.bounds = extent.rect();
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
// DECISION: sized with the default type words (`Words::DEFAULT`): the
// signature has no terminology, so a longer `terminology` word than the
// default can still overflow at the editor's smallest size (L25 reports it).
#[must_use]
pub fn texts_fit(plan_box: &PlanBox, half_width: i32, half_height: i32) -> bool {
    let rect = Rect::from_centre(Point::default(), half_width, half_height);
    let texts = box_texts(plan_box, &px_rect(rect), &Words::DEFAULT);
    texts.name.fits && texts.type_line.fits && texts.description.as_ref().is_none_or(|d| d.fits)
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
    build_with(plan, routes, labels, &Words::DEFAULT)
}

/// `build` with type words, key line entries and type-line brackets from
/// `words` (`terminology`, P2-161..P2-163); `pipeline` calls it.
#[allow(clippy::too_many_lines)]
pub(crate) fn build_with(plan: &ViewPlan, routes: &Routes, labels: &Labels, words: &Words<'_>) -> Scene {
    assert_eq!(
        routes.lines.len(),
        plan.edges.len(),
        "scene::build: routes of another plan (lines)"
    );
    assert_eq!(
        labels.labels.len(),
        plan.edges.len(),
        "scene::build: labels of another plan"
    );
    assert!(
        routes.centres.len() >= plan.boxes.len(),
        "scene::build: routes of another plan (centres)"
    );
    let pal = palette(plan.mode);
    let centres: Vec<Point> = routes.centres[..plan.boxes.len()].to_vec();
    let boxes: Vec<SceneBox> = plan
        .boxes
        .iter()
        .map(|b| scene_box(b, centres[b.id.0 as usize], words))
        .collect();
    let boundaries: Vec<SceneBoundary> = (0..plan.boundaries.len())
        .filter_map(|i| scene_boundary(plan, i, &centres, pal, words))
        .collect();
    let labels: Vec<SceneLabel> = labels
        .labels
        .iter()
        .flatten()
        .map(|l| scene_label(plan, l, &boundaries, pal))
        .collect();
    let label_rects = chevron_keepouts(&labels);
    let mut lines: Vec<SceneLine> = plan
        .edges
        .iter()
        .map(|e| scene_line(e, routes, &boxes, &label_rects, pal))
        .collect();
    add_jumps(plan, &mut lines);
    let bridges = crate::bridge::add(
        plan.options.crossings,
        &crate::bridge::Around {
            written: &plan.bridges,
            bundles: &routes.bundles,
            boxes: &boxes,
            labels: &labels,
        },
        &mut lines,
        routes.crossings,
    );
    let image = plan.image.clone().map(scene_image);
    let mut extent = Extent::default();
    if let Some(image) = &image {
        extent.add_rect(&image.rect);
    }
    for b in &boxes {
        extent.add_rect(&b.rect);
    }
    for l in &lines {
        extent.add_rect(&line_bounds(l));
    }
    for l in &labels {
        add_label(&mut extent, l);
    }
    for b in &boundaries {
        extent.add_rect(&b.rect);
    }
    let drawing = extent.rect();
    let mut below = drawing.max.y + TITLE_GAP_PX;
    let title = plan.title.as_ref().map(|t| {
        let block = chrome_text(t, TITLE_FONT, drawing.min.x, below, pal.ink);
        below = block.bounds.max.y;
        block
    });
    let subtitle = plan.subtitle.as_ref().map(|t| {
        let block = chrome_text(t, SUBTITLE_FONT, drawing.min.x, below, pal.ink_2);
        below = block.bounds.max.y;
        block
    });
    let mut legend = legend(looks(plan, &boxes, &lines, words), &drawing, below + KEY_GAP_PX);
    for entry in &mut legend.entries {
        entry.text.colour = pal.ink_2;
    }
    for r in [
        title.as_ref().map(|t| t.bounds),
        subtitle.as_ref().map(|t| t.bounds),
    ]
    .into_iter()
    .flatten()
    .chain([legend.rect])
    {
        extent.add_rect(&r);
    }
    let bounds = extent.rect().inflate(MARGIN_PX);
    Scene {
        key: plan.key.clone(),
        kind: plan.kind,
        scope: plan.scope.clone(),
        options: plan.options,
        bounds,
        background: pal.page,
        centres,
        boxes,
        lines,
        labels,
        boundaries,
        title,
        subtitle,
        legend,
        crossings: routes.crossings,
        mode: plan.mode,
        image,
        bundles: routes.bundles.clone(),
        bridges,
    }
}

/// The px size of a placeholder (P2-321).
const PLACEHOLDER_PX: (i32, i32) = (480, 160);
/// The px size of an image whose size cannot be read from its bytes.
const IMAGE_FALLBACK_PX: (i32, i32) = (400, 300);
/// The largest drawn image side, px.
const IMAGE_MAX_PX: i32 = 20_000;

/// The scene picture of an image view: the file at its intrinsic size
/// with its top left at the origin, or a placeholder naming the source.
// TODO(srs): P2-322 (c), the drawn size. DECISION: reading taken (B
// landscape-filtered 10c): the image's intrinsic size in px (PNG and JPEG
// headers, SVG `width`/`height`, else `viewBox`), unscaled; the title and
// subtitle sit below it, as on every other view (Structurizr draws the
// title below the diagram too; B said above).
fn scene_image(image: ViewImage) -> SceneImage {
    let placeholder = match (&image.source, &image.asset) {
        (ImageSource::File(_), Some(_)) => None,
        (ImageSource::File(f), None) => Some(format!("image {}", f.value)),
        (ImageSource::Url(u), _) => Some(format!("image {}", u.value)),
        (ImageSource::PlantUml(v), _) => Some(format!("plantuml {}", v.value)),
        (ImageSource::Mermaid(v), _) => Some(format!("mermaid {}", v.value)),
        (ImageSource::Kroki { format, source }, _) => {
            Some(format!("kroki {} {}", format.value, source.value))
        }
    };
    let (w, h) = match (&placeholder, &image.asset) {
        (None, Some(asset)) => intrinsic_size(asset.mime, &asset.bytes).unwrap_or(IMAGE_FALLBACK_PX),
        _ => PLACEHOLDER_PX,
    };
    SceneImage {
        rect: Rect::from_corners(Point::new(0, 0), Point::new(w, h)),
        image,
        placeholder,
    }
}

impl SceneImage {
    /// The file drawn, `None` for a placeholder.
    pub(crate) fn asset_drawn(&self) -> Option<&Asset> {
        self.image.asset.as_ref().filter(|_| self.placeholder.is_none())
    }
}

/// The text of a placeholder, centred in its rectangle; `None` when the
/// picture itself is drawn. `export` draws it.
pub(crate) fn placeholder_text(image: &SceneImage) -> Option<TextBlock> {
    let text = image.placeholder.as_ref()?;
    let font = SUBTITLE_FONT;
    let mut block = text_block(
        vec![format!("{text} (not drawn)")],
        Font {
            size_px: 14,
            line_px: 18,
            weight: font.weight,
        },
        palette(Mode::Light).ink_2,
        middle(image.rect.centre().x, image.rect.centre().y - 9),
        &image.rect,
    );
    block.fits = true;
    Some(block)
}

/// The width and height in px an image file declares: the PNG `IHDR`, the
/// JPEG start-of-frame, the SVG root's `width`/`height` (px or unitless)
/// or else its `viewBox`. `None` when they cannot be read.
pub(crate) fn intrinsic_size(mime: Mime, bytes: &[u8]) -> Option<(i32, i32)> {
    let clamp = |w: f64, h: f64| -> Option<(i32, i32)> {
        let fit = |v: f64| {
            // DECISION: sizes are rounded to whole px and kept within
            // 1..=20000, so a broken header cannot make a huge canvas.
            let r = v.round().clamp(1.0, f64::from(IMAGE_MAX_PX));
            #[expect(clippy::cast_possible_truncation, reason = "clamped to 1..=20000")]
            let px = r as i32;
            px
        };
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then(|| (fit(w), fit(h)))
    };
    match mime {
        Mime::Png => {
            let w = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
            let h = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
            clamp(f64::from(w), f64::from(h))
        }
        Mime::Jpeg => {
            let mut i = 2;
            while i + 9 < bytes.len() {
                if bytes[i] != 0xff {
                    i += 1;
                    continue;
                }
                let marker = bytes[i + 1];
                let len = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
                let frame = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
                if frame {
                    let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]);
                    let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]);
                    return clamp(f64::from(w), f64::from(h));
                }
                i += 2 + len;
            }
            None
        }
        _ => svg_size(std::str::from_utf8(bytes).ok()?).and_then(|(w, h)| clamp(w, h)),
    }
}

/// `width`/`height` of the `<svg>` root in px, else its `viewBox` size.
fn svg_size(text: &str) -> Option<(f64, f64)> {
    let start = text.find("<svg")?;
    let end = start + text[start..].find('>')?;
    let tag = &text[start..end];
    let attr = |name: &str| -> Option<&str> {
        let mut rest = tag;
        loop {
            let at = rest.find(name)?;
            let before = rest[..at].chars().last();
            let after = rest[at + name.len()..].trim_start();
            rest = &rest[at + name.len()..];
            if before.is_some_and(char::is_whitespace) && after.starts_with('=') {
                let value = after[1..].trim_start();
                let quote = value.chars().next()?;
                if quote != '"' && quote != '\'' {
                    return None;
                }
                let value = &value[1..];
                return value.find(quote).map(|e| &value[..e]);
            }
        }
    };
    let px = |v: &str| -> Option<f64> { v.trim().strip_suffix("px").unwrap_or(v.trim()).parse().ok() };
    if let (Some(w), Some(h)) = (attr("width").and_then(px), attr("height").and_then(px)) {
        return Some((w, h));
    }
    let numbers: Vec<f64> = attr("viewBox")?
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect();
    match numbers[..] {
        [_, _, w, h] => Some((w, h)),
        _ => None,
    }
}

// ---------------------------------------------------------------- sizes
//
// DECISION: the px values CORE-1, CORE-2 and CORE-14 do not give, from
// the prototype golden SVGs (`c4.py` `render`) rounded to whole px: box
// corner radius 6, person head radius 30 with the body from 50px below
// the top (corner radius 22; redesign 2026-11-19: head 24, body from
// 30px, corners 14, see `HEAD_RADIUS_PX`), cylinder caps 12px tall, box outline 1px
// in the fill darkened to 72%, boundary 1px dashed 8/6 in #666666
// (redesign 2026-11-19: 6/4, see `BOUNDARY_DASH_PX`), label backing
// white, label text in the destination fill darkened to 62%. Line
// pitches the SRS leaves open: box name 18px, title 22px, subtitle 16px
// (redesign 2026-11-19: 20px and 14px, see `TITLE_FONT`), boundary tag
// 15px, key text 14px.

/// The colours a scene draws with where no style gives one (P2-431).
#[derive(Clone, Copy)]
struct Palette {
    /// `Scene::background`, and the label backing (`paper`).
    page: Colour,
    /// An unstyled line, its arrowhead, port and step disc (`dg-line`).
    line: Colour,
    /// Label text and the title (`dg-ink`).
    ink: Colour,
    /// Subtitle, key text, boundary tag (`dg-ink-2`).
    ink_2: Colour,
    /// An unstyled boundary's outline (`dg-boundary`).
    boundary: Colour,
}

/// `#rrggbb` as a colour, for the token table.
const fn hex(v: u32) -> Colour {
    let [_, r, g, b] = v.to_be_bytes();
    Colour { r, g, b }
}

/// The palette of `mode`.
// redesign 2026-11-19: the owner-approved redesign's diagram tokens
// (design-system tokens.json: `paper`, `dg-line`, `dg-ink`, `dg-ink-2`,
// `dg-boundary`; light is the default, dark the editor's Dark mode).
// Lines default to `dg-line` in both modes instead of the destination's
// fill (CORE-3, conflict noted in docs/deferred.md); a relationship style
// still wins. Supersedes the TODO(srs) P2-431 reading (B styles-themes
// 10c: dark page #111111, lines and boundaries #aaaaaa).
const fn palette(mode: Mode) -> Palette {
    match mode {
        Mode::Dark => Palette {
            page: hex(0x1e_2124),
            line: hex(0x8f_a0b8),
            ink: hex(0xe8_eaec),
            ink_2: hex(0x9a_a2a8),
            boundary: hex(0x73_7c83),
        },
        _ => Palette {
            page: hex(0xff_ffff),
            line: hex(0x4a_5a70),
            ink: hex(0x14_171a),
            ink_2: hex(0x5a_6268),
            boundary: hex(0x7b_848b),
        },
    }
}

/// The technology line's colour of a label drawn in `text` on a scene
/// of `mode`: `dg-ink-2` under default `dg-ink` text, else the text's
/// own colour. Both renderers draw the last, bracketed line of a label
/// with it (redesign 2026-11-19, Edge: "technology under it in brackets
/// in `dg-ink-2`").
// DECISION: computed by the renderers from the scene's mode and the
// label's colour, so `SceneLabel` keeps one text colour (pub layout
// unchanged).
#[must_use]
pub(crate) fn label_tech_colour(text: Colour, mode: Mode) -> Colour {
    let pal = palette(mode);
    if text == pal.ink { pal.ink_2 } else { text }
}

/// The step badge of a dynamic-view label (design-system Edge, "Step"):
/// a filled `dg-line` disc of this radius at the left of the text.
pub(crate) const STEP_DISC_RADIUS_PX: i32 = 8;
/// Between the step disc and the text column.
pub(crate) const STEP_DISC_GAP_PX: i32 = 4;
/// The number inside the disc, bold, in the paper colour.
pub(crate) const STEP_NUMBER_FONT_PX: i32 = 10;
/// From a step label's first baseline up to the disc's centre.
pub(crate) const STEP_DISC_RISE_PX: i32 = 3;
/// From the disc's centre down to the number's baseline.
pub(crate) const STEP_NUMBER_DROP_PX: i32 = 4;

/// A dynamic-view label's first line split into its step number and the
/// rest of the line: `"2: Reads"` gives `("2", "Reads")`, `"2:"` gives
/// `("2", "")`. `None` when the line does not start with `<number>:`.
// DECISION: the renderers know a label is a step label from
// `Scene::kind == ViewKind::Dynamic` plus this split: in a dynamic view
// every plan edge is a step (`view::dynamic_plan`), its description is
// `<order>: <text>` (P3-211), the order is one DSL word (no whitespace),
// and label wrapping never breaks a word, so the first line always starts
// with the whole `<order>:`. No pub field is needed, the scene text stays
// as P3-211 pins it, and the editor's canvas repeats this rule.
pub(crate) fn step_number(line: &str) -> Option<(&str, &str)> {
    let (head, rest) = line.split_once(' ').unwrap_or((line, ""));
    let number = head.strip_suffix(':')?;
    (!number.is_empty()).then_some((number, rest))
}

/// The width in px of a step label drawn as disc, gap and text column
/// (the first line without its `<number>: `, then the other lines), or
/// `None` when `lines` is no step label.
pub(crate) fn step_label_width(lines: &[String], size_px: i32) -> Option<i32> {
    let (_, rest) = step_number(lines.first()?)?;
    let column = std::iter::once(rest)
        .chain(lines[1..].iter().map(String::as_str))
        .map(|l| text_px(l, size_px, Weight::Regular))
        .max()
        .unwrap_or(0);
    let disc = 2 * STEP_DISC_RADIUS_PX;
    Some(if column > 0 { disc + STEP_DISC_GAP_PX + column } else { disc })
}

/// Margin of `Scene::bounds` around everything drawn.
const MARGIN_PX: i32 = 40;
const PAD_PX: i32 = 10;
const NAME_PX: i32 = 14;
const NAME_LINE_PX: i32 = 18;
const SMALL_PX: i32 = 11;
const SMALL_LINE_PX: i32 = 14;
// DECISION: 6px between the type line and the description, as the
// prototype (`node_svg`: type at +15, description at +35 on a 14px pitch).
const DESC_GAP_PX: i32 = 6;
const BOX_RADIUS_PX: i32 = 6;
// redesign 2026-11-19: the person of design-system Node (preview: body
// `rx="14"`, head `r="24"` centred 6px above the body's top), was a 30px
// head over a body from 50px down with 22px corners.
// DECISION: the head's top is the box's top, so the body starts 30px
// down (24 + 6) and the head reaches 18px into it, as in the preview.
const HEAD_RADIUS_PX: i32 = 24;
const BODY_TOP_PX: i32 = 30;
const BODY_RADIUS_PX: i32 = 14;
// redesign 2026-11-19: a 9px lid (design-system Node: "a cylinder has a
// 9 px lid"), was 12px from the prototype.
const CAP_RY_PX: i32 = 9;
const OUTLINE_PX: i32 = 1;

/// A grid rectangle in px.
const fn px_rect(r: Rect) -> Rect {
    Rect {
        min: px_point(r.min),
        max: px_point(r.max),
    }
}

/// A grid point in px.
const fn px_point(p: Point) -> Point {
    Point::new(p.x * GRID_PX, p.y * GRID_PX)
}

/// `colour` with each channel scaled by `num / 100`, rounded down (the
/// prototype's `darker`).
fn darker(colour: Colour, num: u16) -> Colour {
    let scale = |c: u8| u8::try_from(u16::from(c) * num / 100).unwrap_or(u8::MAX);
    Colour {
        r: scale(colour.r),
        g: scale(colour.g),
        b: scale(colour.b),
    }
}

// ----------------------------------------------------------------- text

/// Width in px of `text` at `size_px`, rounded up (CORE-30 table).
fn text_px(text: &str, size_px: i32, weight: Weight) -> i32 {
    (text_advance(text, weight) * size_px + EM - 1) / EM
}

/// `text` wrapped greedily at word boundaries to `max_px` wide at
/// `size_px`; never empty (an empty text is one empty line).
// DECISION: a word wider than `max_px` stays whole on its own line,
// never broken mid-word: the text then does not fit (L25), which is
// what CORE-30 reports rather than hides.
fn wrap(text: &str, size_px: i32, weight: Weight, max_px: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
        } else {
            let candidate = format!("{cur} {word}");
            if text_px(&candidate, size_px, weight) > max_px {
                lines.push(std::mem::replace(&mut cur, word.to_owned()));
            } else {
                cur = candidate;
            }
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Font of one text block: size, line pitch, weight.
#[derive(Clone, Copy)]
struct Font {
    size_px: i32,
    line_px: i32,
    weight: Weight,
}

const NAME_FONT: Font = Font {
    size_px: NAME_PX,
    line_px: NAME_LINE_PX,
    weight: Weight::Bold,
};
const SMALL_FONT: Font = Font {
    size_px: SMALL_PX,
    line_px: SMALL_LINE_PX,
    weight: Weight::Regular,
};

impl Font {
    /// Baseline of a line below the top of its `line_px` band.
    // DECISION: the cap height (about 0.7 em in Inter) is centred in the
    // band, so `TextBlock::bounds` is the stack of bands and a block's
    // first baseline is `bounds.min.y + baseline_px()`.
    const fn baseline_px(self) -> i32 {
        i32::midpoint(self.line_px, self.size_px * 7 / 10)
    }
}

/// Where a text block goes: aligned on `x` by `align`, its first band's
/// top at `top`.
#[derive(Clone, Copy)]
struct Anchor {
    x: i32,
    top: i32,
    align: Align,
}

const fn middle(x: i32, top: i32) -> Anchor {
    Anchor {
        x,
        top,
        align: Align::Middle,
    }
}

const fn start(x: i32, top: i32) -> Anchor {
    Anchor {
        x,
        top,
        align: Align::Start,
    }
}

/// A text block of `lines` in `font` at `anchor`, fitting when its
/// bounds lie in `area`.
fn text_block(
    lines: Vec<String>,
    font: Font,
    colour: Colour,
    anchor: Anchor,
    area: &Rect,
) -> TextBlock {
    let Anchor { x, top, align } = anchor;
    let width = lines
        .iter()
        .map(|l| text_px(l, font.size_px, font.weight))
        .max()
        .unwrap_or(0);
    let left = match align {
        Align::Middle => x - width / 2,
        Align::Start => x,
    };
    let n = i32::try_from(lines.len()).unwrap_or(i32::MAX);
    let bounds = Rect::from_corners(
        Point::new(left, top),
        Point::new(left + width, top + n * font.line_px),
    );
    TextBlock {
        lines,
        at: Point::new(x, top + font.baseline_px()),
        align,
        size_px: font.size_px,
        line_px: font.line_px,
        weight: font.weight,
        colour,
        bounds,
        fits: area.contains(bounds.min) && area.contains(bounds.max),
    }
}

impl TextBlock {
    fn translate(&mut self, dx: i32, dy: i32) {
        self.at = shift(self.at, dx, dy);
        self.bounds = shift_rect(self.bounds, dx, dy);
    }
}

const fn shift(p: Point, dx: i32, dy: i32) -> Point {
    Point::new(p.x + dx, p.y + dy)
}

const fn shift_rect(r: Rect, dx: i32, dy: i32) -> Rect {
    Rect {
        min: shift(r.min, dx, dy),
        max: shift(r.max, dx, dy),
    }
}

// ---------------------------------------------------------------- boxes

/// The three texts of a box.
struct BoxTexts {
    name: TextBlock,
    type_line: TextBlock,
    description: Option<TextBlock>,
}

/// The CORE-2 type line text: `[type: technology]` or `[type]`.
fn type_text(b: &PlanBox, words: &Words<'_>) -> String {
    // TODO(srs): P2-319, the type line of a custom element. DECISION:
    // reading taken (B landscape-filtered 12), checked against Structurizr
    // 2026.10.26 (`structurizr-diagram.js` `formatMetaData`): its metadata
    // in brackets, which `view` puts in `technology` (P2-170), else no
    // type line at all.
    let Some(kind) = words.kind(b.kind, b.external) else {
        return match b.technology.as_deref().map(str::trim) {
            Some(meta) if !meta.is_empty() => words.bracket(meta),
            _ => String::new(),
        };
    };
    match b.technology.as_deref().map(str::trim) {
        Some(tech) if !tech.is_empty() => words.bracket(&format!("{kind}: {tech}")),
        _ => words.bracket(&kind),
    }
}

/// The px sizes an outline is drawn with: the box's or the key swatch's.
#[derive(Clone, Copy)]
struct Dims {
    box_radius: i32,
    rounded_radius: i32,
    cap: i32,
    bar: i32,
    inset: i32,
    tab: i32,
    component_tab: (i32, i32),
}

// redesign 2026-11-19: RoundedBox corners `radius-node-round` 14px
// (tokens.json; Node preview `rx="14"`), was 20px (B ruling 3). P2-415
// still holds: 14 > the Box's 6.
// DECISION: shape proportions (P2-413 gives none, B ruling 6 says "per
// the notation page"): Box 6px and RoundedBox 14px corners;
// Circle the largest centred circle; Ellipse the inscribed ellipse;
// Hexagon points a quarter of the height in from the sides; Bucket a
// cylinder narrowing by a tenth of the width per side; Pipe 12px caps;
// Robot a head half the width and a quarter of the height; Folder a tab a
// third of the width and 10px tall; the four window shapes a 20px bar;
// mobile devices a 10px bezel; Component two 20x10 tabs straddling a body
// 10px in from the left edge.
const BOX_DIMS: Dims = Dims {
    box_radius: BOX_RADIUS_PX,
    rounded_radius: 14,
    cap: CAP_RY_PX,
    bar: 20,
    inset: 10,
    tab: 10,
    component_tab: (20, 10),
};

// redesign 2026-11-19: the key swatch is 18x12 with 2px corners
// (design-system Legend, `.dgh-sw`); the other proportions shrink with it.
const SWATCH_DIMS: Dims = Dims {
    box_radius: 2,
    rounded_radius: 6,
    cap: 3,
    bar: 4,
    inset: 2,
    tab: 3,
    component_tab: (6, 3),
};

/// The screen of an `Outline::MobileDevice` (P2-11): the bezel is
/// `inset_px` on the device's long sides and twice that on its short
/// sides (speaker and button), so a portrait phone's bezel is deep at
/// top and bottom and a landscape phone's at left and right, in any box.
// DECISION: portrait and landscape share the box size and differ by
// where the deep bezel is, so both read as phones in a 240x140 box.
pub(crate) fn mobile_screen(rect: Rect, portrait: bool, inset_px: i32) -> Rect {
    let (dx, dy) = if portrait { (inset_px, 2 * inset_px) } else { (2 * inset_px, inset_px) };
    Rect::from_corners(
        Point::new(rect.min.x + dx, rect.min.y + dy),
        Point::new(rect.max.x - dx, rect.max.y - dy),
    )
}

/// The outline of a box of `shape` filling `rect` (px).
fn outline(shape: Shape, rect: Rect) -> Outline {
    match shape {
        Shape::Person => {
            let head_centre = Point::new(rect.centre().x, rect.min.y + HEAD_RADIUS_PX);
            let body_top = (rect.min.y + BODY_TOP_PX).min(rect.max.y);
            Outline::Person {
                head_centre,
                head_radius_px: HEAD_RADIUS_PX,
                body: Rect::from_corners(Point::new(rect.min.x, body_top), rect.max),
                body_radius_px: BODY_RADIUS_PX,
            }
        }
        _ => shape_outline(shape, rect, BOX_DIMS),
    }
}

/// The outline of every shape but the person's, in `rect` with `d`.
fn shape_outline(shape: Shape, rect: Rect, d: Dims) -> Outline {
    let (w, h) = (rect.width(), rect.height());
    let clamp_r = |r: i32| r.min(w / 2).min(h / 2).max(0);
    match shape {
        Shape::Cylinder => Outline::Cylinder {
            rect,
            cap_ry_px: d.cap.min(h / 4),
        },
        Shape::RoundedBox => Outline::Rect {
            rect,
            radius_px: clamp_r(d.rounded_radius),
        },
        Shape::Circle => {
            let side = w.min(h);
            let c = rect.centre();
            let min = Point::new(c.x - side / 2, c.y - side / 2);
            Outline::Ellipse {
                rect: Rect::from_corners(min, shift(min, side, side)),
                circle: true,
            }
        }
        Shape::Ellipse => Outline::Ellipse {
            rect,
            circle: false,
        },
        Shape::Hexagon => Outline::Hexagon {
            rect,
            inset_px: h / 4,
        },
        Shape::Diamond => Outline::Diamond { rect },
        Shape::Bucket => Outline::Bucket {
            rect,
            cap_ry_px: d.cap.min(h / 4),
        },
        Shape::Pipe => Outline::Pipe {
            rect,
            cap_rx_px: d.cap.min(w / 4),
        },
        Shape::Robot => {
            let head_h = h / 4;
            let head_min = Point::new(rect.centre().x - w / 4, rect.min.y);
            Outline::Robot {
                head: Rect::from_corners(head_min, shift(head_min, w / 2, head_h)),
                body: Rect::from_corners(Point::new(rect.min.x, rect.min.y + head_h), rect.max),
                radius_px: clamp_r(d.box_radius),
            }
        }
        Shape::Folder => {
            let tab_h = d.tab.min(h / 4);
            Outline::Folder {
                rect: Rect::from_corners(Point::new(rect.min.x, rect.min.y + tab_h), rect.max),
                tab: Rect::from_corners(rect.min, shift(rect.min, w / 3, tab_h)),
            }
        }
        Shape::WebBrowser | Shape::Window | Shape::Terminal | Shape::Shell => Outline::Window {
            rect,
            bar_px: d.bar.min(h / 3),
            kind: match shape {
                Shape::WebBrowser => WindowKind::WebBrowser,
                Shape::Window => WindowKind::Window,
                Shape::Terminal => WindowKind::Terminal,
                _ => WindowKind::Shell,
            },
        },
        Shape::MobileDevicePortrait | Shape::MobileDeviceLandscape => Outline::MobileDevice {
            rect,
            portrait: shape == Shape::MobileDevicePortrait,
            inset_px: d.inset.min(w / 4).min(h / 4),
        },
        Shape::Component => {
            let (tw, th) = (d.component_tab.0.min(w / 4), d.component_tab.1.min(h / 6));
            let body = Rect::from_corners(Point::new(rect.min.x + tw / 2, rect.min.y), rect.max);
            let tab = |top: i32| Rect::from_corners(Point::new(rect.min.x, top), Point::new(rect.min.x + tw, top + th));
            let (q1, q2) = (rect.min.y + h / 4 - th / 2, rect.min.y + h / 2);
            Outline::Component {
                rect: body,
                tabs: [tab(q1), tab(q2.max(q1 + th + 1))],
            }
        }
        _ => Outline::Rect {
            rect,
            radius_px: clamp_r(d.box_radius),
        },
    }
}

/// `outline` moved by `(dx, dy)` px.
fn shift_outline(outline: Outline, dx: i32, dy: i32) -> Outline {
    let r = |rect: Rect| shift_rect(rect, dx, dy);
    match outline {
        Outline::Rect { rect, radius_px } => Outline::Rect {
            rect: r(rect),
            radius_px,
        },
        Outline::Person {
            head_centre,
            head_radius_px,
            body,
            body_radius_px,
        } => Outline::Person {
            head_centre: shift(head_centre, dx, dy),
            head_radius_px,
            body: r(body),
            body_radius_px,
        },
        Outline::Cylinder { rect, cap_ry_px } => Outline::Cylinder {
            rect: r(rect),
            cap_ry_px,
        },
        Outline::Ellipse { rect, circle } => Outline::Ellipse {
            rect: r(rect),
            circle,
        },
        Outline::Hexagon { rect, inset_px } => Outline::Hexagon {
            rect: r(rect),
            inset_px,
        },
        Outline::Diamond { rect } => Outline::Diamond { rect: r(rect) },
        Outline::Bucket { rect, cap_ry_px } => Outline::Bucket {
            rect: r(rect),
            cap_ry_px,
        },
        Outline::Pipe { rect, cap_rx_px } => Outline::Pipe {
            rect: r(rect),
            cap_rx_px,
        },
        Outline::Robot {
            head,
            body,
            radius_px,
        } => Outline::Robot {
            head: r(head),
            body: r(body),
            radius_px,
        },
        Outline::Folder { rect, tab } => Outline::Folder {
            rect: r(rect),
            tab: r(tab),
        },
        Outline::Window { rect, bar_px, kind } => Outline::Window {
            rect: r(rect),
            bar_px,
            kind,
        },
        Outline::MobileDevice {
            rect,
            portrait,
            inset_px,
        } => Outline::MobileDevice {
            rect: r(rect),
            portrait,
            inset_px,
        },
        Outline::Component { rect, tabs } => Outline::Component {
            rect: r(rect),
            tabs: [r(tabs[0]), r(tabs[1])],
        },
    }
}

/// `rect` scaled about its centre by `num / 1000` per axis.
fn scaled_about_centre(rect: Rect, num: i32) -> Rect {
    let c = rect.centre();
    let (hw, hh) = (rect.width() * num / 2000, rect.height() * num / 2000);
    Rect::from_corners(Point::new(c.x - hw, c.y - hh), Point::new(c.x + hw, c.y + hh))
}

/// The rectangle a box's texts must fit (CORE-30, P2-413): the part of
/// its outline free of head, caps, bars, tabs and slanted sides, less
/// the 10px padding.
fn text_area(shape: Shape, rect: Rect) -> Rect {
    let inner = match outline(shape, rect) {
        // redesign 2026-11-19: the head reaches into the body, so a
        // person's texts start below the head (Node preview: the text is
        // centred between the head's bottom and the body's bottom).
        Outline::Person {
            head_centre,
            head_radius_px,
            body,
            ..
        } => Rect::from_corners(
            Point::new(body.min.x, (head_centre.y + head_radius_px).clamp(body.min.y, body.max.y)),
            body.max,
        ),
        Outline::Robot { body, .. } => body,
        Outline::Cylinder { rect, cap_ry_px } => Rect::from_corners(
            Point::new(rect.min.x, (rect.min.y + 2 * cap_ry_px).min(rect.max.y)),
            Point::new(rect.max.x, (rect.max.y - cap_ry_px).max(rect.min.y)),
        ),
        Outline::Rect { rect, .. } | Outline::Folder { rect, .. } => rect,
        // 707/1000 ~ 1/sqrt(2): the largest rectangle of the same aspect
        // inside an ellipse.
        Outline::Ellipse { rect, .. } => scaled_about_centre(rect, 707),
        Outline::Hexagon { rect, inset_px } => Rect::from_corners(
            Point::new(rect.min.x + inset_px, rect.min.y),
            Point::new(rect.max.x - inset_px, rect.max.y),
        ),
        Outline::Diamond { rect } => scaled_about_centre(rect, 500),
        Outline::Bucket { rect, cap_ry_px } => {
            let narrow = rect.width() / 10;
            Rect::from_corners(
                Point::new(rect.min.x + narrow, (rect.min.y + 2 * cap_ry_px).min(rect.max.y)),
                Point::new(rect.max.x - narrow, rect.max.y),
            )
        }
        Outline::Pipe { rect, cap_rx_px } => Rect::from_corners(
            Point::new(rect.min.x + cap_rx_px, rect.min.y),
            Point::new((rect.max.x - 2 * cap_rx_px).max(rect.min.x), rect.max.y),
        ),
        Outline::Window { rect, bar_px, .. } => {
            Rect::from_corners(Point::new(rect.min.x, rect.min.y + bar_px), rect.max)
        }
        Outline::MobileDevice {
            rect,
            portrait,
            inset_px,
        } => mobile_screen(rect, portrait, inset_px),
        Outline::Component { rect, tabs } => {
            Rect::from_corners(Point::new(tabs[0].max.x, rect.min.y), rect.max)
        }
    };
    inner.inflate(-PAD_PX)
}

/// The name and small-text fonts of a box styled with `font_size`
/// (P2-407): `None` keeps phase 1 (14px on 18, 11px on 14).
// TODO(srs): P2-407. Reading taken (B styles-themes 5): name N px on an
// N+6 px pitch; type line and description round(N x 11/14) px on that
// size plus 4.
fn box_fonts(font_size: Option<u32>) -> (Font, Font) {
    let Some(n) = font_size else {
        return (NAME_FONT, SMALL_FONT);
    };
    let n = i32::try_from(n).unwrap_or(i32::MAX / 2).clamp(1, 1000);
    let small = (n * 11 + 7) / 14;
    (
        Font {
            size_px: n,
            line_px: n + 6,
            weight: Weight::Bold,
        },
        Font {
            size_px: small,
            line_px: small + 4,
            weight: Weight::Regular,
        },
    )
}

/// Name, type line and description of `b` wrapped to the text area of
/// a box at `rect` (px) and stacked, centred vertically in it (top
/// aligned when they overflow it). `metadata false` gives a type line of
/// no lines, `description false` no description (P2-408).
// DECISION: a hidden type line is a `TextBlock` with no lines and zero
// height (the field is not an `Option`); painters draw nothing for it and
// it always fits.
// DECISION: a styled `fontSize` that overflows the text area steps down
// one px at a time until the stack fits (floor: the phase-1 14px), so the
// description never clips under the box edge and box sizes (data layout,
// placement) stay as they were.
fn box_texts(b: &PlanBox, rect: &Rect, words: &Words<'_>) -> BoxTexts {
    let Some(n) = b.style.font_size else {
        return box_texts_at(b, rect, words, None).0;
    };
    let mut size = n;
    loop {
        let (texts, fits) = box_texts_at(b, rect, words, Some(size));
        if fits || size <= 14 {
            return texts;
        }
        size -= 1;
    }
}

/// `box_texts` at `font_size`, and whether the stack fits the text area.
fn box_texts_at(
    b: &PlanBox,
    rect: &Rect,
    words: &Words<'_>,
    font_size: Option<u32>,
) -> (BoxTexts, bool) {
    let mut area = text_area(b.style.shape, *rect);
    if b.style.icon.is_some() {
        // The texts stay above the icon (P2-409).
        let icon = icon_rect(b.style.shape, *rect);
        area.max.y = (icon.min.y - ICON_GAP_PX).max(area.min.y);
    }
    let width = area.width();
    let (name_font, small_font) = box_fonts(font_size);
    let name = wrap(&b.name, name_font.size_px, Weight::Bold, width);
    let text = type_text(b, words);
    let type_line = if b.style.metadata && !text.is_empty() {
        wrap(&text, small_font.size_px, Weight::Regular, width)
    } else {
        Vec::new()
    };
    let description = b
        .description
        .as_deref()
        .filter(|_| b.style.description)
        .map(|d| wrap(d, small_font.size_px, Weight::Regular, width));
    let count = |v: &Vec<String>| i32::try_from(v.len()).unwrap_or(i32::MAX);
    let name_h = count(&name) * name_font.line_px;
    let type_h = count(&type_line) * small_font.line_px;
    let desc_h = description
        .as_ref()
        .map_or(0, |d| DESC_GAP_PX + count(d) * small_font.line_px);
    let block = name_h + type_h + desc_h;
    let top = area.min.y + ((area.height() - block) / 2).max(0);
    let x = area.centre().x;
    let colour = b.style.color;
    let mut type_block = text_block(
        type_line,
        small_font,
        colour,
        middle(x, top + name_h),
        &area,
    );
    if type_block.lines.is_empty() {
        type_block.fits = true;
    }
    let texts = BoxTexts {
        name: text_block(name, name_font, colour, middle(x, top), &area),
        type_line: type_block,
        description: description.map(|d| {
            let top = top + name_h + type_h + DESC_GAP_PX;
            text_block(d, small_font, colour, middle(x, top), &area)
        }),
    };
    let fits = block <= area.height()
        && texts.name.fits
        && texts.type_line.fits
        && texts.description.as_ref().is_none_or(|d| d.fits);
    (texts, fits)
}

/// Space between an icon and the texts above it.
const ICON_GAP_PX: i32 = 4;

/// Where a box of `shape` at `rect` draws its icon (P2-409): a square
/// centred at the bottom of its text area, at most a quarter of the box
/// height and half the text area's height.
// TODO(srs): P2-409 icon placement. Reading taken (B styles-themes 7a):
// centred in the box's bottom band, at most 1/4 of the box height, clear
// of the texts.
fn icon_rect(shape: Shape, rect: Rect) -> Rect {
    let area = text_area(shape, rect);
    let side = (rect.height() / 4)
        .min(area.height() / 2)
        .min(area.width())
        .max(0);
    let x = rect.centre().x - side / 2;
    Rect::from_corners(Point::new(x, area.max.y - side), Point::new(x + side, area.max.y))
}

/// The dash array of a box outline or line drawn `border` (P2-405,
/// P2-420); `None` for solid.
// DECISION: dashed is 8/6 (the phase-1 boundary dash), dotted 2/4:
// shorter than the dash, and still visible as dots at the 1px default
// stroke width.
// DECISION (redesign 2026-11-19): a styled `border dashed` keeps 8/6 on
// boxes, lines and boundaries alike (P2-405, P2-420 test it); only the
// unstyled boundary takes the design's 6/4 (`BOUNDARY_DASH_PX`).
const fn dash_of(border: Border) -> Option<[i32; 2]> {
    match border {
        Border::Dashed => Some(DASHED_PX),
        Border::Dotted => Some(DOTTED_PX),
        _ => None,
    }
}

const DASHED_PX: [i32; 2] = [8, 6];
const DOTTED_PX: [i32; 2] = [2, 4];

/// The box of `b` with its centre at `centre` (grid units).
fn scene_box(b: &PlanBox, centre: Point, words: &Words<'_>) -> SceneBox {
    let rect = px_rect(b.rect_at(centre));
    let texts = box_texts(b, &rect, words);
    SceneBox {
        id: b.id,
        element: b.element,
        kind: b.kind,
        shape: b.style.shape,
        rect,
        outline: outline(b.style.shape, rect),
        fill: b.style.background,
        stroke: Stroke {
            colour: b
                .style
                .stroke
                .unwrap_or_else(|| darker(b.style.background, 72)),
            width_px: i32::from(b.style.stroke_width),
            dash_px: dash_of(b.style.border),
        },
        name: texts.name,
        type_line: texts.type_line,
        description: texts.description,
        opacity: b.style.opacity.min(100),
        icon: b.style.icon.clone().map(|asset| SceneIcon {
            rect: icon_rect(b.style.shape, rect),
            asset,
        }),
    }
}

// ---------------------------------------------------------------- lines

const LINE_PX: i32 = 2;
const ARROW_LEN_PX: i32 = 10;
const ARROW_HALF_PX: i32 = 5;
const CHEVRON_HALF_DEPTH_PX: i32 = 3;
const CHEVRON_HALF_WIDTH_PX: i32 = 5;
const DOT_RADIUS_PX: i32 = 4;
/// CORE-10: no chevron within 8px of a label.
const CHEVRON_LABEL_GAP_PX: i32 = 8;
/// CORE-10: a chevron on a segment of 100px or more, three from 320px.
const CHEVRON_ONE_PX: i64 = 100;
const CHEVRON_THREE_PX: i64 = 320;

/// `a / b` rounded to nearest, ties towards +infinity; `b > 0`.
const fn div_round(a: i64, b: i64) -> i64 {
    (2 * a + b).div_euclid(2 * b)
}

/// `n` px along the direction of `(dx, dy)`, as a whole-px offset
/// rounded to nearest per axis; `(0, 0)` for a zero vector.
// DECISION: integer only (fixed point and `isqrt`), no floats, so the
// offsets are the same on every machine (CORE-25); axis directions are
// exact, a diagonal 10px step is (7, 7).
fn along(dx: i32, dy: i32, n: i32) -> (i32, i32) {
    let len2 = i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy);
    if len2 == 0 {
        return (0, 0);
    }
    let axis = |d: i32| -> i32 {
        let num = i128::from(n) * i128::from(n) * i128::from(d) * i128::from(d) * 1_000_000;
        let milli = (num / len2).isqrt();
        let v = i32::try_from((milli + 500) / 1000).unwrap_or(i32::MAX);
        if (d < 0) == (n < 0) { v } else { -v }
    };
    (axis(dx), axis(dy))
}

/// The octilinear direction nearest to `(dx, dy)` (exact for an
/// octilinear vector).
fn nearest_dir(dx: i32, dy: i32) -> Dir {
    let (ax, ay) = (i64::from(dx).abs(), i64::from(dy).abs());
    // tan(22.5 deg) ~ 0.4142: within it of an axis is that axis.
    let (sx, sy) = (dx.signum(), dy.signum());
    let (sx, sy) = if ay * 10_000 <= ax * 4142 {
        (sx, 0)
    } else if ax * 10_000 <= ay * 4142 {
        (0, sy)
    } else {
        (sx, sy)
    };
    Dir::ALL
        .into_iter()
        .find(|d| d.delta() == (sx, sy))
        .unwrap_or(Dir::E)
}

/// The arrowhead at `tip` for a segment arriving from `from`, at the
/// phase-1 size.
fn arrow_tip(from: Point, tip: Point) -> Tip {
    arrow_tip_scaled(from, tip, LINE_PX)
}

/// The arrowhead of a line `thickness` px wide: the phase-1 size times
/// `thickness / 2`, never smaller (P2-418).
// TODO(srs): P2-418. Reading taken (B styles-themes 9): the arrowhead and
// join dot scale by thickness/2, never below the phase-1 size; the 20px
// lane pitch does not change.
fn arrow_tip_scaled(from: Point, tip: Point, thickness: i32) -> Tip {
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let scale = |n: i32| n * thickness.max(LINE_PX) / LINE_PX;
    let (bx, by) = along(dx, dy, scale(ARROW_LEN_PX));
    let back = shift(tip, -bx, -by);
    let (px, py) = along(-dy, dx, scale(ARROW_HALF_PX));
    Tip::Arrow([tip, shift(back, px, py), shift(back, -px, -py)])
}

/// The chevrons of the segment `a`..`b` (px): at its middle when it is
/// at least 100px long, at 1/4, 1/2, 3/4 from 320px, skipping any within
/// 8px of a label rectangle.
fn chevrons(a: Point, b: Point, labels: &[Rect]) -> Vec<Chevron> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy);
    let quarters: &[i64] = if len2 >= CHEVRON_THREE_PX * CHEVRON_THREE_PX {
        &[1, 2, 3]
    } else if len2 >= CHEVRON_ONE_PX * CHEVRON_ONE_PX {
        &[2]
    } else {
        &[]
    };
    let dir = nearest_dir(dx, dy);
    let (fx, fy) = along(dx, dy, CHEVRON_HALF_DEPTH_PX);
    let (wx, wy) = along(-dy, dx, CHEVRON_HALF_WIDTH_PX);
    quarters
        .iter()
        .map(|&k| {
            let lerp = |p: i32, q: i32| {
                let v = div_round(i64::from(p) * (4 - k) + i64::from(q) * k, 4);
                i32::try_from(v).unwrap_or(i32::MAX)
            };
            Point::new(lerp(a.x, b.x), lerp(a.y, b.y))
        })
        .filter(|&at| {
            !labels
                .iter()
                .any(|r| r.inflate(CHEVRON_LABEL_GAP_PX).contains(at))
        })
        .map(|at| {
            let back = shift(at, -fx, -fy);
            Chevron {
                at,
                dir,
                points: [
                    shift(back, wx, wy),
                    shift(at, fx, fy),
                    shift(back, -wx, -wy),
                ],
            }
        })
        .collect()
}

/// The point where the ray from `rect`'s centre towards `toward` leaves
/// `rect` (px); the centre itself when `toward` is the centre.
fn edge_toward(rect: &Rect, toward: Point) -> Point {
    let c = rect.centre();
    let (dx, dy) = (i64::from(toward.x - c.x), i64::from(toward.y - c.y));
    let (hw, hh) = (i64::from(rect.width() / 2), i64::from(rect.height() / 2));
    if dx == 0 && dy == 0 {
        return c;
    }
    let (ox, oy) = if dx.abs() * hh >= dy.abs() * hw {
        (dx.signum() * hw, div_round(dy * hw, dx.abs()))
    } else {
        (div_round(dx * hh, dy.abs()), dy.signum() * hh)
    };
    let to_i32 = |v: i64| i32::try_from(v).unwrap_or(i32::MAX);
    Point::new(c.x + to_i32(ox), c.y + to_i32(oy))
}

/// The drawn line of `edge`: its routed polyline in px, or the CORE-13
/// straight fallback between its boxes' edges.
fn scene_line(
    edge: &PlanEdge,
    routes: &Routes,
    boxes: &[SceneBox],
    labels: &[Rect],
    pal: Palette,
) -> SceneLine {
    let source = &boxes[edge.source.0 as usize];
    let destination = &boxes[edge.destination.0 as usize];
    let routed_line = routes.line(edge.id);
    let points: Vec<Point> = match routed_line {
        Some(line) => line.points.iter().map(|&p| px_point(p)).collect(),
        None => vec![
            edge_toward(&source.rect, destination.rect.centre()),
            edge_toward(&destination.rect, source.rect.centre()),
        ],
    };
    let joins = routed_line.and_then(Line::joins);
    let last = points[points.len() - 1];
    let style = &edge.style;
    let thickness = i32::try_from(style.thickness).unwrap_or(i32::MAX).clamp(1, 100);
    let tip = if joins.is_some() {
        Tip::Dot {
            at: last,
            radius_px: DOT_RADIUS_PX * thickness.max(LINE_PX) / LINE_PX,
        }
    } else {
        arrow_tip_scaled(points[points.len() - 2], last, thickness)
    };
    let chevrons = points
        .windows(2)
        .flat_map(|w| chevrons(w[0], w[1], labels))
        .collect();
    SceneLine {
        edge: edge.id,
        relationship: edge.winner(),
        relationships: edge.relationships.clone(),
        source: edge.source,
        destination: edge.destination,
        points,
        // P2-419: a style colour replaces the destination's fill (CORE-3).
        // DECISION: the dash sits on the line's stroke; painters draw the
        // chevrons and the tip solid with the same colour and width, so a
        // dashed line still shows whole direction marks (P2-420).
        stroke: Stroke {
            colour: style.color.unwrap_or(pal.line),
            width_px: thickness,
            dash_px: dash_of(style.style),
        },
        chevrons,
        tip,
        joins,
        routed: routed_line.is_some(),
        opacity: style.opacity.min(100),
        jumps: Vec::new(),
        runs: Vec::new(),
        bars: Vec::new(),
        thickness_written: edge.thickness_written,
    }
}

/// The bounds of a line's points and tip (px).
fn line_bounds(line: &SceneLine) -> Rect {
    let mut e = Extent::default();
    for &p in line.points.iter().chain(line.runs.iter().flatten()).chain(line.bars.iter().flatten()) {
        e.add_point(p);
    }
    match line.tip {
        Tip::Arrow(pts) => pts.into_iter().for_each(|p| e.add_point(p)),
        Tip::Dot { at, radius_px } => e.add_rect(&Rect::from_centre(at, radius_px, radius_px)),
    }
    e.rect()
}

/// Fills `SceneLine::jumps` of every `jump true` line (P2-422): it hops
/// over earlier lines and over later lines that do not jump themselves.
fn add_jumps(plan: &ViewPlan, lines: &mut [SceneLine]) {
    for (i, edge) in plan.edges.iter().enumerate() {
        if edge.style.jump {
            let crossed: Vec<&SceneLine> = lines
                .iter()
                .zip(&plan.edges)
                .enumerate()
                .filter(|&(j, (_, e))| j < i || (j > i && !e.style.jump))
                .map(|(_, (l, _))| l)
                .collect();
            let hops = jumps(&lines[i], &crossed);
            lines[i].jumps = hops;
        }
    }
}

/// Where `line` properly crosses one of the routed lines `earlier`, in
/// its own order from start to end (P2-422). Touches and shared stretches
/// are not hops.
// TODO(srs): P2-422. Readings taken (B styles-themes 8, then checked
// against the reference renderer, where Java wins: Structurizr draws
// `jump true` with JointJS's `jumpover` connector, which hops over every
// link before it and over every later link that is not itself a
// `jumpover`): a `jump true` line hops at every crossing with a line
// drawn before it and with a later line that does not jump, so when both
// jump only the later one hops; the hop is a 6px half circle (export).
fn jumps(line: &SceneLine, earlier: &[&SceneLine]) -> Vec<Point> {
    if !line.routed {
        return Vec::new();
    }
    let mut out = Vec::new();
    for w in line.points.windows(2) {
        let mine = Segment::new(w[0], w[1]);
        let mut here: Vec<Point> = earlier
            .iter()
            .filter(|e| e.routed)
            .flat_map(|e| e.points.windows(2).map(|v| Segment::new(v[0], v[1])))
            .filter(|other| mine.contact(other) == Contact::Cross)
            .filter_map(|other| intersection(&mine, &other))
            .collect();
        // Along the segment from its start, each point once.
        here.sort_by_key(|p| {
            (i64::from(p.x - w[0].x).abs(), i64::from(p.y - w[0].y).abs())
        });
        here.dedup();
        for p in here {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// The crossing point of two segments that cross (`Contact::Cross`),
/// rounded to the nearest px.
fn intersection(first: &Segment, second: &Segment) -> Option<Point> {
    let (x1, y1, x2, y2) = (i128::from(first.a.x), i128::from(first.a.y), i128::from(first.b.x), i128::from(first.b.y));
    let (x3, y3, x4, y4) = (i128::from(second.a.x), i128::from(second.a.y), i128::from(second.b.x), i128::from(second.b.y));
    let den = (x1 - x2) * (y3 - y4) - (y1 - y2) * (x3 - x4);
    if den == 0 {
        return None;
    }
    let cross_first = x1 * y2 - y1 * x2;
    let cross_second = x3 * y4 - y3 * x4;
    let num_x = cross_first * (x3 - x4) - (x1 - x2) * cross_second;
    let num_y = cross_first * (y3 - y4) - (y1 - y2) * cross_second;
    let round = |n: i128| {
        let (n, d) = if den < 0 { (-n, -den) } else { (n, den) };
        i32::try_from((2 * n + d).div_euclid(2 * d)).ok()
    };
    Some(Point::new(round(num_x)?, round(num_y)?))
}

// --------------------------------------------------------------- labels

/// The rectangles chevrons keep 8px from (CORE-10): every label, and
/// every callout's dot.
// DECISION: a callout's dot also keeps chevrons 8px away, so the dot
// never sits on a direction mark (seen on the first callout render).
fn chevron_keepouts(labels: &[SceneLabel]) -> Vec<Rect> {
    labels
        .iter()
        .map(|l| l.rect)
        .chain(
            labels
                .iter()
                .filter_map(|l| l.callout.as_ref())
                .map(|c| Rect::from_centre(c.dot, c.dot_radius_px, c.dot_radius_px)),
        )
        .collect()
}

/// Grows `extent` by label `l`, its callout marks included (P4-316).
fn add_label(extent: &mut Extent, l: &SceneLabel) {
    extent.add_rect(&l.rect);
    if let Some(c) = &l.callout {
        extent.add_rect(&Rect::from_corners(c.leader[0], c.leader[1]));
        extent.add_rect(&Rect::from_centre(c.dot, c.dot_radius_px, c.dot_radius_px));
    }
}

/// The label of one edge in px (`label::Label` geometry) with its text.
fn scene_label(
    plan: &ViewPlan,
    label: &Label,
    boundaries: &[SceneBoundary],
    pal: Palette,
) -> SceneLabel {
    let edge = &plan.edges[label.edge.0 as usize];
    let (centre, rect) = if let Some(c) = &label.callout {
        (c.text.centre(), c.text)
    } else {
        let centre = px_point(label.centre);
        let min = Point::new(
            centre.x - label.width_px / 2,
            centre.y - label.height_px / 2,
        );
        let rect = Rect::from_corners(
            min,
            Point::new(min.x + label.width_px, min.y + label.height_px),
        );
        (centre, rect)
    };
    // Approver ruling 2026-11-17: a backing inside a boundary takes that
    // boundary's fill when it has one, else the page colour.
    // DECISION: the colour actually behind the label's centre: every
    // filled boundary holding it, outermost first (scene order), laid over
    // the page at its own opacity, as the renderer draws them. An unfilled
    // boundary adds nothing; opacity 100 gives the fill itself.
    let backing = boundaries
        .iter()
        .filter(|b| b.rect.contains(centre))
        .fold(pal.page, |under, b| match b.fill {
            Some(fill) => over(fill, b.opacity.min(100), under),
            None => under,
        });
    // redesign 2026-11-19: an unstyled line's label is `dg-ink`; a
    // relationship style's colour still colours its label.
    let mut colour = edge.style.color.unwrap_or(pal.ink);
    // Approver ruling 2026-11-17: in dark mode the text keeps the line's
    // colour only at a contrast of 3:1 or more against its backing.
    if matches!(plan.mode, Mode::Dark) && contrast(colour, backing) < MIN_LABEL_CONTRAST {
        colour = pal.ink;
    }
    let font = Font {
        size_px: label.font_px,
        line_px: label.line_px,
        weight: Weight::Regular,
    };
    // P4-307: the line's own colour and half its width.
    let callout = label.callout.as_ref().map(|c| {
        let thickness = i32::try_from(edge.style.thickness).unwrap_or(i32::MAX).clamp(1, 100);
        SceneCallout {
            shelf: c.shelf,
            leader: c.leader,
            dot: c.leader[1],
            dot_radius_px: CALLOUT_DOT_RADIUS_PX,
            stroke: Stroke {
                colour: edge.style.color.unwrap_or(pal.line),
                width_px: (thickness / 2).max(1),
                dash_px: None,
            },
            crossings: c.crossings.clone(),
        }
    });
    let mut rect = rect;
    let mut text = text_block(
        label.lines.clone(),
        font,
        colour,
        middle(centre.x, rect.min.y + LABEL_PAD_Y_PX),
        &rect,
    );
    // Design-system Edge "Step": a dynamic view's label draws its number
    // in a disc before the text. The lines keep `<number>: ` (P3-211);
    // the bounds become the disc, gap and text column the renderers draw,
    // centred where the text was.
    // `label::label_size` already measured the label this way, so the
    // rectangle holds it.
    // DECISION: should the drawn width still exceed the text area (never
    // expected), the rectangle grows by the difference, split evenly, so
    // the text fits it (no L25) and the hit test covers the disc.
    if plan.kind == ViewKind::Dynamic
        && let Some(width) = step_label_width(&text.lines, font.size_px)
    {
        let left = text.at.x - width / 2;
        text.bounds = Rect::from_corners(
            Point::new(left, text.bounds.min.y),
            Point::new(left + width, text.bounds.max.y),
        );
        let grow = width + 2 * LABEL_PAD_X_PX - rect.width();
        if grow > 0 {
            rect = Rect::from_corners(
                Point::new(rect.min.x - grow / 2, rect.min.y),
                Point::new(rect.max.x + grow - grow / 2, rect.max.y),
            );
        }
        text.fits = rect.contains(text.bounds.min) && rect.contains(text.bounds.max);
    }
    SceneLabel {
        edge: label.edge,
        relationship: edge.winner(),
        rect,
        // P4-302: a callout has no backing.
        fill: if callout.is_some() { None } else { Some(backing) },
        text,
        fit: label.fit.clone(),
        opacity: edge.style.opacity.min(100),
        callout,
    }
}

/// `top` at `opacity`/100 over `under`, each channel rounded to nearest.
fn over(top: Colour, opacity: u8, under: Colour) -> Colour {
    let a = u16::from(opacity.min(100));
    let mix = |t: u8, u: u8| {
        u8::try_from((u16::from(t) * a + u16::from(u) * (100 - a) + 50) / 100)
            .unwrap_or(u8::MAX)
    };
    Colour {
        r: mix(top.r, under.r),
        g: mix(top.g, under.g),
        b: mix(top.b, under.b),
    }
}

/// The least contrast ratio a dark-mode label text keeps its line colour at.
const MIN_LABEL_CONTRAST: f64 = 3.0;

/// The WCAG 2 contrast ratio of two colours, 1 to 21.
fn contrast(a: Colour, b: Colour) -> f64 {
    fn luminance(c: Colour) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    }
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

// ------------------------------------------------------------ boundary

// DECISION (redesign 2026-11-19): the design system's `.dg-boundary`
// dash 6 4 (was 8/6); owner decision 2026-11-19: design values over
// P2-411, which now states 6/4.
const BOUNDARY_DASH_PX: [i32; 2] = [6, 4];
// The tag text as `label::boundary_tag` sizes it (label.rs DECISION):
// bold, from `TAG_INSET_PX` right of the boundary's left edge, baseline
// `TAG_BASELINE_PX` above its bottom edge.
// redesign 2026-11-19: 11px on a 14px pitch, 10px in, baseline 11px up
// (design-system `.dgh-boundary-t` 11/14 semibold `dg-ink-2`, its box
// 10px in and 8px above the bottom edge: baseline at 8 + 14 - 11), was
// 12px on 15, 12px in, 12px up. CORE-1 says bold; the bundled Inter has
// no 600, so the tag stays Bold.
const TAG_FONT: Font = Font {
    size_px: 11,
    line_px: 14,
    weight: Weight::Bold,
};
const TAG_INSET_PX: i32 = 10;
const TAG_BASELINE_PX: i32 = 11;

/// Boundary `index` of `plan` in px, `None` when it has no member; its
/// look from `view::Boundary::style` (P2-411), phase 1 where unset.
fn scene_boundary(
    plan: &ViewPlan,
    index: usize,
    centres: &[Point],
    pal: Palette,
    words: &Words<'_>,
) -> Option<SceneBoundary> {
    let boundary = &plan.boundaries[index];
    let style = &boundary.style;
    let rect = px_rect(plan.boundary_rect(index, centres)?);
    let tag_rect = px_rect(boundary_tag_with(plan, index, centres, words)?);
    let baseline = rect.max.y - TAG_BASELINE_PX;
    let size_px = boundary_tag_font_px(boundary);
    let font = Font {
        size_px,
        line_px: size_px + TAG_FONT.line_px - TAG_FONT.size_px,
        weight: Weight::Bold,
    };
    let node = matches!(boundary.of, BoundaryOf::DeploymentNode(_));
    let colour = style.color.unwrap_or(pal.ink_2);
    let tag = if node {
        // P3-114: name over type line, at the top left.
        text_block(
            crate::label::boundary_tag_lines(plan, index, words),
            Font {
                line_px: size_px + 3,
                ..font
            },
            colour,
            start(rect.min.x + TAG_INSET_PX, rect.min.y + crate::label::NODE_TAG_TOP_PX),
            &tag_rect,
        )
    } else {
        text_block(
            vec![boundary_tag_text(plan, index, words)],
            font,
            colour,
            start(rect.min.x + TAG_INSET_PX, baseline - font.baseline_px()),
            &tag_rect,
        )
    };
    // P3-115: `x<n>` at the bottom right, in the tag's colour.
    let instances = boundary.instances.as_ref().map(|text| {
        let count_font = Font {
            size_px,
            line_px: size_px + 3,
            weight: Weight::Bold,
        };
        let width = text_px(text, count_font.size_px, count_font.weight);
        text_block(
            vec![text.clone()],
            count_font,
            colour,
            start(
                rect.max.x - TAG_INSET_PX - width,
                rect.max.y - crate::label::NODE_TAG_TOP_PX - count_font.line_px,
            ),
            &rect,
        )
    });
    Some(SceneBoundary {
        of: boundary.of,
        members: boundary.members.clone(),
        rect,
        stroke: Stroke {
            colour: style.stroke.unwrap_or(pal.boundary),
            width_px: style.stroke_width.map_or(OUTLINE_PX, i32::from),
            // P3-114: a deployment node is solid unless styled.
            dash_px: style
                .border
                .map_or(if node { None } else { Some(BOUNDARY_DASH_PX) }, dash_of),
        },
        tag_rect,
        tag,
        fill: style.background,
        opacity: style.opacity.min(100),
        icon: None,
        instances,
    })
}

// ------------------------------------------------- title, subtitle, key

// DECISION (redesign 2026-11-19): the title is `.dgh-title` 15px on a
// 20px line; the design says semibold (600), but the bundled Inter has
// no 600 weight (`Weight` is Regular or Bold), so it is Bold. Owner
// decision 2026-11-19: design values over CORE-1 (was 18px on 22px).
const TITLE_FONT: Font = Font {
    size_px: 15,
    line_px: 20,
    weight: Weight::Bold,
};
// DECISION (redesign 2026-11-19): the subtitle is `.dg-sub` 11px Regular
// (was 12px on 16px); the design gives it no line height, so it takes
// the 14px pitch of the other 11px texts and sits right under the title
// line, as the Legend README orders them (title, subtitle, key).
const SUBTITLE_FONT: Font = Font {
    size_px: SMALL_PX,
    line_px: SMALL_LINE_PX,
    weight: Weight::Regular,
};
const KEY_FONT: Font = Font {
    size_px: SMALL_PX,
    line_px: SMALL_LINE_PX,
    weight: Weight::Regular,
};
// redesign 2026-11-19: spacing of screen-Main (Main.html `.dgh-title`
// and `.dgh-key`): the title's line 16px below the drawing (was 30), the
// key's text line 14px below the title's (or subtitle's) line, which is
// `KEY_GAP_PX` plus the row's half-leading (26 - 14) / 2 (was 16 + 6);
// 6px from swatch to text (`.dgh-key span` gap, was 8) and 16px between
// entries (`space-16`, was 24). Main.html: drawing bottom 604, title
// line 620..640, key line from 654; with the 20px title line those gaps
// hold unchanged (16 below the drawing, 14 below the title line). Key
// text 11px, the integer px nearest the design's 10.5px
// (`TextBlock::size_px` is whole px).
/// Space between the drawing and the title (CORE-14: clear of it).
const TITLE_GAP_PX: i32 = 16;
const KEY_GAP_PX: i32 = 8;
const KEY_ROW_PX: i32 = 26;
// redesign 2026-11-19: 18x12 swatches (design-system Legend).
const KEY_BOX_W_PX: i32 = 18;
const KEY_BOX_H_PX: i32 = 12;
const KEY_LINE_W_PX: i32 = 30;
const KEY_TEXT_GAP_PX: i32 = 6;
const KEY_ENTRY_GAP_PX: i32 = 16;
// DECISION: key rows wrap at the drawing's width, or 480px for a narrow
// drawing, so the key never widens the scene much past the drawing.
const KEY_MIN_WIDTH_PX: i32 = 480;

/// A one-line chrome text (title, subtitle) at the left `x`, band top
/// `top`. It has no rectangle to fit, so it always fits.
// DECISION: the title and subtitle are not wrapped: they are one line
// each below the drawing, and the scene grows to hold them.
fn chrome_text(text: &str, font: Font, x: i32, top: i32, colour: Colour) -> TextBlock {
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let lines = if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    };
    let mut block = text_block(lines, font, colour, start(x, top), &Rect::default());
    block.fits = true;
    block
}

/// What a key entry stands for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Box {
        shape: Shape,
        fill: Colour,
        text: Colour,
        stroke: Stroke,
    },
    Line {
        stroke: Stroke,
        dot: bool,
    },
}

/// The swatch outline of `shape` in the key, top left at `at`: the
/// box's own outline at key size (P2-416).
fn swatch_outline(shape: Shape, at: Point) -> Outline {
    let rect = Rect::from_corners(at, shift(at, KEY_BOX_W_PX, KEY_BOX_H_PX));
    match shape {
        // P2-416: the person outline at key size. The redesign's Legend
        // draws a pill for a person; P2-416 keeps the shape outline
        // (docs/deferred.md D-R6).
        Shape::Person => Outline::Person {
            head_centre: shift(at, KEY_BOX_W_PX / 2, 3),
            head_radius_px: 3,
            body: Rect::from_corners(shift(at, 2, 5), shift(at, KEY_BOX_W_PX - 2, KEY_BOX_H_PX)),
            body_radius_px: 3,
        },
        _ => shape_outline(shape, rect, SWATCH_DIMS),
    }
}

/// One line key entry being collected: (style tag, dot), the first
/// line's stroke, whether every line so far shares its colour.
type LineGroup<'a> = ((Option<&'a str>, bool), Stroke, bool);

/// Every distinct look drawn, boxes then lines, each in first-drawn
/// order, with the names it stands for (CORE-1). A box look is shape,
/// fill, text colour and stroke (colour, width, dash: P2-435; opacity and
/// icon do not split). Lines get one entry per relationship-style tag
/// (`PlanEdge::style_tag`) and end (arrow or join dot), named by the tag
/// or `Relationship` (P2-27); the swatch takes the lines' colour when
/// they share one, else #707070, and the first line's width and dash
/// (P2-434).
// TODO(srs): P2-27, P2-435. Readings taken (B styles-themes 12): join-dot
// lines keep their own entry; opacity and icons do not split entries.
fn looks(
    plan: &ViewPlan,
    boxes: &[SceneBox],
    lines: &[SceneLine],
    words: &Words<'_>,
) -> Vec<(Look, Vec<String>)> {
    let mut looks: Vec<(Look, Vec<String>)> = Vec::new();
    for b in boxes {
        let pb = &plan.boxes[b.id.0 as usize];
        let name = pb.style_tag.clone();
        let look = Look::Box {
            shape: b.shape,
            fill: b.fill,
            text: b.name.colour,
            stroke: b.stroke,
        };
        if let Some((_, names)) = looks.iter_mut().find(|(l, _)| *l == look) {
            if !names.contains(&name) {
                names.push(name);
            }
        } else {
            looks.push((look, vec![name]));
        }
    }
    // (tag, dot) -> stroke of the first line, whether all colours agree.
    let mut groups: Vec<LineGroup<'_>> = Vec::new();
    for l in lines {
        let key = (
            plan.edges[l.edge.0 as usize].style_tag.as_deref(),
            matches!(l.tip, Tip::Dot { .. }),
        );
        if let Some((_, stroke, same)) = groups.iter_mut().find(|(k, _, _)| *k == key) {
            *same &= stroke.colour == l.stroke.colour;
        } else {
            groups.push((key, l.stroke, true));
        }
    }
    for ((tag, dot), mut stroke, same) in groups {
        if !same {
            stroke.colour = palette(plan.mode).line;
        }
        looks.push((
            Look::Line { stroke, dot },
            vec![tag.unwrap_or(words.relationship).to_owned()],
        ));
    }
    looks
}

/// The key swatch of `look`, its left edge at `x`, centred on `mid`.
fn swatch(look: Look, x: i32, mid: i32) -> Swatch {
    match look {
        Look::Box {
            shape,
            fill,
            stroke,
            ..
        } => Swatch::Box {
            outline: swatch_outline(shape, Point::new(x, mid - KEY_BOX_H_PX / 2)),
            fill,
            stroke,
        },
        Look::Line { stroke, dot } => {
            let (from, to) = (Point::new(x, mid), Point::new(x + KEY_LINE_W_PX, mid));
            Swatch::Line {
                from,
                to,
                stroke,
                tip: if dot {
                    Tip::Dot {
                        at: to,
                        radius_px: DOT_RADIUS_PX,
                    }
                } else {
                    arrow_tip(from, to)
                },
            }
        }
    }
}

/// The key of `looks` (the drawn boxes and lines), laid out in rows from
/// the drawing's left edge, its top at `top` (CORE-1).
fn legend(looks: Vec<(Look, Vec<String>)>, drawing: &Rect, top: i32) -> Legend {
    let left = drawing.min.x;
    let limit = left + drawing.width().max(KEY_MIN_WIDTH_PX);
    let mut extent = Extent::default();
    extent.add_point(Point::new(left, top));
    let (mut x, mut row_top) = (left, top);
    let mut entries = Vec::with_capacity(looks.len());
    for (look, names) in looks {
        let label = names.join(", ");
        let swatch_w = match look {
            Look::Box { .. } => KEY_BOX_W_PX,
            Look::Line { .. } => KEY_LINE_W_PX,
        };
        let width = swatch_w + KEY_TEXT_GAP_PX + text_px(&label, KEY_FONT.size_px, KEY_FONT.weight);
        if x > left && x + width > limit {
            x = left;
            row_top += KEY_ROW_PX;
        }
        let mid = row_top + KEY_ROW_PX / 2;
        let swatch = swatch(look, x, mid);
        let text = text_block(
            vec![label],
            KEY_FONT,
            // The caller sets the mode's key colour.
            palette(Mode::Light).ink_2,
            start(x + swatch_w + KEY_TEXT_GAP_PX, mid - KEY_FONT.line_px / 2),
            &Rect::default(),
        );
        extent.add_rect(&Rect::from_corners(
            Point::new(x, row_top),
            Point::new(x + width, row_top + KEY_ROW_PX),
        ));
        extent.add_rect(&text.bounds);
        entries.push((swatch, names, text));
        x += width + KEY_ENTRY_GAP_PX;
    }
    let rect = extent.rect();
    Legend {
        rect,
        entries: entries
            .into_iter()
            .map(|(swatch, names, mut text)| {
                text.fits = rect.contains(text.bounds.min) && rect.contains(text.bounds.max);
                LegendEntry {
                    swatch,
                    names,
                    text,
                }
            })
            .collect(),
    }
}

// ------------------------------------------------------------- hit test

/// True when `at` lies inside the shape of `outline` (borders included).
// DECISION: rounded corners count as their full rectangle: at most a
// few px of difference, and the selection mark is the rectangle anyway.
fn outline_contains(outline: &Outline, at: Point) -> bool {
    match *outline {
        Outline::Rect { rect, .. }
        | Outline::Bucket { rect, .. }
        | Outline::Pipe { rect, .. }
        | Outline::Window { rect, .. }
        | Outline::MobileDevice { rect, .. } => rect.contains(at),
        Outline::Person {
            head_centre,
            head_radius_px,
            body,
            ..
        } => {
            let (dx, dy) = (
                i64::from(at.x - head_centre.x),
                i64::from(at.y - head_centre.y),
            );
            let r = i64::from(head_radius_px);
            body.contains(at) || dx * dx + dy * dy <= r * r
        }
        Outline::Cylinder { rect, cap_ry_px } => {
            if !rect.contains(at) {
                return false;
            }
            let ry = i64::from(cap_ry_px);
            let rx = i64::from(rect.width()) / 2;
            let cx = i64::from(rect.min.x) + rx;
            let in_cap = |cy: i64| {
                let (dx, dy) = (i64::from(at.x) - cx, i64::from(at.y) - cy);
                dx * dx * ry * ry + dy * dy * rx * rx <= rx * rx * ry * ry
            };
            let (top, bottom) = (i64::from(rect.min.y) + ry, i64::from(rect.max.y) - ry);
            let y = i64::from(at.y);
            (top..=bottom).contains(&y) || in_cap(top) || in_cap(bottom)
        }
        Outline::Ellipse { rect, .. } => {
            let c = rect.centre();
            let (rx, ry) = (i64::from(rect.width()) / 2, i64::from(rect.height()) / 2);
            let (dx, dy) = (i64::from(at.x - c.x), i64::from(at.y - c.y));
            dx * dx * ry * ry + dy * dy * rx * rx <= rx * rx * ry * ry
        }
        Outline::Hexagon { rect, inset_px } => {
            if !rect.contains(at) {
                return false;
            }
            // The slanted sides: the inset shrinks to 0 at mid height.
            let hh = i64::from(rect.height()) / 2;
            let dy = i64::from(at.y - rect.centre().y).abs();
            let inset = if hh == 0 { 0 } else { i64::from(inset_px) * dy / hh };
            let x = i64::from(at.x);
            x >= i64::from(rect.min.x) + inset && x <= i64::from(rect.max.x) - inset
        }
        Outline::Diamond { rect } => {
            let c = rect.centre();
            let (rx, ry) = (i64::from(rect.width()) / 2, i64::from(rect.height()) / 2);
            let (dx, dy) = (i64::from(at.x - c.x).abs(), i64::from(at.y - c.y).abs());
            dx * ry + dy * rx <= rx * ry
        }
        Outline::Robot { head, body, .. } => head.contains(at) || body.contains(at),
        Outline::Folder { rect, tab } => rect.contains(at) || tab.contains(at),
        Outline::Component { rect, tabs } => {
            rect.contains(at) || tabs.iter().any(|t| t.contains(at))
        }
    }
}

/// True when `at` is within `tol` px of the segment `a`..`b`.
fn near_segment(a: Point, b: Point, at: Point, tol: i32) -> bool {
    let (ux, uy) = (i64::from(b.x - a.x), i64::from(b.y - a.y));
    let (vx, vy) = (i64::from(at.x - a.x), i64::from(at.y - a.y));
    let tol2 = i64::from(tol) * i64::from(tol);
    let len2 = ux * ux + uy * uy;
    let dot = ux * vx + uy * vy;
    if len2 == 0 || dot <= 0 {
        return vx * vx + vy * vy <= tol2;
    }
    if dot >= len2 {
        let (wx, wy) = (i64::from(at.x - b.x), i64::from(at.y - b.y));
        return wx * wx + wy * wy <= tol2;
    }
    let cross = ux * vy - uy * vx;
    i128::from(cross) * i128::from(cross) <= i128::from(tol2) * i128::from(len2)
}

/// True when `at` is within `tol` px of the line's polyline, or on its
/// tip.
fn line_near(line: &SceneLine, at: Point, tol: i32) -> bool {
    let on_tip = match line.tip {
        Tip::Arrow(_) => false,
        Tip::Dot { at: c, radius_px } => near_segment(c, c, at, radius_px.max(tol)),
    };
    on_tip
        || line
            .points
            .windows(2)
            .any(|w| near_segment(w[0], w[1], at, tol))
        || line.points.len() == 1 && near_segment(line.points[0], line.points[0], at, tol)
}

/// True when `at` is within `tol` px of the outline of `rect`.
fn outline_near(rect: &Rect, at: Point, tol: i32) -> bool {
    rect.inflate(tol).contains(at) && !rect.inflate(-tol - 1).contains(at)
}

/// The union of the rectangles added, in px.
#[derive(Default)]
struct Extent(Option<Rect>);

impl Extent {
    fn add_rect(&mut self, r: &Rect) {
        self.0 = Some(match self.0 {
            None => *r,
            Some(e) => Rect::from_corners(
                Point::new(e.min.x.min(r.min.x), e.min.y.min(r.min.y)),
                Point::new(e.max.x.max(r.max.x), e.max.y.max(r.max.y)),
            ),
        });
    }

    fn add_point(&mut self, p: Point) {
        self.add_rect(&Rect { min: p, max: p });
    }

    /// The union, or an empty rectangle at the origin when nothing was
    /// added.
    fn rect(&self) -> Rect {
        self.0.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicBool;

    use crate::geom::{GRID_PX, Segment};
    use crate::label::place_labels;
    use crate::model::parse;
    use crate::place::{Keep, place};
    use crate::route::{Join, Line, LineEnd, Port, RouteConfig, Side, route};
    use crate::view::plan;

    /// A small shop written with roles only: a person, a system with
    /// three containers (one a cylinder), an external system.
    const SHOP: &str = r#"workspace {
  model {
    operator = person "Operator" "Runs the shop day to day."
    shop = softwareSystem "Shop" "Sells goods online." {
      web = container "Web app" "Serves the shop pages and takes orders from the operator." "Rust"
      db = container "Database" "Stores orders." "SQLite" "Database"
      worker = container "Worker"
    }
    bank = softwareSystem "Bank" "Takes card payments." "External"
    operator -> web "Manages orders in" "HTTPS"
    web -> db "Reads and writes orders" "SQL"
    worker -> db "Reads open orders from" "SQL"
    web -> bank "Charges cards through" "HTTPS"
  }
  views {
    systemContext shop "c1" "Shop context" {
      include *
    }
    container shop "c2" "Shop containers" {
      subtitle "The running parts of the shop."
      include *
    }
    styles {
      element "Person" {
        background #08427b
        color #ffffff
      }
      element "Container" {
        background #438dd5
        color #ffffff
      }
      element "Database" {
        shape Cylinder
      }
      element "External" {
        background #999999
      }
    }
  }
}
"#;

    /// Plan, routes and labels of view `key` of `src`: auto-placed,
    /// routed with the default config, labelled.
    fn staged(src: &str, key: &str) -> (ViewPlan, Routes, Labels) {
        let model = match parse(src) {
            Ok(p) => p.model,
            Err(e) => panic!("{e}"),
        };
        let plan = plan(&model, model.view(key).expect("view"));
        let centres = place(&plan, &Keep::default());
        let routes = route(
            &plan,
            &centres,
            &RouteConfig::DEFAULT,
            &AtomicBool::new(false),
        )
        .expect("not stopped");
        let labels = place_labels(&plan, &routes);
        (plan, routes, labels)
    }

    /// The scene box drawing the element named `name`.
    fn scene_box<'a>(scene: &'a Scene, plan: &ViewPlan, name: &str) -> &'a SceneBox {
        let b = plan
            .boxes
            .iter()
            .find(|b| b.name == name)
            .unwrap_or_else(|| panic!("no box {name}"));
        &scene.boxes[b.id.0 as usize]
    }

    fn px_rect(r: Rect) -> Rect {
        Rect::from_corners(
            Point::new(r.min.x * GRID_PX, r.min.y * GRID_PX),
            Point::new(r.max.x * GRID_PX, r.max.y * GRID_PX),
        )
    }

    fn port(at: Point) -> Port {
        Port { at, side: Side::E }
    }

    /// An arrowed line of edge `e` through grid points `pts`.
    #[expect(clippy::unnecessary_wraps, reason = "a routed slot of `Routes::lines`")]
    fn arrow(e: u32, pts: &[(i32, i32)]) -> Option<Line> {
        let points: Vec<Point> = pts.iter().map(|&(x, y)| Point::new(x, y)).collect();
        Some(Line {
            edge: EdgeId(e),
            source: port(points[0]),
            end: LineEnd::Arrow(port(points[points.len() - 1])),
            points,
        })
    }

    /// A line of edge `e` through `pts` ending in a join dot on `trunk`.
    #[expect(clippy::unnecessary_wraps, reason = "a routed slot of `Routes::lines`")]
    fn joined(e: u32, pts: &[(i32, i32)], trunk: u32) -> Option<Line> {
        let points: Vec<Point> = pts.iter().map(|&(x, y)| Point::new(x, y)).collect();
        Some(Line {
            edge: EdgeId(e),
            source: port(points[0]),
            end: LineEnd::Join(Join {
                trunk: EdgeId(trunk),
                at: points[points.len() - 1],
            }),
            points,
        })
    }

    /// Routes of `plan` with the given lines, boxes in a row far below
    /// them (y = 100 grid units), no crossings.
    fn manual(plan: &ViewPlan, lines: Vec<Option<Line>>) -> Routes {
        let centres = (0..plan.boxes.len())
            .map(|i| Point::new(30 * i32::try_from(i).expect("small"), 100))
            .collect();
        Routes {
            lines,
            centres,
            crossings: 0,
            bundles: Vec::new(),
        }
    }

    fn no_labels(plan: &ViewPlan) -> Labels {
        Labels {
            labels: vec![None; plan.edges.len()],
            shared: Vec::new(),
            fallbacks: Vec::new(),
        }
    }

    /// Components of two containers, two of them used by one component
    /// of the other: lifted to one merged line on the container view.
    const LIFTED: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods." {
      front = container "Front" "Shows pages." "Rust" {
        pages = component "Pages" "Renders pages." "Rust"
        cart = component "Cart" "Holds the cart." "Rust"
      }
      store = container "Store" "Keeps orders." "SQLite" {
        orders = component "Orders" "Order table." "SQL"
      }
    }
    operator -> pages "Browses"
    pages -> orders "Reads orders from"
    cart -> orders "Writes carts to"
  }
  views {
    container shop "c2" "Lifted lines" {
      include *
    }
  }
}
"#;

    /// The prototype example converted to c4rizer (tests/data).
    const EXAMPLE: &str = include_str!("../../../tests/data/example.dsl");

    /// `p` lies on the border of `r`.
    fn on_border(r: &Rect, p: Point) -> bool {
        r.contains(p) && (p.x == r.min.x || p.x == r.max.x || p.y == r.min.y || p.y == r.max.y)
    }

    /// The bounds of every box, line, label and boundary.
    fn drawing_extent(scene: &Scene) -> Rect {
        let mut e = Extent::default();
        scene.boxes.iter().for_each(|b| e.add_rect(&b.rect));
        scene.lines.iter().for_each(|l| e.add_rect(&line_bounds(l)));
        scene.labels.iter().for_each(|l| e.add_rect(&l.rect));
        scene.boundaries.iter().for_each(|b| e.add_rect(&b.rect));
        e.rect()
    }

    impl Extent {
        fn of(rects: impl Iterator<Item = Rect>) -> Rect {
            let mut e = Extent::default();
            rects.for_each(|r| e.add_rect(&r));
            e.rect()
        }
    }

    /// `a` lies inside `b`, borders included.
    fn inside(a: &Rect, b: &Rect) -> bool {
        b.contains(a.min) && b.contains(a.max)
    }

    // One test per requirement.
    #[test]
    fn core_1_title_subtitle_key_boundary() {
        // Title and subtitle present; one boundary around the scope's
        // children plus 40px, tag at its bottom left.
        let (plan, routes, labels) = staged(SHOP, "c2");
        let scene = build(&plan, &routes, &labels);
        assert_eq!(scene.key, "c2");
        assert_eq!(scene.options, plan.options);
        let drawing = drawing_extent(&scene);

        let title = scene.title.as_ref().expect("title");
        assert_eq!(title.lines, vec!["Shop containers"]);
        // owner decision 2026-11-19: design values over P2-411 / CORE-1
        // (15px; the design's semibold is Bold, Inter ships no 600).
        assert_eq!((title.size_px, title.weight), (15, Weight::Bold));
        let subtitle = scene.subtitle.as_ref().expect("subtitle");
        assert_eq!(subtitle.lines, vec!["The running parts of the shop."]);
        assert_eq!((subtitle.size_px, subtitle.weight), (11, Weight::Regular));
        // Bottom left below the drawing, subtitle under the title, key
        // under both.
        assert_eq!(title.align, Align::Start);
        assert_eq!(title.at.x, drawing.min.x);
        assert_eq!(subtitle.at.x, drawing.min.x);
        assert!(title.bounds.min.y > drawing.max.y);
        assert!(subtitle.bounds.min.y >= title.bounds.max.y);
        assert!(scene.legend.rect.min.y >= subtitle.bounds.max.y);
        assert_eq!(scene.legend.rect.min.x, drawing.min.x);
        for r in [title.bounds, subtitle.bounds, scene.legend.rect] {
            assert!(inside(&r, &scene.bounds), "{r:?} in {:?}", scene.bounds);
        }

        assert_eq!(scene.boundaries.len(), 1);
        let boundary = &scene.boundaries[0];
        let shop = plan.boundaries[0].of;
        assert_eq!(boundary.of, shop);
        assert_eq!(boundary.members, plan.boundaries[0].members);
        let members = Extent::of(
            boundary
                .members
                .iter()
                .map(|m| scene.boxes[m.0 as usize].rect),
        );
        assert_eq!(boundary.rect, members.inflate(40));
        assert!(boundary.stroke.dash_px.is_some(), "dashed");
        assert_eq!(boundary.tag.lines, vec!["Shop [Software System]"]);
        assert_eq!(boundary.tag.weight, Weight::Bold);
        let tag = crate::label::boundary_tag(&plan, 0, &routes.centres).expect("tag");
        assert_eq!(boundary.tag_rect, px_rect(tag));
        assert!(inside(&boundary.tag.bounds, &boundary.tag_rect));
        assert!(boundary.tag.fits);
        // Bottom left of the boundary.
        assert_eq!(boundary.tag_rect.min.x, boundary.rect.min.x);
        assert_eq!(boundary.tag_rect.max.y, boundary.rect.max.y);
    }

    #[test]
    fn core_1_key_covers_every_family() {
        // Review Focus: one box and no lines still lists that box's
        // family; two tags with the same look give one entry with both
        // names; an unstyled element gets the default colours, in the key.
        let (plan, routes, labels) = staged(SHOP, "c2");
        let scene = build(&plan, &routes, &labels);
        let names: Vec<Vec<&str>> = scene
            .legend
            .entries
            .iter()
            .map(|e| e.names.iter().map(String::as_str).collect())
            .collect();
        // Boxes in box order (operator, web, db, worker, bank), then one
        // `Relationship` entry for every unstyled line (P2-27; phase 1
        // had one per line colour).
        assert_eq!(
            names,
            vec![
                vec!["Person"],
                vec!["Container"],
                vec!["Database"],
                vec!["External"],
                vec!["Relationship"],
            ]
        );
        let swatch_fill = |i: usize| match scene.legend.entries[i].swatch {
            Swatch::Box { fill, outline, .. } => (fill, outline),
            Swatch::Line { .. } => panic!("box swatch expected"),
        };
        assert!(matches!(swatch_fill(0).1, Outline::Person { .. }));
        assert!(matches!(swatch_fill(2).1, Outline::Cylinder { .. }));
        assert_eq!(
            swatch_fill(3).0,
            Colour {
                r: 0x99,
                g: 0x99,
                b: 0x99
            }
        );
        let line_colours: Vec<Colour> = scene.legend.entries[4..]
            .iter()
            .map(|e| match e.swatch {
                Swatch::Line { stroke, tip, .. } => {
                    assert!(matches!(tip, Tip::Arrow(_)));
                    stroke.colour
                }
                Swatch::Box { .. } => panic!("line swatch expected"),
            })
            .collect();
        // The lines take two colours, so the one entry is #707070 (P2-434).
        assert_eq!(line_colours, vec![palette(Mode::Light).line]);
        for e in &scene.legend.entries {
            assert_eq!(e.text.lines, vec![e.names.join(", ")]);
            assert!(inside(&e.text.bounds, &scene.legend.rect));
        }

        key_of_one_box_and_shared_look();
    }

    /// CORE-1 key, Review Focus: one box and no line; two tags of one
    /// look share an entry; an unstyled element is in the key with the
    /// default colours.
    fn key_of_one_box_and_shared_look() {
        let src = r#"workspace {
  model {
    solo = softwareSystem "Solo" "Stands alone."
    left = softwareSystem "Left" "Tagged alpha." "Alpha"
    right = softwareSystem "Right" "Tagged beta." "Beta"
    left -> right "Sends batches to"
  }
  views {
    systemContext solo "one" "Solo context" {
      include solo
    }
    systemContext left "pair" "Pair context" {
      include left right
    }
    styles {
      element "Alpha" {
        background #123456
      }
      element "Beta" {
        background #123456
      }
    }
  }
}
"#;
        let (plan, routes, labels) = staged(src, "one");
        let scene = build(&plan, &routes, &labels);
        assert!(scene.lines.is_empty());
        assert!(scene.title.is_some());
        assert_eq!(scene.legend.entries.len(), 1);
        assert_eq!(scene.legend.entries[0].names, vec!["Software System"]);
        let Swatch::Box { fill, .. } = scene.legend.entries[0].swatch else {
            panic!("box swatch");
        };
        // redesign 2026-11-19: an unstyled system's swatch is the house
        // `dg-system` (was the keyword default #dddddd).
        assert_eq!(fill, Colour { r: 0x1b, g: 0x5f, b: 0xd0 });

        let (plan, routes, labels) = staged(src, "pair");
        let scene = build(&plan, &routes, &labels);
        let names: Vec<Vec<String>> = scene
            .legend
            .entries
            .iter()
            .map(|e| e.names.clone())
            .collect();
        assert_eq!(
            names,
            vec![
                vec!["Alpha".to_owned(), "Beta".to_owned()],
                vec!["Relationship".to_owned()]
            ]
        );
        assert_eq!(scene.legend.entries[0].text.lines, vec!["Alpha, Beta"]);
    }

    #[test]
    fn core_1_no_boundary_without_children() {
        // Review Focus: a scope with no drawn child draws no boundary.
        let src = SHOP.replace(
            "    styles {",
            "    container shop \"bare\" \"No containers\" {\n      include operator bank\n    }\n    styles {",
        );
        let (plan, routes, labels) = staged(&src, "bare");
        assert_eq!(plan.boxes.len(), 2);
        let scene = build(&plan, &routes, &labels);
        assert!(scene.boundaries.is_empty());
        assert_eq!(scene.model_id(ItemKey::Boundary(0)), None);
    }

    #[test]
    fn core_2_person_and_cylinder_shapes() {
        let (plan, routes, labels) = staged(SHOP, "c2");
        let scene = build(&plan, &routes, &labels);
        let operator = scene_box(&scene, &plan, "Operator");
        assert_eq!(operator.shape, Shape::Person);
        let Outline::Person {
            head_centre,
            head_radius_px,
            body,
            ..
        } = operator.outline
        else {
            panic!("person outline: {:?}", operator.outline);
        };
        // The head sits on the top edge, centred; the body spans the
        // box's width and reaches its bottom.
        assert_eq!(head_centre.x, operator.rect.centre().x);
        assert_eq!(head_centre.y - head_radius_px, operator.rect.min.y);
        assert!(head_radius_px > 0);
        assert_eq!(
            (body.min.x, body.max.x),
            (operator.rect.min.x, operator.rect.max.x)
        );
        assert_eq!(body.max.y, operator.rect.max.y);
        assert!(body.min.y > operator.rect.min.y && body.min.y < head_centre.y + head_radius_px);
        // The texts sit in the body, under the head.
        assert!(operator.name.bounds.min.y >= body.min.y);

        let db = scene_box(&scene, &plan, "Database");
        assert_eq!(db.shape, Shape::Cylinder);
        let Outline::Cylinder { rect, cap_ry_px } = db.outline else {
            panic!("cylinder outline: {:?}", db.outline);
        };
        assert_eq!(rect, db.rect);
        assert!(cap_ry_px > 0 && cap_ry_px * 4 < rect.height());
        // The texts sit below the whole top cap.
        assert!(db.name.bounds.min.y >= rect.min.y + 2 * cap_ry_px);

        let web = scene_box(&scene, &plan, "Web app");
        assert_eq!(web.shape, Shape::Box);
        assert!(matches!(web.outline, Outline::Rect { rect, .. } if rect == web.rect));
        // Box rect in px: the plan rect at the routed centre, times 10.
        let b = &plan.boxes[web.id.0 as usize];
        assert_eq!(
            web.rect,
            px_rect(b.rect_at(routes.centres[web.id.0 as usize]))
        );
        assert_eq!(web.rect.width(), 240);
        assert_eq!(web.rect.height(), 140);
        assert_eq!(operator.rect.width(), 200);
        assert_eq!(operator.rect.height(), 180);
    }

    #[test]
    fn core_2_box_texts_name_type_description() {
        // `[Container: Rust]`, `[Container]` with no technology,
        // `[External Software System]` for an `External` system.
        let (plan, routes, labels) = staged(SHOP, "c2");
        let scene = build(&plan, &routes, &labels);
        let web = scene_box(&scene, &plan, "Web app");
        assert_eq!(web.name.lines, vec!["Web app"]);
        assert_eq!(web.name.weight, Weight::Bold);
        assert_eq!(web.name.size_px, 14);
        assert_eq!(web.name.align, Align::Middle);
        assert_eq!(web.name.at.x, web.rect.centre().x);
        assert_eq!(web.type_line.lines, vec!["[Container: Rust]"]);
        assert_eq!((web.type_line.size_px, web.type_line.line_px), (11, 14));
        assert_eq!(web.type_line.weight, Weight::Regular);
        let desc = web.description.as_ref().expect("description");
        assert_eq!(
            desc.lines.join(" "),
            "Serves the shop pages and takes orders from the operator."
        );
        assert!(desc.lines.len() > 1, "wrapped: {:?}", desc.lines);
        assert_eq!((desc.size_px, desc.line_px), (11, 14));
        // Name, type line, description top to bottom, inside the box
        // with 10px padding, in the text colour.
        assert!(web.name.bounds.max.y <= web.type_line.bounds.min.y);
        assert!(web.type_line.bounds.max.y <= desc.bounds.min.y);
        let inner = web.rect.inflate(-10);
        for t in [&web.name, &web.type_line, desc] {
            assert!(inside(&t.bounds, &inner), "{:?} in {inner:?}", t.bounds);
            assert!(t.fits);
            assert_eq!(t.colour, web.name.colour);
        }
        assert_eq!(web.fill, plan.boxes[web.id.0 as usize].style.background);
        assert_eq!(web.name.colour, plan.boxes[web.id.0 as usize].style.color);

        let worker = scene_box(&scene, &plan, "Worker");
        assert_eq!(worker.type_line.lines, vec!["[Container]"]);
        assert_eq!(worker.description, None);
        let bank = scene_box(&scene, &plan, "Bank");
        assert_eq!(bank.type_line.lines, vec!["[External Software System]"]);
        assert_eq!(bank.kind, ElementKind::SoftwareSystem);
        let operator = scene_box(&scene, &plan, "Operator");
        assert_eq!(operator.type_line.lines, vec!["[Person]"]);
        assert_eq!(operator.element, plan.boxes[operator.id.0 as usize].element);
        // `boxes[i].id == BoxId(i)`.
        for (i, b) in scene.boxes.iter().enumerate() {
            assert_eq!(b.id.0 as usize, i);
        }
    }

    #[test]
    fn polish_03_mobile_device_names_stay_inside_the_screen() {
        for shape in ["MobileDevicePortrait", "MobileDeviceLandscape"] {
            let src = SHOP.replace(
                "element \"Database\" {\n        shape Cylinder\n      }",
                &format!(
                    "element \"Element\" {{\n        fontSize 35\n      }}\n      element \"Container\" {{\n        shape {shape}\n      }}"
                ),
            );
            let src = src.replace("container \"Worker\"", "container \"Mobile Device Portrait\"");
            let (plan, routes, labels) = staged(&src, "c2");
            let scene = build(&plan, &routes, &labels);
            let b = scene_box(&scene, &plan, "Mobile Device Portrait");
            let screen = text_area(b.shape, b.rect);
            assert!(b.name.fits, "{shape}: {:?} in {:?}", b.name.bounds, screen);
        }
    }

    #[test]
    fn polish_02_large_font_person_text_fits_the_box() {
        let src = SHOP
            .replace("Runs the shop day to day.", "A customer of the shop, with personal orders and accounts.")
            .replace(
                "element \"Person\" {",
                "element \"Person\" {\n        fontSize 22",
            );
        let (plan, routes, labels) = staged(&src, "c2");
        let scene = build(&plan, &routes, &labels);
        let b = scene_box(&scene, &plan, "Operator");
        let d = b.description.as_ref().expect("description");
        assert!(b.name.fits, "name {:?}", b.name.bounds);
        assert!(b.type_line.fits, "type {:?}", b.type_line.bounds);
        assert!(d.fits, "description {:?} in {:?}", d.bounds, b.rect);
    }

    #[test]
    fn polish_01_white_boxes_keep_visible_lines() {
        let src = SHOP.replace(
            "styles {",
            "styles {\n      element \"Element\" {\n        background #ffffff\n      }",
        );
        // Every box white: the line may not take the destination fill.
        let src = src
            .replace("background #08427b", "")
            .replace("background #438dd5", "")
            .replace("background #999999", "");
        let (plan, routes, labels) = staged(&src, "c2");
        let scene = build(&plan, &routes, &labels);
        assert!(!scene.lines.is_empty());
        let page = scene.background;
        for line in &scene.lines {
            assert!(
                contrast(line.stroke.colour, page) >= 3.0,
                "line {:?} is {:?} on {:?}",
                line.edge,
                line.stroke.colour,
                page
            );
        }
        for label in &scene.labels {
            assert!(contrast(label.text.colour, page) >= 3.0, "label {:?}", label.edge);
        }
    }

    #[test]
    fn core_3_line_takes_target_colour() {
        let (plan, routes, labels) = staged(SHOP, "c2");
        let scene = build(&plan, &routes, &labels);
        assert_eq!(scene.lines.len(), plan.edges.len());
        // redesign 2026-11-19: an unstyled line is `dg-line`, not its
        // destination's fill (owner-approved redesign; the CORE-3 conflict
        // is docs/deferred.md D-R2). A relationship colour still wins
        // (P2-419, tested in styles_themes).
        let dg_line = palette(Mode::Light).line;
        for line in &scene.lines {
            assert_eq!(line.stroke.colour, dg_line, "line {:?}", line.edge);
            assert_eq!(line.stroke.width_px, 2);
            assert_eq!(line.stroke.dash_px, None, "lines are solid");
        }
        // Lines into differently styled boxes share the one line colour.
        let bank = scene_box(&scene, &plan, "Bank");
        let web = scene_box(&scene, &plan, "Web app");
        let into = |b: &SceneBox| {
            scene
                .lines
                .iter()
                .find(|l| l.destination == b.id)
                .expect("a line into it")
                .stroke
                .colour
        };
        assert_eq!(into(bank), dg_line);
        assert_eq!(into(web), dg_line);
    }

    #[test]
    fn core_4_technology_off() {
        // No label text holds the technology; box type lines keep it.
        let src = SHOP.replace(
            "      subtitle \"The running parts of the shop.\"",
            "      subtitle \"The running parts of the shop.\"\n      technology off",
        );
        let (plan, routes, labels) = staged(&src, "c2");
        assert!(!plan.options.technology);
        let scene = build(&plan, &routes, &labels);
        assert!(!scene.options.technology);
        assert!(!scene.labels.is_empty());
        for label in &scene.labels {
            for line in &label.text.lines {
                assert!(!line.contains('['), "technology in label: {line}");
            }
        }
        assert_eq!(
            scene_box(&scene, &plan, "Web app").type_line.lines,
            vec!["[Container: Rust]"]
        );

        // With technology on, labels carry it.
        let (plan, routes, labels) = staged(SHOP, "c2");
        let scene = build(&plan, &routes, &labels);
        assert!(
            scene
                .labels
                .iter()
                .any(|l| l.text.lines.iter().any(|t| t == "[HTTPS]"))
        );
    }

    #[test]
    fn core_5_one_line_per_edge_with_all_relationships() {
        // A merged lifted line is drawn once, tagged with its winner and
        // listing every relationship it stands for.
        let (plan, routes, labels) = staged(LIFTED, "c2");
        let scene = build(&plan, &routes, &labels);
        assert_eq!(scene.lines.len(), plan.edges.len());
        assert_eq!(scene.lines.len(), 2, "operator -> front, front -> store");
        for (line, edge) in scene.lines.iter().zip(&plan.edges) {
            assert_eq!(line.edge, edge.id);
            assert_eq!(
                (line.source, line.destination),
                (edge.source, edge.destination)
            );
            assert_eq!(line.relationships, edge.relationships);
            assert_eq!(line.relationship, edge.relationships[0]);
        }
        let merged = scene
            .lines
            .iter()
            .find(|l| l.relationships.len() == 2)
            .expect("the two component relationships merge into one line");
        assert_eq!(
            scene.boxes[merged.source.0 as usize].name.lines,
            vec!["Front"]
        );
        assert_eq!(
            scene.boxes[merged.destination.0 as usize].name.lines,
            vec!["Store"]
        );
    }

    #[test]
    fn core_7_bounds_contain_every_line() {
        // A line that leaves the boxes' bounds (CORE-7 allows it) still
        // lies inside the scene, tip included.
        let (plan, _, _) = staged(SHOP, "c2");
        let mut lines = vec![None; plan.edges.len()];
        lines[0] = arrow(
            0,
            &[
                (0, 98),
                (0, -40),
                (-60, -40),
                (-60, 300),
                (30, 300),
                (30, 102),
            ],
        );
        let routes = manual(&plan, lines);
        let labels = place_labels(&plan, &routes);
        let scene = build(&plan, &routes, &labels);
        let bounds = scene.bounds;
        for line in &scene.lines {
            for &p in &line.points {
                assert!(bounds.contains(p), "{p:?} outside {bounds:?}");
            }
            if let Tip::Arrow(pts) = line.tip {
                assert!(pts.iter().all(|&p| bounds.contains(p)));
            }
            for c in &line.chevrons {
                assert!(c.points.iter().all(|&p| bounds.contains(p)));
            }
        }
        for item in scene
            .boxes
            .iter()
            .map(|b| b.rect)
            .chain(scene.labels.iter().map(|l| l.rect))
        {
            assert!(inside(&item, &bounds));
        }
        // With a margin: nothing touches the edge.
        assert!(inside(&drawing_extent(&scene).inflate(10), &bounds));
        assert!(bounds.min.x < -600 && bounds.min.y < -400 && bounds.max.y > 3000);
    }

    #[test]
    fn core_10_chevrons_along_line() {
        // One chevron at the middle of a 100px segment, three at 1/4,
        // 1/2, 3/4 of a 320px one, none within 8px of a label.
        let (plan, _, _) = staged(SHOP, "c2");
        let mut lines = vec![None; plan.edges.len()];
        // 100px east, 320px south, 90px east, 141px south-east.
        lines[0] = arrow(0, &[(0, 0), (10, 0), (10, 32), (19, 32), (29, 42)]);
        let routes = manual(&plan, lines);
        let scene = build(&plan, &routes, &no_labels(&plan));
        let line = &scene.lines[0];
        let at: Vec<(Point, Dir)> = line.chevrons.iter().map(|c| (c.at, c.dir)).collect();
        assert_eq!(
            at,
            vec![
                (Point::new(50, 0), Dir::E),
                (Point::new(100, 80), Dir::S),
                (Point::new(100, 160), Dir::S),
                (Point::new(100, 240), Dir::S),
                (Point::new(240, 370), Dir::SE),
            ]
        );
        // An open V pointing along the segment, 6px deep.
        for c in &line.chevrons {
            let (dx, dy) = c.dir.delta();
            let [a, apex, b] = c.points;
            let ahead = |p: Point| (p.x - c.at.x) * dx + (p.y - c.at.y) * dy;
            assert!(ahead(apex) > 0 && ahead(a) < 0 && ahead(b) < 0, "{c:?}");
            assert!((ahead(apex) - ahead(a)).abs() <= 6 * 2, "{c:?}");
            assert_ne!(a, b);
        }
        // The arrowhead: tip on the last vertex, back corners 10px back.
        let Tip::Arrow([tip, l, r]) = line.tip else {
            panic!("arrow: {:?}", line.tip);
        };
        assert_eq!(tip, Point::new(290, 420));
        assert_eq!(l.x + r.x, 2 * (290 - 7));
        assert_eq!(l.y + r.y, 2 * (420 - 7));

        // A label within 8px of the 1/2 chevron of the 320px segment
        // removes that chevron only.
        let mut labels = no_labels(&plan);
        labels.labels[0] = Some(Label {
            edge: EdgeId(0),
            lines: vec!["Reads".to_owned()],
            // px rect 50x20 centred on (130, 160): left edge x = 105.
            centre: Point::new(13, 16),
            width_px: 50,
            height_px: 20,
            fit: Fit::Clear,
            font_px: crate::label::LABEL_FONT_PX,
            line_px: crate::label::LABEL_LINE_PX,
            callout: None,
        });
        let scene = build(&plan, &routes, &labels);
        let at: Vec<Point> = scene.lines[0].chevrons.iter().map(|c| c.at).collect();
        assert_eq!(
            at,
            vec![
                Point::new(50, 0),
                Point::new(100, 80),
                Point::new(100, 240),
                Point::new(240, 370),
            ]
        );
    }

    #[test]
    fn core_8_join_dot_drawn() {
        // CORE-8: a joined line ends in a dot on its trunk, no arrowhead.
        let (plan, _, _) = staged(SHOP, "c2");
        let mut lines = vec![None; plan.edges.len()];
        lines[1] = arrow(1, &[(0, 0), (30, 0)]);
        lines[2] = joined(2, &[(10, 20), (10, 0)], 1);
        let routes = manual(&plan, lines);
        let scene = build(&plan, &routes, &no_labels(&plan));
        let trunk = &scene.lines[1];
        assert!(matches!(trunk.tip, Tip::Arrow(_)));
        assert_eq!(trunk.joins, None);
        let branch = &scene.lines[2];
        assert_eq!(
            branch.tip,
            Tip::Dot {
                at: Point::new(100, 0),
                radius_px: 4
            }
        );
        assert_eq!(branch.joins, Some(EdgeId(1)));
        assert_eq!(
            branch.points,
            vec![Point::new(100, 200), Point::new(100, 0)]
        );
        assert!(branch.routed && trunk.routed);
    }

    #[test]
    fn core_13_unroutable_line_drawn_and_listed() {
        // CORE-13: drawn straight from source box edge to destination
        // box edge on the centre line, with an arrowhead, `routed:
        // false`, in `unroutable()`, no label.
        let (plan, mut routes, _) = staged(SHOP, "c2");
        routes.lines[0] = None;
        routes.lines[3] = None;
        let labels = place_labels(&plan, &routes);
        let scene = build(&plan, &routes, &labels);
        assert_eq!(scene.unroutable(), vec![EdgeId(0), EdgeId(3)]);
        for e in [0, 3] {
            let line = &scene.lines[e];
            assert!(!line.routed);
            assert_eq!(line.joins, None);
            assert!(scene.labels.iter().all(|l| l.edge != line.edge), "no label");
            let src = &scene.boxes[line.source.0 as usize].rect;
            let dst = &scene.boxes[line.destination.0 as usize].rect;
            let (cs, cd) = (src.centre(), dst.centre());
            assert_eq!(line.points.len(), 2);
            let (a, b) = (line.points[0], line.points[1]);
            assert!(on_border(src, a), "{a:?} on {src:?}");
            assert!(on_border(dst, b), "{b:?} on {dst:?}");
            // On the line between the centres, within rounding.
            for p in [a, b] {
                let cross = i64::from(p.x - cs.x) * i64::from(cd.y - cs.y)
                    - i64::from(p.y - cs.y) * i64::from(cd.x - cs.x);
                let len = i64::from((cd.x - cs.x).abs() + (cd.y - cs.y).abs());
                assert!(cross.abs() <= len, "{p:?} off the centre line");
            }
            let Tip::Arrow([tip, ..]) = line.tip else {
                panic!("arrowhead: {:?}", line.tip);
            };
            assert_eq!(tip, b);
        }
        assert!(scene.lines[1].routed && scene.lines[2].routed);
        // Unroutable lines take no part in the crossing pairs.
        assert!(
            scene
                .crossing_pairs()
                .iter()
                .all(|&(a, b, _)| ![a, b].contains(&EdgeId(0)))
        );
    }

    #[test]
    fn core_14_title_key_clear_of_labels() {
        // Title, subtitle and key bounds are disjoint from every label,
        // box and line (docs/deferred.md row from label).
        for (src, key) in [(SHOP, "c2"), (SHOP, "c1"), (LIFTED, "c2")] {
            let (plan, routes, labels) = staged(src, key);
            let scene = build(&plan, &routes, &labels);
            assert!(!scene.labels.is_empty());
            let chrome: Vec<Rect> = [scene.title.as_ref(), scene.subtitle.as_ref()]
                .into_iter()
                .flatten()
                .map(|t| t.bounds)
                .chain([scene.legend.rect])
                .collect();
            let drawn: Vec<Rect> = scene
                .labels
                .iter()
                .map(|l| l.rect)
                .chain(scene.boxes.iter().map(|b| b.rect))
                .chain(scene.boundaries.iter().flat_map(|b| [b.rect, b.tag_rect]))
                .chain(scene.lines.iter().map(line_bounds))
                .collect();
            for c in &chrome {
                for d in &drawn {
                    assert!(!c.overlaps(d), "{key}: {c:?} overlaps {d:?}");
                }
                for l in &scene.lines {
                    for w in l.points.windows(2) {
                        assert!(!Segment::new(w[0], w[1]).passes_through(c));
                    }
                }
            }
        }
    }

    #[test]
    fn core_15_label_fit_and_relationship_carried() {
        // `Fit::OverLines` and `Fit::NoRoom` reach the scene label with
        // the edge's winning relationship (L23, L24).
        let (plan, routes, mut labels) = staged(SHOP, "c2");
        let template = labels
            .labels
            .iter()
            .flatten()
            .next()
            .expect("a label")
            .clone();
        let with = |e: u32, fit: Fit| {
            let mut l = template.clone();
            l.edge = EdgeId(e);
            l.fit = fit;
            Some(l)
        };
        labels.labels[0] = with(0, Fit::OverLines(vec![EdgeId(2)]));
        labels.labels[1] = with(1, Fit::NoRoom);
        labels.labels[2] = with(2, Fit::Clear);
        labels.labels[3] = None;
        let scene = build(&plan, &routes, &labels);
        let fits: Vec<(EdgeId, RelationshipId, Fit)> = scene
            .labels
            .iter()
            .map(|l| (l.edge, l.relationship, l.fit.clone()))
            .collect();
        let winner = |e: usize| plan.edges[e].winner();
        assert_eq!(
            fits,
            vec![
                (EdgeId(0), winner(0), Fit::OverLines(vec![EdgeId(2)])),
                (EdgeId(1), winner(1), Fit::NoRoom),
                (EdgeId(2), winner(2), Fit::Clear),
            ]
        );
        // The label rectangle is `label::Label`'s px rectangle, the text
        // inside it with the paddings.
        let l = &scene.labels[0];
        assert_eq!(l.rect.width(), template.width_px);
        assert_eq!(l.rect.height(), template.height_px);
        assert_eq!(
            l.rect.centre(),
            Point::new(template.centre.x * 10, template.centre.y * 10)
        );
        assert_eq!(l.text.lines, template.lines);
        assert_eq!((l.text.size_px, l.text.line_px), (11, 13));
        assert!(inside(&l.text.bounds, &l.rect.inflate(-3)));
        assert!(l.text.fits);
    }

    #[test]
    fn core_25_same_input_same_scene() {
        for (src, key) in [(SHOP, "c1"), (SHOP, "c2"), (LIFTED, "c2")] {
            let (plan, routes, labels) = staged(src, key);
            let first = build(&plan, &routes, &labels);
            assert_eq!(first, build(&plan, &routes, &labels));
            // The whole chain again, from the text.
            let (plan2, routes2, labels2) = staged(src, key);
            assert_eq!(first, build(&plan2, &routes2, &labels2));
        }
    }

    #[test]
    fn core_26_crossing_pairs_sum_to_crossings() {
        // Pairs ascending, `a < b`, counts sum to `crossings`; a joined
        // line ending on its trunk is no pair; an unroutable line is in
        // none.
        let (plan, _, _) = staged(SHOP, "c2");
        let mut lines = vec![None; plan.edges.len()];
        // Edge 1 is the trunk; edge 0 crosses it once; edge 2 joins it
        // (its dot on the trunk is no crossing); edge 3 is unroutable,
        // drawn straight through the others' boxes.
        lines[1] = arrow(1, &[(0, 10), (40, 10)]);
        lines[0] = arrow(0, &[(20, 0), (20, 20)]);
        lines[2] = joined(2, &[(30, 0), (30, 10)], 1);
        let mut routes = manual(&plan, lines);
        routes.crossings = 1;
        let scene = build(&plan, &routes, &no_labels(&plan));
        assert_eq!(scene.crossings, 1);
        assert_eq!(scene.crossing_pairs(), vec![(EdgeId(0), EdgeId(1), 1)]);

        // Without the join, the branch's end on the trunk counts.
        let mut routes = routes.clone();
        routes.lines[2] = arrow(2, &[(30, 0), (30, 10)]);
        let scene = build(&plan, &routes, &no_labels(&plan));
        assert_eq!(
            scene.crossing_pairs(),
            vec![(EdgeId(0), EdgeId(1), 1), (EdgeId(1), EdgeId(2), 1)]
        );

        // On routed views the pairs sum to the router's count.
        for (src, key) in [
            (SHOP, "c1"),
            (SHOP, "c2"),
            (LIFTED, "c2"),
            (EXAMPLE, "C3-serving"),
        ] {
            let (plan, routes, labels) = staged(src, key);
            let scene = build(&plan, &routes, &labels);
            let pairs = scene.crossing_pairs();
            assert!(
                pairs
                    .windows(2)
                    .all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1))
            );
            assert!(pairs.iter().all(|&(a, b, n)| a < b && n > 0));
            let sum: u32 = pairs.iter().map(|p| p.2).sum();
            assert_eq!(sum, scene.crossings, "{key}");
            assert_eq!(scene.crossings, routes.crossings);
        }
    }

    #[test]
    fn core_30_texts_wrapped_and_fit_flagged() {
        // Lines measured with `label::text_advance`; a text that cannot
        // fit has `fits == false`; `texts_fit` agrees with `build`.
        let src = SHOP
            .replace(
                "worker = container \"Worker\"",
                "worker = container \"Overlongunbreakablecontainername\"",
            )
            .replace(
                "\"Takes card payments.\"",
                "\"Takes card payments for every order, refunds returned goods, settles \
                 the takings with the shop each night, reports fraud to the operator and \
                 keeps every receipt for seven years as the law asks, answers every \
                 dispute within a week and sends the operator a monthly statement.\"",
            );
        let (plan, routes, labels) = staged(&src, "c2");
        let scene = build(&plan, &routes, &labels);
        let width_px =
            |t: &str, b: &TextBlock| (text_advance(t, b.weight) * b.size_px + EM - 1) / EM;

        // Wrapped greedily within the text width, measured with the table.
        let web = scene_box(&scene, &plan, "Web app");
        let area = web.rect.inflate(-10).width();
        let desc = web.description.as_ref().expect("description");
        for line in &desc.lines {
            assert!(width_px(line, desc) <= area, "{line}");
        }
        for w in desc.lines.windows(2) {
            let next = w[1].split(' ').next().expect("a word");
            assert!(
                width_px(&format!("{} {next}", w[0]), desc) > area,
                "greedy: {w:?}"
            );
        }
        let widest = desc
            .lines
            .iter()
            .map(|l| width_px(l, desc))
            .max()
            .expect("lines");
        assert_eq!(desc.bounds.width(), widest);
        assert_eq!(
            desc.bounds.height(),
            14 * i32::try_from(desc.lines.len()).expect("small")
        );

        // Too wide a word, too long a description: flagged, kept whole.
        let long = scene_box(&scene, &plan, "Overlongunbreakablecontainername");
        assert_eq!(long.name.lines, vec!["Overlongunbreakablecontainername"]);
        assert!(!long.name.fits);
        let bank = scene_box(&scene, &plan, "Bank");
        assert!(!bank.description.as_ref().expect("description").fits);
        assert!(bank.name.fits);

        // `texts_fit` agrees with `build`, and is monotone in the size.
        for b in &scene.boxes {
            let pb = &plan.boxes[b.id.0 as usize];
            let all =
                b.name.fits && b.type_line.fits && b.description.as_ref().is_none_or(|d| d.fits);
            assert_eq!(
                texts_fit(pb, pb.half_width, pb.half_height),
                all,
                "{}",
                pb.name
            );
            for hw in 4..20 {
                for hh in 3..16 {
                    if texts_fit(pb, hw, hh) {
                        assert!(
                            texts_fit(pb, hw + 1, hh) && texts_fit(pb, hw, hh + 1),
                            "{} {hw}x{hh}",
                            pb.name
                        );
                    }
                }
            }
        }
        let long_pb = &plan.boxes[long.id.0 as usize];
        assert!(!texts_fit(long_pb, long_pb.half_width, long_pb.half_height));
        assert!(texts_fit(long_pb, 20, long_pb.half_height));
        let bank_pb = &plan.boxes[bank.id.0 as usize];
        assert!(texts_fit(bank_pb, bank_pb.half_width, 20));
    }

    #[test]
    fn every_item_tagged_with_id() {
        // Every box, line, label and boundary maps through `model_id`;
        // title, subtitle and key map to `None`.
        let (plan, routes, labels) = staged(SHOP, "c2");
        let scene = build(&plan, &routes, &labels);
        for b in &scene.boxes {
            assert_eq!(
                scene.model_id(ItemKey::Box(b.id)),
                Some(ModelId::Element(b.element))
            );
            assert_eq!(b.element, plan.boxes[b.id.0 as usize].element);
        }
        for l in &scene.lines {
            let winner = plan.edges[l.edge.0 as usize].winner();
            assert_eq!(l.relationship, winner);
            assert_eq!(
                scene.model_id(ItemKey::Line(l.edge)),
                Some(ModelId::Relationship(winner))
            );
        }
        assert!(!scene.labels.is_empty());
        for l in &scene.labels {
            let winner = plan.edges[l.edge.0 as usize].winner();
            assert_eq!(
                scene.model_id(ItemKey::Label(l.edge)),
                Some(ModelId::Relationship(winner))
            );
        }
        let BoundaryOf::Element(scope) = scene.boundaries[0].of else {
            unreachable!("phase-1 scope boundary")
        };
        assert_eq!(
            scene.model_id(ItemKey::Boundary(0)),
            Some(ModelId::Element(scope))
        );
        assert_eq!(Some(scope), scene.scope.element());
        for key in [ItemKey::Title, ItemKey::Subtitle, ItemKey::Legend] {
            assert_eq!(scene.model_id(key), None);
        }
        // Keys not in this scene.
        let n = u32::try_from(scene.boxes.len()).expect("small");
        assert_eq!(scene.model_id(ItemKey::Box(BoxId(n))), None);
        assert_eq!(scene.model_id(ItemKey::Line(EdgeId(99))), None);
        assert_eq!(scene.model_id(ItemKey::Label(EdgeId(99))), None);
        assert_eq!(scene.model_id(ItemKey::Boundary(1)), None);
    }

    #[test]
    fn edit_1_reselect_by_element() {
        // `boxes_of` gives the box drawing an element (phase 1: one), and
        // nothing for an element not drawn in the view.
        let (plan, routes, labels) = staged(SHOP, "c2");
        let scene = build(&plan, &routes, &labels);
        for b in &scene.boxes {
            assert_eq!(scene.boxes_of(b.element), vec![b.id]);
        }
        // The scope is a boundary on a container view, not a box.
        let BoundaryOf::Element(shop) = scene.boundaries[0].of else {
            unreachable!("phase-1 scope boundary")
        };
        assert!(scene.boxes_of(shop).is_empty());
        assert!(scene.boxes_of(ElementId(999)).is_empty());
    }

    #[test]
    fn edit_5_lines_of_relationship_includes_merged() {
        // `lines_of` finds a merged lifted line both by its winner and by
        // a relationship merged into it; an undrawn one gives nothing.
        let (plan, routes, labels) = staged(LIFTED, "c2");
        let scene = build(&plan, &routes, &labels);
        let merged = scene
            .lines
            .iter()
            .find(|l| l.relationships.len() == 2)
            .expect("merged line");
        assert_eq!(scene.lines_of(merged.relationships[0]), vec![merged.edge]);
        assert_eq!(scene.lines_of(merged.relationships[1]), vec![merged.edge]);
        assert!(scene.lines_of(RelationshipId(999)).is_empty());
        for l in &scene.lines {
            assert_eq!(scene.lines_of(l.relationship), vec![l.edge]);
        }
    }

    #[test]
    fn edit_25_box_moves_before_lines() {
        // `move_box` shifts rect, outline and texts by the same offset
        // and sets `centres`; lines, labels and `crossings` are
        // unchanged; `bounds` contains the moved box; moving to the
        // current centre is a no-op.
        let (plan, routes, labels) = staged(SHOP, "c2");
        let scene = build(&plan, &routes, &labels);
        let web = scene_box(&scene, &plan, "Web app").id;
        let i = web.0 as usize;

        let mut same = scene.clone();
        same.move_box(web, scene.centres[i]);
        assert_eq!(same, scene);

        let mut moved = scene.clone();
        let to = Point::new(scene.centres[i].x + 100, scene.centres[i].y + 3);
        moved.move_box(web, to);
        assert_eq!(moved.centres[i], to);
        let (before, after) = (&scene.boxes[i], &moved.boxes[i]);
        let (dx, dy) = (1000, 30);
        assert_eq!(after.rect, shift_rect(before.rect, dx, dy));
        let Outline::Rect { rect, .. } = after.outline else {
            panic!("box outline");
        };
        assert_eq!(rect, after.rect);
        for (a, b) in [
            (&before.name, &after.name),
            (&before.type_line, &after.type_line),
        ]
        .into_iter()
        .chain(before.description.iter().zip(after.description.iter()))
        {
            assert_eq!(b.at, shift(a.at, dx, dy));
            assert_eq!(b.bounds, shift_rect(a.bounds, dx, dy));
            assert_eq!((&b.lines, b.fits), (&a.lines, a.fits));
        }
        assert_eq!(after.fill, before.fill);
        assert_eq!(moved.lines, scene.lines);
        assert_eq!(moved.labels, scene.labels);
        assert_eq!(moved.boundaries, scene.boundaries);
        assert_eq!(moved.legend, scene.legend);
        assert_eq!(moved.title, scene.title);
        assert_eq!(moved.crossings, scene.crossings);
        for (j, b) in moved.boxes.iter().enumerate() {
            if j != i {
                assert_eq!(b, &scene.boxes[j]);
                assert_eq!(moved.centres[j], scene.centres[j]);
            }
        }
        assert!(inside(&after.rect, &moved.bounds));
        assert!(inside(&scene.bounds, &moved.bounds), "bounds never shrink");

        // A person moves head and body together.
        let operator = scene_box(&scene, &plan, "Operator").id;
        let mut moved = scene.clone();
        let to = Point::new(
            scene.centres[operator.0 as usize].x - 2,
            scene.centres[operator.0 as usize].y,
        );
        moved.move_box(operator, to);
        let (
            Outline::Person {
                head_centre: h0,
                body: b0,
                ..
            },
            Outline::Person {
                head_centre: h1,
                body: b1,
                ..
            },
        ) = (
            scene.boxes[operator.0 as usize].outline,
            moved.boxes[operator.0 as usize].outline,
        )
        else {
            panic!("person outline");
        };
        assert_eq!((h1, b1), (shift(h0, -20, 0), shift_rect(b0, -20, 0)));
    }

    #[test]
    fn edit_28_hit_test_topmost_item() {
        // `hit` returns the topmost item in reverse paint order; empty
        // canvas gives `None`; `item_bounds` contains every hit point.
        let (plan, routes, labels) = staged(SHOP, "c2");
        let scene = build(&plan, &routes, &labels);
        let check = |at: Point, want: ItemKey| {
            assert_eq!(scene.hit(at, 4), Some(want), "at {at:?}");
            let r = scene.item_bounds(want).expect("in scene");
            assert!(
                r.inflate(4).contains(at),
                "{want:?} bounds {r:?} miss {at:?}"
            );
        };
        let over_label = |p: Point| scene.labels.iter().any(|l| l.rect.contains(p));
        // A box under no label, at its centre and on its border (where
        // its lines end: boxes paint over lines).
        for b in &scene.boxes {
            if !over_label(b.rect.centre()) {
                check(b.rect.centre(), ItemKey::Box(b.id));
            }
        }
        for l in &scene.lines {
            if l.routed && !over_label(l.points[0]) {
                check(l.points[0], ItemKey::Box(l.source));
            }
        }
        // Labels paint over lines.
        for l in &scene.labels {
            check(l.rect.centre(), ItemKey::Label(l.edge));
            assert_eq!(scene.item_bounds(ItemKey::Label(l.edge)), Some(l.rect));
        }
        // A line, on a segment clear of boxes and labels, and within the
        // tolerance beside it; well beside it is something else.
        let mut found = false;
        let near = |p: Point, own: EdgeId| {
            scene.boxes.iter().any(|b| b.rect.inflate(20).contains(p))
                || scene.labels.iter().any(|x| x.rect.inflate(20).contains(p))
                || scene
                    .lines
                    .iter()
                    .filter(|o| o.edge != own)
                    .any(|o| o.points.windows(2).any(|w| near_segment(w[0], w[1], p, 20)))
        };
        for l in &scene.lines {
            for w in l.points.windows(2) {
                let (dx, dy) = ((w[1].x - w[0].x).signum(), (w[1].y - w[0].y).signum());
                let steps = (w[1].x - w[0].x).abs().max((w[1].y - w[0].y).abs());
                for k in (0..=steps).step_by(10) {
                    let p = Point::new(w[0].x + dx * k, w[0].y + dy * k);
                    if near(p, l.edge) {
                        continue;
                    }
                    found = true;
                    check(p, ItemKey::Line(l.edge));
                    // 4px beside it, square to the segment (axis only).
                    if dx == 0 || dy == 0 {
                        let side = Point::new(p.x + 4 * dy.abs(), p.y + 4 * dx.abs());
                        check(side, ItemKey::Line(l.edge));
                        let far = Point::new(p.x + 15 * dy.abs(), p.y + 15 * dx.abs());
                        assert_ne!(scene.hit(far, 4), Some(ItemKey::Line(l.edge)));
                    }
                }
            }
        }
        assert!(found, "a line point clear of everything");
        // Title, subtitle, key.
        check(
            scene.title.as_ref().expect("title").bounds.centre(),
            ItemKey::Title,
        );
        check(
            scene.subtitle.as_ref().expect("subtitle").bounds.centre(),
            ItemKey::Subtitle,
        );
        check(
            scene.legend.entries[0].text.bounds.centre(),
            ItemKey::Legend,
        );
        assert_eq!(scene.item_bounds(ItemKey::Legend), Some(scene.legend.rect));
        // The boundary: on its dashed outline and on its tag, not inside.
        let boundary = &scene.boundaries[0];
        check(boundary.tag.bounds.centre(), ItemKey::Boundary(0));
        let r = boundary.rect;
        let edge = (r.min.y..r.max.y)
            .step_by(10)
            .map(|y| Point::new(r.min.x, y))
            .find(|&p| {
                scene
                    .lines
                    .iter()
                    .all(|l| !line_bounds(l).inflate(10).contains(p))
            })
            .expect("a free spot on the left edge");
        check(edge, ItemKey::Boundary(0));
        check(Point::new(edge.x + 3, edge.y), ItemKey::Boundary(0));
        assert_eq!(scene.item_bounds(ItemKey::Boundary(0)), Some(r));
        // Empty canvas.
        assert_eq!(scene.hit(scene.bounds.min, 4), None);
        assert_eq!(
            scene.hit(
                Point::new(scene.bounds.max.x - 1, scene.bounds.min.y + 1),
                4
            ),
            None
        );
        // Keys not in the scene.
        assert_eq!(scene.item_bounds(ItemKey::Line(EdgeId(99))), None);
        assert_eq!(scene.item_bounds(ItemKey::Boundary(3)), None);
        let line = &scene.lines[0];
        let lb = scene.item_bounds(ItemKey::Line(line.edge)).expect("line");
        assert!(line.points.iter().all(|&p| lb.contains(p)));
    }
}
