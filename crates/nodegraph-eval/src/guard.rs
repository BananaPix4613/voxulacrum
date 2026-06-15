//! Frame-schedule thread guard.
//!
//! The engine runs its bevy_ecs frame schedule on a single thread. Graph
//! evaluation must never happen *inline* on that thread - it belongs on the
//! chunk-generation worker pool. The engine wraps its `schedule.run(...)` call
//! in [`enter_schedule_thread`]; [`Evaluator::evaluate`](crate::Evaluator::evaluate)
//! debug_asserts it is not running under that guard.
//!
//! The startup world fill runs on the main thread but *outside* the schedule,
//! so it is intentionally allowed.

use std::cell::Cell;

thread_local! {
    static ON_SCHEDULE_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// RAII guard marking the current thread as actively running the frame
/// schedule. Dropped at the end of the schedule run, clearing the flag.
#[must_use = "the guard must be held for the duration of the schedule run"]
pub struct ScheduleThreadGuard {
    _private: (),
}

impl Drop for ScheduleThreadGuard {
    fn drop(&mut self) {
        ON_SCHEDULE_THREAD.with(|f| f.set(false));
    }
}

/// Mark the current thread as running the frame schedule until the returned
/// guard is dropped.
pub fn enter_schedule_thread() -> ScheduleThreadGuard {
    ON_SCHEDULE_THREAD.with(|f| f.set(true));
    ScheduleThreadGuard { _private: () }
}

/// Whether the current thread is executing inside the frame schedule.
pub fn on_schedule_thread() -> bool {
    ON_SCHEDULE_THREAD.with(|f| f.get())
}
