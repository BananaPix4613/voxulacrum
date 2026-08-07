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

/// CPU-work budget while generation is active (roadmap §7.3: "worst case
/// < 33 ms at an active generation frontier"). A budget of its own because a
/// frontier frame is doing work an at-rest frame is not; holding it to the
/// sustained budget would either fail spuriously or drag the sustained budget
/// up to meet it.
pub const FRONTIER_BUDGET_MS: f32 = 33.0;

/// Four at `FrameStage::Meshing` granularity, four splitting `SUB_STREAM`, and
/// four naming the `sim` and `post` systems whose recorded triggers fired when
/// the reference world's ocean and caves landed.
pub const SUB_COUNT: usize = 12;

/// Display labels, in index order.
pub const SUB_NAMES: [&str; SUB_COUNT] = [
    "stream", "seam", "upload", "pump", "drain", "submit", "scan", "unload",
    "fluid", "climate", "params", "save",
];

/// `streaming_tick_system` - result drain, chunk insertion, per-chunk pass rebuilds.
pub const SUB_STREAM: usize = 0;
/// `seam_smoothing_system` - resident-set scan plus cross-chunk seam finalization.
pub const SUB_SEAM: usize = 1;
/// `meshing_tick_system` - 34³ snapshot extraction and GPU buffer upload.
pub const SUB_UPLOAD: usize = 2;
/// `job_pump_system` - dispatch plus the inspector snapshot. Expected
/// negligible; measured anyway so the four spans sum to the stage and the split
/// can be shown to be complete rather than assumed to be.
pub const SUB_PUMP: usize = 3;

// Phases inside `ChunkStreamingManager::tick`, in the order it runs them. The
// order is the contract between `StreamingTickResult::phase_ms` and these
// indices; streaming reports four unnamed spans and this is where they are
// named.
/// Draining completed generations and inserting them through the door.
pub const SUB_DRAIN: usize = 4;
/// `submit_all_dirty` - the resident scan for mesh-dirty chunks.
pub const SUB_SUBMIT: usize = 5;
/// The candidate scan over the load rect, plus generation submission.
pub const SUB_SCAN: usize = 6;
/// The unload scan, save-on-unload, and eviction through the door.
pub const SUB_UNLOAD: usize = 7;

// `FrameStage::Simulation` and `PostFrame` candidates. Each is an item carried
// out of 0.3.0 with a stated trigger; the reference world met the conditions.
/// `fluid_tick_system` - sums `active.len()` across every resident chunk.
pub const SUB_FLUID: usize = 8;
/// `climate_tick_system` - cleared at ≤2.2 ms in Phase 10, before an ocean existed.
pub const SUB_CLIMATE: usize = 9;
/// `param_change_detection_system` - deep-clones `EngineParams` every frame.
pub const SUB_PARAMS: usize = 10;
/// `persistence_autosave_system` - writes fluid as a full-field snapshot (design §9).
pub const SUB_SAVE: usize = 11;

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
    /// This frame's sub-span milliseconds, indexed by the `SUB_*` constants.
    sub: [f32; SUB_COUNT],
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

    // --- generation frontier (roadmap §7.3) - SESSION-scoped ---
    //
    // Deliberately *not* cleared by `publish`, unlike every accumulator above.
    // A fill at the supported maximum radius lasts ~180 ms, so a one-second
    // window is ~80 % at-rest frames and its "worst" is usually not a frontier
    // frame at all. A session-scoped worst with an explicit reset is the only
    // shape in which this budget is measurable inside the supported range.
    /// Worst CPU frame observed while the streamed set was unsatisfied.
    pub frontier_cpu_max_ms: f32,
    /// Per-stage split of that worst frame, so the figure is attributable
    /// rather than merely alarming.
    pub frontier_worst_stages: [f32; STAGE_COUNT],
    /// Sub-span split of that same frame, one level finer.
    ///
    /// **This is one frame's split, and it is not an attribution.** Spans peak
    /// on different frames - two runs of the same motion named `stream` and
    /// `upload` respectively, purely by which frame happened to be worst. Use
    /// `frontier_sub_max` / `frontier_sub_mean_ms` to ask which span is
    /// expensive; use this only to ask what the single worst frame looked like.
    pub frontier_worst_sub: [f32; SUB_COUNT],
    /// Each span's own maximum across frontier frames, which need not fall on
    /// the same frame as any other span's.
    pub frontier_sub_max: [f32; SUB_COUNT],
    /// Per-span sum across frontier frames, for the mean.
    frontier_sub_sum: [f32; SUB_COUNT],
    /// Frontier frames observed. A worst case drawn from 4 frames and one drawn
    /// from 400 are different claims, and a bare maximum hides which you have
    /// (post-phase-10 §3: a metric with no deficit behind it is not a
    /// measurement).
    pub frontier_frames: u32,
    /// Frontier frames whose CPU work exceeded [`FRONTIER_BUDGET_MS`].
    pub frontier_over_budget: u32,
    /// Running CPU sum over frontier frames, for the mean.
    frontier_cpu_sum: f32,
}

impl FrameTimings {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            last_mark: now,
            frame_start: now,
            stage: [0.0; STAGE_COUNT],
            present: 0.0,
            sub: [0.0; SUB_COUNT],
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
            frontier_cpu_max_ms: 0.0,
            frontier_worst_stages: [0.0; STAGE_COUNT],
            frontier_worst_sub: [0.0; SUB_COUNT],
            frontier_sub_max: [0.0; SUB_COUNT],
            frontier_sub_sum: [0.0; SUB_COUNT],
            frontier_frames: 0,
            frontier_over_budget: 0,
            frontier_cpu_sum: 0.0,
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
        self.sub = [0.0; SUB_COUNT];
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

    /// Add to a sub-span directly. Prefer [`Self::sub_timer`] at a call site;
    /// this exists for tests and for any site that already has a duration.
    pub fn add_sub(&mut self, index: usize, ms: f32) {
        if let Some(slot) = self.sub.get_mut(index) {
            *slot += ms;
        }
    }

    /// Open a sub-span timer. Hold the guard for the region to measure.
    pub fn sub_timer(&mut self, index: usize) -> SubTimer<'_> {
        SubTimer { timings: self, index, start: Instant::now() }
    }

    /// Close the final stage and fold this frame into the one-second
    /// accumulator, publishing when the second elapses. Runs after
    /// `FrameStage::PostFrame`.
    ///
    /// `frontier` is true when this frame ran with the streamed set unsatisfied
    /// - chunks wanted but not resident, or generation jobs still outstanding.
    /// The caller supplies it rather than this module deriving it, so the timing
    /// layer keeps no dependency on streaming.
    pub fn frame_end(&mut self, frontier: bool) {
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

        if frontier {
            self.frontier_frames += 1;
            self.frontier_cpu_sum += cpu_ms;
            // Every frontier frame, not only the worst one: a span's own peak is
            // what says whether it is expensive, and it lands wherever it lands.
            for i in 0..SUB_COUNT {
                self.frontier_sub_sum[i] += self.sub[i];
                self.frontier_sub_max[i] = self.frontier_sub_max[i].max(self.sub[i]);
            }
            if cpu_ms > self.frontier_cpu_max_ms {
                self.frontier_cpu_max_ms = cpu_ms;
                // Captured on the frame that set the maximum, so the split
                // describes *that* frame rather than an average over the fill.
                self.frontier_worst_stages = self.stage;
                self.frontier_worst_sub = self.sub;
            }
            if cpu_ms > FRONTIER_BUDGET_MS {
                self.frontier_over_budget += 1;
            }
        }

        if (now - self.acc_start).as_secs_f32() >= 1.0 {
            self.publish(now);
        }
    }

    /// Mean milliseconds for one sub-span across frontier frames; 0 with no
    /// sample. Together with `frontier_sub_max` this separates "which span
    /// reaches the highest peak" from "which span holds the most time" - two
    /// questions a single worst frame's split conflates.
    pub fn frontier_sub_mean_ms(&self, index: usize) -> f32 {
        if self.frontier_frames == 0 {
            0.0
        } else {
            self.frontier_sub_sum.get(index).copied().unwrap_or(0.0)
                / self.frontier_frames as f32
        }
    }

    /// Mean CPU work across frontier frames this session; 0 with no sample.
    pub fn frontier_cpu_mean_ms(&self) -> f32 {
        if self.frontier_frames == 0 {
            0.0
        } else {
            self.frontier_cpu_sum / self.frontier_frames as f32
        }
    }

    /// Discard the frontier sample. The reading is session-scoped, so a
    /// measurement run starts by clearing whatever the startup fill left behind.
    pub fn reset_frontier(&mut self) {
        self.frontier_cpu_max_ms = 0.0;
        self.frontier_worst_stages = [0.0; STAGE_COUNT];
        self.frontier_worst_sub = [0.0; SUB_COUNT];
        self.frontier_sub_max = [0.0; SUB_COUNT];
        self.frontier_sub_sum = [0.0; SUB_COUNT];
        self.frontier_frames = 0;
        self.frontier_over_budget = 0;
        self.frontier_cpu_sum = 0.0;
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

/// Times a sub-span for as long as it lives, recording on drop.
///
/// A guard rather than a start/stop pair because the systems it wraps have
/// several exit paths: a `return` past a manual stop would silently drop that
/// frame's contribution, and it would do so on precisely the frames cheap
/// enough to bail early - biasing the split in the direction that makes a hot
/// span look cold.
pub struct SubTimer<'a> {
    timings: &'a mut FrameTimings,
    index: usize,
    start: Instant,
}

impl Drop for SubTimer<'_> {
    fn drop(&mut self) {
        let ms = self.start.elapsed().as_secs_f32() * 1000.0;
        self.timings.add_sub(self.index, ms);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_frontier_frames_enter_the_frontier_sample() {
        let mut t = FrameTimings::new();
        t.frame_begin();
        t.frame_end(false);
        assert_eq!(
            t.frontier_frames, 0,
            "an at-rest frame belongs to the 16.6 ms sustained budget, not the \
             33 ms frontier one; counting it dilutes the sample",
        );
        t.frame_begin();
        t.frame_end(true);
        assert_eq!(t.frontier_frames, 1);
    }

    #[test]
    fn publishing_the_one_second_window_does_not_clear_the_frontier_sample() {
        // The asymmetry that makes this measurable: every other accumulator
        // resets each second, this one does not. Pinned because a later edit
        // that "tidies" `publish` by resetting everything would silently make
        // the frontier budget unmeasurable again.
        let mut t = FrameTimings::new();
        t.frame_begin();
        t.frame_end(true);
        t.publish(Instant::now());
        assert_eq!(t.frontier_frames, 1, "the frontier sample is session-scoped");
    }

    #[test]
    fn the_worst_frontier_frame_captures_its_own_sub_span_split() {
        let mut t = FrameTimings::new();

        // Frame 1: deliberately the slower of the two, so which frame holds the
        // maximum is unambiguous. The sleep establishes an ordering; it is not
        // measuring anything.
        t.frame_begin();
        t.add_sub(SUB_UPLOAD, 12.5);
        std::thread::sleep(std::time::Duration::from_millis(2));
        t.frame_end(true);
        assert_eq!(
            t.frontier_worst_sub[SUB_UPLOAD], 12.5,
            "the split must come from the frame that set the maximum",
        );

        // Frame 2: a frontier frame that is not the worst must not overwrite it,
        // or the split would describe the most recent frame instead.
        t.frame_begin();
        t.add_sub(SUB_SEAM, 99.0);
        t.frame_end(true);
        assert_eq!(t.frontier_worst_sub[SUB_UPLOAD], 12.5);
        assert_eq!(t.frontier_worst_sub[SUB_SEAM], 0.0);
    }

    #[test]
    fn sub_spans_do_not_leak_across_frames() {
        let mut t = FrameTimings::new();
        t.frame_begin();
        t.add_sub(SUB_SEAM, 5.0);
        t.frame_end(false);
        t.frame_begin();
        assert_eq!(t.sub[SUB_SEAM], 0.0, "each frame's split must start empty");
    }


    #[test]
    fn per_span_maxima_are_independent_of_the_worst_frame() {
        // The defect this replaced: attributing a budget breach from the single
        // worst frame's split, when the expensive spans peak on *different*
        // frames. Two runs of the same motion named opposite owners that way.
        let mut t = FrameTimings::new();

        // Frame 1 — the slowest frame overall; its cost sits in `stream`.
        t.frame_begin();
        t.add_sub(SUB_STREAM, 40.0);
        std::thread::sleep(std::time::Duration::from_millis(2));
        t.frame_end(true);

        // Frame 2 — cheaper overall, but holds the largest `upload`.
        t.frame_begin();
        t.add_sub(SUB_UPLOAD, 18.0);
        t.frame_end(true);

        assert_eq!(
            t.frontier_worst_sub[SUB_UPLOAD], 0.0,
            "the worst-frame split belongs to frame 1, which had no upload cost",
        );
        assert_eq!(
            t.frontier_sub_max[SUB_UPLOAD], 18.0,
            "but upload's own maximum is frame 2's, and that is the number that \
             says whether upload is expensive",
        );
        assert_eq!(t.frontier_sub_max[SUB_STREAM], 40.0);
    }
}
