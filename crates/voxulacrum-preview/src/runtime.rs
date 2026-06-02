//! Graph loading, evaluation, and watcher polling.
//!
//! The runtime owns:
//!   * the watched directory + the chosen `*.graph.json` path,
//!   * the most recently successfully-evaluated graph (`current_graph`),
//!   * the resulting scalar field (`field`, drives the 2D heatmap),
//!   * the resulting voxel field (`current_terrain`, drives the 3D viewer),
//!   * a monotonic `field_version` that lets the UI cache textures and
//!     mesh uploads cheaply.
//!
//! A single `evaluate_graph` pass extracts both the `Output` (Density) and
//! `TerrainOutput` (Terrain) caches when the user's graph includes them.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use glam::IVec3;
use nodegraph_eval::{CachedOutput, EvalContext, Evaluator, ScalarField};
use nodegraph_hotreload::{bootstrap_example_graph, GraphWatcher};
use nodegraph_ir::{Graph, NodeKind};
use voxel_core::{ChunkBuffer, Voxel};

/// World seed used for the preview chunk evaluation (matches Phase 3 tests).
pub const WORLD_SEED: u64 = 42;
/// Chunk this preview always evaluates.
pub const PREVIEW_CHUNK: IVec3 = IVec3::ZERO;

/// Outputs harvested from one graph evaluation. Either field may be `None`
/// if the graph omits the corresponding terminal node.
struct EvaluatedGraph {
    density: Option<Arc<ScalarField>>,
    terrain: Option<Arc<ChunkBuffer<Voxel, 32>>>,
}

/// Owns the watcher, the currently-loaded graph, and its evaluated fields.
pub struct Runtime {
    watcher: GraphWatcher,
    graph_path: PathBuf,

    /// Density field for the 2D heatmap (from the `Output` node).
    pub field: Option<Arc<ScalarField>>,
    /// Voxel field for the 3D view (from the `TerrainOutput` node).
    current_terrain: Option<Arc<ChunkBuffer<Voxel, 32>>>,
    /// Bumped on every successful (re)evaluation; keys downstream caches.
    pub field_version: u64,
    /// Wall-clock time of the most recent evaluation.
    pub last_eval_ms: f32,
    /// Last evaluation error, if any.
    pub error: Option<String>,
    /// The graph that produced the current fields. Populated on every
    /// successful load/replace so the editor can mirror its Snarl from it.
    current_graph: Option<Graph>,
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
            current_terrain: None,
            field_version: 0,
            last_eval_ms: 0.0,
            error: None,
            current_graph: None,
        };
        me.reload();
        Ok(me)
    }

    /// Path of the graph this preview is bound to.
    pub fn graph_path(&self) -> &std::path::Path {
        &self.graph_path
    }

    /// The most recently successfully-evaluated graph, if any. The editor
    /// reads this once at startup to populate its initial Snarl.
    pub fn current_graph(&self) -> Option<&Graph> {
        self.current_graph.as_ref()
    }

    /// The most recently produced terrain (`TerrainOutput` result), if any.
    pub fn terrain(&self) -> Option<&Arc<ChunkBuffer<Voxel, 32>>> {
        self.current_terrain.as_ref()
    }

    /// Replace the active graph in-memory (no disk read), re-evaluate, and
    /// bump `field_version`. Used by the editor to push live edits.
    pub fn replace_graph(&mut self, graph: Graph) {
        let started = Instant::now();
        match Self::evaluate_graph(&graph) {
            Ok(evaluated) => self.apply_evaluation(Some(graph), evaluated),
            Err(e) => self.error = Some(e),
        }
        self.last_eval_ms = started.elapsed().as_secs_f32() * 1000.0;
    }

    /// Save the given graph to `self.graph_path` (pretty JSON). Returns the
    /// path on success.
    pub fn save_to_disk(&self, graph: &Graph) -> Result<std::path::PathBuf, String> {
        let json = graph.to_json_pretty().map_err(|e| e.to_string())?;
        std::fs::write(&self.graph_path, json).map_err(|e| e.to_string())?;
        Ok(self.graph_path.clone())
    }

    /// Drain pending watcher events; reload + re-evaluate if our graph
    /// changed externally. Returns `true` if the fields were replaced.
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

    // --- internals ---------------------------------------------------------

    fn reload(&mut self) {
        let started = Instant::now();
        match self.try_reload() {
            Ok((graph, evaluated)) => self.apply_evaluation(Some(graph), evaluated),
            Err(e) => {
                log::warn!("reload failed: {}", e);
                self.error = Some(e);
            }
        }
        self.last_eval_ms = started.elapsed().as_secs_f32() * 1000.0;
    }

    fn try_reload(&self) -> Result<(Graph, EvaluatedGraph), String> {
        let text = std::fs::read_to_string(&self.graph_path).map_err(|e| e.to_string())?;
        let graph = Graph::from_json(&text).map_err(|e| e.to_string())?;
        let evaluated = Self::evaluate_graph(&graph)?;
        Ok((graph, evaluated))
    }

    /// Evaluate `graph` once and pull out both the `Output` (density) and
    /// `TerrainOutput` (terrain) caches when present. At least one terminal
    /// must produce a usable field; otherwise this returns an error.
    fn evaluate_graph(graph: &Graph) -> Result<EvaluatedGraph, String> {
        let mut eval = Evaluator::new(graph, EvalContext::new(WORLD_SEED, PREVIEW_CHUNK));
        eval.evaluate().map_err(|e| e.to_string())?;

        let density = graph
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::Output(_)))
            .map(|(id, _)| id)
            .and_then(|id| match eval.cache().get(id) {
                Some(CachedOutput::Scalar(f)) => Some(f.clone()),
                _ => None,
            });

        let terrain = graph
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.kind, NodeKind::TerrainOutput(_)))
            .map(|(id, _)| id)
            .and_then(|id| match eval.cache().get(id) {
                Some(CachedOutput::Terrain(f)) => Some(f.clone()),
                _ => None,
            });

        if density.is_none() && terrain.is_none() {
            return Err("graph has no Output or TerrainOutput node".to_string());
        }
        Ok(EvaluatedGraph { density, terrain })
    }

    /// Commit a successful evaluation's results into the runtime.
    fn apply_evaluation(&mut self, graph: Option<Graph>, evaluated: EvaluatedGraph) {
        if evaluated.density.is_some() {
            self.field = evaluated.density;
        }
        if evaluated.terrain.is_some() {
            self.current_terrain = evaluated.terrain;
        }
        self.field_version = self.field_version.wrapping_add(1);
        self.error = None;
        if let Some(g) = graph {
            self.current_graph = Some(g);
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
