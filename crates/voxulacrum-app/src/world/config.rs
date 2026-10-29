//! Settings the world code owns: chunk streaming budgets, fluid limits and
//! autosave.
//!
//! Kept apart from the editor's `EngineParams` so the world code reads no
//! client settings and can move to server-core (roadmap R3). Built from
//! defaults for now; the headless server (R3c) is its first source other than
//! `Default`.

use bevy_ecs::prelude::Resource;

/// The world's settings, one resource for every server-side system that reads
/// one.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct ServerConfig {
    pub streaming: StreamingConfig,
    pub fluids: FluidLimits,
    pub autosave: AutosaveConfig,
}

/// Chunk streaming's per-tick budgets. How far past a view chunks load and
/// stay loaded belongs to the observer's area of interest, which its camera
/// sizes (the isometric camera's margins are in `CameraParams`).
#[derive(Clone, Debug, PartialEq)]
pub struct StreamingConfig {
    /// Completed generation results inserted into the world per tick.
    pub max_inserts_per_tick: u32,
    /// Main-thread time budget for the unload phase, in milliseconds per tick.
    ///
    /// Eviction runs `save_chunk_on_unload` (zstd + SQLite) for every
    /// persist-dirty chunk leaving the view, and `FinalizeSeam` writes demotions
    /// into nearly every chunk's override bucket - so nearly every eviction is a
    /// write. Unbounded, that phase measured **103 ms on a single frame** during
    /// a pan at maximum supported zoom (`perf-baseline.md` §0.4.0).
    ///
    /// A *time* budget rather than a chunk count, because per-chunk cost scales
    /// with override-bucket size - exactly what differed between regions in that
    /// measurement - so a count calibrated against one region is wrong in the
    /// next. The default is §7.3's "no recurring hitch > 4 ms" line.
    ///
    /// Chunks not evicted this tick are reconsidered the next; nothing is
    /// lost, they stay resident slightly longer. The trade is residency: if
    /// evictions cannot keep pace with a sustained pan, the resident set grows.
    pub unload_budget_ms: f32,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            max_inserts_per_tick: 64,
            unload_budget_ms: 4.0,
        }
    }
}

/// Active-cell thresholds the fluid tick checks.
#[derive(Clone, Debug, PartialEq)]
pub struct FluidLimits {
    /// Above this many active cells the fluid tick logs a warning: a likely
    /// mass-conservation runaway.
    pub warn_active_cells: usize,
    /// Above this many active cells fluids step at half their rate, so a large
    /// flow slows down instead of stalling the frame rate. The fluid tick
    /// decides from the count at that tick, so the result is deterministic.
    pub throttle_active_cells: usize,
}

impl Default for FluidLimits {
    fn default() -> Self {
        Self {
            warn_active_cells: 100_000,
            throttle_active_cells: 20_000,
        }
    }
}

/// How often dirty chunks are saved while they stay resident.
#[derive(Clone, Debug, PartialEq)]
pub struct AutosaveConfig {
    /// Simulated seconds between autosaves. The autosave phase counts them in
    /// ticks ([`Self::interval_ticks`]).
    pub interval_secs: f32,
}

impl AutosaveConfig {
    /// Ticks between autosaves.
    pub fn interval_ticks(&self) -> u64 {
        crate::world::tick::secs_to_ticks(self.interval_secs)
    }
}

impl Default for AutosaveConfig {
    fn default() -> Self {
        Self { interval_secs: 30.0 }
    }
}
