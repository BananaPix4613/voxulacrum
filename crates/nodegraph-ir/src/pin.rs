//! Pin types and compatibility rules.

use serde::{Deserialize, Serialize};

/// The type carried by a pin / edge. A closed enum (not trait objects) so
/// matching is exhaustive and validation is cheap.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum PinType {
    /// `f32`, a single value or per-position scalar.
    Scalar,
    /// 3D scalar field; solid where `> 0`.
    Density,
    /// 2D field over the world `(x, z)` plane - e.g. a surface height or a
    /// climate channel. Distinct from [`Density`](PinType::Density), which is
    /// fully 3D.
    SurfaceField,
    /// Material provider - answers "what block goes here?".
    Material,
    /// Fluid provider - answers "what fluid is here, and to what level?".
    FluidProvider,
    /// A set of `(x, y, z)` points for prop placement. (The design doc names
    /// this type `ScatterPoints`; the code keeps the more general name
    /// `Positions`.)
    Positions,
    /// Position -> prop bindings.
    Assignments,
    /// Editable spline, `f32 -> f32`.
    Curve,
    /// 3D vector (domain warps, offsets).
    Vec3,
    /// Discrete biome identifier.
    BiomeId,
    /// Discrete zone identifier - the top-level region above biomes.
    ZoneId,
    /// `ChunkBuffer<Voxel>` - the final output type. Makes terrain a real flowing pin type.
    Terrain,
}

impl PinType {
    /// True if an output of type `output` may feed an input of type `input`,
    /// either by exact match or an automatic coercion.
    pub fn is_compatible(output: PinType, input: PinType) -> bool
    {
        output == input || Self::can_coerce(output, input)
    }

    /// Automatic coercions, applied implicitly:
    /// - `Scalar -> Density` (constant field)
    /// - `Curve -> Scalar` (the evaluator samples the curve at a scalar input)
    ///
    /// Nothing else coerces - be strict.
    pub fn can_coerce(from: PinType, to: PinType) -> bool {
        use PinType::*;
        matches!((from, to), (Scalar, Density) | (Curve, Scalar))
    }
}

#[cfg(test)]
mod tests {
    use super::PinType::*;
    use super::*;

    #[test]
    fn exact_match_is_compatible() {
        assert!(PinType::is_compatible(Density, Density));
        assert!(PinType::is_compatible(Material, Material));
    }

    #[test]
    fn scalar_coerces_to_density_one_way() {
        assert!(PinType::is_compatible(Scalar, Density));
        assert!(!PinType::is_compatible(Density, Scalar));
    }

    #[test]
    fn curve_coerces_to_scalar() {
        assert!(PinType::is_compatible(Curve, Scalar));
        assert!(!PinType::is_compatible(Scalar, Curve));
    }

    #[test]
    fn unrelated_types_incompatible() {
        assert!(!PinType::is_compatible(Material, Density));
        assert!(!PinType::is_compatible(Positions, Terrain));
    }

    #[test]
    fn new_types_are_strict() {
        // Each new type matches only itself; nothing coerces into or out of it.
        for t in [SurfaceField, FluidProvider, ZoneId] {
            assert!(PinType::is_compatible(t, t));
        }
        assert!(!PinType::is_compatible(Scalar, SurfaceField));
        assert!(!PinType::is_compatible(SurfaceField, Density));
        assert!(!PinType::is_compatible(ZoneId, BiomeId));
        assert!(!PinType::is_compatible(FluidProvider, Material));
    }
}
