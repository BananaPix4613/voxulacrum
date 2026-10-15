//! Blueprint resolution at the hot-reload boundary, keeping the evaluator
//! filesystem-pure. `PlaceBlueprint` nodes carry a name; this layer loads
//! `<name>.blueprint.json`, resolves its material names through the registry,
//! and fills the node's in-memory template.

use std::path::Path;

use nodegraph_ir::{Graph, NodeKind};
use voxel_core::{Blueprint, MaterialRegistry, ResolvedBlueprint};

use crate::error::{HotReloadError, HotReloadResult};
use crate::names::local_name;

/// Load and resolve the blueprint `name` names, for content that loads under
/// `namespace`: `<dir>/<file>.blueprint.json`, with its materials resolved in
/// that namespace.
pub fn load_blueprint(
    dir: &Path,
    name: &str,
    registry: &MaterialRegistry,
    namespace: &str,
) -> HotReloadResult<ResolvedBlueprint> {
    let file = local_name(name, namespace, "blueprint")?;
    Blueprint::load(dir, &file)
        .and_then(|bp| bp.resolve(registry, namespace))
        .map_err(HotReloadError::Blueprint)
}

/// Fill every `PlaceBlueprint` and `PlaceStructure` node's `resolved` template
/// by loading its named blueprint from `dir`. Nodes already resolved are skipped.
/// Errors on the first missing, malformed, or unresolvable blueprint.
pub fn resolve_blueprints(
    graph: &mut Graph,
    dir: &Path,
    registry: &MaterialRegistry,
    namespace: &str,
) -> HotReloadResult<()> {
    for (_, node) in graph.nodes.iter_mut() {
        match &mut node.kind {
            NodeKind::PlaceBlueprint(p) => {
                if p.resolved.is_none() {
                    p.resolved = Some(load_blueprint(dir, &p.blueprint, registry, namespace)?);
                }
            }
            NodeKind::PlaceStructure(p) => {
                if p.resolved.is_none() {
                    p.resolved = Some(load_blueprint(dir, &p.blueprint, registry, namespace)?);
                }
            }
            _ => {}
        }
    }
    Ok(())
}
