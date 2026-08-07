//! Blueprint resolution at the hot-reload boundary, keeping the evaluator
//! filesystem-pure. `PlaceBlueprint` nodes carry a name; this layer loads
//! `<name>.blueprint.json`, resolves its material names through the registry,
//! and fills the node's in-memory template.

use std::path::Path;

use nodegraph_ir::{Graph, NodeKind};
use voxel_core::{Blueprint, MaterialRegistry, ResolvedBlueprint};

use crate::error::{HotReloadError, HotReloadResult};

/// Load and resolve `<dir>/<name>.blueprint.json`.
pub fn load_blueprint(
    dir: &Path,
    name: &str,
    registry: &MaterialRegistry,
) -> HotReloadResult<ResolvedBlueprint> {
    Blueprint::load(dir, name)
        .and_then(|bp| bp.resolve(registry))
        .map_err(HotReloadError::Blueprint)
}

/// Fill every `PlaceBlueprint` and `PlaceStructure` node's `resolved` template
/// by loading its named blueprint from `dir`. Nodes already resolved are skipped.
/// Errors on the first missing, malformed, or unresolvable blueprint.
pub fn resolve_blueprints(
    graph: &mut Graph,
    dir: &Path,
    registry: &MaterialRegistry,
) -> HotReloadResult<()> {
    for (_, node) in graph.nodes.iter_mut() {
        match &mut node.kind {
            NodeKind::PlaceBlueprint(p) => {
                if p.resolved.is_none() {
                    p.resolved = Some(load_blueprint(dir, &p.blueprint, registry)?);
                }
            }
            NodeKind::PlaceStructure(p) => {
                if p.resolved.is_none() {
                    p.resolved = Some(load_blueprint(dir, &p.blueprint, registry)?);
                }
            }
            _ => {}
        }
    }
    Ok(())
}
