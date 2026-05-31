//! Evaluation errors.

use nodegraph_ir::NodeId;
use thiserror::Error;

/// Errors raised during evaluation.
#[derive(Debug, Error)]
pub enum EvalError {
    /// The graph failed IR validation (carrying the error count).
    #[error("graph failed validation: {0} error(s)")]
    InvalidGraph(usize),

    /// The graph contains a cycle and cannot be ordered.
    #[error("graph contains a cycle; cannot evaluate")]
    Cyclic,

    /// A node's output was requested before it was computed (ordering bug).
    #[error("node {0:?} has no cached output (evaluation-order bug)")]
    MissingOutput(NodeId),

    /// A required input pin has no incoming edge.
    #[error("required input pin {pin} on node {node:?} is not connected")]
    MissingInput {
        /// Target node.
        node: NodeId,
        /// Input pin index.
        pin: u16,
    },

    /// An input resolved to the wrong field kind.
    #[error("node {node:?} expected a {expected} input but received {got}")]
    WrongInputType {
        /// Consuming node.
        node: NodeId,
        /// Expected field kind.
        expected: &'static str,
        /// Actual field kind.
        got: &'static str,
    },

    /// PNG encoding failed.
    #[error("image write failed: {0}")]
    Image(#[from] image::ImageError),
}

/// Evaluation result alias.
pub type EvalResult<T> = Result<T, EvalError>;
