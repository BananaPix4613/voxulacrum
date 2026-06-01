//! DomainWarp with amplitude=0 must be a no-op (identity warp).

use glam::IVec3;
use nodegraph_eval::*;
use nodegraph_ir::*;

#[test]
fn zero_amplitude_warp_is_identity() {
    fn perlin_field(via_warp: bool) -> std::sync::Arc<ScalarField> {
        let mut g = Graph::new();
        let perlin = g.add_node(NodeKind::Perlin2D(NoiseParams {
            seed: 7, frequency: 0.05, octaves: 3, lacunarity: 2.0, gain: 0.5,
            fractal_type: FractalType::FBm,
        }));
        if via_warp {
            let pos = g.add_node(NodeKind::WorldPos(WorldPosParams::default()));
            let warp = g.add_node(NodeKind::DomainWarp(DomainWarpParams {
                seed: 99, frequency: 0.05, amplitude: 0.0,
            }));
            g.connect(PinRef::new(pos, 0), PinRef::new(warp, 0)).unwrap();
            g.connect(PinRef::new(warp, 0), PinRef::new(perlin, 0)).unwrap();
        }
        let mut eval = Evaluator::new(&g, EvalContext::new(0, IVec3::ZERO));
        eval.evaluate().unwrap();
        match eval.cache().get(perlin).unwrap() {
            CachedOutput::Scalar(f) => f.clone(),
            _ => unreachable!(),
        }
    }

    let direct = perlin_field(false);
    let warped = perlin_field(true);
    assert_eq!(direct.data(), warped.data());
}
