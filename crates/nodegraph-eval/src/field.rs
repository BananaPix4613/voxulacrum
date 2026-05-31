//! Dense scalar / vector fields produced during evaluation.

use glam::Vec3;

/// Edge length of a chunk field. Matches `voxel-core`'s default `ChunkBuffer`
/// edge length (`N = 32`); kept as a local const to avoid a dependency on
/// `voxel-core` (Phase 3 produces no voxels).
pub const CHUNK_DIM: usize = 32;

/// Dense `CHUNK_DIM³` field of `f32`. Index order `x + y*N + z*N²`, matching
/// `voxel-core`'s `ChunkBuffer` convention.
#[derive(Clone, Debug)]
pub struct ScalarField {
    data: Box<[f32]>,
}

impl ScalarField {
    /// Edge length.
    pub const DIM: usize = CHUNK_DIM;
    /// Total cells (`N³`).
    pub const VOLUME: usize = CHUNK_DIM * CHUNK_DIM * CHUNK_DIM;

    /// All-zero field
    pub fn zeroed() -> Self {
        Self { data: vec![0.0; Self::VOLUME].into_boxed_slice() }
    }

    /// Field filled with a single value.
    pub fn filled(value: f32) -> Self {
        Self { data: vec![value; Self::VOLUME].into_boxed_slice() }
    }

    /// Flat index for local coords.
    #[inline]
    pub fn index(x: usize, y: usize, z: usize) -> usize {
        x + y * CHUNK_DIM + z * CHUNK_DIM * CHUNK_DIM
    }

    /// Read at local coords.
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> f32 {
        self.data[Self::index(x, y, z)]
    }

    /// Write at local coords.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize, v: f32) {
        self.data[Self::index(x, y, z)] = v;
    }

    /// Raw slice (length `N³`, index order `x + y*N + z*N²`).
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// New field from an elementwise unary op.
    pub fn map(&self, f: impl Fn(f32) -> f32) -> Self {
        Self { data: self.data.iter().map(|&v| f(v)).collect() }
    }

    /// New field from an elementwise binary op over two fields.
    pub fn zip_with(&self, other: &Self, f: impl Fn(f32, f32) -> f32) -> Self {
        Self {
            data: self.data.iter().zip(other.data.iter()).map(|(&a, &b)| f(a, b)).collect(),
        }
    }
}

/// Dense `CHUNK_DIM³` field of [`glam::Vec3`]. Output of `WorldPos`.
#[derive(Clone, Debug)]
pub struct Vec3Field {
    data: Box<[Vec3]>,
}

impl Vec3Field {
    /// Total cells (`N³`).
    pub const VOLUME: usize = CHUNK_DIM * CHUNK_DIM * CHUNK_DIM;

    /// Build from a per-coordinate function.
    pub fn from_fn(f: impl Fn(usize, usize, usize) -> Vec3) -> Self {
        let mut data = Vec::with_capacity(Self::VOLUME);
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    data.push(f(x, y, z));
                }
            }
        }
        Self { data: data.into_boxed_slice() }
    }

    /// Read at local coords.
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> Vec3 {
        self.data[ScalarField::index(x, y, z)]
    }

    /// Raw slice.
    pub fn data(&self) -> &[Vec3] {
        &self.data
    }
}
