//! Slab smoothing (design doc §5 stage 7, §"Slab smoothing").
//!
//! A worldgen pass that halves single-cube height transitions on walkable
//! terrain by demoting the high-side cube to a `SlabBottom`. It reads the
//! walkability mask (Substep 5) and rewrites only full `Cube` voxels, so
//! authored slabs are respected.
//!
//! Smoothing distance is a parameter (`traversal_smoothing_distance`): `0`
//! disables the pass entirely (e.g. a flat plains biome), `>= 1` smooths
//! single-voxel steps. Values above 1 currently behave as 1 - true
//! multi-distance staircasing of taller drops needs generated geometry and
//! cross-chunk neighbor voxels, neither of which the single-chunk generation
//! hook has, so it remains future work. The pass runs per chunk in isolation,
//! using the same "above/around the chunk is air" convention as the walkability
//! mask, so transitions that straddle a chunk boundary are left sharp.

use super::chunk::{Chunk, CHUNK_SIZE};
use super::storage::ChunkStorage;
use super::walkability::WalkabilityMask;
use voxel_core::{ShapeId, Voxel};

/// Smooth single-cube steps in place: demote each walkable surface cube that
/// borders a walkable neighbor exactly one cell lower into a `SlabBottom`.
///
/// `distance` is the traversal smoothing distance: `0` disables the pass; `>= 1`
/// applies single-step smoothing. Values above 1 are reserved for future
/// multi-distance smoothing and currently behave as 1.
pub fn smooth_slabs(storage: &mut ChunkStorage, distance: u32) {
    if distance == 0 {
        return; // Smoothing disabled for this chunk (e.g. a flat plains biome).
    }
    let mask = WalkabilityMask::from_storage(storage);
    if mask.is_empty() {
        return; // No walkable surface -> nothing to smooth (keeps Uniform chunks Uniform).
    }

    // Decide every demotion from the immutable original (mask + original shapes),
    // then apply. This makes the pass independent of iteration order.
    let mut demotions: Vec<(usize, Voxel)> = Vec::new();
    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                if !mask.is_walkable(x, y, z) {
                    continue;
                }
                let index = Chunk::voxel_index(x, y, z);
                let here = storage.voxel(index);
                // Only full cubes are smoothed; authored slabs are left as-is.
                if here.shape != ShapeId::Cube {
                    continue;
                }
                if has_lower_walkable_neighbor(&mask, x, y, z) {
                    demotions.push((
                        index,
                        Voxel { material: here.material, shape: ShapeId::SlabBottom, flags: here.flags },
                    ));
                }
            }
        }
    }

    for (index, voxel) in demotions {
        storage.set_voxel(index, voxel);
    }
}

/// True if any of the four horizontal neighbors has a walkable surface exactly
/// one cell below `(x, y, z)`. Out-of-bounds neighbors count as non-walkable
/// (isolated-chunk boundary), so cross-chunk steps are left sharp.
fn has_lower_walkable_neighbor(mask: &WalkabilityMask, x: usize, y: usize, z: usize) -> bool {
    if y == 0 {
        return false; // No cell below within the chunk.
    }
    let ny = y - 1;
    const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    for (dx, dz) in DIRS {
        let nx = x as i32 + dx;
        let nz = z as i32 + dz;
        if nx < 0 || nx >= CHUNK_SIZE as i32 || nz < 0 || nz >= CHUNK_SIZE as i32 {
            continue;
        }
        if mask.is_walkable(nx as usize, ny, nz as usize) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::CHUNK_VOLUME;
    use crate::world::storage::storage_from_arrays;
    use voxel_core::MaterialId;

    const STONE: MaterialId = MaterialId(1);

    fn cube() -> Voxel {
        Voxel::cube(STONE)
    }
    fn slab_bottom() -> Voxel {
        Voxel { material: STONE, shape: ShapeId::SlabBottom, flags: 0 }
    }

    fn storage_with(f: impl FnOnce(&mut [Voxel; CHUNK_VOLUME])) -> ChunkStorage {
        let mut arr = Box::new([Voxel::EMPTY; CHUNK_VOLUME]);
        f(&mut arr);
        storage_from_arrays(&arr)
    }

    fn set(arr: &mut [Voxel; CHUNK_VOLUME], x: usize, y: usize, z: usize, v: Voxel) {
        arr[Chunk::voxel_index(x, y, z)] = v;
    }

    fn shape_at(s: &ChunkStorage, x: usize, y: usize, z: usize) -> ShapeId {
        s.voxel(Chunk::voxel_index(x, y, z)).shape
    }

    #[test]
    fn single_step_high_side_becomes_slab() {
        let mut s = storage_with(|a| {
            set(a, 4, 4, 4, cube()); // high surface at y=4
            set(a, 5, 3, 4, cube()); // neighbor surface one lower
        });
        smooth_slabs(&mut s, 1);
        assert_eq!(shape_at(&s, 4, 4, 4), ShapeId::SlabBottom, "high cube should demote");
        assert_eq!(shape_at(&s, 5, 3, 4), ShapeId::Cube, "low cube unchanged");
        // Material and flags are preserved through the demotion.
        assert_eq!(s.voxel(Chunk::voxel_index(4, 4, 4)).material, STONE);
    }

    #[test]
    fn flat_ground_is_not_smoothed() {
        let mut s = storage_with(|a| {
            set(a, 4, 4, 4, cube());
            set(a, 5, 4, 4, cube());
            set(a, 6, 4, 4, cube());
        });
        smooth_slabs(&mut s, 1);
        for x in [4usize, 5, 6] {
            assert_eq!(shape_at(&s, x, 4, 4), ShapeId::Cube, "flat row must stay cubes");
        }
    }

    #[test]
    fn two_cube_cliff_is_not_smoothed() {
        let mut s = storage_with(|a| {
            set(a, 4, 5, 4, cube()); // surface at y=5
            set(a, 5, 3, 4, cube()); // surface two lower (a cliff, not a step)
        });
        smooth_slabs(&mut s, 1);
        assert_eq!(shape_at(&s, 4, 5, 4), ShapeId::Cube, "2-cube cliff must stay sharp");
    }

    #[test]
    fn authored_slab_is_respected() {
        let mut s = storage_with(|a| {
            set(a, 4, 4, 4, slab_bottom()); // already a slab on the high side
            set(a, 5, 3, 4, cube());
        });
        smooth_slabs(&mut s, 1);
        assert_eq!(shape_at(&s, 4, 4, 4), ShapeId::SlabBottom, "authored slab left intact");
    }

    #[test]
    fn smoothing_is_deterministic() {
        let make = || {
            storage_with(|a| {
                set(a, 4, 4, 4, cube());
                set(a, 5, 3, 4, cube());
                set(a, 6, 2, 4, cube());
                set(a, 10, 5, 10, cube());
                set(a, 11, 5, 10, cube());
            })
        };
        let mut a = make();
        let mut b = make();
        smooth_slabs(&mut a, 1);
        smooth_slabs(&mut b, 1);
        for i in 0..CHUNK_VOLUME {
            assert_eq!(a.voxel(i), b.voxel(i), "nondeterministic at index {i}");
        }
        // Guard against vacuous determinism: the pass must actually rewrite cells.
        let original = make();
        let changed = (0..CHUNK_VOLUME).any(|i| original.voxel(i) != a.voxel(i));
        assert!(changed, "smoothing should have demoted at least one cube");
    }

    #[test]
    fn distance_zero_disables_smoothing() {
        let mut s = storage_with(|a| {
            set(a, 4, 4, 4, cube()); // a 1-cube step distance 1 would smooth
            set(a, 5, 3, 4, cube());
        });
        smooth_slabs(&mut s, 0);
        assert_eq!(shape_at(&s, 4, 4, 4), ShapeId::Cube, "distance 0 leaves cubes untouched");
    }
}
