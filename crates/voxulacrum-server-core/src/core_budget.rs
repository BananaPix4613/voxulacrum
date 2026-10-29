//! Shared CPU worker-budget arithmetic
//! 
//! This originally centralized a *split* between generation and meshing so the
//! two could never silently disagree (drift-review 2.2, fixed in 0.2.0). The
//! split itself is now gone: the job system (P14) owns concurrency globally and
//! dispatches by priority, so there is nothing to partition and no pair of
//! numbers that must sum correctly. Non-oversubscription is enforced directly by
//! the scheduler's single running limit.
//! 
//! What remains is the one number every consumer needs: how many worker threads
//! the machine should give the engine.

/// The worker-thread budget derived from the detected CPU count.
#[derive(Clone, Copy, Debug)]
pub struct CoreBudget {
    /// Size of the shared worker pool, and the scheduler's global concurrency
    /// limit: cores minus a small reserve for the main thread and the OS.
    pub pool_threads: usize,
}

impl CoreBudget {
    /// The single source of truth every consumer consults.
    pub fn detect() -> Self {
        Self { pool_threads: num_cpus::get().saturating_sub(2).max(2) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_reserves_cores_and_floors_at_two() {
        assert!(CoreBudget::detect().pool_threads >= 2);
    }
}
