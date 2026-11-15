//! Resolves one view spec into a view plan: the boxes (id, size,
//! position if any) and edges left after include, exclude, lifting,
//! de-duplication, focus and hide (CORE-5).
//!
//! Depends on: `model`, `geom`.
