//! Naive cube mesher: one quad per exposed voxel face.
//!
//! For each solid voxel in the chunk, emit a quad on every face whose
//! face-adjacent neighbor (looked up in the snapshot's 1-voxel border) is air.
//! No greedy merging, no AO, no edge dedup - the cleanest possible baseline.

use crate::meshing::MaterialConfig;
use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::{ChunkSnapshot, CHUNK_SIZE, CHUNK_WORLD_SIZE, SNAP_PAD, SNAP_SIZE};
use voxel_core::{ShapeId, Voxel};

/// Six axis-aligned face directions: (normal, four CCW corner offsets in [0,1]^3).
const FACES: [([f32; 3], [[f32; 3]; 4]); 6] = [
    // +X (right)
    ([ 1.0, 0.0, 0.0],
     [[1.0,0.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[1.0,0.0,1.0]]),
    // -X (left)
    ([-1.0, 0.0, 0.0],
     [[0.0,0.0,1.0],[0.0,1.0,1.0],[0.0,1.0,0.0],[0.0,0.0,0.0]]),
    // +Y (top)
    ([ 0.0, 1.0, 0.0],
     [[0.0,1.0,0.0],[0.0,1.0,1.0],[1.0,1.0,1.0],[1.0,1.0,0.0]]),
    // -Y (bottom)
    ([ 0.0,-1.0, 0.0],
     [[0.0,0.0,0.0],[1.0,0.0,0.0],[1.0,0.0,1.0],[0.0,0.0,1.0]]),
    // +Z (front)
    ([ 0.0, 0.0, 1.0],
     [[0.0,0.0,1.0],[1.0,0.0,1.0],[1.0,1.0,1.0],[0.0,1.0,1.0]]),
    // -Z (back)
    ([ 0.0, 0.0,-1.0],
     [[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,0.0,0.0],[0.0,0.0,0.0]]),
];

/// Offset of each face's neighbor cell (matches FACES order).
const NEIGHBOR_OFFSETS: [[i32; 3]; 6] = [
    [ 1, 0, 0], [-1, 0, 0],
    [ 0, 1, 0], [ 0,-1, 0],
    [ 0, 0, 1], [ 0, 0,-1],
];

#[inline]
fn snap_voxel(snap: &ChunkSnapshot, x: i32, y: i32, z: i32) -> Voxel {
    let sx = (x + SNAP_PAD as i32) as usize;
    let sy = (y + SNAP_PAD as i32) as usize;
    let sz = (z + SNAP_PAD as i32) as usize;
    snap.materials[sx + sy * SNAP_SIZE + sz * SNAP_SIZE * SNAP_SIZE]
}

/// Vertical extent `[y0, y1]` a shape occupies within its unit cell.
#[inline]
fn shape_y_interval(shape: ShapeId) -> (f32, f32) {
    match shape {
        ShapeId::Cube => (0.0, 1.0),
        ShapeId::SlabBottom => (0.0, 0.5),
        ShapeId::SlabTop => (0.5, 1.0),
        ShapeId::Empty => (0.0, 0.0),
    }
}

/// True if `neighbor` fully spans the interval `[y0, y1]` (occludes a side face).
#[inline]
fn covers_interval(neighbor: ShapeId, y0: f32, y1: f32) -> bool {
    let (ny0, ny1) = shape_y_interval(neighbor);
    ny0 <= y0 && ny1 >= y1
}

/// True if `shape` fills the lower half of its cell (occludes the top face below it).
#[inline]
fn covers_bottom(shape: ShapeId) -> bool {
    matches!(shape, ShapeId::Cube | ShapeId::SlabBottom)
}

/// True if `shape` fills the upper half of its cell (occludes the bottom face above it).
#[inline]
fn covers_top(shape: ShapeId) -> bool {
    matches!(shape, ShapeId::Cube | ShapeId::SlabTop)
}

pub fn generate_chunk_mesh(
    snap: &ChunkSnapshot,
    mat_config: &MaterialConfig,
) -> (Vec<TerrainVertex>, Vec<u32>) {
    // Worst case: 6 faces * 4 verts per solid voxel. Reserve modestly.
    let mut vertices: Vec<TerrainVertex> = Vec::with_capacity(8192);
    let mut indices: Vec<u32> = Vec::with_capacity(12_288);

    let origin = [
        snap.position.x as f32 * CHUNK_WORLD_SIZE,
        snap.position.y as f32 * CHUNK_WORLD_SIZE,
        snap.position.z as f32 * CHUNK_WORLD_SIZE,
    ];

    for z in 0..CHUNK_SIZE as i32 {
        for y in 0..CHUNK_SIZE as i32 {
            for x in 0..CHUNK_SIZE as i32 {
                let v = snap_voxel(snap, x, y, z);
                if !v.is_solid() { continue; }

                let color = mat_config
                    .colors
                    .get(v.material.0 as usize)
                    .copied()
                    .unwrap_or([1.0, 0.0, 1.0]);

                // Vertical extent of this shape; corner Y is remapped onto it below.
                let (y0, y1) = shape_y_interval(v.shape);

                for (face_idx, (normal, corners)) in FACES.iter().enumerate() {
                    let [dx, dy, dz] = NEIGHBOR_OFFSETS[face_idx];
                    let neighbor = snap_voxel(snap, x + dx, y + dy, z + dz);
                    let nshape = neighbor.shape;

                    // FACES order: 0:+X 1:-X 2:+Y 3:-Y 4:+Z 5:-Z.
                    let emit = match face_idx {
                        // Top face at y1: an internal slab top (y1 < 1.0) is always
                        // exposed; a full-height top is hidden only if the cell above
                        // fills its lower half.
                        2 => y1 < 1.0 || !covers_bottom(nshape),
                        // Bottom face at y0: an internal slab bottom (y0 > 0.0) is always
                        // exposed; a floor-level bottom is hidden only if the cell below
                        // fills its upper half.
                        3 => y0 > 0.0 || !covers_top(nshape),
                        // Side faces span [y0, y1]; hidden only if the neighbor covers it.
                        _ => !covers_interval(nshape, y0, y1),
                    };
                    if !emit { continue; }

                    let base = vertices.len() as u32;
                    for c in corners {
                        let cy = if c[1] == 0.0 { y0 } else { y1 };
                        vertices.push(TerrainVertex {
                            position: [
                                origin[0] + x as f32 + c[0],
                                origin[1] + y as f32 + cy,
                                origin[2] + z as f32 + c[2],
                            ],
                            normal: *normal,
                            color,
                            ao: 1.0,
                            material_id: v.material.0 as u32,
                            cell_flags: 0,
                            _pad_vert: [0; 2],
                        });
                    }
                    // Two triangles per quad (0,1,2) and (0,2,3)
                    indices.extend_from_slice(&[
                        base, base + 1, base + 2,
                        base, base + 2, base + 3,
                    ]);
                }
            }
        }
    }

    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::SNAP_VOLUME;
    use glam::IVec3;
    use voxel_core::MaterialId;

    const STONE: MaterialId = MaterialId(1);

    fn empty_snapshot() -> ChunkSnapshot {
        let materials: Box<[Voxel; SNAP_VOLUME]> =
            vec![Voxel::EMPTY; SNAP_VOLUME].into_boxed_slice().try_into().unwrap();
        ChunkSnapshot { position: IVec3::ZERO, materials, border_min: [false; 3] }
    }

    fn put(snap: &mut ChunkSnapshot, x: usize, y: usize, z: usize, v: Voxel) {
        let idx = ChunkSnapshot::snap_index(x + SNAP_PAD, y + SNAP_PAD, z + SNAP_PAD);
        snap.materials[idx] = v;
    }

    fn config() -> MaterialConfig {
        MaterialConfig { colors: vec![[0.0, 0.0, 0.0], [0.5, 0.5, 0.5]] }
    }

    fn slab(material: MaterialId, shape: ShapeId) -> Voxel {
        Voxel { material, shape, flags: 0 }
    }

    #[test]
    fn slab_bottom_top_face_sits_at_midline() {
        let mut snap = empty_snapshot();
        put(&mut snap, 0, 0, 0, slab(STONE, ShapeId::SlabBottom));
        let (verts, _idx) = generate_chunk_mesh(&snap, &config());

        let has_top_midline = verts
            .iter()
            .any(|v| v.normal == [0.0, 1.0, 0.0] && (v.position[1] - 0.5).abs() < 1e-5);
        assert!(has_top_midline, "slab-bottom top face should be at Y=0.5");
        assert!(
            verts.iter().all(|v| v.position[1] <= 0.5 + 1e-5),
            "slab-bottom geometry must not exceed Y=0.5"
        );
    }

    #[test]
    fn slab_top_bottom_face_sits_at_midline() {
        let mut snap = empty_snapshot();
        put(&mut snap, 0, 0, 0, slab(STONE, ShapeId::SlabTop));
        let (verts, _idx) = generate_chunk_mesh(&snap, &config());

        let has_bottom_midline = verts
            .iter()
            .any(|v| v.normal == [0.0, -1.0, 0.0] && (v.position[1] - 0.5).abs() < 1e-5);
        assert!(has_bottom_midline, "slab-top bottom face should be at Y=0.5");
        assert!(
            verts.iter().all(|v| v.position[1] >= 0.5 - 1e-5),
            "slab-top geometry must not drop below Y=0.5"
        );
    }

    #[test]
    fn cube_top_face_sits_at_full_height() {
        let mut snap = empty_snapshot();
        put(&mut snap, 0, 0, 0, Voxel::cube(STONE));
        let (verts, _idx) = generate_chunk_mesh(&snap, &config());

        let has_top_full = verts
            .iter()
            .any(|v| v.normal == [0.0, 1.0, 0.0] && (v.position[1] - 1.0).abs() < 1e-5);
        assert!(has_top_full, "cube top face should be at Y=1.0");
    }

    #[test]
    fn cube_against_cube_culls_shared_face() {
        // Two stacked cubes: the lower cube's top and upper cube's bottom are hidden.
        let mut snap = empty_snapshot();
        put(&mut snap, 0, 0, 0, Voxel::cube(STONE));
        put(&mut snap, 0, 1, 0, Voxel::cube(STONE));
        let (verts, _idx) = generate_chunk_mesh(&snap, &config());

        // No +Y face at Y=1.0 (the interface between the two cubes is culled).
        let interface_top = verts
            .iter()
            .any(|v| v.normal == [0.0, 1.0, 0.0] && (v.position[1] - 1.0).abs() < 1e-5);
        assert!(!interface_top, "shared cube interface should be culled");
    }
}
