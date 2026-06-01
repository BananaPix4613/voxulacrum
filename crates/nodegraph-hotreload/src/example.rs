//! Bootstrap helper: write a default example graph if a watched dir is empty.

use std::path::{Path, PathBuf};

use nodegraph_ir::{AddParams, ConstantParams, CurveMapperParams, DomainWarpParams, FractalType, Graph, MultiplyParams, NodeKind, NoiseParams, OutputParams, PinRef, WorldPosParams};

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

    // Source position field, warped before consumption.
    let pos = g.add_node(NodeKind::WorldPos(WorldPosParams::default()));
    let warp = g.add_node(NodeKind::DomainWarp(DomainWarpParams {
        seed: 11, frequency: 0.01, amplitude: 12.0,
    }));
    g.connect(PinRef::new(pos, 0), PinRef::new(warp, 0)).expect("pos -> warp");

    // Continentalness: very low frequency, FBm.
    let continental = g.add_node(NodeKind::Perlin2D(NoiseParams {
        seed: 1, frequency: 0.005, octaves: 5, lacunarity: 2.0, gain: 0.5,
        fractal_type: FractalType::FBm,
    }));
    g.connect(PinRef::new(warp, 0), PinRef::new(continental, 0)).expect("warp -> continental");

    // Shape continentalness into a flatter-then-rising profile.
    let shaped = g.add_node(NodeKind::CurveMapper(CurveMapperParams {
        stops: vec![(-1.0, -0.6), (-0.2, -0.1), (0.1, 0.1), (0.6, 0.6), (1.0, 0.9)],
    }));
    g.connect(PinRef::new(continental, 0), PinRef::new(shaped, 0)).expect("continental -> shaped");

    // Erosion: mid frequency, FBm.
    let erosion = g.add_node(NodeKind::Perlin2D(NoiseParams {
        seed: 2, frequency: 0.02, octaves: 4, lacunarity: 2.0, gain: 0.5,
        fractal_type: FractalType::FBm,
    }));
    g.connect(PinRef::new(warp, 0), PinRef::new(erosion, 0)).expect("warp -> erosion");

    // Ridges: same family, Ridged fractal — high-frequency relief.
    let ridges = g.add_node(NodeKind::Perlin2D(NoiseParams {
        seed: 3, frequency: 0.04, octaves: 4, lacunarity: 2.0, gain: 0.5,
        fractal_type: FractalType::Ridged,
    }));
    g.connect(PinRef::new(warp, 0), PinRef::new(ridges, 0)).expect("warp -> ridges");

    // Scale ridges down and add into erosion to make detailed relief.
    let ridge_scale = g.add_node(NodeKind::Constant(ConstantParams { value: 0.35 }));
    let ridges_scaled = g.add_node(NodeKind::Multiply(MultiplyParams::default()));
    g.connect(PinRef::new(ridges, 0), PinRef::new(ridges_scaled, 0)).expect("ridges -> mul a");
    g.connect(PinRef::new(ridge_scale, 0), PinRef::new(ridges_scaled, 1)).expect("0.35 -> mul b");

    let detail = g.add_node(NodeKind::Add(AddParams::default()));
    g.connect(PinRef::new(erosion, 0), PinRef::new(detail, 0)).expect("erosion -> add a");
    g.connect(PinRef::new(ridges_scaled, 0), PinRef::new(detail, 1)).expect("ridges*0.35 -> add b");

    // Final = shapedContinental + detail.
    let final_density = g.add_node(NodeKind::Add(AddParams::default()));
    g.connect(PinRef::new(shaped, 0), PinRef::new(final_density, 0)).expect("shaped -> final a");
    g.connect(PinRef::new(detail, 0), PinRef::new(final_density, 1)).expect("detail -> final b");

    let out = g.add_node(NodeKind::Output(OutputParams::default()));
    g.connect(PinRef::new(final_density, 0), PinRef::new(out, 0)).expect("final -> out");
    
    let path = dir.join("example.graph.json");
    let json = g
        .to_json_pretty()
        .expect("default graph serializes");
    std::fs::write(&path, json)?;
    log::info!("bootstrapped example at {}", path.display());
    Ok(Some(path))
}
