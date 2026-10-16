//! Material resolution at the hot-reload boundary: every material a graph names
//! is looked up in the registry and its id cached on the reference, so the
//! evaluator works in ids and never sees a name.

use nodegraph_ir::Graph;
use voxel_core::MaterialRegistry;

use crate::error::{HotReloadError, HotReloadResult};

/// Resolve every material reference in `graph` against `registry`. Names
/// resolve in `namespace`, the namespace of the content holding the graph.
///
/// Ids are always rewritten, never skipped, so a reference renamed since it was
/// last resolved cannot keep its old id. Errors on the first name that is
/// malformed or not in the registry.
pub fn resolve_materials(
    graph: &mut Graph,
    registry: &MaterialRegistry,
    namespace: &str,
) -> HotReloadResult<()> {
    for (_, node) in graph.nodes.iter_mut() {
        for reference in node.kind.material_refs_mut() {
            let id = registry
                .resolve_str(reference.name(), namespace)
                .map_err(HotReloadError::Material)?;
            reference.set_id(id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodegraph_ir::{LayerParams, MaterialRef, NodeId, NodeKind};
    use voxel_core::MaterialId;

    const RON: &str = r#"(materials: [
        (id: 0, id_name: "air", display_name: "Air", color: (0.0, 0.0, 0.0), sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
        (id: 1, id_name: "stone", display_name: "Stone", color: (0.5, 0.5, 0.5), sharpness: 0.5, hardness: 0.5, permeable: false, supports_flora: false),
        (id: 2, id_name: "soil", display_name: "Soil", color: (0.4, 0.2, 0.1), sharpness: 0.5, hardness: 0.5, permeable: false, supports_flora: true),
    ])"#;

    fn registry() -> MaterialRegistry {
        MaterialRegistry::from_ron_str(RON, "voxulacrum").expect("fixture is valid")
    }

    fn layer_graph(top: &str) -> (Graph, NodeId) {
        let mut g = Graph::new();
        let n = g.add_node(NodeKind::Layer(LayerParams {
            bands: vec![(MaterialRef::new(top), 1)],
            fill: MaterialRef::new("stone"),
        }));
        (g, n)
    }

    fn layer(g: &mut Graph, n: NodeId) -> &mut LayerParams {
        match &mut g.nodes[n].kind {
            NodeKind::Layer(p) => p,
            _ => unreachable!("the fixture node is a Layer"),
        }
    }

    #[test]
    fn names_resolve_to_registry_ids() {
        let (mut g, n) = layer_graph("soil");
        resolve_materials(&mut g, &registry(), "voxulacrum").unwrap();
        let p = layer(&mut g, n);
        assert_eq!(p.bands[0].0.id(), Some(MaterialId(2)));
        assert_eq!(p.fill.id(), Some(MaterialId(1)));
    }

    #[test]
    fn an_unknown_name_is_an_error() {
        let (mut g, _) = layer_graph("obsidian");
        let err = resolve_materials(&mut g, &registry(), "voxulacrum").unwrap_err().to_string();
        assert!(err.contains("voxulacrum:obsidian"), "{err}");
    }

    #[test]
    fn a_renamed_reference_is_re_resolved() {
        let (mut g, n) = layer_graph("soil");
        resolve_materials(&mut g, &registry(), "voxulacrum").unwrap();
        // Rename without clearing, as a stale in-memory edit could.
        layer(&mut g, n).bands[0].0 = MaterialRef::with_id("stone", MaterialId(2));
        resolve_materials(&mut g, &registry(), "voxulacrum").unwrap();
        assert_eq!(layer(&mut g, n).bands[0].0.id(), Some(MaterialId(1)), "not the stale 2");
    }
}
