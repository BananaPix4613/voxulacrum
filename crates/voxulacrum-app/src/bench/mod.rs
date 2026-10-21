//! Scripted benchmarks (`--bench`): a scenario file drives the camera and
//! input through a fixed sequence, and the run records the CPU work of every
//! frame in its measured window. The procedure, and the readings taken with
//! it, are in `docs/perf-baseline.md`.

pub mod scenario;
pub mod stats;
mod process_cpu;

pub use process_cpu::process_cpu_seconds;
