//! World regeneration after a graph change: the server's swap, and the client's
//! reaction to it.

use bevy_ecs::prelude::*;
use crate::ecs::resources::VoxelWorld;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::ui::panels::UiState;
use crate::world::events::WorldEvent;
use crate::world::persistence::WorldPersistence;
use crate::world::regen::WorldRegenCoordinator;
use crate::world::streaming::ChunkStreamingManager;

pub fn world_regen_system(
    mut regen: ResMut<WorldRegenCoordinator>,
    mut world: ResMut<VoxelWorld>,
    meshing: Res<MeshingCoordinator>,
    mut ui: ResMut<UiState>,
    persistence: Res<WorldPersistence>,
    mut streaming: ResMut<ChunkStreamingManager>,
    mut events: EventReader<WorldEvent>,
) {
    // **Bus subscriber #2**, independent of the log: it reads the same stream,
    // filters for what is cares about, and neither subscriber knows the other
    // exists. A graph change is queued rather than acted on directly, because an
    // edit arriving mid-regeneration must wait for the next idle tick.
    for event in events.read() {
        match event {
            WorldEvent::GraphChanged { slot } => regen.queue_slot(*slot),
            // The client's half of a swap, beside what the change feed drives:
            // the mesh cache holds meshes of the replaced terrain.
            WorldEvent::WorldRegenerated { .. } => meshing.pipeline.clear_cache(),
            _ => {}
        }
    }
    let regenerate_requested = std::mem::take(&mut ui.regenerate_requested);
    regen.tick(
        &mut world.0,
        regenerate_requested,
        &persistence,
        &mut streaming,
    );
    // Read after the tick, so a regeneration started or swapped in this frame
    // shows this frame.
    ui.regenerating = regen.manager.is_regenerating();
    ui.regen_progress = regen.manager.progress();
}
