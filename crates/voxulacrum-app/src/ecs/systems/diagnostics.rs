//! Frame timing boundaries, the frame counter, and the per-frame stats the
//! performance panel reads.

use bevy_ecs::prelude::*;
use glam::IVec3;
use crate::app::FrameCounter;
use crate::diagnostics::FrameTimings;
use crate::ecs::resources::VoxelWorld;
use crate::jobs::JobSystem;
use crate::simulation::manager::{FrameState, SimulationManager};
use crate::ui::panels::UiState;
use crate::world::chunk::CHUNK_WORLD_SIZE;
use crate::world::streaming::ChunkStreamingManager;

pub fn frame_counter_system(
    mut counter: ResMut<FrameCounter>,
    timings: Res<FrameTimings>,
    mut ui: ResMut<UiState>
) {
    counter.tick();
    ui.fps = counter.fps;
    // Mirror the once-per-second published snapshot; the panel draws from
    // `UiState`, and these values only change on the second boundary, so the
    // copy is not a per-frame cost worth avoiding.
    ui.over_budget_frames = timings.over_budget;
    ui.worst_frame_ms = timings.inter_max_ms;
    ui.cpu_max_ms = timings.cpu_max_ms;
    ui.present_mean_ms = timings.present_mean_ms;
    ui.stage_mean_ms = timings.stage_mean_ms;
    ui.stage_max_ms = timings.stage_max_ms;
    // Session-scoped, so unlike the rows above these change continuously rather
    // than on the second boundary. Mirrored here anyway because the panel draws
    // only from `UiState`; being one frame stale is immaterial for a maximum.
    ui.frontier_cpu_max_ms = timings.frontier_cpu_max_ms;
    ui.frontier_cpu_mean_ms = timings.frontier_cpu_mean_ms();
    ui.frontier_frames = timings.frontier_frames;
    ui.frontier_over_budget = timings.frontier_over_budget;
    ui.frontier_worst_stages = timings.frontier_worst_stages;
    ui.frontier_worst_sub = timings.frontier_worst_sub;
    ui.frontier_sub_max = timings.frontier_sub_max;
    for i in 0..crate::diagnostics::SUB_COUNT {
        ui.frontier_sub_mean[i] = timings.frontier_sub_mean_ms(i);
    }
}

// ==========================================================================
// Frame timing boundaries (diagnostics A1)
// ==========================================================================
// Seven systems that belong to no `FrameStage` set and are ordered against the
// sets themselves, so each runs exactly between two stages. Written as separate
// named functions rather than a closure factory so the schedule reads as an
// explicit list of boundaries and the stage indices are visible at each site.

pub fn frame_timings_begin(mut t: ResMut<FrameTimings>) {
    t.frame_begin();
}

pub fn frame_timings_end_input(mut t: ResMut<FrameTimings>) {
    t.end_stage(0);
}

pub fn frame_timings_end_simulation(mut t: ResMut<FrameTimings>) {
    t.end_stage(1);
}

pub fn frame_timings_end_meshing(mut t: ResMut<FrameTimings>) {
    t.end_stage(2);
}

pub fn frame_timings_end_uniform(mut t: ResMut<FrameTimings>) {
    t.end_stage(3);
}

pub fn frame_timings_end_render(mut t: ResMut<FrameTimings>) {
    t.end_stage(4);
}

pub fn frame_timings_frame_end(
    mut t: ResMut<FrameTimings>,
    mut ui: ResMut<UiState>,
    streaming: Res<ChunkStreamingManager>,
) {
    // Before accumulating, so the frame the button was clicked on starts the
    // fresh sample rather than being discarded by it.
    if std::mem::take(&mut ui.reset_frontier_requested) {
        t.reset_frontier();
    }
    // "At an active generation frontier" (roadmap §7.3) = the streamed set is
    // not yet satisfied: chunks wanted but not resident, or generation jobs
    // still outstanding. `missing` alone would miss the tail of a fill where
    // the last results are landing; `in_flight` alone would miss frames where a
    // deficit is open but backpressure has not submitted yet.
    let s = streaming.stats();
    t.frame_end(s.missing > 0 || s.in_flight > 0);
}

pub fn compute_stats_system(
    world: Res<VoxelWorld>,
    sim: Res<SimulationManager>,
    frame: Res<FrameState>,
    jobs: Res<JobSystem>,
    mut ui: ResMut<UiState>,
) {
    ui.frame_time_ms = frame.dt * 1000.0;
    let mut total_tris: u64 = 0;
    let mut chunks_visible: u32 = 0;
    let mut chunks_total: u32 = 0;
    let mut branch_segments: u64 = 0;
    let mut canopy_lobes: u64 = 0;
    for chunk in world.0.chunks.values() {
        branch_segments += chunk.segments.len() as u64;
        canopy_lobes += chunk.lobes.len() as u64;
        if chunk.mesh.is_some() {
            chunks_total += 1;
            if sim.frustum.is_chunk_visible(chunk.data.coord.into()) {
                chunks_visible += 1;
                total_tris += chunk.mesh.as_ref().unwrap().index_count as u64 / 3;
            }
        }
    }
    ui.total_triangles = total_tris;
    ui.chunks_visible = chunks_visible;
    ui.chunks_total = chunks_total;
    ui.branch_segments = branch_segments;
    ui.canopy_lobes = canopy_lobes;

    ui.mutation_log = world.0.mutation_log();

    // Explicit operator action: ~9 ms per chunk regenerated, so it produces one
    // long frame by design rather than being spread across many.
    if ui.determinism_check_requested {
        ui.determinism_check_requested = false;
        let cam = sim.camera.smooth_target;
        let camera_chunk = IVec3::new(
            (cam.x / CHUNK_WORLD_SIZE).floor() as i32,
            0,
            (cam.z / CHUNK_WORLD_SIZE).floor() as i32,
        );
        ui.determinism = world.0.verify_determinism(32, camera_chunk, jobs.pool());
    }

    // Residency sampling walks every resident chunk summing per-layer bytes, so
    // it runs about once a second rather than joining the per-frame full-set
    // scans that audit §4.9 identifies as the secondary cost at high zoom.
    if ui.residency_countdown == 0 {
        ui.residency = world.0.sample_residency(ui.residency);
        ui.residency_countdown = 60;
    } else {
        ui.residency_countdown -= 1;
    }
}
