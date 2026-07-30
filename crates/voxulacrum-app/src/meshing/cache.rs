use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use glam::IVec3;
use std::time::Instant;

use crate::rendering::pipelines::FaceVertex;
use crate::world::chunk::{ChunkSnapshot, CHUNK_WORLD_SIZE};

/// Cache file format version. 15: cache key no longer hashes the snapshot's
/// edge/corner border cells, which the mesher never reads (drift observation
/// 5.1). Not strictly required - the new keys hash to different paths, so old
/// files are simply never looked up — but bumping makes any that are read
/// reject cleanly rather than relying on hash disjointness.
const CACHE_VERSION: u32 = 15;

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
    face_index: u8,     // 0..5 (== FaceVertex.face_axis); reconstructs the normal
    material_id: u16,
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

/// Six face normals as packed `i8` (Snorm8x4: +/-127 = +/-1.0), in the cube mesher's
/// face order (== `FaceAxis` order): +X, -X, +Y, -Y, +Z, -Z.
const FACE_NORMALS_I8: [[i8; 4]; 6] = [
    [ 127, 0, 0, 0],
    [-127, 0, 0, 0],
    [ 0,  127, 0, 0],
    [ 0, -127, 0, 0],
    [ 0, 0,  127, 0],
    [ 0, 0, -127, 0],
];

fn to_compact(v: &FaceVertex, chunk_origin: [f32; 3]) -> CompactVertex {
    let q = |world: f32, origin: f32| -> u8 {
        (((world - origin) * POS_STEPS_PER_VOXEL).round() as i32).clamp(0, 255) as u8
    };
    CompactVertex {
        pos_x: q(v.position[0], chunk_origin[0]),
        pos_y: q(v.position[1], chunk_origin[1]),
        pos_z: q(v.position[2], chunk_origin[2]),
        face_index: v.face_axis.min(5),
        material_id: v.material_id,
    }
}

/// Reconstruct a `FaceVertex` from its compact form. Color is not stored - the
/// shader composes it from `material_id` - so no color table is needed. The
/// constant bytes here must mirror what the cube mesher writes so a cache hit is
/// byte-identical to a fresh mesh (uv 0, ao_factor 255, §11 bytes 0).
fn from_compact(c: &CompactVertex, chunk_origin: [f32; 3]) -> FaceVertex {
    let face = (c.face_index as usize).min(5);
    FaceVertex {
        position: [
            c.pos_x as f32 / POS_STEPS_PER_VOXEL + chunk_origin[0],
            c.pos_y as f32 / POS_STEPS_PER_VOXEL + chunk_origin[1],
            c.pos_z as f32 / POS_STEPS_PER_VOXEL + chunk_origin[2],
        ],
        normal: FACE_NORMALS_I8[face],
        uv: [0, 0],
        material_id: c.material_id,
        biome_tint_index: 0,
        variant_index: 0,
        face_axis: face as u8,
        occlusion_class: 0,
        light_level_index: 0,
        enclosure_factor: 0,
        edge_flag: 0,
        sway_weight: 0,
        ao_factor: 255,
        _padding: 0,
    }
}

// ============================================================================
// Atomic stats (shared across worker threads)
// ============================================================================

pub struct AtomicCacheStats {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    /// Misses where this chunk position had no cached mesh at all - a genuine
    /// first visit.
    pub misses_cold: AtomicU64,
    /// Misses where a cached mesh existed for this position under a *different*
    /// key, so the content changed and the entry was re-keyed.
    ///
    /// This is drift observation 5.1 made visible: the key hashes the full 34³
    /// snapshot including the one-voxel neighbor border, so a chunk re-keys
    /// whenever a neighbor's edge changes during streaming. A high stale rate
    /// means the cache is rebuilding work it already had.
    pub misses_stale: AtomicU64,
    pub errors: AtomicU64,
    pub total_bytes: AtomicU64,
    pub evictions: AtomicU64,
}

impl AtomicCacheStats {
    pub fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            misses_cold: AtomicU64::new(0),
            misses_stale: AtomicU64::new(0),
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

/// Content key for a chunk's mesh: its interior plus the six face-adjacent
/// border planes, and deliberately **not** the border's edges and corners.
///
/// Those 392 cells (12 edges × 32, plus 8 corners) are filled by
/// `ChunkSnapshot::extract` from the diagonal neighbors, but meshing is gated
/// only on the six *face* neighbors being resident and `cube_mesher` culls
/// against the single face-adjacent voxel - it never reads a diagonal. So
/// including them made the key depend on whether unrelated neighbors happened
/// to be loaded at mesh time, which changes run to run.
///
/// Measured cost of that before this change (drift observation 5.1, priced for
/// the first time in Substep 13a): on a warm second run over an identical path,
/// 1,400 of 4,758 lookups re-keyed - each one a mesh rebuild plus a file delete
/// plus a file write for a mesh already on disk. Hashing only the data meshing
/// actually waits for makes the key load-order independent by construction.
///
/// This remains content-addressed rather than the input-addressed key design §10
/// specifies (graph hash + mesher version + registry hash + seed). Content
/// addressing is stronger for correctness - §10's stated failure mode cannot
/// occur - and that divergence stays recorded rather than closed here.
pub fn compute_cache_key(snapshot: &ChunkSnapshot) -> u64 {
    use crate::world::chunk::{CHUNK_SIZE, SNAP_PAD, SNAP_SIZE};
    use seahash::SeaHasher;
    use std::hash::Hasher;

    let interior = |v: usize| v >= SNAP_PAD && v < SNAP_PAD + CHUNK_SIZE;

    let mut hasher = SeaHasher::new();
    for sz in 0..SNAP_SIZE {
        for sy in 0..SNAP_SIZE {
            for sx in 0..SNAP_SIZE {
                // 0 axes outside = interior, 1 = a face plane, 2 = an edge,
                // 3 = a corner. Skip edges and corners.
                let outside = usize::from(!interior(sx))
                    + usize::from(!interior(sy))
                    + usize::from(!interior(sz));
                if outside >= 2 {
                    continue;
                }
                let v = snapshot.materials[ChunkSnapshot::snap_index(sx, sy, sz)];
                hasher.write_u32(v.pack());
            }
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
) -> Option<(Vec<FaceVertex>, Vec<u32>)> {
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

    let vertices: Vec<FaceVertex> = file.vertices
        .iter()
        .map(|c| from_compact(c, chunk_origin))
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
    vertices: &[FaceVertex],
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

    /// Whether this chunk position has a cached mesh under *any* key. Since
    /// `save_cached_mesh` removes stale siblings, there is at most one file per
    /// position, so this distinguishes a first-visit miss from a re-key.
    pub fn contains(&self, pos: IVec3) -> bool { self.entries.contains_key(&pos) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshing::cube_mesher::generate_chunk_mesh;
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

        let (vertices, indices) = generate_chunk_mesh(&snap);

        // Guard against vacuous round-trip: the mesh must contain half-step Ys.
        assert!(
            vertices.iter().any(|v| v.position[1].fract().abs() > 1e-5),
            "test mesh must contain sub-voxel vertex positions"
        );

        let dir = std::env::temp_dir().join(format!(
            "voxulacrum_mesh_cache_test_{}",
            std::process::id()
        ));
        let key = compute_cache_key(&snap);
        let path = cache_file_path(&dir, snap.position.x, snap.position.y, snap.position.z, key);

        save_cached_mesh(&dir, snap.position, key, &vertices, &indices).expect("save");
        let (loaded_verts, loaded_indices) =
            load_cached_mesh(&path, key).expect("load");
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
