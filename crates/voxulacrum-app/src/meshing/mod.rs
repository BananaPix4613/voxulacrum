pub mod cache;
pub mod coordinator;
mod cube_mesher;

use glam::IVec3;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use bevy_ecs::prelude::Resource;

use crate::core_budget::CoreBudget;
use crate::jobs::{JobKind, JobSystem, Priority};
use crate::rendering::pipelines::FaceVertex;
use crate::world::chunk::ChunkSnapshot;
use cache::AtomicCacheStats;
use cube_mesher::generate_chunk_mesh;

// ============================================================================
// Messages
// ============================================================================

pub struct MeshResult {
    pub chunk_key: IVec3,
    pub vertices: Vec<FaceVertex>,
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
    pub cache_misses_cold: u64,
    pub cache_misses_stale: u64,
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

// ============================================================================
// MeshingPipeline
// ============================================================================

#[derive(Resource)]
pub struct MeshingPipeline {
    /// Cloned into each submitted mesh task to return its result. Behind a `Mutex`
    /// so the pipeline stays `Sync` (`mpsc::Sender` is not).
    result_tx: Mutex<Sender<MeshResult>>,
    result_rx: Mutex<Receiver<MeshResult>>,
    /// Shared cache config handed to each submitted job.
    cache_config: Arc<CacheConfig>,

    chunk_states: HashMap<IVec3, ChunkMeshState>,

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
    pub fn new(mesh_cache: &crate::params::MeshCacheParams) -> Self {
        let cache_dir = crate::paths::asset_root().join("cache").join("meshes");
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            log::warn!("Failed to create cache directory: {}", e);
        }

        let cache_stats = Arc::new(AtomicCacheStats::new());

        let lru = cache::MeshCacheLru::from_scan(&cache_dir);
        cache_stats.total_bytes.store(lru.total_bytes(), Ordering::Relaxed);
        let cache_lru = Arc::new(Mutex::new(lru));

        let cache_config = Arc::new(CacheConfig {
            cache_dir: cache_dir.clone(),
            stats: cache_stats.clone(),
            lru: cache_lru.clone(),
            max_cache_bytes: mesh_cache.max_size_bytes,
            eviction_batch_size: mesh_cache.eviction_batch_size,
        });

        let (res_tx, res_rx) = mpsc::channel::<MeshResult>();

        // Concurrency is the scheduler's to decide now; this is only the stat the
        // Performance panel shows.
        let worker_count = CoreBudget::detect().pool_threads;
        log::info!("MeshingPipeline: scheduler-backed meshing, cache at {cache_dir:?}");

        Self {
            result_tx: Mutex::new(res_tx),
            result_rx: Mutex::new(res_rx),
            cache_config,
            chunk_states: HashMap::new(),
            stats: MeshingStats { worker_count, ..Default::default() },
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

    pub fn drain_pending_submissions(
        &mut self,
        world: &crate::world::World,
        jobs: &mut JobSystem,
        camera_chunk: IVec3,
        max_snapshots: usize,
    ) {
        // Main-thread budget: each submission extracts a 34^3 snapshot, which is a
        // real per-frame cost independent of how many jobs may run concurrently.
        // Operator-tunable via `StreamingParams::max_mesh_per_frame`.
        let mut submitted = 0;
        let mut i = 0;

        // Nearest-first: sort the pending set by distance to the camera so the
        // scan visits chunks in the order the player will see them. Without this,
        // `submit_all_dirty` fills the vec in `HashMap` order and `swap_remove`
        // scrambles it further, so which chunks got deferred by the face-neighbor
        // check below was effectively random - the scattered late-meshing chunks.
        self.pending_submissions.sort_unstable_by_key(|p| {
            let dx = p.x - camera_chunk.x;
            let dz = p.z - camera_chunk.z;
            std::cmp::Reverse(dx * dx + dz * dz)
        });
        
        let cap = jobs.submission_limit(JobKind::Mesh);
        let mut in_flight = self
            .chunk_states
            .values()
            .filter(|s| **s == ChunkMeshState::InProgress)
            .count();

        while i < self.pending_submissions.len()
            && submitted < max_snapshots
            && in_flight < cap
        {
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
                chunk,
                &neighbors,
                world.min_chunk_y,
                world.max_chunk_y,
                world.materials.render_delegated_mask(),
            );
            let mesh_seq = chunk.mesh_seq;

            // Submit to the scheduler; the job returns over the result channel,
            // which `poll` drains. An edit-dirtied chunk (the only thing that sets
            // `mesh_debounce`) rides the interactive class so a placed block
            // re-meshes ahead of bulk streaming.
            let dx = pos.x - camera_chunk.x;
            let dz = pos.z - camera_chunk.z;
            let distance = (dx * dx + dz * dz).max(0) as u32;
            let priority = if chunk.mesh_debounce.is_some() {
                Priority::interactive(distance)
            } else {
                Priority::streaming(distance)
            };
            
            let res_tx = self.result_tx.lock().unwrap().clone();
            let cache_config = self.cache_config.clone();
            jobs.submit(JobKind::Mesh, pos, priority, move || {
                mesh_chunk_task(pos, snapshot, mesh_seq, res_tx, cache_config);
            });
            self.chunk_states.insert(pos, ChunkMeshState::InProgress);
            submitted += 1;
            in_flight += 1;
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
        self.stats.cache_misses_cold = self.cache_stats.misses_cold.load(Ordering::Relaxed);
        self.stats.cache_misses_stale = self.cache_stats.misses_stale.load(Ordering::Relaxed);
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
// Mesh task (runs on the shared generation pool)
// ============================================================================

/// Load-or-mesh one chunk snapshot and return the result over `res_tx`. Runs as a
/// fire-and-forget task on the shared `gen_pool` (Phase-2 unified worker model),
/// replacing the former dedicated mesh-worker threads.
fn mesh_chunk_task(
    chunk_key: IVec3,
    snapshot: ChunkSnapshot,
    mesh_seq: u64,
    res_tx: Sender<MeshResult>,
    cache_config: Arc<CacheConfig>,
) {
    let cache_key = cache::compute_cache_key(&snapshot);

    let cache_path = cache::cache_file_path(
        &cache_config.cache_dir,
        snapshot.position.x,
        snapshot.position.y,
        snapshot.position.z,
        cache_key,
    );

    if let Some((vertices, indices)) = cache::load_cached_mesh(&cache_path, cache_key) {
        cache_config.stats.hits.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut lru) = cache_config.lru.lock() {
            lru.touch(snapshot.position);
        }
        let _ = res_tx.send(MeshResult { chunk_key, vertices, indices, mesh_seq });
        return;
    }

    cache_config.stats.misses.fetch_add(1, Ordering::Relaxed);
    // Attribute the miss: a position the LRU already knows means the content
    // changed and this is a re-key (drift observation 5.1's churn), not a first
    // visit. One map lookup, on a path that already takes this lock on hits.
    let had_entry = cache_config
        .lru
        .lock()
        .map(|lru| lru.contains(snapshot.position))
        .unwrap_or(false);
    if had_entry {
        cache_config.stats.misses_stale.fetch_add(1, Ordering::Relaxed);
    } else {
        cache_config.stats.misses_cold.fetch_add(1, Ordering::Relaxed);
    }
    let (vertices, indices) = generate_chunk_mesh(&snapshot);

    match cache::save_cached_mesh(
        &cache_config.cache_dir,
        snapshot.position,
        cache_key,
        &vertices,
        &indices,
    ) {
        Ok(bytes_written) => {
            if let Ok(mut lru) = cache_config.lru.lock() {
                lru.insert(snapshot.position, cache_path, bytes_written);
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
            log::warn!("Cache save failed for chunk {}: {}", chunk_key, e);
            cache_config.stats.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    let _ = res_tx.send(MeshResult { chunk_key, vertices, indices, mesh_seq });
}
