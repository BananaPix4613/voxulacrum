//! Nodes: stable IDs, polymorphic kinds + params, and static descriptors.

use glam::Vec2;
use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::pin::PinType;

new_key_type! {
    /// Stable identifier for a node. Survives edits and serialization;
    /// edges reference nodes by this key.
    pub struct NodeId;
}

/// Editor category - drives node color grouping in the visual editor.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum NodeCategory {
    /// Noise and coordinate sources.
    Source,
    /// Arithmetic and logic.
    Math,
    /// Curve mapping / thresholds.
    Curves,
    /// Domain transforms (warp, scale, offset, rotate).
    Domain,
    /// Density combinators.
    Density,
    /// Material providers.
    Material,
    /// Point-set generators.
    Positions,
    /// Surface scanners.
    Scanners,
    /// Prop placement.
    Props,
    /// Slope refinement.
    Slope,
    /// Biome routing.
    Biome,
    /// Terminal output.
    Output,
}

/// Specification of a single pin on a node. Static metadata, not per-instance.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct PinSpec {
    /// Display name of the pin.
    pub name: &'static str,
    /// Type carried by the pin.
    pub ty: PinType,
    /// For input pins: whether a connection is mandatory for validity.
    /// Ignored for output pins.
    pub required: bool,
}

/// Editor/runtime metadata for a node kind: display, color, and typed pins.
#[derive(Clone, Debug)]
pub struct NodeDescriptor {
    /// Human-readable name.
    pub display_name: &'static str,
    /// Category (color grouping).
    pub category: NodeCategory,
    /// RGB editor color.
    pub color: [u8; 3],
    /// Input pins, ordered. Pin index = position in this slice.
    pub inputs: &'static [PinSpec],
    /// Output pins, ordered. Pin index = position in this slice.
    pub outputs: &'static [PinSpec],
}

// --- Parameter structs ---------------------------------------------------

/// Fractal mode for noise sources. Mirrors `fastnoise_lite::FractalType`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum FractalType {
    /// Fractional Brownian motion (default).
    #[default]
    FBm,
    /// Ridged multifractal - sharp ridges.
    Ridged,
    /// Ping-pong - banded variant.
    PingPong,
}

/// Parameters shared by every fractal noise source (Perlin2D/3D, Simplex2D/3D).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct NoiseParams {
    /// Per-node seed; combined with `world_seed` via `EvalContext::noise_seed`.
    pub seed: u32,
    /// Base frequency (cycles per world unit).
    pub frequency: f32,
    /// Number of fractal octaves.
    pub octaves: u32,
    /// Frequency multiplier per octave.
    pub lacunarity: f32,
    /// Amplitude multiplier per octave.
    pub gain: f32,
    /// Fractal mode. Defaults to `FBm` for serde-compat with pre-Phase-6 JSON.
    #[serde(default)]
    pub fractal_type: FractalType,
}

impl Default for NoiseParams {
    fn default() -> Self {
        Self { seed: 0, frequency: 0.01, octaves: 4, lacunarity: 2.0, gain: 0.5, fractal_type: FractalType::FBm }
    }
}

/// Back-compat alias - `Perlin2DParams` is now just `NoiseParams`.
pub type Perlin2DParams = NoiseParams;

/// Parameters for [`NodeKind::Threshold`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ThresholdParams {
    /// Values `>= threshold` map to solid, below to empty.
    pub threshold: f32,
}

impl Default for ThresholdParams {
    fn default() -> Self {
        Self { threshold: 0.0 }
    }
}

/// Parameters for [`NodeKind::Output`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct OutputParams {
    /// Label for the output channel (e.g. for multi-output previews).
    pub label: String,
}

impl Default for OutputParams {
    fn default() -> Self {
        Self { label: "terrain".to_string() }
    }
}

/// Parameters for [`NodeKind::Constant`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct ConstantParams {
    /// The constant value emitted at every position.
    pub value: f32,
}

/// Parameters for [`NodeKind::Add`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct AddParams {}

/// Parameters for [`NodeKind::WorldPos`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct WorldPosParams {}

/// Parameters for [`NodeKind::Clamp`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ClampParams {
    /// Lower bound.
    pub min: f32,
    /// Upper bound.
    pub max: f32,
}
impl Default for ClampParams {
    fn default() -> Self { Self { min: 0.0, max: 1.0 } }
}

/// Parameters for [`NodeKind::Remap`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct RemapParams {
    /// Source range start.
    pub src_lo: f32,
    /// Source range end.
    pub src_hi: f32,
    /// Destination range start.
    pub dst_lo: f32,
    /// Destination range end.
    pub dst_hi: f32,
}
impl Default for RemapParams {
    fn default() -> Self {
        Self { src_lo: -1.0, src_hi: 1.0, dst_lo: 0.0, dst_hi: 1.0 }
    }
}

/// Parameters for [`NodeKind::CurveMapper`]. Stops must be sorted ascending.
/// by `x` for sensible behavior; lookup clamps at the endpoints.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CurveMapperParams {
    /// `(x, y)` knot stops; piecewise-linear between them.
    pub stops: Vec<(f32, f32)>,
}
impl Default for CurveMapperParams {
    fn default() -> Self { Self { stops: vec![(0.0, 0.0), (1.0, 1.0)] } }
}

/// Parameters for [`NodeKind::DomainWarp`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct DomainWarpParams {
    /// Per-node seed.
    pub seed: u32,
    /// Frequency of the warp noise.
    pub frequency: f32,
    /// Maximum displacement (world units) applied in XZ.
    pub amplitude: f32,
}
impl Default for DomainWarpParams {
    fn default() -> Self { Self { seed: 0, frequency: 0.02, amplitude: 8.0 } }
}

// Empty param structs for parameterless variants (kept for shape uniformity
// and future-proof param-adding). All derive `Default`.

/// Parameters for [`NodeKind::Multiply`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct MultiplyParams {}
/// Parameters for [`NodeKind::Subtract`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct SubtractParams {}
/// Parameters for [`NodeKind::Min`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct MinParams {}
/// Parameters for [`NodeKind::Max`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct MaxParams {}
/// Parameters for [`NodeKind::Lerp`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct LerpParams {}
/// Parameters for [`NodeKind::Union`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct UnionParams {}
/// Parameters for [`NodeKind::Intersect`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct IntersectParams {}
/// Parameters for [`NodeKind::DensitySubtract`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct DensitySubtractParams {}
/// Parameters for [`NodeKind::Mix`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct MixParams {}
/// Parameters for [`NodeKind::Mask`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct MaskParams {}

/// Polymorphic node kind. Each variant carries its parameter struct.
/// Serialized internally-tagged via the `"type"` field.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NodeKind {
    // --- Sources ---
    /// 2D Perlin source. Optional `position` Vec3 input overrides world position.
    Perlin2D(NoiseParams),
    /// 3D Perlin source, Optional `position` input.
    Perlin3D(NoiseParams),
    /// 2D OpenSimplex2 source. Optional `position` input.
    Simplex2D(NoiseParams),
    /// 3D OpenSimplex2 source. Optional `position` input.
    Simplex3D(NoiseParams),
    /// Uniform scalar source.
    Constant(ConstantParams),
    /// World-space position field.
    WorldPos(WorldPosParams),
    // --- Math ---
    /// Sum of two density fields.
    Add(AddParams),
    /// Elementwise product.
    Multiply(MultiplyParams),
    /// Elementwise difference (`a - b`).
    Subtract(SubtractParams),
    /// Elementwise minimum.
    Min(MinParams),
    /// Elementwise maximum.
    Max(MaxParams),
    /// Clamp into a configured range.
    Clamp(ClampParams),
    /// `a + (b - a) * t`, evaluated per-cell.
    Lerp(LerpParams),
    /// Linear remap from `[src_lo, src_hi]` to `[dst_lo, dst_hi]`.
    Remap(RemapParams),
    // --- Curves ---
    /// Binarize a density field about a threshold.
    Threshold(ThresholdParams),
    /// Piecewise-linear curve lookup.
    CurveMapper(CurveMapperParams),
    // --- Domain ---
    /// XZ domain-warp: displaces an input position field by a noise offset.
    DomainWarp(DomainWarpParams),
    // --- Density combinators ---
    /// CSG union (`max(a, b)`).
    Union(UnionParams),
    /// CSG intersection (`min(a, b)`).
    Intersect(IntersectParams),
    /// CSG difference (`max(a, -b)`).
    DensitySubtract(DensitySubtractParams),
    /// `lerp(a, b, clamp(factor, 0, 1))`.
    Mix(MixParams),
    /// `signal * clamp(mask, 0, 1)`.
    Mask(MaskParams),
    // --- Terminal ---
    /// Terminal node; consumes one density input.
    Output(OutputParams),
}

// Static pin layouts, shared by all instances of a kind.
const NO_PINS: &[PinSpec] = &[];
const NOISE_OUTPUTS: &[PinSpec] =
    &[PinSpec { name: "density", ty: PinType::Density, required: false }];
const DENSITY_IN: &[PinSpec] =
    &[PinSpec { name: "in", ty: PinType::Density, required: true }];
const DENSITY_OUT: &[PinSpec] =
    &[PinSpec { name: "out", ty: PinType::Density, required: false }];
const SCALAR_OUT: &[PinSpec] =
    &[PinSpec { name: "value", ty: PinType::Scalar, required: false }];
const VEC3_OUT: &[PinSpec] =
    &[PinSpec { name: "pos", ty: PinType::Vec3, required: false }];
const VEC3_IN_OPTIONAL: &[PinSpec] =
    &[PinSpec { name: "position", ty: PinType::Vec3, required: false }];
const VEC3_IN_REQUIRED: &[PinSpec] =
    &[PinSpec { name: "position", ty: PinType::Vec3, required: true }];

const DENSITY_2IN: &[PinSpec] = &[
    PinSpec { name: "a", ty: PinType::Density, required: true },
    PinSpec { name: "b", ty: PinType::Density, required: true },
];
const DENSITY_3IN_LERP: &[PinSpec] = &[
    PinSpec { name: "a", ty: PinType::Density, required: true },
    PinSpec { name: "b", ty: PinType::Density, required: true },
    PinSpec { name: "t", ty: PinType::Density, required: true },
];
const DENSITY_3IN_MIX: &[PinSpec] = &[
    PinSpec { name: "a", ty: PinType::Density, required: true },
    PinSpec { name: "b", ty: PinType::Density, required: true },
    PinSpec { name: "factor", ty: PinType::Density, required: true },
];
const DENSITY_MASK_IN: &[PinSpec] = &[
    PinSpec { name: "signal", ty: PinType::Density, required: true },
    PinSpec { name: "mask",   ty: PinType::Density, required: true },
];

impl NodeKind {
    /// Static descriptor (display, color, typed pins) for this kind.
    pub fn descriptor(&self) -> NodeDescriptor {
        const SRC: [u8;3]   = [0x4c, 0x9a, 0xff];
        const MATH: [u8;3]  = [0x9c, 0x7c, 0xff];
        const CURVE: [u8;3] = [0xff, 0xb3, 0x4c];
        const DOM: [u8;3]   = [0xff, 0x7c, 0xc4];
        const DENS: [u8;3]  = [0x6c, 0xc0, 0x6c];
        const OUT: [u8;3]   = [0xe0, 0x4c, 0x4c];
        match self {
            // Sources - all four take an optional Vec3 position override.
            NodeKind::Perlin2D(_) => NodeDescriptor {
                display_name: "Perlin 2D",
                category: NodeCategory::Source,
                color: SRC,
                inputs: VEC3_IN_OPTIONAL,
                outputs: NOISE_OUTPUTS
            },
            NodeKind::Perlin3D(_) => NodeDescriptor {
                display_name: "Perlin 3D",
                category: NodeCategory::Source,
                color: SRC,
                inputs: VEC3_IN_OPTIONAL,
                outputs: NOISE_OUTPUTS
            },
            NodeKind::Simplex2D(_) => NodeDescriptor {
                display_name: "Simplex 2D",
                category: NodeCategory::Source,
                color: SRC,
                inputs: VEC3_IN_OPTIONAL,
                outputs: NOISE_OUTPUTS
            },
            NodeKind::Simplex3D(_) => NodeDescriptor {
                display_name: "Simplex 3D",
                category: NodeCategory::Source,
                color: SRC,
                inputs: VEC3_IN_OPTIONAL,
                outputs: NOISE_OUTPUTS
            },
            NodeKind::Constant(_) => NodeDescriptor {
                display_name: "Constant",
                category: NodeCategory::Source,
                color: SRC,
                inputs: NO_PINS,
                outputs: SCALAR_OUT
            },
            NodeKind::WorldPos(_) => NodeDescriptor {
                display_name: "World Position",
                category: NodeCategory::Source,
                color: SRC,
                inputs: NO_PINS,
                outputs: VEC3_OUT
            },
            // Math
            NodeKind::Add(_) => NodeDescriptor {
                display_name: "Add",
                category: NodeCategory::Math,
                color: MATH,
                inputs: DENSITY_2IN,
                outputs: DENSITY_OUT
            },
            NodeKind::Multiply(_) => NodeDescriptor {
                display_name: "Multiply",
                category: NodeCategory::Math,
                color: MATH,
                inputs: DENSITY_2IN,
                outputs: DENSITY_OUT
            },
            NodeKind::Subtract(_) => NodeDescriptor {
                display_name: "Subtract",
                category: NodeCategory::Math,
                color: MATH,
                inputs: DENSITY_2IN,
                outputs: DENSITY_OUT
            },
            NodeKind::Min(_) => NodeDescriptor {
                display_name: "Min",
                category: NodeCategory::Math,
                color: MATH,
                inputs: DENSITY_2IN,
                outputs: DENSITY_OUT
            },
            NodeKind::Max(_) => NodeDescriptor {
                display_name: "Max",
                category: NodeCategory::Math,
                color: MATH,
                inputs: DENSITY_2IN,
                outputs: DENSITY_OUT
            },
            NodeKind::Clamp(_) => NodeDescriptor {
                display_name: "Clamp",
                category: NodeCategory::Math,
                color: MATH,
                inputs: DENSITY_IN,
                outputs: DENSITY_OUT
            },
            NodeKind::Lerp(_) => NodeDescriptor {
                display_name: "Lerp",
                category: NodeCategory::Math,
                color: MATH,
                inputs: DENSITY_3IN_LERP,
                outputs: DENSITY_OUT
            },
            NodeKind::Remap(_) => NodeDescriptor {
                display_name: "Remap",
                category: NodeCategory::Math,
                color: MATH,
                inputs: DENSITY_IN,
                outputs: DENSITY_OUT
            },
            // Curves
            NodeKind::Threshold(_) => NodeDescriptor {
                display_name: "Threshold",
                category: NodeCategory::Curves,
                color: CURVE,
                inputs: DENSITY_IN,
                outputs: DENSITY_OUT
            },
            NodeKind::CurveMapper(_) => NodeDescriptor {
                display_name: "Curve Mapper",
                category: NodeCategory::Curves,
                color: CURVE,
                inputs: DENSITY_IN,
                outputs: DENSITY_OUT
            },
            // Domain
            NodeKind::DomainWarp(_) => NodeDescriptor {
                display_name: "Domain Warp",
                category: NodeCategory::Domain,
                color: DOM,
                inputs: VEC3_IN_REQUIRED,
                outputs: VEC3_OUT
            },
            // Density combinators
            NodeKind::Union(_) => NodeDescriptor {
                display_name: "Union",
                category: NodeCategory::Density,
                color: DENS,
                inputs: DENSITY_2IN,
                outputs: DENSITY_OUT
            },
            NodeKind::Intersect(_) => NodeDescriptor {
                display_name: "Intersect",
                category: NodeCategory::Density,
                color: DENS,
                inputs: DENSITY_2IN,
                outputs: DENSITY_OUT
            },
            NodeKind::DensitySubtract(_) => NodeDescriptor {
                display_name: "Density Subtract",
                category: NodeCategory::Density,
                color: DENS,
                inputs: DENSITY_2IN,
                outputs: DENSITY_OUT
            },
            NodeKind::Mix(_) => NodeDescriptor {
                display_name: "Mix",
                category: NodeCategory::Density,
                color: DENS,
                inputs: DENSITY_3IN_MIX,
                outputs: DENSITY_OUT
            },
            NodeKind::Mask(_) => NodeDescriptor {
                display_name: "Mask",
                category: NodeCategory::Density,
                color: DENS,
                inputs: DENSITY_MASK_IN,
                outputs: DENSITY_OUT
            },
            // Terminal
            NodeKind::Output(_) => NodeDescriptor {
                display_name: "Output",
                category: NodeCategory::Output,
                color: OUT,
                inputs: DENSITY_IN,
                outputs: NO_PINS
            },
        }
    }

    /// Stable string identifier matching the serde `"type"` tag.
    pub fn type_name(&self) -> &'static str {
        match self {
            NodeKind::Perlin2D(_)         => "Perlin2D",
            NodeKind::Perlin3D(_)         => "Perlin3D",
            NodeKind::Simplex2D(_)        => "Simplex2D",
            NodeKind::Simplex3D(_)        => "Simplex3D",
            NodeKind::Constant(_)         => "Constant",
            NodeKind::WorldPos(_)         => "WorldPos",
            NodeKind::Add(_)              => "Add",
            NodeKind::Multiply(_)         => "Multiply",
            NodeKind::Subtract(_)         => "Subtract",
            NodeKind::Min(_)              => "Min",
            NodeKind::Max(_)              => "Max",
            NodeKind::Clamp(_)            => "Clamp",
            NodeKind::Lerp(_)             => "Lerp",
            NodeKind::Remap(_)            => "Remap",
            NodeKind::Threshold(_)        => "Threshold",
            NodeKind::CurveMapper(_)      => "CurveMapper",
            NodeKind::DomainWarp(_)       => "DomainWarp",
            NodeKind::Union(_)            => "Union",
            NodeKind::Intersect(_)        => "Intersect",
            NodeKind::DensitySubtract(_)  => "DensitySubtract",
            NodeKind::Mix(_)              => "Mix",
            NodeKind::Mask(_)             => "Mask",
            NodeKind::Output(_)           => "Output",
        }
    }
}

/// A node instance in the graph: its kind/params plus editor position.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Node {
    /// Kind and parameters.
    pub kind: NodeKind,
    /// Editor canvas position. Defaults to origin when absent in JSON.
    #[serde(default)]
    pub position: Vec2,
}

impl Node {
    /// Create a node at the origin.
    pub fn new(kind: NodeKind) -> Self {
        Self { kind, position: Vec2::ZERO }
    }

    /// Shorthand for `self.kind.descriptor()`.
    pub fn descriptor(&self) -> NodeDescriptor {
        self.kind.descriptor()
    }
}
