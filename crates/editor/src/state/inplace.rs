//! Where the in-place text editor types each field (EDIT-5): the place,
//! font, size, weight, colour and alignment the diagram draws that text
//! with, taken from the scene, so the field sits IN the shape. A field
//! whose text is empty gets the slot it would take.
//!
//! Depends on: `state` (`TextField`), `c4rizer-core` (scene, label).

use c4rizer_core::geom::{Point, Rect};
use c4rizer_core::label::{LABEL_FONT_PX, LABEL_LINE_PX, Weight};
use c4rizer_core::model::Colour;
use c4rizer_core::scene::{Align, ItemKey, Scene, TextBlock};

use super::TextField;

/// The box padding inside its text area, px (scene `PAD_PX`).
const PAD_PX: i32 = 10;
/// The gap above a box description, px (scene `DESC_GAP_PX`).
const DESC_GAP_PX: i32 = 6;
/// The narrowest a label or title field gets, scene px, so an empty or
/// one-word text still has room to type.
const MIN_FIELD_PX: i32 = 120;

/// One field's place in the diagram, in scene px.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Slot {
    /// Its index in `TextEdit::fields`.
    pub field: usize,
    /// The scene item the replaced text belongs to.
    pub source: ItemKey,
    /// The scene text drawn there, not painted while the field is shown.
    pub replaces: Option<TextBlock>,
    /// The area the field types in: x the text width it wraps to (or is
    /// centred in), y its lines as the scene draws them, one line at
    /// least.
    pub band: Rect,
    /// The item's own rectangle (box, label, title), for the field-name
    /// label's placement.
    pub item: Rect,
    /// Font size, px.
    pub size_px: i32,
    /// Baseline-to-baseline distance, px.
    pub line_px: i32,
    /// Bold face.
    pub bold: bool,
    /// Centred on the band (else start-aligned at its left).
    pub centred: bool,
    /// Text colour.
    pub colour: Colour,
    /// Wraps over several lines (descriptions).
    pub multiline: bool,
    /// Fixed text drawn before and after it (the `[Container: ` and `]`
    /// of a technology).
    pub prefix: String,
    /// See `prefix`.
    pub suffix: String,
    /// A fill under the text, for texts drawn on a label plate.
    pub fill: Option<Colour>,
}

/// The slots of `fields` of the edited `item` in `scene`, one per field
/// that has a place (a field with none is left out), in field order.
#[must_use]
pub(crate) fn slots(scene: &Scene, item: ItemKey, fields: &[TextField]) -> Vec<Slot> {
    let mut out = match item {
        ItemKey::Box(id) => scene.boxes.get(id.0 as usize).map(|b| box_slots(b, fields)).unwrap_or_default(),
        ItemKey::Line(edge) | ItemKey::Label(edge) => label_slots(scene, edge, fields),
        ItemKey::Title | ItemKey::Subtitle => title_slots(scene, fields),
        _ => Vec::new(),
    };
    out.sort_by_key(|s| s.field);
    out
}

/// The field under scene point `at`, if any.
#[must_use]
pub(crate) fn field_at(slots: &[Slot], at: Point) -> Option<usize> {
    slots.iter().find(|s| s.band.contains(at)).map(|s| s.field)
}

fn index_of(fields: &[TextField], label: &str) -> Option<usize> {
    fields.iter().position(|f| f.label == label)
}

/// A slot for `block`'s font and colour at `band`.
fn from_block(field: usize, source: ItemKey, block: &TextBlock, band: Rect, item: Rect) -> Slot {
    Slot {
        field,
        source,
        replaces: (!block.lines.is_empty()).then(|| block.clone()),
        band,
        item,
        size_px: block.size_px,
        line_px: block.line_px,
        bold: matches!(block.weight, Weight::Bold),
        centred: matches!(block.align, Align::Middle),
        colour: block.colour,
        multiline: false,
        prefix: String::new(),
        suffix: String::new(),
        fill: None,
    }
}

/// `lines` rows of `line_px` from `top`, between `left` and `right`.
fn band(left: i32, right: i32, top: i32, lines: usize, line_px: i32) -> Rect {
    let n = i32::try_from(lines.max(1)).unwrap_or(1);
    Rect::from_corners(Point::new(left, top), Point::new(right, top + n * line_px))
}

/// Name, technology (on the type line) and description of a box.
// DECISION: the text width is the box rectangle less the padding,
// symmetric about the name's centre; the scene's per-shape text area is
// private to the core and the name is centred in it, so this matches it
// on rectangles and stays inside on the other shapes.
fn box_slots(b: &c4rizer_core::scene::SceneBox, fields: &[TextField]) -> Vec<Slot> {
    let source = ItemKey::Box(b.id);
    let cx = b.name.at.x;
    let half = ((cx - b.rect.min.x).min(b.rect.max.x - cx) - PAD_PX).max(MIN_FIELD_PX / 4);
    let (left, right) = (cx - half, cx + half);
    let mut out = Vec::new();
    let name_top = b.name.bounds.min.y;
    if let Some(i) = index_of(fields, "name") {
        let band = band(left, right, name_top, b.name.lines.len(), b.name.line_px);
        out.push(from_block(i, source, &b.name, band, b.rect));
    }
    let name_bottom = name_top + i32::try_from(b.name.lines.len().max(1)).unwrap_or(1) * b.name.line_px;
    let type_top = if b.type_line.lines.is_empty() { name_bottom } else { b.type_line.bounds.min.y };
    let mut below_type = type_top;
    if let Some(i) = index_of(fields, "technology") {
        let mut slot = from_block(i, source, &b.type_line, band(left, right, type_top, 1, b.type_line.line_px), b.rect);
        slot.centred = true;
        let line = b.type_line.lines.join(" ");
        // `[Container: tech]` or `[Container]`: the type stays as it is
        // drawn, the technology is typed after it.
        if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let kind = inner.split_once(": ").map_or(inner, |(k, _)| k);
            slot.prefix = format!("[{kind}: ");
            "]".clone_into(&mut slot.suffix);
        }
        out.push(slot);
        below_type = type_top + b.type_line.line_px;
    } else if !b.type_line.lines.is_empty() {
        below_type = b.type_line.bounds.max.y;
    }
    if let Some(i) = index_of(fields, "description") {
        let mut slot = if let Some(d) = &b.description {
            from_block(i, source, d, band(left, right, d.bounds.min.y, d.lines.len(), d.line_px), b.rect)
        } else {
            // The place a description takes: below the type line, in the
            // type line's font.
            let top = below_type + DESC_GAP_PX;
            let mut s = from_block(i, source, &b.type_line, band(left, right, top, 1, b.type_line.line_px), b.rect);
            s.replaces = None;
            s.colour = b.name.colour;
            s.bold = false;
            s
        };
        slot.centred = true;
        slot.multiline = true;
        out.push(slot);
    }
    out
}

/// Description and technology of a line's label (or a step's
/// description); a line with no label gets them at its middle.
// DECISION: a step's label is edited as its description alone; its
// leading step number is not shown while typing (the number is not part
// of the written description).
fn label_slots(scene: &Scene, edge: c4rizer_core::view::EdgeId, fields: &[TextField]) -> Vec<Slot> {
    let desc = index_of(fields, "description");
    let tech = index_of(fields, "technology");
    let has_tech = tech.is_some_and(|t| fields[t].original.is_some());
    let has_desc = desc.is_some_and(|d| fields[d].original.is_some());
    let source = ItemKey::Label(edge);
    let mut out = Vec::new();
    if let Some(l) = scene.labels.iter().find(|l| l.edge == edge) {
        let t = &l.text;
        let width = t.bounds.width().max(MIN_FIELD_PX);
        let (left, right) = if matches!(t.align, Align::Middle) {
            (t.at.x - width / 2, t.at.x + width / 2)
        } else {
            (t.at.x, t.at.x + width)
        };
        let top = t.bounds.min.y;
        let n = t.lines.len();
        let tech_line = has_tech && n > 0 && t.lines[n - 1].starts_with('[') && t.lines[n - 1].ends_with(']');
        let desc_lines = if tech_line { n - 1 } else { n };
        let desc_lines = if has_desc { desc_lines } else { 0 };
        // With no description, its empty slot goes above the technology
        // line, so the technology stays where it is drawn.
        let desc_top = if desc_lines == 0 && tech_line { top - t.line_px } else { top };
        let tech_top = top + i32::try_from(desc_lines).unwrap_or(0) * t.line_px;
        if let Some(i) = desc {
            let mut s = from_block(i, source, t, band(left, right, desc_top, desc_lines, t.line_px), l.rect);
            s.multiline = true;
            s.fill = l.fill;
            out.push(s);
        }
        if let Some(i) = tech {
            let mut s = from_block(i, source, t, band(left, right, tech_top.max(desc_top + t.line_px), 1, t.line_px), l.rect);
            s.replaces = None;
            "[".clone_into(&mut s.prefix);
            "]".clone_into(&mut s.suffix);
            s.fill = l.fill;
            out.push(s);
        }
        // The whole label text is replaced by the fields: suppress it once.
        if let Some(first) = out.first_mut() {
            first.replaces = Some(t.clone());
        }
        return out;
    }
    let Some(line) = scene.lines.iter().find(|l| l.edge == edge) else { return out };
    let mid = middle_of(&line.points);
    let half = MIN_FIELD_PX / 2;
    let item = Rect::from_corners(
        Point::new(mid.x - half, mid.y - LABEL_LINE_PX),
        Point::new(mid.x + half, mid.y + LABEL_LINE_PX),
    );
    let base = |i: usize, top: i32| Slot {
        field: i,
        source,
        replaces: None,
        band: band(mid.x - half, mid.x + half, top, 1, LABEL_LINE_PX),
        item,
        size_px: LABEL_FONT_PX,
        line_px: LABEL_LINE_PX,
        bold: false,
        centred: true,
        colour: line.stroke.colour,
        multiline: false,
        prefix: String::new(),
        suffix: String::new(),
        fill: Some(scene.background),
    };
    if let Some(i) = desc {
        let mut s = base(i, mid.y - LABEL_LINE_PX);
        s.multiline = true;
        out.push(s);
    }
    if let Some(i) = tech {
        let mut s = base(i, mid.y);
        "[".clone_into(&mut s.prefix);
        "]".clone_into(&mut s.suffix);
        out.push(s);
    }
    out
}

/// The middle of a polyline: the middle of its middle segment.
fn middle_of(points: &[Point]) -> Point {
    match points {
        [] => Point::new(0, 0),
        [p] => *p,
        _ => {
            let i = (points.len() - 1) / 2;
            let (a, b) = (points[i], points[i + 1]);
            Point::new(i32::midpoint(a.x, b.x), i32::midpoint(a.y, b.y))
        }
    }
}

/// The view's title and subtitle, below the drawing.
// DECISION: a missing subtitle gets the scene's subtitle font (12px
// regular on a 16px pitch) just below the title.
fn title_slots(scene: &Scene, fields: &[TextField]) -> Vec<Slot> {
    let Some(title) = &scene.title else { return Vec::new() };
    let width = |b: &TextBlock| b.bounds.width().max(MIN_FIELD_PX * 3);
    let left = title.bounds.min.x;
    let mut out = Vec::new();
    let item = Rect::from_corners(
        title.bounds.min,
        scene.subtitle.as_ref().map_or(title.bounds.max, |s| s.bounds.max),
    );
    if let Some(i) = index_of(fields, "title") {
        let band = band(left, left + width(title), title.bounds.min.y, 1, title.line_px);
        out.push(from_block(i, ItemKey::Title, title, band, item));
    }
    if let Some(i) = index_of(fields, "subtitle") {
        let slot = match &scene.subtitle {
            Some(s) => from_block(i, ItemKey::Subtitle, s, band(left, left + width(s), s.bounds.min.y, 1, s.line_px), item),
            None => Slot {
                field: i,
                source: ItemKey::Subtitle,
                replaces: None,
                band: band(left, left + width(title), title.bounds.max.y, 1, 16),
                item,
                size_px: 12,
                line_px: 16,
                bold: false,
                centred: false,
                colour: title.colour,
                multiline: false,
                prefix: String::new(),
                suffix: String::new(),
                fill: None,
            },
        };
        out.push(slot);
    }
    out
}
