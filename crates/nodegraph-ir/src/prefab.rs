//! Prefab templates: a small cluster of voxels stamped at a placement point.
//!
//! Loaded from `<name>.prefab.json` at the hot-reload boundary (see
//! `nodegraph-hotreload`), never by the evaluator itself - the evaluator
//! stays filesystem-pure and only reads an already-resolved template.

use serde::{Deserialize, Serialize};
use voxel_core::Voxel;

/// One voxel of a prefab, at an integer offset from the template origin.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PrefabVoxel {
    /// Offset `[x, y, z]` from the template origin (before anchoring).
    pub at: [i32; 3],
    /// The voxel to stamp.
    pub voxel: Voxel,
}

/// A reusable voxel cluster (a rock, a bush, a structure piece).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PrefabTemplate {
    /// The template-local voxel that lands on the placement column, one cell
    /// above the scanned surface. Defaults to the origin when absent in JSON.
    #[serde(default)]
    pub anchor: [i32; 3],
    /// The voxels making up the prefab.
    pub voxels: Vec<PrefabVoxel>,
}
