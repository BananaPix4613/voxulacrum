//! Engine-side `Resource` wrapper around `nodegraph_hotreload::GraphWatcher`.
//!
//! The hotreload crate is ECS-agnostic; this newtype gives the watcher a
//! bevy_ecs `Resource` identity and a panic-on-failure constructor matching
//! `ShaderWatcher::new`. The polling + reload logic lives in
//! `ecs::systems::graph_hot_reload_system`.

use std::path::Path;

use bevy_ecs::prelude::Resource;
use nodegraph_hotreload::GraphWatcher;

/// `Resource` handle to the graph-file watcher (watches the directory holding
/// the active world graph for `*.json` changes).
#[derive(Resource)]
pub struct GraphWatcherRes(pub GraphWatcher);

impl GraphWatcherRes {
    /// Watch `dir` for graph-file changes. Panics on failure, mirroring
    /// `ShaderWatcher::new`: a watcher we can't create is a fatal startup
    /// misconfiguration, not a per-frame condition.
    pub fn new(dir: &Path) -> Self {
        let watcher = GraphWatcher::new(dir).expect("Failed to create graph file watcher");
        Self(watcher)
    }
}
