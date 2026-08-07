//! The per-column evaluator: fills a World/Zone graph's per-column nodes
//! (`SurfaceNoise`, `WorldOutput`, `ZoneOutput`) into a [`ColumnCache`].
//!
//! This is the 2D analog of [`Evaluator`](crate::Evaluator): same topological
//! whole-graph fill, but each node produces a per-`(x, z)` output rather than a
//! whole-chunk 3D field. Voxel-domain nodes have no per-column evaluation and
//! raise [`EvalError::WrongGraphDomain`].
//!
//! [`ColumnEvaluator::sample_column`] additionally offers a pointwise,
//! chunk-independent evaluation at any absolute world column - the basis for
//! cross-chunk biome-boundary access (a chunk querying a neighbor's columns).

use std::collections::HashMap;
use std::sync::Arc;

use fastnoise_lite::NoiseType;
use nodegraph_ir::{EdgeIndex, Graph, GraphRefTarget, NodeId, NodeKind, Severity};

use crate::column::{ColumnCache, ColumnField, ColumnOutput, IdColumn};
use crate::context::EvalContext;
use crate::error::{EvalError, EvalResult};
use crate::eval::configured_noise;
use crate::field::CHUNK_DIM;

/// Evaluates a World/Zone graph for one chunk, producing per-column outputs.
pub struct ColumnEvaluator<'g> {
    graph: &'g Graph,
    /// `(node, pin) -> source` over `graph.edges`, built once. See
    /// [`EdgeIndex`]; `sample_column` resolves an input per column probed.
    edges: EdgeIndex,
    ctx: EvalContext,
    cache: ColumnCache,
    /// Cross-graph resolution context for `GraphRef` reads (`None` for World and
    /// for graphs with no cross-graph references).
    upstream: Option<&'g UpstreamGraphs<'g>>,
}

impl<'g> ColumnEvaluator<'g> {
    /// New evaluator over a graph and chunk context.
    pub fn new(graph: &'g Graph, ctx: EvalContext) -> Self {
        Self { graph, edges: EdgeIndex::build(graph), ctx, cache: ColumnCache::new(), upstream: None }
    }
    
    /// Attach a cross-graph resolution context so `GraphRef` reads resolve to
    /// upstream graphs' named outputs.
    pub fn with_upstream(mut self, upstream: &'g UpstreamGraphs<'g>) -> Self {
        self.upstream = Some(upstream);
        self
    }

    /// Borrow the per-column cache (populated by [`ColumnEvaluator::evaluate`]).
    pub fn cache(&self) -> &ColumnCache {
        &self.cache
    }

    /// Consume the evaluator, returning its populated per-column cache.
    pub fn into_cache(self) -> ColumnCache {
        self.cache
    }

    /// Validate, then fill every node's per-column output in topological order.
    pub fn evaluate(&mut self) -> EvalResult<()> {
        let errors = self
            .graph
            .validate()
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        if errors > 0 {
            return Err(EvalError::InvalidGraph(errors));
        }
        let order = self.graph.topological_order().map_err(|_| EvalError::Cyclic)?;
        for id in order {
            if self.cache.contains(id) {
                continue;
            }
            // GraphRef reads upstream graphs on demand at input resolution; it
            // has no single cached output, so it is not filled here.
            if matches!(self.graph.nodes[id].kind, NodeKind::GraphRef(_)) {
                continue;
            }
            // Declaration-only nodes have *no pins in either direction*, so they
            // cannot participate in dataflow at all and there is nothing to
            // fill. `PlaceStructure` is read by scanning the graph, not by
            // pulling through pins.
            //
            // The test is deliberately both directions. Terminals such as
            // `WorldOutput` and `ZoneOutput` also declare no *outputs* - their
            // value is read from the cache by node id rather than through a pin
            // - so skipping on outputs alone skips exactly the nodes everything
            // else depends on.
            let d = self.graph.nodes[id].kind.descriptor();
            if d.inputs.is_empty() && d.outputs.is_empty() {
                continue;
            }
            let out = self.fill_node(id)?;
            self.cache.insert(id, out);
        }
        Ok(())
    }

    /// Compute one node's per-column output.
    fn fill_node(&self, id: NodeId) -> EvalResult<ColumnOutput> {
        let node = self.graph.nodes.get(id).ok_or(EvalError::MissingOutput(id))?;
        Ok(match &node.kind {
            NodeKind::SurfaceNoise(p) => {
                let n = configured_noise(
                    self.ctx.noise_seed(p.seed),
                    p,
                    NoiseType::OpenSimplex2,
                );
                let mut field = ColumnField::zeroed();
                for z in 0..CHUNK_DIM {
                    for x in 0..CHUNK_DIM {
                        let w = self.ctx.world_pos(x, 0, z);
                        field.set(x, z, n.get_noise_2d(w.x, w.z));
                    }
                }
                ColumnOutput::Surface(Arc::new(field))
            }
            NodeKind::WorldOutput(p) => {
                let field = self.input_surface(id, 0)?;
                ColumnOutput::Id(Arc::new(quantize_bands(&field, &p.zone_bands, &p.zone_ids)))
            }
            NodeKind::ZoneOutput(p) => {
                let field = self.input_surface(id, 0)?;
                ColumnOutput::Id(Arc::new(quantize_bands(&field, &p.biome_bands, &p.biome_ids)))
            }
            // GraphOutput marks its input value as a named boundary output; the
            // value passes through so a cross-graph reader can sample it by node.
            NodeKind::GraphOutput(_) => ColumnOutput::Surface(self.input_surface(id, 0)?),
            // Every other kind is a voxel-domain node with no per-column fill.
            _ => return Err(EvalError::WrongGraphDomain { node: id }),
        })
    }

    /// Resolve the surface field feeding `(node, pin)`. If the source is a
    /// `GraphRef`, read the referenced upstream graph's named output (bulk, from
    /// the upstream's computed cache); otherwise read this graph's own cache.
    fn input_surface(&self, node: NodeId, pin: u16) -> EvalResult<Arc<ColumnField>> {
        let from = self.edges.source(node, pin).ok_or(EvalError::MissingInput { node, pin })?;
        if let Some(src) = self.graph.nodes.get(from.node) {
            if let NodeKind::GraphRef(gr) = &src.kind {
                let name = self.graph_ref_output_name(from.node, from.pin)?;
                let up = self
                    .upstream
                    .ok_or(EvalError::UnresolvedGraphRef { node: from.node })?;
                return up.surface_field(gr.target, &name, from.node);
            }
        }
        let src = self
            .cache
            .get(from.node)
            .ok_or(EvalError::MissingOutput(from.node))?;
        match src.as_surface() {
            Some(f) => Ok(f.clone()),
            None => Err(EvalError::WrongInputType {
                node,
                expected: "surface",
                got: src.kind_name(),
            }),
        }
    }
    
    /// The boundary-output name a `GraphRef` node exposes an output `pin` (from
    /// its resolved pins). Errors if the node is unresolved or the pin is out of
    /// range.
    fn graph_ref_output_name(&self, graphref: NodeId, pin: u16) -> EvalResult<String> {
        let node = self.graph.nodes.get(graphref).ok_or(EvalError::MissingOutput(graphref))?;
        node.kind
            .effective_outputs()
            .get(pin as usize)
            .map(|p| p.name.to_string())
            .ok_or(EvalError::UnresolvedGraphRef { node: graphref })
    }

    /// Pointwise per-column evaluation at an absolute world column `(world_x,
    /// world_z)`, recomputed from the graph rather than read from the cache.
    ///
    /// Unlike the bulk fill, this is independent of which chunk the evaluator
    /// was built for: the result derives purely from the world column coords
    /// and the chunk-independent [`EvalContext::noise_seed`]. Two chunks that
    /// share a border therefore sample the same world column identically - the
    /// basis for cross-chunk biome-boundary blending.
    pub fn sample_column(
        &self,
        id: NodeId,
        world_x: i32,
        world_z: i32,
    ) -> EvalResult<ColumnSample> {
        let node = self.graph.nodes.get(id).ok_or(EvalError::MissingOutput(id))?;
        Ok(match &node.kind {
            NodeKind::SurfaceNoise(p) => {
                let n = configured_noise(self.ctx.noise_seed(p.seed), p, NoiseType::OpenSimplex2);
                ColumnSample::Surface(n.get_noise_2d(world_x as f32, world_z as f32))
            }
            NodeKind::WorldOutput(p) => {
                let v = self.sample_input_surface(id, 0, world_x, world_z)?;
                ColumnSample::Id(band_id(quantize_one(v, &p.zone_bands), &p.zone_ids))
            }
            NodeKind::ZoneOutput(p) => {
                let v = self.sample_input_surface(id, 0, world_x, world_z)?;
                ColumnSample::Id(band_id(quantize_one(v, &p.biome_bands), &p.biome_ids))
            }
            NodeKind::GraphOutput(_) => {
                ColumnSample::Surface(self.sample_input_surface(id, 0, world_x, world_z)?)
            }
            _ => return Err(EvalError::WrongGraphDomain { node: id }),
        })
    }

    /// Pointwise-resolve the surface value feeding `(node, pin)` at a world
    /// column. The pointwise analog of [`ColumnEvaluator::input_surface`].
    fn sample_input_surface(
        &self,
        node: NodeId,
        pin: u16,
        world_x: i32,
        world_z: i32,
    ) -> EvalResult<f32> {
        let from = self.edges.source(node, pin).ok_or(EvalError::MissingInput { node, pin })?;
        if let Some(src) = self.graph.nodes.get(from.node) {
            if let NodeKind::GraphRef(gr) = &src.kind {
                let name = self.graph_ref_output_name(from.node, from.pin)?;
                let up = self
                    .upstream
                    .ok_or(EvalError::UnresolvedGraphRef { node: from.node })?;
                return up.surface_sample(gr.target, &name, world_x, world_z, from.node);
            }
        }
        match self.sample_column(from.node, world_x, world_z)? {
            ColumnSample::Surface(v) => Ok(v),
            ColumnSample::Id(_) => Err(EvalError::WrongInputType {
                node,
                expected: "surface",
                got: "id",
            }),
        }
    }
}

/// A single column's value from a pointwise [`ColumnEvaluator::sample_column`].
/// The scalar analog of [`ColumnOutput`](crate::ColumnOutput).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnSample {
    /// A continuous per-column scalar (a climate channel value).
    Surface(f32),
    /// A discrete per-column id (zone or biome).
    Id(u16),
}

/// Cross-graph resolution context: for each referable hierarchy graph, its
/// graph + chunk context, its already-computed per-column cache, and the map
/// from boundary-output name to the `GraphOutput` node producing it. A
/// `GraphRef` read resolves against this - the bulk path clones the cached
/// column field; the pointwise path resamples the upstream graph
/// (chunk-independently, so seam-correct).
#[derive(Default)]
pub struct UpstreamGraphs<'g> {
    graphs: HashMap<GraphRefTarget, UpstreamGraph<'g>>,
}

/// One registered upstream graph within [`UpstreamGraphs`].
struct UpstreamGraph<'g> {
    cache: &'g ColumnCache,
    outputs: HashMap<String, NodeId>,
    /// Pointwise evaluator over the upstream graph, built once. `surface_sample`
    /// runs per column probed, so building one per call would put an
    /// `EdgeIndex` construction on a per-column path - which is how an index
    /// becomes a regression.
    point: ColumnEvaluator<'g>,
}

impl<'g> UpstreamGraphs<'g> {
    /// Empty context.
    pub fn new() -> Self {
        Self { graphs: HashMap::new() }
    }
    
    /// Register `graph` - already evaluated into `cache` for `ctx` - as the
    /// upstream for `target`. Its boundary outputs are indexed by name from its
    /// `GraphOutput` nodes.
    pub fn insert(
        &mut self,
        target: GraphRefTarget,
        graph: &'g Graph,
        ctx: EvalContext,
        cache: &'g ColumnCache,
    ) {
        let outputs = graph
            .nodes
            .iter()
            .filter_map(|(id, n)| match &n.kind {
                NodeKind::GraphOutput(p) => Some((p.name.clone(), id)),
                _ => None,
            })
            .collect();
        let point = ColumnEvaluator::new(graph, ctx);
        self.graphs.insert(target, UpstreamGraph { cache, outputs, point });
    }
    
    /// The bulk cached surface field of `target`'s named output. `graphref` is
    /// the referencing node, for error context.
    fn surface_field(
        &self,
        target: GraphRefTarget,
        name: &str,
        graphref: NodeId,
    ) -> EvalResult<Arc<ColumnField>> {
        let ug = self.graphs.get(&target).ok_or(EvalError::UnresolvedGraphRef { node: graphref })?;
        let node = *ug.outputs.get(name).ok_or(EvalError::UnresolvedGraphRef { node: graphref })?;
        match ug.cache.get(node) {
            Some(out) => out.as_surface().cloned().ok_or(EvalError::WrongInputType {
                node,
                expected: "surface",
                got: out.kind_name(),
            }),
            None => Err(EvalError::MissingOutput(node)),
        }
    }
    
    /// The pointwise surface value of `target`'s named output at a world column.
    fn surface_sample(
        &self,
        target: GraphRefTarget,
        name: &str,
        world_x: i32,
        world_z: i32,
        graphref: NodeId,
    ) -> EvalResult<f32> {
        let ug = self.graphs.get(&target).ok_or(EvalError::UnresolvedGraphRef { node: graphref })?;
        let node = *ug.outputs.get(name).ok_or(EvalError::UnresolvedGraphRef { node: graphref })?;
        match ug.point.sample_column(node, world_x, world_z)? {
            ColumnSample::Surface(v) => Ok(v),
            ColumnSample::Id(_) => Err(EvalError::WrongInputType {
                node,
                expected: "surface",
                got: "id",
            }),
        }
    }
}

/// The id a band-quantizing terminal assigns to one column: the number of
/// ascending `bands` thresholds the value meets or exceeds. Empty bands => 0.
fn quantize_one(value: f32, bands: &[f32]) -> u16 {
    bands.iter().filter(|&&b| value >= b).count() as u16
}

/// Map a band region index to its assigned id: explicit `ids[k]` when present,
/// else the region index itself (legacy behavior - graphs without an id map are
/// unchanged).
fn band_id(k: u16, ids: &[u16]) -> u16 {
    ids.get(k as usize).copied().unwrap_or(k)
}

/// Every id a band table can assign, `band_id`'s index fallback included.
/// 
/// `band_count` thresholds produce `band_count + 1` regions, and a short `ids`
/// table falls back to the band *index*. So `zone_bands: [0.0]` with no ids
/// assigns zones 0 **and** 1 - which is how a world silently assigned a zone it
/// had no graph for. One definition, because the evaluator, the generator's
/// startup validation, and the validation CLI all have to agree about it.
pub fn assignable_ids(band_count: usize, ids: &[u16]) -> Vec<u16> {
    let mut out: Vec<u16> = (0..=band_count).map(|k| band_id(k as u16, ids)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Quantize a per-column surface field into a discrete id column: band-index per
/// column via [`quantize_one`], then [`band_id`] to the assigned id. Shared by the
/// World (zone) and Zone (biome) terminals.
fn quantize_bands(field: &ColumnField, bands: &[f32], ids: &[u16]) -> IdColumn {
    let mut out = IdColumn::zeroed();
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            let k = quantize_one(field.get(x, z), bands);
            out.set(x, z, band_id(k, ids));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;
    use nodegraph_ir::{NoiseParams, PinRef, WorldOutputParams, ZoneOutputParams};

    /// WorldGraph shape: a SurfaceNoise climate channel driving a WorldOutput.
    fn world_graph() -> Graph {
        let mut g = Graph::new();
        let noise = g.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let out = g.add_node(NodeKind::WorldOutput(WorldOutputParams::default()));
        g.connect(PinRef::new(noise, 0), PinRef::new(out, 0)).unwrap();
        g
    }

    /// ZoneGraph shape: a SurfaceNoise climate channel driving a ZoneOutput
    /// with `biome_bands`.
    fn zone_graph(biome_bands: Vec<f32>) -> Graph {
        let mut g = Graph::new();
        let noise = g.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let out = g.add_node(NodeKind::ZoneOutput(ZoneOutputParams { biome_bands, ..Default::default() }));
        g.connect(PinRef::new(noise, 0), PinRef::new(out, 0)).unwrap();
        g
    }

    #[test]
    fn world_output_assigns_zone_zero_with_no_bands() {
        let g = world_graph();
        let out_id = g
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::WorldOutput(_)))
            .map(|(id, _)| id)
            .unwrap();
        let mut e = ColumnEvaluator::new(&g, EvalContext::new(7, IVec3::ZERO));
        e.evaluate().unwrap();
        let ids = e.cache().get(out_id).unwrap().as_id().unwrap();
        assert!(ids.data().iter().all(|&z| z == 0), "no bands => single zone 0");
    }

    #[test]
    fn surface_and_zone_columns_are_deterministic() {
        let g = world_graph();
        let run = || {
            let mut e = ColumnEvaluator::new(&g, EvalContext::new(42, IVec3::new(1, 0, 2)));
            e.evaluate().unwrap();
            e.into_cache()
        };
        let (a, b) = (run(), run());
        for (id, _) in g.nodes.iter() {
            match (a.get(id), b.get(id)) {
                (Some(ColumnOutput::Surface(fa)), Some(ColumnOutput::Surface(fb))) => {
                    assert_eq!(fa.data(), fb.data(), "surface field not deterministic");
                }
                (Some(ColumnOutput::Id(ia)), Some(ColumnOutput::Id(ib))) => {
                    assert_eq!(ia.data(), ib.data(), "id column not deterministic");
                }
                _ => panic!("cache mismatch between runs"),
            }
        }
    }

    #[test]
    fn voxel_node_in_column_graph_is_rejected() {
        let mut g = Graph::new();
        g.add_node(NodeKind::Constant(nodegraph_ir::ConstantParams::default()));
        let mut e = ColumnEvaluator::new(&g, EvalContext::new(0, IVec3::ZERO));
        assert!(matches!(e.evaluate(), Err(EvalError::WrongGraphDomain { .. })));
    }

    #[test]
    fn sample_column_matches_whole_chunk_fill() {
        // The pointwise sampler must agree with the bulk fill for the chunk's
        // own columns.
        let g = world_graph();
        let surf_id = g
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::SurfaceNoise(_)))
            .map(|(id, _)| id)
            .unwrap();
        let ctx = EvalContext::new(3, IVec3::new(2, 0, 1));
        let mut e = ColumnEvaluator::new(&g, ctx);
        e.evaluate().unwrap();
        let field = e.cache().get(surf_id).unwrap().as_surface().unwrap();
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let wx = ctx.chunk.x * CHUNK_DIM as i32 + x as i32;
                let wz = ctx.chunk.z * CHUNK_DIM as i32 + z as i32;
                match e.sample_column(surf_id, wx, wz).unwrap() {
                    ColumnSample::Surface(v) => assert_eq!(v, field.get(x, z)),
                    other => panic!("expected surface, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn sample_column_is_chunk_independent() {
        // Two evaluators built for different chunks must agree on any shared
        // world column - the cross-chunk seam guarantee.
        let g = zone_graph(vec![0.0]);
        let out_id = g
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::ZoneOutput(_)))
            .map(|(id, _)| id)
            .unwrap();
        let ea = ColumnEvaluator::new(&g, EvalContext::new(5, IVec3::new(0, 0, 0)));
        let eb = ColumnEvaluator::new(&g, EvalContext::new(5, IVec3::new(10, 0, -3)));
        for (wx, wz) in [(0, 0), (32, 0), (-1, 5), (1000, -250)] {
            let a = ea.sample_column(out_id, wx, wz).unwrap();
            let b = eb.sample_column(out_id, wx, wz).unwrap();
            assert_eq!(a, b, "world column ({wx}, {wz}) differs across chunks");
        }
    }

    #[test]
    fn zone_output_assigns_biome_zero_with_no_bands() {
        let g = zone_graph(vec![]);
        let out_id = g
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::ZoneOutput(_)))
            .map(|(id, _)| id)
            .unwrap();
        let mut e = ColumnEvaluator::new(&g, EvalContext::new(7, IVec3::ZERO));
        e.evaluate().unwrap();
        let ids = e.cache().get(out_id).unwrap().as_id().unwrap();
        assert!(ids.data().iter().all(|&b| b == 0), "no bands => single biome 0");
    }

    #[test]
    fn zone_output_with_one_band_is_deterministic_and_bounded() {
        // A single threshold partitions columns into biome 0 and biome 1.
        let g = zone_graph(vec![0.0]);
        let run = || {
            let mut e = ColumnEvaluator::new(&g, EvalContext::new(9, IVec3::new(3, 0, -1)));
            e.evaluate().unwrap();
            e.into_cache()
        };
        let out_id = g
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::ZoneOutput(_)))
            .map(|(id, _)| id)
            .unwrap();
        let (a, b) = (run(), run());
        let ia = a.get(out_id).unwrap().as_id().unwrap();
        let ib = b.get(out_id).unwrap().as_id().unwrap();
        assert_eq!(ia.data(), ib.data(), "biome column not deterministic");
        assert!(ia.data().iter().all(|&b| b <= 1), "one band => ids in {{0, 1}}");
    }

    #[test]
    fn graph_output_passes_its_input_through() {
        use nodegraph_ir::{GraphKind, GraphOutputParams};
        // SurfaceNoise -> GraphOutput("climate"): the marker's cached value must
        // equal the noise it wraps, both in bulk fill and pointwise.
        let mut g = Graph::of_kind(GraphKind::World);
        let noise = g.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let out = g.add_node(NodeKind::GraphOutput(GraphOutputParams { name: "climate".into() }));
        g.connect(PinRef::new(noise, 0), PinRef::new(out, 0)).unwrap();

        let ctx = EvalContext::new(4, IVec3::new(1, 0, -2));
        let mut e = ColumnEvaluator::new(&g, ctx);
        e.evaluate().unwrap();
        let noise_f = e.cache().get(noise).unwrap().as_surface().unwrap().clone();
        let out_f = e.cache().get(out).unwrap().as_surface().unwrap().clone();
        assert_eq!(noise_f.data(), out_f.data(), "GraphOutput must pass its input through");

        let wx = ctx.chunk.x * CHUNK_DIM as i32 + 5;
        let wz = ctx.chunk.z * CHUNK_DIM as i32 + 6;
        match e.sample_column(out, wx, wz).unwrap() {
            ColumnSample::Surface(v) => assert_eq!(v, out_f.get(5, 6)),
            other => panic!("expected surface, got {other:?}"),
        }
    }

    #[test]
    fn graph_ref_reads_upstream_climate() {
        use nodegraph_ir::{GraphKind, GraphOutputParams, GraphRefParams, ZoneOutputParams};
        // Upstream (World): SurfaceNoise -> GraphOutput("climate").
        let mut world = Graph::of_kind(GraphKind::World);
        let wn = world.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let wo = world.add_node(NodeKind::GraphOutput(GraphOutputParams { name: "climate".into() }));
        world.connect(PinRef::new(wn, 0), PinRef::new(wo, 0)).unwrap();
        world.derive_output_boundary();

        // Downstream (Zone): GraphRef(World).climate -> ZoneOutput(one band).
        let mut zone = Graph::of_kind(GraphKind::Zone);
        let gr = zone.add_node(NodeKind::GraphRef(GraphRefParams {
            target: GraphRefTarget::World,
            ..Default::default()
        }));
        let zo = zone.add_node(NodeKind::ZoneOutput(ZoneOutputParams { biome_bands: vec![0.0], ..Default::default() }));
        let world_boundary = world.boundary.clone();
        zone.resolve_graph_refs(|t| (t == GraphRefTarget::World).then(|| world_boundary.clone()));
        zone.connect(PinRef::new(gr, 0), PinRef::new(zo, 0)).unwrap();

        let ctx = EvalContext::new(9, IVec3::new(2, 0, -1));
        let mut we = ColumnEvaluator::new(&world, ctx);
        we.evaluate().unwrap();
        let world_cache = we.into_cache();
        let mut upstream = UpstreamGraphs::new();
        upstream.insert(GraphRefTarget::World, &world, ctx, &world_cache);

        let mut ze = ColumnEvaluator::new(&zone, ctx).with_upstream(&upstream);
        ze.evaluate().unwrap();
        let biome = ze.cache().get(zo).unwrap().as_id().unwrap();

        // Zone's biome ids must equal quantizing World's climate directly.
        let climate = world_cache.get(wo).unwrap().as_surface().unwrap();
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let expect = if climate.get(x, z) >= 0.0 { 1u16 } else { 0 };
                assert_eq!(biome.get(x, z), expect, "biome must follow World's climate");
            }
        }
    }

    #[test]
    fn pointwise_and_bulk_read_the_same_graph_ref_output() {
        use nodegraph_ir::{GraphKind, GraphOutputParams, GraphRefParams, ZoneOutputParams};
        // World exposes two boundary outputs. Sorted by name, "aridity" is
        // output pin 0 and "temperature" is pin 1.
        let mut world = Graph::of_kind(GraphKind::World);
        let n0 = world.add_node(NodeKind::SurfaceNoise(NoiseParams { seed: 1, ..Default::default() }));
        let o0 = world.add_node(NodeKind::GraphOutput(GraphOutputParams { name: "aridity".into() }));
        world.connect(PinRef::new(n0, 0), PinRef::new(o0, 0)).unwrap();
        let n1 = world.add_node(NodeKind::SurfaceNoise(NoiseParams { seed: 777, frequency: 0.05, ..Default::default() }));
        let o1 = world.add_node(NodeKind::GraphOutput(GraphOutputParams { name: "temperature".into() }));
        world.connect(PinRef::new(n1, 0), PinRef::new(o1, 0)).unwrap();
        world.derive_output_boundary();

        // Zone reads the *second* output pin. Source pin 1 into destination pin
        // 0 is the case the two paths used to disagree about.
        let mut zone = Graph::of_kind(GraphKind::Zone);
        let gr = zone.add_node(NodeKind::GraphRef(GraphRefParams {
            target: GraphRefTarget::World,
            ..Default::default()
        }));
        let zo = zone.add_node(NodeKind::ZoneOutput(ZoneOutputParams { biome_bands: vec![0.0], ..Default::default() }));
        let wb = world.boundary.clone();
        zone.resolve_graph_refs(|t| (t == GraphRefTarget::World).then(|| wb.clone()));
        zone.connect(PinRef::new(gr, 1), PinRef::new(zo, 0)).unwrap();

        let ctx = EvalContext::new(9, IVec3::new(2, 0, -1));
        let mut we = ColumnEvaluator::new(&world, ctx);
        we.evaluate().unwrap();
        let world_cache = we.into_cache();
        let mut upstream = UpstreamGraphs::new();
        upstream.insert(GraphRefTarget::World, &world, ctx, &world_cache);

        let mut ze = ColumnEvaluator::new(&zone, ctx).with_upstream(&upstream);
        ze.evaluate().unwrap();
        let bulk = ze.cache().get(zo).unwrap().as_id().unwrap();

        // Pointwise is how two chunks agree about a shared column, so it
        // disagreeing with the bulk fill is a seam defect, not a detail.
        let mut disagreements = 0;
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let wx = ctx.chunk.x * CHUNK_DIM as i32 + x as i32;
                let wz = ctx.chunk.z * CHUNK_DIM as i32 + z as i32;
                match ze.sample_column(zo, wx, wz).unwrap() {
                    ColumnSample::Id(v) => {
                        if v != bulk.get(x, z) {
                            disagreements += 1;
                        }
                    }
                    other => panic!("expected id, got {other:?}"),
                }
            }
        }
        assert_eq!(disagreements, 0, "pointwise disagreed with bulk on {disagreements} of 1024 columns");
    }
}
