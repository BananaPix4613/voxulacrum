//! Shared CPU worker-budget arithmetic (design doc §12: one rayon-backed worker-
//! pool model). Centralizes the split so pool sizing (`main`), streaming's
//! concurrent-generation cap, and meshing's concurrent-submission cap can never
//! silently disagree (drift-review 2.2).

/// The worker-thread budget derived from the detected CPU count.
#[derive(Clone, Copy, Debug)]
pub struct CoreBudget {
    /// Size of the shared generation + meshing rayon pool: cores minus a small
    /// reserve for the main thread and OS.
    pub pool_threads: usize,
    /// Cap on concurrent in-flight chunk *generations* (streaming's share).
    pub gen_in_flight: usize,
    /// Cap on concurrent in-flight chunk *meshings* (meshing's share). With
    /// `gen_in_flight` this sums to `pool_threads`, so the two subsystems' in-flight
    /// work does not oversubscribe the shared pool.
    pub mesh_in_flight: usize,
}

impl CoreBudget {
    /// Compute the budget from the detected CPU count - the single source of truth
    /// every consumer consults. Change the split here and it changes everywhere.
    pub fn detect() -> Self {
        let usable = num_cpus::get().saturating_sub(2).max(2);
        let gen_in_flight = (usable / 3).max(2);
        let mesh_in_flight = usable.saturating_sub(gen_in_flight).max(2);
        Self { pool_threads: usable, gen_in_flight, mesh_in_flight }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_reserves_cores_and_floors_at_two() {
        let b = CoreBudget::detect();
        assert!(b.pool_threads >= 2);
        assert!(b.gen_in_flight >= 2);
        assert!(b.mesh_in_flight >= 2);
    }
}
