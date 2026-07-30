//! Graph-backed world generation.
//!
//! `WorldGenerator` is the single source of voxel terrain: it holds an
//! `Arc<Graph>` and produces a `ChunkStorage` per chunk by running the
//! nodegraph evaluator and harvesting the `TerrainOutput` node.
//!
//! The evaluator and the engine use different containers on each side of every
//! generated layer - terrain, fluid, and foliage. Crossing between them is the
//! job of [`StorageBoundary`] (see `storage_boundary.rs`) - a deliberate,
//! permanent architectural seam, not a temporary copy. The containers are kept
//! distinct on purpose so each can be tuned for its side of the boundary.
//!
//! The foliage translators (`paint_to_detail_layers`, `scatter_to_store`) live
//! here, beside the generation stage that produces their inputs, but they are
//! reached only through `StorageBoundary::materialize_foliage` - the boundary
//! names the crossing, this module implements the translation.

use std::collections::HashMap;
use std::sync::Arc;

use glam::IVec3;
use nodegraph_eval::{BiomeParams, ChunkEvaluation, EvalContext, PaintLayer, ScatterBucket, WorldEvaluator};
use nodegraph_ir::{Graph, NodeKind};
use serde::{Serialize, Deserialize};
use smallvec::SmallVec;
use voxel_core::LocalPos;

use crate::params::TerrainGenParams;
use super::layers::{
    DetailLayer, DetailLayerId, DetailLayers, DetailTexel, FluidFillMode, FluidLayer,
    PrefabId, ScatterFlags, ScatterInstance, ScatterStore, ScatterTypeId, StableInstanceId,
};
use super::chunk::CHUNK_SIZE;
use super::storage::ChunkStorage;
use super::storage_boundary::StorageBoundary;
use super::tags::{BiomeId, ChunkTags, ZoneId};

/// Biome parameter name for the per-biome slab-smoothing distance.
const TRAVERSAL_SMOOTHING_DISTANCE: &str = "traversal_smoothing_distance";
/// Smoothing distance for biomes that do not declare `traversal_smoothing_distance`.
const DEFAULT_TRAVERSAL_SMOOTHING_DISTANCE: u32 = 1;

/// The single world generator. Evaluates one fixed graph per chunk.
pub struct WorldGenerator {
    /// The multi-graph harness (World/Zone/Biome + libraries). This phase the
    /// World/Zone graphs are empty and the Biome graph produces the terrain.
    world_eval: WorldEvaluator,
    world_seed: u64,
    /// The evaluation -> storage domain boundary every generated layer crosses.
    /// Owns the world's sea level, which the fluid crossing needs.
    storage_boundary: StorageBoundary,
}

impl WorldGenerator {
    /// Build a generator from an owned graph. Fails if the graph has no
    /// `DensityOutput` terminal (a config error worth catching at startup
    /// rather than per-chunk).
    #[allow(dead_code)] // single-graph ctor; app runs the manifest path, tests use this
    pub fn new(graph: Graph, world_seed: u64) -> Result<Self, String> {
        // Validate the biome graph has a density terminal (a startup config
        // error worth catching here). The harness re-locates it internally and
        // additionally logs any validation findings on the full graph set.
        if !graph
            .nodes
            .iter()
            .any(|(_, n)| matches!(n.kind, NodeKind::DensityOutput(_)))
        {
            return Err("graph has no DensityOutput node".to_string());
        }

        let world_eval = WorldEvaluator::new(graph);
        
        Ok(Self {
            world_eval,
            world_seed,
            // Single-graph ctor (tests / tooling): no manifest, so no sea level.
            storage_boundary: StorageBoundary::new(0),
        })
    }
    
    /// Load a generator from a `*.graph.json` file on disk.
    #[allow(dead_code)] // single-graph loader; app runs the manifest path, tests use this
    pub fn from_path(path: &std::path::Path, world_seed: u64) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let graph = Graph::from_json(&text).map_err(|e| e.to_string())?;
        Self::new(graph, world_seed)
    }

    /// Build a generator from a world manifest: a World graph, a Zone graph, and
    /// one or more biome graphs assembled into the multi-graph harness.
    pub fn from_manifest(
        manifest_path: &std::path::Path,
        world_seed: u64,
    ) -> Result<Self, String> {
        let hierarchy = load_hierarchy(manifest_path)?;
        Ok(Self::from_hierarchy(hierarchy, world_seed))
    }

    /// Assemble a generator from an already-loaded hierarchy.
    fn from_hierarchy(hierarchy: LoadedHierarchy, world_seed: u64) -> Self {
        // The primary biome anchors `WorldEvaluator::new`; `with_biomes` then
        // installs the full set, replacing that placeholder entry.
        let primary = hierarchy.biomes[0].1.clone();
        let world_eval = WorldEvaluator::new(primary)
            .with_world(hierarchy.world)
            .with_zone(hierarchy.zone)
            .with_biomes(hierarchy.biomes)
            .with_biome_details(hierarchy.details)
            .with_biome_params(hierarchy.biome_params)
            .with_cross_graph_resolved();
        Self {
            world_eval,
            world_seed,
            storage_boundary: StorageBoundary::new(hierarchy.sea_level),
        }
    }
    
    /// The Biome graph this generator evaluates (the terminal terrain producer).
    #[allow(dead_code)] // graph accessor; used by tests / tooling
    pub fn graph(&self) -> &Graph {
        &self.world_eval.biome_graph()
    }
    
    /// The multi-graph evaluator itself, for callers that need per-biome
    /// parameters outside chunk generation (e.g. the climate simulation's
    /// biome-driven condensation targets, Substep 3c).
    pub fn world_eval(&self) -> &WorldEvaluator {
        &self.world_eval
    }

    /// The global ocean surface (world-Y) this generator fills at/below (design
    /// doc §7). Exposed so generation-finalization passes outside this module
    /// (the seam pass) can apply the same ocean rule to voxels they mutate after
    /// initial generation.
    pub fn sea_level(&self) -> i32 {
        self.storage_boundary.sea_level()
    }

    /// Biome id at a world XZ position, resolved straight from generation via
    /// a cheap column eval - independent of whether that region's chunks are
    /// currently streamed in. `None` for single-biome worlds (no Zone graph).
    /// Used by the climate sim so the cloud humidity field no longer recedes
    /// when zoom shrinks the chunk-load radius.
    pub fn biome_id_at_world(&self, wx: f32, wz: f32) -> Option<u16> {
        let dim = CHUNK_SIZE as i32;
        let chunk_x = (wx / CHUNK_SIZE as f32).floor() as i32;
        let chunk_z = (wz / CHUNK_SIZE as f32).floor() as i32;
        let lx = (wx.floor() as i32).rem_euclid(dim) as usize;
        let lz = (wz.floor() as i32).rem_euclid(dim) as usize;
        let ctx = EvalContext::new(self.world_seed, IVec3::new(chunk_x, 0, chunk_z));
        let col = self.world_eval.biome_column(ctx).ok().flatten()?;
        Some(col.get(lx, lz))
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
                return GeneratedChunk {
                    storage: ChunkStorage::new_air(),
                    tags: ChunkTags::default(),
                    detail_layers: DetailLayers::default(),
                    scatter: ScatterStore::default(),
                    fluids: FluidLayer::default(),
                    smoothing_distances: Box::new([0u8; super::slab_smoothing::COLUMN_COUNT]),
                };
            }
        };
        // The harness composited the biome terrain for this chunk.
        let terrain = &eval.terrain;
        
        // Cross the evaluation -> storage boundary: the evaluator's ChunkBuffer is
        // materialized into the engine's ChunkStorage form. This is a permanent,
        // deliberate seam between two intentionally-distinct containers.
        let mut storage = self.storage_boundary.materialize(terrain);
        
        // Worldgen stage 7: halve single-cube surface steps into slab transitions.
        // Each column's smoothing distance comes from its biome's params (default
        // when unset), so biomes can smooth differently.
        let biome_col = self
            .world_eval
            .zone_output_node()
            .and_then(|n| eval.zone_columns.get(n))
            .and_then(|c| c.as_id())
            .cloned();
        let mut distances = [DEFAULT_TRAVERSAL_SMOOTHING_DISTANCE; super::slab_smoothing::COLUMN_COUNT];
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let biome = biome_col.as_ref().map_or(0, |c| c.get(x, z));
                if let Some(d) = self.world_eval.biome_param(biome, TRAVERSAL_SMOOTHING_DISTANCE) {
                    distances[x + z * CHUNK_SIZE] = d.max(0.0) as u32;
                }
            }
        }
        super::slab_smoothing::smooth_slabs(&mut storage, &distances);

        // Keep the per-column distances (u8 is ample) for the seam pass.
        let mut smoothing_distances =
            Box::new([0u8; super::slab_smoothing::COLUMN_COUNT]);
        for (i, &d) in distances.iter().enumerate() {
            smoothing_distances[i] = d.min(255) as u8;
        }
        
        let tags = self.derive_tags(&eval);

        // Worldgen stage 9 and 10 both cross the eval -> storage boundary
        // (drift-review 2.3). Fluid crosses first so the foliage crossing can be
        // handed the finished field and drop anything it submerges (design §5
        // stage 9 before stage 10; drift-review 1.4).
        let fluids = self.storage_boundary.materialize_fluid(
            &storage,
            position.y,
            eval.fluid_levels.as_deref(),
        );
        let (detail_layers, scatter) = self.storage_boundary.materialize_foliage(
            &eval.foliage.paint,
            &eval.foliage.scatter,
            &storage,
            &fluids,
        );

        GeneratedChunk { storage, tags, detail_layers, scatter, fluids, smoothing_distances }
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

/// Translate the evaluator's foliage paint (eval domain) into the chunk's
/// `DetailLayers` (storage domain). A painted column whose surface the fluid field
/// submerges is dropped (left default), so grass never paints underwater (design
/// §5 stage 9 before stage 10; drift-review 1.4).
pub(super) fn paint_to_detail_layers(
    paint: &[PaintLayer],
    storage: &ChunkStorage,
    fluids: &FluidLayer,
) -> DetailLayers {
    // A chunk with no water can't submerge anything - skip the per-column surface
    // scan entirely so above-sea biomes pay nothing.
    let dry = matches!(fluids.fill_mode, FluidFillMode::Empty) && fluids.cells.is_empty();

    let mut layers: SmallVec<[DetailLayer; 4]> = SmallVec::new();
    for pl in paint {
        let mut layer = DetailLayer::new(DetailLayerId(pl.layer_id));
        for (i, t) in pl.texels.iter().enumerate() {
            if !dry {
                let x = i % CHUNK_SIZE;
                let z = i / CHUNK_SIZE;
                if let Some(sy) = column_surface(storage, x, z) {
                    if super::fluid_gen::foliage_submerged(fluids, x, sy, z) {
                        continue; // submerged column: leave the texel default (no foliage)
                    }
                }
            }
            layer.map[i] = DetailTexel {
                species: t.species,
                density: t.density,
                tint: t.tint,
                flags: t.flags,
            };
        }
        layers.push(layer);
    }
    DetailLayers { layers }
}

/// Topmost solid voxel Y in a column of `storage` (the surface), or `None` if the
/// column is all air.
fn column_surface(storage: &ChunkStorage, x: usize, z: usize) -> Option<usize> {
    for y in (0..CHUNK_SIZE).rev() {
        let idx = x + y * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE;
        if storage.voxel(idx).is_solid() {
            return Some(y);
        }
    }
    None
}

/// Translate the evaluator's scatter buckets (eval domain) into the chunk's
/// `ScatterStore` (storage domain, the generated set), carrying the generated
/// `stable_id`. Instances whose anchor surface the fluid field submerges are
/// dropped, so props never scatter underwater (design §5 stage 9 before stage 10;
/// drift-review 1.4).
pub(super) fn scatter_to_store(scatter: &[ScatterBucket], fluids: &FluidLayer) -> ScatterStore {
    let dry = matches!(fluids.fill_mode, FluidFillMode::Empty) && fluids.cells.is_empty();

    let mut by_type: HashMap<ScatterTypeId, Vec<ScatterInstance>> = HashMap::new();
    for bucket in scatter {
        let instances: Vec<ScatterInstance> = bucket
            .instances
            .iter()
            .filter(|fi| {
                dry || !super::fluid_gen::foliage_submerged(
                    fluids,
                    fi.anchor[0] as usize,
                    fi.anchor[1] as usize,
                    fi.anchor[2] as usize,
                )
            })
            .map(|fi| ScatterInstance {
                anchor: LocalPos::new_unchecked(fi.anchor[0], fi.anchor[1] ,fi.anchor[2]),
                sub_offset: fi.sub_offset,
                rotation_y: fi.rotation_y,
                scale_variant: fi.scale_variant,
                prefab_id: PrefabId(fi.prefab_id),
                flags: ScatterFlags(fi.flags),
                stable_id: StableInstanceId(fi.stable_id),
            })
            .collect();
        if !instances.is_empty() {
            by_type
                .entry(ScatterTypeId(bucket.type_id))
                .or_default()
                .extend(instances);
        }
    }
    ScatterStore { by_type }
}

/// The product of generating one chunk: its voxel storage and identity tags, and
/// Tier-1 foliage paint.
pub struct GeneratedChunk {
    /// Materialized voxel storage.
    pub storage: ChunkStorage,
    /// Zone/biome identity tags derived from the chunk's column evaluation.
    pub tags: ChunkTags,
    /// Tier-1 foliage paint layers, translated from the evaluator's output.
    pub detail_layers: DetailLayers,
    /// Tier-2/3 scatter instances, translated from the evaluator's output.
    pub scatter: ScatterStore,
    /// Ocean/biome fluid layer for this chunk (design doc §7).
    pub fluids: FluidLayer,
    /// Per-column traversal smoothing distances used by the isolated pass, kept
    /// so the cross-chunk seam pass (`world::seam`) can finish boundaries without
    /// re-evaluating the graph.
    pub smoothing_distances: Box<[u8; super::slab_smoothing::COLUMN_COUNT]>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldManifest {
    /// World graph file (climate + zone assignment).
    pub world: String,
    /// Global ocean surface (world-Y); empty voxels at or below fill with water.
    /// Absent => 0 (effectively no ocean for a world sitting above Y 0).
    #[serde(default)]
    pub sea_level: i32,
    /// Zone graph file (biome assignment).
    pub zone: String,
    /// Biome graphs, keyed by the biome id each renders.
    pub biomes: Vec<BiomeManifestEntry>,
}

/// One biome entry in a [`WorldManifest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiomeManifestEntry {
    /// Biome id this graph renders (matches the Zone graph's assignment).
    pub id: u16,
    /// Biome graph file, relative to the manifest directory.
    pub graph: String,
    /// Optional `DetailGraph` (foliage) file, relative to the manifest directory.
    /// Absent => this biome produces no foliage.
    #[serde(default)]
    pub detail: Option<String>,
    /// Per-biome scalar parameters (the biome-param sidecar). Free-form; absent
    /// => no parameters (every `BiomeParam` falls back to its node default).
    #[serde(default)]
    pub params: HashMap<String, f32>,
}

/// The graphs named by a [`WorldManifest`], loaded into memory.
struct LoadedHierarchy {
    world: Graph,
    zone: Graph,
    biomes: Vec<(u16, Graph)>,
    /// Per-biome DetailGraphs (by biome id) for biomes that reference one.
    details: Vec<(u16, Graph)>,
    /// Per-biome scalar parameter sidecars (by biome id).
    biome_params: Vec<(u16, BiomeParams)>,
    /// Global ocean surface (world-Y), from the manifest.
    sea_level: i32,
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
    let mut details = Vec::new();
    let mut biome_params = Vec::new();
    for entry in &manifest.biomes {
        biomes.push((entry.id, read_graph(&dir.join(&entry.graph))?));
        if let Some(detail_file) = &entry.detail {
            details.push((entry.id, read_graph(&dir.join(detail_file))?));
        }
        if !entry.params.is_empty() {
            biome_params.push((entry.id, BiomeParams::from_entries(entry.params.clone())));
        }
    }
    if biomes.is_empty() {
        return Err("world manifest lists no biomes".to_string());
    }
    Ok(LoadedHierarchy { world, zone, biomes, details, biome_params, sea_level: manifest.sea_level })
}

/// Load every editable graph in a world manifest: its slot, display label,
/// on-disk path, and parsed graph. Paths let the editor save each graph back.
pub fn load_world_graphs(
    manifest_path: &std::path::Path,
) -> Result<Vec<(GraphSlot, String, std::path::PathBuf, Graph)>, String> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest: WorldManifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let dir = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut out = vec![
        (GraphSlot::World, "World".to_string(), dir.join(&manifest.world),
            read_graph(&dir.join(&manifest.world))?),
        (GraphSlot::Zone, "Zone".to_string(), dir.join(&manifest.zone),
            read_graph(&dir.join(&manifest.zone))?),
    ];
    for entry in &manifest.biomes {
        out.push((
            GraphSlot::Biome(entry.id),
            biome_label(&entry.graph, entry.id),
            dir.join(&entry.graph),
            read_graph(&dir.join(&entry.graph))?,
        ));
    }
    // Resolve cross-graph pins so GraphRef pins render correctly.
    out[0].3.derive_output_boundary();
    let world_boundary = out[0].3.boundary.clone();
    out[1].3.resolve_graph_refs(|t| match t {
        nodegraph_ir::GraphRefTarget::World => Some(world_boundary.clone()),
        _ => None,
    });
    Ok(out)
}

/// Which hierarchy slot a graph file belongs to, resolved through the manifest.
///
/// The manifest is the resolution root (P2), so it - not the editor's in-memory
/// state - is what answers "what did this file change?". Detail graphs map to
/// their biome, since a detail edit invalidates that biome's chunks.
pub fn slot_for_graph_file(
    manifest_path: &std::path::Path,
    changed: &std::path::Path,
) -> Option<GraphSlot> {
    let changed = changed.canonicalize().ok()?;
    let text = std::fs::read_to_string(manifest_path).ok()?;
    let manifest: WorldManifest = serde_json::from_str(&text).ok()?;
    let dir = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let same = |rel: &str| {
        dir.join(rel)
            .canonicalize()
            .map(|p| p == changed)
            .unwrap_or(false)
    };

    if same(&manifest.world) {
        return Some(GraphSlot::World);
    }
    if same(&manifest.zone) {
        return Some(GraphSlot::Zone);
    }
    for entry in &manifest.biomes {
        if same(&entry.graph) || entry.detail.as_deref().is_some_and(same) {
            return Some(GraphSlot::Biome(entry.id));
        }
    }
    None
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
    WorldGenerator::from_manifest(&world_manifest_path(), params.seed as u64).map(Arc::new)
}

/// Rewrite `manifest_path`, adding a biome entry `(id, graph_file)`. `graph_file`
/// is stored relative to the manifest directory. Overwrites an existing entry
/// with the same id.
pub fn add_biome_entry(
    manifest_path: &std::path::Path,
    id: u16,
    graph_file: &str,
) -> Result<(), String> {
    let text = std::fs::read_to_string(manifest_path).map_err(|e| e.to_string())?;
    let mut manifest: WorldManifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    match manifest.biomes.iter_mut().find(|b| b.id == id) {
        Some(entry) => entry.graph = graph_file.to_string(),
        None => manifest.biomes.push(BiomeManifestEntry {
            id,
            graph: graph_file.to_string(),
            detail: None,
            params: HashMap::new(),
        }),
    }
    let out = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(manifest_path, out).map_err(|e| e.to_string())
}

/// Rewrite `manifest_path`, removing the biome entry with the given id.
pub fn remove_biome_entry(
    manifest_path: &std::path::Path,
    id: u16,
) -> Result<(), String> {
    let text = std::fs::read_to_string(manifest_path).map_err(|e| e.to_string())?;
    let mut manifest: WorldManifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    manifest.biomes.retain(|b| b.id != id);
    let out = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(manifest_path, out).map_err(|e| e.to_string())
}

/// The graphs directory (public so the editor can place new graph files there).
pub fn graphs_dir_public() -> std::path::PathBuf {
    graphs_dir()
}

/// A minimal, valid, renderable Biome graph: a Simplex height field remapped to
/// a Y range, turned into density by subtracting world-Y, materialized by a
/// Layer, and terminated at a DensityOutput. A same starting point for a new
/// biome the author then edits.
pub fn new_biome_graph() -> Graph {
    use glam::Vec2;
    use nodegraph_ir::*;

    let mut g = Graph::of_kind(GraphKind::Biome);
    let noise = NoiseParams { frequency: 0.005, ..NoiseParams::default() };
    let height = g.add_node_at(NodeKind::Simplex2D(noise), Vec2::new(0.0, 0.0));
    let remap = g.add_node_at(
        NodeKind::Remap(RemapParams { src_lo: -1.0, src_hi: 1.0, dst_lo: 24.0, dst_hi: 56.0 }),
        Vec2::new(240.0, 0.0),
    );
    let y = g.add_node_at(
        NodeKind::WorldAxis(WorldAxisParams { axis: Axis::Y }),
        Vec2::new(240.0, 160.0),
    );
    let density = g.add_node_at(NodeKind::Subtract(SubtractParams::default()), Vec2::new(480.0, 60.0));
    let layer = g.add_node_at(NodeKind::Layer(LayerParams::default()), Vec2::new(720.0, 180.0));
    let out = g.add_node_at(NodeKind::DensityOutput(DensityOutputParams::default()), Vec2::new(960.0, 60.0));

    let _ = g.connect(PinRef::new(height, 0), PinRef::new(remap, 0));
    let _ = g.connect(PinRef::new(remap, 0), PinRef::new(density, 0)); // a = height
    let _ = g.connect(PinRef::new(y, 0), PinRef::new(density, 1));     // b = world Y
    let _ = g.connect(PinRef::new(density, 0), PinRef::new(layer, 0)); // density → material
    let _ = g.connect(PinRef::new(density, 0), PinRef::new(out, 0));   // density terminal
    let _ = g.connect(PinRef::new(layer, 0), PinRef::new(out, 1));     // material terminal
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_biome_graph_loads_with_density_output() {
        // `new` rejects a biome graph without a DensityOutput, so a successful
        // load proves the default graph has one.
        WorldGenerator::from_path(&default_graph_path(), 0)
            .expect("biome_meadow.graph.json must load (and have a DensityOutput)");
    }

    #[test]
    fn generated_chunk_tags_default_to_single_biome() {
        let generator = WorldGenerator::from_path(&default_graph_path(), 0)
            .expect("biome_meadow.graph.json must load");
        let generated = generator.generate_chunk(IVec3::ZERO);
        // Empty World/Zone graphs -> legacy single-biome tags.
        assert_eq!(generated.tags.zone, ZoneId(0));
        assert_eq!(generated.tags.biomes.as_slice(), &[BiomeId(0)]);
    }

    #[test]
    fn generate_chunk_is_deterministic() {
        use crate::world::chunk::CHUNK_VOLUME;
        let generator = WorldGenerator::from_manifest(&world_manifest_path(), 7)
            .expect("world manifest must load");
        let pos = IVec3::new(1, 0, 2);
        let a = generator.generate_chunk(pos);
        let b = generator.generate_chunk(pos);
        // Full voxel compare (material + shape + flags) so slab-smoothing
        // determinism is covered, not just occupancy.
        let diffs = (0..CHUNK_VOLUME)
            .filter(|&i| a.storage.voxel(i) != b.storage.voxel(i))
            .count();
        assert_eq!(diffs, 0, "generate_chunk non-deterministic: {diffs} voxels differ");
    }

    #[test]
    fn generate_chunk_is_deterministic_over_slabs() {
        use crate::world::chunk::CHUNK_VOLUME;
        use voxel_core::ShapeId;

        let generator = WorldGenerator::from_manifest(&world_manifest_path(), 7)
            .expect("world manifest must load");

        let slab_count = |g: &GeneratedChunk| {
            (0..CHUNK_VOLUME)
                .filter(|&i| {
                    matches!(
                        g.storage.voxel(i).shape,
                        ShapeId::SlabBottom | ShapeId::SlabTop
                    )
                })
                .count()
        };

        // Guard against a vacuous test. `generate_chunk_is_deterministic` above
        // uses chunk (1,0,2) and asserts nothing about its contents, so if that
        // chunk happens to hold no slabs it proves nothing about slab smoothing
        // (stage 7) - the exact concern raised in `cowork-handoff.md` Part 1.
        // Search for a chunk that demonstrably has slabs, and fail with a
        // diagnostic if the shipped world has none anywhere near the origin.
        let mut found = None;
        'search: for cz in -1..=1 {
            for cx in -1..=1 {
                let pos = IVec3::new(cx, 0, cz);
                let g = generator.generate_chunk(pos);
                let n = slab_count(&g);
                if n > 0 {
                    found = Some((pos, g, n));
                    break 'search;
                }
            }
        }

        let (pos, first, slabs) = found.expect(
            "no slab-bearing chunk in the 3x3 around the origin - slab smoothing \
            (stage 7) is not producing slabs, so any determinism test here would \
            be vacuous. Check traversal_smoothing_distance and smooth_slabs.",
        );

        // Now the real assertion: regenerating that chunk is bit-identical,
        // shape included, so slab smoothing is deterministic and not merely
        // untested.
        let second = generator.generate_chunk(pos);
        assert_eq!(
            slabs,
            slab_count(&second),
            "slab count differs between two generations of {pos:?}",
        );
        let diffs = (0..CHUNK_VOLUME)
            .filter(|&i| first.storage.voxel(i) != second.storage.voxel(i))
            .count();
        assert_eq!(
            diffs, 0,
            "slab-bearing chunk {pos:?} ({slabs} slabs) non-deterministic: \
             {diffs} voxels differ",
        );
    }

    #[test]
    fn distinct_sorted_dedups_and_orders() {
        assert_eq!(distinct_sorted(&[2, 0, 2, 1, 0]), vec![0, 1, 2]);
        assert_eq!(distinct_sorted(&[]), Vec::<u16>::new());
    }

    #[test]
    fn paint_translates_to_detail_layers() {
        use nodegraph_eval::{PaintLayer, PaintTexel};
        let mut texels = Box::new([PaintTexel::default(); 32 * 32]);
        texels[5] = PaintTexel { species: 2, density: 200, tint: 1, flags: 0 };
        // Air storage -> no surface -> submersion filter inert; empty fluids anyway.
        let storage = ChunkStorage::new_air();
        let fluids = FluidLayer::default();
        let dl = paint_to_detail_layers(&[PaintLayer { layer_id: 3, texels }], &storage, &fluids);
        assert_eq!(dl.layers.len(), 1);
        assert_eq!(dl.layers[0].layer_id, DetailLayerId(3));
        assert_eq!(dl.layers[0].map[5].density, 200);
        assert_eq!(dl.layers[0].map[5].species, 2);
        assert_eq!(dl.layers[0].map[0].density, 0); // untouched column
    }

    #[test]
    fn scatter_translates_to_store_carrying_stable_id() {
        use nodegraph_eval::FoliageInstance;
        let bucket = ScatterBucket {
            type_id: 4,
            instances: vec![FoliageInstance {
                anchor: [3, 17, 9],
                sub_offset: [-2, 0, 5],
                rotation_y: 200,
                scale_variant: 1,
                prefab_id: 42,
                stable_id: 0xDEAD_BEEF,
                flags: ScatterFlags::HARVESTABLE,
            }],
        };
        let store = scatter_to_store(&[bucket], &FluidLayer::default());
        let instances = store.by_type.get(&ScatterTypeId(4)).expect("bucket present");
        assert_eq!(instances.len(), 1);
        let si = instances[0];
        assert_eq!(si.anchor, LocalPos::new_unchecked(3, 17, 9));
        assert_eq!(si.sub_offset, [-2, 0, 5]);
        assert_eq!(si.rotation_y, 200);
        assert_eq!(si.scale_variant, 1);
        assert_eq!(si.prefab_id, PrefabId(42));
        assert_eq!(si.flags.0, ScatterFlags::HARVESTABLE);
        assert_eq!(si.stable_id, StableInstanceId(0xDEAD_BEEF)); // now carried
        assert!(store.by_type.get(&ScatterTypeId(0)).is_none());
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
        let generator = WorldGenerator::from_manifest(&world_manifest_path(), 0)
            .expect("world.manifest.json and its graphs must load");
        // The hierarchy composites a chunk without panicking.
        let _ = generator.generate_chunk(IVec3::ZERO);
    }

    #[test]
    fn manifest_parses_biome_detail() {
        let json = r#"{
            "world": "world.graph.json",
            "zone": "zone.graph.json",
            "biomes": [
                { "id": 0, "graph": "biome_meadow.graph.json", "detail": "biome_meadow.detail.json" },
                { "id": 1, "graph": "biome_rocky.graph.json" }
            ]
        }"#;
        let m: WorldManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.biomes[0].detail.as_deref(), Some("biome_meadow.detail.json"));
        assert_eq!(m.biomes[1].detail, None);
    }

    #[test]
    fn meadow_detail_graph_has_valid_scatter_chain() {
        // `read_graph` runs `Graph::from_json`, which validates kind-rules (a
        // Detail graph must have >= 1 paint/scatter terminal). A successful load
        // plus a present ScatterPlace proves the authored chain is well-formed.
        let path = graphs_dir().join("biome_meadow.detail.json");
        let graph = read_graph(&path).expect("biome_meadow.detail.json must load and validate");
        let has_scatter = graph
            .nodes
            .values()
            .any(|n| matches!(n.kind, NodeKind::ScatterPlace(_)));
        assert!(has_scatter, "meadow detail graph must contain a ScatterPlace terminal");
    }

    #[test]
    fn migrated_zone_imports_world_climate() {
        use nodegraph_ir::GraphRefTarget;
        let dir = graphs_dir();
        let mut world = read_graph(&dir.join("world.graph.json")).expect("world.graph.json");
        let mut zone = read_graph(&dir.join("zone.graph.json")).expect("zone.graph.json");

        // World exposes a "climate" boundary output.
        world.derive_output_boundary();
        assert!(world.boundary.outputs.iter().any(|p| p.name == "climate"));

        // Zone's GraphRef(World) resolves to a "climate" pin.
        let wb = world.boundary.clone();
        zone.resolve_graph_refs(|t| (t == GraphRefTarget::World).then(|| wb.clone()));
        let gr = zone
            .nodes
            .values()
            .find(|n| matches!(n.kind, NodeKind::GraphRef(_)))
            .expect("zone has a GraphRef");
        assert!(
            gr.kind.effective_outputs().iter().any(|p| p.name == "climate"),
            "GraphRef must expose World's climate pin"
        );

        // Zone no longer derives its own climate, and the resolved graph is valid.
        assert!(
            !zone.nodes.values().any(|n| matches!(n.kind, NodeKind::SurfaceNoise(_))),
            "zone should import climate, not derive it"
        );
        assert!(!zone.has_errors(), "resolved zone graph must validate");
    }

    #[test]
    fn manifest_parses_biome_params() {
        let json = r#"{
            "world": "world.graph.json",
            "zone": "zone.graph.json",
            "biomes": [
                { "id": 0, "graph": "biome_meadow.graph.json", "params": { "traversal_smoothing_distance": 4.0 } },
                { "id": 1, "graph": "biome_rocky.graph.json" }
            ]
        }"#;
        let m: WorldManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.biomes[0].params.get("traversal_smoothing_distance"), Some(&4.0));
        assert!(m.biomes[1].params.is_empty());
    }

    #[test]
    fn submerged_scatter_is_dropped() {
        use nodegraph_eval::FoliageInstance;
        use super::super::layers::{FluidCell, FluidId};
        // Water at (5,11,5): the instance anchored at (5,10,5) has water directly
        // above and is dropped; the one at (8,10,8) survives.
        let mut fluids = FluidLayer::default();
        fluids.cells.insert(
            LocalPos::new_unchecked(5, 11, 5),
            FluidCell { fluid_id: FluidId::WATER, mass: 65535, flags: 0 },
        );
        let bucket = ScatterBucket {
            type_id: 0,
            instances: vec![
                FoliageInstance { anchor: [5, 10, 5], sub_offset: [0, 0, 0], rotation_y: 0,
                    scale_variant: 0, prefab_id: 1, stable_id: 1, flags: 0 },
                FoliageInstance { anchor: [8, 10, 8], sub_offset: [0, 0, 0], rotation_y: 0,
                    scale_variant: 0, prefab_id: 1, stable_id: 2, flags: 0 },
            ],
        };
        let store = scatter_to_store(&[bucket], &fluids);
        let insts = store.by_type.get(&ScatterTypeId(0)).expect("dry instance kept");
        assert_eq!(insts.len(), 1);
        assert_eq!(insts[0].stable_id, StableInstanceId(2));
    }

    #[test]
    fn submerged_fill_mode_drops_all_foliage() {
        use nodegraph_eval::{FoliageInstance, PaintLayer, PaintTexel};
        use super::super::layers::FluidId;
        let mut fluids = FluidLayer::default();
        fluids.fill_mode = FluidFillMode::Submerged(FluidId::WATER);

        // Scatter: all dropped (submerged chunk).
        let bucket = ScatterBucket {
            type_id: 0,
            instances: vec![FoliageInstance { anchor: [1, 5, 1], sub_offset: [0, 0, 0],
                rotation_y: 0, scale_variant: 0, prefab_id: 1, stable_id: 1, flags: 0 }],
        };
        assert!(scatter_to_store(&[bucket], &fluids).by_type.is_empty());

        // Paint: a solid column's painted texel is dropped.
        let mut storage = ChunkStorage::new_air();
        storage.set_voxel(1 + 5 * 32 + 1 * 32 * 32, voxel_core::Voxel::cube(voxel_core::MaterialId(1)));
        let mut texels = Box::new([PaintTexel::default(); 32 * 32]);
        texels[1 + 1 * 32] = PaintTexel { species: 1, density: 200, tint: 0, flags: 0 };
        let dl = paint_to_detail_layers(&[PaintLayer { layer_id: 0, texels }], &storage, &fluids);
        assert_eq!(dl.layers[0].map[1 + 1 * 32].density, 0, "submerged column paints nothing");
    }
}
