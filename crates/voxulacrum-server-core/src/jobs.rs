//! The unified job system (pillar P14; design §1 "one scheduler, one clock
//! family").
//!
//! All deferred background work runs through one scheduler that owns *when* and
//! *how many*. It deliberately does not own *what comes back*: each consumer
//! keeps its own result channel, so adding a consumer never widens this module's
//! knowledge of payload types. A scheduler that knew every consumer's result
//! type would be the god object the boundary modules are structured to avoid.
//!
//! **What this is not, yet.** Declared dependencies are not built here: at this
//! version generation is the only consumer and no job waits on another. The one
//! real edge - a chunk may not mesh until it and its six face neighbors are
//! generated - becomes expressible when meshing joins, and is where dependency
//! declaration lands. Today that edge is a per-frame poll in
//! `drain_pending_submissions`; turning it into a declared dependency is a
//! measurable improvement rather than a speculative abstraction.
//!
//! **Concurrency is one global budget, not a per-kind partition.** Every kind
//! draws from the same running limit and dispatch is purely by priority, so idle
//! mesh capacity flows to generation instead of sitting reserved. The former
//! static split (`usable/3` to generation) capped generation at roughly a
//! quarter of the pool regardless of whether anything was meshing - the
//! mechanism behind "edges of the screen don't stay filled at some zoom levels".
//!
//! Per-kind `submission_limit`s remain, but they are *backpressure*, not
//! concurrency: they bound how much outstanding work a consumer may hold so its
//! result channel cannot grow without limit.

use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy_ecs::prelude::Resource;
use glam::IVec3;

use crate::core_budget::CoreBudget;

/// The kinds of deferred work the scheduler arbitrates between.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobKind {
    Generate,
    Mesh,
    ChunkIo,
}

impl JobKind {
    pub const COUNT: usize = 3;
    /// Every kind, so the inspector and the stats loop stay exhaustive when a
    /// kind is added.
    pub const ALL: [JobKind; JobKind::COUNT] =
        [JobKind::Generate, JobKind::Mesh, JobKind::ChunkIo];

    pub fn index(self) -> usize {
        match self {
            JobKind::Generate => 0,
            JobKind::Mesh => 1,
            JobKind::ChunkIo => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            JobKind::Generate => "generate",
            JobKind::Mesh => "mesh",
            JobKind::ChunkIo => "chunk-io",
        }
    }
}

/// Scheduling priority; **lower runs first**.
///
/// `class` dominates `distance` (field order is the derived `Ord`), so an
/// interactive job can outrank bulk streaming regardless of where the camera is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority {
    pub class: u8,
    /// Chunk-space squared distance to the observed region - roadmap §10's
    /// "priority derives from distance to the observed region".
    pub distance: u32,
}

impl Priority {
    /// Work the player is waiting on directly - currently the re-mesh after an
    /// edit. Outranks every streaming job regardless of distance, which is what
    /// stops "I placed a block and nothing happened until the camera moved".
    pub const CLASS_INTERACTIVE: u8 = 0;
    /// Bulk streaming work: generation and frontier meshing.
    pub const CLASS_STREAMING: u8 = 1;

    pub fn interactive(distance: u32) -> Self {
        Self { class: Self::CLASS_INTERACTIVE, distance }
    }

    pub fn streaming(distance: u32) -> Self {
        Self { class: Self::CLASS_STREAMING, distance }
    }
}

struct PendingJob {
    priority: Priority,
    /// Submission order, for FIFO within equal priority.
    seq: u64,
    kind: JobKind,
    // Not read today: the panel shows per-kind aggregates, not individual jobs.
    // Retained because it is the identity any per-chunk ordering rule would key
    // on - see the chunk-I/O trigger in `pre-phase-10-audit.md` §6.3.
    #[allow(dead_code)]
    key: IVec3,
    run: Box<dyn FnOnce() + Send>,
}

impl Ord for PendingJob {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reversed: `BinaryHeap` is a max-heap and we want the lowest priority
        // popped first, oldest-first among equals.
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
impl PartialOrd for PendingJob {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for PendingJob {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.seq == other.seq
    }
}
impl Eq for PendingJob {}

/// Per-kind counters, shared with running jobs so a task can record its own
/// completion without routing through the main thread.
struct JobCounters {
    in_flight: [AtomicUsize; JobKind::COUNT],
    completed: [AtomicU64; JobKind::COUNT],
    total_us: [AtomicU64; JobKind::COUNT],
    max_us: [AtomicU64; JobKind::COUNT],
}

impl JobCounters {
    fn new() -> Self {
        Self {
            in_flight: std::array::from_fn(|_| AtomicUsize::new(0)),
            completed: std::array::from_fn(|_| AtomicU64::new(0)),
            total_us: std::array::from_fn(|_| AtomicU64::new(0)),
            max_us: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

/// A snapshot of one kind's load, for the Performance panel and the job
/// inspector (roadmap §4.4).
#[derive(Clone, Copy, Debug, Default)]
pub struct JobKindStats {
    /// Submitted, not yet dispatched.
    pub queued: usize,
    /// Dispatched and executing right now.
    pub in_flight: usize,
    /// This kind's concurrency cap, so the panel can show saturation rather
    /// than a bare count.
    pub cap: usize,
    pub completed: u64,
    pub mean_ms: f32,
    pub max_ms: f32,
}

/// The single scheduler for deferred background work.
#[derive(Resource)]
pub struct JobSystem {
    pool: Arc<rayon::ThreadPool>,
    pending: Mutex<BinaryHeap<PendingJob>>,
    counters: Arc<JobCounters>,
    /// Per-kind backpressure: how much outstanding work a consumer may hold.
    submission_limits: [usize; JobKind::COUNT],
    /// Global concurrency: jobs of any kind executing at once.
    max_running: usize,
    next_seq: u64,
}

impl JobSystem {
    pub fn new(pool: Arc<rayon::ThreadPool>) -> Self {
        let max_running = CoreBudget::detect().pool_threads;
        // Backpressure, not concurrency: a consumer may keep up to a full pool's
        // worth of work outstanding, and the global limit decides how much of it
        // runs. Bounding at the pool size keeps result channels from growing
        // without limit while still letting one kind use the whole pool.
        let submission_limits = [max_running; JobKind::COUNT];

        log::info!(
            "JobSystem: {} pool threads, max running {max_running}, \
             submission limit {max_running} per kind",
            pool.current_num_threads(),
        );

        Self {
            pool,
            pending: Mutex::new(BinaryHeap::new()),
            counters: Arc::new(JobCounters::new()),
            submission_limits,
            max_running,
            next_seq: 0,
        }
    }

    /// How much outstanding work this kind's consumer may hold. Backpressure,
    /// not concurrency - see [`Self::max_running`]. The single source of truth;
    /// consumers read it rather than deriving their own (drift-review 2.2).
    pub fn submission_limit(&self, kind: JobKind) -> usize {
        self.submission_limits[kind.index()]
    }

    /// Jobs of any kind executing at once.
    pub fn max_running(&self) -> usize {
        self.max_running
    }

    /// The shared worker pool, for work that must run *off* the frame-schedule
    /// thread but is not a queued job - currently only the determinism checker,
    /// which is an explicit operator action that blocks until it finishes.
    ///
    /// Graph evaluation `debug_assert`s it is not on the schedule thread
    /// (`nodegraph_eval::guard`), so anything that generates chunks has to get
    /// onto the pool one way or another.
    pub fn pool(&self) -> &rayon::ThreadPool {
        &self.pool
    }

    /// Jobs currently executing, across all kinds.
    pub fn running_total(&self) -> usize {
        (0..JobKind::COUNT)
            .map(|i| self.counters.in_flight[i].load(Ordering::Relaxed))
            .sum()
    }

    /// Queue a job. It starts on the next [`Self::pump`] if its kind has budget.
    pub fn submit(
        &mut self,
        kind: JobKind,
        key: IVec3,
        priority: Priority,
        run: impl FnOnce() + Send + 'static,
    ) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.pending
            .get_mut()
            .expect("job queue mutex poisoned")
            .push(PendingJob { priority, seq, kind, key, run: Box::new(run) });
    }

    /// Dispatch the highest-priority pending jobs onto the shared pool, up to
    /// each kind's cap. Runs once per frame.
    pub fn pump(&mut self) {
        let max_running = self.max_running();
        let pool = Arc::clone(&self.pool);
        let counters = Arc::clone(&self.counters);
        let pending = self.pending.get_mut().expect("job queue mutex poisoned");

        let mut deferred: Vec<PendingJob> = Vec::new();

        while let Some(job) = pending.pop() {
            let running: usize = (0..JobKind::COUNT)
                .map(|i| counters.in_flight[i].load(Ordering::Relaxed))
                .sum();
            if running >= max_running {
                // The pool is full, so no job of any kind can start. Everything
                // left keeps its place in the queue for the next frame.
                deferred.push(job);
                break;
            }

            let idx = job.kind.index();
            counters.in_flight[idx].fetch_add(1, Ordering::Relaxed);
            let counters = Arc::clone(&counters);
            let run = job.run;
            pool.spawn(move || {
                let start = Instant::now();
                run();
                let us = start.elapsed().as_micros() as u64;
                counters.completed[idx].fetch_add(1, Ordering::Relaxed);
                counters.total_us[idx].fetch_add(us, Ordering::Relaxed);
                counters.max_us[idx].fetch_max(us, Ordering::Relaxed);
                // Decremented last: a consumer polling `in_flight` must never see
                // a slot free before its timing has been recorded.
                counters.in_flight[idx].fetch_sub(1, Ordering::Relaxed);
            });
        }

        for job in deferred {
            pending.push(job);
        }
    }

    /// Load snapshot for one kind.
    ///
    /// `in_flight` here counts what is *executing*; it is deliberately not the
    /// same as the outstanding-work counts consumers keep for backpressure
    /// (streaming's `in_flight` set, meshing's `chunk_states`), which include
    /// finished jobs whose results have not been consumed yet.
    pub fn stats(&self, kind: JobKind) -> JobKindStats {
        let idx = kind.index();
        let completed = self.counters.completed[idx].load(Ordering::Relaxed);
        let total_us = self.counters.total_us[idx].load(Ordering::Relaxed);
        JobKindStats {
            queued: self
                .pending
                .lock()
                .expect("job queue mutex poisoned")
                .iter()
                .filter(|j| j.kind == kind)
                .count(),
            in_flight: self.counters.in_flight[idx].load(Ordering::Relaxed),
            cap: self.submission_limits[idx],
            completed,
            mean_ms: if completed > 0 {
                total_us as f32 / completed as f32 / 1000.0
            } else {
                0.0
            },
            max_ms: self.counters.max_us[idx].load(Ordering::Relaxed) as f32 / 1000.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_orders_by_class_then_distance() {
        let near_bulk = Priority::streaming(4);
        let far_bulk = Priority::streaming(400);
        let interactive = Priority { class: 0, distance: u32::MAX };
        assert!(near_bulk < far_bulk, "nearer streaming work runs first");
        assert!(
            interactive < near_bulk,
            "class dominates distance, so an interactive job outranks any bulk job",
        );
    }

    #[test]
    fn pending_pops_lowest_priority_first_then_fifo() {
        let mut heap: BinaryHeap<PendingJob> = BinaryHeap::new();
        let mk = |seq: u64, distance: u32| PendingJob {
            priority: Priority::streaming(distance),
            seq,
            kind: JobKind::Generate,
            key: IVec3::ZERO,
            run: Box::new(|| {}),
        };
        heap.push(mk(0, 100));
        heap.push(mk(1, 4));
        heap.push(mk(2, 4));
        assert_eq!(heap.pop().unwrap().seq, 1, "nearest first");
        assert_eq!(heap.pop().unwrap().seq, 2, "FIFO among equal priority");
        assert_eq!(heap.pop().unwrap().seq, 0, "farthest last");
    }
}
