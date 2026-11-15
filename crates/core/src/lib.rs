//! c4rizer core: parse a diagram file, place, route, label, build a scene
//! per view, lint it and export it. No UI, no threads.
//!
//! Depends on: its modules only.

pub mod geom;
pub mod text;
pub mod model;
pub mod edit;
pub mod view;
mod place;
mod route;
mod label;
mod scene;
mod lint;
mod export;
mod pipeline;
