//! Packages: the unit content loads as (roadmap M2).
//!
//! A package is a directory with a `package.ron` manifest at its root and its
//! content laid out beside it in the fixed content layout (`materials.ron`,
//! `prefabs.ron`, `foliage.ron`, `graphs/`, `libraries/`, `blueprints/`,
//! `species/`). The manifest names the package's id, its save directory (the id
//! unless it names another), its content namespace, its version, the engine
//! versions it supports and the world it starts.
//!
//! This loader takes data-only packages: one that declares code modules is
//! refused until the WebAssembly runtime exists (M11). Dependencies are
//! declared and validated but not loaded yet.

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
    /// Packages this one depends on (not loaded yet).
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
}
