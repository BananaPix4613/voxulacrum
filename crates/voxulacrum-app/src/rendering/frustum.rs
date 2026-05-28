use glam::{IVec3, Mat4, Vec3, Vec4};

use crate::world::chunk::CHUNK_WORLD_SIZE;

/// A half-space plane in Hessian normal form: dot(normal, P) + d = 0.
/// The normal points inward (toward the visible half-space).
/// Stored pre-normalized so that `distance()` returns true signed distance.
#[derive(Clone, Copy)]
struct Plane {
    normal: Vec3,
    d: f32,
}

impl Plane {
    /// Construct from a raw Vec4(A, B, C, D) and normalize.
    fn from_vec4(v: Vec4) -> Self {
        let len = Vec3::new(v.x, v.y, v.z).length();
        if len < 1e-10 {
            return Self {
                normal: Vec3::ZERO,
                d: 0.0,
            };
        }
        let inv = 1.0 / len;
        Self {
            normal: Vec3::new(v.x * inv, v.y * inv, v.z * inv),
            d: v.w * inv,
        }
    }
}

/// Six-plane frustum extracted from a view-projection matrix.
pub struct Frustum {
    planes: [Plane; 6],
}

impl Frustum {
    /// Extract frustum planes from a combined view-projection matrix using
    /// the Gribb/Hartmann method. Works for both perspective and orthographic
    /// projections. Planes point inward.
    /// 
    /// For wgpu's right-handed clip space where x,y in [-w, w] and z in [0, w]:
    ///   Left   = row3 + row0
    ///   Right  = row3 - row0
    ///   Bottom = row3 + row1
    ///   Top    = row3 - row1
    ///   Near   = row2          (z >= 0  =>  row2 alone, not row3 + row2)
    ///   Far    = row3 - row2   (z <= w  =>  row3 - row2)
    pub fn from_view_projection(vp: Mat4) -> Self {
        // glam is column-major. Row i = (col0[i], col1[i], col2[i], col3[i]).
        let row = |i: usize| -> Vec4 {
            Vec4::new(vp.col(0)[i], vp.col(1)[i], vp.col(2)[i], vp.col(3)[i])
        };
        
        let r0 = row(0);
        let r1 = row(1);
        let r2 = row(2);
        let r3 = row(3);
        
        let planes = [
            Plane::from_vec4(r3 + r0), // Left:   w + x >= 0
            Plane::from_vec4(r3 - r0), // Right:  w - x >= 0
            Plane::from_vec4(r3 + r1), // Bottom: w + y >= 0
            Plane::from_vec4(r3 - r1), // Top:    w - y >= 0
            Plane::from_vec4(r2),         // Near:   z >= 0
            Plane::from_vec4(r3 - r2), // Far:    w - z >= 0
        ];
        
        Self { planes }
    }
    
    /// Test whether an axis-aligned bounding box intersects with the frustum.
    /// Uses the p-vertex method: for each plane, select the AABB corner
    /// most in the direction of the plane normal. If that corner is outside
    /// the plane, the entire AABB is outside the frustum.
    pub fn intersects_aabb(&self, min: Vec3, max: Vec3) -> bool {
        for plane in &self.planes {
            let p = Vec3::new(
                if plane.normal.x >= 0.0 { max.x } else { min.x },
                if plane.normal.y >= 0.0 { max.y } else { min.y },
                if plane.normal.z >= 0.0 { max.z } else { min.z },
            );
            
            if plane.normal.dot(p) + plane.d < 0.0 {
                return false;
            }
        }
        true
    }
    
    /// Test whether a chunk at the given chunk-space position is visible.
    pub fn is_chunk_visible(&self, chunk_pos: IVec3) -> bool {
        let min = Vec3::new(
            chunk_pos.x as f32 * CHUNK_WORLD_SIZE,
            chunk_pos.y as f32 * CHUNK_WORLD_SIZE,
            chunk_pos.z as f32 * CHUNK_WORLD_SIZE,
        );
        let max = min + Vec3::splat(CHUNK_WORLD_SIZE);
        self.intersects_aabb(min, max)
    }
}