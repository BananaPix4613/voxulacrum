//! Graph loading, evaluation, and watcher polling.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use glam::IVec3;
use nodegraph_eval::{EvalContext, Evaluator, ScalarField};
use nodegraph_hotreload::{bootstrap_example_graph, GraphWatcher};
use nodegraph_ir::{Graph, NodeKind};

/// World seed used for the preview chunk evaluation (matches Phase 3 tests).
pub const WORLD_SEED: u64 = 42;
/// Chunk this preview always evaluates.
pub const PREVIEW_CHUNK: IVec3 = IVec3::ZERO;

/// Owns the watcher, the currently-loaded graph, and its evaluated field.
pub struct Runtime {
    watcher: GraphWatcher,
    graph_path: PathBuf,
    pub field: Option<Arc<ScalarField>>,
    pub field_version: u64,
    pub last_eval_ms: f32,
    pub error: Option<String>,
}

impl Runtime {
    /// Create the runtime, bootstrapping `example.graph.json` if `dir` has no
    /// JSON graphs yet.
    pub fn new(dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let _ = bootstrap_example_graph(&dir).map_err(|e| e.to_string())?;

        let graph_path = pick_initial_graph(&dir)
            .ok_or_else(|| format!("no .json graphs in {}", dir.display()))?;
        // Canonicalize so equality holds against notify's absolute paths.
        let graph_path = std::fs::canonicalize(&graph_path).map_err(|e| e.to_string())?;

        let watcher = GraphWatcher::new(&dir).map_err(|e| e.to_string())?;
        let mut me = Self {
            watcher,
            graph_path,
            field: None,
            field_version: 0,
            last_eval_ms: 0.0,
            error: None,
        };
        me.reload();
        Ok(me)
    }

    /// Path of the graph this preview is bound to.
    pub fn graph_path(&self) -> &std::path::Path {
        &self.graph_path
    }

    /// Drain pending watcher events; reload + re-evaluate if our graph changed.
    /// Returns `true` if the field was replaced.
    ///
    /// Path equality is brittle across notify backends — Windows may emit
    /// relative paths joined onto the watched dir, absolute canonical paths,
    /// or short-name forms depending on conditions. The watcher is
    /// non-recursive on one dir, so comparing by file name is unique and
    /// platform-stable.
    pub fn poll(&mut self) -> bool {
        let our_name = self.graph_path.file_name();
        let hit = changes_match(&self.watcher.poll_changes(), our_name);
        if hit {
            self.reload();
        }
        hit
    }

    fn reload(&mut self) {
        let started = Instant::now();
        match self.try_reload() {
            Ok(field) => {
                self.field = Some(field);
                self.field_version = self.field_version.wrapping_add(1);
                self.error = None;
            }
            Err(e) => {
                log::warn!("reload failed: {}", e);
                self.error = Some(e);
            }
        }
        self.last_eval_ms = started.elapsed().as_secs_f32() * 1000.0;
    }

    fn try_reload(&self) -> Result<Arc<ScalarField>, String> {
        let text = std::fs::read_to_string(&self.graph_path).map_err(|e| e.to_string())?;
        let graph = Graph::from_json(&text).map_err(|e| e.to_string())?;
        let out_id = graph
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::Output(_)))
            .map(|(id, _)| id)
            .ok_or_else(|| "graph has no Output node".to_string())?;
        let mut eval = Evaluator::new(&graph, EvalContext::new(WORLD_SEED, PREVIEW_CHUNK));
        eval.evaluate().map_err(|e| e.to_string())?;
        // `CachedOutput::Scalar` is already `Arc<ScalarField>` internally; clone is cheap.
        match eval.cache().get(out_id) {
            Some(nodegraph_eval::CachedOutput::Scalar(f)) => Ok(f.clone()),
            _ => Err("Output node produced no scalar".to_string()),
        }
    }
}

fn changes_match(changes: &[PathBuf], target_name: Option<&std::ffi::OsStr>) -> bool {
    let Some(name) = target_name else { return false };
    changes.iter().any(|p| p.file_name() == Some(name))
}

fn pick_initial_graph(dir: &std::path::Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    candidates.sort();
    // Prefer "example.graph.json" if present, else first sorted.
    candidates
        .iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some("example.graph.json"))
        .cloned()
        .or_else(|| candidates.into_iter().next())
}
