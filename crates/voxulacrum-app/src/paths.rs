//! Asset path resolution, anchored to the workspace root at compile time so
//! paths work regardless of the process working directory (cargo run, a
//! release exe launched from any directory, IDE launch, etc.).

use std::path::PathBuf;

/// Workspace root. `CARGO_MANIFEST_DIR` is `<workspace>/crates/voxulacrum-app`
/// at compile time; the root is two levels up.
pub fn asset_root() -> PathBuf {
    // Distribution: assets/shaders/palettes ship next to the .exe, so resolve
    // relative to the executable. Development (`cargo run`): the exe lives in
    // target/<profile>/ with no assets alongside, so fall back to the compile-time
    // workspace root.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("shaders").is_dir() {
                return dir.to_path_buf();
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Directory holding saved engine-parameter presets; `default.json` in it is
/// loaded at startup. Anchored like every other asset path: a relative `params`
/// followed the working directory, so one install saw different presets
/// depending on how it was launched.
pub fn presets_dir() -> PathBuf {
    asset_root().join("params")
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn the_presets_directory_does_not_depend_on_the_working_directory() {
        let dir = presets_dir();
        assert!(dir.is_absolute(), "{dir:?} is relative, so it follows the working directory");
        assert!(dir.starts_with(asset_root()), "{dir:?} is not under the asset root");
    }
}
