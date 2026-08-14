//! Species resolution at the hot-reload boundary, keeping the evaluator
//! filesystem-pure. `PlaceTree` nodes carry a species name and a wood material
//! name; this layer loads `<name>.species.json`, resolves the material through
//! the registry, and fills the node's in-memory template.
//!
//! Mirrors `blueprint.rs` deliberately: both are "a node names authored content
//! on disk, and something at the edge turns the name into data."

use std::path::Path;

use nodegraph_ir::{Graph, NodeKind, ResolvedTree, TreeSpecies};
use voxel_core::MaterialRegistry;

use crate::error::{HotReloadError, HotReloadResult};

/// Schema version this build writes and understands.
pub const SPECIES_VERSION: u32 = 1;

/// On-disk schema for `<name>.species.json`.
///
/// A wrapper around [`TreeSpecies`] rather than a version field on the params
/// themselves: the params are also a live in-memory struct edited by the debug
/// panel, and a version number is a property of a file, not of a species.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SpeciesFile {
    /// Schema version. **Required** - a file without one is not a species of
    /// this format, and later format work must not have to guess.
    pub version: u32,
    /// Stable name; must match the filename.
    pub name: String,
    /// The shape parameters.
    pub species: TreeSpecies,
}

impl SpeciesFile {
    /// Write `<dir>/<name>.species.json`, pretty-printed.
    ///
    /// Validated because the name becomes a filename, on the same rule
    /// `Blueprint::save` uses - a text field that turns into a path needs a
    /// guard, and `../../evil` is not a species name.
    pub fn save(&self, dir: &Path) -> HotReloadResult<()> {
        if self.name.is_empty()
            || !self.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(HotReloadError::Blueprint(format!(
                "species name '{}' must be non-empty and use only letters, digits, '_' and '-' - it becomes a filename",
                self.name
            )));
        }
        std::fs::create_dir_all(dir)
            .map_err(|e| HotReloadError::Blueprint(format!("{}: {e}", dir.display())))?;
        let path = dir.join(format!("{}.species.json", self.name));
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| HotReloadError::Blueprint(e.to_string()))?;
        std::fs::write(&path, text)
            .map_err(|e| HotReloadError::Blueprint(format!("{}: {e}", path.display())))
    }
}

/// Load `<dir>/<name>.species.json`.
pub fn load_species(dir: &Path, name: &str) -> HotReloadResult<TreeSpecies> {
    let path = dir.join(format!("{name}.species.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| HotReloadError::Blueprint(format!("{}: {e}", path.display())))?;
    let file: SpeciesFile = serde_json::from_str(&text)
        .map_err(|e| HotReloadError::Blueprint(format!("{}: {e}", path.display())))?;
    if file.version > SPECIES_VERSION {
        return Err(HotReloadError::Blueprint(format!(
            "species '{name}' is version {} but this build understands up to {SPECIES_VERSION}",
            file.version
        )));
    }
    if file.name != name {
        return Err(HotReloadError::Blueprint(format!(
            "species file {} declares the name {:?}",
            path.display(),
            file.name
        )));
    }
    Ok(file.species)
}

/// Fill every `PlaceTree` node's `resolved` template. Nodes already resolved are
/// skipped. Errors on the first missing, malformed, or unresolvable species -
/// a source that places nothing and says nothing is how a wrong world looks
/// like a working one.
pub fn resolve_species(
    graph: &mut Graph,
    dir: &Path,
    registry: &MaterialRegistry,
) -> HotReloadResult<()> {
    for (_, node) in graph.nodes.iter_mut() {
        let NodeKind::PlaceTree(p) = &mut node.kind else { continue };
        if p.resolved.is_some() {
            continue;
        }
        let species = load_species(dir, &p.species)?;
        let wood = registry.resolve(&p.wood).ok_or_else(|| {
            HotReloadError::Blueprint(format!(
                "species '{}' names an unknown wood material {:?}",
                p.species, p.wood
            ))
        })?;
        p.resolved = Some(ResolvedTree { species, wood });
    }
    Ok(())
}
