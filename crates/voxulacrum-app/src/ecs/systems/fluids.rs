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
        // One two-phase step over every active chunk; its commit goes through
        // the mutation door, which also does the persistence marking.
        let Some(out) = world.0.step_fluids() else { break };
        changed.extend(out.water_rebuild);
    }

    rebuild_water_with_neighbors(&mut water_pass, &world.0, &ctx.device, changed);
}
