//! Asset path resolution, anchored to the workspace root at compile time so
//! paths work regardless of the process working directory (cargo run, a
//! release exe launched from any directory, IDE launch, etc.).

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::package::Package;

/// Workspace root. `CARGO_MANIFEST_DIR` is `<workspace>/crates/voxulacrum-app`
/// at compile time; the root is two levels up.
pub fn asset_root() -> PathBuf {
    // Distribution: assets/packages/shaders/palettes ship next to the .exe, so resolve
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

/// The package this run loads, in load order, the world's own last. Set once
/// by `app::run_with`, before anything reads content. Code that reads content
/// without going through `run_with`, such as the tests, gets the default
/// package's, loaded on first use.
static PACKAGES: OnceLock<Vec<Package>> = OnceLock::new();

/// Directory dependencies are found in: one package per directory, named for
/// its id.
pub fn packages_dir() -> PathBuf {
    asset_root().join("packages")
}

/// Directory of the package the engine starts when no other is named: the
/// shipped sandbox world.
pub fn default_package_root() -> PathBuf {
    packages_dir().join("sandbox")
}

/// Load content from `packages`, in [`Package::load_world`]'s order, and keep
/// this world's saves in the last one's save directory. Called once, at
/// startup, before any content is read.
pub fn set_packages(packages: Vec<Package>) {
    if PACKAGES.set(packages).is_err() {
        panic!("the packages are set once, at startup");
    }
}

/// Every package this run loads, in load order, the world's own last. Loads
/// the default package and its dependencies if none was set.
pub fn packages() -> &'static [Package] {
    PACKAGES.get_or_init(|| {
        Package::load_world(&default_package_root(), &packages_dir())
            .unwrap_or_else(|e| panic!("the default package failed to load: {e}"))
    })
}

/// The package this run's world comes from.
fn package() -> &'static Package {
    packages().last().expect("the load order ends with the world's own package")
}

/// Directory holding world content: graphs, materials, prefabs, libraries,
/// blueprints, species and foliage. The package's root. Engine-owned files
/// (shaders, palettes, input bindings, presets) stay under the asset root.
pub fn content_root() -> PathBuf {
    package().root.clone()
}

/// Directory name under `saves/` for this world: the package's save directory
/// (its id unless the manifest names another), so two packages never share a
/// save.
pub fn save_name() -> &'static str {
    &package().manifest.saves
}

/// Namespace this world's registry entries are known by, and the one bare names
/// in its content resolve in: the package's content namespace.
pub fn namespace() -> &'static str {
    &package().manifest.namespace
}

/// The world manifest this run starts: the package's `world`.
pub fn world_manifest_path() -> PathBuf {
    package()
        .world_manifest()
        .expect("the package names a world: `run_with` refuses one that does not, and the default does")
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

    // No test calls `set_packages`: the setting is process-wide, and the other
    // tests read the sandbox package through these same paths.
    #[test]
    fn no_package_set_is_the_sandbox_package() {
        assert_eq!(content_root(), default_package_root());
        assert_eq!(save_name(), "default");
        assert_eq!(namespace(), "sandbox");
        assert_eq!(world_manifest_path(), content_root().join("graphs").join("world.manifest.json"));
    }
}
