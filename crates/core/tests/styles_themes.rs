//! Step-4 contract tests of the styles-themes row (SRS 4.6: P2-10..13,
//! P2-21, P2-27, P2-400..P2-440). One or more `#[test]` per requirement,
//! named after it; bodies are `todo!()` until step 5. Fixtures go under
//! `tests/data/styles/`. The CLI tests (P2-432) are in
//! `crates/cli/tests/styles_themes.rs`, the editor tests (P2-417 panel,
//! P2-433, P2-436 canvas) in `crates/editor/src/styles_themes_tests.rs`.
//!
//! Each `TODO(srs)` names the SRS hole and the reading taken (the
//! Structurizr-faithful one, from docs/plans/phase2-decisions.md Part B,
//! pending the owner's yes).


use c4rizer_core::model::ElementKey;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

use c4rizer_core::edit::{self, Edit, StyleProperty, StyleTarget};
use c4rizer_core::place::Keep;
use c4rizer_core::{export, pipeline, scene, view};
use c4rizer_core::model::{
    self, Border, Colour, ElementId, Model, ModelError, ModelErrorKind, RelationshipId,
    Loader, Mime, Mode, NoticeKind, ResolvedRelationshipStyle, Routing, Shape, ThemeSource,
};

/// `model_body` and `views_body` inside one workspace.
fn ws(model_body: &str, views_body: &str) -> String {
    format!("workspace {{\n    model {{\n{model_body}\n    }}\n    views {{\n{views_body}\n    }}\n}}\n")
}

/// One `element "<tag>"` block holding `lines`, in a workspace with a
/// software system `sys` tagged `Tagged`.
fn element_block(tag: &str, lines: &str) -> String {
    ws(
        "        sys = softwareSystem \"Store\" \"\" \"Tagged\"",
        &format!("        styles {{\n            element \"{tag}\" {{\n{lines}\n            }}\n        }}"),
    )
}

fn parse(src: &str) -> Model {
    match model::parse(src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e:?}\n{src}"),
    }
}

fn error(src: &str) -> ModelError {
    match model::parse(src) {
        Ok(_) => panic!("parsed:\n{src}"),
        Err(e) => e,
    }
}

fn slice(src: &str, span: c4rizer_core::text::Span) -> &str {
    &src[span.range()]
}

const fn rgb(r: u8, g: u8, b: u8) -> Colour {
    Colour { r, g, b }
}

/// The 19 reference shapes, in the notation page's order, with their
/// reference spelling.
const SHAPES: [(Shape, &str); 19] = [
    (Shape::Box, "Box"),
    (Shape::RoundedBox, "RoundedBox"),
    (Shape::Circle, "Circle"),
    (Shape::Ellipse, "Ellipse"),
    (Shape::Hexagon, "Hexagon"),
    (Shape::Diamond, "Diamond"),
    (Shape::Cylinder, "Cylinder"),
    (Shape::Bucket, "Bucket"),
    (Shape::Pipe, "Pipe"),
    (Shape::Person, "Person"),
    (Shape::Robot, "Robot"),
    (Shape::Folder, "Folder"),
    (Shape::WebBrowser, "WebBrowser"),
    (Shape::Window, "Window"),
    (Shape::Terminal, "Terminal"),
    (Shape::Shell, "Shell"),
    (Shape::MobileDevicePortrait, "MobileDevicePortrait"),
    (Shape::MobileDeviceLandscape, "MobileDeviceLandscape"),
    (Shape::Component, "Component"),
];

/// Two systems, `a -> b` tagged `Tagged`, `b -> a` untagged, a container
/// view of nothing and the given `styles` body.
fn rel_ws(styles: &str) -> String {
    ws(
        "        a = softwareSystem \"Front\"\n        b = softwareSystem \"Back\"\n        a -> b \"Calls\" \"\" \"Tagged\"\n        b -> a \"Replies\"",
        &format!("        systemContext a {{\n            include *\n        }}\n        styles {{\n{styles}\n        }}"),
    )
}

/// One `relationship "<tag>"` block holding `lines`.
fn rel_block(tag: &str, lines: &str) -> String {
    rel_ws(&format!("            relationship \"{tag}\" {{\n{lines}\n            }}"))
}

const TAGGED: RelationshipId = RelationshipId(0);
const PLAIN: RelationshipId = RelationshipId(1);

/// An in-memory `Loader`: files by absolute path; records every read.
#[derive(Default)]
struct MemLoader {
    files: BTreeMap<PathBuf, Vec<u8>>,
    reads: Vec<PathBuf>,
}

impl MemLoader {
    fn with(files: &[(&str, &[u8])]) -> MemLoader {
        MemLoader {
            files: files
                .iter()
                .map(|(p, b)| (PathBuf::from(p), b.to_vec()))
                .collect(),
            reads: Vec::new(),
        }
    }

    fn normal(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for c in path.components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other),
            }
        }
        out
    }
}

impl Loader for MemLoader {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        self.reads.push(path.to_path_buf());
        self.files
            .get(&Self::normal(path))
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
    }

    fn read_dir(&mut self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let normal = Self::normal(path);
        if self.files.contains_key(&normal) || !self.files.keys().any(|k| k.starts_with(&normal)) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "not a folder"));
        }
        Ok(self
            .files
            .keys()
            .filter_map(|k| k.strip_prefix(&normal).ok()?.components().next())
            .map(|c| normal.join(c))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        let normal = Self::normal(path);
        if self.files.keys().any(|k| k.starts_with(&normal)) {
            Ok(normal)
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "no such file"))
        }
    }

    fn env(&self, _name: &str) -> Option<String> {
        None
    }
}

/// The diagram file every theme test parses as.
const ROOT: &str = "/project/workspace.dsl";

fn parse_files(src: &str, loader: &mut MemLoader) -> Result<Model, ModelError> {
    model::parse_with(src, Some(Path::new(ROOT)), loader).map(|p| p.model)
}

/// One system `sys` (tagged `Tagged`), `views_body` in `views`.
fn theme_ws(views_body: &str) -> String {
    ws("        sys = softwareSystem \"Store\" \"\" \"Tagged\"\n        sys -> sys \"Loops\"", views_body)
}

/// A theme JSON file with one element style for `tag`.
fn element_theme(tag: &str, keys: &str) -> Vec<u8> {
    format!("{{\"elements\": [{{\"tag\": \"{tag}\", {keys}}}]}}").into_bytes()
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest-of-image";

/// A context view of `sys` with `user -> sys` and `sys -> peer`; the
/// relationship to `peer` is tagged `Async`. `extra` goes in the view.
fn plan_ws(extra: &str, styles: &str) -> String {
    ws(
        "        user = person \"Operator\"\n        sys = softwareSystem \"Store\" \"\" \"Tagged\"\n        peer = softwareSystem \"Peer\"\n        user -> sys \"Uses\"\n        sys -> peer \"Sends\" \"\" \"Async\"",
        &format!("        systemContext sys \"ctx\" {{\n            include *\n{extra}\n        }}\n        styles {{\n{styles}\n        }}"),
    )
}

/// `edits` applied to `src`; the new text.
fn edited(src: &str, edits: &[Edit]) -> String {
    let parsed = model::parse(src).expect("parses");
    edit::apply(&parsed, edits).unwrap_or_else(|e| panic!("{e}")).text
}

fn set_style(element: &str, target: StyleTarget, property: StyleProperty) -> Edit {
    Edit::SetStyle {
        element: ElementKey::from_written(element),
        target,
        property,
    }
}

/// The lines of `after` that differ from `before` (same line count).
fn changed_lines<'a>(before: &str, after: &'a str) -> Vec<&'a str> {
    assert_eq!(before.lines().count(), after.lines().count(), "{after}");
    before
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .map(|(_, b)| b)
        .collect()
}

fn sys(m: &Model) -> ElementId {
    m.find_written("sys").expect("sys")
}

// ---- Element style properties (styles-themes-01)

/// Each of `shape`, `icon`, `width`, `height`, `background`, `color`,
/// `colour`, `stroke`, `strokeWidth`, `fontSize`, `border`, `opacity`,
/// `metadata`, `description` and `properties` in an `element` block (top-
/// level and in `light`/`dark`) lands in `ElementStyle` with its value
/// span.
#[test]
fn p2_10_every_element_property_parsed() {
    let lines = "                shape Cylinder
                icon icon.png
                width 300
                height 200
                background #112233
                colour #445566
                stroke #778899
                strokeWidth 3
                fontSize 20
                border dashed
                opacity 50
                metadata false
                description false
                properties {
                    owner \"team\"
                }";
    for src in [
        element_block("Tagged", lines),
        ws(
            "        sys = softwareSystem \"Store\"",
            &format!("        styles {{\n            dark {{\n                element \"Tagged\" {{\n{lines}\n                }}\n            }}\n        }}"),
        ),
    ] {
        let m = parse(&src);
        let st = &m.element_styles[0];
        assert_eq!(st.shape.unwrap().value, Shape::Cylinder);
        assert_eq!(slice(&src, st.icon.as_ref().unwrap().span), "icon.png");
        assert_eq!(st.icon.as_ref().unwrap().value, "icon.png");
        assert_eq!(st.width.unwrap().value, 300);
        assert_eq!(slice(&src, st.width.unwrap().span), "300");
        assert_eq!(st.height.unwrap().value, 200);
        assert_eq!(st.background.unwrap().value, rgb(0x11, 0x22, 0x33));
        assert_eq!(st.color.unwrap().value, rgb(0x44, 0x55, 0x66));
        assert_eq!(slice(&src, st.color.unwrap().span), "#445566");
        assert_eq!(st.stroke.unwrap().value, rgb(0x77, 0x88, 0x99));
        assert_eq!(slice(&src, st.stroke.unwrap().span), "#778899");
        assert_eq!(st.stroke_width.unwrap().value, 3);
        assert_eq!(slice(&src, st.stroke_width.unwrap().span), "3");
        assert_eq!(st.font_size.unwrap().value, 20);
        assert_eq!(slice(&src, st.font_size.unwrap().span), "20");
        assert_eq!(st.border.unwrap().value, Border::Dashed);
        assert_eq!(slice(&src, st.border.unwrap().span), "dashed");
        assert_eq!(st.opacity.unwrap().value, 50);
        assert!(!st.metadata.unwrap().value);
        assert_eq!(slice(&src, st.metadata.unwrap().span), "false");
        assert!(!st.description.unwrap().value);
        assert_eq!(st.properties.len(), 1);
        assert_eq!(st.properties[0].name.value, "owner");
        assert_eq!(st.properties[0].value.value, "team");
        assert_eq!(slice(&src, st.properties[0].span), "owner \"team\"");
    }
}

/// Any other word in an `element` block is still a DSL-4 error naming it.
#[test]
fn p2_10_unknown_property_still_rejected() {
    let e = error(&element_block("Tagged", "                glow 3"));
    match &e.kind {
        ModelErrorKind::UnknownConstruct { construct } => assert_eq!(construct, "glow"),
        other => panic!("{other:?}"),
    }
    assert!(e.to_string().contains("glow"), "{e}");
}

/// `colour #112233` sets the same property as `color #112233`.
#[test]
fn p2_400_colour_alias() {
    let a = parse(&element_block("Tagged", "                colour #112233"));
    let b = parse(&element_block("Tagged", "                color #112233"));
    assert_eq!(a.element_styles[0].color.unwrap().value, rgb(0x11, 0x22, 0x33));
    assert_eq!(a.style_of(sys(&a)), b.style_of(sys(&b)));
}

/// A block setting `color` twice, or `color` and `colour`, keeps the last
/// line (as Structurizr), with a no-effect notice at the overridden line.
#[test]
fn p2_400_color_and_colour_last_wins() {
    for lines in [
        "                color #112233\n                color #445566",
        "                color #112233\n                colour #445566",
        "                colour #112233\n                color #445566",
    ] {
        let m = parse(&element_block("Tagged", lines));
        assert_eq!(m.element_styles[0].color.unwrap().value, rgb(0x44, 0x55, 0x66), "{lines}");
        let notices: Vec<_> = m.notices.iter().filter(|n| n.kind == NoticeKind::NoEffect).collect();
        assert_eq!(notices.len(), 1, "{lines}: {:?}", m.notices);
        assert!(notices[0].subject.contains("overridden"), "{:?}", notices[0]);
        // The notice names the earlier line, the one with no effect.
        let first = m.element_styles[0].color.unwrap().span.start;
        assert!(notices[0].span.unwrap().start < first, "{:?}", notices[0]);
    }
    // Any other style property repeated: the last wins too.
    let m = parse(&element_block("Tagged", "                width 300\n                width 500"));
    assert_eq!(m.element_styles[0].width.unwrap().value, 500);
}

/// A CSS/HTML colour name resolves to its CSS hex value for element
/// `background`, `color`/`colour` and `stroke`.
#[test]
fn p2_401_named_colours() {
    let m = parse(&element_block(
        "Tagged",
        "                background rebeccapurple\n                color white\n                stroke darkslategray",
    ));
    let st = m.style_of(sys(&m));
    assert_eq!(st.background, rgb(0x66, 0x33, 0x99));
    assert_eq!(st.color, rgb(0xff, 0xff, 0xff));
    assert_eq!(st.stroke, Some(rgb(0x2f, 0x4f, 0x4f)));
}

/// `RED`, `Red` and `red` all resolve to #ff0000.
#[test]
fn p2_401_named_colour_any_case() {
    for word in ["RED", "Red", "red", "\"red\""] {
        let m = parse(&element_block("Tagged", &format!("                background {word}")));
        assert_eq!(m.style_of(sys(&m)).background, rgb(0xff, 0, 0), "{word}");
    }
}

/// A word that is neither `#rrggbb` nor a CSS colour name is an invalid-
/// value error.
#[test]
fn p2_401_unknown_colour_name_invalid() {
    for line in ["background reddish", "stroke #12345", "colour bluegreen"] {
        let e = error(&element_block("Tagged", &format!("                {line}")));
        let ModelErrorKind::InvalidValue { value, .. } = &e.kind else {
            panic!("{line}: {e:?}");
        };
        assert_eq!(value, line.split(' ').nth(1).unwrap());
    }
}

/// `width W` and `height H` land in `ResolvedStyle::width`/`height`, each
/// on its own.
#[test]
fn p2_402_width_height_resolved() {
    let w = parse(&element_block("Tagged", "                width 300"));
    let st = w.style_of(sys(&w));
    assert_eq!((st.width, st.height), (Some(300), None));
    let h = parse(&element_block("Tagged", "                height 180"));
    let st = h.style_of(sys(&h));
    assert_eq!((st.width, st.height), (None, Some(180)));
    for bad in ["width 0", "height -20", "width wide"] {
        let e = error(&element_block("Tagged", &format!("                {bad}")));
        assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{bad}: {e:?}");
    }
}

/// A width or height that is not a multiple of 20 is rounded up to the
/// next multiple of 20, taken as literal px.
#[test]
fn p2_402_off_grid_width() {
    // Approver ruling 2026-11-17 (replaces B styles-themes 4's 10): the
    // P2-402 sentence as written, the next multiple of 20.
    let m = parse(&element_block("Tagged", "                width 301\n                height 175"));
    let st = m.style_of(sys(&m));
    assert_eq!((st.width, st.height), (Some(320), Some(180)));
    let m = parse(&element_block("Tagged", "                width 320\n                height 190"));
    let st = m.style_of(sys(&m));
    assert_eq!((st.width, st.height), (Some(320), Some(200)));
    // The written value is kept for edits.
    let m = parse(&element_block("Tagged", "                width 301\n                height 175"));
    assert_eq!(m.element_styles[0].width.unwrap().value, 301);
}

/// The drawn box has the resolved size: resolved size equals drawn size.
#[test]
fn p2_402_off_grid_width_drawn() {
    let (m, s) = ctx_scene(&tagged("                width 320\n                height 180"));
    let b = scene_box(&m, &s, "sys");
    assert_eq!((b.rect.width(), b.rect.height()), (320, 180));
    let (m, s) = ctx_scene(&tagged("                width 301\n                height 175"));
    let b = scene_box(&m, &s, "sys");
    let st = m.style_of(sys(&m));
    assert_eq!((b.rect.width(), b.rect.height()), (320, 180));
    assert_eq!((st.width, st.height), (Some(b.rect.width()), Some(b.rect.height())));
}

/// `stroke <colour>` lands in `ElementStyle::stroke` and
/// `ResolvedStyle::stroke`.
#[test]
fn p2_403_stroke_parsed() {
    let m = parse(&element_block("Tagged", "                stroke #abcdef"));
    assert_eq!(m.element_styles[0].stroke.unwrap().value, rgb(0xab, 0xcd, 0xef));
    assert_eq!(m.style_of(sys(&m)).stroke, Some(rgb(0xab, 0xcd, 0xef)));
    let plain = parse(&element_block("Other", "                stroke #abcdef"));
    assert_eq!(plain.style_of(sys(&plain)).stroke, None);
}

/// `strokeWidth N` is kept for 1..10; a value outside the range is clamped.
#[test]
fn p2_404_stroke_width_range() {
    // TODO(srs): P2-404 says invalid-value error; B styles-themes 13 [I]
    // (Structurizr for Java) clamps with no error. Reading taken: clamp;
    // checked against Structurizr 6.2.4's setters (same clamp).
    for (written, kept) in [("1", 1), ("10", 10), ("4", 4), ("0", 1), ("11", 10), ("-3", 1), ("400", 10)] {
        let m = parse(&element_block("Tagged", &format!("                strokeWidth {written}")));
        assert_eq!(m.style_of(sys(&m)).stroke_width, kept, "{written}");
    }
    let plain = parse(&element_block("Other", "                strokeWidth 4"));
    assert_eq!(plain.style_of(sys(&plain)).stroke_width, 1);
    let e = error(&element_block("Tagged", "                strokeWidth thick"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

/// `border solid|dashed|dotted` lands in `ElementStyle::border`; the
/// resolved default is `Border::Solid`.
#[test]
fn p2_405_border_parsed() {
    for (word, border) in [("solid", Border::Solid), ("Dashed", Border::Dashed), ("DOTTED", Border::Dotted)] {
        let m = parse(&element_block("Tagged", &format!("                border {word}")));
        assert_eq!(m.element_styles[0].border.unwrap().value, border);
        assert_eq!(m.style_of(sys(&m)).border, border);
    }
    let plain = parse(&element_block("Other", "                border dashed"));
    assert_eq!(plain.style_of(sys(&plain)).border, Border::Solid);
    let e = error(&element_block("Tagged", "                border wavy"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

/// `opacity N` is kept for 0..100; a value outside the range is clamped.
#[test]
fn p2_406_opacity_range() {
    // TODO(srs): P2-406 says invalid-value error; B styles-themes 13 [I]
    // clamps. Reading taken: clamp.
    for (written, kept) in [("0", 0), ("100", 100), ("35", 35), ("101", 100), ("-5", 0), ("1000", 100)] {
        let m = parse(&element_block("Tagged", &format!("                opacity {written}")));
        assert_eq!(m.style_of(sys(&m)).opacity, kept, "{written}");
    }
    let plain = parse(&element_block("Other", "                opacity 10"));
    assert_eq!(plain.style_of(sys(&plain)).opacity, 100);
    let e = error(&element_block("Tagged", "                opacity half"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

/// `metadata false` and `description false` land in the style; both resolve
/// `true` by default.
#[test]
fn p2_408_metadata_description_flags() {
    let m = parse(&element_block(
        "Tagged",
        "                metadata false\n                description FALSE",
    ));
    let st = m.style_of(sys(&m));
    assert!(!st.metadata);
    assert!(!st.description);
    let t = parse(&element_block("Tagged", "                metadata true"));
    assert!(t.style_of(sys(&t)).metadata);
    let plain = parse(&element_block("Other", "                metadata false"));
    let st = plain.style_of(sys(&plain));
    assert!(st.metadata && st.description);
    let e = error(&element_block("Tagged", "                metadata maybe"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

/// `properties { name value }` in an element style is parsed into
/// `Property` values, prints back byte for byte and changes no scene.
#[test]
fn p2_410_element_style_properties_kept() {
    let with = element_block(
        "Tagged",
        "                background #112233\n                properties {\n                    \"a key\" value\n                    b \"two words\"\n                }",
    );
    let without = element_block("Tagged", "                background #112233");
    let parsed = model::parse(&with).expect("parses");
    assert_eq!(parsed.trees[0].print(), with);
    let props = &parsed.model.element_styles[0].properties;
    assert_eq!(props.len(), 2);
    assert_eq!((props[0].name.value.as_str(), props[0].value.value.as_str()), ("a key", "value"));
    assert_eq!((props[1].name.value.as_str(), props[1].value.value.as_str()), ("b", "two words"));
    let m = &parsed.model;
    let plain = parse(&without);
    assert_eq!(m.style_of(sys(m)), plain.style_of(sys(&plain)));
    // A properties line takes exactly a name and a value.
    let e = error(&element_block("Tagged", "                properties {\n                    a b c\n                }"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

// ---- Shapes (styles-themes-02, -08, -09)

/// All 19 reference shapes parse in any letter case.
#[test]
fn p2_11_all_shapes_parse_case_insensitive() {
    for (shape, word) in SHAPES {
        for written in [word.to_owned(), word.to_ascii_lowercase(), word.to_ascii_uppercase()] {
            let src = element_block("Tagged", &format!("                shape {written}"));
            let m = parse(&src);
            let st = m.element_styles[0].shape.unwrap();
            assert_eq!(st.value, shape, "{written}");
            assert_eq!(slice(&src, st.span), written);
            assert_eq!(m.style_of(sys(&m)).shape, shape);
        }
    }
}

/// An unknown shape word is an invalid-value error.
#[test]
fn p2_11_unknown_shape_invalid() {
    for word in ["Blob", "Rounded", "MobileDevice"] {
        let e = error(&element_block("Tagged", &format!("                shape {word}")));
        let ModelErrorKind::InvalidValue { value, .. } = &e.kind else {
            panic!("{word}: {e:?}");
        };
        assert_eq!(value, word);
    }
}

/// `Shape::keyword` gives the reference spelling of every shape
/// (`MobileDevicePortrait`).
#[test]
fn p2_11_shape_keyword_reference_spelling() {
    for (shape, word) in SHAPES {
        assert_eq!(shape.keyword(), word);
    }
}

/// The name of an outline's variant (and its flag), for comparisons.
fn outline_kind(o: &scene::Outline) -> String {
    use scene::Outline as O;
    match o {
        O::Rect { .. } => "Rect".into(),
        O::Person { .. } => "Person".into(),
        O::Cylinder { .. } => "Cylinder".into(),
        O::Ellipse { circle, .. } => format!("Ellipse {circle}"),
        O::Hexagon { .. } => "Hexagon".into(),
        O::Diamond { .. } => "Diamond".into(),
        O::Bucket { .. } => "Bucket".into(),
        O::Pipe { .. } => "Pipe".into(),
        O::Robot { .. } => "Robot".into(),
        O::Folder { .. } => "Folder".into(),
        O::Window { kind, .. } => format!("Window {kind:?}"),
        O::MobileDevice { portrait, .. } => format!("MobileDevice {portrait}"),
        O::Component { .. } => "Component".into(),
        other => panic!("unknown outline {other:?}"),
    }
}

/// The view `ctx` with `sys` drawn in `shape` at 400x300 px; its model and
/// scene.
fn shaped(shape: &str) -> (Model, scene::Scene) {
    let m = parse(&plan_ws("            size sys 400 300", &tagged(&format!("                shape {shape}"))));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    (m, s)
}

/// Every rectangle an outline is made of (circles as their squares).
fn outline_rects(o: &scene::Outline) -> Vec<c4rizer_core::geom::Rect> {
    use c4rizer_core::geom::Rect;
    use scene::Outline as O;
    match *o {
        O::Rect { rect, .. }
        | O::Cylinder { rect, .. }
        | O::Ellipse { rect, .. }
        | O::Hexagon { rect, .. }
        | O::Diamond { rect }
        | O::Bucket { rect, .. }
        | O::Pipe { rect, .. }
        | O::Window { rect, .. }
        | O::MobileDevice { rect, .. } => vec![rect],
        O::Person { head_centre, head_radius_px, body, .. } => {
            vec![Rect::from_centre(head_centre, head_radius_px, head_radius_px), body]
        }
        O::Robot { head, body, .. } => vec![head, body],
        O::Folder { rect, tab } => vec![rect, tab],
        O::Component { rect, tabs } => vec![rect, tabs[0], tabs[1]],
        ref other => panic!("unknown outline {other:?}"),
    }
}

/// True when `p` lies in the part of `o` texts may use.
fn in_text_part(outline: &scene::Outline, at: c4rizer_core::geom::Point) -> bool {
    use scene::Outline as O;
    let (x, y) = (i64::from(at.x), i64::from(at.y));
    match *outline {
        O::Rect { rect, .. } | O::Folder { rect, .. } => rect.contains(at),
        O::Person { body, .. } | O::Robot { body, .. } => body.contains(at),
        O::Cylinder { rect, cap_ry_px } | O::Bucket { rect, cap_ry_px } => {
            rect.contains(at) && at.y >= rect.min.y + 2 * cap_ry_px
        }
        O::Pipe { rect, cap_rx_px } => rect.contains(at) && at.x <= rect.max.x - 2 * cap_rx_px,
        O::Ellipse { rect, .. } => {
            let c = rect.centre();
            let (rx, ry) = (i64::from(rect.width()) / 2, i64::from(rect.height()) / 2);
            let (dx, dy) = (x - i64::from(c.x), y - i64::from(c.y));
            dx * dx * ry * ry + dy * dy * rx * rx <= rx * rx * ry * ry
        }
        O::Hexagon { rect, inset_px } => {
            rect.contains(at) && at.x >= rect.min.x + inset_px && at.x <= rect.max.x - inset_px
        }
        O::Diamond { rect } => {
            let c = rect.centre();
            let (rx, ry) = (i64::from(rect.width()) / 2, i64::from(rect.height()) / 2);
            (x - i64::from(c.x)).abs() * ry + (y - i64::from(c.y)).abs() * rx <= rx * ry
        }
        O::Window { rect, bar_px, .. } => rect.contains(at) && at.y >= rect.min.y + bar_px,
        O::MobileDevice { rect, inset_px, .. } => rect.inflate(-inset_px).contains(at),
        O::Component { rect, tabs } => rect.contains(at) && at.x >= tabs[0].max.x,
        ref other => panic!("unknown outline {other:?}"),
    }
}

/// Each of the 19 shapes builds its own `Outline` variant (or `Ellipse {
/// circle }` / `Window { kind }` / `MobileDevice { portrait }` value).
#[test]
fn p2_11_each_shape_distinct_outline() {
    let want = [
        "Rect", "Rect", "Ellipse true", "Ellipse false", "Hexagon", "Diamond", "Cylinder",
        "Bucket", "Pipe", "Person", "Robot", "Folder", "Window WebBrowser", "Window Window",
        "Window Terminal", "Window Shell", "MobileDevice true", "MobileDevice false", "Component",
    ];
    let mut seen = Vec::new();
    for ((shape, word), want) in SHAPES.iter().zip(want) {
        let (m, s) = shaped(word);
        let b = scene_box(&m, &s, "sys");
        assert_eq!(b.shape, *shape);
        let kind = outline_kind(&b.outline);
        assert_eq!(kind, want, "{word}");
        // Box and RoundedBox differ by radius only.
        let key = match b.outline {
            scene::Outline::Rect { radius_px, .. } => format!("{kind} {radius_px}"),
            _ => kind,
        };
        assert!(!seen.contains(&key), "{word} repeats {key}");
        seen.push(key);
    }
}

/// For every shape the outline lies inside the box rectangle and every text
/// inside the outline's text area.
#[test]
fn p2_413_outline_inside_rect_texts_inside_outline() {
    for (_, word) in SHAPES {
        let (m, s) = shaped(word);
        let b = scene_box(&m, &s, "sys");
        for r in outline_rects(&b.outline) {
            assert!(b.rect.contains(r.min) && b.rect.contains(r.max), "{word}: {r:?} outside {:?}", b.rect);
        }
        for t in [Some(&b.name), Some(&b.type_line), b.description.as_ref()].into_iter().flatten() {
            if t.lines.is_empty() {
                continue;
            }
            assert!(t.fits, "{word}: {:?} does not fit", t.lines);
            let r = t.bounds;
            for p in [r.min, r.max, c4rizer_core::geom::Point::new(r.min.x, r.max.y), c4rizer_core::geom::Point::new(r.max.x, r.min.y)] {
                assert!(in_text_part(&b.outline, p), "{word}: {:?} outside the text area", t.lines);
            }
        }
    }
}

/// Each outline's proportions follow Structurizr's notation page.
#[test]
fn p2_413_shape_proportions() {
    // TODO(srs): P2-413, exact proportions. Reading taken (B styles-themes
    // 6): per the notation page.
    use scene::Outline as O;
    for (_, word) in SHAPES {
        let (model, picture) = shaped(word);
        let item = scene_box(&model, &picture, "sys");
        let r = item.rect;
        let (w, h) = (r.width(), r.height());
        match item.outline {
            O::Ellipse { rect, circle: true } => {
                assert_eq!((rect.width(), rect.height()), (h, h));
                assert_eq!(rect.centre(), r.centre());
            }
            O::Hexagon { rect, inset_px } => assert_eq!((rect, inset_px), (r, h / 4)),
            // redesign 2026-11-19: 9px caps (Node: "a 9 px lid"), was 12.
            O::Bucket { rect, cap_ry_px } | O::Cylinder { rect, cap_ry_px } => assert_eq!((rect, cap_ry_px), (r, 9)),
            // redesign 2026-11-19: the pipe shares the 9px cylinder cap.
            O::Pipe { rect, cap_rx_px } => assert_eq!((rect, cap_rx_px), (r, 9)),
            O::Robot { head, body, .. } => {
                assert_eq!(head.centre().x, r.centre().x);
                assert_eq!((head.width(), head.height()), (w / 2, h / 4));
                assert_eq!((body.min.y, body.max), (head.max.y, r.max));
            }
            O::Folder { rect, tab } => {
                assert_eq!((tab.min, tab.width(), tab.height()), (r.min, w / 3, 10));
                assert_eq!((rect.min.y, rect.max), (tab.max.y, r.max));
            }
            O::Window { rect, bar_px, .. } => assert_eq!((rect, bar_px), (r, 20)),
            O::MobileDevice { rect, inset_px, .. } => assert_eq!((rect, inset_px), (r, 10)),
            O::Component { rect, tabs } => {
                assert_eq!((rect.min.x, rect.max), (r.min.x + 10, r.max));
                assert!(tabs.iter().all(|t| t.min.x == r.min.x && t.width() == 20 && t.height() == 10));
                assert!(tabs[0].max.y < tabs[1].min.y);
            }
            O::Rect { rect, .. } | O::Ellipse { rect, circle: false } | O::Diamond { rect } => assert_eq!(rect, r),
            // redesign 2026-11-19: a 24px head with the body from 30px
            // down and 14px corners (Node preview), was a 30px head.
            O::Person {
                head_centre,
                head_radius_px,
                body,
                body_radius_px,
            } => {
                assert_eq!((body.max, head_radius_px, body_radius_px), (r.max, 24, 14));
                assert_eq!((head_centre.y - head_radius_px, body.min.y), (r.min.y, r.min.y + 30));
            }
            ref other => panic!("{other:?}"),
        }
    }
}

/// A container view `all` of 19 containers, one per shape (`c<i>` tagged
/// `S<i>`), each connected to the next both ways.
fn every_shape_ws() -> String {
    let mut model = String::from("        sys = softwareSystem \"Store\" {\n");
    let mut styles = String::new();
    for (i, (_, word)) in SHAPES.iter().enumerate() {
        let _ = writeln!(model, "            c{i} = container \"Part {i}\" \"\" \"\" \"S{i}\"");
        let _ = writeln!(styles, "            element \"S{i}\" {{\n                shape {word}\n            }}");
    }
    model.push_str("        }\n");
    for i in 0..SHAPES.len() {
        let j = (i + 1) % SHAPES.len();
        let _ = writeln!(model, "        c{i} -> c{j} \"Sends\"\n        c{j} -> c{i} \"Answers\"");
    }
    ws(&model, &format!("        container sys \"all\" {{\n            include *\n        }}\n        styles {{\n{styles}        }}"))
}

/// The side of `rect` that `at` lies on, away from the corners by 20px,
/// as the outward unit step; `None` when it is on no side.
fn port_side(rect: c4rizer_core::geom::Rect, at: c4rizer_core::geom::Point) -> Option<(i32, i32)> {
    let along_x = at.x >= rect.min.x + 20 && at.x <= rect.max.x - 20;
    let along_y = at.y >= rect.min.y + 20 && at.y <= rect.max.y - 20;
    if at.y == rect.min.y && along_x {
        Some((0, -1))
    } else if at.y == rect.max.y && along_x {
        Some((0, 1))
    } else if at.x == rect.min.x && along_y {
        Some((-1, 0))
    } else if at.x == rect.max.x && along_y {
        Some((1, 0))
    } else {
        None
    }
}

/// CORE-6..9 measured on the box rectangles of `s`: every routed line on
/// the 10px grid, octilinear, leaving and entering square to a rectangle
/// side at its own port, 20px clear of the corners.
fn check_lines_on_rects(s: &scene::Scene) {
    let mut ports = std::collections::BTreeSet::new();
    for line in &s.lines {
        assert!(line.routed, "{:?} unrouted", line.edge);
        for p in &line.points {
            assert!(p.x % 10 == 0 && p.y % 10 == 0, "{p:?} off the grid");
        }
        for w in line.points.windows(2) {
            let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
            assert!(dx == 0 || dy == 0 || dx.abs() == dy.abs(), "{w:?} not octilinear");
        }
        let n = line.points.len();
        let mut ends = vec![(line.source, line.points[0], line.points[1], 1)];
        if matches!(line.tip, scene::Tip::Arrow(_)) {
            ends.push((line.destination, line.points[n - 1], line.points[n - 2], -1));
        }
        for (b, at, next, sign) in ends {
            let rect = s.boxes[b.0 as usize].rect;
            let (ox, oy) = port_side(rect, at).unwrap_or_else(|| panic!("{at:?} not on a side of {rect:?}"));
            let (dx, dy) = ((next.x - at.x).signum(), (next.y - at.y).signum());
            assert_eq!((dx, dy), (ox, oy), "line {:?} not square to the side (sign {sign})", line.edge);
            assert!(ports.insert((b, at)), "two lines share {at:?}");
        }
    }
}

/// A view drawing every shape, connected both ways, meets CORE-6..9
/// measured on the box rectangle.
#[test]
fn p2_414_core_6_to_9_hold_every_shape() {
    let m = parse(&every_shape_ws());
    let s = build_in(&m, "all", Mode::Light).scene;
    assert_eq!(s.boxes.len(), 19);
    let shapes: std::collections::BTreeSet<&str> = s.boxes.iter().map(|b| b.shape.keyword()).collect();
    assert_eq!(shapes.len(), 19);
    check_lines_on_rects(&s);
}

/// Lines end on the box rectangle, not the outline, for Circle, Ellipse,
/// Hexagon, Diamond and Robot.
#[test]
fn p2_414_per_shape_port_rules() {
    // TODO(srs): P2-414, port rules per shape. Reading taken (B styles-
    // themes 6): lines end on the box rectangle, CORE-6 grid ports hold.
    let m = parse(&every_shape_ws());
    let s = build_in(&m, "all", Mode::Light).scene;
    for shape in [Shape::Circle, Shape::Ellipse, Shape::Hexagon, Shape::Diamond, Shape::Robot] {
        let b = s.boxes.iter().find(|b| b.shape == shape).expect("box");
        let ends: Vec<_> = s
            .lines
            .iter()
            .flat_map(|l| {
                let mut v = Vec::new();
                if l.source == b.id {
                    v.push(l.points[0]);
                }
                if l.destination == b.id {
                    v.push(l.points[l.points.len() - 1]);
                }
                v
            })
            .collect();
        assert!(!ends.is_empty(), "{shape:?}");
        for at in ends {
            assert!(port_side(b.rect, at).is_some(), "{shape:?}: {at:?} not on the rectangle");
        }
    }
}

/// A `RoundedBox`'s corner radius is larger than a `Box`'s.
#[test]
fn p2_415_rounded_box_radius_larger() {
    let (m, s) = shaped("RoundedBox");
    let rounded = match scene_box(&m, &s, "sys").outline {
        scene::Outline::Rect { radius_px, .. } => radius_px,
        other => panic!("{other:?}"),
    };
    let (m, s) = shaped("Box");
    let square = match scene_box(&m, &s, "sys").outline {
        scene::Outline::Rect { radius_px, .. } => radius_px,
        other => panic!("{other:?}"),
    };
    assert!(rounded > square, "{rounded} {square}");
}

/// `Box` keeps the phase-1 6px radius and `RoundedBox` uses 20px.
#[test]
fn p2_415_box_corner_radius() {
    // TODO(srs): P2-415. Reading taken (B styles-themes 3): Box 6px (phase-1
    // goldens), RoundedBox 20px.
    // redesign 2026-11-19: RoundedBox 14px (`radius-node-round`), still
    // larger than the Box's 6px.
    for (word, want) in [("Box", 6), ("RoundedBox", 14)] {
        let (m, s) = shaped(word);
        match scene_box(&m, &s, "sys").outline {
            scene::Outline::Rect { radius_px, .. } => assert_eq!(radius_px, want, "{word}"),
            other => panic!("{other:?}"),
        }
    }
}

/// A box key entry's swatch draws that entry's shape outline, for every
/// shape.
#[test]
fn p2_416_swatch_uses_shape_outline() {
    for (_, word) in SHAPES {
        let (m, s) = shaped(word);
        let b = scene_box(&m, &s, "sys");
        let entry = s
            .legend
            .entries
            .iter()
            .find(|e| e.names.iter().any(|n| n == "Tagged"))
            .expect("entry");
        let scene::Swatch::Box { outline, .. } = entry.swatch else {
            panic!("{:?}", entry.swatch)
        };
        assert_eq!(outline_kind(&outline), outline_kind(&b.outline), "{word}");
        for r in outline_rects(&outline) {
            assert!(r.width() <= 40 && r.height() <= 30, "{word}: swatch {r:?}");
        }
    }
}

// ---- Relationship styles (styles-themes-03)

/// Each of `thickness`, `color`, `colour`, `style`, `dashed`, `routing`,
/// `jump`, `fontSize`, `width`, `position`, `opacity`, `properties` lands
/// in `RelationshipStyle` with its value span.
#[test]
fn p2_12_relationship_style_parsed() {
    let src = rel_block(
        "Tagged",
        "                thickness 4
                colour #112233
                style dotted
                routing Curved
                jump true
                fontSize 18
                width 300
                position 30
                opacity 40
                properties {
                    k v
                }",
    );
    let m = parse(&src);
    let st = &m.relationship_styles[0];
    assert_eq!(st.tag.value, "Tagged");
    assert!(slice(&src, st.span).starts_with("relationship \"Tagged\""));
    assert_eq!(st.mode, None);
    assert_eq!(st.theme, None);
    let check = |span, text: &str| assert_eq!(slice(&src, span), text);
    assert_eq!(st.thickness.unwrap().value, 4);
    check(st.thickness.unwrap().span, "4");
    assert_eq!(st.color.unwrap().value, rgb(0x11, 0x22, 0x33));
    check(st.color.unwrap().span, "#112233");
    assert_eq!(st.style.unwrap().value, Border::Dotted);
    check(st.style.unwrap().span, "dotted");
    assert_eq!(st.routing.unwrap().value, Routing::Curved);
    check(st.routing.unwrap().span, "Curved");
    assert!(st.jump.unwrap().value);
    check(st.jump.unwrap().span, "true");
    assert_eq!(st.font_size.unwrap().value, 18);
    check(st.font_size.unwrap().span, "18");
    assert_eq!(st.width.unwrap().value, 300);
    check(st.width.unwrap().span, "300");
    assert_eq!(st.position.unwrap().value, 30);
    check(st.position.unwrap().span, "30");
    assert_eq!(st.opacity.unwrap().value, 40);
    check(st.opacity.unwrap().span, "40");
    assert_eq!(st.properties.len(), 1);
    check(st.properties[0].span, "k v");
    // `color` and `dashed` land in the same fields as `colour` and `style`.
    let src = rel_block("Tagged", "                color #445566\n                dashed true");
    let m = parse(&src);
    let st = &m.relationship_styles[0];
    assert_eq!(st.color.unwrap().value, rgb(0x44, 0x55, 0x66));
    assert_eq!(st.style.unwrap().value, Border::Dashed);
    assert_eq!(slice(&src, st.style.unwrap().span), "true");
}

/// `Model::relationship_style_of` resolves per property over
/// `Relationship`, then the declared tags (DSL-15).
#[test]
fn p2_12_cascade_over_relationship_tags() {
    let m = parse(&rel_ws(
        "            relationship \"Tagged\" {
                thickness 5
            }
            relationship \"Relationship\" {
                thickness 3
                color #010203
            }",
    ));
    let tagged = m.relationship_style_of(TAGGED);
    assert_eq!(tagged.thickness, 5);
    assert_eq!(tagged.color, Some(rgb(1, 2, 3)));
    assert_eq!(tagged.style, Border::Solid);
    let plain = m.relationship_style_of(PLAIN);
    assert_eq!(plain.thickness, 3);
    assert_eq!(plain.color, Some(rgb(1, 2, 3)));
    // No block: the phase-1 look.
    let bare = parse(&rel_ws(""));
    assert_eq!(bare.relationship_style_of(TAGGED), ResolvedRelationshipStyle::DEFAULT);
    // Two blocks for one tag: the later wins per property.
    let m = parse(&rel_ws(
        "            relationship \"Tagged\" {
                thickness 5
                opacity 20
            }
            relationship \"Tagged\" {
                thickness 7
            }",
    ));
    let st = m.relationship_style_of(TAGGED);
    assert_eq!((st.thickness, st.opacity), (7, 20));
}

/// Any other word in a `relationship` block is a DSL-4 error naming it.
#[test]
fn p2_12_unknown_property_rejected() {
    for word in ["background", "shape", "glow"] {
        let e = error(&rel_block("Tagged", &format!("                {word} x")));
        let ModelErrorKind::UnknownConstruct { construct } = &e.kind else {
            panic!("{word}: {e:?}");
        };
        assert_eq!(construct, word);
    }
}

/// A CSS colour name in a relationship `color`/`colour` resolves to its hex
/// value.
#[test]
fn p2_401_relationship_named_colour() {
    for word in ["Crimson", "crimson", "\"CRIMSON\""] {
        let m = parse(&rel_block("Tagged", &format!("                color {word}")));
        assert_eq!(m.relationship_style_of(TAGGED).color, Some(rgb(0xdc, 0x14, 0x3c)));
    }
    let e = error(&rel_block("Tagged", "                colour crimsonish"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

/// `properties { }` in a relationship style is kept byte for byte and
/// changes no scene.
#[test]
fn p2_410_relationship_style_properties_kept() {
    let with = rel_block(
        "Tagged",
        "                thickness 3\n                properties {\n                    \"a key\" \"a value\"\n                }",
    );
    let without = rel_block("Tagged", "                thickness 3");
    let parsed = model::parse(&with).expect("parses");
    assert_eq!(parsed.trees[0].print(), with);
    let props = &parsed.model.relationship_styles[0].properties;
    assert_eq!(props[0].name.value, "a key");
    assert_eq!(props[0].value.value, "a value");
    let plain = parse(&without);
    assert_eq!(
        parsed.model.relationship_style_of(TAGGED),
        plain.relationship_style_of(TAGGED)
    );
    let stop = AtomicBool::new(false);
    let a = pipeline::build_all(&parsed.model, &stop).unwrap();
    let b = pipeline::build_all(&plain, &stop).unwrap();
    assert_eq!(a.views[0].scene, b.views[0].scene);
}

/// `dashed true` means `style dashed`, `dashed false` means `style solid`.
#[test]
fn p2_420_dashed_alias() {
    for (line, border) in [
        ("dashed true", Border::Dashed),
        ("dashed false", Border::Solid),
        ("style dashed", Border::Dashed),
        ("style Solid", Border::Solid),
    ] {
        let m = parse(&rel_block("Tagged", &format!("                {line}")));
        assert_eq!(m.relationship_style_of(TAGGED).style, border, "{line}");
    }
    // One property under two names: the last line wins (P2-400).
    let m = parse(&rel_block("Tagged", "                style dotted\n                dashed true"));
    assert_eq!(m.relationship_style_of(TAGGED).style, Border::Dashed);
    assert!(m.notices.iter().any(|n| n.subject.contains("overridden")), "{:?}", m.notices);
    let e = error(&rel_block("Tagged", "                dashed maybe"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

/// `routing Direct|Orthogonal|Curved` lands in
/// `ResolvedRelationshipStyle::routing`.
#[test]
fn p2_421_routing_parsed() {
    for (word, routing) in [
        ("Direct", Routing::Direct),
        ("orthogonal", Routing::Orthogonal),
        ("CURVED", Routing::Curved),
    ] {
        let m = parse(&rel_block("Tagged", &format!("                routing {word}")));
        assert_eq!(m.relationship_style_of(TAGGED).routing, Some(routing));
        assert_eq!(m.relationship_style_of(PLAIN).routing, None);
    }
    let e = error(&rel_block("Tagged", "                routing Wiggly"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

/// The same file with and without `routing` gives equal scenes.
#[test]
fn p2_421_routing_changes_no_scene() {
    // TODO(srs): P2-421. Reading taken (B styles-themes 1): every value
    // draws with c4rizer's router, no notice.
    let stop = AtomicBool::new(false);
    let plain = parse(&rel_ws(""));
    let base = pipeline::build_all(&plain, &stop).unwrap();
    for word in ["Direct", "Orthogonal", "Curved"] {
        let m = parse(&rel_block("Relationship", &format!("                routing {word}")));
        let built = pipeline::build_all(&m, &stop).unwrap();
        assert_eq!(built.views[0].scene, base.views[0].scene, "{word}");
    }
}

/// `position P` is kept for 0..100; a value outside is clamped.
#[test]
fn p2_425_position_range() {
    // TODO(srs): P2-425 says invalid-value error; B styles-themes 13 [I]
    // clamps. Reading taken: clamp.
    for (written, kept) in [("0", 0), ("100", 100), ("30", 30), ("150", 100), ("-1", 0)] {
        let m = parse(&rel_block("Tagged", &format!("                position {written}")));
        assert_eq!(m.relationship_style_of(TAGGED).position, Some(kept), "{written}");
    }
    let e = error(&rel_block("Tagged", "                position middle"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

/// A relationship `opacity N` is kept for 0..100; a value outside is
/// clamped.
#[test]
fn p2_426_opacity_range() {
    // TODO(srs): P2-426 says invalid-value error; B styles-themes 13 [I]
    // clamps. Reading taken: clamp.
    for (written, kept) in [("0", 0), ("100", 100), ("45", 45), ("101", 100), ("-7", 0)] {
        let m = parse(&rel_block("Tagged", &format!("                opacity {written}")));
        assert_eq!(m.relationship_style_of(TAGGED).opacity, kept, "{written}");
    }
    assert_eq!(parse(&rel_ws("")).relationship_style_of(TAGGED).opacity, 100);
    let e = error(&rel_block("Tagged", "                opacity faint"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

// ---- Light and dark (styles-themes-04)

/// `light { }` and `dark { }` in `styles` hold `element` and `relationship`
/// blocks with `mode` set.
#[test]
fn p2_21_light_dark_blocks_parsed() {
    let src = rel_ws(
        "            element \"Element\" {
                background #111111
            }
            light {
                element \"Element\" {
                    background #eeeeee
                }
                relationship \"Relationship\" {
                    color #222222
                }
            }
            DARK {
                element \"Element\" {
                    background #333333
                }
            }",
    );
    let parsed = model::parse(&src).expect("parses");
    assert_eq!(parsed.trees[0].print(), src);
    let m = &parsed.model;
    let modes: Vec<_> = m.element_styles.iter().map(|s| s.mode).collect();
    assert_eq!(modes, [None, Some(Mode::Light), Some(Mode::Dark)]);
    assert_eq!(m.relationship_styles[0].mode, Some(Mode::Light));
    assert_eq!(slice(&src, m.element_styles[2].background.unwrap().span), "#333333");
}

/// Anything but `element`/`relationship` inside `light`/`dark` is a DSL-4
/// error.
#[test]
fn p2_21_other_children_rejected() {
    for inner in [
        "                styles {\n                }",
        "                light {\n                }",
        "                glow 3",
        "                background #ffffff",
    ] {
        let e = error(&rel_ws(&format!("            dark {{\n{inner}\n            }}")));
        assert!(
            matches!(
                e.kind,
                ModelErrorKind::UnknownConstruct { .. } | ModelErrorKind::LaterPhase { .. }
            ),
            "{inner}: {e:?}"
        );
        assert_eq!(e.position.line, 14, "{inner}");
    }
    // `light` takes no argument and needs a block.
    let e = error(&rel_ws("            light \"x\" {\n            }"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
    let e = error(&rel_ws("            light"));
    assert!(matches!(e.kind, ModelErrorKind::MissingArgument { .. }), "{e:?}");
}

/// `style_of_in`/`relationship_style_of_in` use top-level plus `light`
/// blocks in light mode and top-level plus `dark` blocks in dark mode.
#[test]
fn p2_430_mode_selects_blocks() {
    let m = parse(&rel_ws(
        "            element \"Element\" {
                color #010101
            }
            relationship \"Relationship\" {
                thickness 3
            }
            light {
                element \"Element\" {
                    background #eeeeee
                }
                relationship \"Relationship\" {
                    color #aaaaaa
                }
            }
            dark {
                element \"Element\" {
                    background #222222
                }
                relationship \"Relationship\" {
                    color #bbbbbb
                }
            }",
    ));
    let a = m.find_written("a").unwrap();
    let light = m.style_of_in(a, Mode::Light);
    let dark = m.style_of_in(a, Mode::Dark);
    assert_eq!((light.background, light.color), (rgb(0xee, 0xee, 0xee), rgb(1, 1, 1)));
    assert_eq!((dark.background, dark.color), (rgb(0x22, 0x22, 0x22), rgb(1, 1, 1)));
    assert_eq!(m.style_of(a), light);
    assert_eq!(m.styles_for_in(a, Mode::Light), [0, 1]);
    assert_eq!(m.styles_for_in(a, Mode::Dark), [0, 2]);
    assert_eq!(m.styles_for(a), m.styles_for_in(a, Mode::Light));
    let rl = m.relationship_style_of_in(TAGGED, Mode::Light);
    let rd = m.relationship_style_of_in(TAGGED, Mode::Dark);
    assert_eq!((rl.thickness, rl.color), (3, Some(rgb(0xaa, 0xaa, 0xaa))));
    assert_eq!((rd.thickness, rd.color), (3, Some(rgb(0xbb, 0xbb, 0xbb))));
    assert_eq!(m.relationship_style_of(TAGGED), rl);
}

/// A file with no `light`/`dark` block resolves the same style for every
/// element and relationship in both modes.
#[test]
fn p2_430_no_mode_blocks_same_styles_in_both() {
    let m = parse(&rel_ws(
        "            element \"Software System\" {
                background #123456
                shape RoundedBox
            }
            relationship \"Tagged\" {
                dashed true
            }",
    ));
    for e in &m.elements {
        assert_eq!(m.style_of_in(e.id, Mode::Light), m.style_of_in(e.id, Mode::Dark));
        assert_eq!(m.styles_for_in(e.id, Mode::Light), m.styles_for_in(e.id, Mode::Dark));
    }
    for r in [TAGGED, PLAIN] {
        assert_eq!(
            m.relationship_style_of_in(r, Mode::Light),
            m.relationship_style_of_in(r, Mode::Dark)
        );
    }
}

/// For one tag, a mode block beats a top-level block wherever each is
/// written; theme styles apply in both modes.
#[test]
fn p2_430_mode_block_precedence() {
    // TODO(srs): P2-430. Readings taken (B styles-themes 10a, 10b).
    // The mode block is written first, the top-level block after it: the
    // mode block still wins for its tag.
    let m = parse(&rel_ws(
        "            dark {
                element \"Element\" {
                    background #222222
                }
                relationship \"Relationship\" {
                    thickness 6
                }
            }
            element \"Element\" {
                background #dddddd
                color #333333
            }
            relationship \"Relationship\" {
                thickness 4
            }",
    ));
    let a = m.find_written("a").unwrap();
    let dark = m.style_of_in(a, Mode::Dark);
    assert_eq!((dark.background, dark.color), (rgb(0x22, 0x22, 0x22), rgb(0x33, 0x33, 0x33)));
    assert_eq!(m.style_of_in(a, Mode::Light).background, rgb(0xdd, 0xdd, 0xdd));
    assert_eq!(m.styles_for_in(a, Mode::Dark), [1, 0]);
    assert_eq!(m.relationship_style_of_in(PLAIN, Mode::Dark).thickness, 6);
    assert_eq!(m.relationship_style_of_in(PLAIN, Mode::Light).thickness, 4);
    // A later tag still beats an earlier tag's mode block (DSL-15).
    let m = parse(&rel_ws(
        "            dark {
                element \"Element\" {
                    background #222222
                }
            }
            element \"Software System\" {
                background #444444
            }",
    ));
    let a = m.find_written("a").unwrap();
    assert_eq!(m.style_of_in(a, Mode::Dark).background, rgb(0x44, 0x44, 0x44));
}

// ---- Themes (styles-themes-05)

/// `theme <x>` and `themes <x> <y>` inside `views` fill `Model::themes` in
/// order with their `ThemeSource`.
#[test]
fn p2_13_theme_and_themes_parsed() {
    let src = theme_ws(
        "        theme default\n        themes \"https://example.com/t.json\" one/theme.json\n        theme two.json",
    );
    let mut loader = MemLoader::with(&[
        ("/project/one/theme.json", b"{}"),
        ("/project/two.json", b"{\"elements\": []}"),
    ]);
    let m = parse_files(&src, &mut loader).expect("parses");
    let kinds: Vec<_> = m.themes.iter().map(|t| t.kind.clone()).collect();
    assert_eq!(
        kinds,
        [
            ThemeSource::Name("default".into()),
            ThemeSource::Url("https://example.com/t.json".into()),
            ThemeSource::File("one/theme.json".into()),
            ThemeSource::File("two.json".into()),
        ]
    );
    assert_eq!(slice(&src, m.themes[0].source.span), "default");
    assert_eq!(slice(&src, m.themes[0].span), "theme default");
    assert_eq!(slice(&src, m.themes[1].source.span), "\"https://example.com/t.json\"");
    assert_eq!(m.themes[1].source.value, "https://example.com/t.json");
    assert!(slice(&src, m.themes[2].span).starts_with("themes "));
    assert_eq!(m.themes[2].span, m.themes[1].span);
    let printed = model::parse_with(&src, Some(Path::new(ROOT)), &mut loader).unwrap();
    assert_eq!(printed.trees[0].print(), src);
    // `themes` needs at least one source, `theme` exactly one.
    let e = error(&theme_ws("        themes"));
    assert!(matches!(e.kind, ModelErrorKind::MissingArgument { .. }), "{e:?}");
    let e = error(&theme_ws("        theme a.json b.json"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
}

/// For one tag the file's own block beats the theme's.
#[test]
fn p2_13_file_style_beats_theme_same_tag() {
    // The `theme` line is written after the file's block; the file still
    // wins for the tag.
    let src = theme_ws(
        "        styles {\n            element \"Element\" {\n                background #222222\n            }\n        }\n        theme t.json",
    );
    let mut loader = MemLoader::with(&[(
        "/project/t.json",
        &element_theme("Element", "\"background\": \"#111111\", \"color\": \"#333333\""),
    )]);
    let m = parse_files(&src, &mut loader).expect("parses");
    let st = m.style_of(sys(&m));
    assert_eq!(st.background, rgb(0x22, 0x22, 0x22));
    assert_eq!(st.color, rgb(0x33, 0x33, 0x33));
    // Theme styles come first in `element_styles`.
    assert_eq!(m.element_styles[0].theme, Some(0));
    assert_eq!(m.element_styles[1].theme, None);
    assert_eq!(m.styles_for(sys(&m)), [0, 1]);
}

/// For one tag a later theme beats an earlier one.
#[test]
fn p2_13_later_theme_wins() {
    let src = theme_ws("        themes a.json b.json");
    let mut loader = MemLoader::with(&[
        ("/project/a.json", &element_theme("Element", "\"background\": \"#aaaaaa\", \"stroke\": \"#010101\"")),
        ("/project/b.json", &element_theme("Element", "\"background\": \"#bbbbbb\"")),
    ]);
    let m = parse_files(&src, &mut loader).expect("parses");
    let st = m.style_of(sys(&m));
    assert_eq!(st.background, rgb(0xbb, 0xbb, 0xbb));
    assert_eq!(st.stroke, Some(rgb(1, 1, 1)));
}

/// Across tags the DSL-15 tag order decides, whether a style comes from a
/// theme or the file.
#[test]
fn p2_13_tag_order_across_theme_and_file() {
    let src = theme_ws(
        "        theme t.json\n        styles {\n            element \"Element\" {\n                background #222222\n            }\n            element \"Unrelated\" {\n                background #444444\n            }\n        }",
    );
    let mut loader = MemLoader::with(&[(
        "/project/t.json",
        &element_theme("Software System", "\"background\": \"#111111\""),
    )]);
    let m = parse_files(&src, &mut loader).expect("parses");
    // `Software System` comes after `Element` in the tag order, so the
    // theme's block wins although the file sets `Element`.
    assert_eq!(m.style_of(sys(&m)).background, rgb(0x11, 0x11, 0x11));
}

/// A local theme JSON file (`elements`/`relationships` arrays with `tag`)
/// read through the `Loader` gives styles with `theme: Some(i)`.
#[test]
fn p2_427_theme_json_read() {
    let json = br##"{
      "name": "Roles",
      "elements": [
        {"tag": "Tagged", "background": "#102030", "color": "white", "stroke": "#405060",
         "strokeWidth": 3, "shape": "RoundedBox", "width": 300, "height": 175, "fontSize": 18,
         "border": "Dashed", "opacity": 60, "metadata": false, "description": false}
      ],
      "relationships": [
        {"tag": "Relationship", "thickness": 3, "color": "#707070", "style": "Dotted",
         "routing": "Curved", "jump": true, "fontSize": 12, "width": 150, "position": 40,
         "opacity": 80},
        {"tag": "Async", "dashed": true}
      ]
    }"##;
    let src = theme_ws("        theme roles.json");
    let mut loader = MemLoader::with(&[("/project/roles.json", json)]);
    let m = parse_files(&src, &mut loader).expect("parses");
    let es = &m.element_styles[0];
    assert_eq!(es.theme, Some(0));
    assert_eq!(es.tag.value, "Tagged");
    assert_eq!(es.span, m.themes[0].span);
    assert_eq!(es.background.unwrap().span, m.themes[0].span);
    let st = m.style_of(sys(&m));
    assert_eq!(st.background, rgb(0x10, 0x20, 0x30));
    assert_eq!(st.color, rgb(0xff, 0xff, 0xff));
    assert_eq!(st.stroke, Some(rgb(0x40, 0x50, 0x60)));
    assert_eq!(st.stroke_width, 3);
    assert_eq!(st.shape, Shape::RoundedBox);
    assert_eq!((st.width, st.height), (Some(300), Some(180)));
    assert_eq!(st.font_size, Some(18));
    assert_eq!(st.border, Border::Dashed);
    assert_eq!(st.opacity, 60);
    assert!(!st.metadata && !st.description);
    assert_eq!(m.relationship_styles.len(), 2);
    assert!(m.relationship_styles.iter().all(|r| r.theme == Some(0)));
    assert_eq!(m.relationship_styles[1].style.unwrap().value, Border::Dashed);
    let rs = m.relationship_style_of(RelationshipId(0));
    assert_eq!(rs.thickness, 3);
    assert_eq!(rs.color, Some(rgb(0x70, 0x70, 0x70)));
    assert_eq!(rs.style, Border::Dotted);
    assert_eq!(rs.routing, Some(Routing::Curved));
    assert!(rs.jump);
    assert_eq!((rs.font_size, rs.width, rs.position, rs.opacity), (Some(12), Some(150), Some(40), 80));
}

/// Unknown JSON keys are ignored, and the theme file is never written.
#[test]
fn p2_427_unknown_keys_ignored() {
    let json = br##"{
      "name": "Roles", "description": "x", "logo": "logo.png",
      "font": {"name": "Open Sans", "url": "https://example.com/font.css"},
      "elements": [
        {"tag": "Tagged", "background": "#102030", "iconPosition": "Top",
         "properties": {"a": "b"}, "future": [1, 2.5, null, {"x": "\u00e9\n"}]}
      ],
      "relationships": [{"tag": "Relationship", "lineStyle": "x"}],
      "views": {}
    }"##;
    let src = theme_ws("        theme roles.json");
    let mut loader = MemLoader::with(&[("/project/roles.json", json)]);
    let m = parse_files(&src, &mut loader).expect("parses");
    assert_eq!(m.style_of(sys(&m)).background, rgb(0x10, 0x20, 0x30));
    assert_eq!(m.relationship_style_of(RelationshipId(0)), ResolvedRelationshipStyle::DEFAULT);
    // Only reads, and only the theme file (the `logo` is not fetched).
    assert_eq!(loader.reads, [PathBuf::from("/project/roles.json")]);
    assert_eq!(loader.files[Path::new("/project/roles.json")], json.to_vec());
}

/// A missing theme is `FileRead`, malformed JSON is `FileFormat`, each
/// naming the file and saying what to fix.
#[test]
fn p2_427_bad_theme_is_parse_error_naming_file() {
    let src = theme_ws("        theme missing.json");
    let e = parse_files(&src, &mut MemLoader::default()).unwrap_err();
    let ModelErrorKind::FileRead { path, .. } = &e.kind else {
        panic!("{e:?}");
    };
    assert_eq!(path, "missing.json");
    assert_eq!(slice(&src, e.span), "missing.json");
    let text = e.to_string();
    assert!(text.contains("missing.json") && text.contains(" — "), "{text}");

    for (bytes, what) in [
        (&b"{\"elements\": [ {\"tag\": \"x\"} "[..], "unterminated"),
        (b"[1, 2]", "not an object"),
        (b"{\"elements\": [{\"tag\": \"x\", \"background\": \"blurple\"}]}", "bad colour"),
        (b"{\"elements\": [{\"tag\": \"x\", \"shape\": \"Blob\"}]}", "bad shape"),
        (b"{\"elements\": [{\"background\": \"#ffffff\"}]}", "no tag"),
        (b"\xff\xfe", "not UTF-8"),
    ] {
        let src = theme_ws("        theme bad.json");
        let mut loader = MemLoader::with(&[("/project/bad.json", bytes)]);
        let e = parse_files(&src, &mut loader).unwrap_err();
        let ModelErrorKind::FileFormat { path, reason } = &e.kind else {
            panic!("{what}: {e:?}");
        };
        assert_eq!(path, "bad.json", "{what}");
        assert!(!reason.is_empty());
        let text = e.to_string();
        assert!(text.contains("bad.json") && text.contains(" — "), "{what}: {text}");
    }

    let src = theme_ws("        theme ../outside.json");
    let mut loader = MemLoader::with(&[("/outside.json", b"{}")]);
    let e = parse_files(&src, &mut loader).unwrap_err();
    assert!(matches!(e.kind, ModelErrorKind::PathOutsideRoot { .. }), "{e:?}");
    assert!(loader.reads.is_empty());

    // Without a file path (stdin, `parse`), a theme file cannot be found.
    let e = error(&theme_ws("        theme t.json"));
    assert!(matches!(e.kind, ModelErrorKind::NeedsFile { .. }), "{e:?}");
}

/// A relative `icon` in a theme resolves against the theme file's folder.
#[test]
fn p2_428_theme_icon_relative_to_theme() {
    let src = theme_ws("        theme themes/roles/theme.json");
    let mut loader = MemLoader::with(&[
        (
            "/project/themes/roles/theme.json",
            &element_theme("Tagged", "\"icon\": \"store.png\""),
        ),
        ("/project/themes/roles/store.png", PNG),
    ]);
    let m = parse_files(&src, &mut loader).expect("parses");
    let icon = m.element_styles[0].icon.as_ref().unwrap();
    assert_eq!(icon.value, "themes/roles/store.png");
    assert_eq!(m.assets.len(), 1);
    assert_eq!(m.assets[0].path, "themes/roles/store.png");
    assert_eq!(m.assets[0].mime, Mime::Png);
    assert_eq!(&*m.assets[0].bytes, PNG);
    assert_eq!(m.style_of(sys(&m)).icon.as_ref(), Some(&m.assets[0]));
    assert!(loader.reads.contains(&PathBuf::from("/project/themes/roles/store.png")));
    // A missing icon names its path.
    let mut loader = MemLoader::with(&[(
        "/project/themes/roles/theme.json",
        &element_theme("Tagged", "\"icon\": \"gone.png\""),
    )]);
    let e = parse_files(&src, &mut loader).unwrap_err();
    let ModelErrorKind::FileRead { path, .. } = &e.kind else {
        panic!("{e:?}");
    };
    assert_eq!(path, "themes/roles/gone.png");
}

/// A theme by name or URL draws nothing and adds a P2-3 notice naming it.
#[test]
fn p2_429_url_or_name_theme_notice() {
    // TODO(srs): P2-429. Reading taken (B styles-themes 11, approver C):
    // notice, no network; `theme default` draws only if its styles can be
    // bundled under their licence.
    let src = theme_ws("        theme https://example.com/themes/roles/theme.json");
    let m = parse(&src);
    assert!(m.element_styles.is_empty() && m.relationship_styles.is_empty());
    let notice = m
        .notices
        .iter()
        .find(|n| n.subject.contains("https://example.com/themes/roles/theme.json"))
        .expect("notice naming the URL");
    assert_eq!(notice.kind, NoticeKind::NoEffect);
    assert_eq!(slice(&src, notice.span.unwrap()), "theme https://example.com/themes/roles/theme.json");
    assert_eq!(notice.position.unwrap().line, 7);
    // `default` is bundled (Apache-2.0, structurizr/themes): it draws.
    let m = parse(&theme_ws("        theme default"));
    assert!(m.notices.is_empty());
    let st = m.style_of(sys(&m));
    assert_eq!(st.background, rgb(0x11, 0x68, 0xbd));
    assert_eq!(st.color, rgb(0xff, 0xff, 0xff));
    assert_eq!(st.shape, Shape::RoundedBox);
    assert!(m.element_styles.iter().all(|s| s.theme == Some(0)));
}

// ---- Plan and pipeline (styles-themes-06)

/// A resolved `width`/`height` replaces that dimension of the CORE-2 kind
/// default in the plan box.
#[test]
fn p2_402_tag_size_replaces_kind_default() {
    let m = parse(&plan_ws(
        "",
        "            element \"Tagged\" {\n                width 400\n            }\n            element \"Person\" {\n                height 300\n            }",
    ));
    let plan = view::plan(&m, &m.views[0]);
    let of = |key: &str| plan.boxes.iter().find(|b| b.element == m.find_written(key).unwrap()).unwrap();
    // Software system default: 240x140 = 12x7 half units.
    assert_eq!((of("sys").half_width, of("sys").half_height), (20, 7));
    // Person default: 200x180 = 10x9.
    assert_eq!((of("user").half_width, of("user").half_height), (10, 15));
    assert_eq!((of("peer").half_width, of("peer").half_height), (12, 7));
    assert_eq!(of("sys").style.width, Some(400));
}

/// A view's `size` line beats a style `width`/`height`.
#[test]
fn p2_402_size_line_wins() {
    let m = parse(&plan_ws(
        "            size sys 300 200",
        "            element \"Tagged\" {\n                width 400\n                height 500\n            }",
    ));
    let plan = view::plan(&m, &m.views[0]);
    let b = plan.boxes.iter().find(|b| b.element == sys(&m)).unwrap();
    assert_eq!((b.half_width, b.half_height), (15, 10));
}

/// `PlanEdge::style` is the winner's resolved relationship style.
#[test]
fn p2_12_edge_takes_winner_style() {
    let m = parse(&plan_ws(
        "",
        "            relationship \"Relationship\" {\n                thickness 3\n            }\n            relationship \"Async\" {\n                dashed true\n                color #aa0000\n            }",
    ));
    let plan = view::plan(&m, &m.views[0]);
    for edge in &plan.edges {
        assert_eq!(edge.style, m.relationship_style_of(edge.winner()));
    }
    let async_edge = plan
        .edges
        .iter()
        .find(|e| m.relationship(e.winner()).tags.iter().any(|t| t == "Async"))
        .unwrap();
    assert_eq!(async_edge.style.style, Border::Dashed);
    assert_eq!(async_edge.style.thickness, 3);
    assert_eq!(async_edge.style.color, Some(rgb(0xaa, 0, 0)));
    // A lifted line takes its winner's style too.
    let src = ws(
        "        user = person \"Operator\"\n        sys = softwareSystem \"Store\" {\n            app = container \"App\"\n        }\n        user -> app \"Uses\" \"\" \"Async\"",
        "        systemContext sys {\n            include *\n        }\n        styles {\n            relationship \"Async\" {\n                thickness 5\n            }\n        }",
    );
    let m = parse(&src);
    let plan = view::plan(&m, &m.views[0]);
    assert_eq!(plan.edges.len(), 1);
    assert_eq!(plan.edges[0].style.thickness, 5);
}

/// `view::plan_in(.., Mode::Dark)` resolves dark blocks and sets
/// `ViewPlan::mode`; `plan` equals `plan_in(.., Mode::Light)`.
#[test]
fn p2_430_plan_in_dark_mode() {
    let m = parse(&plan_ws(
        "",
        "            dark {\n                element \"Tagged\" {\n                    background #222222\n                }\n                relationship \"Relationship\" {\n                    color #cccccc\n                }\n            }",
    ));
    let view_spec = &m.views[0];
    let light = view::plan_in(&m, view_spec, Mode::Light);
    let dark = view::plan_in(&m, view_spec, Mode::Dark);
    assert_eq!(view::plan(&m, view_spec), light);
    assert_eq!(light.mode, Mode::Light);
    assert_eq!(dark.mode, Mode::Dark);
    let sys_box = |p: &view::ViewPlan| p.boxes.iter().find(|b| b.element == sys(&m)).unwrap().style.clone();
    assert_eq!(sys_box(&dark).background, rgb(0x22, 0x22, 0x22));
    // redesign 2026-11-19: unstyled in light mode, the plan draws the
    // house `dg-system` fill (was the resolved keyword default #dddddd).
    assert_eq!(sys_box(&light).background, rgb(0x1b, 0x5f, 0xd0));
    assert!(dark.edges.iter().all(|e| e.style.color == Some(rgb(0xcc, 0xcc, 0xcc))));
    assert!(light.edges.iter().all(|e| e.style.color.is_none()));
}

/// `pipeline::build_view_in(.., Mode::Dark, ..)` gives a scene with `mode
/// == Mode::Dark`; `build_view` equals light.
#[test]
fn p2_430_build_view_in_dark_mode() {
    let m = parse(&plan_ws(
        "",
        "            dark {\n                element \"Tagged\" {\n                    background #222222\n                }\n            }",
    ));
    let stop = AtomicBool::new(false);
    let keep = Keep::default();
    let dark = pipeline::build_view_in(&m, "ctx", &keep, Mode::Dark, &stop).unwrap();
    assert_eq!(dark.scene.mode, Mode::Dark);
    assert_eq!(dark.plan.mode, Mode::Dark);
    let light = pipeline::build_view_in(&m, "ctx", &keep, Mode::Light, &stop).unwrap();
    assert_eq!(pipeline::build_view(&m, "ctx", &keep, &stop).unwrap(), light);
    assert_eq!(light.scene.mode, Mode::Light);
    let spec = pipeline::build_spec_in(&m, &m.views[0], &keep, Mode::Dark, &stop).unwrap();
    assert_eq!(spec, dark);
    let all = pipeline::build_all_in(&m, Mode::Dark, &stop).unwrap();
    assert_eq!(all.views[0], dark);
    assert_eq!(pipeline::build_all_in(&m, Mode::Light, &stop).unwrap(), pipeline::build_all(&m, &stop).unwrap());
    assert!(matches!(
        pipeline::build_view_in(&m, "nope", &keep, Mode::Dark, &stop),
        Err(pipeline::BuildError::UnknownView(_))
    ));
    // A drag preview keeps the scene's mode.
    let moved = BTreeMap::new();
    let again = pipeline::rebuild_after_move(&m, &dark, &moved, &c4rizer_core::route::RouteConfig::DEFAULT, &stop).unwrap();
    assert_eq!(again.scene.mode, Mode::Dark);
}

// ---- Scene helpers (styles-themes-07 onwards)

/// The built view `key` of `m` in `mode`.
fn build_in(m: &Model, key: &str, mode: Mode) -> pipeline::ViewBuild {
    pipeline::build_view_in(m, key, &Keep::default(), mode, &AtomicBool::new(false))
        .unwrap_or_else(|e| panic!("{e:?}"))
}

/// The light scene of view `ctx` of `plan_ws("", styles)`, with its model.
fn ctx_scene(styles: &str) -> (Model, scene::Scene) {
    let m = parse(&plan_ws("", styles));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    (m, s)
}

/// The scene box drawing element `key`.
fn scene_box<'a>(m: &Model, s: &'a scene::Scene, key: &str) -> &'a scene::SceneBox {
    let e = m.find_written(key).expect("element");
    s.boxes.iter().find(|b| b.element == e).expect("box")
}

/// `element "Tagged" { lines }` as a `styles` body.
fn tagged(lines: &str) -> String {
    format!("            element \"Tagged\" {{\n{lines}\n            }}")
}

/// The scene line whose winner carries `tag`.
fn line_tagged<'a>(m: &Model, s: &'a scene::Scene, tag: &str) -> &'a scene::SceneLine {
    s.lines
        .iter()
        .find(|l| m.relationship(l.relationship).tags.iter().any(|t| t == tag))
        .expect("line")
}

// ---- Box properties in the scene (styles-themes-07)

/// `stroke` sets the box outline colour; with none it is the fill darkened
/// to 72%.
#[test]
fn p2_403_stroke_colour_and_default() {
    let (m, s) = ctx_scene(&tagged("                background #224466\n                stroke red"));
    assert_eq!(scene_box(&m, &s, "sys").stroke.colour, rgb(0xff, 0, 0));
    // redesign 2026-11-19: no stroke and no fill: the house outline
    // `dg-system-line` (was the #dddddd fill darkened to #9f9f9f). A
    // written fill without a stroke keeps the darkened outline.
    assert_eq!(scene_box(&m, &s, "peer").stroke.colour, rgb(0x1b, 0x5f, 0xd0));
    let (m, s) = ctx_scene(&tagged("                background #dddddd"));
    assert_eq!(scene_box(&m, &s, "sys").stroke.colour, rgb(0x9f, 0x9f, 0x9f));
}

/// `strokeWidth N` gives `SceneBox::stroke.width_px == N`; default 1.
#[test]
fn p2_404_stroke_width_drawn() {
    let (m, s) = ctx_scene(&tagged("                strokeWidth 4"));
    assert_eq!(scene_box(&m, &s, "sys").stroke.width_px, 4);
    assert_eq!(scene_box(&m, &s, "peer").stroke.width_px, 1);
}

/// `border dashed` and `dotted` give dash arrays, dotted shorter than
/// dashed; solid has none.
#[test]
fn p2_405_border_dash_patterns() {
    let dash = |border: &str| {
        let (m, s) = ctx_scene(&tagged(&format!("                border {border}")));
        scene_box(&m, &s, "sys").stroke.dash_px
    };
    assert_eq!(dash("solid"), None);
    let dashed = dash("dashed").expect("dashed");
    let dotted = dash("dotted").expect("dotted");
    assert!(dotted[0] < dashed[0], "{dotted:?} {dashed:?}");
    let (m, s) = ctx_scene("");
    assert_eq!(scene_box(&m, &s, "sys").stroke.dash_px, None);
}

/// `opacity N` gives `SceneBox::opacity == N`; default 100.
#[test]
fn p2_406_box_opacity() {
    let (m, s) = ctx_scene(&tagged("                opacity 40"));
    assert_eq!(scene_box(&m, &s, "sys").opacity, 40);
    assert_eq!(scene_box(&m, &s, "peer").opacity, 100);
}

/// `fontSize N` draws the box name at N px, and wrapping, L25 fit and
/// EDIT-18 minimum size use it.
#[test]
fn p2_407_font_size_name() {
    let (m, s) = ctx_scene(&tagged("                fontSize 24"));
    let b = scene_box(&m, &s, "sys");
    assert_eq!(b.name.size_px, 24);
    assert_eq!(scene_box(&m, &s, "peer").name.size_px, 14);
    // Wrapping uses it: a long name at 40px wraps. A name that overflows
    // the box with a larger size steps down to fit (polish-02), so the
    // fit test and EDIT-18's minimum are checked with one unbreakable word
    // that fits no size.
    let styles = format!("        styles {{\n{}\n        }}", tagged("                fontSize 40"));
    let ctx = "        systemContext sys \"ctx\" {\n            include *\n        }";
    let long = "        sys = softwareSystem \"Inventory Store Service\" \"\" \"Tagged\"";
    let big = parse(&ws(long, &format!("{ctx}\n{styles}")));
    let sb = build_in(&big, "ctx", Mode::Light);
    assert!(sb.scene.boxes[0].name.lines.len() > 1, "{:?}", sb.scene.boxes[0].name.lines);
    let word = "        sys = softwareSystem \"Supercalifragilisticexpialidocious_Inventory_Store\" \"\" \"Tagged\"";
    let big = parse(&ws(word, &format!("{ctx}\n{styles}")));
    let sb = build_in(&big, "ctx", Mode::Light);
    assert!(!sb.scene.boxes[0].name.fits);
    let pb_big = &sb.plan.boxes[0];
    assert!(!scene::texts_fit(pb_big, pb_big.half_width, pb_big.half_height));
    assert!(scene::texts_fit(pb_big, pb_big.half_width * 3, pb_big.half_height * 3));
}

/// Type line and description scale to round(N x 11/14) px on that size plus
/// 4; the name pitch is N + 6.
#[test]
fn p2_407_small_text_scaling() {
    // TODO(srs): P2-407. Reading taken (B styles-themes 5).
    let (m, s) = ctx_scene(&tagged("                fontSize 28"));
    let b = scene_box(&m, &s, "sys");
    assert_eq!((b.name.size_px, b.name.line_px), (28, 34));
    // round(28 x 11 / 14) = 22 on a 26px pitch.
    assert_eq!((b.type_line.size_px, b.type_line.line_px), (22, 26));
    // Unset: phase 1 (14 on 18, 11 on 14).
    let p = scene_box(&m, &s, "peer");
    assert_eq!((p.name.size_px, p.name.line_px, p.type_line.size_px, p.type_line.line_px), (14, 18, 11, 14));
}

/// `metadata false` drops the type line, `description false` the
/// description; the rest still fits.
#[test]
fn p2_408_hidden_metadata_and_description() {
    let src = ws(
        "        sys = softwareSystem \"Store\" \"Keeps the goods.\" \"Tagged\"",
        &format!(
            "        systemContext sys \"ctx\" {{\n            include *\n        }}\n        styles {{\n{}\n        }}",
            tagged("                metadata false\n                description false")
        ),
    );
    let m = parse(&src);
    let s = build_in(&m, "ctx", Mode::Light).scene;
    let b = &s.boxes[0];
    assert!(b.type_line.lines.is_empty(), "{:?}", b.type_line.lines);
    assert!(b.description.is_none());
    assert!(b.name.fits);
    assert!(export::svg(&s).matches("<text").count() >= 1);
    assert!(!export::svg(&s).contains("[Software System]"));
    // Only `metadata false`: the description stays and fits.
    let src = src.replace("                description false\n", "");
    let m = parse(&src);
    let s = build_in(&m, "ctx", Mode::Light).scene;
    let b = &s.boxes[0];
    assert!(b.type_line.lines.is_empty());
    assert!(b.description.as_ref().is_some_and(|d| d.fits));
}

// ---- Line properties in the scene (styles-themes-11)

/// K3,3: three systems `a<i>` each using three systems `b<j>`, so any
/// drawing has a crossing; relationship `k` (in declaration order) is
/// tagged `R<k>`; `styles` in the view set.
fn k33_ws(styles: &str) -> String {
    let mut model = String::new();
    for i in 0..3 {
        let _ = writeln!(model, "        a{i} = softwareSystem \"Source {i}\"\n        b{i} = softwareSystem \"Target {i}\"");
    }
    let mut k = 0;
    for i in 0..3 {
        for j in 0..3 {
            let _ = writeln!(model, "        a{i} -> b{j} \"Feeds\" \"\" \"R{k}\"");
            k += 1;
        }
    }
    ws(
        &model,
        &format!("        systemContext a0 \"ctx\" {{\n            include a0 a1 a2 b0 b1 b2\n        }}\n        styles {{\n{styles}\n        }}"),
    )
}

/// `relationship "<tag>" { jump true }` for each tag, as a `styles` body.
fn jumps_for(tags: &[String]) -> String {
    tags.iter()
        .map(|t| format!("            relationship \"{t}\" {{\n                jump true\n            }}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// True when `at` lies on the polyline `points`.
fn on_line(points: &[c4rizer_core::geom::Point], at: c4rizer_core::geom::Point) -> bool {
    points.windows(2).any(|w| {
        let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
        let (ex, ey) = (at.x - w[0].x, at.y - w[0].y);
        let r = c4rizer_core::geom::Rect::from_corners(
            c4rizer_core::geom::Point::new(w[0].x.min(w[1].x), w[0].y.min(w[1].y)),
            c4rizer_core::geom::Point::new(w[0].x.max(w[1].x), w[0].y.max(w[1].y)),
        );
        r.contains(at) && i64::from(dx) * i64::from(ey) == i64::from(dy) * i64::from(ex)
    })
}

/// The tag `R<k>` of a line of the K3,3 view.
fn r_tag(m: &Model, l: &scene::SceneLine) -> String {
    m.relationship(l.relationship).tags.iter().find(|t| t.starts_with('R') && t.len() == 2).unwrap().clone()
}

/// The first pair of K3,3 lines (earlier, later in paint order) that
/// properly cross somewhere, found with every line jumping.
fn crossing_pair() -> (String, String) {
    let all: Vec<String> = (0..9).map(|k| format!("R{k}")).collect();
    let m = parse(&k33_ws(&jumps_for(&all)));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    for (i, later) in s.lines.iter().enumerate() {
        for at in &later.jumps {
            if let Some(earlier) = s.lines[..i].iter().find(|e| on_line(&e.points, *at)) {
                return (r_tag(&m, earlier), r_tag(&m, later));
            }
        }
    }
    panic!("no hop at all");
}

/// `thickness N` draws the line and its chevrons N px wide; default 2.
#[test]
fn p2_418_thickness() {
    let m = parse(&plan_ws("", "            relationship \"Async\" {\n                thickness 5\n            }"));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    assert_eq!(line_tagged(&m, &s, "Async").stroke.width_px, 5);
    let plain = s.lines.iter().find(|l| !m.relationship(l.relationship).tags.iter().any(|t| t == "Async")).unwrap();
    assert_eq!(plain.stroke.width_px, 2);
}

/// The arrowhead and join dot scale by thickness/2, never below phase-1
/// size; lane pitch unchanged.
#[test]
fn p2_418_tip_scaling() {
    // TODO(srs): P2-418. Reading taken (B styles-themes 9).
    let arrow_len = |thickness: u32| {
        let (_, b) = long_label(&async_rel(&format!("                thickness {thickness}")));
        let line = &b.scene.lines[0];
        let scene::Tip::Arrow([tip, back1, back2]) = line.tip else { panic!("{:?}", line.tip) };
        let mid = c4rizer_core::geom::Point::new(i32::midpoint(back1.x, back2.x), i32::midpoint(back1.y, back2.y));
        ((tip.x - mid.x).abs().max((tip.y - mid.y).abs()), (back1.x - back2.x).abs().max((back1.y - back2.y).abs()), line.points.clone())
    };
    let (len1, w1, p1) = arrow_len(1);
    let (len2, w2, p2) = arrow_len(2);
    let (len6, w6, p6) = arrow_len(6);
    assert_eq!((len1, w1), (10, 10), "never below phase 1");
    assert_eq!((len2, w2), (10, 10));
    assert_eq!((len6, w6), (30, 30));
    // The lanes do not move.
    assert_eq!(p1, p2);
    assert_eq!(p2, p6);
}

/// A relationship colour gives line, chevrons, tip and label text that
/// colour instead of the destination fill.
#[test]
fn p2_419_colour_overrides_core_3() {
    let m = parse(&long_line_ws(&async_rel("                colour #aa0000")));
    let b = build_in(&m, "ctx", Mode::Light);
    let red = rgb(0xaa, 0, 0);
    assert_eq!(b.scene.lines[0].stroke.colour, red);
    assert_eq!(b.scene.labels[0].text.colour, red);
    let svg = export::svg(&b.scene);
    let line_g = svg.lines().find(|l| l.starts_with("<g class=\"line\">")).unwrap();
    assert!(!line_g.contains("#dddddd") && line_g.contains("#aa0000"), "{line_g}");
    // Without it: `dg-line` and a `dg-ink` label.
    // redesign 2026-11-19: was the destination's fill (CORE-3);
    // docs/deferred.md D-R2.
    let (_, b) = long_label("");
    assert_eq!(b.scene.lines[0].stroke.colour, rgb(0x4a, 0x5a, 0x70));
    assert_eq!(b.scene.labels[0].text.colour, rgb(0x14, 0x17, 0x1a));
}

/// `style dashed|dotted` gives the line's polyline a dash array.
#[test]
fn p2_420_line_dash_styles() {
    let dash = |style: &str| {
        let (_, b) = long_label(&async_rel(&format!("                style {style}")));
        b.scene.lines[0].stroke.dash_px
    };
    assert_eq!(dash("solid"), None);
    let dashed = dash("dashed").expect("dashed");
    let dotted = dash("dotted").expect("dotted");
    assert!(dotted[0] < dashed[0]);
    let (_, b) = long_label(&async_rel("                dashed true"));
    assert_eq!(b.scene.lines[0].stroke.dash_px, Some(dashed));
}

/// With no `style`, lines stay solid (CORE-10).
#[test]
fn p2_420_default_solid() {
    let (_, b) = long_label("");
    assert!(b.scene.lines.iter().all(|l| l.stroke.dash_px.is_none()));
    let (_, b) = long_label(&async_rel("                thickness 3"));
    assert!(b.scene.lines.iter().all(|l| l.stroke.dash_px.is_none()));
}

/// A `jump true` line has a `SceneLine::jumps` point at each crossing with
/// a line drawn before it, and with a later line that does not jump.
#[test]
fn p2_422_jumps_at_crossings() {
    // TODO(srs): P2-422. Reading taken (B styles-themes 8, checked against
    // the reference renderer's JointJS `jumpover`, Java wins): hop over
    // lines drawn before it and over later lines that do not jump;
    // 6px-radius half circle.
    let (earlier, later) = crossing_pair();
    // Only the later line jumps: it hops where it crosses the earlier one.
    let m = parse(&k33_ws(&jumps_for(std::slice::from_ref(&later))));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    assert!(s.crossings > 0);
    let hopper = s.lines.iter().find(|l| r_tag(&m, l) == later).unwrap();
    let crossed = s.lines.iter().find(|l| r_tag(&m, l) == earlier).unwrap();
    assert!(hopper.jumps.iter().any(|at| on_line(&crossed.points, *at)), "{:?}", hopper.jumps);
    for l in &s.lines {
        for at in &l.jumps {
            assert!(on_line(&l.points, *at));
            assert!(s.lines.iter().any(|e| e.edge != l.edge && on_line(&e.points, *at)), "{at:?} crosses no line");
        }
        if r_tag(&m, l) != later {
            assert!(l.jumps.is_empty());
        }
    }
    // Only the earlier line jumps: it hops over the later one too, which
    // does not jump.
    let m = parse(&k33_ws(&jumps_for(std::slice::from_ref(&earlier))));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    let early = s.lines.iter().find(|l| r_tag(&m, l) == earlier).unwrap();
    let late = s.lines.iter().find(|l| r_tag(&m, l) == later).unwrap();
    assert!(early.jumps.iter().any(|at| on_line(&late.points, *at)), "{:?}", early.jumps);
    assert!(late.jumps.is_empty());
    // No jump: no hops.
    let m = parse(&k33_ws(""));
    assert!(build_in(&m, "ctx", Mode::Light).scene.lines.iter().all(|l| l.jumps.is_empty()));
}

/// `jump` leaves `Scene::crossings` and L4 unchanged.
#[test]
fn p2_422_crossing_count_unchanged() {
    let all: Vec<String> = (0..9).map(|k| format!("R{k}")).collect();
    let plain = parse(&k33_ws(""));
    let jumped = parse(&k33_ws(&jumps_for(&all)));
    let (a, b) = (build_in(&plain, "ctx", Mode::Light), build_in(&jumped, "ctx", Mode::Light));
    assert!(b.scene.lines.iter().any(|l| !l.jumps.is_empty()));
    assert_eq!(a.scene.crossings, b.scene.crossings);
    assert_eq!(a.scene.crossing_pairs(), b.scene.crossing_pairs());
    assert_eq!(a.report, b.report);
}

/// When both crossing lines jump, only the later-drawn one hops.
#[test]
fn p2_422_both_jump() {
    // TODO(srs): P2-422. Reading taken (B styles-themes 8).
    let (earlier, later) = crossing_pair();
    let m = parse(&k33_ws(&jumps_for(&[earlier.clone(), later.clone()])));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    let early = s.lines.iter().find(|l| r_tag(&m, l) == earlier).unwrap();
    let late = s.lines.iter().find(|l| r_tag(&m, l) == later).unwrap();
    let shared: Vec<_> = late.jumps.iter().filter(|at| on_line(&early.points, **at)).collect();
    assert!(!shared.is_empty(), "the later line hops the earlier one");
    for at in shared {
        assert!(!early.jumps.contains(at), "both hop at {at:?}");
    }
}

/// A relationship `opacity N` gives `SceneLine::opacity` and
/// `SceneLabel::opacity` N.
#[test]
fn p2_426_line_opacity() {
    let (_, b) = long_label(&async_rel("                opacity 30"));
    assert_eq!(b.scene.lines[0].opacity, 30);
    assert_eq!(b.scene.labels[0].opacity, 30);
    let (_, b) = long_label("");
    assert_eq!((b.scene.lines[0].opacity, b.scene.labels[0].opacity), (100, 100));
}

// ---- Labels (styles-themes-10)

/// Two systems 800px apart joined by one long relationship tagged `Async`
/// with a two-line description; `styles` in the view set.
fn long_line_ws(styles: &str) -> String {
    ws(
        "        a = softwareSystem \"Front\"\n        b = softwareSystem \"Back\"\n        a -> b \"Sends the nightly batch of orders to\" \"HTTPS\" \"Async\"",
        &format!(
            "        systemContext a \"ctx\" {{\n            include *\n            at a 0 0\n            at b 800 0\n        }}\n        styles {{\n{styles}\n        }}"
        ),
    )
}

/// `relationship "Async" { lines }` as a `styles` body.
fn async_rel(lines: &str) -> String {
    format!("            relationship \"Async\" {{\n{lines}\n            }}")
}

/// The one label of `long_line_ws(styles)`, and its build.
fn long_label(styles: &str) -> (c4rizer_core::label::Label, pipeline::ViewBuild) {
    let m = parse(&long_line_ws(styles));
    let b = build_in(&m, "ctx", Mode::Light);
    (b.labels.labels[0].clone().expect("label"), b)
}

/// Distance along `points` (grid units) from the start to `at`, x10.
fn along(points: &[c4rizer_core::geom::Point], at: c4rizer_core::geom::Point) -> i32 {
    let mut d = 0;
    for w in points.windows(2) {
        let seg = c4rizer_core::geom::Rect::from_corners(
            c4rizer_core::geom::Point::new(w[0].x.min(w[1].x), w[0].y.min(w[1].y)),
            c4rizer_core::geom::Point::new(w[0].x.max(w[1].x), w[0].y.max(w[1].y)),
        );
        if seg.contains(at) {
            return d + (at.x - w[0].x).abs().max((at.y - w[0].y).abs());
        }
        d += (w[1].x - w[0].x).abs().max((w[1].y - w[0].y).abs());
    }
    panic!("{at:?} not on the line");
}

/// A relationship `fontSize N` gives `Label::font_px == N`, and label size,
/// clearances and L25 use it.
#[test]
fn p2_423_label_font_size() {
    let (plain, _) = long_label("");
    let (big, b) = long_label(&async_rel("                fontSize 20"));
    assert_eq!(plain.font_px, 11);
    assert_eq!(big.font_px, 20);
    assert!(big.width_px > plain.width_px && big.height_px > plain.height_px);
    let l = &b.scene.labels[0];
    assert_eq!(l.text.size_px, 20);
    assert!(l.text.fits, "{:?} in {:?}", l.text.bounds, l.rect);
    assert_eq!((l.rect.width(), l.rect.height()), (big.width_px, big.height_px));
}

/// `Label::line_px` is N + 2.
#[test]
fn p2_423_label_line_pitch() {
    // TODO(srs): P2-423. Reading taken (SRS proposal): N + 2.
    let (plain, _) = long_label("");
    let (big, b) = long_label(&async_rel("                fontSize 20"));
    assert_eq!(plain.line_px, 13);
    assert_eq!(big.line_px, 22);
    assert_eq!(b.scene.labels[0].text.line_px, 22);
    let n = i32::try_from(big.lines.len()).unwrap();
    assert_eq!(big.height_px, n * 22 + 2 * c4rizer_core::label::LABEL_PAD_Y_PX);
}

/// A relationship `width W` wraps the label to W px of text width (CORE-30
/// table).
#[test]
fn p2_424_label_wrapped_to_px_width() {
    let (plain, _) = long_label("");
    let (narrow, _) = long_label(&async_rel("                width 80"));
    assert!(narrow.lines.len() > plain.lines.len(), "{:?}", narrow.lines);
    let px = |t: &str| (c4rizer_core::label::text_advance(t, c4rizer_core::label::Weight::Regular) * 11 + c4rizer_core::label::EM - 1) / c4rizer_core::label::EM;
    // The technology line is not wrapped; every description line fits 80px
    // unless it is one word.
    for line in narrow.lines.iter().filter(|l| !l.starts_with('[')) {
        assert!(px(line) <= 80 || !line.contains(' '), "{line}: {}px", px(line));
    }
    assert_eq!(narrow.lines.last().map(String::as_str), Some("[HTTPS]"));
    // A wide width keeps the description on one line.
    let (wide, _) = long_label(&async_rel("                width 600"));
    assert_eq!(wide.lines.len(), 2, "{:?}", wide.lines);
}

/// `position P` orders candidate centres by distance from P% of the line's
/// length; CORE-14 gaps kept.
#[test]
fn p2_425_position_orders_candidates() {
    let at = |p: u8| {
        let (l, b) = long_label(&async_rel(&format!("                position {p}")));
        let line = b.routes.lines[0].as_ref().expect("routed");
        let total = along(&line.points, *line.points.last().unwrap());
        (along(&line.points, l.centre), total)
    };
    let (near, total) = at(20);
    let (far, _) = at(80);
    let (mid, _) = at(50);
    assert!(near < mid && mid < far, "{near} {mid} {far} of {total}");
    // Within a grid step of the asked spot, the CORE-14 gaps allowing.
    assert!((near * 100 - 20 * total).abs() <= 100, "{near} of {total}");
    assert!((far * 100 - 80 * total).abs() <= 100, "{far} of {total}");
    // Gaps kept even at the ends.
    let (start, _) = at(0);
    let (end, _) = at(100);
    assert!(start >= 3, "{start}");
    assert!(total - end >= 5, "{end} of {total}");
}

/// With no `position`, placement equals phase 1.
#[test]
fn p2_425_unset_position_is_phase_1() {
    let (unset, _) = long_label("");
    let (styled, _) = long_label(&async_rel("                thickness 4"));
    let (fifty, _) = long_label(&async_rel("                position 50"));
    assert_eq!(unset, styled);
    assert_eq!(unset.centre, fifty.centre);
    // The example views place exactly as before (their goldens hold).
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl")).unwrap();
    let m = parse(&src);
    for v in &m.views {
        let b = build_in(&m, &v.key, Mode::Light);
        for l in b.labels.labels.iter().flatten() {
            assert_eq!((l.font_px, l.line_px), (11, 13));
        }
    }
}

// ---- Boundaries and groups (styles-themes-12)

/// A system `sys` ("Store") with containers `app` and `db` (a component
/// `api` in `app`), a container view `c2` and a component view `c3`;
/// `styles` in the view set.
fn boundary_ws(styles: &str) -> String {
    ws(
        "        sys = softwareSystem \"Store\" {\n            app = container \"App\" {\n                api = component \"Api\"\n                core = component \"Core\"\n            }\n            db = container \"Db\"\n        }\n        app -> db \"Reads\"\n        api -> core \"Calls\"",
        &format!(
            "        container sys \"c2\" {{\n            include *\n        }}\n        component app \"c3\" {{\n            include *\n        }}\n        styles {{\n{styles}\n        }}"
        ),
    )
}

/// The one boundary of view `key` of `boundary_ws(styles)`.
fn boundary_of(styles: &str, key: &str) -> scene::SceneBoundary {
    let m = parse(&boundary_ws(styles));
    let s = build_in(&m, key, Mode::Light).scene;
    assert_eq!(s.boundaries.len(), 1);
    s.boundaries[0].clone()
}

/// `element "<tag>" { lines }` as a `styles` body.
fn element_style(tag: &str, lines: &str) -> String {
    format!("            element \"{tag}\" {{\n{lines}\n            }}")
}

/// `Boundary` then `Boundary:SoftwareSystem`/`Boundary:Container` styles
/// set fill, stroke, strokeWidth, color, fontSize, border and opacity of
/// the scope boundary.
#[test]
fn p2_411_boundary_tags_styled() {
    let styles = [
        element_style(
            "Boundary",
            "                background #eeeeee\n                stroke #112233\n                strokeWidth 3\n                color #445566\n                fontSize 16\n                border dotted\n                opacity 50",
        ),
        element_style("Boundary:SoftwareSystem", "                stroke #00ff00"),
        element_style("Boundary:Container", "                stroke #0000ff\n                border solid"),
    ]
    .join("\n");
    let b = boundary_of(&styles, "c2");
    assert_eq!(b.fill, Some(rgb(0xee, 0xee, 0xee)));
    assert_eq!(b.stroke.colour, rgb(0, 0xff, 0));
    assert_eq!(b.stroke.width_px, 3);
    let dotted = b.stroke.dash_px.expect("dotted");
    assert!(dotted[0] < 8);
    assert_eq!(b.opacity, 50);
    assert_eq!(b.tag.colour, rgb(0x44, 0x55, 0x66));
    assert_eq!(b.tag.size_px, 16);
    assert!(b.tag.fits, "{:?} in {:?}", b.tag.bounds, b.tag_rect);
    // A component view's boundary takes `Boundary:Container`.
    let b = boundary_of(&styles, "c3");
    assert_eq!(b.stroke.colour, rgb(0, 0, 0xff));
    assert_eq!(b.stroke.dash_px, None);
    assert_eq!(b.fill, Some(rgb(0xee, 0xee, 0xee)));
}

/// A boundary styled `border dashed` draws the design's boundary dash,
/// 6/4, like an unstyled one (redesign 2026-11-19).
#[test]
fn p2_411_boundary_styled_dashed_is_the_design_dash() {
    let styles = element_style("Boundary", "                border dashed");
    for key in ["c2", "c3"] {
        assert_eq!(boundary_of(&styles, key).stroke.dash_px, Some([6, 4]), "{key}");
    }
}

/// With no boundary style the boundary is the default one (dashed 6/4, 1px,
/// no fill).
#[test]
fn p2_411_boundary_default_unchanged() {
    for key in ["c2", "c3"] {
        let b = boundary_of("", key);
        assert_eq!(b.fill, None);
        assert_eq!(b.opacity, 100);
        // redesign 2026-11-19: outline `dg-boundary`, tag `dg-ink-2`
        // (was #666666 for both); dash, width and size unchanged.
        assert_eq!(b.stroke.colour, rgb(0x7b, 0x84, 0x8b));
        assert_eq!(b.stroke.width_px, 1);
        // owner decision 2026-11-19: design values over P2-411 / CORE-1
        // (`.dg-boundary` dash 6 4, was 8/6).
        assert_eq!(b.stroke.dash_px, Some([6, 4]));
        // redesign 2026-11-19: the tag is 11px (`.dg-boundary-t`), was
        // 12px.
        assert_eq!((b.tag.colour, b.tag.size_px), (rgb(0x5a, 0x62, 0x68), 11));
        assert!(b.tag.lines[0].ends_with(']'));
        assert!(b.icon.is_none());
    }
}

/// `metadata false` drops the bracketed type from the name tag.
#[test]
fn p2_411_boundary_metadata_false() {
    let b = boundary_of(&element_style("Boundary", "                metadata false"), "c2");
    assert_eq!(b.tag.lines, ["Store"]);
    let plain = boundary_of("", "c2");
    assert_eq!(plain.tag.lines, ["Store [Software System]"]);
    assert!(b.tag_rect.width() < plain.tag_rect.width());
}

/// `shape`, `width`, `height` and `description` on `Boundary*` styles
/// change nothing.
#[test]
fn p2_411_ignored_properties() {
    let styles = element_style(
        "Boundary",
        "                shape Hexagon\n                width 900\n                height 900\n                description false",
    );
    for key in ["c2", "c3"] {
        let m = parse(&boundary_ws(&styles));
        let plain = parse(&boundary_ws(""));
        assert_eq!(build_in(&m, key, Mode::Light).scene, build_in(&plain, key, Mode::Light).scene);
    }
}

/// `Group` then `Group:<name>` styles apply to group boundaries, `metadata`
/// excepted.
#[test]
fn p2_412_group_tags_styled() {
    let src = ws(
        "        sys = softwareSystem \"Store\" {\n            group \"Edge\" {\n                app = container \"App\"\n            }\n            db = container \"Db\"\n        }\n        app -> db \"Reads\"",
        &format!(
            "        container sys \"c2\" {{\n            include *\n        }}\n        styles {{\n{}\n{}\n        }}",
            element_style("Group", "                background #eeeeee\n                metadata false"),
            element_style("Group:Edge", "                stroke #00ff00")
        ),
    );
    let m = parse(&src);
    let s = build_in(&m, "c2", Mode::Light).scene;
    let group = s.boundaries.iter().find(|b| matches!(b.of, view::BoundaryOf::Group(_))).expect("group boundary");
    assert_eq!(group.fill, Some(rgb(0xee, 0xee, 0xee)));
    assert_eq!(group.stroke.colour, rgb(0, 0xff, 0));
}

/// A group boundary carries the tags `Group` and `Group:<full name>`, the
/// full name joining nested names with `structurizr.groupSeparator` (B
/// dsl-breadth 14c), and takes boundary styles from them in that order.
#[test]
fn p2_412_group_boundary_tags() {
    let src = ws(
        "        properties {\n            structurizr.groupSeparator /\n        }\n        sys = softwareSystem \"Store\" {\n            group \"Edge\" {\n                app = container \"App\"\n                group \"Inner\" {\n                    api = container \"Api\"\n                }\n            }\n            db = container \"Db\"\n        }\n        app -> db \"Reads\"\n        api -> db \"Writes\"",
        &format!(
            "        container sys \"c2\" {{\n            include *\n        }}\n        styles {{\n{}\n{}\n{}\n        }}",
            element_style("Group", "                background #eeeeee\n                stroke #111111"),
            element_style("Group:Edge/Inner", "                stroke #00ff00"),
            element_style("Inner", "                stroke #ff0000"),
        ),
    );
    let m = parse(&src);
    let s = build_in(&m, "c2", Mode::Light).scene;
    let groups: Vec<_> = s.boundaries.iter().filter(|b| matches!(b.of, view::BoundaryOf::Group(_))).collect();
    assert_eq!(groups.len(), 2);
    let named = |n: &str| {
        *groups
            .iter()
            .find(|b| b.tag.lines.first().is_some_and(|l| l == n))
            .unwrap_or_else(|| panic!("{n}"))
    };
    let (outer, inner) = (named("Edge"), named("Inner"));
    // `Group` styles both; `Group:Edge/Inner` (full name) only the inner;
    // a bare name is no group tag.
    assert_eq!((outer.fill, inner.fill), (Some(rgb(0xee, 0xee, 0xee)), Some(rgb(0xee, 0xee, 0xee))));
    assert_eq!(outer.stroke.colour, rgb(0x11, 0x11, 0x11));
    assert_eq!(inner.stroke.colour, rgb(0, 0xff, 0));
    // The plan holds exactly what `boundary_style_in` resolves for them.
    let plan = view::plan_in(&m, m.view("c2").expect("view"), Mode::Light);
    for b in plan.boundaries.iter().filter(|b| matches!(b.of, view::BoundaryOf::Group(_))) {
        let view::BoundaryOf::Group(g) = b.of else { unreachable!() };
        let full = format!("Group:{}", m.groups[g.0 as usize].full_name);
        assert_eq!(b.style, m.boundary_style_in(&["Group", &full], Mode::Light));
    }
}

/// `element "Boundary:Enterprise"` is accepted, changes no scene, and adds
/// a P2-3 notice naming it.
#[test]
fn p2_440_enterprise_boundary_no_effect() {
    let styles = element_style("Boundary:Enterprise", "                background #ff0000\n                stroke #00ff00");
    let m = parse(&boundary_ws(&styles));
    let plain = parse(&boundary_ws(""));
    for key in ["c2", "c3"] {
        assert_eq!(build_in(&m, key, Mode::Light).scene, build_in(&plain, key, Mode::Light).scene);
    }
    assert_eq!(m.notices.len(), 1, "{:?}", m.notices);
    assert_eq!(m.notices[0].kind, NoticeKind::NoEffect);
    assert!(m.notices[0].subject.contains("Boundary:Enterprise"), "{}", m.notices[0].subject);
    assert!(plain.notices.is_empty());
}

// ---- Key (styles-themes-13)

/// Three systems in a ring: `a -> b` tagged `Async,Batch`, `b -> c`
/// tagged `Async`, `c -> a` untagged; `styles` in the view set.
fn ring_ws(styles: &str) -> String {
    ws(
        "        a = softwareSystem \"Front\"\n        b = softwareSystem \"Back\" \"\" \"Blue\"\n        c = softwareSystem \"Store\"\n        a -> b \"Sends\" \"\" \"Async,Batch\"\n        b -> c \"Writes\" \"\" \"Async\"\n        c -> a \"Answers\"",
        &format!(
            "        systemContext a \"ctx\" {{\n            include a b c\n        }}\n        styles {{\n            element \"Blue\" {{\n                background #1168bd\n            }}\n{styles}\n        }}"
        ),
    )
}

/// The key's line entries of a scene: (names, stroke, dot).
fn line_entries(s: &scene::Scene) -> Vec<(Vec<String>, scene::Stroke, bool)> {
    s.legend
        .entries
        .iter()
        .filter_map(|e| match e.swatch {
            scene::Swatch::Line { stroke, tip, .. } => Some((e.names.clone(), stroke, matches!(tip, scene::Tip::Dot { .. }))),
            _ => None,
        })
        .collect()
}

/// The example file's model.
fn example() -> Model {
    parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl")).unwrap())
}

/// The key has one line entry per relationship-style tag (last styled
/// non-`Relationship` tag of each drawn winner), named by the tag.
#[test]
fn p2_27_one_entry_per_relationship_style_tag() {
    let styles = format!(
        "{}\n{}\n            relationship \"Relationship\" {{\n                thickness 3\n            }}",
        async_rel("                style dashed"),
        "            relationship \"Batch\" {\n                colour #aa0000\n            }"
    );
    let m = parse(&ring_ws(&styles));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    let names: Vec<Vec<String>> = line_entries(&s).into_iter().map(|(n, _, _)| n).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    for want in ["Batch", "Async", "Relationship"] {
        assert!(names.contains(&vec![want.to_owned()]), "{want} in {names:?}");
    }
    // An unstyled tag gives no entry of its own.
    let m = parse(&ring_ws(&async_rel("                style dashed")));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    let names: Vec<Vec<String>> = line_entries(&s).into_iter().map(|(n, _, _)| n).collect();
    assert_eq!(names, [vec!["Async".to_owned()], vec!["Relationship".to_owned()]]);
}

/// All other drawn lines share one entry named `Relationship`; a file with
/// no relationship styles has exactly one line entry.
#[test]
fn p2_27_unnamed_lines_single_relationship_entry() {
    // No relationship style: one entry, though the lines take two colours.
    let m = parse(&ring_ws(""));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    // redesign 2026-11-19: unstyled lines all take `dg-line` now (they
    // took two destination colours), still one entry.
    let colours: std::collections::BTreeSet<_> = s.lines.iter().map(|l| l.stroke.colour).collect();
    assert_eq!(colours.len(), 1);
    let entries = line_entries(&s);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].0, ["Relationship"]);
    // The example styles only `Relationship`: one arrow entry per view.
    let m = example();
    for key in ["C1", "C2"] {
        let s = build_in(&m, key, Mode::Light).scene;
        let entries = line_entries(&s);
        assert_eq!(entries.len(), 1, "{key}: {entries:?}");
        assert_eq!(entries[0].0, ["Relationship"]);
    }
}

/// Join-dot lines keep their own entry.
#[test]
fn p2_27_join_dot_entry() {
    // TODO(srs): P2-27. Reading taken (B styles-themes 12).
    let m = example();
    let mut seen_dot = false;
    for key in ["C3-serving", "C3-startup"] {
        let s = build_in(&m, key, Mode::Light).scene;
        let entries = line_entries(&s);
        let dots = s.lines.iter().any(|l| matches!(l.tip, scene::Tip::Dot { .. }));
        let arrows = s.lines.iter().any(|l| matches!(l.tip, scene::Tip::Arrow(_)));
        assert_eq!(entries.iter().filter(|e| e.2).count(), usize::from(dots), "{key}");
        assert_eq!(entries.iter().filter(|e| !e.2).count(), usize::from(arrows), "{key}");
        assert!(entries.iter().all(|e| e.0 == ["Relationship"]));
        seen_dot |= dots;
    }
    assert!(seen_dot, "the example has join dots");
}

/// A named entry's swatch uses that style's resolved colour, thickness and
/// dash.
#[test]
fn p2_434_named_swatch_stroke() {
    let m = parse(&ring_ws(&async_rel("                colour #aa0000\n                thickness 4\n                style dotted")));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    let (_, stroke, _) = line_entries(&s).into_iter().find(|(n, _, _)| n == &["Async"]).unwrap();
    let line = line_tagged(&m, &s, "Async");
    assert_eq!(stroke.colour, rgb(0xaa, 0, 0));
    assert_eq!(stroke.width_px, 4);
    assert_eq!(stroke.dash_px, line.stroke.dash_px);
    assert!(stroke.dash_px.is_some());
}

/// The `Relationship` entry's swatch is #707070 when its lines have
/// different colours.
#[test]
fn p2_434_unnamed_swatch_colour() {
    // TODO(srs): P2-434. Reading taken (B styles-themes 12): #707070.
    let m = parse(&ring_ws(""));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    // redesign 2026-11-19: the mixed swatch is `dg-line` (was #707070).
    assert_eq!(line_entries(&s)[0].1.colour, rgb(0x4a, 0x5a, 0x70));
    assert_eq!(line_entries(&s)[0].1.width_px, 2);
    // One colour among the lines: the swatch takes it.
    let m = parse(&long_line_ws(""));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    assert_eq!(line_entries(&s)[0].1.colour, s.lines[0].stroke.colour);
}

/// Boxes differing only in stroke colour, stroke width or border get
/// separate key entries; opacity and icon do not split.
#[test]
fn p2_435_box_look_includes_stroke_and_border() {
    // TODO(srs): P2-435. Reading taken (B styles-themes 12): opacity and
    // icons do not split entries.
    let entries = |a: &str, b: &str| {
        let src = ws(
            "        a = softwareSystem \"Front\" \"\" \"One\"\n        b = softwareSystem \"Back\" \"\" \"Two\"\n        a -> b \"Sends\"",
            &format!(
                "        systemContext a \"ctx\" {{\n            include *\n        }}\n        styles {{\n{}\n{}\n        }}",
                element_style("One", a),
                element_style("Two", b)
            ),
        );
        let m = parse(&src);
        let s = build_in(&m, "ctx", Mode::Light).scene;
        s.legend
            .entries
            .iter()
            .filter(|e| matches!(e.swatch, scene::Swatch::Box { .. }))
            .map(|e| e.names.clone())
            .collect::<Vec<_>>()
    };
    let same = "                background #123456";
    assert_eq!(entries(same, same), [vec!["One".to_owned(), "Two".to_owned()]]);
    for split in ["                stroke #ff0000", "                strokeWidth 3", "                border dashed"] {
        assert_eq!(entries(&format!("{same}\n{split}"), same).len(), 2, "{split}");
    }
    for keep in ["                opacity 50", "                icon data:image/png;base64,iVBORw0KGgo="] {
        assert_eq!(entries(&format!("{same}\n{keep}"), same).len(), 1, "{keep}");
    }
}

// ---- Icons (styles-themes-14)

/// `icon <file>` is read through the `Loader` relative to the diagram's
/// folder into `Model::assets` and `ResolvedStyle::icon`; a missing file is
/// `FileRead`.
#[test]
fn p2_409_icon_path_parsed_and_loaded() {
    // TODO(srs): P2-409. Readings taken (B styles-themes 7c, 7e): PNG, JPEG,
    // SVG files and `data:` URIs; missing file is a `FileRead` parse error.
    let src = theme_ws(&format!("        styles {{\n{}\n        }}", tagged("                icon ./icons/store.png")));
    let mut loader = MemLoader::with(&[("/project/icons/store.png", PNG)]);
    let m = parse_files(&src, &mut loader).unwrap();
    let st = m.style_of(sys(&m));
    let icon = st.icon.expect("icon");
    assert_eq!(icon.mime, Mime::Png);
    assert_eq!(&*icon.bytes, PNG);
    assert!(m.assets.iter().any(|a| a.path == "./icons/store.png"));
    assert!(loader.reads.iter().any(|p| p.ends_with("icons/store.png")));
    // The file keeps the path as written.
    assert_eq!(m.element_styles[0].icon.as_ref().unwrap().value, "./icons/store.png");
    // A missing file is a `FileRead` error at the icon.
    let mut empty = MemLoader::with(&[("/project/other.txt", b"x")]);
    let e = parse_files(&src, &mut empty).unwrap_err();
    assert!(matches!(e.kind, ModelErrorKind::FileRead { .. }), "{e:?}");
    assert_eq!(slice(&src, e.span), "./icons/store.png");
    // A path outside the diagram's folder is refused.
    let out = src.replace("./icons/store.png", "../secret.png");
    assert!(matches!(parse_files(&out, &mut loader).unwrap_err().kind, ModelErrorKind::PathOutsideRoot { .. }));
    // A base64 data URI is decoded with no file read.
    let data = src.replace("./icons/store.png", "data:image/png;base64,iVBORw0KGgpyZXN0");
    let m = parse(&data);
    let icon = m.style_of(sys(&m)).icon.expect("data icon");
    assert_eq!((icon.mime, &*icon.bytes), (Mime::Png, &b"\x89PNG\r\n\x1a\nrest"[..]));
    // A URL icon draws nothing and gets a notice naming it.
    let url = src.replace("./icons/store.png", "https://example.com/i.png");
    let m = parse(&url);
    assert!(m.style_of(sys(&m)).icon.is_none());
    assert!(m.notices.iter().any(|n| n.subject.contains("https://example.com/i.png")));
    // Without a loader a file icon is kept as written and draws nothing.
    let m = parse(&src);
    assert!(m.style_of(sys(&m)).icon.is_none());
    assert_eq!(m.element_styles[0].icon.as_ref().unwrap().value, "./icons/store.png");
}

/// `data:` URI icons as Structurizr reads them (B ruling 7c checked against
/// `ImageUtils.isSupportedDataUri`, Java wins): the three prefixes
/// `data:image/png;base64,`, `data:image/jpeg;base64,` and
/// `data:image/svg+xml;` (an SVG need not be base64); no other type; the
/// payload is not checked against the declared type.
#[test]
fn p2_409_data_uri_as_reference() {
    let with = |icon: &str| element_block("Tagged", &format!("                icon {icon}"));
    // An SVG as text, quoted or percent-encoded.
    for icon in [
        "\"data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'/>\"",
        "data:image/svg+xml;charset=utf-8,%3Csvg%20xmlns%3D%27http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%27%2F%3E",
    ] {
        let m = parse(&with(icon));
        let got = m.style_of(sys(&m)).icon.unwrap_or_else(|| panic!("{icon}"));
        assert_eq!(got.mime, Mime::Svg, "{icon}");
        assert!(std::str::from_utf8(&got.bytes).unwrap().starts_with("<svg xmlns="), "{icon}");
    }
    // `image/jpg` is not one of the three.
    let e = error(&with(&format!("data:image/jpg;base64,{PNG_RED_4X4}")));
    assert!(matches!(e.kind, ModelErrorKind::FileFormat { .. }), "{e:?}");
    // A payload that is not the declared type is still taken.
    let m = parse(&with(&format!("data:image/jpeg;base64,{PNG_RED_4X4}")));
    assert!(m.style_of(sys(&m)).icon.is_some());
}

/// A 1x1 PNG, base64.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
/// A small SVG, base64 of `<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>`.
const SVG_ICON: &str = "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI0IiBoZWlnaHQ9IjQiPjxyZWN0IHdpZHRoPSI0IiBoZWlnaHQ9IjQiIGZpbGw9InJlZCIvPjwvc3ZnPg==";

/// `ctx` with `sys` styled with `icon` and a description; model and scene.
fn icon_scene(icon: &str, shape: &str) -> (Model, scene::Scene) {
    let src = ws(
        "        sys = softwareSystem \"Store\" \"Keeps the goods for the shop.\" \"Tagged\"\n        peer = softwareSystem \"Peer\"\n        sys -> peer \"Sends\"",
        &format!(
            "        systemContext sys \"ctx\" {{\n            include *\n        }}\n        styles {{\n{}\n        }}",
            tagged(&format!("                icon {icon}\n                shape {shape}"))
        ),
    );
    let m = parse(&src);
    let s = build_in(&m, "ctx", Mode::Light).scene;
    (m, s)
}

/// `SceneBox::icon` lies centred in the bottom band, at most 1/4 of the box
/// height, clear of the texts.
#[test]
fn p2_409_icon_drawn_inside_box_clear_of_text() {
    // TODO(srs): P2-409. Reading taken (B styles-themes 7a).
    for (_, word) in SHAPES {
        let (m, s) = icon_scene(&format!("data:image/png;base64,{PNG_1X1}"), word);
        let b = scene_box(&m, &s, "sys");
        let icon = b.icon.as_ref().unwrap_or_else(|| panic!("{word}: no icon"));
        let r = icon.rect;
        assert!(b.rect.contains(r.min) && b.rect.contains(r.max), "{word}");
        assert!(r.height() <= b.rect.height() / 4 && r.height() > 0, "{word}: {r:?}");
        assert!((r.centre().x - b.rect.centre().x).abs() <= 1, "{word}: not centred");
        assert!(r.centre().y > b.rect.centre().y, "{word}: not in the bottom band");
        assert!(in_text_part(&b.outline, r.min) && in_text_part(&b.outline, r.max), "{word}: outside the outline");
        // Texts that fit stay clear of the icon; texts too big for a small
        // shape overflow and are L25's to report.
        for t in [Some(&b.name), Some(&b.type_line), b.description.as_ref()].into_iter().flatten() {
            assert!(!t.fits || !t.bounds.overlaps(&r), "{word}: {:?} over the icon", t.lines);
            if word == "Box" {
                assert!(t.fits, "{:?}", t.lines);
            }
        }
        assert_eq!(icon.asset.mime, Mime::Png);
    }
    // No icon, no `SceneIcon`.
    let (m, s) = ctx_scene("");
    assert!(scene_box(&m, &s, "sys").icon.is_none());
}

/// The SVG draws the icon as `<image href="data:...">` and the PNG
/// rasterizes it; EXP-1 scan allows only `data:` URIs.
#[test]
fn p2_409_icon_in_svg_and_png() {
    // TODO(srs): P2-409. Reading taken (B styles-themes 7d): a URL icon is
    // not drawn and gets a P2-3 notice.
    for (mime, data) in [("image/png", PNG_1X1), ("image/svg+xml", SVG_ICON)] {
        let (_, s) = icon_scene(&format!("data:{mime};base64,{data}"), "Box");
        let svg = export::svg(&s);
        assert!(svg.contains("<image x="), "{svg}");
        assert!(svg.contains(&format!("href=\"data:{mime};base64,{data}\"")), "{mime}");
        assert_eq!(svg.matches("href=").count(), 1);
        // The PNG is the SVG rasterized (`png` needs the feature, which the
        // workspace build turns on through the editor).
        #[cfg(feature = "png")]
        assert!(export::png(&s, 400).expect("png").starts_with(b"\x89PNG"));
    }
    // A URL icon is not drawn.
    let (_, s) = icon_scene("https://example.com/i.png", "Box");
    assert!(s.boxes.iter().all(|b| b.icon.is_none()));
    assert!(!export::svg(&s).contains("<image"));
}

/// An opaque red 4x4 PNG.
const PNG_RED_4X4: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAEElEQVR4nGP4z8AARwzEcQCukw/x0F8jngAAAABJRU5ErkJggg==";

/// The PNG draws a PNG icon's pixels, as the SVG and the canvas do: the
/// same scene with the icon taken out rasterizes to other bytes.
#[cfg(feature = "png")]
#[test]
fn p2_409_png_draws_raster_icon() {
    let (_, s) = icon_scene(&format!("data:image/png;base64,{PNG_RED_4X4}"), "Box");
    assert!(s.boxes.iter().any(|b| b.icon.is_some()));
    let mut bare = s.clone();
    for b in &mut bare.boxes {
        b.icon = None;
    }
    assert_ne!(export::png(&s, 400).expect("png"), export::png(&bare, 400).expect("png"));
}

/// `export::rasterize_asset` turns a PNG, JPEG or SVG asset into straight
/// RGBA8 pixels of the asked size, the same image the SVG and PNG exports
/// embed; undecodable bytes give `None` (P2-409, canvas).
#[cfg(feature = "png")]
#[test]
fn p2_409_rasterize_asset_rgba() {
    let m = parse(&element_block("Tagged", &format!("                icon data:image/png;base64,{PNG_RED_4X4}")));
    let png = m.style_of(sys(&m)).icon.expect("png icon");
    let rgba = export::rasterize_asset(&png, 8, 8).expect("decodes");
    assert_eq!(rgba.len(), 8 * 8 * 4);
    // Red, opaque, in the middle of the image.
    let mid = (4 * 8 + 4) * 4;
    assert_eq!(&rgba[mid..mid + 4], &[255, 0, 0, 255]);
    let icon_of = |value: &str| {
        let m = parse(&element_block("Tagged", &format!("                icon \"{value}\"")));
        m.style_of(sys(&m)).icon.expect("icon")
    };
    let svg = icon_of(
        "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'><rect width='4' height='4' fill='#00ff00'/></svg>",
    );
    assert_eq!(svg.mime, Mime::Svg);
    let rgba = export::rasterize_asset(&svg, 6, 6).expect("decodes");
    assert_eq!(&rgba[(3 * 6 + 3) * 4..(3 * 6 + 3) * 4 + 4], &[0, 255, 0, 255]);
    let broken = icon_of("data:image/png;base64,iVBORw0KGgpub3RpbWFnZQ==");
    assert!(export::rasterize_asset(&broken, 8, 8).is_none());
    assert!(export::rasterize_asset(&png, 0, 8).is_none());
}

// ---- SVG and PNG (styles-themes-15)

/// The `<g class="<class>" ...>` lines of an SVG.
fn groups<'a>(svg: &'a str, class: &str) -> Vec<&'a str> {
    let open = format!("<g class=\"{class}\"");
    svg.lines().filter(|l| l.starts_with(&open)).collect()
}

/// The SVG of a fixture using every element property draws each one.
#[test]
fn p2_10_svg_draws_every_property() {
    let src = ws(
        "        sys = softwareSystem \"Store\" \"Keeps the goods.\" \"Tagged\"\n        peer = softwareSystem \"Peer\"\n        sys -> peer \"Sends\"",
        &format!(
            "        systemContext sys \"ctx\" {{\n            include *\n        }}\n        styles {{\n{}\n        }}",
            tagged(&format!(
                "                shape Hexagon\n                icon data:image/png;base64,{PNG_1X1}\n                width 400\n                height 300\n                background #224466\n                colour #fafafa\n                stroke #abcdef\n                strokeWidth 3\n                fontSize 20\n                border dashed\n                opacity 60\n                metadata false\n                description true\n                properties {{\n                    k v\n                }}"
            ))
        ),
    );
    let m = parse(&src);
    let s = build_in(&m, "ctx", Mode::Light).scene;
    let svg = export::svg(&s);
    let b = scene_box(&m, &s, "sys");
    let g = groups(&svg, "box").into_iter().find(|g| g.contains("Store")).expect("box group");
    assert!(g.contains("fill=\"#224466\""), "{g}");
    assert!(g.contains("stroke=\"#abcdef\"") && g.contains("stroke-width=\"3\""), "{g}");
    assert!(g.contains("stroke-dasharray=\"8 6\""), "{g}");
    assert!(g.contains("opacity=\"0.6\""), "{g}");
    assert!(g.contains("font-size=\"20\"") && g.contains("fill=\"#fafafa\""), "{g}");
    assert!(g.contains("<image") && g.contains("Keeps the goods."), "{g}");
    assert!(!g.contains("[Software System]"), "{g}");
    assert_eq!((b.rect.width(), b.rect.height()), (400, 300));
}

/// The SVG draws each shape's outline as the scene gives it.
#[test]
fn p2_11_svg_outline_per_shape() {
    let m = parse(&every_shape_ws());
    let s = build_in(&m, "all", Mode::Light).scene;
    let svg = export::svg(&s);
    let boxes = groups(&svg, "box");
    assert_eq!(boxes.len(), 19);
    let mut outlines = std::collections::BTreeSet::new();
    for (b, g) in s.boxes.iter().zip(&boxes) {
        // The outline part: everything before the first text, with the
        // box's own position taken out.
        let shape_part = &g[..g.find("<text").unwrap()];
        let normal: String = shape_part.chars().filter(|c| !c.is_ascii_digit()).collect();
        let want = match b.shape {
            Shape::Circle | Shape::Ellipse => "<ellipse",
            Shape::Person => "<circle",
            Shape::Hexagon
            | Shape::Diamond
            | Shape::Cylinder
            | Shape::Bucket
            | Shape::Pipe
            | Shape::WebBrowser
            | Shape::Window
            | Shape::Terminal
            | Shape::Shell => "<path",
            _ => "<rect",
        };
        assert!(shape_part.contains(want), "{:?}: {shape_part}", b.shape);
        outlines.insert((normal, b.shape == Shape::RoundedBox || b.shape == Shape::Box, b.shape == Shape::Circle || b.shape == Shape::Ellipse));
    }
    // Box/RoundedBox, Circle/Ellipse and the two mobile devices differ
    // only in numbers; every other shape draws differently.
    assert_eq!(outlines.len(), 16, "{outlines:#?}");
    // Key swatches draw the same kinds.
    assert_eq!(groups(&svg, "key").iter().filter(|g| g.contains("<ellipse")).count(), 2);
}

/// The numeric attributes `names` of every `<tag ...>` element in `svg`,
/// in order.
fn svg_nums(svg: &str, tag: &str, names: &[&str]) -> Vec<Vec<i32>> {
    let open = format!("<{tag} ");
    svg.split(&open)
        .skip(1)
        .map(|el| {
            let el = format!(" {}", &el[..el.find('>').unwrap()]);
            names
                .iter()
                .map(|n| {
                    let key = format!(" {n}=\"");
                    let at = el.find(&key).unwrap_or_else(|| panic!("{n} in {el}")) + key.len();
                    let rest = &el[at..];
                    rest[..rest.find('"').unwrap()].parse().unwrap()
                })
                .collect()
        })
        .collect()
}

/// A portrait and a landscape phone in the same box draw different
/// pictures: the portrait's bezel is deeper at top and bottom, the
/// landscape's at the sides.
#[test]
fn p2_11_svg_mobile_devices_differ() {
    let m = parse(&every_shape_ws());
    let s = build_in(&m, "all", Mode::Light).scene;
    let svg = export::svg(&s);
    let boxes = groups(&svg, "box");
    // (left, top, right, bottom) bezel of each phone's screen.
    let bezel = |shape: Shape| {
        let (_, g) = s.boxes.iter().zip(&boxes).find(|(b, _)| b.shape == shape).unwrap();
        let rects = svg_nums(g, "rect", &["x", "y", "width", "height"]);
        let (body, screen) = (&rects[0], &rects[1]);
        (
            screen[0] - body[0],
            screen[1] - body[1],
            (body[0] + body[2]) - (screen[0] + screen[2]),
            (body[1] + body[3]) - (screen[1] + screen[3]),
        )
    };
    let (pl, pt, pr, pb) = bezel(Shape::MobileDevicePortrait);
    let (ll, lt, lr, lb) = bezel(Shape::MobileDeviceLandscape);
    assert!(pt > pl && pb > pr, "portrait {pl} {pt} {pr} {pb}");
    assert!(ll > lt && lr > lb, "landscape {ll} {lt} {lr} {lb}");
}

/// Terminal and Shell differ by more than the title bar edge: the terminal
/// shows a cursor after its prompt.
#[test]
fn p2_11_svg_terminal_and_shell_differ() {
    let m = parse(&every_shape_ws());
    let s = build_in(&m, "all", Mode::Light).scene;
    let svg = export::svg(&s);
    let boxes = groups(&svg, "box");
    let paths = |shape: Shape| {
        let (_, g) = s.boxes.iter().zip(&boxes).find(|(b, _)| b.shape == shape).unwrap();
        g[..g.find("<text").unwrap()].matches("<path").count()
    };
    assert!(paths(Shape::Terminal) >= paths(Shape::Shell) + 2, "{} {}", paths(Shape::Terminal), paths(Shape::Shell));
}

/// A box at opacity 100 has no opacity attribute; below 100 it has one.
#[test]
fn p2_406_opacity_attribute_only_below_100() {
    let (_, s) = ctx_scene(&tagged("                opacity 40"));
    let svg = export::svg(&s);
    let boxes = groups(&svg, "box");
    assert_eq!(boxes.iter().filter(|g| g.starts_with("<g class=\"box\" opacity=\"0.4\">")).count(), 1, "{svg}");
    assert_eq!(boxes.iter().filter(|g| g.starts_with("<g class=\"box\">")).count(), boxes.len() - 1);
    let (_, s) = ctx_scene(&tagged("                opacity 100"));
    assert!(!export::svg(&s).contains("opacity="));
    let (_, s) = ctx_scene(&tagged("                opacity 5"));
    assert!(export::svg(&s).contains("opacity=\"0.05\""));
}

/// A line at opacity 100 has no opacity attribute; below 100 it has one.
#[test]
fn p2_426_line_opacity_attribute_only_below_100() {
    let (_, b) = long_label(&async_rel("                opacity 25"));
    let svg = export::svg(&b.scene);
    assert_eq!(groups(&svg, "line"), groups(&svg, "line").iter().filter(|g| g.starts_with("<g class=\"line\" opacity=\"0.25\">")).copied().collect::<Vec<_>>());
    assert!(groups(&svg, "label")[0].starts_with("<g class=\"label\" opacity=\"0.25\">"));
    let (_, b) = long_label("");
    assert!(!export::svg(&b.scene).contains("opacity="));
}

/// The SVG draws a hop at each `SceneLine::jumps` point.
#[test]
fn p2_422_svg_jump_hops() {
    let all: Vec<String> = (0..9).map(|k| format!("R{k}")).collect();
    let m = parse(&k33_ws(&jumps_for(&all)));
    let s = build_in(&m, "ctx", Mode::Light).scene;
    let svg = export::svg(&s);
    let lines = groups(&svg, "line");
    let mut hops = 0;
    for (l, g) in s.lines.iter().zip(&lines) {
        let route = &g[..g.find("/>").unwrap()];
        // redesign 2026-11-19: path numbers are space-separated now.
        assert_eq!(route.matches(" A6 6 ").count(), l.jumps.len(), "{route}");
        hops += l.jumps.len();
    }
    assert!(hops > 0);
    // No jump, no arcs in any line.
    let m = parse(&k33_ws(""));
    let svg = export::svg(&build_in(&m, "ctx", Mode::Light).scene);
    assert!(groups(&svg, "line").iter().all(|g| !g.contains(" A6 6 ")));
}

/// The PNG of the all-styles fixture is its SVG rasterized (EXP-2).
#[test]
fn p2_436_png_is_svg_rasterized() {
    let m = parse(&every_shape_ws());
    let s = build_in(&m, "all", Mode::Light).scene;
    let svg = export::svg(&s);
    assert!(svg.starts_with("<svg") && svg.ends_with("</svg>\n"));
    #[cfg(feature = "png")]
    {
        let png = export::png(&s, 800).expect("png");
        assert!(png.starts_with(b"\x89PNG"));
        // Width as asked (IHDR bytes 16..20) and the same bytes twice.
        assert_eq!(u32::from_be_bytes([png[16], png[17], png[18], png[19]]), 800);
        assert_eq!(export::png(&s, 800).expect("png"), png);
    }
}

// ---- Edits (styles-themes-16)

/// A DSL-13 shape edit writes the reference spelling (`shape
/// MobileDevicePortrait`).
#[test]
fn p2_417_shape_written_in_reference_spelling() {
    for (shape, word) in SHAPES {
        let src = element_block("Tagged", "                shape box");
        let after = edited(
            &src,
            &[set_style("sys", StyleTarget::Tag("Tagged".into()), StyleProperty::Shape(Some(shape)))],
        );
        assert_eq!(changed_lines(&src, &after), [format!("                shape {word}")]);
        let m = parse(&after);
        assert_eq!(m.style_of(sys(&m)).shape, shape);
        // A new line also uses the reference spelling.
        let src = element_block("Tagged", "                background #ffffff");
        let after = edited(
            &src,
            &[set_style("sys", StyleTarget::Tag("Tagged".into()), StyleProperty::Shape(Some(shape)))],
        );
        assert!(after.contains(&format!("\n                shape {word}\n")), "{after}");
    }
}

/// A DSL-13 style edit changes only the edited property's line.
#[test]
fn p2_439_edit_keeps_other_properties() {
    let src = element_block(
        "Tagged",
        "                shape Hexagon
                background #111111
                colour #222222
                stroke #333333
                strokeWidth 3
                border dotted
                opacity 70
                properties {
                    k v
                }",
    );
    let after = edited(
        &src,
        &[set_style("sys", StyleTarget::Tag("Tagged".into()), StyleProperty::Background(Some(rgb(0xab, 0xcd, 0xef))))],
    );
    assert_eq!(changed_lines(&src, &after), ["                background #abcdef"]);
    let st = parse(&after).style_of(ElementId(0));
    assert_eq!((st.shape, st.stroke_width, st.opacity), (Shape::Hexagon, 3, 70));
}

/// On a block using `colour`, a text-colour edit rewrites that line and
/// adds no `color` line.
#[test]
fn p2_439_colour_alias_rewritten_in_place() {
    let src = element_block("Tagged", "                colour #222222\n                background #111111");
    let after = edited(
        &src,
        &[set_style("sys", StyleTarget::Tag("Tagged".into()), StyleProperty::Color(Some(rgb(0x44, 0x44, 0x44))))],
    );
    assert_eq!(changed_lines(&src, &after), ["                colour #444444"]);
    assert!(!after.contains("color "), "{after}");
    // Clearing it removes the `colour` line.
    let cleared = edited(
        &src,
        &[set_style("sys", StyleTarget::Tag("Tagged".into()), StyleProperty::Color(None))],
    );
    assert!(!cleared.contains("colour"), "{cleared}");
    assert!(cleared.contains("background #111111"), "{cleared}");
}

/// An `all <tag>` edit on a tag styled only by a theme adds a block to the
/// file's own `styles`.
#[test]
fn p2_439_theme_styles_never_edited() {
    // `default` styles `Software System`; the file has no block for it.
    let src = ws(
        "        sys = softwareSystem \"Store\"",
        "        theme default\n        styles {\n            element \"Other\" {\n                background #010101\n            }\n        }",
    );
    let after = edited(
        &src,
        &[set_style(
            "sys",
            StyleTarget::Tag("Software System".into()),
            StyleProperty::Background(Some(rgb(0x12, 0x34, 0x56))),
        )],
    );
    assert!(after.contains("        theme default\n"), "{after}");
    assert!(after.contains("element \"Software System\" {"), "{after}");
    let m = parse(&after);
    let st = m.style_of(sys(&m));
    assert_eq!(st.background, rgb(0x12, 0x34, 0x56));
    // The theme's other properties still apply.
    assert_eq!(st.color, rgb(0xff, 0xff, 0xff));
    let file_blocks: Vec<_> = m.element_styles.iter().filter(|s| s.theme.is_none()).collect();
    assert_eq!(file_blocks.len(), 2);
    // Clearing a property only a theme sets writes nothing.
    let same = edited(
        &src,
        &[set_style("sys", StyleTarget::Tag("Software System".into()), StyleProperty::Color(None))],
    );
    assert_eq!(same, src);
}

// ---- End to end (styles-themes-19)

/// The all-styles fixture's folder.
const STYLES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/styles");

/// The all-styles fixture's text, and a loader serving its theme file at
/// `/project/theme/theme.json` (the fixture parses as `ROOT`).
fn all_styles() -> (String, MemLoader) {
    let text = std::fs::read_to_string(format!("{STYLES_DIR}/all_styles.dsl")).expect("fixture");
    let file = |p: &str| std::fs::read(format!("{STYLES_DIR}/{p}")).expect(p);
    let (theme, icon, svg) = (
        file("theme/theme.json"),
        file("theme/icons/partner.png"),
        file("icons/database.svg"),
    );
    (
        text,
        MemLoader::with(&[
            ("/project/theme/theme.json", &theme),
            ("/project/theme/icons/partner.png", &icon),
            ("/project/icons/database.svg", &svg),
        ]),
    )
}

/// `tests/data/styles/all_styles.dsl` (every element property, every shape,
/// every relationship property) builds and exports in both modes.
#[test]
fn p2_436_every_style_fixture_renders() {
    let (text, mut loader) = all_styles();
    let m = parse_files(&text, &mut loader).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(m.themes.len() == 2 && m.element_styles.iter().any(|s| s.theme == Some(1)));
    let stop = AtomicBool::new(false);
    for mode in [Mode::Light, Mode::Dark] {
        let all = pipeline::build_all_in(&m, mode, &stop).expect("not stopped");
        assert_eq!(all.views.len(), 2);
        for v in &all.views {
            assert_eq!(v.scene.mode, mode);
            assert!(v.scene.unroutable().is_empty(), "{}", v.plan.key);
            let svg = export::svg(&v.scene);
            assert!(svg.starts_with("<svg") && svg.ends_with("</svg>\n"));
            #[cfg(feature = "png")]
            assert!(export::png(&v.scene, 600).expect("png").starts_with(b"\x89PNG"));
        }
        let shapes: std::collections::BTreeSet<Shape> = all.views[0].scene.boxes.iter().map(|b| b.shape).collect();
        assert_eq!(shapes.len(), 19);
        let lines = &all.views[0].scene.lines;
        assert!(lines.iter().any(|l| l.stroke.dash_px.is_some()));
        assert!(lines.iter().any(|l| l.opacity < 100));
        assert!(lines.iter().any(|l| l.stroke.width_px == 3));
        let boxes = &all.views[0].scene.boxes;
        assert!(boxes.iter().any(|b| b.icon.is_some() && b.opacity == 90));
    }
}

/// A file with no phase-2 style construct gives the phase-1 scene except
/// the key's line entries; the goldens are regenerated once in styles-
/// themes-13 and re-approved.
#[test]
fn p2_437_no_phase_2_construct_same_scene() {
    // The example uses no phase-2 construct but `routing`, which changes no
    // scene (P2-421); its SVGs are the regenerated goldens, byte for byte.
    let m = example();
    let stop = AtomicBool::new(false);
    let all = pipeline::build_all(&m, &stop).expect("not stopped");
    for v in &all.views {
        let golden = std::fs::read_to_string(format!(
            "{}/../../tests/data/golden/{}.svg",
            env!("CARGO_MANIFEST_DIR"),
            v.plan.key
        ))
        .expect("golden");
        assert_eq!(export::svg(&v.scene), golden, "{}", v.plan.key);
        // Every box and line keeps its phase-1 look.
        assert!(v.scene.boxes.iter().all(|b| b.opacity == 100 && b.stroke.width_px == 1 && b.stroke.dash_px.is_none() && b.icon.is_none()));
        assert!(v.scene.lines.iter().all(|l| l.opacity == 100 && l.stroke.width_px == 2 && l.stroke.dash_px.is_none() && l.jumps.is_empty()));
        // owner decision 2026-11-19: design values over P2-411 / CORE-1
        assert!(v.scene.boundaries.iter().all(|b| b.fill.is_none() && b.stroke.dash_px == Some([6, 4])));
    }
    // Without the `routing` block the scenes are the same.
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl")).unwrap();
    let bare = text.replace("            relationship \"Relationship\" {\n                routing Orthogonal\n            }\n", "");
    assert_ne!(bare, text);
    let bare = pipeline::build_all(&parse(&bare), &stop).expect("not stopped");
    for (a, b) in all.views.iter().zip(&bare.views) {
        assert_eq!(a.scene, b.scene);
    }
}

/// A file using every construct of this row round-trips byte-identical
/// (DSL-11).
#[test]
fn p2_438_round_trip_all_styles() {
    let (text, mut loader) = all_styles();
    for construct in [
        "shape MobileDeviceLandscape", "icon data:", "width 300", "height 260", "background SteelBlue",
        "colour white", "color #ffffff", "stroke #123456", "strokeWidth 2", "fontSize 16", "border dotted",
        "opacity 90", "metadata true", "description true", "properties {", "thickness 3", "colour DarkRed",
        "style dashed", "routing Direct", "jump true", "width 120", "position 40", "dashed true", "light {",
        "dark {", "theme default", "themes theme/theme.json",
    ] {
        assert!(text.contains(construct), "{construct}");
    }
    let parsed = model::parse_with(&text, Some(Path::new(ROOT)), &mut loader).expect("parses");
    assert_eq!(parsed.trees[0].print(), text);
    for variant in [text.replace('\n', "\r\n"), text.trim_end().to_owned()] {
        let tree = c4rizer_core::text::parse(c4rizer_core::text::FileId::ROOT, &variant).expect("parses");
        assert_eq!(tree.print(), variant);
    }
}

/// Dark mode draws the redesign's dark diagram: paper #1e2124, unstyled
/// containers `dg-container` with `dg-on-container` text and a
/// `dg-container-line` outline, lines `dg-line`, boundary `dg-boundary`
/// with its tag in `dg-ink-2`.
// redesign 2026-11-19: the design tokens replace the P2-431 reading (B
// styles-themes 10c: #111111 page, #333333/#ffffff boxes, #aaaaaa lines
// and boundaries) and the light #dddddd default fill (docs/deferred.md
// D-R5). The behaviour pinned stays: defaults per mode, styled
// properties keep their value, resolved styles mode-independent.
#[test]
fn p2_431_dark_defaults() {
    let m = parse(&boundary_ws(&element_style("Db", "                background #00796b")));
    let light = build_in(&m, "c2", Mode::Light).scene;
    let dark = build_in(&m, "c2", Mode::Dark).scene;
    assert_eq!(light.background, rgb(0xff, 0xff, 0xff));
    assert_eq!(dark.background, rgb(0x1e, 0x21, 0x24));
    for b in &dark.boxes {
        assert_eq!(b.fill, rgb(0x6f, 0xb0, 0xf8));
        assert_eq!(b.name.colour, rgb(0x07, 0x1a, 0x36));
        assert_eq!(b.stroke.colour, rgb(0x9c, 0xc8, 0xfb));
        assert_eq!(b.stroke.width_px, 1);
    }
    assert!(light.boxes.iter().all(|b| b.fill == rgb(0x5a, 0xa0, 0xf5) && b.stroke.colour == rgb(0x2a, 0x72, 0xe0)));
    assert!(light.lines.iter().all(|l| l.stroke.colour == rgb(0x4a, 0x5a, 0x70)));
    assert!(dark.lines.iter().all(|l| l.stroke.colour == rgb(0x8f, 0xa0, 0xb8)));
    assert_eq!(dark.boundaries[0].stroke.colour, rgb(0x73, 0x7c, 0x83));
    assert_eq!(dark.boundaries[0].tag.colour, rgb(0x9a, 0xa2, 0xa8));
    // A styled property keeps its value in dark mode.
    let m = parse(&boundary_ws(&element_style("Container", "                background #00796b\n                color #000000")));
    let dark = build_in(&m, "c2", Mode::Dark).scene;
    assert!(dark.boxes.iter().all(|b| b.fill == rgb(0, 0x79, 0x6b) && b.name.colour == rgb(0, 0, 0)));
    // The resolved styles stay mode-independent (P2-430).
    let app = m.find_written("app").unwrap();
    assert_eq!(m.style_of_in(app, Mode::Dark), m.style_of_in(app, Mode::Light));
}

/// A root file that includes a file, names a theme file (with an icon)
/// and a file icon, as read by `parse_with` through one `MemLoader`.
fn loader_case() -> (String, MemLoader) {
    let root = "workspace {\n    model {\n        !include people.dsl\n        sys = softwareSystem \"Store\" \"\" \"Tagged\"\n        user -> sys \"Uses\"\n    }\n    views {\n        systemContext sys \"ctx\" {\n            include *\n        }\n        theme themes/theme.json\n        styles {\n            element \"Tagged\" {\n                icon icons/store.png\n            }\n        }\n    }\n}\n".to_owned();
    let theme = br##"{"elements": [{"tag": "Person", "background": "#123456", "icon": "person.png"}]}"##;
    let loader = MemLoader::with(&[
        ("/project/people.dsl", b"user = person \"Operator\"\n"),
        ("/project/themes/theme.json", theme),
        ("/project/themes/person.png", PNG),
        ("/project/icons/store.png", PNG),
    ]);
    (root, loader)
}

/// Themes and icons read through the same `Loader::read` as includes,
/// after every DSL file, so a loader that numbers its reads keeps
/// `read[i]` = `FileId(i + 1)` (P2-136, the CLI's and editor's mapping).
#[test]
fn p2_136_theme_and_icon_reads_keep_file_id_order() {
    let (root, mut loader) = loader_case();
    let parsed = model::parse_with(&root, Some(Path::new(ROOT)), &mut loader).expect("parses");
    assert_eq!(parsed.trees.len(), 2);
    let dsl_reads = parsed.trees.len() - 1;
    for (i, read) in loader.reads.iter().take(dsl_reads).enumerate() {
        assert_eq!(MemLoader::normal(read), MemLoader::normal(&parsed.paths[i + 1]), "read {i}");
    }
    let rest = &loader.reads[dsl_reads..];
    assert_eq!(rest.len(), 3, "{rest:?}");
    assert!(
        rest.iter().all(|p| p.extension().is_some_and(|e| e != "dsl")),
        "{rest:?}"
    );
    let m = &parsed.model;
    assert!(m.assets.iter().any(|a| a.path == "themes/person.png"));
    assert!(m.assets.iter().any(|a| a.path == "icons/store.png"));
}

/// `edit::apply` re-parses with no I/O: a file with a theme file and file
/// icons keeps them across an edit (P2-13, P2-409, P2-139).
#[test]
fn p2_13_edit_reparse_keeps_theme_and_icons() {
    let (root, mut loader) = loader_case();
    let parsed = model::parse_with(&root, Some(Path::new(ROOT)), &mut loader).expect("parses");
    let reads = loader.reads.len();
    let applied = edit::apply(
        &parsed,
        &[Edit::Move {
            view: "ctx".into(),
            element: ElementKey::from_written("sys"),
            centre: Some((400, 200)),
        }],
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(applied.text.contains("sys 400 200"), "{}", applied.text);
    assert_eq!(loader.reads.len(), reads, "reparse reads no file");
    // A style edit on the same file re-parses twice (edit, then check).
    let styled = edit::apply(
        &parsed,
        &[set_style(
            "sys",
            StyleTarget::Tag("Tagged".into()),
            StyleProperty::Background(Some(rgb(1, 2, 3))),
        )],
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(styled.text.contains("theme themes/theme.json"));
}

/// Icons Structurizr accepts but c4rizer cannot draw (a supported `data:`
/// prefix with malformed base64, a GIF, BMP or other image file) are never
/// a parse error: the file opens, the icon is not drawn, and a `NoEffect`
/// notice names it (approver ruling 2026-11-17).
#[test]
fn p2_409_undrawable_icon_notice() {
    let names = |m: &Model, icon: &str| {
        m.notices
            .iter()
            .any(|n| n.kind == NoticeKind::NoEffect && n.subject.contains(icon))
    };
    // Malformed base64 behind a supported prefix.
    let bad = "data:image/png;base64,@@not-base64@@";
    let m = parse(&element_block("Tagged", &format!("                icon {bad}")));
    assert!(m.style_of(sys(&m)).icon.is_none());
    assert!(names(&m, bad), "{:?}", m.notices);
    // GIF and BMP files, and an image file of no known content.
    for (file, bytes) in [
        ("icons/anim.gif", &b"GIF89a\x01\x00\x01\x00"[..]),
        ("icons/old.bmp", &b"BM\x3a\x00\x00\x00"[..]),
        ("icons/photo.webp", &b"RIFF\x00\x00\x00\x00WEBP"[..]),
    ] {
        let src = theme_ws(&format!("        styles {{\n{}\n        }}", tagged(&format!("                icon {file}"))));
        let full = format!("/project/{file}");
        let mut loader = MemLoader::with(&[(full.as_str(), bytes)]);
        let m = parse_files(&src, &mut loader).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(m.style_of(sys(&m)).icon.is_none(), "{file}");
        assert!(names(&m, file), "{file}: {:?}", m.notices);
    }
    // The same from a theme file, relative to the theme.
    let theme = element_theme("Tagged", "\"icon\": \"anim.gif\"");
    let src = theme_ws("        theme themes/theme.json");
    let mut loader = MemLoader::with(&[
        ("/project/themes/theme.json", &theme),
        ("/project/themes/anim.gif", b"GIF89a"),
    ]);
    let m = parse_files(&src, &mut loader).unwrap_or_else(|e| panic!("{e}"));
    assert!(m.style_of(sys(&m)).icon.is_none());
    assert!(names(&m, "themes/anim.gif"), "{:?}", m.notices);
    // A file whose name is not an image type stays an error, as in
    // Structurizr.
    let src = theme_ws(&format!("        styles {{\n{}\n        }}", tagged("                icon notes.txt")));
    let mut loader = MemLoader::with(&[("/project/notes.txt", b"text")]);
    let e = parse_files(&src, &mut loader).unwrap_err();
    assert!(matches!(e.kind, ModelErrorKind::FileFormat { .. }), "{e:?}");
}

// ---- Dark-mode legibility (approver ruling 2026-11-17)

/// A label backing inside a boundary takes that boundary's fill when it
/// has one, else the page colour, in both modes.
#[test]
fn p2_431_label_backing_takes_boundary_fill() {
    let src = ws(
        "        user = person \"Operator\"\n        sys = softwareSystem \"Store\" {\n            app = container \"App\"\n            db = container \"Db\"\n        }\n        user -> app \"Uses\"\n        app -> db \"Reads\"",
        &format!(
            "        container sys \"c2\" {{\n            include *\n        }}\n        styles {{\n{}\n        }}",
            element_style("Boundary", "                background #fafad2")
        ),
    );
    let m = parse(&src);
    let fill = rgb(0xfa, 0xfa, 0xd2);
    for mode in [Mode::Light, Mode::Dark] {
        let s = build_in(&m, "c2", mode).scene;
        let reads = s
            .labels
            .iter()
            .find(|l| l.text.lines.first().is_some_and(|t| t == "Reads"))
            .expect("Reads label");
        assert_eq!(reads.fill, Some(fill), "{mode:?}");
        for l in &s.labels {
            let c = l.rect.centre();
            let inside = s.boundaries.iter().rev().find(|b| b.fill.is_some() && b.rect.contains(c));
            let want = inside.and_then(|b| b.fill).unwrap_or(s.background);
            assert_eq!(l.fill, Some(want), "{mode:?} {:?}", l.text.lines);
        }
    }
    // Unstyled boundary: the page colour, as before.
    let plain = parse(&src.replace("background #fafad2", "stroke #333333"));
    for mode in [Mode::Light, Mode::Dark] {
        let s = build_in(&plain, "c2", mode).scene;
        assert!(s.labels.iter().all(|l| l.fill == Some(s.background)), "{mode:?}");
    }
}

/// A label backing shows the colour actually behind the text: each filled
/// boundary holding the label, outermost first, composited at its own
/// opacity over the page (review 2026-11-17 of the P2-431 ruling).
#[test]
fn p2_431_label_backing_composites_boundary_opacity() {
    let src = ws(
        "        user = person \"Operator\"\n        sys = softwareSystem \"Store\" {\n            app = container \"App\"\n            db = container \"Db\"\n        }\n        user -> app \"Uses\"\n        app -> db \"Reads\"",
        &format!(
            "        container sys \"c2\" {{\n            include *\n        }}\n        styles {{\n{}\n        }}",
            element_style("Boundary", "                background #f4f4f4\n                opacity 80")
        ),
    );
    let m = parse(&src);
    // 0.8 * 0xf4 + 0.2 * page, rounded.
    for (mode, want) in [
        // redesign 2026-11-19: over the dark paper #1e2124 (was #111111).
        (Mode::Dark, rgb(0xc9, 0xca, 0xca)),
        (Mode::Light, rgb(0xf6, 0xf6, 0xf6)),
    ] {
        let s = build_in(&m, "c2", mode).scene;
        let reads = s
            .labels
            .iter()
            .find(|l| l.text.lines.first().is_some_and(|t| t == "Reads"))
            .expect("Reads label");
        assert_eq!(reads.fill, Some(want), "{mode:?}");
    }
}

/// In dark mode a line label's text takes the line's colour only when it
/// contrasts at least 3:1 with the backing, else #aaaaaa; light mode keeps
/// the line's colour.
#[test]
fn p2_431_dark_label_text_contrast() {
    let with = |colour: &str| {
        parse(&plan_ws(
            "",
            &format!("            relationship \"Relationship\" {{\n                color {colour}\n            }}"),
        ))
    };
    let label_colours = |m: &Model, mode: Mode| -> Vec<Colour> {
        build_in(m, "ctx", mode).scene.labels.iter().map(|l| l.text.colour).collect()
    };
    // Navy on the #111111 page: about 1.3:1.
    let navy = with("#000080");
    // redesign 2026-11-19: the fallback is `dg-ink` (was #aaaaaa).
    assert!(label_colours(&navy, Mode::Dark).iter().all(|&c| c == rgb(0xe8, 0xea, 0xec)));
    assert!(label_colours(&navy, Mode::Light).iter().all(|&c| c == rgb(0, 0, 0x80)));
    // Amber on #111111: well above 3:1, kept.
    let amber = with("#ffcc00");
    assert!(label_colours(&amber, Mode::Dark).iter().all(|&c| c == rgb(0xff, 0xcc, 0)));
    // The line itself keeps its colour either way.
    let s = build_in(&navy, "ctx", Mode::Dark).scene;
    assert!(s.lines.iter().all(|l| l.stroke.colour == rgb(0, 0, 0x80)));
}

/// Lines corpus test.dsl uses that Structurizr 6.2.4 accepts and the
/// SRS does not list: element `iconPosition top|bottom|left`,
/// relationship `metadata`/`description` booleans, and `theme`/`themes`
/// inside `styles`.
// TODO(srs): P2-10, P2-12, P2-13 name none of these. Reading taken
// (Structurizr 6.2.4 probed): each is accepted and checked; the first
// two change no scene, a theme line in `styles` is read as one in
// `views`.
#[test]
fn p2_14_structurizr_style_lines_outside_the_srs() {
    let m = parse(&element_block("Tagged", "                iconPosition Left\n                background #112233"));
    assert_eq!(m.element_styles[0].background.unwrap().value, rgb(0x11, 0x22, 0x33));
    let e = error(&element_block("Tagged", "                iconPosition right"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
    let r = parse(&rel_block("Tagged", "                metadata false\n                description true\n                thickness 3"));
    assert_eq!(r.relationship_styles.len(), 1);
    let e = error(&rel_block("Tagged", "                metadata maybe"));
    assert!(matches!(e.kind, ModelErrorKind::InvalidValue { .. }), "{e:?}");
    let t = parse(&rel_ws("            theme default\n            themes default default"));
    assert_eq!(t.themes.len(), 3);
}
