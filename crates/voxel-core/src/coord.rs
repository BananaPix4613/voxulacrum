//! Chunk-space and chunk-local coordinate primitives.
//!
//! These are pure geometric primitives - no material, lighting, or sidecar
//! state - and so live in `voxel-core` per its charter. [`ChunkCoord`] keys a
//! chunk within the world; [`LocalPos`] addresses a voxel cell within a chunk;
//! [`FaceAxis`] names one of a cell's six cardinal faces (distinct from
//! [`crate::BorderAxis`], which names an unsigned chunk-border axis).

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use glam::IVec3;

use crate::CHUNK_DIM;

/// Integer coordinate of a chunk in chunk space (one unit = one chunk).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ChunkCoord {
    /// Chunk X.
    pub x: i32,
    /// Chunk Y.
    pub y: i32,
    /// Chunk Z.
    pub z: i32,
}

impl ChunkCoord {
    /// Construct from components.
    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

impl From<IVec3> for ChunkCoord {
    #[inline]
    fn from(v: IVec3) -> Self {
        Self { x: v.x, y: v.y, z: v.z }
    }
}

impl From<ChunkCoord> for IVec3 {
    #[inline]
    fn from(c: ChunkCoord) -> Self {
        IVec3::new(c.x, c.y, c.z)
    }
}

/// Chunk-local voxel position. Each component is in `0..CHUNK_DIM`.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LocalPos {
    /// Local X, `0..CHUNK_DIM`.
    pub x: u8,
    /// Local Y, `0..CHUNK_DIM`.
    pub y: u8,
    /// Local Z, `0..CHUNK_DIM`.
    pub z: u8,
}

impl LocalPos {
    /// Construct, returning `None` if any component is `>= CHUNK_DIM`.
    #[inline]
    pub fn new(x: u8, y: u8, z: u8) -> Option<Self> {
        let dim = CHUNK_DIM as u8;
        if x < dim && y < dim && z < dim {
            Some(Self { x, y, z })
        } else {
            None
        }
    }

    /// Construct without bounds checking. Callers must ensure each component is
    /// `< CHUNK_DIM`; out-of-range values produce a nonsensical linear index.
    #[inline]
    pub const fn new_unchecked(x: u8, y: u8, z: u8) -> Self {
        Self { x, y, z }
    }

    /// Linear index in `x + y*CHUNK_DIM + z*CHUNK_DIM²` order - the same
    /// ordering the engine's dense voxel arrays use.
    #[inline]
    pub const fn to_index(self) -> usize {
        let dim = CHUNK_DIM;
        self.x as usize + self.y as usize * dim + self.z as usize * dim * dim
    }

    /// Inverse of [`LocalPos::to_index`].
    #[inline]
    pub const fn from_index(index: usize) -> Self {
        let dim = CHUNK_DIM;
        Self {
            x: (index % dim) as u8,
            y: ((index / dim) % dim) as u8,
            z: (index / (dim * dim)) as u8,
        }
    }
}

/// One of a voxel cell's six cardinal faces.
/// 
/// Distinct from [`crate::BorderAxis`] (an unsigned chunk-border axis); a
/// `FaceAxis` is *oriented*, naming a specific outward face used for decal
/// placement and face-keyed override diffs.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum FaceAxis {
    /// +X face.
    PosX,
    /// -X face.
    NegX,
    /// +Y face (top).
    PosY,
    /// -Y face (bottom).
    NegY,
    /// +Z face.
    PosZ,
    /// -Z face.
    NegZ,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn local_pos_index_roundtrips() {
        for index in 0..(CHUNK_DIM * CHUNK_DIM * CHUNK_DIM) {
            assert_eq!(LocalPos::from_index(index).to_index(), index);
        }
    }
    
    #[test]
    fn local_pos_new_rejects_out_of_range() {
        assert!(LocalPos::new(0, 0, 0).is_some());
        assert!(LocalPos::new((CHUNK_DIM - 1) as u8, 0, 0).is_some());
        assert!(LocalPos::new(CHUNK_DIM as u8, 0, 0).is_none());
    }
    
    #[test]
    fn chunk_coord_ivec3_roundtrips() {
        let c = ChunkCoord::new(-3, 7, 12);
        let v: IVec3 = c.into();
        assert_eq!(ChunkCoord::from(v), c);
    }
}
