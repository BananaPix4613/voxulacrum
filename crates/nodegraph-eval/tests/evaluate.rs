//! Phase 3 deliverable + cross-validation tests.

use glam::IVec3;
use nodegraph_eval::*;
use nodegraph_ir::*;

/// Perlin2D + Constant bias → Add → Output.
fn heightmap_graph() -> (Graph, NodeId) {
    let mut g = Graph::new();
    let perlin = g.add_node(NodeKind::Perlin2D(Perlin2DParams {
        seed: 1337,
        frequency: 0.05,
        octaves: 4,
        lacunarity: 2.0,
        gain: 0.5,
        ..Perlin2DParams::default()
    }));
    let bias = g.add_node(NodeKind::Constant(ConstantParams { value: 0.2 }));
    let add = g.add_node(NodeKind::Add(AddParams::default()));
    let out = g.add_node(NodeKind::Output(OutputParams::default()));
    g.connect(PinRef::new(perlin, 0), PinRef::new(add, 0)).unwrap();
    g.connect(PinRef::new(bias, 0), PinRef::new(add, 1)).unwrap();
    g.connect(PinRef::new(add, 0), PinRef::new(out, 0)).unwrap();
    (g, out)
}

#[test]
fn evaluate_and_snapshot_heightmap() {
    let (g, out) = heightmap_graph();
    let mut eval = Evaluator::new(&g, EvalContext::new(42, IVec3::ZERO));
    eval.evaluate().unwrap();
    let field = eval.cache().get(out).unwrap().as_scalar().unwrap();
    let ascii = ascii_heightmap(field, 0, (-1.0, 1.5));
    insta::assert_snapshot!(ascii);
}

#[test]
fn fill_matches_sample() {
    let (g, out) = heightmap_graph();
    let mut eval = Evaluator::new(&g, EvalContext::new(42, IVec3::ZERO));
    eval.evaluate().unwrap();
    let field = eval.cache().get(out).unwrap().as_scalar().unwrap();
    for (x, y, z) in [(0, 0, 0), (5, 0, 7), (31, 0, 31), (13, 0, 2)] {
        let s = eval.sample_density(out, x, y, z).unwrap();
        assert!((field.get(x, y, z) - s).abs() < 1e-6, "mismatch at {x},{y},{z}");
    }
}

#[test]
fn noise_seed_is_chunk_independent() {
    // The same params yield the same noise seed regardless of chunk — the
    // property that keeps noise fields continuous across chunk borders.
    let a = EvalContext::new(42, IVec3::ZERO);
    let b = EvalContext::new(42, IVec3::new(5, -3, 7));
    assert_eq!(a.noise_seed(1337), b.noise_seed(1337));
}

#[test]
fn world_pos_offsets_by_chunk() {
    let ctx = EvalContext::new(0, IVec3::new(1, 0, 0));
    let p = ctx.world_pos(0, 0, 0);
    assert_eq!(p, glam::Vec3::new(CHUNK_DIM as f32, 0.0, 0.0));
}

#[test]
fn worldpos_node_produces_vec3() {
    let mut g = Graph::new();
    let wp = g.add_node(NodeKind::WorldPos(WorldPosParams::default()));
    let mut eval = Evaluator::new(&g, EvalContext::new(0, IVec3::new(2, 0, 0)));
    eval.evaluate().unwrap();
    let f = eval.cache().get(wp).unwrap().as_vec3().unwrap();
    assert_eq!(f.get(0, 0, 0), glam::Vec3::new(2.0 * CHUNK_DIM as f32, 0.0, 0.0));
}

#[test]
fn writes_png_artifact() {
    let (g, out) = heightmap_graph();
    let mut eval = Evaluator::new(&g, EvalContext::new(42, IVec3::ZERO));
    eval.evaluate().unwrap();
    let field = eval.cache().get(out).unwrap().as_scalar().unwrap();
    let path = std::env::temp_dir().join("voxulacrum_phase3_heightmap.png");
    write_heightmap_png(field, 0, (-1.0, 1.5), &path).unwrap();
    assert!(path.exists());
}
