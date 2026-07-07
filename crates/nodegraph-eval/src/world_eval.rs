//! Multi-graph evaluation harness for the five-graph hierarchy.

use std::sync::Arc;

use nodegraph_ir::{Graph, GraphKind, LibraryGraphRegistry, NodeId, NodeKind, Severity};
use voxel_core::{ChunkBuffer, Voxel};

use crate::cache::CachedOutput;
use crate::column::{ColumnCache, IdColumn};
use crate::column_eval::ColumnEvaluator;
use crate::context::EvalContext;
use crate::detail_eval::DetailEvaluator;
use crate::error::EvalResult;
use crate::eval::Evaluator;
use crate::field::CHUNK_DIM;
use crate::foliage::ChunkFoliage;

/// One biome's terrain graph, the biome id it renders, and its located
/// `TerrainOutput` (absent if the graph has none - such a biome produces air),
/// and an optional `DetailGraph` producing the biome's foliage.
struct BiomeGraph {
    id: u16,
    graph: Graph,
    terrain_node: Option<NodeId>,
    detail: Option<Graph>,
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
        let terrain_node = find_terrain_node(&biome);
        let this = Self {
            world: Graph::of_kind(GraphKind::World),
            zone: Graph::of_kind(GraphKind::Zone),
            biomes: vec![BiomeGraph { id: 0, graph: biome, terrain_node, detail: None }],
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
    /// pair; the graph's `TerrainOutput` is located now (a biome without one
    /// produces air for its columns).
    pub fn with_biomes(mut self, biomes: Vec<(u16, Graph)>) -> Self {
        self.biomes = biomes
            .into_iter()
            .map(|(id, graph)| {
                let terrain_node = find_terrain_node(&graph);
                BiomeGraph { id, graph, terrain_node, detail: None }
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

    /// Replace the library registry (builder-style).
    pub fn with_libraries(mut self, libraries: LibraryGraphRegistry) -> Self {
        self.libraries = libraries;
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

    /// Evaluate the graph set for one chunk: World -> per-column zone ids, Zone
    /// -> per-column biome ids, then composite the biome graphs' terrain by that
    /// biome assignment.
    pub fn evaluate_chunk(&self, ctx: EvalContext) -> EvalResult<ChunkEvaluation> {
        let world_columns = Self::eval_columns(&self.world, ctx)?;
        let zone_columns = Self::eval_columns(&self.zone, ctx)?;

        let biome_col = self.biome_id_column(&zone_columns);
        let terrain = self.composite_terrain(ctx, biome_col.as_deref())?;
        let foliage = self.evaluate_foliage(ctx, &terrain, biome_col.as_deref())?;

        Ok(ChunkEvaluation { terrain, world_columns, zone_columns, foliage })
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

    /// Composite each column's biome terrain into one chunk buffer. `biome_col`
    /// gives the biome id per `(x, z)` column; `None` (no Zone graph) means biome
    /// 0 everywhere.
    fn composite_terrain(
        &self,
        ctx: EvalContext,
        biome_col: Option<&IdColumn>,
    ) -> EvalResult<Arc<ChunkBuffer<Voxel, 32>>> {
        let biome_at = |x: usize, z: usize| biome_col.map_or(0u16, |c| c.get(x, z));
        let present = present_biomes(biome_col);

        // Fast path: the whole chunk is one biome - return its terrain directly,
        // no per-voxel copy (the single-biome world's path).
        if present.len() == 1 {
            return Ok(self
                .eval_biome_terrain(ctx, present[0])?
                .unwrap_or_else(empty_terrain));
        }

        // Composite path: per biome, evaluate its terrain and copy its columns.
        let mut out = ChunkBuffer::<Voxel, 32>::uniform(Voxel::EMPTY);
        for &bid in &present {
            let Some(terr) = self.eval_biome_terrain(ctx, bid)? else {
                continue; // unknown / terrain-less biome -> its columns stay air
            };
            for z in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    if biome_at(x, z) == bid {
                        for y in 0..CHUNK_DIM {
                            out.set(x, y, z, terr.get(x, y, z));
                        }
                    }
                }
            }
        }
        Ok(Arc::new(out))
    }

    /// Evaluate one biome graph's whole-chunk terrain, or `None` if the biome id
    /// has no graph or the graph has no `TerrainOutput`.
    fn eval_biome_terrain(
        &self,
        ctx: EvalContext,
        bid: u16,
    ) -> EvalResult<Option<Arc<ChunkBuffer<Voxel, 32>>>> {
        let Some(bg) = self.biomes.iter().find(|b| b.id == bid) else {
            return Ok(None);
        };
        let Some(terrain_node) = bg.terrain_node else {
            return Ok(None);
        };
        let mut eval = Evaluator::new(&bg.graph, ctx);
        eval.evaluate()?;
        Ok(match eval.cache().get(terrain_node) {
            Some(CachedOutput::Terrain(t)) => Some(t.clone()),
            _ => None,
        })
    }

    /// Evaluate a per-column (World/Zone) graph into a cache, or return an empty
    /// cache when the graph has no nodes (consumers default ids to 0).
    fn eval_columns(graph: &Graph, ctx: EvalContext) -> EvalResult<ColumnCache> {
        if graph.nodes.is_empty() {
            return Ok(ColumnCache::new());
        }
        let mut col = ColumnEvaluator::new(graph, ctx);
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

/// Locate a graph's `TerrainOutput` terminal, if any.
fn find_terrain_node(graph: &Graph) -> Option<NodeId> {
    graph
        .nodes
        .iter()
        .find(|(_, n)| matches!(n.kind, NodeKind::TerrainOutput(_)))
        .map(|(id, _)| id)
}

/// An all-air chunk buffer (a biome with no terrain).
fn empty_terrain() -> Arc<ChunkBuffer<Voxel, 32>> {
    Arc::new(ChunkBuffer::uniform(Voxel::EMPTY))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;
    use nodegraph_ir::{BuildTerrainParams, ConstantMaterialParams, ConstantParams, NoiseParams, PinRef, PoissonDistributionParams, TerrainOutputParams, ZoneOutputParams};

    /// A biome graph filling the chunk with uniform `density` + a constant
    /// material: Constant -> BuildTerrain <- ConstantMaterial -> TerrainOutput.
    fn terrain_graph(density: f32) -> Graph {
        let mut g = Graph::new();
        let d = g.add_node(NodeKind::Constant(ConstantParams { value: density }));
        let m = g.add_node(NodeKind::ConstantMaterial(ConstantMaterialParams::default()));
        let bt = g.add_node(NodeKind::BuildTerrain(BuildTerrainParams::default()));
        let to = g.add_node(NodeKind::TerrainOutput(TerrainOutputParams::default()));
        g.connect(PinRef::new(d, 0), PinRef::new(bt, 0)).unwrap();
        g.connect(PinRef::new(m, 0), PinRef::new(bt, 1)).unwrap();
        g.connect(PinRef::new(bt, 0), PinRef::new(to, 0)).unwrap();
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

    fn eval_terrain(graph: &Graph, ctx: EvalContext) -> Arc<ChunkBuffer<Voxel, 32>> {
        let tn = find_terrain_node(graph).unwrap();
        let mut e = Evaluator::new(graph, ctx);
        e.evaluate().unwrap();
        match e.cache().get(tn) {
            Some(CachedOutput::Terrain(t)) => t.clone(),
            _ => panic!("graph produced no terrain"),
        }
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
        let biome = terrain_graph(1.0);
        let ctx = EvalContext::new(0, IVec3::ZERO);
        let eval = WorldEvaluator::new(biome.clone()).evaluate_chunk(ctx).unwrap();
        let expected = eval_terrain(&biome, ctx);
        for i in 0..ChunkBuffer::<Voxel, 32>::VOLUME {
            assert_eq!(eval.terrain.get_index(i), expected.get_index(i));
        }
        assert!(eval.world_columns.is_empty());
        assert!(eval.zone_columns.is_empty());
    }

    #[test]
    fn composite_matches_per_column_biome_terrain() {
        let zone = zone_split();
        let b0 = terrain_graph(1.0);  // solid
        let b1 = terrain_graph(-1.0); // air
        let ctx = EvalContext::new(7, IVec3::new(1, 0, 1));

        let we = WorldEvaluator::new(b0.clone())
            .with_zone(zone.clone())
            .with_biomes(vec![(0, b0.clone()), (1, b1.clone())]);
        let eval = we.evaluate_chunk(ctx).unwrap();

        // Re-derive the per-column biome assignment and each biome's terrain.
        let zone_node = we.zone_output_node().unwrap();
        let mut ze = ColumnEvaluator::new(&zone, ctx);
        ze.evaluate().unwrap();
        let zone_cache = ze.into_cache();
        let biome_ids = zone_cache.get(zone_node).unwrap().as_id().unwrap();
        let t0 = eval_terrain(&b0, ctx);
        let t1 = eval_terrain(&b1, ctx);

        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let expected = if biome_ids.get(x, z) == 0 { &t0 } else { &t1 };
                for y in 0..CHUNK_DIM {
                    assert_eq!(eval.terrain.get(x, y, z), expected.get(x, y, z));
                }
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
}
