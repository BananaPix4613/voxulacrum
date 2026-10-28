//! Wood imposter instance buffers (spec §5).
//!
//! Each chunk owns the branch capsules rooted in it; `meshing::foliage` turns
//! them into instances and this keeps one instance buffer per chunk, replaced
//! when the chunk is (re)meshed and dropped on unload. Mirrors `scatter_pass` -
//! same lifetime, same per-chunk buffer shape - minus the per-prefab grouping,
//! because every capsule uses the same analytic "mesh".

use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::CapsuleInstanceGpu;
use voxel_core::ChunkVersion;

/// One chunk's capsule instances.
pub struct ChunkCapsules {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
    /// The chunk version these instances were built from.
    pub version: ChunkVersion,
}

#[derive(Resource, Default)]
pub struct CapsulePass {
    pub chunks: HashMap<IVec3, ChunkCapsules>,
}

impl CapsulePass {
    /// Replace `pos`'s instance buffer with `instances`, built from the chunk at
    /// `version`, or drop it when the chunk owns no capsules (`None`).
    pub fn set(
        &mut self,
        pos: IVec3,
        version: ChunkVersion,
        instances: Option<&[CapsuleInstanceGpu]>,
        device: &wgpu::Device,
    ) {
        debug_assert!(
            self.chunks.get(&pos).map_or(true, |old| old.version <= version),
            "capsules for chunk {pos} were built from an older version than the ones they replace",
        );
        let Some(instances) = instances else {
            self.chunks.remove(&pos);
            return;
        };
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("capsule_instance_buffer_chunk"),
            contents: bytemuck::cast_slice(instances),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.chunks.insert(
            pos,
            ChunkCapsules { instance_buffer, instance_count: instances.len() as u32, version },
        );
    }

    /// Drop a chunk's instances on unload.
    pub fn remove(&mut self, pos: IVec3) {
        self.chunks.remove(&pos);
    }
}
