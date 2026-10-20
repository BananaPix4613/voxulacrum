//! Library registry.
//!
//! A library binds a typed [`GraphBoundary`] (its named inputs and outputs) to
//! a native [`LibraryKernel`]. The engine package declares one library per
//! kernel, in namespace `voxulacrum`, so content names them as
//! `voxulacrum:<name>` and never repeats their declarations. Each loaded
//! package may also author `libraries/*.library.json`, named in its own
//! namespace; they load after the engine's, in load order. Loading builds a
//! [`LibraryGraphRegistry`] (so a `LibraryRef` node resolves its pins from a
//! library's boundary) alongside a per-id kernel map (so the computation can
//! be invoked where a library's output is consumed). Ids are assigned in load
//! order; nothing saved holds one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use nodegraph_eval::LibraryKernel;
use nodegraph_ir::{Graph, GraphBoundary, GraphKind, LibraryGraphId, LibraryGraphRegistry, NodeKind};
use serde::Deserialize;
use voxel_core::{QualifiedName, ENGINE_NAMESPACE};

/// One authored library asset (`*.library.json`).
#[derive(Deserialize)]
struct LibraryAsset {
    /// Order within its package (contiguous from 0), authoritative over file
    /// order. The loaded id follows every library loaded before the package's.
    id: u32,
    /// Stable string handle (used in diagnostics).
    id_name: String,
    /// Declared typed boundary (inputs/outputs).
    boundary: GraphBoundary,
    /// Native kernel this library binds to (see [`LibraryKernel::from_name`]).
    kernel: String,
}

/// The loaded standard libraries: a boundary registry for `LibraryRef` pin
/// resolution and a per-id native kernel binding for evaluation.
#[derive(Clone, Debug, Default)]
pub struct LoadedLibraries {
    registry: LibraryGraphRegistry,
    kernels: HashMap<LibraryGraphId, LibraryKernel>,
    /// `(id, name)` in id order, for menus and diagnostics. The registry carries
    /// boundaries and the kernel map carries computations; neither carries a
    /// name, and a menu of numeric ids is not a menu.
    entries: Vec<(LibraryGraphId, QualifiedName)>,
}

impl LoadedLibraries {
    /// The boundary registry, for resolving `LibraryRef` pins.
    pub fn registry(&self) -> &LibraryGraphRegistry {
        &self.registry
    }

    /// The native kernel bindings, for evaluating `LibraryRef` nodes.
    pub fn kernels(&self) -> &HashMap<LibraryGraphId, LibraryKernel> {
        &self.kernels
    }

    /// `(id, name)` for every loaded library, in id order.
    pub fn entries(&self) -> &[(LibraryGraphId, QualifiedName)] {
        &self.entries
    }
    
    /// The id of the library named `name`, if one is loaded.
    pub fn resolve(&self, name: &QualifiedName) -> Option<LibraryGraphId> {
        self.entries.iter().find(|(_, n)| n == name).map(|(id, _)| *id)
    }
    
    /// Set the id of every `LibraryRef` in `graph` from its name, read in
    /// `namespace`. Ids are always rewritten, like material ids. Every node is
    /// visited; the error lists each name that is malformed or not loaded.
    pub fn resolve_refs(&self, graph: &mut Graph, namespace: &str) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        for (_, node) in graph.nodes.iter_mut() {
            let NodeKind::LibraryRef(p) = &mut node.kind else { continue };
            match QualifiedName::parse(p.library.name(), namespace) {
                Ok(name) => match self.resolve(&name) {
                    Some(id) => p.library.set_id(id),
                    None => problems.push(format!("library {name} is not loaded")),
                },
                Err(e) => problems.push(format!("library {e}")),
            }
        }
        if problems.is_empty() { Ok(()) } else { Err(problems) }
    }

    /// Build from sets of parsed assets, each named in its own namespace. Ids
    /// run on from one set to the next, in the order given. Within a set,
    /// authored ids must be contiguous from 0 and names valid and unique. Each
    /// boundary is projected into a Library `Graph` (for pin resolution) and
    /// each kernel name resolved. Fails on a non-contiguous id, a bad or
    /// duplicate name, or an unknown kernel name.
    fn from_sets(sets: Vec<(&str, Vec<LibraryAsset>)>) -> Result<Self, String> {
        let mut registry = LibraryGraphRegistry::new();
        let mut kernels = HashMap::new();
        let mut entries: Vec<(LibraryGraphId, QualifiedName)> = Vec::new();
        for (namespace, mut assets) in sets {
            assets.sort_by_key(|a| a.id);
            let first = entries.len() as u32;
            for (i, a) in assets.into_iter().enumerate() {
                if a.id as usize != i {
                    return Err(format!(
                        "library ids must be contiguous from 0; expected {i}, found {}",
                        a.id
                    ));
                }
                let id = LibraryGraphId(first + a.id);
                let name = QualifiedName::new(namespace, &a.id_name)
                    .map_err(|e| format!("library id {}: {e}", a.id))?;
                if entries.iter().any(|(_, existing)| *existing == name) {
                    return Err(format!("duplicate library id_name: {:?}", a.id_name));
                }
                let kernel = LibraryKernel::from_name(&a.kernel).ok_or_else(|| {
                    format!("library {name} names unknown kernel '{}'", a.kernel)
                })?;
                let mut graph = Graph::of_kind(GraphKind::Library);
                graph.boundary = a.boundary;
                registry.insert(id, graph);
                kernels.insert(id, kernel);
                entries.push((id, name));
            }
        }
        Ok(Self { registry, kernels, entries })
    }

    /// The engine package's libraries alone.
    pub fn engine() -> Self {
        Self::from_sets(vec![(ENGINE_NAMESPACE, engine_assets())])
            .expect("the engine package's libraries are valid")
    }

    /// The engine package's libraries, then those authored in each of
    /// `packages`, a `(libraries directory, namespace)` per package in load
    /// order. A missing directory authors none.
    pub fn load(packages: &[(PathBuf, &str)]) -> Result<Self, String> {
        let mut sets = vec![(ENGINE_NAMESPACE, engine_assets())];
        for (dir, namespace) in packages {
            sets.push((*namespace, read_assets(dir)?));
        }
        Self::from_sets(sets)
    }
}

/// The engine package's libraries: one per [`LibraryKernel`], in
/// [`LibraryKernel::ALL`]'s order, named for its kernel and declaring the
/// boundary the kernel gives.
fn engine_assets() -> Vec<LibraryAsset> {
    LibraryKernel::ALL
        .iter()
        .enumerate()
        .map(|(i, k)| LibraryAsset {
            id: i as u32,
            id_name: k.name().into(),
            boundary: k.boundary(),
            kernel: k.name().into(),
        })
        .collect()
}

/// Every `*.library.json` in `dir`, ordered by file name for determinism. A
/// missing directory holds none: a package need not author libraries.
fn read_assets(dir: &Path) -> Result<Vec<LibraryAsset>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".library.json"))
        .collect();
    paths.sort();
    let mut assets = Vec::new();
    for path in paths {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let asset: LibraryAsset =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        assets.push(asset);
    }
    Ok(assets)
}

/// Load the library set: the engine package's, then each loaded package's, in
/// load order.
///
/// Never fails - an invalid asset directory is logged at error level and the
/// engine package's libraries are used alone.
pub fn load_libraries() -> Arc<LoadedLibraries> {
    let packages: Vec<(PathBuf, &str)> = crate::paths::packages()
        .iter()
        .map(|p| (p.root.join("libraries"), p.manifest.namespace.as_str()))
        .collect();
    match LoadedLibraries::load(&packages) {
        Ok(libs) => {
            log::info!(
                "Loaded {} libraries ({} kernels) from the engine and {} package(s)",
                libs.registry.len(),
                libs.kernels.len(),
                packages.len()
            );
            Arc::new(libs)
        }
        Err(e) => {
            log::error!("Failed to load libraries: {e}. Using the engine's alone.");
            Arc::new(LoadedLibraries::engine())
        }
    }
}

/// Shared, immutable library set held as an ECS resource.
#[derive(Resource, Clone)]
pub struct LibrariesRes(pub Arc<LoadedLibraries>);

impl std::ops::Deref for LibrariesRes {
    type Target = LoadedLibraries;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodegraph_ir::{LibraryGraphRef, LibraryRefParams, PinType};

    fn asset(id: u32, id_name: &str, kernel: &str) -> LibraryAsset {
        LibraryAsset {
            id,
            id_name: id_name.into(),
            boundary: GraphBoundary::default(),
            kernel: kernel.into(),
        }
    }

    #[test]
    fn loads_and_binds_kernel() {
        let libs = LoadedLibraries::engine();
        assert_eq!(libs.kernels.len(), 5);
        let id = LibraryGraphId(0);
        assert_eq!(libs.kernels.get(&id), Some(&LibraryKernel::BiomeBorderFade));
        // Boundary is registered so a LibraryRef can resolve its pins.
        let g = libs.registry.get(id).expect("library graph present");
        assert_eq!(g.boundary.inputs.len(), 2);
        assert_eq!(g.boundary.output("weight").unwrap().ty, PinType::SurfaceField);
        assert_eq!(libs.entries[0].1.to_string(), "voxulacrum:biome_border_fade");
    }

    #[test]
    fn the_engine_libraries_keep_their_ids() {
        let libs = LoadedLibraries::engine();
        let names: Vec<String> = libs.entries.iter().map(|(_, n)| n.to_string()).collect();
        assert_eq!(
            names,
            [
                "voxulacrum:biome_border_fade",
                "voxulacrum:standard_cave_noise",
                "voxulacrum:surface_layering",
                "voxulacrum:exposure_layering",
                "voxulacrum:poisson_placement",
            ]
        );
        for (i, (id, _)) in libs.entries.iter().enumerate() {
            assert_eq!(*id, LibraryGraphId(i as u32));
        }
    }

    #[test]
    fn library_ref_resolves_against_loaded_registry() {
        let libs = LoadedLibraries::engine();
        let mut g = Graph::new();
        let n = g.add_node(NodeKind::LibraryRef(LibraryRefParams {
            library: LibraryGraphRef::new("biome_border_fade"),
            resolved: None,
        }));
        libs.resolve_refs(&mut g, ENGINE_NAMESPACE).expect("the library is loaded");
        g.resolve_library_refs(&libs.registry);
        let inputs = g.nodes[n].kind.effective_inputs();
        let outputs = g.nodes[n].kind.effective_outputs();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].name, "distance");
        assert_eq!(inputs[1].name, "radius");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "weight");
    }

    #[test]
    fn a_library_name_resolves_to_its_id() {
        let libs = LoadedLibraries::engine();
        let mut g = Graph::new();
        let n = g.add_node(NodeKind::LibraryRef(LibraryRefParams {
            library: LibraryGraphRef::new("standard_cave_noise"),
            resolved: None,
        }));
        libs.resolve_refs(&mut g, ENGINE_NAMESPACE).unwrap();
        let NodeKind::LibraryRef(p) = &g.nodes[n].kind else { unreachable!() };
        assert_eq!(p.library.id(), Some(LibraryGraphId(1)));
    }

    #[test]
    fn package_content_names_engine_libraries_by_qualified_name() {
        let libs = LoadedLibraries::engine();
        let mut g = Graph::new();
        g.add_node(NodeKind::LibraryRef(LibraryRefParams {
            library: LibraryGraphRef::new("voxulacrum:standard_cave_noise"),
            resolved: None,
        }));
        libs.resolve_refs(&mut g, "demo").expect("a qualified name reaches the engine package");
        g.add_node(NodeKind::LibraryRef(LibraryRefParams {
            library: LibraryGraphRef::new("standard_cave_noise"),
            resolved: None,
        }));
        let problems = libs.resolve_refs(&mut g, "demo").unwrap_err();
        assert_eq!(problems, ["library demo:standard_cave_noise is not loaded"]);
    }

    #[test]
    fn a_package_library_loads_after_the_engine_libraries_in_its_own_namespace() {
        let libs = LoadedLibraries::from_sets(vec![
            (ENGINE_NAMESPACE, engine_assets()),
            ("demo", vec![asset(0, "poisson_placement", "poisson_placement")]),
        ])
            .unwrap();
        let own = QualifiedName::new("demo", "poisson_placement").unwrap();
        let engine = QualifiedName::new(ENGINE_NAMESPACE, "poisson_placement").unwrap();
        assert_eq!(libs.resolve(&own), Some(LibraryGraphId(5)));
        assert_eq!(libs.resolve(&engine), Some(LibraryGraphId(4)));
    }

    #[test]
    fn a_package_without_a_libraries_directory_has_the_engine_libraries() {
        let dir = std::env::temp_dir().join(format!("voxulacrum-no-libraries-{}", std::process::id()));
        let libs = LoadedLibraries::load(&[(dir, "demo")]).unwrap();
        assert_eq!(libs.entries.len(), LibraryKernel::ALL.len());
    }
    
    #[test]
    fn each_package_s_libraries_load_in_load_order_in_its_own_namespace() {
        let dir = std::env::temp_dir().join(format!("voxulacrum-package-libraries-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (package, name) in [("base", "fade"), ("game", "edge")] {
            let libraries = dir.join(package);
            std::fs::create_dir_all(&libraries).unwrap();
            let json = format!(
                r#"{{ "id": 0, "id_name": "{name}", "boundary": {{}}, "kernel": "biome_border_fade" }}"#
            );
            std::fs::write(libraries.join(format!("{name}.library.json")), json).unwrap();
        }
        let libs = LoadedLibraries::load(&[(dir.join("base"), "base"), (dir.join("game"), "game")]);
        let _ = std::fs::remove_dir_all(&dir);
        let libs = libs.unwrap();
        let loaded: Vec<(u32, String)> =
            libs.entries[5..].iter().map(|(id, n)| (id.0, n.to_string())).collect();
        assert_eq!(loaded, [(5, "base:fade".to_string()), (6, "game:edge".to_string())]);
    }

    #[test]
    fn non_contiguous_ids_rejected() {
        let assets = vec![asset(0, "a", "biome_border_fade"), asset(2, "b", "biome_border_fade")];
        let err = LoadedLibraries::from_sets(vec![("demo", assets)]).expect_err("must refuse");
        assert!(err.contains("contiguous"), "{err}");
    }

    #[test]
    fn unknown_kernel_rejected() {
        let assets = vec![asset(0, "x", "does_not_exist")];
        let err = LoadedLibraries::from_sets(vec![("demo", assets)]).expect_err("must refuse");
        assert!(err.contains("does_not_exist"), "{err}");
    }

    #[test]
    fn duplicate_names_rejected() {
        let assets = vec![asset(0, "a", "biome_border_fade"), asset(1, "a", "biome_border_fade")];
        let err = LoadedLibraries::from_sets(vec![("demo", assets)]).expect_err("must refuse");
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn a_name_that_is_not_a_name_is_rejected() {
        let assets = vec![asset(0, "Cave Noise", "standard_cave_noise")];
        let err = LoadedLibraries::from_sets(vec![("demo", assets)]).expect_err("must refuse");
        assert!(err.contains("Cave Noise"), "{err}");
    }
}
