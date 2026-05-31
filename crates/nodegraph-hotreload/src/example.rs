//! Bootstrap helper: write a default example graph if a watched dir is empty.

use std::path::{Path, PathBuf};

use nodegraph_ir::{
    AddParams, ConstantParams, Graph, NodeKind,
    OutputParams, Perlin2DParams, PinRef,
};

use crate::error::HotReloadResult;

/// If `dir` has no `*.json` files, write a default `example.graph.json` and
/// return its path. Returns `Ok(None)` if the dir already has graphs.
///
/// The example is the same Perlin+Constant+Add+Output graph as the Phase 3
/// snapshot test, serialized through `Graph::to_json_pretty` so the slotmap
/// key encoding is always correct.
pub fn bootstrap_example_graph(dir: &Path) -> HotReloadResult<Option<PathBuf>> {
    let already_has_json = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .any(|e| e.path().extension().is_some_and(|x| x == "json"));
    if already_has_json {
        return Ok(None);
    }
    
    let mut g = Graph::new();
    let perlin = g.add_node(NodeKind::Perlin2D(Perlin2DParams {
        seed: 1337,
        frequency: 0.05,
        octaves: 4,
        lacunarity: 2.0,
        gain: 0.5,
    }));
    let bias = g.add_node(NodeKind::Constant(ConstantParams { value: 0.2 }));
    let add = g.add_node(NodeKind::Add(AddParams::default()));
    let out = g.add_node(NodeKind::Output(OutputParams::default()));
    // connect() returns GraphError; can't be `?`'d into HotReloadResult.
    // None of these can fail on this hand-built graph, so expect is fine.
    g.connect(PinRef::new(perlin, 0), PinRef::new(add, 0))
        .expect("perlin -> add(0)");
    g.connect(PinRef::new(bias, 0), PinRef::new(add, 1))
        .expect("bias -> add(1)");
    g.connect(PinRef::new(add, 0), PinRef::new(out, 0))
        .expect("add -> out");
    
    let path = dir.join("example.graph.json");
    let json = g
        .to_json_pretty()
        .expect("default graph serializes");
    std::fs::write(&path, json)?;
    log::info!("bootstrapped example at {}", path.display());
    Ok(Some(path))
}
