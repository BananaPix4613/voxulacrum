//! Graph-backed world generation.
//!
//! `WorldGenerator` is the single source of voxel terrain: it holds an
//! `Arc<Graph>` and produces a `ChunkStorage` per chunk by running the
//! nodegraph evaluator and harvesting the `TerrainOutput` node.
//!
//! The evaluator yields a `ChunkBuffer<Voxel, 32>` (the evaluation-domain
//! container); the engine stores `ChunkStorage` (the storage-domain container).
//! Crossing between them is the job of [`StorageBoundary`] (see
//! `storage_boundary.rs`) — a deliberate, permanent architectural seam, not a
//! temporary copy. The two containers are kept distinct on purpose so each can
//! be tuned for its side of the boundary.

use std::sync::Arc;

use glam::IVec3;
use nodegraph_eval::{ChunkEvaluation, EvalContext, WorldEvaluator};
use nodegraph_ir::{Graph, NodeKind};
use serde::Deserialize;
use smallvec::SmallVec;

use crate::params::TerrainGenParams;
use super::storage::ChunkStorage;
use super::storage_boundary::StorageBoundary;
use super::tags::{BiomeId, ChunkTags, ZoneId};

/// The single world generator. Evaluates one fixed graph per chunk.
pub struct WorldGenerator {
    /// The multi-graph harness (World/Zone/Biome + libraries). This phase the
    /// World/Zone graphs are empty and the Biome graph produces the terrain.
    world_eval: WorldEvaluator,
    world_seed: u64,
    /// The evaluation -> storage domain boundary used to materialize each
    /// evaluated chunk buffer into engine storage form.
    storage_boundary: StorageBoundary,
    /// Slab-smoothing distance applied in [`WorldGenerator::generate_chunk`].
    /// `0` disables smoothing; `>= 1` enables single-step smoothing.
    traversal_smoothing_distance: u32,
}

impl WorldGenerator {
    /// Build a generator from an owned graph. Fails if the graph has no
    /// `TerrainOutput` terminal (a config error worth catching at startup
    /// rather than per-chunk).
    pub fn new(
        graph: Graph,
        world_seed: u64,
        traversal_smoothing_distance: u32,
    ) -> Result<Self, String> {
        // Validate the biome graph has a terrain terminal (a startup config
        // error worth catching here). The harness re-locates it internally and
        // additionally logs any validation findings on the full graph set.
        if !graph
            .nodes
            .iter()
            .any(|(_, n)| matches!(n.kind, NodeKind::TerrainOutput(_)))
        {
            return Err("graph has no TerrainOutput node".to_string());
        }

        let world_eval = WorldEvaluator::new(graph);
        
        Ok(Self {
            world_eval,
            world_seed,
            storage_boundary: StorageBoundary::new(),
            traversal_smoothing_distance,
        })
    }
    
    /// Load a generator from a `*.graph.json` file on disk.
    pub fn from_path(
        path: &std::path::Path,
        world_seed: u64,
        traversal_smoothing_distance: u32,
    ) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let graph = Graph::from_json(&text).map_err(|e| e.to_string())?;
        Self::new(graph, world_seed, traversal_smoothing_distance)
    }

    /// Build a generator from a world manifest: a World graph, a Zone graph, and
    /// one or more biome graphs assembled into the multi-graph harness.
    pub fn from_manifest(
        manifest_path: &std::path::Path,
        world_seed: u64,
        traversal_smoothing_distance: u32,
    ) -> Result<Self, String> {
        let hierarchy = load_hierarchy(manifest_path)?;
        Ok(Self::from_hierarchy(hierarchy, world_seed, traversal_smoothing_distance))
    }

    /// Like [`WorldGenerator::from_manifest`], but substitutes `graph` for the
    /// hierarchy graph named by `slot`. Used when the editor edits one graph: the
    /// rest of the hierarchy is reloaded from the manifest and preserved. A biome
    /// id with no manifest entry is appended.
    pub fn from_manifest_with_override(
        manifest_path: &std::path::Path,
        world_seed: u64,
        traversal_smoothing_distance: u32,
        slot: GraphSlot,
        graph: Graph,
    ) -> Result<Self, String> {
        let mut hierarchy = load_hierarchy(manifest_path)?;
        match slot {
            GraphSlot::World => hierarchy.world = graph,
            GraphSlot::Zone => hierarchy.zone = graph,
            GraphSlot::Biome(id) => {
                match hierarchy.biomes.iter_mut().find(|(bid, _)| *bid == id) {
                    Some(entry) => entry.1 = graph,
                    None => hierarchy.biomes.push((id, graph)),
                }
            }
        }
        Ok(Self::from_hierarchy(hierarchy, world_seed, traversal_smoothing_distance))
    }

    /// Assemble a generator from an already-loaded hierarchy.
    fn from_hierarchy(
        hierarchy: LoadedHierarchy,
        world_seed: u64,
        traversal_smoothing_distance: u32,
    ) -> Self {
        // The primary biome anchors `WorldEvaluator::new`; `with_biomes` then
        // installs the full set, replacing that placeholder entry.
        let primary = hierarchy.biomes[0].1.clone();
        let world_eval = WorldEvaluator::new(primary)
            .with_world(hierarchy.world)
            .with_zone(hierarchy.zone)
            .with_biomes(hierarchy.biomes);
        Self {
            world_eval,
            world_seed,
            storage_boundary: StorageBoundary::new(),
            traversal_smoothing_distance,
        }
    }
    
    /// The Biome graph this generator evaluates (the terminal terrain producer).
    pub fn graph(&self) -> &Graph {
        &self.world_eval.biome_graph()
    }
    
    /// Generate the voxel storage and identity tags for one chunk by evaluating
    /// the graph set. On any evaluation failure this logs and returns an air
    /// chunk with default tags, so callers (streaming workers, initial fill)
    /// stay infallible.
    pub fn generate_chunk(&self, position: IVec3) -> GeneratedChunk {
        let eval = match self
            .world_eval
            .evaluate_chunk(EvalContext::new(self.world_seed, position))
        {
            Ok(eval) => eval,
            Err(e) => {
                log::warn!("graph eval failed for chunk {:?}: {}", position, e);
                return GeneratedChunk { storage: ChunkStorage::new_air(), tags: ChunkTags::default() };
            }
        };
        // The harness composited the biome terrain for this chunk.
        let terrain = &eval.terrain;
        
        // Cross the evaluation -> storage boundary: the evaluator's ChunkBuffer is
        // materialized into the engine's ChunkStorage form. This is a permanent,
        // deliberate seam between two intentionally-distinct containers.
        let mut storage = self.storage_boundary.materialize(terrain);
        
        // Worldgen stage 7: halve single-cube walkable steps into slab transitions.
        super::slab_smoothing::smooth_slabs(&mut storage, self.traversal_smoothing_distance);
        
        let tags = self.derive_tags(&eval);
        GeneratedChunk { storage, tags }
    }

    /// Aggregate a chunk evaluation's per-column zone/biome assignments into
    /// [`ChunkTags`]. `biomes` is the distinct set of biome ids present; `zone`
    /// is a single representative (the smallest distinct zone id - the world is
    /// single-zone, so multi-zone-per-chunk is not yet distinguished). Empty
    /// World/Zone graphs yield the legacy `Zone(0)` + `[Biome(0)]` tags.
    fn derive_tags(&self, eval: &ChunkEvaluation) -> ChunkTags {
        let zone_ids = self
            .world_eval
            .world_output_node()
            .and_then(|n| eval.world_columns.get(n))
            .and_then(|c| c.as_id())
            .map(|ids| distinct_sorted(ids.data()))
            .unwrap_or_default();
        let biome_ids = self
            .world_eval
            .zone_output_node()
            .and_then(|n| eval.zone_columns.get(n))
            .and_then(|c| c.as_id())
            .map(|ids| distinct_sorted(ids.data()))
            .unwrap_or_default();

        let zone = ZoneId(zone_ids.first().copied().unwrap_or(0));
        let mut biomes: SmallVec<[BiomeId; 4]> = biome_ids.into_iter().map(BiomeId).collect();
        if biomes.is_empty() {
            biomes.push(BiomeId(0)); // single-biome fallback (empty Zone graph)
        }
        ChunkTags { zone, biomes, library_refs: SmallVec::new() }
    }
}

/// Distinct ids present in a column, ascending. Deterministic.
fn distinct_sorted(ids: &[u16]) -> Vec<u16> {
    let mut v: Vec<u16> = ids.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

/// The product of generating one chunk: its voxel storage and identity tags.
pub struct GeneratedChunk {
    /// Materialized voxel storage.
    pub storage: ChunkStorage,
    /// Zone/biome identity tags derived from the chunk's column evaluation.
    pub tags: ChunkTags,
}

/// Identifies one graph in the world hierarchy, for routing edits and
/// invalidation to the right slot.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum GraphSlot {
    /// The singleton World graph.
    World,
    /// The Zone graph.
    Zone,
    /// A biome graph, by biome id.
    Biome(u16),
}

/// On-disk manifest describing a world's graph hierarchy. Paths are relative to
/// the manifest file's directory.
#[derive(Debug, Clone, Deserialize)]
pub struct WorldManifest {
    /// World graph file (climate + zone assignment).
    pub world: String,
    /// Zone graph file (biome assignment).
    pub zone: String,
    /// Biome graphs, keyed by the biome id each renders.
    pub biomes: Vec<BiomeManifestEntry>,
}

/// One biome entry in a [`WorldManifest`].
#[derive(Debug, Clone, Deserialize)]
pub struct BiomeManifestEntry {
    /// Biome id this graph renders (matches the Zone graph's assignment).
    pub id: u16,
    /// Biome graph file, relative to the manifest directory.
    pub graph: String,
}

/// The graphs named by a [`WorldManifest`], loaded into memory.
struct LoadedHierarchy {
    world: Graph,
    zone: Graph,
    biomes: Vec<(u16, Graph)>,
}

/// Read and parse one `*.graph.json` file.
fn read_graph(path: &std::path::Path) -> Result<Graph, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Graph::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Load a manifest and every graph it references.
fn load_hierarchy(manifest_path: &std::path::Path) -> Result<LoadedHierarchy, String> {
    let text = std::fs::read_to_string(manifest_path).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest: WorldManifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let dir = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let world = read_graph(&dir.join(&manifest.world))?;
    let zone = read_graph(&dir.join(&manifest.zone))?;
    let mut biomes = Vec::with_capacity(manifest.biomes.len());
    for entry in &manifest.biomes {
        biomes.push((entry.id, read_graph(&dir.join(&entry.graph))?));
    }
    if biomes.is_empty() {
        return Err("world manifest lists no biomes".to_string());
    }
    Ok(LoadedHierarchy { world, zone, biomes })
}

/// Load every editable graph in a world manifest, each with its hierarchy
/// [`GraphSlot`] and a display label (World, Zone, then biomes by id). Used by
/// the editor's graph selector.
pub fn load_world_graphs(
    manifest_path: &std::path::Path,
) -> Result<Vec<(GraphSlot, String, Graph)>, String> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest: WorldManifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let dir = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut out = vec![
        (GraphSlot::World, "World".to_string(), read_graph(&dir.join(&manifest.world))?),
        (GraphSlot::Zone, "Zone".to_string(), read_graph(&dir.join(&manifest.zone))?),
    ];
    for entry in &manifest.biomes {
        out.push((
            GraphSlot::Biome(entry.id),
            biome_label(&entry.graph, entry.id),
            read_graph(&dir.join(&entry.graph))?,
        ));
    }
    Ok(out)
}

/// A display label for a biome from its file name (e.g.
/// "biome_meadow.graph.json" -> "Meadow"), falling back to the id.
fn biome_label(file: &str, id: u16) -> String {
    let stem = file.strip_suffix(".graph.json").unwrap_or(file);
    let name = stem.strip_prefix("biome_").unwrap_or(stem);
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => format!("Biome {id}"),
    }
}

/// Path of the primary (editable) biome graph - biome 0 of the starter world.
/// The hot-reload watcher and embedded editor track this file.
pub fn default_graph_path() -> std::path::PathBuf {
    graphs_dir().join("biome_meadow.graph.json")
}

/// Path of the world manifest assembling the starter world's graph hierarchy
/// (World + Zone + biome graphs).
pub fn world_manifest_path() -> std::path::PathBuf {
    graphs_dir().join("world.manifest.json")
}

/// The directory holding the world's graph assets.
fn graphs_dir() -> std::path::PathBuf {
    crate::paths::asset_root().join("assets").join("graphs")
}

/// Load the default biome graph into a shared generator, seeded from `params`.
///
/// This is the one construction site for the engine's generator; `World`,
/// the streaming workers, and background regeneration all share the resulting
/// `Arc`. Re-reading the file here (rather than caching one immutable graph)
/// means a regeneration picks up an edited `biome_meadow.graph.json`.
pub fn load_default(params: &TerrainGenParams) -> Result<Arc<WorldGenerator>, String> {
    WorldGenerator::from_manifest(
        &world_manifest_path(),
        params.seed as u64,
        params.traversal_smoothing_distance,
    )
    .map(Arc::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_biome_graph_loads_with_terrain_output() {
        // `new` rejects a biome graph without a TerrainOutput, so a successful
        // load proves the default graph has one.
        WorldGenerator::from_path(&default_graph_path(), 0, 1)
            .expect("biome_meadow.graph.json must load (and have a TerrainOutput)");
    }

    #[test]
    fn generated_chunk_tags_default_to_single_biome() {
        let generator = WorldGenerator::from_path(&default_graph_path(), 0, 1)
            .expect("biome_meadow.graph.json must load");
        let generated = generator.generate_chunk(IVec3::ZERO);
        // Empty World/Zone graphs ⇒ legacy single-biome tags.
        assert_eq!(generated.tags.zone, ZoneId(0));
        assert_eq!(generated.tags.biomes.as_slice(), &[BiomeId(0)]);
    }

    #[test]
    fn distinct_sorted_dedups_and_orders() {
        assert_eq!(distinct_sorted(&[2, 0, 2, 1, 0]), vec![0, 1, 2]);
        assert_eq!(distinct_sorted(&[]), Vec::<u16>::new());
    }

    #[test]
    fn world_manifest_parses() {
        let json = r#"{
            "world": "world.graph.json",
            "zone": "zone.graph.json",
            "biomes": [
                { "id": 0, "graph": "biome_meadow.graph.json" },
                { "id": 1, "graph": "biome_rocky.graph.json" }
            ]
        }"#;
        let m: WorldManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.world, "world.graph.json");
        assert_eq!(m.zone, "zone.graph.json");
        assert_eq!(m.biomes.len(), 2);
        assert_eq!(m.biomes[1].id, 1);
        assert_eq!(m.biomes[1].graph, "biome_rocky.graph.json");
    }

    #[test]
    fn default_world_manifest_loads() {
        let generator = WorldGenerator::from_manifest(&world_manifest_path(), 0, 1)
            .expect("world.manifest.json and its graphs must load");
        // The hierarchy composites a chunk without panicking.
        let _ = generator.generate_chunk(IVec3::ZERO);
    }
}
