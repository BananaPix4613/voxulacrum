//! The graph container, mutation API, validation, and JSON serialization.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use slotmap::SlotMap;

use crate::boundary::{BoundaryPort, GraphBoundary};
use crate::crossgraph::GraphRefTarget;
use crate::diagnostic::{Diagnostic, Severity};
use crate::edge::{Edge, PinRef};
use crate::error::{GraphError, GraphResult};
use crate::library::{LibraryGraphId, LibraryGraphRegistry};
use crate::node::{Node, NodeId, NodeKind};
use crate::pin::PinType;

/// The role a [`Graph`] plays in the five-graph hierarchy (design doc §4).
///
/// The variant does not change a graph's structure - every kind is the same
/// slotmap-of-nodes-plus-edges - only the rules it is validated against and the
/// root output(s) it is expected to produce. Those per-kind rules are filled in
/// alongside the functional graph kinds in later substeps; see
/// [`Graph::validate`].
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum GraphKind {
    /// Singleton top-level graph: climate and zone assignment for the world.
    World,
    /// A region graph: terrain framing and biome assignment within one zone.
    Zone,
    /// A terrain graph: density, materials, and per-biome parameters. This is
    /// the shape of the legacy flat terrain graph, so it is the [`Default`].
    #[default]
    Biome,
    /// A fine-detail graph (texel / decal scaffolding). Type-only this phase:
    /// a detail graph may be authored but produces nothing yet.
    Detail,
    /// A reusable sub-graph referenced by other graphs (instanced / shared).
    Library,
}

/// A typed dataflow graph: a slotmap of nodes plus typed edges, tagged with the
/// [`GraphKind`] it plays in the hierarchy.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Graph {
    /// The role this graph plays in the five-graph hierarchy. Serialized graphs
    /// that predate this field deserialize as [`GraphKind::Biome`], so the
    /// legacy flat terrain graph loads unchanged.
    #[serde(default)]
    pub kind: GraphKind,
    /// The named typed inputs/outputs this graph exposes to consumers (a
    /// `LibraryRef` or `GraphRef`). Empty by default, so graphs that predate
    /// this field - and graphs that expose no boundary - round-trip unchanged.
    #[serde(default)]
    pub boundary: GraphBoundary,
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

    /// Empty graph of an explicit [`GraphKind`].
    pub fn of_kind(kind: GraphKind) -> Self {
        Self { kind, ..Self::default() }
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
        // Resolve the effective pin types up front; the borrowed pin lists are
        // scoped so they release the node borrows before `self.edges` is mutated.
        let (out_ty, in_ty) = {
            let from_node = self.nodes.get(from.node).ok_or(GraphError::NodeNotFound(from.node))?;
            let to_node = self.nodes.get(to.node).ok_or(GraphError::NodeNotFound(to.node))?;
            let outputs = from_node.kind.effective_outputs();
            let inputs = to_node.kind.effective_inputs();
            let out_ty = outputs
                .get(from.pin as usize)
                .ok_or(GraphError::PinOutOfRange { node: from.node, pin: from.pin, count: outputs.len() })?
                .ty;
            let in_ty = inputs
                .get(to.pin as usize)
                .ok_or(GraphError::PinOutOfRange { node: to.node, pin: to.pin, count: inputs.len() })?
                .ty;
            (out_ty, in_ty)
        };

        if !PinType::is_compatible(out_ty, in_ty) {
            return Err(GraphError::TypeMismatch { from: out_ty, to: in_ty });
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
    
    /// The library ids referenced by `LibraryRef` nodes in this graph.
    /// Iteration order follows the slotmap and is unspecified; callers that
    /// need determinism should sort.
    pub fn library_refs(&self) -> impl Iterator<Item = LibraryGraphId> + '_ {
        self.nodes.values().filter_map(|n| match &n.kind {
            NodeKind::LibraryRef(p) => Some(p.library),
            _ => None,
        })
    }

    /// Resolve every `LibraryRef` node's pins from `libraries`, caching the
    /// projected boundary on each node. A reference to an unregistered library
    /// resolves to no pins. Idempotent; re-run on load and after edits so the
    /// cached pins track the referenced library's current boundary.
    pub fn resolve_library_refs(&mut self, libraries: &LibraryGraphRegistry) {
        for node in self.nodes.values_mut() {
            if let NodeKind::LibraryRef(params) = &mut node.kind {
                params.resolved =
                    libraries.get(params.library).map(|lib| lib.boundary.to_resolved());
            }
        }
    }

    /// The hierarchy targets referenced by `GraphRef` nodes in this graph.
    /// Iteration order follows the slotmap and is unspecified; callers that need
    /// determinism should sort (see
    /// [`detect_graph_ref_cycle`](crate::detect_graph_ref_cycle)).
    pub fn graph_refs(&self) -> impl Iterator<Item = GraphRefTarget> + '_ {
        self.nodes.values().filter_map(|n| match &n.kind {
            NodeKind::GraphRef(p) => Some(p.target),
            _ => None,
        })
    }

    /// Resolve every `GraphRef` node's output pins from the referenced graph's
    /// boundary outputs, via `boundary_of`. A `GraphRef` exposes only the
    /// target's outputs (it reads, never feeds), so its inputs stay empty. A
    /// target with no boundary resolves to no pins. Idempotent; re-run on load
    /// or after edits.
    pub fn resolve_graph_refs<F>(&mut self, boundary_of: F)
    where
        F: Fn(GraphRefTarget) -> Option<GraphBoundary>,
    {
        for node in self.nodes.values_mut() {
            if let NodeKind::GraphRef(params) = &mut node.kind {
                params.resolved = boundary_of(params.target).map(|b| b.to_resolved_outputs());
            }
        }
    }

    /// Populate `boundary.outputs` from this graph's `GraphOutput` nodes - each
    /// marks its input value as a named boundary output (currently a
    /// [`SurfaceField`](crate::PinType::SurfaceField) climate channel). Outputs
    /// are ordered by name for determinism. Idempotent; call after load or edit.
    /// Declared inputs are left untouched (graphs declare none; libraries author
    /// both sides).
    pub fn derive_output_boundary(&mut self) {
        let mut outputs: Vec<BoundaryPort> = self
            .nodes
            .values()
            .filter_map(|n| match &n.kind {
                NodeKind::GraphOutput(p) => Some(BoundaryPort {
                    name: p.name.clone(),
                    ty: PinType::SurfaceField,
                    description: String::new(),
                }),
                _ => None,
            })
            .collect();
        outputs.sort_by(|a, b| a.name.cmp(&b.name));
        self.boundary.outputs = outputs;
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
            let outputs = from_node.kind.effective_outputs();
            let inputs = to_node.kind.effective_inputs();
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
            for (pin_idx, spec) in node.kind.effective_inputs().iter().enumerate() {
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

        // 5. Per-kind structural rules (root outputs, etc.).
        self.validate_kind_rules(&mut diags);

        // 6. Boundary declaration integrity: port names unique within a side.
        self.validate_boundary(&mut diags);

        diags
    }

    /// Append diagnostics for rules specific to this graph's [`GraphKind`],
    /// beyond the generic checks above (edge integrity, single-input,
    /// required-inputs, acyclicity).
    ///
    /// `DetailGraph`: a non-empty detail graph must terminate in at least one
    /// `PaintDensity` or `ScatterPlace` writer (an empty detail graph is valid -
    /// it produces no foliage). World/Zone/Biome root-output rules are still
    /// scaffolded as generic dataflow; Library graphs are intentionally rule-free.
    fn validate_kind_rules(&self, diags: &mut Vec<Diagnostic>) {
        match self.kind {
            GraphKind::Detail => {
                let has_terminal = self.nodes.values().any(|n| {
                    matches!(n.kind, NodeKind::PaintDensity(_) | NodeKind::ScatterPlace(_))
                });
                if !self.nodes.is_empty() && !has_terminal {
                    diags.push(Diagnostic::error(
                        "DetailGraph has no PaintDensity or ScatterPlace terminal output",
                    ));
                }
            }
            GraphKind::World | GraphKind::Zone | GraphKind::Biome | GraphKind::Library => {}
        }
    }

    /// Append diagnostics for boundary declaration integrity: port names must be
    /// unique within `inputs` and within `outputs` (a name may legitimately
    /// appear once on each side). Consumers resolve ports by name, so a
    /// duplicate would be ambiguous.
    fn validate_boundary(&self, diags: &mut Vec<Diagnostic>) {
        for (side, ports) in [
            ("input", &self.boundary.inputs),
            ("output", &self.boundary.outputs),
        ] {
            for (i, port) in ports.iter().enumerate() {
                if ports[..i].iter().any(|p| p.name == port.name) {
                    diags.push(Diagnostic::error(format!(
                        "duplicate boundary {side} port name '{}'",
                        port.name
                    )));
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_kind_is_biome() {
        assert_eq!(Graph::new().kind, GraphKind::Biome);
        assert_eq!(GraphKind::default(), GraphKind::Biome);
    }

    #[test]
    fn of_kind_sets_kind() {
        assert_eq!(Graph::of_kind(GraphKind::World).kind, GraphKind::World);
        assert_eq!(Graph::of_kind(GraphKind::Library).kind, GraphKind::Library);
    }

    #[test]
    fn kind_round_trips_through_json() {
        let g = Graph::of_kind(GraphKind::Zone);
        let json = g.to_json().unwrap();
        let back = Graph::from_json(&json).unwrap();
        assert_eq!(back.kind, GraphKind::Zone);
    }

    #[test]
    fn legacy_json_without_kind_defaults_to_biome() {
        // Drop the `kind` field to mimic a document authored before it existed.
        let mut v: serde_json::Value =
            serde_json::from_str(&Graph::new().to_json().unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("kind");
        let g: Graph = serde_json::from_value(v).unwrap();
        assert_eq!(g.kind, GraphKind::Biome);
    }

    #[test]
    fn boundary_defaults_empty_and_round_trips() {
        use crate::boundary::{BoundaryPort, GraphBoundary};
        use crate::pin::PinType;
        let mut g = Graph::of_kind(GraphKind::Library);
        assert!(g.boundary.is_empty());
        g.boundary = GraphBoundary {
            inputs: vec![BoundaryPort::new("distance", PinType::SurfaceField)],
            outputs: vec![
                BoundaryPort::new("weight", PinType::SurfaceField)
                    .with_description("own-biome fade weight"),
            ],
        };
        let back = Graph::from_json(&g.to_json().unwrap()).unwrap();
        assert_eq!(back.boundary, g.boundary);
        assert_eq!(back.boundary.output("weight").unwrap().ty, PinType::SurfaceField);
    }

    #[test]
    fn legacy_json_without_boundary_defaults_empty() {
        // Drop the `boundary` field to mimic a document authored before it existed.
        let mut v: serde_json::Value =
            serde_json::from_str(&Graph::new().to_json().unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("boundary");
        let g: Graph = serde_json::from_value(v).unwrap();
        assert!(g.boundary.is_empty());
    }

    #[test]
    fn duplicate_boundary_port_name_is_an_error() {
        use crate::boundary::{BoundaryPort, GraphBoundary};
        use crate::pin::PinType;
        let mut g = Graph::of_kind(GraphKind::Library);
        g.boundary = GraphBoundary {
            inputs: Vec::new(),
            outputs: vec![
                BoundaryPort::new("x", PinType::Density),
                BoundaryPort::new("x", PinType::Scalar),
            ],
        };
        assert!(g.has_errors());
    }

    #[test]
    fn empty_detail_graph_is_valid() {
        let g = Graph::of_kind(GraphKind::Detail);
        assert!(!g.has_errors());
    }

    #[test]
    fn detail_graph_with_terminal_is_valid() {
        use crate::node::PaintDensityParams;
        let mut g = Graph::of_kind(GraphKind::Detail);
        g.add_node(NodeKind::PaintDensity(PaintDensityParams::default()));
        assert!(!g.has_errors());
    }

    #[test]
    fn detail_graph_without_terminal_is_invalid() {
        use crate::node::PoissonDistributionParams;
        let mut g = Graph::of_kind(GraphKind::Detail);
        g.add_node(NodeKind::PoissonDistribution(PoissonDistributionParams::default()));
        assert!(g.has_errors());
    }

    #[test]
    fn library_ref_resolves_pins_from_boundary() {
        use crate::boundary::{BoundaryPort, GraphBoundary};
        use crate::library::{LibraryGraphId, LibraryGraphRegistry};
        use crate::node::LibraryRefParams;
        use crate::pin::PinType;
        
        let mut lib = Graph::of_kind(GraphKind::Library);
        lib.boundary = GraphBoundary {
            inputs: vec![BoundaryPort::new("distance", PinType::SurfaceField)],
            outputs: vec![BoundaryPort::new("weight", PinType::SurfaceField)],
        };
        let mut registry = LibraryGraphRegistry::new();
        registry.insert(LibraryGraphId(3), lib);
        
        let mut g = Graph::new();
        let n = g.add_node(NodeKind::LibraryRef(LibraryRefParams {
            library: LibraryGraphId(3),
            ..Default::default()
        }));
        // Before resolution: no pins.
        assert!(g.nodes[n].kind.effective_inputs().is_empty());
        assert!(g.nodes[n].kind.effective_outputs().is_empty());
        
        // After resolution: pins mirror the library boundary.
        g.resolve_library_refs(&registry);
        let inputs = g.nodes[n].kind.effective_inputs();
        let outputs = g.nodes[n].kind.effective_outputs();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "distance");
        assert_eq!(inputs[0].ty, PinType::SurfaceField);
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "weight");
    }
    
    #[test]
    fn library_ref_to_unregistered_library_resolves_to_no_pins() {
        use crate::library::{LibraryGraphId, LibraryGraphRegistry};
        use crate::node::LibraryRefParams;
        let mut g = Graph::new();
        let n = g.add_node(NodeKind::LibraryRef(LibraryRefParams {
            library: LibraryGraphId(99),
            ..Default::default()
        }));
        g.resolve_library_refs(&LibraryGraphRegistry::new());
        assert!(g.nodes[n].kind.effective_inputs().is_empty());
        assert!(g.nodes[n].kind.effective_outputs().is_empty());
    }
    
    #[test]
    fn connect_uses_resolved_library_pins() {
        use crate::boundary::{BoundaryPort, GraphBoundary};
        use crate::library::{LibraryGraphId, LibraryGraphRegistry};
        use crate::node::{LibraryRefParams, ZoneOutputParams};
        use crate::pin::PinType;
        
        // A library exposing one SurfaceField output "weight".
        let mut lib = Graph::of_kind(GraphKind::Library);
        lib.boundary = GraphBoundary {
            inputs: Vec::new(),
            outputs: vec![BoundaryPort::new("weight", PinType::SurfaceField)],
        };
        let mut registry = LibraryGraphRegistry::new();
        registry.insert(LibraryGraphId(1), lib);
        
        let mut g = Graph::of_kind(GraphKind::Zone);
        let lref = g.add_node(NodeKind::LibraryRef(LibraryRefParams {
            library: LibraryGraphId(1),
            ..Default::default()
        }));
        let out = g.add_node(NodeKind::ZoneOutput(ZoneOutputParams::default()));
        
        // Before resolution the LibraryRef has no output pin 0 -> connect fails.
        assert!(g.connect(PinRef::new(lref, 0), PinRef::new(out, 0)).is_err());
        
        // After resolution its SurfaceField output feeds ZoneOutput's SurfaceField input.
        g.resolve_library_refs(&registry);
        assert!(g.connect(PinRef::new(lref, 0), PinRef::new(out, 0)).is_ok());
    }

    #[test]
    fn derive_output_boundary_from_graph_output_nodes() {
        use crate::node::GraphOutputParams;
        use crate::pin::PinType;
        let mut g = Graph::of_kind(GraphKind::World);
        g.add_node(NodeKind::GraphOutput(GraphOutputParams { name: "climate".into() }));
        g.add_node(NodeKind::GraphOutput(GraphOutputParams { name: "aridity".into() }));
        g.derive_output_boundary();
        assert_eq!(g.boundary.outputs.len(), 2);
        assert_eq!(g.boundary.outputs[0].name, "aridity"); // sorted by name
        assert_eq!(g.boundary.outputs[1].name, "climate");
        assert!(g.boundary.outputs.iter().all(|p| p.ty == PinType::SurfaceField));
    }
}
