//! Resolves one view spec into a view plan: the boxes (id, size,
//! position if any) and edges left after include, exclude, lifting,
//! de-duplication, focus and hide (CORE-5).
//!
//! Depends on: `model`, `geom`.

// Shape of the API: `plan` turns one `ViewSpec` of a `Model` into a
// `ViewPlan`, plain data with pub fields that `place`, `route`, `label`
// and `scene` read without going back to the model (HLD §4: `route`,
// `label` and `scene` do not depend on `model`'s functions). So the plan
// carries everything they draw or test: per box its element, kind,
// resolved style (shape for CORE-9 ports), texts, size and position; per
// edge its ends, winning relationship, texts; the title, subtitle,
// options and the scope boundary. Routing one view needs only its plan
// (CORE-28).
//
// Boxes and edges are addressed by per-plan indices (`BoxId`,
// `EdgeId`) into `ViewPlan::boxes` / `ViewPlan::edges`. They are not the
// stable view-local key of SRS 4.2 (that key is what `at` and `size`
// lines address); identity across plans (CORE-29 previous scene,
// EDIT-25) is `PlanBox::element`. In phase 1 a box maps to exactly one
// element; phase 3 (deployment, dynamic views) draws one element or
// relationship several times, which the index keys already allow.
//
// Positions: the plan is immutable. `PlanBox::centre` holds the `at`
// value only; `place` returns every centre as a `Vec<Point>` indexed by
// `BoxId`, and `route`, `label` and `scene` take `(&ViewPlan, &[Point])`.
// A drag (EDIT-2, EDIT-25) changes the centres, never the plan.
//
// Units: every coordinate and size in the plan is in grid units (1 unit =
// 10px, `geom`), as `place` and `route` work in grid units. The `at` and
// `size` lines are px (`model::At`, `model::Size`); lint checks the px
// values as written (CORE-18, L5) from the model, not from the plan.
//
// DECISION: an off-grid `at` centre is snapped to the nearest grid point
// (`geom::snap_px_to_grid`) and a `size` that is not a multiple of 20px
// gives half-sizes rounded up to whole grid units, so every planned box
// is on the grid; lint still reports the line as written (L5).
// DECISION: boxes are in `Model::included` order (sorted by element id);
// edges are sorted by their winning relationship's id (source order), so
// the plan is deterministic (CORE-25).
// DECISION: the CORE-2 default size depends on the element kind only, as
// CORE-2 states; a `shape Person` container still gets 240x140.
// DECISION: `technology off` (CORE-4) is not applied to the plan data:
// edges keep the technology as written, and `label` and `scene` both
// read it through `PlanEdge::shown_technology`, so they cannot disagree.
// DECISION: CORE-5 "drawn once" applies to lifted lines only. Each
// relationship declared between two drawn elements is its own edge (two
// direct `a -> b` are two lines, as in Structurizr and as `model`'s
// `find_relationship` ordinal treats them). Lifted relationships on an
// ordered box pair merge into the first declared direct edge on that
// pair when there is one, else into one edge whose winner is the first
// declared of them.
// DECISION: `hide <a> -> <b>` removes every drawn edge whose source box
// is `a` and destination box is `b` (one direction only), after lifting.
// DECISION: two relationships in opposite directions between the same
// two boxes are two edges (CORE-5 de-duplicates per ordered pair).
// DECISION: a phase-1 boundary exists only on container and component
// views, around the scope's children that are drawn; a context view's
// scope is a box, not a boundary (CORE-1).
// DECISION: a boundary rectangle is its members' bounds inflated by 4
// grid units (40px, CORE-1), computed by `ViewPlan::boundary_rect` so
// `place` (CORE-23), `label` (CORE-14), `scene` and `lint` (L26) agree.
//
// DSL-7: a `focus` or `hide` id with no box of its own in the view (not
// included, or included only through a drawn ancestor) is lifted to its
// nearest drawn ancestor; when it has none it is ignored.

use std::collections::BTreeMap;

use crate::geom::{Point, Rect, snap_px_to_grid};
use crate::model::{
    ElementId, ElementKind, GroupId, ImpliedRelationships, Mode, Model, RelationshipId,
    ResolvedBoundaryStyle, ResolvedRelationshipStyle, ResolvedStyle, ViewKind, ViewOptions,
    ViewScope, ViewSpec,
};

/// Index of a box in `ViewPlan::boxes`, valid for one plan only. It is
/// not the stable view-local key of SRS 4.2 (what `at` and `size` lines
/// address); identity across plans is `PlanBox::element`. `place`,
/// `route`, `label` and `scene` key their per-box data by it. Serves
/// CORE-5, CORE-1.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BoxId(pub u32);

/// Index of an edge in `ViewPlan::edges`, valid for one plan only;
/// identity across plans is `PlanEdge::winner`. `route` reports an
/// unroutable line by it (CORE-13) and `label` reports a label by it
/// (CORE-15). Serves CORE-5.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeId(pub u32);

/// One box to draw: an element the view includes after `include` and
/// `exclude` (DSL-6), with its size (`size` line or CORE-2 default) and
/// its `at` position (DSL-7) if written (else unplaced, DSL-8). Carries
/// the texts, tags and style `scene` draws (CORE-1 key, CORE-2) and the
/// shape `route` needs for ports (CORE-9). Serves CORE-5, CORE-2, DSL-6,
/// DSL-7, DSL-8.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanBox {
    /// Its own index in `ViewPlan::boxes`.
    pub id: BoxId,
    /// The model element it draws: its identity across plans (CORE-29,
    /// EDIT-25), EDIT hit-testing, CORE-1 tagging.
    pub element: ElementId,
    /// Person, software system, container or component (CORE-2 type
    /// line, CORE-19 level checks).
    pub kind: ElementKind,
    /// True when the element is tagged `External` (CORE-2: type
    /// `External Software System`).
    pub external: bool,
    /// The element's effective tags in model order (`Element`, the kind
    /// tag, then declared tags): the CORE-1 key groups colours by them.
    pub tags: Vec<String>,
    /// The element's resolved style: fill, text colour, shape (DSL-15,
    /// CORE-2, CORE-3 line colour, CORE-9 ports).
    pub style: ResolvedStyle,
    /// Element name, unquoted.
    pub name: String,
    /// Element description, unquoted, if written.
    pub description: Option<String>,
    /// Element technology, unquoted, if written.
    pub technology: Option<String>,
    /// Box centre in grid units from its `at` line, `None` when the view
    /// has none (DSL-8). Never filled in later: `place` returns every
    /// centre as a `Vec<Point>` indexed by `BoxId` (CORE-22).
    pub centre: Option<Point>,
    /// Half the width in grid units, from the `size` line or the CORE-2
    /// default.
    pub half_width: i32,
    /// Half the height in grid units, from the `size` line or the CORE-2
    /// default.
    pub half_height: i32,
}

impl PlanBox {
    /// The box rectangle in grid units at its `at` centre, `None` when
    /// it has no `at` line. Serves DSL-7, CORE-18.
    pub const fn rect(&self) -> Option<Rect> {
        match self.centre {
            Some(centre) => Some(self.rect_at(centre)),
            None => None,
        }
    }

    /// The box rectangle in grid units at `centre` (from `place`). Serves
    /// CORE-7, CORE-18, CORE-22.
    pub const fn rect_at(&self, centre: Point) -> Rect {
        Rect::from_centre(centre, self.half_width, self.half_height)
    }
}

/// One line to draw: one relationship declared between two drawn
/// elements, or one or more lifted relationships on the same ordered
/// pair of boxes, drawn once (CORE-5). Lifted relationships merge into
/// the first declared direct edge on their pair when there is one. Texts
/// are the winner's: the direct relationship, otherwise the first
/// declared lifted one. Serves CORE-5, CORE-14.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanEdge {
    /// Its own index in `ViewPlan::edges`.
    pub id: EdgeId,
    /// The box the line starts at.
    pub source: BoxId,
    /// The box the line points to (CORE-3: the line takes its colour).
    pub destination: BoxId,
    /// Every relationship this line stands for, winner first, then the
    /// merged lifted ones in source order; never empty (CORE-21
    /// coverage, L22).
    pub relationships: Vec<RelationshipId>,
    /// The winner's description, unquoted, if written (CORE-14 label).
    pub description: Option<String>,
    /// The winner's technology, unquoted, as written; read it through
    /// `shown_technology` (CORE-4).
    pub technology: Option<String>,
    /// The winner's resolved relationship style in the plan's mode, with
    /// the view's per-view `line` layout (position, routing, jump) folded
    /// in over it (O9); `ResolvedRelationshipStyle::DEFAULT` for an
    /// unstyled line. `route`, `label` and `scene` read it. Serves P2-12,
    /// P2-418..P2-426.
    pub style: ResolvedRelationshipStyle,
    /// The key entry the line belongs to (P2-27): the last tag of the
    /// winner, in DSL-15 order, that has a relationship style block in
    /// the plan's mode and is not `Relationship`; `None` for the shared
    /// `Relationship` entry.
    // DECISION: crate-private, not pub: `scene` builds the key from the
    // plan alone (HLD: scene does not call model), and the frozen pub
    // surface has no field for it. Listed in docs/deferred.md in case the
    // owner wants it pub.
    pub(crate) style_tag: Option<String>,
}

impl PlanEdge {
    /// The winning relationship, `relationships[0]`: the line's model id
    /// and its identity across plans (CORE-1 tagging, CORE-15 and L1
    /// report names). Serves CORE-5.
    pub fn winner(&self) -> RelationshipId {
        self.relationships[0]
    }

    /// The technology to draw and size the label with: `technology`,
    /// or `None` under `technology off`. `label` and `scene` both call
    /// it. Serves CORE-4, CORE-14.
    pub fn shown_technology(&self, options: &ViewOptions) -> Option<&str> {
        if options.technology {
            self.technology.as_deref()
        } else {
            None
        }
    }
}

/// What a boundary stands for. `#[non_exhaustive]` so phase 2 adds group
/// boundaries (P2-4), which have no element, without a breaking change.
/// Serves CORE-1.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoundaryOf {
    /// An element's drawn children (the view's scope in phase 1).
    Element(ElementId),
    /// A group's drawn members: dashed, the group's name as its name tag
    /// with no bracketed type, nested inside its enclosing group's
    /// boundary. Emitted only when a member is drawn, outer before inner.
    /// Serves P2-130, P2-132.
    // TODO(srs): P2-130, the name tag of a nested group. Reading taken
    // (Structurizr UI): the last segment of the full name. The default
    // colour: the phase-1 boundary look (B dsl-breadth 14c). Whether
    // `structurizr.groups false` hides it: yes (B dsl-breadth 14d).
    Group(GroupId),
}

/// A dashed boundary around drawn boxes (CORE-1). Its rectangle is the
/// members' bounds plus 40px (`ViewPlan::boundary_rect`); the name tag
/// sits at its bottom left. Phase 1 has at most one, around the scope;
/// phase 2 adds nested boundaries (groups, enclosing systems). Serves
/// CORE-1.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Boundary {
    /// What the boundary stands for.
    pub of: BoundaryOf,
    /// Its name, unquoted, drawn as the boundary's name tag.
    pub name: String,
    /// The boxes inside, in `ViewPlan::boxes` order; never empty (a
    /// scope with no drawn child has no boundary).
    pub members: Vec<BoxId>,
    /// Its look from the `Boundary*` or `Group*` element styles in the
    /// plan's mode (`Model::boundary_style_in`). Serves P2-411, P2-412.
    pub style: ResolvedBoundaryStyle,
}

/// Everything one view draws, before placement and routing: boxes,
/// edges, boundaries, title. Immutable once built: `place` returns the
/// centres, `route` routes the edges, `label` places their labels and
/// `scene` draws it all, each from `(&ViewPlan, &[Point])`. Serves
/// CORE-5, CORE-1, DSL-6, DSL-7, DSL-8.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewPlan {
    /// The view's effective key (names the SVG file, CLI-1).
    pub key: String,
    /// System context, container or component.
    pub kind: ViewKind,
    /// The element the view is about.
    pub scope: ViewScope,
    /// The drawn title: the `title` line, else the description; `None`
    /// is lint L8 (DSL-6, CORE-1).
    pub title: Option<String>,
    /// The `subtitle` line, if written (CORE-1).
    pub subtitle: Option<String>,
    /// The view options with defaults filled in (CORE-4, CORE-8,
    /// CORE-14, CORE-17).
    pub options: ViewOptions,
    /// Every box, `boxes[i].id == BoxId(i)`.
    pub boxes: Vec<PlanBox>,
    /// Every line after lifting, de-duplication, `focus` and `hide`,
    /// `edges[i].id == EdgeId(i)`.
    pub edges: Vec<PlanEdge>,
    /// The dashed boundaries, outer before inner: the scope's first
    /// (phase 1 holds at most that one, CORE-1), then one per group with a
    /// drawn member (P2-130).
    pub boundaries: Vec<Boundary>,
    /// The mode styles were resolved in; `scene` draws its defaults
    /// (P2-431). Serves P2-430.
    pub mode: Mode,
}

impl ViewPlan {
    /// Every box drawing `element`, in `boxes` order; empty when the
    /// view does not draw it. Phase 1 yields at most one; phase 3
    /// (P3-2, P3-3) may yield several. Serves CORE-5, EDIT hit-testing.
    pub fn boxes_of(&self, element: ElementId) -> Vec<BoxId> {
        self.boxes
            .iter()
            .filter(|b| b.element == element)
            .map(|b| b.id)
            .collect()
    }

    /// The boxes with no `at` line, in `boxes` order: the ones `place`
    /// positions (DSL-8, CORE-22). Same set as `Model::unplaced`; it is
    /// here because `place` must not call `model`'s functions (HLD §4).
    pub fn unplaced(&self) -> Vec<BoxId> {
        self.boxes
            .iter()
            .filter(|b| b.centre.is_none())
            .map(|b| b.id)
            .collect()
    }

    /// The rectangle of `boundaries[index]` in grid units with every box
    /// at `centres[box]`: its members' bounds, together with the
    /// rectangles of the boundaries nested in it (`nests`), inflated by
    /// 4 grid units (40px). With one boundary (phase 1) that is the
    /// members' bounds plus 40px. `None` when `index` is out of range.
    /// `place` (CORE-23), `label` (CORE-14 name tag), `scene` (CORE-1) and
    /// `lint` (L26) all use it. Serves CORE-1, P2-130.
    ///
    /// # Panics
    ///
    /// When `centres` is shorter than `boxes`.
    // DECISION: a nested boundary's rectangle counts as content of the
    // boundary around it (P2-130 "sits inside its enclosing group's
    // boundary"), so an outer boundary can be wider than its members'
    // bounds plus 40px; with one boundary nothing changes (docs/deferred.md).
    pub fn boundary_rect(&self, index: usize, centres: &[Point]) -> Option<Rect> {
        assert!(
            centres.len() >= self.boxes.len(),
            "boundary_rect: {} centres for {} boxes",
            centres.len(),
            self.boxes.len()
        );
        self.boundary_rect_by(index, &|id: BoxId| {
            Some(self.boxes[id.0 as usize].rect_at(centres[id.0 as usize]))
        })
    }

    /// `boundary_rect` with each member's rectangle from `rect` (`None`:
    /// leave that member out); `place` passes only the boxes it has
    /// placed so far. `None` when `index` is out of range or no member
    /// has a rectangle.
    pub(crate) fn boundary_rect_by(
        &self,
        index: usize,
        rect: &dyn Fn(BoxId) -> Option<Rect>,
    ) -> Option<Rect> {
        let boundary = self.boundaries.get(index)?;
        let nested = (0..self.boundaries.len())
            .filter(|&j| self.nests(index, j))
            .filter_map(|j| self.boundary_rect_by(j, rect));
        let bounds = boundary
            .members
            .iter()
            .filter_map(|&id| rect(id))
            .chain(nested)
            .reduce(union)?;
        Some(bounds.inflate(BOUNDARY_MARGIN_UNITS))
    }

    /// True when boundary `inner` sits inside boundary `outer`: its
    /// members are a subset of `outer`'s, and with equal members the one
    /// listed later is the inner one (outer before inner, P2-130).
    /// Boundaries where neither nests in the other keep apart (`place`).
    pub(crate) fn nests(&self, outer: usize, inner: usize) -> bool {
        if outer == inner {
            return false;
        }
        let (Some(o), Some(i)) = (self.boundaries.get(outer), self.boundaries.get(inner)) else {
            return false;
        };
        i.members.iter().all(|m| o.members.contains(m))
            && (i.members.len() < o.members.len() || outer < inner)
    }
}

/// The smallest rectangle holding `a` and `b`.
fn union(a: Rect, b: Rect) -> Rect {
    Rect::from_corners(
        Point::new(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
        Point::new(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
    )
}

/// Resolves `view` of `model` into its plan: boxes from
/// `Model::included` with their `size` lines (else the CORE-2 default by
/// kind) and `at` lines; every relationship lifted to the nearest drawn
/// ancestors of its ends, dropped when both ends lift to the same box or
/// either has no drawn ancestor; each direct relationship its own edge,
/// lifted ones merged per ordered box pair (CORE-5); then only lines
/// touching a `focus` box are kept, and `hide` lines removed; a `focus`
/// or `hide` id with no box of its own is lifted to its nearest drawn
/// ancestor, and ignored when it has none (DSL-7). Serves
/// CORE-5, CORE-1, CORE-2, DSL-6, DSL-7, DSL-8.
///
/// # Panics
///
/// When `view` does not belong to `model`: its element ids index out of
/// range.
pub fn plan(model: &Model, view: &ViewSpec) -> ViewPlan {
    plan_in(model, view, Mode::Light)
}

/// `plan` in `mode`: box styles from `Model::style_of_in` (a resolved
/// `width`/`height` replaces that dimension of the CORE-2 kind default,
/// and a `size` line still wins), each edge's `style` from
/// `Model::relationship_style_of_in` of its winner, each boundary's
/// `style` from `Model::boundary_style_in`, and `ViewPlan::mode` set.
/// `plan(model, view)` equals `plan_in(model, view, Mode::Light)`.
/// Serves P2-430, P2-12, P2-402, P2-411, P2-412.
///
/// # Panics
///
/// As `plan`.
pub fn plan_in(model: &Model, view: &ViewSpec, mode: Mode) -> ViewPlan {
    let boxes: Vec<PlanBox> = model
        .included(view)
        .into_iter()
        .enumerate()
        .map(|(i, element)| plan_box(model, view, index_u32(i), element, mode))
        .collect();
    let mut edges = plan_edges(model, &boxes);
    filter_focus_and_hide(model, view, &boxes, &mut edges);
    for (i, e) in edges.iter_mut().enumerate() {
        e.id = EdgeId(index_u32(i));
        e.style = model.relationship_style_of_in(e.winner(), mode);
        e.style_tag = style_tag(model, e.winner(), mode);
    }
    let mut boundaries: Vec<Boundary> = scope_boundary(model, view, &boxes, mode).into_iter().collect();
    boundaries.extend(group_boundaries(model, view, &boxes, mode));
    ViewPlan {
        key: view.key.clone(),
        kind: view.kind,
        scope: view.scope.clone(),
        title: view.drawn_title().map(|t| t.value.clone()),
        subtitle: view.subtitle.as_ref().map(|t| t.value.clone()),
        options: view.options(),
        boxes,
        edges,
        boundaries,
        mode,
    }
}

/// The CORE-1 boundary margin: 40px.
const BOUNDARY_MARGIN_UNITS: i32 = 4;

/// Keeps only edges touching a `focus` box, then drops `hide` pairs;
/// ids are lifted to their nearest drawn ancestor or ignored (DSL-7).
fn filter_focus_and_hide(
    model: &Model,
    view: &ViewSpec,
    boxes: &[PlanBox],
    edges: &mut Vec<PlanEdge>,
) {
    let focus: Vec<BoxId> = view
        .focus
        .iter()
        .filter_map(|f| drawn_box(model, boxes, f.value))
        .collect();
    // DECISION: when every `focus` id is ignored (no drawn ancestor), the
    // view has no focus and keeps every line, as if no id were written.
    if !focus.is_empty() {
        edges.retain(|e| focus.contains(&e.source) || focus.contains(&e.destination));
    }
    for hide in &view.hide {
        let (Some(source), Some(destination)) = (
            drawn_box(model, boxes, hide.source.value),
            drawn_box(model, boxes, hide.destination.value),
        ) else {
            continue;
        };
        edges.retain(|e| !(e.source == source && e.destination == destination));
    }
}

/// One boundary per group with a drawn member (P2-130), in
/// `Model::groups` order, so an outer group comes before the groups
/// nested in it. A group's members are the boxes whose element sits in
/// it or in a group nested in it. None when `structurizr.groups` is
/// `false` on the view, or on the view set when the view does not set it
/// (B dsl-breadth 14d).
// DECISION: the name tag is the group's own name (`Group::name`, the
// last segment of a nested name), as the Structurizr UI draws it; the
// look is the phase-1 boundary's (B dsl-breadth 14c). Both close the
// P2-130 TODO(srs) readings in `BoundaryOf::Group`.
fn group_boundaries(model: &Model, view: &ViewSpec, boxes: &[PlanBox], mode: Mode) -> Vec<Boundary> {
    let setting = |props: &[crate::model::Property]| {
        props
            .iter()
            .rev()
            .find(|p| p.name.value == "structurizr.groups")
            .map(|p| !p.value.value.trim().eq_ignore_ascii_case("false"))
    };
    let shown = setting(&view.properties)
        .or_else(|| setting(&model.views_properties))
        .unwrap_or(true);
    if !shown || model.groups.is_empty() {
        return Vec::new();
    }
    // Every group an element sits in, innermost first.
    let chain = |e: ElementId| {
        let mut out = Vec::new();
        let mut g = model.element(e).group;
        while let Some(id) = g {
            // DECISION: a parent link that loops (never built by `model`)
            // stops the walk instead of hanging.
            if out.contains(&id) {
                break;
            }
            out.push(id);
            g = model.groups.get(id.0 as usize).and_then(|x| x.parent);
        }
        out
    };
    let chains: Vec<Vec<GroupId>> = boxes.iter().map(|b| chain(b.element)).collect();
    model
        .groups
        .iter()
        .filter_map(|g| {
            let members: Vec<BoxId> = boxes
                .iter()
                .zip(&chains)
                .filter(|(_, c)| c.contains(&g.id))
                .map(|(b, _)| b.id)
                .collect();
            (!members.is_empty()).then(|| Boundary {
                of: BoundaryOf::Group(g.id),
                name: g.name.value.clone(),
                members,
                // P2-412: `Group`, then `Group:<full name>` (B dsl-breadth
                // 14c), styled as any boundary.
                style: model.boundary_style_in(&["Group", &format!("Group:{}", g.full_name)], mode),
            })
        })
        .collect()
}

/// The phase-1 boundary: on container and component views, the scope's
/// drawn children; `None` on a context view or with no drawn child
/// (CORE-1).
fn scope_boundary(model: &Model, view: &ViewSpec, boxes: &[PlanBox], mode: Mode) -> Option<Boundary> {
    let scope = view.scope.element()?;
    match view.kind {
        ViewKind::Container | ViewKind::Component => {}
        // A context view has none; the kinds after phase 1 are never
        // planned in phase 2 (P2-2).
        _ => return None,
    }
    let members: Vec<BoxId> = boxes
        .iter()
        .filter(|b| model.element(b.element).parent == Some(scope))
        .map(|b| b.id)
        .collect();
    if members.is_empty() {
        return None;
    }
    // P2-411: `Boundary`, then the scope kind's tag.
    let kind_tag = match model.element(scope).kind {
        ElementKind::Container => "Boundary:Container",
        _ => "Boundary:SoftwareSystem",
    };
    Some(Boundary {
        of: BoundaryOf::Element(scope),
        name: model.element(scope).name.value.clone(),
        members,
        style: model.boundary_style_in(&["Boundary", kind_tag], mode),
    })
}

/// The dark-mode defaults of a box (P2-431): a fill and a text colour no
/// style block sets become #333333 and #ffffff. The model's resolved
/// style stays mode-independent (P2-430); only the drawing changes.
// TODO(srs): P2-431. Reading taken (B styles-themes 10c): background
// #111111, box text #ffffff, default box fill #333333, line and boundary
// #aaaaaa (the last three in `scene`).
fn dark_defaults(model: &Model, element: ElementId, style: &mut ResolvedStyle) {
    let blocks = model.styles_for_in(element, Mode::Dark);
    let sets = |f: fn(&crate::model::ElementStyle) -> bool| blocks.iter().any(|&i| f(&model.element_styles[i]));
    if !sets(|s| s.background.is_some()) {
        style.background = crate::model::Colour {
            r: 0x33,
            g: 0x33,
            b: 0x33,
        };
    }
    if !sets(|s| s.color.is_some()) {
        style.color = crate::model::Colour {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        };
    }
}

/// Half of a `size` length in px, in whole grid units rounded up.
fn half_size_units(px: i32) -> i32 {
    let double_unit = 2 * crate::geom::GRID_PX;
    px.saturating_add(double_unit - 1).div_euclid(double_unit)
}

/// `i` as a plan index. Plans hold far fewer than `u32::MAX` items.
fn index_u32(i: usize) -> u32 {
    u32::try_from(i).expect("plan index fits in u32")
}

/// The CORE-2 default half-sizes of a box of `kind`, in grid units.
const fn default_half_size(kind: ElementKind) -> (i32, i32) {
    match kind {
        // 200x180 px
        ElementKind::Person => (10, 9),
        // 240x140 px. DECISION: a custom element (P2-170) takes the
        // software system size: Structurizr gives every box one default.
        ElementKind::SoftwareSystem | ElementKind::Container | ElementKind::Custom => (12, 7),
        // 200x120 px
        ElementKind::Component => (10, 6),
    }
}

/// The box drawing `element`, at index `index`, styled in `mode`.
fn plan_box(
    model: &Model,
    view: &ViewSpec,
    index: u32,
    element: ElementId,
    mode: Mode,
) -> PlanBox {
    let e = model.element(element);
    let mut style = model.style_of_in(element, mode);
    if mode == Mode::Dark {
        dark_defaults(model, element, &mut style);
    }
    // P2-402: a `size` line wins; else a resolved `width`/`height`
    // replaces that dimension of the kind default, each on its own.
    // DECISION: the half size rounds up to whole grid units as for a
    // `size` line, so a styled width of 310px draws 320px wide.
    let (half_width, half_height) = if let Some(size) = view.size_of(element) {
        (
            half_size_units(size.width.value),
            half_size_units(size.height.value),
        )
    } else {
        let (w, h) = default_half_size(e.kind);
        (
            style.width.map_or(w, half_size_units),
            style.height.map_or(h, half_size_units),
        )
    };
    let centre = view
        .position_of(element)
        .map(|at| Point::new(snap_px_to_grid(at.x.value), snap_px_to_grid(at.y.value)));
    PlanBox {
        id: BoxId(index),
        element,
        kind: e.kind,
        external: e.is_external(),
        tags: e.tags.clone(),
        style,
        name: e.name.value.clone(),
        description: e.description.as_ref().map(|d| d.value.clone()),
        // DECISION: a custom element shows its metadata where other boxes
        // show their technology (P2-170; Structurizr draws `[metadata]`).
        technology: match e.kind {
            ElementKind::Custom => e.metadata.as_ref().map(|m| m.value.clone()),
            _ => e.technology.as_ref().map(|t| t.value.clone()),
        },
        centre,
        half_width,
        half_height,
    }
}

/// The box drawing `element` or, failing that, its nearest ancestor
/// with a box; `None` when no ancestor is drawn (CORE-5, DSL-7).
fn drawn_box(model: &Model, boxes: &[PlanBox], element: ElementId) -> Option<BoxId> {
    let mut current = Some(element);
    while let Some(e) = current {
        // Phase 1 draws an element at most once.
        if let Some(b) = boxes.iter().find(|b| b.element == e) {
            return Some(b.id);
        }
        current = model.element(e).parent;
    }
    None
}

/// True when `ancestor` is a strict ancestor of `element`.
fn is_ancestor(model: &Model, ancestor: ElementId, element: ElementId) -> bool {
    let mut current = model.element(element).parent;
    while let Some(p) = current {
        if p == ancestor {
            return true;
        }
        current = model.element(p).parent;
    }
    false
}

/// The P2-27 key tag of `relationship` in `mode` (`PlanEdge::style_tag`).
fn style_tag(model: &Model, relationship: RelationshipId, mode: Mode) -> Option<String> {
    model
        .relationship(relationship)
        .tags
        .iter()
        .rev()
        .filter(|t| t.as_str() != "Relationship")
        .find(|t| {
            model
                .relationship_styles
                .iter()
                .any(|s| s.tag.value == **t && s.mode.is_none_or(|m| m == mode))
        })
        .cloned()
}

/// Every line after lifting and de-duplication (CORE-5), sorted by the
/// winning relationship's id.
///
/// Per CORE-5, a lifted line between an element's box and the box of
/// one of its own ancestors is dropped, in either direction, as
/// Structurizr creates no implied relationship between parent and
/// child. A relationship written directly between a parent and its
/// child is still drawn.
fn plan_edges(model: &Model, boxes: &[PlanBox]) -> Vec<PlanEdge> {
    let new_edge = |r: &crate::model::Relationship, source: BoxId, destination: BoxId| PlanEdge {
        id: EdgeId(0),
        source,
        destination,
        relationships: vec![r.id],
        description: r.description.as_ref().map(|d| d.value.clone()),
        technology: r.technology.as_ref().map(|t| t.value.clone()),
        style: ResolvedRelationshipStyle::DEFAULT,
        style_tag: None,
    };
    let mut edges: Vec<PlanEdge> = Vec::new();
    // Lifted relationships per ordered box pair, in relationship order.
    let mut lifted: BTreeMap<(BoxId, BoxId), Vec<RelationshipId>> = BTreeMap::new();
    for r in &model.relationships {
        let (s, d) = (r.source.value, r.destination.value);
        let (Some(source), Some(destination)) =
            (drawn_box(model, boxes, s), drawn_box(model, boxes, d))
        else {
            continue;
        };
        if source == destination {
            continue;
        }
        let (source_element, destination_element) = (
            boxes[source.0 as usize].element,
            boxes[destination.0 as usize].element,
        );
        let direct = source_element == s && destination_element == d;
        if direct {
            edges.push(new_edge(r, source, destination));
        } else if r.implied_relationships == ImpliedRelationships::False {
            // P2-7: a relationship declared under `!impliedRelationships
            // false` lifts to nothing.
        } else if is_ancestor(model, source_element, destination_element)
            || is_ancestor(model, destination_element, source_element)
        {
            // CORE-5: a lifted parent-child line is dropped.
        } else {
            lifted.entry((source, destination)).or_default().push(r.id);
        }
    }
    for ((source, destination), ids) in lifted {
        // `edges` holds direct edges only here, in relationship order, so
        // the first match is the first declared direct one.
        if let Some(edge) = edges
            .iter_mut()
            .find(|e| e.source == source && e.destination == destination)
        {
            edge.relationships.extend(ids);
        } else {
            let mut edge = new_edge(model.relationship(ids[0]), source, destination);
            edge.relationships = ids;
            edges.push(edge);
        }
    }
    // Ids are assigned by `plan` once focus and hide have run.
    edges.sort_by_key(PlanEdge::winner);
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse;

    /// Roles only. Ids: user 0, admin 1, mail 2, other 3, shop 4, web 5,
    /// api 6, auth 7, db 8, job 9. Relationships r0..r4 in this order.
    const SHOP: &str = r#"        user = person "Operator"
        admin = person "Admin"
        mail = softwareSystem "Mail" "Sends mail." "External"
        other = softwareSystem "Other"
        shop = softwareSystem "Shop" {
            web = container "Web" {
                api = component "Api"
                auth = component "Auth"
            }
            db = container "Db"
            job = container "Job"
        }
        user -> web "Uses"
        admin -> shop "Manages"
        api -> db "Reads"
        api -> mail "Sends"
        job -> api "Calls""#;

    /// `model_body` and `views_body` inside a workspace.
    fn workspace(model_body: &str, views_body: &str) -> String {
        format!(
            "workspace {{\n    model {{\n{model_body}\n    }}\n    views {{\n{views_body}\n    }}\n}}\n"
        )
    }

    /// The model of `source`, panicking with the error text on failure.
    fn model(source: &str) -> Model {
        match parse(source) {
            Ok(parsed) => parsed.model,
            Err(e) => panic!("{e}"),
        }
    }

    /// The plan of the view keyed `key`.
    fn plan_of(m: &Model, key: &str) -> ViewPlan {
        plan(m, m.view(key).expect("view exists"))
    }

    /// The key of `element`.
    fn key(m: &Model, element: ElementId) -> String {
        m.element(element)
            .key
            .as_ref()
            .expect("keyed")
            .value
            .clone()
    }

    /// The element keys of every box, in plan order.
    fn box_keys(m: &Model, p: &ViewPlan) -> Vec<String> {
        p.boxes.iter().map(|b| key(m, b.element)).collect()
    }

    /// Every edge as `source -> destination: description`, in plan order.
    fn edge_lines(m: &Model, p: &ViewPlan) -> Vec<String> {
        p.edges
            .iter()
            .map(|e| {
                let s = key(m, p.boxes[e.source.0 as usize].element);
                let d = key(m, p.boxes[e.destination.0 as usize].element);
                format!("{s} -> {d}: {}", e.description.as_deref().unwrap_or(""))
            })
            .collect()
    }

    /// The context view of `shop` with `include *` over `SHOP` plus
    /// `extra` relationship lines.
    fn context_plan(extra: &str) -> (Model, ViewPlan) {
        let body = format!("{SHOP}\n{extra}");
        let m = model(&workspace(
            &body,
            "        systemContext shop \"c1\" {\n            include *\n        }",
        ));
        let p = plan_of(&m, "c1");
        (m, p)
    }

    /// The container view of `shop` with `include *` and `lines` in its
    /// body, over `SHOP` plus `extra` relationship lines.
    fn container_plan(extra: &str, lines: &str) -> (Model, ViewPlan) {
        let body = format!("{SHOP}\n{extra}");
        let m = model(&workspace(
            &body,
            &format!(
                "        container shop \"c2\" {{\n            include *\n{lines}\n        }}"
            ),
        ));
        let p = plan_of(&m, "c2");
        (m, p)
    }

    /// The box of the element keyed `k`.
    fn box_named<'p>(m: &Model, p: &'p ViewPlan, k: &str) -> &'p PlanBox {
        let id = p.boxes_of(m.find(k).unwrap())[0];
        &p.boxes[id.0 as usize]
    }

    /// The plan's indices are its positions.
    fn assert_indexed(p: &ViewPlan) {
        for (i, b) in p.boxes.iter().enumerate() {
            assert_eq!(b.id, BoxId(u32::try_from(i).unwrap()));
        }
        for (i, e) in p.edges.iter().enumerate() {
            assert_eq!(e.id, EdgeId(u32::try_from(i).unwrap()));
            assert!(!e.relationships.is_empty());
        }
    }

    // Size pins: per-plan keys index per-box and per-edge arrays in
    // place, route, label and scene.
    #[test]
    fn box_id_size_is_pinned() {
        assert_eq!(std::mem::size_of::<BoxId>(), 4);
    }

    #[test]
    fn edge_id_size_is_pinned() {
        assert_eq!(std::mem::size_of::<EdgeId>(), 4);
    }

    // CORE-5, one test per clause.
    #[test]
    fn core_5_lifted_to_nearest_drawn_ancestor() {
        // `api -> mail` lifts two levels to `shop`, the nearest drawn
        // ancestor of `api`; `user -> web` lifts one level.
        let (m, p) = context_plan("");
        assert_indexed(&p);
        assert_eq!(
            edge_lines(&m, &p),
            [
                "user -> shop: Uses",
                "admin -> shop: Manages",
                "shop -> mail: Sends"
            ]
        );
        let sends = m.find_relationship(m.find("api").unwrap(), m.find("mail").unwrap());
        assert_eq!(p.edges[2].relationships, sends);
    }

    #[test]
    fn core_5_declared_between_drawn_wins_over_lifted() {
        // The lifted relationship merges into the direct edge, even when
        // the direct one is declared later.
        let (m, p) = context_plan("        shop -> mail \"Direct\" \"SMTP\"");
        assert_indexed(&p);
        let (shop, mail, api) = (
            m.find("shop").unwrap(),
            m.find("mail").unwrap(),
            m.find("api").unwrap(),
        );
        let direct = m.find_relationship(shop, mail)[0];
        let lifted = m.find_relationship(api, mail)[0];
        let on_pair: Vec<&PlanEdge> = p
            .edges
            .iter()
            .filter(|e| p.boxes[e.destination.0 as usize].element == mail)
            .collect();
        assert_eq!(on_pair.len(), 1);
        assert_eq!(on_pair[0].relationships, [direct, lifted]);
        assert_eq!(on_pair[0].winner(), direct);
        assert_eq!(on_pair[0].description.as_deref(), Some("Direct"));
        assert_eq!(on_pair[0].technology.as_deref(), Some("SMTP"));
    }

    #[test]
    fn core_5_first_declared_wins_among_lifted() {
        // Review Focus: its label is the first by source order.
        let (m, p) = context_plan("        auth -> mail \"Notifies\"");
        let mail = m.find("mail").unwrap();
        let sends = m.find_relationship(m.find("api").unwrap(), mail)[0];
        let notifies = m.find_relationship(m.find("auth").unwrap(), mail)[0];
        let edge = p
            .edges
            .iter()
            .find(|e| p.boxes[e.destination.0 as usize].element == mail)
            .unwrap();
        assert_eq!(edge.winner(), sends);
        assert_eq!(edge.relationships, [sends, notifies]);
        assert_eq!(edge.description.as_deref(), Some("Sends"));
    }

    #[test]
    fn core_5_duplicate_lifted_lines_drawn_once() {
        let (m, p) = context_plan(
            "        user -> api \"Calls\"\n        auth -> mail \"Notifies\"\n        user -> db \"Queries\"",
        );
        assert_indexed(&p);
        assert_eq!(
            edge_lines(&m, &p),
            [
                "user -> shop: Uses",
                "admin -> shop: Manages",
                "shop -> mail: Sends"
            ]
        );
        assert_eq!(p.edges[0].relationships.len(), 3);
        assert_eq!(p.edges[2].relationships.len(), 2);
    }

    #[test]
    fn core_5_two_direct_relationships_same_pair() {
        // Two direct `a -> b` are two edges; not merged.
        let (m, p) =
            context_plan("        user -> shop \"Browses\"\n        user -> shop \"Pays\"");
        assert_indexed(&p);
        assert_eq!(
            edge_lines(&m, &p),
            [
                "admin -> shop: Manages",
                "shop -> mail: Sends",
                "user -> shop: Browses",
                "user -> shop: Pays",
            ]
        );
        // The lifted `user -> web` merges into the first direct edge.
        let user = m.find("user").unwrap();
        let shop = m.find("shop").unwrap();
        let direct = m.find_relationship(user, shop);
        let lifted = m.find_relationship(user, m.find("web").unwrap())[0];
        assert_eq!(p.edges[2].relationships, [direct[0], lifted]);
        assert_eq!(p.edges[3].relationships, [direct[1]]);
    }

    #[test]
    fn core_5_internal_lift_dropped() {
        // Review Focus: ends lifting to the same box, no self-loop.
        // `api -> db` and `job -> api` both lift to `shop -> shop`.
        let (m, p) = context_plan("");
        assert!(p.edges.iter().all(|e| e.source != e.destination));
        let reads = m.find_relationship(m.find("api").unwrap(), m.find("db").unwrap())[0];
        let calls = m.find_relationship(m.find("job").unwrap(), m.find("api").unwrap())[0];
        assert!(
            p.edges
                .iter()
                .all(|e| !e.relationships.contains(&reads) && !e.relationships.contains(&calls))
        );
        assert_eq!(p.edges.len(), 3);
    }

    #[test]
    fn core_5_no_drawn_ancestor_dropped() {
        // E.g. a relationship to the scope system on a container view.
        let m = model(&workspace(
            SHOP,
            "        container shop \"c2\" {\n            include *\n            include admin\n        }",
        ));
        let p = plan_of(&m, "c2");
        assert_indexed(&p);
        assert_eq!(p.boxes_of(m.find("admin").unwrap()).len(), 1);
        assert!(edge_lines(&m, &p).iter().all(|l| !l.contains("admin")));
        assert_eq!(
            edge_lines(&m, &p),
            [
                "user -> web: Uses",
                "web -> db: Reads",
                "web -> mail: Sends",
                "job -> web: Calls"
            ]
        );
    }

    #[test]
    fn core_5_element_and_ancestor_both_drawn() {
        // `web` and its system `shop` are both drawn: `web` is the
        // nearest drawn ancestor of `api`, and `user -> web` stays direct.
        let m = model(&workspace(
            SHOP,
            "        systemContext shop \"n\" {\n            include shop web user mail\n        }",
        ));
        let p = plan_of(&m, "n");
        assert_indexed(&p);
        assert_eq!(box_keys(&m, &p), ["user", "mail", "shop", "web"]);
        assert_eq!(
            edge_lines(&m, &p),
            ["user -> web: Uses", "web -> mail: Sends"]
        );
    }

    #[test]
    fn core_5_lifted_child_to_ancestor_dropped() {
        // `api -> db` lifts to `web -> shop`: `web` is a child of `shop`,
        // and Structurizr implies no parent-child relationship.
        let m = model(&workspace(
            SHOP,
            "        systemContext shop \"n\" {\n            include shop web\n        }",
        ));
        let p = plan_of(&m, "n");
        assert_indexed(&p);
        assert_eq!(box_keys(&m, &p), ["shop", "web"]);
        let reads = m.find_relationship(m.find("api").unwrap(), m.find("db").unwrap())[0];
        assert!(p.edges.iter().all(|e| !e.relationships.contains(&reads)));
        assert!(edge_lines(&m, &p).iter().all(|l| l != "web -> shop: Reads"));
    }

    #[test]
    fn core_5_lifted_ancestor_to_child_dropped() {
        // `job -> api` lifts to `shop -> web`: ancestor to child, dropped.
        let m = model(&workspace(
            SHOP,
            "        systemContext shop \"n\" {\n            include shop web\n        }",
        ));
        let p = plan_of(&m, "n");
        assert_indexed(&p);
        let calls = m.find_relationship(m.find("job").unwrap(), m.find("api").unwrap())[0];
        assert!(p.edges.iter().all(|e| !e.relationships.contains(&calls)));
        assert!(edge_lines(&m, &p).iter().all(|l| l != "shop -> web: Calls"));
    }

    #[test]
    fn core_5_direct_parent_child_relationship_drawn() {
        // Only lifted lines are dropped: a relationship written between
        // a parent and its child is drawn, in either direction.
        let body =
            format!("{SHOP}\n        web -> shop \"Reports\"\n        shop -> web \"Deploys\"");
        let m = model(&workspace(
            &body,
            "        systemContext shop \"n\" {\n            include shop web\n        }",
        ));
        let p = plan_of(&m, "n");
        assert_indexed(&p);
        assert_eq!(
            edge_lines(&m, &p),
            ["web -> shop: Reports", "shop -> web: Deploys"]
        );
    }

    // view-01: include and exclude per view kind (DSL-6). The box set
    // comes from `Model::included`; view-01 implements nothing in
    // view.rs for these, they check that `plan` draws exactly that set.
    #[test]
    fn include_star_context_view() {
        // C1: the scope system and the people and systems linked to it;
        // relationships of nested elements count for their system.
        let m = model(&workspace(
            SHOP,
            "        systemContext shop \"c1\" {\n            include *\n        }",
        ));
        let p = plan_of(&m, "c1");
        assert_indexed(&p);
        assert_eq!(box_keys(&m, &p), ["user", "admin", "mail", "shop"]);
        assert_eq!(p.boxes.len(), m.included(m.view("c1").unwrap()).len());
        assert_eq!(p.kind, ViewKind::SystemContext);
        assert_eq!(p.key, "c1");
    }

    #[test]
    fn include_star_container_view() {
        // C2: the scope's containers and the people and systems linked to
        // them; `admin -> shop` names the scope, not a container.
        let m = model(&workspace(
            SHOP,
            "        container shop \"c2\" {\n            include *\n        }",
        ));
        let p = plan_of(&m, "c2");
        assert_indexed(&p);
        assert_eq!(box_keys(&m, &p), ["user", "mail", "web", "db", "job"]);
        assert_eq!(edge_lines(&m, &p)[0], "user -> web: Uses");
    }

    #[test]
    fn include_star_component_view() {
        // Review Focus: external containers and people linked to the
        // scope's components come in.
        let body = format!("{SHOP}\n        user -> auth \"Signs in\"");
        let m = model(&workspace(
            &body,
            "        component web \"c3\" {\n            include *\n        }",
        ));
        let p = plan_of(&m, "c3");
        assert_indexed(&p);
        assert_eq!(
            box_keys(&m, &p),
            ["user", "mail", "api", "auth", "db", "job"]
        );
        assert!(edge_lines(&m, &p).contains(&"user -> auth: Signs in".to_owned()));
        assert!(edge_lines(&m, &p).contains(&"api -> db: Reads".to_owned()));
    }

    #[test]
    fn exclude_removes_box_and_edges() {
        let m = model(&workspace(
            SHOP,
            "        container shop \"c2\" {\n            include *\n            exclude user\n        }",
        ));
        let p = plan_of(&m, "c2");
        assert_indexed(&p);
        assert_eq!(box_keys(&m, &p), ["mail", "web", "db", "job"]);
        let user = m.find("user").unwrap();
        assert!(p.boxes_of(user).is_empty());
        assert!(edge_lines(&m, &p).iter().all(|l| !l.contains("user")));
        let r0 = m.find_relationship(user, m.find("web").unwrap())[0];
        assert!(p.edges.iter().all(|e| !e.relationships.contains(&r0)));
    }

    #[test]
    fn exclude_of_not_included_is_no_error() {
        // Review Focus.
        let m = model(&workspace(
            SHOP,
            "        container shop \"a\" {\n            include *\n            exclude other\n        }\n        container shop \"b\" {\n            include *\n        }",
        ));
        let (a, b) = (plan_of(&m, "a"), plan_of(&m, "b"));
        assert_eq!(a.boxes, b.boxes);
        assert_eq!(a.edges, b.edges);
    }

    // view-03: focus, hide, sizes, positions (DSL-6, DSL-7, DSL-8).
    #[test]
    fn focus_keeps_only_touching_lines() {
        // Container edges without focus: user -> web, web -> db,
        // web -> mail, job -> web. Boxes are not filtered.
        let (m, p) = container_plan("", "            focus db");
        assert_indexed(&p);
        assert_eq!(edge_lines(&m, &p), ["web -> db: Reads"]);
        assert_eq!(p.boxes.len(), 5);
        let (m, p) = container_plan("", "            focus db\n            focus job");
        assert_indexed(&p);
        assert_eq!(
            edge_lines(&m, &p),
            ["web -> db: Reads", "job -> web: Calls"]
        );
    }

    #[test]
    fn hide_drops_pairs() {
        // One direction only; every edge on the pair goes.
        let (m, p) = container_plan(
            "        web -> mail \"Bounces\"",
            "            hide web -> mail\n            hide db -> web",
        );
        assert_indexed(&p);
        assert_eq!(
            edge_lines(&m, &p),
            ["user -> web: Uses", "web -> db: Reads", "job -> web: Calls"]
        );
        assert_eq!(p.boxes.len(), 5);
    }

    #[test]
    fn dsl_7_focus_hide_lift_to_ancestor() {
        // A `focus` or `hide` id with no box of its own acts on its
        // nearest drawn ancestor; with no drawn ancestor it is ignored.
        let extra = "        user -> db \"Queries\"";
        let all = [
            "user -> web: Uses",
            "web -> db: Reads",
            "web -> mail: Sends",
            "job -> web: Calls",
            "user -> db: Queries",
        ];
        let (m, p) = container_plan(extra, "");
        assert_eq!(edge_lines(&m, &p), all);
        // `api` is drawn as `web`.
        let (m, p) = container_plan(extra, "            focus api");
        assert_eq!(edge_lines(&m, &p), all[..4]);
        let (m, p) = container_plan(extra, "            hide api -> db");
        assert_eq!(
            edge_lines(&m, &p),
            [
                "user -> web: Uses",
                "web -> mail: Sends",
                "job -> web: Calls",
                "user -> db: Queries"
            ]
        );
        // `admin` has no box and no drawn ancestor: ignored.
        let (m, p) = container_plan(extra, "            focus admin");
        assert_eq!(edge_lines(&m, &p), all);
        let (m, p) = container_plan(extra, "            focus admin db");
        assert_eq!(
            edge_lines(&m, &p),
            ["web -> db: Reads", "user -> db: Queries"]
        );
        let (m, p) = container_plan(
            extra,
            "            hide admin -> web\n            hide user -> shop",
        );
        assert_eq!(edge_lines(&m, &p), all);
    }

    #[test]
    fn core_2_default_sizes_by_kind() {
        let (m, p) = context_plan("");
        let half = |b: &PlanBox| (b.half_width, b.half_height);
        // 200x180, 240x140 px.
        assert_eq!(half(box_named(&m, &p, "user")), (10, 9));
        assert_eq!(half(box_named(&m, &p, "shop")), (12, 7));
        let m = model(&workspace(
            SHOP,
            "        component web \"c3\" {\n            include *\n        }",
        ));
        let p = plan_of(&m, "c3");
        // 200x120, 240x140 px.
        assert_eq!(half(box_named(&m, &p, "api")), (10, 6));
        assert_eq!(half(box_named(&m, &p, "db")), (12, 7));
        assert_eq!(half(box_named(&m, &p, "mail")), (12, 7));
    }

    #[test]
    fn dsl_8_unplaced_boxes_marked() {
        let (m, p) = container_plan("", "            at web 100 200");
        let web = box_named(&m, &p, "web").id;
        let expected: Vec<BoxId> = p.boxes.iter().map(|b| b.id).filter(|&b| b != web).collect();
        assert_eq!(p.unplaced(), expected);
        assert!(
            p.unplaced()
                .iter()
                .all(|b| p.boxes[b.0 as usize].centre.is_none())
        );
        let from_model: Vec<ElementId> = p
            .unplaced()
            .iter()
            .map(|b| p.boxes[b.0 as usize].element)
            .collect();
        assert_eq!(from_model, m.unplaced(m.view("c2").unwrap()));
    }

    #[test]
    fn dsl_7_at_and_size_lines_in_grid_units() {
        let (m, p) = container_plan(
            "",
            "            at web 620 -480\n            size web 280 140\n            at db 623 485\n            size db 250 135",
        );
        let (web, db) = (box_named(&m, &p, "web"), box_named(&m, &p, "db"));
        assert_eq!(web.centre, Some(Point::new(62, -48)));
        assert_eq!((web.half_width, web.half_height), (14, 7));
        // Off-grid values: centre snapped, half-sizes rounded up.
        assert_eq!(db.centre, Some(Point::new(62, 49)));
        assert_eq!((db.half_width, db.half_height), (13, 7));
        // No `size` line: the CORE-2 default.
        let job = box_named(&m, &p, "job");
        assert_eq!((job.half_width, job.half_height, job.centre), (12, 7, None));
    }

    // Plan contents the next stages read.
    #[test]
    fn plan_box_carries_kind_external_style_texts() {
        // CORE-2 data, plus effective tags for the CORE-1 key.
        let body = format!(
            "{SHOP}\n        store = softwareSystem \"Store\" {{\n            rows = container \"Rows\" \"Keeps rows.\" \"PostgreSQL\" \"Data\"\n        }}\n        api -> rows \"Writes\""
        );
        let views = "        component web \"c3\" {\n            include *\n            include rows\n        }\n        styles {\n            element \"External\" {\n                background #999999\n                shape Cylinder\n            }\n        }";
        let m = model(&workspace(&body, views));
        let p = plan_of(&m, "c3");
        let mail = box_named(&m, &p, "mail");
        assert_eq!(mail.kind, ElementKind::SoftwareSystem);
        assert!(mail.external);
        assert_eq!(mail.tags, ["Element", "Software System", "External"]);
        assert_eq!(mail.style, m.style_of(mail.element));
        assert_eq!(mail.style.shape, crate::model::Shape::Cylinder);
        assert_eq!(mail.name, "Mail");
        assert_eq!(mail.description.as_deref(), Some("Sends mail."));
        assert_eq!(mail.technology, None);
        let rows = box_named(&m, &p, "rows");
        assert_eq!(rows.kind, ElementKind::Container);
        assert!(!rows.external);
        assert_eq!(rows.tags, ["Element", "Container", "Data"]);
        assert_eq!(
            rows.style,
            ResolvedStyle::default_for(ElementKind::Container)
        );
        assert_eq!(rows.description.as_deref(), Some("Keeps rows."));
        assert_eq!(rows.technology.as_deref(), Some("PostgreSQL"));
        let user_box = context_plan("").1;
        assert_eq!(user_box.boxes[0].style.shape, crate::model::Shape::Person);
    }

    #[test]
    fn boxes_of_and_rect() {
        // `boxes_of`, `rect` (at line) and `rect_at` (placed centre).
        let (m, p) = container_plan("", "            at web 100 200");
        let web = box_named(&m, &p, "web");
        assert_eq!(p.boxes_of(web.element), [web.id]);
        assert!(p.boxes_of(m.find("other").unwrap()).is_empty());
        assert_eq!(
            web.rect(),
            Some(Rect {
                min: Point::new(-2, 13),
                max: Point::new(22, 27)
            })
        );
        let db = box_named(&m, &p, "db");
        assert_eq!(db.rect(), None);
        assert_eq!(
            db.rect_at(Point::new(0, 0)),
            Rect {
                min: Point::new(-12, -7),
                max: Point::new(12, 7)
            }
        );
        assert_eq!(web.rect_at(Point::new(10, 20)), web.rect().unwrap());
    }

    #[test]
    fn edge_winner_and_shown_technology() {
        // CORE-4: `technology off` hides it, the plan keeps it.
        let body = format!("{SHOP}\n        user -> shop \"Browses\" \"HTTPS\"");
        let m = model(&workspace(
            &body,
            "        systemContext shop \"on\" {\n            include *\n        }\n        systemContext shop \"off\" {\n            include *\n            technology off\n        }",
        ));
        let user = m.find("user").unwrap();
        let browses = m.find_relationship(user, m.find("shop").unwrap())[0];
        for key in ["on", "off"] {
            let p = plan_of(&m, key);
            let edge = p
                .edges
                .iter()
                .find(|e| e.relationships.contains(&browses))
                .unwrap();
            assert_eq!(edge.winner(), browses);
            assert_eq!(edge.winner(), edge.relationships[0]);
            assert_eq!(edge.technology.as_deref(), Some("HTTPS"));
            let shown = edge.shown_technology(&p.options);
            assert_eq!(shown, if key == "on" { Some("HTTPS") } else { None });
        }
        let p = plan_of(&m, "on");
        assert_eq!(p.edges[0].shown_technology(&p.options), None);
    }

    #[test]
    fn opposite_directions_are_two_edges() {
        let (m, p) =
            context_plan("        mail -> api \"Bounces\"\n        mail -> shop \"Replies\"");
        assert_indexed(&p);
        assert_eq!(
            edge_lines(&m, &p),
            [
                "user -> shop: Uses",
                "admin -> shop: Manages",
                "shop -> mail: Sends",
                "mail -> shop: Replies",
            ]
        );
        assert_eq!(p.edges[3].relationships.len(), 2);
    }

    #[test]
    fn core_1_boundary_holds_drawn_scope_children() {
        // Container and component views only; none on a context view or
        // when no child is drawn.
        let (m, p) = container_plan("", "            exclude db");
        assert_eq!(p.boundaries.len(), 1);
        let b = &p.boundaries[0];
        assert_eq!(b.of, BoundaryOf::Element(m.find("shop").unwrap()));
        assert_eq!(b.name, "Shop");
        let members: Vec<String> = b
            .members
            .iter()
            .map(|&id| key(&m, p.boxes[id.0 as usize].element))
            .collect();
        assert_eq!(members, ["web", "job"]);
        let m = model(&workspace(
            SHOP,
            "        component web \"c3\" {\n            include *\n        }",
        ));
        let p = plan_of(&m, "c3");
        assert_eq!(p.boundaries.len(), 1);
        assert_eq!(p.boundaries[0].name, "Web");
        let members: Vec<String> = p.boundaries[0]
            .members
            .iter()
            .map(|&id| key(&m, p.boxes[id.0 as usize].element))
            .collect();
        assert_eq!(members, ["api", "auth"]);
        assert!(context_plan("").1.boundaries.is_empty());
        let (_, p) = container_plan("", "            exclude web db job");
        assert!(p.boundaries.is_empty());
    }

    #[test]
    fn core_1_boundary_rect_inflates_member_bounds() {
        let (m, p) = container_plan("", "            size db 200 120");
        let mut centres = vec![Point::new(100, 100); p.boxes.len()];
        centres[box_named(&m, &p, "web").id.0 as usize] = Point::new(0, 0);
        centres[box_named(&m, &p, "db").id.0 as usize] = Point::new(40, 0);
        centres[box_named(&m, &p, "job").id.0 as usize] = Point::new(0, 30);
        // Members: web (-12,-7)-(12,7), db (30,-6)-(50,6), job
        // (-12,23)-(12,37); union (-12,-7)-(50,37), plus 4 units.
        assert_eq!(
            p.boundary_rect(0, &centres),
            Some(Rect {
                min: Point::new(-16, -11),
                max: Point::new(54, 41)
            })
        );
        assert_eq!(p.boundary_rect(1, &centres), None);
    }

    #[test]
    fn dsl_6_title_line_else_description() {
        // Also subtitle and options with defaults.
        let m = model(&workspace(
            SHOP,
            "        container shop \"a\" \"Described\" {\n            include *\n            title \"Titled\"\n            subtitle \"Below\"\n            technology off\n            labelWidth 16\n        }\n        container shop \"b\" \"Described\" {\n            include *\n        }\n        container shop \"c\" {\n            include *\n        }",
        ));
        let a = plan_of(&m, "a");
        assert_eq!(a.title.as_deref(), Some("Titled"));
        assert_eq!(a.subtitle.as_deref(), Some("Below"));
        assert_eq!(a.options, m.view("a").unwrap().options());
        assert!(!a.options.technology);
        assert_eq!(a.options.label_width, 16);
        let b = plan_of(&m, "b");
        assert_eq!(b.title.as_deref(), Some("Described"));
        assert_eq!(b.subtitle, None);
        assert_eq!(b.options, ViewOptions::DEFAULT);
        let c = plan_of(&m, "c");
        assert_eq!(c.title, None);
        assert_eq!(c.scope, m.view("c").unwrap().scope);
    }

    #[test]
    fn core_25_plan_is_deterministic() {
        // Boxes by element id, edges by winning relationship id, whatever
        // the include order.
        let body =
            format!("{SHOP}\n        user -> api \"Calls\"\n        mail -> web \"Bounces\"");
        let m = model(&workspace(
            &body,
            "        container shop \"c2\" {\n            include job mail db user web\n        }",
        ));
        let p = plan_of(&m, "c2");
        assert_eq!(p, plan_of(&m, "c2"));
        assert_eq!(p, plan(&m.clone(), m.view("c2").unwrap()));
        assert!(p.boxes.windows(2).all(|w| w[0].element < w[1].element));
        assert!(p.edges.windows(2).all(|w| w[0].winner() < w[1].winner()));
        assert_eq!(box_keys(&m, &p), ["user", "mail", "web", "db", "job"]);
    }
}
