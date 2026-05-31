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

/// Parameters for [`NodeKind::Perlin2D`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Perlin2DParams {
    /// Seed for the noise field.
    pub seed: u32,
    /// Base frequency.
    pub frequency: f32,
    /// Number of fractal octaves.
    pub octaves: u32,
    /// Frequency multiplier per octave.
    pub lacunarity: f32,
    /// Amplitude multiplier per octave.
    pub gain: f32,
}

impl Default for Perlin2DParams {
    fn default() -> Self {
        Self { seed: 0, frequency: 0.01, octaves: 4, lacunarity: 2.0, gain: 0.5 }
    }
}

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

/// Polymorphic node kind. Each variant carries its parameter struct.
/// Serialized internally-tagged via the `"type"` field.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NodeKind {
    /// 2D Perlin noise source. Outputs a [`PinType::Density`] field.
    Perlin2D(Perlin2DParams),
    /// Binarizes a density field about a threshold.
    Threshold(ThresholdParams),
    /// Terminal node; consumes one density input.
    Output(OutputParams),
    /// Uniform scalar source. Outputs a [`PinType::Scalar`] field.
    Constant(ConstantParams),
    /// Adds two density/scalar fields elementwise.
    Add(AddParams),
    /// Source of world-space position. Outputs a [`PinType::Vec3`] field.
    WorldPos(WorldPosParams),
}

// Static pin layouts, shared by all instances of a kind.
const NO_PINS: &[PinSpec] = &[];
const PERLIN2D_OUTPUTS: &[PinSpec] =
    &[PinSpec { name: "density", ty: PinType::Density, required: false }];
const DENSITY_IN: &[PinSpec] =
    &[PinSpec { name: "in", ty: PinType::Density, required: true }];
const DENSITY_OUT: &[PinSpec] =
    &[PinSpec { name: "out", ty: PinType::Density, required: false }];
const SCALAR_OUT: &[PinSpec] =
    &[PinSpec { name: "value", ty: PinType::Scalar, required: false }];
const ADD_INPUTS: &[PinSpec] = &[
    PinSpec { name: "a", ty: PinType::Density, required: true },
    PinSpec { name: "b", ty: PinType::Density, required: true },
];
const VEC3_OUT: &[PinSpec] =
    &[PinSpec { name: "pos", ty: PinType::Vec3, required: false }];

impl NodeKind {
    /// Static descriptor (display, color, typed pins) for this kind.
    pub fn descriptor(&self) -> NodeDescriptor {
        match self {
            NodeKind::Perlin2D(_) => NodeDescriptor {
                display_name: "Perlin 2D",
                category: NodeCategory::Source,
                color: [0x4c, 0x9a, 0xff],
                inputs: NO_PINS,
                outputs: PERLIN2D_OUTPUTS,
            },
            NodeKind::Threshold(_) => NodeDescriptor {
                display_name: "Threshold",
                category: NodeCategory::Curves,
                color: [0xff, 0xb3, 0x4c],
                inputs: DENSITY_IN,
                outputs: DENSITY_OUT,
            },
            NodeKind::Output(_) => NodeDescriptor {
                display_name: "Output",
                category: NodeCategory::Output,
                color: [0xe0, 0x4c, 0x4c],
                inputs: DENSITY_IN,
                outputs: NO_PINS,
            },
            NodeKind::Constant(_) => NodeDescriptor {
                display_name: "Constant",
                category: NodeCategory::Source,
                color: [0x6c, 0xc0, 0x6c],
                inputs: NO_PINS,
                outputs: SCALAR_OUT,
            },
            NodeKind::Add(_) => NodeDescriptor {
                display_name: "Add",
                category: NodeCategory::Math,
                color: [0x9c, 0x7c, 0xff],
                inputs: ADD_INPUTS,
                outputs: DENSITY_OUT,
            },
            NodeKind::WorldPos(_) => NodeDescriptor {
                display_name: "World Position",
                category: NodeCategory::Source,
                color: [0x4c, 0x9a, 0xff],
                inputs: NO_PINS,
                outputs: VEC3_OUT,
            },
        }
    }

    /// Stable string identifier matching the serde `"type"` tag.
    pub fn type_name(&self) -> &'static str {
        match self {
            NodeKind::Perlin2D(_) => "Perlin2D",
            NodeKind::Threshold(_) => "Threshold",
            NodeKind::Output(_) => "Output",
            NodeKind::Constant(_) => "Constant",
            NodeKind::Add(_) => "Add",
            NodeKind::WorldPos(_) => "WorldPos",
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
