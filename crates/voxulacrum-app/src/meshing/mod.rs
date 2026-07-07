pub mod cache;
pub mod coordinator;
mod cube_mesher;

use glam::IVec3;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use bevy_ecs::prelude::Resource;

use crate::params::MaterialParams;
use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::ChunkSnapshot;
use cache::AtomicCacheStats;
use cube_mesher::generate_chunk_mesh;

// ============================================================================
// Messages
// ============================================================================

struct MeshRequest {
    chunk_key: IVec3,
    snapshot: ChunkSnapshot,
    mesh_seq: u64,
}

pub struct MeshResult {
    pub chunk_key: IVec3,
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u32>,
    pub mesh_seq: u64,
}

// ============================================================================
// State tracking
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChunkMeshState {
    Idle,
    InProgress,
}

#[derive(Clone, Default)]
pub struct MeshingStats {
    pub in_progress: usize,
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
    pub cache_evictions: u64,
}

// ============================================================================
// Shared cache config for workers
// ============================================================================

struct CacheConfig {
    cache_dir: PathBuf,
    stats: Arc<AtomicCacheStats>,
    lru: Arc<Mutex<cache::MeshCacheLru>>,
    max_cache_bytes: u64,
    eviction_batch_size: usize,
}

/// Runtime material properties shared with worker threads. The cube mesher
/// only needs per-material colors; greedy/snap knobs were marching-cubes only.
pub struct MaterialConfig {
    pub colors: Vec<[f32; 3]>,
}

impl MaterialConfig {
    pub fn from_params(mat_params: &MaterialParams) -> Self {
        Self {
            colors: mat_params.entries.iter().map(|e| e.color).collect(),
        }
    }
}

// ============================================================================
// MeshingPipeline
// ============================================================================

#[derive(Resource)]
pub struct MeshingPipeline {
    request_tx: mpsc::SyncSender<MeshRequest>,
    result_rx: Mutex<Receiver<MeshResult>>,
    _workers: Mutex<Vec<JoinHandle<()>>>,

    chunk_states: HashMap<IVec3, ChunkMeshState>,
    material_config: Arc<RwLock<MaterialConfig>>,

    pub stats: MeshingStats,
    batch_start: Option<Instant>,
    pending_submissions: Vec<IVec3>,
    pending_set: HashSet<IVec3>,

    // Cache
    cache_stats: Arc<AtomicCacheStats>,
    cache_dir: PathBuf,
    cache_lru: Arc<Mutex<cache::MeshCacheLru>>,
}

impl MeshingPipeline {
    pub fn new(
        materials: &MaterialParams,
        _meshing: &crate::params::MeshingParams,
        mesh_cache: &crate::params::MeshCacheParams,
    ) -> Self {
        let usable = num_cpus::get().saturating_sub(2).max(2);
        let gen_workers = (usable / 3).max(2);
        let num_workers = (usable - gen_workers).max(2);

        let cache_dir = crate::paths::asset_root().join("cache").join("meshes");
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            log::warn!("Failed to create cache directory: {}", e);
        }

        let cache_stats = Arc::new(AtomicCacheStats::new());

        let lru = cache::MeshCacheLru::from_scan(&cache_dir);
        cache_stats.total_bytes.store(lru.total_bytes(), Ordering::Relaxed);
        let cache_lru = Arc::new(Mutex::new(lru));

        let material_config = Arc::new(RwLock::new(MaterialConfig::from_params(materials)));

        let cache_config = Arc::new(CacheConfig {
            cache_dir: cache_dir.clone(),
            stats: cache_stats.clone(),
            lru: cache_lru.clone(),
            max_cache_bytes: mesh_cache.max_size_bytes,
            eviction_batch_size: mesh_cache.eviction_batch_size,
        });

        let (req_tx, req_rx) = mpsc::sync_channel::<MeshRequest>(256);
        let (res_tx, res_rx) = mpsc::channel::<MeshResult>();
        let req_rx = Arc::new(Mutex::new(req_rx));

        let mut workers = Vec::with_capacity(num_workers);
        for i in 0..num_workers {
            let req_rx = req_rx.clone();
            let res_tx = res_tx.clone();
            let cache_config = cache_config.clone();
            let mat_config = material_config.clone();
            let handle = thread::Builder::new()
                .name(format!("mesh-worker-{}", i))
                .spawn(move || worker_loop(req_rx, res_tx, cache_config, mat_config))
                .expect("Failed to spawn mesh worker thread");
            workers.push(handle);
        }

        log::info!("MeshingPipeline: {} worker threads, cache at {:?}", num_workers, cache_dir);

        Self {
            request_tx: req_tx,
            result_rx: Mutex::new(res_rx),
            _workers: Mutex::new(workers),
            chunk_states: HashMap::new(),
            material_config,
            stats: MeshingStats { worker_count: num_workers, ..Default::default() },
            batch_start: None,
            pending_submissions: Vec::new(),
            pending_set: HashSet::new(),
            cache_stats,
            cache_dir,
            cache_lru,
        }
    }

    /// Queue all dirty chunks for meshing. Actual snapshot creation happens
    /// incrementally in drain_pending_submissions() to avoid blocking the main thread.
    pub fn submit_all_dirty(&mut self, world: &crate::world::World) {
        const DEBOUNCE_MS: u128 = 50;
        for (coord, chunk) in &world.chunks {
            if !chunk.mesh_dirty { continue; }
            if let Some(t) = chunk.mesh_debounce {
                if t.elapsed().as_millis() < DEBOUNCE_MS { continue; }
            }
            let pos: IVec3 = (*coord).into();
            let state = self.chunk_states.get(&pos).copied().unwrap_or(ChunkMeshState::Idle);
            if state != ChunkMeshState::Idle { continue; }
            if self.pending_set.insert(pos) {
                self.pending_submissions.push(pos);
            }
        }
        if !self.pending_submissions.is_empty() && self.batch_start.is_none() {
            self.batch_start = Some(Instant::now());
        }
    }

    pub fn drain_pending_submissions(&mut self, world: &crate::world::World) {
        const MAX_SNAPSHOTS_PER_FRAME: usize = 32;
        let mut submitted = 0;
        let mut i = 0;

        while i < self.pending_submissions.len() && submitted < MAX_SNAPSHOTS_PER_FRAME {
            let pos = self.pending_submissions[i];

            let state = self.chunk_states.get(&pos).copied().unwrap_or(ChunkMeshState::Idle);
            if state != ChunkMeshState::Idle {
                self.pending_submissions.swap_remove(i);
                self.pending_set.remove(&pos);
                continue;
            }

            let chunk = match world.get_chunk(pos) {
                Some(c) => c,
                None => {
                    self.pending_submissions.swap_remove(i);
                    self.pending_set.remove(&pos);
                    continue;
                }
            };

            // Cube meshing only needs face-adjacent neighbor materials.
            if !world.has_face_neighbors(pos) {
                i += 1;
                continue;
            }
            self.pending_submissions.swap_remove(i);
            self.pending_set.remove(&pos);

            let neighbors = world.build_neighbors(pos);
            let snapshot = ChunkSnapshot::extract(
                chunk, &neighbors, world.min_chunk_y, world.max_chunk_y,
            );
            let mesh_seq = chunk.mesh_seq;

            match self.request_tx.try_send(MeshRequest {
                chunk_key: pos,
                snapshot,
                mesh_seq,
            }) {
                Ok(()) => {
                    self.chunk_states.insert(pos, ChunkMeshState::InProgress);
                    submitted += 1;
                }
                Err(mpsc::TrySendError::Full(_)) => {
                    self.pending_submissions.push(pos);
                    self.pending_set.insert(pos);
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    log::error!("Mesh request channel disconnected");
                }
            }
        }
    }

    /// Poll for completed results. Returns meshes ready for GPU upload.
    pub fn poll(&mut self) -> Vec<MeshResult> {
        let mut completed = Vec::new();

        let rx = self.result_rx.get_mut().unwrap();
        while let Ok(result) = rx.try_recv() {
            self.chunk_states.remove(&result.chunk_key);
            self.stats.total_meshed += 1;
            completed.push(result);
        }

        self.stats.in_progress = self.chunk_states
            .values()
            .filter(|s| **s == ChunkMeshState::InProgress)
            .count();
        self.stats.pending_submissions = self.pending_submissions.len();

        self.stats.cache_hits = self.cache_stats.hits.load(Ordering::Relaxed);
        self.stats.cache_misses = self.cache_stats.misses.load(Ordering::Relaxed);
        self.stats.cache_errors = self.cache_stats.errors.load(Ordering::Relaxed);
        self.stats.cache_bytes = self.cache_stats.total_bytes.load(Ordering::Relaxed);
        self.stats.cache_files = self.cache_lru.lock().unwrap().entry_count() as u64;
        self.stats.cache_evictions = self.cache_stats.evictions.load(Ordering::Relaxed);

        if !completed.is_empty()
            && self.pending_submissions.is_empty()
            && self.stats.in_progress == 0
        {
            if let Some(start) = self.batch_start.take() {
                self.stats.last_batch_time_ms = start.elapsed().as_secs_f32() * 1000.0;
                log::info!(
                    "Meshing batch complete in {:.0}ms (cache: {} hits, {} misses)",
                    self.stats.last_batch_time_ms,
                    self.stats.cache_hits,
                    self.stats.cache_misses,
                );
            }
        }

        completed
    }

    pub fn pending_count(&self) -> usize {
        self.pending_submissions.len()
            + self.chunk_states.values().filter(|s| **s != ChunkMeshState::Idle).count()
    }

    pub fn is_idle(&self) -> bool {
        self.pending_submissions.is_empty()
            && self.chunk_states.values().all(|s| *s == ChunkMeshState::Idle)
    }

    pub fn remove_chunk_state(&mut self, pos: IVec3) {
        self.chunk_states.remove(&pos);
        if self.pending_set.remove(&pos) {
            self.pending_submissions.retain(|p| *p != pos);
        }
    }

    pub fn is_chunk_in_flight(&self, pos: IVec3) -> bool {
        matches!(self.chunk_states.get(&pos), Some(ChunkMeshState::InProgress))
    }

    pub fn reset_for_new_world(&mut self) {
        self.chunk_states.clear();
        self.pending_submissions.clear();
        self.pending_set.clear();
        self.batch_start = None;

        let worker_count = self.stats.worker_count;
        self.stats = MeshingStats { worker_count, ..Default::default() };

        let rx = self.result_rx.get_mut().unwrap();
        while rx.try_recv().is_ok() {}

        log::info!("MeshingPipeline reset for new world");
    }

    pub fn update_material_config(&self, materials: &MaterialParams, _meshing: &crate::params::MeshingParams) {
        let mut config = self.material_config.write().unwrap();
        *config = MaterialConfig::from_params(materials);
    }

    pub fn clear_cache(&self) {
        match cache::clear_cache(&self.cache_dir) {
            Ok(n) => log::info!("Cleared {} cached mesh files", n),
            Err(e) => log::warn!("Failed to clear mesh cache: {}", e),
        }
        self.cache_stats.reset();
        if let Ok(mut lru) = self.cache_lru.lock() {
            *lru = cache::MeshCacheLru::from_scan(&self.cache_dir);
        }
    }
}

// ============================================================================
// Worker thread
// ============================================================================

fn worker_loop(
    req_rx: Arc<Mutex<Receiver<MeshRequest>>>,
    res_tx: Sender<MeshResult>,
    cache_config: Arc<CacheConfig>,
    material_config: Arc<RwLock<MaterialConfig>>,
) {
    loop {
        let req = {
            let rx = match req_rx.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            match rx.recv() {
                Ok(r) => r,
                Err(_) => return,
            }
        };

        let mat_config = material_config.read().unwrap();
        let cache_key = cache::compute_cache_key(&req.snapshot, &mat_config.colors);

        let cache_path = cache::cache_file_path(
            &cache_config.cache_dir,
            req.snapshot.position.x,
            req.snapshot.position.y,
            req.snapshot.position.z,
            cache_key,
        );
        
        if let Some((vertices, indices)) = cache::load_cached_mesh(&cache_path, cache_key, &mat_config.colors) {
            cache_config.stats.hits.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut lru) = cache_config.lru.lock() {
                lru.touch(req.snapshot.position);
            }
            let _ = res_tx.send(MeshResult {
                chunk_key: req.chunk_key,
                vertices,
                indices,
                mesh_seq: req.mesh_seq,
            });
            continue;
        }

        cache_config.stats.misses.fetch_add(1, Ordering::Relaxed);
        let (vertices, indices) = generate_chunk_mesh(&req.snapshot, &mat_config);
        drop(mat_config);

        match cache::save_cached_mesh(
            &cache_config.cache_dir,
            req.snapshot.position,
            cache_key,
            &vertices,
            &indices,
        ) {
            Ok(bytes_written) => {
                if let Ok(mut lru) = cache_config.lru.lock() {
                    lru.insert(req.snapshot.position, cache_path, bytes_written);
                    if lru.total_bytes() > cache_config.max_cache_bytes {
                        let freed = lru.evict(
                            cache_config.max_cache_bytes,
                            cache_config.eviction_batch_size,
                        );
                        if freed > 0 {
                            cache_config.stats.evictions.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    cache_config.stats.total_bytes.store(lru.total_bytes(), Ordering::Relaxed);
                }
            }
            Err(e) => {
                log::warn!("Cache save failed for chunk {}: {}", req.chunk_key, e);
                cache_config.stats.errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        let _ = res_tx.send(MeshResult {
            chunk_key: req.chunk_key,
            vertices,
            indices,
            mesh_seq: req.mesh_seq,
        });
    }
}
