//! Voxel shape.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Geometric shape of a voxel cell.
///
/// Four variants — fits in 2 bits (discriminants 0..=3); the packed
/// [`crate::Voxel`] format reserves 3 bits for future shape additions
/// (stairs, chamfers, etc.). Half-height slabs replace the old slope
/// vocabulary (design doc v1.2 §2).
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(u8)]
pub enum ShapeId {
    /// Empty cell.
    #[default]
    Empty = 0,
    /// Full cube.
    Cube = 1,
    /// Half-height slab in the bottom of the cell (Y range [0.0, 0.5]).
    SlabBottom = 2,
    /// Half-height slab in the top of the cell (Y range [0.5, 1.0]).
    SlabTop = 3,
}

impl ShapeId {
    /// Maximum discriminant value currently in use.
    pub const MAX_DISCRIMINANT: u8 = 3;

    /// Bit budget reserved by the [`crate::Voxel`] packed format.
    pub const BIT_BUDGET: u32 = 3;

    /// Round-trip the discriminant from raw bits.
    #[inline]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Empty),
            1 => Some(Self::Cube),
            2 => Some(Self::SlabBottom),
            3 => Some(Self::SlabTop),
            _ => None,
        }
    }

    /// True if this shape is anything other than [`ShapeId::Empty`].
    #[inline]
    pub const fn is_solid(self) -> bool {
        !matches!(self, Self::Empty)
    }

    /// True for the half-height slab shapes (used by the preview mesher).
    #[inline]
    pub const fn is_slab(self) -> bool {
        matches!(self, Self::SlabBottom | Self::SlabTop)
    }
}
