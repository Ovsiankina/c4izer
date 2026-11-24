//! Typed model built from the syntax tree: elements, relationships with
//! their implicit tags, element styles, and view specs (kind, scope, key,
//! description, title, subtitle, options, `autoLayout`, `at` and `size`
//! lines). Each item keeps the span it came from. Validates ids
//! (DSL-1..8, DSL-14, DSL-15).
//!
//! Depends on: `text`.

// Shape of the model: plain data with pub fields, read by `edit`, `view`,
// `lint` and `pipeline`. Elements and relationships live in `Vec`s in
// source order; `ElementId` and `RelationshipId` are indices into them,
// so ids are stable for one parse and ordering is deterministic
// (CORE-25). No `HashMap`/`HashSet` anywhere in this API.
//
// Every value `edit` may rewrite is a `Spanned<T>`: the parsed value plus
// the bytes it was written as (DSL-9, DSL-10, DSL-13). Spans carry the
// source-file id (DSL-16), always `FileId::ROOT` in phase 1.
//
// Read-only data types are `#[non_exhaustive]` so phase 2 and 3 (groups,
// relationship styles, every style property, `!include`, landscape,
// dynamic and deployment views, view-local keys) add fields and variants
// without breaking callers. Closed sets (`Colour`, the id
// newtypes) stay exhaustive.
//
// DECISION: keywords match case-insensitively (SRS 4.1), and so do
// element and relationship ids (Structurizr; owner ruling, phase2-decisions
// Part C, A.3 item 4); tags and view keys match exactly as written.
// DECISION: when a value is given both positionally and by a body line
// (`description`, `technology`), the later one in the file wins and is
// the literal kept, so `edit` rewrites the bytes that take effect.
// DECISION: colours are `#rrggbb` only, hex digits of either case (SRS
// 4.1 keyword table); `#rgb` and colour names are `InvalidValue` until
// P2-10.
//
// Identifier rules (DSL-7, P2-6, P2-142..P2-146): an element may have no
// `<id> =` (`Element::key` is `None`) and a relationship may have one
// (`r = a -> b`). Element, relationship and deployment ids share one
// register. Under flat ids (the default) a dotted `<id>` `a.b.c` names the
// element keyed `c` whose parent is keyed `b` inside `a` (`Model::find_written`);
// under `!identifiers hierarchical` an element's full id is its parent's
// full id, `.`, its own id (`Model::full_key`, an `ElementKey`), and relationship ids stay
// global. A view with no key gets
// `<Kind>-NNN` (`SystemContext-001`), numbered per kind in declaration
// order. `relationship "<tag>" { ... }` blocks in `styles` are parsed
// into `Model::relationship_styles` (P2-12).
//
// DSL-15: styles cascade per property in the element's tag order
// (`Element`, the kind tag, then the declared tags); a later tag wins,
// and when the same tag has two style blocks the later block wins.

use std::cell::RefCell;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use std::collections::{BTreeMap, BTreeSet};

mod theme;
pub(crate) use theme::sniff as sniff_image;
mod archetype;
mod deploy;
mod expr;
mod json;
mod load;

use crate::text::{
    self, FileId, ParseError, ParseErrorKind, Position, Span, Statement, SyntaxTree, TokenKind,
};

/// Index of an element in `Model::elements`. Stable for one parse; later
/// stages tag boxes with it (CORE-1, EDIT hit-testing). Serves DSL-1.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementId(pub u32);

/// Index of a relationship in `Model::relationships`. Stable for one
/// parse; later stages tag lines with it. Serves DSL-2.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelationshipId(pub u32);

/// The canonical key of an element, the one `Model::full_key` gives and
/// `Model::find` resolves: the dotted full id under `!identifiers
/// hierarchical` (`shop.web`), the id under flat ids. Edits and the
/// editor's selection name an element by it, so the name still resolves
/// in a newer text (EDIT-24). It is not the declaration id
/// `Element::key`: under hierarchical ids a nested element's bare id does
/// not resolve (P2-142). Text from a file, the command line or JSON is
/// resolved with `Model::find_written`; `ElementKey::from_written` takes
/// text that already is a canonical key. Serves P2-6, P2-142, EDIT-24.
// APPROVER: new pub item (owner, 2026-11-23: a bare declaration id must
// not compile where a canonical key is expected).
// DECISION: a newtype over `String` with no `From<&str>` or
// `From<String>`, so every conversion from text is the explicit,
// greppable `from_written`; `Ord` is the string order, so sorted
// selections and maps keep their order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementKey(String);

impl ElementKey {
    /// `text` taken as a canonical key as is, unchecked: a key read back
    /// from text written from one (saved UI state, a line `edit` wrote),
    /// or a test's literal. Text that still has to be resolved against a
    /// model (a dotted path under flat ids, another case) goes through
    /// `Model::find_written` and `Model::full_key`. Serves P2-142.
    #[must_use]
    pub fn from_written(text: impl Into<String>) -> ElementKey {
        ElementKey(text.into())
    }

    /// The key as text, as written in the file.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key as owned text.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ElementKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A parsed value and the bytes it was written as, so `edit` can rewrite
/// exactly those bytes (DSL-9, DSL-10, DSL-13). For a string literal the
/// span covers the quotes. The span names its file (DSL-16).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Spanned<T> {
    /// The value, already unquoted or converted.
    pub value: T,
    /// The token(s) it came from.
    pub span: Span,
}

/// A string literal: its value (unquoted) and its span (quotes
/// included). Serves DSL-1, DSL-2, DSL-10.
pub type Literal = Spanned<String>;

/// The element kinds of phase 1. Phase 2 and 3 add custom elements and
/// deployment kinds. Serves DSL-1.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ElementKind {
    /// `person`
    Person,
    /// `softwareSystem`
    SoftwareSystem,
    /// `container`, only inside a software system.
    Container,
    /// `component`, only inside a container.
    Component,
    /// `element <name> [metadata] [description] [tags]`: a custom element,
    /// declared in `model` (P2-170). Keyword `element`, default tag
    /// `Element` only. Serves P2-170, P2-155, P2-181.
    Custom,
    /// `deploymentNode`, in a deployment environment or another
    /// deployment node (P3-101). Drawn as a boundary (P3-114). Serves
    /// P3-1, P3-101.
    DeploymentNode,
    /// `infrastructureNode`, in a deployment node (P3-103). Serves P3-1,
    /// P3-103.
    InfrastructureNode,
    /// `softwareSystemInstance <system>` (or `instanceOf` a system), in a
    /// deployment node (P3-104). Serves P3-1, P3-104.
    SoftwareSystemInstance,
    /// `containerInstance <container>` (or `instanceOf` a container), in
    /// a deployment node (P3-104). Serves P3-1, P3-104.
    ContainerInstance,
}

impl ElementKind {
    /// The DSL keyword (`person`, `softwareSystem`, ...). Serves DSL-1,
    /// DSL-4 (error text).
    pub const fn keyword(self) -> &'static str {
        match self {
            ElementKind::Person => "person",
            ElementKind::SoftwareSystem => "softwareSystem",
            ElementKind::Container => "container",
            ElementKind::Component => "component",
            ElementKind::Custom => "element",
            ElementKind::DeploymentNode => "deploymentNode",
            ElementKind::InfrastructureNode => "infrastructureNode",
            ElementKind::SoftwareSystemInstance => "softwareSystemInstance",
            ElementKind::ContainerInstance => "containerInstance",
        }
    }

    /// The implicit kind tag (`Person`, `Software System`, `Container`,
    /// `Component`); every element also gets `Element` before it.
    /// Serves DSL-15.
    pub const fn default_tag(self) -> &'static str {
        match self {
            ElementKind::Person => "Person",
            ElementKind::SoftwareSystem => "Software System",
            ElementKind::Container => "Container",
            ElementKind::Component => "Component",
            // DECISION: a custom element's kind tag is `Element` itself
            // (A.2: default_tag "Element"); P2-170 keeps only the implicit
            // tag `Element`, so the builder must not add it twice.
            ElementKind::Custom => "Element",
            ElementKind::DeploymentNode => "Deployment Node",
            ElementKind::InfrastructureNode => "Infrastructure Node",
            ElementKind::SoftwareSystemInstance => "Software System Instance",
            ElementKind::ContainerInstance => "Container Instance",
        }
    }
}

/// One element. Serves DSL-1, DSL-15.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Element {
    /// Its own index in `Model::elements`.
    pub id: ElementId,
    /// The declaration id as written (`app` in `app = container ...`),
    /// without its parents: not a lookup key. Under `!identifiers
    /// hierarchical` a nested element's bare id does not resolve; name an
    /// element with `Model::full_key` (`ElementKey`). `None` for an
    /// element declared without `<id> =` (P2-145). Serves DSL-5, DSL-7.
    pub key: Option<Spanned<String>>,
    /// Person, software system, container or component.
    pub kind: ElementKind,
    /// The enclosing element, `None` at top level.
    pub parent: Option<ElementId>,
    /// Name literal (DSL-10 rewrites it).
    pub name: Literal,
    /// Description literal, positional or from a `description` line, if
    /// written.
    pub description: Option<Literal>,
    /// Technology literal, positional or from a `technology` line, if
    /// written. Persons and software systems have none.
    pub technology: Option<Literal>,
    /// Effective tags in order: `Element`, the kind tag, then the
    /// declared tags as written, trimmed (DSL-15). Style resolution reads
    /// this order.
    pub tags: Vec<String>,
    /// Every declared tags literal (positional, then `tags` lines), in
    /// file order; `edit` adds a tag through the block (DSL-13).
    pub declared_tags: Vec<Literal>,
    /// The whole statement, block included.
    pub span: Span,
    /// The innermost `group` the element was declared in, or the group a
    /// `group "<name>"` line in a component block names. Serves P2-4,
    /// P2-127, P2-128, P2-129.
    pub group: Option<GroupId>,
    /// `url <url>` (P2-171).
    pub url: Option<Literal>,
    /// `properties { ... }` pairs in file order (P2-172).
    pub properties: Vec<Property>,
    /// `perspectives { ... }`, either form, in file order (P2-173).
    pub perspectives: Vec<Perspective>,
    /// A custom element's metadata argument, and an archetype's metadata
    /// default (P2-170, P2-181); `None` for every other kind.
    pub metadata: Option<Literal>,
    /// `!docs` / `!adrs` / `!decisions` on this element, in file order
    /// (P2-159, P2-160).
    pub docs: Vec<Attachment>,
    /// The deployment environment of a deployment node, infrastructure
    /// node or instance; `None` for every other kind. Serves P3-1,
    /// P3-101, P3-103, P3-104.
    pub environment: Option<EnvironmentId>,
    /// The element an instance is an instance of (a software system or a
    /// container); `None` for every other kind. Serves P3-104.
    pub instance_of: Option<ElementId>,
    /// An instance's number among the instances of the same element in
    /// the same environment, from 1 in declaration order; `None` for
    /// every other kind. Serves P3-104, P3-117.
    pub instance_number: Option<u32>,
    /// A deployment node's `instances` value as written (`2`, `1..3`);
    /// `None` when not written (it then counts as `1`). Serves P3-102,
    /// P3-115.
    pub instances: Option<Literal>,
    /// The names of an instance's deployment groups, in the order
    /// written; empty for an instance in the default group and for every
    /// other kind. Serves P3-104, P3-106.
    pub deployment_groups: Vec<String>,
}

impl Element {
    /// True when the element carries the `External` tag (CORE-2: drawn as
    /// `External Software System`). Serves DSL-1.
    pub fn is_external(&self) -> bool {
        self.has_tag("External")
    }

    /// True when `tags` contains `tag`, implicit tags included. Serves
    /// DSL-15.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

/// One relationship `source -> destination "description" "technology"
/// "tags"`, declared at top level or inside an element block (where
/// `this` or an omitted source means the enclosing element). Serves
/// DSL-2, DSL-15.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Relationship {
    /// Its own index in `Model::relationships`. Valid for one parse
    /// only; across parses use `Model::find_relationship` (EDIT-24).
    pub id: RelationshipId,
    /// The identifier, for `r = a -> b ...`. Always `None` in phase 1:
    /// a named relationship is a `LaterPhase` error (DSL-7); phase 2
    /// fills it.
    pub key: Option<Spanned<String>>,
    /// Source element, as written: the id token, `this`, or for an
    /// implied source (`-> b` in an element block) the span of the `->`
    /// token, so DSL-10 inserts land next to it. May be declared later
    /// in the file than the relationship (forward reference).
    pub source: Spanned<ElementId>,
    /// Destination element, as written.
    pub destination: Spanned<ElementId>,
    /// Description literal, if written (DSL-10 rewrites it).
    pub description: Option<Literal>,
    /// Technology literal, if written.
    pub technology: Option<Literal>,
    /// Effective tags: `Relationship`, then the declared tags (DSL-15).
    pub tags: Vec<String>,
    /// Every declared tags literal, in file order.
    pub declared_tags: Vec<Literal>,
    /// The whole statement, block included.
    pub span: Span,
    /// `url <url>` (P2-171).
    pub url: Option<Literal>,
    /// `properties { ... }` pairs in file order (P2-172).
    pub properties: Vec<Property>,
    /// `perspectives { ... }`, either form, in file order (P2-173).
    pub perspectives: Vec<Perspective>,
    /// The `!impliedRelationships` strategy in force where this
    /// relationship was declared: a directive applies to the
    /// relationships after it only, as in Structurizr. Lifting (CORE-5)
    /// and `include *` read it per relationship. Approved pub addition
    /// (2026-11-17). Serves P2-7, P2-153, P2-154.
    pub implied_relationships: ImpliedRelationships,
    /// For a relationship copied between two instances (P3-106), the
    /// model relationship it copies; `None` for a written one. Serves
    /// P3-106, P3-118.
    pub copied_from: Option<RelationshipId>,
}

/// Index of a deployment environment in `Model::environments`. Stable
/// for one parse. Serves P3-1, P3-100.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnvironmentId(pub u32);

/// One `deploymentEnvironment`. Serves P3-1, P3-100.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Environment {
    /// Its own index in `Model::environments`.
    pub id: EnvironmentId,
    /// The identifier written before `=`, if any.
    pub key: Option<Spanned<String>>,
    /// Its name literal.
    pub name: Literal,
    /// The whole statement, block included.
    pub span: Span,
}

/// A box shape: the 19 shapes of Structurizr's notation page, parsed
/// in any letter case. Serves DSL-3, CORE-2, P2-11.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum Shape {
    /// Plain box, the default.
    #[default]
    Box,
    /// `shape Person`.
    Person,
    /// `shape Cylinder`.
    Cylinder,
    /// `shape RoundedBox`: a box with a larger corner radius than `Box`
    /// (P2-415).
    RoundedBox,
    /// `shape Circle`.
    Circle,
    /// `shape Ellipse`.
    Ellipse,
    /// `shape Hexagon`.
    Hexagon,
    /// `shape Diamond`.
    Diamond,
    /// `shape Bucket`.
    Bucket,
    /// `shape Pipe`: a horizontal cylinder.
    Pipe,
    /// `shape Robot`.
    Robot,
    /// `shape Folder`.
    Folder,
    /// `shape WebBrowser`.
    WebBrowser,
    /// `shape Window`.
    Window,
    /// `shape Terminal`.
    Terminal,
    /// `shape Shell`.
    Shell,
    /// `shape MobileDevicePortrait`.
    MobileDevicePortrait,
    /// `shape MobileDeviceLandscape`.
    MobileDeviceLandscape,
    /// `shape Component`.
    Component,
}

impl Shape {
    /// The reference's spelling (`"MobileDevicePortrait"`), which `edit`
    /// writes (DSL-13) and the style panel shows. Serves P2-11, P2-417.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Shape::Box => "Box",
            Shape::Person => "Person",
            Shape::Cylinder => "Cylinder",
            Shape::RoundedBox => "RoundedBox",
            Shape::Circle => "Circle",
            Shape::Ellipse => "Ellipse",
            Shape::Hexagon => "Hexagon",
            Shape::Diamond => "Diamond",
            Shape::Bucket => "Bucket",
            Shape::Pipe => "Pipe",
            Shape::Robot => "Robot",
            Shape::Folder => "Folder",
            Shape::WebBrowser => "WebBrowser",
            Shape::Window => "Window",
            Shape::Terminal => "Terminal",
            Shape::Shell => "Shell",
            Shape::MobileDevicePortrait => "MobileDevicePortrait",
            Shape::MobileDeviceLandscape => "MobileDeviceLandscape",
            Shape::Component => "Component",
        }
    }

    /// Every shape, in declaration order.
    const ALL: [Shape; 19] = [
        Shape::Box,
        Shape::Person,
        Shape::Cylinder,
        Shape::RoundedBox,
        Shape::Circle,
        Shape::Ellipse,
        Shape::Hexagon,
        Shape::Diamond,
        Shape::Bucket,
        Shape::Pipe,
        Shape::Robot,
        Shape::Folder,
        Shape::WebBrowser,
        Shape::Window,
        Shape::Terminal,
        Shape::Shell,
        Shape::MobileDevicePortrait,
        Shape::MobileDeviceLandscape,
        Shape::Component,
    ];
}

/// A `#rrggbb` colour. Serves DSL-3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Colour {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl fmt::Display for Colour {
    /// `#rrggbb`, lowercase, as `edit` writes it (DSL-13) and the scene
    /// draws it (DSL-3).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// How a box outline or a line is stroked: `border` on an element style,
/// `style` (and `dashed true|false`) on a relationship style. Serves
/// P2-405, P2-420.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Border {
    /// No dash array; the default for boxes and, unlike Structurizr, for
    /// lines (CORE-10, B styles-themes 2).
    #[default]
    Solid,
    /// A dash.
    Dashed,
    /// A dash shorter than `Dashed`'s.
    Dotted,
}

/// Light or dark: which `light { }` / `dark { }` style blocks apply, and
/// the colour defaults a view is drawn with. Serves P2-21, P2-430..P2-433.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum Mode {
    /// The default (Structurizr `-mode light`).
    #[default]
    Light,
    /// `dark`.
    Dark,
}

/// A relationship style's or per-view line's `routing`. Parsed and kept;
/// every value draws with c4rizer's router. Serves P2-421 (and
/// editor-breadth P2-709, O8).
// TODO(srs): P2-421, what each value means under the circuit-board
// router. Reading taken (B styles-themes 1): parsed and stored, every
// value draws with c4rizer's router (CORE-6..9), no notice.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Routing {
    /// `routing Direct`.
    Direct,
    /// `routing Orthogonal`.
    Orthogonal,
    /// `routing Curved`.
    Curved,
}

/// One `element "<tag>" { ... }` block in the `styles` block of `views`.
/// Each property keeps the span of its value so `edit` rewrites only that
/// value (DSL-13). Phase 2 adds the other style properties (P2-10).
/// Serves DSL-3.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ElementStyle {
    /// The tag this style applies to.
    pub tag: Literal,
    /// `background #rrggbb`.
    pub background: Option<Spanned<Colour>>,
    /// `color #rrggbb` (text colour).
    pub color: Option<Spanned<Colour>>,
    /// `shape Person|Cylinder|Box`.
    pub shape: Option<Spanned<Shape>>,
    /// The whole `element` statement, block included.
    pub span: Span,
    /// `stroke <colour>`: the box outline colour (P2-403).
    pub stroke: Option<Spanned<Colour>>,
    /// `strokeWidth N`, 1..10 (P2-404).
    // TODO(srs): P2-404/P2-406/P2-425/P2-426 say an out-of-range value
    // is an invalid-value error; B styles-themes 13 [I] (Structurizr for
    // Java's setters) clamps to the range with no error. Reading taken:
    // clamp, the Structurizr-faithful one. Checked against Structurizr
    // 6.2.4, the verified reference (approver ruling 2026-11-17):
    // `ElementStyle.setStrokeWidth` clamps to 1..10, `setOpacity` and
    // `RelationshipStyle.setOpacity`/`setPosition` to 0..100, and the DSL
    // parsers call them with `Integer.parseInt`, a non-number being the
    // only error. Same as here.
    pub stroke_width: Option<Spanned<u8>>,
    /// `width W` in px (P2-402).
    // P2-402 (approver ruling 2026-11-17): literal px; a value that is
    // not a multiple of 20 is rounded up to the next multiple of 20.
    pub width: Option<Spanned<i32>>,
    /// `height H` in px (P2-402).
    pub height: Option<Spanned<i32>>,
    /// `fontSize N`: the box name's px size (P2-407).
    pub font_size: Option<Spanned<u32>>,
    /// `border solid|dashed|dotted` (P2-405).
    pub border: Option<Spanned<Border>>,
    /// `opacity N`, 0..100 (P2-406).
    pub opacity: Option<Spanned<u8>>,
    /// `metadata true|false`: the `[type: technology]` line (P2-408).
    pub metadata: Option<Spanned<bool>>,
    /// `description true|false` (P2-408).
    pub description: Option<Spanned<bool>>,
    /// `icon <file>` as written (P2-409); the bytes are in
    /// `ResolvedStyle::icon` and `Model::assets`.
    pub icon: Option<Literal>,
    /// `properties { name value }`, kept byte for byte, no effect
    /// (P2-410).
    pub properties: Vec<Property>,
    /// `Some` when the block sits in `light { }` or `dark { }`, `None`
    /// for a top-level block (P2-21, P2-430).
    pub mode: Option<Mode>,
    /// `Some(i)` for a style read from `Model::themes[i]`, `None` for the
    /// file's own block; `edit` never edits a theme style (P2-13,
    /// P2-439). A theme style's `span` is its `theme` statement's.
    pub theme: Option<usize>,
}

/// The style an element ends up with: each property from the last of its
/// tags that sets it, else the keyword-table default for its kind
/// (`ResolvedStyle::default_for`). Serves DSL-3, DSL-15, CORE-2.
// DECISION: Clone but not Copy: phase 2 adds heap data (icon, font).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedStyle {
    /// Box fill.
    pub background: Colour,
    /// Box text colour.
    pub color: Colour,
    /// Box shape.
    pub shape: Shape,
    /// Outline colour; `None`: the fill darkened to 72% (P2-403).
    pub stroke: Option<Colour>,
    /// Outline width in px; 1 by default, as in phase 1 (P2-404).
    pub stroke_width: u8,
    /// Box width in px replacing the kind default (P2-402).
    pub width: Option<i32>,
    /// Box height in px replacing the kind default (P2-402).
    pub height: Option<i32>,
    /// Name size in px; `None`: the phase-1 14px (P2-407).
    // TODO(srs): P2-407, how the type line and description scale. Reading
    // taken (B styles-themes 5): name N px on an N+6 px pitch; type line
    // and description round(N x 11/14) px on that size plus 4.
    pub font_size: Option<u32>,
    /// Outline dash (P2-405).
    pub border: Border,
    /// 0..100; 100 by default (P2-406).
    pub opacity: u8,
    /// Draw the `[type: technology]` line (P2-408).
    pub metadata: bool,
    /// Draw the description (P2-408).
    pub description: bool,
    /// The icon file's bytes (P2-409, P2-428, O3).
    // TODO(srs): P2-409. Readings taken (B styles-themes 7): drawn centred
    // in the box's bottom band, at most 1/4 of the box height, clear of
    // the texts; PNG, JPEG and SVG local files and `data:` URIs accepted;
    // a URL icon is not drawn and gets a P2-3 notice; a missing or
    // unreadable file is a `FileRead` parse error naming the path.
    pub icon: Option<Asset>,
}

impl ResolvedStyle {
    /// The keyword-table defaults for an element of `kind`: background
    /// `#dddddd`, colour `#000000`, shape `Person` for a person, else
    /// `Box`. Serves DSL-15, CORE-2, CORE-9.
    pub const fn default_for(kind: ElementKind) -> ResolvedStyle {
        ResolvedStyle {
            background: Colour {
                r: 0xdd,
                g: 0xdd,
                b: 0xdd,
            },
            color: Colour { r: 0, g: 0, b: 0 },
            shape: match kind {
                ElementKind::Person => Shape::Person,
                _ => Shape::Box,
            },
            // DECISION: the phase-2 properties default to their phase-1
            // look (P2-437), so phase-1 files draw unchanged.
            stroke: None,
            stroke_width: 1,
            width: None,
            height: None,
            font_size: None,
            border: Border::Solid,
            opacity: 100,
            metadata: true,
            description: true,
            icon: None,
        }
    }
}

/// One `relationship "<tag>" { ... }` block, top-level in `styles` or in
/// `light`/`dark`, or read from a theme. Each property keeps its value
/// span (DSL-13). Serves P2-12, P2-418..P2-426, P2-410.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelationshipStyle {
    /// The tag this style applies to.
    pub tag: Literal,
    /// `thickness N`: line and chevron width in px (P2-418).
    pub thickness: Option<Spanned<u32>>,
    /// `color`/`colour` (one property, P2-400, P2-401, P2-419).
    pub color: Option<Spanned<Colour>>,
    /// `style solid|dashed|dotted`, or `dashed true|false` (P2-420).
    pub style: Option<Spanned<Border>>,
    /// `routing Direct|Orthogonal|Curved` (P2-421).
    pub routing: Option<Spanned<Routing>>,
    /// `jump true|false` (P2-422).
    pub jump: Option<Spanned<bool>>,
    /// `fontSize N`: label size in px (P2-423).
    pub font_size: Option<Spanned<u32>>,
    /// `width W`: label text width in px (P2-424).
    pub width: Option<Spanned<u32>>,
    /// `position P`, 0..100 along the line (P2-425).
    pub position: Option<Spanned<u8>>,
    /// `opacity N`, 0..100 (P2-426).
    pub opacity: Option<Spanned<u8>>,
    /// `properties { name value }`, kept, no effect (P2-410).
    pub properties: Vec<Property>,
    /// As `ElementStyle::mode` (P2-21, P2-430).
    pub mode: Option<Mode>,
    /// As `ElementStyle::theme` (P2-13, P2-439).
    pub theme: Option<usize>,
    /// The whole `relationship` statement, block included.
    pub span: Span,
}

/// The style a drawn line ends up with: each property from the last of
/// its winning relationship's tags that sets it (DSL-15), else the
/// phase-1 default (`DEFAULT`). `view::plan_in` folds the per-view
/// `line` layout into it (O9). Serves P2-12, P2-418..P2-426.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResolvedRelationshipStyle {
    /// Line and chevron width in px (P2-418).
    // TODO(srs): P2-418, scaling. Reading taken (B styles-themes 9): the
    // arrowhead and join dot scale by thickness/2, never below the
    // phase-1 size; the 20px lane pitch does not change.
    pub thickness: u32,
    /// `None`: the plain line colour (CORE-3), or in a `lineColours box`
    /// view the end box's fill (P5-300); `Some` overrides both for line,
    /// chevrons, tip and label text (P2-419, P5-303).
    pub color: Option<Colour>,
    /// Polyline dash (P2-420).
    pub style: Border,
    /// Parsed routing, no effect on the drawing (P2-421).
    pub routing: Option<Routing>,
    /// Hop at crossings (P2-422).
    // TODO(srs): P2-422. Readings taken (B styles-themes 8): a `jump
    // true` line hops at every crossing with a line drawn before it; when
    // both jump, the later-drawn line hops; a hop is a 6px-radius half
    // circle.
    pub jump: bool,
    /// Label size in px; `None`: the phase-1 11px (P2-423).
    // TODO(srs): P2-423, the line pitch. Reading taken (SRS proposal, as
    // 11/13 in phase 1): N + 2.
    pub font_size: Option<u32>,
    /// Label text width in px; `None`: `labelWidth` characters (P2-424).
    pub width: Option<u32>,
    /// Label position in percent of the line's length; `None`: phase-1
    /// placement (P2-425).
    pub position: Option<u8>,
    /// 0..100 (P2-426).
    pub opacity: u8,
}

impl ResolvedRelationshipStyle {
    /// The phase-1 line look: 2px, destination colour, solid, no routing,
    /// no jump, phase-1 label, opacity 100 (CORE-2, CORE-3, CORE-10).
    /// Serves P2-12, P2-437.
    pub const DEFAULT: ResolvedRelationshipStyle = ResolvedRelationshipStyle {
        thickness: 2,
        color: None,
        style: Border::Solid,
        routing: None,
        jump: false,
        font_size: None,
        width: None,
        position: None,
        opacity: 100,
    };
}

/// The look of a boundary: element styles for `Boundary` then
/// `Boundary:SoftwareSystem`/`Boundary:Container` (scope boundaries), or
/// `Group` then `Group:<name>` (group boundaries), per property; `None`
/// keeps the phase-1 value (dashed 8/6, 1px, #666666, no fill). Serves
/// P2-411, P2-412.
// TODO(srs): P2-411, a boundary `icon`. Reading taken (B styles-themes
// 7b): drawn left of its name tag, at the tag's height.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResolvedBoundaryStyle {
    /// Fill; `None`: no fill.
    pub background: Option<Colour>,
    /// Outline colour.
    pub stroke: Option<Colour>,
    /// Outline width in px.
    pub stroke_width: Option<u8>,
    /// Name-tag text colour.
    pub color: Option<Colour>,
    /// Name-tag size in px.
    pub font_size: Option<u32>,
    /// Outline dash; `None`: the phase-1 8/6 dash.
    pub border: Option<Border>,
    /// 0..100.
    pub opacity: u8,
    /// `false` drops the bracketed type from the name tag (scope
    /// boundaries only, P2-411).
    pub metadata: bool,
}

impl ResolvedBoundaryStyle {
    /// The phase-1 boundary look: every property `None`, opacity 100,
    /// metadata shown (CORE-1). Serves P2-411, P2-437.
    // DECISION: a pub const beside `ResolvedRelationshipStyle::DEFAULT`
    // (not in A.2) so `view::plan` and test plans build a phase-1
    // boundary without calling the model.
    pub const DEFAULT: ResolvedBoundaryStyle = ResolvedBoundaryStyle {
        background: None,
        stroke: None,
        stroke_width: None,
        color: None,
        font_size: None,
        border: None,
        opacity: 100,
        metadata: true,
    };
}

/// One `theme`/`themes` source in `views`, in theme order. Serves P2-13,
/// P2-427..P2-429.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Theme {
    /// The source as written.
    pub source: Literal,
    /// What the source names.
    pub kind: ThemeSource,
    /// The `theme`/`themes` statement.
    pub span: Span,
}

/// What a theme source names. Serves P2-13, P2-427, P2-429.
// TODO(srs): P2-429, bundled copies, a cache or a fetch. Reading taken (B
// styles-themes 11 with approver note C): a name or URL draws nothing and
// gets a P2-3 notice naming it (no network in phase 2; phase-3 row
// `network`); `theme default` by name draws if its styles can be bundled
// under their licence. A theme file must stay under the diagram's folder
// (`PathOutsideRoot`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ThemeSource {
    /// A local Structurizr theme JSON file, relative to the diagram
    /// file's folder (P2-427).
    File(String),
    /// A named theme (`default`, ...).
    Name(String),
    /// An `https://` URL.
    Url(String),
}

/// The view kinds of phase 1. Phase 2 and 3 add `systemLandscape`,
/// `filtered`, `custom`, `image`, `dynamic` and `deployment`. Serves
/// DSL-6.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ViewKind {
    /// `systemContext <system>`
    SystemContext,
    /// `container <system>`
    Container,
    /// `component <container>`
    Component,
    /// `systemLandscape [key] [description]` (P2-2, P2-8, P2-124).
    SystemLandscape,
    /// `filtered <base> <include|exclude> <tags>` (P2-2, P2-9).
    Filtered,
    /// `custom [key] [title] [description]` (P2-2, P2-19).
    Custom,
    /// `image <*|element> [key]` (P2-2, P2-3).
    Image,
    /// `dynamic <*|system|container> [key] [description]` (P2-121).
    Dynamic,
    /// `deployment <*|system> <environment> [key] [description]` (P2-179).
    Deployment,
}

impl ViewKind {
    /// The DSL keyword (`systemContext`, `container`, `component`, ...).
    /// Serves DSL-4 (error text), DSL-6, P2-122.
    pub const fn keyword(self) -> &'static str {
        match self {
            ViewKind::SystemContext => "systemContext",
            ViewKind::Container => "container",
            ViewKind::Component => "component",
            ViewKind::SystemLandscape => "systemLandscape",
            ViewKind::Filtered => "filtered",
            ViewKind::Custom => "custom",
            ViewKind::Image => "image",
            ViewKind::Dynamic => "dynamic",
            ViewKind::Deployment => "deployment",
        }
    }

    /// True for the kinds c4rizer draws: the phase-1 kinds after
    /// dsl-breadth, plus `SystemLandscape`, `Filtered`, `Custom` and
    /// `Image` once landscape-filtered lands, `Deployment` once
    /// deployment-view lands (P3-2), and `Dynamic` once dynamic-view
    /// lands (P3-200). A view whose kind is not drawable is
    /// listed in `pipeline::AllViews::unsupported`. Serves P2-2, P2-122,
    /// P2-123, P2-180.
    pub const fn drawable(self) -> bool {
        matches!(
            self,
            ViewKind::SystemContext
                | ViewKind::Container
                | ViewKind::Component
                | ViewKind::SystemLandscape
                | ViewKind::Filtered
                | ViewKind::Custom
                | ViewKind::Image
                | ViewKind::Deployment
                | ViewKind::Dynamic
        )
    }
}

/// What a view is about. Every phase-1 view names one element; phase 2
/// adds a view with no scope (`systemLandscape`, `custom`) and phase 3 a
/// wildcard scope (`dynamic *`, `deployment *`). Serves DSL-6.
// DECISION: Clone but not Copy: phase 2 and 3 add scopes with heap data.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ViewScope {
    /// The software system (context and container views) or container
    /// (component views), as written.
    Element(Spanned<ElementId>),
    /// No scope argument: `systemLandscape`, `custom`, `filtered`. Serves
    /// P2-2, P2-124.
    Unscoped,
    /// A written `*`: `dynamic *`, `image *`. Serves P2-121.
    Wildcard(Span),
    /// `deployment <*|system> <environment>`; `software_system: None` for
    /// `*`. Serves P2-179.
    Deployment {
        /// The software system, `None` for `*`.
        software_system: Option<Spanned<ElementId>>,
        /// The deployment environment's name or id, as written.
        environment: Literal,
    },
}

impl ViewScope {
    /// The scope element, `None` for a view with no single scope
    /// (`Unscoped`, `Wildcard`, `Deployment`). Serves DSL-6.
    pub const fn element(&self) -> Option<ElementId> {
        match self {
            ViewScope::Element(e) => Some(e.value),
            // DECISION: A.3 item 3 fixes this value; a deployment view's
            // system is not its single scope element.
            ViewScope::Unscoped | ViewScope::Wildcard(_) | ViewScope::Deployment { .. } => None,
        }
    }
}

/// One entry of an `include` or `exclude` line. Phase 2 adds expressions
/// and relationship patterns (P2-20). Serves DSL-6.
// DECISION: Clone but not Copy: phase 2 adds expressions (P2-20).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Selector {
    /// `*`: what Structurizr selects for the view kind and scope (SRS 4.1
    /// keyword table).
    All,
    /// One element, by id or dotted path.
    Element(ElementId),
    /// `*?` (P2-323): on a system context, container or component view,
    /// the elements of `*` with only the relationships to or from the
    /// scoped element (C1) or the scope's children (C2, C3). Serves
    /// P2-20, P2-323.
    // TODO(srs): P2-323, `*?` on any other view kind. Reading taken (B
    // landscape-filtered 15, Structurizr 6.2.4 `StaticViewContentParser`
    // calls `addDefaultElements(false)` for every static view): it means
    // the same as `*`.
    AllReluctant,
    /// A named relationship `r = a -> b` (dsl-breadth P2-146) in
    /// `include` or `exclude`: adds or removes that relationship's line,
    /// drawn only when both ends are in the view. Serves P2-20, P2-341.
    Relationship(RelationshipId),
    /// An element or relationship expression (/dsl/expressions), written
    /// in double quotes when it holds whitespace (P2-337). Element
    /// expressions add or remove boxes; relationship expressions act on
    /// the drawn lines after lifting (CORE-5) and never add a box
    /// (P2-334). Serves P2-20, P2-324..P2-337.
    Expression(Expression),
}

/// `hide <src> -> <dst>`: leave out the line from `source` to
/// `destination`, one pair per line. Serves DSL-6.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hide {
    /// Source element, as written.
    pub source: Spanned<ElementId>,
    /// Destination element, as written.
    pub destination: Spanned<ElementId>,
    /// The whole `hide` line.
    pub span: Span,
}

/// `at <id> <x> <y>`: the centre of a box, in pixels. Serves DSL-7,
/// DSL-9.
///
/// The model accepts any integer: an off-grid centre is kept as written
/// and flagged by lint (CORE-18, L5), not rejected here. In phase 1 the
/// line addresses an element by id or dotted path; the view-local key of
/// the Interfaces paragraph equals the element id, and a later phase adds
/// a view-local key field for views that draw an element twice.
// DECISION: Clone but not Copy: the later view-local key is heap data.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct At {
    /// The box, as written.
    pub element: Spanned<ElementId>,
    /// Centre x in pixels.
    pub x: Spanned<i32>,
    /// Centre y in pixels.
    pub y: Spanned<i32>,
    /// The whole `at` line, line ending excluded; DSL-9 rewrites this.
    pub span: Span,
}

/// `size <id> <w> <h>`: a box size override, in pixels. Serves DSL-7.
///
/// Sizes that are not multiples of 20 are kept as written and flagged by
/// lint (CORE-18, L5); a width or height that is not `> 0` is
/// `InvalidValue`. `i32` like `At`, so callers do one kind of pixel
/// arithmetic. Addresses the box as `At` does.
// DECISION: Clone but not Copy, as `At`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Size {
    /// The box, as written.
    pub element: Spanned<ElementId>,
    /// Width in pixels, `> 0`.
    pub width: Spanned<i32>,
    /// Height in pixels, `> 0`.
    pub height: Spanned<i32>,
    /// The whole `size` line, line ending excluded.
    pub span: Span,
}

/// A view's c4rizer options with keyword-table defaults filled in.
/// Serves DSL-6, CORE-4, CORE-8, CORE-14, CORE-17.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ViewOptions {
    /// `technology on|off`.
    pub technology: bool,
    /// `bus on|off`.
    pub bus: bool,
    /// `labelWidth <n>`: characters per label line.
    pub label_width: u32,
    /// `maxCrossings <n>`: crossing budget.
    pub max_crossings: u32,
    /// `labels line|callout` (c4rizer extension): on-line labels or
    /// callouts beside the line. Serves P4-300.
    // APPROVER: new pub item
    pub labels: LabelMode,
    /// `crossings gap|jump|plain` (c4rizer extension): how the view marks
    /// where two lines cross. Serves P4-500.
    // APPROVER: new pub item
    pub crossings: CrossingStyle,
    /// `lineColours plain|box` (c4rizer extension): unstyled lines in the
    /// plain line colour, or in the fill of the box they point to.
    /// Serves P5-300.
    // PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-13)
    pub line_colours: LineColours,
}

/// What colour a view gives its unstyled lines: `lineColours plain|box`
/// (or `lineColors`; c4rizer extension). A relationship style colour
/// wins over both. Serves P5-300..P5-305.
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-13, OR-14)
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LineColours {
    /// Every unstyled line in the plain line colour (`dg-line`): the
    /// default.
    #[default]
    Plain,
    /// Every unstyled line in the fill of the box it points to (P5-300),
    /// both lines of a two-way pair in the fill of the more connected box
    /// (P5-305).
    Box,
}

impl LineColours {
    /// The written keyword: `plain` or `box`. Serves P5-300, P5-308.
    // PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-13)
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            LineColours::Plain => "plain",
            LineColours::Box => "box",
        }
    }

    /// The mode written `word`, ignoring case.
    fn from_keyword(word: &str) -> Option<LineColours> {
        [LineColours::Plain, LineColours::Box]
            .into_iter()
            .find(|m| m.keyword().eq_ignore_ascii_case(word))
    }
}

/// How a view marks line crossings: `crossings gap|jump|plain` (c4rizer
/// extension). Serves P4-500, P4-506, P4-508.
// APPROVER: new pub item
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CrossingStyle {
    /// The carrier is cut around the crossing, each cut end with a T bar
    /// (P4-506): the default.
    #[default]
    Gap,
    /// The carrier makes a small rectangular hop around the other line
    /// (P4-508).
    Jump,
    /// Lines cross as drawn, no mark.
    Plain,
}

impl CrossingStyle {
    /// The written keyword: `gap`, `jump` or `plain`. Serves P4-500,
    /// P4-521.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            CrossingStyle::Gap => "gap",
            CrossingStyle::Jump => "jump",
            CrossingStyle::Plain => "plain",
        }
    }

    /// The style written `word`, ignoring case.
    fn from_keyword(word: &str) -> Option<CrossingStyle> {
        [CrossingStyle::Gap, CrossingStyle::Jump, CrossingStyle::Plain]
            .into_iter()
            .find(|m| m.keyword().eq_ignore_ascii_case(word))
    }
}

/// How a view draws its line labels: `labels line|callout` (c4rizer
/// extension). Serves P4-300..P4-302.
// APPROVER: new pub item
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LabelMode {
    /// On its own line, with a backing (CORE-14, CORE-15): the default.
    #[default]
    Line,
    /// Beside its line on a shelf, with a slanted leader to the line
    /// (P4-302..P4-313).
    Callout,
}

impl LabelMode {
    /// The written keyword: `line` or `callout`. Serves P4-300, P4-318.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            LabelMode::Line => "line",
            LabelMode::Callout => "callout",
        }
    }

    /// The mode written `word`, ignoring case.
    fn from_keyword(word: &str) -> Option<LabelMode> {
        [LabelMode::Line, LabelMode::Callout]
            .into_iter()
            .find(|m| m.keyword().eq_ignore_ascii_case(word))
    }
}

impl ViewOptions {
    /// The keyword-table defaults: technology on, bus off, label width
    /// 24, crossing budget 0. Serves DSL-6.
    pub const DEFAULT: ViewOptions = ViewOptions {
        technology: true,
        bus: false,
        label_width: 24,
        max_crossings: 0,
        labels: LabelMode::Line,
        crossings: CrossingStyle::Gap,
        line_colours: LineColours::Plain,
    };
}

/// One view as written. Options are `None` when absent; `options()`
/// fills the defaults. Serves DSL-6, DSL-7, DSL-8.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewSpec {
    /// System context, container or component.
    pub kind: ViewKind,
    /// The element the view is about.
    pub scope: ViewScope,
    /// The effective key: as written, else generated. Unique across the
    /// file; names the SVG file (CLI-1).
    pub key: String,
    /// The key literal, if written.
    pub declared_key: Option<Literal>,
    /// The description: positional, or a `description` line in the body.
    pub description: Option<Literal>,
    /// The `title` line; wins over the description as the drawn title.
    pub title: Option<Literal>,
    /// `subtitle "..."` (c4rizer extension).
    pub subtitle: Option<Literal>,
    /// Every `include` entry, in order, all lines concatenated.
    pub include: Vec<Spanned<Selector>>,
    /// Every `exclude` entry, in order.
    pub exclude: Vec<Spanned<Selector>>,
    /// `autoLayout ...`, kept byte for byte; its arguments have no
    /// effect, unplaced boxes are auto-placed whether or not it is
    /// written (CORE-22).
    pub auto_layout: Option<Span>,
    /// `focus <id>...`: only lines touching these elements are drawn.
    pub focus: Vec<Spanned<ElementId>>,
    /// `hide <src> -> <dst>` lines.
    pub hide: Vec<Hide>,
    /// `technology on|off`.
    pub technology: Option<Spanned<bool>>,
    /// `bus on|off`.
    pub bus: Option<Spanned<bool>>,
    /// `labelWidth <n>`.
    pub label_width: Option<Spanned<u32>>,
    /// `maxCrossings <n>`.
    pub max_crossings: Option<Spanned<u32>>,
    /// `labels line|callout` (c4rizer extension). Serves P4-300.
    // APPROVER: new pub item
    pub labels: Option<Spanned<LabelMode>>,
    /// `crossings gap|jump|plain` (c4rizer extension). Serves P4-500.
    // APPROVER: new pub item
    pub crossings: Option<Spanned<CrossingStyle>>,
    /// `lineColours plain|box` (or `lineColors`; c4rizer extension): the
    /// value as written. Serves P5-300.
    // PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-13)
    pub line_colours: Option<Spanned<LineColours>>,
    /// `at` lines, in file order; at most one per element.
    pub positions: Vec<At>,
    /// `size` lines, in file order; at most one per element.
    pub sizes: Vec<Size>,
    /// The whole view statement, block included; `edit` adds a missing
    /// `at` line before its closing brace (DSL-9).
    pub span: Span,
    /// `properties { ... }` in the view body, in file order (P2-172);
    /// kept byte for byte, no effect on the scene (P2-339).
    pub properties: Vec<Property>,
    /// The base key, mode and tags of a `filtered` view; `None` for any
    /// other kind. Serves P2-9, P2-306..P2-311.
    pub filter: Option<Filter>,
    /// The source of an `image` view; `None` for any other kind. Serves
    /// P2-19, P2-320, P2-321.
    pub image: Option<ViewImage>,
    /// The steps of a `dynamic` view, in file order (parallel blocks
    /// flattened); empty for every other kind. Serves P3-3, P3-200.
    pub steps: Vec<DynamicStep>,
    /// The `default` line, if written: the view the editor opens when no
    /// last-viewed view applies (EDIT-13). Serves P2-20, P2-338.
    pub default: Option<Span>,
    /// `paperSize <keyword>` (c4rizer extension): the view's canvas size,
    /// `None` for auto (P2-701). Serves P2-701..P2-704.
    pub paper_size: Option<Spanned<PaperSize>>,
    /// `line <relationship> [position N] [routing R] [jump on|off]`
    /// lines (c4rizer extension), in file order, at most one per
    /// relationship. Serves P2-708..P2-710.
    pub lines: Vec<LineLayout>,
    /// `via <src> -> <dst> <x> <y> ...` lines (`via <n>: ...` in a
    /// dynamic view), in file order, at most one per drawn line: the
    /// hand-added vertices of each line. Serves P3-4, P3-300..P3-302.
    pub vias: Vec<Via>,
    /// `anchor <src> -> <dst> [from <point>] [to <point>]` lines (c4rizer
    /// extension; `anchor <n>: ...` in a dynamic view), in file order, at
    /// most one per drawn line, each naming a line the view draws: a
    /// refused one is left out with a notice (P4-203). Serves
    /// P4-200..P4-203.
    // APPROVER: new pub item
    pub anchors: Vec<AnchorLine>,
    /// `bundle <src> -> <dst> <src> -> <dst> ...` lines (c4rizer
    /// extension, the editor's "Bus"), in file order, each keeping only
    /// the members the view draws, no line in two of them, and at least two
    /// members: a refused member or line is left out with a notice
    /// (P4-401). Serves P4-400..P4-402.
    // APPROVER: new pub item
    pub bundles: Vec<BundleLine>,
    /// `bridge <src> -> <dst> <src> -> <dst>` lines (c4rizer extension),
    /// in file order, each naming two lines the view draws, at most one
    /// per pair: a refused one is left out with a notice (P4-501). Serves
    /// P4-501, P4-502, P4-505.
    // APPROVER: new pub item
    pub bridges: Vec<BridgeLine>,
}

/// One `bridge` line: at the crossings of the two lines it names, the
/// first one (or the bus it belongs to) carries the bridge, the second
/// passes whole. Each line is named as a `bundle` member is. Serves
/// P4-501, P4-505.
// DECISION: the two lines reuse `BundleMember` (ends as written, step
// number, span): the same `[<n>:] <src> -> <dst>` shape, checked by the
// same rule, no second copy of it.
// APPROVER: new pub item
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BridgeLine {
    /// The line that carries the bridge (cut or hopping).
    pub carrier: BundleMember,
    /// The line that passes whole.
    pub other: BundleMember,
    /// The whole `bridge` line, line ending excluded.
    pub span: Span,
}

/// One `bundle` line: its members, in the order written. Serves P4-400.
// APPROVER: new pub item
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BundleLine {
    /// The member lines, as written.
    pub members: Vec<BundleMember>,
    /// The whole `bundle` line, line ending excluded; `edit` rewrites this.
    pub span: Span,
}

/// One member of a `bundle` line: the drawn line from `source` to
/// `destination`, named as `via` names it (P3-300, P3-301). Serves P4-400.
// APPROVER: new pub item
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BundleMember {
    /// The line's source element, as written.
    pub source: Spanned<ElementId>,
    /// The line's destination element, as written.
    pub destination: Spanned<ElementId>,
    /// `<n>:` of a dynamic view's member, without the colon.
    pub order: Option<Spanned<String>>,
    /// The member as written (`[<n>:] <src> -> <dst>`).
    pub span: Span,
}

/// One `anchor` line: the drawn line from `source` to `destination` (in
/// a dynamic view, the step numbered `order`, else the lowest-numbered
/// step between those ends, as `via` names it) has its source end pinned
/// to `from` and/or its destination end pinned to `to`, each a point of
/// its box (`anchor::Anchor`). At least one of them is set. Serves
/// P4-200, P4-201.
// APPROVER: new pub item
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnchorLine {
    /// The line's source element, as written.
    pub source: Spanned<ElementId>,
    /// The line's destination element, as written.
    pub destination: Spanned<ElementId>,
    /// `<n>:` of a dynamic view's `anchor <n>: ...`, without the colon.
    pub order: Option<Spanned<String>>,
    /// The source end's point (`from <point>`).
    pub from: Option<Spanned<crate::anchor::Anchor>>,
    /// The destination end's point (`to <point>`).
    pub to: Option<Spanned<crate::anchor::Anchor>>,
    /// The whole `anchor` line, line ending excluded; `edit` rewrites this.
    pub span: Span,
}

/// One `via` line: the drawn line from `source` to `destination` (in a
/// dynamic view, the step numbered `order`, else the lowest-numbered step
/// between those ends) passes through `points`, in order. Coordinates
/// are px, multiples of 10, as written. Serves P3-4, P3-300, P3-301.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Via {
    /// The line's source element, as written.
    pub source: Spanned<ElementId>,
    /// The line's destination element, as written.
    pub destination: Spanned<ElementId>,
    /// `<n>:` of a dynamic view's `via <n>: ...`, without the colon.
    pub order: Option<Spanned<String>>,
    /// The vertices `(x, y)` in px, in order; each span covers both
    /// numbers. Never empty.
    pub points: Vec<Spanned<(i32, i32)>>,
    /// The whole `via` line, line ending excluded; `edit` rewrites this.
    pub span: Span,
}

/// Canvas size of a view (P2-701), Structurizr's `PaperSize` list: the
/// `keyword` is the Structurizr enum name (`A4_Landscape`), and
/// `size_px` its width and height in px at 300 dpi.
// TODO(srs): P2-701 (a), (b). Reading taken (B editor-breadth 5): the
// list is A6..A0 portrait and landscape, Letter, Legal, the three slide
// formats; the view line is `paperSize <keyword>`, placed like `at`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaperSize {
    /// A6 portrait.
    A6Portrait,
    /// A6 landscape.
    A6Landscape,
    /// A5 portrait.
    A5Portrait,
    /// A5 landscape.
    A5Landscape,
    /// A4 portrait.
    A4Portrait,
    /// A4 landscape.
    A4Landscape,
    /// A3 portrait.
    A3Portrait,
    /// A3 landscape.
    A3Landscape,
    /// A2 portrait.
    A2Portrait,
    /// A2 landscape.
    A2Landscape,
    /// A1 portrait.
    A1Portrait,
    /// A1 landscape.
    A1Landscape,
    /// A0 portrait.
    A0Portrait,
    /// A0 landscape.
    A0Landscape,
    /// US Letter portrait.
    LetterPortrait,
    /// US Letter landscape.
    LetterLandscape,
    /// US Legal portrait.
    LegalPortrait,
    /// US Legal landscape.
    LegalLandscape,
    /// Slide 4:3.
    Slide4x3,
    /// Slide 16:9.
    Slide16x9,
    /// Slide 16:10.
    Slide16x10,
}

impl PaperSize {
    /// Every size, in declaration order (Structurizr's enum order).
    pub(crate) const ALL: [PaperSize; 21] = [
        PaperSize::A6Portrait,
        PaperSize::A6Landscape,
        PaperSize::A5Portrait,
        PaperSize::A5Landscape,
        PaperSize::A4Portrait,
        PaperSize::A4Landscape,
        PaperSize::A3Portrait,
        PaperSize::A3Landscape,
        PaperSize::A2Portrait,
        PaperSize::A2Landscape,
        PaperSize::A1Portrait,
        PaperSize::A1Landscape,
        PaperSize::A0Portrait,
        PaperSize::A0Landscape,
        PaperSize::LetterPortrait,
        PaperSize::LetterLandscape,
        PaperSize::LegalPortrait,
        PaperSize::LegalLandscape,
        PaperSize::Slide4x3,
        PaperSize::Slide16x9,
        PaperSize::Slide16x10,
    ];

    /// The written keyword, Structurizr's enum name: `A4_Landscape`,
    /// `Letter_Portrait`, `Slide_16_9`. Serves P2-701.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            PaperSize::A6Portrait => "A6_Portrait",
            PaperSize::A6Landscape => "A6_Landscape",
            PaperSize::A5Portrait => "A5_Portrait",
            PaperSize::A5Landscape => "A5_Landscape",
            PaperSize::A4Portrait => "A4_Portrait",
            PaperSize::A4Landscape => "A4_Landscape",
            PaperSize::A3Portrait => "A3_Portrait",
            PaperSize::A3Landscape => "A3_Landscape",
            PaperSize::A2Portrait => "A2_Portrait",
            PaperSize::A2Landscape => "A2_Landscape",
            PaperSize::A1Portrait => "A1_Portrait",
            PaperSize::A1Landscape => "A1_Landscape",
            PaperSize::A0Portrait => "A0_Portrait",
            PaperSize::A0Landscape => "A0_Landscape",
            PaperSize::LetterPortrait => "Letter_Portrait",
            PaperSize::LetterLandscape => "Letter_Landscape",
            PaperSize::LegalPortrait => "Legal_Portrait",
            PaperSize::LegalLandscape => "Legal_Landscape",
            PaperSize::Slide4x3 => "Slide_4_3",
            PaperSize::Slide16x9 => "Slide_16_9",
            PaperSize::Slide16x10 => "Slide_16_10",
        }
    }

    /// Width and height in px (Structurizr's 300 dpi values). Serves
    /// P2-701.
    // DECISION: the values of structurizr-core 6.2.4 `PaperSize`, read
    // from its bytecode (A3 portrait is 3508 x 4961 there, not 4960).
    #[must_use]
    pub const fn size_px(self) -> (i32, i32) {
        match self {
            PaperSize::A6Portrait => (1240, 1748),
            PaperSize::A6Landscape => (1748, 1240),
            PaperSize::A5Portrait => (1748, 2480),
            PaperSize::A5Landscape => (2480, 1748),
            PaperSize::A4Portrait => (2480, 3508),
            PaperSize::A4Landscape => (3508, 2480),
            PaperSize::A3Portrait => (3508, 4961),
            PaperSize::A3Landscape => (4961, 3508),
            PaperSize::A2Portrait => (4961, 7016),
            PaperSize::A2Landscape => (7016, 4961),
            PaperSize::A1Portrait => (7016, 9933),
            PaperSize::A1Landscape => (9933, 7016),
            PaperSize::A0Portrait => (9933, 14043),
            PaperSize::A0Landscape => (14043, 9933),
            PaperSize::LetterPortrait => (2550, 3300),
            PaperSize::LetterLandscape => (3300, 2550),
            PaperSize::LegalPortrait => (2550, 4200),
            PaperSize::LegalLandscape => (4200, 2550),
            PaperSize::Slide4x3 => (3306, 2480),
            PaperSize::Slide16x9 => (3508, 1973),
            PaperSize::Slide16x10 => (3508, 2193),
        }
    }

    /// The size whose keyword is `word`, ignoring ASCII case.
    pub(crate) fn from_keyword(word: &str) -> Option<PaperSize> {
        PaperSize::ALL.into_iter().find(|p| p.keyword().eq_ignore_ascii_case(word))
    }
}

impl Routing {
    /// The `line` keyword: `direct`, `orthogonal`, `curved` (P2-709).
    pub(crate) const fn line_keyword(self) -> &'static str {
        match self {
            Routing::Direct => "direct",
            Routing::Orthogonal => "orthogonal",
            Routing::Curved => "curved",
        }
    }
}

/// One per-view `line` extension line: the label position, routing and
/// jumps of one relationship's line in this view, folded over its
/// relationship style by `view::plan_in` (O9). Serves P2-708..P2-710.
// TODO(srs): P2-708..P2-710, the per-view syntax. Reading taken (B
// editor-breadth 6): `line <relationship id | src -> dst> [position N]
// [routing direct|orthogonal|curved] [jump on|off]`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LineLayout {
    /// The relationship, as written (its id, or `src -> dst`).
    pub relationship: Spanned<RelationshipId>,
    /// `position N`, 0..100.
    pub position: Option<Spanned<u8>>,
    /// `routing direct|orthogonal|curved`.
    pub routing: Option<Spanned<Routing>>,
    /// `jump on|off`.
    pub jump: Option<Spanned<bool>>,
    /// The whole line.
    pub span: Span,
}

/// The mode of a `filtered` view. Serves P2-9, P2-308.
// DECISION: closed set (the DSL has exactly these two words), so not
// #[non_exhaustive].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterMode {
    /// `include`: keep the boxes and lines carrying any listed tag.
    Include,
    /// `exclude`: drop the boxes and lines carrying any listed tag.
    Exclude,
}

/// `filtered <baseKey> <include|exclude> "<tags>"`: the base view and the
/// tag filter. Serves P2-9, P2-306..P2-311.
// TODO(srs): P2-306, base kinds and declaration order. Reading taken (B
// landscape-filtered 2; Structurizr 6.2.4 `FilteredViewParser`): the base
// must be a system landscape, system context, container or component
// view declared earlier in the file, else `InvalidBaseView` or
// `UnknownView`.
// TODO(srs): P2-307, c4rizer extensions in a filtered body. Reading taken
// (B landscape-filtered 1): only `default`, `title`, `description` and
// `properties`; any other line is an error naming it.
// TODO(srs): P2-310, a lifted line merging several relationships. Reading
// taken (B landscape-filtered 3): kept when at least one merged
// relationship passes, drawn re-merged from the survivors only.
// TODO(srs): P2-311, options of a filtered view. Reading taken (B
// landscape-filtered 4): the base view's c4rizer options.
// TODO(srs): P2-312, unplaced boxes. Reading taken (B landscape-filtered
// 1): exactly where the base view places them, auto-placed centres
// included.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Filter {
    /// The base view's key, as written.
    pub base: Literal,
    /// `include` or `exclude`, as written.
    pub mode: Spanned<FilterMode>,
    /// The tags argument, as written (comma-separated).
    pub tags: Literal,
}

impl Filter {
    /// The tags of `self.tags`, split on `,` and trimmed, empty entries
    /// dropped, in written order. Serves P2-308.
    pub fn tags(&self) -> Vec<&str> {
        self.tags
            .value
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect()
    }

    /// True when an item with `tags` passes the filter: in `include`
    /// mode it has at least one listed tag, in `exclude` mode none
    /// (P2-308, P2-309).
    pub(crate) fn passes(&self, tags: &[String]) -> bool {
        let listed = self.tags();
        let any = tags.iter().any(|t| listed.contains(&t.as_str()));
        match self.mode.value {
            FilterMode::Include => any,
            FilterMode::Exclude => !any,
        }
    }
}

/// Where an `image` view's picture comes from (the body's source line).
/// Serves P2-19, P2-320, P2-321.
// TODO(srs): P2-322, image views. Readings taken (B landscape-filtered
// 10): (a) a file path is relative to the diagram file's directory and
// must stay under it (`PathOutsideRoot`); (b) a missing file is
// `FileRead`, bytes neither PNG nor SVG are `FileFormat`, both naming the
// path; (c) drawn at intrinsic size in px (SVG `width`/`height`, else
// `viewBox`) under the title; (d) an `https://` source draws a
// placeholder naming the URL and adds a P2-3 notice (no network in phase
// 2); (e) `image *` has no scope, `image <element>` is scoped to it; (f)
// an element-scoped image view makes its element enterable (EDIT-15) when
// no container or component view of it exists.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImageSource {
    /// `image <file>`: a local PNG or SVG file.
    File(Literal),
    /// `image <https url>`.
    Url(Literal),
    /// `plantuml <file|url|view key>`: drawn as a placeholder (P2-321).
    PlantUml(Literal),
    /// `mermaid <file|url|view key>`: drawn as a placeholder (P2-321).
    Mermaid(Literal),
    /// `kroki <format> <source>`: parsed, draws nothing, P2-3 notice
    /// (P2-321).
    Kroki {
        /// The Kroki diagram format.
        format: Literal,
        /// The file, URL or view key.
        source: Literal,
    },
}

/// One step of a `dynamic` view (P3-200): `[order:] <src> -> <dst>
/// [desc] [tech]` or `[order:] <relationship id> [desc]`, in a view body
/// or a parallel `{ }` block. Serves P3-3, P3-200..P3-210.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DynamicStep {
    /// The source element, as written; for a relationship-id step, the
    /// relationship's source with the id token's span.
    pub source: Spanned<ElementId>,
    /// The destination element, as written; for a relationship-id step,
    /// the relationship's destination with the id token's span.
    pub destination: Spanned<ElementId>,
    /// The model relationship the step stands for (P3-201, P3-202): for
    /// an implied step, the declared relationship it is implied from.
    pub relationship: RelationshipId,
    /// The step's own description literal, if written (P3-203).
    pub description: Option<Literal>,
    /// The step's own technology literal, if written (P3-203).
    pub technology: Option<Literal>,
    /// The sequence number as drawn: the written order, else the counter
    /// (P3-208..P3-210).
    pub order: String,
    /// The written `n:` token, if any (P3-209, P3-221).
    // APPROVER: new pub item
    pub written_order: Option<Span>,
    /// The innermost parallel `{ }` block holding the step, `None` at the
    /// top of the view body (P3-210, P3-220, P3-221).
    // APPROVER: new pub item
    pub parallel: Option<Span>,
    /// True for a response step (P3-201): its ends match `relationship`
    /// in reverse, so its line runs from `source` to `destination`, back
    /// along the relationship, dashed (Structurizr 6.2.4
    /// `RelationshipView.response`).
    // APPROVER: new pub item
    pub response: bool,
    /// The whole step line, line ending excluded.
    pub span: Span,
}

/// The picture of an `image` view. Serves P2-19, P2-320, P2-321.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewImage {
    /// The source line, as written.
    pub source: ImageSource,
    /// The bytes of a local PNG or SVG file, read at parse time through
    /// the `Loader` (O3); `None` for every other source.
    pub asset: Option<Asset>,
    /// The whole source line.
    pub span: Span,
}

impl ViewSpec {
    /// The options with defaults filled in. Serves DSL-6.
    pub const fn options(&self) -> ViewOptions {
        let d = ViewOptions::DEFAULT;
        ViewOptions {
            technology: match self.technology {
                Some(v) => v.value,
                None => d.technology,
            },
            bus: match self.bus {
                Some(v) => v.value,
                None => d.bus,
            },
            label_width: match self.label_width {
                Some(v) => v.value,
                None => d.label_width,
            },
            max_crossings: match self.max_crossings {
                Some(v) => v.value,
                None => d.max_crossings,
            },
            labels: match self.labels {
                Some(v) => v.value,
                None => d.labels,
            },
            crossings: match self.crossings {
                Some(v) => v.value,
                None => d.crossings,
            },
            line_colours: match self.line_colours {
                Some(v) => v.value,
                None => d.line_colours,
            },
        }
    }

    /// The drawn title: the `title` line, else the description; `None`
    /// is lint L8. Serves DSL-6.
    pub const fn drawn_title(&self) -> Option<&Literal> {
        match &self.title {
            Some(title) => Some(title),
            None => self.description.as_ref(),
        }
    }

    /// The `at` line for `element`, if any. Serves DSL-7, DSL-9.
    pub fn position_of(&self, element: ElementId) -> Option<&At> {
        self.positions.iter().find(|a| a.element.value == element)
    }

    /// The `size` line for `element`, if any. Serves DSL-7.
    pub fn size_of(&self, element: ElementId) -> Option<&Size> {
        self.sizes.iter().find(|s| s.element.value == element)
    }

    /// The view-local key of drawn step `step` (index into `steps`):
    /// `<relationship key or source->destination>#<number>`, element keys
    /// canonical (`Model::full_key`), with `~2`, `~3`... added when an
    /// earlier drawn step has the same key, so keys are unique in the
    /// view. `None` for a step that is not drawn. Serves P3-212.
    // APPROVER: new pub item
    pub fn step_key(&self, model: &Model, step: usize) -> Option<String> {
        let base = |i: usize| -> Option<String> {
            let s = self.steps.get(i)?;
            let r = model.relationship(s.relationship);
            let key = |e: ElementId| model.full_id(e).unwrap_or_else(|| model.element(e).name.value.clone());
            let name = if let Some(k) = &r.key {
                k.value.clone()
            } else {
                format!("{}->{}", key(s.source.value), key(s.destination.value))
            };
            Some(format!("{name}#{}", s.order))
        };
        let drawn = self.drawn_steps();
        let at = drawn.iter().position(|&i| i == step)?;
        let own = base(step)?;
        let earlier = drawn[..at].iter().filter(|&&i| base(i).as_deref() == Some(own.as_str())).count();
        Some(if earlier == 0 { own } else { format!("{own}~{}", earlier + 1) })
    }

    /// The steps of a dynamic view that are drawn, as indices into
    /// `steps`, in plan edge order: edge `i` draws `steps[drawn_steps()[i]]`.
    /// A step with the same relationship and number as an earlier step
    /// is drawn once (Structurizr 6.2.4 keeps one line, probed), and a
    /// step from an element to itself is not drawn. Empty for every other
    /// kind. Serves P3-212, P3-216, P3-219.
    // APPROVER: new pub item
    pub fn drawn_steps(&self) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for (i, step) in self.steps.iter().enumerate() {
            // DECISION: a step from an element to itself draws no line, as
            // a static view drops a line whose ends share a box (CORE-5).
            if step.source.value == step.destination.value {
                continue;
            }
            let twin = out.iter().any(|&j| {
                self.steps[j].relationship == step.relationship && self.steps[j].order == step.order
            });
            if !twin {
                out.push(i);
            }
        }
        out
    }
}

// ------------------------------------------------- phase 2: dsl-breadth

/// Index of a group in `Model::groups`; `groups[i].id == GroupId(i)`.
/// Stable for one parse. Serves P2-4, P2-129.
// TODO(owner): `#[repr(transparent)]` is a data-layout item (CLAUDE.md);
// it mirrors `ElementId` and the `size_of` test pins 4 bytes.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(pub u32);

/// A `group "<name>" { ... }` block. Serves P2-4, P2-127..P2-131.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Group {
    /// This group's index in `Model::groups`.
    pub id: GroupId,
    /// `g = group "x"`, used by `element==g` (P2-166); `None` when the
    /// group has no identifier.
    pub key: Option<Spanned<String>>,
    /// The name as written.
    pub name: Literal,
    /// The names from the outermost group down, joined with the model
    /// property `structurizr.groupSeparator` (P2-129); equal to `name`
    /// for a group that is not nested.
    // TODO(srs): P2-129, a nested group with no separator set. Reading
    // taken (B dsl-breadth 14b, Structurizr for Java):
    // `ModelErrorKind::NestedGroupNeedsSeparator`.
    pub full_name: String,
    /// The enclosing group, `None` at the outermost level.
    pub parent: Option<GroupId>,
    /// The element whose block holds the group, `None` at model level.
    pub owner: Option<ElementId>,
    /// The whole statement, block included.
    pub span: Span,
}

/// One `<name> <value>` line of a `properties` block, on any owner
/// (workspace, model, views, configuration, element, relationship, view,
/// style). Serves P2-172.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Property {
    /// The property name, as written.
    pub name: Literal,
    /// The value, as written (after `${}` substitution, P2-150).
    pub value: Literal,
    /// The whole line.
    pub span: Span,
}

/// One perspective of an element or relationship: the one-line
/// `<name> <description> [value]` form or the `perspective <name> {
/// description, value, url }` block form. Serves P2-173.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Perspective {
    /// The perspective name.
    pub name: Literal,
    /// Its description.
    pub description: Option<Literal>,
    /// Its value.
    pub value: Option<Literal>,
    /// Its URL (block form only).
    pub url: Option<Literal>,
    /// The whole line or block.
    pub span: Span,
}

/// Which directive an `Attachment` came from. Serves P2-159, P2-160.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttachmentKind {
    /// `!docs <path> [fqcn]`.
    Docs,
    /// `!adrs <path> [type|fqcn]` or its alias `!decisions`.
    Adrs,
}

/// A `!docs`, `!adrs` or `!decisions` directive on the workspace or an
/// element: the path and importer are recorded, the files are not read in
/// phase 2. Serves P2-159, P2-160.
// TODO(srs): P2-159, whether phase 2 reads or checks the files. Reading
// taken (B dsl-breadth 19, Structurizr fails the parse on a missing
// path): a path that does not exist is `ModelErrorKind::FileRead`, one
// outside the diagram's folder `PathOutsideRoot`; the content is not read.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Attachment {
    /// Docs or ADRs.
    pub kind: AttachmentKind,
    /// The path as written.
    pub path: Literal,
    /// The importer type or FQCN, if written; a FQCN also adds a notice
    /// (P2-3).
    pub importer: Option<Literal>,
    /// The whole directive.
    pub span: Span,
}

/// `!identifiers flat|hierarchical`. Serves P2-6, P2-142..P2-144.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Identifiers {
    /// Every element id is global (the default).
    #[default]
    Flat,
    /// An element's full id is its parent's full id, `.`, its own id.
    Hierarchical,
}

/// The `!impliedRelationships` setting. Serves P2-7, P2-153, P2-154.
// P2-7, a directive placed between relationships (B dsl-breadth 17,
// checked against Structurizr for Java; approver ruling 2026-11-17): it
// applies to relationships declared after it. `Model` holds the last
// value written; each `Relationship::implied_relationships` holds the
// one in force at its declaration, which `view` and `include *` read.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum ImpliedRelationships {
    /// No directive: CORE-5 lifting.
    #[default]
    Default,
    /// `!impliedRelationships true`: same as `Default` (P2-153).
    True,
    /// `!impliedRelationships false`: no lifting (P2-7).
    False,
    /// `!impliedRelationships <fqcn>`: same as `Default`, plus a notice
    /// (P2-154).
    Strategy(Literal),
}

/// `configuration { scope ... }`. Serves P2-175.
// TODO(srs): P2-175, the validation the scope applies. Reading taken
// (Structurizr's workspace-scope validators, structurizr-core 6.2.4,
// checked; differs from B dsl-breadth 20): `Landscape` fails when any
// software system has containers or documentation, `SoftwareSystem` when
// more than one has, `None` checks nothing (`ScopeViolation`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigurationScope {
    /// `scope landscape`.
    Landscape,
    /// `scope softwaresystem`.
    SoftwareSystem,
    /// `scope none`.
    None,
}

/// The bracket style of the box type line (`terminology { metadata ... }`).
/// Serves P2-163.
// TODO(srs): P2-163, the glyphs for `double-angle`. Reading taken (B
// dsl-breadth 18a): `«` and `»`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetadataSymbols {
    /// `[...]`, the default.
    Square,
    /// `(...)`.
    Round,
    /// `{...}`.
    Curly,
    /// `<...>`.
    Angle,
    /// `«...»`.
    DoubleAngle,
    /// No brackets.
    None,
}

/// The `terminology` block of `views`: a written word replaces the type
/// word on box type lines, boundary name tags and key entries.
/// `deployment_node` and `infrastructure_node` are stored and take effect
/// in phase 3. Serves P2-161, P2-162, P2-163.
// TODO(srs): P2-161, the CORE-2 `External Software System` type word under
// terminology. Reading taken (B dsl-breadth 18b): `External <word>`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Terminology {
    /// `person "<word>"`.
    pub person: Option<Literal>,
    /// `softwareSystem "<word>"`.
    pub software_system: Option<Literal>,
    /// `container "<word>"`.
    pub container: Option<Literal>,
    /// `component "<word>"`.
    pub component: Option<Literal>,
    /// `deploymentNode "<word>"` (phase 3 effect).
    pub deployment_node: Option<Literal>,
    /// `infrastructureNode "<word>"` (phase 3 effect).
    pub infrastructure_node: Option<Literal>,
    /// `relationship "<word>"`: the key's line entries (P2-162).
    pub relationship: Option<Literal>,
    /// `metadata round|curly|angle|double-angle|square|none` (P2-163).
    pub metadata: Option<Spanned<MetadataSymbols>>,
}

/// What a `Notice` reports. Serves P2-3, P2-154.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NoticeKind {
    /// A construct accepted with no effect (P2-3): `!script`, `!plugin`,
    /// `!components`, `healthCheck`, `configuration visibility|users`, an
    /// external-source `image` view, `plantuml.*`/`mermaid.*`/`kroki.*`
    /// view-set properties, `!impliedRelationships <fqcn>`, a
    /// `!docs`/`!adrs` importer FQCN.
    NoEffect,
    /// import-layout: a JSON view whose key names no view in the file;
    /// it is skipped (P2-505). `subject` is the key.
    ViewNotInFile,
    /// import-layout: a JSON deployment or image view, or a file view
    /// c4rizer does not draw or position; it is skipped (P2-510).
    ViewNotDrawable,
    /// import-layout: a view matched on a key c4rizer generated, which
    /// the reference says is not stable (P2-505).
    GeneratedKey,
    /// import-layout: a drawn box with no JSON element to take a
    /// position from (P2-507). `subject` is the element's name.
    UnmatchedBox,
    /// import-layout: a JSON element view whose `id` is not in the JSON
    /// model (P2-506). `subject` is the id.
    UnknownJsonElement,
    /// import-layout: a matched box whose element has no identifier, so
    /// no `at` line can name it (P2-511). `subject` is its name.
    NoIdentifier,
    /// import-layout: layout data c4rizer cannot hold (relationship
    /// `vertices`, `routing`, `position`; view `paperSize`,
    /// `dimensions`; `themes`), named once per view (P2-510).
    NotImported,
    /// An `anchor` line c4rizer ignores (P4-203): malformed, naming an
    /// unknown element, a line the view does not draw, or a line that
    /// already has one. `subject` says which and why.
    // APPROVER: new pub item
    AnchorIgnored,
    /// A `bundle` line, or one member of it, c4rizer ignores (P4-401):
    /// malformed, naming an unknown element, a line the view does not
    /// draw, a line named twice or already bundled, or a bundle left with
    /// fewer than two members. `subject` says which and why.
    // APPROVER: new pub item
    BundleIgnored,
    /// A `bridge` line c4rizer ignores (P4-501): malformed, naming an
    /// unknown element, a line the view does not draw, the same line
    /// twice, or a pair an earlier `bridge` line already decides.
    /// `subject` says which and why.
    // APPROVER: new pub item
    BridgeIgnored,
}

/// Something the file says that c4rizer accepts with no effect: one per
/// construct, in file order, in `Model::notices`. Notices are never
/// errors and add nothing to the exit code (P2-125). `Display` gives
/// `<line>:<column>: <subject> is accepted and has no effect`; the caller
/// adds the path of `span.file`. Serves P2-3, P2-125, P2-126.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Notice {
    /// What kind of remark this is.
    pub kind: NoticeKind,
    /// The construct as named in P2-3 (`!script`, `healthCheck`,
    /// `plantuml.url`, ...).
    pub subject: String,
    /// The view key, for a construct inside a view.
    pub view: Option<String>,
    /// The construct's bytes; names its file (DSL-16).
    pub span: Option<Span>,
    /// Line and column of `span.start`.
    pub position: Option<Position>,
}

impl fmt::Display for Notice {
    /// `<line>:<column>: <subject> is accepted and has no effect`
    /// (P2-125); the caller adds `notice: <path>:`.
    // TODO(srs): P2-125, the exact line format. Reading taken (B
    // dsl-breadth 24): `notice: <path>:<line>:<column>: <construct> is
    // accepted and has no effect`.
    ///
    /// An import notice (P2-505..P2-511) has no position and reads
    /// `view <key>: <what> — <what it means or what to change>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(Position { line, column }) = self.position {
            write!(f, "{line}:{column}: ")?;
        }
        let subject = &self.subject;
        let view = self.view.as_deref().unwrap_or_default();
        match self.kind {
            NoticeKind::NoEffect => write!(f, "{subject} is accepted and has no effect"),
            NoticeKind::ViewNotInFile => {
                write!(f, "view {subject}: no view with this key in the file — skipped")
            }
            NoticeKind::ViewNotDrawable => {
                write!(f, "view {subject}: c4rizer does not draw or position this view yet — skipped")
            }
            NoticeKind::GeneratedKey => write!(
                f,
                "view {subject}: matched on a generated key, which changes when views are added — give the view an explicit key"
            ),
            NoticeKind::UnmatchedBox => write!(
                f,
                "view {view}: no position for `{subject}` in the JSON — its `at` line is kept"
            ),
            NoticeKind::UnknownJsonElement => write!(
                f,
                "view {view}: JSON element id {subject} is not in the JSON model — skipped"
            ),
            NoticeKind::NoIdentifier => write!(
                f,
                "view {view}: `{subject}` has no identifier, so no `at` line can name it — give it one (`id = ...`)"
            ),
            NoticeKind::NotImported if self.view.is_none() => {
                write!(f, "{subject}: not fetched and not imported")
            }
            NoticeKind::NotImported => {
                write!(f, "view {view}: `{subject}` not imported — c4rizer cannot hold it")
            }
            NoticeKind::AnchorIgnored => write!(f, "view {view}: {subject} — the anchor line is ignored"),
            NoticeKind::BundleIgnored => write!(f, "view {view}: {subject} — ignored in the bundle line"),
            NoticeKind::BridgeIgnored => write!(f, "view {view}: {subject} — the bridge line is ignored"),
        }
    }
}

/// The type of a file read at parse time. Serves P2-409, P2-320 (styles-
/// themes, landscape-filtered); defined here so every row shares it (O3).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mime {
    /// `image/png`.
    Png,
    /// `image/jpeg`.
    Jpeg,
    /// `image/svg+xml`.
    Svg,
}

/// An external file a style or view names, read through the `Loader` at
/// parse time; cheap to clone. Serves P2-409, P2-428, P2-320 (O3).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Asset {
    /// The path as written, relative to the file that names it.
    pub path: String,
    /// The detected type.
    pub mime: Mime,
    /// The file's bytes.
    pub bytes: Arc<[u8]>,
}

/// An element or relationship expression (/dsl/expressions), plain values
/// with no spans: the `Spanned<Selector>` or the directive statement
/// holds the span (O1). One evaluator: `Model::select_elements` and
/// `Model::select_relationships`. Serves P2-166, P2-168 (and
/// landscape-filtered's view expressions).
// TODO(srs): P2-166/P2-168, mixed `&&`/`||`. Reading taken (owner ruling,
// phase2-decisions Part C, overriding B dsl-breadth 25): Structurizr for
// Java's split, `&&` first with each side read whole (see `model/expr.rs`);
// `MixedOperators` is never raised. No parentheses.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Expression {
    /// One element by id.
    Element(ElementId),
    /// Every member of a group (`element==g`, or a group id).
    Group(GroupId),
    /// `->x`, `element==->x`: `x` and the elements with a relationship to it.
    Afferent(Box<Expression>),
    /// `x->`, `element==x->`: `x` and the elements it has a relationship to.
    Efferent(Box<Expression>),
    /// `->x->`, `element==->x->`: both directions.
    AfferentEfferent(Box<Expression>),
    /// `element.type==<type>`; `kind: None` for a type the model does not
    /// build in phase 2 (`DeploymentNode`, ...), which matches nothing.
    ElementType {
        /// The kind, when phase 2 builds it.
        kind: Option<ElementKind>,
        /// The type name as written.
        name: String,
    },
    /// `element.parent==<id>`: the direct children of that element.
    ElementParent(ElementId),
    /// `element.tag==a,b` (all of) or `element.tag!=a,b`.
    ElementTag {
        /// The tags, all of which must match.
        tags: Vec<String>,
        /// `!=` instead of `==`.
        negated: bool,
    },
    /// `element.technology==x` or `!=`.
    ElementTechnology {
        /// The technology.
        technology: String,
        /// `!=` instead of `==`.
        negated: bool,
    },
    /// `element.properties[name]==value`.
    ElementProperty {
        /// Property name.
        name: String,
        /// Property value.
        value: String,
    },
    /// `element.group==<full group name>`.
    ElementGroup(String),
    /// `*->*`, `a->*`, `*->b`, `a->b`, `relationship==...`,
    /// `relationship.source==a`, `relationship.destination==b`; `None`
    /// stands for `*`.
    Relationships {
        /// Source, `None` for any.
        source: Option<ElementId>,
        /// Destination, `None` for any.
        destination: Option<ElementId>,
    },
    /// `relationship.tag==a,b` (all of) or `!=`.
    RelationshipTag {
        /// The tags, all of which must match.
        tags: Vec<String>,
        /// `!=` instead of `==`.
        negated: bool,
    },
    /// `relationship.properties[name]==value`.
    RelationshipProperty {
        /// Property name.
        name: String,
        /// Property value.
        value: String,
    },
    /// `a && b && ...`: at least two operands.
    And(Vec<Expression>),
    /// `a || b || ...`: at least two operands.
    Or(Vec<Expression>),
}

/// File access for `parse_with`: `!include`, `workspace extends`, and
/// (styles-themes, landscape-filtered) theme, icon and image files; plus
/// the environment for `${NAME}` (P2-152). Core performs every path check
/// itself (inside the diagram's folder, cycles, `..` and symlinks through
/// `canonicalize`), so a test loader gets the same rules. New methods
/// will only ever come with default bodies. Serves P2-5, P2-16,
/// P2-133..P2-138, P2-152.
pub trait Loader {
    /// The bytes of the file at `path`; text files are UTF-8-checked by
    /// core.
    ///
    /// # Errors
    ///
    /// Any read error; core reports it as `ModelErrorKind::FileRead`.
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>>;

    /// The entries of the directory at `path` (P2-133), in any order;
    /// core sorts them.
    ///
    /// # Errors
    ///
    /// Any read error.
    fn read_dir(&mut self, path: &Path) -> io::Result<Vec<PathBuf>>;

    /// `path` with `..` and symlinks resolved (P2-134).
    ///
    /// # Errors
    ///
    /// Any resolution error.
    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf>;

    /// The environment variable `name`, `None` when unset (P2-152).
    fn env(&self, name: &str) -> Option<String>;
}

/// `std::fs` and `std::env`: core's only I/O helper besides export (HLD
/// §3 amendment, pending). The CLI and the editor use it. Serves P2-5,
/// P2-16, P2-152.
#[derive(Debug, Clone, Copy, Default)]
pub struct FsLoader;

impl Loader for FsLoader {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn read_dir(&mut self, path: &Path) -> io::Result<Vec<PathBuf>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.path()))
            .collect()
    }

    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// Refuses every path and sees no environment: what `parse` uses.
/// Serves P2-5, P2-152.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoFiles;

impl Loader for NoFiles {
    fn read(&mut self, _path: &Path) -> io::Result<Vec<u8>> {
        Err(no_files())
    }

    fn read_dir(&mut self, _path: &Path) -> io::Result<Vec<PathBuf>> {
        Err(no_files())
    }

    fn canonicalize(&mut self, _path: &Path) -> io::Result<PathBuf> {
        Err(no_files())
    }

    fn env(&self, _name: &str) -> Option<String> {
        None
    }
}

/// The error `NoFiles` gives for every path.
fn no_files() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "parsed without file access")
}

/// The whole diagram file, typed. Fields are pub plain data; ids index
/// the `Vec`s. Serves DSL-1..8, DSL-15.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Model {
    /// `workspace "<name>"`.
    pub name: Option<Literal>,
    /// `workspace "<name>" "<description>"`.
    pub description: Option<Literal>,
    /// Every element in source order (parents before children);
    /// `elements[i].id == ElementId(i)`.
    pub elements: Vec<Element>,
    /// Every relationship in source order;
    /// `relationships[i].id == RelationshipId(i)`.
    pub relationships: Vec<Relationship>,
    /// Every `element` style block in source order.
    pub element_styles: Vec<ElementStyle>,
    /// The `styles` statement inside `views`, if any; `edit` adds style
    /// blocks into it (DSL-13).
    pub styles_span: Option<Span>,
    /// The `views` statement, block included, if any; `edit` adds a
    /// `styles` block (DSL-13) or a default view for an enterable box
    /// (EDIT-15) into it without re-walking the tree.
    pub views_span: Option<Span>,
    /// Every view in source order.
    pub views: Vec<ViewSpec>,
    /// Every `group` block in source order (outer before inner);
    /// `groups[i].id == GroupId(i)`. Serves P2-4, P2-129.
    pub groups: Vec<Group>,
    /// Constructs accepted with no effect, in file order (P2-3).
    pub notices: Vec<Notice>,
    /// `!identifiers` in the workspace or `model` block (P2-6).
    pub identifiers: Identifiers,
    /// `!impliedRelationships` (P2-7, P2-153, P2-154).
    pub implied_relationships: ImpliedRelationships,
    /// `terminology` in `views` (P2-161..P2-163).
    pub terminology: Terminology,
    /// `properties` of the workspace block (P2-172).
    pub workspace_properties: Vec<Property>,
    /// `properties` of the `model` block, `structurizr.groupSeparator`
    /// among them (P2-172, P2-129).
    pub model_properties: Vec<Property>,
    /// `properties` of the `views` block, `structurizr.*` among them
    /// (P2-172, P2-176).
    pub views_properties: Vec<Property>,
    /// `properties` of the `configuration` block (P2-172).
    pub configuration_properties: Vec<Property>,
    /// `configuration { scope ... }` (P2-175).
    pub configuration_scope: Option<Spanned<ConfigurationScope>>,
    /// Workspace-level `!docs` / `!adrs` / `!decisions` (P2-159, P2-160).
    pub docs: Vec<Attachment>,
    /// Every `relationship` style block: theme styles first in theme
    /// order, then the file's own in source order (P2-12, P2-13).
    pub relationship_styles: Vec<RelationshipStyle>,
    /// Every `theme`/`themes` source in order (P2-13).
    pub themes: Vec<Theme>,
    /// Every file read at parse time (icons, image-view files), in load
    /// order, de-duplicated by path (P2-409, P2-428, O3).
    pub assets: Vec<Asset>,
    /// Every `deploymentEnvironment` in declaration order;
    /// `environments[i].id == EnvironmentId(i)`. Serves P3-1, P3-100.
    pub environments: Vec<Environment>,
}

impl Model {
    /// Builds the model from a syntax tree: elements, relationships,
    /// styles and views, with every id checked. Forward references are
    /// allowed; `at`/`size` coordinates are accepted on or off the grid
    /// (CORE-18 is lint's). Serves DSL-1..8, DSL-14, DSL-15.
    ///
    /// # Errors
    ///
    /// Returns the first problem in file order: a construct outside the
    /// phase-1 keyword table, naming its phase when it has one (DSL-4); an
    /// undeclared id (DSL-5); a duplicate element id or view key; invalid
    /// nesting or view scope; a missing or invalid argument; an `at` or
    /// `size` line for an element the view does not include, or twice for
    /// one element.
    pub fn from_tree(tree: &SyntaxTree) -> Result<Model, ModelError> {
        let mut loader = NoFiles;
        let mut builder = Builder::new(std::slice::from_ref(tree), &mut loader);
        builder.file()?;
        let mut model = builder.model;
        theme::apply(&mut model, std::slice::from_ref(tree), None)?;
        Ok(model)
    }

    /// The element with this id. Panics on an id from another model.
    /// Serves DSL-1.
    pub fn element(&self, id: ElementId) -> &Element {
        &self.elements[id.0 as usize]
    }

    /// The relationship with this id. Panics on an id from another model.
    /// Serves DSL-2.
    pub fn relationship(&self, id: RelationshipId) -> &Relationship {
        &self.relationships[id.0 as usize]
    }

    /// The element whose canonical key (`full_key`) is `key`, ignoring
    /// ASCII case. `edit` re-finds an element in a newer text by it
    /// (EDIT-24). Serves P2-6, P2-142, EDIT-24.
    // DECISION: the same resolution as `find_written`, so a key whose
    // case changed in the file still resolves as before; the parameter
    // type, not the algorithm, says the key is canonical.
    #[must_use]
    pub fn find(&self, key: &ElementKey) -> Option<ElementId> {
        self.find_written(key.as_str())
    }

    /// The element named by `written`, text from a file, the command line
    /// or JSON, ignoring ASCII case: an identifier, or a dotted path of
    /// identifiers from an ancestor down (`system.app`); under
    /// `!identifiers hierarchical`, the full id (`full_key`). Its
    /// canonical key is then `full_key`. Serves DSL-5, DSL-7, P2-6,
    /// P2-142.
    #[must_use]
    pub fn find_written(&self, key: &str) -> Option<ElementId> {
        if self.identifiers == Identifiers::Hierarchical {
            return self
                .elements
                .iter()
                .find(|e| {
                    self.full_id(e.id)
                        .is_some_and(|k| k.eq_ignore_ascii_case(key))
                })
                .map(|e| e.id);
        }
        find_path(&self.elements, key, |k| {
            self.elements
                .iter()
                .find(|e| e.key.as_ref().is_some_and(|key| key.value.eq_ignore_ascii_case(k)))
                .map(|e| e.id)
        })
        .or_else(|| self.find_instance(key))
    }

    /// The first instance without an id whose view-local key
    /// `<element key>_<n>` is `key`, ignoring case (P3-117).
    // DECISION: the same key may name an instance in each environment; the
    // first in model order wins here, and a view resolves it within its own
    // environment (`at` lines, `edit`).
    fn find_instance(&self, key: &str) -> Option<ElementId> {
        self.elements
            .iter()
            .filter(|e| e.key.is_none() && e.instance_of.is_some())
            .find(|e| self.full_id(e.id).is_some_and(|k| k.eq_ignore_ascii_case(key)))
            .map(|e| e.id)
    }

    /// Every relationship from `source` to `destination`, in source
    /// order. A relationship's identity across parses is (source key,
    /// destination key, index in this list), which `edit` uses to re-apply
    /// an action to a newer text (EDIT-24, EDIT-19). Serves DSL-2.
    pub fn find_relationship(
        &self,
        source: ElementId,
        destination: ElementId,
    ) -> Vec<RelationshipId> {
        self.relationships
            .iter()
            .filter(|r| r.source.value == source && r.destination.value == destination)
            .map(|r| r.id)
            .collect()
    }

    /// Direct children of `parent`, in source order. Serves DSL-1.
    // DECISION: returns a Vec, not `impl Iterator`, so the signature does
    // not pin an iterator type; element counts are small.
    pub fn children(&self, parent: ElementId) -> Vec<ElementId> {
        self.elements
            .iter()
            .filter(|e| e.parent == Some(parent))
            .map(|e| e.id)
            .collect()
    }

    /// The view whose effective key is `key`. Serves DSL-6.
    pub fn view(&self, key: &str) -> Option<&ViewSpec> {
        self.views.iter().find(|v| v.key == key)
    }

    /// The indices into `element_styles` of every block that applies to
    /// `element` in light mode (`styles_for_in(element, Mode::Light)`), in
    /// apply order: by the element's tag order, blocks for the same tag in
    /// file order; later entries override earlier ones per property. `edit` reads it to find the block a property came from
    /// (DSL-13). Serves DSL-15.
    // DECISION: plain `usize` indices; a style id newtype would add pub
    // surface for a list only `edit` and the style panel read.
    pub fn styles_for(&self, element: ElementId) -> Vec<usize> {
        self.styles_for_in(element, Mode::Light)
    }

    /// The style of `element`: `styles_for` applied per property over
    /// `ResolvedStyle::default_for(kind)`, so a property no block sets
    /// takes its kind's default (shape `Person` for a person, else `Box`).
    /// Serves DSL-3, DSL-15, CORE-2, CORE-9.
    pub fn style_of(&self, element: ElementId) -> ResolvedStyle {
        self.style_of_in(element, Mode::Light)
    }

    /// `blocks` (indices into `element_styles`) applied per property, in
    /// order, over the kind default of `element`.
    fn resolve_element(&self, element: ElementId, blocks: &[usize]) -> ResolvedStyle {
        let mut style = ResolvedStyle::default_for(self.element(element).kind);
        for &i in blocks {
            let block = &self.element_styles[i];
            if let Some(c) = block.background {
                style.background = c.value;
            }
            if let Some(c) = block.color {
                style.color = c.value;
            }
            if let Some(s) = block.shape {
                style.shape = s.value;
            }
            if let Some(c) = block.stroke {
                style.stroke = Some(c.value);
            }
            if let Some(w) = block.stroke_width {
                style.stroke_width = w.value;
            }
            if let Some(w) = block.width {
                style.width = Some(round_up_to_grid(w.value));
            }
            if let Some(h) = block.height {
                style.height = Some(round_up_to_grid(h.value));
            }
            if let Some(f) = block.font_size {
                style.font_size = Some(f.value);
            }
            if let Some(b) = block.border {
                style.border = b.value;
            }
            if let Some(o) = block.opacity {
                style.opacity = o.value;
            }
            if let Some(m) = block.metadata {
                style.metadata = m.value;
            }
            if let Some(d) = block.description {
                style.description = d.value;
            }
            if let Some(icon) = &block.icon {
                // The asset loaded for this path, if any (P2-409, O3).
                if let Some(asset) = self.assets.iter().find(|a| a.path == icon.value) {
                    style.icon = Some(asset.clone());
                }
            }
        }
        style
    }

    /// `styles_for` in `mode`: the top-level blocks and those of `mode`'s
    /// `light`/`dark` block; theme styles (in both modes) resolve as if
    /// written before the file's first block, so for one tag the file's
    /// block beats a theme's and a later theme beats an earlier one
    /// (P2-13). Serves P2-21, P2-430, P2-13, DSL-15.
    // TODO(srs): P2-430. Readings taken (B styles-themes 10a, 10b): for
    // one tag and mode, a `light`/`dark` block beats a top-level block
    // wherever each is written; theme styles apply in both modes.
    pub fn styles_for_in(&self, element: ElementId, mode: Mode) -> Vec<usize> {
        let blocks: Vec<StyleKey<'_>> = self
            .element_styles
            .iter()
            .map(|s| StyleKey {
                tag: &s.tag.value,
                mode: s.mode,
                theme: s.theme,
            })
            .collect();
        apply_order(&self.element(element).tags, &blocks, mode)
    }

    /// `style_of` in `mode`: `styles_for_in` applied per property over
    /// `ResolvedStyle::default_for(kind)`. Serves P2-10, P2-400..P2-410,
    /// P2-430, DSL-15.
    pub fn style_of_in(&self, element: ElementId, mode: Mode) -> ResolvedStyle {
        self.resolve_element(element, &self.styles_for_in(element, mode))
    }

    /// The look of a response step's line (P3-201) in `mode`: the
    /// relationship's styles, then `Relationship/Response` styles, as the
    /// Structurizr renderer adds that tag for a response; dashed unless one
    /// of them sets `style`.
    // DECISION: dashed by default although c4rizer lines are solid
    // (CORE-10): Structurizr draws every line dashed by default, so its
    // responses are dashed, and the approver ruled them dashed; the
    // dash tells a response from a request between the same two boxes.
    pub(crate) fn response_style_in(&self, relationship: RelationshipId, mode: Mode) -> ResolvedRelationshipStyle {
        let blocks: Vec<StyleKey<'_>> = self
            .relationship_styles
            .iter()
            .map(|s| StyleKey {
                tag: &s.tag.value,
                mode: s.mode,
                theme: s.theme,
            })
            .collect();
        let mut tags = self.relationship(relationship).tags.clone();
        tags.push(RESPONSE_TAG.to_owned());
        let order = apply_order(&tags, &blocks, mode);
        let mut style = self.resolve_relationship(&order);
        if order.iter().all(|&i| self.relationship_styles[i].style.is_none()) {
            style.style = Border::Dashed;
        }
        style
    }

    /// True when a relationship style block that applies to
    /// `relationship` in `mode` writes `thickness` (a response step's
    /// lookup adds `Relationship/Response`, as `response_style_in`): the
    /// renderers then draw the resolved width as written, even the
    /// default 2px they otherwise draw 1.5px (redesign `edge-width`).
    // DECISION: any applying block counts, theme blocks included: a
    // theme's `thickness 2` is a written style too, and which block wins
    // does not matter, only that the width did not come from DEFAULT.
    pub(crate) fn relationship_thickness_written_in(
        &self,
        relationship: RelationshipId,
        mode: Mode,
        response: bool,
    ) -> bool {
        let r = self.relationship(relationship);
        let response_tag = response.then(|| RESPONSE_TAG.to_owned());
        r.tags.iter().chain(response_tag.iter()).any(|tag| {
            self.relationship_styles
                .iter()
                .any(|s| s.tag.value == *tag && s.mode.is_none_or(|m| m == mode) && s.thickness.is_some())
        })
    }

    /// `relationship_style_of_in(relationship, Mode::Light)`. Serves
    /// P2-12.
    pub fn relationship_style_of(&self, relationship: RelationshipId) -> ResolvedRelationshipStyle {
        self.relationship_style_of_in(relationship, Mode::Light)
    }

    /// The indices into `relationship_styles` that apply to
    /// `relationship` in `mode`, in apply order (as `styles_for_in`).
    fn relationship_styles_for(&self, relationship: RelationshipId, mode: Mode) -> Vec<usize> {
        let blocks: Vec<StyleKey<'_>> = self
            .relationship_styles
            .iter()
            .map(|s| StyleKey {
                tag: &s.tag.value,
                mode: s.mode,
                theme: s.theme,
            })
            .collect();
        apply_order(&self.relationship(relationship).tags, &blocks, mode)
    }

    /// `blocks` applied per property, in order, over
    /// `ResolvedRelationshipStyle::DEFAULT`.
    fn resolve_relationship(&self, blocks: &[usize]) -> ResolvedRelationshipStyle {
        let mut style = ResolvedRelationshipStyle::DEFAULT;
        for &i in blocks {
            let block = &self.relationship_styles[i];
            if let Some(v) = block.thickness {
                style.thickness = v.value;
            }
            if let Some(v) = block.color {
                style.color = Some(v.value);
            }
            if let Some(v) = block.style {
                style.style = v.value;
            }
            if let Some(v) = block.routing {
                style.routing = Some(v.value);
            }
            if let Some(v) = block.jump {
                style.jump = v.value;
            }
            if let Some(v) = block.font_size {
                style.font_size = Some(v.value);
            }
            if let Some(v) = block.width {
                style.width = Some(v.value);
            }
            if let Some(v) = block.position {
                style.position = Some(v.value);
            }
            if let Some(v) = block.opacity {
                style.opacity = v.value;
            }
        }
        style
    }

    /// The style of `relationship` in `mode`: `relationship_styles` for
    /// `Relationship` then its declared tags (DSL-15 order, same mode
    /// and theme rules as `styles_for_in`), applied per property over
    /// `ResolvedRelationshipStyle::DEFAULT`. Serves P2-12, P2-13, P2-430.
    pub fn relationship_style_of_in(
        &self,
        relationship: RelationshipId,
        mode: Mode,
    ) -> ResolvedRelationshipStyle {
        self.resolve_relationship(&self.relationship_styles_for(relationship, mode))
    }

    /// The look of a boundary carrying `tags` (`["Boundary",
    /// "Boundary:SoftwareSystem"]`, `["Group", "Group:<name>"]`) in
    /// `mode`: element styles for those tags, in that order, per
    /// property. `shape`, `width`, `height` and `description` are
    /// ignored; `Boundary:Enterprise` changes nothing (P2-440). Serves
    /// P2-411, P2-412, P2-440.
    pub fn boundary_style_in(&self, tags: &[&str], mode: Mode) -> ResolvedBoundaryStyle {
        let tags: Vec<String> = tags
            .iter()
            .filter(|t| **t != ENTERPRISE_BOUNDARY_TAG)
            .map(|t| (*t).to_owned())
            .collect();
        let blocks: Vec<StyleKey<'_>> = self
            .element_styles
            .iter()
            .map(|s| StyleKey {
                tag: &s.tag.value,
                mode: s.mode,
                theme: s.theme,
            })
            .collect();
        let mut style = ResolvedBoundaryStyle::DEFAULT;
        for i in apply_order(&tags, &blocks, mode) {
            let block = &self.element_styles[i];
            if let Some(v) = block.background {
                style.background = Some(v.value);
            }
            if let Some(v) = block.stroke {
                style.stroke = Some(v.value);
            }
            if let Some(v) = block.stroke_width {
                style.stroke_width = Some(v.value);
            }
            if let Some(v) = block.color {
                style.color = Some(v.value);
            }
            if let Some(v) = block.font_size {
                style.font_size = Some(v.value);
            }
            if let Some(v) = block.border {
                style.border = Some(v.value);
            }
            if let Some(v) = block.opacity {
                style.opacity = v.value;
            }
            if let Some(v) = block.metadata {
                style.metadata = v.value;
            }
        }
        style
    }

    /// The elements `view` includes after `include` and `exclude`
    /// (`*` expands as the keyword table says for the view kind and
    /// scope), sorted by id. Lifting, `focus` and `hide` are `view`'s job
    /// (CORE-5). Serves DSL-6, DSL-7, DSL-8.
    // DECISION: `include` and `exclude` act as sets: everything any
    // `include` selects, minus everything any `exclude` selects, whatever
    // the line order (Structurizr applies includes, then excludes).
    // DECISION: an explicitly included element is kept whatever its kind
    // (docs/workspace.dsl includes containers in component views).
    pub fn included(&self, view: &ViewSpec) -> Vec<ElementId> {
        if view.kind == ViewKind::Dynamic {
            // P3-213: a dynamic view draws exactly its step ends.
            let ends: BTreeSet<ElementId> = view
                .steps
                .iter()
                .flat_map(|s| [s.source.value, s.destination.value])
                .collect();
            return ends.into_iter().collect();
        }
        if view.kind == ViewKind::Deployment {
            return self.included_deployment(view);
        }
        if view.kind == ViewKind::Filtered {
            // P2-9, P2-308: the base view's elements that pass the filter.
            let (Some(base), Some(filter)) = (self.base_view(view), view.filter.as_ref()) else {
                return Vec::new();
            };
            return self
                .included(base)
                .into_iter()
                .filter(|&e| filter.passes(&self.element(e).tags))
                .collect();
        }
        let mut set = BTreeSet::new();
        for selector in &view.include {
            match &selector.value {
                // TODO(srs): P2-323, `*?` on other kinds. DECISION: reading
                // taken (B landscape-filtered 15; Structurizr 6.2.4, checked:
                // a landscape `*?` equals `*`): the elements of `*`; only
                // the lines differ, on C1..C3 (`view`).
                Selector::All | Selector::AllReluctant => self.include_all(view, &mut set),
                Selector::Element(id) => {
                    set.insert(*id);
                }
                // P2-334, P2-341: relationships add no box.
                Selector::Relationship(_) => {}
                Selector::Expression(e) if is_relationship_expression(e) => {}
                Selector::Expression(e) => {
                    set.extend(
                        self.select_elements(e)
                            .into_iter()
                            .filter_map(|x| self.drawn_level(view, x)),
                    );
                }
            }
        }
        for selector in &view.exclude {
            match &selector.value {
                Selector::All | Selector::AllReluctant => set.clear(),
                Selector::Element(id) => {
                    set.remove(id);
                }
                // P2-332, P2-341: they remove lines only (`view`).
                Selector::Relationship(_) => {}
                Selector::Expression(e) if is_relationship_expression(e) => {}
                // DECISION: an excluded expression removes the elements it
                // selects as they are, never an ancestor drawn for them
                // (Structurizr removes only elements in the view).
                Selector::Expression(e) => {
                    for x in self.select_elements(e) {
                        set.remove(&x);
                    }
                }
            }
        }
        set.into_iter().collect()
    }

    /// The environment a deployment view names (its name exactly, or its
    /// id ignoring case); `None` for any other view.
    fn view_environment(&self, view: &ViewSpec) -> Option<EnvironmentId> {
        let ViewScope::Deployment { environment, .. } = &view.scope else {
            return None;
        };
        self.environments
            .iter()
            .find(|e| e.name.value == environment.value)
            .or_else(|| {
                self.environments.iter().find(|e| {
                    e.key
                        .as_ref()
                        .is_some_and(|k| k.value.eq_ignore_ascii_case(&environment.value))
                })
            })
            .map(|e| e.id)
    }

    /// `included` for a deployment view (P3-110..P3-112): boxes are the
    /// instances and infrastructure nodes of the view's environment.
    // DECISION: an element of a static kind named in a deployment view's
    // `include` adds nothing (Structurizr draws only deployment elements
    // there).
    fn included_deployment(&self, view: &ViewSpec) -> Vec<ElementId> {
        let Some(env) = self.view_environment(view) else {
            return Vec::new();
        };
        let boxed = |e: &Element| {
            e.environment == Some(env)
                && matches!(
                    e.kind,
                    ElementKind::InfrastructureNode
                        | ElementKind::SoftwareSystemInstance
                        | ElementKind::ContainerInstance
                )
        };
        // The boxes a selected element stands for: itself, or every box
        // under a deployment node (P3-112).
        let expand = |id: ElementId| -> Vec<ElementId> {
            let e = self.element(id);
            if e.environment != Some(env) {
                return Vec::new();
            }
            if e.kind == ElementKind::DeploymentNode {
                return self
                    .elements
                    .iter()
                    .filter(|x| boxed(x) && self.is_inside(x.id, id))
                    .map(|x| x.id)
                    .collect();
            }
            if boxed(e) { vec![id] } else { Vec::new() }
        };
        let mut set = BTreeSet::new();
        for selector in &view.include {
            match &selector.value {
                Selector::All | Selector::AllReluctant => {
                    set.extend(self.deployment_all(view, env));
                }
                Selector::Element(id) => set.extend(expand(*id)),
                Selector::Relationship(_) => {}
                Selector::Expression(e) if is_relationship_expression(e) => {}
                Selector::Expression(e) => {
                    for x in self.select_elements(e) {
                        set.extend(expand(x));
                    }
                }
            }
        }
        for selector in &view.exclude {
            match &selector.value {
                Selector::All | Selector::AllReluctant => set.clear(),
                Selector::Element(id) => {
                    for x in expand(*id) {
                        set.remove(&x);
                    }
                }
                Selector::Relationship(_) => {}
                Selector::Expression(e) if is_relationship_expression(e) => {}
                Selector::Expression(e) => {
                    for x in self.select_elements(e) {
                        for y in expand(x) {
                            set.remove(&y);
                        }
                    }
                }
            }
        }
        set.into_iter().collect()
    }

    /// What `include *` selects on a deployment view of `env`: scoped to
    /// `S`, the container instances of `S`, every software system
    /// instance and every infrastructure node (P3-110); `*`, every
    /// container instance and infrastructure node, and every software
    /// system instance of a system with no container instance in the
    /// view (P3-111). Structurizr 6.2.4 (probed).
    fn deployment_all(&self, view: &ViewSpec, env: EnvironmentId) -> Vec<ElementId> {
        let ViewScope::Deployment { software_system, .. } = &view.scope else {
            return Vec::new();
        };
        let system = software_system.map(|s| s.value);
        let in_env = |e: &&Element| e.environment == Some(env);
        let containers: Vec<ElementId> = self
            .elements
            .iter()
            .filter(in_env)
            .filter(|e| e.kind == ElementKind::ContainerInstance)
            .filter(|e| {
                system.is_none_or(|s| e.instance_of.is_some_and(|c| self.element(c).parent == Some(s)))
            })
            .map(|e| e.id)
            .collect();
        let shown_systems: Vec<ElementId> = containers
            .iter()
            .filter_map(|&c| self.element(c).instance_of.and_then(|x| self.element(x).parent))
            .collect();
        self.elements
            .iter()
            .filter(in_env)
            .filter(|e| match e.kind {
                ElementKind::InfrastructureNode => true,
                ElementKind::ContainerInstance => containers.contains(&e.id),
                ElementKind::SoftwareSystemInstance => {
                    system.is_some() || e.instance_of.is_none_or(|s| !shown_systems.contains(&s))
                }
                _ => false,
            })
            .map(|e| e.id)
            .collect()
    }

    /// The element a view of `view`'s kind draws for `element` when an
    /// expression selects it: itself or its nearest ancestor of a kind the
    /// view draws; `None` when there is none, or when that is the scope of
    /// a container or component view (drawn as its boundary).
    // TODO(srs): P2-325, a coupled element nested below the view's level.
    // DECISION: reading taken (B landscape-filtered 13, a product choice):
    // drawn as its nearest ancestor the kind draws, as `include *` lifts
    // relationships (CORE-5); dropped when it has none. Structurizr 6.2.4
    // (checked) drops such an element instead (docs/deferred.md).
    fn drawn_level(&self, view: &ViewSpec, element: ElementId) -> Option<ElementId> {
        let draws = |k: ElementKind| match view.kind {
            ViewKind::SystemLandscape | ViewKind::SystemContext => {
                matches!(k, ElementKind::Person | ElementKind::SoftwareSystem)
            }
            ViewKind::Container => matches!(
                k,
                ElementKind::Person | ElementKind::SoftwareSystem | ElementKind::Container
            ),
            ViewKind::Component => matches!(
                k,
                ElementKind::Person
                    | ElementKind::SoftwareSystem
                    | ElementKind::Container
                    | ElementKind::Component
            ),
            ViewKind::Custom => k == ElementKind::Custom,
            ViewKind::Filtered | ViewKind::Image | ViewKind::Dynamic | ViewKind::Deployment => false,
        };
        let mut current = Some(element);
        while let Some(e) = current {
            if draws(self.element(e).kind) {
                break;
            }
            current = self.element(e).parent;
        }
        let found = current?;
        let boundary = matches!(view.kind, ViewKind::Container | ViewKind::Component)
            && view
                .scope
                .element()
                .is_some_and(|scope| found == scope || self.is_inside(scope, found));
        (!boundary).then_some(found)
    }

    /// True when `element` is strictly inside `ancestor`.
    fn is_inside(&self, element: ElementId, ancestor: ElementId) -> bool {
        let mut current = self.element(element).parent;
        while let Some(p) = current {
            if p == ancestor {
                return true;
            }
            current = self.element(p).parent;
        }
        false
    }

    /// The ancestor-or-self of `element` whose parent is `parent`
    /// (`None` meaning top level).
    fn ancestor_under(&self, element: ElementId, parent: Option<ElementId>) -> ElementId {
        let mut current = element;
        while self.element(current).parent != parent {
            match self.element(current).parent {
                Some(p) => current = p,
                None => break,
            }
        }
        current
    }

    /// What `include *` selects (keyword table): C1 the scope system and
    /// the people and systems linked to it; C2 the scope's containers and
    /// the people and systems linked to them; C3 the scope's components
    /// and the elements linked to them (containers of the same system,
    /// else people and systems). A relationship of a nested element
    /// counts for its ancestors (Structurizr implied relationships).
    fn include_all(&self, view: &ViewSpec, set: &mut BTreeSet<ElementId>) {
        // `include *` of the kinds not drawn yet selects nothing until
        // their rows land (P2-2).
        if view.kind == ViewKind::Custom {
            // P2-318: every custom element.
            set.extend(
                self.elements
                    .iter()
                    .filter(|e| e.kind == ElementKind::Custom)
                    .map(|e| e.id),
            );
            return;
        }
        // TODO(srs): P2-302. Structurizr 6.2.4 (checked) also adds every
        // custom element to a landscape `include *`; the SRS sentence
        // (settled, "every person and every software system") is followed
        // (docs/deferred.md).
        if view.kind == ViewKind::SystemLandscape {
            // P2-302: every person and software system, connected or not.
            set.extend(
                self.elements
                    .iter()
                    .filter(|e| matches!(e.kind, ElementKind::Person | ElementKind::SoftwareSystem))
                    .map(|e| e.id),
            );
            return;
        }
        let Some(scope) = view.scope.element().filter(|_| view.kind.drawable()) else {
            return;
        };
        let context = view.kind == ViewKind::SystemContext;
        if context {
            set.insert(scope);
        } else {
            set.extend(self.children(scope));
        }
        let inside = |e: ElementId| (context && e == scope) || self.is_inside(e, scope);
        let system = match view.kind {
            ViewKind::Component => self.element(scope).parent,
            _ => None,
        };
        // P2-7: for a relationship declared under `!impliedRelationships
        // false`, only one of a selected element itself, to the element
        // shown, counts (Structurizr for Java, checked).
        let member = |e: ElementId| {
            if context {
                e == scope
            } else {
                self.element(e).parent == Some(scope)
            }
        };
        for r in &self.relationships {
            let (s, d) = (r.source.value, r.destination.value);
            for (near, far) in [(s, d), (d, s)] {
                if !inside(near) || inside(far) || far == scope || self.is_inside(scope, far) {
                    continue;
                }
                let shown = match system {
                    Some(sys) if self.is_inside(far, sys) => self.ancestor_under(far, Some(sys)),
                    _ => self.ancestor_under(far, None),
                };
                let direct_only = r.implied_relationships == ImpliedRelationships::False;
                if direct_only && (shown != far || !member(near)) {
                    continue;
                }
                set.insert(shown);
            }
        }
    }

    /// The elements an element expression selects, sorted by id. Serves
    /// P2-166 (and landscape-filtered's view expressions).
    pub fn select_elements(&self, expression: &Expression) -> Vec<ElementId> {
        self.select_elements_impl(expression)
    }

    /// The relationships a relationship expression selects, sorted by id.
    /// Serves P2-168 (and landscape-filtered's view expressions).
    pub fn select_relationships(&self, expression: &Expression) -> Vec<RelationshipId> {
        self.select_relationships_impl(expression)
    }

    /// The base view of a `filtered` view: the view whose key is
    /// `view.filter.base`; `None` for any other kind or an unknown key.
    /// Serves P2-9, P2-306.
    pub fn base_view(&self, view: &ViewSpec) -> Option<&ViewSpec> {
        let filter = view.filter.as_ref().filter(|_| view.kind == ViewKind::Filtered)?;
        self.view(&filter.base.value)
    }

    /// The keys of the views that are the base of at least one `filtered`
    /// view, in declaration order, each once: the editor leaves them out
    /// of its view list (EDIT-8). Serves P2-314.
    // TODO(srs): P2-314, `render` and EDIT-15/16 on a base view. Reading
    // taken (B landscape-filtered 5): `c4rizer render` still writes
    // `<baseKey>.svg`; EDIT-15 and EDIT-16 may open it, and it then shows
    // in the list until another view is opened.
    pub fn filtered_bases(&self) -> Vec<&str> {
        let bases: BTreeSet<&str> = self
            .views
            .iter()
            .filter(|v| v.kind == ViewKind::Filtered)
            .filter_map(|v| v.filter.as_ref())
            .map(|f| f.base.value.as_str())
            .collect();
        self.views
            .iter()
            .map(|v| v.key.as_str())
            .filter(|k| bases.contains(k))
            .collect()
    }

    /// The key of the view the editor opens when no last-viewed view
    /// applies (EDIT-13): the view carrying `default`; `None` when no view
    /// does. Serves P2-338.
    // TODO(srs): P2-338, several `default` views. Reading taken (B
    // landscape-filtered 16, Structurizr: each `default` sets the view
    // set's default key): the last one declared wins.
    pub fn default_view_key(&self) -> Option<&str> {
        // DECISION: the last `default` declared wins, as each Structurizr
        // `default` line sets the view set's default key again.
        self.views
            .iter()
            .rev()
            .find(|v| v.default.is_some())
            .map(|v| v.key.as_str())
    }

    /// The canonical edit key of `id`: its id under flat identifiers, its
    /// dotted full id under `!identifiers hierarchical` (groups add no
    /// segment, P2-131); `None` for an element declared without an id
    /// (P2-145). `Model::find(full_key)` gives `id` back. Serves P2-6,
    /// P2-142, P2-145.
    // TODO(srs): P2-145, how `at`, `size` and edits address an anonymous
    // box. Reading taken (B dsl-breadth 10): never; it is auto-placed on
    // every build and the editor refuses moves, resizes and per-box style
    // edits on it, never adding an identifier (EDIT-21).
    #[must_use]
    pub fn full_key(&self, id: ElementId) -> Option<ElementKey> {
        self.full_id(id).map(ElementKey)
    }

    /// `full_key` as text.
    fn full_id(&self, id: ElementId) -> Option<String> {
        let element = self.element(id);
        let Some(key) = element.key.as_ref() else {
            // P3-117: an instance without an id is `<element key>_<n>`.
            // TODO(srs): P3-117, the key of a deployment or infrastructure
            // node without an id. Reading taken: none, as for any element
            // without an id (P2-145): the editor cannot select or move it
            // (docs/deferred.md).
            let of = element.instance_of?;
            return Some(format!("{}_{}", self.full_id(of)?, element.instance_number?));
        };
        if self.identifiers == Identifiers::Flat {
            return Some(key.value.clone());
        }
        // DECISION: an ancestor without an id adds no segment.
        let mut parts = vec![key.value.as_str()];
        let mut current = element.parent;
        while let Some(p) = current {
            let parent = self.element(p);
            if let Some(k) = &parent.key {
                parts.push(&k.value);
            }
            current = parent.parent;
        }
        // P3-109: a deployment id starts with its environment's id.
        let top = self.ancestor_under(id, None);
        if let Some(env) = self.element(top).environment
            && let Some(k) = self.environments.get(env.0 as usize).and_then(|e| e.key.as_ref())
        {
            parts.push(&k.value);
        }
        parts.reverse();
        Some(parts.join("."))
    }

    /// The elements `view` includes that have no `at` line, sorted by
    /// id: these are auto-placed (CORE-22). Serves DSL-8.
    pub fn unplaced(&self, view: &ViewSpec) -> Vec<ElementId> {
        self.included(view)
            .into_iter()
            .filter(|&e| view.position_of(e).is_none())
            .collect()
    }
}

/// What `parse` returns: the syntax trees and the model built from them.
/// `#[non_exhaustive]` so `!include` (P2-5) can add data without an API
/// change. Serves DSL-1..8, DSL-14, DSL-16.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Parsed {
    /// One tree per source file, indexed by `FileId`: in phase 1 exactly
    /// one, the root file (`trees[0]`). Kept for `edit` (DSL-9, DSL-10,
    /// DSL-13); `!include` (P2-5) appends the included files.
    pub trees: Vec<SyntaxTree>,
    /// The typed model.
    pub model: Model,
    /// The path of each tree, indexed by `FileId`; empty when parsed
    /// without a path (`parse`, or `parse_with(_, None, _)`). The editor
    /// watches each of them (P2-141). Serves P2-135, P2-136, P2-141.
    pub paths: Vec<PathBuf>,
    /// What `reparse` needs to rebuild the model without I/O.
    replay: load::Replay,
}

/// Parses `source`, the root file (`FileId::ROOT`), into its syntax tree
/// and model. The tree is kept for `edit` (DSL-9, DSL-10, DSL-13). Serves
/// DSL-1..8, DSL-14.
///
/// # Errors
///
/// Returns a `ModelErrorKind::Syntax` error when `text::parse` fails
/// (DSL-4), else any error of `Model::from_tree`.
pub fn parse(source: &str) -> Result<Parsed, ModelError> {
    parse_with(source, None, &mut NoFiles)
}

/// Parses `source`, the root file, at `path` (`None`: stdin or in
/// memory; then any file-relative construct is `NeedsFile`), reading
/// `!include` files and `workspace extends` bases through `loader` and
/// substituting `!const`/`!var`/`${NAME}` as each line is reached
/// (environment through `Loader::env`). Each file read gets the next
/// `FileId` from 1 in first-read order and its own tree. Serves P2-5,
/// P2-16, P2-133..P2-138, P2-150..P2-152, P2-164, P2-165.
// TODO(srs): P2-133, order, recursion and extensions of a directory
// include. Reading taken (B dsl-breadth 5): every regular file under it,
// recursively, ascending relative path compared byte by byte, names
// starting with `.` skipped.
// TODO(srs): P2-138 and P2-16, HTTPS includes, URL and JSON bases.
// Reading taken (B dsl-breadth 6): `UrlNotSupported` naming the URL; a
// JSON base is `LaterPhase` (phase 3).
// TODO(srs): P2-152, a constant or variable and an environment variable
// with the same name. Reading taken (B dsl-breadth 4): the constant or
// variable defined earlier in the file wins.
// TODO(srs): P2-151, redefining a `!const`. Reading taken (B dsl-breadth
// 2): `DuplicateConstant` naming both lines; a `!var` may be redefined.
// TODO(srs): P2-148, text block value rules (/dsl/text-blocks is 404).
// Reading taken (B dsl-breadth 1, checked against Structurizr for Java):
// the lines between the delimiters, common leading whitespace of
// non-blank lines removed, no final line break; `\"` gives `"` (Java
// does so, B 1 said no escapes), no other escape.
///
/// # Errors
///
/// `ModelErrorKind::Syntax` or any `Model::from_tree` error, in the file
/// it occurs in (P2-136); `FileRead`, `PathOutsideRoot`, `IncludeCycle`,
/// `InsecureUrl`, `UrlNotSupported`, `NeedsFile`, `DuplicateConstant`.
pub fn parse_with(
    source: &str,
    path: Option<&Path>,
    loader: &mut dyn Loader,
) -> Result<Parsed, ModelError> {
    load::parse_with(source, path, loader)
}

/// Reads `source`, a Structurizr workspace JSON (what `export -f json`
/// writes, P3-413), into the model the DSL it was exported from gives:
/// same element names, kinds, parents, tags, relationships, views and
/// styles (P3-416), each view keeping its element positions as `at` and
/// `size` lines (P3-417). Unknown keys are ignored (P3-418). `loader`
/// serves the same file access as `parse_with` (P3-416: callers pass
/// what they pass there). Spans of the items point into the DSL text
/// written from `source` (as `FileId::ROOT`), not into the JSON; a JSON
/// syntax error carries the JSON's own line and column (approver
/// ruling). Serves P3-416, P3-417, P3-418.
///
/// # Errors
///
/// A `ModelError` with the line and column in `source` when it is not
/// RFC 8259 JSON (or not UTF-8), or is JSON but not a workspace (no
/// top-level object with a `model` or `views` object) (P3-418).
pub fn parse_json(source: &[u8], loader: &mut dyn Loader) -> Result<Model, ModelError> {
    let text = std::str::from_utf8(source).map_err(|e| {
        let at = index_u32(e.valid_up_to());
        let shown = String::from_utf8_lossy(source);
        json_error("it is not UTF-8 text".into(), at, text::position(&shown, at))
    })?;
    let mut dsl = json::to_dsl(text).map_err(|e| {
        let at = offset_of(text, e.position);
        json_error(e.message, at, e.position)
    })?;
    // P3-318: a generated `via` line the view refuses (a line it does not
    // draw, from hostile or hand-edited JSON) is dropped and the text read
    // again, never failing the workspace.
    // DECISION: at most `JSON_VIA_RETRIES` lines are dropped one by one;
    // past that every generated `via` line goes at once, so a JSON full of
    // bad vertices costs a bounded number of parses.
    // DECISION: the first `JSON_VIA_RETRIES` refused lines are dropped one
    // by one (the common case: a few bad lines, one parse each); past that
    // the generated `via` lines are put back in groups (`restore_vias`),
    // so only the refused ones go, in at most twice as many parses as
    // there are lines.
    for _ in 0..JSON_VIA_RETRIES {
        match load::parse_with(&dsl, None, loader) {
            Ok(p) => return Ok(p.model),
            Err(e) => {
                let start = (e.span.file == FileId::ROOT).then(|| e.span.range().start);
                let Some(line) = start.and_then(|at| generated_via_line(&dsl, at)) else {
                    return Err(e);
                };
                dsl.replace_range(line, "");
            }
        }
    }
    match load::parse_with(&dsl, None, loader) {
        Ok(p) => Ok(p.model),
        Err(e) if e.span.file == FileId::ROOT && generated_via_line(&dsl, e.span.range().start).is_some() => {
            restore_vias(&dsl, loader)
        }
        Err(e) => Err(e),
    }
}

/// `dsl` with only the `via` lines the views accept: every `via` line is
/// taken out, then put back in halves, a half that is refused being split
/// again down to single lines, which are dropped.
fn restore_vias(dsl: &str, loader: &mut dyn Loader) -> Result<Model, ModelError> {
    let lines: Vec<&str> = dsl.split_inclusive('\n').collect();
    let vias: Vec<usize> = (0..lines.len()).filter(|&i| is_via_line(lines[i])).collect();
    let mut keep: Vec<bool> = lines.iter().map(|l| !is_via_line(l)).collect();
    let text = |keep: &[bool]| -> String { lines.iter().zip(keep).filter(|(_, k)| **k).map(|(l, _)| *l).collect() };
    let mut pending: Vec<&[usize]> = vec![&vias];
    while let Some(group) = pending.pop() {
        if group.is_empty() {
            continue;
        }
        for &i in group {
            keep[i] = true;
        }
        if load::parse_with(&text(&keep), None, loader).is_ok() {
            continue;
        }
        for &i in group {
            keep[i] = false;
        }
        if let [_] = group {
            continue;
        }
        let (first, second) = group.split_at(group.len() / 2);
        // The first half goes in before the second is tried.
        pending.push(second);
        pending.push(first);
    }
    load::parse_with(&text(&keep), None, loader).map(|p| p.model)
}

/// How many refused `via` lines `parse_json` drops one at a time.
const JSON_VIA_RETRIES: usize = 16;

/// True when `line` of DSL text written from JSON is a `via` line (not an
/// element declared with the identifier `via`).
fn is_via_line(line: &str) -> bool {
    let mut words = line.split_whitespace();
    // DECISION: `via -> db` is a relationship from an element identified
    // `via`, as `via = ...` is a declaration of one.
    // DECISION: `via -> db` is a relationship from an element identified
    // `via`, as `via = ...` is a declaration of one.
    words.next() == Some("via") && words.next().is_some_and(|w| w != "=" && w != "->")
}

/// The byte range of the whole line holding byte `at` of `dsl`, its
/// line ending included, when it is a `via` line.
fn generated_via_line(dsl: &str, at: usize) -> Option<std::ops::Range<usize>> {
    let at = at.min(dsl.len());
    let start = dsl[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = dsl[at..].find('\n').map_or(dsl.len(), |i| at + i + 1);
    is_via_line(&dsl[start..end]).then_some(start..end)
}

/// The byte offset of `position` (1-based line and character column) in
/// `source`, clamped to its end.
fn offset_of(source: &str, position: Position) -> u32 {
    let mut line = 1;
    let mut column = 1;
    for (i, c) in source.char_indices() {
        if line == position.line && column == position.column {
            return index_u32(i);
        }
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    index_u32(source.len())
}

/// A JSON workspace that cannot be read (P3-418): `FileFormat`, at
/// `at` in the JSON.
fn json_error(reason: String, at: u32, position: Position) -> ModelError {
    ModelError {
        kind: ModelErrorKind::FileFormat {
            path: "the workspace JSON".into(),
            reason,
        },
        span: Span::new(FileId::ROOT, at, at),
        position,
    }
}

/// The Structurizr canonical name of element `id`: `<Type>://<names>`,
/// the names of its ancestors and itself joined with `.`, each with `.`
/// and `/` removed (`DefaultLayoutMergeStrategy`, P2-506, P3-427).
/// Serves P3-427, P3-416.
// TODO(srs): P3-427, deployment elements. Reading taken: the doc
// sentence above for every kind (ancestors joined with `.`), the type
// being the kind's name without spaces (`DeploymentNode://...`).
pub fn canonical_name(model: &Model, id: ElementId) -> String {
    let mut names = Vec::new();
    let mut current = Some(id);
    while let Some(e) = current {
        let element = model.element(e);
        names.push(element.name.value.replace(['.', '/'], ""));
        current = element.parent;
    }
    names.reverse();
    let kind = match model.element(id).kind {
        ElementKind::Custom => "Custom",
        k => k.default_tag(),
    };
    format!("{}://{}", kind.replace(' ', ""), names.join("."))
}

/// `parsed` with the tree of `file` replaced by `source`, the model built
/// again from the trees in memory with no I/O: the files read, the
/// includes and the environment values of the first parse are kept.
/// `edit::apply` re-parses through it between edits (P2-139). Serves
/// P2-139, P2-140.
///
/// # Errors
///
/// A syntax or model error in the new text, as `parse_with` gives it.
pub(crate) fn reparse(parsed: &Parsed, file: FileId, source: &str) -> Result<Parsed, ModelError> {
    load::reparse(parsed, file, source)
}

/// Why a file could not become a model (DSL-4, DSL-5). `Display` names
/// the construct or id and says what to change (SRS 5.1).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModelErrorKind {
    /// A lexing or brace error from `text`.
    Syntax(ParseErrorKind),
    /// A construct that is in no phase of the grammar, or a dropped
    /// prototype option (`tries`, `seed`). DSL-4.
    UnknownConstruct {
        /// The construct as written (first word of the statement).
        construct: String,
    },
    /// A construct planned for a later phase (`!include`, `group`,
    /// `deployment`, `dynamic`, `systemLandscape`, `filtered`, `themes`,
    /// expressions in `include`, an element without `<id> =`, a named
    /// relationship `r = a -> b`, ...): the error names the phase (DSL-4,
    /// DSL-7).
    LaterPhase {
        /// The construct as written.
        construct: String,
        /// The phase it is planned for (2 or 3, SRS section 2).
        phase: u8,
    },
    /// A statement lacks a required argument (e.g. a `person` with no
    /// name, an `at` line with no y). DSL-4.
    MissingArgument {
        /// The construct.
        construct: String,
        /// What is missing, e.g. `"name"`.
        argument: &'static str,
    },
    /// An argument is present but not valid: bad number, bad colour,
    /// unknown shape, `on`/`off` expected, extra argument. DSL-4.
    InvalidValue {
        /// The construct.
        construct: String,
        /// The value as written.
        value: String,
    },
    /// A container outside a software system, a component outside a
    /// container, or any element inside a person or component. DSL-1.
    InvalidNesting {
        /// The misplaced element's kind.
        kind: ElementKind,
        /// The kind of the enclosing element, `None` at top level.
        parent: Option<ElementKind>,
    },
    /// An id used but never declared (DSL-5).
    UndeclaredId {
        /// The id or dotted path as written.
        id: String,
    },
    /// An element id declared twice; the error's span is the second
    /// declaration (Review Focus: names both lines).
    DuplicateId {
        /// The id.
        id: String,
        /// The first declaration.
        first: Span,
        /// Line and column of `first`, for `Display`.
        first_position: Position,
    },
    /// A view key used twice (Review Focus).
    DuplicateViewKey {
        /// The key.
        key: String,
        /// The first view with it.
        first: Span,
        /// Line and column of `first`, for `Display`.
        first_position: Position,
    },
    /// A view scope of the wrong kind: context and container views need
    /// a software system, component views a container. DSL-6.
    InvalidScope {
        /// The view kind.
        kind: ViewKind,
        /// The scope id as written.
        scope: String,
    },
    /// An `at` or `size` line for an element the view does not include
    /// (Review Focus, DSL-7).
    NotInView {
        /// The element id as written.
        id: String,
        /// The view key.
        view: String,
    },
    /// A second `at` or `size` line for the same element in one view
    /// (DSL-7), or a second view line that takes one value (`title`,
    /// `autoLayout`, an option).
    DuplicateLine {
        /// The construct, e.g. `at`.
        construct: &'static str,
        /// The element id as written, empty for a view line.
        id: String,
    },
    /// `this`, or a relationship with no source, outside an element
    /// block (Review Focus).
    NoEnclosingElement,
    /// A `Loader` read failed: an included file, a base workspace, a
    /// `!docs`/`!adrs` path (P2-5, P2-16, P2-159).
    FileRead {
        /// The path as written.
        path: String,
        /// The I/O error, in words.
        error: String,
    },
    /// A file read through the `Loader` is not what it must be (not
    /// UTF-8 for a DSL file; styles-themes and landscape-filtered add
    /// malformed themes and assets).
    FileFormat {
        /// The path as written.
        path: String,
        /// Why it was refused, in words.
        reason: String,
    },
    /// A relative path that resolves outside the including file's
    /// directory and its subdirectories (P2-134).
    PathOutsideRoot {
        /// The path as written.
        path: String,
    },
    /// An include cycle, a self-include included; names the file that
    /// closes the cycle (P2-137).
    IncludeCycle {
        /// That file's path.
        path: String,
    },
    /// `!include` of a URL that is not `https://` (P2-138).
    InsecureUrl {
        /// The URL.
        url: String,
    },
    /// An HTTPS `!include` or `workspace extends <url>`: download the
    /// file and include it locally (B dsl-breadth 6).
    UrlNotSupported {
        /// The URL.
        url: String,
    },
    /// A file-relative construct (`!include`, `workspace extends`) in a
    /// file parsed without a path.
    NeedsFile {
        /// The construct.
        construct: String,
    },
    /// Two people or software systems with the same name, two containers
    /// with the same name in one software system, or two components with
    /// the same name in one container (P2-177); span is the second.
    // TODO(srs): P2-177 is an open question in the SRS (Structurizr for
    // Java model rule). Reading taken (B dsl-breadth 13): this error.
    DuplicateName {
        /// The name.
        name: String,
        /// The first declaration.
        first: Span,
        /// Line and column of `first`.
        first_position: Position,
    },
    /// A name defined by `!const` defined again (P2-151).
    DuplicateConstant {
        /// The name.
        name: String,
        /// The first definition.
        first: Span,
        /// Line and column of `first`.
        first_position: Position,
    },
    /// An element archetype whose base is not an element kind, `group`
    /// or another archetype (P2-158).
    InvalidArchetype {
        /// The base as written.
        base: String,
    },
    /// A `--name->` naming no relationship archetype, or an element
    /// declared from an unknown archetype (P2-158).
    UnknownArchetype {
        /// The name as written.
        name: String,
    },
    /// A `group` nested in a `group` with no `structurizr.groupSeparator`
    /// model property (P2-129, B dsl-breadth 14b).
    NestedGroupNeedsSeparator,
    /// The model breaks `configuration { scope ... }` (P2-175).
    ScopeViolation {
        /// The scope.
        scope: ConfigurationScope,
        /// The offending software system's id or name.
        element: String,
    },
    /// An expression mixing `&&` and `||` (P2-166, P2-168).
    MixedOperators {
        /// The expression as written.
        expression: String,
    },
    /// A `filtered` view whose base key names no view declared before it
    /// (P2-306, DSL-5).
    UnknownView {
        /// The base key as written.
        key: String,
    },
    /// A `filtered` view whose base is not a system landscape, system
    /// context, container or component view (P2-306).
    InvalidBaseView {
        /// The base key as written.
        key: String,
        /// The base view's kind.
        kind: ViewKind,
    },
    /// An element other than a custom element (`element`, P2-17) in a
    /// custom view's `include` (P2-317).
    NotCustomElement {
        /// The id as written.
        id: String,
    },
    /// A dynamic step `a -> b` with no relationship from `a` to `b` in the
    /// model, direct or implied, with the written technology if any
    /// (P3-201).
    // APPROVER: new pub item
    NoRelationship {
        /// The source element's name.
        source: String,
        /// The destination element's name.
        destination: String,
        /// The step's written technology, if any.
        technology: Option<String>,
    },
    /// Two `via` lines for one drawn line in one view (P3-302); the
    /// error's span is the second line.
    DuplicateVia {
        /// The line as written in the second `via` line (`a -> b`).
        line: String,
        /// The first `via` line for it.
        first: Span,
        /// Line and column of `first`, for `Display`.
        first_position: Position,
    },
    /// A dynamic step end the view's scope does not permit (P3-204..P3-207).
    // APPROVER: new pub item
    NotPermittedInView {
        /// The element's name.
        name: String,
        /// Why, as Structurizr 6.2.4 words it.
        reason: String,
    },
    /// A relationship in a deployment block that Structurizr does not
    /// permit (between two software system or container instances,
    /// P3-107), or a `-/>` with no copied relationship to remove
    /// (P3-108). Serves P3-107, P3-108.
    // APPROVER: new pub item
    DeploymentRelationship {
        /// The source as written.
        source: String,
        /// The destination as written.
        destination: String,
        /// Why: `"not permitted"` or `"no copied relationship"`.
        reason: &'static str,
    },
    /// An `at` or `size` line naming a deployment node, whose boundary
    /// follows its contents (P3-122). Serves P3-122.
    // APPROVER: new pub item
    DeploymentNodeLayout {
        /// The id as written.
        id: String,
    },
}

/// A model error: what, where, and its line and column (DSL-4, DSL-5).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelError {
    /// What went wrong.
    pub kind: ModelErrorKind,
    /// The offending token or statement; names its file (DSL-16).
    pub span: Span,
    /// Line and column of `span.start`.
    pub position: Position,
}

impl fmt::Display for ModelError {
    /// `line:column: <what> — <what to change>` (DSL-4, SRS 5.1). The
    /// caller prefixes the path of `span.file`.
    // DECISION: one arm per kind keeps every message next to its kind;
    // the phase-2 kinds push it past clippy's 100 lines. Step 5 may move
    // their text into a helper instead.
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let ModelErrorKind::Syntax(kind) = self.kind {
            let error = ParseError {
                kind,
                span: self.span,
                position: self.position,
            };
            return error.fmt(f);
        }
        let Position { line, column } = self.position;
        write!(f, "{line}:{column}: ")?;
        match &self.kind {
            ModelErrorKind::Syntax(_) => Ok(()),
            ModelErrorKind::UnknownConstruct { construct } => write!(
                f,
                "`{construct}` is not part of the phase-1 language here — remove it or check its spelling and place against the keyword table"
            ),
            ModelErrorKind::LaterPhase { construct, phase } => write!(
                f,
                "`{construct}` is planned for phase {phase} and not supported yet — remove it or rewrite it with phase-1 keywords"
            ),
            ModelErrorKind::MissingArgument {
                construct,
                argument,
            } => write!(f, "`{construct}` needs a {argument} — add it"),
            ModelErrorKind::InvalidValue { construct, value } => write!(
                f,
                "`{value}` is not a valid value for `{construct}` — write what the keyword table gives for it"
            ),
            ModelErrorKind::InvalidNesting { kind, parent } => {
                let place = match kind {
                    ElementKind::Person | ElementKind::SoftwareSystem => "directly into `model`",
                    ElementKind::Container => "into a software system block",
                    ElementKind::Component => "into a container block",
                    ElementKind::Custom => "directly into `model` or a group there",
                    ElementKind::DeploymentNode => "into a deployment environment or node",
                    ElementKind::InfrastructureNode
                    | ElementKind::SoftwareSystemInstance
                    | ElementKind::ContainerInstance => "into a deployment node",
                };
                match parent {
                    Some(p) => write!(
                        f,
                        "a {} cannot be declared inside a {} — move it {place}",
                        kind_name(*kind),
                        kind_name(*p)
                    ),
                    None => write!(
                        f,
                        "a {} cannot be declared at the top of `model` — move it {place}",
                        kind_name(*kind)
                    ),
                }
            }
            ModelErrorKind::UndeclaredId { id } => write!(
                f,
                "`{id}` is not declared — declare it in `model` or fix the spelling"
            ),
            ModelErrorKind::DuplicateId {
                id, first_position, ..
            } => write!(
                f,
                "`{id}` is already declared at {}:{} — give one of the two elements another id",
                first_position.line, first_position.column
            ),
            ModelErrorKind::DuplicateViewKey {
                key,
                first_position,
                ..
            } => write!(
                f,
                "view key `{key}` is already used at {}:{} — give one of the two views another key",
                first_position.line, first_position.column
            ),
            ModelErrorKind::InvalidScope { kind, scope } => {
                let needed = match kind {
                    ViewKind::SystemContext | ViewKind::Container | ViewKind::Deployment => {
                        "software system"
                    }
                    ViewKind::Component => "container",
                    ViewKind::Dynamic => "software system or container",
                    ViewKind::Image => "element",
                    ViewKind::SystemLandscape | ViewKind::Filtered | ViewKind::Custom => {
                        "nothing"
                    }
                };
                write!(
                    f,
                    "a `{}` view needs a {needed} as its scope and `{scope}` is not one — name a {needed}",
                    kind.keyword()
                )
            }
            ModelErrorKind::NotInView { id, view } => write!(
                f,
                "`{id}` is not included in view `{view}` — include it in the view or remove this line"
            ),
            ModelErrorKind::DuplicateLine { construct, id } if id.is_empty() => write!(
                f,
                "a second `{construct}` line in this block — keep only one"
            ),
            ModelErrorKind::DuplicateLine { construct, id } => write!(
                f,
                "a second `{construct}` line for `{id}` in this view — keep only one"
            ),
            ModelErrorKind::NoEnclosingElement => f.write_str(
                "`this` and a relationship without a source only work inside an element block — name the source element",
            ),
            ModelErrorKind::DuplicateConstant {
                name,
                first_position,
                ..
            } => write!(
                f,
                "constant `{name}` is already defined at {}:{} — a constant cannot be redefined; use `!var` for a value that changes",
                first_position.line, first_position.column
            ),
            ModelErrorKind::ScopeViolation { scope, element } => {
                let (word, rule) = match scope {
                    ConfigurationScope::Landscape => (
                        "landscape",
                        "no software system may have containers or documentation",
                    ),
                    ConfigurationScope::SoftwareSystem => (
                        "softwaresystem",
                        "only one software system may have containers or documentation",
                    ),
                    ConfigurationScope::None => ("none", "nothing is checked"),
                };
                write!(
                    f,
                    "`{element}` breaks `scope {word}`: {rule} — change the scope or move the detail"
                )
            }
            other => phase_2_message(other, f),
        }
    }
}

/// The `Display` text of the phase-2 error kinds (SRS 5.1: what, then
/// what to change).
fn phase_2_message(kind: &ModelErrorKind, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match kind {
        ModelErrorKind::FileRead { path, error } => write!(
            f,
            "cannot read `{path}`: {error} — check the path, relative to the file that names it"
        ),
        ModelErrorKind::FileFormat { path, reason } => {
            write!(f, "`{path}` cannot be used: {reason} — fix or replace the file")
        }
        ModelErrorKind::PathOutsideRoot { path } => write!(
            f,
            "`{path}` lies outside the folder of the file that names it — move the file into that folder or one below it"
        ),
        ModelErrorKind::IncludeCycle { path } => write!(
            f,
            "`{path}` includes itself, directly or through other files — remove one `!include` of the cycle"
        ),
        ModelErrorKind::InsecureUrl { url } => write!(
            f,
            "`{url}` is not an https:// URL — use https://, or download the file and include it by path"
        ),
        ModelErrorKind::UrlNotSupported { url } => write!(
            f,
            "`{url}` is a URL and this version reads local files only — download the file and include it by path"
        ),
        ModelErrorKind::NeedsFile { construct } => write!(
            f,
            "`{construct}` reads a file relative to this one, and this text has no file — open it from a file"
        ),
        ModelErrorKind::DuplicateName {
            name,
            first_position,
            ..
        } => write!(
            f,
            "an element named `{name}` is already declared here at {}:{} — give one of the two another name",
            first_position.line, first_position.column
        ),
        ModelErrorKind::InvalidArchetype { base } => write!(
            f,
            "`{base}` cannot be an archetype's base — base it on an element kind, `group`, `element` or another archetype"
        ),
        ModelErrorKind::UnknownArchetype { name } => write!(
            f,
            "`{name}` is not a declared archetype — declare it in `archetypes` or fix the spelling"
        ),
        ModelErrorKind::NestedGroupNeedsSeparator => f.write_str(
            "a group inside a group needs a separator — add `structurizr.groupSeparator` to the `properties` of `model`",
        ),
        ModelErrorKind::MixedOperators { expression } => write!(
            f,
            "`{expression}` mixes `&&` and `||` — split it into separate lines"
        ),
        ModelErrorKind::UnknownView { key } => write!(
            f,
            "no view with key `{key}` is declared before this line — declare the base view first or fix the key"
        ),
        ModelErrorKind::InvalidBaseView { key, kind } => write!(
            f,
            "`{key}` is a {} view, which cannot be the base of a filtered view — use a systemLandscape, systemContext, container or component view",
            kind.keyword()
        ),
        ModelErrorKind::NotCustomElement { id } => write!(
            f,
            "`{id}` is not a custom element — a custom view can only include elements declared with `element`"
        ),
        other => phase_3_message(other, f),
    }
}

/// The `Display` text of the phase-3 error kinds (SRS 5.1: what, then
/// what to change).
// DECISION: split from `phase_2_message` at the phase-3 kinds to keep each
// under clippy's line limit.
fn phase_3_message(kind: &ModelErrorKind, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match kind {
        ModelErrorKind::NoRelationship {
            source,
            destination,
            technology,
        } => match technology {
            Some(t) => write!(
                f,
                "a relationship between \"{source}\" and \"{destination}\" with technology \"{t}\" does not exist in the model — declare it in `model` or change the step"
            ),
            None => write!(
                f,
                "a relationship between \"{source}\" and \"{destination}\" does not exist in the model — declare it in `model` or change the step"
            ),
        },
        ModelErrorKind::NotPermittedInView { name, reason } => {
            write!(f, "\"{name}\": {reason} — change the step or the view's scope")
        }
        ModelErrorKind::DeploymentRelationship {
            source,
            destination,
            reason,
        } => {
            if *reason == "not permitted" {
                write!(
                    f,
                    "a relationship between `{source}` and `{destination}` is not permitted — instances get their relationships from the model; write it between the elements"
                )
            } else {
                write!(
                    f,
                    "there is no relationship between instances `{source}` and `{destination}` to remove — check both ends"
                )
            }
        }
        ModelErrorKind::DeploymentNodeLayout { id } => write!(
            f,
            "`{id}` is a deployment node — its boundary follows its contents; place the instances and infrastructure nodes inside it instead"
        ),
        ModelErrorKind::DuplicateVia {
            line, first_position, ..
        } => write!(
            f,
            "`{line}` already has a `via` line at {}:{} — keep one `via` line per line and list every vertex in it",
            first_position.line, first_position.column
        ),
        _ => f.write_str("the file cannot become a model here"),
    }
}

impl std::error::Error for ModelError {}

impl From<ParseError> for ModelError {
    /// Wraps a `text` error as `ModelErrorKind::Syntax`, same span and
    /// position. Serves DSL-4.
    fn from(error: ParseError) -> ModelError {
        ModelError {
            kind: ModelErrorKind::Syntax(error.kind),
            span: error.span,
            position: error.position,
        }
    }
}

// ------------------------------------------------------------- building

/// Lowercase words for an element kind, for error text.
const fn kind_name(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Person => "person",
        ElementKind::SoftwareSystem => "software system",
        ElementKind::Container => "container",
        ElementKind::Component => "component",
        ElementKind::Custom => "custom element",
        ElementKind::DeploymentNode => "deployment node",
        ElementKind::InfrastructureNode => "infrastructure node",
        ElementKind::SoftwareSystemInstance => "software system instance",
        ElementKind::ContainerInstance => "container instance",
    }
}

/// The element kind whose keyword is `word`, ignoring case.
fn element_kind(word: &str) -> Option<ElementKind> {
    [
        ElementKind::Person,
        ElementKind::SoftwareSystem,
        ElementKind::Container,
        ElementKind::Component,
    ]
    .into_iter()
    .find(|k| k.keyword().eq_ignore_ascii_case(word))
}

/// True for a character of a `!const`/`!var` name (P2-150).
fn is_var_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// True for a word usable as an element id.
// DECISION: ids are ASCII letters, digits, `_` and `-`, not starting with
// `-` (so they never look like an arrow), and never `this`: the
// Structurizr identifier alphabet.
fn is_identifier(word: &str) -> bool {
    let mut chars = word.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !word.eq_ignore_ascii_case("this")
}

/// Resolves a dotted path `a.b.c`: `by_key` finds the element keyed `c`,
/// then each earlier segment must key the parent of the one after it.
// DECISION: each segment is the direct parent of the next one, the
// Structurizr hierarchical-identifier rule (`system.app.api`).
fn find_path(
    elements: &[Element],
    path: &str,
    by_key: impl Fn(&str) -> Option<ElementId>,
) -> Option<ElementId> {
    let mut segments = path.rsplit('.');
    let found = by_key(segments.next()?)?;
    let mut current = found;
    for segment in segments {
        let parent = elements[current.0 as usize].parent?;
        let key = elements[parent.0 as usize].key.as_ref()?;
        if !key.value.eq_ignore_ascii_case(segment) {
            return None;
        }
        current = parent;
    }
    Some(found)
}

/// `usize` index to the `u32` of the id newtypes. Element counts are far
/// below `u32::MAX` (a source is at most 4 GiB and an element takes bytes).
fn index_u32(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// The significant tokens of one statement head.
struct Head<'t> {
    tree: &'t SyntaxTree,
    /// Indices into the tree's tokens, trivia left out.
    tokens: Vec<usize>,
    /// Per token, the text after `${NAME}` substitution (P2-150), `None`
    /// when it is the text written.
    texts: Vec<Option<String>>,
}

impl Head<'_> {
    fn len(&self) -> usize {
        self.tokens.len()
    }

    fn kind(&self, i: usize) -> Option<TokenKind> {
        self.tokens.get(i).map(|&t| self.tree.tokens()[t].kind)
    }

    fn text(&self, i: usize) -> &str {
        match self.texts.get(i) {
            Some(Some(text)) => text,
            _ => self.tree.token_text(self.tokens[i]),
        }
    }

    /// True when token `i` can be a string value: a quoted string, a
    /// `"""` text block (P2-148) or a bare word (P2-149).
    fn is_string(&self, i: usize) -> bool {
        matches!(
            self.kind(i),
            Some(TokenKind::String | TokenKind::TextBlock | TokenKind::Word)
        )
    }

    fn span(&self, i: usize) -> Span {
        self.tree.span(self.tokens[i])
    }

    /// True when token `i` is the word or directive `keyword`, ignoring
    /// case.
    fn is_keyword(&self, i: usize, keyword: &str) -> bool {
        matches!(self.kind(i), Some(TokenKind::Word | TokenKind::Directive))
            && self.text(i).eq_ignore_ascii_case(keyword)
    }

    /// Position of the first `->` or archetype arrow `--name->` (P2-157),
    /// if any.
    fn arrow(&self) -> Option<usize> {
        (0..self.len()).find(|&i| match self.kind(i) {
            Some(TokenKind::Arrow) => true,
            Some(TokenKind::Word) => archetype::arrow_archetype(self.text(i)).is_some(),
            _ => false,
        })
    }

    /// Span from token `from` to token `to`, both included.
    fn span_of(&self, from: usize, to: usize) -> Span {
        let (a, b) = (self.span(from), self.span(to));
        Span::new(a.file, a.start, b.end)
    }

    /// Token `i` as a string literal: a quoted string unquoted, a text
    /// block's value (P2-148), a bare word as written (P2-149). The span
    /// is always the bytes written (DSL-10).
    fn literal(&self, i: usize) -> Literal {
        let value = match self.kind(i) {
            Some(TokenKind::String) => text::unquote(self.text(i)),
            Some(TokenKind::TextBlock) => text_block_value(self.text(i)),
            _ => self.text(i).to_owned(),
        };
        Spanned {
            value,
            span: self.span(i),
        }
    }
}

/// The value of a `"""` text block (P2-148, /dsl/text-blocks is 404).
// DECISION: Structurizr for Java (DSL 6.2.4, checked): the lines between
// the delimiters, a blank first and last line dropped, the whitespace
// prefix common to the non-blank lines removed, no final line break; `\"`
// gives `"` and no other escape is processed. A block written on one line
// (Java refuses it) gives the text between the delimiters.
fn text_block_value(raw: &str) -> String {
    let inner = raw.strip_prefix("\"\"\"").unwrap_or(raw);
    let inner = inner.strip_suffix("\"\"\"").unwrap_or(inner);
    let inner = inner.replace("\\\"", "\"");
    let mut lines: Vec<&str> = inner
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    if lines.len() > 1 {
        if lines.first().is_some_and(|l| l.trim().is_empty()) {
            lines.remove(0);
        }
        if lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
    }
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| l.get(indent..).unwrap_or_else(|| l.trim_start()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A relationship end written as an id, resolved once the whole `model`
/// block is read (forward references, Review Focus).
struct Pending {
    relationship: usize,
    source: bool,
    key: String,
    span: Span,
    /// The element whose block holds the relationship (P2-142).
    context: Option<ElementId>,
    /// The deployment environment the relationship is written in (P3-109).
    environment: Option<EnvironmentId>,
}

/// State of one `Model::from_tree` call.
struct Builder<'t> {
    /// Every tree of the parse, indexed by `FileId`.
    trees: &'t [SyntaxTree],
    /// The tree the statement being read is in.
    tree: &'t SyntaxTree,
    /// File access and the environment (P2-152).
    loader: &'t mut dyn Loader,
    /// The path of each tree, by `FileId`; empty without a path.
    paths: Vec<PathBuf>,
    /// The files each `!include` inlines, by (file, statement start)
    /// (P2-5).
    included: BTreeMap<(FileId, u32), Vec<FileId>>,
    /// The `workspace extends` base of each file that has one, read as
    /// its own tree (P2-16).
    bases: BTreeMap<FileId, FileId>,
    /// `!const` and `!var` values in force (P2-150, P2-151).
    vars: BTreeMap<String, Var>,
    /// Every environment variable looked up, for `reparse`.
    env_used: RefCell<BTreeMap<String, Option<String>>>,
    model: Model,
    /// Element id → element, for resolving and duplicate checks:
    /// lowercased, the full id under hierarchical ids.
    keys: BTreeMap<String, ElementId>,
    /// Relationship id → relationship, lowercased (P2-146).
    relationship_keys: BTreeMap<String, RelationshipId>,
    /// Deployment and group ids, lowercased, with their declaration
    /// (P2-120): they share the id register.
    other_ids: BTreeMap<String, Span>,
    /// Relationship ends still to resolve.
    pending: Vec<Pending>,
    /// Archetypes by lowercased name (P2-155).
    archetypes: BTreeMap<String, archetype::Archetype>,
    /// The archetype chain's tags of each element declared from one.
    archetype_tags: BTreeMap<ElementId, Vec<String>>,
    /// The archetype tags of each relationship declared from one, by
    /// index.
    relationship_archetype_tags: BTreeMap<usize, Vec<String>>,
    /// The `group` blocks around the statement being read, outermost
    /// first (P2-4).
    group_stack: Vec<GroupId>,
    /// Deployment environments: name and lowercased id (P2-179).
    environments: Vec<(String, Option<String>)>,
    /// The deployment walk (P3-1, P3-106..P3-108).
    deploy: deploy::DeployState,
    /// The environment of the deployment view being read: its lines also
    /// name an instance without an id by `<element key>_<n>` (P3-117).
    view_environment: Option<EnvironmentId>,
    /// Every element id declared anywhere in the `model` block, found
    /// before the walk so a failed walk can tell a forward reference from
    /// an undeclared id.
    declared: BTreeSet<String>,
    seen_workspace: bool,
    seen_model: bool,
}

/// A `!const` or `!var` definition.
struct Var {
    value: String,
    constant: bool,
    /// The definition line.
    span: Span,
}

impl<'t> Builder<'t> {
    fn new(trees: &'t [SyntaxTree], loader: &'t mut dyn Loader) -> Builder<'t> {
        Builder {
            trees,
            tree: &trees[0],
            loader,
            paths: Vec::new(),
            included: BTreeMap::new(),
            bases: BTreeMap::new(),
            vars: BTreeMap::new(),
            env_used: RefCell::new(BTreeMap::new()),
            model: Model {
                name: None,
                description: None,
                elements: Vec::new(),
                relationships: Vec::new(),
                element_styles: Vec::new(),
                styles_span: None,
                views_span: None,
                views: Vec::new(),
                groups: Vec::new(),
                notices: Vec::new(),
                identifiers: Identifiers::Flat,
                implied_relationships: ImpliedRelationships::Default,
                terminology: Terminology::default(),
                workspace_properties: Vec::new(),
                model_properties: Vec::new(),
                views_properties: Vec::new(),
                configuration_properties: Vec::new(),
                configuration_scope: None,
                docs: Vec::new(),
                relationship_styles: Vec::new(),
                themes: Vec::new(),
                assets: Vec::new(),
                environments: Vec::new(),
            },
            keys: BTreeMap::new(),
            relationship_keys: BTreeMap::new(),
            other_ids: BTreeMap::new(),
            pending: Vec::new(),
            archetypes: BTreeMap::new(),
            archetype_tags: BTreeMap::new(),
            relationship_archetype_tags: BTreeMap::new(),
            group_stack: Vec::new(),
            environments: Vec::new(),
            deploy: deploy::DeployState::default(),
            view_environment: None,
            declared: BTreeSet::new(),
            seen_workspace: false,
            seen_model: false,
        }
    }

    fn error(&self, kind: ModelErrorKind, span: Span) -> ModelError {
        ModelError {
            kind,
            span,
            position: self.position(span),
        }
    }

    /// Line and column of `span.start` in the file it names.
    fn position(&self, span: Span) -> Position {
        let tree = self.trees.get(span.file.0 as usize).unwrap_or(self.tree);
        text::position(tree.source(), span.start)
    }

    /// The bytes `span` covers, in the file it names.
    fn written(&self, span: Span) -> &'t str {
        let tree = self.trees.get(span.file.0 as usize).unwrap_or(self.tree);
        tree.source().get(span.range()).unwrap_or_default()
    }

    /// `text` with every `${NAME}` replaced (P2-150..P2-152): a constant
    /// or variable in force wins, then the environment; an unknown name
    /// is left as written. `None` when nothing changed.
    // DECISION: a name is `a-zA-Z0-9-_.` (P2-150); `${` not followed by
    // such a name and `}` is plain text.
    fn substitute(&self, text: &str) -> Option<String> {
        if !text.contains("${") {
            return None;
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        let mut changed = false;
        while let Some(at) = rest.find("${") {
            out.push_str(&rest[..at]);
            let after = &rest[at + 2..];
            let name_len = after
                .find(|c: char| !is_var_name_char(c))
                .unwrap_or(after.len());
            let value = (name_len > 0 && after[name_len..].starts_with('}'))
                .then(|| self.lookup_var(&after[..name_len]))
                .flatten();
            if let Some(value) = value {
                out.push_str(&value);
                rest = &after[name_len + 1..];
                changed = true;
            } else {
                out.push_str("${");
                rest = after;
            }
        }
        out.push_str(rest);
        changed.then_some(out)
    }

    /// The value of `${name}`: a constant or variable, else the
    /// environment (recorded for `reparse`).
    // DECISION: B dsl-breadth 4, checked against Structurizr for Java: a
    // constant or variable defined earlier wins over the environment.
    fn lookup_var(&self, name: &str) -> Option<String> {
        if let Some(var) = self.vars.get(name) {
            return Some(var.value.clone());
        }
        let value = self.loader.env(name);
        self.env_used
            .borrow_mut()
            .insert(name.to_owned(), value.clone());
        value
    }

    /// `!const NAME value` or `!var NAME value` (P2-150, P2-151).
    fn define(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        let constant = head.is_keyword(0, "!const");
        self.no_block(statement, head)?;
        if head.len() < 3 {
            return Err(self.missing(head, if head.len() < 2 { "name" } else { "value" }));
        }
        if head.len() > 3 {
            return Err(self.invalid(head, head.text(0), 3));
        }
        // TODO(srs): P2-150 shows a bare name only. Reading taken
        // (Structurizr 6.2.4 opens corpus
        // constants-and-variables-from-workspace-extension-parent.dsl): a
        // quoted name is the same name, checked unquoted.
        let written = self.written(head.span(1));
        let name = match head.kind(1) {
            Some(TokenKind::Word) => written.to_owned(),
            Some(TokenKind::String) => text::unquote(written),
            _ => return Err(self.invalid(head, head.text(0), 1)),
        };
        if name.is_empty() || !name.chars().all(is_var_name_char) {
            return Err(self.invalid(head, head.text(0), 1));
        }
        let name = name.as_str();
        if !head.is_string(2) {
            return Err(self.invalid(head, head.text(0), 2));
        }
        // DECISION: Structurizr for Java (checked): redefining a name a
        // `!const` holds is an error with either directive, and so is a
        // `!const` over a `!var`; only `!var` over `!var` redefines.
        let line = head.span_of(0, head.len() - 1);
        if let Some(first) = self.vars.get(name)
            && (first.constant || constant)
        {
            return Err(self.error(
                ModelErrorKind::DuplicateConstant {
                    name: name.to_owned(),
                    first: first.span,
                    first_position: self.position(first.span),
                },
                line,
            ));
        }
        let value = head.literal(2).value;
        self.vars.insert(
            name.to_owned(),
            Var {
                value,
                constant,
                span: line,
            },
        );
        Ok(())
    }

    /// The folder of the file `file` names, when it was read from a path.
    fn folder(&self, file: FileId) -> Option<PathBuf> {
        let path = self.paths.get(file.0 as usize)?;
        Some(path.parent().map_or_else(PathBuf::new, Path::to_path_buf))
    }

    /// `written`, a path relative to the file it is written in, checked:
    /// it exists and lies inside the diagram's (the root file's) folder
    /// after `..` and symlinks are resolved (P2-159). `None` when the file
    /// was parsed without a path.
    // DECISION: the containment folder is the root file's, as the P2-159
    // reading says ("outside the diagram's folder") and Structurizr 6.2.4
    // does (corpus include-directory.dsl: `!docs ../../docs` in an
    // included file two folders down); `!include` keeps P2-134's
    // including-file rule.
    fn checked_path(&mut self, written: &Literal) -> Result<Option<PathBuf>, ModelError> {
        let Some(folder) = self.folder(written.span.file) else {
            return Ok(None);
        };
        let diagram = self.folder(FileId::ROOT).unwrap_or_else(|| folder.clone());
        let joined = folder.join(&written.value);
        let resolved = self.loader.canonicalize(&joined).map_err(|e| {
            self.error(
                ModelErrorKind::FileRead {
                    path: written.value.clone(),
                    error: e.to_string(),
                },
                written.span,
            )
        })?;
        let root = self.loader.canonicalize(&diagram).unwrap_or(diagram);
        if !resolved.starts_with(&root) {
            return Err(self.error(
                ModelErrorKind::PathOutsideRoot {
                    path: written.value.clone(),
                },
                written.span,
            ));
        }
        Ok(Some(resolved))
    }

    /// `!docs <path> [fqcn]`, `!adrs <path> [type|fqcn]` or `!decisions`
    /// (P2-159, P2-160): recorded, the path checked when the file has
    /// one, the files not read.
    // TODO(srs): P2-159, whether phase 2 reads or checks the files.
    // DECISION: B dsl-breadth 19, checked against Structurizr for Java
    // (a missing path fails the parse): a missing path is `FileRead`, one
    // outside the diagram's folder `PathOutsideRoot`; a text parsed
    // without a path records it unchecked. A `!docs` importer, or an
    // `!adrs` importer holding a `.`, is a FQCN and gives a notice (P2-3).
    fn attachment(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
    ) -> Result<Attachment, ModelError> {
        self.attachment_block(statement)?;
        let mut strings = self.strings(head, head.text(0), 1, 2)?.into_iter();
        let Some(path) = strings.next() else {
            return Err(self.missing(head, "path"));
        };
        let importer = strings.next();
        let docs = head.is_keyword(0, "!docs");
        self.checked_path(&path)?;
        if let Some(i) = &importer
            && (docs || i.value.contains('.'))
        {
            let subject = head.text(0).to_ascii_lowercase();
            let subject = match subject.as_str() {
                "!docs" => "!docs",
                "!adrs" => "!adrs",
                _ => "!decisions",
            };
            self.notice(subject, None, head.span_of(0, head.len() - 1));
        }
        Ok(Attachment {
            kind: if docs {
                AttachmentKind::Docs
            } else {
                AttachmentKind::Adrs
            },
            path,
            importer,
            span: head.span_of(0, head.len() - 1),
        })
    }

    /// The optional `{ exclude <pattern>... }` block of `!docs`, `!adrs`
    /// or `!decisions`: checked, kept nowhere.
    // TODO(srs): P2-159, P2-160 name no block. Reading taken (Structurizr
    // 6.2.4 opens corpus docs/workspace.dsl and decisions/workspace.dsl):
    // only `exclude` lines of one or more strings; phase 2 reads no
    // documentation file, so the patterns change nothing.
    fn attachment_block(&mut self, statement: &Statement) -> Result<(), ModelError> {
        let Some(block) = &statement.block else {
            return Ok(());
        };
        for inner in &block.statements {
            let line = self.head(inner)?;
            if self.common(inner, &line)? {
                continue;
            }
            if !line.is_keyword(0, "exclude") {
                return Err(self.reject(&line, 0));
            }
            self.no_block(inner, &line)?;
            if line.len() < 2 {
                return Err(self.missing(&line, "pattern"));
            }
            self.strings(&line, "exclude", 1, usize::MAX)?;
        }
        Ok(())
    }

    /// Records a construct accepted with no effect (P2-3), in file order.
    fn notice(&mut self, subject: &str, view: Option<&str>, span: Span) {
        self.model.notices.push(Notice {
            kind: NoticeKind::NoEffect,
            subject: subject.to_owned(),
            view: view.map(str::to_owned),
            span: Some(span),
            position: Some(self.position(span)),
        });
    }

    /// Statements every block accepts: `!const` and `!var`. True when
    /// `head` was one of them.
    fn common(&mut self, statement: &Statement, head: &Head<'_>) -> Result<bool, ModelError> {
        if head.is_keyword(0, "!const") || head.is_keyword(0, "!var") {
            self.define(statement, head)?;
            return Ok(true);
        }
        // P2-3: `!script <language> { <raw body> }` or `!script <file>
        // [{ parameters }]`, and `!plugin <fqcn> [{ parameters }]`: one
        // notice each; the raw body is one token, skipped whole.
        if head.is_keyword(0, "!script") || head.is_keyword(0, "!plugin") {
            if head.len() < 2 {
                return Err(self.missing(head, "name"));
            }
            if head.len() > 2 {
                return Err(self.invalid(head, head.text(0), 2));
            }
            let subject = if head.is_keyword(0, "!script") { "!script" } else { "!plugin" };
            self.notice(subject, None, head.span_of(0, head.len() - 1));
            return Ok(true);
        }
        Ok(false)
    }

    /// The head of `statement`. A line continuation and the newline after
    /// it are trivia, so a continued statement reads as one line
    /// (P2-147); a block with no head is in no construct but a dynamic
    /// view's parallel block, which reads it itself.
    fn head(&self, statement: &Statement) -> Result<Head<'t>, ModelError> {
        let tokens: Vec<usize> = statement
            .head
            .clone()
            .filter(|&i| !self.tree.tokens()[i].kind.is_trivia())
            .collect();
        if tokens.is_empty() {
            let span = Span::new(
                self.tree.file(),
                statement.span.start,
                statement.span.start + 1,
            );
            return Err(self.error(
                ModelErrorKind::UnknownConstruct {
                    construct: "{".into(),
                },
                span,
            ));
        }
        let texts = tokens
            .iter()
            .map(|&i| self.substitute(self.tree.token_text(i)))
            .collect();
        Ok(Head {
            tree: self.tree,
            tokens,
            texts,
        })
    }

    /// The error for token `i` of `head` when it starts no construct
    /// allowed here: `UnknownConstruct` (DSL-4). Every construct of the
    /// language reference is handled since dsl-breadth-20 (P2-1), so no
    /// word is `LaterPhase` any more; `LaterPhase` stays for a JSON
    /// `workspace extends` base (phase 3).
    fn reject(&self, head: &Head<'_>, i: usize) -> ModelError {
        self.error(
            ModelErrorKind::UnknownConstruct {
                construct: head.text(i).into(),
            },
            head.span(i),
        )
    }

    fn invalid(&self, head: &Head<'_>, construct: &str, i: usize) -> ModelError {
        self.error(
            ModelErrorKind::InvalidValue {
                construct: construct.into(),
                value: head.text(i).into(),
            },
            head.span(i),
        )
    }

    fn missing(&self, head: &Head<'_>, argument: &'static str) -> ModelError {
        self.error(
            ModelErrorKind::MissingArgument {
                construct: head.text(0).into(),
                argument,
            },
            head.span_of(0, head.len() - 1),
        )
    }

    /// Error unless `head` has nothing after its keyword.
    fn no_arguments(&self, head: &Head<'_>) -> Result<(), ModelError> {
        if head.len() > 1 {
            return Err(self.invalid(head, head.text(0), 1));
        }
        Ok(())
    }

    /// Error when a statement that takes no block has one.
    fn no_block(&self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        match &statement.block {
            Some(block) => Err(self.error(
                ModelErrorKind::InvalidValue {
                    construct: head.text(0).into(),
                    value: "{".into(),
                },
                self.tree.span(block.open),
            )),
            None => Ok(()),
        }
    }

    /// The block of a statement that needs one.
    fn block<'s>(
        &self,
        statement: &'s Statement,
        head: &Head<'_>,
    ) -> Result<&'s [Statement], ModelError> {
        match &statement.block {
            Some(block) => Ok(&block.statements),
            None => Err(self.missing(head, "`{ }` block")),
        }
    }

    /// Exactly one string after the keyword.
    fn one_string(&self, head: &Head<'_>) -> Result<Literal, ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "string"));
        }
        if !head.is_string(1) {
            return Err(self.invalid(head, head.text(0), 1));
        }
        if head.len() > 2 {
            return Err(self.invalid(head, head.text(0), 2));
        }
        Ok(head.literal(1))
    }

    /// Strings from token `from` on, at most `max`.
    fn strings(
        &self,
        head: &Head<'_>,
        construct: &str,
        from: usize,
        max: usize,
    ) -> Result<Vec<Literal>, ModelError> {
        let mut out = Vec::new();
        for i in from..head.len() {
            if !head.is_string(i) || out.len() == max {
                return Err(self.invalid(head, construct, i));
            }
            out.push(head.literal(i));
        }
        Ok(out)
    }

    // --------------------------------------------------------- file level

    /// The files the `!include` `statement` (in the current tree)
    /// inlines, if it is one.
    fn included_by(&self, statement: &Statement) -> Option<Vec<FileId>> {
        self.included
            .get(&(self.tree.file(), statement.span.start))
            .cloned()
    }

    /// Calls `f` on each of `statements`, the statements of an included
    /// file standing in for its `!include` (P2-5), with `self.tree` set to
    /// the tree each statement is in.
    fn each(
        &mut self,
        statements: &[Statement],
        f: &mut dyn FnMut(&mut Self, &Statement) -> Result<(), ModelError>,
    ) -> Result<(), ModelError> {
        for statement in statements {
            if let Some(files) = self.included_by(statement) {
                let trees = self.trees;
                let saved = self.tree;
                for file in files {
                    let tree = &trees[file.0 as usize];
                    self.tree = tree;
                    let result = self.each(tree.statements(), f);
                    self.tree = saved;
                    result?;
                }
                continue;
            }
            f(self, statement)?;
        }
        Ok(())
    }

    fn file(&mut self) -> Result<(), ModelError> {
        let tree = self.tree;
        self.file_statements(tree.statements())?;
        self.check_scope()
    }

    /// The top-level statements of a file: one `workspace`.
    fn file_statements(&mut self, statements: &[Statement]) -> Result<(), ModelError> {
        self.each(statements, &mut |this, statement| {
            let head = this.head(statement)?;
            if this.common(statement, &head)? {
                return Ok(());
            }
            if head.is_keyword(0, "workspace") {
                this.workspace(statement, &head)
            } else {
                Err(this.reject(&head, 0))
            }
        })
    }

    fn workspace(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        if std::mem::replace(&mut self.seen_workspace, true) {
            return Err(self.duplicate_line("workspace", head));
        }
        if head.is_keyword(1, "extends") {
            self.extend(head)?;
        } else {
            let mut strings = self.strings(head, "workspace", 1, 2)?.into_iter();
            self.model.name = strings.next();
            self.model.description = strings.next();
        }
        self.each(self.block(statement, head)?, &mut |this, inner| {
            let head = this.head(inner)?;
            if this.common(inner, &head)? {
                return Ok(());
            }
            if head.is_keyword(0, "model") {
                this.model_block(inner, &head)?;
            } else if head.is_keyword(0, "views") {
                this.views_block(inner, &head)?;
            } else if head.is_keyword(0, "name") {
                // DECISION: a `name`/`description` line wins over the
                // positional value, as the later one in the file (P2-174).
                this.model.name = Some(this.one_string(&head)?);
                this.no_block(inner, &head)?;
            } else if head.is_keyword(0, "description") {
                this.model.description = Some(this.one_string(&head)?);
                this.no_block(inner, &head)?;
            } else if head.is_keyword(0, "properties") {
                let properties = this.properties(inner, &head)?;
                this.model.workspace_properties.extend(properties);
            } else if head.is_keyword(0, "configuration") {
                this.configuration(inner, &head)?;
            } else if is_attachment(&head) {
                let attachment = this.attachment(inner, &head)?;
                this.model.docs.push(attachment);
            } else if head.is_keyword(0, "!identifiers") {
                this.identifiers(inner, &head)?;
            } else if head.is_keyword(0, "!impliedRelationships") {
                this.implied_relationships(inner, &head)?;
            } else {
                return Err(this.reject(&head, 0));
            }
            Ok(())
        })?;
        Ok(())
    }

    /// `configuration { scope ..., visibility ..., users { }, properties
    /// { } }` (P2-175, P2-3).
    fn configuration(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        self.no_arguments(head)?;
        for inner in self.block(statement, head)? {
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            if head.is_keyword(0, "scope") {
                self.no_block(inner, &head)?;
                let word = self.one_word(&head)?.to_ascii_lowercase();
                let value = match word.as_str() {
                    "landscape" => ConfigurationScope::Landscape,
                    "softwaresystem" => ConfigurationScope::SoftwareSystem,
                    "none" => ConfigurationScope::None,
                    _ => return Err(self.invalid(&head, "scope", 1)),
                };
                self.model.configuration_scope = Some(Spanned {
                    value,
                    span: head.span(1),
                });
            } else if head.is_keyword(0, "visibility") {
                self.no_block(inner, &head)?;
                let word = self.one_word(&head)?;
                if !["private", "public"].iter().any(|w| word.eq_ignore_ascii_case(w)) {
                    return Err(self.invalid(&head, "visibility", 1));
                }
                self.notice("configuration visibility", None, head.span_of(0, 1));
            } else if head.is_keyword(0, "users") {
                self.no_arguments(&head)?;
                self.block(inner, &head)?;
                self.notice("configuration users", None, head.span(0));
            } else if head.is_keyword(0, "properties") {
                let properties = self.properties(inner, &head)?;
                self.model.configuration_properties.extend(properties);
            } else {
                return Err(self.reject(&head, 0));
            }
        }
        Ok(())
    }

    /// `properties { <name> <value> ... }` (P2-172): one pair per line.
    fn properties(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
    ) -> Result<Vec<Property>, ModelError> {
        self.no_arguments(head)?;
        let mut out = Vec::new();
        for inner in self.block(statement, head)? {
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            self.no_block(inner, &head)?;
            if head.len() < 2 {
                return Err(self.missing(&head, "value"));
            }
            for i in 0..2 {
                if !head.is_string(i) {
                    return Err(self.invalid(&head, "properties", i));
                }
            }
            if head.len() > 2 {
                return Err(self.invalid(&head, "properties", 2));
            }
            out.push(Property {
                name: head.literal(0),
                value: head.literal(1),
                span: head.span_of(0, head.len() - 1),
            });
        }
        Ok(out)
    }

    /// `perspectives { <name> <description> [value] | perspective <name>
    /// { description, value, url } ... }` (P2-173).
    fn perspectives(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
    ) -> Result<Vec<Perspective>, ModelError> {
        self.no_arguments(head)?;
        let mut out = Vec::new();
        for inner in self.block(statement, head)? {
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            if head.is_keyword(0, "perspective") && inner.block.is_some() {
                let name = self.one_string(&head)?;
                let mut p = Perspective {
                    name,
                    description: None,
                    value: None,
                    url: None,
                    span: inner.span,
                };
                for line in self.block(inner, &head)? {
                    let h = self.head(line)?;
                    self.no_block(line, &h)?;
                    let slot = if h.is_keyword(0, "description") {
                        &mut p.description
                    } else if h.is_keyword(0, "value") {
                        &mut p.value
                    } else if h.is_keyword(0, "url") {
                        &mut p.url
                    } else {
                        return Err(self.reject(&h, 0));
                    };
                    *slot = Some(self.one_string(&h)?);
                }
                out.push(p);
                continue;
            }
            self.no_block(inner, &head)?;
            let mut strings = self.strings(&head, "perspectives", 0, 3)?.into_iter();
            let Some(name) = strings.next() else {
                return Err(self.missing(&head, "name"));
            };
            let Some(description) = strings.next() else {
                return Err(self.missing(&head, "description"));
            };
            out.push(Perspective {
                name,
                description: Some(description),
                value: strings.next(),
                url: None,
                span: head.span_of(0, head.len() - 1),
            });
        }
        Ok(out)
    }

    /// The configuration scope holds (P2-175): Structurizr's workspace
    /// scope validators, which the server applies to every workspace.
    // DECISION: Structurizr for Java (structurizr-core 6.2.4 validators,
    // checked): `landscape` fails on the first software system with
    // containers or documentation; `softwaresystem` fails when more than
    // one has them (none is fine); `none` checks nothing. The error names
    // the system by its id, else its name, at the `scope` value.
    fn check_scope(&self) -> Result<(), ModelError> {
        let Some(scope) = self.model.configuration_scope else {
            return Ok(());
        };
        let detailed: Vec<&Element> = self
            .model
            .elements
            .iter()
            .filter(|e| {
                e.kind == ElementKind::SoftwareSystem
                    && (!e.docs.is_empty() || !self.model.children(e.id).is_empty())
            })
            .collect();
        let offender = match scope.value {
            ConfigurationScope::Landscape => detailed.first(),
            ConfigurationScope::SoftwareSystem => detailed.get(1),
            ConfigurationScope::None => None,
        };
        match offender {
            Some(e) => Err(self.error(
                ModelErrorKind::ScopeViolation {
                    scope: scope.value,
                    element: e
                        .key
                        .as_ref()
                        .map_or_else(|| e.name.value.clone(), |k| k.value.clone()),
                },
                scope.span,
            )),
            None => Ok(()),
        }
    }

    /// `workspace extends <base> { ... }` (P2-16): the base workspace is
    /// built first, then this file's block applies on top of it, its
    /// `model`, `views` and `styles` blocks adding to the base's.
    fn extend(&mut self, head: &Head<'_>) -> Result<(), ModelError> {
        if head.len() > 3 {
            return Err(self.invalid(head, "workspace extends", 3));
        }
        let Some(&base) = self.bases.get(&self.tree.file()) else {
            return Err(self.error(
                ModelErrorKind::NeedsFile {
                    construct: "workspace extends".into(),
                },
                head.span_of(0, head.len() - 1),
            ));
        };
        let trees = self.trees;
        let saved = self.tree;
        let tree = &trees[base.0 as usize];
        self.tree = tree;
        self.seen_workspace = false;
        let result = self.file_statements(tree.statements());
        self.tree = saved;
        result?;
        // The extending file's blocks add to the base's.
        self.seen_model = false;
        Ok(())
    }

    /// P2-400: a property line `head` that sets again what an earlier line
    /// at `earlier` set; the later line wins, as in Structurizr, and the
    /// earlier one gets a no-effect notice naming it.
    fn overridden(&mut self, earlier: Option<Span>, head: &Head<'_>) {
        let Some(span) = earlier else {
            return;
        };
        let later = self.position(head.span(0)).line;
        let subject = format!("an earlier `{}` line (overridden by line {later})", head.text(0));
        self.notice(&subject, None, span);
    }

    fn duplicate_line(&self, construct: &'static str, head: &Head<'_>) -> ModelError {
        self.error(
            ModelErrorKind::DuplicateLine {
                construct,
                id: String::new(),
            },
            head.span(0),
        )
    }

    // -------------------------------------------------------------- model

    fn model_block(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        if std::mem::replace(&mut self.seen_model, true) {
            return Err(self.duplicate_line("model", head));
        }
        self.no_arguments(head)?;
        let statements = self.block(statement, head)?;
        self.prescan(self.tree, statements);
        let walked = self.each(statements, &mut |this, inner| this.model_statement(inner, None));
        if let Err(e) = walked {
            return Err(self.undeclared_before(e));
        }
        self.resolve_pending()?;
        self.finish_deployment()
    }

    /// Collects every `<id> = ...` id in `statements`, nested blocks
    /// included, lowercased.
    fn prescan(&mut self, tree: &'t SyntaxTree, statements: &[Statement]) {
        for statement in statements {
            if let Some(files) = self.included.get(&(tree.file(), statement.span.start)).cloned() {
                for file in files {
                    let included = &self.trees[file.0 as usize];
                    self.prescan(included, included.statements());
                }
                continue;
            }
            let ids: Vec<usize> = statement
                .head
                .clone()
                .filter(|&i| !tree.tokens()[i].kind.is_trivia())
                .take(2)
                .collect();
            if let [id, eq] = ids[..]
                && tree.tokens()[eq].kind == TokenKind::Equals
            {
                self.declared
                    .insert(tree.token_text(id).to_ascii_lowercase());
            }
            if let Some(block) = &statement.block {
                self.prescan(tree, &block.statements);
            }
        }
    }

    /// The walk stopped at `e`: an undeclared id earlier in the file is
    /// the first problem, so it wins.
    // DECISION: a relationship end before `e` counts as declared when its
    // last segment is an id written anywhere in `model` (prescan); the
    // full dotted-path check needs the elements after `e`.
    fn undeclared_before(&self, e: ModelError) -> ModelError {
        for p in &self.pending {
            if p.span.start >= e.span.start && p.span.file == e.span.file {
                break;
            }
            let last = p.key.rsplit('.').next().unwrap_or_default();
            if self.resolve(&p.key, p.context).is_none()
                && !self.declared.contains(&last.to_ascii_lowercase())
            {
                return self.error(ModelErrorKind::UndeclaredId { id: p.key.clone() }, p.span);
            }
        }
        e
    }

    /// The element `path` names, read inside the block of `context`
    /// (`None` outside any element block). Ids match ignoring ASCII case
    /// (Structurizr; owner ruling, phase2-decisions Part C).
    fn resolve(&self, path: &str, context: Option<ElementId>) -> Option<ElementId> {
        self.resolve_in(path, context, self.deploy.environment())
    }

    /// `resolve`, read inside deployment environment `environment` (or
    /// none): under hierarchical ids a path not found otherwise is also
    /// looked up under the environment's id (P3-109).
    fn resolve_in(
        &self,
        path: &str,
        context: Option<ElementId>,
        environment: Option<EnvironmentId>,
    ) -> Option<ElementId> {
        if self.model.identifiers == Identifiers::Hierarchical {
            // DECISION: B dsl-breadth 11, checked against Structurizr for
            // Java: inside an element's block an id is looked up under
            // that element's full id first, then (a superset) under each
            // ancestor's, then as a full id.
            let mut scope = context;
            while let Some(c) = scope {
                if let Some(prefix) = self.model.full_id(c) {
                    let key = format!("{prefix}.{path}").to_ascii_lowercase();
                    if let Some(&id) = self.keys.get(&key) {
                        return Some(id);
                    }
                }
                scope = self.model.element(c).parent;
            }
            // DECISION: a path written at environment level (no element
            // block around it) is also tried under the environment's id,
            // as Structurizr's deployment context does
            // (no-relationship.dsl: `computer.webbrowser.ui -/> ...`).
            let environment = context.and_then(|c| self.model.element(c).environment).or(environment);
            return self.keys.get(&path.to_ascii_lowercase()).copied().or_else(|| {
                let key = self.model.environments.get(environment?.0 as usize)?.key.as_ref()?;
                self.keys.get(&format!("{}.{path}", key.value).to_ascii_lowercase()).copied()
            });
        }
        find_path(&self.model.elements, path, |k| {
            self.keys.get(&k.to_ascii_lowercase()).copied()
        })
    }

    fn resolve_pending(&mut self) -> Result<(), ModelError> {
        for p in std::mem::take(&mut self.pending) {
            let Some(id) = self.resolve_in(&p.key, p.context, p.environment) else {
                return Err(self.error(ModelErrorKind::UndeclaredId { id: p.key }, p.span));
            };
            let r = &mut self.model.relationships[p.relationship];
            if p.source {
                r.source.value = id;
            } else {
                r.destination.value = id;
            }
        }
        Ok(())
    }

    /// `DuplicateId` when `key` (as written, full under hierarchical ids)
    /// is already an element, relationship or deployment id: one register
    /// for all of them, ignoring case (P2-144, P2-146, P2-120).
    // DECISION: B dsl-breadth 12, checked against Structurizr for Java
    // ("The identifier ... is already in use"): relationship and element
    // ids share the register.
    fn check_free(&self, key: &str, span: Span) -> Result<(), ModelError> {
        let lower = key.to_ascii_lowercase();
        let first = if let Some(&e) = self.keys.get(&lower) {
            let e = self.model.element(e);
            Some(e.key.as_ref().map_or(e.span, |k| k.span))
        } else if let Some(&r) = self.relationship_keys.get(&lower) {
            let r = &self.model.relationships[r.0 as usize];
            Some(r.key.as_ref().map_or(r.span, |k| k.span))
        } else {
            self.other_ids.get(&lower).copied()
        };
        match first {
            Some(first) => Err(self.error(
                ModelErrorKind::DuplicateId {
                    id: key.to_owned(),
                    first,
                    first_position: self.position(first),
                },
                span,
            )),
            None => Ok(()),
        }
    }

    /// One statement in `model` (`parent` is `None`) or in an element
    /// block.
    fn model_statement(
        &mut self,
        statement: &Statement,
        parent: Option<ElementId>,
    ) -> Result<(), ModelError> {
        let head = self.head(statement)?;
        if self.common(statement, &head)? {
            return Ok(());
        }
        let named = head.kind(1) == Some(TokenKind::Equals);
        if named && head.len() < 3 {
            return Err(self.error(
                ModelErrorKind::MissingArgument {
                    construct: "=".into(),
                    argument: "element",
                },
                head.span_of(0, 1),
            ));
        }
        let key = named.then_some(0);
        let kw = if named { 2 } else { 0 };
        if head.arrow().is_some() {
            return self.relationship(statement, &head, parent, key);
        }
        if head.is_keyword(kw, "!extend") || head.is_keyword(kw, "!ref") {
            // P2-169: `!relationship` when the id names a relationship.
            let names_relationship = head.len() > kw + 1
                && self
                    .relationship_keys
                    .contains_key(&head.text(kw + 1).to_ascii_lowercase());
            if names_relationship && kw == 0 {
                return self.relationship_directive(statement, &head);
            }
            return self.element_directive(statement, &head, parent);
        }
        if head.is_keyword(kw, "!element") {
            return self.element_directive(statement, &head, parent);
        }
        if head.is_keyword(0, "!elements") {
            return self.elements_directive(statement, &head, parent);
        }
        if head.is_keyword(0, "!relationships") {
            return self.relationships_directive(statement, &head, parent);
        }
        if head.is_keyword(kw, "!relationship") {
            return self.relationship_directive(statement, &head);
        }
        if parent.is_none() && head.is_keyword(kw, "deploymentEnvironment") {
            return self.deployment_environment(statement, &head);
        }
        let parent_kind = parent.map(|p| self.model.element(p).kind);
        let group_block = matches!(
            parent_kind,
            None | Some(ElementKind::SoftwareSystem | ElementKind::Container)
        );
        let archetype = self.element_archetype(&head, kw).map(|a| a.base);
        let group_keyword =
            head.is_keyword(kw, "group") || archetype == Some(archetype::Base::Group);
        if group_keyword && statement.block.is_some() && group_block {
            return self.group(statement, &head, parent);
        }
        if parent.is_none() && head.is_keyword(0, "archetypes") {
            return self.archetypes(statement, &head);
        }
        if matches!(archetype, Some(archetype::Base::Element(_))) || head.is_keyword(kw, "element") {
            return self.element(statement, &head, parent);
        }
        if head.kind(kw) == Some(TokenKind::Word) && element_kind(head.text(kw)).is_some() {
            return self.element(statement, &head, parent);
        }
        if named {
            return Err(self.reject(&head, kw));
        }
        match parent {
            Some(id) => self.element_line(statement, &head, id),
            None if head.is_keyword(0, "properties") => {
                let properties = self.properties(statement, &head)?;
                self.model.model_properties.extend(properties);
                Ok(())
            }
            None if head.is_keyword(0, "!identifiers") => self.identifiers(statement, &head),
            None if head.is_keyword(0, "!impliedrelationships") => {
                self.implied_relationships(statement, &head)
            }
            None => Err(self.reject(&head, 0)),
        }
    }

    /// `[<id> =] group <name> { ... }` in `model`, a software system or a
    /// container (P2-4, P2-127, P2-129): every element declared directly
    /// inside has the group.
    fn group(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        parent: Option<ElementId>,
    ) -> Result<(), ModelError> {
        let (key, kw) = declared_at(head);
        let mut strings = self.strings(head, "group", kw + 1, 1)?.into_iter();
        let Some(name) = strings.next() else {
            return Err(self.missing(head, "name"));
        };
        let enclosing = self.group_stack.last().copied();
        let full_name = match enclosing {
            None => name.value.clone(),
            Some(outer) => {
                // DECISION: B dsl-breadth 14b, checked against Structurizr
                // for Java ("To use nested groups, please define a model
                // property named structurizr.groupSeparator").
                let Some(separator) = self
                    .model
                    .model_properties
                    .iter()
                    .rev()
                    .find(|p| p.name.value == "structurizr.groupSeparator")
                else {
                    return Err(self.error(
                        ModelErrorKind::NestedGroupNeedsSeparator,
                        head.span_of(0, head.len() - 1),
                    ));
                };
                let outer = &self.model.groups[outer.0 as usize].full_name;
                format!("{outer}{}{}", separator.value.value, name.value)
            }
        };
        let key = match key {
            Some(k) => {
                if head.kind(k) != Some(TokenKind::Word) || !is_identifier(head.text(k)) {
                    return Err(self.invalid(head, "group", k));
                }
                self.check_free(head.text(k), head.span(k))?;
                self.other_ids
                    .insert(head.text(k).to_ascii_lowercase(), head.span(k));
                Some(Spanned {
                    value: head.text(k).to_owned(),
                    span: head.span(k),
                })
            }
            None => None,
        };
        let id = GroupId(index_u32(self.model.groups.len()));
        self.model.groups.push(Group {
            id,
            key,
            name,
            full_name,
            parent: enclosing,
            owner: parent,
            span: statement.span,
        });
        self.group_stack.push(id);
        let result = self.group_body(statement, parent);
        self.group_stack.pop();
        result
    }

    /// The statements of a group block: element declarations of the
    /// block's level, nested groups, relationships and directives; no
    /// element-block line (Structurizr's group context).
    fn group_body(
        &mut self,
        statement: &Statement,
        parent: Option<ElementId>,
    ) -> Result<(), ModelError> {
        let Some(block) = &statement.block else {
            return Ok(());
        };
        self.each(&block.statements, &mut |this, inner| {
            let head = this.head(inner)?;
            let (_, kw) = declared_at(&head);
            let member = head.kind(1) == Some(TokenKind::Equals)
                || head.arrow().is_some()
                || head.kind(0) == Some(TokenKind::Directive)
                || head.is_keyword(kw, "group")
                || head.is_keyword(kw, "element")
                || this.element_archetype(&head, kw).is_some()
                || element_kind(head.text(kw)).is_some();
            if !member {
                return Err(this.reject(&head, 0));
            }
            this.model_statement(inner, parent)?;
            Ok(())
        })?;
        Ok(())
    }

    /// `group "<name>"` in a component block (P2-128): the component's
    /// group, shared by every component of the container naming it.
    // DECISION: one `Group` per name per container, owned by the
    // container, not nested in any group block (Structurizr keeps groups
    // as names, so equal names are one group).
    fn group_line(&mut self, id: ElementId, name: Literal, span: Span) {
        let owner = self.model.element(id).parent;
        let existing = self
            .model
            .groups
            .iter()
            .find(|g| g.owner == owner && g.parent.is_none() && g.full_name == name.value)
            .map(|g| g.id);
        let group = existing.unwrap_or_else(|| {
            let gid = GroupId(index_u32(self.model.groups.len()));
            self.model.groups.push(Group {
                id: gid,
                key: None,
                full_name: name.value.clone(),
                name,
                parent: None,
                owner,
                span,
            });
            gid
        });
        self.model.elements[id.0 as usize].group = Some(group);
    }

    /// `!identifiers flat|hierarchical` (P2-6, P2-144).
    fn identifiers(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        self.no_block(statement, head)?;
        let word = self.one_word(head)?;
        self.model.identifiers = if word.eq_ignore_ascii_case("flat") {
            Identifiers::Flat
        } else if word.eq_ignore_ascii_case("hierarchical") {
            Identifiers::Hierarchical
        } else {
            return Err(self.invalid(head, "!identifiers", 1));
        };
        Ok(())
    }

    /// `!impliedRelationships true|false|<fqcn>` (P2-7, P2-153, P2-154).
    fn implied_relationships(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
    ) -> Result<(), ModelError> {
        self.no_block(statement, head)?;
        if head.len() < 2 {
            return Err(self.missing(head, "value"));
        }
        if head.len() > 2 {
            return Err(self.invalid(head, head.text(0), 2));
        }
        if !head.is_string(1) {
            return Err(self.invalid(head, head.text(0), 1));
        }
        let value = head.literal(1);
        self.model.implied_relationships = if value.value.eq_ignore_ascii_case("true") {
            ImpliedRelationships::True
        } else if value.value.eq_ignore_ascii_case("false") {
            ImpliedRelationships::False
        } else {
            self.notice("!impliedRelationships", None, head.span_of(0, 1));
            ImpliedRelationships::Strategy(value)
        };
        Ok(())
    }

    /// `!element <id> { ... }`: the block's lines and statements apply to
    /// that existing element (P2-17, P2-142, P2-165).
    fn element_directive(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        parent: Option<ElementId>,
    ) -> Result<(), ModelError> {
        let kw = if head.kind(1) == Some(TokenKind::Equals) { 2 } else { 0 };
        if head.len() < kw + 2 {
            return Err(self.missing(head, "element"));
        }
        if head.len() > kw + 2 {
            return Err(self.invalid(head, head.text(kw), kw + 2));
        }
        let written = head.literal(kw + 1).value;
        let found = if written.contains("://") {
            self.by_canonical_name(&written)
        } else {
            self.resolve(&written, parent)
        };
        let Some(id) = found else {
            if kw == 0 && parent.is_none() && self.deployment_element(statement, &written)? {
                return Ok(());
            }
            return Err(self.error(
                ModelErrorKind::UndeclaredId { id: written },
                head.span(kw + 1),
            ));
        };
        if kw == 2 {
            self.bind(head, id)?;
        }
        if matches!(
            self.model.element(id).kind,
            ElementKind::DeploymentNode
                | ElementKind::InfrastructureNode
                | ElementKind::SoftwareSystemInstance
                | ElementKind::ContainerInstance
        ) {
            self.reopen_deployment(statement, id)?;
        } else {
            self.element_block(statement, id)?;
        }
        self.retag_element(id);
        Ok(())
    }

    /// The element whose Structurizr canonical name is `name`
    /// (`Person://Name`, `Container://System.Container`, ...).
    fn by_canonical_name(&self, name: &str) -> Option<ElementId> {
        self.model
            .elements
            .iter()
            .find(|e| self.canonical_name(e.id) == name)
            .map(|e| e.id)
    }

    /// Structurizr's canonical name of `id`: the type, `://`, the names
    /// from the top-level ancestor down joined by `.`.
    fn canonical_name(&self, id: ElementId) -> String {
        let element = self.model.element(id);
        let kind = match element.kind {
            ElementKind::Person => "Person",
            ElementKind::SoftwareSystem => "SoftwareSystem",
            ElementKind::Container => "Container",
            ElementKind::Component => "Component",
            ElementKind::Custom => "Custom",
            ElementKind::DeploymentNode => "DeploymentNode",
            ElementKind::InfrastructureNode => "InfrastructureNode",
            ElementKind::SoftwareSystemInstance => "SoftwareSystemInstance",
            ElementKind::ContainerInstance => "ContainerInstance",
        };
        let mut names = vec![element.name.value.as_str()];
        let mut current = element.parent;
        while let Some(p) = current {
            names.push(&self.model.element(p).name.value);
            current = self.model.element(p).parent;
        }
        // DECISION: a deployment element's canonical name is Structurizr's:
        // the environment, then the names from the top node down, joined
        // by `/` (`DeploymentNode://Live/AWS/US-East-1`).
        if let Some(environment) = element.environment {
            names.push(&self.model.environments[environment.0 as usize].name.value);
            names.reverse();
            return format!("{kind}://{}", names.join("/"));
        }
        names.reverse();
        format!("{kind}://{}", names.join("."))
    }

    /// `<id> = !element <canonical name>` (P2-169): gives an element
    /// declared without an id the id at token 0.
    // DECISION: B dsl-breadth 23, checked against Structurizr for Java,
    // which refuses an element that already has an id ("already
    // registered with an identifier"): `DuplicateId` naming that id.
    fn bind(&mut self, head: &Head<'_>, id: ElementId) -> Result<(), ModelError> {
        if head.kind(0) != Some(TokenKind::Word) || !is_identifier(head.text(0)) {
            return Err(self.invalid(head, "=", 0));
        }
        if let Some(existing) = &self.model.element(id).key {
            return Err(self.error(
                ModelErrorKind::DuplicateId {
                    id: existing.value.clone(),
                    first: existing.span,
                    first_position: self.position(existing.span),
                },
                head.span(0),
            ));
        }
        let parent = self.model.element(id).parent;
        let full = self.full_key_of(parent, head.text(0));
        self.check_free(&full, head.span(0))?;
        self.keys.insert(full.to_ascii_lowercase(), id);
        self.model.elements[id.0 as usize].key = Some(Spanned {
            value: head.text(0).to_owned(),
            span: head.span(0),
        });
        Ok(())
    }

    /// `!relationship <id> { ... }`: `tag`, `tags`, `url`, `properties`
    /// and `perspectives` for that relationship (P2-146, P2-167).
    fn relationship_directive(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
    ) -> Result<(), ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "relationship"));
        }
        if head.len() > 2 {
            return Err(self.invalid(head, head.text(0), 2));
        }
        let written = head.text(1);
        let Some(&r) = self.relationship_keys.get(&written.to_ascii_lowercase()) else {
            return Err(self.error(
                ModelErrorKind::UndeclaredId {
                    id: written.to_owned(),
                },
                head.span(1),
            ));
        };
        let index = r.0 as usize;
        for inner in self.block(statement, head)? {
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            self.relationship_line(inner, &head, index)?;
        }
        self.retag_relationship(index);
        Ok(())
    }

    /// Recomputes an element's effective tags: `Element`, its kind tag,
    /// its archetype chain's tags root down, its declared tags (DSL-15;
    /// B dsl-breadth 15b, checked against Structurizr for Java).
    fn retag_element(&mut self, id: ElementId) {
        let element = &mut self.model.elements[id.0 as usize];
        let mut implicit = vec!["Element", element.kind.default_tag()];
        if let Some(tags) = self.archetype_tags.get(&id) {
            implicit.extend(tags.iter().map(String::as_str));
        }
        element.tags = effective_tags(&implicit, &element.declared_tags);
    }

    /// `[<id> =] <kind> <name> ...`. An element without an id parses and
    /// has no key (P2-145).
    // DECISION: the five deployment fields (P3-1) pushed it past 100 lines;
    // kept whole so the declaration reads in one place.
    #[allow(clippy::too_many_lines)]
    fn element(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        parent: Option<ElementId>,
    ) -> Result<(), ModelError> {
        let (key, kw) = declared_at(head);
        let archetype = self.element_archetype(head, kw).cloned();
        let kind = match (&archetype, element_kind(head.text(kw))) {
            (Some(a), _) => match a.base {
                archetype::Base::Element(kind) => kind,
                _ => return Err(self.reject(head, kw)),
            },
            (None, Some(kind)) => kind,
            (None, None) => ElementKind::Custom,
        };
        let keyword = kind.keyword();
        if let Some(k) = key
            && (head.kind(k) != Some(TokenKind::Word) || !is_identifier(head.text(k)))
        {
            return Err(self.invalid(head, keyword, k));
        }
        let parent_kind = parent.map(|p| self.model.element(p).kind);
        let allowed = match kind {
            ElementKind::Person | ElementKind::SoftwareSystem => parent_kind.is_none(),
            ElementKind::Container => parent_kind == Some(ElementKind::SoftwareSystem),
            ElementKind::Component => parent_kind == Some(ElementKind::Container),
            // P2-170: in `model` only (and its groups, B dsl-breadth 14a).
            ElementKind::Custom => parent.is_none(),
            // P3-1: deployment elements are built by `deploy`, never here.
            ElementKind::DeploymentNode
            | ElementKind::InfrastructureNode
            | ElementKind::SoftwareSystemInstance
            | ElementKind::ContainerInstance => false,
        };
        if !allowed {
            return Err(self.error(
                ModelErrorKind::InvalidNesting {
                    kind,
                    parent: parent_kind,
                },
                head.span(kw),
            ));
        }
        let key = key.map(|k| Spanned {
            value: head.text(k).to_owned(),
            span: head.span(k),
        });
        let full = key.as_ref().map(|k| self.full_key_of(parent, &k.value));
        if let (Some(k), Some(full)) = (&key, &full) {
            self.check_free(full, k.span)?;
        }
        let has_technology = matches!(kind, ElementKind::Container | ElementKind::Component);
        let custom = kind == ElementKind::Custom;
        let max = if has_technology || custom { 4 } else { 3 };
        let mut strings = self.strings(head, keyword, kw + 1, max)?.into_iter();
        let Some(name) = strings.next() else {
            return Err(self.error(
                ModelErrorKind::MissingArgument {
                    construct: keyword.into(),
                    argument: "name",
                },
                head.span_of(0, head.len() - 1),
            ));
        };
        self.check_name(kind, parent, &name)?;
        let metadata = if custom { strings.next() } else { None };
        let description = strings.next();
        let technology = if has_technology { strings.next() } else { None };
        let declared_tags: Vec<Literal> = strings.collect();
        let id = ElementId(index_u32(self.model.elements.len()));
        // P2-155, P2-181: positional values win over the archetype's
        // defaults; technology applies to containers and components only,
        // metadata to custom elements only.
        let a = archetype.unwrap_or_else(|| archetype::Archetype::new(archetype::Base::Element(kind)));
        let description = description.or(a.description);
        let technology = if has_technology {
            technology.or(a.technology)
        } else {
            None
        };
        let metadata = if custom { metadata.or(a.metadata) } else { None };
        if !a.tags.is_empty() {
            self.archetype_tags.insert(id, a.tags);
        }
        if let Some(full) = full {
            self.keys.insert(full.to_ascii_lowercase(), id);
        }
        self.model.elements.push(Element {
            id,
            key,
            kind,
            parent,
            name,
            description,
            technology,
            tags: Vec::new(),
            declared_tags,
            span: statement.span,
            group: self.group_stack.last().copied(),
            url: a.url,
            properties: a.properties,
            perspectives: a.perspectives,
            metadata,
            docs: Vec::new(),
            environment: None,
            instance_of: None,
            instance_number: None,
            instances: None,
            deployment_groups: Vec::new(),
        });
        self.element_block(statement, id)?;
        self.retag_element(id);
        Ok(())
    }

    /// The statements of `id`'s block; the groups around the element do
    /// not hold its children.
    fn element_block(&mut self, statement: &Statement, id: ElementId) -> Result<(), ModelError> {
        let Some(block) = &statement.block else {
            return Ok(());
        };
        let groups = std::mem::take(&mut self.group_stack);
        let result = self.each(&block.statements, &mut |this, inner| {
            this.model_statement(inner, Some(id))
        });
        self.group_stack = groups;
        result
    }

    /// The register key of an element `key` declared in `parent`'s block:
    /// `key` under flat ids, `<parent's full id>.key` under hierarchical
    /// ids (P2-6).
    fn full_key_of(&self, parent: Option<ElementId>, key: &str) -> String {
        match (self.model.identifiers, parent.and_then(|p| self.model.full_id(p))) {
            (Identifiers::Hierarchical, Some(prefix)) => format!("{prefix}.{key}"),
            _ => key.to_owned(),
        }
    }

    /// `DuplicateName` for a second person or software system of the same
    /// name, or a second container (component) of the same name in one
    /// software system (container) (P2-177).
    // DECISION: B dsl-breadth 13, checked against Structurizr for Java:
    // the rule holds per kind (a person and a software system may share a
    // name), names compared exactly.
    fn check_name(
        &self,
        kind: ElementKind,
        parent: Option<ElementId>,
        name: &Literal,
    ) -> Result<(), ModelError> {
        if !matches!(
            kind,
            ElementKind::Person
                | ElementKind::SoftwareSystem
                | ElementKind::Container
                | ElementKind::Component
        ) {
            return Ok(());
        }
        let first = self
            .model
            .elements
            .iter()
            .find(|e| e.kind == kind && e.parent == parent && e.name.value == name.value);
        match first {
            Some(first) => Err(self.error(
                ModelErrorKind::DuplicateName {
                    name: name.value.clone(),
                    first: first.name.span,
                    first_position: self.position(first.name.span),
                },
                name.span,
            )),
            None => Ok(()),
        }
    }

    /// A `description`, `technology` or `tags` line in an element block.
    fn element_line(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        id: ElementId,
    ) -> Result<(), ModelError> {
        let kind = self.model.element(id).kind;
        let has_technology = matches!(kind, ElementKind::Container | ElementKind::Component);
        if head.is_keyword(0, "description") {
            let value = self.one_string(head)?;
            self.no_block(statement, head)?;
            self.model.elements[id.0 as usize].description = Some(value);
        } else if head.is_keyword(0, "technology") && has_technology {
            let value = self.one_string(head)?;
            self.no_block(statement, head)?;
            self.model.elements[id.0 as usize].technology = Some(value);
        } else if head.is_keyword(0, "tags") {
            let tags = self.tags_line(head)?;
            self.no_block(statement, head)?;
            self.model.elements[id.0 as usize]
                .declared_tags
                .extend(tags);
        } else if head.is_keyword(0, "tag") {
            // P2-18: one tag, after the tags before it.
            let tag = self.one_string(head)?;
            self.no_block(statement, head)?;
            self.model.elements[id.0 as usize].declared_tags.push(tag);
        } else if head.is_keyword(0, "url") {
            let url = self.one_string(head)?;
            self.no_block(statement, head)?;
            self.model.elements[id.0 as usize].url = Some(url);
        } else if is_attachment(head)
            && matches!(
                kind,
                ElementKind::SoftwareSystem | ElementKind::Container | ElementKind::Component
            )
        {
            let attachment = self.attachment(statement, head)?;
            self.model.elements[id.0 as usize].docs.push(attachment);
        } else if head.is_keyword(0, "group") && kind == ElementKind::Component {
            let name = self.one_string(head)?;
            self.no_block(statement, head)?;
            self.group_line(id, name, head.span_of(0, head.len() - 1));
        } else if head.is_keyword(0, "!components") && kind == ElementKind::Container {
            // P2-3: accepted with no effect; its block is kept as written.
            self.notice("!components", None, head.span_of(0, head.len() - 1));
        } else if head.is_keyword(0, "properties") {
            let properties = self.properties(statement, head)?;
            self.model.elements[id.0 as usize]
                .properties
                .extend(properties);
        } else if head.is_keyword(0, "perspectives") {
            let perspectives = self.perspectives(statement, head)?;
            self.model.elements[id.0 as usize]
                .perspectives
                .extend(perspectives);
        } else {
            // DECISION: `technology` in a person or software system block is
            // an unknown construct there (keyword table: container or
            // component block only).
            return Err(self.reject(head, 0));
        }
        Ok(())
    }

    /// `tags "a,b" ["c"]...`: one or more strings.
    // DECISION: several strings on one `tags` line are accepted, each a
    // literal, as Structurizr does.
    fn tags_line(&self, head: &Head<'_>) -> Result<Vec<Literal>, ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "string"));
        }
        self.strings(head, "tags", 1, usize::MAX)
    }

    /// `[<id> =] [src] -> dst ["description"] ["technology"] ["tags"]
    /// [{ }]`, the id (if any) at token `key` (P2-146).
    fn relationship(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        parent: Option<ElementId>,
        key: Option<usize>,
    ) -> Result<(), ModelError> {
        let from = key.map_or(0, |k| k + 2);
        let arrow = head.arrow().unwrap_or_default();
        if arrow < from || arrow > from + 1 {
            return Err(self.invalid(head, "->", from + 1));
        }
        let key = match key {
            Some(k) => {
                if head.kind(k) != Some(TokenKind::Word) || !is_identifier(head.text(k)) {
                    return Err(self.invalid(head, "->", k));
                }
                // P2-143: relationship ids stay global.
                self.check_free(head.text(k), head.span(k))?;
                Some(Spanned {
                    value: head.text(k).to_owned(),
                    span: head.span(k),
                })
            }
            None => None,
        };
        let index = self.model.relationships.len();
        let mut pending = Vec::new();
        let source = if arrow == from {
            let Some(p) = parent else {
                return Err(self.error(ModelErrorKind::NoEnclosingElement, head.span(0)));
            };
            Spanned {
                value: p,
                span: head.span(0),
            }
        } else {
            let (end, key) = self.end(head, from, parent)?;
            pending.extend(key.map(|key| self.pending_end((index, true), key, end.span, parent)));
            end
        };
        if head.len() <= arrow + 1 {
            return Err(self.error(
                ModelErrorKind::MissingArgument {
                    construct: "->".into(),
                    argument: "destination",
                },
                head.span(arrow),
            ));
        }
        let (destination, dkey) = self.end(head, arrow + 1, parent)?;
        pending.extend(dkey.map(|key| self.pending_end((index, false), key, destination.span, parent)));
        let archetype = self.relationship_archetype(head, arrow)?;
        let mut strings = self.strings(head, "->", arrow + 2, 3)?.into_iter();
        let description = strings.next();
        let technology = strings.next();
        let declared_tags: Vec<Literal> = strings.collect();
        // P2-157: the archetype's values unless written.
        let a = archetype.unwrap_or_else(|| archetype::Archetype::new(archetype::Base::Relationship));
        let description = description.or(a.description);
        let technology = technology.or(a.technology);
        if !a.tags.is_empty() {
            self.relationship_archetype_tags.insert(index, a.tags);
        }
        self.pending.extend(pending);
        if let Some(k) = &key {
            self.relationship_keys
                .insert(k.value.to_ascii_lowercase(), RelationshipId(index_u32(index)));
        }
        self.model.relationships.push(Relationship {
            id: RelationshipId(index_u32(index)),
            key,
            source,
            destination,
            description,
            technology,
            tags: Vec::new(),
            declared_tags,
            span: statement.span,
            url: a.url,
            properties: a.properties,
            perspectives: a.perspectives,
            implied_relationships: self.model.implied_relationships.clone(),
            copied_from: None,
        });
        if let Some(block) = &statement.block {
            for inner in &block.statements {
                let head = self.head(inner)?;
                if self.common(inner, &head)? {
                    continue;
                }
                self.relationship_line(inner, &head, index)?;
            }
        }
        self.retag_relationship(index);
        Ok(())
    }

    /// The `Pending` end `key` (source when `source`) of relationship
    /// `index`, read in `parent`'s block and the current environment.
    fn pending_end(&self, (index, source): (usize, bool), key: String, span: Span, parent: Option<ElementId>) -> Pending {
        Pending {
            relationship: index,
            source,
            key,
            span,
            context: parent,
            environment: self.deploy.environment(),
        }
    }

    /// A `tag`, `tags`, `url`, `properties` or `perspectives` line in a
    /// relationship block (P2-18, P2-171..P2-173); the body Structurizr
    /// allows there.
    fn relationship_line(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        index: usize,
    ) -> Result<(), ModelError> {
        if head.is_keyword(0, "tags") {
            let tags = self.tags_line(head)?;
            self.no_block(statement, head)?;
            self.model.relationships[index].declared_tags.extend(tags);
        } else if head.is_keyword(0, "tag") {
            let tag = self.one_string(head)?;
            self.no_block(statement, head)?;
            self.model.relationships[index].declared_tags.push(tag);
        } else if head.is_keyword(0, "url") {
            let url = self.one_string(head)?;
            self.no_block(statement, head)?;
            self.model.relationships[index].url = Some(url);
        } else if head.is_keyword(0, "properties") {
            let properties = self.properties(statement, head)?;
            self.model.relationships[index].properties.extend(properties);
        } else if head.is_keyword(0, "perspectives") {
            let perspectives = self.perspectives(statement, head)?;
            self.model.relationships[index]
                .perspectives
                .extend(perspectives);
        } else {
            return Err(self.reject(head, 0));
        }
        Ok(())
    }

    /// Recomputes a relationship's effective tags: `Relationship`, its
    /// archetype's tags, its declared tags (DSL-15).
    fn retag_relationship(&mut self, index: usize) {
        let r = &mut self.model.relationships[index];
        let mut implicit = vec!["Relationship"];
        if let Some(tags) = self.relationship_archetype_tags.get(&index) {
            implicit.extend(tags.iter().map(String::as_str));
        }
        r.tags = effective_tags(&implicit, &r.declared_tags);
    }

    /// One relationship end at token `i`: `this`, or an id to resolve
    /// later, returned as the second value (with a placeholder element).
    fn end(
        &self,
        head: &Head<'_>,
        i: usize,
        parent: Option<ElementId>,
    ) -> Result<(Spanned<ElementId>, Option<String>), ModelError> {
        if head.kind(i) != Some(TokenKind::Word) {
            return Err(self.invalid(head, "->", i));
        }
        let span = head.span(i);
        if head.text(i).eq_ignore_ascii_case("this") {
            let Some(p) = parent else {
                return Err(self.error(ModelErrorKind::NoEnclosingElement, span));
            };
            return Ok((Spanned { value: p, span }, None));
        }
        // Placeholder until `resolve_pending`.
        let placeholder = Spanned {
            value: ElementId(u32::MAX),
            span,
        };
        Ok((placeholder, Some(head.text(i).into())))
    }
}

impl Builder<'_> {
    // -------------------------------------------------------------- views

    fn views_block(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        if self
            .model
            .views_span
            .is_some_and(|s| s.file == statement.span.file)
        {
            return Err(self.duplicate_line("views", head));
        }
        self.no_arguments(head)?;
        self.model.views_span = Some(statement.span);
        self.each(self.block(statement, head)?, &mut |this, inner| {
            let head = this.head(inner)?;
            if this.common(inner, &head)? {
                return Ok(());
            }
            let kind = [
                ViewKind::SystemContext,
                ViewKind::Container,
                ViewKind::Component,
            ]
            .into_iter()
            .find(|k| head.is_keyword(0, k.keyword()));
            let other = [
                ViewKind::SystemLandscape,
                ViewKind::Filtered,
                ViewKind::Custom,
                ViewKind::Image,
                ViewKind::Dynamic,
                ViewKind::Deployment,
            ]
            .into_iter()
            .find(|k| head.is_keyword(0, k.keyword()));
            if let Some(kind) = kind {
                this.view(inner, &head, kind)?;
            } else if let Some(kind) = other {
                this.other_view(inner, &head, kind)?;
            } else if head.is_keyword(0, "styles") {
                this.styles_block(inner, &head)?;
            } else if head.is_keyword(0, "properties") {
                let properties = this.properties(inner, &head)?;
                // P2-3: the PlantUML, Mermaid and Kroki view-set properties.
                for p in &properties {
                    let name = p.name.value.to_ascii_lowercase();
                    if ["plantuml.", "mermaid.", "kroki."]
                        .iter()
                        .any(|prefix| name.starts_with(prefix))
                    {
                        this.notice(&p.name.value, None, p.span);
                    }
                }
                this.model.views_properties.extend(properties);
            } else if head.is_keyword(0, "terminology") {
                this.terminology(inner, &head)?;
            } else if head.is_keyword(0, "theme") || head.is_keyword(0, "themes") {
                this.theme_line(inner, &head)?;
            } else {
                return Err(this.reject(&head, 0));
            }
            Ok(())
        })?;
        Ok(())
    }

    /// `terminology { <type> <word> ... metadata <brackets> }` in `views`
    /// (P2-161..P2-163); the words Structurizr's terminology block takes.
    fn terminology(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        self.no_arguments(head)?;
        for inner in self.block(statement, head)? {
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            self.no_block(inner, &head)?;
            if head.is_keyword(0, "metadata") {
                let word = self.one_word(&head)?.to_ascii_lowercase();
                let value = match word.as_str() {
                    "square" => MetadataSymbols::Square,
                    "round" => MetadataSymbols::Round,
                    "curly" => MetadataSymbols::Curly,
                    "angle" => MetadataSymbols::Angle,
                    "double-angle" => MetadataSymbols::DoubleAngle,
                    "none" => MetadataSymbols::None,
                    _ => return Err(self.invalid(&head, "metadata", 1)),
                };
                self.model.terminology.metadata = Some(Spanned {
                    value,
                    span: head.span(1),
                });
                continue;
            }
            let words = [
                "person",
                "softwareSystem",
                "container",
                "component",
                "deploymentNode",
                "infrastructureNode",
                "relationship",
            ];
            let Some(&word) = words.iter().find(|w| head.is_keyword(0, w)) else {
                return Err(self.reject(&head, 0));
            };
            let value = Some(self.one_string(&head)?);
            let t = &mut self.model.terminology;
            match word {
                "person" => t.person = value,
                "softwareSystem" => t.software_system = value,
                "container" => t.container = value,
                "component" => t.component = value,
                "deploymentNode" => t.deployment_node = value,
                "infrastructureNode" => t.infrastructure_node = value,
                _ => t.relationship = value,
            }
        }
        Ok(())
    }

    /// `theme <source>` or `themes <source> [...]` (P2-13): appended to
    /// `Model::themes`; read once the file is built (`theme::apply`).
    // DECISION: any number of `theme`/`themes` lines, in file order, as
    // the language page sets no limit.
    fn theme_line(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        self.no_block(statement, head)?;
        if head.len() < 2 {
            return Err(self.missing(head, "theme"));
        }
        let single = head.is_keyword(0, "theme");
        if single && head.len() > 2 {
            return Err(self.invalid(head, "theme", 2));
        }
        let span = head.span_of(0, head.len() - 1);
        for i in 1..head.len() {
            if !matches!(head.kind(i), Some(TokenKind::Word | TokenKind::String)) {
                return Err(self.invalid(head, head.text(0), i));
            }
            let source = head.literal(i);
            self.model.themes.push(Theme {
                kind: theme::classify(&source.value),
                source,
                span,
            });
        }
        Ok(())
    }

    /// An element named in a view line: declared earlier in the file.
    // DECISION: view lines resolve ids against the elements declared above
    // them, as the Structurizr parser reads top-down (`model` before
    // `views`); only relationships may refer forward.
    fn view_element(&self, head: &Head<'_>, i: usize) -> Result<Spanned<ElementId>, ModelError> {
        if head.kind(i) != Some(TokenKind::Word) {
            return Err(self.invalid(head, head.text(0), i));
        }
        let span = head.span(i);
        match self.resolve(head.text(i), None) {
            Some(value) => Ok(Spanned { value, span }),
            None => Err(self.error(
                ModelErrorKind::UndeclaredId {
                    id: head.text(i).into(),
                },
                span,
            )),
        }
    }

    /// `systemContext|container|component <scope> ["key"] ["description"] { }`.
    fn view(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        kind: ViewKind,
    ) -> Result<(), ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "scope"));
        }
        let scope = self.view_element(head, 1)?;
        let needed = match kind {
            ViewKind::Component => ElementKind::Container,
            _ => ElementKind::SoftwareSystem,
        };
        if self.model.element(scope.value).kind != needed {
            return Err(self.error(
                ModelErrorKind::InvalidScope {
                    kind,
                    scope: head.text(1).into(),
                },
                scope.span,
            ));
        }
        let mut strings = self.strings(head, kind.keyword(), 2, 2)?.into_iter();
        let declared_key = strings.next();
        let description = strings.next();
        let key = match &declared_key {
            Some(literal) => {
                self.check_view_key(literal)?;
                literal.value.clone()
            }
            None => self.generated_key(kind),
        };
        let mut view = new_view(kind, ViewScope::Element(scope), key, declared_key, statement.span);
        view.description = description;
        self.view_body(statement, head, &mut view)?;
        self.check_positions(&view)?;
        self.check_vias(&view)?;
        self.check_anchors(&mut view);
        self.check_bundles(&mut view);
        self.check_bridges(&mut view);
        self.model.views.push(view);
        Ok(())
    }

    /// The effective key: the written one, checked, else `<Kind>-NNN`.
    fn view_key(&self, kind: ViewKind, declared: Option<&Literal>) -> Result<String, ModelError> {
        match declared {
            Some(literal) => {
                self.check_view_key(literal)?;
                Ok(literal.value.clone())
            }
            None => Ok(self.generated_key(kind)),
        }
    }

    /// A view of a kind c4rizer does not draw yet (P2-2, P2-121, P2-124,
    /// P2-179): parsed and checked, in `Model::views`, never planned.
    /// `systemLandscape [key] [description]`, `custom [key] [title]
    /// [description]`, `filtered <base> <include|exclude> <tags> [key]
    /// [description]`, `image <*|element> [key]`, `dynamic
    /// <*|system|container> [key] [description]`, `deployment <*|system>
    /// <environment> [key] [description]`.
    fn other_view(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        kind: ViewKind,
    ) -> Result<(), ModelError> {
        let keyword = kind.keyword();
        let (scope, first_string) = match kind {
            ViewKind::SystemLandscape | ViewKind::Custom => (ViewScope::Unscoped, 1),
            ViewKind::Filtered => {
                if head.len() < 4 {
                    return Err(self.missing(head, "base view, `include|exclude` and tags"));
                }
                if !head.is_string(1) {
                    return Err(self.invalid(head, keyword, 1));
                }
                if !head.is_keyword(2, "include") && !head.is_keyword(2, "exclude") {
                    return Err(self.invalid(head, keyword, 2));
                }
                if !head.is_string(3) {
                    return Err(self.invalid(head, keyword, 3));
                }
                (ViewScope::Unscoped, 4)
            }
            ViewKind::Image | ViewKind::Dynamic => {
                if head.len() < 2 {
                    return Err(self.missing(head, "scope"));
                }
                if head.text(1) == "*" {
                    (ViewScope::Wildcard(head.span(1)), 2)
                } else {
                    let scope = self.view_element(head, 1)?;
                    let fits = kind == ViewKind::Image
                        || matches!(
                            self.model.element(scope.value).kind,
                            ElementKind::SoftwareSystem | ElementKind::Container
                        );
                    if !fits {
                        return Err(self.error(
                            ModelErrorKind::InvalidScope {
                                kind,
                                scope: head.text(1).into(),
                            },
                            scope.span,
                        ));
                    }
                    (ViewScope::Element(scope), 2)
                }
            }
            _ => (self.deployment_scope(head)?, 3),
        };
        let max = match kind {
            ViewKind::Custom => 3,
            ViewKind::Image => 1,
            _ => 2,
        };
        let mut strings = self.strings(head, keyword, first_string, max)?.into_iter();
        let declared_key = strings.next();
        let key = self.view_key(kind, declared_key.as_ref())?;
        let mut view = new_view(kind, scope, key, declared_key, statement.span);
        // DECISION: a custom view's positional title is set after the
        // body, so a body `title` line is no duplicate and wins as the
        // later one, as a body `description` does (Structurizr 6.2.4
        // probed; corpus test.dsl).
        let positional_title = if kind == ViewKind::Custom { strings.next() } else { None };
        view.description = strings.next();
        if kind == ViewKind::Filtered {
            view.filter = Some(self.filter(head)?);
            if statement.block.is_some() {
                self.filtered_body(statement, head, &mut view)?;
            }
        } else if statement.block.is_some() {
            self.view_body(statement, head, &mut view)?;
        }
        if view.title.is_none() {
            view.title = positional_title;
        }
        if kind == ViewKind::Custom {
            self.check_custom(&view)?;
        }
        if kind.drawable() {
            self.check_positions(&view)?;
            self.check_vias(&view)?;
            self.check_anchors(&mut view);
            self.check_bundles(&mut view);
            self.check_bridges(&mut view);
        }
        self.model.views.push(view);
        Ok(())
    }

    /// P2-317: a custom view includes custom elements only. An id of
    /// another element is `NotCustomElement`; an `element.type` of another
    /// type is `InvalidValue` naming it, as Structurizr 6.2.4 refuses it
    /// ("not valid for this view"). Other expressions keep only the custom
    /// elements they select (`Model::included`), as there.
    fn check_custom(&self, view: &ViewSpec) -> Result<(), ModelError> {
        fn bad_type(e: &Expression) -> Option<&str> {
            match e {
                Expression::ElementType { kind, name } if *kind != Some(ElementKind::Custom) => Some(name),
                Expression::And(parts) | Expression::Or(parts) => parts.iter().find_map(bad_type),
                _ => None,
            }
        }
        for selector in &view.include {
            match &selector.value {
                Selector::Element(id) if self.model.element(*id).kind != ElementKind::Custom => {
                    return Err(self.error(
                        ModelErrorKind::NotCustomElement {
                            id: self.written(selector.span).trim_matches('"').to_owned(),
                        },
                        selector.span,
                    ));
                }
                Selector::Expression(e) => {
                    if let Some(name) = bad_type(e) {
                        return Err(self.error(
                            ModelErrorKind::InvalidValue {
                                construct: "element.type".into(),
                                value: name.to_owned(),
                            },
                            selector.span,
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The base, mode and tags of `filtered <base> <include|exclude>
    /// <tags>`; the base must be a static view declared above (P2-306).
    // TODO(srs): P2-306, base kinds and declaration order. DECISION:
    // reading taken (B landscape-filtered 2), checked against Structurizr
    // 6.2.4 `FilteredViewParser`: the base is looked up when the line is
    // read, so it must be declared above (`UnknownView`); a system
    // landscape, system context, container or component view, and also a
    // deployment view as Structurizr allows (drawn when phase 3 draws its
    // base); any other kind is `InvalidBaseView`.
    fn filter(&self, head: &Head<'_>) -> Result<Filter, ModelError> {
        let base = head.literal(1);
        let Some(found) = self.model.view(&base.value) else {
            return Err(self.error(
                ModelErrorKind::UnknownView {
                    key: base.value.clone(),
                },
                base.span,
            ));
        };
        let fits = matches!(
            found.kind,
            ViewKind::SystemLandscape
                | ViewKind::SystemContext
                | ViewKind::Container
                | ViewKind::Component
                | ViewKind::Deployment
        );
        if !fits {
            return Err(self.error(
                ModelErrorKind::InvalidBaseView {
                    key: base.value.clone(),
                    kind: found.kind,
                },
                base.span,
            ));
        }
        let mode = if head.is_keyword(2, "include") {
            FilterMode::Include
        } else {
            FilterMode::Exclude
        };
        Ok(Filter {
            base,
            mode: Spanned {
                value: mode,
                span: head.span(2),
            },
            tags: head.literal(3),
        })
    }

    /// The body of a filtered view: `default`, `title`, `description` and
    /// `properties` (P2-307); any other line is an error naming it.
    // TODO(srs): P2-307, c4rizer extensions in a filtered body. DECISION:
    // reading taken (B landscape-filtered 1): refused like `include`,
    // naming the word (`UnknownConstruct`), never ignored (DSL-4); the
    // layout lives in the base view's lines (P2-9).
    fn filtered_body(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        view: &mut ViewSpec,
    ) -> Result<(), ModelError> {
        let mut seen_description = false;
        self.each(self.block(statement, head)?, &mut |this, inner| {
            let head = this.head(inner)?;
            if this.common(inner, &head)? {
                return Ok(());
            }
            if head.is_keyword(0, "properties") {
                let properties = this.properties(inner, &head)?;
                view.properties.extend(properties);
                return Ok(());
            }
            let allowed = ["default", "title", "description"]
                .iter()
                .any(|k| head.is_keyword(0, k));
            if !allowed {
                return Err(this.reject(&head, 0));
            }
            this.view_line(&head, view, &mut seen_description)?;
            this.no_block(inner, &head)?;
            Ok(())
        })?;
        Ok(())
    }

    /// `deployment <*|system> <environment>`: the scope and environment,
    /// both declared (P2-179).
    fn deployment_scope(&self, head: &Head<'_>) -> Result<ViewScope, ModelError> {
        if head.len() < 3 {
            return Err(self.missing(head, "scope and environment"));
        }
        let software_system = if head.text(1) == "*" {
            None
        } else {
            let scope = self.view_element(head, 1)?;
            if self.model.element(scope.value).kind != ElementKind::SoftwareSystem {
                return Err(self.error(
                    ModelErrorKind::InvalidScope {
                        kind: ViewKind::Deployment,
                        scope: head.text(1).into(),
                    },
                    scope.span,
                ));
            }
            Some(scope)
        };
        if !head.is_string(2) {
            return Err(self.invalid(head, "deployment", 2));
        }
        let environment = head.literal(2);
        // DECISION: the environment is named by its name, exactly, or by
        // its id, ignoring case, as Structurizr finds it either way.
        let lower = environment.value.to_ascii_lowercase();
        let known = self.environments.iter().any(|(name, key)| {
            *name == environment.value || key.as_deref() == Some(lower.as_str())
        });
        if !known {
            return Err(self.error(
                ModelErrorKind::UndeclaredId {
                    id: environment.value,
                },
                head.span(2),
            ));
        }
        Ok(ViewScope::Deployment {
            software_system,
            environment,
        })
    }

    /// The body of any view: shared lines, plus dynamic steps, image
    /// sources and deployment-view selectors for those kinds.
    fn view_body(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        view: &mut ViewSpec,
    ) -> Result<(), ModelError> {
        self.view_environment = self.model.view_environment(view);
        let result = self.view_body_lines(statement, head, view);
        self.view_environment = None;
        result
    }

    /// The lines of `view_body`.
    fn view_body_lines(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        view: &mut ViewSpec,
    ) -> Result<(), ModelError> {
        let mut seen_description = false;
        let mut counter = SequenceCounter::default();
        // `light` then `dark` image sources (P2-320).
        let mut mode_images: [Option<ViewImage>; 2] = [None, None];
        self.each(self.block(statement, head)?, &mut |this, inner| {
            if view.kind == ViewKind::Dynamic && inner.head.is_empty() {
                this.dynamic_parallel(inner, view, &mut counter)?;
                return Ok(());
            }
            let head = this.head(inner)?;
            if this.common(inner, &head)? {
                return Ok(());
            }
            if head.is_keyword(0, "properties") {
                let properties = this.properties(inner, &head)?;
                view.properties.extend(properties);
                return Ok(());
            }
            if head.is_keyword(0, "animation") {
                // P2-178: kept byte for byte, no effect.
                this.no_arguments(&head)?;
                this.block(inner, &head)?;
                // P2-300, P2-316: on the kinds this row draws, the P2-3
                // notice says it draws nothing until phase 3.
                if matches!(view.kind, ViewKind::SystemLandscape | ViewKind::Custom) {
                    this.notice("animation", Some(&view.key), head.span_of(0, head.len() - 1));
                }
                return Ok(());
            }
            let kind = view.kind;
            if kind == ViewKind::Image && (head.is_keyword(0, "light") || head.is_keyword(0, "dark")) {
                let slot = usize::from(head.is_keyword(0, "dark"));
                let image = this.mode_image(inner, &head, view, mode_images[slot].is_some())?;
                mode_images[slot] = image;
                return Ok(());
            }
            match kind {
                ViewKind::Dynamic if !is_dynamic_line(&head) => {
                    let step = this.dynamic_step(&head, view, &mut counter, None)?;
                    view.steps.push(step);
                    return this.dynamic_step_block(inner);
                }
                ViewKind::Image if this.image_source(&head, view)? => {
                    this.no_block(inner, &head)?;
                    return Ok(());
                }
                // P2-320: besides the source, an image view holds only
                // `default`, `title` and `description` (and `properties`).
                ViewKind::Image
                    if !["default", "title", "description"]
                        .iter()
                        .any(|k| head.is_keyword(0, k)) =>
                {
                    return Err(this.reject(&head, 0));
                }
                _ => {}
            }
            this.view_line(&head, view, &mut seen_description)?;
            this.no_block(inner, &head)?;
            Ok(())
        })?;
        if view.image.is_none() {
            let [light, dark] = mode_images;
            view.image = light.or(dark);
        }
        Ok(())
    }

    /// `light { <source> }` or `dark { <source> }` in an image view: the
    /// one source line inside, checked as a plain source is, and a P2-3
    /// notice. `seen` when the view had this block before.
    // TODO(srs): P2-320 names no `light`/`dark` block. Reading taken
    // (Structurizr 6.2.4 opens corpus test.dsl, which has both): one
    // source line per block; the view draws its plain source, else the
    // `light` one, else the `dark` one (c4rizer draws no dark variant).
    fn mode_image(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        view: &ViewSpec,
        seen: bool,
    ) -> Result<Option<ViewImage>, ModelError> {
        self.no_arguments(head)?;
        if seen {
            let construct = if head.is_keyword(0, "dark") { "dark" } else { "light" };
            return Err(self.duplicate_line(construct, head));
        }
        let mut scratch = new_view(
            ViewKind::Image,
            ViewScope::Unscoped,
            view.key.clone(),
            None,
            statement.span,
        );
        let notices = self.model.notices.len();
        for inner in self.block(statement, head)? {
            let line = self.head(inner)?;
            if self.common(inner, &line)? {
                continue;
            }
            if !self.image_source(&line, &mut scratch)? {
                return Err(self.reject(&line, 0));
            }
            self.no_block(inner, &line)?;
        }
        // The block's own notice stands for any its source line gave.
        self.model.notices.truncate(notices);
        let subject = head.text(0).to_ascii_lowercase();
        self.notice(&subject, Some(&view.key), head.span_of(0, head.len() - 1));
        Ok(scratch.image)
    }

    /// A `{ }` block of parallel steps in a dynamic view (P2-121), numbered
    /// as Structurizr 6.2.4's `DynamicViewParallelSequenceDslContext`
    /// does (P3-210): the block starts a parallel sequence and ends it
    /// with "continue numbering" only when it holds no step of its own.
    fn dynamic_parallel(
        &mut self,
        statement: &Statement,
        view: &mut ViewSpec,
        counter: &mut SequenceCounter,
    ) -> Result<(), ModelError> {
        let Some(block) = &statement.block else {
            return Ok(());
        };
        counter.start_parallel();
        let mut has_steps = false;
        for inner in &block.statements {
            if inner.head.is_empty() {
                self.dynamic_parallel(inner, view, counter)?;
                continue;
            }
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            if is_dynamic_line(&head) {
                return Err(self.reject(&head, 0));
            }
            let step = self.dynamic_step(&head, view, counter, Some(statement.span))?;
            view.steps.push(step);
            has_steps = true;
            self.dynamic_step_block(inner)?;
        }
        counter.end_parallel(!has_steps);
        Ok(())
    }

    /// The optional block of a dynamic step: `url` and `properties` lines
    /// only, checked and kept nowhere (`DynamicStep` holds no url or
    /// properties).
    // DECISION: Structurizr 6.2.4 probed (corpus test.dsl): a step block
    // takes `url "<url>"` and `properties { }`; any other line is
    // refused ("expected: url").
    fn dynamic_step_block(&mut self, statement: &Statement) -> Result<(), ModelError> {
        let Some(block) = &statement.block else {
            return Ok(());
        };
        for inner in &block.statements {
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            if head.is_keyword(0, "properties") {
                self.properties(inner, &head)?;
            } else if head.is_keyword(0, "url") {
                self.one_string(&head)?;
                self.no_block(inner, &head)?;
            } else {
                return Err(self.reject(&head, 0));
            }
        }
        Ok(())
    }

    /// `[order:] <src> -> <dst> [desc] [tech]` or `[order:] <relationship
    /// id> [desc]` (P2-121), resolved to the relationship it stands for
    /// and checked against the view's scope (P3-200..P3-209).
    // DECISION: a word ending in `:` first is the order; a word that is no
    // view keyword starts a relationship-id step.
    fn dynamic_step(
        &self,
        head: &Head<'_>,
        view: &ViewSpec,
        counter: &mut SequenceCounter,
        parallel: Option<Span>,
    ) -> Result<DynamicStep, ModelError> {
        let o = usize::from(head.kind(0) == Some(TokenKind::Word) && head.text(0).ends_with(':'));
        if head.len() <= o {
            return Err(self.missing(head, "step"));
        }
        let (source, destination, relationship, response, description, technology) =
            if head.kind(o + 1) == Some(TokenKind::Arrow) {
                if head.len() < o + 3 {
                    return Err(self.missing(head, "destination"));
                }
                let source = self.view_element(head, o)?;
                let destination = self.view_element(head, o + 2)?;
                let mut texts = self.strings(head, "->", o + 3, 2)?.into_iter();
                let (description, technology) = (texts.next(), texts.next());
                let tech = technology.as_ref().map(|t| t.value.as_str());
                let Some((relationship, response)) = self.step_or_response(view, source.value, destination.value, tech)
                else {
                    return Err(self.error(
                        ModelErrorKind::NoRelationship {
                            source: self.model.element(source.value).name.value.clone(),
                            destination: self.model.element(destination.value).name.value.clone(),
                            technology: tech.map(str::to_owned),
                        },
                        head.span_of(o, o + 2),
                    ));
                };
                (source, destination, relationship, response, description, technology)
            } else {
                if head.kind(o) != Some(TokenKind::Word) {
                    return Err(self.invalid(head, "dynamic", o));
                }
                let Some(&relationship) = self.relationship_keys.get(&head.text(o).to_ascii_lowercase()) else {
                    return Err(self.error(
                        ModelErrorKind::UndeclaredId {
                            id: head.text(o).into(),
                        },
                        head.span(o),
                    ));
                };
                let description = self.strings(head, "dynamic", o + 1, 1)?.into_iter().next();
                let r = self.model.relationship(relationship);
                let span = head.span(o);
                (
                    Spanned { value: r.source.value, span },
                    Spanned { value: r.destination.value, span },
                    relationship,
                    false,
                    description,
                    None,
                )
            };
        self.check_step_end(view, source)?;
        self.check_step_end(view, destination)?;
        let written = (o == 1).then(|| head.text(0).trim_end_matches(':').to_owned());
        let order = counter.next(written.as_deref());
        Ok(DynamicStep {
            source,
            destination,
            relationship,
            description,
            technology,
            order,
            written_order: (o == 1).then(|| head.span(0)),
            parallel,
            response,
            span: head.span_of(0, head.len() - 1),
        })
    }

    /// The relationship a step from `source` to `destination` stands for,
    /// and whether the step is a response (P3-201), as Structurizr 6.2.4's
    /// `DynamicView.add` decides: the forward relationship
    /// (`step_relationship`) if any, else the one from `destination` to
    /// `source` (with the written technology, if any) when an earlier
    /// step of the view already stands for it, as a response.
    // DECISION: the reverse lookup goes through `step_relationship` too, so
    // an implied reverse relationship counts as 6.2.4's efferent implied
    // relationships do.
    fn step_or_response(
        &self,
        view: &ViewSpec,
        source: ElementId,
        destination: ElementId,
        technology: Option<&str>,
    ) -> Option<(RelationshipId, bool)> {
        if let Some(r) = self.step_relationship(source, destination, technology) {
            return Some((r, false));
        }
        let r = self.step_relationship(destination, source, technology)?;
        view.steps.iter().any(|s| s.relationship == r).then_some((r, true))
    }

    /// The relationship a step from `source` to `destination` stands for
    /// (P3-201): the first declared from `source` to `destination`, or
    /// implied between them (a relationship between their descendants,
    /// P2-7), in declaration order; with a written technology, only one
    /// with exactly that technology, as Structurizr 6.2.4 selects
    /// (probed: "a relationship between A and B with technology T does
    /// not exist").
    // DECISION: declaration order mixes direct and implied candidates, as
    // Structurizr creates an implied relationship when the relationship
    // it comes from is declared and returns the first efferent one.
    fn step_relationship(
        &self,
        source: ElementId,
        destination: ElementId,
        technology: Option<&str>,
    ) -> Option<RelationshipId> {
        let model = &self.model;
        let within = |e: ElementId, ancestor: ElementId| e == ancestor || model.is_inside(e, ancestor);
        let related = source == destination
            || model.is_inside(source, destination)
            || model.is_inside(destination, source);
        model
            .relationships
            .iter()
            .filter(|r| {
                technology.is_none_or(|t| r.technology.as_ref().is_some_and(|x| x.value == t))
            })
            .find(|r| {
                let (s, d) = (r.source.value, r.destination.value);
                (s == source && d == destination)
                    || (!related
                        && r.implied_relationships != ImpliedRelationships::False
                        && within(s, source)
                        && within(d, destination))
            })
            .map(|r| r.id)
    }

    /// The scope rules of Structurizr 6.2.4's `DynamicView` (P3-204..P3-207).
    // DECISION: a custom element passes every scope, as 6.2.4 returns
    // early for a `CustomElement`.
    // DECISION: 6.2.4 also refuses an element whose parent or child is
    // already in the view (`checkParentAndChildrenHaveNotAlreadyBeenAdded`);
    // the SRS does not ask for it and c4rizer draws both (a difference).
    fn check_step_end(&self, view: &ViewSpec, end: Spanned<ElementId>) -> Result<(), ModelError> {
        let element = self.model.element(end.value);
        let refuse = |reason: String| {
            Err(self.error(
                ModelErrorKind::NotPermittedInView {
                    name: element.name.value.clone(),
                    reason,
                },
                end.span,
            ))
        };
        if element.kind == ElementKind::Custom {
            return Ok(());
        }
        let Some(scope) = view.scope.element() else {
            // P3-206: `dynamic *`.
            if matches!(element.kind, ElementKind::Person | ElementKind::SoftwareSystem) {
                return Ok(());
            }
            return refuse("only people and software systems can be added to this dynamic view".to_owned());
        };
        let scope_element = self.model.element(scope);
        // P3-207, and P3-205's "other than the one holding C".
        if end.value == scope || (scope_element.kind == ElementKind::Container && scope_element.parent == Some(end.value)) {
            return refuse("is already the scope of this view and cannot be added to it".to_owned());
        }
        match element.kind {
            ElementKind::Person | ElementKind::SoftwareSystem | ElementKind::Container => return Ok(()),
            ElementKind::Component => {}
            // P3-204, P3-205 list the kinds a scoped view takes; deployment
            // elements (deployment-view) are not among them.
            // DECISION: the wording is c4rizer's (not probed against 6.2.4),
            // in the style of the P3-206 sentence.
            _ => {
                return refuse(
                    "only people, software systems, containers and components can be added to a dynamic view"
                        .to_owned(),
                );
            }
        }
        if scope_element.kind == ElementKind::SoftwareSystem {
            // P3-204.
            return refuse("components can't be added to a dynamic view when the scope is a software system".to_owned());
        }
        if element.parent != Some(scope) {
            // P3-205. DECISION: 6.2.4 lets a component of another container
            // in; the approved SRS sentence refuses it (a difference).
            return refuse(format!(
                "only components of \"{}\" can be added to this dynamic view",
                scope_element.name.value
            ));
        }
        Ok(())
    }

    /// `plantuml|mermaid|kroki|image <source>` in an image view, kept in
    /// `ViewSpec::image` (P2-320, P2-321). False when `head` is no source
    /// line. A local file is read after the file is built (`theme::apply`
    /// reads it through the loader into `ViewImage::asset`).
    // DECISION: approved extra behaviour (landscape-filtered brief): a
    // URL, PlantUML, Mermaid or Kroki source draws a placeholder naming
    // it and gets a P2-3 notice; nothing is fetched (no network).
    // DECISION: a second source line in one view is `DuplicateLine`, so no
    // source is dropped silently (DSL-4).
    fn image_source(&mut self, head: &Head<'_>, view: &mut ViewSpec) -> Result<bool, ModelError> {
        let min = if head.is_keyword(0, "kroki") { 3 } else { 2 };
        if !["plantuml", "mermaid", "kroki", "image"]
            .iter()
            .any(|k| head.is_keyword(0, k))
        {
            return Ok(false);
        }
        if head.len() < min {
            return Err(self.missing(head, "source"));
        }
        if head.len() > min {
            return Err(self.invalid(head, head.text(0), min));
        }
        if view.image.is_some() {
            return Err(self.duplicate_line("image", head));
        }
        let span = head.span_of(0, head.len() - 1);
        let keyword = head.text(0).to_ascii_lowercase();
        let value = head.literal(1);
        let lower = value.value.to_ascii_lowercase();
        let url = lower.starts_with("https://") || lower.starts_with("http://");
        let source = match keyword.as_str() {
            "plantuml" => ImageSource::PlantUml(value),
            "mermaid" => ImageSource::Mermaid(value),
            "kroki" => ImageSource::Kroki {
                format: value,
                source: head.literal(2),
            },
            _ if url => ImageSource::Url(value),
            _ => ImageSource::File(value),
        };
        // P2-3 (P2-321, B landscape-filtered 10d): accepted, not drawn.
        match &source {
            ImageSource::Url(u) => {
                let subject = format!("image {} (not fetched, a placeholder is drawn)", u.value);
                self.notice(&subject, Some(&view.key), span);
            }
            ImageSource::File(_) => {}
            _ => self.notice(&keyword, Some(&view.key), span),
        }
        view.image = Some(ViewImage {
            source,
            asset: None,
            span,
        });
        Ok(true)
    }

    /// An id in a view line: `resolve`, else, in a deployment view, an
    /// instance without an id of that view's environment named
    /// `<element key>_<n>` (P3-117).
    fn resolve_in_view(&self, t: &str) -> Option<ElementId> {
        self.resolve(t, None).or_else(|| {
            let env = self.view_environment?;
            self.model
                .elements
                .iter()
                .filter(|e| e.environment == Some(env) && e.key.is_none() && e.instance_of.is_some())
                .find(|e| self.model.full_id(e.id).is_some_and(|k| k.eq_ignore_ascii_case(t)))
                .map(|e| e.id)
        })
    }

    /// A written view key: a file-name-safe word, not used before.
    // DECISION: keys are ASCII letters, digits, `_`, `-` and `.`, not
    // starting with `.`: the key names the SVG file (CLI-1).
    fn check_view_key(&self, literal: &Literal) -> Result<(), ModelError> {
        let k = &literal.value;
        let valid = !k.is_empty()
            && !k.starts_with('.')
            && k.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        if !valid {
            return Err(self.error(
                ModelErrorKind::InvalidValue {
                    construct: "view key".into(),
                    value: k.clone(),
                },
                literal.span,
            ));
        }
        if let Some(first) = self.model.view(k) {
            return Err(self.error(
                ModelErrorKind::DuplicateViewKey {
                    key: k.clone(),
                    first: first.span,
                    first_position: self.position(first.span),
                },
                literal.span,
            ));
        }
        Ok(())
    }

    /// `<Kind>-NNN`, the first number not taken (DSL-7).
    // DECISION: a generated key skips numbers already used as keys, as
    // Structurizr does, so it never collides with a written one above it.
    fn generated_key(&self, kind: ViewKind) -> String {
        // DECISION: B dsl-breadth 22, checked against Structurizr for Java
        // (custom and filtered): `Custom-NNN`, `Image-NNN`, `Filtered-NNN`.
        let prefix = match kind {
            ViewKind::SystemContext => "SystemContext",
            ViewKind::Container => "Container",
            ViewKind::Component => "Component",
            ViewKind::SystemLandscape => "SystemLandscape",
            ViewKind::Filtered => "Filtered",
            ViewKind::Custom => "Custom",
            ViewKind::Image => "Image",
            ViewKind::Dynamic => "Dynamic",
            ViewKind::Deployment => "Deployment",
        };
        let mut n = 1;
        loop {
            let key = format!("{prefix}-{n:03}");
            if self.model.view(&key).is_none() {
                return key;
            }
            n += 1;
        }
    }

    /// One line of a view body.
    fn view_line(
        &mut self,
        head: &Head<'_>,
        view: &mut ViewSpec,
        seen_description: &mut bool,
    ) -> Result<(), ModelError> {
        let word = head.text(0).to_ascii_lowercase();
        if head.kind(0) != Some(TokenKind::Word) {
            return Err(self.reject(head, 0));
        }
        match word.as_str() {
            "include" => view.include.extend(self.selectors(head)?),
            "exclude" => view.exclude.extend(self.selectors(head)?),
            "autolayout" => {
                self.overridden(view.auto_layout, head);
                self.auto_layout(head)?;
                view.auto_layout = Some(head.span_of(0, head.len() - 1));
            }
            "title" => {
                self.overridden(view.title.as_ref().map(|t| t.span), head);
                view.title = Some(self.one_string(head)?);
            }
            "description" => {
                let earlier = view.description.as_ref().filter(|_| *seen_description).map(|d| d.span);
                self.overridden(earlier, head);
                *seen_description = true;
                view.description = Some(self.one_string(head)?);
            }
            "subtitle" => {
                self.overridden(view.subtitle.as_ref().map(|t| t.span), head);
                view.subtitle = Some(self.one_string(head)?);
            }
            "technology" => {
                self.overridden(view.technology.map(|t| t.span), head);
                view.technology = Some(self.on_off(head)?);
            }
            "bus" => {
                self.overridden(view.bus.map(|t| t.span), head);
                view.bus = Some(self.on_off(head)?);
            }
            "labelwidth" => {
                self.overridden(view.label_width.map(|t| t.span), head);
                view.label_width = Some(self.count(head, 1)?);
            }
            "maxcrossings" => {
                self.overridden(view.max_crossings.map(|t| t.span), head);
                view.max_crossings = Some(self.count(head, 0)?);
            }
            // P4-300.
            "labels" => {
                self.overridden(view.labels.map(|t| t.span), head);
                let word = self.one_word(head)?;
                let value = LabelMode::from_keyword(word).ok_or_else(|| self.invalid(head, "labels", 1))?;
                view.labels = Some(Spanned {
                    value,
                    span: head.span(1),
                });
            }
            "crossings" | "bridge" => self.bridges_view_line(head, view)?,
            "linecolours" | "linecolors" => self.line_colours_view_line(head, view)?,
            "focus" => {
                if head.len() < 2 {
                    return Err(self.missing(head, "element"));
                }
                for i in 1..head.len() {
                    view.focus.push(self.view_element(head, i)?);
                }
            }
            "hide" => view.hide.push(self.hide(head)?),
            // P2-338. DECISION: a second `default` in the same view is
            // accepted, as Structurizr just sets the default key again.
            "default" => {
                self.no_arguments(head)?;
                view.default = Some(head.span_of(0, head.len() - 1));
            }
            "at" => {
                let (element, x, y) = self.box_line(head, false)?;
                view.positions.push(At {
                    element,
                    x,
                    y,
                    span: head.span_of(0, head.len() - 1),
                });
            }
            "papersize" | "line" => self.layout_line(head, view)?,
            "via" => {
                let via = self.via_line(head, view.kind == ViewKind::Dynamic)?;
                view.vias.push(via);
            }
            "bundle" => match self.bundle_line(head, view.kind == ViewKind::Dynamic) {
                Ok(line) => view.bundles.push(line),
                Err(why) => self.bundle_notice(view, head.span_of(0, head.len() - 1), &why),
            },
            "anchor" => match self.anchor_line(head, view.kind == ViewKind::Dynamic) {
                Ok(line) => view.anchors.push(line),
                Err(why) => self.anchor_notice(view, head.span_of(0, head.len() - 1), &why),
            },
            "size" => {
                let (element, width, height) = self.box_line(head, true)?;
                view.sizes.push(Size {
                    element,
                    width,
                    height,
                    span: head.span_of(0, head.len() - 1),
                });
            }
            _ => return Err(self.reject(head, 0)),
        }
        Ok(())
    }

    /// The entries of an `include` or `exclude` line: `*`, `*?` (P2-323),
    /// an element id, a relationship id (P2-341) or an expression
    /// (P2-324..P2-337).
    // DECISION: a bare `&&`, `||` or `->` token is the unquoted half of an
    // expression holding whitespace (P2-337): an error naming the token,
    // as Structurizr reports "the element/relationship `&&` does not
    // exist".
    fn selectors(&self, head: &Head<'_>) -> Result<Vec<Spanned<Selector>>, ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "element"));
        }
        let mut out = Vec::new();
        for i in 1..head.len() {
            let span = head.span(i);
            if !matches!(head.kind(i), Some(TokenKind::Word | TokenKind::String)) {
                return Err(self.invalid(head, head.text(0), i));
            }
            let t = head.literal(i).value;
            if ["&&", "||", "->"].contains(&t.as_str()) {
                return Err(self.invalid(head, head.text(0), i));
            }
            let value = if t == "*" {
                Selector::All
            } else if t == "*?" {
                Selector::AllReluctant
            } else if is_expression(&t) {
                Selector::Expression(self.view_expression(&t, span)?)
            } else if let Some(id) = self.resolve_in_view(&t) {
                Selector::Element(id)
            } else if let Some(&r) = self.relationship_keys.get(&t.to_ascii_lowercase()) {
                Selector::Relationship(r)
            } else if self
                .model
                .groups
                .iter()
                .any(|g| g.key.as_ref().is_some_and(|k| k.value.eq_ignore_ascii_case(&t)))
            {
                // A group id selects its members (Structurizr 6.2.4).
                Selector::Expression(self.view_expression(&t, span)?)
            } else {
                return Err(self.error(ModelErrorKind::UndeclaredId { id: t }, span));
            };
            out.push(Spanned { value, span });
        }
        Ok(out)
    }

    /// `autoLayout [tb|bt|lr|rl] [rankSep] [nodeSep]`: checked, then kept
    /// as written; the arguments have no effect (CORE-22).
    fn auto_layout(&self, head: &Head<'_>) -> Result<(), ModelError> {
        for i in 1..head.len() {
            let t = head.text(i);
            let valid = head.kind(i) == Some(TokenKind::Word)
                && match i {
                    1 => ["tb", "bt", "lr", "rl"]
                        .iter()
                        .any(|d| t.eq_ignore_ascii_case(d)),
                    2 | 3 => t.parse::<u32>().is_ok(),
                    _ => false,
                };
            if !valid {
                return Err(self.invalid(head, "autoLayout", i));
            }
        }
        Ok(())
    }

    /// `on` or `off`, ignoring case.
    fn on_off(&self, head: &Head<'_>) -> Result<Spanned<bool>, ModelError> {
        let value = self.one_word(head)?;
        let value = if value.eq_ignore_ascii_case("on") {
            true
        } else if value.eq_ignore_ascii_case("off") {
            false
        } else {
            return Err(self.invalid(head, head.text(0), 1));
        };
        Ok(Spanned {
            value,
            span: head.span(1),
        })
    }

    /// A whole number at least `min`.
    fn count(&self, head: &Head<'_>, min: u32) -> Result<Spanned<u32>, ModelError> {
        match self.one_word(head)?.parse::<u32>() {
            Ok(value) if value >= min => Ok(Spanned {
                value,
                span: head.span(1),
            }),
            _ => Err(self.invalid(head, head.text(0), 1)),
        }
    }

    /// Exactly one bare word after the keyword.
    fn one_word<'h>(&self, head: &'h Head<'_>) -> Result<&'h str, ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "value"));
        }
        if head.len() > 2 {
            return Err(self.invalid(head, head.text(0), 2));
        }
        if head.kind(1) != Some(TokenKind::Word) {
            return Err(self.invalid(head, head.text(0), 1));
        }
        Ok(head.text(1))
    }

    /// `hide <src> -> <dst>`.
    fn hide(&self, head: &Head<'_>) -> Result<Hide, ModelError> {
        if head.len() < 4 {
            return Err(self.missing(head, "`<source> -> <destination>`"));
        }
        if head.kind(2) != Some(TokenKind::Arrow) {
            return Err(self.invalid(head, "hide", 2));
        }
        if head.len() > 4 {
            return Err(self.invalid(head, "hide", 4));
        }
        Ok(Hide {
            source: self.view_element(head, 1)?,
            destination: self.view_element(head, 3)?,
            span: head.span_of(0, head.len() - 1),
        })
    }

    /// A `paperSize` (P2-701) or `line` (P2-708..P2-710) extension line,
    /// at most one `paperSize` per view and one `line` per relationship.
    fn layout_line(&mut self, head: &Head<'_>, view: &mut ViewSpec) -> Result<(), ModelError> {
        if head.is_keyword(0, "paperSize") {
            self.overridden(view.paper_size.map(|p| p.span), head);
            let word = self.one_word(head)?;
            let value = PaperSize::from_keyword(word).ok_or_else(|| self.invalid(head, "paperSize", 1))?;
            view.paper_size = Some(Spanned {
                value,
                span: head.span(1),
            });
            return Ok(());
        }
        let line = self.line_layout(head)?;
        if view.lines.iter().any(|l| l.relationship.value == line.relationship.value) {
            return Err(self.duplicate_line("line", head));
        }
        view.lines.push(line);
        Ok(())
    }

    /// `line <relationship id | src -> dst> [position N] [routing
    /// direct|orthogonal|curved] [jump on|off]` (P2-708..P2-710).
    // DECISION: `src -> dst` names the first relationship from `src` to
    // `dst` in declaration order (`Model::find_relationship`), as
    // `edit` writes it; any other one needs its identifier.
    // DECISION: `jump` also takes `true`/`false`, the relationship style's
    // spelling (P2-12); `edit` writes `on`/`off` (B editor-breadth 6).
    // DECISION: each option at most once per line, at least one option.
    fn line_layout(&self, head: &Head<'_>) -> Result<LineLayout, ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "relationship"));
        }
        let (relationship, mut i) = if head.kind(2) == Some(TokenKind::Arrow) {
            if head.len() < 4 {
                return Err(self.missing(head, "destination"));
            }
            let source = self.view_element(head, 1)?;
            let destination = self.view_element(head, 3)?;
            let found = self.model.find_relationship(source.value, destination.value);
            let Some(&id) = found.first() else {
                return Err(self.error(
                    ModelErrorKind::UndeclaredId {
                        id: format!("{} -> {}", head.text(1), head.text(3)),
                    },
                    head.span_of(1, 3),
                ));
            };
            (
                Spanned {
                    value: id,
                    span: head.span_of(1, 3),
                },
                4,
            )
        } else {
            if head.kind(1) != Some(TokenKind::Word) {
                return Err(self.invalid(head, "line", 1));
            }
            let Some(&id) = self.relationship_keys.get(&head.text(1).to_ascii_lowercase()) else {
                return Err(self.error(
                    ModelErrorKind::UndeclaredId {
                        id: head.text(1).into(),
                    },
                    head.span(1),
                ));
            };
            (
                Spanned {
                    value: id,
                    span: head.span(1),
                },
                2,
            )
        };
        if i >= head.len() {
            return Err(self.missing(head, "position, routing or jump"));
        }
        let mut line = LineLayout {
            relationship,
            position: None,
            routing: None,
            jump: None,
            span: head.span_of(0, head.len() - 1),
        };
        while i < head.len() {
            if i + 1 >= head.len() || head.kind(i + 1) != Some(TokenKind::Word) {
                return Err(self.invalid(head, "line", i));
            }
            let (word, value) = (head.text(i).to_ascii_lowercase(), head.text(i + 1));
            let span = head.span(i + 1);
            let bad = || self.invalid(head, "line", i + 1);
            match word.as_str() {
                "position" if line.position.is_none() => {
                    let n = value.parse::<u8>().ok().filter(|n| *n <= 100).ok_or_else(bad)?;
                    line.position = Some(Spanned { value: n, span });
                }
                "routing" if line.routing.is_none() => {
                    let r = [Routing::Direct, Routing::Orthogonal, Routing::Curved]
                        .into_iter()
                        .find(|r| r.line_keyword().eq_ignore_ascii_case(value))
                        .ok_or_else(bad)?;
                    line.routing = Some(Spanned { value: r, span });
                }
                "jump" if line.jump.is_none() => {
                    let on = if value.eq_ignore_ascii_case("on") || value.eq_ignore_ascii_case("true") {
                        true
                    } else if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("false") {
                        false
                    } else {
                        return Err(bad());
                    };
                    line.jump = Some(Spanned { value: on, span });
                }
                _ => return Err(self.invalid(head, "line", i)),
            }
            i += 2;
        }
        Ok(line)
    }

    /// `via [<n>:] <src> -> <dst> <x1> <y1> [<x2> <y2> ...]` (P3-300,
    /// P3-301): the ends as written, an even number of px coordinates,
    /// each a multiple of 10. `<n>:` only in a dynamic view.
    // DECISION: the ends resolve like `hide`'s (any declared element);
    // whether the view draws that line is checked once the view is
    // parsed (`check_vias`), since it needs the whole view.
    // DECISION: an odd count names the lone last value (P3-302: "an
    // error naming the value").
    fn via_line(&self, head: &Head<'_>, dynamic: bool) -> Result<Via, ModelError> {
        let ordered = head.len() > 1 && head.kind(1) == Some(TokenKind::Word) && head.text(1).ends_with(':');
        if ordered && !dynamic {
            return Err(self.invalid(head, "via", 1));
        }
        let o = 1 + usize::from(ordered);
        if head.len() < o + 3 {
            return Err(self.missing(head, "`<source> -> <destination>`"));
        }
        if head.kind(o + 1) != Some(TokenKind::Arrow) {
            return Err(self.invalid(head, "via", o + 1));
        }
        let source = self.view_element(head, o)?;
        let destination = self.view_element(head, o + 2)?;
        let first = o + 3;
        if head.len() <= first {
            return Err(self.missing(head, "vertex"));
        }
        let mut values = Vec::new();
        for i in first..head.len() {
            let value = match head.kind(i) {
                Some(TokenKind::Word) => head.text(i).parse::<i32>().ok(),
                _ => None,
            };
            match value {
                Some(v) if v % 10 == 0 => values.push(v),
                _ => return Err(self.invalid(head, "via", i)),
            }
        }
        if values.len() % 2 == 1 {
            return Err(self.invalid(head, "via", head.len() - 1));
        }
        let points = values
            .chunks(2)
            .enumerate()
            .map(|(k, xy)| Spanned {
                value: (xy[0], xy[1]),
                span: head.span_of(first + 2 * k, first + 2 * k + 1),
            })
            .collect();
        Ok(Via {
            source,
            destination,
            order: ordered.then(|| Spanned {
                value: head.text(1).trim_end_matches(':').to_owned(),
                span: head.span(1),
            }),
            points,
            span: head.span_of(0, head.len() - 1),
        })
    }

    /// Every `via` line names a line the view draws, at most one per
    /// line (P3-302); the first problem in file order is reported.
    // DECISION: "draws" is read from `view::plan` of the view as parsed so
    // far (styles do not change which lines are drawn), so the check and
    // the plan never disagree; `model` reaching into `view` is the price.
    fn check_vias(&self, view: &ViewSpec) -> Result<(), ModelError> {
        if view.vias.is_empty() {
            return Ok(());
        }
        let plan = crate::view::plan(&self.model, view);
        let mut seen: BTreeMap<crate::view::EdgeId, Span> = BTreeMap::new();
        for via in &view.vias {
            let line = self.written(Span::new(via.source.span.file, via.source.span.start, via.destination.span.end)).to_owned();
            let Some(edge) = crate::view::via_edge(&plan, via) else {
                return Err(self.error(
                    ModelErrorKind::NotInView {
                        id: line,
                        view: view.key.clone(),
                    },
                    via.span,
                ));
            };
            if let Some(&first) = seen.get(&edge) {
                return Err(self.error(
                    ModelErrorKind::DuplicateVia {
                        line,
                        first,
                        first_position: self.position(first),
                    },
                    via.span,
                ));
            }
            seen.insert(edge, via.span);
        }
        Ok(())
    }

    /// `anchor [<n>:] <src> -> <dst> [from <point>] [to <point>]`
    /// (P4-200, P4-201): the ends as written, then `from` and/or `to`, each
    /// once, each followed by a corner keyword or a side keyword and an
    /// optional whole percent. `Err` holds why the line is ignored
    /// (P4-203); nothing here fails the file.
    // DECISION: the ends resolve like `via`'s (any declared element);
    // whether the view draws the line is checked once the view is parsed
    // (`check_anchors`), as for `via`.
    fn anchor_line(&self, head: &Head<'_>, dynamic: bool) -> Result<AnchorLine, String> {
        let ordered = head.len() > 1 && head.kind(1) == Some(TokenKind::Word) && head.text(1).ends_with(':');
        if ordered && !dynamic {
            return Err(format!("`{}` names a step outside a dynamic view", head.text(1)));
        }
        let o = 1 + usize::from(ordered);
        if head.len() < o + 3 || head.kind(o + 1) != Some(TokenKind::Arrow) {
            return Err("`anchor` needs `<source> -> <destination>`".to_owned());
        }
        let element = |i: usize| self.view_element(head, i).map_err(|_| format!("no element `{}`", head.text(i)));
        let source = element(o)?;
        let destination = element(o + 2)?;
        let mut from = None;
        let mut to = None;
        let mut i = o + 3;
        while i < head.len() {
            let word = head.text(i).to_ascii_lowercase();
            let slot = match (head.kind(i), word.as_str()) {
                (Some(TokenKind::Word), "from") => &mut from,
                (Some(TokenKind::Word), "to") => &mut to,
                _ => return Err(format!("`{}` is not `from` or `to`", head.text(i))),
            };
            if slot.is_some() {
                return Err(format!("`{word}` is given twice"));
            }
            let Some(point) = (i + 1 < head.len()).then(|| head.text(i + 1)) else {
                return Err(format!("`{word}` needs a point"));
            };
            let percent = (i + 2 < head.len() && head.kind(i + 2) == Some(TokenKind::Word))
                .then(|| head.text(i + 2).parse::<u32>().ok())
                .flatten();
            let last = i + 1 + usize::from(percent.is_some());
            let anchor = crate::anchor::Anchor::parse(point, percent).ok_or_else(|| {
                let written = self.written(head.span_of(i + 1, last));
                format!("`{written}` is not a point (a corner, or a side and a percent 0..100)")
            })?;
            *slot = Some(Spanned {
                value: anchor,
                span: head.span_of(i + 1, last),
            });
            i = last + 1;
        }
        if from.is_none() && to.is_none() {
            return Err("`anchor` needs `from <point>` or `to <point>`".to_owned());
        }
        Ok(AnchorLine {
            source,
            destination,
            order: ordered.then(|| Spanned {
                value: head.text(1).trim_end_matches(':').to_owned(),
                span: head.span(1),
            }),
            from,
            to,
            span: head.span_of(0, head.len() - 1),
        })
    }

    /// Records that the `anchor` line at `span` of `view` is ignored,
    /// and `why` (P4-203).
    fn anchor_notice(&mut self, view: &ViewSpec, span: Span, why: &str) {
        let written = self.written(span).to_owned();
        self.model.notices.push(Notice {
            kind: NoticeKind::AnchorIgnored,
            subject: format!("`{written}`: {why}"),
            view: Some(view.key.clone()),
            span: Some(span),
            position: Some(self.position(span)),
        });
    }

    /// `bundle [<n>:] <src> -> <dst> [<n>:] <src> -> <dst> ...` (P4-400):
    /// one or more members, each the ends as written with an optional step
    /// number. `Err` holds why the whole line is ignored (P4-401); an
    /// unknown element drops only its member, with a notice, in
    /// `check_bundles`.
    // DECISION: members are plain `<src> -> <dst>` triples one after the
    // other (a comma would be part of a word, TokenKind::Word), so the
    // line reads like a list of `via` heads.
    fn bundle_line(&self, head: &Head<'_>, dynamic: bool) -> Result<BundleLine, String> {
        let mut members = Vec::new();
        let mut i = 1;
        while i < head.len() {
            let ordered = head.kind(i) == Some(TokenKind::Word) && head.text(i).ends_with(':');
            if ordered && !dynamic {
                return Err(format!("`{}` names a step outside a dynamic view", head.text(i)));
            }
            let o = i + usize::from(ordered);
            if head.len() < o + 3 || head.kind(o + 1) != Some(TokenKind::Arrow) || head.kind(o) != Some(TokenKind::Word) || head.kind(o + 2) != Some(TokenKind::Word) {
                return Err(format!("`{}` does not start `<source> -> <destination>`", head.text(i)));
            }
            let element = |k: usize| match self.view_element(head, k) {
                Ok(e) => e,
                // An unknown element: kept as id MAX, dropped with a
                // notice by `check_bundles`.
                Err(_) => Spanned {
                    value: ElementId(u32::MAX),
                    span: head.span(k),
                },
            };
            members.push(BundleMember {
                source: element(o),
                destination: element(o + 2),
                order: ordered.then(|| Spanned {
                    value: head.text(i).trim_end_matches(':').to_owned(),
                    span: head.span(i),
                }),
                span: head.span_of(i, o + 2),
            });
            i = o + 3;
        }
        if members.is_empty() {
            return Err("`bundle` needs `<source> -> <destination>` members".to_owned());
        }
        Ok(BundleLine {
            members,
            span: head.span_of(0, head.len() - 1),
        })
    }

    /// Records that the `bundle` line or member at `span` of `view` is
    /// ignored, and `why` (P4-401).
    fn bundle_notice(&mut self, view: &ViewSpec, span: Span, why: &str) {
        let written = self.written(span).to_owned();
        self.model.notices.push(Notice {
            kind: NoticeKind::BundleIgnored,
            subject: format!("`{written}`: {why}"),
            view: Some(view.key.clone()),
            span: Some(span),
            position: Some(self.position(span)),
        });
    }

    /// Keeps, per `bundle` line of `view`, the members that name a line
    /// the view draws and that no earlier member claimed, and the lines
    /// left with two members or more; the rest is dropped with a notice
    /// (P4-401).
    fn check_bundles(&mut self, view: &mut ViewSpec) {
        if view.bundles.is_empty() {
            return;
        }
        let plan = crate::view::plan(&self.model, view);
        let mut seen: BTreeMap<crate::view::EdgeId, Span> = BTreeMap::new();
        let mut kept = Vec::new();
        for mut line in std::mem::take(&mut view.bundles) {
            let mut members = Vec::new();
            for m in std::mem::take(&mut line.members) {
                let why = if m.source.value.0 == u32::MAX || m.destination.value.0 == u32::MAX {
                    let bad = if m.source.value.0 == u32::MAX { m.source.span } else { m.destination.span };
                    Some(format!("no element `{}`", self.written(bad)))
                } else if let Some(edge) = crate::view::bundle_edge(&plan, &m) {
                    if let Some(&first) = seen.get(&edge) {
                        let at = self.position(first);
                        Some(format!("this line is already bundled at {}:{}", at.line, at.column))
                    } else {
                        seen.insert(edge, m.span);
                        None
                    }
                } else {
                    Some("the view draws no such line".to_owned())
                };
                match why {
                    Some(why) => self.bundle_notice(view, m.span, &why),
                    None => members.push(m),
                }
            }
            if members.len() < 2 {
                for m in &members {
                    if let Some(edge) = crate::view::bundle_edge(&plan, m) {
                        seen.remove(&edge);
                    }
                }
                self.bundle_notice(view, line.span, "a bundle needs two members or more");
                continue;
            }
            line.members = members;
            kept.push(line);
        }
        view.bundles = kept;
    }

    /// A `lineColours` (or `lineColors`) line of `view` (P5-300, P5-301).
    fn line_colours_view_line(&mut self, head: &Head<'_>, view: &mut ViewSpec) -> Result<(), ModelError> {
        self.overridden(view.line_colours.map(|t| t.span), head);
        let word = self.one_word(head)?;
        let value = LineColours::from_keyword(word).ok_or_else(|| self.invalid(head, head.text(0), 1))?;
        view.line_colours = Some(Spanned {
            value,
            span: head.span(1),
        });
        Ok(())
    }

    /// A `crossings` (P4-500) or `bridge` (P4-501) line of `view`.
    fn bridges_view_line(&mut self, head: &Head<'_>, view: &mut ViewSpec) -> Result<(), ModelError> {
        if head.is_keyword(0, "crossings") {
            self.overridden(view.crossings.map(|t| t.span), head);
            let word = self.one_word(head)?;
            let value = CrossingStyle::from_keyword(word).ok_or_else(|| self.invalid(head, "crossings", 1))?;
            view.crossings = Some(Spanned {
                value,
                span: head.span(1),
            });
            return Ok(());
        }
        match self.bridge_line(head, view.kind == ViewKind::Dynamic) {
            Ok(line) => view.bridges.push(line),
            Err(why) => self.bridge_notice(view, head.span_of(0, head.len() - 1), &why),
        }
        Ok(())
    }

    /// `bridge [<n>:] <src> -> <dst> [<n>:] <src> -> <dst>` (P4-501):
    /// exactly two lines, read as `bundle_line` reads its members. `Err`
    /// holds why the whole line is ignored; unknown elements and lines the
    /// view does not draw are refused in `check_bridges`.
    fn bridge_line(&self, head: &Head<'_>, dynamic: bool) -> Result<BridgeLine, String> {
        let BundleLine { mut members, span } = self.bundle_line(head, dynamic).map_err(|why| why.replace("`bundle`", "`bridge`"))?;
        if members.len() != 2 {
            return Err(format!("`bridge` names two lines, not {}", members.len()));
        }
        let other = members.pop().expect("two members");
        let carrier = members.pop().expect("two members");
        Ok(BridgeLine { carrier, other, span })
    }

    /// Records that the `bridge` line at `span` of `view` is ignored, and
    /// `why` (P4-501).
    fn bridge_notice(&mut self, view: &ViewSpec, span: Span, why: &str) {
        let written = self.written(span).to_owned();
        self.model.notices.push(Notice {
            kind: NoticeKind::BridgeIgnored,
            subject: format!("`{written}`: {why}"),
            view: Some(view.key.clone()),
            span: Some(span),
            position: Some(self.position(span)),
        });
    }

    /// Keeps the `bridge` lines of `view` whose two lines the view draws,
    /// are not the same line, and whose pair no earlier `bridge` line
    /// names; the rest is dropped with a notice (P4-501). A pair that does
    /// not cross is kept: it is unused until the lines cross.
    // DECISION: the first line per pair wins, as `check_bundles` keeps the
    // first bundle naming a line; the pair is unordered, so `bridge a -> b
    // c -> d` and `bridge c -> d a -> b` are the same pair.
    fn check_bridges(&mut self, view: &mut ViewSpec) {
        if view.bridges.is_empty() {
            return;
        }
        let plan = crate::view::plan(&self.model, view);
        let mut seen: BTreeMap<(crate::view::EdgeId, crate::view::EdgeId), Span> = BTreeMap::new();
        let mut kept = Vec::new();
        for line in std::mem::take(&mut view.bridges) {
            let unknown = [&line.carrier, &line.other].into_iter().find_map(|m| {
                if m.source.value.0 == u32::MAX {
                    Some(m.source.span)
                } else if m.destination.value.0 == u32::MAX {
                    Some(m.destination.span)
                } else {
                    None
                }
            });
            let why = if let Some(bad) = unknown {
                Some(format!("no element `{}`", self.written(bad)))
            } else {
                match (crate::view::bundle_edge(&plan, &line.carrier), crate::view::bundle_edge(&plan, &line.other)) {
                    (Some(a), Some(b)) if a == b => Some("it names the same line twice".to_owned()),
                    (Some(a), Some(b)) => {
                        let pair = (a.min(b), a.max(b));
                        if let Some(&first) = seen.get(&pair) {
                            let at = self.position(first);
                            Some(format!("this pair is already decided at {}:{}", at.line, at.column))
                        } else {
                            seen.insert(pair, line.span);
                            None
                        }
                    }
                    _ => Some("the view draws no such line".to_owned()),
                }
            };
            match why {
                Some(why) => self.bridge_notice(view, line.span, &why),
                None => kept.push(line),
            }
        }
        view.bridges = kept;
    }

    /// Keeps the `anchor` lines of `view` that name a line the view draws,
    /// the first per line; the others are dropped with a notice (P4-203).
    fn check_anchors(&mut self, view: &mut ViewSpec) {
        if view.anchors.is_empty() {
            return;
        }
        let plan = crate::view::plan(&self.model, view);
        let mut seen: BTreeMap<crate::view::EdgeId, Span> = BTreeMap::new();
        let mut kept = Vec::new();
        for line in std::mem::take(&mut view.anchors) {
            let why = if let Some(edge) = crate::view::anchor_edge(&plan, &line) {
                if let Some(&first) = seen.get(&edge) {
                    let at = self.position(first);
                    Some(format!("this line already has an anchor line at {}:{}", at.line, at.column))
                } else {
                    seen.insert(edge, line.span);
                    None
                }
            } else {
                Some("the view draws no such line".to_owned())
            };
            match why {
                Some(why) => self.anchor_notice(view, line.span, &why),
                None => kept.push(line),
            }
        }
        view.anchors = kept;
    }

    /// `at <id> <x> <y>` or, with `size`, `size <id> <w> <h>` (both `> 0`).
    fn box_line(&self, head: &Head<'_>, size: bool) -> Result<BoxLine, ModelError> {
        let names: [&'static str; 3] = if size {
            ["element", "width", "height"]
        } else {
            ["element", "x", "y"]
        };
        if head.len() < 4 {
            return Err(self.missing(head, names[head.len() - 1]));
        }
        if head.len() > 4 {
            return Err(self.invalid(head, head.text(0), 4));
        }
        let number = |i: usize| -> Result<Spanned<i32>, ModelError> {
            let parsed = match head.kind(i) {
                Some(TokenKind::Word) => head.text(i).parse::<i32>().ok(),
                _ => None,
            };
            match parsed {
                Some(value) if !size || value > 0 => Ok(Spanned {
                    value,
                    span: head.span(i),
                }),
                _ => Err(self.invalid(head, head.text(0), i)),
            }
        };
        let element = match self.resolve_in_view(head.text(1)).filter(|_| head.kind(1) == Some(TokenKind::Word)) {
            Some(value) => Spanned {
                value,
                span: head.span(1),
            },
            None => self.view_element(head, 1)?,
        };
        Ok((element, number(2)?, number(3)?))
    }

    /// `at` and `size` lines name included elements, at most once each
    /// (DSL-7, Review Focus); the first problem in file order is reported.
    fn check_positions(&self, view: &ViewSpec) -> Result<(), ModelError> {
        let included: BTreeSet<ElementId> = self.model.included(view).into_iter().collect();
        let mut lines: Vec<(&'static str, Spanned<ElementId>, Span)> = view
            .positions
            .iter()
            .map(|a| ("at", a.element, a.span))
            .chain(view.sizes.iter().map(|s| ("size", s.element, s.span)))
            .collect();
        lines.sort_by_key(|(_, _, span)| span.start);
        let mut seen = BTreeSet::new();
        for (construct, element, span) in lines {
            let id = self.written(element.span).to_owned();
            // P3-122: a node's boundary follows its contents.
            if self.model.element(element.value).kind == ElementKind::DeploymentNode {
                return Err(self.error(ModelErrorKind::DeploymentNodeLayout { id }, span));
            }
            if !included.contains(&element.value) {
                return Err(self.error(
                    ModelErrorKind::NotInView {
                        id,
                        view: view.key.clone(),
                    },
                    span,
                ));
            }
            if !seen.insert((construct, element.value)) {
                return Err(self.error(ModelErrorKind::DuplicateLine { construct, id }, span));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------- styles

    fn styles_block(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        if self
            .model
            .styles_span
            .is_some_and(|s| s.file == statement.span.file)
        {
            return Err(self.duplicate_line("styles", head));
        }
        self.no_arguments(head)?;
        self.model.styles_span = Some(statement.span);
        self.each(self.block(statement, head)?, &mut |this, inner| {
            let head = this.head(inner)?;
            if this.common(inner, &head)? {
                return Ok(());
            }
            let mode = if head.is_keyword(0, "light") {
                Some(Mode::Light)
            } else if head.is_keyword(0, "dark") {
                Some(Mode::Dark)
            } else {
                None
            };
            // TODO(srs): P2-13 places `theme`/`themes` in `views`. Reading
            // taken (Structurizr 6.2.4 opens corpus test.dsl): in `styles`
            // the line reads as in `views`.
            if head.is_keyword(0, "theme") || head.is_keyword(0, "themes") {
                return this.theme_line(inner, &head);
            }
            if let Some(mode) = mode {
                // DECISION: any number of `light`/`dark` blocks, each
                // read in file order (the language page sets no limit).
                this.no_arguments(&head)?;
                this.each(this.block(inner, &head)?, &mut |this, style| {
                    let style_head = this.head(style)?;
                    if this.common(style, &style_head)? {
                        return Ok(());
                    }
                    this.style_statement(style, &style_head, Some(mode))
                })?;
            } else {
                this.style_statement(inner, &head, None)?;
            }
            Ok(())
        })?;
        Ok(())
    }

    /// An `element` or `relationship` style block, top-level (`mode`
    /// `None`) or in `light`/`dark` (P2-21); anything else is DSL-4.
    fn style_statement(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        mode: Option<Mode>,
    ) -> Result<(), ModelError> {
        if head.is_keyword(0, "element") {
            self.element_style(statement, head, mode)
        } else if head.is_keyword(0, "relationship") {
            self.relationship_style(statement, head, mode)
        } else {
            Err(self.reject(head, 0))
        }
    }

    fn element_style(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        mode: Option<Mode>,
    ) -> Result<(), ModelError> {
        let tag = self.one_string(head)?;
        let mut style = ElementStyle {
            tag,
            background: None,
            color: None,
            shape: None,
            span: statement.span,
            stroke: None,
            stroke_width: None,
            width: None,
            height: None,
            font_size: None,
            border: None,
            opacity: None,
            metadata: None,
            description: None,
            icon: None,
            properties: Vec::new(),
            mode,
            theme: None,
        };
        for inner in self.block(statement, head)? {
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            if head.is_keyword(0, "properties") {
                let properties = self.style_properties(inner, &head, !style.properties.is_empty())?;
                style.properties = properties;
                continue;
            }
            self.element_style_line(inner, &head, &mut style)?;
        }
        // P2-440: the deprecated enterprise boundary is accepted, kept and
        // drawn nowhere, with a P2-3 notice naming it.
        if style.tag.value == ENTERPRISE_BOUNDARY_TAG {
            self.model.notices.push(Notice {
                kind: NoticeKind::NoEffect,
                subject: format!("element \"{ENTERPRISE_BOUNDARY_TAG}\""),
                view: None,
                span: Some(statement.span),
                position: Some(text::position(self.tree.source(), statement.span.start)),
            });
        }
        self.model.element_styles.push(style);
        Ok(())
    }

    /// One `relationship "<tag>" { ... }` style block (P2-12).
    fn relationship_style(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        mode: Option<Mode>,
    ) -> Result<(), ModelError> {
        let tag = self.one_string(head)?;
        let mut style = RelationshipStyle {
            tag,
            thickness: None,
            color: None,
            style: None,
            routing: None,
            jump: None,
            font_size: None,
            width: None,
            position: None,
            opacity: None,
            properties: Vec::new(),
            mode,
            theme: None,
            span: statement.span,
        };
        for inner in self.block(statement, head)? {
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            if head.is_keyword(0, "properties") {
                let properties = self.style_properties(inner, &head, !style.properties.is_empty())?;
                style.properties = properties;
                continue;
            }
            self.relationship_style_line(inner, &head, &mut style)?;
        }
        self.model.relationship_styles.push(style);
        Ok(())
    }

    /// One property line of a `relationship` style block (P2-12).
    fn relationship_style_line(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        style: &mut RelationshipStyle,
    ) -> Result<(), ModelError> {
        const KNOWN: &[&str] = &[
            "thickness",
            "color",
            "colour",
            "style",
            "dashed",
            "routing",
            "jump",
            "fontsize",
            "width",
            "position",
            "opacity",
            "metadata",
            "description",
        ];
        let word = head.text(0).to_ascii_lowercase();
        if head.kind(0) != Some(TokenKind::Word) || !KNOWN.contains(&word.as_str()) {
            return Err(self.reject(head, 0));
        }
        self.no_block(statement, head)?;
        // TODO(srs): P2-12 lists no `metadata`/`description`. Reading taken
        // (Structurizr 6.2.4 opens corpus test.dsl): a `true|false` value
        // is checked and changes no scene (nothing stores it).
        if word == "metadata" || word == "description" {
            let construct = if word == "metadata" { "metadata" } else { "description" };
            self.boolean(head, construct)?;
            return Ok(());
        }
        // P2-400: a property set again keeps the last line.
        let earlier = match word.as_str() {
            "thickness" => style.thickness.map(|v| v.span),
            "color" | "colour" => style.color.map(|v| v.span),
            "style" | "dashed" => style.style.map(|v| v.span),
            "routing" => style.routing.map(|v| v.span),
            "jump" => style.jump.map(|v| v.span),
            "fontsize" => style.font_size.map(|v| v.span),
            "width" => style.width.map(|v| v.span),
            "position" => style.position.map(|v| v.span),
            _ => style.opacity.map(|v| v.span),
        };
        self.overridden(earlier, head);
        let unsigned = |v: Spanned<i32>| Spanned {
            value: v.value.unsigned_abs(),
            span: v.span,
        };
        match word.as_str() {
            "thickness" => {
                let v = self.positive(head, "thickness")?;
                style.thickness = Some(unsigned(v));
            }
            "color" | "colour" => {
                let construct = if word == "color" { "color" } else { "colour" };
                style.color = Some(self.colour(head, construct)?);
            }
            "style" => style.style = Some(self.border(head, "style")?),
            "dashed" => {
                // DECISION: `dashed` (cookbook form) and `style` are one
                // property, like `color`/`colour`: setting both is the
                // duplicate-line error (P2-420).
                let v = self.boolean(head, "dashed")?;
                style.style = Some(Spanned {
                    value: if v.value { Border::Dashed } else { Border::Solid },
                    span: v.span,
                });
            }
            "routing" => {
                let value = self.style_value(head, "routing")?;
                let Some(routing) = parse_routing(&value) else {
                    return Err(self.invalid(head, "routing", 1));
                };
                style.routing = Some(Spanned {
                    value: routing,
                    span: head.span(1),
                });
            }
            "jump" => style.jump = Some(self.boolean(head, "jump")?),
            "fontsize" => {
                let v = self.positive(head, "fontSize")?;
                style.font_size = Some(unsigned(v));
            }
            "width" => {
                let v = self.positive(head, "width")?;
                style.width = Some(unsigned(v));
            }
            "position" => {
                let v = self.clamped(head, "position", 0..=100)?;
                style.position = Some(v);
            }
            _ => {
                let v = self.clamped(head, "opacity", 0..=100)?;
                style.opacity = Some(v);
            }
        }
        Ok(())
    }

    /// One property line of an `element` style block (P2-10).
    fn element_style_line(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        style: &mut ElementStyle,
    ) -> Result<(), ModelError> {
        const KNOWN: &[&str] = &[
            "background",
            "color",
            "colour",
            "stroke",
            "shape",
            "icon",
            "width",
            "height",
            "strokewidth",
            "fontsize",
            "border",
            "opacity",
            "metadata",
            "description",
            "iconposition",
        ];
        let word = head.text(0).to_ascii_lowercase();
        if head.kind(0) != Some(TokenKind::Word) || !KNOWN.contains(&word.as_str()) {
            return Err(self.reject(head, 0));
        }
        self.no_block(statement, head)?;
        // P2-400: a property set again keeps the last line.
        let earlier = match word.as_str() {
            "background" => style.background.map(|v| v.span),
            "color" | "colour" => style.color.map(|v| v.span),
            "stroke" => style.stroke.map(|v| v.span),
            "shape" => style.shape.map(|v| v.span),
            "icon" => style.icon.as_ref().map(|v| v.span),
            "width" => style.width.map(|v| v.span),
            "height" => style.height.map(|v| v.span),
            "fontsize" => style.font_size.map(|v| v.span),
            "strokewidth" => style.stroke_width.map(|v| v.span),
            "opacity" => style.opacity.map(|v| v.span),
            "border" => style.border.map(|v| v.span),
            "metadata" => style.metadata.map(|v| v.span),
            "description" => style.description.map(|v| v.span),
            _ => None,
        };
        self.overridden(earlier, head);
        match word.as_str() {
            "iconposition" => {
                // TODO(srs): P2-10 and P2-409 name no `iconPosition`
                // (P2-409: "no DSL property chooses"). Reading taken
                // (Structurizr 6.2.4 probed, corpus test.dsl):
                // `top|bottom|left` in any case is accepted and changes no
                // scene; any other value is refused, as there.
                let value = self.style_value(head, "iconPosition")?;
                if !["top", "bottom", "left"].iter().any(|v| value.eq_ignore_ascii_case(v)) {
                    return Err(self.invalid(head, "iconPosition", 1));
                }
            }
            "background" => {
                style.background = Some(self.colour(head, "background")?);
            }
            "color" | "colour" => {
                // P2-400: one property under two names.
                let construct = if word == "color" { "color" } else { "colour" };
                style.color = Some(self.colour(head, construct)?);
            }
            "stroke" => style.stroke = Some(self.colour(head, "stroke")?),
            "shape" => style.shape = Some(self.shape(head)?),
            "icon" => {
                let value = self.style_value(head, "icon")?;
                style.icon = Some(Spanned {
                    value,
                    span: head.span(1),
                });
            }
            "width" => {
                let v = self.positive(head, "width")?;
                style.width = Some(v);
            }
            "height" => {
                let v = self.positive(head, "height")?;
                style.height = Some(v);
            }
            "fontsize" => {
                let v = self.positive(head, "fontSize")?;
                style.font_size = Some(Spanned {
                    value: v.value.unsigned_abs(),
                    span: v.span,
                });
            }
            "strokewidth" => {
                let v = self.clamped(head, "strokeWidth", 1..=10)?;
                style.stroke_width = Some(v);
            }
            "opacity" => {
                let v = self.clamped(head, "opacity", 0..=100)?;
                style.opacity = Some(v);
            }
            "border" => style.border = Some(self.border(head, "border")?),
            "metadata" => {
                style.metadata = Some(self.boolean(head, "metadata")?);
            }
            _ => {
                let v = self.boolean(head, "description")?;
                style.description = Some(v);
            }
        }
        Ok(())
    }

    /// `properties { name value ... }` in a style block, kept with spans
    /// and no effect (P2-410).
    // DECISION: a private reader for style blocks only; dsl-breadth's
    // P2-172 reader for every other owner may replace it at merge.
    fn style_properties(
        &self,
        statement: &Statement,
        head: &Head<'_>,
        seen: bool,
    ) -> Result<Vec<Property>, ModelError> {
        if seen {
            return Err(self.duplicate_line("properties", head));
        }
        self.no_arguments(head)?;
        let mut out = Vec::new();
        for inner in self.block(statement, head)? {
            let line = self.head(inner)?;
            self.no_block(inner, &line)?;
            if line.len() < 2 {
                return Err(self.missing(&line, "value"));
            }
            if line.len() > 2 {
                return Err(self.invalid(&line, "properties", 2));
            }
            for i in 0..2 {
                if !matches!(line.kind(i), Some(TokenKind::Word | TokenKind::String)) {
                    return Err(self.invalid(&line, "properties", i));
                }
            }
            out.push(Property {
                name: line.literal(0),
                value: line.literal(1),
                span: line.span_of(0, 1),
            });
        }
        Ok(out)
    }

    /// A whole number above 0 (`width`, `height`, `fontSize`).
    fn positive(
        &self,
        head: &Head<'_>,
        construct: &'static str,
    ) -> Result<Spanned<i32>, ModelError> {
        let value = self.style_value(head, construct)?;
        match value.parse::<i32>() {
            Ok(v) if v > 0 => Ok(Spanned {
                value: v,
                span: head.span(1),
            }),
            _ => Err(self.invalid(head, construct, 1)),
        }
    }

    /// A whole number clamped to `min..=max` (`strokeWidth`, `opacity`,
    /// `position`).
    // DECISION: an out-of-range number is clamped with no error (B
    // styles-themes 13 [I], Structurizr for Java's setters); a word that
    // is no number at all is still an invalid value.
    fn clamped(
        &self,
        head: &Head<'_>,
        construct: &'static str,
        range: std::ops::RangeInclusive<u8>,
    ) -> Result<Spanned<u8>, ModelError> {
        let (min, max) = (*range.start(), *range.end());
        let value = self.style_value(head, construct)?;
        let Ok(v) = value.parse::<i64>() else {
            return Err(self.invalid(head, construct, 1));
        };
        let v = u8::try_from(v.clamp(i64::from(min), i64::from(max))).unwrap_or(max);
        Ok(Spanned {
            value: v,
            span: head.span(1),
        })
    }

    /// `solid|dashed|dotted`, any case (P2-405, P2-420).
    fn border(
        &self,
        head: &Head<'_>,
        construct: &'static str,
    ) -> Result<Spanned<Border>, ModelError> {
        let value = self.style_value(head, construct)?;
        let Some(border) = parse_border(&value) else {
            return Err(self.invalid(head, construct, 1));
        };
        Ok(Spanned {
            value: border,
            span: head.span(1),
        })
    }

    /// `true|false`, any case.
    fn boolean(
        &self,
        head: &Head<'_>,
        construct: &'static str,
    ) -> Result<Spanned<bool>, ModelError> {
        let value = self.style_value(head, construct)?;
        let Some(b) = parse_bool(&value) else {
            return Err(self.invalid(head, construct, 1));
        };
        Ok(Spanned {
            value: b,
            span: head.span(1),
        })
    }

    /// The single value of a style property line, as text and token index.
    // DECISION: a style value may be bare (`#0b3c6f`) or quoted
    // (`"#0b3c6f"`), as Structurizr accepts both.
    fn style_value(
        &self,
        head: &Head<'_>,
        construct: &'static str,
    ) -> Result<String, ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "value"));
        }
        if head.len() > 2 {
            return Err(self.invalid(head, construct, 2));
        }
        match head.kind(1) {
            Some(TokenKind::Word | TokenKind::String | TokenKind::TextBlock) => {
                Ok(head.literal(1).value)
            }
            _ => Err(self.invalid(head, construct, 1)),
        }
    }

    fn colour(
        &self,
        head: &Head<'_>,
        construct: &'static str,
    ) -> Result<Spanned<Colour>, ModelError> {
        let value = self.style_value(head, construct)?;
        let Some(colour) = parse_colour(&value) else {
            return Err(self.invalid(head, construct, 1));
        };
        Ok(Spanned {
            value: colour,
            span: head.span(1),
        })
    }

    fn shape(&self, head: &Head<'_>) -> Result<Spanned<Shape>, ModelError> {
        let value = self.style_value(head, "shape")?;
        let Some(shape) = Shape::ALL
            .into_iter()
            .find(|s| s.keyword().eq_ignore_ascii_case(&value))
        else {
            return Err(self.invalid(head, "shape", 1));
        };
        Ok(Spanned {
            value: shape,
            span: head.span(1),
        })
    }
}

/// An `at` or `size` line: the element and its two numbers.
type BoxLine = (Spanned<ElementId>, Spanned<i32>, Spanned<i32>);


/// True when `head` is one of `DYNAMIC_LINES`, not a step.
fn is_dynamic_line(head: &Head<'_>) -> bool {
    let o = usize::from(head.kind(0) == Some(TokenKind::Word) && head.text(0).ends_with(':'));
    o == 0
        && head.kind(1) != Some(TokenKind::Arrow)
        && DYNAMIC_LINES.iter().any(|k| head.is_keyword(0, k))
}

/// The sequence counter of a dynamic view: Structurizr 6.2.4's
/// `SequenceNumber` with its `SequenceCounter` and
/// `ParallelSequenceCounter` (P3-208..P3-210).
struct SequenceCounter {
    /// The counter stack: the view's counter first, one per open parallel
    /// block after it; each holds its last number and whether it was
    /// incremented.
    stack: Vec<(i64, bool)>,
}

impl Default for SequenceCounter {
    fn default() -> Self {
        SequenceCounter {
            stack: vec![(0, false)],
        }
    }
}

impl SequenceCounter {
    /// The next step's number: the written order if any, else the
    /// counter (P3-208, P3-209).
    // DECISION: P3-209 as approved: an integer written order sets the
    // counter, so the following plain steps continue from it. Structurizr
    // 6.2.4 leaves its counter alone (probed `w`: `u->a; 5: a->b; b->t`
    // numbers 1, 5, 3); listed as a difference.
    // DECISION: a non-integer written order (`2a:`) still uses up one
    // count, as 6.2.4 calls `getNext` before `setOrder`; P3-209 is silent.
    fn next(&mut self, written: Option<&str>) -> String {
        let top = self.stack.last_mut().expect("the view's counter is never popped");
        top.0 += 1;
        top.1 = true;
        match written {
            Some(w) => {
                if let Ok(n) = w.parse::<i64>() {
                    top.0 = n;
                }
                w.to_owned()
            }
            None => top.0.to_string(),
        }
    }

    /// `startParallelSequence`: a counter starting from the current one.
    fn start_parallel(&mut self) {
        let from = self.stack.last().map_or(0, |t| t.0);
        self.stack.push((from, false));
    }

    /// `endParallelSequence(continue)`: back to the parent counter; with
    /// `continue`, the parent takes the block's number, plus one when the
    /// block itself counted nothing.
    fn end_parallel(&mut self, continue_numbering: bool) {
        if self.stack.len() < 2 {
            return;
        }
        let (seq, incremented) = self.stack.pop().expect("checked above");
        if continue_numbering {
            let parent = self.stack.last_mut().expect("checked above");
            parent.0 = seq;
            if !incremented {
                parent.0 += 1;
                parent.1 = true;
            }
        }
    }
}

/// The view lines a dynamic view takes besides its steps. `at`, `size`,
/// `paperSize` and `line` are the c4rizer layout lines the editor writes
/// (DSL-9, P2-701, P2-708).
const DYNAMIC_LINES: &[&str] = &[
    "at",
    "via",
    "anchor",
    "bundle",
    "bridge",
    "size",
    "papersize",
    "line",
    "autolayout",
    "default",
    "description",
    "title",
    "properties",
    "subtitle",
    "technology",
    "labelwidth",
    "maxcrossings",
    "bus",
    "labels",
    "crossings",
    "linecolours",
    "linecolors",
];

/// True for a relationship expression (P2-332): a relationship form, or
/// `&&`/`||` over relationship forms.
pub(crate) fn is_relationship_expression(e: &Expression) -> bool {
    match e {
        Expression::Relationships { .. }
        | Expression::RelationshipTag { .. }
        | Expression::RelationshipProperty { .. } => true,
        // An empty `Or` comes only from a relationship pattern whose
        // expression side selects nothing.
        Expression::And(parts) | Expression::Or(parts) => {
            parts.first().is_none_or(is_relationship_expression)
        }
        _ => false,
    }
}

/// True for an `include`/`exclude` entry that is an expression or a
/// relationship pattern, not `*` or an id (P2-178).
fn is_expression(t: &str) -> bool {
    let lower = t.to_ascii_lowercase();
    t == "*?"
        || ["==", "!=", "->", "&&", "||"].iter().any(|op| t.contains(op))
        || lower.starts_with("element.")
        || lower.starts_with("relationship.")
}

/// A view with nothing but its kind, scope, key and statement span set.
fn new_view(
    kind: ViewKind,
    scope: ViewScope,
    key: String,
    declared_key: Option<Literal>,
    span: Span,
) -> ViewSpec {
    ViewSpec {
        kind,
        scope,
        key,
        declared_key,
        description: None,
        title: None,
        subtitle: None,
        include: Vec::new(),
        exclude: Vec::new(),
        auto_layout: None,
        focus: Vec::new(),
        hide: Vec::new(),
        technology: None,
        bus: None,
        label_width: None,
        max_crossings: None,
        labels: None,
        crossings: None,
        line_colours: None,
        positions: Vec::new(),
        sizes: Vec::new(),
        span,
        properties: Vec::new(),
        filter: None,
        image: None,
        steps: Vec::new(),
        default: None,
        paper_size: None,
        lines: Vec::new(),
        vias: Vec::new(),
        anchors: Vec::new(),
        bundles: Vec::new(),
        bridges: Vec::new(),
    }
}

/// True for `!docs`, `!adrs` and `!decisions`.
fn is_attachment(head: &Head<'_>) -> bool {
    ["!docs", "!adrs", "!decisions"]
        .iter()
        .any(|k| head.is_keyword(0, k))
}

/// Where a declaration's id and keyword are: `(Some(0), 2)` for `<id> =
/// <keyword> ...`, `(None, 0)` for `<keyword> ...`.
fn declared_at(head: &Head<'_>) -> (Option<usize>, usize) {
    if head.kind(1) == Some(TokenKind::Equals) {
        (Some(0), 2)
    } else {
        (None, 0)
    }
}

/// `#rrggbb`, hex digits of either case, or a CSS colour name in any
/// letter case (P2-401).
fn parse_colour(text: &str) -> Option<Colour> {
    let Some(hex) = text.strip_prefix('#') else {
        let lower = text.to_ascii_lowercase();
        return CSS_COLOURS
            .binary_search_by(|(name, _)| (*name).cmp(lower.as_str()))
            .ok()
            .map(|i| CSS_COLOURS[i].1);
    };
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Colour {
        r: channel(0)?,
        g: channel(2)?,
        b: channel(4)?,
    })
}

/// The deprecated enterprise boundary's tag: accepted, no effect, a
/// P2-3 notice (P2-440).
const ENTERPRISE_BOUNDARY_TAG: &str = "Boundary:Enterprise";

/// What `apply_order` needs of one element or relationship style block.
struct StyleKey<'m> {
    tag: &'m str,
    mode: Option<Mode>,
    theme: Option<usize>,
}

/// The indices of `blocks` that apply in `mode` to something carrying
/// `tags`, in apply order: by tag order (DSL-15); within one tag, theme
/// blocks first by theme then list order, then the file's top-level
/// blocks, then its `mode` blocks, each in list order (P2-13, P2-430).
// DECISION: B styles-themes 10a/10b: for one tag a `light`/`dark` block
// beats a top-level one wherever each is written; theme styles carry no
// mode and apply in both.
/// The tag the Structurizr renderer adds to a response line's
/// relationship for its style lookup (P3-201).
const RESPONSE_TAG: &str = "Relationship/Response";

fn apply_order(tags: &[String], blocks: &[StyleKey<'_>], mode: Mode) -> Vec<usize> {
    let mut out = Vec::new();
    for tag in tags {
        let mut matching: Vec<(u8, usize, usize)> = blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.tag == tag && b.mode.is_none_or(|m| m == mode))
            .map(|(i, b)| match (b.theme, b.mode) {
                (Some(t), _) => (0, t, i),
                (None, None) => (1, 0, i),
                (None, Some(_)) => (2, 0, i),
            })
            .collect();
        matching.sort_unstable();
        out.extend(matching.into_iter().map(|(_, _, i)| i));
    }
    out
}

/// `value` rounded up to the next multiple of 20, the box size step
/// (DSL-7), so the resolved size is the drawn size (P2-402).
// P2-402 (approver ruling 2026-11-17, replacing B styles-themes 4's
// 10px): literal px, rounded up to the next multiple of 20; the written
// value stays in the style.
fn round_up_to_grid(value: i32) -> i32 {
    value.saturating_add(19).div_euclid(20) * 20
}

/// `solid|dashed|dotted`, any case.
fn parse_border(text: &str) -> Option<Border> {
    match text.to_ascii_lowercase().as_str() {
        "solid" => Some(Border::Solid),
        "dashed" => Some(Border::Dashed),
        "dotted" => Some(Border::Dotted),
        _ => None,
    }
}

/// `direct|orthogonal|curved`, any case (P2-421).
fn parse_routing(text: &str) -> Option<Routing> {
    match text.to_ascii_lowercase().as_str() {
        "direct" => Some(Routing::Direct),
        "orthogonal" => Some(Routing::Orthogonal),
        "curved" => Some(Routing::Curved),
        _ => None,
    }
}

/// `true|false`, any case.
fn parse_bool(text: &str) -> Option<bool> {
    if text.eq_ignore_ascii_case("true") {
        Some(true)
    } else if text.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

/// The 148 CSS Color Module Level 4 named colours, sorted by name for
/// binary search (P2-401; determinism, CORE-25).
const CSS_COLOURS: &[(&str, Colour)] = &[
    ("aliceblue", Colour { r: 0xf0, g: 0xf8, b: 0xff }),
    ("antiquewhite", Colour { r: 0xfa, g: 0xeb, b: 0xd7 }),
    ("aqua", Colour { r: 0x00, g: 0xff, b: 0xff }),
    ("aquamarine", Colour { r: 0x7f, g: 0xff, b: 0xd4 }),
    ("azure", Colour { r: 0xf0, g: 0xff, b: 0xff }),
    ("beige", Colour { r: 0xf5, g: 0xf5, b: 0xdc }),
    ("bisque", Colour { r: 0xff, g: 0xe4, b: 0xc4 }),
    ("black", Colour { r: 0x00, g: 0x00, b: 0x00 }),
    ("blanchedalmond", Colour { r: 0xff, g: 0xeb, b: 0xcd }),
    ("blue", Colour { r: 0x00, g: 0x00, b: 0xff }),
    ("blueviolet", Colour { r: 0x8a, g: 0x2b, b: 0xe2 }),
    ("brown", Colour { r: 0xa5, g: 0x2a, b: 0x2a }),
    ("burlywood", Colour { r: 0xde, g: 0xb8, b: 0x87 }),
    ("cadetblue", Colour { r: 0x5f, g: 0x9e, b: 0xa0 }),
    ("chartreuse", Colour { r: 0x7f, g: 0xff, b: 0x00 }),
    ("chocolate", Colour { r: 0xd2, g: 0x69, b: 0x1e }),
    ("coral", Colour { r: 0xff, g: 0x7f, b: 0x50 }),
    ("cornflowerblue", Colour { r: 0x64, g: 0x95, b: 0xed }),
    ("cornsilk", Colour { r: 0xff, g: 0xf8, b: 0xdc }),
    ("crimson", Colour { r: 0xdc, g: 0x14, b: 0x3c }),
    ("cyan", Colour { r: 0x00, g: 0xff, b: 0xff }),
    ("darkblue", Colour { r: 0x00, g: 0x00, b: 0x8b }),
    ("darkcyan", Colour { r: 0x00, g: 0x8b, b: 0x8b }),
    ("darkgoldenrod", Colour { r: 0xb8, g: 0x86, b: 0x0b }),
    ("darkgray", Colour { r: 0xa9, g: 0xa9, b: 0xa9 }),
    ("darkgreen", Colour { r: 0x00, g: 0x64, b: 0x00 }),
    ("darkgrey", Colour { r: 0xa9, g: 0xa9, b: 0xa9 }),
    ("darkkhaki", Colour { r: 0xbd, g: 0xb7, b: 0x6b }),
    ("darkmagenta", Colour { r: 0x8b, g: 0x00, b: 0x8b }),
    ("darkolivegreen", Colour { r: 0x55, g: 0x6b, b: 0x2f }),
    ("darkorange", Colour { r: 0xff, g: 0x8c, b: 0x00 }),
    ("darkorchid", Colour { r: 0x99, g: 0x32, b: 0xcc }),
    ("darkred", Colour { r: 0x8b, g: 0x00, b: 0x00 }),
    ("darksalmon", Colour { r: 0xe9, g: 0x96, b: 0x7a }),
    ("darkseagreen", Colour { r: 0x8f, g: 0xbc, b: 0x8f }),
    ("darkslateblue", Colour { r: 0x48, g: 0x3d, b: 0x8b }),
    ("darkslategray", Colour { r: 0x2f, g: 0x4f, b: 0x4f }),
    ("darkslategrey", Colour { r: 0x2f, g: 0x4f, b: 0x4f }),
    ("darkturquoise", Colour { r: 0x00, g: 0xce, b: 0xd1 }),
    ("darkviolet", Colour { r: 0x94, g: 0x00, b: 0xd3 }),
    ("deeppink", Colour { r: 0xff, g: 0x14, b: 0x93 }),
    ("deepskyblue", Colour { r: 0x00, g: 0xbf, b: 0xff }),
    ("dimgray", Colour { r: 0x69, g: 0x69, b: 0x69 }),
    ("dimgrey", Colour { r: 0x69, g: 0x69, b: 0x69 }),
    ("dodgerblue", Colour { r: 0x1e, g: 0x90, b: 0xff }),
    ("firebrick", Colour { r: 0xb2, g: 0x22, b: 0x22 }),
    ("floralwhite", Colour { r: 0xff, g: 0xfa, b: 0xf0 }),
    ("forestgreen", Colour { r: 0x22, g: 0x8b, b: 0x22 }),
    ("fuchsia", Colour { r: 0xff, g: 0x00, b: 0xff }),
    ("gainsboro", Colour { r: 0xdc, g: 0xdc, b: 0xdc }),
    ("ghostwhite", Colour { r: 0xf8, g: 0xf8, b: 0xff }),
    ("gold", Colour { r: 0xff, g: 0xd7, b: 0x00 }),
    ("goldenrod", Colour { r: 0xda, g: 0xa5, b: 0x20 }),
    ("gray", Colour { r: 0x80, g: 0x80, b: 0x80 }),
    ("green", Colour { r: 0x00, g: 0x80, b: 0x00 }),
    ("greenyellow", Colour { r: 0xad, g: 0xff, b: 0x2f }),
    ("grey", Colour { r: 0x80, g: 0x80, b: 0x80 }),
    ("honeydew", Colour { r: 0xf0, g: 0xff, b: 0xf0 }),
    ("hotpink", Colour { r: 0xff, g: 0x69, b: 0xb4 }),
    ("indianred", Colour { r: 0xcd, g: 0x5c, b: 0x5c }),
    ("indigo", Colour { r: 0x4b, g: 0x00, b: 0x82 }),
    ("ivory", Colour { r: 0xff, g: 0xff, b: 0xf0 }),
    ("khaki", Colour { r: 0xf0, g: 0xe6, b: 0x8c }),
    ("lavender", Colour { r: 0xe6, g: 0xe6, b: 0xfa }),
    ("lavenderblush", Colour { r: 0xff, g: 0xf0, b: 0xf5 }),
    ("lawngreen", Colour { r: 0x7c, g: 0xfc, b: 0x00 }),
    ("lemonchiffon", Colour { r: 0xff, g: 0xfa, b: 0xcd }),
    ("lightblue", Colour { r: 0xad, g: 0xd8, b: 0xe6 }),
    ("lightcoral", Colour { r: 0xf0, g: 0x80, b: 0x80 }),
    ("lightcyan", Colour { r: 0xe0, g: 0xff, b: 0xff }),
    ("lightgoldenrodyellow", Colour { r: 0xfa, g: 0xfa, b: 0xd2 }),
    ("lightgray", Colour { r: 0xd3, g: 0xd3, b: 0xd3 }),
    ("lightgreen", Colour { r: 0x90, g: 0xee, b: 0x90 }),
    ("lightgrey", Colour { r: 0xd3, g: 0xd3, b: 0xd3 }),
    ("lightpink", Colour { r: 0xff, g: 0xb6, b: 0xc1 }),
    ("lightsalmon", Colour { r: 0xff, g: 0xa0, b: 0x7a }),
    ("lightseagreen", Colour { r: 0x20, g: 0xb2, b: 0xaa }),
    ("lightskyblue", Colour { r: 0x87, g: 0xce, b: 0xfa }),
    ("lightslategray", Colour { r: 0x77, g: 0x88, b: 0x99 }),
    ("lightslategrey", Colour { r: 0x77, g: 0x88, b: 0x99 }),
    ("lightsteelblue", Colour { r: 0xb0, g: 0xc4, b: 0xde }),
    ("lightyellow", Colour { r: 0xff, g: 0xff, b: 0xe0 }),
    ("lime", Colour { r: 0x00, g: 0xff, b: 0x00 }),
    ("limegreen", Colour { r: 0x32, g: 0xcd, b: 0x32 }),
    ("linen", Colour { r: 0xfa, g: 0xf0, b: 0xe6 }),
    ("magenta", Colour { r: 0xff, g: 0x00, b: 0xff }),
    ("maroon", Colour { r: 0x80, g: 0x00, b: 0x00 }),
    ("mediumaquamarine", Colour { r: 0x66, g: 0xcd, b: 0xaa }),
    ("mediumblue", Colour { r: 0x00, g: 0x00, b: 0xcd }),
    ("mediumorchid", Colour { r: 0xba, g: 0x55, b: 0xd3 }),
    ("mediumpurple", Colour { r: 0x93, g: 0x70, b: 0xdb }),
    ("mediumseagreen", Colour { r: 0x3c, g: 0xb3, b: 0x71 }),
    ("mediumslateblue", Colour { r: 0x7b, g: 0x68, b: 0xee }),
    ("mediumspringgreen", Colour { r: 0x00, g: 0xfa, b: 0x9a }),
    ("mediumturquoise", Colour { r: 0x48, g: 0xd1, b: 0xcc }),
    ("mediumvioletred", Colour { r: 0xc7, g: 0x15, b: 0x85 }),
    ("midnightblue", Colour { r: 0x19, g: 0x19, b: 0x70 }),
    ("mintcream", Colour { r: 0xf5, g: 0xff, b: 0xfa }),
    ("mistyrose", Colour { r: 0xff, g: 0xe4, b: 0xe1 }),
    ("moccasin", Colour { r: 0xff, g: 0xe4, b: 0xb5 }),
    ("navajowhite", Colour { r: 0xff, g: 0xde, b: 0xad }),
    ("navy", Colour { r: 0x00, g: 0x00, b: 0x80 }),
    ("oldlace", Colour { r: 0xfd, g: 0xf5, b: 0xe6 }),
    ("olive", Colour { r: 0x80, g: 0x80, b: 0x00 }),
    ("olivedrab", Colour { r: 0x6b, g: 0x8e, b: 0x23 }),
    ("orange", Colour { r: 0xff, g: 0xa5, b: 0x00 }),
    ("orangered", Colour { r: 0xff, g: 0x45, b: 0x00 }),
    ("orchid", Colour { r: 0xda, g: 0x70, b: 0xd6 }),
    ("palegoldenrod", Colour { r: 0xee, g: 0xe8, b: 0xaa }),
    ("palegreen", Colour { r: 0x98, g: 0xfb, b: 0x98 }),
    ("paleturquoise", Colour { r: 0xaf, g: 0xee, b: 0xee }),
    ("palevioletred", Colour { r: 0xdb, g: 0x70, b: 0x93 }),
    ("papayawhip", Colour { r: 0xff, g: 0xef, b: 0xd5 }),
    ("peachpuff", Colour { r: 0xff, g: 0xda, b: 0xb9 }),
    ("peru", Colour { r: 0xcd, g: 0x85, b: 0x3f }),
    ("pink", Colour { r: 0xff, g: 0xc0, b: 0xcb }),
    ("plum", Colour { r: 0xdd, g: 0xa0, b: 0xdd }),
    ("powderblue", Colour { r: 0xb0, g: 0xe0, b: 0xe6 }),
    ("purple", Colour { r: 0x80, g: 0x00, b: 0x80 }),
    ("rebeccapurple", Colour { r: 0x66, g: 0x33, b: 0x99 }),
    ("red", Colour { r: 0xff, g: 0x00, b: 0x00 }),
    ("rosybrown", Colour { r: 0xbc, g: 0x8f, b: 0x8f }),
    ("royalblue", Colour { r: 0x41, g: 0x69, b: 0xe1 }),
    ("saddlebrown", Colour { r: 0x8b, g: 0x45, b: 0x13 }),
    ("salmon", Colour { r: 0xfa, g: 0x80, b: 0x72 }),
    ("sandybrown", Colour { r: 0xf4, g: 0xa4, b: 0x60 }),
    ("seagreen", Colour { r: 0x2e, g: 0x8b, b: 0x57 }),
    ("seashell", Colour { r: 0xff, g: 0xf5, b: 0xee }),
    ("sienna", Colour { r: 0xa0, g: 0x52, b: 0x2d }),
    ("silver", Colour { r: 0xc0, g: 0xc0, b: 0xc0 }),
    ("skyblue", Colour { r: 0x87, g: 0xce, b: 0xeb }),
    ("slateblue", Colour { r: 0x6a, g: 0x5a, b: 0xcd }),
    ("slategray", Colour { r: 0x70, g: 0x80, b: 0x90 }),
    ("slategrey", Colour { r: 0x70, g: 0x80, b: 0x90 }),
    ("snow", Colour { r: 0xff, g: 0xfa, b: 0xfa }),
    ("springgreen", Colour { r: 0x00, g: 0xff, b: 0x7f }),
    ("steelblue", Colour { r: 0x46, g: 0x82, b: 0xb4 }),
    ("tan", Colour { r: 0xd2, g: 0xb4, b: 0x8c }),
    ("teal", Colour { r: 0x00, g: 0x80, b: 0x80 }),
    ("thistle", Colour { r: 0xd8, g: 0xbf, b: 0xd8 }),
    ("tomato", Colour { r: 0xff, g: 0x63, b: 0x47 }),
    ("turquoise", Colour { r: 0x40, g: 0xe0, b: 0xd0 }),
    ("violet", Colour { r: 0xee, g: 0x82, b: 0xee }),
    ("wheat", Colour { r: 0xf5, g: 0xde, b: 0xb3 }),
    ("white", Colour { r: 0xff, g: 0xff, b: 0xff }),
    ("whitesmoke", Colour { r: 0xf5, g: 0xf5, b: 0xf5 }),
    ("yellow", Colour { r: 0xff, g: 0xff, b: 0x00 }),
    ("yellowgreen", Colour { r: 0x9a, g: 0xcd, b: 0x32 }),
];

/// `implicit` then every comma-separated declared tag, trimmed, empty
/// ones and repeats dropped (DSL-15).
// DECISION: a repeated tag keeps its first place: Structurizr keeps tags
// as an ordered set.
fn effective_tags(implicit: &[&str], declared: &[Literal]) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    let declared = declared.iter().flat_map(|l| l.value.split(','));
    for tag in implicit.iter().copied().chain(declared) {
        let tag = tag.trim();
        if !tag.is_empty() && !tags.iter().any(|t| t == tag) {
            tags.push(tag.to_owned());
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    // Size pins: ids are tagged onto every scene item.
    #[test]
    fn element_id_size_is_pinned() {
        assert_eq!(std::mem::size_of::<ElementId>(), 4);
    }

    /// P2-142: `full_key` gives the canonical key `find` resolves; written
    /// text resolves through `find_written`; the key is the text as
    /// written, ordered as text.
    #[test]
    fn p2_142_element_key_round_trips_through_find() {
        let m = model(
            "workspace {\n    !identifiers hierarchical\n    model {\n        shop = softwareSystem \"Shop\" {\n            web = container \"Web\"\n        }\n    }\n}\n",
        );
        let web = m.find_written("shop.web").expect("full id resolves");
        assert_eq!(m.find_written("web"), None, "the bare id does not");
        let key = m.full_key(web).expect("keyed");
        assert_eq!(key.as_str(), "shop.web");
        assert_eq!(key.to_string(), "shop.web");
        assert_eq!(m.find(&key), Some(web));
        assert_eq!(m.find(&ElementKey::from_written("SHOP.WEB")), Some(web), "ignoring case");
        assert!(ElementKey::from_written("a.b") < ElementKey::from_written("a.c"));
        assert_eq!(key.into_string(), "shop.web");
    }

    #[test]
    fn relationship_id_size_is_pinned() {
        assert_eq!(std::mem::size_of::<RelationshipId>(), 4);
    }

    /// The model of `source`, panicking with the error text on failure.
    fn model(source: &str) -> Model {
        match parse(source) {
            Ok(parsed) => parsed.model,
            Err(e) => panic!("{e:?}"),
        }
    }

    /// The error of `source`, panicking when it parses.
    fn error(source: &str) -> ModelError {
        match parse(source) {
            Ok(parsed) => panic!("expected an error, got {:?}", parsed.model),
            Err(e) => e,
        }
    }

    /// `body` inside `workspace { model { ... } }`.
    fn in_model(body: &str) -> String {
        format!("workspace {{\n    model {{\n{body}\n    }}\n}}\n")
    }

    /// The text `span` covers in `source`.
    fn text(source: &str, span: Span) -> &str {
        &source[span.range()]
    }

    // model-01: elements and nesting
    #[test]
    fn dsl_1_elements_with_parents() {
        // Positional values and `description`/`technology`/`tags` lines.
        let src = in_model(
            r#"        user = person "Operator" "Runs the thing." "Staff, Night"
        system = softwareSystem "Shop" {
            description "Sells things."
            tags "Core"
            app = container "Server" "Serves pages." "Rust" "Web" {
                technology "Rust, axum"
                api = component "Api" {
                    description "Answers requests."
                    technology "axum"
                    tags "Entry,  Edge "
                }
            }
        }"#,
        );
        let m = model(&src);
        assert_eq!(m.elements.len(), 4);
        for (i, e) in m.elements.iter().enumerate() {
            assert_eq!(e.id, ElementId(u32::try_from(i).unwrap()));
        }
        let user = m.element(m.find_written("user").unwrap());
        assert_eq!(user.kind, ElementKind::Person);
        assert_eq!(user.parent, None);
        assert_eq!(user.name.value, "Operator");
        assert_eq!(text(&src, user.name.span), "\"Operator\"");
        assert_eq!(user.description.as_ref().unwrap().value, "Runs the thing.");
        assert_eq!(user.technology, None);
        assert_eq!(user.tags, ["Element", "Person", "Staff", "Night"]);
        assert_eq!(user.key.as_ref().unwrap().value, "user");
        assert!(text(&src, user.span).starts_with("user = person"));

        let system = m.element(m.find_written("system").unwrap());
        assert_eq!(system.kind, ElementKind::SoftwareSystem);
        assert_eq!(system.description.as_ref().unwrap().value, "Sells things.");
        assert_eq!(system.tags, ["Element", "Software System", "Core"]);
        assert!(text(&src, system.span).ends_with('}'));

        let app = m.element(m.find_written("app").unwrap());
        assert_eq!(app.kind, ElementKind::Container);
        assert_eq!(app.parent, Some(system.id));
        assert_eq!(app.description.as_ref().unwrap().value, "Serves pages.");
        // The later `technology` line wins over the positional value.
        assert_eq!(app.technology.as_ref().unwrap().value, "Rust, axum");
        assert_eq!(app.tags, ["Element", "Container", "Web"]);

        let api = m.element(m.find_written("api").unwrap());
        assert_eq!(api.kind, ElementKind::Component);
        assert_eq!(api.parent, Some(app.id));
        assert_eq!(api.description.as_ref().unwrap().value, "Answers requests.");
        assert_eq!(api.technology.as_ref().unwrap().value, "axum");
        assert_eq!(api.tags, ["Element", "Component", "Entry", "Edge"]);
        assert_eq!(api.declared_tags.len(), 1);
        assert!(api.has_tag("Edge"));
        assert!(api.has_tag("Element"));
        assert!(!api.has_tag("External"));

        assert_eq!(m.children(system.id), [app.id]);
        assert_eq!(m.children(app.id), [api.id]);
        assert!(m.children(user.id).is_empty());
        assert_eq!(m.find_written("system.app"), Some(app.id));
        assert_eq!(m.find_written("system.app.api"), Some(api.id));
        assert_eq!(m.find_written("app.system"), None);
        assert_eq!(m.find_written("nothing"), None);
    }

    #[test]
    fn invalid_nesting_is_error() {
        let e = error(&in_model(r#"        app = container "Server""#));
        assert_eq!(
            e.kind,
            ModelErrorKind::InvalidNesting {
                kind: ElementKind::Container,
                parent: None
            }
        );
        let e = error(&in_model(
            r#"        user = person "Operator" {
            app = container "Server"
        }"#,
        ));
        assert_eq!(
            e.kind,
            ModelErrorKind::InvalidNesting {
                kind: ElementKind::Container,
                parent: Some(ElementKind::Person)
            }
        );
        assert_eq!(e.position.line, 4);
    }

    #[test]
    fn external_tag_is_detected() {
        let m = model(&in_model(
            r#"        mail = softwareSystem "Mail" "Sends mail." "External""#,
        ));
        assert!(m.elements[0].is_external());
    }

    #[test]
    fn missing_name_and_extra_argument_are_errors() {
        let e = error(&in_model("        user = person"));
        assert!(matches!(
            e.kind,
            ModelErrorKind::MissingArgument {
                argument: "name",
                ..
            }
        ));
        let e = error(&in_model(r#"        user = person "a" "b" "c" "d""#));
        assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }));
    }

    #[test]
    fn dsl_5_undeclared_id_is_error() {
        let src = in_model(
            r#"        user = person "Operator"
        user -> shop "Buys from""#,
        );
        let e = error(&src);
        assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "shop".into() });
        assert_eq!(text(&src, e.span), "shop");
        assert_eq!(
            e.position,
            Position {
                line: 4,
                column: 17
            }
        );
        let shown = e.to_string();
        assert!(shown.starts_with("4:17: "), "{shown}");
        assert!(shown.contains("`shop`"), "{shown}");
    }

    #[test]
    fn duplicate_id_is_error() {
        // Review Focus, S12: the error carries both spans.
        let src = in_model(
            r#"        user = person "Operator"
        user = person "Auditor""#,
        );
        let e = error(&src);
        let ModelErrorKind::DuplicateId {
            id,
            first,
            first_position,
        } = &e.kind
        else {
            panic!("{e:?}");
        };
        assert_eq!(id, "user");
        assert_eq!(first_position.line, 3);
        assert_eq!(text(&src, *first), "user");
        assert_eq!(e.position.line, 4);
        let shown = e.to_string();
        assert!(shown.starts_with("4:9: "), "{shown}");
        assert!(shown.contains("3:9"), "{shown}");
    }

    #[test]
    fn keyword_named_id_parses() {
        // docs/workspace.dsl: `model = container ...`; an id may equal a
        // keyword.
        let m = model(&in_model(
            r#"        system = softwareSystem "Tool" {
            model = container "Diagram file"
            views = container "Exports"
        }"#,
        ));
        assert_eq!(
            m.element(m.find_written("model").unwrap()).name.value,
            "Diagram file"
        );
        assert_eq!(m.find_written("system.views"), Some(ElementId(2)));
    }

    #[test]
    fn keywords_are_case_insensitive() {
        // SRS 4.1.
        let m = model(
            r#"WORKSPACE "W" {
    Model {
        user = PERSON "Operator"
        shop = SoftwareSYSTEM "Shop" {
            DESCRIPTION "Sells."
        }
    }
}
"#,
        );
        assert_eq!(m.name.as_ref().unwrap().value, "W");
        assert_eq!(m.elements[0].kind, ElementKind::Person);
        assert_eq!(m.elements[1].kind, ElementKind::SoftwareSystem);
        assert_eq!(m.elements[1].description.as_ref().unwrap().value, "Sells.");
        // Ids match ignoring case (Structurizr; owner ruling,
        // phase2-decisions Part C, A.3 item 4).
        assert_eq!(m.find_written("USER"), Some(ElementId(0)));
    }

    // model-02: relationships
    #[test]
    fn dsl_2_relationships_top_level_and_nested() {
        let src = in_model(
            r#"        user = person "Operator"
        system = softwareSystem "Shop" {
            app = container "Server" {
                app -> db "Reads from" "SQL" "Data"
            }
            db = container "Database"
        }
        user -> app "Uses" "HTTPS" {
            tags "Main, Web"
        }
        user -> system"#,
        );
        let m = model(&src);
        assert_eq!(m.relationships.len(), 3);
        let (user, app, db, system) = (
            m.find_written("user").unwrap(),
            m.find_written("app").unwrap(),
            m.find_written("db").unwrap(),
            m.find_written("system").unwrap(),
        );
        let nested = m.relationship(RelationshipId(0));
        assert_eq!(nested.id, RelationshipId(0));
        assert_eq!((nested.source.value, nested.destination.value), (app, db));
        assert_eq!(text(&src, nested.source.span), "app");
        assert_eq!(text(&src, nested.destination.span), "db");
        assert_eq!(nested.description.as_ref().unwrap().value, "Reads from");
        assert_eq!(nested.technology.as_ref().unwrap().value, "SQL");
        assert_eq!(nested.tags, ["Relationship", "Data"]);
        assert_eq!(nested.key, None);

        let top = m.relationship(RelationshipId(1));
        assert_eq!((top.source.value, top.destination.value), (user, app));
        assert_eq!(top.description.as_ref().unwrap().value, "Uses");
        assert_eq!(top.technology.as_ref().unwrap().value, "HTTPS");
        assert_eq!(top.tags, ["Relationship", "Main", "Web"]);
        assert_eq!(top.declared_tags.len(), 1);
        assert!(text(&src, top.span).starts_with("user -> app"));
        assert!(text(&src, top.span).ends_with('}'));

        let bare = m.relationship(RelationshipId(2));
        assert_eq!((bare.source.value, bare.destination.value), (user, system));
        assert_eq!(bare.description, None);
        assert_eq!(bare.technology, None);
        assert_eq!(bare.tags, ["Relationship"]);
    }

    #[test]
    fn forward_reference_allowed() {
        // Review Focus: id used before it is declared.
        let m = model(&in_model(
            r#"        user -> shop "Buys from"
        shop = softwareSystem "Shop"
        user = person "Operator""#,
        ));
        let r = &m.relationships[0];
        assert_eq!(r.source.value, m.find_written("user").unwrap());
        assert_eq!(r.destination.value, m.find_written("shop").unwrap());
    }

    #[test]
    fn undeclared_id_before_a_later_error_is_reported_first() {
        // "the first problem in file order", with forward references.
        let src = in_model(
            r#"        user -> nobody
        user = person "Operator"
        bogus line"#,
        );
        let e = error(&src);
        assert_eq!(
            e.kind,
            ModelErrorKind::UndeclaredId {
                id: "nobody".into()
            }
        );
        let src = in_model(
            r#"        user -> later
        bogus line
        later = person "Later"
        user = person "Operator""#,
        );
        let e = error(&src);
        assert_eq!(
            e.kind,
            ModelErrorKind::UnknownConstruct {
                construct: "bogus".into()
            }
        );
    }

    #[test]
    fn this_and_implied_source_in_element_block() {
        // DSL-2: `this -> x` and `-> x "desc"` inside an element block.
        let src = in_model(
            r#"        mail = softwareSystem "Mail"
        user = person "Operator" {
            this -> mail "Reads"
            -> mail "Sends" "SMTP"
        }"#,
        );
        let m = model(&src);
        let (user, mail) = (m.find_written("user").unwrap(), m.find_written("mail").unwrap());
        let this = &m.relationships[0];
        assert_eq!((this.source.value, this.destination.value), (user, mail));
        assert_eq!(text(&src, this.source.span), "this");
        let implied = &m.relationships[1];
        assert_eq!(
            (implied.source.value, implied.destination.value),
            (user, mail)
        );
        assert_eq!(text(&src, implied.source.span), "->");
        assert_eq!(implied.description.as_ref().unwrap().value, "Sends");
        assert_eq!(implied.technology.as_ref().unwrap().value, "SMTP");

        let e = error(&in_model(r#"        -> mail "Sends""#));
        assert_eq!(e.kind, ModelErrorKind::NoEnclosingElement);
        let e = error(&in_model(r#"        this -> mail "Sends""#));
        assert_eq!(e.kind, ModelErrorKind::NoEnclosingElement);
    }

    #[test]
    fn find_relationship_by_ends_and_ordinal() {
        // S6, EDIT-24: two `a -> b` lines are found in source order.
        let m = model(&in_model(
            r#"        a = person "Operator"
        b = softwareSystem "Shop"
        a -> b "First"
        b -> a "Back"
        a -> b "Second""#,
        ));
        let (a, b) = (m.find_written("a").unwrap(), m.find_written("b").unwrap());
        assert_eq!(
            m.find_relationship(a, b),
            [RelationshipId(0), RelationshipId(2)]
        );
        assert_eq!(m.find_relationship(b, a), [RelationshipId(1)]);
        assert!(m.find_relationship(a, a).is_empty());
    }

    /// `model_body` and `views_body` inside one workspace.
    fn in_workspace(model_body: &str, views_body: &str) -> String {
        format!(
            "workspace {{\n    model {{\n{model_body}\n    }}\n    views {{\n{views_body}\n    }}\n}}\n"
        )
    }

    const C: fn(u8, u8, u8) -> Colour = |r, g, b| Colour { r, g, b };

    // model-03: styles
    #[test]
    fn dsl_3_element_styles() {
        let src = in_workspace(
            r#"        user = person "Operator"
        db = softwareSystem "Store" "" "Database""#,
            r#"        styles {
            element "Person" {
                shape person
                background #0B3C6F
                color #ffffff
            }
            element "Database" {
                shape Cylinder
            }
        }"#,
        );
        let m = model(&src);
        assert_eq!(m.element_styles.len(), 2);
        let person = &m.element_styles[0];
        assert_eq!(person.tag.value, "Person");
        assert_eq!(person.shape.unwrap().value, Shape::Person);
        assert_eq!(person.background.unwrap().value, C(0x0b, 0x3c, 0x6f));
        assert_eq!(text(&src, person.background.unwrap().span), "#0B3C6F");
        assert_eq!(person.color.unwrap().value, C(255, 255, 255));
        assert!(text(&src, person.span).starts_with("element \"Person\""));
        assert_eq!(m.element_styles[1].background, None);
        assert!(text(&src, m.styles_span.unwrap()).starts_with("styles {"));
        assert!(text(&src, m.views_span.unwrap()).starts_with("views {"));

        let user = m.style_of(m.find_written("user").unwrap());
        assert_eq!(user.shape, Shape::Person);
        assert_eq!(user.background, C(0x0b, 0x3c, 0x6f));
        assert_eq!(user.color, C(255, 255, 255));
        let db = m.style_of(m.find_written("db").unwrap());
        assert_eq!(db.shape, Shape::Cylinder);
        assert_eq!(db.background, C(0xdd, 0xdd, 0xdd));
        assert_eq!(db.color, C(0, 0, 0));
        assert_eq!(C(0x0b, 0x3c, 0x6f).to_string(), "#0b3c6f");
    }

    #[test]
    fn invalid_style_values_are_errors() {
        for (line, value) in [
            ("background #12345", "#12345"),
            ("background #12345g", "#12345g"),
            ("color reddish", "reddish"),
            ("shape Blob", "Blob"),
        ] {
            let src = in_workspace(
                "",
                &format!(
                    "        styles {{\n            element \"X\" {{\n                {line}\n            }}\n        }}"
                ),
            );
            let e = error(&src);
            let ModelErrorKind::InvalidValue { value: v, .. } = &e.kind else {
                panic!("{line}: {e:?}");
            };
            assert_eq!(v, value);
        }
    }

    #[test]
    fn style_resolution_last_tag_wins() {
        let m = model(&in_workspace(
            r#"        db = softwareSystem "Store" "" "Database, Legacy""#,
            r#"        styles {
            element "Legacy" {
                background #111111
            }
            element "Database" {
                background #222222
                color #333333
            }
            element "Software System" {
                background #444444
                color #555555
                shape Cylinder
            }
        }"#,
        ));
        // Tag order: Element, Software System, Database, Legacy.
        let style = m.style_of(ElementId(0));
        assert_eq!(style.background, C(0x11, 0x11, 0x11));
        assert_eq!(style.color, C(0x33, 0x33, 0x33));
        assert_eq!(style.shape, Shape::Cylinder);
    }

    #[test]
    fn dsl_15_implicit_tags_and_per_property_cascade() {
        // `Element` + kind tag, `Relationship`; unset property takes the
        // keyword-table default.
        let m = model(&in_workspace(
            r#"        user = person "Operator"
        shop = softwareSystem "Shop" {
            app = container "App" {
                api = component "Api"
            }
        }
        user -> shop"#,
            r#"        styles {
            element "Element" {
                color #010203
            }
        }"#,
        ));
        let tags = |k: &str| m.element(m.find_written(k).unwrap()).tags.clone();
        assert_eq!(tags("user"), ["Element", "Person"]);
        assert_eq!(tags("shop"), ["Element", "Software System"]);
        assert_eq!(tags("app"), ["Element", "Container"]);
        assert_eq!(tags("api"), ["Element", "Component"]);
        assert_eq!(m.relationships[0].tags, ["Relationship"]);

        let user = m.style_of(m.find_written("user").unwrap());
        assert_eq!(user.color, C(1, 2, 3));
        assert_eq!(user.background, C(0xdd, 0xdd, 0xdd));
        assert_eq!(user.shape, Shape::Person);
        assert_eq!(m.style_of(m.find_written("api").unwrap()).shape, Shape::Box);
        assert_eq!(
            ResolvedStyle::default_for(ElementKind::Person),
            ResolvedStyle {
                background: C(0xdd, 0xdd, 0xdd),
                color: C(0, 0, 0),
                shape: Shape::Person,
                ..ResolvedStyle::default_for(ElementKind::Person)
            }
        );
        assert_eq!(
            ResolvedStyle::default_for(ElementKind::Container).shape,
            Shape::Box
        );
    }

    #[test]
    fn styles_for_lists_blocks_in_apply_order() {
        // S5, DSL-13.
        let m = model(&in_workspace(
            r#"        db = softwareSystem "Store" "" "Database""#,
            r#"        styles {
            element "Database" {
                shape Cylinder
            }
            element "Unused" {
                shape Person
            }
            element "Element" {
                background #000000
            }
            element "Database" {
                background #ffffff
            }
        }"#,
        ));
        assert_eq!(m.styles_for(ElementId(0)), [2, 0, 3]);
        assert_eq!(m.style_of(ElementId(0)).background, C(255, 255, 255));
    }

    #[test]
    fn relationship_style_block_is_parsed() {
        // P2-12: `relationship "<tag>" { ... }` in `styles` is parsed
        // into `relationship_styles` and kept byte for byte (DSL-11).
        let src = in_workspace(
            r#"        user = person "Operator""#,
            r#"        styles {
            relationship "Relationship" {
                routing Orthogonal
                thickness 4
            }
        }"#,
        );
        let parsed = parse(&src).expect("parses");
        assert!(parsed.model.element_styles.is_empty());
        let style = &parsed.model.relationship_styles[0];
        assert_eq!(style.routing.unwrap().value, Routing::Orthogonal);
        assert_eq!(style.thickness.unwrap().value, 4);
        assert_eq!(parsed.trees[0].print(), src);
    }

    #[test]
    fn styles_outside_views_is_error() {
        // DSL-3: `styles` sits inside `views`.
        let src = "workspace {\n    styles {\n    }\n}\n";
        let e = error(src);
        assert_eq!(
            e.kind,
            ModelErrorKind::UnknownConstruct {
                construct: "styles".into()
            }
        );
        let e = error(&in_model("        styles {\n        }"));
        assert!(matches!(e.kind, ModelErrorKind::UnknownConstruct { .. }));
    }

    /// A model with every view kind's `include *` cases.
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

    /// The keys of `ids`, for readable asserts.
    fn keys(m: &Model, ids: &[ElementId]) -> Vec<String> {
        ids.iter()
            .map(|&id| m.element(id).key.as_ref().unwrap().value.clone())
            .collect()
    }

    // model-04: views
    #[test]
    fn dsl_6_view_options() {
        // technology, bus, labelWidth, maxCrossings, focus, hide a -> b.
        let src = in_workspace(
            SHOP,
            r#"        container shop "c2" {
            include *
            technology off
            bus ON
            labelWidth 16
            maxCrossings 2
            focus web db
            focus job
            hide user -> web
            hide api -> db
        }
        systemContext shop "c1" {
            include *
        }"#,
        );
        let m = model(&src);
        let v = m.view("c2").unwrap();
        assert_eq!(v.kind, ViewKind::Container);
        assert_eq!(v.scope.element(), m.find_written("shop"));
        assert!(!v.technology.unwrap().value);
        assert_eq!(text(&src, v.technology.unwrap().span), "off");
        assert!(v.bus.unwrap().value);
        assert_eq!(v.label_width.unwrap().value, 16);
        assert_eq!(v.max_crossings.unwrap().value, 2);
        assert_eq!(
            v.options(),
            ViewOptions {
                technology: false,
                bus: true,
                label_width: 16,
                max_crossings: 2,
                labels: LabelMode::Line,
                crossings: CrossingStyle::Gap,
                line_colours: LineColours::Plain,
            }
        );
        let focus: Vec<ElementId> = v.focus.iter().map(|f| f.value).collect();
        assert_eq!(keys(&m, &focus), ["web", "db", "job"]);
        assert_eq!(v.hide.len(), 2);
        assert_eq!(v.hide[0].source.value, m.find_written("user").unwrap());
        assert_eq!(v.hide[0].destination.value, m.find_written("web").unwrap());
        assert_eq!(text(&src, v.hide[1].span), "hide api -> db");
        assert_eq!(v.include.len(), 1);
        assert_eq!(v.include[0].value, Selector::All);
        assert_eq!(m.view("c1").unwrap().options(), ViewOptions::DEFAULT);
        assert!(m.view("c3").is_none());

        for bad in [
            "technology maybe",
            "bus 1",
            "labelWidth 0",
            "labelWidth wide",
            "maxCrossings -1",
        ] {
            let e = error(&in_workspace(
                SHOP,
                &format!("        container shop {{\n            {bad}\n        }}"),
            ));
            assert!(
                matches!(e.kind, ModelErrorKind::InvalidValue { .. }),
                "{bad}: {e:?}"
            );
        }
        // P2-400: a repeated view property line wins over the earlier
        // one, which gets a no-effect notice.
        let m = model(&in_workspace(
            SHOP,
            "        container shop {\n            bus on\n            bus off\n        }",
        ));
        assert_eq!(m.views[0].bus.map(|b| b.value), Some(false));
        assert!(m.notices.iter().any(|n| n.kind == NoticeKind::NoEffect && n.subject.contains("`bus`")), "{:?}", m.notices);
    }

    #[test]
    fn include_star_follows_the_view_kind() {
        // Keyword table, `include`: C1, C2 and C3 selections.
        let m = model(&in_workspace(
            SHOP,
            r#"        systemContext shop "c1" {
            include *
        }
        container shop "c2" {
            include *
        }
        component web "c3" {
            include *
        }
        component web "c3-picked" {
            include api user
            include other
            exclude other
        }"#,
        ));
        let included = |key: &str| keys(&m, &m.included(m.view(key).unwrap()));
        assert_eq!(included("c1"), ["user", "admin", "mail", "shop"]);
        assert_eq!(included("c2"), ["user", "mail", "web", "db", "job"]);
        assert_eq!(included("c3"), ["mail", "api", "auth", "db", "job"]);
        assert_eq!(included("c3-picked"), ["user", "api"]);
    }

    #[test]
    fn invalid_scope_is_error() {
        let e = error(&in_workspace(SHOP, "        container web {\n        }"));
        assert_eq!(
            e.kind,
            ModelErrorKind::InvalidScope {
                kind: ViewKind::Container,
                scope: "web".into()
            }
        );
        let e = error(&in_workspace(SHOP, "        component shop {\n        }"));
        assert!(matches!(e.kind, ModelErrorKind::InvalidScope { .. }));
        let e = error(&in_workspace(
            SHOP,
            "        systemContext nowhere {\n        }",
        ));
        assert_eq!(
            e.kind,
            ModelErrorKind::UndeclaredId {
                id: "nowhere".into()
            }
        );
        let e = error(&in_workspace(
            SHOP,
            "        systemContext shop {\n            include ghost\n        }",
        ));
        assert_eq!(e.kind, ModelErrorKind::UndeclaredId { id: "ghost".into() });
    }

    #[test]
    fn view_key_and_description_optional_title_wins() {
        // DSL-6: positional string is the description; `title` wins;
        // missing key is generated.
        let src = in_workspace(
            SHOP,
            r#"        systemContext shop {
            include *
        }
        systemContext shop "ctx" "Context of the shop" {
            include *
        }
        container shop {
            description "Parts of the shop"
            title "The shop"
            subtitle "What runs"
        }
        systemContext shop {
        }"#,
        );
        let m = model(&src);
        let keys: Vec<&str> = m.views.iter().map(|v| v.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "SystemContext-001",
                "ctx",
                "Container-001",
                "SystemContext-002"
            ]
        );
        assert_eq!(m.views[0].declared_key, None);
        assert_eq!(m.views[0].drawn_title(), None);
        let ctx = &m.views[1];
        assert_eq!(
            text(&src, ctx.declared_key.as_ref().unwrap().span),
            "\"ctx\""
        );
        assert_eq!(ctx.drawn_title().unwrap().value, "Context of the shop");
        let c2 = &m.views[2];
        assert_eq!(c2.description.as_ref().unwrap().value, "Parts of the shop");
        assert_eq!(c2.drawn_title().unwrap().value, "The shop");
        assert_eq!(c2.subtitle.as_ref().unwrap().value, "What runs");
        assert!(text(&src, c2.span).starts_with("container shop {"));
        assert!(text(&src, c2.span).ends_with('}'));
    }

    #[test]
    fn dropped_options_tries_and_seed_are_errors() {
        // SRS 4.1: dropped, routing is deterministic (CORE-25).
        for option in ["tries 5", "seed 42"] {
            let e = error(&in_workspace(
                SHOP,
                &format!("        container shop {{\n            {option}\n        }}"),
            ));
            let word = option.split(' ').next().unwrap();
            assert_eq!(
                e.kind,
                ModelErrorKind::UnknownConstruct {
                    construct: word.into()
                }
            );
            assert_eq!(e.position.line, 23);
        }
    }

    #[test]
    fn dsl_7_at_and_size_lines() {
        let src = in_workspace(
            SHOP,
            r#"        container shop "c2" {
            include *
            at user 220 170
            size web 280 140
            at web 620 -480
        }"#,
        );
        let m = model(&src);
        let v = m.view("c2").unwrap();
        let (user, web) = (m.find_written("user").unwrap(), m.find_written("web").unwrap());
        assert_eq!(v.positions.len(), 2);
        let at = v.position_of(web).unwrap();
        assert_eq!((at.x.value, at.y.value), (620, -480));
        assert_eq!(text(&src, at.span), "at web 620 -480");
        assert_eq!(text(&src, at.y.span), "-480");
        assert_eq!(text(&src, at.element.span), "web");
        assert_eq!(v.position_of(user).unwrap().x.value, 220);
        assert!(v.position_of(m.find_written("db").unwrap()).is_none());
        let size = v.size_of(web).unwrap();
        assert_eq!((size.width.value, size.height.value), (280, 140));
        assert_eq!(text(&src, size.span), "size web 280 140");
        assert!(v.size_of(user).is_none());

        for (line, kind) in [
            ("at user 1", "missing"),
            ("at user 1 2 3", "invalid"),
            ("at user x 2", "invalid"),
            ("size web 0 140", "invalid"),
            ("size web 280 -20", "invalid"),
        ] {
            let e = error(&in_workspace(
                SHOP,
                &format!(
                    "        container shop {{\n            include *\n            {line}\n        }}"
                ),
            ));
            match kind {
                "missing" => assert!(
                    matches!(e.kind, ModelErrorKind::MissingArgument { .. }),
                    "{line}: {e:?}"
                ),
                _ => assert!(
                    matches!(e.kind, ModelErrorKind::InvalidValue { .. }),
                    "{line}: {e:?}"
                ),
            }
        }
        // At most one of each per box.
        let e = error(&in_workspace(
            SHOP,
            "        container shop {\n            include *\n            at web 10 10\n            at shop.web 20 20\n        }",
        ));
        assert_eq!(
            e.kind,
            ModelErrorKind::DuplicateLine {
                construct: "at",
                id: "shop.web".into()
            }
        );
        assert_eq!(e.position.line, 25);
    }

    #[test]
    fn dotted_path_addresses_element() {
        // DSL-7.
        let m = model(&in_workspace(
            SHOP,
            r#"        component shop.web "c3" {
            include *
            at shop.web.api 100 200
            size shop.db 200 140
        }"#,
        ));
        let v = m.view("c3").unwrap();
        assert_eq!(v.scope.element(), m.find_written("web"));
        assert_eq!(v.positions[0].element.value, m.find_written("api").unwrap());
        assert_eq!(v.sizes[0].element.value, m.find_written("db").unwrap());
        let e = error(&in_workspace(
            SHOP,
            "        component shop.web {\n            include *\n            at web.shop 1 2\n        }",
        ));
        assert_eq!(
            e.kind,
            ModelErrorKind::UndeclaredId {
                id: "web.shop".into()
            }
        );
    }

    #[test]
    fn off_grid_at_is_accepted() {
        // S4: kept as written; lint flags it (CORE-18, L5).
        let m = model(&in_workspace(
            SHOP,
            "        container shop \"c2\" {\n            include *\n            at web 623 481\n            size web 250 135\n        }",
        ));
        let v = m.view("c2").unwrap();
        assert_eq!((v.positions[0].x.value, v.positions[0].y.value), (623, 481));
        assert_eq!(v.sizes[0].width.value, 250);
    }

    #[test]
    fn dsl_8_unplaced_elements_listed() {
        let m = model(&in_workspace(
            SHOP,
            r#"        container shop "c2" {
            include *
            at web 100 100
            at user 300 100
        }"#,
        ));
        let v = m.view("c2").unwrap();
        assert_eq!(keys(&m, &m.unplaced(v)), ["mail", "db", "job"]);
    }

    #[test]
    fn at_for_excluded_element_is_error() {
        // Review Focus.
        let src = in_workspace(
            SHOP,
            r#"        container shop "c2" {
            include *
            exclude mail
            at mail 100 100
        }"#,
        );
        let e = error(&src);
        assert_eq!(
            e.kind,
            ModelErrorKind::NotInView {
                id: "mail".into(),
                view: "c2".into()
            }
        );
        assert_eq!(text(&src, e.span), "at mail 100 100");
        let e = error(&in_workspace(
            SHOP,
            "        container shop \"c2\" {\n            size other 200 140\n        }",
        ));
        assert!(matches!(e.kind, ModelErrorKind::NotInView { .. }));
    }

    #[test]
    fn duplicate_view_key_is_error() {
        // Review Focus.
        let src = in_workspace(
            SHOP,
            r#"        systemContext shop "main" {
        }
        container shop "main" {
        }"#,
        );
        let e = error(&src);
        let ModelErrorKind::DuplicateViewKey {
            key,
            first,
            first_position,
        } = &e.kind
        else {
            panic!("{e:?}");
        };
        assert_eq!(key, "main");
        assert!(text(&src, *first).starts_with("systemContext shop"));
        assert_eq!(first_position.line, 22);
        assert_eq!(e.position.line, 24);
        assert!(e.to_string().contains("22:9"), "{e}");
        // A generated key never collides with a written one.
        let m = model(&in_workspace(
            SHOP,
            "        systemContext shop \"SystemContext-001\" {\n        }\n        systemContext shop {\n        }",
        ));
        assert_eq!(m.views[1].key, "SystemContext-002");
    }

    #[test]
    fn auto_layout_is_accepted_and_kept() {
        // DSL-6: span kept; boxes still auto-placed.
        let src = in_workspace(
            SHOP,
            r#"        container shop "c2" {
            include *
            autoLayout lr 300 100
        }
        container shop "plain" {
            include *
            autolayout
        }"#,
        );
        let m = model(&src);
        let v = m.view("c2").unwrap();
        assert_eq!(text(&src, v.auto_layout.unwrap()), "autoLayout lr 300 100");
        assert_eq!(m.unplaced(v).len(), m.included(v).len());
        assert!(m.view("plain").unwrap().auto_layout.is_some());
        let e = error(&in_workspace(
            SHOP,
            "        container shop {\n            autoLayout sideways\n        }",
        ));
        assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }));
    }

    // model-05: out-of-phase syntax
    #[test]
    fn dsl_4_unknown_construct_is_error() {
        let src = in_model("        user = person \"Operator\"\n        frobnicate user");
        let e = error(&src);
        assert_eq!(
            e.kind,
            ModelErrorKind::UnknownConstruct {
                construct: "frobnicate".into()
            }
        );
        assert_eq!(e.position, Position { line: 4, column: 9 });
        assert_eq!(text(&src, e.span), "frobnicate");
        let shown = e.to_string();
        assert!(shown.starts_with("4:9: "), "{shown}");
        assert!(shown.contains("`frobnicate`"), "{shown}");
        // Nothing is skipped silently, in any block.
        for src in [
            "!bogus\n".to_owned(),
            "something\n".to_owned(),
            "workspace {\n    branding {\n    }\n}\n".to_owned(),
            in_workspace(SHOP, "        branding {\n        }"),
            in_workspace(
                SHOP,
                "        container shop {\n            colorize on\n        }",
            ),
            in_workspace(
                SHOP,
                "        styles {\n            element \"X\" {\n                glow on\n            }\n        }",
            ),
            in_model(
                "        user = person \"Operator\" {\n            technology \"Rust\"\n        }",
            ),
            in_model(
                "        a = person \"A\"\n        b = person \"B\"\n        a -> b {\n            colour red\n        }",
            ),
        ] {
            let e = error(&src);
            assert!(
                matches!(
                    e.kind,
                    ModelErrorKind::UnknownConstruct { .. } | ModelErrorKind::LaterPhase { .. }
                ),
                "{src}: {e:?}"
            );
        }
        // Syntax errors from `text` come through as DSL-4 errors too.
        let e = error("workspace {\n    model {\n");
        assert!(matches!(
            e.kind,
            ModelErrorKind::Syntax(ParseErrorKind::UnclosedBrace)
        ));
        assert!(e.to_string().starts_with("2:11: "), "{e}");
    }

    /// Sources with a later-phase construct, the construct and its phase.
    fn later_phase_cases() -> Vec<(String, &'static str, u8)> {
        vec![
        ]
    }

    /// Later-phase cases inside `views`.
    fn later_phase_view_cases() -> Vec<(String, &'static str, u8)> {
        vec![
        ]
    }

    #[test]
    fn dsl_4_later_phase_names_the_phase() {
        let cases = later_phase_cases()
            .into_iter()
            .chain(later_phase_view_cases());
        for (src, construct, phase) in cases {
            let e = error(&src);
            assert_eq!(
                e.kind,
                ModelErrorKind::LaterPhase {
                    construct: construct.into(),
                    phase
                },
                "{src}"
            );
            let shown = e.to_string();
            assert!(shown.contains(&format!("phase {phase}")), "{shown}");
            let prefix = format!("{}:{}: ", e.position.line, e.position.column);
            assert!(shown.starts_with(&prefix), "{shown}");
        }
    }

    // model-06: golden input.
    // DECISION: the test stays in this module instead of moving to
    // crates/core/tests/golden_model.rs as the plan says: the module oracle
    // filters on `model::`, and the task limits edits to this file and
    // tests/data/.

    /// A file under the repository root, read at compile time.
    macro_rules! repo_file {
        ($path:literal) => {
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../", $path))
        };
    }

    /// Lines per view as the prototype draws them: relationship ends
    /// lifted to their nearest included ancestor, self-lines and `hide`
    /// pairs dropped, `focus` applied, one line per pair. Lifting is the
    /// `view` module's job (CORE-5); this restates the prototype rule to
    /// check the golden counts.
    fn prototype_line_count(m: &Model, v: &ViewSpec) -> usize {
        let included = m.included(v);
        let lift = |mut e: ElementId| loop {
            if included.contains(&e) {
                return Some(e);
            }
            e = m.element(e).parent?;
        };
        let hidden: BTreeSet<(ElementId, ElementId)> = v
            .hide
            .iter()
            .map(|h| (h.source.value, h.destination.value))
            .collect();
        let focus: BTreeSet<ElementId> = v.focus.iter().map(|f| f.value).collect();
        let mut pairs = BTreeSet::new();
        for r in &m.relationships {
            let (Some(s), Some(d)) = (lift(r.source.value), lift(r.destination.value)) else {
                continue;
            };
            if s == d || hidden.contains(&(s, d)) {
                continue;
            }
            if !focus.is_empty() && !focus.contains(&s) && !focus.contains(&d) {
                continue;
            }
            pairs.insert((s, d));
        }
        pairs.len()
    }

    #[test]
    fn dsl_12_example_parses_with_counts() {
        // Four views keyed `C1`, `C2`, `C3-serving`, `C3-startup`.
        let m = model(repo_file!("tests/data/example.dsl"));
        let keys: Vec<&str> = m.views.iter().map(|v| v.key.as_str()).collect();
        assert_eq!(keys, ["C1", "C2", "C3-serving", "C3-startup"]);
        assert_eq!(m.elements.len(), 21);
        assert_eq!(m.relationships.len(), 45);
        assert_eq!(m.element_styles.len(), 8);
        // Boxes and lines per view as printed by the prototype
        // (`c4.py example/workspace.dsl example/layout.json`).
        let expected = [
            ("C1", 5, 4, 1),
            ("C2", 9, 10, 6),
            ("C3-serving", 16, 21, 5),
            ("C3-startup", 11, 14, 0),
        ];
        for (key, boxes, lines, sizes) in expected {
            let v = m.view(key).unwrap();
            assert_eq!(m.included(v).len(), boxes, "{key} boxes");
            assert_eq!(prototype_line_count(&m, v), lines, "{key} lines");
            // Every box is placed by hand, as in layout.json.
            assert!(m.unplaced(v).is_empty(), "{key} unplaced");
            assert_eq!(v.positions.len(), boxes, "{key} at lines");
            assert_eq!(v.sizes.len(), sizes, "{key} size lines");
            assert!(v.subtitle.is_some(), "{key} subtitle");
        }
        let c1 = m.view("C1").unwrap().options();
        assert!(!c1.technology);
        let serving = m.view("C3-serving").unwrap();
        assert_eq!(
            serving.options(),
            ViewOptions {
                technology: true,
                bus: true,
                label_width: 16,
                max_crossings: 1,
                labels: LabelMode::Line,
                crossings: CrossingStyle::Gap,
                line_colours: LineColours::Plain,
            }
        );
        assert_eq!(serving.hide.len(), 2);
        assert_eq!(m.view("C3-startup").unwrap().focus.len(), 3);
        let app = m.find_written("app").unwrap();
        let at = m.view("C2").unwrap().position_of(app).unwrap();
        assert_eq!((at.x.value, at.y.value), (800, 540));
        let db = m.find_written("db").unwrap();
        assert_eq!(m.style_of(db).shape, Shape::Cylinder);
        assert!(m.element(m.find_written("vikunja").unwrap()).is_external());
    }

    // Oracle fixture test; the example's `relationship` style block is
    // accepted and kept (keyword table).
    #[test]
    fn dsl_14_repo_and_example_files_parse() {
        // docs/workspace.dsl and tests/data/example/workspace.dsl.
        for (name, source, views) in [
            ("docs/workspace.dsl", repo_file!("docs/workspace.dsl"), 5),
            (
                "tests/data/example/workspace.dsl",
                repo_file!("tests/data/example/workspace.dsl"),
                4,
            ),
        ] {
            let parsed = match parse(source) {
                Ok(parsed) => parsed,
                Err(e) => panic!("{name}:{e}"),
            };
            let m = &parsed.model;
            assert_eq!(m.views.len(), views, "{name}");
            for v in &m.views {
                // Every view has boxes to draw (all auto-placed here).
                assert!(!m.included(v).is_empty(), "{name} {}", v.key);
                assert!(v.auto_layout.is_some(), "{name} {}", v.key);
            }
            assert_eq!(parsed.trees[0].print(), source, "{name}");
        }
    }
}
