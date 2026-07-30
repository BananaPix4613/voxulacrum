//! The evaluation -> storage domain boundary
//!
//! The nodegraph evaluator and the engine deliberately use *different*
//! containers, and this module is the single, permanent crossing between them.
//! **Every** generated chunk layer crosses here - terrain, fluid, and foliage -
//! so the seam is one thing rather than one convention per layer (drift-review
//! 2.3, closed in 0.3.0; before that terrain crossed here, foliage crossed
//! through free functions, and fluid had no named crossing at all).
//!
//! - **Evaluation domain:** `ChunkBuffer<Voxel, 32>`, `PaintLayer`,
//!   `ScatterBucket`, `ColumnField` - what the evaluator produces while running
//!   a graph, tuned for write-once / read-by-coordinate access.
//! - **Storage domain:** `ChunkStorage`, `FluidLayer`, `DetailLayers`,
//!   `ScatterStore` - the engine's compact at-rest representations, tuned for
//!   memory footprint and runtime access.
//!
//! Keeping the two sides separate is intentional: each is optimized for its own
//! side, and unifying them would force one domain to inherit the other's
//! trade-offs. These methods are therefore *not* temporary scaffolding - they
//! are the architectural seam that lets the two domains evolve independently.
//!
//! **The boundary names the crossing; it does not own the rules.** Fluid
//! generation semantics live in [`super::fluid_gen`], whose helpers
//! (`fluid_capacity`, `sea_surface_mass`, `foliage_submerged`) are shared with
//! the cross-chunk seam finalization pass and the fluid simulation - they are
//! generation rules, not translation. The foliage translators live in
//! [`super::world_generator`] beside the stage that produces their inputs. What
//! this module guarantees is that there is exactly one entry point per layer and
//! that `generate_chunk` calls nothing else to cross.
//!
//! "Materialize" means *transform an already-produced evaluation result into
//! storage form* - not lazy compute-then-realize. The evaluation has already
//! run; this is the cross-domain copy of its output. The two index orderings are
//! proven semantically equivalent by
//! `engine_and_eval_containers_are_semantically_equivalent` in `storage.rs`.

use nodegraph_eval::{ColumnField, PaintLayer, ScatterBucket};
use voxel_core::{ChunkBuffer, Voxel};

use super::chunk::{CHUNK_SIZE, CHUNK_VOLUME};
use super::layers::{DetailLayers, FluidLayer, ScatterStore};
use super::storage::{self, ChunkStorage};

/// The permanent seam between the evaluation-domain voxel container and the
/// storage-domain container.
///
/// Carries the boundary's configuration - currently the world's sea level,
/// which the fluid crossing needs. No `Default`: a silently-zero sea level would
/// flood or drain a world without complaint, so the value must be supplied.
#[derive(Debug, Clone, Copy)]
pub struct StorageBoundary {
    /// Global ocean surface (world-Y): empty voxels at or below it fill with
    /// water at generation time (design doc §7). Sourced from the manifest.
    sea_level: i32,
}

impl StorageBoundary {
    pub fn new(sea_level: i32) -> Self {
        Self { sea_level }
    }

    /// The global ocean surface this boundary fills at/below. Exposed so
    /// generation-finalization passes that run outside generation (the seam
    /// pass) apply the same ocean rule to voxels they mutate later.
    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    /// Materialize an evaluated chunk buffer into the engine's storage form
    /// (generation stage 5's output, after slab smoothing).
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

    /// Materialize the chunk's fluid layer (generation stage 9): ocean fill from
    /// the global sea level, then any biome-authored ponds layered on top.
    ///
    /// `pond_levels` is the evaluator's per-column pond-surface field and is the
    /// one genuinely eval-domain input on this path - `ocean_fill` reads storage
    /// and writes storage, so before 0.3.0 this crossing had no name to cross by.
    pub fn materialize_fluid(
        &self,
        storage: &ChunkStorage,
        chunk_y: i32,
        pond_levels: Option<&ColumnField>,
    ) -> FluidLayer {
        let mut fluids = super::fluid_gen::ocean_fill(storage, chunk_y, self.sea_level);
        if let Some(levels) = pond_levels {
            super::fluid_gen::apply_biome_ponds(
                &mut fluids,
                storage,
                chunk_y,
                self.sea_level,
                levels,
            );
        }
        fluids
    }
    
    /// Materialize the chunk's foliage layers (generation stage 10): Tier-1
    /// paint into `DetailLayers` and Tier-2/3 scatter into `ScatterStore`.
    /// 
    /// Must run after [`Self::materialize_fluid`] and be given its result, so
    /// both translators can drop anything the finished fluid field submerges
    /// (design §5 stage 9 before stage 10; drift-review 1.4). Taking `fluids` by
    /// reference rather than recomputing it is what makes that ordering a
    /// compile-time requirement instead of a convention.
    pub fn materialize_foliage(
        &self,
        paint: &[PaintLayer],
        scatter: &[ScatterBucket],
        storage: &ChunkStorage,
        fluids: &FluidLayer,
    ) -> (DetailLayers, ScatterStore) {
        (
            super::world_generator::paint_to_detail_layers(paint, storage, fluids),
            super::world_generator::scatter_to_store(scatter, fluids),
        )
    }
}
