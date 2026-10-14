//! The world event bus: the per-frame buffer swap, the mutation door's outbox
//! pump, and the event-count log.

use bevy_ecs::prelude::*;
use crate::ecs::resources::VoxelWorld;
use crate::ui::panels::UiState;

// ==========================================================================
// World event bus (design §6.7)
// ==========================================================================

/// Swap `Events<WorldEvent>`'s double buffer once per frame.
///
/// A full Bevy `App` registers this automatically via `add_event`; this engine
/// owns its own schedule, so it is explicit. Without it events are never aged
/// out and every `EventReader` re-reads the whole history.
pub fn world_event_update_system(mut events: ResMut<Events<crate::world::events::WorldEvent>>) {
    events.update();
}

/// Publish the mutation door's outbox onto the box.
///
/// The door is a method on `World`, not a system, so it cannot hold an
/// `EventWriter`. This runs unconditionally every frame, which is what makes the
/// outbox bounded.
pub fn world_event_pump_system(
    mut world: ResMut<VoxelWorld>,
    mut writer: EventWriter<crate::world::events::WorldEvent>,
) {
    for event in world.0.drain_events() {
        writer.write(event);
    }
}

/// **Bus subscriber #1**: the mutation-and-event log's event counts.
///
/// A genuine subscriber - it reads the stream and knows nothing about who
/// emitted, which is the property the bus exists to provide.
pub fn world_event_log_system(
    mut events: EventReader<crate::world::events::WorldEvent>,
    mut ui: ResMut<UiState>,
) {
    for event in events.read() {
        ui.event_counts[event.index()] += 1;
        ui.events_total += 1;
    }
}
