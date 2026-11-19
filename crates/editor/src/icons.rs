//! The editor's icon set (redesign 2026-11-19, `assets/Icons`): 16 px
//! box, 1.5 px stroke, square caps, mitre joins, painted in the colour of
//! the text beside them.
//!
//! Depends on: `c4rizer-core` (`export::rasterize_asset`), `eframe`.

// Shape: each icon is its SVG body, copied from the design pack's
// screens. It is rasterized once per physical pixel size in white
// through the same resvg path as the diagram's icons, kept as a texture,
// and tinted at paint time, so one texture serves every colour.
// DECISION: textures rather than egui paths: resvg draws the arcs, caps
// and joins exactly as the pack's SVGs, at no new dependency.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, Pos2, Rect, TextureHandle, TextureOptions};

/// One icon of the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(missing_docs, reason = "each variant is the icon of its name")]
// DECISION: the whole set of the design pack stays, pinned by
// `every_icon_draws`, though a few icons (tag, layers, map, graph, tree,
// sort, page, callout, chevron up) have no place in the chrome yet.
#[cfg_attr(not(test), allow(dead_code, reason = "the design pack's whole set; some icons wait for a place"))]
pub enum Icon {
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    ChevronUp,
    ArrowUp,
    ArrowDown,
    Enter,
    External,
    PanelLeft,
    PanelRight,
    Open,
    Diagram,
    Explore,
    Search,
    Close,
    Check,
    Plus,
    Minus,
    Undo,
    Redo,
    Edit,
    Reset,
    Copy,
    Export,
    FitView,
    FitWidth,
    FitHeight,
    Fullscreen,
    Dark,
    AutoPlace,
    Magnet,
    Centre,
    VertexRemove,
    Bus,
    Callout,
    Routing,
    Page,
    AlignLeft,
    AlignCentre,
    AlignRight,
    AlignTop,
    AlignMiddle,
    AlignBottom,
    DistributeH,
    DistributeV,
    Warn,
    Info,
    Lock,
    Tag,
    Layers,
    Map,
    Graph,
    Tree,
    Sort,
}

impl Icon {
    /// The SVG body on the 16 px grid; `class="f"` marks a filled part.
    #[must_use]
    pub const fn body(self) -> &'static str {
        match self {
            Icon::ChevronLeft => r#"<path d="M10 3.5 5.5 8 10 12.5"/>"#,
            Icon::ChevronRight => r#"<path d="M6 3.5 10.5 8 6 12.5"/>"#,
            Icon::ChevronDown => r#"<path d="M3.5 6 8 10.5 12.5 6"/>"#,
            Icon::ChevronUp => r#"<path d="M3.5 10 8 5.5 12.5 10"/>"#,
            Icon::ArrowUp => r#"<path d="M8 13V3.5M4 7.5 8 3.5 12 7.5"/>"#,
            Icon::ArrowDown => r#"<path d="M8 3v9.5M4 8.5 8 12.5 12 8.5"/>"#,
            Icon::Enter => r#"<path d="M3.5 3v6.5H12M9 6.5l3 3-3 3"/>"#,
            Icon::External => r#"<path d="M6.5 3.5h-3v9h9v-3M9 3.5h3.5V7M12.5 3.5 7.5 8.5"/>"#,
            Icon::PanelLeft => r#"<path d="M2 3h12v10H2zM6 3v10"/>"#,
            Icon::PanelRight => r#"<path d="M2 3h12v10H2zM10 3v10"/>"#,
            Icon::Open => r#"<path d="M2 4h4l1.5 2H14v7H2z"/>"#,
            Icon::Diagram => r#"<path d="M2 2.5h5v4H2zM9 9.5h5v4H9zM4.5 6.5v5H9"/>"#,
            Icon::Explore => r#"<path d="M3 3.5h4M3 3.5v9h4M3 8h4M10 3.5h3.5M10 8h3.5M10 12.5h3.5"/>"#,
            Icon::Search => r#"<circle cx="7" cy="7" r="4"/><path d="M10 10l3.5 3.5"/>"#,
            Icon::Close => r#"<path d="M3.5 3.5l9 9M12.5 3.5l-9 9"/>"#,
            Icon::Check => r#"<path d="M3 8.5 6.5 12 13 4.5"/>"#,
            Icon::Plus => r#"<path d="M8 3v10M3 8h10"/>"#,
            Icon::Minus => r#"<path d="M3 8h10"/>"#,
            Icon::Undo => r#"<path d="M6 3.5 3 6.5 6 9.5M3.5 6.5H10a3.5 3.5 0 0 1 0 7H7"/>"#,
            Icon::Redo => r#"<path d="M10 3.5 13 6.5 10 9.5M12.5 6.5H6a3.5 3.5 0 0 0 0 7h3"/>"#,
            Icon::Edit => r#"<path d="M3 13l.5-2.5 7-7 2 2-7 7zM9 5l2 2"/>"#,
            Icon::Reset => r#"<path d="M3.5 8a4.5 4.5 0 1 0 1.5-3.4M3 2.5V5.5H6"/>"#,
            Icon::Copy => r#"<path d="M5.5 5.5h8v8h-8zM10.5 5.5v-3h-8v8h3"/>"#,
            Icon::Export => r#"<path d="M8 2.5v8M5 5.5l3-3 3 3M3 9.5v4h10v-4"/>"#,
            Icon::FitView => r#"<path d="M2.5 6V2.5H6M10 2.5h3.5V6M13.5 10v3.5H10M6 13.5H2.5V10"/>"#,
            Icon::FitWidth => r#"<path d="M2.5 3v10M13.5 3v10M5.5 8h5M7 6 5 8l2 2M9 6l2 2-2 2"/>"#,
            Icon::FitHeight => r#"<path d="M3 2.5h10M3 13.5h10M8 5.5v5M6 7l2-2 2 2M6 9l2 2 2-2"/>"#,
            Icon::Fullscreen => r#"<path d="M9.5 2.5h4v4M13.5 2.5 9 7M6.5 13.5h-4v-4M2.5 13.5 7 9"/>"#,
            Icon::Dark => r#"<circle cx="8" cy="8" r="5.5"/><path class="f" d="M8 2.5a5.5 5.5 0 0 1 0 11z"/>"#,
            Icon::AutoPlace => r#"<path d="M2.5 2.5H7V7H2.5zM9 2.5h4.5V7H9zM2.5 9H7v4.5H2.5zM9 9h4.5v4.5H9z"/>"#,
            Icon::Magnet => r#"<path d="M3.5 2.5V8a4.5 4.5 0 0 0 9 0V2.5h-3V8a1.5 1.5 0 0 1-3 0V2.5zM3.5 5h3M9.5 5h3"/>"#,
            Icon::Centre => r#"<path d="M8 1.5v3M8 11.5v3M1.5 8h3M11.5 8h3M5.5 5.5h5v5h-5z"/>"#,
            Icon::VertexRemove => r#"<path d="M2 12.5 6.5 7l7.5 3.5M5 5.5h3v3H5zM10.5 3.5H14"/>"#,
            Icon::Bus => r#"<path d="M2 3.5h8.5v6H14M2 6.5h5.5v6H14"/>"#,
            Icon::Callout => r#"<path d="M2.5 2.5h8V7h-8zM6.5 7l4 5.5"/><circle class="f" cx="11" cy="13" r="1.5"/>"#,
            Icon::Routing => {
                r#"<path d="M2.5 12.5H6a2.5 2.5 0 0 0 2.5-2.5V6A2.5 2.5 0 0 1 11 3.5h2.5M11.5 1.5l2 2-2 2"/>"#
            }
            Icon::Page => r#"<path d="M4 2.5h5.5L12 5v8.5H4zM9.5 2.5V5H12"/>"#,
            Icon::AlignLeft => r#"<path d="M2.5 2v12M5 4.5h6V7H5zM5 9h8.5v2.5H5z"/>"#,
            Icon::AlignCentre => r#"<path d="M8 2v12M4.5 4.5h7V7h-7zM3 9h10v2.5H3z"/>"#,
            Icon::AlignRight => r#"<path d="M13.5 2v12M5 4.5h6V7H5zM2.5 9H11v2.5H2.5z"/>"#,
            Icon::AlignTop => r#"<path d="M2 2.5h12M4.5 5H7v6H4.5zM9 5h2.5v8.5H9z"/>"#,
            Icon::AlignMiddle => r#"<path d="M2 8h12M4.5 4.5H7v7H4.5zM9 3h2.5v10H9z"/>"#,
            Icon::AlignBottom => r#"<path d="M2 13.5h12M4.5 5H7v6H4.5zM9 2.5h2.5V11H9z"/>"#,
            Icon::DistributeH => r#"<path d="M2.5 2v12M13.5 2v12M6.5 4.5h3v7h-3z"/>"#,
            Icon::DistributeV => r#"<path d="M2 2.5h12M2 13.5h12M4.5 6.5h7v3h-7z"/>"#,
            Icon::Warn => r#"<path d="M8 2.5 14 13H2zM8 6.5v3M8 11v.5"/>"#,
            Icon::Info => r#"<circle cx="8" cy="8" r="5.5"/><path d="M8 7.5V11M8 5v.5"/>"#,
            Icon::Lock => r#"<path d="M4 7.5h8v6H4zM5.5 7.5v-2a2.5 2.5 0 0 1 5 0v2"/>"#,
            Icon::Tag => r#"<path d="M2.5 2.5H8L13.5 8 8 13.5 2.5 8zM5.5 5.5v.01"/>"#,
            Icon::Layers => r#"<path d="M8 2.5 14 6 8 9.5 2 6zM2.5 9.5 8 12.5l5.5-3"/>"#,
            Icon::Map => r#"<path d="M6.5 2h3v3h-3zM2 11h3v3H2zM11 11h3v3h-3zM8 5v3M3.5 11V8h9v3"/>"#,
            Icon::Graph => {
                r#"<circle cx="4" cy="4" r="1.75"/><circle cx="12" cy="6" r="1.75"/><circle cx="6" cy="12" r="1.75"/><path d="M5.7 4.4 10.3 5.6M4.5 5.7l1 4.6M10.8 7.3 7.2 10.8"/>"#
            }
            Icon::Tree => r#"<path d="M2 6.5h3.5v3H2zM10.5 2.5H14v3h-3.5zM10.5 10.5H14v3h-3.5zM5.5 8H8M10.5 4H8v8h2.5"/>"#,
            Icon::Sort => r#"<path d="M5 3v10M2.5 10.5 5 13l2.5-2.5M11 13V3M8.5 5.5 11 3l2.5 2.5"/>"#,
        }
    }

    /// The whole SVG document of the icon, in white, `stroke` px wide.
    #[must_use]
    pub fn svg(self, stroke: f32) -> String {
        let body = self.body().replace(r#"class="f""#, r##"fill="#fff" stroke="none""##);
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="#fff" stroke-width="{stroke}" stroke-linecap="square" stroke-linejoin="miter">{body}</svg>"##
        )
    }
}

type Cache = Arc<Mutex<BTreeMap<(Icon, u32, u32), Option<TextureHandle>>>>;

/// The white texture of `icon` at `px` physical pixels, made once.
fn texture(ctx: &egui::Context, icon: Icon, px: u32, stroke_milli: u32) -> Option<TextureHandle> {
    let cache: Cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(egui::Id::unique("c4rizer-ui-icons")).clone());
    let mut cache = cache.lock().ok()?;
    let key = (icon, px, stroke_milli);
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }
    #[allow(clippy::cast_precision_loss)]
    let svg = icon.svg(stroke_milli as f32 / 1000.0);
    let (asset, _, _) = c4rizer_core::explore::image_asset(&format!("icon-{icon:?}.svg"), svg.as_bytes())?;
    let texture = c4rizer_core::export::rasterize_asset(&asset, px, px).map(|rgba| {
        let image = egui::ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &rgba);
        ctx.load_texture(format!("ui-icon-{icon:?}-{px}-{stroke_milli}"), image, TextureOptions::LINEAR)
    });
    cache.insert(key, texture.clone());
    texture
}

/// Paints `icon` into `rect` (square) in `colour`.
pub fn paint(painter: &egui::Painter, icon: Icon, rect: Rect, colour: Color32) {
    paint_stroke(painter, icon, rect, colour, 1.5);
}

/// Paints `icon` into `rect` in `colour` with a `stroke` px stroke on the
/// 16 px grid (the checkbox tick is 2).
pub fn paint_stroke(painter: &egui::Painter, icon: Icon, rect: Rect, colour: Color32, stroke: f32) {
    let ppp = painter.ctx().pixels_per_point();
    // Icon sizes are small positive floats; the casts saturate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let px = (rect.width() * ppp).round().max(1.0) as u32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let milli = (stroke * 1000.0).round() as u32;
    if let Some(t) = texture(painter.ctx(), icon, px, milli) {
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(t.id(), rect, uv, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::Icon;

    /// Every icon rasterizes: its SVG is well formed and draws something.
    #[test]
    fn every_icon_draws() {
        for icon in ALL {
            let (asset, w, h) = c4rizer_core::explore::image_asset("i.svg", icon.svg(1.5).as_bytes()).expect("an SVG with a size");
            assert_eq!((w, h), (16, 16));
            let rgba = c4rizer_core::export::rasterize_asset(&asset, 32, 32).expect("decodes");
            assert!(rgba.chunks(4).any(|p| p[3] > 0), "{icon:?} draws nothing");
        }
    }

    const ALL: [Icon; 54] = [
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::ChevronUp,
        Icon::ArrowUp,
        Icon::ArrowDown,
        Icon::Enter,
        Icon::External,
        Icon::PanelLeft,
        Icon::PanelRight,
        Icon::Open,
        Icon::Diagram,
        Icon::Explore,
        Icon::Search,
        Icon::Close,
        Icon::Check,
        Icon::Plus,
        Icon::Minus,
        Icon::Undo,
        Icon::Redo,
        Icon::Edit,
        Icon::Reset,
        Icon::Copy,
        Icon::Export,
        Icon::FitView,
        Icon::FitWidth,
        Icon::FitHeight,
        Icon::Fullscreen,
        Icon::Dark,
        Icon::AutoPlace,
        Icon::Magnet,
        Icon::Centre,
        Icon::VertexRemove,
        Icon::Bus,
        Icon::Callout,
        Icon::Routing,
        Icon::Page,
        Icon::AlignLeft,
        Icon::AlignCentre,
        Icon::AlignRight,
        Icon::AlignTop,
        Icon::AlignMiddle,
        Icon::AlignBottom,
        Icon::DistributeH,
        Icon::DistributeV,
        Icon::Warn,
        Icon::Info,
        Icon::Lock,
        Icon::Tag,
        Icon::Layers,
        Icon::Map,
        Icon::Graph,
        Icon::Tree,
        Icon::Sort,
    ];
}
