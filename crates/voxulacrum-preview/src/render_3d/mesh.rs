//! Per-voxel face-culled mesher. Dispatches each non-empty voxel to its
//! per-`(ShapeId, Rotation)` triangle table from `shape_table.rs`; emits a
//! triangle either unconditionally (slope surfaces, cut faces) or when the
//! named face-neighbor is air.

use bytemuck::{Pod, Zeroable};
use voxel_core::{ChunkBuffer, Voxel};

use crate::material_colors::material_color;
use crate::render_3d::shape_table::{covers_face, shape_geometry, FaceCondition, NeighborDir};

/// One vertex of one triangle. Three floats position, three normal, three color.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct FaceVertex {
    pub position: [f32; 3],
    pub normal:   [f32; 3],
    pub color:    [f32; 3],
}

/// CPU-side mesh: vertices for `wgpu::PrimitiveTopology::TriangleList`.
pub struct ChunkMesh {
    pub vertices: Vec<FaceVertex>,
}

impl ChunkMesh {
    pub fn vertex_count(&self) -> u32 { self.vertices.len() as u32 }
}

const N: usize = 32;

/// Emit a face-culled mesh for `chunk`. Each non-empty voxel contributes
/// its shape's triangles, with conditional faces culled against the
/// neighbor's solidity (out-of-bounds counts as air).
pub fn build_chunk_mesh(chunk: &ChunkBuffer<Voxel, 32>) -> ChunkMesh {
    let mut verts: Vec<FaceVertex> = Vec::with_capacity(8 * 1024);

    for z in 0..N {
        for y in 0..N {
            for x in 0..N {
                let v = chunk.get(x, y, z);
                if !v.is_solid() { continue; }
                let color = material_color(v.material);
                let base = [x as f32, y as f32, z as f32];

                for tri in shape_geometry(v.shape, v.rotation) {
                    let draw = match tri.condition {
                        FaceCondition::Always => true,
                        FaceCondition::WhenAirAt(dir) => {
                            !neighbor_blocks_face(chunk, x as i32, y as i32, z as i32, dir)
                        }
                    };
                    if !draw { continue; }
                    for &vert in &tri.verts {
                        verts.push(FaceVertex {
                            position: [
                                base[0] + vert[0],
                                base[1] + vert[1],
                                base[2] + vert[2],
                            ],
                            normal: tri.normal,
                            color,
                        });
                    }
                }
            }
        }
    }

    ChunkMesh { vertices: verts }
}

/// True iff the neighbor cell at `(x+dir.x, y+dir.y, z+dir.z)` is in-bounds,
/// solid, AND its face facing back at us is a full 1×1 quad (per
/// [`covers_face`]).
///
/// Stricter than a plain `is_solid` check: an outer-corner neighbor whose
/// `-Z` side is a half-triangle does NOT block our `+Z` cube face. Without
/// this, the cube on the back side of the corner culls its face and leaves
/// a visible hole peeking through the corner's half-triangle.
#[inline]
fn neighbor_blocks_face(
    buf: &ChunkBuffer<Voxel, 32>,
    x: i32, y: i32, z: i32,
    dir: NeighborDir,
) -> bool {
    let [dx, dy, dz] = dir.offset();
    let (nx, ny, nz) = (x + dx, y + dy, z + dz);
    if !(0..N as i32).contains(&nx)
        || !(0..N as i32).contains(&ny)
        || !(0..N as i32).contains(&nz)
    {
        return false;
    }
    let n = buf.get(nx as usize, ny as usize, nz as usize);
    if !n.is_solid() {
        return false;
    }
    covers_face(n.shape, n.rotation, opposite_dir(dir))
}

#[inline]
fn opposite_dir(d: NeighborDir) -> NeighborDir {
    use NeighborDir::*;
    match d {
        PosX => NegX, NegX => PosX,
        PosY => NegY, NegY => PosY,
        PosZ => NegZ, NegZ => PosZ,
    }
}
