//! Surface scanners: annotate scatter points with the terrain surface Y and
//! filter by flatness / material.

use nodegraph_ir::FindFlatParams;
use voxel_core::{ChunkBuffer, Voxel};

use crate::field::CHUNK_DIM;
use crate::scatter::ScatterPoint;

const N: i32 = CHUNK_DIM as i32;

/// For each point: find the topmost solid voxel in its column, reject if the
/// surface material is not allowed (empty list = any), reject if the 8-
/// neighborhood is not flat to within `max_step`. Survivors get `surface_y`.
pub(crate) fn find_flat(
    terrain: &ChunkBuffer<Voxel, 32>,
    points: &[ScatterPoint],
    p: &FindFlatParams,
) -> Vec<ScatterPoint> {
    let mut out = Vec::new();
    for &pt in points {
        // Clamp to an in-bounds column. Margin-band points sample the nearest
        // edge column - a Phase-10 approximation; true neighbor-chunk surface
        // scan is Phase 11.
        let cx = (pt.lx.round() as i32).clamp(0, N - 1);
        let cz = (pt.lz.round() as i32).clamp(0, N - 1);
        let Some(sy) = topmost_solid(terrain, cx, cz) else { continue; };
        let surf = terrain.get(cx as usize, sy as usize, cz as usize);
        if !p.on_materials.is_empty() && !p.on_materials.contains(&surf.material) {
            continue;
        }
        if !neighborhood_flat(terrain, cx, cz, sy, p.max_step) { continue; }
        out.push(ScatterPoint { surface_y: Some(sy), ..pt });
    }
    out
}

fn topmost_solid(terrain: &ChunkBuffer<Voxel, 32>, x: i32, z: i32) -> Option<i32> {
    for y in (0..N).rev() {
        if terrain.get(x as usize, y as usize, z as usize).is_solid() {
            return Some(y);
        }
    }
    None
}

fn neighborhood_flat(
    terrain: &ChunkBuffer<Voxel, 32>,
    x: i32, z: i32, sy: i32, max_step: i32,
) -> bool {
    for dz in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dz == 0 { continue; }
            let (nx, nz) = (x + dx, z + dz);
            if nx < 0 || nz < 0 || nx >= N || nz >= N { continue; }
            match topmost_solid(terrain, nx, nz) {
                Some(ny) if (ny - sy).abs() <= max_step => {}
                _ => return false, // empty column or too-steep step
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::MaterialId;

    fn flat_terrain(top: i32, mat: MaterialId) -> ChunkBuffer<Voxel, 32> {
        let mut b: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        for z in 0..CHUNK_DIM { for x in 0..CHUNK_DIM {
            for y in 0..=(top as usize) { b.set(x, y, z, Voxel::cube(mat)); }
        }}
        b
    }

    fn pt(lx: f32, lz: f32) -> ScatterPoint {
        ScatterPoint { lx, lz, surface_y: None, seed: 0 }
    }

    #[test]
    fn flat_ground_passes_and_sets_surface() {
        let t = flat_terrain(10, MaterialId(6));
        let out = find_flat(&t, &[pt(16.0, 16.0)], &FindFlatParams::default());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].surface_y, Some(10));
    }

    #[test]
    fn wrong_material_rejected() {
        let t = flat_terrain(10, MaterialId(2)); // granite
        let p = FindFlatParams { max_step: 1, on_materials: vec![MaterialId(6)] };
        assert!(find_flat(&t, &[pt(16.0, 16.0)], &p).is_empty());
    }

    #[test]
    fn cliff_edge_rejected() {
        // Tall pillar surrounded by air → neighbors empty → not flat.
        let mut t: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        for y in 0..12 { t.set(16, y, 16, Voxel::cube(MaterialId(6))); }
        assert!(find_flat(&t, &[pt(16.0, 16.0)], &FindFlatParams::default()).is_empty());
    }
}
