//! The terrain meshes of resident chunks, on the GPU.

use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use voxel_core::ChunkVersion;
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::FaceVertex;

/// One chunk's terrain mesh on the GPU.
pub struct ChunkMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    /// Vertex + index buffer bytes. Recorded at upload rather than inferred from
    /// `index_count` and quad topology, so the residency panel reports the real
    /// GPU footprint instead of an estimate.
    pub gpu_bytes: u64,
    /// The chunk version this mesh was built from.
    pub version: ChunkVersion,
}

/// One chunk's terrain mesh as the mesher produced it, queued for upload.
pub struct TerrainMesh {
    pub vertices: Vec<FaceVertex>,
    pub indices: Vec<u32>,
}

/// The terrain meshes of resident chunks, keyed by chunk coordinate. Client
/// state: the world holds no mesh (R4). Filled and emptied from `ChunkUploads`
/// by `chunk_upload_system`. A render-world resource.
#[derive(Resource, Default)]
pub struct ChunkMeshes {
    pub meshes: HashMap<IVec3, ChunkMesh>,
}

impl ChunkMeshes {
    /// Upload `mesh`, built from the chunk at `version`, replacing the old one.
    /// An empty or absent mesh removes the entry, since the chunk has nothing
    /// to draw.
    pub fn set(
        &mut self,
        pos: IVec3,
        version: ChunkVersion,
        mesh: Option<&TerrainMesh>,
        device: &wgpu::Device,
    ) {
        debug_assert!(
            self.meshes.get(&pos).map_or(true, |old| old.version <= version),
            "terrain for chunk {pos} was built from an older version than the one it replaces",
        );
        let Some(TerrainMesh { vertices, indices }) = mesh else {
            self.meshes.remove(&pos);
            return;
        };
        if vertices.is_empty() || indices.is_empty() {
            self.meshes.remove(&pos);
            return;
        }

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk_vertex_buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk_index_buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        self.meshes.insert(
            pos,
            ChunkMesh {
                vertex_buffer,
                index_buffer,
                index_count: indices.len() as u32,
                gpu_bytes: (size_of_val(vertices.as_slice()) + size_of_val(indices.as_slice())) as u64,
                version,
            },
        );
    }

    /// Drop the mesh of an unloaded chunk. GPU buffers are freed on drop.
    pub fn remove(&mut self, pos: IVec3) {
        self.meshes.remove(&pos);
    }

    /// Vertex + index bytes across every mesh.
    pub fn gpu_bytes(&self) -> u64 {
        self.meshes.values().map(|m| m.gpu_bytes).sum()
    }
}
