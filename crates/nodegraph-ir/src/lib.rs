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
//! It does not evaluate graphs (that is the evaluator crate) and does not
//! depend on `voxel-core`. [`PinType::Terrain`] is a bare tag here; no
//! chunk buffer is referenced. The IR is purely structure + validation +
//! serialization.
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

mod diagnostic;
mod edge;
mod error;
mod graph;
mod node;
mod pin;

pub use diagnostic::{Diagnostic, Severity};
pub use edge::{Edge, PinRef};
pub use error::{GraphError, GraphResult};
pub use graph::Graph;
pub use node::{
    AddParams, ClampParams, ConstantMaterialParams, ConstantParams, CurveMapperParams,
    DensitySubtractParams, DomainWarpParams, FractalType, IntersectParams, LayerParams,
    LerpParams, MaskParams, MaxParams, MinParams, MixParams, MultiplyParams, Node,
    NodeCategory, NodeDescriptor, NodeId, NodeKind, NoiseParams, OutputParams,
    Perlin2DParams, PinSpec, QueueParams, RemapParams, SubtractParams,
    TerrainOutputParams, ThresholdParams, UnionParams, WorldPosParams,
};
pub use pin::PinType;