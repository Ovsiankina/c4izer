//! Tests of the anchors row (SRS 4.8: P4-200..P4-212, P4-215..P4-220),
//! core part. One test per requirement at least, named after it. The
//! editor tests (P4-213..P4-218) are in `crates/editor/src/anchors_tests.rs`.

// Tests name the model `m`, the build `b`, a line `l`, as the router does.
#![allow(clippy::many_single_char_names, clippy::similar_names)]

use std::sync::atomic::AtomicBool;

use c4rizer_core::anchor::{self, Anchor, AnchorCorner, AnchorSide};
use c4rizer_core::edit;
use c4rizer_core::export;
use c4rizer_core::geom::{Contact, Dir, Point, Rect, Segment};
use c4rizer_core::model::{self, Model, NoticeKind, ViewSpec};
use c4rizer_core::pipeline::{self, ViewBuild};
use c4rizer_core::place::Keep;
use c4rizer_core::route::{LineEnd, Side};
use c4rizer_core::scene::SceneLine;
use c4rizer_core::view::BoxId;

const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl"));
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/golden");

/// A shop, roles only. Relationship order: 0 customer -> web, 1 web -> db,
/// 2 web -> api, 3 api -> db, 4 admin -> web, 5 web -> bank.
const SHOP: &str = r#"        customer = person "Customer" "Buys goods."
        admin = person "Admin" "Runs the shop."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust"
            api = container "Api" "Answers calls." "Rust"
            db = container "Db" "Stores orders." "SQL" "Database"
        }
        bank = softwareSystem "Bank" "Moves money." "External"
        customer -> web "Browses" "HTTPS"
        web -> db "Reads orders" "SQL"
        web -> api "Calls" "JSON"
        api -> db "Writes orders" "SQL"
        admin -> web "Manages" "HTTPS"
        web -> bank "Charges cards" "HTTPS""#;

/// `views_body` in a workspace over `SHOP`.
fn ws(views_body: &str) -> String {
    format!("workspace {{\n    model {{\n{SHOP}\n    }}\n    views {{\n{views_body}\n    }}\n}}\n")
}

/// A container view of the shop holding `lines`, every box placed. Web
/// is 240 x 140 px at (200, 400): x 80..320, y 330..470.
fn c2(lines: &str) -> String {
    ws(&format!(
        "        container shop \"c2\" \"Shop\" {{\n            include *\n            at customer 200 100\n            at admin 700 100\n            at web 200 400\n            at api 700 400\n            at db 200 800\n            at bank 700 800\n{lines}\n        }}"
    ))
}

/// A dynamic view over `SHOP`: steps 1 customer -> web, 2 web -> db,
/// 3 web -> db again, each box placed.
fn dynamic(lines: &str) -> String {
    ws(&format!(
        "        dynamic shop \"flow\" \"Flow\" {{\n            customer -> web \"Opens\"\n            web -> db \"Loads\"\n            web -> db \"Loads again\"\n            at customer 200 100\n            at web 200 400\n            at db 200 800\n{lines}\n        }}"
    ))
}

/// The model of `src`, panicking with the error text.
fn parse(src: &str) -> Model {
    match model::parse(src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    }
}

/// The view keyed `key`.
fn view<'m>(m: &'m Model, key: &str) -> &'m ViewSpec {
    m.view(key).expect("view exists")
}

/// The build of view `key` of `m`.
fn build(m: &Model, key: &str) -> ViewBuild {
    match pipeline::build_view(m, key, &Keep::default(), &AtomicBool::new(false)) {
        Ok(b) => b,
        Err(e) => panic!("{e:?}"),
    }
}

/// The build of the `c2` view of `c2(lines)`.
fn c2_build(lines: &str) -> (Model, ViewBuild) {
    let m = parse(&c2(lines));
    let b = build(&m, "c2");
    (m, b)
}

/// The scene line drawn from the box named `from` to the box named `to`.
fn line<'b>(m: &Model, b: &'b ViewBuild, from: &str, to: &str) -> &'b SceneLine {
    let name = |id: BoxId| m.element(b.plan.boxes[id.0 as usize].element).name.value.as_str();
    b.scene
        .lines
        .iter()
        .find(|l| name(l.source) == from && name(l.destination) == to)
        .expect("line drawn")
}

/// The scene rectangle (px) of the box named `name`.
fn rect(m: &Model, b: &ViewBuild, name: &str) -> Rect {
    b.scene
        .boxes
        .iter()
        .find(|x| m.element(x.element).name.value == name)
        .expect("box drawn")
        .rect
}

/// The direction of the segment `a -> b`.
fn dir(a: Point, b: Point) -> Dir {
    Segment::new(a, b).dir().expect("octilinear")
}

/// The anchor `word [percent]`.
fn a(word: &str, percent: Option<u32>) -> Anchor {
    Anchor::parse(word, percent).expect("an anchor")
}

/// The `AnchorIgnored` notices of `m`, as displayed.
fn ignored(m: &Model) -> Vec<String> {
    m.notices.iter().filter(|n| n.kind == NoticeKind::AnchorIgnored).map(ToString::to_string).collect()
}

#[test]
fn p4_200_anchor_line_parsed() {
    let src = c2("            anchor web -> db from bottom to top 25");
    let m = parse(&src);
    let v = view(&m, "c2");
    assert_eq!(v.anchors.len(), 1);
    let l = &v.anchors[0];
    assert_eq!(m.element(l.source.value).name.value, "Web");
    assert_eq!(m.element(l.destination.value).name.value, "Db");
    assert_eq!(l.order, None);
    assert_eq!(l.from.map(|p| p.value), Some(a("bottom", None)));
    assert_eq!(l.to.map(|p| p.value), Some(a("top", Some(25))));
    assert_eq!(&src[l.span.range()], "anchor web -> db from bottom to top 25");
    assert_eq!(&src[l.to.expect("to").span.range()], "top 25");
    // One end only, either one; keywords ignore case.
    let m = parse(&c2("            ANCHOR web -> db TO BottomLeft"));
    let l = &view(&m, "c2").anchors[0];
    assert_eq!((l.from, l.to.map(|p| p.value)), (None, Some(Anchor::Corner(AnchorCorner::BottomLeft))));
    // In a dynamic view `anchor <n>:` names the step, as `via` does.
    let m = parse(&dynamic("            anchor 3: web -> db from left"));
    let v = view(&m, "flow");
    assert_eq!(v.anchors[0].order.as_ref().map(|o| o.value.as_str()), Some("3"));
    let plan = c4rizer_core::view::plan(&m, v);
    let of = |order: &str| plan.edges.iter().find(|e| e.order.as_deref() == Some(order)).expect("edge").source_anchor;
    assert_eq!(of("3"), Some(a("left", None)));
    assert_eq!(of("2"), None);
    assert!(ignored(&m).is_empty());
}

#[test]
fn p4_200_swap_steps_renumbers_anchor_lines() {
    let src = dynamic("            anchor 3: web -> db from left");
    let parsed = model::parse(&src).expect("parses");
    let done = edit::apply(
        &parsed,
        &[edit::Edit::SwapSteps {
            view: "flow".to_owned(),
            first: 1,
        }],
    )
    .expect("applies");
    assert!(done.text.contains("anchor 2: web -> db from left"), "{}", done.text);
    let back = edit::apply(&model::parse(&done.text).expect("parses"), &done.inverse).expect("applies");
    assert_eq!(back.text, src);
}

#[test]
fn p4_201_points_written() {
    let cases = [
        ("topLeft", Anchor::Corner(AnchorCorner::TopLeft)),
        ("topright", Anchor::Corner(AnchorCorner::TopRight)),
        ("BOTTOMRIGHT", Anchor::Corner(AnchorCorner::BottomRight)),
        ("bottomLeft", Anchor::Corner(AnchorCorner::BottomLeft)),
        ("top", Anchor::side(AnchorSide::Top, 50)),
        ("right 0", Anchor::side(AnchorSide::Right, 0)),
        ("bottom 100", Anchor::side(AnchorSide::Bottom, 100)),
        ("left 33", Anchor::side(AnchorSide::Left, 33)),
    ];
    for (text, want) in cases {
        let m = parse(&c2(&format!("            anchor web -> db from {text}")));
        assert_eq!(view(&m, "c2").anchors[0].from.map(|p| p.value), Some(want), "{text}");
        assert!(ignored(&m).is_empty(), "{text}");
    }
    // Written back in the shortest form: the middle without its 50.
    assert_eq!(Anchor::side(AnchorSide::Top, 50).to_string(), "top");
    assert_eq!(Anchor::side(AnchorSide::Left, 25).to_string(), "left 25");
    assert_eq!(Anchor::Corner(AnchorCorner::BottomRight).to_string(), "bottomRight");
}

#[test]
fn p4_202_point_relative_to_box() {
    let pinned = "            anchor web -> db from right 25";
    let (m, b) = c2_build(pinned);
    let l = line(&m, &b, "Web", "Db");
    assert_eq!(l.points[0], Point::new(320, 370), "a quarter down web's right side (y 330 + 140 / 4, on the grid)");
    // Moved: the point moves with the box.
    let moved = c2(pinned).replace("at web 200 400", "at web 100 300");
    let m = parse(&moved);
    let b = build(&m, "c2");
    assert_eq!(line(&m, &b, "Web", "Db").points[0], Point::new(220, 270));
    // Resized: still a quarter of the new height.
    let resized = c2(&format!("            size web 400 200\n{pinned}"));
    let m = parse(&resized);
    let b = build(&m, "c2");
    let r = rect(&m, &b, "Web");
    assert_eq!(line(&m, &b, "Web", "Db").points[0], Point::new(r.max.x, r.min.y + 50));
    // The grid point of each kind of anchor on a 240 x 140 box.
    let g = Rect::from_centre(Point::new(20, 40), 12, 7);
    assert_eq!(a("topLeft", None).point(g), Point::new(8, 33));
    assert_eq!(a("bottomRight", None).point(g), Point::new(32, 47));
    assert_eq!(a("top", None).point(g), Point::new(20, 33));
    assert_eq!(a("left", Some(25)).point(g), Point::new(8, 37), "round(14 * 0.25) = 4 (3.5 rounds up)");
}

#[test]
fn p4_203_bad_anchor_lines_are_notices() {
    let bad = [
        ("anchor web -> db", "needs `from <point>` or `to <point>`"),
        ("anchor web -> db from", "needs a point"),
        ("anchor web -> db from middle", "is not a point"),
        ("anchor web -> db from top 101", "is not a point"),
        ("anchor web -> db from topLeft 10", "`topLeft 10` is not a point"),
        ("anchor web -> db from top to left from right", "given twice"),
        ("anchor web -> db at top", "not `from` or `to`"),
        ("anchor web db from top", "needs `<source> -> <destination>`"),
        ("anchor web -> nowhere from top", "no element `nowhere`"),
        ("anchor db -> web from top", "draws no such line"),
        ("anchor 2: web -> db from top", "outside a dynamic view"),
    ];
    for (text, why) in bad {
        // The bad line sits beside a good one, which still applies.
        let src = c2(&format!("            {text}\n            anchor api -> db to right"));
        let m = parse(&src);
        let notes = ignored(&m);
        assert_eq!(notes.len(), 1, "{text}: {notes:?}");
        assert!(notes[0].contains(why), "{text}: {}", notes[0]);
        assert!(notes[0].contains("view c2"), "{}", notes[0]);
        let n = m.notices.iter().find(|n| n.kind == NoticeKind::AnchorIgnored).expect("notice");
        assert_eq!(&src[n.span.expect("span").range()], text, "the notice names the line");
        assert_eq!(view(&m, "c2").anchors.len(), 1, "{text}: only the good line is kept");
        let b = build(&m, "c2");
        let r = rect(&m, &b, "Db");
        assert_eq!(line(&m, &b, "Api", "Db").points.last(), Some(&Point::new(r.max.x, 800)));
    }
    // A hidden line is not drawn either.
    let m = parse(&c2("            hide web -> db\n            anchor web -> db from top"));
    assert_eq!(ignored(&m).len(), 1);
    // A second anchor line for one line: the first wins.
    let src = c2("            anchor web -> db from left\n            anchor web -> db from right");
    let m = parse(&src);
    let notes = ignored(&m);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("already has an anchor line"), "{}", notes[0]);
    let anchors = &view(&m, "c2").anchors;
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].from.map(|p| p.value), Some(a("left", None)));
}

#[test]
fn p4_204_plan_edge_carries_anchors() {
    let m = parse(&c2("            anchor web -> db from left to topRight"));
    let plan = c4rizer_core::view::plan(&m, view(&m, "c2"));
    let element = |b: BoxId| m.element(plan.boxes[b.0 as usize].element).name.value.clone();
    for e in &plan.edges {
        if element(e.source) == "Web" && element(e.destination) == "Db" {
            assert_eq!(e.source_anchor, Some(a("left", None)));
            assert_eq!(e.destination_anchor, Some(a("topRight", None)));
        } else {
            assert_eq!((e.source_anchor, e.destination_anchor), (None, None));
        }
    }
}

#[test]
fn p4_205_snap_point_rule() {
    // 240 x 140 px: four corners, top and bottom in quarters, the short
    // sides at their middle.
    let r = Rect::from_centre(Point::new(20, 40), 12, 7);
    let names: Vec<String> = anchor::snap_points(r).iter().map(|(a, _)| a.to_string()).collect();
    assert_eq!(
        names,
        [
            "topLeft", "topRight", "bottomRight", "bottomLeft", "top 25", "top", "top 75", "right", "bottom 25", "bottom", "bottom 75",
            "left"
        ]
    );
    // Each point is where its anchor resolves.
    for (a, p) in anchor::snap_points(r) {
        assert_eq!(a.point(r), p);
    }
    // 360 px: sixths; 200 px: the middle only.
    let per_side = |w_px: i32| {
        let r = Rect::from_centre(Point::new(0, 0), w_px / 20, 3);
        anchor::snap_points(r).iter().filter(|(a, _)| matches!(a, Anchor::Side { side: AnchorSide::Top, .. })).count()
    };
    assert_eq!([per_side(200), per_side(240), per_side(360), per_side(480), per_side(720)], [1, 3, 5, 7, 11]);
    // About L / n apart: never closer than 50 px on a side up to 1000 px,
    // the middle always present.
    for w in (40..=1000).step_by(20) {
        let r = Rect::from_centre(Point::new(0, 0), w / 20, 3);
        let mut xs: Vec<i32> = anchor::snap_points(r).iter().filter(|(_, p)| p.y == r.min.y).map(|(_, p)| p.x).collect();
        xs.sort_unstable();
        assert!(xs.contains(&0), "{w}: the middle");
        if w >= 120 {
            assert!(xs.windows(2).all(|p| (p[1] - p[0]) * 10 >= 50), "{w}: {xs:?}");
        }
    }
}

#[test]
fn p4_206_pinned_end_exact_and_square() {
    let (m, b) = c2_build("            anchor web -> db from left 25 to right 75");
    let l = line(&m, &b, "Web", "Db");
    let (w, d) = (rect(&m, &b, "Web"), rect(&m, &b, "Db"));
    assert!(l.routed);
    assert_eq!(l.points[0], Point::new(w.min.x, 370));
    assert_eq!(dir(l.points[0], l.points[1]), Dir::W, "leaves square to the left side");
    assert!((l.points[0].x - l.points[1].x) >= 20, "for 20 px at least");
    let n = l.points.len();
    assert_eq!(l.points[n - 1], Point::new(d.max.x, 840));
    assert_eq!(dir(l.points[n - 2], l.points[n - 1]), Dir::W, "arrives square into the right side");
    assert!((l.points[n - 2].x - l.points[n - 1].x) >= 20);
    let r = b.routes.line(l.edge).expect("routed");
    assert_eq!(r.source.side, Side::W);
    assert!(matches!(r.end, LineEnd::Arrow(p) if p.side == Side::E), "the arrowhead sits on the pinned side");
    // Every side, as a destination: the last segment heads into the box.
    for (side, into) in [("top", Dir::S), ("right", Dir::W), ("bottom", Dir::N), ("left", Dir::E)] {
        let (m, b) = c2_build(&format!("            anchor customer -> web to {side}"));
        let l = line(&m, &b, "Customer", "Web");
        let n = l.points.len();
        assert!(l.routed, "{side}");
        assert_eq!(dir(l.points[n - 2], l.points[n - 1]), into, "{side}: {:?}", l.points);
        assert_eq!(l.points[n - 1], a(side, None).point(Rect::from_centre(Point::new(20, 40), 12, 7)).scaled(), "{side}");
    }
}

/// Grid to px for these tests.
trait Scaled {
    fn scaled(self) -> Point;
}

impl Scaled for Point {
    fn scaled(self) -> Point {
        Point::new(self.x * 10, self.y * 10)
    }
}

#[test]
fn p4_207_corner_leaves_square_to_one_of_its_sides() {
    for (corner, ways) in [
        ("topLeft", [Dir::N, Dir::W]),
        ("topRight", [Dir::N, Dir::E]),
        ("bottomRight", [Dir::S, Dir::E]),
        ("bottomLeft", [Dir::S, Dir::W]),
    ] {
        let (m, b) = c2_build(&format!("            anchor web -> bank from {corner}"));
        let l = line(&m, &b, "Web", "Bank");
        assert!(l.routed, "{corner}");
        assert_eq!(l.points[0], a(corner, None).point(Rect::from_centre(Point::new(20, 40), 12, 7)).scaled(), "{corner}");
        let first = dir(l.points[0], l.points[1]);
        assert!(ways.contains(&first), "{corner}: leaves {first:?}");
        let r = b.routes.line(l.edge).expect("routed");
        let side = match first {
            Dir::N => Side::N,
            Dir::E => Side::E,
            Dir::S => Side::S,
            _ => Side::W,
        };
        assert_eq!(r.source.side, side, "{corner}: the port's side is the way it leaves");
    }
    // Bank lies right of and below web: from the bottom-right corner the
    // cheaper way is one of the two facing it.
    let (m, b) = c2_build("            anchor web -> bank from bottomRight");
    let l = line(&m, &b, "Web", "Bank");
    assert!([Dir::S, Dir::E].contains(&dir(l.points[0], l.points[1])));
}

#[test]
fn p4_208_rest_stays_routers_and_combines_with_vertices() {
    // Only the source pinned: the destination port is the router's, on a
    // side of db as without the anchor.
    let (m0, plain) = c2_build("");
    let (m, b) = c2_build("            anchor web -> db from left");
    let l = line(&m, &b, "Web", "Db");
    assert_eq!(l.points[0], Point::new(80, 400));
    let r = b.routes.line(l.edge).expect("routed");
    let r0 = plain.routes.line(line(&m0, &plain, "Web", "Db").edge).expect("routed");
    assert_ne!(r.points, r0.points);
    // With a vertex too: through the vertex, from and to the anchors.
    let (m, b) = c2_build("            via web -> db -100 600\n            anchor web -> db from left to left");
    let l = line(&m, &b, "Web", "Db");
    assert!(l.routed);
    assert_eq!(l.points[0], Point::new(80, 400));
    assert_eq!(*l.points.last().expect("end"), Point::new(80, 800));
    let v = Point::new(-100, 600);
    assert!(
        l.points.windows(2).any(|w| {
            let (p, q) = (w[0], w[1]);
            let (dx, dy, px, py) = (q.x - p.x, q.y - p.y, v.x - p.x, v.y - p.y);
            dx * py == dy * px && px * dx + py * dy >= 0 && px * px + py * py <= dx * dx + dy * dy
        }),
        "passes the vertex: {:?}",
        l.points
    );
}

#[test]
fn p4_209_anchored_line_never_trunk_or_joined() {
    let (_, plain) = c2_build("            bus on");
    assert!(plain.routes.lines.iter().flatten().any(|l| l.joins().is_some()), "the fixture joins");
    for pin in ["anchor web -> db to top", "anchor api -> db from left"] {
        let (_, b) = c2_build(&format!("            bus on\n            {pin}"));
        let pinned: Vec<_> = b.plan.edges.iter().filter(|e| e.source_anchor.is_some() || e.destination_anchor.is_some()).map(|e| e.id).collect();
        assert_eq!(pinned.len(), 1);
        for l in b.routes.lines.iter().flatten() {
            if pinned.contains(&l.edge) {
                assert!(l.joins().is_none(), "{pin}: never joins");
            }
            assert!(l.joins().is_none_or(|t| !pinned.contains(&t)), "{pin}: never a trunk");
        }
    }
}

#[test]
fn p4_210_no_anchor_scene_unchanged() {
    let m = parse(EXAMPLE);
    for key in ["C1", "C2", "C3-serving", "C3-startup"] {
        let golden = std::fs::read_to_string(format!("{GOLDEN}/{key}.svg")).expect("golden");
        assert!(export::svg(&build(&m, key).scene) == golden, "{key}: svg differs from its golden");
    }
    // An anchor in another view leaves this one as it was.
    let (_, plain) = c2_build("");
    let other = ws(
        "        container shop \"c2\" \"Shop\" {\n            include *\n            at customer 200 100\n            at admin 700 100\n            at web 200 400\n            at api 700 400\n            at db 200 800\n            at bank 700 800\n        }\n        container shop \"other\" {\n            include *\n            anchor web -> db from left\n        }",
    );
    assert_eq!(build(&parse(&other), "c2").scene, plain.scene);
    // A bad anchor line changes nothing either.
    let (_, ignored_line) = c2_build("            anchor db -> web from left");
    assert_eq!(ignored_line.scene, plain.scene);
}

/// A hub above `n` boxes in a row, each line from the hub's bottom side
/// to one box, the hub `width` px wide.
fn fan(n: usize, width: i32, pins: &[Option<u32>]) -> String {
    use std::fmt::Write as _;
    let mut model = String::from("        hub = softwareSystem \"Hub\"\n");
    let mut lines = String::new();
    let mut at = String::from("            at hub 800 200\n");
    let _ = writeln!(at, "            size hub {width} 140");
    for i in 0..n {
        let _ = writeln!(model, "        s{i} = softwareSystem \"S{i}\"");
        let _ = writeln!(model, "        hub -> s{i} \"Calls\"");
        let x = 800 + (i32::try_from(i).expect("small") - i32::try_from(n / 2).expect("small")) * 300;
        let _ = writeln!(at, "            at s{i} {x} 600");
        if let Some(Some(p)) = pins.get(i) {
            let _ = writeln!(lines, "            anchor hub -> s{i} from bottom {p}");
        }
    }
    format!("workspace {{\n    model {{\n{model}    }}\n    views {{\n        systemLandscape \"l\" {{\n            include *\n{at}{lines}        }}\n    }}\n}}\n")
}

#[test]
fn p4_211_neighbouring_pins_neither_cross_nor_overlap() {
    for (n, width, pins) in [
        (3, 240, vec![Some(25), Some(50), Some(75)]),
        (5, 440, vec![Some(17), Some(33), Some(50), Some(67), Some(83)]),
        // Some pinned, the rest left to the router.
        (5, 440, vec![Some(17), None, Some(50), None, Some(83)]),
    ] {
        let src = fan(n, width, &pins);
        let m = parse(&src);
        assert!(ignored(&m).is_empty(), "{:?}", ignored(&m));
        let b = build(&m, "l");
        assert!(b.scene.lines.iter().all(|l| l.routed), "{n}: every line routed");
        assert_eq!(b.scene.crossings, 0, "{n} lines from {width} px: no crossing {:?}", b.scene.lines.iter().map(|l| &l.points).collect::<Vec<_>>());
        let segs = |l: &SceneLine| l.points.windows(2).map(|w| Segment::new(w[0], w[1])).collect::<Vec<_>>();
        for (i, x) in b.scene.lines.iter().enumerate() {
            for y in &b.scene.lines[i + 1..] {
                for s in segs(x) {
                    for t in segs(y) {
                        assert_ne!(s.contact(&t), Contact::Overlap, "{s:?} runs along {t:?}");
                    }
                }
            }
        }
        // Each pinned line starts at its point.
        let hub = rect(&m, &b, "Hub");
        for (i, pin) in pins.iter().enumerate() {
            let Some(p) = pin else { continue };
            let l = line(&m, &b, "Hub", &format!("S{i}"));
            let x = hub.min.x + (hub.width() / 10 * i32::try_from(*p).expect("small") + 50) / 100 * 10;
            assert_eq!(l.points[0], Point::new(x, hub.max.y), "S{i}");
        }
    }
}

#[test]
fn p4_212_walled_in_pin_still_honoured() {
    // The vertex line api -> db runs along y = 490, 20 px under web's
    // bottom side, across the stub of web's bottom middle.
    let (m, b) = c2_build("            via api -> db 700 490 -60 490\n            anchor web -> db from bottom");
    let l = line(&m, &b, "Web", "Db");
    assert!(l.routed, "{:?}", b.report.findings.iter().map(|f| &f.message).collect::<Vec<_>>());
    assert_eq!(l.points[0], Point::new(200, 470));
    assert_eq!(dir(l.points[0], l.points[1]), Dir::S);
}

/// `SetAnchor` on view `c2` for `src -> dst`.
fn set_anchor(src: &str, dst: &str, from: Option<Anchor>, to: Option<Anchor>) -> edit::Edit {
    edit::Edit::SetAnchor {
        view: "c2".to_owned(),
        source: src.to_owned(),
        destination: dst.to_owned(),
        order: None,
        from,
        to,
    }
}

/// `edits` applied to `src`.
fn applied(src: &str, edits: &[edit::Edit]) -> edit::Applied {
    let parsed = model::parse(src).unwrap_or_else(|e| panic!("{e}"));
    edit::apply(&parsed, edits).unwrap_or_else(|e| panic!("{e:?}"))
}

#[test]
fn p4_215_set_anchor_adds_after_last_layout_line() {
    let src = c2("");
    let done = applied(&src, &[set_anchor("web", "db", Some(a("bottom", None)), None)]);
    assert!(done.text.contains("            at bank 700 800\n            anchor web -> db from bottom\n"), "{}", done.text);
    assert_eq!(applied(&done.text, &done.inverse).text, src, "the inverse removes it, byte for byte");
    // After the view's `via` lines, and with the other anchor lines.
    let src = c2("            via web -> db 0 600");
    let done = applied(&src, &[set_anchor("web", "db", None, Some(a("topLeft", None)))]);
    assert!(done.text.contains("via web -> db 0 600\n            anchor web -> db to topLeft\n"), "{}", done.text);
    let two = applied(&done.text, &[set_anchor("api", "db", Some(a("left", Some(25))), Some(a("right", None)))]);
    assert!(two.text.contains("anchor web -> db to topLeft\n            anchor api -> db from left 25 to right\n"), "{}", two.text);
    // Rewrite: only what follows the ends changes; the other pin is kept
    // because the edit names both ends.
    let src = c2("            anchor web -> db from bottom");
    let done = applied(&src, &[set_anchor("web", "db", Some(a("bottom", None)), Some(a("top", Some(75))))]);
    assert_eq!(done.text, src.replace("anchor web -> db from bottom", "anchor web -> db from bottom to top 75"));
    assert_eq!(done.inverse, [set_anchor("web", "db", Some(a("bottom", None)), None)]);
    assert_eq!(applied(&done.text, &done.inverse).text, src);
    // A dynamic step by number.
    let src = dynamic("");
    let done = applied(
        &src,
        &[edit::Edit::SetAnchor {
            view: "flow".to_owned(),
            source: "web".to_owned(),
            destination: "db".to_owned(),
            order: Some("3".to_owned()),
            from: Some(a("left", None)),
            to: None,
        }],
    );
    assert!(done.text.contains("            at db 200 800\n            anchor 3: web -> db from left\n"), "{}", done.text);
    // A line the view does not draw is refused.
    let parsed = model::parse(&c2("")).expect("parses");
    assert!(edit::apply(&parsed, &[set_anchor("db", "web", Some(a("top", None)), None)]).is_err());
}

#[test]
fn p4_216_set_anchor_removes_line_when_no_pin_left() {
    let src = c2("            anchor web -> db from bottom");
    let gone = applied(&src, &[set_anchor("web", "db", None, None)]);
    assert_eq!(gone.text, src.replace("            anchor web -> db from bottom\n", ""));
    assert_eq!(gone.inverse, [set_anchor("web", "db", Some(a("bottom", None)), None)]);
    assert_eq!(applied(&gone.text, &gone.inverse).text, src);
    // The line is the router's again.
    let (_, plain) = c2_build("");
    let after = build(&parse(&gone.text), "c2");
    assert_eq!(after.scene.lines, plain.scene.lines);
    // Removing what is not there changes nothing.
    let none = applied(&c2(""), &[set_anchor("web", "db", None, None)]);
    assert_eq!(none.text, c2(""));
}

#[test]
fn p4_214_drag_preview_rebuild() {
    let never = AtomicBool::new(false);
    let (m, b) = c2_build("");
    let edge = line(&m, &b, "Web", "Db").edge;
    let config = pipeline::PREVIEW_ROUTE;
    let moved = pipeline::rebuild_with_anchors(&m, &b, edge, Some(a("left", None)), None, &config, &never).expect("never stopped");
    assert_eq!(line(&m, &moved, "Web", "Db").points[0], Point::new(80, 400));
    assert_eq!(moved.plan.edges[edge.0 as usize].source_anchor, Some(a("left", None)));
    assert_eq!(moved.routes.centres, b.routes.centres, "no box moves");
    // The same anchors give `previous`; a set stop flag yields nothing.
    let same = pipeline::rebuild_with_anchors(&m, &b, edge, None, None, &config, &never).expect("never stopped");
    assert_eq!(same.scene, b.scene);
    assert!(pipeline::rebuild_with_anchors(&m, &b, edge, Some(a("left", None)), None, &config, &AtomicBool::new(true)).is_err());
}

#[test]
fn p4_219_export_draws_scene_lines() {
    let (m, b) = c2_build("            anchor web -> db from left to bottomLeft");
    let l = line(&m, &b, "Web", "Db");
    let d: Vec<String> = l
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}{},{}", if i == 0 { 'M' } else { 'L' }, p.x, p.y))
        .collect();
    let svg = export::svg(&b.scene);
    assert!(svg.contains(&format!("d=\"{}\"", d.join(" "))), "the SVG draws the scene's line: {d:?}");
}

#[test]
fn p4_220_json_export_has_no_anchors() {
    let json = |src: &str| {
        let m = parse(src);
        let all = pipeline::build_all(&m, &AtomicBool::new(false)).expect("never stopped");
        export::json(&m, &all, Some(src))
    };
    let with = json(&c2("            anchor web -> db from left"));
    assert!(!with.contains("anchor"), "Structurizr has no field for anchors");
}
