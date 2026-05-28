//! Validation diagnostics.

use crate::node::NodeId;

/// Severity of a [`Diagnostic`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Severity {
    /// Graph is invalid; cannot be evaluated.
    Error,
    /// Suspicious but evaluable.
    Warning,
    /// Informational.
    Info,
}

/// A single validation finding, optionally attributed to a node and/or edge.
#[derive(Clone, PartialEq, Debug)]
pub struct Diagnostic {
    /// Severity.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
    /// Node this finding concerns, if any.
    pub node: Option<NodeId>,
    /// Index into `Graph::edges` this finding concerns, if any.
    pub edge: Option<usize>,
}

impl Diagnostic {
    /// New error diagnostic.
    pub fn error(message: impl Into<String>) -> Self {
        Self { severity: Severity::Error, message: message.into(), node: None, edge: None }
    }

    /// New warning diagnostic.
    pub fn warning(message: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, message: message.into(), node: None, edge: None }
    }

    /// New info diagnostic.
    pub fn info(message: impl Into<String>) -> Self {
        Self { severity: Severity::Info, message: message.into(), node: None, edge: None }
    }

    /// Attribute this diagnostic to a node.
    pub fn with_node(mut self, node: NodeId) -> Self {
        self.node = Some(node);
        self
    }

    /// Attribute this diagnostic to an edge index.
    pub fn with_edge(mut self, edge: usize) -> Self {
        self.edge = Some(edge);
        self
    }
}
