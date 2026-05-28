//! The [`Voxel`] struct.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use static_assertions::const_assert;

use crate::error::{VoxelCoreError, VoxelCoreResult};
use crate::material::MaterialId;
use crate::shape::{Rotation, ShapeId};

/// A single voxel.
///
/// Logical bit budget (see [`Voxel::pack`]):
///
/// | field    | bits | offset |
/// |----------|------|--------|
/// | shape    | 5    | 0      |
/// | rotation | 2    | 5      |
/// | material | 16   | 7      |
/// | flags    | 8    | 23     |
/// | total    | 31   |        |
///
/// In-memory layout is `#[repr(C)]` with natural alignment - 6 bytes, not
/// bit-packed. Use [`Voxel::pack`] / [`Voxel::unpack`] for the 31-bit form
/// when serializing or uploading to a GPU buffer.
#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Voxel {
    /// Geometric shape.
    pub shape: ShapeId,
    /// Quarter-turn rotation about Y.
    pub rotation: Rotation,
    /// Material ID.
    pub material: MaterialId,
    /// Bitflags: bit 0 = light source, bit 1 = water-logged, others reserved.
    pub flags: u8,
}

// --- Bit-layout constants ------------------------------------------------

const SHAPE_OFFSET: u32 = 0;
const ROTATION_OFFSET: u32 = SHAPE_OFFSET + ShapeId::BIT_BUDGET;
const MATERIAL_OFFSET: u32 = ROTATION_OFFSET + Rotation::BIT_BUDGET;
const FLAGS_OFFSET: u32 = MATERIAL_OFFSET + 16;
const TOTAL_BITS: u32 = FLAGS_OFFSET + 8;

const SHAPE_MASK: u32 = (1 << ShapeId::BIT_BUDGET) - 1;
const ROTATION_MASK: u32 = (1 << Rotation::BIT_BUDGET) - 1;
const MATERIAL_MASK: u32 = (1 << 16) - 1;
const FLAGS_MASK: u32 = (1 << 8) - 1;

// Compile-time guarantees the spec is satisfied.
const_assert!(TOTAL_BITS == 31);
const_assert!(TOTAL_BITS <= 32);
// 6 bytes (`shape:u8` + `rotation:u8` + `material:u16` + `flags:u8` + pad to u16).
const_assert!(std::mem::size_of::<Voxel>() == 6);

// --- Flag bit definitions ------------------------------------------------

impl Voxel {
    /// Bit 0: this voxel is a light source.
    pub const FLAG_LIGHT_SOURCE: u8 = 1 << 0;
    /// Bit 1: this voxel is water-logged.
    pub const FLAG_WATER_LOGGED: u8 = 1 << 1;

    /// The empty / air voxel.
    pub const EMPTY: Self = Self {
        shape: ShapeId::Empty,
        rotation: Rotation::None,
        material: MaterialId::AIR,
        flags: 0,
    };

    /// Construct a solid cube of the given material.
    #[inline]
    pub const fn cube(material: MaterialId) -> Self {
        Self {
            shape: ShapeId::Cube,
            rotation: Rotation::None,
            material,
            flags: 0,
        }
    }

    /// True if shape is anything other than [`ShapeId::Empty`].
    #[inline]
    pub const fn is_solid(&self) -> bool {
        self.shape.is_solid()
    }

    /// Pack into 31-bit form occupying a `u32`. High bit is reserved (always 0).
    #[inline]
    pub const fn pack(&self) -> u32 {
        ((self.shape as u32) & SHAPE_MASK) << SHAPE_OFFSET
            | ((self.rotation as u32) & ROTATION_MASK) << ROTATION_OFFSET
            | ((self.material.raw() as u32) & MATERIAL_MASK) << MATERIAL_OFFSET
            | ((self.flags as u32) & FLAGS_MASK) << FLAGS_OFFSET
    }

    /// Unpack from 31-bit form. Errors if the shape discriminant is invalid.
    #[inline]
    pub fn unpack(bits: u32) -> VoxelCoreResult<Self> {
        let shape_raw = ((bits >> SHAPE_OFFSET) & SHAPE_MASK) as u8;
        let rotation_raw = ((bits >> ROTATION_OFFSET) & ROTATION_MASK) as u8;
        let material_raw = ((bits >> MATERIAL_OFFSET) & MATERIAL_MASK) as u16;
        let flags = ((bits >> FLAGS_OFFSET) & FLAGS_MASK) as u8;
        let shape = ShapeId::from_raw(shape_raw)
            .ok_or(VoxelCoreError::InvalidShape(shape_raw))?;
        Ok(Self {
            shape,
            rotation: Rotation::from_raw(rotation_raw),
            material: MaterialId(material_raw),
            flags,
        })
    }
}

// --- Tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voxel_struct_is_six_bytes() {
        assert_eq!(std::mem::size_of::<Voxel>(), 6);
    }

    #[test]
    fn shape_discriminant_fits_in_budget() {
        assert!((ShapeId::MAX_DISCRIMINANT as u32) < (1 << ShapeId::BIT_BUDGET));
    }

    #[test]
    fn pack_unpack_roundtrip_default() {
        let v = Voxel::default();
        let bits = v.pack();
        assert_eq!(bits, 0);
        assert_eq!(Voxel::unpack(bits).unwrap(), v);
    }

    #[test]
    fn pack_unpack_roundtrip_all_fields() {
        let v = Voxel {
            shape: ShapeId::InnerCornerSE,
            rotation: Rotation::Cw270,
            material: MaterialId(0xBEEF),
            flags: Voxel::FLAG_LIGHT_SOURCE | Voxel::FLAG_WATER_LOGGED,
        };
        let bits = v.pack();
        let back = Voxel::unpack(bits).unwrap();
        assert_eq!(back, v);
        // High bit must be unused.
        assert_eq!(bits & 0x8000_0000, 0);
    }

    #[test]
    fn unpack_invalid_shape_errors() {
        // discriminant 31 (max in 5 bits) is unused.
        let bits: u32 = 31;
        assert!(matches!(
            Voxel::unpack(bits),
            Err(VoxelCoreError::InvalidShape(31))
        ));
    }
}
