//! # nodegraph-hotreload
//!
//! Lightweight file-watcher for `nodegraph-ir` JSON graphs.
//!
//! [`GraphWatcher`] mirrors the engine's `ShaderWatcher` pattern: it owns a
//! `notify` recommended watcher, drains events into an `mpsc` channel, and
//! exposes a pull-based [`GraphWatcher::poll_changes`] that returns
//! deduplicated `.json` paths since the last poll.
//!
//! The crate is engine-agnostic - no ECS, no UI deps. Wrap with a `Resource`
//! in the app crate if you want bevy_ecs integration.
//!
//! ## Coarse diffing (Phase 4)
//!
//! Any change to a watched file invalidates the whole graph for that file.
//! Per-node / per-chunk incremental diffing is Phase 9.

#![warn(missing_docs)]

mod blueprint;
mod species;
mod error;
mod example;
mod watcher;

pub use error::{HotReloadError, HotReloadResult};
pub use example::{bootstrap_example_graph, build_example_graph};
pub use blueprint::{load_blueprint, resolve_blueprints};
pub use species::{load_species, resolve_species, SpeciesFile, SPECIES_VERSION};
pub use watcher::GraphWatcher;
