//! Asset path resolution, anchored to the workspace root at compile time so
//! paths work regardless of the process working directory (cargo run, a
//! release exe launched from any directory, IDE launch, etc.).

use std::path::PathBuf;

/// Workspace root. `CARGO_MANIFEST_DIR` is `<workspace>/crates/voxulacrum-app`
/// at compile time; the root is two levels up.
pub fn asset_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}
