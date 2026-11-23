//! A scene, or several, as one PDF document (owner request 2026-11-23,
//! the editor's File > Export): one page per scene, every shape a vector
//! path, every text line the bundled faces' glyph outlines, as
//! `svg_outlined` draws them.
//!
//! Depends on: `scene`, `export` (the SVG writer), usvg (feature
//! `outline`); icons in PNG need resvg's decoder (feature `png`).

// DECISION: the PDF is written by hand from usvg's tree, the same tree
// `svg_outlined` and `png` read, instead of through a PDF crate: usvg is
// already an approved dependency and the scene uses only what a short
// writer covers (solid fills and strokes, dashes, opacity, transforms,
// icons). No new dependency (CLAUDE.md).
// DECISION: text is drawn as glyph outlines, not as embedded fonts: the
// output then looks exactly like the SVG and PNG exports, at the cost of
// text that cannot be selected or searched in a PDF reader.
// DECISION: content and raw image streams are deflated by the small
// encoder in `flate` (no new dependency); JPEG icons pass through as
// they are. Glyph outlines dominate a page, so coordinates are written
// with two decimals (0.01 px), finer than any print.
// DECISION: a group's opacity is multiplied into each fill and stroke
// below it instead of drawn as a transparency group; the scene's
// translucent groups (dimmed boxes, lines, labels) differ only where two
// of their own shapes overlap.
// DECISION: a gradient paint draws as its first stop's colour and a
// pattern paint is skipped; clip paths, masks and filters are ignored.
// The scene's SVG writes none of them.
// DECISION: 1 px = 0.75 pt (96 px per inch), so a page is the view's
// size printed at 100 %.

use std::fmt::Write as _;

use super::PdfError;
use crate::scene::Scene;

mod flate;

/// CSS px to PDF points: 96 px per inch, 72 pt per inch.
const PT_PER_PX: f32 = 0.75;

/// Object numbers fixed by the writer: catalog, page tree, info.
const CATALOG: usize = 1;
const PAGES: usize = 2;
const INFO: usize = 3;

/// The PDF of `scenes`, one page each, in order.
pub(super) fn pdf(scenes: &[&Scene]) -> Result<Vec<u8>, PdfError> {
    if scenes.is_empty() {
        return Err(PdfError::NoPage);
    }
    let opt = super::outline_options();
    let mut doc = Doc::new();
    for _ in [CATALOG, PAGES, INFO] {
        doc.reserve();
    }
    let mut kids = Vec::with_capacity(scenes.len());
    for scene in scenes {
        let tree = usvg::Tree::from_str(&super::svg(scene), &opt).map_err(|e| PdfError::Parse(e.to_string()))?;
        kids.push(page(&mut doc, &tree));
    }
    doc.begin(CATALOG);
    let _ = write!(doc.text, "<< /Type /Catalog /Pages {PAGES} 0 R >>");
    doc.end();
    doc.begin(PAGES);
    let refs: Vec<String> = kids.iter().map(|k| format!("{k} 0 R")).collect();
    let _ = write!(doc.text, "<< /Type /Pages /Kids [{}] /Count {} >>", refs.join(" "), kids.len());
    doc.end();
    doc.begin(INFO);
    doc.text.push_str("<< /Producer (c4rizer) >>");
    doc.end();
    Ok(doc.finish())
}

/// Writes one page for `tree` and returns its object number.
fn page(doc: &mut Doc, tree: &usvg::Tree) -> usize {
    let page_obj = doc.reserve();
    let content_obj = doc.reserve();
    let width = tree.size().width() * PT_PER_PX;
    let height = tree.size().height() * PT_PER_PX;
    let mut painter = Painter {
        doc,
        ops: String::new(),
        alphas: Vec::new(),
        images: Vec::new(),
    };
    // The page's user space is the SVG's: px, y down.
    painter.ops.push_str("1 0 0 -1 0 ");
    num(&mut painter.ops, height);
    painter.ops.push_str(" cm ");
    num(&mut painter.ops, PT_PER_PX);
    painter.ops.push_str(" 0 0 ");
    num(&mut painter.ops, PT_PER_PX);
    painter.ops.push_str(" 0 0 cm\n");
    painter.group(tree.root(), usvg::Transform::identity(), 1.0);
    let Painter { doc, ops, alphas, images } = painter;
    let mut resources = String::from("<<");
    if !alphas.is_empty() {
        resources.push_str(" /ExtGState <<");
        for (i, (fill, stroke)) in alphas.iter().enumerate() {
            let _ = write!(resources, " /G{i} << /ca ");
            permille(&mut resources, *fill);
            resources.push_str(" /CA ");
            permille(&mut resources, *stroke);
            resources.push_str(" >>");
        }
        resources.push_str(" >>");
    }
    if !images.is_empty() {
        resources.push_str(" /XObject <<");
        for (i, obj) in images.iter().enumerate() {
            let _ = write!(resources, " /Im{i} {obj} 0 R");
        }
        resources.push_str(" >>");
    }
    resources.push_str(" >>");
    doc.deflated(content_obj, "", ops.as_bytes());
    doc.begin(page_obj);
    let mut dict = String::new();
    let _ = write!(dict, "<< /Type /Page /Parent {PAGES} 0 R /MediaBox [0 0 ");
    num(&mut dict, width);
    dict.push(' ');
    num(&mut dict, height);
    let _ = write!(dict, "] /Resources {resources} /Contents {content_obj} 0 R >>");
    doc.text.push_str(&dict);
    doc.end();
    page_obj
}

/// The bytes of the document under construction and each object's byte
/// offset, for the cross-reference table.
struct Doc {
    out: Vec<u8>,
    /// Text of the object being written, flushed into `out` by `end`.
    text: String,
    /// Byte offset of object `n` at index `n - 1`; 0 until written.
    offsets: Vec<usize>,
}

impl Doc {
    fn new() -> Self {
        // The comment's four bytes over 127 mark the file as binary.
        let mut out = b"%PDF-1.4\n%".to_vec();
        out.extend_from_slice(&[0xE2, 0xE3, 0xCF, 0xD3, b'\n']);
        Self {
            out,
            text: String::new(),
            offsets: Vec::new(),
        }
    }

    /// A new object number.
    fn reserve(&mut self) -> usize {
        self.offsets.push(0);
        self.offsets.len()
    }

    /// Starts object `n`; its body goes to `text`.
    fn begin(&mut self, n: usize) {
        self.offsets[n - 1] = self.out.len();
        self.text.clear();
        let _ = writeln!(self.text, "{n} 0 obj");
    }

    /// Ends the object `begin` started.
    fn end(&mut self) {
        self.text.push_str("\nendobj\n");
        self.out.extend_from_slice(self.text.as_bytes());
        self.text.clear();
    }

    /// Writes object `n` as a stream with `data` and the extra
    /// dictionary entries `dict`.
    fn stream(&mut self, n: usize, dict: &str, data: &[u8]) {
        self.begin(n);
        let _ = write!(self.text, "<< /Length {}{dict} >>\nstream\n", data.len());
        self.out.extend_from_slice(self.text.as_bytes());
        self.out.extend_from_slice(data);
        self.text.clear();
        self.text.push_str("\nendstream");
        self.end();
    }

    /// `stream` with `data` deflated (`/FlateDecode`).
    fn deflated(&mut self, n: usize, dict: &str, data: &[u8]) {
        let packed = flate::zlib(data);
        self.stream(n, &format!("{dict} /Filter /FlateDecode"), &packed);
    }

    /// The cross-reference table and trailer after every object.
    fn finish(mut self) -> Vec<u8> {
        let xref = self.out.len();
        let mut tail = String::new();
        let _ = write!(tail, "xref\n0 {}\n0000000000 65535 f \n", self.offsets.len() + 1);
        for offset in &self.offsets {
            let _ = writeln!(tail, "{offset:010} 00000 n ");
        }
        let _ = write!(
            tail,
            "trailer\n<< /Size {} /Root {CATALOG} 0 R /Info {INFO} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            self.offsets.len() + 1
        );
        self.out.extend_from_slice(tail.as_bytes());
        self.out
    }
}

/// One page's content stream and the resources it names.
struct Painter<'a> {
    doc: &'a mut Doc,
    ops: String,
    /// Fill and stroke alpha in thousandths, `/G<index>`.
    alphas: Vec<(u16, u16)>,
    /// Image object numbers, `/Im<index>`.
    images: Vec<usize>,
}

impl Painter<'_> {
    /// Draws `group`'s children under `parent` (the transform of the
    /// group's parent) at `opacity` (the parents' opacities multiplied).
    fn group(&mut self, group: &usvg::Group, parent: usvg::Transform, opacity: f32) {
        let ts = parent.pre_concat(group.transform());
        let opacity = opacity * group.opacity().get();
        for node in group.children() {
            match node {
                usvg::Node::Group(g) => self.group(g, ts, opacity),
                usvg::Node::Path(p) => self.path(p, ts, opacity),
                usvg::Node::Image(i) => self.image(i, ts, opacity),
                usvg::Node::Text(t) => self.group(t.flattened(), ts, opacity),
            }
        }
    }

    fn path(&mut self, path: &usvg::Path, ts: usvg::Transform, opacity: f32) {
        if !path.is_visible() {
            return;
        }
        // DECISION: a path is written relative to its bounds' top-left
        // corner (rounded to 0.01 px), which moves into its `cm`: equal
        // shapes are then equal bytes and deflate codes the repeat as one
        // match. usvg merges a text line into one path, so this trims about
        // a tenth; glyph-level reuse would need `ttf-parser` (deferred).
        let bounds = path.data().bounds();
        let origin = ((bounds.left() * 100.0).round() / 100.0, (bounds.top() * 100.0).round() / 100.0);
        let ts = ts.pre_translate(origin.0, origin.1);
        let fill = |me: &mut Self| {
            let Some(fill) = path.fill() else { return };
            let Some(colour) = colour(fill.paint()) else { return };
            me.begin(ts, fill.opacity().get() * opacity, 1.0);
            rgb(&mut me.ops, colour, "rg");
            segments(&mut me.ops, path.data(), origin);
            me.ops.push_str(match fill.rule() {
                usvg::FillRule::NonZero => "f",
                usvg::FillRule::EvenOdd => "f*",
            });
            me.ops.push_str("\nQ\n");
        };
        let stroke = |me: &mut Self| {
            let Some(stroke) = path.stroke() else { return };
            let Some(colour) = colour(stroke.paint()) else { return };
            me.begin(ts, 1.0, stroke.opacity().get() * opacity);
            rgb(&mut me.ops, colour, "RG");
            num(&mut me.ops, stroke.width().get());
            me.ops.push_str(" w ");
            me.ops.push_str(match stroke.linecap() {
                usvg::LineCap::Butt => "0 J ",
                usvg::LineCap::Round => "1 J ",
                usvg::LineCap::Square => "2 J ",
            });
            me.ops.push_str(match stroke.linejoin() {
                usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => "0 j ",
                usvg::LineJoin::Round => "1 j ",
                usvg::LineJoin::Bevel => "2 j ",
            });
            num(&mut me.ops, stroke.miterlimit().get());
            me.ops.push_str(" M ");
            if let Some(dashes) = stroke.dasharray() {
                me.ops.push('[');
                for (i, d) in dashes.iter().enumerate() {
                    if i > 0 {
                        me.ops.push(' ');
                    }
                    num(&mut me.ops, *d);
                }
                me.ops.push_str("] ");
                num(&mut me.ops, stroke.dashoffset());
                me.ops.push_str(" d ");
            }
            me.ops.push('\n');
            segments(&mut me.ops, path.data(), origin);
            me.ops.push_str("S\nQ\n");
        };
        match path.paint_order() {
            usvg::PaintOrder::FillAndStroke => {
                fill(self);
                stroke(self);
            }
            usvg::PaintOrder::StrokeAndFill => {
                stroke(self);
                fill(self);
            }
        }
    }

    /// `q`, the transform, and the graphics state for these alphas.
    fn begin(&mut self, ts: usvg::Transform, fill: f32, stroke: f32) {
        self.ops.push_str("q ");
        for v in [ts.sx, ts.ky, ts.kx, ts.sy, ts.tx, ts.ty] {
            num(&mut self.ops, v);
            self.ops.push(' ');
        }
        self.ops.push_str("cm\n");
        let key = (to_permille(fill), to_permille(stroke));
        if key != (1000, 1000) {
            let at = self.alphas.iter().position(|a| *a == key).unwrap_or_else(|| {
                self.alphas.push(key);
                self.alphas.len() - 1
            });
            let _ = writeln!(self.ops, "/G{at} gs");
        }
    }

    fn image(&mut self, image: &usvg::Image, ts: usvg::Transform, opacity: f32) {
        if !image.is_visible() {
            return;
        }
        let size = image.size();
        let obj = match image.kind() {
            usvg::ImageKind::SVG(tree) => {
                let inner = ts.pre_scale(size.width() / tree.size().width(), size.height() / tree.size().height());
                self.group(tree.root(), inner, opacity);
                return;
            }
            usvg::ImageKind::JPEG(data) => jpeg(self.doc, data),
            usvg::ImageKind::PNG(data) => png(self.doc, data),
            // DECISION: GIF and WebP icons are skipped: no decoder is
            // reachable without a new dependency.
            usvg::ImageKind::GIF(_) | usvg::ImageKind::WEBP(_) => None,
        };
        let Some(obj) = obj else { return };
        let index = self.images.len();
        self.images.push(obj);
        self.begin(ts, opacity, opacity);
        // The image's unit square, y up, onto its box, y down.
        num(&mut self.ops, size.width());
        self.ops.push_str(" 0 0 ");
        num(&mut self.ops, -size.height());
        self.ops.push_str(" 0 ");
        num(&mut self.ops, size.height());
        let _ = write!(self.ops, " cm /Im{index} Do\nQ\n");
    }
}

/// The solid colour a paint draws, if any.
fn colour(paint: &usvg::Paint) -> Option<usvg::Color> {
    match paint {
        usvg::Paint::Color(c) => Some(*c),
        usvg::Paint::LinearGradient(g) => g.stops().first().map(usvg::Stop::color),
        usvg::Paint::RadialGradient(g) => g.stops().first().map(usvg::Stop::color),
        usvg::Paint::Pattern(_) => None,
    }
}

/// `r g b <op>` with each channel in 0..1.
fn rgb(out: &mut String, c: usvg::Color, op: &str) {
    for v in [c.red, c.green, c.blue] {
        num(out, f32::from(v) / 255.0);
        out.push(' ');
    }
    out.push_str(op);
    out.push('\n');
}

/// The path's segments as PDF path operators, relative to `origin`; a
/// quadratic becomes the cubic that draws the same curve.
fn segments(out: &mut String, path: &usvg::tiny_skia_path::Path, origin: (f32, f32)) {
    use usvg::tiny_skia_path::PathSegment;
    let mut at = (0.0_f32, 0.0_f32);
    let mut start = at;
    let point = |out: &mut String, x: f32, y: f32| {
        num(out, x - origin.0);
        out.push(' ');
        num(out, y - origin.1);
        out.push(' ');
    };
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                point(out, p.x, p.y);
                out.push_str("m\n");
                at = (p.x, p.y);
                start = at;
            }
            PathSegment::LineTo(p) => {
                point(out, p.x, p.y);
                out.push_str("l\n");
                at = (p.x, p.y);
            }
            PathSegment::QuadTo(q, p) => {
                let c1 = (at.0 + 2.0 / 3.0 * (q.x - at.0), at.1 + 2.0 / 3.0 * (q.y - at.1));
                let c2 = (p.x + 2.0 / 3.0 * (q.x - p.x), p.y + 2.0 / 3.0 * (q.y - p.y));
                point(out, c1.0, c1.1);
                point(out, c2.0, c2.1);
                point(out, p.x, p.y);
                out.push_str("c\n");
                at = (p.x, p.y);
            }
            PathSegment::CubicTo(c1, c2, p) => {
                point(out, c1.x, c1.y);
                point(out, c2.x, c2.y);
                point(out, p.x, p.y);
                out.push_str("c\n");
                at = (p.x, p.y);
            }
            PathSegment::Close => {
                out.push_str("h\n");
                at = start;
            }
        }
    }
}

/// `v` with at most two decimals, no exponent, no trailing zeros; a
/// value that is not finite is written as 0.
fn num(out: &mut String, v: f32) {
    let v = if v.is_finite() { v } else { 0.0 };
    let mut s = format!("{v:.2}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" {
        "0".clone_into(&mut s);
    }
    out.push_str(&s);
}

/// An alpha in thousandths, clamped to 0..=1000.
fn to_permille(alpha: f32) -> u16 {
    // DECISION: thousandths are finer than any opacity the scene writes
    // (whole percent) and keep one `/G` entry per distinct value.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = (alpha.clamp(0.0, 1.0) * 1000.0).round() as u16;
    v
}

/// Thousandths as a decimal.
fn permille(out: &mut String, v: u16) {
    num(out, f32::from(v) / 1000.0);
}

/// A JPEG as an image object, passed through (`DCTDecode`); `None` when
/// its frame header cannot be found.
fn jpeg(doc: &mut Doc, data: &[u8]) -> Option<usize> {
    let (width, height, components) = jpeg_frame(data)?;
    let space = match components {
        1 => "/DeviceGray",
        3 => "/DeviceRGB",
        4 => "/DeviceCMYK",
        _ => return None,
    };
    let obj = doc.reserve();
    let dict = format!(
        " /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace {space} /BitsPerComponent 8 /Filter /DCTDecode"
    );
    doc.stream(obj, &dict, data);
    Some(obj)
}

/// Width, height and component count from a JPEG's start-of-frame
/// marker.
fn jpeg_frame(data: &[u8]) -> Option<(u16, u16, u8)> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        // SOF0..SOF15 except DHT (C4), JPG (C8) and DAC (CC).
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let f = data.get(i + 4..i + 10)?;
            let height = u16::from_be_bytes([f[1], f[2]]);
            let width = u16::from_be_bytes([f[3], f[4]]);
            return Some((width, height, f[5]));
        }
        i += 2 + len;
    }
    None
}

/// A PNG decoded and written as an RGB image object with its alpha as a
/// soft mask; `None` when it does not decode or without feature `png`.
#[cfg(feature = "png")]
fn png(doc: &mut Doc, data: &[u8]) -> Option<usize> {
    let pixmap = resvg::tiny_skia::Pixmap::decode_png(data).ok()?;
    let mut colour = Vec::with_capacity(pixmap.pixels().len() * 3);
    let mut alpha = Vec::with_capacity(pixmap.pixels().len());
    for px in pixmap.pixels() {
        let c = px.demultiply();
        colour.extend_from_slice(&[c.red(), c.green(), c.blue()]);
        alpha.push(c.alpha());
    }
    let (width, height) = (pixmap.width(), pixmap.height());
    let mask = if alpha.iter().all(|a| *a == 255) {
        String::new()
    } else {
        let obj = doc.reserve();
        let dict = format!(
            " /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 8"
        );
        doc.deflated(obj, &dict, &alpha);
        format!(" /SMask {obj} 0 R")
    };
    let obj = doc.reserve();
    let dict = format!(
        " /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8{mask}"
    );
    doc.deflated(obj, &dict, &colour);
    Some(obj)
}

#[cfg(not(feature = "png"))]
fn png(_doc: &mut Doc, _data: &[u8]) -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicBool;

    use crate::label::place_labels;
    use crate::model::parse;
    use crate::place::{Keep, place};
    use crate::route::{RouteConfig, route};
    use crate::scene::build;
    use crate::view::plan;

    const SHOP: &str = r#"workspace {
  model {
    operator = person "Operator" "Runs the shop."
    shop = softwareSystem "Shop" "Sells goods online." {
      web = container "Web" "Serves pages." "Rust"
      db = container "Database" "Stores orders." "SQLite" "Database"
    }
    operator -> web "Uses" "HTTPS"
    web -> db "Reads" "SQL"
  }
  views {
    systemContext shop "c1" "Shop context" {
      include *
    }
    container shop "c2" "Shop containers" {
      include *
    }
    styles {
      element "Database" {
        shape Cylinder
      }
    }
  }
}
"#;

    fn scene_of(key: &str) -> Scene {
        let model = parse(SHOP).expect("parses").model;
        let plan = plan(&model, model.view(key).expect("view"));
        let centres = place(&plan, &Keep::default());
        let routes = route(&plan, &centres, &RouteConfig::DEFAULT, &AtomicBool::new(false)).expect("not stopped");
        let labels = place_labels(&plan, &routes);
        build(&plan, &routes, &labels)
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Every cross-reference entry points at the start of its object.
    fn assert_xref(bytes: &[u8]) {
        // Offsets count bytes; the header's binary comment is not UTF-8,
        // so the tail is read from the raw bytes.
        let at = bytes.windows(10).rposition(|w| w == b"startxref\n").expect("startxref");
        let start: usize = text(&bytes[at + 10..]).lines().next().and_then(|n| n.parse().ok()).expect("offset");
        assert!(bytes[start..].starts_with(b"xref\n"), "startxref points at the table");
        let table = text(&bytes[start..]);
        let entries: Vec<&str> = table.lines().skip(3).take_while(|l| !l.starts_with("trailer")).collect();
        assert!(!entries.is_empty());
        for (i, entry) in entries.iter().enumerate() {
            let offset: usize = entry[..10].parse().expect("offset");
            let head = format!("{} 0 obj", i + 1);
            assert!(bytes[offset..].starts_with(head.as_bytes()), "object {} at its offset", i + 1);
        }
    }

    #[test]
    fn pdf_one_view_is_one_page_of_paths() {
        let scene = scene_of("c1");
        let bytes = pdf(&[&scene]).expect("pdf");
        assert!(bytes.starts_with(b"%PDF-1.4\n"));
        assert!(bytes.ends_with(b"%%EOF\n"));
        let s = text(&bytes);
        assert_eq!(s.matches("/Type /Page ").count(), 1);
        assert!(s.contains("/Count 1"));
        // Text is outlined: no font resource; the content is deflated.
        assert!(!s.contains("/Font"));
        assert!(s.contains("/Filter /FlateDecode"));
        assert_xref(&bytes);
    }

    #[test]
    fn pdf_page_size_is_the_view_at_96_dpi() {
        let scene = scene_of("c1");
        let bytes = pdf(&[&scene]).expect("pdf");
        let s = text(&bytes);
        let svg = crate::export::svg(&scene);
        let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("svg");
        let mut want = String::from("/MediaBox [0 0 ");
        num(&mut want, tree.size().width() * 0.75);
        want.push(' ');
        num(&mut want, tree.size().height() * 0.75);
        want.push(']');
        assert!(s.contains(&want), "{want} in the page");
    }

    #[test]
    fn pdf_many_views_are_pages_in_order() {
        let (c1, c2) = (scene_of("c1"), scene_of("c2"));
        let bytes = pdf(&[&c1, &c2]).expect("pdf");
        let s = text(&bytes);
        assert_eq!(s.matches("/Type /Page ").count(), 2);
        assert!(s.contains("/Count 2"));
        assert_xref(&bytes);
        // Same input, same bytes.
        assert_eq!(bytes, pdf(&[&c1, &c2]).expect("pdf"));
    }

    #[test]
    fn pdf_without_scene_is_refused() {
        assert!(matches!(pdf(&[]), Err(PdfError::NoPage)));
    }

    #[test]
    fn pdf_numbers_have_no_exponent_nor_trailing_zeros() {
        let mut s = String::new();
        for v in [0.0, -0.001, 1.5, 2.0, 1e-9, 123_456.79, 0.126, f32::NAN] {
            num(&mut s, v);
            s.push(' ');
        }
        assert_eq!(s, "0 0 1.5 2 0 123456.79 0.13 0 ");
    }

    #[test]
    fn pdf_jpeg_frame_is_read() {
        // SOI, APP0 (len 4), SOF0: precision 8, height 2, width 3, 3 components.
        let data = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x02, 0x00, 0x03, 0x03,
        ];
        assert_eq!(jpeg_frame(&data), Some((3, 2, 3)));
        assert_eq!(jpeg_frame(b"not a jpeg"), None);
    }
}
