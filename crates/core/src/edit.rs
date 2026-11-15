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
// element key, which in phase 1 is the bare `model::Element::key` (the
// identifier written before `=`), the same string `model::Model::find`
// resolves. The editor gets it from a hit-tested `ElementId` as
// `model.element(id).key`; for a relationship it uses
// `RelationshipRef::of`. Nothing else is a valid key.
//
// EDIT-21 (product rule, SRS section 2: "it is not draw.io"): there is no
// operation that creates or deletes an element or a relationship, and
// none may be added. The complete list of bytes `apply` may insert or
// remove, which the implementation's guard is built from:
// - `at` and `size` lines in a view;
// - `title` and `subtitle` lines in a view;
// - a default view block (EDIT-15, `Edit::AddDefaultView`): a view, not
//   an element, appended as the last view in `views`;
// - string literals of existing statements (DSL-10), including `""`
//   placeholders for skipped positional arguments;
// - style property lines (`background`, `color`, `shape`) in an
//   `element "<tag>"` block;
// - `element "<tag>"` style blocks and the `styles` block;
// - a per-box tag (DSL-13) on an existing element: the tag in a `tags`
//   line, the `tags` line itself, and the element's `{ }` block when the
//   element had none.
// Removal exists only so an inverse gives the bytes back (EDIT-19), and
// a block (`{ }` of an element, `element "<tag>"`, `styles`) or a `tags`
// line is removed only when it holds nothing but what `edit` wrote there;
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
// `Edit::AddDefaultView`, the first edit of that write's batch.

use std::fmt;

use crate::model::{self, Colour, Parsed, RelationshipId, Shape, ViewKind};

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
        todo!()
    }

    /// The key-based reference of relationship `id` in `model`: its ends'
    /// canonical element keys and its index in
    /// `model.find_relationship(source, destination)`. The editor calls it
    /// on a hit-tested line so it never re-implements identity. Panics on
    /// an id from another model, like `model::Model::relationship`.
    /// Serves DSL-10, EDIT-19, EDIT-24.
    pub fn of(model: &model::Model, id: RelationshipId) -> RelationshipRef {
        todo!()
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
}

/// Where a style change is written (DSL-13, EDIT-17).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StyleTarget {
    /// `all <tag>`: edit or add the property in that tag's `element`
    /// block, adding the block at the end of `styles` when there is none.
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
    /// at the end of `styles` (DSL-13).
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
/// style change, and a default view is added (EDIT-15).
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
    /// `<scope id>-<kind>`, appended as the last view in `views`, holding
    /// `include *` and no `at` lines. It adds a view, not an element
    /// (EDIT-21). The editor puts it first in the batch of the first write
    /// made in a default view (move, resize, style), so that write is one
    /// step; its inverse removes the view block.
    AddDefaultView {
        /// The scope element key (the bare `model::Element::key` in phase
        /// 1).
        scope: String,
        /// Container view of a system, component view of a container.
        kind: ViewKind,
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
    /// edit wrote there; it then removes only the tag or the property
    /// (EDIT-21). The inverse of a text insertion that added `""`
    /// placeholders also takes those placeholders out.
    pub inverse: Vec<Edit>,
}

/// Applies `edits` in order to the root file of `parsed`, finding each
/// target by key, and returns the patched text and the inverse edits.
/// The same call re-applies a user action to a newer text after an
/// external change (EDIT-24): the caller parses the new file and passes
/// it. An empty list returns the source unchanged (DSL-11). Serves DSL-9,
/// DSL-10, DSL-13, EDIT-19, EDIT-24.
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
/// does not carry, a style edit in a file with no `views` block).
pub fn apply(parsed: &Parsed, edits: &[Edit]) -> Result<Applied, EditError> {
    todo!()
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
        todo!()
    }
}

impl std::error::Error for EditError {}

#[cfg(test)]
mod tests {
    use super::*;

    // One test per requirement.

    #[test]
    fn dsl_7_at_most_one_at_and_size_line_per_box() {
        // A move or resize of a box that already has a line rewrites it,
        // never adds a second one.
        todo!()
    }

    #[test]
    fn dsl_9_move_rewrites_only_at_line() {
        // Byte diff of input vs output is exactly the target line.
        todo!()
    }

    #[test]
    fn dsl_10_set_name_rewrites_only_literal() {
        todo!()
    }

    #[test]
    fn dsl_10_relationship_and_view_title_literals() {
        // Relationship description and technology, view title and
        // subtitle: each rewrites only its literal.
        todo!()
    }

    #[test]
    fn dsl_10_missing_view_title_inserted_as_first_body_line() {
        // A view with no `title` (and one with no `subtitle`): the line
        // is the first line of the view body, indented per DSL-9.
        todo!()
    }

    #[test]
    fn dsl_11_empty_edit_list_is_identity() {
        // LF and CRLF, with and without a final newline.
        todo!()
    }

    #[test]
    fn dsl_13_style_change_writes_styles_block() {
        todo!()
    }

    #[test]
    fn edit_17_multi_box_style_is_one_batch() {
        // SetStyle on three selected boxes in one `apply`: one `Applied`,
        // one inverse list that undoes all three.
        todo!()
    }

    #[test]
    fn edit_18_resize_rewrites_only_size_line() {
        // DSL-7: byte diff is exactly the `size` line; off-grid sizes are
        // written as given.
        todo!()
    }

    #[test]
    fn edit_19_inverse_is_semantic_and_survives_reload() {
        // The inverse is a list of `Edit`s, applied by key to a reloaded
        // text with an unrelated external change.
        todo!()
    }

    #[test]
    fn edit_21_no_element_or_relationship_created_or_deleted() {
        // Every edit kind, applied to a file: element and relationship
        // counts and keys are unchanged.
        todo!()
    }

    #[test]
    fn edit_21_inverse_never_removes_nonempty_block() {
        // ThisBox SetStyle creates an element's `{ }` block; an external
        // change adds a child element and a nested relationship to it and
        // another style to the `element "<tag>"` block. Applying the
        // inverse (and ResetStyle) removes only the tag and the property:
        // the block, the child, the relationship and the other style stay.
        todo!()
    }

    #[test]
    fn edit_24_reapply_on_changed_text() {
        todo!()
    }

    #[test]
    fn edit_24_relationship_ref_of_round_trips() {
        // For every relationship of a model with parallel `a -> b` lines:
        // `RelationshipRef::of` then resolving the ref in a re-parse of
        // the same text gives the same relationship.
        todo!()
    }

    #[test]
    fn edit_24_drifted_target_reports_conflict() {
        // A relationship with the same ends inserted before the target
        // shifts the ordinal; SetText with `expected` reports `Conflict`
        // and writes nothing.
        todo!()
    }

    #[test]
    fn edit_26_batch_of_moves_writes_every_at_line_in_one_step() {
        // A view with no `at` line, a batch of Moves ordered by
        // `ElementId`: one `Applied`, the new lines in batch order, and a
        // gone target in the batch writes nothing.
        todo!()
    }

    // edit-01: move and resize

    #[test]
    fn move_inserts_missing_at_line() {
        // DSL-9: after the last `at` line, else before the closing brace,
        // copying indentation and line ending.
        todo!()
    }

    // edit-02: text edits

    #[test]
    fn set_label_escapes_quotes() {
        // DSL-10: `"` and `\` escaped.
        todo!()
    }

    #[test]
    fn clearing_non_trailing_positional_writes_placeholder() {
        // DSL-10: SetText Description None on `container "n" "d" "t"`
        // writes `""`; the technology stays in its slot.
        todo!()
    }

    #[test]
    fn technology_on_person_or_system_is_invalid() {
        // DSL-10: `InvalidValue`, nothing written.
        todo!()
    }

    // edit-03: style edits

    #[test]
    fn dsl_13_shared_tag_gets_unique_tag() {
        // ThisBox on an element whose tag is shared: only it changes.
        // `all <tag>` on the same shared tag: every element carrying it
        // changes, as asked.
        todo!()
    }

    #[test]
    fn style_tag_not_carried_is_invalid() {
        // DSL-13: `StyleTarget::Tag` with a tag the element lacks is
        // `InvalidValue`; a tag overridden later in the cascade (DSL-15)
        // is written all the same.
        todo!()
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
        todo!()
    }

    #[test]
    fn edit_24_target_gone_reported() {
        todo!()
    }

    // Review Focus

    #[test]
    fn newline_in_text_is_refused() {
        // DSL-10: `\n`, `\r\n` and a lone `\r` are `Newline`; another
        // control character is `InvalidValue`.
        todo!()
    }

    #[test]
    fn absent_literal_inserted_with_placeholders() {
        // DSL-10: a technology on a container with no description gets
        // `""` for the description.
        todo!()
    }

    #[test]
    fn reapply_after_rename_reports_target_gone() {
        // EDIT-24: never patches the wrong span.
        todo!()
    }

    #[test]
    fn insert_at_line_into_one_line_view() {
        // DSL-9: `{ include * }` is split onto lines; the new line gets
        // the view's indentation plus one indent unit.
        todo!()
    }

    #[test]
    fn dsl_9_at_line_after_view_head_gets_body_indentation() {
        // A view whose body has no `at` line: the new line takes the first
        // body line's indentation, else the view's plus four spaces.
        todo!()
    }

    #[test]
    fn dsl_9_new_size_line_after_last_size_else_last_at() {
        // DSL-9, EDIT-18: after the last `size` line; with none, after the
        // last `at` line; with neither, before the closing brace.
        todo!()
    }

    #[test]
    fn dsl_13_this_box_tag_is_dotted_id_with_suffix() {
        // The tag is the full dotted id; `-2`, `-3` when another element
        // already carries it; the `tags` line is last in the block.
        todo!()
    }

    #[test]
    fn edit_15_default_view_written_on_first_write() {
        // AddDefaultView then a Move in one batch: the view (key
        // `<scope id>-<kind>`, `include *`, no `at` lines from it) is the
        // last view in `views`, written first; the inverse removes it.
        todo!()
    }

    #[test]
    fn reset_style_removes_tag_and_block() {
        // DSL-13.
        todo!()
    }

    #[test]
    fn batch_is_atomic() {
        // One gone target in a multi-box batch: nothing written.
        todo!()
    }

    #[test]
    fn crlf_file_keeps_line_endings() {
        // DSL-9: an inserted line copies `\r\n` from the line above.
        todo!()
    }
}
