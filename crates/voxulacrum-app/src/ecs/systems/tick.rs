//! The frame's clocks: measuring presentation time, and running the
//! simulation ticks that time owes.

use bevy_ecs::prelude::*;
use crate::ecs::clocks::{PresentationClock, TickDriver};
use crate::world::tick::run_tick;

/// Measure this frame's presentation time. Runs in `FrameStage::Input`, so
/// every later stage reads this frame's `dt`.
pub fn presentation_clock_system(mut clock: ResMut<PresentationClock>) {
    clock.advance();
}

/// Run the ticks this frame's time owes the simulation. Exclusive because each
/// tick runs the whole tick schedule on the ECS world.
pub fn run_ticks_system(ecs: &mut World) {
    let dt = ecs.resource::<PresentationClock>().dt;
    let ticks = ecs.resource_mut::<TickDriver>().advance(dt);
    for _ in 0..ticks {
        run_tick(ecs);
    }
}
