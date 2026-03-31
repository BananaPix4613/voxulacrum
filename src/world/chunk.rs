use std::sync::Arc;
use glam::IVec3;
use smallvec::SmallVec;
use std::time::Instant;

use super::storage::ChunkStorage;
use super::voxel::MAT_AIR;

pub const CHUNK_SIZE: usize = 32;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;
pub const VOXEL_SCALE: f32 = 0.5;
pub const CHUNK_WORLD_SIZE: f32 = CHUNK_SIZE as f32 * VOXEL_SCALE; // 16.0
/// Number of voxels of padding on each side of the snapshot.
/// Must be >= AO kernel radius (2) + 1 to prevent boundary AO seams.
pub const SNAP_PAD: usize = 3;
/// Padded snapshot size: CHUNK_SIZE + 2*SNAP_PAD
pub const SNAP_SIZE: usize = CHUNK_SIZE + 2 * SNAP_PAD;
pub const SNAP_VOLUME: usize = SNAP_SIZE * SNAP_SIZE * SNAP_SIZE;
/// When edit count exceeds this, store full chunk instead of delta.
pub const DELTA_THRESHOLD: usize = CHUNK_VOLUME / 4; // 8192

pub struct ChunkMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

/// A single voxel modification for delta persistence.
#[derive(Clone, Copy)]
pub struct VoxelEdit {
    pub index: u16,
    pub density: i8,
    pub material_id: u16,
    // Optional fields (bitflag-controlled in serialization)
    pub moisture: Option<u8>,
    pub flora_id: Option<u16>,
    pub flora_growth: Option<u8>,
}

pub struct Chunk {
    pub position: IVec3,
    pub storage: Arc<ChunkStorage>,
    pub mesh_dirty: bool,
    /// Monotonic counter incremented each time mesh_dirty is set to true.
    /// Used to detect stale mesh results: if a neighbor loads while this chunk
    /// is mid-mesh, the returned mesh has incorrect borders. The pipeline records
    /// mesh_seq at snapshot time and only clears mesh_dirty on upload if it matches.
    pub mesh_seq: u64,
    pub mesh: Option<ChunkMesh>,
    pub generation: u64,
    pub persist_dirty: bool,
    pub edit_list: Option<Vec<VoxelEdit>>,
    /// When the last edit occurred. None = generation-triggered dirty (no debounce).
    pub mesh_debounce: Option<Instant>,
}

impl Chunk {
    pub fn new(position: IVec3, storage: Arc<ChunkStorage>) -> Self {
        Self {
            position,
            storage,
            mesh_dirty: true,
            mesh_seq: 0,
            mesh: None,
            generation: 0,
            persist_dirty: false,
            edit_list: None,
            mesh_debounce: None,
        }
    }

    /// Create a chunk with default air storage.
    pub fn new_air(position: IVec3) -> Self {
        Self::new(position, Arc::new(ChunkStorage::new_air()))
    }

    /// Mark this chunk as needing re-meshing, incrementing the sequence counter.
    pub fn mark_mesh_dirty(&mut self) {
        self.mesh_dirty = true;
        self.mesh_seq = self.mesh_seq.wrapping_add(1);
    }
    
    /// Mark dirty due to a voxel edit (applies debounce timer).
    pub fn mark_mesh_dirty_from_edit(&mut self) {
        self.mesh_dirty = true;
        self.mesh_seq = self.mesh_seq.wrapping_add(1);
        self.mesh_debounce = Some(Instant::now());
    }

    #[inline]
    pub fn voxel_index(x: usize, y: usize, z: usize) -> usize {
        x + y * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE
    }

    #[inline]
    pub fn density(&self, x: usize, y: usize, z: usize) -> i8 {
        self.storage.density(Self::voxel_index(x, y, z))
    }

    #[inline]
    pub fn material(&self, x: usize, y: usize, z: usize) -> u16 {
        self.storage.material(Self::voxel_index(x, y, z))
    }

    #[inline]
    pub fn is_solid(&self, x: usize, y: usize, z: usize) -> bool {
        self.storage.density(Self::voxel_index(x, y, z)) > 0
    }
}

pub struct ChunkNeighbors<'a> {
    /// 27-entry array: index = (dx+1)*9 + (dy+1)*3 + (dz+1)
    /// where dx,dy,dz ∈ {-1, 0, 1}. Index 13 = (0,0,0) = self, unused.
    pub neighbors: [Option<&'a Chunk>; 27],
}

impl<'a> ChunkNeighbors<'a> {
    pub fn empty() -> Self {
        Self { neighbors: [None; 27] }
    }

    #[inline]
    pub fn set(&mut self, dx: i32, dy: i32, dz: i32, chunk: Option<&'a Chunk>) {
        let idx = ((dx + 1) * 9 + (dy + 1) * 3 + (dz + 1)) as usize;
        self.neighbors[idx] = chunk;
    }

    #[inline]
    pub fn get(&self, dx: i32, dy: i32, dz: i32) -> Option<&'a Chunk> {
        let idx = ((dx + 1) * 9 + (dy + 1) * 3 + (dz + 1)) as usize;
        self.neighbors[idx]
    }
}

// ============================================================================
// ChunkSnapshot — SoA density array + lazy material lookup
// ============================================================================

/// Material lookup through Arc<ChunkStorage> references.
/// Only accessed at surface voxels (~10-20% of volume), so indirection cost is minimal.
pub struct SnapshotMaterials {
    /// 27-entry array indexed by (dx+1)*9 + (dy+1)*3 + (dz+1).
    /// Index 13 = center chunk.
    storages: [Option<Arc<ChunkStorage>>; 27],
}

impl SnapshotMaterials {
    /// Look up material at chunk-local coordinates (supports neighbor ranges).
    pub fn material(&self, x: i32, y: i32, z: i32) -> u16 {
        let cs = CHUNK_SIZE as i32;
        let (dx, lx) = if x < 0 {
            (-1, (x + cs) as usize)
        } else if x >= cs {
            (1, (x - cs) as usize)
        } else {
            (0, x as usize)
        };
        let (dy, ly) = if y < 0 {
            (-1, (y + cs) as usize)
        } else if y >= cs {
            (1, (y - cs) as usize)
        } else {
            (0, y as usize)
        };
        let (dz, lz) = if z < 0 {
            (-1, (z + cs) as usize)
        } else if z >= cs {
            (1, (z - cs) as usize)
        } else {
            (0, z as usize)
        };

        let ni = ((dx + 1) * 9 + (dy + 1) * 3 + (dz + 1)) as usize;
        if let Some(storage) = &self.storages[ni] {
            return storage.material(Chunk::voxel_index(lx, ly, lz));
        }

        // Diagonal/edge neighbor not loaded — use center chunk's nearest boundary voxel.
        let cx = x.clamp(0, CHUNK_SIZE as i32 - 1) as usize;
        let cy = y.clamp(0, CHUNK_SIZE as i32 - 1) as usize;
        let cz = z.clamp(0, CHUNK_SIZE as i32 - 1) as usize;
        match &self.storages[13] {
            Some(storage) => storage.material(Chunk::voxel_index(cx, cy, cz)),
            None => MAT_AIR,
        }
    }
}

/// A self-contained snapshot of all data needed to mesh one chunk.
/// Contains a flat density array for cache-optimal MC iteration and
/// lazy material lookup through Arc references to chunk storages.
pub struct ChunkSnapshot {
    pub position: IVec3,
    pub density: Box<[i8; SNAP_VOLUME]>,
    pub materials: SnapshotMaterials,
    /// True per axis if this chunk is at the negative world border.
    pub border_min: [bool; 3],
}

impl ChunkSnapshot {
    /// Create a snapshot by copying voxels from the chunk and its neighbors.
    pub fn extract(chunk: &Chunk, neighbors: &ChunkNeighbors, min_chunk_y: i32, _max_chunk_y: i32) -> Self {
        // Allocate density array on heap
        let mut density: Box<[i8; SNAP_VOLUME]> = unsafe {
            let v: Vec<i8> = vec![0i8; SNAP_VOLUME];
            let boxed_slice = v.into_boxed_slice();
            Box::from_raw(Box::into_raw(boxed_slice) as *mut [i8; SNAP_VOLUME])
        };

        // Fill interior: chunk's own 32^3 voxels at snapshot coords [PAD..CHUNK_SIZE+PAD)
        // Optimized: copy row-by-row for cache locality
        match chunk.storage.density_slice() {
            Some(src_density) => {
                for z in 0..CHUNK_SIZE {
                    for y in 0..CHUNK_SIZE {
                        let src_start = Chunk::voxel_index(0, y, z);
                        let dst_start = Self::snap_index(SNAP_PAD, y + SNAP_PAD, z + SNAP_PAD);
                        density[dst_start..dst_start + CHUNK_SIZE]
                            .copy_from_slice(&src_density[src_start..src_start + CHUNK_SIZE]);
                    }
                }
            }
            None => {
                // Uniform chunk: fill interior with constant density
                let d = chunk.storage.density(0);
                for z in 0..CHUNK_SIZE {
                    for y in 0..CHUNK_SIZE {
                        let dst_start = Self::snap_index(SNAP_PAD, y + SNAP_PAD, z + SNAP_PAD);
                        density[dst_start..dst_start + CHUNK_SIZE].fill(d);
                    }
                }
            }
        }

        // Fill border voxels
        for sz in 0..SNAP_SIZE {
            for sy in 0..SNAP_SIZE {
                for sx in 0..SNAP_SIZE {
                    // Skip interior (already filled)
                    if sx >= SNAP_PAD && sx < CHUNK_SIZE + SNAP_PAD
                        && sy >= SNAP_PAD && sy < CHUNK_SIZE + SNAP_PAD
                        && sz >= SNAP_PAD && sz < CHUNK_SIZE + SNAP_PAD
                    {
                        continue;
                    }

                    let cx = sx as i32 - SNAP_PAD as i32;
                    let cy = sy as i32 - SNAP_PAD as i32;
                    let cz = sz as i32 - SNAP_PAD as i32;

                    density[Self::snap_index(sx, sy, sz)] =
                        resolve_density(chunk, neighbors, cx, cy, cz);
                }
            }
        }

        // Build material sources from Arc references
        let mut storages: [Option<Arc<ChunkStorage>>; 27] = Default::default();
        // Center chunk at index 13 = (0+1)*9 + (0+1)*3 + (0+1)
        storages[13] = Some(Arc::clone(&chunk.storage));
        for (i, neighbor_opt) in neighbors.neighbors.iter().enumerate() {
            if i == 13 { continue; } // skip center
            if let Some(neighbor) = neighbor_opt {
                storages[i] = Some(Arc::clone(&neighbor.storage));
            }
        }

        let border_min = [
            false,
            chunk.position.y == min_chunk_y,
            false,
        ];

        Self {
            position: chunk.position,
            density,
            materials: SnapshotMaterials { storages },
            border_min,
        }
    }

    #[inline]
    pub fn snap_index(sx: usize, sy: usize, sz: usize) -> usize {
        sx + sy * SNAP_SIZE + sz * SNAP_SIZE * SNAP_SIZE
    }

    /// Read density at chunk-local coordinates (x in -PAD..=CHUNK_SIZE+PAD-1).
    #[inline]
    pub fn get_density(&self, x: i32, y: i32, z: i32) -> i8 {
        let pad = SNAP_PAD as i32;
        let sx = (x + pad) as usize;
        let sy = (y + pad) as usize;
        let sz = (z + pad) as usize;
        debug_assert!(
            sx < SNAP_SIZE && sy < SNAP_SIZE && sz < SNAP_SIZE,
            "ChunkSnapshot::get_density out of range: ({}, {}, {})", x, y, z
        );
        self.density[Self::snap_index(sx, sy, sz)]
    }

    /// Read material at chunk-local coordinates (lazy lookup through Arc refs).
    #[inline]
    pub fn get_material(&self, x: i32, y: i32, z: i32) -> u16 {
        self.materials.material(x, y, z)
    }
}

/// Resolve density at chunk-local coords, reading from chunk or neighbors.
///
/// When the required neighbor chunk is not loaded (edge/diagonal neighbors are
/// not guaranteed to be present), we fall back through progressively simpler
/// lookups: first try face neighbors along each out-of-range axis, then fall
/// back to the chunk's own nearest corner voxel. This prevents false air
/// pockets at chunk corners that would produce vertex artifacts in marching cubes.
fn resolve_density(chunk: &Chunk, neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> i8 {
    let cs = CHUNK_SIZE as i32;
    let (dx, lx) = if x < 0 { (-1, (x + cs) as usize) }
                else if x >= cs { (1, (x - cs) as usize) }
                else { (0, x as usize) };
    let (dy, ly) = if y < 0 { (-1, (y + cs) as usize) }
                else if y >= cs { (1, (y - cs) as usize) }
                else { (0, y as usize) };
    let (dz, lz) = if z < 0 { (-1, (z + cs) as usize) }
                else if z >= cs { (1, (z - cs) as usize) }
                else { (0, z as usize) };

    if dx == 0 && dy == 0 && dz == 0 {
        return chunk.storage.density(Chunk::voxel_index(lx, ly, lz));
    }

    if let Some(neighbor) = neighbors.get(dx, dy, dz) {
        return neighbor.storage.density(Chunk::voxel_index(lx, ly, lz));
    }

    // Neighbor not loaded (edge/diagonal neighbors are not guaranteed present).
    // Use the chunk's own nearest boundary voxel as an approximation. This
    // assumes terrain is locally continuous, which holds for the 2-voxel border.
    let cx = x.clamp(0, cs - 1) as usize;
    let cy = y.clamp(0, cs - 1) as usize;
    let cz = z.clamp(0, cs - 1) as usize;
    chunk.storage.density(Chunk::voxel_index(cx, cy, cz))
}

/// Given a flat voxel index, return chunk-relative offsets of neighbors
/// whose mesh is also stale due to border proximity (within 1 voxel of face).
pub fn border_dirty_neighbors(index: u16) -> SmallVec<[IVec3; 3]> {
    let x = (index as usize % CHUNK_SIZE) as i32;
    let y = ((index as usize / CHUNK_SIZE) % CHUNK_SIZE) as i32;
    let z = (index as usize / (CHUNK_SIZE * CHUNK_SIZE)) as i32;
    let mut neighbors = SmallVec::new();
    if x == 0                       { neighbors.push(IVec3::new(-1, 0, 0)); }
    if x == (CHUNK_SIZE as i32 - 1) { neighbors.push(IVec3::new( 1, 0, 0)); }
    if y == 0                       { neighbors.push(IVec3::new( 0,-1, 0)); }
    if y == (CHUNK_SIZE as i32 - 1) { neighbors.push(IVec3::new( 0, 1, 0)); }
    if z == 0                       { neighbors.push(IVec3::new( 0, 0,-1)); }
    if z == (CHUNK_SIZE as i32 - 1) { neighbors.push(IVec3::new( 0, 0, 1)); }
    neighbors
}