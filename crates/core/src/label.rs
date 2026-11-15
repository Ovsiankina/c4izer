//! Places each edge label on its line, clear of boxes, other labels and
//! other lines, and reports labels with no room (CORE-14, 15).
//!
//! Depends on: `route`, `view`, `geom`.

// Shape of the API: `place_labels` takes `(&ViewPlan, &Routes)` (the plan
// and the routed lines; the box centres are `routes.centres`, the ones
// the lines were routed at, so boxes and lines cannot disagree) and
// returns `Labels`, plain data indexed by `EdgeId` like `Routes::lines`,
// which `scene`, `pipeline` and (through the scene) `lint` read. Labels
// for one view need only that view's plan and routes (CORE-28); the drag
// preview re-runs `place_labels` after `route::reroute`.
//
// DECISION: named `place_labels`, not `place`, so `pipeline`, which also
// imports `place::place`, needs no renaming import.
//
// Units: a label's centre is a grid point on its own line (grid units,
// `geom`); its size is in px (sub-grid, CORE-30 text widths). `bounds()`
// rounds the px rectangle outward to the grid for the geom overlap tests
// (geom.rs header).
// DECISION: every px quantity (sizes, constants, `EM`, `text_advance`) is
// `i32` like `geom::grid_to_px`, so the rectangle and width formulas in
// the docs below need no casts.
//
// Text widths (CORE-30): no other contract has a text-measuring item
// (geom says widths are measured by `scene` and `label`, not geom), and
// `scene` depends on `label`, never the reverse (HLD §4). So the width
// table lives here, as the earliest stage that needs it: `text_advance`
// gives a string's advance in thousandths of an em from the bundled
// Inter width table; `scene` sizes and wraps box texts, the key and the
// title with the same function, so label and scene cannot disagree.
// HLD §4 says the CORE-30 width table lives here and scene uses it.
//
// DECISION: the width table is font-size agnostic (thousandths of an em,
// integers), so any font size a caller uses gives the same exact,
// deterministic arithmetic (CORE-25); the caller multiplies by its size.
// DECISION: `Weight` covers regular and bold (box names are bold), the
// two bundled static faces (`scene::FONT_REGULAR`, `scene::FONT_BOLD`).
// DECISION: title and key are not label obstacles here: their place is
// `scene`'s, which lays them outside the bounds of every box, line and
// label (`Label::bounds`), so CORE-14's "no title, no key" holds there.
// Its test, `core_14_title_key_clear_of_labels`, is scene's to own (row
// in docs/deferred.md; scene-03 at step 4). The boundary name tag is an
// obstacle, and its rectangle is `boundary_tag`, which `scene` draws at,
// so both agree.
// DECISION: an edge with no line (`Routes::lines[e] == None`) has no
// label: CORE-13 draws an unroutable line with no label, and its L1
// finding names it.
// DECISION: an edge with no description and no shown technology has no
// label (lint L12 reports it from the plan, not from `Labels`).
// DECISION: a label with no clear spot is drawn centred on the grid
// point of its line nearest half the line's length (CORE-15 "middle of
// its line"); ties go to the point nearer the start. CORE-15 wins over
// the plan's Review Focus, which now reads "reported, never placed over
// a box as a clear spot": a spot over a box is never chosen as clear.
// DECISION: a joined line shares its trunk's label when both have the
// same `description` and the same `technology` as written, not the shown
// technology: CORE-14 says "description and technology", so under
// `technology off` two lines that differ only in technology keep a label
// each.
// DECISION: edges are placed in ascending `EdgeId`; each label keeps
// clear of the labels placed before it, so the order is part of the
// output (CORE-25).
// L23/L24 reach `lint` only through the scene (lint depends on scene and
// model, HLD): row in docs/deferred.md asks scene's label item to carry
// `Fit` and the edge's winning relationship id.
//
// CORE-14: label text is 11px Inter Regular on a 13px line pitch,
// padded 5px left and right and 3px top and bottom; the 30px and 50px
// gaps are measured along the line, from its start port and back from
// its last vertex, to the label's centre.
// CORE-1: the boundary name tag reads the scope's name, then its type in
// brackets (`[Software System]` on a container view, `[Container]` on a
// component view), in bold, as `boundary_tag` sizes it.

use crate::geom::{Point, Rect};
use crate::model::ViewOptions;
use crate::route::Routes;
use crate::view::{EdgeId, PlanEdge, ViewPlan};

/// Thousandths of an em: the unit of `text_advance`. A text's width in
/// px at font size `s` px is `text_advance(..) * s / EM` (round up to be
/// safe). Serves CORE-30.
pub const EM: i32 = 1000;

/// Font weight of a measured text. `#[non_exhaustive]` so a later phase
/// (P2 `fontSize`, styles) can add weights. Serves CORE-30.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Weight {
    /// Inter Regular: labels, descriptions, the key.
    Regular,
    /// Inter Bold: box names, the title, the boundary name tag.
    Bold,
}

/// The advance width of `text` in thousandths of an em (`EM`), summed per
/// character from the fixed width table derived from the bundled Inter
/// font; no system font metrics, no kerning, so the same text gives the
/// same width on every machine (CORE-25). A character missing from the
/// table counts as a fixed fallback width. The one width source of the
/// crate: `label` sizes labels and the boundary tag with it, `scene`
/// wraps and sizes every other text with it. Serves CORE-30, CORE-14.
#[must_use]
pub fn text_advance(text: &str, weight: Weight) -> i32 {
    todo!()
}

/// Label text size in px (Inter Regular, CORE-14). Serves CORE-14,
/// CORE-30.
pub const LABEL_FONT_PX: i32 = 11;

/// Distance in px between the baselines of two label lines. Serves
/// CORE-14 (13px pitch), CORE-30.
pub const LABEL_LINE_PX: i32 = 13;

/// Padding in px between a label's text and its rectangle, left and
/// right (CORE-14: 5px). Serves CORE-14, CORE-30.
pub const LABEL_PAD_X_PX: i32 = 5;

/// Padding in px between a label's text and its rectangle, top and
/// bottom (CORE-14: 3px). Serves CORE-14, CORE-30.
pub const LABEL_PAD_Y_PX: i32 = 3;

/// Least distance in px along a line from its start port to its label's
/// centre (CORE-14: measured along the line to the label's centre).
pub const START_GAP_PX: i32 = 30;

/// Least distance in px along a line back from its arrowhead to its
/// label's centre (CORE-14). A joined line ends in a dot, not an
/// arrowhead: the gap does not apply at its end. Measured along the line
/// to the label's centre (CORE-14).
pub const ARROW_GAP_PX: i32 = 50;

/// The text lines of `edge`'s label: its description wrapped at
/// `options.label_width` characters (counted as Unicode scalar values;
/// words split on whitespace, a word longer than the width alone on its
/// line, never broken), then `[technology]` on its own line when
/// `edge.shown_technology(options)` is `Some` (CORE-4: none under
/// `technology off`). Empty when the edge has neither. Serves CORE-14,
/// CORE-4.
#[must_use]
pub fn label_lines(edge: &PlanEdge, options: &ViewOptions) -> Vec<String> {
    todo!()
}

/// How a placed label sits. `#[non_exhaustive]` so later phases (a label
/// moved by hand, P2 `position`) can add cases. Serves CORE-14, CORE-15.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Fit {
    /// On its own line, clear of every box, label, line and the
    /// boundary tag (CORE-14).
    Clear,
    /// The only spots left cover other lines: drawn there, reported as
    /// warning L23 (CORE-15). Holds the lines it covers, ascending.
    OverLines(Vec<EdgeId>),
    /// No spot clear of boxes, labels and the boundary tag: drawn at the
    /// middle of its line, reported as warning L24 (CORE-15).
    NoRoom,
}

/// One placed label. Its rectangle in px has its centre at
/// `grid_to_px(centre)`, left edge at `grid_to_px(centre.x) -
/// width_px / 2` and top edge at `grid_to_px(centre.y) - height_px / 2`
/// (integer division); `scene` draws the text lines inside it with
/// `LABEL_FONT_PX`, `LABEL_LINE_PX` and the paddings. Serves CORE-14,
/// CORE-15, CORE-10 (chevrons keep 8px from it).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Label {
    /// The edge it labels; always equals its index in `Labels::labels`.
    pub edge: EdgeId,
    /// The text lines, `label_lines` of the edge; never empty.
    pub lines: Vec<String>,
    /// The label's centre: a grid point on its own line (CORE-14).
    pub centre: Point,
    /// Rectangle width in px: the widest line (`text_advance` at
    /// `LABEL_FONT_PX`, rounded up) plus twice `LABEL_PAD_X_PX`.
    pub width_px: i32,
    /// Rectangle height in px: `lines.len() * LABEL_LINE_PX` plus twice
    /// `LABEL_PAD_Y_PX`.
    pub height_px: i32,
    /// Whether it sits clear, over lines, or with no room (CORE-15).
    pub fit: Fit,
}

impl Label {
    /// The label rectangle in grid units, rounded outward from its px
    /// rectangle: what the overlap tests against boxes, lines, labels and
    /// the boundary tag use, and what `scene` keeps the title and key
    /// clear of. Serves CORE-14.
    #[must_use]
    pub const fn bounds(&self) -> Rect {
        todo!()
    }
}

/// The labels of one view. Same plan and routes give an equal
/// value on every run and machine (CORE-25). Serves CORE-14, CORE-15.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Labels {
    /// One entry per plan edge, `labels[i]` for `EdgeId(i)`. `None` when
    /// the edge is unroutable, has empty `label_lines`, or is listed in
    /// `shared`.
    pub labels: Vec<Option<Label>>,
    /// `(joined, trunk)` pairs, ascending: a joined line whose trunk has
    /// the same `description` and the same `technology` (as written, not
    /// `shown_technology`), so it has no label of its own (CORE-14).
    pub shared: Vec<(EdgeId, EdgeId)>,
}

impl Labels {
    /// The label of `edge`, `None` when it has none or is out of range.
    /// Serves CORE-14.
    #[must_use]
    pub fn label(&self, edge: EdgeId) -> Option<&Label> {
        todo!()
    }

    /// Edges whose label covers another line, ascending: warning L23,
    /// named by `plan.edges[e].winner()` (CORE-15).
    #[must_use]
    pub fn over_lines(&self) -> Vec<EdgeId> {
        todo!()
    }

    /// Edges whose label has no room, ascending: warning L24, named by
    /// `plan.edges[e].winner()` (CORE-15, CORE-23 "no label without
    /// room").
    #[must_use]
    pub fn no_room(&self) -> Vec<EdgeId> {
        todo!()
    }
}

/// The rectangle, in grid units rounded outward, of the name tag of
/// `plan.boundaries[index]` with every box at `centres[box]`: at the
/// bottom left inside `ViewPlan::boundary_rect`, sized (with
/// `text_advance`, bold) for the boundary name followed by the scope's
/// type in brackets. `None` when `index` is out of range. Labels keep
/// clear of it (CORE-14); `scene` draws the tag in it (CORE-1). Serves
/// CORE-14, CORE-1.
///
/// # Panics
///
/// When `centres` is shorter than `plan.boxes`.
#[must_use]
pub fn boundary_tag(plan: &ViewPlan, index: usize, centres: &[Point]) -> Option<Rect> {
    todo!()
}

/// Places the label of every routed edge of `plan` on its own line, with
/// every box at `routes.centres[box]` and lines from `routes`. Edges are
/// placed in ascending `EdgeId`. Candidate centres are the line's grid
/// points (on any segment, so a label may straddle a bend) at least
/// `START_GAP_PX` along the line from its start and `ARROW_GAP_PX` back
/// from its arrowhead, ordered by distance along the line from half its
/// length, ties to the one nearer the start. The first candidate clear
/// of every box, every label already placed, every other line and every
/// boundary tag wins (`Fit::Clear`); else, among the candidates still
/// clear of every box, placed label and boundary tag, the one covering
/// the fewest other lines, ties broken by the same order
/// (`Fit::OverLines`); else the middle of the line (`Fit::NoRoom`). A
/// joined line whose trunk has the same description and technology gets
/// none (`Labels::shared`); any other joined line's label stays on its
/// own branch, never on the trunk. Deterministic (CORE-25). Serves
/// CORE-14, CORE-15, CORE-25.
///
/// # Panics
///
/// When `routes` is a `Routes` of another plan (`routes.lines` not as
/// long as `plan.edges`, or `routes.centres` not as long as
/// `plan.boxes`).
#[must_use]
pub fn place_labels(plan: &ViewPlan, routes: &Routes) -> Labels {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test per requirement clause.
    #[test]
    fn core_14_wrapped_at_label_width() {
        // Description wrapped at `labelWidth` characters, then
        // `[technology]` on its own line; a long word stays whole.
        todo!()
    }

    #[test]
    fn core_14_label_on_its_line() {
        // Centre is a grid point of its own line.
        todo!()
    }

    #[test]
    fn core_14_gaps_from_start_and_arrowhead() {
        // >= 30px from start, >= 50px from the arrowhead; a joined line
        // has no arrowhead gap at its dot.
        todo!()
    }

    #[test]
    fn core_14_label_clear_of_boxes_labels_lines() {
        todo!()
    }

    #[test]
    fn core_14_clear_of_boundary_tag() {
        todo!()
    }

    #[test]
    fn core_14_joined_line_same_text_has_no_label() {
        // Listed in `shared`; a joined line with other text keeps its
        // label on its own branch, not on the trunk (Review Focus). Same
        // description, different technology under `technology off`:
        // both keep a label (CORE-14 compares the technology).
        todo!()
    }

    #[test]
    fn core_14_long_label_straddles_bend_only_if_clear() {
        // Label longer than any straight run: centred across a bend when
        // that spot is clear, else reported (Review Focus).
        todo!()
    }

    #[test]
    fn core_14_placed_in_edge_order() {
        // Two edges competing for one spot: the lower `EdgeId` gets it.
        todo!()
    }

    #[test]
    fn core_14_no_text_no_label() {
        // No description and no shown technology: `labels[e]` is `None`
        // and `e` is not in `shared` (lint L12 reads the plan).
        todo!()
    }

    #[test]
    fn core_14_bounds_round_px_rect_outward() {
        // A px rectangle not on the grid: `bounds()` contains it and is
        // the smallest grid rectangle that does.
        todo!()
    }

    #[test]
    fn core_1_boundary_tag_bottom_left_in_boundary() {
        // Inside `boundary_rect`, at its bottom left, sized bold for the
        // name then `[type]`; `None` for an out-of-range index.
        todo!()
    }

    #[test]
    fn core_15_over_line_reported() {
        // `Fit::OverLines` names the covered lines; `over_lines()` lists
        // it. The spot is still clear of boxes, labels and the boundary
        // tag, and covers the fewest lines (ties: nearer the middle, then
        // nearer the start).
        todo!()
    }

    #[test]
    fn core_15_no_room_reported() {
        // Very short line between adjacent boxes: reported as L24 and
        // drawn at the middle of its line, as CORE-15 says, even when
        // that covers a box: CORE-15 wins; such a spot is never chosen
        // as a clear spot (plan Review Focus).
        todo!()
    }

    #[test]
    fn core_25_same_input_same_labels() {
        todo!()
    }

    #[test]
    fn core_30_text_advance_from_width_table() {
        // Per-character sum, bold wider than regular, fallback width for
        // a character missing from the table.
        todo!()
    }

    #[test]
    fn core_4_technology_off_hides_tech_line() {
        todo!()
    }

    #[test]
    fn core_13_unroutable_edge_has_no_label() {
        todo!()
    }
}
