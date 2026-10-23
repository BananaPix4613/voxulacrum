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

/// Chunk streaming margins and per-frame budgets.
///
/// The two margins size the isometric view test, which R6 moves into the
/// camera module as that camera's area of interest. They stay here until then.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamingConfig {
    /// Proportional load margin: fraction of visible extent added as buffer.
    /// E.g., 0.15 = 15% buffer on each side beyond the visible view rect.
    /// A minimum of 2 chunks is always enforced regardless of zoom.
    pub load_margin: f32,
    /// Proportional unload margin: fraction of visible extent beyond which
    /// chunks are unloaded. Should be larger than load_margin to prevent
    /// load/unload thrashing (hysteresis).
    pub unload_margin: f32,
    /// Completed generation results inserted into the world per frame.
    pub max_gen_per_frame: u32,
    /// Main-thread time budget for the unload phase, in milliseconds.
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
    /// Chunks not evicted this frame are reconsidered next frame; nothing is
    /// lost, they stay resident slightly longer. The trade is residency: if
    /// evictions cannot keep pace with a sustained pan, the resident set grows.
    pub unload_budget_ms: f32,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            load_margin: 0.40,
            unload_margin: 0.55,
            max_gen_per_frame: 64,
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
    /// Seconds of frame time between autosaves. R2 counts it in ticks.
    pub interval_secs: f32,
}

impl Default for AutosaveConfig {
    fn default() -> Self {
        Self { interval_secs: 30.0 }
    }
}
