//! Cross-chunk seam finalization (Substep 2b).
//!
//! `slab_smoothing::smooth_slabs` runs per chunk in isolation at generation
//! time (design §5 stage 7), so out-of-chunk neighbor columns are unknown and
//! surface steps that straddle a chunk border stay sharp: a bottom-layer cube
//! that should step down onto the chunk below, or an X/Z-border step whose
//! lower side is in the adjacent chunk. On chunk-Y seams this showed in-engine
//! as slabs failing to generate (a confirmed defect).
//!
//! This module derives those boundary slab demotions with real neighbor voxels,
//! using the same surface-step formulation as the isolated pass, once a chunk's
//! face neighbors are resident (the gate meshing already waits on).
//! `seam_smoothing_system` drives it. The pass is idempotent (only
//! `Cube`->`SlabBottom`; slabs are skipped) and order-independent (demotion
//! changes a surface's shape, never its cell, and both shapes are standable, so
//! a neighbor finalized first can't change another chunk's decision).
//!
//! Neighbor voxels are captured as cheap `Arc<ChunkStorage>` clones so the pass
//! holds no world borrow while it mutates the target. This is generation
//! finalization, not an actor edit, so - like `generate_chunk` and the streaming
//! loader - it constructs generated storage directly rather than going through
//! the mutation command API (which governs Authoring/PlayTime actor edits and
//! persistence; seam slabs are regenerable and never persisted).

use std::sync::Arc;

use glam::IVec3;
use voxel_core::{ShapeId, Voxel};

use super::chunk::{Chunk, CHUNK_SIZE};
use super::storage::ChunkStorage;
use super::World;

/// Owned storages of a chunk and its 26 neighbors (index `(dx+1)*9 + (dy+1)*3 +
/// (dz+1)`, center = 13), captured as `Arc` clones. Missing neighbors read empty.
pub struct NeighborStorages {
    storages: [Option<Arc<ChunkStorage>>; 27],
}

impl NeighborStorages {
    /// Capture the chunk at `pos` and its resident neighbors from `world`.
    pub fn capture(world: &World, pos: IVec3) -> Self {
        let mut storages: [Option<Arc<ChunkStorage>>; 27] = std::array::from_fn(|_| None);
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let idx = ((dx + 1) * 9 + (dy + 1) * 3 + (dz + 1)) as usize;
                    if let Some(c) = world.get_chunk(pos + IVec3::new(dx, dy, dz)) {
                        storages[idx] = Some(Arc::clone(&c.data.voxels));
                    }
                }
            }
        }
        Self { storages }
    }
    
    /// Voxel at chunk-local `(x, y, z)`; coords outside `[0, CHUNK_SIZE)` read the
    /// appropriate neighbor (one chunk away per axis), or `Voxel::EMPTY` if absent.
    #[inline]
    pub fn voxel(&self, x: i32, y: i32, z: i32) -> Voxel {
        let cs = CHUNK_SIZE as i32;
        let (dx, lx) = axis_split(x, cs);
        let (dy, ly) = axis_split(y, cs);
        let (dz, lz) = axis_split(z, cs);
        let idx = ((dx + 1) * 9 + (dy + 1) * 3 + (dz + 1)) as usize;
        match &self.storages[idx] {
            Some(s) => s.voxel(Chunk::voxel_index(lx as usize, ly as usize, lz as usize)),
            None => Voxel::EMPTY,
        }
    }
}

/// Split a possibly-out-of-range local coord into (neighbor offset, in-neighbor
/// local coord). Valid for `v` in `[-CHUNK_SIZE, 2*CHUNK_SIZE)`.
#[inline]
fn axis_split(v: i32, cs: i32) -> (i32, i32) {
    if v < 0 {
        (-1, v + cs)
    } else if v >= cs {
        (1, v - cs)
    } else {
        (0, v)
    }
}

/// The center chunk's surface cell in a column: the topmost in-chunk solid with
/// an empty cell above it (read from the chunk above at `y == CHUNK_SIZE - 1`).
/// A column whose topmost solid continues into the chunk above is buried - no
/// exposed surface in this chunk - and yields `None`.
fn surface_y(nb: &NeighborStorages, x: i32, z: i32) -> Option<i32> {
    for y in (0..CHUNK_SIZE as i32).rev() {
        if nb.voxel(x, y, z).is_solid() {
            return (!nb.voxel(x, y + 1, z).is_solid()).then_some(y);
        }
    }
    None
}

/// True if the cell at `(x, y, z)` - possibly in an adjacent chunk - is a
/// standable exposed surface: `Cube` or `SlabBottom` top with an empty cell
/// above it. (`SlabTop`'s top is at the cell ceiling, a full step, so it does
/// not soften the step above it - matching the isolated pass.)
#[inline]
fn standable_surface(nb: &NeighborStorages, x: i32, y: i32, z: i32) -> bool {
    matches!(nb.voxel(x, y, z).shape, ShapeId::Cube | ShapeId::SlabBottom)
        && !nb.voxel(x, y + 1, z).is_solid()
}

/// Boundary slab demotions for the center chunk: each exposed surface `Cube`
/// whose horizontally adjacent column - including columns in adjacent chunks -
/// has a standable surface exactly one cell lower becomes a `SlabBottom`.
/// Returns `(voxel_index, new_voxel)` for the caller to apply. Interior cubes
/// the isolated pass already demoted are slabs now and skipped, so this only
/// adds the seam demotions it could not see.
pub fn seam_demotions(nb: &NeighborStorages, distances: &[u8]) -> Vec<(usize, Voxel)> {
    debug_assert_eq!(distances.len(), CHUNK_SIZE * CHUNK_SIZE);
    if distances.iter().all(|&d| d == 0) {
        return Vec::new();
    }
    let mut demotions = Vec::new();
    for z in 0..CHUNK_SIZE as i32 {
        for x in 0..CHUNK_SIZE as i32 {
            if distances[x as usize + z as usize * CHUNK_SIZE] == 0 {
                continue;
            }
            let Some(sy) = surface_y(nb, x, z) else {
                continue; // no exposed surface in this chunk's part of the column
            };
            let here = nb.voxel(x, sy, z);
            if here.shape != ShapeId::Cube {
                continue; // already a slab (isolated pass or an earlier seam run)
            }
            // The lower side may sit at sy - 1 == -1, i.e. in the chunk below:
            // exactly the chunk-Y seam the isolated pass cannot see.
            const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
            if DIRS.iter().any(|&(dx, dz)| standable_surface(nb, x + dx, sy - 1, z + dz)) {
                let index = Chunk::voxel_index(x as usize, sy as usize, z as usize);
                demotions.push((
                    index,
                    Voxel { material: here.material, shape: ShapeId::SlabBottom, flags: here.flags },
                ));
            }
        }
    }
    demotions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::CHUNK_VOLUME;
    use crate::world::storage::storage_from_arrays;
    use voxel_core::MaterialId;
    
    fn arc_storage(f: impl FnOnce(&mut [Voxel; CHUNK_VOLUME])) -> Arc<ChunkStorage> {
        let mut arr = Box::new([Voxel::EMPTY; CHUNK_VOLUME]);
        f(&mut arr);
        Arc::new(storage_from_arrays(&arr))
    }
    
    /// A bottom-layer surface cube in the center chunk, with a surface one cell
    /// lower across the chunk-Y seam, must demote to a slab - the case the
    /// isolated `y == 0` guard misses.
    #[test]
    fn bottom_layer_cube_smooths_against_chunk_below() {
        let cube = Voxel::cube(MaterialId(1));
        let center = arc_storage(|a| {
            a[Chunk::voxel_index(5, 0, 5)] = cube;
        });
        let below = arc_storage(|a| {
            a[Chunk::voxel_index(6, CHUNK_SIZE - 1, 5)] = cube;
        });
        let mut storages: [Option<Arc<ChunkStorage>>; 27] = std::array::from_fn(|_| None);
        storages[13] = Some(center); // (0,0,0)
        storages[10] = Some(below);  // (0,-1,0)
        let nb = NeighborStorages { storages };
        
        let distances = [1u8; CHUNK_SIZE * CHUNK_SIZE];
        let idx = Chunk::voxel_index(5, 0, 5);
        assert!(
            seam_demotions(&nb, &distances)
                .iter()
                .any(|(i, v)| *i == idx && v.shape == ShapeId::SlabBottom),
            "bottom-layer cube should demote against the chunk below"
        );
    }
    
    /// A surface cube on the chunk's X border, with the lower surface across the
    /// X seam in the adjacent chunk, must demote - the in-chunk pass can't see it.
    #[test]
    fn border_cube_smooths_against_adjacent_chunk() {
        let cube = Voxel::cube(MaterialId(1));
        let center = arc_storage(|a| {
            a[Chunk::voxel_index(0, 5, 9)] = cube; // on the -X border
        });
        let west = arc_storage(|a| {
            a[Chunk::voxel_index(CHUNK_SIZE - 1, 4, 9)] = cube; // one lower, across the seam
        });
        let mut storages: [Option<Arc<ChunkStorage>>; 27] = std::array::from_fn(|_| None);
        storages[13] = Some(center); // (0,0,0)
        storages[4] = Some(west);    // (-1,0,0)
        let nb = NeighborStorages { storages };

        let distances = [1u8; CHUNK_SIZE * CHUNK_SIZE];
        let idx = Chunk::voxel_index(0, 5, 9);
        assert!(
            seam_demotions(&nb, &distances)
                .iter()
                .any(|(i, v)| *i == idx && v.shape == ShapeId::SlabBottom),
            "border cube should demote against the adjacent chunk's lower surface"
        );
    }

    /// With no chunk below (isolated view), the same cube stays sharp - nothing
    /// to step down onto - matching pre-seam behavior.
    #[test]
    fn isolated_view_leaves_it_sharp() {
        let cube = Voxel::cube(MaterialId(1));
        let center = arc_storage(|a| {
            a[Chunk::voxel_index(5, 0, 5)] = cube;
        });
        let mut storages: [Option<Arc<ChunkStorage>>; 27] = std::array::from_fn(|_| None);
        storages[13] = Some(center);
        let nb = NeighborStorages { storages };
        let distances = [1u8; CHUNK_SIZE * CHUNK_SIZE];
        assert!(seam_demotions(&nb, &distances).is_empty());
    }
    /// A top-layer cell whose terrain continues into the chunk above is buried,
    /// not a surface: the seam pass must not demote it even when an adjacent
    /// column steps down. (The double-slab artifact: a false slab here stacks
    /// under the upper chunk's real surface slab at the chunk-Y boundary.)
    #[test]
    fn buried_top_cell_is_not_demoted() {
        let cube = Voxel::cube(MaterialId(1));
        let top = CHUNK_SIZE - 1;
        let center = arc_storage(|a| {
            a[Chunk::voxel_index(4, top, 4)] = cube; // continues upward...
            a[Chunk::voxel_index(5, top - 1, 4)] = cube; // ...with a step beside it
        });
        let above = arc_storage(|a| {
            a[Chunk::voxel_index(4, 0, 4)] = cube; // the real surface cell
        });
        let mut storages: [Option<Arc<ChunkStorage>>; 27] = std::array::from_fn(|_| None);
        storages[13] = Some(center); // (0,0,0)
        storages[16] = Some(above);  // (0,1,0)
        let nb = NeighborStorages { storages };

        let distances = [1u8; CHUNK_SIZE * CHUNK_SIZE];
        let idx = Chunk::voxel_index(4, top, 4);
        assert!(
            !seam_demotions(&nb, &distances).iter().any(|(i, _)| *i == idx),
            "buried top-layer cell must stay a cube"
        );
    }

    /// The same top-layer step with genuine air above (chunk above resident and
    /// empty there) is a real surface: the seam pass demotes it - the isolated
    /// pass defers all top-layer decisions here.
    #[test]
    fn exposed_top_cell_is_demoted() {
        let cube = Voxel::cube(MaterialId(1));
        let top = CHUNK_SIZE - 1;
        let center = arc_storage(|a| {
            a[Chunk::voxel_index(4, top, 4)] = cube;
            a[Chunk::voxel_index(5, top - 1, 4)] = cube;
        });
        let above = arc_storage(|_| {}); // resident and all air
        let mut storages: [Option<Arc<ChunkStorage>>; 27] = std::array::from_fn(|_| None);
        storages[13] = Some(center);
        storages[16] = Some(above);
        let nb = NeighborStorages { storages };

        let distances = [1u8; CHUNK_SIZE * CHUNK_SIZE];
        let idx = Chunk::voxel_index(4, top, 4);
        assert!(
            seam_demotions(&nb, &distances)
                .iter()
                .any(|(i, v)| *i == idx && v.shape == ShapeId::SlabBottom),
            "exposed top-layer step should demote"
        );
    }
}
