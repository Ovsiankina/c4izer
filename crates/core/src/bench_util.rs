//! The P5-900 benchmark helper (test builds only), the one of this
//! crate: times a body, prints one tab-separated `BENCH` line and asserts
//! the median is within its budget. Std only, so an integration test
//! includes this file with `#[path = "../src/bench_util.rs"] mod bench_util;`.
//! The editor's `testutil::bench` has the same signature and output.
// DECISION: in `src` under `cfg(test)`, not under `tests/`: the line
// colour pass it times is crate-private (`scene::line_colour`), and a
// std-only file can still be shared with the integration tests by path.
#![allow(dead_code, clippy::cast_precision_loss)]

use std::time::{Duration, Instant};

/// Timed runs after the warm-up.
pub(crate) const RUNS: usize = 21;

/// The feature column's only values, one per phase 5 row.
pub(crate) const FEATURES: [&str; 8] = [
    "groups",
    "tags",
    "line colour",
    "line style",
    "lint resolve",
    "guides",
    "view folders",
    "map tree",
];

/// True (after printing why) when this is not a release build: a timing
/// in a debug build means nothing.
pub(crate) fn debug_build(test: &str) -> bool {
    if cfg!(debug_assertions) {
        println!("{test}: skipped, timing needs `cargo test --release`");
        return true;
    }
    false
}

/// Times `body` labelled `[id, feature, what, input]`: one warm-up call,
/// then `RUNS` runs, each looping `body` enough times to take over 1 ms
/// (the time divided by the loop count); prints
/// `BENCH\t<id>\t<feature>\t<what>\t<input>\t<median_us>\t<min_us>\t<max_us>\t<runs>\t<budget_us>`
/// (microseconds, three decimals) and returns the median.
///
/// # Panics
///
/// When `feature` is not one of `FEATURES`, or the median is over
/// `budget_us`.
// DECISION: the loop count is set once from the warm-up, so every timed
// run does the same work.
pub(crate) fn bench([id, feature, what, input]: [&str; 4], budget_us: u64, mut body: impl FnMut()) -> f64 {
    assert!(FEATURES.contains(&feature), "{id}: unknown feature {feature:?}");
    let t = Instant::now();
    body();
    let once = t.elapsed().max(Duration::from_nanos(1));
    let reps = u32::try_from(Duration::from_millis(1).as_nanos() / once.as_nanos() + 1).unwrap_or(u32::MAX);
    let mut times: Vec<f64> = (0..RUNS)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..reps {
                body();
            }
            t.elapsed().as_secs_f64() * 1e6 / f64::from(reps)
        })
        .collect();
    times.sort_by(f64::total_cmp);
    let (median, min, max) = (times[RUNS / 2], times[0], times[RUNS - 1]);
    println!("BENCH\t{id}\t{feature}\t{what}\t{input}\t{median:.3}\t{min:.3}\t{max:.3}\t{RUNS}\t{budget_us}");
    assert!(
        median <= budget_us as f64,
        "{id} {what} on {input}: median {median:.3} us over the budget {budget_us} us"
    );
    median
}
