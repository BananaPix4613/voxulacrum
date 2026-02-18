use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::ChunkSnapshot;
use crate::world::voxel::MATERIAL_TABLE;

/// Cache file format version. Increment when TerrainVertex layout or
/// serialization format changes to automatically invalidate old caches.
const CACHE_VERSION: u32 = 2;

// ============================================================================
// Cache file format
// ============================================================================

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    key_hash: u64,
    vertex_count: u32,
    index_count: u32,
    vertices: Vec<TerrainVertex>,
    indices: Vec<u32>,
}

// ============================================================================
// Atomic stats (shared across worker threads)
// ============================================================================

pub struct AtomicCacheStats {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub errors: AtomicU64,
}

impl AtomicCacheStats {
    pub fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }

    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
    }
}

/// Snapshot of cache stats for display in the UI.
#[derive(Clone, Default, Debug)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub errors: u64,
    pub files_on_disk: u64,
    pub bytes_on_disk: u64,
}

// ============================================================================
// Cache key computation
// ============================================================================

/// Compute a cache key from snapshot voxel data and material sharpness values.
///
/// Hashes:
/// - `density` (i8) and `material` (u16) for all 34^3 voxels in the snapshot
/// - Sharpness values from MATERIAL_TABLE (affects normal biasing in meshing)
///
/// Uses SeaHash for fast, portable, non-cryptographic hasing.
pub fn compute_cache_key(snapshot: &ChunkSnapshot, sharpness_values: &[f32]) -> u64 {
    use seahash::SeaHasher;
    use std::hash::Hasher;

    let mut hasher = SeaHasher::new();

    // Hash density and material for every voxel in the 34^3 snapshot.
    // These are the only voxel fields that affect mesh output.
    for voxel in snapshot.voxels.iter() {
        hasher.write_i8(voxel.density);
        hasher.write_u16(voxel.material);
    }

    // Hash material sharpness values (affect QEF normal biasing).
    for &s in sharpness_values {
        hasher.write(&s.to_le_bytes());
    }

    hasher.finish()
}

/// Extract sharpness values from the static MATERIAL_TABLE.
pub fn extract_sharpness_values() -> Vec<f32> {
    MATERIAL_TABLE.iter().map(|m| m.sharpness).collect()
}

// ============================================================================
// File path
// ============================================================================

/// Build the cache file path for a chunk.
/// Format: `{cache_dir}/{x}_{y}_{z}_{hash:016x}.bin`
pub fn cache_file_path(
    cache_dir: &Path,
    chunk_x: i32,
    chunk_y: i32,
    chunk_z: i32,
    key: u64,
) -> PathBuf {
    cache_dir.join(format!("{}_{}_{}_{:016x}.bin", chunk_x, chunk_y, chunk_z, key))
}

// ============================================================================
// Load / Save
// ============================================================================

/// Attempt to load a cached mesh from disk.
///
/// Returns `None` on any failure: missing file, version mismatch, key mismatch,
/// corrupted data, or deserialization error. Stale/corrupt files are deleted.
pub fn load_cached_mesh(
    path: &Path,
    expected_key: u64,
) -> Option<(Vec<TerrainVertex>, Vec<u32>)> {
    let data = std::fs::read(path).ok()?;
    let file: CacheFile = bincode::deserialize(&data).ok()?;

    if file.version != CACHE_VERSION {
        let _ = std::fs::remove_file(path);
        return None;
    }
    if file.key_hash != expected_key {
        return None;
    }
    if file.vertex_count as usize != file.vertices.len()
        || file.index_count as usize != file.indices.len()
    {
        let _ = std::fs::remove_file(path);
        return None;
    }

    Some((file.vertices, file.indices))
}

/// Save a mesh to the disk cache. Errors are non-fatal (caller should log and
/// continue).
pub fn save_cached_mesh(
    path: &Path,
    key: u64,
    vertices: &[TerrainVertex],
    indices: &[u32],
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = CacheFile {
        version: CACHE_VERSION,
        key_hash: key,
        vertex_count: vertices.len() as u32,
        index_count: indices.len() as u32,
        vertices: vertices.to_vec(),
        indices: indices.to_vec(),
    };

    let data =
        bincode::serialize(&file).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    std::fs::write(path, data)
}

// ============================================================================
// Cache management
// ============================================================================

/// Delete all `.bin` files in the cache directory. Returns the number deleted.
pub fn clear_cache(cache_dir: &Path) -> io::Result<u64> {
    let mut count = 0u64;
    if cache_dir.exists() {
        for entry in std::fs::read_dir(cache_dir)? {
            let entry = entry?;
            if entry.path().extension().map_or(false, |e| e == "bin") {
                let _ = std::fs::remove_file(entry.path());
                count += 1;
            }
        }
    }
    Ok(count)
}

/// Count files and total byte size of `.bin` files in the cache directory.
pub fn disk_usage(cache_dir: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().map_or(false, |e| e == "bin") {
                files += 1;
                bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (files, bytes)
}

// ============================================================================
// World generation cache
// ============================================================================

/// Compute a cache key for world generation params.
pub fn compute_world_cache_key(params: &crate::params::TerrainGenParams) -> u64 {
    use seahash::SeaHasher;
    use std::hash::Hasher;

    let mut hasher = SeaHasher::new();
    hasher.write(&params.base_height.to_le_bytes());
    hasher.write(&params.cliff_threshold.to_le_bytes());
    hasher.write(&params.hill_amplitude.to_le_bytes());
    hasher.write(&params.hill_frequency.to_le_bytes());
    hasher.write(&params.ridge_amplitude.to_le_bytes());
    hasher.write(&params.ridge_frequency.to_le_bytes());
    hasher.write(&params.detail_amplitude.to_le_bytes());
    hasher.write(&params.detail_frequency.to_le_bytes());
    hasher.write(&params.cave_amplitude.to_le_bytes());
    hasher.write(&params.cave_frequency.to_le_bytes());
    hasher.write_i32(params.seed);
    hasher.finish()
}

/// Path for the world generation cache file.
pub fn world_cache_path(cache_dir: &Path, key: u64) -> PathBuf {
    // Use parent of meshes dir (i.e., `cache/`)
    let parent = cache_dir.parent().unwrap_or(cache_dir);
    parent.join(format!("world_{:016x}.bin", key))
}

/// Save world voxel data to disk using bytemuck for zero-copy serialization.
/// File format: [u64 key][u32 chunks_x][u32 chunks_y][u32 chunks_z][u32 chunk_count]
///              then for each chunk: [i32 pos.x][i32 pos.y][i32 pos.z][voxels...]
pub fn save_world_cache(
    path: &Path,
    key: u64,
    world: &crate::world::World,
) -> io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let chunk_count = world.chunks.len() as u32;
    let voxel_bytes_per_chunk = std::mem::size_of::<crate::world::voxel::Voxel>()
        * crate::world::chunk::CHUNK_VOLUME;

    // Estimate total size for pre-allocation
    let header_size = 8 + 4 + 4 + 4 + 4; // key + dims + count
    let per_chunk = 12 + voxel_bytes_per_chunk; // 3 x i32 position + voxels
    let total_size = header_size + per_chunk * world.chunks.len();

    let mut buf = Vec::with_capacity(total_size);
    buf.write_all(&key.to_le_bytes())?;
    buf.write_all(&(world.chunks_x as u32).to_le_bytes())?;
    buf.write_all(&(world.chunks_y as u32).to_le_bytes())?;
    buf.write_all(&(world.chunks_z as u32).to_le_bytes())?;
    buf.write_all(&chunk_count.to_le_bytes())?;

    for chunk in &world.chunks {
        buf.write_all(&chunk.position.x.to_le_bytes())?;
        buf.write_all(&chunk.position.y.to_le_bytes())?;
        buf.write_all(&chunk.position.z.to_le_bytes())?;
        buf.write_all(bytemuck::cast_slice(chunk.voxels.as_ref()))?;
    }

    std::fs::write(path, buf)
}

/// Load world voxel data from the cache. Returns None if cache is missing,
/// corrupted, or key doesn't match.
pub fn load_world_cache(
    path: &Path,
    expected_key: u64,
) -> Option<Vec<crate::world::chunk::Chunk>> {
    use crate::world::chunk::{Chunk, CHUNK_VOLUME};
    use crate::world::voxel::Voxel;
    use glam::IVec3;

    let data = std::fs::read(path).ok()?;
    let voxel_size = std::mem::size_of::<Voxel>();

    // Read header (8 + 4 + 4 + 4 + 4 = 24 bytes)
    if data.len() < 24 {
        let _ = std::fs::remove_file(path);
        return None;
    }

    let key = u64::from_le_bytes(data[0..8].try_into().ok()?);
    if key != expected_key {
        return None;
    }

    let _chunks_x = u32::from_le_bytes(data[8..12].try_into().ok()?);
    let _chunks_y = u32::from_le_bytes(data[12..16].try_into().ok()?);
    let _chunks_z = u32::from_le_bytes(data[16..20].try_into().ok()?);
    let chunk_count = u32::from_le_bytes(data[20..24].try_into().ok()?) as usize;

    let per_chunk_bytes = 12 + voxel_size * CHUNK_VOLUME;
    let expected_size = 24 + per_chunk_bytes * chunk_count;
    if data.len() < expected_size {
        let _ = std::fs::remove_file(path);
        return None;
    }

    let mut chunks = Vec::with_capacity(chunk_count);
    let mut offset = 24;

    for _ in 0..chunk_count {
        let px = i32::from_le_bytes(data[offset..offset + 4].try_into().ok()?);
        let py = i32::from_le_bytes(data[offset + 4..offset + 8].try_into().ok()?);
        let pz = i32::from_le_bytes(data[offset + 8..offset + 12].try_into().ok()?);
        offset += 12;

        let voxel_data = &data[offset..offset + voxel_size * CHUNK_VOLUME];
        offset += voxel_size * CHUNK_VOLUME;

        // Allocate chunk and copy voxel data
        let mut chunk = Chunk::new(IVec3::new(px, py, pz));
        let src: &[Voxel] = bytemuck::cast_slice(voxel_data);
        chunk.voxels.copy_from_slice(src);
        chunks.push(chunk);
    }

    Some(chunks)
}

/// Delete old world cache files (all `world_*.bin` files in the cache parent dir).
pub fn clear_world_cache(cache_dir: &Path) -> io::Result<u64> {
    let parent = cache_dir.parent().unwrap_or(cache_dir);
    let mut count = 0u64;
    if parent.exists() {
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("world_") && name_str.ends_with(".bin") {
                let _ = std::fs::remove_file(entry.path());
                count += 1;
            }
        }
    }
    Ok(count)
}