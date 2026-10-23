//! Ticks the simulation manager (camera, time of day, wind, frustum) and
//! publishes this frame's `FrameState`.

use bevy_ecs::prelude::*;
use crate::ecs::clocks::{PresentationClock, TickDriver};
use crate::ecs::resources::VoxelWorld;
use crate::input::InputState;
use crate::rendering::render_targets::RenderTargets;
use crate::simulation::manager::SimulationManager;
use crate::ui::panels::UiState;
use crate::world::mutation::EngineMode;
use crate::world::tick::CurrentTick;

/// Vertical offset from the player's feet to the camera look-at point, framing
/// the player's middle (AABB is 1.8 tall).
const CAMERA_FOLLOW_OFFSET: glam::Vec3 = glam::Vec3::new(0.0, 1.0, 0.0);

pub fn simulation_tick_system(
    mut sim: ResMut<SimulationManager>,
    ui: Res<UiState>,
    render_targets: Res<RenderTargets>,
    input: Res<InputState>,
    world: Res<VoxelWorld>,
    clock: Res<PresentationClock>,
    driver: Res<TickDriver>,
    tick: Res<CurrentTick>,
    players: Query<&crate::player::systems::Player>,
    mut commands: Commands,
) {
    // Interpolated player feet position (Play mode), for both camera follow and cutaway.
    let player_pos = if world.0.mode == EngineMode::Play {
        players
            .iter()
            .next()
            .map(|p| p.prev.position.lerp(p.curr.position, driver.alpha()))
    } else {
        None
    };
    let follow = player_pos.map(|p| p + CAMERA_FOLLOW_OFFSET);

    sim.advance_world_time(driver.sim_seconds(tick.0), &ui.params);
    let frame = sim.tick(&ui.params, &render_targets, &input, follow, clock.dt, clock.elapsed);

    commands.insert_resource(frame);
}
