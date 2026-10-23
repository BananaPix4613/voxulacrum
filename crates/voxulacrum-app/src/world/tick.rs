//! The simulation tick (`engine-design.md` §12, Runtime spine, "Time"): one
//! tick rate, the tick counter, and the schedule a tick runs.
//!
//! Simulation advances only in whole ticks. Code writes durations in seconds
//! or in ticks derived from [`TICK_RATE_HZ`], never as a bare tick count, so
//! the tests can run at another rate to keep that true.

use bevy_ecs::prelude::Resource;
use bevy_ecs::schedule::{ScheduleLabel, SystemSet};

/// Simulation ticks per second. 30 unless the build sets
/// `VOXULACRUM_TICK_RATE_HZ`, which exists so the tests can run at another rate
/// and show that nothing depends on 30.
pub const TICK_RATE_HZ: u32 = match option_env!("VOXULACRUM_TICK_RATE_HZ") {
    Some(rate) => parse_rate(rate),
    None => 30,
};

/// The simulated time one tick covers, in seconds.
pub const TICK_DT: f32 = 1.0 / TICK_RATE_HZ as f32;

/// Ticks between steps of a system that runs at `rate_hz`, a sub-rate of the
/// tick. The rate must divide [`TICK_RATE_HZ`], so every step falls on a tick
/// and the sub-rate never drifts. Declared as a const, a rate that doesn't
/// divide it fails the build.
pub const fn ticks_per_step(rate_hz: u32) -> u64 {
    assert!(
        rate_hz > 0 && TICK_RATE_HZ % rate_hz == 0,
        " a sub-rate must divide TICK_RATE_HZ",
    );
    (TICK_RATE_HZ / rate_hz) as u64
}

/// Parse the build's tick-rate override. A malformed value fails the build
/// rather than falling back, so a typo can't quietly run the tests at 30.
const fn parse_rate(rate: &str) -> u32 {
    let bytes = rate.as_bytes();
    assert!(!bytes.is_empty(), "VOXULACRUM_TICK_RATE_HZ is set but empty");
    let mut hz = 0u32;
    let mut i = 0;
    while i < bytes.len() {
        assert!(
            bytes[i].is_ascii_digit(),
            "VOXULACRUM_TICK_RATE_HZ must be a whole number of hertz",
        );
        hz = hz * 10 + (bytes[i] - b'0') as u32;
        i += 1;
    }
    assert!(hz > 0, "VOXULACRUM_TICK_RATE_HZ must be above zero");
    hz
}

/// A tick's number. Tick 0 is the first one a session runs. Later it
/// timestamps network messages, replays and scheduled updates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimTick(pub u64);

impl SimTick {
    /// Whether a system stepping every `ticks` ticks runs on this tick.
    pub fn is_multiple_of(self, ticks: u64) -> bool {
        self.0.is_multiple_of(ticks)
    }
}

/// The tick the simulation is on: inside a tick's phases, the tick running;
/// between ticks, the next one to run, which is also the count already run.
///
/// Not saved, so every session starts again at tick 0. Scheduled ticks, whose
/// due ticks persist with their chunk, need it saved.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct CurrentTick(pub SimTick);

/// The schedule one tick runs, phase by phase. Built by
/// `ecs::schedule::build_tick_schedule` and run by [`run_tick`].
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TickSchedule;

/// A tick's phases, in the order they run (design §12, "Time"). Phases the
/// engine doesn't run yet (commands, scheduled and block ticks, residency,
/// publishing) join in that order when they are built.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TickPhase {
    /// Step each player with its input for this tick.
    Players,
    /// Run the systems that step at a sub-rate of the tick, on the ticks that
    /// rate falls on.
    SubRates,
}

/// Run one tick: every phase of [`TickSchedule`], then advance [`CurrentTick`].
/// The integrated client runs it from its frame; the headless server (R3c)
/// will run it on its own clock.
pub fn run_tick(ecs: &mut bevy_ecs::world::World) {
    ecs.run_schedule(TickSchedule);
    let mut current = ecs.resource_mut::<CurrentTick>();
    current.0 = SimTick(current.0.0 + 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tick_lasts_one_over_the_rate() {
        assert!((TICK_DT * TICK_RATE_HZ as f32 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_rate_override_parses_whole_hertz() {
        assert_eq!(parse_rate("20"), 20);
        assert_eq!(parse_rate("30"), 30);
    }

    #[test]
    fn a_sub_rate_steps_on_evenly_spaced_ticks() {
        assert_eq!(ticks_per_step(TICK_RATE_HZ), 1);
        let every = ticks_per_step(10);
        assert_eq!(every * 10, u64::from(TICK_RATE_HZ));
        let steps = (0..u64::from(TICK_RATE_HZ))
            .filter(|&t| SimTick(t).is_multiple_of(every))
            .count();
        assert_eq!(steps, 10, "ten steps in one second of ticks");
    }

    #[test]
    fn running_a_tick_advances_the_tick_number() {
        let mut ecs = bevy_ecs::world::World::new();
        ecs.add_schedule(bevy_ecs::schedule::Schedule::new(TickSchedule));
        ecs.insert_resource(CurrentTick::default());
        run_tick(&mut ecs);
        run_tick(&mut ecs);
        assert_eq!(ecs.resource::<CurrentTick>().0, SimTick(2));
    }
}
