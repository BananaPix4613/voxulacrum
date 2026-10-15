//! Voxulacrum, an engine for isometric voxel games.
//!
//! This crate is the engine's only supported surface for games and the
//! testbed. Depend on `voxulacrum` and on no other crate from the engine
//! repository; CI fails a consumer that does (`cargo run -p boundary-check`).
//!
//! # Module layout
//!
//! | Path | Contents |
//! |---|---|
//! | [`run`] | Engine entry point: runs the shipped reference world |
//! | [`run_with`] | Engine entry point with [`RunOptions`] |
//! | [`RunOptions`] | How the engine starts: where world content comes from and where its saves go |
//!
//! A subsystem's API arrives as a module named for that subsystem when a
//! consumer first needs it, not before.
//!
//! # Stability
//!
//! Every item exported here is a promise (roadmap §7.2). Before 1.0 a breaking
//! change is allowed, but the changelog lists it with a migration note. Items
//! exported here follow three rules:
//!
//! - No third-party types (`glam`, `wgpu`, `winit`, `bevy_ecs`) in signatures.
//! - Public structs and enums are `#[non_exhaustive]`, or have only private
//!   fields, so adding to them is not a breaking change.
//! - Identifiers are newtypes, not bare integers.
//!
//! Everything else in the engine is internal and may change without notice.

#![warn(missing_docs)]

pub use voxulacrum_app::{run, run_with, RunOptions};
