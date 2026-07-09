//! # nodegraph-ir
//!
//! Typed dataflow graph intermediate representation for the voxulacrum
//! world-generation node system.
//!
//! A [`Graph`] is a [`slotmap::SlotMap`] of [`Node`]s plus a `Vec` of typed
//! [`Edge`]s. Each node has a polymorphic [`NodeKind`] (its parameters) and a
//! static [`NodeDescriptor`] (its display metadata and typed pin layout).
//! Edges connect an output [`PinRef`] to an input `PinRef`; they are legal
//! only between [compatible](PinType::is_compatible) pin types.
//!
//! ## What this crate does *not* do
//!
//! It does not evaluate graphs (that is the evaluator crate) and performs no
//! filesystem I/O. It embeds `voxel-core` value types (`MaterialId`, `Voxel`)
//! so that prefab templates can round-trip, but references no chunk buffer for
//! [`PinType::Terrain`] — that is a bare tag here. The IR is purely structure
//! + validation + serialization.
//!
//! ## Serialization
//!
//! [`Graph`] round-trips through JSON via `serde`. Node variants are
//! internally tagged (`#[serde(tag = "type")]`), so a node serializes as
//! `{ "type": "Perlin2D", "seed": 0, "frequency": 0.01, ... }`. Stable
//! `slotmap` keys are preserved across a round-trip, so edges keep resolving.
//!
//! ## Example
//!
//! ```
//! use nodegraph_ir::*;
//!
//! let mut g = Graph::new();
//! let perlin = g.add_node(NodeKind::Perlin2D(Perlin2DParams::default()));
//! let thresh = g.add_node(NodeKind::Threshold(ThresholdParams::default()));
//! let out = g.add_node(NodeKind::Output(OutputParams::default()));
//! g.connect(PinRef::new(perlin, 0), PinRef::new(thresh, 0)).unwrap();
//! g.connect(PinRef::new(thresh, 0), PinRef::new(out, 0)).unwrap();
//! assert!(!g.has_errors());
//! ```

#![warn(missing_docs)]

mod boundary;
mod crossgraph;
mod diagnostic;
mod edge;
mod error;
mod graph;
mod library;
mod node;
mod pin;
mod prefab;

pub use boundary::{BoundaryPort, EffectivePin, GraphBoundary, ResolvedBoundary, ResolvedPin};
pub use crossgraph::{detect_graph_ref_cycle, GraphRefTarget};
pub use diagnostic::{Diagnostic, Severity};
pub use edge::{Edge, PinRef};
pub use error::{GraphError, GraphResult};
pub use graph::{Graph, GraphKind};
pub use library::{LibraryGraphId, LibraryGraphRegistry};
pub use node::{
    AddParams, Axis, BiomeParamParams, ClampParams, ConstantMaterialParams, ConstantParams,
    CurveMapperParams, DensityOutputParams, DensitySubtractParams, DomainWarpParams, FractalType,
    GraphOutputParams, GraphRefParams, IntersectParams, LayerParams, LerpParams, LibraryRefParams,
    MaskParams, MaxParams, MinParams, MixParams, MultiplyParams, Node, NodeCategory, NodeDescriptor,
    NodeId, NodeKind, NoiseParams, OutputParams, Perlin2DParams, PinSpec, QueueParams, RemapParams,
    SubtractParams, TerrainOutputParams, ThresholdParams, UnionParams, WorldAxisParams,
    WorldPosParams, BuildTerrainParams, JitteredGridParams, PoissonDiskParams, FindFlatParams,
    PlaceTreeParams, PlacePrefabParams, WorldOutputParams, ZoneOutputParams, YBandParams,
    PoissonDistributionParams, SurfaceFilterParams, BiomeContextMaskParams,
    SpeciesPickerParams, PaintDensityParams, ScatterPlaceParams,
};
pub use pin::PinType;
pub use prefab::{PrefabTemplate, PrefabVoxel};
