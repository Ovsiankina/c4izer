//! Writes a scene as SVG text, and as PNG bytes behind the `png`
//! feature (EXP-1, 2); writes a model's views in Structurizr's text
//! formats (`text`), as Structurizr workspace JSON and as a Structurizr
//! theme (SRS 4.7, row exporters: P3-400..P3-424).
//!
//! Depends on: `scene`, `model`, `pipeline`, `inspect`.

// Shape of the API: `svg(&Scene) -> String` and, with feature `png`,
// `png(&Scene, width_px) -> Result<Vec<u8>, PngError>`. Both take
// the scene, not SVG text, so a PNG is always the view's own SVG
// rasterized (EXP-2) and the CLI (CLI-1, CLI-4) and the editor (EDIT-7)
// call the same two functions. Export draws exactly the scene's items in
// the scene's paint order and computes no geometry, colour or wrap of its
// own (CORE-1, EDIT-12).
//
// `png` renders with the bundled faces `scene::FONT_REGULAR` and
// `scene::FONT_BOLD` (owner's ruling: the font bytes live in `scene`
// beside `FONT_FAMILY`), not with a fonts parameter. No system font is
// ever loaded (EXP-2): resvg is built without `system-fonts`
// (crates/core/Cargo.toml).
// DECISION: characters XML 1.0 cannot hold become U+FFFD rather than
// being dropped, so a stray control character shows as a visible mark.
// DECISION: `png` checks width, bounds and size before loading the fonts,
// so those errors come first and cost nothing.
// DECISION: `png` may parse the two bundled faces on every call, so the
// editor's "export all views" (EDIT-7) loads them once per view; caching
// a parsed font set is a step-5 choice that changes no signature.
// DECISION: `png` and `PngError` exist only with feature `png`; the CLI
// tells a missing feature from a failed render at compile time (cli plan,
// Review Focus: exit 2 with a clear message).
//
// EXP-1: the SVG does not embed the font; it names it in `font-family`
// with a `sans-serif` fallback (`font-family="Inter, sans-serif"`) and
// carries no `@font-face`, so a viewer without Inter falls back to its
// sans-serif and the golden SVGs (CLI-5) carry no font bytes. An option
// to embed the font or outline the text is phase 2 (P2-25).
//
// The editor depends on `c4rizer-core` with feature `png` (EDIT-7), so
// the workspace oracle (`cargo test --workspace`, `cargo clippy
// --workspace`) compiles, lints and tests the `png` code through it.
//
// CORE-25's own test ("build each `example/` view twice and compare SVG
// bytes") needs the whole pipeline, which export cannot run; it is asked
// of pipeline in docs/deferred.md (`core_25_example_svg_bytes_identical`).
// Here `core_25_svg_deterministic_bytes` covers only the export half.
//
// redesign 2026-11-19 (owner-approved visual redesign, design-system
// Edge/Node): the renderers draw a line's stroke 1.5px when the scene
// gives the default 2px, round its orthogonal corners (10px, at most half
// the shorter leg), end it in a notched arrowhead (9 long, 7 wide, 2.5
// notch) with a paper-filled port dot (r 2.5) at its source, set labels
// on a paper plate (6x4 padding, 3px corners), cut a person's head from
// its body with a 2.5px paper ring and draw a cylinder's lid in the text
// colour. DECISION: this drawing geometry is computed here and, the same
// way, in the editor's canvas.rs from the scene's own points, so the
// scene's pub structs stay as they are (no new pub field); numbers it
// makes are written with at most one decimal (CORE-25 allows
// fixed-precision decimals), so equal scenes still give equal bytes.

use std::fmt::Write as _;

use crate::geom::{Point, Rect};
use crate::label::Weight;
use crate::model::{Colour, Mime};
use crate::scene::{
    Align, FONT_FAMILY, Outline, Scene, SceneIcon, Stroke, Swatch, TextBlock, Tip, WindowKind,
};
#[cfg(feature = "png")]
use crate::scene::{FONT_BOLD, FONT_REGULAR};

mod json_out;
mod static_out;
mod structurizr;
pub mod text;
mod theme_out;

/// The two `PlantUML` exporters of Structurizr 6.2.4 (`export -f
/// plantuml/structurizr`, `-f plantuml/c4plantuml`; `-f plantuml` is
/// `Structurizr`). Serves P3-406, P3-407.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flavour {
    /// `StructurizrPlantUMLExporter`: `<style>` classes, a `-key.puml`
    /// legend per view (P3-404, P3-406).
    Structurizr,
    /// `C4PlantUMLExporter`: the C4-PlantUML macros (P3-407).
    C4PlantUml,
}

/// The workspace as Structurizr workspace JSON, the text of
/// `<file stem>.json` (P3-413): every element, relationship (implied and
/// copied ones included), view, style, theme reference, property,
/// documentation section and decision (P3-414), each view element at its
/// box's top-left in px from the scene of `all` and each relationship
/// view with its vertices (P3-318), and the workspace properties
/// `structurizr.dsl` (base64 of `dsl`, the root file's text, when given)
/// and `structurizr.inspection.error|warning|info|ignore` (the counts of
/// `inspect::inspect`, P3-415). Ids are Structurizr's (P3-408).
/// Deterministic, `\n` line endings and a final newline (P3-403).
/// Serves P3-413, P3-414, P3-415, P3-318.
pub fn json(model: &crate::model::Model, all: &crate::pipeline::AllViews, dsl: Option<&str>) -> String {
    json_out::workspace(model, all, dsl)
}

/// The workspace's own element and relationship styles (not those from
/// themes) as a Structurizr theme, the text of `workspace-theme.json`:
/// `name`, `description`, `elements`, `relationships` in Structurizr's
/// order (P3-421). Loaded back with `theme <file>`, it gives the same
/// resolved styles (P3-422). Serves P3-421, P3-422.
pub fn theme(model: &crate::model::Model) -> String {
    theme_out::write(model)
}

/// c4rizer's own script-free static site (P3-423, ruling E4): `index.html`
/// listing every view in view-list order with title and key (P3-424),
/// one `<key>.svg` per drawable view (P2-659 `name` mode) and one HTML
/// page per view linking its enterable boxes and back to the index.
/// Paths are relative to the output folder. Serves P3-423, P3-424.
/// Every page needs no script and no network and passes the EXP-1 scan;
/// view pages are `view-<key>.html` (`export/static_out.rs` DECISION).
/// Deterministic (P3-403).
pub fn static_site(
    model: &crate::model::Model,
    all: &crate::pipeline::AllViews,
) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    static_out::site(model, all)
}

// DECISION: each item's `<g>` carries a `class` naming its kind
// (`boundary`, `line`, `box`, `label`, `title`, `subtitle`, `key`), so a
// reader of the file (and the paint-order test) tells the items apart;
// a class is no reference and nothing styles it.
// DECISION: `font-family` sits on every `<text>` rather than once on the
// root, so no viewer default or wrapper can come between a text and
// Inter.
// DECISION: the root carries `xml:space="preserve"`: the scene's lines
// are final (CORE-30), so runs of spaces must not collapse.
// DECISION: one item per output line (`\n` after the background and
// each `<g>`), so two exports diff line by line.

/// The scene as one standalone SVG document (EXP-1, CORE-30, CORE-25).
///
/// One `<svg>` whose `viewBox`, `width` and `height` are `scene.bounds`
/// in px, `scene.background` filled over `bounds`, then every item in the
/// scene's paint order (an image view's picture or placeholder,
/// boundaries, lines with chevrons and tip, boxes,
/// labels, title, subtitle, key), each in its own `<g>`. Texts are drawn
/// line by line exactly as `TextBlock::lines` holds them, never
/// re-wrapped, with `font-family` naming `scene::FONT_FAMILY` first and a
/// `sans-serif` fallback (CORE-30), bold for `Weight::Bold`. No external
/// reference (EXP-1): no `<script>`, no event attribute, no `xlink:`
/// attribute and no `href` except an icon's or an image view's
/// `<image href="data:...">` (P2-409, P2-340; a `data:` URI fetches
/// nothing, so no `<use>` either), no
/// `<foreignObject>`, no `url(...)`, no `@import` or `@font-face`; the
/// only URI in the document is the `xmlns` namespace
/// `http://www.w3.org/2000/svg`, which nothing fetches. Text and
/// attribute values are XML-escaped (`<`, `>`, `&`, `"`, `'`); a
/// character XML 1.0 cannot hold at all (U+0000..U+001F except tab, LF
/// and CR, and U+FFFE, U+FFFF) is written as U+FFFD, so the document
/// always parses. Every
/// number is an integer, so an equal scene gives the same bytes on every
/// run and machine (CORE-25). Serves EXP-1, CORE-25, CORE-30, CLI-1,
/// EDIT-7.
#[must_use]
pub fn svg(scene: &Scene) -> String {
    let mut w = Writer {
        paper: scene.background,
        mode: scene.mode,
        out: String::new(),
    };
    let b = scene.bounds;
    w.raw("<svg xmlns=\"http://www.w3.org/2000/svg\"");
    w.attr_str(
        "viewBox",
        &format!("{} {} {} {}", b.min.x, b.min.y, b.width(), b.height()),
    );
    w.attr_num("width", b.width());
    w.attr_num("height", b.height());
    w.raw(" xml:space=\"preserve\">\n");
    w.rect(b, 0, Some(scene.background), None);
    w.raw("\n");
    if let Some(image) = &scene.image {
        // P2-320, P2-340: the file inside a `data:` URI; P2-321: a dashed
        // placeholder naming the source.
        w.open("image");
        match (&image.asset_drawn(), crate::scene::placeholder_text(image)) {
            (Some(asset), _) => w.picture(image.rect, asset),
            (None, Some(text)) => {
                let stroke = Stroke {
                    colour: text.colour,
                    width_px: 1,
                    dash_px: Some([8, 6]),
                };
                w.rect(image.rect, 0, None, Some(&stroke));
                w.text(&text);
            }
            (None, None) => {}
        }
        w.close();
    }
    for boundary in &scene.boundaries {
        w.open_with("boundary", boundary.opacity);
        w.rect(boundary.rect, 0, boundary.fill, Some(&boundary.stroke));
        w.text(&boundary.tag);
        boundary.instances.iter().for_each(|count| w.text(count)); // P3-115
        w.close();
    }
    for line in &scene.lines {
        w.line(line);
    }
    for item in &scene.boxes {
        w.open_with("box", item.opacity);
        // redesign 2026-11-19: a cylinder's lid line in the text colour
        // at 45% over the fill (design-system `.dg-lid`).
        let lid = over(item.name.colour, LID_OPACITY, item.fill);
        w.box_outline(&item.outline, item.fill, &item.stroke, Some(lid));
        if let Some(icon) = &item.icon {
            w.image(icon);
        }
        w.text(&item.name);
        w.text(&item.type_line);
        if let Some(description) = &item.description {
            w.text(description);
        }
        w.close();
    }
    for label in &scene.labels {
        w.label(label);
    }
    for (class, text) in [("title", &scene.title), ("subtitle", &scene.subtitle)] {
        if let Some(text) = text {
            w.open(class);
            w.text(text);
            w.close();
        }
    }
    for entry in &scene.legend.entries {
        w.open("key");
        match &entry.swatch {
            Swatch::Box {
                outline,
                fill,
                stroke,
            } => w.outline(outline, *fill, stroke),
            Swatch::Line {
                from,
                to,
                stroke,
                tip,
            } => {
                // The key's line sample: route, head, port, as a line.
                w.route(&[vec![*from, *to]], &[], stroke, tip);
                w.head(tip, stroke);
                w.port(*from, stroke);
            }
        }
        w.text(&entry.text);
        w.close();
    }
    w.raw("</svg>\n");
    w.out
}

/// The scene as `svg` writes it, plus one `<style>` holding one
/// `@font-face` per bundled face the scene's text uses (`--svg-text
/// embed`): `font-family` `scene::FONT_FAMILY`, `font-weight` `normal`
/// for `scene::FONT_REGULAR` and `bold` for `scene::FONT_BOLD`, `src:
/// url(data:font/ttf;base64,...)` holding the whole face. Apart from those
/// `data:` URIs the document passes the EXP-1 scan. Serves P2-651,
/// P2-654..P2-657.
// TODO(srs): P2-656, font subsetting. Reading taken (B cli-breadth 10):
// the whole bundled face, no subsetting (a subsetter is a new
// dependency).
// DECISION: the `<style>` is the root's first child, on its own line
// right after the root tag, one rule per line; the rest of the document
// is `svg`'s, byte for byte. The base64 is the icon encoder's (std only,
// RFC 4648 with padding).
#[must_use]
pub fn svg_embedded(scene: &Scene) -> String {
    let named = svg(scene);
    let (regular, bold) = faces_used(scene);
    let mut style = String::from("<style>");
    for (used, weight, bytes) in [
        (regular, "normal", crate::scene::FONT_REGULAR),
        (bold, "bold", crate::scene::FONT_BOLD),
    ] {
        if !used {
            continue;
        }
        if !style.ends_with('>') {
            style.push('\n');
        }
        let _ = write!(
            style,
            "@font-face{{font-family:\"{FONT_FAMILY}\";font-weight:{weight};src:url(data:font/ttf;base64,"
        );
        base64_into(&mut style, bytes);
        style.push_str(")}");
    }
    style.push_str("</style>\n");
    // The root tag is `svg`'s first line.
    let split = named.find('\n').map_or(named.len(), |i| i + 1);
    let mut out = String::with_capacity(named.len() + style.len());
    out.push_str(&named[..split]);
    out.push_str(&style);
    out.push_str(&named[split..]);
    out
}

/// Whether the scene draws any regular and any bold text line (P2-655).
fn faces_used(scene: &Scene) -> (bool, bool) {
    let mut blocks: Vec<&TextBlock> = scene.boundaries.iter().map(|b| &b.tag).collect();
    blocks.extend(scene.boundaries.iter().filter_map(|b| b.instances.as_ref()));
    for b in &scene.boxes {
        blocks.extend([&b.name, &b.type_line]);
        blocks.extend(b.description.iter());
    }
    blocks.extend(scene.labels.iter().map(|l| &l.text));
    blocks.extend(scene.title.iter().chain(scene.subtitle.iter()));
    blocks.extend(scene.legend.entries.iter().map(|e| &e.text));
    let drawn = blocks.iter().filter(|t| !t.lines.is_empty());
    let (mut regular, mut bold) = (false, false);
    for t in drawn {
        if t.weight == Weight::Bold {
            bold = true;
        } else {
            regular = true;
        }
    }
    (regular, bold)
}

/// The scene as `svg` writes it with every text line drawn as filled
/// paths from the bundled faces' glyph outlines (`--svg-text outline`): no
/// `<text>`, no `font-family`, no `@font-face`, and no external
/// reference (EXP-1). Serves P2-651, P2-654, P2-658..P2-660.
///
/// # Errors
///
/// `OutlineError::Parse` when usvg cannot read the named SVG (never
/// expected: `svg` always writes a valid document).
// DECISION: usvg reads the named SVG with a font database holding only
// the two bundled faces (the regular one also as `sans-serif`, like
// `png`), and its writer turns every text into the glyph paths it shaped
// (`preserve_text: false`). The writer's own layout (indent, number
// precision 8) is kept: the output is no longer one item per line.
#[cfg(feature = "outline")]
pub fn svg_outlined(scene: &Scene) -> Result<String, OutlineError> {
    let mut db = usvg::fontdb::Database::new();
    db.load_font_data(crate::scene::FONT_REGULAR.to_vec());
    db.load_font_data(crate::scene::FONT_BOLD.to_vec());
    db.set_sans_serif_family(FONT_FAMILY);
    let opt = usvg::Options {
        font_family: FONT_FAMILY.to_owned(),
        fontdb: std::sync::Arc::new(db),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&svg(scene), &opt)
        .map_err(|e| OutlineError::Parse(e.to_string()))?;
    Ok(plain_image_href(&tree.to_string(&usvg::WriteOptions {
        preserve_text: false,
        ..usvg::WriteOptions::default()
    })))
}

/// usvg's writer output with each icon (P2-409) spelled as `svg` spells
/// it, `<image href="data:image/<type>;base64,...">`, and without the
/// `xmlns:xlink` declaration once no `xlink:` attribute is left, so an
/// outlined view with an icon passes EXP-1 like its `name` SVG (P2-659).
// DECISION: a narrow text post-pass over usvg 0.50's writer (it has no
// option for SVG 2 `href`). Only ` xlink:href="data:` is rewritten: that
// byte run can only be an attribute start, since the writer escapes `"`
// inside values and outlined text leaves no character data. Any other
// `xlink:` use (`#id` refs of textPath, feImage; the scene has none) is
// left as written, keeps its namespace and fails the EXP-1 tests.
#[cfg(feature = "outline")]
fn plain_image_href(svg: &str) -> String {
    const XLINK_DATA: &str = " xlink:href=\"data:image/";
    const XMLNS_XLINK: &str = " xmlns:xlink=\"http://www.w3.org/1999/xlink\"";
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(at) = rest.find(XLINK_DATA) {
        out.push_str(&rest[..at]);
        out.push_str(" href=\"data:image/");
        rest = &rest[at + XLINK_DATA.len()..];
        // usvg writes `;base64, <data>`; a data URI has no space there.
        let value_end = rest.find('"').unwrap_or(rest.len());
        if let Some(b64) = rest[..value_end].find(";base64, ") {
            out.push_str(&rest[..b64]);
            out.push_str(";base64,");
            rest = &rest[b64 + ";base64, ".len()..];
        }
    }
    out.push_str(rest);
    if !out.contains("xlink:") {
        out = out.replacen(XMLNS_XLINK, "", 1);
    }
    out
}

/// Why `svg_outlined` produced no SVG. Serves P2-658, CLI exit 2.
#[cfg(feature = "outline")]
#[non_exhaustive]
#[derive(Debug)]
pub enum OutlineError {
    /// usvg refused the named SVG; the text says why.
    Parse(String),
}

#[cfg(feature = "outline")]
impl std::fmt::Display for OutlineError {
    /// One sentence saying what failed and what to do (SRS 5.1).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(why) => write!(
                f,
                "The SVG could not be outlined ({why}); export it with --svg-text name or embed instead."
            ),
        }
    }
}

#[cfg(feature = "outline")]
impl std::error::Error for OutlineError {}

/// The SVG text under construction. Writing to a `String` cannot fail,
/// so `write!` results are dropped.
struct Writer {
    out: String,
    /// The page colour (`Scene::background`): label plates, the port
    /// dot's fill and the person's head ring (redesign 2026-11-19).
    paper: Colour,
    /// The scene's mode, for the label technology colour.
    mode: crate::model::Mode,
}

impl Writer {
    fn raw(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn attr_num(&mut self, name: &str, value: i32) {
        let _ = write!(self.out, " {name}=\"{value}\"");
    }

    fn attr_str(&mut self, name: &str, value: &str) {
        let _ = write!(self.out, " {name}=\"");
        escape_into(&mut self.out, value);
        self.out.push('"');
    }

    fn attr_colour(&mut self, name: &str, value: Colour) {
        let _ = write!(self.out, " {name}=\"{value}\"");
    }

    fn open(&mut self, class: &str) {
        let _ = write!(self.out, "<g class=\"{class}\">");
    }

    /// `open` with an `opacity` attribute when `opacity` is below 100
    /// (P2-406, P2-426): `0.<two digits>` with trailing zeros dropped.
    // DECISION: the opacity is written as a decimal made from the integer
    // percentage by text (no float formatting), so equal scenes still
    // give equal bytes (CORE-25).
    fn open_with(&mut self, class: &str, opacity: u8) {
        if opacity >= 100 {
            self.open(class);
            return;
        }
        let digits = format!("{opacity:02}");
        let digits = digits.trim_end_matches('0');
        let value = if digits.is_empty() {
            "0".to_owned()
        } else {
            format!("0.{digits}")
        };
        let _ = write!(self.out, "<g class=\"{class}\" opacity=\"{value}\">");
    }

    fn close(&mut self) {
        self.out.push_str("</g>\n");
    }

    fn fill(&mut self, fill: Option<Colour>) {
        match fill {
            Some(c) => self.attr_colour("fill", c),
            None => self.raw(" fill=\"none\""),
        }
    }

    fn stroke(&mut self, stroke: Option<&Stroke>) {
        if let Some(s) = stroke {
            self.attr_colour("stroke", s.colour);
            self.attr_num("stroke-width", s.width_px);
            if let Some([dash, gap]) = s.dash_px {
                let _ = write!(self.out, " stroke-dasharray=\"{dash} {gap}\"");
            }
        }
    }

    fn rect(&mut self, r: Rect, radius_px: i32, fill: Option<Colour>, stroke: Option<&Stroke>) {
        self.raw("<rect");
        self.attr_num("x", r.min.x);
        self.attr_num("y", r.min.y);
        self.attr_num("width", r.width());
        self.attr_num("height", r.height());
        if radius_px > 0 {
            self.attr_num("rx", radius_px);
        }
        self.fill(fill);
        self.stroke(stroke);
        self.raw("/>");
    }

    fn circle(&mut self, centre: Point, radius_px: i32, fill: Colour, stroke: Option<&Stroke>) {
        self.raw("<circle");
        self.attr_num("cx", centre.x);
        self.attr_num("cy", centre.y);
        self.attr_num("r", radius_px);
        self.attr_colour("fill", fill);
        self.stroke(stroke);
        self.raw("/>");
    }

    fn path(&mut self, d: &str, fill: Option<Colour>, stroke: Option<&Stroke>) {
        self.raw("<path");
        self.attr_str("d", d);
        self.fill(fill);
        self.stroke(stroke);
        self.raw("/>");
    }

    /// An open polyline: a line's route, a chevron's V, a key sample.
    fn polyline(&mut self, points: &[Point], stroke: &Stroke) {
        self.raw("<path");
        self.attr_str("d", &path_data(points, false));
        self.raw(" fill=\"none\"");
        self.stroke(Some(stroke));
        // DECISION: round joins, as the prototype's lines, so a
        // 45° bend and a chevron apex show no mitre spike.
        self.raw(" stroke-linejoin=\"round\"/>");
    }

    /// One line group: route (or its runs and T bars, P4-519), chevrons,
    /// tip.
    fn line(&mut self, line: &crate::scene::SceneLine) {
        self.open_with("line", line.opacity);
        // DECISION (scene): chevrons and the tip are solid on a dashed
        // line (P2-420).
        let solid = Stroke {
            dash_px: None,
            ..line.stroke
        };
        // P4-519: a bridged line's runs, then its T bars (solid, as the
        // chevrons).
        let whole = [line.points.clone()];
        let runs: &[Vec<Point>] = if line.runs.is_empty() { &whole } else { &line.runs };
        self.route(runs, &line.jumps, &line.stroke, &line.tip);
        let width = edge_width(line.stroke.width_px);
        for bar in &line.bars {
            self.edge_stroke(&path_data(bar, false), &solid, width);
        }
        for chevron in &line.chevrons {
            self.edge_stroke(&path_data(&chevron.points, false), &solid, width);
        }
        self.head(&line.tip, &line.stroke);
        if let Some(&source) = line.points.first() {
            self.port(source, &line.stroke);
        }
        self.close();
    }

    /// A line's runs: rounded corners, a 6px half-circle hop at each of
    /// `jumps` (P2-422), the run reaching the arrow's tip stopped in its
    /// notch.
    fn route(&mut self, runs: &[Vec<Point>], jumps: &[Point], stroke: &Stroke, tip: &Tip) {
        let width = edge_width(stroke.width_px);
        for run in runs {
            let trim = match tip {
                Tip::Arrow(t) if run.last() == Some(&t[0]) => head_trim(stroke.width_px),
                _ => 0.0,
            };
            let d = svg_path(&edge_path(run, jumps, trim));
            self.edge_stroke(&d, stroke, width);
        }
    }

    /// An open path in `stroke`'s colour and dash at `width` px, round
    /// joins.
    fn edge_stroke(&mut self, d: &str, stroke: &Stroke, width: f64) {
        self.raw("<path");
        self.attr_str("d", d);
        self.raw(" fill=\"none\"");
        self.attr_colour("stroke", stroke.colour);
        let _ = write!(self.out, " stroke-width=\"{}\"", num(width));
        if let Some([dash, gap]) = stroke.dash_px {
            let _ = write!(self.out, " stroke-dasharray=\"{dash} {gap}\"");
        }
        self.raw(" stroke-linejoin=\"round\"/>");
    }

    /// The notched arrowhead, or the join dot (redesign 2026-11-19).
    fn head(&mut self, tip: &Tip, stroke: &Stroke) {
        match tip {
            Tip::Arrow(t) => {
                let pts = notched_head(t, stroke.width_px);
                let mut d = String::new();
                for (i, (x, y)) in pts.iter().enumerate() {
                    let _ = write!(d, "{}{} {} ", if i == 0 { 'M' } else { 'L' }, num(*x), num(*y));
                }
                d.push('Z');
                self.path(&d, Some(stroke.colour), None);
            }
            other => self.tip(other, stroke.colour),
        }
    }

    /// The port: a paper dot ringed in the line's colour at its source.
    fn port(&mut self, at: Point, stroke: &Stroke) {
        self.raw("<circle");
        self.attr_num("cx", at.x);
        self.attr_num("cy", at.y);
        let _ = write!(self.out, " r=\"{}\"", num(PORT_RADIUS_PX));
        self.attr_colour("fill", self.paper);
        self.attr_colour("stroke", stroke.colour);
        let _ = write!(self.out, " stroke-width=\"{}\"", num(edge_width(stroke.width_px)));
        self.raw("/>");
    }

    fn ellipse(&mut self, rect: Rect, fill: Option<Colour>, stroke: Option<&Stroke>) {
        let c = rect.centre();
        self.raw("<ellipse");
        self.attr_num("cx", c.x);
        self.attr_num("cy", c.y);
        self.attr_num("rx", rect.width() / 2);
        self.attr_num("ry", rect.height() / 2);
        self.fill(fill);
        self.stroke(stroke);
        self.raw("/>");
    }

    /// One label group: backing, callout marks (P4-316), text.
    fn label(&mut self, label: &crate::scene::SceneLabel) {
        self.open_with("label", label.opacity);
        if let Some(fill) = label.fill {
            // redesign 2026-11-19: a plate 6x4px around the text, 3px
            // corners (design-system Edge).
            self.rect(label_plate(&label.text), PLATE_RADIUS_PX, Some(fill), None);
        }
        // P4-316: a callout's shelf, leader and dot, before its text.
        if let Some(c) = &label.callout {
            self.callout(c);
        }
        let tech = crate::scene::label_tech_colour(label.text.colour, self.mode);
        self.text_tech(&label.text, Some(tech));
        self.close();
    }

    /// A callout's shelf, leader and dot (P4-316).
    fn callout(&mut self, c: &crate::scene::SceneCallout) {
        self.polyline(&c.shelf, &c.stroke);
        self.polyline(&c.leader, &c.stroke);
        self.tip(
            &Tip::Dot {
                at: c.dot,
                radius_px: c.dot_radius_px,
            },
            c.stroke.colour,
        );
    }

    fn tip(&mut self, tip: &Tip, colour: Colour) {
        match tip {
            Tip::Arrow(points) => self.path(&path_data(points, true), Some(colour), None),
            Tip::Dot { at, radius_px } => self.circle(*at, *radius_px, colour, None),
        }
    }

    fn outline(&mut self, outline: &Outline, fill: Colour, stroke: &Stroke) {
        self.box_outline(outline, fill, stroke, None);
    }

    /// `outline`, a cylinder's lid line in `lid` (else the stroke colour).
    fn box_outline(&mut self, outline: &Outline, fill: Colour, stroke: &Stroke, lid: Option<Colour>) {
        match *outline {
            Outline::Rect { rect, radius_px } => {
                self.rect(rect, radius_px, Some(fill), Some(stroke));
            }
            Outline::Person {
                head_centre,
                head_radius_px,
                body,
                body_radius_px,
            } => {
                self.rect(body, body_radius_px, Some(fill), Some(stroke));
                // redesign 2026-11-19: the head cut from the body by a
                // 2.5px paper ring.
                // DECISION: on boxes only (`lid` is given for boxes); a key
                // swatch's 3px head would vanish in the ring.
                if lid.is_some() {
                self.raw("<circle");
                self.attr_num("cx", head_centre.x);
                self.attr_num("cy", head_centre.y);
                let _ = write!(self.out, " r=\"{}\"", num(f64::from(head_radius_px) + HEAD_RING_PX));
                self.attr_colour("fill", self.paper);
                self.raw("/>");
                }
                self.circle(head_centre, head_radius_px, fill, Some(stroke));
            }
            Outline::Cylinder { rect, cap_ry_px } => {
                // DECISION: the arcs' x radius is half the width rounded
                // down; for an odd width SVG scales a too-small radius up
                // to fit exactly, so the cap still spans the width.
                let (x, y, w, h) = (rect.min.x, rect.min.y, rect.width(), rect.height());
                let (rx, ry) = (w / 2, cap_ry_px);
                let top = y + ry;
                let body = format!(
                    "M{x},{top} A{rx},{ry} 0 0 1 {},{top} L{},{} A{rx},{ry} 0 0 1 {x},{} Z",
                    x + w,
                    x + w,
                    y + h - ry,
                    y + h - ry
                );
                self.path(&body, Some(fill), Some(stroke));
                let cap = format!("M{x},{top} A{rx},{ry} 0 0 0 {},{top}", x + w);
                let lid_stroke = Stroke {
                    colour: lid.unwrap_or(stroke.colour),
                    ..*stroke
                };
                self.path(&cap, None, Some(&lid_stroke));
            }
            other => self.shape_outline(&other, fill, stroke),
        }
    }

    /// The phase-2 outlines (P2-11), as `outline`.
    fn shape_outline(&mut self, outline: &Outline, fill: Colour, stroke: &Stroke) {
        match *outline {
            Outline::Ellipse { rect, .. } => self.ellipse(rect, Some(fill), Some(stroke)),
            Outline::Hexagon { rect, inset_px } => {
                let (l, r, t, b, cy) = (rect.min.x, rect.max.x, rect.min.y, rect.max.y, rect.centre().y);
                let pts = [
                    Point::new(l + inset_px, t),
                    Point::new(r - inset_px, t),
                    Point::new(r, cy),
                    Point::new(r - inset_px, b),
                    Point::new(l + inset_px, b),
                    Point::new(l, cy),
                ];
                self.path(&path_data(&pts, true), Some(fill), Some(stroke));
            }
            Outline::Diamond { rect } => {
                let c = rect.centre();
                let pts = [
                    Point::new(c.x, rect.min.y),
                    Point::new(rect.max.x, c.y),
                    Point::new(c.x, rect.max.y),
                    Point::new(rect.min.x, c.y),
                ];
                self.path(&path_data(&pts, true), Some(fill), Some(stroke));
            }
            Outline::Bucket { rect, cap_ry_px } => {
                // DECISION (scene): the bucket narrows by a tenth of its
                // width per side towards the bottom.
                let (left, right, bottom) = (rect.min.x, rect.max.x, rect.max.y);
                let (rx, ry, narrow) = (rect.width() / 2, cap_ry_px, rect.width() / 10);
                let top = rect.min.y + ry;
                let body = format!(
                    "M{left},{top} A{rx},{ry} 0 0 1 {right},{top} L{},{bottom} L{},{bottom} Z",
                    right - narrow,
                    left + narrow,
                );
                self.path(&body, Some(fill), Some(stroke));
                let cap = format!("M{left},{top} A{rx},{ry} 0 0 0 {right},{top}");
                self.path(&cap, None, Some(stroke));
            }
            Outline::Pipe { rect, cap_rx_px } => {
                let (x, y, w, h) = (rect.min.x, rect.min.y, rect.width(), rect.height());
                let (rx, ry) = (cap_rx_px, h / 2);
                let (left, right) = (x + rx, x + w - rx);
                let body = format!(
                    "M{left},{y} L{right},{y} A{rx},{ry} 0 0 1 {right},{} L{left},{} A{rx},{ry} 0 0 1 {left},{y} Z",
                    y + h,
                    y + h
                );
                self.path(&body, Some(fill), Some(stroke));
                let cap = format!("M{right},{y} A{rx},{ry} 0 0 0 {right},{}", y + h);
                self.path(&cap, None, Some(stroke));
            }
            Outline::Robot {
                head,
                body,
                radius_px,
            } => {
                self.rect(head, radius_px, Some(fill), Some(stroke));
                self.rect(body, radius_px, Some(fill), Some(stroke));
            }
            Outline::Folder { rect, tab } => {
                self.rect(tab, 0, Some(fill), Some(stroke));
                self.rect(rect, 0, Some(fill), Some(stroke));
            }
            Outline::Window { rect, bar_px, kind } => {
                self.rect(rect, 0, Some(fill), Some(stroke));
                for d in window_marks(rect, bar_px, kind) {
                    self.path(&d, None, Some(stroke));
                }
            }
            Outline::MobileDevice {
                rect,
                portrait,
                inset_px,
            } => {
                self.rect(rect, inset_px, Some(fill), Some(stroke));
                self.rect(crate::scene::mobile_screen(rect, portrait, inset_px), 0, None, Some(stroke));
            }
            Outline::Component { rect, tabs } => {
                self.rect(rect, 0, Some(fill), Some(stroke));
                for tab in tabs {
                    self.rect(tab, 0, Some(fill), Some(stroke));
                }
            }
            Outline::Rect { .. } | Outline::Person { .. } | Outline::Cylinder { .. } => {}
        }
    }

    /// An icon as `<image href="data:<mime>;base64,...">` (P2-409): the
    /// only `href` the document carries, and no external reference
    /// (EXP-1).
    // DECISION: `preserveAspectRatio` is left at its default (centre,
    // meet), so a non-square image keeps its shape inside the square.
    fn image(&mut self, icon: &SceneIcon) {
        self.picture(icon.rect, &icon.asset);
    }

    /// `asset` as `<image href="data:<mime>;base64,...">` over `rect`: an
    /// icon (P2-409) or an image view's picture (P2-320, P2-340).
    fn picture(&mut self, rect: Rect, asset: &crate::model::Asset) {
        let mime = match asset.mime {
            Mime::Png => "image/png",
            Mime::Jpeg => "image/jpeg",
            _ => "image/svg+xml",
        };
        self.raw("<image");
        self.attr_num("x", rect.min.x);
        self.attr_num("y", rect.min.y);
        self.attr_num("width", rect.width());
        self.attr_num("height", rect.height());
        let _ = write!(self.out, " href=\"data:{mime};base64,");
        base64_into(&mut self.out, &asset.bytes);
        self.raw("\"/>");
    }

    /// One `<text>` per line, baselines `line_px` apart, never re-wrapped
    /// (CORE-30).
    fn text(&mut self, block: &TextBlock) {
        self.text_tech(block, None);
    }

    /// `text`, the last line in `tech` when given and bracketed (a
    /// label's technology, redesign 2026-11-19).
    fn text_tech(&mut self, block: &TextBlock, tech: Option<Colour>) {
        let anchor = match block.align {
            Align::Start => "start",
            Align::Middle => "middle",
        };
        let mut y = block.at.y;
        let last = block.lines.len().saturating_sub(1);
        for (i, line) in block.lines.iter().enumerate() {
            let colour = match tech {
                Some(t) if i == last && i > 0 && line.starts_with('[') && line.ends_with(']') => t,
                _ => block.colour,
            };
            self.raw("<text");
            self.attr_num("x", block.at.x);
            self.attr_num("y", y);
            let _ = write!(self.out, " font-family=\"{FONT_FAMILY}, sans-serif\"");
            self.attr_num("font-size", block.size_px);
            if block.weight == Weight::Bold {
                self.raw(" font-weight=\"bold\"");
            }
            self.attr_colour("fill", colour);
            let _ = write!(self.out, " text-anchor=\"{anchor}\">");
            escape_into(&mut self.out, line);
            self.raw("</text>");
            y += block.line_px;
        }
    }
}

/// The decoration strokes of a window-like outline (P2-11), as path data:
/// the title bar's lower edge, then per kind an address field
/// (`WebBrowser`), three buttons (`Window`) or a prompt (`Terminal`,
/// `Shell`; a shell has no title bar edge, a terminal adds a cursor after
/// its prompt).
// DECISION: decorations are strokes only, in the outline's colour, so
// they read on any fill; the canvas draws the same paths.
pub(crate) fn window_marks(rect: Rect, bar_px: i32, kind: WindowKind) -> Vec<String> {
    let (left, right) = (rect.min.x, rect.max.x);
    let bar = rect.min.y + bar_px;
    let mut out = Vec::new();
    if kind != WindowKind::Shell {
        out.push(format!("M{left},{bar} L{right},{bar}"));
    }
    let mid = rect.min.y + bar_px / 2;
    let unit = (bar_px / 5).max(1);
    match kind {
        WindowKind::WebBrowser => {
            let (from, to) = (left + 4 * unit + bar_px, right - 2 * unit);
            let (top, bottom) = (mid - unit, mid + unit);
            out.push(format!("M{from},{top} L{to},{top} L{to},{bottom} L{from},{bottom} Z"));
        }
        WindowKind::Window => {
            for i in 1..=3 {
                let cx = left + i * 3 * unit;
                out.push(format!(
                    "M{},{mid} A{unit},{unit} 0 1 0 {},{mid} A{unit},{unit} 0 1 0 {},{mid}",
                    cx - unit,
                    cx + unit,
                    cx - unit
                ));
            }
        }
        _ => {
            let (px, py) = (left + bar_px / 2, bar + bar_px / 2);
            out.push(format!("M{px},{py} L{},{} L{px},{}", px + unit * 2, py + unit * 2, py + unit * 4));
            if kind == WindowKind::Terminal {
                // DECISION: the terminal's cursor is an underscore right of
                // the prompt, so Terminal and Shell differ beyond the bar.
                let (from, y) = (px + unit * 3, py + unit * 4);
                out.push(format!("M{from},{y} L{},{y}", from + unit * 3));
            }
        }
    }
    out
}

// ------------------------------------------------ edge drawing geometry
//
// redesign 2026-11-19 (design-system Edge). The editor's canvas.rs
// repeats these numbers and steps; keep the two in step (EDIT-12).

/// Stroke width a renderer draws a line `width_px` wide at: the scene's
/// default 2px line is drawn `edge-width` 1.5px.
// DECISION: the scene keeps integer widths (pub `Stroke::width_px`), so
// the renderers map exactly the default 2px to 1.5px; any other written
// thickness draws as written.
pub(crate) fn edge_width(width_px: i32) -> f64 {
    if width_px == 2 { 1.5 } else { f64::from(width_px) }
}

/// Corner radius of a turn (`edge-corner`), at most half the shorter leg.
const CORNER_PX: f64 = 10.0;
/// Hop radius where a line jumps another (P2-422).
const HOP_RADIUS_PX: f64 = 6.0;
/// Arrowhead length, half width and notch at the default width
/// (`edge-head`: 9 long, 7 wide, 2.5 notch); they scale with thickness/2
/// as the scene's arrowhead does (P2-418).
const HEAD_LEN_PX: f64 = 9.0;
const HEAD_HALF_PX: f64 = 3.5;
const HEAD_NOTCH_PX: f64 = 2.5;
/// How far into the head past the notch the line runs, so no seam shows.
const HEAD_OVERLAP_PX: f64 = 0.5;
/// Port dot radius (`edge-port`).
const PORT_RADIUS_PX: f64 = 2.5;
/// Paper ring between a person's head and body.
const HEAD_RING_PX: f64 = 2.5;
/// Label plate padding and corner radius.
const PLATE_PAD_X_PX: i32 = 6;
const PLATE_PAD_Y_PX: i32 = 4;
const PLATE_RADIUS_PX: i32 = 3;
/// A cylinder lid's line: the text colour at this opacity over the fill.
const LID_OPACITY: u8 = 45;

/// The scale of a line's head: thickness/2, never below 1 (P2-418).
fn head_scale(width_px: i32) -> f64 {
    f64::from(width_px.max(2)) / 2.0
}

/// How far the run ending at an arrow's tip stops short of it: in the
/// notch, plus `HEAD_OVERLAP_PX`.
fn head_trim(width_px: i32) -> f64 {
    (HEAD_LEN_PX - HEAD_NOTCH_PX - HEAD_OVERLAP_PX) * head_scale(width_px)
}

/// The notched head on the scene's arrow `t` (tip, back corners): tip,
/// back corner, notch, back corner.
fn notched_head(t: &[Point; 3], width_px: i32) -> [(f64, f64); 4] {
    let f = |p: Point| (f64::from(p.x), f64::from(p.y));
    let (tip, b1, b2) = (f(t[0]), f(t[1]), f(t[2]));
    let back = (f64::midpoint(b1.0, b2.0), f64::midpoint(b1.1, b2.1));
    let (dx, dy) = unit(back, tip);
    let (nx, ny) = (-dy, dx);
    let k = head_scale(width_px);
    let (len, half, notch) = (HEAD_LEN_PX * k, HEAD_HALF_PX * k, (HEAD_LEN_PX - HEAD_NOTCH_PX) * k);
    [
        tip,
        (tip.0 - dx * len + nx * half, tip.1 - dy * len + ny * half),
        (tip.0 - dx * notch, tip.1 - dy * notch),
        (tip.0 - dx * len - nx * half, tip.1 - dy * len - ny * half),
    ]
}

/// The unit vector from `a` to `b`, zero when they meet.
fn unit(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    if len < 1e-9 { (0.0, 0.0) } else { (dx / len, dy / len) }
}

/// The plate behind a label's text: its bounds plus the padding.
fn label_plate(text: &TextBlock) -> Rect {
    let b = text.bounds;
    Rect::from_corners(
        Point::new(b.min.x - PLATE_PAD_X_PX, b.min.y - PLATE_PAD_Y_PX),
        Point::new(b.max.x + PLATE_PAD_X_PX, b.max.y + PLATE_PAD_Y_PX),
    )
}

/// One step of an edge's path, scene px.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Seg {
    /// Start here.
    Move((f64, f64)),
    /// Straight to here.
    Line((f64, f64)),
    /// A rounded corner: quadratic through the control (the vertex) to
    /// the end.
    Quad((f64, f64), (f64, f64)),
    /// A hop over a crossing (P2-422): half circle, sweep 1, to here.
    Hop((f64, f64)),
}

/// The path of a run: its last point pulled back `trim` px, each turn
/// rounded by `CORNER_PX` (at most half the shorter leg), a hop at each
/// of `jumps` on a segment.
fn edge_path(points: &[Point], jumps: &[Point], trim: f64) -> Vec<Seg> {
    let mut pts: Vec<(f64, f64)> = points.iter().map(|p| (f64::from(p.x), f64::from(p.y))).collect();
    let n = pts.len();
    if n == 0 {
        return Vec::new();
    }
    if n >= 2 && trim > 0.0 {
        let (a, b) = (pts[n - 2], pts[n - 1]);
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        let back = trim.min(len);
        let (dx, dy) = unit(a, b);
        pts[n - 1] = (b.0 - dx * back, b.1 - dy * back);
    }
    let dist = |a: (f64, f64), b: (f64, f64)| (b.0 - a.0).hypot(b.1 - a.1);
    let radius: Vec<f64> = (0..n)
        .map(|i| {
            if i == 0 || i + 1 == n {
                return 0.0;
            }
            let (u, v) = (unit(pts[i - 1], pts[i]), unit(pts[i], pts[i + 1]));
            if (u.0 * v.1 - u.1 * v.0).abs() < 1e-9 {
                return 0.0;
            }
            CORNER_PX.min(dist(pts[i - 1], pts[i]) / 2.0).min(dist(pts[i], pts[i + 1]) / 2.0)
        })
        .collect();
    let mut out = vec![Seg::Move(pts[0])];
    for i in 1..n {
        let (a, b) = (pts[i - 1], pts[i]);
        let (dx, dy) = unit(a, b);
        let mut on: Vec<Point> = jumps
            .iter()
            .copied()
            .filter(|j| on_segment(points[i - 1], points[i], *j))
            .collect();
        on.sort_by_key(|j| (j.x - points[i - 1].x).abs() + (j.y - points[i - 1].y).abs());
        for j in on {
            let (jx, jy) = (f64::from(j.x), f64::from(j.y));
            out.push(Seg::Line((jx - dx * HOP_RADIUS_PX, jy - dy * HOP_RADIUS_PX)));
            out.push(Seg::Hop((jx + dx * HOP_RADIUS_PX, jy + dy * HOP_RADIUS_PX)));
        }
        let r = radius[i];
        out.push(Seg::Line((b.0 - dx * r, b.1 - dy * r)));
        if r > 0.0 {
            let (ex, ey) = unit(b, pts[i + 1]);
            out.push(Seg::Quad(b, (b.0 + ex * r, b.1 + ey * r)));
        }
    }
    out
}

/// `segs` as SVG path data.
fn svg_path(segs: &[Seg]) -> String {
    let mut d = String::new();
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            d.push(' ');
        }
        let _ = match *seg {
            Seg::Move((x, y)) => write!(d, "M{} {}", num(x), num(y)),
            Seg::Line((x, y)) => write!(d, "L{} {}", num(x), num(y)),
            Seg::Quad((cx, cy), (x, y)) => write!(d, "Q{} {} {} {}", num(cx), num(cy), num(x), num(y)),
            Seg::Hop((x, y)) => write!(d, "A{r} {r} 0 0 1 {} {}", num(x), num(y), r = num(HOP_RADIUS_PX)),
        };
    }
    d
}

/// `v` rounded to one decimal, written without a trailing `.0`; the
/// same bytes on every machine (CORE-25: fixed-precision decimals).
fn num(v: f64) -> String {
    #[expect(clippy::cast_possible_truncation, reason = "scene px are far inside i64")]
    let tenths = (v * 10.0).round() as i64;
    let sign = if tenths < 0 { "-" } else { "" };
    let a = tenths.unsigned_abs();
    if a.is_multiple_of(10) {
        format!("{sign}{}", a / 10)
    } else {
        format!("{sign}{}.{}", a / 10, a % 10)
    }
}

/// `top` at `opacity`/100 over `under`, each channel rounded to nearest.
fn over(top: Colour, opacity: u8, under: Colour) -> Colour {
    let a = u16::from(opacity.min(100));
    let mix = |t: u8, u: u8| u8::try_from((u16::from(t) * a + u16::from(u) * (100 - a) + 50) / 100).unwrap_or(u8::MAX);
    Colour {
        r: mix(top.r, under.r),
        g: mix(top.g, under.g),
        b: mix(top.b, under.b),
    }
}

/// True when `p` lies on the segment `a`..`b`, ends excluded.
fn on_segment(a: Point, b: Point, p: Point) -> bool {
    let cross = i64::from(b.x - a.x) * i64::from(p.y - a.y) - i64::from(b.y - a.y) * i64::from(p.x - a.x);
    let within = |v: i32, l: i32, h: i32| v >= l.min(h) && v <= l.max(h);
    cross == 0 && within(p.x, a.x, b.x) && within(p.y, a.y, b.y) && p != a && p != b
}

/// Appends `bytes` as standard base64 with `=` padding (RFC 4648).
fn base64_into(out: &mut String, bytes: &[u8]) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &b)| acc | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
}

/// `M x,y L x,y ...`, closed with `Z` when `closed`.
fn path_data(points: &[Point], closed: bool) -> String {
    let mut d = String::new();
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            d.push(' ');
        }
        let _ = write!(d, "{}{},{}", if i == 0 { 'M' } else { 'L' }, p.x, p.y);
    }
    if closed {
        d.push_str(" Z");
    }
    d
}

/// Appends `s` XML-escaped for text and double- or single-quoted
/// attribute values; characters XML 1.0 cannot hold become U+FFFD.
fn escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            '\u{0}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}' => out.push('\u{fffd}'),
            _ => out.push(c),
        }
    }
}

/// The scene's SVG (`svg`) rasterized to PNG bytes at `width_px` wide,
/// the height scaled to keep `scene.bounds`' aspect ratio, larger or
/// smaller than the scene alike (EXP-2). The height is computed in `u64`
/// as `(h * width_px + w / 2) / w` with `w`, `h` = `scene.bounds`'
/// width and height in px (halves round up), and is at least 1.
///
/// Texts are rendered with the bundled faces `scene::FONT_REGULAR` and
/// `scene::FONT_BOLD` only (CORE-30), never a system font. Faces are
/// found by family name `scene::FONT_FAMILY`, and the regular face is
/// also registered as the `sans-serif` family, so the SVG's fallback
/// resolves to Inter too. Same scene and width give the same bytes on
/// one machine and build (only
/// the SVG is byte-stable across machines, CORE-25). The CLI and the
/// editor pass twice `scene.bounds` width unless asked otherwise (CLI-4,
/// EDIT-7). Serves EXP-2, CLI-4, EDIT-7.
///
/// # Errors
///
/// `PngError::ZeroWidth` when `width_px` is 0; `PngError::NoFont` when
/// the bundled faces cannot be loaded (never expected; there is never a
/// system-font fallback); `PngError::TooLarge` when the height
/// does not fit `u32` (reported as `u32::MAX`) or the image cannot be
/// allocated; `PngError::Render` when `scene.bounds` has zero width or
/// height, or rasterizing or encoding fails. Checked in this order:
/// width, bounds, size, fonts.
#[cfg(feature = "png")]
pub fn png(scene: &Scene, width_px: u32) -> Result<Vec<u8>, PngError> {
    use resvg::{tiny_skia, usvg};

    if width_px == 0 {
        return Err(PngError::ZeroWidth);
    }
    let (Ok(w), Ok(h)) = (
        u64::try_from(scene.bounds.width()),
        u64::try_from(scene.bounds.height()),
    ) else {
        return Err(PngError::Render(empty_bounds(scene)));
    };
    if w == 0 || h == 0 {
        return Err(PngError::Render(empty_bounds(scene)));
    }
    let height = ((h * u64::from(width_px) + w / 2) / w).max(1);
    let Ok(height_px) = u32::try_from(height) else {
        return Err(PngError::TooLarge {
            width_px,
            height_px: u32::MAX,
        });
    };
    let mut pixmap = tiny_skia::Pixmap::new(width_px, height_px).ok_or(PngError::TooLarge {
        width_px,
        height_px,
    })?;

    // DECISION: a fresh font database per call (see the header): parsing
    // two faces is cheap next to rasterizing, and no global state exists.
    let mut db = usvg::fontdb::Database::new();
    db.load_font_data(FONT_REGULAR.to_vec());
    db.load_font_data(FONT_BOLD.to_vec());
    if db.len() != 2
        || db
            .faces()
            .any(|f| !f.families.iter().any(|(n, _)| n == FONT_FAMILY))
    {
        return Err(PngError::NoFont);
    }
    db.set_sans_serif_family(FONT_FAMILY);
    let opt = usvg::Options {
        font_family: FONT_FAMILY.to_owned(),
        fontdb: std::sync::Arc::new(db),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&svg(scene), &opt)
        .map_err(|e| PngError::Render(format!("the SVG did not parse ({e})")))?;
    let size = tree.size();
    // DECISION: x and y scale separately so the drawing fills the
    // rounded height exactly; the two differ by under one pixel's worth.
    #[allow(clippy::cast_precision_loss)]
    let transform = tiny_skia::Transform::from_scale(
        width_px as f32 / size.width(),
        height_px as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap
        .encode_png()
        .map_err(|e| PngError::Render(format!("PNG encoding failed ({e})")))
}

/// `asset` (an icon or image file, `model::Asset`) rasterized to
/// `width_px` x `height_px` straight (not premultiplied) RGBA8 pixels,
/// row-major from the top left, `width_px * height_px * 4` bytes: the
/// image the SVG export embeds as `<image>` drawn into a box of that size,
/// centred and scaled to fit with its aspect ratio kept (the SVG default),
/// transparent around it. The editor paints icons and image primitives
/// with it as a texture (P2-409, P2-436). Approved pub addition
/// (2026-11-17). Serves P2-409, EDIT-12.
///
/// Returns `None` when either size is 0, the image cannot be allocated,
/// or the bytes do not decode as the asset's type.
#[cfg(feature = "png")]
#[must_use]
pub fn rasterize_asset(asset: &crate::model::Asset, width_px: u32, height_px: u32) -> Option<Vec<u8>> {
    use resvg::{tiny_skia, usvg};

    if width_px == 0 || height_px == 0 {
        return None;
    }
    let mime = match asset.mime {
        Mime::Png => "image/png",
        Mime::Jpeg => "image/jpeg",
        _ => "image/svg+xml",
    };
    // DECISION: the asset goes through the same `<image>` the SVG export
    // writes, so the canvas, the SVG and the PNG decode and fit it alike.
    let mut doc = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width_px}\" height=\"{height_px}\"><image width=\"{width_px}\" height=\"{height_px}\" href=\"data:{mime};base64,"
    );
    base64_into(&mut doc, &asset.bytes);
    doc.push_str("\"/></svg>");
    let tree = usvg::Tree::from_str(&doc, &usvg::Options::default()).ok()?;
    // An image usvg cannot decode is left out of the tree.
    if !tree.root().has_children() {
        return None;
    }
    let mut pixmap = tiny_skia::Pixmap::new(width_px, height_px)?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    Some(
        pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect(),
    )
}

#[cfg(feature = "png")]
fn empty_bounds(scene: &Scene) -> String {
    format!(
        "the scene is {} x {} px, so there is nothing to draw",
        scene.bounds.width(),
        scene.bounds.height()
    )
}

/// Why `png` produced no image. `#[non_exhaustive]` for later causes.
/// Serves EXP-2, CLI-2 (the CLI exits 2 on it).
#[cfg(feature = "png")]
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PngError {
    /// The requested width was 0.
    ZeroWidth,
    /// The bundled font bytes are not a font resvg can load.
    NoFont,
    /// The image of this size cannot be allocated.
    TooLarge {
        /// The requested width in px.
        width_px: u32,
        /// The height in px that keeps the scene's aspect ratio.
        height_px: u32,
    },
    /// Rasterizing or PNG encoding failed; the text says what failed.
    Render(String),
}

#[cfg(feature = "png")]
impl std::fmt::Display for PngError {
    /// One sentence saying what failed and what to change (SRS 5.1).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroWidth => f.write_str("PNG width is 0 px; ask for a width of at least 1 px."),
            Self::NoFont => f.write_str(
                "The bundled Inter font could not be loaded, so no PNG was drawn; reinstall or rebuild c4rizer.",
            ),
            Self::TooLarge {
                width_px,
                height_px,
            } => write!(
                f,
                "A {width_px} x {height_px} px PNG is too large to allocate; ask for a smaller width."
            ),
            Self::Render(why) => write!(f, "PNG rendering failed: {why}; export the SVG instead."),
        }
    }
}

#[cfg(feature = "png")]
impl std::error::Error for PngError {}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicBool;

    use crate::geom::{Point, Rect};
    use crate::label::place_labels;
    use crate::model::parse;
    use crate::place::{Keep, place};
    use crate::route::{RouteConfig, route};
    use crate::scene::{FONT_FAMILY, Outline, Swatch, TextBlock, Tip, build};
    use crate::view::plan;

    /// A small shop written with roles only: a person, a system with
    /// three containers (one a cylinder), an external system. Names carry
    /// characters XML must escape.
    const SHOP: &str = r#"workspace {
  model {
    operator = person "Operator" "Runs the shop day to day."
    shop = softwareSystem "Shop" "Sells goods online." {
      web = container "Web <app> & 'co'" "Serves the \"shop\" pages and takes orders." "Rust"
      db = container "Database" "Stores orders." "SQLite" "Database"
      worker = container "Worker"
    }
    bank = softwareSystem "Bank" "Takes card payments." "External"
    operator -> web "Manages orders in" "HTTPS"
    web -> db "Reads & writes <orders>" "SQL"
    worker -> db "Reads open orders from" "SQL"
    web -> bank "Charges cards through" "HTTPS"
  }
  views {
    systemContext shop "c1" "Shop context" {
      include *
    }
    container shop "c2" "Shop containers" {
      subtitle "The running parts of the shop."
      include *
    }
    styles {
      element "Person" {
        shape Person
        background #08427b
        color #ffffff
      }
      element "Container" {
        background #438dd5
        color #ffffff
      }
      element "Database" {
        shape Cylinder
      }
      element "External" {
        background #999999
      }
    }
  }
}
"#;

    /// The scene of view `key` of `src`: auto-placed, routed, labelled.
    fn scene_of(src: &str, key: &str) -> Scene {
        let model = match parse(src) {
            Ok(p) => p.model,
            Err(e) => panic!("{e}"),
        };
        let plan = plan(&model, model.view(key).expect("view"));
        let centres = place(&plan, &Keep::default());
        let routes = route(
            &plan,
            &centres,
            &RouteConfig::DEFAULT,
            &AtomicBool::new(false),
        )
        .expect("not stopped");
        let labels = place_labels(&plan, &routes);
        build(&plan, &routes, &labels)
    }

    /// One parsed XML element: name, attributes in order, children, and
    /// its own text content (unescaped).
    #[derive(Debug, Default)]
    struct El {
        name: String,
        attrs: Vec<(String, String)>,
        children: Vec<El>,
        text: String,
    }

    impl El {
        fn attr(&self, name: &str) -> Option<&str> {
            self.attrs
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        }

        /// Every element below this one, depth first, in document order.
        fn walk<'a>(&'a self, out: &mut Vec<&'a El>) {
            for c in &self.children {
                out.push(c);
                c.walk(out);
            }
        }

        fn all(&self) -> Vec<&El> {
            let mut out = vec![self];
            self.walk(&mut out);
            out
        }
    }

    fn unescape(s: &str) -> Result<String, String> {
        let mut out = String::new();
        let mut rest = s;
        while let Some(i) = rest.find('&') {
            out.push_str(&rest[..i]);
            rest = &rest[i..];
            let end = rest.find(';').ok_or("unterminated entity")?;
            out.push(match &rest[..=end] {
                "&lt;" => '<',
                "&gt;" => '>',
                "&amp;" => '&',
                "&quot;" => '"',
                "&apos;" => '\'',
                other => return Err(format!("unknown entity {other}")),
            });
            rest = &rest[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    /// A strict little parser for the subset of XML `svg` writes: one
    /// root, balanced tags, double-quoted attributes, only the five
    /// predefined entities, no raw `<` in text or values. `Err` = the
    /// document would not parse.
    fn parse_xml(doc: &str) -> Result<El, String> {
        let mut stack: Vec<El> = vec![El::default()];
        let mut rest = doc;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix("</") {
                let end = after.find('>').ok_or("unterminated close tag")?;
                let name = &after[..end];
                let el = stack.pop().ok_or("close without open")?;
                if el.name != name {
                    return Err(format!("</{name}> closes <{}>", el.name));
                }
                stack.last_mut().ok_or("unbalanced")?.children.push(el);
                rest = &after[end + 1..];
            } else if let Some(after) = rest.strip_prefix('<') {
                let name_end = after
                    .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                    .ok_or("unterminated tag")?;
                let mut el = El {
                    name: after[..name_end].to_owned(),
                    ..El::default()
                };
                if el.name.is_empty() || el.name.starts_with(['?', '!']) {
                    return Err(format!("unexpected markup <{}", el.name));
                }
                let mut tail = &after[name_end..];
                loop {
                    tail = tail.trim_start();
                    if let Some(t) = tail.strip_prefix("/>") {
                        tail = t;
                        stack.last_mut().ok_or("unbalanced")?.children.push(el);
                        break;
                    }
                    if let Some(t) = tail.strip_prefix('>') {
                        tail = t;
                        stack.push(el);
                        break;
                    }
                    let eq = tail.find('=').ok_or("attribute without value")?;
                    let name = tail[..eq].to_owned();
                    if name.is_empty() || name.contains(char::is_whitespace) {
                        return Err(format!("bad attribute name {name:?}"));
                    }
                    let v = tail[eq + 1..]
                        .strip_prefix('"')
                        .ok_or("attribute value not double-quoted")?;
                    let close = v.find('"').ok_or("unterminated attribute value")?;
                    let raw = &v[..close];
                    if raw.contains('<') {
                        return Err("raw < in attribute value".into());
                    }
                    el.attrs.push((name, unescape(raw)?));
                    tail = &v[close + 1..];
                }
                rest = tail;
            } else {
                let end = rest.find('<').unwrap_or(rest.len());
                let text = unescape(&rest[..end])?;
                if stack.len() == 1 && !text.trim().is_empty() {
                    return Err("text outside the root".into());
                }
                stack.last_mut().ok_or("unbalanced")?.text.push_str(&text);
                rest = &rest[end..];
            }
        }
        let mut doc_node = stack.pop().ok_or("unbalanced")?;
        if !stack.is_empty() {
            return Err(format!("<{}> never closed", doc_node.name));
        }
        if doc_node.children.len() != 1 {
            return Err("not exactly one root".into());
        }
        Ok(doc_node.children.remove(0))
    }

    fn root(svg_text: &str) -> El {
        parse_xml(svg_text).unwrap_or_else(|e| panic!("{e}\n{svg_text}"))
    }

    /// Every text line of `scene`, in paint order.
    fn painted_lines(scene: &Scene) -> Vec<String> {
        let mut blocks: Vec<&TextBlock> = Vec::new();
        blocks.extend(scene.boundaries.iter().map(|b| &b.tag));
        for b in &scene.boxes {
            blocks.push(&b.name);
            blocks.push(&b.type_line);
            blocks.extend(b.description.iter());
        }
        blocks.extend(scene.labels.iter().map(|l| &l.text));
        blocks.extend(scene.title.iter());
        blocks.extend(scene.subtitle.iter());
        blocks.extend(scene.legend.entries.iter().map(|e| &e.text));
        blocks
            .iter()
            .flat_map(|t| t.lines.iter().cloned())
            .collect()
    }

    fn texts(root: &El) -> Vec<&El> {
        root.all()
            .into_iter()
            .filter(|e| e.name == "text")
            .collect()
    }

    // One test per requirement, plus the Review Focus extras marked so.
    #[test]
    fn exp_1_svg_no_script_no_external_reference() {
        for key in ["c1", "c2"] {
            let out = svg(&scene_of(SHOP, key));
            let root = root(&out);
            assert_eq!(root.name, "svg");
            assert_eq!(root.attr("xmlns"), Some("http://www.w3.org/2000/svg"));
            for el in root.all() {
                assert!(
                    ["svg", "g", "rect", "path", "circle", "text"].contains(&el.name.as_str()),
                    "unexpected element <{}>",
                    el.name
                );
                for (name, value) in &el.attrs {
                    let lower = name.to_ascii_lowercase();
                    assert!(!lower.starts_with("on"), "event attribute {name}");
                    assert!(!lower.contains("href"), "href attribute {name}");
                    assert!(!lower.starts_with("xlink"), "xlink attribute {name}");
                    assert!(!value.contains("url("), "url() in {name}");
                }
            }
            for banned in [
                "<script",
                "<image",
                "<foreignObject",
                "<use",
                "<style",
                "url(",
                "@import",
                "@font-face",
                "xlink:",
                "href",
                "data:",
            ] {
                assert!(!out.contains(banned), "{banned} in {key}");
            }
            assert_eq!(out.matches("http").count(), 1, "only the xmlns URI");
            assert_eq!(out.matches("://").count(), 1, "only the xmlns URI");
        }
    }

    /// EXP-1 with icons (P2-409): the only `href` is an `<image>`'s
    /// base64 `data:` URI, and the base64 round-trips.
    #[test]
    fn exp_1_icon_only_data_uri() {
        let src = SHOP.replace(
            "        shape Cylinder\n",
            "        shape Cylinder\n        icon data:image/png;base64,iVBORw0KGgpyZXN0\n",
        );
        let out = svg(&scene_of(&src, "c2"));
        let root = root(&out);
        let images: Vec<&El> = root.all().into_iter().filter(|e| e.name == "image").collect();
        assert_eq!(images.len(), 1, "{out}");
        assert_eq!(images[0].attr("href"), Some("data:image/png;base64,iVBORw0KGgpyZXN0"));
        for el in root.all() {
            for (name, value) in &el.attrs {
                if name.to_ascii_lowercase().contains("href") {
                    assert_eq!(el.name, "image");
                    assert!(value.starts_with("data:image/png;base64,"));
                }
            }
        }
        for banned in ["<script", "<foreignObject", "<use", "url(", "xlink:", "@font-face"] {
            assert!(!out.contains(banned), "{banned}");
        }
        assert_eq!(out.matches("://").count(), 1, "only the xmlns URI");
        let mut b64 = String::new();
        base64_into(&mut b64, b"ab");
        assert_eq!(b64, "YWI=");
        b64.clear();
        base64_into(&mut b64, b"abc");
        assert_eq!(b64, "YWJj");
        b64.clear();
        base64_into(&mut b64, b"a");
        assert_eq!(b64, "YQ==");
    }

    #[test]
    fn core_30_svg_names_bundled_font_first() {
        let family = format!("{FONT_FAMILY}, sans-serif");
        assert_eq!(family, "Inter, sans-serif");
        let scene = scene_of(SHOP, "c2");
        let root = root(&svg(&scene));
        let texts = texts(&root);
        assert!(!texts.is_empty());
        for t in &texts {
            assert_eq!(t.attr("font-family"), Some(family.as_str()), "{t:?}");
        }
        // No other font-family anywhere that could override it.
        for el in root.all() {
            if el.name != "text" {
                assert_eq!(el.attr("font-family"), None, "<{}>", el.name);
            }
        }
        // One `<text>` per scene line, in paint order, never re-wrapped.
        let drawn: Vec<String> = texts.iter().map(|t| t.text.clone()).collect();
        assert_eq!(drawn, painted_lines(&scene));
        // Each line on its own baseline: at.y + i * line_px, and bold
        // where the scene says bold.
        let first = &scene.boxes[0];
        let name_texts: Vec<&&El> = texts
            .iter()
            .filter(|t| first.name.lines.contains(&t.text))
            .collect();
        for (i, t) in name_texts.iter().enumerate().take(first.name.lines.len()) {
            let line = i32::try_from(i).expect("small");
            assert_eq!(
                t.attr("y"),
                Some(
                    (first.name.at.y + line * first.name.line_px)
                        .to_string()
                        .as_str()
                )
            );
            assert_eq!(t.attr("font-weight"), Some("bold"));
            assert_eq!(t.attr("text-anchor"), Some("middle"));
        }
        let type_text = texts
            .iter()
            .find(|t| t.text == first.type_line.lines[0])
            .expect("type line drawn");
        assert_eq!(type_text.attr("font-weight"), None);
        assert_eq!(
            type_text.attr("font-size"),
            Some(first.type_line.size_px.to_string().as_str())
        );
    }

    #[test]
    fn exp_1_svg_escapes_text() {
        let mut scene = scene_of(SHOP, "c2");
        // Through the whole pipeline: names and labels from the file.
        let out = svg(&scene);
        assert!(
            out.contains("Web &lt;app&gt; &amp; &apos;co&apos;"),
            "{out}"
        );
        assert!(out.contains("Reads &amp; writes &lt;orders&gt;"), "{out}");
        assert!(!out.contains("<app>"));
        assert!(!out.contains("<orders>"));
        // Hand-made lines: quotes and characters XML cannot hold.
        scene.boxes[0].name.lines = vec![
            "a<b & \"c\" 'd' >".to_owned(),
            "x\u{1}y\u{FFFE}z\u{FFFF}\u{0}".to_owned(),
            "tab\tok".to_owned(),
        ];
        let out = svg(&scene);
        assert!(out.contains(">a&lt;b &amp; &quot;c&quot; &apos;d&apos; &gt;</text>"));
        assert!(out.contains(">x\u{FFFD}y\u{FFFD}z\u{FFFD}\u{FFFD}</text>"));
        assert!(out.contains(">tab\tok</text>"));
        for bad in ['\u{0}', '\u{1}', '\u{FFFE}', '\u{FFFF}'] {
            assert!(!out.contains(bad), "{bad:?} written raw");
        }
        let root = root(&out);
        let drawn: Vec<String> = texts(&root).iter().map(|t| t.text.clone()).collect();
        assert!(drawn.contains(&"a<b & \"c\" 'd' >".to_owned()));
        assert!(drawn.contains(&"x\u{FFFD}y\u{FFFD}z\u{FFFD}\u{FFFD}".to_owned()));
        #[cfg(feature = "png")]
        {
            let doc = resvg::usvg::roxmltree::Document::parse(&out);
            assert!(doc.is_ok(), "{doc:?}");
        }
    }

    #[test]
    fn core_25_svg_deterministic_bytes() {
        for key in ["c1", "c2"] {
            let a = svg(&scene_of(SHOP, key));
            let b = svg(&scene_of(SHOP, key));
            assert_eq!(a, b, "{key}");
            assert_eq!(svg(&scene_of(SHOP, key)), a, "{key} third run");
            // Fixed precision: every number in an attribute value is an
            // integer or has exactly one decimal (CORE-25).
            // redesign 2026-11-19: was integers only; the redesign's
            // 1.5px edges, notched heads and rounded corners need tenths.
            for el in root(&a).all() {
                for (name, value) in &el.attrs {
                    if name == "xmlns" {
                        continue;
                    }
                    let b = value.as_bytes();
                    for (i, w) in b.windows(2).enumerate() {
                        if w[0].is_ascii_digit() && w[1] == b'.' {
                            let after = &b[i + 2..];
                            let digits = after.iter().take_while(|c| c.is_ascii_digit()).count();
                            assert_eq!(digits, 1, "not one decimal in {name}=\"{value}\"");
                        }
                        assert!(
                            !(w[0] == b'.' && i == 0),
                            "number without integer part in {name}=\"{value}\""
                        );
                    }
                }
            }
        }
    }

    /// Box groups hold their outline shapes then one text per line; key
    /// groups their swatch shapes then their text.
    fn assert_box_and_key_groups(scene: &Scene, groups: &[&El]) {
        // Box groups: outline shapes then one text per line.
        let box_groups = groups.iter().filter(|g| g.attr("class") == Some("box"));
        for (b, g) in scene.boxes.iter().zip(box_groups) {
            let shapes = match b.outline {
                Outline::Rect { .. } => vec!["rect"],
                // redesign 2026-11-19: the paper ring, then the head.
                Outline::Person { .. } => vec!["rect", "circle", "circle"],
                Outline::Cylinder { .. } | Outline::Bucket { .. } | Outline::Pipe { .. } => {
                    vec!["path", "path"]
                }
                Outline::Ellipse { .. } => vec!["ellipse"],
                Outline::Hexagon { .. } | Outline::Diamond { .. } => vec!["path"],
                Outline::Robot { .. } | Outline::Folder { .. } | Outline::MobileDevice { .. } => {
                    vec!["rect", "rect"]
                }
                Outline::Window { kind, .. } => {
                    let marks = match kind {
                        crate::scene::WindowKind::Window => 4,
                        crate::scene::WindowKind::Terminal => 3,
                        crate::scene::WindowKind::Shell => 1,
                        _ => 2,
                    };
                    let mut v = vec!["rect"];
                    v.extend(std::iter::repeat_n("path", marks));
                    v
                }
                Outline::Component { .. } => vec!["rect", "rect", "rect"],
            };
            let lines = b.name.lines.len()
                + b.type_line.lines.len()
                + b.description.as_ref().map_or(0, |d| d.lines.len());
            let names: Vec<&str> = g.children.iter().map(|e| e.name.as_str()).collect();
            let mut want = shapes.clone();
            want.extend(std::iter::repeat_n("text", lines));
            assert_eq!(names, want);
        }
        assert!(
            scene
                .boxes
                .iter()
                .any(|b| matches!(b.outline, Outline::Person { .. }))
        );
        assert!(
            scene
                .boxes
                .iter()
                .any(|b| matches!(b.outline, Outline::Cylinder { .. }))
        );

        // Key groups: swatch shapes then text.
        let key_groups = groups.iter().filter(|g| g.attr("class") == Some("key"));
        for (e, g) in scene.legend.entries.iter().zip(key_groups) {
            // redesign 2026-11-19: a line sample has a port after its
            // head.
            let shapes = match e.swatch {
                Swatch::Box {
                    outline: Outline::Rect { .. },
                    ..
                } => 1,
                Swatch::Box { .. } => 2,
                Swatch::Line { .. } => 3,
            };
            assert_eq!(g.children.len(), shapes + e.text.lines.len());
        }
    }

    #[test]
    fn core_1_svg_paint_order_and_viewbox() {
        let mut scene = scene_of(SHOP, "c2");
        scene.bounds = Rect::from_corners(Point::new(-40, -30), scene.bounds.max);
        let (w, h) = (scene.bounds.width(), scene.bounds.height());
        let root = root(&svg(&scene));
        assert_eq!(
            root.attr("viewBox"),
            Some(format!("-40 -30 {w} {h}").as_str())
        );
        assert_eq!(root.attr("width"), Some(w.to_string().as_str()));
        assert_eq!(root.attr("height"), Some(h.to_string().as_str()));

        // First the background over the bounds.
        let bg = &root.children[0];
        assert_eq!(bg.name, "rect");
        assert_eq!(bg.attr("x"), Some("-40"));
        assert_eq!(bg.attr("y"), Some("-30"));
        assert_eq!(bg.attr("width"), Some(w.to_string().as_str()));
        assert_eq!(bg.attr("height"), Some(h.to_string().as_str()));
        assert_eq!(bg.attr("fill"), Some(scene.background.to_string().as_str()));

        // Then one group per item, in the scene's paint order.
        let groups: Vec<&El> = root.children[1..].iter().collect();
        assert!(groups.iter().all(|g| g.name == "g"));
        let mut want = Vec::new();
        want.extend(scene.boundaries.iter().map(|_| "boundary"));
        want.extend(scene.lines.iter().map(|_| "line"));
        want.extend(scene.boxes.iter().map(|_| "box"));
        want.extend(scene.labels.iter().map(|_| "label"));
        want.extend(scene.title.iter().map(|_| "title"));
        want.extend(scene.subtitle.iter().map(|_| "subtitle"));
        want.extend(scene.legend.entries.iter().map(|_| "key"));
        let got: Vec<&str> = groups
            .iter()
            .map(|g| g.attr("class").unwrap_or(""))
            .collect();
        assert_eq!(got, want);
        assert!(!scene.boundaries.is_empty() && !scene.labels.is_empty());

        // Each line group holds exactly its route, chevrons, tip and port.
        // redesign 2026-11-19: the route has rounded corners and stops
        // in the arrow's notch, and a port dot follows the tip.
        let line_groups = groups.iter().filter(|g| g.attr("class") == Some("line"));
        for (line, g) in scene.lines.iter().zip(line_groups) {
            let first = line.points[0];
            let d = g.children[0].attr("d").expect("route data");
            assert!(d.starts_with(&format!("M{} {}", first.x, first.y)), "{d}");
            assert_eq!(g.children[0].attr("fill"), Some("none"));
            let stroke = line.stroke.colour.to_string();
            assert_eq!(g.children[0].attr("stroke"), Some(stroke.as_str()));
            assert_eq!(g.children[0].attr("stroke-width"), Some("1.5"));
            assert_eq!(g.children.len(), 1 + line.chevrons.len() + 2);
            let tip = match line.tip {
                Tip::Arrow(_) => "path",
                Tip::Dot { .. } => "circle",
            };
            let n = g.children.len();
            assert_eq!(g.children[n - 2].name, tip);
            let port = &g.children[n - 1];
            assert_eq!(port.name, "circle");
            assert_eq!(port.attr("cx"), Some(first.x.to_string().as_str()));
            assert_eq!(port.attr("r"), Some("2.5"));
            assert_eq!(port.attr("fill"), Some(scene.background.to_string().as_str()));
        }

        assert_box_and_key_groups(&scene, &groups);

        // An empty scene draws its background and nothing else.
        let mut empty = scene.clone();
        empty.boundaries.clear();
        empty.lines.clear();
        empty.boxes.clear();
        empty.labels.clear();
        empty.title = None;
        empty.subtitle = None;
        empty.legend.entries.clear();
        let drawn = parse_xml(&svg(&empty)).expect("parses");
        assert_eq!(drawn.children.len(), 1);
    }

    /// PNG width and height from the IHDR chunk; asserts the signature.
    #[cfg(feature = "png")]
    fn png_size(bytes: &[u8]) -> (u32, u32) {
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");
        assert_eq!(&bytes[12..16], b"IHDR");
        let be =
            |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        (be(16), be(20))
    }

    /// The reference raster: `svg_text` parsed with a font database
    /// holding only the two bundled faces, drawn at `width` x `height`.
    #[cfg(feature = "png")]
    fn reference_raster(svg_text: &str, width: u32, height: u32) -> Vec<u8> {
        use resvg::{tiny_skia, usvg};
        let mut db = usvg::fontdb::Database::new();
        db.load_font_data(crate::scene::FONT_REGULAR.to_vec());
        db.load_font_data(crate::scene::FONT_BOLD.to_vec());
        db.set_sans_serif_family(FONT_FAMILY);
        assert_eq!(db.len(), 2, "only the bundled faces");
        let opt = usvg::Options {
            font_family: FONT_FAMILY.to_owned(),
            fontdb: std::sync::Arc::new(db),
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_str(svg_text, &opt).expect("own SVG parses");
        let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("allocates");
        let size = tree.size();
        #[allow(clippy::cast_precision_loss)]
        let transform = tiny_skia::Transform::from_scale(
            width as f32 / size.width(),
            height as f32 / size.height(),
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        pixmap.encode_png().expect("encodes")
    }

    /// The documented height rule, in u64.
    #[cfg(feature = "png")]
    fn rule_height(scene: &Scene, width_px: u32) -> u32 {
        let w = u64::try_from(scene.bounds.width()).expect("positive");
        let h = u64::try_from(scene.bounds.height()).expect("positive");
        u32::try_from(((h * u64::from(width_px) + w / 2) / w).max(1)).expect("fits")
    }

    #[cfg(feature = "png")]
    #[test]
    fn exp_2_png_matches_svg_raster() {
        let scene = scene_of(SHOP, "c2");
        let width = u32::try_from(scene.bounds.width() * 2).expect("positive");
        let height = rule_height(&scene, width);
        let bytes = png(&scene, width).expect("renders");
        assert_eq!(png_size(&bytes), (width, height));
        assert_eq!(bytes, reference_raster(&svg(&scene), width, height));
        // Same scene and width, same bytes.
        assert_eq!(png(&scene, width).expect("renders"), bytes);
    }

    #[cfg(feature = "png")]
    #[test]
    fn exp_2_png_scaled_keeps_aspect() {
        use resvg::tiny_skia::Pixmap;
        let scene = scene_of(SHOP, "c2");
        let w = scene.bounds.width();
        let cylinder = scene
            .boxes
            .iter()
            .find(|b| matches!(b.outline, Outline::Cylinder { .. }))
            .expect("a cylinder");
        let centre = cylinder.rect.centre();
        for width in [
            u32::try_from(w / 3).expect("positive"),
            u32::try_from(w * 3 / 2).expect("positive"),
        ] {
            let bytes = png(&scene, width).expect("renders");
            let height = rule_height(&scene, width);
            assert_eq!(png_size(&bytes), (width, height), "width {width}");
            let pixmap = Pixmap::decode_png(&bytes).expect("decodes");
            // The cylinder's centre lands where the scale puts it, in its
            // fill: the drawing is scaled, not cropped or stretched.
            let scale = f64::from(width) / f64::from(w);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let at = |v: i32, min: i32| (f64::from(v - min) * scale) as u32;
            // Sample 20px inside its left side at mid height, clear of
            // the centred text and of the outline.
            let px = pixmap
                .pixel(
                    at(cylinder.rect.min.x + 20, scene.bounds.min.x),
                    at(centre.y, scene.bounds.min.y),
                )
                .expect("inside")
                .demultiply();
            let fill = cylinder.fill;
            assert_eq!(
                (px.red(), px.green(), px.blue()),
                (fill.r, fill.g, fill.b),
                "width {width}"
            );
            // A corner shows the background.
            let corner = pixmap.pixel(0, 0).expect("inside").demultiply();
            let bg = scene.background;
            assert_eq!(
                (corner.red(), corner.green(), corner.blue()),
                (bg.r, bg.g, bg.b)
            );
        }
        // The height rule rounds halves up: 100 x 3 / 200 = 1.5 -> 2.
        let mut tie = scene.clone();
        tie.bounds = Rect::from_corners(Point::new(0, 0), Point::new(200, 100));
        assert_eq!(png_size(&png(&tie, 3).expect("renders")), (3, 2));
        // 100 x 1 / 200 = 0.5 rounds up to 1; 0.4 would round to 0 and
        // is lifted to 1.
        assert_eq!(png_size(&png(&tie, 1).expect("renders")), (1, 1));
        tie.bounds = Rect::from_corners(Point::new(0, 0), Point::new(1000, 1));
        assert_eq!(png_size(&png(&tie, 100).expect("renders")), (100, 1));
    }

    #[cfg(feature = "png")]
    #[test]
    fn exp_2_png_no_system_font_fallback() {
        let mut scene = scene_of(SHOP, "c2");
        // Glyphs Inter lacks (CJK, an emoji) beside Latin text: a system
        // fallback font would draw them and change the pixels.
        scene.boxes[0].name.lines = vec!["漢字 \u{1F600} Box".to_owned()];
        if let Some(d) = scene.boxes[1].description.as_mut() {
            d.lines = vec!["סקר ひらがな".to_owned()];
        }
        let width = u32::try_from(scene.bounds.width()).expect("positive");
        let bytes = png(&scene, width).expect("renders");
        assert_eq!(
            bytes,
            reference_raster(&svg(&scene), width, rule_height(&scene, width))
        );
    }

    #[cfg(feature = "png")]
    #[test]
    fn exp_2_png_errors() {
        let scene = scene_of(SHOP, "c2");
        assert_eq!(png(&scene, 0), Err(PngError::ZeroWidth));

        let mut tall = scene.clone();
        tall.bounds = Rect::from_corners(Point::new(0, 0), Point::new(1, 1000));
        assert_eq!(
            png(&tall, u32::MAX),
            Err(PngError::TooLarge {
                width_px: u32::MAX,
                height_px: u32::MAX
            })
        );
        // Fits u32 but cannot be allocated.
        let mut wide = scene.clone();
        wide.bounds = Rect::from_corners(Point::new(0, 0), Point::new(1000, 1));
        let height = rule_height(&wide, u32::MAX);
        assert_eq!(
            png(&wide, u32::MAX),
            Err(PngError::TooLarge {
                width_px: u32::MAX,
                height_px: height
            })
        );

        let mut flat = scene.clone();
        flat.bounds = Rect::from_corners(Point::new(10, 0), Point::new(10, 500));
        assert!(matches!(png(&flat, 100), Err(PngError::Render(_))));
        flat.bounds = Rect::from_corners(Point::new(0, 10), Point::new(500, 10));
        assert!(matches!(png(&flat, 100), Err(PngError::Render(_))));
        // Checked in order: width, then bounds, then size.
        assert_eq!(png(&flat, 0), Err(PngError::ZeroWidth));
        assert!(matches!(png(&flat, u32::MAX), Err(PngError::Render(_))));

        // Every error reads as one sentence.
        for e in [
            PngError::ZeroWidth,
            PngError::NoFont,
            PngError::TooLarge {
                width_px: 7,
                height_px: 9,
            },
            PngError::Render("bad".into()),
        ] {
            let text = e.to_string();
            assert!(text.ends_with('.'), "{text}");
            assert!(
                text.chars().next().is_some_and(char::is_uppercase),
                "{text}"
            );
            assert_eq!(text.matches(". ").count(), 0, "one sentence: {text}");
        }
        assert!(
            PngError::TooLarge {
                width_px: 7,
                height_px: 9
            }
            .to_string()
            .contains('7')
        );
        assert!(PngError::Render("bad".into()).to_string().contains("bad"));
    }
}
