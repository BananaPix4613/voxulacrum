//! Phase 6: deterministic math + curve node tests.

use glam::IVec3;
use nodegraph_eval::*;
use nodegraph_ir::*;

const CTX: EvalContext = EvalContext { world_seed: 0, chunk: IVec3::ZERO };

fn eval_uniform(graph: Graph, target: NodeId) -> f32 {
    let mut eval = Evaluator::new(&graph, CTX);
    eval.evaluate().unwrap();
    eval.cache().get(target).unwrap().as_scalar().unwrap().get(0, 0, 0)
}

fn two_const_op(a: f32, b: f32, kind: NodeKind) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let ka = g.add_node(NodeKind::Constant(ConstantParams { value: a }));
    let kb = g.add_node(NodeKind::Constant(ConstantParams { value: b }));
    let op = g.add_node(kind);
    g.connect(PinRef::new(ka, 0), PinRef::new(op, 0)).unwrap();
    g.connect(PinRef::new(kb, 0), PinRef::new(op, 1)).unwrap();
    (g, op)
}

#[test] fn add()      { let (g, n) = two_const_op(3.0, 4.0, NodeKind::Add(AddParams::default()));           assert_eq!(eval_uniform(g, n),  7.0); }
#[test] fn multiply() { let (g, n) = two_const_op(3.0, 4.0, NodeKind::Multiply(MultiplyParams::default())); assert_eq!(eval_uniform(g, n), 12.0); }
#[test] fn subtract() { let (g, n) = two_const_op(3.0, 4.0, NodeKind::Subtract(SubtractParams::default())); assert_eq!(eval_uniform(g, n), -1.0); }
#[test] fn min()      { let (g, n) = two_const_op(3.0, 4.0, NodeKind::Min(MinParams::default()));           assert_eq!(eval_uniform(g, n),  3.0); }
#[test] fn max()      { let (g, n) = two_const_op(3.0, 4.0, NodeKind::Max(MaxParams::default()));           assert_eq!(eval_uniform(g, n),  4.0); }

#[test]
fn clamp() {
    let mut g = Graph::new();
    let k = g.add_node(NodeKind::Constant(ConstantParams { value: 5.0 }));
    let c = g.add_node(NodeKind::Clamp(ClampParams { min: 0.0, max: 1.0 }));
    g.connect(PinRef::new(k, 0), PinRef::new(c, 0)).unwrap();
    assert_eq!(eval_uniform(g, c), 1.0);
}

#[test]
fn lerp_half() {
    let mut g = Graph::new();
    let a = g.add_node(NodeKind::Constant(ConstantParams { value: 10.0 }));
    let b = g.add_node(NodeKind::Constant(ConstantParams { value: 20.0 }));
    let t = g.add_node(NodeKind::Constant(ConstantParams { value: 0.5 }));
    let l = g.add_node(NodeKind::Lerp(LerpParams::default()));
    g.connect(PinRef::new(a, 0), PinRef::new(l, 0)).unwrap();
    g.connect(PinRef::new(b, 0), PinRef::new(l, 1)).unwrap();
    g.connect(PinRef::new(t, 0), PinRef::new(l, 2)).unwrap();
    assert_eq!(eval_uniform(g, l), 15.0);
}

#[test]
fn remap_default_negone_one_to_zero_one() {
    let mut g = Graph::new();
    let k = g.add_node(NodeKind::Constant(ConstantParams { value: 0.0 })); // midpoint of [-1,1]
    let r = g.add_node(NodeKind::Remap(RemapParams::default()));
    g.connect(PinRef::new(k, 0), PinRef::new(r, 0)).unwrap();
    assert_eq!(eval_uniform(g, r), 0.5); // midpoint of [0,1]
}

#[test]
fn curve_mapper_piecewise_linear() {
    let mut g = Graph::new();
    let k = g.add_node(NodeKind::Constant(ConstantParams { value: 0.25 }));
    let c = g.add_node(NodeKind::CurveMapper(CurveMapperParams {
        stops: vec![(0.0, 0.0), (1.0, 4.0)],
    }));
    g.connect(PinRef::new(k, 0), PinRef::new(c, 0)).unwrap();
    assert!((eval_uniform(g, c) - 1.0).abs() < 1e-6);
}

#[test]
fn curve_mapper_clamps_at_endpoints() {
    let mut g = Graph::new();
    let k = g.add_node(NodeKind::Constant(ConstantParams { value: -5.0 }));
    let c = g.add_node(NodeKind::CurveMapper(CurveMapperParams {
        stops: vec![(0.0, 7.0), (1.0, 9.0)],
    }));
    g.connect(PinRef::new(k, 0), PinRef::new(c, 0)).unwrap();
    assert_eq!(eval_uniform(g, c), 7.0);
}

#[test]
fn threshold_below_above() {
    let mut g = Graph::new();
    let k = g.add_node(NodeKind::Constant(ConstantParams { value: 0.4 }));
    let t = g.add_node(NodeKind::Threshold(ThresholdParams { threshold: 0.5 }));
    g.connect(PinRef::new(k, 0), PinRef::new(t, 0)).unwrap();
    assert_eq!(eval_uniform(g, t), 0.0);

    let mut g = Graph::new();
    let k = g.add_node(NodeKind::Constant(ConstantParams { value: 0.7 }));
    let t = g.add_node(NodeKind::Threshold(ThresholdParams { threshold: 0.5 }));
    g.connect(PinRef::new(k, 0), PinRef::new(t, 0)).unwrap();
    assert_eq!(eval_uniform(g, t), 1.0);
}
