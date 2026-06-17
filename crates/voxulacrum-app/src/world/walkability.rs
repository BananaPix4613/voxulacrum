//! Walkability mask (design doc §5 stage 6, §"Walkability mask").
//!
//! A derived per-chunk artifact marking the voxels a player can stand on top of.
//! Computed once from a chunk's voxels and reused by slab smoothing (Substep 6),
//! AI pathfinding, and player movement - so none of them re-derive it.
//!
//! Phase 3 scope (Substep 5): the type plus the pure computation
//! [`WalkabilityMask::from_storage`] and its determinism test. No chunk stores a
//! mask yet; Substep 6 adds the field and the first consumer.
//!
//! Boundary convention: the headroom checks (voxel above, voxel two above) read
//! positions above the chunk's top as empty (air), matching the mesher's
//! closed-world "missing neighbor = empty" rule. The computation is therefore a
//! pure function of the chunk's own voxels - exact for the topmost chunk, and
//! approximate only at the top two voxel layers of interior chunks.

use super::chunk::{Chunk, CHUNK_SIZE, CHUNK_VOLUME};
use super::storage::ChunkStorage;
use voxel_core::ShapeId;

/// Number of `u64` words needed for one bit per voxel (`32768 / 64 = 512`).
const WORDS: usize = CHUNK_VOLUME / 64;

/// One bit per voxel; set when the player can stand on top of that voxel.
///
/// Stored sparsely - `bits` is `None` when no voxel is walkable (fully-air,
/// fully-buried, or sub-surface chunks, the common case), costing O(1).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct WalkabilityMask {
    bits: Option<Box<[u64; WORDS]>>,
}

impl WalkabilityMask {
    /// True when no voxel in the chunk is walkable.
    pub fn is_empty(&self) -> bool {
        self.bits.is_none()
    }

    /// Number of walkable voxels. A mask-introspection API for future AI
    /// pathfinding / debug overlays; no runtime caller yet (exercised by tests),
    /// hence the targeted allow rather than a module-wide one.
    #[allow(dead_code)]
    pub fn count(&self) -> u32 {
        match &self.bits {
            Some(words) => words.iter().map(|w| w.count_ones()).sum(),
            None => 0,
        }
    }

    /// Walkability of the voxel at flat index `index` (see [`Chunk::voxel_index`]).
    #[inline]
    pub fn get(&self, index: usize) -> bool {
        match &self.bits {
            Some(words) => (words[index / 64] >> (index % 64)) & 1 == 1,
            None => false,
        }
    }

    /// Walkability of the voxel at chunk-local coordinates.
    #[inline]
    pub fn is_walkable(&self, x: usize, y: usize, z: usize) -> bool {
        self.get(Chunk::voxel_index(x, y, z))
    }

    /// Compute the mask from a chunk's voxels.
    ///
    /// A voxel is walkable when its top surface is horizontal and standable
    /// (`Cube` or `SlabBottom` - `SlabTop`'s top is at the cell ceiling with no
    /// headroom), the voxel directly above is empty, and the voxel two above is
    /// empty (player headroom). Positions above the chunk top count as air.
    pub fn from_storage(voxels: &ChunkStorage) -> Self {
        let mut bits: Option<Box<[u64; WORDS]>> = None;
        for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    if !Self::is_walkable_at(voxels, x, y, z) {
                        continue;
                    }
                    let index = Chunk::voxel_index(x, y, z);
                    let words = bits.get_or_insert_with(|| Box::new([0u64; WORDS]));
                    words[index / 64] |= 1u64 << (index % 64);
                }
            }
        }
        Self { bits }
    }

    /// The walkability predicate for a single cell, with above-chunk = air.
    fn is_walkable_at(voxels: &ChunkStorage, x: usize, y: usize, z: usize) -> bool {
        let here = voxels.voxel(Chunk::voxel_index(x, y, z));
        // Top surface must be horizontal and standable.
        if !matches!(here.shape, ShapeId::Cube | ShapeId::SlabBottom) {
            return false;
        }
        // Headroom: the two cells above must be empty (air). Above the chunk top
        // is treated as air (isolated-chunk boundary).
        let above_clear =
            y + 1 >= CHUNK_SIZE || !voxels.voxel(Chunk::voxel_index(x, y + 1, z)).is_solid();
        let headroom_clear =
            y + 2 >= CHUNK_SIZE || !voxels.voxel(Chunk::voxel_index(x, y + 2, z)).is_solid();
        above_clear && headroom_clear
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::storage::storage_from_arrays;
    use voxel_core::{MaterialId, Voxel};

    const STONE: MaterialId = MaterialId(1);

    fn cube() -> Voxel {
        Voxel::cube(STONE)
    }
    fn slab_bottom() -> Voxel {
        Voxel { material: STONE, shape: ShapeId::SlabBottom, flags: 0 }
    }
    fn slab_top() -> Voxel {
        Voxel { material: STONE, shape: ShapeId::SlabTop, flags: 0 }
    }

    /// Build storage from an all-air array mutated by `f`.
    fn storage_with(f: impl FnOnce(&mut [Voxel; CHUNK_VOLUME])) -> ChunkStorage {
        let mut arr = Box::new([Voxel::EMPTY; CHUNK_VOLUME]);
        f(&mut arr);
        storage_from_arrays(&arr)
    }

    fn set(arr: &mut [Voxel; CHUNK_VOLUME], x: usize, y: usize, z: usize, v: Voxel) {
        arr[Chunk::voxel_index(x, y, z)] = v;
    }

    #[test]
    fn determinism_recompute_matches() {
        // A varied pattern, recomputed twice, must be bit-identical.
        let storage = storage_with(|a| {
            set(a, 4, 4, 4, cube());               // walkable: air above
            set(a, 5, 4, 4, cube());               // buried: cube directly above
            set(a, 5, 5, 4, cube());
            set(a, 6, 4, 4, slab_bottom());        // walkable slab
            set(a, 7, 4, 4, slab_top());           // not walkable on top
            set(a, 8, CHUNK_SIZE - 1, 4, cube());  // top layer: air above-chunk
        });
        let a = WalkabilityMask::from_storage(&storage);
        let b = WalkabilityMask::from_storage(&storage);
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn cube_with_air_above_is_walkable() {
        let s = storage_with(|a| set(a, 4, 4, 4, cube()));
        let m = WalkabilityMask::from_storage(&s);
        assert!(m.is_walkable(4, 4, 4));
        assert_eq!(m.count(), 1);
    }

    #[test]
    fn cube_directly_below_cube_is_not_walkable() {
        let s = storage_with(|a| {
            set(a, 4, 4, 4, cube());
            set(a, 4, 5, 4, cube()); // no headroom for the lower cube
        });
        let m = WalkabilityMask::from_storage(&s);
        assert!(!m.is_walkable(4, 4, 4));
        // the upper cube has air above and two-above -> walkable.
        assert!(m.is_walkable(4, 5, 4));
    }

    #[test]
    fn headroom_requires_two_clear_cells() {
        let s = storage_with(|a| {
            set(a, 4, 4, 4, cube());
            set(a, 4, 6, 4, cube()); // one air (y+1) then a cube two-above (y+2)
        });
        let m = WalkabilityMask::from_storage(&s);
        assert!(!m.is_walkable(4, 4, 4));
    }

    #[test]
    fn slab_bottom_is_walkable_slab_top_is_not() {
        let s = storage_with(|a| {
            set(a, 4, 4, 4, slab_bottom());
            set(a, 6, 4, 4, slab_top());
        });
        let m = WalkabilityMask::from_storage(&s);
        assert!(m.is_walkable(4, 4, 4));
        assert!(!m.is_walkable(6, 4, 4));
    }

    #[test]
    fn top_layer_uses_air_above_chunk() {
        let s = storage_with(|a| set(a, 4, CHUNK_SIZE - 1, 4, cube()));
        let m = WalkabilityMask::from_storage(&s);
        assert!(m.is_walkable(4, CHUNK_SIZE - 1, 4));
    }

    #[test]
    fn all_air_mask_is_empty() {
        let s = storage_with(|_| {});
        let m = WalkabilityMask::from_storage(&s);
        assert!(m.is_empty());
        assert_eq!(m.count(), 0);
        assert!(!m.is_walkable(0, 0, 0));
    }
}