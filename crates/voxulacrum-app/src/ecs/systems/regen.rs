//! World regeneration after a graph change.

use bevy_ecs::prelude::*;
use crate::ecs::resources::VoxelWorld;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::water_pass::WaterPass;
use crate::ui::panels::UiState;
use crate::world::persistence::WorldPersistence;
use crate::world::regen::WorldRegenCoordinator;
use crate::world::streaming::ChunkStreamingManager;

pub fn world_regen_system(
    mut regen: ResMut<WorldRegenCoordinator>,
    mut world: ResMut<VoxelWorld>,
    mut meshing: ResMut<MeshingCoordinator>,
    mut detail_paint: ResMut<DetailPaintPass>,
    mut scatter: ResMut<ScatterPass>,
    mut water_pass: ResMut<WaterPass>,
    mut ui: ResMut<UiState>,
    ctx: Res<RenderContext>,
    persistence: Res<WorldPersistence>,
    mut streaming: ResMut<ChunkStreamingManager>,
    mut events: EventReader<crate::world::events::WorldEvent>,
) {
    // **Bus subscriber #2**, independent of the log: it reads the same stream,
    // filters for what is cares about, and neither subscriber knows the other
    // exists. Queued rather than acted on directly, because an edit arriving
    // mid-regeneration must wait for the next idle tick.
    for event in events.read() {
        if let crate::world::events::WorldEvent::GraphChanged { slot } = event {
            regen.queue_slot(*slot);
        }
    }
    let regenerate_requested = std::mem::take(&mut ui.regenerate_requested);
    regen.tick(
        &mut world.0,
        &mut meshing,
        &mut detail_paint,
        &mut scatter,
        &mut water_pass,
        regenerate_requested,
        &ctx,
        &persistence,
        &mut streaming,
    );
    // Read after the tick, so a regeneration started or swapped in this frame
    // shows this frame.
    ui.regenerating = regen.manager.is_regenerating();
    ui.regen_progress = regen.manager.progress();
}
