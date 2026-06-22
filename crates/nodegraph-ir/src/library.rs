//! Library graphs: reusable sub-graphs referenced by `LibraryRef` nodes, and
//! a registry that resolves those references and rejects cycles.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::graph::Graph;

/// Stable identifier for a library graph. A [`NodeKind::LibraryRef`] node
/// carries one of these; the [`LibraryGraphRegistry`] maps it back to the
/// referenced [`Graph`].
///
/// [`NodeKind::LibraryRef`]: crate::NodeKind::LibraryRef
#[derive(
    Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default, Serialize, Deserialize,
)]
pub struct LibraryGraphId(pub u32);

/// A set of library graphs keyed by [`LibraryGraphId`], with cycle detection
/// over their `LibraryRef` references.
#[derive(Clone, Debug, Default)]
pub struct LibraryGraphRegistry {
    graphs: HashMap<LibraryGraphId, Graph>,
}

impl LibraryGraphRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Insert or replace the library graph under `id`. Returns the previous
    /// graph at that id, if any.
    pub fn insert(&mut self, id: LibraryGraphId, graph: Graph) -> Option<Graph> {
        self.graphs.insert(id, graph)
    }
    
    /// The graph registered under `id`, if any.
    pub fn get(&self, id: LibraryGraphId) -> Option<&Graph> {
        self.graphs.get(&id)
    }
    
    /// True if `id` is registered.
    pub fn contains(&self, id: LibraryGraphId) -> bool {
        self.graphs.contains_key(&id)
    }
    
    /// Number of registered library graphs.
    pub fn len(&self) -> usize {
        self.graphs.len()
    }
    
    /// True if no library graphs are registered.
    pub fn is_empty(&self) -> bool {
        self.graphs.is_empty()
    }
    
    /// Detect a cycle in the library-reference graph (library A references
    /// library B via a `LibraryRef` node, and so on). Returns the ids on a
    /// cycle, or `None` if the references are acyclic.
    /// 
    /// Ids are visited in sorted order and each node's references are sorted,
    /// so the reported cycle is deterministic. A reference to an unregistered
    /// id has no outgoing edges here and is simply skipped - resolving missing
    /// references is a separate concern.
    pub fn detect_cycle(&self) -> Option<Vec<LibraryGraphId>> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            White,
            Gray,
            Black,
        }
        
        let mut ids: Vec<LibraryGraphId> = self.graphs.keys().copied().collect();
        ids.sort();
        
        // Precompute each library's sorted, deduplicated outgoing references.
        let children: HashMap<LibraryGraphId, Vec<LibraryGraphId>> = ids
            .iter()
            .map(|&id| {
                let mut refs: Vec<LibraryGraphId> = self.graphs[&id].library_refs().collect();
                refs.sort();
                refs.dedup();
                (id, refs)
            })
            .collect();
        
        let mut state: HashMap<LibraryGraphId, Mark> =
            ids.iter().map(|&id| (id, Mark::White)).collect();
        let mut stack: Vec<LibraryGraphId> = Vec::new();
        
        // Iterative 3-color DFS so deep reference chains can't overflow the
        // call stack. `work` holds (node, next-child-index) frames.
        for &root in &ids {
            if state[&root] != Mark::White {
                continue;
            }
            let mut work: Vec<(LibraryGraphId, usize)> = vec![(root, 0)];
            state.insert(root, Mark::Gray);
            stack.push(root);
            while let Some(&(node, idx)) = work.last() {
                let kids = &children[&node];
                if idx < kids.len() {
                    work.last_mut().unwrap().1 += 1;
                    let child = kids[idx];
                    match state.get(&child).copied() {
                        Some(Mark::Gray) => {
                            // Back-edge: slice the cycle out of the gray stack.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Graph;
    use crate::node::{LibraryRefParams, NodeKind};
    
    /// A library graph whose only nodes are `LibraryRef`s to `targets`.
    fn lib_referencing(targets: &[u32]) -> Graph {
        let mut g = Graph::new();
        for &t in targets {
            g.add_node(NodeKind::LibraryRef(LibraryRefParams {
                library: LibraryGraphId(t),
            }));
        }
        g
    }
    
    #[test]
    fn empty_registry_is_acyclic() {
        assert_eq!(LibraryGraphRegistry::new().detect_cycle(), None);
    }
    
    #[test]
    fn acyclic_chain_has_no_cycle() {
        let mut reg = LibraryGraphRegistry::new();
        reg.insert(LibraryGraphId(0), lib_referencing(&[1]));
        reg.insert(LibraryGraphId(1), lib_referencing(&[2]));
        reg.insert(LibraryGraphId(2), lib_referencing(&[]));
        assert_eq!(reg.detect_cycle(), None);
    }
    
    #[test]
    fn self_reference_is_a_cycle() {
        let mut reg = LibraryGraphRegistry::new();
        reg.insert(LibraryGraphId(7), lib_referencing(&[7]));
        assert_eq!(reg.detect_cycle(), Some(vec![LibraryGraphId(7)]));
    }
    
    #[test]
    fn two_node_cycle_is_detected() {
        let mut reg = LibraryGraphRegistry::new();
        reg.insert(LibraryGraphId(0), lib_referencing(&[1]));
        reg.insert(LibraryGraphId(1), lib_referencing(&[0]));
        // Sorted traversal starts at 0, so the cycle reports as [0, 1].
        assert_eq!(
            reg.detect_cycle(),
            Some(vec![LibraryGraphId(0), LibraryGraphId(1)])
        );
    }
    
    #[test]
    fn reference_to_unregistered_id_is_not_a_cycle() {
        let mut reg = LibraryGraphRegistry::new();
        reg.insert(LibraryGraphId(0), lib_referencing(&[99]));
        assert_eq!(reg.detect_cycle(), None);
    }
}