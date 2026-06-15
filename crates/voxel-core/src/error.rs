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

    /// Failed to read a material RON file from disk.
    #[error("failed to read material RON file {path}: {source}")]
    MaterialRonRead {
        /// Path that could not be read.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A material RON document could not be parsed.
    #[error("failed to parse material RON: {0}")]
    MaterialRonParse(String),

    /// A material RON document parsed but failed semantic validation
    /// (duplicate id, duplicate id_name, or non-contiguous id range from 0).
    #[error("invalid material registry: {0}")]
    MaterialRonValidation(String),
}

/// Convenience alias for results in this crate.
pub type VoxelCoreResult<T> = std::result::Result<T, VoxelCoreError>;
