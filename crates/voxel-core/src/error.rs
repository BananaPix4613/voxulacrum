//! Error types for voxel-core.

use thiserror::Error;

/// Crate-wide error type.
#[derive(Debug, Error)]
pub enum VoxelCoreError {
    /// A coordinate fell outside `[0, N)` on at least one axis.
    #[error("voxel coordinate {coord:?} out of range for chunk edge {edge}")]
    OutOfRange {
        /// The offending coordinate.
        coord: (i32, i32, i32),
        /// The chunk edge length N.
        edge: usize,
    },
    
    /// Dense buffer constructed with the wrong slice length.
    #[error("dense buffer length {got} does not match expected volume {expected}")]
    WrongLength {
        /// Length of the slice passed in.
        got: usize,
        /// Expected `N^3`.
        expected: usize,
    },
    
    /// The packed `u32` Voxel form contained an invalid shape discriminant.
    #[error("invalid shape discriminant {0} in packed Voxel")]
    InvalidShape(u8),

    /// RON-driven material loading is deferred to a later phase (Phase 5
    /// modding); future asset path `assets/materials/*.ron`. Returned by
    /// [`crate::MaterialRegistry::load_from_ron`] so the stub can never
    /// silently fall back to a default registry.
    #[error("RON material loading is deferred to a future phase (Phase 5 modding)")]
    RonLoadingDeferredToFuturePhase,
}

/// Convenience alias for results in this crate.
pub type VoxelCoreResult<T> = std::result::Result<T, VoxelCoreError>;
