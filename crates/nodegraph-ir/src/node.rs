//! Nodes: stable IDs, polymorphic kinds + params, and static descriptors.

use glam::Vec2;
use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::boundary::{EffectivePin, ResolvedBoundary, ResolvedPin};
use crate::crossgraph::GraphRefTarget;
use crate::graph::GraphKind;
use crate::library::LibraryGraphId;
use crate::pin::PinType;
use voxel_core::ResolvedBlueprint;

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
    /// Biome routing.
    Biome,
    /// Library graph references.
    Library,
    /// Cross-graph references into the hierarchy.
    Graph,
    /// Foliage placement (DetailGraph).
    Foliage,
    /// Terminal output.
    Output,
}

impl NodeCategory {
    /// Every category, in menu order. The editor's node menu iterates this
    /// rather than a list of its own, so a new category cannot be added without
    /// appearing there - the previous handwritten list has already gone stale
    /// and omitted `Library`, which would have hidden every reference node.
    pub const ALL: [NodeCategory; 14] = [
        NodeCategory::Source,
        NodeCategory::Math,
        NodeCategory::Curves,
        NodeCategory::Domain,
        NodeCategory::Density,
        NodeCategory::Material,
        NodeCategory::Positions,
        NodeCategory::Scanners,
        NodeCategory::Props,
        NodeCategory::Biome,
        NodeCategory::Library,
        NodeCategory::Graph,
        NodeCategory::Foliage,
        NodeCategory::Output,
    ];
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

/// World coordinate axis selector for [`NodeKind::WorldAxis`].
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Axis {
    /// World X.
    X,
    /// World Y (vertical) - the common case for elevation gradients.
    #[default]
    Y,
    /// World Z.
    Z,
}

/// Parameters for [`NodeKind::WorldAxis`]: emit one world-space coordinate
/// axis as a scalar field. No inputs.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct WorldAxisParams {
    /// Which world axis to emit.
    pub axis: Axis,
}

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

/// Parameters for [`NodeKind::ConstantMaterial`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ConstantMaterialParams {
    /// The material emitted at every position.
    pub material: voxel_core::MaterialId,
}

impl Default for ConstantMaterialParams {
    fn default() -> Self {
        Self { material: voxel_core::MaterialId(1) } // 1 = stone-ish
    }
}

/// Parameters for [`NodeKind::Layer`]: cake-style top-down material stacking.
///
/// For each `(x, z)` column the input density determines the surface Y; cells
/// above it emit `AIR`. Cells at or below the surface walk `bands` top-down.
/// accumulating thickness; the band whose range covers `depth = surface_y - y`
/// wins. Past the last band, every remaining cell uses `fill`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct LayerParams {
    /// Top-down `(material, thickness_in_voxels)` pairs.
    pub bands: Vec<(voxel_core::MaterialId, u32)>,
    /// Material applied below the last band.
    pub fill: voxel_core::MaterialId,
}

impl Default for LayerParams {
    fn default() -> Self {
        // Grass on top, dirt under, stone fill below.
        Self {
            bands: vec![
                (voxel_core::MaterialId(6), 1), // grass soil
                (voxel_core::MaterialId(3), 3), // soil
            ],
            fill: voxel_core::MaterialId(1), // limestone
        }
    }
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
/// Parameters for [`NodeKind::Abs`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct AbsParams {}
/// Parameters for [`NodeKind::SurfaceToDensity`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct SurfaceToDensityParams {}
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
/// Parameters for [`NodeKind::Queue`] (no parameters yet - 2-ary fall-through).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct QueueParams {}
/// Parameters for [`NodeKind::TerrainOutput`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct TerrainOutputParams {}
/// Parameters for [`NodeKind::BuildTerrain`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct BuildTerrainParams {}
/// Parameters for [`NodeKind::DensityOutput`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct DensityOutputParams {}
/// Parameters for [`NodeKind::FluidOutput`] (no parameters yet).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)] pub struct FluidOutputParams {}

/// Parameters for [`NodeKind::JitteredGrid`]: points on a regular XZ grid,
/// each cell offset by a per-cell random jitter, kept with probability
/// `density`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct JitteredGridParams {
    /// Per-node seed; combined with world cell coords for seam-correctness.
    pub seed: u32,
    /// Grid cell size in world units.
    pub cell_size: f32,
    /// Max jitter as a fraction of a cell (0 = centered, 1 = anywhere in cell).
    pub jitter: f32,
    /// Probability `[0,1]` that a given cell emits a point.
    pub density: f32,
}
impl Default for JitteredGridParams {
    fn default() -> Self { Self { seed: 0, cell_size: 6.0, jitter: 0.6, density: 0.5 } }
}

/// Parameters for [`NodeKind::PoissonDisk`]: Bridson blue-noise scatter with a
/// minimum spacing of `radius`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PoissonDiskParams {
    /// Per-node seed (per-chunk; not seam-continuous).
    pub seed: u32,
    /// Minimum distance between any two points, in world units.
    pub radius: f32,
    /// Candidate attempts per active point (Bridson `k`).
    pub k: u32,
}
impl Default for PoissonDiskParams {
    fn default() -> Self { Self { seed: 0, radius: 4.0, k: 30 } }
}

/// Parameters for [`NodeKind::FindFlat`]: keep only points whose surface is
/// flat (neighbor surface Y within `max_step` and, optionally, on one of the
/// listed materials (empty = any solid).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct FindFlatParams {
    /// Max allowed surface-Y difference to any 8-neighbor column.
    pub max_step: i32,
    /// Allowed surface materials. Empty = accept any solid surface.
    pub on_materials: Vec<voxel_core::MaterialId>,
}
impl Default for FindFlatParams {
    fn default() -> Self { Self { max_step: 1, on_materials: Vec::new() } }
}

/// Per-species shape parameters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
// Every field defaults, so a species file declares only what differs from a
// generic broadleaf - and adding a field later cannot break one already on
// disk, which is the half of versioning a version number does not cover.
#[serde(default)]
pub struct TreeSpecies {
    /// Radius of the trunk at the ground, in voxels. **Authored, not derived.**
    ///
    /// The pipe model inverts exactly - root area is the sum over every tip, so
    /// `trunk = tip * tips^(1/PIPE_EXP)` and therefore
    /// `tip = trunk / tips^(1/PIPE_EXP)`. Reading it this way makes trunk
    /// thickness a species property instead of a side effect of how many twigs
    /// a species happens to generate, which is what it was: changing the branch
    /// structure moved the trunk by nearly 4x and nothing said so.
    ///
    /// It is also the only radius that interacts with the voxel grid, so it is
    /// what decides whether a tree collides at all.
    pub trunk_radius: f32,
    /// Radius of a branch tip, in voxels. **Also authored**, and the reason the
    /// profile exponent is solved rather than fixed.
    ///
    /// Deriving this from `trunk_radius` and the tip count - as an area-
    /// conserving pipe model must - gives `trunk / tips^(1/PIPE_EXP)`. A real
    /// tree has on the order of 10^5 twigs, so that ratio is large. A skeleton
    /// with 25 terminals gets a ratio of 4, and every branch ends in a club.
    /// Author both ends and let the exponent absorb the difference.
    pub tip_radius: f32,
    /// Between-tree variation, as a fraction, applied to internode length and
    /// branch angle. On top of the root yaw and phyllotactic phase, which are
    /// always randomized per tree.
    pub variance: f32,
    /// Length of the first internode, in voxels.
    pub internode: f32,
    /// Length multiplier at each successive step along one axis.
    pub taper: f32,
    /// Length multiplier applied when a lateral leaves its parent axis.
    pub child_scale: f32,
    /// Internodes an axis extends by before it terminates.
    pub axis_steps: u8,
    /// Deepest branching order. 0 is a bare trunk.
    pub max_order: u8,
    /// Laterals emitted per internode.
    pub laterals: u8,
    /// Angle a lateral leaves its parent axis at, in radians.
    pub branch_angle: f32,
    /// Per-step bend toward world up (positive) or away from it (negative), as
    /// a fraction of the full correction. Gravitropism.
    pub gravitropism: f32,
    /// Random angular wobble per internode, in radians.
    pub wobble: f32,
    /// Attraction points scattered through the crown. 0 disables colonization
    /// entirely, leaving the turtle skeleton alone.
    pub crown_attractors: u16,
    /// Crown ellipsoid center height, as a fraction of the turtle's own height.
    pub crown_center: f32,
    /// Crown ellipsoid horizontal radius, in voxels.
    pub crown_radius: f32,
    /// Crown ellipsoid vertical radius, in voxels.
    pub crown_height: f32,
    /// How far a node can see an attractor. Larger fills faster and straighter.
    pub influence: f32,
    /// Attractors this close to a node are consumed. Sized against
    /// `colonize_step` and measured rather than reasoned: at ~1.5x the step the
    /// crown exhausts its attractors at about half the node cap, so every age
    /// above 0.5 produces the identical tree. Below ~0.8x the step the budget
    /// is the binding constraint at every age, which is what makes the age
    /// range live end to end.
    pub kill_radius: f32,
    /// Length of one colonization growth, in voxels.
    pub colonize_step: f32,
    /// Most lobes a canopy may carry. 0 disables the canopy entirely.
    pub max_lobes: u8,
    /// Lobe radius per unit of supplying cross-section.
    pub lobe_scale: f32,
    /// Per-lobe random size variation, as a fraction.
    pub lobe_size_jitter: f32,
    /// Per-axis random stretch, as a fraction, making a lobe an ellipsoid.
    pub lobe_anisotropy: f32,
    /// Lobes closer than this fraction of their summed radii are merged.
    pub lobe_separation: f32,
    /// Lobes smaller than this are dropped, in voxels.
    pub lobe_min_radius: f32,
    /// Declared upper bound on horizontal crown reach, in voxels. A **bound,
    /// not a target**: the margin band is sized from it before generation runs,
    /// so it cannot be derived from the tree it bounds.
    ///
    /// Nothing checks it yet. The check belongs to the feature source that
    /// sizes the band, and asserting here would panic the app while sliders are
    /// being dragged in the debug panel.
    pub max_crown_reach: f32,
}

impl Default for TreeSpecies {
    /// A generic broadleaf. Tuning is content work, not a code default.
    ///
    /// `axis_steps` and `max_order` are deliberately modest: the turtle's job
    /// is the trunk and primary limbs, and a turtle that spends the whole node
    /// budget leaves the crown with nothing. These terminate at 56 nodes.
    fn default() -> Self {
        Self {
            trunk_radius: 1.0,
            tip_radius: 0.06,
            variance: 0.25,
            internode: 1.2,
            taper: 0.88,
            child_scale: 0.7,
            axis_steps: 5,
            max_order: 2,
            laterals: 1,
            branch_angle: 0.9,
            gravitropism: 0.25,
            wobble: 0.18,
            crown_attractors: 0,
            crown_center: 0.85,
            crown_radius: 3.5,
            crown_height: 2.5,
            influence: 4.0,
            kill_radius: 0.35,
            colonize_step: 0.45,
            max_lobes: 6,
            lobe_scale: 1.6,
            lobe_size_jitter: 0.28,
            lobe_anisotropy: 0.22,
            lobe_separation: 0.75,
            lobe_min_radius: 0.4,
            max_crown_reach: 8.0,
        }
    }
}

/// A species and its wood material, resolved at the hot-reload boundary.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ResolvedTree {
    /// Shape parameters, loaded from `<species>.species.json`.
    pub species: TreeSpecies,
    /// Wood material id, resolved from the node's material name.
    pub wood: voxel_core::MaterialId,
}

/// Parameters for [`NodeKind::PlaceTree`]: a rule producing trees on a coarse
/// world grid, in a ZoneGraph.
///
/// A declaration rather than a dataflow node, exactly like
/// [`NodeKind::PlaceStructure`] - generation scans the zone graph for these
/// rather than evaluating them in a chain, so it carries no pins.
///
/// The grid fields deliberately mirror `PlaceStructureParams`, because both
/// become a `FeatureSource` and the cell grid, density roll and ordering are
/// one implementation shared by both payloads. Folding the two nodes into one
/// with a payload enum is the tidier end state and is **deferred**: it would be
/// a wire-format change to a node that ships in live content, and the trigger
/// for paying that is a third payload kind.
///
/// Two fields `PlaceStructureParams` has are deliberately absent. `random_yaw`
/// buys nothing when every seed already grows a differently-shaped tree, and
/// `surface_offset` is fixed at 0 because a trunk stands *on* its anchor voxel
/// where a blueprint's first cell replaces it - a field whose only correct
/// value is one is a field that only makes wrong worlds reachable.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PlaceTreeParams {
    /// Species name; resolves to `<name>.species.json` in the species dir.
    pub species: String,
    /// Wood material, **by name**. Numeric ids in assets are what D5 exists to
    /// undo, and a new node should not add another one.
    pub wood: String,
    /// Edge length of a feature cell, in voxels. One candidate per cell.
    pub cell_size: i32,
    /// Chance a cell produces a tree, `0..=1`.
    pub density: f32,
    /// Node-local seed, mixed with the world seed and the cell coordinates.
    pub seed: u32,
    /// Maturity, `0..=1`. A node budget rather than a scale - see `TreeSpecies`.
    pub age: f32,
    /// Biomes this source appears in. Empty means every biome in the zone.
    #[serde(default)]
    pub biomes: Vec<u16>,
    /// Resolved species + wood (filled by `resolve_species`; `None` until then).
    #[serde(skip)]
    pub resolved: Option<ResolvedTree>,
}

impl Default for PlaceTreeParams {
    fn default() -> Self {
        Self {
            species: "oak".to_string(),
            wood: "wood".to_string(),
            cell_size: 24,
            density: 0.35,
            seed: 1,
            age: 1.0,
            biomes: Vec::new(),
            resolved: None,
        }
    }
}

/// Parameters for [`NodeKind::PlaceBlueprint`]: stamp a named voxel template.
/// `resolved` is loaded from disk and material-resolved at the hot-reload
/// boundary and is not serialized (the `blueprint` name is the source of truth
/// on disk).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PlaceBlueprintParams {
    /// Blueprint name; resolves to `<name>.blueprint.json` in the blueprint dir.
    pub blueprint: String,
    /// Resolved template (filled by `resolve_blueprints`; `None` until then).
    #[serde(skip)]
    pub resolved: Option<ResolvedBlueprint>,
}
impl Default for PlaceBlueprintParams {
    fn default() -> Self { Self { blueprint: "rock".to_string(), resolved: None } }
}

/// Foliage a structure occurrence anchors, in addition to the voxels it stamps.
/// 
/// A tree is the case this exists for: the trunk is a blueprint that occupies
/// the voxel grid - so it collides and can be broken - and the canopy is foliage
/// that does not. Declaring both on one node is what keeps them at the same
/// place; two independent placements would have to agree, and eventually would
/// not.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct StructureCanopy {
    /// Scatter type bucket the instance lands in.
    pub type_id: u16,
    /// Prefab instanced at the anchor.
    pub prefab_id: u32,
    /// Voxels above the structure's anchor the canopy sits at - normally the
    /// trunk's height, so the canopy crowns it.
    pub y_offset: i32,
}

impl Default for StructureCanopy {
    fn default() -> Self {
        Self { type_id: 0, prefab_id: 0, y_offset: 4 }
    }
}

/// Parameters for [`NodeKind::PlaceStructure`]: a rule producing structure
/// occurrences on a coarse world grid (design §5, cross-chunk features).
///
/// There is deliberately no zone field. Structures live on the ZoneGraph, so
/// the zone is the graph the node sits in - which makes it impossible to author
/// a structure whose declared zone disagrees with where it was written.
///
/// `resolved` is filled at the hot-reload boundary and is not serialized, the
/// same arrangement [`PlaceBlueprintParams`] uses.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PlaceStructureParams {
    /// Blueprint name; resolves to `<name>.blueprint.json`.
    pub blueprint: String,
    /// Edge length of a feature cell, in voxels. One candidate per cell.
    pub cell_size: i32,
    /// Chance a cell produces an occurrence, `0..=1`.
    pub density: f32,
    /// Node-local seed, mixed with the world seed and cell coordinates.
    pub seed: u32,
    /// Whether each occurrence takes a random quarter turn.
    pub random_yaw: bool,
    /// How far above the surface the anchor lands.
    pub surface_offset: i32,
    /// Biomes this source appears in. Empty means every biome in the zone.
    #[serde(default)]
    pub biomes: Vec<u16>,
    /// Foliage each occurrence anchors, if any. `None` is a plain structure.
    #[serde(skip)]
    pub canopy: Option<StructureCanopy>,
    /// Resolved template (filled by `resolve_blueprints`; `None` until then).
    #[serde(skip)]
    pub resolved: Option<ResolvedBlueprint>,
}
impl Default for PlaceStructureParams {
    fn default() -> Self {
        Self {
            blueprint: "rock".to_string(),
            cell_size: 48,
            density: 0.25,
            seed: 1,
            random_yaw: true,
            surface_offset: 1,
            biomes: Vec::new(),
            canopy: None,
            resolved: None,
        }
    }
}

/// Parameters of a river network (design §5, "River networks").
///
/// Lives here rather than in the evaluator for the same reason [`NoiseParams`]
/// does: a node's parameters are IR, serialized and edited and diffed, while the
/// field they describe is evaluation.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct RiverParams {
    /// Edge length of a network cell, in voxels. One node per cell.
    pub cell_size: f32,
    /// Node-local seed, mixed with the world seed.
    pub seed: u32,
    /// Frequency of the elevation-potential field.
    pub potential_frequency: f32,
    /// World-Y the potential maps onto at potential -1.
    pub bed_low: f32,
    /// World-Y the potential maps onto at potential +1.
    pub bed_high: f32,
    /// Half-width at the headwaters, in voxels.
    pub min_width: f32,
    /// Half-width at the mouth, in voxels.
    pub max_width: f32,
    /// How far below the bank the channel center cuts.
    pub depth: f32,
}

impl Default for RiverParams {
    fn default() -> Self {
        Self {
            cell_size: 128.0,
            seed: 1,
            potential_frequency: 0.004,
            bed_low: 14.0,
            bed_high: 46.0,
            min_width: 3.0,
            max_width: 9.0,
            depth: 4.0,
        }
    }
}

/// Parameters for [`NodeKind::LibraryRef`]: a reference to a reusable library
/// graph by id. The referenced library's boundary is resolved into this node's
/// pins by [`Graph::resolve_library_refs`](crate::Graph::resolve_library_refs)
/// and cached in `resolved` (recomputed on load; not serialized).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct LibraryRefParams {
    /// The library graph this node instantiates.
    pub library: LibraryGraphId,
    /// Pins resolved from the referenced library's boundary. `None` until a
    /// resolve pass runs, in which case the node presents no pins.
    #[serde(skip)]
    pub resolved: Option<ResolvedBoundary>,
}

/// Equality ignores the cached `resolved` pins (derived state): two references
/// are equal when they target the same library.
impl PartialEq for LibraryRefParams {
    fn eq(&self, other: &Self) -> bool {
        self.library == other.library
    }
}

/// Parameters for [`NodeKind::GraphRef`]: a reference to another graph in the
/// hierarchy (the World graph, the Zone graph, or a Biome graph). The referenced
/// graph's declared boundary outputs are resolved into this node's output pins
/// where the hierarchy is assembled (a later substep) and cached in `resolved`
/// (recomputed on load; not serialized).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct GraphRefParams {
    /// Which hierarchy graph this node reads from.
    pub target: GraphRefTarget,
    /// Pins resolved from the referenced graph's boundary. `None` until a
    /// resolve pass runs, in which case the node presents no pins.
    #[serde(skip)]
    pub resolved: Option<ResolvedBoundary>,
}

/// Equality ignores the cached `resolved` pins (derived state); two references
/// are equal when they name the same target.
impl PartialEq for GraphRefParams {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target
    }
}

/// Parameters for [`NodeKind::GraphOutput`]: marks this node's input value as a
/// named boundary output of its graph, importable cross-graph by a `GraphRef`.
/// A graph's [`GraphBoundary`](crate::GraphBoundary) outputs are derived from
/// its `GraphOutput` nodes (see
/// [`Graph::derive_output_boundary`](crate::Graph::derive_output_boundary)).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct GraphOutputParams {
    /// The boundary output name this node exposes.
    pub name: String,
}

/// Parameters for [`NodeKind::WorldOutput`]: the WorldGraph's terminal. Maps a
/// per-column climate [`SurfaceField`](crate::PinType::SurfaceField) to a
/// discrete zone id by counting how many ascending `zone_bands` thresholds each
/// column value meets or exceeds. Empty bands => zone 0 everywhere (single zone).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct WorldOutputParams {
    /// Ascending climate thresholds partitioning columns into bands. N thresholds
    /// => N+1 band regions; region k = how many thresholds a column's value meets.
    pub zone_bands: Vec<f32>,
    /// Zone id assigned to each band region (index 0..=zone_bands.len()). A missing
    /// entry defaults to the region index, so graphs without it load unchanged.
    #[serde(default)]
    pub zone_ids: Vec<u16>,
}

/// Parameters for [`NodeKind::ZoneOutput`]: the ZoneGraph's terminal. Maps a
/// per-column climate [`SurfaceField`](crate::PinType::SurfaceField) to a
/// discrete biome id by counting how many ascending `biome_bands` thresholds
/// each column value meets or exceeds. Empty bands => biome 0 everywhere.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct ZoneOutputParams {
    /// Ascending climate thresholds partitioning columns into bands. N thresholds
    /// => N+1 band regions; region k = how many thresholds a column's value meets.
    pub biome_bands: Vec<f32>,
    /// Biome id assigned to each band region (index 0..=biome_bands.len()). A
    /// missing entry defaults to the region index (legacy behavior), so graphs
    /// authored before explicit assignment load unchanged. Because the artist
    /// picks the biome per region, ids need not follow band order.
    #[serde(default)]
    pub biome_ids: Vec<u16>,
}

/// Parameters for [`NodeKind::YBand`]: a vertical density gate. Emits `1.0`
/// where `min <= worldY < max` and `0.0` elsewhere - multiply or mask it
/// against a shape to confine that shape to a vertical band. Open-ended bands
/// (everything below / above a height) use an extreme `min` or `max`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct YBandParams {
    /// Inclusive lower world-Y bound.
    pub min: f32,
    /// Exclusive upper world-Y bound.
    pub max: f32,
}
impl Default for YBandParams {
    fn default() -> Self { Self { min: 0.0, max: 64.0 } }
}

/// Parameters for [`NodeKind::PoissonDistribution`]: blue-noise candidate points
/// over the chunk's XZ footprint.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PoissonDistributionParams {
    /// Per-node seed (combined with world seed + chunk for determinism).
    pub seed: u32,
    /// Minimum spacing between points, world units.
    pub radius: f32,
    /// Per-point positional jitter as a fraction of `radius` (`0..1`).
    pub jitter: f32,
}
impl Default for PoissonDistributionParams {
    fn default() -> Self { Self { seed: 0, radius: 4.0, jitter: 0.6 } }
}

/// Parameters for [`NodeKind::SurfaceFilter`]: keep candidates whose surface is
/// shallow enough, within a height band, and (optionally) on allowed materials.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SurfaceFilterParams {
    /// Max surface slope to keep (steeper rejected).
    pub max_slope: f32,
    /// Minimum surface world-Y to keep.
    pub min_height: f32,
    /// Maximum surface world-Y to keep.
    pub max_height: f32,
    /// Allowed surface materials. Empty = any solid surface.
    pub materials: Vec<voxel_core::MaterialId>,
}
impl Default for SurfaceFilterParams {
    fn default() -> Self {
        Self { max_slope: 1.0, min_height: -1024.0, max_height: 1024.0, materials: Vec::new() }
    }
}

/// Parameters for [`NodeKind::BiomeContextMask`]: keep only candidates whose
/// column is assigned this biome id.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct BiomeContextMaskParams {
    /// Biome id candidates must match.
    pub biome: u16,
}

/// Parameters for [`NodeKind::SpeciesPicker`]: assign each candidate a species
/// index by weighted random choice.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SpeciesPickerParams {
    /// Per-node seed.
    pub seed: u32,
    /// Weight per species index (`weights[i]` weights species `i`).
    pub weights: Vec<f32>,
}
impl Default for SpeciesPickerParams {
    fn default() -> Self { Self { seed: 0, weights: vec![1.0] } }
}

/// Parameters for [`NodeKind::PaintDensity`]: a Tier-1 terminal writing a detail
/// (foliage paint) layer. The optional `SurfaceField` input modulates density.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PaintDensityParams {
    /// Target detail layer id (app `DetailLayerId`).
    pub layer_id: u16,
    /// Species variant within the layer (`0` = none).
    pub species: u8,
    /// Base density `0..=255` (scaled by the optional input field).
    pub density: u8,
    /// Tint palette index.
    pub tint: u8,
}
impl Default for PaintDensityParams {
    fn default() -> Self { Self { layer_id: 0, species: 1, density: 255, tint: 0 } }
}

/// Parameters for [`NodeKind::ScatterPlace`]: a Tier-2/3 terminal writing scatter
/// instances for each assigned candidate.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct ScatterPlaceParams {
    /// Per-node seed (rotation/scale variation).
    pub seed: u32,
    /// Scatter type bucket (app `ScatterTypeId`).
    pub type_id: u16,
    /// Prefab to instance (app `PrefabId`).
    pub prefab_id: u32,
}

/// Parameters for [`NodeKind::BiomeParam`]: read a named per-biome scalar from
/// the biome-param sidecar, or `default` when the active biome has no such entry
/// (or no sidecar is threaded).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct BiomeParamParams {
    /// The parameter name to read.
    pub name: String,
    /// Fallback value when the parameter is absent.
    pub default: f32,
}

/// Parameters for [`NodeKind::WorldParam`]: read a named world-level scalar from
/// the world-param sidecar, or `default` when the world declares no such entry
/// (or no sidecar is threaded).
/// 
/// The world analogue of [`BiomeParamParams`]. Separate rather than shared
/// because the two read different sidecars, and a node whose meaning depends on
/// which evaluator happens to hold it is not a node an author can reason about.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct WorldParamParams {
    /// The parameter name to read.
    pub name: String,
    /// Fallback value when the parameter is absent.
    pub default: f32,
}

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
    /// World-space coordinate axis as a scalar field (no inputs).
    WorldAxis(WorldAxisParams),
    /// Vertical density gate: `1.0` inside `[min, max)` world-Y, else `0.0`.
    YBand(YBandParams),
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
    /// Absolute value (`|v|`).
    Abs(AbsParams),
    // --- Density combinators ---
    /// Lift a per-column [`SurfaceField`](crate::PinType::SurfaceField) into a
    /// density field, constant down each column. The one door from the column
    /// domain into a density chain; nothing coerces between the two.
    SurfaceToDensity(SurfaceToDensityParams),
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
    // --- Material providers ---
    /// Uniform-material source.
    ConstantMaterial(ConstantMaterialParams),
    /// Cake-style top-down material stacking, surface-relative.
    Layer(LayerParams),
    /// 2-ary priority chain; `primary`'s value wins unless it's `AIR`,
    /// in which case `fallback` is used.
    Queue(QueueParams),
    // --- Terminal ---
    /// Terrain terminal: consumes a density + a material field, produces
    /// a `ChunkBuffer<Voxel>` of cubes.
    TerrainOutput(TerrainOutputParams),
    /// Builds a terrain (`ChunkBuffer<Voxel>`) from density + material fields.
    BuildTerrain(BuildTerrainParams),
    /// Biome terminal: exposes a continuous density + material as this biome's
    /// contribution, composited and fused to voxels post-composition.
    DensityOutput(DensityOutputParams),
    /// Biome fluid terminal: `level` (Scalar water-surface world-Y) + `mask`
    /// (Density; pond present where the per-column value > 0). Harvested per
    /// column by the world evaluator, not the single-graph density evaluator.
    FluidOutput(FluidOutputParams),
    // --- Positions ---
    /// Jittered-grid point scatter.
    JitteredGrid(JitteredGridParams),
    /// Poisson-disk (blue-noise) point scatter.
    PoissonDisk(PoissonDiskParams),
    // --- Scanners ---
    /// Annotate points with surface Y; reject steep / wrong-material spots.
    FindFlat(FindFlatParams),
    // --- Props ---
    /// Stamp a procedural tree (trunk + canopy) at each point.
    PlaceTree(PlaceTreeParams),
    /// Stamp a named blueprint at each point.
    PlaceBlueprint(PlaceBlueprintParams),
    /// Carve a river network out of a density field.
    River(RiverParams),
    /// Declare a structure source on the owning ZoneGraph. Carries no pins: the
    /// evaluator scans for these rather than pulling a value through them.
    PlaceStructure(PlaceStructureParams),
    // --- Per-column (World/Zone graphs) ---
    /// 2D world-space noise emitting a per-column `SurfaceField` (a climate
    /// channel). Evaluated by the column evaluator, not the voxel evaluator.
    SurfaceNoise(NoiseParams),
    /// WorldGraph terminal: assigns a per-column zone id from a climate field.
    WorldOutput(WorldOutputParams),
    /// ZoneGraph terminal: assigns a per-column biome id from a climate field
    ZoneOutput(ZoneOutputParams),
    // --- References ---
    /// References a reusable library graph by id, exposing the library's
    /// declared boundary as this node's pins (resolved at load time).
    LibraryRef(LibraryRefParams),
    /// References another graph in the hierarchy (World/Zone/Biome), exposing
    /// its declared boundary outputs as this node's pins.
    GraphRef(GraphRefParams),
    /// Marks its input value as a named boundary output of this graph (imported
    /// cross-graph by a `GraphRef`).
    GraphOutput(GraphOutputParams),
    /// Terminal node; consumes one density input.
    Output(OutputParams),
    // --- Foliage (DetailGraph) ---
    /// Blue-noise candidate points over the chunk footprint.
    PoissonDistribution(PoissonDistributionParams),
    /// Filter candidate points by surface slope / height / material.
    SurfaceFilter(SurfaceFilterParams),
    /// Filter candidate points to a biome.
    BiomeContextMask(BiomeContextMaskParams),
    /// Assign each candidate a species by weighted choice.
    SpeciesPicker(SpeciesPickerParams),
    /// Tier-1 terminal: write a detail (foliage paint) layer.
    PaintDensity(PaintDensityParams),
    /// Tier-2/3 terminal: write scatter instances.
    ScatterPlace(ScatterPlaceParams),
    /// Reads a named per-biome scalar parameter (the biome-param sidecar) as a
    /// uniform scalar field, falling back to a node default when unset.
    BiomeParam(BiomeParamParams),
    /// Read a named scalar from the world-param sidecar.
    WorldParam(WorldParamParams),
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
const MATERIAL_OUT: &[PinSpec] =
    &[PinSpec { name: "material", ty: PinType::Material, required: false }];
const QUEUE_IN: &[PinSpec] = &[
    PinSpec { name: "primary",  ty: PinType::Material, required: true },
    PinSpec { name: "fallback", ty: PinType::Material, required: true },
];
const TERRAIN_OUT: &[PinSpec] =
    &[PinSpec { name: "terrain", ty: PinType::Terrain, required: false }];
const BUILD_TERRAIN_IN: &[PinSpec] = &[
    PinSpec { name: "density",  ty: PinType::Density,  required: true },
    PinSpec { name: "material", ty: PinType::Material, required: true },
];
const FLUID_OUTPUT_IN: &[PinSpec] = &[
    PinSpec { name: "level", ty: PinType::Scalar,  required: true },
    PinSpec { name: "mask",  ty: PinType::Density, required: true },
];
const TERRAIN_TERMINAL_IN: &[PinSpec] =
    &[PinSpec { name: "terrain", ty: PinType::Terrain, required: true }];
const POSITIONS_OUT: &[PinSpec] =
    &[PinSpec { name: "positions", ty: PinType::Positions, required: false }];
const SCAN_IN: &[PinSpec] = &[
    PinSpec { name: "terrain", ty: PinType::Terrain,   required: true },
    PinSpec { name: "points",  ty: PinType::Positions, required: true },
];
const PLACE_IN: &[PinSpec] = &[
    PinSpec { name: "terrain", ty: PinType::Terrain,   required: true },
    PinSpec { name: "points",  ty: PinType::Positions, required: true },
];
const SURFACE_OUT: &[PinSpec] =
    &[PinSpec { name: "surface", ty: PinType::SurfaceField, required: false }];
const SURFACE_IN_ANY: &[PinSpec] =
    &[PinSpec { name: "surface", ty: PinType::SurfaceField, required: true }];
const SURFACE_IN_REQUIRED: &[PinSpec] =
    &[PinSpec { name: "zone field", ty: PinType::SurfaceField, required: true }];
const SURFACE_IN_BIOME: &[PinSpec] =
    &[PinSpec { name: "biome field", ty: PinType::SurfaceField, required: true }];
const GRAPH_OUTPUT_IN: &[PinSpec] =
    &[PinSpec { name: "value", ty: PinType::SurfaceField, required: true }];
const POSITIONS_IN: &[PinSpec] =
    &[PinSpec { name: "points", ty: PinType::Positions, required: true }];
const ASSIGNMENTS_OUT: &[PinSpec] =
    &[PinSpec { name: "assignments", ty: PinType::Assignments, required: false }];
const ASSIGNMENTS_IN: &[PinSpec] =
    &[PinSpec { name: "assignments", ty: PinType::Assignments, required: true }];
const SURFACE_IN_OPTIONAL: &[PinSpec] =
    &[PinSpec { name: "density", ty: PinType::SurfaceField, required: false }];
const PAINT_OUT: &[PinSpec] =
    &[PinSpec { name: "paint", ty: PinType::PaintOutput, required: false }];
const SCATTER_OUT: &[PinSpec] =
    &[PinSpec { name: "scatter", ty: PinType::ScatterOutput, required: false }];

impl NodeKind {
    /// Static descriptor (display, color, typed pins) for this kind.
    pub fn descriptor(&self) -> NodeDescriptor {
        const SRC: [u8;3]   = [0x4c, 0x9a, 0xff];
        const MATH: [u8;3]  = [0x9c, 0x7c, 0xff];
        const CURVE: [u8;3] = [0xff, 0xb3, 0x4c];
        const DOM: [u8;3]   = [0xff, 0x7c, 0xc4];
        const DENS: [u8;3]  = [0x6c, 0xc0, 0x6c];
        const OUT: [u8;3]   = [0xe0, 0x4c, 0x4c];
        const FOLIAGE: [u8;3] = [0x6c, 0xb0, 0x4c];
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
            NodeKind::WorldAxis(_) => NodeDescriptor {
                display_name: "World Axis",
                category: NodeCategory::Source,
                color: SRC,
                inputs: NO_PINS,
                outputs: SCALAR_OUT
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
            NodeKind::Abs(_) => NodeDescriptor {
                display_name: "Abs",
                category: NodeCategory::Math,
                color: MATH,
                inputs: DENSITY_IN,
                outputs: DENSITY_OUT
            },
            NodeKind::SurfaceToDensity(_) => NodeDescriptor {
                display_name: "Surface To Density",
                category: NodeCategory::Density,
                color: DENS,
                inputs: SURFACE_IN_ANY,
                outputs: DENSITY_OUT
            },
            // Curves
            NodeKind::River(_) => NodeDescriptor {
                display_name: "River",
                category: NodeCategory::Density,
                color: DENS,
                inputs: DENSITY_IN,
                outputs: DENSITY_OUT
            },
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
            NodeKind::ConstantMaterial(_) => NodeDescriptor {
                display_name: "Constant Material",
                category: NodeCategory::Material,
                color: [0xe0, 0x4c, 0x4c],
                inputs: NO_PINS,
                outputs: MATERIAL_OUT,
            },
            NodeKind::Layer(_) => NodeDescriptor {
                display_name: "Layer",
                category: NodeCategory::Material,
                color: [0xe0, 0x4c, 0x4c],
                inputs: DENSITY_IN,
                outputs: MATERIAL_OUT,
            },
            NodeKind::Queue(_) => NodeDescriptor {
                display_name: "Queue",
                category: NodeCategory::Material,
                color: [0xe0, 0x4c, 0x4c],
                inputs: QUEUE_IN,
                outputs: MATERIAL_OUT,
            },
            NodeKind::BuildTerrain(_) => NodeDescriptor {
                display_name: "Build Terrain",
                category: NodeCategory::Material,
                color: [0xe0, 0x4c, 0x4c],
                inputs: BUILD_TERRAIN_IN,
                outputs: TERRAIN_OUT,
            },
            NodeKind::DensityOutput(_) => NodeDescriptor {
                display_name: "Density Output",
                category: NodeCategory::Output,
                color: OUT,
                inputs: BUILD_TERRAIN_IN,
                outputs: NO_PINS,
            },
            NodeKind::FluidOutput(_) => NodeDescriptor {
                display_name: "Fluid Output",
                category: NodeCategory::Output,
                color: OUT,
                inputs: FLUID_OUTPUT_IN,
                outputs: NO_PINS,
            },
            NodeKind::JitteredGrid(_) => NodeDescriptor {
                display_name: "Jittered Grid",
                category: NodeCategory::Positions,
                color: [0xe0, 0xc0, 0x4c],
                inputs: NO_PINS,
                outputs: POSITIONS_OUT,
            },
            NodeKind::PoissonDisk(_) => NodeDescriptor {
                display_name: "Poisson Disk",
                category: NodeCategory::Positions,
                color: [0xe0, 0xc0, 0x4c],
                inputs: NO_PINS,
                outputs: POSITIONS_OUT,
            },
            NodeKind::FindFlat(_) => NodeDescriptor {
                display_name: "Find Flat",
                category: NodeCategory::Scanners,
                color: [0xb0, 0xb0, 0xc0],
                inputs: SCAN_IN,
                outputs: POSITIONS_OUT,
            },
            NodeKind::PlaceTree(_) => NodeDescriptor {
                display_name: "Place Tree",
                category: NodeCategory::Props,
                // No pins: a declaration scanned out of a ZoneGraph, not a
                // stage in a density chain. Same shape as Place Structure.
                inputs: NO_PINS,
                outputs: NO_PINS,
                color: [0xc8, 0x90, 0x60],
            },
            NodeKind::PlaceBlueprint(_) => NodeDescriptor {
                display_name: "Place Blueprint",
                category: NodeCategory::Props,
                color: [0xc8, 0x90, 0x60],
                inputs: PLACE_IN,
                outputs: TERRAIN_OUT,
            },
            NodeKind::PlaceStructure(_) => NodeDescriptor {
                display_name: "Place Structure",
                category: NodeCategory::Props,
                color: [0xc8, 0x90, 0x60],
                inputs: NO_PINS,
                outputs: NO_PINS,
            },
            NodeKind::TerrainOutput(_) => NodeDescriptor {
                display_name: "Terrain Output",
                category: NodeCategory::Output, // unchanged
                color: [0xe0, 0x4c, 0x4c],
                inputs: TERRAIN_TERMINAL_IN, // shrunk from (density, material) to (terrain)
                outputs: NO_PINS,
            },
            NodeKind::LibraryRef(_) => NodeDescriptor {
                display_name: "Library Ref",
                category: NodeCategory::Library,
                color: [0xb0, 0x80, 0xff],
                inputs: NO_PINS,  // fallback; real pins come from effective_inputs()
                outputs: NO_PINS,
            },
            NodeKind::GraphRef(_) => NodeDescriptor {
                display_name: "Graph Ref",
                category: NodeCategory::Graph,
                color: [0x80, 0xa0, 0xff],
                inputs: NO_PINS,  // GraphRef has no inputs; outputs via effective_outputs()
                outputs: NO_PINS,
            },
            NodeKind::GraphOutput(_) => NodeDescriptor {
                display_name: "Graph Output",
                category: NodeCategory::Output,
                color: OUT,
                inputs: GRAPH_OUTPUT_IN,
                outputs: NO_PINS,
            },
            NodeKind::SurfaceNoise(_) => NodeDescriptor {
                display_name: "Surface Noise",
                category: NodeCategory::Source,
                color: SRC,
                inputs: NO_PINS,
                outputs: SURFACE_OUT,
            },
            NodeKind::WorldOutput(_) => NodeDescriptor {
                display_name: "World Output",
                category: NodeCategory::Output,
                color: OUT,
                inputs: SURFACE_IN_REQUIRED,
                outputs: NO_PINS,
            },
            NodeKind::ZoneOutput(_) => NodeDescriptor {
                display_name: "Zone Output",
                category: NodeCategory::Output,
                color: OUT,
                inputs: SURFACE_IN_BIOME,
                outputs: NO_PINS,
            },
            NodeKind::YBand(_) => NodeDescriptor {
                display_name: "Y Band",
                category: NodeCategory::Source,
                color: SRC,
                inputs: NO_PINS,
                outputs: DENSITY_OUT,
            },
            NodeKind::PoissonDistribution(_) => NodeDescriptor {
                display_name: "Poisson Distribution",
                category: NodeCategory::Foliage,
                color: FOLIAGE,
                inputs: NO_PINS,
                outputs: POSITIONS_OUT,
            },
            NodeKind::SurfaceFilter(_) => NodeDescriptor {
                display_name: "Surface Filter",
                category: NodeCategory::Foliage,
                color: FOLIAGE,
                inputs: POSITIONS_IN,
                outputs: POSITIONS_OUT,
            },
            NodeKind::BiomeContextMask(_) => NodeDescriptor {
                display_name: "Biome Context Mask",
                category: NodeCategory::Foliage,
                color: FOLIAGE,
                inputs: POSITIONS_IN,
                outputs: POSITIONS_OUT,
            },
            NodeKind::SpeciesPicker(_) => NodeDescriptor {
                display_name: "Species Picker",
                category: NodeCategory::Foliage,
                color: FOLIAGE,
                inputs: POSITIONS_IN,
                outputs: ASSIGNMENTS_OUT,
            },
            NodeKind::PaintDensity(_) => NodeDescriptor {
                display_name: "Paint Density",
                category: NodeCategory::Foliage,
                color: FOLIAGE,
                inputs: SURFACE_IN_OPTIONAL,
                outputs: PAINT_OUT,
            },
            NodeKind::ScatterPlace(_) => NodeDescriptor {
                display_name: "Scatter Place",
                category: NodeCategory::Foliage,
                color: FOLIAGE,
                inputs: ASSIGNMENTS_IN,
                outputs: SCATTER_OUT,
            },
            NodeKind::BiomeParam(_) => NodeDescriptor {
                display_name: "Biome Param",
                category: NodeCategory::Biome,
                color: SRC,
                inputs: NO_PINS,
                outputs: SCALAR_OUT,
            },
            NodeKind::WorldParam(_) => NodeDescriptor {
                display_name: "World Param",
                category: NodeCategory::Source,
                color: SRC,
                inputs: NO_PINS,
                outputs: SCALAR_OUT,
            },
        }
    }
    
    /// This node's effective input pins: the resolved dynamic pins of a
    /// reference node whose boundary has been resolved (see
    /// [`Graph::resolve_library_refs`](crate::Graph::resolve_library_refs)),
    /// otherwise the static [`descriptor`](NodeKind::descriptor) inputs. This is
    /// the pin set `connect`, `validate`, and the editor operate on.
    pub fn effective_inputs(&self) -> Vec<EffectivePin<'_>> {
        match self.resolved_boundary() {
            Some(rb) => rb.inputs.iter().map(resolved_pin_to_effective).collect(),
            None => self.descriptor().inputs.iter().map(spec_to_effective).collect(),
        }
    }
    
    /// This node's effective output pins - the output analogue of
    /// [`effective_inputs`](NodeKind::effective_inputs).
    pub fn effective_outputs(&self) -> Vec<EffectivePin<'_>> {
        match self.resolved_boundary() {
            Some(rb) => rb.outputs.iter().map(resolved_pin_to_effective).collect(),
            None => self.descriptor().outputs.iter().map(spec_to_effective).collect(),
        }
    }

    /// Whether this kind means the same thing one dimension down: elementwise
    /// arithmetic and curves, which read a field and write a field without
    /// caring how many dimensions it has, plus the uniform sources that fill
    /// one.
    ///
    /// These are the kinds whose declared `Density` / `Scalar` pins carry
    /// [`SurfaceField`](PinType::SurfaceField) in a per-column graph, and the
    /// set must be exactly what `ColumnEvaluator` implements - a kind that
    /// type-checks into a World graph and then has no per-column evaluation
    /// fails the chunk at generation instead of failing the author at edit time.
    /// `nodegraph-eval` asserts the two agree.
    ///
    /// Deliberately excluded: `Perlin2D` and the other noise sources, which are
    /// 2D and *look* domain-agnostic. The column domain's noise is
    /// `SurfaceNoise`, and admitting a second spelling would mean two ways to
    /// write the same field with different seeds.
    pub fn is_domain_agnostic(&self) -> bool {
        matches!(
            self,
            NodeKind::Constant(_)
                | NodeKind::WorldParam(_)
                | NodeKind::Add(_)
                | NodeKind::Subtract(_)
                | NodeKind::Multiply(_)
                | NodeKind::Min(_)
                | NodeKind::Max(_)
                | NodeKind::Clamp(_)
                | NodeKind::Lerp(_)
                | NodeKind::Remap(_)
                | NodeKind::Abs(_)
                | NodeKind::Threshold(_)
                | NodeKind::CurveMapper(_)
        )
    }

    /// Whether this kind has a per-column evaluation at all, and so may appear
    /// in a World or Zone graph.
    ///
    /// `PlaceStructure` and `PlaceTree` qualify without being evaluated: each
    /// declares a feature source on the owning ZoneGraph and carries no pins,
    /// so the column evaluator skips it rather than filling it.
    pub fn is_column_kind(&self) -> bool {
        self.is_domain_agnostic()
            || matches!(
                self,
                NodeKind::SurfaceNoise(_)
                    | NodeKind::WorldOutput(_)
                    | NodeKind::ZoneOutput(_)
                    | NodeKind::GraphOutput(_)
                    | NodeKind::GraphRef(_)
                    | NodeKind::PlaceStructure(_)
                    | NodeKind::PlaceTree(_)
            )
    }

    /// This node's input pins as they read in a graph of `kind` - the pin set
    /// `connect`, `validate` and the editor operate on.
    pub fn effective_inputs_in(&self, kind: GraphKind) -> Vec<EffectivePin<'_>> {
        let mut pins = self.effective_inputs();
        self.apply_domain(kind, &mut pins);
        pins
    }

    /// The output analogue of
    /// [`effective_inputs_in`](NodeKind::effective_inputs_in).
    pub fn effective_outputs_in(&self, kind: GraphKind) -> Vec<EffectivePin<'_>> {
        let mut pins = self.effective_outputs();
        self.apply_domain(kind, &mut pins);
        pins
    }
    
    /// Rewrite field pins into the domain `kind` evaluates in. Only the
    /// domain-agnostic kinds move; everything else declares the one domain it
    /// belongs to and keeps it.
    fn apply_domain(&self, kind: GraphKind, pins: &mut [EffectivePin<'_>]) {
        if !(kind.is_per_column() && self.is_domain_agnostic()) {
            return;
        }
        for pin in pins {
            if matches!(pin.ty, PinType::Density | PinType::Scalar) {
                pin.ty = PinType::SurfaceField;
            }
        }
    }
    
    /// The resolved boundary cached on this node, if it is a reference node
    /// (`LibraryRef`) whose pins have been resolved.
    fn resolved_boundary(&self) -> Option<&ResolvedBoundary> {
        match self {
            NodeKind::LibraryRef(p) => p.resolved.as_ref(),
            NodeKind::GraphRef(p) => p.resolved.as_ref(),
            _ => None,
        }
    }

    /// Every `MaterialId` this node's parameters reference.
    /// 
    /// Here rather than in the validator, so the knowledge lives with the node
    /// definitions that own it.
    /// 
    /// **Maintenance obligation:** a new material-carrying node kind must be
    /// added below. The wildcard arm means the compiler will not remind you, and
    /// a missing arm is a silent gap in the lint rather than a build failure -
    /// spelling out all fifty variants to buy that guarantee is not worth what
    /// it costs to read.
    pub fn material_refs(&self) -> Vec<voxel_core::MaterialId> {
        match self {
            NodeKind::ConstantMaterial(p) => vec![p.material],
            NodeKind::Layer(p) => {
                let mut out: Vec<voxel_core::MaterialId> =
                    p.bands.iter().map(|(m, _)| *m).collect();
                out.push(p.fill);
                out
            }
            NodeKind::FindFlat(p) => p.on_materials.clone(),
            NodeKind::SurfaceFilter(p) => p.materials.clone(),
            _ => Vec::new(),
        }
    }
    
    /// Stable string identifier matching the serde `"type"` tag.
    pub fn type_name(&self) -> &'static str {
        match self {
            NodeKind::Perlin2D(_)            => "Perlin2D",
            NodeKind::Perlin3D(_)            => "Perlin3D",
            NodeKind::Simplex2D(_)           => "Simplex2D",
            NodeKind::Simplex3D(_)           => "Simplex3D",
            NodeKind::Constant(_)            => "Constant",
            NodeKind::WorldPos(_)            => "WorldPos",
            NodeKind::WorldAxis(_)           => "WorldAxis",
            NodeKind::Add(_)                 => "Add",
            NodeKind::Multiply(_)            => "Multiply",
            NodeKind::Subtract(_)            => "Subtract",
            NodeKind::Min(_)                 => "Min",
            NodeKind::Max(_)                 => "Max",
            NodeKind::Clamp(_)               => "Clamp",
            NodeKind::Lerp(_)                => "Lerp",
            NodeKind::Remap(_)               => "Remap",
            NodeKind::Threshold(_)           => "Threshold",
            NodeKind::CurveMapper(_)         => "CurveMapper",
            NodeKind::DomainWarp(_)          => "DomainWarp",
            NodeKind::Union(_)               => "Union",
            NodeKind::Intersect(_)           => "Intersect",
            NodeKind::DensitySubtract(_)     => "DensitySubtract",
            NodeKind::Mix(_)                 => "Mix",
            NodeKind::Mask(_)                => "Mask",
            NodeKind::Output(_)              => "Output",
            NodeKind::ConstantMaterial(_)    => "ConstantMaterial",
            NodeKind::Layer(_)               => "Layer",
            NodeKind::Queue(_)               => "Queue",
            NodeKind::TerrainOutput(_)       => "TerrainOutput",
            NodeKind::BuildTerrain(_)        => "BuildTerrain",
            NodeKind::DensityOutput(_)       => "DensityOutput",
            NodeKind::FluidOutput(_)         => "FluidOutput",
            NodeKind::JitteredGrid(_)        => "JitteredGrid",
            NodeKind::PoissonDisk(_)         => "PoissonDisk",
            NodeKind::FindFlat(_)            => "FindFlat",
            NodeKind::PlaceTree(_)           => "PlaceTree",
            NodeKind::PlaceBlueprint(_)      => "PlaceBlueprint",
            NodeKind::PlaceStructure(_)      => "PlaceStructure",
            NodeKind::River(_)               => "River",
            NodeKind::Abs(_)                 => "Abs",
            NodeKind::SurfaceToDensity(_)    => "SurfaceToDensity",
            NodeKind::LibraryRef(_)          => "LibraryRef",
            NodeKind::GraphRef(_)            => "GraphRef",
            NodeKind::GraphOutput(_)         => "GraphOutput",
            NodeKind::SurfaceNoise(_)        => "SurfaceNoise",
            NodeKind::WorldOutput(_)         => "WorldOutput",
            NodeKind::ZoneOutput(_)          => "ZoneOutput",
            NodeKind::YBand(_)               => "YBand",
            NodeKind::PoissonDistribution(_) => "PoissonDistribution",
            NodeKind::SurfaceFilter(_)       => "SurfaceFilter",
            NodeKind::BiomeContextMask(_)    => "BiomeContextMask",
            NodeKind::SpeciesPicker(_)       => "SpeciesPicker",
            NodeKind::PaintDensity(_)        => "PaintDensity",
            NodeKind::ScatterPlace(_)        => "ScatterPlace",
            NodeKind::BiomeParam(_)          => "BiomeParam",
            NodeKind::WorldParam(_)          => "WorldParam",
        }
    }
}

/// Project a static [`PinSpec`] onto an [`EffectivePin`] (borrowing its
/// `'static` name).
fn spec_to_effective(spec: &PinSpec) -> EffectivePin<'_> {
    EffectivePin { name: spec.name, ty: spec.ty, required: spec.required }
}

/// Project a [`ResolvedPin`] onto an [`EffectivePin`] (borrowing its name).
fn resolved_pin_to_effective(pin: &ResolvedPin) -> EffectivePin<'_> {
    EffectivePin { name: &pin.name, ty: pin.ty, required: pin.required }
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
