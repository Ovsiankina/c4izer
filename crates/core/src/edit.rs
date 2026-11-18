//! Semantic edits (move box, resize, set text, set style) turned into
//! byte patches on the original text, touching nothing else (DSL-9, 10,
//! 13). Re-applies an edit to a newer text by id (EDIT-24).
//!
//! Depends on: `text`, `model`.

// Shape of the API: an `Edit` is a semantic operation that names its
// target by the keys written in the file (element key, view key,
// relationship ends + ordinal), never by a `Span` or an index, so the
// same value applies to a newer text (EDIT-24) and serves as an undo step
// (EDIT-19: "semantic operations with their previous value, not byte
// patches"). `apply` turns a list of edits into a new text plus the
// inverse list; the editor keeps both, nothing here does I/O.
//
// Element key: every `element: String` in this module is the canonical
// element key, `model::Model::full_key`: the identifier written before
// `=` under flat ids, the dotted full id under `!identifiers hierarchical`
// (P2-142), the same string `model::Model::find` resolves.
//
// Files (P2-139, P2-140): each step plans on one file and re-parses
// through `model::reparse`, which reads no file: a text edit goes to the
// file holding its target's statement, an `at`/`size` line to the file
// holding the view block, a per-box `tags` line to the file declaring the
// element, style blocks and default views to the root file.
// `Applied::files` lists the other files whose bytes changed. The editor gets it from a hit-tested `ElementId` as
// `model.element(id).key`; for a relationship it uses
// `RelationshipRef::of`. Nothing else is a valid key.
//
// EDIT-21 (product rule, SRS section 2: "it is not draw.io"): there is no
// operation that creates or deletes an element or a relationship, and
// none may be added. The complete list of bytes `apply` may insert or
// remove, which the implementation's guard is built from:
// - `at` and `size` lines in a view;
// - the c4rizer view extension lines `paperSize` and `line` (P2-701,
//   P2-708..P2-710, P2-733);
// - `title` and `subtitle` lines in a view;
// - a default view block (EDIT-15, `Edit::AddDefaultView`): a view, not
//   an element, appended as the last view in `views`, and removed again
//   by `Edit::RemoveDefaultView` (inverse only, EDIT-19) while it holds
//   nothing but `include *`, `at` and `size` lines;
// - the `views` block itself, when the file has none (a model-only file,
//   valid Structurizr) and an edit needs one: created as the last block
//   of the workspace body, indented per DSL-9 (EDIT-15, DSL-13); removed
//   by `Edit::RemoveDefaultView` when the view was all it held;
// - string literals of existing statements (DSL-10), including `""`
//   placeholders for skipped positional arguments;
// - the order of two neighbouring step lines of a dynamic view, each
//   moved whole (`Edit::SwapSteps`, P3-220): bytes move, none are added
//   or removed;
// - style property lines (`background`, `color`, `shape`) in an
//   `element "<tag>"` block;
// - `element "<tag>"` style blocks and the `styles` block, which lives
//   inside `views` as Structurizr has it and, when missing, is created as
//   the last block of `views` (the `views` block created first if
//   needed, DSL-13);
// - a per-box tag (DSL-13) on an existing element: the tag in a `tags`
//   line, the `tags` line itself, and the element's `{ }` block when the
//   element had none.
// Removal exists only so an inverse gives the bytes back (EDIT-19), and
// a block (`{ }` of an element, `element "<tag>"`, `styles`) or a `tags`
// line is removed only when it holds nothing but what `edit` wrote there;
// the same holds for a `views` block that `edit` created: the inverse
// removes it only when it holds nothing but what `edit` wrote;
// otherwise only the tag or property goes. Removing an `at` or `size`
// line sends the box back to auto-placement or its kind's size; the
// element stays.
//
// EDIT-26 (first write in a view writes an `at` line for every box) is
// the editor's job: it builds one batch of `Edit::Move`s for every box at
// its current position, ordered by `ElementId` so the new lines come out
// in a deterministic order; `apply` writes them in batch order as DSL-9
// says, one step. A recorded single `Move` re-applied (EDIT-24) to a text
// whose view lost every `at` line is no longer a first write: the editor
// must detect that and rebuild the EDIT-26 batch, not re-apply the lone
// edit.
//
// DECISION: positions and sizes are `i32` pixels, as the `at`/`size`
// lines and `model::At`/`model::Size` hold them, not `geom::Point` grid
// units: `edit` depends on `text` and `model` only (C4 box in
// docs/workspace.dsl). Grid snapping (EDIT-2, DSL-7 multiples of 20) and
// the minimum size that fits the wrapped text (EDIT-18, CORE-30) are the
// editor's checks before it builds the edit; off-grid values are written
// as given and flagged by lint L5, as `model::Size` documents.
// DECISION: a batch is atomic: if any edit's target is gone or its value
// is refused, nothing is written and the error names that edit's index.
// EDIT-19 skips the whole step whose target no longer exists; a
// multi-box operation is one step, so it is skipped whole.
// DECISION: the missing `at` line goes after the view's last `at` line,
// else before the view's closing brace, as DSL-9 says; the plan's "after
// its `include` lines" is superseded by the SRS.
// DECISION: no hot struct here: edits are a few per user action, so no
// `#[repr(C)]` and no `size_of` pin.
// DECISION: `\r`, like `\n`, is refused as `Newline` (text.rs treats a
// lone `\r` as whitespace, so it would survive in a literal and break
// CRLF files); any other control character except tab is refused as
// `InvalidValue`.
// DECISION: `SetText` carries an optional `expected` current value, so a
// target that drifted (a relationship with the same ends inserted
// earlier shifts the ordinal; a keyless view inserted earlier shifts a
// generated key such as `Container-001`) is reported as `Conflict`
// instead of patched (EDIT-24: never patch the wrong span). Inverses
// always fill it when the target had a value.
//
// DSL-9: a new `at` line goes after the view's last `at` line, else
// before the closing brace, copying the indentation and line ending of
// the line above; when the line above is the view head it gets the
// view's body indentation (the first body line's, else the view's plus
// one level of the file's indent unit, four spaces by default), and a
// one-line view `{ ... }` is first split onto lines.
// DSL-9: a new `size` line is placed the same way after the view's last
// `size` line, else after its last `at` line (else before the closing
// brace).
// DSL-10: a missing view `title` or `subtitle` line is inserted as the
// first line of the view body, indented per DSL-9.
// DSL-13: the "this box only" tag is the element's full dotted id
// (`a.b.c`), suffixed `-2`, `-3`... when another element already carries
// that tag; its `tags` line is the last line of the element's block,
// creating the block if needed.
// EDIT-15: the default view (`include *`, key `<scope id>-<kind>`) is
// written by `edit` on the first write made in it, as
// `Edit::AddDefaultView`, the first edit of that write's batch. In a
// model-only file that edit also creates the `views` block.
// DECISION: the `<kind>` in a default view key is `ViewKind::keyword`
// (`api-container`, `api-component`, `web-systemContext`): the DSL
// spelling of the kind, unambiguous and stable across releases.
// DECISION: "holds nothing but what `edit` wrote" is read from the text,
// since an inverse carries no history: a block (`element "<tag>"`,
// `styles`, `views`, an element's `{ }`) is removed only when, after the
// removal, it would hold no statement and no comment.
// DECISION: `ResetStyle` removes every `background`/`color`/`shape` line
// of the per-box `element "<tag>"` block and the block itself unless it
// holds a comment (then the comment stays), then the per-box tag. The
// inverse of a first "this box only" change is a `SetStyle ThisBox`
// with `None`, which removes only that property (EDIT-21).
// DECISION: splitting a one-line block (DSL-9) puts each statement and
// comment on its own line one unit below the owner; only the blank space
// between them is rewritten.

use std::fmt;
use std::ops::Range;

use crate::model::{
    self, Colour, ElementId, ElementKind, ElementStyle, PaperSize, Parsed, RelationshipId, Routing, Shape, ViewKind, ViewSpec,
};
use crate::text::{self, Block, FileId, Span, Statement, SyntaxTree, TokenKind};

/// A relationship named across parses by its ends and its position among
/// the relationships with the same ends, as `model::Model::find_relationship`
/// defines identity (EDIT-24, EDIT-19). Serves DSL-10.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelationshipRef {
    /// Source element key: the canonical element key (the bare
    /// `model::Element::key` in phase 1).
    pub source: String,
    /// Destination element key, canonical like `source`.
    pub destination: String,
    /// Index among the `source -> destination` relationships, in source
    /// order, from 0.
    pub ordinal: u32,
}

impl RelationshipRef {
    /// The `ordinal`-th relationship from `source` to `destination`.
    /// Serves DSL-10, EDIT-24.
    pub fn new(source: impl Into<String>, destination: impl Into<String>, ordinal: u32) -> Self {
        RelationshipRef {
            source: source.into(),
            destination: destination.into(),
            ordinal,
        }
    }

    /// The key-based reference of relationship `id` in `model`: its ends'
    /// canonical element keys and its index in
    /// `model.find_relationship(source, destination)`. The editor calls it
    /// on a hit-tested line so it never re-implements identity. Panics on
    /// an id from another model, like `model::Model::relationship`.
    /// Serves DSL-10, EDIT-19, EDIT-24.
    pub fn of(model: &model::Model, id: RelationshipId) -> RelationshipRef {
        let r = model.relationship(id);
        let (source, destination) = (r.source.value, r.destination.value);
        let index = model
            .find_relationship(source, destination)
            .iter()
            .position(|&other| other == id)
            .unwrap_or_default();
        RelationshipRef {
            source: element_key(model, source),
            destination: element_key(model, destination),
            ordinal: u32::try_from(index).unwrap_or(u32::MAX),
        }
    }
}

/// The string fields of an element that DSL-10 rewrites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ElementField {
    /// The name literal; never absent.
    Name,
    /// The description, positional or from a `description` line.
    Description,
    /// The technology, positional or from a `technology` line. Containers
    /// and components only: on a person or software system any edit of
    /// it is `EditErrorKind::InvalidValue`.
    Technology,
}

/// The string fields of a relationship that DSL-10 rewrites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RelationshipField {
    /// The description literal.
    Description,
    /// The technology literal.
    Technology,
}

/// The string fields of a view that DSL-10 rewrites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ViewField {
    /// The `title` line.
    Title,
    /// The `subtitle` line (c4rizer extension).
    Subtitle,
}

/// Which string literal a text edit rewrites, named by keys so it can be
/// re-applied to a newer text (EDIT-24). Serves DSL-10, EDIT-5.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TextTarget {
    /// A field of the element with this key.
    Element {
        /// The canonical element key (the bare `model::Element::key` in
        /// phase 1).
        element: String,
        /// Which field.
        field: ElementField,
    },
    /// A field of a relationship.
    Relationship {
        /// The relationship, from `RelationshipRef::of`.
        relationship: RelationshipRef,
        /// Which field.
        field: RelationshipField,
    },
    /// A field of the view with this effective key.
    View {
        /// View key (`model::ViewSpec::key`).
        view: String,
        /// Which field.
        field: ViewField,
    },
    /// The description literal of a dynamic view's step: the step's own
    /// text, not the relationship's (P3-218). Writes only that step's
    /// line.
    // APPROVER: new pub item
    Step {
        /// View key (`model::ViewSpec::key`).
        view: String,
        /// Index into `model::ViewSpec::steps` (file order).
        step: u32,
    },
}

/// Where a style change is written (DSL-13, EDIT-17).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StyleTarget {
    /// `all <tag>`: edit or add the property in that tag's `element`
    /// block, adding the block at the end of `styles` when there is none;
    /// a missing `styles` block is created as the last block of `views`,
    /// and a missing `views` block as the last block of the workspace
    /// body, indented per DSL-9 (DSL-13).
    /// The change hits every element carrying the tag; that is the
    /// explicit meaning of `all`. The element must carry `tag`, else
    /// `EditErrorKind::InvalidValue`. When a later tag of the element sets
    /// the same property, the edit is written but has no visible effect
    /// on this element (DSL-15 cascade); the editor offers the tags
    /// accordingly.
    Tag(String),
    /// `this box only`: a tag named after the element's full dotted id,
    /// suffixed `-2`, `-3`... when another element already carries that
    /// tag, added to the element (a `tags` line as the last line of its
    /// block, the block created if needed), and an `element "<tag>"` block
    /// at the end of `styles`, `styles` and `views` created as for `Tag`
    /// when missing (DSL-13).
    ThisBox,
}

/// One style property and its new value. `None` removes the property
/// line from the block, so the cascade falls through to earlier tags or
/// the default (DSL-15); the `element "<tag>"` block and the `styles`
/// block are removed with it only when they then hold nothing but what
/// `edit` wrote there (EDIT-21). `None` exists for inverses (EDIT-19).
/// Serves DSL-13.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StyleProperty {
    /// `background #rrggbb`, written lowercase (`Colour`'s `Display`).
    Background(Option<Colour>),
    /// `color #rrggbb` (text colour).
    Color(Option<Colour>),
    /// `shape Person|Cylinder|Box`.
    Shape(Option<Shape>),
}

/// One semantic edit. Targets are named by the keys written in the file,
/// so an edit applies to any later text where they still resolve
/// (EDIT-24) and doubles as an undo step (EDIT-19).
///
/// There is deliberately no variant that creates or deletes an element
/// or a relationship (EDIT-21); only layout (`at`, `size`), text and
/// style change, and a default view is added (EDIT-15) and, by an
/// inverse only, removed again (EDIT-19).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Edit {
    /// Move a box: rewrite its `at` line in `view`, or add one after the
    /// view's last `at` line (before the closing brace when it has none),
    /// copying indentation and line ending of the line above (DSL-9,
    /// DSL-7, EDIT-3). `None` removes the `at` line, so the box is
    /// auto-placed again (CORE-22); used by inverses (EDIT-19).
    Move {
        /// View key (`model::ViewSpec::key`).
        view: String,
        /// The canonical element key (the bare `model::Element::key` in
        /// phase 1).
        element: String,
        /// New centre in pixels, in the order `(x, y)`, or `None` for no
        /// `at` line. Written as given; grid snapping is the editor's job
        /// (EDIT-2).
        centre: Option<(i32, i32)>,
    },
    /// Resize a box: rewrite or add only its `size` line in `view` (DSL-7,
    /// EDIT-18); a new line goes after the view's last `size` line, else
    /// after its last `at` line, indented per DSL-9. `None` removes the `size` line, so the box takes its
    /// kind's size (CORE-2); used by inverses (EDIT-19).
    Resize {
        /// View key (`model::ViewSpec::key`).
        view: String,
        /// The canonical element key (the bare `model::Element::key` in
        /// phase 1).
        element: String,
        /// New size in pixels, in the order `(width, height)`, each
        /// `> 0`, or `None` for no `size` line. Off-grid values (DSL-7:
        /// multiples of 20) are written as given and flagged by lint L5,
        /// as `model::Size` does; snapping is the editor's job.
        size: Option<(i32, i32)>,
    },
    /// Rewrite one string literal with `"` and `\` escaped; `\n` and `\r`
    /// are refused (`Newline`), other control characters except tab are
    /// refused (`InvalidValue`); an absent literal is inserted, with `""`
    /// placeholders for skipped positional arguments (DSL-10, EDIT-5).
    ///
    /// `None` makes the field absent, for inverses (EDIT-19): it removes
    /// the literal only when it is the trailing positional argument or a
    /// body line (`description`, `technology`, `title`, `subtitle`); on a
    /// positional with later positionals after it (the description of
    /// `container "n" "d" "t"`) it writes `""`, so the later arguments
    /// keep their slots. `None` is refused for an element name.
    SetText {
        /// The literal to rewrite.
        target: TextTarget,
        /// The new value, or `None` for absent.
        value: Option<String>,
        /// The value the target must hold now, unescaped, or `None` to
        /// skip the check. A mismatch is `EditErrorKind::Conflict` and
        /// nothing is written (EDIT-24, EDIT-19 "with their previous
        /// value").
        expected: Option<String>,
    },
    /// Change one style property of an element, written where `target`
    /// says (DSL-13, EDIT-17). A multi-box change is one batch, one undo
    /// step (EDIT-17, EDIT-19).
    SetStyle {
        /// The canonical element key (the bare `model::Element::key` in
        /// phase 1).
        element: String,
        /// `all <tag>` or `this box only`.
        target: StyleTarget,
        /// The property and its value.
        property: StyleProperty,
    },
    /// "Reset to tag style": remove the element's per-box tag and its
    /// `element "<tag>"` block, if present (DSL-13). The block, the `tags`
    /// line, the element's `{ }` block and the `styles` block are each
    /// removed only when they hold nothing but what `edit` wrote there;
    /// otherwise only the tag, or the per-box properties, go (EDIT-21).
    ResetStyle {
        /// The canonical element key (the bare `model::Element::key` in
        /// phase 1).
        element: String,
    },
    /// Write the default view of `scope` (EDIT-15): a `kind` view with key
    /// `<scope id>-<kind>` (`<kind>` is `ViewKind::keyword`), appended as
    /// the last view in `views` (before a `styles` block that follows the
    /// views), holding `include *` and no `at` lines. When the file has
    /// no `views` block (model only), it first creates one as the last
    /// block of the workspace body, indented per DSL-9. It adds a view,
    /// not an element (EDIT-21). The editor puts it first in the batch of
    /// the first write made in a default view (move, resize, style), so
    /// that write is one step and its `Move`/`Resize` lines land in the
    /// new view; its inverse, `RemoveDefaultView`, removes the view
    /// block, and the `views` block too when it holds nothing but what
    /// `edit` wrote there.
    AddDefaultView {
        /// The scope element key (the bare `model::Element::key` in phase
        /// 1).
        scope: String,
        /// `ViewKind::Container`: container view of a software system;
        /// `ViewKind::Component`: component view of a container;
        /// `ViewKind::SystemContext`: system context view of a software
        /// system (`systemContext <scope> "<scope id>-systemContext" {
        /// include * }`), as shown in a model-only file (EDIT-15). A
        /// scope whose element kind does not fit `kind` (a container
        /// scope for `Container` or `SystemContext`, a system scope for
        /// `Component`, a person) is `EditErrorKind::InvalidValue`.
        kind: ViewKind,
    },
    /// Remove the default view of `scope` (EDIT-15, EDIT-19): the `kind`
    /// view with key `<scope id>-<kind>` that `AddDefaultView` wrote.
    /// Inverse only: it is the inverse of `AddDefaultView`, and the
    /// editor never builds it from a user gesture (EDIT-21: no gesture
    /// deletes anything from the file).
    ///
    /// The view block is removed only while it holds nothing but what
    /// `edit` wrote there: one `include *` line plus `at` and `size`
    /// lines (and the `paperSize` and `line` lines of P2-701 and
    /// P2-708..P2-710), a head of keyword, scope and key only, and no
    /// comment.
    /// Otherwise it is refused (`EditErrorKind::InvalidValue`) and nothing
    /// is written. The `views` block goes with it under the same rule:
    /// only when the view was all it held, as when `AddDefaultView`
    /// created it. The inverse adds the view back with its `at` and
    /// `size` lines.
    RemoveDefaultView {
        /// The scope element key (the bare `model::Element::key` in phase
        /// 1).
        scope: String,
        /// The view kind, as in `AddDefaultView`; a view with the key
        /// `<scope id>-<kind>` of another kind or scope is
        /// `EditErrorKind::InvalidValue`, no view with that key is
        /// `EditErrorKind::TargetGone`.
        kind: ViewKind,
    },
    /// Set the canvas size of `view`: rewrite or add its `paperSize`
    /// line, or remove it with `None` (auto). Serves P2-701.
    SetCanvasSize {
        /// View key (`model::ViewSpec::key`).
        view: String,
        /// The size, `None` for auto (no line).
        size: Option<PaperSize>,
    },
    /// Set the label position of `relationship`'s line in `view`, in the
    /// view's `line` line for it (added, rewritten, or removed when it
    /// would hold nothing). Serves P2-708.
    SetLinePosition {
        /// View key.
        view: String,
        /// The relationship.
        relationship: RelationshipRef,
        /// 0..100, `None` to drop the setting.
        position: Option<u8>,
    },
    /// Set the routing of `relationship`'s line in `view`, as
    /// `SetLinePosition`. Serves P2-709.
    SetLineRouting {
        /// View key.
        view: String,
        /// The relationship.
        relationship: RelationshipRef,
        /// The routing, `None` to drop the setting.
        routing: Option<Routing>,
    },
    /// Set jumps on `relationship`'s line in `view`, as
    /// `SetLinePosition`. Serves P2-710.
    SetLineJump {
        /// View key.
        view: String,
        /// The relationship.
        relationship: RelationshipRef,
        /// On or off, `None` to drop the setting.
        jump: Option<bool>,
    },
    /// Swap step `first` of dynamic view `view` with the step after it
    /// in file order: the two step lines trade places, each keeping its
    /// bytes, and the indentation of each line's position stays. Refused
    /// (`InvalidValue`) when either step has a written order or the two
    /// sit in different parallel blocks (P3-221). Its own inverse
    /// (EDIT-19). Refines the approved `edit::swap_lines` helper. Serves
    /// P3-220.
    // APPROVER: new pub item
    SwapSteps {
        /// View key (`model::ViewSpec::key`).
        view: String,
        /// Index into `model::ViewSpec::steps` of the upper step.
        first: u32,
    },
}

/// The result of `apply`: the new text and the edits that undo it.
/// Serves DSL-9, DSL-10, DSL-13, EDIT-19.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Applied {
    /// The patched root file (`FileId::ROOT`); every byte outside the
    /// edited targets is the input's (DSL-9, DSL-10, DSL-13). Phase 2
    /// (P2-5) sends a text edit to the file its literal came from; that
    /// will be a new per-file field, and this one keeps meaning the root
    /// file.
    pub text: String,
    /// The inverse edits, in the order to apply them (EDIT-19). Applied
    /// to `text` with nothing changed in between, they give the input
    /// back byte for byte wherever the edit rewrote in place or inserted
    /// at the canonical spot; otherwise (a removed line that sat between
    /// others, a `ResetStyle` whose block and tag are re-added at the
    /// end of `styles` and as a new `tags` line, an external change in
    /// between) they give the same model, not the same bytes. An inverse
    /// never removes a block (`{ }` of an element, `element "<tag>"`,
    /// `styles`) or a `tags` line that holds anything besides what the
    /// edit wrote there, nor a `views` block holding anything besides
    /// what edits wrote; it then removes only the tag or the property
    /// (EDIT-21). The inverse of a text insertion that added `""`
    /// placeholders also takes those placeholders out.
    pub inverse: Vec<Edit>,
    /// The patched files other than the root, by `FileId`, ascending: a
    /// text edit goes to the file its literal came from (P2-139), and an
    /// `at`/`size` line to the file holding the view block (P2-140).
    /// Empty when only the root changed.
    // TODO(srs): P2-140, `at`/`size` for a view declared in an included
    // file. Reading taken (B dsl-breadth 7): the file holding the view's
    // block. P2-139, a DSL-13 per-box `tags` line for an element declared
    // in an included file: the file holding its declaration (B 8). P2-16,
    // a text edit to a literal from a `workspace extends` base: the base
    // file (B 9).
    pub files: Vec<(FileId, String)>,
}

/// Applies `edits` in order to the root file of `parsed`, finding each
/// target by key, and returns the patched text and the inverse edits.
/// The same call re-applies a user action to a newer text after an
/// external change (EDIT-24): the caller parses the new file and passes
/// it. An empty list returns the source unchanged (DSL-11). When an edit
/// needs a `views` block (a default view, a style) and the file has none,
/// `apply` creates it as the last block of the workspace body, indented
/// per DSL-9, and a missing `styles` block as the last block of `views`
/// (EDIT-15, DSL-13). Serves DSL-9, DSL-10, DSL-13, EDIT-15, EDIT-19,
/// EDIT-24.
///
/// # Errors
///
/// Nothing is written when any edit fails (the batch is one step):
/// a target element, relationship or view no longer resolves, or the
/// element is not in the view (`EditErrorKind::TargetGone`, EDIT-24); a
/// `SetText` target holds a value other than its `expected`
/// (`EditErrorKind::Conflict`, EDIT-24); a text value holds `\n` or `\r`
/// (`EditErrorKind::Newline`, DSL-10); a value is refused
/// (`EditErrorKind::InvalidValue`: size not `> 0`, an absent element
/// name, a technology on a person or software system, another control
/// character in a text value, `StyleTarget::Tag` with a tag the element
/// does not carry, an `AddDefaultView` scope whose kind does not fit, an
/// `AddDefaultView` or `RemoveDefaultView` whose key `<scope>-<kind>`
/// names a view of another scope or kind, a `RemoveDefaultView` whose
/// view holds more than `edit` wrote; the same view already written is
/// not an error for `AddDefaultView`).
pub fn apply(parsed: &Parsed, edits: &[Edit]) -> Result<Applied, EditError> {
    // DECISION: each edit is applied to the text left by the previous one,
    // re-parsed in between, so every edit finds its target by key in the
    // text it patches; edits are a few per user action (EDIT-26 batches a
    // view's boxes), so re-parsing is cheap next to routing.
    let mut current = parsed.clone();
    let mut steps = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let inverse = apply_one(&mut current, edit).map_err(|kind| EditError { kind, edit: index })?;
        steps.push(inverse);
    }
    let text = match current.trees.first() {
        Some(tree) => tree.source().to_owned(),
        None => String::new(),
    };
    // P2-139, P2-140: every other file whose bytes changed, by `FileId`.
    let files = current
        .trees
        .iter()
        .zip(&parsed.trees)
        .skip(1)
        .filter(|(now, before)| now.source() != before.source())
        .map(|(now, _)| (now.file(), now.source().to_owned()))
        .collect();
    Ok(Applied {
        text,
        inverse: steps.into_iter().rev().flatten().collect(),
        files,
    })
}

/// Why an edit could not be applied (DSL-10, EDIT-24). `Display` says
/// what to change (SRS 5.1).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EditErrorKind {
    /// The element, relationship or view the edit names does not exist in
    /// this text, or the element is not in the view: the action is
    /// dropped and the status bar says so (EDIT-24, EDIT-19).
    TargetGone {
        /// The key that did not resolve, as the edit names it.
        target: String,
    },
    /// The target resolves but no longer holds the value the edit
    /// expected, so it is probably a different literal now (EDIT-24).
    Conflict {
        /// The value the edit expected.
        expected: String,
        /// The value found, `None` when the field is absent.
        found: Option<String>,
    },
    /// A text value contains `\n` or `\r` (DSL-10).
    Newline,
    /// The literal's written bytes hold a `${NAME}` substitution
    /// (P2-150): rewriting it would drop the reference, so the edit is
    /// refused and no file changes.
    // TODO(srs): P2-151, a DSL-10 edit of a literal holding `${...}`.
    // Reading taken (B dsl-breadth 3): refused, naming the constant.
    Substituted {
        /// The constant or variable name.
        name: String,
    },
    /// A value the file cannot hold, shaped like
    /// `model::ModelErrorKind::InvalidValue` so `Display` can quote it.
    InvalidValue {
        /// The refused value, as the edit gives it.
        value: String,
        /// Why it was refused, in words.
        reason: &'static str,
    },
}

/// An edit error: what went wrong and which edit of the batch caused it.
/// Serves EDIT-24, DSL-10.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EditError {
    /// What went wrong.
    pub kind: EditErrorKind,
    /// Index into the `edits` slice passed to `apply`.
    pub edit: usize,
}

impl fmt::Display for EditError {
    /// One sentence for the status bar: what was dropped and why (EDIT-24,
    /// SRS 5.1).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            EditErrorKind::TargetGone { target } => write!(
                f,
                "change dropped: `{target}` is no longer in the file or not in this view"
            ),
            EditErrorKind::Conflict {
                expected,
                found: Some(found),
            } => write!(
                f,
                "change dropped: expected \"{expected}\" but the file now holds \"{found}\"; edit it again"
            ),
            EditErrorKind::Conflict {
                expected,
                found: None,
            } => write!(
                f,
                "change dropped: expected \"{expected}\" but the file now holds no value; edit it again"
            ),
            EditErrorKind::Newline => {
                write!(f, "text not written: remove the line break, a string holds one line")
            }
            EditErrorKind::InvalidValue { value, reason } => {
                write!(f, "change not written: `{value}`: {reason}")
            }
            EditErrorKind::Substituted { name } => write!(
                f,
                "change not written: this text uses `${{{name}}}` — change the `!const`/`!var` `{name}` instead"
            ),
        }
    }
}

impl std::error::Error for EditError {}

// ------------------------------------------------------------ the driver

type Res<T> = Result<T, EditErrorKind>;

/// One byte replacement on the current root text.
struct Patch {
    range: Range<usize>,
    text: String,
}

impl Patch {
    fn insert(at: usize, text: String) -> Patch {
        Patch { range: at..at, text }
    }

    fn remove(range: Range<usize>) -> Patch {
        Patch {
            range,
            text: String::new(),
        }
    }
}

/// What one step does to the current text: the patches, or first a
/// one-line block split onto lines (DSL-9), after which the step is
/// planned again on the re-parsed text.
enum Plan {
    Patches(Vec<Patch>),
    Split(Patch),
}

/// Applies one edit to `current` and returns its inverse edits, in the
/// order to apply them.
fn apply_one(current: &mut Parsed, edit: &Edit) -> Res<Vec<Edit>> {
    match edit {
        Edit::Move {
            view,
            element,
            centre,
        } => box_line(current, view, element, *centre, LineKind::At),
        Edit::Resize {
            view,
            element,
            size,
        } => box_line(current, view, element, *size, LineKind::Size),
        Edit::SetText {
            target,
            value,
            expected,
        } => set_text(current, target, value.as_deref(), expected.as_deref()),
        Edit::SetStyle {
            element,
            target,
            property,
        } => set_style(current, element, target, *property),
        Edit::ResetStyle { element } => reset_style(current, element),
        Edit::AddDefaultView { scope, kind } => add_default_view(current, scope, *kind),
        Edit::RemoveDefaultView { scope, kind } => remove_default_view(current, scope, *kind),
        Edit::SetCanvasSize { view, size } => set_canvas_size(current, view, *size),
        Edit::SetLinePosition {
            view,
            relationship,
            position,
        } => set_line(current, view, relationship, LineSetting::Position(*position)),
        Edit::SetLineRouting {
            view,
            relationship,
            routing,
        } => set_line(current, view, relationship, LineSetting::Routing(*routing)),
        Edit::SetLineJump {
            view,
            relationship,
            jump,
        } => set_line(current, view, relationship, LineSetting::Jump(*jump)),
        Edit::SwapSteps { view, first } => swap_steps(current, view, *first),
    }
}

/// `Edit::SwapSteps`: swaps the lines of steps `first` and `first + 1`.
// DECISION: each step's statement bytes move whole; what lies outside
// the two statements (indentation, line endings, comments on their own
// lines) stays where it is, so each line keeps the indentation of its
// place (P3-220).
fn swap_steps(current: &mut Parsed, view: &str, first: u32) -> Res<Vec<Edit>> {
    let model = &current.model;
    let refused = |reason| EditErrorKind::InvalidValue {
        value: format!("{view} step {first}"),
        reason,
    };
    let upper = step_of(model, view, first)?;
    let lower = step_of(model, view, first.saturating_add(1))?;
    if upper.written_order.is_some() || lower.written_order.is_some() {
        return Err(refused("a step with a written order is never renumbered by a move"));
    }
    if upper.parallel != lower.parallel {
        return Err(refused("a step cannot move across a parallel block's border"));
    }
    let file = model.view(view).ok_or_else(|| gone(view))?.span.file;
    let (a, b) = (upper.span, lower.span);
    run_in(current, file, |cx| {
        let (Some(x), Some(y)) = (cx.leaf_holding(a), cx.leaf_holding(b)) else {
            return Err(refused("its statement was not found"));
        };
        let text = |st: &Statement| cx.src[st.span.range()].to_owned();
        Ok(Plan::Patches(vec![
            Patch {
                range: x.span.range(),
                text: text(y),
            },
            Patch {
                range: y.span.range(),
                text: text(x),
            },
        ]))
    })?;
    Ok(vec![Edit::SwapSteps {
        view: view.to_owned(),
        first,
    }])
}

/// Step `step` of dynamic view `view`.
fn step_of<'m>(model: &'m model::Model, view: &str, step: u32) -> Res<&'m model::DynamicStep> {
    model
        .view(view)
        .ok_or_else(|| gone(view))?
        .steps
        .get(step as usize)
        .ok_or_else(|| gone(&format!("{view} step {step}")))
}

/// Applies `patches` to the text of `file` and re-parses `current`
/// through `model::reparse`, which reads no file (P2-139).
fn commit(current: &mut Parsed, file: FileId, mut patches: Vec<Patch>) -> Res<()> {
    if patches.is_empty() {
        return Ok(());
    }
    patches.sort_by_key(|p| std::cmp::Reverse(p.range.start));
    let mut text = tree_of(current, file).source().to_owned();
    for patch in patches {
        text.replace_range(patch.range, &patch.text);
    }
    *current = model::reparse(current, file, &text).map_err(|e| EditErrorKind::InvalidValue {
        value: e.to_string(),
        reason: "the changed file would not parse",
    })?;
    Ok(())
}

/// Plans one step on the root file of `current` and commits it.
fn run(current: &mut Parsed, plan: impl Fn(&Ctx<'_>) -> Res<Plan>) -> Res<()> {
    run_in(current, FileId::ROOT, plan)
}

/// Plans one step on file `file` of `current` and commits it, splitting
/// one-line blocks first when the plan asks for it.
fn run_in(current: &mut Parsed, file: FileId, plan: impl Fn(&Ctx<'_>) -> Res<Plan>) -> Res<()> {
    // DECISION: a step splits at most a few blocks (each split puts that
    // block's braces on their own lines for good); the bound only guards
    // against a planning bug looping forever.
    for _ in 0..8 {
        let planned = plan(&Ctx::in_file(current, file))?;
        match planned {
            Plan::Patches(patches) => return commit(current, file, patches),
            Plan::Split(patch) => commit(current, file, vec![patch])?,
        }
    }
    Err(EditErrorKind::InvalidValue {
        value: String::new(),
        reason: "the block could not be split onto lines",
    })
}

fn root(parsed: &Parsed) -> &SyntaxTree {
    &parsed.trees[FileId::ROOT.0 as usize]
}

/// The tree of `file`, the root when there is no such file.
fn tree_of(parsed: &Parsed, file: FileId) -> &SyntaxTree {
    parsed.trees.get(file.0 as usize).unwrap_or_else(|| root(parsed))
}

// ------------------------------------------------------- text and lines

/// Index of the first byte of the line holding `pos`.
fn line_start(src: &str, pos: usize) -> usize {
    src[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// Index of the line ending (`\r\n` or `\n`) of the line holding `pos`,
/// or the end of the source.
fn line_end(src: &str, pos: usize) -> usize {
    let end = src[pos..].find('\n').map_or(src.len(), |i| pos + i);
    if end < src.len() && end > pos && src.as_bytes()[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

/// Leading spaces and tabs of the line holding `pos`.
fn indent_at(src: &str, pos: usize) -> &str {
    let start = line_start(src, pos);
    let rest = &src[start..];
    let len = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    &rest[..len]
}

/// True when only spaces and tabs come before `pos` on its line.
fn starts_line(src: &str, pos: usize) -> bool {
    src[line_start(src, pos)..pos]
        .chars()
        .all(|c| c == ' ' || c == '\t')
}

/// The line ending that ends the line before the one starting at
/// `line_start` (which must be > 0).
fn eol_before(src: &str, line_start: usize) -> &'static str {
    if line_start >= 2 && src.as_bytes()[line_start - 2] == b'\r' {
        "\r\n"
    } else {
        "\n"
    }
}

/// The file's line ending: that of its first line, `\n` by default.
fn file_eol(src: &str) -> &'static str {
    match src.find('\n') {
        Some(i) if i > 0 && src.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// The file's indent unit (DSL-9): the indentation of its first indented
/// line, four spaces when no line is indented.
// DECISION: the first indented line is one level deep in any file that
// nests `model` in `workspace`, so its indentation is the unit.
fn indent_unit(src: &str) -> String {
    for line in src.lines() {
        let rest = line.trim_start_matches([' ', '\t']);
        if !rest.trim().is_empty() && rest.len() < line.len() {
            return line[..line.len() - rest.len()].to_owned();
        }
    }
    "    ".to_owned()
}

/// Lines to insert, each with its depth below the insertion indent.
type Lines = Vec<(usize, String)>;

fn one_line(text: String) -> Lines {
    vec![(0, text)]
}

/// The tree view the planners read: source, tokens, model.
struct Ctx<'a> {
    tree: &'a SyntaxTree,
    src: &'a str,
    model: &'a model::Model,
    unit: String,
    eol: &'static str,
}

impl<'a> Ctx<'a> {
    /// The planners' view of file `file` (P2-139, P2-140).
    fn in_file(parsed: &'a Parsed, file: FileId) -> Ctx<'a> {
        let tree = tree_of(parsed, file);
        let src = tree.source();
        Ctx {
            tree,
            src,
            model: &parsed.model,
            unit: indent_unit(src),
            eol: file_eol(src),
        }
    }

    fn token(&self, index: usize) -> text::Token {
        self.tree.tokens()[index]
    }

    /// The statement whose span is `span`, at any depth.
    fn statement(&self, span: Span) -> Option<&'a Statement> {
        fn find(statements: &[Statement], span: Span) -> Option<&Statement> {
            for s in statements {
                if s.span == span {
                    return Some(s);
                }
                if s.span.start <= span.start
                    && span.end <= s.span.end
                    && let Some(block) = &s.block
                {
                    return find(&block.statements, span);
                }
            }
            None
        }
        find(self.tree.statements(), span)
    }

    /// The statement of `block` whose span holds `span`.
    fn child_holding(block: &'a Block, span: Span) -> Option<&'a Statement> {
        block
            .statements
            .iter()
            .find(|s| s.span.start <= span.start && span.end <= s.span.end)
    }

    /// The innermost statement holding `span`: a step line inside a view
    /// or a parallel `{ }` block (P3-218, P3-220).
    fn leaf_holding(&self, span: Span) -> Option<&'a Statement> {
        fn find(statements: &[Statement], span: Span) -> Option<&Statement> {
            let s = statements
                .iter()
                .find(|s| s.span.start <= span.start && span.end <= s.span.end)?;
            match &s.block {
                Some(block) => find(&block.statements, span).or(Some(s)),
                None => Some(s),
            }
        }
        find(self.tree.statements(), span)
    }

    /// Indices of the head's non-trivia tokens.
    fn head(&self, statement: &Statement) -> Vec<usize> {
        statement
            .head
            .clone()
            .filter(|&i| !self.token(i).kind.is_trivia())
            .collect()
    }

    /// The first head word of `statement`, if it is a word.
    fn keyword(&self, statement: &Statement) -> Option<&'a str> {
        let first = *self.head(statement).first()?;
        (self.token(first).kind == TokenKind::Word).then(|| self.tree.token_text(first))
    }

    /// The top-level `workspace` statement.
    fn workspace(&self) -> Option<&'a Statement> {
        self.tree.statements().iter().find(|s| {
            self.keyword(s)
                .is_some_and(|k| k.eq_ignore_ascii_case("workspace"))
                && s.block.is_some()
        })
    }

    /// True when `block` holds `statement` and nothing else but blank
    /// space: no other statement, no comment.
    fn holds_only(&self, block: &Block, statement: &Statement) -> bool {
        block.statements.len() == 1
            && block.statements[0].span == statement.span
            && (block.open + 1..block.close).all(|i| {
                let t = self.token(i);
                (t.start >= statement.span.start && t.end <= statement.span.end)
                    || matches!(t.kind, TokenKind::Whitespace | TokenKind::Newline)
            })
    }

    /// The indentation of `owner`'s body lines: the first body statement
    /// that starts its own line, else `owner`'s indentation plus one unit
    /// (DSL-9).
    fn body_indent(&self, owner: &Statement, block: &Block) -> String {
        match block
            .statements
            .iter()
            .find(|s| starts_line(self.src, s.span.start as usize))
        {
            Some(s) => indent_at(self.src, s.span.start as usize).to_owned(),
            None => format!("{}{}", indent_at(self.src, owner.span.start as usize), self.unit),
        }
    }

    fn render(&self, indent: &str, lines: &Lines, eol: &str, eol_first: bool) -> String {
        let mut out = String::new();
        for (depth, line) in lines {
            if eol_first {
                out.push_str(eol);
            }
            out.push_str(indent);
            for _ in 0..*depth {
                out.push_str(&self.unit);
            }
            out.push_str(line);
            if !eol_first {
                out.push_str(eol);
            }
        }
        out
    }

    /// Inserts `lines` after the line of `statement`, a statement of
    /// `owner`'s `block`, with its indentation and line ending (DSL-9).
    fn insert_after(
        &self,
        owner: &Statement,
        block: &Block,
        statement: &Statement,
        lines: &Lines,
    ) -> Plan {
        let tokens = self.tree.tokens();
        let mut i = tokens.partition_point(|t| t.start < statement.span.end);
        while let Some(t) = tokens.get(i) {
            match t.kind {
                TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment => i += 1,
                TokenKind::Newline => {
                    let indent = if starts_line(self.src, statement.span.start as usize) {
                        indent_at(self.src, statement.span.start as usize).to_owned()
                    } else {
                        self.body_indent(owner, block)
                    };
                    let eol = &self.src[t.range()];
                    let text = self.render(&indent, lines, eol, true);
                    return Plan::Patches(vec![Patch::insert(t.start as usize, text)]);
                }
                _ => return Plan::Split(self.split(owner, block)),
            }
        }
        Plan::Split(self.split(owner, block))
    }

    /// Inserts `lines` as the last lines of `owner`'s `block`, before its
    /// closing brace (DSL-9).
    fn insert_last(&self, owner: &Statement, block: &Block, lines: &Lines) -> Plan {
        let close = self.token(block.close).start as usize;
        let start = line_start(self.src, close);
        if start == 0 || !starts_line(self.src, close) {
            return Plan::Split(self.split(owner, block));
        }
        let eol = eol_before(self.src, start);
        let above = line_start(self.src, start - 1);
        let open = self.token(block.open).start as usize;
        let above_text = &self.src[above..start];
        let indent = if (above..start).contains(&open) {
            self.body_indent(owner, block)
        } else if above_text.trim().is_empty() {
            match block.statements.last() {
                Some(s) if starts_line(self.src, s.span.start as usize) => {
                    indent_at(self.src, s.span.start as usize).to_owned()
                }
                _ => self.body_indent(owner, block),
            }
        } else {
            indent_at(self.src, above).to_owned()
        };
        Plan::Patches(vec![Patch::insert(
            start,
            self.render(&indent, lines, eol, false),
        )])
    }

    /// Inserts `lines` as the first lines of `owner`'s `block` (DSL-10).
    fn insert_first(&self, owner: &Statement, block: &Block, lines: &Lines) -> Plan {
        let tokens = self.tree.tokens();
        let mut i = block.open + 1;
        while let Some(t) = tokens.get(i) {
            match t.kind {
                TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment => i += 1,
                TokenKind::Newline => {
                    let eol = &self.src[t.range()];
                    let indent = self.body_indent(owner, block);
                    let text = self.render(&indent, lines, eol, false);
                    return Plan::Patches(vec![Patch::insert(t.end as usize, text)]);
                }
                _ => break,
            }
        }
        Plan::Split(self.split(owner, block))
    }

    /// Inserts `lines` before the line of `statement`, with its
    /// indentation.
    fn insert_before(
        &self,
        owner: &Statement,
        block: &Block,
        statement: &Statement,
        lines: &Lines,
    ) -> Plan {
        let pos = statement.span.start as usize;
        let start = line_start(self.src, pos);
        if start == 0 || !starts_line(self.src, pos) {
            return Plan::Split(self.split(owner, block));
        }
        let indent = indent_at(self.src, pos).to_owned();
        let eol = eol_before(self.src, start);
        Plan::Patches(vec![Patch::insert(
            start,
            self.render(&indent, lines, eol, false),
        )])
    }

    /// Rewrites `owner`'s `block` with each statement and comment on its
    /// own line, indented one unit below `owner`, and the closing brace on
    /// its own line (DSL-9: a one-line view is first split onto lines).
    fn split(&self, owner: &Statement, block: &Block) -> Patch {
        let owner_indent = indent_at(self.src, owner.span.start as usize);
        let body = format!("{owner_indent}{}", self.unit);
        let mut items: Vec<&str> = Vec::new();
        let mut statements = block.statements.iter().peekable();
        let mut i = block.open + 1;
        while i < block.close {
            let t = self.token(i);
            if let Some(s) = statements.peek().filter(|s| s.span.start == t.start) {
                items.push(&self.src[s.span.range()]);
                let end = s.span.end;
                statements.next();
                while i < block.close && self.token(i).start < end {
                    i += 1;
                }
                continue;
            }
            if matches!(t.kind, TokenKind::LineComment | TokenKind::BlockComment) {
                items.push(&self.src[t.range()]);
            }
            i += 1;
        }
        let mut text = String::from("{");
        text.push_str(self.eol);
        for item in items {
            text.push_str(&body);
            text.push_str(item);
            text.push_str(self.eol);
        }
        text.push_str(owner_indent);
        text.push('}');
        let open = self.token(block.open).start as usize;
        let close = self.token(block.close).end as usize;
        Patch {
            range: open..close,
            text,
        }
    }

    /// Removes the line(s) of the statement at `span`, line ending
    /// included; a statement sharing its line loses only its bytes and
    /// the blank before them.
    fn remove_line(&self, span: Span) -> Patch {
        let (start, end) = (span.start as usize, span.end as usize);
        let first = line_start(self.src, start);
        let last = line_end(self.src, end);
        let alone = starts_line(self.src, start) && self.src[end..last].trim().is_empty();
        if !alone {
            let before = self.src[..start].trim_end_matches([' ', '\t']).len();
            return Patch::remove(before..end);
        }
        let rest = &self.src[last..];
        if rest.starts_with("\r\n") {
            Patch::remove(first..last + 2)
        } else if rest.starts_with('\n') {
            Patch::remove(first..last + 1)
        } else if first > 0 {
            // Last line with no final newline: take the newline before.
            Patch::remove(first - eol_before(self.src, first).len()..last)
        } else {
            Patch::remove(first..last)
        }
    }

    /// True when every token of `block` is blank space or inside one of
    /// its statements: no comment of its own.
    fn only_statements(&self, block: &Block) -> bool {
        (block.open + 1..block.close).all(|i| {
            let t = self.token(i);
            matches!(t.kind, TokenKind::Whitespace | TokenKind::Newline)
                || block
                    .statements
                    .iter()
                    .any(|s| s.span.start <= t.start && t.end <= s.span.end)
        })
    }

    /// Removes `statement`, or the outermost of `parents` (statement
    /// spans, innermost first) that holds nothing but it (EDIT-21: a
    /// block goes only when it holds nothing else).
    fn remove_nested(&self, statement: &Statement, parents: &[Option<Span>]) -> Patch {
        let mut target = statement;
        for span in parents.iter().flatten() {
            let Some(parent) = self.statement(*span) else {
                break;
            };
            match &parent.block {
                Some(block) if self.holds_only(block, target) => target = parent,
                _ => break,
            }
        }
        self.remove_line(target.span)
    }

    /// True when the view statement `view` holds nothing but what `edit`
    /// writes in a default view (EDIT-15): a head of keyword, scope and
    /// key, one `include *` line, `at` and `size` lines, no comment.
    fn holds_only_default_view(&self, view: &Statement) -> bool {
        let Some(block) = &view.block else {
            return false;
        };
        if self.head(view).len() != 3 || !self.only_statements(block) {
            return false;
        }
        let mut includes = 0;
        for statement in &block.statements {
            let words: Vec<&str> = self
                .head(statement)
                .into_iter()
                .map(|i| self.tree.token_text(i))
                .collect();
            if statement.block.is_some() {
                return false;
            }
            match words.first().map(|w| w.to_ascii_lowercase()).as_deref() {
                Some("include") if words.len() == 2 && words[1] == "*" => includes += 1,
                Some("at" | "size" | "papersize" | "line") => {}
                _ => return false,
            }
        }
        includes == 1
    }

    /// Inserts `lines` as the last lines of `styles`, creating `styles`
    /// as the last block of `views` and `views` as the last block of the
    /// workspace body when missing (DSL-13, EDIT-15).
    fn insert_in_styles(&self, lines: Lines) -> Res<Plan> {
        if let Some(styles) = self.model.styles_span.and_then(|s| self.statement(s))
            && let Some(block) = &styles.block
        {
            return Ok(self.insert_last(styles, block, &lines));
        }
        self.insert_in_views(wrap("styles", lines))
    }

    /// Inserts `lines` as the last lines of `views`, creating it as the
    /// last block of the workspace body when missing (DSL-13, EDIT-15).
    fn insert_in_views(&self, lines: Lines) -> Res<Plan> {
        if let Some(views) = self.model.views_span.and_then(|s| self.statement(s))
            && let Some(block) = &views.block
        {
            return Ok(self.insert_last(views, block, &lines));
        }
        let lines = wrap("views", lines);
        match self.workspace() {
            Some(workspace) => match &workspace.block {
                Some(block) => Ok(self.insert_last(workspace, block, &lines)),
                None => Err(no_workspace()),
            },
            None => Err(no_workspace()),
        }
    }

}

/// `lines` inside a new `<keyword> { }` block.
fn wrap(keyword: &str, lines: Lines) -> Lines {
    let mut out = vec![(0, format!("{keyword} {{"))];
    out.extend(lines.into_iter().map(|(depth, line)| (depth + 1, line)));
    out.push((0, "}".to_owned()));
    out
}

fn no_workspace() -> EditErrorKind {
    EditErrorKind::InvalidValue {
        value: String::new(),
        reason: "the file has no workspace block to hold views",
    }
}

/// The bare key of `id` (phase 1: always written).
/// The canonical element key: `Model::full_key`, the dotted full id
/// under hierarchical ids (P2-142).
fn element_key(model: &model::Model, id: ElementId) -> String {
    model.full_key(id).unwrap_or_default()
}

/// The identifier written before `=`, without its parents.
fn bare_key(model: &model::Model, id: ElementId) -> String {
    model
        .element(id)
        .key
        .as_ref()
        .map(|k| k.value.clone())
        .unwrap_or_default()
}

fn gone(target: &str) -> EditErrorKind {
    EditErrorKind::TargetGone {
        target: target.to_owned(),
    }
}

fn find_element(model: &model::Model, key: &str) -> Res<ElementId> {
    model.find(key).ok_or_else(|| gone(key))
}

// ----------------------------------------------------- at and size lines

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    At,
    Size,
}

impl LineKind {
    const fn keyword(self) -> &'static str {
        match self {
            LineKind::At => "at",
            LineKind::Size => "size",
        }
    }

    fn edit(self, view: &str, element: &str, value: Option<(i32, i32)>) -> Edit {
        match self {
            LineKind::At => Edit::Move {
                view: view.to_owned(),
                element: element.to_owned(),
                centre: value,
            },
            LineKind::Size => Edit::Resize {
                view: view.to_owned(),
                element: element.to_owned(),
                size: value,
            },
        }
    }

    /// The line for `element` in `view`, as (line span, first value span,
    /// second value span, values).
    fn find(self, view: &ViewSpec, element: ElementId) -> Option<(Span, Span, Span, (i32, i32))> {
        match self {
            LineKind::At => view
                .position_of(element)
                .map(|a| (a.span, a.x.span, a.y.span, (a.x.value, a.y.value))),
            LineKind::Size => view.size_of(element).map(|s| {
                (
                    s.span,
                    s.width.span,
                    s.height.span,
                    (s.width.value, s.height.value),
                )
            }),
        }
    }
}

/// The view and element of a box edit: the view exists and the element
/// is in it (EDIT-24).
fn find_box<'m>(model: &'m model::Model, view: &str, element: &str) -> Res<(&'m ViewSpec, ElementId)> {
    let spec = model.view(view).ok_or_else(|| gone(view))?;
    // P3-117: an instance without an id is `<element key>_<n>` in its
    // view's environment; another environment may use the same key.
    let included = model.included(spec);
    let local = included
        .iter()
        .copied()
        .find(|&e| model.element(e).key.is_none() && model.full_key(e).is_some_and(|k| k.eq_ignore_ascii_case(element)));
    let id = match local {
        Some(id) => id,
        None => find_element(model, element)?,
    };
    let drawn = included.contains(&id)
        || spec.position_of(id).is_some()
        || spec.size_of(id).is_some();
    if drawn { Ok((spec, id)) } else { Err(gone(element)) }
}

/// `Move` and `Resize` (DSL-7, DSL-9, EDIT-18).
fn box_line(
    current: &mut Parsed,
    view: &str,
    element: &str,
    value: Option<(i32, i32)>,
    kind: LineKind,
) -> Res<Vec<Edit>> {
    if let (LineKind::Size, Some((w, h))) = (kind, value)
        && (w <= 0 || h <= 0)
    {
        return Err(EditErrorKind::InvalidValue {
            value: format!("{w} {h}"),
            reason: "a box width and height must be greater than 0",
        });
    }
    let (spec, id) = find_box(&current.model, view, element)?;
    let old = kind.find(spec, id).map(|(_, _, _, v)| v);
    let inverse = vec![kind.edit(view, element, old)];
    // DECISION: B dsl-breadth 7: the line goes into the file holding the
    // view block, the root file when the root declares the view (P2-140).
    let file = spec.span.file;
    run_in(current, file, |cx| {
        let (spec, id) = find_box(cx.model, view, element)?;
        let existing = kind.find(spec, id);
        Ok(match (existing, value) {
            (None, None) => Plan::Patches(Vec::new()),
            (Some((line, _, _, _)), None) => Plan::Patches(vec![cx.remove_line(line)]),
            (Some((_, a, b, _)), Some((x, y))) => Plan::Patches(vec![
                Patch {
                    range: a.range(),
                    text: x.to_string(),
                },
                Patch {
                    range: b.range(),
                    text: y.to_string(),
                },
            ]),
            (None, Some((x, y))) => {
                let line = one_line(format!("{} {element} {x} {y}", kind.keyword()));
                let owner = cx.statement(spec.span).ok_or_else(|| gone(view))?;
                let Some(block) = &owner.block else {
                    return Err(gone(view));
                };
                // DSL-9: after the last line of the same kind; a `size`
                // line falls back to the last `at` line.
                let anchor = match kind {
                    LineKind::At => spec.positions.last().map(|a| a.span),
                    LineKind::Size => spec
                        .sizes
                        .last()
                        .map(|s| s.span)
                        .or_else(|| spec.positions.last().map(|a| a.span)),
                };
                match anchor.and_then(|a| Ctx::child_holding(block, a)) {
                    Some(after) => cx.insert_after(owner, block, after, &line),
                    None => cx.insert_last(owner, block, &line),
                }
            }
        })
    })?;
    Ok(inverse)
}

// ------------------------------------------- paperSize and line lines

/// The view statement and its block for `spec` in `cx`.
fn view_block<'a>(cx: &Ctx<'a>, spec: &ViewSpec, view: &str) -> Res<(&'a Statement, &'a Block)> {
    let owner = cx.statement(spec.span).ok_or_else(|| gone(view))?;
    match &owner.block {
        Some(block) => Ok((owner, block)),
        None => Err(gone(view)),
    }
}

/// Inserts `line` after the last statement of `block` whose span is one
/// of `anchors` (tried in order), else before the closing brace (DSL-9).
fn insert_after_first(cx: &Ctx<'_>, owner: &Statement, block: &Block, anchors: &[Option<Span>], line: &Lines) -> Plan {
    match anchors.iter().flatten().find_map(|a| Ctx::child_holding(block, *a)) {
        Some(after) => cx.insert_after(owner, block, after, line),
        None => cx.insert_last(owner, block, line),
    }
}

/// `SetCanvasSize` (P2-701): the `paperSize` line, after the view's last
/// `size` line, else its last `at` line, else before the closing brace.
// DECISION: B editor-breadth 5b says "placed like `at` lines": a new
// `paperSize` line follows the box lines it sizes the canvas for.
fn set_canvas_size(current: &mut Parsed, view: &str, size: Option<PaperSize>) -> Res<Vec<Edit>> {
    let spec = current.model.view(view).ok_or_else(|| gone(view))?;
    let inverse = vec![Edit::SetCanvasSize {
        view: view.to_owned(),
        size: spec.paper_size.map(|p| p.value),
    }];
    let file = spec.span.file;
    run_in(current, file, |cx| {
        let spec = cx.model.view(view).ok_or_else(|| gone(view))?;
        let (owner, block) = view_block(cx, spec, view)?;
        Ok(match (spec.paper_size, size) {
            (None, None) => Plan::Patches(Vec::new()),
            (Some(old), None) => {
                let line = Ctx::child_holding(block, old.span).ok_or_else(|| gone(view))?;
                Plan::Patches(vec![cx.remove_line(line.span)])
            }
            (Some(old), Some(new)) => Plan::Patches(vec![Patch {
                range: old.span.range(),
                text: new.keyword().to_owned(),
            }]),
            (None, Some(new)) => {
                let line = one_line(format!("paperSize {}", new.keyword()));
                let anchors = [spec.sizes.last().map(|s| s.span), spec.positions.last().map(|a| a.span)];
                insert_after_first(cx, owner, block, &anchors, &line)
            }
        })
    })?;
    Ok(inverse)
}

/// One setting of a `line` line.
#[derive(Clone, Copy)]
enum LineSetting {
    Position(Option<u8>),
    Routing(Option<Routing>),
    Jump(Option<bool>),
}

impl LineSetting {
    /// This setting as `line` holds it now.
    fn of(self, line: Option<&model::LineLayout>) -> LineSetting {
        match self {
            LineSetting::Position(_) => LineSetting::Position(line.and_then(|l| l.position).map(|p| p.value)),
            LineSetting::Routing(_) => LineSetting::Routing(line.and_then(|l| l.routing).map(|r| r.value)),
            LineSetting::Jump(_) => LineSetting::Jump(line.and_then(|l| l.jump).map(|j| j.value)),
        }
    }

    fn edit(self, view: &str, relationship: &RelationshipRef) -> Edit {
        let (view, relationship) = (view.to_owned(), relationship.clone());
        match self {
            LineSetting::Position(position) => Edit::SetLinePosition {
                view,
                relationship,
                position,
            },
            LineSetting::Routing(routing) => Edit::SetLineRouting {
                view,
                relationship,
                routing,
            },
            LineSetting::Jump(jump) => Edit::SetLineJump {
                view,
                relationship,
                jump,
            },
        }
    }
}

/// The options part of a `line` line: `position N`, `routing R`, `jump
/// on|off`, in that order, those that are set.
fn line_options(position: Option<u8>, routing: Option<Routing>, jump: Option<bool>) -> String {
    let mut out = String::new();
    if let Some(p) = position {
        out.push_str(" position ");
        out.push_str(&p.to_string());
    }
    if let Some(r) = routing {
        out.push_str(" routing ");
        out.push_str(r.line_keyword());
    }
    if let Some(j) = jump {
        out.push_str(if j { " jump on" } else { " jump off" });
    }
    out
}

/// How a new `line` line names `relationship`: its identifier, else
/// `src -> dst` when it is the first relationship between those ends.
// DECISION: an unnamed relationship that is not the first `src -> dst`
// one cannot be named by the line grammar (B editor-breadth 6); the edit
// is refused, as P2-145 refuses edits on elements without an identifier.
fn line_name(model: &model::Model, id: RelationshipId, r: &RelationshipRef) -> Res<String> {
    if let Some(key) = &model.relationship(id).key {
        return Ok(key.value.clone());
    }
    if r.ordinal == 0 {
        return Ok(format!("{} -> {}", r.source, r.destination));
    }
    Err(EditErrorKind::InvalidValue {
        value: format!("{} -> {}", r.source, r.destination),
        reason: "give this relationship an identifier: another relationship has the same ends",
    })
}

/// `SetLinePosition`, `SetLineRouting`, `SetLineJump` (P2-708..P2-710):
/// the view's `line` line for the relationship, rewritten with the
/// setting changed, added after the view's last `line` line (else its
/// `paperSize`, last `size` or last `at` line), or removed when it would
/// hold nothing.
fn set_line(current: &mut Parsed, view: &str, relationship: &RelationshipRef, setting: LineSetting) -> Res<Vec<Edit>> {
    let spec = current.model.view(view).ok_or_else(|| gone(view))?;
    let id = resolve_relationship(&current.model, relationship)?;
    let existing = spec.lines.iter().find(|l| l.relationship.value == id);
    let inverse = vec![setting.of(existing).edit(view, relationship)];
    let file = spec.span.file;
    run_in(current, file, |cx| {
        let spec = cx.model.view(view).ok_or_else(|| gone(view))?;
        let (owner, block) = view_block(cx, spec, view)?;
        let existing = spec.lines.iter().find(|l| l.relationship.value == id);
        let mut position = existing.and_then(|l| l.position).map(|p| p.value);
        let mut routing = existing.and_then(|l| l.routing).map(|r| r.value);
        let mut jump = existing.and_then(|l| l.jump).map(|j| j.value);
        match setting {
            LineSetting::Position(p) => position = p,
            LineSetting::Routing(r) => routing = r,
            LineSetting::Jump(j) => jump = j,
        }
        let empty = position.is_none() && routing.is_none() && jump.is_none();
        Ok(match existing {
            None if empty => Plan::Patches(Vec::new()),
            Some(line) if empty => {
                let statement = Ctx::child_holding(block, line.span).ok_or_else(|| gone(view))?;
                Plan::Patches(vec![cx.remove_line(statement.span)])
            }
            Some(line) => {
                let name = &cx.src[line.relationship.span.range()];
                Plan::Patches(vec![Patch {
                    range: line.span.range(),
                    text: format!("line {name}{}", line_options(position, routing, jump)),
                }])
            }
            None => {
                let name = line_name(cx.model, id, relationship)?;
                let line = one_line(format!("line {name}{}", line_options(position, routing, jump)));
                let anchors = [
                    spec.lines.last().map(|l| l.span),
                    spec.paper_size.map(|p| p.span),
                    spec.sizes.last().map(|s| s.span),
                    spec.positions.last().map(|a| a.span),
                ];
                insert_after_first(cx, owner, block, &anchors, &line)
            }
        })
    })?;
    Ok(inverse)
}

// ------------------------------------------------------------ text edits

fn set_text(
    current: &mut Parsed,
    target: &TextTarget,
    value: Option<&str>,
    expected: Option<&str>,
) -> Res<Vec<Edit>> {
    if let Some(v) = value {
        check_value(v)?;
    }
    // P2-139: the file the target's statement is in; a literal from a
    // `workspace extends` base patches the base (B dsl-breadth 9).
    let file = target_file(&current.model, target)?;
    let field = TextField::resolve(&Ctx::in_file(current, file), target, value)?;
    if let Some(name) = field.current.as_ref().and_then(|l| substituted(current, l.span)) {
        return Err(EditErrorKind::Substituted { name });
    }
    let found = field.current.as_ref().map(|l| l.value.clone());
    if let Some(expected) = expected
        && found.as_deref() != Some(expected)
    {
        return Err(EditErrorKind::Conflict {
            expected: expected.to_owned(),
            found,
        });
    }
    let inverse = vec![Edit::SetText {
        target: target.clone(),
        value: found,
        expected: value.map(str::to_owned),
    }];
    run_in(current, file, |cx| {
        Ok(TextField::resolve(cx, target, value)?.plan(cx, value))
    })?;
    Ok(inverse)
}

/// The name of the first `${NAME}` in the bytes `span` covers, if any:
/// rewriting that literal would drop the reference (P2-151).
// DECISION: B dsl-breadth 3: the edit is refused and names the constant.
fn substituted(parsed: &Parsed, span: Span) -> Option<String> {
    let tree = parsed.trees.get(span.file.0 as usize)?;
    let written = tree.source().get(span.range())?;
    let at = written.find("${")?;
    let rest = &written[at + 2..];
    let end = rest.find('}').unwrap_or(rest.len());
    Some(rest[..end].to_owned())
}

/// The file holding the statement a text target belongs to.
fn target_file(model: &model::Model, target: &TextTarget) -> Res<FileId> {
    Ok(match target {
        TextTarget::Element { element, .. } => model.element(find_element(model, element)?).span.file,
        TextTarget::Relationship { relationship, .. } => {
            model
                .relationship(resolve_relationship(model, relationship)?)
                .span
                .file
        }
        TextTarget::View { view, .. } => model.view(view).ok_or_else(|| gone(view))?.span.file,
        TextTarget::Step { view, .. } => model.view(view).ok_or_else(|| gone(view))?.span.file,
    })
}

/// The file declaring the element keyed `element`: a per-box `tags` line
/// goes there (B dsl-breadth 8).
fn element_file(model: &model::Model, element: &str) -> Res<FileId> {
    Ok(model.element(find_element(model, element)?).span.file)
}

/// DSL-10: one line, no control character but tab.
fn check_value(value: &str) -> Res<()> {
    if value.contains(['\n', '\r']) {
        return Err(EditErrorKind::Newline);
    }
    if value.chars().any(|c| c.is_control() && c != '\t') {
        return Err(EditErrorKind::InvalidValue {
            value: value.to_owned(),
            reason: "a string cannot hold a control character other than tab",
        });
    }
    Ok(())
}

/// Where a text target's literal lives in the current text.
struct TextField<'a> {
    /// The literal now, if written.
    current: Option<model::Literal>,
    /// The statement holding the positional arguments, or the view.
    owner: &'a Statement,
    place: Place,
}

enum Place {
    /// Positional argument `slot` (0 = the first string after `base`,
    /// an index into the head's non-trivia tokens); slots below `keep`
    /// are never removed (an element's name).
    Positional {
        base: usize,
        slot: usize,
        keep: usize,
    },
    /// A body line only (`title`, `subtitle`).
    Line(&'static str),
}

impl<'a> TextField<'a> {
    fn resolve(cx: &Ctx<'a>, target: &TextTarget, value: Option<&str>) -> Res<TextField<'a>> {
        let model = cx.model;
        let refused = |reason| EditErrorKind::InvalidValue {
            value: value.unwrap_or_default().to_owned(),
            reason,
        };
        let (current, span, place) = match target {
            TextTarget::Element { element, field } => {
                let e = model.element(find_element(model, element)?);
                let (current, slot) = match field {
                    ElementField::Name if value.is_none() => {
                        return Err(refused("an element name cannot be removed"));
                    }
                    ElementField::Name => (Some(e.name.clone()), 0),
                    ElementField::Description => (e.description.clone(), 1),
                    ElementField::Technology
                        if matches!(e.kind, ElementKind::Person | ElementKind::SoftwareSystem) =>
                    {
                        return Err(refused("a person or software system has no technology"));
                    }
                    ElementField::Technology => (e.technology.clone(), 2),
                };
                // `key = kind "name" ...`: the name is the fourth token.
                let place = Place::Positional {
                    base: 3,
                    slot,
                    keep: 1,
                };
                (current, e.span, place)
            }
            TextTarget::Relationship {
                relationship,
                field,
            } => {
                let r = model.relationship(resolve_relationship(model, relationship)?);
                let (current, slot) = match field {
                    RelationshipField::Description => (r.description.clone(), 0),
                    RelationshipField::Technology => (r.technology.clone(), 1),
                };
                let owner = cx.statement(r.span).ok_or_else(|| gone(&relationship.source))?;
                let arrow = cx
                    .head(owner)
                    .iter()
                    .position(|&i| cx.token(i).kind == TokenKind::Arrow)
                    .unwrap_or_default();
                let place = Place::Positional {
                    base: arrow + 2,
                    slot,
                    keep: 0,
                };
                (current, r.span, place)
            }
            TextTarget::View { view, field } => {
                let v = model.view(view).ok_or_else(|| gone(view))?;
                match field {
                    ViewField::Title => (v.title.clone(), v.span, Place::Line("title")),
                    ViewField::Subtitle => (v.subtitle.clone(), v.span, Place::Line("subtitle")),
                }
            }
            TextTarget::Step { view, step } => {
                // P3-218: the step's own description, the first string
                // after `<src> -> <dst>` or after the relationship id.
                let st = step_of(model, view, *step)?;
                let owner = cx.leaf_holding(st.span).ok_or_else(|| refused("its statement was not found"))?;
                let head = cx.head(owner);
                let base = match head.iter().position(|&i| cx.token(i).kind == TokenKind::Arrow) {
                    Some(arrow) => arrow + 2,
                    None => usize::from(st.written_order.is_some()) + 1,
                };
                let place = Place::Positional { base, slot: 0, keep: 0 };
                (st.description.clone(), owner.span, place)
            }
        };
        let owner = cx.statement(span).ok_or_else(|| refused("its statement was not found"))?;
        Ok(TextField {
            current,
            owner,
            place,
        })
    }

    fn plan(&self, cx: &Ctx<'_>, value: Option<&str>) -> Plan {
        let head = cx.head(self.owner);
        let replace = |span: Span, text: String| {
            Plan::Patches(vec![Patch {
                range: span.range(),
                text,
            }])
        };
        match (&self.current, value, &self.place) {
            (None, None, _) => Plan::Patches(Vec::new()),
            // DECISION: a bare word (P2-149) or a text block (P2-148) is
            // rewritten as a quoted literal in its place.
            (Some(lit), Some(v), _) => replace(lit.span, text::quote(v)),
            (Some(lit), None, Place::Positional { base, keep, .. }) => {
                match head.iter().position(|&i| cx.tree.span(i) == lit.span) {
                    // A body line (`description`, `technology`).
                    None => match &self.owner.block {
                        Some(block) => match Ctx::child_holding(block, lit.span) {
                            Some(line) => Plan::Patches(vec![cx.remove_line(line.span)]),
                            None => Plan::Patches(Vec::new()),
                        },
                        None => Plan::Patches(Vec::new()),
                    },
                    Some(k) if k + 1 == head.len() => {
                        // The trailing positional: removed with the `""`
                        // placeholders before it (DSL-10, Applied docs).
                        let mut first = k;
                        while first > base + keep && cx.tree.token_text(head[first - 1]) == "\"\"" {
                            first -= 1;
                        }
                        let from = cx.token(head[first - 1]).end as usize;
                        Plan::Patches(vec![Patch::remove(from..lit.span.end as usize)])
                    }
                    Some(_) => replace(lit.span, "\"\"".to_owned()),
                }
            }
            (Some(lit), None, Place::Line(_)) => match &self.owner.block {
                Some(block) => match Ctx::child_holding(block, lit.span) {
                    Some(line) => Plan::Patches(vec![cx.remove_line(line.span)]),
                    None => Plan::Patches(Vec::new()),
                },
                None => Plan::Patches(Vec::new()),
            },
            (None, Some(v), Place::Positional { base, slot, .. }) => {
                let written = head.len().saturating_sub(*base);
                let mut text = " \"\"".repeat(slot.saturating_sub(written));
                text.push(' ');
                text.push_str(&text::quote(v));
                let at = head.last().map_or(self.owner.span.end, |&i| cx.token(i).end);
                Plan::Patches(vec![Patch::insert(at as usize, text)])
            }
            (None, Some(v), Place::Line(keyword)) => match &self.owner.block {
                Some(block) => cx.insert_first(
                    self.owner,
                    block,
                    &one_line(format!("{keyword} {}", text::quote(v))),
                ),
                None => Plan::Patches(Vec::new()),
            },
        }
    }
}

/// The relationship a reference names in `model` (EDIT-24).
fn resolve_relationship(model: &model::Model, r: &RelationshipRef) -> Res<RelationshipId> {
    let source = find_element(model, &r.source)?;
    let destination = find_element(model, &r.destination)?;
    model
        .find_relationship(source, destination)
        .get(r.ordinal as usize)
        .copied()
        .ok_or_else(|| gone(&format!("{} -> {}", r.source, r.destination)))
}

// ----------------------------------------------------------- style edits

fn set_style(
    current: &mut Parsed,
    element: &str,
    target: &StyleTarget,
    property: StyleProperty,
) -> Res<Vec<Edit>> {
    let model = &current.model;
    let id = find_element(model, element)?;
    let undo = |property| {
        vec![Edit::SetStyle {
            element: element.to_owned(),
            target: target.clone(),
            property,
        }]
    };
    match target {
        StyleTarget::Tag(tag) => {
            if !model.element(id).has_tag(tag) {
                return Err(EditErrorKind::InvalidValue {
                    value: tag.clone(),
                    reason: "the element does not carry this tag",
                });
            }
            let inverse = undo(tag_value(model, tag, property));
            run(current, |cx| plan_tag_style(cx, tag, property))?;
            Ok(inverse)
        }
        StyleTarget::ThisBox => match per_box_tag(model, id) {
            Some(tag) => {
                let inverse = undo(tag_value(model, &tag, property));
                run(current, |cx| plan_tag_style(cx, &tag, property))?;
                // DECISION: when clearing a property leaves no `element
                // "<tag>"` block, the per-box tag goes too, so undoing a
                // first "this box only" change gives the bytes back; while
                // the block holds another property the tag stays, since
                // removing it would drop a style `edit` did not write.
                let unstyled = file_blocks(&current.model, &tag).next().is_none();
                if prop_parts(property).1.is_none() && unstyled {
                    let file = element_file(&current.model, element)?;
                    run_in(current, file, |cx| plan_remove_tag(cx, element, &tag))?;
                }
                Ok(inverse)
            }
            None if prop_parts(property).1.is_none() => Ok(Vec::new()),
            None => {
                let tag = new_per_box_tag(model, id);
                let inverse = undo(cleared(property));
                let file = element_file(&current.model, element)?;
                run_in(current, file, |cx| plan_add_tag(cx, element, &tag))?;
                run(current, |cx| plan_tag_style(cx, &tag, property))?;
                Ok(inverse)
            }
        },
    }
}

fn reset_style(current: &mut Parsed, element: &str) -> Res<Vec<Edit>> {
    let model = &current.model;
    let id = find_element(model, element)?;
    let Some(tag) = per_box_tag(model, id) else {
        return Ok(Vec::new());
    };
    let resolved = [
        StyleProperty::Background(None),
        StyleProperty::Color(None),
        StyleProperty::Shape(None),
    ]
    .map(|p| tag_value(model, &tag, p));
    let inverse = resolved
        .into_iter()
        .filter(|p| prop_parts(*p).1.is_some())
        .map(|property| Edit::SetStyle {
            element: element.to_owned(),
            target: StyleTarget::ThisBox,
            property,
        })
        .collect();
    // One block (or its properties) per pass; each pass removes bytes,
    // so the loop ends.
    loop {
        let before = root(current).source().len();
        run(current, |cx| Ok(plan_clear_block(cx, &tag)))?;
        if root(current).source().len() == before {
            break;
        }
    }
    let file = element_file(&current.model, element)?;
    run_in(current, file, |cx| plan_remove_tag(cx, element, &tag))?;
    Ok(inverse)
}

/// The property keyword and its value as written, `None` to remove.
fn prop_parts(property: StyleProperty) -> (&'static str, Option<String>) {
    match property {
        StyleProperty::Background(v) => ("background", v.map(|c| c.to_string())),
        StyleProperty::Color(v) => ("color", v.map(|c| c.to_string())),
        StyleProperty::Shape(v) => ("shape", v.map(|s| shape_word(s).to_owned())),
    }
}

/// P2-417: the reference spelling (`MobileDevicePortrait`).
const fn shape_word(shape: Shape) -> &'static str {
    shape.keyword()
}

/// The same property with no value.
const fn cleared(property: StyleProperty) -> StyleProperty {
    match property {
        StyleProperty::Background(_) => StyleProperty::Background(None),
        StyleProperty::Color(_) => StyleProperty::Color(None),
        StyleProperty::Shape(_) => StyleProperty::Shape(None),
    }
}

/// `property`'s kind as `style` sets it: the value's span and the value.
fn prop_in(style: &ElementStyle, property: StyleProperty) -> (Option<Span>, StyleProperty) {
    match property {
        StyleProperty::Background(_) => (
            style.background.map(|c| c.span),
            StyleProperty::Background(style.background.map(|c| c.value)),
        ),
        StyleProperty::Color(_) => (
            style.color.map(|c| c.span),
            StyleProperty::Color(style.color.map(|c| c.value)),
        ),
        StyleProperty::Shape(_) => (
            style.shape.map(|s| s.span),
            StyleProperty::Shape(style.shape.map(|s| s.value)),
        ),
    }
}

/// The file's own top-level `element "<tag>"` blocks, in file order:
/// the only blocks `edit` reads or writes (P2-439).
// DECISION: a theme style is never edited (its span is the `theme`
// statement), and neither is a block inside `light`/`dark`: an edit
// writes the top-level block, which applies in both modes. A mode block
// that sets the same property still wins in its mode (P2-430); the
// editor's mode switch (styles-themes-17) decides whether to edit it.
fn file_blocks<'m>(
    model: &'m model::Model,
    tag: &'m str,
) -> impl DoubleEndedIterator<Item = &'m ElementStyle> {
    model
        .element_styles
        .iter()
        .filter(move |s| s.theme.is_none() && s.mode.is_none() && s.tag.value == tag)
}

/// `property`'s kind as the file's `element "<tag>"` blocks set it, later
/// blocks winning (DSL-15).
fn tag_value(model: &model::Model, tag: &str, property: StyleProperty) -> StyleProperty {
    file_blocks(model, tag)
        .rev()
        .find_map(|s| match prop_in(s, property) {
            (Some(_), value) => Some(value),
            (None, _) => None,
        })
        .unwrap_or(cleared(property))
}

/// Writes `property` into the `element "<tag>"` block (DSL-13).
// DECISION: with several blocks for one tag, the value is rewritten in
// the last block that sets it (the one that takes effect, DSL-15), and a
// new property line goes into the last block.
fn plan_tag_style(cx: &Ctx<'_>, tag: &str, property: StyleProperty) -> Res<Plan> {
    let (keyword, value) = prop_parts(property);
    let blocks: Vec<&ElementStyle> = file_blocks(cx.model, tag).collect();
    let setting = blocks
        .iter()
        .rev()
        .find_map(|s| prop_in(s, property).0.map(|span| (*s, span)));
    let missing = || EditErrorKind::InvalidValue {
        value: tag.to_owned(),
        reason: "its style block was not found",
    };
    Ok(match (setting, value) {
        (Some((_, span)), Some(v)) => Plan::Patches(vec![Patch {
            range: span.range(),
            text: v,
        }]),
        (Some((style, span)), None) => {
            let owner = cx.statement(style.span).ok_or_else(missing)?;
            let block = owner.block.as_ref().ok_or_else(missing)?;
            let line = Ctx::child_holding(block, span).ok_or_else(missing)?;
            let parents = [Some(style.span), cx.model.styles_span, cx.model.views_span];
            Plan::Patches(vec![cx.remove_nested(line, &parents)])
        }
        (None, None) => Plan::Patches(Vec::new()),
        (None, Some(v)) => {
            let line = format!("{keyword} {v}");
            if let Some(style) = blocks.last() {
                let owner = cx.statement(style.span).ok_or_else(missing)?;
                let block = owner.block.as_ref().ok_or_else(missing)?;
                cx.insert_last(owner, block, &one_line(line))
            } else {
                let lines = vec![
                    (0, format!("element {} {{", text::quote(tag))),
                    (1, line),
                    (0, "}".to_owned()),
                ];
                cx.insert_in_styles(lines)?
            }
        }
    })
}

/// Removes the properties of the last `element "<tag>"` block that has
/// any: the whole block (and `styles`, `views` when they hold nothing
/// else) when it holds no comment, else only the property lines.
fn plan_clear_block(cx: &Ctx<'_>, tag: &str) -> Plan {
    for style in file_blocks(cx.model, tag).rev() {
        let Some(owner) = cx.statement(style.span) else {
            continue;
        };
        let Some(block) = &owner.block else {
            continue;
        };
        if cx.only_statements(block) {
            let parents = [cx.model.styles_span, cx.model.views_span];
            return Plan::Patches(vec![cx.remove_nested(owner, &parents)]);
        }
        if !block.statements.is_empty() {
            return Plan::Patches(
                block
                    .statements
                    .iter()
                    .map(|s| cx.remove_line(s.span))
                    .collect(),
            );
        }
    }
    Plan::Patches(Vec::new())
}

/// The element's full dotted id (`a.b.c`, DSL-7, DSL-13).
fn dotted_id(model: &model::Model, id: ElementId) -> String {
    let mut parts = vec![bare_key(model, id)];
    let mut parent = model.element(id).parent;
    while let Some(p) = parent {
        parts.push(bare_key(model, p));
        parent = model.element(p).parent;
    }
    parts.reverse();
    parts.join(".")
}

/// Every tag `id` declares, split and trimmed, in order.
fn declared_tags(model: &model::Model, id: ElementId) -> Vec<String> {
    model
        .element(id)
        .declared_tags
        .iter()
        .flat_map(|l| l.value.split(','))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The element's "this box only" tag, if it has one: its dotted id, or
/// that id suffixed `-N` (N >= 2), carried by no other element (DSL-13).
fn per_box_tag(model: &model::Model, id: ElementId) -> Option<String> {
    let dotted = dotted_id(model, id);
    declared_tags(model, id)
        .into_iter()
        .rev()
        .filter(|t| !model.elements.iter().any(|e| e.id != id && e.has_tag(t)))
        .find(|t| {
            t == &dotted
                || t.strip_prefix(dotted.as_str())
                    .and_then(|rest| rest.strip_prefix('-'))
                    .and_then(|n| n.parse::<u32>().ok())
                    .is_some_and(|n| n >= 2)
        })
}

/// A new "this box only" tag: the dotted id, suffixed `-2`, `-3`... when
/// an element already carries it (DSL-13).
fn new_per_box_tag(model: &model::Model, id: ElementId) -> String {
    let dotted = dotted_id(model, id);
    let taken = |t: &str| model.elements.iter().any(|e| e.has_tag(t));
    if !taken(&dotted) {
        return dotted;
    }
    (2..=u32::MAX)
        .map(|n| format!("{dotted}-{n}"))
        .find(|t| !taken(t))
        .unwrap_or(dotted)
}

/// Adds `tags "<tag>"` as the last line of the element's block, creating
/// the block when it has none (DSL-13).
fn plan_add_tag(cx: &Ctx<'_>, element: &str, tag: &str) -> Res<Plan> {
    let e = cx.model.element(find_element(cx.model, element)?);
    let owner = cx.statement(e.span).ok_or_else(|| gone(element))?;
    let line = format!("tags {}", text::quote(tag));
    if let Some(block) = &owner.block {
        return Ok(cx.insert_last(owner, block, &one_line(line)));
    }
    let head = cx.head(owner);
    let at = head.last().map_or(owner.span.end, |&i| cx.token(i).end) as usize;
    let indent = indent_at(cx.src, owner.span.start as usize);
    let end = line_end(cx.src, at);
    let eol = if cx.src[end..].starts_with("\r\n") {
        "\r\n"
    } else if cx.src[end..].starts_with('\n') {
        "\n"
    } else {
        cx.eol
    };
    let unit = &cx.unit;
    let text = format!(" {{{eol}{indent}{unit}{line}{eol}{indent}}}");
    Ok(Plan::Patches(vec![Patch::insert(at, text)]))
}

/// Removes `tag` from the element: from its tags literal, the literal
/// when it holds only that tag, the `tags` line when it holds only that
/// literal, and the element's block when it holds only that line.
fn plan_remove_tag(cx: &Ctx<'_>, element: &str, tag: &str) -> Res<Plan> {
    let e = cx.model.element(find_element(cx.model, element)?);
    let owner = cx.statement(e.span).ok_or_else(|| gone(element))?;
    let Some(literal) = e
        .declared_tags
        .iter()
        .find(|l| l.value.split(',').any(|t| t.trim() == tag))
    else {
        return Ok(Plan::Patches(Vec::new()));
    };
    let rest: Vec<&str> = literal
        .value
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != tag)
        .collect();
    if !rest.is_empty() {
        // DECISION: the remaining tags are written joined by "," with no
        // spaces; `model` trims them either way.
        return Ok(Plan::Patches(vec![Patch {
            range: literal.span.range(),
            text: text::quote(&rest.join(",")),
        }]));
    }
    let head = cx.head(owner);
    if let Some(k) = head.iter().position(|&i| cx.tree.span(i) == literal.span) {
        // Positional tags literal.
        return Ok(Plan::Patches(vec![if k + 1 == head.len() {
            Patch::remove(cx.token(head[k - 1]).end as usize..literal.span.end as usize)
        } else {
            Patch {
                range: literal.span.range(),
                text: "\"\"".to_owned(),
            }
        }]));
    }
    let Some(block) = &owner.block else {
        return Ok(Plan::Patches(Vec::new()));
    };
    let Some(line) = Ctx::child_holding(block, literal.span) else {
        return Ok(Plan::Patches(Vec::new()));
    };
    let line_head = cx.head(line);
    if line_head.len() > 2 {
        // `tags "a" "b"`: only this literal goes.
        let k = line_head
            .iter()
            .position(|&i| cx.tree.span(i) == literal.span)
            .unwrap_or(1);
        let from = cx.token(line_head[k - 1]).end as usize;
        return Ok(Plan::Patches(vec![Patch::remove(from..literal.span.end as usize)]));
    }
    if cx.holds_only(block, line) {
        let from = head.last().map_or(owner.span.start, |&i| cx.token(i).end) as usize;
        let to = cx.token(block.close).end as usize;
        return Ok(Plan::Patches(vec![Patch::remove(from..to)]));
    }
    Ok(Plan::Patches(vec![cx.remove_line(line.span)]))
}

// ---------------------------------------------------------- default view

fn add_default_view(current: &mut Parsed, scope: &str, kind: ViewKind) -> Res<Vec<Edit>> {
    let model = &current.model;
    let id = find_element(model, scope)?;
    let fits = match kind {
        ViewKind::SystemContext | ViewKind::Container => {
            model.element(id).kind == ElementKind::SoftwareSystem
        }
        ViewKind::Component => model.element(id).kind == ElementKind::Container,
        // EDIT-15 default views are context, container and component
        // views only: a landscape, filtered, custom or image view has no
        // scope element to default from, and dynamic and deployment views
        // (drawn since phase 3) are no EDIT-15 default.
        ViewKind::SystemLandscape
        | ViewKind::Filtered
        | ViewKind::Custom
        | ViewKind::Image
        | ViewKind::Dynamic
        | ViewKind::Deployment => false,
    };
    if !fits {
        return Err(EditErrorKind::InvalidValue {
            value: scope.to_owned(),
            reason: "this element cannot be the scope of that view kind",
        });
    }
    let key = format!("{scope}-{}", kind.keyword());
    if let Some(view) = model.view(&key) {
        if view.kind == kind && view.scope.element() == Some(id) {
            // DECISION: the same view (key, scope, kind) is already written,
            // e.g. a redo after an undo whose `RemoveDefaultView` was refused
            // because the view held more: a no-op with no inverse, so the
            // batch's later edits still apply (EDIT-19).
            return Ok(Vec::new());
        }
        // DECISION: another view holds the default key; refused rather than
        // adding a second view with the same key (model rejects duplicates).
        return Err(EditErrorKind::InvalidValue {
            value: key,
            reason: "another view already has this key",
        });
    }
    let lines: Lines = vec![
        (0, format!("{} {scope} {} {{", kind.keyword(), text::quote(&key))),
        (1, "include *".to_owned()),
        (0, "}".to_owned()),
    ];
    run(current, |cx| plan_add_view(cx, &lines))?;
    Ok(vec![Edit::RemoveDefaultView {
        scope: scope.to_owned(),
        kind,
    }])
}

/// `RemoveDefaultView` (EDIT-15, EDIT-19): removes the view `<scope>-<kind>`
/// while it holds nothing but what `edit` wrote, and `views` with it when
/// the view was all it held.
// DECISION: "what `edit` wrote" in a default view is read from the text:
// a head of keyword, scope and key, one `include *` line, `at` and `size`
// lines, no comment. `title`/`subtitle` lines are refused although `edit`
// can write them: they may be the author's, and the approved change
// (docs/deferred.md) lists `include *`, `at` and `size` only.
// DECISION: the `views` block is removed whenever the view was all it
// held, since the text does not say who created it; a file whose `views`
// block was empty before `AddDefaultView` gets the same model back, not
// the same bytes.
fn remove_default_view(current: &mut Parsed, scope: &str, kind: ViewKind) -> Res<Vec<Edit>> {
    let key = format!("{scope}-{}", kind.keyword());
    let model = &current.model;
    let view = model.view(&key).ok_or_else(|| gone(&key))?;
    let id = find_element(model, scope)?;
    if view.kind != kind || view.scope.element() != Some(id) {
        return Err(EditErrorKind::InvalidValue {
            value: key,
            reason: "this view is not the default view of that scope and kind",
        });
    }
    let mut inverse = vec![Edit::AddDefaultView {
        scope: scope.to_owned(),
        kind,
    }];
    inverse.extend(view.positions.iter().map(|a| {
        LineKind::At.edit(
            &key,
            &element_key(model, a.element.value),
            Some((a.x.value, a.y.value)),
        )
    }));
    inverse.extend(view.sizes.iter().map(|z| {
        LineKind::Size.edit(
            &key,
            &element_key(model, z.element.value),
            Some((z.width.value, z.height.value)),
        )
    }));
    run(current, |cx| {
        let spec = cx.model.view(&key).ok_or_else(|| gone(&key))?;
        let owner = cx.statement(spec.span).ok_or_else(|| gone(&key))?;
        if !cx.holds_only_default_view(owner) {
            return Err(EditErrorKind::InvalidValue {
                value: key.clone(),
                reason: "the view holds more than the default view; remove it in the file",
            });
        }
        Ok(Plan::Patches(vec![
            cx.remove_nested(owner, &[cx.model.views_span]),
        ]))
    })?;
    Ok(inverse)
}

/// Appends a view as the last view in `views` (before a `styles` block
/// that follows the views), creating `views` when missing (EDIT-15).
fn plan_add_view(cx: &Ctx<'_>, lines: &Lines) -> Res<Plan> {
    let Some(views) = cx.model.views_span.and_then(|s| cx.statement(s)) else {
        return cx.insert_in_views(lines.clone());
    };
    let Some(block) = &views.block else {
        return cx.insert_in_views(lines.clone());
    };
    let is = |s: &&Statement, words: &[&str]| {
        cx.keyword(s)
            .is_some_and(|k| words.iter().any(|w| k.eq_ignore_ascii_case(w)))
    };
    let kinds = [
        ViewKind::SystemContext.keyword(),
        ViewKind::Container.keyword(),
        ViewKind::Component.keyword(),
    ];
    if let Some(last) = block.statements.iter().rev().find(|s| is(s, &kinds)) {
        return Ok(cx.insert_after(views, block, last, lines));
    }
    if let Some(styles) = block.statements.iter().find(|s| is(s, &["styles"])) {
        return Ok(cx.insert_before(views, block, styles, lines));
    }
    Ok(cx.insert_last(views, block, lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test data: roles only (DSL rule), small enough to read in a diff.
    const BASE: &str = "\
workspace {
    model {
        user = person \"User\"
        sys = softwareSystem \"System\" {
            api = container \"API\" \"Serves requests\" \"Rust\"
            db = container \"Store\"
        }
        user -> api \"Uses\"
    }
    views {
        container sys \"c2\" {
            include *
            at user 100 100
            autoLayout
        }
    }
}
";

    fn parse(source: &str) -> Parsed {
        model::parse(source).unwrap_or_else(|e| panic!("{e}\n{source}"))
    }

    fn ok(source: &str, edits: &[Edit]) -> Applied {
        let applied = apply(&parse(source), edits).unwrap_or_else(|e| panic!("{e}"));
        // Every output parses.
        parse(&applied.text);
        applied
    }

    fn fail(source: &str, edits: &[Edit]) -> EditError {
        match apply(&parse(source), edits) {
            Ok(a) => panic!("expected an error, got:\n{}", a.text),
            Err(e) => e,
        }
    }

    /// Applies `edits`, then their inverse, and returns both texts.
    fn round_trip(source: &str, edits: &[Edit]) -> (String, String) {
        let applied = ok(source, edits);
        let back = ok(&applied.text, &applied.inverse);
        (applied.text, back.text)
    }

    /// The differing middle of `a` and `b` after removing the common
    /// prefix and suffix: (removed from `a`, inserted in `b`).
    fn diff(a: &str, b: &str) -> (String, String) {
        let p = a
            .bytes()
            .zip(b.bytes())
            .take_while(|(x, y)| x == y)
            .count();
        let (ra, rb) = (&a.as_bytes()[p..], &b.as_bytes()[p..]);
        let s = ra
            .iter()
            .rev()
            .zip(rb.iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        (
            String::from_utf8_lossy(&ra[..ra.len() - s]).into_owned(),
            String::from_utf8_lossy(&rb[..rb.len() - s]).into_owned(),
        )
    }

    fn mv(view: &str, element: &str, x: i32, y: i32) -> Edit {
        Edit::Move {
            view: view.into(),
            element: element.into(),
            centre: Some((x, y)),
        }
    }

    fn resize(view: &str, element: &str, w: i32, h: i32) -> Edit {
        Edit::Resize {
            view: view.into(),
            element: element.into(),
            size: Some((w, h)),
        }
    }

    fn set_text(target: TextTarget, value: &str) -> Edit {
        Edit::SetText {
            target,
            value: Some(value.into()),
            expected: None,
        }
    }

    fn clear_text(target: TextTarget) -> Edit {
        Edit::SetText {
            target,
            value: None,
            expected: None,
        }
    }

    fn el_field(element: &str, field: ElementField) -> TextTarget {
        TextTarget::Element {
            element: element.into(),
            field,
        }
    }

    fn rel_field(source: &str, destination: &str, ordinal: u32, field: RelationshipField) -> TextTarget {
        TextTarget::Relationship {
            relationship: RelationshipRef::new(source, destination, ordinal),
            field,
        }
    }

    fn view_field(view: &str, field: ViewField) -> TextTarget {
        TextTarget::View {
            view: view.into(),
            field,
        }
    }

    /// `BASE` with a shared tag and a styles block.
    fn styled() -> String {
        BASE.replace(
            "\"Serves requests\" \"Rust\"",
            "\"Serves requests\" \"Rust\" \"Service\"",
        )
        .replace(
            "db = container \"Store\"",
            "db = container \"Store\" {\n                tags \"Service\"\n            }",
        )
        .replace(
            "            autoLayout\n        }\n",
            "            autoLayout\n        }\n        styles {\n            element \"Service\" {\n                background #aa0000\n            }\n        }\n",
        )
    }

    /// A model-only file: no `views` block (EDIT-15, DSL-13).
    const MODEL_ONLY: &str = "\
workspace {
    model {
        user = person \"User\"
        sys = softwareSystem \"System\" {
            api = container \"API\"
        }
        user -> api \"Uses\"
    }
}
";

    fn style(element: &str, target: StyleTarget, property: StyleProperty) -> Edit {
        Edit::SetStyle {
            element: element.into(),
            target,
            property,
        }
    }

    fn tag(name: &str) -> StyleTarget {
        StyleTarget::Tag(name.into())
    }

    fn style_of(source: &str, element: &str) -> model::ResolvedStyle {
        let model = parse(source).model;
        model.style_of(model.find(element).unwrap())
    }

    const RED: Colour = Colour { r: 0xaa, g: 0, b: 0 };
    const BLUE: Colour = Colour { r: 0, g: 0, b: 0xbb };
    const GREEN: Colour = Colour { r: 0, g: 0xcc, b: 0 };

    // One test per requirement.

    #[test]
    fn dsl_7_at_most_one_at_and_size_line_per_box() {
        // A move or resize of a box that already has a line rewrites it,
        // never adds a second one.
        let first = ok(BASE, &[mv("c2", "user", 300, 200), resize("c2", "user", 240, 160)]);
        let second = ok(
            &first.text,
            &[mv("c2", "user", 400, 220), resize("c2", "user", 260, 180)],
        );
        assert_eq!(second.text.matches("at user").count(), 1);
        assert_eq!(second.text.matches("size user").count(), 1);
        assert!(second.text.contains("at user 400 220\n"));
        assert!(second.text.contains("size user 260 180\n"));
        let view = parse(&second.text).model.views[0].clone();
        assert_eq!((view.positions.len(), view.sizes.len()), (1, 1));
    }

    #[test]
    fn dsl_9_move_rewrites_only_at_line() {
        // Byte diff of input vs output is exactly the target line.
        let applied = ok(BASE, &[mv("c2", "user", 340, 120)]);
        assert_eq!(
            applied.text,
            BASE.replace("at user 100 100", "at user 340 120")
        );
        let (removed, inserted) = diff(BASE, &applied.text);
        assert!("at user 100 100".contains(&removed), "{removed:?}");
        assert!("at user 340 120".contains(&inserted), "{inserted:?}");
        assert_eq!(
            applied.inverse,
            vec![Edit::Move {
                view: "c2".into(),
                element: "user".into(),
                centre: Some((100, 100)),
            }]
        );
    }

    #[test]
    fn dsl_10_set_name_rewrites_only_literal() {
        let applied = ok(BASE, &[set_text(el_field("api", ElementField::Name), "Gateway")]);
        assert_eq!(
            applied.text,
            BASE.replace("container \"API\"", "container \"Gateway\"")
        );
        let (removed, inserted) = diff(BASE, &applied.text);
        assert_eq!((removed.as_str(), inserted.as_str()), ("API", "Gateway"));
        assert_eq!(
            applied.inverse,
            vec![Edit::SetText {
                target: el_field("api", ElementField::Name),
                value: Some("API".into()),
                expected: Some("Gateway".into()),
            }]
        );
        // A name cannot be made absent.
        let e = fail(BASE, &[clear_text(el_field("api", ElementField::Name))]);
        assert!(matches!(e.kind, EditErrorKind::InvalidValue { .. }));
        // A description line in the block is the literal rewritten.
        let line = BASE.replace(
            "db = container \"Store\"",
            "db = container \"Store\" {\n                description \"Keeps rows\"\n            }",
        );
        let applied = ok(&line, &[set_text(el_field("db", ElementField::Description), "Keeps data")]);
        assert_eq!(applied.text, line.replace("Keeps rows", "Keeps data"));
        // Removing the line and undoing gives the same model: the value
        // comes back as the positional description (Applied docs).
        let (cleared, back) =
            round_trip(&line, &[clear_text(el_field("db", ElementField::Description))]);
        assert!(!cleared.contains("Keeps rows"));
        let model = parse(&back).model;
        let db = model.find("db").unwrap();
        assert_eq!(model.element(db).description.as_ref().unwrap().value, "Keeps rows");
    }

    #[test]
    fn dsl_10_relationship_and_view_title_literals() {
        // Relationship description and technology, view title and
        // subtitle: each rewrites only its literal.
        let source = BASE
            .replace("user -> api \"Uses\"", "user -> api \"Uses\" \"HTTPS\"")
            .replace(
                "            include *\n",
                "            title \"Containers\"\n            subtitle \"Today\"\n            include *\n",
            );
        let cases = [
            (
                rel_field("user", "api", 0, RelationshipField::Description),
                "\"Uses\"",
                "\"Calls\"",
            ),
            (
                rel_field("user", "api", 0, RelationshipField::Technology),
                "\"HTTPS\"",
                "\"gRPC\"",
            ),
            (view_field("c2", ViewField::Title), "\"Containers\"", "\"Parts\""),
            (view_field("c2", ViewField::Subtitle), "\"Today\"", "\"Plan\""),
        ];
        for (target, old, new) in cases {
            let value = new.trim_matches('"');
            let applied = ok(&source, &[set_text(target, value)]);
            assert_eq!(applied.text, source.replacen(old, new, 1));
            assert_eq!(ok(&applied.text, &applied.inverse).text, source);
        }
        // Absent relationship technology: inserted after the description.
        let applied = ok(
            BASE,
            &[set_text(rel_field("user", "api", 0, RelationshipField::Technology), "HTTPS")],
        );
        assert!(applied.text.contains("user -> api \"Uses\" \"HTTPS\"\n"));
        assert_eq!(ok(&applied.text, &applied.inverse).text, BASE);
        // A relationship that does not exist is gone.
        let e = fail(
            BASE,
            &[set_text(rel_field("user", "api", 1, RelationshipField::Description), "x")],
        );
        assert!(matches!(e.kind, EditErrorKind::TargetGone { .. }));
    }

    #[test]
    fn dsl_10_missing_view_title_inserted_as_first_body_line() {
        // A view with no `title` (and one with no `subtitle`): the line
        // is the first line of the view body, indented per DSL-9.
        let applied = ok(BASE, &[set_text(view_field("c2", ViewField::Title), "Containers")]);
        assert_eq!(
            applied.text,
            BASE.replace(
                "\"c2\" {\n",
                "\"c2\" {\n            title \"Containers\"\n"
            )
        );
        assert_eq!(ok(&applied.text, &applied.inverse).text, BASE);
        let applied = ok(
            &applied.text,
            &[set_text(view_field("c2", ViewField::Subtitle), "Today")],
        );
        assert!(applied.text.contains(
            "\"c2\" {\n            subtitle \"Today\"\n            title \"Containers\"\n"
        ));
        // One-line view: split first.
        let one = BASE.replace(
            "{\n            include *\n            at user 100 100\n            autoLayout\n        }",
            "{ include * }",
        );
        let applied = ok(&one, &[set_text(view_field("c2", ViewField::Title), "T")]);
        assert!(applied.text.contains(
            "\"c2\" {\n            title \"T\"\n            include *\n        }\n"
        ));
    }

    #[test]
    fn dsl_11_empty_edit_list_is_identity() {
        // LF and CRLF, with and without a final newline.
        let lf = BASE.to_owned();
        let crlf = BASE.replace('\n', "\r\n");
        for source in [
            lf.clone(),
            lf.trim_end().to_owned(),
            crlf.clone(),
            crlf.trim_end().to_owned(),
        ] {
            let applied = ok(&source, &[]);
            assert_eq!(applied.text, source);
            assert!(applied.inverse.is_empty());
        }
    }

    #[test]
    fn dsl_13_style_change_writes_styles_block() {
        let source = styled();
        // Rewrite in place: only the value changes.
        let applied = ok(&source, &[style("api", tag("Service"), StyleProperty::Background(Some(BLUE)))]);
        assert_eq!(applied.text, source.replace("#aa0000", "#0000bb"));
        assert_eq!(ok(&applied.text, &applied.inverse).text, source);
        // A new property line, last in the tag's block.
        let applied = ok(&source, &[style("db", tag("Service"), StyleProperty::Color(Some(RED)))]);
        assert!(applied.text.contains(
            "            element \"Service\" {\n                background #aa0000\n                color #aa0000\n            }\n"
        ));
        assert_eq!(ok(&applied.text, &applied.inverse).text, source);
        // A new tag block at the end of `styles`.
        let applied = ok(
            &source,
            &[style("api", tag("Container"), StyleProperty::Shape(Some(Shape::Cylinder)))],
        );
        assert!(applied.text.contains(
            "                background #aa0000\n            }\n            element \"Container\" {\n                shape Cylinder\n            }\n        }\n"
        ));
        assert_eq!(style_of(&applied.text, "db").shape, Shape::Cylinder);
        assert_eq!(ok(&applied.text, &applied.inverse).text, source);
        // No `styles` block: created as the last block of `views`.
        let applied = ok(BASE, &[style("user", tag("Element"), StyleProperty::Background(Some(RED)))]);
        assert!(applied.text.contains(
            "            autoLayout\n        }\n        styles {\n            element \"Element\" {\n                background #aa0000\n            }\n        }\n    }\n}\n"
        ));
        assert_eq!(ok(&applied.text, &applied.inverse).text, BASE);
    }

    #[test]
    fn edit_17_multi_box_style_is_one_batch() {
        // SetStyle on three selected boxes in one `apply`: one `Applied`,
        // one inverse list that undoes all three.
        let source = styled();
        let batch: Vec<Edit> = ["user", "api", "db"]
            .iter()
            .map(|e| style(e, StyleTarget::ThisBox, StyleProperty::Background(Some(GREEN))))
            .collect();
        let applied = ok(&source, &batch);
        for e in ["user", "api", "db", "sys"] {
            let expected = if e == "sys" {
                model::ResolvedStyle::default_for(model::ElementKind::SoftwareSystem).background
            } else {
                GREEN
            };
            assert_eq!(style_of(&applied.text, e).background, expected, "{e}");
        }
        assert_eq!(applied.inverse.len(), 3);
        assert_eq!(ok(&applied.text, &applied.inverse).text, source);
    }

    #[test]
    fn edit_18_resize_rewrites_only_size_line() {
        // DSL-7: byte diff is exactly the `size` line; off-grid sizes are
        // written as given.
        let source = BASE.replace(
            "            at user 100 100\n",
            "            at user 100 100\n            size user 200 120\n",
        );
        let applied = ok(&source, &[resize("c2", "user", 250, 130)]);
        assert_eq!(
            applied.text,
            source.replace("size user 200 120", "size user 250 130")
        );
        let (removed, inserted) = diff(&source, &applied.text);
        assert!("size user 200 120".contains(&removed));
        assert!("size user 250 130".contains(&inserted));
        // A size must be > 0.
        let e = fail(&source, &[resize("c2", "user", 0, 120)]);
        assert!(matches!(e.kind, EditErrorKind::InvalidValue { .. }));
        assert_eq!(e.edit, 0);
    }

    #[test]
    fn edit_19_inverse_is_semantic_and_survives_reload() {
        // The inverse is a list of `Edit`s, applied by key to a reloaded
        // text with an unrelated external change.
        let applied = ok(
            BASE,
            &[
                mv("c2", "user", 300, 300),
                set_text(el_field("api", ElementField::Name), "Gateway"),
            ],
        );
        assert_eq!(
            applied.inverse,
            vec![
                Edit::SetText {
                    target: el_field("api", ElementField::Name),
                    value: Some("API".into()),
                    expected: Some("Gateway".into()),
                },
                Edit::Move {
                    view: "c2".into(),
                    element: "user".into(),
                    centre: Some((100, 100)),
                },
            ]
        );
        // External change: a comment and a new element shift every span.
        let reloaded = applied.text.replace(
            "    model {\n",
            "    // edited elsewhere\n    model {\n        auditor = person \"Auditor\"\n",
        );
        let undone = ok(&reloaded, &applied.inverse);
        assert_eq!(
            undone.text,
            BASE.replace(
                "    model {\n",
                "    // edited elsewhere\n    model {\n        auditor = person \"Auditor\"\n",
            )
        );
    }

    #[test]
    fn edit_21_no_element_or_relationship_created_or_deleted() {
        // Every edit kind, applied to a file: element and relationship
        // counts and keys are unchanged.
        let source = styled();
        let identity = |text: &str| {
            let model = parse(text).model;
            let elements: Vec<String> = model.elements.iter().map(|e| dotted_id(&model, e.id)).collect();
            let relationships: Vec<RelationshipRef> = model
                .relationships
                .iter()
                .map(|r| RelationshipRef::of(&model, r.id))
                .collect();
            (elements, relationships)
        };
        let before = identity(&source);
        let edits = [
            mv("c2", "api", 300, 100),
            resize("c2", "api", 240, 160),
            Edit::Move {
                view: "c2".into(),
                element: "user".into(),
                centre: None,
            },
            set_text(el_field("db", ElementField::Technology), "SQL"),
            set_text(rel_field("user", "api", 0, RelationshipField::Technology), "HTTPS"),
            set_text(view_field("c2", ViewField::Title), "Containers"),
            style("api", tag("Service"), StyleProperty::Color(Some(BLUE))),
            style("db", StyleTarget::ThisBox, StyleProperty::Shape(Some(Shape::Cylinder))),
            Edit::ResetStyle { element: "db".into() },
            Edit::AddDefaultView {
                scope: "api".into(),
                kind: ViewKind::Component,
            },
        ];
        for edit in &edits {
            let applied = ok(&source, std::slice::from_ref(edit));
            assert_eq!(identity(&applied.text), before, "{edit:?}");
            assert_eq!(identity(&ok(&applied.text, &applied.inverse).text), before);
        }
        let all = ok(&source, &edits);
        assert_eq!(identity(&all.text), before);
        assert_eq!(identity(&ok(&all.text, &all.inverse).text), before);
    }

    #[test]
    fn edit_21_inverse_never_removes_nonempty_block() {
        // ThisBox SetStyle creates an element's `{ }` block; an external
        // change adds a child element and a nested relationship to it and
        // another style to the `element "<tag>"` block. Applying the
        // inverse removes only the property: the block, the child, the
        // relationship and the other style stay (and the tag, which the
        // other style still needs). ResetStyle then removes the tag and
        // the per-box style; the element's block, the child and the
        // relationship stay.
        let source = BASE.replace(
            "        user -> api \"Uses\"\n",
            "        ext = softwareSystem \"External\"\n        user -> api \"Uses\"\n",
        );
        let applied = ok(&source, &[style("ext", StyleTarget::ThisBox, StyleProperty::Background(Some(BLUE)))]);
        assert!(applied.text.contains(
            "ext = softwareSystem \"External\" {\n            tags \"ext\"\n        }\n"
        ));
        let external = applied
            .text
            .replace(
                "            tags \"ext\"\n",
                "            tags \"ext\"\n            worker = container \"Worker\"\n            worker -> api \"Calls\"\n",
            )
            .replace(
                "                background #0000bb\n",
                "                background #0000bb\n                shape Cylinder\n",
            );
        let undone = ok(&external, &applied.inverse);
        assert!(!undone.text.contains("#0000bb"));
        assert!(undone.text.contains("element \"ext\" {\n                shape Cylinder\n            }"));
        assert!(undone.text.contains(
            "ext = softwareSystem \"External\" {\n            tags \"ext\"\n            worker = container \"Worker\"\n            worker -> api \"Calls\"\n        }"
        ));
        assert_eq!(style_of(&undone.text, "ext").shape, Shape::Cylinder);
        let reset = ok(&undone.text, &[Edit::ResetStyle { element: "ext".into() }]);
        assert!(reset.text.contains(
            "ext = softwareSystem \"External\" {\n            worker = container \"Worker\"\n            worker -> api \"Calls\"\n        }"
        ));
        assert!(!reset.text.contains("\"ext\""));
        let model = parse(&reset.text).model;
        assert!(model.find("ext.worker").is_some());
        assert_eq!(model.relationships.len(), 2);
    }

    #[test]
    fn edit_24_reapply_on_changed_text() {
        // A recorded action (move box X, set label of Y) re-applied to a
        // text changed on disk lands on the same targets.
        let action = [
            mv("c2", "user", 300, 300),
            set_text(rel_field("user", "api", 0, RelationshipField::Description), "Calls"),
            style("api", StyleTarget::ThisBox, StyleProperty::Color(Some(RED))),
        ];
        let changed = BASE
            .replace(
                "        user = person \"User\"\n",
                "        // a comment\n        admin = person \"Admin\"\n        user = person \"User\"\n",
            )
            .replace(
                "        user -> api \"Uses\"\n",
                "        admin -> api \"Configures\"\n        user -> api \"Uses\"\n",
            )
            .replace("at user 100 100", "at user 120 140\n            at admin 400 100");
        let applied = ok(&changed, &action);
        assert!(applied.text.contains("at user 300 300\n            at admin 400 100"));
        assert!(applied.text.contains("admin -> api \"Configures\"\n        user -> api \"Calls\"\n"));
        assert_eq!(style_of(&applied.text, "api").color, RED);
        assert_ne!(style_of(&applied.text, "admin").color, RED);
        // The same action on the original gives the same edits.
        let original = ok(BASE, &action);
        assert_eq!(original.inverse.len(), applied.inverse.len());
        assert!(applied.inverse.contains(&Edit::Move {
            view: "c2".into(),
            element: "user".into(),
            centre: Some((120, 140)),
        }));
    }

    #[test]
    fn edit_24_relationship_ref_of_round_trips() {
        // For every relationship of a model with parallel `a -> b` lines:
        // `RelationshipRef::of` then resolving the ref in a re-parse of
        // the same text gives the same relationship.
        let source = BASE.replace(
            "        user -> api \"Uses\"\n",
            "        user -> api \"Uses\"\n        api -> db \"Reads\"\n        user -> api \"Logs in\"\n        sys -> user \"Notifies\"\n        user -> api \"Pays\"\n",
        );
        let first = parse(&source);
        let again = parse(&source);
        let model = &first.model;
        for r in &model.relationships {
            let reference = RelationshipRef::of(model, r.id);
            let s = again.model.find(&reference.source).unwrap();
            let d = again.model.find(&reference.destination).unwrap();
            let found = again.model.find_relationship(s, d)[reference.ordinal as usize];
            assert_eq!(found, r.id);
            assert_eq!(
                again.model.relationship(found).description,
                r.description
            );
        }
        let pays = model
            .relationships
            .iter()
            .find(|r| r.description.as_ref().is_some_and(|d| d.value == "Pays"))
            .unwrap();
        assert_eq!(
            RelationshipRef::of(model, pays.id),
            RelationshipRef::new("user", "api", 2)
        );
    }

    #[test]
    fn edit_24_drifted_target_reports_conflict() {
        // A relationship with the same ends inserted before the target
        // shifts the ordinal; SetText with `expected` reports `Conflict`
        // and writes nothing.
        let edit = Edit::SetText {
            target: rel_field("user", "api", 0, RelationshipField::Description),
            value: Some("Calls".into()),
            expected: Some("Uses".into()),
        };
        // Unchanged text: the check passes.
        assert!(ok(BASE, std::slice::from_ref(&edit)).text.contains("\"Calls\""));
        let drifted = BASE.replace(
            "        user -> api \"Uses\"\n",
            "        user -> api \"Logs in\"\n        user -> api \"Uses\"\n",
        );
        let e = fail(&drifted, &[edit]);
        assert_eq!(
            e.kind,
            EditErrorKind::Conflict {
                expected: "Uses".into(),
                found: Some("Logs in".into()),
            }
        );
        assert!(e.to_string().contains("Logs in"));
    }

    #[test]
    fn edit_26_batch_of_moves_writes_every_at_line_in_one_step() {
        // A view with no `at` line, a batch of Moves ordered by
        // `ElementId`: one `Applied`, the new lines in batch order, and a
        // gone target in the batch writes nothing.
        let source = BASE.replace("            at user 100 100\n", "");
        let batch = [
            mv("c2", "user", 100, 100),
            mv("c2", "api", 300, 100),
            mv("c2", "db", 300, 300),
        ];
        let applied = ok(&source, &batch);
        assert!(applied.text.contains(
            "            include *\n            autoLayout\n            at user 100 100\n            at api 300 100\n            at db 300 300\n        }"
        ));
        assert_eq!(applied.inverse.len(), 3);
        assert_eq!(ok(&applied.text, &applied.inverse).text, source);
        let mut gone = batch.to_vec();
        gone.insert(1, mv("c2", "nobody", 0, 0));
        let e = fail(&source, &gone);
        assert_eq!(e.edit, 1);
        assert_eq!(
            e.kind,
            EditErrorKind::TargetGone {
                target: "nobody".into()
            }
        );
    }

    // edit-01: move and resize

    #[test]
    fn move_inserts_missing_at_line() {
        // DSL-9: after the last `at` line, else before the closing brace,
        // copying indentation and line ending.
        let applied = ok(BASE, &[mv("c2", "api", 300, 100)]);
        assert_eq!(
            applied.text,
            BASE.replace(
                "            at user 100 100\n",
                "            at user 100 100\n            at api 300 100\n"
            )
        );
        let none = BASE.replace("            at user 100 100\n", "");
        let applied = ok(&none, &[mv("c2", "api", 300, 100)]);
        assert_eq!(
            applied.text,
            none.replace(
                "            autoLayout\n        }",
                "            autoLayout\n            at api 300 100\n        }"
            )
        );
        // A box that is not in the view is gone for this edit.
        let mut outside = BASE.replace("include *", "include api");
        outside = outside.replace("            at user 100 100\n", "");
        let e = fail(&outside, &[mv("c2", "user", 0, 0)]);
        assert!(matches!(e.kind, EditErrorKind::TargetGone { .. }));
    }

    // edit-02: text edits

    #[test]
    fn set_label_escapes_quotes() {
        // DSL-10: `"` and `\` escaped.
        let value = r#"say "hi" \ bye"#;
        let applied = ok(
            BASE,
            &[set_text(rel_field("user", "api", 0, RelationshipField::Description), value)],
        );
        assert!(applied.text.contains(r#"user -> api "say \"hi\" \\ bye""#));
        let model = parse(&applied.text).model;
        assert_eq!(model.relationships[0].description.as_ref().unwrap().value, value);
        // Rewriting an escaped literal: expected compares unescaped values.
        let again = ok(
            &applied.text,
            &[Edit::SetText {
                target: rel_field("user", "api", 0, RelationshipField::Description),
                value: Some("plain".into()),
                expected: Some(value.into()),
            }],
        );
        assert_eq!(again.text, BASE.replace("\"Uses\"", "\"plain\""));
    }

    #[test]
    fn clearing_non_trailing_positional_writes_placeholder() {
        // DSL-10: SetText Description None on `container "n" "d" "t"`
        // writes `""`; the technology stays in its slot.
        let applied = ok(BASE, &[clear_text(el_field("api", ElementField::Description))]);
        assert!(applied.text.contains("api = container \"API\" \"\" \"Rust\"\n"));
        let api = parse(&applied.text).model.find("api").unwrap();
        let model = parse(&applied.text).model;
        assert_eq!(model.element(api).technology.as_ref().unwrap().value, "Rust");
        assert_eq!(ok(&applied.text, &applied.inverse).text, BASE);
        // The trailing positional is removed.
        let applied = ok(BASE, &[clear_text(el_field("api", ElementField::Technology))]);
        assert!(applied.text.contains("api = container \"API\" \"Serves requests\"\n"));
        assert_eq!(ok(&applied.text, &applied.inverse).text, BASE);
    }

    #[test]
    fn technology_on_person_or_system_is_invalid() {
        // DSL-10: `InvalidValue`, nothing written.
        for element in ["user", "sys"] {
            let e = fail(BASE, &[set_text(el_field(element, ElementField::Technology), "Rust")]);
            assert!(matches!(e.kind, EditErrorKind::InvalidValue { .. }), "{e:?}");
            let e = fail(BASE, &[clear_text(el_field(element, ElementField::Technology))]);
            assert!(matches!(e.kind, EditErrorKind::InvalidValue { .. }), "{e:?}");
        }
    }

    // edit-03: style edits

    #[test]
    fn dsl_13_shared_tag_gets_unique_tag() {
        // ThisBox on an element whose tag is shared: only it changes.
        // `all <tag>` on the same shared tag: every element carrying it
        // changes, as asked.
        let source = styled();
        let applied = ok(&source, &[style("api", StyleTarget::ThisBox, StyleProperty::Background(Some(BLUE)))]);
        assert_eq!(style_of(&applied.text, "api").background, BLUE);
        assert_eq!(style_of(&applied.text, "db").background, RED);
        assert!(applied.text.contains("element \"Service\" {\n                background #aa0000\n"));
        let all = ok(&source, &[style("api", tag("Service"), StyleProperty::Color(Some(BLUE)))]);
        assert_eq!(style_of(&all.text, "api").color, BLUE);
        assert_eq!(style_of(&all.text, "db").color, BLUE);
    }

    #[test]
    fn style_tag_not_carried_is_invalid() {
        // DSL-13: `StyleTarget::Tag` with a tag the element lacks is
        // `InvalidValue`; a tag overridden later in the cascade (DSL-15)
        // is written all the same.
        let source = styled();
        let e = fail(&source, &[style("user", tag("Service"), StyleProperty::Background(Some(BLUE)))]);
        assert_eq!(
            e.kind,
            EditErrorKind::InvalidValue {
                value: "Service".into(),
                reason: "the element does not carry this tag",
            }
        );
        let boxed = ok(&source, &[style("api", StyleTarget::ThisBox, StyleProperty::Background(Some(BLUE)))]);
        let shadowed = ok(&boxed.text, &[style("api", tag("Service"), StyleProperty::Background(Some(GREEN)))]);
        assert!(shadowed.text.contains("background #00cc00"));
        assert_eq!(style_of(&shadowed.text, "api").background, BLUE);
        assert_eq!(style_of(&shadowed.text, "db").background, GREEN);
    }

    // edit-04: inverse and re-apply

    #[test]
    fn inverse_edit_restores_bytes() {
        // EDIT-19, nothing changed in between. One case per variant:
        // Move rewrite and Move insert at the canonical spot (bytes);
        // Move None on a middle `at` line (same model, line re-added
        // last); Resize likewise; SetText rewrite and insert with
        // placeholders (bytes); SetStyle Tag and ThisBox (bytes);
        // ResetStyle (same model, block and tag re-added at the end).
        let source = styled().replace(
            "            at user 100 100\n",
            "            at user 100 100\n            at api 300 100\n            at db 300 300\n            size user 200 100\n            size api 240 160\n            size db 200 100\n",
        );
        let bytes = [
            mv("c2", "api", 320, 120),
            resize("c2", "api", 260, 180),
            set_text(el_field("api", ElementField::Name), "Gateway"),
            set_text(rel_field("user", "api", 0, RelationshipField::Technology), "HTTPS"),
            set_text(el_field("db", ElementField::Technology), "SQL"),
            set_text(view_field("c2", ViewField::Subtitle), "Today"),
            style("api", tag("Service"), StyleProperty::Background(Some(BLUE))),
            style("api", StyleTarget::ThisBox, StyleProperty::Background(Some(BLUE))),
        ];
        for edit in bytes {
            let (_, back) = round_trip(&source, std::slice::from_ref(&edit));
            assert_eq!(back, source, "{edit:?}");
        }
        let insert = source.replace("            at api 300 100\n", "");
        let (_, back) = round_trip(&insert, &[mv("c2", "api", 300, 100)]);
        assert_eq!(back, insert);
        // Same model, not the same bytes.
        let same_model = |a: &str, b: &str| {
            let (a, b) = (parse(a).model, parse(b).model);
            let boxes = |m: &model::Model| {
                let v = &m.views[0];
                let mut at: Vec<_> = v.positions.iter().map(|p| (p.element.value, p.x.value, p.y.value)).collect();
                let mut size: Vec<_> = v.sizes.iter().map(|s| (s.element.value, s.width.value, s.height.value)).collect();
                at.sort_unstable();
                size.sort_unstable();
                (at, size)
            };
            assert_eq!(boxes(&a), boxes(&b));
            for e in &a.elements {
                assert_eq!(a.style_of(e.id), b.style_of(e.id));
            }
        };
        let moved = [
            Edit::Move {
                view: "c2".into(),
                element: "api".into(),
                centre: None,
            },
            Edit::Resize {
                view: "c2".into(),
                element: "api".into(),
                size: None,
            },
        ];
        for edit in moved {
            let (cleared, back) = round_trip(&source, std::slice::from_ref(&edit));
            assert_ne!(cleared, source);
            same_model(&back, &source);
        }
        let (_, back) = round_trip(&source, &[mv("c2", "api", 0, 0)]);
        assert_eq!(back, source);
        let boxed = ok(&source, &[style("db", StyleTarget::ThisBox, StyleProperty::Shape(Some(Shape::Cylinder)))]);
        let (_, back) = round_trip(&boxed.text, &[Edit::ResetStyle { element: "db".into() }]);
        same_model(&back, &boxed.text);
    }

    #[test]
    fn edit_24_target_gone_reported() {
        let removed = BASE
            .replace("            db = container \"Store\"\n", "")
            .replace("container sys \"c2\"", "container sys \"c2-new\"");
        let cases = [
            (mv("c2", "user", 0, 0), "c2"),
            (set_text(el_field("db", ElementField::Name), "x"), "db"),
            (style("db", tag("Container"), StyleProperty::Background(Some(RED))), "db"),
            (Edit::ResetStyle { element: "db".into() }, "db"),
            (
                set_text(rel_field("user", "db", 0, RelationshipField::Description), "x"),
                "db",
            ),
            (set_text(view_field("c2", ViewField::Title), "x"), "c2"),
        ];
        for (edit, target) in cases {
            let e = fail(&removed, &[edit]);
            assert_eq!(
                e.kind,
                EditErrorKind::TargetGone {
                    target: target.into()
                }
            );
            assert!(e.to_string().contains(target));
        }
    }

    // Review Focus

    #[test]
    fn newline_in_text_is_refused() {
        // DSL-10: `\n`, `\r\n` and a lone `\r` are `Newline`; another
        // control character is `InvalidValue`.
        for value in ["a\nb", "a\r\nb", "a\rb"] {
            let e = fail(BASE, &[set_text(el_field("api", ElementField::Name), value)]);
            assert_eq!(e.kind, EditErrorKind::Newline);
        }
        let e = fail(BASE, &[set_text(el_field("api", ElementField::Name), "a\u{7}b")]);
        assert!(matches!(e.kind, EditErrorKind::InvalidValue { .. }));
        // A tab is fine.
        let applied = ok(BASE, &[set_text(el_field("api", ElementField::Name), "a\tb")]);
        assert!(applied.text.contains("container \"a\tb\""));
    }

    #[test]
    fn absent_literal_inserted_with_placeholders() {
        // DSL-10: a technology on a container with no description gets
        // `""` for the description.
        let applied = ok(BASE, &[set_text(el_field("db", ElementField::Technology), "SQL")]);
        assert!(applied.text.contains("db = container \"Store\" \"\" \"SQL\"\n"));
        // The inverse takes the placeholder out too.
        assert_eq!(ok(&applied.text, &applied.inverse).text, BASE);
        let applied = ok(BASE, &[set_text(el_field("user", ElementField::Description), "A role")]);
        assert!(applied.text.contains("user = person \"User\" \"A role\"\n"));
        assert_eq!(ok(&applied.text, &applied.inverse).text, BASE);
    }

    #[test]
    fn reapply_after_rename_reports_target_gone() {
        // EDIT-24: never patches the wrong span.
        let renamed = BASE.replace("api = container", "gateway = container").replace(
            "user -> api",
            "user -> gateway",
        );
        let e = fail(
            &renamed,
            &[
                mv("c2", "user", 300, 300),
                set_text(el_field("api", ElementField::Name), "Gateway"),
            ],
        );
        assert_eq!(e.edit, 1);
        assert_eq!(
            e.kind,
            EditErrorKind::TargetGone {
                target: "api".into()
            }
        );
        let e = fail(
            &renamed,
            &[set_text(rel_field("user", "api", 0, RelationshipField::Description), "x")],
        );
        assert!(matches!(e.kind, EditErrorKind::TargetGone { .. }));
    }

    #[test]
    fn insert_at_line_into_one_line_view() {
        // DSL-9: `{ include * }` is split onto lines; the new line gets
        // the view's indentation plus one indent unit.
        let source = BASE.replace(
            "{\n            include *\n            at user 100 100\n            autoLayout\n        }",
            "{ include * }",
        );
        let applied = ok(&source, &[mv("c2", "api", 300, 100)]);
        assert!(
            applied.text.contains(
                "        container sys \"c2\" {\n            include *\n            at api 300 100\n        }\n"
            ),
            "{}",
            applied.text
        );
        // Tab-indented file: the unit is the file's.
        let tabs = "workspace {\n\tmodel {\n\t\ta = softwareSystem \"A\"\n\t}\n\tviews {\n\t\tsystemContext a \"c1\" { include * }\n\t}\n}\n";
        let applied = ok(tabs, &[mv("c1", "a", 100, 100)]);
        assert!(applied.text.contains(
            "\t\tsystemContext a \"c1\" {\n\t\t\tinclude *\n\t\t\tat a 100 100\n\t\t}\n"
        ));
    }

    #[test]
    fn dsl_9_at_line_after_view_head_gets_body_indentation() {
        // A view whose body has no `at` line: the new line takes the first
        // body line's indentation, else the view's plus four spaces.
        let source = BASE.replace(
            "            include *\n            at user 100 100\n            autoLayout\n",
            "              include *\n",
        );
        let applied = ok(&source, &[mv("c2", "api", 300, 100)]);
        assert!(applied.text.contains(
            "              include *\n              at api 300 100\n        }"
        ));
        // The line above is the view head, and no body line starts its
        // own line: the view's indentation plus the file's unit.
        let head = BASE.replace(
            "{\n            include *\n            at user 100 100\n            autoLayout\n        }",
            "{ include *\n        }",
        );
        let applied = ok(&head, &[mv("c2", "api", 300, 100)]);
        assert!(
            applied
                .text
                .contains("\"c2\" { include *\n            at api 300 100\n        }"),
            "{}",
            applied.text
        );
    }

    #[test]
    fn dsl_9_new_size_line_after_last_size_else_last_at() {
        // DSL-9, EDIT-18: after the last `size` line; with none, after the
        // last `at` line; with neither, before the closing brace.
        let after_at = ok(BASE, &[resize("c2", "api", 240, 160)]);
        assert!(after_at.text.contains(
            "            at user 100 100\n            size api 240 160\n            autoLayout\n"
        ));
        let after_size = ok(&after_at.text, &[resize("c2", "db", 200, 120)]);
        assert!(after_size.text.contains(
            "            size api 240 160\n            size db 200 120\n            autoLayout\n"
        ));
        let none = BASE.replace("            at user 100 100\n", "");
        let applied = ok(&none, &[resize("c2", "db", 200, 120)]);
        assert!(
            applied
                .text
                .contains("            autoLayout\n            size db 200 120\n        }")
        );
    }

    #[test]
    fn dsl_13_this_box_tag_is_dotted_id_with_suffix() {
        // The tag is the full dotted id; `-2`, `-3` when another element
        // already carries it; the `tags` line is last in the block.
        let source = styled();
        let applied = ok(&source, &[style("api", StyleTarget::ThisBox, StyleProperty::Shape(Some(Shape::Cylinder)))]);
        assert!(applied.text.contains(
            "api = container \"API\" \"Serves requests\" \"Rust\" \"Service\" {\n                tags \"sys.api\"\n            }\n"
        ));
        assert!(applied.text.contains(
            "            element \"sys.api\" {\n                shape Cylinder\n            }\n        }\n"
        ));
        // `db` has a block: the tags line is its last line. A top-level
        // element's tag is its bare id.
        let applied = ok(
            &source,
            &[
                style("db", StyleTarget::ThisBox, StyleProperty::Color(Some(RED))),
                style("user", StyleTarget::ThisBox, StyleProperty::Color(Some(RED))),
            ],
        );
        assert!(applied.text.contains(
            "db = container \"Store\" {\n                tags \"Service\"\n                tags \"sys.db\"\n            }"
        ));
        assert!(applied.text.contains("user = person \"User\" {\n            tags \"user\"\n        }\n"));
        // Taken by another element: suffixed.
        let taken = source.replace("\"Store\" {\n                tags \"Service\"", "\"Store\" {\n                tags \"Service,sys.api,sys.api-2\"");
        let applied = ok(&taken, &[style("api", StyleTarget::ThisBox, StyleProperty::Color(Some(RED)))]);
        assert!(applied.text.contains("tags \"sys.api-3\""));
        assert_eq!(style_of(&applied.text, "api").color, RED);
        assert_ne!(style_of(&applied.text, "db").color, RED);
        // A second change reuses the box's tag.
        let again = ok(&applied.text, &[style("api", StyleTarget::ThisBox, StyleProperty::Background(Some(BLUE)))]);
        assert_eq!(again.text.matches("sys.api-3").count(), 2);
        assert!(again.text.contains("element \"sys.api-3\" {\n                color #aa0000\n                background #0000bb\n"));
    }

    #[test]
    fn edit_15_default_view_written_on_first_write() {
        // AddDefaultView then a Move in one batch: the view (key
        // `<scope id>-<kind>`, `include *`, no `at` lines from it) is the
        // last view in `views`, written first. The inverse part is in
        // `edit_15_default_view_inverse_removes_it`.
        let source = styled();
        let applied = ok(
            &source,
            &[
                Edit::AddDefaultView {
                    scope: "sys".into(),
                    kind: ViewKind::Container,
                },
                mv("sys-container", "db", 200, 200),
            ],
        );
        assert!(applied.text.contains(
            "            autoLayout\n        }\n        container sys \"sys-container\" {\n            include *\n            at db 200 200\n        }\n        styles {\n"
        ));
        let model = parse(&applied.text).model;
        assert_eq!(model.views.last().unwrap().key, "sys-container");
        // The same view (key, scope and kind) already exists: written
        // already, nothing to add (EDIT-19 redo after an undo that keeps it).
        let again = ok(
            &applied.text,
            &[Edit::AddDefaultView {
                scope: "sys".into(),
                kind: ViewKind::Container,
            }],
        );
        assert_eq!(again.text, applied.text);
        assert!(again.inverse.is_empty());
        // A view of another kind holds that key: refused.
        let clash = source.replace(
            "    views {\n",
            "    views {\n        systemContext sys \"sys-container\" {\n            include *\n        }\n",
        );
        let e = fail(
            &clash,
            &[Edit::AddDefaultView {
                scope: "sys".into(),
                kind: ViewKind::Container,
            }],
        );
        assert!(matches!(e.kind, EditErrorKind::InvalidValue { .. }));
        // Without `styles`, the view is the last block of `views`.
        let applied = ok(
            BASE,
            &[Edit::AddDefaultView {
                scope: "sys".into(),
                kind: ViewKind::SystemContext,
            }],
        );
        assert!(applied.text.ends_with(
            "            autoLayout\n        }\n        systemContext sys \"sys-systemContext\" {\n            include *\n        }\n    }\n}\n"
        ));
    }

    #[test]
    fn edit_19_redo_after_undo_of_default_view_batch() {
        // EDIT-19: apply, undo, redo. The undo removes the view
        // (`RemoveDefaultView`) and the redo writes it and the batch's
        // later edits again.
        for second in [
            mv("sys-systemContext", "user", 10, 10),
            style("user", StyleTarget::ThisBox, StyleProperty::Shape(Some(Shape::Cylinder))),
        ] {
            let batch = [
                Edit::AddDefaultView {
                    scope: "sys".into(),
                    kind: ViewKind::SystemContext,
                },
                second,
            ];
            let first = ok(MODEL_ONLY, &batch);
            let undone = ok(&first.text, &first.inverse);
            let redone = ok(&undone.text, &batch);
            assert_eq!(redone.text, first.text);
        }
    }

    #[test]
    fn edit_15_model_only_file_default_view_and_move() {
        // A file with no `views` block: AddDefaultView (Container) then a
        // Move in one batch creates `views` as the last block of the
        // workspace body, indented per DSL-9, holding the new view with
        // its `at` line. The inverse part is in
        // `edit_15_default_view_inverse_removes_it`.
        let applied = ok(
            MODEL_ONLY,
            &[
                Edit::AddDefaultView {
                    scope: "sys".into(),
                    kind: ViewKind::Container,
                },
                mv("sys-container", "api", 200, 100),
            ],
        );
        assert_eq!(
            applied.text,
            MODEL_ONLY.replace(
                "    }\n}\n",
                "    }\n    views {\n        container sys \"sys-container\" {\n            include *\n            at api 200 100\n        }\n    }\n}\n"
            )
        );
    }

    #[test]
    fn dsl_13_model_only_file_set_style_creates_views_and_styles() {
        // A file with no `views` block: SetStyle (Tag and ThisBox) creates
        // `views` (last block of the workspace body) holding `styles`
        // holding the `element "<tag>"` block; the inverse gives the input
        // bytes back.
        let applied = ok(MODEL_ONLY, &[style("api", tag("Container"), StyleProperty::Background(Some(RED)))]);
        assert_eq!(
            applied.text,
            MODEL_ONLY.replace(
                "    }\n}\n",
                "    }\n    views {\n        styles {\n            element \"Container\" {\n                background #aa0000\n            }\n        }\n    }\n}\n"
            )
        );
        assert_eq!(ok(&applied.text, &applied.inverse).text, MODEL_ONLY);
        let applied = ok(MODEL_ONLY, &[style("user", StyleTarget::ThisBox, StyleProperty::Shape(Some(Shape::Cylinder)))]);
        assert!(applied.text.contains("user = person \"User\" {\n            tags \"user\"\n        }\n"));
        assert!(applied.text.ends_with(
            "    views {\n        styles {\n            element \"user\" {\n                shape Cylinder\n            }\n        }\n    }\n}\n"
        ));
        assert_eq!(style_of(&applied.text, "user").shape, Shape::Cylinder);
        assert_eq!(ok(&applied.text, &applied.inverse).text, MODEL_ONLY);
    }

    #[test]
    fn edit_15_inverse_keeps_views_block_holding_more() {
        // A `views` block created by `edit`, then an external change adds
        // a view to it: the inverse removes only the default view, the
        // `views` block stays.
        let applied = ok(
            MODEL_ONLY,
            &[Edit::AddDefaultView {
                scope: "sys".into(),
                kind: ViewKind::Container,
            }],
        );
        let external = applied.text.replace(
            "    views {\n",
            "    views {\n        systemContext sys \"c1\" {\n            include *\n        }\n",
        );
        let undone = ok(&external, &applied.inverse);
        assert!(!undone.text.contains("sys-container"));
        assert!(undone.text.contains(
            "    views {\n        systemContext sys \"c1\" {\n            include *\n        }\n    }\n"
        ));
    }

    #[test]
    fn edit_15_default_view_inverse_removes_it() {
        // EDIT-15, EDIT-19: undoing the first write in a default view
        // removes the view, and the `views` block `edit` created for it.
        let source = styled();
        let (_, back) = round_trip(
            &source,
            &[
                Edit::AddDefaultView {
                    scope: "sys".into(),
                    kind: ViewKind::Container,
                },
                mv("sys-container", "db", 200, 200),
            ],
        );
        assert_eq!(back, source);
        let (_, back) = round_trip(
            MODEL_ONLY,
            &[
                Edit::AddDefaultView {
                    scope: "sys".into(),
                    kind: ViewKind::Container,
                },
                mv("sys-container", "api", 200, 100),
            ],
        );
        assert_eq!(back, MODEL_ONLY);
    }

    fn remove_default(scope: &str, kind: ViewKind) -> Edit {
        Edit::RemoveDefaultView {
            scope: scope.into(),
            kind,
        }
    }

    #[test]
    fn edit_19_inverse_of_add_default_view_is_remove_default_view() {
        let applied = ok(
            MODEL_ONLY,
            &[
                Edit::AddDefaultView {
                    scope: "sys".into(),
                    kind: ViewKind::Container,
                },
                mv("sys-container", "api", 200, 100),
            ],
        );
        assert_eq!(
            applied.inverse.last(),
            Some(&remove_default("sys", ViewKind::Container))
        );
    }

    #[test]
    fn edit_19_remove_default_view_refused_when_view_holds_more() {
        // Anything besides `include *`, `at` and `size` lines (another
        // statement, a comment, a head argument) refuses the removal and
        // nothing is written.
        let added = ok(
            &styled(),
            &[Edit::AddDefaultView {
                scope: "sys".into(),
                kind: ViewKind::Container,
            }],
        )
        .text;
        let open = "        container sys \"sys-container\" {\n            include *\n";
        assert!(added.contains(open));
        for extra in [
            "            autoLayout\n",
            "            exclude user\n",
            "            include user\n",
            "            // mine\n",
            "            title \"Mine\"\n",
        ] {
            let source = added.replace(open, &format!("{open}{extra}"));
            let e = fail(&source, &[remove_default("sys", ViewKind::Container)]);
            assert!(
                matches!(e.kind, EditErrorKind::InvalidValue { .. }),
                "{extra:?}"
            );
        }
        let described = added.replace(
            "container sys \"sys-container\" {",
            "container sys \"sys-container\" \"Mine\" {",
        );
        let e = fail(&described, &[remove_default("sys", ViewKind::Container)]);
        assert!(matches!(e.kind, EditErrorKind::InvalidValue { .. }));
        // `at` and `size` lines are what `edit` writes: removed with the
        // view, and given back by its inverse.
        let placed = ok(
            &added,
            &[
                mv("sys-container", "api", 200, 100),
                resize("sys-container", "api", 240, 160),
            ],
        )
        .text;
        let removed = ok(&placed, &[remove_default("sys", ViewKind::Container)]);
        assert_eq!(removed.text, styled());
        assert_eq!(ok(&removed.text, &removed.inverse).text, placed);
    }

    #[test]
    fn edit_19_remove_default_view_target_checks() {
        // No view with that key: the target is gone. A view with that key
        // but another kind or scope is refused.
        let e = fail(BASE, &[remove_default("sys", ViewKind::Container)]);
        assert!(matches!(e.kind, EditErrorKind::TargetGone { .. }));
        let clash = BASE.replace(
            "    views {\n",
            "    views {\n        systemContext sys \"sys-container\" {\n            include *\n        }\n",
        );
        let e = fail(&clash, &[remove_default("sys", ViewKind::Container)]);
        assert!(matches!(e.kind, EditErrorKind::InvalidValue { .. }));
    }

    #[test]
    fn edit_15_system_context_default_view() {
        // AddDefaultView with `ViewKind::SystemContext` on a software
        // system writes `systemContext <id> "<id>-systemContext"` with
        // `include *`; on a model-only file, followed by a first Move in
        // it, the move is written into that view; a container scope is
        // `InvalidValue`.
        let applied = ok(
            MODEL_ONLY,
            &[
                Edit::AddDefaultView {
                    scope: "sys".into(),
                    kind: ViewKind::SystemContext,
                },
                mv("sys-systemContext", "user", 100, 100),
            ],
        );
        assert!(applied.text.contains(
            "    views {\n        systemContext sys \"sys-systemContext\" {\n            include *\n            at user 100 100\n        }\n    }\n"
        ));
        let view = parse(&applied.text).model.views[0].clone();
        assert_eq!(view.kind, ViewKind::SystemContext);
        for (scope, kind) in [
            ("api", ViewKind::SystemContext),
            ("api", ViewKind::Container),
            ("sys", ViewKind::Component),
            ("user", ViewKind::Container),
        ] {
            let e = fail(
                MODEL_ONLY,
                &[Edit::AddDefaultView {
                    scope: scope.into(),
                    kind,
                }],
            );
            assert!(matches!(e.kind, EditErrorKind::InvalidValue { .. }), "{scope} {kind:?}");
        }
        let e = fail(
            MODEL_ONLY,
            &[Edit::AddDefaultView {
                scope: "nobody".into(),
                kind: ViewKind::Container,
            }],
        );
        assert!(matches!(e.kind, EditErrorKind::TargetGone { .. }));
    }

    #[test]
    fn reset_style_removes_tag_and_block() {
        // DSL-13.
        let source = styled();
        let boxed = ok(
            &source,
            &[
                style("api", StyleTarget::ThisBox, StyleProperty::Background(Some(BLUE))),
                style("api", StyleTarget::ThisBox, StyleProperty::Color(Some(GREEN))),
            ],
        );
        let reset = ok(&boxed.text, &[Edit::ResetStyle { element: "api".into() }]);
        assert_eq!(reset.text, source);
        assert_eq!(style_of(&reset.text, "api").background, RED);
        // Its inverse gives the per-box style back.
        let back = ok(&reset.text, &reset.inverse);
        assert_eq!(style_of(&back.text, "api").background, BLUE);
        assert_eq!(style_of(&back.text, "api").color, GREEN);
        // No per-box tag: nothing to do.
        let none = ok(&source, &[Edit::ResetStyle { element: "db".into() }]);
        assert_eq!(none.text, source);
        assert!(none.inverse.is_empty());
        // A block holding a comment keeps it; the properties and the tag go.
        let commented = boxed.text.replace(
            "element \"sys.api\" {\n",
            "element \"sys.api\" {\n                // kept\n",
        );
        let reset = ok(&commented, &[Edit::ResetStyle { element: "api".into() }]);
        assert!(reset.text.contains("element \"sys.api\" {\n                // kept\n            }"));
        assert!(!reset.text.contains("tags \"sys.api\""));
        assert_eq!(style_of(&reset.text, "api").background, RED);
    }

    #[test]
    fn batch_is_atomic() {
        // One gone target in a multi-box batch: nothing written.
        let e = fail(
            BASE,
            &[
                mv("c2", "user", 0, 0),
                mv("c2", "api", 0, 0),
                mv("gone-view", "db", 0, 0),
            ],
        );
        assert_eq!(e.edit, 2);
        assert_eq!(
            e.kind,
            EditErrorKind::TargetGone {
                target: "gone-view".into()
            }
        );
        let text = e.to_string();
        assert!(text.contains("gone-view"), "{text}");
    }

    #[test]
    fn crlf_file_keeps_line_endings() {
        // DSL-9: an inserted line copies `\r\n` from the line above.
        let source = BASE.replace('\n', "\r\n");
        let applied = ok(&source, &[mv("c2", "api", 300, 100), resize("c2", "api", 240, 160)]);
        assert!(applied.text.contains(
            "            at user 100 100\r\n            at api 300 100\r\n            size api 240 160\r\n"
        ));
        assert!(!applied.text.replace("\r\n", "").contains('\n'));
        let split = source.replace(
            "{\r\n            include *\r\n            at user 100 100\r\n            autoLayout\r\n        }",
            "{ include * }",
        );
        let applied = ok(&split, &[mv("c2", "api", 300, 100)]);
        assert!(!applied.text.replace("\r\n", "").contains('\n'));
    }
}
