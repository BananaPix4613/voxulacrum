//! The fluid simulation tick.

use bevy_ecs::prelude::*;
use super::meshing::rebuild_water_with_neighbors;
use crate::diagnostics::{FrameTimings, SUB_FLUID};
use crate::ecs::resources::{FluidClock, VoxelWorld};
use crate::input::InputState;
use crate::interaction::PickState;
use crate::rendering::render_context::RenderContext;
use crate::rendering::water_pass::WaterPass;
use crate::simulation::manager::FrameState;
use crate::world::mutation::{EngineMode, MutationCommand, WorldMutation};

pub fn fluid_tick_system(
    mut clock: ResMut<FluidClock>,
    frame: Res<FrameState>,
    input: Res<InputState>,
    pick: Res<PickState>,
    mut world: ResMut<VoxelWorld>,
    mut water_pass: ResMut<WaterPass>,
    ctx: Res<RenderContext>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_FLUID);
    let mut changed: std::collections::HashSet<glam::IVec3> = std::collections::HashSet::new();

    // Debug disturbance (Authoring dev tool): press G with the cursor over terrain
    // to pour a short water column. Inert in Play mode (fluid interaction there is
    // a later substep) rather than a mode-rejected panic.
    if world.0.mode == EngineMode::Authoring
        && input.just_pressed(crate::input::GameAction::PourWater)
    {
        if let Some(anchor) = pick.anchor {
            let out = world
                .0
                .execute(MutationCommand::authoring(WorldMutation::PourFluidColumn { anchor }))
                .expect("authoring pour in authoring mode");
            changed.extend(out.water_rebuild);
        }
    }

    let active_total: usize =
        world.0.chunks.values().map(|c| c.data.fluids.active.len()).sum();
    if active_total > 100_000 {
        log::warn!("fluid: {active_total} active cells - possible mass-conservation runaway");
    }
    let mut ticks = clock.ticks_for(frame.dt);
    // Throttle: during a very large disturbance, run fewer ticks per frame so a
    // big flow slows down instead of stalling the framerate. Each tick is still a
    // full, deterministic global two-phase; we just do fewer per frame.
    if active_total > 20_000 {
        ticks = ticks.min(1);
    }
    for _ in 0..ticks {
        // Phase 1: plan every active chunk from the shared pre-tick snapshot,
        // reading neighbor chunks read-only (nothing mutates yet).
        let active: Vec<voxel_core::ChunkCoord> = world
            .0
            .chunks
            .iter()
            .filter(|(_, c)| !c.data.fluids.active.is_empty())
            .map(|(coord, _)| *coord)
            .collect();
        if active.is_empty() {
            break;
        }
        let mut plans: Vec<(voxel_core::ChunkCoord, crate::world::fluid_sim::ChunkPlan)> =
            Vec::new();
        {
            let w = &world.0;
            for &coord in &active {
                let Some(this) = w.chunks.get(&coord) else { continue };
                let faces = fluid_neighbor_faces(w, coord);
                let sample = crate::world::fluid_sim::NeighborSample::new(
                    &this.data.fluids,
                    this.data.voxels.as_ref(),
                    faces,
                );
                let plan = crate::world::fluid_sim::plan_chunk(&this.data.fluids, &sample);
                plans.push((coord, plan));
            }
        }
        // Phase 2 and 3 (commit + wake) are the tick's write half and go
        // through the mutation door, which also does the persistence marking
        // that used to be a separate `MarkFluidDirty` call. The sim runs in any
        // mode, so this is a System mutation, not an actor edit.
        let out = world
            .0
            .execute(MutationCommand::system(WorldMutation::CommitFluidPlans { plans }))
            .expect("system fluid commit accepted in any mode");
        changed.extend(out.water_rebuild);
    }

    rebuild_water_with_neighbors(&mut water_pass, &world.0, &ctx.device, changed);
}

/// Read-only `(fluids, storage)` of the six face-neighbors of `coord`, in the
/// order `NeighborSample` expects: `[-x, +x, -y, +y, -z, +z]`.
fn fluid_neighbor_faces<'a>(
    world: &'a crate::world::World,
    coord: voxel_core::ChunkCoord,
) -> [Option<(&'a crate::world::layers::FluidLayer, &'a crate::world::storage::ChunkStorage)>; 6] {
    let offs = [(-1i32, 0, 0), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1)];
    let mut faces: [Option<(
        &crate::world::layers::FluidLayer,
        &crate::world::storage::ChunkStorage,
    )>; 6] = [None; 6];
    for (i, &(dx, dy, dz)) in offs.iter().enumerate() {
        let nc = voxel_core::ChunkCoord::new(coord.x + dx, coord.y + dy, coord.z + dz);
        if let Some(chunk) = world.chunks.get(&nc) {
            faces[i] = Some((&chunk.data.fluids, chunk.data.voxels.as_ref()));
        }
    }
    faces
}
