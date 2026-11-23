//! Phase 5 benchmarks (SRS P5-900), shared by the core's `perf_p5_*`
//! tests through `#[path = "bench_util/bench.rs"] mod bench;`: one
//! warm-up, at least 11 timed runs, the median against a budget, and one
//! tab-separated `BENCH` line per benchmark.

// DECISION: a file under a folder, included by `#[path]`, so it is not a
// test crate of its own and adds no dependency (design section 9).
// DECISION: times print in microseconds with three decimals: a snap
// query or a short loop body is well under one microsecond.

use std::time::{Duration, Instant};

/// Timed runs per benchmark (design section 9: at least 11).
pub const RUNS: usize = 11;

/// True (after printing why) when this is not a release build: a timing
/// in a debug build means nothing.
pub fn debug_build(test: &str) -> bool {
    if cfg!(debug_assertions) {
        println!("{test}: skipped, timing needs `cargo test --release`");
        return true;
    }
    false
}

/// Times `body`: one warm-up call, then `RUNS` runs, each looping `body`
/// enough times to take over 1 ms and divided by that count; prints
/// `BENCH\t<id>\t<feature>\t<what>\t<input>\t<median_us>\t<min_us>\t<max_us>\t<runs>\t<budget_us>`
/// and asserts `median <= budget_us`.
#[expect(clippy::too_many_arguments, reason = "the six fields of the BENCH line, then the body")]
pub fn bench(id: &str, feature: &str, what: &str, input: &str, budget_us: u64, mut body: impl FnMut()) {
    let t = Instant::now();
    body();
    let once = t.elapsed().max(Duration::from_nanos(1));
    let reps = u32::try_from((Duration::from_millis(1).as_nanos() / once.as_nanos()) + 1).unwrap_or(u32::MAX);
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
    #[expect(clippy::cast_precision_loss, reason = "budgets are far below 2^52 us")]
    let budget = budget_us as f64;
    assert!(median <= budget, "{id}: median {median:.3} us over the budget {budget_us} us");
}
