//! CLI demo: watch `assets/graphs/` (or an arg-supplied dir), re-evaluate
//! each `*.json` graph on change, and write its `<name>.png` heightmap.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glam::IVec3;
use nodegraph_eval::{render_graph_to_png, EvalContext};
use nodegraph_hotreload::GraphWatcher;
use nodegraph_ir::{Graph};
use voxel_core::{MaterialRegistry, ENGINE_NAMESPACE};

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
    let _ = nodegraph_hotreload::bootstrap_example_graph(&dir)?;

    // Graphs name their materials; resolve them against the content root the
    // graphs live in, as the engine does at load.
    let materials_path = dir.join("..").join("materials.ron");
    let materials = match MaterialRegistry::load_from_ron(&materials_path, ENGINE_NAMESPACE) {
        Ok(registry) => Some(registry),
        Err(e) => {
            log::warn!("{}: {e}; graphs that name materials will not render", materials_path.display());
            None
        }
    };

    // Initial render of every existing graph.
    for path in list_graph_files(&dir)? {
        regenerate(&path, materials.as_ref());
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
            regenerate(&path, materials.as_ref());
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

fn regenerate(json_path: &Path, materials: Option<&MaterialRegistry>) {
    let started = Instant::now();
    let result: Result<PathBuf, Box<dyn std::error::Error>> = (|| {
        let text = std::fs::read_to_string(json_path)?;
        let mut graph = Graph::from_json(&text)?;
        if let Some(registry) = materials {
            nodegraph_hotreload::resolve_materials(&mut graph, registry, ENGINE_NAMESPACE)?;
        }
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
