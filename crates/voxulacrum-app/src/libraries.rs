//! Data-driven library registry (Phase 6 Substep 2).
//!
//! Standard libraries are authored as `assets/libraries/*.library.json`: each
//! declares a typed [`GraphBoundary`] (its named inputs/outputs) plus the native
//! `kernel` it binds to. Loading builds a [`LibraryGraphRegistry`] (so a
//! `LibraryRef` node resolves its pins from a library's boundary) alongside a
//! per-id [`LibraryKernel`] map (so the computation can be invoked where a
//! library's output is consumed - the density-blend pass, Substep 8). Directory
//! scan primary, hand-authored fallback so the engine never fails to start over
//! libraries. Mirrors the material / prefab registries.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use nodegraph_eval::LibraryKernel;
use nodegraph_ir::{
    BoundaryPort, Graph, GraphBoundary, GraphKind, LibraryGraphId, LibraryGraphRegistry, NodeKind,
    PinType,
};
use serde::Deserialize;
use voxel_core::QualifiedName;

/// One authored library asset (`*.library.json`).
#[derive(Deserialize)]
struct LibraryAsset {
    /// Stable numeric id (contiguous from 0), authoritative over file order.
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

    /// Build from parsed assets, naming them in `namespace`: validate contiguous
    /// ids and valid, unique names, project each boundary into a Library `Graph`
    /// (for pin resolution) and resolve its kernel name. Fails on an empty set,
    /// a non-contiguous id, a bad or duplicate name, or an unknown kernel name.
    fn from_assets(mut assets: Vec<LibraryAsset>, namespace: &str) -> Result<Self, String> {
        assets.sort_by_key(|a| a.id);
        if assets.is_empty() {
            return Err("library registry must have at least one entry".into());
        }
        let mut registry = LibraryGraphRegistry::new();
        let mut kernels = HashMap::new();
        let mut entries: Vec<(LibraryGraphId, QualifiedName)> = Vec::new();
        for (i, a) in assets.into_iter().enumerate() {
            if a.id as usize != i {
                return Err(format!(
                    "library ids must be contiguous from 0; expected {i}, found {}",
                    a.id
                ));
            }
            let id = LibraryGraphId(a.id);
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
        Ok(Self { registry, kernels, entries })
    }

    /// Load every `*.library.json` in `dir`, ordered by filename for
    /// determinism, naming them in `namespace`.
    pub fn load_from_dir(dir: &std::path::Path, namespace: &str) -> Result<Self, String> {
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
        Self::from_assets(assets, namespace)
    }

    /// Hand-authored fallback, kept in sync with the shipped assets (locked by a
    /// test), named in `namespace` like a loaded set.
    pub fn load_initial(namespace: &str) -> Self {
        let assets = vec![
            LibraryAsset {
                id: 0,
                id_name: "biome_border_fade".into(),
                boundary: GraphBoundary {
                    inputs: vec![
                        BoundaryPort::new("distance", PinType::SurfaceField),
                        BoundaryPort::new("radius", PinType::Scalar),
                    ],
                    outputs: vec![BoundaryPort::new("weight", PinType::SurfaceField)],
                },
                kernel: "biome_border_fade".into(),
            },
            LibraryAsset {
                id: 1,
                id_name: "standard_cave_noise".into(),
                boundary: GraphBoundary {
                    inputs: vec![BoundaryPort::new("position", PinType::Vec3)],
                    outputs: vec![BoundaryPort::new("density", PinType::Density)],
                },
                kernel: "standard_cave_noise".into(),
            },
            LibraryAsset {
                id: 2,
                id_name: "surface_layering".into(),
                boundary: GraphBoundary {
                    inputs: vec![BoundaryPort::new("density", PinType::Density)],
                    outputs: vec![BoundaryPort::new("material", PinType::Material)],
                },
                kernel: "surface_layering".into(),
            },
            LibraryAsset {
                id: 3,
                id_name: "exposure_layering".into(),
                boundary: GraphBoundary {
                    inputs: vec![
                        BoundaryPort::new("base_material", PinType::Material),
                        BoundaryPort::new("cap_material", PinType::Material),
                        BoundaryPort::new("exposure", PinType::Scalar),
                        BoundaryPort::new("threshold", PinType::Scalar),
                    ],
                    outputs: vec![BoundaryPort::new("material", PinType::Material)],
                },
                kernel: "exposure_layering".into(),
            },
            LibraryAsset {
                id: 4,
                id_name: "poisson_placement".into(),
                boundary: GraphBoundary {
                    inputs: Vec::new(),
                    outputs: vec![BoundaryPort::new("points", PinType::Positions)],
                },
                kernel: "poisson_placement".into(),
            },
        ];
        Self::from_assets(assets, namespace).expect("built-in library set is valid")
    }
}

/// Directory holding authored library assets.
pub fn libraries_dir() -> PathBuf {
    crate::paths::content_root().join("libraries")
}

/// Load the shared library set: dir-scan primary, `load_initial` fallback.
///
/// Never fails - a missing or invalid asset directory is logged at error level
/// and the hand-authored fallback set is used instead.
pub fn load_libraries() -> Arc<LoadedLibraries> {
    let dir = libraries_dir();
    let namespace = crate::paths::namespace();
    match LoadedLibraries::load_from_dir(&dir, namespace) {
        Ok(libs) => {
            log::info!(
                "Loaded {} libraries ({} kernels) from {}",
                libs.registry.len(),
                libs.kernels.len(),
                dir.display()
            );
            Arc::new(libs)
        }
        Err(e) => {
            log::error!(
                "Failed to load libraries from {}: {e}. Falling back to built-in set.",
                dir.display()
            );
            Arc::new(LoadedLibraries::load_initial(namespace))
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
    use nodegraph_ir::{LibraryGraphRef, LibraryRefParams};
    use voxel_core::ENGINE_NAMESPACE;

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
        let libs = LoadedLibraries::load_initial(ENGINE_NAMESPACE);
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
    fn library_ref_resolves_against_loaded_registry() {
        let libs = LoadedLibraries::load_initial(ENGINE_NAMESPACE);
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
        let libs = LoadedLibraries::load_initial(ENGINE_NAMESPACE);
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
    fn every_unknown_library_is_reported() {
        let libs = LoadedLibraries::load_initial(ENGINE_NAMESPACE);
        let mut g = Graph::new();
        for name in ["no_such_library", "Bad Name"] {
            g.add_node(NodeKind::LibraryRef(LibraryRefParams {
                library: LibraryGraphRef::new(name),
                resolved: None,
            }));
        }
        let problems = libs.resolve_refs(&mut g, ENGINE_NAMESPACE).unwrap_err();
        assert_eq!(problems.len(), 2, "{problems:?}")
    }

    #[test]
    fn non_contiguous_ids_rejected() {
        let assets = vec![asset(0, "a", "biome_border_fade"), asset(2, "b", "biome_border_fade")];
        let err = LoadedLibraries::from_assets(assets, ENGINE_NAMESPACE).expect_err("must refuse");
        assert!(err.contains("contiguous"), "{err}");
    }

    #[test]
    fn unknown_kernel_rejected() {
        let assets = vec![asset(0, "x", "does_not_exist")];
        let err = LoadedLibraries::from_assets(assets, ENGINE_NAMESPACE).expect_err("must refuse");
        assert!(err.contains("does_not_exist"), "{err}");
    }

    #[test]
    fn duplicate_names_rejected() {
        let assets = vec![asset(0, "a", "biome_border_fade"), asset(1, "a", "biome_border_fade")];
        let err = LoadedLibraries::from_assets(assets, ENGINE_NAMESPACE).expect_err("must refuse");
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn a_name_that_is_not_a_name_is_rejected() {
        let assets = vec![asset(0, "Cave Noise", "standard_cave_noise")];
        let err = LoadedLibraries::from_assets(assets, ENGINE_NAMESPACE).expect_err("must refuse");
        assert!(err.contains("Cave Noise"), "{err}");
    }

    #[test]
    fn fallback_matches_shipped_asset() {
        // The built-in fallback must match the authored file so the two never drift.
        let shipped = [
            include_str!("../../../assets/libraries/biome_border_fade.library.json"),
            include_str!("../../../assets/libraries/standard_cave_noise.library.json"),
            include_str!("../../../assets/libraries/surface_layering.library.json"),
            include_str!("../../../assets/libraries/exposure_layering.library.json"),
            include_str!("../../../assets/libraries/poisson_placement.library.json"),
        ];
        let assets: Vec<LibraryAsset> = shipped
            .iter()
            .map(|s| serde_json::from_str(s).expect("asset JSON valid"))
            .collect();
        let from_files =
            LoadedLibraries::from_assets(assets, ENGINE_NAMESPACE).expect("assets valid");
        let initial = LoadedLibraries::load_initial(ENGINE_NAMESPACE);
        assert_eq!(from_files.kernels, initial.kernels);
        assert_eq!(from_files.entries, initial.entries, "names drifted");
        for id in 0u32..5 {
            let gid = LibraryGraphId(id);
            assert_eq!(
                from_files.registry.get(gid).unwrap().boundary,
                initial.registry.get(gid).unwrap().boundary,
                "shipped asset {id} boundary drifted from load_initial"
            );
        }
    }
}
