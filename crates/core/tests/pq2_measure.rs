//! Scratch measurement for place-quality-2 (not for dev).
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
use c4rizer_core::lint::{Rule, Severity};
use c4rizer_core::model::parse;
use c4rizer_core::pipeline::{ViewBuild, build_view};
use c4rizer_core::place::{self, Keep};

const WS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example/workspace.dsl"));
const EX: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/example.dsl"));
const GEN: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/core_31_generated.dsl"));

fn strip(s: &str) -> String {
    s.lines().filter(|l| !l.trim_start().starts_with("at ")).collect::<Vec<_>>().join("\n")
}

fn length_px(b: &ViewBuild) -> i64 {
    let mut milli: i64 = 0;
    for line in b.routes.lines.iter().flatten() {
        for w in line.points.windows(2) {
            let (dx, dy) = ((w[1].x - w[0].x).abs(), (w[1].y - w[0].y).abs());
            let steps = i64::from(dx.max(dy));
            milli += if dx != 0 && dy != 0 { steps * 14_142 } else { steps * 10_000 };
        }
    }
    milli / 1000
}

fn med(runs: usize, mut f: impl FnMut()) -> Duration {
    f();
    let mut t: Vec<Duration> = (0..runs).map(|_| { let s = Instant::now(); f(); s.elapsed() }).collect();
    t.sort();
    t[runs / 2]
}

#[test]
#[ignore = "scratch"]
fn pq2_measure() {
    let runs: usize = std::env::var("PQ2_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
    let srcs = [("workspace.dsl", WS.to_string()), ("example.dsl-noat", strip(EX)), ("core31-noat", strip(GEN))];
    println!("src view boxes edges | cross length L23 noroom warnings | place_ms build_ms");
    for (name, src) in &srcs {
        let m = match parse(src) { Ok(p) => p.model, Err(e) => panic!("{e}") };
        for v in &m.views {
            let plan = c4rizer_core::view::plan(&m, v);
            let tp = med(runs, || { let _ = place::place(&plan, &Keep::default()); });
            let go = AtomicBool::new(false);
            let mut out = None;
            let tb = med(runs, || { out = Some(build_view(&m, &v.key, &Keep::default(), &go).unwrap()); });
            let b = out.unwrap();
            let warn = b.report.findings.iter().filter(|f| f.severity() == Severity::Warning).count();
            let l23 = b.report.findings.iter().filter(|f| f.rule == Rule::LabelOverLine).count();
            println!("{name} {} {} {} | {} {} {} {} {} | {:.1} {:.1}", v.key, plan.boxes.len(), plan.edges.len(),
                b.routes.crossings, length_px(&b), l23, b.labels.no_room().len(), warn,
                tp.as_secs_f64()*1e3, tb.as_secs_f64()*1e3);
        }
    }
}

fn line_len(pts: &[c4rizer_core::geom::Point]) -> i64 {
    let mut milli = 0i64;
    for w in pts.windows(2) {
        let (dx, dy) = ((w[1].x - w[0].x).abs(), (w[1].y - w[0].y).abs());
        let steps = i64::from(dx.max(dy));
        milli += if dx != 0 && dy != 0 { steps * 14_142 } else { steps * 10_000 };
    }
    milli / 1000
}

#[test]
#[ignore = "scratch"]
fn pq2_dump() {
    let key = std::env::var("PQ2_VIEW").unwrap_or("C2".into());
    let m = parse(WS).unwrap().model;
    let b = build_view(&m, &key, &Keep::default(), &AtomicBool::new(false)).unwrap();
    for (i, bx) in b.plan.boxes.iter().enumerate() {
        let c = b.routes.centres[i];
        println!("box {:<18} ({:>4},{:>4}) px  half {}x{}", bx.name, c.x*10, c.y*10, bx.half_width*10, bx.half_height*10);
    }
    for (e, l) in b.plan.edges.iter().zip(&b.routes.lines) {
        let n = |id: c4rizer_core::view::BoxId| b.plan.boxes[id.0 as usize].name.clone();
        println!("edge {:<16} -> {:<16} {:>5}", n(e.source), n(e.destination), l.as_ref().map_or(-1, |l| line_len(&l.points)));
    }
    for f in &b.report.findings { println!("lint {:?} {:?}", f.rule, f.severity()); }
}

#[test]
#[ignore = "scratch"]
fn pq2_quality() {
    let m = parse(WS).unwrap().model;
    let only = std::env::var("PQ2_VIEW").ok();
    for v in &m.views {
        if only.as_ref().is_some_and(|o| o != &v.key) { continue; }
        let t = Instant::now();
        let b = build_view(&m, &v.key, &Keep::default(), &AtomicBool::new(false)).unwrap();
        let warn = b.report.findings.iter().filter(|f| f.severity() == Severity::Warning).count();
        let l23 = b.report.findings.iter().filter(|f| f.rule == Rule::LabelOverLine).count();
        println!("Q {} cross {} len {} L23 {} noroom {} warn {} | {:.0} ms", v.key, b.routes.crossings, length_px(&b), l23, b.labels.no_room().len(), warn, t.elapsed().as_secs_f64()*1e3);
    }
}
