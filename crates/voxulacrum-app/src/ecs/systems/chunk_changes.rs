//! The client's side of the changed-chunk feed (design §12, "Render world and
//! extract"): once per frame, drain what the mutation door changed and rebuild
//! what the client derives from those chunks.

use bevy_ecs::prelude::*;
use glam::IVec3;
use super::meshing::rebuild_water_with_neighbors;
use crate::diagnostics::{FrameTimings, SUB_FLUID};
use crate::ecs::resources::VoxelWorld;
use crate::rendering::render_context::RenderContext;
use crate::rendering::water_pass::WaterPass;
use crate::world::changes::ChunkLayers;

/// Drain the change feed, however many ticks ran since the last frame, and
/// rebuild the water meshes of chunks whose fluid alone changed. A change that
/// also touched voxels is left to meshing, which rebuilds a chunk's water when
/// its new mesh lands, so water and terrain appear on the same frame.
///
/// Meshing and the foliage buffers still follow the door's rebuild lists; the
/// feed takes them over next (R4).
pub fn chunk_change_system(
    mut world: ResMut<VoxelWorld>,
    mut water_pass: ResMut<WaterPass>,
    ctx: Res<RenderContext>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_FLUID);
    let fluid_only: Vec<IVec3> = world
        .0
        .drain_changes()
        .filter(|c| c.layers.contains(ChunkLayers::FLUID) && !c.layers.contains(ChunkLayers::VOXELS))
        .map(|c| IVec3::from(c.chunk))
        .collect();
    rebuild_water_with_neighbors(&mut water_pass, &world.0, &ctx.device, fluid_only);
}
