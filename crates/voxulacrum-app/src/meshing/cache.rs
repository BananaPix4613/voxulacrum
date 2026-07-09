use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use glam::IVec3;
use std::time::Instant;

use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::{ChunkSnapshot, CHUNK_WORLD_SIZE};

/// Cache file format version. Bumped for half-step vertex quantization (v11
/// stored integer positions, which flattened slab meshes on the round-trip).
const CACHE_VERSION: u32 = 12;

/// Positions are quantized to this many steps per voxel. Shapes occupy
/// half-cell vertical intervals (see `shape_y_interval`), so 2 steps/voxel
/// makes the round-trip exact; doubled coords (0..=64) fit in u8.
const POS_STEPS_PER_VOXEL: f32 = 2.0;

// ============================================================================
// Compact disk vertex
// ============================================================================

/// Cube-mesh vertices live on half-voxel steps: cubes on integer corners,
/// slab faces at +0.5. Positions are stored at `POS_STEPS_PER_VOXEL`
/// resolution in u8, normals are one of six face directions, materials in u8.
#[derive(Serialize, Deserialize)]
struct CompactVertex {
    pos_x: u8,
    pos_y: u8,
    pos_z: u8,
    normal_index: u8,
    material_id: u8,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    key_hash: u64,
    chunk_pos: [i32; 3],
    vertex_count: u32,
    index_count: u32,
    index_format: u8,       // 0 = u16, 1 = u32
    vertices: Vec<CompactVertex>,
    indices_u16: Vec<u16>,
    indices_u32: Vec<u32>,
}

/// Six face normals in the same order the cube mesher emits them.
const FACE_NORMALS: [[f32; 3]; 6] = [
    [ 1.0, 0.0, 0.0],
    [-1.0, 0.0, 0.0],
    [ 0.0, 1.0, 0.0],
    [ 0.0,-1.0, 0.0],
    [ 0.0, 0.0, 1.0],
    [ 0.0, 0.0,-1.0],
];

fn face_normal_index(n: [f32; 3]) -> u8 {
    for (i, fn_) in FACE_NORMALS.iter().enumerate() {
        if (n[0] - fn_[0]).abs() < 1e-3
            && (n[1] - fn_[1]).abs() < 1e-3
            && (n[2] - fn_[2]).abs() < 1e-3
        {
            return i as u8;
        }
    }
    0
}

fn to_compact(v: &TerrainVertex, chunk_origin: [f32; 3]) -> CompactVertex {
    let q = |world: f32, origin: f32| -> u8 {
        (((world - origin) * POS_STEPS_PER_VOXEL).round() as i32).clamp(0, 255) as u8
    };
    CompactVertex {
        pos_x: q(v.position[0], chunk_origin[0]),
        pos_y: q(v.position[1], chunk_origin[1]),
        pos_z: q(v.position[2], chunk_origin[2]),
        normal_index: face_normal_index(v.normal),
        material_id: v.material_id as u8,
    }
}

fn from_compact(
    c: &CompactVertex,
    chunk_origin: [f32; 3],
    colors: &[[f32; 3]],
) -> TerrainVertex {
    let mat_idx = c.material_id as usize;
    let color = if mat_idx < colors.len() { colors[mat_idx] } else { [0.0; 3] };
    TerrainVertex {
        position: [
            c.pos_x as f32 / POS_STEPS_PER_VOXEL + chunk_origin[0],
            c.pos_y as f32 / POS_STEPS_PER_VOXEL + chunk_origin[1],
            c.pos_z as f32 / POS_STEPS_PER_VOXEL + chunk_origin[2],
        ],
        normal: FACE_NORMALS[(c.normal_index as usize).min(5)],
        color,
        ao: 1.0,
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

// ============================================================================
// Cache key
// ============================================================================

/// Hash the entire 34^3 materials snapshot plus the material color table.
/// The cube mesher's output depends only on materials + colors, so anything
/// else would inflate the key without changing correctness.
pub fn compute_cache_key(snapshot: &ChunkSnapshot, colors: &[[f32; 3]]) -> u64 {
    use seahash::SeaHasher;
    use std::hash::Hasher;

    let mut hasher = SeaHasher::new();
    for &m in snapshot.materials.iter() {
        hasher.write_u32(m.pack());
    }
    for color in colors {
        for &c in color {
            hasher.write(&c.to_le_bytes());
        }
    }
    hasher.finish()
}

// ============================================================================
// File path
// ============================================================================

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
    if file.key_hash != expected_key { return None; }
    if file.vertex_count as usize != file.vertices.len() {
        let _ = std::fs::remove_file(path);
        return None;
    }

    let chunk_origin = [
        file.chunk_pos[0] as f32 * CHUNK_WORLD_SIZE,
        file.chunk_pos[1] as f32 * CHUNK_WORLD_SIZE,
        file.chunk_pos[2] as f32 * CHUNK_WORLD_SIZE,
    ];

    let vertices: Vec<TerrainVertex> = file.vertices
        .iter()
        .map(|c| from_compact(c, chunk_origin, colors))
        .collect();

    let indices = if file.index_format == 0 {
        file.indices_u16.iter().map(|&i| i as u32).collect()
    } else {
        file.indices_u32
    };

    Some((vertices, indices))
}

fn remove_stale_for_chunk(
    cache_dir: &Path,
    chunk_x: i32,
    chunk_y: i32,
    chunk_z: i32,
    current_key: u64,
) {
    let prefix = format!("{}_{}_{}_", chunk_x, chunk_y, chunk_z);
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

    let use_u16 = vertices.len() <= u16::MAX as usize;

    let file = CacheFile {
        version: CACHE_VERSION,
        key_hash: key,
        chunk_pos: [chunk_pos.x, chunk_pos.y, chunk_pos.z],
        vertex_count: vertices.len() as u32,
        index_count: indices.len() as u32,
        index_format: if use_u16 { 0 } else { 1 },
        vertices: compact_verts,
        indices_u16: if use_u16 { indices.iter().map(|&i| i as u16).collect() } else { Vec::new() },
        indices_u32: if use_u16 { Vec::new() } else { indices.to_vec() },
    };

    let serialized = bincode::serialize(&file)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let compressed = lz4_flex::compress_prepend_size(&serialized);
    let size = compressed.len() as u64;
    std::fs::write(path, compressed)?;
    Ok(size)
}

// ============================================================================
// Cache management
// ============================================================================

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

// ============================================================================
// LRU tracking
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

    pub fn touch(&mut self, pos: IVec3) {
        if let Some(entry) = self.entries.get_mut(&pos) {
            entry.last_access = Instant::now();
        }
    }

    pub fn insert(&mut self, pos: IVec3, path: PathBuf, size: u64) {
        let old_size = self.entries.get(&pos).map_or(0, |e| e.size_bytes);
        self.total_bytes = self.total_bytes - old_size + size;
        self.entries.insert(pos, LruEntry {
            path,
            last_access: Instant::now(),
            size_bytes: size,
        });
    }

    pub fn evict(&mut self, max_bytes: u64, batch_size: usize) -> u64 {
        if self.total_bytes <= max_bytes { return 0; }

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

    pub fn total_bytes(&self) -> u64 { self.total_bytes }
    pub fn entry_count(&self) -> usize { self.entries.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshing::{cube_mesher::generate_chunk_mesh, MaterialConfig};
    use crate::world::chunk::{SNAP_PAD, SNAP_VOLUME};
    use voxel_core::{MaterialId, ShapeId, Voxel};

    /// The save/load round-trip must reproduce vertex positions exactly,
    /// including sub-voxel (half-step) coordinates. Regression test for the
    /// v11 format, whose integer quantization flattened slab faces (y + 0.5)
    /// to full cube height on load.
    #[test]
    fn roundtrip_preserves_subvoxel_vertex_positions() {
        // A snapshot with one cube and one slab, at a non-zero chunk position
        // so the origin add/subtract is exercised too.
        let materials: Box<[Voxel; SNAP_VOLUME]> =
            vec![Voxel::EMPTY; SNAP_VOLUME].into_boxed_slice().try_into().unwrap();
        let mut snap = ChunkSnapshot {
            position: IVec3::new(3, 1, -2),
            materials,
            border_min: [false; 3],
        };
        let stone = MaterialId(1);
        let put = |snap: &mut ChunkSnapshot, x: usize, y: usize, z: usize, v: Voxel| {
            snap.materials[ChunkSnapshot::snap_index(x + SNAP_PAD, y + SNAP_PAD, z + SNAP_PAD)] = v;
        };
        put(&mut snap, 4, 4, 4, Voxel::cube(stone));
        put(&mut snap, 5, 4, 4, Voxel { material: stone, shape: ShapeId::SlabBottom, flags: 0 });

        let config = MaterialConfig { colors: vec![[0.0; 3], [0.5, 0.5, 0.5]] };
        let (vertices, indices) = generate_chunk_mesh(&snap, &config);

        // Guard against vacuous round-trip: the mesh must contain half-step Ys.
        assert!(
            vertices.iter().any(|v| v.position[1].fract().abs() > 1e-5),
            "test mesh must contain sub-voxel vertex positions"
        );

        let dir = std::env::temp_dir().join(format!(
            "voxulacrum_mesh_cache_test_{}",
            std::process::id()
        ));
        let key = compute_cache_key(&snap, &config.colors);
        let path = cache_file_path(&dir, snap.position.x, snap.position.y, snap.position.z, key);

        save_cached_mesh(&dir, snap.position, key, &vertices, &indices).expect("save");
        let (loaded_verts, loaded_indices) =
            load_cached_mesh(&path, key, &config.colors).expect("load");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(loaded_indices, indices);
        assert_eq!(loaded_verts.len(), vertices.len());
        for (a, b) in vertices.iter().zip(&loaded_verts) {
            for axis in 0..3 {
                assert!(
                    (a.position[axis] - b.position[axis]).abs() < 1e-5,
                    "vertex position changed across cache round-trip: {:?} -> {:?}",
                    a.position, b.position
                );
            }
            assert_eq!(a.normal, b.normal);
            assert_eq!(a.material_id, b.material_id);
        }
    }
}

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
