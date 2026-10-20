//! Packages: the unit content loads as (roadmap M2).
//!
//! A package is a directory with a `package.ron` manifest at its root and its
//! content laid out beside it in the fixed content layout (`materials.ron`,
//! `prefabs.ron`, `foliage.ron`, `play.ron`, `graphs/`, `libraries/`,
//! `blueprints/`, `species/`). The manifest names the package's id, its save
//! directory (the id unless it names another), its content namespace, its
//! version, the engine versions it supports and the world it starts.
//!
//! This loader takes data-only packages: one that declares code modules is
//! refused until the WebAssembly runtime exists (M11). A package's
//! dependencies are found in `packages/<id>/` and ordered ahead of it
//! ([`Package::load_order`]). The engine package, `voxulacrum`, is built in.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use semver::{Version, VersionReq};
use serde::Deserialize;
use voxel_core::{QualifiedName, ENGINE_NAMESPACE};

/// File name of a package's manifest, at the package root.
pub const MANIFEST_FILE: &str = "package.ron";

/// What a package is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum PackageKind {
    /// A game: content a world is started from.
    Game,
    /// A mod: content added to a game.
    Mod,
}

/// A declared dependency: another package's id and the versions it accepts.
#[derive(Clone, Debug, PartialEq)]
pub struct Dependency {
    /// The other package's id.
    pub id: String,
    /// The versions of it this package accepts.
    pub version: VersionReq,
}

/// A validated package manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct PackageManifest {
    /// Identity, and the package's save directory under `saves/`.
    pub id: String,
    /// The package's directory under `saves/`: the id unless the manifest names
    /// another, so a package can keep a save directory it predates.
    pub saves: String,
    /// The namespace bare names in this package's content resolve in.
    pub namespace: String,
    /// The package's own version.
    pub version: Version,
    /// What the package is for.
    pub kind: PackageKind,
    /// The engine versions this package supports.
    pub engine: VersionReq,
    /// The world manifest this package starts, relative to its root.
    pub world: Option<PathBuf>,
    /// Packages this one depends on, which load before it.
    pub dependencies: Vec<Dependency>,
}

/// A package loaded from disk: its validated manifest and its directory.
#[derive(Clone, Debug)]
pub struct Package {
    /// The package's directory: the content root.
    pub root: PathBuf,
    /// The validated manifest.
    pub manifest: PackageManifest,
}

/// Files only the package a world starts from may hold: the tables whose ids
/// saves store, which two packages cannot share until saves carry a registry
/// mapping (D3, M4).
const ROOT_ONLY: [&str; 3] = ["materials.ron", "prefabs.ron", "foliage.ron"];

impl Package {
    /// Load and validate the package at `root` for this engine build.
    pub fn load(root: &Path) -> Result<Self, String> {
        Self::load_for(root, &engine_version())
    }

    fn load_for(root: &Path, engine: &Version) -> Result<Self, String> {
        let path = root.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let manifest = PackageManifest::parse(&text, engine)?;
        let package = Self { root: root.to_path_buf(), manifest };
        if let Some(world) = package.world_manifest() {
            if !world.is_file() {
                return Err(format!("world {} does not exist", world.display()));
            }
        }
        Ok(package)
    }

    /// Every package a world started from `root` loads, in load order: each
    /// dependency before the packages that need it, ties broken by id, and
    /// `root` last. A dependency is found at `<packages_dir>/<id>/`. The engine
    /// package is built in, so depending on `voxulacrum` checks this engine's
    /// version and loads nothing.
    pub fn load_order(root: Package, packages_dir: &Path) -> Result<Vec<Package>, String> {
        Self::load_order_for(root, packages_dir, &engine_version())
    }

    fn load_order_for(root: Package, packages_dir: &Path, engine: &Version) -> Result<Vec<Package>, String> {
        // Load every package reachable from `root`, checking each requirement
        // against the version that loaded.
        let mut loaded: BTreeMap<String, Package> = BTreeMap::new();
        let mut pending = vec![root.manifest.id.clone()];
        loaded.insert(root.manifest.id.clone(), root);
        while let Some(id) = pending.pop() {
            let dependencies = loaded[&id].manifest.dependencies.clone();
            for d in dependencies {
                if d.id == ENGINE_NAMESPACE {
                    if !d.version.matches(engine) {
                        return Err(format!(
                            "{id} needs engine package {} {}, but this engine is {engine}",
                            d.id, d.version
                        ));
                    }
                    continue;
                }
                if !loaded.contains_key(&d.id) {
                    let dependency = Self::load_dependency(&d.id, packages_dir, engine)
                        .map_err(|e| format!("{id} depends on {}: {e}", d.id))?;
                    loaded.insert(d.id.clone(), dependency);
                    pending.push(d.id.clone());
                }
                let version = &loaded[&d.id].manifest.version;
                if !d.version.matches(version) {
                    return Err(format!("{id} needs {} {}, but {} is {version}", d.id, d.version, d.id));
                }
            }
        }

        // Order them: a package is placed once everything it depends on is,
        // and of the packages ready to place, the lowest id goes first.
        let mut waiting: BTreeMap<String, BTreeSet<String>> = loaded
            .iter()
            .map(|(id, p)| {
                let on = p.manifest.dependencies.iter().filter(|d| d.id != ENGINE_NAMESPACE);
                (id.clone(), on.map(|d| d.id.clone()).collect())
            })
            .collect();
        let mut order = Vec::with_capacity(loaded.len());
        while let Some(id) = waiting.iter().find(|(_, on)| on.is_empty()).map(|(id, _)| id.clone()) {
            waiting.remove(&id);
            for on in waiting.values_mut() {
                on.remove(&id);
            }
            order.push(loaded.remove(&id).expect("every waiting package is loaded"));
        }
        if !waiting.is_empty() {
            let ids: Vec<&str> = waiting.keys().map(String::as_str).collect();
            return Err(format!("dependencies form a cycle among {}", ids.join(", ")));
        }
        // Each package's names are known by its namespace, so two may not share one.
        for (i, p) in order.iter().enumerate() {
            let earlier = order[..i].iter().find(|q| q.manifest.namespace == p.manifest.namespace);
            if let Some(q) = earlier {
                return Err(format!(
                    "{} and {} both use namespace `{}`",
                    q.manifest.id, p.manifest.id, p.manifest.namespace
                ));
            }
        }
        Ok(order)
    }

    /// Load the dependency `id` from `<packages_dir>/<id>/`, refusing a
    /// directory whose manifest names another id, or a package holding files
    /// only a world's own package may.
    fn load_dependency(id: &str, packages_dir: &Path, engine: &Version) -> Result<Self, String> {
        let root = packages_dir.join(id);
        if !root.join(MANIFEST_FILE).is_file() {
            return Err(format!("no package at {}", root.display()));
        }
        let package = Self::load_for(&root, engine)?;
        if package.manifest.id != id {
            return Err(format!("the package at {} is {:?}", root.display(), package.manifest.id));
        }
        if let Some(file) = ROOT_ONLY.iter().find(|f| root.join(f).exists()) {
            return Err(format!(
                "it ships {file}, which only the package a world starts from may until saves \
                 carry a registry mapping (D3, M4)"
            ));
        }
        Ok(package)
    }

    /// The world manifest this package declares, as a full path.
    pub fn world_manifest(&self) -> Option<PathBuf> {
        self.manifest.world.as_ref().map(|w| self.root.join(w))
    }
}

/// This build's engine version, which a package's `engine` must match.
pub fn engine_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the workspace version is semver")
}

impl PackageManifest {
    /// Parse and validate a manifest for an engine at version `engine`.
    fn parse(text: &str, engine: &Version) -> Result<Self, String> {
        let file: ManifestRon = ron::from_str(text).map_err(|e| e.to_string())?;

        QualifiedName::validate_namespace(&file.id).map_err(|e| format!("id: {e}"))?;
        let namespace = if file.namespace.is_empty() { file.id.clone() } else { file.namespace };
        QualifiedName::validate_namespace(&namespace).map_err(|e| format!("namespace: {e}"))?;
        for (what, value) in [("id", &file.id), ("namespace", &namespace)] {
            if value == ENGINE_NAMESPACE {
                return Err(format!("{what}: `{ENGINE_NAMESPACE}` is the engine package's"));
            }
        }
        // The namespace rule also keeps it one directory name under `saves/`.
        let saves = if file.saves.is_empty() { file.id.clone() } else { file.saves };
        QualifiedName::validate_namespace(&saves).map_err(|e| format!("saves: {e}"))?;

        let version = Version::parse(&file.version)
            .map_err(|e| format!("version {:?}: {e}", file.version))?;
        let engine_req = VersionReq::parse(&file.engine)
            .map_err(|e| format!("engine {:?}: {e}", file.engine))?;
        if !engine_req.matches(engine) {
            return Err(format!("needs engine {engine_req}, but this engine is {engine}"));
        }

        if !file.modules.is_empty() {
            return Err(format!(
                "declares {} code module(s); only data-only packages load until the \
                 WebAssembly runtime (M11)",
                file.modules.len()
            ));
        }

        let world = if file.world.is_empty() { None } else { Some(PathBuf::from(&file.world)) };
        if let Some(w) = &world {
            let escapes = w.is_absolute() || w.components().any(|c| matches!(c, Component::ParentDir));
            if escapes {
                return Err(format!("world {:?} must be a path inside the package", file.world));
            }
        }

        let mut dependencies: Vec<Dependency> = Vec::with_capacity(file.dependencies.len());
        for d in file.dependencies {
            QualifiedName::validate_namespace(&d.id).map_err(|e| format!("dependency id: {e}"))?;
            if d.id == file.id {
                return Err("depends on itself".to_string());
            }
            if dependencies.iter().any(|x| x.id == d.id) {
                return Err(format!("depends on {:?} twice", d.id));
            }
            let req = VersionReq::parse(&d.version)
                .map_err(|e| format!("dependency {} version {:?}: {e}", d.id, d.version))?;
            dependencies.push(Dependency { id: d.id, version: req });
        }

        Ok(Self { id: file.id, saves, namespace, version, kind: file.kind, engine: engine_req, world, dependencies })
    }
}

/// On-disk schema of `package.ron`. Optional strings are empty when absent,
/// so authors write `namespace: "x"` rather than RON's `Some("x")`.
#[derive(Deserialize)]
struct ManifestRon {
    id: String,
    #[serde(default)]
    saves: String,
    #[serde(default)]
    namespace: String,
    version: String,
    kind: PackageKind,
    engine: String,
    #[serde(default)]
    world: String,
    #[serde(default)]
    dependencies: Vec<DependencyRon>,
    #[serde(default)]
    modules: Vec<String>,
}

/// On-disk schema of one dependency.
#[derive(Deserialize)]
struct DependencyRon {
    id: String,
    version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Version {
        Version::parse("0.4.2").unwrap()
    }

    fn manifest(body: &str) -> String {
        format!(r#"( id: "demo", version: "0.1.0", kind: Game, engine: "^0.4", {body} )"#)
    }

    fn parse(body: &str) -> Result<PackageManifest, String> {
        PackageManifest::parse(&manifest(body), &engine())
    }

    #[test]
    fn a_minimal_manifest_parses_and_the_namespace_defaults_to_the_id() {
        let m = parse("").unwrap();
        assert_eq!((m.id.as_str(), m.namespace.as_str()), ("demo", "demo"));
        assert_eq!(m.saves, "demo", "the save directory defaults to the id");
        assert_eq!(m.kind, PackageKind::Game);
        assert_eq!(m.world, None);
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn an_explicit_namespace_and_world_are_kept() {
        let m = parse(r#"namespace: "demo_content", world: "graphs/world.manifest.json","#).unwrap();
        assert_eq!(m.namespace, "demo_content");
        assert_eq!(m.world, Some(PathBuf::from("graphs/world.manifest.json")));
    }

    #[test]
    fn an_explicit_save_directory_is_kept() {
        let m = parse(r#"saves: "default","#).unwrap();
        assert_eq!((m.id.as_str(), m.saves.as_str()), ("demo", "default"));
    }

    #[test]
    fn a_save_directory_that_is_not_one_directory_name_is_refused() {
        let err = parse(r#"saves: "../elsewhere","#).unwrap_err();
        assert!(err.starts_with("saves:"), "{err}");
    }

    #[test]
    fn a_package_for_another_engine_is_refused() {
        let text = r#"( id: "demo", version: "0.1.0", kind: Game, engine: "^0.5" )"#;
        let err = PackageManifest::parse(text, &engine()).unwrap_err();
        assert!(err.contains("needs engine ^0.5, but this engine is 0.4.2"), "{err}");
    }

    #[test]
    fn code_modules_are_refused() {
        let err = parse(r#"modules: ["logic.wasm"],"#).unwrap_err();
        assert!(err.contains("data-only"), "{err}");
    }

    #[test]
    fn an_id_that_is_not_a_namespace_is_refused() {
        let text = r#"( id: "My Game", version: "0.1.0", kind: Game, engine: "^0.4" )"#;
        let err = PackageManifest::parse(text, &engine()).unwrap_err();
        assert!(err.starts_with("id:"), "{err}");
    }

    #[test]
    fn the_engine_package_id_and_namespace_are_refused() {
        let text = r#"( id: "voxulacrum", version: "0.1.0", kind: Game, engine: "^0.4" )"#;
        let err = PackageManifest::parse(text, &engine()).unwrap_err();
        assert!(err.starts_with("id:"), "{err}");
        let err = parse(r#"namespace: "voxulacrum","#).unwrap_err();
        assert!(err.starts_with("namespace:"), "{err}");
    }

    #[test]
    fn a_version_that_is_not_semver_is_refused() {
        let text = r#"( id: "demo", version: "one", kind: Game, engine: "^0.4" )"#;
        let err = PackageManifest::parse(text, &engine()).unwrap_err();
        assert!(err.contains("version \"one\""), "{err}");
    }

    #[test]
    fn a_world_outside_the_package_is_refused() {
        let err = parse(r#"world: "../other/world.manifest.json","#).unwrap_err();
        assert!(err.contains("inside the package"), "{err}");
    }

    #[test]
    fn dependencies_are_validated() {
        let m = parse(r#"dependencies: [(id: "voxulacrum", version: "^0.4")],"#).unwrap();
        assert_eq!(m.dependencies[0].id, "voxulacrum");
        assert!(parse(r#"dependencies: [(id: "demo", version: "^0.1")],"#)
            .unwrap_err()
            .contains("itself"));
        assert!(parse(r#"dependencies: [(id: "a", version: "1"), (id: "a", version: "2")],"#)
            .unwrap_err()
            .contains("twice"));
    }

    #[test]
    fn a_missing_world_is_refused_at_load() {
        let root = std::env::temp_dir().join(format!("voxulacrum-package-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(MANIFEST_FILE), manifest(r#"world: "graphs/world.manifest.json","#))
            .unwrap();
        let err = Package::load_for(&root, &engine()).unwrap_err();
        let _ = std::fs::remove_dir_all(&root);
        assert!(err.contains("does not exist"), "{err}");
    }

    /// A scratch directory for one test, emptied first.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("voxulacrum-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write package `id` at `version` into `dir`, with `dependencies` as the
    /// body of its RON list, and return its root.
    fn write_package(dir: &Path, id: &str, version: &str, dependencies: &str) -> PathBuf {
        let root = dir.join(id);
        std::fs::create_dir_all(&root).unwrap();
        let text = format!(
            r#"( id: "{id}", version: "{version}", kind: Mod, engine: "^0.4", dependencies: [{dependencies}] )"#
        );
        std::fs::write(root.join(MANIFEST_FILE), text).unwrap();
        root
    }

    /// The ids `root` in `dir` loads, in load order.
    fn order(dir: &Path, root: &str) -> Result<Vec<String>, String> {
        let root = Package::load_for(&dir.join(root), &engine())?;
        let order = Package::load_order_for(root, dir, &engine())?;
        Ok(order.into_iter().map(|p| p.manifest.id).collect())
    }

    #[test]
    fn dependencies_load_before_the_packages_that_need_them_then_by_id() {
        let dir = scratch("load-order");
        write_package(&dir, "base", "1.0.0", "");
        write_package(&dir, "zeta", "1.0.0", r#"(id: "base", version: "^1")"#);
        write_package(&dir, "alpha", "1.0.0", "");
        let deps = r#"(id: "zeta", version: "^1"), (id: "alpha", version: "^1"), (id: "base", version: "^1")"#;
        write_package(&dir, "game", "0.1.0", deps);
        let ids = order(&dir, "game");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(ids.unwrap(), ["alpha", "base", "zeta", "game"]);
    }

    #[test]
    fn the_engine_package_is_built_in() {
        let dir = scratch("engine-dependency");
        write_package(&dir, "game", "0.1.0", r#"(id: "voxulacrum", version: "^0.4")"#);
        write_package(&dir, "old", "0.1.0", r#"(id: "voxulacrum", version: "^0.3")"#);
        let (game, old) = (order(&dir, "game"), order(&dir, "old"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(game.unwrap(), ["game"], "the engine package loads nothing from disk");
        let err = old.unwrap_err();
        assert!(err.contains("old needs engine package voxulacrum ^0.3"), "{err}");
    }

    #[test]
    fn a_missing_dependency_is_refused() {
        let dir = scratch("missing-dependency");
        write_package(&dir, "game", "0.1.0", r#"(id: "absent", version: "^1")"#);
        let err = order(&dir, "game").unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("game depends on absent: no package at"), "{err}");
    }

    #[test]
    fn a_dependency_cycle_is_refused() {
        let dir = scratch("dependency-cycle");
        write_package(&dir, "a", "1.0.0", r#"(id: "b", version: "^1")"#);
        write_package(&dir, "b", "1.0.0", r#"(id: "a", version: "^1")"#);
        write_package(&dir, "game", "0.1.0", r#"(id: "a", version: "^1")"#);
        let err = order(&dir, "game").unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("cycle among a, b, game"), "{err}");
    }

    #[test]
    fn a_dependency_of_the_wrong_version_is_refused() {
        let dir = scratch("dependency-version");
        write_package(&dir, "base", "2.0.0", "");
        write_package(&dir, "game", "0.1.0", r#"(id: "base", version: "^1")"#);
        let err = order(&dir, "game").unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("game needs base ^1, but base is 2.0.0"), "{err}");
    }

    #[test]
    fn a_dependency_directory_holding_another_package_is_refused() {
        let dir = scratch("dependency-id");
        let other = write_package(&dir, "other", "1.0.0", "");
        std::fs::rename(&other, dir.join("base")).unwrap();
        write_package(&dir, "game", "0.1.0", r#"(id: "base", version: "^1")"#);
        let err = order(&dir, "game").unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains(r#"is "other""#), "{err}");
    }

    #[test]
    fn a_dependency_that_ships_a_saved_table_is_refused() {
        let dir = scratch("dependency-table");
        let base = write_package(&dir, "base", "1.0.0", "");
        std::fs::write(base.join("materials.ron"), "").unwrap();
        write_package(&dir, "game", "0.1.0", r#"(id: "base", version: "^1")"#);
        let err = order(&dir, "game").unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("ships materials.ron"), "{err}");
    }

    #[test]
    fn two_packages_sharing_a_namespace_are_refused() {
        let dir = scratch("shared-namespace");
        let base = write_package(&dir, "base", "1.0.0", "");
        let text = r#"( id: "base", namespace: "game", version: "1.0.0", kind: Mod, engine: "^0.4" )"#;
        std::fs::write(base.join(MANIFEST_FILE), text).unwrap();
        write_package(&dir, "game", "0.1.0", r#"(id: "base", version: "^1")"#);
        let err = order(&dir, "game").unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("base and game both use namespace `game`"), "{err}");
    }
}
