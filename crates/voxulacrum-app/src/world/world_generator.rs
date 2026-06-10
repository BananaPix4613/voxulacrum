//! Graph-backed world generation.
//!
//! `WorldGenerator` is the single source of voxel terrain: it holds an
//! `Arc<Graph>` and produces a `ChunkStorage` per chunk by running the
//! nodegraph evaluator and harvesting the `TerrainOutput` node, mirroring
//! `voxulacrum-preview`'s runtime extraction.
//!
//! PHASE 3 SHIM: the evaluator yields a `ChunkBuffer<Voxel, 32>`, which is
//! index-copied into the engine's `ChunkStorage`. The two containers are
//! semantically equivalent (see the Phase-0 parity test in `storage.rs`).
//! When the engine adopts `ChunkBuffer` directly (Phase 3), this copy is
//! deleted.

use std::sync::Arc;

use glam::IVec3;
use nodegraph_eval::{CachedOutput, EvalContext, Evaluator};
use nodegraph_ir::{Graph, NodeId, NodeKind, Severity};
use voxel_core::Voxel;

use crate::params::TerrainGenParams;
use super::chunk::{CHUNK_SIZE, CHUNK_VOLUME};
use super::storage::{self, ChunkStorage};

/// The single world generator. Evaluates one fixed graph per chunk.
pub struct WorldGenerator {
    graph: Arc<Graph>,
    world_seed: u64,
    /// Precomputed id of the graph's `TerrainOutput` node, so we don't
    /// re-scan the node set on every chunk.
    terrain_node: NodeId,
}

impl WorldGenerator {
    /// Build a generator from an owned graph. Fails if the graph has no
    /// `TerrainOutput` terminal (a config error worth catching at startup
    /// rather than per-chunk).
    pub fn new(graph: Graph, world_seed: u64) -> Result<Self, String> {
        let terrain_node = graph
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::TerrainOutput(_)))
            .map(|(id, _)| id)
            .ok_or_else(|| "graph has no TerrainOutput node".to_string())?;
        
        // Surface graph validation findings at load (the same checks the editor
        // shows). A graph can still carry a TerrainOutput while having missing
        // inputs or type mismatches, so log them rather than relying solely on
        // the terminal-node check above.
        for d in graph.validate() {
            match d.severity {
                Severity::Error => log::error!("graph validation: {}", d.message),
                Severity::Warning => log::warn!("graph validation: {}", d.message),
                Severity::Info => log::info!("graph validation: {}", d.message),
            }
        }
        
        Ok(Self {
            graph: Arc::new(graph),
            world_seed,
            terrain_node,
        })
    }
    
    /// Load a generator from a `*.graph.json` file on disk.
    pub fn from_path(path: &std::path::Path, world_seed: u64) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let graph = Graph::from_json(&text).map_err(|e| e.to_string())?;
        Self::new(graph, world_seed)
    }
    
    /// The graph this generator evaluates (shared, cheap to clone).
    pub fn graph(&self) -> &Arc<Graph> {
        &self.graph
    }
    
    /// Generate the voxel storage for one chunk by evaluating the graph.
    /// On any evaluation failure this logs and returns an air chunk, so
    /// callers (streaming workers, initial fill) stay infallible.
    pub fn generate_chunk_storage(&self, position: IVec3) -> ChunkStorage {
        let mut eval = Evaluator::new(&*self.graph, EvalContext::new(self.world_seed, position));
        if let Err(e) = eval.evaluate() {
            log::warn!("graph eval failed for chunk {:?}: {}", position, e);
            return ChunkStorage::new_air();
        }
        let terrain = match eval.cache().get(self.terrain_node) {
            Some(CachedOutput::Terrain(f)) => f,
            _ => {
                log::warn!("graph produced no terrain for chunk {:?}", position);
                return ChunkStorage::new_air();
            }
        };
        
        // PHASE 3 SHIM: index-copy ChunkBuffer<Voxel,32> -> flat engine array.
        // Decode matches engine voxel_index(x,y,z) = x + y*32 + z*32^2, which
        // the Phase-0 parity test proves equivalent to ChunkBuffer's order.
        let mut voxel_arr = [Voxel::EMPTY; CHUNK_VOLUME];
        for i in 0..CHUNK_VOLUME {
            let x = i % CHUNK_SIZE;
            let y = (i / CHUNK_SIZE) % CHUNK_SIZE;
            let z = i / (CHUNK_SIZE * CHUNK_SIZE);
            voxel_arr[i] = terrain.get(x, y, z);
        }
        storage::storage_from_arrays(&voxel_arr)
    }
}

/// Path of the default biome graph the engine generates from in Phase 1.
pub fn default_graph_path() -> std::path::PathBuf {
    crate::paths::asset_root()
        .join("assets")
        .join("graphs")
        .join("default_biome.graph.json")
}

/// Load the default biome graph into a shared generator, seeded from `params`.
///
/// This is the one construction site for the engine's generator; `World`,
/// the streaming workers, and background regeneration all share the resulting
/// `Arc`. Re-reading the file here (rather than caching one immutable graph)
/// means a regeneration picks up an edited `default_biome.graph.json`.
pub fn load_default(params: &TerrainGenParams) -> Result<Arc<WorldGenerator>, String> {
    WorldGenerator::from_path(&default_graph_path(), params.seed as u64).map(Arc::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn default_biome_graph_loads_with_terrain_output() {
        let generator = WorldGenerator::from_path(&default_graph_path(), 0)
            .expect("default_biome.graph.json must load into a WorldGenerator");
        let node = &generator.graph().nodes[generator.terrain_node];
        assert!(
            matches!(node.kind, NodeKind::TerrainOutput(_)),
            "resolved terrain_node is not a TerrainOutput",
        );
    }
    
    #[test]
    fn default_biome_graph_has_no_validation_errors() {
        let text = std::fs::read_to_string(default_graph_path())
            .expect("read default_biome.graph.json");
        let graph = Graph::from_json(&text).expect("parse default_biome.graph.json");
        let errors = graph
            .validate()
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        assert_eq!(errors, 0, "default graph has {errors} validation error(s)");
    }
}
