//! Frame timing instrumentation (roadmap §4.4 `FrameTimings`, deliverable A1).
//!
//! Measures three separate quantities, because conflating them is what left the
//! engine's frame cost unreadable across two phases:
//!
//! 1. **CPU time per `FrameStage`** - real work, attributable to one stage.
//! 2. **The present block** - time inside `get_current_texture()` and
//!    `present()`. Under `PresentMode::Fifo` this is vsync *waiting*, not work.
//!    It is a subset of the `Render` stage and is reported separately so a
//!    vsync-bound frame is distinguishable from a CPU-bound one.
//! 3. **Wall-clock inter-frame time** - what the panel used to call "Frame". On
//!    a vsync-limited app it is dominated by (2) and is a poor budget metric.
//!
//! The budget (roadmap §7.3) is therefore stated against CPU work:
//! `cpu_ms = frame_ms - present_ms`, compared to [`FRAME_BUDGET_MS`].
//!
//! Values accumulate per frame and publish once per second: a per-frame readout
//! of something recurring 60 times a second is unreadable at human sampling
//! rates, which is why the first framing of the at-rest hitch was wrong.

use std::time::Instant;

use bevy_ecs::prelude::Resource;

/// One entry per `FrameStage`, in schedule order.
pub const STAGE_COUNT: usize = 6;

/// Display labels for the six stages, in schedule order.
pub const STAGE_NAMES: [&str; STAGE_COUNT] =
    ["input", "sim", "mesh", "uniform", "render", "post"];

/// Per-frame CPU-work budget in milliseconds (roadmap §7.3: 16.6 ms sustained).
pub const FRAME_BUDGET_MS: f32 = 16.6;

/// Per-`FrameStage` timing, accumulated per frame and published each second.
#[derive(Resource)]
pub struct FrameTimings {
    /// Start of the stage span currently being measured.
    last_mark: Instant,
    /// Start of the current frame, for the wall-clock inter-frame delta.
    frame_start: Instant,
    /// This frame's per-stage CPU milliseconds.
    stage: [f32; STAGE_COUNT],
    /// This frame's swapchain acquire + present block, in milliseconds.
    present: f32,
    /// This frame's begin-to-begin interval, in milliseconds.
    inter_frame: f32,
    /// The previous frame's schedule span, so `outside` pairs correctly
    /// (the interval measured at begin(N) spans the gap after end(N-1)).
    prev_schedule: f32,
    /// Gap between the previous schedule run ending and this one starting.
    outside: f32,

    // --- one-second accumulator ---
    acc_start: Instant,
    acc_frames: u32,
    acc_stage_sum: [f32; STAGE_COUNT],
    acc_stage_max: [f32; STAGE_COUNT],
    acc_present_sum: f32,
    acc_present_max: f32,
    acc_cpu_max: f32,
    acc_frame_max: f32,
    acc_outside_sum: f32,
    acc_outside_max: f32,
    acc_inter_max: f32,
    acc_over_budget: u32,

    // --- published once per second; the panel and the log read these ---
    pub fps: f32,
    pub stage_mean_ms: [f32; STAGE_COUNT],
    pub stage_max_ms: [f32; STAGE_COUNT],
    pub present_mean_ms: f32,
    pub present_max_ms: f32,
    /// Worst CPU work (schedule span minus present) in the last second.
    pub cpu_max_ms: f32,
    /// Worst schedule-run span (begin-to-end of the ECS frame) last second.
    pub frame_max_ms: f32,
    /// Time between the end of one schedule run and the start of the next —
    /// the winit event-loop turn, which no `FrameStage` covers. Split out
    /// because the schedule span and the true inter-frame interval disagreed
    /// by ~15 ms in one regime and agreed exactly in another, and nothing
    /// inside the schedule could account for the difference.
    pub outside_mean_ms: f32,
    pub outside_max_ms: f32,
    /// Worst true inter-frame interval (begin-to-begin) in the last second.
    pub inter_max_ms: f32,
    /// Frames whose CPU work exceeded [`FRAME_BUDGET_MS`] in the last second.
    pub over_budget: u32,
}

impl FrameTimings {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            last_mark: now,
            frame_start: now,
            stage: [0.0; STAGE_COUNT],
            present: 0.0,
            inter_frame: 0.0,
            prev_schedule: 0.0,
            outside: 0.0,
            acc_start: now,
            acc_frames: 0,
            acc_stage_sum: [0.0; STAGE_COUNT],
            acc_stage_max: [0.0; STAGE_COUNT],
            acc_present_sum: 0.0,
            acc_present_max: 0.0,
            acc_cpu_max: 0.0,
            acc_frame_max: 0.0,
            acc_outside_sum: 0.0,
            acc_outside_max: 0.0,
            acc_inter_max: 0.0,
            acc_over_budget: 0,
            fps: 0.0,
            stage_mean_ms: [0.0; STAGE_COUNT],
            stage_max_ms: [0.0; STAGE_COUNT],
            present_mean_ms: 0.0,
            present_max_ms: 0.0,
            cpu_max_ms: 0.0,
            frame_max_ms: 0.0,
            outside_mean_ms: 0.0,
            outside_max_ms: 0.0,
            inter_max_ms: 0.0,
            over_budget: 0,
        }
    }

    /// Open a new frame. Runs before `FrameStage::Input`.
    pub fn frame_begin(&mut self) {
        let now = Instant::now();
        // `frame_start` still holds the *previous* frame's begin until it is
        // overwritten below, so the interval is available for free here.
        self.inter_frame = (now - self.frame_start).as_secs_f32() * 1000.0;
        self.outside = (self.inter_frame - self.prev_schedule).max(0.0);
        self.frame_start = now;
        self.last_mark = now;
        self.stage = [0.0; STAGE_COUNT];
        self.present = 0.0;
    }

    /// Close stage `index` and open the next. Runs on each stage boundary.
    /// An out-of-range index still advances the mark, so a mis-wired boundary
    /// loses one stage's attribution rather than corrupting every later span.
    pub fn end_stage(&mut self, index: usize) {
        let now = Instant::now();
        if let Some(slot) = self.stage.get_mut(index) {
            *slot = (now - self.last_mark).as_secs_f32() * 1000.0;
        }
        self.last_mark = now;
    }

    /// Record time blocked in swapchain acquire or present. Called twice per
    /// frame from inside the render stage; both spans fold into one total.
    /// A frame that early-returns before presenting simply reports less
    /// present time, which over-reports its CPU work - rare and acceptable.
    pub fn add_present(&mut self, ms: f32) {
        self.present += ms;
    }

    /// Close the final stage and fold this frame into the one-second
    /// accumulator, publishing when the second elapses. Runs after
    /// `FrameStage::PostFrame`.
    pub fn frame_end(&mut self) {
        self.end_stage(STAGE_COUNT - 1);

        // `end_stage` just set `last_mark` to now; reuse it rather than taking
        // a second clock reading that would double-count the gap between them.
        let now = self.last_mark;
        let frame_ms = (now - self.frame_start).as_secs_f32() * 1000.0;
        let cpu_ms = (frame_ms - self.present).max(0.0);
        self.prev_schedule = frame_ms;

        self.acc_frames += 1;
        for i in 0..STAGE_COUNT {
            self.acc_stage_sum[i] += self.stage[i];
            self.acc_stage_max[i] = self.acc_stage_max[i].max(self.stage[i]);
        }
        self.acc_present_sum += self.present;
        self.acc_present_max = self.acc_present_max.max(self.present);
        self.acc_cpu_max = self.acc_cpu_max.max(cpu_ms);
        self.acc_frame_max = self.acc_frame_max.max(frame_ms);
        self.acc_outside_sum += self.outside;
        self.acc_outside_max = self.acc_outside_max.max(self.outside);
        self.acc_inter_max = self.acc_inter_max.max(self.inter_frame);
        if cpu_ms > FRAME_BUDGET_MS {
            self.acc_over_budget += 1;
        }

        if (now - self.acc_start).as_secs_f32() >= 1.0 {
            self.publish(now);
        }
    }

    fn publish(&mut self, now: Instant) {
        let n = self.acc_frames.max(1) as f32;
        self.fps = self.acc_frames as f32;
        for i in 0..STAGE_COUNT {
            self.stage_mean_ms[i] = self.acc_stage_sum[i] / n;
            self.stage_max_ms[i] = self.acc_stage_max[i];
        }
        self.present_mean_ms = self.acc_present_sum / n;
        self.present_max_ms = self.acc_present_max;
        self.cpu_max_ms = self.acc_cpu_max;
        self.frame_max_ms = self.acc_frame_max;
        self.outside_mean_ms = self.acc_outside_sum / n;
        self.outside_max_ms = self.acc_outside_max;
        self.inter_max_ms = self.acc_inter_max;
        self.over_budget = self.acc_over_budget;

        // Logged as well as panelled so a measurement can be taken with the UI
        // hidden, and so a session leaves a readable timeline in the console.
        log::info!(
            "frame: {:.0} fps | cpu max {:.1} (budget {:.1}, over {}) | \
             present mean {:.1} max {:.1} | outside mean {:.1} max {:.1} | \
             schedule max {:.1} | interframe max {:.1}   [ms]",
            self.fps,
            self.cpu_max_ms,
            FRAME_BUDGET_MS,
            self.over_budget,
            self.present_mean_ms,
            self.present_max_ms,
            self.outside_mean_ms,
            self.outside_max_ms,
            self.frame_max_ms,
            self.inter_max_ms,
        );
        let stages: Vec<String> = (0..STAGE_COUNT)
            .map(|i| {
                format!(
                    "{} {:.2}/{:.2}",
                    STAGE_NAMES[i], self.stage_mean_ms[i], self.stage_max_ms[i]
                )
            })
            .collect();
        log::info!("stages mean/max ms: {}", stages.join("  "));

        self.acc_start = now;
        self.acc_frames = 0;
        self.acc_stage_sum = [0.0; STAGE_COUNT];
        self.acc_stage_max = [0.0; STAGE_COUNT];
        self.acc_present_sum = 0.0;
        self.acc_present_max = 0.0;
        self.acc_cpu_max = 0.0;
        self.acc_frame_max = 0.0;
        self.acc_outside_sum = 0.0;
        self.acc_outside_max = 0.0;
        self.acc_inter_max = 0.0;
        self.acc_over_budget = 0;
    }
}

// ============================================================================
// Residency (roadmap §4.4 memory/residency panel)
// ============================================================================

/// Resident memory by chunk layer, plus GPU buffers.
///
/// The `peak_*` fields server §7.3's "session growth flat after warm-up": a
/// resident total that keeps climbing while you revisit the same area is a leak,
/// and nothing reported it before this existed.
#[derive(Clone, Copy, Debug, Default)]
pub struct ResidencyStats {
    pub chunks: usize,
    /// Chunks whose voxel storage collapsed to a single value.
    pub uniform_chunks: usize,
    pub voxel_bytes: u64,
    pub detail_bytes: u64,
    pub scatter_bytes: u64,
    pub fluid_bytes: u64,
    pub override_bytes: u64,
    /// Vertex + index buffers of resident chunk meshes.
    pub gpu_mesh_bytes: u64,
    /// Highest CPU-side total seen this session.
    pub peak_cpu_bytes: u64,
    /// Highest GPU mesh total seen this session.
    pub peak_gpu_bytes: u64,
}

impl ResidencyStats {
    /// CPU-side total across every layer.
    pub fn cpu_bytes(&self) -> u64 {
        self.voxel_bytes
            + self.detail_bytes
            + self.scatter_bytes
            + self.fluid_bytes
            + self.override_bytes
    }
}
