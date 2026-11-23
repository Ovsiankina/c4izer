//! The chrome's widgets (redesign 2026-11-19, `design-system/components`):
//! Button, Segmented, Section, Field rows, Select, Checkbox, Swatch,
//! Chip, Kbd, Menu item and the tooltip that names the action then its
//! key. Every value is a `theme` token.
//!
//! Depends on: `theme`, `icons`, `eframe`.

// Shape: free functions over `egui::Ui` that allocate, paint and return
// the `Response`, like egui's own widgets. None keeps state of its own
// except a Section's open flag and a Swatch's popup, both in egui memory.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::icons::{self, Icon};
use crate::theme::{
    ACCENT, ACCENT_HOVER, ACCENT_LINE, ACCENT_SOFT, BG_ACTIVE, BG_CANVAS, BG_OVERLAY, BG_PANEL, BG_RAISED, INK, INK_2,
    INK_3, INK_DISABLED, LINE, LINE_2, LINE_STRONG, ON_ACCENT, OK, OK_LINE, OK_SOFT, PAD, RADIUS_1, SIZE_CONTROL,
    SIZE_ICON, SIZE_ROW, WARN, WARN_LINE, WARN_SOFT,
};

fn r1() -> CornerRadius {
    CornerRadius::same(RADIUS_1)
}

// ------------------------------------------------------------ focus

/// True while the keyboard drives: from a Tab until the next pointer
/// press (behaviour change 2: the focus ring shows only then).
pub fn keyboard_driving(ctx: &egui::Context) -> bool {
    let id = egui::Id::unique("c4rizer-keyboard-driving");
    let (tab, pointer) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::Tab),
            i.pointer.any_pressed() || i.pointer.delta() != Vec2::ZERO && i.pointer.any_down(),
        )
    });
    let mut on = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if tab {
        on = true;
    }
    if pointer {
        on = false;
    }
    ctx.data_mut(|d| d.insert_temp(id, on));
    on
}

/// The focus ring: 2 px `accent`, 1 px out, only while the keyboard
/// drives.
pub fn focus_ring(ui: &Ui, response: &Response) {
    if response.has_focus() && keyboard_driving(ui.ctx()) {
        ui.painter().rect_stroke(
            response.rect.expand(1.0),
            CornerRadius::same(RADIUS_1 + 1),
            Stroke::new(2.0, ACCENT),
            StrokeKind::Outside,
        );
    }
}

// ------------------------------------------------------------ text helpers

fn text_width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), INK).size().x)
}

/// `text` cut with an ellipsis to fit `room` px in `font`.
#[must_use]
pub fn elide(ui: &Ui, text: &str, font: &FontId, room: f32) -> String {
    if text_width(ui, text, font) <= room {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        n -= 1;
        let cut: String = chars[..n].iter().collect::<String>() + "…";
        if text_width(ui, &cut, font) <= room {
            return cut;
        }
    }
    "…".to_owned()
}

// ------------------------------------------------------------ button

/// How a button looks at rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Look {
    /// `bg-raised` with a `line-2` border.
    #[default]
    Normal,
    /// No ground until hovered, `ink-2` (top bar, status bar).
    Ghost,
    /// The one `accent` button of a screen.
    Primary,
}

/// A button: icon, text or both (`components/Button`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Button<'a> {
    icon: Option<Icon>,
    text: Option<&'a str>,
    look: Look,
    on: bool,
    enabled: bool,
    block: bool,
}

impl<'a> Button<'a> {
    /// A button with `text`.
    #[must_use]
    pub fn new(text: &'a str) -> Self {
        Button {
            text: Some(text),
            enabled: true,
            ..Button::default()
        }
    }

    /// An icon-only button, `size-control` square.
    #[must_use]
    pub fn icon(icon: Icon) -> Self {
        Button {
            icon: Some(icon),
            enabled: true,
            ..Button::default()
        }
    }

    /// Adds an icon before the text.
    #[must_use]
    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Sets the look.
    #[must_use]
    pub fn look(mut self, look: Look) -> Self {
        self.look = look;
        self
    }

    /// A toggle that is on: `accent-soft`, `accent-line`, `accent`.
    #[must_use]
    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    /// Disabled: `ink-disabled`, no ground.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Takes the whole width.
    #[must_use]
    pub fn block(mut self) -> Self {
        self.block = true;
        self
    }

    fn font(&self) -> FontId {
        if self.look == Look::Primary {
            FontId::new(12.0, egui::FontFamily::Name(crate::theme::MEDIUM.into()))
        } else {
            crate::theme::ui_font()
        }
    }

    /// Adds the button to `ui`.
    pub fn show(self, ui: &mut Ui) -> Response {
        let font = self.font();
        let text_w = self.text.map_or(0.0, |t| text_width(ui, t, &font));
        let natural = match (self.icon, self.text) {
            (_, None) => SIZE_CONTROL,
            (None, Some(_)) => text_w + 16.0,
            (Some(_), Some(_)) => SIZE_ICON + 4.0 + text_w + 16.0,
        };
        let width = if self.block { ui.available_width() } else { natural };
        let sense = if self.enabled { Sense::click() } else { Sense::hover() };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, SIZE_CONTROL), sense);
        if ui.is_rect_visible(rect) {
            let hovered = self.enabled && response.hovered();
            let pressed = self.enabled && response.is_pointer_button_down_on();
            let (fill, border, ink, icon_ink) = match (self.enabled, self.look, self.on) {
                (false, Look::Normal | Look::Primary, _) => (None, LINE, INK_DISABLED, INK_DISABLED),
                (false, Look::Ghost, _) => (None, Color32::TRANSPARENT, INK_DISABLED, INK_DISABLED),
                (true, _, true) => (Some(ACCENT_SOFT), ACCENT_LINE, ACCENT, ACCENT),
                (true, Look::Primary, false) => {
                    let c = if hovered { ACCENT_HOVER } else { ACCENT };
                    (Some(c), c, ON_ACCENT, ON_ACCENT)
                }
                (true, Look::Normal, false) => {
                    let fill = if pressed {
                        BG_CANVAS
                    } else if hovered {
                        BG_ACTIVE
                    } else {
                        BG_RAISED
                    };
                    (Some(fill), if hovered { LINE_STRONG } else { LINE_2 }, INK, INK)
                }
                (true, Look::Ghost, false) => {
                    let fill = if pressed {
                        Some(BG_CANVAS)
                    } else if hovered {
                        Some(BG_RAISED)
                    } else {
                        None
                    };
                    let ink = if hovered { INK } else { INK_2 };
                    (fill, Color32::TRANSPARENT, ink, ink)
                }
            };
            let p = ui.painter();
            if let Some(fill) = fill {
                p.rect_filled(rect, r1(), fill);
            }
            if border != Color32::TRANSPARENT {
                p.rect_stroke(rect, r1(), Stroke::new(1.0, border), StrokeKind::Inside);
            }
            let content = match self.icon {
                Some(_) if self.text.is_some() => SIZE_ICON + 4.0 + text_w,
                Some(_) => SIZE_ICON,
                None => text_w,
            };
            let mut x = rect.center().x - content / 2.0;
            if let Some(icon) = self.icon {
                let r = Rect::from_min_size(egui::pos2(x.round(), (rect.center().y - 8.0).round()), Vec2::splat(SIZE_ICON));
                icons::paint(p, icon, r, icon_ink);
                x += SIZE_ICON + 4.0;
            }
            if let Some(text) = self.text {
                p.text(egui::pos2(x, rect.center().y), Align2::LEFT_CENTER, text, font, ink);
            }
            if self.enabled && hovered {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
        }
        focus_ring(ui, &response);
        response
    }
}

/// A tooltip naming the action, then its key as a key cap (behaviour
/// change 14).
pub fn tip(response: Response, action: &str, key: Option<&str>) -> Response {
    let action = action.to_owned();
    let key = key.map(str::to_owned);
    response.on_hover_ui(move |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(egui::RichText::new(&action).color(INK));
            if let Some(key) = &key {
                for part in key.split('+') {
                    kbd(ui, part);
                }
            }
        });
    })
}

// ------------------------------------------------------------ segmented

/// One item of a segmented control.
#[derive(Debug, Clone, Copy)]
pub struct Segment<'a> {
    /// Its icon.
    pub icon: Option<Icon>,
    /// Its text (`None`: icon only).
    pub text: Option<&'a str>,
    /// True when it is the one on.
    pub on: bool,
    /// The tooltip's action.
    pub tip: &'a str,
    /// The tooltip's key.
    pub key: Option<&'a str>,
    /// False when it cannot run.
    pub enabled: bool,
}

impl<'a> Segment<'a> {
    /// A text item.
    #[must_use]
    pub fn text(text: &'a str, tip: &'a str) -> Self {
        Segment {
            icon: None,
            text: Some(text),
            on: false,
            tip,
            key: None,
            enabled: true,
        }
    }

    /// An icon item.
    #[must_use]
    pub fn icon(icon: Icon, tip: &'a str) -> Self {
        Segment {
            icon: Some(icon),
            text: None,
            on: false,
            tip,
            key: None,
            enabled: true,
        }
    }

    /// Adds an icon before the text.
    #[must_use]
    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Marks it on.
    #[must_use]
    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    /// Its key, for the tooltip.
    #[must_use]
    pub fn key(mut self, key: &'a str) -> Self {
        self.key = Some(key);
        self
    }

    /// Enables or disables it.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// A segmented control (`components/Segmented`); `fill` spreads the items
/// over the whole width. Returns the index clicked.
pub fn segmented(ui: &mut Ui, items: &[Segment<'_>], fill: bool) -> Option<usize> {
    let font = crate::theme::ui_font();
    let widths: Vec<f32> = items
        .iter()
        .map(|s| match (s.icon, s.text) {
            (_, None) => SIZE_CONTROL - 2.0,
            (None, Some(t)) => text_width(ui, t, &font) + 16.0,
            (Some(_), Some(t)) => SIZE_ICON + 4.0 + text_width(ui, t, &font) + 16.0,
        })
        .collect();
    let natural: f32 = widths.iter().sum::<f32>() + 2.0;
    let total = if fill { ui.available_width() } else { natural };
    let (outer, _) = ui.allocate_exact_size(Vec2::new(total, SIZE_CONTROL), Sense::hover());
    #[allow(clippy::cast_precision_loss)]
    let each = (total - 2.0) / items.len().max(1) as f32;
    let p = ui.painter().clone();
    p.rect_filled(outer, r1(), BG_CANVAS);
    let mut clicked = None;
    let mut x = outer.min.x + 1.0;
    for (i, seg) in items.iter().enumerate() {
        let w = if fill { each } else { widths[i] };
        let rect = Rect::from_min_size(egui::pos2(x, outer.min.y + 1.0), Vec2::new(w, SIZE_CONTROL - 2.0));
        let id = ui.scope_id().with(("c4rizer-seg", i, seg.tip));
        let sense = if seg.enabled { Sense::click() } else { Sense::hover() };
        let response = ui.interact(rect, id, sense);
        let hovered = seg.enabled && response.hovered();
        let ink = if !seg.enabled {
            INK_DISABLED
        } else if seg.on {
            ACCENT
        } else if hovered {
            INK
        } else {
            INK_2
        };
        if seg.on {
            p.rect_filled(rect, CornerRadius::ZERO, ACCENT_SOFT);
        } else if hovered {
            p.rect_filled(rect, CornerRadius::ZERO, BG_RAISED);
        }
        let content = match (seg.icon, seg.text) {
            (Some(_), Some(t)) => SIZE_ICON + 4.0 + text_width(ui, t, &font),
            (Some(_), None) => SIZE_ICON,
            (None, Some(t)) => text_width(ui, t, &font),
            (None, None) => 0.0,
        };
        let mut cx = rect.center().x - content / 2.0;
        if let Some(icon) = seg.icon {
            let r = Rect::from_min_size(egui::pos2(cx.round(), (rect.center().y - 8.0).round()), Vec2::splat(SIZE_ICON));
            icons::paint(&p, icon, r, ink);
            cx += SIZE_ICON + 4.0;
        }
        if let Some(t) = seg.text {
            p.text(egui::pos2(cx, rect.center().y), Align2::LEFT_CENTER, t, font.clone(), ink);
        }
        if i + 1 < items.len() {
            let sx = (rect.max.x).round() - 0.5;
            p.line_segment([egui::pos2(sx, outer.min.y), egui::pos2(sx, outer.max.y)], Stroke::new(1.0, LINE_2));
        }
        let response = tip(response, seg.tip, seg.key);
        if response.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    p.rect_stroke(outer, r1(), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
    clicked
}

// ------------------------------------------------------------ section

/// A panel section (`components/Section`): a 28 px header with a fold
/// chevron, a label and an optional count; the body padded 0 12 12; a
/// hairline under it. `count` is `(text, warn)`.
#[expect(clippy::too_many_arguments, reason = "a section's id, title, count and fold, then its body")]
pub fn section<R>(
    ui: &mut Ui,
    id: &str,
    title: &str,
    count: Option<(String, bool)>,
    default_open: bool,
    body: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let key = egui::Id::unique(("c4rizer-section", id));
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(key)).unwrap_or(default_open);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SIZE_ROW), Sense::click());
    if response.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_temp(key, open));
    }
    let p = ui.painter();
    let chevron = if open { Icon::ChevronDown } else { Icon::ChevronRight };
    let icon_rect = Rect::from_min_size(egui::pos2(rect.min.x + PAD, rect.center().y - 6.0), Vec2::splat(12.0));
    icons::paint(p, chevron, icon_rect, INK_3);
    let label = crate::theme::label(title);
    let galley = egui::WidgetText::from(label).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Small);
    p.galley(egui::pos2(icon_rect.max.x + 8.0, rect.center().y - galley.size().y / 2.0), galley, INK_3);
    if let Some((text, warn)) = count {
        p.text(
            egui::pos2(rect.max.x - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            text,
            FontId::monospace(10.0),
            if warn { WARN } else { INK_3 },
        );
    }
    let inner = if open {
        let inner = egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 0,
                bottom: 12,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(4.0, 8.0);
                ui.set_width(ui.available_width());
                body(ui)
            })
            .inner;
        Some(inner)
    } else {
        None
    };
    hairline(ui);
    inner
}

/// A full-width 1 px `line` rule.
pub fn hairline(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, LINE);
}

/// The header of the selection section: a label line, used where the
/// section does not fold (`NOTHING SELECTED`, `SOFTWARE SYSTEM`).
pub fn head_label(ui: &mut Ui, text: &str, dot: Option<Color32>) {
    let galley = egui::WidgetText::from(crate::theme::label(text)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        ui.available_width() - 16.0,
        egui::TextStyle::Small,
    );
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 12.0), Sense::hover());
    let mut x = rect.min.x;
    if let Some(c) = dot {
        let r = Rect::from_min_size(egui::pos2(x, rect.center().y - 4.0), Vec2::splat(8.0));
        ui.painter().rect_filled(r, CornerRadius::same(1), c);
        x += 16.0;
    }
    ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, INK_3);
}

// ------------------------------------------------------------ form

/// A form row: a 64 px label column, then `control` filling the rest.
pub fn form_row<R>(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(64.0, ui.spacing().interact_size.y), Sense::hover());
        let galley = egui::WidgetText::from(crate::theme::label(label)).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            64.0,
            egui::TextStyle::Small,
        );
        ui.painter()
            .galley(egui::pos2(rect.min.x, rect.center().y - galley.size().y / 2.0), galley, INK_3);
        control(ui)
    })
    .inner
}

/// A property row of the details: label column, then a value that wraps.
pub fn prop_row(ui: &mut Ui, label: &str, value: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(64.0, 18.0), Sense::hover());
        let galley = egui::WidgetText::from(crate::theme::label(label)).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            64.0,
            egui::TextStyle::Small,
        );
        ui.painter()
            .galley(egui::pos2(rect.min.x, rect.center().y - galley.size().y / 2.0), galley, INK_3);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            value(ui);
        });
    });
}

/// Styles `ui` for fields and selects: `bg-canvas` inside, `line-strong`
/// around, `accent` when open (`components/Field`, `Select`).
pub fn field_style(ui: &mut Ui) {
    let w = &mut ui.visuals_mut().widgets;
    for wv in [&mut w.inactive, &mut w.hovered, &mut w.active] {
        wv.bg_fill = BG_CANVAS;
        wv.weak_bg_fill = BG_CANVAS;
        wv.bg_stroke = Stroke::new(1.0, LINE_STRONG);
    }
    w.open.bg_stroke = Stroke::new(1.0, ACCENT);
    ui.visuals_mut().selection.stroke = Stroke::new(1.0, ACCENT);
}

/// A select (`components/Select`): a combo box filling the width with a
/// chevron, showing whole rows, at most seven, then scrolling (behaviour
/// change 9). `choices` are the rows; returns the index picked.
pub fn select(ui: &mut Ui, id: &str, current: &str, choices: &[String], selected: Option<usize>) -> Option<usize> {
    let mut picked = None;
    ui.scope(|ui| {
        field_style(ui);
        let width = ui.available_width();
        egui::ComboBox::from_id_salt(("c4rizer-select", id))
            .width(width)
            .height(7.0 * SIZE_ROW)
            .selected_text(current)
            .icon(|ui, rect, visuals, _open| {
                let r = Rect::from_center_size(rect.center(), Vec2::splat(SIZE_ICON));
                let _ = visuals;
                icons::paint(ui.painter(), Icon::ChevronDown, r, INK_2);
            })
            .show_ui(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, text) in choices.iter().enumerate() {
                    if menu_row(ui, None, text, selected == Some(i), true).clicked() {
                        picked = Some(i);
                    }
                }
            });
    });
    picked
}

// ------------------------------------------------------------ checkbox

/// A checkbox (`components/Checkbox`): 14 px box, `accent` with a tick
/// when on. Returns the response; `changed` when clicked.
pub fn checkbox(ui: &mut Ui, on: &mut bool, text: &str) -> Response {
    let font = crate::theme::ui_font();
    let w = 14.0 + 8.0 + text_width(ui, text, &font);
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(w, SIZE_CONTROL), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let p = ui.painter();
    let b = Rect::from_min_size(egui::pos2(rect.min.x, rect.center().y - 7.0), Vec2::splat(14.0));
    if *on {
        p.rect_filled(b, r1(), ACCENT);
        icons::paint_stroke(p, Icon::Check, b.shrink(1.0), ON_ACCENT, 2.0);
    } else {
        p.rect_filled(b, r1(), BG_CANVAS);
        p.rect_stroke(b, r1(), Stroke::new(1.0, LINE_STRONG), StrokeKind::Inside);
    }
    p.text(egui::pos2(b.max.x + 8.0, rect.center().y), Align2::LEFT_CENTER, text, font, INK);
    focus_ring(ui, &response);
    response
}

// ------------------------------------------------------------ swatch

/// A colour swatch field (`components/Swatch`): chip and hex code; a click
/// opens the picker. `None` shows the mixed chip ("differs"). Returns the
/// colour picked this frame.
pub fn swatch(ui: &mut Ui, id: &str, value: Option<[u8; 3]>) -> Option<[u8; 3]> {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, SIZE_CONTROL), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, r1(), BG_CANVAS);
    let border = if response.hovered() { INK_3 } else { LINE_STRONG };
    p.rect_stroke(rect, r1(), Stroke::new(1.0, border), StrokeKind::Inside);
    let chip = Rect::from_min_size(egui::pos2(rect.min.x + 4.0, rect.center().y - 8.0), Vec2::splat(16.0));
    if let Some([r, g, b]) = value {
        p.rect_filled(chip, CornerRadius::same(1), Color32::from_rgb(r, g, b));
        p.text(
            egui::pos2(chip.max.x + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            format!("#{r:02x}{g:02x}{b:02x}"),
            crate::theme::code_font(),
            INK,
        );
    } else {
        p.rect_filled(chip, CornerRadius::same(1), BG_CANVAS);
        let hatch = p.with_clip_rect(chip);
        let mut x = chip.min.x - 16.0;
        while x < chip.max.x {
            hatch.line_segment(
                [egui::pos2(x, chip.max.y), egui::pos2(x + 16.0, chip.min.y)],
                Stroke::new(2.0, BG_ACTIVE),
            );
            x += 4.0;
        }
        p.text(
            egui::pos2(chip.max.x + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            "differs",
            crate::theme::code_font(),
            INK_3,
        );
    }
    p.rect_stroke(chip, CornerRadius::same(1), Stroke::new(1.0, LINE_STRONG), StrokeKind::Inside);
    let mut picked = None;
    let key = egui::Id::unique(("c4rizer-swatch-colour", id));
    egui::Popup::from_toggle_button_response(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let start = value.unwrap_or([128, 128, 128]);
            let mut c = ui
                .ctx()
                .data(|d| d.get_temp::<Color32>(key))
                .unwrap_or_else(|| Color32::from_rgb(start[0], start[1], start[2]));
            if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
                picked = Some([c.r(), c.g(), c.b()]);
            }
            ui.ctx().data_mut(|d| d.insert_temp(key, c));
        });
    if !egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response)) {
        ui.ctx().data_mut(|d| d.remove::<Color32>(key));
    }
    focus_ring(ui, &response);
    picked
}

// ------------------------------------------------------------ chip, kbd

/// How a chip reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipKind {
    /// A tag or plain value.
    Plain,
    /// A key or id, mono.
    Key,
    /// The current item.
    Accent,
    /// A lint code.
    Warn,
    /// Fine.
    Ok,
}

/// A chip (`components/Chip`): 18 px high.
pub fn chip(ui: &mut Ui, text: &str, kind: ChipKind) -> Response {
    let font = match kind {
        ChipKind::Key => FontId::monospace(10.0),
        _ => FontId::proportional(11.0),
    };
    let (fill, border, ink) = match kind {
        ChipKind::Plain => (BG_RAISED, LINE_2, INK_2),
        ChipKind::Key => (BG_RAISED, LINE_2, INK),
        ChipKind::Accent => (ACCENT_SOFT, ACCENT_LINE, ACCENT),
        ChipKind::Warn => (WARN_SOFT, WARN_LINE, WARN),
        ChipKind::Ok => (OK_SOFT, OK_LINE, OK),
    };
    let w = text_width(ui, text, &font) + 12.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(w, 18.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, r1(), fill);
    p.rect_stroke(rect, r1(), Stroke::new(1.0, border), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, text, font, ink);
    response
}

/// What a tag row did this frame (P5-204).
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-12 / issue #2)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagRowAction {
    /// The remove mark of this tag was clicked.
    Remove(String),
    /// The "+" chip was clicked.
    Add,
}

/// The editable tag row of boxes and lines (P5-204): one chip per tag of
/// `tags`, those in `removable` with a remove mark on hover, then a "+"
/// chip when `add`. Chips wrap. Returns what was clicked.
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-12 / issue #2)
// DECISION: a removable chip keeps the room of its mark when not hovered,
// so the row does not reflow under the pointer.
pub fn tag_row(ui: &mut Ui, tags: &[String], removable: &[String], add: bool) -> Option<TagRowAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(4.0);
        let font = FontId::proportional(11.0);
        for tag in tags {
            if !removable.contains(tag) {
                chip(ui, tag, ChipKind::Plain);
                continue;
            }
            let w = text_width(ui, tag, &font) + 12.0 + SIZE_ICON;
            let (rect, response) = ui.allocate_exact_size(Vec2::new(w, 18.0), Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, r1(), BG_RAISED);
            p.rect_stroke(rect, r1(), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
            p.text(egui::pos2(rect.min.x + 6.0, rect.center().y), Align2::LEFT_CENTER, tag, font.clone(), INK_2);
            let mark = Rect::from_center_size(egui::pos2(rect.max.x - 2.0 - SIZE_ICON / 2.0, rect.center().y), Vec2::splat(10.0));
            let over_mark = response.hover_pos().is_some_and(|at| mark.expand(2.0).contains(at));
            if response.hovered() {
                icons::paint(p, Icon::Close, mark, if over_mark { INK } else { INK_3 });
            }
            let response = response.on_hover_text(format!("Remove the tag {tag} (one undo step)"));
            if response.clicked() && over_mark {
                action = Some(TagRowAction::Remove(tag.clone()));
            }
        }
        if add {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(18.0, 18.0), Sense::click());
            let p = ui.painter();
            let border = if response.hovered() { INK_3 } else { LINE_2 };
            p.rect_filled(rect, r1(), BG_RAISED);
            p.rect_stroke(rect, r1(), Stroke::new(1.0, border), StrokeKind::Inside);
            icons::paint(p, Icon::Plus, Rect::from_center_size(rect.center(), Vec2::splat(12.0)), INK_2);
            let response = tip(response, "Add a tag", None);
            focus_ring(ui, &response);
            if response.clicked() {
                action = Some(TagRowAction::Add);
            }
        }
    });
    action
}

/// What the add-tag popup's body did this frame (P5-205).
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-12 / issue #2)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagPick {
    /// The field's new text, when it changed.
    pub typed: Option<String>,
    /// The row clicked.
    pub picked: Option<String>,
}

/// The add-tag popup's body (P5-205): a text field holding `query`, with
/// focus, then one row per entry of `rows`. Enter and Escape are the
/// state's keys.
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-12 / issue #2)
pub fn tag_picker(ui: &mut Ui, query: &str, rows: &[String]) -> TagPick {
    let mut pick = TagPick::default();
    let mut buffer = query.to_owned();
    let field = ui.scope(|ui| {
        field_style(ui);
        ui.add(
            egui::TextEdit::singleline(&mut buffer)
                .hint_text(egui::RichText::new("new or existing tag").color(INK_3))
                .desired_width(f32::INFINITY)
                .align(Align2::LEFT_CENTER)
                .min_size(Vec2::new(0.0, SIZE_CONTROL)),
        )
    });
    if !field.inner.has_focus() {
        field.inner.request_focus();
    }
    if buffer != query {
        pick.typed = Some(buffer);
    }
    ui.add_space(4.0);
    if rows.is_empty() {
        ui.label(egui::RichText::new("no existing tag matches; Enter adds it").color(INK_3));
    }
    for row in rows {
        if menu_row(ui, Some(Icon::Tag), row, false, true).clicked() {
            pick.picked = Some(row.clone());
        }
    }
    pick
}

/// A key cap (`components/Kbd`).
pub fn kbd(ui: &mut Ui, text: &str) -> Response {
    let font = FontId::monospace(10.0);
    let w = (text_width(ui, text, &font) + 8.0).max(16.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(w, 16.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, r1(), BG_PANEL);
    p.rect_stroke(rect, r1(), Stroke::new(1.0, LINE_STRONG), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, text, font, INK_2);
    response
}

// ------------------------------------------------------------ menu

/// A menu row (`components/Menu`): 26 px, `accent-soft` on hover, an
/// optional icon; disabled rows show `ink-disabled` and take no click
/// (behaviour change 5).
pub fn menu_row(ui: &mut Ui, icon: Option<Icon>, text: &str, on: bool, enabled: bool) -> Response {
    let width = ui.available_width().max(192.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 26.0), sense);
    let hovered = enabled && response.hovered();
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, r1(), ACCENT_SOFT);
    }
    let ink = if !enabled {
        INK_DISABLED
    } else if on {
        ACCENT
    } else {
        INK
    };
    let mut x = rect.min.x + 8.0;
    if let Some(icon) = icon {
        let icon_ink = if !enabled {
            INK_DISABLED
        } else if hovered || on {
            ACCENT
        } else {
            INK_2
        };
        icons::paint(p, icon, Rect::from_min_size(egui::pos2(x, rect.center().y - 8.0), Vec2::splat(SIZE_ICON)), icon_ink);
        x += SIZE_ICON + 8.0;
    }
    let font = crate::theme::ui_font();
    let room = rect.max.x - x - 8.0;
    p.text(egui::pos2(x, rect.center().y), Align2::LEFT_CENTER, elide(ui, text, &font, room), font, ink);
    response
}

/// The frame of anything that floats: `bg-overlay`, `line-2`, `radius-2`,
/// the overlay shadow.
pub fn overlay_frame(ui: &Ui) -> egui::Frame {
    egui::Frame::popup(ui.style())
        .fill(BG_OVERLAY)
        .stroke(Stroke::new(1.0, LINE_2))
        .corner_radius(CornerRadius::same(crate::theme::RADIUS_2))
        .inner_margin(egui::Margin::same(4))
}
