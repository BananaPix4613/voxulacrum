//! Asset path resolution, anchored to the workspace root at compile time so
//! paths work regardless of the process working directory (cargo run, a
//! release exe launched from any directory, IDE launch, etc.).

use std::path::PathBuf;
use std::sync::OnceLock;

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

/// Where world content comes from. Set once by `app::run_with`, before anything
/// reads content; unset means the shipped reference world.
struct Content {
    name: String,
    root: PathBuf,
}

static CONTENT: OnceLock<Content> = OnceLock::new();

/// Point content lookups at `root` and this world's saves at `saves/<name>`.
/// Called once, at startup, before any content is read.
pub fn set_content(name: String, root: PathBuf) {
    if CONTENT.set(Content { name, root }).is_err() {
        panic!("the content root is set once, at startup");
    }
}

/// Directory holding world content: graphs, materials, prefabs, libraries,
/// blueprints and species. `assets/` under the asset root unless
/// `RunOptions::content` points elsewhere. Engine-owned files (shaders,
/// palettes, input bindings, presets) stay under the asset root regardless.
pub fn content_root() -> PathBuf {
    match CONTENT.get() {
        Some(content) => content.root.clone(),
        None => asset_root().join("assets"),
    }
}

/// Directory name under `saves/` for this world. It follows the content root,
/// so a second world never opens the reference world's save.
pub fn save_name() -> &'static str {
    CONTENT.get().map_or("default", |content| content.name.as_str())
}

/// Namespace this world's registry entries are known by, and the one bare names
/// in its content resolve in: the engine's own for the reference world, the
/// content name otherwise.
pub fn namespace() -> &'static str {
    CONTENT.get().map_or(voxel_core::ENGINE_NAMESPACE, |content| content.name.as_str())
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

    // No test calls `set_content`: the setting is process-wide, and the other
    // tests read the reference world through these same paths.
    #[test]
    fn unset_content_is_the_reference_world() {
        assert_eq!(content_root(), asset_root().join("assets"));
        assert_eq!(save_name(), "default");
        assert_eq!(namespace(), voxel_core::ENGINE_NAMESPACE);
    }
}
