//! Pointwise-probe bench.
//!
//! The bulk fill (`biome.rs`) evaluates a whole chunk once. This measures the
//! *other* path: `sample_density` and `sample_column`, which recompute one
//! position from the graph and are what a chunk-Y seam, a border scan and a
//! feature probe run per column.
//!
//! Two of the cases load the graphs actually shipped in `assets/`, so the
//! numbers describe today's content. The synthetic chains exist because that
//! path's cost is quadratic in graph size - one recursion per node, and an
//! input resolution per recursion - and the shipped graphs are far too small to
//! show it.

use criterion::{criterion_group, criterion_main, Criterion};
use glam::IVec3;
use nodegraph_eval::*;
use nodegraph_ir::*;
use std::hint::black_box;

/// A graph from `assets/graphs`.
fn load(name: &str) -> Graph {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/graphs/").to_string() + name;
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn find(g: &Graph, pred: fn(&NodeKind) -> bool) -> NodeId {
    g.nodes.iter().find(|(_, n)| pred(&n.kind)).map(|(id, _)| id).expect("no such node")
}

/// The node feeding `(node, 0)`, or `node` itself if that pin is unconnected.
fn upstream_of(g: &Graph, node: NodeId) -> NodeId {
    g.edges
        .iter()
        .find(|e| e.to.node == node && e.to.pin == 0)
        .map(|e| e.from.node)
        .unwrap_or(node)
}

/// `chain` Add nodes in series, each with its own Constant, fed by one noise
/// source and ending in an Output. Edge count is `2 * chain + 1`.
fn chain_graph(chain: usize) -> Graph {
    let mut g = Graph::new();
    let mut acc = g.add_node(NodeKind::Perlin2D(NoiseParams {
        seed: 1,
        frequency: 0.01,
        octaves: 2,
        lacunarity: 2.0,
        gain: 0.5,
        fractal_type: FractalType::FBm,
    }));
    for i in 0..chain {
        let k = g.add_node(NodeKind::Constant(ConstantParams { value: i as f32 * 0.001 }));
        let add = g.add_node(NodeKind::Add(AddParams::default()));
        g.connect(PinRef::new(acc, 0), PinRef::new(add, 0)).unwrap();
        g.connect(PinRef::new(k, 0), PinRef::new(add, 1)).unwrap();
        acc = add;
    }
    let out = g.add_node(NodeKind::Output(OutputParams::default()));
    g.connect(PinRef::new(acc, 0), PinRef::new(out, 0)).unwrap();
    g
}

/// World: `SurfaceNoise -> GraphOutput("climate")`. The upstream half of the
/// cross-graph case, built rather than loaded so the bench keeps measuring the
/// same shape when content changes.
fn climate_world() -> Graph {
    let mut g = Graph::of_kind(GraphKind::World);
    let n = g.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
    let o = g.add_node(NodeKind::GraphOutput(GraphOutputParams { name: "climate".into() }));
    g.connect(PinRef::new(n, 0), PinRef::new(o, 0)).unwrap();
    g.derive_output_boundary();
    g
}

/// Zone: `GraphRef(World).climate -> ZoneOutput`.
fn climate_zone(world: &Graph) -> (Graph, NodeId) {
    let mut g = Graph::of_kind(GraphKind::Zone);
    let gr = g.add_node(NodeKind::GraphRef(GraphRefParams {
        target: GraphRefTarget::World,
        ..Default::default()
    }));
    let zo = g.add_node(NodeKind::ZoneOutput(ZoneOutputParams {
        biome_bands: vec![0.0],
        ..Default::default()
    }));
    let wb = world.boundary.clone();
    g.resolve_graph_refs(|t| (t == GraphRefTarget::World).then(|| wb.clone()));
    g.connect(PinRef::new(gr, 0), PinRef::new(zo, 0)).unwrap();
    (g, zo)
}

fn bench(c: &mut Criterion) {
    let ctx = EvalContext::new(42, IVec3::ZERO);

    // --- Voxel domain, shipped content. The seam continuation samples the
    // density chain, not the terminal, so step back off `DensityOutput`.
    let meadow = load("biome_meadow.graph.json");
    let meadow_term = find(&meadow, |k| {
        matches!(k, NodeKind::DensityOutput(_) | NodeKind::Output(_))
    });
    let meadow_density = upstream_of(&meadow, meadow_term);
    c.bench_function("sample_density/biome_meadow/1024", |b| {
        let e = Evaluator::new(&meadow, ctx);
        b.iter(|| {
            let mut acc = 0.0f32;
            for z in 0..32 {
                for x in 0..32 {
                    acc += e.sample_density(meadow_density, x, 16, z).unwrap();
                }
            }
            black_box(acc)
        });
    });

    // --- Column domain, shipped content: the zone-assignment probe.
    let world = load("world.graph.json");
    let world_out = find(&world, |k| matches!(k, NodeKind::WorldOutput(_)));
    c.bench_function("sample_column/world/1024", |b| {
        let e = ColumnEvaluator::new(&world, ctx);
        b.iter(|| {
            let mut acc = 0u32;
            for z in 0..32 {
                for x in 0..32 {
                    if let ColumnSample::Id(v) = e.sample_column(world_out, x, z).unwrap() {
                        acc += v as u32;
                    }
                }
            }
            black_box(acc)
        });
    });

    // --- Column domain across a graph boundary. Separate because a `GraphRef`
    // read leaves this graph and resamples the upstream one per column, which
    // is a different cost from anything above.
    let cw = climate_world();
    let (cz, cz_out) = climate_zone(&cw);
    let mut we = ColumnEvaluator::new(&cw, ctx);
    we.evaluate().unwrap();
    let world_cache = we.into_cache();
    let mut upstream = UpstreamGraphs::new();
    upstream.insert(GraphRefTarget::World, &cw, ctx, &world_cache, None);
    c.bench_function("sample_column/zone_via_graph_ref/1024", |b| {
        let e = ColumnEvaluator::new(&cz, ctx).with_upstream(&upstream);
        b.iter(|| {
            let mut acc = 0u32;
            for z in 0..32 {
                for x in 0..32 {
                    if let ColumnSample::Id(v) = e.sample_column(cz_out, x, z).unwrap() {
                        acc += v as u32;
                    }
                }
            }
            black_box(acc)
        });
    });

    // --- Scaling in graph size: identical arithmetic, more nodes to walk.
    for chain in [4usize, 16, 64] {
        let g = chain_graph(chain);
        let out = find(&g, |k| matches!(k, NodeKind::Output(_)));
        c.bench_function(&format!("sample_density/chain_{}edges/1024", 2 * chain + 1), |b| {
            let e = Evaluator::new(&g, ctx);
            b.iter(|| {
                let mut acc = 0.0f32;
                for z in 0..32 {
                    for x in 0..32 {
                        acc += e.sample_density(out, x, 16, z).unwrap();
                    }
                }
                black_box(acc)
            });
        });
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
