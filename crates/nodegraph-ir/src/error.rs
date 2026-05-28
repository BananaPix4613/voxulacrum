//! Error type for graph mutation and serialization.

use thiserror::Error;

use crate::node::NodeId;
use crate::pin::PinType;

/// Errors returned by mutating [`Graph`](crate::Graph) operations.
#[derive(Debug, Error)]
pub enum GraphError {
    /// A referenced node does not exist in the graph.
    #[error("node {0:?} not found in graph")]
    NodeNotFound(NodeId),

    /// A pin index is out of range for the node's descriptor.
    #[error("pin index {pin} out of range for node {node:?} ({count} pins available)")]
    PinOutOfRange {
        /// Node whose pin was indexed.
        node: NodeId,
        /// The out-of-range pin index.
        pin: u16,
        /// Number of pins actually available.
        count: usize,
    },

    /// Output and input pin types are not compatible.
    #[error("type mismatch: output {from:?} is not compatible with input {to:?}")]
    TypeMismatch {
        /// Output pin type.
        from: PinType,
        /// Input pin type.
        to: PinType,
    },

    /// The target input pin already has an incoming edge.
    #[error("input pin {pin} on node {node:?} is already connected")]
    InputAlreadyConnected {
        /// Target node.
        node: NodeId,
        /// Target input pin index.
        pin: u16,
    },

    /// JSON (de)serialization failure.
    #[error("graph (de)serialization failed: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Result alias for graph operations.
pub type GraphResult<T> = Result<T, GraphError>;
