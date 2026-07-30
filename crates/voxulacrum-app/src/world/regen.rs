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
use crate::world::mutation::{EngineMode, MutationCommand, WorldMutation};

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

        // Regen is authoritative Authoring-mode work (§9). If the user is in Play
        // mode, leave the completed result pending until they return to Authoring
        // rather than swapping (and mode-rejecting) mid-Play.
        if world.mode == EngineMode::Authoring {
            if let Some((new_chunks, regen_params, regen_generator)) = self.manager.poll_regeneration() {
                // Swap the completed regeneration into the world through the mutation
                // API: merge regenerated chunks (retaining untouched ones; a full regen
                // overwrites all), adopt the shared generator, and mark every chunk
                // mesh-dirty. Authoring-mode authoritative regen (§9).
                world
                    .execute(MutationCommand::authoring(WorldMutation::SwapRegeneratedChunks {
                        chunks: new_chunks,
                        generator: regen_generator.clone(),
                    }))
                    .expect("authoring regen swap in authoring mode");

                // Reset the meshing pipeline, then resubmit every (now-dirty) chunk.
                meshing.pipeline.reset_for_new_world();
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
        if !self.manager.is_regenerating() && !ui_state.pending_graph_reload.is_empty() {
            let slots = std::mem::take(&mut ui_state.pending_graph_reload);
            let seed = ui_state.params.terrain_gen.seed as u64;
            // The whole hierarchy reloads from disk. There is no edited-graph
            // override, so a mix of in-editor and on-disk state cannot occur.
            match crate::world::world_generator::WorldGenerator::from_manifest(
                &crate::world::world_generator::world_manifest_path(),
                seed,
            ) {
                Ok(gen) => {
                    let positions = select_invalidated(world, &slots);
                    log::info!(
                        "regenerating {} chunks for {:?}",
                        positions.len(),
                        slots,
                    );
                    self.manager.start_regeneration(
                        &ui_state.params.terrain_gen,
                        std::sync::Arc::new(gen),
                        positions,
                    );
                }
                Err(e) => log::error!("graph reload: invalid graph set: {e}"),
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
    ///
    /// Unreachable today: with one Zone graph governing the whole world, a Zone
    /// edit can move any column's biome, so `classify_edit` returns `AllChunks`.
    /// This becomes correct - and is design §4's intended shape - once multiple
    /// Zone graphs exist and a zone assignment is stable across a Zone-graph
    /// edit, since then only the edited zone's chunks are affected.
    #[allow(dead_code)]
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

/// Which chunks an edit to `slot` invalidates (design §4's invalidation table).
///
/// **Tag matching is backward-looking**, and that governs this mapping. Tags
/// describe the *previous* generation, so matching on them is only sound when
/// the edit cannot change what the tags would become:
///
/// - A **Biome** (or Detail) edit changes how an already-assigned biome looks. A
///   chunk tagged with that biome still will be, so narrow matching is correct.
/// - A **Zone** edit changes which biome each column *is assigned*. The chunks
///   that need regenerating are exactly those whose assignment changes — and
///   their current tags describe the assignment being replaced. Matching
///   `tags.zone` silently skips every chunk crossing a moved band boundary,
///   which is why moving a biome threshold left the old terrain in place until a
///   full "Regenerate World".
/// - A **World** edit changes climate, hence zone assignment, hence everything.
fn classify_edit(slot: GraphSlot) -> InvalidationTarget {
    match slot {
        GraphSlot::World => InvalidationTarget::AllChunks,
        GraphSlot::Zone => InvalidationTarget::AllChunks,
        GraphSlot::Biome(id) => InvalidationTarget::Biome(BiomeId(id)),
    }
}

/// Loaded chunk positions invalidated by changes to `slots`, by tag-set lookup
/// (§4's invalidation table). A chunk regenerates if *any* changed slot claims
/// it, so several files saved at once union rather than the last one winning.
fn select_invalidated(world: &World, slots: &[GraphSlot]) -> Vec<IVec3> {
    let targets: Vec<InvalidationTarget> = slots.iter().copied().map(classify_edit).collect();
    world
        .chunks
        .iter()
        .filter(|(_, chunk)| targets.iter().any(|t| t.matches(&chunk.data.tags)))
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
        assert!(matches!(classify_edit(GraphSlot::Zone), InvalidationTarget::AllChunks));
        assert!(matches!(
            classify_edit(GraphSlot::Biome(1)),
            InvalidationTarget::Biome(BiomeId(1))
        ));
    }
}
