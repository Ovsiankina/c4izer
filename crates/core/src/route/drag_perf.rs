//! Drag preview measurements (no budget, numbers printed only): where
//! the time of one `pipeline::rebuild_after_move` goes, and which drags
//! make it slow. Every test is `#[ignore]`d and prefixed `perf_drag_`;
//! run in release:
//! `cargo test -p c4rizer-core --release --lib -- --ignored perf_drag_ --nocapture --test-threads 1`.
//!
//! The router counters come from `route/probe.rs` (test builds only), so
//! these runs carry the hooks' small cost; `perf_core_31_drag_preview_measured`
//! in `tests/speed.rs` times the same baseline without them.
// DECISION: unit tests, not `tests/speed.rs`: the phase split needs the
// crate-private `label::place_labels_with`, `scene::build_with` and the
// router's probe.
// DECISION: the printing helpers cast counts and nanoseconds to f64 and
// grow long, and the scenarios name boxes `h`, `l`, `e` as the router
// names its locals; those pedantic lints are allowed for this file only.
#![allow(
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use super::probe::{self, Stats};
use super::{Effort, Score, reroute};
use crate::geom::Point;
use crate::label;
use crate::lint;
use crate::model::{Model, parse};
use crate::pipeline::{PREVIEW_ROUTE, ViewBuild, build_view, rebuild_after_move};
use crate::place::Keep;
use crate::scene;
use crate::view::{BoxId, ViewPlan};

const GENERATED: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/core_31_generated.dsl"));
const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl"));
const WORKSPACE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example/workspace.dsl"));
/// Read at run time, read-only; skipped when it does not parse.
const DOCS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/workspace.dsl");

const RUNS: usize = 7;
const STEP_RUNS: usize = 3;

fn debug_build(test: &str) -> bool {
    if cfg!(debug_assertions) {
        println!("{test}: skipped, timing needs `cargo test --release`");
        return true;
    }
    false
}

fn model(src: &str) -> Model {
    match parse(src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    }
}

fn go() -> AtomicBool {
    AtomicBool::new(false)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// One timed drag preview, phase by phase, as `rebuild_after_move` runs it.
#[derive(Debug, Clone, Default)]
struct Sample {
    total: Duration,
    centres: Duration,
    reroute: Duration,
    plan_clone: Duration,
    labels: Duration,
    scene: Duration,
    lint: Duration,
    stats: Stats,
    crossings: u32,
}

/// `rebuild_after_move`'s body, timed per stage (same calls, same order).
fn preview(m: &Model, prev: &ViewBuild, moved: &BTreeMap<BoxId, Point>) -> (Sample, ViewBuild) {
    let stop = go();
    let t = Instant::now();
    let mut centres = prev.routes.centres.clone();
    for (id, &at) in moved {
        if let Some(c) = centres.get_mut(id.0 as usize) {
            *c = at;
        }
    }
    let t_centres = t.elapsed();
    probe::reset();
    let t1 = Instant::now();
    let routes = reroute(&prev.plan, &centres, &prev.routes, &PREVIEW_ROUTE, &stop).expect("not stopped");
    let t_reroute = t1.elapsed();
    let stats = probe::take();
    let t2 = Instant::now();
    let plan = prev.plan.clone();
    let t_plan = t2.elapsed();
    let t3 = Instant::now();
    let words = label::Words::of(&m.terminology);
    let labels = label::place_labels_with(&plan, &routes, &words);
    let t_labels = t3.elapsed();
    let t4 = Instant::now();
    let sc = scene::build_with(&plan, &routes, &labels, &words);
    let t_scene = t4.elapsed();
    let t5 = Instant::now();
    let report = lint::lint(m, &sc);
    let t_lint = t5.elapsed();
    let total = t.elapsed();
    let crossings = routes.crossings;
    let build = ViewBuild {
        plan,
        routes,
        labels,
        scene: sc,
        report,
    };
    (
        Sample {
            total,
            centres: t_centres,
            reroute: t_reroute,
            plan_clone: t_plan,
            labels: t_labels,
            scene: t_scene,
            lint: t_lint,
            stats,
            crossings,
        },
        build,
    )
}

/// Median-total sample of `runs` previews after one warm-up, plus the
/// max total and the max stop gap / longest search seen over the runs.
struct Measured {
    med: Sample,
    max_total: Duration,
    max_gap: Duration,
    max_search: Duration,
}

fn measure(m: &Model, prev: &ViewBuild, moved: &BTreeMap<BoxId, Point>, runs: usize) -> Measured {
    let (_, check) = preview(m, prev, moved);
    // The timed copy of the body must equal the real entry point.
    let real = rebuild_after_move(m, prev, moved, &PREVIEW_ROUTE, &go()).expect("not stopped");
    assert_eq!(check.routes, real.routes, "timed copy differs from rebuild_after_move");
    assert_eq!(check.scene, real.scene, "timed copy differs from rebuild_after_move");
    let mut samples: Vec<Sample> = (0..runs).map(|_| preview(m, prev, moved).0).collect();
    let max_total = samples.iter().map(|s| s.total).max().unwrap_or_default();
    let max_gap = samples.iter().map(|s| s.stats.max_gap).max().unwrap_or_default();
    let max_search = samples.iter().map(|s| s.stats.max_search).max().unwrap_or_default();
    samples.sort_by_key(|s| s.total);
    Measured {
        med: samples[runs / 2].clone(),
        max_total,
        max_gap,
        max_search,
    }
}

fn header() {
    println!(
        "{:<44} {:>7} {:>7} | {:>7} {:>6} {:>6} {:>6} {:>5} | {:>6} {:>7} {:>6} {:>5} {:>5} | {:>3} {:>7} {:>8} {:>2} {:>2} {:<16} {:>4} {:>8} {:>6} {:>4} {:>3} {:>6} {:>6} {:>4}",
        "scenario",
        "med",
        "max",
        "reroute",
        "label",
        "scene",
        "lint",
        "clone",
        "board",
        "search",
        "costfr",
        "score",
        "scrch",
        "brk",
        "path",
        "cells",
        "or",
        "ps",
        "1st X/C/U",
        "srch",
        "pops",
        "maxpop",
        "gave",
        "cf",
        "maxsr",
        "maxgap",
        "Xout"
    );
}

fn first(s: &Stats) -> String {
    s.first_pass
        .iter()
        .map(|(x, c, u)| format!("{x}/{c}/{u}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn row(name: &str, r: &Measured) {
    let s = &r.med;
    let st = &s.stats;
    println!(
        "{:<44} {:>7.1} {:>7.1} | {:>7.1} {:>6.1} {:>6.1} {:>6.1} {:>5.2} | {:>6.1} {:>7.1} {:>6.1} {:>5.1} {:>5.2} | {:>3} {:>7} {:>8} {:>2} {:>2} {:<16} {:>4} {:>8} {:>6} {:>4} {:>3} {:>6.1} {:>6.1} {:>4}",
        name,
        ms(s.total),
        ms(r.max_total),
        ms(s.reroute),
        ms(s.labels),
        ms(s.scene),
        ms(s.lint),
        ms(s.centres + s.plan_clone),
        ms(st.board),
        ms(st.search),
        ms(st.cost_from_time),
        ms(st.score),
        ms(st.scratch),
        st.broken,
        st.path,
        st.grid_cells,
        st.orders,
        st.passes,
        first(st),
        st.searches,
        st.pops,
        st.max_pops,
        st.gave_up,
        st.cost_from,
        ms(r.max_search),
        ms(r.max_gap),
        s.crossings
    );
}

fn degree(plan: &ViewPlan, b: BoxId) -> usize {
    plan.edges.iter().filter(|e| e.source == b || e.destination == b).count()
}

/// The box with the most edges (lowest id on a tie).
fn hub(plan: &ViewPlan) -> BoxId {
    let mut best = plan.boxes[0].id;
    for b in &plan.boxes {
        if degree(plan, b.id) > degree(plan, best) {
            best = b.id;
        }
    }
    best
}

/// The box with the fewest edges, at least one (lowest id on a tie).
fn leaf(plan: &ViewPlan) -> Option<BoxId> {
    plan.boxes
        .iter()
        .map(|b| b.id)
        .filter(|&b| degree(plan, b) > 0)
        .min_by_key(|&b| (degree(plan, b), b.0))
}

fn at(prev: &ViewBuild, b: BoxId) -> Point {
    prev.routes.centres[b.0 as usize]
}

fn shifted(prev: &ViewBuild, boxes: &[BoxId], dx: i32, dy: i32) -> BTreeMap<BoxId, Point> {
    boxes
        .iter()
        .map(|&b| {
            let c = at(prev, b);
            (b, Point::new(c.x + dx, c.y + dy))
        })
        .collect()
}

/// The `k` boxes nearest `centre_box` (itself first): a multi-selection.
fn cluster(prev: &ViewBuild, centre_box: BoxId, k: usize) -> Vec<BoxId> {
    let c = at(prev, centre_box);
    let mut all: Vec<BoxId> = prev.plan.boxes.iter().map(|b| b.id).collect();
    all.sort_by_key(|&b| {
        let p = at(prev, b);
        (i64::from(p.x - c.x).pow(2) + i64::from(p.y - c.y).pow(2), b.0)
    });
    all.truncate(k);
    all
}

/// The `k` boxes of highest degree.
fn top_degree(plan: &ViewPlan, k: usize) -> Vec<BoxId> {
    let mut all: Vec<BoxId> = plan.boxes.iter().map(|b| b.id).collect();
    all.sort_by_key(|&b| (std::cmp::Reverse(degree(plan, b)), b.0));
    all.truncate(k);
    all
}

fn extent(prev: &ViewBuild) -> (Point, Point) {
    let mut lo = Point::new(i32::MAX, i32::MAX);
    let mut hi = Point::new(i32::MIN, i32::MIN);
    for b in &prev.plan.boxes {
        let r = b.rect_at(at(prev, b.id));
        lo = Point::new(lo.x.min(r.min.x), lo.y.min(r.min.y));
        hi = Point::new(hi.x.max(r.max.x), hi.y.max(r.max.y));
    }
    (lo, hi)
}

/// The scenarios a..e of the task on one view.
fn matrix(prev: &ViewBuild) -> Vec<(String, BTreeMap<BoxId, Point>)> {
    let plan = &prev.plan;
    let h = hub(plan);
    let hd = degree(plan, h);
    let mut out = Vec::new();
    if let Some(l) = leaf(plan) {
        out.push((format!("a leaf (deg {}) +2x", degree(plan, l)), shifted(prev, &[l], 2, 0)));
    }
    out.push((format!("b hub (deg {hd}) +2x"), shifted(prev, &[h], 2, 0)));
    out.push((format!("b hub (deg {hd}) +1y"), shifted(prev, &[h], 0, 1)));
    for (dx, dy, n) in [(40, 0, "+40x"), (-40, 0, "-40x"), (0, 40, "+40y"), (0, -40, "-40y")] {
        out.push((format!("c hub {n}"), shifted(prev, &[h], dx, dy)));
    }
    let (lo, hi) = extent(prev);
    let c = at(prev, h);
    out.push(("c hub past right edge +30".into(), BTreeMap::from([(h, Point::new(hi.x + 30, c.y))])));
    out.push(("c hub past top edge -30".into(), BTreeMap::from([(h, Point::new(c.x, lo.y - 30))])));
    for k in [2, 4, 8] {
        if k > plan.boxes.len() {
            continue;
        }
        let sel = cluster(prev, h, k);
        out.push((format!("d cluster {k} +2x"), shifted(prev, &sel, 2, 0)));
        out.push((format!("d cluster {k} +30x+20y"), shifted(prev, &sel, 30, 20)));
    }
    if plan.boxes.len() >= 4 {
        let sel = top_degree(plan, 4);
        out.push(("d top-degree 4 +2x".into(), shifted(prev, &sel, 2, 0)));
        out.push(("d top-degree 4 +30x+20y".into(), shifted(prev, &sel, 30, 20)));
    }
    // e: the leaf onto its nearest box, touching it, and between the two
    // ends of the longest edge it is not on.
    if let Some(l) = leaf(plan) {
        let nearest = cluster(prev, l, 2).get(1).copied();
        if let Some(n) = nearest {
            let nb = &plan.boxes[n.0 as usize];
            let lb = &plan.boxes[l.0 as usize];
            let nr = nb.rect_at(at(prev, n));
            let lr = lb.rect_at(at(prev, l));
            let (nw, lw) = (nr.max.x - nr.min.x, lr.max.x - lr.min.x);
            let nc = at(prev, n);
            out.push(("e leaf overlaps nearest box".into(), BTreeMap::from([(l, Point::new(nc.x + nw / 2, nc.y))])));
            out.push((
                "e leaf touches nearest box".into(),
                BTreeMap::from([(l, Point::new(nr.max.x + 1 + lw / 2 + 1, nc.y))]),
            ));
        }
        let longest = plan
            .edges
            .iter()
            .filter(|e| e.source != l && e.destination != l && e.source != e.destination)
            .max_by_key(|e| {
                let (a, b) = (at(prev, e.source), at(prev, e.destination));
                ((a.x - b.x).abs() + (a.y - b.y).abs(), std::cmp::Reverse(e.id.0))
            });
        if let Some(e) = longest {
            let (a, b) = (at(prev, e.source), at(prev, e.destination));
            out.push((
                "e leaf between ends of longest edge".into(),
                BTreeMap::from([(l, Point::new(i32::midpoint(a.x, b.x), i32::midpoint(a.y, b.y)))]),
            ));
        }
    }
    out
}

fn full(m: &Model, key: &str, keep: &Keep) -> ViewBuild {
    build_view(m, key, keep, &go()).expect("builds")
}

/// The full build after a release (DEFAULT config, EDIT-25): the moved
/// boxes in `Keep::moved` (they win over their `at` lines, as the
/// written-back lines do), every other box kept where it was.
fn release_keep(prev: &ViewBuild, moved: &BTreeMap<BoxId, Point>) -> Keep {
    let mut keep = Keep::from_previous(&prev.plan, &prev.routes.centres);
    keep.moved.clone_from(moved);
    keep
}

/// Median and max of `runs` full builds with `keep`, with the router
/// counters of the last one.
fn release_cost(m: &Model, key: &str, keep: &Keep, runs: usize) -> (Duration, Duration, Stats, ViewBuild) {
    let mut times = Vec::new();
    let mut last = None;
    let mut stats = Stats::default();
    for _ in 0..runs {
        probe::reset();
        let t = Instant::now();
        let b = full(m, key, keep);
        times.push(t.elapsed());
        stats = probe::take();
        last = Some(b);
    }
    times.sort();
    (times[runs / 2], times[runs - 1], stats, last.expect("ran"))
}

fn release_row(name: &str, (med, max, st, b): &(Duration, Duration, Stats, ViewBuild)) {
    println!(
        "release {name:<36} med {:>7.1} max {:>7.1} ms | route searches {} pops {} gave {} cf {} orders {} passes {} board {:.1} search {:.1} score {:.1} | maxsearch {:.1} maxgap {:.1} ({}) | crossings {}",
        ms(*med),
        ms(*max),
        st.searches,
        st.pops,
        st.gave_up,
        st.cost_from,
        st.orders,
        st.passes,
        ms(st.board),
        ms(st.search),
        ms(st.score),
        ms(st.max_search),
        ms(st.max_gap),
        st.gap_from.zip(st.gap_to).map(|(a, b)| format!("{}..{}", a.line(), b.line())).unwrap_or_default(),
        b.routes.crossings
    );
}

fn view_summary(name: &str, b: &ViewBuild) {
    let h = hub(&b.plan);
    println!(
        "== {name}: {} boxes, {} edges, {} bundles (fallback {:?}), crossings {}, unroutable {}, hub deg {}, maxCrossings {}",
        b.plan.boxes.len(),
        b.plan.edges.len(),
        b.plan.bundles.len(),
        b.routes.bundles.iter().map(|x| x.fallback).collect::<Vec<_>>(),
        b.routes.crossings,
        b.routes.lines.iter().filter(|l| l.is_none()).count(),
        degree(&b.plan, h),
        b.plan.options.max_crossings
    );
}

fn run_matrix(m: &Model, prev: &ViewBuild, runs: usize) {
    header();
    for (name, moved) in matrix(prev) {
        let r = measure(m, prev, &moved, runs);
        row(&name, &r);
    }
}

/// 1 + 2a..e on the generated 40-box / 60-edge view.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_generated_matrix() {
    if debug_build("perf_drag_generated_matrix") {
        return;
    }
    let m = model(GENERATED);
    let prev = full(&m, "generated", &Keep::default());
    view_summary("generated", &prev);
    run_matrix(&m, &prev, RUNS);
}

/// 1 + 2a..e on every drawable view of the example files and, when it
/// parses, `docs/workspace.dsl` (read-only).
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_real_views_matrix() {
    if debug_build("perf_drag_real_views_matrix") {
        return;
    }
    let docs = std::fs::read_to_string(DOCS).ok();
    let mut files: Vec<(&str, String)> = vec![("example.dsl", EXAMPLE.to_owned()), ("example/workspace.dsl", WORKSPACE.to_owned())];
    if let Some(d) = docs {
        files.push(("docs/workspace.dsl", d));
    }
    for (file, src) in files {
        let m = match parse(&src) {
            Ok(p) => p.model,
            Err(e) => {
                println!("{file}: does not parse, skipped: {e}");
                continue;
            }
        };
        for v in &m.views {
            let Ok(prev) = build_view(&m, &v.key, &Keep::default(), &go()) else {
                continue;
            };
            if prev.plan.edges.is_empty() {
                continue;
            }
            view_summary(&format!("{file} {}", v.key), &prev);
            run_matrix(&m, &prev, RUNS);
        }
    }
}

fn path_offset(i: i32) -> (i32, i32) {
    (2 * i.min(20), 2 * (i - 20).max(0))
}

/// A drag of `boxes` along 40 snapped positions (20 x 2 cells right,
/// then 20 x 2 cells down), every step against the pre-drag build.
fn series(m: &Model, prev: &ViewBuild, boxes: &[BoxId], name: &str) {
    println!("-- series {name} ({} boxes): step dx dy | med max ms | reroute | brk or ps 1st-X/C/U crossed srch pops maxpop gave cf | maxsearch maxgap | Xout", boxes.len());
    let mut totals = Vec::new();
    for i in 1..=40 {
        let (dx, dy) = path_offset(i);
        let moved = shifted(prev, boxes, dx, dy);
        let r = measure(m, prev, &moved, STEP_RUNS);
        let s = &r.med;
        let st = &s.stats;
        totals.push(ms(s.total));
        println!(
            "{i:>3} {dx:>3} {dy:>3} | {:>7.1} {:>7.1} | {:>7.1} | {:>3} {} {:>2} {:<12} {:<8} {:>4} {:>8} {:>6} {:>3} {:>3} | {:>6.1} {:>6.1} | {}",
            ms(s.total),
            ms(r.max_total),
            ms(s.reroute),
            st.broken,
            st.orders,
            st.passes,
            first(st),
            st.first_crossed.iter().map(ToString::to_string).collect::<Vec<_>>().join(","),
            st.searches,
            st.pops,
            st.max_pops,
            st.gave_up,
            st.cost_from,
            ms(r.max_search),
            ms(r.max_gap),
            s.crossings
        );
    }
    totals.sort_by(f64::total_cmp);
    println!(
        "-- series {name}: min {:.1} median {:.1} p90 {:.1} max {:.1} ms",
        totals[0],
        totals[totals.len() / 2],
        totals[totals.len() * 9 / 10],
        totals[totals.len() - 1]
    );
}

/// 2f on the generated view: the hub, and a 4-box cluster round it.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_generated_series() {
    if debug_build("perf_drag_generated_series") {
        return;
    }
    let m = model(GENERATED);
    let prev = full(&m, "generated", &Keep::default());
    view_summary("generated", &prev);
    let h = hub(&prev.plan);
    series(&m, &prev, &[h], "hub");
    series(&m, &prev, &cluster(&prev, h, 4), "cluster 4");
    series(&m, &prev, &cluster(&prev, h, 8), "cluster 8");
}

/// 2f on `docs/workspace.dsl`'s largest view, when it parses.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_docs_series() {
    if debug_build("perf_drag_docs_series") {
        return;
    }
    let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) else {
        println!("docs/workspace.dsl does not parse, skipped");
        return;
    };
    let mut best: Option<ViewBuild> = None;
    for v in &m.views {
        if let Ok(b) = build_view(&m, &v.key, &Keep::default(), &go())
            && best.as_ref().is_none_or(|x| b.plan.edges.len() > x.plan.edges.len())
        {
            best = Some(b);
        }
    }
    let Some(prev) = best else { return };
    view_summary(&format!("docs/workspace.dsl {}", prev.plan.key), &prev);
    let h = hub(&prev.plan);
    series(&m, &prev, &[h], "hub");
    series(&m, &prev, &cluster(&prev, h, 4), "cluster 4");
}

/// `SplitMix64`, as `tests/speed.rs`.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as usize) as i32
    }
}

/// 2g + 3: ten random multi-box moves on the generated view, each
/// released with a full build, then the matrix and a series again on the
/// resulting layout; every release timed.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_generated_played_around() {
    if debug_build("perf_drag_generated_played_around") {
        return;
    }
    let m = model(GENERATED);
    let key = "generated";
    let mut prev = full(&m, key, &Keep::default());
    view_summary("generated before", &prev);
    let mut rng = Rng(0x00C4_D4A6);
    let mut moved_all: BTreeMap<BoxId, Point> = BTreeMap::new();
    let n = prev.plan.boxes.len();
    for step in 0..10 {
        let k = 1 + rng.below(4);
        let mut sel: Vec<BoxId> = Vec::new();
        while sel.len() < k {
            let b = BoxId(rng.below(n) as u32);
            if !sel.contains(&b) {
                sel.push(b);
            }
        }
        let (dx, dy) = (rng.range(-15, 15), rng.range(-15, 15));
        let moved = shifted(&prev, &sel, dx, dy);
        // The preview at the drop position, then the release build.
        let r = measure(&m, &prev, &moved, STEP_RUNS);
        moved_all.extend(moved.iter().map(|(&b, &p)| (b, p)));
        let keep = release_keep(&prev, &moved_all);
        let rel = release_cost(&m, key, &keep, 3);
        println!(
            "move {step}: {k} boxes by ({dx},{dy}): preview med {:.1} ms (reroute {:.1}, brk {}, passes {}, gave {}) -> release med {:.1} max {:.1} ms, crossings {}",
            ms(r.med.total),
            ms(r.med.reroute),
            r.med.stats.broken,
            r.med.stats.passes,
            r.med.stats.gave_up,
            ms(rel.0),
            ms(rel.1),
            rel.3.routes.crossings
        );
        prev = rel.3;
    }
    view_summary("generated after 10 moves", &prev);
    run_matrix(&m, &prev, RUNS);
    let h = hub(&prev.plan);
    series(&m, &prev, &[h], "played hub");
    series(&m, &prev, &cluster(&prev, h, 4), "played cluster 4");
    // 3: the release after (b), (d) on this layout.
    for (name, sel) in [("played b hub +2x", vec![h]), ("played d cluster 4 +30x+20y", cluster(&prev, h, 4))] {
        let d = if sel.len() == 1 { (2, 0) } else { (30, 20) };
        // The earlier moves stay in `moved`: the file's `at` lines still
        // hold the pristine grid here, as the model was never edited.
        let mut moved = moved_all.clone();
        moved.extend(shifted(&prev, &sel, d.0, d.1));
        let keep = release_keep(&prev, &moved);
        release_row(name, &release_cost(&m, key, &keep, 3));
    }
}

/// 3: the full DEFAULT build after a release of (b) and (d) on the
/// pristine generated view and on the real views' heaviest.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_release_cost() {
    if debug_build("perf_drag_release_cost") {
        return;
    }
    let mut cases: Vec<(String, Model, String)> = vec![("generated".into(), model(GENERATED), "generated".into())];
    cases.push(("example C3-serving".into(), model(EXAMPLE), "C3-serving".into()));
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) {
        let keys: Vec<String> = m.views.iter().map(|v| v.key.clone()).collect();
        for k in keys {
            cases.push((format!("docs {k}"), m.clone(), k));
        }
    }
    for (name, m, key) in cases {
        let Ok(prev) = build_view(&m, &key, &Keep::default(), &go()) else {
            continue;
        };
        if prev.plan.edges.is_empty() {
            continue;
        }
        view_summary(&name, &prev);
        let h = hub(&prev.plan);
        release_row(&format!("{name} nothing moved"), &release_cost(&m, &key, &release_keep(&prev, &BTreeMap::new()), 3));
        release_row(&format!("{name} b hub +2x"), &release_cost(&m, &key, &release_keep(&prev, &shifted(&prev, &[h], 2, 0)), 3));
        let sel = cluster(&prev, h, 4.min(prev.plan.boxes.len()));
        release_row(
            &format!("{name} d cluster 4 +30x+20y"),
            &release_cost(&m, &key, &release_keep(&prev, &shifted(&prev, &sel, 30, 20)), 3),
        );
    }
}

/// The task's representative scenarios only (a quick before/after check):
/// on the generated view the hub +2x, hub +40x and 8-box cluster
/// +30x+20y; on `docs/workspace.dsl` C3-entry (when it parses) the hub
/// +2x, the 8-box cluster +2x and the hub +40y.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_representative() {
    if debug_build("perf_drag_representative") {
        return;
    }
    let pick = |m: &Model, prev: &ViewBuild, names: &[&str]| {
        header();
        for (name, moved) in matrix(prev) {
            if names.iter().any(|n| name.starts_with(n)) {
                row(&name, &measure(m, prev, &moved, RUNS));
            }
        }
    };
    let m = model(GENERATED);
    let prev = full(&m, "generated", &Keep::default());
    view_summary("generated", &prev);
    pick(&m, &prev, &["b hub (deg 7) +2x", "c hub +40x", "d cluster 8 +30x+20y"]);
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model)
        && let Ok(prev) = build_view(&m, "C3-entry", &Keep::default(), &go())
    {
        view_summary("docs/workspace.dsl C3-entry", &prev);
        pick(&m, &prev, &["b hub (deg 16) +2x", "d cluster 8 +2x", "c hub +40y"]);
    }
}

/// Where the flooding searches come from: per scenario of the matrix of
/// the generated view, of `example.dsl` C3-serving and of
/// `docs/workspace.dsl` C3-entry, the searches that found nothing and
/// the ones over 100k pops (one run each, counts only).
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_floods() {
    if debug_build("perf_drag_floods") {
        return;
    }
    let mut cases: Vec<(String, Model, String)> = vec![("generated".into(), model(GENERATED), "generated".into())];
    cases.push(("example C3-serving".into(), model(EXAMPLE), "C3-serving".into()));
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) {
        cases.push(("docs C3-entry".into(), m, "C3-entry".into()));
    }
    for (name, m, key) in cases {
        let Ok(prev) = build_view(&m, &key, &Keep::default(), &go()) else {
            continue;
        };
        view_summary(&name, &prev);
        for (sc, moved) in matrix(&prev) {
            let (s, _) = preview(&m, &prev, &moved);
            let st = &s.stats;
            println!(
                "flood {sc:<36} {:>7.1} ms | srch {:>4} pops {:>9} | none {:>3} pops {:>9} retried {:>3} | big {:>3} pops {:>9} none {:>3} | gave {:>4} cf {:>4} cells {:>9} | back {}/{} max {} states {} | carried {} rejoin {} nocand {} noleg {} legpops {} | ps {} 1st {}",
                ms(s.reroute),
                st.searches,
                st.pops,
                st.none,
                st.none_pops,
                st.retried,
                st.big,
                st.big_pops,
                st.big_none,
                st.gave_up,
                st.cost_from,
                st.cf_cells,
                st.back_proved,
                st.back,
                st.back_max_proof,
                st.back_states,
                st.translated,
                st.rejoined,
                st.rejoin_nocand,
                st.rejoin_noleg,
                st.rejoin_pops,
                st.passes,
                first(st)
            );
        }
    }
}

/// Stop latency of the real preview: `rebuild_after_move` runs on a
/// thread, the flag is set after a delay, and the time from the store to
/// the return is taken; the worst over a sweep of delays (0.5 ms to the
/// preview's own length) per representative scenario. A preview that
/// finished before the store counts as no sample.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_stop_latency() {
    use std::sync::atomic::Ordering;
    if debug_build("perf_drag_stop_latency") {
        return;
    }
    let sweep = |m: &Model, prev: &ViewBuild, names: &[&str]| {
        for (name, moved) in matrix(prev) {
            if !names.iter().any(|n| name.starts_with(n)) {
                continue;
            }
            let t = Instant::now();
            let _ = rebuild_after_move(m, prev, &moved, &PREVIEW_ROUTE, &go());
            let whole = t.elapsed();
            let mut worst = Duration::ZERO;
            let mut samples = 0;
            let mut delay = Duration::from_micros(500);
            while delay < whole {
                let flag = AtomicBool::new(false);
                let (stopped, late) = std::thread::scope(|s| {
                    let h = s.spawn(|| {
                        let r = rebuild_after_move(m, prev, &moved, &PREVIEW_ROUTE, &flag);
                        (r.is_err(), Instant::now())
                    });
                    std::thread::sleep(delay);
                    let set = Instant::now();
                    flag.store(true, Ordering::Relaxed);
                    let (stopped, done) = h.join().expect("joins");
                    (stopped, done.saturating_duration_since(set))
                });
                if stopped {
                    samples += 1;
                    worst = worst.max(late);
                }
                delay = delay.mul_f64(1.3);
            }
            println!("stop {name:<36} preview {:>7.1} ms | {samples:>2} stopped samples, worst stop latency {:>6.2} ms", ms(whole), ms(worst));
        }
    };
    let m = model(GENERATED);
    let prev = full(&m, "generated", &Keep::default());
    sweep(&m, &prev, &["b hub (deg 7) +2x", "c hub +40x", "d cluster 8 +30x+20y"]);
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model)
        && let Ok(prev) = build_view(&m, "C3-entry", &Keep::default(), &go())
    {
        sweep(&m, &prev, &["b hub (deg 16) +2x", "d cluster 8 +2x", "c hub +40y"]);
    }
}

/// The element key of a generated box ("Service 07" is `s07`).
fn generated_key(plan: &ViewPlan, b: BoxId) -> String {
    let name = &plan.boxes[b.0 as usize].name;
    format!("s{}", name.trim_start_matches("Service "))
}

/// 2h: the generated view with a bundle of two hub lines, and with a
/// hand vertex on one hub line; (b) on each.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_generated_bundle_and_vertex() {
    if debug_build("perf_drag_generated_bundle_and_vertex") {
        return;
    }
    let base = model(GENERATED);
    let prev = full(&base, "generated", &Keep::default());
    let h = hub(&prev.plan);
    let hub_edges: Vec<_> = prev.plan.edges.iter().filter(|e| e.source == h || e.destination == h).collect();
    let out: Vec<_> = hub_edges.iter().filter(|e| e.source == h).collect();
    let anchor = "            at s39";
    let mut variants = Vec::new();
    if out.len() >= 2 {
        let line = format!(
            "            bundle {} -> {} {} -> {}\n",
            generated_key(&prev.plan, out[0].source),
            generated_key(&prev.plan, out[0].destination),
            generated_key(&prev.plan, out[1].source),
            generated_key(&prev.plan, out[1].destination)
        );
        variants.push(("bundle of 2 hub lines", line));
    }
    // A bundle that does not fall back: the pair of lines whose sources
    // and destinations lie closest together (four distinct boxes), the
    // first of the five closest pairs whose full build routes it.
    let dist = |a: BoxId, b: BoxId| {
        let (p, q) = (at(&prev, a), at(&prev, b));
        (p.x - q.x).abs() + (p.y - q.y).abs()
    };
    let mut pairs = Vec::new();
    for (i, e) in prev.plan.edges.iter().enumerate() {
        for f in &prev.plan.edges[i + 1..] {
            let ends = [e.source, e.destination, f.source, f.destination];
            if (0..4).all(|k| (k + 1..4).all(|j| ends[k] != ends[j])) {
                pairs.push((dist(e.source, f.source) + dist(e.destination, f.destination), e.id, f.id));
            }
        }
    }
    pairs.sort_by_key(|p| (p.0, p.1.0, p.2.0));
    for &(_, e, f) in pairs.iter().take(5) {
        let (e, f) = (&prev.plan.edges[e.0 as usize], &prev.plan.edges[f.0 as usize]);
        let line = format!(
            "            bundle {} -> {} {} -> {}\n",
            generated_key(&prev.plan, e.source),
            generated_key(&prev.plan, e.destination),
            generated_key(&prev.plan, f.source),
            generated_key(&prev.plan, f.destination)
        );
        let m = model(&GENERATED.replacen(anchor, &format!("{line}{anchor}"), 1));
        if full(&m, "generated", &Keep::default()).routes.bundles.iter().all(|b| !b.fallback) {
            variants.push(("bundle of 2 close lines", line));
            break;
        }
    }
    // A vertex 170px right of the midpoint of a hub line (centres in px):
    // half the 340px column pitch, so it sits in the gap between columns
    // and rows when both ends share a column, as s04 -> s12 does.
    let e = hub_edges[0];
    let (a, b) = (at(&prev, e.source), at(&prev, e.destination));
    let line = format!(
        "            via {} -> {} {} {}\n",
        generated_key(&prev.plan, e.source),
        generated_key(&prev.plan, e.destination),
        (a.x + b.x) * 5 + 170,
        (a.y + b.y) * 5
    );
    variants.push(("vertex on a hub line", line));
    for (name, line) in variants {
        let src = GENERATED.replacen(anchor, &format!("{line}{anchor}"), 1);
        let m = model(&src);
        let p = full(&m, "generated", &Keep::default());
        view_summary(&format!("generated + {name} ({})", line.trim()), &p);
        header();
        let hh = hub(&p.plan);
        row(&format!("h {name}: b hub +2x"), &measure(&m, &p, &shifted(&p, &[hh], 2, 0), RUNS));
        row(&format!("h {name}: a leaf +2x"), &measure(&m, &p, &shifted(&p, &[leaf(&p.plan).expect("leaf")], 2, 0), RUNS));
        let rel = release_cost(&m, "generated", &release_keep(&p, &shifted(&p, &[hh], 2, 0)), 3);
        view_summary(&format!("release of h {name}"), &rel.3);
        release_row(&format!("h {name} b hub +2x"), &rel);
    }
}

/// The scene stage split: `scene::build_with` as the preview runs it, and
/// again with `Routes::crossings` set to 0, which makes `bridge::add`
/// return at once; the difference is the bridge (crossing) work.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_scene_split() {
    if debug_build("perf_drag_scene_split") {
        return;
    }
    let mut cases: Vec<(String, Model, String)> = vec![("generated".into(), model(GENERATED), "generated".into())];
    cases.push(("example C3-serving".into(), model(EXAMPLE), "C3-serving".into()));
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) {
        cases.push(("docs C3-entry".into(), m.clone(), "C3-entry".into()));
        cases.push(("docs C3-stages".into(), m, "C3-stages".into()));
    }
    for (name, m, key) in cases {
        let Ok(b) = build_view(&m, &key, &Keep::default(), &go()) else {
            continue;
        };
        let words = label::Words::of(&m.terminology);
        let time = |routes: &super::Routes| {
            let mut t: Vec<Duration> = (0..RUNS)
                .map(|_| {
                    let s = Instant::now();
                    let _ = scene::build_with(&b.plan, routes, &b.labels, &words);
                    s.elapsed()
                })
                .collect();
            t.sort();
            t[RUNS / 2]
        };
        let with = time(&b.routes);
        let mut plain = b.routes.clone();
        plain.crossings = 0;
        let without = time(&plain);
        println!(
            "scene {name}: {:.1} ms, without bridges {:.1} ms (crossings {}, style {:?}, bridges drawn {})",
            ms(with),
            ms(without),
            b.routes.crossings,
            b.plan.options.crossings,
            b.scene.bridges.len()
        );
    }
}

/// A scenario of `perf_drag_rules`: name, model, pre-drag build, view key,
/// moved boxes.
type RuleCase = (String, Model, ViewBuild, String, BTreeMap<BoxId, Point>);

/// Step 5 of the preview work: candidate caps on a worse-than-before
/// preview's rip-up, each measured for time and for the preview's
/// crossings / clashes / unroutable against the current preview (`R0`)
/// and the release build at the same centres. Scenarios: the
/// representative six and every 4th step of the generated 8-box and
/// C3-entry 4-box drag series.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_rules() {
    if debug_build("perf_drag_rules") {
        return;
    }
    let all = |orders, rounds, unroutable_only, pops| Effort { orders, rounds, unroutable_only, pops };
    let rules: Vec<(&str, Option<Effort>)> = vec![
        ("R0 current", None),
        ("R1 1 pass 1 order", Some(all(1, 0, false, u64::MAX))),
        ("R2 1 pass/order 2 ord", Some(all(2, 0, false, u64::MAX))),
        ("R3 unroutable 1 order", Some(all(1, u32::MAX, true, u64::MAX))),
        ("R4 1 round 1 order", Some(all(1, 1, false, u64::MAX))),
        ("R5 1 round 2 orders", Some(all(2, 1, false, u64::MAX))),
        ("R6 pops 300k 2 orders", Some(all(2, u32::MAX, false, 300_000))),
        ("R7 unroutable 2 orders", Some(all(2, u32::MAX, true, u64::MAX))),
        ("R8 pops 300k 1 order", Some(all(1, u32::MAX, false, 300_000))),
        ("R9 unroutable 1 round", Some(all(1, 1, true, u64::MAX))),
        ("R10 unroutable pops 300k", Some(all(1, u32::MAX, true, 300_000))),
    ];
    // `RULES=R0,R9` runs only those (by the name's first word).
    let only: Option<Vec<String>> = std::env::var("RULES").ok().map(|v| v.split(',').map(str::to_owned).collect());
    let rules: Vec<(&str, Option<Effort>)> =
        rules.into_iter().filter(|(n, _)| only.as_ref().is_none_or(|o| o.iter().any(|w| n.split(' ').next() == Some(w.as_str())))).collect();
    let mut cases: Vec<RuleCase> = Vec::new();
    let gm = model(GENERATED);
    let gprev = full(&gm, "generated", &Keep::default());
    for (name, moved) in matrix(&gprev) {
        if ["b hub (deg 7) +2x", "c hub +40x", "d cluster 8 +30x+20y"].contains(&name.as_str()) {
            cases.push((format!("gen {name}"), gm.clone(), gprev.clone(), "generated".into(), moved));
        }
    }
    let h = hub(&gprev.plan);
    let sel = cluster(&gprev, h, 8);
    for i in (4..=40).step_by(4) {
        let (dx, dy) = path_offset(i);
        cases.push((format!("gen series8 {i}"), gm.clone(), gprev.clone(), "generated".into(), shifted(&gprev, &sel, dx, dy)));
    }
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model)
        && let Ok(prev) = build_view(&m, "C3-entry", &Keep::default(), &go())
    {
        for (name, moved) in matrix(&prev) {
            if ["b hub (deg 16) +2x", "d cluster 8 +2x", "c hub +40y"].contains(&name.as_str()) {
                cases.push((format!("C3 {name}"), m.clone(), prev.clone(), "C3-entry".into(), moved));
            }
        }
        let h = hub(&prev.plan);
        let sel = cluster(&prev, h, 4);
        for i in (4..=40).step_by(4) {
            let (dx, dy) = path_offset(i);
            cases.push((format!("C3 series4 {i}"), m.clone(), prev.clone(), "C3-entry".into(), shifted(&prev, &sel, dx, dy)));
        }
    }
    let score = |b: &ViewBuild| {
        let s = Score::of(&b.routes.lines);
        (s.crossings, s.clashes, s.unroutable)
    };
    let releases: Vec<(u32, u32, usize)> = cases.iter().map(|(_, m, prev, key, moved)| score(&full(m, key, &release_keep(prev, moved)))).collect();
    for (rule, effort) in &rules {
        probe::set_effort(*effort);
        let mut series: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        let mut sums: BTreeMap<&str, [i64; 6]> = BTreeMap::new();
        for (k, (name, m, prev, _, moved)) in cases.iter().enumerate() {
            let mut t: Vec<(f64, ViewBuild)> = (0..3).map(|_| {
                let (s, b) = preview(m, prev, moved);
                (ms(s.total), b)
            }).collect();
            t.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (x, c, u) = score(&t[1].1);
            let (rx, rc, ru) = releases[k];
            let group = if name.contains("series8") { "gen series8" } else if name.contains("series4") { "C3 series4" } else { "single" };
            series.entry(group).or_default().push(t[1].0);
            let e = sums.entry(group).or_insert([0; 6]);
            e[0] += i64::from(x);
            e[1] += i64::from(c);
            e[2] += u as i64;
            e[3] += i64::from(rx);
            e[4] += i64::from(rc);
            e[5] += ru as i64;
            if group == "single" {
                println!("rule {rule:<24} {name:<28} {:>7.1} ms | X/C/U {x}/{c}/{u} | release {rx}/{rc}/{ru}", t[1].0);
            }
        }
        for (g, mut v) in series {
            v.sort_by(f64::total_cmp);
            let n = v.len();
            let e = sums[g];
            println!(
                "rule {rule:<24} {g:<12} n {n:>2} median {:>7.1} p90 {:>7.1} max {:>7.1} ms | sum X/C/U {}/{}/{} | release {}/{}/{}",
                v[n / 2], v[(n * 9 / 10).min(n - 1)], v[n - 1], e[0], e[1], e[2], e[3], e[4], e[5]
            );
        }
    }
    probe::set_effort(None);
}

/// Step 7 of the preview work: the full build after release of every
/// matrix scenario of the generated view, `example.dsl` C3-serving and
/// `docs/workspace.dsl` C3-entry, with the full build's no-path proofs
/// on and off: the `Routes` must be equal; the times are printed.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_full_proofs() {
    if debug_build("perf_drag_full_proofs") {
        return;
    }
    let mut cases: Vec<(String, Model, String)> = vec![("generated".into(), model(GENERATED), "generated".into())];
    cases.push(("example C3-serving".into(), model(EXAMPLE), "C3-serving".into()));
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) {
        cases.push(("docs C3-entry".into(), m, "C3-entry".into()));
    }
    let (mut on_all, mut off_all, mut n) = (Duration::ZERO, Duration::ZERO, 0);
    for (name, m, key) in cases {
        let Ok(prev) = build_view(&m, &key, &Keep::default(), &go()) else {
            continue;
        };
        let mut scen = matrix(&prev);
        scen.insert(0, ("nothing moved".into(), BTreeMap::new()));
        for (sc, moved) in scen {
            let keep = release_keep(&prev, &moved);
            let t = Instant::now();
            let on = full(&m, &key, &keep);
            let t_on = t.elapsed();
            probe::set_no_full_proofs(true);
            let t = Instant::now();
            let off = full(&m, &key, &keep);
            let t_off = t.elapsed();
            probe::set_no_full_proofs(false);
            assert_eq!(on.routes, off.routes, "{name} {sc}: the proofs changed the routes");
            on_all += t_on;
            off_all += t_off;
            n += 1;
            println!("full {name:<20} {sc:<36} proofs {:>8.1} ms, without {:>8.1} ms", ms(t_on), ms(t_off));
        }
    }
    println!("full: {n} builds equal; total with proofs {:.0} ms, without {:.0} ms", ms(on_all), ms(off_all));
}

/// Step 7: stop latency of a full build (`build_view`, as the editor's
/// parked build runs it) on the views of `tests/data/example/workspace.dsl`
/// (no `at` lines: auto-placement, then routing, labels, scene, lint) and
/// on the generated view: the flag set after a sweep of delays, the worst
/// time from the store to the return.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_full_build_stop_latency() {
    use std::sync::atomic::Ordering;
    if debug_build("perf_drag_full_build_stop_latency") {
        return;
    }
    let mut cases: Vec<(Model, String)> = Vec::new();
    let w = model(WORKSPACE);
    for v in &w.views {
        cases.push((w.clone(), v.key.clone()));
    }
    cases.push((model(GENERATED), "generated".into()));
    for (m, key) in cases {
        let t = Instant::now();
        let _ = build_view(&m, &key, &Keep::default(), &go());
        let whole = t.elapsed();
        let mut worst = Duration::ZERO;
        let mut samples = 0;
        let mut delay = Duration::from_micros(500);
        while delay < whole {
            let flag = AtomicBool::new(false);
            let (stopped, late) = std::thread::scope(|s| {
                let h = s.spawn(|| {
                    let r = build_view(&m, &key, &Keep::default(), &flag);
                    (r.is_err(), Instant::now())
                });
                std::thread::sleep(delay);
                let set = Instant::now();
                flag.store(true, Ordering::Relaxed);
                let (stopped, done) = h.join().expect("joins");
                (stopped, done.saturating_duration_since(set))
            });
            if stopped {
                samples += 1;
                worst = worst.max(late);
            }
            delay = delay.mul_f64(1.3);
        }
        println!("full stop {key:<14} build {:>7.1} ms | {samples:>2} stopped samples, worst stop latency {:>6.2} ms", ms(whole), ms(worst));
    }
}

/// Pictures for a look at the preview (no assertion): with `SNAP_DIR`
/// set, writes PNGs of the pre-drag build, the preview and the release
/// build of a few drags there.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_snapshots() {
    let Ok(dir) = std::env::var("SNAP_DIR") else {
        println!("perf_drag_snapshots: set SNAP_DIR");
        return;
    };
    let mut cases: Vec<(String, Model, String, Vec<&str>)> = vec![("gen".into(), model(GENERATED), "generated".into(), vec!["b hub (deg 7) +2x", "d cluster 8 +30x+20y"])];
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) {
        cases.push(("c3".into(), m, "C3-entry".into(), vec!["b hub (deg 16) +2x", "d cluster 8 +2x"]));
    }
    for (tag, m, key, names) in cases {
        let prev = full(&m, &key, &Keep::default());
        for (k, (name, moved)) in matrix(&prev).into_iter().filter(|(n, _)| names.contains(&n.as_str())).enumerate() {
            let pre = rebuild_after_move(&m, &prev, &moved, &PREVIEW_ROUTE, &go()).expect("runs");
            let rel = full(&m, &key, &release_keep(&prev, &moved));
            for (what, b) in [("0before", &prev), ("1preview", &pre), ("2release", &rel)] {
                std::fs::write(format!("{dir}/{tag}{k}_{what}.svg"), crate::export::svg(&b.scene)).expect("write");
            }
            println!("{tag}{k}: {name}");
        }
    }
}

/// Step 11: the full build after release of every matrix scenario of the
/// generated view, `example.dsl` C3-serving and `docs/workspace.dsl`
/// C3-entry (and each with nothing moved), with the edge orders on
/// threads and on one: the `Routes` must be equal; the times are printed.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_parallel_orders() {
    if debug_build("perf_drag_parallel_orders") {
        return;
    }
    let mut cases: Vec<(String, Model, String)> = vec![("generated".into(), model(GENERATED), "generated".into())];
    cases.push(("example C3-serving".into(), model(EXAMPLE), "C3-serving".into()));
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) {
        cases.push(("docs C3-entry".into(), m, "C3-entry".into()));
    }
    let (mut par_all, mut seq_all, mut n) = (Duration::ZERO, Duration::ZERO, 0);
    for (name, m, key) in cases {
        let Ok(prev) = build_view(&m, &key, &Keep::default(), &go()) else {
            continue;
        };
        let mut scen = matrix(&prev);
        scen.insert(0, ("nothing moved".into(), BTreeMap::new()));
        for (sc, moved) in scen {
            let keep = release_keep(&prev, &moved);
            let t = Instant::now();
            let par = full(&m, &key, &keep);
            let t_par = t.elapsed();
            probe::set_sequential(true);
            let t = Instant::now();
            let seq = full(&m, &key, &keep);
            let t_seq = t.elapsed();
            probe::set_sequential(false);
            assert_eq!(par.routes, seq.routes, "{name} {sc}: threads changed the routes");
            assert_eq!(par.scene, seq.scene, "{name} {sc}: threads changed the scene");
            par_all += t_par;
            seq_all += t_seq;
            n += 1;
            println!("par {name:<20} {sc:<36} threads {:>8.1} ms, one thread {:>8.1} ms", ms(t_par), ms(t_seq));
        }
    }
    println!("par: {n} builds equal; total on threads {:.0} ms, on one {:.0} ms", ms(par_all), ms(seq_all));
}

// ------------------------------------------------------------ goal board
//
// The scoreboard of the drag performance round: fixed views, fixed
// pointer-like series, fixed seeds. Its inputs never change; only the
// code it measures does.

/// One pointer-like drag path: `n` consecutive one-cell (10px) steps,
/// heading one of the eight directions for a run of 3..=6 steps before
/// turning, from a fixed seed. Cumulative offsets, first step included.
fn pointer_path(seed: u64, n: usize) -> Vec<(i32, i32)> {
    const DIRS: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
    let mut rng = Rng(seed);
    let mut out = Vec::with_capacity(n);
    let (mut x, mut y) = (0, 0);
    let mut dir = rng.below(8);
    let mut left = 0;
    while out.len() < n {
        if left == 0 {
            // Turn by 1..=3 eighths either way: a hand on a mouse, not a
            // random walk back on itself.
            let by = 1 + rng.below(3);
            dir = if rng.below(2) == 0 { (dir + by) % 8 } else { (dir + 8 - by) % 8 };
            left = 3 + rng.below(4);
        }
        x += DIRS[dir].0;
        y += DIRS[dir].1;
        left -= 1;
        out.push((x, y));
    }
    out
}

/// One step of a goal series: total, route, labels+scene+lint, bridges
/// (ms), A* pops, and the preview's unroutable / clashes / crossings.
#[derive(Debug, Clone, Copy, Default)]
struct GoalStep {
    total: f64,
    route: f64,
    rest: f64,
    bridges: f64,
    pops: u64,
    unroutable: usize,
    clashes: u32,
    crossings: u32,
    flood_states: u64,
    flood_ms: f64,
    /// Lines bridged or detoured, repairs the guard refused, full searches.
    repaired: u32,
    refused: usize,
    full: u32,
}

fn pct(v: &[f64], p: usize) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[(s.len() * p / 100).min(s.len() - 1)]
}

/// One run of a series: every step against the pre-drag build `prev`, as
/// the editor's drag does.
#[inline(never)]
fn goal_run(m: &Model, prev: &ViewBuild, boxes: &[BoxId], path: &[(i32, i32)], check: bool) -> Vec<GoalStep> {
    let words = label::Words::of(&m.terminology);
    path.iter().map(|&(dx, dy)| goal_step(m, prev, &shifted(prev, boxes, dx, dy), check, &words).0).collect()
}

/// One timed goal step: the preview of `moved` from `prev`, and the build.
fn goal_step(m: &Model, prev: &ViewBuild, moved: &BTreeMap<BoxId, Point>, check: bool, words: &label::Words<'_>) -> (GoalStep, ViewBuild) {
    let moved = moved.clone();
            let (s, b) = preview(m, prev, &moved);
            if check {
                let real = rebuild_after_move(m, prev, &moved, &PREVIEW_ROUTE, &go()).expect("not stopped");
                assert_eq!(b.routes, real.routes, "timed copy differs from rebuild_after_move");
                assert_eq!(b.scene, real.scene, "timed copy differs from rebuild_after_move");
            }
            // Bridges: the scene again without crossings, where
            // `bridge::add` returns at once (outside the timed total).
            let mut plain = b.routes.clone();
            plain.crossings = 0;
            let t = Instant::now();
            let _ = scene::build_with(&b.plan, &plain, &b.labels, words);
            let no_bridges = ms(t.elapsed());
            let score = Score::of(&b.routes.lines);
            if std::env::var_os("GOAL_VERBOSE").is_some() {
                let st = &s.stats;
                println!(
                    "step {:?} | {:>6.1} route {:>6.1} | brk {:>2} stale {} carried {:>2} rejoin {:>2} (nocand {} noleg {} worse {} legpops {}) | ps {:>2} srch {:>3} pops {:>8} max {:>7} cf {:>3} cells {:>8} | board {:.1} search {:.1} cf {:.1} score {:.1} scratch {:.1} | U/C/X {}/{}/{}",
                    moved.values().next(),
                    ms(s.total),
                    ms(s.reroute),
                    st.broken,
                    st.dir_stale,
                    st.translated,
                    st.rejoined,
                    st.rejoin_nocand,
                    st.rejoin_noleg,
                    st.rejoin_worse,
                    st.rejoin_pops,
                    st.passes,
                    st.searches,
                    st.pops,
                    st.max_pops,
                    st.cost_from,
                    st.cf_cells,
                    ms(st.board),
                    ms(st.search),
                    ms(st.cost_from_time),
                    ms(st.score),
                    ms(st.scratch),
                    score.unroutable,
                    score.clashes,
                    score.crossings
                );
            }
            (
            GoalStep {
                total: ms(s.total),
                route: ms(s.reroute),
                rest: ms(s.labels + s.scene + s.lint + s.centres + s.plan_clone),
                bridges: (ms(s.scene) - no_bridges).max(0.0),
                pops: s.stats.pops + s.stats.rejoin_pops,
                unroutable: score.unroutable,
                clashes: score.clashes,
                crossings: score.crossings,
                flood_states: s.stats.dir_states,
                flood_ms: ms(s.stats.dir_time),
                repaired: s.stats.cats.iter().filter(|c| c.0.ends_with("bridged") || c.0.ends_with("detoured")).map(|c| c.1).sum(),
                refused: 0,
                full: s.stats.cats.iter().filter(|c| c.0.contains("search")).map(|c| c.1).sum(),
            },
            b,
        )
}

/// Three runs of a series (one warm-up first); the run whose p90 is the
/// middle one.
fn goal_series(m: &Model, prev: &ViewBuild, boxes: &[BoxId], path: &[(i32, i32)]) -> Vec<GoalStep> {
    let _ = goal_run(m, prev, boxes, path, true);
    let mut runs: Vec<Vec<GoalStep>> = (0..3).map(|_| goal_run(m, prev, boxes, path, false)).collect();
    runs.sort_by(|a, b| {
        let p = |r: &Vec<GoalStep>| pct(&r.iter().map(|s| s.total).collect::<Vec<_>>(), 90);
        p(a).total_cmp(&p(b))
    });
    runs.swap_remove(1)
}

fn goal_line(name: &str, steps: &[GoalStep]) {
    let tot: Vec<f64> = steps.iter().map(|s| s.total).collect();
    let n = steps.len() as f64;
    let route: f64 = steps.iter().map(|s| s.route).sum();
    let rest: f64 = steps.iter().map(|s| s.rest).sum();
    let bridges: f64 = steps.iter().map(|s| s.bridges).sum();
    let pops: u64 = steps.iter().map(|s| s.pops).sum();
    let u: usize = steps.iter().map(|s| s.unroutable).sum();
    let c: u32 = steps.iter().map(|s| s.clashes).sum();
    let x: u32 = steps.iter().map(|s| s.crossings).sum();
    let fs: u64 = steps.iter().map(|s| s.flood_states).sum();
    let fms: f64 = steps.iter().map(|s| s.flood_ms).sum();
    let rep: u32 = steps.iter().map(|s| s.repaired).sum();
    let refd: usize = steps.iter().map(|s| s.refused).sum();
    let fulls: u32 = steps.iter().map(|s| s.full).sum();
    println!(
        "goal {name:<34} n {:>3} | med {:>6.1} p90 {:>6.1} max {:>6.1} ms | mean route {:>6.1} rest {:>5.1} (bridges {:>4.1}) ms | pops {:>9} {:>6.0}/ms | flood {:>8.0}/step {:>5.1} ns/state | repaired/refused/full {rep}/{refd}/{fulls} | sum U/C/X {u}/{c}/{x}",
        steps.len(),
        pct(&tot, 50),
        pct(&tot, 90),
        pct(&tot, 100),
        route / n,
        rest / n,
        bridges / n,
        pops,
        pops as f64 / route.max(1e-9),
        fs as f64 / n,
        fms * 1e6 / (fs.max(1) as f64)
    );
}

/// The boxes of a goal view's single-box series: the hub, then the next
/// four of highest degree with at least 4 lines (lowest id on a tie).
fn goal_singles(plan: &ViewPlan) -> Vec<BoxId> {
    let mut all = top_degree(plan, plan.boxes.len());
    all.retain(|&b| degree(plan, b) >= 4);
    all.truncate(5);
    all
}

/// The goal scoreboard: on the generated 40-box view and on
/// `docs/workspace.dsl` C3-entry, (a) the 8 boxes nearest the hub dragged
/// together along a 48-step pointer path, (b) each of up to 5 boxes of 4+
/// lines (the hub first) alone along a 40-step path; every step against
/// the pre-drag build, 3 runs after a warm-up, the middle run printed.
/// Targets: (a) p90 <= 33 ms, (b) p90 < 10 ms.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_goal() {
    if debug_build("perf_drag_goal") {
        return;
    }
    let mut views: Vec<(&str, Model, &str)> = vec![("generated", model(GENERATED), "generated")];
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) {
        views.push(("C3-entry", m, "C3-entry"));
    }
    for (name, m, key) in &views {
        let prev = full(m, key, &Keep::default());
        println!("loadavg {}", std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim());
        view_summary(name, &prev);
        let h = hub(&prev.plan);
        let eight = cluster(&prev, h, 8);
        let a = goal_series(m, &prev, &eight, &pointer_path(0x8B0C, 48));
        goal_line(&format!("{name} 8 boxes"), &a);
        let mut pooled = Vec::new();
        for (k, b) in goal_singles(&prev.plan).into_iter().enumerate() {
            let s = goal_series(m, &prev, &[b], &pointer_path(0x51_0000 + k as u64, 40));
            goal_line(&format!("{name} single {} (deg {})", b.0, degree(&prev.plan, b)), &s);
            pooled.extend(s);
        }
        goal_line(&format!("{name} single pooled"), &pooled);
    }
}

/// The full builds the round must leave byte-identical: with `FULL_DUMP`
/// set to a file path, writes the `Routes` and SVG of the full build of
/// the generated view, `example.dsl` C3-serving and `docs/workspace.dsl`
/// C3-entry, untouched and after a release of every matrix scenario and
/// of the goal series' last positions; diff two dumps to compare.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_goal_full_dump() {
    let Ok(path) = std::env::var("FULL_DUMP") else {
        println!("perf_drag_goal_full_dump: set FULL_DUMP");
        return;
    };
    let mut cases: Vec<(String, Model, String)> = vec![("generated".into(), model(GENERATED), "generated".into())];
    cases.push(("example C3-serving".into(), model(EXAMPLE), "C3-serving".into()));
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) {
        cases.push(("docs C3-entry".into(), m, "C3-entry".into()));
    }
    let mut out = String::new();
    let mut n = 0;
    for (name, m, key) in cases {
        let prev = full(&m, &key, &Keep::default());
        let mut scen = matrix(&prev);
        scen.insert(0, ("nothing moved".into(), BTreeMap::new()));
        let h = hub(&prev.plan);
        let (dx, dy) = *pointer_path(0x8B0C, 48).last().expect("steps");
        scen.push(("goal 8 boxes end".into(), shifted(&prev, &cluster(&prev, h, 8), dx, dy)));
        for (k, b) in goal_singles(&prev.plan).into_iter().enumerate() {
            let (dx, dy) = *pointer_path(0x51_0000 + k as u64, 40).last().expect("steps");
            scen.push((format!("goal single {} end", b.0), shifted(&prev, &[b], dx, dy)));
        }
        for (sc, moved) in scen {
            let b = full(&m, &key, &release_keep(&prev, &moved));
            let _ = writeln!(out, "== {name} / {sc}\n{:?}\n{}", b.routes, crate::export::svg(&b.scene));
            n += 1;
        }
    }
    std::fs::write(&path, out).expect("write dump");
    println!("perf_drag_goal_full_dump: {n} full builds written to {path}");
}

/// One run of one goal series, for a profiler: `GOAL_PROFILE` is
/// `<view>:<8|single>` (`generated:8`, `C3-entry:single`, ...).
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_goal_profile() {
    let Ok(what) = std::env::var("GOAL_PROFILE") else {
        println!("perf_drag_goal_profile: set GOAL_PROFILE");
        return;
    };
    let (view, kind) = what.split_once(':').expect("view:kind");
    let m = if view == "generated" {
        model(GENERATED)
    } else {
        std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model).expect("docs parse")
    };
    let prev = full(&m, view, &Keep::default());
    let steps: usize = std::env::var("GOAL_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(48);
    let path = |seed: u64, n: usize| pointer_path(seed, n).into_iter().take(steps).collect::<Vec<_>>();
    let t = Instant::now();
    if kind == "8" {
        let eight = cluster(&prev, hub(&prev.plan), 8);
        goal_line(&what, &goal_run(&m, &prev, &eight, &path(0x8B0C, 48), false));
    } else {
        for (k, b) in goal_singles(&prev.plan).into_iter().enumerate() {
            goal_line(&what, &goal_run(&m, &prev, &[b], &path(0x51_0000 + k as u64, 40), false));
        }
    }
    println!("profile run {:.0} ms", ms(t.elapsed()));
}

/// One goal series: the boxes dragged and their path.
type GoalSeries = (Vec<BoxId>, Vec<(i32, i32)>);

/// A/B of `probe::set_variant` on the goal series, interleaved step by
/// step so machine load hits both alike: `GOAL_AB=<variant>`, with
/// `GOAL_PROFILE`/`GOAL_STEPS` as `perf_drag_goal_profile`.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_goal_ab() {
    let (Ok(what), Ok(v)) = (std::env::var("GOAL_PROFILE"), std::env::var("GOAL_AB")) else {
        println!("perf_drag_goal_ab: set GOAL_PROFILE and GOAL_AB");
        return;
    };
    let v: u32 = v.parse().expect("variant");
    let (view, kind) = what.split_once(':').expect("view:kind");
    let m = if view == "generated" {
        model(GENERATED)
    } else {
        std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model).expect("docs parse")
    };
    let prev = full(&m, view, &Keep::default());
    let steps: usize = std::env::var("GOAL_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(48);
    let mut series: Vec<GoalSeries> = Vec::new();
    if kind == "8" {
        series.push((cluster(&prev, hub(&prev.plan), 8), pointer_path(0x8B0C, 48)));
    } else {
        for (k, b) in goal_singles(&prev.plan).into_iter().enumerate() {
            series.push((vec![b], pointer_path(0x51_0000 + k as u64, 40)));
        }
    }
    let reps: usize = std::env::var("GOAL_REPS").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for (boxes, path) in &series {
        for &step in path.iter().take(steps) {
            for _ in 0..reps {
                probe::set_variant(0);
                a.extend(goal_run(&m, &prev, boxes, &[step], false));
                probe::set_variant(v);
                b.extend(goal_run(&m, &prev, boxes, &[step], false));
            }
        }
    }
    probe::set_variant(0);
    goal_line(&format!("{what} current"), &a);
    goal_line(&format!("{what} variant {v}"), &b);
}

/// One chained run of a goal series (`perf_drag_goal_chained`): each step
/// previewed from the previous step's preview, from the pre-drag build at
/// the start and after every `reset` steps (`None`: never), on every
/// `stride`-th step of `path` only (dropped frames). Returns the steps
/// taken (index into `path`) and their measurements.
#[allow(clippy::too_many_arguments)]
fn chained_run(m: &Model, prev: &ViewBuild, boxes: &[BoxId], path: &[(i32, i32)], reset: Option<usize>, stride: usize) -> Vec<(usize, GoalStep)> {
    let words = label::Words::of(&m.terminology);
    let mut base = prev.clone();
    let mut out = Vec::new();
    for (n, k) in (stride - 1..path.len()).step_by(stride).enumerate() {
        if reset.is_some_and(|r| n % r == 0) {
            base = prev.clone();
        }
        let (dx, dy) = path[k];
        let (step, build) = goal_step(m, &base, &shifted(prev, boxes, dx, dy), false, &words);
        out.push((k, step));
        base = build;
    }
    out
}

/// Evidence for starting each drag step from the previous preview (no
/// editor change): the goal series of `perf_drag_goal`, chained with
/// resets every 8, 16 or no steps, and on every 3rd step (dropped
/// frames), each against the unchained preview of the same steps (U/C/X
/// side by side, so drift shows). One run each.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_goal_chained() {
    if debug_build("perf_drag_goal_chained") {
        return;
    }
    let mut views: Vec<(&str, Model, &str)> = vec![("generated", model(GENERATED), "generated")];
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) {
        views.push(("C3-entry", m, "C3-entry"));
    }
    for (name, m, key) in &views {
        let prev = full(m, key, &Keep::default());
        println!("loadavg {}", std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim());
        view_summary(name, &prev);
        let h = hub(&prev.plan);
        let mut series: Vec<(String, GoalSeries)> = vec![(format!("{name} 8 boxes"), (cluster(&prev, h, 8), pointer_path(0x8B0C, 48)))];
        for (k, b) in goal_singles(&prev.plan).into_iter().enumerate() {
            series.push((format!("{name} single {}", b.0), (vec![b], pointer_path(0x51_0000 + k as u64, 40))));
        }
        let mut pooled: BTreeMap<String, (Vec<GoalStep>, Vec<GoalStep>)> = BTreeMap::new();
        for (sname, (boxes, path)) in &series {
            let plain = goal_run(m, &prev, boxes, path, false);
            for (label, reset, stride) in [("chain no reset", None, 1), ("chain every 2nd", None, 2), ("chain every 3rd", None, 3), ("chain every 5th", None, 5)] {
                let run = chained_run(m, &prev, boxes, path, reset, stride);
                let same: Vec<GoalStep> = run.iter().map(|&(k, _)| plain[k]).collect();
                let steps: Vec<GoalStep> = run.into_iter().map(|(_, s)| s).collect();
                let group = if boxes.len() > 1 { format!("{name} 8 boxes") } else { format!("{name} single pooled") };
                let e = pooled.entry(format!("{group} {label}")).or_default();
                e.0.extend(steps);
                e.1.extend(same);
                let _ = sname;
            }
        }
        for (k, (chained, plain)) in &pooled {
            goal_line(k, chained);
            goal_line(&format!("{k} (unchained, same steps)"), plain);
        }
    }
}

/// Per treatment: lines, ms, flood states, A* pops (`perf_drag_goal_diag`).
type Cats = BTreeMap<String, (u32, f64, u64, u64)>;

/// Round-7 diagnosis: per step of the goal 8-box series (and of each box's
/// single series when `GOAL_DIAG_SINGLE` is set), unchained and chained
/// (each step from the previous preview, never reset), how many lines were
/// untouched, carried, rejoined, or searched in full and why, with the
/// time, flood states and A* pops of each kind, summed over the series
/// and for its 4 slowest steps.
#[test]
#[ignore = "perf: drag measurement, release only"]
fn perf_drag_goal_diag() {
    if debug_build("perf_drag_goal_diag") {
        return;
    }
    let mut views: Vec<(&str, Model, &str)> = vec![("generated", model(GENERATED), "generated")];
    if let Some(m) = std::fs::read_to_string(DOCS).ok().and_then(|s| parse(&s).ok()).map(|p| p.model) {
        views.push(("C3-entry", m, "C3-entry"));
    }
    let add = |into: &mut Cats, st: &Stats| {
        for (name, n, t, f, p) in &st.cats {
            let e = into.entry(name.clone()).or_default();
            e.0 += n;
            e.1 += ms(*t);
            e.2 += f;
            e.3 += p;
        }
    };
    let show = |title: &str, cats: &Cats, steps: usize| {
        println!("  {title}");
        for (name, (n, t, f, p)) in cats {
            let k = steps.max(1) as f64;
            println!("    {name:<42} lines/step {:>5.1} | ms/step {:>6.1} | flood states/step {:>9.0} | A* pops/step {:>8.0}", f64::from(*n) / k, t / k, *f as f64 / k, *p as f64 / k);
        }
    };
    for (name, m, key) in &views {
        let prev = full(m, key, &Keep::default());
        println!("loadavg {}", std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim());
        view_summary(name, &prev);
        let edges = prev.plan.edges.len();
        let boxes = cluster(&prev, hub(&prev.plan), 8);
        let path = pointer_path(0x8B0C, 48);
        for chained in [false, true] {
            println!("== {name} 8 boxes, {}", if chained { "chained (no reset)" } else { "unchained" });
            let mut base = prev.clone();
            let mut all: Cats = BTreeMap::new();
            let mut steps: Vec<(f64, f64, Stats)> = Vec::new();
            for (i, &(dx, dy)) in path.iter().enumerate() {
                let moved = shifted(&prev, &boxes, dx, dy);
                let (s, b) = preview(m, &base, &moved);
                let st = &s.stats;
                let line: Vec<String> = st.cats.iter().map(|c| format!("{}={}", c.0, c.1)).collect();
                println!(
                    "  step {i:>2} ({dx:>3},{dy:>3}) total {:>6.1} route {:>6.1} | untouched {} carried {} | {}",
                    ms(s.total),
                    ms(s.reroute),
                    edges - st.broken,
                    st.translated,
                    line.join(", ")
                );
                add(&mut all, st);
                steps.push((ms(s.total), ms(s.reroute), s.stats.clone()));
                if chained {
                    base = b;
                }
            }
            show("whole series, per step:", &all, steps.len());
            let mut order: Vec<usize> = (0..steps.len()).collect();
            order.sort_by(|&a, &b| steps[b].0.total_cmp(&steps[a].0));
            let mut worst: Cats = BTreeMap::new();
            for &k in &order[..4] {
                add(&mut worst, &steps[k].2);
            }
            let tot: Vec<f64> = steps.iter().map(|s| s.0).collect();
            println!(
                "  median {:.1} ms, p90 {:.1} ms; 4 slowest steps {:?} at {:?} ms",
                pct(&tot, 50),
                pct(&tot, 90),
                &order[..4],
                order[..4].iter().map(|&k| (steps[k].0 * 10.0).round() / 10.0).collect::<Vec<_>>()
            );
            show("4 slowest steps, per step:", &worst, 4);
        }
    }
}
