use bevy_ecs::prelude::Resource;
use glam::IVec3;
use crate::world::world_generator::GraphSlot;

use crate::meshing;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::rendering::render_context::RenderContext;
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::water_pass::WaterPass;
use crate::ui::panels::UiState;
use crate::world::{World, WorldManager};
use crate::world::persistence::WorldPersistence;
use crate::world::streaming::ChunkStreamingManager;
use crate::world::tags::{BiomeId, ChunkTags, ZoneId};

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
        detail_paint: &mut DetailPaintPass,
        scatter: &mut ScatterPass,
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
            // Merge regenerated voxel data into the world and adopt the shared
            // generator. Targeted invalidation regenerates a subset, so we merge
            // (not replace) to retain chunks the edit didn't touch; a full regen
            // produces every loaded position and overwrites all of them.
            world.chunks.extend(new_chunks);
            world.generator = regen_generator.clone();
            
            // Reset meshing pipeline
            meshing.pipeline.reset_for_new_world();
            for chunk in world.chunks.values_mut() {
                chunk.mesh_dirty = true;
            }
            meshing.pipeline.submit_all_dirty(world);

            *detail_paint = DetailPaintPass::new(ctx, world);
            let prefabs = scatter.prefabs().clone();
            *scatter = ScatterPass::new(ctx, world, &prefabs);
            log::info!("Detail paint + scatter passes rebuilt after regeneration");

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

            // let world_key = meshing::cache::compute_world_cache_key(&regen_params);
            // let world_cache_path = meshing::cache::world_cache_path(&cache_dir, world_key);
            // if let Err(e) = meshing::cache::save_world_cache(
            //     &world_cache_path, world_key, world,
            // ) {
            //     log::warn!("Failed to save regenerated world cache: {}", e);
            // } else {
            //     log::info!("Regenerated world saved to cache");
            // }

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
        // final edit is never lost. If reassembling from the manifest fails
        // (e.g. a missing file), we log and keep the old world.
        if !self.manager.is_regenerating() {
            if let Some((slot, graph)) = ui_state.pending_graph.take() {
                // Tag-driven invalidation: classify the edit by its hierarchy
                // slot, then regenerate only the chunks whose tags match (§4).
                let target = classify_edit(slot);
                let seed = ui_state.params.terrain_gen.seed as u64;
                // Reassemble the full hierarchy from the manifest, swapping in the
                // edited graph for its slot so the rest of the hierarchy persists.
                match crate::world::world_generator::WorldGenerator::from_manifest_with_override(
                    &crate::world::world_generator::world_manifest_path(),
                    seed,
                    ui_state.params.terrain_gen.traversal_smoothing_distance,
                    slot,
                    graph,
                ) {
                    Ok(gen) => {
                        let positions = select_invalidated(world, &target);
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

/// Which loaded chunks a graph edit invalidates (design doc §4 invalidation
/// table). `ChunkTags` makes this a tag-set lookup rather than a full scan.
enum InvalidationTarget {
    /// Every loaded chunk (a WorldGraph edit, or a conservative fallback).
    AllChunks,
    /// Chunks tagged with a specific zone.
    Zone(ZoneId),
    /// Chunks tagged with a specific biome.
    Biome(BiomeId),
}

impl InvalidationTarget {
    /// Whether a chunk carrying `tags` must regenerate for this edit.
    fn matches(&self, tags: &ChunkTags) -> bool {
        match self {
            InvalidationTarget::AllChunks => true,
            InvalidationTarget::Zone(z) => tags.zone == *z,
            InvalidationTarget::Biome(b) => tags.biomes.contains(b),
        }
    }
}

/// Classify which chunks an edit to a graph of `kind` invalidates: a World
/// edit touches every chunk, a Zone edit its zone (single zone for now), a Biome
/// edit only chunks tagged with that biome id.
fn classify_edit(slot: GraphSlot) -> InvalidationTarget {
    match slot {
        GraphSlot::World => InvalidationTarget::AllChunks,
        GraphSlot::Zone => InvalidationTarget::Zone(ZoneId(0)),
        GraphSlot::Biome(id) => InvalidationTarget::Biome(BiomeId(id)),
    }
}

/// Loaded chunk positions invalidated by an edit, found by tag-set lookup.
fn select_invalidated(world: &World, target: &InvalidationTarget) -> Vec<IVec3> {
    world
        .chunks
        .iter()
        .filter(|(_, chunk)| target.matches(&chunk.data.tags))
        .map(|(&coord, _)| IVec3::from(coord))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(zone: u16, biomes: &[u16]) -> ChunkTags {
        ChunkTags {
            zone: ZoneId(zone),
            biomes: biomes.iter().map(|&b| BiomeId(b)).collect(),
            library_refs: Default::default(),
        }
    }

    #[test]
    fn all_chunks_matches_everything() {
        let t = InvalidationTarget::AllChunks;
        assert!(t.matches(&tags(0, &[0])));
        assert!(t.matches(&tags(3, &[7, 9])));
    }

    #[test]
    fn zone_target_matches_by_zone() {
        let t = InvalidationTarget::Zone(ZoneId(2));
        assert!(t.matches(&tags(2, &[0])));
        assert!(!t.matches(&tags(1, &[0])));
    }

    #[test]
    fn biome_target_matches_membership() {
        let t = InvalidationTarget::Biome(BiomeId(5));
        assert!(t.matches(&tags(0, &[1, 5])));
        assert!(!t.matches(&tags(0, &[1, 2])));
    }

    #[test]
    fn edits_classify_by_graph_kind() {
        assert!(matches!(classify_edit(GraphSlot::World), InvalidationTarget::AllChunks));
        assert!(matches!(classify_edit(GraphSlot::Zone), InvalidationTarget::Zone(_)));
        assert!(matches!(
            classify_edit(GraphSlot::Biome(1)),
            InvalidationTarget::Biome(BiomeId(1))
        ));
    }
}
