//! The job pump that dispatches queued work onto the shared pool.

use bevy_ecs::prelude::*;
use crate::diagnostics::{FrameTimings, SUB_PUMP};
use crate::jobs::JobSystem;
use crate::ui::panels::UiState;

/// Dispatch queued jobs onto the shared pool (P14). Runs after every consumer
/// has submitted for this frame and before meshing consumes results, so a job
/// submitted this frame starts this frame.
pub fn job_pump_system(
    mut jobs: ResMut<JobSystem>,
    mut ui: ResMut<UiState>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_PUMP);
    jobs.pump();
    // Snapshot after pumping, so the panel shows the dispatch this frame
    // produced rather than the state that led to it.
    for kind in crate::jobs::JobKind::ALL {
        ui.job_stats[kind.index()] = jobs.stats(kind);
    }
    ui.job_running_total = jobs.running_total();
    ui.job_max_running = jobs.max_running();
}
