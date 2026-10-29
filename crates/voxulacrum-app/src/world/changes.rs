//! The changed-chunk feed (design §12, "The client/server boundary"): one entry
//! for each chunk whose data the mutation door changed, naming the layers that
//! changed and the version the chunk now carries, and one for each chunk that
//! left the resident set. The client rebuilds what it derives from a chunk
//! (meshes, water, foliage buffers) from this feed rather than from
//! render-shaped rebuild lists, and never scans the resident set to find what
//! changed.
//!
//! The feed describes data, not render work. Which neighbors to re-mesh, and
//! whether to debounce an edit, are the client's decisions, made from `faces`
//! and `origin`.

use std::ops::{BitOr, BitOrAssign};
use glam::IVec3;
use voxel_core::ChunkCoord;
/// Defined in `voxel-core` so the renderer can tag its entries without reading
/// the world code. Every change the door applies stamps a chunk with one.
pub use voxel_core::ChunkVersion;

use super::mutation::MutationOrigin;

/// The chunk layers a change touched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChunkLayers(u8);

impl ChunkLayers {
    /// No layer: a chunk that left the resident set.
    pub const NONE: Self = Self(0);
    pub const VOXELS: Self = Self(1 << 0);
    pub const FLUID: Self = Self(1 << 1);
    pub const SCATTER: Self = Self(1 << 2);
    pub const DETAIL: Self = Self(1 << 3);
    /// Every layer: a chunk that was loaded or replaced.
    pub const ALL: Self = Self(0b1111);
    
    /// Whether every layer in `other` is in `self`.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for ChunkLayers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

impl BitOrAssign for ChunkLayers {
    fn bitor_assign(&mut self, rhs: Self) { self.0 |= rhs.0; }
}

/// The faces of a chunk whose boundary cells changed, so the neighbor across
/// each one sees different cells along their shared face. A chunk that was
/// loaded or replaced changes all six.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChunkFaces(u8);

impl ChunkFaces {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(0b11_1111);
    
    /// Offsets to the six face neighbors, in bit order.
    const OFFSETS: [IVec3; 6] = [
        IVec3::new(-1, 0, 0),
        IVec3::new(1, 0, 0),
        IVec3::new(0, -1, 0),
        IVec3::new(0, 1, 0),
        IVec3::new(0, 0, -1),
        IVec3::new(0, 0, 1),
    ];
    
    /// The face toward the neighbor at `offset`, or `NONE` when `offset` is not
    /// a face neighbor's.
    pub fn toward(offset: IVec3) -> Self {
        Self::OFFSETS
            .iter()
            .position(|&o| o == offset)
            .map_or(Self::NONE, |i| Self(1 << i))
    }

    /// Offsets to the neighbors across the faces in this set.
    pub fn neighbor_offsets(self) -> impl Iterator<Item = IVec3> {
        Self::OFFSETS
            .into_iter()
            .enumerate()
            .filter(move |&(i, _)| self.0 & (1 << i) != 0)
            .map(|(_, offset)| offset)
    }
}

impl BitOr for ChunkFaces {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

impl BitOrAssign for ChunkFaces {
    fn bitor_assign(&mut self, rhs: Self) { self.0 |= rhs.0; }
}

/// One entry in the feed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkChange {
    pub chunk: ChunkCoord,
    /// The version the change left the chunk at.
    #[allow(dead_code)] // read once render entries are tagged with it (R4)
    pub version: ChunkVersion,
    pub layers: ChunkLayers,
    /// Faces whose boundary cells changed; `NONE` unless `layers` has `VOXELS`.
    pub faces: ChunkFaces,
    /// The origin of the command that made the change: an actor's edit
    /// (`Authoring` or `PlayTime`) or engine infrastructure (`System`).
    pub origin: MutationOrigin,
    /// Whether the chunk is resident after the change. False when it left the
    /// resident set: the client drops what it built from it, `version` is the
    /// last one the chunk carried, and `layers` and `faces` are `NONE`.
    pub resident: bool,
}
