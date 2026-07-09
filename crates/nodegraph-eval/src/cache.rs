//! Chunk-scoped common-subexpression cache.

use std::collections::HashMap;
use std::sync::Arc;

use nodegraph_ir::NodeId;
use voxel_core::{ChunkBuffer, MaterialId, Voxel};

use crate::field::{ScalarField, Vec3Field};
use crate::scatter::ScatterPoint;

/// A computed node output. Shared via `Arc` so cache reads are cheap clones.
#[derive(Clone)]
pub enum CachedOutput {
    /// A scalar / density field.
    Scalar(Arc<ScalarField>),
    /// A vector field (e.g. `WorldPos`, `DomainWarp`).
    Vec3(Arc<Vec3Field>),
    /// A material field - palette-compressed via `voxel-core`.
    Material(Arc<ChunkBuffer<MaterialId, 32>>),
    /// A terrain field (the `TerrainOutput` result).
    Terrain(Arc<ChunkBuffer<Voxel, 32>>),
    /// A scattered point set (for prop placement).
    Positions(Arc<Vec<ScatterPoint>>),
    /// A biome layer: continuous density + material, produced by `DensityOutput`
    /// and composited (density blended, material selected) before fusing to voxels.
    BiomeLayer {
        /// Continuous density field (solid where `> 0`).
        density: Arc<ScalarField>,
        /// Material field.
        material: Arc<ChunkBuffer<MaterialId, 32>>,
    },
}

impl CachedOutput {
    /// Borrow as a scalar field, if it is one.
    pub fn as_scalar(&self) -> Option<&ScalarField> {
        if let Self::Scalar(f) = self { Some(f) } else { None }
    }

    /// Borrow as a vector field, if it is one.
    pub fn as_vec3(&self) -> Option<&Vec3Field> {
        if let Self::Vec3(f) = self { Some(f) } else { None }
    }

    /// Borrow as a material, if it is one.
    pub fn as_material(&self) -> Option<&ChunkBuffer<MaterialId, 32>> {
        if let Self::Material(f) = self { Some(f) } else { None }
    }

    /// Borrow as a terrain, if it is one.
    pub fn as_terrain(&self) -> Option<&ChunkBuffer<Voxel, 32>> {
        if let Self::Terrain(f) = self { Some(f) } else { None }
    }
    
    /// Borrow as a point set, if it is one.
    pub fn as_positions(&self) -> Option<&Vec<ScatterPoint>> {
        if let Self::Positions(p) = self { Some(p) } else { None }
    }

    /// Borrow as a biome layer (density + material), if it is one.
    pub fn as_biome_layer(
        &self,
    ) -> Option<(&Arc<ScalarField>, &Arc<ChunkBuffer<MaterialId, 32>>)> {
        if let Self::BiomeLayer { density, material } = self {
            Some((density, material))
        } else {
            None
        }
    }

    /// Static name for diagnostics.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Scalar(_)   => "scalar",
            Self::Vec3(_)     => "vec3",
            Self::Material(_) => "material",
            Self::Terrain(_)  => "terrain",
            Self::Positions(_) => "positions",
            Self::BiomeLayer { .. } => "biome layer",
        }
    }
}

/// Common-subexpression cache scoped to a single chunk evaluation. Each
/// node's output is computed at most once.
#[derive(Default)]
pub struct EvalCache {
    outputs: HashMap<NodeId, CachedOutput>,
}

impl EvalCache {
    /// Empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cached output for a node, if present.
    pub fn get(&self, id: NodeId) -> Option<&CachedOutput> {
        self.outputs.get(&id)
    }

    /// Whether a node's output is cached.
    pub fn contains(&self, id: NodeId) -> bool {
        self.outputs.contains_key(&id)
    }

    /// Store a node's output.
    pub fn insert(&mut self, id: NodeId, out: CachedOutput) {
        self.outputs.insert(id, out);
    }
}
