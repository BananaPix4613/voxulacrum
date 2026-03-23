use bevy_ecs::prelude::Resource;

use crate::meshing::MeshingPipeline;
use crate::rendering::render_context::RenderContext;
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
    pub fn tick(
        &mut self,
        world: &mut World,
        ctx: &RenderContext,
        ui_state: &mut UiState,
    ) {
        // Drain pending snapshot submissions (bounded per frame)
        self.pipeline.drain_pending_submissions(world);
        
        // Poll completed chunks
        let completed = self.pipeline.poll();
        for result in completed {
            world.upload_mesh_result(
                result.chunk_index,
                &result.vertices,
                &result.indices,
                &ctx.device,
            );
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
        
        // Clear boundary maps when pipeline finishes
        if self.pipeline.is_idle() {
            self.pipeline.clear_boundary_maps();
        }
        
        // Handle explicit "Remesh" button press
        if ui_state.remesh_requested {
            ui_state.remesh_requested = false;
            ui_state.mesh_params_pending = false;
            log::info!("Remesh requested by user");
            self.pipeline.update_material_config(
                &ui_state.params.materials,
                &ui_state.params.meshing,
            );
            self.pipeline.reset_for_new_world();
            for chunk in &mut world.chunks {
                chunk.mesh_dirty = true;
            }
            self.pipeline.submit_all_dirty(world);
        }
    }
}