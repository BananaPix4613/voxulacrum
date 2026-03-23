use std::path::PathBuf;
use bevy_ecs::prelude::Resource;

use crate::meshing;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::rendering::render_context::RenderContext;
use crate::rendering::vegetation_pass::VegetationPass;
use crate::rendering::water_pass::WaterPass;
use crate::simulation::water::StaticWater;
use crate::ui::panels::UiState;
use crate::world::{World, WorldManager};

#[derive(Resource)]
pub struct WorldRegenCoordinator {
    pub manager: WorldManager,
}

impl WorldRegenCoordinator {
    pub fn new() -> Self {
        Self {
            manager: WorldManager::new(),
        }
    }
    
    /// Poll for regen completion and handle regen requests.
    pub fn tick(
        &mut self,
        world: &mut World,
        meshing: &mut MeshingCoordinator,
        vegetation_pass: &mut VegetationPass,
        water_pass: &mut WaterPass,
        static_water: &mut StaticWater,
        ui_state: &mut UiState,
        ctx: &RenderContext,
    ) {
        // Feed regeneration status to UI
        ui_state.regenerating = self.manager.is_regenerating();
        ui_state.regen_progress = self.manager.progress();
        
        // Poll for completed background regeneration
        if let Some((new_chunks, regen_params)) = self.manager.poll_regeneration() {
            // Swap new voxel data into the world
            world.chunks = new_chunks;
            world.generator =
                crate::world::generation::TerrainGenerator::new(&regen_params);
            
            // Reset meshing pipeline
            meshing.pipeline.reset_for_new_world();
            for chunk in &mut world.chunks {
                chunk.mesh_dirty = true;
            }
            meshing.pipeline.submit_all_dirty(world);
            
            // Rebuild vegetation pass
            *vegetation_pass = VegetationPass::new(
                ctx,
                world,
                &ui_state.params.vegetation,
            );
            log::info!("Vegetation pass rebuilt after regeneration");

            // Rebuild water passes
            *static_water = StaticWater::new(
                world,
                &ui_state.params.water,
                &regen_params,
            );
            *water_pass = WaterPass::new(ctx, static_water);
            log::info!("Water passes rebuilt after regeneration");
            
            // Clear stale caches and save new world
            let cache_dir = PathBuf::from("cache/meshes");
            meshing.pipeline.clear_cache();
            let _ = meshing::cache::clear_world_cache(&cache_dir);
            
            let world_key = meshing::cache::compute_world_cache_key(&regen_params);
            let world_cache_path = meshing::cache::world_cache_path(&cache_dir, world_key);
            if let Err(e) = meshing::cache::save_world_cache(
                &world_cache_path,
                world_key,
                world,
            ) {
                log::warn!("Failed to save regenerated world cache: {}", e);
            } else {
                log::info!("Regenerated world saved to cache");
            }
            
            ui_state.regenerating = false;
            
            // If terrain params changed during regen, restart
            if regen_params != ui_state.params.terrain_gen {
                log::info!("Terrain params changed during regeneration, restarting");
                self.manager.start_regeneration(&ui_state.params.terrain_gen);
            }
        }
        
        // Handle explicit "Regenerate World" button press
        if ui_state.regenerate_requested {
            ui_state.regenerate_requested = false;
            if !self.manager.is_regenerating() {
                self.manager.start_regeneration(&ui_state.params.terrain_gen);
            }
        }
    }
}