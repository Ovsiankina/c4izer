//! The Phase 5 benchmark helper (P5-900, design section 9): one warm-up,
//! at least 11 timed runs, the median, one `BENCH` line, the budget
//! assert. Included by `#[path]` from each test file that times
//! something; a file under `tests/common/` is not a test crate itself.

#![allow(dead_code, reason = "each including test file uses part of it")]

use std::time::{Duration, Instant};

/// Timed runs per benchmark (design section 9: at least 11).
pub const RUNS: usize = 11;

/// True (after printing why) when this is not a release build.
pub fn debug_build(test: &str) -> bool {
    if cfg!(debug_assertions) {
        println!("{test}: skipped, timing needs `cargo test --release`");
        return true;
    }
    false
}

/// Times `body` and prints
/// `BENCH\t<id>\t<feature>\t<what>\t<input>\t<median_us>\t<min_us>\t<max_us>\t<runs>\t<budget_us>`,
/// then asserts `median <= budget_us`. Work under 1 ms is looped so one
/// run takes over 1 ms, and divided.
// DECISION: the loop count is set once from the warm-up run, so every
// timed run does the same work.
#[expect(clippy::too_many_arguments, reason = "one argument per column of the BENCH line, then the body")]
pub fn run(id: &str, feature: &str, what: &str, input: &str, budget_us: u32, mut body: impl FnMut()) {
    let t = Instant::now();
    body();
    let once = t.elapsed().max(Duration::from_nanos(1));
    let reps = u32::try_from((Duration::from_millis(1).as_nanos() / once.as_nanos()) + 1).unwrap_or(u32::MAX).max(1);
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
    println!("BENCH\t{id}\t{feature}\t{what}\t{input}\t{median:.1}\t{min:.1}\t{max:.1}\t{RUNS}\t{budget_us}");
    assert!(
        median <= f64::from(budget_us),
        "{id} {what}: median {median:.1} us over the budget of {budget_us} us"
    );
}
