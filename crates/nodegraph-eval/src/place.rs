//! Prop stamping: write blueprint voxels into a terrain buffer at scanned
//! surface points, clipping anything outside the chunk's `[0, N)` bounds.

use voxel_core::{ChunkBuffer, ResolvedBlueprint, Voxel};

use crate::field::CHUNK_DIM;
use crate::scatter::ScatterPoint;

const N: i32 = CHUNK_DIM as i32;

/// Stamp a resolved blueprint at every point with a surface. The blueprint's
/// `anchor` cell lands one cell above the surface on the point's column.
/// Solid cells overwrite unconditionally; clipped at chunk bounds.
pub(crate) fn place_blueprints(
    terrain: &ChunkBuffer<Voxel, 32>,
    points: &[ScatterPoint],
    blueprint: &ResolvedBlueprint,
) -> ChunkBuffer<Voxel, 32> {
    let mut out = terrain.clone();
    out.make_dense();
    for &pt in points {
        let Some(sy) = pt.surface_y else { continue; };
        let bx = pt.lx.round() as i32 - blueprint.anchor[0];
        let by = sy + 1 - blueprint.anchor[1];
        let bz = pt.lz.round() as i32 - blueprint.anchor[2];
        for &(at, voxel) in &blueprint.cells {
            set_clipped(&mut out, bx + at[0], by + at[1], bz + at[2], voxel, false);
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
    use voxel_core::{DestructionPolicy, MaterialId};

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
    fn blueprint_stamps_and_clips_at_border() {
        let t = floor(8);
        // One-cell blueprint at the origin, anchored at origin.
        let bp = ResolvedBlueprint {
            name: "test".to_string(),
            anchor: [0, 0, 0],
            cells: vec![([0, 0, 0], Voxel::cube(MaterialId(2)))],
            destruction: DestructionPolicy::Destroy,
            protected_volume: false,
        };
        // In-bounds point lands granite at (16, 9, 16).
        let out = place_blueprints(&t, &[scanned(16.0, 16.0, 8)], &bp);
        assert_eq!(out.get(16, 9, 16).material, MaterialId(2));
        // A point outside [0,N) is fully clipped (no panic, no write). The
        // cell at (0,9,16) sits above the floor top (y=8), so it stays air.
        let out2 = place_blueprints(&t, &[scanned(-5.0, 16.0, 8)], &bp);
        assert_eq!(out2.get(0, 9, 16).material, MaterialId(0)); // air, unchanged
    }
}
