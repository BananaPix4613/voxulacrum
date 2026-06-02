//! Prefab resolution at the hot-reload boundary, keeping the evaluator
//! filesystem-pure. `PlacePrefab` nodes carry a name; this layer loads
//! `<name>.prefab.json` and fills the node's in-memory template.

use std::path::Path;

use nodegraph_ir::{Graph, NodeKind, PrefabTemplate};

use crate::error::HotReloadResult;

/// Load `<dir>/<name>.prefab.json` into a [`PrefabTemplate`].
pub fn load_prefab(dir: &Path, name: &str) -> HotReloadResult<PrefabTemplate> {
    let path = dir.join(format!("{name}.prefab.json"));
    let text = std::fs::read_to_string(&path)?;
    let template = serde_json::from_str(&text)?;
    Ok(template)
}

/// Fill every `PlacePrefab` node's `template` by loading its named prefab from
/// `dir`. Nodes already resolved are skipped. Errors on the first missing or
/// malformed prefab file.
pub fn resolve_prefabs(graph: &mut Graph, dir: &Path) -> HotReloadResult<()> {
    for (_, node) in graph.nodes.iter_mut() {
        if let NodeKind::PlacePrefab(p) = &mut node.kind {
            if p.template.is_none() {
                p.template = Some(load_prefab(dir, &p.prefab)?);
            }
        }
    }
    Ok(())
}
