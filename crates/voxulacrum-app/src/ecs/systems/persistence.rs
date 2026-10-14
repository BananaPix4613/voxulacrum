//! Periodic autosave of dirty chunks.

use bevy_ecs::prelude::*;
use crate::diagnostics::{FrameTimings, SUB_SAVE};
use crate::ecs::resources::VoxelWorld;
use crate::simulation::manager::FrameState;
use crate::world::persistence::WorldPersistence;

pub fn persistence_autosave_system(
    mut persistence: ResMut<WorldPersistence>,
    mut world: ResMut<VoxelWorld>,
    frame: Res<FrameState>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_SAVE);
    persistence.tick_autosave(frame.dt, &mut world.0);
}
