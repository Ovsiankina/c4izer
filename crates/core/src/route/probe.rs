//! Test-only counters of one router call (drag preview measurement).
//! Compiled only under `cfg(test)`: every hook in `route.rs` is a
//! `#[cfg(test)]` statement, so a non-test build has none of this.
// DECISION: a thread-local struct private to the crate, reset and read by
// the measurement tests in `route/drag_perf.rs`; no pub item, no layout
// change, nothing in production builds.

use std::cell::RefCell;
use std::time::{Duration, Instant};

/// What one measured router call did.
#[derive(Debug, Clone, Default)]
pub(super) struct Stats {
    /// Edges `broken` picked (the re-routed set), before `forced`.
    pub broken: usize,
    /// Lines of the set a preview kept translated with their boxes.
    pub translated: usize,
    /// Lines a preview rejoined to their previous route (one end moved).
    pub rejoined: usize,
    /// Leg pops of rejoin attempts; attempts with no rejoin cell; legs that
    /// found nothing.
    pub rejoin_pops: u64,
    pub rejoin_nocand: usize,
    pub rejoin_noleg: usize,
    /// Rejoins dropped for crossing more lines than the previous route.
    pub rejoin_worse: usize,
    /// Which solver ran: "solve", "pinned", "bundled", "none".
    pub path: &'static str,
    /// Largest board (cells) of any `solve` in the call.
    pub grid_cells: usize,
    pub solves: u32,
    pub scratch: Duration,
    pub orders: u32,
    /// Every `pass` (first pass of each order + rip-up rounds).
    pub passes: u32,
    /// Crossings / clashes / unroutable of each order's first pass.
    pub first_pass: Vec<(u32, u32, usize)>,
    /// Crossing cells (`Pass::crossed`) of each order's first pass.
    pub first_crossed: Vec<usize>,
    /// `Board::new` + kept stamping inside `pass`.
    pub board: Duration,
    pub searches: u32,
    pub search: Duration,
    pub max_search: Duration,
    pub pops: u64,
    pub max_pops: u32,
    /// Octile attempts that hit `POP_LIMIT`.
    pub gave_up: u32,
    pub cost_from: u32,
    pub cost_from_time: Duration,
    pub score: Duration,
    /// Longest wall time between two reads of the stop flag (from the
    /// call's start to its first check, and from its last check to the
    /// end, included).
    pub max_gap: Duration,
    /// The reads of the stop flag before and after `max_gap`.
    pub gap_from: Option<&'static std::panic::Location<'static>>,
    pub gap_to: Option<&'static std::panic::Location<'static>>,
    last_at: Option<&'static std::panic::Location<'static>>,
    /// Most search states (A* pops, `cost_from` cells, leg pops) handled
    /// between two reads of the stop flag: the gap in work, not time, so
    /// a test can bound it on any machine.
    pub max_work: u64,
    work: u64,
    /// Searches that found no path, and the A* pops they spent.
    pub none: u32,
    pub none_pops: u64,
    /// Searches whose window found no path, retried on the whole board.
    pub retried: u32,
    /// Searches over 100k pops: count, pops, how many found nothing.
    pub big: u32,
    pub big_pops: u64,
    pub big_none: u32,
    /// `cost_from` cells popped in all.
    pub cf_cells: u64,
    /// `cost_from_dir` states popped, and the time it took.
    pub dir_states: u64,
    pub dir_time: Duration,
    pub dir_stale: u64,


    /// Why the current search went the way it did (`search`, `rejoin`).
    pub reason: &'static str,
    /// Per treatment of a searched line: name, lines, time, flood states,
    /// A* pops.
    pub cats: Vec<(String, u32, Duration, u64, u64)>,

    /// `no_way_back` runs, proofs, most states a proof took, states in all.
    pub back: u32,
    pub back_proved: u32,
    pub back_max_proof: u32,
    pub back_states: u64,
    last_check: Option<Instant>,
    /// Cost of the last path `astar` found.
    pub last_cost: u32,
}

thread_local! {
    static STATS: RefCell<Stats> = RefCell::new(Stats::default());
    static EFFORT: RefCell<Option<super::Effort>> = const { RefCell::new(None) };
    static NO_FULL_PROOFS: RefCell<bool> = const { RefCell::new(false) };
    static SEQUENTIAL: RefCell<bool> = const { RefCell::new(false) };
    static VARIANT: RefCell<u32> = const { RefCell::new(0) };
}

/// A/B switch for measurements: bits select older code paths (0: the
/// current code). Bit 0: the preview's per-cell bound instead of the
/// per-(cell, heading) one; bit 2: no rejoin retry with every port;
/// bit 3: FIFO ties; bit 4: the rejoin as it was (no stub filter, octile
/// leg, no guard); bit 5: the rejoin guard on this board.
pub(super) fn set_variant(v: u32) {
    VARIANT.with(|x| *x.borrow_mut() = v);
}

pub(super) fn variant() -> u32 {
    VARIANT.with(|x| *x.borrow())
}

/// Forces the full build's edge orders onto one thread (`true`) to
/// compare against.
pub(super) fn set_sequential(on: bool) {
    SEQUENTIAL.with(|x| *x.borrow_mut() = on);
}

pub(super) fn sequential() -> bool {
    SEQUENTIAL.with(|x| *x.borrow())
}

/// Adds the counters of another thread's run (an edge order routed on a
/// worker thread) to this thread's.
pub(super) fn merge(o: &Stats) {
    with(|s| {
        s.orders += o.orders;
        s.passes += o.passes;
        s.first_pass.extend_from_slice(&o.first_pass);
        s.first_crossed.extend_from_slice(&o.first_crossed);
        s.board += o.board;
        s.searches += o.searches;
        s.search += o.search;
        s.max_search = s.max_search.max(o.max_search);
        s.pops += o.pops;
        s.max_pops = s.max_pops.max(o.max_pops);
        s.gave_up += o.gave_up;
        s.cost_from += o.cost_from;
        s.cost_from_time += o.cost_from_time;
        s.score += o.score;
        s.max_gap = s.max_gap.max(o.max_gap);
        s.max_work = s.max_work.max(o.max_work);
        s.none += o.none;
        s.none_pops += o.none_pops;
        s.retried += o.retried;
        s.big += o.big;
        s.big_pops += o.big_pops;
        s.big_none += o.big_none;
        s.cf_cells += o.cf_cells;
        s.dir_states += o.dir_states;
        s.dir_time += o.dir_time;
        s.dir_stale += o.dir_stale;
        s.rejoined += o.rejoined;
        s.rejoin_pops += o.rejoin_pops;
        s.back += o.back;
        s.back_proved += o.back_proved;
        s.back_max_proof = s.back_max_proof.max(o.back_max_proof);
        s.back_states += o.back_states;
    });
}

/// The settings a worker thread inherits from the thread that spawned it.
pub(super) fn settings() -> (bool, bool) {
    (no_full_proofs(), sequential())
}

pub(super) fn set_settings((no_proofs, seq): (bool, bool)) {
    set_no_full_proofs(no_proofs);
    set_sequential(seq);
}

/// Turns the full build's no-path proofs off (`true`) to compare against.
pub(super) fn set_no_full_proofs(off: bool) {
    NO_FULL_PROOFS.with(|x| *x.borrow_mut() = off);
}

pub(super) fn no_full_proofs() -> bool {
    NO_FULL_PROOFS.with(|x| *x.borrow())
}

/// A preview effort to try instead of `PREVIEW_EFFORT` (measurement).
pub(super) fn set_effort(e: Option<super::Effort>) {
    EFFORT.with(|x| *x.borrow_mut() = e);
}

pub(super) fn effort() -> Option<super::Effort> {
    EFFORT.with(|x| *x.borrow())
}

/// Clears the counters and starts the stop-gap clock.
pub(super) fn reset() {
    STATS.with(|s| {
        *s.borrow_mut() = Stats {
            last_check: Some(Instant::now()),
            ..Stats::default()
        };
    });
}

/// Closes the last stop gap and returns the counters.
pub(super) fn take() -> Stats {
    check();
    STATS.with(|s| s.borrow().clone())
}

pub(super) fn with_get<T>(f: impl FnOnce(&Stats) -> T) -> T {
    STATS.with(|s| f(&s.borrow()))
}

/// Files the search just ended under its `reason` (with "rip-up" when
/// it ran in a rip-up round), from the clock, flood states and pops at its
/// start.
pub(super) fn categorise(t0: Instant, flood0: u64, pops0: u64, passes: u32) {
    with(|s| {
        let name = if passes > 1 { format!("rip-up round, {}", s.reason) } else { s.reason.to_owned() };
        let (t, f, p) = (t0.elapsed(), s.dir_states - flood0, s.pops - pops0);
        if let Some(c) = s.cats.iter_mut().find(|c| c.0 == name) {
            c.1 += 1;
            c.2 += t;
            c.3 += f;
            c.4 += p;
        } else {
            s.cats.push((name, 1, t, f, p));
        }
    });
}

pub(super) fn with(f: impl FnOnce(&mut Stats)) {
    STATS.with(|s| f(&mut s.borrow_mut()));
}

/// A read of the stop flag.
#[track_caller]
pub(super) fn check() {
    let at = std::panic::Location::caller();
    with(|s| {
        let now = Instant::now();
        if let Some(t) = s.last_check
            && now - t > s.max_gap
        {
            s.max_gap = now - t;
            s.gap_from = s.last_at;
            s.gap_to = Some(at);
        }
        s.last_check = Some(now);
        s.last_at = Some(at);
        s.max_work = s.max_work.max(s.work);
        s.work = 0;
    });
}

/// `n` search states handled since the loop last reported.
pub(super) fn work(n: u32) {
    with(|s| s.work += u64::from(n));
}

/// One search's end: its pops (`pops` before and after) and outcome.
pub(super) fn searched(pops_before: u64, found: bool) {
    with(|s| {
        let n = s.pops - pops_before;
        if !found {
            s.none += 1;
            s.none_pops += n;
        }
        if n > 100_000 {
            s.big += 1;
            s.big_pops += n;
            s.big_none += u32::from(!found);
        }
    });
}

/// Total A* pops so far.
pub(super) fn pops_so_far() -> u64 {
    STATS.with(|s| s.borrow().pops)
}

/// The pops of one A* attempt, at its exit.
pub(super) fn pops(n: u32, gave_up: bool) {
    with(|s| {
        s.pops += u64::from(n);
        s.max_pops = s.max_pops.max(n);
        s.gave_up += u32::from(gave_up);
        // The loop reported every whole `STOP_EVERY` at its stop reads.
        s.work += u64::from(n % super::STOP_EVERY);
    });
}
