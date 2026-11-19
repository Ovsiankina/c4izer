//! The in-place text editor on the canvas (EDIT-5, `components/Marks`):
//! every field of the edited item typed IN the shape, at the place and in
//! the font, size, weight, colour and alignment the diagram draws that
//! text; the canvas's own drawing of those texts left out. The focused
//! field wears a 1.5 px `mark` border with its name above it as a label;
//! the others a faint dashed hint.
//!
//! Depends on: `state::inplace` (the slots), `canvas` (fonts, text
//! primitives), `theme` (marks, label font).

use std::sync::Arc;

use c4rizer_core::geom::Point;
use c4rizer_core::model::{Colour, Mode};
use eframe::egui::{self, Align, Align2, Color32, FontFamily, FontId, Galley, Pos2, Stroke, Vec2};
use eframe::egui::text::{LayoutJob, TextFormat};

use crate::camera::{Camera, ScreenPoint};
use crate::canvas::{self, PaintItem, Primitive, Source};
use crate::state::inplace::Slot;
use crate::state::{Event, Input, Key, Modifiers, TextEdit};
use crate::theme;

/// The fields' room around their text, screen px, for the marks.
const OUTLINE_PAD: f32 = 3.0;

/// Drops from `items` the scene texts the fields of `slots` replace, so
/// nothing is drawn twice (EDIT-5).
pub(crate) fn suppress(items: &mut Vec<PaintItem>, slots: &[Slot], camera: &Camera) {
    for slot in slots {
        let Some(block) = &slot.replaces else { continue };
        let lines = canvas::text_items(block, camera);
        // DECISION: matched on source, anchor and text, not colour: the
        // canvas recolours a label's technology line.
        items.retain(|item| {
            let Primitive::Text { at, text, .. } = &item.primitive else { return true };
            item.source != Source::Scene(slot.source)
                || !lines.iter().any(|l| matches!(l, Primitive::Text { at: a, text: t, .. } if a == at && t == text))
        });
    }
}

/// Drops from `inputs` the canvas presses that land on a field of
/// `slots`: egui reports the canvas as under the pointer there too, and a
/// canvas click ends the edit (EDIT-5). The release stays: with no press
/// it does nothing.
pub(super) fn keep_presses_on_fields(inputs: &mut Vec<Input>, slots: &[Slot], camera: &Camera) {
    if slots.is_empty() {
        return;
    }
    let on_field = |at: ScreenPoint| {
        slots.iter().any(|s| {
            let (a, b) = (camera.to_screen(s.band.min), camera.to_screen(s.band.max));
            let pad = OUTLINE_PAD + 1.0;
            at.x >= a.x - pad && at.x <= b.x + pad && at.y >= a.y - pad && at.y <= b.y + pad
        })
    };
    inputs.retain(|i| !matches!(i, Input::PointerDown { at, .. } if on_field(*at)));
}

/// The key presses that move the editor's focus from field `current` to
/// field `target` of `count`: Tab moves to the next field (EDIT-5), so the
/// shell reaches a clicked field the way the details panel does.
#[must_use]
pub(crate) fn focus_keys(current: usize, target: usize, count: usize) -> Vec<Event> {
    if count == 0 || target >= count {
        return Vec::new();
    }
    let tabs = (target + count - current % count) % count;
    (0..tabs)
        .map(|_| {
            Event::Input(Input::KeyDown {
                key: Key::Tab,
                mods: Modifiers::default(),
                text_focus: true,
            })
        })
        .collect()
}

fn colour32(c: Colour) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

/// A field's face: font, ink and line pitch, screen px.
struct Face {
    font: FontId,
    ink: Color32,
    line: f32,
}

impl Face {
    /// Lays out `text` at `line` px a line, wrapped to `wrap`
    /// (multi-line), centred or start-aligned.
    fn layout(&self, ui: &egui::Ui, text: &str, wrap: Option<f32>, centred: bool) -> Arc<Galley> {
        let format = TextFormat {
            font_id: self.font.clone(),
            color: self.ink,
            line_height: Some(self.line),
            ..TextFormat::default()
        };
        let mut job = LayoutJob::single_section(text.to_owned(), format);
        if let Some(w) = wrap {
            job.wrap.max_width = w;
            job.break_on_newline = true;
        }
        job.halign = if centred { Align::Center } else { Align::Min };
        job.keep_trailing_whitespace = true;
        ui.fonts_mut(|f| f.layout_job(job))
    }
}

/// The galley's first baseline below its top, px.
fn baseline_of(galley: &Galley) -> f32 {
    galley
        .rows
        .first()
        .map_or(0.0, |r| r.pos.y + r.row.glyphs.first().map_or(0.0, |g| g.pos.y))
}

/// One drawn field: where it went and what came of it.
struct Shown {
    field: usize,
    outline: egui::Rect,
    pointed: bool,
    focused_now: bool,
    buffer: String,
    id: egui::Id,
}

/// Draws every field of `edit` at its slot over the canvas whose top-left
/// is `origin`, with the marks, and returns the events: the typed text of
/// the focused field (`Event::Text`) and, for a click on another field,
/// the Tab presses that move the focus there.
#[expect(clippy::too_many_lines, reason = "one pass: lay out, type, mark")]
#[expect(clippy::too_many_arguments, reason = "the canvas's frame, camera and mode, the edit and its slots")]
pub(super) fn fields(ui: &mut egui::Ui, origin: Pos2, camera: &Camera, edit: &TextEdit, slots: &[Slot], mode: Mode) -> Vec<Event> {
    let marks = theme::Marks::of(mode);
    let zoom = camera.zoom;
    let to_screen = |p: Point| {
        let s = camera.to_screen(p);
        origin + Vec2::new(s.x, s.y)
    };
    let mut shown: Vec<Shown> = Vec::new();
    for slot in slots {
        let Some(field) = edit.fields.get(slot.field) else { continue };
        #[expect(clippy::cast_precision_loss, reason = "font sizes are small")]
        let (size, line) = (slot.size_px as f32 * zoom, slot.line_px as f32 * zoom);
        if size < 1.0 {
            continue;
        }
        let font = FontId::new(
            size,
            FontFamily::Name(if slot.bold { canvas::BOLD_FAMILY } else { canvas::REGULAR_FAMILY }.into()),
        );
        let ink = colour32(slot.colour);
        let face = Face { font: font.clone(), ink, line };
        let band = egui::Rect::from_min_max(to_screen(slot.band.min), to_screen(slot.band.max));
        // The scene's first baseline in a line band (`Font::baseline_px`).
        let baseline_px = i32::midpoint(slot.line_px, slot.size_px * 7 / 10);
        let baseline = to_screen(Point::new(slot.band.min.x, slot.band.min.y + baseline_px)).y;
        let probe = face.layout(ui, "Hg", None, false);
        let top = baseline - baseline_of(&probe);
        let wrap = slot.multiline.then_some(band.width());
        let hint = field.label;
        let empty = field.buffer.is_empty();
        let shown_text = if empty { hint } else { field.buffer.as_str() };
        let measured = face.layout(ui, shown_text, wrap, slot.centred);
        // With a prefix (`[Container: ` ... `]`) the three parts are laid
        // out as one centred line; else the field takes the band.
        let mut fixed: Vec<(Pos2, Arc<Galley>)> = Vec::new();
        let (edit_rect, align, text_area) = if slot.prefix.is_empty() && slot.suffix.is_empty() {
            let height = measured.size().y.max(line);
            let align = if slot.centred { Align2::CENTER_TOP } else { Align2::LEFT_TOP };
            let rect = egui::Rect::from_min_size(Pos2::new(band.left(), top), Vec2::new(band.width(), height));
            (rect, align, rect.union(band))
        } else {
            let pre = face.layout(ui, &slot.prefix, None, false);
            let suf = face.layout(ui, &slot.suffix, None, false);
            let w = measured.size().x + 2.0;
            let total = pre.size().x + w + suf.size().x;
            let left = if slot.centred { band.center().x - total / 2.0 } else { band.left() };
            fixed.push((Pos2::new(left + pre.size().x + w, top), suf));
            fixed.push((Pos2::new(left, top), pre.clone()));
            let rect = egui::Rect::from_min_size(Pos2::new(left + pre.size().x, top), Vec2::new(w, line));
            let whole = egui::Rect::from_min_size(Pos2::new(left, top), Vec2::new(total, line));
            (rect, Align2::LEFT_TOP, whole)
        };
        if let Some(fill) = slot.fill {
            let plate = text_area.expand2(Vec2::new(OUTLINE_PAD, 1.0));
            ui.painter().rect_filled(plate, egui::CornerRadius::same(3), colour32(fill));
        }
        for (pos, galley) in fixed {
            ui.painter().galley(pos, galley, ink);
        }
        if empty {
            // The faint placeholder naming the field, where its text goes.
            let faint = Face { ink: ink.gamma_multiply(0.4), font: font.clone(), line }.layout(ui, hint, wrap, slot.centred);
            let pos = align.align_size_within_rect(faint.size(), edit_rect).min;
            let pos = Pos2::new(pos.x - faint.rect.left(), pos.y);
            ui.painter().galley(pos, faint, ink);
        }
        let mut buffer = field.buffer.clone();
        let id = egui::Id::unique(("c4rizer-text-edit", slot.field));
        let (multiline, centred) = (slot.multiline, slot.centred);
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
            face.layout(ui, text.as_str(), multiline.then_some(wrap_width), centred && align == Align2::CENTER_TOP)
        };
        let response = ui
            .scope(|ui| {
                ui.visuals_mut().selection.bg_fill = marks.mark.gamma_multiply(0.3);
                ui.visuals_mut().selection.stroke = Stroke::new(1.0, ink);
                ui.visuals_mut().text_cursor.stroke = Stroke::new(1.5, marks.mark);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.spacing_mut().interact_size = Vec2::ZERO;
                let base = if multiline {
                    // EDIT-5: Enter confirms; it never starts a new line.
                    egui::TextEdit::multiline(&mut buffer).return_key(None).desired_rows(1)
                } else {
                    egui::TextEdit::singleline(&mut buffer).clip_text(slot.prefix.is_empty())
                };
                ui.put(
                    edit_rect,
                    base.id(id)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .background_color(Color32::TRANSPARENT)
                        .text_color(ink)
                        .font(font.clone())
                        .align(align)
                        .desired_width(edit_rect.width())
                        .layouter(&mut layouter),
                )
            })
            .inner;
        let outline = text_area.union(response.rect).expand2(Vec2::new(OUTLINE_PAD, 0.0));
        shown.push(Shown {
            field: slot.field,
            outline,
            pointed: response.is_pointer_button_down_on() || response.clicked(),
            focused_now: ui.memory(|m| m.has_focus(response.id)),
            buffer,
            id: response.id,
        });
    }
    // A press on another field moves the focus there (by Tab presses, as
    // the details panel does); the focused field keeps egui's focus.
    let count = edit.fields.len();
    let target = shown.iter().find(|s| s.pointed && s.field != edit.current).map_or(edit.current, |s| s.field);
    let mut out = focus_keys(edit.current, target, count);
    if let Some(s) = shown.iter().find(|s| s.field == target) {
        if s.buffer != edit.fields[target].buffer {
            out.push(Event::Text(s.buffer.clone()));
        }
        if !s.focused_now {
            ui.memory_mut(|m| m.request_focus(s.id));
        }
    }
    paint_marks(ui, &shown, target, slots, camera, origin, &marks, edit);
    out
}

/// The 1.5 px `mark` border and label of the focused field, the dashed
/// hints of the others (`components/Marks`).
#[expect(clippy::too_many_arguments, reason = "the marks need the whole field set")]
fn paint_marks(
    ui: &egui::Ui,
    shown: &[Shown],
    focused: usize,
    slots: &[Slot],
    camera: &Camera,
    origin: Pos2,
    marks: &theme::Marks,
    edit: &TextEdit,
) {
    let p = ui.painter();
    let radius = egui::CornerRadius::same(2);
    for s in shown.iter().filter(|s| s.field != focused) {
        let r = s.outline;
        let path = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        p.extend(egui::Shape::dashed_line(&path, Stroke::new(1.0, marks.mark.gamma_multiply(0.55)), 3.0, 3.0));
    }
    let Some(s) = shown.iter().find(|s| s.field == focused) else { return };
    p.rect_stroke(s.outline, radius, Stroke::new(1.5, marks.mark), egui::StrokeKind::Outside);
    let Some(field) = edit.fields.get(focused) else { return };
    let label = p.layout_no_wrap(field.label.to_uppercase(), theme::label_font(), marks.handle);
    let chip = Vec2::new(label.size().x + 8.0, label.size().y + 2.0);
    let inside = egui::Rect::from_min_size(s.outline.left_top() - Vec2::new(0.0, chip.y + 2.0), chip);
    let item = slots.iter().find(|sl| sl.field == focused).map(|sl| {
        let a = camera.to_screen(sl.item.min);
        let b = camera.to_screen(sl.item.max);
        egui::Rect::from_min_max(origin + Vec2::new(a.x, a.y), origin + Vec2::new(b.x, b.y))
    });
    // The label sits just above the border, or above the item when it
    // would cover another field or leave the item there.
    let collides = shown.iter().any(|o| o.field != focused && o.outline.intersects(inside))
        || item.is_some_and(|i| !i.contains_rect(inside));
    let chip_rect = match item {
        Some(i) if collides => egui::Rect::from_min_size(Pos2::new(s.outline.left(), i.top() - chip.y - 3.0), chip),
        _ => inside,
    };
    p.rect_filled(chip_rect, radius, marks.mark);
    p.galley(chip_rect.min + Vec2::new(4.0, 1.0), label, marks.handle);
}

#[cfg(test)]
mod tests {
    use c4rizer_core::geom::{Point, Rect};
    use c4rizer_core::model::Colour;
    use c4rizer_core::scene::ItemKey;

    use super::{focus_keys, keep_presses_on_fields};
    use crate::camera::{Camera, ScreenPoint};
    use crate::state::inplace::Slot;
    use crate::state::{Button, Event, Input, Key, Modifiers};

    fn tabs(events: &[Event]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, Event::Input(Input::KeyDown { key: Key::Tab, .. })))
            .count()
    }

    /// EDIT-5: a click on field `target` moves the focus there by Tab
    /// presses, wrapping past the last field; none for the focused one.
    #[test]
    fn edit_5_focus_keys_wrap() {
        assert_eq!(tabs(&focus_keys(0, 2, 3)), 2);
        assert_eq!(tabs(&focus_keys(2, 1, 3)), 2);
        assert_eq!(tabs(&focus_keys(1, 1, 3)), 0);
        assert!(focus_keys(0, 3, 3).is_empty(), "no such field");
        assert!(focus_keys(0, 0, 0).is_empty());
    }

    /// EDIT-5: a press on a field is not a canvas press (which would end
    /// the edit); a press elsewhere and every release stay.
    #[test]
    fn edit_5_presses_on_fields_are_the_fields() {
        let rect = Rect::from_corners(Point::new(0, 0), Point::new(100, 20));
        let slot = Slot {
            field: 0,
            source: ItemKey::Title,
            replaces: None,
            band: rect,
            item: rect,
            size_px: 14,
            line_px: 18,
            bold: true,
            centred: true,
            colour: Colour { r: 0, g: 0, b: 0 },
            multiline: false,
            prefix: String::new(),
            suffix: String::new(),
            fill: None,
        };
        let camera = Camera::default();
        let on = camera.to_screen(Point::new(50, 10));
        let off = camera.to_screen(Point::new(50, 200));
        let down = |at: ScreenPoint| Input::PointerDown {
            at,
            button: Button::Left,
            mods: Modifiers::default(),
        };
        let up = Input::PointerUp { at: on, button: Button::Left };
        let mut inputs = vec![down(on), up, down(off)];
        keep_presses_on_fields(&mut inputs, &[slot], &camera);
        assert_eq!(inputs, vec![up, down(off)]);
    }
}
