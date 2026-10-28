//! Canopy lobe instance buffers (spec §6.2).
//!
//! A direct sibling of `capsule_pass`: each chunk owns the lobes centered in
//! it, which `meshing::foliage` turns into instances; this keeps one instance
//! buffer per chunk, replaced on (re)mesh and dropped on unload. Separate from
//! the capsules only because the primitive differs - an ellipsoid and a round
//! cone need different solves, so they cannot share a draw.

use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::LobeInstanceGpu;
use voxel_core::ChunkVersion;

/// One chunk's lobe instances.
pub struct ChunkLobes {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
    /// The chunk version these instances were built from.
    pub version: ChunkVersion,
}

#[derive(Resource, Default)]
pub struct CanopyPass {
    pub chunks: HashMap<IVec3, ChunkLobes>,
}

impl CanopyPass {
    /// Replace `pos`'s instance buffer with `instances`, built from the chunk at
    /// `version`, or drop it when the chunk owns no lobes (`None`).
    pub fn set(
        &mut self,
        pos: IVec3,
        version: ChunkVersion,
        instances: Option<&[LobeInstanceGpu]>,
        device: &wgpu::Device,
    ) {
        debug_assert!(
            self.chunks.get(&pos).map_or(true, |old| old.version <= version),
            "lobes for chunk {pos} were built from an older version than the ones they replace",
        );
        let Some(instances) = instances else {
            self.chunks.remove(&pos);
            return;
        };
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("canopy_lobe_instance_buffer_chunk"),
            contents: bytemuck::cast_slice(instances),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.chunks.insert(
            pos,
            ChunkLobes { instance_buffer, instance_count: instances.len() as u32, version },
        );
    }

    /// Drop a chunk's instances on unload.
    pub fn remove(&mut self, pos: IVec3) {
        self.chunks.remove(&pos);
    }
}
