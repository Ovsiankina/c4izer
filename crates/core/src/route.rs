//! Octilinear grid router: 0/45/90 degree lines on the 10px grid, one
//! port per line, bus lanes and joins, fewest crossings, rip-up and
//! retry, deterministic output; can re-route a subset of edges against
//! the rest (CORE-6..13, 25).
//!
//! Depends on: `view`, `geom`.
