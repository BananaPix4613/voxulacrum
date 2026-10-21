//! Scripted benchmarks (`--bench`): a scenario file drives the camera and
//! input through a fixed sequence, and the run records the CPU work of every
//! frame in its measured window. The procedure, and the readings taken with
//! it, are in `docs/perf-baseline.md`.

pub mod results;
pub mod run;
pub mod scenario;
pub mod stats;
mod process_cpu;

use std::path::PathBuf;

pub use process_cpu::process_cpu_seconds;
pub use run::{add_systems, BenchRun};

use crate::params::EngineParams;
use scenario::Scenario;

/// Directory under `saves/` a benchmark keeps its world in. Emptied before
/// every run, so each run starts from freshly generated terrain and none
/// touches the package's own saves.
pub const SAVE_NAME: &str = "bench";

/// A benchmark the command line asked for, checked and ready to run.
pub struct BenchSetup {
    pub scenario: Scenario,
    /// Where the results file goes.
    pub output: PathBuf,
}

impl BenchSetup {
    /// The benchmark `--bench <scenario>` asks for, or `None` without the flag.
    /// `<scenario>` is a path when it ends in `.ron`, and otherwise the name of
    /// a file in the engine's `bench/` directory. `package` is the id of the
    /// world this run loads, and a scenario written for another is refused.
    /// So is a build with debug assertions, whose timings are not the engine's
    /// (`docs/perf-baseline.md`, "Measurement hazards").
    pub fn from_args(args: &[String], package: &str) -> Result<Option<Self>, String> {
        let Some(i) = args.iter().position(|a| a == "--bench") else {
            return Ok(None);
        };
        let Some(arg) = args.get(i + 1).filter(|a| !a.starts_with("--")) else {
            return Err("--bench takes a scenario: a name in bench/, or a path to a .ron file".into());
        };
        let scenario = Scenario::load(&scenario_path(arg))?;
        if scenario.package != package {
            return Err(format!(
                "scenario {} was written for package {}, and this run loads {package}",
                scenario.name, scenario.package,
            ));
        }
        if cfg!(debug_assertions) {
            return Err("--bench measures release builds only; run it with cargo run --release".into());
        }
        let output = crate::paths::asset_root()
            .join("target")
            .join("bench")
            .join(format!("{}.json", scenario.name));
        Ok(Some(Self { scenario, output }))
    }
}

/// The file `--bench <arg>` names.
fn scenario_path(arg: &str) -> PathBuf {
    if arg.ends_with(".ron") {
        PathBuf::from(arg)
    } else {
        crate::paths::asset_root().join("bench").join(format!("{arg}.ron"))
    }
}

/// Empty the benchmark's save directory, so the run starts from freshly
/// generated terrain.
pub fn clear_save() -> Result<(), String> {
    let dir = crate::paths::asset_root().join("saves").join(SAVE_NAME);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", dir.display())),
    }
}

/// The parameters a benchmark runs with: the engine's defaults whatever
/// presets are saved, so a tuned preset can't change what is measured, with the
/// mesh disk cache bypassed, so every run meshes its chunks rather than reading
/// whatever an earlier session left in the cache.
pub fn params() -> EngineParams {
    let mut params = EngineParams::default();
    params.mesh_cache.enabled = false;
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn no_bench_flag_is_no_benchmark() {
        assert!(BenchSetup::from_args(&args("--gpu low-power"), "sandbox").unwrap().is_none());
    }

    #[test]
    fn a_bench_flag_without_a_scenario_is_refused() {
        assert!(BenchSetup::from_args(&args("--bench"), "sandbox").is_err());
        assert!(BenchSetup::from_args(&args("--bench --gpu low-power"), "sandbox").is_err());
    }

    #[test]
    fn a_scenario_is_a_name_in_bench_or_a_path() {
        let root = crate::paths::asset_root();
        assert_eq!(scenario_path("s1-rest-launch"), root.join("bench").join("s1-rest-launch.ron"));
        assert_eq!(scenario_path("elsewhere/mine.ron"), PathBuf::from("elsewhere/mine.ron"));
    }

    #[test]
    fn a_scenario_for_another_package_is_refused() {
        let err = BenchSetup::from_args(&args("--bench s1-rest-launch"), "testbed").err().unwrap();
        assert!(err.contains("written for package sandbox"), "{err}");
    }

    // Tests build with debug assertions, so this is the refusal they reach.
    #[cfg(debug_assertions)]
    #[test]
    fn a_debug_build_refuses_to_benchmark() {
        let err = BenchSetup::from_args(&args("--bench s1-rest-launch"), "sandbox").err().unwrap();
        assert!(err.contains("release builds only"), "{err}");
    }

    #[test]
    fn a_benchmark_bypasses_the_mesh_cache_and_ignores_presets() {
        let params = params();
        assert!(!params.mesh_cache.enabled);
        assert_eq!(params.camera, EngineParams::default().camera);
    }
}
