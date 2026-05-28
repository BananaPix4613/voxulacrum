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
    /// Material provider - answers "what block goes here?".
    Material,
    /// A set of `(x, y, z)` points for prop placement.
    Positions,
    /// Position -> prop bindings.
    Assignments,
    /// Editable spline, `f32 -> f32`.
    Curve,
    /// 3D vector (domain warps, offsets).
    Vec3,
    /// Discrete biome identifier.
    BiomeId,
    /// `ChunkBuffer<Voxel>` - the final output type. A bare tag in the IR.
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
}
