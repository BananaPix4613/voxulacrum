//! Bootstrap helper: write a default example graph if a watched dir is empty.
//!
//! The default is a "rolling hills + pockets" graph that exercises every
//! Phase-8 pin type (Vec3 → Density × 2 → Curve → Math → Material → Terrain),
//! and produces visually interesting 3D voxel output thanks to a Perlin3D
//! source layered on top of a 2D continentalness signal.

use std::path::{Path, PathBuf};

use nodegraph_ir::{AddParams, BuildTerrainParams, ConstantParams, CurveMapperParams, DomainWarpParams, FractalType, Graph, LayerParams, MultiplyParams, NodeKind, NoiseParams, OutputParams, Perlin2DParams, PinRef, SlopeRefinerParams, TerrainOutputParams, WorldPosParams};
use voxel_core::MaterialId;

use crate::error::HotReloadResult;

/// If `dir` has no `*.json` files, write a default `example.graph.json` and
/// return its path. Returns `Ok(None)` if the dir already has graphs.
pub fn bootstrap_example_graph(dir: &Path) -> HotReloadResult<Option<PathBuf>> {
    let already_has_json = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .any(|e| e.path().extension().is_some_and(|x| x == "json"));
    if already_has_json {
        return Ok(None);
    }

    let path = dir.join("example.graph.json");
    let graph = build_example_graph();
    let json = graph
        .to_json_pretty()
        .expect("default graph serializes");
    std::fs::write(&path, json)?;
    log::info!("bootstrapped example at {}", path.display());
    Ok(Some(path))
}

/// The actual graph builder, factored out so tools / regen binaries can
/// rebuild the canonical example without going through the dir check.
pub fn build_example_graph() -> Graph {
    let mut g = Graph::new();

    // --- World position → domain-warped position field --------------------
    let pos = g.add_node(NodeKind::WorldPos(WorldPosParams::default()));
    let warp = g.add_node(NodeKind::DomainWarp(DomainWarpParams {
        seed: 11,
        frequency: 0.005,
        amplitude: 6.0,
    }));
    g.connect(PinRef::new(pos, 0), PinRef::new(warp, 0)).expect("pos -> warp");

    // --- 3D density: gives the chunk real volumetric variation ------------
    let perlin3d = g.add_node(NodeKind::Perlin3D(NoiseParams {
        seed: 1,
        frequency: 0.045,
        octaves: 4,
        lacunarity: 2.0,
        gain: 0.5,
        fractal_type: FractalType::FBm,
    }));
    g.connect(PinRef::new(warp, 0), PinRef::new(perlin3d, 0))
        .expect("warp -> perlin3d");

    // --- 2D continentalness, steepened through a curve --------------------
    let continent = g.add_node(NodeKind::Perlin2D(Perlin2DParams {
        seed: 2,
        frequency: 0.008,
        octaves: 5,
        lacunarity: 2.0,
        gain: 0.5,
        fractal_type: FractalType::FBm,
    }));
    g.connect(PinRef::new(warp, 0), PinRef::new(continent, 0))
        .expect("warp -> continent");

    let shaped = g.add_node(NodeKind::CurveMapper(CurveMapperParams {
        stops: vec![
            (-1.0, -0.8),
            (-0.2, -0.3),
            (0.2, 0.3),
            (1.0, 0.8),
        ],
    }));
    g.connect(PinRef::new(continent, 0), PinRef::new(shaped, 0))
        .expect("continent -> shaped");

    // --- Weighted sum: shaped_continental + 0.5 × perlin3d ----------------
    let weight = g.add_node(NodeKind::Constant(ConstantParams { value: 0.5 }));
    let weighted = g.add_node(NodeKind::Multiply(MultiplyParams::default()));
    g.connect(PinRef::new(perlin3d, 0), PinRef::new(weighted, 0))
        .expect("perlin3d -> mul.a");
    g.connect(PinRef::new(weight, 0), PinRef::new(weighted, 1))
        .expect("0.5 -> mul.b");

    let final_density = g.add_node(NodeKind::Add(AddParams::default()));
    g.connect(PinRef::new(shaped, 0), PinRef::new(final_density, 0))
        .expect("shaped -> add.a");
    g.connect(PinRef::new(weighted, 0), PinRef::new(final_density, 1))
        .expect("weighted -> add.b");

    // --- Terminals: 2D heatmap + 3D voxel pipeline ------------------------
    let out = g.add_node(NodeKind::Output(OutputParams::default()));
    g.connect(PinRef::new(final_density, 0), PinRef::new(out, 0))
        .expect("density -> output");

    // Layered materials: 1 voxel grass, 3 voxels dirt, 8 voxels limestone,
    // granite below — gives ~12 banded layers visible in cliff faces.
    let layer = g.add_node(NodeKind::Layer(LayerParams {
        bands: vec![
            (MaterialId(6), 1), // grass-soil
            (MaterialId(3), 3), // soil
            (MaterialId(1), 8), // limestone
        ],
        fill: MaterialId(2), // granite
    }));
    g.connect(PinRef::new(final_density, 0), PinRef::new(layer, 0))
        .expect("density -> layer");

    // Phase-9 terrain tail: density + material → BuildTerrain → SlopeRefiner
    // → TerrainOutput. The refiner reclassifies surface cubes as
    // slopes/corners; the terminal just passes its terrain through to the
    // runtime cache.
    let build = g.add_node(NodeKind::BuildTerrain(BuildTerrainParams::default()));
    g.connect(PinRef::new(final_density, 0), PinRef::new(build, 0))
        .expect("density -> build.density");
    g.connect(PinRef::new(layer, 0), PinRef::new(build, 1))
        .expect("layer -> build.material");

    let refiner = g.add_node(NodeKind::SlopeRefiner(SlopeRefinerParams::default()));
    g.connect(PinRef::new(build, 0), PinRef::new(refiner, 0))
        .expect("build -> refiner.terrain");

    let terrain = g.add_node(NodeKind::TerrainOutput(TerrainOutputParams::default()));
    g.connect(PinRef::new(refiner, 0), PinRef::new(terrain, 0))
        .expect("refiner -> terrain.terrain");

    g
}
