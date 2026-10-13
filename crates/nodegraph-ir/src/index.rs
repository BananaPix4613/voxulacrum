//! An input-pin index over a graph's edges: `(node, pin) -> source pin`.
//! 
//! Every evaluator resolves a node's inputs by asking "which output feeds this
//! input pin". Answered against [`Graph::edges`](crate::Graph::edges) directly
//! that is a linear scan, and the pointwise samplers ask once per input per
//! sample, which makes a pointwise probe quadratic in graph size.
//! 
//! [`Graph::connect`](crate::Graph::connect) rejects a second edge into an input
//! pin, so the relation is a function and this can be a plain lookup rather than
//! a multimap. Build one per evaluator, from a graph that does not change while
//! the evaluator lives.
//! 
//! Sources live in one contiguous `Vec` with a per-node window, not a `Vec` per
//! node. That is a measured choice: a `SecondaryMap<NodeId, Vec<_>>` was slower
//! than the linear scan it replaced at every graph size in the tree, because
//! each lookup chased a per-node allocation.

use slotmap::SecondaryMap;

use crate::edge::PinRef;
use crate::graph::Graph;
use crate::node::NodeId;

/// Sources by input pin, in node-contiguous storage.
#[derive(Clone, Debug, Default)]
pub struct EdgeIndex {
    /// Per node, its `(start, len)` window into `slots`. A node with no
    /// incoming edges is absent.
    windows: SecondaryMap<NodeId, (u32, u32)>,
    /// Sources by pin index within each node's window. A pin with no edge is
    /// `None`.
    slots: Vec<Option<PinRef>>,
}

impl EdgeIndex {
    /// Index `graph`'s edges by their destination pin.
    pub fn build(graph: &Graph) -> Self {
        // Pass one: how many pins lots each destination node needs.
        let mut windows: SecondaryMap<NodeId, (u32, u32)> = SecondaryMap::new();
        for edge in &graph.edges {
            let needed = edge.to.pin as u32 + 1;
            match windows.get_mut(edge.to.node) {
                Some((_, len)) => *len = (*len).max(needed),
                None => {
                    windows.insert(edge.to.node, (0, needed));
                }
            }
        }
        // Pass two: hand out windows, then fill them.
        let mut next = 0u32;
        for (_, (start, len)) in windows.iter_mut() {
            *start = next;
            next += *len;
        }
        let mut slots = vec![None; next as usize];
        for edge in &graph.edges {
            // Present: every destination was inserted in pass one.
            if let Some(&(start, _)) = windows.get(edge.to.node) {
                // Last edge wins, which `connect` cannot produce; a graph
                // deserialized with a duplicate destination is malformed and
                // `validate` reports it.
                slots[(start + edge.to.pin as u32) as usize] = Some(edge.from);
            }
        }
        Self { windows, slots }
    }
    
    /// The output pin feeding `(node, pin)`, or `None` if that input is
    /// unconnected.
    #[inline]
    pub fn source(&self, node: NodeId, pin: u16) -> Option<PinRef> {
        let &(start, len) = self.windows.get(node)?;
        let pin = pin as u32;
        if pin >= len {
            return None;
        }
        self.slots[(start + pin) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{AddParams, ConstantParams, LerpParams, NodeKind, OutputParams};
    
    /// `a -> add.0`, `b -> add.1`, `add -> lerp.0`, `b -> lerp.2`,
    /// `lerp -> out.0`. Lerp's middle pin is deliberately left unconnected, so
    /// a gap inside a window is covered.
    fn graph() -> (Graph, NodeId, NodeId, NodeId, NodeId) {
        let mut g = Graph::new();
        let a = g.add_node(NodeKind::Constant(ConstantParams { value: 1.0 }));
        let b = g.add_node(NodeKind::Constant(ConstantParams { value: 2.0 }));
        let add = g.add_node(NodeKind::Add(AddParams::default()));
        g.connect(PinRef::new(a, 0), PinRef::new(add, 0)).unwrap();
        g.connect(PinRef::new(b, 0), PinRef::new(add, 1)).unwrap();
        let lerp = g.add_node(NodeKind::Lerp(LerpParams::default()));
        g.connect(PinRef::new(add, 0), PinRef::new(lerp, 0)).unwrap();
        g.connect(PinRef::new(b, 0), PinRef::new(lerp, 2)).unwrap();
        let out = g.add_node(NodeKind::Output(OutputParams::default()));
        g.connect(PinRef::new(lerp, 0), PinRef::new(out, 0)).unwrap();
        (g, a, b, add, lerp)
    }
    
    #[test]
    fn resolves_each_input_pin_to_its_source() {
        let (g, a, b, add, _) = graph();
        let ix = EdgeIndex::build(&g);
        assert_eq!(ix.source(add, 0), Some(PinRef::new(a, 0)));
        assert_eq!(ix.source(add, 1), Some(PinRef::new(b, 0)));
    }
    
    #[test]
    fn agrees_with_a_linear_scan_over_every_pin() {
        // The property that matters: this must be the scan it replaces, for
        // every node and every pin, including pins past the declared ones.
        let (g, ..) = graph();
        let ix = EdgeIndex::build(&g);
        for (id, node) in g.nodes.iter() {
            for pin in 0..node.kind.effective_inputs().len() as u16 + 2 {
                let scanned = g
                    .edges
                    .iter()
                    .find(|e| e.to.node == id && e.to.pin == pin)
                    .map(|e| e.from);
                assert_eq!(ix.source(id, pin), scanned, "node {id:?} pin {pin}");
            }
        }
    }
    
    #[test]
    fn a_gap_inside_a_window_reads_as_unconnected() {
        let (g, _, _, _, lerp) = graph();
        let ix = EdgeIndex::build(&g);
        assert!(ix.source(lerp, 0).is_some());
        assert_eq!(ix.source(lerp, 1), None);
        assert!(ix.source(lerp, 2).is_some());
    }
    
    #[test]
    fn source_nodes_and_foreign_nodes_are_none() {
        let (g, a, ..) = graph();
        let ix = EdgeIndex::build(&g);
        assert_eq!(ix.source(a, 0), None);
        
        // A key from a different slotmap must not alias into the windows.
        let mut other = Graph::new();
        let stranger = other.add_node(NodeKind::Output(OutputParams::default()));
        assert_eq!(ix.source(stranger, 0), None);
    }
    
    #[test]
    fn an_empty_graph_indexes_to_nothing() {
        let ix = EdgeIndex::build(&Graph::new());
        let mut probe = Graph::new();
        let n = probe.add_node(NodeKind::Output(OutputParams::default()));
        assert_eq!(ix.source(n, 0), None);
    }
}
