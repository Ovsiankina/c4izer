//! Tests of the line-colour row (SRS 4.9: P5-300..P5-311), core part.
//! One test per requirement at least, named after it. The colour pass
//! itself (P5-304, P5-305 tie-breaks) and the P5-312 benchmarks are unit
//! tests in `crates/core/src/scene/line_colour.rs`; the editor's Settings
//! row (P5-309) is tested in the editor crate.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use c4rizer_core::edit::{self, Edit};
use c4rizer_core::export::{self, Flavour};
use c4rizer_core::geom::Point;
use c4rizer_core::model::{self, Colour, LineColours, Model};
use c4rizer_core::pipeline::{self, PREVIEW_ROUTE, ViewBuild};
use c4rizer_core::place::Keep;
use c4rizer_core::scene::{Scene, Swatch};

const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl"));
const DYNAMIC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/dynamic_view.dsl"));
const DEPLOYMENT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/deployment_view.dsl"));
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/golden");
const KEYS: [&str; 4] = ["C1", "C2", "C3-serving", "C3-startup"];

/// Three systems in three fills: `a -> b`, `b -> a` (a two-way pair),
/// `c -> b`; `b` is the most connected box.
const TRIO: &str = r#"workspace {
    model {
        a = softwareSystem "Alpha" "First system." "A"
        b = softwareSystem "Beta" "Second system." "B"
        c = softwareSystem "Gamma" "Third system." "C"
        a -> b "Sends orders to"
        b -> a "Confirms to"
        c -> b "Reads from"
    }
    views {
        systemLandscape "trio" "Trio" {
            include *
            at a 200 300
            at b 900 300
            at c 900 800
SETTING
        }
        styles {
            element "A" {
                background #08427b
            }
            element "B" {
                background #438dd5
            }
            element "C" {
                background #85bbf0
            }
STYLES
        }
    }
}
"#;

const BLUE_A: Colour = Colour { r: 0x08, g: 0x42, b: 0x7b };
const BLUE_B: Colour = Colour { r: 0x43, g: 0x8d, b: 0xd5 };

fn parse(src: &str) -> Model {
    match model::parse(src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    }
}

fn build(m: &Model, key: &str) -> ViewBuild {
    match pipeline::build_view(m, key, &Keep::default(), &AtomicBool::new(false)) {
        Ok(b) => b,
        Err(e) => panic!("{e:?}"),
    }
}

fn trio_src(setting: &str, styles: &str) -> String {
    TRIO.replace("SETTING", setting).replace("STYLES\n", styles)
}

fn trio(setting: &str, styles: &str) -> ViewBuild {
    build(&parse(&trio_src(setting, styles)), "trio")
}

/// The stroke colour of the line `from -> to` (box names) in `scene`.
fn colour(b: &ViewBuild, from: &str, to: &str) -> Colour {
    let name = |id: c4rizer_core::view::BoxId| b.plan.boxes[id.0 as usize].name.as_str();
    b.scene
        .lines
        .iter()
        .find(|l| name(l.source) == from && name(l.destination) == to)
        .unwrap_or_else(|| panic!("no line {from} -> {to}"))
        .stroke
        .colour
}

/// `src` with `line` added as the first line of every view block that
/// draws lines.
fn every_view(src: &str, line: &str) -> String {
    let mut out = String::new();
    let mut in_views = false;
    for l in src.lines() {
        out.push_str(l);
        out.push('\n');
        let t = l.trim_start();
        if t.starts_with("views") && t.ends_with('{') {
            in_views = true;
            continue;
        }
        let kinds = ["systemContext ", "container ", "component ", "dynamic ", "deployment ", "systemLandscape ", "custom "];
        if in_views && t.ends_with('{') && kinds.iter().any(|k| t.starts_with(k)) {
            out.push_str("            ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The plain line colour of a light scene: the colour of an unstyled
/// line in a view without `lineColours`.
fn plain_colour() -> Colour {
    colour(&trio("", ""), "Gamma", "Beta")
}

#[test]
fn p5_300_line_colours_parsed() {
    for (setting, want) in [
        ("lineColours box", LineColours::Box),
        ("lineColors box", LineColours::Box),
        ("LINECOLOURS Box", LineColours::Box),
        ("lineColours plain", LineColours::Plain),
        ("", LineColours::Plain),
    ] {
        let m = parse(&trio_src(setting, ""));
        let v = m.view("trio").expect("view");
        assert_eq!(v.options().line_colours, want, "{setting:?}");
        assert_eq!(v.line_colours.map(|l| l.value), (!setting.is_empty()).then_some(want), "{setting:?}");
    }
    assert_eq!(LineColours::Box.keyword(), "box");
    assert_eq!(LineColours::Plain.keyword(), "plain");
    assert_eq!(model::ViewOptions::DEFAULT.line_colours, LineColours::Plain);
}

#[test]
fn p5_300_box_view_lines_take_their_end_box_fill() {
    let b = trio("lineColours box", "");
    assert_eq!(colour(&b, "Gamma", "Beta"), BLUE_B);
    // Plain (written or not): every unstyled line in the plain colour.
    let plain = plain_colour();
    assert_ne!(plain, BLUE_B);
    for setting in ["", "lineColours plain"] {
        let b = trio(setting, "");
        assert!(b.scene.lines.iter().all(|l| l.stroke.colour == plain), "{setting:?}");
    }
}

#[test]
fn p5_301_bad_values_are_errors_and_a_second_line_overrides() {
    for setting in ["lineColours boxes", "lineColours", "lineColours box plain", "lineColours \"box\""] {
        let e = model::parse(&trio_src(setting, "")).expect_err(setting);
        assert!(e.to_string().to_ascii_lowercase().contains("linecolours"), "{setting}: {e}");
    }
    let m = parse(&trio_src("lineColours box\n            lineColors plain", ""));
    assert_eq!(m.view("trio").expect("view").options().line_colours, LineColours::Plain);
    assert!(
        m.notices.iter().any(|n| n.to_string().contains("earlier `lineColors` line")),
        "{:?}",
        m.notices
    );
}

#[test]
fn p5_302_accepted_in_every_view_that_draws_lines() {
    for (name, src) in [("example", EXAMPLE), ("dynamic", DYNAMIC), ("deployment", DEPLOYMENT)] {
        let m = parse(&every_view(src, "lineColours box"));
        let drawn: Vec<_> = m.views.iter().filter(|v| v.filter.is_none() && v.image.is_none()).collect();
        assert!(!drawn.is_empty(), "{name}");
        for v in drawn {
            assert_eq!(v.options().line_colours, LineColours::Box, "{name} {}", v.key);
            let b = build(&m, &v.key);
            assert_eq!(b.scene.options.line_colours, LineColours::Box, "{name} {}", v.key);
        }
    }
    // An image view draws no line and refuses the line.
    let image = r#"workspace {
    model {
        a = softwareSystem "Alpha"
    }
    views {
        image * "pic" {
            plantuml diagram.puml
SETTING
        }
    }
}
"#;
    assert!(model::parse(&image.replace("SETTING\n", "")).is_ok());
    assert!(model::parse(&image.replace("SETTING", "            lineColours box")).is_err());
}

#[test]
fn p5_303_a_relationship_style_colour_wins() {
    let styles = "            relationship \"Relationship\" {\n                color #aa0000\n            }\n";
    let b = trio("lineColours box", styles);
    let red = Colour { r: 0xaa, g: 0, b: 0 };
    assert!(b.scene.lines.iter().all(|l| l.stroke.colour == red));
}

#[test]
fn p5_304_a_box_too_close_to_the_page_keeps_the_plain_colour() {
    let white = "            element \"Element\" {\n                background #ffffff\n            }\n";
    let src = trio_src("lineColours box", white)
        .replace("background #08427b", "background #ffffff")
        .replace("background #438dd5", "background #fafafa")
        .replace("background #85bbf0", "background #ffffff");
    let b = build(&parse(&src), "trio");
    let plain = plain_colour();
    assert!(b.scene.lines.iter().all(|l| l.stroke.colour == plain));
}

#[test]
fn p5_305_a_two_way_pair_takes_the_more_connected_box() {
    let b = trio("lineColours box", "");
    // b has three lines, a two: both lines of the pair are b's fill.
    assert_eq!(colour(&b, "Alpha", "Beta"), BLUE_B);
    assert_eq!(colour(&b, "Beta", "Alpha"), BLUE_B);
    // Without c -> b the two tie, then tie on incoming: a is declared first.
    let src = trio_src("lineColours box", "").replace("        c -> b \"Reads from\"\n", "");
    let b = build(&parse(&src), "trio");
    assert_eq!(colour(&b, "Alpha", "Beta"), BLUE_A);
    assert_eq!(colour(&b, "Beta", "Alpha"), BLUE_A);
}

#[test]
fn p5_306_moving_a_box_keeps_every_colour() {
    let m = parse(&trio_src("lineColours box", ""));
    let b = build(&m, "trio");
    let colours = |s: &Scene| s.lines.iter().map(|l| l.stroke.colour).collect::<Vec<_>>();
    let before = colours(&b.scene);
    let a = b.plan.boxes.iter().find(|x| x.name == "Alpha").expect("a").id;
    // Drag preview: `Scene::move_box`, then the rebuild after the move,
    // then a full build at the new place.
    let mut moved = b.scene.clone();
    moved.move_box(a, Point::new(20, 90));
    assert_eq!(colours(&moved), before);
    let at = BTreeMap::from([(a, Point::new(20, 90))]);
    let again = pipeline::rebuild_after_move(&m, &b, &at, &PREVIEW_ROUTE, &AtomicBool::new(false)).expect("rebuilt");
    assert_eq!(colours(&again.scene), before);
    // Moving the most connected box far away changes nothing either.
    let beta = b.plan.boxes.iter().find(|x| x.name == "Beta").expect("b").id;
    let far = BTreeMap::from([(beta, Point::new(10, 10))]);
    let again = pipeline::rebuild_after_move(&m, &b, &far, &PREVIEW_ROUTE, &AtomicBool::new(false)).expect("rebuilt");
    assert_eq!(colours(&again.scene), before);
}

#[test]
fn p5_307_labels_keep_their_colour_and_the_key_still_draws() {
    let plain = trio("", "");
    let boxed = trio("lineColours box", "");
    let texts = |b: &ViewBuild| b.scene.labels.iter().map(|l| l.text.colour).collect::<Vec<_>>();
    assert_eq!(texts(&boxed), texts(&plain));
    let swatches = |s: &Scene| -> Vec<Colour> {
        s.legend
            .entries
            .iter()
            .filter_map(|e| match e.swatch {
                Swatch::Line { stroke, .. } => Some(stroke.colour),
                _ => None,
            })
            .collect()
    };
    // Every line ends in b's fill: the key's Relationship entry shows it.
    assert_eq!(swatches(&boxed.scene), vec![BLUE_B]);
    // With `a -> c` the lines differ in colour: one entry, plain colour.
    let src = trio_src("lineColours box", "").replace(
        "        c -> b \"Reads from\"\n",
        "        c -> b \"Reads from\"\n        a -> c \"Feeds\"\n",
    );
    let mixed = build(&parse(&src), "trio");
    assert_ne!(colour(&mixed, "Alpha", "Gamma"), colour(&mixed, "Gamma", "Beta"));
    assert_eq!(swatches(&mixed.scene), vec![plain_colour()]);
    assert!(export::svg(&mixed.scene).contains("Relationship"), "the key is drawn");
}

#[test]
fn p5_307_callouts_take_their_line_colour() {
    let b = trio("lineColours box\n            labels callout", "");
    for l in &b.scene.labels {
        let line = b.scene.lines.iter().find(|x| x.edge == l.edge).expect("line");
        let c = l.callout.as_ref().expect("callout");
        assert_eq!(c.stroke.colour, line.stroke.colour);
    }
}

// ------------------------------------------------------------ the edit

fn set(src: &str, mode: Option<LineColours>) -> edit::Applied {
    edit::apply(
        &model::parse(src).expect("parses"),
        &[Edit::SetLineColours {
            view: "trio".to_owned(),
            mode,
        }],
    )
    .expect("applies")
}

fn undone(done: &edit::Applied) -> String {
    edit::apply(&model::parse(&done.text).expect("parses"), &done.inverse).expect("applies").text
}

#[test]
fn p5_308_added_after_the_last_option_line_and_undone() {
    let src = trio_src("            maxCrossings 2", "");
    let done = set(&src, Some(LineColours::Box));
    assert!(done.text.contains("maxCrossings 2\n            lineColours box\n"), "{}", done.text);
    assert_eq!(parse(&done.text).view("trio").expect("view").options().line_colours, LineColours::Box);
    assert_eq!(undone(&done), src);
}

#[test]
fn p5_308_added_after_the_include_line_without_options() {
    let src = TRIO.replace("SETTING\n", "").replace("STYLES\n", "");
    let done = set(&src, Some(LineColours::Box));
    assert!(done.text.contains("include *\n            lineColours box\n"), "{}", done.text);
    assert_eq!(undone(&done), src);
}

#[test]
fn p5_308_rewritten_in_place_keeping_the_spelling_then_removed() {
    let src = TRIO.replace("SETTING", "            lineColors box").replace("STYLES\n", "");
    let plain = set(&src, Some(LineColours::Plain));
    assert_eq!(plain.text, src.replace("lineColors box", "lineColors plain"));
    assert_eq!(undone(&plain), src);
    let bare = TRIO.replace("SETTING\n", "").replace("STYLES\n", "");
    let gone = set(&src, None);
    assert_eq!(gone.text, bare);
    // The inverse of a removal writes the setting back where an added
    // line goes (EDIT-19, `Applied::inverse`): the same model, and the
    // same bytes when the line stood there, as the editor writes it.
    assert_eq!(parse(&undone(&gone)).view("trio").expect("view").options().line_colours, LineColours::Box);
    let placed = bare.replace("include *\n", "include *\n            lineColours box\n");
    let gone = set(&placed, None);
    assert_eq!(gone.text, bare);
    assert_eq!(undone(&gone), placed);
    // `None` on a view without the line changes nothing.
    assert_eq!(set(&bare, None).text, bare);
}

// ------------------------------------------------- plain stays as it was

#[test]
fn p5_310_plain_is_byte_identical() {
    // Goldens: every example view with `lineColours plain` draws its
    // golden SVG, as without the line.
    let m = parse(&every_view(EXAMPLE, "lineColours plain"));
    for key in KEYS {
        let b = build(&m, key);
        let want = std::fs::read_to_string(format!("{GOLDEN}/{key}.svg")).expect("golden");
        assert!(export::svg(&b.scene) == want, "{key} differs from its golden with `lineColours plain`");
    }
}

#[test]
fn p5_311_exporters_ignore_the_option() {
    let all_texts = |m: &Model| -> Vec<(String, String)> {
        let all = pipeline::build_all(m, &AtomicBool::new(false)).expect("builds");
        let mut out = export::text::plantuml(m, &all, Flavour::Structurizr);
        out.extend(export::text::plantuml(m, &all, Flavour::C4PlantUml));
        out.extend(export::text::mermaid(m, &all));
        out.extend(export::text::websequencediagrams(m, &all));
        out.extend(export::text::dot(m, &all, false));
        out.extend(export::text::ilograph(m, &all));
        out.push(("theme".into(), export::theme(m)));
        out
    };
    for src in [EXAMPLE, DYNAMIC, DEPLOYMENT] {
        let boxed_src = every_view(src, "lineColours box");
        let boxed = parse(&boxed_src);
        assert_eq!(all_texts(&boxed), all_texts(&parse(src)));
        // The JSON export carries the DSL text, the line included.
        let all = pipeline::build_all(&boxed, &AtomicBool::new(false)).expect("builds");
        assert!(export::json(&boxed, &all, Some(&boxed_src)).contains("\"structurizr.dsl\""));
    }
}
