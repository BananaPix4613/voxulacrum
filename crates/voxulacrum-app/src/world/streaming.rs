use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use bevy_ecs::prelude::Resource;
use glam::{IVec3, Vec3};

use crate::meshing::coordinator::MeshingCoordinator;
use crate::params::StreamingParams;
use crate::world::chunk::CHUNK_WORLD_SIZE;
use crate::world::overrides::ChunkOverrides;
use crate::world::persistence::{ChunkEdits, ChunkRecord, WorldDatabase};
use crate::world::world_generator::{GeneratedChunk, WorldGenerator};
use crate::world::World;
use crate::world::mutation::{MutationCommand, WorldMutation};

/// Monotonic, process-global token identifying the current DB configuration.
/// Each `ChunkStreamingManager` (including every rebuild after a regen) claims a
/// fresh value via `fetch_add`. Generation tasks carry the token they were
/// spawned under; a pool worker's thread-local DB handle (see `with_worker_db`)
/// reopens whenever the token it cached differs from the task's token. Because
/// the counter only ever increases, a rebuilt manager can never collide with a
/// handle a worker cached for a previous manager instance.
static NEXT_DB_GENERATION: AtomicU64 = AtomicU64::new(0);

struct GenResult {
    chunk: crate::world::chunk::LoadedChunk,
}

/// Run `f` with this pool worker's cached read-only DB handle for `generation`.
///
/// Chunk generation now runs as fire-and-forget tasks on the shared rayon pool
/// rather than on dedicated threads that each owned a DB handle for life. To
/// keep the "one reused read-only handle per worker" property, each pool worker
/// memoizes its handle in a thread-local, keyed by the DB generation token. When
/// a task arrives carrying a newer token (after a regen rebuilt the manager with
/// a possibly-different DB path/dictionary), the worker transparently reopens.
/// A `None` handle (no DB path, or open failure) is cached too, so a failed open
/// is not retried on every chunk.
fn with_worker_db<R>(
    generation: u64,
    db_path: &Option<PathBuf>,
    dict: Option<&[u8]>,
    f: impl FnOnce(Option<&WorldDatabase>) -> R,
) -> R {
    thread_local! {
        static WORKER_DB: RefCell<Option<(u64, Option<WorldDatabase>)>> = RefCell::new(None);
    }
    WORKER_DB.with(|cell| {
        let mut slot = cell.borrow_mut();
        let stale = match &*slot {
            Some((cached_gen, _)) => *cached_gen != generation,
            None => true,
        };
        if stale {
            let db = db_path.as_ref().and_then(|path| {
                WorldDatabase::open_readonly(path, dict)
                    .map_err(|e| log::warn!("Streaming worker DB open failed: {e}"))
                    .ok()
            });
            *slot = Some((generation, db));
        }
        let db_ref = slot.as_ref().unwrap().1.as_ref();
        f(db_ref)
    })
}

/// Overlay a chunk's persisted `fluid_diffs` onto its generated fluid layer as
/// settled cells (design doc §7 / §9: player edits persist; the active set
/// re-derives at runtime).
fn apply_persisted_fluid(
    fluids: &mut crate::world::layers::FluidLayer,
    overrides: &Option<ChunkOverrides>,
) {
    let Some(ovr) = overrides else { return };
    for (&pos, &cell) in &ovr.fluid_diffs {
        fluids.cells.insert(pos, cell);
        // Cells that were mid-flow at save time resume flowing; settled cells stay
        // put (the settled flag mirrors the active set - see FluidLayer::settle).
        if cell.flags & crate::world::layers::FluidCell::FLAG_SETTLED == 0 {
            fluids.activate(pos);
        }
    }
}

/// Camera state needed for frustum-based chunk loading.
pub struct CameraView {
    pub zoom: f32,
    pub aspect: f32,
    pub rotation: f32,
    pub camera_chunk: IVec3,
    #[allow(dead_code)] // camera world-space position; retained for view math
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
    /// Half-extents (in chunk space), with proportional margin:
    ///   half_right = zoom * aspect / CHUNK_WORLD_SIZE * (1 + margin_fraction) + MIN_MARGIN
    ///   half_up    = zoom * sin(θ) / CHUNK_WORLD_SIZE * (1 + margin_fraction) + MIN_MARGIN
    ///
    /// A chunk is inside the view rectangle if BOTH projections are within bounds:
    ///   |dot(offset, right_dir)| <= half_right  AND  |dot(offset, up_dir)| <= half_up
    pub fn is_in_view_rect(&self, chunk_x: i32, chunk_z: i32, margin_fraction: f32) -> bool {
        let (half_right, half_up) = self.half_extents(margin_fraction);

        let dx = (chunk_x - self.camera_chunk.x) as f32;
        let dz = (chunk_z - self.camera_chunk.z) as f32;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Project offset onto camera-local axes on ground plane
        let local_right = dx * sin_r - dz * cos_r;     // along screen X
        let local_up    = -dx * cos_r - dz * sin_r;    // along screen Y (ground component)

        // Rectangle test: both projections must be within their half-extent
        local_right.abs() <= half_right && local_up.abs() <= half_up
    }

    /// Compute the half-extents of the view rectangle with proportional margin.
    /// Returns (half_right, half_up) in chunk-space units.
    ///
    /// The margin is computed as a fraction of the LARGER visible axis and then
    /// applied equally to both axes. This ensures the forward/backward direction
    /// (foreshortened by the isometric pitch) gets the same absolute buffer as
    /// the horizontal direction, preventing pop-in when the camera moves forward.
    fn half_extents(&self, margin_fraction: f32) -> (f32, f32) {
        const MIN_MARGIN: f32 = 2.0; // minimum buffer in chunks, prevents pop-in at low zoom
        let _sin_theta = (1.0_f32 / 3.0).sqrt(); // sin(atan(1/√2)) = 1/√3
        let half_right_vis = self.zoom * self.aspect / CHUNK_WORLD_SIZE;
        let half_up_vis    = self.zoom * self.aspect / CHUNK_WORLD_SIZE;

        // Derive margin from the larger axis so both get adequate buffer
        let margin_chunks = (half_right_vis.max(half_up_vis) * margin_fraction).max(MIN_MARGIN);
        (half_right_vis + margin_chunks, half_up_vis + margin_chunks)
    }

    /// Priority score: lower = more urgent to load.
    /// Simple Euclidean distance in chunk space from camera_chunk center.
    /// The diamond test already filters out irrelevant chunks, so
    /// center-first loading is the main priority need.
    #[allow(dead_code)] // load-priority helper; center-first loading API
    pub fn priority_score(&self, pos: IVec3) -> f32 {
        let dx = (pos.x - self.camera_chunk.x) as f32;
        let dz = (pos.z - self.camera_chunk.z) as f32;
        (dx * dx + dz * dz).sqrt()
    }

    /// Bounding Chebyshev radius that fully contains the rotated view rectangle
    /// with proportional margin. This is the AABB half-extent of the rotated
    /// rectangle in world-axis-aligned chunk space.
    pub fn bounding_radius(&self, margin_fraction: f32) -> i32 {
        let (half_right, half_up) = self.half_extents(margin_fraction);

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
    /// Shared chunk-generation pool (also used by the startup fill and regen).
    pool: Arc<rayon::ThreadPool>,
    /// Current generator; cloned into each spawned generation task.
    generator: Arc<WorldGenerator>,
    /// Read-only voxel-edit DB inputs handed to each task's worker handle.
    db_path: Option<PathBuf>,
    dict_bytes: Option<Arc<Vec<u8>>>,
    /// DB generation token for this manager instance (see `NEXT_DB_GENERATION`).
    db_generation: u64,
    /// Positions with a generation task in flight. Main-thread-only: tasks are
    /// recorded here on spawn and cleared when their result arrives, so no lock
    /// is needed. Doubles as the dedup set when rebuilding the per-frame queue.
    in_flight: HashSet<IVec3>,
    /// Upper bound on concurrent in-flight generations. Self-bounds streaming's
    /// share of the shared pool so it cannot starve regen or the startup fill.
    max_in_flight: usize,
    result_tx: mpsc::Sender<GenResult>,
    gen_result_rx: Mutex<mpsc::Receiver<GenResult>>,
}

impl ChunkStreamingManager {
    pub fn new(
        pool: Arc<rayon::ThreadPool>,
        params: &StreamingParams,
        generator: Arc<WorldGenerator>,
        db_path: Option<PathBuf>,
        dict_bytes: Option<Arc<Vec<u8>>>,
    ) -> Self {
        let max_in_flight = crate::core_budget::CoreBudget::detect().gen_in_flight;
        let db_generation = NEXT_DB_GENERATION.fetch_add(1, Ordering::Relaxed);
        let (result_tx, result_rx) = mpsc::channel::<GenResult>();

        log::info!(
            "ChunkStreamingManager: pool-backed generation (max {} in flight, db gen {})",
            max_in_flight, db_generation
        );

        Self {
            params: params.clone(),
            last_camera_chunk: None,
            pool,
            generator,
            db_path,
            dict_bytes,
            db_generation,
            in_flight: HashSet::new(),
            max_in_flight,
            result_tx,
            gen_result_rx: Mutex::new(result_rx),
        }
    }

    /// Rebuild to use a new shared generator (e.g. after regen). Recreating the
    /// manager drops the old result channel - generation tasks still in flight
    /// from the previous generator find their sender disconnected and discard
    /// their output - and claims a fresh DB generation token so pool workers
    /// reopen their handles against the new DB inputs.
    pub fn rebuild_for_new_params(
        &mut self,
        generator: Arc<WorldGenerator>,
        db_path: Option<PathBuf>,
        dict_bytes: Option<Arc<Vec<u8>>>,    
    ) {
        *self = Self::new(self.pool.clone(), &self.params, generator, db_path, dict_bytes);
        self.last_camera_chunk = None;
    }

    /// Submit one chunk-generation task to the shared pool. Records `pos` in the
    /// in-flight set; the task generates base terrain, overlays saved voxel
    /// edits via this worker's cached read-only DB handle, and returns the chunk
    /// over the result channel. If the manager has since been rebuilt, the send
    /// fails and the result is silently dropped.
    fn spawn_generation(&mut self, pos: IVec3) {
        self.in_flight.insert(pos);

        let gen = Arc::clone(&self.generator);
        let db_path = self.db_path.clone();
        let dict = self.dict_bytes.clone();
        let generation = self.db_generation;
        let tx = self.result_tx.clone();

        self.pool.spawn(move || {
            // Generate base terrain + identity tags.
            let GeneratedChunk { mut storage, tags: generated_tags, detail_layers, scatter, fluids, smoothing_distances } =
                gen.generate_chunk(pos);

            // Overlay saved edits from this worker's cached read-only DB handle.
            let mut overrides = None;
            let mut loaded_tags = None;
            let mut loaded_seam_finalized = false;
            with_worker_db(
                generation,
                &db_path,
                dict.as_deref().map(|v| v.as_slice()),
                |db| {
                    if let Some(db) = db {
                        match db.load_chunk_record(pos) {
                            Ok(Some(ChunkRecord { edits, tags, seam_finalized })) => {
                                match edits {
                                    ChunkEdits::Delta(delta) => {
                                        // Self-healing: drop overrides that already
                                        // match base terrain (no-ops). Phase 3 only
                                        // re-applies voxel_diffs; the other override
                                        // layers round-trip in the blob but have no
                                        // runtime consumer yet.
                                        let mut filtered = ChunkOverrides::default();
                                        for (cell, voxel) in &delta.voxel_diffs {
                                            if storage.voxel(cell.to_index()) != *voxel {
                                                filtered.voxel_diffs.insert(*cell, *voxel);
                                            }
                                        }

                                        if !filtered.voxel_diffs.is_empty() {
                                            storage = crate::world::persistence::apply_overrides_to_storage(&storage, &filtered);
                                        }
                                        // Scatter overrides now have a runtime consumer (Phase 5),
                                        // so carry the player diff forward; without this, placed/
                                        // removed props are lost on reload. (detail/fluid/decal
                                        // still round-trip only, no consumer yet.)
                                        filtered.scatter_removed = delta.scatter_removed;
                                        filtered.scatter_added = delta.scatter_added;
                                        // Fluid now has a runtime consumer (Phase 7): carry
                                        // persisted player-poured water forward too.
                                        filtered.fluid_diffs = delta.fluid_diffs;
                                        overrides = if filtered.is_empty() { None } else { Some(filtered) };
                                    }
                                    ChunkEdits::Full(full) => {
                                        storage = full;
                                        // Full replacement: no individual edit tracking.
                                    }
                                }
                                loaded_tags = Some(tags);
                                loaded_seam_finalized = seam_finalized;
                            }
                            Ok(None) => {}
                            Err(e) => log::warn!("Streaming: load record for {pos:?} failed: {e}"),
                        }
                    }
                },
            );

            let mut chunk = crate::world::chunk::LoadedChunk::new(pos, std::sync::Arc::new(storage));
            chunk.seam_finalized = loaded_seam_finalized;
            chunk.data.overrides = overrides;
            chunk.set_generated_layers(detail_layers, scatter, fluids, smoothing_distances);
            // Overlay persisted player-poured water on top of the generated
            // fluid, as settled (the active set is runtime-only and re-derives).
            apply_persisted_fluid(&mut chunk.data.fluids, &chunk.data.overrides);
            chunk.data.tags = loaded_tags.unwrap_or(generated_tags);
            chunk.persist_dirty = false;

            let _ = tx.send(GenResult { chunk });
        });
    }

    /// Main tick: load/unload chunks based on camera position and frustum.
    ///
    /// Returns the list of newly inserted and unloaded chunk positions so the
    /// caller can do incremental work (e.g. foliage per-chunk).
    pub fn tick(
        &mut self,
        world: &mut World,
        meshing: &mut MeshingCoordinator,
        camera_view: &CameraView,
        _dt: f32,
        on_unload: &mut dyn FnMut(&crate::world::chunk::LoadedChunk),
    ) -> StreamingTickResult {
        let min_y = self.params.min_chunk_y;
        let max_y = self.params.max_chunk_y;
        let load_margin = self.params.load_margin;
        let unload_margin = self.params.unload_margin;

        let cam_cx = camera_view.camera_chunk.x;
        let cam_cz = camera_view.camera_chunk.z;

        let mut result = StreamingTickResult {
            inserted: Vec::new(),
            unloaded: Vec::new(),
        };

        // --- Poll completed chunk generations (capped per frame) ---
        //
        // Without a cap, burst chunk completions can insert dozens of chunks in
        // a single frame, each marking 6 neighbors dirty. This overwhelms the
        // meshing pipeline and causes main-thread stalls during snapshot
        // creation. Cap insertions to spread the load across frames; undrained
        // results stay in the channel and their positions stay in `in_flight`,
        // so they are neither re-queued nor lost.
        let max_insertions = self.params.max_gen_per_frame as usize;
        let meshing_backlog = meshing.pipeline.pending_count();
        // Reduce insertion rate when meshing is already backlogged
        let effective_cap = if meshing_backlog > 128 {
            (max_insertions / 4).max(2)
        } else if meshing_backlog > 64 {
            (max_insertions / 2).max(4)
        } else {
            max_insertions
        };
        {
            let rx = self.gen_result_rx.get_mut().unwrap();
            let mut inserted_count = 0usize;
            while let Ok(gen_result) = rx.try_recv() {
                let pos: IVec3 = gen_result.chunk.data.coord.into();
                self.in_flight.remove(&pos);

                // Discard if chunk is now outside the view rect (camera moved)
                if !camera_view.is_in_view_rect(pos.x, pos.z, unload_margin) {
                    continue;
                }

                // Insert + face-neighbor mesh-dirty flow through the mutation API.
                world
                    .execute(MutationCommand::system(WorldMutation::InsertLoadedChunk {
                        chunk: gen_result.chunk,
                    }))
                    .expect("system chunk insert accepted in any mode");
                result.inserted.push(pos);
                inserted_count += 1;

                // Mark the 6 face-adjacent neighbors dirty so their boundary
                // faces re-cull against the newly loaded chunk.
                for offset in [IVec3::new( 1, 0, 0), IVec3::new(-1, 0, 0),
                               IVec3::new( 0, 1, 0), IVec3::new( 0,-1, 0),
                               IVec3::new( 0, 0, 1), IVec3::new( 0, 0,-1)] {
                    if let Some(neighbor) = world.get_chunk_mut(pos + offset) {
                        neighbor.mark_mesh_dirty();
                    }
                }

                // Stop draining channel when cap is hit. Remaining results stay in
                // the channel and their chunks stay in `in_flight`, so they are
                // not re-queued. Next frame picks up where we left off.
                if inserted_count >= effective_cap {
                    break;
                }
            }
        }

        if !result.inserted.is_empty() {
            meshing.pipeline.submit_all_dirty(world);
        }

        // --- Spawn new generation tasks, nearest-first, up to the budget ---
        //
        // The priority queue is rebuilt every frame on the main thread: it is a
        // pure function of camera position, currently-loaded chunks, and the
        // in-flight set, so there is no shared queue to keep in sync. We pop the
        // nearest candidates and spawn them onto the shared pool until the
        // in-flight budget is full; the rest are reconsidered next frame.
        self.last_camera_chunk = Some(IVec3::new(cam_cx, 0, cam_cz));

        let budget = self.max_in_flight.saturating_sub(self.in_flight.len());
        if budget > 0 {
            let mut queue: BinaryHeap<Reverse<(i32, [i32; 3])>> = BinaryHeap::new();
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
                        if world.get_chunk(pos).is_none()
                            && !self.in_flight.contains(&pos)
                        {
                            let priority = dx * dx + dz * dz;
                            queue.push(Reverse((priority, [cx, cy, cz])));
                        }
                    }
                }
            }

            for _ in 0..budget {
                let Some(Reverse((_, arr))) = queue.pop() else { break };
                self.spawn_generation(IVec3::new(arr[0], arr[1], arr[2]));
            }
        }

        // --- Unload chunks outside the view rectangle ---
        let to_unload: Vec<IVec3> = world
            .chunks
            .keys()
            .filter(|c| !camera_view.is_in_view_rect(c.x, c.z, unload_margin))
            .map(|c| IVec3::from(*c))
            .collect();

        for pos in to_unload {
            if meshing.pipeline.is_chunk_in_flight(pos) {
                continue;
            }
            meshing.pipeline.remove_chunk_state(pos);
            // Save dirty chunk before dropping
            if let Some(chunk) = world.get_chunk(pos) {
                on_unload(chunk);
            }
            world.remove_chunk(pos);
            result.unloaded.push(pos);
        }

        result
    }

    pub fn pending_gen_count(&self) -> usize {
        self.in_flight.len()
    }
}
