//! What a benchmark run measured, as written to its results file.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::stats::Summary;
use crate::jobs::JobKindStats;

/// One run's measurements, over its `Measure` window only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BenchResult {
    /// The scenario's name, such as `S1`.
    pub scenario: String,
    /// The package whose world ran.
    pub package: String,
    /// The engine version that ran it.
    pub engine: String,
    /// The GPU, as the Performance panel names it.
    pub gpu: String,
    /// The window's drawable size in pixels.
    pub surface: (u32, u32),
    /// Wall-clock seconds the window lasted.
    pub seconds: f64,
    /// Frames recorded.
    pub frames: usize,
    /// Of those, frames with the streamed set unsatisfied: chunks missing or
    /// generation outstanding. Zero for a run that was at rest throughout.
    pub frontier_frames: usize,
    /// CPU work per frame, in milliseconds: the schedule span minus the
    /// present block, as the Performance panel measures it.
    pub frame_cpu_ms: Summary,
    /// Mean milliseconds per frame in each stage, in schedule order. The
    /// render stage includes the present block.
    pub stage_mean_ms: Vec<(String, f32)>,
    /// CPU seconds the whole process used per wall second, every thread
    /// included; `None` where the platform's figure isn't read.
    pub process_cpu_per_second: Option<f64>,
    /// Generation jobs that finished inside the window.
    pub generate: JobWindow,
    /// Meshing jobs that finished inside the window.
    pub mesh: JobWindow,
    /// The last fill before the window opened: milliseconds from the view
    /// changing until the streamed set was complete, and how many chunks it
    /// loaded.
    pub fill_ms: f32,
    pub fill_chunks: usize,
    /// Every fill that completed inside the window, in order. A moving view
    /// opens one at each chunk boundary it crosses, so a pan records many.
    pub fills: Vec<Fill>,
    /// Input actions the scenario pressed inside the window.
    pub presses: u32,
    /// Commands from actors (Authoring and Play origins: edits, pours, stamps)
    /// the mutation door accepted inside the window. Set against `presses`, it
    /// shows whether scripted edits landed: one aimed out of reach does nothing.
    pub actor_commands: u64,
    /// Straight-line distance across the ground between where the player was
    /// first and last seen inside the window; 0 with no player. A walk that
    /// stalls against a cliff shows here.
    pub walked: f32,
    /// Streaming and drawing when the window closed.
    pub load_radius: i32,
    pub resident_chunks: usize,
    pub chunks_visible: u32,
    pub chunks_meshed: u32,
    pub triangles: u64,
    /// Resident memory when the window closed, in MB: chunk layers on the CPU,
    /// and chunk mesh buffers on the GPU.
    pub cpu_memory_mb: f32,
    pub gpu_memory_mb: f32,
}

/// One completed fill: milliseconds from the view changing until the streamed
/// set was complete, and how many chunks it had to load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    pub ms: f32,
    pub chunks: usize,
}

/// Jobs of one kind that finished inside a window, and their mean duration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobWindow {
    pub completed: u64,
    pub mean_ms: f32,
}

impl JobWindow {
    /// The jobs that finished between two snapshots of one kind's session
    /// totals.
    pub fn between(start: &JobKindStats, end: &JobKindStats) -> Self {
        let completed = end.completed.saturating_sub(start.completed);
        let total_ms = f64::from(end.mean_ms) * end.completed as f64
            - f64::from(start.mean_ms) * start.completed as f64;
        Self {
            completed,
            mean_ms: if completed > 0 { (total_ms / completed as f64) as f32 } else { 0.0 },
        }
    }
}

impl BenchResult {
    /// Write the results to `path` as JSON, creating its directory.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Two lines for the console: the headline figures, and where the full
    /// results went.
    pub fn report(&self, path: &Path) -> String {
        let process = match self.process_cpu_per_second {
            Some(cpu) => format!("{cpu:.2} CPU s/s"),
            None => "process CPU not read".to_string(),
        };
        let cpu = &self.frame_cpu_ms;
        format!(
            "bench {}: {} frames in {:.1} s, frame CPU mean {:.2} / p95 {:.2} / max {:.2} ms, {process}\n  results: {}",
            self.scenario, self.frames, self.seconds, cpu.mean, cpu.p95, cpu.max, path.display(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(completed: u64, mean_ms: f32) -> JobKindStats {
        JobKindStats { completed, mean_ms, ..Default::default() }
    }

    #[test]
    fn a_job_window_counts_only_the_jobs_inside_it() {
        // 100 jobs averaging 10 ms before; 50 more averaging 4 ms inside.
        let window = JobWindow::between(&stats(100, 10.0), &stats(150, 8.0));
        assert_eq!(window.completed, 50);
        assert!((window.mean_ms - 4.0).abs() < 1e-4, "{}", window.mean_ms);
    }

    #[test]
    fn a_job_window_with_no_jobs_has_no_mean() {
        assert_eq!(JobWindow::between(&stats(7, 9.0), &stats(7, 9.0)), JobWindow::default());
    }

    #[test]
    fn a_result_survives_its_file() {
        let result = BenchResult {
            scenario: "S1".into(),
            frames: 3600,
            frame_cpu_ms: Summary { mean: 3.5, p95: 4.25, max: 9.0 },
            stage_mean_ms: vec![("input".into(), 0.02), ("render".into(), 12.5)],
            process_cpu_per_second: Some(1.5),
            fills: vec![Fill { ms: 286.0, chunks: 116 }],
            ..Default::default()
        };
        let path = std::env::temp_dir()
            .join(format!("voxulacrum-bench-{}", std::process::id()))
            .join("S1.json");
        result.write(&path).unwrap();
        let read: BenchResult = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read, result);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
