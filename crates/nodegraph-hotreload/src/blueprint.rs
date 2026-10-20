//! Blueprint resolution at the hot-reload boundary, keeping the evaluator
//! filesystem-pure. `PlaceBlueprint` nodes carry a name; this layer loads
//! `<name>.blueprint.json`, resolves its material names through the registry,
//! and fills the node's in-memory template.

use nodegraph_ir::{Graph, NodeKind};
use voxel_core::{Blueprint, MaterialRegistry, ResolvedBlueprint};

use crate::error::{HotReloadError, HotReloadResult};
use crate::names::ContentDirs;

/// Load and resolve the blueprint `name` names, read in `namespace`: its
/// `<file>.blueprint.json` comes from the directory `dirs` gives the name's
/// namespace, and its own bare material names resolve in that namespace.
pub fn load_blueprint(
    dirs: &ContentDirs,
    name: &str,
    registry: &MaterialRegistry,
    namespace: &str,
) -> HotReloadResult<ResolvedBlueprint> {
    let (name, dir) = dirs.locate(name, namespace, "blueprint")?;
    Blueprint::load(dir, name.name())
        .and_then(|bp| bp.resolve(registry, name.namespace()))
        .map_err(HotReloadError::Blueprint)
}

/// Fill every `PlaceBlueprint` and `PlaceStructure` node's `resolved` template
/// by loading its named blueprint from the namespace it names. Nodes already
/// resolved are skipped. Errors on the first missing, malformed, or
/// unresolvable blueprints.
pub fn resolve_blueprints(
    graph: &mut Graph,
    dirs: &ContentDirs,
    registry: &MaterialRegistry,
    namespace: &str,
) -> HotReloadResult<()> {
    for (_, node) in graph.nodes.iter_mut() {
        match &mut node.kind {
            NodeKind::PlaceBlueprint(p) => {
                if p.resolved.is_none() {
                    p.resolved = Some(load_blueprint(dirs, &p.blueprint, registry, namespace)?);
                }
            }
            NodeKind::PlaceStructure(p) => {
                if p.resolved.is_none() {
                    p.resolved = Some(load_blueprint(dirs, &p.blueprint, registry, namespace)?);
                }
            }
            _ => {}
        }
    }
    Ok(())
}
