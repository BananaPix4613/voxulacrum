//! Per-chunk evaluation context. Explicit; no global state.

use glam::{IVec3, Vec3};

use crate::field::CHUNK_DIM;

/// Inputs that situate a chunk evaluation: the world seed and chunk coords.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvalContext {
    /// World-wide seed.
    pub world_seed: u64,
    /// Chunk coordinate (in chunks, not voxels).
    pub chunk: IVec3,
}

impl EvalContext {
    /// New context.
    pub fn new(world_seed: u64, chunk: IVec3) -> Self {
        Self { world_seed, chunk }
    }

    /// World-space position of a chunk-local voxel coordinate.
    #[inline]
    pub fn world_pos(&self, x: usize, y: usize, z: usize) -> Vec3 {
        Vec3::new(
            (self.chunk.x * CHUNK_DIM as i32 + x as i32) as f32,
            (self.chunk.y * CHUNK_DIM as i32 + y as i32) as f32,
            (self.chunk.z * CHUNK_DIM as i32 + z as i32) as f32,
        )
    }

    /// Seed for a coherent noise field, derived from `(world_seed, node-local
    /// seed)`. **Chunk coordinates are deliberately excluded** so the field is
    /// globally continuous (no seams at chunk borders); continuity comes from
    /// feeding world coordinates into the noise function. Per-chunk stochastic
    /// nodes (scatter) get a different, chunk-aware derivation in Phase 10.
    pub fn noise_seed(&self, node_local_seed: u32) -> i32 {
        let mut h = self.world_seed ^ (node_local_seed as u64).wrapping_mul(0x9E3779B97F4A7C15);
        h ^= h >> 30;
        h = h.wrapping_mul(0xBF58476D1CE4E5B9);
        h ^= h >> 27;
        h = h.wrapping_mul(0x94D049BB133111EB);
        h ^= h >> 31;
        h as i32
    }
}
