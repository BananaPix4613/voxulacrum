//! `cargo run -p boundary-check`: reads this workspace's `cargo metadata` and
//! checks the engine/game boundary. Exits 1 on any violation.
//!
//! `--manifest-path <Cargo.toml>` checks another workspace instead, such as a
//! game's, against the engine crates listed by the workspace its `voxulacrum`
//! dependency belongs to. A game's CI runs it that way.

use std::ffi::OsString;
use std::process::{Command, ExitCode};

use boundary_check::Summary;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let checked = match args.as_slice() {
        [] => metadata(&["--no-deps".into()]).map(|ws| boundary_check::check(&ws)),
        [flag, path] if flag.to_str() == Some("--manifest-path") => check_outside(path),
        _ => Err("usage: boundary-check [--manifest-path <Cargo.toml>]".to_string()),
    };

    match checked {
        Err(e) => {
            eprintln!("boundary-check: {e}");
            ExitCode::FAILURE
        }
        Ok(Ok(summary)) => {
            let consumers = if summary.consumers.is_empty() {
                "0 consumers".to_string()
            } else {
                format!("{} consumers ({})", summary.consumers.len(), summary.consumers.join(", "))
            };
            println!("boundary-check: {} engine crates, {consumers}, ok", summary.engine_crates);
            ExitCode::SUCCESS
        }
        Ok(Err(errors)) => {
            for error in &errors {
                eprintln!("boundary-check: {error}");
            }
            eprintln!("boundary-check: {} problem(s); see crates/boundary-check", errors.len());
            ExitCode::FAILURE
        }
    }
}

/// Check the workspace at `manifest_path` against the engine workspace its
/// `voxulacrum` dependency belongs to.
fn check_outside(manifest_path: &OsString) -> Result<Result<Summary, Vec<String>>, String> {
    let workspace = metadata(&["--manifest-path".into(), manifest_path.clone()])?;
    let facade = workspace["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == boundary_check::FACADE)
        .and_then(|p| p["manifest_path"].as_str())
        .ok_or_else(|| {
            format!(
                "{} does not depend on `{}`",
                manifest_path.to_string_lossy(),
                boundary_check::FACADE
            )
        })?
        .to_string();
    let engine = metadata(&["--no-deps".into(), "--manifest-path".into(), facade.into()])?;
    Ok(boundary_check::check_consumers(&workspace, &engine))
}

/// `cargo metadata --format-version 1` with `args`, parsed.
fn metadata(args: &[OsString]) -> Result<serde_json::Value, String> {
    // Cargo sets `CARGO` for `cargo run`, so this uses the same toolchain.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1"])
        .args(args)
        .output()
        .map_err(|e| format!("could not run cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("cargo metadata returned invalid JSON: {e}"))
}
