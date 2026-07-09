//! Cross-graph references: a `GraphRef` node points at another graph in the
//! hierarchy (the World graph, the Zone graph, or a Biome graph) and exposes
//! that graph's declared boundary outputs as its own pins.
//!
//! This module defines the reference *target* and the cycle check over
//! graph-to-graph references, mirroring the library-reference cycle check on
//! [`LibraryGraphRegistry`](crate::LibraryGraphRegistry). Resolving a target's
//! boundary into pins is wired where the hierarchy is assembled (a later
//! substep); nothing here performs that resolution.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::graph::Graph;

/// Which graph in the five-graph hierarchy a
/// [`GraphRef`](crate::NodeKind::GraphRef) points at. `Biome` carries the biome
/// id; `World` and `Zone` are singletons.
#[derive(
    Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default, Serialize, Deserialize,
)]
pub enum GraphRefTarget {
    /// The singleton World graph.
    #[default]
    World,
    /// The Zone graph.
    Zone,
    /// A biome graph, by biome id.
    Biome(u16),
}

/// Detect a cycle among graph-to-graph (`GraphRef`) references across a
/// hierarchy. `graphs` pairs each present target with its graph; a graph's
/// outgoing edges are the targets its `GraphRef` nodes name. Returns the targets
/// on a cycle (deterministic: targets and each graph's references are visited in
/// sorted order), or `None` if acyclic. A reference to a target absent from
/// `graphs` has no outgoing edges and is simply skipped.
pub fn detect_graph_ref_cycle(
    graphs: &[(GraphRefTarget, &Graph)],
) -> Option<Vec<GraphRefTarget>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Gray,
        Black,
    }

    // Sorted node set + sorted, deduplicated outgoing edges => deterministic.
    let mut targets: Vec<GraphRefTarget> = graphs.iter().map(|(t, _)| *t).collect();
    targets.sort();
    targets.dedup();

    let children: HashMap<GraphRefTarget, Vec<GraphRefTarget>> = graphs
        .iter()
        .map(|(t, g)| {
            let mut refs: Vec<GraphRefTarget> = g.graph_refs().collect();
            refs.sort();
            refs.dedup();
            (*t, refs)
        })
        .collect();

    let mut state: HashMap<GraphRefTarget, Mark> =
        targets.iter().map(|&t| (t, Mark::White)).collect();
    let mut stack: Vec<GraphRefTarget> = Vec::new();

    for &root in &targets {
        if state[&root] != Mark::White {
            continue;
        }
        let mut work: Vec<(GraphRefTarget, usize)> = vec![(root, 0)];
        state.insert(root, Mark::Gray);
        stack.push(root);
        while let Some(&(node, idx)) = work.last() {
            let kids = children.get(&node).map(Vec::as_slice).unwrap_or(&[]);
            if idx < kids.len() {
                work.last_mut().unwrap().1 += 1;
                let child = kids[idx];
                match state.get(&child).copied() {
                    Some(Mark::Gray) => {
                        let start = stack.iter().position(|&n| n == child).unwrap();
                        return Some(stack[start..].to_vec());
                    }
                    Some(Mark::White) => {
                        state.insert(child, Mark::Gray);
                        stack.push(child);
                        work.push((child, 0));
                    }
                    // Black (finished) or unregistered: nothing to expand.
                    _ => {}
                }
            } else {
                state.insert(node, Mark::Black);
                stack.pop();
                work.pop();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, GraphKind};
    use crate::node::{GraphRefParams, NodeKind};

    /// A graph whose `GraphRef` nodes point at each target in `targets`.
    fn graph_referencing(kind: GraphKind, targets: &[GraphRefTarget]) -> Graph {
        let mut g = Graph::of_kind(kind);
        for &t in targets {
            g.add_node(NodeKind::GraphRef(GraphRefParams { target: t, ..Default::default() }));
        }
        g
    }

    #[test]
    fn empty_hierarchy_is_acyclic() {
        assert_eq!(detect_graph_ref_cycle(&[]), None);
    }

    #[test]
    fn zone_referencing_world_is_acyclic() {
        let world = graph_referencing(GraphKind::World, &[]);
        let zone = graph_referencing(GraphKind::Zone, &[GraphRefTarget::World]);
        let graphs = [(GraphRefTarget::World, &world), (GraphRefTarget::Zone, &zone)];
        assert_eq!(detect_graph_ref_cycle(&graphs), None);
    }

    #[test]
    fn zone_biome_mutual_reference_is_a_cycle() {
        // Zone -> Biome(0) -> Zone.
        let zone = graph_referencing(GraphKind::Zone, &[GraphRefTarget::Biome(0)]);
        let biome = graph_referencing(GraphKind::Biome, &[GraphRefTarget::Zone]);
        let graphs = [(GraphRefTarget::Zone, &zone), (GraphRefTarget::Biome(0), &biome)];
        let cycle = detect_graph_ref_cycle(&graphs).expect("cycle");
        assert!(cycle.contains(&GraphRefTarget::Zone));
        assert!(cycle.contains(&GraphRefTarget::Biome(0)));
    }

    #[test]
    fn self_reference_is_a_cycle() {
        let zone = graph_referencing(GraphKind::Zone, &[GraphRefTarget::Zone]);
        assert_eq!(
            detect_graph_ref_cycle(&[(GraphRefTarget::Zone, &zone)]),
            Some(vec![GraphRefTarget::Zone])
        );
    }

    #[test]
    fn reference_to_absent_target_is_not_a_cycle() {
        let zone = graph_referencing(GraphKind::Zone, &[GraphRefTarget::Biome(5)]);
        assert_eq!(detect_graph_ref_cycle(&[(GraphRefTarget::Zone, &zone)]), None);
    }
}
