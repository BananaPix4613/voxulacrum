//! Regenerate `<dir>/example.graph.json` from the canonical
//! `build_example_graph` recipe. Overwrites any existing file.
//!
//! ```text
//! cargo run -p nodegraph-hotreload --bin regen_example
//! cargo run -p nodegraph-hotreload --bin regen_example -- some/other/dir
//! ```

use std::path::PathBuf;

use nodegraph_hotreload::build_example_graph;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/graphs"));

    std::fs::create_dir_all(&dir)?;
    let target = dir.join("example.graph.json");
    if target.exists() {
        std::fs::remove_file(&target)?;
        log::info!("removed existing {}", target.display());
    }

    let graph = build_example_graph();
    let json = graph.to_json_pretty()?;
    std::fs::write(&target, json)?;
    log::info!("wrote {} ({} nodes)", target.display(), graph.nodes.len());
    Ok(())
}
