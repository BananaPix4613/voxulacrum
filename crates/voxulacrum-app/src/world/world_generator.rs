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
    zone_labels: Vec<(u16, String)>,
    biome_labels: Vec<(u16, String)>,
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
            zone_labels: Vec::new(),
            biome_labels: Vec::new(),
        })
    }
    
    /// Load a generator from a `*.graph.json` file on disk.
    #[allow(dead_code)] // single-graph loader; app runs the manifest path, tests use this
    pub fn from_path(path: &std::path::Path, world_seed: u64) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let graph = Graph::from_json(&text).map_err(|e| e.to_string())?;
        Self::new(graph, world_seed)
    }

    /// Build a generator from a world manifest, at the seed the manifest
    /// declares.
    pub fn from_manifest(manifest_path: &std::path::Path) -> Result<Self, String> {
        let hierarchy = load_hierarchy(manifest_path)?;
        let seed = hierarchy.seed;
        Ok(Self::from_hierarchy(hierarchy, seed))
    }

    /// Same, with the manifest's seed overridden.
    ///
    /// For `--verify-generation --seed S`, which checks determinism at seeds the
    /// world was not authored against. Nothing in the running engine uses it -
    /// a seed the engine chose over the manifest's would be exactly the second
    /// source of truth this move removes.
    pub fn from_manifest_seeded(
        manifest_path: &std::path::Path,
        world_seed: u64,
    ) -> Result<Self, String> {
        let hierarchy = load_hierarchy(manifest_path)?;
        Ok(Self::from_hierarchy(hierarchy, world_seed))
    }

    /// The seed this generator was built at.
    pub fn world_seed(&self) -> u64 {
        self.world_seed
    }

    /// Assemble a generator from an already-loaded hierarchy.
    fn from_hierarchy(hierarchy: LoadedHierarchy, world_seed: u64) -> Self {
        // The primary biome anchors `WorldEvaluator::new`; `with_biomes` then
        // installs the full set, replacing that placeholder entry.
        let primary = hierarchy.biomes[0].1.clone();
        // Libraries load from their asset directory rather than being threaded
        // in from the app: the generator is built in several places (startup,
        // regen, the headless verifier) and a library set that depended on which
        // one built it would be a second source of truth.
        let libraries = crate::libraries::load_libraries();
        let world_eval = WorldEvaluator::new(primary)
            .with_world(hierarchy.world)
            .with_zones(hierarchy.zones)
            .with_biomes(hierarchy.biomes)
            .with_biome_details(hierarchy.details)
            .with_biome_params(hierarchy.biome_params)
            .with_libraries(libraries.registry().clone())
            .with_library_kernels(libraries.kernels().clone())
            .with_cross_graph_resolved();
        Self {
            world_eval,
            world_seed,
            storage_boundary: StorageBoundary::new(hierarchy.sea_level),
            zone_labels: hierarchy.zone_labels,
            biome_labels: hierarchy.biome_labels,
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

    /// Biome id at a world XZ position, resolved straight from generation -
    /// independent of whether that region's chunks are streamed in.
    ///
    /// Pointwise. The climate simulation calls this per cloud cell, and until
    /// Substep 12c it went through a path that filled every zone graph over a
    /// whole chunk to read one column: 2.12 ms mean, 15.17 ms max, and the
    /// entire measured cost of the `sim` stage.
    pub fn biome_id_at_world(&self, wx: f32, wz: f32) -> Option<u16> {
        self.world_eval
            .sample_biome_at(self.world_seed, wx.floor() as i32, wz.floor() as i32)
            .ok()
    }

    /// Inspect the column at integer world coordinates (roadmap §4.3).
    ///
    /// Mirrors `biome_id_at_world`'s framing: the caller names a world column
    /// and this resolves the owning chunk, so nothing outside generation has to
    /// know how columns map to chunks.
    pub fn inspect_column(
        &self,
        wx: i32,
        wz: i32,
        chunk_y_range: std::ops::Range<i32>,
    ) -> Result<ColumnInspection, String> {
        let dim = CHUNK_SIZE as i32;
        let ctx = EvalContext::new(
            self.world_seed,
            IVec3::new(wx.div_euclid(dim), 0, wz.div_euclid(dim)),
        );
        let report = self
            .world_eval
            .inspect_column(
                ctx,
                wx.rem_euclid(dim) as usize,
                wz.rem_euclid(dim) as usize,
                chunk_y_range,
            )
            .map_err(|e| e.to_string())?;
        let name_of = |table: &[(u16, String)], id: u16| {
            table
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| format!("unregistered {id}"))
        };
        Ok(ColumnInspection {
            zone_name: name_of(&self.zone_labels, report.zone_id),
            biome_name: name_of(&self.biome_labels, report.biome_id),
            neighbor_name: name_of(&self.biome_labels, report.border_neighbor),
            sea_level: self.sea_level(),
            report,
        })
    }


    /// Sample a top-down id map centred on `(cx, cz)` (roadmap §4.3).
    pub fn sample_id_map(
        &self,
        center_x: i32,
        center_z: i32,
        step: i32,
        dim: usize,
    ) -> Result<nodegraph_eval::IdMap, String> {
        let half = (dim as i32 / 2) * step.max(1);
        self.world_eval
            .sample_id_map(
                self.world_seed,
                center_x - half,
                center_z - half,
                step,
                dim,
            )
            .map_err(|e| e.to_string())
    }

    /// `(id, name)` per zone, for legends and pickers.
    pub fn zone_labels(&self) -> &[(u16, String)] {
        &self.zone_labels
    }

    /// `(id, name)` per biome.
    pub fn biome_labels(&self) -> &[(u16, String)] {
        &self.biome_labels
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
        let biome_col = eval.biome_ids.clone();
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
        let zone_ids = eval
            .zone_ids
            .as_ref()
            .map(|ids| distinct_sorted(ids.data()))
            .unwrap_or_default();
        let biome_ids = eval
            .biome_ids
            .as_ref()
            .map(|ids| distinct_sorted(ids.data()))
            .unwrap_or_default();

        // Every distinct zone, not a representative: a chunk on a zone border
        // belongs to both, and an edit to either must reach it.
        let mut zones: SmallVec<[ZoneId; 2]> = zone_ids.into_iter().map(ZoneId).collect();
        if zones.is_empty() {
            zones.push(ZoneId(0)); // single-zone fallback (empty World graph)
        }
        let mut biomes: SmallVec<[BiomeId; 4]> = biome_ids.into_iter().map(BiomeId).collect();
        if biomes.is_empty() {
            biomes.push(BiomeId(0)); // single-biome fallback (empty Zone graph)
        }
        ChunkTags { zones, biomes, library_refs: SmallVec::new() }
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

/// Identifies one editable source in the world hierarchy, for routing edits and
/// invalidation.
///
/// Mostly graphs, plus the manifest - which is not a graph but is an edit source
/// with its own invalidation rule (design §4's table begins with it).
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum GraphSlot {
    /// The world manifest itself: sea level, seed, graph registrations.
    /// 
    /// Produced only by [`slot_for_graph_file`]; the editor's slot list never
    /// contains it, because there is no canvas for it.
    Manifest,
    /// The singleton World graph.
    World,
    /// The zone graph, by zone id.
    Zone(u16),
    /// A biome graph, by biome id.
    Biome(u16),
    /// A biome's detail (foliage) graph, by the biome id it belongs to.
    ///
    /// Distinct from `Biome` even though both invalidate the same chunks: the
    /// editor needs to open them as separate canvases, and design §4 specifies a
    /// detail-only re-pass that this slot is the prerequisite for.
    Detail(u16),
}

/// A [`ColumnReport`](nodegraph_eval::ColumnReport) with its ids resolved to
/// names and the world constants a reader needs to interpret it.
///
/// Names are resolved here rather than in `nodegraph-eval`: that crate deals in
/// ids and knows nothing about manifests, and it should stay that way.
pub struct ColumnInspection {
    pub report: nodegraph_eval::ColumnReport,
    pub zone_name: String,
    pub biome_name: String,
    pub neighbor_name: String,
    /// Manifest sea level, so the panel can report the ocean the storage
    /// boundary will apply - which generation's fluid output does not include.
    pub sea_level: i32,
}

/// Current manifest schema version.
///
/// Present from the first byte deliberately: 0.6.0's format endgame has to
/// migrate every asset format this version introduces, and a format with no
/// version field can only be migrated by guessing at its shape (roadmap §8).
pub const MANIFEST_VERSION: u32 = 1;

/// Seed used when a manifest declares none - the value the shipped world was
/// authored against, carried over from `TerrainGenParams` when the seed moved
/// onto the manifest. A pre-seed manifest therefore keeps generating the world
/// it always did.
pub const DEFAULT_SEED: u64 = 54321;

/// One zone entry in a [`WorldManifest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneManifestEntry {
    /// Zone id this graph governs (matches the World graph's assignment).
    pub id: u16,
    /// Zone graph file, relative to the manifest directory.
    pub graph: String,
}

/// On-disk manifest describing a world's graph hierarchy. Paths are relative to
/// the manifest file's directory.
///
/// Read through [`WorldManifest::read`], never by deserializing directly, so
/// every consumer gets a migrated manifest and none has to know what a pre-v1
/// file looked like.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldManifest {
    /// Schema version. Absent means pre-v1; [`WorldManifest::migrate`] upgrades
    /// in memory and the next write emits the current version.
    #[serde(default)]
    pub version: u32,
    /// World graph file (climate + zone assignment).
    pub world: String,
    /// Global ocean surface (world-Y); empty voxels at or below fill with water.
    /// Absent => 0 (effectively no ocean for a world sitting above Y 0).
    #[serde(default)]
    pub sea_level: i32,
    /// World seed. Every RNG in generation derives from it.
    ///
    /// Here rather than on `TerrainGenParams` because P1 states determinism over
    /// *same seed + same graphs + same coordinates*, and that is only checkable
    /// if all three live in one place. It also makes a world's identity
    /// self-contained: the manifest and the graphs it names are the whole input.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Zone graphs, keyed by the zone id each governs.
    #[serde(default)]
    pub zones: Vec<ZoneManifestEntry>,
    /// **Legacy, pre-v1.** The single-zone field, read for migration only and
    /// never written back - `migrate` folds it into `zones` as zone 0. Remove
    /// when no unmigrated manifest can exist, i.e. at the 0.6.0 format freeze.
    #[serde(rename = "zone", default, skip_serializing)]
    pub legacy_zone: Option<String>,
    /// Biome graphs, keyed by the biome id each renders.
    pub biomes: Vec<BiomeManifestEntry>,
}

fn default_seed() -> u64 {
    DEFAULT_SEED
}

impl WorldManifest {
    /// Read and migrate a manifest. The single parse site - five call sites
    /// previously each did their own `read_to_string` + `from_str`, which is
    /// five places a migration would have had to be remembered.
    pub fn read(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut manifest: WorldManifest =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        manifest.migrate();
        if manifest.zones.is_empty() {
            return Err(format!("{}: manifest lists no zones", path.display()));
        }
        Ok(manifest)
    }

    /// Write a manifest, stamping the current schema version.
    ///
    /// The single write site, for the same reason [`Self::read`] is the single
    /// parse site: a second writer is a second place the version stamp or the
    /// legacy-field suppression can be forgotten.
    pub fn write(&self, path: &std::path::Path) -> Result<(), String> {
        let mut out = self.clone();
        out.version = MANIFEST_VERSION;
        let json = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Bring an in-memory manifest up to [`MANIFEST_VERSION`]. Idempotent, so
    /// re-reading an already-migrated file is a no-op.
    fn migrate(&mut self) {
        // Pre-v1 files carry `zone` and no `zones`. A file with both is not a
        // shape this engine ever wrote; `zones` wins, because it is the one the
        // author edited most recently by definition.
        if self.zones.is_empty() {
            if let Some(graph) = self.legacy_zone.take() {
                self.zones.push(ZoneManifestEntry { id: 0, graph });
            }
        }
        self.legacy_zone = None;
        self.version = MANIFEST_VERSION;
    }
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
    zones: Vec<(u16, Graph)>,
    biomes: Vec<(u16, Graph)>,
    /// Per-biome DetailGraphs (by biome id) for biomes that reference one.
    details: Vec<(u16, Graph)>,
    /// Per-biome scalar parameter sidecars (by biome id).
    biome_params: Vec<(u16, BiomeParams)>,
    /// Global ocean surface (world-Y), from the manifest.
    sea_level: i32,
    /// World seed, from the manifest.
    seed: u64,
    /// `(id, display name)` per zone and per biome, for tools that must name an
    /// id rather than print it. A bare id is not an answer to "which one is
    /// this" — the same reason library references are picked by name.
    zone_labels: Vec<(u16, String)>,
    biome_labels: Vec<(u16, String)>,
}

/// Read and parse one `*.graph.json` file, resolving its blueprint references.
///
/// Blueprint resolution lives *here*, at the single read site, rather than at
/// each caller: `load_hierarchy` (generator) and `load_world_graphs` (editor)
/// both go through this function, and resolving on only one of them is the
/// mistake `resolve_library_refs` made - a reference that looked fine on one
/// path and was silently empty on the other.
///
/// The registry is loaded per read rather than cached: a cached one would
/// survive a `materials.ron` edit and resolve against a stale table.
fn read_graph(path: &std::path::Path) -> Result<Graph, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut graph = Graph::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let registry = crate::materials::load_registry();
    nodegraph_hotreload::resolve_blueprints(
        &mut graph,
        &crate::world::blueprint::blueprint_dir(),
        &registry,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(graph)
}

/// Load a manifest and every graph it references.
fn load_hierarchy(manifest_path: &std::path::Path) -> Result<LoadedHierarchy, String> {
    let manifest = WorldManifest::read(manifest_path)?;
    let dir = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let world = read_graph(&dir.join(&manifest.world))?;
    let mut zones = Vec::with_capacity(manifest.zones.len());
    for entry in &manifest.zones {
        zones.push((entry.id, read_graph(&dir.join(&entry.graph))?));
    }
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
    let zone_labels: Vec<(u16, String)> = manifest
        .zones
        .iter()
        .map(|z| (z.id, zone_label(&z.graph, z.id)))
        .collect();
    let biome_labels: Vec<(u16, String)> = manifest
        .biomes
        .iter()
        .map(|b| (b.id, biome_label(&b.graph, b.id)))
        .collect();
    Ok(LoadedHierarchy {
        world,
        zones,
        biomes,
        details,
        biome_params,
        sea_level: manifest.sea_level,
        seed: manifest.seed,
        zone_labels,
        biome_labels,
    })
}

/// Load every editable graph in a world manifest: its slot, display label,
/// on-disk path, and parsed graph. Paths let the editor save each graph back.
pub fn load_world_graphs(
    manifest_path: &std::path::Path,
) -> Result<Vec<(GraphSlot, String, std::path::PathBuf, Graph)>, String> {
    let manifest = WorldManifest::read(manifest_path)?;
    let dir = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    // Every zone is editable, whether or not the evaluator reaches it yet
    // (Substep 4b) - a zone you cannot open is a zone you cannot author.
    let mut out = vec![
        (GraphSlot::World, "World".to_string(), dir.join(&manifest.world),
            read_graph(&dir.join(&manifest.world))?),
    ];
    for entry in &manifest.zones {
        out.push((
            GraphSlot::Zone(entry.id),
            zone_label(&entry.graph, entry.id),
            dir.join(&entry.graph),
            read_graph(&dir.join(&entry.graph))?,
        ));
    }
    for entry in &manifest.biomes {
        let label = biome_label(&entry.graph, entry.id);
        out.push((
            GraphSlot::Biome(entry.id),
            label.clone(),
            dir.join(&entry.graph),
            read_graph(&dir.join(&entry.graph))?,
        ));
        // Immediately after its biome, so the selector reads as a hierarchy
        // rather than two lists a reader has to join mentally.
        if let Some(detail) = &entry.detail {
            out.push((
                GraphSlot::Detail(entry.id),
                format!("{label} Detail"),
                dir.join(detail),
                read_graph(&dir.join(detail))?,
            ));
        }
    }
    // Library pins, on this path too. This was resolved only on the generator's
    // path, so a `LibraryRef` opened in the editor showed no pins no matter how
    // many times it was saved and reloaded - the same two-resolution-paths
    // mistake as the `out[1]` index this loop replaced.
    let libraries = crate::libraries::load_libraries();
    for entry in out.iter_mut() {
        entry.3.resolve_library_refs(libraries.registry());
    }

    out[0].3.derive_output_boundary();
    let world_boundary = out[0].3.boundary.clone();
    for (slot, label, _, graph) in out.iter_mut().skip(1) {
        graph.resolve_graph_refs(|t| match t {
            nodegraph_ir::GraphRefTarget::World => Some(world_boundary.clone()),
            _ => None,
        });
        if let Some(node) = unresolved_graph_ref(graph) {
            log::warn!(
                "{label} ({slot:?}): GraphRef {node:?} did not resolve - it will \
                 render with no pins, and saving this graph would drop the wires \
                 into it",
            );
        }
    }
    Ok(out)
}

/// The first `GraphRef` in `graph` whose pins were never resolved, if any.
///
/// A node in that state renders with no pins and cannot round-trip through the
/// canvas, so this is a save-time data-loss hazard rather than a display defect.
/// Checked after every resolution pass so a future call site that forgets one
/// says so at load time instead of at the next Save.
fn unresolved_graph_ref(graph: &Graph) -> Option<nodegraph_ir::NodeId> {
    graph.nodes.iter().find_map(|(id, n)| match &n.kind {
        NodeKind::GraphRef(p) if p.resolved.is_none() => Some(id),
        _ => None,
    })
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
    if manifest_path.canonicalize().map(|p| p == changed).unwrap_or(false) {
        return Some(GraphSlot::Manifest);
    }
    let manifest = WorldManifest::read(manifest_path).ok()?;
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
    if let Some(entry) = manifest.zones.iter().find(|z| same(&z.graph)) {
        return Some(GraphSlot::Zone(entry.id));
    }
    for entry in &manifest.biomes {
        if same(&entry.graph) {
            return Some(GraphSlot::Biome(entry.id));
        }
        if entry.detail.as_deref().is_some_and(same) {
            return Some(GraphSlot::Detail(entry.id));
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

/// A display label for a zone from its file name (e.g. "zone_highlands.graph.json"
/// -> "Highlands", "zone.graph.json" -> "Zone"), falling back to the id.
fn zone_label(file: &str, id: u16) -> String {
    let stem = file.strip_suffix(".graph.json").unwrap_or(file);
    let name = stem.strip_prefix("zone_").unwrap_or(stem);
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => format!("Zone {id}"),
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
pub fn load_default() -> Result<Arc<WorldGenerator>, String> {
    WorldGenerator::from_manifest(&world_manifest_path()).map(Arc::new)
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

/// A minimal, valid ZoneGraph: reads the World graph's first declared output as
/// a climate field and assigns biome 0 to every column. A starting point the
/// author then bands with the ZoneOutput picker.
///
/// Takes the World boundary because a `GraphRef` has **no pins until resolved**,
/// and `Graph::connect` validates against the pin count - so an unresolved ref
/// cannot be wired. Resolve, then connect.
///
/// Pin 0 is the World's first boundary output, which for the shipped World graph
/// is `climate`. A World graph exposing several outputs may need the author to
/// rewire; a World graph exposing none leaves the ZoneOutput's required input
/// unconnected, which surfaces as an error badge rather than silently.
pub fn new_zone_graph(world_boundary: &nodegraph_ir::GraphBoundary) -> Graph {
    use glam::Vec2;
    use nodegraph_ir::*;

    let mut g = Graph::of_kind(GraphKind::Zone);
    let climate = g.add_node_at(
        NodeKind::GraphRef(GraphRefParams {
            target: GraphRefTarget::World,
            ..Default::default()
        }),
        Vec2::new(0.0, 0.0),
    );
    let boundary = world_boundary.clone();
    g.resolve_graph_refs(|t| match t {
        GraphRefTarget::World => Some(boundary.clone()),
        _ => None,
    });
    let out = g.add_node_at(
        NodeKind::ZoneOutput(ZoneOutputParams {
            biome_bands: Vec::new(),
            biome_ids: vec![0],
            ..Default::default()
        }),
        Vec2::new(280.0, 0.0),
    );
    let _ = g.connect(PinRef::new(climate, 0), PinRef::new(out, 0));
    g
}

/// A minimal, valid DetailGraph: one `PaintDensity` terminal painting a default
/// species across the biome.
///
/// `PaintDensity`'s surface input is optional, so this validates as authored and
/// produces visible foliage immediately - an author can see the graph is
/// attached before authoring anything into it.
pub fn new_detail_graph() -> Graph {
    use glam::Vec2;
    use nodegraph_ir::*;

    let mut g = Graph::of_kind(GraphKind::Detail);
    g.add_node_at(
        NodeKind::PaintDensity(PaintDensityParams {
            species: 1,
            density: 180,
            ..Default::default()
        }),
        Vec2::new(0.0, 0.0),
    );
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
        assert_eq!(generated.tags.zones.as_slice(), &[ZoneId(0)]);
        assert_eq!(generated.tags.biomes.as_slice(), &[BiomeId(0)]);
    }

    #[test]
    fn generate_chunk_is_deterministic() {
        use crate::world::chunk::CHUNK_VOLUME;
        let generator = WorldGenerator::from_manifest(&world_manifest_path())
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

        let generator = WorldGenerator::from_manifest(&world_manifest_path())
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
        // Deserialized directly rather than through `read`, so this covers the
        // pre-v1 shape and the migration that upgrades it - `read` would have
        // done both and tested neither in isolation.
        let mut m: WorldManifest = serde_json::from_str(json).unwrap();
        assert!(m.zones.is_empty(), "pre-v1 has no `zones` until migrated");
        m.migrate();
        assert_eq!(m.world, "world.graph.json");
        assert_eq!(m.zones.len(), 1);
        assert_eq!(m.zones[0].id, 0);
        assert_eq!(m.zones[0].graph, "zone.graph.json");
        assert_eq!(m.biomes.len(), 2);
        assert_eq!(m.biomes[1].id, 1);
        assert_eq!(m.biomes[1].graph, "biome_rocky.graph.json");
    }

    #[test]
    fn default_world_manifest_loads() {
        let generator = WorldGenerator::from_manifest(&world_manifest_path())
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


    #[test]
    fn pre_v1_manifest_migrates_its_single_zone() {
        // The exact shape shipped before this substep.
        let legacy = r#"{
            "world": "world.graph.json",
            "sea_level": 24,
            "zone": "zone.graph.json",
            "biomes": [{ "id": 0, "graph": "b.graph.json" }]
        }"#;
        let mut m: WorldManifest = serde_json::from_str(legacy).expect("legacy parses");
        assert_eq!(m.version, 0, "absent version reads as pre-v1");
        m.migrate();

        assert_eq!(m.version, MANIFEST_VERSION);
        assert_eq!(m.zones.len(), 1);
        assert_eq!(m.zones[0].id, 0);
        assert_eq!(m.zones[0].graph, "zone.graph.json");
        assert!(m.legacy_zone.is_none(), "the legacy field is consumed, not kept");
    }

    #[test]
    fn migration_is_idempotent_and_never_writes_the_legacy_field() {
        let v1 = r#"{
            "version": 1,
            "world": "world.graph.json",
            "sea_level": 24,
            "zones": [{ "id": 0, "graph": "zone.graph.json" }],
            "biomes": [{ "id": 0, "graph": "b.graph.json" }]
        }"#;
        let mut m: WorldManifest = serde_json::from_str(v1).expect("v1 parses");
        m.migrate();
        m.migrate();
        assert_eq!(m.zones.len(), 1, "re-migrating must not duplicate the zone");

        let written = serde_json::to_string(&m).expect("serializes");
        assert!(
            !written.contains("\"zone\""),
            "the legacy field must never be written back: {written}",
        );
        assert!(written.contains("\"version\":1"));
    }

    #[test]
    fn shipped_manifest_is_v1_and_loads() {
        // Locks the asset against the schema, the way the prefab and library
        // registries lock their RON against their fallbacks.
        let m = WorldManifest::read(&world_manifest_path())
            .expect("shipped manifest loads");
        assert_eq!(m.version, MANIFEST_VERSION);
        assert!(!m.zones.is_empty());
        assert!(!m.biomes.is_empty());
    }
    
    #[test]
    fn saving_the_manifest_routes_to_a_structural_edit() {
        // Before this existed the water logged "ignoring unreferenced graph
        // file" and nothing regenerated - so editing sea level in-engine
        // changed the file and not the world.
        let path = world_manifest_path();
        assert_eq!(
            slot_for_graph_file(&path, &path),
            Some(GraphSlot::Manifest),
        );
    }

    #[test]
    fn the_shipped_world_assigns_every_registered_zone_and_biome() {
        // The direct guard for the content gate: "3+ biomes across 2 zones"
        // stays true. Registration is not assignment - `--validate-graphs` warns
        // when a band table *can* never reach an entry, but a band whose
        // threshold no climate value crosses is registered, assignable, and
        // still absent from the world.
        //
        // Sampled rather than generated: assignment is a column-pipeline
        // property, and 4,096 pointwise samples cost a fraction of 4,096 chunks.
        let generator =
            WorldGenerator::from_manifest(&world_manifest_path()).expect("world loads");
        let manifest = WorldManifest::read(&world_manifest_path()).expect("manifest loads");

        let map = generator
            .sample_id_map(0, 0, 32, 64)
            .expect("id map samples");
        let zones: std::collections::HashSet<u16> = map.zone.iter().copied().collect();
        let biomes: std::collections::HashSet<u16> = map.biome.iter().copied().collect();

        for z in &manifest.zones {
            assert!(
                zones.contains(&z.id),
                "zone {} is registered but never assigned within 2048 units of the origin",
                z.id,
            );
        }
        for b in &manifest.biomes {
            assert!(
                biomes.contains(&b.id),
                "biome {} is registered but never assigned within 2048 units of the origin",
                b.id,
            );
        }
    }

    #[test]
    fn the_shipped_world_has_a_library_backed_biome() {
        // The cave-bearing biome carves with `standard_cave_noise` through a
        // `LibraryRef`. Asserted structurally, and against the manifest rather
        // than a file name, so renaming a biome does not break it.
        //
        // Disconnection is covered elsewhere: a `LibraryRef` with an unconnected
        // `position` fails `Graph::validate`, which `--validate-graphs` runs in
        // CI. Containment plus that is reachability.
        let path = world_manifest_path();
        let manifest = WorldManifest::read(&path).expect("manifest loads");
        let dir = path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();

        let has_library = manifest.biomes.iter().any(|b| {
            std::fs::read_to_string(dir.join(&b.graph))
                .ok()
                .and_then(|t| Graph::from_json(&t).ok())
                .is_some_and(|g| {
                    g.nodes
                        .iter()
                        .any(|(_, n)| matches!(n.kind, NodeKind::LibraryRef(_)))
                })
        });
        assert!(
            has_library,
            "no biome references a library — the cave-bearing biome lost its \
             StandardCaveNoise reference, and caves would vanish silently",
        );
    }
}
