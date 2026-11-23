//! The P5-900 benchmark line (test builds only): times a body, prints
//! one tab-separated `BENCH` line and asserts the median is within its
//! budget. Std only, so an integration test can include this file with
//! `#[path = "../src/bench_util.rs"] mod bench_util;`.
// DECISION: in `src` under `cfg(test)`, not under `tests/`: the line
// colour pass it times is crate-private (`scene::line_colour`), and a
// std-only file can still be shared with the integration tests by path.
#![allow(dead_code, clippy::cast_precision_loss)]

use std::time::{Duration, Instant};

/// Timed runs after the warm-up (P5-900: at least 11).
pub(crate) const RUNS: usize = 11;

/// One benchmark: the fields of its `BENCH` line before the numbers.
pub(crate) struct Bench<'a> {
    /// Requirement id, e.g. `P5-312`.
    pub(crate) id: &'a str,
    /// Feature row, e.g. `line colour`.
    pub(crate) feature: &'a str,
    /// What is timed.
    pub(crate) what: &'a str,
    /// The input.
    pub(crate) input: &'a str,
    /// The budget for the median, in microseconds.
    pub(crate) budget_us: f64,
}

/// True (after printing why) when this is not a release build.
pub(crate) fn debug_build(test: &str) -> bool {
    if cfg!(debug_assertions) {
        println!("{test}: skipped, timing needs `cargo test --release`");
        return true;
    }
    false
}

impl Bench<'_> {
    /// Runs `f` once to warm up, then `RUNS` timed runs, each looping `f`
    /// enough times to take over 1 ms (time divided by the loop count);
    /// prints the `BENCH` line and returns the median in microseconds.
    ///
    /// # Panics
    ///
    /// When the median is over `budget_us`.
    pub(crate) fn run(&self, mut f: impl FnMut()) -> f64 {
        let t = Instant::now();
        f();
        let once = t.elapsed().max(Duration::from_nanos(1));
        let inner = u32::try_from(Duration::from_millis(1).as_nanos() / once.as_nanos() + 1).unwrap_or(u32::MAX);
        let mut times: Vec<f64> = (0..RUNS)
            .map(|_| {
                let t = Instant::now();
                for _ in 0..inner {
                    f();
                }
                t.elapsed().as_secs_f64() * 1e6 / f64::from(inner)
            })
            .collect();
        times.sort_by(f64::total_cmp);
        let (median, min, max) = (times[RUNS / 2], times[0], times[RUNS - 1]);
        println!(
            "BENCH\t{}\t{}\t{}\t{}\t{median:.1}\t{min:.1}\t{max:.1}\t{RUNS}\t{:.0}",
            self.id, self.feature, self.what, self.input, self.budget_us
        );
        assert!(
            median <= self.budget_us,
            "{} {}: median {median:.1} us over the budget {:.0} us",
            self.id,
            self.what,
            self.budget_us
        );
        median
    }
}
