use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy_ecs::prelude::Resource;
use glam::{IVec3, Vec3};

use crate::jobs::{JobKind, JobSystem, Priority};
use crate::meshing::coordinator::MeshingCoordinator;
use crate::params::StreamingParams;
use crate::world::chunk::CHUNK_WORLD_SIZE;
use crate::world::persistence::WorldDatabase;
use crate::world::world_generator::WorldGenerator;
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

/// Camera state needed for frustum-based chunk loading.
pub struct CameraView {
    pub zoom: f32,
    pub aspect: f32,
    pub rotation: f32,
    pub camera_chunk: IVec3,
    /// Camera target in world space. Its **altitude** is the reference the
    /// view test measures chunk height against - this is the "view math" the
    /// field was retained for.
    pub camera_world_pos: Vec3,
    /// World-Y span the streamed set covers, as `(low, high)`. A chunk's
    /// altitude shifts where it lands on screen, so the view test cannot decide
    /// visibility from a footprint alone.
    pub world_y_range: (f32, f32),
}

impl CameraView {
    /// Ground-equivalent offset, in chunks, along the screen-up axis produced by
    /// a point at altitude `world_y`.
    ///
    /// Under the isometric projection, moving `d` along the ground's away-axis
    /// shifts a point up-screen by `d·sin(pitch)`, while raising it by `Δy`
    /// shifts it up-screen by `Δy·cos(pitch)`. An altitude is therefore
    /// screen-equivalent to `Δy/tan(pitch)` = `Δy·√2` of ground. Altitude has no
    /// screen-X component - the camera's right vector is horizontal — which is
    /// why only the up axis needs correcting.
    fn altitude_shift_chunks(&self, world_y: f32) -> f32 {
        // 1/tan(atan(1/√2)) = √2
        const INV_TAN_PITCH: f32 = std::f32::consts::SQRT_2;
        (world_y - self.camera_world_pos.y) * INV_TAN_PITCH / CHUNK_WORLD_SIZE
    }

    /// Test if a chunk XZ position falls inside the camera's screen-rectangle
    /// projected onto the ground plane (a rotated parallelogram in world space).
    ///
    /// For rotation `r` and isometric pitch θ = atan(1/√2):
    ///   right_dir = (sin(r), -cos(r))   — screen X axis on ground
    ///   up_dir    = (-cos(r), -sin(r))   — screen Y axis on ground (foreshortened)
    ///
    /// Half-extents (in chunk space), with proportional margin:
    ///   half_right = zoom * aspect  / CHUNK_WORLD_SIZE * (1 + margin_fraction) + MIN_MARGIN
    ///   half_up    = zoom / sin(θ)  / CHUNK_WORLD_SIZE * (1 + margin_fraction) + MIN_MARGIN
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

        if local_right.abs() > half_right {
            return false;
        }

        // Altitude shifts a chunk up-screen, so a column whose *footprint* sits
        // outside the band can still have raised geometry on screen - tall
        // terrain at the near edge, which previously failed to load at all.
        // Test the column's whole projected span against the band.
        //
        // This is column-granular by necessity: a per-chunk Y test would be
        // tighter, but skipping a middle layer would leave the layer above it
        // permanently unmeshable, since `has_face_neighbors` requires both Y
        // neighbors resident.
        let span_lo = local_up + self.altitude_shift_chunks(self.world_y_range.0);
        let span_hi = local_up + self.altitude_shift_chunks(self.world_y_range.1);
        span_lo <= half_up && span_hi >= -half_up
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

        // `zoom` is the camera's orthographic half-height in world units, so the
        // screen-X ground axis covers `zoom * aspect`. The screen-Y ground axis
        // is foreshortened by the isometric tilt, so filling the same screen
        // height takes MORE ground: `zoom / sin(pitch)`.
        //
        // Both axes previously used `aspect`, which is within 3% of correct at
        // 16:9 (√3 ≈ 1.732 vs 16/9 ≈ 1.778) and wrong elsewhere - under-loading
        // the away axis by 30% at 4:3 (pop-in when moving forward) and
        // over-loading by 26% at 21:9. `Camera::visible_radius` is the
        // authoritative statement of this math in the codebase; this now agrees
        // with it.
        let sin_pitch = (1.0_f32 / 3.0).sqrt(); // sin(atan(1/√2)) = 1/√3
        let half_right_vis = self.zoom * self.aspect / CHUNK_WORLD_SIZE;
        let half_up_vis    = self.zoom / sin_pitch / CHUNK_WORLD_SIZE;

        // Margin from the larger axis, applied equally, so the foreshortened
        // direction gets the same absolute buffer as the horizontal one.
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

        // `is_in_view_rect` now admits columns up to one altitude-shift outside
        // the band, so the scan has to reach that far or it never offers them.
        let lift = self
            .altitude_shift_chunks(self.world_y_range.0)
            .abs()
            .max(self.altitude_shift_chunks(self.world_y_range.1).abs());
        let half_up = half_up + lift;

        let cos_r = self.rotation.cos().abs();
        let sin_r = self.rotation.sin().abs();

        // AABB of the rotated rectangle
        let aabb_half_x = half_right * sin_r + half_up * cos_r;
        let aabb_half_z = half_right * cos_r + half_up * sin_r;
        aabb_half_x.max(aabb_half_z).ceil() as i32
    }
}

/// Phases `tick` reports timings for, in the order it runs them.
pub const PHASE_COUNT: usize = 4;

/// Close one `tick` phase: record elapsed since `mark` into `out[i]`, then reset
/// the mark. Plain `Instant`s rather than the diagnostics timer type, so this
/// module keeps no dependency on diagnostics - the caller names the spans.
fn close_phase(i: usize, mark: &mut Instant, out: &mut [f32; PHASE_COUNT]) {
    let now = Instant::now();
    out[i] = (now - *mark).as_secs_f32() * 1000.0;
    *mark = now;
}

/// Result of a streaming tick - lists of chunk positions that changed.
pub struct StreamingTickResult {
    pub inserted: Vec<IVec3>,
    pub unloaded: Vec<IVec3>,
    /// Per-phase milliseconds, in `tick`'s execution order: drain-and-insert,
    /// `submit_all_dirty`, candidate scan, unload-and-persist. Named by the
    /// `diagnostics::SUB_DRAIN..=SUB_UNLOAD` constants; the order here is the
    /// contract between the two.
    pub phase_ms: [f32; PHASE_COUNT],
}

/// What streaming is doing right now (roadmap §4.4 streaming visualizer).
///
/// `fill_ms` is the §7.3 budget metric: milliseconds from the last view change
/// until every chunk inside the load rect is resident. The budget is 2000 ms at
/// any supported zoom, and until this existed there was no way to tell whether
/// the engine met it.
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamingStats {
    /// Chunks resident in the world.
    pub resident: usize,
    /// Chunks inside the load rect this frame.
    pub wanted: usize,
    /// Wanted but not resident. Zero means the view is fully populated.
    pub missing: usize,
    /// Submitted, result not yet consumed. A subset of `missing`.
    pub in_flight: usize,
    /// Chunks evicted this session - the churn signal.
    pub evicted_total: u64,
    /// Bounding scan radius in chunks; tracks zoom.
    pub load_radius: i32,
    /// Fill time for the most recent settle, in ms.
    pub fill_ms: f32,
    /// Chunks that settle had to load - the peak deficit. Makes `fill_ms`
    /// interpretable, and distinguishes a real measurement from a no-op.
    pub fill_chunks: usize,
    /// Still filling, so `fill_ms` describes the *previous* settle.
    pub filling: bool,
}

#[derive(Resource)]
pub struct ChunkStreamingManager {
    pub params: StreamingParams,
    last_camera_chunk: Option<IVec3>,
    /// Shared chunk-generation pool (also used by the startup fill and regen).
    generator: Arc<WorldGenerator>,
    /// Read-only voxel-edit DB inputs handed to each task's worker handle.
    db_path: Option<PathBuf>,
    dict_bytes: Option<Arc<Vec<u8>>>,
    /// DB generation token for this manager instance (see `NEXT_DB_GENERATION`).
    db_generation: u64,
    /// Positions submitted whose result has not been consumed yet. The scheduler
    /// counts *running* jobs; this tracks *outstanding* ones, which is what
    /// dedups the per-frame candidate scan and bounds the result channel.
    in_flight: HashSet<IVec3>,
    result_tx: mpsc::Sender<GenResult>,
    gen_result_rx: Mutex<mpsc::Receiver<GenResult>>,
    stats: StreamingStats,
    /// When the view last changed; the fill clock runs from here.
    view_changed_at: Instant,
    /// Camera chunk + zoom, to detect a view change.
    last_view: (i32, i32, f32),
    /// Peak deficit observed since the current settle began.
    fill_peak: usize,
}

impl ChunkStreamingManager {
    pub fn new(
        params: &StreamingParams,
        generator: Arc<WorldGenerator>,
        db_path: Option<PathBuf>,
        dict_bytes: Option<Arc<Vec<u8>>>,
    ) -> Self {
        let db_generation = NEXT_DB_GENERATION.fetch_add(1, Ordering::Relaxed);
        let (result_tx, result_rx) = mpsc::channel::<GenResult>();

        log::info!("ChunkStreamingManager: scheduler-backed generation (db gen {db_generation})");

        Self {
            params: params.clone(),
            last_camera_chunk: None,
            generator,
            db_path,
            dict_bytes,
            db_generation,
            in_flight: HashSet::new(),
            result_tx,
            gen_result_rx: Mutex::new(result_rx),
            stats: StreamingStats::default(),
            view_changed_at: Instant::now(),
            // Deliberately not a plausible camera state, so the first tick
            // registers as a view change and starts the fill clock.
            last_view: (i32::MIN, i32::MIN, f32::NAN),
            fill_peak: 0,
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
        *self = Self::new(&self.params, generator, db_path, dict_bytes);
        self.last_camera_chunk = None;
    }

    /// Queue one chunk-generation job with the scheduler. Records `pos` as
    /// outstanding; the job generates base terrain, overlays saved edits via
    /// this worker's cached read-only DB handle, and returns the chunk over the
    /// result channel. If the manager has since been rebuilt the send fails and
    /// the result is dropped.
    ///
    /// `distance` is chunk-space squared distance to the camera — the scheduler
    /// orders by it, so nearest terrain generates first (roadmap §10).
    fn submit_generation(&mut self, jobs: &mut JobSystem, pos: IVec3, distance: u32) {
        self.in_flight.insert(pos);

        let gen = Arc::clone(&self.generator);
        let db_path = self.db_path.clone();
        let dict = self.dict_bytes.clone();
        let generation = self.db_generation;
        let tx = self.result_tx.clone();

        jobs.submit(JobKind::Generate, pos, Priority::streaming(distance), move || {
            let generated = gen.generate_chunk(pos);
            let mut chunk = crate::world::LoadedChunk::from_generated(pos, generated);
            with_worker_db(
                generation,
                &db_path,
                dict.as_deref().map(|v| v.as_slice()),
                |db| {
                    if let Some(db) = db {
                        crate::world::persistence::apply_persisted_record(&mut chunk, db);
                    }
                },
            );
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
        jobs: &mut JobSystem,
        camera_view: &CameraView,
        _dt: f32,
        on_unload: &mut dyn FnMut(&crate::world::chunk::LoadedChunk),
    ) -> StreamingTickResult {
        let min_y = world.min_chunk_y;
        let max_y = world.max_chunk_y;
        let load_margin = self.params.load_margin;
        let unload_margin = self.params.unload_margin;

        let cam_cx = camera_view.camera_chunk.x;
        let cam_cz = camera_view.camera_chunk.z;

        let mut result = StreamingTickResult {
            inserted: Vec::new(),
            unloaded: Vec::new(),
            phase_ms: [0.0; PHASE_COUNT],
        };

        let mut phase_ms = [0.0f32; PHASE_COUNT];
        let mut mark = Instant::now();

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

                // Face-neighbor mesh-dirty marking is the `InsertLoadedChunk`
                // handler's job, and it already happened above; it used to be
                // repeated here, marking every neighbor twice per insert.

                // Stop draining channel when cap is hit. Remaining results stay in
                // the channel and their chunks stay in `in_flight`, so they are
                // not re-queued. Next frame picks up where we left off.
                if inserted_count >= effective_cap {
                    break;
                }
            }
        }
        close_phase(0, &mut mark, &mut phase_ms);

        if !result.inserted.is_empty() {
            meshing.pipeline.submit_all_dirty(world);
        }
        close_phase(1, &mut mark, &mut phase_ms);

        // --- Spawn new generation tasks, nearest-first, up to the budget ---
        //
        // The priority queue is rebuilt every frame on the main thread: it is a
        // pure function of camera position, currently-loaded chunks, and the
        // in-flight set, so there is no shared queue to keep in sync. We pop the
        // nearest candidates and spawn them onto the shared pool until the
        // in-flight budget is full; the rest are reconsidered next frame.
        self.last_camera_chunk = Some(IVec3::new(cam_cx, 0, cam_cz));

        // The cap now comes from the scheduler, which is its single owner
        // (drift-review 2.2). Bounding by *outstanding* rather than *running*
        // work is deliberate and matches the previous behavior exactly: it also
        // bounds how many completed results can pile up in the channel.
        let budget = jobs
            .submission_limit(JobKind::Generate)
            .saturating_sub(self.in_flight.len());

        // Scan only when there is budget to spend. Making this unconditional to
        // feed the fill metric was a regression: it is O(radius² × cy) on the
        // main thread and it ran on exactly the saturated frames a zoom-out
        // produces, where it previously did not run at all.
        //
        // The metric does not need it. `missing == 0` implies `in_flight == 0`
        // implies full budget, so the zero transition is always observed on a
        // frame that does scan; only the intermediate counts go approximate.
        let radius = camera_view.bounding_radius(load_margin);
        let mut wanted = self.stats.wanted;
        let mut missing = self.in_flight.len();
        if budget > 0 {
            let mut queue: BinaryHeap<Reverse<(i32, [i32; 3])>> = BinaryHeap::new();
            wanted = 0;
            missing = 0;
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let cx = cam_cx + dx;
                    let cz = cam_cz + dz;
                    if !camera_view.is_in_view_rect(cx, cz, load_margin) {
                        continue;
                    }
                    for cy in min_y..max_y {
                        let pos = IVec3::new(cx, cy, cz);
                        wanted += 1;
                        if world.get_chunk(pos).is_some() {
                            continue;
                        }
                        // Not resident yet - in-flight ones count as missing too.
                        // so `missing == 0` really does mean fully populated.
                        missing += 1;
                        if !self.in_flight.contains(&pos) {
                            let priority = dx * dx + dz * dz;
                            queue.push(Reverse((priority, [cx, cy, cz])));
                        }
                    }
                }
            }

            for _ in 0..budget {
                let Some(Reverse((distance, arr))) = queue.pop() else { break };
                self.submit_generation(
                    jobs,
                    IVec3::new(arr[0], arr[1], arr[2]),
                    distance.max(0) as u32,
                );
            }
        }
        close_phase(2, &mut mark, &mut phase_ms);

        // --- Unload chunks outside the view rectangle ---
        let to_unload: Vec<IVec3> = world
        .chunks
        .keys()
            .filter(|c| !camera_view.is_in_view_rect(c.x, c.z, unload_margin))
            .map(|c| IVec3::from(*c))
            .collect();

        // Bounded by time (see `StreamingParams::unload_budget_ms`). `mark` is
        // the start of this phase - `close_phase(2, ...)` reset it immediately
        // above - so it doubles as the budget clock without a second `Instant`,
        // and the budget correctly covers the resident scan as well as the
        // per-chunk saves.
        let unload_budget = std::time::Duration::from_secs_f32(
            self.params.unload_budget_ms.max(0.0) / 1000.0,
        );
        let mut evicted = 0usize;
        for pos in to_unload {
            // One eviction always happens, so a chunk costing more than the whole
            // budget cannot stall eviction indefinitely.
            if evicted > 0 && mark.elapsed() >= unload_budget {
                break;
            }
            if meshing.pipeline.is_chunk_in_flight(pos) {
                continue;
            }
            meshing.pipeline.remove_chunk_state(pos);
            // Save before dropping: persistence lives outside `World`, so the
            // save reads the chunk here and the door performs the residency
            // change itself.
            if let Some(chunk) = world.get_chunk(pos) {
                on_unload(chunk);
            }
            world
                .execute(MutationCommand::system(WorldMutation::EvictChunk { chunk: pos }))
                .expect("system chunk evict accepted in any mode");
            result.unloaded.push(pos);
            evicted += 1;
        }
        close_phase(3, &mut mark, &mut phase_ms);

        // --- Fill-time metric (roadmap §7.3) ---
        // The clock restarts whenever the view changes and stops the first frame
        // the load rect is fully populated, so `fill_ms` answers exactly the
        // budget's question: how long after the camera settles until the visible
        // area is complete.
        let view = (cam_cx, cam_cz, camera_view.zoom);
        if view != self.last_view {
            self.last_view = view;
            self.view_changed_at = Instant::now();
            self.stats.filling = true;
            self.fill_peak = 0;
        }
        if self.stats.filling {
            self.fill_peak = self.fill_peak.max(missing);
            if missing == 0 {
                if self.fill_peak > 0 {
                    self.stats.fill_ms =
                        self.view_changed_at.elapsed().as_secs_f32() * 1000.0;
                    self.stats.fill_chunks = self.fill_peak;
                }
                // A no-op settle leaves the previous measurement standing rather
                // than overwriting it with a meaningless near-zero.
                self.stats.filling = false;
            }
        }

        self.stats.resident = world.chunks.len();
        self.stats.wanted = wanted;
        self.stats.missing = missing;
        self.stats.in_flight = self.in_flight.len();
        self.stats.evicted_total += result.unloaded.len() as u64;
        self.stats.load_radius = radius;

        result.phase_ms = phase_ms;
        result
    }

    /// Current streaming load, for the visualizer.
    pub fn stats(&self) -> StreamingStats {
        self.stats
    }
}
