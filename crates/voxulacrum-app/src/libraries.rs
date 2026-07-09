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
    BoundaryPort, Graph, GraphBoundary, GraphKind, LibraryGraphId, LibraryGraphRegistry, PinType,
};
use serde::Deserialize;

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
}

impl LoadedLibraries {
    /// Build from parsed assets: validate contiguous ids, project each boundary
    /// into a Library `Graph` (for pin resolution) and resolve its kernel name.
    /// Fails on an empty set, a non-contiguous id, or an unknown kernel name.
    fn from_assets(mut assets: Vec<LibraryAsset>) -> Result<Self, String> {
        assets.sort_by_key(|a| a.id);
        if assets.is_empty() {
            return Err("library registry must have at least one entry".into());
        }
        let mut registry = LibraryGraphRegistry::new();
        let mut kernels = HashMap::new();
        for (i, a) in assets.into_iter().enumerate() {
            if a.id as usize != i {
                return Err(format!(
                    "library ids must be contiguous from 0; expected {i}, found {}",
                    a.id
                ));
            }
            let id = LibraryGraphId(a.id);
            let kernel = LibraryKernel::from_name(&a.kernel).ok_or_else(|| {
                format!("library '{}' names unknown kernel '{}'", a.id_name, a.kernel)
            })?;
            let mut graph = Graph::of_kind(GraphKind::Library);
            graph.boundary = a.boundary;
            registry.insert(id, graph);
            kernels.insert(id, kernel);
        }
        Ok(Self { registry, kernels })
    }

    /// Load every `*.library.json` in `dir`, ordered by filename for determinism.
    pub fn load_from_dir(dir: &std::path::Path) -> Result<Self, String> {
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
        Self::from_assets(assets)
    }

    /// Hand-authored fallback, kept in sync with the shipped assets (locked by a test).
    pub fn load_initial() -> Self {
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
        Self::from_assets(assets).expect("built-in library set is valid")
    }
}

/// Directory holding authored library assets.
pub fn libraries_dir() -> PathBuf {
    crate::paths::asset_root().join("assets").join("libraries")
}

/// Load the shared library set: dir-scan primary, `load_initial` fallback.
///
/// Never fails - a missing or invalid asset directory is logged at error level
/// and the hand-authored fallback set is used instead.
pub fn load_libraries() -> Arc<LoadedLibraries> {
    let dir = libraries_dir();
    match LoadedLibraries::load_from_dir(&dir) {
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
            Arc::new(LoadedLibraries::load_initial())
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
    use nodegraph_ir::{LibraryRefParams, NodeKind};

    #[test]
    fn loads_and_binds_kernel() {
        let libs = LoadedLibraries::load_initial();
        assert_eq!(libs.kernels.len(), 5);
        let id = LibraryGraphId(0);
        assert_eq!(libs.kernels.get(&id), Some(&LibraryKernel::BiomeBorderFade));
        // Boundary is registered so a LibraryRef can resolve its pins.
        let g = libs.registry.get(id).expect("library graph present");
        assert_eq!(g.boundary.inputs.len(), 2);
        assert_eq!(g.boundary.output("weight").unwrap().ty, PinType::SurfaceField);
    }

    #[test]
    fn library_ref_resolves_against_loaded_registry() {
        let libs = LoadedLibraries::load_initial();
        let mut g = Graph::new();
        let n = g.add_node(NodeKind::LibraryRef(LibraryRefParams {
            library: LibraryGraphId(0),
            ..Default::default()
        }));
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
    fn non_contiguous_ids_rejected() {
        let a0 = LibraryAsset {
            id: 0, id_name: "a".into(), boundary: GraphBoundary::default(),
            kernel: "biome_border_fade".into(),
        };
        let a2 = LibraryAsset {
            id: 2, id_name: "b".into(), boundary: GraphBoundary::default(),
            kernel: "biome_border_fade".into(),
        };
        assert!(LoadedLibraries::from_assets(vec![a0, a2]).is_err());
    }

    #[test]
    fn unknown_kernel_rejected() {
        let a = LibraryAsset {
            id: 0, id_name: "x".into(), boundary: GraphBoundary::default(),
            kernel: "does_not_exist".into(),
        };
        assert!(LoadedLibraries::from_assets(vec![a]).is_err());
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
        let from_files = LoadedLibraries::from_assets(assets).expect("assets valid");
        let initial = LoadedLibraries::load_initial();
        assert_eq!(from_files.kernels, initial.kernels);
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
