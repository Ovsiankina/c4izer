//! Automatic placement: gives a grid position to every box of a view
//! plan that has none, never moving a placed box, scoring candidates by
//! straight-line crossings (CORE-22, 23).
//!
//! Depends on: `view`, `geom`.
