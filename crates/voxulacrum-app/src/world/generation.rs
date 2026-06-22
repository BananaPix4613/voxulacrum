//! World extent constants.
//!
//! Terrain generation now lives in the nodegraph evaluator, driven by
//! [`crate::world::world_generator::WorldGenerator`]. The old param-based
//! `TerrainGenerator` was removed in Phase 1 once `biome_meadow.graph.json`
//! reproduced its output; only the world-extent constants remain here, still
//! consumed by `World::generate`.

/// Initial load radius dimensions in chunks
pub const WORLD_CHUNKS_X: usize = 8;
pub const WORLD_CHUNKS_Z: usize = 8;
