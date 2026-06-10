use std::collections::HashMap;
use bevy_ecs::prelude::Resource;
use glam::IVec3;

/// Per-chunk water mesh on GPU. Retained for Phase 3/5; unconstructed in Phase 1.
#[allow(dead_code)]
pub struct ChunkWaterMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

/// Water surface pass.
///
/// Phase 1: no-op. Terrain now comes from the node graph (`WorldGenerator`),
/// which exposes no analytic `terrain_height` for the old column-scan water
/// mesher, so that mesher is removed. The struct, the GPU water pipeline
/// (in `PipelineRegistry`), and the per-frame meshing-stage scheduling are
/// retained so water can return as a graph-driven pass in Phase 3 (water
/// table) and Phase 5 (flow), per design §7. `chunk_meshes` stays empty, so
/// the scene pass draws no water.
#[derive(Resource)]
pub struct WaterPass {
    /// Per-chunk water meshes, keyed by chunk position. Empty in Phase 1.
    pub chunk_meshes: HashMap<IVec3, ChunkWaterMesh>,
}

impl WaterPass {
    /// Create an empty water pass. See the type-level Phase 1 no-op note.
    pub fn new() -> Self {
        Self { chunk_meshes: HashMap::new() }
    }

    /// No-op in Phase 1. Retained so meshing-stage scheduling is unchanged;
    /// re-enters graph-driven water generation in Phase 3/5 (design §7).
    pub fn add_chunk_water(&mut self, _pos: IVec3) {}

    /// Remove water mesh for an unloaded chunk. GPU buffers dropped.
    pub fn remove_chunk_water(&mut self, pos: IVec3) {
        self.chunk_meshes.remove(&pos);
    }

    /// Clear all water meshes (used during regen).
    pub fn clear_all(&mut self) {
        self.chunk_meshes.clear();
    }
}
