pub mod cache;
mod marching_cubes;
mod mc_tables;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use std::sync::RwLock;

use crate::params::MaterialParams;
use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::ChunkSnapshot;
use cache::AtomicCacheStats;
use marching_cubes::{
    generate_cell_vertices_from_snapshot, generate_faces_from_snapshot, BoundaryVertexMap,
    CellVertexData, OwnedNeighborBoundaries,
};

// ============================================================================
// Messages
// ============================================================================

struct Phase1Request {
    chunk_index: usize,
    snapshot: ChunkSnapshot,
}

enum Phase1Outcome {
    /// Normal Phase 1 completed. Needs Phase 2.
    Computed {
        cell_data: CellVertexData,
        snapshot: ChunkSnapshot,
        cache_key: u64,
    },
    /// Cache hit. Skip Phase 2 entirely.
    CacheHit {
        vertices: Vec<TerrainVertex>,
        indices: Vec<u32>,
    },
}

struct Phase1Result {
    chunk_index: usize,
    outcome: Phase1Outcome,
}

struct Phase2Request {
    chunk_index: usize,
    cell_data: CellVertexData,
    snapshot: ChunkSnapshot,
    neighbor_boundaries: OwnedNeighborBoundaries,
    cache_key: u64,
}

pub struct Phase2Result {
    pub chunk_index: usize,
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u32>,
}

// ============================================================================
// State tracking
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChunkMeshState {
    Idle,
    Phase1InProgress,
    Phase1Complete,
    Phase2InProgress,
}

#[derive(Clone, Default)]
pub struct MeshingStats {
    pub phase1_in_progress: usize,
    pub phase1_complete: usize,
    pub phase2_in_progress: usize,
    pub total_meshed: u64,
    pub worker_count: usize,
    pub last_batch_time_ms: f32,
    pub pending_submissions: usize,
    // Cache stats
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_errors: u64,
    pub cache_files: u64,
    pub cache_bytes: u64,
}

// ============================================================================
// Shared cache config for workers
// ============================================================================

struct CacheConfig {
    cache_dir: PathBuf,
    stats: Arc<AtomicCacheStats>,
}

/// Runtime material properties shared with worker threads.
/// Updated from the main thread when material params change.
pub struct MaterialConfig {
    pub colors: Vec<[f32; 3]>,
    pub sharpness: Vec<f32>,
    pub greedy_merge_enabled: bool,
    pub flat_threshold_error: f32,
    pub flat_normal_threshold: f32,
}

impl MaterialConfig {
    pub fn from_params(mat_params: &MaterialParams, mesh_params: &crate::params::MeshingParams) -> Self {
        Self {
            colors: mat_params.entries.iter().map(|e| e.color).collect(),
            sharpness: mat_params.entries.iter().map(|e| e.sharpness).collect(),
            greedy_merge_enabled: mesh_params.greedy_merge_enabled,
            flat_threshold_error: mesh_params.flat_threshold_error,
            flat_normal_threshold: mesh_params.flat_normal_threshold,
        }
    }
}

// ============================================================================
// MeshingPipeline
// ============================================================================

pub struct MeshingPipeline {
    p1_request_tx: mpsc::SyncSender<Phase1Request>,
    p1_result_rx: Receiver<Phase1Result>,
    p2_request_tx: mpsc::SyncSender<Phase2Request>,
    p2_result_rx: Receiver<Phase2Result>,
    _workers: Vec<JoinHandle<()>>,

    chunk_states: Vec<ChunkMeshState>,
    pending_phase1: HashMap<usize, (CellVertexData, ChunkSnapshot, u64)>,
    boundary_maps: HashMap<usize, BoundaryVertexMap>,
    material_config: Arc<RwLock<MaterialConfig>>,

    chunks_x: usize,
    chunks_y: usize,
    chunks_z: usize,

    pub stats: MeshingStats,
    batch_start: Option<Instant>,
    pending_submissions: Vec<usize>,

    // Cache
    cache_stats: Arc<AtomicCacheStats>,
    cache_dir: PathBuf,
    cache_disk_stats_stale: bool,
}

impl MeshingPipeline {
    pub fn new(chunks_x: usize, chunks_y: usize, chunks_z: usize, materials: &MaterialParams, meshing: &crate::params::MeshingParams) -> Self {
        let total_chunks = chunks_x * chunks_y * chunks_z;
        let num_workers = num_cpus::get().saturating_sub(2).max(2);

        // Setup cache directory
        let cache_dir = PathBuf::from("cache/meshes");
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            log::warn!("Failed to create cache directory: {}", e);
        }

        let cache_stats = Arc::new(AtomicCacheStats::new());
        let material_config = Arc::new(RwLock::new(MaterialConfig::from_params(materials, meshing)));

        let cache_config = Arc::new(CacheConfig {
            cache_dir: cache_dir.clone(),
            stats: cache_stats.clone(),
        });

        let (p1_tx, p1_rx) = mpsc::sync_channel::<Phase1Request>(256);
        let (p1_result_tx, p1_result_rx) = mpsc::channel::<Phase1Result>();
        let (p2_tx, p2_rx) = mpsc::sync_channel::<Phase2Request>(256);
        let (p2_result_tx, p2_result_rx) = mpsc::channel::<Phase2Result>();

        let p1_rx = Arc::new(Mutex::new(p1_rx));
        let p2_rx = Arc::new(Mutex::new(p2_rx));

        let mut workers = Vec::with_capacity(num_workers);
        for i in 0..num_workers {
            let p1_rx = p1_rx.clone();
            let p1_result_tx = p1_result_tx.clone();
            let p2_rx = p2_rx.clone();
            let p2_result_tx = p2_result_tx.clone();
            let cache_config = cache_config.clone();
            let mat_config = material_config.clone();

            let handle = thread::Builder::new()
                .name(format!("mesh-worker-{}", i))
                .spawn(move || {
                    worker_loop(p1_rx, p1_result_tx, p2_rx, p2_result_tx, cache_config, mat_config);
                })
                .expect("Failed to spawn mesh worker thread");

            workers.push(handle);
        }

        log::info!("MeshingPipeline: {} worker threads, cache at {:?}", num_workers, cache_dir);

        Self {
            p1_request_tx: p1_tx,
            p1_result_rx,
            p2_request_tx: p2_tx,
            p2_result_rx,
            _workers: workers,
            chunk_states: vec![ChunkMeshState::Idle; total_chunks],
            pending_phase1: HashMap::new(),
            boundary_maps: HashMap::new(),
            material_config,
            chunks_x,
            chunks_y,
            chunks_z,
            stats: MeshingStats {
                worker_count: num_workers,
                ..Default::default()
            },
            batch_start: None,
            pending_submissions: Vec::new(),
            cache_stats,
            cache_dir,
            cache_disk_stats_stale: true,
        }
    }

    /// Queue all dirty chunks for meshing. Actual snapshot creation happens
    /// incrementally in drain_pending_submissions() to avoid blocking the main thread.
    pub fn submit_all_dirty(&mut self, world: &crate::world::World) {
        for i in 0..world.chunks.len() {
            if !world.chunks[i].mesh_dirty {
                continue;
            }
            if self.chunk_states[i] != ChunkMeshState::Idle {
                continue;
            }
            if !self.pending_submissions.contains(&i) {
                self.pending_submissions.push(i);
            }
        }
        if !self.pending_submissions.is_empty() && self.batch_start.is_none() {
            self.batch_start = Some(Instant::now());
        }
    }

    /// Create snapshots for a bounded number of pending chunks and submit them.
    /// Call this each frame from the main loop, passing the world reference.
    pub fn drain_pending_submissions(&mut self, world: &crate::world::World) {
        const MAX_SNAPSHOTS_PER_FRAME: usize = 8;
        let count = self.pending_submissions.len().min(MAX_SNAPSHOTS_PER_FRAME);
        if count == 0 {
            return;
        }

        let batch: Vec<usize> = self.pending_submissions.drain(..count).collect();
        for i in batch {
            if self.chunk_states[i] != ChunkMeshState::Idle {
                continue;
            }

            let chunk = &world.chunks[i];
            let cx = chunk.position.x as usize;
            let cy = chunk.position.y as usize;
            let cz = chunk.position.z as usize;
            let neighbors = world.build_neighbors(cx, cy, cz);
            let snapshot = ChunkSnapshot::extract(chunk, &neighbors);

            match self.p1_request_tx.try_send(Phase1Request {
                chunk_index: i,
                snapshot,
            }) {
                Ok(()) => {
                    self.chunk_states[i] = ChunkMeshState::Phase1InProgress;
                }
                Err(mpsc::TrySendError::Full(_)) => {
                    // Channel full - re-queue for next frame
                    self.pending_submissions.push(i);
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    log::error!("Phase 1 request channel disconnected");
                }
            }
        }
    }

    /// Poll for completed results. Returns meshes ready for GPU upload.
    pub fn poll(&mut self) -> Vec<Phase2Result> {
        let mut completed = Vec::new();

        // Drain Phase 1 results
        while let Ok(result) = self.p1_result_rx.try_recv() {
            let idx = result.chunk_index;
            match result.outcome {
                Phase1Outcome::CacheHit { vertices, indices } => {
                    // Cache hit: directly produce a completed result, skip Phase 2
                    self.chunk_states[idx] = ChunkMeshState::Idle;
                    self.stats.total_meshed += 1;
                    completed.push(Phase2Result {
                        chunk_index: idx,
                        vertices,
                        indices,
                    });
                }
                Phase1Outcome::Computed {
                    cell_data,
                    snapshot,
                    cache_key,
                } => {
                    // Normal Phase 1 complete -- store for Phase 2 dispatch
                    self.chunk_states[idx] = ChunkMeshState::Phase1Complete;
                    self.boundary_maps
                        .insert(idx, cell_data.boundary_map.clone());
                    self.pending_phase1
                        .insert(idx, (cell_data, snapshot, cache_key));
                }
            }
        }

        // Dispatch Phase 2 for chunks whose neighbors have completed Phase 1
        let pending_indices: Vec<usize> = self.pending_phase1.keys().cloned().collect();
        for idx in pending_indices {
            if self.can_dispatch_phase2(idx) {
                self.dispatch_phase2(idx);
            }
        }

        // Drain Phase 2 results
        while let Ok(result) = self.p2_result_rx.try_recv() {
            self.chunk_states[result.chunk_index] = ChunkMeshState::Idle;
            self.stats.total_meshed += 1;
            completed.push(result);
        }

        // Update stats
        self.stats.phase1_in_progress = self
            .chunk_states
            .iter()
            .filter(|s| **s == ChunkMeshState::Phase1InProgress)
            .count();
        self.stats.phase1_complete = self
            .chunk_states
            .iter()
            .filter(|s| **s == ChunkMeshState::Phase1Complete)
            .count();
        self.stats.phase2_in_progress = self
            .chunk_states
            .iter()
            .filter(|s| **s == ChunkMeshState::Phase2InProgress)
            .count();
        self.stats.pending_submissions = self.pending_submissions.len();

        // Cache stats (atomics are cheap to read)
        self.stats.cache_hits = self.cache_stats.hits.load(Ordering::Relaxed);
        self.stats.cache_misses = self.cache_stats.misses.load(Ordering::Relaxed);
        self.stats.cache_errors = self.cache_stats.errors.load(Ordering::Relaxed);

        // Disk stats: only recompute when marked stale (expensive I/O)
        if self.cache_disk_stats_stale {
            let (files, bytes) = cache::disk_usage(&self.cache_dir);
            self.stats.cache_files = files;
            self.stats.cache_bytes = bytes;
            self.cache_disk_stats_stale = false;
        }

        // Record batch time when everything finishes
        if !completed.is_empty()
            && self.pending_submissions.is_empty()
            && self.stats.phase1_in_progress == 0
            && self.stats.phase1_complete == 0
            && self.stats.phase2_in_progress == 0
        {
            if let Some(start) = self.batch_start.take() {
                self.stats.last_batch_time_ms = start.elapsed().as_secs_f32() * 1000.0;
                log::info!(
                    "Meshing batch complete in {:.0}ms (cache: {} hits, {} misses)",
                    self.stats.last_batch_time_ms,
                    self.stats.cache_hits,
                    self.stats.cache_misses,
                );
                self.cache_disk_stats_stale = true;
            }
        }

        completed
    }

    fn can_dispatch_phase2(&self, chunk_index: usize) -> bool {
        let (cx, cy, cz) = self.index_to_coords(chunk_index);

        for dz in 0u8..=1 {
            for dy in 0u8..=1 {
                for dx in 0u8..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let nx = cx + dx as usize;
                    let ny = cy + dy as usize;
                    let nz = cz + dz as usize;
                    if nx < self.chunks_x && ny < self.chunks_y && nz < self.chunks_z {
                        let ni = self.coords_to_index(nx, ny, nz);
                        // Block if neighbor is still computing Phase 1
                        if self.chunk_states[ni] == ChunkMeshState::Phase1InProgress {
                            return false;
                        }
                        // Block if neighbor hasn't been submitted yet
                        if self.pending_submissions.contains(&ni) {
                            return false;
                        }
                    }
                }
            }
        }

        true
    }

    fn dispatch_phase2(&mut self, chunk_index: usize) {
        let (cell_data, snapshot, cache_key) = match self.pending_phase1.remove(&chunk_index) {
            Some(data) => data,
            None => return,
        };

        let (cx, cy, cz) = self.index_to_coords(chunk_index);

        let mut nb = OwnedNeighborBoundaries::empty();
        for dz in 0u8..=1 {
            for dy in 0u8..=1 {
                for dx in 0u8..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let nx = cx + dx as usize;
                    let ny = cy + dy as usize;
                    let nz = cz + dz as usize;
                    if nx < self.chunks_x && ny < self.chunks_y && nz < self.chunks_z {
                        let ni = self.coords_to_index(nx, ny, nz);
                        let map_idx = dx as usize + (dy as usize) * 2 + (dz as usize) * 4;
                        if let Some(bmap) = self.boundary_maps.get(&ni) {
                            nb.maps[map_idx] = Some(bmap.clone());
                        }
                    }
                }
            }
        }

        match self.p2_request_tx.try_send(Phase2Request {
            chunk_index,
            cell_data,
            snapshot,
            neighbor_boundaries: nb,
            cache_key,
        }) {
            Ok(()) => {
                self.chunk_states[chunk_index] = ChunkMeshState::Phase2InProgress;
            }
            Err(mpsc::TrySendError::Full(req)) => {
                // Re-insert for retry next frame
                self.pending_phase1.insert(chunk_index, (req.cell_data, req.snapshot, req.cache_key));
                // State stays Phase1Complete
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                log::error!("Phase 2 request channel disconnected");
            }
        }
    }

    pub fn is_idle(&self) -> bool {
        self.pending_submissions.is_empty()
            && self.chunk_states.iter().all(|s| *s == ChunkMeshState::Idle)
            && self.pending_phase1.is_empty()
    }

    pub fn clear_boundary_maps(&mut self) {
        self.boundary_maps.clear();
    }

    /// Clear all cached mesh files from disk and reset cache stats.
    pub fn clear_cache(&mut self) {
        match cache::clear_cache(&self.cache_dir) {
            Ok(count) => log::info!("Cleared {} cache files", count),
            Err(e) => log::warn!("Failed to clear cache: {}", e),
        }
        self.cache_stats.reset();
        self.cache_disk_stats_stale = true;
    }

    /// Update runtime material properties. Workers will pick up the new values
    /// on their next mesh computation.
    pub fn update_material_config(&self, mat_params: &MaterialParams, mesh_params: &crate::params::MeshingParams) {
        if let Ok(mut config) = self.material_config.write() {
            *config = MaterialConfig::from_params(mat_params, mesh_params);
        }
    }

    /// Reset all internal pipeline state for a freshly swapped world.
    ///
    /// Clears chunk tracking, pending data, boundary maps, and drains any
    /// stale in-flight results from worker threads. Does NOT shut down workers
    /// or clear the disk cache.
    pub fn reset_for_new_world(&mut self) {
        // Reset all chunk states to Idle
        for state in &mut self.chunk_states {
            *state = ChunkMeshState::Idle;
        }

        // Clear pending Phase 1 data (snapshots/cell data from old world)
        self.pending_phase1.clear();

        // Clear boundary maps from old world
        self.boundary_maps.clear();

        // Clear pending submissions queue
        self.pending_submissions.clear();

        // Reset batch timing
        self.batch_start = None;

        // Reset stats counters (preserve worker count)
        let worker_count = self.stats.worker_count;
        self.stats = MeshingStats {
            worker_count,
            ..Default::default()
        };

        // Drain any in-flight results from worker threads to discard stale data.
        // Workers may still be processing old-world requests; those results will
        // refer to old voxel data and must not be uploaded after the swap.
        while self.p1_result_rx.try_recv().is_ok() {}
        while self.p2_result_rx.try_recv().is_ok() {}

        log::info!("MeshingPipeline reset for new world");
    }

    fn index_to_coords(&self, index: usize) -> (usize, usize, usize) {
        let cx = index % self.chunks_x;
        let cy = (index / self.chunks_x) % self.chunks_y;
        let cz = index / (self.chunks_x * self.chunks_y);
        (cx, cy, cz)
    }

    fn coords_to_index(&self, cx: usize, cy: usize, cz: usize) -> usize {
        cx + cy * self.chunks_x + cz * self.chunks_x * self.chunks_y
    }
}

// ============================================================================
// Worker thread
// ============================================================================

fn worker_loop(
    p1_rx: Arc<Mutex<Receiver<Phase1Request>>>,
    p1_result_tx: Sender<Phase1Result>,
    p2_rx: Arc<Mutex<Receiver<Phase2Request>>>,
    p2_result_tx: Sender<Phase2Result>,
    cache_config: Arc<CacheConfig>,
    material_config: Arc<RwLock<MaterialConfig>>,
) {
    loop {
        // Try Phase 2 first (higher priority - unblocks GPU upload)
        if let Ok(rx) = p2_rx.lock() {
            if let Ok(req) = rx.try_recv() {
                drop(rx);
                let mut cell_data = req.cell_data;
                let nb = req.neighbor_boundaries.as_ref();
                let indices =
                    generate_faces_from_snapshot(&mut cell_data, &req.snapshot, &nb);

                // Save to cache (fire-and-forget, errors are non-fatal)
                let cache_path = cache::cache_file_path(
                    &cache_config.cache_dir,
                    req.snapshot.position.x,
                    req.snapshot.position.y,
                    req.snapshot.position.z,
                    req.cache_key,
                );
                if let Err(e) =
                    cache::save_cached_mesh(&cache_path, req.cache_key, &cell_data.vertices, &indices)
                {
                    log::warn!("Cache save failed for chunk {}: {}", req.chunk_index, e);
                    cache_config
                        .stats
                        .errors
                        .fetch_add(1, Ordering::Relaxed);
                }

                let _ = p2_result_tx.send(Phase2Result {
                    chunk_index: req.chunk_index,
                    vertices: cell_data.vertices,
                    indices,
                });
                continue;
            }
        }

        // Try Phase 1
        if let Ok(rx) = p1_rx.lock() {
            match rx.try_recv() {
                Ok(req) => {
                    drop(rx);

                    // Read current material config (snapshot for this computation)
                    let mat_config = material_config.read().unwrap();

                    // Compute cache key from snapshot voxel data + material + meshing properties
                    let cache_key = cache::compute_cache_key(
                        &req.snapshot,
                        &mat_config.sharpness,
                        &mat_config.colors,
                        mat_config.greedy_merge_enabled,
                        mat_config.flat_threshold_error,
                        mat_config.flat_normal_threshold,
                    );

                    // Try loading from disk cache
                    let cache_path = cache::cache_file_path(
                        &cache_config.cache_dir,
                        req.snapshot.position.x,
                        req.snapshot.position.y,
                        req.snapshot.position.z,
                        cache_key,
                    );

                    if let Some((vertices, indices)) =
                        cache::load_cached_mesh(&cache_path, cache_key)
                    {
                        // Cache hit -- skip both phases
                        cache_config
                            .stats
                            .hits
                            .fetch_add(1, Ordering::Relaxed);
                        let _ = p1_result_tx.send(Phase1Result {
                            chunk_index: req.chunk_index,
                            outcome: Phase1Outcome::CacheHit { vertices, indices },
                        });
                        continue;
                    }

                    // Cache miss -- compute Phase 1 with current material config
                    cache_config
                        .stats
                        .misses
                        .fetch_add(1, Ordering::Relaxed);
                    let cell_data = generate_cell_vertices_from_snapshot(
                        &req.snapshot,
                        &mat_config,
                    );
                    drop(mat_config); // Release lock

                    let _ = p1_result_tx.send(Phase1Result {
                        chunk_index: req.chunk_index,
                        outcome: Phase1Outcome::Computed {
                            cell_data,
                            snapshot: req.snapshot,
                            cache_key,
                        },
                    });
                    continue;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    drop(rx);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
    }
}