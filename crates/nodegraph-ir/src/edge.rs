//! Edges and pin references.

use serde::{Deserialize, Serialize};

use crate::node::NodeId;

/// A reference to a specific pin on a specific node. Pin index is the
/// position in the node's `inputs` or `outputs` descriptor slice.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PinRef {
    /// The node this pin belongs to.
    pub node: NodeId,
    /// Index into the node's pin list (inputs or outputs, by edge role).
    pub pin: u16,
}

impl PinRef {
    /// Construct a pin reference.
    pub fn new(node: NodeId, pin: u16) -> Self {
        Self { node, pin }
    }
}

/// A directed, typed connection from an output pin (`from`) to an input
/// pin (`to`).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Edge {
    /// Output side (source).
    pub from: PinRef,
    /// Input side (destination).
    pub to: PinRef,
}
