//! Library target for the engine. The `voxulacrum-app` binary calls [`run`].

mod app;
mod camera;
mod climate;
mod cloud_shadow;
mod ecs;
mod meshing;
mod params;
// The render world is its own crate; the app's modules reach it by the module
// path they have always used.
use voxulacrum_render as rendering;
// So is the server core. The app still reaches the world types directly, by the
// module paths they have always had, until R5 narrows that to the client/server
// boundary.
use voxulacrum_server_core::{
    core_budget, foliage, jobs, libraries, materials, package, paths, play, prefabs, world,
};
mod shader_reload;
mod graph_reload;
mod simulation;
mod ui;
mod room;
mod palette;
mod input;
mod interaction;
mod player;
mod diagnostics;
mod validate;
mod verify;
mod determinism;
mod bench;

pub use app::{run, run_with, RunOptions};
