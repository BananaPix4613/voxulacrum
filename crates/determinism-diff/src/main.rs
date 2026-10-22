//! `cargo run -p determinism-diff -- <a.json> <b.json>`: compares two
//! determinism manifests. Exits 0 when they are identical, 1 when they differ,
//! and 2 when they cannot be compared.

use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [a, b] = args.as_slice() else {
        eprintln!("usage: determinism-diff <a.json> <b.json>");
        return ExitCode::from(2);
    };
    let (ma, mb) = match (read(Path::new(a)), read(Path::new(b))) {
        (Ok(ma), Ok(mb)) => (ma, mb),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("determinism-diff: {e}");
            return ExitCode::from(2);
        }
    };
    let (header, reports) = match determinism_diff::compare(&ma, &mb) {
        Ok(compared) => compared,
        Err(e) => {
            eprintln!("determinism-diff: cannot compare: {e}");
            return ExitCode::from(2);
        }
    };

    println!(
        "determinism-diff: {} against {} ({}, seed {})",
        header.platforms[0], header.platforms[1], header.namespace, header.seed,
    );
    let mut differing = 0usize;
    for report in &reports {
        match &report.first {
            None => println!("  {}: identical ({} records)", report.name, report.records),
            Some(first) => {
                differing += 1;
                println!(
                    "  {}: {} of {} records differ; first: {first}",
                    report.name, report.differing, report.records,
                );
            }
        }
    }
    if differing > 0 {
        eprintln!("determinism-diff: FAILED - {differing} of {} sections differ", reports.len());
        return ExitCode::FAILURE;
    }
    println!("determinism-diff: identical");
    ExitCode::SUCCESS
}

fn read(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}
