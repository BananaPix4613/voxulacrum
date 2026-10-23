//! The frame's clocks: measuring presentation time, and running the
//! simulation ticks that time owes.

use bevy_ecs::prelude::*;
use crate::diagnostics::FrameTimings;
use crate::ecs::clocks::{PresentationClock, TickDriver};
use crate::world::tick::{run_tick, TickPhase, TickTimings};

/// Measure this frame's presentation time. Runs in `FrameStage::Input`, so
/// every later stage reads this frame's `dt`.
pub fn presentation_clock_system(mut clock: ResMut<PresentationClock>) {
    clock.advance();
}

/// Run the ticks this frame's time owes the simulation. Exclusive because each
/// tick runs the whole tick schedule on the ECS world.
pub fn run_ticks_system(ecs: &mut World) {
    let dt = ecs.resource::<PresentationClock>().dt;
    let (ticks, dropped) = {
        let mut driver = ecs.resource_mut::<TickDriver>();
        (driver.advance(dt), driver.last_dropped())
    };
    ecs.resource_mut::<FrameTimings>().add_ticks(ticks, dropped);
    for _ in 0..ticks {
        run_tick(ecs);
    }
}

// Tick timing boundaries (the tick inspector, roadmap §4.4). Like the frame's
// timing boundaries, they belong to no phase and are ordered against the
// phases, so each lands exactly between two. They read the clock to measure,
// and nothing in the simulation reads what they record.

pub fn tick_timings_begin(mut t: ResMut<TickTimings>) {
    t.begin();
}

pub fn tick_timings_end_players(mut t: ResMut<TickTimings>) {
    t.end_phase(TickPhase::Players);
}

pub fn tick_timings_end_sub_rates(mut t: ResMut<TickTimings>) {
    t.end_phase(TickPhase::SubRates);
}

pub fn tick_timings_end_autosave(mut t: ResMut<TickTimings>) {
    t.end_phase(TickPhase::Autosave);
    t.end_tick();
}
