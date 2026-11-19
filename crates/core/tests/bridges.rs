//! Tests of the bridges row (SRS 4.8: P4-500..P4-520, P4-521 and P4-522
//! edits), core part. One test per requirement at least, named after it.
//! The editor tests (P4-521..P4-523) are in
//! `crates/editor/src/bridges_tests.rs`; tests on hand-made geometry
//! (P4-515, P4-516) are also in `bridge.rs`.
//!
//! The fixtures are a ring of four boxes (two tall, two wide, touching)
//! with two lines pinned inside it, so the router has to cross them; the
//! anchors set where and at which angle.

// Tests name the model `m`, the build `b`, a line `l`, as the other rows do.
#![allow(clippy::many_single_char_names, clippy::similar_names, clippy::too_many_lines)]

use std::sync::atomic::AtomicBool;

use c4rizer_core::bridge::{self, BAR_HALF_PX, GAP_HALF_SLANT_PX, GAP_HALF_SQUARE_PX, HOP_PX};
use c4rizer_core::edit::{self, Edit, LineRef};
use c4rizer_core::export;
use c4rizer_core::geom::{Contact, Point, Segment};
use c4rizer_core::lint;
use c4rizer_core::model::{self, CrossingStyle, Model, NoticeKind};
use c4rizer_core::pipeline::{self, ViewBuild};
use c4rizer_core::place::Keep;
use c4rizer_core::scene::{ItemKey, Scene, SceneBridge, SceneLine};
use c4rizer_core::view::EdgeId;

const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl"));
const WORKSPACE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example/workspace.dsl"));
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/golden");

/// The ring: `{desc}` follows each relationship, `{cd}` and `{ab}` are the
/// anchors of `c -> d` and `a -> b`, `{lines}` more view lines.
const RING: &str = r#"workspace "Ring" {
    model {
        s = softwareSystem "S" "Does things." {
            a = container "A" "West." "Rust"
            b = container "B" "East." "Rust"
            c = container "C" "North." "Rust"
            d = container "D" "South." "Rust"
            u = container "U" "Aside." "Rust"
        }
        a -> b{desc}
        c -> d{desc}
    }
    views {
        container s "ring" "Ring" {
            include a b c d u
            maxCrossings 1
            at a 300 500
            at b 1100 500
            at c 700 100
            at d 700 900
            at u 2000 500
            size c 2400 100
            size d 2400 100
            size a 200 900
            size b 200 900
            anchor c -> d {cd}
            anchor a -> b {ab}
{lines}
        }
    }
}
"#;

/// Square crossing of a horizontal `a -> b` and a vertical `c -> d` at
/// (630, 410).
const ORTHO: (&str, &str) = ("from bottom 47 to top 47", "from right 40 to left 40");
/// Two diagonals crossing square, the owner's X, at (810, 570).
const X: (&str, &str) = ("from bottom 56 to top 44", "from right 30 to left 75");
/// A horizontal `a -> b` across the diagonal of `c -> d` at 45°, at
/// (730, 650).
const SLANT: (&str, &str) = ("from bottom 56 to top 44", "from right 67 to left 67");
/// Both lines through the middles: their chevrons meet at the crossing.
const MIDDLE: (&str, &str) = ("from bottom to top", "from right to left");

/// Three lines on a bus from the top row to the bottom row (tracks at x
/// 880, 900, 920), crossed by `w -> e` at y 490; `{lines}` more view lines.
const BUS: &str = r#"workspace "Bus" {
    model {
        s = softwareSystem "S" "Does things." {
            a = container "A" "Top one." "Rust"
            b = container "B" "Top two." "Rust"
            c = container "C" "Top three." "Rust"
            x = container "X" "Bottom one." "Rust"
            y = container "Y" "Bottom two." "Rust"
            z = container "Z" "Bottom three." "Rust"
            w = container "W" "West." "Rust"
            e = container "E" "East." "Rust"
        }
        a -> x
        b -> y
        c -> z
        w -> e
    }
    views {
        container s "bus" "Bus" {
            include *
            maxCrossings 3
            at a 300 100
            at b 900 100
            at c 1500 100
            at x 300 1100
            at y 900 1100
            at z 1500 1100
            at w -100 600
            at e 2000 600
            size a 600 100
            size b 600 100
            size c 600 100
            size x 600 100
            size y 600 100
            size z 600 100
            size w 200 1100
            size e 200 1100
            bundle a -> x b -> y c -> z
            anchor w -> e from right 40 to left 40
{lines}
        }
    }
}
"#;

/// `lines`, each as a view line.
fn indented(lines: &[&str]) -> String {
    lines.iter().fold(String::new(), |mut out, l| {
        out.push_str("            ");
        out.push_str(l);
        out.push('\n');
        out
    })
}

fn ring(anchors: (&str, &str), desc: bool, lines: &[&str]) -> String {
    let text = indented(lines);
    RING.replace("{desc}", if desc { " \"Feeds\"" } else { "" })
        .replace("{cd}", anchors.0)
        .replace("{ab}", anchors.1)
        .replace("{lines}\n", &text)
}

fn bus(lines: &[&str]) -> String {
    let text = indented(lines);
    BUS.replace("{lines}\n", &text)
}

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

fn scene_of(src: &str, key: &str) -> Scene {
    build(&parse(src), key).scene
}

fn line(s: &Scene, e: u32) -> &SceneLine {
    s.lines.iter().find(|l| l.edge == EdgeId(e)).expect("line")
}

fn only_bridge(s: &Scene) -> &SceneBridge {
    assert_eq!(s.bridges.len(), 1, "{:?}", s.bridges);
    &s.bridges[0]
}

fn d(p: Point, q: Point) -> f64 {
    f64::from(p.x - q.x).hypot(f64::from(p.y - q.y))
}

/// Distance from `p` to the segment `a..b`.
fn seg_dist(p: (f64, f64), a: Point, b: Point) -> f64 {
    let (ax, ay, bx, by) = (f64::from(a.x), f64::from(a.y), f64::from(b.x), f64::from(b.y));
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let k = if l2 == 0.0 { 0.0 } else { (((p.0 - ax) * dx + (p.1 - ay) * dy) / l2).clamp(0.0, 1.0) };
    (ax + dx * k - p.0).hypot(ay + dy * k - p.1)
}

/// Distance from the segment `p..q` to the polyline `pts`, sampled every
/// quarter px along `p..q`.
fn poly_dist(p: Point, q: Point, pts: &[Point]) -> f64 {
    let n = (d(p, q) * 4.0).ceil().max(1.0);
    let mut best = f64::INFINITY;
    let mut i = 0.0;
    while i <= n {
        let k = i / n;
        let s = (f64::from(p.x) + f64::from(q.x - p.x) * k, f64::from(p.y) + f64::from(q.y - p.y) * k);
        for w in pts.windows(2) {
            best = best.min(seg_dist(s, w[0], w[1]));
        }
        i += 1.0;
    }
    best
}

/// The gap of a carrier with one gap: (end of run 0, start of run 1).
fn gap_ends(l: &SceneLine) -> (Point, Point) {
    assert_eq!(l.runs.len(), 2, "{:?}", l.runs);
    (*l.runs[0].last().expect("run"), l.runs[1][0])
}

/// `src` with `line` as the first line of the view block whose head
/// contains `head`.
fn in_view(src: &str, head: &str, line: &str) -> String {
    let at = src.find(head).expect("view head");
    let brace = at + src[at..].find('{').expect("brace");
    format!("{}{{\n            {line}{}", &src[..brace], &src[brace + 1..])
}

// ------------------------------------------------------------- setting

#[test]
fn p4_500_crossings_setting() {
    let m = parse(&ring(ORTHO, false, &[]));
    assert_eq!(m.view("ring").expect("view").options().crossings, CrossingStyle::Gap);
    for (word, style) in [("jump", CrossingStyle::Jump), ("PLAIN", CrossingStyle::Plain), ("Gap", CrossingStyle::Gap)] {
        let m = parse(&ring(ORTHO, false, &[&format!("crossings {word}")]));
        assert_eq!(m.view("ring").expect("view").options().crossings, style, "{word}");
    }
    let err = model::parse(&ring(ORTHO, false, &["crossings wavy"])).expect_err("an unknown style is an error");
    assert!(err.to_string().contains("crossings"), "{err}");
    // `plain` is the drawing without this row: no runs, no bars, no list.
    let s = scene_of(&ring(ORTHO, false, &["crossings plain"]), "ring");
    assert_eq!(s.crossings, 1);
    assert!(s.bridges.is_empty());
    assert!(s.lines.iter().all(|l| l.runs.is_empty() && l.bars.is_empty()));
}

#[test]
fn p4_501_bridge_line_notices() {
    let bad = [
        "bridge a -> b",
        "bridge a -> b c -> d a -> b",
        "bridge a -> nowhere c -> d",
        "bridge a -> c c -> d",
        "bridge a -> b a -> b",
        "bridge 1: a -> b c -> d",
        "bridge a b c",
    ];
    for line in bad {
        let p = model::parse(&ring(ORTHO, false, &[line])).expect("a bad bridge line never fails the file");
        let notes: Vec<_> = p.model.notices.iter().filter(|n| n.kind == NoticeKind::BridgeIgnored).collect();
        assert_eq!(notes.len(), 1, "{line}: {notes:?}");
        assert!(p.model.view("ring").expect("view").bridges.is_empty(), "{line}");
    }
    // The same pair again, in either order: the first wins.
    let p = model::parse(&ring(ORTHO, false, &["bridge c -> d a -> b", "bridge a -> b c -> d"])).expect("parses");
    assert_eq!(p.model.notices.iter().filter(|n| n.kind == NoticeKind::BridgeIgnored).count(), 1);
    assert_eq!(p.model.view("ring").expect("view").bridges.len(), 1);
    // A pair that does not cross is kept, unused, silently.
    let p = model::parse(&bus(&["bridge a -> x c -> z"])).expect("parses");
    assert!(p.model.notices.iter().all(|n| n.kind != NoticeKind::BridgeIgnored));
    assert_eq!(p.model.view("bus").expect("view").bridges.len(), 1);
    assert_eq!(scene_of(&bus(&["bridge a -> x c -> z"]), "bus").bridges, scene_of(&bus(&[]), "bus").bridges);
}

#[test]
fn p4_502_plan_bridges() {
    let b = build(&parse(&ring(ORTHO, false, &["bridge c -> d a -> b"])), "ring");
    assert_eq!(b.plan.bridges, vec![(EdgeId(1), EdgeId(0))]);
    assert!(build(&parse(&ring(ORTHO, false, &[])), "ring").plan.bridges.is_empty());
}

// -------------------------------------------------------------- carrier

#[test]
fn p4_503_scene_lists_each_crossing() {
    let s = scene_of(&ring(ORTHO, false, &[]), "ring");
    let site = only_bridge(&s);
    assert_eq!(site.at, Point::new(630, 410));
    assert!(site.drawn);
    // Jump style lists the same crossing.
    let j = scene_of(&ring(ORTHO, false, &["crossings jump"]), "ring");
    assert_eq!(only_bridge(&j).at, site.at);
    // A `jump true` hop (P2-422) keeps its own mark.
    let s = scene_of(&ring(ORTHO, false, &["line a -> b jump on"]), "ring");
    assert!(!line(&s, 0).jumps.is_empty(), "the fixture hops");
    assert!(s.bridges.is_empty(), "{:?}", s.bridges);
    assert!(line(&s, 0).runs.is_empty() && line(&s, 1).runs.is_empty());
}

#[test]
fn p4_504_default_carrier() {
    let s = scene_of(&ring(ORTHO, false, &[]), "ring");
    let site = only_bridge(&s);
    assert_eq!((site.carrier, site.passing), (EdgeId(0), EdgeId(1)), "the lower EdgeId carries");
    assert!(site.by_default);
    // An unrelated box moved: same carrier.
    let moved = ring(ORTHO, false, &[]).replace("at u 2000 500", "at u 2000 1400");
    assert_eq!(only_bridge(&scene_of(&moved, "ring")).carrier, EdgeId(0));
    // A single line crossing a bus carries.
    let s = scene_of(&bus(&[]), "bus");
    assert_eq!(s.bridges.len(), 3);
    for site in &s.bridges {
        assert_eq!(site.carrier, EdgeId(3), "{site:?}");
        assert!(site.by_default);
    }
}

#[test]
fn p4_505_bridge_line_picks_the_carrier() {
    let s = scene_of(&ring(ORTHO, false, &["bridge c -> d a -> b"]), "ring");
    let site = only_bridge(&s);
    assert_eq!((site.carrier, site.passing), (EdgeId(1), EdgeId(0)));
    assert!(!site.by_default);
    assert!(line(&s, 0).runs.is_empty(), "the passing line is whole");
    assert!(!line(&s, 1).runs.is_empty());
    // Naming the default carrier keeps it.
    let s = scene_of(&ring(ORTHO, false, &["bridge a -> b c -> d"]), "ring");
    assert_eq!(only_bridge(&s).carrier, EdgeId(0));
    assert!(only_bridge(&s).by_default);
}

// ------------------------------------------------------------------ gap

#[test]
fn p4_506_gap_square_and_slanted() {
    let s = scene_of(&ring(ORTHO, false, &[]), "ring");
    let c = line(&s, 0);
    assert_eq!(gap_ends(c), (Point::new(630 - GAP_HALF_SQUARE_PX, 410), Point::new(630 + GAP_HALF_SQUARE_PX, 410)));
    assert_eq!(GAP_HALF_SQUARE_PX * 2, 16);
    assert_eq!(c.bars, vec![[Point::new(622, 415), Point::new(622, 405)], [Point::new(638, 415), Point::new(638, 405)]]);
    assert!(c.bars.iter().all(|bar| (d(bar[0], bar[1]) - f64::from(BAR_HALF_PX * 2)).abs() < 1e-9));
    assert_eq!(BAR_HALF_PX * 2, 10);
    // The runs join up to the route.
    assert_eq!(c.runs[0][0], c.points[0]);
    assert_eq!(c.runs[1].last(), c.points.last());
    assert!(line(&s, 1).runs.is_empty() && line(&s, 1).bars.is_empty(), "the passing line is whole");
    // Bars in the carrier's colour and width, solid.
    let svg = export::svg(&s);
    let bar = format!(
        "<path d=\"M622,415 L622,405\" fill=\"none\" stroke=\"#{:02x}{:02x}{:02x}\" stroke-width=\"{}\" stroke-linejoin=\"round\"/>",
        c.stroke.colour.r, c.stroke.colour.g, c.stroke.colour.b, c.stroke.width_px
    );
    assert!(svg.contains(&bar), "{svg}");
    // 45°: 28px.
    let s = scene_of(&ring(SLANT, false, &[]), "ring");
    assert_eq!(only_bridge(&s).at, Point::new(730, 650));
    assert_eq!(gap_ends(line(&s, 0)), (Point::new(730 - GAP_HALF_SLANT_PX, 650), Point::new(730 + GAP_HALF_SLANT_PX, 650)));
    assert_eq!(GAP_HALF_SLANT_PX * 2, 28);
}

#[test]
fn p4_507_bars_clear_of_the_passing_line() {
    let cases = [
        ring(ORTHO, false, &[]),
        ring(X, false, &[]),
        ring(SLANT, false, &[]),
        ring(SLANT, false, &["bridge c -> d a -> b"]),
        ring(X, false, &["bridge c -> d a -> b"]),
        bus(&[]),
        bus(&["bridge a -> x w -> e"]),
    ];
    let mut bars = 0;
    for src in &cases {
        let m = parse(src);
        let key = &m.views[0].key;
        let s = build(&m, key).scene;
        for site in s.bridges.iter().filter(|b| b.drawn) {
            let c = s.lines.iter().find(|l| l.edge == site.carrier).expect("carrier");
            let p = s.lines.iter().find(|l| l.edge == site.passing).expect("passing");
            for bar in &c.bars {
                bars += 1;
                let gap = poly_dist(bar[0], bar[1], &p.points);
                assert!(gap >= 3.0, "{key}: bar {bar:?} {gap:.2}px from {:?}", p.edge);
            }
        }
    }
    assert!(bars >= 14, "{bars}");
}

// ----------------------------------------------------------------- jump

#[test]
fn p4_508_jump_shape_and_side() {
    let s = scene_of(&ring(ORTHO, false, &["crossings jump"]), "ring");
    let c = line(&s, 0);
    assert!(c.bars.is_empty());
    assert_eq!(c.runs.len(), 1);
    let hop = [Point::new(622, 410), Point::new(622, 410 - HOP_PX), Point::new(638, 410 - HOP_PX), Point::new(638, 410)];
    assert!(c.runs[0].windows(4).any(|w| w == hop), "{:?}", c.runs[0]);
    assert_eq!(c.runs[0][0], c.points[0]);
    assert_eq!(c.runs[0].last(), c.points.last());
    // A vertical carrier hops to the left.
    let s = scene_of(&ring(ORTHO, false, &["crossings jump", "bridge c -> d a -> b"]), "ring");
    let c = line(&s, 1);
    let hop = [Point::new(630, 402), Point::new(622, 402), Point::new(622, 418), Point::new(630, 418)];
    assert!(c.runs[0].windows(4).any(|w| w == hop), "{:?}", c.runs[0]);
    // The passing line crosses the middle side only: legs clear of it.
    let p = &line(&s, 0).points;
    assert!(poly_dist(hop[0], hop[1], p) >= 3.0 && poly_dist(hop[2], hop[3], p) >= 3.0);
    // Right angles, no arc.
    assert!(!export::svg(&s).contains(" A"));
}

#[test]
fn p4_509_diagonal_x_gap_and_jump() {
    let s = scene_of(&ring(X, false, &[]), "ring");
    let site = only_bridge(&s).clone();
    assert_eq!(site.at, Point::new(810, 570));
    let c = s.lines.iter().find(|l| l.edge == site.carrier).expect("carrier");
    let (p, q) = gap_ends(c);
    // On the carrier's own diagonal, equal steps: 8px each side is 6 per axis.
    assert_eq!((p.x - p.y, q.x - q.y), (240, 240), "on the line x - y = 240");
    assert_eq!((q.x - p.x, q.y - p.y), (12, 12));
    for bar in &c.bars {
        assert_eq!(((bar[1].x - bar[0].x).abs(), (bar[1].y - bar[0].y).abs()), (8, 8), "{bar:?}");
        assert_eq!((bar[1].x - bar[0].x) + (bar[1].y - bar[0].y), 0, "square to the carrier");
    }
    // Jump: the hop stands out 6px on each axis, on the upper side.
    let s = scene_of(&ring(X, false, &["crossings jump"]), "ring");
    let c = s.lines.iter().find(|l| l.edge == site.carrier).expect("carrier");
    let run = &c.runs[0];
    let k = run.iter().position(|&r| r == p).expect("hop start");
    assert_eq!(&run[k..k + 4], &[p, Point::new(p.x + 6, p.y - 6), Point::new(q.x + 6, q.y - 6), q]);
}

// ----------------------------------------------------------------- buses

#[test]
fn p4_510_bus_carries_gap_fused_bars() {
    let s = scene_of(&bus(&["bridge a -> x w -> e"]), "bus");
    assert_eq!(s.bridge_fallbacks(), 0);
    // Every track cut at the same place.
    for e in 0..3 {
        let (p, q) = gap_ends(line(&s, e));
        assert_eq!((p.y, q.y), (482, 498), "track {e}");
    }
    // On each side the bars meet halfway: one bar from 875 to 925.
    for y in [482, 498] {
        let mut row: Vec<(i32, i32)> = (0..3)
            .flat_map(|e| line(&s, e).bars.clone())
            .filter(|b| b[0].y == y && b[1].y == y)
            .map(|b| (b[0].x.min(b[1].x), b[0].x.max(b[1].x)))
            .collect();
        row.sort_unstable();
        assert_eq!(row.len(), 3, "{row:?}");
        assert_eq!(row[0].0, 880 - BAR_HALF_PX);
        assert_eq!(row[2].1, 920 + BAR_HALF_PX);
        assert!(row.windows(2).all(|w| w[0].1 == w[1].0), "continuous: {row:?}");
    }
    // The tracks themselves still do not touch.
    for e in 0..3 {
        for f in e + 1..3 {
            for w in line(&s, e).points.windows(2) {
                for v in line(&s, f).points.windows(2) {
                    assert_eq!(Segment::new(w[0], w[1]).contact(&Segment::new(v[0], v[1])), Contact::Disjoint);
                }
            }
        }
    }
    assert!(line(&s, 3).runs.is_empty(), "the single line passes whole");
}

#[test]
fn p4_511_line_carries_gap_over_the_bus() {
    let s = scene_of(&bus(&[]), "bus");
    let c = line(&s, 3);
    assert_eq!(gap_ends(c), (Point::new(880 - GAP_HALF_SQUARE_PX, 490), Point::new(920 + GAP_HALF_SQUARE_PX, 490)));
    assert_eq!(c.bars.len(), 2);
    assert!((0..3).all(|e| line(&s, e).runs.is_empty()));
}

#[test]
fn p4_512_bus_carries_jump_side_by_side() {
    let s = scene_of(&bus(&["crossings jump", "bridge b -> y w -> e"]), "bus");
    for (e, x) in [(0, 880), (1, 900), (2, 920)] {
        let run = &line(&s, e).runs[0];
        let hop = [Point::new(x, 482), Point::new(x - HOP_PX, 482), Point::new(x - HOP_PX, 498), Point::new(x, 498)];
        assert!(run.windows(4).any(|w| w == hop), "track {e}: {run:?}");
    }
}

#[test]
fn p4_513_line_carries_jump_over_the_bus() {
    let s = scene_of(&bus(&["crossings jump"]), "bus");
    let run = &line(&s, 3).runs[0];
    let hop = [Point::new(872, 490), Point::new(872, 482), Point::new(928, 482), Point::new(928, 490)];
    assert!(run.windows(4).any(|w| w == hop), "{run:?}");
    assert_eq!(s.bridges.len(), 3);
}

#[test]
fn p4_514_the_choice_applies_to_the_whole_bus() {
    // Naming the middle member flips all three tracks.
    let s = scene_of(&bus(&["bridge b -> y w -> e"]), "bus");
    for site in &s.bridges {
        assert_eq!(site.passing, EdgeId(3), "{site:?}");
        assert!(!site.by_default);
    }
    assert!((0..3).all(|e| line(&s, e).runs.len() == 2));
}

// --------------------------------------------------------- crowded cases

#[test]
fn p4_515_parallel_crossings_merge() {
    // Three parallel tracks 20px apart: one gap, not three.
    let s = scene_of(&bus(&[]), "bus");
    assert_eq!(line(&s, 3).runs.len(), 2);
    assert!(s.bridges.iter().all(|b| b.drawn));
}

#[test]
fn p4_516_fallbacks_label_and_chevron() {
    // A label beside the crossing: drawn plain, counted.
    let s = scene_of(&ring(X, true, &[]), "ring");
    assert_eq!(s.bridge_fallbacks(), 1, "{:?}", s.bridges);
    assert!(s.lines.iter().all(|l| l.runs.is_empty()));
    // Chevrons of both lines on the crossing: the bridge wins, they slide
    // along their own lines, clear of it; with `plain` they stay.
    let s = scene_of(&ring(MIDDLE, false, &[]), "ring");
    let plain = scene_of(&ring(MIDDLE, false, &["crossings plain"]), "ring");
    assert_eq!(s.crossings, 1);
    assert_eq!(s.bridge_fallbacks(), 0, "{:?}", s.bridges);
    let at = only_bridge(&s).at;
    for e in 0..2 {
        let (moved, kept) = (&line(&s, e).chevrons, &line(&plain, e).chevrons);
        assert_eq!(moved.len(), kept.len(), "slid, not removed");
        assert!(kept.iter().any(|c| d(c.at, at) < 1.0), "the fixture has a chevron on the crossing");
        for c in moved {
            let carrier = line(&s, only_bridge(&s).carrier.0);
            let (p, q) = gap_ends(carrier);
            let marks: Vec<[Point; 2]> = carrier.bars.iter().copied().chain([[p, q]]).collect();
            for m in &marks {
                assert!(poly_dist(m[0], m[1], &c.points) >= 7.99, "{c:?} near {m:?}");
            }
            assert!(d(c.at, at) > 1.0);
            let on_route = line(&s, e).points.windows(2).any(|w| poly_dist(c.at, c.at, &[w[0], w[1]]) < 0.01);
            assert!(on_route, "on its own line");
        }
    }
    // No bridge ever lies on a label, in either label mode.
    for mode in ["labels line", "labels callout"] {
        for anchors in [ORTHO, X, SLANT] {
            let s = scene_of(&ring(anchors, true, &[mode]), "ring");
            for l in &s.lines {
                for lab in &s.labels {
                    let r = lab.rect.inflate(2);
                    for bar in &l.bars {
                        assert!(!r.contains(bar[0]) && !r.contains(bar[1]), "{mode}: {bar:?} in {:?}", lab.rect);
                    }
                    for w in l.runs.windows(2) {
                        let (end, next) = (*w[0].last().expect("run"), w[1][0]);
                        let mid = Point::new(i32::midpoint(end.x, next.x), i32::midpoint(end.y, next.y));
                        assert!(!r.contains(mid), "{mode}: a gap under {:?}", lab.rect);
                    }
                }
            }
        }
    }
}

#[test]
fn p4_517_fallback_counts_on_the_examples() {
    let mut counts = Vec::new();
    for (name, src) in [("example", EXAMPLE), ("workspace", WORKSPACE)] {
        let m = parse(src);
        let all = pipeline::build_all(&m, &AtomicBool::new(false)).expect("never stopped");
        for b in &all.views {
            let s = &b.scene;
            let n = s.bridges.iter().filter(|x| !x.drawn).count();
            assert_eq!(s.bridge_fallbacks() as usize, n);
            counts.push(format!("{name}/{}: {} crossings, {} fallbacks", s.key, s.bridges.len(), n));
        }
    }
    let counts = counts.join("\n");
    println!("{counts}");
    assert!(counts.contains("example/C3-serving: 1 crossings, 0 fallbacks"), "{counts}");
    assert!(counts.contains("workspace/C3-serving: 1 crossings, 0 fallbacks"), "{counts}");
    assert!(counts.contains("workspace/C3-startup: 8 crossings, 2 fallbacks"), "{counts}");
}

// ------------------------------------------------------ drawing and export

#[test]
fn p4_518_drawing_only() {
    for anchors in [ORTHO, X, SLANT] {
        let gap = build(&parse(&ring(anchors, false, &[])), "ring");
        let plain = build(&parse(&ring(anchors, false, &["crossings plain"])), "ring");
        assert_eq!(gap.routes, plain.routes);
        assert_eq!(gap.scene.crossings, plain.scene.crossings);
        let points = |s: &Scene| s.lines.iter().map(|l| l.points.clone()).collect::<Vec<_>>();
        assert_eq!(points(&gap.scene), points(&plain.scene));
        let m = parse(&ring(anchors, false, &[]));
        let findings = |s: &Scene| lint::lint(&m, s).findings.iter().map(|f| format!("{f:?}")).collect::<Vec<_>>();
        assert_eq!(findings(&gap.scene), findings(&plain.scene));
    }
    // A click inside the gap, off the passing line, still hits the carrier.
    let s = scene_of(&ring(ORTHO, false, &[]), "ring");
    assert_eq!(s.hit(Point::new(625, 410), 2), Some(ItemKey::Line(EdgeId(0))));
    assert_eq!(s.hit(Point::new(635, 410), 2), Some(ItemKey::Line(EdgeId(0))));
}

#[test]
fn p4_519_unchanged_without_crossings() {
    let m = parse(EXAMPLE);
    let plain = parse(&EXAMPLE.replace("include *", "include *\n        crossings plain"));
    for key in ["C1", "C2", "C3-startup"] {
        let a = build(&m, key).scene;
        assert!(a.bridges.is_empty() && a.lines.iter().all(|l| l.runs.is_empty()));
        assert_eq!(export::svg(&a), export::svg(&build(&plain, key).scene), "{key}");
    }
}

#[test]
fn p4_520_golden_diff_confined_to_the_carrier() {
    let gap = scene_of(EXAMPLE, "C3-serving");
    let plain = scene_of(&in_view(EXAMPLE, "component app \"C3-serving\"", "crossings plain"), "C3-serving");
    let golden = std::fs::read_to_string(format!("{GOLDEN}/C3-serving.svg")).expect("golden");
    assert_eq!(export::svg(&gap), golden, "the golden is today's output");
    let g: Vec<String> = export::svg(&gap).lines().map(str::to_owned).collect();
    let p: Vec<String> = export::svg(&plain).lines().map(str::to_owned).collect();
    assert_eq!(g.len(), p.len());
    let differ: Vec<usize> = (0..g.len()).filter(|&i| g[i] != p[i]).collect();
    assert_eq!(differ.len(), 1, "{differ:?}");
    let site = only_bridge(&gap);
    let c = gap.lines.iter().find(|l| l.edge == site.carrier).expect("carrier");
    // The plain group with its route path replaced by the runs and bars.
    let path = |pts: &[Point]| {
        let d: Vec<String> = pts.iter().enumerate().map(|(i, q)| format!("{}{},{}", if i == 0 { "M" } else { "L" }, q.x, q.y)).collect();
        format!("<path d=\"{}\" fill=\"none\"", d.join(" "))
    };
    let i = differ[0];
    assert!(p[i].starts_with("<g class=\"line\">"), "{}", p[i]);
    let route = path(&c.points);
    let style = " stroke=\"#2e7d32\" stroke-width=\"2\" stroke-linejoin=\"round\"/>";
    assert!(p[i].contains(&format!("{route}{style}")), "{}", p[i]);
    let pieces: Vec<Vec<Point>> = c.runs.iter().cloned().chain(c.bars.iter().map(|b| b.to_vec())).collect();
    let new = pieces.iter().fold(String::new(), |mut out, pts| {
        out.push_str(&path(pts));
        out.push_str(style);
        out
    });
    assert_eq!(g[i], p[i].replacen(&format!("{route}{style}"), &new, 1));
    assert_eq!((c.runs.len(), c.bars.len()), (2, 2));
    // The other goldens are untouched.
    for key in ["C1", "C2", "C3-startup"] {
        let golden = std::fs::read_to_string(format!("{GOLDEN}/{key}.svg")).expect("golden");
        assert_eq!(export::svg(&scene_of(EXAMPLE, key)), golden, "{key}");
    }
}

// ------------------------------------------------------------------- edit

#[test]
fn p4_521_set_crossing_style_edit_and_inverse() {
    let src = ring(ORTHO, false, &[]);
    let done = edit::apply(
        &model::parse(&src).expect("parses"),
        &[Edit::SetCrossingStyle {
            view: "ring".to_owned(),
            style: Some(CrossingStyle::Jump),
        }],
    )
    .expect("applies");
    assert!(done.text.contains("maxCrossings 1\n            crossings jump\n"), "{}", done.text);
    let back = edit::apply(&model::parse(&done.text).expect("parses"), &done.inverse).expect("applies");
    assert_eq!(back.text, src);
    let again = edit::apply(
        &model::parse(&done.text).expect("parses"),
        &[Edit::SetCrossingStyle {
            view: "ring".to_owned(),
            style: None,
        }],
    )
    .expect("applies");
    assert_eq!(again.text, src);
}

#[test]
fn p4_522_set_bridges_edit_flip_and_inverse() {
    let src = ring(ORTHO, false, &[]);
    let done = edit::apply(
        &model::parse(&src).expect("parses"),
        &[Edit::SetBridges {
            view: "ring".to_owned(),
            bridges: vec![(LineRef::new("c", "d", None), LineRef::new("a", "b", None))],
        }],
    )
    .expect("applies");
    assert!(done.text.contains("anchor a -> b from right 40 to left 40\n            bridge c -> d a -> b\n"), "{}", done.text);
    assert_eq!(only_bridge(&scene_of(&done.text, "ring")).carrier, EdgeId(1));
    let back = edit::apply(&model::parse(&done.text).expect("parses"), &done.inverse).expect("applies");
    assert_eq!(back.text, src);
    // The flip from the default adds the line; back removes it.
    let b = build(&parse(&src), "ring");
    let site = only_bridge(&b.scene).clone();
    assert_eq!(bridge::flipped(&b.plan, &b.scene, &site), vec![(EdgeId(1), EdgeId(0))]);
    let b = build(&parse(&done.text), "ring");
    let site = only_bridge(&b.scene).clone();
    assert!(bridge::flipped(&b.plan, &b.scene, &site).is_empty());
}


