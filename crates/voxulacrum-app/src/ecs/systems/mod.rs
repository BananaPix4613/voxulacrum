//! Frame systems, one module per subsystem. `ecs::schedule` registers them
//! and sets their order.

pub mod diagnostics;
pub mod world_events;
pub mod hot_reload;
pub mod simulation;
pub mod meshing;
pub mod fluids;
pub mod chunk_changes;
pub mod climate;
pub mod player;
pub mod visibility;
pub mod editor_tools;
pub mod streaming;
pub mod jobs;
pub mod extract;
pub mod regen;
pub mod persistence;
pub mod tick;
