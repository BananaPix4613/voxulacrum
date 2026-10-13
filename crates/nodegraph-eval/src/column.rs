//! Per-column (2D) field container and cache.
//!
//! Many world-generation quantities are constant down a vertical column -
//! surface height, climate channels, zone/biome assignment. A [`ColumnField`]
//! holds one `f32` per `(x, z)` column of a chunk, and [`ColumnCache`] memoizes
//! these per node: the 2D analog of [`EvalCache`](crate::EvalCache).
//!
//! No node populates a [`ColumnField`] yet - `SurfaceField`-producing nodes
//! arrive in the functional-graph substeps. This module is the container they
//! will write into.

use std::collections::HashMap;
use std::sync::Arc;

use nodegraph_ir::NodeId;

use crate::field::CHUNK_DIM;

/// Dense `CHUNK_DIM x CHUNK_DIM` plane of `f32`, one value per `(x, z)` column.
/// Index order `x + z*N`, matching the XZ-major layout of the 3D fields.
#[derive(Clone, Debug)]
pub struct ColumnField {
    data: Box<[f32]>,
}

impl ColumnField {
    /// Edge length.
    pub const DIM: usize = CHUNK_DIM;
    /// Total columns (`N²`).
    pub const AREA: usize = CHUNK_DIM * CHUNK_DIM;

    /// All-zero plane.
    pub fn zeroed() -> Self {
        Self { data: vec![0.0; Self::AREA].into_boxed_slice() }
    }

    /// Plane filled with a single value.
    pub fn filled(value: f32) -> Self {
        Self { data: vec![value; Self::AREA].into_boxed_slice() }
    }

    /// Flat index for column coords.
    #[inline]
    pub fn index(x: usize, z: usize) -> usize {
        x + z * CHUNK_DIM
    }

    /// Read at column coords.
    #[inline]
    pub fn get(&self, x: usize, z: usize) -> f32 {
        self.data[Self::index(x, z)]
    }

    /// Write at column coords.
    #[inline]
    pub fn set(&mut self, x: usize, z: usize, v: f32) {
        self.data[Self::index(x, z)] = v;
    }

    /// Raw slice (length `N²`, index order `x + z*N`).
    pub fn data(&self) -> &[f32] {
        &self.data
    }
    
    /// New plane from an elementwise unary op.
    pub fn map(&self, f: impl Fn(f32) -> f32) -> Self {
        Self { data: self.data.iter().map(|&v| f(v)).collect() }
    }
    
    /// New plane from an elementwise binary op over two planes.
    pub fn zip_with(&self, other: &Self, f: impl Fn(f32, f32) -> f32) -> Self {
        Self {
            data: self.data.iter().zip(other.data.iter()).map(|(&a, &b)| f(a, b)).collect(),
        }
    }
}

/// Dense `CHUNK_DIM x CHUNK_DIM` plane of `u16`, one discrete id per `(x, z)`
/// column - a zone id (Substep 5) or biome id (Substep 6). Index order
/// `x + z*N`, matching [`ColumnField`].
#[derive(Clone, Debug)]
pub struct IdColumn {
    data: Box<[u16]>,
}

impl IdColumn {
    /// Edge length.
    pub const DIM: usize = CHUNK_DIM;
    /// Total columns (`N²`).
    pub const AREA: usize = CHUNK_DIM * CHUNK_DIM;

    /// All-zero plane (id 0)
    pub fn zeroed() -> Self {
        Self { data: vec![0; Self::AREA].into_boxed_slice() }
    }

    /// Plane filled with a single id.
    pub fn filled(id: u16) -> Self {
        Self { data: vec![id; Self::AREA].into_boxed_slice() }
    }

    /// Read at column coords.
    #[inline]
    pub fn get(&self, x: usize, z: usize) -> u16 {
        self.data[ColumnField::index(x, z)]
    }

    /// Write at column coords.
    #[inline]
    pub fn set(&mut self, x: usize, z: usize, id: u16) {
        self.data[ColumnField::index(x, z)] = id;
    }

    /// Raw slice (length `N²`, index order `x + z*N`).
    pub fn data(&self) -> &[u16] {
        &self.data
    }
}

/// A computed per-column node output. Shared via `Arc` so cache reads are cheap
/// clones. The 2D analog of [`CachedOutput`](crate::CachedOutput).
#[derive(Clone)]
pub enum ColumnOutput {
    /// A continuous per-column scalar field (a climate channel).
    Surface(Arc<ColumnField>),
    /// A discrete per-column id field (zone or biome assignment).
    Id(Arc<IdColumn>),
}

impl ColumnOutput {
    /// Borrow as a surface field, if it is one.
    pub fn as_surface(&self) -> Option<&Arc<ColumnField>> {
        if let Self::Surface(f) = self { Some(f) } else { None }
    }

    /// Borrow as an id column, if it is one.
    pub fn as_id(&self) -> Option<&Arc<IdColumn>> {
        if let Self::Id(f) = self { Some(f) } else { None }
    }

    /// Static name for diagnostics.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Surface(_) => "surface",
            Self::Id(_) => "id",
        }
    }
}

/// Per-column memo scoped to a single chunk evaluation: each node's per-column
/// output is computed at most once. The 2D analog of
/// [`EvalCache`](crate::EvalCache).
#[derive(Default)]
pub struct ColumnCache {
    columns: HashMap<NodeId, ColumnOutput>,
}

impl ColumnCache {
    /// Empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cached output for a node, if present.
    pub fn get(&self, id: NodeId) -> Option<&ColumnOutput> {
        self.columns.get(&id)
    }

    /// Whether a node's output is cached.
    pub fn contains(&self, id: NodeId) -> bool {
        self.columns.contains_key(&id)
    }

    /// Store a node's output.
    pub fn insert(&mut self, id: NodeId, out: ColumnOutput) {
        self.columns.insert(id, out);
    }

    /// Number of cached columns.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// True if nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodegraph_ir::{ConstantParams, Graph, NodeKind};

    #[test]
    fn column_field_roundtrips() {
        let mut f = ColumnField::zeroed();
        f.set(3, 5, 1.5);
        assert_eq!(f.get(3, 5), 1.5);
        assert_eq!(f.get(0, 0), 0.0);
        assert_eq!(ColumnField::filled(2.0).get(7, 7), 2.0);
    }

    #[test]
    fn column_cache_insert_and_get() {
        let mut g = Graph::new();
        let a = g.add_node(NodeKind::Constant(ConstantParams::default()));
        let b = g.add_node(NodeKind::Constant(ConstantParams::default()));

        let mut cache = ColumnCache::new();
        assert!(cache.is_empty());
        cache.insert(a, ColumnOutput::Surface(Arc::new(ColumnField::filled(1.0))));
        assert!(cache.contains(a));
        assert!(!cache.contains(b));
        assert_eq!(cache.get(a).unwrap().as_surface().unwrap().get(0, 0), 1.0);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn id_column_roundtrips() {
        let mut c = IdColumn::zeroed();
        c.set(2, 4, 7);
        assert_eq!(c.get(2, 4), 7);
        assert_eq!(c.get(0, 0), 0);
        assert_eq!(IdColumn::filled(3).get(9, 9), 3);
    }
}
