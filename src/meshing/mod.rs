pub mod dual_contouring;
pub mod qef;

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use glam::IVec3;

use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::ChunkSnapshot;
use dual_contouring::{
    BoundaryVertexMap, CellVertexData, OwnedNeighborBoundaries,
    generate_cell_vertices_from_snapshot, generate_faces_from_snapshot,
};

// ============================================================================
// Messages
// ============================================================================

struct Phase1Request {
    chunk_index: usize,
    snapshot: ChunkSnapshot,
}

struct Phase1Result {
    chunk_index: usize,
    cell_data: CellVertexData,
    snapshot: ChunkSnapshot,
}

struct Phase2Request {
    chunk_index: usize,
    cell_data: CellVertexData,
    snapshot: ChunkSnapshot,
    neighbor_boundaries: OwnedNeighborBoundaries,
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
}

// ============================================================================
// MeshingPipeline
// ============================================================================

pub struct MeshingPipeline {
    p1_request_tx: Sender<Phase1Request>,
    p1_result_rx: Receiver<Phase1Result>,
    p2_request_tx: Sender<Phase2Request>,
    p2_result_rx: Receiver<Phase2Result>,
    _workers: Vec<JoinHandle<()>>,

    chunk_states: Vec<ChunkMeshState>,
    pending_phase1: HashMap<usize, (CellVertexData, ChunkSnapshot)>,
    boundary_maps: HashMap<usize, BoundaryVertexMap>,

    chunks_x: usize,
    chunks_y: usize,
    chunks_z: usize,

    pub stats: MeshingStats,
    batch_start: Option<Instant>,
    pending_submissions: Vec<usize>,
}

impl MeshingPipeline {
    pub fn new(chunks_x: usize, chunks_y: usize, chunks_z: usize) -> Self {
        let total_chunks = chunks_x * chunks_y * chunks_z;
        let num_workers = num_cpus::get().saturating_sub(2).max(2);

        let (p1_tx, p1_rx) = mpsc::channel::<Phase1Request>();
        let (p1_result_tx, p1_result_rx) = mpsc::channel::<Phase1Result>();
        let (p2_tx, p2_rx) = mpsc::channel::<Phase2Request>();
        let (p2_result_tx, p2_result_rx) = mpsc::channel::<Phase2Result>();

        let p1_rx = Arc::new(Mutex::new(p1_rx));
        let p2_rx = Arc::new(Mutex::new(p2_rx));

        let mut workers = Vec::with_capacity(num_workers);
        for i in 0..num_workers {
            let p1_rx = p1_rx.clone();
            let p1_result_tx = p1_result_tx.clone();
            let p2_rx = p2_rx.clone();
            let p2_result_tx = p2_result_tx.clone();

            let handle = thread::Builder::new()
                .name(format!("mesh-worker-{}", i))
                .spawn(move || {
                    worker_loop(p1_rx, p1_result_tx, p2_rx, p2_result_tx);
                })
                .expect("Failed to spawn mesh worker thread");

            workers.push(handle);
        }

        log::info!("MeshingPipeline: {} worker threads", num_workers);

        Self {
            p1_request_tx: p1_tx,
            p1_result_rx,
            p2_request_tx: p2_tx,
            p2_result_rx,
            _workers: workers,
            chunk_states: vec![ChunkMeshState::Idle; total_chunks],
            pending_phase1: HashMap::new(),
            boundary_maps: HashMap::new(),
            chunks_x,
            chunks_y,
            chunks_z,
            stats: MeshingStats {
                worker_count: num_workers,
                ..Default::default()
            },
            batch_start: None,
            pending_submissions: Vec::new(),
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

            self.chunk_states[i] = ChunkMeshState::Phase1InProgress;
            let _ = self.p1_request_tx.send(Phase1Request {
                chunk_index: i,
                snapshot,
            });
        }
    }

    /// Poll for completed results. Returns meshes ready for GPU upload.
    pub fn poll(&mut self) -> Vec<Phase2Result> {
        let mut completed = Vec::new();

        // Drain Phase 1 results
        while let Ok(result) = self.p1_result_rx.try_recv() {
            let idx = result.chunk_index;
            self.chunk_states[idx] = ChunkMeshState::Phase1Complete;
            self.boundary_maps.insert(idx, result.cell_data.boundary_map.clone());
            self.pending_phase1.insert(idx, (result.cell_data, result.snapshot));
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
        self.stats.phase1_in_progress = self.chunk_states.iter()
            .filter(|s| **s == ChunkMeshState::Phase1InProgress).count();
        self.stats.phase1_complete = self.chunk_states.iter()
            .filter(|s| **s == ChunkMeshState::Phase1Complete).count();
        self.stats.phase2_in_progress = self.chunk_states.iter()
            .filter(|s| **s == ChunkMeshState::Phase2InProgress).count();
        self.stats.pending_submissions = self.pending_submissions.len();

        // Record batch time when everything finishes
        if !completed.is_empty()
            && self.pending_submissions.is_empty()
            && self.stats.phase1_in_progress == 0
            && self.stats.phase1_complete == 0
            && self.stats.phase2_in_progress == 0
        {
            if let Some(start) = self.batch_start.take() {
                self.stats.last_batch_time_ms = start.elapsed().as_secs_f32() * 1000.0;
                log::info!("Meshing batch complete in {:.0}ms", self.stats.last_batch_time_ms);
            }
        }

        completed
    }

    fn can_dispatch_phase2(&self, chunk_index: usize) -> bool {
        let (cx, cy, cz) = self.index_to_coords(chunk_index);

        for dz in 0u8..=1 {
            for dy in 0u8..=1 {
                for dx in 0u8..=1 {
                    if dx == 0 && dy == 0 && dz == 0 { continue; }
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
        let (cell_data, snapshot) = match self.pending_phase1.remove(&chunk_index) {
            Some(data) => data,
            None => return,
        };

        let (cx, cy, cz) = self.index_to_coords(chunk_index);

        let mut nb = OwnedNeighborBoundaries::empty();
        for dz in 0u8..=1 {
            for dy in 0u8..=1 {
                for dx in 0u8..=1 {
                    if dx == 0 && dy == 0 && dz == 0 { continue; }
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

        self.chunk_states[chunk_index] = ChunkMeshState::Phase2InProgress;

        let _ = self.p2_request_tx.send(Phase2Request {
            chunk_index,
            cell_data,
            snapshot,
            neighbor_boundaries: nb,
        });
    }

    pub fn is_idle(&self) -> bool {
        self.pending_submissions.is_empty()
            && self.chunk_states.iter().all(|s| *s == ChunkMeshState::Idle)
            && self.pending_phase1.is_empty()
    }

    pub fn clear_boundary_maps(&mut self) {
        self.boundary_maps.clear();
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
) {
    loop {
        // Try Phase 2 first (higher priority - unblocks GPU upload)
        if let Ok(rx) = p2_rx.lock() {
            if let Ok(req) = rx.try_recv() {
                drop(rx);
                let mut cell_data = req.cell_data;
                let nb = req.neighbor_boundaries.as_ref();
                let indices = generate_faces_from_snapshot(&mut cell_data, &req.snapshot, &nb);
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
                    let cell_data = generate_cell_vertices_from_snapshot(&req.snapshot);
                    let _ = p1_result_tx.send(Phase1Result {
                        chunk_index: req.chunk_index,
                        cell_data,
                        snapshot: req.snapshot,
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