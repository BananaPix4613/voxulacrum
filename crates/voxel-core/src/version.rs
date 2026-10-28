//! Chunk data versions.

/// A chunk's data version. Every change to a chunk's data stamps the chunk with
/// a fresh version from one counter the world owns, so a version is never
/// reused, even by a chunk that unloads and loads again. Anything built from a
/// chunk records the version it was built from, and is stale once the chunk
/// moves past it.
/// 
/// Runtime only: versions are not saved, and each session counts from zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkVersion(pub u64);
