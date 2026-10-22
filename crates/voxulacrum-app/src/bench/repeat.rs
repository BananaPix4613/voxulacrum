//! Repeated benchmark runs (`--runs <n>`): the scenario runs `n` times, each in
//! its own process so no run inherits another's caches, threads or heap, with a
//! pause between them so the machine starts each one at about the same
//! temperature. Then the median and spread of each headline figure are printed
//! and written beside the single-run results.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::results::{write_json, BenchResult};
use super::stats::Spread;
use super::BenchSetup;

/// Seconds between runs unless `--cooldown` says otherwise.
const DEFAULT_COOLDOWN: f32 = 30.0;

/// How a repeated benchmark runs, as the command line asks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Repeat {
    pub runs: u32,
    /// Seconds between one run ending and the next starting.
    pub cooldown: f32,
}

impl Repeat {
    /// What `--runs <n>` and `--cooldown <seconds>` ask for, or `None` without
    /// `--runs`.
    pub fn from_args(args: &[String]) -> Result<Option<Self>, String> {
        let cooldown = match value(args, "--cooldown")? {
            Some(text) => match text.parse::<f32>() {
                Ok(seconds) if seconds.is_finite() && seconds >= 0.0 => Some(seconds),
                _ => return Err(format!("--cooldown takes a number of seconds, not {text:?}")),
            },
            None => None,
        };
        let Some(text) = value(args, "--runs")? else {
            if cooldown.is_some() {
                return Err("--cooldown sets the pause between --runs, so it needs --runs".into());
            }
            return Ok(None);
        };
        let runs = match text.parse::<u32>() {
            Ok(n) if n > 0 => n,
            _ => return Err(format!("--runs takes a positive whole number, not {text:?}")),
        };
        Ok(Some(Self { runs, cooldown: cooldown.unwrap_or(DEFAULT_COOLDOWN) }))
    }
}

/// The value after `flag`, or `None` without the flag.
fn value<'a>(args: &'a [String], flag: &str) -> Result<Option<&'a str>, String> {
    let Some(i) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    match args.get(i + 1).filter(|a| !a.starts_with("--")) {
        Some(v) => Ok(Some(v)),
        None => Err(format!("{flag} takes a value")),
    }
}

/// A child's command line: the parent's, less `--runs` and `--cooldown` and
/// their values, so each child is one ordinary `--bench` run.
fn child_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--runs" || args[i] == "--cooldown" {
            i += 2;
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

/// One headline figure across the runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Figure {
    pub name: String,
    /// One per run, in order; `None` where that run had no value.
    pub values: Vec<Option<f64>>,
    pub spread: Spread,
}

/// Every run of a repeated benchmark, and its headline figures across them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunSet {
    pub scenario: String,
    pub figures: Vec<Figure>,
    pub runs: Vec<BenchResult>,
}

/// The figures a repeated run summarizes, and how to read each from one run's
/// results: `None` where a run has no value, such as a job mean for a run that
/// finished no jobs of that kind.
const FIGURES: [(&str, fn(&BenchResult) -> Option<f64>); 7] = [
    ("frame CPU mean ms", |r| Some(f64::from(r.frame_cpu_ms.mean))),
    ("frame CPU p95 ms", |r| Some(f64::from(r.frame_cpu_ms.p95))),
    ("frame CPU max ms", |r| Some(f64::from(r.frame_cpu_ms.max))),
    ("process CPU s/s", |r| r.process_cpu_per_second),
    ("generate mean ms", |r| (r.generate.completed > 0).then_some(f64::from(r.generate.mean_ms))),
    ("mesh mean ms", |r| (r.mesh.completed > 0).then_some(f64::from(r.mesh.mean_ms))),
    ("fill mean ms", fill_mean),
];

/// The mean of a run's fills, in milliseconds; `None` with no fills.
fn fill_mean(result: &BenchResult) -> Option<f64> {
    if result.fills.is_empty() {
        return None;
    }
    let sum: f64 = result.fills.iter().map(|fill| f64::from(fill.ms)).sum();
    Some(sum / result.fills.len() as f64)
}

impl RunSet {
    /// Summarize `runs`, keeping each figure at least one run has.
    pub fn of(scenario: &str, runs: Vec<BenchResult>) -> Self {
        let figures = FIGURES
            .iter()
            .filter_map(|(name, read)| {
                let values: Vec<Option<f64>> = runs.iter().map(read).collect();
                let present: Vec<f64> = values.iter().flatten().copied().collect();
                (!present.is_empty()).then(|| Figure {
                    name: name.to_string(),
                    values,
                    spread: Spread::of(&present),
                })
            })
            .collect();
        Self { scenario: scenario.to_string(), figures, runs }
    }

    /// The figures as a table for the console: one row per figure, one column
    /// per run, then the median and the spread.
    pub fn table(&self) -> String {
        let mut out = format!("bench {}: {} runs\n{:<20}", self.scenario, self.runs.len(), "");
        for i in 1..=self.runs.len() {
            out.push_str(&format!("{:>9}", format!("run {i}")));
        }
        out.push_str(&format!("{:>9}{:>9}\n", "median", "spread"));
        for figure in &self.figures {
            out.push_str(&format!("{:<20}", figure.name));
            for value in &figure.values {
                match value {
                    Some(v) => out.push_str(&format!("{v:>9.2}")),
                    None => out.push_str(&format!("{:>9}", "-")),
                }
            }
            out.push_str(&format!("{:>9.2}{:>8.1}%\n", figure.spread.median, figure.spread.percent));
        }
        out
    }
}

/// Run the benchmark `repeat.runs` times, each as a child process with the same
/// command line less `--runs` and `--cooldown`, then print and write the
/// figures across them. Returns the process's exit code: 0 with every run done,
/// otherwise that of the first run that failed.
pub fn run(repeat: Repeat, args: &[String], setup: &BenchSetup) -> i32 {
    let name = &setup.scenario.name;
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("bench {name}: this executable's path could not be read: {e}");
            return 1;
        }
    };
    let child = child_args(args);
    let mut results = Vec::with_capacity(repeat.runs as usize);
    for i in 1..=repeat.runs {
        if i > 1 {
            println!("bench {name}: cooling down for {} s", repeat.cooldown);
            std::thread::sleep(Duration::from_secs_f32(repeat.cooldown));
        }
        println!("bench {name}: run {i} of {}", repeat.runs);
        // Removed first, so a run that exits without writing cannot pass off
        // the previous run's results as its own.
        if let Err(e) = remove_stale(&setup.output) {
            eprintln!("bench {name}: {e}");
            return 1;
        }
        let status = match Command::new(&exe).args(&child).status() {
            Ok(status) => status,
            Err(e) => {
                eprintln!("bench {name}: run {i} could not start: {e}");
                return 1;
            }
        };
        if !status.success() {
            eprintln!("bench {name}: run {i} failed ({status}); stopping");
            return status.code().unwrap_or(1);
        }
        match read_result(&setup.output) {
            Ok(result) => results.push(result),
            Err(e) => {
                eprintln!("bench {name}: run {i}'s results could not be read: {e}");
                return 1;
            }
        }
    }
    let set = RunSet::of(name, results);
    let path = setup.output.with_file_name(format!("{name}-runs.json"));
    print!("{}", set.table());
    match write_json(&set, &path) {
        Ok(()) => {
            println!("  results: {}", path.display());
            0
        }
        Err(e) => {
            eprintln!("bench {name}: results not written: {e}");
            1
        }
    }
}

fn remove_stale(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn read_result(path: &Path) -> Result<BenchResult, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::results::JobWindow;
    use crate::bench::stats::Summary;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn no_runs_flag_is_a_single_run() {
        assert_eq!(Repeat::from_args(&args("--bench s1-rest-launch")), Ok(None));
    }

    #[test]
    fn runs_pause_thirty_seconds_unless_told_otherwise() {
        assert_eq!(
            Repeat::from_args(&args("--bench s1-rest-launch --runs 5")),
            Ok(Some(Repeat { runs: 5, cooldown: 30.0 })),
        );
        assert_eq!(
            Repeat::from_args(&args("--runs 3 --cooldown 10")),
            Ok(Some(Repeat { runs: 3, cooldown: 10.0 })),
        );
    }

    #[test]
    fn a_run_count_that_is_not_a_positive_whole_number_is_refused() {
        for text in ["--runs 0", "--runs five", "--runs 2.5", "--runs", "--runs --cooldown 5"] {
            assert!(Repeat::from_args(&args(text)).is_err(), "{text}");
        }
    }

    #[test]
    fn a_cooldown_needs_runs_and_a_number_of_seconds_not_below_zero() {
        for text in ["--cooldown 10", "--runs 2 --cooldown -1", "--runs 2 --cooldown soon"] {
            assert!(Repeat::from_args(&args(text)).is_err(), "{text}");
        }
    }

    #[test]
    fn a_child_runs_once_with_everything_else_the_parent_was_given() {
        assert_eq!(
            child_args(&args("--bench s1-rest-launch --runs 5 --gpu low-power --cooldown 10")),
            args("--bench s1-rest-launch --gpu low-power"),
        );
    }

    #[test]
    fn a_run_set_summarizes_only_the_figures_its_runs_have() {
        let run = |mean: f32, generated: u64| BenchResult {
            frame_cpu_ms: Summary { mean, p95: mean, max: mean },
            generate: JobWindow { completed: generated, mean_ms: 8.0 },
            ..Default::default()
        };
        let set = RunSet::of("S1", vec![run(3.0, 0), run(5.0, 40), run(4.0, 0)]);
        let names: Vec<&str> = set.figures.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["frame CPU mean ms", "frame CPU p95 ms", "frame CPU max ms", "generate mean ms"]);
        assert_eq!(set.figures[0].spread.median, 4.0);
        assert_eq!(set.figures[3].values, vec![None, Some(8.0), None]);
        assert!(set.table().contains("median"));
    }
}
