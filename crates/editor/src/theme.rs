//! The editor's look (redesign 2026-11-19, `design-system/tokens.json`):
//! the chrome colour tokens, the fonts (Geist for the chrome, Geist Mono
//! for data, Inter for the diagram) and egui's `Visuals` and `Spacing`
//! set once at start-up from them.
//!
//! Depends on: `c4rizer-core` (the Inter faces), `eframe`.

// Shape: plain constants named after the tokens, so a widget reads
// `theme::INK_2` where the design says `ink-2`. The chrome is dark in
// both diagram modes; only `mark*` and the diagram colours follow the
// `Dark` toggle, and those are read per mode (`Marks::of`).

use std::sync::Arc;

use c4rizer_core::model::Mode;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Margin, RichText, Shadow,
    Stroke, TextStyle, Vec2,
};

const fn hex(rgb: u32) -> Color32 {
    // The masks keep each channel in 0..=255, so the casts never truncate.
    #[allow(clippy::cast_possible_truncation)]
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

// ------------------------------------------------------------ chrome colours

/// Behind the paper and inside fields.
pub const BG_CANVAS: Color32 = hex(0x0d0f10);
/// Bars and panels.
pub const BG_PANEL: Color32 = hex(0x17191b);
/// Buttons and hovered rows.
pub const BG_RAISED: Color32 = hex(0x1f2224);
/// A hovered button.
pub const BG_ACTIVE: Color32 = hex(0x2a2e31);
/// Anything that floats.
pub const BG_OVERLAY: Color32 = hex(0x1c1f21);
/// Hairline between regions.
pub const LINE: Color32 = hex(0x2a2e31);
/// Around buttons and overlays.
pub const LINE_2: Color32 = hex(0x3a4044);
/// Around anything you type in or tick.
pub const LINE_STRONG: Color32 = hex(0x6b7277);
/// Text.
pub const INK: Color32 = hex(0xeceef0);
/// Secondary text.
pub const INK_2: Color32 = hex(0xa7aeb3);
/// Labels, placeholders, counts.
pub const INK_3: Color32 = hex(0x868e94);
/// Controls that cannot be used.
pub const INK_DISABLED: Color32 = hex(0x5a6166);
/// The editor's own colour: current item, toggle on, focus, primary.
pub const ACCENT: Color32 = hex(0xa99cf5);
/// A hovered primary button.
pub const ACCENT_HOVER: Color32 = hex(0xbeb4f8);
/// The current item's fill.
pub const ACCENT_SOFT: Color32 = hex(0x2a2742);
/// The border of a toggle that is on.
pub const ACCENT_LINE: Color32 = hex(0x5d54a0);
/// Text on an `ACCENT` fill.
pub const ON_ACCENT: Color32 = hex(0x15131f);
/// Parse and I/O errors.
pub const DANGER: Color32 = hex(0xf08a7c);
/// Lint.
pub const WARN: Color32 = hex(0xe2b866);
/// Lint chip ground.
pub const WARN_SOFT: Color32 = hex(0x352d1c);
/// Lint chip border.
pub const WARN_LINE: Color32 = hex(0x6e5a2c);
/// The live dot.
pub const OK: Color32 = hex(0x6fc7b5);
/// An ok chip ground.
pub const OK_SOFT: Color32 = hex(0x1c302d);
/// An ok chip border.
pub const OK_LINE: Color32 = hex(0x33605a);

// ------------------------------------------------------------ sizes

/// Controls.
pub const SIZE_CONTROL: f32 = 24.0;
/// Rows, the status bar.
pub const SIZE_ROW: f32 = 28.0;
/// The top bar.
pub const SIZE_BAR: f32 = 40.0;
/// Icons.
pub const SIZE_ICON: f32 = 16.0;
/// Controls' corners.
pub const RADIUS_1: u8 = 2;
/// Overlays' corners.
pub const RADIUS_2: u8 = 4;
/// A panel's side padding.
pub const PAD: f32 = 12.0;

// ------------------------------------------------------------ marks

/// The editor marks of one diagram mode: they must hold on the paper
/// (white or dark) while the chrome stays dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marks {
    /// Selection, the text being edited, the current target.
    pub mark: Color32,
    /// Handle fill.
    pub handle: Color32,
    /// Hints: snap points, ticks.
    pub hint: Color32,
}

impl Marks {
    /// The marks of `mode`.
    #[must_use]
    pub const fn of(mode: Mode) -> Marks {
        match mode {
            Mode::Dark => Marks {
                mark: hex(0xa99cf5),
                handle: hex(0x1e2124),
                hint: hex(0x737c83),
            },
            _ => Marks {
                mark: hex(0x6a55e3),
                handle: hex(0xffffff),
                hint: hex(0x7b848b),
            },
        }
    }
}

// ------------------------------------------------------------ fonts

/// Geist SemiBold: `title`, `ui-strong`.
pub const SEMIBOLD: &str = "Geist SemiBold";
/// Geist Medium: `display`, a primary button, recent file names.
pub const MEDIUM: &str = "Geist Medium";
/// Geist Mono Medium: `label`.
pub const MONO_MEDIUM: &str = "Geist Mono Medium";
/// Inter Regular, the diagram's face (also `canvas::REGULAR_FAMILY`).
pub const INTER: &str = "Inter";
/// Inter Bold (`canvas::BOLD_FAMILY`).
pub const INTER_BOLD: &str = "Inter Bold";

const GEIST: &[u8] = include_bytes!("../../../assets/fonts/Geist-Variable.ttf");
const GEIST_MONO: &[u8] = include_bytes!("../../../assets/fonts/GeistMono-Variable.ttf");

/// One weight of a variable face.
fn weight(bytes: &'static [u8], wght: f32) -> Arc<FontData> {
    let tweak = FontTweak {
        coords: egui::epaint::text::VariationCoords::new([(b"wght", wght)]),
        ..FontTweak::default()
    };
    Arc::new(FontData::from_static(bytes).tweak(tweak))
}

/// The font definitions: Geist as `Proportional`, Geist Mono as
/// `Monospace`, the named weights above, Inter for the diagram; egui's
/// own faces stay behind each as fallbacks for symbols.
// DECISION: the variable Geist files are instanced by egui (`FontTweak::
// coords`, skrifa) rather than shipped as one static file per weight; the
// pack's WOFF2 were only unpacked to TTF, the outlines are unchanged.
#[must_use]
pub fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let fallbacks: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mono_fallbacks: Vec<String> = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let faces = [
        ("Geist-400", weight(GEIST, 400.0)),
        ("Geist-500", weight(GEIST, 500.0)),
        ("Geist-600", weight(GEIST, 600.0)),
        ("GeistMono-400", weight(GEIST_MONO, 400.0)),
        ("GeistMono-500", weight(GEIST_MONO, 500.0)),
        ("Inter-Regular", Arc::new(FontData::from_static(c4rizer_core::scene::FONT_REGULAR))),
        ("Inter-Bold", Arc::new(FontData::from_static(c4rizer_core::scene::FONT_BOLD))),
    ];
    for (name, data) in faces {
        fonts.font_data.insert(name.to_owned(), data);
    }
    let with = |first: &str, rest: &[String]| -> Vec<String> {
        std::iter::once(first.to_owned()).chain(rest.iter().cloned()).collect()
    };
    fonts.families.insert(FontFamily::Proportional, with("Geist-400", &fallbacks));
    fonts.families.insert(FontFamily::Monospace, with("GeistMono-400", &mono_fallbacks));
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), with("Geist-600", &fallbacks));
    fonts.families.insert(FontFamily::Name(MEDIUM.into()), with("Geist-500", &fallbacks));
    fonts.families.insert(FontFamily::Name(MONO_MEDIUM.into()), with("GeistMono-500", &mono_fallbacks));
    fonts.families.insert(FontFamily::Name(INTER.into()), vec!["Inter-Regular".to_owned()]);
    fonts.families.insert(FontFamily::Name(INTER_BOLD.into()), vec!["Inter-Bold".to_owned()]);
    fonts
}

/// `ui`: every control and row.
#[must_use]
pub fn ui_font() -> FontId {
    FontId::proportional(12.0)
}

/// `ui-strong`.
#[must_use]
pub fn strong_font() -> FontId {
    FontId::new(12.0, FontFamily::Name(SEMIBOLD.into()))
}

/// `title`: names the selection.
#[must_use]
pub fn title_font() -> FontId {
    FontId::new(15.0, FontFamily::Name(SEMIBOLD.into()))
}

/// `code`: keys, ids, paths, zoom, the status bar.
#[must_use]
pub fn code_font() -> FontId {
    FontId::monospace(11.0)
}

/// `label`: names of things in the interface, upper case.
#[must_use]
pub fn label_font() -> FontId {
    FontId::new(10.0, FontFamily::Name(MONO_MEDIUM.into()))
}

/// `label` text: mono, upper case, tracked, `ink-3`.
#[must_use]
pub fn label(text: &str) -> RichText {
    RichText::new(text.to_uppercase())
        .font(label_font())
        .extra_letter_spacing(0.6)
        .color(INK_3)
}

/// `code` text in `colour`.
#[must_use]
pub fn code(text: impl Into<String>, colour: Color32) -> RichText {
    RichText::new(text).font(code_font()).color(colour)
}

// ------------------------------------------------------------ visuals

/// The egui style from the tokens (`guides/02-egui.md`).
#[must_use]
pub fn style(base: &egui::Style) -> egui::Style {
    let mut style = base.clone();
    style.text_styles = [
        (TextStyle::Body, ui_font()),
        (TextStyle::Button, ui_font()),
        (TextStyle::Small, label_font()),
        (TextStyle::Monospace, code_font()),
        (TextStyle::Heading, title_font()),
    ]
    .into();
    let r1 = CornerRadius::same(RADIUS_1);
    let r2 = CornerRadius::same(RADIUS_2);
    let shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(115),
    };
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = BG_PANEL;
    v.extreme_bg_color = BG_CANVAS;
    v.text_edit_bg_color = Some(BG_CANVAS);
    v.faint_bg_color = BG_RAISED;
    v.code_bg_color = BG_RAISED;
    v.window_fill = BG_OVERLAY;
    v.window_stroke = Stroke::new(1.0, LINE_2);
    v.window_corner_radius = r2;
    v.menu_corner_radius = r2;
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = ACCENT_SOFT;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.weak_text_color = Some(INK_3);
    v.striped = false;
    v.indent_has_left_vline = false;
    v.collapsing_header_frame = false;
    v.window_highlight_topmost = false;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = BG_PANEL;
    w.noninteractive.weak_bg_fill = BG_PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, INK_2);
    w.inactive.bg_fill = BG_RAISED;
    w.inactive.weak_bg_fill = BG_RAISED;
    w.inactive.bg_stroke = Stroke::new(1.0, LINE_2);
    w.inactive.fg_stroke = Stroke::new(1.0, INK);
    w.hovered.bg_fill = BG_ACTIVE;
    w.hovered.weak_bg_fill = BG_ACTIVE;
    w.hovered.bg_stroke = Stroke::new(1.0, LINE_STRONG);
    w.hovered.fg_stroke = Stroke::new(1.0, INK);
    w.active.bg_fill = BG_CANVAS;
    w.active.weak_bg_fill = BG_CANVAS;
    w.active.bg_stroke = Stroke::new(1.0, LINE_STRONG);
    w.active.fg_stroke = Stroke::new(1.0, INK);
    w.open.bg_fill = BG_CANVAS;
    w.open.weak_bg_fill = BG_CANVAS;
    w.open.bg_stroke = Stroke::new(1.0, ACCENT);
    w.open.fg_stroke = Stroke::new(1.0, INK);
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = r1;
        wv.expansion = 0.0;
    }
    let s = &mut style.spacing;
    s.interact_size = Vec2::new(40.0, SIZE_CONTROL);
    s.item_spacing = Vec2::splat(4.0);
    s.button_padding = Vec2::new(8.0, 4.0);
    s.menu_margin = Margin::same(4);
    s.window_margin = Margin::same(12);
    s.icon_width = 14.0;
    s.icon_width_inner = 8.0;
    s.icon_spacing = 8.0;
    s.combo_height = 7.0 * SIZE_ROW;
    style.interaction.tooltip_delay = 0.4;
    style
}

/// Installs the fonts and the style on `ctx` (once, at start-up).
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
    // The chrome is dark whatever the system theme says.
    ctx.set_theme(egui::ThemePreference::Dark);
    let base = (*ctx.global_style()).clone();
    let style = style(&base);
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}
