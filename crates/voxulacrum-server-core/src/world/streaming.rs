use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use voxel_core::AreaOfInterest;

use crate::jobs::{JobKind, JobSystem, Priority};
use crate::world::config::StreamingConfig;
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

/// An observer's id (design §12, "Residency").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObserverId(pub u32);

impl ObserverId {
    /// The local player.
    pub const LOCAL: Self = Self(0);
}

/// Every observer, with the area it needs resident. Residency is computed from
/// these alone: the resident set is the union of their areas plus every chunk
/// with active fluid.
///
/// Each observer's camera or controller supplies its area. The local client
/// writes observer 0's area directly for now; R5 makes that a `SetInterest`
/// command.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Observers {
    areas: BTreeMap<ObserverId, AreaOfInterest>,
}

impl Observers {
    /// Set `id`'s area, adding the observer if it is new.
    pub fn set(&mut self, id: ObserverId, area: AreaOfInterest) {
        self.areas.insert(id, area);
    }

    /// Remove an observer. Its chunks unload unless another observer keeps them.
    #[allow(dead_code)] // the headless server's observers come and go (R3c); tests use it now
    pub fn remove(&mut self, id: ObserverId) {
        self.areas.remove(&id);
    }

    /// `id`'s area, if it is an observer.
    pub fn get(&self, id: ObserverId) -> Option<&AreaOfInterest> {
        self.areas.get(&id)
    }

    /// Every observer's area, in id order.
    pub fn areas(&self) -> impl Iterator<Item = &AreaOfInterest> + '_ {
        self.areas.values()
    }

    /// Whether some observer's area keeps the chunk at `pos` loaded.
    pub fn retains(&self, pos: IVec3) -> bool {
        self.areas().any(|area| area.retains(pos))
    }

    /// Squared horizontal distance, in chunks, from the column of `pos` to the
    /// nearest observer's center column. `None` with no observers.
    pub fn distance_sq(&self, pos: IVec3) -> Option<i32> {
        self.areas().map(|area| area.distance_sq(pos)).min()
    }
}

/// Phases `tick` reports timings for, in the order it runs them.
pub const PHASE_COUNT: usize = 3;

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
    /// candidate scan, unload-and-persist. Named by the
    /// `diagnostics::SUB_DRAIN..=SUB_UNLOAD` constants; the order here is the
    /// contract between the two.
    pub phase_ms: [f32; PHASE_COUNT],
}

/// What residency and seam finalization cost in the ticks since the frame last
/// took the figures, in milliseconds, for the Performance panel's streaming
/// sub-spans. Measurement only: nothing in the simulation reads it.
#[derive(Resource, Default)]
pub struct ResidencyTimings {
    /// The whole residency system.
    pub stream_ms: f32,
    /// `tick`'s phases, in `StreamingTickResult::phase_ms` order.
    pub phase_ms: [f32; PHASE_COUNT],
    /// Seam finalization.
    pub seam_ms: f32,
}

/// What streaming is doing right now (roadmap §4.4 streaming visualizer).
///
/// `fill_ms` is the §7.3 budget metric: milliseconds from the last change of an
/// observer's area until every chunk the areas load is resident. The budget is
///2000 ms at any supported zoom, and until this existed there was no way to tell
/// whether the engine met it.
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamingStats {
    /// Chunks resident in the world.
    pub resident: usize,
    /// Chunks inside some observer's load area as of the last tick.
    pub wanted: usize,
    /// Wanted but not resident. Zero means the view is fully populated.
    pub missing: usize,
    /// Submitted, result not yet consumed. A subset of `missing`.
    pub in_flight: usize,
    /// Chunks evicted this session - the churn signal.
    pub evicted_total: u64,
    /// The widest observer's scan reach in chunks; tracks zoom.
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
    pub params: StreamingConfig,
    /// Shared chunk-generation pool (also used by the startup fill and regen).
    generator: Arc<WorldGenerator>,
    /// Read-only voxel-edit DB inputs handed to each task's worker handle.
    db_path: Option<PathBuf>,
    dict_bytes: Option<Arc<Vec<u8>>>,
    /// DB generation token for this manager instance (see `NEXT_DB_GENERATION`).
    db_generation: u64,
    /// Positions submitted whose result has not been consumed yet. The scheduler
    /// counts *running* jobs; this tracks *outstanding* ones, which is what
    /// dedups the per-tick candidate scan and bounds the result channel.
    in_flight: HashSet<IVec3>,
    result_tx: mpsc::Sender<GenResult>,
    gen_result_rx: Mutex<mpsc::Receiver<GenResult>>,
    stats: StreamingStats,
    /// When an observer's area last changed; the fill clock runs from here.
    view_changed_at: Instant,
    /// The observers as the last tick saw them, to detect a change of area.
    last_view: Option<Observers>,
    /// Peak deficit observed since the current settle began.
    fill_peak: usize,
}

impl ChunkStreamingManager {
    pub fn new(
        params: &StreamingConfig,
        generator: Arc<WorldGenerator>,
        db_path: Option<PathBuf>,
        dict_bytes: Option<Arc<Vec<u8>>>,
    ) -> Self {
        let db_generation = NEXT_DB_GENERATION.fetch_add(1, Ordering::Relaxed);
        let (result_tx, result_rx) = mpsc::channel::<GenResult>();

        log::info!("ChunkStreamingManager: scheduler-backed generation (db gen {db_generation})");

        Self {
            params: params.clone(),
            generator,
            db_path,
            dict_bytes,
            db_generation,
            in_flight: HashSet::new(),
            result_tx,
            gen_result_rx: Mutex::new(result_rx),
            stats: StreamingStats::default(),
            view_changed_at: Instant::now(),
            // None, so the first tick registers as a change and starts the fill
            // clock.
            last_view: None,
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
    }

    /// Queue one chunk-generation job with the scheduler. Records `pos` as
    /// outstanding; the job generates base terrain, overlays saved edits via
    /// this worker's cached read-only DB handle, and returns the chunk over the
    /// result channel. If the manager has since been rebuilt the send fails and
    /// the result is dropped.
    ///
    /// `distance` is chunk-space squared distance to the nearest observer - the
    /// scheduler orders by it, so nearest terrain generates first (roadmap §10).
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

    /// Main tick: load the chunks some observer's area wants, nearest first,
    /// and unload the ones no observer's area keeps and no active fluid holds.
    ///
    /// Returns the list of newly inserted and unloaded chunk positions so the
    /// caller can do incremental work (e.g. foliage per-chunk).
    pub fn tick(
        &mut self,
        world: &mut World,
        jobs: &mut JobSystem,
        observers: &Observers,
        on_unload: &mut dyn FnMut(&crate::world::chunk::LoadedChunk),
    ) -> StreamingTickResult {
        let min_y = world.min_chunk_y;
        let max_y = world.max_chunk_y;

        let mut result = StreamingTickResult {
            inserted: Vec::new(),
            unloaded: Vec::new(),
            phase_ms: [0.0; PHASE_COUNT],
        };

        let mut phase_ms = [0.0f32; PHASE_COUNT];
        let mut mark = Instant::now();

        // --- Poll completed chunk generations (capped per tick) ---
        //
        // Cap insertions to spread a burst of completions across ticks;
        // undrained results stay in the channel and their positions stay in
        // `in_flight`, so they are neither re-queued nor lost. The mesher paces
        // its own snapshots (`max_mesh_per_frame`), so residency does not slow
        // down for its backlog: a server does not wait on one client's mesher.
        let effective_cap = self.params.max_inserts_per_tick as usize;
        {
            let rx = self.gen_result_rx.get_mut().unwrap();
            let mut inserted_count = 0usize;
            while let Ok(gen_result) = rx.try_recv() {
                let pos: IVec3 = gen_result.chunk.data.coord.into();
                self.in_flight.remove(&pos);

                // Discard if no observer keeps it any more (they moved on).
                if !observers.retains(pos) {
                    continue;
                }

                // Insert through the mutation API; the change feed carries it to
                // the mesher.
                world
                    .execute(MutationCommand::system(WorldMutation::InsertLoadedChunk {
                        chunk: gen_result.chunk,
                    }))
                    .expect("system chunk insert accepted in any mode");
                result.inserted.push(pos);
                inserted_count += 1;

                // The client re-meshes the face neighbors from the change the
                // insert publishes.

                // Stop draining channel when cap is hit. Remaining results stay in
                // the channel and their chunks stay in `in_flight`, so they are
                // not re-queued. The next tick picks up where we left off.
                if inserted_count >= effective_cap {
                    break;
                }
            }
        }
        close_phase(0, &mut mark, &mut phase_ms);

        // --- Spawn new generation tasks, nearest-first, up to the budget ---
        //
        // The priority queue is rebuilt every tick on the main thread: it is a
        // pure function of the observers' areas, currently-loaded chunks, and
        // the in-flight set, so there is no shared queue to keep in sync. We pop
        // the nearest candidates and spawn them onto the shared pool until the
        // in-flight budget is full; the rest are reconsidered next tick.

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
        let radius = observers.areas().map(|area| area.load.reach()).max().unwrap_or(0);
        let mut wanted = self.stats.wanted;
        let mut missing = self.in_flight.len();
        if budget > 0 {
            let mut queue: BinaryHeap<Reverse<(i32, [i32; 3])>> = BinaryHeap::new();
            wanted = 0;
            missing = 0;
            // Each observer's columns in turn, in id order. A chunk an earlier
            // observer already wants is counted once, under that observer.
            for (i, area) in observers.areas().enumerate() {
                let reach = area.load.reach();
                let layers = area.chunk_y.0.max(min_y)..area.chunk_y.1.min(max_y);
                for dz in -reach..=reach {
                    for dx in -reach..=reach {
                        if !area.load.contains(dx, dz) {
                            continue;
                        }
                        let cx = area.center.0 + dx;
                        let cz = area.center.1 + dz;
                        for cy in layers.clone() {
                            let pos = IVec3::new(cx, cy, cz);
                            if observers.areas().take(i).any(|earlier| earlier.loads(pos)) {
                                continue;
                            }
                            wanted += 1;
                            if world.get_chunk(pos).is_some() {
                                continue;
                            }
                            // Not resident yet - in-flight ones count as missing
                            // too, so `missing == 0` really does mean fully
                            // populated.
                            missing += 1;
                            if !self.in_flight.contains(&pos) {
                                let priority = observers.distance_sq(pos).unwrap_or(0);
                                queue.push(Reverse((priority, [cx, cy, cz])));
                            }
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
        close_phase(1, &mut mark, &mut phase_ms);

        // --- Unload chunks no observer keeps and no active fluid holds ---
        // The resident set is the observers' areas plus every chunk with active
        // fluid (design §12, "Residency"), so a chunk whose water is still
        // moving stays until it settles.
        let to_unload: Vec<IVec3> = world
            .chunks
            .iter()
            .filter(|(_, chunk)| chunk.data.fluids.active.is_empty())
            .map(|(c, _)| IVec3::from(*c))
            .filter(|&pos| !observers.retains(pos))
            .collect();

        // Bounded by time (see `StreamingConfig::unload_budget_ms`). `mark` is
        // the start of this phase - `close_phase(1, ...)` reset it immediately
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
            // The eviction reaches the client through the change feed, which
            // forgets the chunk's mesh state and drops its render entries.
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
        close_phase(2, &mut mark, &mut phase_ms);

        // --- Fill-time metric (roadmap §7.3) ---
        // The clock restarts whenever an observer's area changes and stops the
        // first tick every area's load footprint is fully populated, so
        // `fill_ms` answers exactly the budget's question: how long after the
        // camera settles until the visible area is complete.
        if self.last_view.as_ref() != Some(observers) {
            self.last_view = Some(observers.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;
    use voxel_core::Footprint;
    use crate::world::world_generator;

    /// One chunk column, one layer tall, as an observer's area.
    fn column(x: i32, z: i32) -> AreaOfInterest {
        AreaOfInterest {
            center: (x, z),
            chunk_y: (0, 1),
            load: Footprint::Disc { radius: 0.0 },
            retain: Footprint::Disc { radius: 0.0 },
        }
    }

    /// One streaming tick, then dispatch what it submitted.
    fn tick(
        streaming: &mut ChunkStreamingManager,
        world: &mut World,
        jobs: &mut JobSystem,
        observers: &Observers,
    ) {
        streaming.tick(world, jobs, observers, &mut |_| {});
        jobs.pump();
    }

    #[test]
    fn two_observers_far_apart_keep_both_areas_loaded() {
        let generator = world_generator::load_default().expect("default generator loads");
        let mut world = World::from_chunks(
            HashMap::new(),
            generator.clone(),
            Arc::new(crate::materials::embedded_registry()),
            0,
            1,
        );
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().expect("pool");
        let mut jobs = JobSystem::new(Arc::new(pool));
        let mut streaming =
            ChunkStreamingManager::new(&StreamingConfig::default(), generator, None, None);
        let (near, far) = (IVec3::new(0, 0, 0), IVec3::new(1000, 0, -1000));
        let mut observers = Observers::default();
        observers.set(ObserverId::LOCAL, column(near.x, near.z));
        observers.set(ObserverId(1), column(far.x, far.z));

        let deadline = Instant::now() + Duration::from_secs(60);
        while world.get_chunk(near).is_none() || world.get_chunk(far).is_none() {
            assert!(Instant::now() < deadline, "both columns generate within a minute");
            tick(&mut streaming, &mut world, &mut jobs, &observers);
            std::thread::sleep(Duration::from_millis(5));
        }

        for _ in 0..100 {
            tick(&mut streaming, &mut world, &mut jobs, &observers);
        }
        assert!(world.get_chunk(near).is_some(), "observer 0's column stays");
        assert!(world.get_chunk(far).is_some(), "observer 1's column stays");
        assert_eq!(world.chunks.len(), 2, "and nothing else loaded");
        assert_eq!(streaming.stats().missing, 0);

        observers.remove(ObserverId(1));
        tick(&mut streaming, &mut world, &mut jobs, &observers);
        assert!(world.get_chunk(far).is_none(), "no observer keeps it");
        assert!(world.get_chunk(near).is_some(), "observer 0 still does");
    }
}
