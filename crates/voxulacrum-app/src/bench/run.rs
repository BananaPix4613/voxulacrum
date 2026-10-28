//! Running a scenario: one system steps through it at the start of each frame,
//! and another records each frame inside its `Measure` once the frame's
//! timings have closed.

use std::time::Instant;

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use glam::{Vec2, Vec3};

use super::results::{BenchResult, Fill, JobWindow};
use super::scenario::{Scenario, Step};
use super::stats::Summary;
use super::{process_cpu_seconds, BenchSetup};
use crate::diagnostics::{FrameTimings, STAGE_COUNT, STAGE_NAMES};
use crate::ecs::schedule::FrameStage;
use crate::input::{GameAction, InputState, PointerState};
use crate::jobs::{JobKind, JobKindStats, JobSystem};
use crate::player::systems::Player;
use crate::ecs::resources::WindowHandle;
use crate::simulation::manager::SimulationManager;
use crate::ui::panels::UiState;
use crate::world::mutation::{MutationLog, MutationOrigin};
use crate::world::streaming::ChunkStreamingManager;

/// Consecutive frames the engine must be at rest before a `Settle` ends. One
/// idle frame can fall between a job finishing and the next being submitted,
/// so a single one is not rest.
const REST_FRAMES: u32 = 30;

const BYTES_PER_MB: f32 = 1024.0 * 1024.0;

/// A scenario's steps, flattened: a `Measure` becomes its own steps between a
/// begin and an end.
#[derive(Clone, Debug, PartialEq)]
enum Op {
    Camera { target: Vec3, zoom: f32 },
    Pan { to: Vec3, seconds: f32 },
    Cursor { x: f32, y: f32 },
    Press(GameAction),
    Move { direction: Vec2, seconds: f32, every: f32, presses: Vec<GameAction> },
    Settle { timeout: f32 },
    Wait(f32),
    BeginMeasure,
    EndMeasure,
}

/// Append `steps` to `ops`, flattened.
fn program(steps: &[Step], ops: &mut Vec<Op>) {
    for step in steps {
        match step {
            Step::Camera { target, zoom } => {
                ops.push(Op::Camera { target: Vec3::from(*target), zoom: *zoom });
            }
            Step::Pan { to, seconds } => {
                ops.push(Op::Pan { to: Vec3::from(*to), seconds: *seconds });
            }
            Step::Cursor { x, y } => ops.push(Op::Cursor { x: *x, y: *y }),
            Step::Press(action) => ops.push(Op::Press(*action)),
            Step::Move { direction, seconds, every, presses } => ops.push(Op::Move {
                direction: Vec2::from(*direction),
                seconds: *seconds,
                every: *every,
                presses: presses.clone(),
            }),
            Step::Settle { timeout } => ops.push(Op::Settle { timeout: *timeout }),
            Step::Wait(seconds) => ops.push(Op::Wait(*seconds)),
            Step::Measure(inner) => {
                ops.push(Op::BeginMeasure);
                program(inner, ops);
                ops.push(Op::EndMeasure);
            }
        }
    }
}

/// Where a pan from `from` to `to` over `seconds` puts the camera after
/// `elapsed` seconds, and whether it has arrived. A function of time rather
/// than of frames, so the path is the same at any frame rate.
fn pan_position(from: Vec3, to: Vec3, elapsed: f32, seconds: f32) -> (Vec3, bool) {
    if elapsed >= seconds {
        (to, true)
    } else {
        (from.lerp(to, elapsed / seconds), false)
    }
}

/// Commands from actors, Authoring and Play origins, the log has counted since
/// launch. `System` commands (streaming, the fluid sim, seam finalization) are
/// the engine's own and not what a scenario's presses cause.
fn actor_commands(log: &MutationLog) -> u64 {
    log.counts
        .iter()
        .map(|by_origin| {
            by_origin[MutationOrigin::Authoring.index()] + by_origin[MutationOrigin::PlayTime.index()]
        })
        .sum()
}

/// What the measured window has recorded so far.
struct Recording {
    started: Instant,
    process_cpu: Option<f64>,
    generate: JobKindStats,
    mesh: JobKindStats,
    fill_ms: f32,
    fill_chunks: usize,
    /// The streaming fill figures last seen, so a change marks a fill that
    /// just completed.
    last_fill: (f32, usize),
    fills: Vec<Fill>,
    frame_cpu: Vec<f32>,
    stage_sum: [f64; STAGE_COUNT],
    frontier_frames: usize,
    /// Input actions the scenario pressed inside the window.
    presses: u32,
    /// Actor commands the log had counted when the window opened.
    actor_commands: u64,
    /// Where the player was first seen inside the window, and where last.
    walk: Option<(Vec3, Vec3)>,
}

impl Recording {
    fn begin(jobs: &JobSystem, streaming: &ChunkStreamingManager, ui: &UiState) -> Self {
        let s = streaming.stats();
        Self {
            started: Instant::now(),
            process_cpu: process_cpu_seconds(),
            generate: jobs.stats(JobKind::Generate),
            mesh: jobs.stats(JobKind::Mesh),
            fill_ms: s.fill_ms,
            fill_chunks: s.fill_chunks,
            last_fill: (s.fill_ms, s.fill_chunks),
            fills: Vec::new(),
            // Two minutes at 60 fps, so the longest scenario never reallocates.
            frame_cpu: Vec::with_capacity(8192),
            stage_sum: [0.0; STAGE_COUNT],
            frontier_frames: 0,
            presses: 0,
            actor_commands: actor_commands(&ui.mutation_log),
            walk: None,
        }
    }

    fn record(&mut self, cpu_ms: f32, stages: &[f32; STAGE_COUNT], frontier: bool) {
        self.frame_cpu.push(cpu_ms);
        for (sum, &ms) in self.stage_sum.iter_mut().zip(stages) {
            *sum += f64::from(ms);
        }
        if frontier {
            self.frontier_frames += 1;
        }
    }

    /// Note streaming's fill figures for this frame. Streaming publishes a fill
    /// when it completes and keeps it until the next, so a change is a new one.
    fn note_fill(&mut self, ms: f32, chunks: usize) {
        if (ms, chunks) != self.last_fill {
            self.last_fill = (ms, chunks);
            self.fills.push(Fill { ms, chunks });
        }
    }

    /// Note where the player is this frame.
    fn note_player(&mut self, position: Vec3) {
        let first = self.walk.map_or(position, |(first, _)| first);
        self.walk = Some((first, position));
    }

    fn end(
        self,
        scenario: &Scenario,
        jobs: &JobSystem,
        streaming: &ChunkStreamingManager,
        ui: &UiState,
        surface: (u32, u32),
    ) -> BenchResult {
        let seconds = self.started.elapsed().as_secs_f64();
        let frames = self.frame_cpu.len();
        let process_cpu_per_second = match (self.process_cpu, process_cpu_seconds()) {
            (Some(start), Some(end)) if seconds > 0.0 => Some((end - start) / seconds),
            _ => None,
        };
        let s = streaming.stats();
        BenchResult {
            scenario: scenario.name.clone(),
            package: scenario.package.clone(),
            engine: env!("CARGO_PKG_VERSION").to_string(),
            gpu: ui.gpu.clone(),
            surface,
            seconds,
            frames,
            frontier_frames: self.frontier_frames,
            frame_cpu_ms: Summary::of(&self.frame_cpu),
            stage_mean_ms: STAGE_NAMES
                .iter()
                .zip(self.stage_sum)
                .map(|(name, sum)| (name.to_string(), (sum / frames.max(1) as f64) as f32))
                .collect(),
            process_cpu_per_second,
            generate: JobWindow::between(&self.generate, &jobs.stats(JobKind::Generate)),
            mesh: JobWindow::between(&self.mesh, &jobs.stats(JobKind::Mesh)),
            fill_ms: self.fill_ms,
            fill_chunks: self.fill_chunks,
            fills: self.fills,
            presses: self.presses,
            actor_commands: actor_commands(&ui.mutation_log).saturating_sub(self.actor_commands),
            walked: self
                .walk
                .map_or(0.0, |(first, last)| Vec2::new(last.x - first.x, last.z - first.z).length()),
            load_radius: s.load_radius,
            resident_chunks: s.resident,
            chunks_visible: ui.chunks_visible,
            chunks_meshed: ui.chunks_total,
            triangles: ui.total_triangles,
            draws: ui.draws,
            cpu_memory_mb: ui.residency.cpu_bytes() as f32 / BYTES_PER_MB,
            gpu_memory_mb: ui.residency.gpu_mesh_bytes as f32 / BYTES_PER_MB,
        }
    }
}

/// A benchmark in progress: where it is in its scenario, and what it has
/// recorded.
#[derive(Resource)]
pub struct BenchRun {
    setup: BenchSetup,
    ops: Vec<Op>,
    /// Index into `ops` of the step running now.
    next: usize,
    /// When the step running now started.
    op_started: Instant,
    /// True on the frame the step running now is first run.
    op_fresh: bool,
    /// Where the pan running now started from.
    pan_from: Vec3,
    /// Presses the move running now has made.
    presses_made: u32,
    /// Consecutive frames at rest, for `Settle`.
    rest_frames: u32,
    recording: Option<Recording>,
    result: Option<BenchResult>,
    exit_code: Option<i32>,
}

impl BenchRun {
    pub fn new(setup: BenchSetup) -> Self {
        let mut ops = Vec::new();
        program(&setup.scenario.steps, &mut ops);
        Self {
            setup,
            ops,
            next: 0,
            op_started: Instant::now(),
            op_fresh: true,
            pan_from: Vec3::ZERO,
            presses_made: 0,
            rest_frames: 0,
            recording: None,
            result: None,
            exit_code: None,
        }
    }

    /// The process's exit code once the run is over: 0 with its results
    /// written, 1 if it failed.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    fn fail(&mut self, why: &str) {
        eprintln!("bench {}: {why}", self.setup.scenario.name);
        self.recording = None;
        self.exit_code = Some(1);
    }

    /// Write the results and end the run.
    fn finish(&mut self) {
        let Some(result) = self.result.take() else {
            self.fail("the scenario ended without measuring");
            return;
        };
        match result.write(&self.setup.output) {
            Ok(()) => {
                println!("{}", result.report(&self.setup.output));
                self.exit_code = Some(0);
            }
            Err(e) => self.fail(&format!("results not written: {e}")),
        }
    }
}

/// Whether nothing is loading: the streamed set is complete, and no job of
/// any kind is queued or running. Chunks at the edge of the streamed set that
/// wait for a neighbor to mesh never leave the mesher's queue, so its queue is
/// not part of the test; its jobs are.
fn at_rest(streaming: &ChunkStreamingManager, jobs: &JobSystem) -> bool {
    let s = streaming.stats();
    s.missing == 0
        && s.in_flight == 0
        && JobKind::ALL.iter().all(|&kind| {
            let job = jobs.stats(kind);
            job.queued == 0 && job.in_flight == 0
        })
}

/// Step through the scenario: run every step that finishes at once, and stop
/// at the first that has to wait for a later frame.
pub fn bench_drive_system(
    mut bench: ResMut<BenchRun>,
    mut sim: ResMut<SimulationManager>,
    streaming: Res<ChunkStreamingManager>,
    jobs: Res<JobSystem>,
    ui: Res<UiState>,
    window: Res<WindowHandle>,
    mut input: ResMut<InputState>,
    mut pointer: ResMut<PointerState>,
) {
    let bench = &mut *bench;
    if bench.exit_code.is_some() {
        return;
    }
    let rest = at_rest(&streaming, &jobs);
    while let Some(op) = bench.ops.get(bench.next).cloned() {
        let elapsed = bench.op_started.elapsed().as_secs_f32();
        let fresh = std::mem::replace(&mut bench.op_fresh, false);
        let done = match op {
            Op::Pan { to, seconds } => {
                if fresh {
                    bench.pan_from = sim.camera.target;
                }
                let (at, arrived) = pan_position(bench.pan_from, to, elapsed, seconds);
                // As for `Camera`: both targets, so the path is the scenario's
                // and not the smoothing's.
                sim.camera.target = at;
                sim.camera.smooth_target = at;
                arrived
            }
            Op::Cursor { x, y } => {
                let size = window.inner_size();
                pointer.cursor = Some((x * size.width as f32, y * size.height as f32));
                true
            }
            Op::Press(action) => {
                input.press(action);
                if let Some(recording) = bench.recording.as_mut() {
                    recording.presses += 1;
                }
                true
            }
            Op::Move { direction, seconds, every, presses } => {
                if fresh {
                    bench.presses_made = 0;
                }
                input.set_move_vector(direction);
                // One press per frame at most; a long frame catches up over
                // the next few rather than pressing twice at once.
                let due = (elapsed / every) as u32;
                if !presses.is_empty() && elapsed < seconds && bench.presses_made < due {
                    input.press(presses[bench.presses_made as usize % presses.len()]);
                    bench.presses_made += 1;
                    if let Some(recording) = bench.recording.as_mut() {
                        recording.presses += 1;
                    }
                }
                elapsed >= seconds
            }
            Op::Camera { target, zoom } => {
                // Both targets, so the camera's smoothing has nothing to ease
                // toward and the view is where the scenario put it this frame.
                sim.camera.target = target;
                sim.camera.smooth_target = target;
                sim.camera.zoom = zoom;
                true
            }
            Op::Settle { timeout } => {
                bench.rest_frames = if rest { bench.rest_frames + 1 } else { 0 };
                if bench.rest_frames >= REST_FRAMES {
                    true
                } else if elapsed > timeout {
                    let s = streaming.stats();
                    bench.fail(&format!(
                        "not at rest after {timeout} s: {} chunks missing, {} generating, {} jobs running",
                        s.missing,
                        s.in_flight,
                        jobs.running_total(),
                    ));
                    return;
                } else {
                    false
                }
            }
            Op::Wait(seconds) => elapsed >= seconds,
            Op::BeginMeasure => {
                bench.recording = Some(Recording::begin(&jobs, &streaming, &ui));
                true
            }
            Op::EndMeasure => {
                if let Some(recording) = bench.recording.take() {
                    let size = window.inner_size();
                    bench.result = Some(recording.end(
                        &bench.setup.scenario,
                        &jobs,
                        &streaming,
                        &ui,
                        (size.width, size.height),
                    ));
                }
                true
            }
        };
        if !done {
            return;
        }
        bench.next += 1;
        bench.op_started = Instant::now();
        bench.op_fresh = true;
        bench.rest_frames = 0;
    }
    bench.finish();
}

/// Record the frame that just closed, while a `Measure` runs.
pub fn bench_record_system(
    mut bench: ResMut<BenchRun>,
    timings: Res<FrameTimings>,
    streaming: Res<ChunkStreamingManager>,
    players: Query<&Player>,
) {
    let Some(recording) = bench.recording.as_mut() else {
        return;
    };
    let (cpu_ms, stages) = timings.last_frame();
    // The same test `frame_timings_frame_end` uses for the frontier sample.
    let s = streaming.stats();
    recording.record(cpu_ms, &stages, s.missing > 0 || s.in_flight > 0);
    recording.note_fill(s.fill_ms, s.fill_chunks);
    if let Some(player) = players.iter().next() {
        recording.note_player(player.curr.position);
    }
}

/// Add the benchmark's systems to the frame schedule: stepping through the
/// scenario once input has been read and before anything in the same stage
/// reads it, so a scripted press is seen this frame rather than cleared unread
/// by the next; and recording each frame after its timings close, outside the
/// span they measure.
pub fn add_systems(schedule: &mut Schedule) {
    schedule.add_systems((
        bench_drive_system
            .in_set(FrameStage::Input)
            .after(crate::input::process_input_system)
            .before(crate::input::toggle_ui_system)
            .before(crate::player::systems::player_mode_toggle_system),
        bench_record_system.after(crate::ecs::systems::diagnostics::frame_timings_frame_end),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recording with nothing in it, for tests that cannot build the job
    /// system and streaming manager `Recording::begin` reads.
    fn empty_recording() -> Recording {
        Recording {
            started: Instant::now(),
            process_cpu: None,
            generate: JobKindStats::default(),
            mesh: JobKindStats::default(),
            fill_ms: 0.0,
            fill_chunks: 0,
            last_fill: (0.0, 0),
            fills: Vec::new(),
            frame_cpu: Vec::new(),
            stage_sum: [0.0; STAGE_COUNT],
            frontier_frames: 0,
            presses: 0,
            actor_commands: 0,
            walk: None,
        }
    }

    #[test]
    fn actor_commands_count_authoring_and_play_but_not_the_engine() {
        let mut log = MutationLog::default();
        log.counts[0][MutationOrigin::PlayTime.index()] = 5;
        log.counts[4][MutationOrigin::Authoring.index()] = 1;
        log.counts[6][MutationOrigin::System.index()] = 900;
        assert_eq!(actor_commands(&log), 6);
    }

    #[test]
    fn a_walk_keeps_where_the_player_started_and_where_they_are() {
        let mut recording = empty_recording();
        recording.note_player(Vec3::new(64.0, 30.0, 64.0));
        recording.note_player(Vec3::new(70.0, 31.0, 64.0));
        recording.note_player(Vec3::new(67.0, 32.0, 68.0));
        assert_eq!(recording.walk, Some((Vec3::new(64.0, 30.0, 64.0), Vec3::new(67.0, 32.0, 68.0))));
    }

    #[test]
    fn a_measure_flattens_into_its_steps_between_a_begin_and_an_end() {
        let steps = [
            Step::Camera { target: (1.0, 2.0, 3.0), zoom: 40.0 },
            Step::Settle { timeout: 60.0 },
            Step::Measure(vec![Step::Wait(60.0)]),
        ];
        let mut ops = Vec::new();
        program(&steps, &mut ops);
        assert_eq!(
            ops,
            vec![
                Op::Camera { target: Vec3::new(1.0, 2.0, 3.0), zoom: 40.0 },
                Op::Settle { timeout: 60.0 },
                Op::BeginMeasure,
                Op::Wait(60.0),
                Op::EndMeasure,
            ],
        );
    }

    #[test]
    fn a_pan_moves_with_time_and_arrives_exactly() {
        let from = Vec3::new(64.0, 32.0, 64.0);
        let to = Vec3::new(364.0, 32.0, 64.0);
        assert_eq!(pan_position(from, to, 0.0, 25.0), (from, false));
        assert_eq!(pan_position(from, to, 12.5, 25.0), (Vec3::new(214.0, 32.0, 64.0), false));
        assert_eq!(pan_position(from, to, 25.3, 25.0), (to, true));
    }

    #[test]
    fn each_fill_streaming_completes_is_recorded_once() {
        let mut recording = empty_recording();
        recording.note_fill(0.0, 0);
        recording.note_fill(286.0, 116);
        recording.note_fill(286.0, 116);
        recording.note_fill(310.5, 120);
        assert_eq!(
            recording.fills,
            vec![Fill { ms: 286.0, chunks: 116 }, Fill { ms: 310.5, chunks: 120 }],
        );
    }

    #[test]
    fn a_recording_sums_its_frames_and_counts_the_frontier() {
        let mut recording = empty_recording();
        recording.record(3.0, &[1.0, 2.0, 0.0, 0.0, 10.0, 0.0], false);
        recording.record(5.0, &[1.0, 4.0, 0.0, 0.0, 12.0, 0.0], true);
        assert_eq!(recording.frame_cpu, vec![3.0, 5.0]);
        assert_eq!(recording.stage_sum[1], 6.0);
        assert_eq!(recording.frontier_frames, 1);
    }
}
