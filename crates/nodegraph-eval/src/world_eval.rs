//! Multi-graph evaluation harness for the five-graph hierarchy.

use std::collections::HashMap;
use std::sync::Arc;

use nodegraph_ir::{Graph, GraphKind, GraphRefTarget, LibraryGraphRegistry, NodeId, NodeKind, Severity};
use voxel_core::{ChunkBuffer, MaterialId, ShapeId, Voxel};

use crate::biome_params::BiomeParams;
use crate::border::{analyze_biome_borders, biome_border_fade, blend_density, BorderAnalysis};
use crate::library_kernel::surface_layering;
use crate::column::{ColumnCache, ColumnField, IdColumn};
use crate::column_eval::{ColumnEvaluator, UpstreamGraphs};
use crate::context::EvalContext;
use crate::detail_eval::DetailEvaluator;
use crate::error::EvalResult;
use crate::eval::Evaluator;
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
    zone: Graph,
    biomes: Vec<BiomeGraph>,
    libraries: LibraryGraphRegistry,
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
            zone: Graph::of_kind(GraphKind::Zone),
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
        };
        this.log_validation();
        this
    }

    /// Replace the World graph (builder-style).
    pub fn with_world(mut self, world: Graph) -> Self {
        self.world = world;
        self
    }

    /// Replace the Zone graph (builder-style).
    pub fn with_zone(mut self, zone: Graph) -> Self {
        self.zone = zone;
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

    /// Finalize cross-graph dataflow: derive the World and Zone boundaries from
    /// their `GraphOutput` nodes, then resolve the Zone graph's `GraphRef` pins
    /// against those boundaries. Call once after the graphs are installed (it is
    /// a no-op when nothing references anything). Biome-graph cross-graph imports
    /// arrive with the biome density rework (Substep 7).
    pub fn with_cross_graph_resolved(mut self) -> Self {
        self.world.derive_output_boundary();
        self.zone.derive_output_boundary();
        let world_boundary = self.world.boundary.clone();
        let zone_boundary = self.zone.boundary.clone();
        let boundary_of = |t: GraphRefTarget| match t {
            GraphRefTarget::World => Some(world_boundary.clone()),
            GraphRefTarget::Zone => Some(zone_boundary.clone()),
            GraphRefTarget::Biome(_) => None,
        };
        self.zone.resolve_graph_refs(boundary_of);
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

    /// The Zone graph.
    pub fn zone_graph(&self) -> &Graph {
        &self.zone
    }

    /// The library registry.
    pub fn libraries(&self) -> &LibraryGraphRegistry {
        &self.libraries
    }

    /// The `NodeId` of the WorldGraph's terminal `WorldOutput`, if one exists.
    pub fn world_output_node(&self) -> Option<NodeId> {
        self.world
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::WorldOutput(_)))
            .map(|(id, _)| id)
    }

    /// The `NodeId` of the ZoneGraph's terminal `ZoneOutput`, if one exists.
    pub fn zone_output_node(&self) -> Option<NodeId> {
        self.zone
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::ZoneOutput(_)))
            .map(|(id, _)| id)
    }

    /// A biome's scalar parameter by name, if that biome exists and declares it.
    pub fn biome_param(&self, biome_id: u16, name: &str) -> Option<f32> {
        self.biomes
            .iter()
            .find(|b| b.id == biome_id)
            .and_then(|b| b.params.get(name))
    }

    /// Evaluate the graph set for one chunk: World -> per-column zone ids, Zone
    /// -> per-column biome ids, then composite the biome graphs' terrain by that
    /// biome assignment.
    pub fn evaluate_chunk(&self, ctx: EvalContext) -> EvalResult<ChunkEvaluation> {
        let world_columns = Self::eval_columns(&self.world, ctx, None)?;

        // Evaluate Zone with World as its cross-graph upstream, and analyze biome
        // borders (a cross-chunk scan) so the composite can blend densities at
        // boundaries. Scoped so the upstream's borrow of `world_columns` ends
        // before it is moved out.
        let (zone_columns, border) = {
            let mut upstream = UpstreamGraphs::new();
            if !self.world.nodes.is_empty() {
                upstream.insert(GraphRefTarget::World, &self.world, ctx, &world_columns);
            }
            if self.zone.nodes.is_empty() {
                (ColumnCache::new(), None)
            } else {
                let mut zone_eval = ColumnEvaluator::new(&self.zone, ctx).with_upstream(&upstream);
                zone_eval.evaluate()?;
                let border = match self.zone_output_node() {
                    Some(zn) => Some(analyze_biome_borders(&zone_eval, zn, ctx.chunk, FADE_RADIUS)?),
                    None => None,
                };
                (zone_eval.into_cache(), border)
            }
        };

        let biome_col = self.biome_id_column(&zone_columns);
        let terrain = self.composite_terrain(ctx, biome_col.as_deref(), border.as_ref())?;
        // Stage 9 (fluid) precedes stage 10 (foliage) per design doc §5. Foliage
        // submersion is applied at the storage boundary (world_generator), where
        // the fully-composited fluid field is available; the order here matches
        // the documented stage sequence.
        let fluid_levels = self.composite_fluid(ctx, biome_col.as_deref())?;
        let foliage = self.evaluate_foliage(ctx, &terrain, biome_col.as_deref())?;

        Ok(ChunkEvaluation { terrain, world_columns, zone_columns, foliage, fluid_levels })
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

    /// The Zone graph's per-column biome assignment for this chunk, if any.
    fn biome_id_column(&self, zone_columns: &ColumnCache) -> Option<Arc<IdColumn>> {
        self.zone_output_node()
            .and_then(|n| zone_columns.get(n))
            .and_then(|c| c.as_id())
            .cloned()
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

            let mut eval = Evaluator::new(&bg.graph, ctx).with_biome_params(&bg.params);
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
            if let Some(layer) = self.eval_biome_layer(ctx, bid)? {
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

    /// Evaluate one biome graph's (density, material) layer, or `None` if the
    /// biome id has no graph or the graph has no `DensityOutput`. The evaluator
    /// is kept alive on the returned [`LayerEval`] so the composite can
    /// pointwise-sample density above the chunk window (chunk-Y seam depth).
    fn eval_biome_layer(&self, ctx: EvalContext, bid: u16) -> EvalResult<Option<LayerEval<'_>>> {
        let Some(bg) = self.biomes.iter().find(|b| b.id == bid) else {
            return Ok(None);
        };
        let Some(density_node) = bg.density_node else {
            return Ok(None);
        };
        let mut eval = Evaluator::new(&bg.graph, ctx).with_biome_params(&bg.params);
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

    /// Evaluate a per-column (World/Zone) graph into a cache, or return an empty
    /// cache when the graph has no nodes (consumers default ids to 0). An
    /// `upstream` context resolves any `GraphRef` reads to other graphs' outputs.
    fn eval_columns<'a>(
        graph: &Graph,
        ctx: EvalContext,
        upstream: Option<&'a UpstreamGraphs<'a>>,
    ) -> EvalResult<ColumnCache> {
        if graph.nodes.is_empty() {
            return Ok(ColumnCache::new());
        }
        let mut col = ColumnEvaluator::new(graph, ctx);
        if let Some(up) = upstream {
            col = col.with_upstream(up);
        }
        col.evaluate()?;
        Ok(col.into_cache())
    }

    /// Validate each graph + the library reference set, logging findings.
    fn log_validation(&self) {
        let mut graphs: Vec<(String, &Graph)> = vec![
            ("world".to_string(), &self.world),
            ("zone".to_string(), &self.zone),
        ];
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
    /// WorldGraph per-column outputs (climate channels + zone ids). Empty when
    /// the WorldGraph has no nodes.
    pub world_columns: ColumnCache,
    /// ZoneGraph per-column outputs (climate channels + biome ids). Empty when
    /// the ZoneGraph has no nodes.
    pub zone_columns: ColumnCache,
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
        let out = g.add_node(NodeKind::ZoneOutput(ZoneOutputParams { biome_bands: vec![0.0] }));
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
    fn world_and_zone_are_empty_by_default() {
        let we = WorldEvaluator::new(terrain_graph(1.0));
        assert_eq!(we.world_graph().kind, GraphKind::World);
        assert_eq!(we.zone_graph().kind, GraphKind::Zone);
        assert!(we.world_graph().nodes.is_empty());
        assert!(we.zone_graph().nodes.is_empty());
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
        assert!(eval.world_columns.is_empty());
        assert!(eval.zone_columns.is_empty());
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

        let zone_node = we.zone_output_node().unwrap();
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
        let zone_node = we.world_output_node().expect("world output present");
        let ids = eval.world_columns.get(zone_node).unwrap().as_id().unwrap();
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
        let biome_node = we.zone_output_node().expect("zone output present");
        let ids = eval.zone_columns.get(biome_node).unwrap().as_id().unwrap();
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
        let zo = zone.add_node(NodeKind::ZoneOutput(ZoneOutputParams { biome_bands: vec![0.0] }));
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

        // Zone's biome column must equal quantizing World's climate directly.
        let zone_node = we.zone_output_node().unwrap();
        let biome_ids = eval.zone_columns.get(zone_node).unwrap().as_id().unwrap();
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
}
