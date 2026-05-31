//! Chunk-scoped common-subexpression cache.

use std::collections::HashMap;
use std::sync::Arc;

use nodegraph_ir::NodeId;

use crate::field::{ScalarField, Vec3Field};

/// A computed node output. Shared via `Arc` so cache reads are cheap clones.
#[derive(Clone)]
pub enum CachedOutput {
    /// A scalar / density field.
    Scalar(Arc<ScalarField>),
    /// A vector field (e.g. `WorldPos`).
    Vec3(Arc<Vec3Field>),
}

impl CachedOutput {
    /// Borrow as a scalar field, if it is one.
    pub fn as_scalar(&self) -> Option<&ScalarField> {
        match self {
            CachedOutput::Scalar(f) => Some(f),
            CachedOutput::Vec3(_) => None,
        }
    }

    /// Borrow as a vector field, if it is one.
    pub fn as_vec3(&self) -> Option<&Vec3Field> {
        match self {
            CachedOutput::Vec3(f) => Some(f),
            CachedOutput::Scalar(_) => None,
        }
    }

    /// Static name for diagnostics.
    pub fn kind_name(&self) -> &'static str {
        match self {
            CachedOutput::Scalar(_) => "scalar",
            CachedOutput::Vec3(_) => "vec3",
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
