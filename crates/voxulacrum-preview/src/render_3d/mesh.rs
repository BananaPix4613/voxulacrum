//! Per-voxel face-culled mesher. Emits cube faces for solid voxels, culling a
//! face when its axis-neighbor is solid. `SlabBottom`/`SlabTop` emit a
//! half-height box (Y range [0,0.5] / [0.5,1]) and always draw their faces.

use bytemuck::{Pod, Zeroable};
use voxel_core::{ChunkBuffer, ShapeId, Voxel};

use crate::material_colors::material_color;

/// One vertex: position, normal, color. Consumed as `TriangleList`.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct FaceVertex {
    pub position: [f32; 3],
    pub normal:   [f32; 3],
    pub color:    [f32; 3],
}
/// CPU-side mesh.
pub struct ChunkMesh {
    pub vertices: Vec<FaceVertex>,
}

impl ChunkMesh {
    pub fn vertex_count(&self) -> u32 { self.vertices.len() as u32 }
}

const N: usize = 32;

/// (normal, neighbor offset, 4 CCW corners of the unit-cube face). Corner y is
/// in {0,1}; slabs rescale y into a half-height range.
const FACES: [([f32; 3], [i32; 3], [[f32; 3]; 4]); 6] = [
    ([ 1.0, 0.0, 0.0], [ 1, 0, 0], [[1.0,0.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[1.0,0.0,1.0]]),
    ([-1.0, 0.0, 0.0], [-1, 0, 0], [[0.0,0.0,1.0],[0.0,1.0,1.0],[0.0,1.0,0.0],[0.0,0.0,0.0]]),
    ([ 0.0, 1.0, 0.0], [ 0, 1, 0], [[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[0.0,1.0,1.0]]),
    ([ 0.0,-1.0, 0.0], [ 0,-1, 0], [[0.0,0.0,1.0],[1.0,0.0,1.0],[1.0,0.0,0.0],[0.0,0.0,0.0]]),
    ([ 0.0, 0.0, 1.0], [ 0, 0, 1], [[1.0,0.0,1.0],[1.0,1.0,1.0],[0.0,1.0,1.0],[0.0,0.0,1.0]]),
    ([ 0.0, 0.0,-1.0], [ 0, 0,-1], [[0.0,0.0,0.0],[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,0.0,0.0]]),
];

/// Two triangles per quad: corners (0,1,2) and (0,2,3).
const TRI: [usize; 6] = [0, 1, 2, 0, 2, 3];

pub fn build_chunk_mesh(chunk: &ChunkBuffer<Voxel, 32>) -> ChunkMesh {
    let mut verts: Vec<FaceVertex> = Vec::with_capacity(8 * 1024);

    for z in 0..N {
        for y in 0..N {
            for x in 0..N {
                let v = chunk.get(x, y, z);
                if !v.is_solid() { continue; }
                let color = material_color(v.material);
                let (y_lo, y_hi) = match v.shape {
                    ShapeId::SlabBottom => (0.0, 0.5),
                    ShapeId::SlabTop    => (0.5, 1.0),
                    _                   => (0.0, 1.0),
                };
                let base = [x as f32, y as f32, z as f32];
                let cull = !v.shape.is_slab();

                for (normal, off, corners) in FACES.iter() {
                    if cull
                        && neighbor_is_solid(
                        chunk,
                        x as i32 + off[0],
                        y as i32 + off[1],
                        z as i32 + off[2],
                    )
                    {
                        continue;
                    }
                    for &ci in &TRI {
                        let c = corners[ci];
                        let cy = y_lo + c[1] * (y_hi - y_lo);
                        verts.push(FaceVertex {
                            position: [base[0] + c[0], base[1] + cy, base[2] + c[2]],
                            normal: *normal,
                            color,
                        });
                    }
                }
            }
        }
    }

    ChunkMesh { vertices: verts }
}

#[inline]
fn neighbor_is_solid(buf: &ChunkBuffer<Voxel, 32>, nx: i32, ny: i32, nz: i32) -> bool {
    if !(0..N as i32).contains(&nx)
        || !(0..N as i32).contains(&ny)
        || !(0..N as i32).contains(&nz)
    {
        return false;
    }
    buf.get(nx as usize, ny as usize, nz as usize).is_solid()
}
