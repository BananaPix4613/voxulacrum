//! The graph container, mutation API, validation, and JSON serialization.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use slotmap::SlotMap;

use crate::diagnostic::{Diagnostic, Severity};
use crate::edge::{Edge, PinRef};
use crate::error::{GraphError, GraphResult};
use crate::node::{Node, NodeId, NodeKind};
use crate::pin::PinType;

/// A typed dataflow graph: a slotmap of nodes plus typed edges.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Graph {
    /// Nodes keyed by stable [`NodeId`].
    pub nodes: SlotMap<NodeId, Node>,
    /// Directed edges (output pin -> input pin).
    pub edges: Vec<Edge>,
}

impl Graph {
    /// Empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a node at the origin; returns its stable id.
    pub fn add_node(&mut self, kind: NodeKind) -> NodeId {
        self.nodes.insert(Node::new(kind))
    }

    /// Insert a node at an editor position.
    pub fn add_node_at(&mut self, kind: NodeKind, position: glam::Vec2) -> NodeId {
        self.nodes.insert(Node { kind, position })
    }

    /// Remove a node and any edges touching it. Returns the removed node.
    pub fn remove_node(&mut self, id: NodeId) -> Option<Node> {
        self.edges.retain(|e| e.from.node != id && e.to.node != id);
        self.nodes.remove(id)
    }

    /// Connect an output pin to an input pin, validating eagerly:
    /// both nodes exist, both pin indices are in range, the types are
    /// compatible, and the input is not already connected.
    pub fn connect(&mut self, from: PinRef, to: PinRef) -> GraphResult<()> {
        let from_node = self.nodes.get(from.node).ok_or(GraphError::NodeNotFound(from.node))?;
        let to_node = self.nodes.get(to.node).ok_or(GraphError::NodeNotFound(to.node))?;

        let outputs = from_node.kind.descriptor().outputs;
        let inputs = to_node.kind.descriptor().inputs;

        let out_spec = outputs.get(from.pin as usize).ok_or(GraphError::PinOutOfRange {
            node: from.node,
            pin: from.pin,
            count: outputs.len(),
        })?;
        let in_spec = inputs.get(to.pin as usize).ok_or(GraphError::PinOutOfRange {
            node: to.node,
            pin: to.pin,
            count: inputs.len(),
        })?;

        if !PinType::is_compatible(out_spec.ty, in_spec.ty) {
            return Err(GraphError::TypeMismatch { from: out_spec.ty, to: in_spec.ty });
        }

        if self.edges.iter().any(|e| e.to == to) {
            return Err(GraphError::InputAlreadyConnected { node: to.node, pin: to.pin });
        }

        self.edges.push(Edge { from, to });
        Ok(())
    }

    /// Remove any edge feeding the given input pin. Returns true if removed.
    pub fn disconnect(&mut self, to: PinRef) -> bool {
        let before = self.edges.len();
        self.edges.retain(|e| e.to != to);
        self.edges.len() != before
    }

    /// Validate the whole graph. Returns all findings; an empty result (or
    /// one with no [`Severity::Error`]) means the graph is evaluable.
    ///
    /// Checks: edge integrity (nodes/pins exist), type compatibility,
    /// at-most-one connection per input pin, all required inputs connected,
    /// and acyclicity.
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut diags = Vec::new();

        // 1. Edge integrity + type checks.
        for (i, edge) in self.edges.iter().enumerate() {
            let Some(from_node) = self.nodes.get(edge.from.node) else {
                diags.push(Diagnostic::error("edge references missing source node").with_edge(i));
                continue;
            };
            let Some(to_node) = self.nodes.get(edge.to.node) else {
                diags.push(Diagnostic::error("edge references missing target target node").with_edge(i));
                continue;
            };
            let outputs = from_node.kind.descriptor().outputs;
            let inputs = to_node.kind.descriptor().inputs;
            let Some(out_spec) = outputs.get(edge.from.pin as usize) else {
                diags.push(
                    Diagnostic::error(format!("output pin {} out of range", edge.from.pin))
                        .with_edge(i),
                );
                continue;
            };
            let Some(in_spec) = inputs.get(edge.to.pin as usize) else {
                diags.push(
                    Diagnostic::error(format!("input pin {} out of range", edge.to.pin))
                        .with_edge(i),
                );
                continue;
            };
            if !PinType::is_compatible(out_spec.ty, in_spec.ty) {
                diags.push(
                    Diagnostic::error(format!(
                        "type mismatch: {:?} cannot feed {:?}",
                        out_spec.ty, in_spec.ty
                    ))
                    .with_edge(i),
                );
            }
        }

        // 2. At most one edge per input pin.
        let mut input_counts: HashMap<(NodeId, u16), usize> = HashMap::new();
        for edge in &self.edges {
            *input_counts.entry((edge.to.node, edge.to.pin)).or_default() += 1;
        }
        for ((node, pin), count) in input_counts {
            if count > 1 {
                diags.push(
                    Diagnostic::error(format!(
                        "input pin {pin} has {count} incoming edges (max 1)"
                    ))
                    .with_node(node),
                );
            }
        }

        // 3. All required inputs connected.
        for (id, node) in &self.nodes {
            for (pin_idx, spec) in node.kind.descriptor().inputs.iter().enumerate() {
                if spec.required {
                    let connected = self
                        .edges
                        .iter()
                        .any(|e| e.to.node == id && e.to.pin as usize == pin_idx);
                    if !connected {
                        diags.push(
                            Diagnostic::error(format!(
                                "required input '{}' is not connected",
                                spec.name
                            ))
                            .with_node(id),
                        );
                    }
                }
            }
        }

        // 4. Acyclicity.
        if let Some(cycle) = self.detect_cycle() {
            let mut d = Diagnostic::error(format!(
                "graph contains a cycle involving {} node(s)",
                cycle.len()
            ));
            if let Some(&first) = cycle.first() {
                d = d.with_node(first);
            }
            diags.push(d);
        }

        diags
    }

    /// True if validation produces any [`Severity::Error`].
    pub fn has_errors(&self) -> bool {
        self.validate().iter().any(|d| d.severity == Severity::Error)
    }

    /// Node IDs in dependency-first (topological) order. Returns `Err` holding
    /// the set of nodes in or downstream of a cycle when the graph is cyclic.
    ///
    /// Any valid topological order yields identical evaluation output, since
    /// nodes are pure functions of their inputs.
    pub fn topological_order(&self) -> Result<Vec<NodeId>, Vec<NodeId>> {
        let mut in_degree: HashMap<NodeId, usize> = self.nodes.keys().map(|k| (k, 0)).collect();
        for edge in &self.edges {
            if self.nodes.contains_key(edge.from.node) && self.nodes.contains_key(edge.to.node) {
                *in_degree.entry(edge.to.node).or_default() += 1;
            }
        }
        let mut queue: Vec<NodeId> =
            in_degree.iter().filter(|(_, &d)| d == 0).map(|(&id, _)| id).collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(n) = queue.pop() {
            order.push(n);
            for edge in &self.edges {
                if edge.from.node == n && self.nodes.contains_key(edge.to.node) {
                    if let Some(d) = in_degree.get_mut(&edge.to.node) {
                        *d -= 1;
                        if *d == 0 {
                            queue.push(edge.to.node);
                        }
                    }
                }
            }
        }
        if order.len() == self.nodes.len() {
            Ok(order)
        } else {
            Err(in_degree.into_iter().filter(|(_, d)| *d > 0).map(|(id, _)| id).collect())
        }
    }

    /// Returns the nodes in a cycle, or `None` if acyclic.
    fn detect_cycle(&self) -> Option<Vec<NodeId>> {
        self.topological_order().err()
    }

    /// Serialize to compact JSON.
    pub fn to_json(&self) -> GraphResult<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Serialize to pretty JSON.
    pub fn to_json_pretty(&self) -> GraphResult<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Deserialize from JSON.
    pub fn from_json(s: &str) -> GraphResult<Self> {
        Ok(serde_json::from_str(s)?)
    }
}
