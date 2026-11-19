//! c4rizer core: parse a diagram file, place, route, label, build a scene
//! per view, lint it and export it. No UI, no threads.
//!
//! Depends on: its modules only.

pub mod geom;
pub mod anchor;
pub mod text;
pub mod model;
pub mod edit;
pub mod view;
pub mod place;
pub mod route;
pub mod label;
pub mod scene;
pub mod lint;
pub mod inspect;
pub mod export;
pub mod pipeline;
pub mod import;
pub mod explore;
