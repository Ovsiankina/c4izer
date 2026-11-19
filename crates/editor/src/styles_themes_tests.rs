//! Step-4 contract tests of the styles-themes row, editor part (P2-417
//! panel, P2-433, P2-436 canvas).

use std::path::Path;
use std::sync::atomic::AtomicBool;

use c4rizer_core::model::{Mode, Shape};
use c4rizer_core::scene::Scene;

use crate::camera::Camera;
use crate::canvas::{PaintItem, Primitive, Source, paint_items};
use crate::files::{self, FileEvent, UiState};
use crate::state::{Command, DrawList, Effect, Event, PANEL_SHAPES, State};
use crate::testutil::TempDir;
use crate::worker::{self, ExportFormat, Job, Request, ViewRef};

/// One system in a context view, with a dark-mode fill for it.
const MODED: &str = r#"workspace {
    model {
        operator = person "Operator" "Runs the shop."
        shop = softwareSystem "Shop" "Sells goods."
        operator -> shop "Runs orders through"
    }
    views {
        systemContext shop "context" "The shop" {
            include *
        }
        styles {
            dark {
                element "Software System" {
                    background #222222
                }
            }
        }
    }
}
"#;

/// The shell, synchronously: every effect run at once; the work and
/// writes seen are kept.
struct Shell {
    s: State,
    work: Vec<Request>,
    writes: usize,
}

impl Shell {
    fn open(path: &Path) -> Shell {
        let mut shell = Shell {
            s: State::new(UiState::default()),
            work: Vec::new(),
            writes: 0,
        };
        let effects = shell.s.handle(Event::Command(Command::Open(path.to_path_buf())));
        shell.run(effects);
        shell
    }

    fn run(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            let next = match effect {
                Effect::Read(path) => {
                    let event = match files::read(&path) {
                        Ok((text, hash)) => FileEvent::Loaded { path, text, hash },
                        Err(e) => FileEvent::OpenFailed {
                            path,
                            error: e.to_string(),
                        },
                    };
                    self.s.handle(Event::File(event))
                }
                Effect::Write(request) => {
                    self.writes += 1;
                    self.s.handle(Event::File(files::write_checked(&request)))
                }
                Effect::Work(request) => {
                    self.work.push(request.clone());
                    match worker::serve(request, &AtomicBool::new(false)) {
                        Some(reply) => self.s.handle(Event::Worker(reply)),
                        None => Vec::new(),
                    }
                }
                Effect::Watch(_)
                | Effect::Dialog(_)
                | Effect::SaveUiState(_)
                | Effect::Fullscreen(_)
                | Effect::OpenUrl(_) => Vec::new(),
            };
            self.run(next);
        }
    }

    fn scene(&self) -> &Scene {
        self.s.scene().expect("a scene")
    }
}

/// `MODED` written to a fresh folder, opened.
fn moded() -> (TempDir, Shell) {
    let dir = TempDir::new("styles-themes");
    let path = dir.join("shop.dsl");
    std::fs::write(&path, MODED).expect("fixture written");
    let shell = Shell::open(&path);
    (dir, shell)
}

/// The style panel (EDIT-17) offers all 19 shapes, in the reference
/// spelling (`Shape::keyword`).
#[test]
fn p2_417_panel_offers_all_shapes() {
    let words: Vec<&str> = PANEL_SHAPES.iter().map(|s| s.keyword()).collect();
    assert_eq!(
        words,
        [
            "Box",
            "RoundedBox",
            "Circle",
            "Ellipse",
            "Hexagon",
            "Diamond",
            "Cylinder",
            "Bucket",
            "Pipe",
            "Person",
            "Robot",
            "Folder",
            "WebBrowser",
            "Window",
            "Terminal",
            "Shell",
            "MobileDevicePortrait",
            "MobileDeviceLandscape",
            "Component",
        ]
    );
    let distinct: std::collections::BTreeSet<String> = PANEL_SHAPES.iter().map(|s| format!("{s:?}")).collect();
    assert_eq!(distinct.len(), 19);
    assert!(PANEL_SHAPES.contains(&Shape::Cylinder));
}

/// The fill of the "Shop" box.
fn shop_fill(sh: &Shell) -> (u8, u8, u8) {
    let b = sh.scene().boxes.iter().find(|b| b.name.lines == ["Shop"]).expect("shop box");
    (b.fill.r, b.fill.g, b.fill.b)
}

/// The light/dark switch rebuilds the shown view with
/// `pipeline::build_view_in` in the chosen mode.
#[test]
fn p2_433_mode_switch_rebuilds() {
    let (_dir, mut sh) = moded();
    assert_eq!(sh.scene().mode, Mode::Light);
    let light = shop_fill(&sh);
    let before = sh.work.len();
    let effects = sh.s.set_mode(Mode::Dark);
    sh.run(effects);
    assert!(
        sh.work[before..]
            .iter()
            .any(|r| matches!(r.job, Job::Build { shown: true, mode: Mode::Dark, .. }))
    );
    assert_eq!(sh.s.mode(), Mode::Dark);
    assert_eq!(sh.scene().mode, Mode::Dark);
    assert_ne!(light, shop_fill(&sh));
    assert_eq!(shop_fill(&sh), (0x22, 0x22, 0x22));
    // Switching to the shown mode asks nothing; back to light rebuilds.
    assert!(sh.s.set_mode(Mode::Dark).is_empty());
    let effects = sh.s.set_mode(Mode::Light);
    sh.run(effects);
    assert_eq!(sh.scene().mode, Mode::Light);
    assert_eq!(shop_fill(&sh), light);
}

/// Switching mode writes nothing to the file and pushes no undo step.
#[test]
fn p2_433_mode_switch_writes_nothing() {
    let (dir, mut sh) = moded();
    let undo = sh.s.history.undo.len();
    let effects = sh.s.set_mode(Mode::Dark);
    assert!(!effects.iter().any(|e| matches!(e, Effect::Write(_))));
    sh.run(effects);
    assert_eq!(sh.writes, 0);
    assert_eq!(sh.s.history.undo.len(), undo);
    assert_eq!(std::fs::read_to_string(dir.join("shop.dsl")).expect("file"), MODED);
}

/// The editor's export (EDIT-7) uses the shown mode.
#[test]
fn p2_433_export_uses_shown_mode() {
    let (dir, mut sh) = moded();
    let effects = sh.s.set_mode(Mode::Dark);
    sh.run(effects);
    let out = dir.join("context.svg");
    let effects = sh.s.handle(Event::Command(Command::Export {
        targets: vec![(ViewRef::Declared("context".into()), out.clone())],
        format: ExportFormat::Svg,
    }));
    sh.run(effects);
    assert!(sh.work.iter().any(|r| matches!(r.job, Job::Export { mode: Mode::Dark, .. })));
    let svg = std::fs::read_to_string(&out).expect("exported");
    assert!(svg.contains("#222222"), "{svg}");
    assert_eq!(svg, c4rizer_core::export::svg(sh.scene()));
}

/// The SVG `<g>` lines of `class`, in order.
fn svg_groups<'a>(svg: &'a str, class: &str) -> Vec<&'a str> {
    let open = format!("<g class=\"{class}\"");
    svg.lines().filter(|l| l.starts_with(&open)).collect()
}

/// The non-text primitives of the draw list for `source`, and the
/// opacities of all its items.
fn shapes_of(items: &[PaintItem], source: Source) -> (Vec<&Primitive>, Vec<u8>) {
    let of: Vec<&PaintItem> = items.iter().filter(|i| i.source == source).collect();
    (
        of.iter()
            .map(|i| &i.primitive)
            .filter(|p| !matches!(p, Primitive::Text { .. }))
            .collect(),
        of.iter().map(|i| i.opacity).collect(),
    )
}

/// The `opacity` attribute of an SVG group as a percentage (100 when
/// absent).
fn svg_opacity(group: &str) -> u8 {
    let head = &group[..group.find('>').expect("tag end")];
    head.split("opacity=\"").nth(1).map_or(100, |v| {
        let v = &v[..v.find('"').expect("quote")];
        let frac = v.strip_prefix("0.").unwrap_or("0");
        let digits = format!("{frac:0<2}");
        digits[..2].parse().expect("two digits")
    })
}

/// For the all-styles fixture the canvas draw list has the same items,
/// outlines, strokes, dashes, opacities and jumps as its SVG (EDIT-12).
#[test]
#[expect(clippy::too_many_lines, reason = "one walk over every scene item")]
fn p2_436_draw_list_matches_svg() {
    use c4rizer_core::scene::ItemKey;
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/styles/all_styles.dsl"))
        .expect("fixture");
    // As the editor parses it: through `parse_with` with the file's path,
    // so the local theme and icon files are read (P2-13, P2-409).
    let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/styles/all_styles.dsl"));
    let parsed = c4rizer_core::model::parse_with(&text, Some(path), &mut c4rizer_core::model::FsLoader)
        .expect("fixture parses");
    assert!(parsed.model.assets.iter().any(|a| a.path == "icons/database.svg"));
    for mode in [Mode::Light, Mode::Dark] {
        let build = c4rizer_core::pipeline::build_view_in(
            &parsed.model,
            "all",
            &c4rizer_core::place::Keep::default(),
            mode,
            &AtomicBool::new(false),
        )
        .expect("builds");
        let scene = &build.scene;
        let svg = c4rizer_core::export::svg(scene);
        let list = DrawList {
            scene: Some(scene),
            camera: Camera::default(),
            marks: Vec::new(),
            rubber_band: None,
            resize_outline: None,
            editing: None,
            read_only: false,
            hide_descriptions: false,
            hide_metadata: false,
            hide_key: false,
            faded: Vec::new(),
            tooltip: None,
            written_thickness: crate::canvas::thickness_written(&parsed.model, scene),
        };
        let items = paint_items(&list);
        let boxes = svg_groups(&svg, "box");
        assert_eq!(boxes.len(), scene.boxes.len());
        assert!(scene.boxes.iter().any(|b| b.icon.is_some()), "the fixture draws an icon");
        for (b, g) in scene.boxes.iter().zip(boxes) {
            let (prims, opacities) = shapes_of(&items, Source::Scene(ItemKey::Box(b.id)));
            let svg_shapes: usize = ["<rect", "<path", "<circle", "<ellipse", "<image"]
                .iter()
                .map(|t| g.matches(t).count())
                .sum();
            assert_eq!(prims.len(), svg_shapes, "{:?}: {g}", b.shape);
            assert_eq!(
                prims.iter().any(|p| matches!(p, Primitive::Image { .. })),
                g.contains("<image")
            );
            // An icon is painted as its image, not a frame: the primitive
            // carries the asset the SVG embeds, and the canvas's texture
            // of it has the image's own pixels (P2-409).
            for p in &prims {
                let Primitive::Image { min, max, asset } = p else { continue };
                let icon = b.icon.as_ref().expect("scene icon");
                assert_eq!(asset, &icon.asset);
                let w = (max.x - min.x).round();
                let h = (max.y - min.y).round();
                let image = crate::canvas::icon_image(asset, w, h).expect("icon decodes");
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let size = [w as usize, h as usize];
                assert_eq!(image.size, size);
                let opaque: Vec<_> = image.pixels.iter().filter(|c| c.a() > 0).collect();
                assert!(!opaque.is_empty(), "{}: nothing drawn", icon.asset.path);
                assert!(
                    opaque.iter().any(|c| c.r() != c.g() || c.g() != c.b()),
                    "{}: only grey pixels",
                    icon.asset.path
                );
            }
            let dashed = prims.iter().any(|p| matches!(p, Primitive::Polygon { dash: Some(_), .. }));
            assert_eq!(dashed, g.contains("stroke-dasharray"), "{:?}", b.shape);
            assert!(opacities.iter().all(|&o| o == b.opacity));
            assert_eq!(svg_opacity(g), b.opacity);
            let width = prims.iter().find_map(|p| match p {
                Primitive::Polygon { stroke: Some((_, w)), .. } => Some(*w),
                _ => None,
            });
            assert_eq!(width, Some(f32::from(u8::try_from(b.stroke.width_px).expect("small"))));
        }
        let lines = svg_groups(&svg, "line");
        assert_eq!(lines.len(), scene.lines.len());
        let mut hops = 0;
        for (l, g) in scene.lines.iter().zip(lines) {
            let (prims, opacities) = shapes_of(&items, Source::Scene(ItemKey::Line(l.edge)));
            // redesign 2026-11-19: the port is one SVG circle and two
            // canvas discs (ring and paper).
            assert_eq!(prims.len(), g.matches("<path").count() + g.matches("<circle").count() + 1);
            let Primitive::Path { dash, points, .. } = prims[0] else {
                panic!("the route comes first")
            };
            let route = &g[..g.find("/>").expect("route")];
            assert_eq!(dash.is_some(), route.contains("stroke-dasharray"));
            // redesign 2026-11-19: rounded corners add points, so the
            // route is checked by its start and its hops (9 points each).
            assert_eq!(points[0], list.camera.to_screen(l.points[0]));
            assert!(points.len() >= l.points.len() + 9 * l.jumps.len());
            assert_eq!(route.matches(" A6 6 ").count(), l.jumps.len());
            hops += l.jumps.len();
            assert!(opacities.iter().all(|&o| o == l.opacity));
            assert_eq!(svg_opacity(g), l.opacity);
        }
        assert!(hops > 0 || scene.crossings == 0, "{mode:?}");
        for (i, (b, g)) in scene.boundaries.iter().zip(svg_groups(&svg, "boundary")).enumerate() {
            let (prims, _) = shapes_of(&items, Source::Scene(ItemKey::Boundary(u32::try_from(i).expect("small"))));
            let Primitive::Polygon { fill, dash, .. } = prims[0] else {
                panic!("boundary outline")
            };
            assert_eq!(fill.is_some(), b.fill.is_some());
            assert_eq!(dash.is_some(), g.contains("stroke-dasharray"));
            assert_eq!(svg_opacity(g), b.opacity);
        }
    }
}

/// A file with a local theme and a local icon file, opened in the editor.
const THEMED: &str = r#"workspace {
    model {
        operator = person "Operator" "Runs the shop."
        shop = softwareSystem "Shop" "Sells goods." "Store"
        operator -> shop "Runs orders through"
    }
    views {
        systemContext shop "context" "The shop" {
            include *
        }
        theme theme.json
        styles {
            element "Store" {
                icon store.svg
            }
        }
    }
}
"#;

const THEME_JSON: &str = r##"{"elements": [{"tag": "Software System", "background": "#2e7d32"}]}"##;

const STORE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#ff0000"/></svg>"##;

/// The editor parses through `parse_with` with the file's path: a local
/// theme draws and a local icon file is loaded, and an edit
/// (`edit::apply`, its re-parse) on that file keeps working (P2-13,
/// P2-409, EDIT-17).
#[test]
fn p2_13_editor_opens_and_edits_themed_file() {
    use crate::state::{Button, Input, Modifiers};
    use c4rizer_core::edit::{StyleProperty, StyleTarget};
    use c4rizer_core::model::Colour;
    let dir = TempDir::new("styles-themes-themed");
    let path = dir.join("shop.dsl");
    std::fs::write(&path, THEMED).expect("fixture written");
    std::fs::write(dir.join("theme.json"), THEME_JSON).expect("theme written");
    std::fs::write(dir.join("store.svg"), STORE_SVG).expect("icon written");
    let mut sh = Shell::open(&path);
    assert_eq!(shop_fill(&sh), (0x2e, 0x7d, 0x32), "theme colour");
    let shop = sh.scene().boxes.iter().find(|b| b.name.lines == ["Shop"]).expect("shop box");
    assert!(shop.icon.is_some(), "file icon drawn");
    let at = sh.s.camera.to_screen(shop.rect.centre());
    let none = Modifiers::default();
    let effects = sh.s.handle(Event::Input(Input::PointerDown {
        at,
        button: Button::Left,
        mods: none,
    }));
    sh.run(effects);
    let effects = sh.s.handle(Event::Input(Input::PointerUp { at, button: Button::Left }));
    sh.run(effects);
    let effects = sh.s.handle(Event::Command(Command::SetStyle {
        target: StyleTarget::Tag("Store".into()),
        property: StyleProperty::Color(Some(Colour { r: 0xff, g: 0xff, b: 0x00 })),
    }));
    sh.run(effects);
    assert_eq!(sh.writes, 1, "the edit was written");
    let written = std::fs::read_to_string(&path).expect("file");
    assert!(written.contains("theme theme.json") && written.contains("#ffff00"), "{written}");
    assert!(sh.s.doc.as_ref().and_then(|d| d.parse_error.as_ref()).is_none());
    assert_eq!(shop_fill(&sh), (0x2e, 0x7d, 0x32), "theme still drawn");
}

/// One line whose style writes `thickness 2`, one unstyled.
const THICK: &str = r#"workspace {
    model {
        a = softwareSystem "A"
        b = softwareSystem "B"
        c = softwareSystem "C"
        a -> b "Thick" "" "Thick"
        a -> c "Plain"
    }
    views {
        systemLandscape "l" {
            include *
        }
        styles {
            relationship "Thick" {
                thickness 2
            }
        }
    }
}
"#;

/// P2-418 with the redesign's 1.5px edge (EDIT-12): the canvas paints a
/// line whose style writes `thickness 2` exactly 2px wide and an
/// unstyled line 1.5px, as the SVG strokes them.
#[test]
fn p2_418_canvas_written_thickness_2_matches_svg() {
    use c4rizer_core::scene::ItemKey;
    let dir = TempDir::new("thick");
    let path = dir.join("thick.dsl");
    std::fs::write(&path, THICK).expect("fixture written");
    let sh = Shell::open(&path);
    let list = sh.s.draw_list();
    let scene = list.scene.expect("a scene");
    let svg = c4rizer_core::export::svg(scene);
    let items = paint_items(&list);
    let model = sh.s.model().expect("a parse");
    let groups = svg_groups(&svg, "line");
    assert_eq!(groups.len(), 2);
    for (l, g) in scene.lines.iter().zip(groups) {
        let thick = model.relationship(l.relationship).tags.iter().any(|t| t == "Thick");
        let want = if thick { 2.0 } else { 1.5 };
        let route = &g[..g.find("/>").expect("route")];
        assert!(route.contains(&format!("stroke-width=\"{want}\"")), "{route}");
        let (prims, _) = shapes_of(&items, Source::Scene(ItemKey::Line(l.edge)));
        let widths: Vec<f32> = prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Path { width, .. } => Some(width / list.camera.zoom),
                _ => None,
            })
            .collect();
        assert!(!widths.is_empty());
        for w in widths {
            assert!((w - want).abs() < 1e-4, "thick {thick}: {w}");
        }
    }
}
