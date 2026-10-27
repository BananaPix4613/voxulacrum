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

/// One chunk's capsule instances.
pub struct ChunkCapsules {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
}

#[derive(Resource, Default)]
pub struct CapsulePass {
    pub chunks: HashMap<IVec3, ChunkCapsules>,
}

impl CapsulePass {
    /// Replace `pos`'s instance buffer with `instances`, or drop it when the
    /// chunk owns no capsules (`None`).
    pub fn set(
        &mut self,
        pos: IVec3,
        instances: Option<&[CapsuleInstanceGpu]>,
        device: &wgpu::Device,
    ) {
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
            ChunkCapsules { instance_buffer, instance_count: instances.len() as u32 },
        );
    }

    /// Drop a chunk's instances on unload.
    pub fn remove(&mut self, pos: IVec3) {
        self.chunks.remove(&pos);
    }
}
