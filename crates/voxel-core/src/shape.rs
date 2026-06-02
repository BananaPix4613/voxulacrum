//! Voxel shape and rotation.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Geometric shape of a voxel cell.
///
/// 14 named variants total - fits in 4 bits (discriminants 0..=13) but the
/// system-prompt budget reserves 5 bits for future shape additions
/// (chamfers, stairs, half-blocks, etc.).
///
/// Direction-bearing variants (`SlopeN`/`E`/`S`/`W`, the four outer corners,
/// the four inner corners) name a *canonical* facing. The separate
/// [`Rotation`] field on [`crate::Voxel`] composes additional 90 degree turns.
/// A `SlopeN` with `Rotation::Cw90` is equivalent to a `SlopeE` with
/// `Rotation::None`; the renderer/mesher chooses one canonical form when
/// emitting geometry.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(u8)]
pub enum ShapeId {
    /// Empty cell.
    #[default]
    Empty = 0,
    /// Full cube.
    Cube = 1,
    /// 45 degree slope, high edge to the north.
    SlopeN = 2,
    /// 45 degree slope, high edge to the east.
    SlopeE = 3,
    /// 45 degree slope, high edge to the south.
    SlopeS = 4,
    /// 45 degree slope, high edge to the west.
    SlopeW = 5,
    /// Outer-corner slope, high corner at NE.
    OuterCornerNE = 6,
    /// Outer-corner slope, high corner at NW.
    OuterCornerNW = 7,
    /// Outer-corner slope, high corner at SE.
    OuterCornerSE = 8,
    /// Outer-corner slope, high corner at SW.
    OuterCornerSW = 9,
    /// Inner-corner slope, low corner at NE.
    InnerCornerNE = 10,
    /// Inner-corner slope, low corner at NW.
    InnerCornerNW = 11,
    /// Inner-corner slope, low corner at SE.
    InnerCornerSE = 12,
    /// Inner-corner slope, low corner at SW.
    InnerCornerSW = 13,
    /// Ceiling slope, high edge to the north — vertical mirror of [`Self::SlopeN`].
    /// Solid material is at the top of the cell; the slope hangs down and
    /// opens toward `+Z` (south).
    CeilingSlopeN = 14,
    /// Ceiling slope, high edge to the east — vertical mirror of [`Self::SlopeE`].
    CeilingSlopeE = 15,
    /// Ceiling slope, high edge to the south — vertical mirror of [`Self::SlopeS`].
    CeilingSlopeS = 16,
    /// Ceiling slope, high edge to the west — vertical mirror of [`Self::SlopeW`].
    CeilingSlopeW = 17,
    /// Ceiling outer-corner, low corner at NE — vertical mirror of [`Self::OuterCornerNE`].
    CeilingOuterCornerNE = 18,
    /// Ceiling outer-corner, low corner at NW — vertical mirror of [`Self::OuterCornerNW`].
    CeilingOuterCornerNW = 19,
    /// Ceiling outer-corner, low corner at SE — vertical mirror of [`Self::OuterCornerSE`].
    CeilingOuterCornerSE = 20,
    /// Ceiling outer-corner, low corner at SW — vertical mirror of [`Self::OuterCornerSW`].
    CeilingOuterCornerSW = 21,
}

impl ShapeId {
    /// Maximum discriminant value currently in use. Tests assert this fits
    /// inside the 5-bit budget declared by the voxel format.
    pub const MAX_DISCRIMINANT: u8 = 21;

    /// Bit budget reserved by the [`crate::Voxel`] packed format.
    pub const BIT_BUDGET: u32 = 5;

    /// Round-trip the discriminant from raw bits.
    #[inline]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Empty),
            1 => Some(Self::Cube),
            2 => Some(Self::SlopeN),
            3 => Some(Self::SlopeE),
            4 => Some(Self::SlopeS),
            5 => Some(Self::SlopeW),
            6 => Some(Self::OuterCornerNE),
            7 => Some(Self::OuterCornerNW),
            8 => Some(Self::OuterCornerSE),
            9 => Some(Self::OuterCornerSW),
            10 => Some(Self::InnerCornerNE),
            11 => Some(Self::InnerCornerNW),
            12 => Some(Self::InnerCornerSE),
            13 => Some(Self::InnerCornerSW),
            14 => Some(Self::CeilingSlopeN),
            15 => Some(Self::CeilingSlopeE),
            16 => Some(Self::CeilingSlopeS),
            17 => Some(Self::CeilingSlopeW),
            18 => Some(Self::CeilingOuterCornerNE),
            19 => Some(Self::CeilingOuterCornerNW),
            20 => Some(Self::CeilingOuterCornerSE),
            21 => Some(Self::CeilingOuterCornerSW),
            _ => None,
        }
    }

    /// True if this shape is anything other than [`ShapeId::Empty`].
    #[inline]
    pub const fn is_solid(self) -> bool {
        !matches!(self, Self::Empty)
    }
}

/// Quarter-turn rotation bout the world Y axis. Two bits; four cases.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(u8)]
pub enum Rotation {
    /// No rotation.
    #[default]
    None = 0,
    /// 90 degrees clockwise (looking down +Y).
    Cw90 = 1,
    /// 180 degrees.
    Cw180 = 2,
    /// 270 degrees clockwise (= 90 degrees counter-clockwise).
    Cw270 = 3,
}

impl Rotation {
    /// Bit budget reserved by the [`crate::Voxel`] packed format.
    pub const BIT_BUDGET: u32 = 2;

    /// Round-trip from raw bits.
    #[inline]
    pub const fn from_raw(raw: u8) -> Self {
        match raw & 0b11 {
            0 => Self::None,
            1 => Self::Cw90,
            2 => Self::Cw180,
            _ => Self::Cw270,
        }
    }
}
