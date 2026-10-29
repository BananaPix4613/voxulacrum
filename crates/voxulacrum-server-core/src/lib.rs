//! The engine's server core: world and chunk state, generation glue, fluids,
//! persistence, the mutation door and its events, residency, the tick
//! framework, the player step, the job system, and package and registry
//! loading (`engine-design.md` §12, Runtime spine, "Crates and sides").
//!
//! It depends on no renderer, windowing or UI crate (`wgpu`, `naga`, `winit`,
//! `raw-window-handle`, `egui`, `eframe`), directly or transitively.
//!
//! Everything here is public for now, because the app still reaches the world
//! types directly. R5 narrows that to the client/server boundary.

pub mod core_budget;
pub mod foliage;
pub mod jobs;
pub mod libraries;
pub mod materials;
pub mod package;
pub mod paths;
pub mod play;
pub mod player;
pub mod prefabs;
pub mod world;
