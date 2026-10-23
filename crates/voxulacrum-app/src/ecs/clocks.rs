//! The client's two clocks (`engine-design.md` §12, Runtime spine, "Time"):
//! the frame clock presentation runs on, and the driver that turns frame time
//! into simulation ticks. Simulation code reads neither; it advances only in
//! ticks.

use std::time::Instant;

use bevy_ecs::prelude::Resource;

use crate::world::tick::TICK_RATE_HZ;

/// Longest frame the driver counts, in seconds. Time past it is lost rather
/// than run, so one long stall (a debugger pause, a window drag) can't queue a
/// burst of catch-up ticks.
const MAX_FRAME_DT: f32 = 0.25;

/// Most ticks one frame runs. A backlog past it is dropped: a machine too slow
/// for the tick rate runs the simulation slower instead of spiraling.
const MAX_TICKS_PER_FRAME: u32 = 5;

/// Presentation time: real time measured once per frame, for camera motion,
/// cloud animation, water shader time and UI. Never a simulation input.
#[derive(Resource)]
pub struct PresentationClock {
    last_frame: Instant,
    /// Seconds since the previous frame.
    pub dt: f32,
    /// Seconds since the clock started.
    pub elapsed: f32,
}

impl PresentationClock {
    pub fn new() -> Self {
        Self { last_frame: Instant::now(), dt: 0.0, elapsed: 0.0 }
    }
    
    /// Measure this frame.
    pub fn advance(&mut self) {
        let now = Instant::now();
        self.dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.elapsed += self.dt;
    }
}

/// Turns frame time into whole simulation ticks for the integrated client.
/// The headless server sleeps until each tick's deadline instead.
#[derive(Resource, Default)]
pub struct TickDriver {
    /// Time owed to the simulation, in ticks. Below 1 between frames.
    owed: f64,
}

impl TickDriver {
    /// Add a frame's time and return how many ticks to run now.
    pub fn advance(&mut self, frame_dt: f32) -> u32 {
        self.owed += f64::from(frame_dt.min(MAX_FRAME_DT)) * f64::from(TICK_RATE_HZ);
        let due = self.owed.floor();
        self.owed -= due;
        (due as u32).min(MAX_TICKS_PER_FRAME)
    }
    
    /// Progress from the last tick toward the next (0..1), for interpolating
    /// what the frame draws.
    pub fn alpha(&self) -> f32 {
        self.owed as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::tick::TICK_DT;

    #[test]
    fn whole_ticks_run_and_the_fraction_carries() {
        let mut driver = TickDriver::default();
        assert_eq!(driver.advance(TICK_DT * 2.5), 2);
        assert!((driver.alpha() - 0.5).abs() < 1e-3, "alpha {}", driver.alpha());
        assert_eq!(driver.advance(TICK_DT * 0.75), 1, "0.5 carried plus 0.75 owes one tick");
        assert!((driver.alpha() - 0.25).abs() < 1e-3, "alpha {}", driver.alpha());
    }

    #[test]
    fn a_long_frame_runs_at_most_the_cap_and_drops_the_backlog() {
        let mut driver = TickDriver::default();
        let owed = (MAX_FRAME_DT * TICK_RATE_HZ as f32).floor() as u32;
        assert_eq!(driver.advance(10.0), owed.min(MAX_TICKS_PER_FRAME));
        assert!(driver.alpha() < 1.0, "the backlog past the cap is dropped, not carried");
    }

    #[test]
    fn short_frames_add_up_to_whole_ticks() {
        // 7 ms frames: most run no tick, some run one, and none is lost.
        let mut driver = TickDriver::default();
        let ran: u32 = (0..1000).map(|_| driver.advance(0.007)).sum();
        assert_eq!(ran, (7.0 * f64::from(TICK_RATE_HZ)).floor() as u32);
    }
}
