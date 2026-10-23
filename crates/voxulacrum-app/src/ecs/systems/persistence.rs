//! Periodic autosave of dirty chunks.

use bevy_ecs::prelude::*;
use crate::ecs::resources::VoxelWorld;
use crate::world::config::ServerConfig;
use crate::world::persistence::WorldPersistence;
use crate::world::tick::CurrentTick;

/// Tick phase `Autosave`: save every dirty resident chunk once per autosave
/// interval. Not on tick 0, so the first autosave comes one interval into the
/// session.
pub fn persistence_autosave_system(
    tick: Res<CurrentTick>,
    persistence: Res<WorldPersistence>,
    mut world: ResMut<VoxelWorld>,
    config: Res<ServerConfig>,
) {
    let tick = tick.0;
    if tick.0 == 0 || !tick.is_multiple_of(config.autosave.interval_ticks()) {
        return;
    }
    persistence.autosave(&mut world.0);
}
