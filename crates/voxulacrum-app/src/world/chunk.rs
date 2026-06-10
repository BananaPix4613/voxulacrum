use std::sync::Arc;
use glam::IVec3;
use smallvec::SmallVec;
use std::time::Instant;

use super::storage::ChunkStorage;
use voxel_core::Voxel;

pub const CHUNK_SIZE: usize = voxel_core::CHUNK_DIM;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;
pub const VOXEL_SCALE: f32 = 1.0;
pub const CHUNK_WORLD_SIZE: f32 = CHUNK_SIZE as f32 * VOXEL_SCALE; // 32.0
pub const SNAP_PAD: usize = 1;
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
    pub voxel: Voxel,
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
    pub fn voxel(&self, x: usize, y: usize, z: usize) -> Voxel {
        self.storage.voxel(Self::voxel_index(x, y, z))
    }

    #[inline]
    pub fn is_solid(&self, x: usize, y: usize, z: usize) -> bool {
        self.storage.is_solid(Self::voxel_index(x, y, z))
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

/// Self-contained snapshot used by the cube mesher. Holds the chunk's 32^3
/// voxels plus a 1-voxel border copied from face/edge/corner neighbors.
/// Missing neighbors fall back to `Voxel::EMPTY` (closed-world boundary).
pub struct ChunkSnapshot {
    pub position: IVec3,
    pub materials: Box<[Voxel; SNAP_VOLUME]>, // 34*34*34 = 39_304
    /// True per axis if this chunk is at the negative world border.
    pub border_min: [bool; 3],
}

impl ChunkSnapshot {
    pub fn extract(chunk: &Chunk, neighbors: &ChunkNeighbors, min_chunk_y: i32, _max_chunk_y: i32) -> Self {
        let mut materials: Box<[Voxel; SNAP_VOLUME]> = unsafe {
            let v: Vec<Voxel> = vec![Voxel::EMPTY; SNAP_VOLUME];
            let boxed_slice = v.into_boxed_slice();
            Box::from_raw(Box::into_raw(boxed_slice) as *mut [Voxel; SNAP_VOLUME])
        };

        // Fill interior: chunk's own 32^3 voxels at snapshot coords [PAD..CHUNK_SIZE+PAD).
        for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let m = chunk.storage.voxel(Chunk::voxel_index(x, y, z));
                    materials[Self::snap_index(x + SNAP_PAD, y + SNAP_PAD, z + SNAP_PAD)] = m;
                }
            }
        }

        // Fill 1-voxel border from face-adjacent neighbors (missing -> Voxel::EMPTY).
        for sz in 0..SNAP_SIZE {
            for sy in 0..SNAP_SIZE {
                for sx in 0..SNAP_SIZE {
                    let interior = sx >= SNAP_PAD && sx < CHUNK_SIZE + SNAP_PAD
                        && sy >= SNAP_PAD && sy < CHUNK_SIZE + SNAP_PAD
                        && sz >= SNAP_PAD && sz < CHUNK_SIZE + SNAP_PAD;
                    if interior { continue; }
                    let cx = sx as i32 - SNAP_PAD as i32;
                    let cy = sy as i32 - SNAP_PAD as i32;
                    let cz = sz as i32 - SNAP_PAD as i32;
                    materials[Self::snap_index(sx, sy, sz)] =
                        resolve_voxel(neighbors, cx, cy, cz);
                }
            }
        }

        let border_min = [false, chunk.position.y == min_chunk_y, false];

        Self {
            position: chunk.position,
            materials,
            border_min,
        }
    }

    #[inline]
    pub fn snap_index(sx: usize, sy: usize, sz: usize) -> usize {
        sx + sy * SNAP_SIZE + sz * SNAP_SIZE * SNAP_SIZE
    }

    /// Read the voxel at chunk-local coordinates (x in -PAD..CHUNK_SIZE+PAD).
    #[inline]
    pub fn get_voxel(&self, x: i32, y: i32, z: i32) -> Voxel {
        let pad = SNAP_PAD as i32;
        let sx = (x + pad) as usize;
        let sy = (y + pad) as usize;
        let sz = (z + pad) as usize;
        debug_assert!(
            sx < SNAP_SIZE && sy < SNAP_SIZE && sz < SNAP_SIZE,
            "ChunkSnapshot::get_voxel out of range: ({}, {}, {})", x, y, z
        );
        self.materials[Self::snap_index(sx, sy, sz)]
    }
}

/// Resolve a voxel at a chunk-border coord by reading the appropriate neighbor.
/// Missing neighbors (edge/corner slots) fall back to `Voxel::EMPTY`.
fn resolve_voxel(neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> Voxel {
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
        return Voxel::EMPTY;
    }

    match neighbors.get(dx, dy, dz) {
        Some(neighbor) => neighbor.storage.voxel(Chunk::voxel_index(lx, ly, lz)),
        None => Voxel::EMPTY,
    }
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