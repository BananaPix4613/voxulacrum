//! The fluid simulation: its tick phase and the editor's pour. The water meshes
//! they change are rebuilt from the change feed (`chunk_changes`).

use bevy_ecs::prelude::*;
use crate::diagnostics::{FrameTimings, SUB_FLUID};
use crate::ecs::resources::VoxelWorld;
use crate::input::InputState;
use crate::interaction::PickState;
use crate::world::config::ServerConfig;
use crate::world::mutation::{EngineMode, MutationCommand, WorldMutation};
use crate::world::tick::{ticks_per_step, CurrentTick};

/// Fluid steps per second. Each step's flow is fixed, so this sets how fast
/// water moves.
const FLUID_RATE_HZ: u32 = 10;

/// Ticks between fluid steps.
const FLUID_TICK_INTERVAL: u64 = ticks_per_step(FLUID_RATE_HZ);

/// Tick phase `SubRates`: one fluid step on each fluid tick. Its commit
/// publishes the chunks whose water changed to the change feed.
pub fn fluid_tick_system(
    tick: Res<CurrentTick>,
    mut world: ResMut<VoxelWorld>,
    config: Res<ServerConfig>,
) {
    if !tick.0.is_multiple_of(FLUID_TICK_INTERVAL) {
        return;
    }
    let limits = &config.fluids;
    let active_total: usize =
        world.0.chunks.values().map(|c| c.data.fluids.active.len()).sum();
    if active_total > limits.warn_active_cells {
        log::warn!("fluid: {active_total} active cells - possible mass-conservation runaway");
    }
    // Throttle: a very large disturbance steps on every other fluid tick, so a
    // big flow slows down instead of stalling the frame. Decided from the
    // active-cell count at this tick, never from frame timing, so the result is
    // the same however the frames fall.
    if active_total > limits.throttle_active_cells
        && !tick.0.is_multiple_of(2 * FLUID_TICK_INTERVAL)
    {
        return;
    }
    // One two-phase step over every active chunk; its commit goes through the
    // mutation door, which also does the persistence marking.
    world.0.step_fluids();
}

/// Debug disturbance (Authoring dev tool): press G with the cursor over terrain
/// to pour a short water column. Inert in Play mode (fluid interaction there is
/// a later substep) rather than a mode-rejected panic.
///
/// The pour reaches the door from the frame, between ticks, as every edit does
/// until commands are queued for the next tick (R5).
pub fn fluid_pour_system(
    input: Res<InputState>,
    pick: Res<PickState>,
    mut world: ResMut<VoxelWorld>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_FLUID);
    if world.0.mode != EngineMode::Authoring
        || !input.just_pressed(crate::input::GameAction::PourWater)
    {
        return;
    }
    if let Some(anchor) = pick.anchor {
        world
            .0
            .execute(MutationCommand::authoring(WorldMutation::PourFluidColumn { anchor }))
            .expect("authoring pour in authoring mode");
    }
}
