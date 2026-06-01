//! Conversion between `egui_snarl::Snarl<NodeKind>` and `nodegraph_ir::Graph`.

use std::collections::HashMap;

use egui_snarl::{InPinId, NodeId as SnarlNodeId, OutPinId, Snarl};
use glam::Vec2;
use nodegraph_ir::{Graph, NodeId as IrNodeId, NodeKind, PinRef};

/// Build a fresh `Graph` from the editor's `Snarl<NodeKind>`. Also returns
/// the `snarl-id → ir-id` map so the viewer can look up per-node
/// diagnostics by IR id while it renders.
///
/// Edges that fail eager validation (type mismatch, missing pin) are
/// skipped silently; `Graph::validate()` reports them as diagnostics.
pub fn snarl_to_graph(
    snarl: &Snarl<NodeKind>,
) -> (Graph, HashMap<SnarlNodeId, IrNodeId>) {
    let mut g = Graph::new();
    let mut id_map: HashMap<SnarlNodeId, IrNodeId> = HashMap::new();

    for (snarl_id, pos, value) in snarl.nodes_pos_ids() {
        let ir_id = g.add_node_at(value.clone(), Vec2::new(pos.x, pos.y));
        id_map.insert(snarl_id, ir_id);
    }

    for (out, inp) in snarl.wires() {
        let (Some(&from_ir), Some(&to_ir)) = (id_map.get(&out.node), id_map.get(&inp.node)) else {
            continue;
        };
        let _ = g.connect(
            PinRef::new(from_ir, out.output as u16),
            PinRef::new(to_ir, inp.input as u16),
        );
    }

    (g, id_map)
}

/// Populate a fresh `Snarl<NodeKind>` from a `Graph`. Node positions from
/// `Node::position` are placed onto the canvas.
pub fn graph_to_snarl(graph: &Graph) -> Snarl<NodeKind> {
    let mut snarl: Snarl<NodeKind> = Snarl::new();
    let mut id_map: HashMap<IrNodeId, SnarlNodeId> = HashMap::new();

    for (ir_id, node) in &graph.nodes {
        let pos = egui::pos2(node.position.x, node.position.y);
        let snarl_id = snarl.insert_node(pos, node.kind.clone());
        id_map.insert(ir_id, snarl_id);
    }
    for edge in &graph.edges {
        let (Some(&from_s), Some(&to_s)) = (id_map.get(&edge.from.node), id_map.get(&edge.to.node)) else {
            continue;
        };
        snarl.connect(
            OutPinId { node: from_s, output: edge.from.pin as usize },
            InPinId { node: to_s, input: edge.to.pin as usize },
        );
    }
    snarl
}
