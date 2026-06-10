//! Naive cube mesher: one quad per exposed voxel face.
//!
//! For each solid voxel in the chunk, emit a quad on every face whose
//! face-adjacent neighbor (looked up in the snapshot's 1-voxel border) is air.
//! No greedy merging, no AO, no edge dedup - the cleanest possible baseline.

use crate::meshing::MaterialConfig;
use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::{ChunkSnapshot, CHUNK_SIZE, CHUNK_WORLD_SIZE, SNAP_PAD, SNAP_SIZE};
use voxel_core::Voxel;

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
                // Any solid shape renders as a full cube in Phase 0; slab faces are Phase 3.

                let color = mat_config
                    .colors
                    .get(v.material.0 as usize)
                    .copied()
                    .unwrap_or([1.0, 0.0, 1.0]);

                for (face_idx, (normal, corners)) in FACES.iter().enumerate() {
                    let [dx, dy, dz] = NEIGHBOR_OFFSETS[face_idx];
                    let neighbor = snap_voxel(snap, x + dx, y + dy, z + dz);
                    if neighbor.is_solid() { continue; }

                    let base = vertices.len() as u32;
                    for c in corners {
                        vertices.push(TerrainVertex {
                            position: [
                                origin[0] + x as f32 + c[0],
                                origin[1] + y as f32 + c[1],
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
