//! Face-culled cube mesher.

use bytemuck::{Pod, Zeroable};
use voxel_core::{ChunkBuffer, ShapeId, Voxel};

use crate::material_colors::material_color;

/// One vertex of one cube face. Three floats position, three normal, three color.
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
    pub fn vertex_count(&self) -> u32 { self.vertices.len() as u32}
}

// One face quad is two triangles drawn from the in-loop `quad` table below;
// six faces, with `(neighbor offset, quad corner, u-basis, v-basis, normal)`
// entries below `build_chunk_mesh`.

/// Emit a face-culled mesh for `chunk`. Faces between two solid voxels
/// are skipped; only voxels with `ShapeId::Cube` produce faces.
pub fn build_chunk_mesh(chunk: &ChunkBuffer<Voxel, 32>) -> ChunkMesh {
    let mut verts = Vec::new();

    let solid = |x: i32, y: i32, z: i32| -> bool {
        if !(0..32).contains(&x) || !(0..32).contains(&y) || !(0..32).contains(&z) {
            return false;
        }
        chunk.get(x as usize, y as usize, z as usize).shape == ShapeId::Cube
    };

    // (neighbor_offset, quad_corner, u_basis, v_basis, normal)
    //
    // Each face's `corner` sits at the low corner of that face within the
    // unit cube, and `u_basis + v_basis` keep all four vertices inside
    // [0,1]³. The winding is CCW from outside the cube, i.e.
    // `u_basis × v_basis == normal`. The pipeline uses
    // FrontFace::Ccw + cull_mode=Back, so front faces face the camera and
    // back faces cull cleanly.
    let faces: &[([i32; 3], [f32; 3], [f32; 3], [f32; 3], [f32; 3])] = &[
        // +X (right, x=1):  u=+Y, v=+Z  →  Y×Z = +X
        ([ 1, 0, 0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [ 1.0, 0.0, 0.0]),
        // -X (left,  x=0):  u=+Z, v=+Y  →  Z×Y = -X
        ([-1, 0, 0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]),
        // +Y (top,   y=1):  u=+Z, v=+X  →  Z×X = +Y
        ([ 0, 1, 0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [ 0.0, 1.0, 0.0]),
        // -Y (bot,   y=0):  u=+X, v=+Z  →  X×Z = -Y
        ([ 0,-1, 0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [ 0.0,-1.0, 0.0]),
        // +Z (front, z=1):  u=+X, v=+Y  →  X×Y = +Z
        ([ 0, 0, 1], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [ 0.0, 0.0, 1.0]),
        // -Z (back,  z=0):  u=+Y, v=+X  →  Y×X = -Z
        ([ 0, 0,-1], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [ 0.0, 0.0,-1.0]),
    ];

    for z in 0..32 {
        for y in 0..32 {
            for x in 0..32{
                let v = chunk.get(x, y, z);
                if v.shape != ShapeId::Cube { continue; }
                let color = material_color(v.material);
                let base = [x as f32, y as f32, z as f32];

                for (off, corner, bu, bv, n) in faces {
                    if solid(x as i32 + off[0], y as i32 + off[1], z as i32 + off[2]) {
                        continue;
                    }
                    // Two triangles per face: (0,1,2) and (0,2,3) of FACE_VERTS' u-v.
                    let quad = [
                        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]
                    ];
                    let tris = [0, 1, 2, 0, 2, 3];
                    for &i in &tris {
                        let (u, v_) = (quad[i][0], quad[i][1]);
                        let p = [
                            base[0] + corner[0] + bu[0] * u + bv[0] * v_,
                            base[1] + corner[1] + bu[1] * u + bv[1] * v_,
                            base[2] + corner[2] + bu[2] * u + bv[2] * v_,
                        ];
                        verts.push(FaceVertex { position: p, normal: *n, color });
                    }
                }
            }
        }
    }
    
    ChunkMesh { vertices: verts }
}
