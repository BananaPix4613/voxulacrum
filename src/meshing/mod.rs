pub mod cache;
pub mod coordinator;
mod marching_cubes;
mod mc_tables;

use glam::IVec3;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use std::sync::RwLock;
use bevy_ecs::prelude::Resource;

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
    chunk_key: IVec3,
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
    chunk_key: IVec3,
    outcome: Phase1Outcome,
}

struct Phase2Request {
    chunk_key: IVec3,
    cell_data: CellVertexData,
    snapshot: ChunkSnapshot,
    neighbor_boundaries: OwnedNeighborBoundaries,
    cache_key: u64,
}

pub struct Phase2Result {
    pub chunk_key: IVec3,
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

#[derive(Resource)]
pub struct MeshingPipeline {
    p1_request_tx: mpsc::SyncSender<Phase1Request>,
    p1_result_rx: Mutex<Receiver<Phase1Result>>,
    p2_request_tx: mpsc::SyncSender<Phase2Request>,
    p2_result_rx: Mutex<Receiver<Phase2Result>>,
    _workers: Mutex<Vec<JoinHandle<()>>>,

    chunk_states: HashMap<IVec3, ChunkMeshState>,
    pending_phase1: HashMap<IVec3, (CellVertexData, ChunkSnapshot, u64)>,
    boundary_maps: HashMap<IVec3, BoundaryVertexMap>,
    material_config: Arc<RwLock<MaterialConfig>>,

    pub stats: MeshingStats,
    batch_start: Option<Instant>,
    pending_submissions: Vec<IVec3>,

    // Cache
    cache_stats: Arc<AtomicCacheStats>,
    cache_dir: PathBuf,
    cache_disk_stats_stale: bool,
}

impl MeshingPipeline {
    pub fn new(materials: &MaterialParams, meshing: &crate::params::MeshingParams) -> Self {
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
            p1_result_rx: Mutex::new(p1_result_rx),
            p2_request_tx: p2_tx,
            p2_result_rx: Mutex::new(p2_result_rx),
            _workers: Mutex::new(workers),
            chunk_states: HashMap::new(),
            pending_phase1: HashMap::new(),
            boundary_maps: HashMap::new(),
            material_config,
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
        for (pos, chunk) in &world.chunks {
            if !chunk.mesh_dirty {
                continue;
            }
            let state = self.chunk_states.get(pos).copied().unwrap_or(ChunkMeshState::Idle);
            if state != ChunkMeshState::Idle {
                continue;
            }
            if !self.pending_submissions.contains(pos) {
                self.pending_submissions.push(*pos);
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

        let batch: Vec<IVec3> = self.pending_submissions.drain(..count).collect();
        for pos in batch {
            let state = self.chunk_states.get(&pos).copied().unwrap_or(ChunkMeshState::Idle);
            if state != ChunkMeshState::Idle {
                continue;
            }

            let chunk = match world.chunks.get(&pos) {
                Some(c) => c,
                None => continue, // Chunk unloaded since submission
            };
            let neighbors = world.build_neighbors(pos);
            let snapshot = ChunkSnapshot::extract(chunk, &neighbors, world.min_chunk_y, world.max_chunk_y);

            match self.p1_request_tx.try_send(Phase1Request {
                chunk_key: pos,
                snapshot,
            }) {
                Ok(()) => {
                    self.chunk_states.insert(pos, ChunkMeshState::Phase1InProgress);
                }
                Err(mpsc::TrySendError::Full(_)) => {
                    self.pending_submissions.push(pos);
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
        let p1_rx = self.p1_result_rx.get_mut().unwrap();
        while let Ok(result) = p1_rx.try_recv() {
            let key = result.chunk_key;
            match result.outcome {
                Phase1Outcome::CacheHit { vertices, indices } => {
                    // Cache hit: directly produce a completed result, skip Phase 2
                    self.chunk_states.remove(&key);
                    self.stats.total_meshed += 1;
                    completed.push(Phase2Result {
                        chunk_key: key,
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
                    self.chunk_states.insert(key, ChunkMeshState::Phase1Complete);
                    self.boundary_maps
                        .insert(key, cell_data.boundary_map.clone());
                    self.pending_phase1
                        .insert(key, (cell_data, snapshot, cache_key));
                }
            }
        }

        // Dispatch Phase 2 for chunks whose neighbors have completed Phase 1
        let pending_keys: Vec<IVec3> = self.pending_phase1.keys().cloned().collect();
        for key in pending_keys {
            if self.can_dispatch_phase2(key) {
                self.dispatch_phase2(key);
            }
        }

        // Drain Phase 2 results
        let p2_rx = self.p2_result_rx.get_mut().unwrap();
        while let Ok(result) = p2_rx.try_recv() {
            self.chunk_states.remove(&result.chunk_key);
            self.stats.total_meshed += 1;
            completed.push(result);
        }

        // Update stats
        self.stats.phase1_in_progress = self
            .chunk_states
            .values()
            .filter(|s| **s == ChunkMeshState::Phase1InProgress)
            .count();
        self.stats.phase1_complete = self
            .chunk_states
            .values()
            .filter(|s| **s == ChunkMeshState::Phase1Complete)
            .count();
        self.stats.phase2_in_progress = self
            .chunk_states
            .values()
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

    fn can_dispatch_phase2(&self, chunk_key: IVec3) -> bool {
        // Check the 7 positive-offset neighbors: (1,0,0), (0,1,0), (0,0,1),
        // (1,1,0), (1,0,1), (0,1,1), (1,1,1)
        for dz in 0i32..=1 {
            for dy in 0i32..=1 {
                for dx in 0i32..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let n_key = chunk_key + IVec3::new(dx, dy, dz);
                    // If neighbor is actively computing Phase 1, block
                    if self.chunk_states.get(&n_key) == Some(&ChunkMeshState::Phase1InProgress) {
                        return false;
                    }
                    // If neighbor is queued but not yet submitted, block
                    if self.pending_submissions.contains(&n_key) {
                        return false;
                    }
                    // If neighbor doesn't exist at all (not loaded), that's OK —
                    // we'll mesh without its boundary data
                }
            }
        }

        true
    }

    fn dispatch_phase2(&mut self, chunk_key: IVec3) {
        let (cell_data, snapshot, cache_key) = match self.pending_phase1.remove(&chunk_key) {
            Some(data) => data,
            None => return,
        };

        let mut nb = OwnedNeighborBoundaries::empty();
        for dz in 0i32..=1 {
            for dy in 0i32..=1 {
                for dx in 0i32..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let n_key = chunk_key + IVec3::new(dx, dy, dz);
                    let map_idx = dx as usize + (dy as usize) * 2 + (dz as usize) * 4;
                    if let Some(bmap) = self.boundary_maps.get(&n_key) {
                        nb.maps[map_idx] = Some(bmap.clone());
                    }
                }
            }
        }

        match self.p2_request_tx.try_send(Phase2Request {
            chunk_key,
            cell_data,
            snapshot,
            neighbor_boundaries: nb,
            cache_key,
        }) {
            Ok(()) => {
                self.chunk_states.insert(chunk_key, ChunkMeshState::Phase2InProgress);
            }
            Err(mpsc::TrySendError::Full(req)) => {
                self.pending_phase1.insert(chunk_key, (req.cell_data, req.snapshot, req.cache_key));
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                log::error!("Phase 2 request channel disconnected");
            }
        }
    }

    pub fn is_idle(&self) -> bool {
        self.pending_submissions.is_empty()
            && self.chunk_states.values().all(|s| *s == ChunkMeshState::Idle)
            && self.pending_phase1.is_empty()
    }

    pub fn clear_boundary_maps(&mut self) {
        self.boundary_maps.clear();
    }

    /// Remove all pipeline tracking state for a chunk (used during unload).
    pub fn remove_chunk_state(&mut self, pos: IVec3) {
        self.chunk_states.remove(&pos);
        self.pending_phase1.remove(&pos);
        self.boundary_maps.remove(&pos);
        self.pending_submissions.retain(|p| *p != pos);
    }

    /// Check if a chunk has active meshing work in flight.
    pub fn is_chunk_in_flight(&self, pos: IVec3) -> bool {
        matches!(
            self.chunk_states.get(&pos),
            Some(ChunkMeshState::Phase1InProgress | ChunkMeshState::Phase2InProgress)
        )
    }

    /// Reset all internal pipeline state for a freshly swapped world.
    ///
    /// Clears chunk tracking, pending data, boundary maps, and drains any
    /// stale in-flight results from worker threads. Does NOT shut down workers
    /// or clear the disk cache.
    pub fn reset_for_new_world(&mut self) {
        self.chunk_states.clear();
        self.pending_phase1.clear();
        self.boundary_maps.clear();
        self.pending_submissions.clear();
        self.batch_start = None;

        let worker_count = self.stats.worker_count;
        self.stats = MeshingStats {
            worker_count,
            ..Default::default()
        };

        let p1_rx = self.p1_result_rx.get_mut().unwrap();
        let p2_rx = self.p2_result_rx.get_mut().unwrap();
        while p1_rx.try_recv().is_ok() {}
        while p2_rx.try_recv().is_ok() {}

        log::info!("MeshingPipeline reset for new world");
    }

    /// Update material config so workers pick up new colors/sharpness.
    pub fn update_material_config(&self, materials: &MaterialParams, meshing: &crate::params::MeshingParams) {
        let mut config = self.material_config.write().unwrap();
        *config = MaterialConfig::from_params(materials, meshing);
    }

    /// Clear the disk mesh cache.
    pub fn clear_cache(&self) {
        match cache::clear_cache(&self.cache_dir) {
            Ok(n) => log::info!("Cleared {} cached mesh files", n),
            Err(e) => log::warn!("Failed to clear mesh cache: {}", e),
        }
        self.cache_stats.reset();
        // Disk stats will be recalculated on next poll
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
                let indices = generate_faces_from_snapshot(&mut cell_data, &req.snapshot, &nb);

                // Save to cache (fire-and-forget, errors are non-fatal)
                if let Err(e) = cache::save_cached_mesh(
                    &cache_config.cache_dir,
                    req.snapshot.position.x,
                    req.snapshot.position.y,
                    req.snapshot.position.z,
                    req.cache_key,
                    &cell_data.vertices,
                    &indices,
                )
                {
                    log::warn!("Cache save failed for chunk {}: {}", req.chunk_key, e);
                    cache_config.stats.errors.fetch_add(1, Ordering::Relaxed);
                }

                let _ = p2_result_tx.send(Phase2Result {
                    chunk_key: req.chunk_key,
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
                        cache_config.stats.hits.fetch_add(1, Ordering::Relaxed);
                        let _ = p1_result_tx.send(Phase1Result {
                            chunk_key: req.chunk_key,
                            outcome: Phase1Outcome::CacheHit { vertices, indices },
                        });
                        continue;
                    }

                    // Cache miss -- compute Phase 1 with current material config
                    cache_config.stats.misses.fetch_add(1, Ordering::Relaxed);
                    let cell_data = generate_cell_vertices_from_snapshot(
                        &req.snapshot,
                        &mat_config,
                    );
                    drop(mat_config); // Release lock

                    let _ = p1_result_tx.send(Phase1Result {
                        chunk_key: req.chunk_key,
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