//! Error type for hot-reload operations.

use thiserror::Error;

/// Errors raised by [`crate::GraphWatcher`] setup.
#[derive(Debug, Error)]
pub enum HotReloadError {
    /// Underlying `notify` failure.
    #[error("notify: {0}")]
    Notify(#[from] notify::Error),

    /// I/O failure (e.g. watched dir does not exist).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias for hot-reload operations.
pub type HotReloadResult<T> = Result<T, HotReloadError>;
