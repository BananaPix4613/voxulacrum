//! Slab smoothing (design doc §5 stage 7, §"Slab smoothing").
//!
//! A worldgen pass that halves single-cube height transitions on the terrain
//! surface by demoting the high-side cube to a `SlabBottom`. It operates on the
//! chunk's **surface heightfield** — the topmost solid voxel per column — and
//! rewrites only full `Cube` voxels, so authored slabs are respected.
//!
//! Formulation: a column's surface cube demotes when a horizontally adjacent
//! column's surface sits exactly one cell lower on a standable top (`Cube` or
//! `SlabBottom`). This is a per-column step comparison, not a per-voxel
//! walkability test. For top surfaces it decides exactly what the old
//! walkability formulation decided (a topmost solid always has clear air above
//! it, so the old headroom checks were automatically satisfied there); the one
//! deliberate behavior change is **top surface only**: interior surfaces (cave
//! floors under an intact roof) are no longer smoothed - they stay sharp until
//! cave content decides otherwise. Entity fit is the collision system's
//! question (geometry is the authority there), not worldgen's.
//!
//! Smoothing distance is a per-column parameter (`traversal_smoothing_distance`
//! via the biome params sidecar): `0` disables the column, `>= 1` smooths
//! single-voxel steps. Values above 1 are reserved for future multi-distance
//! step distribution (a heightfield relaxation, which this formulation is the
//! substrate for) and currently behave as 1. The pass runs per chunk in
//! isolation — out-of-chunk neighbor columns are unknown and never trigger a
//! demotion, and the top and bottom in-chunk layers are skipped entirely (their
//! exposure and step targets live in the chunks above/below); the cross-chunk
//! seam pass (`world::seam`) adds those boundary demotions once real neighbors
//! are resident.

use super::chunk::{Chunk, CHUNK_SIZE};
use super::storage::ChunkStorage;
use voxel_core::{ShapeId, Voxel};

/// Number of `(x, z)` columns in a chunk - the length [`smooth_slabs`] expects
/// for its per-column `distances` slice.
pub const COLUMN_COUNT: usize = CHUNK_SIZE * CHUNK_SIZE;

/// A column's surface: the flat index and shape of its topmost solid voxel.
type Surface = Option<(usize, ShapeId)>;

/// Smooth single-cube surface steps in place: demote each surface cube that
/// borders an in-chunk neighbor column whose surface is exactly one cell lower
/// into a `SlabBottom`.
pub fn smooth_slabs(storage: &mut ChunkStorage, distances: &[u32]) {
    debug_assert_eq!(distances.len(), COLUMN_COUNT, "one smoothing distance per column");
    if distances.iter().all(|&d| d == 0) {
        return; // No column is smoothed (e.g. every present biome is flat plains).
    }
    // A uniform chunk is all one voxel: either no surface (air) or a flat top
    // (solid) - no steps either way. Early-out keeps Uniform chunks Uniform.
    if storage.is_uniform() {
        return;
    }

    // The immutable surface heightfield: topmost solid voxel per column
    // (above-the-chunk is treated as air, matching the isolated-pass convention).
    let heights = surface_heights(storage);

    // Decide every demotion from the heightfield, then apply - independent of
    // iteration order by construction.
    let mut demotions: Vec<(usize, Voxel)> = Vec::new();
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            if distances[x + z * CHUNK_SIZE] == 0 {
                continue;
            }
            let Some((sy, shape)) = heights[x + z * CHUNK_SIZE] else {
                continue; // no surface in this column
            };
            // The top in-chunk cell's exposure is unknowable in isolation: if the
            // terrain continues into the chunk above, this cell is buried, and
            // demoting it would stack a false slab under the upper chunk's real
            // surface slab. Symmetric to the sy == 0 guard below: the seam pass
            // owns both chunk-Y-boundary layers, deciding with real neighbors.
            if sy == CHUNK_SIZE - 1 {
                continue;
            }
            // Only full cubes are smoothed; authored slabs are left as-is.
            if shape != ShapeId::Cube {
                continue;
            }
            if has_lower_neighbor_surface(&heights, x, sy, z) {
                let index = Chunk::voxel_index(x, sy, z);
                let here = storage.voxel(index);
                demotions.push((
                    index,
                    Voxel { material: here.material, shape: ShapeId::SlabBottom, flags: here.flags },
                ));
            }
        }
    }

    for (index, voxel) in demotions {
        storage.set_voxel(index, voxel);
    }
}

/// Per-column surface heightfield: `(y, shape)` of the topmost solid voxel, or
/// `None` for an all-air column.
fn surface_heights(storage: &ChunkStorage) -> Box<[Surface; COLUMN_COUNT]> {
    let mut heights: Box<[Surface; COLUMN_COUNT]> = Box::new([None; COLUMN_COUNT]);
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            for y in (0..CHUNK_SIZE).rev() {
                let v = storage.voxel(Chunk::voxel_index(x, y, z));
                if v.is_solid() {
                    heights[x + z * CHUNK_SIZE] = Some((y, v.shape));
                    break;
                }
            }
        }
    }
    heights
}

/// True if any of the four horizontal neighbor columns has its surface exactly
/// one cell below `sy` on a standable top (`Cube` or `SlabBottom` - a `SlabTop`'s
/// top is at the cell ceiling, a full step, so it does not soften this one).
/// Out-of-chunk neighbors are unknown and never trigger a demotion here; the
/// seam pass (`world::seam`) owns those.
fn has_lower_neighbor_surface(
    heights: &[Surface; COLUMN_COUNT],
    x: usize,
    sy: usize,
    z: usize,
) -> bool {
    if sy == 0 {
        return false; // no in-chunk cell below; the seam pass sees the chunk beneath
    }
    const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    DIRS.iter().any(|&(dx, dz)| {
        let nx = x as i32 + dx;
        let nz = z as i32 + dz;
        if nx < 0 || nx >= CHUNK_SIZE as i32 || nz < 0 || nz >= CHUNK_SIZE as i32 {
            return false;
        }
        matches!(
            heights[nx as usize + nz as usize * CHUNK_SIZE],
            Some((ny, ShapeId::Cube | ShapeId::SlabBottom)) if ny == sy - 1
        )
    })
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

    /// A per-column distances slice with the same distance everywhere.
    fn uniform(distance: u32) -> [u32; COLUMN_COUNT] {
        [distance; COLUMN_COUNT]
    }

    #[test]
    fn single_step_high_side_becomes_slab() {
        let mut s = storage_with(|a| {
            set(a, 4, 4, 4, cube()); // high surface at y=4
            set(a, 5, 3, 4, cube()); // neighbor surface one lower
        });
        smooth_slabs(&mut s, &uniform(1));
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
        smooth_slabs(&mut s, &uniform(1));
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
        smooth_slabs(&mut s, &uniform(1));
        assert_eq!(shape_at(&s, 4, 5, 4), ShapeId::Cube, "2-cube cliff must stay sharp");
    }

    #[test]
    fn authored_slab_is_respected() {
        let mut s = storage_with(|a| {
            set(a, 4, 4, 4, slab_bottom()); // already a slab on the high side
            set(a, 5, 3, 4, cube());
        });
        smooth_slabs(&mut s, &uniform(1));
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
        smooth_slabs(&mut a, &uniform(1));
        smooth_slabs(&mut b, &uniform(1));
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
        smooth_slabs(&mut s, &uniform(0));
        assert_eq!(shape_at(&s, 4, 4, 4), ShapeId::Cube, "distance 0 leaves cubes untouched");
    }

    #[test]
    fn per_column_distance_gates_smoothing() {
        // Two identical 1-cube steps in different columns; only the column whose
        // distance is >= 1 gets smoothed.
        let mut s = storage_with(|a| {
            set(a, 4, 4, 4, cube()); // column (4, 4)
            set(a, 5, 3, 4, cube());
            set(a, 10, 4, 10, cube()); // column (10, 10)
            set(a, 11, 3, 10, cube());
        });
        let mut d = uniform(0);
        d[4 + 4 * CHUNK_SIZE] = 1; // enable smoothing only for column (4, 4)
        smooth_slabs(&mut s, &d);
        assert_eq!(shape_at(&s, 4, 4, 4), ShapeId::SlabBottom, "enabled column smooths");
        assert_eq!(shape_at(&s, 10, 4, 10), ShapeId::Cube, "disabled column stays a cube");
    }

    #[test]
    fn interior_surface_under_roof_is_left_sharp() {
        // A cave-like step under an intact roof: the roof is the column surface,
        // so the interior floor step is not smoothed (top-surface-only rule).
        let mut s = storage_with(|a| {
            set(a, 4, 8, 4, cube()); // roof over column (4,4)
            set(a, 5, 8, 4, cube()); // roof over column (5,4)
            set(a, 4, 4, 4, cube()); // interior floor, high side
            set(a, 5, 3, 4, cube()); // interior floor, one lower
        });
        smooth_slabs(&mut s, &uniform(1));
        assert_eq!(shape_at(&s, 4, 4, 4), ShapeId::Cube, "interior floor stays sharp");
        assert_eq!(shape_at(&s, 4, 8, 4), ShapeId::Cube, "flat roof stays sharp");
    }
    #[test]
    fn top_layer_step_is_deferred_to_seam_pass() {
        // A 1-step whose high side is the top in-chunk cell: in isolation its
        // exposure is unknowable (the real surface may be in the chunk above,
        // making this cell buried), so the isolated pass leaves it sharp and the
        // seam pass decides with the actual chunk above resident.
        let top = CHUNK_SIZE - 1;
        let mut s = storage_with(|a| {
            set(a, 4, top, 4, cube());     // high side at the chunk-Y boundary
            set(a, 5, top - 1, 4, cube()); // neighbor surface one lower
        });
        smooth_slabs(&mut s, &uniform(1));
        assert_eq!(shape_at(&s, 4, top, 4), ShapeId::Cube, "top layer defers to the seam pass");
    }
}
