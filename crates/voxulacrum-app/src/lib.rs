//! Library target for the engine. The `voxulacrum-app` binary calls [`run`].

mod app;
mod camera;
mod cloud_shadow;
mod ecs;
mod meshing;
mod params;
mod rendering;
mod shader_reload;
mod graph_reload;
mod simulation;
mod ui;
mod world;
mod palette;
mod input;
mod interaction;
mod paths;
mod materials;
mod prefabs;
mod libraries;
mod core_budget;
mod player;
mod diagnostics;
mod foliage;
mod jobs;
mod package;
mod validate;
mod verify;

pub use app::{run, run_with, RunOptions};
