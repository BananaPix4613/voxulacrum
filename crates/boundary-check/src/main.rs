//! `cargo run -p boundary-check`: reads this workspace's `cargo metadata` and
//! checks the engine/game boundary. Exits 1 on any violation.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    // Cargo sets `CARGO` for `cargo run`, so this uses the same toolchain.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = match Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            eprintln!(
                "boundary-check: cargo metadata failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("boundary-check: could not run cargo metadata: {e}");
            return ExitCode::FAILURE;
        }
    };
    let metadata: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(metadata) => metadata,
        Err(e) => {
            eprintln!("boundary-check: cargo metadata returned invalid JSON: {e}");
            return ExitCode::FAILURE;
        }
    };

    match boundary_check::check(&metadata) {
        Ok(summary) => {
            let consumers = if summary.consumers.is_empty() {
                "0 consumers".to_string()
            } else {
                format!("{} consumers ({})", summary.consumers.len(), summary.consumers.join(", "))
            };
            println!("boundary-check: {} engine crates, {consumers}, ok", summary.engine_crates);
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for error in &errors {
                eprintln!("boundary-check: {error}");
            }
            eprintln!("boundary-check: {} problem(s); see crates/boundary-check", errors.len());
            ExitCode::FAILURE
        }
    }
}
