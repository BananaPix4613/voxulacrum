use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use glam::IVec3;
use std::time::Instant;

use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::ChunkSnapshot;

/// Cache file format version. Increment when TerrainVertex layout or
/// serialization format changes to automatically invalidate old caches.
const CACHE_VERSION: u32 = 9;

// ============================================================================
// Cache file format
// ============================================================================

/// Ultra-compact disk vertex format (6 bytes). NOT used on GPU.
///
/// Exploits domain constraints:
/// - Positions are snapped to 0.125 grid (u8 per axis, relative to chunk origin)
/// - Normals are one of 26 predefined directions (u8 index)
/// - Colors are derived from material_id (not stored)
/// - AO range 0.5..1.0 (u8 quantized)
/// - Only 9 materials (u8)
/// - cell_flags always 0 (not stored)
#[derive(Serialize, Deserialize)]
struct CompactVertex {
    pos_x: u8,
    pos_y: u8,
    pos_z: u8,
    normal_index: u8,
    ao: u8,
    material_id: u8,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    key_hash: u64,
    chunk_pos: [i32; 3],    // needed to reconstruct world-space positions
    vertex_count: u32,
    index_count: u32,
    index_format: u8,       // 0 = u16 indices, 1 = u32 indices
    vertices: Vec<CompactVertex>,
    indices_u16: Vec<u16>,  // populated when index_format == 0
    indices_u32: Vec<u32>,  // populated when index_format == 1
}

use crate::world::chunk::CHUNK_WORLD_SIZE;
use super::{find_normal_index, ALLOWED_NORMALS};

fn to_compact(v: &TerrainVertex, chunk_origin: [f32; 3]) -> CompactVertex {
    CompactVertex {
        pos_x: ((v.position[0] - chunk_origin[0] + 0.5) * 8.0 + 0.5) as u8,
        pos_y: ((v.position[1] - chunk_origin[1] + 0.5) * 8.0 + 0.5) as u8,
        pos_z: ((v.position[2] - chunk_origin[2] + 0.5) * 8.0 + 0.5) as u8,
        normal_index: find_normal_index(v.normal),
        ao: ((v.ao - 0.5) * 510.0 + 0.5) as u8,
        material_id: v.material_id as u8,
    }
}

fn from_compact(
    c: &CompactVertex,
    chunk_origin: [f32; 3],
    colors: &[[f32; 3]],
) -> TerrainVertex {
    let mat_idx = c.material_id as usize;
    let color = if mat_idx < colors.len() {
        colors[mat_idx]
    } else {
        [0.0; 3]
    };
    TerrainVertex {
        position: [
            c.pos_x as f32 / 8.0 - 0.5 + chunk_origin[0],
            c.pos_y as f32 / 8.0 - 0.5 + chunk_origin[1],
            c.pos_z as f32 / 8.0 - 0.5 + chunk_origin[2],
        ],
        normal: ALLOWED_NORMALS[c.normal_index.min(ALLOWED_NORMALS.len() as u8 - 1) as usize],
        color,
        ao: c.ao as f32 / 510.0 + 0.5,
        material_id: c.material_id as u32,
        cell_flags: 0,
        _pad_vert: [0; 2],
    }
}

// ============================================================================
// Atomic stats (shared across worker threads)
// ============================================================================

pub struct AtomicCacheStats {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub errors: AtomicU64,
    pub total_bytes: AtomicU64,
    pub evictions: AtomicU64,
}

impl AtomicCacheStats {
    pub fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        self.total_bytes.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
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
/// - `density` (i8) and `material` (u16) for interior + face-adjacent border voxels
/// - Sharpness values from MATERIAL_TABLE (affects normal biasing in meshing)
///
/// Edge and corner border voxels (where 2+ axes are in the border region) are
/// excluded from the hash. This makes the cache key stable regardless of whether
/// diagonal neighbors are loaded — only face-adjacent neighbor data (which affects
/// visible seams) contributes to the key.
///
/// Uses SeaHash for fast, portable, non-cryptographic hashing.
pub fn compute_cache_key(
    snapshot: &ChunkSnapshot,
    sharpness_values: &[f32],
    colors: &[[f32; 3]],
    greedy_enabled: bool,
    flat_error: f32,
    flat_normal: f32,
) -> u64 {
    use crate::world::chunk::{CHUNK_SIZE, SNAP_PAD, SNAP_SIZE};
    use seahash::SeaHasher;
    use std::hash::Hasher;

    let mut hasher = SeaHasher::new();

    for sz in 0..SNAP_SIZE {
        let z_border = sz < SNAP_PAD || sz >= CHUNK_SIZE + SNAP_PAD;
        for sy in 0..SNAP_SIZE {
            let y_border = sy < SNAP_PAD || sy >= CHUNK_SIZE + SNAP_PAD;
            if z_border && y_border {
                continue;
            }
            for sx in 0..SNAP_SIZE {
                let x_border = sx < SNAP_PAD || sx >= CHUNK_SIZE + SNAP_PAD;
                let border_count = x_border as u8 + y_border as u8 + z_border as u8;
                if border_count >= 2 {
                    continue;
                }
                let idx = sx + sy * SNAP_SIZE + sz * SNAP_SIZE * SNAP_SIZE;
                hasher.write_i8(snapshot.density[idx]);
                // Material lookup via chunk-local coordinates
                let cx = sx as i32 - SNAP_PAD as i32;
                let cy = sy as i32 - SNAP_PAD as i32;
                let cz = sz as i32 - SNAP_PAD as i32;
                hasher.write_u16(snapshot.get_material(cx, cy, cz));
            }
        }
    }

    for &s in sharpness_values {
        hasher.write(&s.to_le_bytes());
    }

    for color in colors {
        for &c in color {
            hasher.write(&c.to_le_bytes());
        }
    }

    hasher.write(&(greedy_enabled as u8).to_le_bytes());
    hasher.write(&flat_error.to_le_bytes());
    hasher.write(&flat_normal.to_le_bytes());

    hasher.finish()
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
    colors: &[[f32; 3]],
) -> Option<(Vec<TerrainVertex>, Vec<u32>)> {
    let compressed = std::fs::read(path).ok()?;
    let serialized = lz4_flex::decompress_size_prepended(&compressed).ok()?;
    let file: CacheFile = bincode::deserialize(&serialized).ok()?;

    if file.version != CACHE_VERSION {
        let _ = std::fs::remove_file(path);
        return None;
    }
    if file.key_hash != expected_key {
        return None;
    }
    if file.vertex_count as usize != file.vertices.len() {
        let _ = std::fs::remove_file(path);
        return None;
    }

    let chunk_origin = [
        file.chunk_pos[0] as f32 * CHUNK_WORLD_SIZE,
        file.chunk_pos[1] as f32 * CHUNK_WORLD_SIZE,
        file.chunk_pos[2] as f32 * CHUNK_WORLD_SIZE,
    ];

    let vertices: Vec<TerrainVertex> = file
        .vertices
        .iter()
        .map(|c| from_compact(c, chunk_origin, colors))
        .collect();

    // Reconstruct u32 indices from whichever format was stored
    let indices = if file.index_format == 0 {
        file.indices_u16.iter().map(|&i| i as u32).collect()
    } else {
        file.indices_u32
    };

    Some((vertices, indices))
}

/// Remove stale cache files for a chunk position, keeping only the file that
/// matches `current_key`.  Called before saving so that parameter changes don't
/// leave orphaned files with the old hash on disk.
fn remove_stale_for_chunk(
    cache_dir: &Path,
    chunk_x: i32,
    chunk_y: i32,
    chunk_z: i32,
    current_key: u64,
) {
    let prefix = format!("{}_{}_{}_{}", chunk_x, chunk_y, chunk_z, "");
    // ^ e.g. "0_5_-2_"
    let current_name = format!("{}_{}_{}_{:016x}.bin", chunk_x, chunk_y, chunk_z, current_key);

    let entries = match std::fs::read_dir(cache_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with(&prefix)
            && name_str.ends_with(".bin")
            && *name_str != current_name
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Save a mesh to the disk cache. Removes any stale cache files for the same
/// chunk position before writing.  Errors are non-fatal (caller should log and
/// continue).
pub fn save_cached_mesh(
    cache_dir: &Path,
    chunk_pos: glam::IVec3,
    key: u64,
    vertices: &[TerrainVertex],
    indices: &[u32],
) -> io::Result<u64> {
    std::fs::create_dir_all(cache_dir)?;
    remove_stale_for_chunk(cache_dir, chunk_pos.x, chunk_pos.y, chunk_pos.z, key);

    let path = cache_file_path(cache_dir, chunk_pos.x, chunk_pos.y, chunk_pos.z, key);

    let chunk_origin = [
        chunk_pos.x as f32 * CHUNK_WORLD_SIZE,
        chunk_pos.y as f32 * CHUNK_WORLD_SIZE,
        chunk_pos.z as f32 * CHUNK_WORLD_SIZE,
    ];
    let compact_verts: Vec<CompactVertex> = vertices
        .iter()
        .map(|v| to_compact(v, chunk_origin))
        .collect();

    // Use u16 indices when vertex count fits
    let use_u16 = vertices.len() <= u16::MAX as usize;

    let file = CacheFile {
        version: CACHE_VERSION,
        key_hash: key,
        chunk_pos: [chunk_pos.x, chunk_pos.y, chunk_pos.z],
        vertex_count: vertices.len() as u32,
        index_count: indices.len() as u32,
        index_format: if use_u16 { 0 } else { 1 },
        vertices: compact_verts,
        indices_u16: if use_u16 {
            indices.iter().map(|&i| i as u16).collect()
        } else {
            Vec::new()
        },
        indices_u32: if use_u16 {
            Vec::new()
        } else {
            indices.to_vec()
        },
    };

    let serialized =
        bincode::serialize(&file).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let compressed = lz4_flex::compress_prepend_size(&serialized);
    let size = compressed.len() as u64;
    std::fs::write(path, compressed)?;
    Ok(size)
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
// LRU tracking for cache eviction
// ============================================================================

struct LruEntry {
    path: PathBuf,
    last_access: Instant,
    size_bytes: u64,
}

pub struct MeshCacheLru {
    entries: HashMap<IVec3, LruEntry>,
    total_bytes: u64,
}

impl MeshCacheLru {
    /// Scan cache directory and populate LRU map. All entries start with the
    /// same access time (startup) since we have no persistent access history.
    pub fn from_scan(cache_dir: &Path) -> Self {
        let mut lru = Self { entries: HashMap::new(), total_bytes: 0 };
        let dir = match std::fs::read_dir(cache_dir) {
            Ok(d) => d,
            Err(_) => return lru,
        };
        let now = Instant::now();
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().map_or(true, |e| e != "bin") { continue; }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(pos) = parse_chunk_pos_from_filename(&name_str) {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                lru.entries.insert(pos, LruEntry {
                    path,
                    last_access: now,
                    size_bytes: size,
                });
                lru.total_bytes += size;
            }
        }
        lru
    }

    /// Update last-access time for a cache hit.
    pub fn touch(&mut self, pos: IVec3) {
        if let Some(entry) = self.entries.get_mut(&pos) {
            entry.last_access = Instant::now();
        }
    }

    /// Record a new or replaced cache write.
    pub fn insert(&mut self, pos: IVec3, path: PathBuf, size: u64) {
        let old_size = self.entries.get(&pos).map_or(0, |e| e.size_bytes);
        self.total_bytes = self.total_bytes - old_size + size;
        self.entries.insert(pos, LruEntry {
            path,
            last_access: Instant::now(),
            size_bytes: size,
        });
    }

    /// Evict oldest entries until under `max_bytes` or `batch_size` reached.
    /// Returns total bytes freed.
    pub fn evict(&mut self, max_bytes: u64, batch_size: usize) -> u64 {
        if self.total_bytes <= max_bytes { return 0; }

        // Collect and sort by access time (oldest first)
        let mut by_age: Vec<(IVec3, Instant)> = self.entries.iter()
            .map(|(&k, v)| (k, v.last_access))
            .collect();
        by_age.sort_by_key(|(_, t)| *t);

        let mut freed = 0u64;
        let mut count = 0usize;
        for (pos, _) in by_age {
            if self.total_bytes <= max_bytes || count >= batch_size { break; }
            if let Some(entry) = self.entries.remove(&pos) {
                let _ = std::fs::remove_file(&entry.path);
                self.total_bytes -= entry.size_bytes;
                freed += entry.size_bytes;
                count += 1;
            }
        }
        freed
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

/// Parse chunk position from a cache filename like "3_-1_5_00abc...def.bin".
fn parse_chunk_pos_from_filename(name: &str) -> Option<IVec3> {
    let stem = name.strip_suffix(".bin")?;
    let parts: Vec<&str> = stem.splitn(4, '_').collect();
    if parts.len() != 4 { return None; }
    Some(IVec3::new(
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

// ============================================================================
// World generation cache
// ============================================================================

/// Compute a cache key for world generation params.
pub fn compute_world_cache_key(params: &crate::params::TerrainGenParams) -> u64 {
    use seahash::SeaHasher;
    use std::hash::Hasher;

    let mut hasher = SeaHasher::new();
    // 3D terrain shape
    hasher.write(&params.terrain_freq.to_le_bytes());
    hasher.write(&params.selector_freq.to_le_bytes());
    hasher.write(&params.terrain_octaves.to_le_bytes());
    hasher.write(&params.terrain_gain.to_le_bytes());
    hasher.write(&params.y_squash.to_le_bytes());
    hasher.write(&params.terrain_warp.to_le_bytes());
    // 2D column noise
    hasher.write(&params.continental_freq.to_le_bytes());
    hasher.write(&params.temperature_freq.to_le_bytes());
    hasher.write(&params.humidity_freq.to_le_bytes());
    hasher.write(&params.erosion_freq.to_le_bytes());
    hasher.write(&params.elevation_freq.to_le_bytes());
    hasher.write(&params.elevation_warp.to_le_bytes());
    // Density post-processing
    hasher.write(&params.density_scale.to_le_bytes());
    hasher.write(&params.floor_level.to_le_bytes());
    hasher.write(&params.ceiling_level.to_le_bytes());
    // Cave system
    hasher.write(&(params.cave_enabled as u8).to_le_bytes());
    hasher.write(&params.water_level.to_le_bytes());
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

    // Estimate: populated ~98KB, uniform ~15B. Use populated upper bound.
    let per_chunk_estimate = 12 + 1 + 32768 + 32768 * 2;
    let total_estimate = 24 + per_chunk_estimate * world.chunks.len();

    let mut buf = Vec::with_capacity(total_estimate);
    buf.write_all(&key.to_le_bytes())?;
    // Format version 2 (distinguishes from old format which wrote 0 here)
    buf.write_all(&2u32.to_le_bytes())?;
    buf.write_all(&0u32.to_le_bytes())?;
    buf.write_all(&0u32.to_le_bytes())?;
    buf.write_all(&chunk_count.to_le_bytes())?;

    for chunk in world.chunks.values() {
        buf.write_all(&chunk.position.x.to_le_bytes())?;
        buf.write_all(&chunk.position.y.to_le_bytes())?;
        buf.write_all(&chunk.position.z.to_le_bytes())?;

        if chunk.storage.is_uniform() {
            buf.write_all(&[0u8])?; // variant tag: Uniform
            buf.write_all(&[chunk.storage.density(0) as u8])?;
            buf.write_all(&chunk.storage.material(0).to_le_bytes())?;
        } else {
            buf.write_all(&[1u8])?; // variant tag: Populated
            // Write density array
            if let Some(density_slice) = chunk.storage.density_slice() {
                // SAFETY: i8 and u8 have the same layout
                let bytes: &[u8] = unsafe {
                    std::slice::from_raw_parts(
                        density_slice.as_ptr() as *const u8,
                        crate::world::chunk::CHUNK_VOLUME,
                    )
                };
                buf.write_all(bytes)?;
            }
            // Write material array (flat u16 for simplicity)
            for i in 0..crate::world::chunk::CHUNK_VOLUME {
                buf.write_all(&chunk.storage.material(i).to_le_bytes())?;
            }
        }
    }

    let compressed = lz4_flex::compress_prepend_size(&buf);
    std::fs::write(path, compressed)
}

/// Load world voxel data from the cache. Returns None if cache is missing,
/// corrupted, or key doesn't match.
pub fn load_world_cache(
    path: &Path,
    expected_key: u64,
) -> Option<std::collections::HashMap<glam::IVec3, crate::world::chunk::Chunk>> {
    use crate::world::chunk::{Chunk, CHUNK_VOLUME};
    use crate::world::storage;
    use crate::world::voxel::MAT_AIR;
    use glam::IVec3;

    let compressed = std::fs::read(path).ok()?;
    let data = lz4_flex::decompress_size_prepended(&compressed).ok()?;

    if data.len() < 24 {
        let _ = std::fs::remove_file(path);
        return None;
    }

    let key = u64::from_le_bytes(data[0..8].try_into().ok()?);
    if key != expected_key {
        return None;
    }

    let format_version = u32::from_le_bytes(data[8..12].try_into().ok()?);
    if format_version != 2 {
        // Old format or unknown version - invalidate
        let _ = std::fs::remove_file(path);
        return None;
    }

    let chunk_count = u32::from_le_bytes(data[20..24].try_into().ok()?) as usize;

    let mut chunks = std::collections::HashMap::with_capacity(chunk_count);
    let mut offset = 24;

    for _ in 0..chunk_count {
        if offset + 13 > data.len() {
            let _ = std::fs::remove_file(path);
            return None;
        }

        let px = i32::from_le_bytes(data[offset..offset + 4].try_into().ok()?);
        let py = i32::from_le_bytes(data[offset + 4..offset + 8].try_into().ok()?);
        let pz = i32::from_le_bytes(data[offset + 8..offset + 12].try_into().ok()?);
        offset += 12;

        let variant_tag = data[offset];
        offset += 1;

        let pos = IVec3::new(px, py, pz);

        let chunk_storage = if variant_tag == 0 {
            // Uniform
            if offset + 3 > data.len() { return None; }
            let density = data[offset] as i8;
            offset += 1;
            let material_id = u16::from_le_bytes(data[offset..offset + 2].try_into().ok()?);
            offset += 2;
            crate::world::storage::ChunkStorage::Uniform { density, material_id}
        } else {
            // Populated
            let density_bytes = CHUNK_VOLUME;
            let material_bytes = CHUNK_VOLUME * 2;
            if offset + density_bytes + material_bytes > data.len() {
                let _ = std::fs::remove_file(path);
                return None;
            }

            // Read density
            let mut density_arr = vec![0i8; CHUNK_VOLUME].into_boxed_slice();
            for i in 0..CHUNK_VOLUME {
                density_arr[i] = data[offset + i] as i8;
            }
            offset += density_bytes;

            // Read material
            let mut material_arr = [MAT_AIR; CHUNK_VOLUME];
            for i in 0..CHUNK_VOLUME {
                let base = offset + i * 2;
                material_arr[i] = u16::from_le_bytes(data[base..base + 2].try_into().ok()?);
            }
            offset += material_bytes;

            let density_box: Box<[i8; CHUNK_VOLUME]> = unsafe {
                Box::from_raw(Box::into_raw(density_arr) as *mut [i8; CHUNK_VOLUME])
            };
            storage::storage_from_arrays(density_box, &material_arr)
        };

        let chunk = Chunk::new(pos, std::sync::Arc::new(chunk_storage));
        chunks.insert(pos, chunk);
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