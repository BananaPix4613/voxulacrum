use bevy_ecs::prelude::Resource;
use glam::IVec3;

use crate::meshing;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::rendering::render_context::RenderContext;
use crate::rendering::vegetation_pass::VegetationPass;
use crate::rendering::water_pass::WaterPass;
use crate::ui::panels::UiState;
use crate::world::{World, WorldManager};
use crate::world::persistence::WorldPersistence;
use crate::world::streaming::ChunkStreamingManager;

#[derive(Resource)]
pub struct WorldRegenCoordinator {
    pub manager: WorldManager,
}

impl WorldRegenCoordinator {
    pub fn new(pool: std::sync::Arc<rayon::ThreadPool>) -> Self {
        Self {
            manager: WorldManager::new(pool),
        }
    }
    
    /// Poll for regen completion and handle regen requests.
    pub fn tick(
        &mut self,
        world: &mut World,
        meshing: &mut MeshingCoordinator,
        vegetation_pass: &mut VegetationPass,
        water_pass: &mut WaterPass,
        ui_state: &mut UiState,
        ctx: &RenderContext,
        persistence: &WorldPersistence,
        streaming: &mut ChunkStreamingManager,
    ) {
        // Feed regeneration status to UI
        ui_state.regenerating = self.manager.is_regenerating();
        ui_state.regen_progress = self.manager.progress();
        
        // Poll for completed background regeneration
        if let Some((new_chunks, regen_params, regen_generator)) = self.manager.poll_regeneration() {
            // Swap new voxel data into the world and adopt the shared generator.
            world.chunks = new_chunks;
            world.generator = regen_generator.clone();
            
            // Reset meshing pipeline
            meshing.pipeline.reset_for_new_world();
            for chunk in world.chunks.values_mut() {
                chunk.mesh_dirty = true;
            }
            meshing.pipeline.submit_all_dirty(world);

            let material_registry = vegetation_pass.registry.clone();
            *vegetation_pass = VegetationPass::new(ctx, world, &ui_state.params.vegetation, material_registry);
            log::info!("Vegetation pass rebuilt after regeneration");

            // Water is a Phase 1 no-op; just clear any retained meshes.
            water_pass.clear_all();

            // Clear saved chunk edits (stale under new terrain params)
            match persistence.clear_all_chunks() {
                Ok(n) if n > 0 => log::info!("Cleared {n} saved chunk edits for new terrain params"),
                Err(e) => log::warn!("Failed to clear saved chunks: {e}"),
                _ => {}
            }
            
            // Clear stale caches and save new world
            let cache_dir = crate::paths::asset_root().join("cache").join("meshes");
            meshing.pipeline.clear_cache();
            let _ = meshing::cache::clear_world_cache(&cache_dir);

            let world_key = meshing::cache::compute_world_cache_key(&regen_params);
            let world_cache_path = meshing::cache::world_cache_path(&cache_dir, world_key);
            if let Err(e) = meshing::cache::save_world_cache(
                &world_cache_path, world_key, world,
            ) {
                log::warn!("Failed to save regenerated world cache: {}", e);
            } else {
                log::info!("Regenerated world saved to cache");
            }

            // Rebuild streaming workers so new chunks use the new generator
            streaming.rebuild_for_new_params(
                regen_generator.clone(),
                persistence.db_path.clone(),
                persistence.dictionary_bytes().map(|b| std::sync::Arc::new(b)),
            );
            log::info!("Streaming workers rebuilt for new generator");

            ui_state.regenerating = false;
            
            // If terrain params changed during regen, restart
            if regen_params != ui_state.params.terrain_gen {
                log::info!("Terrain params changed during regeneration, restarting");
                match crate::world::world_generator::load_default(&ui_state.params.terrain_gen) {
                    Ok(gen) => {
                        let positions: Vec<IVec3> = world.chunks.keys().map(|c| IVec3::from(*c)).collect();
                        self.manager.start_regeneration(&ui_state.params.terrain_gen, gen, positions);
                    }
                    Err(e) => log::error!("Regen restart: failed to load generator: {e}"),
                }
            }
        }
        
        // Handle explicit "Regenerate World" button press
        if ui_state.regenerate_requested {
            ui_state.regenerate_requested = false;
            if !self.manager.is_regenerating() {
                match crate::world::world_generator::load_default(&ui_state.params.terrain_gen) {
                    Ok(gen) => {
                        let positions: Vec<IVec3> = world.chunks.keys().map(|c| IVec3::from(*c)).collect();
                        self.manager.start_regeneration(&ui_state.params.terrain_gen, gen, positions);
                    }
                    Err(e) => log::error!("Regenerate: failed to load generator: {e}"),
                }
            }
        }
        
        // --- Live graph edits from the embedded editor (auto-regen) ---
        // The render system stashes the editor's graph here on every dirty
        // edit. Rebuild the generator from that graph and regenerate the loaded
        // world. Only drain while idle: edits arriving mid-regen stay queued
        // (latest wins) and fire on the next tick after completion, so the
        // final edit is never lost. WorldGenerator::new validates that the
        // graph still has a TerrainOutput; if not, we log and keep the old
        // world rather than wiping it.
        if !self.manager.is_regenerating() {
            if let Some(graph) = ui_state.pending_graph.take() {
                let seed = ui_state.params.terrain_gen.seed as u64;
                match crate::world::world_generator::WorldGenerator::new(graph, seed) {
                    Ok(gen) => {
                        let positions: Vec<IVec3> = world.chunks.keys().map(|c| IVec3::from(*c)).collect();
                        self.manager.start_regeneration(
                            &ui_state.params.terrain_gen,
                            std::sync::Arc::new(gen),
                            positions,
                        );
                    }
                    Err(e) => log::error!("Graph-edit regen: invalid graph: {e}"),
                }
            }
        }
    }
}
