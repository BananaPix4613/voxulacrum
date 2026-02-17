use glam::IVec3;
use super::voxel::Voxel;

pub const CHUNK_SIZE: usize = 32;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;
pub const VOXEL_SCALE: f32 = 0.5;
pub const CHUNK_WORLD_SIZE: f32 = CHUNK_SIZE as f32 * VOXEL_SCALE; // 16.0
/// Padded snapshot size: CHUNK_SIZE + 2 (one voxel border on each side)
pub const SNAP_SIZE: usize = CHUNK_SIZE + 2;
pub const SNAP_VOLUME: usize = SNAP_SIZE * SNAP_SIZE * SNAP_SIZE;

pub struct ChunkMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

pub struct Chunk {
    pub position: IVec3,
    pub voxels: Box<[Voxel; CHUNK_VOLUME]>,
    pub mesh_dirty: bool,
    pub mesh: Option<ChunkMesh>,
}

impl Chunk {
    pub fn new(position: IVec3) -> Self {
        // Allocate on heap to avoid stack overflow (384 KB per chunk)
        let voxels: Box<[Voxel; CHUNK_VOLUME]> = unsafe {
            let v: Vec<Voxel> = vec![Voxel::default(); CHUNK_VOLUME];
            let boxed_slice = v.into_boxed_slice();
            Box::from_raw(Box::into_raw(boxed_slice) as *mut [Voxel; CHUNK_VOLUME])
        };

        Self {
            position,
            voxels,
            mesh_dirty: true,
            mesh: None,
        }
    }

    #[inline]
    pub fn voxel_index(x: usize, y: usize, z: usize) -> usize {
        x + y * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE
    }

    #[inline]
    pub fn get_voxel(&self, x: usize, y: usize, z: usize) -> &Voxel {
        &self.voxels[Self::voxel_index(x, y, z)]
    }

    #[inline]
    pub fn get_voxel_mut(&mut self, x: usize, y: usize, z: usize) -> &mut Voxel {
        &mut self.voxels[Self::voxel_index(x, y, z)]
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

/// A self-contained, owned copy of all voxel data needed to mesh one chunk.
/// Contains CHUNK_SIZE+2 voxels in each dimension: the chunk's own 32^3 voxels
/// plus a 1-voxel-deep border from neighbors. This allows the DC algorithm to
/// compute density gradients and sample corner densities at chunk boundaries
/// without referencing the world.
///
/// Coordinate convention: `get_voxel(x, y, z)` uses chunk-local coordinates
/// where x in -1..=CHUNK_SIZE. Internally stored at offset +1, so snapshot
/// index = (x+1, y+1, z+1).
pub struct ChunkSnapshot {
    pub position: IVec3,
    pub voxels: Box<[Voxel; SNAP_VOLUME]>,
}

impl ChunkSnapshot {
    /// Create a snapshot by copying voxels from the chunk and its neighbors.
    pub fn extract(chunk: &Chunk, neighbors: &ChunkNeighbors) -> Self {
        let mut voxels: Box<[Voxel; SNAP_VOLUME]> = unsafe {
            let v: Vec<Voxel> = vec![Voxel::default(); SNAP_VOLUME];
            let boxed_slice = v.into_boxed_slice();
            Box::from_raw(Box::into_raw(boxed_slice) as *mut [Voxel; SNAP_VOLUME])
        };

        // Fill the interior: chunk's own voxels at snapshot coords [1..CHUNK_SIZE+1)
        for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let si = Self::snap_index(x + 1, y + 1, z + 1);
                    voxels[si] = chunk.voxels[Chunk::voxel_index(x, y, z)];
                }
            }
        }

        // Fill border voxels (cells where at least one coord is 0 or SNAP_SIZE-1)
        for sz in 0..SNAP_SIZE {
            for sy in 0..SNAP_SIZE {
                for sx in 0..SNAP_SIZE {
                    // Skip interior (already filled)
                    if sx >= 1 && sx <= CHUNK_SIZE
                        && sy >= 1 && sy <= CHUNK_SIZE
                        && sz >= 1 && sz <= CHUNK_SIZE
                    {
                        continue;
                    }

                    // Convert snapshot coord to chunk-local coord
                    let cx = sx as i32 - 1;
                    let cy = sy as i32 - 1;
                    let cz = sz as i32 - 1;

                    voxels[Self::snap_index(sx, sy, sz)] =
                        resolve_voxel_copy(chunk, neighbors, cx, cy, cz);
                }
            }
        }

        Self {
            position: chunk.position,
            voxels,
        }
    }

    #[inline]
    pub fn snap_index(sx: usize, sy: usize, sz: usize) -> usize {
        sx + sy * SNAP_SIZE + sz * SNAP_SIZE * SNAP_SIZE
    }

    /// Sample a voxel at chunk-local coordinates where x in -1..=CHUNK_SIZE.
    #[inline]
    pub fn get_voxel(&self, x: i32, y: i32, z: i32) -> &Voxel {
        let sx = (x + 1) as usize;
        let sy = (y + 1) as usize;
        let sz = (z + 1) as usize;
        debug_assert!(
            sx < SNAP_SIZE && sy < SNAP_SIZE && sz < SNAP_SIZE,
            "ChunkSnapshot::get_voxel out of range: ({}, {}, {})", x, y, z
        );
        &self.voxels[Self::snap_index(sx, sy, sz)]
    }
}

/// Resolve a voxel at chunk-local coords, copying from chunk or neighbors.
/// Returns a default (air) voxel if the neighbor is missing.
fn resolve_voxel_copy(chunk: &Chunk, neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> Voxel {
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
        return *chunk.get_voxel(lx, ly, lz);
    }

    match neighbors.get(dx, dy, dz) {
        Some(neighbor) => *neighbor.get_voxel(lx, ly, lz),
        None => Voxel::default(),
    }
}