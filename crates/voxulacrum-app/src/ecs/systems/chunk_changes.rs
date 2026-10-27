//! The client's side of the changed-chunk feed (design §12, "Render world and
//! extract"): once per frame, drain what the mutation door changed and rebuild
//! what the client derives from those chunks.

use bevy_ecs::prelude::*;
use glam::IVec3;
use super::meshing::rebuild_water_with_neighbors;
use crate::diagnostics::{FrameTimings, SUB_FLUID};
use crate::ecs::resources::VoxelWorld;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::meshing::foliage::build_scatter;
use crate::prefabs::PrefabRegistryRes;
use crate::rendering::render_context::RenderContext;
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::water_pass::WaterPass;
use crate::world::changes::{ChunkChange, ChunkLayers};
use crate::world::mutation::MutationOrigin;

/// Drain the change feed, however many ticks ran since the last frame. Runs
/// after streaming and seam finalization and before meshing, so a chunk changed
/// this frame starts meshing this frame.
///
/// A voxel change re-meshes the chunk and the resident neighbor across each
/// face it changed; an actor's edit is debounced and meshed ahead of streaming.
/// Meshing rebuilds a chunk's foliage and water when its new mesh lands, so a
/// change that touched voxels needs nothing else here. A change that left the
/// voxels alone rebuilds what it touched directly: water (with the neighbors its
/// surface welds to) and scatter.
pub fn chunk_change_system(
    mut world: ResMut<VoxelWorld>,
    mut meshing: ResMut<MeshingCoordinator>,
    mut water_pass: ResMut<WaterPass>,
    mut scatter: ResMut<ScatterPass>,
    prefabs: Res<PrefabRegistryRes>,
    ctx: Res<RenderContext>,
    mut timings: ResMut<FrameTimings>,
) {
    let changes: Vec<ChunkChange> = world.0.drain_changes().collect();
    let mut water: Vec<IVec3> = Vec::new();
    for change in changes {
        let pos = IVec3::from(change.chunk);
        // Evicted since the change was published.
        let Some(chunk) = world.0.get_chunk(pos) else { continue };
        if change.layers.contains(ChunkLayers::VOXELS) {
            let edit = change.origin != MutationOrigin::System;
            meshing.pipeline.invalidate(pos, edit);
            for offset in change.faces.neighbor_offsets() {
                if world.0.get_chunk(pos + offset).is_some() {
                    meshing.pipeline.invalidate(pos + offset, edit);
                }
            }
            continue;
        }
        if change.layers.contains(ChunkLayers::FLUID) {
            water.push(pos);
        }
        if change.layers.contains(ChunkLayers::SCATTER) {
            scatter.set(pos, build_scatter(chunk, &prefabs).as_deref(), &ctx.device);
        }
    }
    let _timer = timings.sub_timer(SUB_FLUID);
    rebuild_water_with_neighbors(&mut water_pass, &world.0, &ctx.device, water);
}
