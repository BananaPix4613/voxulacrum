//! The evaluation -> storage domain boundary
//!
//! The nodegraph evaluator and the engine deliberately use *different* voxel
//! containers, and this module is the single, permanent crossing between them:
//!
//! - **Evaluation domain:** `ChunkBuffer<Voxel, 32>` - a dense, fixed-size cube
//!   the evaluator fills while running a graph, tuned for the evaluator's
//!   write-once / read-by-coordinate access.
//! - **Storage domain:** `ChunkStorage` (a `PalettedBitArray` underneath) - the
//!   engine's compact, palette-compressed at-rest representation, tuned for
//!   memory footprint and the engine's runtime access.
//!
//! Keeping the two containers separate is intentional: each is optimized for its
//! side of the boundary, and unifying them would force one domain to inherit the
//! other's trade-offs. [`StorageBoundary::materialize`] is therefore *not*
//! temporary scaffolding - it is the architectural seam that lets the two
//! domains evolve independently.
//!
//! "Materialize" here means *transform an already-produced evaluation result
//! into storage form* - not lazy compute-then-realize. The evaluation has
//! already run; this is the cross-domain copy of its output. The two index
//! orderings are proven semantically equivalent by
//! `engine_and_eval_containers_are_semantically_equivalent` in `storage.rs`.

use voxel_core::{ChunkBuffer, Voxel};

use super::chunk::{CHUNK_SIZE, CHUNK_VOLUME};
use super::storage::{self, ChunkStorage};

/// The permanent seam between the evaluation-domain voxel container and the
/// storage-domain container.
///
/// Stateless today, but modeled as a value rather than a free function so any
/// future boundary configuration - alternate packings, validation hooks - has a
/// home without disturbing call sites.
#[derive(Debug, Default, Clone, Copy)]
pub struct StorageBoundary;

impl StorageBoundary {
    pub fn new() -> Self {
        Self
    }

    /// Materialize an evaluated chunk buffer into the engine's storage form.
    ///
    /// Index-copies the dense `ChunkBuffer` into a flat array using the engine's
    /// `voxel_index(x, y, z) = x + y*32 + z*32^2` decode, then packs it into a
    /// `ChunkStorage` (collapsing to `Uniform` when possible). The decode order
    /// matches `ChunkBuffer`'s layout - see the parity test in `storage.rs`.
    pub fn materialize(&self, chunk: &ChunkBuffer<Voxel, 32>) -> ChunkStorage {
        let mut voxel_arr = [Voxel::EMPTY; CHUNK_VOLUME];
        for i in 0..CHUNK_VOLUME {
            let x = i % CHUNK_SIZE;
            let y = (i / CHUNK_SIZE) % CHUNK_SIZE;
            let z = i / (CHUNK_SIZE * CHUNK_SIZE);
            voxel_arr[i] = chunk.get(x, y, z);
        }
        storage::storage_from_arrays(&voxel_arr)
    }
}
