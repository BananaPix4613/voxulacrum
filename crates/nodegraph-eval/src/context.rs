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

    /// World-absolute seed for a per-chunk stochastic process (e.g. `PoissonDisk`'s
    /// Bridson walk over this chunk's margin band). Derived from the chunk's
    /// world-space base voxel (`chunk * CHUNK_DIM`) rather than chunk coordinates.
    /// so it matches the world-position derivation the rest of the stack uses (§12:
    /// no RNG from chunk coordinates alone) and folds the Y base - vertically
    /// stacked chunks no longer share an XZ layout (observation 5.3).
    ///
    /// This makes the *seed* world-absolute; it does not make the Poisson point set
    /// seam-continuous. Bridson is a single global sequential walk seeded once per
    /// chunk, so points in the margin overlap still differ across a chunk border.
    /// True seam continuity would need a world-tiled Bridson (out of scope); the
    /// margin-band ownership model (`scatter.rs`) already handles props whose
    /// footprint straddles a border.
    pub fn chunk_world_seed(&self, node_local_seed: u32) -> u64 {
        let base = self.chunk * CHUNK_DIM as i32; // world-space base voxel of the chunk
        let mut h = self.world_seed;
        h = mix64(h ^ (node_local_seed as u64).wrapping_mul(0x9E3779B97F4A7C15));
        h = mix64(h ^ (base.x as i64 as u64).wrapping_mul(0xD1B54A32D192ED03));
        h = mix64(h ^ (base.y as i64 as u64).wrapping_mul(0x9E3779B97F4A7C15));
        h = mix64(h ^ (base.z as i64 as u64).wrapping_mul(0xABC98388FB8FAC03));
        h
    }

    /// World-absolute grid-cell seed, independent of which chunk evaluates it.
    /// Two chunks sharing a border cell derive the identical seed, so
    /// `JitteredGrid` scatter is seamless across chunk boundaries.
    pub fn world_cell_seed(&self, node_local_seed: u32, cell_x: i64, cell_z: i64) -> u64 {
        let mut h = self.world_seed;
        h = mix64(h ^ (node_local_seed as u64).wrapping_mul(0x9E3779B97F4A7C15));
        h = mix64(h ^ (cell_x as u64).wrapping_mul(0xD1B54A32D192ED03));
        h = mix64(h ^ (cell_z as u64).wrapping_mul(0xABC98388FB8FAC03));
        h
    }
}

/// SplitMix64 finalizer used to derive scatter seeds + stable foliage ids.
#[inline]
pub(crate) fn mix64(z: u64) -> u64 {
    let mut z = z.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}
