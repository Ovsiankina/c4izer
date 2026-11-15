//! Checks a scene against the model: angles, crossings against the
//! budget, box grid and spacing, C4 level rules, intent labels, coverage
//! across views, and the split hint (CORE-16..21, 26); it also reports
//! what routing, labelling and text fitting could not do: unroutable
//! lines (CORE-13, L1), labels over lines or with no room (CORE-15, L23,
//! L24) and text that does not fit (CORE-30, L25).
//!
//! Depends on: `scene`, `model`, `text` (for `Span`).

// Shape of the API: `lint(&Model, &Scene) -> Report` runs every per-view
// rule of the SRS 4.2 lint table (L1..L21, L23..L26) over one built scene;
// `coverage(&Model, &[&Scene]) -> Vec<Finding>` runs L22, the only rule
// that looks across views, and is reported outside any view (CLI-1 prints
// it after all views). Pipeline calls `lint` per view and `coverage` once
// over every scene it built; the CLI prints `Report::stats` and the
// findings; the editor lists the findings beside the view (EDIT-6) and
// uses `Finding::items` to mark them on the canvas and `Finding::spans`
// to jump to the source.
//
// DECISION: lint reads geometry only through the scene (HLD: lint
// depends on scene and model): `Scene::unroutable` (L1), line points (L2,
// L3), `Scene::crossings` and `Scene::crossing_pairs` (L4, CORE-26; same
// count as the router, so L4 and CORE-11 agree; lint never recounts),
// box rects (L6, L7, L26), `SceneLabel::fit` (L23, L24),
// `TextBlock::fits` (L25). Names, descriptions, technologies, kinds,
// parents and spans come from the model by the ids the scene items carry.
// DECISION: L5 does not read the scene: `view` snaps every centre and
// size to the grid, so `SceneBox::rect` and `Scene::centres` are always on
// it. L5 reads the lines as written, `model.view(&scene.key)` ->
// `ViewSpec::positions` (`At::x`, `At::y`) and `ViewSpec::sizes`
// (`Size::width`, `Size::height`), with `At::span` / `Size::span` as the
// finding's span. A view not in `Model::views` (EDIT-15 default views)
// has no `at`/`size` lines and so no L5.
// DECISION: L12 reads the model, not `Scene::labels`: a line has no label
// when its winning relationship's description is empty or absent. A line
// joining a trunk with the same label (CORE-14) and an unroutable line
// draw no `SceneLabel` and are not L12 for that.
// DECISION: `coverage` takes the scenes, not the view plans: which
// relationships a view draws is `SceneLine::relationships` after lifting
// (CORE-5), and lint does not depend on `view`. Callers pass every
// component view's scene; a container with a component view of
// `Model::views` that has no scene in the slice is skipped (no L22 for
// it), so a partial slice (the editor routing views in the background,
// EDIT-14, CORE-28) gives no false errors.
// DECISION: `lint` expects a scene from `scene::build`. After
// `Scene::move_box` (drag preview) boxes, lines and `crossings` disagree,
// so its report is not meaningful.
// DECISION: no hot struct here (the SRS and HLD name none, a view has
// tens of findings), so no `#[repr]` and no size pin.
// DECISION: the rule is an enum named after what it checks, with its
// table id from `Rule::code` and its level from `Rule::severity`, so the
// level lives in one place (CORE-16: "at the level the table gives").
// DECISION: findings of one report come in rule order (L1 first), then in
// the order of the first item involved (box, then edge order); same
// scene and model give an equal report (CORE-25).
//
// CORE-16: each finding's text is the prototype `c4.py` `lint` message
// for that rule (~/Projects/c4-design/scripts/c4.py), naming elements by
// quoted name. L4 ends with the CORE-26 split hint: `<c> line crossings,
// budget is <b>: move boxes or raise maxCrossings on purpose; split the
// view (focus, hide); boxes involved: "<name>", ...`. L22 reads as CLI-1 (`<src> -> <dst> is in the model but on no
// component view of "<scope>"`), with `<src>`, `<dst>` and `<scope>`
// element names; CORE-21: `<scope>` is the source's container if the
// relationship fails there, else the destination's.
// L18: a word is a maximal run of ASCII letters and digits, and matching
// is case-sensitive, so `HTTP/2` and `gRPC-web` match and `HTTPServer`
// and `https` do not.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::geom::Segment;
use crate::label::Fit;
use crate::model::{ElementId, ElementKind, Model, RelationshipId, ViewKind};
use crate::scene::{ItemKey, ModelId, Scene, SceneBox, SceneLine};
use crate::text::Span;
use crate::view::{BoundaryOf, BoxId, EdgeId};

/// The level of a finding. Errors fail the run (CLI-2), warnings do not.
/// Ordered `Warning < Error`. Serves CORE-16, CLI-1, CLI-2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// Printed as `warning: <text>` (CLI-1); does not change the exit code.
    Warning,
    /// Printed as `ERROR: <text>` (CLI-1); exit code 1 (CLI-2).
    Error,
}

/// One rule of the SRS 4.2 lint table; `code` gives its table id and
/// `severity` its level. `#[non_exhaustive]`: later phases add rules.
/// Serves CORE-16..21, CORE-26, CORE-13, CORE-15, CORE-30.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Rule {
    /// L1, error: a line could not be routed (CORE-13).
    Unroutable,
    /// L2, error: a segment is not at 0°, 45° or 90° (CORE-6).
    SegmentAngle,
    /// L3, error: a segment passes through the inside of a box other
    /// than its own two ends (CORE-7).
    ThroughBox,
    /// L4, error: crossings above `maxCrossings`; carries the CORE-26
    /// split hint (CORE-17, CORE-26).
    Crossings,
    /// L5, error: a box centre, half-width or half-height is not a
    /// multiple of 10 (CORE-18), read from the `at` and `size` lines as
    /// written, not from the snapped scene.
    OffGrid,
    /// L6, error: two boxes, each grown by 30px on every side, overlap
    /// (CORE-18).
    TooClose,
    /// L7, warning: more than 3 boxes, and distinct centre rows plus
    /// distinct centre columns exceed boxes + 1.
    Alignment,
    /// L8, error: the view has no title.
    NoTitle,
    /// L9, error: a box has an empty name or an empty description
    /// (CORE-20).
    NameOrDescription,
    /// L10, error: a container or component has no technology (CORE-20).
    NoTechnology,
    /// L11, warning: a box description is over 110 characters (CORE-20),
    /// counted as `char`s.
    // DECISION: chars, not bytes; the SRS does not say, and a byte count
    // would flag short non-ASCII descriptions.
    LongDescription,
    /// L12, error: a line has no label (CORE-20): the winning
    /// relationship's description is empty or absent in the model. Not
    /// the absence of a `SceneLabel`: a line joining a trunk with the same
    /// label (CORE-14) and an unroutable line draw none.
    NoLabel,
    /// L13, error: a label is only a stop word (`uses`, `use`, `calls`,
    /// `sends`, `reads`, `writes`) (CORE-20).
    VagueLabel,
    /// L14, warning: both A -> B and B -> A are drawn, once per pair.
    TwoWay,
    /// L15, error: a C1 line shows a technology (CORE-19).
    C1Technology,
    /// L16, error: a C1 box is not a person or a software system
    /// (CORE-19).
    C1BoxKind,
    /// L17, warning: a C1 view has more than 8 boxes (CORE-19).
    C1TooManyBoxes,
    /// L18, error: a C1 box name or line label holds a protocol word
    /// (CORE-19).
    C1Protocol,
    /// L19, error: a C2 view shows a component (CORE-19).
    C2Component,
    /// L20, warning: a C2 line has a container at one end, no person at
    /// either end, and no technology (CORE-19).
    C2NoTechnology,
    /// L21, error: a C3 view shows no component whose parent is the
    /// scope (CORE-19).
    C3NoScopeComponent,
    /// L22, error: a relationship of a component is on none of its
    /// container's component views; outside any view (CORE-21).
    Coverage,
    /// L23, warning: a label covers another line (CORE-15).
    LabelOverLine,
    /// L24, warning: a label has no room (CORE-15).
    LabelNoRoom,
    /// L25, warning: a text does not fit its box or label rectangle
    /// (CORE-30).
    TextFit,
    /// L26, error: a box that is not a child of the scope overlaps the
    /// boundary rectangle.
    BoundaryIntrusion,
}

impl Rule {
    /// The table id, `"L1"` to `"L26"`. Serves CORE-16.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Rule::Unroutable => "L1",
            Rule::SegmentAngle => "L2",
            Rule::ThroughBox => "L3",
            Rule::Crossings => "L4",
            Rule::OffGrid => "L5",
            Rule::TooClose => "L6",
            Rule::Alignment => "L7",
            Rule::NoTitle => "L8",
            Rule::NameOrDescription => "L9",
            Rule::NoTechnology => "L10",
            Rule::LongDescription => "L11",
            Rule::NoLabel => "L12",
            Rule::VagueLabel => "L13",
            Rule::TwoWay => "L14",
            Rule::C1Technology => "L15",
            Rule::C1BoxKind => "L16",
            Rule::C1TooManyBoxes => "L17",
            Rule::C1Protocol => "L18",
            Rule::C2Component => "L19",
            Rule::C2NoTechnology => "L20",
            Rule::C3NoScopeComponent => "L21",
            Rule::Coverage => "L22",
            Rule::LabelOverLine => "L23",
            Rule::LabelNoRoom => "L24",
            Rule::TextFit => "L25",
            Rule::BoundaryIntrusion => "L26",
        }
    }

    /// The level the lint table gives this rule. Serves CORE-16.
    #[must_use]
    pub const fn severity(self) -> Severity {
        match self {
            Rule::Alignment
            | Rule::LongDescription
            | Rule::TwoWay
            | Rule::C1TooManyBoxes
            | Rule::C2NoTechnology
            | Rule::LabelOverLine
            | Rule::LabelNoRoom
            | Rule::TextFit => Severity::Warning,
            Rule::Unroutable
            | Rule::SegmentAngle
            | Rule::ThroughBox
            | Rule::Crossings
            | Rule::OffGrid
            | Rule::TooClose
            | Rule::NoTitle
            | Rule::NameOrDescription
            | Rule::NoTechnology
            | Rule::NoLabel
            | Rule::VagueLabel
            | Rule::C1Technology
            | Rule::C1BoxKind
            | Rule::C1Protocol
            | Rule::C2Component
            | Rule::C3NoScopeComponent
            | Rule::Coverage
            | Rule::BoundaryIntrusion => Severity::Error,
        }
    }
}

/// The CORE-26 suggestion carried by an L4 finding: split the view with
/// `focus` or `hide`, naming what takes part in the crossings. Serves
/// CORE-26.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SplitHint {
    /// The elements of the boxes at either end of a crossing line, each
    /// once, in box order.
    pub boxes: Vec<ElementId>,
    /// The winning relationships of the crossing lines, each once, in
    /// edge order.
    pub relationships: Vec<RelationshipId>,
}

/// One broken rule. Serves CORE-16 ("with the ids involved, at the level
/// the table gives"), CORE-26, EDIT-6.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Finding {
    /// The rule broken; its level is `rule.severity()`.
    pub rule: Rule,
    /// The view's effective key (`Scene::key`); `None` for a finding
    /// outside any view (L22).
    pub view: Option<String>,
    /// The model ids involved, in the order the message names them.
    pub subjects: Vec<ModelId>,
    /// The scene items to mark on the canvas (EDIT-6), view-local to the
    /// scene linted. Empty for L22 and for the view-wide rules L7, L8
    /// and L17; L4 marks its crossing lines.
    pub items: Vec<ItemKey>,
    /// Source spans the editor can point at, most specific first: the
    /// `at`/`size` line for L5 (`At::span`, `Size::span`), the element or relationship
    /// declaration, the view declaration for view-wide rules. Empty for a
    /// view not written in the file (EDIT-15 default views).
    pub spans: Vec<Span>,
    /// The text the CLI prints after `warning: ` or `ERROR: ` (CLI-1).
    pub message: String,
    /// The CORE-26 split hint, on an L4 finding only.
    pub hint: Option<SplitHint>,
}

impl Finding {
    /// `rule.severity()`. Serves CORE-16, CLI-2.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        self.rule.severity()
    }
}

impl fmt::Display for Finding {
    /// Writes `message` only; the CLI adds the `warning: ` or `ERROR: `
    /// prefix (CLI-1). Serves CLI-1, EDIT-6.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The counts the CLI prints per view (CLI-1): `<n> boxes, <m> lines,
/// <c> crossings (budget <b>)` and `alignment: <r> rows, <k> columns for
/// <n> boxes`. Serves CLI-1, CORE-17, L7.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Stats {
    /// Boxes drawn.
    pub boxes: u32,
    /// Lines drawn, unroutable ones included.
    pub lines: u32,
    /// `Scene::crossings`.
    pub crossings: u32,
    /// The view's `maxCrossings` (`Scene::options`).
    pub budget: u32,
    /// Distinct box centre rows (L7).
    pub rows: u32,
    /// Distinct box centre columns (L7).
    pub columns: u32,
}

/// The lint result of one view: its counts and its findings, errors and
/// warnings mixed in rule order. Serves CORE-16, CLI-1, CLI-2, EDIT-6.
///
/// Built only by `lint` (`#[non_exhaustive]`, as `Finding` and `Stats`):
/// tests outside core get a report through the pipeline.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Report {
    /// The view's effective key (`Scene::key`).
    pub view: String,
    /// The counts for the CLI header lines.
    pub stats: Stats,
    /// Every broken per-view rule, in rule order then item order.
    pub findings: Vec<Finding>,
}

impl Report {
    /// True when any finding is an error (CLI-2 exit code 1). Serves
    /// CORE-16, CLI-2.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity() == Severity::Error)
    }

    /// The warnings, in report order (CLI-1 prints them first). Serves
    /// CLI-1.
    // DECISION: a `Vec`, not `impl Iterator`: a `todo!()` stub cannot
    // return an opaque iterator, and `Model::children` makes the same
    // choice; a view has tens of findings.
    #[must_use]
    pub fn warnings(&self) -> Vec<&Finding> {
        self.of_level(Severity::Warning)
    }

    /// The errors, in report order (CLI-1 prints them after the
    /// warnings). Serves CLI-1, CLI-2.
    #[must_use]
    pub fn errors(&self) -> Vec<&Finding> {
        self.of_level(Severity::Error)
    }

    /// The findings at `level`, in report order.
    fn of_level(&self, level: Severity) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity() == level)
            .collect()
    }
}

/// Lints one built view: every rule of the lint table except L22, over
/// what `scene` draws (after lifting) and the model behind its ids.
/// Geometry rules (L2, L3) skip lines with `routed: false`, which are
/// L1 only. L4 and `Stats::crossings` use `Scene::crossings` and
/// `Scene::crossing_pairs` as given and never recount; L6, L7 and L26 use
/// the box rects. L5 reads the view's `at` and `size` lines as written
/// (`model.view(&scene.key)` -> `ViewSpec::positions` / `sizes`, spans
/// `At::span` / `Size::span`), since the scene is snapped to the grid; a
/// view not in `Model::views` (EDIT-15 default views) has no L5. L12 reads
/// the winning relationship's description in the model, not
/// `Scene::labels`. An L4 finding carries the split hint (CORE-26).
/// Deterministic (CORE-25). Serves CORE-16..20, CORE-26, CORE-13,
/// CORE-15, CORE-30.
///
/// Expects a scene from `scene::build`, not one changed by
/// `Scene::move_box` (a drag preview), whose boxes, lines and crossings
/// disagree.
///
/// # Panics
///
/// When `scene` was not built from `model` (an element or relationship
/// id it carries is not in `model`).
#[must_use]
pub fn lint(model: &Model, scene: &Scene) -> Report {
    let mut cx = Lint::new(model, scene);
    // Rule order (L1 first); each rule pushes in box, then edge order.
    cx.unroutable();
    cx.segment_angles();
    cx.through_boxes();
    cx.crossings();
    cx.off_grid();
    cx.too_close();
    cx.alignment();
    cx.no_title();
    cx.box_texts();
    cx.line_labels();
    cx.two_way();
    cx.c1();
    cx.c2();
    cx.c3();
    cx.label_fit();
    cx.text_fit();
    cx.boundary_intrusion();
    let mut findings = cx.findings;
    // DECISION: a stable sort by rule after collecting, so the rule order
    // holds even where one pass emits two rules (L9..L11, L12..L13).
    findings.sort_by_key(|f| f.rule);
    Report {
        view: scene.key.clone(),
        stats: stats(scene),
        findings,
    }
}

/// L22 (CORE-21): for each container with at least one component view
/// in `model.views`, every relationship with one of its components at
/// either end must be drawn (`SceneLine::relationships`) on at least one
/// of that container's component views among `scenes`; each one that is
/// not gives one error, once per relationship, with `view: None`. Scenes
/// of other kinds, or of other scopes, are ignored for that container.
/// Ordered by relationship id. Serves CORE-21, CLI-1.
///
/// Callers pass the scene of every component view in `model.views`. A
/// container with a component view that has no scene in `scenes` is
/// skipped, so a partial slice gives no false L22 (it gives no L22 for
/// that container at all).
///
/// The `"<scope>"` of the CLI-1 message is one container: the source's
/// container (when the source is a component of a checked container) if
/// the relationship fails there, else the destination's.
/// A relationship that fails both containers is still one finding, named
/// with the source's container (CORE-21).
///
/// # Panics
///
/// When a scene was not built from `model`.
#[must_use]
pub fn coverage(model: &Model, scenes: &[&Scene]) -> Vec<Finding> {
    // Containers with a component view, each with every one of those
    // views among `scenes`; the value is what those views draw.
    let mut checked: BTreeMap<ElementId, BTreeSet<RelationshipId>> = BTreeMap::new();
    let mut skipped: BTreeSet<ElementId> = BTreeSet::new();
    for view in model.views.iter().filter(|v| v.kind == ViewKind::Component) {
        let Some(container) = view.scope.element() else {
            continue;
        };
        let present = scenes
            .iter()
            .any(|s| s.kind == ViewKind::Component && s.key == view.key);
        if present {
            checked.entry(container).or_default();
        } else {
            skipped.insert(container);
        }
    }
    for container in &skipped {
        checked.remove(container);
    }
    // DECISION: a scene counts for a container by its kind and scope,
    // not its key, so a component view the editor built for the same
    // container (EDIT-15) covers too.
    for scene in scenes.iter().filter(|s| s.kind == ViewKind::Component) {
        let Some(drawn) = scene
            .scope
            .element()
            .and_then(|c| checked.get_mut(&c))
        else {
            continue;
        };
        for line in &scene.lines {
            drawn.extend(line.relationships.iter().copied());
        }
    }
    // The checked container of a component end, if any.
    let container_of = |e: ElementId| -> Option<ElementId> {
        let el = model.element(e);
        if el.kind != ElementKind::Component {
            return None;
        }
        el.parent.filter(|p| checked.contains_key(p))
    };
    let fails = |c: Option<ElementId>, r: RelationshipId| -> Option<ElementId> {
        c.filter(|c| checked.get(c).is_some_and(|d| !d.contains(&r)))
    };
    let mut out = Vec::new();
    for r in &model.relationships {
        let (src, dst) = (r.source.value, r.destination.value);
        let Some(scope) =
            fails(container_of(src), r.id).or_else(|| fails(container_of(dst), r.id))
        else {
            continue;
        };
        let name = |e: ElementId| model.element(e).name.value.as_str();
        out.push(Finding {
            rule: Rule::Coverage,
            view: None,
            // DECISION: the relationship, then the container named as
            // `<scope>`; the message's `<src>` and `<dst>` are the
            // relationship's ends.
            subjects: vec![ModelId::Relationship(r.id), ModelId::Element(scope)],
            items: Vec::new(),
            spans: vec![r.span],
            message: format!(
                "{} -> {} is in the model but on no component view of \"{}\"",
                name(src),
                name(dst),
                name(scope)
            ),
            hint: None,
        });
    }
    out
}

/// L18: the protocol words, matched whole-word and case-sensitive.
const PROTOCOL_WORDS: [&str; 10] = [
    "SMTP", "HTTP", "HTTPS", "SQL", "JSON", "REST", "gRPC", "TCP", "API", "VAPID",
];

/// L13: labels that say nothing, compared trimmed and lowercased.
const STOP_WORDS: [&str; 6] = ["uses", "use", "calls", "sends", "reads", "writes"];

/// L6: each box grows by this many px on every side.
const SPACING_PX: i32 = 30;

/// L11: the longest description, in chars.
const MAX_DESCRIPTION_CHARS: usize = 110;

/// L17: the most boxes a context view should hold.
const MAX_C1_BOXES: usize = 8;

/// L5: the grid step in px.
const GRID: i32 = 10;

/// The first protocol word of `text` (L18): a word is a maximal run of
/// ASCII letters and digits.
fn protocol_word(text: &str) -> Option<&str> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| PROTOCOL_WORDS.contains(w))
}

/// True when `label`, trimmed and lowercased, is only a stop word (L13).
fn is_stop_word(label: &str) -> bool {
    STOP_WORDS.contains(&label.trim().to_lowercase().as_str())
}

/// True when `text` is absent or blank.
// DECISION: blank (whitespace only) counts as empty for L9, L10, L12 and
// the L8 title: a name of spaces draws nothing, as an absent one.
fn blank(text: Option<&str>) -> bool {
    text.is_none_or(|t| t.trim().is_empty())
}

/// A count as `u32`, saturating.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The CLI-1 counts of one scene.
fn stats(scene: &Scene) -> Stats {
    let rows: BTreeSet<i32> = scene.boxes.iter().map(|b| b.rect.centre().y).collect();
    let columns: BTreeSet<i32> = scene.boxes.iter().map(|b| b.rect.centre().x).collect();
    Stats {
        boxes: count(scene.boxes.len()),
        lines: count(scene.lines.len()),
        crossings: scene.crossings,
        budget: scene.options.max_crossings,
        rows: count(rows.len()),
        columns: count(columns.len()),
    }
}

/// `"a", "b"`: names quoted and joined by commas.
fn quoted_list<'n>(names: impl Iterator<Item = &'n str>) -> String {
    names
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One lint run over one scene: the inputs and the findings so far.
struct Lint<'a> {
    model: &'a Model,
    scene: &'a Scene,
    /// The view declaration, `None` for a view not in the file.
    view_span: Option<Span>,
    findings: Vec<Finding>,
}

impl<'a> Lint<'a> {
    fn new(model: &'a Model, scene: &'a Scene) -> Self {
        Lint {
            model,
            scene,
            view_span: model.view(&scene.key).map(|v| v.span),
            findings: Vec::new(),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one argument per `Finding` field the rule fills"
    )]
    fn push(
        &mut self,
        rule: Rule,
        subjects: Vec<ModelId>,
        items: Vec<ItemKey>,
        spans: Vec<Span>,
        message: String,
    ) {
        self.findings.push(Finding {
            rule,
            view: Some(self.scene.key.clone()),
            subjects,
            items,
            spans,
            message,
            hint: None,
        });
    }

    /// The view declaration as a span list (empty for a default view).
    fn view_spans(&self) -> Vec<Span> {
        self.view_span.into_iter().collect()
    }

    fn element_of(&self, b: BoxId) -> ElementId {
        self.scene.boxes[b.0 as usize].element
    }

    fn name(&self, e: ElementId) -> &'a str {
        &self.model.element(e).name.value
    }

    fn kind_of(&self, b: BoxId) -> ElementKind {
        self.scene.boxes[b.0 as usize].kind
    }

    /// `"<src>" -> "<dst>"` of a drawn line, by its boxes' names.
    fn ends(&self, l: &SceneLine) -> String {
        format!(
            "\"{}\" -> \"{}\"",
            self.name(self.element_of(l.source)),
            self.name(self.element_of(l.destination))
        )
    }

    fn rel_span(&self, r: RelationshipId) -> Span {
        self.model.relationship(r).span
    }

    fn el_span(&self, e: ElementId) -> Span {
        self.model.element(e).span
    }

    /// The winning relationship's description, if any.
    fn description(&self, r: RelationshipId) -> Option<&'a str> {
        self.model
            .relationship(r)
            .description
            .as_ref()
            .map(|d| d.value.as_str())
    }

    /// The technology a line shows: none with `technology off` (CORE-4),
    /// as the prototype blanks it before linting.
    fn shown_technology(&self, r: RelationshipId) -> Option<&'a str> {
        if !self.scene.options.technology {
            return None;
        }
        self.model
            .relationship(r)
            .technology
            .as_ref()
            .map(|t| t.value.as_str())
            .filter(|t| !t.trim().is_empty())
    }

    /// One finding about one line.
    fn push_line(&mut self, rule: Rule, l: &SceneLine, message: String) {
        self.push(
            rule,
            vec![ModelId::Relationship(l.relationship)],
            vec![ItemKey::Line(l.edge)],
            vec![self.rel_span(l.relationship)],
            message,
        );
    }

    /// One finding about one box.
    fn push_box(&mut self, rule: Rule, b: &SceneBox, message: String) {
        self.push(
            rule,
            vec![ModelId::Element(b.element)],
            vec![ItemKey::Box(b.id)],
            vec![self.el_span(b.element)],
            message,
        );
    }

    /// One view-wide finding about several boxes.
    fn push_boxes(&mut self, rule: Rule, boxes: &[&SceneBox], message: String) {
        self.push(
            rule,
            boxes.iter().map(|b| ModelId::Element(b.element)).collect(),
            boxes.iter().map(|b| ItemKey::Box(b.id)).collect(),
            boxes.iter().map(|b| self.el_span(b.element)).collect(),
            message,
        );
    }

    /// L1.
    fn unroutable(&mut self) {
        let scene = self.scene;
        for l in scene.lines.iter().filter(|l| !l.routed) {
            let msg = format!("could not route {}: give the boxes more room", self.ends(l));
            self.push_line(Rule::Unroutable, l, msg);
        }
    }

    /// L2, routed lines only.
    fn segment_angles(&mut self) {
        let scene = self.scene;
        for l in scene.lines.iter().filter(|l| l.routed) {
            for w in l.points.windows(2) {
                let (a, b) = (w[0], w[1]);
                let (dx, dy) = ((b.x - a.x).abs(), (b.y - a.y).abs());
                if dx == 0 || dy == 0 || dx == dy {
                    continue;
                }
                let msg = format!(
                    "{}: segment ({}, {})-({}, {}) is not at 0/45/90 degrees",
                    self.ends(l),
                    a.x,
                    a.y,
                    b.x,
                    b.y
                );
                self.push_line(Rule::SegmentAngle, l, msg);
            }
        }
    }

    /// L3, routed lines only; once per line and box.
    // DECISION: once per (line, box), not once per segment as the
    // prototype: a line bending inside a box is one fault to fix.
    fn through_boxes(&mut self) {
        let scene = self.scene;
        for l in scene.lines.iter().filter(|l| l.routed) {
            for b in &scene.boxes {
                if b.id == l.source || b.id == l.destination {
                    continue;
                }
                let through = l
                    .points
                    .windows(2)
                    .any(|w| Segment::new(w[0], w[1]).passes_through(&b.rect));
                if !through {
                    continue;
                }
                let msg = format!(
                    "{} passes through box \"{}\"",
                    self.ends(l),
                    self.name(b.element)
                );
                self.push(
                    Rule::ThroughBox,
                    vec![
                        ModelId::Relationship(l.relationship),
                        ModelId::Element(b.element),
                    ],
                    vec![ItemKey::Line(l.edge), ItemKey::Box(b.id)],
                    vec![self.rel_span(l.relationship), self.el_span(b.element)],
                    msg,
                );
            }
        }
    }

    /// L4 with the CORE-26 split hint.
    fn crossings(&mut self) {
        let scene = self.scene;
        let (crossings, budget) = (scene.crossings, scene.options.max_crossings);
        if crossings <= budget {
            return;
        }
        let edges: BTreeSet<EdgeId> = scene
            .crossing_pairs()
            .iter()
            .flat_map(|&(a, b, _)| [a, b])
            .collect();
        let lines: Vec<&SceneLine> = scene
            .lines
            .iter()
            .filter(|l| edges.contains(&l.edge))
            .collect();
        let box_ids: BTreeSet<BoxId> = lines
            .iter()
            .flat_map(|l| [l.source, l.destination])
            .collect();
        let mut boxes: Vec<ElementId> = Vec::new();
        for b in box_ids {
            let e = self.element_of(b);
            if !boxes.contains(&e) {
                boxes.push(e);
            }
        }
        let mut relationships: Vec<RelationshipId> = Vec::new();
        for l in &lines {
            if !relationships.contains(&l.relationship) {
                relationships.push(l.relationship);
            }
        }
        // DECISION: the prototype's L4 text, then the CORE-26 hint after a
        // semicolon (CORE-16: "L4 ends with the split hint").
        let msg = format!(
            "{crossings} line crossings, budget is {budget}: move boxes or raise \
             maxCrossings on purpose; split the view (focus, hide); boxes involved: {}",
            quoted_list(boxes.iter().map(|&e| self.name(e)))
        );
        let mut subjects: Vec<ModelId> = boxes.iter().map(|&e| ModelId::Element(e)).collect();
        subjects.extend(relationships.iter().map(|&r| ModelId::Relationship(r)));
        let mut spans: Vec<Span> = relationships.iter().map(|&r| self.rel_span(r)).collect();
        spans.extend(self.view_span);
        let items = lines.iter().map(|l| ItemKey::Line(l.edge)).collect();
        self.push(Rule::Crossings, subjects, items, spans, msg);
        if let Some(f) = self.findings.last_mut() {
            f.hint = Some(SplitHint {
                boxes,
                relationships,
            });
        }
    }

    /// L5, from the `at` and `size` lines as written.
    fn off_grid(&mut self) {
        let (model, scene) = (self.model, self.scene);
        let Some(view) = model.view(&scene.key) else {
            return;
        };
        let off = |v: i32| v.rem_euclid(GRID) != 0;
        for b in &scene.boxes {
            let name = self.name(b.element);
            if let Some(at) = view.position_of(b.element)
                && (off(at.x.value) || off(at.y.value))
            {
                let msg = format!(
                    "box \"{name}\" centre ({}, {}) is off the 10px grid",
                    at.x.value, at.y.value
                );
                self.push(
                    Rule::OffGrid,
                    vec![ModelId::Element(b.element)],
                    vec![ItemKey::Box(b.id)],
                    vec![at.span, self.el_span(b.element)],
                    msg,
                );
            }
            // A half-size on the grid is a size on a 20px step.
            if let Some(size) = view.size_of(b.element)
                && (size.width.value.rem_euclid(2 * GRID) != 0
                    || size.height.value.rem_euclid(2 * GRID) != 0)
            {
                let msg = format!(
                    "box \"{name}\" size {}x{} has a half-size off the 10px grid",
                    size.width.value, size.height.value
                );
                self.push(
                    Rule::OffGrid,
                    vec![ModelId::Element(b.element)],
                    vec![ItemKey::Box(b.id)],
                    vec![size.span, self.el_span(b.element)],
                    msg,
                );
            }
        }
    }

    /// L6, per pair of boxes in box order.
    fn too_close(&mut self) {
        let scene = self.scene;
        for (i, a) in scene.boxes.iter().enumerate() {
            for b in &scene.boxes[i + 1..] {
                if !a
                    .rect
                    .inflate(SPACING_PX)
                    .overlaps(&b.rect.inflate(SPACING_PX))
                {
                    continue;
                }
                let msg = format!(
                    "boxes \"{}\" and \"{}\" are closer than 60px",
                    self.name(a.element),
                    self.name(b.element)
                );
                self.push(
                    Rule::TooClose,
                    vec![ModelId::Element(a.element), ModelId::Element(b.element)],
                    vec![ItemKey::Box(a.id), ItemKey::Box(b.id)],
                    vec![self.el_span(a.element), self.el_span(b.element)],
                    msg,
                );
            }
        }
    }

    /// L7.
    fn alignment(&mut self) {
        let s = stats(self.scene);
        if s.boxes > 3 && s.rows + s.columns > s.boxes + 1 {
            let spans = self.view_spans();
            self.push(
                Rule::Alignment,
                Vec::new(),
                Vec::new(),
                spans,
                "boxes share few rows/columns: snap centres onto a common grid".to_owned(),
            );
        }
    }

    /// L8.
    fn no_title(&mut self) {
        let untitled = self
            .scene
            .title
            .as_ref()
            .is_none_or(|t| t.lines.iter().all(|l| l.trim().is_empty()));
        if untitled {
            let spans = self.view_spans();
            self.push(
                Rule::NoTitle,
                Vec::new(),
                Vec::new(),
                spans,
                "view has no title".to_owned(),
            );
        }
    }

    /// L9, L10, L11 per box, from the model element.
    fn box_texts(&mut self) {
        let (model, scene) = (self.model, self.scene);
        for b in &scene.boxes {
            let e = model.element(b.element);
            let name = e.name.value.as_str();
            let description = e.description.as_ref().map(|d| d.value.as_str());
            if blank(Some(name)) || blank(description) {
                let msg = format!("\"{name}\" needs a name and a description");
                self.push_box(Rule::NameOrDescription, b, msg);
            }
            let technical = matches!(e.kind, ElementKind::Container | ElementKind::Component);
            if technical && blank(e.technology.as_ref().map(|t| t.value.as_str())) {
                let msg = format!("\"{name}\" needs a technology");
                self.push_box(Rule::NoTechnology, b, msg);
            }
            if description.is_some_and(|d| d.chars().count() > MAX_DESCRIPTION_CHARS) {
                let msg = format!("\"{name}\" description is over 110 characters");
                self.push_box(Rule::LongDescription, b, msg);
            }
        }
    }

    /// L12, L13 per line, from the winning relationship.
    fn line_labels(&mut self) {
        let scene = self.scene;
        for l in &scene.lines {
            match self.description(l.relationship) {
                d if blank(d) => {
                    let msg = format!("{} has no label", self.ends(l));
                    self.push_line(Rule::NoLabel, l, msg);
                }
                Some(d) if is_stop_word(d) => {
                    let msg = format!(
                        "{}: label \"{d}\" says nothing, state the intent",
                        self.ends(l)
                    );
                    self.push_line(Rule::VagueLabel, l, msg);
                }
                _ => {}
            }
        }
    }

    /// L14, once per pair of boxes, at the pair's first line.
    fn two_way(&mut self) {
        let scene = self.scene;
        let mut seen: BTreeSet<(BoxId, BoxId)> = BTreeSet::new();
        for l in &scene.lines {
            let Some(back) = scene
                .lines
                .iter()
                .find(|m| m.source == l.destination && m.destination == l.source)
            else {
                continue;
            };
            let pair = (l.source.min(l.destination), l.source.max(l.destination));
            if l.source == l.destination || !seen.insert(pair) {
                continue;
            }
            let msg = format!(
                "\"{}\" <-> \"{}\": two-way pair, check both are needed",
                self.name(self.element_of(l.source)),
                self.name(self.element_of(l.destination))
            );
            self.push(
                Rule::TwoWay,
                vec![
                    ModelId::Relationship(l.relationship),
                    ModelId::Relationship(back.relationship),
                ],
                vec![ItemKey::Line(l.edge), ItemKey::Line(back.edge)],
                vec![self.rel_span(l.relationship), self.rel_span(back.relationship)],
                msg,
            );
        }
    }

    /// L15..L18 on a context view.
    fn c1(&mut self) {
        let scene = self.scene;
        if scene.kind != ViewKind::SystemContext {
            return;
        }
        // DECISION: L15 and L16 are one finding per view, as the
        // prototype prints them, carrying every line or box involved.
        let teched: Vec<&SceneLine> = scene
            .lines
            .iter()
            .filter(|l| self.shown_technology(l.relationship).is_some())
            .collect();
        if !teched.is_empty() {
            self.push(
                Rule::C1Technology,
                teched
                    .iter()
                    .map(|l| ModelId::Relationship(l.relationship))
                    .collect(),
                teched.iter().map(|l| ItemKey::Line(l.edge)).collect(),
                teched.iter().map(|l| self.rel_span(l.relationship)).collect(),
                "C1 shows a protocol/technology on a line: set \"technology off\"".to_owned(),
            );
        }
        let bad: Vec<&SceneBox> = scene
            .boxes
            .iter()
            .filter(|b| !matches!(b.kind, ElementKind::Person | ElementKind::SoftwareSystem))
            .collect();
        if !bad.is_empty() {
            let msg = format!(
                "C1 may only hold people and software systems, found {}",
                quoted_list(bad.iter().map(|b| self.name(b.element)))
            );
            self.push_boxes(Rule::C1BoxKind, &bad, msg);
        }
        if scene.boxes.len() > MAX_C1_BOXES {
            let spans = self.view_spans();
            self.push(
                Rule::C1TooManyBoxes,
                Vec::new(),
                Vec::new(),
                spans,
                format!(
                    "C1 has {} boxes: a context diagram should fit in one glance",
                    scene.boxes.len()
                ),
            );
        }
        for b in &scene.boxes {
            let name = self.name(b.element);
            if protocol_word(name).is_some() {
                self.push_box(Rule::C1Protocol, b, format!("C1 box \"{name}\" names a protocol"));
            }
        }
        for l in &scene.lines {
            if let Some(d) = self.description(l.relationship)
                && protocol_word(d).is_some()
            {
                self.push_line(Rule::C1Protocol, l, format!("C1 label \"{d}\" names a protocol"));
            }
        }
    }

    /// L19, L20 on a container view.
    fn c2(&mut self) {
        let scene = self.scene;
        if scene.kind != ViewKind::Container {
            return;
        }
        let bad: Vec<&SceneBox> = scene
            .boxes
            .iter()
            .filter(|b| b.kind == ElementKind::Component)
            .collect();
        if !bad.is_empty() {
            let msg = format!(
                "C2 must not show components, found {}",
                quoted_list(bad.iter().map(|b| self.name(b.element)))
            );
            self.push_boxes(Rule::C2Component, &bad, msg);
        }
        for l in &scene.lines {
            let kinds = [self.kind_of(l.source), self.kind_of(l.destination)];
            if kinds.contains(&ElementKind::Container)
                && !kinds.contains(&ElementKind::Person)
                && self.shown_technology(l.relationship).is_none()
            {
                let msg = format!("{}: container link without a protocol", self.ends(l));
                self.push_line(Rule::C2NoTechnology, l, msg);
            }
        }
    }

    /// L21 on a component view.
    fn c3(&mut self) {
        let (model, scene) = (self.model, self.scene);
        if scene.kind != ViewKind::Component {
            return;
        }
        let scope = scene.scope.element();
        let shown = scene.boxes.iter().any(|b| {
            b.kind == ElementKind::Component && model.element(b.element).parent == scope
        });
        if !shown {
            let spans = self.view_spans();
            self.push(
                Rule::C3NoScopeComponent,
                scope.map(ModelId::Element).into_iter().collect(),
                Vec::new(),
                spans,
                "C3 shows no component of its scope container".to_owned(),
            );
        }
    }

    /// L23, L24 from the label fit `label` placed.
    fn label_fit(&mut self) {
        let scene = self.scene;
        for lab in &scene.labels {
            let ends = scene
                .lines
                .iter()
                .find(|l| l.edge == lab.edge)
                .map_or_else(String::new, |l| self.ends(l));
            let desc = self.description(lab.relationship).unwrap_or("");
            match &lab.fit {
                Fit::Clear => {}
                Fit::OverLines(covered) => {
                    let mut subjects = vec![ModelId::Relationship(lab.relationship)];
                    let mut items = vec![ItemKey::Label(lab.edge)];
                    for &e in covered {
                        items.push(ItemKey::Line(e));
                        if let Some(l) = scene.lines.iter().find(|l| l.edge == e) {
                            subjects.push(ModelId::Relationship(l.relationship));
                        }
                    }
                    self.push(
                        Rule::LabelOverLine,
                        subjects,
                        items,
                        vec![self.rel_span(lab.relationship)],
                        format!("label \"{desc}\" ({ends}) covers another line"),
                    );
                }
                Fit::NoRoom => self.push(
                    Rule::LabelNoRoom,
                    vec![ModelId::Relationship(lab.relationship)],
                    vec![ItemKey::Label(lab.edge)],
                    vec![self.rel_span(lab.relationship)],
                    format!("no room for label \"{desc}\" ({ends})"),
                ),
            }
        }
    }

    /// L25: one per box, label or boundary tag with a text that does not
    /// fit.
    // DECISION: one finding per item, not per text block: the box is the
    // thing to resize, whichever of its texts overflows.
    fn text_fit(&mut self) {
        let scene = self.scene;
        for b in &scene.boxes {
            let fits = b.name.fits
                && b.type_line.fits
                && b.description.as_ref().is_none_or(|d| d.fits);
            if !fits {
                let msg = format!("text of box \"{}\" does not fit its box", self.name(b.element));
                self.push_box(Rule::TextFit, b, msg);
            }
        }
        for lab in scene.labels.iter().filter(|l| !l.text.fits) {
            let ends = scene
                .lines
                .iter()
                .find(|l| l.edge == lab.edge)
                .map_or_else(String::new, |l| self.ends(l));
            let desc = self.description(lab.relationship).unwrap_or("");
            self.push(
                Rule::TextFit,
                vec![ModelId::Relationship(lab.relationship)],
                vec![ItemKey::Label(lab.edge)],
                vec![self.rel_span(lab.relationship)],
                format!("text of label \"{desc}\" ({ends}) does not fit its rectangle"),
            );
        }
        for (i, bd) in scene.boundaries.iter().enumerate() {
            if bd.tag.fits {
                continue;
            }
            let BoundaryOf::Element(e) = bd.of else {
                todo!()
            };
            self.push(
                Rule::TextFit,
                vec![ModelId::Element(e)],
                vec![ItemKey::Boundary(count(i))],
                vec![self.el_span(e)],
                format!("name tag of the \"{}\" boundary does not fit", self.name(e)),
            );
        }
    }

    /// L26: a box that is not a member of a boundary overlaps it.
    fn boundary_intrusion(&mut self) {
        let scene = self.scene;
        for (i, bd) in scene.boundaries.iter().enumerate() {
            let BoundaryOf::Element(scope) = bd.of else {
                todo!()
            };
            for b in &scene.boxes {
                if bd.members.contains(&b.id) || !b.rect.overlaps(&bd.rect) {
                    continue;
                }
                let msg = format!(
                    "box \"{}\" is not part of \"{}\" but overlaps its boundary",
                    self.name(b.element),
                    self.name(scope)
                );
                self.push(
                    Rule::BoundaryIntrusion,
                    vec![ModelId::Element(b.element), ModelId::Element(scope)],
                    vec![ItemKey::Box(b.id), ItemKey::Boundary(count(i))],
                    vec![self.el_span(b.element)],
                    msg,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fmt::Write as _;
    use std::sync::atomic::AtomicBool;

    use crate::geom::{Point, Rect};
    use crate::label::place_labels;
    use crate::model::{ElementKind, ViewScope, parse};
    use crate::place::{Keep, place};
    use crate::route::{RouteConfig, route};
    use crate::scene::{SceneBox, SceneLine, build};
    use crate::view::{EdgeId, plan};

    /// A small shop written with roles only: a person, a system with
    /// three containers, an external system. `worker` has no description
    /// and no technology (L9, L10).
    const SHOP: &str = r#"workspace {
  model {
    operator = person "Operator" "Runs the shop day to day."
    shop = softwareSystem "Shop" "Sells goods online." {
      web = container "Web app" "Serves the shop pages." "Rust"
      db = container "Database" "Stores orders." "SQLite"
      worker = container "Worker"
    }
    bank = softwareSystem "Bank" "Takes card payments." "External"
    operator -> web "Manages orders in" "HTTPS"
    web -> db "Reads and writes orders" "SQL"
    worker -> db "Reads open orders from" "SQL"
    web -> bank "Charges cards through" "HTTPS"
  }
  views {
    container shop "c2" "Shop containers" {
      include *
    }
  }
}
"#;

    /// `at` and `size` lines on and off the 10px grid (L5).
    const GRID: &str = r#"workspace {
  model {
    shop = softwareSystem "Shop" "Sells goods online." {
      web = container "Web app" "Serves the shop pages." "Rust"
      db = container "Database" "Stores orders." "SQLite"
      queue = container "Queue" "Holds jobs." "Redis"
    }
    web -> db "Reads and writes orders" "SQL"
    web -> queue "Puts jobs on" "AMQP"
  }
  views {
    container shop "grid" "Grid" {
      include *
      at web 205 200
      at db 600 200
      at queue 1000 200
      size db 250 140
    }
  }
}
"#;

    /// Two systems that use each other (L14).
    const TWO_WAY: &str = r#"workspace {
  model {
    shop = softwareSystem "Shop" "Sells goods online."
    bank = softwareSystem "Bank" "Takes card payments."
    stock = softwareSystem "Stock" "Counts goods."
    shop -> bank "Charges cards through"
    bank -> shop "Confirms payments to"
    shop -> stock "Reserves goods in"
  }
  views {
    systemContext shop "c1" "Two way" {
      include *
    }
  }
}
"#;

    /// A context view with protocols in a box name and a label, and
    /// technologies on its lines (L15, L18).
    const CONTEXT: &str = r#"workspace {
  model {
    operator = person "Operator" "Runs the shop day to day."
    shop = softwareSystem "HTTP gateway" "Sells goods online." {
      web = container "Web app" "Serves the shop pages." "Rust"
    }
    bank = softwareSystem "Bank" "Takes card payments." "External"
    operator -> shop "Manages orders in" "HTTPS"
    shop -> bank "Calls the REST API of" "HTTPS"
  }
  views {
    systemContext shop "c1" "Context" {
      include *
    }
  }
}
"#;

    /// Container links with and without a technology (L20).
    const LINKS: &str = r#"workspace {
  model {
    operator = person "Operator" "Runs the shop day to day."
    shop = softwareSystem "Shop" "Sells goods online." {
      web = container "Web app" "Serves the shop pages." "Rust"
      db = container "Database" "Stores orders." "SQLite"
      queue = container "Queue" "Holds jobs." "Redis"
    }
    operator -> web "Manages orders in"
    web -> db "Reads and writes orders"
    web -> queue "Puts jobs on" "AMQP"
  }
  views {
    container shop "c2" "Links" {
      include *
    }
  }
}
"#;

    /// Components of two containers; `c3` is the front's view.
    const COMPONENTS: &str = r#"workspace {
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
    pages -> cart "Fills"
    cart -> orders "Writes carts to"
  }
  views {
    component front "c3" "Front components" {
      include *
    }
  }
}
"#;

    /// Three containers, two with a component view, one without (L22).
    /// Relationships in id order: operator -> pages, pages -> cart,
    /// cart -> orders, handler -> bank, orders -> bank.
    const COVER: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    bank = softwareSystem "Bank" "Takes payments." "External"
    shop = softwareSystem "Shop" "Sells goods." {
      front = container "Front" "Shows pages." "Rust" {
        pages = component "Pages" "Renders pages." "Rust"
        cart = component "Cart" "Holds the cart." "Rust"
      }
      store = container "Store" "Keeps orders." "SQLite" {
        orders = component "Orders" "Order table." "SQL"
      }
      api = container "Api" "Takes payment callbacks." "Rust" {
        handler = component "Handler" "Handles callbacks." "Rust"
      }
    }
    operator -> pages "Browses"
    pages -> cart "Fills"
    cart -> orders "Writes carts to"
    handler -> bank "Confirms payments with"
    orders -> bank "Reports totals to"
  }
  views {
    component front "front" "Front components" {
      include *
    }
    component store "store" "Store components" {
      include *
    }
  }
}
"#;

    /// `scene` drawing exactly one line per entry, each standing for the
    /// given relationships (a merged line when more than one).
    fn drawing(scene: &Scene, lines: &[&[RelationshipId]]) -> Scene {
        let mut out = scene.clone();
        let template = scene.lines[0].clone();
        out.labels.clear();
        out.lines = lines
            .iter()
            .enumerate()
            .map(|(i, rs)| {
                let mut l = template.clone();
                l.edge = EdgeId(count(i));
                l.relationship = rs[0];
                l.relationships = rs.to_vec();
                l
            })
            .collect();
        out
    }

    /// Name, description, technology and label faults (L9..L13).
    fn names_src() -> String {
        let long = "x".repeat(111);
        let wide = "é".repeat(110);
        format!(
            r#"workspace {{
  model {{
    operator = person "Operator" "Runs the shop day to day."
    shop = softwareSystem "Shop" "Sells goods online." {{
      web = container "Web app" "Serves the shop pages." "Rust"
      worker = container "Worker"
      db = container "Database" "{long}" "SQLite"
      cache = container "Cache" "{wide}" "Redis"
    }}
    operator -> web " Uses "
    web -> db
    worker -> db "Reads open orders from" "SQL"
    web -> cache "uses the cache to fetch pages" "Redis"
  }}
  views {{
    container shop "c2" "Names" {{
      include *
    }}
  }}
}}
"#
        )
    }

    /// The prototype example converted to c4rizer (tests/data).
    const EXAMPLE: &str = include_str!("../../../tests/data/example.dsl");

    fn model_of(src: &str) -> Model {
        match parse(src) {
            Ok(p) => p.model,
            Err(e) => panic!("{e}"),
        }
    }

    /// The built scene of view `key`: auto-placed where no `at` line,
    /// routed with the default config, labelled.
    fn scene_of(model: &Model, key: &str) -> Scene {
        let plan = plan(model, model.view(key).expect("view"));
        let centres = place(&plan, &Keep::default());
        let routes = route(
            &plan,
            &centres,
            &RouteConfig::DEFAULT,
            &AtomicBool::new(false),
        )
        .expect("not stopped");
        let labels = place_labels(&plan, &routes);
        build(&plan, &routes, &labels)
    }

    fn el(model: &Model, key: &str) -> ElementId {
        model.find(key).unwrap_or_else(|| panic!("no element {key}"))
    }

    fn rel(model: &Model, src: &str, dst: &str) -> RelationshipId {
        let (s, d) = (el(model, src), el(model, dst));
        model
            .relationships
            .iter()
            .find(|r| r.source.value == s && r.destination.value == d)
            .unwrap_or_else(|| panic!("no relationship {src} -> {dst}"))
            .id
    }

    fn box_index(model: &Model, scene: &Scene, key: &str) -> usize {
        let e = el(model, key);
        scene
            .boxes
            .iter()
            .position(|b| b.element == e)
            .unwrap_or_else(|| panic!("no box {key}"))
    }

    fn the_box<'a>(model: &Model, scene: &'a Scene, key: &str) -> &'a SceneBox {
        &scene.boxes[box_index(model, scene, key)]
    }

    fn line_index(model: &Model, scene: &Scene, src: &str, dst: &str) -> usize {
        let r = rel(model, src, dst);
        scene
            .lines
            .iter()
            .position(|l| l.relationship == r)
            .unwrap_or_else(|| panic!("no line {src} -> {dst}"))
    }

    fn the_line<'a>(model: &Model, scene: &'a Scene, src: &str, dst: &str) -> &'a SceneLine {
        &scene.lines[line_index(model, scene, src, dst)]
    }

    fn of_rule(report: &Report, rule: Rule) -> Vec<&Finding> {
        report.findings.iter().filter(|f| f.rule == rule).collect()
    }

    /// Moves every box so its centre is `centres[i]` (px), keeping its
    /// size.
    fn set_centres(scene: &mut Scene, centres: &[(i32, i32)]) {
        for (b, &(x, y)) in scene.boxes.iter_mut().zip(centres) {
            let (hw, hh) = (b.rect.width() / 2, b.rect.height() / 2);
            b.rect = Rect::from_centre(Point::new(x, y), hw, hh);
        }
    }

    /// Every rule, in table order.
    const ALL: [Rule; 26] = [
        Rule::Unroutable,
        Rule::SegmentAngle,
        Rule::ThroughBox,
        Rule::Crossings,
        Rule::OffGrid,
        Rule::TooClose,
        Rule::Alignment,
        Rule::NoTitle,
        Rule::NameOrDescription,
        Rule::NoTechnology,
        Rule::LongDescription,
        Rule::NoLabel,
        Rule::VagueLabel,
        Rule::TwoWay,
        Rule::C1Technology,
        Rule::C1BoxKind,
        Rule::C1TooManyBoxes,
        Rule::C1Protocol,
        Rule::C2Component,
        Rule::C2NoTechnology,
        Rule::C3NoScopeComponent,
        Rule::Coverage,
        Rule::LabelOverLine,
        Rule::LabelNoRoom,
        Rule::TextFit,
        Rule::BoundaryIntrusion,
    ];

    // One test per requirement.
    #[test]
    fn core_16_errors_and_warnings_split_like_prototype() {
        // Every rule's level is the lint table's; `has_errors` is true
        // only with an error; warnings alone keep it false.
        let warnings = ["L7", "L11", "L14", "L17", "L20", "L23", "L24", "L25"];
        for rule in ALL {
            let want = if warnings.contains(&rule.code()) {
                Severity::Warning
            } else {
                Severity::Error
            };
            assert_eq!(rule.severity(), want, "{}", rule.code());
        }
        assert!(Severity::Warning < Severity::Error);

        // A clean view with one warning (L11) only.
        let long = "x".repeat(111);
        let src = SHOP
            .replace("Stores orders.", &long)
            .replace(r#"worker = container "Worker""#, r#"worker = container "Worker" "Runs jobs." "Rust""#);
        let model = model_of(&src);
        let mut scene = scene_of(&model, "c2");
        // Clear geometry so only the description rule speaks.
        scene.labels.iter_mut().for_each(|l| l.fit = Fit::Clear);
        scene.labels.iter_mut().for_each(|l| l.text.fits = true);
        for b in &mut scene.boxes {
            b.name.fits = true;
            b.type_line.fits = true;
            if let Some(d) = b.description.as_mut() {
                d.fits = true;
            }
        }
        let report = lint(&model, &scene);
        assert!(
            report.errors().is_empty(),
            "unexpected errors: {:?}",
            report.errors()
        );
        assert!(!report.has_errors());
        let warned: Vec<Rule> = report.warnings().iter().map(|f| f.rule).collect();
        assert!(warned.contains(&Rule::LongDescription));
        assert!(report.warnings().iter().all(|f| f.severity() == Severity::Warning));

        // One error flips it.
        scene.title = None;
        let report = lint(&model, &scene);
        assert!(report.has_errors());
        assert_eq!(report.errors().len(), 1);
        assert_eq!(report.errors()[0].rule, Rule::NoTitle);
        assert_eq!(report.errors()[0].severity(), Severity::Error);
        assert_eq!(
            report.warnings().len() + report.errors().len(),
            report.findings.len()
        );
    }

    #[test]
    fn core_16_rule_codes_l1_to_l26() {
        for (i, rule) in ALL.iter().enumerate() {
            assert_eq!(rule.code(), format!("L{}", i + 1));
        }
        // Rule order is table order.
        assert!(ALL.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn core_16_findings_name_ids_items_and_spans() {
        // Each finding names its model ids; per-view findings carry the
        // view key and canvas items; spans point at the declarations.
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        scene.title = None;
        let report = lint(&model, &scene);
        assert_eq!(report.view, "c2");

        let worker = el(&model, "worker");
        let l10 = of_rule(&report, Rule::NoTechnology);
        assert_eq!(l10.len(), 1);
        let f = l10[0];
        assert_eq!(f.view.as_deref(), Some("c2"));
        assert_eq!(f.subjects, vec![ModelId::Element(worker)]);
        assert_eq!(
            f.items,
            vec![ItemKey::Box(the_box(&model, &scene, "worker").id)]
        );
        assert_eq!(f.spans, vec![model.element(worker).span]);
        assert_eq!(f.message, r#""Worker" needs a technology"#);
        assert_eq!(f.to_string(), f.message);
        assert!(f.hint.is_none());

        let l9 = of_rule(&report, Rule::NameOrDescription);
        assert_eq!(l9.len(), 1);
        assert_eq!(l9[0].message, r#""Worker" needs a name and a description"#);

        let l8 = of_rule(&report, Rule::NoTitle);
        assert_eq!(l8.len(), 1);
        assert!(l8[0].items.is_empty());
        assert!(l8[0].subjects.is_empty());
        assert_eq!(l8[0].spans, vec![model.view("c2").expect("view").span]);
        assert_eq!(l8[0].message, "view has no title");

        // A view not written in the file has no spans for view rules.
        scene.key = "default-view".to_owned();
        let report = lint(&model, &scene);
        assert!(of_rule(&report, Rule::NoTitle)[0].spans.is_empty());
        assert_eq!(report.view, "default-view");
    }

    #[test]
    fn core_16_deterministic_order() {
        // Same model and scene give an equal report, rule order first.
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        scene.title = None;
        set_centres(&mut scene, &[(0, 0), (100, 13), (200, 27), (300, 31), (400, 45)]);
        let a = lint(&model, &scene);
        let b = lint(&model, &scene);
        assert_eq!(a, b);
        assert!(a.findings.len() > 3);
        assert!(a.findings.windows(2).all(|w| w[0].rule <= w[1].rule));
        // Within one rule, item order: the L6 pairs come in box order.
        let l6: Vec<&Vec<ItemKey>> = of_rule(&a, Rule::TooClose).iter().map(|f| &f.items).collect();
        let mut sorted = l6.clone();
        sorted.sort();
        assert_eq!(l6, sorted);
    }

    #[test]
    fn core_13_unroutable_line_l1_only() {
        // An unroutable line is L1 and never L2 or L3.
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        let i = line_index(&model, &scene, "web", "bank");
        let bank = the_box(&model, &scene, "db").rect;
        let line = &mut scene.lines[i];
        line.routed = false;
        // Off-angle and through another box: still L1 only.
        line.points = vec![
            Point::new(bank.min.x - 37, bank.min.y - 11),
            Point::new(bank.max.x + 41, bank.max.y + 13),
        ];
        let edge = line.edge;
        let report = lint(&model, &scene);
        let l1 = of_rule(&report, Rule::Unroutable);
        assert_eq!(l1.len(), 1);
        let r = rel(&model, "web", "bank");
        assert_eq!(l1[0].subjects, vec![ModelId::Relationship(r)]);
        assert_eq!(l1[0].items, vec![ItemKey::Line(edge)]);
        assert_eq!(l1[0].spans, vec![model.relationship(r).span]);
        assert_eq!(
            l1[0].message,
            r#"could not route "Web app" -> "Bank": give the boxes more room"#
        );
        for rule in [Rule::SegmentAngle, Rule::ThroughBox] {
            assert!(
                of_rule(&report, rule)
                    .iter()
                    .all(|f| !f.items.contains(&ItemKey::Line(edge))),
                "{rule:?}"
            );
        }
    }

    #[test]
    fn core_16_off_angle_segment_l2() {
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        let before = of_rule(&lint(&model, &scene), Rule::SegmentAngle).len();
        assert_eq!(before, 0, "a routed scene is octilinear");
        let far = scene.bounds.max.x + 1000;
        let i = line_index(&model, &scene, "web", "db");
        scene.lines[i].points = vec![
            Point::new(far, 0),
            Point::new(far + 30, 0),
            Point::new(far + 60, 10),
            Point::new(far + 80, 30),
        ];
        let report = lint(&model, &scene);
        let l2 = of_rule(&report, Rule::SegmentAngle);
        assert_eq!(l2.len(), 1, "{l2:?}");
        assert_eq!(l2[0].items, vec![ItemKey::Line(scene.lines[i].edge)]);
        let far30 = far + 30;
        let far60 = far + 60;
        assert_eq!(
            l2[0].message,
            format!(
                r#""Web app" -> "Database": segment ({far30}, 0)-({far60}, 10) is not at 0/45/90 degrees"#
            )
        );
        assert!(of_rule(&report, Rule::ThroughBox).is_empty());
    }

    #[test]
    fn core_16_segment_through_box_l3() {
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        assert!(of_rule(&lint(&model, &scene), Rule::ThroughBox).is_empty());
        let bank = the_box(&model, &scene, "bank").clone();
        let i = line_index(&model, &scene, "operator", "web");
        let c = bank.rect.centre();
        scene.lines[i].points = vec![
            Point::new(bank.rect.min.x - 40, c.y),
            Point::new(bank.rect.max.x + 40, c.y),
        ];
        // Running along a box border is not through it.
        let j = line_index(&model, &scene, "worker", "db");
        scene.lines[j].points = vec![
            Point::new(bank.rect.min.x - 40, bank.rect.min.y),
            Point::new(bank.rect.max.x + 40, bank.rect.min.y),
        ];
        let report = lint(&model, &scene);
        let l3 = of_rule(&report, Rule::ThroughBox);
        assert_eq!(l3.len(), 1, "{l3:?}");
        let r = rel(&model, "operator", "web");
        assert_eq!(
            l3[0].subjects,
            vec![ModelId::Relationship(r), ModelId::Element(bank.element)]
        );
        assert_eq!(
            l3[0].items,
            vec![ItemKey::Line(scene.lines[i].edge), ItemKey::Box(bank.id)]
        );
        assert_eq!(l3[0].message, r#""Operator" -> "Web app" passes through box "Bank""#);
    }

    #[test]
    fn core_16_alignment_warning_l7() {
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        assert_eq!(scene.boxes.len(), 5);
        // One row, five columns: 1 + 5 = boxes + 1, no warning.
        set_centres(&mut scene, &[(0, 0), (300, 0), (600, 0), (900, 0), (1200, 0)]);
        assert!(of_rule(&lint(&model, &scene), Rule::Alignment).is_empty());
        // Two rows, five columns: 7 > 6.
        set_centres(&mut scene, &[(0, 0), (300, 300), (600, 0), (900, 0), (1200, 0)]);
        let report = lint(&model, &scene);
        let l7 = of_rule(&report, Rule::Alignment);
        assert_eq!(l7.len(), 1);
        assert_eq!(l7[0].severity(), Severity::Warning);
        assert!(l7[0].items.is_empty());
        assert_eq!(
            l7[0].message,
            "boxes share few rows/columns: snap centres onto a common grid"
        );
        // Three boxes or fewer never warn.
        let model = model_of(TWO_WAY);
        let mut scene = scene_of(&model, "c1");
        assert_eq!(scene.boxes.len(), 3);
        set_centres(&mut scene, &[(0, 0), (300, 300), (600, 600)]);
        assert!(of_rule(&lint(&model, &scene), Rule::Alignment).is_empty());
    }

    #[test]
    fn core_16_missing_title_l8() {
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        assert!(of_rule(&lint(&model, &scene), Rule::NoTitle).is_empty());
        scene.title = None;
        let report = lint(&model, &scene);
        assert_eq!(of_rule(&report, Rule::NoTitle).len(), 1);
        // A view written with no title and no description.
        let src = SHOP.replace(r#"container shop "c2" "Shop containers""#, r#"container shop "c2""#);
        let model = model_of(&src);
        let scene = scene_of(&model, "c2");
        assert_eq!(of_rule(&lint(&model, &scene), Rule::NoTitle).len(), 1);
    }

    #[test]
    fn core_16_two_way_lines_once_per_pair_l14() {
        let model = model_of(TWO_WAY);
        let scene = scene_of(&model, "c1");
        let report = lint(&model, &scene);
        let l14 = of_rule(&report, Rule::TwoWay);
        assert_eq!(l14.len(), 1, "{l14:?}");
        assert_eq!(l14[0].severity(), Severity::Warning);
        let (a, b) = (rel(&model, "shop", "bank"), rel(&model, "bank", "shop"));
        assert_eq!(
            l14[0].subjects,
            vec![ModelId::Relationship(a), ModelId::Relationship(b)]
        );
        assert_eq!(
            l14[0].items,
            vec![
                ItemKey::Line(the_line(&model, &scene, "shop", "bank").edge),
                ItemKey::Line(the_line(&model, &scene, "bank", "shop").edge)
            ]
        );
        assert_eq!(
            l14[0].message,
            r#""Shop" <-> "Bank": two-way pair, check both are needed"#
        );
    }

    #[test]
    fn core_15_label_over_line_and_no_room_l23_l24() {
        // Both name the relationship (CORE-15).
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        // Whatever label placed, start from every label clear.
        scene.labels.iter_mut().for_each(|l| l.fit = Fit::Clear);
        let report = lint(&model, &scene);
        assert!(of_rule(&report, Rule::LabelOverLine).is_empty());
        assert!(of_rule(&report, Rule::LabelNoRoom).is_empty());
        let other: EdgeId = the_line(&model, &scene, "web", "db").edge;
        let r_op = rel(&model, "operator", "web");
        let r_bank = rel(&model, "web", "bank");
        let op = scene
            .labels
            .iter()
            .position(|l| l.relationship == r_op)
            .expect("label");
        let bank = scene
            .labels
            .iter()
            .position(|l| l.relationship == r_bank)
            .expect("label");
        scene.labels[op].fit = Fit::OverLines(vec![other]);
        scene.labels[bank].fit = Fit::NoRoom;
        let report = lint(&model, &scene);
        let l23 = of_rule(&report, Rule::LabelOverLine);
        assert_eq!(l23.len(), 1);
        assert_eq!(l23[0].severity(), Severity::Warning);
        assert_eq!(l23[0].subjects[0], ModelId::Relationship(r_op));
        assert_eq!(
            l23[0].items,
            vec![ItemKey::Label(scene.labels[op].edge), ItemKey::Line(other)]
        );
        assert_eq!(
            l23[0].message,
            r#"label "Manages orders in" ("Operator" -> "Web app") covers another line"#
        );
        let l24 = of_rule(&report, Rule::LabelNoRoom);
        assert_eq!(l24.len(), 1);
        assert_eq!(l24[0].subjects, vec![ModelId::Relationship(r_bank)]);
        assert_eq!(l24[0].items, vec![ItemKey::Label(scene.labels[bank].edge)]);
        assert_eq!(
            l24[0].message,
            r#"no room for label "Charges cards through" ("Web app" -> "Bank")"#
        );
    }

    #[test]
    fn core_30_text_does_not_fit_l25() {
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        assert!(of_rule(&lint(&model, &scene), Rule::TextFit).is_empty());
        let web = box_index(&model, &scene, "web");
        if let Some(d) = scene.boxes[web].description.as_mut() {
            d.fits = false;
        }
        scene.boxes[web].name.fits = false;
        scene.labels[0].text.fits = false;
        let report = lint(&model, &scene);
        let l25 = of_rule(&report, Rule::TextFit);
        // One per box, one per label.
        assert_eq!(l25.len(), 2, "{l25:?}");
        assert!(l25.iter().all(|f| f.severity() == Severity::Warning));
        assert_eq!(l25[0].items, vec![ItemKey::Box(scene.boxes[web].id)]);
        assert_eq!(l25[0].message, r#"text of box "Web app" does not fit its box"#);
        assert_eq!(l25[1].items, vec![ItemKey::Label(scene.labels[0].edge)]);
        assert_eq!(
            l25[1].subjects,
            vec![ModelId::Relationship(scene.labels[0].relationship)]
        );
    }

    #[test]
    fn core_16_box_intrudes_boundary_l26() {
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        assert!(of_rule(&lint(&model, &scene), Rule::BoundaryIntrusion).is_empty());
        let boundary = scene.boundaries[0].rect;
        let bank = box_index(&model, &scene, "bank");
        let b = &mut scene.boxes[bank];
        let (hw, hh) = (b.rect.width() / 2, b.rect.height() / 2);
        // Overlapping the boundary's left edge by 10px.
        b.rect = Rect::from_centre(
            Point::new(boundary.min.x - hw + 10, boundary.centre().y),
            hw,
            hh,
        );
        let report = lint(&model, &scene);
        let l26 = of_rule(&report, Rule::BoundaryIntrusion);
        assert_eq!(l26.len(), 1, "{l26:?}");
        assert_eq!(
            l26[0].subjects,
            vec![
                ModelId::Element(el(&model, "bank")),
                ModelId::Element(el(&model, "shop"))
            ]
        );
        assert_eq!(
            l26[0].items,
            vec![ItemKey::Box(scene.boxes[bank].id), ItemKey::Boundary(0)]
        );
        assert_eq!(
            l26[0].message,
            r#"box "Bank" is not part of "Shop" but overlaps its boundary"#
        );
        // Touching the edge is not an overlap.
        let b = &mut scene.boxes[bank];
        b.rect = Rect::from_centre(
            Point::new(boundary.min.x - hw, boundary.centre().y),
            hw,
            hh,
        );
        assert!(of_rule(&lint(&model, &scene), Rule::BoundaryIntrusion).is_empty());
    }

    #[test]
    fn core_16_stats_rows_columns_budget() {
        // `Stats` counts as CLI-1 prints them.
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        set_centres(&mut scene, &[(0, 0), (300, 300), (600, 0), (900, 0), (900, 300)]);
        scene.crossings = 2;
        scene.options.max_crossings = 3;
        let stats = lint(&model, &scene).stats;
        assert_eq!(
            stats,
            Stats {
                boxes: 5,
                lines: 4,
                crossings: 2,
                budget: 3,
                rows: 2,
                columns: 4,
            }
        );
        // Unroutable lines are counted.
        scene.lines[0].routed = false;
        assert_eq!(lint(&model, &scene).stats.lines, 4);
    }

    #[test]
    fn core_17_crossings_over_budget_error() {
        // `crossings > max_crossings` is L4; equal to the budget is not.
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        assert_eq!(scene.crossings, 0);
        assert!(of_rule(&lint(&model, &scene), Rule::Crossings).is_empty());
        scene.crossings = 1;
        let report = lint(&model, &scene);
        let l4 = of_rule(&report, Rule::Crossings);
        assert_eq!(l4.len(), 1);
        assert_eq!(l4[0].severity(), Severity::Error);
        assert!(
            l4[0].message.starts_with(
                "1 line crossings, budget is 0: move boxes or raise maxCrossings on purpose; split the view (focus, hide); boxes involved:"
            ),
            "{}",
            l4[0].message
        );
        assert!(l4[0].hint.is_some());
        scene.options.max_crossings = 1;
        assert!(of_rule(&lint(&model, &scene), Rule::Crossings).is_empty());
    }

    #[test]
    fn core_17_crossings_taken_from_scene() {
        // L4 and `Stats::crossings` use `Scene::crossings` as given and
        // never recount (counting is geom's and route's, tested there). A
        // scene fixture with a crossing at a bend (counted once) and a
        // joined line ending on its trunk (no crossing) gives L4 and
        // `Stats::crossings` equal to `Scene::crossings`.
        let model = model_of(EXAMPLE);
        for key in ["C3-serving", "C3-startup"] {
            let mut scene = scene_of(&model, key);
            let report = lint(&model, &scene);
            assert_eq!(report.stats.crossings, scene.crossings, "{key}");
            assert_eq!(
                of_rule(&report, Rule::Crossings).len(),
                usize::from(scene.crossings > scene.options.max_crossings),
                "{key}"
            );
            // A crossing count the geometry does not show is taken as is.
            scene.crossings = 7;
            let report = lint(&model, &scene);
            assert_eq!(report.stats.crossings, 7);
            assert!(of_rule(&report, Rule::Crossings)[0].message.starts_with("7 line crossings"));
        }
    }

    #[test]
    fn core_18_off_grid_and_too_close_error() {
        // L5 for an `at` centre or `size` half-size off the 10px grid as
        // written (the scene is snapped, so L5 reads `ViewSpec`), span
        // `At::span` / `Size::span`; no L5 for a view not in
        // `Model::views`. L6 for edges 50px apart, none at 60px.
        let model = model_of(GRID);
        let mut scene = scene_of(&model, "grid");
        let view = model.view("grid").expect("view");
        let (web, db) = (el(&model, "web"), el(&model, "db"));
        let report = lint(&model, &scene);
        let l5 = of_rule(&report, Rule::OffGrid);
        assert_eq!(l5.len(), 2, "{l5:?}");
        assert_eq!(l5[0].subjects, vec![ModelId::Element(web)]);
        assert_eq!(
            l5[0].spans,
            vec![view.position_of(web).expect("at").span, model.element(web).span]
        );
        assert_eq!(
            l5[0].message,
            r#"box "Web app" centre (205, 200) is off the 10px grid"#
        );
        assert_eq!(l5[1].subjects, vec![ModelId::Element(db)]);
        assert_eq!(l5[1].spans[0], view.size_of(db).expect("size").span);
        assert_eq!(
            l5[1].message,
            r#"box "Database" size 250x140 has a half-size off the 10px grid"#
        );
        // Not written in the file: no `at` lines, no L5.
        scene.key = "default-view".to_owned();
        assert!(of_rule(&lint(&model, &scene), Rule::OffGrid).is_empty());

        // L6 on the box rects.
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        let w = scene.boxes[0].rect.width() / 2 + scene.boxes[1].rect.width() / 2;
        let far = [(0, 2000), (0, 3000), (0, 4000)];
        let mut centres = vec![(0, 0), (w + 50, 0)];
        centres.extend_from_slice(&far);
        set_centres(&mut scene, &centres);
        let report = lint(&model, &scene);
        let l6 = of_rule(&report, Rule::TooClose);
        assert_eq!(l6.len(), 1, "{l6:?}");
        let (a, b) = (&scene.boxes[0], &scene.boxes[1]);
        assert_eq!(l6[0].items, vec![ItemKey::Box(a.id), ItemKey::Box(b.id)]);
        assert_eq!(
            l6[0].subjects,
            vec![ModelId::Element(a.element), ModelId::Element(b.element)]
        );
        let (na, nb) = (
            &model.element(a.element).name.value,
            &model.element(b.element).name.value,
        );
        assert_eq!(
            l6[0].message,
            format!(r#"boxes "{na}" and "{nb}" are closer than 60px"#)
        );
        centres[1] = (w + 60, 0);
        set_centres(&mut scene, &centres);
        assert!(of_rule(&lint(&model, &scene), Rule::TooClose).is_empty());
        // Vertically too.
        centres[1] = (0, scene.boxes[0].rect.height() / 2 + scene.boxes[1].rect.height() / 2 + 59);
        set_centres(&mut scene, &centres);
        assert_eq!(of_rule(&lint(&model, &scene), Rule::TooClose).len(), 1);
    }

    #[test]
    fn core_19_c1_rejects_containers_and_protocols() {
        // L15, L16, L17, L18.
        let model = model_of(CONTEXT);
        let mut scene = scene_of(&model, "c1");
        let report = lint(&model, &scene);

        let l15 = of_rule(&report, Rule::C1Technology);
        assert_eq!(l15.len(), 1, "{l15:?}");
        assert_eq!(l15[0].items.len(), 2);
        assert_eq!(
            l15[0].message,
            r#"C1 shows a protocol/technology on a line: set "technology off""#
        );
        assert!(of_rule(&report, Rule::C1BoxKind).is_empty());
        assert!(of_rule(&report, Rule::C1TooManyBoxes).is_empty());

        let l18 = of_rule(&report, Rule::C1Protocol);
        let messages: Vec<&str> = l18.iter().map(|f| f.message.as_str()).collect();
        assert_eq!(
            messages,
            vec![
                r#"C1 box "HTTP gateway" names a protocol"#,
                r#"C1 label "Calls the REST API of" names a protocol"#,
            ]
        );
        assert_eq!(
            l18[0].subjects,
            vec![ModelId::Element(el(&model, "shop"))]
        );
        assert_eq!(
            l18[1].subjects,
            vec![ModelId::Relationship(rel(&model, "shop", "bank"))]
        );

        // `technology off` hides them: no L15.
        scene.options.technology = false;
        assert!(of_rule(&lint(&model, &scene), Rule::C1Technology).is_empty());

        // A container on a C1 view.
        let bank = box_index(&model, &scene, "bank");
        scene.boxes[bank].kind = ElementKind::Container;
        let report = lint(&model, &scene);
        let l16 = of_rule(&report, Rule::C1BoxKind);
        assert_eq!(l16.len(), 1);
        assert_eq!(l16[0].items, vec![ItemKey::Box(scene.boxes[bank].id)]);
        assert_eq!(
            l16[0].message,
            r#"C1 may only hold people and software systems, found "Bank""#
        );

        // Nine boxes is a warning, eight is not.
        let partners = |n: usize| {
            let m = (1..=n).fold(String::new(), |mut m, i| {
                let _ = writeln!(
                    m,
                    "    p{i} = softwareSystem \"Partner {i}\" \"Trades goods.\"\n    shop -> p{i} \"Orders goods from\""
                );
                m
            });
            format!(
                "workspace {{\n  model {{\n    shop = softwareSystem \"Shop\" \"Sells goods.\"\n{m}  }}\n  views {{\n    systemContext shop \"c1\" \"Partners\" {{\n      include *\n    }}\n  }}\n}}\n"
            )
        };
        let model = model_of(&partners(7));
        let scene = scene_of(&model, "c1");
        assert_eq!(scene.boxes.len(), 8);
        assert!(of_rule(&lint(&model, &scene), Rule::C1TooManyBoxes).is_empty());
        let model = model_of(&partners(8));
        let scene = scene_of(&model, "c1");
        let report = lint(&model, &scene);
        let l17 = of_rule(&report, Rule::C1TooManyBoxes);
        assert_eq!(l17.len(), 1);
        assert_eq!(l17[0].severity(), Severity::Warning);
        assert_eq!(
            l17[0].message,
            "C1 has 9 boxes: a context diagram should fit in one glance"
        );
        // Only C1 runs the C1 rules.
        let model = model_of(SHOP);
        let report = lint(&model, &scene_of(&model, "c2"));
        assert!(of_rule(&report, Rule::C1Technology).is_empty());
        assert!(of_rule(&report, Rule::C1BoxKind).is_empty());
    }

    #[test]
    fn core_19_protocol_word_whole_word_case_sensitive() {
        // L18: `HTTPS` and `HTTP/2` match; `https`, `HTTPServer` and
        // "chattiness" do not.
        for hit in [
            "Calls over HTTPS",
            "HTTP/2 stream",
            "gRPC-web",
            "SQL",
            "reads JSON",
            "API",
            "via SMTP.",
            "VAPID keys",
            "TCP",
            "REST",
        ] {
            assert!(protocol_word(hit).is_some(), "{hit}");
        }
        for miss in [
            "https",
            "HTTPServer",
            "chattiness",
            "Restores state",
            "APIs",
            "grpc",
            "SQLite",
            "",
        ] {
            assert_eq!(protocol_word(miss), None, "{miss}");
        }
        // Through lint: a box named "HTTPServer" is not L18.
        let src = CONTEXT
            .replace("HTTP gateway", "HTTPServer")
            .replace("Calls the REST API of", "Restores carts in");
        let model = model_of(&src);
        let scene = scene_of(&model, "c1");
        assert!(of_rule(&lint(&model, &scene), Rule::C1Protocol).is_empty());
    }

    #[test]
    fn core_19_c2_rejects_components() {
        // L19, and L20 as a warning.
        let model = model_of(LINKS);
        let mut scene = scene_of(&model, "c2");
        let report = lint(&model, &scene);
        assert!(of_rule(&report, Rule::C2Component).is_empty());
        // Container to container with no technology warns; a person's
        // link and a link with a technology do not.
        let l20 = of_rule(&report, Rule::C2NoTechnology);
        assert_eq!(l20.len(), 1, "{l20:?}");
        assert_eq!(l20[0].severity(), Severity::Warning);
        assert_eq!(
            l20[0].subjects,
            vec![ModelId::Relationship(rel(&model, "web", "db"))]
        );
        assert_eq!(
            l20[0].message,
            r#""Web app" -> "Database": container link without a protocol"#
        );
        // `technology off` shows none: the AMQP link warns too.
        scene.options.technology = false;
        assert_eq!(of_rule(&lint(&model, &scene), Rule::C2NoTechnology).len(), 2);

        let web = box_index(&model, &scene, "web");
        scene.boxes[web].kind = ElementKind::Component;
        let report = lint(&model, &scene);
        let l19 = of_rule(&report, Rule::C2Component);
        assert_eq!(l19.len(), 1);
        assert_eq!(l19[0].severity(), Severity::Error);
        assert_eq!(l19[0].subjects, vec![ModelId::Element(el(&model, "web"))]);
        assert_eq!(
            l19[0].message,
            r#"C2 must not show components, found "Web app""#
        );
    }

    #[test]
    fn core_19_c3_needs_scope_component() {
        // L21.
        let model = model_of(COMPONENTS);
        let mut scene = scene_of(&model, "c3");
        assert!(of_rule(&lint(&model, &scene), Rule::C3NoScopeComponent).is_empty());
        // The same boxes on a view of another container.
        let store = el(&model, "store");
        let ViewScope::Element(mut scope) = scene.scope.clone() else {
            unreachable!("phase-1 scoped view")
        };
        scope.value = store;
        scene.scope = ViewScope::Element(scope);
        let report = lint(&model, &scene);
        let l21 = of_rule(&report, Rule::C3NoScopeComponent);
        assert_eq!(l21.len(), 1);
        assert_eq!(l21[0].severity(), Severity::Error);
        assert_eq!(l21[0].subjects, vec![ModelId::Element(store)]);
        assert_eq!(l21[0].message, "C3 shows no component of its scope container");
        // A child of the scope counts only as a component box.
        let front = el(&model, "front");
        let mut scene = scene_of(&model, "c3");
        for b in &mut scene.boxes {
            if model.element(b.element).parent == Some(front) {
                b.kind = ElementKind::Container;
            }
        }
        assert_eq!(of_rule(&lint(&model, &scene), Rule::C3NoScopeComponent).len(), 1);
        // Only C3 runs it.
        scene.kind = ViewKind::Container;
        assert!(of_rule(&lint(&model, &scene), Rule::C3NoScopeComponent).is_empty());
    }

    #[test]
    fn core_20_name_description_technology_intent() {
        // L9, L10, L11 (over 110 characters, counted as chars), L12, L13.
        // L12 comes from the model: a joined line with the same label as
        // its trunk (CORE-14) and an unroutable line have no `SceneLabel`
        // and are not L12; a relationship with an empty description is.
        let model = model_of(&names_src());
        let mut scene = scene_of(&model, "c2");
        let report = lint(&model, &scene);
        let names = |rule| -> Vec<String> {
            of_rule(&report, rule)
                .iter()
                .map(|f| f.message.clone())
                .collect()
        };
        assert_eq!(
            names(Rule::NameOrDescription),
            vec![r#""Worker" needs a name and a description"#]
        );
        assert_eq!(names(Rule::NoTechnology), vec![r#""Worker" needs a technology"#]);
        // 111 ASCII chars warn; 110 two-byte chars do not.
        assert_eq!(
            names(Rule::LongDescription),
            vec![r#""Database" description is over 110 characters"#]
        );
        assert_eq!(names(Rule::NoLabel), vec![r#""Web app" -> "Database" has no label"#]);
        assert_eq!(
            of_rule(&report, Rule::NoLabel)[0].subjects,
            vec![ModelId::Relationship(rel(&model, "web", "db"))]
        );
        assert_eq!(
            names(Rule::VagueLabel),
            vec![r#""Operator" -> "Web app": label " Uses " says nothing, state the intent"#]
        );
        // A person and a system need no technology.
        assert!(
            of_rule(&report, Rule::NoTechnology)
                .iter()
                .all(|f| f.subjects == vec![ModelId::Element(el(&model, "worker"))])
        );

        // L12 reads the model: no `SceneLabel` (joined to a trunk with the
        // same label, or unroutable) is not L12.
        scene.labels.clear();
        let i = line_index(&model, &scene, "worker", "db");
        scene.lines[i].routed = false;
        let report = lint(&model, &scene);
        assert_eq!(of_rule(&report, Rule::NoLabel).len(), 1);

        // On the example's bus views, joined lines share their trunk's
        // label and are still not L12.
        let model = model_of(EXAMPLE);
        let scene = scene_of(&model, "C3-serving");
        assert!(scene.lines.iter().any(|l| l.joins.is_some()));
        assert!(of_rule(&lint(&model, &scene), Rule::NoLabel).is_empty());
    }

    #[test]
    fn core_20_stop_word_label_only() {
        // Review Focus: "uses" alone is L13; " Uses " trimmed and
        // lowercased too; "uses the API to fetch" is not.
        for stop in ["uses", " Uses ", "USE", "calls", "Sends", "reads", "writes\t"] {
            assert!(is_stop_word(stop), "{stop:?}");
        }
        for intent in ["uses the API to fetch", "Reads orders from", "", "user", "called by"] {
            assert!(!is_stop_word(intent), "{intent:?}");
        }
        let model = model_of(&names_src());
        let scene = scene_of(&model, "c2");
        let report = lint(&model, &scene);
        let l13 = of_rule(&report, Rule::VagueLabel);
        assert_eq!(l13.len(), 1);
        assert_eq!(
            l13[0].subjects,
            vec![ModelId::Relationship(rel(&model, "operator", "web"))]
        );
    }

    #[test]
    fn core_21_relationship_on_no_view_error() {
        // L22 once per relationship, `view: None`, message as CLI-1.
        let model = model_of(COVER);
        let (front, store) = (scene_of(&model, "front"), scene_of(&model, "store"));
        let r = |s, d| rel(&model, s, d);
        let (browse, fill, write, report) = (
            r("operator", "pages"),
            r("pages", "cart"),
            r("cart", "orders"),
            r("orders", "bank"),
        );
        // cart -> orders and operator -> pages on no view.
        let f = drawing(&front, &[&[fill]]);
        let s = drawing(&store, &[&[report]]);
        let found = coverage(&model, &[&f, &s]);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|x| x.rule == Rule::Coverage));
        assert!(found.iter().all(|x| x.view.is_none()));
        assert!(found.iter().all(|x| x.items.is_empty()));
        assert!(found.iter().all(|x| x.severity() == Severity::Error));
        // Ordered by relationship id.
        assert_eq!(
            found[0].message,
            r#"Operator -> Pages is in the model but on no component view of "Front""#
        );
        assert_eq!(
            found[1].message,
            r#"Cart -> Orders is in the model but on no component view of "Front""#
        );
        assert_eq!(
            found[1].subjects,
            vec![
                ModelId::Relationship(write),
                ModelId::Element(el(&model, "front"))
            ]
        );
        assert_eq!(found[1].spans, vec![model.relationship(write).span]);
        assert!(found.iter().all(|x| x.subjects[0] != ModelId::Relationship(browse)
            || x.message.starts_with("Operator")));

        // Everything drawn somewhere: nothing to report.
        let f = drawing(&front, &[&[browse], &[fill], &[write]]);
        let s = drawing(&store, &[&[write], &[report]]);
        assert!(coverage(&model, &[&f, &s]).is_empty());

        // The example: the two C3 views together show every component
        // relationship.
        let model = model_of(EXAMPLE);
        let serving = scene_of(&model, "C3-serving");
        let startup = scene_of(&model, "C3-startup");
        let found = coverage(&model, &[&serving, &startup]);
        assert!(found.is_empty(), "{found:?}");
        // Without the startup view's lines, cli's relationships fail.
        let bare = drawing(&startup, &[&[serving.lines[0].relationship]]);
        let found = coverage(&model, &[&serving, &bare]);
        assert!(
            found
                .iter()
                .any(|x| x.message.starts_with("cli -> web is in the model")),
            "{found:?}"
        );
    }

    #[test]
    fn core_21_coverage_same_scope_only() {
        // Review Focus: a relationship drawn only on another container's
        // component view still fails; a container with no component view
        // is not checked; a lifted line covers the relationships it
        // stands for. A relationship between components of two checked
        // containers that neither covers is one L22, `<scope>` the
        // source's container; covered by the source's only, `<scope>` the
        // destination's (CORE-21). A container with a component
        // view missing from `scenes` is skipped.
        let model = model_of(COVER);
        let (front, store) = (scene_of(&model, "front"), scene_of(&model, "store"));
        let r = |s, d| rel(&model, s, d);
        let (browse, fill, write, report) = (
            r("operator", "pages"),
            r("pages", "cart"),
            r("cart", "orders"),
            r("orders", "bank"),
        );
        let scopes = |found: &[Finding]| -> Vec<String> {
            found.iter().map(|x| x.message.clone()).collect()
        };

        // Covered by the source's container only: the destination's fails.
        let f = drawing(&front, &[&[browse], &[fill], &[write]]);
        let s = drawing(&store, &[&[report]]);
        assert_eq!(
            scopes(&coverage(&model, &[&f, &s])),
            vec![r#"Cart -> Orders is in the model but on no component view of "Store""#]
        );
        // Covered by the destination's only: the source's fails.
        let f = drawing(&front, &[&[browse], &[fill]]);
        let s = drawing(&store, &[&[write], &[report]]);
        assert_eq!(
            scopes(&coverage(&model, &[&f, &s])),
            vec![r#"Cart -> Orders is in the model but on no component view of "Front""#]
        );
        // Neither: one finding, the source's container.
        let f = drawing(&front, &[&[browse], &[fill]]);
        let s = drawing(&store, &[&[report]]);
        assert_eq!(
            scopes(&coverage(&model, &[&f, &s])),
            vec![r#"Cart -> Orders is in the model but on no component view of "Front""#]
        );

        // Drawn only on another container's component view still fails.
        let f = drawing(&front, &[&[fill], &[write]]);
        let s = drawing(&store, &[&[browse], &[write], &[report]]);
        assert_eq!(
            scopes(&coverage(&model, &[&f, &s])),
            vec![r#"Operator -> Pages is in the model but on no component view of "Front""#]
        );
        // Scenes of another kind do not count.
        let mut other = drawing(&front, &[&[browse]]);
        other.kind = ViewKind::Container;
        other.key = "other".to_owned();
        assert_eq!(coverage(&model, &[&f, &s, &other]).len(), 1);

        // A lifted (merged) line covers every relationship it stands for.
        let f = drawing(&front, &[&[fill, browse], &[write]]);
        let s = drawing(&store, &[&[write], &[report]]);
        assert!(coverage(&model, &[&f, &s]).is_empty());

        // The api container has no component view: handler -> bank is
        // never checked, even with nothing drawn.
        let f = drawing(&front, &[&[fill]]);
        let s = drawing(&store, &[&[report]]);
        let handler = r("handler", "bank");
        assert!(
            coverage(&model, &[&f, &s])
                .iter()
                .all(|x| x.subjects[0] != ModelId::Relationship(handler))
        );

        // The store's view missing from the slice: the store is skipped
        // (orders -> bank is not reported), the front is still checked
        // and cart -> orders fails only there.
        let f = drawing(&front, &[&[browse], &[fill]]);
        assert_eq!(
            scopes(&coverage(&model, &[&f])),
            vec![r#"Cart -> Orders is in the model but on no component view of "Front""#]
        );
        let f = drawing(&front, &[&[browse], &[fill], &[write]]);
        assert!(coverage(&model, &[&f]).is_empty());
        assert!(coverage(&model, &[]).is_empty());
    }

    #[test]
    fn core_26_split_hint_names_boxes() {
        // The L4 finding carries the boxes and relationships of the
        // crossing lines (`Scene::crossing_pairs`) and names them.
        let model = model_of(SHOP);
        let mut scene = scene_of(&model, "c2");
        scene.labels.clear();
        let far = scene.bounds.max.x + 1000;
        // Park every line apart, then cross two of them once.
        for (k, l) in scene.lines.iter_mut().enumerate() {
            let y = 1000 * i32::try_from(k).expect("small");
            l.points = vec![Point::new(far, y), Point::new(far + 100, y)];
        }
        let a = line_index(&model, &scene, "operator", "web");
        let b = line_index(&model, &scene, "worker", "db");
        let ya = scene.lines[a].points[0].y;
        scene.lines[b].points = vec![
            Point::new(far + 50, ya - 50),
            Point::new(far + 50, ya + 50),
        ];
        let pairs = scene.crossing_pairs();
        assert_eq!(pairs.len(), 1);
        scene.crossings = pairs[0].2;
        let report = lint(&model, &scene);
        let l4 = of_rule(&report, Rule::Crossings);
        assert_eq!(l4.len(), 1);
        let hint = l4[0].hint.as_ref().expect("split hint");
        // Box order: the four ends sorted by box id.
        let mut ends: Vec<usize> = ["operator", "web", "worker", "db"]
            .iter()
            .map(|k| box_index(&model, &scene, k))
            .collect();
        ends.sort_unstable();
        let want: Vec<ElementId> = ends.iter().map(|&i| scene.boxes[i].element).collect();
        assert_eq!(hint.boxes, want);
        let (la, lb) = (&scene.lines[a], &scene.lines[b]);
        let mut rels = [(la.edge, la.relationship), (lb.edge, lb.relationship)];
        rels.sort_unstable();
        assert_eq!(
            hint.relationships,
            rels.iter().map(|&(_, r)| r).collect::<Vec<_>>()
        );
        assert_eq!(
            l4[0].items,
            rels.iter().map(|&(e, _)| ItemKey::Line(e)).collect::<Vec<_>>()
        );
        let names = want
            .iter()
            .map(|&e| format!("\"{}\"", model.element(e).name.value))
            .collect::<Vec<_>>()
            .join(", ");
        assert!(
            l4[0].message.ends_with(&format!(
                "split the view (focus, hide); boxes involved: {names}"
            )),
            "{}",
            l4[0].message
        );
        assert!(l4[0].message.starts_with("1 line crossings, budget is 0"));
        // The subjects are the boxes, then the relationships.
        assert_eq!(l4[0].subjects.len(), 6);
        assert_eq!(l4[0].subjects[0], ModelId::Element(want[0]));
        // No other finding carries a hint.
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.hint.is_none() || f.rule == Rule::Crossings)
        );
    }
}
