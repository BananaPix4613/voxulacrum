//! Prop stamping: write tree / prefab voxels into a terrain buffer at scanned
//! surface points, clipping anything outside the chunk's `[0, N)` bounds.

use nodegraph_ir::{PlaceTreeParams, PrefabTemplate};
use voxel_core::{ChunkBuffer, Voxel};

use crate::field::CHUNK_DIM;
use crate::scatter::{ScatterPoint, SplitMix64};

const N: i32 = CHUNK_DIM as i32;

/// Stamp a trunk + spherical canopy at every point that has a surface.
/// Returns a new buffer (input cloned). Leaves overwrite only air; trunk
/// overwrites unconditionally. Out-of-bounds voxels are clipped.
pub(crate) fn place_trees(
    terrain: &ChunkBuffer<Voxel, 32>,
    points: &[ScatterPoint],
    p: &PlaceTreeParams,
) -> ChunkBuffer<Voxel, 32> {
    let mut out = terrain.clone();
    out.make_dense();
    for &pt in points {
        let Some(sy) = pt.surface_y else { continue; };
        let mut rng = SplitMix64::new(pt.seed ^ (p.seed as u64).wrapping_mul(0x9E3779B97F4A7C15));
        let lo = p.trunk_min;
        let hi = p.trunk_max.max(lo);
        let trunk_h = (lo + (rng.next_u64() % (hi - lo + 1) as u64) as u32) as i32;
        let bx = pt.lx.round() as i32;
        let bz = pt.lz.round() as i32;
        let trunk_top = sy + trunk_h;
        for y in (sy + 1)..=trunk_top {
            set_clipped(&mut out, bx, y, bz, Voxel::cube(p.trunk_material), false);
        }
        let r = p.canopy_radius as i32;
        let cy = trunk_top;
        for dy in -r..=r {
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy + dz * dz > r * r { continue; }
                    set_clipped(&mut out, bx + dx, cy + dy, bz + dz,
                                Voxel::cube(p.leaf_material), true);
                }
            }
        }
    }
    out.try_collapse();
    out
}


/// Stamp a prefab template at every point with a surface. The template's
/// `anchor` voxel lands one cell above the surface on the point's column.
/// Solid template voxels overwrite unconditionally; clipped at chunk bounds.
pub(crate) fn place_prefabs(
    terrain: &ChunkBuffer<Voxel, 32>,
    points: &[ScatterPoint],
    template: &PrefabTemplate,
) -> ChunkBuffer<Voxel, 32> {
    let mut out = terrain.clone();
    out.make_dense();
    for &pt in points {
        let Some(sy) = pt.surface_y else { continue; };
        let bx = pt.lx.round() as i32 - template.anchor[0];
        let by = sy + 1 - template.anchor[1];
        let bz = pt.lz.round() as i32 - template.anchor[2];
        for pv in &template.voxels {
            set_clipped(&mut out, bx + pv.at[0], by + pv.at[1], bz + pv.at[2], pv.voxel, false);
        }
    }
    out.try_collapse();
    out
}

/// Write a voxel if in-bounds. When `air_only`, skip already-solid cells.
fn set_clipped(out: &mut ChunkBuffer<Voxel, 32>, x: i32, y: i32, z: i32, v: Voxel, air_only: bool) {
    if x < 0 || y < 0 || z < 0 || x >= N || y >= N || z >= N { return; }
    if air_only && out.get(x as usize, y as usize, z as usize).is_solid() { return; }
    out.set(x as usize, y as usize, z as usize, v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodegraph_ir::PrefabVoxel;
    use voxel_core::MaterialId;

    fn floor(top: i32) -> ChunkBuffer<Voxel, 32> {
        let mut b: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        for z in 0..CHUNK_DIM { for x in 0..CHUNK_DIM {
            for y in 0..=(top as usize) { b.set(x, y, z, Voxel::cube(MaterialId(6))); }
        }}
        b
    }

    fn scanned(lx: f32, lz: f32, sy: i32) -> ScatterPoint {
        ScatterPoint { lx, lz, surface_y: Some(sy), seed: 99 }
    }

    #[test]
    fn tree_stamps_trunk_above_surface() {
        let t = floor(8);
        let p = PlaceTreeParams::default();
        let out = place_trees(&t, &[scanned(16.0, 16.0, 8)], &p);
        // Cell directly above the surface is trunk material.
        assert_eq!(out.get(16, 9, 16).material, p.trunk_material);
        // Surface itself is unchanged grass.
        assert_eq!(out.get(16, 8, 16).material, MaterialId(6));
    }

    #[test]
    fn prefab_stamps_and_clips_at_border() {
        let t = floor(8);
        // One-voxel prefab at the origin, anchored at origin.
        let tpl = PrefabTemplate {
            anchor: [0, 0, 0],
            voxels: vec![PrefabVoxel { at: [0, 0, 0], voxel: Voxel::cube(MaterialId(2)) }],
        };
        // In-bounds point lands granite at (16, 9, 16).
        let out = place_prefabs(&t, &[scanned(16.0, 16.0, 8)], &tpl);
        assert_eq!(out.get(16, 9, 16).material, MaterialId(2));
        // A point outside [0,N) is fully clipped (no panic, no write). The
        // cell at (0,9,16) sits above the floor top (y=8), so it stays air.
        let out2 = place_prefabs(&t, &[scanned(-5.0, 16.0, 8)], &tpl);
        assert_eq!(out2.get(0, 9, 16).material, MaterialId(0)); // air, unchanged
    }
}
