//! Biome-graph evaluation bench. Tracks the spec's `<50 ms per 32³ chunk`
//! success criterion.

use criterion::{criterion_group, criterion_main, Criterion};
use glam::IVec3;
use nodegraph_eval::*;
use nodegraph_ir::*;

fn biome_graph() -> Graph {
    // Inline copy of `bootstrap_example_graph`'s body — the same biome graph,
    // built directly (no FS).
    let mut g = Graph::new();
    let pos = g.add_node(NodeKind::WorldPos(WorldPosParams::default()));
    let warp = g.add_node(NodeKind::DomainWarp(DomainWarpParams { seed: 11, frequency: 0.01, amplitude: 12.0 }));
    g.connect(PinRef::new(pos, 0), PinRef::new(warp, 0)).unwrap();
    let continental = g.add_node(NodeKind::Perlin2D(NoiseParams { seed: 1, frequency: 0.005, octaves: 5, lacunarity: 2.0, gain: 0.5, fractal_type: FractalType::FBm }));
    g.connect(PinRef::new(warp, 0), PinRef::new(continental, 0)).unwrap();
    let shaped = g.add_node(NodeKind::CurveMapper(CurveMapperParams { stops: vec![(-1.0,-0.6),(-0.2,-0.1),(0.1,0.1),(0.6,0.6),(1.0,0.9)] }));
    g.connect(PinRef::new(continental, 0), PinRef::new(shaped, 0)).unwrap();
    let erosion = g.add_node(NodeKind::Perlin2D(NoiseParams { seed: 2, frequency: 0.02, octaves: 4, lacunarity: 2.0, gain: 0.5, fractal_type: FractalType::FBm }));
    g.connect(PinRef::new(warp, 0), PinRef::new(erosion, 0)).unwrap();
    let ridges = g.add_node(NodeKind::Perlin2D(NoiseParams { seed: 3, frequency: 0.04, octaves: 4, lacunarity: 2.0, gain: 0.5, fractal_type: FractalType::Ridged }));
    g.connect(PinRef::new(warp, 0), PinRef::new(ridges, 0)).unwrap();
    let scale = g.add_node(NodeKind::Constant(ConstantParams { value: 0.35 }));
    let ridges_scaled = g.add_node(NodeKind::Multiply(MultiplyParams::default()));
    g.connect(PinRef::new(ridges, 0), PinRef::new(ridges_scaled, 0)).unwrap();
    g.connect(PinRef::new(scale, 0), PinRef::new(ridges_scaled, 1)).unwrap();
    let detail = g.add_node(NodeKind::Add(AddParams::default()));
    g.connect(PinRef::new(erosion, 0), PinRef::new(detail, 0)).unwrap();
    g.connect(PinRef::new(ridges_scaled, 0), PinRef::new(detail, 1)).unwrap();
    let final_d = g.add_node(NodeKind::Add(AddParams::default()));
    g.connect(PinRef::new(shaped, 0), PinRef::new(final_d, 0)).unwrap();
    g.connect(PinRef::new(detail, 0), PinRef::new(final_d, 1)).unwrap();
    let out = g.add_node(NodeKind::Output(OutputParams::default()));
    g.connect(PinRef::new(final_d, 0), PinRef::new(out, 0)).unwrap();
    g
}

fn bench(c: &mut Criterion) {
    let g = biome_graph();
    c.bench_function("biome_chunk_eval", |b| {
        b.iter(|| {
            let mut e = Evaluator::new(&g, EvalContext::new(42, IVec3::ZERO));
            e.evaluate().unwrap();
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
