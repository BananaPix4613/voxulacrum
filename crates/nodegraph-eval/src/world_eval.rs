//! Multi-graph evaluation harness for the five-graph hierarchy.

use std::collections::HashMap;
use std::sync::Arc;
use glam::IVec3;
use nodegraph_ir::{
    Graph, GraphKind, GraphRefTarget, LibraryGraphRegistry, LibraryGraphId, NodeId, NodeKind,
    Severity,
};
use voxel_core::{ChunkBuffer, MaterialId, ShapeId, Voxel};

use crate::biome_params::BiomeParams;
use crate::border::{analyze_borders, biome_border_fade, blend_density, sample_id, BorderAnalysis};
use crate::library_kernel::{surface_layering, LibraryKernel};
use crate::column::{ColumnCache, ColumnField, IdColumn};
use crate::column_eval::{ColumnEvaluator, ColumnSample, UpstreamGraphs};
use crate::context::EvalContext;
use crate::detail_eval::DetailEvaluator;
use crate::error::EvalResult;
use crate::eval::Evaluator;
use crate::EvalError;
use crate::feature::{ColumnProbe, FeatureSource};
use crate::field::{ScalarField, CHUNK_DIM};
use crate::foliage::ChunkFoliage;

/// Biome-border fade radius, in world columns: how far the density blend reaches
/// from a biome boundary. Larger widens the transition (and the cross-column
/// border scan). Could later become a per-biome parameter.
const FADE_RADIUS: i32 = 10;

/// How a biome assigns material to its (composed) density: a depth-layered cake
/// (`Layer`), a single material (`ConstantMaterial`), or - for any other material
/// chain - fall back to the biome's precomputed material field.
enum BiomeMaterialRule {
    /// Top-down `(material, thickness)` bands + fill below (see `surface_layering`).
    Layer { bands: Vec<(MaterialId, u32)>, fill: MaterialId },
    /// A single uniform material.
    Uniform(MaterialId),
    /// Fall back to the biome's precomputed material field (shifted).
    Field,
}

/// Derive a biome's [`BiomeMaterialRule`] from the node feeding its
/// `DensityOutput` material input (pin 1), so material can be reapplied to the
/// blended surface instead of read from a surface-relative precomputed field.
fn extract_material_rule(graph: &Graph, density_node: NodeId) -> BiomeMaterialRule {
    let Some(edge) = graph.edges.iter().find(|e| e.to.node == density_node && e.to.pin == 1) else {
        return BiomeMaterialRule::Field;
    };
    match graph.nodes.get(edge.from.node).map(|n| &n.kind) {
        Some(NodeKind::Layer(p)) => BiomeMaterialRule::Layer { bands: p.bands.clone(), fill: p.fill },
        Some(NodeKind::ConstantMaterial(p)) => BiomeMaterialRule::Uniform(p.material),
        _ => BiomeMaterialRule::Field,
    }
}

/// One biome's terrain graph, the biome id it renders, its located
/// `DensityOutput` (absent if the graph has none - such a biome produces air),
/// its material rule, and an optional `DetailGraph` producing the biome's foliage.
struct BiomeGraph {
    id: u16,
    graph: Graph,
    density_node: Option<NodeId>,
    material_rule: BiomeMaterialRule,
    /// Located `FluidOutput` terminal, if the biome authors water.
    fluid_node: Option<NodeId>,
    detail: Option<Graph>,
    params: BiomeParams,
}

/// One zone's biome-assignment graph, the zone id it governs, and its located
/// `ZoneOutput` (absent if the graph has none - such a zone assigns biome 0).
struct ZoneGraph {
    id: u16,
    graph: Graph,
    output: Option<NodeId>,
}

/// Per-column identity for one chunk: which zone each column is in, which biome
/// that zone assigns it, and - when asked for - the biome-border analysis the
/// terrain composite blends by.
///
/// Produced by one function so the cheap lookup and the full evaluation cannot
/// disagree about a column's biome.
#[derive(Default)]
struct ResolvedColumns {
    zone_ids: Option<Arc<IdColumn>>,
    biome_ids: Option<Arc<IdColumn>>,
    border: Option<BorderAnalysis>,
    /// The World graph's per-column cache, kept past this function so the
    /// downstream biome graphs can resolve `GraphRef(World)` reads against the
    /// same values the zone assignment used - rather than re-deriving them and
    /// risking a second answer.
    world_cache: ColumnCache,
}

/// What the column pipeline produced for one world column (roadmap §4.3).
///
/// Reported from the same `resolve_columns` and `analyze_borders` generation
/// runs on, not from a parallel derivation - a debug tool that computes its own
/// answer tells you what *it* thinks, and the two disagreeing is exactly the
/// bug you would be using it to find.
#[derive(Clone, Debug)]
pub struct ColumnReport {
    /// The world column inspected.
    pub world_x: i32,
    /// The world column inspected.
    pub world_z: i32,
    /// The chunk that owns it.
    pub chunk: IVec3,
    /// The World graph's named boundary outputs - its climate channels - sorted
    /// by name. Sorted because slotmap iteration order is unspecified, and rows
    /// that reshuffle between refreshes make a panel unreadable.
    pub climate: Vec<(String, f32)>,
    /// Zone assigned by the World graph.
    pub zone_id: u16,
    /// Biome assigned by that zone's graph.
    pub biome_id: u16,
    /// Distance to the nearest column of a different biome, capped at the fade
    /// radius.
    pub border_distance: f32,
    /// The biome at that distance - the one this column's density blends toward.
    pub border_neighbor: u16,
    /// The fade radius the distance is capped at, so the reader can tell "at the
    /// cap" from "far".
    pub fade_radius: f32,
    /// The **own biome's** density at each Y of this chunk window, as
    /// `(world_y, value)`. `None` where the density chain has no pointwise
    /// implementation - which is itself diagnostic, since that is exactly the
    /// condition that breaks chunk-Y seam material continuation.
    ///
    /// The *blended* density the composite actually used is not reported: it
    /// lives in a local inside `composite_terrain`, and threading it out for a
    /// debug tool is a change to a hot path worth making deliberately.
    pub density: Vec<(i32, Option<f32>)>,
    /// Topmost solid cell's world Y within this chunk window, if any.
    pub surface_y: Option<i32>,
    /// Composited voxels from the surface downward, at most ten - the material
    /// cake as generation produced it.
    pub voxels: Vec<(i32, MaterialId, ShapeId)>,
    /// Biome-authored **pond** surface world-Y, from a `FluidOutput` terminal.
    ///
    /// Named for the mechanism, not for "fluid": the global ocean is applied
    /// later at the storage boundary from the manifest's `sea_level`, and
    /// poured water is a runtime override on a resident chunk. Neither is here,
    /// and a field called `fluid_level` that reports only one of the three
    /// reads as a bug when it is empty.
    pub pond_level: Option<f32>,
    /// Which chunk the voxel rows were composited from.
    pub voxel_chunk_y: i32,
}


/// A top-down grid of zone and biome ids (roadmap §4.3, biome/zone map).
///
/// Sampled pointwise at absolute world columns, so covering a thousand world
/// units costs a grid of noise evaluations rather than a thousand chunks. That
/// is the tool's reason to exist: at the scale where "where are my biomes" is a
/// real question, generating the answer is not an option.
pub struct IdMap {
    /// Grid edge length, in samples.
    pub dim: usize,
    /// World column of sample `(0, 0)`.
    pub origin_x: i32,
    /// World column of sample `(0, 0)`.
    pub origin_z: i32,
    /// World units between samples.
    pub step: i32,
    /// Zone id per sample, row-major (`x + z * dim`).
    pub zone: Vec<u16>,
    /// Biome id per sample, row-major.
    pub biome: Vec<u16>,
}

/// Owns the graph set for one world - the World and Zone graphs, the biome
/// graphs, and the library registry - and evaluates them for a chunk.
///
/// The World and Zone graphs evaluate to per-column caches (climate channels +
/// zone / biome id assignment). Each biome graph produces a whole-chunk terrain;
/// [`WorldEvaluator::evaluate_chunk`] composites them per column by the Zone
/// graph's biome assignment (a hard cut at biome borders - density fade is not
/// applied to finished voxels). A single-biome world composites to that biome's
/// terrain unchanged.
pub struct WorldEvaluator {
    world: Graph,
    /// One entry per manifest zone. A column's zone id selects which of these
    /// assigns its biome.
    zones: Vec<ZoneGraph>,
    biomes: Vec<BiomeGraph>,
    libraries: LibraryGraphRegistry,
    /// Native kernel per library id. Separate from `libraries`, which carries
    /// only boundaries: one answers "what pins does this reference have", the
    /// other "what does it compute".
    kernels: HashMap<LibraryGraphId, LibraryKernel>,
    /// Vertical extent of the world, in chunks. The feature probe scans this
    /// span for a column's surface; a structure's anchor may sit in a different
    /// chunk-Y than the chunk deriving it.
    chunk_y_range: std::ops::Range<i32>,
}

impl WorldEvaluator {
    /// Build a harness around a single biome graph (id 0). with empty World/Zone
    /// graphs and no libraries. Validates every graph and the library references
    /// set, logging findings.
    pub fn new(biome: Graph) -> Self {
        let density_node = find_density_node(&biome);
        let material_rule =
            density_node.map_or(BiomeMaterialRule::Field, |n| extract_material_rule(&biome, n));
        let fluid_node = find_fluid_node(&biome);
        let this = Self {
            world: Graph::of_kind(GraphKind::World),
            zones: Vec::new(),
            biomes: vec![BiomeGraph {
                id: 0,
                graph: biome,
                density_node,
                material_rule,
                fluid_node,
                detail: None,
                params: BiomeParams::new(),
            }],
            libraries: LibraryGraphRegistry::new(),
            kernels: HashMap::new(),
            chunk_y_range: 0..4,
        };
        this.log_validation();
        this
    }

    /// Replace the World graph (builder-style).
    pub fn with_world(mut self, world: Graph) -> Self {
        self.world = world;
        self
    }

    /// Install a single zone as zone 0 (builder-style). A convenience over
    /// [`Self::with_zones`] for tests and single-zone worlds.
    pub fn with_zone(self, zone: Graph) -> Self {
        self.with_zones(vec![(0, zone)])
    }

    /// Replace the zone set (builder-style). Each entry is a `(zone id, graph)`
    /// pair; the graph's `ZoneOutput` is located now, and a zone without one
    /// assigns biome 0 to every column it governs.
    pub fn with_zones(mut self, zones: Vec<(u16, Graph)>) -> Self {
        self.zones = zones
            .into_iter()
            .map(|(id, graph)| {
                let output = find_zone_output(&graph);
                ZoneGraph { id, graph, output }
            })
            .collect();
        self
    }

    /// Replace the biome set (builder-style). Each entry is a `(biome id, graph)`
    /// pair; the graph's `DensityOutput` is located now (a biome without one
    /// produces air for its columns).
    pub fn with_biomes(mut self, biomes: Vec<(u16, Graph)>) -> Self {
        self.biomes = biomes
            .into_iter()
            .map(|(id, graph)| {
                let density_node = find_density_node(&graph);
                let material_rule = density_node
                    .map_or(BiomeMaterialRule::Field, |n| extract_material_rule(&graph, n));
                let fluid_node = find_fluid_node(&graph);
                BiomeGraph {
                    id,
                    graph,
                    density_node,
                    material_rule,
                    fluid_node,
                    detail: None,
                    params: BiomeParams::new(),
                }
            })
            .collect();
        self
    }

    /// Attach a `DetailGraph` to each biome by id (builder-style). Entries whose
    /// id has no matching biome are ignored; biomes without a detail graph
    /// produce no foliage.
    pub fn with_biome_details(mut self, details: Vec<(u16, Graph)>) -> Self {
        for (id, detail) in details {
            if let Some(bg) = self.biomes.iter_mut().find(|b| b.id == id) {
                bg.detail = Some(detail);
            }
        }
        self
    }

    /// Attach per-biome scalar parameters by id (builder-style). Entries whose id
    /// has no matching biome are ignored; a biome without an entry keeps an empty
    /// sidecar (every `BiomeParam` falls back to its node default).
    pub fn with_biome_params(mut self, params: Vec<(u16, BiomeParams)>) -> Self {
        for (id, p) in params {
            if let Some(bg) = self.biomes.iter_mut().find(|b| b.id == id) {
                bg.params = p;
            }
        }
        self
    }

    /// Replace the library registry (builder-style).
    pub fn with_libraries(mut self, libraries: LibraryGraphRegistry) -> Self {
        self.libraries = libraries;
        self
    }

    /// Install the native kernel bindings (builder-style).
    pub fn with_library_kernels(
        mut self,
        kernels: HashMap<LibraryGraphId, LibraryKernel>,
    ) -> Self {
        self.kernels = kernels;
        self
    }

    /// Finalize cross-graph dataflow: derive the World and Zone boundaries from
    /// their `GraphOutput` nodes, then resolve the Zone graph's `GraphRef` pins
    /// against those boundaries. Call once after the graphs are installed (it is
    /// a no-op when nothing references anything). Biome-graph cross-graph imports
    /// arrive with the biome density rework (Substep 7).
    pub fn with_cross_graph_resolved(mut self) -> Self {
        self.world.derive_output_boundary();
        let world_boundary = self.world.boundary.clone();
        // Library pins first: a `LibraryRef` with no pins cannot be wired, and
        // every graph tier may reference one.
        for bg in &mut self.biomes {
            bg.graph.resolve_library_refs(&self.libraries);
            if let Some(d) = bg.detail.as_mut() {
                d.resolve_library_refs(&self.libraries);
            }
        }
        self.world.resolve_library_refs(&self.libraries);
        for zg in &mut self.zones {
            zg.graph.resolve_library_refs(&self.libraries);
        }

        for zg in &mut self.zones {
            zg.graph.derive_output_boundary();
            // `GraphRefTarget::Zone` resolves to nothing: with several zones
            // there is no single "the zone", and a zone referencing itself is
            // not a meaningful read. Matches what `load_world_graphs` does on
            // the editor side, so the two cannot disagree about which refs
            // render pins.
            zg.graph.resolve_graph_refs(|t: GraphRefTarget| match t {
                GraphRefTarget::World => Some(world_boundary.clone()),
                _ => None,
            });
        }

        // Biome and detail graphs too. The editors `load_world_graphs` has
        // always resolved these, so a `GraphRef` dropped into a biome graph
        // rendered a pin, wired, and saved - and then failed that chunk at
        // generation, because this side resolved zone graphs only.
        for bg in &mut self.biomes {
            bg.graph.resolve_graph_refs(|t: GraphRefTarget| match t {
                GraphRefTarget::World => Some(world_boundary.clone()),
                _ => None,
            });
            if let Some(d) = bg.detail.as_mut() {
                d.resolve_graph_refs(|t: GraphRefTarget| match t {
                    GraphRefTarget::World => Some(world_boundary.clone()),
                    _ => None,
                });
            }
        }
        self
    }

    /// The primary (first) biome graph - used by the editor and single-biome
    /// worlds. The multi-graph editor selector chooses among biomes later.
    pub fn biome_graph(&self) -> &Graph {
        &self.biomes[0].graph
    }

    /// The World graph.
    pub fn world_graph(&self) -> &Graph {
        &self.world
    }

    /// Every zone graph, in manifest order.
    pub fn zone_graphs(&self) -> impl Iterator<Item = &Graph> + '_ {
        self.zones.iter().map(|z| &z.graph)
    }

    /// The library registry.
    pub fn libraries(&self) -> &LibraryGraphRegistry {
        &self.libraries
    }

    /// Set the world's vertical extent, which the feature probe scans.
    pub fn with_chunk_y_range(mut self, range: std::ops::Range<i32>) -> Self {
        self.chunk_y_range = range;
        self
    }

    /// The `NodeId` of the WorldGraph's terminal `WorldOutput`, if one exists.
    pub fn world_output_node(&self) -> Option<NodeId> {
        self.world
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::WorldOutput(_)))
            .map(|(id, _)| id)
    }


    /// Zone ids the World graph's terminal can assign.
    ///
    /// An empty `zone_ids` does **not** mean "zone 0": `band_id` falls back to
    /// the band index, so `zone_bands: [0.0]` with no id table assigns zones 0
    /// *and* 1. That default is harmless while nothing reads zone ids and
    /// silently reassigns half a world the moment something does.
    fn assignable_zone_ids(&self) -> Vec<u16> {
        let Some(node) = self.world_output_node() else {
            return vec![0];
        };
        let Some(NodeKind::WorldOutput(p)) = self.world.nodes.get(node).map(|n| &n.kind) else {
            return vec![0];
        };
        crate::column_eval::assignable_ids(p.zone_bands.len(), &p.zone_ids)
    }

    /// A biome's scalar parameter by name, if that biome exists and declares it.
    pub fn biome_param(&self, biome_id: u16, name: &str) -> Option<f32> {
        self.biomes
            .iter()
            .find(|b| b.id == biome_id)
            .and_then(|b| b.params.get(name))
    }

    /// Biome id at one absolute world column, sampled pointwise.
    ///
    /// Two graph walks. This replaced `biome_column`, which bulk-filled the
    /// World graph *and every zone graph* across a whole 32² chunk - 3,072
    /// column evaluations once a second zone existed - and then read a single
    /// cell out of the result. Callers that want a *region* of ids want
    /// [`Self::sample_id_map`], which amortizes the same setup across a grid.
    ///
    /// Chunk-independent, like every pointwise path here, so two callers asking
    /// about the same column get the same answer regardless of where they are.
    pub fn sample_biome_at(&self, world_seed: u64, wx: i32, wz: i32) -> EvalResult<u16> {
        Ok(self.sample_zone_and_biome_at(world_seed, wx, wz)?.1)
    }

    /// Both column identities in one pass, pointwise.
    ///
    /// Factored out rather than copied because the feature probe needs the zone
    /// as well as the biome, and two implementations of "which zone is this
    /// column" is two answers waiting to disagree.
    pub fn sample_zone_and_biome_at(
        &self,
        world_seed: u64,
        wx: i32,
        wz: i32,
    ) -> EvalResult<(u16, u16)> {
        if self.zones.is_empty() {
            return Ok((0, 0));
        }
        let ctx = EvalContext::new(world_seed, IVec3::ZERO);
        let world_point = ColumnEvaluator::new(&self.world, ctx);
        let zone = match self.world_output_node() {
            Some(n) => sample_id(&world_point, n, wx, wz)?,
            None => 0,
        };

        // `insert` wants a cache reference; the pointwise path never reads it.
        let empty = ColumnCache::new();
        let mut upstream = UpstreamGraphs::new();
        if !self.world.nodes.is_empty() {
            upstream.insert(GraphRefTarget::World, &self.world, ctx, &empty);
        }
        let Some(zg) = self.zones.iter().find(|z| z.id == zone) else {
            return Ok((zone, 0));
        };
        let Some(out) = zg.output else {
            return Ok((zone, 0));
        };
        let zone_point = ColumnEvaluator::new(&zg.graph, ctx).with_upstream(&upstream);
        Ok((zone, sample_id(&zone_point, out, wx, wz)?))
    }

    /// Resolve per-column zone and biome ids for one chunk, and optionally the
    /// biome-border analysis.
    ///
    /// The single implementation of "which biome is this column", shared by
    /// [`Self::biome_column`] and [`Self::evaluate_chunk`]. Two implementations
    /// would be two chances to disagree about a chunk's contents - and one of
    /// them feeds the climate simulation while the other feeds terrain.
    fn resolve_columns(&self, ctx: EvalContext, borders: bool) -> EvalResult<ResolvedColumns> {
        if self.world.nodes.is_empty() && self.zones.is_empty() {
            return Ok(ResolvedColumns::default());
        }

        // The World evaluator stays alive rather than being consumed into its
        // cache: the border scan samples it pointwise at world columns outside
        // this chunk, which `sample_column` does without any bulk fill.
        let mut world_eval = ColumnEvaluator::new(&self.world, ctx);
        if !self.world.nodes.is_empty() {
            world_eval.evaluate()?;
        }
        let world_output = self.world_output_node();

        let zone_ids: Option<Arc<IdColumn>> = world_output
            .and_then(|n| world_eval.cache().get(n))
            .and_then(|c| c.as_id())
            .cloned();

        // Zone assignment and biome assignment are independent questions. A
        // World graph can assign zones with no zone graph existing to turn them
        // into biomes — and reporting no zone ids in that case would empty
        // `ChunkTags.zones`, losing identity the World graph did compute.
        if self.zones.is_empty() {
            return Ok(ResolvedColumns {
                zone_ids,
                biome_ids: None,
                border: None,
                world_cache: world_eval.into_cache(),
            });
        }

        let mut upstream = UpstreamGraphs::new();
        if !self.world.nodes.is_empty() {
            upstream.insert(GraphRefTarget::World, &self.world, ctx, world_eval.cache());
        }

        // Every zone, not only those with a column in this chunk. The border
        // scan reaches FADE_RADIUS past the chunk and can meet a zone that has
        // no column inside it, so "present" is the wrong set - and computing
        // the right one costs a pointwise pass over the extended footprint,
        // which is more than evaluating a few small zone graphs.
        // Trigger: revisit if a world authors enough zones to show up in
        // `generate mean`.
        let mut zone_evals: Vec<(u16, Option<NodeId>, ColumnEvaluator)> =
            Vec::with_capacity(self.zones.len());
        for zg in &self.zones {
            let mut ev = ColumnEvaluator::new(&zg.graph, ctx).with_upstream(&upstream);
            if !zg.graph.nodes.is_empty() {
                ev.evaluate()?;
            }
            zone_evals.push((zg.id, zg.output, ev));
        }
        let zone_for = |zid: u16| zone_evals.iter().find(|(id, _, _)| *id == zid);

        // Per column: the zone says which graph assigns the biome.
        let mut biome = IdColumn::zeroed();
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let zid = zone_ids.as_ref().map_or(0, |c| c.get(x, z));
                if let Some((_, Some(out), ev)) = zone_for(zid) {
                    if let Some(ids) = ev.cache().get(*out).and_then(|c| c.as_id()) {
                        biome.set(x, z, ids.get(x, z));
                    }
                }
            }
        }

        let border = if borders {
            // Two levels: the zone at a world column, then that zone's biome.
            // Both pointwise and chunk-independent, so two chunks sharing a
            // border derive the same value for the same column.
            let biome_at_world = |wx: i32, wz: i32| -> EvalResult<u16> {
                let zid = match world_output {
                    Some(n) => sample_id(&world_eval, n, wx, wz)?,
                    None => 0,
                };
                match zone_for(zid) {
                    Some((_, Some(out), ev)) => sample_id(ev, *out, wx, wz),
                    // A zone id with no graph, or a zone graph with no terminal:
                    // biome 0, the same fallback an absent biome graph gets.
                    // Catching an unassignable zone id belongs in graph
                    // validation (Substep 10), not per column at generation time.
                    _ => Ok(0),
                }
            };
            Some(analyze_borders(biome_at_world, ctx.chunk, FADE_RADIUS)?)
        } else {
            None
        };

        // Both borrow the world cache and are finished with it; the cache
        // itself outlives this function on the returned value.
        drop(zone_evals);
        drop(upstream);
        Ok(ResolvedColumns {
            zone_ids,
            biome_ids: Some(Arc::new(biome)),
            border,
            world_cache: world_eval.into_cache(),
        })
    }

    /// `chunk_y_range` is the vertical span to report over, in chunk Y. The
    /// column pipeline is 2D and independent of chunk Y, so `ctx`'s own chunk Y
    /// is ignored; the voxel rows are taken from whichever chunk in the range
    /// holds the surface.
    ///
    /// Reporting one chunk window was wrong: a column's surface is wherever the
    /// biome puts it, and a window that happens not to contain it shows a slice
    /// with no relationship to what the author is looking at.
    pub fn inspect_column(
        &self,
        ctx: EvalContext,
        lx: usize,
        lz: usize,
        chunk_y_range: std::ops::Range<i32>,
    ) -> EvalResult<ColumnReport> {
        let cols = self.resolve_columns(ctx, true)?;
        // The column pipeline is chunk-Y independent, so one context serves the
        // several vertical windows this report walks.
        let upstream = self.upstream_for(ctx, &cols.world_cache);
        let dim = CHUNK_DIM as i32;
        let world_x = ctx.chunk.x * dim + lx as i32;
        let world_z = ctx.chunk.z * dim + lz as i32;

        // Pointwise: `sample_column` walks the graph directly, so the climate
        // channels need no bulk fill of their own.
        let world_point = ColumnEvaluator::new(&self.world, ctx);
        let mut climate: Vec<(String, f32)> = self
            .world
            .nodes
            .iter()
            .filter_map(|(id, node)| match &node.kind {
                NodeKind::GraphOutput(p) => match world_point.sample_column(id, world_x, world_z) {
                    Ok(ColumnSample::Surface(v)) => Some((p.name.clone(), v)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        climate.sort_by(|a, b| a.0.cmp(&b.0));

        let biome_id = cols.biome_ids.as_ref().map_or(0, |c| c.get(lx, lz));

        // One biome evaluation at the bottom of the range, then pointwise
        // sampling across the whole span: `sample_density` is world-position
        // arithmetic, so it reads above its own chunk window happily — which is
        // the same property the chunk-Y seam continuation relies on.
        let base_chunk_y = chunk_y_range.start;
        let span = (chunk_y_range.end - chunk_y_range.start).max(1) * dim;
        let base_y = base_chunk_y * dim;
        let base_ctx = EvalContext::new(
            ctx.world_seed,
            IVec3::new(ctx.chunk.x, base_chunk_y, ctx.chunk.z),
        );

        let mut density = Vec::new();
        let mut own_surface = None;
        if let Some(layer) = self.eval_biome_layer(base_ctx, biome_id, &upstream)? {
            for y in 0..span as usize {
                let v = layer.sample_density(lx, y, lz);
                if v.is_some_and(|d| d > 0.0) {
                    own_surface = Some(base_y + y as i32);
                }
                density.push((base_y + y as i32, v));
            }
        }

        // Composite only the chunk holding the surface. Compositing the whole
        // range would be several times the cost for rows nobody reads.
        let surface_chunk_y = own_surface
            .map(|y| y.div_euclid(dim))
            .unwrap_or(base_chunk_y)
            .clamp(chunk_y_range.start, chunk_y_range.end - 1);
        let voxel_ctx = EvalContext::new(
            ctx.world_seed,
            IVec3::new(ctx.chunk.x, surface_chunk_y, ctx.chunk.z),
        );
        let voxel_base_y = surface_chunk_y * dim;

        let biome_col = cols.biome_ids.clone();
        let terrain =
            self.composite_terrain(voxel_ctx, biome_col.as_deref(), cols.border.as_ref(), &upstream)?;
        let fluid = self.composite_fluid(voxel_ctx, biome_col.as_deref(), &upstream)?;

        let mut surface_y = None;
        let mut voxels = Vec::new();
        for y in (0..CHUNK_DIM).rev() {
            let v = terrain.get(lx, y, lz);
            if v == Voxel::EMPTY {
                continue;
            }
            if surface_y.is_none() {
                surface_y = Some(voxel_base_y + y as i32);
            }
            voxels.push((voxel_base_y + y as i32, v.material, v.shape));
            if voxels.len() >= 10 {
                break;
            }
        }

        let pond_level = fluid
            .as_ref()
            .map(|f| f.get(lx, lz))
            .filter(|v| *v != NO_POND);

        Ok(ColumnReport {
            world_x,
            world_z,
            chunk: ctx.chunk,
            climate,
            zone_id: cols.zone_ids.as_ref().map_or(0, |c| c.get(lx, lz)),
            biome_id,
            border_distance: cols.border.as_ref().map_or(0.0, |b| b.distance.get(lx, lz)),
            border_neighbor: cols.border.as_ref().map_or(0, |b| b.neighbor.get(lx, lz)),
            fade_radius: FADE_RADIUS as f32,
            density,
            surface_y,
            voxels,
            pond_level,
            voxel_chunk_y: surface_chunk_y,
        })
    }


    /// Sample zone and biome ids over a top-down grid, generating nothing.
    ///
    /// Uses the same `sample_id` path `resolve_columns` and `analyze_borders`
    /// use, so the map cannot disagree with the world it previews - a preview
    /// that derives its own answer previews something else.
    pub fn sample_id_map(
        &self,
        world_seed: u64,
        origin_x: i32,
        origin_z: i32,
        step: i32,
        dim: usize,
    ) -> EvalResult<IdMap> {
        let step = step.max(1);
        let ctx = EvalContext::new(world_seed, IVec3::ZERO);
        let world_point = ColumnEvaluator::new(&self.world, ctx);
        let world_output = self.world_output_node();

        // `insert` wants a cache reference; the pointwise path never reads it,
        // resampling the upstream graph instead. An empty one is honest.
        let empty = ColumnCache::new();
        let mut upstream = UpstreamGraphs::new();
        if !self.world.nodes.is_empty() {
            upstream.insert(GraphRefTarget::World, &self.world, ctx, &empty);
        }
        let zone_points: Vec<(u16, Option<NodeId>, ColumnEvaluator)> = self
            .zones
            .iter()
            .map(|zg| {
                (
                    zg.id,
                    zg.output,
                    ColumnEvaluator::new(&zg.graph, ctx).with_upstream(&upstream),
                )
            })
            .collect();

        let mut zone = Vec::with_capacity(dim * dim);
        let mut biome = Vec::with_capacity(dim * dim);
        for gz in 0..dim {
            for gx in 0..dim {
                let wx = origin_x + gx as i32 * step;
                let wz = origin_z + gz as i32 * step;
                let z = match world_output {
                    Some(n) => sample_id(&world_point, n, wx, wz)?,
                    None => 0,
                };
                let b = match zone_points.iter().find(|(id, _, _)| *id == z) {
                    Some((_, Some(out), ev)) => sample_id(ev, *out, wx, wz)?,
                    _ => 0,
                };
                zone.push(z);
                biome.push(b);
            }
        }
        Ok(IdMap { dim, origin_x, origin_z, step, zone, biome })
    }

    /// Evaluate the graph set for one chunk: World -> per-column zone ids, each
    /// zone -> its columns' biome ids, then composite the biome graphs' terrain
    /// by that assignment.
    pub fn evaluate_chunk(&self, ctx: EvalContext) -> EvalResult<ChunkEvaluation> {
        let cols = self.resolve_columns(ctx, true)?;
        let biome_col = cols.biome_ids.clone();
        // Built once and shared by every stage that evaluates a biome graph, so
        // a `GraphRef(World)` reads the same columns the zone assignment did.
        let upstream = self.upstream_for(ctx, &cols.world_cache);

        let terrain =
            self.composite_terrain(ctx, biome_col.as_deref(), cols.border.as_ref(), &upstream)?;
        // Stage 8 (structures) sits between smoothing (7) and fluid (9), per §5.
        let terrain = self.apply_structures(ctx, terrain, &upstream)?;
        // Stage 9 (fluid) precedes stage 10 (foliage) per design doc §5. Foliage
        // submersion is applied at the storage boundary (world_generator), where
        // the fully-composited fluid field is available; the order here matches
        // the documented stage sequence.
        let fluid_levels = self.composite_fluid(ctx, biome_col.as_deref(), &upstream)?;
        let foliage = self.evaluate_foliage(ctx, &terrain, biome_col.as_deref())?;

        Ok(ChunkEvaluation {
            terrain,
            zone_ids: cols.zone_ids,
            biome_ids: cols.biome_ids,
            foliage,
            fluid_levels,
        })
    }

    /// Run each present biome's `DetailGraph` (if any) against the composited
    /// terrain, restricting each to its own columns, and union the results.
    fn evaluate_foliage(
        &self,
        ctx: EvalContext,
        terrain: &ChunkBuffer<Voxel, 32>,
        biome_col: Option<&IdColumn>,
    ) -> EvalResult<ChunkFoliage> {
        let mut foliage = ChunkFoliage::default();
        for bid in present_biomes(biome_col) {
            let Some(bg) = self.biomes.iter().find(|b| b.id == bid) else { continue };
            let Some(detail) = &bg.detail else { continue };
            let mut de = DetailEvaluator::new(detail, ctx, terrain, biome_col, bid);
            foliage.merge(de.evaluate()?);
        }
        Ok(foliage)
    }

    /// Per-column pond water-surface level (world-Y) authored by present biomes'
    /// `FluidOutput` terminals. `None` when no present biome authors fluid;
    /// otherwise a column reads [`NO_POND`] where its biome places no pond.
    ///
    /// Each fluid biome is re-evaluated (a full biome eval) and its `FluidOutput`
    /// mask (cached as the node's value) + `level` input are read: where the mask
    /// is positive in that biome's columns, the pond level is that biome's `level`.
    fn composite_fluid(
        &self,
        ctx: EvalContext,
        biome_col: Option<&IdColumn>,
        upstream: &UpstreamGraphs,
    ) -> EvalResult<Option<Arc<ColumnField>>> {
        let biome_at = |x: usize, z: usize| biome_col.map_or(0u16, |c| c.get(x, z));
        let present = present_biomes(biome_col);
        if !present
            .iter()
            .any(|&b| self.biomes.iter().any(|bg| bg.id == b && bg.fluid_node.is_some()))
        {
            return Ok(None);
        }

        let mut levels = ColumnField::filled(NO_POND);
        for &bid in &present {
            let Some(bg) = self.biomes.iter().find(|b| b.id == bid) else { continue };
            let Some(fluid_node) = bg.fluid_node else { continue };
            let Some(level_src) = input_source(&bg.graph, fluid_node, 0) else { continue };

            let mut eval = Evaluator::new(&bg.graph, ctx)
                .with_biome_params(&bg.params)
                .with_library_kernels(&self.kernels)
                .with_upstream(upstream);
            eval.evaluate()?;
            let cache = eval.cache();
            // The FluidOutput node caches its `mask` field; read it at y=0 (the
            // mask chain is 2D). `level` is the scalar feeding pin 0.
            let Some(mask) = cache.get(fluid_node).and_then(|o| o.as_scalar()) else { continue };
            let Some(level) =
                cache.get(level_src).and_then(|o| o.as_scalar()).map(|f| f.get(0, 0, 0))
            else {
                continue;
            };

            for z in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    if biome_at(x, z) == bid && mask.get(x, 0, z) > 0.0 {
                        levels.set(x, z, level);
                    }
                }
            }
        }
        Ok(Some(Arc::new(levels)))
    }

    /// Composite each column's biome terrain into one chunk buffer. `biome_col`
    /// gives the biome id per `(x, z)` column; `None` (no Zone graph) means biome
    /// 0 everywhere.
    fn composite_terrain(
        &self,
        ctx: EvalContext,
        biome_col: Option<&IdColumn>,
        border: Option<&BorderAnalysis>,
        upstream: &UpstreamGraphs,
    ) -> EvalResult<Arc<ChunkBuffer<Voxel, 32>>> {
        let biome_at = |x: usize, z: usize| biome_col.map_or(0u16, |c| c.get(x, z));

        // Evaluate a layer for every biome we need: those present in this chunk
        // plus any that appear as a fade neighbor in the border analysis.
        let mut needed = present_biomes(biome_col);
        if let Some(b) = border {
            for &n in b.neighbor.data() {
                if !needed.contains(&n) {
                    needed.push(n);
                }
            }
        }
        let mut layers: HashMap<u16, LayerEval> =
            HashMap::new();
        for bid in needed {
            if let Some(layer) = self.eval_biome_layer(ctx, bid, upstream)? {
                layers.insert(bid, layer);
            }
        }

        // Per column: blend the own biome's density toward its nearest differing
        // neighbor by the fade weight (density interpolates). The own material is
        // surface-relative, so shift it to the *blended* surface - otherwise the
        // cells the blend raises above the biome's own surface would read as air
        // (material 0). Material stays the winning biome's (a sharp material cut).
        let mut out = ChunkBuffer::<Voxel, 32>::uniform(Voxel::EMPTY);
        let mut col = [0.0f32; CHUNK_DIM];
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let own = biome_at(x, z);
                let Some(own_layer) = layers.get(&own) else {
                    continue; // own biome has no layer -> air column
                };
                let (weight, neighbor) = match border {
                    Some(b) if b.neighbor.get(x, z) != own => (
                        biome_border_fade(b.distance.get(x, z), FADE_RADIUS as f32),
                        layers.get(&b.neighbor.get(x, z)),
                    ),
                    _ => (1.0, None),
                };
                // Blended density column + its surface (topmost solid cell).
                let mut composed_surface = None;
                for y in 0..CHUNK_DIM {
                    let od = own_layer.density.get(x, y, z);
                    col[y] = match neighbor {
                        Some(nl) => blend_density(od, nl.density.get(x, y, z), weight),
                        None => od,
                    };
                    if col[y] > 0.0 {
                        composed_surface = Some(y);
                    }
                }
                let Some(cs) = composed_surface else { continue };
                // Material comes from the WINNING biome's rule, reapplied at the
                // depth below the blended surface `cs` - so it stays consistent
                // with the biome the column belongs to, never the fade neighbor's.
                let own_rule = self.biomes.iter().find(|b| b.id == own).map(|b| &b.material_rule);
                // Chunk-Y seam depth continuation: a solid window-top cell may be
                // buried - the true surface can lie in the chunk above, and
                // measuring band depth from the window top paints surface
                // material (grass strata) along every chunk-Y seam. Sample the
                // blended density above the window, up to the material cake's
                // total band depth (anything deeper is fill regardless), and
                // push banding depth down by the continuation. Unsampleable
                // density chains fall back to the pre-continuation behavior.
                let max_above = match own_rule {
                    Some(BiomeMaterialRule::Layer { bands, .. }) => {
                        bands.iter().map(|&(_, thickness)| thickness).sum::<u32>()
                    }
                    _ => 0,
                };
                let mut above = 0u32;
                if cs == CHUNK_DIM - 1 && max_above > 0 {
                    while above < max_above {
                        let ay = CHUNK_DIM + above as usize;
                        let Some(od) = own_layer.sample_density(x, ay, z) else { break };
                        let d = match neighbor {
                            Some(nl) => match nl.sample_density(x, ay, z) {
                                Some(nd) => blend_density(od, nd, weight),
                                None => od,
                            },
                            None => od,
                        };
                        if d <= 0.0 {
                            break;
                        }
                        above += 1;
                    }
                }
                let own_surface =
                    (0..CHUNK_DIM).rev().find(|&y| own_layer.density.get(x, y, z) > 0.0);
                for y in 0..CHUNK_DIM {
                    if col[y] > 0.0 {
                        let depth = (cs - y) as u32 + above;
                        let material = match own_rule {
                            Some(BiomeMaterialRule::Layer { bands, fill }) => {
                                surface_layering(depth, bands, *fill)
                            }
                            Some(BiomeMaterialRule::Uniform(m)) => *m,
                            _ => {
                                // Fallback for other material chains: shift the
                                // precomputed surface-relative field. (Window-
                                // relative; a Field-rule biome still bands from
                                // the window top at chunk-Y seams.)
                                let shift = own_surface.map_or(0, |os| os as i32 - cs as i32);
                                let src = (y as i32 + shift).clamp(0, CHUNK_DIM as i32 - 1) as usize;
                                own_layer.material.get(x, src, z)
                            }
                        };
                        out.set(x, y, z, Voxel { shape: ShapeId::Cube, material, flags: 0 });
                    }
                }
            }
        }
        out.try_collapse();
        Ok(Arc::new(out))
    }

    /// Collect every `PlaceStructure` declaration across the zone graphs.
    ///
    /// The zone comes from the graph the node lives in (§5 puts structures on
    /// the ZoneGraph), so a source cannot claim a zone it was not authored in.
    ///
    /// Sorted by content rather than left in slotmap order: the source index is
    /// the last tiebreak in `derive_features`'s ordering, and determinism must
    /// not rest on iteration order happening to be stable.
    fn structure_sources(&self) -> EvalResult<Vec<FeatureSource>> {
        let mut out = Vec::new();
        for zone in &self.zones {
            for (id, node) in zone.graph.nodes.iter() {
                let NodeKind::PlaceStructure(p) = &node.kind else { continue };
                // Not a skip: a source with no template places nothing and says
                // nothing, which is how a wrong world looks like a working one.
                let blueprint = p
                    .resolved
                    .as_ref()
                    .ok_or(EvalError::UnresolvedBlueprint { node: id })?;
                out.push(FeatureSource {
                    cell_size: p.cell_size,
                    density: p.density,
                    seed: p.seed,
                    zones: vec![zone.id],
                    random_yaw: p.random_yaw,
                    surface_offset: p.surface_offset,
                    blueprint: blueprint.clone(),
                });
            }
        }
        out.sort_by(|a, b| {
            (a.zones.first().copied(), a.seed, a.cell_size, &a.blueprint.name)
                .cmp(&(b.zones.first().copied(), b.seed, b.cell_size, &b.blueprint.name))
        });
        Ok(out)
    }

    /// Zone and surface height for one world column, sampled **pointwise**.
    ///
    /// This is the load-bearing property of the whole feature model: both facts
    /// are functions of world position, so every chunk that derives a feature
    /// gets the same answer. A read of a neighboring chunk's voxels here would
    /// reintroduce exactly the order dependence §5 rules out.
    ///
    /// The surface scan mirrors `inspect_column`'s: one biome evaluation at the
    /// bottom of the range, then pointwise density sampling across the span.
    fn probe_column(
        &self,
        world_seed: u64,
        wx: i32,
        wz: i32,
        chunk_y_range: std::ops::Range<i32>,
        upstream: &UpstreamGraphs,
    ) -> EvalResult<Option<ColumnProbe>> {
        let (zone_id, biome_id) = self.sample_zone_and_biome_at(world_seed, wx, wz)?;
        let dim = CHUNK_DIM as i32;
        let ctx = EvalContext::new(
            world_seed,
            IVec3::new(wx.div_euclid(dim), chunk_y_range.start, wz.div_euclid(dim)),
        );
        let (lx, lz) = (wx.rem_euclid(dim) as usize, wz.rem_euclid(dim) as usize);
        let Some(layer) = self.eval_biome_layer(ctx, biome_id, upstream)? else {
            return Ok(None);
        };
        let span = ((chunk_y_range.end - chunk_y_range.start).max(1) * dim) as usize;
        let base_y = chunk_y_range.start * dim;
        let mut surface_y = None;
        for y in 0..span {
            if layer.sample_density(lx, y, lz).is_some_and(|d| d > 0.0) {
                surface_y = Some(base_y + y as i32);
            }
        }
        Ok(surface_y.map(|surface_y| ColumnProbe { zone_id, surface_y }))
    }

    /// Stage 8: derive the features reaching this chunk and stamp their
    /// in-window parts.
    ///
    /// Returns the terrain untouched when no source exists, which is what keeps
    /// this inert - and the generation hash unchanged - until content declares a
    /// structure.
    fn apply_structures(
        &self,
        ctx: EvalContext,
        terrain: Arc<ChunkBuffer<Voxel, 32>>,
        upstream: &UpstreamGraphs,
    ) -> EvalResult<Arc<ChunkBuffer<Voxel, 32>>> {
        let sources = self.structure_sources()?;
        if sources.is_empty() {
            return Ok(terrain);
        }
        let range = self.chunk_y_range.clone();
        let mut probe_err = None;
        let features = crate::feature::derive_features(ctx, &sources, |wx, wz| {
            if probe_err.is_some() {
                return None;
            }
            match self.probe_column(ctx.world_seed, wx, wz, range.clone(), upstream) {
                Ok(p) => p,
                Err(e) => {
                    probe_err = Some(e);
                    None
                }
            }
        });
        if let Some(e) = probe_err {
            return Err(e);
        }
        Ok(Arc::new(crate::feature::stamp_features(
            &terrain, ctx, &sources, &features
        )))
    }

    /// Evaluate one biome graph's (density, material) layer, or `None` if the
    /// biome id has no graph or the graph has no `DensityOutput`. The evaluator
    /// is kept alive on the returned [`LayerEval`] so the composite can
    /// pointwise-sample density above the chunk window (chunk-Y seam depth).
    fn eval_biome_layer<'u>(
        &'u self,
        ctx: EvalContext,
        bid: u16,
        upstream: &'u UpstreamGraphs<'u>,
    ) -> EvalResult<Option<LayerEval<'u>>> {
        let Some(bg) = self.biomes.iter().find(|b| b.id == bid) else {
            return Ok(None);
        };
        let Some(density_node) = bg.density_node else {
            return Ok(None);
        };
        let mut eval = Evaluator::new(&bg.graph, ctx)
            .with_biome_params(&bg.params)
            .with_library_kernels(&self.kernels)
            .with_upstream(upstream);
        eval.evaluate()?;
        let fields = eval
            .cache()
            .get(density_node)
            .and_then(|out| out.as_biome_layer())
            .map(|(d, m)| (d.clone(), m.clone()));
        Ok(fields.map(|(density, material)| LayerEval {
            density,
            material,
            density_src: input_source(&bg.graph, density_node, 0),
            eval,
        }))
    }

    /// Cross-graph context for one chunk: the World graph's boundary outputs,
    /// readable by a `GraphRef(World)` in any downstream graph. Empty when there
    /// is no World graph, which leaves such a reference an `UnresolvedGraphRef`
    /// rather than silently reading zero.
    fn upstream_for<'u>(&'u self, ctx: EvalContext, cache: &'u ColumnCache) -> UpstreamGraphs<'u> {
        let mut up = UpstreamGraphs::new();
        if !self.world.nodes.is_empty() {
            up.insert(GraphRefTarget::World, &self.world, ctx, cache);
        }
        up
    }

    /// Validate each graph + the library reference set, logging findings.
    fn log_validation(&self) {
        let mut graphs: Vec<(String, &Graph)> = vec![("world".to_string(), &self.world)];
        for zg in &self.zones {
            graphs.push((format!("zone[{}]", zg.id), &zg.graph));
        }
        for bg in &self.biomes {
            graphs.push((format!("biome[{}]", bg.id), &bg.graph));
        }
        for (label, graph) in graphs {
            for d in graph.validate() {
                match d.severity {
                    Severity::Error => log::error!("{label} graph validation: {}", d.message),
                    Severity::Warning => log::warn!("{label} graph validation: {}", d.message),
                    Severity::Info => log::info!("{label} graph validation: {}", d.message),
                }
            }
        }
        if let Some(cycle) = self.libraries.detect_cycle() {
            log::error!("library graphs contain a reference cycle: {cycle:?}");
        }
        if !self.zones.is_empty() {
            for zid in self.assignable_zone_ids() {
                if !self.zones.iter().any(|z| z.id == zid) {
                    log::error!(
                        "WorldGraph assigns zone {zid}, which no manifest zone \
                         implements; every column in it falls back to biome 0",
                    );
                }
            }
        }
    }
}

/// One biome's evaluated (density, material) layer, kept alive with its
/// evaluator so the composite can pointwise-sample density above the chunk
/// window (chunk-Y seam depth continuation).
struct LayerEval<'g> {
    density: Arc<ScalarField>,
    material: Arc<ChunkBuffer<MaterialId, 32>>,
    /// Root of the density expression feeding the `DensityOutput` (pin 0),
    /// for pull-based sampling outside the filled window.
    density_src: Option<NodeId>,
    eval: Evaluator<'g>,
}

impl LayerEval<'_> {
    /// Pointwise density at chunk-local coords (which may lie above the filled
    /// window, e.g. `y >= CHUNK_DIM`). `None` when the density chain has no
    /// single source or contains nodes the pointwise sampler doesn't support -
    /// callers treat that as "unknown, assume air" (the pre-continuation
    /// behavior).
    fn sample_density(&self, x: usize, y: usize, z: usize) -> Option<f32> {
        self.density_src.and_then(|n| self.eval.sample_density(n, x, y, z).ok())
    }
}

/// Locate a graph's `ZoneOutput` terminal, if any.
fn find_zone_output(graph: &Graph) -> Option<NodeId> {
    graph
        .nodes
        .iter()
        .find(|(_, n)| matches!(n.kind, NodeKind::ZoneOutput(_)))
        .map(|(id, _)| id)
}

/// Locate a graph's `DensityOutput` terminal, if any.
fn find_density_node(graph: &Graph) -> Option<NodeId> {
    graph
        .nodes
        .iter()
        .find(|(_, n)| matches!(n.kind, NodeKind::DensityOutput(_)))
        .map(|(id, _)| id)
}

/// Sentinel for "no pond in this column" in a fluid-level field.
pub const NO_POND: f32 = f32::MIN;

/// Locate a graph's `FluidOutput` terminal, if any.
fn find_fluid_node(graph: &Graph) -> Option<NodeId> {
    graph
        .nodes
        .iter()
        .find(|(_, n)| matches!(n.kind, NodeKind::FluidOutput(_)))
        .map(|(id, _)| id)
}

/// The node feeding `(node, pin)`, if any.
fn input_source(graph: &Graph, node: NodeId, pin: u16) -> Option<NodeId> {
    graph.edges.iter().find(|e| e.to.node == node && e.to.pin == pin).map(|e| e.from.node)
}

/// Distinct biome ids present across a chunk's columns (`None` => `[0]`).
fn present_biomes(biome_col: Option<&IdColumn>) -> Vec<u16> {
    let biome_at = |x: usize, z: usize| biome_col.map_or(0u16, |c| c.get(x, z));
    let mut present: Vec<u16> = Vec::new();
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            let b = biome_at(x, z);
            if !present.contains(&b) {
                present.push(b);
            }
        }
    }
    present
}

/// The result of evaluating the graph set for one chunk: the composited terrain,
/// the World and Zone per-column caches, and the composited foliage.
pub struct ChunkEvaluation {
    /// Composited chunk terrain (per-column biome selection).
    pub terrain: Arc<ChunkBuffer<Voxel, 32>>,
    /// Per-column zone id. `None` when no zone graphs are installed.
    ///
    /// Resolved ids rather than raw caches: with several zone graphs there is no
    /// single cache a consumer could look a biome up in, and requiring one to
    /// know which node in which graph produced the answer is what made the old
    /// shape single-zone.
    pub zone_ids: Option<Arc<IdColumn>>,
    /// Per-column biome id, resolved through each column's zone.
    pub biome_ids: Option<Arc<IdColumn>>,
    /// Per-biome foliage (paint + scatter), unioned across the chunk's biomes.
    /// Empty when no biome has a `DetailGraph`.
    pub foliage: ChunkFoliage,
    /// Per-column pond water-surface level (world-Y), or `None` when no biome
    /// authors fluid. Columns without a pond read [`NO_POND`].
    pub fluid_levels: Option<Arc<ColumnField>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;
    use nodegraph_ir::{ConstantMaterialParams, ConstantParams, DensityOutputParams, LayerParams, NoiseParams, PinRef, YBandParams, ZoneOutputParams};

    /// A biome graph filling the chunk with uniform `density` + a constant
    /// material: Constant -> BuildTerrain <- ConstantMaterial -> TerrainOutput.
    fn terrain_graph(density: f32) -> Graph {
        let mut g = Graph::new();
        let d = g.add_node(NodeKind::Constant(ConstantParams { value: density }));
        let m = g.add_node(NodeKind::ConstantMaterial(ConstantMaterialParams::default()));
        let out = g.add_node(NodeKind::DensityOutput(DensityOutputParams::default()));
        g.connect(PinRef::new(d, 0), PinRef::new(out, 0)).unwrap(); // Scalar -> Density coercion
        g.connect(PinRef::new(m, 0), PinRef::new(out, 1)).unwrap();
        g
    }

    fn terrain_graph_mat(density: f32, material: MaterialId) -> Graph {
        let mut g = Graph::new();
        let d = g.add_node(NodeKind::Constant(ConstantParams { value: density }));
        let m = g.add_node(NodeKind::ConstantMaterial(ConstantMaterialParams { material }));
        let out = g.add_node(NodeKind::DensityOutput(DensityOutputParams::default()));
        g.connect(PinRef::new(d, 0), PinRef::new(out, 0)).unwrap();
        g.connect(PinRef::new(m, 0), PinRef::new(out, 1)).unwrap();
        g
    }

    /// A Zone graph splitting columns into biomes 0/1 by a climate band at 0.
    fn zone_split() -> Graph {
        let mut g = Graph::of_kind(GraphKind::Zone);
        let noise = g.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let out = g.add_node(NodeKind::ZoneOutput(ZoneOutputParams { biome_bands: vec![0.0], ..Default::default() }));
        g.connect(PinRef::new(noise, 0), PinRef::new(out, 0)).unwrap();
        g
    }

    /// The expected fused terrain of a constant-density `terrain_graph`: solid
    /// cubes of the constant material where density > 0, else air.
    fn expected_terrain(density: f32) -> Arc<ChunkBuffer<Voxel, 32>> {
        let voxel = if density > 0.0 {
            Voxel {
                shape: ShapeId::Cube,
                material: ConstantMaterialParams::default().material,
                flags: 0,
            }
        } else {
            Voxel::EMPTY
        };
        Arc::new(ChunkBuffer::uniform(voxel))
    }

    #[test]
    fn world_and_zones_are_empty_by_default() {
        let we = WorldEvaluator::new(terrain_graph(1.0));
        assert_eq!(we.world_graph().kind, GraphKind::World);
        assert!(we.world_graph().nodes.is_empty());
        assert_eq!(we.zone_graphs().count(), 0);
        assert!(we.libraries().is_empty());
    }

    #[test]
    fn single_biome_composites_to_that_biome() {
        let ctx = EvalContext::new(0, IVec3::ZERO);
        let eval = WorldEvaluator::new(terrain_graph(1.0)).evaluate_chunk(ctx).unwrap();
        let expected = expected_terrain(1.0);
        for i in 0..ChunkBuffer::<Voxel, 32>::VOLUME {
            assert_eq!(eval.terrain.get_index(i), expected.get_index(i));
        }
        assert!(eval.zone_ids.is_none());
        assert!(eval.biome_ids.is_none());
    }

    #[test]
    fn composite_selects_winning_biome_material() {
        // Two solid biomes with different materials: the density blend keeps every
        // column solid, and the material is a sharp per-column winner.
        let zone = zone_split();
        let b0 = terrain_graph_mat(5.0, MaterialId(1));
        let b1 = terrain_graph_mat(5.0, MaterialId(2));
        let ctx = EvalContext::new(7, IVec3::new(1, 0, 1));
        let we = WorldEvaluator::new(b0.clone())
            .with_zone(zone.clone())
            .with_biomes(vec![(0, b0.clone()), (1, b1.clone())]);
        let eval = we.evaluate_chunk(ctx).unwrap();

        // Located from the graph rather than read off `eval`, so the expected
        // biome ids stay an independent derivation rather than a restatement of
        // what the composite already used.
        let zone_node = find_zone_output(&zone).unwrap();
        let mut ze = ColumnEvaluator::new(&zone, ctx);
        ze.evaluate().unwrap();
        let biome_ids = ze.into_cache().get(zone_node).unwrap().as_id().unwrap().clone();

        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let expected = if biome_ids.get(x, z) == 0 { MaterialId(1) } else { MaterialId(2) };
                let v = eval.terrain.get(x, 0, z);
                assert_ne!(v, Voxel::EMPTY, "both biomes solid -> every column solid");
                assert_eq!(v.material, expected, "material follows the winning biome");
            }
        }
    }

    #[test]
    fn world_graph_populates_zone_column() {
        use nodegraph_ir::WorldOutputParams;
        let mut world = Graph::of_kind(GraphKind::World);
        let noise = world.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let out = world.add_node(NodeKind::WorldOutput(WorldOutputParams::default()));
        world.connect(PinRef::new(noise, 0), PinRef::new(out, 0)).unwrap();

        let we = WorldEvaluator::new(terrain_graph(1.0)).with_world(world);
        let eval = we.evaluate_chunk(EvalContext::new(0, IVec3::ZERO)).unwrap();
        let ids = eval.zone_ids.expect("a World graph with a WorldOutput assigns zones");
        assert!(ids.data().iter().all(|&z| z == 0));
    }

    #[test]
    fn zone_graph_populates_biome_column() {
        let mut zone = Graph::of_kind(GraphKind::Zone);
        let noise = zone.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let out = zone.add_node(NodeKind::ZoneOutput(ZoneOutputParams::default()));
        zone.connect(PinRef::new(noise, 0), PinRef::new(out, 0)).unwrap();

        let we = WorldEvaluator::new(terrain_graph(1.0)).with_zone(zone);
        let eval = we.evaluate_chunk(EvalContext::new(0, IVec3::ZERO)).unwrap();
        let ids = eval.biome_ids.expect("a zone graph assigns biomes");
        assert!(ids.data().iter().all(|&b| b == 0));
    }

    #[test]
    fn no_detail_graph_produces_empty_foliage() {
        let we = WorldEvaluator::new(terrain_graph(1.0));
        let eval = we.evaluate_chunk(EvalContext::new(0, IVec3::ZERO)).unwrap();
        assert!(eval.foliage.is_empty());
    }

    #[test]
    fn biome_detail_graph_produces_foliage() {
        use nodegraph_ir::{PoissonDistributionParams, ScatterPlaceParams, SpeciesPickerParams};

        // A scatter DetailGraph: Poisson -> SpeciesPicker -> ScatterPlace.
        let mut detail = Graph::of_kind(GraphKind::Detail);
        let pois = detail.add_node(NodeKind::PoissonDistribution(PoissonDistributionParams {
            seed: 1,
            radius: 8.0,
            jitter: 0.5,
        }));
        let pick = detail.add_node(NodeKind::SpeciesPicker(SpeciesPickerParams::default()));
        let place = detail.add_node(NodeKind::ScatterPlace(ScatterPlaceParams {
            seed: 2,
            type_id: 0,
            prefab_id: 5,
        }));
        detail.connect(PinRef::new(pois, 0), PinRef::new(pick, 0)).unwrap();
        detail.connect(PinRef::new(pick, 0), PinRef::new(place, 0)).unwrap();

        // terrain_graph(1.0) is fully solid -> every column has a surface.
        let we = WorldEvaluator::new(terrain_graph(1.0)).with_biome_details(vec![(0, detail)]);
        let eval = we.evaluate_chunk(EvalContext::new(7, IVec3::ZERO)).unwrap();
        assert!(!eval.foliage.scatter.is_empty());
        assert!(!eval.foliage.scatter[0].instances.is_empty());
    }

    #[test]
    fn world_climate_drives_zone_biome_assignment() {
        use nodegraph_ir::{GraphOutputParams, GraphRefParams, GraphRefTarget};

        // World: SurfaceNoise -> GraphOutput("climate").
        let mut world = Graph::of_kind(GraphKind::World);
        let wn = world.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let wo = world.add_node(NodeKind::GraphOutput(GraphOutputParams { name: "climate".into() }));
        world.connect(PinRef::new(wn, 0), PinRef::new(wo, 0)).unwrap();
        world.derive_output_boundary();

        // Zone: GraphRef(World).climate -> ZoneOutput(one band). No own SurfaceNoise.
        let mut zone = Graph::of_kind(GraphKind::Zone);
        let gr = zone.add_node(NodeKind::GraphRef(GraphRefParams {
            target: GraphRefTarget::World,
            ..Default::default()
        }));
        let zo = zone.add_node(NodeKind::ZoneOutput(ZoneOutputParams { biome_bands: vec![0.0], ..Default::default() }));
        let wb = world.boundary.clone();
        zone.resolve_graph_refs(|t| (t == GraphRefTarget::World).then(|| wb.clone()));
        zone.connect(PinRef::new(gr, 0), PinRef::new(zo, 0)).unwrap();

        let b0 = terrain_graph(1.0); // solid
        let b1 = terrain_graph(-1.0); // air
        let ctx = EvalContext::new(9, IVec3::new(2, 0, -1));

        let we = WorldEvaluator::new(b0.clone())
            .with_world(world.clone())
            .with_zone(zone)
            .with_biomes(vec![(0, b0.clone()), (1, b1.clone())])
            .with_cross_graph_resolved();
        let eval = we.evaluate_chunk(ctx).unwrap();

        // The zone's biome column must equal quantizing World's climate directly.
        let biome_ids = eval.biome_ids.clone().expect("a zone graph assigns biomes");
        let mut wc = ColumnEvaluator::new(&world, ctx);
        wc.evaluate().unwrap();
        let climate = wc.cache().get(wo).unwrap().as_surface().unwrap();
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let expect = if climate.get(x, z) >= 0.0 { 1u16 } else { 0 };
                assert_eq!(biome_ids.get(x, z), expect, "biome must follow World's climate");
            }
        }
    }

    #[test]
    fn biome_param_threads_into_biome_terrain() {
        use nodegraph_ir::BiomeParamParams;
        // Biome graph: BiomeParam("solid") drives density into BuildTerrain.
        let mut biome = Graph::new();
        let bp = biome.add_node(NodeKind::BiomeParam(BiomeParamParams { name: "solid".into(), default: -1.0 }));
        let m = biome.add_node(NodeKind::ConstantMaterial(ConstantMaterialParams::default()));
        let out = biome.add_node(NodeKind::DensityOutput(DensityOutputParams::default()));
        biome.connect(PinRef::new(bp, 0), PinRef::new(out, 0)).unwrap(); // Scalar -> Density coercion
        biome.connect(PinRef::new(m, 0), PinRef::new(out, 1)).unwrap();

        let ctx = EvalContext::new(0, IVec3::ZERO);

        // Default (-1) -> all air.
        let air = WorldEvaluator::new(biome.clone()).evaluate_chunk(ctx).unwrap();
        assert!(
            (0..ChunkBuffer::<Voxel, 32>::VOLUME).all(|i| air.terrain.get_index(i) == Voxel::EMPTY),
            "default biome param should leave the chunk air"
        );

        // Sidecar solid=1 -> solid terrain.
        let mut params = BiomeParams::new();
        params.set("solid", 1.0);
        let solid = WorldEvaluator::new(biome.clone())
            .with_biome_params(vec![(0, params)])
            .evaluate_chunk(ctx)
            .unwrap();
        assert!(
            (0..ChunkBuffer::<Voxel, 32>::VOLUME).any(|i| solid.terrain.get_index(i) != Voxel::EMPTY),
            "biome param should thread through to solid terrain"
        );
    }

    #[test]
    fn biome_param_reads_the_right_biome() {
        let mut p0 = BiomeParams::new();
        p0.set("d", 3.0);
        let mut p1 = BiomeParams::new();
        p1.set("d", 0.0);
        let we = WorldEvaluator::new(terrain_graph(1.0))
            .with_biomes(vec![(0, terrain_graph(1.0)), (1, terrain_graph(1.0))])
            .with_biome_params(vec![(0, p0), (1, p1)]);
        assert_eq!(we.biome_param(0, "d"), Some(3.0));
        assert_eq!(we.biome_param(1, "d"), Some(0.0));
        assert_eq!(we.biome_param(0, "missing"), None);
        assert_eq!(we.biome_param(9, "d"), None);
    }

    #[test]
    fn fluid_output_produces_pond_levels() {
        use nodegraph_ir::{ConstantParams, FluidOutputParams, PinRef};
        let mut biome = Graph::new();
        let level = biome.add_node(NodeKind::Constant(ConstantParams { value: 30.0 }));
        let mask = biome.add_node(NodeKind::Constant(ConstantParams { value: 1.0 }));
        let fout = biome.add_node(NodeKind::FluidOutput(FluidOutputParams::default()));
        biome.connect(PinRef::new(level, 0), PinRef::new(fout, 0)).unwrap();
        biome.connect(PinRef::new(mask, 0), PinRef::new(fout, 1)).unwrap();

        let we = WorldEvaluator::new(biome);
        let a = we.evaluate_chunk(EvalContext::new(0, IVec3::ZERO)).unwrap();
        let levels = a.fluid_levels.expect("pond field present");
        assert_eq!(levels.get(0, 0), 30.0);
        assert_eq!(levels.get(31, 31), 30.0);
        // Deterministic.
        let b = we.evaluate_chunk(EvalContext::new(0, IVec3::ZERO)).unwrap();
        assert_eq!(b.fluid_levels.unwrap().get(5, 5), 30.0);
    }
    const T_GRASS: MaterialId = MaterialId(6);
    const T_SOIL: MaterialId = MaterialId(3);
    const T_STONE: MaterialId = MaterialId(1);

    /// A biome solid below world `max_y` (YBand) with a grass/soil/stone cake,
    /// for chunk-Y seam banding tests.
    fn banded_terrain_graph(max_y: f32) -> Graph {
        let mut g = Graph::new();
        let band = g.add_node(NodeKind::YBand(YBandParams { min: -1024.0, max: max_y }));
        let layer = g.add_node(NodeKind::Layer(LayerParams {
            bands: vec![(T_GRASS, 1), (T_SOIL, 3)],
            fill: T_STONE,
        }));
        let out = g.add_node(NodeKind::DensityOutput(DensityOutputParams::default()));
        g.connect(PinRef::new(band, 0), PinRef::new(layer, 0)).unwrap(); // Layer reads the density
        g.connect(PinRef::new(band, 0), PinRef::new(out, 0)).unwrap();
        g.connect(PinRef::new(layer, 0), PinRef::new(out, 1)).unwrap();
        g
    }

    fn material_at(eval: &ChunkEvaluation, x: usize, y: usize, z: usize) -> MaterialId {
        eval.terrain.get(x, y, z).material
    }

    /// A column whose terrain continues into the chunk above must band its
    /// window-top cells by depth from the TRUE surface (in the chunk above),
    /// not paint surface material at the chunk-Y seam (the grass-strata bug).
    #[test]
    fn chunk_y_seam_bands_depth_from_true_surface() {
        // Surface at world y=39: chunk (0,0,0)'s window (0..32) is entirely
        // buried - its top cell is 8 deep, well past the 4-cell cake -> stone.
        let we = WorldEvaluator::new(banded_terrain_graph(40.0));
        let below = we.evaluate_chunk(EvalContext::new(7, IVec3::new(0, 0, 0))).unwrap();
        assert_eq!(material_at(&below, 5, 31, 5), T_STONE, "buried window top is fill");
        assert_eq!(material_at(&below, 5, 30, 5), T_STONE);

        // The chunk above (window 32..64) holds the real surface at local y=7:
        // grass there, soil for the 3 cells beneath, stone below - unchanged.
        let above = we.evaluate_chunk(EvalContext::new(7, IVec3::new(0, 1, 0))).unwrap();
        assert_eq!(material_at(&above, 5, 7, 5), T_GRASS, "true surface keeps grass");
        assert_eq!(material_at(&above, 5, 6, 5), T_SOIL);
        assert_eq!(material_at(&above, 5, 4, 5), T_SOIL);
        assert_eq!(material_at(&above, 5, 3, 5), T_STONE);
    }

    /// A surface genuinely at the window top (air in the chunk above) keeps its
    /// surface bands - the continuation only engages when terrain continues.
    #[test]
    fn exposed_window_top_keeps_surface_bands() {
        // Surface at world y=31: the window-top cell IS the surface.
        let we = WorldEvaluator::new(banded_terrain_graph(32.0));
        let eval = we.evaluate_chunk(EvalContext::new(7, IVec3::new(0, 0, 0))).unwrap();
        assert_eq!(material_at(&eval, 5, 31, 5), T_GRASS, "exposed top stays grass");
        assert_eq!(material_at(&eval, 5, 30, 5), T_SOIL);
        assert_eq!(material_at(&eval, 5, 27, 5), T_STONE);
    }

    /// A partially-buried window top: surface 2 cells into the chunk above ->
    /// the window-top cell is 2 deep (soil), not stone and not grass.
    #[test]
    fn shallow_continuation_shifts_bands_partially() {
        // Surface at world y=33 (2 cells above the window top at 31).
        let we = WorldEvaluator::new(banded_terrain_graph(34.0));
        let eval = we.evaluate_chunk(EvalContext::new(7, IVec3::new(0, 0, 0))).unwrap();
        assert_eq!(material_at(&eval, 5, 31, 5), T_SOIL, "2 deep -> soil band");
        assert_eq!(material_at(&eval, 5, 30, 5), T_SOIL, "3 deep -> last soil");
        assert_eq!(material_at(&eval, 5, 29, 5), T_STONE, "4 deep -> fill");
    }

    #[test]
    fn empty_zone_ids_fall_back_to_the_band_index() {
        use nodegraph_ir::WorldOutputParams;
        // The defect this pins: `zone_ids: []` reads as "unassigned", but
        // `band_id` falls back to the band index, so one threshold assigns two
        // distinct zones. That was inert until zone ids selected a zone graph,
        // at which point half the shipped world silently became biome 0.
        let mut world = Graph::of_kind(GraphKind::World);
        let noise = world.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let out = world.add_node(NodeKind::WorldOutput(WorldOutputParams {
            zone_bands: vec![0.0],
            zone_ids: Vec::new(),
            ..Default::default()
        }));
        world.connect(PinRef::new(noise, 0), PinRef::new(out, 0)).unwrap();
        let we = WorldEvaluator::new(terrain_graph(1.0)).with_world(world);
        assert_eq!(we.assignable_zone_ids(), vec![0, 1]);

        // Explicit ids collapse both bands onto one zone — the shipped shape.
        let mut world = Graph::of_kind(GraphKind::World);
        let noise = world.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let out = world.add_node(NodeKind::WorldOutput(WorldOutputParams {
            zone_bands: vec![0.0],
            zone_ids: vec![0, 0],
            ..Default::default()
        }));
        world.connect(PinRef::new(noise, 0), PinRef::new(out, 0)).unwrap();
        let we = WorldEvaluator::new(terrain_graph(1.0)).with_world(world);
        assert_eq!(we.assignable_zone_ids(), vec![0]);
    }

    /// A ZoneGraph assigning `biome` to every column: no bands means band 0
    /// everywhere, and `biome_ids[0]` is the answer.
    fn zone_const_biome(biome: u16) -> Graph {
        let mut g = Graph::of_kind(GraphKind::Zone);
        // High frequency so a 32-column chunk certainly crosses the World's
        // zone threshold.
        let noise = g.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let out = g.add_node(NodeKind::ZoneOutput(ZoneOutputParams {
            biome_bands: Vec::new(),
            biome_ids: vec![biome],
            ..Default::default()
        }));
        g.connect(PinRef::new(noise, 0), PinRef::new(out, 0)).unwrap();
        g
    }

    /// A horizontal bar, wide in X so it straddles X seams often. Anchored at
    /// its middle, unrotated, so "crosses the seam" is a property of where the
    /// hash puts it rather than of which yaw came up.
    fn bar_blueprint() -> voxel_core::ResolvedBlueprint {
        let v = Voxel::cube(MaterialId(2));
        voxel_core::ResolvedBlueprint {
            name: "bar".to_string(),
            anchor: [3, 0, 0],
            cells: (0..7).map(|x| ([x, 0, 0], v)).collect(),
            destruction: voxel_core::DestructionPolicy::Destroy,
            protected_volume: false,
        }
    }

    /// Zone 0's graph, plus a structure declaration. The `PlaceStructure` node
    /// is unconnected by design - it has no pins, and the evaluator finds it by
    /// scanning rather than by pulling a value through it.
    fn zone_with_structure() -> Graph {
        use nodegraph_ir::PlaceStructureParams;
        let mut g = zone_const_biome(0);
        g.add_node(NodeKind::PlaceStructure(PlaceStructureParams {
            blueprint: "bar".to_string(),
            // Small cells at full density so the two-chunk window contains many
            // candidates: the test needs a seam-crosser to exist, and the guard
            // below fails loudly rather than passing vacuously if none does.
            cell_size: 8,
            density: 1.0,
            seed: 4,
            random_yaw: false,
            surface_offset: 0,
            resolved: Some(bar_blueprint()),
        }));
        g
    }

    #[test]
    fn each_column_takes_its_biome_from_its_own_zone() {
        use nodegraph_ir::WorldOutputParams;

        // World: climate splits the chunk into zone 0 and zone 1. The frequency
        // is deliberately high so a single chunk straddles the threshold many
        // times - a chunk that lands entirely in one zone would pass this test
        // while proving nothing, which is the failure the slab-determinism test
        // had before Phase 10 put a guard on it.
        let mut world = Graph::of_kind(GraphKind::World);
        let noise = world.add_node(NodeKind::SurfaceNoise(NoiseParams {
            frequency: 0.2,
            ..NoiseParams::default()
        }));
        let wo = world.add_node(NodeKind::WorldOutput(WorldOutputParams {
            zone_bands: vec![0.0],
            zone_ids: vec![0, 1],
            ..Default::default()
        }));
        world.connect(PinRef::new(noise, 0), PinRef::new(wo, 0)).unwrap();

        // Two zones whose biome tables disagree: zone 0 says biome 0 everywhere,
        // zone 1 says biome 1 everywhere. So a column's biome is a direct
        // readout of which zone graph answered for it.
        let b0 = terrain_graph_mat(5.0, MaterialId(1));
        let b1 = terrain_graph_mat(5.0, MaterialId(2));
        let ctx = EvalContext::new(11, IVec3::new(1, 0, -2));
        let we = WorldEvaluator::new(b0.clone())
            .with_world(world)
            .with_zones(vec![(0, zone_const_biome(0)), (1, zone_const_biome(1))])
            .with_biomes(vec![(0, b0), (1, b1)])
            .with_cross_graph_resolved();
        let eval = we.evaluate_chunk(ctx).unwrap();

        let zones = eval.zone_ids.clone().expect("the World graph assigns zones");
        let biomes = eval.biome_ids.clone().expect("the zone graphs assign biomes");

        assert!(
            zones.data().iter().any(|&z| z == 0) && zones.data().iter().any(|&z| z == 1),
            "the test chunk must straddle a zone border, or it asserts nothing",
        );

        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                assert_eq!(
                    biomes.get(x, z),
                    zones.get(x, z),
                    "column ({x},{z}) must take its biome from its own zone's table",
                );
            }
        }

        // And the assignment must survive all the way into voxels: both biomes
        // are solid, so density blending keeps every column solid and material
        // is the winning biome's - a sharp per-column cut.
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let expected = if zones.get(x, z) == 0 { MaterialId(1) } else { MaterialId(2) };
                let v = eval.terrain.get(x, 0, z);
                assert_ne!(v, Voxel::EMPTY, "both biomes are solid");
                assert_eq!(v.material, expected, "material follows the column's zone");
            }
        }
    }

    /// A biome whose density chain runs through a `LibraryRef`, with a material
    /// cake on top.
    ///
    /// `Max(1.0, cave)` is deliberately always solid: the point is to exercise
    /// the *pointwise* path through the library, not to produce caves. Returns
    /// the graph plus the kernel map it needs.
    fn library_density_biome() -> (Graph, HashMap<nodegraph_ir::LibraryGraphId, LibraryKernel>) {
        use nodegraph_ir::{
            BoundaryPort, ConstantParams, GraphBoundary, LayerParams, LibraryGraphId,
            LibraryGraphRegistry, LibraryRefParams, MaxParams, PinType, WorldPosParams,
        };

        const CAVE: LibraryGraphId = LibraryGraphId(0);

        // The library's declared boundary, so the reference can be wired -
        // `connect` validates against pin count, and an unresolved reference has
        // none.
        let mut lib = Graph::of_kind(GraphKind::Library);
        lib.boundary = GraphBoundary {
            inputs: vec![BoundaryPort::new("position", PinType::Vec3)],
            outputs: vec![BoundaryPort::new("density", PinType::Density)],
        };
        let mut registry = LibraryGraphRegistry::new();
        registry.insert(CAVE, lib);

        let mut g = Graph::new();
        let pos = g.add_node(NodeKind::WorldPos(WorldPosParams::default()));
        let cave = g.add_node(NodeKind::LibraryRef(LibraryRefParams {
            library: CAVE,
            ..Default::default()
        }));
        g.resolve_library_refs(&registry);

        let one = g.add_node(NodeKind::Constant(ConstantParams { value: 1.0 }));
        let solid = g.add_node(NodeKind::Max(MaxParams::default()));
        let layer = g.add_node(NodeKind::Layer(LayerParams {
            bands: vec![(MaterialId(6), 1), (MaterialId(3), 3)],
            fill: MaterialId(1),
        }));
        let out = g.add_node(NodeKind::DensityOutput(DensityOutputParams::default()));

        g.connect(PinRef::new(pos, 0), PinRef::new(cave, 0)).unwrap();
        g.connect(PinRef::new(one, 0), PinRef::new(solid, 0)).unwrap();
        g.connect(PinRef::new(cave, 0), PinRef::new(solid, 1)).unwrap();
        g.connect(PinRef::new(solid, 0), PinRef::new(layer, 0)).unwrap();
        g.connect(PinRef::new(solid, 0), PinRef::new(out, 0)).unwrap();
        g.connect(PinRef::new(layer, 0), PinRef::new(out, 1)).unwrap();

        (g, HashMap::from([(CAVE, LibraryKernel::StandardCaveNoise)]))
    }

    #[test]
    fn a_library_ref_samples_pointwise_above_the_chunk_window() {
        // The direct form: the chunk-Y seam correction samples density at
        // `y >= CHUNK_DIM`, where no filled field exists. Before Substep 6a this
        // returned `UnresolvedLibraryRef` for any chain containing a library.
        let (graph, kernels) = library_density_biome();
        let solid = graph
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::Max(_)))
            .map(|(id, _)| id)
            .expect("the chain has a Max");

        let mut eval = Evaluator::new(&graph, EvalContext::new(3, IVec3::ZERO))
            .with_library_kernels(&kernels);
        eval.evaluate().unwrap();

        let above = eval
            .sample_density(solid, 5, CHUNK_DIM + 2, 7)
            .expect("a library in the chain must sample above the window");
        assert!(above >= 1.0, "Max(1.0, cave) is at least 1.0 everywhere");
    }

    #[test]
    fn a_library_in_the_density_chain_keeps_chunk_y_seam_material_continuous() {
        // The end-to-end form. With density solid everywhere, every column's
        // topmost in-window cell is buried by the chunk above, so its material
        // must be the cake's *fill* — not its surface band.
        //
        // If the pointwise sample fails, `above` stays 0, depth at y = 31 reads
        // as 0, and the seam paints grass. That is the reported artifact, and
        // this is the assertion that distinguishes the two.
        let (graph, kernels) = library_density_biome();
        let ctx = EvalContext::new(3, IVec3::ZERO);
        let eval = WorldEvaluator::new(graph)
            .with_library_kernels(kernels)
            .evaluate_chunk(ctx)
            .expect("evaluates with kernels attached");

        let top = CHUNK_DIM - 1;
        for z in [0usize, 13, 31] {
            for x in [0usize, 17, 31] {
                let v = eval.terrain.get(x, top, z);
                assert_ne!(v, Voxel::EMPTY, "the column is solid through the window top");
                assert_eq!(
                    v.material,
                    MaterialId(1),
                    "column ({x},{z}) at the chunk-Y seam must read as buried fill, \
                     not surface — grass here is the banding artifact",
                );
            }
        }
    }

    #[test]
    fn a_library_density_chain_is_deterministic() {
        // Every generation pass ships with one. A library kernel seeded from the
        // library id rather than a node-local seed is still a pure function of
        // (position, id), and this is what says so.
        let ctx = EvalContext::new(3, IVec3::new(-2, 0, 5));
        let run = || {
            let (graph, kernels) = library_density_biome();
            WorldEvaluator::new(graph)
                .with_library_kernels(kernels)
                .evaluate_chunk(ctx)
                .unwrap()
        };
        let (a, b) = (run(), run());
        for i in 0..ChunkBuffer::<Voxel, 32>::VOLUME {
            assert_eq!(a.terrain.get_index(i), b.terrain.get_index(i), "cell {i} diverged");
        }
    }

    #[test]
    fn inspect_column_reports_what_generation_used() {
        // The inspector's whole value is that it agrees with generation. This
        // pins that: same evaluator, same chunk, the reported biome must equal
        // the one the composite selected.
        let zone = zone_split();
        let b0 = terrain_graph_mat(5.0, MaterialId(1));
        let b1 = terrain_graph_mat(5.0, MaterialId(2));
        let ctx = EvalContext::new(7, IVec3::new(1, 0, 1));
        let we = WorldEvaluator::new(b0.clone())
            .with_zone(zone)
            .with_biomes(vec![(0, b0), (1, b1)]);

        let eval = we.evaluate_chunk(ctx).unwrap();
        let biomes = eval.biome_ids.clone().expect("the zone graph assigns biomes");
        assert!(
            biomes.data().iter().any(|&b| b == 0) && biomes.data().iter().any(|&b| b == 1),
            "the test chunk must contain both biomes, or this asserts nothing",
        );

        for (x, z) in [(0usize, 0usize), (17, 5), (31, 31)] {
            let r = we.inspect_column(ctx, x, z, 0..1).unwrap();
            assert_eq!(
                r.biome_id,
                biomes.get(x, z),
                "column ({x},{z}): the inspector must report the biome generation used",
            );
            assert!(
                (0.0..=r.fade_radius).contains(&r.border_distance),
                "border distance is capped at the fade radius",
            );
        }
    }


    #[test]
    fn the_map_agrees_with_the_column_inspector() {
        // A preview that derives its own answer previews a different world.
        // This pins that the map's sampling path and the inspector's are the
        // same path.
        let zone = zone_split();
        let b0 = terrain_graph_mat(5.0, MaterialId(1));
        let b1 = terrain_graph_mat(5.0, MaterialId(2));
        let we = WorldEvaluator::new(b0.clone())
            .with_zone(zone)
            .with_biomes(vec![(0, b0), (1, b1)]);

        let step = 4;
        let dim = 24;
        let map = we.sample_id_map(7, 0, 0, step, dim).unwrap();
        assert!(
            map.biome.iter().any(|&b| b == 0) && map.biome.iter().any(|&b| b == 1),
            "the sampled area must contain both biomes, or this asserts nothing",
        );

        for (gx, gz) in [(0usize, 0usize), (7, 3), (23, 23)] {
            let wx = gx as i32 * step;
            let wz = gz as i32 * step;
            let ctx = EvalContext::new(7, IVec3::new(wx.div_euclid(32), 0, wz.div_euclid(32)));
            let r = we
                .inspect_column(
                    ctx,
                    wx.rem_euclid(32) as usize,
                    wz.rem_euclid(32) as usize,
                    0..1,
                )
                .unwrap();
            assert_eq!(map.biome[gx + gz * dim], r.biome_id, "map vs inspector at ({wx},{wz})");
            assert_eq!(map.zone[gx + gz * dim], r.zone_id);
            // The pointwise single-column lookup is a third path to the same
            // answer, and the one the climate sim calls per cloud cell.
            assert_eq!(
                we.sample_biome_at(7, wx, wz).unwrap(),
                r.biome_id,
                "pointwise lookup vs inspector at ({wx},{wz})",
            );
        }
    }

    #[test]
    fn a_structure_crossing_a_chunk_seam_is_stamped_whole_by_the_two_chunks() {
        // The property the cross-chunk feature model exists to provide, checked
        // end-to-end rather than on `derive_features` alone. The aggregate hash
        // cannot stand in for this: it samples a 128-voxel footprint, so whether
        // it observes a structure at all depends on where one happens to land.
        const SEED: u64 = 31;
        let we = WorldEvaluator::new(terrain_graph_mat(1.0, MaterialId(1)))
            .with_zones(vec![(0, zone_with_structure())])
            .with_biomes(vec![(0, terrain_graph_mat(1.0, MaterialId(1)))])
            .with_cross_graph_resolved()
            .with_chunk_y_range(0..1);

        let ctx0 = EvalContext::new(SEED, IVec3::new(0, 0, 0));
        let ctx1 = EvalContext::new(SEED, IVec3::new(1, 0, 0));
        let c0 = we.evaluate_chunk(ctx0).unwrap();
        let c1 = we.evaluate_chunk(ctx1).unwrap();

        // Locate a crosser using the same derivation the evaluator uses. This
        // only *finds* the feature - every assertion below reads the two
        // independently generated terrains, which is the part under test.
        let sources = we.structure_sources().unwrap();
        // Empty: this world's biome graphs read no upstream, and the probe is
        // being driven directly rather than through `evaluate_chunk`.
        let probe_cache = ColumnCache::new();
        let probe_upstream = we.upstream_for(ctx1, &probe_cache);
        let features = crate::feature::derive_features(ctx1, &sources, |x, z| {
            we.probe_column(SEED, x, z, 0..1, &probe_upstream).unwrap()
        });
        let bp = &sources[0].blueprint;
        let anchor = IVec3::from_array(bp.anchor);

        let mut checked_left = 0usize;
        let mut checked_right = 0usize;
        for feature in &features {
            let cells: Vec<IVec3> = bp
                .cells
                .iter()
                .map(|(at, _)| feature.anchor + (IVec3::from_array(*at) - anchor))
                .collect();
            let crosses = cells.iter().any(|c| c.x < 32) && cells.iter().any(|c| c.x >= 32);
            if !crosses {
                continue;
            }
            for cell in cells {
                if cell.y < 0 || cell.y >= 32 || cell.z < 0 || cell.z >= 32 {
                    continue;
                }
                let (y, z) = (cell.y as usize, cell.z as usize);
                if (0..32).contains(&cell.x) {
                    assert_eq!(
                        c0.terrain.get(cell.x as usize, y, z).material,
                        MaterialId(2),
                        "chunk (0,0,0) dropped its half of a structure at {cell:?}",
                    );
                    checked_left += 1;
                } else if (32..64).contains(&cell.x) {
                    assert_eq!(
                        c1.terrain.get((cell.x - 32) as usize, y, z).material,
                        MaterialId(2),
                        "chunk (1,0,0) dropped its half of a structure at {cell:?}",
                    );
                    checked_right += 1;
                }
            }
        }

        // Without this the test passes vacuously when no feature crosses - the
        // same failure the zone-border test guards against. If it ever trips,
        // the fix is a different SEED, not a weaker assertion.
        assert!(
            checked_left > 0 && checked_right > 0,
            "no structure straddled the seam ({checked_left} left, {checked_right} right); \
             this test proves nothing until one does",
        );
    }

    #[test]
    fn a_biome_density_chain_reads_the_world_graph() {
        use nodegraph_ir::{GraphOutputParams, GraphRefParams, SurfaceToDensityParams};
        // B2 and B3 end to end, through the production path: a biome graph whose
        // density comes from a World climate channel, generated by
        // `evaluate_chunk`. Before this, the same graph failed the whole chunk
        // with `UnresolvedGraphRef`.
        let mut world = Graph::of_kind(GraphKind::World);
        let wn = world.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let wo = world.add_node(NodeKind::GraphOutput(GraphOutputParams { name: "climate".into() }));
        world.connect(PinRef::new(wn, 0), PinRef::new(wo, 0)).unwrap();

        let mut biome = Graph::new();
        let gr = biome.add_node(NodeKind::GraphRef(GraphRefParams {
            target: GraphRefTarget::World,
            ..Default::default()
        }));
        let s2d = biome.add_node(NodeKind::SurfaceToDensity(SurfaceToDensityParams::default()));
        let mat = biome.add_node(NodeKind::ConstantMaterial(ConstantMaterialParams::default()));
        let out = biome.add_node(NodeKind::DensityOutput(DensityOutputParams::default()));

        // Pins are resolved by the builder, exactly as loading a world does -
        // wiring the GraphRef before that would have nothing to wire to.
        let we = WorldEvaluator::new(biome).with_world(world).with_cross_graph_resolved();
        let mut biome = we.biome_graph().clone();
        biome.connect(PinRef::new(gr, 0), PinRef::new(s2d, 0)).unwrap();
        biome.connect(PinRef::new(s2d, 0), PinRef::new(out, 0)).unwrap();
        biome.connect(PinRef::new(mat, 0), PinRef::new(out, 1)).unwrap();
        let we = WorldEvaluator::new(biome)
            .with_world(we.world_graph().clone())
            .with_cross_graph_resolved();

        let ctx = EvalContext::new(7, IVec3::new(3, 0, -2));
        let chunk = we.evaluate_chunk(ctx).expect("a biome GraphRef must evaluate");

        // Solid exactly where the world's climate channel is positive.
        let world_point = ColumnEvaluator::new(we.world_graph(), ctx);
        let climate_node = we
            .world_graph()
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::GraphOutput(_)))
            .map(|(id, _)| id)
            .unwrap();
        let dim = CHUNK_DIM as i32;
        let (mut solid, mut air) = (0, 0);
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let wx = ctx.chunk.x * dim + x as i32;
                let wz = ctx.chunk.z * dim + z as i32;
                let climate = match world_point.sample_column(climate_node, wx, wz).unwrap() {
                    ColumnSample::Surface(v) => v,
                    other => panic!("expected surface, got {other:?}"),
                };
                let filled = chunk.terrain.get(x, 0, z).material != MaterialId::AIR;
                assert_eq!(filled, climate > 0.0, "column ({wx}, {wz}) climate {climate}");
                if filled { solid += 1 } else { air += 1 }
            }
        }
        // A chunk that came out all one way would satisfy the loop and prove
        // nothing about the channel actually reaching the density.
        assert!(solid > 0 && air > 0, "expected a mix, got {solid} solid / {air} air");
    }
}
