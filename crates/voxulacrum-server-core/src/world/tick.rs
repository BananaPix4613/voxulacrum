//! The simulation tick (`engine-design.md` §12, Runtime spine, "Time"): one
//! tick rate, the tick counter, and the schedule a tick runs.
//!
//! Simulation advances only in whole ticks. Code writes durations in seconds
//! or in ticks derived from [`TICK_RATE_HZ`], never as a bare tick count, so
//! the tests can run at another rate to keep that true.

use std::time::Instant;

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
        "a sub-rate must divide TICK_RATE_HZ",
    );
    (TICK_RATE_HZ / rate_hz) as u64
}

/// The whole number of ticks nearest `secs` seconds, and at least one.
pub fn secs_to_ticks(secs: f32) -> u64 {
    (secs * TICK_RATE_HZ as f32).round().max(1.0) as u64
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
/// engine doesn't run yet (commands, scheduled and block ticks, publishing)
/// join in that order when they are built.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TickPhase {
    /// Step each player with its input for this tick.
    Players,
    /// Run the systems that step at a sub-rate of the tick, on the ticks that
    /// rate falls on.
    SubRates,
    /// Load what the observers' areas want, insert finished generation
    /// results, evict what no area keeps, and finalize seams.
    Residency,
    /// Save dirty chunks once per autosave interval.
    Autosave,
}

impl TickPhase {
    /// How many phases a tick runs.
    pub const COUNT: usize = 4;

    /// Every phase, in the order a tick runs them.
    pub const ALL: [TickPhase; Self::COUNT] =
        [Self::Players, Self::SubRates, Self::Residency, Self::Autosave];

    /// This phase's position in [`Self::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// The phase's name on the tick inspector.
    pub fn name(self) -> &'static str {
        match self {
            Self::Players => "players",
            Self::SubRates => "sub-rates",
            Self::Residency => "residency",
            Self::Autosave => "autosave",
        }
    }
}

/// What each tick phase costs, for the tick inspector (roadmap §4.4).
/// Measurement only: the timing systems around the phases read the clock, and
/// nothing in the simulation reads this.
#[derive(Resource)]
pub struct TickTimings {
    /// Start of the phase being measured.
    mark: Instant,
    acc_start: Instant,
    acc_ticks: u32,
    acc_sum_ms: [f32; TickPhase::COUNT],
    acc_max_ms: [f32; TickPhase::COUNT],
    /// Each phase's mean milliseconds per tick over the last published second.
    pub mean_ms: [f32; TickPhase::COUNT],
    /// Each phase's longest single run in that second, in milliseconds.
    pub max_ms: [f32; TickPhase::COUNT],
}

impl TickTimings {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            mark: now,
            acc_start: now,
            acc_ticks: 0,
            acc_sum_ms: [0.0; TickPhase::COUNT],
            acc_max_ms: [0.0; TickPhase::COUNT],
            mean_ms: [0.0; TickPhase::COUNT],
            max_ms: [0.0; TickPhase::COUNT],
        }
    }

    /// Open a tick, before its first phase.
    pub fn begin(&mut self) {
        self.mark = Instant::now();
    }

    /// Close `phase` and open the next.
    pub fn end_phase(&mut self, phase: TickPhase) {
        let now = Instant::now();
        let ms = (now - self.mark).as_secs_f32() * 1000.0;
        self.mark = now;
        let i = phase.index();
        self.acc_sum_ms[i] += ms;
        self.acc_max_ms[i] = self.acc_max_ms[i].max(ms);
    }

    /// Close the tick, publishing once a second has passed since the last
    /// publish.
    pub fn end_tick(&mut self) {
        self.acc_ticks += 1;
        let now = Instant::now();
        if (now - self.acc_start).as_secs_f32() >= 1.0 {
            self.publish(now);
        }
    }

    fn publish(&mut self, now: Instant) {
        let n = self.acc_ticks.max(1) as f32;
        for i in 0..TickPhase::COUNT {
            self.mean_ms[i] = self.acc_sum_ms[i] / n;
            self.max_ms[i] = self.acc_max_ms[i];
        }
        self.acc_start = now;
        self.acc_ticks = 0;
        self.acc_sum_ms = [0.0; TickPhase::COUNT];
        self.acc_max_ms = [0.0; TickPhase::COUNT];
    }
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
    fn seconds_convert_to_whole_ticks() {
        assert_eq!(secs_to_ticks(30.0), 30 * u64::from(TICK_RATE_HZ));
        assert_eq!(secs_to_ticks(0.0), 1, "an interval is never zero ticks");
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

    #[test]
    fn a_phases_time_lands_in_its_own_slot() {
        let mut t = TickTimings::new();
        t.begin();
        t.end_phase(TickPhase::Players);
        std::thread::sleep(std::time::Duration::from_millis(2));
        t.end_phase(TickPhase::SubRates);
        t.end_phase(TickPhase::Autosave);
        t.end_tick();
        t.publish(Instant::now());
        let sub_rates = t.max_ms[TickPhase::SubRates.index()];
        assert!(sub_rates >= 2.0, "{sub_rates}");
        assert!(t.max_ms[TickPhase::Players.index()] < sub_rates);
        assert!(t.max_ms[TickPhase::Autosave.index()] < sub_rates);
        assert_eq!(t.mean_ms, t.max_ms, "one tick, so each phase's mean is its only run");
    }
}
