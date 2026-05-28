//! Phase 2 deliverable + validation tests.

use nodegraph_ir::*;

fn build_three_node_graph() -> (Graph, NodeId, NodeId, NodeId) {
    let mut g = Graph::new();
    let perlin = g.add_node(NodeKind::Perlin2D(Perlin2DParams::default()));
    let thresh = g.add_node(NodeKind::Threshold(ThresholdParams { threshold: 0.3 }));
    let out = g.add_node(NodeKind::Output(OutputParams::default()));
    g.connect(PinRef::new(perlin, 0), PinRef::new(thresh, 0)).unwrap();
    g.connect(PinRef::new(thresh, 0), PinRef::new(out, 0)).unwrap();
    (g, perlin, thresh, out)
}

#[test]
fn three_node_graph_validates_clean() {
    let (g, ..) = build_three_node_graph();
    let diags = g.validate();
    assert!(
        diags.iter().all(|d| d.severity != Severity::Error),
        "unexpected errors: {diags:?}"
    );
}

#[test]
fn json_round_trip_is_stable_and_valid() {
    let (g, ..) = build_three_node_graph();
    let json1 = g.to_json_pretty().unwrap();
    let g2 = Graph::from_json(&json1).unwrap();
    let json2 = g2.to_json_pretty().unwrap();
    // Structurally identical after a round trip (proves slotmap keys survived).
    assert_eq!(json1, json2);
    // Edges still resolve against the reloaded nodes.
    assert!(!g2.has_errors());
}

#[test]
fn node_serializes_with_type_tag() {
    let (g, ..) = build_three_node_graph();
    let json = g.to_json().unwrap();
    assert!(json.contains("\"type\":\"Perlin2D\""));
    assert!(json.contains("\"type\":\"Threshold\""));
    assert!(json.contains("\"type\":\"Output\""));
}

#[test]
fn connecting_from_node_without_output_errors() {
    let mut g = Graph::new();
    let out = g.add_node(NodeKind::Output(OutputParams::default())); // no outputs
    let thresh = g.add_node(NodeKind::Threshold(ThresholdParams::default()));
    let res = g.connect(PinRef::new(out, 0), PinRef::new(thresh, 0));
    assert!(matches!(res, Err(GraphError::PinOutOfRange { .. })));
}

#[test]
fn duplicate_input_connection_rejected() {
    let mut g = Graph::new();
    let p1 = g.add_node(NodeKind::Perlin2D(Perlin2DParams::default()));
    let p2 = g.add_node(NodeKind::Perlin2D(Perlin2DParams::default()));
    let t = g.add_node(NodeKind::Threshold(ThresholdParams::default()));
    g.connect(PinRef::new(p1, 0), PinRef::new(t, 0)).unwrap();
    let dup = g.connect(PinRef::new(p2, 0), PinRef::new(t, 0));
    assert!(matches!(dup, Err(GraphError::InputAlreadyConnected { .. })));
}

#[test]
fn missing_required_input_flagged() {
    let mut g = Graph::new();
    g.add_node(NodeKind::Threshold(ThresholdParams::default())); // required input unconnected
    let diags = g.validate();
    assert!(diags
        .iter()
        .any(|d| d.severity == Severity::Error && d.message.contains("required input")));
}

#[test]
fn cycle_detected() {
    let mut g = Graph::new();
    let a = g.add_node(NodeKind::Threshold(ThresholdParams::default()));
    let b = g.add_node(NodeKind::Threshold(ThresholdParams::default()));
    g.connect(PinRef::new(a, 0), PinRef::new(b, 0)).unwrap();
    g.connect(PinRef::new(b, 0), PinRef::new(a, 0)).unwrap();
    let diags = g.validate();
    assert!(diags.iter().any(|d| d.message.contains("cycle")));
}
