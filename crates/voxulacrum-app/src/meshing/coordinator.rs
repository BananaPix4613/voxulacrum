use bevy_ecs::prelude::Resource;
use glam::IVec3;
use crate::meshing::MeshingPipeline;
use crate::rendering::chunk_meshes::TerrainMesh;
use crate::rendering::uploads::{ChunkUpload, ChunkUploads};
use crate::ui::panels::UiState;
use crate::world::World;

#[derive(Resource)]
pub struct MeshingCoordinator {
    pub pipeline: MeshingPipeline,
}

impl MeshingCoordinator {
    pub fn new(pipeline: MeshingPipeline) -> Self {
        Self { pipeline }
    }
    
    /// Poll for completed meshes, handle UI requests.
    /// Returns the chunk positions that received new mesh data this frame.
    pub fn tick(
        &mut self,
        world: &World,
        uploads: &mut ChunkUploads,
        jobs: &mut crate::jobs::JobSystem,
        ui_state: &mut UiState,
        camera_chunk: IVec3,
        max_snapshots: usize,
    ) -> Vec<IVec3> {
        // Retry every frame: a chunk marked dirty this frame is still inside its
        // debounce window and gets skipped here, but the next frame's call (or the
        // one after) picks it up once the window clears - this is what makes an
        // edit's mesh update land within a frame or two instead of only on the next
        // incidental streaming-triggered sweep.
        self.pipeline.submit_dirty();

        // Drain pending snapshot submissions (bounded per frame)
        self.pipeline.drain_pending_submissions(world, jobs, camera_chunk, max_snapshots);
        
        // Poll completed chunks
        let completed = self.pipeline.poll();
        let mut meshed_positions = Vec::with_capacity(completed.len());
        for result in completed {
            let pos = result.chunk_key;
            // The chunk unloaded while its mesh was in flight.
            if world.get_chunk(pos).is_none() {
                continue;
            }
            self.pipeline.mark_meshed(pos, result.mesh_seq);
            uploads.terrain.insert(pos, ChunkUpload::Set {
                version: result.version,
                data: Some(TerrainMesh { vertices: result.vertices, indices: result.indices }),
            });
            meshed_positions.push(pos);
        }
        
        // Update meshing stats for UI
        ui_state.meshing_stats = self.pipeline.stats.clone();
        ui_state.remeshing = !self.pipeline.is_idle();
        
        // Handle cache clear request
        if ui_state.clear_cache_requested {
            ui_state.clear_cache_requested = false;
            self.pipeline.clear_cache();
            log::info!("Cache cleared by user");
        }
        
        // Handle explicit "Remesh" button press
        if ui_state.remesh_requested {
            ui_state.remesh_requested = false;
            ui_state.mesh_params_pending = false;
            log::info!("Remesh requested by user");
            self.pipeline.reset_for_new_world();
            for &coord in world.chunks.keys() {
                self.pipeline.invalidate(coord.into(), false);
            }
            self.pipeline.submit_dirty();
        }
        
        meshed_positions
    }
}
