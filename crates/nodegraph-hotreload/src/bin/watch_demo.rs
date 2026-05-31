//! CLI demo: watch `assets/graphs/` (or an arg-supplied dir), re-evaluate
//! each `*.json` graph on change, and write its `<name>.png` heightmap.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glam::IVec3;
use nodegraph_eval::{render_graph_to_png, EvalContext};
use nodegraph_hotreload::GraphWatcher;
use nodegraph_ir::{
    AddParams, ConstantParams, Graph, NodeKind,
    OutputParams, Perlin2DParams, PinRef,
};

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const QUIET_WINDOW: Duration = Duration::from_millis(50);
const Y_SLICE: usize = 0;
const RANGE: (f32, f32) = (-1.0, 1.5);
const WORLD_SEED: u64 = 42;
const CHUNK: IVec3 = IVec3::ZERO;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/graphs"));

    std::fs::create_dir_all(&dir)?;
    bootstrap_example_if_empty(&dir)?;

    // Initial render of every existing graph.
    for path in list_graph_files(&dir)? {
        regenerate(&path);
    }

    let watcher = GraphWatcher::new(&dir)?;
    log::info!("hot-reload demo running; edit any *.json in {} (Ctrl-C to quit)", dir.display());

    loop {
        std::thread::sleep(POLL_INTERVAL);
        let mut changes = watcher.poll_changes();
        if changes.is_empty() {
            continue;
        }
        // Coalesce a burst of editor saves into one regeneration per path.
        loop {
            std::thread::sleep(QUIET_WINDOW);
            let more = watcher.poll_changes();
            if more.is_empty() {
                break;
            }
            for p in more {
                if !changes.contains(&p) {
                    changes.push(p);
                }
            }
        }
        for path in changes {
            regenerate(&path);
        }
    }
}

fn list_graph_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
    Ok(out)
}

fn regenerate(json_path: &Path) {
    let started = Instant::now();
    let result: Result<PathBuf, Box<dyn std::error::Error>> = (|| {
        let text = std::fs::read_to_string(json_path)?;
        let graph = Graph::from_json(&text)?;
        let png_path = json_path.with_extension("png");
        render_graph_to_png(
            &graph,
            EvalContext::new(WORLD_SEED, CHUNK),
            Y_SLICE,
            RANGE,
            &png_path,
        )?;
        Ok(png_path)
    })();
    let elapsed = started.elapsed();
    match result {
        Ok(png) => log::info!(
            "regenerated {} -> {} in {:?}",
            json_path.display(),
            png.display(),
            elapsed
        ),
        Err(e) => log::error!("failed to regenerate {}: {}", json_path.display(), e),
    }
}

/// If the watched dir has no `*.json` files yet, write a default example so
/// the demo has something to load. Built from the same Perlin+Constant+Add
/// graph as the Phase 3 test; serialized through the real
/// `Graph::to_json_pretty` so the slotmap encoding is always correct.
fn bootstrap_example_if_empty(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let any_json = list_graph_files(dir)?.into_iter().next().is_some();
    if any_json {
        return Ok(());
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
    g.connect(PinRef::new(perlin, 0), PinRef::new(add, 0))?;
    g.connect(PinRef::new(bias, 0), PinRef::new(add, 1))?;
    g.connect(PinRef::new(add, 0), PinRef::new(out, 0))?;
    let path = dir.join("example.graph.json");
    std::fs::write(&path, g.to_json_pretty()?)?;
    log::info!("bootstrapped example at {}", path.display());
    Ok(())
}
