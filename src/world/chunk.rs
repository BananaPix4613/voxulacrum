use glam::IVec3;
use super::voxel::Voxel;

pub const CHUNK_SIZE: usize = 32;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

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
    pub pos_x: Option<&'a Chunk>,
    pub neg_x: Option<&'a Chunk>,
    pub pos_y: Option<&'a Chunk>,
    pub neg_y: Option<&'a Chunk>,
    pub pos_z: Option<&'a Chunk>,
    pub neg_z: Option<&'a Chunk>,
}