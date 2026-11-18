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
// rounds the px rectangle outward to the grid (geom.rs header) for
// `scene`, which keeps the title and key clear of it.
// DECISION: the placement overlap tests (labels against boxes, boundary
// tags, placed labels and lines) run on the exact px rectangle, with
// boxes, tags and lines scaled to px: the grid bounds can be up to 9px
// larger per side than the drawn label, which rejected clear spots
// (false L24) and counted lines that only touch the drawn label (false
// L23). Boxes and tags are grid-aligned, so against them the px test
// gives the same answer as the grid one.
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
// DECISION: a joined line shares its trunk's label when the trunk's
// label as drawn (`label_lines`) equals its own: CORE-14 compares the
// trunk's *label*, and under `technology off` CORE-4 drops the
// technology from it, so two lines that differ only in technology share
// one label (as the prototype clears `tech` before its `said` check).
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

use crate::geom::{GRID_PX, Point, Rect, Segment, grid_to_px};
use crate::model::{ElementKind, MetadataSymbols, Terminology, ViewKind, ViewOptions};
use crate::route::{Line, Routes};
use crate::view::{BoundaryOf, EdgeId, PlanEdge, ViewPlan};

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
    text.chars().map(|c| advance(c, weight)).sum()
}

/// Advance of one character in thousandths of an em.
fn advance(c: char, weight: Weight) -> i32 {
    let (ascii, latin): (&[i16], &[i16]) = match weight {
        Weight::Regular => (&REGULAR_0020, &REGULAR_00A0),
        Weight::Bold => (&BOLD_0020, &BOLD_00A0),
    };
    let code = u32::from(c);
    let entry = match code {
        0x20..=0x7E => ascii.get((code - 0x20) as usize),
        0xA0..=0x17F => latin.get((code - 0xA0) as usize),
        _ => None,
    };
    match entry {
        Some(&w) if w >= 0 => i32::from(w),
        _ => FALLBACK_ADVANCE,
    }
}

// DECISION: a character outside the table (or with no glyph in Inter,
// marked -1) counts one em: about the widest Latin glyph and about a
// CJK glyph's width, so a measured text errs on the wide side and L25
// errs towards reporting.
const FALLBACK_ADVANCE: i32 = EM;

// DECISION: the width table covers Basic Latin (U+0020..U+007E), Latin-1
// Supplement and Latin Extended-A (U+00A0..U+017F): the scripts of the
// example and of European names. Values are the `hmtx` advances of the
// bundled Inter 4.1 static TTFs (unitsPerEm 2048), mapped through their
// `cmap` and scaled to thousandths of an em rounded to nearest; -1 marks
// a code point with no glyph. Extracted once with a stand-alone script
// (no font parsing at run time, no dependency).
#[rustfmt::skip]
const REGULAR_0020: [i16; 95] = [
    281, 288, 466, 633, 642, 982, 644, 300, 365, 365, 501, 662, 288, 460, 288, 360,
    631, 407, 610, 618, 646, 593, 620, 566, 619, 620, 288, 302, 662, 662, 662, 511,
    966, 690, 654, 730, 722, 601, 590, 746, 743, 269, 571, 672, 565, 903, 753, 765,
    639, 765, 644, 642, 646, 744, 690, 985, 682, 679, 629, 365, 360, 365, 471, 456,
    323, 562, 612, 571, 612, 583, 370, 613, 591, 242, 242, 549, 242, 876, 591, 600,
    612, 612, 376, 528, 327, 591, 562, 818, 546, 562, 552, 426, 333, 426, 662,
];
#[rustfmt::skip]
const REGULAR_00A0: [i16; 224] = [
    281, 288, 571, 611, 725, 550, 270, 568, 591, 914, 454, 583, 662, -1, 666, 478,
    456, 662, 442, 446, 323, 586, 603, 288, 268, 304, 482, 583, 802, 847, 882, 511,
    690, 690, 690, 690, 690, 690, 994, 730, 601, 601, 601, 601, 269, 269, 269, 269,
    735, 753, 765, 765, 765, 765, 765, 662, 765, 744, 744, 744, 744, 679, 636, 616,
    562, 562, 562, 562, 562, 562, 917, 571, 583, 583, 583, 583, 242, 242, 242, 242,
    583, 591, 600, 600, 600, 600, 600, 662, 600, 591, 591, 591, 591, 562, 612, 562,
    690, 562, 690, 562, 690, 562, 730, 571, 730, 571, 730, 571, 730, 571, 722, 710,
    735, 612, 601, 583, 601, 583, 601, 583, 601, 583, 601, 583, 746, 613, 746, 613,
    746, 613, 746, 613, 743, 591, 776, 591, 269, 242, 269, 242, 269, 242, 269, 242,
    269, 242, 839, 484, 571, 242, 672, 549, 525, 565, 242, 565, 242, 565, 340, 563,
    359, 595, 242, 753, 591, 753, 591, 753, 591, -1, 753, 591, 765, 600, 765, 600,
    765, 600, 1005, 1000, 644, 376, 644, 376, 644, 376, 642, 528, 642, 528, 642, 528,
    642, 528, 646, 327, 646, 376, 646, 327, 744, 591, 744, 591, 744, 591, 744, 591,
    744, 591, 744, 591, 985, 818, 679, 562, 679, 629, 552, 629, 552, 629, 552, 314,
];
#[rustfmt::skip]
const BOLD_0020: [i16; 95] = [
    237, 338, 552, 649, 655, 1016, 672, 339, 377, 377, 559, 679, 334, 468, 334, 388,
    674, 431, 630, 646, 676, 622, 649, 582, 651, 649, 334, 343, 679, 679, 679, 560,
    1016, 747, 662, 740, 722, 607, 587, 750, 747, 281, 584, 719, 565, 932, 762, 771,
    648, 777, 657, 655, 667, 732, 747, 1038, 738, 731, 664, 377, 388, 377, 487, 476,
    365, 581, 630, 588, 630, 596, 398, 632, 623, 271, 271, 580, 271, 913, 623, 613,
    630, 630, 407, 560, 366, 623, 600, 850, 580, 602, 573, 469, 372, 469, 679,
];
#[rustfmt::skip]
const BOLD_00A0: [i16; 224] = [
    237, 338, 588, 639, 762, 570, 339, 568, 623, 914, 464, 660, 679, -1, 662, 439,
    459, 679, 460, 472, 365, 628, 599, 334, 364, 333, 490, 660, 846, 881, 920, 560,
    747, 747, 747, 747, 747, 747, 1022, 740, 607, 607, 607, 607, 281, 281, 281, 281,
    760, 762, 771, 771, 771, 771, 771, 679, 771, 732, 732, 732, 732, 731, 669, 657,
    581, 581, 581, 581, 581, 581, 910, 588, 596, 596, 596, 596, 271, 271, 271, 271,
    599, 623, 613, 613, 613, 613, 613, 679, 613, 623, 623, 623, 623, 602, 630, 602,
    747, 581, 747, 581, 747, 581, 740, 588, 740, 588, 740, 588, 740, 588, 722, 754,
    760, 630, 607, 596, 607, 596, 607, 596, 607, 596, 607, 596, 750, 632, 750, 632,
    750, 632, 750, 632, 747, 623, 807, 623, 281, 271, 281, 271, 281, 271, 281, 271,
    281, 271, 865, 542, 584, 271, 719, 580, 586, 565, 271, 565, 271, 565, 395, 578,
    449, 605, 271, 762, 623, 762, 623, 762, 623, -1, 762, 623, 771, 613, 771, 613,
    771, 613, 1018, 988, 657, 407, 657, 407, 657, 407, 655, 560, 655, 560, 655, 560,
    655, 560, 667, 366, 667, 441, 667, 366, 732, 623, 732, 623, 732, 623, 732, 623,
    732, 623, 732, 623, 1038, 850, 731, 602, 731, 664, 573, 664, 573, 664, 573, 345,
];

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
/// `technology off`). Empty when the edge has neither. With a style
/// `width W` the description wraps to W px at the label's size instead
/// (P2-424). Serves CORE-14, CORE-4, P2-424.
#[must_use]
pub fn label_lines(edge: &PlanEdge, options: &ViewOptions) -> Vec<String> {
    let description = edge.description.as_deref().unwrap_or("");
    let mut lines = match edge.style.width {
        // P2-424: wrap to W px of text at the label's own size (CORE-30
        // table), in place of `labelWidth` characters.
        Some(w) => wrap_px(
            description,
            label_font_px(edge),
            i32::try_from(w).unwrap_or(i32::MAX),
        ),
        None => wrap_chars(description, options.label_width),
    };
    // DECISION: a blank technology counts as none, as in the prototype
    // (`if e['tech']`), so no empty `[]` line is drawn.
    if let Some(tech) = edge.shown_technology(options)
        && !tech.trim().is_empty()
    {
        lines.push(format!("[{tech}]"));
    }
    lines
}

/// `text` wrapped at `width` characters (Unicode scalar values), words
/// split on whitespace, a longer word alone on its line.
fn wrap_chars(text: &str, width: u32) -> Vec<String> {
    let width = usize::try_from(width).unwrap_or(usize::MAX);
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for word in text.split_whitespace() {
        let n = word.chars().count();
        if cur_len > 0 && cur_len + 1 + n > width {
            lines.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        if cur_len > 0 {
            cur.push(' ');
            cur_len += 1;
        }
        cur.push_str(word);
        cur_len += n;
    }
    if cur_len > 0 {
        lines.push(cur);
    }
    lines
}

/// `text` wrapped greedily to `max_px` of Inter Regular at `size_px`
/// (CORE-30 table), a wider word alone on its line (P2-424).
fn wrap_px(text: &str, size_px: i32, max_px: i32) -> Vec<String> {
    let width = |t: &str| (text_advance(t, Weight::Regular) * size_px + EM - 1) / EM;
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
            continue;
        }
        let candidate = format!("{cur} {word}");
        if width(&candidate) > max_px {
            lines.push(std::mem::replace(&mut cur, word.to_owned()));
        } else {
            cur = candidate;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// The label text size of `edge`: its style's `fontSize`, else
/// `LABEL_FONT_PX` (P2-423).
fn label_font_px(edge: &PlanEdge) -> i32 {
    edge.style
        .font_size
        .map_or(LABEL_FONT_PX, |n| i32::try_from(n).unwrap_or(LABEL_FONT_PX).clamp(1, 1000))
}

/// The label line pitch of `edge`: `fontSize + 2`, else `LABEL_LINE_PX`
/// (P2-423).
// TODO(srs): P2-423, the line pitch. Reading taken (SRS proposal): N + 2,
// as 11 on 13 in phase 1.
fn label_line_px(edge: &PlanEdge) -> i32 {
    if edge.style.font_size.is_some() {
        label_font_px(edge) + 2
    } else {
        LABEL_LINE_PX
    }
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
    /// Text size in px: the edge style's `font_size`, else
    /// `LABEL_FONT_PX` (P2-423).
    pub font_px: i32,
    /// Line pitch in px: `font_px + 2`, else `LABEL_LINE_PX` (P2-423).
    // TODO(srs): P2-423, the line pitch. Reading taken (SRS proposal):
    // N + 2, as 11/13 in phase 1.
    pub line_px: i32,
}

impl Label {
    /// The label rectangle in px, exactly as drawn: the rectangle the
    /// placement overlap tests use.
    const fn px_rect(&self) -> Rect {
        let left = grid_to_px(self.centre.x) - self.width_px / 2;
        let top = grid_to_px(self.centre.y) - self.height_px / 2;
        Rect::from_corners(
            Point::new(left, top),
            Point::new(left + self.width_px, top + self.height_px),
        )
    }

    /// The label rectangle in grid units, rounded outward from its px
    /// rectangle: what `scene` keeps the title and key clear of. The
    /// placement overlap tests against boxes, lines, labels and the
    /// boundary tag use the px rectangle itself (CORE-15). Serves
    /// CORE-14.
    #[must_use]
    pub const fn bounds(&self) -> Rect {
        let px = self.px_rect();
        Rect::from_corners(
            Point::new(px.min.x.div_euclid(GRID_PX), px.min.y.div_euclid(GRID_PX)),
            Point::new(ceil_grid(px.max.x), ceil_grid(px.max.y)),
        )
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
    /// `(joined, trunk)` pairs, ascending: a joined line whose trunk's
    /// drawn label (`label_lines` under the view's options) equals its
    /// own, so it has no label of its own (CORE-14).
    pub shared: Vec<(EdgeId, EdgeId)>,
}

impl Labels {
    /// The label of `edge`, `None` when it has none or is out of range.
    /// Serves CORE-14.
    #[must_use]
    pub fn label(&self, edge: EdgeId) -> Option<&Label> {
        self.labels.get(edge.0 as usize).and_then(Option::as_ref)
    }

    /// Edges whose label covers another line, ascending: warning L23,
    /// named by `plan.edges[e].winner()` (CORE-15).
    #[must_use]
    pub fn over_lines(&self) -> Vec<EdgeId> {
        self.with_fit(|f| matches!(f, Fit::OverLines(_)))
    }

    /// Edges whose label has no room, ascending: warning L24, named by
    /// `plan.edges[e].winner()` (CORE-15, CORE-23 "no label without
    /// room").
    #[must_use]
    pub fn no_room(&self) -> Vec<EdgeId> {
        self.with_fit(|f| *f == Fit::NoRoom)
    }

    /// Edges whose label's fit passes `keep`, ascending.
    fn with_fit(&self, keep: impl Fn(&Fit) -> bool) -> Vec<EdgeId> {
        self.labels
            .iter()
            .flatten()
            .filter(|l| keep(&l.fit))
            .map(|l| l.edge)
            .collect()
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
    boundary_tag_with(plan, index, centres, &Words::DEFAULT)
}

/// `boundary_tag` with the tag's type word from `words` (P2-161).
pub(crate) fn boundary_tag_with(
    plan: &ViewPlan,
    index: usize,
    centres: &[Point],
    words: &Words<'_>,
) -> Option<Rect> {
    let outer = plan.boundary_rect(index, centres)?;
    let boundary = &plan.boundaries[index];
    let font_px = boundary_tag_font_px(boundary);
    if matches!(boundary.of, BoundaryOf::DeploymentNode(_)) {
        // P3-114: name over type line at the top left.
        let lines = boundary_tag_lines(plan, index, words);
        let text_px = lines
            .iter()
            .map(|l| (text_advance(l, Weight::Bold) * font_px + EM - 1) / EM)
            .max()
            .unwrap_or(0);
        let right_px = grid_to_px(outer.min.x) + TAG_TEXT_INSET_PX + text_px + TAG_PAD_RIGHT_PX;
        let n = i32::try_from(lines.len()).unwrap_or(1);
        let height = ceil_grid(NODE_TAG_TOP_PX + n * (font_px + 3) + 2);
        return Some(Rect::from_corners(
            Point::new(outer.min.x, outer.min.y),
            Point::new(ceil_grid(right_px).min(outer.max.x).max(outer.min.x + 1), outer.min.y + height),
        ));
    }
    let text = boundary_tag_text(plan, index, words);
    let text_px = (text_advance(&text, Weight::Bold) * font_px + EM - 1) / EM;
    let right_px = grid_to_px(outer.min.x) + TAG_TEXT_INSET_PX + text_px + TAG_PAD_RIGHT_PX;
    // DECISION: a larger tag font (P2-411 `fontSize`) grows the tag by the
    // same px, so the 30px band at 12px keeps 18px of room around the text.
    let height = TAG_HEIGHT_UNITS.max(ceil_grid(font_px + 18));
    Some(Rect::from_corners(
        Point::new(outer.min.x, outer.max.y - height),
        Point::new(ceil_grid(right_px), outer.max.y),
    ))
}

/// The top of a deployment node tag's first line below the boundary's
/// top edge, in px (P3-114).
pub(crate) const NODE_TAG_TOP_PX: i32 = 6;

/// The lines of boundary `index`'s tag: one (`boundary_tag_text`), or
/// for a deployment node its name then `[Deployment Node: <technology>]`
/// (`[Deployment Node]` without one), the type line left out under
/// `metadata false` (P3-114, P2-411).
pub(crate) fn boundary_tag_lines(plan: &ViewPlan, index: usize, words: &Words<'_>) -> Vec<String> {
    let boundary = &plan.boundaries[index];
    if !matches!(boundary.of, BoundaryOf::DeploymentNode(_)) {
        return vec![boundary_tag_text(plan, index, words)];
    }
    let mut lines = vec![boundary.name.clone()];
    if boundary.style.metadata {
        lines.push(match boundary.technology.as_deref().map(str::trim) {
            Some(t) if !t.is_empty() => format!("[{}: {t}]", words.deployment_node),
            _ => format!("[{}]", words.deployment_node),
        });
    }
    lines
}

/// The name-tag size in px of `boundary`: its style's `fontSize`, else
/// 12 (P2-411). `scene` draws the tag at it.
pub(crate) fn boundary_tag_font_px(boundary: &crate::view::Boundary) -> i32 {
    boundary
        .style
        .font_size
        .map_or(TAG_FONT_PX, |n| i32::try_from(n).unwrap_or(TAG_FONT_PX).clamp(1, 1000))
}

/// The name-tag text of `boundary`: the name, then the scope's type in
/// brackets unless its style says `metadata false` (P2-411). `scene`
/// draws exactly this text.
pub(crate) fn boundary_tag_text(plan: &ViewPlan, index: usize, words: &Words<'_>) -> String {
    let boundary = &plan.boundaries[index];
    if boundary.style.metadata {
        tag_text(plan, index, words)
    } else {
        boundary.name.clone()
    }
}

// DECISION: the boundary name tag (CORE-1 gives its text, not its size):
// 12px Inter Bold, as the prototype's 12.5px rounded to a whole px; its
// rectangle starts at the boundary's left edge and spans the bottom 30px
// (the prototype's tag, `y1 - 30 .. y1 - 2`, rounded outward); `scene`
// draws the text from 12px right of the boundary's left edge with its
// baseline 12px above the bottom edge (prototype `x0 + 12, y1 - 12`),
// and 8px of room stay right of the text. The 40px boundary margin
// keeps every member box clear of it.
const TAG_FONT_PX: i32 = 12;
const TAG_TEXT_INSET_PX: i32 = 12;
const TAG_PAD_RIGHT_PX: i32 = 8;
const TAG_HEIGHT_UNITS: i32 = 3;

/// The text of boundary `index`'s tag: the name, then the scope's type
/// in brackets (CORE-1); a group's tag is its name alone (P2-130).
// DECISION: the type follows the view kind (phase 1's one boundary is
// the scope's): a container view's scope is a software system, a
// component view's a container; a context view draws no boundary in
// phase 1, and its scope is a software system too.
pub(crate) fn tag_text(plan: &ViewPlan, index: usize, words: &Words<'_>) -> String {
    let boundary = &plan.boundaries[index];
    if matches!(boundary.of, BoundaryOf::Group(_)) {
        return boundary.name.clone();
    }
    if matches!(boundary.of, BoundaryOf::DeploymentNode(_)) {
        return boundary_tag_lines(plan, index, words).join(" ");
    }
    if plan.kind == ViewKind::Dynamic {
        // P3-214: the scope's software system first, then (container
        // scope) the container inside it.
        let kind = if index == 0 { words.software_system } else { words.container };
        return format!("{} [{kind}]", boundary.name);
    }
    let kind = match plan.kind {
        ViewKind::Component => words.container,
        // A filtered view keeps its base view's scope boundary (P2-9): a
        // container's when it surrounds components (component base).
        ViewKind::Filtered => {
            let components = boundary.members.iter().any(|m| {
                plan.boxes
                    .get(m.0 as usize)
                    .is_some_and(|b| b.kind == crate::model::ElementKind::Component)
            });
            if components {
                words.container
            } else {
                words.software_system
            }
        }
        // DECISION: landscape, custom and image views draw no scope
        // boundary (P2-8, P2-19), and dynamic and deployment views are not
        // drawn in phase 2; a scope boundary of one would surround a
        // software system's boxes, as on context and container views.
        ViewKind::SystemContext
        | ViewKind::Container
        | ViewKind::SystemLandscape
        | ViewKind::Custom
        | ViewKind::Image
        | ViewKind::Dynamic
        | ViewKind::Deployment => words.software_system,
    };
    format!("{} [{kind}]", boundary.name)
}

/// The type words and brackets texts are drawn with: the CORE-1/CORE-2
/// words, or those of the model's `terminology` block (P2-161..P2-163).
/// `pipeline` builds it from `Model::terminology` and passes it to
/// `place_labels_with` and `scene::build_with`; the pub `place_labels`,
/// `boundary_tag`, `scene::build` and `scene::texts_fit` use `DEFAULT`.
// DECISION: terminology reaches `label` and `scene` as a crate-private
// argument from `pipeline`, not a `ViewPlan` field, so no pub signature
// or layout changes (docs/deferred.md had it as a pub `ViewPlan` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Words<'a> {
    pub(crate) person: &'a str,
    pub(crate) software_system: &'a str,
    pub(crate) container: &'a str,
    pub(crate) component: &'a str,
    /// `deploymentNode` (P2-161, P3-114).
    pub(crate) deployment_node: &'a str,
    /// `infrastructureNode` (P2-161, P3-116).
    pub(crate) infrastructure_node: &'a str,
    /// The word of every key line entry (P2-162).
    pub(crate) relationship: &'a str,
    /// The box type line's brackets (P2-163).
    pub(crate) metadata: MetadataSymbols,
}

impl<'a> Words<'a> {
    /// The phase-1 words: Person, Software System, Container, Component,
    /// Relationship, square brackets.
    pub(crate) const DEFAULT: Words<'static> = Words {
        person: "Person",
        software_system: "Software System",
        container: "Container",
        component: "Component",
        deployment_node: "Deployment Node",
        infrastructure_node: "Infrastructure Node",
        relationship: "Relationship",
        metadata: MetadataSymbols::Square,
    };

    /// The words of `t`, each unwritten one the default.
    pub(crate) fn of(t: &'a Terminology) -> Self {
        let d = Words::DEFAULT;
        let word = |l: &'a Option<crate::model::Literal>, default: &'static str| {
            l.as_ref().map_or(default, |l| l.value.as_str())
        };
        Words {
            person: word(&t.person, d.person),
            software_system: word(&t.software_system, d.software_system),
            container: word(&t.container, d.container),
            component: word(&t.component, d.component),
            deployment_node: word(&t.deployment_node, d.deployment_node),
            infrastructure_node: word(&t.infrastructure_node, d.infrastructure_node),
            relationship: word(&t.relationship, d.relationship),
            metadata: t.metadata.as_ref().map_or(d.metadata, |m| m.value),
        }
    }

    /// The type word of a box of `kind` (`External <word>` for an external
    /// software system, B dsl-breadth 18b); `None` for a custom element.
    pub(crate) fn kind(&self, kind: ElementKind, external: bool) -> Option<String> {
        Some(match kind {
            ElementKind::Person => self.person.to_owned(),
            // P3-116: an instance is typed as its element (Structurizr
            // 6.2.4 PlantUML: `[Container: Java]`), an infrastructure node
            // as itself.
            ElementKind::SoftwareSystem | ElementKind::SoftwareSystemInstance if external => {
                format!("External {}", self.software_system)
            }
            ElementKind::SoftwareSystem | ElementKind::SoftwareSystemInstance => self.software_system.to_owned(),
            ElementKind::Container | ElementKind::ContainerInstance => self.container.to_owned(),
            ElementKind::Component => self.component.to_owned(),
            ElementKind::InfrastructureNode => self.infrastructure_node.to_owned(),
            ElementKind::DeploymentNode => self.deployment_node.to_owned(),
            _ => return None,
        })
    }

    /// `inner` in the box type line's brackets (P2-163).
    // DECISION: only the box type line takes them (P2-163 names only it);
    // boundary name tags keep `[...]`.
    pub(crate) fn bracket(&self, inner: &str) -> String {
        let (open, close) = match self.metadata {
            MetadataSymbols::Round => ("(", ")"),
            MetadataSymbols::Curly => ("{", "}"),
            MetadataSymbols::Angle => ("<", ">"),
            MetadataSymbols::DoubleAngle => ("\u{ab}", "\u{bb}"),
            MetadataSymbols::None => ("", ""),
            _ => ("[", "]"),
        };
        format!("{open}{inner}{close}")
    }
}

/// Places the label of every routed edge of `plan` on its own line, with
/// every box at `routes.centres[box]` and lines from `routes`. Edges are
/// placed in ascending `EdgeId`. Candidate centres are the line's grid
/// points (on any segment, so a label may straddle a bend) at least
/// `START_GAP_PX` along the line from its start and `ARROW_GAP_PX` back
/// from its arrowhead, ordered by distance along the line from half its
/// length (or from `position` percent of it, P2-425), ties to the one
/// nearer the start. The first candidate clear
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
    place_labels_with(plan, routes, &Words::DEFAULT)
}

/// `place_labels` with boundary tags sized from `words` (P2-161).
pub(crate) fn place_labels_with(plan: &ViewPlan, routes: &Routes, words: &Words<'_>) -> Labels {
    assert_eq!(
        routes.lines.len(),
        plan.edges.len(),
        "place_labels: routes of another plan (lines)"
    );
    assert_eq!(
        routes.centres.len(),
        plan.boxes.len(),
        "place_labels: routes of another plan (centres)"
    );
    let scene = Obstacles::new(plan, routes, words);
    let mut labels: Vec<Option<Label>> = Vec::with_capacity(plan.edges.len());
    let mut shared = Vec::new();
    for edge in &plan.edges {
        let Some(line) = routes.line(edge.id) else {
            labels.push(None);
            continue;
        };
        let lines = label_lines(edge, &plan.options);
        if lines.is_empty() {
            labels.push(None);
            continue;
        }
        if let Some(trunk) = line.joins() {
            let t = &plan.edges[trunk.0 as usize];
            if label_lines(t, &plan.options) == lines {
                shared.push((edge.id, trunk));
                labels.push(None);
                continue;
            }
        }
        let (font_px, line_px) = (label_font_px(edge), label_line_px(edge));
        let (width_px, height_px) = label_size(&lines, font_px, line_px);
        let mut label = Label {
            edge: edge.id,
            lines,
            centre: Point::default(),
            width_px,
            height_px,
            fit: Fit::NoRoom,
            font_px,
            line_px,
        };
        let spots = walk(&line.points);
        let placed: Vec<Rect> = labels.iter().flatten().map(Label::px_rect).collect();
        // Best so far: the spot, the lines it covers, and whether it sits
        // on a boundary stroke (only ever true on a dynamic view).
        let mut best: Option<(Point, Vec<EdgeId>, bool)> = None;
        for c in candidates(&spots, line.joins().is_some(), edge.style.position) {
            label.centre = c;
            let rect = label.px_rect();
            if scene.blocked(&rect) || placed.iter().any(|p| p.overlaps(&rect)) {
                continue;
            }
            let covered = scene.covered(&rect, edge.id);
            let on_stroke = scene.on_stroke(&rect);
            if covered.is_empty() && !on_stroke {
                best = Some((c, covered, on_stroke));
                break;
            }
            if best
                .as_ref()
                .is_none_or(|(_, b, s)| (covered.len(), on_stroke) < (b.len(), *s))
            {
                best = Some((c, covered, on_stroke));
            }
        }
        (label.centre, label.fit) = match best {
            Some((c, covered, _)) if covered.is_empty() => (c, Fit::Clear),
            Some((c, covered, _)) => (c, Fit::OverLines(covered)),
            None => (middle(&spots), Fit::NoRoom),
        };
        labels.push(Some(label));
    }
    Labels { labels, shared }
}

/// What a label keeps clear of, apart from the labels placed before it:
/// boxes and boundary tags (never covered by a chosen spot) and every
/// drawn line (covered only as `Fit::OverLines`). All in px.
struct Obstacles {
    boxes: Vec<Rect>,
    tags: Vec<Rect>,
    /// The four sides of every boundary as thin px bands, on a dynamic
    /// view only; empty on every other kind.
    strokes: Vec<Rect>,
    /// Per edge, ascending, the segments drawn for it, in px.
    lines: Vec<(EdgeId, Vec<Segment>)>,
}

impl Obstacles {
    fn new(plan: &ViewPlan, routes: &Routes, words: &Words<'_>) -> Self {
        let centres = &routes.centres;
        let boxes = plan
            .boxes
            .iter()
            .map(|b| rect_px(&b.rect_at(centres[b.id.0 as usize])))
            .collect();
        let tags = (0..plan.boundaries.len())
            .filter_map(|i| boundary_tag_with(plan, i, centres, words))
            .map(|t| rect_px(&t))
            .collect();
        // DECISION: an unroutable line is still drawn (CORE-13: straight,
        // on the line between its box centres), so labels keep clear of
        // it like any other line; the part of that segment inside its two
        // boxes changes nothing, as no chosen spot overlaps a box.
        let lines = plan
            .edges
            .iter()
            .map(|e| {
                let segs = routes.line(e.id).map_or_else(
                    || {
                        vec![Segment::new(
                            centres[e.source.0 as usize],
                            centres[e.destination.0 as usize],
                        )]
                    },
                    Line::segments,
                );
                let segs = segs
                    .iter()
                    .map(|s| Segment::new(point_px(s.a), point_px(s.b)))
                    .collect();
                (e.id, segs)
            })
            .collect();
        // DECISION: on a dynamic view a label keeps off the dashed scope
        // boundaries where a spot allows (approver finding on the checkout
        // render): a spot on a stroke is taken only when every other spot
        // covers more lines, and it never changes the fit. Limited to
        // dynamic views so the static views' placement (and the goldens)
        // stays exactly as before.
        let strokes = if plan.kind == crate::model::ViewKind::Dynamic {
            (0..plan.boundaries.len())
                .filter_map(|i| plan.boundary_rect(i, centres))
                .flat_map(|r| stroke_bands(&rect_px(&r)))
                .collect()
        } else {
            Vec::new()
        };
        Self {
            boxes,
            tags,
            strokes,
            lines,
        }
    }

    /// True when the px `rect` lies across a boundary stroke (dynamic
    /// views only).
    fn on_stroke(&self, rect: &Rect) -> bool {
        self.strokes.iter().any(|s| s.overlaps(rect))
    }

    /// True when the px `rect` overlaps a box or a boundary tag.
    fn blocked(&self, rect: &Rect) -> bool {
        self.boxes
            .iter()
            .chain(&self.tags)
            .any(|b| b.overlaps(rect))
    }

    /// The lines other than `own` with a point strictly inside the px
    /// `rect`, ascending.
    // DECISION: a line running along the drawn rectangle's border, or
    // touching it at one point, is not covered (`Segment::passes_through`
    // on px coordinates): it at most grazes the label's edge.
    fn covered(&self, rect: &Rect, own: EdgeId) -> Vec<EdgeId> {
        self.lines
            .iter()
            .filter(|(e, segs)| *e != own && segs.iter().any(|s| s.passes_through(rect)))
            .map(|(e, _)| *e)
            .collect()
    }
}

// DECISION: a label keeps 2px off a boundary stroke, so its white
// rectangle never cuts the dashes.
const STROKE_CLEAR_PX: i32 = 2;

/// The four sides of the px rectangle `r` as bands `STROKE_CLEAR_PX`
/// wide on each side of the stroke.
fn stroke_bands(r: &Rect) -> [Rect; 4] {
    let c = STROKE_CLEAR_PX;
    let band = |x0, y0, x1, y1| Rect::from_corners(Point::new(x0, y0), Point::new(x1, y1));
    [
        band(r.min.x - c, r.min.y - c, r.max.x + c, r.min.y + c),
        band(r.min.x - c, r.max.y - c, r.max.x + c, r.max.y + c),
        band(r.min.x - c, r.min.y - c, r.min.x + c, r.max.y + c),
        band(r.max.x - c, r.min.y - c, r.max.x + c, r.max.y + c),
    ]
}

/// A grid point in px.
const fn point_px(p: Point) -> Point {
    Point::new(grid_to_px(p.x), grid_to_px(p.y))
}

/// A grid rectangle in px.
const fn rect_px(r: &Rect) -> Rect {
    Rect::from_corners(point_px(r.min), point_px(r.max))
}

/// `px` rounded up to the next grid unit (towards +infinity).
const fn ceil_grid(px: i32) -> i32 {
    -(-px).div_euclid(GRID_PX)
}

/// Rectangle size in px of a label of `lines` (CORE-14 font, pitch and
/// paddings).
fn label_size(lines: &[String], font_px: i32, line_px: i32) -> (i32, i32) {
    let widest = lines
        .iter()
        .map(|s| text_advance(s, Weight::Regular))
        .max()
        .unwrap_or(0);
    let text_px = (widest * font_px + EM - 1) / EM;
    let count = i32::try_from(lines.len()).unwrap_or(i32::MAX / line_px.max(1));
    (
        text_px + 2 * LABEL_PAD_X_PX,
        count * line_px + 2 * LABEL_PAD_Y_PX,
    )
}

// DECISION: distances along a line are integers in thousandths of a px:
// a straight grid step is exactly 10_000, a diagonal one 14_142 (10px
// times sqrt 2, truncated). Exact, deterministic integer arithmetic
// (CORE-25); the truncation never decides a 30px or 50px gap, as no
// count of diagonal steps lands within 0.001px of a multiple of 10px.
const STRAIGHT_MILLI_PX: i64 = 10_000;
const DIAGONAL_MILLI_PX: i64 = 14_142;
const MILLI: i64 = 1_000;

/// Every grid point of the polyline `points` with its distance along
/// the line from the start, in thousandths of a px, start to end.
fn walk(points: &[Point]) -> Vec<(Point, i64)> {
    let mut out = Vec::new();
    let Some(&first) = points.first() else {
        return out;
    };
    out.push((first, 0));
    let mut dist = 0;
    for w in points.windows(2) {
        let (dx, dy) = ((w[1].x - w[0].x).signum(), (w[1].y - w[0].y).signum());
        let step = if dx != 0 && dy != 0 {
            DIAGONAL_MILLI_PX
        } else {
            STRAIGHT_MILLI_PX
        };
        let mut p = w[0];
        while p != w[1] {
            p = Point::new(p.x + dx, p.y + dy);
            dist += step;
            out.push((p, dist));
        }
    }
    out
}

/// Sort key of a spot: distance from half the line's length (doubled, to
/// stay whole), then distance from the start.
fn by_middle(total: i64) -> impl Fn(&(Point, i64)) -> (i64, i64) {
    move |&(_, d)| ((2 * d - total).abs(), d)
}

/// Sort key of a spot for a label at `percent` of the line's length
/// (P2-425): distance from that point (times 100, to stay whole), then
/// distance from the start. At 50 it orders as `by_middle`.
fn by_position(total: i64, percent: u8) -> impl Fn(&(Point, i64)) -> (i64, i64) {
    let p = i64::from(percent.min(100));
    move |&(_, d)| ((100 * d - p * total).abs(), d)
}

/// The candidate centres of a line walked by `walk`, best first: at
/// least `START_GAP_PX` from the start and, unless the line ends in a
/// join dot, `ARROW_GAP_PX` back from its end; ordered by distance from
/// the middle, ties to the one nearer the start.
fn candidates(spots: &[(Point, i64)], joined: bool, position: Option<u8>) -> Vec<Point> {
    let total = spots.last().map_or(0, |&(_, d)| d);
    let start = i64::from(START_GAP_PX) * MILLI;
    let end = if joined {
        0
    } else {
        i64::from(ARROW_GAP_PX) * MILLI
    };
    let mut keep: Vec<(Point, i64)> = spots
        .iter()
        .copied()
        .filter(|&(_, d)| d >= start && total - d >= end)
        .collect();
    match position {
        // No `position`: exactly phase 1 (P2-425).
        None => keep.sort_by_key(by_middle(total)),
        Some(p) => keep.sort_by_key(by_position(total, p)),
    }
    keep.into_iter().map(|(p, _)| p).collect()
}

/// The grid point of a walked line nearest half its length, ties to the
/// one nearer the start (CORE-15 "middle of its line").
fn middle(spots: &[(Point, i64)]) -> Point {
    let total = spots.last().map_or(0, |&(_, d)| d);
    spots
        .iter()
        .min_by_key(|s| by_middle(total)(s))
        .map_or(Point::default(), |&(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::geom::{Segment, grid_to_px};
    use crate::model::{ViewKind, parse};
    use crate::route::{Join, Line, LineEnd, Port, Side};
    use crate::view::{Boundary, BoundaryOf, BoxId, plan};

    /// A two-box context plan written with roles only: the box and edge
    /// templates every test plan is cloned from.
    fn template() -> ViewPlan {
        let src = "workspace {\n  model {\n    operator = person \"Operator\"\n    shop = softwareSystem \"Shop\"\n    operator -> shop \"Uses\"\n  }\n  views {\n    systemContext shop \"ctx\" {\n      include *\n    }\n  }\n}\n";
        let model = match parse(src) {
            Ok(p) => p.model,
            Err(e) => panic!("{e}"),
        };
        plan(&model, model.view("ctx").expect("view"))
    }

    fn id(i: usize) -> u32 {
        u32::try_from(i).expect("small")
    }

    fn opt(s: &str) -> Option<String> {
        (!s.is_empty()).then(|| s.to_owned())
    }

    /// A plan of 80x60px boxes and edges `(source, destination,
    /// description, technology)`, an empty text meaning none; no
    /// boundary.
    fn mk(boxes: usize, edges: &[(u32, u32, &str, &str)]) -> ViewPlan {
        let mut p = template();
        let tb = p.boxes[1].clone();
        let te = p.edges[0].clone();
        p.boxes = (0..boxes)
            .map(|i| {
                let mut b = tb.clone();
                b.id = BoxId(id(i));
                b.half_width = 4;
                b.half_height = 3;
                b.centre = None;
                b
            })
            .collect();
        p.edges = edges
            .iter()
            .enumerate()
            .map(|(i, &(s, d, desc, tech))| {
                let mut e = te.clone();
                e.id = EdgeId(id(i));
                e.source = BoxId(s);
                e.destination = BoxId(d);
                e.description = opt(desc);
                e.technology = opt(tech);
                e
            })
            .collect();
        p.boundaries = Vec::new();
        p
    }

    fn pt(x: i32, y: i32) -> Point {
        Point::new(x, y)
    }

    /// Box centres far from every test line (rows of boxes at y = 100).
    fn far(n: usize) -> Vec<Point> {
        (0..n)
            .map(|i| pt(100 + 20 * id(i).cast_signed(), 100))
            .collect()
    }

    fn port(at: Point) -> Port {
        Port { at, side: Side::E }
    }

    /// An arrowed line of edge `e` through `pts`.
    #[expect(clippy::unnecessary_wraps, reason = "a routed slot of `Routes::lines`")]
    fn line(e: u32, pts: &[(i32, i32)]) -> Option<Line> {
        let points: Vec<Point> = pts.iter().map(|&(x, y)| pt(x, y)).collect();
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
        let points: Vec<Point> = pts.iter().map(|&(x, y)| pt(x, y)).collect();
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

    fn routes(centres: Vec<Point>, lines: Vec<Option<Line>>) -> Routes {
        Routes {
            lines,
            centres,
            crossings: 0,
        }
    }

    /// Every grid point of a polyline, vertices included.
    fn grid_points(points: &[Point]) -> Vec<Point> {
        let mut out = vec![points[0]];
        for w in points.windows(2) {
            let (dx, dy) = ((w[1].x - w[0].x).signum(), (w[1].y - w[0].y).signum());
            let mut p = w[0];
            while p != w[1] {
                p = pt(p.x + dx, p.y + dy);
                out.push(p);
            }
        }
        out
    }

    fn label(labels: &Labels, e: u32) -> &Label {
        labels.label(EdgeId(e)).expect("labelled")
    }

    /// The label at `centre` with `label`'s size: what a candidate spot
    /// would cover.
    fn moved(label: &Label, centre: Point) -> Rect {
        let mut l = label.clone();
        l.centre = centre;
        l.bounds()
    }

    fn covers(rect: &Rect, line: &Line) -> bool {
        line.segments().iter().any(|s| s.passes_through(rect))
    }

    /// Whether `line`, scaled to px, has a point strictly inside the px
    /// `rect`.
    fn covers_px(rect: &Rect, line: &Line) -> bool {
        line.segments()
            .iter()
            .any(|s| Segment::new(point_px(s.a), point_px(s.b)).passes_through(rect))
    }

    /// Bounds of a label 43px x 19px (one line `Reads`) at `c`.
    fn reads_at(c: Point) -> Rect {
        Rect::from_corners(pt(c.x - 3, c.y - 1), pt(c.x + 3, c.y + 1))
    }

    #[test]
    fn core_14_wrapped_at_label_width() {
        // Description wrapped at `labelWidth` characters, then
        // `[technology]` on its own line; a long word stays whole.
        let p = mk(
            2,
            &[(
                0,
                1,
                "Reads the current stock levels of every warehouse item",
                "SQL",
            )],
        );
        assert_eq!(
            label_lines(&p.edges[0], &p.options),
            [
                "Reads the current stock",
                "levels of every",
                "warehouse item",
                "[SQL]"
            ]
        );
        let mut narrow = p.options;
        narrow.label_width = 10;
        let mut e = p.edges[0].clone();
        e.description = Some("Reconciliation  of\tledgers".into());
        e.technology = None;
        assert_eq!(label_lines(&e, &narrow), ["Reconciliation", "of ledgers"]);
        // Width counts characters, not bytes: 10 characters, 12 bytes.
        e.description = Some("Grüße über".into());
        assert_eq!(label_lines(&e, &narrow), ["Grüße über"]);
        e.description = Some("Grüße überall".into());
        assert_eq!(label_lines(&e, &narrow), ["Grüße", "überall"]);
        // Only a technology: one bracketed line.
        e.description = None;
        e.technology = Some("Message queue".into());
        assert_eq!(label_lines(&e, &narrow), ["[Message queue]"]);
        // The placed label carries the same lines and is sized from them.
        let r = routes(far(2), vec![line(0, &[(0, 0), (40, 0)])]);
        let labels = place_labels(&p, &r);
        let l = label(&labels, 0);
        assert_eq!(l.lines, label_lines(&p.edges[0], &p.options));
        let widest = l
            .lines
            .iter()
            .map(|s| text_advance(s, Weight::Regular))
            .max()
            .expect("lines");
        assert_eq!(
            l.width_px,
            (widest * LABEL_FONT_PX + EM - 1) / EM + 2 * LABEL_PAD_X_PX
        );
        assert_eq!(l.height_px, 4 * LABEL_LINE_PX + 2 * LABEL_PAD_Y_PX);
    }

    #[test]
    fn core_14_label_on_its_line() {
        // Centre is a grid point of its own line.
        let p = mk(2, &[(0, 1, "Reads", ""), (0, 1, "Writes", "")]);
        let r = routes(
            far(2),
            vec![
                line(0, &[(0, 0), (6, 0), (10, 4), (10, 20)]),
                line(1, &[(-10, 30), (-4, 30), (2, 24), (30, 24)]),
            ],
        );
        let labels = place_labels(&p, &r);
        for e in 0..2 {
            let l = label(&labels, e);
            assert_eq!(l.edge, EdgeId(e));
            let own = r.line(EdgeId(e)).expect("routed");
            assert!(
                grid_points(&own.points).contains(&l.centre),
                "edge {e}: centre {:?} not on its line",
                l.centre
            );
        }
    }

    #[test]
    fn core_14_gaps_from_start_and_arrowhead() {
        // >= 30px from start, >= 50px from the arrowhead; a joined line
        // has no arrowhead gap at its dot.
        let p = mk(
            2,
            &[
                (0, 1, "Reads", ""),
                (0, 1, "Reads", ""),
                (0, 1, "Reads", ""),
                (0, 1, "Writes", ""),
            ],
        );
        // Edge 3 is the trunk edge 2 joins, drawn far away: only the gap
        // rule is under test here.
        let r = routes(
            far(2),
            vec![
                // 80px: one spot, 30px from the start and 50px from the end.
                line(0, &[(0, 0), (8, 0)]),
                // 70px: no spot keeps both gaps.
                line(1, &[(0, 10), (7, 10)]),
                // 40px, ending in a dot: 30px and 40px both qualify.
                joined(2, &[(0, 20), (4, 20)], 3),
                line(3, &[(0, 40), (30, 40)]),
            ],
        );
        let labels = place_labels(&p, &r);
        assert_eq!(label(&labels, 0).centre, pt(3, 0));
        assert_eq!(label(&labels, 0).fit, Fit::Clear);
        // No spot: drawn at the middle (35px: 30px and 40px tie, the one
        // nearer the start wins) and reported.
        assert_eq!(label(&labels, 1).centre, pt(3, 10));
        assert_eq!(label(&labels, 1).fit, Fit::NoRoom);
        // Joined: 30px is nearer the middle (20px) than 40px.
        assert_eq!(label(&labels, 2).centre, pt(3, 20));
        assert_eq!(label(&labels, 2).fit, Fit::Clear);
        // An arrowed line of the same 40px has no spot.
        let r2 = routes(
            far(2),
            vec![
                line(0, &[(0, 0), (8, 0)]),
                line(1, &[(0, 10), (7, 10)]),
                line(2, &[(0, 20), (4, 20)]),
                line(3, &[(0, 40), (30, 40)]),
            ],
        );
        assert_eq!(label(&place_labels(&p, &r2), 2).fit, Fit::NoRoom);
    }

    #[test]
    fn core_14_label_clear_of_boxes_labels_lines() {
        // Edge 1's line runs from x 0 to x 30 at y 0; its labels are
        // 43px wide, compared in px. Edge 2's line blocks centres x 4..8,
        // edge 0's label (placed first, centre x 13) blocks x 9..17, a
        // box blocks x 18..25: only x 3 is clear.
        let p = mk(
            3,
            &[
                (0, 1, "Reads", ""),
                (0, 1, "Reads", ""),
                (0, 1, "Reads", ""),
            ],
        );
        let mut centres = far(3);
        centres[2] = pt(22, -3);
        let mut p = p;
        p.boxes[2].half_width = 2;
        p.boxes[2].half_height = 3;
        let r = routes(
            centres,
            vec![
                line(0, &[(7, 1), (19, 1)]),
                line(1, &[(0, 0), (30, 0)]),
                line(2, &[(6, -5), (6, 5)]),
            ],
        );
        let labels = place_labels(&p, &r);
        assert_eq!(label(&labels, 0).centre, pt(13, 1));
        let l = label(&labels, 1);
        assert_eq!(l.centre, pt(3, 0));
        assert_eq!(l.fit, Fit::Clear);
        assert_eq!(l.bounds(), reads_at(pt(3, 0)));
        // The rule itself, on every placed label.
        let all: Vec<&Label> = labels.labels.iter().flatten().collect();
        for a in &all {
            if a.fit != Fit::Clear {
                continue;
            }
            for (i, b) in p.boxes.iter().enumerate() {
                assert!(!a.px_rect().overlaps(&rect_px(&b.rect_at(r.centres[i]))));
            }
            for b in &all {
                if a.edge != b.edge {
                    assert!(!a.px_rect().overlaps(&b.px_rect()));
                }
            }
            for other in r.lines.iter().flatten() {
                if other.edge != a.edge {
                    assert!(!covers_px(&a.px_rect(), other));
                }
            }
        }
        // The middle was blocked: the label moved for a reason.
        let mut mid = l.clone();
        mid.centre = pt(15, 0);
        assert!(mid.px_rect().overlaps(&label(&labels, 0).px_rect()));
    }

    #[test]
    fn core_14_clear_of_boundary_tag() {
        let mut p = mk(2, &[(0, 1, "Reads", "")]);
        p.kind = ViewKind::Container;
        p.boundaries = vec![Boundary {
            of: BoundaryOf::Element(p.boxes[0].element),
            instances: None,
            technology: None,
            name: "Shop".into(),
            members: vec![BoxId(0)],
            style: crate::model::ResolvedBoundaryStyle::DEFAULT,
        }];
        let mut centres = far(2);
        centres[0] = pt(0, 0);
        // Boundary (-8,-7)..(8,7); the line runs inside it at y 6, over
        // the tag's height.
        let r = routes(centres.clone(), vec![line(0, &[(-20, 6), (10, 6)])]);
        let tag = boundary_tag(&p, 0, &centres).expect("tag");
        let labels = place_labels(&p, &r);
        let l = label(&labels, 0);
        assert_eq!(l.fit, Fit::Clear);
        assert!(!l.bounds().overlaps(&tag));
        // The middle spot is under the tag: the label moved for it.
        assert!(moved(l, pt(-5, 6)).overlaps(&tag));
        assert_ne!(l.centre, pt(-5, 6));
    }

    #[test]
    fn core_14_joined_line_same_text_has_no_label() {
        // Listed in `shared`; a joined line with other text keeps its
        // label on its own branch, not on the trunk (Review Focus). Same
        // description, different technology under `technology off`:
        // the drawn labels match, so the joined line shares (CORE-14
        // compares the trunk's label, CORE-4 hides the technology).
        let lines = || {
            vec![
                line(0, &[(20, -15), (20, 5)]),
                joined(1, &[(0, 0), (20, 0)], 0),
            ]
        };
        let same = mk(2, &[(0, 1, "Reads", "SQL"), (0, 1, "Reads", "SQL")]);
        let labels = place_labels(&same, &routes(far(2), lines()));
        assert_eq!(labels.shared, [(EdgeId(1), EdgeId(0))]);
        assert!(labels.labels[1].is_none());
        assert!(labels.labels[0].is_some());

        let other = mk(2, &[(0, 1, "Reads", "SQL"), (0, 1, "Writes", "SQL")]);
        let r = routes(far(2), lines());
        let labels = place_labels(&other, &r);
        assert!(labels.shared.is_empty());
        let l = label(&labels, 1);
        assert_eq!(l.fit, Fit::Clear);
        let own = r.line(EdgeId(1)).expect("routed");
        let trunk = r.line(EdgeId(0)).expect("routed");
        assert!(grid_points(&own.points).contains(&l.centre));
        assert!(!grid_points(&trunk.points).contains(&l.centre));
        assert!(!covers(&l.bounds(), trunk));

        let mut tech_off = mk(2, &[(0, 1, "Reads", "HTTPS"), (0, 1, "Reads", "gRPC")]);
        tech_off.options.technology = false;
        let labels = place_labels(&tech_off, &routes(far(2), lines()));
        assert_eq!(labels.shared, [(EdgeId(1), EdgeId(0))]);
        assert_eq!(label(&labels, 0).lines, ["Reads"]);
        assert!(labels.labels[1].is_none());
    }

    #[test]
    fn core_14_long_label_straddles_bend_only_if_clear() {
        // Label longer than any straight run: centred across a bend when
        // that spot is clear, else reported (Review Focus).
        let path = [(0, 0), (5, 0), (8, 3), (8, 10)];
        let p = mk(2, &[(0, 1, "Sends nightly reconciliation", "")]);
        let r = routes(far(2), vec![line(0, &path)]);
        let labels = place_labels(&p, &r);
        let l = label(&labels, 0);
        let own = r.line(EdgeId(0)).expect("routed");
        for s in own.segments() {
            let len_sq = s.len_px_squared().expect("octilinear");
            assert!(i64::from(l.width_px).pow(2) > len_sq, "{s:?} longer");
        }
        assert_eq!(l.centre, pt(7, 2));
        assert_eq!(l.fit, Fit::Clear);
        // The label rectangle spans the bend at (8, 3).
        let (cx, cy) = (grid_to_px(l.centre.x), grid_to_px(l.centre.y));
        let (left, top) = (cx - l.width_px / 2, cy - l.height_px / 2);
        let bend = (grid_to_px(8), grid_to_px(3));
        assert!(left < bend.0 && bend.0 < left + l.width_px);
        assert!(top < bend.1 && bend.1 < top + l.height_px);

        // Two lines crossing every spot: not clear, so reported.
        let p = mk(
            2,
            &[
                (0, 1, "Sends nightly reconciliation", ""),
                (0, 1, "Reads", ""),
                (0, 1, "Reads", ""),
            ],
        );
        let r = routes(
            far(2),
            vec![
                line(0, &path),
                line(1, &[(-10, 1), (30, 1)]),
                line(2, &[(-10, 4), (30, 4)]),
            ],
        );
        let labels = place_labels(&p, &r);
        assert_ne!(label(&labels, 0).fit, Fit::Clear);
        assert!(labels.over_lines().contains(&EdgeId(0)) || labels.no_room().contains(&EdgeId(0)));
    }

    #[test]
    fn core_14_placed_in_edge_order() {
        // Two edges competing for one spot: the lower `EdgeId` gets it.
        let p = mk(2, &[(0, 1, "Reads", ""), (0, 1, "Reads", "")]);
        let a = [(0, 0), (20, 0)];
        let b = [(0, 1), (20, 1)];
        let labels = place_labels(&p, &routes(far(2), vec![line(0, &a), line(1, &b)]));
        assert_eq!(label(&labels, 0).centre, pt(10, 0));
        // Nearest the middle clear of label 0 in px: x 5 (x 15 ties, the
        // one nearer the start wins).
        assert_eq!(label(&labels, 1).centre, pt(5, 1));
        assert_eq!(label(&labels, 1).fit, Fit::Clear);
        // Swapped ids: the line at y 1 now comes first and takes the middle.
        let labels = place_labels(&p, &routes(far(2), vec![line(0, &b), line(1, &a)]));
        assert_eq!(label(&labels, 0).centre, pt(10, 1));
        assert_ne!(label(&labels, 1).centre, pt(10, 0));
    }

    #[test]
    fn core_14_no_text_no_label() {
        // No description and no shown technology: `labels[e]` is `None`
        // and `e` is not in `shared` (lint L12 reads the plan).
        let mut p = mk(2, &[(0, 1, "", ""), (0, 1, "", ""), (0, 1, "", "SQL")]);
        p.options.technology = false;
        let r = routes(
            far(2),
            vec![
                line(0, &[(20, -15), (20, 5)]),
                joined(1, &[(0, 0), (20, 0)], 0),
                line(2, &[(0, 30), (30, 30)]),
            ],
        );
        let labels = place_labels(&p, &r);
        assert_eq!(labels.labels, [None, None, None]);
        assert!(labels.shared.is_empty());
        assert!(label_lines(&p.edges[2], &p.options).is_empty());
    }

    #[test]
    fn core_14_bounds_round_px_rect_outward() {
        // A px rectangle not on the grid: `bounds()` contains it and is
        // the smallest grid rectangle that does.
        let l = Label {
            edge: EdgeId(0),
            lines: vec!["Reads".into()],
            centre: pt(3, 2),
            width_px: 43,
            height_px: 19,
            fit: Fit::Clear,
            font_px: LABEL_FONT_PX,
            line_px: LABEL_LINE_PX,
        };
        // px rectangle: x 9..52, y 11..30.
        assert_eq!(l.bounds(), Rect::from_corners(pt(0, 1), pt(6, 3)));
        // Negative coordinates round outward too (floor, not truncation).
        let mut n = l.clone();
        n.centre = pt(-3, -2);
        // px rectangle: x -51..-8, y -29..-10.
        assert_eq!(n.bounds(), Rect::from_corners(pt(-6, -3), pt(0, -1)));
        // A px rectangle already on the grid is kept as is.
        let mut g = l;
        g.width_px = 40;
        g.height_px = 20;
        assert_eq!(g.bounds(), Rect::from_corners(pt(1, 1), pt(5, 3)));
    }

    #[test]
    fn core_1_boundary_tag_bottom_left_in_boundary() {
        // Inside `boundary_rect`, at its bottom left, sized bold for the
        // name then `[type]`; `None` for an out-of-range index.
        let mut p = mk(2, &[]);
        p.kind = ViewKind::Container;
        p.boundaries = vec![Boundary {
            of: BoundaryOf::Element(p.boxes[0].element),
            instances: None,
            technology: None,
            name: "Shop".into(),
            members: vec![BoxId(0), BoxId(1)],
            style: crate::model::ResolvedBoundaryStyle::DEFAULT,
        }];
        let centres = vec![pt(0, 0), pt(30, 20)];
        let outer = p.boundary_rect(0, &centres).expect("boundary");
        let tag = boundary_tag(&p, 0, &centres).expect("tag");
        assert_eq!(tag.min.x, outer.min.x);
        assert_eq!(tag.max.y, outer.max.y);
        assert!(outer.contains(tag.min) && outer.contains(tag.max));
        let text_px = |s: &str| (text_advance(s, Weight::Bold) * 12 + EM - 1) / EM;
        assert!(grid_to_px(tag.width()) >= text_px("Shop [Software System]") + 12);
        assert!(grid_to_px(tag.height()) >= 12);
        assert!(boundary_tag(&p, 1, &centres).is_none());
        // A component view names the scope `[Container]`: a shorter tag.
        p.kind = ViewKind::Component;
        let short = boundary_tag(&p, 0, &centres).expect("tag");
        assert!(short.width() < tag.width());
        assert!(grid_to_px(short.width()) >= text_px("Shop [Container]") + 12);
        // Boxes keep clear of their own boundary's tag (40px margin).
        for (i, b) in p.boxes.iter().enumerate() {
            assert!(!b.rect_at(centres[i]).overlaps(&tag));
        }
    }

    #[test]
    fn core_15_over_line_reported() {
        // `Fit::OverLines` names the covered lines; `over_lines()` lists
        // it. The spot is still clear of boxes, labels and the boundary
        // tag, and covers the fewest lines (ties: nearer the middle, then
        // nearer the start).
        let xs = [4, 7, 10, 13, 16];
        let mut edges = vec![(0, 1, "Reads", "")];
        edges.extend(xs.iter().map(|_| (0, 1, "Reads", "")));
        let p = mk(2, &edges);
        let mut lines = vec![line(0, &[(0, 0), (20, 0)])];
        // Vertical lines crossing edge 0 at x 4, 7, 10, 13, 16.
        for (i, &x) in xs.iter().enumerate() {
            lines.push(line(id(i + 1), &[(x, -20), (x, 20)]));
        }
        let r = routes(far(2), lines);
        let labels = place_labels(&p, &r);
        let l = label(&labels, 0);
        // x 10 covers only the line at x 10; x 8, 9, 11, 12 cover two.
        assert_eq!(l.centre, pt(10, 0));
        assert_eq!(l.fit, Fit::OverLines(vec![EdgeId(3)]));
        assert!(labels.over_lines().contains(&EdgeId(0)));
        let own = r.line(EdgeId(0)).expect("routed");
        let count = |c: Point| {
            r.lines
                .iter()
                .flatten()
                .filter(|o| o.edge != EdgeId(0) && covers(&moved(l, c), o))
                .count()
        };
        for c in grid_points(&own.points) {
            if (3..=15).contains(&c.x) {
                assert!(count(c) >= 1);
            }
        }
        for (i, b) in p.boxes.iter().enumerate() {
            assert!(!l.bounds().overlaps(&b.rect_at(r.centres[i])));
        }
        let over = labels.over_lines();
        let mut sorted = over.clone();
        sorted.sort();
        assert_eq!(over, sorted);
        for e in &over {
            assert!(matches!(label(&labels, e.0).fit, Fit::OverLines(ref v) if !v.is_empty()));
        }
    }

    #[test]
    fn core_15_no_room_reported() {
        // Very short line between adjacent boxes: reported as L24 and
        // drawn at the middle of its line, as CORE-15 says, even when
        // that covers a box: CORE-15 wins; such a spot is never chosen
        // as a clear spot (plan Review Focus).
        let p = mk(2, &[(0, 1, "Reads", "")]);
        let centres = vec![pt(0, 0), pt(10, 0)];
        let r = routes(centres.clone(), vec![line(0, &[(4, 0), (6, 0)])]);
        let labels = place_labels(&p, &r);
        let l = label(&labels, 0);
        assert_eq!(l.fit, Fit::NoRoom);
        assert_eq!(l.centre, pt(5, 0));
        assert_eq!(labels.no_room(), [EdgeId(0)]);
        assert!(labels.over_lines().is_empty());
        // Long enough for the gaps, but every spot is over a box: still
        // no room, never placed as clear.
        let mut p = mk(3, &[(0, 1, "Reads", "")]);
        p.boxes[2].half_width = 20;
        p.boxes[2].half_height = 2;
        let r = routes(
            vec![pt(-40, 0), pt(40, 0), pt(10, 2)],
            vec![line(0, &[(-5, 0), (25, 0)])],
        );
        let labels = place_labels(&p, &r);
        assert_eq!(label(&labels, 0).fit, Fit::NoRoom);
        assert_eq!(label(&labels, 0).centre, pt(10, 0));
        assert_eq!(labels.no_room(), [EdgeId(0)]);
    }

    #[test]
    fn core_15_labels_compared_in_px_not_grid() {
        // Two `Reads` labels (43x19px) 7px apart in px, whose grid
        // bounds (rounded outward) overlap: the second still sits clear,
        // not reported as L24.
        let p = mk(2, &[(0, 1, "Reads", ""), (0, 1, "Reads", "")]);
        let r = routes(
            far(2),
            vec![line(0, &[(0, 0), (8, 0)]), line(1, &[(5, 1), (13, 1)])],
        );
        let labels = place_labels(&p, &r);
        assert_eq!(label(&labels, 0).centre, pt(3, 0));
        assert_eq!(label(&labels, 0).fit, Fit::Clear);
        assert_eq!(label(&labels, 1).centre, pt(8, 1));
        assert_eq!(label(&labels, 1).fit, Fit::Clear);
        assert!(labels.no_room().is_empty());
    }

    #[test]
    fn core_15_line_touching_px_corner_not_covered() {
        // Edge 0's only spot (3,2) has px rect x 9..52 / y 11..30; the
        // diagonal line px x+y=20 only touches its corner (9,11), though
        // it crosses the grid bounds: clear, not reported as L23.
        let p = mk(2, &[(0, 1, "Reads", ""), (0, 1, "Reads", "")]);
        let r = routes(
            far(2),
            vec![line(0, &[(0, 2), (8, 2)]), line(1, &[(-1, 3), (1, 1)])],
        );
        let labels = place_labels(&p, &r);
        assert_eq!(label(&labels, 0).centre, pt(3, 2));
        assert_eq!(label(&labels, 0).fit, Fit::Clear);
        assert!(labels.over_lines().is_empty());
    }

    #[test]
    fn core_25_same_input_same_labels() {
        let mut p = mk(
            3,
            &[
                (0, 1, "Reads the current stock levels", "SQL"),
                (1, 2, "Sends nightly reconciliation", "SFTP"),
                (2, 0, "Publishes events", ""),
                (0, 2, "Reads the current stock levels", "SQL"),
            ],
        );
        p.kind = ViewKind::Container;
        p.boundaries = vec![Boundary {
            of: BoundaryOf::Element(p.boxes[0].element),
            instances: None,
            technology: None,
            name: "Shop".into(),
            members: vec![BoxId(0)],
            style: crate::model::ResolvedBoundaryStyle::DEFAULT,
        }];
        let r = routes(
            vec![pt(0, 0), pt(40, 0), pt(40, 30)],
            vec![
                line(0, &[(4, 0), (36, 0)]),
                line(1, &[(40, 3), (40, 27)]),
                line(2, &[(36, 30), (10, 30), (0, 20), (0, 3)]),
                joined(3, &[(4, 2), (20, 2), (22, 0)], 0),
            ],
        );
        let first = place_labels(&p, &r);
        for _ in 0..5 {
            assert_eq!(place_labels(&p, &r), first);
        }
        assert_eq!(first.labels.len(), p.edges.len());
        for (i, l) in first.labels.iter().enumerate() {
            if let Some(l) = l {
                assert_eq!(l.edge, EdgeId(id(i)));
            }
        }
    }

    #[test]
    fn core_30_text_advance_from_width_table() {
        // Per-character sum, bold wider than regular, fallback width for
        // a character missing from the table.
        assert_eq!(text_advance("", Weight::Regular), 0);
        // Inter 4.1 Regular `a`: 1151 of 2048 units per em.
        assert_eq!(text_advance("a", Weight::Regular), 562);
        assert_eq!(
            text_advance("Reads", Weight::Regular),
            ["R", "e", "a", "d", "s"]
                .iter()
                .map(|c| text_advance(c, Weight::Regular))
                .sum::<i32>()
        );
        assert!(text_advance("Reads", Weight::Bold) > text_advance("Reads", Weight::Regular));
        assert!(text_advance("é", Weight::Regular) > 0);
        // A character not in the table: the fallback, the same for both
        // weights.
        let cjk = text_advance("\u{4e2d}", Weight::Regular);
        assert!(cjk > 0);
        assert_eq!(cjk, text_advance("\u{4e2d}", Weight::Bold));
        assert_eq!(text_advance("\u{4e2d}\u{6587}", Weight::Regular), 2 * cjk);
    }

    #[test]
    fn core_4_technology_off_hides_tech_line() {
        let mut p = mk(2, &[(0, 1, "Reads stock", "SQL")]);
        assert_eq!(
            label_lines(&p.edges[0], &p.options),
            ["Reads stock", "[SQL]"]
        );
        p.options.technology = false;
        assert_eq!(label_lines(&p.edges[0], &p.options), ["Reads stock"]);
        let r = routes(far(2), vec![line(0, &[(0, 0), (30, 0)])]);
        let labels = place_labels(&p, &r);
        let l = label(&labels, 0);
        assert!(l.lines.iter().all(|s| !s.contains('[')));
        assert_eq!(l.height_px, LABEL_LINE_PX + 2 * LABEL_PAD_Y_PX);
    }

    #[test]
    fn core_13_unroutable_edge_has_no_label() {
        // Edge 0 is unroutable: no label; it is drawn straight between
        // its boxes (x 0), which edge 1's label keeps clear of.
        let p = mk(2, &[(0, 1, "Reads", ""), (0, 1, "Writes", "")]);
        let centres = vec![pt(0, -20), pt(0, 20)];
        let fallback = Segment::new(centres[0], centres[1]);
        let r = routes(centres, vec![None, line(1, &[(-15, 0), (15, 0)])]);
        let labels = place_labels(&p, &r);
        assert!(labels.labels[0].is_none());
        assert!(labels.label(EdgeId(0)).is_none());
        assert!(labels.label(EdgeId(9)).is_none());
        assert!(!labels.no_room().contains(&EdgeId(0)));
        let l = label(&labels, 1);
        assert_eq!(l.fit, Fit::Clear);
        assert!(!fallback.passes_through(&l.bounds()));
    }
}
