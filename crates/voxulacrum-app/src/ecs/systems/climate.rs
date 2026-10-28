//! The climate tick: the cloud layers and the regional weather they sample.

use bevy_ecs::prelude::*;
use glam::IVec3;
use crate::diagnostics::{FrameTimings, SUB_CLIMATE};
use crate::ecs::clocks::PresentationClock;
use crate::ecs::resources::VoxelWorld;
use crate::simulation::manager::SimulationManager;
use crate::ui::panels::UiState;
use crate::world::chunk::CHUNK_WORLD_SIZE;

/// The primary biome tag and a cheap "has notable water" signal for
/// whichever loaded chunk column covers `world_pos` (`None` if that column
/// isn't loaded yet). The water check reuses the chunk's already-resident
/// `FluidLayer` - no new persisted data, no extra generation cost, and it's
/// a one-way read (never mutates fluid state), so it doesn't reintroduce the
/// fluid/weather coupling the climate design deliberately avoids.
fn regional_weather_data_at(world: &crate::world::World, world_pos: glam::Vec2) -> Option<(u16, bool)> {
    let chunk_x = (world_pos.x / CHUNK_WORLD_SIZE).floor() as i32;
    let chunk_z = (world_pos.y / CHUNK_WORLD_SIZE).floor() as i32;
    for chunk_y in world.min_chunk_y..world.max_chunk_y {
        let coord = voxel_core::ChunkCoord::from(IVec3::new(chunk_x, chunk_y, chunk_z));
        if let Some(chunk) = world.chunks.get(&coord) {
            let biome = chunk.data.tags.biomes.first().map(|b| b.0).unwrap_or(0);
            let has_water = !chunk.data.fluids.cells.is_empty();
            return Some((biome, has_water));
        }
    }
    None
}

/// Advance the simulated cloud layers, pack them into the envelope texels
/// `extract_system` uploads, and refresh the offsets and coverages
/// `write_uniforms_system` sends to the shaders. Runs after
/// `simulation_tick_system`, so it reads this frame's camera position and wind.
pub fn climate_tick_system(
    mut sim: ResMut<SimulationManager>,
    ui: Res<UiState>,
    world: Res<VoxelWorld>,
    clock: Res<PresentationClock>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_CLIMATE);
    // The cloud automaton is client presentation, so it runs on presentation
    // time (`engine-design.md` §12, "Time").
    let dt = clock.dt.min(0.25);

    let camera_xz = glam::Vec2::new(sim.camera.smooth_target.x, sim.camera.smooth_target.z);
    let base_wind = sim.wind.wind_vector;
    let elapsed = clock.elapsed;
    let world_eval = world.0.generator.world_eval();

    sim.cloud_shadow.tick(
        &ui.params.cloud,
        camera_xz,
        base_wind,
        dt,
        elapsed,
        world_eval,
        |world_pos| {
            match world.0.generator.biome_id_at_world(world_pos.x, world_pos.y) {
                // Biome from the generator (residency-independent); water as a
                // best-effort refinement only where the chunk is resident.
                Some(biome) => {
                    let has_water = regional_weather_data_at(&world.0, world_pos)
                        .is_some_and(|(_, w)| w);
                    Some((biome, has_water))
                }
                // Single-biome world (no Zone graph): fall back to the old
                // loaded-chunk lookup wholesale.
                None => regional_weather_data_at(&world.0, world_pos),
            }
        },
        |world_pos| regional_weather_data_at(&world.0, world_pos).map(|(_, w)| w),
    );
}
