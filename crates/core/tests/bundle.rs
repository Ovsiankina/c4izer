//! Tests of the bus row (SRS 4.8: P4-400..P4-422, P4-427 core part):
//! bundles, written `bundle` in the DSL and shown as "Bus" in the editor.
//! One test per requirement at least, named after it. The editor tests
//! (P4-423..P4-427) are in `crates/editor/src/bundle_tests.rs`.

// Tests name the model `m`, the build `b`, a line `l`, as the router does.
#![allow(clippy::many_single_char_names, clippy::similar_names)]

use std::sync::atomic::AtomicBool;

use c4rizer_core::edit;
use c4rizer_core::export;
use c4rizer_core::geom::{Contact, Dir, Point, Rect, Segment, crossings};
use c4rizer_core::lint;
use c4rizer_core::model::{self, Model, NoticeKind};
use c4rizer_core::pipeline::{self, ViewBuild};
use c4rizer_core::place::Keep;
use c4rizer_core::route::{Line, RoutedBundle};
use c4rizer_core::view::EdgeId;

const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example/workspace.dsl"));

/// Eight containers, roles only: three on top (`a`, `b`, `c`), three
/// below (`x`, `y`, `z`), two at the sides (`left`, `right`).
/// Relationships, in order: 0 a -> x, 1 b -> y, 2 c -> z, 3 a -> z,
/// 4 c -> x, 5 left -> right, 6 b -> x, 7 a -> y, 8 left -> x,
/// 9 b -> z, 10 a -> b, 11 b -> c, 12 a -> right, 13 b -> right,
/// 14 c -> right.
const GRID: &str = r#"        s = softwareSystem "S" "Does things." {
            a = container "A" "Top one." "Rust"
            b = container "B" "Top two." "Rust"
            c = container "C" "Top three." "Rust"
            x = container "X" "Bottom one." "Rust"
            y = container "Y" "Bottom two." "Rust"
            z = container "Z" "Bottom three." "Rust"
            left = container "Left" "West side." "Rust"
            right = container "Right" "East side." "Rust"
        }
        a -> x "Feeds X"
        b -> y "Feeds Y"
        c -> z "Feeds Z"
        a -> z "Feeds Z too"
        c -> x "Feeds X too"
        left -> right "Talks across"
        b -> x "Feeds X again"
        a -> y "Feeds Y too"
        left -> x "Feeds X from the side"
        b -> z "Feeds Z again"
        a -> b "Calls B"
        b -> c "Calls C"
        a -> right "Tells Right"
        b -> right "Tells Right too"
        c -> right "Tells Right again""#;

/// A container view of `GRID` with `lines` in its block: tops at y 100,
/// bottoms at y 1100, the sides at y 600, far apart.
fn grid(lines: &str) -> String {
    grid_at(1100, lines)
}

/// `grid` with the bottom row at `bottom` px and the sides half way.
fn grid_at(bottom: i32, lines: &str) -> String {
    let side = i32::midpoint(100, bottom);
    format!(
        "workspace {{\n    model {{\n{GRID}\n    }}\n    views {{\n        container s \"g\" \"Grid\" {{\n            include *\n            at a 400 100\n            at b 800 100\n            at c 1200 100\n            at x 400 {bottom}\n            at y 800 {bottom}\n            at z 1200 {bottom}\n            at left -200 {side}\n            at right 1800 {side}\n{lines}\n        }}\n    }}\n}}\n"
    )
}

/// The model of `src`, panicking with the error text.
fn parse(src: &str) -> Model {
    match model::parse(src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    }
}

/// The build of view `key` of `m`.
fn build(m: &Model, key: &str) -> ViewBuild {
    match pipeline::build_view(m, key, &Keep::default(), &AtomicBool::new(false)) {
        Ok(b) => b,
        Err(e) => panic!("{e:?}"),
    }
}

/// The model and build of the grid view with `lines`.
fn grid_build(lines: &str) -> (Model, ViewBuild) {
    let m = parse(&grid(lines));
    let b = build(&m, "g");
    (m, b)
}

/// The example workspace with `lines` added after the line holding `head`.
fn example_with(head: &str, lines: &str) -> String {
    let at = EXAMPLE.find(head).expect("view head") + head.len();
    let nl = EXAMPLE[at..].find('\n').expect("newline") + at + 1;
    format!("{}{}\n{}", &EXAMPLE[..nl], lines, &EXAMPLE[nl..])
}

/// The `BundleIgnored` notices of `m`, as displayed.
fn ignored(m: &Model) -> Vec<String> {
    m.notices.iter().filter(|n| n.kind == NoticeKind::BundleIgnored).map(ToString::to_string).collect()
}

/// The routed line of `e`.
fn line(b: &ViewBuild, e: EdgeId) -> &Line {
    b.routes.line(e).expect("routed")
}

/// The edge drawn from the box named `from` to the box named `to`.
fn edge(m: &Model, b: &ViewBuild, from: &str, to: &str) -> EdgeId {
    let name = |i: u32| m.element(b.plan.boxes[i as usize].element).name.value.as_str();
    b.plan.edges.iter().find(|e| name(e.source.0) == from && name(e.destination.0) == to).expect("edge").id
}

/// The only bundle of `b`, routed on a shared stretch.
fn routed(b: &ViewBuild) -> &RoutedBundle {
    let r = &b.routes.bundles[0];
    assert!(!r.fallback, "bundle fell back: {r:?}");
    r
}

/// The direction of `a -> b`.
fn dir(a: Point, b: Point) -> Dir {
    Segment::new(a, b).dir().expect("octilinear")
}

/// `p` in px.
fn px(p: Point) -> Point {
    Point::new(p.x * 10, p.y * 10)
}

/// True when the segment `s` lies on the polyline `points`.
fn on_line(s: Segment, points: &[Point]) -> bool {
    let at = |p: Point| Segment::new(p, p);
    points.windows(2).any(|w| {
        let t = Segment::new(w[0], w[1]);
        t.dir() == s.dir() && t.contact(&at(s.a)) != Contact::Disjoint && t.contact(&at(s.b)) != Contact::Disjoint
    })
}

/// The lint messages of `b` with rule code `code`.
fn findings(m: &Model, b: &ViewBuild, code: &str) -> Vec<String> {
    lint::lint(m, &b.scene).findings.iter().filter(|f| f.rule.code() == code).map(|f| f.message.clone()).collect()
}

/// The signed across offset from `p` to `q` for a segment heading `d`:
/// axis units on an axis segment, `x ± y` units on a diagonal one.
fn across(d: Dir, p: Point, q: Point) -> i32 {
    let (dx, dy) = d.delta();
    (dy * q.x - dx * q.y) - (dy * p.x - dx * p.y)
}

/// The track segments of `r`.
fn track_segments(r: &RoutedBundle) -> Vec<Segment> {
    r.shared.iter().flat_map(|t| t.windows(2).map(|w| Segment::new(w[0], w[1])).collect::<Vec<_>>()).collect()
}

/// Three lines from the top row straight down to the bottom row.
const STRAIGHT: &str = "            bundle a -> x b -> y c -> z";
/// Three lines from different sources to different destinations, one
/// coming in from the side.
const MIXED: &str = "            bundle a -> y b -> z left -> x";
/// Three lines from the top row to the box on the right.
const RIGHT: &str = "            bundle a -> right b -> right c -> right";

#[test]
fn p4_400_bundle_line_parsed() {
    let m = parse(&grid(STRAIGHT));
    let v = m.view("g").expect("view");
    assert_eq!(v.bundles.len(), 1);
    assert_eq!(v.bundles[0].members.len(), 3);
    assert!(ignored(&m).is_empty(), "{:?}", ignored(&m));
    // A dynamic view names steps.
    let src = format!(
        "workspace {{\n    model {{\n{GRID}\n    }}\n    views {{\n        dynamic s \"d\" \"Flow\" {{\n            a -> x \"One\"\n            b -> y \"Two\"\n            bundle 1: a -> x 2: b -> y\n        }}\n    }}\n}}\n"
    );
    let m = parse(&src);
    let v = m.view("d").expect("view");
    assert_eq!(v.bundles[0].members.len(), 2);
    assert_eq!(v.bundles[0].members[1].order.as_ref().map(|o| o.value.as_str()), Some("2"));
}

#[test]
fn p4_401_bad_bundle_lines_are_notices() {
    // Malformed: the whole line goes.
    let m = parse(&grid("            bundle a -> x b"));
    assert_eq!(m.view("g").expect("view").bundles.len(), 0);
    assert_eq!(ignored(&m).len(), 1);
    let m = parse(&grid("            bundle 1: a -> x b -> y"));
    assert_eq!(ignored(&m).len(), 1, "a step number outside a dynamic view");
    // An unknown element, a line not drawn, a line twice, a line already
    // bundled: that member goes, with a notice each.
    let m = parse(&grid("            bundle a -> x b -> y nobody -> x x -> a a -> x\n            bundle a -> x c -> z b -> x"));
    let n = ignored(&m);
    assert_eq!(n.len(), 4, "{n:?}");
    assert!(n.iter().any(|s| s.contains("no element `nobody`")), "{n:?}");
    assert!(n.iter().any(|s| s.contains("draws no such line")), "{n:?}");
    assert_eq!(n.iter().filter(|s| s.contains("already bundled")).count(), 2, "{n:?}");
    let v = m.view("g").expect("view");
    assert_eq!(v.bundles.len(), 2);
    assert_eq!(v.bundles[1].members.len(), 2);
    // A bundle left with one member is ignored, and the file still parses.
    let m = parse(&grid("            bundle a -> x x -> a"));
    assert_eq!(m.view("g").expect("view").bundles.len(), 0);
    assert!(ignored(&m).iter().any(|s| s.contains("two members or more")));
}

#[test]
fn p4_402_plan_carries_bundles() {
    let (_, b) = grid_build(STRAIGHT);
    assert_eq!(b.plan.bundles, vec![vec![EdgeId(0), EdgeId(1), EdgeId(2)]]);
    let (_, b) = grid_build("");
    assert!(b.plan.bundles.is_empty());
    assert!(b.routes.bundles.is_empty());
    assert!(b.scene.bundles.is_empty());
}

#[test]
fn p4_403_no_bundle_same_scene_and_same_boxes() {
    // Without a bundle line: no bundles anywhere (`golden.rs` pins the
    // bytes).
    let m = parse(EXAMPLE);
    for key in ["C1", "C2", "C3-serving", "C3-startup"] {
        assert!(build(&m, key).routes.bundles.is_empty());
    }
    // With one: the boxes stay where they were (an autoLayout view).
    let src = example_with("\"C2\" \"Container diagram for Atomanja\" {", "            bundle admin -> app worker -> app witness -> app");
    let b = build(&parse(&src), "C2");
    assert_eq!(b.routes.centres, build(&m, "C2").routes.centres);
}

#[test]
fn p4_404_bundles_routed_before_other_lines() {
    // The members' lines do not depend on the other lines of the view.
    let (mw, without) = grid_build(&format!("{STRAIGHT}\n            exclude \"left -> right\""));
    let (ma, all) = grid_build(STRAIGHT);
    for (s, d) in [("A", "X"), ("B", "Y"), ("C", "Z")] {
        let p = &line(&without, edge(&mw, &without, s, d)).points;
        let q = &line(&all, edge(&ma, &all, s, d)).points;
        assert_eq!(p, q, "{s} -> {d}");
    }
}

#[test]
fn p4_405_tracks_are_parallel_stretches_of_their_lines() {
    for lines in [STRAIGHT, MIXED, RIGHT] {
        let (_, b) = grid_build(lines);
        let r = routed(&b);
        assert_eq!(r.tracks.len(), 3);
        let n = r.shared[0].len();
        for (k, t) in r.shared.iter().enumerate() {
            assert_eq!(t.len(), n, "same number of segments");
            let l = line(&b, r.tracks[k]);
            for w in t.windows(2) {
                assert!(on_line(Segment::new(w[0], w[1]), &l.points), "{lines}: track {k} on its line");
            }
            for i in 0..n - 1 {
                let (d, d0) = (dir(t[i], t[i + 1]), dir(r.shared[0][i], r.shared[0][i + 1]));
                assert!(d == d0 || d == d0.opposite(), "{lines}: parallel");
            }
        }
    }
}

#[test]
fn p4_406_pitch_20px_on_axes_3_units_on_diagonals() {
    for lines in [STRAIGHT, MIXED, RIGHT] {
        let (_, b) = grid_build(lines);
        let r = routed(&b);
        for k in 0..r.shared.len() - 1 {
            let (t, u) = (&r.shared[k], &r.shared[k + 1]);
            for i in 0..t.len() - 1 {
                let d = dir(t[i], t[i + 1]);
                let j = if dir(u[i], u[i + 1]) == d { i } else { u.len() - 2 - i };
                let off = across(d, t[i], if j == i { u[i] } else { u[j + 1] }).abs();
                assert_eq!(off, if d.is_diagonal() { 3 } else { 2 }, "{lines}: tracks {k}, {} segment {i}", k + 1);
            }
        }
    }
}

#[test]
fn p4_407_corners_concentric() {
    // Wherever a spine bends (the bent spine itself is pinned by the
    // router's unit test `p4_407_tracks_of_a_bent_spine`).
    for lines in [STRAIGHT, MIXED, RIGHT] {
        let (_, b) = grid_build(lines);
        let r = routed(&b);
        let d0 = dir(r.shared[0][0], r.shared[0][1]);
        let tracks: Vec<Vec<Point>> = r
            .shared
            .iter()
            .map(|t| if dir(t[0], t[1]) == d0 { t.clone() } else { t.iter().rev().copied().collect() })
            .collect();
        for k in 0..tracks.len() - 1 {
            let (t, u) = (&tracks[k], &tracks[k + 1]);
            for i in 1..t.len() - 1 {
                let (d1, d2) = (dir(t[i - 1], t[i]), dir(t[i], t[i + 1]));
                assert_eq!(d1.turn_steps(d2), 1, "{lines}: 45° bend");
                // A counter-clockwise turn has the left track (k + 1) inside.
                let left_turn = Dir::ALL[(d1 as usize + 1) % 8] == d2;
                let (inner, outer) = if left_turn { (u, t) } else { (t, u) };
                let (dx, dy) = d1.delta();
                let along = |p: Point| dx * p.x + dy * p.y;
                assert!(along(inner[i]) < along(outer[i]), "{lines}: inner bends first at bend {i}");
            }
        }
    }
}

#[test]
fn p4_408_no_shared_stretch_falls_back_with_l31() {
    // Neighbours in one row: no stretch between the groups.
    let (m, b) = grid_build("            bundle a -> b b -> c");
    assert!(b.routes.bundles[0].fallback, "{:?}", b.routes.bundles[0]);
    assert!(b.routes.unroutable().is_empty());
    let f = findings(&m, &b, "L31");
    assert_eq!(f.len(), 1, "{f:?}");
    assert!(f[0].contains("has no shared stretch; routed as ordinary lines"), "{f:?}");
}

#[test]
fn p4_409_tracks_keep_clear_of_boxes() {
    for lines in [STRAIGHT, MIXED, RIGHT] {
        let (_, b) = grid_build(lines);
        let r = routed(&b);
        for s in track_segments(r) {
            for bx in &b.plan.boxes {
                let rect = bx.rect_at(b.routes.centres[bx.id.0 as usize]).inflate(1);
                assert!(!s.passes_through(&rect), "{lines}: track within 10px of a box");
            }
        }
    }
}

#[test]
fn p4_410_member_lines_meet_core_6() {
    for lines in [STRAIGHT, MIXED, RIGHT] {
        let (_, b) = grid_build(lines);
        let r = routed(&b);
        for &e in &r.tracks {
            let l = line(&b, e);
            for w in l.points.windows(3) {
                assert!(dir(w[0], w[1]).turn_steps(dir(w[1], w[2])) <= 1, "{lines}: bend over 45° in {:?}", l.points);
            }
            for w in l.points.windows(2) {
                assert!(Segment::new(w[0], w[1]).len_steps().expect("octilinear") >= 2, "{lines}: short segment {:?}", l.points);
            }
        }
    }
}

#[test]
fn p4_411_track_order_follows_the_ends() {
    let (_, b) = grid_build(STRAIGHT);
    let r = routed(&b);
    // Seen along the spine (downward) track 0 is the rightmost, the
    // westmost: x ascending, so a -> x, b -> y, c -> z.
    assert_eq!(r.tracks, vec![EdgeId(0), EdgeId(1), EdgeId(2)]);
    let xs: Vec<i32> = r.shared.iter().map(|t| t[0].x).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
}

/// The crossings between the members of `r`, counted as L4 counts.
fn member_crossings(b: &ViewBuild, r: &RoutedBundle) -> u32 {
    let mut x = 0;
    for (i, &e) in r.tracks.iter().enumerate() {
        for &f in &r.tracks[i + 1..] {
            x += crossings(&line(b, e).points, &line(b, f).points);
        }
    }
    x
}

#[test]
fn p4_412_members_never_cross_when_an_order_exists() {
    for lines in [STRAIGHT, MIXED, RIGHT] {
        let (m, b) = grid_build(lines);
        let r = routed(&b);
        assert_eq!(member_crossings(&b, r), 0, "{lines}");
        assert_eq!(r.crossings, 0, "{lines}");
        assert!(findings(&m, &b, "L30").is_empty());
    }
    // Permuted ends: whatever crossings remain are counted and reported
    // (the fan-outs may also get round the boxes instead).
    let (m, b) = grid_build("            bundle a -> z b -> y c -> x");
    let r = routed(&b);
    let x = member_crossings(&b, r);
    assert_eq!(r.crossings, x);
    let f = findings(&m, &b, "L30");
    if x == 0 {
        assert!(f.is_empty());
    } else {
        assert_eq!(f.len(), 1);
        assert!(f[0].contains(&format!("crosses itself {x} times: its ends are in opposite orders")), "{f:?}");
    }
}

#[test]
fn p4_413_others_cross_square_across_the_whole_bundle() {
    // A tall view: going round the bundle costs more than crossing it.
    let m = parse(&grid_at(6100, STRAIGHT));
    let b = build(&m, "g");
    let r = routed(&b);
    let l = line(&b, edge(&m, &b, "Left", "Right"));
    let tracks = track_segments(r);
    let mut crossed = 0;
    for w in l.points.windows(2) {
        let s = Segment::new(w[0], w[1]);
        let hit: Vec<&Segment> = tracks.iter().filter(|t| t.contact(&s) != Contact::Disjoint).collect();
        if hit.is_empty() {
            continue;
        }
        crossed += 1;
        assert_eq!(hit.len(), r.shared.len(), "one segment across every track");
        let d = s.dir().expect("octilinear");
        for t in hit {
            assert_eq!(d.turn_steps(t.dir().expect("octilinear")), 2, "square to the tracks");
        }
        for p in [s.a, s.b] {
            assert!(tracks.iter().all(|t| t.contact(&Segment::new(p, p)) == Contact::Disjoint), "no bend on a track");
        }
    }
    assert_eq!(crossed, 1, "{:?}", l.points);
}

#[test]
fn p4_414_member_never_joins_with_bus_on() {
    let (_, b) = grid_build("            bus on\n            bundle a -> x b -> x");
    let r = &b.routes.bundles[0];
    for &e in &r.members {
        assert_eq!(line(&b, e).joins(), None);
    }
    for l in b.routes.lines.iter().flatten() {
        if let Some(t) = l.joins() {
            assert!(!r.members.contains(&t), "joined onto a member");
        }
    }
}

#[test]
fn p4_415_anchor_kept_at_its_end() {
    let (m, b) = grid_build(&format!("{STRAIGHT}\n            anchor a -> x from bottom 25"));
    let r = routed(&b);
    let e = edge(&m, &b, "A", "X");
    assert!(r.tracks.contains(&e));
    let bx = &b.plan.boxes[b.plan.edges[e.0 as usize].source.0 as usize];
    let rect = bx.rect_at(b.routes.centres[bx.id.0 as usize]);
    let want = Point::new(rect.min.x + ((rect.max.x - rect.min.x) * 25 + 50) / 100, rect.max.y);
    assert_eq!(line(&b, e).points[0], want, "leaves exactly its anchor");
}

#[test]
fn p4_416_vertices_kept_on_fan_outs() {
    // A vertex near `a` goes on the source fan-out.
    let (m, b) = grid_build(&format!("{STRAIGHT}\n            via a -> x 300 260"));
    let r = routed(&b);
    let e = edge(&m, &b, "A", "X");
    assert!(r.left.is_empty());
    assert!(r.tracks.contains(&e));
    assert!(line(&b, e).points.contains(&Point::new(30, 26)), "{:?}", line(&b, e).points);
}

#[test]
fn p4_417_vertices_out_of_order_leave_the_bundle() {
    // A vertex near `x` first, then one near `a`: the split breaks order.
    let (m, b) = grid_build(&format!("{STRAIGHT}\n            via a -> x 300 1000 300 200"));
    let r = routed(&b);
    let e = edge(&m, &b, "A", "X");
    assert_eq!(r.left, vec![e]);
    assert!(!r.tracks.contains(&e));
    let p = &line(&b, e).points;
    assert!(p.contains(&Point::new(30, 100)) && p.contains(&Point::new(30, 20)), "{p:?}");
    let f = findings(&m, &b, "L31");
    assert!(f.iter().any(|s| s.contains("keeps its vertices and leaves its bundle")), "{f:?}");
}

/// Every member label of `b` crosses no other member's track (px).
fn labels_off_peer_tracks(b: &ViewBuild) {
    for r in &b.routes.bundles {
        for lab in b.labels.labels.iter().flatten() {
            if !r.tracks.contains(&lab.edge) {
                continue;
            }
            let rect = lab.callout.as_ref().map_or_else(
                || {
                    let c = px(lab.centre);
                    let min = Point::new(c.x - lab.width_px / 2, c.y - lab.height_px / 2);
                    Rect::from_corners(min, Point::new(min.x + lab.width_px, min.y + lab.height_px))
                },
                |c| c.text,
            );
            for (k, t) in r.shared.iter().enumerate() {
                if r.tracks[k] == lab.edge {
                    continue;
                }
                for w in t.windows(2) {
                    let s = Segment::new(px(w[0]), px(w[1]));
                    assert!(!s.passes_through(&rect), "label of {:?} on the track of {:?}", lab.edge, r.tracks[k]);
                }
            }
        }
    }
}

#[test]
fn p4_418_labels_off_neighbouring_tracks() {
    let (_, b) = grid_build(STRAIGHT);
    labels_off_peer_tracks(&b);
    let src = example_with("\"C2\" \"Container diagram for Atomanja\" {", "            bundle admin -> app worker -> app witness -> app\n            bundle app -> devdb app -> db app -> vikunja");
    labels_off_peer_tracks(&build(&parse(&src), "C2"));
}

#[test]
fn p4_419_callouts_cross_no_other_member() {
    let (_, b) = grid_build(&format!("{STRAIGHT}\n            labels callout"));
    labels_off_peer_tracks(&b);
    let r = routed(&b);
    for lab in b.labels.labels.iter().flatten() {
        let Some(c) = &lab.callout else { continue };
        if !r.tracks.contains(&lab.edge) {
            continue;
        }
        let leader = Segment::new(c.leader[0], c.leader[1]);
        for &e in r.tracks.iter().filter(|&&e| e != lab.edge) {
            for w in line(&b, e).points.windows(2) {
                let s = Segment::new(px(w[0]), px(w[1]));
                assert_eq!(s.contact(&leader), Contact::Disjoint, "leader of {:?} across {e:?}", lab.edge);
            }
        }
    }
}

#[test]
fn p4_420_scene_lists_bundles_and_svg_draws_the_scene() {
    let (_, b) = grid_build(STRAIGHT);
    assert_eq!(b.scene.bundles, b.routes.bundles);
    // The SVG is drawn from the same scene as the canvas (EDIT-12): each
    // scene line is one path through its points, or one per run where a
    // bridge cuts it (P4-519).
    let svg = export::svg(&b.scene);
    for l in &b.scene.lines {
        let runs = if l.runs.is_empty() { vec![l.points.clone()] } else { l.runs.clone() };
        for run in runs {
            let rest: Vec<String> = run[1..].iter().map(|p| format!("L{},{}", p.x, p.y)).collect();
            let d = format!("M{},{} {}", run[0].x, run[0].y, rest.join(" "));
            assert!(svg.contains(&format!("d=\"{d}\"")), "{d}");
        }
    }
}

#[test]
fn p4_421_json_export_has_no_bundles() {
    let src = grid(STRAIGHT);
    let m = parse(&src);
    let all = pipeline::build_all(&m, &AtomicBool::new(false)).expect("never stopped");
    let json = export::json(&m, &all, Some(&src));
    assert!(!json.contains("bundle"), "Structurizr has no field for bundles");
}

#[test]
fn p4_422_real_cases() {
    let cases = [
        ("C2", "\"C2\" \"Container diagram for Atomanja\" {", "            bundle admin -> app worker -> app witness -> app\n            bundle app -> devdb app -> db app -> vikunja"),
        (
            "C3-serving",
            "\"C3-serving\" \"Component diagram for the Atomanja server: serving and polling\" {",
            "            bundle vikunjaClient -> vikunja store -> devdb store -> db\n            bundle witness -> web worker -> web admin -> web",
        ),
    ];
    for (key, head, lines) in cases {
        let m = parse(&example_with(head, lines));
        assert!(ignored(&m).is_empty(), "{:?}", ignored(&m));
        let b = build(&m, key);
        assert!(b.routes.unroutable().is_empty(), "{key}: unroutable");
        assert_eq!(b.routes.bundles.len(), 2);
        for r in &b.routes.bundles {
            assert!(!r.fallback, "{key}: {r:?}");
            assert_eq!(r.tracks.len(), 3);
            assert_eq!(member_crossings(&b, r), 0, "{key}: {r:?}");
            for k in 0..r.shared.len() - 1 {
                let (t, u) = (&r.shared[k], &r.shared[k + 1]);
                for i in 0..t.len() - 1 {
                    let d = dir(t[i], t[i + 1]);
                    assert_eq!(dir(u[i], u[i + 1]), d, "{key}: parallel");
                    assert_eq!(across(d, t[i], u[i]).abs(), if d.is_diagonal() { 3 } else { 2 }, "{key}: pitch");
                }
            }
        }
        labels_off_peer_tracks(&b);
        assert!(findings(&m, &b, "L30").is_empty() && findings(&m, &b, "L31").is_empty());
    }
}

/// `SetBundles` of the grid view to `bundles` (pairs of keys).
fn set_bundles(bundles: &[&[(&str, &str)]]) -> edit::Edit {
    edit::Edit::SetBundles {
        view: "g".to_owned(),
        bundles: bundles.iter().map(|b| b.iter().map(|&(s, d)| edit::LineRef::new(s, d, None)).collect()).collect(),
    }
}

/// `edits` applied to `src`.
fn applied(src: &str, edits: &[edit::Edit]) -> edit::Applied {
    let parsed = model::parse(src).unwrap_or_else(|e| panic!("{e}"));
    edit::apply(&parsed, edits).unwrap_or_else(|e| panic!("{e:?}"))
}

#[test]
fn p4_427_set_bundles_writes_lines_and_undoes() {
    let src = grid("            anchor a -> x from bottom");
    let done = applied(&src, &[set_bundles(&[&[("a", "x"), ("b", "y")]])]);
    assert!(done.text.contains("anchor a -> x from bottom\n            bundle a -> x b -> y\n"), "{}", done.text);
    assert_eq!(parse(&done.text).view("g").expect("view").bundles.len(), 1);
    assert_eq!(applied(&done.text, &done.inverse).text, src, "the inverse removes it, byte for byte");
    // Rewritten in place, a second one after it, then all removed.
    let two = applied(&done.text, &[set_bundles(&[&[("a", "x"), ("b", "y"), ("c", "z")], &[("a", "z"), ("c", "x")]])]);
    assert!(two.text.contains("            bundle a -> x b -> y c -> z\n            bundle a -> z c -> x\n"), "{}", two.text);
    assert_eq!(applied(&two.text, &two.inverse).text, done.text);
    let none = applied(&two.text, &[set_bundles(&[])]);
    assert!(!none.text.contains("bundle"), "{}", none.text);
    assert_eq!(applied(&none.text, &none.inverse).text, two.text);
}
