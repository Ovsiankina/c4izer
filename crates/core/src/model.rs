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
// element keyed `c` whose parent is keyed `b` inside `a` (`Model::find`);
// under `!identifiers hierarchical` an element's full id is its parent's
// full id, `.`, its own id (`Model::full_key`), and relationship ids stay
// global. A view with no key gets
// `<Kind>-NNN` (`SystemContext-001`), numbered per kind in declaration
// order. `relationship "<tag>" { ... }` blocks in `styles` are accepted
// and kept byte for byte in the tree, with no effect until P2-12 (keyword
// table, DSL-14).
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

mod archetype;
mod deploy;
mod expr;
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
        }
    }
}

/// One element. Serves DSL-1, DSL-15.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Element {
    /// Its own index in `Model::elements`.
    pub id: ElementId,
    /// The identifier written in the file (`app` in `app = container
    /// ...`), unique across the file (DSL-5). Always `Some` in phase 1:
    /// an element without `<id> =` is a `LaterPhase` error (DSL-7);
    /// phase 2 accepts it as `None`.
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
}

/// A box shape. Phase 2 adds the rest of Structurizr's shapes (P2-11).
/// Serves DSL-3, CORE-2.
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
        }
    }
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
    /// `Image` once landscape-filtered lands. Never `Dynamic` or
    /// `Deployment` in phase 2. A view whose kind is not drawable is
    /// listed in `pipeline::AllViews::unsupported`. Serves P2-2, P2-122,
    /// P2-123, P2-180.
    pub const fn drawable(self) -> bool {
        matches!(
            self,
            ViewKind::SystemContext | ViewKind::Container | ViewKind::Component
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
}

impl ViewOptions {
    /// The keyword-table defaults: technology on, bus off, label width
    /// 24, crossing budget 0. Serves DSL-6.
    pub const DEFAULT: ViewOptions = ViewOptions {
        technology: true,
        bus: false,
        label_width: 24,
        max_crossings: 0,
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
    /// `at` lines, in file order; at most one per element.
    pub positions: Vec<At>,
    /// `size` lines, in file order; at most one per element.
    pub sizes: Vec<Size>,
    /// The whole view statement, block included; `edit` adds a missing
    /// `at` line before its closing brace (DSL-9).
    pub span: Span,
    /// `properties { ... }` in the view body, in file order (P2-172).
    pub properties: Vec<Property>,
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
// TODO(srs): P2-7, a directive placed between relationships. Reading taken
// (B dsl-breadth 17, checked against Structurizr for Java): it applies to
// relationships declared after it. This field holds the last value
// written, and `view` and `include *` read it for the whole model: a
// per-relationship record needs a field on `Relationship` (data layout,
// docs/deferred.md), so a directive between relationships also applies to
// the ones above it.
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(Position { line, column }) = self.position {
            write!(f, "{line}:{column}: ")?;
        }
        write!(f, "{} is accepted and has no effect", self.subject)
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
        Ok(builder.model)
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

    /// The element named by `key`, ignoring ASCII case: an identifier, or
    /// a dotted path of identifiers from an ancestor down (`system.app`);
    /// under `!identifiers hierarchical`, the full id (`full_key`).
    /// Serves DSL-5, DSL-7, P2-6, P2-142, and `edit` re-finding an element
    /// in a newer text (EDIT-24).
    pub fn find(&self, key: &str) -> Option<ElementId> {
        if self.identifiers == Identifiers::Hierarchical {
            return self
                .elements
                .iter()
                .find(|e| {
                    self.full_key(e.id)
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
    /// `element`, in apply order: by the element's tag order, blocks for
    /// the same tag in file order; later entries override earlier ones per
    /// property. `edit` reads it to find the block a property came from
    /// (DSL-13). Serves DSL-15.
    // DECISION: plain `usize` indices; a style id newtype would add pub
    // surface for a list only `edit` and the style panel read.
    pub fn styles_for(&self, element: ElementId) -> Vec<usize> {
        let mut out = Vec::new();
        for tag in &self.element(element).tags {
            out.extend(
                self.element_styles
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.tag.value == *tag)
                    .map(|(i, _)| i),
            );
        }
        out
    }

    /// The style of `element`: `styles_for` applied per property over
    /// `ResolvedStyle::default_for(kind)`, so a property no block sets
    /// takes its kind's default (shape `Person` for a person, else `Box`).
    /// Serves DSL-3, DSL-15, CORE-2, CORE-9.
    pub fn style_of(&self, element: ElementId) -> ResolvedStyle {
        let mut style = ResolvedStyle::default_for(self.element(element).kind);
        for i in self.styles_for(element) {
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
        let mut set = BTreeSet::new();
        for selector in &view.include {
            match selector.value {
                Selector::All => self.include_all(view, &mut set),
                Selector::Element(id) => {
                    set.insert(id);
                }
            }
        }
        for selector in &view.exclude {
            match selector.value {
                Selector::All => set.clear(),
                Selector::Element(id) => {
                    set.remove(&id);
                }
            }
        }
        set.into_iter().collect()
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
        // P2-7: under `!impliedRelationships false` only a relationship of
        // a selected element itself, to the element shown, counts
        // (Structurizr for Java, checked).
        let direct_only = self.implied_relationships == ImpliedRelationships::False;
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

    /// The canonical edit key of `id`: its id under flat identifiers, its
    /// dotted full id under `!identifiers hierarchical` (groups add no
    /// segment, P2-131); `None` for an element declared without an id
    /// (P2-145). `Model::find(full_key)` gives `id` back. Serves P2-6,
    /// P2-142, P2-145.
    // TODO(srs): P2-145, how `at`, `size` and edits address an anonymous
    // box. Reading taken (B dsl-breadth 10): never; it is auto-placed on
    // every build and the editor refuses moves, resizes and per-box style
    // edits on it, never adding an identifier (EDIT-21).
    pub fn full_key(&self, id: ElementId) -> Option<String> {
        let element = self.element(id);
        let key = element.key.as_ref()?;
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
        let name = self.written(head.span(1));
        if head.kind(1) != Some(TokenKind::Word) || !name.chars().all(is_var_name_char) {
            return Err(self.invalid(head, head.text(0), 1));
        }
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
    /// it exists and lies inside that file's folder after `..` and
    /// symlinks are resolved (P2-134, P2-159). `None` when the file was
    /// parsed without a path.
    fn checked_path(&mut self, written: &Literal) -> Result<Option<PathBuf>, ModelError> {
        let Some(folder) = self.folder(written.span.file) else {
            return Ok(None);
        };
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
        let root = self.loader.canonicalize(&folder).unwrap_or(folder);
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
        self.no_block(statement, head)?;
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
        self.resolve_pending()
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
        if self.model.identifiers == Identifiers::Hierarchical {
            // DECISION: B dsl-breadth 11, checked against Structurizr for
            // Java: inside an element's block an id is looked up under
            // that element's full id first, then (a superset) under each
            // ancestor's, then as a full id.
            let mut scope = context;
            while let Some(c) = scope {
                if let Some(prefix) = self.model.full_key(c) {
                    let key = format!("{prefix}.{path}").to_ascii_lowercase();
                    if let Some(&id) = self.keys.get(&key) {
                        return Some(id);
                    }
                }
                scope = self.model.element(c).parent;
            }
            return self.keys.get(&path.to_ascii_lowercase()).copied();
        }
        find_path(&self.model.elements, path, |k| {
            self.keys.get(&k.to_ascii_lowercase()).copied()
        })
    }

    fn resolve_pending(&mut self) -> Result<(), ModelError> {
        for p in std::mem::take(&mut self.pending) {
            let Some(id) = self.resolve(&p.key, p.context) else {
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
            return Err(self.error(
                ModelErrorKind::UndeclaredId { id: written },
                head.span(kw + 1),
            ));
        };
        if kw == 2 {
            self.bind(head, id)?;
        }
        self.element_block(statement, id)?;
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
        };
        let mut names = vec![element.name.value.as_str()];
        let mut current = element.parent;
        while let Some(p) = current {
            names.push(&self.model.element(p).name.value);
            current = self.model.element(p).parent;
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
        match (self.model.identifiers, parent.and_then(|p| self.model.full_key(p))) {
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
            pending.extend(key.map(|key| Pending {
                relationship: index,
                source: true,
                key,
                span: end.span,
                context: parent,
            }));
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
        pending.extend(dkey.map(|key| Pending {
            relationship: index,
            source: false,
            key,
            span: destination.span,
            context: parent,
        }));
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
                // P2-178: kept byte for byte, no effect until styles-themes.
                if head.len() < 2 {
                    return Err(this.missing(&head, "theme"));
                }
                if head.is_keyword(0, "theme") && head.len() > 2 {
                    return Err(this.invalid(&head, "theme", 2));
                }
                this.no_block(inner, &head)?;
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
        if kind == ViewKind::Custom {
            view.title = strings.next();
        }
        view.description = strings.next();
        if kind == ViewKind::Filtered {
            self.no_block(statement, head)?;
        } else if statement.block.is_some() {
            self.view_body(statement, head, &mut view)?;
        }
        self.model.views.push(view);
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
        let mut seen_description = false;
        self.each(self.block(statement, head)?, &mut |this, inner| {
            if view.kind == ViewKind::Dynamic && inner.head.is_empty() {
                this.dynamic_parallel(inner)?;
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
                return Ok(());
            }
            match view.kind {
                ViewKind::Dynamic if this.dynamic_step(&head)? => {
                    this.no_block(inner, &head)?;
                    return Ok(());
                }
                ViewKind::Image if this.image_source(&head, &view.key)? => {
                    this.no_block(inner, &head)?;
                    return Ok(());
                }
                ViewKind::Deployment
                    if head.is_keyword(0, "include") || head.is_keyword(0, "exclude") =>
                {
                    this.deployment_selectors(&head)?;
                    this.no_block(inner, &head)?;
                    return Ok(());
                }
                _ => {}
            }
            this.view_line(&head, view, &mut seen_description)?;
            this.no_block(inner, &head)?;
            Ok(())
        })?;
        Ok(())
    }

    /// A `{ }` block of parallel steps in a dynamic view (P2-121).
    fn dynamic_parallel(&mut self, statement: &Statement) -> Result<(), ModelError> {
        let Some(block) = &statement.block else {
            return Ok(());
        };
        for inner in &block.statements {
            if inner.head.is_empty() {
                self.dynamic_parallel(inner)?;
                continue;
            }
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            if !self.dynamic_step(&head)? {
                return Err(self.reject(&head, 0));
            }
            self.no_block(inner, &head)?;
        }
        Ok(())
    }

    /// `[order:] <src> -> <dst> [desc] [tech]` or `[order:] <relationship
    /// id> [desc]`, ids checked (P2-121). False when `head` is no step.
    // DECISION: a word ending in `:` first is the order; a word that is no
    // view keyword starts a relationship-id step.
    fn dynamic_step(&self, head: &Head<'_>) -> Result<bool, ModelError> {
        let o = usize::from(head.kind(0) == Some(TokenKind::Word) && head.text(0).ends_with(':'));
        if head.len() <= o {
            return Err(self.missing(head, "step"));
        }
        if head.kind(o + 1) == Some(TokenKind::Arrow) {
            if head.len() < o + 3 {
                return Err(self.missing(head, "destination"));
            }
            self.view_element(head, o)?;
            self.view_element(head, o + 2)?;
            self.strings(head, "->", o + 3, 2)?;
            return Ok(true);
        }
        if o == 0 && DYNAMIC_LINES.iter().any(|k| head.is_keyword(0, k)) {
            return Ok(false);
        }
        if head.kind(o) != Some(TokenKind::Word) {
            return Err(self.invalid(head, "dynamic", o));
        }
        if !self
            .relationship_keys
            .contains_key(&head.text(o).to_ascii_lowercase())
        {
            return Err(self.error(
                ModelErrorKind::UndeclaredId {
                    id: head.text(o).into(),
                },
                head.span(o),
            ));
        }
        self.strings(head, "dynamic", o + 1, 1)?;
        Ok(true)
    }

    /// `plantuml|mermaid|kroki|image <source>` in an image view. False
    /// when `head` is no source line.
    fn image_source(&mut self, head: &Head<'_>, view: &str) -> Result<bool, ModelError> {
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
        // P2-3: an external-source image view is accepted with no effect;
        // `image <file>` is landscape-filtered's (P2-320).
        if !head.is_keyword(0, "image") {
            let subject = head.text(0).to_ascii_lowercase();
            self.notice(&subject, Some(view), head.span_of(0, head.len() - 1));
        }
        Ok(true)
    }

    /// `include`/`exclude` in a deployment view: `*`, element and
    /// deployment ids (checked, P2-179) and expressions (P2-178); nothing
    /// is stored until phase 3 draws deployment views.
    fn deployment_selectors(&self, head: &Head<'_>) -> Result<(), ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "element"));
        }
        for i in 1..head.len() {
            let t = head.text(i);
            if t == "*" || head.kind(i) != Some(TokenKind::Word) || is_expression(t) {
                continue;
            }
            let known = self.resolve(t, None).is_some()
                || self.other_ids.contains_key(&t.to_ascii_lowercase());
            if !known {
                return Err(self.error(ModelErrorKind::UndeclaredId { id: t.into() }, head.span(i)));
            }
        }
        Ok(())
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
        &self,
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
                if view.auto_layout.is_some() {
                    return Err(self.duplicate_line("autoLayout", head));
                }
                self.auto_layout(head)?;
                view.auto_layout = Some(head.span_of(0, head.len() - 1));
            }
            "title" => {
                once(view.title.is_some(), || self.duplicate_line("title", head))?;
                view.title = Some(self.one_string(head)?);
            }
            "description" => {
                once(*seen_description, || {
                    self.duplicate_line("description", head)
                })?;
                *seen_description = true;
                view.description = Some(self.one_string(head)?);
            }
            "subtitle" => {
                once(view.subtitle.is_some(), || {
                    self.duplicate_line("subtitle", head)
                })?;
                view.subtitle = Some(self.one_string(head)?);
            }
            "technology" => {
                once(view.technology.is_some(), || {
                    self.duplicate_line("technology", head)
                })?;
                view.technology = Some(self.on_off(head)?);
            }
            "bus" => {
                once(view.bus.is_some(), || self.duplicate_line("bus", head))?;
                view.bus = Some(self.on_off(head)?);
            }
            "labelwidth" => {
                once(view.label_width.is_some(), || {
                    self.duplicate_line("labelWidth", head)
                })?;
                view.label_width = Some(self.count(head, 1)?);
            }
            "maxcrossings" => {
                once(view.max_crossings.is_some(), || {
                    self.duplicate_line("maxCrossings", head)
                })?;
                view.max_crossings = Some(self.count(head, 0)?);
            }
            "focus" => {
                if head.len() < 2 {
                    return Err(self.missing(head, "element"));
                }
                for i in 1..head.len() {
                    view.focus.push(self.view_element(head, i)?);
                }
            }
            "hide" => view.hide.push(self.hide(head)?),
            // P2-178: no effect until landscape-filtered (P2-338).
            "default" => self.no_arguments(head)?,
            "at" => {
                let (element, x, y) = self.box_line(head, false)?;
                view.positions.push(At {
                    element,
                    x,
                    y,
                    span: head.span_of(0, head.len() - 1),
                });
            }
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

    /// The entries of an `include` or `exclude` line.
    fn selectors(&self, head: &Head<'_>) -> Result<Vec<Spanned<Selector>>, ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "element"));
        }
        let mut out = Vec::new();
        for i in 1..head.len() {
            let span = head.span(i);
            let t = head.text(i);
            let expression = head.kind(i) != Some(TokenKind::Word) || is_expression(t);
            if expression {
                // DECISION: P2-178: an expression or relationship pattern
                // is kept byte for byte and selects nothing until
                // landscape-filtered (P2-20) gives it an effect.
                continue;
            }
            let value = if t == "*" {
                Selector::All
            } else {
                Selector::Element(self.view_element(head, i)?.value)
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
        let element = self.view_element(head, 1)?;
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
            if head.is_keyword(0, "element") {
                this.element_style(inner, &head)?;
            } else if head.is_keyword(0, "light") || head.is_keyword(0, "dark") {
                // P2-178: kept byte for byte, no effect until styles-themes.
                this.no_arguments(&head)?;
                this.block(inner, &head)?;
            } else if head.is_keyword(0, "relationship") {
                // Keyword table: accepted and kept in the tree, no effect
                // until P2-12; its body is not read.
                this.one_string(&head)?;
                this.block(inner, &head)?;
            } else {
                return Err(this.reject(&head, 0));
            }
            Ok(())
        })?;
        Ok(())
    }

    fn element_style(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        let tag = self.one_string(head)?;
        let mut style = ElementStyle {
            tag,
            background: None,
            color: None,
            shape: None,
            span: statement.span,
        };
        for inner in self.block(statement, head)? {
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            if head.is_keyword(0, "properties") {
                self.properties(inner, &head)?;
                continue;
            }
            let known = ["background", "color", "shape"]
                .iter()
                .any(|k| head.is_keyword(0, k));
            if !known {
                // P2-178: every other Structurizr element style property is
                // accepted, kept byte for byte, with no effect until
                // styles-themes.
                if NO_EFFECT_STYLE.iter().any(|k| head.is_keyword(0, k)) {
                    self.no_block(inner, &head)?;
                    if head.len() < 2 {
                        return Err(self.missing(&head, "value"));
                    }
                    continue;
                }
                return Err(self.reject(&head, 0));
            }
            self.no_block(inner, &head)?;
            if head.is_keyword(0, "background") {
                let value = self.colour(&head, style.background.is_some(), "background")?;
                style.background = Some(value);
            } else if head.is_keyword(0, "color") {
                let value = self.colour(&head, style.color.is_some(), "color")?;
                style.color = Some(value);
            } else if head.is_keyword(0, "shape") {
                style.shape = self.shape(&head, style.shape.is_some())?;
            } else {
                return Err(self.reject(&head, 0));
            }
        }
        self.model.element_styles.push(style);
        Ok(())
    }

    /// The single value of a style property line, as text and token index.
    // DECISION: a style value may be bare (`#0b3c6f`) or quoted
    // (`"#0b3c6f"`), as Structurizr accepts both.
    fn style_value(
        &self,
        head: &Head<'_>,
        seen: bool,
        construct: &'static str,
    ) -> Result<String, ModelError> {
        if seen {
            return Err(self.duplicate_line(construct, head));
        }
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
        seen: bool,
        construct: &'static str,
    ) -> Result<Spanned<Colour>, ModelError> {
        let value = self.style_value(head, seen, construct)?;
        let Some(colour) = parse_colour(&value) else {
            return Err(self.invalid(head, construct, 1));
        };
        Ok(Spanned {
            value: colour,
            span: head.span(1),
        })
    }

    fn shape(&self, head: &Head<'_>, seen: bool) -> Result<Option<Spanned<Shape>>, ModelError> {
        let value = self.style_value(head, seen, "shape")?;
        let lower = value.to_ascii_lowercase();
        let shape = match lower.as_str() {
            "box" => Shape::Box,
            "person" => Shape::Person,
            "cylinder" => Shape::Cylinder,
            _ if LATER_SHAPES.contains(&lower.as_str()) => {
                // P2-178: accepted with no effect until styles-themes
                // (P2-11); the kind's default shape stays.
                return Ok(None);
            }
            _ => return Err(self.invalid(head, "shape", 1)),
        };
        Ok(Some(Spanned {
            value: shape,
            span: head.span(1),
        }))
    }
}

/// An `at` or `size` line: the element and its two numbers.
type BoxLine = (Spanned<ElementId>, Spanned<i32>, Spanned<i32>);

/// `Err(error())` when a single-valued line was already `seen`.
fn once(seen: bool, error: impl FnOnce() -> ModelError) -> Result<(), ModelError> {
    if seen { Err(error()) } else { Ok(()) }
}

/// Element style properties accepted with no effect until styles-themes
/// (P2-178), lowercase-insensitive.
const NO_EFFECT_STYLE: &[&str] = &[
    "icon",
    "width",
    "height",
    "stroke",
    "strokeWidth",
    "fontSize",
    "border",
    "opacity",
    "metadata",
    "description",
    "colour",
];

/// Structurizr shapes planned for phase 2 (P2-11), lowercase.
const LATER_SHAPES: &[&str] = &[
    "roundedbox",
    "circle",
    "ellipse",
    "hexagon",
    "diamond",
    "bucket",
    "pipe",
    "robot",
    "folder",
    "webbrowser",
    "window",
    "terminal",
    "shell",
    "mobiledeviceportrait",
    "mobiledevicelandscape",
    "component",
];

/// The view lines a dynamic view takes besides its steps.
const DYNAMIC_LINES: &[&str] = &[
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
];

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
        positions: Vec::new(),
        sizes: Vec::new(),
        span,
        properties: Vec::new(),
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

/// `#rrggbb`, hex digits of either case.
fn parse_colour(text: &str) -> Option<Colour> {
    let hex = text.strip_prefix('#')?;
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
        let user = m.element(m.find("user").unwrap());
        assert_eq!(user.kind, ElementKind::Person);
        assert_eq!(user.parent, None);
        assert_eq!(user.name.value, "Operator");
        assert_eq!(text(&src, user.name.span), "\"Operator\"");
        assert_eq!(user.description.as_ref().unwrap().value, "Runs the thing.");
        assert_eq!(user.technology, None);
        assert_eq!(user.tags, ["Element", "Person", "Staff", "Night"]);
        assert_eq!(user.key.as_ref().unwrap().value, "user");
        assert!(text(&src, user.span).starts_with("user = person"));

        let system = m.element(m.find("system").unwrap());
        assert_eq!(system.kind, ElementKind::SoftwareSystem);
        assert_eq!(system.description.as_ref().unwrap().value, "Sells things.");
        assert_eq!(system.tags, ["Element", "Software System", "Core"]);
        assert!(text(&src, system.span).ends_with('}'));

        let app = m.element(m.find("app").unwrap());
        assert_eq!(app.kind, ElementKind::Container);
        assert_eq!(app.parent, Some(system.id));
        assert_eq!(app.description.as_ref().unwrap().value, "Serves pages.");
        // The later `technology` line wins over the positional value.
        assert_eq!(app.technology.as_ref().unwrap().value, "Rust, axum");
        assert_eq!(app.tags, ["Element", "Container", "Web"]);

        let api = m.element(m.find("api").unwrap());
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
        assert_eq!(m.find("system.app"), Some(app.id));
        assert_eq!(m.find("system.app.api"), Some(api.id));
        assert_eq!(m.find("app.system"), None);
        assert_eq!(m.find("nothing"), None);
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
            m.element(m.find("model").unwrap()).name.value,
            "Diagram file"
        );
        assert_eq!(m.find("system.views"), Some(ElementId(2)));
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
        assert_eq!(m.find("USER"), Some(ElementId(0)));
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
            m.find("user").unwrap(),
            m.find("app").unwrap(),
            m.find("db").unwrap(),
            m.find("system").unwrap(),
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
        assert_eq!(r.source.value, m.find("user").unwrap());
        assert_eq!(r.destination.value, m.find("shop").unwrap());
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
        let (user, mail) = (m.find("user").unwrap(), m.find("mail").unwrap());
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
        let (a, b) = (m.find("a").unwrap(), m.find("b").unwrap());
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

        let user = m.style_of(m.find("user").unwrap());
        assert_eq!(user.shape, Shape::Person);
        assert_eq!(user.background, C(0x0b, 0x3c, 0x6f));
        assert_eq!(user.color, C(255, 255, 255));
        let db = m.style_of(m.find("db").unwrap());
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
            ("color red", "red"),
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
        let tags = |k: &str| m.element(m.find(k).unwrap()).tags.clone();
        assert_eq!(tags("user"), ["Element", "Person"]);
        assert_eq!(tags("shop"), ["Element", "Software System"]);
        assert_eq!(tags("app"), ["Element", "Container"]);
        assert_eq!(tags("api"), ["Element", "Component"]);
        assert_eq!(m.relationships[0].tags, ["Relationship"]);

        let user = m.style_of(m.find("user").unwrap());
        assert_eq!(user.color, C(1, 2, 3));
        assert_eq!(user.background, C(0xdd, 0xdd, 0xdd));
        assert_eq!(user.shape, Shape::Person);
        assert_eq!(m.style_of(m.find("api").unwrap()).shape, Shape::Box);
        assert_eq!(
            ResolvedStyle::default_for(ElementKind::Person),
            ResolvedStyle {
                background: C(0xdd, 0xdd, 0xdd),
                color: C(0, 0, 0),
                shape: Shape::Person
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
    fn relationship_style_block_is_kept_without_effect() {
        // Keyword table: `relationship "<tag>" { ... }` in `styles` is
        // accepted, kept byte for byte, no effect until P2-12 (DSL-14).
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
        assert_eq!(v.scope.element(), m.find("shop"));
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
                max_crossings: 2
            }
        );
        let focus: Vec<ElementId> = v.focus.iter().map(|f| f.value).collect();
        assert_eq!(keys(&m, &focus), ["web", "db", "job"]);
        assert_eq!(v.hide.len(), 2);
        assert_eq!(v.hide[0].source.value, m.find("user").unwrap());
        assert_eq!(v.hide[0].destination.value, m.find("web").unwrap());
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
        let e = error(&in_workspace(
            SHOP,
            "        container shop {\n            bus on\n            bus off\n        }",
        ));
        assert_eq!(
            e.kind,
            ModelErrorKind::DuplicateLine {
                construct: "bus",
                id: String::new()
            }
        );
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
        let (user, web) = (m.find("user").unwrap(), m.find("web").unwrap());
        assert_eq!(v.positions.len(), 2);
        let at = v.position_of(web).unwrap();
        assert_eq!((at.x.value, at.y.value), (620, -480));
        assert_eq!(text(&src, at.span), "at web 620 -480");
        assert_eq!(text(&src, at.y.span), "-480");
        assert_eq!(text(&src, at.element.span), "web");
        assert_eq!(v.position_of(user).unwrap().x.value, 220);
        assert!(v.position_of(m.find("db").unwrap()).is_none());
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
        assert_eq!(v.scope.element(), m.find("web"));
        assert_eq!(v.positions[0].element.value, m.find("api").unwrap());
        assert_eq!(v.sizes[0].element.value, m.find("db").unwrap());
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
                max_crossings: 1
            }
        );
        assert_eq!(serving.hide.len(), 2);
        assert_eq!(m.view("C3-startup").unwrap().focus.len(), 3);
        let app = m.find("app").unwrap();
        let at = m.view("C2").unwrap().position_of(app).unwrap();
        assert_eq!((at.x.value, at.y.value), (800, 540));
        let db = m.find("db").unwrap();
        assert_eq!(m.style_of(db).shape, Shape::Cylinder);
        assert!(m.element(m.find("vikunja").unwrap()).is_external());
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
