//! Router quality bar on `tests/data/example.dsl` (CORE-11, CORE-12,
//! route-06). No `perf_*` here: CORE-24, 27 and 31 belong to pipeline.

use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::{GRID_PX, Point};
use c4rizer_core::model::parse;
use c4rizer_core::route::{RouteConfig, Routes, route};
use c4rizer_core::view::{ViewPlan, plan};

const EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/data/example.dsl"
));

/// The plan of view `key` and its centres: every box of the example has
/// an `at` line, so no placement is needed.
fn view(key: &str) -> (ViewPlan, Vec<Point>) {
    let model = match parse(EXAMPLE) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    };
    let p = plan(&model, model.view(key).expect("view exists"));
    let centres = p
        .boxes
        .iter()
        .map(|b| b.centre.expect("every example box has an `at` line"))
        .collect();
    (p, centres)
}

fn routed(key: &str) -> Routes {
    let (p, c) = view(key);
    route(&p, &c, &RouteConfig::DEFAULT, &AtomicBool::new(false)).expect("never stopped")
}

/// CORE-11: every view of the example routes with crossings no higher
/// than the prototype's: C1 0, C2 0, C3 <= 1, C3-startup 0.
#[test]
fn core_11_example_crossings_no_worse() {
    for (key, bar) in [("C1", 0), ("C2", 0), ("C3-serving", 1), ("C3-startup", 0)] {
        let r = routed(key);
        assert!(
            r.unroutable().is_empty(),
            "{key}: unroutable {:?}",
            r.unroutable()
        );
        assert!(
            r.crossings <= bar,
            "{key}: {} crossings, prototype {bar}",
            r.crossings
        );
    }
}

/// CORE-12: every view of the example has a summed lane length
/// (`Routes::lanes`) no lower than the prototype's: C1 0px, C2 70px,
/// C3 220px, C3-startup 110px (SRS CORE-12, measured once on the
/// prototype's output).
#[test]
fn core_12_example_lane_length_no_worse() {
    for (key, bar) in [("C1", 0), ("C2", 70), ("C3-serving", 220), ("C3-startup", 110)] {
        let px: i32 = routed(key)
            .lanes()
            .iter()
            .map(|l| l.a_run.len_steps().expect("axis run") * GRID_PX)
            .sum();
        assert!(px >= bar, "{key}: lanes {px}px, prototype {bar}px");
    }
}


/// Every perpendicular gap, in cells, between two axis runs of different
/// lines that head the same way, share a stretch and lie 1..=3 cells
/// apart (the unit tests' `same_way_gaps`, on the pub API).
fn same_way_gaps(r: &Routes) -> Vec<i32> {
    use c4rizer_core::geom::{Dir, Segment};
    let runs: Vec<Vec<(Dir, Point, Point)>> = r
        .lines
        .iter()
        .flatten()
        .map(|l| {
            l.points
                .windows(2)
                .filter_map(|w| {
                    let d = Segment::new(w[0], w[1]).dir()?;
                    (!d.is_diagonal()).then_some((d, w[0], w[1]))
                })
                .collect()
        })
        .collect();
    let mut gaps = Vec::new();
    for (i, a) in runs.iter().enumerate() {
        for b in &runs[i + 1..] {
            for &(da, a0, a1) in a {
                for &(db, b0, b1) in b {
                    if da != db {
                        continue;
                    }
                    let horizontal = matches!(da, Dir::E | Dir::W);
                    let (across, along) = if horizontal {
                        ((a0.y - b0.y).abs(), [a0.x, a1.x, b0.x, b1.x])
                    } else {
                        ((a0.x - b0.x).abs(), [a0.y, a1.y, b0.y, b1.y])
                    };
                    let lo = along[0].min(along[1]).max(along[2].min(along[3]));
                    let hi = along[0].max(along[1]).min(along[2].max(along[3]));
                    if hi > lo && (1..=3).contains(&across) {
                        gaps.push(across);
                    }
                }
            }
        }
    }
    gaps
}

/// CORE-12: on every view of the example, lines heading the same way
/// that run beside each other are exactly 20px (2 cells) apart.
#[test]
fn core_12_example_same_way_gaps_are_20px() {
    for key in ["C1", "C2", "C3-serving", "C3-startup"] {
        let gaps = same_way_gaps(&routed(key));
        assert!(gaps.iter().all(|&g| g == 2), "{key}: same-way gaps {gaps:?}");
    }
}
