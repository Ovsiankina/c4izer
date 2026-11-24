# Benchmark datasheet

Timings of the features of SRS 4.9 (owner requests 2026-11-23, GitHub
issues #1 to #11). Each row is a floor: the budget is what the oracle
enforces from now on, the median is what there is to beat.

| | |
|---|---|
| Date | 2026-11-23 |
| Commit | `907074d` on `worktree-owner-requests-2026-11-23` (base `dev` 255bd5f) |
| Machine | Intel Core i9-13900T, 32 logical CPUs, 62 GB, Linux 7.3.4 |
| Toolchain | rustc 1.97.1, `--release` |
| Command | `cargo test --workspace --release -- --ignored perf_p5 --nocapture --test-threads=1` |
| Load | nothing else building or running tests |

## Method

- Every benchmark is a test named `perf_p5_<requirement>_<what>`,
  `#[ignore]`d and skipped in a debug build (SRS P5-900).
- One warm-up call, then 21 runs. A run loops the body until it takes
  over 1 ms and divides, so sub-microsecond work is measured.
- Reported: median, fastest and slowest run, in microseconds (us).
  1,000 us = 1 ms. One frame at 60 Hz is 16,667 us.
- Budget: about 4x the highest median of three quiet runs, rounded up,
  never under 50 us. A median over its budget fails the test.
- Headroom = budget / median of this run.
- Helpers: `crates/core/src/bench_util.rs` and `bench` in
  `crates/editor/src/testutil.rs`. Both print the same `BENCH` line
  this sheet is made from.

## Summary

| feature | issue | slowest row (median) | runs when |
|---|---|---|---|
| groups | #1, OR-11 | 1,091.8 us (SetGroup create, move in, move out, example/workspace.dsl, 3 boxes) | once per Group / Remove from group action |
| tags | #2, OR-12 | 825.9 us (AddTag then RemoveTag apply, example/workspace.dsl) | once per tag added or removed |
| line colour | #3 #4, OR-13 OR-14 | 70.84 us (colour pass alone, generated 1000 boxes / 3000 lines) | once per scene build of a `lineColours box` view |
| line style | #5 #6, OR-15 OR-16 | 746.0 us (SetLineStyle this line, apply, example/workspace.dsl) | once per colour change of a line |
| lint resolve | #7, OR-17 | 596.9 us (resolve_all, generated 400 boxes) | `resolve`: once per finding when a build is shown; `resolve_all`: once per click |
| guides | #8, OR-18 | 77.59 us (index build, 1000 boxes) | index: once per drag start; snap query: every pointer move of a drag |
| view folders | #10 #11, OR-20 OR-21 | 87.59 us (folder list build, 500 views) | list build: once per parse or view change; frame: every frame |
| map tree | #9, OR-19 | 90.32 us (map panel frame, tree mode, 500 views) | rows build: once per parse; frame: every frame |

## Groups (#1, OR-11)

| id | benchmark | input | median us | min us | max us | budget us | headroom |
|---|---|---|---:|---:|---:|---:|---:|
| P5-100 | SetGroup create, move in, move out | example/workspace.dsl, 3 boxes | 1,091.8 | 889.6 | 1,650.1 | 7,000 | 6.4x |

## Tags (#2, OR-12)

| id | benchmark | input | median us | min us | max us | budget us | headroom |
|---|---|---|---:|---:|---:|---:|---:|
| P5-200 | AddTag then RemoveTag apply | example/workspace.dsl | 825.9 | 433.8 | 1,454.7 | 2,200 | 2.7x |

## Line colour (#3 #4, OR-13 OR-14)

| id | benchmark | input | median us | min us | max us | budget us | headroom |
|---|---|---|---:|---:|---:|---:|---:|
| P5-312 | colour pass alone | generated 40 boxes / 60 lines | 0.809 | 0.787 | 1.11 | 50 | 61.8x |
| P5-312 | colour pass alone | generated 1000 boxes / 3000 lines | 70.84 | 70.50 | 73.81 | 300 | 4.2x |
| P5-312 | scene build, lineColours box | example.dsl C3-serving | 51.20 | 50.34 | 51.59 | 260 | 5.1x |
| P5-312 | scene build, plain | example.dsl C3-serving | 50.84 | 49.65 | 55.32 | 260 | 5.1x |

## Line style (#5 #6, OR-15 OR-16)

| id | benchmark | input | median us | min us | max us | budget us | headroom |
|---|---|---|---:|---:|---:|---:|---:|
| P5-360 | SetLineStyle this line, apply | example/workspace.dsl | 746.0 | 259.5 | 998.1 | 2,200 | 2.9x |

## Lint resolve (#7, OR-17)

| id | benchmark | input | median us | min us | max us | budget us | headroom |
|---|---|---|---:|---:|---:|---:|---:|
| P5-400 | resolve L6 | generated 40 boxes | 0.275 | 0.271 | 0.518 | 50 | 181.8x |
| P5-400 | resolve L6 | generated 400 boxes | 6.76 | 4.45 | 7.01 | 50 | 7.4x |
| P5-405 | resolve_all | generated 40 boxes | 7.65 | 7.59 | 7.92 | 50 | 6.5x |
| P5-405 | resolve_all | generated 400 boxes | 596.9 | 592.2 | 949.5 | 2,500 | 4.2x |

## Guides (#8, OR-18)

| id | benchmark | input | median us | min us | max us | budget us | headroom |
|---|---|---|---:|---:|---:|---:|---:|
| P5-500 | index build | 40 boxes | 2.60 | 2.48 | 3.03 | 50 | 19.2x |
| P5-500 | index build | 1000 boxes | 77.59 | 74.60 | 91.39 | 450 | 5.8x |
| P5-501 | drag_to with guides | example C3-serving | 11.80 | 10.78 | 16.24 | 60 | 5.1x |
| P5-501 | drag_to without guides | example C3-serving | 10.42 | 10.36 | 13.52 | 60 | 5.8x |
| P5-501 | snap query + guides | 40 boxes | 0.131 | 0.087 | 0.187 | 50 | 381.7x |
| P5-501 | snap query + guides | 1000 boxes | 0.178 | 0.177 | 0.180 | 50 | 280.9x |

## View folders (#10 #11, OR-20 OR-21)

| id | benchmark | input | median us | min us | max us | budget us | headroom |
|---|---|---|---:|---:|---:|---:|---:|
| P5-606 | folder list build | 50 views | 6.91 | 6.80 | 9.87 | 50 | 7.2x |
| P5-606 | folder list build | 500 views | 87.59 | 68.58 | 118.8 | 450 | 5.1x |
| P5-606 | view list frame | 500 views | 62.63 | 52.95 | 107.0 | 250 | 4.0x |

## Map tree (#9, OR-19)

| id | benchmark | input | median us | min us | max us | budget us | headroom |
|---|---|---|---:|---:|---:|---:|---:|
| P5-705 | map panel frame, tree mode | 500 views | 90.32 | 85.15 | 113.2 | 350 | 3.9x |
| P5-705 | tree rows build | 50 views | 3.93 | 3.88 | 4.33 | 50 | 12.7x |
| P5-705 | tree rows build | 500 views | 47.05 | 46.73 | 48.46 | 200 | 4.3x |

## Reading the numbers

- **Per pointer move** (the path that decides how a drag feels): the
  guide snap query costs under 0.2 us with 1,000 boxes. `drag_to` with
  guides and without differ by about 1 us on C3-serving, inside the
  run-to-run spread.
- **Per frame**: the view list with 500 views and the map in tree mode
  with 500 views each take under 0.1 ms, under 1% of a 60 Hz frame.
- **Per scene build**: `lineColours box` adds nothing measurable to the
  C3-serving scene build (both about 51 us); the colour pass alone is
  71 us for 1,000 boxes and 3,000 lines. A `plain` view does not run it.
- **Per edit**: the DSL edits (group, tag, line style) take 0.7 to 1.1
  ms on the example workspace. Almost all of it is the re-parse after
  each patch, the same cost every existing edit pays; the spread
  between runs is wide (fastest to slowest about 2x to 4x).
- **Per click**: "Resolve all" on 400 crowded boxes takes 0.6 ms.

## Existing budgets, same run

Not part of phase 5; listed so a regression from this work would show.
Command: `cargo test --workspace --release -- --ignored perf_ --nocapture --test-threads=1`
(49 passed, 0 failed).

| test | measured | budget |
|---|---:|---:|
| `perf_core_24` re-render after a move | 52.7 ms | 100 ms |
| `perf_core_27` first view | 52.4 ms | 300 ms |
| `perf_core_31` generated view, 40 boxes / 60 lines | 262.9 ms | 1000 ms |
| `perf_core_31` drag preview | 3.5 ms | none |
| `perf_edit_29` reload a view | 55.1 ms | 200 ms |
| `perf_p2_111` C3-startup build | 469.2 ms | 1000 ms |
| `perf_p2_111` C3-startup place | 263.0 ms | 600 ms |
| `perf_p3_503` explore search | 0.16 ms | 16 ms |
| `perf_p3_503` explore page frame, 1000 elements | 1.24 ms | 16 ms |
| `startup_edit_14` first view (three runs, each alone) | 283 to 340 ms | 500 ms |

## Adding a benchmark

1. Write `perf_p5_<req>_<what>` (or the next phase's prefix) with
   `#[ignore]`, return early in a debug build, call the crate's `bench`
   helper with `[id, feature, what, input]`, a budget and the body.
2. Run the command above three times on a quiet machine; set the budget
   to about 4x the highest median.
3. Add or refresh the row here from the `BENCH` lines.
