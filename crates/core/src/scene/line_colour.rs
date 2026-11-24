//! The end-box colour of each line in a `lineColours box` view
//! (P5-300..P5-305): one pass per scene build, O(lines log lines),
//! independent of box positions (P5-306).

use super::contrast;
use crate::model::Colour;
use crate::view::{BoxId, ViewPlan};

/// Under this WCAG contrast against the page an end-box fill is not used
/// and the line keeps the plain line colour (P5-304; the white-on-white
/// guard `polish_01` pins).
const MIN_LINE_CONTRAST: f64 = 1.3;

/// Per plan edge, the colour its line takes in a `lineColours box` view
/// when no relationship style colours it: the fill of the box it points
/// to (P5-300), for a two-way pair the fill of the more connected box
/// (P5-305); `None` (the plain line colour) when that fill is too close
/// to the `page` (P5-304). `fill` gives a box's fill.
// DECISION: a degree table plus a sorted list of ordered pairs (binary
// search for the reverse) and the contrast test once per end box, not
// per line, memoised on the last fill: on 1000 boxes / 3000 lines the
// median went from 853 us (`HashSet`, a test per line) to about 100 us
// (P5-312). O(edges log edges); nothing reads positions, so a drag
// never changes a colour (P5-306).
pub(super) fn end_box_colours(plan: &ViewPlan, fill: impl Fn(BoxId) -> Colour, page: Colour) -> Vec<Option<Colour>> {
    let pair = |from: BoxId, to: BoxId| (u64::from(from.0) << 32) | u64::from(to.0);
    // (lines touching the box, lines ending at it), per box.
    let mut degree = vec![(0_u32, 0_u32); plan.boxes.len()];
    let mut pairs: Vec<u64> = Vec::with_capacity(plan.edges.len());
    for e in &plan.edges {
        degree[e.source.0 as usize].0 += 1;
        let d = &mut degree[e.destination.0 as usize];
        d.0 += 1;
        d.1 += 1;
        pairs.push(pair(e.source, e.destination));
    }
    pairs.sort_unstable();
    // Per box: not yet asked, or its fill when usable on `page` (P5-304).
    let mut usable: Vec<Option<Option<Colour>>> = vec![None; plan.boxes.len()];
    let mut last = (page, false);
    // P5-305 tie-break: more lines, then more incoming, then the box
    // whose element is declared first, then the lower box id.
    let rank = |b: BoxId| {
        let (all, incoming) = degree[b.0 as usize];
        (
            std::cmp::Reverse(all),
            std::cmp::Reverse(incoming),
            plan.boxes[b.0 as usize].element,
            b,
        )
    };
    plan.edges
        .iter()
        .map(|e| {
            let (s, d) = (e.source, e.destination);
            let two_way = s != d && pairs.binary_search(&pair(d, s)).is_ok();
            let end = if two_way && rank(s) < rank(d) { s } else { d };
            *usable[end.0 as usize].get_or_insert_with(|| {
                let colour = fill(end);
                // Fills come from a few styles: the last answer is reused.
                if last.0 != colour {
                    last = (colour, contrast(colour, page) >= MIN_LINE_CONTRAST);
                }
                last.1.then_some(colour)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;
    use crate::bench_util::{bench, debug_build};
    use crate::model::{LineColours, Mode, parse};
    use crate::view::plan;

    const WHITE: Colour = Colour { r: 0xff, g: 0xff, b: 0xff };

    /// A landscape of `boxes` systems and the `lines` given as `(from, to)`
    /// indexes, in a `lineColours box` view.
    fn landscape(boxes: usize, lines: &[(usize, usize)]) -> String {
        let mut src = String::from("workspace {\n  model {\n");
        for i in 0..boxes {
            writeln!(src, "    s{i} = softwareSystem \"S{i}\"").expect("write");
        }
        for (a, b) in lines {
            writeln!(src, "    s{a} -> s{b} \"Uses\"").expect("write");
        }
        src.push_str("  }\n  views {\n    systemLandscape \"all\" {\n      include *\n      lineColours box\n    }\n  }\n}\n");
        src
    }

    fn plan_of(src: &str) -> ViewPlan {
        let m = match parse(src) {
            Ok(p) => p.model,
            Err(e) => panic!("{e}"),
        };
        let p = plan(&m, m.view("all").expect("view"));
        assert_eq!(p.options.line_colours, LineColours::Box);
        p
    }

    /// Box `i`'s fill is grey `i + 1`, so a colour names its box.
    fn grey(b: BoxId) -> Colour {
        let v = u8::try_from(b.0 + 1).expect("small");
        Colour { r: v, g: v, b: v }
    }

    fn ends(p: &ViewPlan) -> Vec<Option<u32>> {
        end_box_colours(p, grey, WHITE)
            .into_iter()
            .map(|c| c.map(|c| u32::from(c.r) - 1))
            .collect()
    }

    /// The box id of system `s<i>` in `p`.
    fn id(p: &ViewPlan, name: &str) -> u32 {
        p.boxes.iter().find(|b| b.name == name).expect("box").id.0
    }

    #[test]
    fn p5_300_one_way_lines_take_their_end_box() {
        let p = plan_of(&landscape(3, &[(0, 1), (1, 2), (2, 0)]));
        let want: Vec<Option<u32>> = p.edges.iter().map(|e| Some(e.destination.0)).collect();
        assert_eq!(ends(&p), want);
    }

    #[test]
    fn p5_305_two_way_takes_the_more_connected_box() {
        // s1 has three lines (s0 <-> s1, s2 -> s1), s0 two.
        let p = plan_of(&landscape(3, &[(0, 1), (1, 0), (2, 1)]));
        let s1 = id(&p, "S1");
        for (e, end) in p.edges.iter().zip(ends(&p)) {
            let want = if e.destination.0 == s1 || e.source.0 == s1 && e.destination.0 == id(&p, "S0") {
                s1
            } else {
                e.destination.0
            };
            assert_eq!(end, Some(want), "{:?} -> {:?}", e.source, e.destination);
        }
        // s0 more connected now: both lines of the pair take s0.
        let p = plan_of(&landscape(3, &[(0, 1), (1, 0), (2, 0)]));
        let s0 = id(&p, "S0");
        let pair: Vec<Option<u32>> = p
            .edges
            .iter()
            .zip(ends(&p))
            .filter(|(e, _)| e.source.0 != id(&p, "S2"))
            .map(|(_, c)| c)
            .collect();
        assert_eq!(pair, vec![Some(s0), Some(s0)]);
    }

    #[test]
    fn p5_305_ties_go_to_more_incoming_then_first_declared() {
        // Equal lines (4 each); s1 has more incoming (3 against 1).
        let p = plan_of(&landscape(4, &[(0, 1), (1, 0), (2, 1), (3, 1), (0, 2), (0, 3)]));
        let (s0, s1) = (id(&p, "S0"), id(&p, "S1"));
        let pair: Vec<Option<u32>> = p
            .edges
            .iter()
            .zip(ends(&p))
            .filter(|(e, _)| [s0, s1].contains(&e.source.0) && [s0, s1].contains(&e.destination.0))
            .map(|(_, c)| c)
            .collect();
        assert_eq!(pair, vec![Some(s1), Some(s1)]);
        // A bare pair: same lines, same incoming: the first declared, s0.
        let p = plan_of(&landscape(2, &[(1, 0), (0, 1)]));
        let s0 = id(&p, "S0");
        assert_eq!(ends(&p), vec![Some(s0), Some(s0)]);
    }

    #[test]
    fn p5_304_a_fill_too_close_to_the_page_keeps_the_plain_colour() {
        let p = plan_of(&landscape(2, &[(0, 1)]));
        let near_white = |_| Colour { r: 0xf4, g: 0xf4, b: 0xf4 };
        assert_eq!(end_box_colours(&p, near_white, WHITE), vec![None]);
        let blue = Colour { r: 0x43, g: 0x8d, b: 0xd5 };
        assert_eq!(end_box_colours(&p, |_| blue, WHITE), vec![Some(blue)]);
        // On a dark page a dark fill is the one refused.
        let page = Colour { r: 0x1e, g: 0x21, b: 0x24 };
        assert_eq!(end_box_colours(&p, |_| page, page), vec![None]);
    }

    /// `boxes` systems, `lines` relationships from a fixed LCG, about one
    /// in ten answered by its reverse (two-way pairs).
    fn generated(boxes: usize, lines: usize) -> Vec<(usize, usize)> {
        let mut state: u64 = 0x05ee_dc01_00f1_14e5;
        let mut next = |n: usize| {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(state >> 33).expect("31 bits") % n
        };
        let mut out = Vec::with_capacity(lines);
        while out.len() < lines {
            let a = next(boxes);
            let b = (a + 1 + next(boxes - 1)) % boxes;
            out.push((a, b));
            if out.len() < lines && next(10) == 0 {
                out.push((b, a));
            }
        }
        out
    }

    fn bench_pass(test: &str, boxes: usize, lines: usize, budget_us: u64) {
        if debug_build(test) {
            return;
        }
        let p = plan_of(&landscape(boxes, &generated(boxes, lines)));
        assert_eq!((p.boxes.len(), p.edges.len()), (boxes, lines));
        let fills: Vec<Colour> = p.boxes.iter().map(|b| b.style.background).collect();
        let page = Colour { r: 0xff, g: 0xff, b: 0xff };
        let input = format!("generated {boxes} boxes / {lines} lines");
        bench(["P5-312", "line colour", "colour pass alone", &input], budget_us, || {
            std::hint::black_box(end_box_colours(std::hint::black_box(&p), |b| fills[b.0 as usize], page));
        });
    }

    #[test]
    #[ignore = "perf: release perf oracle only"]
    fn perf_p5_312_colour_pass_40_boxes() {
        bench_pass("perf_p5_312_colour_pass_40_boxes", 40, 60, BUDGET_40_US);
    }

    #[test]
    #[ignore = "perf: release perf oracle only"]
    fn perf_p5_312_colour_pass_1000_boxes() {
        bench_pass("perf_p5_312_colour_pass_1000_boxes", 1000, 3000, BUDGET_1000_US);
    }

    /// P5-312: a full `scene::build` of `C3-serving` of the example, the
    /// heaviest view, plain and then `lineColours box` (same plan, routes
    /// and labels; only the option differs).
    #[test]
    #[ignore = "perf: release perf oracle only"]
    fn perf_p5_312_scene_build_plain_vs_box() {
        const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl"));
        if debug_build("perf_p5_312_scene_build_plain_vs_box") {
            return;
        }
        let m = match parse(EXAMPLE) {
            Ok(p) => p.model,
            Err(e) => panic!("{e}"),
        };
        let stop = std::sync::atomic::AtomicBool::new(false);
        let b = crate::pipeline::build_view(&m, "C3-serving", &crate::place::Keep::default(), &stop).expect("builds");
        assert_eq!(b.plan.mode, Mode::Light);
        let mut boxed = b.plan.clone();
        boxed.options.line_colours = LineColours::Box;
        for (what, plan, budget_us) in [
            ("scene build, plain", &b.plan, BUDGET_SCENE_US),
            ("scene build, lineColours box", &boxed, BUDGET_SCENE_US),
        ] {
            bench(["P5-312", "line colour", what, "example.dsl C3-serving"], budget_us, || {
                std::hint::black_box(super::super::build(std::hint::black_box(plan), &b.routes, &b.labels));
            });
        }
    }

    // Budgets (P5-900): about 4x the highest of three medians measured on
    // a quiet machine 2026-11-23 (1.5 us, 73 us, plain 63 us / box 53 us),
    // rounded up, at least 50 us.
    const BUDGET_40_US: u64 = 50;
    const BUDGET_1000_US: u64 = 300;
    const BUDGET_SCENE_US: u64 = 260;
}
