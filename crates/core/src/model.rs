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
// DECISION: keywords match case-insensitively (SRS 4.1); ids, tags and
// view keys match exactly as written.
// DECISION: when a value is given both positionally and by a body line
// (`description`, `technology`), the later one in the file wins and is
// the literal kept, so `edit` rewrites the bytes that take effect.
// DECISION: colours are `#rrggbb` only, hex digits of either case (SRS
// 4.1 keyword table); `#rgb` and colour names are `InvalidValue` until
// P2-10.
//
// Phase-1 identifier rules (DSL-7): every element has an `<id> =` and no
// relationship has one; an element without an id and a named
// relationship are `LaterPhase` errors (phase 2). `Element::key` and
// `Relationship::key` stay `Option` so phase 2 fills them without an API
// change. A dotted `<id>` `a.b.c` names the element keyed `c` whose
// parent is keyed `b` inside `a` (`Model::find`). A view with no key gets
// `<Kind>-NNN` (`SystemContext-001`), numbered per kind in declaration
// order. `relationship "<tag>" { ... }` blocks in `styles` are accepted
// and kept byte for byte in the tree, with no effect until P2-12 (keyword
// table, DSL-14).
//
// TODO(srs): DSL-15 cascade order is to be confirmed against the
// Structurizr reference (SRS 4.1 open holes); drafted as written in
// DSL-15 (tag order, later tags win, per property).

use std::fmt;

use crate::text::{ParseError, ParseErrorKind, Position, Span, SyntaxTree};

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
}

impl ElementKind {
    /// The DSL keyword (`person`, `softwareSystem`, ...). Serves DSL-1,
    /// DSL-4 (error text).
    pub const fn keyword(self) -> &'static str {
        todo!()
    }

    /// The implicit kind tag (`Person`, `Software System`, `Container`,
    /// `Component`); every element also gets `Element` before it.
    /// Serves DSL-15.
    pub const fn default_tag(self) -> &'static str {
        todo!()
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
}

impl Element {
    /// True when the element carries the `External` tag (CORE-2: drawn as
    /// `External Software System`). Serves DSL-1.
    pub fn is_external(&self) -> bool {
        todo!()
    }

    /// True when `tags` contains `tag`, implicit tags included. Serves
    /// DSL-15.
    pub fn has_tag(&self, tag: &str) -> bool {
        todo!()
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
        todo!()
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
        todo!()
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
}

impl ViewKind {
    /// The DSL keyword (`systemContext`, `container`, `component`).
    /// Serves DSL-4 (error text), DSL-6.
    pub const fn keyword(self) -> &'static str {
        todo!()
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
}

impl ViewScope {
    /// The scope element, `None` for a view with no single scope (none in
    /// phase 1). Serves DSL-6.
    pub const fn element(&self) -> Option<ElementId> {
        todo!()
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
}

impl ViewSpec {
    /// The options with defaults filled in. Serves DSL-6.
    pub const fn options(&self) -> ViewOptions {
        todo!()
    }

    /// The drawn title: the `title` line, else the description; `None`
    /// is lint L8. Serves DSL-6.
    pub const fn drawn_title(&self) -> Option<&Literal> {
        todo!()
    }

    /// The `at` line for `element`, if any. Serves DSL-7, DSL-9.
    pub fn position_of(&self, element: ElementId) -> Option<&At> {
        todo!()
    }

    /// The `size` line for `element`, if any. Serves DSL-7.
    pub fn size_of(&self, element: ElementId) -> Option<&Size> {
        todo!()
    }
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
        todo!()
    }

    /// The element with this id. Panics on an id from another model.
    /// Serves DSL-1.
    pub fn element(&self, id: ElementId) -> &Element {
        todo!()
    }

    /// The relationship with this id. Panics on an id from another model.
    /// Serves DSL-2.
    pub fn relationship(&self, id: RelationshipId) -> &Relationship {
        todo!()
    }

    /// The element named by `key`: an identifier, or a dotted path of
    /// identifiers from an ancestor down (`system.app`). Serves DSL-5,
    /// DSL-7, and `edit` re-finding an element in a newer text (EDIT-24).
    pub fn find(&self, key: &str) -> Option<ElementId> {
        todo!()
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
        todo!()
    }

    /// Direct children of `parent`, in source order. Serves DSL-1.
    // DECISION: returns a Vec, not `impl Iterator`, so the signature does
    // not pin an iterator type; element counts are small.
    pub fn children(&self, parent: ElementId) -> Vec<ElementId> {
        todo!()
    }

    /// The view whose effective key is `key`. Serves DSL-6.
    pub fn view(&self, key: &str) -> Option<&ViewSpec> {
        todo!()
    }

    /// The indices into `element_styles` of every block that applies to
    /// `element`, in apply order: by the element's tag order, blocks for
    /// the same tag in file order; later entries override earlier ones per
    /// property. `edit` reads it to find the block a property came from
    /// (DSL-13). Serves DSL-15.
    // DECISION: plain `usize` indices; a style id newtype would add pub
    // surface for a list only `edit` and the style panel read.
    pub fn styles_for(&self, element: ElementId) -> Vec<usize> {
        todo!()
    }

    /// The style of `element`: `styles_for` applied per property over
    /// `ResolvedStyle::default_for(kind)`, so a property no block sets
    /// takes its kind's default (shape `Person` for a person, else `Box`).
    /// Serves DSL-3, DSL-15, CORE-2, CORE-9.
    pub fn style_of(&self, element: ElementId) -> ResolvedStyle {
        todo!()
    }

    /// The elements `view` includes after `include` and `exclude`
    /// (`*` expands as the keyword table says for the view kind and
    /// scope), sorted by id. Lifting, `focus` and `hide` are `view`'s job
    /// (CORE-5). Serves DSL-6, DSL-7, DSL-8.
    pub fn included(&self, view: &ViewSpec) -> Vec<ElementId> {
        todo!()
    }

    /// The elements `view` includes that have no `at` line, sorted by
    /// id: these are auto-placed (CORE-22). Serves DSL-8.
    pub fn unplaced(&self, view: &ViewSpec) -> Vec<ElementId> {
        todo!()
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
    todo!()
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl std::error::Error for ModelError {}

impl From<ParseError> for ModelError {
    /// Wraps a `text` error as `ModelErrorKind::Syntax`, same span and
    /// position. Serves DSL-4.
    fn from(error: ParseError) -> ModelError {
        todo!()
    }
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

    // model-01: elements and nesting
    #[test]
    fn dsl_1_elements_with_parents() {
        // Positional values and `description`/`technology`/`tags` lines.
        todo!()
    }

    #[test]
    fn dsl_5_undeclared_id_is_error() {
        todo!()
    }

    #[test]
    fn duplicate_id_is_error() {
        // Review Focus, S12: the error carries both spans.
        todo!()
    }

    #[test]
    fn element_without_id_is_error() {
        // DSL-7: an element without `<id> =` is a DSL-4 error naming
        // phase 2.
        todo!()
    }

    #[test]
    fn keyword_named_id_parses() {
        // docs/workspace.dsl: `model = container ...`; an id may equal a
        // keyword.
        todo!()
    }

    #[test]
    fn keywords_are_case_insensitive() {
        // SRS 4.1.
        todo!()
    }

    // model-02: relationships
    #[test]
    fn dsl_2_relationships_top_level_and_nested() {
        todo!()
    }

    #[test]
    fn forward_reference_allowed() {
        // Review Focus: id used before it is declared.
        todo!()
    }

    #[test]
    fn this_and_implied_source_in_element_block() {
        // DSL-2: `this -> x` and `-> x "desc"` inside an element block.
        todo!()
    }

    #[test]
    fn find_relationship_by_ends_and_ordinal() {
        // S6, EDIT-24: two `a -> b` lines are found in source order.
        todo!()
    }

    #[test]
    fn named_relationship_is_error_in_phase_1() {
        // DSL-7: `r = a -> b` is a DSL-4 error naming phase 2.
        todo!()
    }

    // model-03: styles
    #[test]
    fn dsl_3_element_styles() {
        todo!()
    }

    #[test]
    fn style_resolution_last_tag_wins() {
        todo!()
    }

    #[test]
    fn dsl_15_implicit_tags_and_per_property_cascade() {
        // `Element` + kind tag, `Relationship`; unset property takes the
        // keyword-table default.
        todo!()
    }

    #[test]
    fn styles_for_lists_blocks_in_apply_order() {
        // S5, DSL-13.
        todo!()
    }

    #[test]
    fn relationship_style_block_is_kept_without_effect() {
        // Keyword table: `relationship "<tag>" { ... }` in `styles` is
        // accepted, kept byte for byte, no effect until P2-12 (DSL-14).
        todo!()
    }

    #[test]
    fn styles_outside_views_is_error() {
        // DSL-3: `styles` sits inside `views`.
        todo!()
    }

    // model-04: views
    #[test]
    fn dsl_6_view_options() {
        // technology, bus, labelWidth, maxCrossings, focus, hide a -> b.
        todo!()
    }

    #[test]
    fn view_key_and_description_optional_title_wins() {
        // DSL-6: positional string is the description; `title` wins;
        // missing key is generated.
        todo!()
    }

    #[test]
    fn dropped_options_tries_and_seed_are_errors() {
        // SRS 4.1: dropped, routing is deterministic (CORE-25).
        todo!()
    }

    #[test]
    fn dsl_7_at_and_size_lines() {
        todo!()
    }

    #[test]
    fn dotted_path_addresses_element() {
        // DSL-7.
        todo!()
    }

    #[test]
    fn off_grid_at_is_accepted() {
        // S4: kept as written; lint flags it (CORE-18, L5).
        todo!()
    }

    #[test]
    fn dsl_8_unplaced_elements_listed() {
        todo!()
    }

    #[test]
    fn at_for_excluded_element_is_error() {
        // Review Focus.
        todo!()
    }

    #[test]
    fn duplicate_view_key_is_error() {
        // Review Focus.
        todo!()
    }

    #[test]
    fn auto_layout_is_accepted_and_kept() {
        // DSL-6: span kept; boxes still auto-placed.
        todo!()
    }

    // model-05: out-of-phase syntax
    #[test]
    fn dsl_4_unknown_construct_is_error() {
        todo!()
    }

    #[test]
    fn dsl_4_later_phase_names_the_phase() {
        todo!()
    }

    // model-06: golden input. The plan places this test in
    // crates/core/tests/golden_model.rs; it moves there at model-06.
    #[test]
    fn dsl_12_example_parses_with_counts() {
        // Four views keyed `C1`, `C2`, `C3-serving`, `C3-startup`.
        todo!()
    }

    // Oracle fixture test; the example's `relationship` style block is
    // accepted and kept (keyword table).
    #[test]
    fn dsl_14_repo_and_example_files_parse() {
        // docs/workspace.dsl and tests/data/example/workspace.dsl.
        todo!()
    }
}
