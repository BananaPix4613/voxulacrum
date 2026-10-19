//! Asset path resolution, anchored to the workspace root at compile time so
//! paths work regardless of the process working directory (cargo run, a
//! release exe launched from any directory, IDE launch, etc.).

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::package::Package;

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

/// The package this run's world comes from. Set once by `app::run_with`, before
/// anything reads content; unset means the shipped reference world.
static PACKAGE: OnceLock<Package> = OnceLock::new();

/// Load content from `package`, and keep this world's saves at
/// `saves/<package id>`. Called once, at startup, before any content is read.
pub fn set_package(package: Package) {
    if PACKAGE.set(package).is_err() {
        panic!("the package is set once, at startup");
    }
}

/// Directory holding world content: graphs, materials, prefabs, libraries,
/// blueprints, species and foliage. The package's root, or `assets/` under the
/// asset root for the reference world. Engine-owned files (shaders, palettes,
/// input bindings, presets) stay under the asset root regardless.
pub fn content_root() -> PathBuf {
    match PACKAGE.get() {
        Some(package) => package.root.clone(),
        None => asset_root().join("assets"),
    }
}

/// Directory name under `saves/` for this world: the package's id, so a second
/// world never opens the reference world's save.
pub fn save_name() -> &'static str {
    PACKAGE.get().map_or("default", |package| package.manifest.id.as_str())
}

/// Namespace this world's registry entries are known by, and the one bare names
/// in its content resolve in: the engine's own for the reference world, the
/// package's content namespace otherwise.
pub fn namespace() -> &'static str {
    PACKAGE
        .get()
        .map_or(voxel_core::ENGINE_NAMESPACE, |package| package.manifest.namespace.as_str())
}

/// The world manifest this run starts: the package's `world`, or the reference
/// world's `graphs/world.manifest.json`.
pub fn world_manifest_path() -> PathBuf {
    PACKAGE
        .get()
        .and_then(Package::world_manifest)
        .unwrap_or_else(|| content_root().join("graphs").join("world.manifest.json"))
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

    // No test calls `set_package`: the setting is process-wide, and the other
    // tests read the reference world through these same paths.
    #[test]
    fn no_package_is_the_reference_world() {
        assert_eq!(content_root(), asset_root().join("assets"));
        assert_eq!(save_name(), "default");
        assert_eq!(namespace(), voxel_core::ENGINE_NAMESPACE);
        assert_eq!(world_manifest_path(), content_root().join("graphs").join("world.manifest.json"));
    }
}
