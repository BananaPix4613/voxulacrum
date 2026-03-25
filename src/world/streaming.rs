use std::collections::{BinaryHeap, HashSet};
use std::cmp::Reverse;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use bevy_ecs::prelude::Resource;
use glam::{IVec3, Vec3};

use crate::meshing::coordinator::MeshingCoordinator;
use crate::params::{StreamingParams, TerrainGenParams};
use crate::world::chunk::CHUNK_WORLD_SIZE;
use crate::world::generation::TerrainGenerator;
use crate::world::World;

/// Shared work queue between main thread and generation workers.
/// The main thread clears and rebuilds this every frame with current
/// priorities. Workers pop the highest-priority (lowest distance) item.
struct SharedWorkQueue {
    /// Min-heap by squared distance from camera. Item: (priority, [x, y, z]).
    /// IVec3 doesn't implement Ord, so we store as [i32; 3].
    queue: Mutex<BinaryHeap<Reverse<(i32, [i32; 3])>>>,
    condvar: Condvar,
    shutdown: AtomicBool,
}

impl SharedWorkQueue {
    fn new() -> Self {
        Self {
            queue: Mutex::new(BinaryHeap::new()),
            condvar: Condvar::new(),
            shutdown: AtomicBool::new(false),
        }
    }
}

struct GenResult {
    chunk: crate::world::chunk::Chunk,
}

/// Camera state needed for frustum-based chunk loading.
pub struct CameraView {
    pub zoom: f32,
    pub aspect: f32,
    pub rotation: f32,
    pub camera_chunk: IVec3,
    pub camera_world_pos: Vec3,
}

impl CameraView {
    /// Test if a chunk XZ position falls inside the camera's screen-rectangle
    /// projected onto the ground plane (a rotated parallelogram in world space).
    ///
    /// For rotation `r` and isometric pitch θ = atan(1/√2):
    ///   right_dir = (sin(r), -cos(r))   — screen X axis on ground
    ///   up_dir    = (-cos(r), -sin(r))   — screen Y axis on ground (foreshortened)
    ///
    /// Half-extents (in chunk space):
    ///   half_right = zoom * aspect / CHUNK_WORLD_SIZE + margin
    ///   half_up    = zoom * sin(θ) / CHUNK_WORLD_SIZE + margin
    ///
    /// A chunk is inside the view rectangle if BOTH projections are within bounds:
    ///   |dot(offset, right_dir)| <= half_right  AND  |dot(offset, up_dir)| <= half_up
    pub fn is_in_view_rect(&self, chunk_x: i32, chunk_z: i32, margin: i32) -> bool {
        let sin_theta = (1.0_f32 / 3.0).sqrt(); // sin(atan(1/√2)) = 1/√3
        let dx = (chunk_x - self.camera_chunk.x) as f32;
        let dz = (chunk_z - self.camera_chunk.z) as f32;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Project offset onto camera-local axes on ground plane
        let local_right = dx * sin_r - dz * cos_r;     // along screen X
        let local_up    = -dx * cos_r - dz * sin_r;    // along screen Y (ground component)

        let half_right = self.zoom * self.aspect / CHUNK_WORLD_SIZE + margin as f32;
        let half_up    = self.zoom * sin_theta / CHUNK_WORLD_SIZE + margin as f32;

        // Rectangle test: both projections must be within their half-extent
        local_right.abs() <= half_right && local_up.abs() <= half_up
    }

    /// Priority score: lower = more urgent to load.
    /// Simple Euclidean distance in chunk space from camera_chunk center.
    /// The diamond test already filters out irrelevant chunks, so
    /// center-first loading is the main priority need.
    pub fn priority_score(&self, pos: IVec3) -> f32 {
        let dx = (pos.x - self.camera_chunk.x) as f32;
        let dz = (pos.z - self.camera_chunk.z) as f32;
        (dx * dx + dz * dz).sqrt()
    }

    /// Bounding Chebyshev radius that fully contains the rotated view rectangle
    /// + margin. This is the AABB half-extent of the rotated rectangle in
    /// world-axis-aligned chunk space.
    pub fn bounding_radius(&self, margin: i32) -> i32 {
        let sin_theta = (1.0_f32 / 3.0).sqrt();
        let half_right = self.zoom * self.aspect / CHUNK_WORLD_SIZE + margin as f32;
        let half_up    = self.zoom * sin_theta / CHUNK_WORLD_SIZE + margin as f32;

        let cos_r = self.rotation.cos().abs();
        let sin_r = self.rotation.sin().abs();

        // AABB of the rotated rectangle
        let aabb_half_x = half_right * sin_r + half_up * cos_r;
        let aabb_half_z = half_right * cos_r + half_up * sin_r;
        aabb_half_x.max(aabb_half_z).ceil() as i32
    }
}

/// Result of a streaming tick — lists of chunk positions that changed.
pub struct StreamingTickResult {
    pub inserted: Vec<IVec3>,
    pub unloaded: Vec<IVec3>,
}

#[derive(Resource)]
pub struct ChunkStreamingManager {
    pub params: StreamingParams,
    last_camera_chunk: Option<IVec3>,
    work_queue: Arc<SharedWorkQueue>,
    /// Tracks chunks that workers have popped from the queue and are actively
    /// generating. Main thread removes entries when it receives results.
    /// This prevents re-queuing chunks that are mid-generation.
    in_flight: Arc<Mutex<HashSet<IVec3>>>,
    gen_result_rx: Mutex<mpsc::Receiver<GenResult>>,
    _workers: Vec<JoinHandle<()>>,
}

impl ChunkStreamingManager {
    pub fn new(
        params: &StreamingParams,
        terrain_params: &TerrainGenParams,
    ) -> Self {
        let num_workers = 4.min(num_cpus::get().saturating_sub(2).max(1));

        let work_queue = Arc::new(SharedWorkQueue::new());
        let in_flight: Arc<Mutex<HashSet<IVec3>>> = Arc::new(Mutex::new(HashSet::new()));
        let (result_tx, result_rx) = mpsc::channel::<GenResult>();

        let terrain_params = Arc::new(terrain_params.clone());

        let mut workers = Vec::with_capacity(num_workers);
        for i in 0..num_workers {
            let wq = work_queue.clone();
            let flight = in_flight.clone();
            let tx = result_tx.clone();
            let tp = terrain_params.clone();

            let handle = std::thread::Builder::new()
                .name(format!("chunk-gen-{}", i))
                .spawn(move || {
                    let generator = TerrainGenerator::new(&tp);
                    loop {
                        // Pop highest-priority item (lowest squared distance)
                        let pos = {
                            let mut q = wq.queue.lock().unwrap();
                            loop {
                                if wq.shutdown.load(Ordering::Relaxed) {
                                    return;
                                }
                                if let Some(Reverse((_, arr))) = q.pop() {
                                    break IVec3::new(arr[0], arr[1], arr[2]);
                                }
                                // Queue empty - wait for main thread to rebuild it
                                q = wq.condvar.wait(q).unwrap();
                            }
                        };

                        // Mark as in-flight so main thread won't re-queue it
                        flight.lock().unwrap().insert(pos);

                        let mut chunk = crate::world::chunk::Chunk::new(pos);
                        generator.generate_chunk(&mut chunk, &tp);

                        if tx.send(GenResult { chunk }).is_err() {
                            return; // Main thread dropped, shut down
                        }
                    }
                })
                .expect("Failed to spawn chunk generation worker");

            workers.push(handle);
        }

        log::info!("ChunkStreamingManager: {} generation workers", num_workers);

        Self {
            params: params.clone(),
            last_camera_chunk: None,
            work_queue,
            in_flight,
            gen_result_rx: Mutex::new(result_rx),
            _workers: workers,
        }
    }

    /// Rebuild workers when terrain params change (new generator needed).
    pub fn rebuild_for_new_params(&mut self, terrain_params: &TerrainGenParams) {
        *self = Self::new(&self.params, terrain_params);
        self.last_camera_chunk = None;
    }

    /// Main tick: load/unload chunks based on camera position and frustum.
    ///
    /// Returns the list of newly inserted and unloaded chunk positions so the
    /// caller can do incremental work (e.g. vegetation per-chunk).
    pub fn tick(
        &mut self,
        world: &mut World,
        meshing: &mut MeshingCoordinator,
        camera_view: &CameraView,
        _dt: f32,
    ) -> StreamingTickResult {
        let min_y = self.params.min_chunk_y;
        let max_y = self.params.max_chunk_y;
        let load_margin = self.params.load_distance as i32;
        let unload_margin = self.params.unload_distance as i32;

        let cam_cx = camera_view.camera_chunk.x;
        let cam_cz = camera_view.camera_chunk.z;

        let mut result = StreamingTickResult {
            inserted: Vec::new(),
            unloaded: Vec::new(),
        };

        // --- Poll completed chunk generations ---
        {
            let rx = self.gen_result_rx.get_mut().unwrap();
            let mut flight = self.in_flight.lock().unwrap();
            while let Ok(gen_result) = rx.try_recv() {
                let pos = gen_result.chunk.position;
                flight.remove(&pos);

                // Discard if chunk is now outside the view rect (camera moved)
                if !camera_view.is_in_view_rect(pos.x, pos.z, unload_margin) {
                    continue;
                }

                world.insert_chunk(gen_result.chunk);
                result.inserted.push(pos);

                // Mark face-adjacent neighbors for re-meshing (seam fix)
                for &offset in &[
                    IVec3::X, IVec3::NEG_X,
                    IVec3::Y, IVec3::NEG_Y,
                    IVec3::Z, IVec3::NEG_Z,
                ] {
                    let neighbor_pos = pos + offset;
                    if let Some(neighbor) = world.chunks.get_mut(&neighbor_pos) {
                        neighbor.mesh_dirty = true;
                    }
                }
            }
        }

        if !result.inserted.is_empty() {
            meshing.pipeline.submit_all_dirty(world);
        }

        // --- Rebuild the work queue with current priorities ---
        // Snapshot in_flight first, then lock queue (consistent lock ordering).
        let in_flight_snapshot: HashSet<IVec3> = self.in_flight.lock().unwrap().clone();
        {
            let mut q = self.work_queue.queue.lock().unwrap();
            q.clear();

            let radius = camera_view.bounding_radius(load_margin);
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let cx = cam_cx + dx;
                    let cz = cam_cz + dz;
                    if !camera_view.is_in_view_rect(cx, cz, load_margin) {
                        continue;
                    }
                    for cy in min_y..max_y {
                        let pos = IVec3::new(cx, cy, cz);
                        if !world.chunks.contains_key(&pos)
                            && !in_flight_snapshot.contains(&pos)
                        {
                            let priority = dx * dx + dz * dz;
                            q.push(Reverse((priority, [cx, cy, cz])));
                        }
                    }
                }
            }
        }
        // Wake all workers so they grab new high-priority items
        self.work_queue.condvar.notify_all();

        // --- Unload chunks outside the view rectangle ---
        self.last_camera_chunk = Some(IVec3::new(cam_cx, 0, cam_cz));

        let to_unload: Vec<IVec3> = world
            .chunks
            .keys()
            .filter(|pos| !camera_view.is_in_view_rect(pos.x, pos.z, unload_margin))
            .copied()
            .collect();

        for pos in to_unload {
            if meshing.pipeline.is_chunk_in_flight(pos) {
                continue;
            }
            meshing.pipeline.remove_chunk_state(pos);
            world.remove_chunk(pos);
            result.unloaded.push(pos);
        }

        result
    }

    pub fn pending_gen_count(&self) -> usize {
        let in_flight = self.in_flight.lock().unwrap().len();
        let queued = self.work_queue.queue.lock().unwrap().len();
        in_flight + queued
    }
}

impl Drop for ChunkStreamingManager {
    fn drop(&mut self) {
        // Signal workers to exit and wake them from condvar wait
        self.work_queue.shutdown.store(true, Ordering::Relaxed);
        self.work_queue.condvar.notify_all();
        // JoinHandles drop here, detaching threads.
        // Workers will see the shutdown flag and return.
    }
}
