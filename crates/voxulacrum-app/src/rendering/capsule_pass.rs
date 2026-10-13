//! Wood imposter instance buffers (spec §5).
//!
//! Each chunk owns the branch capsules rooted in it; this turns them into one
//! instance buffer per chunk, rebuilt when the chunk is (re)meshed and dropped
//! on unload. Mirrors `scatter_pass` - same lifetime, same per-chunk buffer
//! shape - minus the per-prefab grouping, because every capsule uses the same
//! analytic "mesh".

use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::CapsuleInstanceGpu;
use crate::rendering::render_context::RenderContext;
use crate::world::chunk::{LoadedChunk, VOXEL_SCALE};
use crate::world::World;

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
    /// Full rebuild - scans every loaded chunk. Initial load and regen.
    pub fn new(ctx: &RenderContext, world: &World) -> Self {
        let mut pass = Self::default();
        for &coord in world.chunks.keys() {
            let pos = IVec3::from(coord);
            pass.add_chunk(pos, world, &ctx.device);
        }
        log::info!("Capsules: built instances for {} chunks", pass.chunks.len());
        pass
    }

    /// Build or refresh one chunk's instance buffer.
    pub fn add_chunk(&mut self, pos: IVec3, world: &World, device: &wgpu::Device) {
        let Some(chunk) = world.get_chunk(pos) else { return };
        match build(chunk, device) {
            Some(c) => {
                self.chunks.insert(pos, c);
            }
            None => {
                self.chunks.remove(&pos);
            }
        }
    }

    /// Drop a chunk's instances on unload.
    pub fn remove_chunk(&mut self, pos: IVec3) {
        self.chunks.remove(&pos);
    }
}

/// `None` when the chunk owns no capsules, so an empty chunk costs no buffer.
fn build(chunk: &LoadedChunk, device: &wgpu::Device) -> Option<ChunkCapsules> {
    if chunk.segments.is_empty() {
        return None;
    }
    let instances: Vec<CapsuleInstanceGpu> = chunk
        .segments
        .iter()
        .map(|s| CapsuleInstanceGpu {
            // Segments are in voxel units; the render world scales by
            // VOXEL_SCALE, exactly as the scatter pass does for its anchors.
            a: (s.a * VOXEL_SCALE).to_array(),
            ra: s.ra * VOXEL_SCALE,
            b: (s.b * VOXEL_SCALE).to_array(),
            rb: s.rb * VOXEL_SCALE,
        })
        .collect();

    let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("capsule_instance_buffer_chunk"),
        contents: bytemuck::cast_slice(&instances),
        usage: wgpu::BufferUsages::VERTEX,
    });
    Some(ChunkCapsules { instance_buffer, instance_count: instances.len() as u32 })
}
